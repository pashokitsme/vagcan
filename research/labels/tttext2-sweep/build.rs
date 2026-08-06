//! Stage the shipped `.rod` decoder + key search for `include!`.
//!
//! The point of this crate is that it drives *the shipped code*, not a copy of
//! it — two `.rod` conclusions in this project have already had to be reopened
//! because a research re-implementation and `crates/` disagreed. But the files
//! cannot be `include!`d as they stand:
//!
//! * they open with `//!` module docs, and an inner attribute may not arrive
//!   from a macro expansion (E0753);
//! * `rod.rs` does `include_bytes!("rod_mt.bin")`, which would resolve next to
//!   the staged copy instead of next to the original.
//!
//! So each file is copied byte-for-byte with exactly two mechanical edits:
//! `//!` at the start of a line becomes `//`, and the two `include_bytes!`
//! paths are made absolute. Nothing else is touched, and the copies are
//! regenerated from `crates/` on every build.

use std::path::{Path, PathBuf};

fn stage(src: &Path, dst: &Path) {
	println!("cargo:rerun-if-changed={}", src.display());
	let text = std::fs::read_to_string(src).unwrap_or_else(|e| panic!("reading {}: {e}", src.display()));
	let dir = src.parent().expect("a source file has a parent");
	let mut out = String::with_capacity(text.len() + 256);
	for line in text.lines() {
		let trimmed = line.trim_start();
		let line = if let Some(rest) = trimmed.strip_prefix("//!") {
			let indent = &line[..line.len() - trimmed.len()];
			format!("{indent}//{rest}")
		} else {
			line.to_string()
		};
		// `include_bytes!("x.bin")` -> `include_bytes!("<abs dir>/x.bin")`
		let line = if let Some(i) = line.find("include_bytes!(\"") {
			let head = &line[..i + "include_bytes!(\"".len()];
			let tail = &line[i + "include_bytes!(\"".len()..];
			format!("{head}{}/{tail}", dir.display())
		} else {
			line
		};
		out.push_str(&line);
		out.push('\n');
	}
	std::fs::write(dst, out).unwrap_or_else(|e| panic!("writing {}: {e}", dst.display()));
}

fn main() {
	let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
	let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../crates/vag-data/src");
	let root = root.canonicalize().unwrap_or_else(|e| panic!("locating {}: {e}", root.display()));
	stage(&root.join("tea.rs"), &out.join("tea.rs"));
	stage(&root.join("rod.rs"), &out.join("rod.rs"));
	stage(&root.join("rod/crack.rs"), &out.join("crack.rs"));
}
