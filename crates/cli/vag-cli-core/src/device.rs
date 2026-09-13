//! Choosing which USB-CAN adapter to talk to.
//!
//! A person running this on a car has exactly one adapter plugged in, so
//! requiring them to paste a `/dev/cu.usbmodem…` path every time is friction
//! for nothing. `--device` stays available for the ambiguous cases; when it is
//! omitted we pick the obvious candidate and say which one we picked.

/// The rate every slcan adapter on this project's bench runs at.
///
/// In `core` because it is a property of the cable rather than of any command,
/// and three crates open that cable now.
pub const ADAPTER_BAUD: u32 = 115_200;

use anyhow::{Context as _, Result, bail};
use vag_uds_can::{AdapterInfo, BOARD_PROBE_WAIT, BoardAnswer, SerialSlcan, SlcanBackend, SlcanBitrate, SlcanMode, list_adapters, probe_board};

/// How a vag-dash board reads in the listing, by what it answered.
const BOARD_SLCAN: &str = "vag-dash board — slcan firmware answering";
const BOARD_SILENT: &str = "vag-dash board — not answering slcan (display firmware? flash the slcan image)";

/// Resolve the adapter to open.
///
/// With `--device` given, that path is used — no guessing behind the user's
/// back — with one check: a vag-dash board that does not answer slcan is
/// refused at once, because opening it would only time out and blame the car.
/// Without it: exactly one candidate is used automatically, none or several is
/// an error that lists what was found, because silently picking one of two
/// adapters is how you end up talking to the wrong bus.
///
/// A vag-dash board ([`vag_uds_can::BOARD_USB`]) counts as an adapter only when
/// it answers slcan's `V` query: its ids are every ESP32's, and it runs either
/// the display image or the adapter image under them. Nothing else is ever
/// asked anything.
pub fn resolve(requested: Option<&str>) -> Result<String> {
	resolve_with(requested, list_adapters().map_err(Into::into), |path| {
		probe_board(path, ADAPTER_BAUD, BOARD_PROBE_WAIT)
	})
}

/// [`resolve`] with the listing and the board probe handed in, so the choice
/// can be tested without a serial port.
pub fn resolve_with(requested: Option<&str>, listing: Result<Vec<AdapterInfo>>, mut probe: impl FnMut(&str) -> BoardAnswer) -> Result<String> {
	if let Some(path) = requested {
		// A listing that fails says nothing about the path, so the path is
		// honoured as given; only a path the listing names as a board is asked.
		let board = listing
			.ok()
			.and_then(|found| found.into_iter().find(|a| a.board && same_node(&a.path, path)));
		// Asked on the listed `cu.*` node, which is the one to open.
		if let Some(board) = board {
			match probe(&board.path) {
				BoardAnswer::Slcan => {}
				answer => bail!("{}", not_an_adapter(path, &answer)),
			}
		}
		return Ok(path.to_string());
	}

	// Which boards would not open, kept apart from the silent ones: a busy port
	// may well be a working slcan board, and that wants different advice.
	let mut unopened: Vec<String> = Vec::new();
	let found = probed(listing?, |path| {
		let answer = probe(path);
		if matches!(answer, BoardAnswer::Unopened(_)) {
			unopened.push(path.to_string());
		}
		answer
	});

	// A recognised CAN adapter wins outright. Someone with a CANable plugged in
	// next to an Arduino means the CANable, and making them spell that out
	// every time is friction for nothing. A board that answered `V` is one.
	let known: Vec<&AdapterInfo> = found.iter().filter(|a| a.known).collect();
	match known.as_slice() {
		[only] => {
			eprintln!("using {} ({})", only.path, only.description);
			return Ok(only.path.clone());
		}
		[] => {}
		several => bail!("several CAN adapters found — say which one:\n{}", device_lines(several.iter().copied())),
	}

	// No recognised adapter. A board that did not answer is known *not* to be
	// one, so it is no candidate — but it is named, since it is the likeliest
	// thing somebody plugged in meaning to use.
	let (silent, others): (Vec<&AdapterInfo>, Vec<&AdapterInfo>) = found.iter().partition(|a| a.board);
	match (others.as_slice(), silent.as_slice()) {
		([], []) => bail!(
			"no USB-CAN adapter found.\n\
             Plug one in and check it enumerated: `vagcan devices`.\n\
             If it is plugged in but missing, unplug and replug it — the adapter can \
             enumerate on USB without macOS attaching a serial node."
		),
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
					"\n{}\nIt is running the display firmware (or another image), not the adapter one. Flash its \
                     `slcan` image (research/dash/can-bring-up.md §9.2).",
					lines(&silent)
				));
			}
			bail!("{why}\nOr plug in a CAN adapter.")
		}
		([only], _) => {
			for board in &silent {
				eprintln!("not using {} ({})", board.path, board.description);
			}
			eprintln!("using {} ({})", only.path, only.description);
			Ok(only.path.clone())
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
			"{path} is a vag-dash board that does not answer slcan — it is running the display firmware \
             (or another image), not the adapter one, and a car command on it would only time out.\n\
             Flash its `slcan` image (research/dash/can-bring-up.md §9.2), or name a CAN adapter."
		),
	}
}

/// Every candidate adapter, for `vagcan devices` — each vag-dash board asked
/// for its slcan version first, so the list says which image it runs.
pub fn list() -> Result<Vec<AdapterInfo>> {
	Ok(probed(list_adapters()?, |path| probe_board(path, ADAPTER_BAUD, BOARD_PROBE_WAIT)))
}

/// The listing with every board's answer folded in: a board that answered is a
/// recognised adapter, and every board's description says what it answered.
/// Only boards are probed. Recognised adapters sort first again afterwards.
pub fn probed(mut found: Vec<AdapterInfo>, mut probe: impl FnMut(&str) -> BoardAnswer) -> Vec<AdapterInfo> {
	for adapter in found.iter_mut().filter(|a| a.board) {
		let answer = probe(&adapter.path);
		adapter.known = answer == BoardAnswer::Slcan;
		adapter.description = match answer {
			BoardAnswer::Slcan => BOARD_SLCAN.to_string(),
			BoardAnswer::Silent => BOARD_SILENT.to_string(),
			BoardAnswer::Unopened(why) => format!("vag-dash board — could not be opened to ask its firmware ({why})"),
		};
	}
	found.sort_by(|a, b| b.known.cmp(&a.known).then_with(|| a.path.cmp(&b.path)));
	found
}

/// Render the device list for a human.
pub fn render_list(found: &[AdapterInfo]) -> String {
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
	out.push_str("  only one is connected.");
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

	#[test]
	fn a_board_on_the_display_image_does_not_take_the_canables_place() {
		// The owner's desk. Master auto-picked the CANable; the board's ids
		// alone must not turn that into "several devices — say which one".
		let picked = resolve_with(None, Ok(vec![canable(), board()]), answering(BoardAnswer::Silent)).unwrap();
		assert_eq!(picked, CANABLE);
	}

	#[test]
	fn a_board_on_the_display_image_alone_is_a_firmware_error_not_a_car_timeout() {
		let err = resolve_with(None, Ok(vec![board()]), answering(BoardAnswer::Silent))
			.unwrap_err()
			.to_string();
		assert!(err.contains(BOARD), "{err}");
		assert!(err.contains("slcan"), "{err}");
		assert!(err.contains("firmware"), "{err}");
	}

	#[test]
	fn a_board_on_the_slcan_image_alone_is_picked() {
		assert_eq!(resolve_with(None, Ok(vec![board()]), answering(BoardAnswer::Slcan)).unwrap(), BOARD);
	}

	#[test]
	fn a_board_on_the_slcan_image_next_to_a_canable_is_a_question() {
		let err = resolve_with(None, Ok(vec![canable(), board()]), answering(BoardAnswer::Slcan))
			.unwrap_err()
			.to_string();
		assert!(err.contains("say which one") && err.contains(CANABLE) && err.contains(BOARD), "{err}");
	}

	#[test]
	fn a_silent_board_does_not_win_over_an_unrecognised_adapter() {
		// It used to: its ids made it "known", so it beat a device that may
		// well be the slcan adapter somebody meant.
		let picked = resolve_with(None, Ok(vec![anonymous(), board()]), answering(BoardAnswer::Silent)).unwrap();
		assert_eq!(picked, "/dev/cu.usbserial-A10");
	}

	#[test]
	fn a_busy_board_says_so() {
		let err = resolve_with(None, Ok(vec![board()]), answering(BoardAnswer::Unopened("Resource busy".into())))
			.unwrap_err()
			.to_string();
		assert!(err.contains("Resource busy"), "{err}");
		// A busy slcan board is a working adapter; reflashing it is the wrong advice.
		assert!(
			!err.contains("Flash") && !err.contains("display firmware"),
			"firmware advice for a busy port: {err}"
		);
		assert!(err.contains("close"), "say what to do about the port: {err}");
	}

	#[test]
	fn a_busy_board_and_a_silent_one_each_get_their_own_advice() {
		const SECOND: &str = "/dev/cu.usbmodem1201";
		let mut second = board();
		second.path = SECOND.into();
		let err = resolve_with(None, Ok(vec![board(), second]), |path| match path {
			BOARD => BoardAnswer::Unopened("Resource busy".into()),
			_ => BoardAnswer::Silent,
		})
		.unwrap_err()
		.to_string();
		assert!(err.contains(BOARD) && err.contains(SECOND), "{err}");
		assert!(err.contains("Resource busy") && err.contains("close"), "{err}");
		assert!(err.contains("Flash"), "the silent one still needs its image: {err}");
	}

	#[test]
	fn an_explicit_board_that_does_not_answer_fails_fast() {
		for path in [BOARD, "/dev/tty.usbmodem1101"] {
			let err = resolve_with(Some(path), Ok(vec![canable(), board()]), answering(BoardAnswer::Silent))
				.unwrap_err()
				.to_string();
			assert!(err.contains("firmware"), "{path}: {err}");
		}
	}

	#[test]
	fn an_explicit_board_that_answers_is_used() {
		assert_eq!(
			resolve_with(Some(BOARD), Ok(vec![board()]), answering(BoardAnswer::Slcan)).unwrap(),
			BOARD
		);
	}

	#[test]
	fn only_espressif_boards_are_ever_probed() {
		assert_eq!(resolve_with(None, Ok(vec![canable(), anonymous()]), never).unwrap(), CANABLE);
		assert_eq!(resolve_with(Some(CANABLE), Ok(vec![canable(), anonymous()]), never).unwrap(), CANABLE);
		let listed = probed(vec![canable(), anonymous()], never);
		assert_eq!(listed, vec![canable(), anonymous()]);
	}

	#[test]
	fn the_listing_says_which_image_the_board_is_running() {
		let slcan = render_list(&probed(vec![board()], answering(BoardAnswer::Slcan)));
		assert!(slcan.contains(&format!("* {BOARD}")), "{slcan}");
		assert!(slcan.contains("slcan firmware answering"), "{slcan}");

		let display = render_list(&probed(vec![board()], answering(BoardAnswer::Silent)));
		assert!(display.contains(&format!("  {BOARD}")), "{display}");
		assert!(display.contains("not answering slcan"), "{display}");
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
		assert_eq!(resolve(Some("/dev/whatever")).unwrap(), "/dev/whatever");
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
}
