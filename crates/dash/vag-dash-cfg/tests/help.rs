//! `dashcfg --help` from the built binary, as a person runs it.
//!
//! Before it had a parser, `--help` was a board's name: the binary went straight to
//! Bluetooth, and from a terminal macOS does not allow Bluetooth it was killed with
//! exit 134 and no output. A binary that touches Bluetooth here fails this test the
//! same way (or, with no Bluetooth at all, with an error instead of the usage).

use std::process::Command;

#[test]
fn help_is_printed_without_touching_bluetooth() {
	for flag in ["--help", "-h"] {
		// HOME and `VAGCAN_HOME` at a directory of its own: a process a test starts never
		// reads the person's `~/.vagcan`.
		let home = std::env::temp_dir().join(format!("dashcfg-help-{}", std::process::id()));
		let out = Command::new(env!("CARGO_BIN_EXE_dashcfg"))
			.arg(flag)
			.env("HOME", &home)
			.env("VAGCAN_HOME", home.join(".vagcan"))
			.output()
			.expect("dashcfg runs");
		let stdout = String::from_utf8_lossy(&out.stdout);
		assert!(
			out.status.success(),
			"{flag}: {:?}\n{stdout}{}",
			out.status,
			String::from_utf8_lossy(&out.stderr)
		);
		assert!(stdout.contains("Usage: dashcfg [NAME]"), "{flag}: {stdout}");
		assert!(!stdout.contains("looking for"), "{flag} started a scan: {stdout}");
	}
}
