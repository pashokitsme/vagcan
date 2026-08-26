//! `~/.vagcan/names.csv` — the owner's own wording for a channel, in more than
//! one language.
//!
//! **Keyed by text id, never by identifier.** `IDE00022` and `MAS18568` are
//! VW's own keys for a piece of text, shared across projects and cars; a
//! dictionary keyed by them is a dictionary, and it moves to the next car
//! untouched. A table keyed by `(unit, identifier)` would be a table about one
//! vehicle in a file the tool ships around, which is the thing `CLAUDE.md`
//! forbids outright.
//!
//! It exists because the vendor wording is written for a diagnostic engineer.
//! `Brake_pedal_information_plausibility` is accurate and unreadable at an open
//! driver's door, and neither ODIS nor VCDS is going to fix that. This file wins
//! over both — see [`crate::extracted::Extracted::name_of`] — and anything it
//! does not mention falls through to them unchanged, so it is worth writing one
//! line at a time.
//!
//! ```csv
//! text_id,en,ru
//! IDE00022,"Boost pressure, actual","Давление наддува, фактическое"
//! MAS18568,Oil temperature,Температура масла
//! ```
//!
//! One file for every project, because the ids are. The column is chosen by
//! `language` in [`crate::config`]; an empty cell is not a name and falls
//! through, so a half-translated file is a useful file.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::Language;

/// The column heading each language is written under.
pub const HEADINGS: [(&str, Language); 2] = [("en", Language::En), ("ru", Language::Ru)];

/// Where the glossary lives.
pub fn path() -> anyhow::Result<PathBuf> {
	Ok(crate::datadir::vagcan_dir()?.join("names.csv"))
}

/// The owner's names in one language, or nothing at all.
///
/// A missing file is the ordinary state — most people never write one — and a
/// file that will not parse costs the lines that will not parse and nothing
/// else. Neither is an error: this is wording, and no run should end over it.
pub fn load(language: Language) -> BTreeMap<String, String> {
	let Ok(path) = path() else { return BTreeMap::new() };
	let Ok(text) = std::fs::read_to_string(&path) else {
		return BTreeMap::new();
	};
	parse(&text, language)
}

/// Read a glossary's text, taking one language's column.
///
/// The header row names the columns, so a file may carry more languages than
/// this build knows and still be read for the one it was asked for. A row whose
/// cell for that language is blank is skipped rather than stored as an empty
/// name — the whole point of falling through is that a missing translation
/// leaves the vendor's wording in place.
pub fn parse(text: &str, language: Language) -> BTreeMap<String, String> {
	let mut rows = read_csv(text).into_iter();
	let Some(header) = rows.next() else { return BTreeMap::new() };
	let column = |name: &str| header.iter().position(|cell| cell.trim().eq_ignore_ascii_case(name));
	let Some(id_at) = column("text_id") else { return BTreeMap::new() };
	let Some(name_at) = column(language.code()) else {
		return BTreeMap::new();
	};

	let mut out = BTreeMap::new();
	for row in rows {
		let (Some(id), Some(name)) = (row.get(id_at), row.get(name_at)) else {
			continue;
		};
		let (id, name) = (id.trim(), name.trim());
		if id.is_empty() || name.is_empty() {
			continue;
		}
		out.insert(id.to_owned(), name.to_owned());
	}
	out
}

/// One field, quoted only when it has to be.
///
/// A measurement name contains a comma often enough that this is not an edge
/// case: `Boost pressure, actual` is one of the first channels anybody looks at.
fn quote(field: &str) -> String {
	if field.contains([',', '"', '\n', '\r']) {
		return format!("\"{}\"", field.replace('"', "\"\""));
	}
	field.to_owned()
}

/// RFC 4180, as much of it as a hand-edited file needs.
///
/// Written here rather than taken as a dependency: it is quoting, doubled
/// quotes and both line endings, and the alternative is a crate for forty lines.
/// A row that ends inside an open quote keeps what it has rather than being
/// dropped — somebody's half-finished edit should still load the lines above it.
fn read_csv(text: &str) -> Vec<Vec<String>> {
	let mut rows = Vec::new();
	let mut row = Vec::new();
	let mut field = String::new();
	let mut quoted = false;
	let mut chars = text.chars().peekable();

	while let Some(c) = chars.next() {
		match (quoted, c) {
			(true, '"') => match chars.peek() {
				Some('"') => {
					field.push('"');
					chars.next();
				}
				_ => quoted = false,
			},
			(true, c) => field.push(c),
			(false, '"') if field.is_empty() => quoted = true,
			(false, ',') => row.push(std::mem::take(&mut field)),
			(false, '\r') => {
				if chars.peek() == Some(&'\n') {
					chars.next();
				}
				row.push(std::mem::take(&mut field));
				rows.push(std::mem::take(&mut row));
			}
			(false, '\n') => {
				row.push(std::mem::take(&mut field));
				rows.push(std::mem::take(&mut row));
			}
			(false, c) => field.push(c),
		}
	}
	if !field.is_empty() || !row.is_empty() {
		row.push(field);
		rows.push(row);
	}
	// A trailing newline leaves one empty row, which is not a line anybody wrote.
	rows.retain(|row| row.iter().any(|cell| !cell.trim().is_empty()));
	rows
}

/// Write or refresh `~/.vagcan/names.csv` from what this project knows.
///
/// **Never destructive.** Every line already in the file is kept exactly as it
/// is; ids the project knows and the file does not are appended with empty
/// cells. Regenerating after an afternoon of translating must not cost the
/// afternoon.
///
/// The seed carries a fourth column, `current`, holding what the channel is
/// called today. It is not read back — [`parse`] takes only the columns the
/// header names as languages — and it is there because translating a list of
/// bare ids is not something anybody can do.
pub fn seed(project: &crate::project::Project) -> anyhow::Result<Seeded> {
	let path = path()?;
	let existing = std::fs::read_to_string(&path).unwrap_or_default();
	let mut rows: BTreeMap<String, BTreeMap<Language, String>> = BTreeMap::new();
	let mut current: BTreeMap<String, String> = BTreeMap::new();

	// What is already written, in every language this build has a column for.
	for (_, language) in HEADINGS {
		for (id, name) in parse(&existing, language) {
			rows.entry(id).or_default().insert(language, name);
		}
	}
	let before = rows.len();

	// Everything the project can name, whether or not it is written yet.
	for (id, name) in vag_db::text_ids(&project.cache()).unwrap_or_default() {
		rows.entry(id.clone()).or_default();
		current.insert(id, name);
	}
	for (id, name) in crate::extracted::open(project).names() {
		rows.entry(id.clone()).or_default();
		current.entry(id).or_insert(name);
	}

	let table: Vec<(String, BTreeMap<Language, String>)> = rows.into_iter().collect();
	let mut text = render_with_current(&table, &current);
	if !text.ends_with('\n') {
		text.push('\n');
	}
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent)?;
	}
	std::fs::write(&path, text)?;
	Ok(Seeded {
		path,
		total: table.len(),
		added: table.len().saturating_sub(before),
		translated: before,
	})
}

/// What a [`seed`] run did.
pub struct Seeded {
	pub path: PathBuf,
	/// Lines in the file afterwards.
	pub total: usize,
	/// Lines this run added.
	pub added: usize,
	/// Lines that already carried at least one translation.
	pub translated: usize,
}

/// A glossary file's whole text, ready to write.
///
/// Every language gets a column whether or not it has anything in it, so the
/// file shows what can be filled in rather than only what already is. The
/// `current` column holds what each channel is called today; it is written to be
/// read by a person and is never read back — [`parse`] takes only the columns
/// the header names as languages.
fn render_with_current(rows: &[(String, BTreeMap<Language, String>)], current: &BTreeMap<String, String>) -> String {
	let mut out = String::new();
	out.push_str("text_id");
	for (heading, _) in HEADINGS {
		out.push(',');
		out.push_str(heading);
	}
	out.push_str(",current\n");
	for (id, names) in rows {
		out.push_str(&quote(id));
		for (_, language) in HEADINGS {
			out.push(',');
			out.push_str(&quote(names.get(&language).map(String::as_str).unwrap_or("")));
		}
		out.push(',');
		out.push_str(&quote(current.get(id).map(String::as_str).unwrap_or("")));
		out.push('\n');
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_name_with_a_comma_survives_the_round_trip() {
		// Not an edge case: `Boost pressure, actual` is among the first channels
		// anybody looks at, and a reader that split on commas would file half
		// of it as a translation.
		let mut names = BTreeMap::new();
		names.insert(Language::En, "Boost pressure, actual".to_string());
		names.insert(Language::Ru, "Давление наддува, фактическое".to_string());
		let text = render_with_current(&[("IDE00022".to_string(), names)], &BTreeMap::new());

		assert!(text.contains("\"Boost pressure, actual\""), "{text}");
		assert_eq!(
			parse(&text, Language::En).get("IDE00022").map(String::as_str),
			Some("Boost pressure, actual")
		);
		assert_eq!(
			parse(&text, Language::Ru).get("IDE00022").map(String::as_str),
			Some("Давление наддува, фактическое")
		);
	}

	#[test]
	fn a_quote_inside_a_name_is_doubled_and_read_back() {
		let mut names = BTreeMap::new();
		names.insert(Language::En, "Sensor \"G62\" reading".to_string());
		let text = render_with_current(&[("IDE00025".to_string(), names)], &BTreeMap::new());
		assert_eq!(
			parse(&text, Language::En).get("IDE00025").map(String::as_str),
			Some("Sensor \"G62\" reading")
		);
	}

	#[test]
	fn a_blank_cell_falls_through_instead_of_naming_a_channel_nothing() {
		// The whole reason this file is worth writing one line at a time: a
		// half-translated glossary must leave the vendor's wording in place,
		// not replace it with an empty label.
		let text = "text_id,en,ru\nIDE00022,Boost pressure,\nMAS18568,,Температура масла\n";
		let en = parse(text, Language::En);
		let ru = parse(text, Language::Ru);
		assert_eq!(en.get("IDE00022").map(String::as_str), Some("Boost pressure"));
		assert_eq!(en.get("MAS18568"), None, "no English for this one yet");
		assert_eq!(ru.get("MAS18568").map(String::as_str), Some("Температура масла"));
		assert_eq!(ru.get("IDE00022"), None);
	}

	#[test]
	fn a_file_may_carry_languages_this_build_has_no_column_for() {
		// The header names the columns, so somebody adding `de` does not break
		// the two this build reads — and does not get German by accident.
		let text = "text_id,en,de,ru\nIDE00022,Boost,Ladedruck,Наддув\n";
		assert_eq!(parse(text, Language::En).get("IDE00022").map(String::as_str), Some("Boost"));
		assert_eq!(parse(text, Language::Ru).get("IDE00022").map(String::as_str), Some("Наддув"));
	}

	#[test]
	fn a_file_with_no_column_for_the_language_asked_for_is_no_names() {
		// Rather than the first column, or the id, or anything else that would
		// put text on screen that nobody wrote for that language.
		let text = "text_id,en\nIDE00022,Boost pressure\n";
		assert!(parse(text, Language::Ru).is_empty());
	}

	#[test]
	fn windows_line_endings_and_a_trailing_newline_are_ordinary() {
		// This file is hand-edited, and on more than one platform.
		let text = "text_id,en,ru\r\nIDE00022,Boost,Наддув\r\n\r\n";
		let en = parse(text, Language::En);
		assert_eq!(en.len(), 1);
		assert_eq!(en.get("IDE00022").map(String::as_str), Some("Boost"));
	}

	#[test]
	fn a_file_that_is_not_a_glossary_names_nothing() {
		for broken in ["", "nonsense", "a,b,c\n1,2,3\n"] {
			assert!(parse(broken, Language::En).is_empty(), "{broken:?}");
		}
	}
}
