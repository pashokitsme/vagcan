//! What the CLI crates' source may not spell out by hand (CLAUDE.md, "Command output").
//!
//! A grep over the non-test source of `vag-cli-core`, `vag-cli-diag`, `vag-cli-measure`,
//! `vag-cli` and `vag-dash-cfg`: comments and `#[cfg(test)]` items are left out, since a
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
///
/// A `#[cfg(test)]` item — `mod tests`, or one `fn`, `use` or `impl` — is left out up to its
/// end, and only it: the code after it is scanned.
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
		let mut skipping: Option<Skip> = None;
		for (n, line) in text.lines().enumerate() {
			if let Some(skip) = &mut skipping {
				if skip.done(line) {
					skipping = None;
				}
				continue;
			}
			let trimmed = line.trim_start();
			if trimmed.starts_with("#[cfg(test)]") {
				skipping = Some(Skip::default());
				continue;
			}
			if trimmed.starts_with("//") {
				continue;
			}
			lines.push((shown.clone(), n + 1, line.to_string()));
		}
	}
	lines
}

/// The item under a `#[cfg(test)]`, followed to its end: its other attributes, then a line
/// ending in `;` before any brace opens, or the line that closes every brace it opened.
#[derive(Default)]
struct Skip {
	depth: i64,
	opened: bool,
}

impl Skip {
	/// Whether `line` is the item's last line.
	fn done(&mut self, line: &str) -> bool {
		let trimmed = line.trim();
		if !self.opened && (trimmed.is_empty() || trimmed.starts_with("#[") || trimmed.starts_with("//")) {
			return false;
		}
		for c in code_part(line).chars() {
			match c {
				'{' => {
					self.depth += 1;
					self.opened = true;
				}
				'}' => self.depth -= 1,
				_ => {}
			}
		}
		match self.opened {
			true => self.depth <= 0,
			false => trimmed.ends_with(';'),
		}
	}
}

/// `line` without its string and char literals and its trailing comment, for counting braces.
fn code_part(line: &str) -> String {
	let chars: Vec<char> = line.chars().collect();
	let mut out = String::new();
	let mut i = 0;
	while i < chars.len() {
		let c = chars[i];
		// A char literal is `'x'` or starts `'\`; a lone `'` is a lifetime.
		let char_literal = c == '\'' && (chars.get(i + 1) == Some(&'\\') || chars.get(i + 2) == Some(&'\''));
		if c == '"' || char_literal {
			i += 1;
			while i < chars.len() && chars[i] != c {
				i += if chars[i] == '\\' { 2 } else { 1 };
			}
		} else if c == '/' && chars.get(i + 1) == Some(&'/') {
			break;
		} else {
			out.push(c);
		}
		i += 1;
	}
	out
}

/// The separator or the bullet in `line`, as itself or spelled as an escape.
fn glyph_in(line: &str) -> bool {
	let lower = line.to_lowercase();
	line.contains('·') || line.contains('•') || ["\\u{b7}", "\\u{00b7}", "\\u{2022}"].iter().any(|e| lower.contains(e))
}

#[test]
fn a_cfg_test_item_is_skipped_to_its_end_and_no_further() {
	let mut f = Skip::default();
	assert!(!f.done("fn helper() {"));
	assert!(!f.done("\tlet open = \"{\";"));
	assert!(!f.done("\tlet c = '{';"));
	assert!(f.done("}"));
	let mut u = Skip::default();
	assert!(!u.done("#[allow(unused)]"));
	assert!(u.done("use std::fmt;"));
	let mut lifetime = Skip::default();
	assert!(!lifetime.done("fn x<'a>(s: &'a str) -> &'a str {"));
	assert!(lifetime.done("}"));
	assert!(glyph_in("let s = '\\u{B7}';") && glyph_in("\"\\u{2022}\"") && glyph_in("\"\\u{00b7}\""));
	assert!(!glyph_in("let s = SEP;"));
}

/// `·` and `•` come from `vag_cli_core::marks`, never a literal nor an escape (owner,
/// 2026-10-21). `vcdslog.rs` is left out: its two are bytes of the CP1251 table, not output.
#[test]
fn no_literal_glyphs() {
	let found: Vec<String> = code_lines(&["vag-cli-core/src/marks.rs", "vag-cli-core/src/vcdslog.rs"])
		.into_iter()
		.filter(|(_, _, line)| glyph_in(line))
		.map(|(file, n, line)| format!("{file}:{n}: {}", line.trim()))
		.collect();
	assert!(
		found.is_empty(),
		"a literal separator: use `vag_cli_core::marks::SEP` / `facts` / `DOT`\n{}",
		found.join("\n")
	);
}
