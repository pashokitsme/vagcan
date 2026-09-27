//! `dash.toml` read strictly: every key a table holds is one that table takes, and a key the
//! build reads has the type it asks for.
//!
//! The parser used to look up the keys it knew and never walk a table, so a typo was dropped
//! without a word: `hzz = 50` read as the default rate, a misspelled `[[alarm]]` built a plan
//! with no alarms, and `decimals = "2"` acted as if absent (found writing
//! `docs/dash/dash-toml.md`, 2026-09-27). The owner writes this file by hand.
//!
//! The key lists below are the whole vocabulary. `docs/dash/dash-toml.md` lists the same keys
//! under the same sections, and a test holds the two together.

use toml_edit::{Item, Key, Table};

use super::Error;

/// The keys one table of `dash.toml` takes. The parser reads nothing else, and
/// [`super::parse_input`] refuses anything else by name.
pub(super) struct Keys {
	/// How the file writes the section: `[[channel]]`, `[stalk]`; empty at the top level.
	pub section: &'static str,
	/// What a refusal calls the table: `[[channel]]`, `a chart page`.
	pub name: &'static str,
	/// The `kind` that picks this table, in a section that has several.
	pub kind: Option<&'static str>,
	pub keys: &'static [&'static str],
}

pub(super) const TOP: Keys = Keys {
	section: "",
	name: "the top level",
	kind: None,
	keys: &["vin", "language", "survey"],
};

pub(super) const CHANNEL: Keys = Keys {
	section: "[[channel]]",
	name: "[[channel]]",
	kind: None,
	keys: &["ref", "label", "decimals", "hz", "setpoint"],
};

pub(super) const VALUES: Keys = Keys {
	section: "[[page]]",
	name: "a values page",
	kind: Some("values"),
	keys: &["kind", "title", "cells"],
};

pub(super) const CHART: Keys = Keys {
	section: "[[page]]",
	name: "a chart page",
	kind: Some("chart"),
	keys: &["kind", "cell", "min", "max"],
};

pub(super) const THRESHOLD: Keys = Keys {
	section: "[[alarm]]",
	name: "a threshold rule",
	kind: Some("threshold"),
	keys: &["kind", "channels", "page", "direction", "trip", "release"],
};

pub(super) const DRIFT: Keys = Keys {
	section: "[[alarm]]",
	name: "a drift rule",
	kind: Some("drift"),
	keys: &["kind", "channels", "page", "percent", "hold_ms", "release_percent", "min_setpoint"],
};

/// Known here and parsed elsewhere: `[[button]]` arrives in its own change, and until it does
/// its keys are at least not refused as typos.
pub(super) const BUTTON: Keys = Keys {
	section: "[[button]]",
	name: "[[button]]",
	kind: None,
	keys: &["pin", "action"],
};

pub(super) const STALK: Keys = Keys {
	section: "[stalk]",
	name: "[stalk]",
	kind: None,
	keys: &[
		"read",
		"rocker",
		"switch",
		"next",
		"previous",
		"measure",
		"switch_off",
		"cruise",
		"cruise_off",
	],
};

pub(super) const STOPWATCH: Keys = Keys {
	section: "[stopwatch]",
	name: "[stopwatch]",
	kind: None,
	keys: &["speed", "km_h_per_unit", "marks"],
};

/// Every section the file may hold, as the file writes it, in the reference's order.
const SECTIONS: [&str; 6] = ["[[channel]]", "[[page]]", "[[alarm]]", "[[button]]", "[stalk]", "[stopwatch]"];

/// Every table, for a refusal that finds one table's key in another.
const TABLES: [&Keys; 9] = [&TOP, &CHANNEL, &VALUES, &CHART, &THRESHOLD, &DRIFT, &BUTTON, &STALK, &STOPWATCH];

/// One table of the file, read strictly, and where it is — for a refusal to point at.
pub(super) struct Reader<'a> {
	/// The whole file, for line numbers.
	source: &'a str,
	table: &'a Table,
	/// How a refusal names the table: `[[alarm]] 2`, `[stalk]`; empty at the top level.
	place: String,
}

impl<'a> Reader<'a> {
	pub(super) fn new(source: &'a str, table: &'a Table, place: String) -> Reader<'a> {
		Reader { source, table, place }
	}

	/// Refuse the first key `keys` does not list — and at the top level, a section the file
	/// does not have — naming it, and what the table takes.
	pub(super) fn takes(&self, keys: &Keys) -> Result<(), Error> {
		let top = keys.section.is_empty();
		match self
			.table
			.iter()
			.find(|(key, _)| !keys.keys.contains(key) && !(top && SECTIONS.iter().any(|s| bare(s) == *key)))
		{
			None => Ok(()),
			Some((key, item)) => Err(self.refuse(key, format!("{} {}", unknown(key, item, keys), takes(keys)))),
		}
	}

	/// An optional string: `None` when absent, and refused as `expected` when it is there and
	/// not a string — never read as absent.
	pub(super) fn string(&self, key: &str, expected: &str) -> Result<Option<&'a str>, Error> {
		self.optional(key, expected, Item::as_str)
	}

	/// An optional integer, the same way.
	pub(super) fn integer(&self, key: &str, expected: &str) -> Result<Option<i64>, Error> {
		self.optional(key, expected, Item::as_integer)
	}

	fn optional<T>(&self, key: &str, expected: &str, read: impl FnOnce(&'a Item) -> Option<T>) -> Result<Option<T>, Error> {
		let Some(item) = self.table.get(key) else {
			return Ok(None);
		};
		match read(item) {
			Some(value) => Ok(Some(value)),
			None => Err(self.refuse(key, format!("{key} must be {expected}, not {}", a(item.type_name())))),
		}
	}

	/// A refusal of `key`: the line it is on, the table it is in, and `why`.
	pub(super) fn refuse(&self, key: &str, why: String) -> Error {
		let line = self.line(key).map(|n| format!("line {n}: ")).unwrap_or_default();
		let place = match self.place.as_str() {
			"" => String::new(),
			place => format!("{place}: "),
		};
		Error::Parse(format!("dash.toml: {line}{place}{why}"))
	}

	/// The line `key` is written on, counted from 1.
	fn line(&self, key: &str) -> Option<usize> {
		let span = self
			.table
			.key(key)
			.and_then(Key::span)
			.or_else(|| self.table.get(key).and_then(Item::span))?;
		Some(self.source.get(..span.start)?.matches('\n').count() + 1)
	}
}

/// What a key `keys` does not take is: a key of the table's other kind, a top-level key under
/// a section header, a key of another section, or a typo. TOML gives a key to the header above
/// it, so a key in the wrong table is as likely as a misspelled one.
fn unknown(key: &str, item: &Item, keys: &Keys) -> String {
	let top = keys.section.is_empty();
	// Only the top level holds sections — `foo.bar = 1` is a dotted key, not one — and anywhere
	// else a table is a key like any other. A section is never a key in the wrong place:
	// `[[channels]]` is a typo, not `channels` of an `[[alarm]]`.
	match item {
		Item::ArrayOfTables(_) if top => return typo("section", &format!("[[{key}]]"), key, keys),
		Item::Table(table) if top && !table.is_dotted() => return typo("section", &format!("[{key}]"), key, keys),
		_ => {}
	}
	if !top && let Some(other) = TABLES.iter().find(|t| t.section == keys.section && t.keys.contains(&key)) {
		return format!(
			"\"{key}\" is a key of {} (kind = \"{}\"), and this is {}.",
			other.name,
			other.kind.unwrap_or_default(),
			keys.name
		);
	}
	if !top && TOP.keys.contains(&key) {
		return format!("\"{key}\" is a top-level key: write it above the first section.");
	}
	let mut elsewhere: Vec<&str> = Vec::new();
	for table in TABLES
		.iter()
		.filter(|t| !t.section.is_empty() && t.section != keys.section && t.keys.contains(&key))
	{
		if !elsewhere.contains(&table.section) {
			elsewhere.push(table.section);
		}
	}
	if !elsewhere.is_empty() {
		return format!("\"{key}\" is a key of {}.", elsewhere.join(" and "));
	}
	typo("key", &format!("\"{key}\""), key, keys)
}

/// `unknown key "hzz" — did you mean "hz"?`: the table's own keys, and at the top level its
/// sections, nearest the one written. A misspelled section is only ever another section.
fn typo(noun: &str, spelled: &str, key: &str, keys: &Keys) -> String {
	let mut candidates: Vec<(&str, String)> = Vec::new();
	if noun != "section" {
		candidates.extend(keys.keys.iter().map(|k| (*k, format!("\"{k}\""))));
	}
	if keys.section.is_empty() {
		candidates.extend(SECTIONS.iter().map(|s| (bare(s), s.to_string())));
	}
	match nearest(key, &candidates) {
		Some(meant) => format!("unknown {noun} {spelled} — did you mean {meant}?"),
		None => format!("unknown {noun} {spelled}."),
	}
}

/// `A values page takes kind, title, cells`.
fn takes(keys: &Keys) -> String {
	let mut list: Vec<&str> = keys.keys.to_vec();
	if keys.section.is_empty() {
		list.extend(SECTIONS);
	}
	let mut name = keys.name.chars();
	let name = name.next().map_or(String::new(), |first| first.to_uppercase().chain(name).collect());
	format!("{name} takes {}", list.join(", "))
}

/// `channel` for `[[channel]]`.
fn bare(section: &str) -> &str {
	section.trim_matches(['[', ']'])
}

/// `an integer`, `a string`: TOML's name for a type, as a sentence says it.
pub(super) fn a(type_name: &str) -> String {
	if type_name.starts_with(['a', 'e', 'i', 'o', 'u']) {
		format!("an {type_name}")
	} else {
		format!("a {type_name}")
	}
}

/// The candidates (`bare` name, as a refusal spells it) nearest `typed`, case aside: two edits
/// at most, and fewer edits than it has letters — two edits turn any two-letter key into any
/// other, which is no guess. Several equally near are all named.
fn nearest(typed: &str, candidates: &[(&str, String)]) -> Option<String> {
	let typed_lower = typed.to_lowercase();
	let letters = typed.chars().count();
	let scored: Vec<(usize, &str)> = candidates
		.iter()
		.map(|(name, spelled)| (edit_distance(&typed_lower, &name.to_lowercase()), spelled.as_str()))
		.filter(|&(distance, _)| distance <= 2 && distance < letters)
		.collect();
	let least = scored.iter().map(|&(distance, _)| distance).min()?;
	let meant: Vec<&str> = scored.iter().filter(|&&(distance, _)| distance == least).map(|&(_, s)| s).collect();
	Some(meant.join(" or "))
}

/// Levenshtein distance over characters: insertions, deletions, substitutions. The same
/// function `vag-cli-diag`'s `setup::source` guesses a mistyped path with.
fn edit_distance(a: &str, b: &str) -> usize {
	let b: Vec<char> = b.chars().collect();
	let mut row: Vec<usize> = (0..=b.len()).collect();
	for (i, ca) in a.chars().enumerate() {
		let mut diagonal = row[0];
		row[0] = i + 1;
		for (j, cb) in b.iter().enumerate() {
			let above = row[j + 1];
			row[j + 1] = (above + 1).min(row[j] + 1).min(diagonal + usize::from(ca != *cb));
			diagonal = above;
		}
	}
	row[b.len()]
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::dash::{AlarmRuleInput, PageInput, parse_input};

	/// Every table the file has, every key of each, each once: the file the refusals are
	/// written against. Parsed only, so no name in it has to resolve.
	const BASE: &str = r#"vin = "TESTVIN0000000001"
language = "en"
survey = "/nowhere/survey.jsonl"

[[channel]]
ref = "01:IDE00001"
label = "One"
decimals = 1
hz = 10
setpoint = "01:IDE00002"

[[page]]
kind = "values"
title = "A"
cells = ["01:IDE00001"]

[[page]]
kind = "chart"
cell = "01:IDE00001"
min = 0
max = 10

[[alarm]]
channels = ["01:IDE00001"]
page = "A"
direction = "above"
trip = 5
release = 4

[[alarm]]
kind = "drift"
channels = ["01:IDE00001"]
page = "A"
percent = 10
release_percent = 6
hold_ms = 1000
min_setpoint = 0.5

[stalk]
read = "75A:4C21"
rocker = "Rocker"
switch = "Switch"
next = "plus"
previous = "minus"
measure = "limit"
switch_off = "off"
cruise = "01:2001"
cruise_off = "off"

[stopwatch]
speed = "01:IDE00001"
km_h_per_unit = 0.0
marks = [60, 100]
"#;

	/// `BASE` with `line` added right under the first line that is `under`, and the number of
	/// the line it landed on.
	fn with(under: &str, line: &str) -> (String, usize) {
		let mut out = Vec::new();
		let mut at = 0;
		for (i, l) in BASE.lines().enumerate() {
			out.push(l);
			if at == 0 && l == under {
				out.push(line);
				at = i + 2;
			}
		}
		assert!(at > 0, "{under:?} is not a line of BASE");
		(out.join("\n") + "\n", at)
	}

	/// `BASE` with every line that is `from` replaced by `to`, and the number of the first.
	fn replacing(from: &str, to: &str) -> (String, usize) {
		let first = BASE
			.lines()
			.position(|l| l == from)
			.unwrap_or_else(|| panic!("{from:?} is not a line of BASE"));
		let text: Vec<&str> = BASE.lines().map(|l| if l == from { to } else { l }).collect();
		(text.join("\n") + "\n", first + 1)
	}

	fn refused(text: &str) -> String {
		match parse_input(text) {
			Ok(input) => panic!("built, and should not have: {input:?}\n{text}"),
			Err(e) => e.to_string(),
		}
	}

	#[test]
	fn the_base_file_parses() {
		let input = parse_input(BASE).unwrap();
		assert_eq!((input.channels.len(), input.pages.len(), input.alarms.len()), (1, 2, 2));
		assert!(input.stalk.is_some() && input.stopwatch.is_some());
	}

	#[test]
	fn a_misspelled_key_is_refused_by_line_table_and_name_with_the_nearest_key_and_the_keys_the_table_takes() {
		let (text, line) = with("ref = \"01:IDE00001\"", "hzz = 50");
		assert_eq!(
			refused(&text),
			format!(
				"dash.toml: line {line}: [[channel]] 1: unknown key \"hzz\" — did you mean \"hz\"? [[channel]] takes ref, label, decimals, hz, setpoint"
			)
		);
	}

	#[test]
	fn an_unknown_key_is_refused_in_every_table() {
		for (under, place, takes) in [
			(
				"vin = \"TESTVIN0000000001\"",
				"",
				"The top level takes vin, language, survey, [[channel]], [[page]], [[alarm]], [[button]], [stalk], [stopwatch]",
			),
			(
				"ref = \"01:IDE00001\"",
				"[[channel]] 1: ",
				"[[channel]] takes ref, label, decimals, hz, setpoint",
			),
			("title = \"A\"", "[[page]] 1: ", "A values page takes kind, title, cells"),
			("cell = \"01:IDE00001\"", "[[page]] 2: ", "A chart page takes kind, cell, min, max"),
			(
				"direction = \"above\"",
				"[[alarm]] 1: ",
				"A threshold rule takes kind, channels, page, direction, trip, release",
			),
			(
				"kind = \"drift\"",
				"[[alarm]] 2: ",
				"A drift rule takes kind, channels, page, percent, hold_ms, release_percent, min_setpoint",
			),
			(
				"read = \"75A:4C21\"",
				"[stalk]: ",
				"[stalk] takes read, rocker, switch, next, previous, measure, switch_off, cruise, cruise_off",
			),
			(
				"speed = \"01:IDE00001\"",
				"[stopwatch]: ",
				"[stopwatch] takes speed, km_h_per_unit, marks",
			),
		] {
			let (text, line) = with(under, "bogus = 1");
			let why = refused(&text);
			assert_eq!(
				why,
				format!("dash.toml: line {line}: {place}unknown key \"bogus\". {takes}"),
				"under {under:?}"
			);
		}
	}

	#[test]
	fn a_misspelled_section_is_refused_rather_than_read_as_absent() {
		// A misspelled `[[alarm]]` used to build a plan with no alarms.
		let (text, line) = replacing("[[alarm]]", "[[alarms]]");
		let why = refused(&text);
		assert!(
			why.starts_with(&format!(
				"dash.toml: line {line}: unknown section [[alarms]] — did you mean [[alarm]]? The top level takes vin,"
			)),
			"{why}"
		);
		let (text, line) = replacing("[stopwatch]", "[stopwach]");
		assert!(
			refused(&text).starts_with(&format!("dash.toml: line {line}: unknown section [stopwach] — did you mean [stopwatch]?")),
			"{text}"
		);
		for (text, says) in [
			(
				replacing("[[channel]]", "[[channels]]").0,
				"unknown section [[channels]] — did you mean [[channel]]?",
			),
			(with("marks = [60, 100]", "[extras]").0, "unknown section [extras]. The top level takes"),
			// A section is only ever another section misspelled: `[[vins]]` is no `vin`.
			(format!("{BASE}[[vins]]\n"), "unknown section [[vins]]. The top level takes"),
			// A dotted key at the top level is a key, not a section.
			(
				with("vin = \"TESTVIN0000000001\"", "foo.bar = 1").0,
				"unknown key \"foo\". The top level takes",
			),
			(
				with("survey = \"/nowhere/survey.jsonl\"", "langauge = \"ru\"").0,
				"unknown key \"langauge\" — did you mean \"language\"?",
			),
		] {
			let why = refused(&text);
			assert!(why.contains(says), "{says}: {why}");
		}
	}

	#[test]
	fn a_key_of_the_other_kind_of_page_or_rule_says_which_kind_takes_it() {
		for (under, line, says) in [
			(
				"title = \"A\"",
				"min = 0",
				"[[page]] 1: \"min\" is a key of a chart page (kind = \"chart\"), and this is a values page. A values page takes kind, title, cells",
			),
			(
				"cell = \"01:IDE00001\"",
				"title = \"B\"",
				"[[page]] 2: \"title\" is a key of a values page (kind = \"values\"), and this is a chart page. A chart page takes kind, cell, min, max",
			),
			(
				"kind = \"drift\"",
				"trip = 1",
				"[[alarm]] 2: \"trip\" is a key of a threshold rule (kind = \"threshold\"), and this is a drift rule. A drift rule takes",
			),
			(
				"direction = \"above\"",
				"percent = 10",
				"[[alarm]] 1: \"percent\" is a key of a drift rule (kind = \"drift\"), and this is a threshold rule. A threshold rule takes",
			),
			(
				"direction = \"above\"",
				"hold_ms = 1000",
				"[[alarm]] 1: \"hold_ms\" is a key of a drift rule (kind = \"drift\")",
			),
		] {
			let (text, at) = with(under, line);
			let why = refused(&text);
			assert!(why.starts_with(&format!("dash.toml: line {at}: {says}")), "{line}: {why}");
		}
	}

	#[test]
	fn a_key_under_the_wrong_header_says_where_it_belongs() {
		// TOML gives a key to the header above it: a top-level key written at the bottom of the
		// file is the last section's.
		let text = format!("{BASE}survey = \"/elsewhere.jsonl\"\n");
		let why = refused(&text);
		assert!(
			why.contains("[stopwatch]: \"survey\" is a top-level key: write it above the first section. [stopwatch] takes"),
			"{why}"
		);
		let (text, _) = with("title = \"A\"", "hz = 10");
		let why = refused(&text);
		assert!(why.contains("[[page]] 1: \"hz\" is a key of [[channel]]. A values page takes"), "{why}");
		let (text, _) = with("ref = \"01:IDE00001\"", "kind = \"values\"");
		let why = refused(&text);
		assert!(why.contains("[[channel]] 1: \"kind\" is a key of [[page]] and [[alarm]]."), "{why}");
	}

	#[test]
	fn the_nearest_key_is_two_edits_away_at_most_and_a_tie_names_both() {
		let keys = |names: &[&'static str]| -> Vec<(&'static str, String)> { names.iter().map(|k| (*k, format!("\"{k}\""))).collect() };
		assert_eq!(nearest("mix", &keys(&["min", "max"])), Some("\"min\" or \"max\"".to_string()));
		assert_eq!(nearest("setpiont", &keys(&["setpoint", "ref"])), Some("\"setpoint\"".to_string()));
		assert_eq!(nearest("hold", &keys(&["hold_ms"])), None, "three edits");
		assert_eq!(edit_distance("kitten", "sitting"), 3);
	}

	#[test]
	fn a_near_miss_in_case_is_a_near_miss_and_a_short_key_is_not_guessed_at() {
		let (text, _) = with("ref = \"01:IDE00001\"", "Hz = 10");
		assert!(refused(&text).contains("unknown key \"Hz\" — did you mean \"hz\"?"));
		// Two edits turn any two-letter key into any other: no guess.
		let (text, _) = with("ref = \"01:IDE00001\"", "x = 10");
		let why = refused(&text);
		assert!(why.contains("unknown key \"x\". [[channel]] takes"), "{why}");
	}

	#[test]
	fn an_optional_key_of_the_wrong_type_is_refused_by_the_type_it_takes() {
		for (from, to, says) in [
			("label = \"One\"", "label = 5", "[[channel]] 1: label must be a string, not an integer"),
			(
				"decimals = 1",
				"decimals = 1.5",
				"[[channel]] 1: decimals must be a whole number from 0 to 3, not a float",
			),
			(
				"decimals = 1",
				"decimals = \"2\"",
				"[[channel]] 1: decimals must be a whole number from 0 to 3, not a string",
			),
			(
				"language = \"en\"",
				"language = 1",
				"language must be a string, \"en\" or \"ru\", not an integer",
			),
			(
				"survey = \"/nowhere/survey.jsonl\"",
				"survey = 5",
				"survey must be a string, a file path, not an integer",
			),
			("title = \"A\"", "title = 5", "[[page]] 1: title must be a string, not an integer"),
			("title = \"A\"", "title = [\"A\"]", "[[page]] 1: title must be a string, not an array"),
		] {
			let (text, line) = replacing(from, to);
			assert_eq!(refused(&text), format!("dash.toml: line {line}: {says}"), "{to}");
		}
		// An alarm whose `kind` is not a string used to be a threshold rule.
		let (text, line) = with("[[alarm]]", "kind = 5");
		assert_eq!(
			refused(&text),
			format!("dash.toml: line {line}: [[alarm]] 1: kind must be a string, \"threshold\" or \"drift\", not an integer")
		);
	}

	#[test]
	fn a_chart_scale_must_fit_the_boards_f32() {
		for (from, to) in [
			("min = 0", "min = -inf"),
			("min = 0", "min = nan"),
			("min = 0", "min = -1e39"),
			("max = 10", "max = inf"),
			("max = 10", "max = 1e39"),
		] {
			let (text, line) = replacing(from, to);
			let key = &to[..3];
			assert_eq!(
				refused(&text),
				format!("dash.toml: line {line}: [[page]] 2: {key} must be a finite number the board's 32-bit float holds, within ±3.4028235e38"),
				"{to}"
			);
		}
		let (text, line) = replacing("min = 0", "min = \"0\"");
		assert_eq!(
			refused(&text),
			format!("dash.toml: line {line}: [[page]] 2: min must be a number, not a string")
		);
		let (text, _) = replacing("max = 10", "max = 3.4e38");
		let input = parse_input(&text).unwrap();
		assert!(matches!(input.pages[1], PageInput::Chart { max, .. } if max == 3.4e38));
	}

	#[test]
	fn a_rate_or_a_share_the_boards_f32_holds_as_zero_is_refused() {
		// Above 0 as written, 0 on the board: a channel read at the fallback rate, a drift rule
		// that never clears.
		let (text, _) = replacing("hz = 10", "hz = 1e-50");
		assert_eq!(
			refused(&text),
			"dash.toml: 01:IDE00001: hz 1e-50 is too small for the board, which would hold it as 0"
		);
		let (text, _) = replacing("release_percent = 6", "release_percent = 1e-50");
		assert_eq!(
			refused(&text),
			"dash.toml: alarm #2: release_percent 1e-50 is too small for the board, which would hold it as 0"
		);
		let (text, _) = replacing("release_percent = 6", "release_percent = 1e-30");
		assert!(matches!(
			parse_input(&text).unwrap().alarms[1].rule,
			AlarmRuleInput::Drift { release_percent, .. } if release_percent == 1e-30
		));
	}

	/// `[[button]]` is parsed by its own change. Until then its keys are known here, so a file
	/// with buttons builds, and a typo in one is refused like any other.
	#[test]
	fn a_button_takes_pin_and_action() {
		let text = format!("{BASE}[[button]]\npin = 9\naction = \"next\"\n");
		parse_input(&text).unwrap();
		let text = format!("{BASE}[[button]]\npinn = 9\n");
		assert!(
			refused(&text).contains("[[button]] 1: unknown key \"pinn\" — did you mean \"pin\"? [[button]] takes pin, action"),
			"{text}"
		);
	}

	const REFERENCE: &str = include_str!("../../../../../docs/dash/dash-toml.md");

	/// The reference's full example is a file that builds: read out of the document, so the two
	/// cannot drift apart.
	#[test]
	fn the_full_example_in_the_reference_parses() {
		let after = REFERENCE.split_once("## A full example").expect("the reference has a full example").1;
		let example = after
			.split_once("```toml\n")
			.and_then(|(_, rest)| rest.split_once("\n```"))
			.expect("the example is a fenced toml block")
			.0;
		let input = parse_input(example).unwrap_or_else(|e| panic!("{e}\n{example}"));
		assert_eq!((input.channels.len(), input.pages.len(), input.alarms.len()), (3, 2, 2));
		assert!(input.stalk.is_some() && input.stopwatch.is_some());
		assert_eq!(input.language, Some(crate::config::Language::Ru));
	}

	/// The keys the reference's tables list under each section are the keys the parser takes:
	/// a key the owner reads about is one the build accepts, and the reverse.
	#[test]
	fn the_reference_lists_the_keys_the_parser_takes() {
		// Under each `### ` heading, each table's first column of keys, table by table: a
		// section with two kinds has two tables, in the order the reference gives them.
		let mut listed: Vec<(String, Vec<Vec<String>>)> = Vec::new();
		let mut in_table = false;
		for line in REFERENCE.lines() {
			if let Some(heading) = line.strip_prefix("### ") {
				listed.push((heading.trim_matches('`').to_string(), Vec::new()));
			} else if line.starts_with("## ") {
				listed.push((String::new(), Vec::new()));
			}
			let row = line.starts_with('|');
			if let Some((_, tables)) = listed.last_mut() {
				if row && !in_table {
					tables.push(Vec::new());
				}
				if let Some(rest) = line.strip_prefix("| `")
					&& let Some((key, _)) = rest.split_once('`')
					&& let Some(keys) = tables.last_mut()
				{
					keys.push(key.to_string());
				}
			}
			in_table = row;
		}
		for (heading, kinds) in [
			("Top level", vec![&TOP]),
			("[[channel]]", vec![&CHANNEL]),
			("[[page]]", vec![&VALUES, &CHART]),
			("[[alarm]]", vec![&THRESHOLD, &DRIFT]),
			("[stalk]", vec![&STALK]),
			("[stopwatch]", vec![&STOPWATCH]),
			("[[button]]", vec![&BUTTON]),
		] {
			let (_, tables) = listed
				.iter()
				.find(|(h, _)| h == heading)
				.unwrap_or_else(|| panic!("the reference has no ### {heading}"));
			let documented: Vec<Vec<&str>> = tables
				.iter()
				.filter(|keys| !keys.is_empty())
				.map(|keys| keys.iter().map(String::as_str).collect())
				.collect();
			// `[[button]]` has no table in the reference yet.
			if documented.is_empty() && heading == "[[button]]" {
				continue;
			}
			let implemented: Vec<Vec<&str>> = kinds.iter().map(|k| k.keys.to_vec()).collect();
			assert_eq!(documented, implemented, "{heading}");
		}
	}
}
