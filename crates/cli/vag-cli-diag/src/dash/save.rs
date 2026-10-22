//! What `dev dash edit`'s Save writes: the editor's pages, labels and decimals made into the
//! car's `dash.toml` by editing the file that is there, not by writing a new one.
//!
//! The request is the editor's state ([`Body`]), never TOML: the page sends what it holds and
//! [`Body::check`] takes nothing it did not check. [`edit`] then changes exactly this:
//!
//! - every `[[page]]`, replaced as a whole, in the editor's order, where the file's first
//!   `[[page]]` was. A page the editor opened from the file (`from`) keeps that table's own
//!   comments and every value it did not change; a page taken off goes with its comments.
//! - `label` of an existing `[[channel]]`, where the editor's differs — and `decimals`, which
//!   the page carries through as the file has them;
//! - a new `[[channel]]` for each channel a page shows that the file has none for, after the
//!   last one, with the channel's name and unit of measure in a comment.
//!
//! Everything else stays as written, as far as `toml_edit` keeps it: comments, the order of
//! the channels and of their keys, alarms, `[stalk]`, `[stopwatch]`, `[strip]`, `language`. A
//! channel no page shows any more stays: it may carry an alarm.

use std::collections::BTreeMap;

use serde::Deserialize;
use toml_edit::{Array, ArrayOfTables, DocumentMut, Item, Table, Value};
use vag_cli_core::dash::{Input, PageInput, Reference};

/// The longest label or title taken when it is new — the page's own fields hold 16 and 34. A
/// label or title the file already has is taken as it is: the build sets no length, and a
/// file it accepts must save.
pub const MAX_NEW_TEXT: usize = 64;

/// The order a person writes a channel's keys in, as far as a save adds any.
const CHANNEL_KEYS: &[&str] = &["ref", "label", "decimals"];

/// The order a person writes a page's keys in.
const PAGE_KEYS: &[&str] = &["kind", "title", "cells", "cell", "min", "max", "volts"];

/// A save request as the page sends it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
	/// What `layout.json` said the file was when the page read it ([`super::edit::file_fingerprint`]).
	pub fingerprint: String,
	pub pages: Vec<PageBody>,
	/// The label of each channel that has one, by reference; a file channel missing here has
	/// none.
	#[serde(default)]
	pub labels: BTreeMap<String, String>,
	/// Likewise its decimals.
	#[serde(default)]
	pub decimals: BTreeMap<String, i64>,
}

/// One page as the page sends it: `layout.json`'s shape.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageBody {
	pub kind: String,
	#[serde(default)]
	pub title: String,
	#[serde(default)]
	pub cells: Option<Vec<String>>,
	#[serde(default)]
	pub cell: Option<String>,
	#[serde(default)]
	pub min: Option<f64>,
	#[serde(default)]
	pub max: Option<f64>,
	#[serde(default = "on")]
	pub volts: bool,
	/// The index of the file's `[[page]]` this one was opened from, if any.
	#[serde(default)]
	pub from: Option<usize>,
}

fn on() -> bool {
	true
}

/// A save request, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Wanted {
	pub pages: Vec<Page>,
	pub labels: BTreeMap<Reference, String>,
	pub decimals: BTreeMap<Reference, u8>,
}

/// One page, checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
	/// Trimmed; empty is none.
	pub title: String,
	pub shows: Shows,
	pub volts: bool,
	pub from: Option<usize>,
}

/// What a page shows.
#[derive(Debug, Clone, PartialEq)]
pub enum Shows {
	Values(Vec<Reference>),
	Chart { cell: Reference, min: f64, max: f64 },
}

impl Page {
	fn refs(&self) -> &[Reference] {
		match &self.shows {
			Shows::Values(cells) => cells,
			Shows::Chart { cell, .. } => std::slice::from_ref(cell),
		}
	}
}

/// Text a person typed, trimmed: what the file already has (`was`) as it is, and anything
/// new only when it is short and has no control character in it.
fn text(what: &str, raw: &str, was: Option<&str>) -> Result<String, String> {
	let trimmed = raw.trim();
	if was == Some(trimmed) {
		return Ok(trimmed.to_string());
	}
	if trimmed.chars().any(char::is_control) {
		return Err(format!("{what} has a control character in it"));
	}
	if trimmed.chars().count() > MAX_NEW_TEXT {
		return Err(format!("{what} is longer than {MAX_NEW_TEXT} characters"));
	}
	Ok(trimmed.to_string())
}

fn reference(what: &str, raw: &str) -> Result<Reference, String> {
	Reference::parse(raw).map_err(|e| format!("{what}: {e}"))
}

impl Body {
	/// The request checked field by field, against `file` — the car's `dash.toml` as parsed,
	/// `None` when there is none. A request over the board's limits (the renderer's) is refused,
	/// never cut; whether the car has the channels is the build's question.
	pub fn check(self, file: Option<&Input>) -> Result<Wanted, String> {
		use vag_dash_render::pages::MAX_PAGES;
		use vag_dash_render::plan::MAX_VALUE_CELLS;
		if self.pages.is_empty() {
			return Err("no page to save".to_string());
		}
		if self.pages.len() > MAX_PAGES {
			return Err(format!("{} pages, and the board holds at most {MAX_PAGES}", self.pages.len()));
		}
		let file_pages: &[PageInput] = file.map_or(&[], |f| f.pages.as_slice());
		let file_pages = file_pages.len();
		let mut pages = Vec::new();
		let mut taken = vec![false; file_pages];
		for (i, page) in self.pages.into_iter().enumerate() {
			let n = i + 1;
			let was = page.from.and_then(|from| file?.pages.get(from)).map(|p| match p {
				PageInput::Values { title, .. } | PageInput::Chart { title, .. } => title.as_str(),
			});
			let title = text(&format!("page {n}'s title"), &page.title, was)?;
			let shows = match page.kind.as_str() {
				"values" => {
					if page.cell.is_some() || page.min.is_some() || page.max.is_some() {
						return Err(format!("page {n}: a values page has cells, not a cell and a scale"));
					}
					let cells = page.cells.unwrap_or_default();
					if cells.len() > MAX_VALUE_CELLS {
						return Err(format!(
							"page {n}: {} cells, and a values page holds at most {MAX_VALUE_CELLS}",
							cells.len()
						));
					}
					Shows::Values(cells.iter().map(|c| reference(&format!("page {n}"), c)).collect::<Result<_, _>>()?)
				}
				"chart" => {
					if page.cells.is_some() {
						return Err(format!("page {n}: a chart page has one cell, not cells"));
					}
					let cell = page
						.cell
						.filter(|c| !c.trim().is_empty())
						.ok_or(format!("page {n}: the chart shows no channel"))?;
					let (Some(min), Some(max)) = (page.min, page.max) else {
						return Err(format!("page {n}: the chart needs min and max"));
					};
					if !min.is_finite() || !max.is_finite() {
						return Err(format!("page {n}: min and max must be finite numbers"));
					}
					Shows::Chart {
						cell: reference(&format!("page {n}"), &cell)?,
						min,
						max,
					}
				}
				other => return Err(format!("page {n}: kind {other:?} is not \"values\" or \"chart\"")),
			};
			if let Some(from) = page.from {
				match taken.get_mut(from) {
					None => return Err(format!("page {n} comes from page {} of a file with {file_pages}", from + 1)),
					Some(true) => return Err(format!("two pages come from page {} of the file", from + 1)),
					Some(seen) => *seen = true,
				}
			}
			pages.push(Page {
				title,
				shows,
				volts: page.volts,
				from: page.from,
			});
		}
		let mut labels = BTreeMap::new();
		for (key, label) in self.labels {
			let at = reference("a label's channel", &key)?;
			let was = file
				.and_then(|f| f.channels.iter().find(|c| c.reference == at))
				.and_then(|c| c.label.as_deref());
			let label = text(&format!("the label of {at}"), &label, was)?;
			if label.is_empty() {
				continue;
			}
			if labels.insert(at.clone(), label.clone()).is_some_and(|was| was != label) {
				return Err(format!("two labels for {at}"));
			}
		}
		let mut decimals = BTreeMap::new();
		for (key, n) in self.decimals {
			let at = reference("a channel's decimals", &key)?;
			let n = u8::try_from(n)
				.ok()
				.filter(|n| *n <= 3)
				.ok_or(format!("decimals {n} for {at} is not 0 to 3"))?;
			if decimals.insert(at.clone(), n).is_some_and(|was| was != n) {
				return Err(format!("two decimals for {at}"));
			}
		}
		Ok(Wanted { pages, labels, decimals })
	}
}

/// `current` — the file as it is, or `None` for a car with none yet — with `wanted` in it.
/// `describe` says a channel's name and unit of measure, for the comment on a new one.
pub fn edit(current: Option<&str>, vin: &str, wanted: &Wanted, describe: impl Fn(&Reference) -> Option<String>) -> Result<String, String> {
	let mut doc: DocumentMut = match current {
		Some(text) => text.parse().map_err(|e| format!("dash.toml: {e}"))?,
		None => {
			let mut doc = DocumentMut::new();
			doc.insert("vin", toml_edit::value(vin));
			doc
		}
	};

	// The channels there are: a label or decimals the editor changed, and nothing else.
	let mut known: Vec<Reference> = Vec::new();
	if let Some(item) = doc.get_mut("channel") {
		let channels = item
			.as_array_of_tables_mut()
			.ok_or("dash.toml: channel is not a list of [[channel]] tables")?;
		for table in channels.iter_mut() {
			let Some(at) = table.get("ref").and_then(Item::as_str).and_then(|r| Reference::parse(r).ok()) else {
				continue;
			};
			let label = wanted.labels.get(&at).map(String::as_str);
			if table.get("label").and_then(Item::as_str).map(str::trim).filter(|l| !l.is_empty()) != label {
				match label {
					Some(label) => put(table, "label", label.into(), CHANNEL_KEYS),
					None => drop(table.remove("label")),
				}
			}
			let decimals = wanted.decimals.get(&at).map(|&d| i64::from(d));
			if table.get("decimals").and_then(Item::as_integer) != decimals {
				match decimals {
					Some(d) => put(table, "decimals", d.into(), CHANNEL_KEYS),
					None => drop(table.remove("decimals")),
				}
			}
			known.push(at);
		}
	}

	// A channel a page shows and the file has no `[[channel]]` for: one after the last.
	let mut fresh: Vec<&Reference> = Vec::new();
	for at in wanted.pages.iter().flat_map(Page::refs) {
		if !known.contains(at) && !fresh.contains(&at) {
			fresh.push(at);
		}
	}
	if !fresh.is_empty() {
		let channels = doc
			.entry("channel")
			.or_insert(Item::ArrayOfTables(ArrayOfTables::new()))
			.as_array_of_tables_mut()
			.ok_or("dash.toml: channel is not a list of [[channel]] tables")?;
		for at in fresh {
			let mut table = Table::new();
			let mut reference = Value::from(at.to_string());
			if let Some(said) = describe(at) {
				let said: String = said.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
				reference.decor_mut().set_suffix(format!("   # {}", said.trim()));
			}
			table.insert("ref", Item::Value(reference));
			if let Some(label) = wanted.labels.get(at) {
				table.insert("label", toml_edit::value(label.as_str()));
			}
			if let Some(&d) = wanted.decimals.get(at) {
				table.insert("decimals", toml_edit::value(i64::from(d)));
			}
			channels.push(table);
		}
	}

	// The pages, all of them, where the first one was.
	let old: Vec<Table> = doc
		.get("page")
		.and_then(Item::as_array_of_tables)
		.map(|pages| pages.iter().cloned().collect())
		.unwrap_or_default();
	let first = old.first().and_then(Table::position);
	let mut pages = ArrayOfTables::new();
	for page in &wanted.pages {
		let mut table = match page.from {
			Some(i) => old.get(i).cloned().ok_or(format!("the file has no page {}", i + 1))?,
			None => Table::new(),
		};
		write_page(&mut table, page);
		table.set_position(first);
		pages.push(table);
	}
	match doc.get_mut("page") {
		Some(item) => *item = Item::ArrayOfTables(pages),
		None => drop(doc.insert("page", Item::ArrayOfTables(pages))),
	}
	let mut text = doc.to_string();
	if !text.ends_with('\n') {
		text.push('\n');
	}
	Ok(text)
}

/// `key = new` in `table`. A key it has keeps its place and the comment its old value had. A
/// key it has not goes right after the last key `order` ranks before it — after `ref` for a
/// label — and every key that was there keeps its order: nothing of the person's moves.
fn put(table: &mut Table, key: &str, mut new: Value, order: &[&str]) {
	if let Some(item) = table.get_mut(key) {
		if let Some(old) = item.as_value() {
			*new.decor_mut() = old.decor().clone();
		}
		*item = Item::Value(new);
		return;
	}
	let rank = |k: &str| order.iter().position(|o| *o == k).unwrap_or(order.len());
	let keys: Vec<String> = table.iter().map(|(k, _)| k.to_string()).collect();
	let at = keys.iter().rposition(|k| rank(k) < rank(key)).map_or(0, |i| i + 1);
	table.insert(key, Item::Value(new));
	// The keys after that place go behind the new one, each with its own formatting.
	for k in &keys[at..] {
		if let Some((k, item)) = table.remove_entry(k) {
			table.insert_formatted(&k, item);
		}
	}
}

/// A number as a person writes it: `70`, not `70.0`.
fn number(v: f64) -> Value {
	if v.fract() == 0.0 && v.abs() < 1e15 {
		Value::from(v as i64)
	} else {
		Value::from(v)
	}
}

fn number_at(table: &Table, key: &str) -> Option<f64> {
	let item = table.get(key)?;
	item.as_float().or_else(|| item.as_integer().map(|n| n as f64))
}

fn reference_at(item: &Item) -> Option<Reference> {
	item.as_str().and_then(|r| Reference::parse(r).ok())
}

/// `page` written into `table` — a table from the file, or a new one — touching only what
/// differs from what it holds.
fn write_page(table: &mut Table, page: &Page) {
	let kind = match page.shows {
		Shows::Values(_) => "values",
		Shows::Chart { .. } => "chart",
	};
	if table.get("kind").and_then(Item::as_str) != Some(kind) {
		put(table, "kind", kind.into(), PAGE_KEYS);
	}
	if table.get("title").and_then(Item::as_str).unwrap_or("").trim() != page.title {
		match page.title.is_empty() {
			true => drop(table.remove("title")),
			false => put(table, "title", page.title.as_str().into(), PAGE_KEYS),
		}
	}
	match &page.shows {
		Shows::Values(cells) => {
			for key in ["cell", "min", "max"] {
				table.remove(key);
			}
			let was: Option<Vec<Reference>> = table
				.get("cells")
				.and_then(Item::as_array)
				.and_then(|a| a.iter().map(|v| v.as_str().and_then(|r| Reference::parse(r).ok())).collect());
			if was.as_ref() != Some(cells) {
				put(
					table,
					"cells",
					Value::Array(cells.iter().map(ToString::to_string).collect::<Array>()),
					PAGE_KEYS,
				);
			}
		}
		Shows::Chart { cell, min, max } => {
			table.remove("cells");
			if table.get("cell").and_then(reference_at).as_ref() != Some(cell) {
				put(table, "cell", cell.to_string().into(), PAGE_KEYS);
			}
			for (key, v) in [("min", *min), ("max", *max)] {
				if number_at(table, key) != Some(v) {
					put(table, key, number(v), PAGE_KEYS);
				}
			}
		}
	}
	if table.get("volts").and_then(Item::as_bool).unwrap_or(true) != page.volts {
		match page.volts {
			true => drop(table.remove("volts")),
			false => put(table, "volts", false.into(), PAGE_KEYS),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	/// A made-up car's file, with what a person writes into one: comments above and between
	/// tables and after values, alarms, a setpoint, the sections the editor does not touch.
	const FILE: &str = r#"# The dash of a made-up car.
vin = "TESTVIN0000000001"
language = "en"

# ---- channels ----
# Coolant: the alarm the owner set.
[[channel]]
ref = "01:IDE00001"   # Coolant, °C
label = "COOLANT"
alarm_max = 110
alarm_max_off = 105
hz = 1

[[channel]]
ref = "01:IDE00002"
decimals = 1
setpoint = "01:IDE00003"
drift_percent = 10
drift_off_percent = 5
drift_hold_ms = 2000
drift_min_setpoint = 1.5

# not on a page, kept for its alarm
[[channel]]
ref = "01:IDE00004"
alarm_min = 0.5
zero_at_rest = true

# ---- pages ----
[[page]]
kind = "values"
title = "MAIN"
cells = ["01:IDE00001", "01:IDE00002"]   # the two that matter

# the chart page
[[page]]
kind = "chart"
cell = "01:IDE00001"
min = 70
max = 120
volts = false

# ---- the rest ----
[strip]
volts = "01:IDE00002"

[stopwatch]
speed = "01:IDE00002"
km_h_per_unit = 1.0
marks = [100]

# the lever
[stalk]
read = "75A:4C21"
rocker = "Rocker"
switch = "Switch"
next = "up"
previous = "down"
measure = "press"
switch_off = "off"
cruise = "01:IDE00005"
cruise_off = "off"
# the end
"#;

	/// The page as `layout.json` opened it on [`FILE`].
	fn as_opened() -> serde_json::Value {
		json!({
			"fingerprint": "f",
			"pages": [
				{"kind": "values", "title": "MAIN", "cells": ["01:IDE00001", "01:IDE00002"], "volts": true, "from": 0},
				{"kind": "chart", "title": "", "cell": "01:IDE00001", "min": 70.0, "max": 120.0, "volts": false, "from": 1},
			],
			"labels": {"01:IDE00001": "COOLANT"},
			"decimals": {"01:IDE00002": 1},
		})
	}

	fn wanted(body: serde_json::Value, file: Option<&str>) -> Result<Wanted, String> {
		let file = file.map(|f| vag_cli_core::dash::parse_input(f).unwrap());
		serde_json::from_value::<Body>(body).map_err(|e| e.to_string())?.check(file.as_ref())
	}

	fn saved(body: serde_json::Value) -> String {
		let wanted = wanted(body, Some(FILE)).unwrap();
		edit(Some(FILE), "TESTVIN0000000001", &wanted, |r| Some(format!("Name of {r}, °C"))).unwrap()
	}

	/// The file with its `[[page]]` tables taken out: what a save must leave as it was.
	fn without_pages(text: &str) -> String {
		let mut doc: DocumentMut = text.parse().unwrap();
		doc.remove("page");
		doc.to_string()
	}

	#[test]
	fn the_file_as_opened_saves_as_it_was_byte_for_byte() {
		assert_eq!(saved(as_opened()), FILE);
		// And it parses as a build input, which is what makes it a test of a real file.
		vag_cli_core::dash::parse_input(FILE).unwrap();
	}

	#[test]
	fn pages_are_replaced_in_the_editor_s_order_and_everything_else_is_kept() {
		let mut body = as_opened();
		// The chart first, the values page changed, and a new page at the end.
		body["pages"] = json!([
			{"kind": "chart", "title": "", "cell": "01:IDE00001", "min": 70.0, "max": 120.0, "volts": false, "from": 1},
			{"kind": "values", "title": "MAIN", "cells": ["01:IDE00002", "01:IDE00001"], "volts": true, "from": 0},
			{"kind": "values", "title": "OIL", "cells": ["01:IDE00004"], "volts": true},
		]);
		let out = saved(body);
		assert_eq!(without_pages(&out), without_pages(FILE), "{out}");
		let input = vag_cli_core::dash::parse_input(&out).unwrap();
		let titles: Vec<String> = input
			.pages
			.iter()
			.map(|p| match p {
				vag_cli_core::dash::PageInput::Values { title, .. } | vag_cli_core::dash::PageInput::Chart { title, .. } => title.clone(),
			})
			.collect();
		assert_eq!(titles, ["", "MAIN", "OIL"]);
		// A page keeps its own comments and what did not change; the pages stay where the first
		// one was, before `[strip]`.
		assert!(
			out.contains(
				"\n# ---- pages ----\n[[page]]\nkind = \"values\"\ntitle = \"MAIN\"\ncells = [\"01:IDE00002\", \"01:IDE00001\"]   # the two that matter\n"
			),
			"{out}"
		);
		assert!(
			out.contains("\n# the chart page\n[[page]]\nkind = \"chart\"\ncell = \"01:IDE00001\"\nmin = 70\nmax = 120\nvolts = false\n"),
			"{out}"
		);
		assert!(
			out.contains("\n[[page]]\nkind = \"values\"\ntitle = \"OIL\"\ncells = [\"01:IDE00004\"]\n"),
			"{out}"
		);
		let at = |needle: &str| out.find(needle).unwrap_or_else(|| panic!("{needle} in {out}"));
		assert!(at("# the chart page") < at("# ---- pages ----"), "in the editor's order: {out}");
		assert!(at("title = \"OIL\"") < at("# ---- the rest ----"), "{out}");
		// The channel no page shows any more is still there, with its alarm.
		assert!(out.contains("# not on a page, kept for its alarm\n[[channel]]\nref = \"01:IDE00004\"\nalarm_min = 0.5\n"));
	}

	#[test]
	fn a_new_channel_goes_after_the_last_with_its_name_and_its_label() {
		let mut body = as_opened();
		body["pages"][0]["cells"] = json!(["01:IDE00001", "01:IDE00002", "01:IDE00009", "01:1234@8"]);
		body["labels"]["01:IDE00009"] = json!("BOOST");
		let out = saved(body);
		assert!(
			out.contains(
				"zero_at_rest = true\n\n[[channel]]\nref = \"01:IDE00009\"   # Name of 01:IDE00009, °C\nlabel = \"BOOST\"\n\n[[channel]]\nref = \"01:1234@8\"   # Name of 01:1234@8, °C\n\n# ---- pages ----\n"
			),
			"{out}"
		);
		let input = vag_cli_core::dash::parse_input(&out).unwrap();
		assert_eq!(input.channels.len(), 5);
		assert_eq!(input.channels[3].label.as_deref(), Some("BOOST"));
		// A channel the file has, spelled with the unit's request id, is the same channel.
		let mut body = as_opened();
		body["pages"][0]["cells"] = json!(["7E0:IDE00001", "01:IDE00002"]);
		let out = saved(body);
		assert_eq!(vag_cli_core::dash::parse_input(&out).unwrap().channels.len(), 3, "{out}");
	}

	#[test]
	fn a_relabel_or_new_decimals_changes_that_channel_and_no_other() {
		let mut body = as_opened();
		body["labels"] = json!({"01:IDE00001": "WATER", "01:IDE00004": "LOW"});
		body["decimals"] = json!({"01:IDE00002": 2});
		let out = saved(body);
		let expected = FILE
			.replace("label = \"COOLANT\"", "label = \"WATER\"")
			.replace("decimals = 1", "decimals = 2")
			.replace("ref = \"01:IDE00004\"\n", "ref = \"01:IDE00004\"\nlabel = \"LOW\"\n");
		assert_eq!(out, expected);
		// A label taken away, and decimals with it: the keys go, the rest stays.
		let mut body = as_opened();
		body["labels"] = json!({});
		body["decimals"] = json!({});
		let out = saved(body);
		assert_eq!(out, FILE.replace("label = \"COOLANT\"\n", "").replace("decimals = 1\n", ""));
	}

	#[test]
	fn a_page_that_changes_kind_takes_the_keys_of_its_kind() {
		let mut body = as_opened();
		body["pages"][1] = json!({"kind": "values", "title": "", "cells": ["01:IDE00001"], "volts": true, "from": 1});
		let out = saved(body);
		assert!(
			out.contains("\n# the chart page\n[[page]]\nkind = \"values\"\ncells = [\"01:IDE00001\"]\n"),
			"{out}"
		);
		vag_cli_core::dash::parse_input(&out).unwrap();
	}

	#[test]
	fn a_car_with_no_file_gets_one_of_its_vin_its_channels_and_its_pages() {
		let body = json!({
			"fingerprint": "absent",
			"pages": [
				{"kind": "values", "title": "MAIN", "cells": ["01:IDE00001", "01:IDE00002"], "volts": true},
				{"kind": "chart", "title": "", "cell": "01:IDE00002", "min": -0.5, "max": 2.5, "volts": true},
			],
			"labels": {"01:IDE00002": "BOOST"},
		});
		let wanted = wanted(body, None).unwrap();
		let out = edit(None, "TESTVIN0000000001", &wanted, |r| {
			(r.to_string() == "01:IDE00001").then(|| "Coolant, °C".to_string())
		})
		.unwrap();
		assert_eq!(
			out,
			"vin = \"TESTVIN0000000001\"\n\n[[channel]]\nref = \"01:IDE00001\"   # Coolant, °C\n\n[[channel]]\nref = \"01:IDE00002\"\nlabel = \"BOOST\"\n\n\
			 [[page]]\nkind = \"values\"\ntitle = \"MAIN\"\ncells = [\"01:IDE00001\", \"01:IDE00002\"]\n\n\
			 [[page]]\nkind = \"chart\"\ncell = \"01:IDE00002\"\nmin = -0.5\nmax = 2.5\n"
		);
		vag_cli_core::dash::parse_input(&out).unwrap();
	}

	#[test]
	fn nothing_the_page_sends_is_taken_unchecked() {
		let refused = |change: &dyn Fn(&mut serde_json::Value), why: &str| {
			let mut body = as_opened();
			change(&mut body);
			let said = wanted(body, Some(FILE)).unwrap_err();
			assert!(said.contains(why), "{why}: {said}");
		};
		refused(&|b| b["pages"] = json!([]), "no page");
		refused(&|b| b["pages"][0]["kind"] = json!("gauge"), "kind \"gauge\"");
		refused(&|b| b["pages"][0]["cells"] = json!(["nope"]), "page 1:");
		refused(&|b| b["pages"][0]["cells"] = json!(["01:A", "01:B", "01:C", "01:D", "01:E"]), "5 cells");
		refused(&|b| b["pages"][1]["cell"] = json!(""), "shows no channel");
		refused(&|b| b["pages"][1]["min"] = json!(null), "needs min and max");
		refused(&|b| b["pages"][0]["min"] = json!(1), "a values page has cells");
		refused(&|b| b["pages"][1]["cells"] = json!([]), "not cells");
		refused(&|b| b["pages"][0]["title"] = json!("A\nB"), "control character");
		refused(&|b| b["pages"][0]["title"] = json!("X".repeat(MAX_NEW_TEXT + 1)), "longer than");
		refused(&|b| b["pages"][1]["from"] = json!(0), "two pages come from page 1");
		refused(&|b| b["pages"][1]["from"] = json!(7), "page 8 of a file with 2");
		refused(&|b| b["labels"] = json!({"01:IDE00001": "L".repeat(MAX_NEW_TEXT + 1)}), "longer than");
		refused(&|b| b["labels"] = json!({"01:IDE00001": "A\u{7}"}), "control character");
		refused(&|b| b["labels"] = json!({"01:X": "A", "7E0:X": "B"}), "two labels");
		refused(&|b| b["labels"] = json!({"no unit": "A"}), "a label's channel");
		refused(&|b| b["decimals"] = json!({"01:IDE00002": 4}), "decimals 4");
		refused(&|b| b["decimals"] = json!({"01:IDE00002": -1}), "decimals -1");
		let too_many: Vec<serde_json::Value> = (0..=vag_dash_render::pages::MAX_PAGES)
			.map(|_| json!({"kind": "values", "cells": []}))
			.collect();
		refused(&|b| b["pages"] = json!(too_many), "the board holds at most");
		// The shape itself: no field the page does not send, no field of the wrong type.
		for bad in [
			json!({"fingerprint": "f", "pages": [], "toml": "x"}),
			json!({"fingerprint": 1, "pages": []}),
		] {
			assert!(serde_json::from_value::<Body>(bad).is_err());
		}
		let mut body = as_opened();
		body["pages"][0]["extra"] = json!(true);
		assert!(serde_json::from_value::<Body>(body).is_err());
	}

	#[test]
	fn a_name_with_a_line_break_cannot_break_the_file() {
		let mut body = as_opened();
		body["pages"][0]["cells"] = json!(["01:IDE00009"]);
		let wanted = wanted(body, Some(FILE)).unwrap();
		let out = edit(Some(FILE), "TESTVIN0000000001", &wanted, |_| Some("two\nlines = 1".to_string())).unwrap();
		assert!(out.contains("ref = \"01:IDE00009\"   # two lines = 1\n"), "{out}");
		vag_cli_core::dash::parse_input(&out).unwrap();
	}

	#[test]
	fn pages_between_other_tables_go_together_where_the_first_was() {
		let file = "vin = \"TESTVIN0000000001\"\n\n[[channel]]\nref = \"01:IDE00001\"\n\n[[page]]\nkind = \"values\"\ncells = [\"01:IDE00001\"]\n\n\
		            # a channel written between the pages\n[[channel]]\nref = \"01:IDE00002\"\n\n[[page]]\nkind = \"values\"\ncells = [\"01:IDE00002\"]\n";
		let body = json!({
			"fingerprint": "f",
			"pages": [
				{"kind": "values", "cells": ["01:IDE00001"], "from": 0},
				{"kind": "values", "cells": ["01:IDE00002"], "from": 1},
			],
		});
		let wanted = wanted(body, Some(file)).unwrap();
		let out = edit(Some(file), "TESTVIN0000000001", &wanted, |_| None).unwrap();
		// The comment above the second channel stays with that channel, after the pages.
		assert_eq!(
			out,
			"vin = \"TESTVIN0000000001\"\n\n[[channel]]\nref = \"01:IDE00001\"\n\n[[page]]\nkind = \"values\"\ncells = [\"01:IDE00001\"]\n\n\
			 [[page]]\nkind = \"values\"\ncells = [\"01:IDE00002\"]\n\n# a channel written between the pages\n[[channel]]\nref = \"01:IDE00002\"\n"
		);
		vag_cli_core::dash::parse_input(&out).unwrap();
	}

	#[test]
	fn a_label_added_goes_after_ref_and_moves_nothing_else() {
		// The second channel holds a setpoint, a drift alarm and decimals, and no label.
		let mut body = as_opened();
		body["labels"]["01:IDE00002"] = json!("BOOST");
		let out = saved(body);
		assert_eq!(
			out,
			FILE.replace(
				"ref = \"01:IDE00002\"\ndecimals = 1\n",
				"ref = \"01:IDE00002\"\nlabel = \"BOOST\"\ndecimals = 1\n"
			),
			"the label after ref, and every key of the person's where it was"
		);
		// Decimals added where the label is the last of the ranked keys: after it, the rest stays.
		let file = "vin = \"TESTVIN0000000001\"\n[[channel]]\nref = \"01:IDE00001\"\nsetpoint = \"01:IDE00002\"\nlabel = \"A\"\nhz = 1\n[[page]]\nkind = \"values\"\ncells = [\"01:IDE00001\"]\n";
		let body = json!({
			"fingerprint": "f",
			"pages": [{"kind": "values", "cells": ["01:IDE00001"], "from": 0}],
			"labels": {"01:IDE00001": "A"},
			"decimals": {"01:IDE00001": 2},
		});
		let out = edit(Some(file), "TESTVIN0000000001", &wanted(body, Some(file)).unwrap(), |_| None).unwrap();
		assert!(
			out.contains("ref = \"01:IDE00001\"\nsetpoint = \"01:IDE00002\"\nlabel = \"A\"\ndecimals = 2\nhz = 1\n"),
			"{out}"
		);
	}

	#[test]
	fn a_long_label_or_title_the_file_has_is_saved_as_it_is() {
		// The build sets no length: a file it accepts saves, whatever it holds.
		let long = "A VERY LONG LABEL THAT NO CELL OF THE BOARD HOLDS, WRITTEN BY HAND, LONGER THAN ANY LIMIT";
		let file = FILE
			.replace("label = \"COOLANT\"", &format!("label = \"{long}\""))
			.replace("title = \"MAIN\"", &format!("title = \"{long}\""));
		let mut body = as_opened();
		body["labels"]["01:IDE00001"] = json!(long);
		body["pages"][0]["title"] = json!(long);
		let wanted = wanted(body.clone(), Some(&file)).unwrap();
		assert_eq!(edit(Some(&file), "TESTVIN0000000001", &wanted, |_| None).unwrap(), file);
		// The same text new to the file is refused.
		assert!(super::tests::wanted(body, Some(FILE)).unwrap_err().contains("longer than"));
	}
}
