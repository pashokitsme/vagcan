//! `dashcfg` — configure the dash over BLE.
//!
//! The device is **always visible** (owner, 2026-09-13/14): it advertises from
//! boot and again after every disconnect, with no button and no pairing. This
//! tool still waits patiently rather than failing with "not found", because a
//! board that is powered off or already serving another central is not on the
//! air either.

use anyhow::{Result, bail};
use btleplug::api::{CharPropFlags, Characteristic, Peripheral as _, WriteType};
use btleplug::platform::Peripheral;
use clap::Parser;
use futures::StreamExt;
use std::time::Duration;
use vag_dash_ble::{Lines, flush, open_nus, scan, stdin_lines};

const DEFAULT_NAME: &str = "vagcan-dash";
const SCAN_SECS: u64 = 4;

/// The arguments. Parsed before anything touches Bluetooth, so `--help` answers from any
/// terminal: a BLE call from one macOS does not allow gets the process killed without a word.
#[derive(Parser, Debug)]
#[command(
	name = "dashcfg",
	version,
	about = "Set the dash board's brightness and page over Bluetooth, and save them on the board.",
	after_help = COMMANDS
)]
struct Cli {
	/// The board's Bluetooth name.
	#[arg(default_value = DEFAULT_NAME)]
	name: String,
}

/// What can be typed once connected; the board's own `help` lists the same words.
const COMMANDS: &str = "\
Once connected, type:
  state               the board's state
  get                 the settings and every page
  set brightness N    how bright the screen is, 0-255
  set page N          the page shown, counting from 0
  save                keep the settings on the board
  load                go back to what the board has saved
  defaults            the default settings, kept only after 'save'
  erase               delete the saved settings; the board starts on defaults
  q                   quit";

/// What the device told us about itself. Parsed from the one line it pushes on
/// every change, so the client never has to poll to stay right.
#[derive(Debug, Default, Clone)]
struct DashState {
	page: Option<u8>,
	pages: Option<u8>,
	brightness: Option<u8>,
	unsaved: Option<bool>,
	/// A finished run of the stopwatch is in RAM and not yet in flash.
	run_pending: Option<bool>,
	generation: Option<u32>,
	kind: Option<String>,
	cells: Option<String>,
	/// The board's fault count once it has ended: the stored codes, or `?` for no count.
	faults: Option<String>,
	/// Of the stored codes, the ones failing now.
	failing: Option<u32>,
	/// The units counted of the units asked, as the board says them: `17/18`.
	units: Option<String>,
}

impl DashState {
	/// `state page=1/2 brightness=77 unsaved=0 run_pending=0 gen=3 kind=chart cells=[0]`
	///
	/// Unknown keys are ignored rather than rejected: the firmware will grow
	/// fields, and a client that refuses to parse a newer device is a client
	/// that has to be updated in lockstep with it.
	fn parse(line: &str) -> Option<Self> {
		let rest = line.strip_prefix("state ")?;
		let mut s = Self::default();
		let mut tokens = rest.split_whitespace();
		while let Some(token) = tokens.next() {
			let Some((key, value)) = token.split_once('=') else {
				continue;
			};
			// A list is printed as `[2, 3, 0, 1]`, spaces and all: the value runs to the
			// token that closes it, or it would stop at `[2,`.
			let mut value = value.to_string();
			if value.starts_with('[') {
				while !value.ends_with(']') {
					let Some(more) = tokens.next() else { break };
					value.push(' ');
					value.push_str(more);
				}
			}
			let value = value.as_str();
			match key {
				"page" => {
					if let Some((now, total)) = value.split_once('/') {
						s.page = now.parse().ok();
						s.pages = total.parse().ok();
					}
				}
				"brightness" => s.brightness = value.parse().ok(),
				"unsaved" => s.unsaved = Some(value != "0"),
				"run_pending" => s.run_pending = Some(value != "0"),
				"gen" => s.generation = value.parse().ok(),
				"kind" => s.kind = Some(value.to_string()),
				"cells" => s.cells = Some(value.to_string()),
				"faults" => s.faults = Some(value.to_string()),
				"failing" => s.failing = value.parse().ok(),
				"units" => s.units = Some(value.to_string()),
				_ => {}
			}
		}
		Some(s)
	}

	fn show(&self, name: &str) {
		const WIDTH: usize = 46;
		println!("\n┌─ {name} {}", "─".repeat(WIDTH.saturating_sub(name.len() + 4)));
		match (self.page, self.pages) {
			(Some(page), Some(pages)) => {
				let kind = self.kind.as_deref().unwrap_or("?");
				let cells = self.cells.as_deref().unwrap_or("?");
				println!("│ page        {} of {pages}   {kind} {cells}", page + 1);
			}
			_ => println!("│ page        ?"),
		}
		if let Some(b) = self.brightness {
			let filled = usize::from(b) * 20 / 255;
			println!("│ brightness  {b:>3}  [{}{}]", "█".repeat(filled), "·".repeat(20 - filled));
		}
		println!("│ storage     {}", self.storage());
		println!("│ faults      {}", self.faults());
		println!("└{}", "─".repeat(WIDTH));
	}

	/// The board's fault count, for the panel's faults line: stored and failing now and how many
	/// units they are of, `?` when there is no count, `-` until the board has counted — once, 10 s
	/// after it starts — and no key at all from an image from before the count.
	fn faults(&self) -> String {
		let Some(stored) = self.faults.as_deref() else {
			return "this board's image does not count faults".to_string();
		};
		match stored {
			"?" => "? — no count: the board's USB log says why".to_string(),
			"-" => "not counted yet".to_string(),
			stored => {
				let mut line = format!("{stored} stored");
				if let Some(failing) = self.failing {
					line.push_str(&format!(", {failing} failing now"));
				}
				if let Some((counted, asked)) = self.units.as_deref().and_then(|u| u.split_once('/')) {
					line.push_str(&format!(" — {counted} of {asked} units counted"));
				}
				line
			}
		}
	}

	/// What flash holds against RAM, for the panel's storage line — and a run the stopwatch kept,
	/// which RAM alone holds until the board writes it: `unsaved=0` alone read "saved" over it.
	fn storage(&self) -> String {
		let saved = match self.unsaved {
			Some(true) => "UNSAVED — 'save' to keep it".to_string(),
			Some(false) => format!("saved (generation {})", self.generation.unwrap_or(0)),
			None => "?".to_string(),
		};
		match self.run_pending {
			Some(true) => format!("{saved}, and a run is in RAM only — 'save' keeps it now"),
			_ => saved,
		}
	}
}

#[tokio::main]
async fn main() -> Result<()> {
	let Cli { name } = Cli::parse();
	let mut lines = stdin_lines();
	let adapter = vag_dash_ble::adapter().await?;

	let target = wait_for(&adapter, &name).await?;
	println!("\nfound {name}, connecting ...");
	let (rx, tx) = open_nus(&target).await?;
	println!("connected");

	let result = session(&mut lines, &target, rx, tx, &name).await;
	target.disconnect().await.ok();
	result
}

/// Scans until the device appears. It is not on the air while powered off or
/// connected to another central, so the hint is repeated rather than printed
/// once and scrolled away.
async fn wait_for(adapter: &btleplug::platform::Adapter, name: &str) -> Result<Peripheral> {
	let mut attempt = 0u32;
	loop {
		attempt += 1;
		if attempt == 1 {
			println!("looking for {name} ...");
		} else if attempt % 3 == 0 {
			println!("still nothing — is the board powered, and is another central connected to it?");
		}
		for found in scan(adapter, SCAN_SECS).await? {
			if found.name.as_deref() == Some(name) {
				return Ok(found.peripheral);
			}
		}
	}
}

async fn session(lines: &mut Lines, p: &Peripheral, rx: Characteristic, tx: Characteristic, name: &str) -> Result<()> {
	if !tx.properties.contains(CharPropFlags::NOTIFY) {
		bail!("the device cannot notify, so nothing can come back");
	}
	p.subscribe(&tx).await?;
	let mut notifications = p.notifications().await?;
	let write_type = if rx.properties.contains(CharPropFlags::WRITE) {
		WriteType::WithResponse
	} else {
		WriteType::WithoutResponse
	};

	// Ask, rather than wait to be told. The device pushes its state on
	// connecting, but a notification sent before the subscription lands is
	// dropped by design — so the first state always comes from a request.
	p.write(&rx, b"state", write_type).await?;
	let mut text = LineBuffer::default();

	println!("\ncommands: state | get | set brightness N | set page N | save | load | defaults | erase | q");
	println!(
		"the device pushes its state when an input on it turns the page, when it turns adapter or panel, when a run is kept or written, and when its fault count ends.\n"
	);

	loop {
		print!("dash> ");
		flush();
		tokio::select! {
				// Bias towards notifications: a page turned on the device should
				// appear the moment it happens, not after the next command.
				biased;
				Some(n) = notifications.next() => {
						for line in text.push(&n.value) {
								print!("\r");
								report(&line, name);
						}
						print!("dash> ");
						flush();
				}
				line = lines.next_line() => {
						let Some(line) = line? else { break };
						let line = line.trim();
						if line.is_empty() {
								continue;
						}
						if line.eq_ignore_ascii_case("q") || line.eq_ignore_ascii_case("quit") {
								break;
						}
						if let Err(e) = p.write(&rx, line.as_bytes(), write_type).await {
								println!("  write failed: {e}");
								continue;
						}
						// A reply is a line, and a line may come in several notifications.
						let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
						let mut replied = false;
						while !replied {
								match tokio::time::timeout_at(deadline, notifications.next()).await {
										Ok(Some(n)) => {
												for line in text.push(&n.value) {
														report(&line, name);
														replied = true;
												}
										}
										Ok(None) => return Ok(()),
										Err(_) => {
												println!("  (no reply in 3 s — the connection may have dropped)");
												break;
										}
								}
						}
				}
		}
	}
	p.unsubscribe(&tx).await.ok();
	Ok(())
}

/// A state push gets drawn as a panel; anything else is the device answering a
/// command, and is printed as it came.
fn report(line: &str, name: &str) {
	match DashState::parse(line) {
		Some(state) => state.show(name),
		None => println!("  {line}"),
	}
}

/// The board's text, joined across notifications and cut into lines.
///
/// The board ends every reply and every state push with `\n` and cuts it at the
/// notification size — 20 bytes before the ATT MTU exchange, 244 after — so one
/// notification is not one line. Bytes that are not UTF-8 are shown replaced rather
/// than dropped: a mangled line is a thing to see.
#[derive(Debug, Default)]
struct LineBuffer {
	partial: Vec<u8>,
}

impl LineBuffer {
	/// Take one notification; get back every line it completed, without its `\n`.
	fn push(&mut self, bytes: &[u8]) -> Vec<String> {
		self.partial.extend_from_slice(bytes);
		let mut lines = Vec::new();
		while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
			let line: Vec<u8> = self.partial.drain(..=end).collect();
			lines.push(String::from_utf8_lossy(&line[..end]).trim_end_matches('\r').to_string());
		}
		lines
	}
}
#[cfg(test)]
mod tests {
	use super::{Cli, DEFAULT_NAME, LineBuffer};
	use clap::Parser;
	use clap::error::ErrorKind;

	fn name(args: &[&str]) -> String {
		Cli::try_parse_from(std::iter::once("dashcfg").chain(args.iter().copied())).unwrap().name
	}

	/// Every way `dashcfg` was started before it had a parser: bare, for the board named
	/// `vagcan-dash`, and with a board's name (`research/dash/can-bring-up.md`).
	#[test]
	fn the_board_is_the_default_name_or_the_one_given() {
		assert_eq!(name(&[]), DEFAULT_NAME);
		assert_eq!(name(&["vagcan-dash"]), "vagcan-dash");
		assert_eq!(name(&["garage dash"]), "garage dash");
	}

	/// `--help` was taken for a board's name and started a scan, which macOS kills from a
	/// terminal it does not allow Bluetooth: exit 134 and not a byte said.
	#[test]
	fn help_and_version_are_answers_not_board_names() {
		for flag in ["--help", "-h"] {
			let err = Cli::try_parse_from(["dashcfg", flag]).unwrap_err();
			assert_eq!(err.kind(), ErrorKind::DisplayHelp, "{flag}");
			let text = err.to_string();
			assert!(text.contains("Usage: dashcfg [NAME]"), "{text}");
			assert!(text.contains("set brightness N"), "{text}");
		}
		assert_eq!(
			Cli::try_parse_from(["dashcfg", "--version"]).unwrap_err().kind(),
			ErrorKind::DisplayVersion
		);
		assert_eq!(
			Cli::try_parse_from(["dashcfg", "--bogus"]).unwrap_err().kind(),
			ErrorKind::UnknownArgument
		);
		assert_eq!(Cli::try_parse_from(["dashcfg", "a", "b"]).unwrap_err().kind(), ErrorKind::UnknownArgument);
	}

	/// The board ends every text reply with `\n` and cuts it at the notification size,
	/// 20 bytes before the MTU exchange: a line is only a line once its end has come.
	#[test]
	fn a_line_cut_into_notifications_comes_out_whole_and_once() {
		let mut buffer = LineBuffer::default();
		assert!(buffer.push(b"state page=0/2 brigh").is_empty());
		assert!(buffer.push(b"tness=128 unsaved=1 ").is_empty());
		assert_eq!(buffer.push(b"gen=5\n"), ["state page=0/2 brightness=128 unsaved=1 gen=5"]);
	}

	#[test]
	fn two_lines_in_one_notification_are_two_lines_and_a_tail_waits() {
		let mut buffer = LineBuffer::default();
		assert_eq!(buffer.push(b"ok: page 1\nstate page=1/2"), ["ok: page 1"]);
		assert_eq!(buffer.push(b" gen=5\n"), ["state page=1/2 gen=5"]);
	}

	/// A finished run the stopwatch kept is in RAM until the board writes it at a standstill, or
	/// a `save` does: `unsaved=0` alone reads "saved" over a run a power-off loses.
	#[test]
	fn a_run_only_ram_holds_is_said_beside_what_flash_holds() {
		let storage = |line: &str| super::DashState::parse(line).unwrap().storage();
		let pending = "a run is in RAM only — 'save' keeps it now";
		assert_eq!(
			storage("state page=0/2 brightness=128 unsaved=0 run_pending=1 gen=5 mode=panel"),
			format!("saved (generation 5), and {pending}")
		);
		assert_eq!(
			storage("state page=0/2 brightness=128 unsaved=1 run_pending=1 gen=5"),
			format!("UNSAVED — 'save' to keep it, and {pending}")
		);
		assert_eq!(storage("state page=0/2 unsaved=0 run_pending=0 gen=5"), "saved (generation 5)");
		// A board from before the key says nothing of a run.
		assert_eq!(storage("state page=0/2 unsaved=1 gen=5"), "UNSAVED — 'save' to keep it");
	}

	/// The board's fault count (`todo/dash/20`): pushed once it ends, `faults=9 failing=1` or
	/// `faults=?`, nothing before.
	#[test]
	fn the_fault_count_is_said_as_stored_and_failing_now_or_why_there_is_none() {
		let faults = |line: &str| super::DashState::parse(line).unwrap().faults();
		assert_eq!(
			faults("state page=0/2 brightness=128 unsaved=0 run_pending=0 gen=5 mode=panel faults=9 failing=1 units=17/18 kind=values cells=[0]"),
			"9 stored, 1 failing now — 17 of 18 units counted"
		);
		assert_eq!(
			faults("state page=0/2 mode=panel faults=0 failing=0 units=4/4"),
			"0 stored, 0 failing now — 4 of 4 units counted"
		);
		assert_eq!(faults("state page=0/2 mode=panel faults=?"), "? — no count: the board's USB log says why");
		// The board counts once, 10 s after it starts: until then `-`.
		assert_eq!(faults("state page=0/2 mode=panel faults=-"), "not counted yet");
		// No key at all: an image from before the count (review round 2).
		assert_eq!(faults("state page=0/2 mode=panel"), "this board's image does not count faults");
	}

	#[test]
	fn a_list_value_keeps_its_spaces() {
		// The firmware's own line, as the bench read it on 2026-09-14.
		let state = super::DashState::parse("state page=0/2 brightness=128 unsaved=1 gen=5 mode=panel kind=values cells=[2, 3, 0, 1]").unwrap();
		assert_eq!(state.cells.as_deref(), Some("[2, 3, 0, 1]"));
		assert_eq!(state.kind.as_deref(), Some("values"));
		assert_eq!(state.generation, Some(5));
	}
}
