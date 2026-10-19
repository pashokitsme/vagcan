//! Handing a file or a page to the desktop.

use std::ffi::OsStr;

/// Ask the desktop to open a file or a URL in its default application. Every failure is
/// ignored, including the absence of anything to ask: the caller has printed what it opens.
pub fn open(target: impl AsRef<OsStr>) {
	let opener = if cfg!(target_os = "macos") {
		"open"
	} else if cfg!(target_os = "windows") {
		"explorer"
	} else {
		"xdg-open"
	};
	let _ = std::process::Command::new(opener)
		.arg(target)
		.stdout(std::process::Stdio::null())
		.stderr(std::process::Stdio::null())
		.spawn();
}
