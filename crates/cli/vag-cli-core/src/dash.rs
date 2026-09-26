//! `vagcan dev dash build` — the plan the dash firmware executes, resolved here.
//!
//! `todo/dash/01-plan-format.md` is the specification. The device resolves
//! nothing: every unit address, identifier, bit layout, scaling, unit string
//! and label it will ever use is decided on the laptop, where the catalogs
//! are, and written into a `static PLAN` the firmware `include!`s. This module
//! is that decision. It reads the same three things `watch` reads — the
//! project's cache, the proven rows, the owner's glossary — through the same
//! functions ([`crate::plan::available`], [`crate::extracted::Extracted`]), so
//! the dash cannot show a number `watch` would not.
//!
//! # The build input
//!
//! A small TOML the owner writes by hand, `~/.vagcan/dash/<VIN>/dash.toml`:
//!
//! ```toml
//! vin = "XW8AD4NE9JH008917"
//! language = "ru"                 # optional; the settings' language otherwise
//! survey = "…/survey.jsonl"        # optional; ~/.vagcan/cars/<VIN>/survey.jsonl otherwise
//!
//! [[channel]]
//! ref = "01:IDE00025"              # <unit>:<text id>, or <unit>:<DID>[@<bit offset>]
//! label = "ОЖ"                     # optional; the glossary's wording otherwise
//! decimals = 0                     # optional; derived from the scaling otherwise
//! hz = 10                          # optional; how often the panel reads it, 2 otherwise
//! setpoint = "01:IDE00190"         # optional; what the unit asked for, on the same unit
//!
//! [[channel]]
//! ref = "02:IDE00102"
//!
//! [[page]]
//! kind = "values"
//! title = "MAIN"
//! cells = ["01:IDE00025", "02:IDE00102"]
//!
//! [[page]]
//! kind = "chart"
//! cell = "01:IDE00025"
//! min = 70
//! max = 110
//!
//! [[alarm]]                        # optional; at most 4, in priority order
//! channels = ["01:IDE00025"]       # each under [[channel]], all shown on `page`
//! page = "MAIN"                    # the title of a values page
//! direction = "above"              # or "below"
//! trip = 105                       # fires at or past this
//! release = 100                    # clears only once back past this
//!
//! [[alarm]]                        # the other kind: drift from a specified value
//! kind = "drift"
//! channels = ["01:IDE00191"]       # each with a `setpoint` of its own
//! page = "MAIN"
//! percent = 10                     # fires past this share of the specified value
//! release_percent = 6              # clears under this share
//! hold_ms = 1000                   # and only once the drift has held that long
//! min_setpoint = 0.5               # under this specified value the rule says nothing
//! ```
//!
//! A unit is spelled the way every other command spells it — `01`, `02`, or a
//! request id — and a channel by the text id the row carries (stable across
//! variants, and the key the glossary is written under) or by identifier and
//! bit offset when it has none. Which unit *variant* the car has is not
//! written here: it comes from the survey, from what the unit said about
//! itself, exactly as `watch` finds it.
//!
//! # Refusals
//!
//! A channel the resolved variant does not declare fails the build and the
//! message names it. So does one whose scaling is not linear — an enum or an
//! unreversed anchor cannot be multiplied, and a plan that carried a guess
//! would give away the one thing it is for. Only `0x22` reads exist in the
//! catalog's vocabulary ([`ReadId::Uds`]), so no other service can be asked
//! for, by construction rather than by check.
//!
//! # Outputs
//!
//! `plan.json` for a person and the simulator, `plan.rs` for the firmware, both
//! under `~/.vagcan/dash/<VIN>/` and neither committed anywhere: they are
//! derived from VW's data and describe one owner's car.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml_edit::{DocumentMut, Item};
use vag_dash_render::alarm::MAX_ALARMS;
use vag_dash_render::pages::MAX_PAGES;
use vag_data_labels::catalog::{CatalogStore, ReadId, Scaling};
use vag_data_labels::measure::RawForm;
use vag_uds_client::address::{self, UnitAddress};

use crate::config::Language;
use crate::extracted::Extracted;
use crate::plan::{self as poll, UnitIdentity};

/// How a build input names one channel.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reference {
	/// `<unit>:<text id>` — the join key the label files and the glossary share.
	TextId { request: u16, text_id: String },
	/// `<unit>:<DID>[@<bit offset>]` — for a row that carries no text id, such
	/// as a proven one or a standard OBD-II parameter.
	Field { request: u16, did: u16, bit_offset: u32 },
}

impl Reference {
	/// Parse the spelling the input uses. The unit part accepts whatever
	/// [`vag_uds_client::address::parse`] accepts.
	pub fn parse(text: &str) -> Result<Reference, Error> {
		let text = text.trim();
		let (unit, rest) = text
			.split_once(':')
			.ok_or_else(|| Error::Parse(format!("{text:?}: a channel is <unit>:<text id> or <unit>:<DID>[@<bit>]")))?;
		let request = address::parse(unit).map_err(Error::Parse)?.request;
		let rest = rest.trim();
		if rest.is_empty() {
			return Err(Error::Parse(format!("{text:?}: nothing after the unit")));
		}
		let (did_text, bits) = match rest.split_once('@') {
			Some((d, b)) => (d, Some(b)),
			None => (rest, None),
		};
		let looks_like_did = did_text.len() == 4 && did_text.chars().all(|c| c.is_ascii_hexdigit());
		if looks_like_did {
			let did = u16::from_str_radix(did_text, 16).map_err(|_| Error::Parse(format!("{text:?}: {did_text:?} is not a hex identifier")))?;
			let bit_offset = match bits {
				Some(b) => b
					.trim()
					.parse::<u32>()
					.map_err(|_| Error::Parse(format!("{text:?}: {b:?} is not a bit offset")))?,
				None => 0,
			};
			Ok(Reference::Field { request, did, bit_offset })
		} else {
			if bits.is_some() {
				return Err(Error::Parse(format!("{text:?}: a bit offset goes with an identifier, not a text id")));
			}
			Ok(Reference::TextId {
				request,
				text_id: rest.to_string(),
			})
		}
	}

	fn request(&self) -> u16 {
		match self {
			Reference::TextId { request, .. } | Reference::Field { request, .. } => *request,
		}
	}
}

impl fmt::Display for Reference {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let unit = UnitAddress::from_request(self.request())
			.map(|a| a.label())
			.unwrap_or_else(|| format!("{:03X}", self.request()));
		match self {
			Reference::TextId { text_id, .. } => write!(f, "{unit}:{text_id}"),
			Reference::Field { did, bit_offset: 0, .. } => write!(f, "{unit}:{did:04X}"),
			Reference::Field { did, bit_offset, .. } => write!(f, "{unit}:{did:04X}@{bit_offset}"),
		}
	}
}

/// One `[[channel]]` of the input.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelInput {
	pub reference: Reference,
	/// The panel's wording, when the glossary's is not it. Ten characters.
	pub label: Option<String>,
	pub decimals: Option<u8>,
	/// How often the panel reads it while its page is shown, in readings a second.
	/// [`DEFAULT_HZ`] when absent. Written by the owner, never derived: a rate taken
	/// from a unit of measure would be a guess about what the owner wants to see.
	pub hz: Option<f64>,
	/// The channel holding what the unit asked for, where the owner paired one
	/// (`todo/dash/18-setpoints-and-drift.md`). Written down and never guessed: the label
	/// files spell the two halves of a pair three different ways, and a wrong pair shows a
	/// difference that means nothing.
	pub setpoint: Option<Reference>,
}

/// A channel's rate when `dash.toml` gives none (owner, 2026-09-14).
pub const DEFAULT_HZ: f64 = 2.0;
/// The fastest rate a channel may ask for: the board's whole ceiling of exchanges
/// a second (`vag_uds_client::schedule::Budget::ceiling_per_s`), so one channel can
/// never ask for more than the bus is given.
pub const MAX_HZ: f64 = 100.0;

/// One `[[page]]` of the input.
#[derive(Debug, Clone, PartialEq)]
pub enum PageInput {
	Values { title: String, cells: Vec<Reference> },
	Chart { cell: Reference, min: f64, max: f64 },
}

/// Which way a reading has to go for an `[[alarm]]` to fire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
	/// At or below `trip`; clears above `release`.
	Below,
	/// At or above `trip`; clears below `release`.
	Above,
}

/// What an `[[alarm]]` watches for: a threshold, or drift from a specified value.
#[derive(Debug, Clone, PartialEq)]
pub enum AlarmRuleInput {
	Threshold {
		direction: Direction,
		trip: f64,
		release: f64,
	},
	/// `todo/dash/18-setpoints-and-drift.md` §4. Every channel it watches must have a
	/// `setpoint`, or there is nothing to be far from.
	Drift {
		percent: f64,
		release_percent: f64,
		hold_ms: u64,
		min_setpoint: f64,
	},
}

/// One `[[alarm]]` of the input: the owner's rule, never the code's.
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmInput {
	pub channels: Vec<Reference>,
	/// The title of the values page the rule raises.
	pub page: String,
	pub rule: AlarmRuleInput,
}

/// The whole input, parsed and nothing more.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
	pub vin: String,
	pub language: Option<Language>,
	pub survey: Option<PathBuf>,
	pub channels: Vec<ChannelInput>,
	pub pages: Vec<PageInput>,
	/// In the file's order, which is priority.
	pub alarms: Vec<AlarmInput>,
}

/// Parse a build input. Only the shape is checked here; whether the car has
/// the channels is [`build`]'s question.
pub fn parse_input(text: &str) -> Result<Input, Error> {
	let doc: DocumentMut = text.parse().map_err(|e| Error::Parse(format!("dash.toml: {e}")))?;
	let string = |item: Option<&Item>, what: &str| -> Result<String, Error> {
		item
			.and_then(Item::as_str)
			.map(|s| s.trim().to_string())
			.filter(|s| !s.is_empty())
			.ok_or_else(|| Error::Parse(format!("dash.toml: {what} is missing or not a string")))
	};
	let vin = string(doc.get("vin"), "vin")?;
	let language = match doc.get("language").and_then(Item::as_str) {
		Some(code) => {
			Some(Language::parse(code).ok_or_else(|| Error::Parse(format!("dash.toml: language {code:?} is not one this build has words for")))?)
		}
		None => None,
	};
	let survey = doc.get("survey").and_then(Item::as_str).map(PathBuf::from);

	let mut channels = Vec::new();
	if let Some(tables) = doc.get("channel").and_then(Item::as_array_of_tables) {
		for (i, table) in tables.iter().enumerate() {
			let reference = Reference::parse(&string(table.get("ref"), &format!("channel #{}'s ref", i + 1))?)?;
			let label = table
				.get("label")
				.and_then(Item::as_str)
				.map(|s| s.trim().to_string())
				.filter(|s| !s.is_empty());
			let decimals = match table.get("decimals").and_then(Item::as_integer) {
				Some(d) if (0..=3).contains(&d) => Some(d as u8),
				Some(d) => return Err(Error::Parse(format!("dash.toml: {reference}: decimals {d} is not 0..=3"))),
				None => None,
			};
			let hz = match table.get("hz") {
				None => None,
				Some(item) => match item.as_float().or_else(|| item.as_integer().map(|n| n as f64)) {
					Some(hz) if hz.is_finite() && hz > 0.0 && hz <= MAX_HZ => Some(hz),
					_ => {
						return Err(Error::Parse(format!(
							"dash.toml: {reference}: hz must be a number above 0 and at most {MAX_HZ}"
						)));
					}
				},
			};
			let setpoint = match table.get("setpoint") {
				None => None,
				Some(item) => Some(Reference::parse(
					item
						.as_str()
						.ok_or_else(|| Error::Parse(format!("dash.toml: {reference}: setpoint is not a string")))?,
				)?),
			};
			channels.push(ChannelInput {
				reference,
				label,
				decimals,
				hz,
				setpoint,
			});
		}
	}
	if channels.is_empty() {
		return Err(Error::Parse(
			"dash.toml: no [[channel]] — a plan with nothing to read is not a plan".to_string(),
		));
	}

	let number = |item: Option<&Item>| item.and_then(|i| i.as_float().or_else(|| i.as_integer().map(|n| n as f64)));
	let mut pages = Vec::new();
	if let Some(tables) = doc.get("page").and_then(Item::as_array_of_tables) {
		for (i, table) in tables.iter().enumerate() {
			let n = i + 1;
			let kind = string(table.get("kind"), &format!("page #{n}'s kind"))?;
			match kind.as_str() {
				"values" => {
					let title = table.get("title").and_then(Item::as_str).unwrap_or("").trim().to_string();
					let cells = table
						.get("cells")
						.and_then(Item::as_array)
						.ok_or_else(|| Error::Parse(format!("dash.toml: page #{n} has no cells")))?
						.iter()
						.map(|v| {
							v.as_str()
								.ok_or_else(|| Error::Parse(format!("dash.toml: page #{n}: a cell is not a string")))
								.and_then(Reference::parse)
						})
						.collect::<Result<Vec<_>, _>>()?;
					pages.push(PageInput::Values { title, cells });
				}
				"chart" => {
					let cell = Reference::parse(&string(table.get("cell"), &format!("page #{n}'s cell"))?)?;
					let min = number(table.get("min")).ok_or_else(|| Error::Parse(format!("dash.toml: page #{n} needs min")))?;
					let max = number(table.get("max")).ok_or_else(|| Error::Parse(format!("dash.toml: page #{n} needs max")))?;
					pages.push(PageInput::Chart { cell, min, max });
				}
				other => {
					return Err(Error::Parse(format!(
						"dash.toml: page #{n}: kind {other:?} is not \"values\" or \"chart\""
					)));
				}
			}
		}
	}
	if pages.is_empty() {
		return Err(Error::Parse("dash.toml: no [[page]]".to_string()));
	}

	let mut alarms = Vec::new();
	if let Some(item) = doc.get("alarm") {
		// A single `[alarm]` table would otherwise be skipped without a word, and an alarm
		// the owner believes is armed and is not is the one failure an alarm cannot have.
		let tables = item
			.as_array_of_tables()
			.ok_or_else(|| Error::Parse("dash.toml: alarm must be written as [[alarm]] tables, one per rule".to_string()))?;
		for (i, table) in tables.iter().enumerate() {
			let n = i + 1;
			let channels = table
				.get("channels")
				.and_then(Item::as_array)
				.ok_or_else(|| Error::Parse(format!("dash.toml: alarm #{n} has no channels list")))?
				.iter()
				.map(|v| {
					v.as_str()
						.ok_or_else(|| Error::Parse(format!("dash.toml: alarm #{n}: a channel is not a string")))
						.and_then(Reference::parse)
				})
				.collect::<Result<Vec<_>, _>>()?;
			let page = string(table.get("page"), &format!("alarm #{n}'s page"))?;
			// The board compares in `f32`, so a threshold past what one holds is refused
			// rather than turned into an infinity nothing ever reaches.
			let threshold = |what: &str| match number(table.get(what)) {
				Some(v) if v.is_finite() && (v as f32).is_finite() => Ok(v),
				_ => Err(Error::Parse(format!("dash.toml: alarm #{n} needs {what}, a finite number"))),
			};
			// No `kind` is the threshold rule, so every `dash.toml` written before drift
			// existed still builds.
			let rule = match table.get("kind").and_then(Item::as_str).map(str::trim).unwrap_or("threshold") {
				"threshold" => {
					let direction = match string(table.get("direction"), &format!("alarm #{n}'s direction"))?.as_str() {
						"below" => Direction::Below,
						"above" => Direction::Above,
						other => {
							return Err(Error::Parse(format!(
								"dash.toml: alarm #{n}: direction {other:?} is not \"below\" or \"above\""
							)));
						}
					};
					AlarmRuleInput::Threshold {
						direction,
						trip: threshold("trip")?,
						release: threshold("release")?,
					}
				}
				"drift" => {
					let share = |what: &str| match threshold(what)? {
						v if v > 0.0 => Ok(v),
						v => Err(Error::Parse(format!("dash.toml: alarm #{n}: {what} {v} is not above zero"))),
					};
					let hold_ms = match table.get("hold_ms").and_then(Item::as_integer) {
						Some(ms) if ms >= 0 => ms as u64,
						_ => {
							return Err(Error::Parse(format!(
								"dash.toml: alarm #{n} needs hold_ms, whole milliseconds the drift has to hold"
							)));
						}
					};
					let min_setpoint = match threshold("min_setpoint")? {
						v if v >= 0.0 => v,
						v => return Err(Error::Parse(format!("dash.toml: alarm #{n}: min_setpoint {v} is below zero"))),
					};
					AlarmRuleInput::Drift {
						percent: share("percent")?,
						release_percent: share("release_percent")?,
						hold_ms,
						min_setpoint,
					}
				}
				other => {
					return Err(Error::Parse(format!(
						"dash.toml: alarm #{n}: kind {other:?} is not \"threshold\" or \"drift\""
					)));
				}
			};
			alarms.push(AlarmInput { channels, page, rule });
		}
	}
	Ok(Input {
		vin,
		language,
		survey,
		channels,
		pages,
		alarms,
	})
}

/// What can go wrong between an input and a plan. Every variant names the
/// thing that failed, because "the build failed" is not something a person can
/// act on.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
	Parse(String),
	/// The car reported nothing about this unit — no survey line for it.
	UnknownUnit(u16),
	/// A request id in neither of the blocks this tool knows the response rule for.
	NoResponseRule(u16),
	/// The resolved variant does not declare it and nothing proved it.
	Undeclared(Reference),
	/// More than one row answers to it; the input has to say which.
	Ambiguous(Reference, Vec<String>),
	/// Declared, but with a scaling the device cannot apply.
	NotLinear(Reference, String),
	/// The same channel twice in `[[channel]]` — the second would be dropped
	/// with its label, and dropping silently is how a wrong label ships.
	Duplicate(Reference),
	/// The car's own survey put this identifier to the unit and it was silent.
	NotAnswered(Reference),
	/// A factor or offset that is not a number the device can multiply by.
	NotFinite(Reference),
	/// The survey has no `F187` for this unit, so the firmware could never
	/// confirm it is talking to the unit the plan was built for.
	NoPartNumber(u16),
	/// A page names a channel the input's `[[channel]]` list does not carry.
	PageRefersToUnknown {
		page: usize,
		reference: Reference,
	},
	Page(usize, String),
	/// An `[[alarm]]` the board could not honour, by its number in the file.
	Alarm(usize, String),
	/// More `[[alarm]]` rules than [`MAX_ALARMS`].
	TooManyAlarms(usize),
	/// More `[[page]]` tables than the board holds, [`MAX_PAGES`].
	TooManyPages(usize),
	/// One row declared twice under two spellings — `01:IDE00191` and `01:202A`.
	SameRow {
		first: Reference,
		second: Reference,
	},
	/// A `setpoint` the plan cannot pair with its channel.
	Setpoint {
		channel: Reference,
		setpoint: Reference,
		why: String,
	},
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Error::Parse(why) => write!(f, "{why}"),
			Error::UnknownUnit(r) => write!(
				f,
				"unit {:03X} is not in the survey — the car has not said what it is, so nothing can be resolved for it",
				r
			),
			Error::NoResponseRule(r) => write!(f, "unit {r:03X}: no rule for which id it answers on"),
			Error::Undeclared(r) => write!(f, "{r}: the car's variant does not declare this channel and nothing has proven it"),
			Error::Ambiguous(r, rows) => write!(
				f,
				"{r}: {} rows answer to it — name one by identifier and bit offset: {}",
				rows.len(),
				rows.join(", ")
			),
			Error::NotLinear(r, s) => write!(f, "{r}: scaling is {s}, not linear — the device can multiply and nothing else"),
			Error::Duplicate(r) => write!(f, "{r} is listed twice under [[channel]]"),
			Error::SameRow { first, second } => write!(
				f,
				"{first} and {second} are the same row — one unit, identifier, bits and scaling written two ways; keep one [[channel]]"
			),
			Error::NotAnswered(r) => write!(f, "{r}: the survey asked the unit for this identifier and it did not answer"),
			Error::NotFinite(r) => write!(f, "{r}: its scaling is not a finite number"),
			Error::NoPartNumber(r) => write!(
				f,
				"unit {r:03X}: the survey has no part number (F187) for it, and the firmware checks the unit against the plan by that"
			),
			Error::PageRefersToUnknown { page, reference } => write!(f, "page #{page}: {reference} is not in the [[channel]] list"),
			Error::Page(n, why) => write!(f, "page #{n}: {why}"),
			Error::Alarm(n, why) => write!(f, "alarm #{n}: {why}"),
			Error::TooManyPages(n) => write!(f, "{n} [[page]] tables, and the board holds at most {MAX_PAGES}"),
			Error::Setpoint { channel, setpoint, why } => {
				write!(f, "{channel}: its setpoint {setpoint} {why}")
			}
			Error::TooManyAlarms(n) => write!(
				f,
				"{n} [[alarm]] rules, and the board holds at most {MAX_ALARMS} — each rule's channels are read at full rate on every page"
			),
		}
	}
}

impl std::error::Error for Error {}

/// One unit of the plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Unit {
	pub request: u16,
	pub response: u16,
	pub part_number: String,
	pub odx_name: Option<String>,
}

/// One channel, fully resolved. Field for field what the firmware holds, plus
/// the two provenance fields a person reading `plan.json` wants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
	pub unit: u16,
	pub did: u16,
	pub bit_offset: u32,
	pub bit_length: u32,
	pub signed: bool,
	pub big_endian: bool,
	pub factor: f64,
	pub offset: f64,
	pub decimals: u8,
	pub unit_text: String,
	pub label: String,
	pub proven: bool,
	/// Readings a second while the channel's page is shown. A `plan.json` written
	/// before rates existed reads as [`DEFAULT_HZ`].
	#[serde(default = "default_hz")]
	pub hz: f64,
	/// Where the row came from: its text id, or the catalog's own name.
	pub source: String,
	/// The channel holding what the unit asked for, by index into the plan's channels. A
	/// `plan.json` written before setpoints existed has none.
	#[serde(default)]
	pub setpoint: Option<u16>,
}

fn default_hz() -> f64 {
	DEFAULT_HZ
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Page {
	Chart { channel: u16, min: f64, max: f64 },
	Values { title: String, cells: Vec<u16> },
}

/// One alarm's rule, resolved. `specified` is indices into the plan's channels, one per
/// watched channel, in the same order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AlarmRule {
	Threshold {
		direction: Direction,
		trip: f64,
		release: f64,
	},
	Drift {
		specified: Vec<u16>,
		percent: f64,
		release_percent: f64,
		hold_ms: u64,
		min_setpoint: f64,
	},
}

/// One alarm, resolved: indices into the plan's channels and pages, which is what
/// `vag_dash_render::alarm::ChannelId` and `PageId` are for an image built for one plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alarm {
	pub channels: Vec<u16>,
	pub page: u16,
	#[serde(flatten)]
	pub rule: AlarmRule,
}

/// The plan, as `plan.json` holds it. [`to_rust`] writes the same content as
/// the `static` the firmware links.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Plan {
	pub vin: String,
	pub language: String,
	pub units: Vec<Unit>,
	pub channels: Vec<Channel>,
	pub pages: Vec<Page>,
	/// In priority order. A `plan.json` written before alarms existed has none.
	#[serde(default)]
	pub alarms: Vec<Alarm>,
}

impl Plan {
	pub fn to_json(&self) -> String {
		serde_json::to_string_pretty(self).expect("a plan serialises")
	}

	pub fn from_json(text: &str) -> Result<Plan, serde_json::Error> {
		serde_json::from_str(text)
	}

	/// The plan as the board holds it — what [`to_rust`] writes as source, built in memory
	/// for a host program that runs the board's own code over it (`vagcan dev recording
	/// dash`). Numbers are narrowed to `f32` exactly as the source spells them.
	///
	/// **It leaks.** The board's plan is `&'static` throughout because on the board it is a
	/// `static`; a host process builds one per run and keeps it to the end, so the few
	/// kilobytes are the process's for its lifetime either way.
	pub fn to_device(&self) -> vag_dash_render::plan::Plan {
		use vag_dash_render::alarm::{self as device_alarm, ChannelId, PageId};
		use vag_dash_render::plan as device;
		fn text(s: &str) -> &'static str {
			String::leak(s.to_string())
		}
		fn ids(channels: &[u16]) -> &'static [ChannelId] {
			Vec::leak(channels.iter().map(|&c| ChannelId(c)).collect())
		}
		let units = self
			.units
			.iter()
			.map(|u| device::Unit {
				request: u.request,
				response: u.response,
				part_number: text(&u.part_number),
			})
			.collect();
		let channels = self
			.channels
			.iter()
			.map(|c| device::Channel {
				unit: c.unit,
				did: c.did,
				bit_offset: c.bit_offset,
				bit_length: c.bit_length,
				signed: c.signed,
				big_endian: c.big_endian,
				factor: c.factor as f32,
				offset: c.offset as f32,
				decimals: c.decimals,
				unit_text: text(&c.unit_text),
				label: text(&c.label),
				proven: c.proven,
				hz: c.hz as f32,
				setpoint: c.setpoint,
			})
			.collect();
		let pages = self
			.pages
			.iter()
			.map(|p| match p {
				Page::Chart { channel, min, max } => device::Page::Chart {
					channel: *channel,
					min: *min as f32,
					max: *max as f32,
				},
				Page::Values { title, cells } => device::Page::Values {
					title: text(title),
					cells: Vec::leak(cells.clone()),
				},
			})
			.collect();
		let alarms = self
			.alarms
			.iter()
			.map(|a| device_alarm::Alarm {
				channels: ids(&a.channels),
				page: PageId(a.page),
				rule: match &a.rule {
					AlarmRule::Threshold { direction, trip, release } => device_alarm::Rule::Threshold {
						trip: *trip as f32,
						release: *release as f32,
						direction: match direction {
							Direction::Below => device_alarm::Direction::Below,
							Direction::Above => device_alarm::Direction::Above,
						},
					},
					AlarmRule::Drift {
						specified,
						percent,
						release_percent,
						hold_ms,
						min_setpoint,
					} => device_alarm::Rule::Drift {
						specified: ids(specified),
						percent: *percent as f32,
						release_percent: *release_percent as f32,
						hold_ms: *hold_ms,
						min_setpoint: *min_setpoint as f32,
					},
				},
			})
			.collect();
		device::Plan {
			vin: text(&self.vin),
			language: text(&self.language),
			units: Vec::leak(units),
			channels: Vec::leak(channels),
			pages: Vec::leak(pages),
			alarms: Vec::leak(alarms),
		}
	}
}

/// The plan and the build log that explains every row of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
	pub plan: Plan,
	pub notes: Vec<String>,
}

/// What a channel reads on the bus: unit, identifier, and the bits taken from the answer —
/// which is what makes two resolved channels one channel, however each was spelled.
///
/// Scaling is not part of it, and need not be: `plan::available` offers one row per field
/// (unit, identifier, bit offset), a later definition replacing an earlier one, so one field
/// never resolves to two scalings.
fn read_of(c: &Channel) -> (u16, u16, u32, u32) {
	(c.unit, c.did, c.bit_offset, c.bit_length)
}

/// The plan index of a channel the input names, by what the name resolves to: a page cell or an
/// alarm channel may spell a row differently from its `[[channel]]` and still mean it. `None`
/// when no `[[channel]]` resolves to that row.
fn index_by_row(
	reference: &Reference,
	channels: &[Channel],
	index_of: &BTreeMap<Reference, u16>,
	offered: &[poll::Channel],
	answered: Option<&poll::Answered>,
	units: &[UnitIdentity],
) -> Option<u16> {
	if let Some(index) = index_of.get(reference) {
		return Some(*index);
	}
	let probe = ChannelInput {
		reference: reference.clone(),
		label: None,
		decimals: None,
		hz: None,
		setpoint: None,
	};
	let resolved = resolve_channel(&probe, offered, answered, units, &mut Vec::new()).ok()?;
	channels.iter().position(|c| read_of(c) == read_of(&resolved)).map(|at| at as u16)
}

/// One `[[channel]]` against what the car reported and what the project knows: the same rules
/// for a channel the owner named and for a specified value it paired with (`todo/dash/18`).
fn resolve_channel(
	wanted: &ChannelInput,
	offered: &[poll::Channel],
	answered: Option<&poll::Answered>,
	units: &[UnitIdentity],
	notes: &mut Vec<String>,
) -> Result<Channel, Error> {
	let request = wanted.reference.request();
	if !units.iter().any(|u| u.request == request) {
		return Err(Error::UnknownUnit(request));
	}
	let matches: Vec<&poll::Channel> = offered
		.iter()
		.filter(|c| c.request == request && c.def.is_some())
		.filter(|c| match &wanted.reference {
			Reference::TextId { text_id, .. } => c.text_id.as_deref() == Some(text_id.as_str()),
			Reference::Field { did, bit_offset, .. } => c.did == *did && c.def.as_ref().map_or(0, |d| d.raw_form.bit_offset()) == *bit_offset,
		})
		.collect();
	// A text id can name a field the device cannot show beside one it can:
	// on the reference car every OBD-II parameter's id also sits on its
	// "supported" bit in the `F400`/`F420`/… masks, an enum with no unit. A
	// numeric cell can only take a linear row, so only those are candidates;
	// what remains ambiguous is ambiguous.
	let (linear, other): (Vec<&poll::Channel>, Vec<&poll::Channel>) = matches
		.iter()
		.partition(|c| matches!(c.def.as_ref().map(|d| &d.scaling), Some(Scaling::Linear(_))));
	let found = match (linear.as_slice(), other.as_slice()) {
		([one], _) => *one,
		([], []) => return Err(Error::Undeclared(wanted.reference.clone())),
		([], [first, ..]) => {
			let kind = match first.def.as_ref().map(|d| &d.scaling) {
				Some(Scaling::Enum { .. }) => "an enumeration",
				Some(Scaling::Anchor { .. }) => "a single proven point with no slope",
				_ => "not a quantity",
			};
			return Err(Error::NotLinear(wanted.reference.clone(), kind.to_string()));
		}
		(many, _) => {
			let names = many
				.iter()
				.map(|c| format!("{:04X}@{} {}", c.did, c.def.as_ref().map_or(0, |d| d.raw_form.bit_offset()), c.label()))
				.collect();
			return Err(Error::Ambiguous(wanted.reference.clone(), names));
		}
	};
	let def = found.def.as_ref().expect("filtered on def");
	let ReadId::Uds(did) = def.address;
	let (factor, offset) = match &def.scaling {
		Scaling::Linear(s) => (s.factor, s.offset),
		Scaling::Enum { .. } => return Err(Error::NotLinear(wanted.reference.clone(), "an enumeration".to_string())),
		Scaling::Anchor { .. } => {
			return Err(Error::NotLinear(
				wanted.reference.clone(),
				"a single proven point with no slope".to_string(),
			));
		}
	};
	if !factor.is_finite() || !offset.is_finite() {
		return Err(Error::NotFinite(wanted.reference.clone()));
	}
	// What the catalog declares is one thing; what the car answers is the
	// survey's to say. Silence where the survey asked is a refusal to build
	// on — an identifier that never comes back is a dash forever, and a
	// plan is for showing numbers. Where the survey never asked, nothing
	// is claimed either way (`Answered::saw`), and a standard OBD-II row
	// says so in the log, because the standard mandates it and this car
	// may still not carry it.
	let standard = !found.proven && found.text_id.is_none();
	match answered.and_then(|a| a.saw(request, did)) {
		Some(false) => return Err(Error::NotAnswered(wanted.reference.clone())),
		None if standard => notes.push(format!(
			"{}: a standard OBD-II row; the survey has no record of the car answering {did:04X}",
			wanted.reference
		)),
		_ => {}
	}
	let (bit_offset, bit_length, signed, big_endian) = bits_of(def.raw_form);
	let label = wanted.label.clone().unwrap_or_else(|| found.label());
	let decimals = wanted.decimals.unwrap_or_else(|| decimals_for(factor));
	let source = found.text_id.clone().unwrap_or_else(|| def.name.to_string());
	let hz = wanted.hz.unwrap_or(DEFAULT_HZ);
	notes.push(format!(
		"{label} ← {} {did:04X}@{bit_offset}/{bit_length} {} {}{} ×{factor} {offset:+} at {hz} Hz {} ({})",
		wanted.reference,
		if signed { "i" } else { "u" },
		if big_endian { "BE" } else { "LE" },
		if bit_length % 8 == 0 { "" } else { " bits" },
		if found.proven { "proven" } else { "declared" },
		source
	));
	Ok(Channel {
		unit: request,
		did,
		bit_offset,
		bit_length,
		signed,
		big_endian,
		factor,
		offset,
		decimals,
		unit_text: def.unit.to_string(),
		label,
		proven: found.proven,
		hz,
		source,
		setpoint: None,
	})
}

/// Resolve an input against what the car reported and what the project knows.
///
/// `units` are the car's own words about itself (from its survey); `store` and
/// `extracted` are the project — the same two `watch` opens. Pure: reads
/// nothing but its arguments, writes nothing.
pub fn build(
	input: &Input,
	store: &CatalogStore,
	extracted: &Extracted,
	units: &[UnitIdentity],
	answered: Option<&poll::Answered>,
	default_language: Language,
) -> Result<Built, Error> {
	let language = input.language.unwrap_or(default_language);
	let offered = poll::available(store, extracted, units);
	let mut notes = Vec::new();
	let mut channels: Vec<Channel> = Vec::new();
	let mut index_of: BTreeMap<Reference, u16> = BTreeMap::new();

	for wanted in &input.channels {
		if index_of.contains_key(&wanted.reference) {
			return Err(Error::Duplicate(wanted.reference.clone()));
		}
		let mut resolution = Vec::new();
		let resolved = resolve_channel(wanted, &offered, answered, units, &mut resolution)?;
		// The same row under its other spelling is the same row: `01:IDE00191` and `01:202A`
		// would otherwise both be added, both subscribed and both drawable (review,
		// 2026-09-15).
		if let Some(at) = channels.iter().position(|c| read_of(c) == read_of(&resolved)) {
			return Err(Error::SameRow {
				first: input.channels[at].reference.clone(),
				second: wanted.reference.clone(),
			});
		}
		notes.append(&mut resolution);
		let index = channels.len() as u16;
		channels.push(resolved);
		index_of.insert(wanted.reference.clone(), index);
	}

	// Second pass, so a setpoint may name a channel the input declares later — and so a
	// setpoint the input does not declare at all is appended once, after everything the
	// owner asked for by name.
	for (i, wanted) in input.channels.iter().enumerate() {
		let Some(reference) = wanted.setpoint.clone() else {
			continue;
		};
		let refuse = |why: &str| {
			Err(Error::Setpoint {
				channel: wanted.reference.clone(),
				setpoint: reference.clone(),
				why: why.to_string(),
			})
		};
		if reference.request() != wanted.reference.request() {
			// Two units are two exchanges and two moments; the difference between them would
			// be partly the delay (`todo/dash/18` §2).
			return refuse("is on another unit — a pair is read in one request, so both halves live on one");
		}
		// **Resolved, then compared — never compared as spellings.** `01:IDE00191` and
		// `01:202A` are two ways of writing one row, and a pair that looked different but read
		// the same identifier would draw `+0.00` for ever and never trip a drift rule
		// (review, 2026-09-15).
		let hidden = ChannelInput {
			reference: reference.clone(),
			label: None,
			decimals: wanted.decimals,
			hz: wanted.hz,
			setpoint: None,
		};
		// Resolved into a log of its own: a setpoint that turns out to be a channel the input
		// already has is not added, and a build log saying it was would be a lie.
		let mut resolution = Vec::new();
		let resolved = match index_of.get(&reference) {
			// Already a `[[channel]]`: the owner's own, with the rate they gave it.
			Some(index) => channels[*index as usize].clone(),
			None => resolve_channel(&hidden, &offered, answered, units, &mut resolution)?,
		};
		// The same read is refused whatever the scaling: one raw value scaled two ways and
		// subtracted from itself is not a difference anyone asked for. The width is part of the
		// read — two fields can share an identifier and an offset (review, 2026-09-15).
		if read_of(&resolved) == read_of(&channels[i]) {
			return refuse("is the channel itself — the same unit, identifier and bits, however it is spelled");
		}
		// Its own `[[channel]]`, by the channel it resolves to rather than by how it was written.
		let existing = channels.iter().position(|c| read_of(c) == read_of(&resolved)).map(|at| at as u16);
		if let Some(index) = existing
			&& usize::from(index) < input.channels.len()
			&& input.channels[usize::from(index)].setpoint.is_some()
		{
			return refuse("has a setpoint of its own");
		}
		// What is actually paired: the existing channel where there is one — with the rate and
		// unit it was declared with, however its spelling differs from this one — and the fresh
		// resolution otherwise (review, 2026-09-15).
		let paired = existing.map_or(&resolved, |index| &channels[usize::from(index)]);
		// Both halves go out in one request only if both are due at the same rate; at two rates
		// the difference is between numbers up to a period apart.
		if paired.hz != channels[i].hz {
			return refuse(&format!(
				"is read at {} Hz and the channel it explains at {} Hz — a pair is read in one request, so they share a rate",
				paired.hz, channels[i].hz
			));
		}
		if paired.unit_text != channels[i].unit_text {
			return refuse(&format!(
				"reads in {:?} against the channel's {:?}",
				paired.unit_text, channels[i].unit_text
			));
		}
		let index = match existing {
			Some(index) => index,
			None => {
				// Not a `[[channel]]` of its own: added once, at its channel's rate, and never
				// offered as a cell.
				let index = channels.len() as u16;
				channels.push(resolved);
				index_of.insert(reference.clone(), index);
				notes.append(&mut resolution);
				index
			}
		};
		channels[i].setpoint = Some(index);
		notes.push(format!("{}: its specified value is {reference}", wanted.reference));
	}

	let mut plan_units: Vec<Unit> = Vec::new();
	for c in &channels {
		if plan_units.iter().any(|u| u.request == c.unit) {
			continue;
		}
		let address = UnitAddress::from_request(c.unit).ok_or(Error::NoResponseRule(c.unit))?;
		let identity = units.iter().find(|u| u.request == c.unit).ok_or(Error::UnknownUnit(c.unit))?;
		let part_number = identity
			.part_number
			.clone()
			.filter(|p| !p.trim().is_empty())
			.ok_or(Error::NoPartNumber(c.unit))?;
		plan_units.push(Unit {
			request: address.request,
			response: address.response,
			part_number,
			odx_name: identity.odx_name.clone(),
		});
	}

	// The board holds `MAX_PAGES`, and an alarm raises a page by its plan index: a page
	// past the board's is one an alarm could name and the glass could never show.
	if input.pages.len() > MAX_PAGES {
		return Err(Error::TooManyPages(input.pages.len()));
	}
	let mut pages = Vec::new();
	for (i, page) in input.pages.iter().enumerate() {
		let n = i + 1;
		let index = |r: &Reference| {
			index_by_row(r, &channels, &index_of, &offered, answered, units).ok_or_else(|| Error::PageRefersToUnknown {
				page: n,
				reference: r.clone(),
			})
		};
		match page {
			PageInput::Values { title, cells } => {
				if cells.is_empty() || cells.len() > 4 {
					return Err(Error::Page(n, format!("a values page holds 1 to 4 cells, not {}", cells.len())));
				}
				let cells = cells.iter().map(index).collect::<Result<Vec<_>, _>>()?;
				pages.push(Page::Values { title: title.clone(), cells });
			}
			PageInput::Chart { cell, min, max } => {
				if min.partial_cmp(max) != Some(std::cmp::Ordering::Less) {
					return Err(Error::Page(n, format!("min {min} is not below max {max}")));
				}
				let channel = index(cell)?;
				// The device finds a chart's range by its channel, so a second
				// chart of the same channel could only ever draw the first's.
				if pages.iter().any(|p| matches!(p, Page::Chart { channel: c, .. } if *c == channel)) {
					return Err(Error::Page(n, format!("{cell} already has a chart page; one range per channel")));
				}
				pages.push(Page::Chart {
					channel,
					min: *min,
					max: *max,
				});
			}
		}
	}

	// Alarms last: a rule names channels and a page, and both are resolved by now.
	if input.alarms.len() > MAX_ALARMS {
		return Err(Error::TooManyAlarms(input.alarms.len()));
	}
	let mut alarms = Vec::new();
	for (i, wanted) in input.alarms.iter().enumerate() {
		let n = i + 1;
		let refuse = |why: String| Error::Alarm(n, why);
		if wanted.channels.is_empty() {
			return Err(refuse("watches no channels".to_string()));
		}
		let watched = wanted
			.channels
			.iter()
			.map(|r| index_by_row(r, &channels, &index_of, &offered, answered, units).ok_or_else(|| refuse(format!("{r} is not in the [[channel]] list"))))
			.collect::<Result<Vec<u16>, _>>()?;
		// The page is named by title, and only a values page has one: a takeover shows
		// cells, and a chart has one cell and no room to invert it.
		let titled: Vec<(usize, &Vec<u16>)> = pages
			.iter()
			.enumerate()
			.filter_map(|(p, page)| match page {
				Page::Values { title, cells } if *title == wanted.page => Some((p, cells)),
				_ => None,
			})
			.collect();
		let (page, cells) = match titled.as_slice() {
			[one] => *one,
			[] => {
				return Err(refuse(format!(
					"no values page is titled {:?} — a chart page has no title and cannot explain an alarm",
					wanted.page
				)));
			}
			many => {
				return Err(refuse(format!(
					"{} values pages are titled {:?} — give them different titles",
					many.len(),
					wanted.page
				)));
			}
		};
		if let Some((r, _)) = wanted.channels.iter().zip(&watched).find(|&(_, index)| !cells.contains(index)) {
			return Err(refuse(format!(
				"page {:?} does not show {r} — the page an alarm raises shows every channel it watches",
				wanted.page
			)));
		}
		let refs: Vec<String> = wanted.channels.iter().map(ToString::to_string).collect();
		// Compared as the board will compare them, in `f32`: two thresholds a hair apart in
		// the file can be one value there, and one value is no hysteresis.
		let rule = match wanted.rule {
			AlarmRuleInput::Threshold { direction, trip, release } => {
				let (board_trip, board_release) = (trip as f32, release as f32);
				match direction {
					Direction::Below if board_release <= board_trip => {
						return Err(refuse(format!(
							"release {board_release} is not above trip {board_trip} — a \"below\" alarm releases above where it trips"
						)));
					}
					Direction::Above if board_release >= board_trip => {
						return Err(refuse(format!(
							"release {board_release} is not below trip {board_trip} — an \"above\" alarm releases below where it trips"
						)));
					}
					_ => {}
				}
				notes.push(format!(
					"alarm #{n}: {} at or {} {trip}, released past {release} → page {:?}",
					refs.join(", "),
					if direction == Direction::Below { "below" } else { "above" },
					wanted.page
				));
				AlarmRule::Threshold { direction, trip, release }
			}
			AlarmRuleInput::Drift {
				percent,
				release_percent,
				hold_ms,
				min_setpoint,
			} => {
				if release_percent as f32 >= percent as f32 {
					return Err(refuse(format!(
						"release_percent {release_percent} is not under percent {percent} — a drift alarm releases under where it trips"
					)));
				}
				// Every watched channel needs the other half of its pair, or the rule has
				// nothing to measure against.
				let mut specified = Vec::new();
				for (r, index) in wanted.channels.iter().zip(&watched) {
					match channels[*index as usize].setpoint {
						Some(s) => specified.push(s),
						None => {
							return Err(refuse(format!(
								"{r} has no setpoint — a drift rule watches channels the plan pairs with a specified value"
							)));
						}
					}
				}
				notes.push(format!(
					"alarm #{n}: {} more than {percent}% from its specified value for {hold_ms} ms, released under {release_percent}%, ignored under {min_setpoint} → page {:?}",
					refs.join(", "),
					wanted.page
				));
				AlarmRule::Drift {
					specified,
					percent,
					release_percent,
					hold_ms,
					min_setpoint,
				}
			}
		};
		alarms.push(Alarm {
			channels: watched,
			page: page as u16,
			rule,
		});
	}

	Ok(Built {
		plan: Plan {
			vin: input.vin.clone(),
			language: language.code().to_string(),
			units: plan_units,
			channels,
			pages,
			alarms,
		},
		notes,
	})
}

/// The catalog's raw form, in the four numbers the firmware reads by.
fn bits_of(form: RawForm) -> (u32, u32, bool, bool) {
	match form {
		RawForm::U8First => (0, 8, false, true),
		RawForm::U8Second => (8, 8, false, true),
		RawForm::U16Be => (0, 16, false, true),
		RawForm::U16Le => (0, 16, false, false),
		RawForm::I16Be => (0, 16, true, true),
		RawForm::U24Be => (0, 24, false, true),
		RawForm::U32Be => (0, 32, false, true),
		RawForm::Int {
			byte_offset,
			byte_length,
			signed,
			big_endian,
		} => (u32::from(byte_offset) * 8, u32::from(byte_length) * 8, signed, big_endian),
		RawForm::Bits {
			bit_offset,
			bit_length,
			signed,
		} => (bit_offset, u32::from(bit_length), signed, true),
	}
}

/// Places after the point that a scaling's resolution earns: ×0.1 is one, ×0.001
/// is three, and a whole-number factor is none.
fn decimals_for(factor: f64) -> u8 {
	// The sign is direction, not resolution: ×−0.001 is as fine as ×0.001.
	let factor = factor.abs();
	if factor.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater) || factor >= 1.0 {
		return 0;
	}
	(-factor.log10()).ceil().clamp(0.0, 3.0) as u8
}

/// The alarm types the generated plan names, so nothing is imported unused.
fn alarm_imports(plan: &Plan) -> Vec<&'static str> {
	let mut imports = vec!["Alarm"];
	if plan.alarms.is_empty() {
		return imports;
	}
	imports.extend(["ChannelId", "PageId", "Rule"]);
	if plan.alarms.iter().any(|a| matches!(a.rule, AlarmRule::Threshold { .. })) {
		imports.push("Direction");
	}
	imports
}

/// The plan as Rust source: a `static PLAN` of `vag_dash_render::plan::Plan`.
pub fn to_rust(plan: &Plan) -> String {
	use std::fmt::Write as _;
	// `build` refuses a non-finite scaling, and `Debug` of a finite `f32`
	// always spells a point or an exponent, so this is a valid literal.
	let float = |v: f64| {
		debug_assert!(v.is_finite());
		format!("{:?}", v as f32)
	};
	let mut out = String::new();
	let _ = writeln!(out, "// Generated by `vagcan dev dash build` for VIN {}.", plan.vin);
	let _ = writeln!(out, "// Derived from VW's data and one owner's car: do not edit, do not commit.");
	let _ = writeln!(out, "use vag_dash_render::plan::{{Channel, Page, Plan, Unit}};");
	// Only what the rules actually name: the firmware lints the generated file with
	// `-D warnings`, so an unused import is a build that fails (review, 2026-09-15). A plan
	// whose rules are all drift never names `Direction`; one with no rules names nothing but
	// the type itself.
	let mut imports = alarm_imports(plan);
	imports.sort_unstable();
	let _ = match imports.as_slice() {
		[one] => writeln!(out, "use vag_dash_render::alarm::{one};"),
		many => writeln!(out, "use vag_dash_render::alarm::{{{}}};", many.join(", ")),
	};
	let _ = writeln!(out);
	let _ = writeln!(
		out,
		"pub static PLAN: Plan = Plan {{ vin: {:?}, language: {:?}, units: &UNITS, channels: &CHANNELS, pages: &PAGES, alarms: &ALARMS }};",
		plan.vin, plan.language
	);
	let _ = writeln!(out);
	let _ = writeln!(out, "static UNITS: [Unit; {}] = [", plan.units.len());
	for u in &plan.units {
		let _ = writeln!(
			out,
			"\tUnit {{ request: 0x{:03X}, response: 0x{:03X}, part_number: {:?} }},",
			u.request, u.response, u.part_number
		);
	}
	let _ = writeln!(out, "];");
	let _ = writeln!(out);
	let _ = writeln!(out, "static CHANNELS: [Channel; {}] = [", plan.channels.len());
	for c in &plan.channels {
		let _ = writeln!(
			out,
			"\tChannel {{ unit: 0x{:03X}, did: 0x{:04X}, bit_offset: {}, bit_length: {}, signed: {}, big_endian: {}, factor: {}, offset: {}, decimals: {}, unit_text: {:?}, label: {:?}, proven: {}, hz: {}, setpoint: {} }},",
			c.unit,
			c.did,
			c.bit_offset,
			c.bit_length,
			c.signed,
			c.big_endian,
			float(c.factor),
			float(c.offset),
			c.decimals,
			c.unit_text,
			c.label,
			c.proven,
			float(c.hz),
			match c.setpoint {
				Some(index) => format!("Some({index})"),
				None => "None".to_string(),
			}
		);
	}
	let _ = writeln!(out, "];");
	let _ = writeln!(out);
	for (i, p) in plan.pages.iter().enumerate() {
		if let Page::Values { cells, .. } = p {
			let list: Vec<String> = cells.iter().map(|c| c.to_string()).collect();
			let _ = writeln!(out, "static CELLS_{i}: [u16; {}] = [{}];", cells.len(), list.join(", "));
		}
	}
	let _ = writeln!(out, "static PAGES: [Page; {}] = [", plan.pages.len());
	for (i, p) in plan.pages.iter().enumerate() {
		match p {
			Page::Chart { channel, min, max } => {
				let _ = writeln!(out, "\tPage::Chart {{ channel: {channel}, min: {}, max: {} }},", float(*min), float(*max));
			}
			Page::Values { title, .. } => {
				let _ = writeln!(out, "\tPage::Values {{ title: {title:?}, cells: &CELLS_{i} }},");
			}
		}
	}
	let _ = writeln!(out, "];");
	let _ = writeln!(out);
	for (i, a) in plan.alarms.iter().enumerate() {
		let ids = |channels: &[u16]| {
			let list: Vec<String> = channels.iter().map(|c| format!("ChannelId({c})")).collect();
			format!("[{}]", list.join(", "))
		};
		let _ = writeln!(
			out,
			"static ALARM_CHANNELS_{i}: [ChannelId; {}] = {};",
			a.channels.len(),
			ids(&a.channels)
		);
		if let AlarmRule::Drift { specified, .. } = &a.rule {
			let _ = writeln!(out, "static ALARM_SPECIFIED_{i}: [ChannelId; {}] = {};", specified.len(), ids(specified));
		}
	}
	let _ = writeln!(out, "static ALARMS: [Alarm<'static>; {}] = [", plan.alarms.len());
	for (i, a) in plan.alarms.iter().enumerate() {
		let rule = match &a.rule {
			AlarmRule::Threshold { direction, trip, release } => format!(
				"Rule::Threshold {{ trip: {}, release: {}, direction: Direction::{} }}",
				float(*trip),
				float(*release),
				match direction {
					Direction::Below => "Below",
					Direction::Above => "Above",
				}
			),
			AlarmRule::Drift {
				percent,
				release_percent,
				hold_ms,
				min_setpoint,
				..
			} => format!(
				"Rule::Drift {{ specified: &ALARM_SPECIFIED_{i}, percent: {}, release_percent: {}, hold_ms: {hold_ms}, min_setpoint: {} }}",
				float(*percent),
				float(*release_percent),
				float(*min_setpoint)
			),
		};
		let _ = writeln!(
			out,
			"\tAlarm {{ channels: &ALARM_CHANNELS_{i}, page: PageId({}), rule: {rule} }},",
			a.page
		);
	}
	let _ = writeln!(out, "];");
	out
}

/// Where a car's plan lives and what was written there.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
	pub built: Built,
	pub dir: PathBuf,
	pub json: PathBuf,
	pub rust: PathBuf,
	/// Everything the build read, for a build script to watch: the input, the
	/// survey, the project's cache and proven rows, the name table, and the
	/// settings and glossary that decide the labels' language and wording.
	pub inputs: Vec<PathBuf>,
}

/// A car's build input resolved into a plan, and what the resolution read — for a caller
/// that needs more of it than the plan. Nothing is written.
#[derive(Debug)]
pub struct Resolved {
	pub built: Built,
	/// `~/.vagcan/dash/<VIN>/`, where the outputs go.
	pub dir: PathBuf,
	/// Everything the build read — see [`Written::inputs`].
	pub inputs: Vec<PathBuf>,
	/// The survey the build read, as text.
	pub survey: String,
	/// What each unit said about itself in that survey: what the catalogs were looked up by.
	pub units: Vec<UnitIdentity>,
	pub store: CatalogStore,
	pub extracted: Extracted,
}

/// [`build_for_car`] short of writing anything: read the input, the survey and the
/// project, and build. For a command that reads the plan and keeps nothing
/// (`vagcan dev recording dash`).
///
/// `input` defaults to `dash.toml` under `~/.vagcan/dash/<VIN>/`.
pub fn resolve_for_car(vin: &str, input: Option<&Path>) -> anyhow::Result<Resolved> {
	use anyhow::Context as _;
	let dir = crate::datadir::dash_dir(vin)?;
	let input_path = input.map(Path::to_path_buf).unwrap_or_else(|| dir.join("dash.toml"));
	let text = std::fs::read_to_string(&input_path).with_context(|| format!("no build input at {}", input_path.display()))?;
	let parsed = parse_input(&text)?;
	if !parsed.vin.eq_ignore_ascii_case(vin.trim()) {
		anyhow::bail!("{} is for VIN {} but the build asked for {vin}", input_path.display(), parsed.vin);
	}
	let survey_path = match &parsed.survey {
		Some(p) => p.clone(),
		None => crate::datadir::survey_cache(vin)?,
	};
	let survey = std::fs::read_to_string(&survey_path).with_context(|| {
		format!(
			"no survey at {} — run `vagcan dev survey` on the car, or name one with `survey =`",
			survey_path.display()
		)
	})?;
	let units = poll::identities_from_survey(&survey);
	let answered = poll::answered_from_survey(&survey);
	let project = crate::project::current()?;
	let store = CatalogStore::open(project.measurements_dir());
	let extracted = crate::extracted::open(&project);
	let language = crate::config::language(&crate::config::load());
	let built = build(&parsed, &store, &extracted, &units, Some(&answered), language)?;
	let inputs = vec![
		input_path,
		survey_path,
		project.cache(),
		project.measurements_dir(),
		project.names(),
		crate::config::path()?,
		crate::glossary::path()?,
	];
	Ok(Resolved {
		built,
		dir,
		inputs,
		survey,
		units,
		store,
		extracted,
	})
}

/// The whole command: read the input, the survey and the project, build,
/// write `plan.json` and `plan.rs` under `~/.vagcan/dash/<VIN>/`.
///
/// `input` defaults to `dash.toml` in that directory. The firmware's build
/// script calls this too, so `cargo build` of the firmware *is* the plan build.
pub fn build_for_car(vin: &str, input: Option<&Path>) -> anyhow::Result<Written> {
	use anyhow::Context as _;
	let Resolved { built, dir, inputs, .. } = resolve_for_car(vin, input)?;
	std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
	let json = dir.join("plan.json");
	let rust = dir.join("plan.rs");
	std::fs::write(&json, built.plan.to_json()).with_context(|| format!("writing {}", json.display()))?;
	std::fs::write(&rust, to_rust(&built.plan)).with_context(|| format!("writing {}", rust.display()))?;
	Ok(Written {
		built,
		dir,
		json,
		rust,
		inputs,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::path::Path;
	use vag_data_labels::catalog::{MeasurementCatalog, MeasurementDef};
	use vag_data_labels::measure::LinearScale;
	use vag_data_labels::odis::Reading;

	const ENGINE: u16 = 0x7E0;
	const GEARBOX: u16 = 0x7E1;

	/// **Every byte synthetic.** A cache in a temp directory, a variant name
	/// nobody's car reports, and no read of `~/.vagcan` anywhere.
	fn extracted_with(dir: &Path, variants: &[(&str, Vec<Reading>)], names: &[(&str, &str)]) -> Extracted {
		let cache = dir.join("cache.sqlite");
		for (name, readings) in variants {
			vag_data_db::put_readings(&cache, "/nowhere/TEST", name, readings).expect("the fixture writes");
		}
		Extracted::synthetic(cache, names.iter().map(|(id, text)| (id.to_string(), text.to_string())).collect())
	}

	#[allow(clippy::too_many_arguments)]
	fn reading(
		did: u16,
		name: &str,
		text_id: &str,
		bit_offset: u32,
		bit_length: u32,
		signed: bool,
		big_endian: bool,
		factor: f64,
		offset: f64,
	) -> Reading {
		Reading {
			did,
			name: name.to_string(),
			unit: Some("°C".to_string()),
			bit_offset,
			bit_length,
			signed,
			big_endian,
			scaling: Scaling::Linear(LinearScale { factor, offset }),
			text_id: (!text_id.is_empty()).then(|| text_id.to_string()),
		}
	}

	fn identity(request: u16, part: &str, odx: &str) -> UnitIdentity {
		UnitIdentity {
			request,
			part_number: Some(part.to_string()),
			odx_name: Some(odx.to_string()),
			odx_version: Some("001004".to_string()),
			component: None,
		}
	}

	fn input(channels: &[&str], pages: &str) -> Input {
		let mut text = String::from("vin = \"TESTVIN0000000001\"\n");
		for c in channels {
			text.push_str(&format!("[[channel]]\nref = \"{c}\"\n"));
		}
		text.push_str(pages);
		parse_input(&text).expect("the fixture parses")
	}

	fn values_page(cells: &[&str]) -> String {
		values_page_titled("T", cells)
	}

	fn values_page_titled(title: &str, cells: &[&str]) -> String {
		let list: Vec<String> = cells.iter().map(|c| format!("\"{c}\"")).collect();
		format!("[[page]]\nkind = \"values\"\ntitle = \"{title}\"\ncells = [{}]\n", list.join(", "))
	}

	fn alarm(channels: &[&str], page: &str, direction: &str, trip: f64, release: f64) -> String {
		let list: Vec<String> = channels.iter().map(|c| format!("\"{c}\"")).collect();
		format!(
			"[[alarm]]\nchannels = [{}]\npage = \"{page}\"\ndirection = \"{direction}\"\ntrip = {trip:?}\nrelease = {release:?}\n",
			list.join(", ")
		)
	}

	/// Three neutral channels on one unit; values pages `A` (channels 0 and 1) and `B`
	/// (channel 2), a chart of channel 0; then whatever the test appends.
	fn build_with_alarms(extra: &str) -> Result<Built, Error> {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![
					reading(0x1001, "One", "IDE00001", 0, 8, false, true, 1.0, 0.0),
					reading(0x1002, "Two", "IDE00002", 0, 8, false, true, 1.0, 0.0),
					reading(0x1003, "Three", "IDE00003", 0, 8, false, true, 1.0, 0.0),
				],
			)],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let text = format!(
			"vin = \"TESTVIN0000000001\"\n[[channel]]\nref = \"01:IDE00001\"\n[[channel]]\nref = \"01:IDE00002\"\n[[channel]]\nref = \"01:IDE00003\"\n{}{}[[page]]\nkind = \"chart\"\ncell = \"01:IDE00001\"\nmin = 0\nmax = 10\n{extra}",
			values_page_titled("A", &["01:IDE00001", "01:IDE00002"]),
			values_page_titled("B", &["01:IDE00003"]),
		);
		build(
			&parse_input(&text)?,
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
	}

	/// A unit with a channel and the specified value behind it, and whatever the test appends.
	fn build_with_setpoint(extra: &str) -> Result<Built, Error> {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![
					reading(0x202A, "Boost pressure", "IDE00191", 0, 16, false, true, 0.001, 0.0),
					reading(0x2029, "Boost pressure commanded value", "IDE00190", 0, 16, false, true, 0.001, 0.0),
					Reading {
						unit: Some("bar".to_string()),
						..reading(0x2030, "In another unit", "IDE00999", 0, 16, false, true, 0.001, 0.0)
					},
				],
			)],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let text = format!("vin = \"TESTVIN0000000001\"\n{extra}{}", values_page_titled("A", &["01:IDE00191"]),);
		build(
			&parse_input(&text)?,
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
	}

	#[test]
	fn a_setpoint_becomes_a_channel_of_its_own_and_its_index_is_kept() {
		let built = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\nhz = 10\n").unwrap();
		assert_eq!(built.plan.channels.len(), 2, "the specified value was added");
		let actual = &built.plan.channels[0];
		let specified = &built.plan.channels[1];
		assert_eq!(actual.setpoint, Some(1));
		assert_eq!(specified.did, 0x2029);
		assert_eq!(specified.setpoint, None, "the pair does not nest");
		assert_eq!(specified.hz, actual.hz, "read as often as the channel it explains");
		assert_eq!(specified.unit, actual.unit, "one unit, so one request");
		assert!(to_rust(&built.plan).contains("setpoint: Some(1)"), "the firmware's plan carries it");
	}

	#[test]
	fn a_setpoint_the_input_already_declares_is_not_added_twice() {
		let built = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\n[[channel]]\nref = \"01:IDE00190\"\n").unwrap();
		assert_eq!(built.plan.channels.len(), 2);
		assert_eq!(built.plan.channels[0].setpoint, Some(1));
	}

	#[test]
	fn a_setpoint_on_another_unit_is_refused() {
		let why = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"02:IDE00190\"\n").unwrap_err();
		assert!(matches!(&why, Error::Setpoint { why, .. } if why.contains("another unit")), "{why}");
	}

	#[test]
	fn a_setpoint_in_another_unit_of_measure_is_refused() {
		let why = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00999\"\n").unwrap_err();
		assert!(matches!(&why, Error::Setpoint { why, .. } if why.contains("reads in")), "{why}");
	}

	#[test]
	fn a_setpoint_that_is_the_channel_itself_is_refused() {
		let why = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00191\"\n").unwrap_err();
		assert!(matches!(&why, Error::Setpoint { why, .. } if why.contains("the channel itself")), "{why}");
	}

	#[test]
	fn a_setpoint_with_a_setpoint_of_its_own_is_refused() {
		let why = build_with_setpoint(
			"[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\n[[channel]]\nref = \"01:IDE00190\"\nsetpoint = \"01:IDE00999\"\n",
		)
		.unwrap_err();
		assert!(
			matches!(&why, Error::Setpoint { why, .. } if why.contains("setpoint of its own")),
			"{why}"
		);
	}

	/// The drift rule the tests build on: page `A` shows the drifting channel.
	fn drift_alarm(percent: f64, release_percent: f64) -> String {
		format!(
			"[[alarm]]\nkind = \"drift\"\nchannels = [\"01:IDE00191\"]\npage = \"A\"\npercent = {percent:?}\nrelease_percent = {release_percent:?}\nhold_ms = 1000\nmin_setpoint = 0.5\n"
		)
	}

	#[test]
	fn a_drift_rule_reaches_both_outputs_with_the_pair_it_watches() {
		let built = build_with_setpoint(&format!(
			"[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\n{}",
			drift_alarm(10.0, 6.0)
		))
		.unwrap();
		assert_eq!(
			built.plan.alarms,
			vec![Alarm {
				channels: vec![0],
				page: 0,
				rule: AlarmRule::Drift {
					specified: vec![1],
					percent: 10.0,
					release_percent: 6.0,
					hold_ms: 1000,
					min_setpoint: 0.5,
				},
			}]
		);
		let json = built.plan.to_json();
		assert_eq!(Plan::from_json(&json).unwrap(), built.plan);
		assert!(json.contains("\"kind\": \"drift\""), "{json}");
		let rust = to_rust(&built.plan);
		assert!(rust.contains("static ALARM_SPECIFIED_0: [ChannelId; 1] = [ChannelId(1)];"), "{rust}");
		assert!(
			rust.contains("rule: Rule::Drift { specified: &ALARM_SPECIFIED_0, percent: 10.0, release_percent: 6.0, hold_ms: 1000, min_setpoint: 0.5 }"),
			"{rust}"
		);
	}

	#[test]
	fn a_drift_rule_over_a_channel_with_no_specified_value_is_refused() {
		let why = build_with_setpoint(&format!("[[channel]]\nref = \"01:IDE00191\"\n{}", drift_alarm(10.0, 6.0))).unwrap_err();
		assert!(matches!(&why, Error::Alarm(1, why) if why.contains("has no setpoint")), "{why}");
	}

	#[test]
	fn a_drift_rule_that_does_not_release_under_its_percent_is_refused() {
		let why = build_with_setpoint(&format!(
			"[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\n{}",
			drift_alarm(10.0, 10.0)
		))
		.unwrap_err();
		assert!(matches!(&why, Error::Alarm(1, why) if why.contains("releases under")), "{why}");
	}

	#[test]
	fn an_alarm_of_an_unknown_kind_is_refused() {
		let why = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\n[[alarm]]\nkind = \"wobble\"\nchannels = [\"01:IDE00191\"]\npage = \"A\"\n")
			.unwrap_err();
		assert!(
			matches!(&why, Error::Parse(why) if why.contains("is not \"threshold\" or \"drift\"")),
			"{why}"
		);
	}

	#[test]
	fn a_setpoint_spelled_as_an_identifier_is_the_same_row_as_its_text_id() {
		// `01:202A` and `01:IDE00191` are one row written two ways: pairing a channel with
		// itself that way drew `+0.00` for ever before the build compared resolved rows.
		let why = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:202A\"\n").unwrap_err();
		assert!(matches!(&why, Error::Setpoint { why, .. } if why.contains("the channel itself")), "{why}");
	}

	#[test]
	fn a_setpoint_and_its_channel_spelled_differently_are_one_channel() {
		let built = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:2029\"\n[[channel]]\nref = \"01:IDE00190\"\n").unwrap();
		assert_eq!(built.plan.channels.len(), 2, "the specified value is not added twice");
		assert_eq!(built.plan.channels[0].setpoint, Some(1));
	}

	#[test]
	fn a_setpoint_read_at_another_rate_than_its_channel_is_refused() {
		// Two rates are two moments, and the difference would be between numbers up to a
		// period apart.
		let why = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\nhz = 10\n[[channel]]\nref = \"01:IDE00190\"\n")
			.unwrap_err();
		assert!(matches!(&why, Error::Setpoint { why, .. } if why.contains("share a rate")), "{why}");
	}

	#[test]
	fn a_plan_whose_rules_are_all_drift_does_not_import_direction() {
		// The firmware lints the generated plan with `-D warnings`, so an unused import is a
		// build that fails.
		let built = build_with_setpoint(&format!(
			"[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\n{}",
			drift_alarm(10.0, 6.0)
		))
		.unwrap();
		let rust = to_rust(&built.plan);
		assert!(rust.contains("use vag_dash_render::alarm::{Alarm, ChannelId, PageId, Rule};"), "{rust}");
		assert!(!rust.contains("Direction"), "{rust}");
	}

	#[test]
	fn two_channels_sharing_an_undeclared_setpoint_must_share_its_rate() {
		// The second pairing would read the same specified value at another rate, so the
		// difference would be between numbers up to a period apart.
		let why = build_with_setpoint(
			"[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\nhz = 10\n[[channel]]\nref = \"01:IDE00999\"\nsetpoint = \"01:IDE00190\"\n",
		)
		.unwrap_err();
		assert!(matches!(&why, Error::Setpoint { why, .. } if why.contains("share a rate")), "{why}");
	}

	#[test]
	fn one_row_declared_under_both_spellings_is_refused_naming_both() {
		let why = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\n[[channel]]\nref = \"01:202A\"\n").unwrap_err();
		assert!(
			matches!(&why, Error::SameRow { first, second } if first.to_string() == "01:IDE00191" && second.to_string() == "01:202A"),
			"{why}"
		);
		let said = why.to_string();
		assert!(said.contains("01:IDE00191") && said.contains("01:202A"), "{said}");
	}

	#[test]
	fn a_setpoint_spelled_differently_from_its_channel_must_still_share_its_rate() {
		// Spelled `01:2029`, declared as `01:IDE00190` at 2 Hz, paired with a 10 Hz channel: the
		// rate compared is the declared channel's, not the fresh resolution's.
		let why =
			build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:2029\"\nhz = 10\n[[channel]]\nref = \"01:IDE00190\"\n").unwrap_err();
		assert!(matches!(&why, Error::Setpoint { why, .. } if why.contains("share a rate")), "{why}");
	}

	#[test]
	fn a_page_and_an_alarm_may_spell_a_channel_as_its_other_spelling() {
		let built = build_with_setpoint(
			"[[channel]]\nref = \"01:IDE00191\"\n[[alarm]]\nchannels = [\"01:202A\"]\npage = \"B\"\ndirection = \"above\"\ntrip = 2.5\nrelease = 2.3\n[[page]]\nkind = \"values\"\ntitle = \"B\"\ncells = [\"01:202A\"]\n",
		)
		.unwrap();
		assert_eq!(built.plan.channels.len(), 1, "one row, one channel");
		assert!(
			built
				.plan
				.pages
				.iter()
				.any(|p| matches!(p, Page::Values { title, cells } if title == "B" && cells == &vec![0]))
		);
		assert_eq!(built.plan.alarms[0].channels, vec![0]);
	}

	#[test]
	fn a_setpoint_that_is_already_a_channel_leaves_one_line_in_the_log() {
		let built = build_with_setpoint("[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:2029\"\n[[channel]]\nref = \"01:IDE00190\"\n").unwrap();
		let resolutions = built.notes.iter().filter(|n| n.contains("2029@0/16")).count();
		assert_eq!(resolutions, 1, "the row is resolved once and logged once: {:?}", built.notes);
	}

	#[test]
	fn references_parse_both_spellings() {
		assert_eq!(
			Reference::parse("01:IDE00025").unwrap(),
			Reference::TextId {
				request: ENGINE,
				text_id: "IDE00025".to_string()
			}
		);
		assert_eq!(
			Reference::parse("7E1:380A").unwrap(),
			Reference::Field {
				request: GEARBOX,
				did: 0x380A,
				bit_offset: 0
			}
		);
		assert_eq!(
			Reference::parse("02:3816@3").unwrap(),
			Reference::Field {
				request: GEARBOX,
				did: 0x3816,
				bit_offset: 3
			}
		);
		assert_eq!(Reference::parse("02:3816@3").unwrap().to_string(), "02:3816@3");
		assert!(Reference::parse("IDE00025").is_err(), "no unit");
		assert!(Reference::parse("01:IDE00025@3").is_err(), "a bit offset on a text id");
	}

	#[test]
	fn the_plan_in_memory_is_the_one_the_generated_static_describes() {
		use vag_dash_render::alarm::{ChannelId, Direction as DeviceDirection, PageId, Rule};
		use vag_dash_render::plan::Page as DevicePage;
		let built = build_with_alarms(&alarm(&["01:IDE00002"], "A", "below", -2.0, -1.5)).unwrap();
		let device = built.plan.to_device();
		let rust = to_rust(&built.plan);

		assert_eq!((device.vin, device.language), ("TESTVIN0000000001", "en"));
		assert_eq!(device.units.len(), 1);
		assert_eq!((device.units[0].request, device.units[0].part_number), (ENGINE, "PART1"));
		assert_eq!(device.channels.len(), 3);
		for (c, d) in built.plan.channels.iter().zip(device.channels) {
			assert_eq!((d.unit, d.did, d.bit_offset, d.bit_length), (c.unit, c.did, c.bit_offset, c.bit_length));
			assert_eq!((d.factor, d.offset, d.hz), (c.factor as f32, c.offset as f32, c.hz as f32));
			assert_eq!((d.label, d.unit_text, d.decimals), (c.label.as_str(), c.unit_text.as_str(), c.decimals));
			// The source says the same numbers the memory holds.
			assert!(rust.contains(&format!("did: 0x{:04X}, bit_offset: {}", d.did, d.bit_offset)), "{rust}");
		}
		assert_eq!(
			device.pages,
			[
				DevicePage::Values { title: "A", cells: &[0, 1] },
				DevicePage::Values { title: "B", cells: &[2] },
				DevicePage::Chart {
					channel: 0,
					min: 0.0,
					max: 10.0
				},
			]
		);
		assert_eq!(device.alarms.len(), 1);
		assert_eq!(device.alarms[0].channels, [ChannelId(1)]);
		assert_eq!(device.alarms[0].page, PageId(0));
		assert_eq!(
			device.alarms[0].rule,
			Rule::Threshold {
				trip: -2.0,
				release: -1.5,
				direction: DeviceDirection::Below
			}
		);
		assert!(device.watched(1) && !device.watched(0), "the board's own question answers from it");
	}

	#[test]
	fn a_drift_rule_in_memory_pairs_the_same_specified_values_as_the_source() {
		use vag_dash_render::alarm::{ChannelId, Rule};
		let built = build_with_setpoint(&format!(
			"[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\n{}",
			drift_alarm(10.0, 6.0)
		))
		.unwrap();
		let device = built.plan.to_device();
		let specified = built.plan.channels.iter().position(|c| c.did == 0x2029).unwrap() as u16;
		let actual = built.plan.channels.iter().position(|c| c.did == 0x202A).unwrap() as u16;
		assert_eq!(device.channels[usize::from(actual)].setpoint, Some(specified));
		assert_eq!(device.alarms[0].channels, [ChannelId(actual)]);
		assert_eq!(
			device.alarms[0].rule,
			Rule::Drift {
				specified: &[ChannelId(specified)],
				percent: 10.0,
				release_percent: 6.0,
				hold_ms: 1000,
				min_setpoint: 0.5
			}
		);
	}

	#[test]
	fn a_plan_round_trips_through_json() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x2029, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[("IDE00191", "Boost pressure")],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let built = build(
			&input(
				&["01:IDE00191"],
				&format!(
					"{}[[page]]\nkind = \"chart\"\ncell = \"01:IDE00191\"\nmin = 0.9\nmax = 2.1\n",
					values_page(&["01:IDE00191"])
				),
			),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap();
		let json = built.plan.to_json();
		let back = Plan::from_json(&json).unwrap();
		assert_eq!(back, built.plan);
		assert_eq!(back.to_json(), json);
		assert_eq!(back.channels[0].label, "Boost pressure", "named through the text id");
		assert_eq!(back.channels[0].decimals, 3, "×0.001 earns three places");
		assert_eq!(
			back.units,
			vec![Unit {
				request: ENGINE,
				response: 0x7E8,
				part_number: "PART1".to_string(),
				odx_name: Some("EV_Test".to_string())
			}]
		);
	}

	/// The column the byte-order flag exists for. Asserted on the decoded
	/// value, through the firmware's own decoder, not on the flag.
	#[test]
	fn a_little_endian_row_survives_the_build() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![reading(0x380A, "Input shaft speed", "IDE00001", 0, 16, false, false, 1.0, 0.0)],
			)],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let built = build(
			&input(&["02:IDE00001"], &values_page(&["02:IDE00001"])),
			&store,
			&extracted,
			&[identity(GEARBOX, "PART2", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap();
		let c = &built.plan.channels[0];
		let device = vag_dash_render::plan::Channel {
			unit: c.unit,
			did: c.did,
			bit_offset: c.bit_offset,
			bit_length: c.bit_length,
			signed: c.signed,
			big_endian: c.big_endian,
			factor: c.factor as f32,
			offset: c.offset as f32,
			decimals: c.decimals,
			unit_text: "",
			label: "",
			proven: c.proven,
			hz: c.hz as f32,
			setpoint: c.setpoint,
		};
		assert_eq!(device.decode(&[0xB2, 0x02]), Some(690.0), "690 /min, not 45570");
		assert!(to_rust(&built.plan).contains("big_endian: false"));
	}

	#[test]
	fn a_sub_byte_field_keeps_its_offset_in_bits_and_two_fields_are_two_channels() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![
					reading(0x3816, "Gear engaged", "IDE00010", 0, 4, false, true, 1.0, 0.0),
					reading(0x3816, "Clutch closed", "IDE00011", 4, 1, false, true, 1.0, 0.0),
				],
			)],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let built = build(
			&input(&["02:3816", "02:3816@4"], &values_page(&["02:3816", "02:3816@4"])),
			&store,
			&extracted,
			&[identity(GEARBOX, "PART2", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap();
		assert_eq!(built.plan.channels.len(), 2);
		assert_eq!((built.plan.channels[0].bit_offset, built.plan.channels[0].bit_length), (0, 4));
		assert_eq!((built.plan.channels[1].bit_offset, built.plan.channels[1].bit_length), (4, 1));
		assert_eq!(
			built.plan.pages,
			vec![Page::Values {
				title: "T".to_string(),
				cells: vec![0, 1]
			}]
		);
	}

	#[test]
	fn a_proven_measurement_beats_a_declared_one() {
		let here = tempfile::tempdir().unwrap();
		let proven = here.path().join("proven");
		std::fs::create_dir_all(&proven).unwrap();
		let catalog = MeasurementCatalog::new(vec![MeasurementDef {
			name: "Boost (driven)".into(),
			unit: "bar".into(),
			address: ReadId::Uds(0x202A),
			raw_form: RawForm::U16Be,
			scaling: Scaling::Linear(LinearScale { factor: 0.002, offset: 0.0 }),
		}]);
		std::fs::write(proven.join("PART1.json"), catalog.to_json().unwrap()).unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x202A, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let store = CatalogStore::open(&proven);
		let built = build(
			&input(&["01:202A"], &values_page(&["01:202A"])),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap();
		let c = &built.plan.channels[0];
		assert!(c.proven);
		assert_eq!(c.factor, 0.002);
		assert_eq!(c.label, "Boost (driven)");
		assert!(built.notes[0].contains("proven"), "{}", built.notes[0]);
	}

	/// The reference car puts every OBD-II parameter's text id on two fields:
	/// the value, and its "supported" bit in the `F400` mask. The bit is an
	/// enum with no unit; a cell wants the quantity, and the build says so
	/// without being told.
	#[test]
	fn a_text_id_shared_with_a_flag_picks_the_quantity() {
		let here = tempfile::tempdir().unwrap();
		let mut flag = reading(0xF400, "Engine Coolant Temperature", "IDE00025", 3, 1, false, true, 1.0, 0.0);
		flag.unit = None;
		flag.scaling = Scaling::Enum {
			levels: vec![(0, "no".to_string()), (1, "yes".to_string())],
		};
		let value = reading(0xF405, "Engine Coolant Temperature", "IDE00025", 0, 8, false, true, 1.0, -40.0);
		let extracted = extracted_with(here.path(), &[("EV_Test_001", vec![flag, value])], &[]);
		let store = CatalogStore::open(here.path().join("proven"));
		let built = build(
			&input(&["01:IDE00025"], &values_page(&["01:IDE00025"])),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap();
		assert_eq!(built.plan.channels.len(), 1);
		assert_eq!(
			(
				built.plan.channels[0].did,
				built.plan.channels[0].bit_length,
				built.plan.channels[0].offset
			),
			(0xF405, 8, -40.0)
		);
	}

	#[test]
	fn a_channel_the_variant_lacks_fails_and_the_message_names_it() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x2029, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let err = build(
			&input(&["01:IDE99999"], &values_page(&["01:IDE99999"])),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap_err();
		assert_eq!(err, Error::Undeclared(Reference::parse("01:IDE99999").unwrap()));
		assert!(err.to_string().contains("01:IDE99999"), "{err}");

		let err = build(
			&input(&["02:IDE00191"], &values_page(&["02:IDE00191"])),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap_err();
		assert_eq!(err, Error::UnknownUnit(GEARBOX), "a unit the survey never saw");
	}

	#[test]
	fn a_scaling_the_device_cannot_apply_is_refused() {
		let here = tempfile::tempdir().unwrap();
		let mut gear = reading(0x3816, "Gear", "IDE00010", 0, 8, false, true, 1.0, 0.0);
		gear.scaling = Scaling::Enum {
			levels: vec![(1, "N".to_string()), (2, "1".to_string())],
		};
		let extracted = extracted_with(here.path(), &[("EV_Test_001", vec![gear])], &[]);
		let store = CatalogStore::open(here.path().join("proven"));
		let err = build(
			&input(&["02:IDE00010"], &values_page(&["02:IDE00010"])),
			&store,
			&extracted,
			&[identity(GEARBOX, "PART2", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap_err();
		assert!(matches!(err, Error::NotLinear(_, _)), "{err}");
		assert!(err.to_string().contains("02:IDE00010"), "{err}");
	}

	#[test]
	fn pages_are_checked_against_the_channel_list() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x2029, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let units = [identity(ENGINE, "PART1", "EV_Test")];
		let err = build(
			&input(&["01:IDE00191"], &values_page(&["01:IDE00192"])),
			&store,
			&extracted,
			&units,
			None,
			Language::En,
		)
		.unwrap_err();
		assert!(matches!(err, Error::PageRefersToUnknown { page: 1, .. }), "{err}");
		let err = build(
			&input(&["01:IDE00191"], "[[page]]\nkind = \"chart\"\ncell = \"01:IDE00191\"\nmin = 2\nmax = 1\n"),
			&store,
			&extracted,
			&units,
			None,
			Language::En,
		)
		.unwrap_err();
		assert!(matches!(err, Error::Page(1, _)), "{err}");
		assert!(parse_input("vin = \"X\"\n").is_err(), "no channels");
	}

	#[test]
	fn the_rust_form_is_the_static_the_firmware_links() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![reading(0xF405, "Engine Coolant Temperature", "IDE00025", 0, 8, false, true, 1.0, -40.0)],
			)],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let text = format!(
			"vin = \"TESTVIN0000000001\"\nlanguage = \"ru\"\n[[channel]]\nref = \"01:IDE00025\"\nlabel = \"ОЖ\"\n{}",
			values_page(&["01:IDE00025"])
		);
		let built = build(
			&parse_input(&text).unwrap(),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap();
		let rust = to_rust(&built.plan);
		assert!(rust.contains("pub static PLAN: Plan"), "{rust}");
		assert!(rust.contains("language: \"ru\""), "{rust}");
		assert!(rust.contains("did: 0xF405, bit_offset: 0, bit_length: 8, signed: false, big_endian: true, factor: 1.0, offset: -40.0, decimals: 0, unit_text: \"°C\", label: \"ОЖ\", proven: false, hz: 2.0"), "{rust}");
		assert!(rust.contains("static CELLS_0: [u16; 1] = [0];"), "{rust}");
		assert!(rust.contains("Page::Values { title: \"T\", cells: &CELLS_0 }"), "{rust}");
		assert!(rust.contains("do not commit"), "{rust}");
	}

	/// A drive that proves a row must not make it unaddressable: the declared
	/// row's text id rides along, the scaling is the proven one.
	#[test]
	fn a_proven_row_keeps_the_text_id_the_declared_one_had() {
		let here = tempfile::tempdir().unwrap();
		let proven = here.path().join("proven");
		std::fs::create_dir_all(&proven).unwrap();
		let catalog = MeasurementCatalog::new(vec![MeasurementDef {
			name: "Boost (driven)".into(),
			unit: "bar".into(),
			address: ReadId::Uds(0x202A),
			raw_form: RawForm::U16Be,
			scaling: Scaling::Linear(LinearScale { factor: 0.002, offset: 0.0 }),
		}]);
		std::fs::write(proven.join("PART1.json"), catalog.to_json().unwrap()).unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x202A, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let store = CatalogStore::open(&proven);
		let built = build(
			&input(&["01:IDE00191"], &values_page(&["01:IDE00191"])),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap();
		assert!(built.plan.channels[0].proven);
		assert_eq!(built.plan.channels[0].factor, 0.002);
	}

	/// The survey is the car's own word on what answers. Silence where it
	/// asked fails the build; a standard OBD-II row the survey never put to
	/// the car builds, and the log says the survey has no record of it.
	#[test]
	fn the_survey_decides_what_the_car_answers() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x2029, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let units = [identity(ENGINE, "PART1", "EV_Test")];
		let mut answered = poll::Answered::default();
		answered.units.insert(ENGINE);
		answered.asked.insert(ENGINE, vec![0x2000..=0x20FF, 0xF400..=0xF4FF]);
		answered.dids.insert((ENGINE, 0xF405));

		let err = build(
			&input(&["01:IDE00191"], &values_page(&["01:IDE00191"])),
			&store,
			&extracted,
			&units,
			Some(&answered),
			Language::En,
		)
		.unwrap_err();
		assert_eq!(err, Error::NotAnswered(Reference::parse("01:IDE00191").unwrap()));

		let built = build(
			&input(&["01:F405"], &values_page(&["01:F405"])),
			&store,
			&extracted,
			&units,
			Some(&answered),
			Language::En,
		)
		.unwrap();
		assert_eq!(built.plan.channels[0].did, 0xF405, "a standard row the car was seen to answer");
		assert!(!built.notes.iter().any(|n| n.contains("no record")), "{:?}", built.notes);

		let mut never_asked = poll::Answered::default();
		never_asked.units.insert(ENGINE);
		let built = build(
			&input(&["01:F423"], &values_page(&["01:F423"])),
			&store,
			&extracted,
			&units,
			Some(&never_asked),
			Language::En,
		)
		.unwrap();
		assert!(
			built.notes.iter().any(|n| n.contains("no record of the car answering F423")),
			"{:?}",
			built.notes
		);
	}

	#[test]
	fn a_negative_factor_keeps_its_resolution_and_a_repeated_ref_is_refused() {
		assert_eq!(decimals_for(-0.001), 3);
		assert_eq!(decimals_for(0.1), 1);
		assert_eq!(decimals_for(1.0), 0);
		assert_eq!(decimals_for(-5.0), 0);
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x2029, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let err = build(
			&input(&["01:IDE00191", "01:IDE00191"], &values_page(&["01:IDE00191"])),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap_err();
		assert_eq!(err, Error::Duplicate(Reference::parse("01:IDE00191").unwrap()));
	}

	#[test]
	fn a_unit_without_a_part_number_and_a_second_chart_of_one_channel_are_refused() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x2029, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let mut nameless = identity(ENGINE, "PART1", "EV_Test");
		nameless.part_number = Some("  ".to_string());
		let err = build(
			&input(&["01:IDE00191"], &values_page(&["01:IDE00191"])),
			&store,
			&extracted,
			&[nameless],
			None,
			Language::En,
		)
		.unwrap_err();
		assert_eq!(err, Error::NoPartNumber(ENGINE));

		let twice = "[[page]]\nkind = \"chart\"\ncell = \"01:IDE00191\"\nmin = 0.9\nmax = 2.1\n[[page]]\nkind = \"chart\"\ncell = \"01:IDE00191\"\nmin = 0\nmax = 3\n";
		let err = build(
			&input(&["01:IDE00191"], twice),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap_err();
		assert!(matches!(err, Error::Page(2, _)), "{err}");
	}

	/// A channel's rate is the owner's: written in `dash.toml`, 2 Hz when not, and
	/// carried unchanged into both outputs.
	#[test]
	fn a_rate_is_the_inputs_or_two_hertz_and_reaches_the_firmware() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![
					reading(0x2029, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0),
					reading(0xF405, "Engine Coolant Temperature", "IDE00025", 0, 8, false, true, 1.0, -40.0),
				],
			)],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let text = format!(
			"vin = \"TESTVIN0000000001\"\n[[channel]]\nref = \"01:IDE00191\"\nhz = 10\n[[channel]]\nref = \"01:IDE00025\"\n[[channel]]\nref = \"01:F405\"\nhz = 0.5\n{}",
			values_page(&["01:IDE00191", "01:IDE00025"])
		);
		let input = parse_input(&text).unwrap();
		assert_eq!(input.channels.iter().map(|c| c.hz).collect::<Vec<_>>(), [Some(10.0), None, Some(0.5)]);
		let text = format!(
			"vin = \"TESTVIN0000000001\"\n[[channel]]\nref = \"01:IDE00191\"\nhz = 10\n[[channel]]\nref = \"01:IDE00025\"\n{}",
			values_page(&["01:IDE00191", "01:IDE00025"])
		);
		let built = build(
			&parse_input(&text).unwrap(),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			None,
			Language::En,
		)
		.unwrap();
		assert_eq!(built.plan.channels.iter().map(|c| c.hz).collect::<Vec<_>>(), [10.0, DEFAULT_HZ]);
		let rust = to_rust(&built.plan);
		assert!(rust.contains("proven: false, hz: 10.0, setpoint: None }"), "{rust}");
		assert!(rust.contains("proven: false, hz: 2.0, setpoint: None }"), "{rust}");
		assert!(built.notes[0].contains("at 10 Hz"), "{}", built.notes[0]);

		// A plan.json from before rates reads as the default.
		let mut json: serde_json::Value = serde_json::from_str(&built.plan.to_json()).unwrap();
		json["channels"][0].as_object_mut().unwrap().remove("hz");
		let old = Plan::from_json(&json.to_string()).unwrap();
		assert_eq!(old.channels[0].hz, DEFAULT_HZ);
	}

	#[test]
	fn a_rate_that_is_not_a_positive_number_within_the_ceiling_is_refused() {
		for bad in ["0", "-1", "100.5", "\"fast\"", "nan"] {
			let text = format!(
				"vin = \"X\"\n[[channel]]\nref = \"01:IDE00025\"\nhz = {bad}\n{}",
				values_page(&["01:IDE00025"])
			);
			let err = parse_input(&text).unwrap_err();
			assert!(err.to_string().contains("hz must be"), "{bad}: {err}");
		}
		let text = format!(
			"vin = \"X\"\n[[channel]]\nref = \"01:IDE00025\"\nhz = 100\n{}",
			values_page(&["01:IDE00025"])
		);
		assert_eq!(parse_input(&text).unwrap().channels[0].hz, Some(MAX_HZ));
	}

	#[test]
	fn alarms_reach_both_outputs_in_file_order_by_plan_index() {
		let text = format!(
			"{}{}",
			alarm(&["01:IDE00003"], "B", "above", 10.0, 8.0),
			alarm(&["01:IDE00002", "01:IDE00001"], "A", "below", 0.0, 1.0)
		);
		let built = build_with_alarms(&text).unwrap();
		assert_eq!(
			built.plan.alarms,
			vec![
				Alarm {
					channels: vec![2],
					page: 1,
					rule: AlarmRule::Threshold {
						direction: Direction::Above,
						trip: 10.0,
						release: 8.0
					},
				},
				Alarm {
					channels: vec![1, 0],
					page: 0,
					rule: AlarmRule::Threshold {
						direction: Direction::Below,
						trip: 0.0,
						release: 1.0
					},
				},
			],
			"the file's order is the priority"
		);
		assert!(
			built.notes.iter().any(|n| n.starts_with("alarm #1: 01:IDE00003 at or above 10")),
			"{:?}",
			built.notes
		);

		let json = built.plan.to_json();
		assert_eq!(Plan::from_json(&json).unwrap(), built.plan);
		assert!(json.contains("\"direction\": \"above\""), "{json}");

		let rust = to_rust(&built.plan);
		assert!(
			rust.contains("use vag_dash_render::alarm::{Alarm, ChannelId, Direction, PageId, Rule};"),
			"{rust}"
		);
		assert!(rust.contains("alarms: &ALARMS }"), "{rust}");
		assert!(
			rust.contains("static ALARM_CHANNELS_1: [ChannelId; 2] = [ChannelId(1), ChannelId(0)];"),
			"{rust}"
		);
		assert!(rust.contains("static ALARMS: [Alarm<'static>; 2] = ["), "{rust}");
		assert!(
			rust.contains(
				"Alarm { channels: &ALARM_CHANNELS_0, page: PageId(1), rule: Rule::Threshold { trip: 10.0, release: 8.0, direction: Direction::Above } },"
			),
			"{rust}"
		);

		// A plan.json from before alarms loads with none, and its Rust imports nothing unused.
		let mut old: serde_json::Value = serde_json::from_str(&json).unwrap();
		old.as_object_mut().unwrap().remove("alarms");
		let old = Plan::from_json(&old.to_string()).unwrap();
		assert!(old.alarms.is_empty());
		let rust = to_rust(&old);
		assert!(rust.contains("use vag_dash_render::alarm::Alarm;\n"), "{rust}");
		assert!(rust.contains("static ALARMS: [Alarm<'static>; 0] = ["), "{rust}");
		assert!(!rust.contains("ChannelId"), "{rust}");
	}

	#[test]
	fn an_alarm_the_board_could_not_honour_is_refused_and_the_message_says_why() {
		let refused = |text: &str| build_with_alarms(text).unwrap_err().to_string();
		let good = alarm(&["01:IDE00001"], "A", "below", 0.0, 1.0);
		assert_eq!(
			refused(&alarm(&["01:IDE00009"], "A", "below", 0.0, 1.0)),
			"alarm #1: 01:IDE00009 is not in the [[channel]] list"
		);
		assert_eq!(refused(&alarm(&[], "A", "below", 0.0, 1.0)), "alarm #1: watches no channels");
		assert_eq!(
			refused(&alarm(&["01:IDE00001"], "NOPE", "below", 0.0, 1.0)),
			"alarm #1: no values page is titled \"NOPE\" — a chart page has no title and cannot explain an alarm"
		);
		assert_eq!(
			refused(&format!(
				"{}{}",
				values_page_titled("A", &["01:IDE00003"]),
				alarm(&["01:IDE00003"], "A", "above", 1.0, 0.0)
			)),
			"alarm #1: 2 values pages are titled \"A\" — give them different titles"
		);
		assert_eq!(
			refused(&alarm(&["01:IDE00003", "01:IDE00001"], "B", "above", 10.0, 8.0)),
			"alarm #1: page \"B\" does not show 01:IDE00001 — the page an alarm raises shows every channel it watches"
		);
		assert_eq!(
			refused(&alarm(&["01:IDE00001"], "A", "below", 0.0, -1.0)),
			"alarm #1: release -1 is not above trip 0 — a \"below\" alarm releases above where it trips"
		);
		assert_eq!(
			refused(&alarm(&["01:IDE00001"], "A", "below", 0.5, 0.5)),
			"alarm #1: release 0.5 is not above trip 0.5 — a \"below\" alarm releases above where it trips",
			"no band is no hysteresis"
		);
		assert_eq!(
			refused(&alarm(&["01:IDE00003"], "B", "above", 10.0, 12.0)),
			"alarm #1: release 12 is not below trip 10 — an \"above\" alarm releases below where it trips"
		);
		assert!(
			refused(&format!("{good}{}", alarm(&["01:IDE00009"], "A", "below", 0.0, 1.0))).starts_with("alarm #2: "),
			"a rule is named by its place in the file"
		);
		assert_eq!(
			refused(&good.repeat(MAX_ALARMS + 1)),
			format!(
				"{} [[alarm]] rules, and the board holds at most 4 — each rule's channels are read at full rate on every page",
				MAX_ALARMS + 1
			)
		);
		assert_eq!(build_with_alarms(&good.repeat(MAX_ALARMS)).unwrap().plan.alarms.len(), MAX_ALARMS);
		assert_eq!(
			refused(&alarm(&["01:IDE00003"], "B", "above", 100.000001, 100.0)),
			"alarm #1: release 100 is not below trip 100 — an \"above\" alarm releases below where it trips",
			"apart in the file, one value in the board's f32"
		);
	}

	/// An alarm raises its page by plan index, and the board holds `MAX_PAGES`: a plan
	/// with more is one whose alarm could name a page the glass never shows.
	#[test]
	fn more_pages_than_the_board_holds_are_refused() {
		// The fixture has three pages of its own.
		let extra = |n: usize| values_page_titled("P", &["01:IDE00001"]).repeat(n);
		assert_eq!(
			build_with_alarms(&extra(MAX_PAGES - 2)).unwrap_err().to_string(),
			format!("{} [[page]] tables, and the board holds at most 8", MAX_PAGES + 1)
		);
		assert_eq!(build_with_alarms(&extra(MAX_PAGES - 3)).unwrap().plan.pages.len(), MAX_PAGES);
	}

	#[test]
	fn an_alarm_of_the_wrong_shape_is_refused_when_the_input_is_read() {
		let shape = |table: &str| {
			let text = format!(
				"vin = \"X\"\n[[channel]]\nref = \"01:IDE00001\"\n{}{table}",
				values_page(&["01:IDE00001"])
			);
			parse_input(&text).unwrap_err().to_string()
		};
		let with = |lines: &str| format!("[[alarm]]\nchannels = [\"01:IDE00001\"]\npage = \"T\"\n{lines}\n");
		assert_eq!(
			shape(&with("direction = \"sideways\"\ntrip = 1\nrelease = 0")),
			"dash.toml: alarm #1: direction \"sideways\" is not \"below\" or \"above\""
		);
		assert_eq!(
			shape(&with("trip = 1\nrelease = 0")),
			"dash.toml: alarm #1's direction is missing or not a string"
		);
		assert_eq!(
			shape(&with("direction = \"above\"\nrelease = 0")),
			"dash.toml: alarm #1 needs trip, a finite number"
		);
		assert_eq!(
			shape(&with("direction = \"above\"\ntrip = 1\nrelease = \"low\"")),
			"dash.toml: alarm #1 needs release, a finite number"
		);
		assert_eq!(
			shape(&with("direction = \"above\"\ntrip = 1e40\nrelease = 0")),
			"dash.toml: alarm #1 needs trip, a finite number",
			"past what the board's f32 holds"
		);
		assert_eq!(
			shape(&with("direction = \"above\"\ntrip = nan\nrelease = 0")),
			"dash.toml: alarm #1 needs trip, a finite number"
		);
		assert_eq!(
			shape("[[alarm]]\npage = \"T\"\ndirection = \"above\"\ntrip = 1\nrelease = 0\n"),
			"dash.toml: alarm #1 has no channels list"
		);
		assert_eq!(
			shape("[[alarm]]\nchannels = [\"01:IDE00001\"]\ndirection = \"above\"\ntrip = 1\nrelease = 0\n"),
			"dash.toml: alarm #1's page is missing or not a string"
		);
		assert_eq!(
			shape("[alarm]\npage = \"T\"\n"),
			"dash.toml: alarm must be written as [[alarm]] tables, one per rule"
		);
	}

	/// The one writer is [`build_for_car`], it needs a VIN, and no test may
	/// give it one — the same rule `watch/favourites.rs` keeps for itself.
	#[test]
	fn nothing_here_writes_into_the_owners_own_vagcan() {
		let source = include_str!("dash.rs");
		// The markers are spelled in halves so this test does not contain them
		// and the region it checks runs to the end of the file, itself included.
		let tests = source.split(concat!("#[cfg(", "test)]")).nth(1).expect("this module has tests");
		assert!(
			!tests.contains(concat!("build_for_", "car(")),
			"a test calls the writer, which writes into the owner's real ~/.vagcan"
		);
	}
}
