//! Everything whose input is a recording this tool made.
//!
//! `vagcan watch --out` writes a CSV of whatever was on screen. These commands
//! read it back afterwards, at a desk, and are the reason a drive is worth
//! recording at all — but none has anything to say while the car is in front of
//! you, so none belongs at the top level.
//!
//! The distinction from `vagcan dev vcds` is what the input *is*, not whether it is
//! offline: those read VCDS's files, these read ours.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Subcommand;

use crate::ui::picker;
use crate::{dashreplay, discover};

// Clone for the reason `vcds::Tool` is: the dispatcher keeps a copy of the
// command so it can be run again after the label data has been made.
#[derive(Clone, Subcommand)]
pub enum Tool {
	/// Find which identifiers carry discrete state — a gear, a mode, a switch.
	///
	/// FOR: the values that cannot be fitted. A two-level signal fits any line
	/// exactly, so a gear or a lamp is found by noticing what changed when, not
	/// by least squares.
	///
	/// IN: a `vagcan watch --out` recording, ideally one where the thing you
	/// are looking for was deliberately operated.
	///
	/// OUT: on stdout, a table of the stepped ones — fewest values first — and
	/// a line counting every column as a candidate, continuous or never moved.
	/// With `--pairs`, a second table of candidates that changed together.
	/// `-v` says how a column is sorted; `-vv` when each candidate changed.
	Discover {
		/// Recording written by `vagcan watch --out`. Left out, the recordings
		/// in the current directory are offered as a list.
		#[arg(long, value_name = "FILE")]
		log: Option<String>,
		/// Also list identifiers that changed at the same moments.
		#[arg(long)]
		pairs: bool,
	},

	/// Play a recorded drive on the dash panel, with its alarms — no board, no car.
	///
	/// FOR: seeing what the dash would have shown on that drive: which page, when an
	/// alarm took the screen, which cell it inverted, when it handed back, and what a
	/// press would have done. The pages, the alarms and the drawing are the board's own
	/// code; the values are the recording's, read at the rates the board reads them. A
	/// plan channel the recording does not hold has no value and never trips an alarm.
	///
	/// IN: a `vagcan watch --out` recording, and the car's `dash.toml` as `vagcan dev
	/// dash build` reads it — with the car's recorded units and this project's catalogs. A column
	/// counts for a plan channel only when it is the same unit, identifier and field.
	///
	/// OUT: in a terminal, the panel drawn at the recording's pace with the alarm log
	/// under it (`q` leaves); piped, only the alarm log, at once. The log ends on
	/// stdout either way. `-v` says what the replay leaves out. Nothing is written.
	Dash {
		/// The car whose plan to use, as `vagcan info` reports it.
		#[arg(value_name = "VIN")]
		vin: String,
		/// Recording written by `vagcan watch --out`. Left out, the recordings
		/// in the current directory are offered as a list.
		#[arg(long, value_name = "FILE")]
		log: Option<String>,
		/// Build input to read instead of `~/.vagcan/dash/<VIN>/dash.toml`.
		#[arg(long, value_name = "FILE")]
		input: Option<PathBuf>,
		/// A press at this time of the recording, in seconds of its `t_s`, as
		/// `dashsim`'s press on the board: it silences an alarm on the screen, or
		/// turns to the next page. Repeat for more presses.
		#[arg(long = "press", value_name = "SECONDS", value_parser = seconds)]
		presses: Vec<f64>,
		/// Playback speed in a terminal, from 0.01 to 100. 2 is twice as fast as it
		/// happened.
		#[arg(long, default_value_t = 1.0, value_name = "N", value_parser = speed)]
		speed: f64,
	},
}

/// A playback speed: a factor from [`SLOWEST`] to [`FASTEST`]. Refused outside it rather
/// than quietly changed; the floor also keeps a frame's moment a time the clock can hold.
fn speed(text: &str) -> Result<f64, String> {
	match text.trim().parse::<f64>() {
		Ok(n) if (SLOWEST..=FASTEST).contains(&n) => Ok(n),
		_ => Err(format!("{text:?} is not a speed from {SLOWEST} to {FASTEST}, like 2")),
	}
}

const SLOWEST: f64 = 0.01;
const FASTEST: f64 = 100.0;

/// A time of the recording: a number of seconds, not negative.
fn seconds(text: &str) -> Result<f64, String> {
	match text.trim().parse::<f64>() {
		Ok(s) if s.is_finite() && s >= 0.0 => Ok(s),
		_ => Err(format!("{text:?} is not a time in seconds, like 12.5")),
	}
}

/// The path as typed, or the one picked off a list. `instead` is the command
/// line that needs no list, for whoever is running this down a pipe.
fn pick_when_absent(log: Option<String>, instead: &str) -> Result<Option<String>> {
	match log {
		Some(log) => Ok(Some(log)),
		None => pick_recording(instead),
	}
}

/// The recording to work on, when the command line did not name one.
///
/// **The current directory, because recordings have no home of their own.**
/// `watch --out` writes wherever it was told to; unlike a car's sessions, which
/// `datadir` files under a VIN, nothing has ever settled where a `.csv` from a
/// drive belongs. So the list is of the directory the person is standing in,
/// which is where their last `--out` put one — a guess, but the same guess they
/// made, and not a new convention invented by a file chooser.
///
/// Newest first: these are named by whoever ran `watch`, but the one wanted is
/// nearly always the last drive, and `entries` dates every row.
///
/// `None` is somebody who left the list — an answer, not a failure.
fn pick_recording(instead: &str) -> Result<Option<String>> {
	let mut chooser = crate::ui::Console::new(instead);
	let picked = picker::pick_path(&mut chooser, Path::new("."), &[recordings()])?;
	Ok(picked.map(|path| path.to_string_lossy().into_owned()))
}

/// What a list of recordings is: the `.csv` files, last drive at the top.
fn recordings() -> picker::Level<'static> {
	picker::Level::files("recording")
		.ending(".csv")
		.newest_first()
		.filled_by("vagcan watch --out drive.csv   records one")
}

pub fn run(tool: Tool) -> Result<()> {
	match tool {
		Tool::Discover { log, pairs } => {
			let Some(log) = pick_when_absent(log, "vagcan dev recording discover --log FILE.csv")? else {
				return Ok(());
			};
			let text = std::fs::read_to_string(&log).with_context(|| format!("reading the recording {log:?}"))?;
			discover(&log, &text, pairs)
		}
		Tool::Dash {
			vin,
			log,
			input,
			presses,
			speed,
		} => {
			let Some(log) = pick_when_absent(log, &format!("vagcan dev recording dash {vin} --log FILE.csv"))? else {
				return Ok(());
			};
			dashreplay::run(&vin, &log, input.as_deref(), &presses, speed)
		}
	}
}

/// `discover` over one recording's text: the tables on stdout, the notes on stderr.
fn discover(log: &str, text: &str, pairs: bool) -> Result<()> {
	use vag_cli_core::verbosity::NOTES;
	let columns = discover::classify(text).map_err(|e| anyhow::anyhow!("{log}: {e}"))?;
	let rows = text.lines().skip(1).filter(|l| !l.trim().is_empty()).count();
	let mut said = vec![(NOTES, format!("recording  {log}: {rows} rows, {} columns", columns.len()))];
	said.extend(discover::notes(&columns, discover::untimed(text)));
	vag_cli_core::out!("{}", discover::render(&columns));
	if pairs {
		let window = 0.5;
		said.insert(2, (NOTES, discover::pairs_note(window)));
		vag_cli_core::outln!();
		vag_cli_core::out!("{}", discover::render_pairs(&discover::co_changing(&columns, window)));
	}
	for (level, line) in said {
		vag_cli_core::verbosity::say(level, &line);
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A directory holding what a few drives and one unrelated file leave behind.
	fn a_directory_with(names: &[&str]) -> std::path::PathBuf {
		let dir = std::env::temp_dir().join(format!("vagcan-recording-{}-{:?}", std::process::id(), std::thread::current().id()));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		for name in names {
			std::fs::write(dir.join(name), "t_s\n0.000\n").unwrap();
		}
		dir
	}

	#[test]
	fn the_list_offers_recordings_last_drive_first_and_nothing_else() {
		// `watch --out` names these, so they sort as text and the drive
		// somebody just finished is at the wrong end. And a directory a person
		// works in holds more than recordings: a sniff capture's `.jsonl`, notes.
		let dir = a_directory_with(&["2026-08-02-1030.csv", "2026-08-04-1241.csv", "capture.jsonl", "notes.md"]);
		let names: Vec<String> = picker::entries(&dir, &recordings()).into_iter().map(|c| c.name).collect();
		assert_eq!(names, ["2026-08-04-1241.csv", "2026-08-02-1030.csv"]);
	}

	/// Two switches flipping together and a value that never moved, in the current layout.
	const DRIVE: &str = "t_s,A_t_s,A,B_t_s,B,C_t_s,C\n0.0,0.0,0,0.0,0,0.0,7\n1.0,1.0,1,1.0,1,1.0,7\n2.0,2.0,0,2.0,0,2.0,7\n";

	#[test]
	fn discover_prints_its_tables_and_one_line_each_and_the_level_only_adds_notes() {
		use vag_cli_core::verbosity::{Capture, assert_split};
		let run = |level| {
			let capture = Capture::start(level);
			discover("drive.csv", DRIVE, true).unwrap();
			capture.finish()
		};
		let runs = [run(0), run(1), run(2)];
		assert_eq!(
			runs[0].stdout,
			"column  values  changes  reads  seen\n\
			 A            2        2      3  0 1\n\
			 B            2        2      3  0 1\n\
			 3 columns · 2 candidates · 0 continuous · 1 never moved\n\
			 \n\
			 first  second  coincide\n\
			 A      B           100%\n\
			 1 pair changed together\n"
		);
		assert_eq!(runs[0].stderr, "", "nothing but data by default");
		assert!(
			runs[1].stderr.starts_with("recording  drive.csv: 3 rows, 3 columns\ncandidate: "),
			"{}",
			runs[1].stderr
		);
		assert!(
			runs[1].stderr.contains("pair: two candidates whose changes fall within 0.5 s"),
			"{}",
			runs[1].stderr
		);
		assert!(runs[2].stderr.contains("A: changed at 1.00 2.00 s"), "{}", runs[2].stderr);
		assert_split("dev recording discover", &runs);
	}

	#[test]
	fn a_path_on_the_command_line_is_taken_as_typed_and_asks_nobody() {
		// The picker is what happens when the argument is absent; a path that
		// was given is never second-guessed, and no terminal is touched. This
		// is also what keeps every one of these commands working in a pipe.
		let given = pick_when_absent(Some("drive.csv".into()), "unused").unwrap();
		assert_eq!(given.as_deref(), Some("drive.csv"));
	}
}
