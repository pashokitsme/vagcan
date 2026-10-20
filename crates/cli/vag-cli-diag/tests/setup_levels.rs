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
	// A stage's seconds differ from one run to the next; what is compared is that the line is there.
	captured.stderr = captured
		.stderr
		.lines()
		.map(|line| match line.strip_prefix("timing  ") {
			Some(rest) => format!("timing  {}", rest.rsplit_once("  ").map_or(rest, |(stage, _)| stage)),
			None => line.to_string(),
		})
		.map(|line| line + "\n")
		.collect();
	captured
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
			assert_eq!(
				quiet.stdout,
				"source  step      state    detail\n\
				 vcds    copy      wrote    3 files, 3 copied (30 B), 0 current\n\
				 vcds    labels    wrote    1 label files\n\
				 vcds    names     missing  no text table\n\
				 vcds    keys      wrote    0 keys\n\
				 vcds    registry  pending  no car recorded: run `vagcan watch` with the car\n\
				 \n\
				 default: new project · 5 steps · 1 gap · fault text · no scalings\n",
				"level 0, stdout"
			);
			// A first read is the one wait of minutes said at level 0; nothing else is.
			assert_eq!(
				quiet.stderr, "first read of this installation: tens of minutes, mostly key searches; later runs reuse them\n",
				"level 0, stderr"
			);
			for said in [
				"project  default  ",
				"vcds copy  ",
				"vcds names: none of [\"TTTEXT.ROD\", \"TTText-RUS.rod\"] is under ",
				"vcds registry: no car recorded on this machine",
				"timing  vcds copy",
				"timing  vcds registry",
				"next: vagcan devices, vagcan info, vagcan faults (stored faults, named)",
				"scalings: none yet",
			] {
				assert!(
					verbose.stderr.lines().any(|l| l.starts_with(said)),
					"-v lacks {said:?}:\n{}",
					verbose.stderr
				);
			}
			assert!(wire.stderr.contains("vcds copy: "), "-vv names each file copied:\n{}", wire.stderr);
			assert!(wire.stderr.contains(" → "), "{}", wire.stderr);
		}
		"one-car" => {
			let [quiet, verbose, _] = levels("setup, one car recorded", Cars::One);
			assert!(
				quiet.stdout.contains("vcds    registry  missing  RM.rod did not open\n"),
				"{}",
				quiet.stdout
			);
			assert!(
				quiet
					.stdout
					.ends_with("default: new project · 5 steps · 2 gaps · fault text · no scalings\n"),
				"{}",
				quiet.stdout
			);
			assert!(
				verbose
					.stderr
					.contains("vcds registry: the registry did not open: there is no RM.rod in this installation"),
				"{}",
				verbose.stderr
			);
		}
		"unreadable" => {
			let [quiet, verbose, _] = levels("setup, a record that does not read", Cars::Unreadable);
			assert!(
				quiet.stdout.contains("vcds    registry  failed   not a record this tool wrote: "),
				"{}",
				quiet.stdout
			);
			// What to do is said at every level, last.
			assert!(
				quiet
					.stderr
					.ends_with("move the file the failed row names aside, then re-run `vagcan setup <VCDS installation>`\n"),
				"{}",
				quiet.stderr
			);
			assert!(
				verbose
					.stderr
					.contains("vcds registry: a car's record on this machine is not one this tool wrote: "),
				"{}",
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
				"reading the VCDS registry: 2 units (1 of this car new); minutes when a key is searched for, the first time only\n\
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
