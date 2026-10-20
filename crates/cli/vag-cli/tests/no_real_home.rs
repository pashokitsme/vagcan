//! The binary under test never touches the `~/.vagcan` HOME points at.
//!
//! Opening a project's `cache.sqlite` can upgrade it in place, so a test that reached the
//! person's own data dir wrote into it: `cargo test --workspace` upgraded the reference
//! machine's cache on 2026-10-20, through two tests that resolve a unit before refusing a
//! device. A test build's default is a temporary directory (`datadir::vagcan_dir`); this
//! holds it to that, from outside the process, with a cache an open would visibly change.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A `~/.vagcan` holding one project whose cache an open rewrites: at the schema version
/// before today's, with a source row that owns nothing, which the upgrade removes — and a
/// `sources.json` beside it, which the first read of the project's sources removes.
fn canary(home: &Path) -> PathBuf {
	let project = home.join(".vagcan").join("data").join("SK37X");
	std::fs::create_dir_all(&project).unwrap();
	let cache = project.join("cache.sqlite");
	let fault = vag_data_labels::odis::Fault {
		dop: "DTCDOP_VAGUDS".into(),
		code: 297,
		display_code: None,
		text: Some("invented".into()),
		text_id: None,
		short_name: None,
		level: 2,
		temporary: false,
	};
	vag_data_db::put_faults(&cache, "/nowhere/SK37X", "EV_Brake", &[fault]).unwrap();
	rusqlite::Connection::open(&cache)
		.unwrap()
		.execute_batch("INSERT INTO source (kind, dir) VALUES ('vcds', 'vendor/vcds-en/Labels'); PRAGMA user_version = 1;")
		.unwrap();
	std::fs::write(project.join("sources.json"), r#"{"sources": []}"#).unwrap();
	std::fs::write(home.join(".vagcan").join("config.toml"), "project = \"SK37X\"\n").unwrap();
	project
}

/// Every file under `dir`, with its bytes.
fn snapshot(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
	let mut out = Vec::new();
	let mut stack = vec![dir.to_path_buf()];
	while let Some(at) = stack.pop() {
		for entry in std::fs::read_dir(&at).unwrap().flatten() {
			let path = entry.path();
			match path.is_dir() {
				true => stack.push(path),
				false => out.push((path.clone(), std::fs::read(&path).unwrap())),
			}
		}
	}
	out.sort();
	out
}

/// `vagcan` with these arguments, HOME at `home` and no `VAGCAN_HOME`: what a test that
/// forgot an override runs.
fn run(home: &Path, args: &[&str]) -> std::process::Output {
	Command::new(env!("CARGO_BIN_EXE_vagcan"))
		.args(args)
		.env("HOME", home)
		.env_remove(vag_cli_core::datadir::ROOT_ENV)
		.output()
		.expect("vagcan runs")
}

/// The commands that run with no car and no adapter, and the two whose tests reached the
/// reference machine's cache.
const CAR_FREE: &[&[&str]] = &[
	&[],
	&["setup", "--list"],
	&["setup", "--drop", "vcds"],
	&["units", "--identify", "ZZZ", "--device", "ble", "--slcan"],
	&["sensors", "--unit", "ZZZ", "--device", "ble", "--slcan"],
	&["faults", "--unit", "ZZZ", "--device", "ble", "--slcan"],
	&["devices"],
];

#[test]
fn no_car_free_command_of_a_test_build_touches_the_data_dir_home_points_at() {
	let home = tempfile::tempdir().unwrap();
	canary(home.path());
	let before = snapshot(home.path());
	for args in CAR_FREE {
		run(home.path(), args);
		assert_eq!(snapshot(home.path()), before, "`vagcan {}` changed a file under HOME", args.join(" "));
	}
}

#[test]
fn the_canary_is_one_an_open_would_change() {
	// The guard above proves nothing unless reaching this data dir changes it. Named
	// through the override, the same command reads it — and the upgrade and the read of
	// its sources rewrite it.
	let home = tempfile::tempdir().unwrap();
	let project = canary(home.path());
	let before = snapshot(home.path());
	let out = Command::new(env!("CARGO_BIN_EXE_vagcan"))
		.args(["setup", "--list"])
		.env("HOME", home.path())
		.env(vag_cli_core::datadir::ROOT_ENV, home.path().join(".vagcan"))
		.output()
		.expect("vagcan runs");
	assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
	assert!(String::from_utf8_lossy(&out.stdout).contains("/nowhere/SK37X"));
	assert_ne!(snapshot(home.path()), before, "reaching the data dir left the canary as it was");
	assert!(!project.join("sources.json").exists());
}
