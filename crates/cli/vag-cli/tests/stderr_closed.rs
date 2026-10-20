//! A stderr that takes nothing never makes `vagcan` panic.
//!
//! `eprintln!` panics when stderr refuses the bytes, and the panic's own message has
//! nowhere to go either: under a full disk the UX review of PR #38 saw `vagcan -v faults`
//! end with exit 101 and nothing said. macOS has no `/dev/full`, so stderr here is a pipe
//! whose reading end is closed before the run: every write to it fails (EPIPE; Rust's
//! runtime ignores SIGPIPE). The command fails for want of a project, on purpose — the
//! error report is the write that used to panic — and touches no adapter.

use std::process::{Command, Stdio};

#[test]
fn a_run_whose_stderr_takes_nothing_ends_with_its_own_exit_code() {
	let home = tempfile::tempdir().unwrap();
	for args in [&["-v", "faults"][..], &["-vv", "faults", "--unit", "01"]] {
		let mut child = Command::new(env!("CARGO_BIN_EXE_vagcan"))
			.args(args)
			.env("HOME", home.path())
			.env_remove("VAGCAN_PROJECT")
			.stdin(Stdio::null())
			.stdout(Stdio::null())
			.stderr(Stdio::piped())
			.spawn()
			.unwrap();
		// Nobody will read stderr: close it before the run writes to it.
		drop(child.stderr.take());
		let status = child.wait().unwrap();
		assert_eq!(status.code(), Some(1), "{args:?}: a failed run, not a panic (101)");
	}
}
