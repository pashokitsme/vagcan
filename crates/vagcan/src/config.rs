//! `~/.vagcan/config.toml` — the settings that are not about one car, and the
//! per-car marks that have to live *somewhere* a person can edit.
//!
//! TOML rather than JSON because this file is written by hand as often as by
//! the tool: it takes comments, it does not mind a trailing comma, and a person
//! who opens it can see what the options are. The old `config.json` is migrated
//! on first read and removed, so there is one file claiming these facts rather
//! than two that disagree by next month.
//!
//! ```toml
//! # Which project a bare command means.
//! project = "SK37X"
//!
//! # Which column of ~/.vagcan/names.csv channel names are read from.
//! language = "ru"
//!
//! # Channels marked with `f` in `watch`, per car, keyed by VIN.
//! [favourites]
//! XW8AD4NE9JH008917 = ["7E0:202A:0", "7E1:380A:0"]
//! ```
//!
//! **The document is kept as a `toml::Table`, not deserialized into a struct.**
//! A struct would silently drop anything this version does not know about — a
//! key from a newer build, or a note somebody left themselves — and this file
//! is theirs to write in. Every accessor below reads and writes one key of it.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Which language a channel name is shown in.
///
/// Only the languages [`crate::glossary`] has columns for. It is a closed set on
/// purpose: a value nobody wrote a column for would silently fall back to the
/// vendor wording and look like the glossary had failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Language {
	#[default]
	En,
	Ru,
}

impl Language {
	pub fn code(self) -> &'static str {
		match self {
			Language::En => "en",
			Language::Ru => "ru",
		}
	}

	/// Parse a code, or `None` for anything else — including a language this
	/// build has no column for, which is a setting that cannot be honoured
	/// rather than one to approximate.
	pub fn parse(code: &str) -> Option<Language> {
		match code.trim().to_ascii_lowercase().as_str() {
			"en" => Some(Language::En),
			"ru" => Some(Language::Ru),
			_ => None,
		}
	}
}

/// Where the settings live.
pub fn path() -> Result<PathBuf> {
	Ok(crate::datadir::vagcan_dir()?.join("config.toml"))
}

/// The whole document, or an empty one.
///
/// A file that will not parse is an empty document rather than an error: these
/// are preferences, and failing a drive because a settings file has a typo in it
/// is the wrong trade — the same judgement [`crate::watch::favourites`] made
/// when it lived in its own file.
pub fn load() -> toml::Table {
	match path() {
		Ok(path) => load_at(&path),
		Err(_) => toml::Table::new(),
	}
}

fn load_at(path: &Path) -> toml::Table {
	migrate_json(path);
	std::fs::read_to_string(path)
		.ok()
		.and_then(|text| text.parse::<toml::Table>().ok())
		.unwrap_or_default()
}

/// Carry `config.json` over to `config.toml`, once.
///
/// Best-effort throughout. The only key the JSON file ever held is `project`,
/// and a machine that cannot be migrated is a machine that names its project
/// again — annoying, not damaging. The JSON is removed only after the TOML is
/// written, so an interrupted migration leaves the old file in place.
fn migrate_json(toml_path: &Path) {
	if toml_path.exists() {
		return;
	}
	let Some(dir) = toml_path.parent() else { return };
	let json = dir.join("config.json");
	let Ok(text) = std::fs::read_to_string(&json) else { return };
	let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
		return;
	};
	let mut document = toml::Table::new();
	if let Some(project) = value.get("project").and_then(|v| v.as_str()) {
		document.insert("project".into(), toml::Value::String(project.to_owned()));
	}
	if write_at(toml_path, &document).is_ok() {
		let _ = std::fs::remove_file(&json);
	}
}

/// Write the document back.
pub fn save(document: &toml::Table) -> Result<()> {
	write_at(&path()?, document)
}

fn write_at(path: &Path, document: &toml::Table) -> Result<()> {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
	}
	std::fs::write(path, toml::to_string_pretty(document)?).with_context(|| format!("writing {}", path.display()))
}

/// Which project a bare command means, if the file says.
pub fn project(document: &toml::Table) -> Option<String> {
	document.get("project")?.as_str().map(str::to_owned)
}

/// Write down which project a bare command means from now on.
pub fn set_project(id: &str) -> Result<()> {
	let mut document = load();
	document.insert("project".into(), toml::Value::String(id.to_owned()));
	save(&document)
}

/// Which language channel names are shown in.
///
/// An unreadable or unknown value is the default rather than an error, and the
/// caller is expected to say so — see [`language_complaint`]. A settings file
/// that quietly did something other than what it says is worse than one that is
/// ignored out loud.
pub fn language(document: &toml::Table) -> Language {
	document
		.get("language")
		.and_then(|v| v.as_str())
		.and_then(Language::parse)
		.unwrap_or_default()
}

/// What to say about a `language` this build cannot honour, if there is one.
pub fn language_complaint(document: &toml::Table) -> Option<String> {
	let written = document.get("language")?.as_str()?;
	match Language::parse(written) {
		Some(_) => None,
		None => Some(format!(
			"config.toml sets language = {written:?}, which this build has no column for — using {}. \
			 The glossary ~/.vagcan/names.csv has one column per language; add one and name it here.",
			Language::default().code()
		)),
	}
}

/// One car's favourite channels, as the keys `watch` writes.
pub fn favourites(document: &toml::Table, vin: &str) -> Vec<String> {
	document
		.get("favourites")
		.and_then(|v| v.as_table())
		.and_then(|table| table.get(vin))
		.and_then(|v| v.as_array())
		.map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
		.unwrap_or_default()
}

/// Replace one car's favourites, leaving every other car's alone.
pub fn set_favourites(document: &mut toml::Table, vin: &str, keys: &[String]) {
	let table = document
		.entry("favourites".to_string())
		.or_insert_with(|| toml::Value::Table(toml::Table::new()));
	// A `favourites` key that is not a table is somebody's edit, and replacing
	// it wholesale would throw that away without saying so. Replaced only when
	// it is already the shape this owns.
	if !table.is_table() {
		*table = toml::Value::Table(toml::Table::new());
	}
	if let Some(table) = table.as_table_mut() {
		let list: Vec<toml::Value> = keys.iter().map(|k| toml::Value::String(k.clone())).collect();
		table.insert(vin.to_owned(), toml::Value::Array(list));
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn temp(tag: &str) -> PathBuf {
		let path = std::env::temp_dir().join(format!("vagcan-config-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
		let _ = std::fs::remove_dir_all(&path);
		std::fs::create_dir_all(&path).unwrap();
		path
	}

	#[test]
	fn a_json_config_becomes_a_toml_one_and_the_json_goes() {
		// Two files claiming which project a bare command means would disagree
		// within a month, so the migration finishes rather than leaving both.
		let dir = temp("migrate");
		std::fs::write(dir.join("config.json"), r#"{"project": "SK37X"}"#).unwrap();
		let document = load_at(&dir.join("config.toml"));

		assert_eq!(project(&document).as_deref(), Some("SK37X"));
		assert!(dir.join("config.toml").is_file());
		assert!(!dir.join("config.json").exists(), "the old file is gone, not shadowed");
		let _ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn hand_written_keys_survive_being_written_by_the_tool() {
		// This file is a person's to write in. A struct would drop anything
		// this build does not know about — a key from a newer version, or a
		// note somebody left themselves — the moment `watch` saved a favourite.
		let dir = temp("preserve");
		let path = dir.join("config.toml");
		std::fs::write(&path, "project = \"SK37X\"\nmy_own_note = \"do not lose me\"\n").unwrap();

		let mut document = load_at(&path);
		set_favourites(&mut document, "XW8AD4NE9JH008917", &["7E0:202A:0".to_string()]);
		write_at(&path, &document).unwrap();

		let back = load_at(&path);
		assert_eq!(project(&back).as_deref(), Some("SK37X"));
		assert_eq!(back.get("my_own_note").and_then(|v| v.as_str()), Some("do not lose me"));
		assert_eq!(favourites(&back, "XW8AD4NE9JH008917"), vec!["7E0:202A:0"]);
		let _ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn one_cars_marks_never_touch_anothers() {
		let mut document = toml::Table::new();
		set_favourites(&mut document, "AAA", &["7E0:202A:0".to_string()]);
		set_favourites(&mut document, "BBB", &["713:1001:0".to_string()]);
		set_favourites(&mut document, "AAA", &["7E1:380A:0".to_string()]);

		assert_eq!(favourites(&document, "AAA"), vec!["7E1:380A:0"]);
		assert_eq!(favourites(&document, "BBB"), vec!["713:1001:0"]);
		assert_eq!(favourites(&document, "CCC"), Vec::<String>::new());
	}

	#[test]
	fn a_language_this_build_cannot_honour_is_said_out_loud() {
		// Falling back silently would make the glossary look broken: names
		// would arrive in the vendor's wording and nothing would say why.
		let document: toml::Table = "language = \"de\"\n".parse().unwrap();
		assert_eq!(language(&document), Language::En);
		let why = language_complaint(&document).expect("it complains");
		assert!(why.contains("\"de\""), "{why}");
		assert!(why.contains("names.csv"), "{why}");

		let fine: toml::Table = "language = \"ru\"\n".parse().unwrap();
		assert_eq!(language(&fine), Language::Ru);
		assert_eq!(language_complaint(&fine), None);
	}

	#[test]
	fn a_settings_file_that_will_not_parse_is_no_settings_rather_than_a_lost_drive() {
		let dir = temp("broken");
		let path = dir.join("config.toml");
		std::fs::write(&path, "this is not toml = = =").unwrap();
		let document = load_at(&path);
		assert!(document.is_empty());
		assert_eq!(language(&document), Language::En);
		let _ = std::fs::remove_dir_all(&dir);
	}
}
