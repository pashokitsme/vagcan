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
//! A name in that style — `Pedal_signal_plausibility_state`, say — is accurate
//! and unreadable at an open driver's door, and neither ODIS nor VCDS is going
//! to fix that. This file wins over both — see
//! [`crate::extracted::Extracted::name_of`] — and anything it does not mention
//! falls through to them unchanged, so it is worth writing one line at a time.
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

/// The owner's names in every language this build has a column for, or nothing at all.
///
/// Every column rather than the one `config.toml` names, because one reader asks for
/// another: a dash plan is labelled in its own `language` (`crate::dash::build`).
///
/// A missing file is the ordinary state — most people never write one — and a
/// file that will not parse costs the lines that will not parse and nothing
/// else. Neither is an error: this is wording, and no run should end over it.
pub fn load() -> BTreeMap<Language, BTreeMap<String, String>> {
	let Ok(path) = path() else { return BTreeMap::new() };
	let Ok(text) = std::fs::read_to_string(&path) else {
		return BTreeMap::new();
	};
	parse_all(&text)
}

/// [`parse`], for every language this build has a column for.
pub fn parse_all(text: &str) -> BTreeMap<Language, BTreeMap<String, String>> {
	HEADINGS.iter().map(|&(_, language)| (language, parse(text, language))).collect()
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
	// Excel's "CSV UTF-8" starts the file with a byte-order mark, and `trim`
	// keeps U+FEFF — so the header would read `\u{FEFF}text_id` and the whole
	// file would name nothing.
	let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
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
/// **Never destructive.** Every translation in a column this build reads (`en`,
/// `ru`) is kept; text ids the project's channels carry and the file does not
/// are appended with empty cells. Regenerating after an afternoon of
/// translating must not cost the afternoon. A blank line holds no work, and is
/// written again only while a channel still carries its id. A file this cannot
/// write back whole is refused rather than written over: one that is not
/// UTF-8, has no `text_id` column, or has a column this build does not write
/// (a language it has no heading for, a notes column). A byte-order mark is
/// kept, because Excel needs it to open the file as UTF-8.
///
/// The seed carries a fourth column, `current`, holding what the channel is
/// called today. It is not read back — [`parse`] takes only the columns the
/// header names as languages — and it is there because translating a list of
/// bare ids is not something anybody can do.
pub fn seed(project: &crate::project::Project) -> anyhow::Result<Seeded> {
	seed_into(project, path()?)
}

/// [`seed`], into a file named by the caller — a test's, never the owner's.
fn seed_into(project: &crate::project::Project, path: PathBuf) -> anyhow::Result<Seeded> {
	// A file that is there and does not read is somebody's work in a form this
	// cannot parse — saved as Windows-1251, say. Treating it as empty would
	// write the project's ids over every translation in it.
	let existing = match std::fs::read_to_string(&path) {
		Ok(text) => text,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
		Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
			anyhow::bail!("{} is not UTF-8 text; not writing over it", path.display())
		}
		Err(e) => anyhow::bail!("reading {}: {e}", path.display()),
	};
	if !existing.trim().is_empty() {
		let header = read_csv(&existing).into_iter().next().unwrap_or_default();
		let cells: Vec<String> = header.iter().map(|cell| cell.trim().to_ascii_lowercase()).collect();
		if !cells.iter().any(|cell| cell == "text_id") {
			anyhow::bail!(
				"{} has no `text_id` column, so none of its lines can be kept; not writing over it",
				path.display()
			);
		}
		let written = |cell: &str| cell == "text_id" || cell == "current" || HEADINGS.iter().any(|(heading, _)| cell == *heading);
		if let Some(other) = cells.iter().find(|cell| !written(cell)) {
			anyhow::bail!(
				"{} has a column `{other}` this build does not write back; not writing over it",
				path.display()
			);
		}
	}
	let mut rows: BTreeMap<String, BTreeMap<Language, String>> = BTreeMap::new();
	let mut current: BTreeMap<String, String> = BTreeMap::new();

	// What is already written, in every language this build has a column for.
	for (_, language) in HEADINGS {
		for (id, name) in parse(&existing, language) {
			rows.entry(id).or_default().insert(language, name);
		}
	}
	let translated = rows.len();

	// Every text id a channel of this project carries, whether or not it is
	// written yet.
	for (id, name) in vag_data_db::text_ids(&project.cache()).unwrap_or_default() {
		rows.entry(id.clone()).or_default();
		current.insert(id, name);
	}
	// VCDS's wording, for rows that are here already — never a row of its own.
	// `names.json` is keyed by the text table's six-digit record ids, and no
	// channel carries one: a channel's text id is VW's (`IDE#####`), so a row
	// keyed by a record id is a row nothing looks up. While a solver read a
	// quarter of the table that cost 14,738 such rows; read exactly, it would be
	// every record — 195,910, enum states, units and countries among them — in
	// a file a person is meant to go through by hand.
	let vcds = crate::extracted::open(project).names();
	for id in rows.keys() {
		if let Some(name) = vcds.get(id) {
			current.entry(id.clone()).or_insert_with(|| name.clone());
		}
	}

	let table: Vec<(String, BTreeMap<Language, String>)> = rows.into_iter().collect();
	let mut text = render_with_current(&table, &current);
	if !text.ends_with('\n') {
		text.push('\n');
	}
	if existing.starts_with('\u{FEFF}') {
		text.insert(0, '\u{FEFF}');
	}
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent)?;
	}
	std::fs::write(&path, text)?;
	Ok(Seeded {
		path,
		total: table.len(),
		blank: table.len().saturating_sub(translated),
		translated,
	})
}

/// What a [`seed`] run did.
pub struct Seeded {
	pub path: PathBuf,
	/// Lines in the file afterwards.
	pub total: usize,
	/// Lines still waiting for a translation. **Not "added this run"** — a
	/// re-run of an unchanged file reports the same number, because that is
	/// what it counts: the work left, not the work just done.
	pub blank: usize,
	/// Lines that already carry at least one translation.
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
	fn vcds_wording_fills_rows_that_are_here_and_adds_none() {
		// `names.json` is keyed by the text table's record ids, which no channel
		// carries. Read exactly it holds every record, so seeding a row per key
		// would bury the ids a person can use under ~196,000 they cannot.
		let here = tempfile::tempdir().unwrap();
		let project = crate::project::Project {
			id: "TEST".to_string(),
			dir: here.path().to_path_buf(),
		};
		crate::project::record_source(
			&project,
			crate::project::SourceEntry {
				kind: vag_data_db::VCDS,
				path: "vcds".to_string(),
				version: None,
				detail: None,
			},
		)
		.unwrap();
		// Invented records: the text table's words are Ross-Tech's.
		std::fs::write(
			project.names(),
			r#"{"000017": "Invented flap position", "000081": "Status", "910231": "Invented Shaft Speed Probe"}"#,
		)
		.unwrap();
		let glossary = here.path().join("names.csv");
		// A translated row keyed by a record id, and a blank one.
		std::fs::write(&glossary, "text_id,en,ru,current\n000017,My flap reading,,\n000081,,,Status\n").unwrap();

		let seeded = seed_into(&project, glossary.clone()).unwrap();
		let text = std::fs::read_to_string(&glossary).unwrap();
		assert!(text.contains("000017,My flap reading,,Invented flap position"), "{text}");
		assert!(!text.contains("910231"), "a record id no channel carries became a row: {text}");
		assert!(!text.contains("000081"), "a blank row nothing carries is not work to keep: {text}");
		assert_eq!((seeded.total, seeded.translated, seeded.blank), (1, 1, 0));
	}

	/// A project with nothing in it, for seeding a glossary that is not the owner's.
	fn empty_project(dir: &std::path::Path) -> crate::project::Project {
		crate::project::Project {
			id: "TEST".to_string(),
			dir: dir.to_path_buf(),
		}
	}

	#[test]
	fn a_byte_order_mark_does_not_hide_the_header() {
		let text = "\u{FEFF}text_id,en,ru\nIDE00022,Boost,\n";
		assert_eq!(parse(text, Language::En).get("IDE00022").map(String::as_str), Some("Boost"));
		let here = tempfile::tempdir().unwrap();
		let glossary = here.path().join("names.csv");
		std::fs::write(&glossary, text).unwrap();
		seed_into(&empty_project(here.path()), glossary.clone()).unwrap();
		let back = std::fs::read_to_string(&glossary).unwrap();
		assert!(back.contains("IDE00022,Boost"), "the translation was lost: {back}");
		assert!(back.starts_with('\u{FEFF}'), "the mark Excel needs for UTF-8 was dropped");
	}

	#[test]
	fn a_file_that_does_not_read_is_refused_rather_than_written_over() {
		let here = tempfile::tempdir().unwrap();
		let glossary = here.path().join("names.csv");
		// Windows-1251 Cyrillic: not UTF-8.
		let cp1251 = b"text_id,en,ru\nIDE00022,Boost,\xc4\xe0\xe2\xeb\xe5\xed\xe8\xe5\n".to_vec();
		std::fs::write(&glossary, &cp1251).unwrap();
		assert!(seed_into(&empty_project(here.path()), glossary.clone()).is_err());
		assert_eq!(std::fs::read(&glossary).unwrap(), cp1251);

		let headless = b"id,en\nIDE00022,Boost\n".to_vec();
		std::fs::write(&glossary, &headless).unwrap();
		assert!(seed_into(&empty_project(here.path()), glossary.clone()).is_err());
		assert_eq!(std::fs::read(&glossary).unwrap(), headless);

		// A column this build does not write back would be dropped by the rewrite.
		let german = b"text_id,en,de\nIDE00022,Boost,Ladedruck\n".to_vec();
		std::fs::write(&glossary, &german).unwrap();
		let Err(refused) = seed_into(&empty_project(here.path()), glossary.clone()) else {
			panic!("a column it would drop was written over");
		};
		let refused = refused.to_string();
		assert!(refused.contains("`de`"), "{refused}");
		assert_eq!(std::fs::read(&glossary).unwrap(), german);
	}

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
