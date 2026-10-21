//! The glyphs every command prints, in one place (owner, 2026-10-21: «надо закрепить сепаратор
//! в коде, чтобы не повторяться. Не писать •, а использовать ·»).
//!
//! A CLI crate writes none of these as a literal: it names the constant, so a line in `faults`
//! and a line in `setup` cannot drift to two separators. `no_literal_glyphs`
//! (`vag-cli-core/tests/source_guard.rs`) holds the line for `·` and `•`. The board draws its own
//! glyphs; the firmware and the render crates are not covered.

/// Between facts on one line: `2 stored · 1 failing now · 15 units read`.
///
/// Used by every summary line (`faults`, `info`, `units`, `setup`, `dev vcds …`, `dev sniff`,
/// `dev recording …`, `measure`), by `watch`'s and `dev dash replay`'s status lines and by the
/// chart's legend. Build a line with [`facts`] rather than by hand.
pub const SEP: &str = " · ";

/// The middle dot alone: an empty cell of a bar (`ui::bars`, `dashcfg`'s brightness) and a value
/// not read yet (`measure`'s live table).
pub const DOT: char = '·';

/// The first column of a row that succeeded, in a list where a row can fail (CLAUDE.md,
/// "Command output"). No command prints one yet; one that does takes this.
pub const OK: &str = "✓";

/// The first column of a row that failed; before a fault code failing now (`faults`).
pub const FAIL: &str = "✗";

/// Before a fault code stored and not failing now (`faults`).
pub const STORED: &str = "·";

/// Before a fault code the unit lists and has not confirmed (`faults --all`).
pub const UNCONFIRMED: &str = "○";

/// Where text was cut to fit: `table`'s cells, the menu's and the picker's lines, a PDU's bytes
/// in `dev sniff` and in a property dump, a column's values in `dev recording discover`.
pub const ELLIPSIS: char = '…';

/// `parts` as one line of facts, [`SEP`] between them, an empty part left out.
///
/// ```
/// use vag_cli_core::marks::facts;
/// assert_eq!(facts(["2 stored", "", "15 units read"]), "2 stored · 15 units read");
/// ```
pub fn facts<S: AsRef<str>>(parts: impl IntoIterator<Item = S>) -> String {
	let mut line = String::new();
	for part in parts {
		let part = part.as_ref();
		if part.is_empty() {
			continue;
		}
		if !line.is_empty() {
			line.push_str(SEP);
		}
		line.push_str(part);
	}
	line
}

#[cfg(test)]
mod tests {
	use super::{SEP, facts};

	#[test]
	fn facts_are_joined_by_the_separator_and_empty_ones_dropped() {
		assert_eq!(SEP, " \u{b7} ");
		assert_eq!(facts(["a", "b", "c"]), "a · b · c");
		assert_eq!(facts(["", "a", "", "b", ""]), "a · b");
		assert_eq!(facts(Vec::<String>::new()), "");
		assert_eq!(facts([String::from("one")]), "one");
	}
}
