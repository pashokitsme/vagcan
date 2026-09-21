//! Choosing what to talk to the car through: a USB-CAN adapter, or the dash board on its
//! cable or over BLE.
//!
//! A person running this on a car has exactly one adapter plugged in, so
//! requiring them to paste a `/dev/cu.usbmodem…` path every time is friction
//! for nothing. `--device` stays available for the ambiguous cases; when it is
//! omitted we pick the obvious candidate and say which one we picked.
//!
//! **The dash board over BLE is the other way in** (`.archive/tasks/done/dash/16-uds-over-ble.md`,
//! "Choosing the device" and "Zero friction"). `--device ble` scans for it, takes the
//! one board heard and says so, and offers a menu when there are several; `--device
//! ble:<name>` picks one by name without asking. **Nothing else ever scans** (owner,
//! 2026-09-14): with no `--device` the choice is USB only, and "no USB-CAN adapter found"
//! ends with [`BLE_HINT`]. A scan started from an app macOS does not allow Bluetooth gets
//! the process killed, and it prompts somebody who has no board at all. A command the BLE
//! link cannot carry resolves a cable only ([`resolve_cable_for`]).
//!
//! **The dash board on its USB cable is the third** (`todo/dash/14-one-bus-three-clients.md`
//! §3). Its ids are every ESP32's, so each such port is asked: a board that answers Hello
//! runs the `dash` image and is read through, as over BLE, with its panel still running
//! ([`Target::BoardUsb`]); one that answers slcan's `V` runs the `slcan` image and is an
//! adapter. Either is a recognised adapter in the choice. `--slcan` makes a `dash` board a
//! plain adapter too, and changes nothing else.

/// The rate every slcan adapter on this project's bench runs at.
///
/// In `core` because it is a property of the cable rather than of any command,
/// and three crates open that cable now.
pub const ADAPTER_BAUD: u32 = 115_200;

/// The longest a BLE scan for boards listens: `dashcfg`'s figure. A scan that hears a
/// board stops [`vag_dash_ble::SCAN_SETTLE`] after it, so only a scan that hears nothing
/// takes this long.
pub const BLE_SCAN: std::time::Duration = std::time::Duration::from_secs(4);

use anyhow::{Context as _, Result, bail};
use vag_uds_can::{
	AdapterInfo, BOARD_PROBE_WAIT, BoardAnswer, CanError, HANDSHAKE_WAIT, SerialPipe, SerialSlcan, SlcanBackend, SlcanBitrate, SlcanMode,
	list_adapters, probe_board,
};

use crate::bus::{Budget, Bus, Carrier};
use crate::ui::menu::{Asker, Item};

/// How a vag-dash board reads in the listing, by what it answered.
const BOARD_DASH: &str = "vag-dash board — dash image (reads through the board, panel keeps running)";
const BOARD_SLCAN: &str = "vag-dash board — slcan image";
const BOARD_SILENT: &str = "vag-dash board — answers neither Hello nor slcan (an older dash image? reflash `dash` or `slcan`)";

/// Why `--slcan` is refused with `--device ble…`.
pub const SLCAN_OVER_BLE: &str =
	"--slcan makes the board's USB cable a plain adapter; it has no meaning over BLE — drop --slcan, or name the board's serial path";

/// How to reach an adapter over Bluetooth, said wherever no USB device was found:
/// nothing looks for it unasked.
pub const BLE_HINT: &str = "An adapter over Bluetooth: --device ble";

/// [`BLE_HINT`] where no car command is running — `vagcan devices` and the bare `vagcan`,
/// which take no `--device` — so it names a command that does.
pub const BLE_COMMAND_HINT: &str = "An adapter over Bluetooth: vagcan info --device ble";

/// Why no board was heard, as far as this side can tell.
const NO_BOARD: &str = "an adapter on the OBD port is powered by it, so the ignition must be on, and it must be in range of this computer.";

/// What a command talks to the car through, once `--device` is resolved.
pub enum Target<H = BleHandle> {
	/// A USB-CAN adapter at this serial path — or the board on its `slcan` image, or on its
	/// `dash` image made a plain adapter by `--slcan`.
	Serial(String),
	/// The dash board on its USB cable, on its `dash` image: read through the framed link,
	/// as over BLE, with the panel still running.
	BoardUsb { path: String },
	/// The dash board over BLE.
	Ble(Board<H>),
}

/// A dash board a BLE scan heard offering its UART.
pub struct Board<H = BleHandle> {
	/// The name it advertised.
	pub name: String,
	/// What the platform calls it — on macOS a per-host UUID — which tells two boards of
	/// one name apart.
	pub id: String,
	pub rssi: Option<i16>,
	/// What connecting to it takes.
	pub handle: H,
}

/// What connecting to a board a scan heard takes.
pub struct BleHandle {
	adapter: vag_dash_ble::Adapter,
	peripheral: vag_dash_ble::Peripheral,
}

/// Whether `--device` names the board over BLE: `ble` is `Some(None)`, `ble:<name>`
/// is `Some(Some(name))`, and anything else — a path — is `None`.
pub fn ble_request(requested: &str) -> Option<Option<&str>> {
	match requested.strip_prefix("ble")? {
		"" => Some(None),
		rest => rest.strip_prefix(':').map(Some),
	}
}

/// Resolve what to talk to the car through (module docs).
///
/// A path is a USB device: an slcan adapter, or the dash board on its cable, checked as
/// [`named_usb`] checks one. `ble` and `ble:<name>` scan for the dash board. Nothing at all
/// runs the USB choice and nothing else: when it finds no adapter, the refusal ends with
/// [`BLE_HINT`] rather than a scan.
pub async fn resolve(requested: Option<&str>, slcan: bool) -> Result<Target> {
	// Never shown: with nobody at the keyboard the choice refuses before the menu is asked.
	let mut console = crate::ui::Console::new("--device ble:<name>");
	resolve_with(
		requested,
		slcan,
		list_adapters().map_err(Into::into),
		probe,
		scan_ble,
		crate::ui::can_ask(),
		&mut console,
	)
	.await
}

/// [`resolve`] with the serial listing, the board probe, the BLE scan, whether anybody
/// is at the keyboard and the menu handed in, so every choice can be tested without a
/// port or a radio.
pub async fn resolve_with<H>(
	requested: Option<&str>,
	slcan: bool,
	listing: Result<Vec<AdapterInfo>>,
	probe: impl FnMut(&str) -> BoardAnswer,
	scan: impl AsyncFnOnce() -> Result<Vec<Board<H>>>,
	can_ask: bool,
	asker: &mut impl Asker,
) -> Result<Target<H>> {
	if let Some(requested) = requested {
		return match ble_request(requested) {
			Some(_) if slcan => bail!("{SLCAN_OVER_BLE}"),
			Some(name) => {
				eprintln!("looking for an adapter over BLE…");
				// The adapter's own error as it came: on macOS it says where access is allowed.
				let boards = scan().await?;
				choose_board(boards, name, can_ask, asker).map(Target::Ble)
			}
			None => named_usb(requested, listing, probe).map(|usb| usb.target(slcan)),
		};
	}
	let no_cable = match choose_usb(listing, probe, slcan, true)? {
		Found::Picked(usb) => return Ok(usb.target(slcan)),
		Found::Nothing(why) => why,
		// A dash board is always a candidate here, so it is never the one left out.
		Found::OnlyThroughTheBoard => unreachable!("a dash board is a candidate when it may be read through"),
	};
	// No scan: Bluetooth is looked at only when `--device ble…` asks (module docs).
	match slcan {
		true => bail!("{no_cable}\n--slcan asks for a USB adapter."),
		false => bail!("{no_cable}\n{BLE_HINT}"),
	}
}

/// The one board heard, the one named, or the one chosen from a menu; refused when
/// there are several and nobody to choose.
fn choose_board<H>(mut boards: Vec<Board<H>>, name: Option<&str>, can_ask: bool, asker: &mut impl Asker) -> Result<Board<H>> {
	if let Some(name) = name {
		if name.is_empty() {
			bail!("`--device ble:` needs an adapter's name after the colon, e.g. `--device ble:vagcan-dash`");
		}
		let heard = choices(&boards);
		boards.retain(|b| b.name == name || b.id == name);
		match boards.len() {
			0 if heard.is_empty() => bail!("no adapter named {name} answered over BLE: {NO_BOARD}"),
			0 => bail!("no adapter named {name} answered over BLE. Heard:\n{heard}"),
			1 => {}
			_ => bail!("several adapters are named {name} — name one by its id:\n{}", choices(&boards)),
		}
	}
	let at = match boards.len() {
		0 => bail!("no adapter found over BLE: {NO_BOARD}"),
		1 => 0,
		_ if !can_ask => bail!("several adapters answered over BLE — say which one:\n{}", choices(&boards)),
		_ => {
			let details: Vec<String> = boards.iter().map(signal).collect();
			let items: Vec<Item<'_>> = boards
				.iter()
				.zip(&details)
				.map(|(board, detail)| Item { label: &board.name, detail })
				.collect();
			match asker.ask("Which adapter?", &items, 0)? {
				Some(at) => at,
				None => bail!("no adapter chosen"),
			}
		}
	};
	let board = boards.into_iter().nth(at).context("the menu answered a row it did not show")?;
	eprintln!("using {} over BLE", board.name);
	Ok(board)
}

/// One `--device` line per board: by name, or by id where two boards share a name.
fn choices<H>(boards: &[Board<H>]) -> String {
	boards
		.iter()
		.map(|board| {
			let shared = boards.iter().filter(|other| other.name == board.name).count() > 1;
			let by = if shared { &board.id } else { &board.name };
			format!("  --device ble:{by}   {}", signal(board))
		})
		.collect::<Vec<_>>()
		.join("\n")
}

fn signal<H>(board: &Board<H>) -> String {
	match board.rssi {
		Some(rssi) => format!("{rssi} dBm, id {}", board.id),
		None => format!("id {}", board.id),
	}
}

/// The boards in range, over this computer's first Bluetooth adapter.
async fn scan_ble() -> Result<Vec<Board>> {
	let adapter = vag_dash_ble::adapter().await?;
	let found = vag_dash_ble::scan_boards(&adapter, BLE_SCAN).await?;
	Ok(
		found
			.into_iter()
			.map(|found| Board {
				name: found.label(),
				id: found.id(),
				rssi: found.rssi,
				handle: BleHandle {
					adapter: adapter.clone(),
					peripheral: found.peripheral,
				},
			})
			.collect(),
	)
}

fn probe(path: &str) -> BoardAnswer {
	probe_board(path, ADAPTER_BAUD, BOARD_PROBE_WAIT)
}

/// A USB device the choice settled on, before `--slcan` has had its say.
enum Usb {
	/// Something that speaks slcan: a CAN adapter, or the board on its `slcan` image.
	Adapter(String),
	/// The board on its `dash` image: it answered Hello.
	Dash(String),
}

impl Usb {
	/// `--slcan` makes a `dash` board a plain adapter: the slcan channel-open sequence
	/// switches the image into its adapter mode (`vag_uds_client::console`).
	fn target<H>(self, slcan: bool) -> Target<H> {
		match self {
			Usb::Adapter(path) => Target::Serial(path),
			Usb::Dash(path) if slcan => Target::Serial(path),
			Usb::Dash(path) => Target::BoardUsb { path },
		}
	}
}

/// What the USB choice came to with no `--device`.
enum Found {
	Picked(Usb),
	/// No USB-CAN adapter at all, and why, in full.
	Nothing(String),
	/// Nothing to use but a dash board, on a path that does not go through the board.
	OnlyThroughTheBoard,
}

/// Why a command that does not go through the dash board is refused, on each way to it.
pub struct NotThroughTheBoard<'a> {
	/// With `--device ble…`; also added under "no USB-CAN adapter found".
	pub over_ble: &'a str,
	/// With the board on its cable, on its `dash` image, and no `--slcan`.
	pub over_usb: &'a str,
}

/// Resolve a serial adapter, for a command that does not go through the dash board.
///
/// Refused before a session starts: `--device ble…` with `why.over_ble` ([`SLCAN_OVER_BLE`]
/// with `slcan`), and the board on its `dash` image with `why.over_usb` — unless `slcan`
/// made it a plain adapter, where the cable's own guards apply. Otherwise a USB device is
/// resolved as [`resolve_with`] resolves one, with `why.over_ble` added when none is found,
/// so nobody is left wondering why the board over BLE was not tried.
pub fn resolve_cable_for(requested: Option<&str>, slcan: bool, why: &NotThroughTheBoard) -> Result<String> {
	resolve_cable_for_with(requested, slcan, why, list_adapters().map_err(Into::into), probe)
}

/// [`resolve_cable_for`] with the listing and the board probe handed in.
pub fn resolve_cable_for_with(
	requested: Option<&str>,
	slcan: bool,
	why: &NotThroughTheBoard,
	listing: Result<Vec<AdapterInfo>>,
	probe: impl FnMut(&str) -> BoardAnswer,
) -> Result<String> {
	if requested.is_some_and(|r| ble_request(r).is_some()) {
		bail!("{}", if slcan { SLCAN_OVER_BLE } else { why.over_ble });
	}
	let usb = match requested {
		Some(path) => named_usb(path, listing, probe)?,
		// A dash board is a candidate only as `--slcan`'s adapter: without it the board is
		// refused here, so it must not stand in the way of another device either.
		None => match choose_usb(listing, probe, slcan, slcan)? {
			Found::Picked(usb) => usb,
			Found::Nothing(no_cable) if slcan => bail!("{no_cable}"),
			Found::Nothing(no_cable) => bail!("{no_cable}\n{}", why.over_ble),
			Found::OnlyThroughTheBoard => bail!("{}", why.over_usb),
		},
	};
	match usb.target::<()>(slcan) {
		Target::Serial(path) => Ok(path),
		Target::BoardUsb { .. } | Target::Ble(_) => bail!("{}", why.over_usb),
	}
}

/// The USB device `--device` names.
///
/// That path is used — no guessing behind the user's back — with one check: a vag-dash
/// board that answers neither Hello nor slcan is refused at once, because opening it would
/// only time out and blame the car.
///
/// A vag-dash board ([`vag_uds_can::BOARD_USB`]) is asked because its ids are every ESP32's,
/// and it runs the display image or the adapter image under them. Nothing else is ever
/// asked anything.
fn named_usb(path: &str, listing: Result<Vec<AdapterInfo>>, mut probe: impl FnMut(&str) -> BoardAnswer) -> Result<Usb> {
	// A listing that fails says nothing about the path, so the path is
	// honoured as given; only a path the listing names as a board is asked.
	let board = listing
		.ok()
		.and_then(|found| found.into_iter().find(|a| a.board && same_node(&a.path, path)));
	let Some(board) = board else {
		return Ok(Usb::Adapter(path.to_string()));
	};
	// Asked on the listed `cu.*` node, which is the one to open.
	match probe(&board.path) {
		BoardAnswer::Slcan => Ok(Usb::Adapter(path.to_string())),
		BoardAnswer::Dash { .. } => Ok(Usb::Dash(path.to_string())),
		answer => bail!("{}", not_an_adapter(path, &answer)),
	}
}

/// The USB choice with no `--device`.
///
/// Exactly one candidate is used automatically, and said; several is an error that lists
/// what was found, because silently picking one of two adapters is how you end up talking
/// to the wrong bus. "No USB-CAN adapter at all" is an answer rather than an error, because
/// [`resolve_with`] looks for the board over BLE next. `slcan` only changes what is said
/// about a `dash` board picked.
///
/// `dash_candidate`: whether a board on its `dash` image may be picked at all. A command
/// that does not go through the board passes `false` without `--slcan`: such a board is
/// then no candidate — beside a CANable the CANable is picked, beside an unrecognised
/// device that device is — and alone it is [`Found::OnlyThroughTheBoard`].
fn choose_usb(listing: Result<Vec<AdapterInfo>>, mut probe: impl FnMut(&str) -> BoardAnswer, slcan: bool, dash_candidate: bool) -> Result<Found> {
	// Which boards would not open, kept apart from the silent ones: a busy port may well
	// be a working board, and that wants different advice. Which answered Hello, because
	// those are read through rather than opened as adapters.
	let mut unopened: Vec<String> = Vec::new();
	let mut dash: Vec<String> = Vec::new();
	let found = probed(listing?, |path| {
		let answer = probe(path);
		match answer {
			BoardAnswer::Unopened(_) => unopened.push(path.to_string()),
			BoardAnswer::Dash { .. } => dash.push(path.to_string()),
			BoardAnswer::Slcan | BoardAnswer::Silent => {}
		}
		answer
	});

	// A recognised CAN adapter wins outright. Someone with a CANable plugged in
	// next to an Arduino means the CANable, and making them spell that out
	// every time is friction for nothing. A board that answered Hello or `V` is one.
	let known: Vec<&AdapterInfo> = found.iter().filter(|a| a.known && (dash_candidate || !dash.contains(&a.path))).collect();
	match known.as_slice() {
		[only] if dash.contains(&only.path) => {
			let how = match slcan {
				true => "vag-dash board — dash image, a plain adapter by --slcan",
				false => only.description.as_str(),
			};
			eprintln!("using {} ({how})", only.path);
			return Ok(Found::Picked(Usb::Dash(only.path.clone())));
		}
		[only] => {
			eprintln!("using {} ({})", only.path, only.description);
			return Ok(Found::Picked(Usb::Adapter(only.path.clone())));
		}
		[] => {}
		several => bail!("several CAN adapters found — say which one:\n{}", device_lines(several.iter().copied())),
	}

	// No recognised adapter. A board that did not answer is known *not* to be
	// one, so it is no candidate — but it is named, since it is the likeliest
	// thing somebody plugged in meaning to use.
	let (silent, others): (Vec<&AdapterInfo>, Vec<&AdapterInfo>) = found.iter().partition(|a| a.board);
	match (others.as_slice(), silent.as_slice()) {
		([], []) => Ok(Found::Nothing(
			"no USB-CAN adapter found.\n\
             Plug one in and check it enumerated: `vagcan devices`.\n\
             If it is plugged in but missing, unplug and replug it — the adapter can \
             enumerate on USB without macOS attaching a serial node."
				.to_string(),
		)),
		([], boards) if boards.iter().any(|b| dash.contains(&b.path)) => Ok(Found::OnlyThroughTheBoard),
		([], boards) => {
			let (busy, silent): (Vec<&AdapterInfo>, Vec<&AdapterInfo>) = boards.iter().partition(|b| unopened.contains(&b.path));
			let lines = |group: &[&AdapterInfo]| {
				group
					.iter()
					.map(|b| format!("  {}   {}", b.path, b.description))
					.collect::<Vec<_>>()
					.join("\n")
			};
			let mut why = String::from("no USB-CAN adapter found — only a vag-dash board, and it cannot be used:");
			if !busy.is_empty() {
				why.push_str(&format!(
					"\n{}\nIt could not be opened, so nobody knows which firmware it runs. Another program \
                     (a monitor, `dashsim`, another vagcan) may be holding the port — close it and try again.",
					lines(&busy)
				));
			}
			if !silent.is_empty() {
				why.push_str(&format!(
					"\n{}\nIt answers neither Hello nor slcan — likely an older `dash` image. Reflash its `dash` or \
                     `slcan` image (research/dash/can-bring-up.md §9.2).",
					lines(&silent)
				));
			}
			Ok(Found::Nothing(format!("{why}\nOr plug in a CAN adapter.")))
		}
		([only], _) => {
			for board in &silent {
				eprintln!("not using {} ({})", board.path, board.description);
			}
			eprintln!("using {} ({})", only.path, only.description);
			Ok(Found::Picked(Usb::Adapter(only.path.clone())))
		}
		(_, _) => bail!("several serial devices found — say which one:\n{}", device_lines(found.iter())),
	}
}

fn device_lines<'a>(found: impl Iterator<Item = &'a AdapterInfo>) -> String {
	found
		.map(|a| format!("  --device {}   {}", a.path, a.description))
		.collect::<Vec<_>>()
		.join("\n")
}

/// macOS gives one device a `tty.*` and a `cu.*` node; the listing keeps `cu.*`.
fn same_node(listed: &str, given: &str) -> bool {
	listed == given || listed == given.replacen("/dev/tty.", "/dev/cu.", 1)
}

/// Why an explicitly named board will not be opened.
fn not_an_adapter(path: &str, answer: &BoardAnswer) -> String {
	match answer {
		BoardAnswer::Unopened(why) => format!(
			"{path} is a vag-dash board, and it could not be opened to ask which firmware it runs: {why}\n\
             Another program (a monitor, `dashsim`, another vagcan) may be holding the port."
		),
		_ => format!(
			"{path} is a vag-dash board that answers neither Hello nor slcan — likely an older `dash` image, \
             and a car command on it would only time out.\n\
             Reflash its `dash` or `slcan` image (research/dash/can-bring-up.md §9.2), name a CAN adapter, \
             or use the board over BLE: `--device ble`."
		),
	}
}

/// Every candidate adapter, for `vagcan devices` — each vag-dash board asked
/// which image it runs first, so the list says.
pub fn list() -> Result<Vec<AdapterInfo>> {
	Ok(probed(list_adapters()?, probe))
}

/// The listing with every board's answer folded in: a board that answered Hello or `V`
/// is a recognised adapter, and every board's description says what it answered.
/// Only boards are probed. Recognised adapters sort first again afterwards.
pub fn probed(mut found: Vec<AdapterInfo>, mut probe: impl FnMut(&str) -> BoardAnswer) -> Vec<AdapterInfo> {
	for adapter in found.iter_mut().filter(|a| a.board) {
		let answer = probe(&adapter.path);
		adapter.known = matches!(answer, BoardAnswer::Slcan | BoardAnswer::Dash { .. });
		adapter.description = match answer {
			BoardAnswer::Slcan => BOARD_SLCAN.to_string(),
			BoardAnswer::Dash { version } => format!("{BOARD_DASH}, {version}"),
			BoardAnswer::Silent => BOARD_SILENT.to_string(),
			BoardAnswer::Unopened(why) => format!("vag-dash board — could not be opened to ask its firmware ({why})"),
		};
	}
	found.sort_by(|a, b| b.known.cmp(&a.known).then_with(|| a.path.cmp(&b.path)));
	found
}

/// Render the device list for a human.
///
/// Serial devices only, then [`BLE_COMMAND_HINT`]: `devices` never scans Bluetooth.
pub fn render_list(found: &[AdapterInfo]) -> String {
	format!("{}\n\n{BLE_COMMAND_HINT}", serial_list(found))
}

fn serial_list(found: &[AdapterInfo]) -> String {
	if found.is_empty() {
		return "No serial devices found.\n\n\
                If your adapter is plugged in, unplug and replug it: it can enumerate on USB \
                without macOS attaching a serial node, and then there is nothing to open."
			.to_string();
	}
	let mut out = String::from("Serial devices:\n\n");
	for a in found {
		let mark = if a.known { "*" } else { " " };
		out.push_str(&format!("{mark} {}\n    {}\n", a.path, a.description));
	}
	out.push_str("\n* = recognised CAN adapter. Pass one with --device, or omit --device when\n");
	out.push_str("  only one is connected. --slcan makes a dash board a plain adapter.");
	out
}

/// Open the adapter, saying what to do when it will not open.
///
/// **Every command that touches the car opened it with these same three lines**
/// — eight of them, each naming the bitrate and the mode and each appending the
/// same context — and the bitrate is not a preference anybody was choosing
/// between: VW's diagnostic CAN is 500 kbit/s (ISO 15765-4), so a site free to
/// spell it differently is a site free to spell it wrong.
///
/// `mode` is the one thing that genuinely varies: `dev sniff` opens `Silent` by
/// default, which is the whole point of it (`--active` asks for `Normal`), and
/// everything else is `Normal`.
pub async fn open(path: &str, baud: u32, mode: SlcanMode) -> Result<SerialSlcan> {
	SlcanBackend::open_mode(path, baud, SlcanBitrate::Rate500k, mode)
		.await
		.with_context(|| open_failure(path))
}

/// Open what `target` names and hand it to a bus: what every car command talks to the
/// car through.
///
/// A cable is opened and given to the scheduler with its default budget. The dash board,
/// on its cable or over BLE, is connected and given to [`Bus::start_remote`], because it
/// runs the scheduler itself; on its cable a session starts with a Hello
/// ([`vag_uds_can::StreamPipe::handshake`]). `dev sniff` is the one command that opens a
/// cable without this, because it reads frames, not answers.
pub async fn open_bus(target: &Target) -> Result<Bus> {
	match target {
		Target::Serial(path) => {
			let adapter = open(path, ADAPTER_BAUD, SlcanMode::Normal).await?;
			Ok(Bus::start(adapter, Budget::default()))
		}
		Target::BoardUsb { path } => {
			let (pipe, _) = SerialPipe::open_board(path, ADAPTER_BAUD).await.map_err(|failed| match failed {
				CanError::Timeout => anyhow::anyhow!(
					"the dash board on {path} did not answer Hello within {} ms — run `vagcan devices` to see what it is running",
					HANDSHAKE_WAIT.as_millis()
				),
				failed => anyhow::Error::new(failed).context(open_failure(path)),
			})?;
			Ok(Bus::start_remote(pipe, &format!("the dash board on {path}"), Carrier::Usb))
		}
		Target::Ble(board) => {
			let pipe = vag_dash_ble::NusPipe::connect(&board.handle.adapter, &board.handle.peripheral)
				.await
				.with_context(|| format!("connecting to {} over BLE", board.name))?;
			Ok(Bus::start_remote(pipe, &board.name, Carrier::Ble))
		}
	}
}

/// [`resolve`] and [`open_bus`] in one, for a command whose `--device` and `--slcan` are
/// all it needs.
pub async fn connect(requested: Option<&str>, slcan: bool) -> Result<Bus> {
	open_bus(&resolve(requested, slcan).await?).await
}

/// What to say when the adapter will not open.
///
/// The `devices` command exists precisely for this moment and used to be
/// unreachable from it: nothing ever told the user it was there.
pub fn open_failure(path: &str) -> String {
	format!("opening the adapter at {path} — run `vagcan devices` to list what is connected")
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ui::menu::{Answer, Scripted};

	fn adapter(path: &str, desc: &str, known: bool) -> AdapterInfo {
		AdapterInfo {
			path: path.to_string(),
			description: desc.to_string(),
			known,
			board: false,
		}
	}

	const BOARD: &str = "/dev/cu.usbmodem1101";
	const CANABLE: &str = "/dev/cu.usbmodem206E37A148451";

	/// What the listing makes of each device — the real classification, so
	/// these tests see the ids the way `vagcan` does.
	fn board() -> AdapterInfo {
		vag_uds_can::classify_usb(BOARD.into(), 0x303a, 0x1001, Some("USB JTAG/serial debug unit".into()))
	}
	fn canable() -> AdapterInfo {
		vag_uds_can::classify_usb(CANABLE.into(), 0x16d0, 0x117e, None)
	}
	fn anonymous() -> AdapterInfo {
		vag_uds_can::classify_usb("/dev/cu.usbserial-A10".into(), 0x0403, 0x6001, None)
	}

	fn answering(answer: BoardAnswer) -> impl FnMut(&str) -> BoardAnswer {
		move |path| {
			assert_eq!(path, BOARD, "only the board may be probed");
			answer.clone()
		}
	}
	fn never(path: &str) -> BoardAnswer {
		panic!("probed {path}, which is not an Espressif board")
	}
	fn dash() -> BoardAnswer {
		BoardAnswer::Dash { version: "0.1.0".into() }
	}

	/// Refusals with no command's words in them: `core` knows no command.
	const WHY: NotThroughTheBoard<'static> = NotThroughTheBoard {
		over_ble: "refused over BLE",
		over_usb: "refused through the dash board",
	};

	/// The USB choice for a command that does not go through the board, without `--slcan`.
	fn cable(requested: Option<&str>, listing: Result<Vec<AdapterInfo>>, probe: impl FnMut(&str) -> BoardAnswer) -> Result<String> {
		resolve_cable_for_with(requested, false, &WHY, listing, probe)
	}

	/// [`resolve_with`] without `--slcan`.
	async fn resolve_as<H>(
		requested: Option<&str>,
		listing: Result<Vec<AdapterInfo>>,
		probe: impl FnMut(&str) -> BoardAnswer,
		scan: impl AsyncFnOnce() -> Result<Vec<Board<H>>>,
		can_ask: bool,
		asker: &mut impl Asker,
	) -> Result<Target<H>> {
		resolve_with(requested, false, listing, probe, scan, can_ask, asker).await
	}

	#[test]
	fn a_board_on_the_display_image_does_not_take_the_canables_place() {
		// The owner's desk. Master auto-picked the CANable; the board's ids
		// alone must not turn that into "several devices — say which one".
		let picked = cable(None, Ok(vec![canable(), board()]), answering(BoardAnswer::Silent)).unwrap();
		assert_eq!(picked, CANABLE);
	}

	#[test]
	fn a_board_on_the_display_image_alone_is_a_firmware_error_not_a_car_timeout() {
		let err = cable(None, Ok(vec![board()]), answering(BoardAnswer::Silent)).unwrap_err().to_string();
		assert!(err.contains(BOARD), "{err}");
		assert!(err.contains("slcan"), "{err}");
		assert!(err.contains("Reflash"), "{err}");
	}

	#[test]
	fn a_board_on_the_slcan_image_alone_is_picked() {
		assert_eq!(cable(None, Ok(vec![board()]), answering(BoardAnswer::Slcan)).unwrap(), BOARD);
	}

	#[test]
	fn a_board_on_the_slcan_image_next_to_a_canable_is_a_question() {
		let err = cable(None, Ok(vec![canable(), board()]), answering(BoardAnswer::Slcan))
			.unwrap_err()
			.to_string();
		assert!(err.contains("say which one") && err.contains(CANABLE) && err.contains(BOARD), "{err}");
	}

	#[test]
	fn a_silent_board_does_not_win_over_an_unrecognised_adapter() {
		// It used to: its ids made it "known", so it beat a device that may
		// well be the slcan adapter somebody meant.
		let picked = cable(None, Ok(vec![anonymous(), board()]), answering(BoardAnswer::Silent)).unwrap();
		assert_eq!(picked, "/dev/cu.usbserial-A10");
	}

	#[test]
	fn a_busy_board_says_so() {
		let err = cable(None, Ok(vec![board()]), answering(BoardAnswer::Unopened("Resource busy".into())))
			.unwrap_err()
			.to_string();
		assert!(err.contains("Resource busy"), "{err}");
		// A busy slcan board is a working adapter; reflashing it is the wrong advice.
		assert!(!err.contains("eflash"), "firmware advice for a busy port: {err}");
		assert!(err.contains("close"), "say what to do about the port: {err}");
	}

	#[test]
	fn a_busy_board_and_a_silent_one_each_get_their_own_advice() {
		const SECOND: &str = "/dev/cu.usbmodem1201";
		let mut second = board();
		second.path = SECOND.into();
		let err = cable(None, Ok(vec![board(), second]), |path| match path {
			BOARD => BoardAnswer::Unopened("Resource busy".into()),
			_ => BoardAnswer::Silent,
		})
		.unwrap_err()
		.to_string();
		assert!(err.contains(BOARD) && err.contains(SECOND), "{err}");
		assert!(err.contains("Resource busy") && err.contains("close"), "{err}");
		assert!(err.contains("Reflash"), "the silent one still needs its image: {err}");
	}

	#[test]
	fn an_explicit_board_that_does_not_answer_fails_fast() {
		for path in [BOARD, "/dev/tty.usbmodem1101"] {
			let err = cable(Some(path), Ok(vec![canable(), board()]), answering(BoardAnswer::Silent))
				.unwrap_err()
				.to_string();
			assert!(err.contains("Reflash"), "{path}: {err}");
			assert!(err.contains("--device ble"), "the board has another way in: {err}");
		}
	}

	#[test]
	fn an_explicit_board_that_answers_is_used() {
		assert_eq!(cable(Some(BOARD), Ok(vec![board()]), answering(BoardAnswer::Slcan)).unwrap(), BOARD);
	}

	#[test]
	fn only_espressif_boards_are_ever_probed() {
		assert_eq!(cable(None, Ok(vec![canable(), anonymous()]), never).unwrap(), CANABLE);
		assert_eq!(cable(Some(CANABLE), Ok(vec![canable(), anonymous()]), never).unwrap(), CANABLE);
		let listed = probed(vec![canable(), anonymous()], never);
		assert_eq!(listed, vec![canable(), anonymous()]);
	}

	#[test]
	fn the_listing_says_which_image_the_board_is_running() {
		let through = render_list(&probed(vec![board()], answering(dash())));
		assert!(through.contains(&format!("* {BOARD}")), "{through}");
		assert!(
			through.contains("vag-dash board — dash image (reads through the board, panel keeps running), 0.1.0"),
			"{through}"
		);

		let slcan = render_list(&probed(vec![board()], answering(BoardAnswer::Slcan)));
		assert!(slcan.contains(&format!("* {BOARD}")), "{slcan}");
		assert!(slcan.contains("vag-dash board — slcan image"), "{slcan}");

		let silent = render_list(&probed(vec![board()], answering(BoardAnswer::Silent)));
		assert!(silent.contains(&format!("  {BOARD}")), "{silent}");
		assert!(
			silent.contains("answers neither Hello nor slcan") && silent.contains("reflash"),
			"{silent}"
		);
	}

	#[test]
	fn a_recognised_adapter_wins_over_anonymous_serial_devices() {
		// The situation on the real machine: a CANable plus whatever else the
		// laptop exposes. Picking must not degrade into "several found".
		let found = [
			adapter("/dev/cu.usbmodem1", "CANable 2.0 (slcan)", true),
			adapter("/dev/cu.usbserial", "USB 0403:6001", false),
		];
		let known: Vec<&AdapterInfo> = found.iter().filter(|a| a.known).collect();
		assert_eq!(known.len(), 1);
		assert_eq!(known[0].path, "/dev/cu.usbmodem1");
	}

	#[test]
	fn an_explicit_device_is_used_verbatim() {
		// Never second-guess an explicit choice, even a path that looks odd.
		assert_eq!(resolve_cable_for(Some("/dev/whatever"), false, &WHY).unwrap(), "/dev/whatever");
	}

	#[test]
	fn the_listing_marks_recognised_adapters_and_explains_the_mark() {
		let text = render_list(&[
			adapter("/dev/cu.usbmodem1", "CANable 2.0 (slcan)", true),
			adapter("/dev/cu.usbserial", "USB 0403:6001", false),
		]);
		assert!(text.contains("* /dev/cu.usbmodem1"), "{text}");
		assert!(text.contains("  /dev/cu.usbserial"), "{text}");
		assert!(text.contains("recognised CAN adapter"), "{text}");
	}

	#[test]
	fn an_empty_listing_explains_the_enumeration_trap() {
		// The failure that actually happened on the car: the adapter present on
		// USB, no serial node, every open failing with "No such file".
		let text = render_list(&[]);
		assert!(text.contains("unplug and replug"), "{text}");
	}

	// --- the board over BLE ---

	fn heard(name: &str, id: &str, rssi: i16) -> Board<()> {
		Board {
			name: name.into(),
			id: id.into(),
			rssi: Some(rssi),
			handle: (),
		}
	}

	fn two() -> Vec<Board<()>> {
		vec![heard("vagcan-dash", "AAAA", -55), heard("vagcan-dash-garage", "BBBB", -80)]
	}

	/// A scan that hears `boards`.
	fn hears(boards: Vec<Board<()>>) -> impl AsyncFnOnce() -> Result<Vec<Board<()>>> {
		async move || Ok(boards)
	}

	/// A scan that must not happen.
	fn no_scan() -> impl AsyncFnOnce() -> Result<Vec<Board<()>>> {
		async || -> Result<Vec<Board<()>>> { panic!("scanned BLE, which this choice must not do") }
	}

	/// A scan that fails the way a Mac without Bluetooth access does.
	fn denied() -> impl AsyncFnOnce() -> Result<Vec<Board<()>>> {
		async || -> Result<Vec<Board<()>>> { bail!("no Bluetooth adapter (on macOS this also means Bluetooth access was denied)") }
	}

	fn chosen(target: Result<Target<()>>) -> Board<()> {
		match target {
			Ok(Target::Ble(board)) => board,
			Ok(Target::Serial(path) | Target::BoardUsb { path }) => panic!("chose the USB device {path}, expected a board over BLE"),
			Err(e) => panic!("refused: {e:#}"),
		}
	}

	fn serial(target: Result<Target<()>>) -> String {
		match target {
			Ok(Target::Serial(path)) => path,
			Ok(Target::BoardUsb { path }) => panic!("chose to read through the dash board on {path}, expected a plain adapter"),
			Ok(Target::Ble(board)) => panic!("chose {} over BLE, expected a cable", board.name),
			Err(e) => panic!("refused: {e:#}"),
		}
	}

	fn board_usb(target: Result<Target<()>>) -> String {
		match target {
			Ok(Target::BoardUsb { path }) => path,
			Ok(Target::Serial(path)) => panic!("chose {path} as a plain adapter, expected to read through the dash board"),
			Ok(Target::Ble(board)) => panic!("chose {} over BLE, expected the board's cable", board.name),
			Err(e) => panic!("refused: {e:#}"),
		}
	}

	fn refused(target: Result<Target<()>>) -> String {
		match target {
			Ok(_) => panic!("chose something, expected a refusal"),
			Err(e) => format!("{e:#}"),
		}
	}

	#[test]
	fn slcan_over_ble_says_what_to_do_instead() {
		assert!(
			SLCAN_OVER_BLE.ends_with("— drop --slcan, or name the board's serial path"),
			"{SLCAN_OVER_BLE}"
		);
	}

	#[test]
	fn only_ble_and_ble_with_a_name_mean_the_board() {
		assert_eq!(ble_request("ble"), Some(None));
		assert_eq!(ble_request("ble:vagcan-dash"), Some(Some("vagcan-dash")));
		assert_eq!(ble_request("ble:"), Some(Some("")));
		assert_eq!(ble_request(BOARD), None);
		assert_eq!(ble_request("bleh"), None);
	}

	#[tokio::test]
	async fn ble_takes_the_one_board_it_hears_even_with_a_cable_plugged_in() {
		let mut menu = Scripted::new(vec![]);
		let board = chosen(
			resolve_as(
				Some("ble"),
				Ok(vec![canable()]),
				never,
				hears(vec![heard("vagcan-dash", "A", -60)]),
				true,
				&mut menu,
			)
			.await,
		);
		assert_eq!(board.name, "vagcan-dash");
		assert!(menu.seen.is_empty(), "one board is taken without asking");
	}

	#[tokio::test]
	async fn ble_with_no_board_in_range_says_it_needs_the_ignition_and_range() {
		let err = refused(resolve_as(Some("ble"), Ok(vec![]), never, hears(vec![]), true, &mut Scripted::new(vec![])).await);
		assert!(err.contains("ignition") && err.contains("range"), "{err}");
	}

	#[tokio::test]
	async fn ble_with_several_boards_asks_which() {
		let mut menu = Scripted::new(vec![Answer::Pick(1)]);
		let board = chosen(resolve_as(Some("ble"), Ok(vec![]), never, hears(two()), true, &mut menu).await);
		assert_eq!(board.name, "vagcan-dash-garage");
		assert_eq!(menu.last_labels(), vec!["vagcan-dash", "vagcan-dash-garage"]);
		assert!(menu.last_menu().contains("-80 dBm"), "{}", menu.last_menu());
	}

	#[tokio::test]
	async fn ble_with_several_boards_and_nobody_to_ask_lists_them_and_refuses() {
		let mut menu = Scripted::new(vec![]);
		let err = refused(resolve_as(Some("ble"), Ok(vec![]), never, hears(two()), false, &mut menu).await);
		assert!(
			err.contains("--device ble:vagcan-dash ") && err.contains("--device ble:vagcan-dash-garage "),
			"{err}"
		);
		assert!(menu.seen.is_empty(), "no menu with nobody to see it");
	}

	#[tokio::test]
	async fn ble_by_name_picks_that_board_without_asking() {
		let mut menu = Scripted::new(vec![]);
		let board = chosen(resolve_as(Some("ble:vagcan-dash-garage"), Ok(vec![]), never, hears(two()), true, &mut menu).await);
		assert_eq!(board.id, "BBBB");
		assert!(menu.seen.is_empty());
	}

	#[tokio::test]
	async fn ble_by_a_name_nobody_answers_to_says_what_was_heard() {
		let err = refused(resolve_as(Some("ble:kitchen"), Ok(vec![]), never, hears(two()), true, &mut Scripted::new(vec![])).await);
		assert!(err.contains("kitchen") && err.contains("--device ble:vagcan-dash-garage"), "{err}");
		let err = refused(resolve_as(Some("ble:"), Ok(vec![]), never, hears(two()), true, &mut Scripted::new(vec![])).await);
		assert!(err.contains("needs an adapter's name"), "{err}");
	}

	#[tokio::test]
	async fn two_boards_of_one_name_are_told_apart_by_id() {
		let same = || vec![heard("vagcan-dash", "AAAA", -50), heard("vagcan-dash", "BBBB", -70)];
		let err = refused(
			resolve_as(
				Some("ble:vagcan-dash"),
				Ok(vec![]),
				never,
				hears(same()),
				true,
				&mut Scripted::new(vec![]),
			)
			.await,
		);
		assert!(err.contains("--device ble:AAAA") && err.contains("--device ble:BBBB"), "{err}");
		let board = chosen(resolve_as(Some("ble:BBBB"), Ok(vec![]), never, hears(same()), false, &mut Scripted::new(vec![])).await);
		assert_eq!(board.id, "BBBB");
	}

	#[tokio::test]
	async fn a_bluetooth_failure_is_shown_as_it_came() {
		let err = refused(resolve_as(Some("ble"), Ok(vec![]), never, denied(), true, &mut Scripted::new(vec![])).await);
		assert!(err.contains("Bluetooth access was denied"), "{err}");
	}

	/// Bluetooth is scanned only when `--device ble…` asks for it: on macOS a scan from an
	/// app not allowed Bluetooth gets the process killed, and it prompts somebody who has
	/// no board at all.
	#[tokio::test]
	async fn with_no_cable_nothing_is_scanned_and_the_error_says_how_to_ask_for_ble() {
		let cases = [
			(vec![], BoardAnswer::Silent),
			(vec![board()], BoardAnswer::Silent),
			(vec![board()], BoardAnswer::Unopened("Resource busy".into())),
		];
		for (listing, answer) in cases {
			let err = refused(resolve_as(None, Ok(listing), answering(answer), no_scan(), true, &mut Scripted::new(vec![])).await);
			assert!(err.contains("no USB-CAN adapter found"), "{err}");
			assert!(err.ends_with("An adapter over Bluetooth: --device ble"), "{err}");
		}
	}

	/// `devices` takes no `--device`, so the listing names a command that does.
	#[test]
	fn the_device_listing_ends_by_saying_how_to_reach_a_board_over_bluetooth() {
		for found in [vec![], vec![canable()]] {
			let text = render_list(&found);
			assert!(text.ends_with("An adapter over Bluetooth: vagcan info --device ble"), "{text}");
		}
	}

	#[tokio::test]
	async fn a_cable_a_path_or_an_ambiguous_listing_never_scans() {
		let menu = &mut Scripted::new(vec![]);
		let picked = serial(resolve_as(None, Ok(vec![canable(), board()]), answering(BoardAnswer::Silent), no_scan(), true, menu).await);
		assert_eq!(picked, CANABLE);
		assert_eq!(
			serial(resolve_as(Some(CANABLE), Ok(vec![canable()]), never, no_scan(), true, menu).await),
			CANABLE
		);
		let second = vag_uds_can::classify_usb("/dev/cu.usbserial-B20".into(), 0x0403, 0x6001, None);
		let err = refused(resolve_as(None, Ok(vec![anonymous(), second]), never, no_scan(), true, menu).await);
		assert!(err.contains("say which one"), "several cables are a question about cables: {err}");
	}

	#[test]
	fn a_command_that_does_not_go_through_the_board_takes_the_other_device_beside_a_dash_board() {
		// Survey, sniff, identify without `--slcan`: the board is refused on this path, so
		// it is no candidate either.
		assert_eq!(cable(None, Ok(vec![canable(), board()]), answering(dash())).unwrap(), CANABLE);
		assert_eq!(
			cable(None, Ok(vec![anonymous(), board()]), answering(dash())).unwrap(),
			"/dev/cu.usbserial-A10"
		);
		// Alone it is refused in the command's own words, not as "no adapter".
		assert_eq!(cable(None, Ok(vec![board()]), answering(dash())).unwrap_err().to_string(), WHY.over_usb);
		// With `--slcan` it is an adapter again: beside a CANable a question, beside an
		// unrecognised device the pick.
		let both = resolve_cable_for_with(None, true, &WHY, Ok(vec![canable(), board()]), answering(dash()));
		assert!(both.unwrap_err().to_string().contains("say which one"));
		let beside = resolve_cable_for_with(None, true, &WHY, Ok(vec![anonymous(), board()]), answering(dash()));
		assert_eq!(beside.unwrap(), BOARD);
	}

	#[test]
	fn a_command_that_needs_a_cable_refuses_ble_and_says_why_when_no_cable_is_found() {
		for requested in ["ble", "ble:vagcan-dash"] {
			let err = cable(Some(requested), Ok(vec![canable()]), never).unwrap_err();
			assert_eq!(err.to_string(), WHY.over_ble);
		}
		let err = cable(None, Ok(vec![]), never).unwrap_err().to_string();
		assert!(err.contains("no USB-CAN adapter found") && err.contains(WHY.over_ble), "{err}");
		assert_eq!(cable(None, Ok(vec![canable()]), never).unwrap(), CANABLE);
		assert_eq!(cable(Some(CANABLE), Ok(vec![canable()]), never).unwrap(), CANABLE);
	}

	// --- the board on its USB cable ---

	#[tokio::test]
	async fn a_dash_board_alone_is_read_through_and_nothing_is_scanned() {
		let menu = &mut Scripted::new(vec![]);
		let alone = resolve_as(None, Ok(vec![board()]), answering(dash()), no_scan(), true, menu).await;
		assert_eq!(board_usb(alone), BOARD);
		// Beside a device that is no recognised adapter it still wins, as a CANable does.
		let beside = resolve_as(None, Ok(vec![anonymous(), board()]), answering(dash()), no_scan(), true, menu).await;
		assert_eq!(board_usb(beside), BOARD);
	}

	#[tokio::test]
	async fn a_dash_board_next_to_a_canable_is_a_question_naming_both() {
		let menu = &mut Scripted::new(vec![]);
		let err = refused(resolve_as(None, Ok(vec![canable(), board()]), answering(dash()), no_scan(), true, menu).await);
		assert!(err.contains("several CAN adapters found — say which one"), "{err}");
		assert!(
			err.contains(&format!("--device {CANABLE}")) && err.contains(&format!("--device {BOARD}")),
			"{err}"
		);
		assert!(err.contains("dash image"), "{err}");
	}

	#[tokio::test]
	async fn a_dash_board_named_by_its_path_is_read_through_and_slcan_makes_it_an_adapter() {
		let menu = &mut Scripted::new(vec![]);
		for path in [BOARD, "/dev/tty.usbmodem1101"] {
			let listing = || Ok(vec![canable(), board()]);
			let through = resolve_as(Some(path), listing(), answering(dash()), no_scan(), true, menu).await;
			assert_eq!(board_usb(through), path);
			let adapter = resolve_with(Some(path), true, listing(), answering(dash()), no_scan(), true, menu).await;
			assert_eq!(serial(adapter), path);
		}
	}

	#[tokio::test]
	async fn slcan_makes_a_dash_board_an_adapter_and_changes_nothing_else() {
		let menu = &mut Scripted::new(vec![]);
		let alone = resolve_with(None, true, Ok(vec![board()]), answering(dash()), no_scan(), true, menu).await;
		assert_eq!(serial(alone), BOARD);
		let canable_only = resolve_with(None, true, Ok(vec![canable()]), never, no_scan(), true, menu).await;
		assert_eq!(serial(canable_only), CANABLE);
		let slcan_image = resolve_with(None, true, Ok(vec![board()]), answering(BoardAnswer::Slcan), no_scan(), true, menu).await;
		assert_eq!(serial(slcan_image), BOARD);
		let named = resolve_with(Some(CANABLE), true, Ok(vec![canable()]), never, no_scan(), true, menu).await;
		assert_eq!(serial(named), CANABLE);
		let both = resolve_with(None, true, Ok(vec![canable(), board()]), answering(dash()), no_scan(), true, menu).await;
		assert!(refused(both).contains("say which one"));
	}

	#[tokio::test]
	async fn slcan_over_ble_is_refused_and_with_no_usb_adapter_nothing_is_scanned() {
		let menu = &mut Scripted::new(vec![]);
		for requested in ["ble", "ble:vagcan-dash"] {
			let err = refused(resolve_with(Some(requested), true, Ok(vec![]), never, no_scan(), true, menu).await);
			assert_eq!(err, SLCAN_OVER_BLE);
		}
		let err = refused(resolve_with(None, true, Ok(vec![]), never, no_scan(), true, menu).await);
		assert!(err.contains("no USB-CAN adapter found") && err.contains("--slcan"), "{err}");
	}

	#[test]
	fn what_does_not_go_through_the_board_is_refused_on_each_way_to_it_unless_slcan_makes_the_cable_an_adapter() {
		// `vagcan`'s three such commands — survey, identify, sniff — each with its own words.
		let commands = [
			NotThroughTheBoard {
				over_ble: "survey over BLE",
				over_usb: "survey through the board",
			},
			NotThroughTheBoard {
				over_ble: "identify over BLE",
				over_usb: "identify through the board",
			},
			NotThroughTheBoard {
				over_ble: "sniff over BLE",
				over_usb: "sniff needs --slcan",
			},
		];
		for why in &commands {
			for slcan in [false, true] {
				// The board picked with no `--device`, and named by its path.
				for requested in [None, Some(BOARD)] {
					let got = resolve_cable_for_with(requested, slcan, why, Ok(vec![board()]), answering(dash()));
					match slcan {
						false => assert_eq!(got.unwrap_err().to_string(), why.over_usb, "{requested:?}"),
						true => assert_eq!(got.unwrap(), BOARD, "{requested:?}"),
					}
				}
				for requested in ["ble", "ble:vagcan-dash"] {
					let got = resolve_cable_for_with(Some(requested), slcan, why, Ok(vec![board()]), never);
					let expected = if slcan { SLCAN_OVER_BLE } else { why.over_ble };
					assert_eq!(got.unwrap_err().to_string(), expected, "{requested}");
				}
			}
		}
	}
}
