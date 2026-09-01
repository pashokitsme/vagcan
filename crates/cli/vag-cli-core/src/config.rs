//! `~/.vagcan/config.toml` — the settings that are not about one car, and the
//! per-car marks that have to live *somewhere* a person can edit.
//!
//! TOML rather than JSON because this file is written by hand as often as by
//! the tool: it takes comments, and a person who opens it can see what the
//! options are. The old `config.json` is migrated on first read and removed, so
//! there is one file claiming these facts rather than two that disagree by next
//! month.
//!
//! **Read and written with `toml_edit`, which preserves formatting.** The first
//! version used `toml`, which parses to a value tree with nowhere to keep a
//! comment — so the first time `watch` saved a favourite it silently deleted
//! every note the owner had written, and the whole reason for leaving JSON with
//! it. Anything that edits this file goes through here and through `toml_edit`.
//!
//! YAML was considered and declined, and two of the three arguments against it
//! turned out to be weaker than they looked. **A key such as `7E0:202A:0` does
//! parse unquoted** — checked, not assumed. And while `serde_yaml` ships as
//! `0.9.34+deprecated` and the `serde_yml` fork as an unmaintained shim,
//! `yaml-rust2` is alive and YAML 1.2 compliant, and format-preserving editors
//! (`yaml-edit`, `yamlpatch`) do exist.
//!
//! What is left is narrower and still decided it. `language: no` is `false` in
//! YAML, and `no` is Norwegian — on the one field in this file that holds a
//! language code. The layout is whitespace-significant in a file people edit by
//! hand, where a wrong indent under `favourites` is a different document rather
//! than an error. And `toml_edit` is what cargo edits every `Cargo.toml` with,
//! against YAML editors at `0.2.3` and `1.30.0-rc1`: for a file this small the
//! gain is how it reads, and the risk is a settings file that stops loading.
//!
//! Worth revisiting if this file grows enough for the nesting to matter. It has
//! three keys.
//!
//! ```toml
//! # Which project a bare command means.
//! project = "SK37X"
//!
//! # Which column of ~/.vagcan/names.csv channel names are read from.
//! language = "ru"
//!
//! # How often `watch` asks the car, in hertz. `--hz` overrides it for one run.
//! hz = 10.0
//!
//! # Show each channel's own key at the end of its row.
//! show_key = false
//!
//! # Channels marked with `f` in `watch`, per car, keyed by VIN.
//! [favourites]
//! XW8AD4NE9JH008917 = ["7E0:202A:0", "7E1:380A:0"]
//!
//! # Channels the chart draws, per car.
//! [charted]
//! XW8AD4NE9JH008917 = ["7E0:202A:0"]
//! ```
//!
//! **The document is kept as a `toml::Table`, not deserialized into a struct.**
//! A struct would silently drop anything this version does not know about — a
//! key from a newer build, or a note somebody left themselves — and this file
//! is theirs to write in. Every accessor below reads and writes one key of it.

use std::path::{Path, PathBuf};

use toml_edit::DocumentMut as Document;

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

/// What a fresh settings file says, so that opening it shows the options.
///
/// Written only when there is no file at all. A person who has never seen this
/// tool's settings should be able to learn them by opening it, and an empty file
/// teaches nothing.
const FRESH: &str = "\
# vagcan settings. Yours to edit — comments and layout are kept.\n\
\n\
# Which car's data a bare command means, as a directory name under ~/.vagcan/data/.\n\
# `vagcan setup` writes this; VAGCAN_PROJECT and --project override it.\n\
# project = \"SK37X\"\n\
\n\
# Which column of ~/.vagcan/names.csv channel names are read from: \"en\" or \"ru\".\n\
# language = \"en\"\n\
\n\
# How often `watch` asks the car, in hertz. `--hz` overrides it for one run.\n\
# hz = 10.0\n\
\n\
# Show each channel's own key at the end of its row, so a name worth changing\n\
# can be found in names.csv.\n\
# show_key = false\n\
\n\
# Channels marked with `f` in `watch`, and the ones the chart draws. Per car,\n\
# keyed by VIN, written by the tool.\n\
# [favourites]\n\
# XW8AD4NE9JH008917 = [\"7E0:202A:0\"]\n";

/// The whole document, or an empty one.
///
/// A file that will not parse is an empty document rather than an error: these
/// are preferences, and failing a drive because a settings file has a typo in it
/// is the wrong trade — the same judgement [`crate::watch::favourites`] made
/// when it lived in its own file.
pub fn load() -> Document {
	match path() {
		Ok(path) => load_at(&path),
		Err(_) => Document::new(),
	}
}

fn load_at(path: &Path) -> Document {
	migrate_json(path);
	std::fs::read_to_string(path)
		.ok()
		.and_then(|text| text.parse::<Document>().ok())
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
	let Ok(text) = std::fs::read_to_string(&json) else {
		// No old file either: start from the commented template, so the first
		// person to open this learns what the options are.
		let _ = std::fs::create_dir_all(dir);
		let _ = std::fs::write(toml_path, FRESH);
		return;
	};
	let mut document: Document = FRESH.parse().unwrap_or_default();
	if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text)
		&& let Some(project) = value.get("project").and_then(|v| v.as_str())
	{
		document["project"] = toml_edit::value(project);
	}
	if write_at(toml_path, &document).is_ok() {
		let _ = std::fs::remove_file(&json);
	}
}

/// Write the document back.
pub fn save(document: &Document) -> Result<()> {
	write_at(&path()?, document)
}

fn write_at(path: &Path, document: &Document) -> Result<()> {
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
	}
	std::fs::write(path, document.to_string()).with_context(|| format!("writing {}", path.display()))
}

/// Which project a bare command means, if the file says.
pub fn project(document: &Document) -> Option<String> {
	document.get("project")?.as_str().map(str::to_owned)
}

/// Write down which project a bare command means from now on.
pub fn set_project(id: &str) -> Result<()> {
	let mut document = load();
	document["project"] = toml_edit::value(id);
	save(&document)
}

/// Which language channel names are shown in.
///
/// An unreadable or unknown value is the default rather than an error, and the
/// caller is expected to say so — see [`language_complaint`]. A settings file
/// that quietly did something other than what it says is worse than one that is
/// ignored out loud.
pub fn language(document: &Document) -> Language {
	document
		.get("language")
		.and_then(|v| v.as_str())
		.and_then(Language::parse)
		.unwrap_or_default()
}

/// What to say about a `language` this build cannot honour, if there is one.
pub fn language_complaint(document: &Document) -> Option<String> {
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

/// How often `watch` asks the car, in hertz.
///
/// A setting rather than only a flag because it is how somebody likes to work,
/// not something they decide per run — and because the useful values are found
/// by trying them on a car, which is exactly the moment a flag is not to hand.
/// Out-of-range or unreadable is [`DEFAULT_HZ`]: a rate of zero is a screen that
/// never updates, and a rate of a thousand is the same screen with the bus
/// saturated.
pub fn hz(document: &Document) -> f64 {
	document
		.get("hz")
		.and_then(|v| v.as_float().or_else(|| v.as_integer().map(|n| n as f64)))
		.filter(|hz| (MIN_HZ..=MAX_HZ).contains(hz))
		.unwrap_or(DEFAULT_HZ)
}

/// What `watch` polls at when nothing says otherwise.
pub const DEFAULT_HZ: f64 = 10.0;
/// Slower than this is a screen that looks frozen.
pub const MIN_HZ: f64 = 0.5;
/// Faster than this the bus, not the setting, is the limit — and asking for it
/// only fills the link with requests the car will not answer any sooner.
pub const MAX_HZ: f64 = 50.0;

/// Write down the rate for next time.
pub fn set_hz(document: &mut Document, hz: f64) {
	document["hz"] = toml_edit::value(hz.clamp(MIN_HZ, MAX_HZ));
}

/// Whether a channel's own key is shown at the end of its row.
///
/// Off by default: it is four columns of hex nobody reads while driving. On, it
/// is how somebody who has just seen a badly-named channel finds the line to
/// write in `names.csv` — which is the only way to answer "what is this row
/// actually called underneath".
pub fn show_key(document: &Document) -> bool {
	document.get("show_key").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Write down whether to show it.
pub fn set_show_key(document: &mut Document, show: bool) {
	document["show_key"] = toml_edit::value(show);
}

/// One car's charted channels, as the keys `watch` writes.
pub fn charted(document: &Document, vin: &str) -> Vec<String> {
	list(document, "charted", vin)
}

/// Replace one car's charted channels, leaving every other car's alone.
pub fn set_charted(document: &mut Document, vin: &str, keys: &[String]) {
	set_list(document, "charted", vin, keys)
}

/// One car's favourite channels, as the keys `watch` writes.
pub fn favourites(document: &Document, vin: &str) -> Vec<String> {
	list(document, "favourites", vin)
}

/// One car's entry in a per-VIN table of written keys.
fn list(document: &Document, table: &str, vin: &str) -> Vec<String> {
	document
		.get(table)
		.and_then(|v| v.as_table_like())
		.and_then(|table| table.get(vin))
		.and_then(|v| v.as_array())
		.map(|list| list.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
		.unwrap_or_default()
}

/// Replace one car's favourites, leaving every other car's alone.
pub fn set_favourites(document: &mut Document, vin: &str, keys: &[String]) {
	set_list(document, "favourites", vin, keys)
}

fn set_list(document: &mut Document, table: &str, vin: &str, keys: &[String]) {
	// A key of this name that is not a table is somebody's edit, and replacing
	// it wholesale would throw that away without saying so — but there is
	// nowhere else to put a car's marks, so it is replaced and only then.
	if !document.get(table).is_some_and(|v| v.is_table_like()) {
		document[table] = toml_edit::Item::Table(toml_edit::Table::new());
	}
	let mut array = toml_edit::Array::new();
	for key in keys {
		array.push(key.as_str());
	}
	document[table][vin] = toml_edit::value(array);
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
	fn hand_written_keys_and_comments_survive_being_written_by_the_tool() {
		// This file is a person's to write in, and the tool writes to it on
		// every `f`. Anything it does not itself understand — a key from a
		// newer version, a note somebody left themselves, a comment — has to
		// come back out the other side.
		let dir = temp("preserve");
		let path = dir.join("config.toml");
		std::fs::write(
			&path,
			"# why this car and not the other one\nproject = \"SK37X\"\nmy_own_note = \"do not lose me\"\n",
		)
		.unwrap();

		let mut document = load_at(&path);
		set_favourites(&mut document, "XW8AD4NE9JH008917", &["7E0:202A:0".to_string()]);
		write_at(&path, &document).unwrap();

		let back = load_at(&path);
		assert_eq!(project(&back).as_deref(), Some("SK37X"));
		assert_eq!(back.get("my_own_note").and_then(|v| v.as_str()), Some("do not lose me"));
		assert_eq!(favourites(&back, "XW8AD4NE9JH008917"), vec!["7E0:202A:0"]);
		// And the comment, which is the whole reason this file is not JSON. The
		// first version of this module parsed to a value tree with nowhere to
		// keep one, so `watch` deleted every note in the file the first time
		// somebody pressed `f`.
		let text = std::fs::read_to_string(&path).unwrap();
		assert!(text.contains("# why this car and not the other one"), "{text}");
		let _ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn one_cars_marks_never_touch_anothers() {
		let mut document = Document::new();
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
		let document: Document = "language = \"de\"\n".parse().unwrap();
		assert_eq!(language(&document), Language::En);
		let why = language_complaint(&document).expect("it complains");
		assert!(why.contains("\"de\""), "{why}");
		assert!(why.contains("names.csv"), "{why}");

		let fine: Document = "language = \"ru\"\n".parse().unwrap();
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
