//! `vagcan setup <VCDS installation>` at `-v` levels 0, 1 and 2, whole, against a tiny
//! made-up installation: what it prints at level 0 is pinned line for line, and the three
//! runs keep the split every command keeps ([`vag_cli_core::verbosity::assert_split`]).
//!
//! Nothing here is a fact about any car: two `.rod` files that are not `.rod` files, one
//! label file with one made-up row, a fault-text file of five bytes.
//!
//! **Each scenario runs in a child process of this test binary.** `setup` writes under
//! `~/.vagcan`, which no test may write into, so the child gets `HOME` pointed at a temporary
//! directory in its environment — no `set_var` in a process with other threads. And its
//! stdin is `/dev/null`: with a terminal on stdin, `setup` asks what to call the project, and
//! a test run from one would wait for an answer nobody gives.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use vag_cli_core::verbosity::{Capture, Captured, assert_split};

/// Set in the child: which scenario to run, and where.
const CHILD: &str = "VAGCAN_SETUP_LEVELS_CHILD";

/// A VCDS installation in miniature: one of each thing `setup` copies or parses.
fn install(at: &Path) -> PathBuf {
	let root = at.join("vcds");
	for (rel, bytes) in [
		("UDS_EV/RD.rod", &b"registry"[..]),
		("UDS_EV/EV_ECM.rod", b"a unit's own file"),
		("Labels/part.lbl", b"001,1,Engine Speed,,"),
		("Codes.dat", b"texts"),
	] {
		let path = root.join(rel);
		std::fs::create_dir_all(path.parent().unwrap()).unwrap();
		std::fs::write(&path, bytes).unwrap();
	}
	root
}

/// What the home holds before a run: no car, one car recorded, or a record that does not read.
#[derive(Clone, Copy)]
enum Cars {
	None,
	One,
	Unreadable,
}

fn cars(home: &Path, cars: Cars) {
	let dir = home.join(".vagcan").join("cars").join("TESTVIN0000000001");
	let record = dir.join(vag_cli_core::datadir::UNITS_FILE);
	match cars {
		Cars::None => {}
		Cars::One => {
			let unit = vag_cli_core::plan::UnitIdentity {
				request: 0x7E0,
				part_number: Some("000000000A".into()),
				odx_name: Some("EV_ECM".into()),
				odx_version: Some("001001".into()),
				component: Some("ENGINE".into()),
			};
			vag_cli_core::units::record_to(&record, "TESTVIN0000000001", &[unit]).unwrap();
		}
		Cars::Unreadable => {
			std::fs::create_dir_all(&dir).unwrap();
			std::fs::write(&record, b"{ not a record").unwrap();
		}
	}
}

/// One `setup` run at `level`, in a home made fresh for it.
fn run_at(level: u8, home: &Path, root: &Path, state: Cars) -> Captured {
	let _ = std::fs::remove_dir_all(home.join(".vagcan"));
	cars(home, state);
	let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
	let capture = Capture::start_all(level);
	let root = root.display().to_string();
	runtime
		.block_on(vag_cli_diag::setup::run(vag_cli_diag::setup::Options {
			dir: Some(&root),
			refresh: false,
			archive_base: vag_cli_diag::setup::vendor::ARCHIVE_BASE,
			download: false,
		}))
		.unwrap();
	let mut captured = capture.finish();
	// A step's seconds differ from one run to the next; what is compared is that the line is there.
	captured.stderr = captured.stderr.lines().map(|line| without_seconds(line) + "\n").collect();
	captured
}

/// A line with its seconds — `(0.0 s; <path>)`, `in 3.1s:`, `stage: 0.01s` — made the
/// same in every run: `N s`.
fn without_seconds(line: &str) -> String {
	let chars: Vec<char> = line.chars().collect();
	let mut out = String::with_capacity(line.len());
	let mut at = 0;
	while at < chars.len() {
		// A number with a point in it, then `s` or ` s` and no letter after.
		let start = at;
		let mut end = at;
		while end < chars.len() && chars[end].is_ascii_digit() {
			end += 1;
		}
		let digits = end > start && (start == 0 || !chars[start - 1].is_ascii_alphanumeric());
		if digits && end < chars.len() && chars[end] == '.' {
			let mut frac = end + 1;
			while frac < chars.len() && chars[frac].is_ascii_digit() {
				frac += 1;
			}
			let unit = match (chars.get(frac), chars.get(frac + 1)) {
				(Some('s'), next) => Some((frac + 1, next)),
				(Some(' '), Some('s')) => Some((frac + 2, chars.get(frac + 2))),
				_ => None,
			};
			if frac > end + 1
				&& let Some((after, next)) = unit
				&& !next.is_some_and(|c| c.is_ascii_alphanumeric())
			{
				out.push_str("N s");
				at = after;
				continue;
			}
		}
		out.push(chars[at]);
		at += 1;
	}
	out
}

/// The three levels of one scenario, split-checked; what level 0 wrote comes back.
fn levels(what: &str, state: Cars) -> [Captured; 3] {
	// The home the parent gave this process: everything `setup` resolves is under it.
	let home = PathBuf::from(std::env::var_os("HOME").expect("the parent sets HOME"));
	let here = tempfile::tempdir().unwrap();
	let root = install(here.path());
	let runs = [0, 1, 2].map(|level| run_at(level, &home, &root, state));
	assert_split(what, &runs);
	runs
}

/// The child's side: run the scenario named in [`CHILD`].
fn child(scenario: &str) {
	match scenario {
		"no-car" => {
			let [quiet, verbose, wire] = levels("setup, no car recorded", Cars::None);
			// On stderr: what is read, a line per step, one `Ready:` line and its hint. A
			// VCDS installation asks no name and lands in the machine's store.
			assert_eq!(quiet.stdout, "", "level 0, stdout");
			let (heading, rest) = quiet.stderr.split_once('\n').unwrap();
			assert!(
				heading.starts_with("Read the VCDS installation at ") && heading.ends_with("/vcds"),
				"{heading}"
			);
			assert_eq!(
				rest,
				"  copied 2 .rod files and the fault text (30 B)\n\
				 \x20 parsed 1 label file\n\
				 \x20 skipped the measurement names: no text table\n\
				 \x20 saved no .rod keys: every section opens by its file name\n\
				 \x20 skipped the channels: read when a car command first meets the car\n\
				 Ready with 1 gap: fault text and no scalings for every car.\n\
				 \x20 (use \"vagcan faults\" with the car connected)\n",
				"level 0, stderr"
			);
			assert!(
				!vag_cli_core::datadir::projects_dir().unwrap().exists(),
				"a VCDS installation made a project"
			);
			assert!(vag_cli_core::datadir::vcds_dir().unwrap().join("cache.sqlite").is_file());
			for said in [
				"    N s, ",
				"    none of [\"TTTEXT.ROD\", \"TTText-RUS.rod\"] is under ",
				"    no car recorded on this machine",
				"  (scalings: none yet",
				"  (took N s)",
			] {
				assert!(
					verbose.stderr.lines().any(|l| l.starts_with(said)),
					"-v lacks {said:?}:\n{}",
					verbose.stderr
				);
			}
			assert!(
				wire.stderr.lines().any(|l| l.starts_with("    copying ") && l.contains("EV_ECM.rod to ")),
				"-vv names each file copied:\n{}",
				wire.stderr
			);
		}
		"one-car" => {
			let [quiet, verbose, _] = levels("setup, one car recorded", Cars::One);
			assert!(
				quiet.stderr.contains("\n  skipped the channels: RM.rod did not open\n"),
				"{}",
				quiet.stderr
			);
			assert!(
				quiet.stderr.contains("\nReady with 2 gaps: fault text and no scalings for every car.\n"),
				"{}",
				quiet.stderr
			);
			assert!(
				verbose
					.stderr
					.contains("\n    the registry did not open: there is no RM.rod in this installation"),
				"{}",
				verbose.stderr
			);
		}
		"unreadable" => {
			let [quiet, verbose, _] = levels("setup, a record that does not read", Cars::Unreadable);
			// What to do is said at every level, right under the line that failed: the
			// file the line names, and the command that reads the installation again.
			let record = vag_cli_core::datadir::cars_dir()
				.unwrap()
				.join("TESTVIN0000000001")
				.join(vag_cli_core::datadir::UNITS_FILE);
			let failed = format!("  failed to read the channels: not a record this tool wrote: {}", record.display());
			let lines: Vec<&str> = quiet.stderr.lines().collect();
			let at = lines.iter().position(|l| *l == failed).unwrap_or_else(|| panic!("{}", quiet.stderr));
			assert!(
				lines[at + 1].starts_with(&format!("help: move {} aside, then run `vagcan setup ", record.display())),
				"{}",
				quiet.stderr
			);
			assert!(
				verbose.stderr.lines().any(|l| l.starts_with("    ") && l.contains("line 1 column")),
				"the cause at -v:\n{}",
				verbose.stderr
			);
		}
		"ensure" => {
			// What `watch`, `measure` and `units --identify` show when a unit of the car
			// has not been tried with the installation: one status line, and — this
			// installation has no registry — one line saying the read did not happen.
			let home = PathBuf::from(std::env::var_os("HOME").unwrap());
			let here = tempfile::tempdir().unwrap();
			let root = install(here.path());
			let unit = vag_cli_core::plan::UnitIdentity {
				request: 0x7E1,
				part_number: Some("000000000B".into()),
				odx_name: Some("EV_TCM".into()),
				odx_version: Some("002002".into()),
				component: Some("GEARBOX".into()),
			};
			let runs = [0, 1, 2].map(|level| {
				run_at(0, &home, &root, Cars::One);
				let capture = Capture::start_all(level);
				vag_cli_core::registry::ensure(std::slice::from_ref(&unit));
				capture.finish()
			});
			assert_split("registry::ensure", &runs);
			assert_eq!(runs[0].stdout, "", "a note of the read is never data");
			assert_eq!(
				runs[0].stderr,
				"reading the VCDS registry: 2 units (1 of this car new), the first time only\n\
				 the VCDS registry did not open: there is no RM.rod in this installation\n"
			);
			assert!(
				runs[1]
					.stderr
					.contains(&format!("vcds registry: {}\n", root.canonicalize().unwrap().display())),
				"{}",
				runs[1].stderr
			);
			assert!(runs[2].stderr.contains("vcds registry: opening RM.rod\n"), "{}", runs[2].stderr);
		}
		other => panic!("no scenario {other:?}"),
	}
}

/// Run `scenario` in a child of this binary, with its own home and no terminal on stdin.
fn in_child(test: &str, scenario: &str) {
	if let Ok(asked) = std::env::var(CHILD) {
		if asked == scenario {
			child(scenario);
		}
		return;
	}
	let home = tempfile::tempdir().unwrap();
	let out = Command::new(std::env::current_exe().unwrap())
		.args([test, "--exact", "--nocapture", "--test-threads=1"])
		.env(CHILD, scenario)
		.env("HOME", home.path())
		.env(vag_cli_core::datadir::ROOT_ENV, home.path().join(".vagcan"))
		.env_remove(vag_cli_core::project::PROJECT_ENV)
		.stdin(Stdio::null())
		.output()
		.unwrap();
	assert!(
		out.status.success(),
		"{scenario} failed:\n--- stdout\n{}\n--- stderr\n{}",
		String::from_utf8_lossy(&out.stdout),
		String::from_utf8_lossy(&out.stderr)
	);
	// The child ran the test, not nothing: the harness says how many it ran.
	assert!(
		String::from_utf8_lossy(&out.stdout).contains("1 passed"),
		"{}",
		String::from_utf8_lossy(&out.stdout)
	);
}

#[test]
fn setup_with_no_car_recorded_prints_one_row_per_step_and_keeps_the_split() {
	in_child("setup_with_no_car_recorded_prints_one_row_per_step_and_keeps_the_split", "no-car");
}

#[test]
fn setup_with_a_car_whose_installation_has_no_registry_says_so_in_its_row() {
	in_child("setup_with_a_car_whose_installation_has_no_registry_says_so_in_its_row", "one-car");
}

#[test]
fn setup_with_a_record_that_does_not_read_closes_on_what_to_do() {
	in_child("setup_with_a_record_that_does_not_read_closes_on_what_to_do", "unreadable");
}

#[test]
fn the_registry_read_a_car_command_triggers_says_one_line_at_level_0() {
	in_child("the_registry_read_a_car_command_triggers_says_one_line_at_level_0", "ensure");
}
