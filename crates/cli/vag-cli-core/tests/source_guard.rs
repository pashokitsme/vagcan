//! What the CLI crates' source may not spell out by hand (CLAUDE.md, "Command output").
//!
//! A grep over the non-test source of `vag-cli-core`, `vag-cli-diag`, `vag-cli-measure`,
//! `vag-cli` and `vag-dash-cfg`: comments and `#[cfg(test)]` modules are left out, since a
//! formula in a doc comment and a test pinning a line are not output.

use std::path::{Path, PathBuf};

const CRATES: [&str; 5] = [
	"crates/cli/vag-cli-core",
	"crates/cli/vag-cli-diag",
	"crates/cli/vag-cli-measure",
	"crates/cli/vag-cli",
	"crates/dash/vag-dash-cfg",
];

fn repo() -> PathBuf {
	Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
	let Ok(entries) = std::fs::read_dir(dir) else { return };
	for entry in entries.flatten() {
		let path = entry.path();
		if path.is_dir() {
			rust_files(&path, out);
		} else if path.extension().is_some_and(|e| e == "rs") {
			out.push(path);
		}
	}
}

/// Every non-comment line of non-test code in the CLI crates' `src/`, as (file, line number,
/// line), the files whose path ends in one of `skip` left out.
fn code_lines(skip: &[&str]) -> Vec<(String, usize, String)> {
	let mut files = Vec::new();
	for krate in CRATES {
		rust_files(&repo().join(krate).join("src"), &mut files);
	}
	assert!(
		files.len() > 50,
		"found only {} files: is the repository where this test thinks?",
		files.len()
	);
	let mut lines = Vec::new();
	for file in files {
		let shown = file.strip_prefix(repo()).unwrap_or(&file).display().to_string();
		if skip.iter().any(|s| shown.ends_with(s)) {
			continue;
		}
		let text = std::fs::read_to_string(&file).expect("a source file reads");
		for (n, line) in text.lines().enumerate() {
			if line.trim_start().starts_with("#[cfg(test)]") {
				break;
			}
			if line.trim_start().starts_with("//") {
				continue;
			}
			lines.push((shown.clone(), n + 1, line.to_string()));
		}
	}
	lines
}

/// `·` and `•` come from `vag_cli_core::marks`, never a literal (owner, 2026-10-21).
#[test]
fn no_literal_glyphs() {
	let found: Vec<String> = code_lines(&["vag-cli-core/src/marks.rs"])
		.into_iter()
		.filter(|(_, _, line)| line.contains('·') || line.contains('•'))
		.map(|(file, n, line)| format!("{file}:{n}: {}", line.trim()))
		.collect();
	assert!(
		found.is_empty(),
		"a literal separator: use `vag_cli_core::marks::SEP` / `facts` / `DOT`\n{}",
		found.join("\n")
	);
}
