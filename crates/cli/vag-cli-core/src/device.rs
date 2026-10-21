//! Choosing what to talk to the car through: a USB-CAN adapter, or the dash board on its
//! cable or over BLE.
//!
//! A person running this on a car has exactly one adapter plugged in, so
//! requiring them to paste a `/dev/cu.usbmodem…` path every time is friction
//! for nothing. `--device` stays available for the ambiguous cases; when it is
//! omitted we pick the obvious candidate and say which one we picked.
//!
//! **The dash board over BLE is the other way in** (`.archive/tasks/done/dash/16-uds-over-ble.md`,
//! "Choosing the device" and "Zero friction"). `--device ble` (`--ble` for short, [`DeviceArg`])
//! scans for it, takes the one board heard and says so, and offers a menu when there are
//! several; `--device ble:<name>` picks one by name without asking. **Nothing else ever scans** (owner,
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

use anyhow::{Context as _, Result, anyhow, bail};
use vag_uds_can::{
	AdapterInfo, BOARD_PROBE_WAIT, BoardAnswer, CanError, HANDSHAKE_WAIT, SerialPipe, SerialSlcan, SlcanBackend, SlcanBitrate, SlcanMode,
	list_adapters, probe_board,
};

use crate::bus::{Budget, Bus, Carrier};
use crate::report::Advice as _;
use crate::ui::menu::{Asker, Item};

/// How a vag-dash board reads in the listing, by what it answered.
/// The `dash` image reads through the board, its panel running; `devices -v` says so.
const BOARD_DASH: &str = "dash board";
const BOARD_SLCAN: &str = "dash board, slcan image";
const BOARD_SILENT: &str = "dash board, no answer to Hello or slcan: reflash `dash` or `slcan`";

/// Why `--slcan` is refused with `--device ble…`: the error line of [`slcan_over_ble`].
pub const SLCAN_OVER_BLE: &str = "`--slcan` has no meaning over BLE";

/// [`SLCAN_OVER_BLE`], with why and what to do instead.
pub fn slcan_over_ble() -> anyhow::Error {
	anyhow!("{SLCAN_OVER_BLE}")
		.note("`--slcan` makes the board's USB cable a plain adapter")
		.help("drop `--slcan`, or name the board's serial path with `--device`")
}

/// How to reach an adapter over Bluetooth, said wherever no USB device was found:
/// nothing looks for it unasked.
pub const BLE_HINT: &str = "over Bluetooth, add `--ble`";

/// [`BLE_HINT`] where no car command is running — `vagcan devices` and the bare `vagcan`,
/// which take no `--device` — so it names a command that does.
pub const BLE_COMMAND_HINT: &str = "an adapter over Bluetooth: `vagcan info --ble`";

/// No USB device to use at all: what failed, then what to do.
fn no_adapter() -> anyhow::Error {
	anyhow!("no USB-CAN adapter found")
		.help("plug one in and check `vagcan devices`")
		.help("plugged in but not listed? unplug it and plug it back in")
}

// `--device` and its shorthand `--ble`, flattened into every command that reads through
// whatever [`resolve`] picks. `--ble` is `--device ble` and nothing else, so the rule is
// here once: [`DeviceArg::requested`] is the only way a command reads the pair. A command
// that says more about the adapter overrides the help with `mut_arg("device", …)`.
// (Not a doc comment: clap would take one on a flattened struct as the command's `about`.)
// No `display_order` here: the two are declared together, so clap lists `--ble` right
// under `--device` by itself; a fixed number would tie with the first flags of a command
// that declares the pair last (`dev vcds labels`) and sort in between them. The globals
// are what used to come between, and they are ordered last where they are declared.
#[derive(Clone, Debug, Default, clap::Args)]
pub struct DeviceArg {
	/// Adapter to use: a serial path (a USB-CAN adapter, or the dash board on its USB
	/// cable), `ble` for an adapter over Bluetooth, or `ble:<name>` for one adapter by
	/// name. Omit it to use the one adapter or board on USB; Bluetooth is looked for only
	/// when asked. With neither flag, `VAGCAN_DEVICE` names a serial path.
	#[arg(long, value_name = "PATH|ble|ble:NAME")]
	pub device: Option<String>,
	/// Same as `--device ble`: the dash board over Bluetooth.
	#[arg(long, conflicts_with = "device")]
	pub ble: bool,
}

/// The environment variable that names the serial adapter when neither `--device` nor
/// `--ble` is given: `vagcan dev sim`'s port for the e2e cases (`tests/e2e/`), which run every
/// command against a virtual car, and a shell that always reads through one adapter. A serial
/// path only; it is said on stderr whenever it is used.
pub const DEVICE_ENV: &str = "VAGCAN_DEVICE";

impl DeviceArg {
	/// What the flags ask for, as [`resolve`] takes it: `--ble` is `ble`. [`DEVICE_ENV`] is
	/// not read here: [`resolve`] and [`resolve_cable_for`] fall back to it, once.
	pub fn requested(&self) -> Option<&str> {
		if self.ble { Some("ble") } else { self.device.as_deref() }
	}
}

/// `requested`, or [`DEVICE_ENV`] when nothing was: the fallback every resolution takes —
/// [`resolve`] and [`resolve_cable_for`] here, and a caller that resolves through
/// [`resolve_cable_for_with`] with its own listing.
pub fn requested_or_env(requested: Option<&str>) -> Result<Option<String>> {
	if let Some(requested) = requested {
		return Ok(Some(requested.to_string()));
	}
	let from_env = env_device(std::env::var_os(DEVICE_ENV), |path| {
		use std::os::unix::fs::FileTypeExt as _;
		std::fs::metadata(path).is_ok_and(|m| m.file_type().is_char_device())
	})?;
	if let Some(path) = &from_env {
		crate::say!("note: adapter from {DEVICE_ENV}={path}");
	}
	Ok(from_env)
}

/// [`DEVICE_ENV`]'s value as an adapter: unset or empty is none; anything that is not a
/// character device is refused, so a stale path — a pseudo-terminal a closed `dev sim` left,
/// now a terminal tab's — is never opened and written slcan to.
fn env_device(value: Option<std::ffi::OsString>, is_char_device: impl Fn(&std::path::Path) -> bool) -> Result<Option<String>> {
	let Some(value) = value.filter(|v| !v.is_empty()) else {
		return Ok(None);
	};
	let path = value.to_string_lossy().into_owned();
	if !is_char_device(std::path::Path::new(&path)) {
		return Err(
			anyhow!("{DEVICE_ENV}={path} is not a serial device")
				.help(format!("unset {DEVICE_ENV}"))
				.help("name the adapter instead: `--device <path>`"),
		);
	}
	Ok(Some(path))
}

/// Why no board was heard, as far as this side can tell, and what to do about it.
fn no_board(err: anyhow::Error) -> anyhow::Error {
	err
		.note("an adapter on the OBD port is powered by it")
		.help("turn the ignition on")
		.help("bring this computer within range of the board")
}

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
	let requested = requested_or_env(requested)?;
	resolve_with(
		requested.as_deref(),
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
			Some(_) if slcan => return Err(slcan_over_ble()),
			Some(name) => {
				crate::note!("scanning BLE for {} s at most", BLE_SCAN.as_secs());
				// The adapter's own error as it came: on macOS it says where access is allowed.
				let boards = {
					let _waiting = crate::progress::Spinner::new("looking for the dash board over BLE");
					scan().await?
				};
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
	Err(match slcan {
		true => no_cable.note("`--slcan` asks for a USB adapter"),
		false => no_cable.help(BLE_HINT),
	})
}

/// The one board heard, the one named, or the one chosen from a menu; refused when
/// there are several and nobody to choose.
fn choose_board<H>(mut boards: Vec<Board<H>>, name: Option<&str>, can_ask: bool, asker: &mut impl Asker) -> Result<Board<H>> {
	if let Some(name) = name {
		if name.is_empty() {
			return Err(anyhow!("`--device ble:` needs an adapter's name after the colon").help("name it, as in `--device ble:vagcan-dash`"));
		}
		let heard = choices(&boards);
		boards.retain(|b| b.name == name || b.id == name);
		match boards.len() {
			0 if heard.is_empty() => return Err(no_board(anyhow!("no adapter named {name} answered over BLE"))),
			0 => return Err(anyhow!("no adapter named {name} answered over BLE").note(format!("heard:\n{heard}"))),
			1 => {}
			_ => {
				return Err(anyhow!("several adapters are named {name}").help(format!("name one by its id:\n{}", choices(&boards))));
			}
		}
	}
	let at = match boards.len() {
		0 => return Err(no_board(anyhow!("no adapter found over BLE"))),
		1 => 0,
		_ if !can_ask => {
			return Err(anyhow!("several adapters answered over BLE").help(format!("say which one:\n{}", choices(&boards))));
		}
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
	crate::say!(
		"note: adapter {} over BLE{}",
		board.name,
		board.rssi.map(|r| format!(", {r} dBm")).unwrap_or_default()
	);
	crate::note!("BLE id {}", board.id);
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

/// The boards in range, over this computer's first Bluetooth adapter. Getting the adapter
/// says [`vag_dash_ble::PERMISSION_HINT`] first on macOS, which kills a process it does not
/// allow Bluetooth before any error can be printed.
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

/// Ask the USB device at `path` whether it is the dash board, and on which image: the probe
/// [`resolve_cable_for`] runs, for a caller that hands [`resolve_cable_for_with`] its own listing.
pub fn probe(path: &str) -> BoardAnswer {
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
	/// No USB-CAN adapter at all: the error that says so, with its hints.
	Nothing(anyhow::Error),
	/// Nothing to use but a dash board, on a path that does not go through the board.
	OnlyThroughTheBoard,
}

/// Why a command that does not go through the dash board is refused, on each way to it.
pub struct NotThroughTheBoard<'a> {
	/// With `--device ble…` or `--ble`.
	pub over_ble: &'a str,
	/// With the board on its cable, on its `dash` image, and no `--slcan`.
	pub over_usb: &'a str,
	/// `help:` lines under the BLE refusal: what to type instead.
	pub ble_help: &'a [&'a str],
	/// `help:` lines under the refusal through the board.
	pub usb_help: &'a [&'a str],
}

impl NotThroughTheBoard<'_> {
	/// The refusal with `--device ble…`, its help attached.
	fn refused_over_ble(&self) -> anyhow::Error {
		self.ble_help.iter().fold(anyhow!("{}", self.over_ble), |err, help| err.help(help))
	}

	/// The refusal through the board on its `dash` image, its help attached.
	fn refused_over_usb(&self) -> anyhow::Error {
		self.usb_help.iter().fold(anyhow!("{}", self.over_usb), |err, help| err.help(help))
	}
}

/// Resolve a serial adapter, for a command that does not go through the dash board.
///
/// Refused before a session starts: `--device ble…` with `why.over_ble` ([`SLCAN_OVER_BLE`]
/// with `slcan`), and the board on its `dash` image with `why.over_usb` — unless `slcan`
/// made it a plain adapter, where the cable's own guards apply. Otherwise a USB device is
/// resolved as [`resolve_with`] resolves one. With none found, the message is the plain "no
/// USB-CAN adapter": `why.over_ble` under it read as an answer to a request nobody made, and
/// the BLE hint other commands get is left off, since BLE is refused here.
pub fn resolve_cable_for(requested: Option<&str>, slcan: bool, why: &NotThroughTheBoard) -> Result<String> {
	let requested = requested_or_env(requested)?;
	resolve_cable_for_with(requested.as_deref(), slcan, why, list_adapters().map_err(Into::into), probe)
}

/// The half of [`resolve_cable_for`] that needs nothing at all: `--device ble…` refused,
/// with `why.over_ble` ([`SLCAN_OVER_BLE`] with `slcan`). For a command that checks its
/// data before it resolves a device, and would otherwise answer a BLE request with that.
pub fn refuse_ble(requested: Option<&str>, slcan: bool, why: &NotThroughTheBoard) -> Result<()> {
	if requested.is_some_and(|r| ble_request(r).is_some()) {
		return Err(if slcan { slcan_over_ble() } else { why.refused_over_ble() });
	}
	Ok(())
}

/// [`resolve_cable_for`] with the listing and the board probe handed in.
pub fn resolve_cable_for_with(
	requested: Option<&str>,
	slcan: bool,
	why: &NotThroughTheBoard,
	listing: Result<Vec<AdapterInfo>>,
	probe: impl FnMut(&str) -> BoardAnswer,
) -> Result<String> {
	refuse_ble(requested, slcan, why)?;
	let usb = match requested {
		Some(path) => named_usb(path, listing, probe)?,
		// A dash board is a candidate only as `--slcan`'s adapter: without it the board is
		// refused here, so it must not stand in the way of another device either.
		None => match choose_usb(listing, probe, slcan, slcan)? {
			Found::Picked(usb) => usb,
			Found::Nothing(no_cable) => return Err(no_cable),
			Found::OnlyThroughTheBoard => return Err(why.refused_over_usb()),
		},
	};
	match usb.target::<()>(slcan) {
		Target::Serial(path) => Ok(path),
		Target::BoardUsb { .. } | Target::Ble(_) => Err(why.refused_over_usb()),
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
		answer => Err(not_an_adapter(path, &answer)),
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
				true => "dash board, a plain adapter by --slcan",
				false => only.description.as_str(),
			};
			crate::say!("note: adapter {} ({how})", only.path);
			return Ok(Found::Picked(Usb::Dash(only.path.clone())));
		}
		[only] => {
			crate::say!("note: adapter {} ({})", only.path, only.description);
			return Ok(Found::Picked(Usb::Adapter(only.path.clone())));
		}
		[] => {}
		several => {
			return Err(anyhow!("several CAN adapters found").help(format!("say which one:\n{}", device_lines(several.iter().copied()))));
		}
	}

	// No recognised adapter. A board that did not answer is known *not* to be
	// one, so it is no candidate — but it is named, since it is the likeliest
	// thing somebody plugged in meaning to use.
	let (silent, others): (Vec<&AdapterInfo>, Vec<&AdapterInfo>) = found.iter().partition(|a| a.board);
	match (others.as_slice(), silent.as_slice()) {
		([], []) => Ok(Found::Nothing(no_adapter())),
		([], boards) if boards.iter().any(|b| dash.contains(&b.path)) => Ok(Found::OnlyThroughTheBoard),
		([], boards) => {
			let (busy, silent): (Vec<&AdapterInfo>, Vec<&AdapterInfo>) = boards.iter().partition(|b| unopened.contains(&b.path));
			let mut why = anyhow!("no USB-CAN adapter found, only a vag-dash board it cannot use");
			for board in busy.iter().chain(&silent) {
				why = why.note(format!("{}: {}", board.path, board.description));
			}
			if !busy.is_empty() {
				why = why.help("close the program holding the port (a monitor, `dashsim`, another vagcan) and try again");
			}
			if !silent.is_empty() {
				why = reflash(why);
			}
			Ok(Found::Nothing(why.help("plug in a CAN adapter instead")))
		}
		([only], _) => {
			for board in &silent {
				crate::note!("not using {}  {}", board.path, board.description);
			}
			crate::say!("note: adapter {} ({})", only.path, only.description);
			Ok(Found::Picked(Usb::Adapter(only.path.clone())))
		}
		(_, _) => Err(anyhow!("several serial devices found").help(format!("say which one:\n{}", device_lines(found.iter())))),
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
// Says `--device ble`, not `--ble`: this also reaches `dev sniff`, which takes no `--ble`.
fn not_an_adapter(path: &str, answer: &BoardAnswer) -> anyhow::Error {
	match answer {
		BoardAnswer::Unopened(why) => anyhow!("{path} is a vag-dash board, and it could not be opened: {why}")
			.note("its firmware is not known until it opens")
			.help("close the program holding the port (a monitor, `dashsim`, another vagcan) and try again"),
		_ => reflash(
			anyhow!("{path} is a vag-dash board that answers neither Hello nor slcan")
				.note("it runs another image, or none, and a car command on it would only time out"),
		)
		.help("name a CAN adapter instead: `--device <path>`")
		.help("use the board over BLE instead: `--ble`"),
	}
}

/// The two images a board may be given back, each as what to type in a vagcan checkout.
fn reflash(err: anyhow::Error) -> anyhow::Error {
	err
		.help("from a vagcan checkout, reflash the `dash` image: `just fw-flash dash`")
		.help("to use it as a plain adapter, reflash the `slcan` image: `just fw-flash slcan`")
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
			// The plan's id is what `vagcan dev dash build` printed for the plan it was built with.
			BoardAnswer::Dash { version, plan: Some(plan) } => format!("{BOARD_DASH} {version}  plan {plan:016x}"),
			BoardAnswer::Dash { version, plan: None } => format!("{BOARD_DASH} {version}"),
			BoardAnswer::Silent => BOARD_SILENT.to_string(),
			BoardAnswer::Unopened(why) => format!("dash board, could not be opened to ask its firmware ({why})"),
		};
	}
	found.sort_by(|a, b| b.known.cmp(&a.known).then_with(|| a.path.cmp(&b.path)));
	found
}

/// The device list, as `vagcan devices` prints it on stdout: `gh pr list`'s shape, a count,
/// then one row per serial device, recognised CAN adapters first, each marked `✓` when a car
/// command can use it and `✗` when it cannot.
///
/// Serial devices only: `devices` never scans Bluetooth ([`list_notes`] says how to).
pub fn render_list(found: &[AdapterInfo]) -> String {
	let adapters = found.iter().filter(|a| a.known).count();
	let summary = crate::marks::facts([
		format!("{} serial {}", found.len(), crate::table::plural(found.len(), "device")),
		match found.is_empty() {
			true => String::new(),
			false => format!("{adapters} CAN {}", crate::table::plural(adapters, "adapter")),
		},
	]);
	let mut table = crate::table::Table::new(&["", "PATH", "DEVICE"]);
	for a in found {
		table.row(vec![
			if a.known { crate::marks::OK } else { crate::marks::FAIL }.to_string(),
			a.path.clone(),
			a.description.clone(),
		]);
	}
	crate::table::list(&summary, &table, crate::table::terminal_width())
}

/// What `devices` says on stderr beside the list: at level 0 only what to do when there is
/// nothing to list, the rest at `-v`.
pub fn list_notes(found: &[AdapterInfo]) -> Vec<(u8, String)> {
	// Hints in git's shape, each in parentheses under the listing it belongs to.
	let hint = |text: &str| format!("  ({text})");
	let mut notes = Vec::new();
	if found.is_empty() {
		notes.push((
			0,
			hint("plugged in but not listed? unplug it and plug it back in: USB can enumerate it with no serial node"),
		));
	} else {
		notes.push((1, hint("use \"--device <path>\" to pick one; with one adapter it is used without it")));
		if found.iter().any(|a| a.board) {
			notes.push((
				1,
				hint("a dash board on its dash image is read through, its panel running; \"--slcan\" makes it a plain adapter"),
			));
		}
	}
	// With nothing on USB the way over Bluetooth is the next thing to try, said at once.
	notes.push((if found.is_empty() { 0 } else { 1 }, hint(BLE_COMMAND_HINT)));
	notes
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
		.map_err(|e| opening(e.into(), path))
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
				CanError::Timeout => anyhow::anyhow!("the dash board on {path} did not answer Hello within {} ms", HANDSHAKE_WAIT.as_millis())
					.help("run `vagcan devices` to see what it is running"),
				failed => opening(anyhow::Error::new(failed), path),
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
	format!("opening the adapter at {path}")
}

/// `err` as the failure to open the adapter at `path`, with what to run about it.
fn opening(err: anyhow::Error, path: &str) -> anyhow::Error {
	err.context(open_failure(path)).help("run `vagcan devices` to list what is connected")
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

	/// A command line that takes the pair and nothing else.
	#[derive(clap::Parser)]
	struct TakesADevice {
		#[command(flatten)]
		device: DeviceArg,
	}

	fn requested(args: &[&str]) -> Result<Option<String>, clap::Error> {
		use clap::Parser as _;
		let parsed = TakesADevice::try_parse_from(std::iter::once("cmd").chain(args.iter().copied()))?;
		Ok(parsed.device.requested().map(str::to_owned))
	}

	#[test]
	fn the_environment_names_a_serial_device_or_is_refused() {
		let os = |s: &str| Some(std::ffi::OsString::from(s));
		let char_device = |p: &std::path::Path| p == std::path::Path::new("/dev/ttys9");
		assert_eq!(env_device(None, char_device).unwrap(), None);
		assert_eq!(env_device(os(""), char_device).unwrap(), None);
		assert_eq!(env_device(os("/dev/ttys9"), char_device).unwrap().as_deref(), Some("/dev/ttys9"));
		let stale = env_device(os("/dev/ttys404"), char_device).unwrap_err().to_string();
		assert!(stale.starts_with("VAGCAN_DEVICE=/dev/ttys404 is not a serial device"), "{stale}");
	}

	#[test]
	fn the_flags_alone_are_what_was_requested() {
		let arg = |device: Option<&str>, ble: bool| DeviceArg {
			device: device.map(str::to_string),
			ble,
		};
		assert_eq!(arg(Some("/dev/x"), false).requested(), Some("/dev/x"));
		assert_eq!(arg(None, true).requested(), Some("ble"));
		assert_eq!(arg(None, false).requested(), None, "the environment is resolution's, not the flags'");
	}

	#[test]
	fn ble_is_device_ble_and_refused_beside_it() {
		assert_eq!(requested(&["--ble"]).unwrap().as_deref(), Some("ble"));
		assert_eq!(requested(&["--device", "ble"]).unwrap().as_deref(), Some("ble"));
		assert_eq!(requested(&["--device", "ble:vagcan-dash"]).unwrap().as_deref(), Some("ble:vagcan-dash"));
		assert_eq!(requested(&["--device", CANABLE]).unwrap().as_deref(), Some(CANABLE));
		assert_eq!(requested(&[]).unwrap(), None);
		for device in [CANABLE, "ble", "ble:vagcan-dash"] {
			let refused = requested(&["--ble", "--device", device]).expect_err("two adapters named");
			assert_eq!(refused.kind(), clap::error::ErrorKind::ArgumentConflict, "{device}");
		}
		// It means the same thing to the resolver as the long spelling does.
		let short = requested(&["--ble"]).unwrap();
		assert_eq!(short.as_deref().and_then(ble_request), Some(None));
	}

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
		BoardAnswer::Dash {
			version: "0.1.0".into(),
			plan: None,
		}
	}

	/// Refusals with no command's words in them: `core` knows no command.
	const WHY: NotThroughTheBoard<'static> = NotThroughTheBoard {
		over_ble: "refused over BLE",
		over_usb: "refused through the dash board",
		ble_help: &[],
		usb_help: &[],
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
		let err = shown(&cable(None, Ok(vec![board()]), answering(BoardAnswer::Silent)).unwrap_err());
		assert!(err.contains(BOARD), "{err}");
		assert!(err.contains("slcan"), "{err}");
		assert!(err.contains("reflash"), "{err}");
	}

	#[test]
	fn a_board_on_the_slcan_image_alone_is_picked() {
		assert_eq!(cable(None, Ok(vec![board()]), answering(BoardAnswer::Slcan)).unwrap(), BOARD);
	}

	#[test]
	fn a_board_on_the_slcan_image_next_to_a_canable_is_a_question() {
		let err = shown(&cable(None, Ok(vec![canable(), board()]), answering(BoardAnswer::Slcan)).unwrap_err());
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
		let err = shown(&cable(None, Ok(vec![board()]), answering(BoardAnswer::Unopened("Resource busy".into()))).unwrap_err());
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
		let err = shown(
			&cable(None, Ok(vec![board(), second]), |path| match path {
				BOARD => BoardAnswer::Unopened("Resource busy".into()),
				_ => BoardAnswer::Silent,
			})
			.unwrap_err(),
		);
		assert!(err.contains(BOARD) && err.contains(SECOND), "{err}");
		assert!(err.contains("Resource busy") && err.contains("close"), "{err}");
		assert!(err.contains("reflash"), "the silent one still needs its image: {err}");
	}

	#[test]
	fn an_explicit_board_that_does_not_answer_fails_fast() {
		for path in [BOARD, "/dev/tty.usbmodem1101"] {
			let err = shown(&cable(Some(path), Ok(vec![canable(), board()]), answering(BoardAnswer::Silent)).unwrap_err());
			assert!(err.contains("reflash"), "{path}: {err}");
			assert!(err.contains("--ble"), "the board has another way in: {err}");
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
		assert_eq!(
			through,
			format!("1 serial device · 1 CAN adapter\n\n   PATH                  DEVICE\n✓  {BOARD}  dash board 0.1.0\n"),
			"{through}"
		);
		let named = BoardAnswer::Dash {
			version: "0.1.0".into(),
			plan: Some(0x7fa5_728f_35ff_8bcd),
		};
		let named = render_list(&probed(vec![board()], answering(named)));
		assert!(named.contains("  dash board 0.1.0  plan 7fa5728f35ff8bcd\n"), "{named}");

		let slcan = render_list(&probed(vec![board()], answering(BoardAnswer::Slcan)));
		assert!(slcan.contains(&format!("✓  {BOARD}  dash board, slcan image")), "{slcan}");

		let silent = render_list(&probed(vec![board()], answering(BoardAnswer::Silent)));
		assert!(silent.contains(&format!("✗  {BOARD}  dash board, no answer")), "{silent}");
		assert!(silent.contains("reflash"), "{silent}");
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
	fn the_listing_is_a_table_that_marks_recognised_adapters() {
		let found = [
			adapter("/dev/cu.usbmodem1", "CANable 2.0 (slcan)", true),
			adapter("/dev/cu.usbserial", "USB 0403:6001", false),
		];
		assert_eq!(
			render_list(&found),
			"2 serial devices · 1 CAN adapter\n\
			 \n\
			 \x20  PATH               DEVICE\n\
			 ✓  /dev/cu.usbmodem1  CANable 2.0 (slcan)\n\
			 ✗  /dev/cu.usbserial  USB 0403:6001\n"
		);
		// How to pass one is a note, not a line of the list.
		let notes = list_notes(&found);
		assert!(notes.iter().all(|(level, _)| *level >= 1), "{notes:?}");
		assert!(notes.iter().any(|(_, n)| n.contains("--device")), "{notes:?}");
	}

	#[test]
	fn an_empty_listing_explains_the_enumeration_trap() {
		// The failure that actually happened on the car: the adapter present on
		// USB, no serial node, every open failing with "No such file".
		assert_eq!(render_list(&[]), "0 serial devices\n");
		let notes = list_notes(&[]);
		assert!(notes.iter().any(|(level, n)| *level == 0 && n.contains("plug it back in")), "{notes:?}");
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
			Err(e) => shown(&e),
		}
	}

	/// The error as `vagcan` reports it, at level 0, without colour.
	fn shown(e: &anyhow::Error) -> String {
		crate::report::render(e, 0, crate::report::Colour::Never)
	}

	#[test]
	fn slcan_over_ble_says_what_to_do_instead() {
		let said = shown(&slcan_over_ble());
		assert!(said.starts_with(&format!("error: {SLCAN_OVER_BLE}\n")), "{said}");
		assert!(
			said.ends_with("help: drop `--slcan`, or name the board's serial path with `--device`\n"),
			"{said}"
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
	/// What every car command says with nothing plugged in: what failed, then one `help:`
	/// line per action (UX review of PR #38; cargo's shape since 2026-10-21).
	#[tokio::test]
	async fn no_adapter_is_what_failed_and_one_fix_line() {
		let err = refused(resolve_as(None, Ok(vec![]), never, no_scan(), true, &mut Scripted::new(vec![])).await);
		assert_eq!(
			err,
			"error: no USB-CAN adapter found\n\
			 \n\
			 help: plug one in and check `vagcan devices`\n\
			 help: plugged in but not listed? unplug it and plug it back in\n\
			 help: over Bluetooth, add `--ble`\n"
		);
	}

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
			assert!(err.ends_with("help: over Bluetooth, add `--ble`\n"), "{err}");
		}
	}

	/// `devices` takes no `--device`, so the listing names a command that does.
	#[test]
	fn the_device_listing_says_at_v_how_to_reach_a_board_over_bluetooth() {
		for found in [vec![], vec![canable()]] {
			assert!(!render_list(&found).contains("Bluetooth"));
			let notes = list_notes(&found);
			assert!(
				notes.contains(&(if found.is_empty() { 0 } else { 1 }, format!("  ({BLE_COMMAND_HINT})"))),
				"{notes:?}"
			);
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
		// Sniff and identify without `--slcan`: the board is refused on this path, so
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
		assert!(shown(&both.unwrap_err()).contains("say which one"));
		let beside = resolve_cable_for_with(None, true, &WHY, Ok(vec![anonymous(), board()]), answering(dash()));
		assert_eq!(beside.unwrap(), BOARD);
	}

	#[test]
	fn a_command_that_needs_a_cable_refuses_ble_and_says_nothing_of_ble_when_not_asked() {
		for requested in ["ble", "ble:vagcan-dash"] {
			let err = cable(Some(requested), Ok(vec![canable()]), never).unwrap_err();
			assert_eq!(err.to_string(), WHY.over_ble);
		}
		// BLE was not asked for, so the refusal of BLE is not said.
		let err = shown(&cable(None, Ok(vec![]), never).unwrap_err());
		assert!(err.contains("no USB-CAN adapter found") && !err.contains(WHY.over_ble), "{err}");
		assert!(!err.contains("Bluetooth"), "no hint at a link this command refuses: {err}");
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
		assert!(err.contains("several CAN adapters found\n\nhelp: say which one"), "{err}");
		assert!(
			err.contains(&format!("--device {CANABLE}")) && err.contains(&format!("--device {BOARD}")),
			"{err}"
		);
		assert!(err.contains("dash board 0.1.0"), "{err}");
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
			assert!(err.starts_with(&format!("error: {SLCAN_OVER_BLE}\n")), "{err}");
		}
		let err = refused(resolve_with(None, true, Ok(vec![]), never, no_scan(), true, menu).await);
		assert!(err.contains("no USB-CAN adapter found") && err.contains("--slcan"), "{err}");
	}

	#[test]
	fn what_does_not_go_through_the_board_is_refused_on_each_way_to_it_unless_slcan_makes_the_cable_an_adapter() {
		// `vagcan`'s two such commands — identify and sniff — each with its own words.
		let commands = [
			NotThroughTheBoard {
				over_ble: "identify over BLE",
				over_usb: "identify through the board",
				ble_help: &[],
				usb_help: &[],
			},
			NotThroughTheBoard {
				over_ble: "sniff over BLE",
				over_usb: "sniff needs --slcan",
				ble_help: &[],
				usb_help: &[],
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
