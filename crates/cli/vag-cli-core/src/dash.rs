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
//! language = "ru"                 # optional: the labels' and the board's; the settings' otherwise
//!
//! [[channel]]
//! ref = "01:IDE00025"              # <unit>:<text id>, or <unit>:<DID>[@<bit offset>]
//! label = "ОЖ"                     # optional; the glossary's wording otherwise
//! decimals = 0                     # optional; derived from the scaling otherwise
//! hz = 10                          # optional; how often the panel reads it, 2 otherwise
//! setpoint = "01:IDE00190"         # optional; what the unit asked for, on the same unit
//! zero_at_rest = true              # optional; 0 while the car stands (the stopwatch's speed reads 0)
//! alarm_max = 105                  # optional; an alarm at or above this, on every page
//! alarm_max_off = 100              # optional; it goes off at or under this; 5 % under by default
//! alarm_min = 40                   # optional; an alarm at or below this (either bound, or both)
//! alarm_min_off = 45               # optional; it goes off at or above this; 5 % over by default
//!
//! [[channel]]
//! ref = "01:IDE00191"
//! setpoint = "01:IDE00190"
//! drift_percent = 10               # optional, with a setpoint: an alarm this far from it
//! drift_off_percent = 6            # off again at or under this share
//! drift_hold_ms = 1000             # and only once the drift has held that long
//! drift_min_setpoint = 0.5         # under this specified value the drift says nothing
//!
//! [[channel]]
//! ref = "02:IDE00102"
//!
//! [strip]                          # optional: the battery voltage in the strip over the pages
//! volts = "01:IDE00018"            # a [[channel]]
//!
//! [[page]]
//! kind = "values"
//! title = "MAIN"                   # optional; drawn in the strip
//! cells = ["01:IDE00025", "02:IDE00102"]
//! volts = false                    # optional; true by default — the strip's voltage on this page
//!
//! [[page]]
//! kind = "chart"
//! title = "COOLANT"                # optional; drawn in the strip, in place of the header's label
//! cell = "01:IDE00025"
//! min = 70
//! max = 110
//!
//! [stalk]                          # optional: the cruise lever as buttons (`todo/dash/19`)
//! read = "70C:1105"                # the one identifier carrying the rocker and the switch
//! rocker = "…"                     # field names as the ODIS project spells them
//! switch = "…"
//! next = "…"                       # states of the rocker, as the project spells them
//! previous = "…"
//! measure = "…"                    # switches the stopwatch on and off
//! switch_off = "…"                 # the switch's off state
//! cruise = "01:203C"               # the engine's cruise status, an enumerated field
//! cruise_off = "…"                 # its off state
//!
//! [stopwatch]                      # optional: the stopwatch page
//! speed = "02:380B"                # a [[channel]], with no offset in its scaling
//! km_h_per_unit = 0.0              # measured on the car; 0 = not measured, the page says so
//! marks = [60, 100]                # km/h, at most 3, each once, none zero
//!
//! [[button]]                       # optional: a button on a pin, to GND; at most 3
//! pin = 3                          # 3, 4 or 5 — the board's free pins
//! action = "next"                  # next | previous | stopwatch
//! ```
//!
//! An alarm raises the first values page that shows its channel, else its chart page. Its priority is the channel's
//! `alarm_priority` (1 first), then the channels' order in the file. The build's `bus:` lines are an
//! estimate of bus time — the board's planner run over the plan at one constant cost an exchange
//! (`dash/bus_time.rs`) — said per page on the glass; they refuse nothing.
//!
//! A unit is spelled the way every other command spells it — `01`, `02`, or a
//! request id — and a channel by the text id the row carries (stable across
//! variants, and the key the glossary is written under) or by identifier and
//! bit offset when it has none. Which unit *variant* the car has is not
//! written here: it comes from the car's record of its units
//! (`~/.vagcan/cars/<VIN>/units.json`, [`crate::units`]) — what each unit said
//! about itself to `vagcan units --identify` or `vagcan watch`, exactly as
//! `watch` finds it.
//!
//! # Refusals
//!
//! The input is read strictly (`dash/strict.rs`): a key or a section it does not have, or a key
//! of the other `kind` of page or rule, is refused with its line, and an optional key of the
//! wrong type is refused rather than read as absent. `docs/dash/dash-toml.md` is the reference
//! for every key.
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
//! `plan.json` for a person to read, `plan.rs` for the firmware, both
//! under `~/.vagcan/dash/<VIN>/` and neither committed anywhere: they are
//! derived from VW's data and describe one owner's car. Both name the plan by its id
//! ([`plan_id`]): `plan.rs` as `PLAN_ID`, `plan.json` as `id`.

use crate::report::Advice as _;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml_edit::{Document, Item};
use vag_dash_render::control::{BUTTON_PINS, Command, MAX_BUTTONS, is_button_pin};
use vag_dash_render::pages::MAX_PAGES;
use vag_dash_render::plan::MAX_VALUE_CELLS;
use vag_dash_render::stopwatch::{MAX_MARKS, MIN_FIT_SAMPLES, SLOWEST_SPEED_PERIOD_MS, START_FIT_MS};
use vag_data_labels::catalog::{CatalogStore, Level, ReadId, Scaling};
use vag_data_labels::measure::RawForm;
use vag_uds_client::address::{self, UnitAddress};
use vag_uds_client::guard::MIN_PERIOD_MS;

use crate::config::Language;
use crate::extracted::Extracted;
use crate::plan::{self as poll, UnitIdentity};

mod bus_time;
mod strict;
use strict::Reader;

/// Where a person finds the ids `dash.toml` names channels by. `watch`'s settings screen spells
/// the setting and its value this way; a test in `vag-cli-diag`'s `watch` holds the two together.
pub const FIND_IDS: &str = "find your car's ids with `vagcan watch`: press `,` and set \"Key at the end of each row\" to shown";

/// The smallest `dash.toml` that builds: one value on one page. `<VIN>` is the car's.
///
/// One text for two readers: README's "Building the dash" shows it (a test holds the README
/// to it), and `dev dash build --help` prints it, which a build with no file points at.
pub const SMALLEST_INPUT: &str = "vin = \"<VIN>\"

[[channel]]
ref = \"01:IDE00025\"    # coolant temperature: unit 01, channel id IDE00025

[[page]]
kind = \"values\"
cells = [\"01:IDE00025\"]
";

/// The reference for every key of `dash.toml`, as a path in the repository.
pub const REFERENCE_DOC: &str = "docs/dash/dash-toml.md";

/// What a build with no `dash.toml` says: where it looked, and how to make one — the page
/// editor, the smallest file in `--help`, every key in the reference, the ids in `watch`.
pub fn no_input(vin: &str, path: &Path) -> anyhow::Error {
	anyhow::anyhow!("no dash.toml for {vin} yet")
		.note(format!("a build reads {}", path.display()))
		.help(format!("lay out its pages with `vagcan dev dash edit {vin}`, then write them there"))
		.help("see the smallest one: `vagcan dev dash build --help`")
		.help(format!("read every key in {REFERENCE_DOC} in the vagcan repository"))
		.help(FIND_IDS)
}

/// What a build whose input is not at `path` says, without an errno (UX review, 2026-10-19).
/// A path the build was `given` (`--input`) is a mistyped path, not a car with no file yet: it
/// gets no starter.
fn missing_input(vin: &str, path: &Path, given: bool) -> anyhow::Error {
	if given {
		anyhow::anyhow!("no file at {}", path.display())
	} else {
		no_input(vin, path)
	}
}

/// What a build for a car with no record of its units says: the commands that write it, in one
/// line. `units --identify` with no unit named, not `--identify <unit>`: that one is a sweep of
/// one unit, and the record of every unit is what the build needs.
pub fn no_record(vin: &str, record: &Path) -> anyhow::Error {
	anyhow::anyhow!("no record of {vin}'s control units at {}", record.display())
		.help("connect to the car and run `vagcan units --identify` or `vagcan watch` once; either writes it")
}

/// Whether the board, which holds this number as an `f32`, holds it at all: finite here, and
/// not an infinity once narrowed.
fn fits_f32(v: f64) -> bool {
	v.is_finite() && (v as f32).is_finite()
}

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
	/// `zero_at_rest = true`: shown as 0 while the car stands — the plan's standstill, today the
	/// stopwatch's speed reading 0 (owner, 2026-10-05: the ESC's acceleration reads the road's
	/// slope at a standstill).
	pub zero_at_rest: bool,
	/// `alarm_max` and its `alarm_max_off`, where the channel has an upper bound.
	pub alarm_max: Option<BoundInput>,
	/// `alarm_min` and its `alarm_min_off`, where it has a lower one.
	pub alarm_min: Option<BoundInput>,
	/// The `drift_*` keys, where the channel has a drift alarm.
	pub drift: Option<DriftInput>,
	/// `alarm_priority`: 1 is the most important. Channels with it come before channels without,
	/// ascending; ties, and channels without it, keep the file's order (owner, 2026-10-19).
	pub alarm_priority: Option<u32>,
}

impl ChannelInput {
	/// A channel by its reference and nothing else.
	fn bare(reference: Reference) -> ChannelInput {
		ChannelInput {
			reference,
			label: None,
			decimals: None,
			hz: None,
			setpoint: None,
			zero_at_rest: false,
			alarm_max: None,
			alarm_min: None,
			drift: None,
			alarm_priority: None,
		}
	}
}

/// One bound of a channel, as `dash.toml` writes it (owner, 2026-10-19): the alarm at or past
/// `at`, and off again at `off` — further inside — or, where the file gives none, at the
/// default [`build`] derives from the channel's resolution.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundInput {
	pub at: f64,
	pub off: Option<f64>,
}

/// A channel's drift alarm (`todo/dash/18-setpoints-and-drift.md` §4): the channel more than
/// `percent` of its specified value away from it, for `hold_ms` without a break; off again at
/// or under `off_percent`; nothing said while the specified value is under `min_setpoint`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftInput {
	pub percent: f64,
	pub off_percent: f64,
	pub hold_ms: u64,
	pub min_setpoint: f64,
}

/// How far inside its bound an alarm goes off when the file gives no `_off` value: this share
/// of the bound's magnitude (owner, 2026-10-19: 5 %), and never less than one step of the
/// channel's resolution — `|factor|`, what one count of its raw value is worth: a distance the
/// unit cannot report is no distance.
pub const DEFAULT_OFF_SHARE: f64 = 0.05;

/// A channel's rate when `dash.toml` gives none (owner, 2026-09-14).
pub const DEFAULT_HZ: f64 = 2.0;
/// The fastest rate a channel may ask for: one read every [`MIN_PERIOD_MS`], the floor the
/// board's guard holds every host's subscription to — 50 Hz, what `measure` reads road speed
/// at. One cap on one bus (`todo/dash/27`, archived): the panel asks no faster than a host may,
/// and `watch`'s own ceiling (`config::MAX_HZ`) is the same 50. It was the planner's whole
/// ceiling of exchanges a second (100) before, which let one channel ask for all of them.
pub const MAX_HZ: f64 = 1000.0 / MIN_PERIOD_MS as f64;

/// One `[[page]]` of the input.
#[derive(Debug, Clone, PartialEq)]
pub enum PageInput {
	Values {
		title: String,
		cells: Vec<Reference>,
		/// `volts`, where the page writes it: whether the strip shows the voltage here. `None`
		/// is the default, on.
		volts: Option<bool>,
	},
	Chart {
		title: String,
		cell: Reference,
		min: f64,
		max: f64,
		volts: Option<bool>,
	},
}

impl PageInput {
	fn volts(&self) -> Option<bool> {
		match self {
			PageInput::Values { volts, .. } | PageInput::Chart { volts, .. } => *volts,
		}
	}
}

/// `[strip]`: what the strip over the pages shows besides their titles (owner, 2026-10-19).
#[derive(Debug, Clone, PartialEq)]
pub struct StripInput {
	/// The battery voltage: a `[[channel]]`.
	pub volts: Reference,
}

/// `[stalk]`: the cruise lever as buttons (`todo/dash/19`). Every name is the ODIS
/// project's own spelling; the build resolves them against the car's variant.
#[derive(Debug, Clone, PartialEq)]
pub struct StalkInput {
	/// The unit and the identifier whose answer carries the rocker and the switch.
	pub request: u16,
	pub did: u16,
	/// Field names within that identifier.
	pub rocker: String,
	pub switch: String,
	/// States of the rocker.
	pub next: String,
	pub previous: String,
	pub measure: String,
	/// The switch's off state.
	pub switch_off: String,
	/// The cruise status: an enumerated field, named as a channel is.
	pub cruise: Reference,
	/// Its off state.
	pub cruise_off: String,
}

/// `[stopwatch]`: the stopwatch page (`todo/dash/19`, `todo/dash/14` §6).
#[derive(Debug, Clone, PartialEq)]
pub struct StopwatchInput {
	/// A `[[channel]]`.
	pub speed: Reference,
	/// km/h per unit of the channel's value, measured on the car; `0.0` is "not measured".
	pub km_h_per_unit: f64,
	/// In km/h, in the owner's order.
	pub marks: Vec<u16>,
}

/// A `[[button]]`: a button on one of the board's free pins, wired to GND, and the one command
/// a press of it gives (`todo/dash/19`, "Input backends"). Nothing about it is resolved
/// against the car, so the input's and the plan's are one type. In `plan.json` its action is
/// the word `dash.toml` gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Button {
	/// The GPIO, one of [`BUTTON_PINS`].
	pub pin: u8,
	#[serde(with = "action_name")]
	pub action: Command,
}

/// A [`Command`] in `plan.json` by its `dash.toml` name ([`Command::name`]) — one vocabulary,
/// the board's.
mod action_name {
	use serde::de::Error as _;
	use serde::{Deserialize, Deserializer, Serializer};
	use vag_dash_render::control::Command;

	pub fn serialize<S: Serializer>(command: &Command, to: S) -> Result<S::Ok, S::Error> {
		to.serialize_str(command.name())
	}

	pub fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Command, D::Error> {
		let name = String::deserialize(from)?;
		Command::from_name(&name).ok_or_else(|| D::Error::custom(format!("action {name:?} is not next, previous or stopwatch")))
	}
}

/// The whole input, parsed and nothing more.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
	pub vin: String,
	pub language: Option<Language>,
	/// In the file's order, which is also the alarms' priority.
	pub channels: Vec<ChannelInput>,
	pub pages: Vec<PageInput>,
	pub stalk: Option<StalkInput>,
	pub stopwatch: Option<StopwatchInput>,
	/// In the file's order.
	pub buttons: Vec<Button>,
	pub strip: Option<StripInput>,
}

/// Parse a build input. Only the shape is checked here; whether the car has
/// the channels is [`build`]'s question.
///
/// **Strictly** (`dash/strict.rs`): a key or a section this file does not have is refused by
/// name, and an optional key of the wrong type is refused rather than read as absent.
pub fn parse_input(text: &str) -> Result<Input, Error> {
	let doc = Document::parse(text).map_err(|e| Error::Parse(format!("dash.toml: {e}")))?;
	let top = Reader::new(text, doc.as_table(), String::new());
	// `[[alarm]]` is gone (owner, 2026-10-19): an alarm is keys of the channel it watches. Two
	// ways to write one job is a defect, so the old one is refused, with its rewrite.
	if let Some(item) = doc.get("alarm") {
		let declared: Vec<String> = doc
			.get("channel")
			.and_then(Item::as_array_of_tables)
			.into_iter()
			.flat_map(|tables| tables.iter())
			.filter_map(|table| table.get("ref").and_then(Item::as_str).map(str::to_string))
			.collect();
		return Err(top.refuse("alarm", migrate_alarms(item, &declared)));
	}

	top.takes(&strict::TOP)?;
	// `[channel]` for `[[channel]]`: TOML reads it as one table, which is not the list the
	// build reads, so without this it was "no [[channel]]" under a channel in plain sight.
	for name in ["channel", "page", "button"] {
		if doc.get(name).is_some_and(Item::is_table) {
			return Err(top.refuse(
				name,
				format!("[{name}] has one pair of brackets — write [[{name}]], two pairs: the file holds a list of them"),
			));
		}
	}
	let string = |item: Option<&Item>, what: &str| -> Result<String, Error> {
		item
			.and_then(Item::as_str)
			.map(|s| s.trim().to_string())
			.filter(|s| !s.is_empty())
			.ok_or_else(|| Error::Parse(format!("dash.toml: {what} is missing or not a string")))
	};
	let number = |item: Option<&Item>| item.and_then(|i| i.as_float().or_else(|| i.as_integer().map(|n| n as f64)));
	let vin = string(doc.get("vin"), "vin")?;
	let language = match top.string("language", "a string, \"en\" or \"ru\"")? {
		Some(code) => {
			Some(Language::parse(code).ok_or_else(|| Error::Parse(format!("dash.toml: language {code:?} is not one this build has words for")))?)
		}
		None => None,
	};
	let mut channels = Vec::new();
	if let Some(tables) = doc.get("channel").and_then(Item::as_array_of_tables) {
		for (i, table) in tables.iter().enumerate() {
			let at = Reader::new(text, table, format!("[[channel]] {}", i + 1));
			at.takes(&strict::CHANNEL)?;
			let reference = Reference::parse(&string(table.get("ref"), &format!("channel #{}'s ref", i + 1))?)?;
			let label = at
				.string("label", "a string")?
				.map(str::trim)
				.filter(|s| !s.is_empty())
				.map(str::to_string);
			let decimals = match at.integer("decimals", "a whole number from 0 to 3")? {
				Some(d) if (0..=3).contains(&d) => Some(d as u8),
				Some(d) => return Err(Error::Parse(format!("dash.toml: {reference}: decimals {d} is not 0..=3"))),
				None => None,
			};
			let hz = match table.get("hz") {
				None => None,
				Some(item) => match item.as_float().or_else(|| item.as_integer().map(|n| n as f64)) {
					// Above 0 as the board holds it too: an `f32` rounds a small enough rate to 0,
					// which the board reads as "no rate" and polls at its fallback instead.
					Some(hz) if hz > 0.0 && hz <= MAX_HZ && (hz as f32) == 0.0 => {
						return Err(Error::Parse(format!(
							"dash.toml: {reference}: hz {hz:e} is too small for the board, which would hold it as 0"
						)));
					}
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
			let zero_at_rest = match table.get("zero_at_rest") {
				None => false,
				Some(item) => item
					.as_bool()
					.ok_or_else(|| Error::Parse(format!("dash.toml: {reference}: zero_at_rest is not true or false")))?,
			};
			let (alarm_max, alarm_min, drift) = parse_alarms(&at, table, &number)?;
			let alarm_priority = match at.integer("alarm_priority", "a whole number, 1 or more")? {
				None => None,
				Some(_) if alarm_max.is_none() && alarm_min.is_none() && drift.is_none() => {
					return Err(at.refuse(
						"alarm_priority",
						"alarm_priority needs an alarm on the channel: alarm_max, alarm_min or drift_percent".to_string(),
					));
				}
				Some(n) if n >= 1 && n <= i64::from(u32::MAX) => Some(n as u32),
				Some(n) => return Err(at.refuse("alarm_priority", format!("alarm_priority {n} is not a whole number, 1 or more"))),
			};
			channels.push(ChannelInput {
				reference,
				label,
				decimals,
				hz,
				setpoint,
				zero_at_rest,
				alarm_max,
				alarm_min,
				drift,
				alarm_priority,
			});
		}
	}
	if channels.is_empty() {
		return Err(Error::Parse(
			"dash.toml: no [[channel]] — a plan with nothing to read is not a plan".to_string(),
		));
	}

	let mut pages = Vec::new();
	if let Some(tables) = doc.get("page").and_then(Item::as_array_of_tables) {
		for (i, table) in tables.iter().enumerate() {
			let n = i + 1;
			let at = Reader::new(text, table, format!("[[page]] {n}"));
			let kind = string(table.get("kind"), &format!("page #{n}'s kind"))?;
			match kind.as_str() {
				"values" => {
					at.takes(&strict::VALUES)?;
					let title = at.string("title", "a string")?.unwrap_or("").trim().to_string();
					let volts = at.boolean("volts")?;
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
					pages.push(PageInput::Values { title, cells, volts });
				}
				"chart" => {
					at.takes(&strict::CHART)?;
					let title = at.string("title", "a string")?.unwrap_or("").trim().to_string();
					let volts = at.boolean("volts")?;
					let cell = Reference::parse(&string(table.get("cell"), &format!("page #{n}'s cell"))?)?;
					// The board draws the chart in `f32`: an end past what one holds would reach the
					// generated source as an `inf` the firmware's build cannot read.
					let end = |key: &str| match (table.get(key), number(table.get(key))) {
						(None, _) => Err(Error::Parse(format!("dash.toml: page #{n} needs {key}"))),
						(Some(item), None) => Err(at.refuse(key, format!("{key} must be a number, not {}", strict::a(item.type_name())))),
						(_, Some(v)) if fits_f32(v) => Ok(v),
						(_, Some(_)) => Err(at.refuse(
							key,
							format!("{key} must be a finite number the board's 32-bit float holds, within ±{:e}", f32::MAX),
						)),
					};
					let (min, max) = (end("min")?, end("max")?);
					pages.push(PageInput::Chart {
						title,
						cell,
						min,
						max,
						volts,
					});
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

	// A typo in a `[[button]]` table is refused here, like any other key; its values are read
	// by `parse_buttons` below.
	if let Some(tables) = doc.get("button").and_then(Item::as_array_of_tables) {
		for (i, table) in tables.iter().enumerate() {
			Reader::new(text, table, format!("[[button]] {}", i + 1)).takes(&strict::BUTTON)?;
		}
	}
	let stalk = match doc.get("stalk") {
		None => None,
		Some(item) => Some(parse_stalk(item, text, &string)?),
	};
	let stopwatch = match doc.get("stopwatch") {
		None => None,
		Some(item) => Some(parse_stopwatch(item, text, &string, &number)?),
	};
	let buttons = match doc.get("button") {
		None => Vec::new(),
		Some(item) => parse_buttons(item)?,
	};
	let strip = match doc.get("strip") {
		None => None,
		Some(item) => Some(parse_strip(item, text, &string)?),
	};
	Ok(Input {
		vin,
		language,
		channels,
		pages,
		stalk,
		stopwatch,
		buttons,
		strip,
	})
}

/// A channel's `alarm_max`, `alarm_min` and drift, as written.
type ChannelAlarms = (Option<BoundInput>, Option<BoundInput>, Option<DriftInput>);

/// A `[[channel]]`'s alarm keys, shape only: its bounds as written and its drift. The defaults
/// of `alarm_max_off` and `alarm_min_off`, the check of one bound against the other and the
/// drift's setpoint are [`build`]'s: they need the resolved channel.
fn parse_alarms(at: &Reader<'_>, table: &toml_edit::Table, number: &impl Fn(Option<&Item>) -> Option<f64>) -> Result<ChannelAlarms, Error> {
	// The board compares in `f32`, so a value past what one holds is refused rather than turned
	// into an infinity nothing ever reaches.
	let value = |key: &str| -> Result<Option<f64>, Error> {
		let Some(item) = table.get(key) else { return Ok(None) };
		match number(Some(item)) {
			None => Err(at.refuse(key, format!("{key} must be a number, not {}", strict::a(item.type_name())))),
			Some(v) if fits_f32(v) => Ok(Some(v)),
			Some(v) => Err(at.refuse(
				key,
				format!("{key} {v:e} is not a number the board's 32-bit float holds (within ±{:e})", f32::MAX),
			)),
		}
	};
	let bound = |key: &str, off_key: &str, inside: &str, beyond: fn(f32, f32) -> bool| -> Result<Option<BoundInput>, Error> {
		match (value(key)?, value(off_key)?) {
			(None, None) => Ok(None),
			(None, Some(_)) => Err(at.refuse(off_key, format!("{off_key} needs {key}: it is the value the {key} alarm goes off at"))),
			(Some(at_value), off) => {
				if let Some(off) = off
					&& !beyond(off as f32, at_value as f32)
				{
					return Err(at.refuse(
						off_key,
						format!(
							"{off_key} {} is not {inside} {key} {} — the alarm goes off once the value is back at {off_key}",
							off as f32, at_value as f32
						),
					));
				}
				Ok(Some(BoundInput { at: at_value, off }))
			}
		}
	};
	let alarm_max = bound("alarm_max", "alarm_max_off", "under", |off, at| off < at)?;
	let alarm_min = bound("alarm_min", "alarm_min_off", "over", |off, at| off > at)?;

	const DRIFT: [&str; 4] = ["drift_percent", "drift_off_percent", "drift_hold_ms", "drift_min_setpoint"];
	let given: Vec<&str> = DRIFT.iter().copied().filter(|k| table.contains_key(k)).collect();
	if given.is_empty() {
		return Ok((alarm_max, alarm_min, None));
	}
	let missing: Vec<&str> = DRIFT.iter().copied().filter(|k| !table.contains_key(k)).collect();
	if !missing.is_empty() {
		return Err(at.refuse(
			given[0],
			format!("a drift alarm takes all four of {} — missing {}", DRIFT.join(", "), missing.join(", ")),
		));
	}
	// Above zero as the board holds it too: a share an `f32` rounds to 0 is a `drift_off_percent`
	// nothing ever drops to, an alarm that never goes off.
	let share = |key: &str| -> Result<f64, Error> {
		match value(key)?.expect("checked present") {
			v if v > 0.0 && (v as f32) > 0.0 => Ok(v),
			v if v > 0.0 => Err(at.refuse(key, format!("{key} {v:e} is too small for the board, which would hold it as 0"))),
			v => Err(at.refuse(key, format!("{key} {v} is not above zero"))),
		}
	};
	let percent = share("drift_percent")?;
	let off_percent = share("drift_off_percent")?;
	if off_percent as f32 >= percent as f32 {
		return Err(at.refuse(
			"drift_off_percent",
			format!(
				"drift_off_percent {off_percent} is not under drift_percent {percent} — the alarm goes off once the drift is back at drift_off_percent"
			),
		));
	}
	let hold_ms = match table.get("drift_hold_ms").and_then(Item::as_integer) {
		Some(ms) if ms >= 0 => ms as u64,
		_ => return Err(at.refuse("drift_hold_ms", "drift_hold_ms must be whole milliseconds, 0 or more".to_string())),
	};
	let min_setpoint = match value("drift_min_setpoint")?.expect("checked present") {
		v if v >= 0.0 => v,
		v => return Err(at.refuse("drift_min_setpoint", format!("drift_min_setpoint {v} is below zero"))),
	};
	Ok((
		alarm_max,
		alarm_min,
		Some(DriftInput {
			percent,
			off_percent,
			hold_ms,
			min_setpoint,
		}),
	))
}

/// What to write instead of the `[[alarm]]` tables `item` holds: each rule's numbers as the keys
/// of every channel it listed, one `[[channel]]` block per channel, read loosely — a rule that
/// cannot be read is said in words. `declared` is the file's `[[channel]]` refs, in order: where
/// the old rules' order is not the channels', each block carries the `alarm_priority` that keeps
/// it — no channel moves, so nothing on the board is renumbered (owner, 2026-10-19).
fn migrate_alarms(item: &Item, declared: &[String]) -> String {
	let number = |item: Option<&Item>| -> Option<String> {
		let item = item?;
		item
			.as_integer()
			.map(|n| n.to_string())
			.or_else(|| item.as_float().map(|v| format!("{v:?}")))
	};
	// Per channel, in the order the rules name them — the old priority — the number of the first
	// rule that names it, and its keys.
	type Block = (String, usize, Vec<(&'static str, String)>);
	let mut blocks: Vec<Block> = Vec::new();
	let mut unread = 0;
	for (n, table) in item.as_array_of_tables().into_iter().flat_map(|tables| tables.iter()).enumerate() {
		let get = |key: &str| number(table.get(key));
		let channels: Vec<String> = table
			.get("channels")
			.and_then(Item::as_array)
			.map(|list| list.iter().filter_map(|v| v.as_str()).map(|c| c.trim().to_string()).collect())
			.unwrap_or_default();
		let keys: Option<Vec<(&'static str, String)>> = match table.get("kind").and_then(Item::as_str).map(str::trim) {
			Some("drift") => match (get("percent"), get("release_percent"), get("hold_ms"), get("min_setpoint")) {
				(Some(p), Some(r), Some(h), Some(m)) => Some(vec![
					("drift_percent", p),
					("drift_off_percent", r),
					("drift_hold_ms", h),
					("drift_min_setpoint", m),
				]),
				_ => None,
			},
			None | Some("threshold") => match (table.get("direction").and_then(Item::as_str), get("trip"), get("release")) {
				(Some("above"), Some(trip), Some(release)) => Some(vec![("alarm_max", trip), ("alarm_max_off", release)]),
				(Some("below"), Some(trip), Some(release)) => Some(vec![("alarm_min", trip), ("alarm_min_off", release)]),
				_ => None,
			},
			Some(_) => None,
		};
		let Some(keys) = keys.filter(|_| !channels.is_empty()) else {
			unread += 1;
			continue;
		};
		// A key a channel already has from an earlier rule cannot be written twice.
		let clash = channels.iter().any(|c| {
			blocks
				.iter()
				.any(|(at, _, have)| at == c && have.iter().any(|(k, _)| keys.iter().any(|(new, _)| new == k)))
		});
		if clash {
			unread += 1;
			continue;
		}
		for channel in channels {
			match blocks.iter_mut().find(|(at, _, _)| *at == channel) {
				Some((_, _, have)) => have.extend(keys.iter().cloned()),
				None => blocks.push((channel, n + 1, keys.clone())),
			}
		}
	}
	let mut why = String::from(
		"[[alarm]] is gone: an alarm is now keys of the [[channel]] it watches. It raises the first values page that shows \
		 that channel — `page` is gone. Delete every [[alarm]] table",
	);
	if blocks.is_empty() {
		why.push('.');
	} else {
		// The old priority was the rules' order; without `alarm_priority` the new one is the
		// channels' order in the file.
		let old: Vec<&str> = blocks.iter().map(|(c, _, _)| c.as_str()).collect();
		let now: Vec<&str> = declared.iter().map(|c| c.trim()).filter(|c| old.contains(c)).collect();
		let kept: Vec<&str> = old.iter().copied().filter(|c| now.contains(c)).collect();
		let reordered = now != kept;
		why.push_str(" and add these keys to the [[channel]] with the same ref:\n");
		for (channel, rule, keys) in &blocks {
			why.push_str(&format!("\n[[channel]]\nref = {channel:?}\n"));
			for (key, value) in keys {
				why.push_str(&format!("{key} = {value}\n"));
			}
			if reordered {
				why.push_str(&format!("alarm_priority = {rule}\n"));
			}
		}
		if reordered {
			why.push_str(
				"\nalarm_priority keeps the [[alarm]] order: the channels are not in it in the file, and without alarm_priority \
				 the order in the file decides.",
			);
		}
	}
	if unread > 0 || blocks.is_empty() {
		why.push_str(
			"\nA threshold rule's trip and release become alarm_max and alarm_max_off for direction = \"above\", alarm_min and \
			 alarm_min_off for \"below\"; a drift rule's percent, release_percent, hold_ms and min_setpoint become drift_percent, \
			 drift_off_percent, drift_hold_ms and drift_min_setpoint.",
		);
	}
	why
}
/// `[strip]`, shape only: the channel the voltage is read from. Whether it is a `[[channel]]`
/// is [`build`]'s question.
fn parse_strip(item: &Item, source: &str, string: &impl Fn(Option<&Item>, &str) -> Result<String, Error>) -> Result<StripInput, Error> {
	let table = item
		.as_table()
		.ok_or_else(|| Error::Parse("dash.toml: strip must be one [strip] table".to_string()))?;
	Reader::new(source, table, "[strip]".to_string()).takes(&strict::STRIP)?;
	let volts = Reference::parse(&string(table.get("volts"), "[strip] volts")?)?;
	Ok(StripInput { volts })
}

/// `[[button]]`, whole — nothing in it is the car's: each a pin the board has free and an
/// action, one button per pin, at most [`MAX_BUTTONS`].
fn parse_buttons(item: &Item) -> Result<Vec<Button>, Error> {
	let refuse = |why: String| Err(Error::Parse(format!("dash.toml: {why}")));
	// A single `[button]` table would otherwise be skipped without a word.
	let Some(tables) = item.as_array_of_tables() else {
		return refuse("button must be written as [[button]] tables, one per button".to_string());
	};
	let names = || {
		let names: Vec<String> = Command::ALL.iter().map(|c| format!("{:?}", c.name())).collect();
		format!("{} or {}", names[..names.len() - 1].join(", "), names[names.len() - 1])
	};
	if tables.len() > MAX_BUTTONS {
		return refuse(format!(
			"{} [[button]] tables, and the board has {MAX_BUTTONS} pins free for one: {}",
			tables.len(),
			pins("and")
		));
	}
	let mut buttons: Vec<Button> = Vec::new();
	for (i, table) in tables.iter().enumerate() {
		let n = i + 1;
		let Some(pin) = table.get("pin").and_then(Item::as_integer) else {
			return refuse(format!("button #{n} needs pin, a whole number: {}", pins("or")));
		};
		let pin = match u8::try_from(pin) {
			Ok(free) if is_button_pin(free) => free,
			_ => {
				let why = pin_taken(pin).unwrap_or("is not free");
				return refuse(format!("button #{n}: pin {pin} {why} — a button goes on pin {}", pins("or")));
			}
		};
		if let Some(at) = buttons.iter().position(|b| b.pin == pin) {
			return refuse(format!("button #{n}: pin {pin} is button #{}'s already — one button per pin", at + 1));
		}
		let Some(name) = table.get("action").and_then(Item::as_str).map(str::trim) else {
			return refuse(format!("button #{n} needs action: {}", names()));
		};
		let Some(action) = Command::from_name(name) else {
			return refuse(format!("button #{n}: action {name:?} is not {}", names()));
		};
		buttons.push(Button { pin, action });
	}
	Ok(buttons)
}

/// The pins a `[[button]]` may take, for a person: `3, 4 or 5` (`and` in place of `or` for a
/// list of all of them).
fn pins(last: &str) -> String {
	let pins: Vec<String> = BUTTON_PINS.iter().map(u8::to_string).collect();
	format!("{} {last} {}", pins[..pins.len() - 1].join(", "), pins[pins.len() - 1])
}

/// What holds GPIO `pin` on this board, worded to follow "pin N" — `None` for a pin a
/// `[[button]]` may take. The board's wiring, not any car's (`todo/dash/15-enclosure.md` §3;
/// [`BUTTON_PINS`] is the other half of the same table).
fn pin_taken(pin: i64) -> Option<&'static str> {
	Some(match pin {
		0 => "is the OLED's D/C",
		1 => "is the CAN transceiver's RX",
		2 => "is a strapping pin, read at reset",
		3..=5 => return None,
		6 => "is the CAN transceiver's TX",
		7 => "is the OLED's SDIN",
		8 => "is the LED's, and a strapping pin",
		9 => "is the BOOT button's, and a strapping pin",
		10 => "is the OLED's SCLK",
		11 => "is not broken out on the SuperMini",
		12..=17 => "is the SPI flash's",
		18 => "is USB's D−",
		19 => "is USB's D+",
		20 => "is the OLED's RES",
		21 => "is the OLED's CS",
		_ => "is not a GPIO of the ESP32-C3",
	})
}

/// `[stalk]`, shape only: every key present, `read` one identifier.
fn parse_stalk(item: &Item, source: &str, string: &impl Fn(Option<&Item>, &str) -> Result<String, Error>) -> Result<StalkInput, Error> {
	let table = item
		.as_table()
		.ok_or_else(|| Error::Parse("dash.toml: stalk must be one [stalk] table".to_string()))?;
	Reader::new(source, table, "[stalk]".to_string()).takes(&strict::STALK)?;
	let text = |key: &str| string(table.get(key), &format!("[stalk] {key}"));
	let (request, did) = match Reference::parse(&text("read")?)? {
		Reference::Field { request, did, bit_offset: 0 } => (request, did),
		other => {
			return Err(Error::Parse(format!(
				"dash.toml: [stalk] read {other} is not <unit>:<DID> — one identifier, whose fields the rocker and the switch are"
			)));
		}
	};
	Ok(StalkInput {
		request,
		did,
		rocker: text("rocker")?,
		switch: text("switch")?,
		next: text("next")?,
		previous: text("previous")?,
		measure: text("measure")?,
		switch_off: text("switch_off")?,
		cruise: Reference::parse(&text("cruise")?)?,
		cruise_off: text("cruise_off")?,
	})
}

/// `[stopwatch]`, shape only: a speed reference, a factor that is a number at or above
/// zero, and marks that are each a speed, once.
fn parse_stopwatch(
	item: &Item,
	source: &str,
	string: &impl Fn(Option<&Item>, &str) -> Result<String, Error>,
	number: &impl Fn(Option<&Item>) -> Option<f64>,
) -> Result<StopwatchInput, Error> {
	let table = item
		.as_table()
		.ok_or_else(|| Error::Parse("dash.toml: stopwatch must be one [stopwatch] table".to_string()))?;
	Reader::new(source, table, "[stopwatch]".to_string()).takes(&strict::STOPWATCH)?;
	let speed = Reference::parse(&string(table.get("speed"), "[stopwatch] speed")?)?;
	// Compared as the board holds it, in `f32`: a factor too small for one would reach the
	// board as the zero that means "not measured", and one too large as an infinity.
	let km_h_per_unit = match number(table.get("km_h_per_unit")) {
		Some(v) if v.is_finite() && (v == 0.0 || (v > 0.0 && (v as f32).is_finite() && (v as f32) > 0.0)) => v,
		Some(v) if v > 0.0 && (v as f32) == 0.0 => {
			return Err(Error::Parse(format!(
				"dash.toml: [stopwatch] km_h_per_unit {v} is too small for the board, which would hold it as 0 — not measured"
			)));
		}
		Some(v) if v > 0.0 && (v as f32).is_infinite() => {
			return Err(Error::Parse(format!(
				"dash.toml: [stopwatch] km_h_per_unit {v:e} is too large for the board, which holds it as a 32-bit float (at most {:e})",
				f32::MAX
			)));
		}
		_ => {
			return Err(Error::Parse(
				"dash.toml: [stopwatch] km_h_per_unit must be a number at or above 0 — 0 until it is measured on the car".to_string(),
			));
		}
	};
	let list = table
		.get("marks")
		.and_then(Item::as_array)
		.ok_or_else(|| Error::Parse("dash.toml: [stopwatch] marks must be a list of speeds in km/h".to_string()))?;
	let mut marks: Vec<u16> = Vec::new();
	for value in list.iter() {
		let mark = match value.as_integer() {
			Some(v) if (1..=i64::from(u16::MAX)).contains(&v) => v as u16,
			_ => {
				return Err(Error::Parse(format!(
					"dash.toml: [stopwatch] mark {value} is not a whole speed in km/h above 0 — a run starts at 0, so 0 is no mark"
				)));
			}
		};
		if marks.contains(&mark) {
			return Err(Error::Parse(format!("dash.toml: [stopwatch] mark {mark} is listed twice")));
		}
		marks.push(mark);
	}
	if marks.is_empty() || marks.len() > MAX_MARKS {
		return Err(Error::Parse(format!(
			"dash.toml: [stopwatch] has {} marks, and the page holds 1 to {MAX_MARKS}",
			marks.len()
		)));
	}
	Ok(StopwatchInput { speed, km_h_per_unit, marks })
}

/// What can go wrong between an input and a plan. Every variant names the
/// thing that failed, because "the build failed" is not something a person can
/// act on.
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
	Parse(String),
	/// The car's record has nothing about this unit — it was not among the
	/// units that answered the command that wrote it.
	UnknownUnit(u16),
	/// A request id in neither of the blocks this tool knows the response rule for.
	NoResponseRule(u16),
	/// The resolved variant does not declare it and nothing proved it.
	Undeclared(Reference),
	/// A channel's name where its id goes — `01:Engine Coolant Temperature`, or a text with a
	/// space no row is named. `ids` are the rows on that unit carrying that name that a numeric
	/// cell takes, each spelled the way the build reads it as that row; `not_linear`, that the
	/// name is carried only by rows the board cannot show.
	NameNotId {
		reference: Reference,
		ids: Vec<String>,
		not_linear: bool,
	},
	/// The unit is on the car's record, and the project has no channel at all for it.
	NoChannels(Reference),
	/// More than one row answers to it; the input has to say which.
	Ambiguous(Reference, Vec<String>),
	/// Declared, but with a scaling the device cannot apply.
	NotLinear(Reference, String),
	/// The same channel twice in `[[channel]]` — the second would be dropped
	/// with its label, and dropping silently is how a wrong label ships.
	Duplicate(Reference),
	/// A factor or offset that is not a number the device can multiply by.
	NotFinite(Reference),
	/// The car's record has no `F187` for this unit, so the firmware could never
	/// confirm it is talking to the unit the plan was built for.
	NoPartNumber(u16),
	/// A page names a channel the input's `[[channel]]` list does not carry.
	PageRefersToUnknown {
		page: usize,
		reference: Reference,
	},
	Page(usize, String),
	/// A channel's alarm keys the board could not honour.
	Alarm(Reference, String),
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
	/// A `[stalk]` the project or the car cannot honour.
	Stalk(String),
	/// A `[stopwatch]` the plan cannot honour.
	Stopwatch(String),
	/// A `[strip]` the plan cannot honour.
	Strip(String),
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Error::Parse(why) => write!(f, "{why}"),
			Error::UnknownUnit(r) => write!(
				f,
				"unit {r:03X} is not in the car's record of its units — it has not said what it is, so nothing can be resolved for it; \
				 `vagcan units --identify` with the car records every unit that answers"
			),
			Error::NoResponseRule(r) => write!(f, "unit {r:03X}: no rule for which id it answers on"),
			Error::Undeclared(r) => {
				let unit = r.to_string();
				let unit = unit.split(':').next().unwrap_or_default();
				let what = match r {
					Reference::TextId { text_id, .. } => format!("no channel with the id {text_id}"),
					Reference::Field { did, bit_offset, .. } => format!("no channel at identifier {did:04X}, bit {bit_offset}"),
				};
				write!(
					f,
					"{r}: unit {unit} has {what} in this car's project data, and none was measured on a car — {FIND_IDS}"
				)
			}
			Error::NameNotId { reference, ids, not_linear } => {
				let Reference::TextId { text_id, .. } = reference else {
					return write!(f, "{reference}: a name, not a channel id — {FIND_IDS}");
				};
				match ids.as_slice() {
					[] if *not_linear => write!(
						f,
						"{reference}: {text_id:?} is the name of a channel the board cannot show — its scaling is not linear; pick a numeric one: {FIND_IDS}"
					),
					[] => write!(
						f,
						"{reference}: {text_id:?} is a name or a mistyped id, and dash.toml takes a channel's id — {FIND_IDS}"
					),
					[one] => write!(f, "{reference}: {text_id:?} is the channel's name — write its id: {one}"),
					many => write!(
						f,
						"{reference}: {text_id:?} is the name of {} channels — write the id of one: {}",
						many.len(),
						many.join(", ")
					),
				}
			}
			Error::NoChannels(r) => {
				let unit = r.to_string();
				let unit = unit.split(':').next().unwrap_or_default();
				write!(
					f,
					"{r}: unit {unit} is in the car's record, but the project data has no channels for its variant — pick a channel of another unit; {FIND_IDS}"
				)
			}
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
			Error::NotFinite(r) => write!(f, "{r}: its scaling is not a finite number the board's 32-bit float holds"),
			Error::NoPartNumber(r) => write!(
				f,
				"unit {r:03X}: the car's record has no part number (F187) for it, and the firmware checks the unit against the plan by that"
			),
			Error::PageRefersToUnknown { page, reference } => write!(f, "page #{page}: {reference} is not in the [[channel]] list"),
			Error::Page(n, why) => write!(f, "page #{n}: {why}"),
			Error::Alarm(r, why) => write!(f, "{r}: {why}"),
			Error::TooManyPages(n) => write!(f, "{n} [[page]] tables, and the board holds at most {MAX_PAGES}"),
			Error::Setpoint { channel, setpoint, why } => {
				write!(f, "{channel}: its setpoint {setpoint} {why}")
			}
			Error::Stalk(why) => write!(f, "[stalk] {why}"),
			Error::Stopwatch(why) => write!(f, "[stopwatch] {why}"),
			Error::Strip(why) => write!(f, "[strip] {why}"),
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
	/// Readings a second while the channel's page is shown.
	pub hz: f64,
	/// Where the row came from: its text id, or the catalog's own name.
	pub source: String,
	/// The channel holding what the unit asked for, by index into the plan's channels.
	pub setpoint: Option<u16>,
	/// Shown as 0 while the car stands (`dash.toml`'s `zero_at_rest`).
	pub zero_at_rest: bool,
}

/// One page. `title` is drawn in the strip over it; `volts` is whether the strip shows the
/// voltage there.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Page {
	Chart {
		title: String,
		channel: u16,
		min: f64,
		max: f64,
		volts: bool,
	},
	Values {
		title: String,
		cells: Vec<u16>,
		volts: bool,
	},
}

/// `[strip]`, resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Strip {
	/// The battery voltage, by plan index.
	pub volts: u16,
}

/// One bound of a channel, resolved: the alarm at or past `at`, off again at `off` — the
/// file's, or the default [`build`] derived.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Bound {
	pub at: f64,
	pub off: f64,
}

/// One alarm's rule, resolved. `specified` is the plan index of the channel's specified value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum AlarmRule {
	/// `alarm_max` and `alarm_min`, either or both.
	Bounds {
		#[serde(default, skip_serializing_if = "Option::is_none")]
		max: Option<Bound>,
		#[serde(default, skip_serializing_if = "Option::is_none")]
		min: Option<Bound>,
	},
	/// The `drift_*` keys.
	Drift {
		specified: u16,
		percent: f64,
		off_percent: f64,
		hold_ms: u64,
		min_setpoint: f64,
	},
}

/// One alarm, resolved: one channel's bounds or its drift, and the first values page that
/// shows the channel, else its chart page — indices into the plan's channels and pages, which is what
/// `vag_dash_render::alarm::ChannelId` and `PageId` are for an image built for one plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alarm {
	pub channel: u16,
	pub page: u16,
	#[serde(flatten)]
	pub rule: AlarmRule,
}

/// One state of an enumerated field: its interval of raw values, both ends included, and
/// its name — the name for a person reading `plan.json`, the interval for the board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
	pub lower: i32,
	pub upper: i32,
	pub name: String,
}

impl State {
	fn of(levels: &[Level]) -> Vec<State> {
		levels
			.iter()
			.map(|l| State {
				lower: l.lower(),
				upper: l.upper(),
				name: l.name().to_string(),
			})
			.collect()
	}
}

/// `[stalk]`, resolved: the three fields as plan channels, each with its states in the
/// project's order, and the states that mean something by their place in those lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stalk {
	pub rocker: u16,
	pub switch: u16,
	pub cruise: u16,
	pub rocker_states: Vec<State>,
	pub switch_states: Vec<State>,
	pub cruise_states: Vec<State>,
	pub next: u16,
	pub previous: u16,
	pub measure: u16,
	pub switch_off: u16,
	pub cruise_off: u16,
}

/// `[stopwatch]`, resolved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Stopwatch {
	/// The speed channel, by plan index.
	pub speed: u16,
	pub km_h_per_unit: f64,
	pub marks: Vec<u16>,
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
	/// In priority order: the channels' order, a channel's bounds before its drift.
	pub alarms: Vec<Alarm>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stalk: Option<Stalk>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stopwatch: Option<Stopwatch>,
	/// In the file's order.
	#[serde(default, skip_serializing_if = "Vec::is_empty")]
	pub buttons: Vec<Button>,
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub strip: Option<Strip>,
	/// Where the plan learns that the car stands.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub standstill: Option<Standstill>,
}

/// Where "the car stands" comes from, resolved: one fact of the plan, not of the stopwatch
/// (`todo/dash/26`, archived). Spelled `{"kind": "speed", "channel": N}` in `plan.json`, so a
/// later source is another `kind` and a file of today reads unchanged.
///
/// `dash.toml` names no source of its own today: the plan takes `[stopwatch]`'s `speed`. A
/// later source — a unit's own standstill state, which needs the car first — is an optional
/// `[standstill]` table, and a file without one keeps the stopwatch's speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Standstill {
	/// A speed channel, by plan index: the car stands while it reads exactly 0.
	Speed { channel: u16 },
}

impl Standstill {
	/// The channel the fact is read from.
	pub fn channel(&self) -> u16 {
		match *self {
			Standstill::Speed { channel } => channel,
		}
	}
}

/// The joins the board indexes by, resolved from what `plan.json` spells for a person
/// (`todo/dash/27`, archived): each channel's unit as its index into the plan's units, each
/// page's strip voltage, each chart page's history slot. [`Plan::to_device`] and [`to_rust`]
/// both write these, so the replay and the board index the same thing.
struct Joins {
	units: Vec<u8>,
	volts: Vec<Option<u16>>,
	slots: Vec<Option<u8>>,
}

impl Plan {
	/// The plan, with its [`plan_id`] first as `"id"`: sixteen hex digits, as the board says it.
	/// Written for a person; nothing reads `plan.json` back.
	pub fn to_json(&self) -> String {
		#[derive(Serialize)]
		struct Identified<'a> {
			id: String,
			#[serde(flatten)]
			plan: &'a Plan,
		}
		let identified = Identified {
			id: format!("{:016x}", plan_id(self)),
			plan: self,
		};
		serde_json::to_string_pretty(&identified).expect("a plan serialises")
	}

	/// The joins the board indexes by ([`Joins`]). `Err` where a channel's unit is not among the
	/// plan's units, or there are more units than an index of the board holds — a plan [`build`]
	/// does not make, since it lists every channel's unit.
	fn joins(&self) -> Result<Joins, String> {
		let units = self
			.channels
			.iter()
			.map(|c| {
				let at = self.units.iter().position(|u| u.request == c.unit).ok_or_else(|| {
					format!(
						"channel {:03X}:{:04X} is read from unit {:03X}, which its units do not list",
						c.unit, c.did, c.unit
					)
				})?;
				u8::try_from(at).map_err(|_| format!("{} units, more than the board's {} by index", self.units.len(), u8::MAX))
			})
			.collect::<Result<Vec<u8>, String>>()?;
		// The strip shows the voltage where the page lets it and does not show the channel itself
		// already, as a cell or as its chart: the same number twice on one page says nothing the
		// once does not.
		let volts = self
			.pages
			.iter()
			.map(|page| self.strip.filter(|s| page.volts() && !page.shows(s.volts)).map(|s| s.volts))
			.collect();
		if let Some(standstill) = self.standstill
			&& usize::from(standstill.channel()) >= self.channels.len()
		{
			return Err(format!(
				"its standstill is read from channel {}, which it does not have",
				standstill.channel()
			));
		}
		let mut charts = 0u8;
		let slots = self
			.pages
			.iter()
			.map(|page| match page {
				Page::Chart { .. } => {
					charts += 1;
					Some(charts - 1)
				}
				Page::Values { .. } => None,
			})
			.collect();
		Ok(Joins { units, volts, slots })
	}

	/// The plan as the board holds it — what [`to_rust`] writes as source, built in memory
	/// for a host program that runs the board's own code over it (`vagcan dev recording
	/// dash`). Numbers are narrowed to `f32` exactly as the source spells them.
	///
	/// **It leaks.** The board's plan is `&'static` throughout because on the board it is a
	/// `static`; a host process builds one per run and keeps it to the end, so the few
	/// kilobytes are the process's for its lifetime either way.
	///
	/// # Panics
	///
	/// On a plan whose joins do not hold ([`Plan::joins`]): [`build`] makes none.
	pub fn to_device(&self) -> vag_dash_render::plan::Plan {
		use vag_dash_render::alarm::{self as device_alarm, ChannelId, PageId};
		use vag_dash_render::plan as device;
		let joins = self.joins().expect("a plan from `build` joins");
		fn text(s: &str) -> &'static str {
			String::leak(s.to_string())
		}
		fn bound(b: &Option<Bound>) -> Option<device_alarm::Bound> {
			b.map(|b| device_alarm::Bound {
				at: b.at as f32,
				off: b.off as f32,
			})
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
			.zip(&joins.units)
			.map(|(c, &unit)| device::Channel {
				unit,
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
				zero_at_rest: c.zero_at_rest,
			})
			.collect();
		let pages = self
			.pages
			.iter()
			.zip(joins.volts.iter().zip(&joins.slots))
			.map(|(p, (&volts, &slot))| match p {
				Page::Chart {
					title, channel, min, max, ..
				} => device::Page::Chart {
					title: text(title),
					channel: *channel,
					slot: slot.expect("a chart page has a slot"),
					min: *min as f32,
					max: *max as f32,
					volts,
				},
				Page::Values { title, cells, .. } => device::Page::Values {
					title: text(title),
					cells: Vec::leak(cells.clone()),
					volts,
				},
			})
			.collect();
		let alarms = self
			.alarms
			.iter()
			.map(|a| device_alarm::Alarm {
				channel: ChannelId(a.channel),
				page: PageId(a.page),
				rule: match &a.rule {
					AlarmRule::Bounds { max, min } => device_alarm::Rule::Bounds {
						max: bound(max),
						min: bound(min),
					},
					AlarmRule::Drift {
						specified,
						percent,
						off_percent,
						hold_ms,
						min_setpoint,
					} => device_alarm::Rule::Drift {
						specified: ChannelId(*specified),
						percent: *percent as f32,
						off_percent: *off_percent as f32,
						hold_ms: *hold_ms,
						min_setpoint: *min_setpoint as f32,
					},
				},
			})
			.collect();
		fn bands(states: &[State]) -> &'static [device::Band] {
			Vec::leak(
				states
					.iter()
					.map(|s| device::Band {
						lower: s.lower,
						upper: s.upper,
					})
					.collect(),
			)
		}
		use vag_dash_render::stalk::{StateIndex, States};
		let stalk = self.stalk.as_ref().map(|s| device::StalkPlan {
			rocker: s.rocker,
			switch: s.switch,
			cruise: s.cruise,
			rocker_states: bands(&s.rocker_states),
			switch_states: bands(&s.switch_states),
			cruise_states: bands(&s.cruise_states),
			states: States {
				next: StateIndex(s.next),
				previous: StateIndex(s.previous),
				measure: StateIndex(s.measure),
				switch_off: StateIndex(s.switch_off),
				cruise_off: StateIndex(s.cruise_off),
			},
		});
		let stopwatch = self.stopwatch.as_ref().map(|s| device::StopwatchPlan {
			speed: s.speed,
			km_h_per_unit: s.km_h_per_unit as f32,
			marks: Vec::leak(s.marks.clone()),
		});
		device::Plan {
			vin: text(&self.vin),
			language: text(&self.language),
			units: Vec::leak(units),
			channels: Vec::leak(channels),
			pages: Vec::leak(pages),
			alarms: Vec::leak(alarms),
			stalk,
			stopwatch,
			buttons: Vec::leak(
				self
					.buttons
					.iter()
					.map(|b| device::ButtonPlan {
						pin: b.pin,
						action: b.action,
					})
					.collect(),
			),
			standstill: self.standstill.map(|s| match s {
				Standstill::Speed { channel } => device::Standstill::Speed(channel),
			}),
		}
	}
}

/// The plan and the build log that explains every row of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
	pub plan: Plan,
	/// Every line of the log, in the order the build decided it: what a firmware build prints
	/// as its warnings, and `dev dash build` at `-v`.
	pub notes: Vec<String>,
	/// The lines of `notes` that say what the input lacks and what to write — a page nothing
	/// opens, a strip on no page: what `dev dash build` says by default.
	pub advice: Vec<String>,
	/// How each channel resolved, one row a channel in the order `notes` logs them: what
	/// `dev dash build -v` prints as a table.
	pub channels: Vec<ChannelRow>,
	/// The bus estimate, one row a run: what `dev dash build -v` prints as a table.
	pub bus: Vec<BusRow>,
	/// What the bus would serve slower than the board asks, in plan order — a row of
	/// `dev dash build`'s own block when there is any.
	pub over_budget: Vec<OverBudget>,
	/// The lines of `notes` that `channels` and `bus` hold as rows: the firmware build prints
	/// them as lines, `dev dash build -v` as the tables.
	pub tabled: Vec<String>,
}

impl Built {
	/// The lines of the log that are neither advice nor a table's row, in order: what
	/// `dev dash build -v` prints as lines.
	pub fn prose(&self) -> impl Iterator<Item = &String> {
		self.notes.iter().filter(|n| !self.advice.contains(n) && !self.tabled.contains(n))
	}
}

/// One channel as the build resolved it: a row of `dev dash build -v`'s channel table, and a
/// line of the firmware build's log.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelRow {
	pub label: String,
	/// As the input wrote it: `01:IDE00191`.
	pub reference: String,
	pub did: u16,
	pub bit_offset: u32,
	pub bit_length: u32,
	pub signed: bool,
	pub big_endian: bool,
	pub factor: f64,
	pub offset: f64,
	pub hz: f64,
	/// Measured on a car, rather than declared by the project's data.
	pub proven: bool,
	/// The row's own name: its text id, else the definition's name.
	pub source: String,
}

impl ChannelRow {
	/// The table's header, the cells' order.
	pub const HEADERS: [&'static str; 8] = ["label", "channel", "did@bit/width", "type", "scale", "offset", "hz", "source"];

	/// The cells under [`ChannelRow::HEADERS`].
	pub fn cells(&self) -> Vec<String> {
		vec![
			self.label.clone(),
			self.reference.clone(),
			format!("{:04X}@{}/{}", self.did, self.bit_offset, self.bit_length),
			format!("{} {}", if self.signed { "i" } else { "u" }, if self.big_endian { "BE" } else { "LE" }),
			self.factor.to_string(),
			self.offset.to_string(),
			self.hz.to_string(),
			format!("{} {}", if self.proven { "measured" } else { "declared" }, self.source),
		]
	}

	/// The row as one line of the log: `BOOST ← 01:IDE00191 202A@0/16 u BE ×0.001 +0 at 2 Hz, …`.
	pub fn line(&self) -> String {
		format!(
			"{} ← {} {:04X}@{}/{} {} {}{} ×{} {:+} at {} Hz, {} ({})",
			self.label,
			self.reference,
			self.did,
			self.bit_offset,
			self.bit_length,
			if self.signed { "i" } else { "u" },
			if self.big_endian { "BE" } else { "LE" },
			if self.bit_length % 8 == 0 { "" } else { " bits" },
			self.factor,
			self.offset,
			self.hz,
			if self.proven { "measured on a car" } else { "from the project's data" },
			self.source
		)
	}
}

/// One run of the bus estimate: what is on the glass, its share of the bus, and the reads it
/// would serve slower than their rate.
#[derive(Debug, Clone, PartialEq)]
pub struct BusRow {
	/// `1 "MAIN"`, `1 "MAIN" cruise on`, `stopwatch`.
	pub page: String,
	/// `~88 %`.
	pub load: String,
	/// `01:202A BOOST 8.3/s of 10`, empty when the bus serves every read at its rate.
	pub short_of: String,
}

impl BusRow {
	/// The table's header, the cells' order.
	pub const HEADERS: [&'static str; 3] = ["page", "load", "short-of"];

	/// The cells under [`BusRow::HEADERS`].
	pub fn cells(&self) -> Vec<String> {
		vec![self.page.clone(), self.load.clone(), self.short_of.clone()]
	}
}

/// What the bus estimate finds short: a read the board wants at its rate and would not get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverBudget {
	/// A plan page, numbered from 1 as the input's `page #n`.
	Page(usize),
	/// A stopwatch run.
	Stopwatch,
}

/// `page 1, 3, stopwatch` — the over-budget row's value.
pub fn over_budget_text(over: &[OverBudget]) -> String {
	let pages: Vec<String> = over
		.iter()
		.filter_map(|o| match o {
			OverBudget::Page(n) => Some(n.to_string()),
			OverBudget::Stopwatch => None,
		})
		.collect();
	let mut parts = Vec::new();
	if !pages.is_empty() {
		parts.push(format!("page {}", pages.join(", ")));
	}
	if over.contains(&OverBudget::Stopwatch) {
		parts.push("stopwatch".to_string());
	}
	parts.join(", ")
}

/// What one channel's resolution logs: its lines, and its row of the channel table.
#[derive(Debug, Default)]
struct Resolution {
	notes: Vec<String>,
	rows: Vec<ChannelRow>,
}

impl Resolution {
	/// Into the build's log, once the channel is in the plan.
	fn into_log(self, notes: &mut Vec<String>, rows: &mut Vec<ChannelRow>) {
		notes.extend(self.notes);
		rows.extend(self.rows);
	}
}

/// One line of the log that is also advice: what the input lacks and what to write.
fn advise(notes: &mut Vec<String>, advice: &mut Vec<String>, text: String) {
	advice.push(text.clone());
	notes.push(text);
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

/// What a page cell, the strip's voltage or the stopwatch's speed names, among the plan's channels.
enum Named {
	/// A `[[channel]]`, by plan index.
	Channel(u16),
	/// A setpoint with no `[[channel]]` of its own: read with its channel, never offered as a
	/// cell (`todo/dash/18`, owner 2026-09-27).
	Setpoint,
	/// Nothing the input declares.
	Unknown,
}

impl Named {
	/// What a refusal says of a [`Named::Setpoint`].
	fn setpoint_refusal(reference: &Reference) -> String {
		format!("{reference} is a setpoint with no [[channel]] of its own — declare it as a [[channel]] to show it")
	}
}

/// The plan index of a channel the input names, by what the name resolves to: a page cell may
/// spell a row differently from its `[[channel]]` and still mean it.
///
/// Only the first `declared` channels are the input's `[[channel]]`s; after them come the
/// setpoints the build added, and a name that lands on one of those is [`Named::Setpoint`], in
/// either spelling.
fn index_by_row(
	reference: &Reference,
	channels: &[Channel],
	declared: usize,
	index_of: &BTreeMap<Reference, u16>,
	offered: &[poll::Channel],
	units: &[UnitIdentity],
) -> Named {
	let named = |at: usize| match at {
		at if at < declared => Named::Channel(at as u16),
		at if channels[..declared].iter().any(|c| c.setpoint == Some(at as u16)) => Named::Setpoint,
		_ => Named::Unknown,
	};
	if let Some(index) = index_of.get(reference) {
		return named(usize::from(*index));
	}
	let probe = ChannelInput::bare(reference.clone());
	let Ok(resolved) = resolve_channel(&probe, offered, units, &mut Resolution::default()) else {
		return Named::Unknown;
	};
	match channels.iter().position(|c| read_of(c) == read_of(&resolved)) {
		Some(at) => named(at),
		None => Named::Unknown,
	}
}

/// One `[[channel]]` against what the car reported and what the project knows: the same rules
/// for a channel the owner named and for a specified value it paired with (`todo/dash/18`).
fn resolve_channel(wanted: &ChannelInput, offered: &[poll::Channel], units: &[UnitIdentity], log: &mut Resolution) -> Result<Channel, Error> {
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
	if crate::verbosity::at(crate::verbosity::WIRE) {
		let on_unit = offered.iter().filter(|c| c.request == request && c.def.is_some()).count();
		crate::verbosity::say(crate::verbosity::WIRE, &candidate_lookup(&wanted.reference, &matches, on_unit));
	}
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
		([], []) => return Err(no_such_channel(&wanted.reference, offered)),
		([], [first, ..]) => {
			let kind = match first.def.as_ref().map(|d| &d.scaling) {
				Some(Scaling::Enum { .. }) => "an enumeration",
				Some(Scaling::Anchor { .. }) => "a single proven point with no slope",
				_ => "not a quantity",
			};
			return Err(Error::NotLinear(wanted.reference.clone(), kind.to_string()));
		}
		(many, _) => return Err(Error::Ambiguous(wanted.reference.clone(), many.iter().map(|c| row_name(c)).collect())),
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
	// Finite as the board holds it too: a factor past an `f32` reaches the generated source as
	// an `inf` the firmware's build cannot read, and one an `f32` rounds to 0 makes a channel
	// that only ever shows its offset — a speed that never reads as moving.
	if !fits_f32(factor) || !fits_f32(offset) || (factor != 0.0 && factor as f32 == 0.0) {
		return Err(Error::NotFinite(wanted.reference.clone()));
	}
	// What the catalog declares is one thing; what this car answers is another,
	// and the record of its units says nothing about identifiers. A standard
	// OBD-II row is the one case worth a word: the standard mandates it, and
	// this car may still not carry it — nobody's description of this car says.
	//
	// The standard's row is asked of the row itself (`Channel::is_standard`),
	// not inferred from a missing text id: a VCDS row gives up an id ODIS gives
	// another field, and one from an install whose text table is shut carries
	// none.
	if found.is_standard() {
		log.notes.push(format!(
			"{}: a standard OBD-II row, which the standard mandates and this car may still not carry",
			wanted.reference
		));
	}
	let (bit_offset, bit_length, signed, big_endian) = bits_of(def.raw_form);
	let label = wanted.label.clone().unwrap_or_else(|| found.label());
	let decimals = wanted.decimals.unwrap_or_else(|| decimals_for(factor));
	let source = found.text_id.clone().unwrap_or_else(|| def.name.to_string());
	let hz = wanted.hz.unwrap_or(DEFAULT_HZ);
	let row = ChannelRow {
		label: label.clone(),
		reference: wanted.reference.to_string(),
		did,
		bit_offset,
		bit_length,
		signed,
		big_endian,
		factor,
		offset,
		hz,
		proven: found.proven,
		source: source.clone(),
	};
	log.notes.push(row.line());
	log.rows.push(row);
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
		zero_at_rest: wanted.zero_at_rest,
	})
}

/// Why nothing on a recorded unit answers to `reference`, in the three ways a person gets there
/// (UX review, 2026-10-19): the unit has no channels in the project at all; a channel's name was
/// written where its id goes; or the id is simply not this car's.
fn no_such_channel(reference: &Reference, offered: &[poll::Channel]) -> Error {
	let request = reference.request();
	let on_unit: Vec<&poll::Channel> = offered.iter().filter(|c| c.request == request && c.def.is_some()).collect();
	if on_unit.is_empty() {
		return Error::NoChannels(reference.clone());
	}
	let Reference::TextId { text_id, .. } = reference else {
		return Error::Undeclared(reference.clone());
	};
	let wanted = text_id.trim().to_lowercase();
	let named: Vec<&poll::Channel> = on_unit
		.iter()
		.copied()
		.filter(|c| c.label().trim().to_lowercase() == wanted || c.def.as_ref().is_some_and(|d| d.name.trim().to_lowercase() == wanted))
		.collect();
	// No text id of any project carries a space; a name nearly always does.
	if named.is_empty() && !text_id.contains(char::is_whitespace) {
		return Error::Undeclared(reference.clone());
	}
	// Only what a numeric cell takes is worth suggesting, and only in a spelling the build reads
	// as that row: its text id where that is the one linear row carrying it — the rule of
	// [`resolve_channel`] — its identifier and bit where that is, and nothing where neither is.
	let is_linear = |c: &poll::Channel| matches!(c.def.as_ref().map(|d| &d.scaling), Some(Scaling::Linear(_)));
	let bit_of = |c: &poll::Channel| c.def.as_ref().map_or(0, |d| d.raw_form.bit_offset());
	let linear: Vec<&poll::Channel> = on_unit.iter().copied().filter(|c| is_linear(c)).collect();
	let mut ids: Vec<String> = Vec::new();
	for c in named.iter().copied().filter(|c| is_linear(c)) {
		let by_text = c
			.text_id
			.as_ref()
			.filter(|id| linear.iter().filter(|o| o.text_id.as_ref() == Some(*id)).count() == 1);
		let by_field = linear.iter().filter(|o| o.did == c.did && bit_of(o) == bit_of(c)).count() == 1;
		let spelled = match (by_text, by_field) {
			(Some(id), _) => Reference::TextId {
				request,
				text_id: id.clone(),
			},
			(None, true) => Reference::Field {
				request,
				did: c.did,
				bit_offset: bit_of(c),
			},
			(None, false) => continue,
		}
		.to_string();
		if !ids.contains(&spelled) {
			ids.push(spelled);
		}
	}
	Error::NameNotId {
		reference: reference.clone(),
		not_linear: !named.is_empty() && !named.iter().any(|c| is_linear(c)),
		ids,
	}
}

/// `-vv`: what the catalogs offered each recorded unit, looked up by what the unit said
/// about itself — one line a unit.
fn catalog_lookups(offered: &[poll::Channel], units: &[UnitIdentity]) -> Vec<String> {
	let or_none = |text: &Option<String>| text.as_deref().filter(|t| !t.is_empty()).unwrap_or("-").to_string();
	units
		.iter()
		.map(|unit| {
			let rows: Vec<&poll::Channel> = offered.iter().filter(|c| c.request == unit.request && c.def.is_some()).collect();
			let measured = rows.iter().filter(|c| c.proven).count();
			format!(
				"catalog {}: part {}, odx {} {} → {}, {measured} measured on a car",
				unit_label(unit.request),
				or_none(&unit.part_number),
				or_none(&unit.odx_name),
				or_none(&unit.odx_version),
				count(rows.len(), "row"),
			)
		})
		.collect()
}

/// `-vv`: the rows a reference was looked up among on its unit, and how each scales — what
/// [`resolve_channel`] chose from.
fn candidate_lookup(reference: &Reference, candidates: &[&poll::Channel], on_unit: usize) -> String {
	let rows: Vec<String> = candidates
		.iter()
		.map(|c| {
			let kind = match c.def.as_ref().map(|d| &d.scaling) {
				Some(Scaling::Linear(_)) => "linear",
				Some(Scaling::Enum { .. }) => "enum",
				Some(Scaling::Anchor { .. }) => "anchor",
				None => "raw",
			};
			let source = if c.proven { "measured" } else { "declared" };
			format!("{} {kind} {source}", row_name(c))
		})
		.collect();
	match rows.as_slice() {
		[] => format!("lookup {reference}: none of the {} on its unit", count(on_unit, "row")),
		_ => format!("lookup {reference}: {} of {} — {}", rows.len(), count(on_unit, "row"), rows.join("; ")),
	}
}

/// A count and its noun: `1 row`, `2 rows`.
fn count(n: usize, singular: &str) -> String {
	match n {
		1 => format!("1 {singular}"),
		n => format!("{n} {singular}s"),
	}
}

/// A unit as the plan's notes write it: its short number, else its request id.
fn unit_label(request: u16) -> String {
	UnitAddress::from_request(request)
		.map(|a| a.label())
		.unwrap_or_else(|| format!("{request:03X}"))
}

/// One row as an [`Error::Ambiguous`] lists it: what to write instead — its identifier and bit
/// offset — and its name.
fn row_name(c: &poll::Channel) -> String {
	format!("{:04X}@{} {}", c.did, c.def.as_ref().map_or(0, |d| d.raw_form.bit_offset()), c.label())
}

/// Resolve an input against what the car reported and what the project knows.
///
/// `units` are the car's own words about itself (from its record); `store` and
/// `extracted` are the project — the same two `watch` opens. Pure: reads
/// nothing but its arguments, writes nothing — at `-vv` it says on stderr what each unit's
/// catalog held and which rows each name was chosen from, the lookups a failed build needs
/// and the log it returns would lose with it.
///
/// **One language for the plan** (owner, 2026-09-27): the input's `language`, or
/// `default_language` — `config.toml`'s — without one. It is the board's own words and the
/// glossary column every label is taken from, whichever language `extracted` was opened in.
pub fn build(input: &Input, store: &CatalogStore, extracted: &Extracted, units: &[UnitIdentity], default_language: Language) -> Result<Built, Error> {
	let language = input.language.unwrap_or(default_language);
	// A copy speaking the plan's language, only when the caller's does not: the caller's own
	// stays as it was, for what it names in `config.toml`'s (`vagcan dev recording dash`
	// matches a recording's headings, written by `watch`, against it).
	let respoken;
	let extracted = if extracted.language() == language {
		extracted
	} else {
		respoken = extracted.clone().in_language(language);
		&respoken
	};
	let offered = poll::available(store, extracted, units);
	if crate::verbosity::at(crate::verbosity::WIRE) {
		for line in catalog_lookups(&offered, units) {
			crate::verbosity::say(crate::verbosity::WIRE, &line);
		}
	}
	let mut notes = Vec::new();
	let mut advice = Vec::new();
	let mut rows = Vec::new();
	let mut channels: Vec<Channel> = Vec::new();
	let mut index_of: BTreeMap<Reference, u16> = BTreeMap::new();

	for wanted in &input.channels {
		if index_of.contains_key(&wanted.reference) {
			return Err(Error::Duplicate(wanted.reference.clone()));
		}
		let mut resolution = Resolution::default();
		let resolved = resolve_channel(wanted, &offered, units, &mut resolution)?;
		// The same row under its other spelling is the same row: `01:IDE00191` and `01:202A`
		// would otherwise both be added, both subscribed and both drawable (review,
		// 2026-09-15).
		if let Some(at) = channels.iter().position(|c| read_of(c) == read_of(&resolved)) {
			return Err(Error::SameRow {
				first: input.channels[at].reference.clone(),
				second: wanted.reference.clone(),
			});
		}
		resolution.into_log(&mut notes, &mut rows);
		let index = channels.len() as u16;
		channels.push(resolved);
		index_of.insert(wanted.reference.clone(), index);
	}
	// The owner's `[[channel]]`s, one plan channel each and in order; everything after them the
	// build adds, and no page, strip or stopwatch may name.
	let declared = channels.len();

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
			decimals: wanted.decimals,
			hz: wanted.hz,
			..ChannelInput::bare(reference.clone())
		};
		// Resolved into a log of its own: a setpoint that turns out to be a channel the input
		// already has is not added, and a build log saying it was would be a lie.
		let mut resolution = Resolution::default();
		let resolved = match index_of.get(&reference) {
			// Already a `[[channel]]`: the owner's own, with the rate they gave it.
			Some(index) => channels[*index as usize].clone(),
			None => resolve_channel(&hidden, &offered, units, &mut resolution)?,
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
				resolution.into_log(&mut notes, &mut rows);
				index
			}
		};
		channels[i].setpoint = Some(index);
		notes.push(format!("{}: its specified value is {reference}", wanted.reference));
	}

	// The stopwatch's speed is one of the owner's `[[channel]]`s, resolved above.
	let stopwatch = match &input.stopwatch {
		None => None,
		Some(wanted) => {
			let speed = index_by_row(&wanted.speed, &channels, declared, &index_of, &offered, units);
			Some(resolve_stopwatch(wanted, speed, &channels, &mut notes, &mut advice)?)
		}
	};
	// Whether the car stands is one fact of the plan (`todo/dash/26`, archived), and its one
	// source today is the stopwatch's speed reading 0 — a later one, a unit's own standstill
	// state, would be a `[standstill]` table, and needs the car first.
	let standstill = stopwatch.as_ref().map(|s| Standstill::Speed { channel: s.speed });
	// `zero_at_rest` asks the plan whether the car stands: with no source there is nothing to
	// ask, and the flag would silently do nothing.
	if standstill.is_none()
		&& let Some(wanted) = input.channels.iter().find(|c| c.zero_at_rest)
	{
		return Err(Error::Parse(format!(
			"dash.toml: {}: zero_at_rest needs a source for whether the car stands — today a [stopwatch], whose speed reading 0 says it",
			wanted.reference
		)));
	}
	// The strip's voltage is one of the owner's `[[channel]]`s too: its rate and its row are
	// theirs to choose, as the stopwatch's speed is.
	let strip = match &input.strip {
		None => None,
		Some(wanted) => {
			let volts = match index_by_row(&wanted.volts, &channels, declared, &index_of, &offered, units) {
				Named::Channel(index) => index,
				Named::Setpoint => return Err(Error::Strip(format!("volts {}", Named::setpoint_refusal(&wanted.volts)))),
				Named::Unknown => return Err(Error::Strip(format!("volts {} is not in the [[channel]] list", wanted.volts))),
			};
			Some(Strip { volts })
		}
	};
	// A page's `volts` says whether the strip shows the voltage there: without a `[strip]` there
	// is no voltage, and the key would silently do nothing.
	if strip.is_none()
		&& let Some(at) = input.pages.iter().position(|p| p.volts().is_some())
	{
		return Err(Error::Parse(format!(
			"dash.toml: page #{}: volts needs a [strip] — its volts is the channel the voltage is read from",
			at + 1
		)));
	}
	// The lever's fields are enumerations, which no `[[channel]]` can be: they join the plan
	// as channels of their own, on no page, read only for the lever.
	let stalk = match &input.stalk {
		None => None,
		Some(wanted) => Some(resolve_stalk(
			wanted,
			&mut channels,
			&offered,
			units,
			Sources { store, extracted },
			&mut notes,
		)?),
	};
	// Nothing about a button is the car's: it reaches the plan as written.
	for b in &input.buttons {
		notes.push(format!("button: pin {} → {}", b.pin, b.action.name()));
		if b.action == Command::Stopwatch && stopwatch.is_none() {
			advise(
				&mut notes,
				&mut advice,
				format!(
					"button: pin {} is a stopwatch button, and there is no [stopwatch] — a press of it only silences an alarm",
					b.pin
				),
			);
		}
	}
	let opens = stalk.is_some() || input.buttons.iter().any(|b| b.action == Command::Stopwatch);
	if stopwatch.is_some() && !opens {
		advise(
			&mut notes,
			&mut advice,
			"stopwatch: nothing opens the page — give [stalk] a measure, or a [[button]] action = \"stopwatch\"".to_string(),
		);
	}
	if stalk.is_some() && stopwatch.is_none() {
		advise(
			&mut notes,
			&mut advice,
			"stalk: there is no [stopwatch] — a press of measure only silences an alarm".to_string(),
		);
	}
	// BOOT and RESET are no input (owner, 2026-09-27), so a board with none of its own turns its
	// page from a bench tool alone, and an alarm stays until its channel ends it — a channel gone
	// silent mid-episode holds it on the glass (`screen.rs`,
	// `a_stale_channel_neither_trips_nor_releases`).
	let paged = stalk.is_some() || input.buttons.iter().any(|b| b.action != Command::Stopwatch);
	if input.pages.len() > 1 && !paged {
		let why = "only dashsim or dashcfg's set page turns the page";
		advise(
			&mut notes,
			&mut advice,
			format!("input: no [stalk] and no [[button]] with next or previous — {why}"),
		);
	}
	let alarmed = input
		.channels
		.iter()
		.any(|c| c.alarm_max.is_some() || c.alarm_min.is_some() || c.drift.is_some());
	if alarmed && stalk.is_none() && input.buttons.is_empty() {
		let why = "an alarm stays up until its channel answers in range again; only dashsim silences it";
		advise(&mut notes, &mut advice, format!("input: no [stalk] and no [[button]] — {why}"));
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
	// past the board's is one an alarm could raise and the glass could never show.
	if input.pages.len() > MAX_PAGES {
		return Err(Error::TooManyPages(input.pages.len()));
	}
	let mut pages = Vec::new();
	for (i, page) in input.pages.iter().enumerate() {
		let n = i + 1;
		let index = |r: &Reference| match index_by_row(r, &channels, declared, &index_of, &offered, units) {
			Named::Channel(index) => Ok(index),
			Named::Setpoint => Err(Error::Page(n, Named::setpoint_refusal(r))),
			Named::Unknown => Err(Error::PageRefersToUnknown {
				page: n,
				reference: r.clone(),
			}),
		};
		match page {
			PageInput::Values { title, cells, volts } => {
				if cells.is_empty() || cells.len() > MAX_VALUE_CELLS {
					return Err(Error::Page(
						n,
						format!("a values page holds 1 to {MAX_VALUE_CELLS} cells, not {}", cells.len()),
					));
				}
				let cells = cells.iter().map(index).collect::<Result<Vec<_>, _>>()?;
				pages.push(Page::Values {
					title: title.clone(),
					cells,
					volts: volts.unwrap_or(true),
				});
			}
			PageInput::Chart {
				title,
				cell,
				min,
				max,
				volts,
			} => {
				// Compared as the board draws them, in `f32`: two ends a hair apart in the file can
				// be one value there, and the board draws no trace for a scale with no height. A
				// scale whose height overflows one draws every value on the floor.
				let (low, high) = (*min as f32, *max as f32);
				if low.partial_cmp(&high) != Some(std::cmp::Ordering::Less) {
					return Err(Error::Page(n, format!("min {low} is not below max {high}")));
				}
				if !(high - low).is_finite() {
					return Err(Error::Page(
						n,
						format!("min {low:e} and max {high:e} are further apart than the board's 32-bit float holds"),
					));
				}
				let channel = index(cell)?;
				// The device finds a chart's range by its channel, so a second
				// chart of the same channel could only ever draw the first's.
				if pages.iter().any(|p| matches!(p, Page::Chart { channel: c, .. } if *c == channel)) {
					return Err(Error::Page(n, format!("{cell} already has a chart page; one range per channel")));
				}
				pages.push(Page::Chart {
					title: title.clone(),
					channel,
					min: *min,
					max: *max,
					volts: volts.unwrap_or(true),
				});
			}
		}
	}
	if let (Some(strip), Some(wanted)) = (strip, &input.strip) {
		strip_notes(strip, &wanted.volts, &channels, &pages, &mut notes, &mut advice);
	}

	// Alarms last: a channel's alarm raises the first values page that shows it, else its chart,
	// and the pages are resolved by now.
	let alarms = resolve_alarms(input, &channels, &pages, &mut notes)?;
	let plan = Plan {
		vin: input.vin.clone(),
		language: language.code().to_string(),
		units: plan_units,
		channels,
		pages,
		alarms,
		stalk,
		stopwatch,
		buttons: input.buttons.clone(),
		strip,
		standstill,
	};
	// Every join the board indexes by resolves: `plan_units` lists every channel's unit, so only
	// a count past what an index holds could fail here.
	plan.joins().map_err(Error::Parse)?;
	// Bus time, the board's planner run over the plan at one constant cost an exchange: an
	// estimate, said and never refused (2026-10-20: the plan it refused runs on the car).
	let bus = bus_log(&plan, &input.channels);
	notes.push(bus.header);
	notes.extend(bus.lines.iter().cloned());
	let tabled = rows.iter().map(ChannelRow::line).chain(bus.lines).collect();
	Ok(Built {
		plan,
		notes,
		advice,
		channels: rows,
		bus: bus.rows,
		over_budget: bus.over_budget,
		tabled,
	})
}

/// Every `[[channel]]`'s alarms, in priority order — `alarm_priority` ascending, then the file's
/// order; a channel's bounds before its drift — each raising the first values page that shows
/// the channel, else its chart page. `channels[i]` is `input`'s i-th.
fn resolve_alarms(input: &Input, channels: &[Channel], pages: &[Page], notes: &mut Vec<String>) -> Result<Vec<Alarm>, Error> {
	// Each rule with its channel's priority, for the sort at the end.
	let mut alarms: Vec<(Option<u32>, Alarm)> = Vec::new();
	for (i, wanted) in input.channels.iter().enumerate() {
		let first_key = match (&wanted.alarm_max, &wanted.alarm_min, &wanted.drift) {
			(Some(_), _, _) => "alarm_max",
			(None, Some(_), _) => "alarm_min",
			(None, None, Some(_)) => "drift_percent",
			(None, None, None) => continue,
		};
		let refuse = |why: String| Error::Alarm(wanted.reference.clone(), why);
		let index = i as u16;
		let channel = &channels[i];
		// The first values page with it as a cell, where its cell is inverted; else its chart page,
		// whose header is (`todo/dash/27`, archived).
		let values = pages
			.iter()
			.position(|p| matches!(p, Page::Values { cells, .. } if cells.contains(&index)));
		let chart = || pages.iter().position(|p| matches!(p, Page::Chart { channel, .. } if *channel == index));
		let Some(page) = values.or_else(chart) else {
			return Err(refuse(format!(
				"{first_key} needs a page that shows the channel — the alarm raises the first values page with it as a cell, else its chart page, and no [[page]] has it"
			)));
		};
		let raises = match &pages[page] {
			Page::Values { title, .. } | Page::Chart { title, .. } if !title.is_empty() => format!("page {} {title:?}", page + 1),
			_ => format!("page {}", page + 1),
		};
		let priority = wanted.alarm_priority.map_or_else(String::new, |n| format!(", priority {n}"));
		// One step of the channel's resolution, what one count of its raw value is worth: a
		// distance the unit cannot report is no distance (owner, 2026-10-19). A factor of 0 — a
		// value that never moves — has none, and the display's step stands in.
		let step = match channel.factor.abs() {
			f if f > 0.0 => f,
			_ => 10f64.powi(-i32::from(channel.decimals)),
		};
		let away = |at: f64| (DEFAULT_OFF_SHARE * at.abs()).max(step);
		// A default is rounded to the places the step has: the rounding moves it at most half a
		// step, so it stays at least half a step inside its bound, and the build's line and
		// `plan.json` read `1.4`, not `1.4000000000000001`. Twelve places is past any resolution
		// a unit reports and still exact in an f64.
		let places = (-step.log10()).ceil().clamp(0.0, 12.0) as i32;
		let scale = 10f64.powi(places);
		let shown = |v: f64| (v * scale).round() / scale;
		let max = wanted
			.alarm_max
			.map(|b| (b.at, b.off.unwrap_or_else(|| shown(b.at - away(b.at))), b.off.is_none()));
		let min = wanted
			.alarm_min
			.map(|b| (b.at, b.off.unwrap_or_else(|| shown(b.at + away(b.at))), b.off.is_none()));
		// Checked as the board holds them, in `f32`: a default the narrowing makes the bound
		// itself is no off value at all.
		let default_said = |derived: bool| if derived { " (its default)" } else { "" };
		if let Some((at, off, derived)) = max
			&& off as f32 >= at as f32
		{
			return Err(refuse(format!(
				"alarm_max_off {off}{} is alarm_max {at} on the board — write alarm_max_off under it",
				default_said(derived)
			)));
		}
		if let Some((at, off, derived)) = min
			&& off as f32 <= at as f32
		{
			return Err(refuse(format!(
				"alarm_min_off {off}{} is alarm_min {at} on the board — write alarm_min_off over it",
				default_said(derived)
			)));
		}
		if let (Some((_, max_off, max_derived)), Some((_, min_off, min_derived))) = (max, min)
			&& min_off as f32 >= max_off as f32
		{
			return Err(refuse(format!(
				"alarm_min_off {min_off}{} is not under alarm_max_off {max_off}{} — no value would be off for both; write the two so that alarm_min_off is under alarm_max_off",
				default_said(min_derived),
				default_said(max_derived)
			)));
		}
		if max.is_some() || min.is_some() {
			let mut said = Vec::new();
			if let Some((at, off, derived)) = max {
				said.push(format!("at or above {at}, off at or under {off}{}", default_said(derived)));
			}
			if let Some((at, off, derived)) = min {
				said.push(format!("at or below {at}, off at or above {off}{}", default_said(derived)));
			}
			notes.push(format!("alarm: {} {} → {raises}{priority}", wanted.reference, said.join("; ")));
			let bound = |b: Option<(f64, f64, bool)>| b.map(|(at, off, _)| Bound { at, off });
			alarms.push((
				wanted.alarm_priority,
				Alarm {
					channel: index,
					page: page as u16,
					rule: AlarmRule::Bounds {
						max: bound(max),
						min: bound(min),
					},
				},
			));
		}
		if let Some(drift) = wanted.drift {
			// The drift is measured against what the unit asked for: no pair, nothing to be far from.
			let Some(specified) = channel.setpoint else {
				return Err(refuse(
					"drift_percent needs a setpoint — the drift is measured against what the unit asked for".to_string(),
				));
			};
			notes.push(format!(
				"alarm: {} {}% or more from its specified value for {} ms, off at or under {}%, nothing said under {} → {raises}{priority}",
				wanted.reference, drift.percent, drift.hold_ms, drift.off_percent, drift.min_setpoint
			));
			alarms.push((
				wanted.alarm_priority,
				Alarm {
					channel: index,
					page: page as u16,
					rule: AlarmRule::Drift {
						specified,
						percent: drift.percent,
						off_percent: drift.off_percent,
						hold_ms: drift.hold_ms,
						min_setpoint: drift.min_setpoint,
					},
				},
			));
		}
	}
	// Stable: ties, and every channel without a priority, keep the file's order.
	alarms.sort_by_key(|(priority, _)| (priority.is_none(), *priority));
	Ok(alarms.into_iter().map(|(_, alarm)| alarm).collect())
}

/// Bus time, estimated by the board's own planner run over the plan (`dash/bus_time.rs`): a line
/// that says what the estimate is, then one per page on the glass — its share of the bus with
/// cruise off and, with a lever, on, each run with the reads it would serve slower than their
/// rate — and one for a stopwatch run.
///
/// **Said, never refused** (2026-10-20). The cost an exchange is an upper bound from a BLE
/// recording; at it every page of the owner's plan came out short, and that plan runs on the
/// car. A strict check waits for the board's own `exch_ms=` from a drive.
fn bus_log(plan: &Plan, inputs: &[ChannelInput]) -> BusLog {
	use bus_time::{EXCHANGE_MS, Glass};
	use vag_dash_render::plan::{LEVER_CLOSED_PERIOD_MS, LEVER_OPEN_PERIOD_MS};
	let device = plan.to_device();
	let percent = |share: f64| format!("~{:.0} %", share * 100.0);
	let shown = |v: f64| {
		let text = format!("{v:.1}");
		text.strip_suffix(".0").map(str::to_string).unwrap_or(text)
	};
	let what = |index: u16| -> String {
		let stalk = plan.stalk.as_ref();
		if stalk.is_some_and(|s| s.rocker == index) {
			return "the lever".to_string();
		}
		if stalk.is_some_and(|s| s.cruise == index) {
			return "the cruise status".to_string();
		}
		let channel = &plan.channels[usize::from(index)];
		match inputs.get(usize::from(index)) {
			Some(wanted) => format!("{} {}", wanted.reference, channel.label),
			None => format!("{} {}", channel.source, channel.label),
		}
	};

	let lever = match plan.stalk {
		Some(_) => format!(
			"; the lever is read {} times a second with cruise off, {} with it on",
			1000 / LEVER_OPEN_PERIOD_MS,
			1000 / LEVER_CLOSED_PERIOD_MS
		),
		None => String::new(),
	};
	let header =
		format!("bus: an estimate at {EXCHANGE_MS} ms an exchange, an upper bound from a BLE recording on the reference car; it refuses nothing{lever}");
	// The reads a run would serve slower than their rate: `the lever 18.9/s of 20`.
	let short = |run: &bus_time::Run| -> Vec<String> {
		run
			.thinned
			.iter()
			.map(|t| format!("{} {}/s of {}", what(t.channel), shown(t.got_per_s), shown(t.asked_per_s)))
			.collect()
	};
	// One run's share, the reads it would serve slower than their rate, and whether it thins the
	// pages behind the glass: `~88 % — reads the lever 18.9/s of 20`.
	let said = |run: &bus_time::Run| {
		let mut text = percent(run.busy);
		let reads = short(run);
		if !reads.is_empty() {
			text.push_str(&format!(" — reads {}", reads.join(", ")));
		}
		if run.behind_thinned {
			text.push_str(" (the other pages less often than once a second)");
		}
		text
	};
	// The same run as a row: `1 "MAIN"  ~88 %  the lever 18.9/s of 20`.
	let row = |page: String, run: &bus_time::Run| {
		let mut short_of = short(run).join(", ");
		if run.behind_thinned {
			if !short_of.is_empty() {
				short_of.push_str("; ");
			}
			short_of.push_str("other pages under 1/s");
		}
		BusRow {
			page,
			load: percent(run.busy),
			short_of,
		}
	};
	let mut log = BusLog {
		header,
		lines: Vec::new(),
		rows: Vec::new(),
		over_budget: Vec::new(),
	};
	for estimate in bus_time::estimate(&device, EXCHANGE_MS) {
		let off = &estimate.cruise_off;
		let (glass, page, over) = match off.glass {
			Glass::Page(page) => {
				let cell = match device.pages[page].title() {
					"" => format!("{}", page + 1),
					title => format!("{} {title:?}", page + 1),
				};
				(format!("page {cell}"), cell, OverBudget::Page(page + 1))
			}
			Glass::Run => ("a stopwatch run".to_string(), "stopwatch".to_string(), OverBudget::Stopwatch),
		};
		if std::iter::once(off).chain(&estimate.cruise_on).any(|run| !run.thinned.is_empty()) {
			log.over_budget.push(over);
		}
		match &estimate.cruise_on {
			Some(on) => {
				log.lines.push(format!("bus: {glass} cruise off {}; cruise on {}", said(off), said(on)));
				log.rows.push(row(format!("{page} cruise off"), off));
				log.rows.push(row(format!("{page} cruise on"), on));
			}
			None => {
				log.lines.push(format!("bus: {glass} {}", said(off)));
				log.rows.push(row(page, off));
			}
		}
	}
	log
}

/// The bus estimate as the build logs it: the line that says what it is, one line an estimate,
/// the same as one row a run, and what comes out short.
struct BusLog {
	header: String,
	lines: Vec<String>,
	rows: Vec<BusRow>,
	over_budget: Vec<OverBudget>,
}
/// What the build says of the strip: which pages show the voltage, and a channel that does not
/// read in volts, which the strip writes it as.
fn strip_notes(strip: Strip, reference: &Reference, channels: &[Channel], pages: &[Page], notes: &mut Vec<String>, advice: &mut Vec<String>) {
	let shown: Vec<String> = pages
		.iter()
		.enumerate()
		.filter(|(_, page)| page.volts() && !page.shows(strip.volts))
		.map(|(at, _)| (at + 1).to_string())
		.collect();
	match shown.as_slice() {
		[] => advise(
			notes,
			advice,
			format!("strip: volts {reference} is on no page — each has volts = false or shows it itself"),
		),
		[one] => notes.push(format!("strip: volts {reference} on page {one}")),
		many => notes.push(format!("strip: volts {reference} on pages {}", many.join(", "))),
	}
	let unit = &channels[usize::from(strip.volts)].unit_text;
	if unit != "V" {
		advise(
			notes,
			advice,
			format!("strip: volts {reference} reads in {unit:?}, and the strip writes it as V"),
		);
	}
}

impl Page {
	fn volts(&self) -> bool {
		match self {
			Page::Chart { volts, .. } | Page::Values { volts, .. } => *volts,
		}
	}

	/// Whether the page shows the channel at `index` itself, as a cell or as its chart.
	fn shows(&self, index: u16) -> bool {
		match self {
			Page::Chart { channel, .. } => *channel == index,
			Page::Values { cells, .. } => cells.contains(&index),
		}
	}
}

/// `[stopwatch]` against the resolved channels: the speed is a `[[channel]]` whose scaling
/// has no offset — the stopwatch takes the channel's zero for a standstill, and with an
/// offset the zero is somewhere else — and whose factor is above zero, or moving forward
/// would read as going nowhere. `speed` is what [`index_by_row`] made of `wanted.speed`.
fn resolve_stopwatch(
	wanted: &StopwatchInput,
	speed: Named,
	channels: &[Channel],
	notes: &mut Vec<String>,
	advice: &mut Vec<String>,
) -> Result<Stopwatch, Error> {
	let speed = match speed {
		Named::Channel(index) => index,
		Named::Setpoint => return Err(Error::Stopwatch(format!("speed {}", Named::setpoint_refusal(&wanted.speed)))),
		Named::Unknown => return Err(Error::Stopwatch(format!("speed {} is not in the [[channel]] list", wanted.speed))),
	};
	let channel = &channels[usize::from(speed)];
	if channel.offset != 0.0 {
		return Err(Error::Stopwatch(format!(
			"speed {}: its scaling has an offset ({:+}) — a standstill is the channel's zero, and with an offset there is none",
			wanted.speed, channel.offset
		)));
	}
	if channel.factor <= 0.0 {
		return Err(Error::Stopwatch(format!(
			"speed {}: its scaling's factor {} is not above zero — moving forward would never read as moving",
			wanted.speed, channel.factor
		)));
	}
	// The launch fit wants `MIN_FIT_SAMPLES` moving samples in its first `START_FIT_MS`, both
	// ends included, counted from the first moving one: read every `p` ms, that is the samples
	// at 0, p, 2p, …. The fit alone needs `MIN_FIT_SAMPLES - 1` periods in the window; one
	// answer late or lost costs a period, so `MIN_FIT_SAMPLES` of them have to fit. Read slower,
	// one late answer loses the launch, and the run has crossings and no time — which only the
	// car would show. Checked on the period the board polls at, not on `1000 / hz`: it rounds.
	// The bound is the stopwatch's own, which the firmware's silence assert counts too.
	let longest_ms = SLOWEST_SPEED_PERIOD_MS;
	let period_ms = board_period_ms(channel.hz);
	if u64::from(period_ms) > longest_ms {
		// The slowest rate the board polls every `longest_ms` or sooner, in tenths of a hertz,
		// rounded up: its rounding takes `1000 / hz` below `longest_ms + 0.5` to `longest_ms`.
		let slowest_hz = (10_000.0 / (longest_ms as f64 + 0.5)).ceil() / 10.0;
		return Err(Error::Stopwatch(format!(
			"speed {} is read every {period_ms} ms (hz = {}) — the launch fit needs {MIN_FIT_SAMPLES} samples in its first \
			 {START_FIT_MS} ms even when one answer is late, so a reading every {longest_ms} ms or sooner: give its [[channel]] \
			 an hz of {slowest_hz} or more; 50 is recommended",
			wanted.speed, channel.hz
		)));
	}
	let marks: Vec<String> = wanted.marks.iter().map(|m| format!("0-{m}")).collect();
	match wanted.km_h_per_unit {
		0.0 => advise(
			notes,
			advice,
			format!(
				"stopwatch: {} {}, km_h_per_unit not measured — the page says so and times nothing",
				wanted.speed,
				marks.join(", ")
			),
		),
		factor => notes.push(format!("stopwatch: {} × {factor} km/h, {}", wanted.speed, marks.join(", "))),
	}
	Ok(Stopwatch {
		speed,
		km_h_per_unit: wanted.km_h_per_unit,
		marks: wanted.marks.clone(),
	})
}

/// Every how many milliseconds the board reads a channel asked for at `hz`: the board's own
/// rule (`vag_dash_render::plan::Channel::period_ms`), on the `f32` the plan narrows `hz` to,
/// so a check on it is a check on what the board does.
fn board_period_ms(hz: f64) -> u32 {
	let channel = vag_dash_render::plan::Channel {
		unit: 0,
		did: 0,
		bit_offset: 0,
		bit_length: 0,
		signed: false,
		big_endian: false,
		factor: 0.0,
		offset: 0.0,
		decimals: 0,
		unit_text: "",
		label: "",
		proven: false,
		hz: hz as f32,
		setpoint: None,
		zero_at_rest: false,
	};
	channel.period_ms()
}

/// One enumerated field the car's variant declares, found by `pick`, as a plan channel on
/// no page: raw value, no scaling, its states beside it.
fn state_field(offered: &[poll::Channel], request: u16, pick: impl Fn(&poll::Channel) -> bool) -> Vec<&poll::Channel> {
	offered.iter().filter(|c| c.request == request && c.def.is_some() && pick(c)).collect()
}

fn levels_of(found: &poll::Channel) -> Option<&[Level]> {
	match &found.def.as_ref()?.scaling {
		Scaling::Enum { levels } => Some(levels),
		_ => None,
	}
}

fn state_channel(found: &poll::Channel, label: String) -> Channel {
	let def = found.def.as_ref().expect("picked on def");
	let ReadId::Uds(did) = def.address;
	let (bit_offset, bit_length, signed, big_endian) = bits_of(def.raw_form);
	Channel {
		unit: found.request,
		did,
		bit_offset,
		bit_length,
		signed,
		big_endian,
		// The raw value: a state is looked up by it, never scaled.
		factor: 1.0,
		offset: 0.0,
		decimals: 0,
		unit_text: String::new(),
		label,
		proven: found.proven,
		hz: DEFAULT_HZ,
		source: found.text_id.clone().unwrap_or_else(|| def.name.to_string()),
		setpoint: None,
		zero_at_rest: false,
	}
}

/// A state by the name the project gives it, as its place in the field's list. A name the
/// field gives two bands is refused: the board takes a state as one band, its place in the
/// list, and would react to one of them and never to the other. The message names the bands,
/// so the owner sees that it is the project's layout and not a typo.
///
/// Names are compared trimmed on both sides ([`same_name`]), and listed trimmed, as the
/// owner can type them.
fn state_index(levels: &[Level], name: &str, key: &str, field: &str) -> Result<u16, Error> {
	let at = levels.iter().position(|l| same_name(l.name(), name)).ok_or_else(|| {
		let names: Vec<String> = levels.iter().map(|l| format!("{:?}", l.name().trim())).collect();
		Error::Stalk(format!(
			"{key}: {name:?} is not a state of {field:?} — its states are {}",
			names.join(", ")
		))
	})?;
	let bands: Vec<String> = levels.iter().filter(|l| same_name(l.name(), name)).map(band_text).collect();
	if bands.len() > 1 {
		let what = if key.ends_with("_off") { "an off state" } else { "a button" };
		return Err(Error::Stalk(format!(
			"{key}: {:?} names {} bands of {field:?} ({}) — the board takes {what} as one band, so this state cannot be used yet",
			name.trim(),
			bands.len(),
			bands.join(", ")
		)));
	}
	Ok(at as u16)
}

/// A level's raw values as a person reads them: `51–101`, `7` for a point, and an unbounded
/// end as the way it runs.
fn band_text(level: &Level) -> String {
	match (level.lower(), level.upper()) {
		(lower, upper) if lower == upper => lower.to_string(),
		(i32::MIN, i32::MAX) => "any value".to_string(),
		(i32::MIN, upper) => format!("up to {upper}"),
		(lower, i32::MAX) => format!("{lower} and up"),
		(lower, upper) => format!("{lower}–{upper}"),
	}
}

/// Whether a name in `dash.toml` is the project's name. Trimmed on both sides: `dash.toml` is
/// trimmed when it is read, and an ODIS project spells some names with a space at an end
/// (`"Fahrbereitschaft "`) that nobody can see, so compared as written such a field or state
/// could never be named. Two names the project tells apart by that space alone are then
/// one name, which the callers refuse as a name given twice.
fn same_name(project: &str, owner: &str) -> bool {
	project.trim() == owner.trim()
}

/// The project a `[stalk]` is resolved against: the proven rows and what the project declares.
#[derive(Clone, Copy)]
struct Sources<'a> {
	store: &'a CatalogStore,
	extracted: &'a Extracted,
}

/// **A state is a band, and the lever needs the band.** A switch read as a voltage answers
/// inside its band, rarely on its lower end, so a state kept as one value matches nearly
/// nothing and the lever never presses. Two places can have lost the bands, and both are
/// refused with what to do:
///
/// - the project's cache, written before each level kept its upper end — for the whole unit;
/// - a proven row in `measurements/`, which wins at its field ([`crate::extracted::tagged`])
///   and was written before state ranges: every state a single value where the project's
///   own row for the field gives bands. A field the project itself gives as single values —
///   a digital status — is one, proven or not.
fn states_keep_their_bands(key: &str, found: &poll::Channel, identity: &UnitIdentity, sources: Sources<'_>) -> Result<(), Error> {
	let (odx_name, version) = (identity.odx_name.as_deref(), identity.odx_version.as_deref());
	if sources.extracted.levels_predate_bounds(odx_name, version) {
		return Err(Error::Stalk(format!(
			"unit {:03X}: the project's cache keeps only the lower end of each state — run `vagcan setup` again, so a reading anywhere in a state's band is that state",
			identity.request
		)));
	}
	let Some(def) = found.def.as_ref().filter(|_| found.proven) else {
		return Ok(());
	};
	let declared = sources.extracted.declared_at(odx_name, version, def);
	let enum_levels = |def: &vag_data_labels::catalog::MeasurementDef| match &def.scaling {
		Scaling::Enum { levels } => Some(levels.clone()),
		_ => None,
	};
	let single = |levels: &[Level]| levels.iter().all(|l| l.lower() == l.upper());
	match (enum_levels(def), declared.as_ref().and_then(enum_levels)) {
		(Some(proven), Some(declared)) if single(&proven) && !single(&declared) => {
			let file = sources
				.store
				.file_for_unit(identity.part_number.as_deref(), odx_name)
				.map_or_else(|| "the unit's proven catalog".to_string(), |path| path.display().to_string());
			let name = def.name.trim();
			Err(Error::Stalk(format!(
				"{key}: the proven row for {name:?} in {file} holds each state as a single value — it predates state ranges, and \
				 the project gives {name:?} bands, so a reading inside one would be no state; write its states as \
				 [lower, upper, \"name\"] or remove the row"
			)))
		}
		_ => Ok(()),
	}
}

/// `[stalk]` against the project and the car: every name the owner wrote is the project's
/// own, for the variant the car reported, and the states come with the intervals the
/// project gives them ([`states_keep_their_bands`]). The three fields are appended to
/// `channels`.
fn resolve_stalk(
	wanted: &StalkInput,
	channels: &mut Vec<Channel>,
	offered: &[poll::Channel],
	units: &[UnitIdentity],
	sources: Sources<'_>,
	notes: &mut Vec<String>,
) -> Result<Stalk, Error> {
	let read = Reference::Field {
		request: wanted.request,
		did: wanted.did,
		bit_offset: 0,
	};
	let identity_of = |request: u16| units.iter().find(|u| u.request == request).ok_or(Error::UnknownUnit(request));
	let (lever_unit, cruise_unit) = (identity_of(wanted.request)?, identity_of(wanted.cruise.request())?);
	let in_read = state_field(offered, wanted.request, |c| c.did == wanted.did);
	if in_read.is_empty() {
		return Err(Error::Stalk(format!("read {read}: the car's variant declares no such identifier")));
	}
	let field = |key: &str, name: &str| -> Result<(&poll::Channel, &[Level]), Error> {
		let named: Vec<&&poll::Channel> = in_read
			.iter()
			.filter(|c| c.def.as_ref().is_some_and(|d| same_name(&d.name, name)))
			.collect();
		let found = match named.as_slice() {
			[one] => **one,
			[] => {
				let names: Vec<String> = in_read
					.iter()
					.filter_map(|c| c.def.as_ref())
					.map(|d| format!("{:?}", d.name.trim()))
					.collect();
				return Err(Error::Stalk(format!(
					"{key}: {read} has no field named {name:?} — its fields are {}",
					names.join(", ")
				)));
			}
			many => return Err(Error::Stalk(format!("{key}: {read} has {} fields named {name:?}", many.len()))),
		};
		let levels = levels_of(found).ok_or_else(|| Error::Stalk(format!("{key}: {name:?} is a quantity, not a list of states")))?;
		states_keep_their_bands(key, found, lever_unit, sources)?;
		Ok((found, levels))
	};
	let (rocker, rocker_levels) = field("rocker", &wanted.rocker)?;
	let (switch, switch_levels) = field("switch", &wanted.switch)?;
	if wanted.rocker == wanted.switch {
		return Err(Error::Stalk(format!("rocker and switch are both {:?}", wanted.rocker)));
	}
	let next = state_index(rocker_levels, &wanted.next, "next", &wanted.rocker)?;
	let previous = state_index(rocker_levels, &wanted.previous, "previous", &wanted.rocker)?;
	let measure = state_index(rocker_levels, &wanted.measure, "measure", &wanted.rocker)?;
	if next == previous || next == measure || previous == measure {
		return Err(Error::Stalk(
			"next, previous and measure name the same state — each is a button of its own".to_string(),
		));
	}
	let switch_off = state_index(switch_levels, &wanted.switch_off, "switch_off", &wanted.switch)?;

	let cruise_request = wanted.cruise.request();
	let candidates = state_field(offered, cruise_request, |c| match &wanted.cruise {
		Reference::TextId { text_id, .. } => c.text_id.as_deref() == Some(text_id.as_str()),
		Reference::Field { did, bit_offset, .. } => c.did == *did && c.def.as_ref().map_or(0, |d| d.raw_form.bit_offset()) == *bit_offset,
	});
	let states: Vec<&poll::Channel> = candidates.iter().copied().filter(|c| levels_of(c).is_some()).collect();
	let cruise = match (states.as_slice(), candidates.as_slice()) {
		([one], _) => *one,
		([], []) => {
			return Err(Error::Stalk(format!(
				"cruise {}: the car's variant declares no such field",
				wanted.cruise
			)));
		}
		([], _) => return Err(Error::Stalk(format!("cruise {}: a quantity, not a list of states", wanted.cruise))),
		(many, _) => return Err(Error::Ambiguous(wanted.cruise.clone(), many.iter().map(|c| row_name(c)).collect())),
	};
	// The gate opens only when two answers say off: the switch's, and the cruise status's. A
	// cruise status read out of the lever's own identifier is that one answer again, however
	// it is spelled — compared as resolved.
	if cruise.request == wanted.request && cruise.did == wanted.did {
		return Err(Error::Stalk(format!(
			"cruise {} is in read's own identifier {read} — the lever is used only when two answers say cruise is off, and one \
			 identifier is one answer; name the cruise status the unit that runs cruise control reports",
			wanted.cruise
		)));
	}
	states_keep_their_bands("cruise", cruise, cruise_unit, sources)?;
	let cruise_levels = levels_of(cruise).expect("picked on levels");
	let cruise_name = cruise.label();
	let cruise_off = state_index(cruise_levels, &wanted.cruise_off, "cruise_off", &cruise_name)?;

	let index = channels.len() as u16;
	channels.push(state_channel(rocker, wanted.rocker.clone()));
	channels.push(state_channel(switch, wanted.switch.clone()));
	channels.push(state_channel(cruise, cruise_name));
	notes.push(format!(
		"stalk: {read} rocker {:?} (next {:?}, previous {:?}, measure {:?}), switch {:?} off at {:?}; cruise {} off at {:?}",
		wanted.rocker, wanted.next, wanted.previous, wanted.measure, wanted.switch, wanted.switch_off, wanted.cruise, wanted.cruise_off
	));
	Ok(Stalk {
		rocker: index,
		switch: index + 1,
		cruise: index + 2,
		rocker_states: State::of(rocker_levels),
		switch_states: State::of(switch_levels),
		cruise_states: State::of(cruise_levels),
		next,
		previous,
		measure,
		switch_off,
		cruise_off,
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
	if plan.alarms.iter().any(|a| matches!(a.rule, AlarmRule::Bounds { .. })) {
		imports.push("Bound");
	}
	imports
}

/// The plan as Rust source: a `static PLAN` of `vag_dash_render::plan::Plan`, and last its
/// [`plan_id`] as `PLAN_ID`.
pub fn to_rust(plan: &Plan) -> String {
	let mut out = source(plan);
	let id = fnv1a64(out.as_bytes());
	out.push_str(&format!(
		"\n/// FNV-1a 64 of every byte above this line: which plan this is (`vag_cli_core::dash::plan_id`).\npub const PLAN_ID: u64 = 0x{id:016x};\n"
	));
	out
}

/// Which plan this is: FNV-1a 64 over its generated source ([`to_rust`] short of the id),
/// byte for byte what the firmware compiles — so a plan the board would run differently on
/// has another id, and the same plan has the same one on every machine and every build.
/// The board keeps it beside its settings (`vag-dash-fw`'s `schema`) and says it in its
/// Hello reply and its `state` line. A change to how the generator spells the source also
/// changes it; that only makes the board say its settings were saved against another plan,
/// and nothing of them is dropped for it.
pub fn plan_id(plan: &Plan) -> u64 {
	fnv1a64(source(plan).as_bytes())
}

/// 64-bit FNV-1a (Fowler–Noll–Vo; the offset basis and prime are the algorithm's own).
fn fnv1a64(bytes: &[u8]) -> u64 {
	const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
	const PRIME: u64 = 0x0000_0100_0000_01b3;
	bytes
		.iter()
		.fold(OFFSET_BASIS, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(PRIME))
}

/// The plan's source, short of its id: what [`plan_id`] is taken over.
fn source(plan: &Plan) -> String {
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
	let joins = plan.joins().expect("a plan from `build` joins");
	let mut plan_types = vec!["Channel", "Page", "Plan", "Unit"];
	if plan.stalk.is_some() {
		plan_types.extend(["Band", "StalkPlan"]);
	}
	if plan.stopwatch.is_some() {
		plan_types.push("StopwatchPlan");
	}
	if !plan.buttons.is_empty() {
		plan_types.push("ButtonPlan");
	}
	if plan.standstill.is_some() {
		plan_types.push("Standstill");
	}
	plan_types.sort_unstable();
	let _ = writeln!(out, "use vag_dash_render::plan::{{{}}};", plan_types.join(", "));
	if plan.stalk.is_some() {
		let _ = writeln!(out, "use vag_dash_render::stalk::{{StateIndex, States}};");
	}
	if !plan.buttons.is_empty() {
		let _ = writeln!(out, "use vag_dash_render::control::Command;");
	}
	// Only what the rules actually name: the firmware lints the generated file with
	// `-D warnings`, so an unused import is a build that fails (review, 2026-09-15). A plan
	// whose rules are all drift never names `Bound`; one with no rules names nothing but
	// the type itself.
	let mut imports = alarm_imports(plan);
	imports.sort_unstable();
	let _ = match imports.as_slice() {
		[one] => writeln!(out, "use vag_dash_render::alarm::{one};"),
		many => writeln!(out, "use vag_dash_render::alarm::{{{}}};", many.join(", ")),
	};
	let _ = writeln!(out);
	let stalk = match &plan.stalk {
		None => "None".to_string(),
		Some(s) => format!(
			"Some(StalkPlan {{ rocker: {}, switch: {}, cruise: {}, rocker_states: &STALK_ROCKER, switch_states: &STALK_SWITCH, cruise_states: &STALK_CRUISE, states: States {{ next: StateIndex({}), previous: StateIndex({}), measure: StateIndex({}), switch_off: StateIndex({}), cruise_off: StateIndex({}) }} }})",
			s.rocker, s.switch, s.cruise, s.next, s.previous, s.measure, s.switch_off, s.cruise_off
		),
	};
	let stopwatch = match &plan.stopwatch {
		None => "None".to_string(),
		Some(s) => format!(
			"Some(StopwatchPlan {{ speed: {}, km_h_per_unit: {}, marks: &MARKS }})",
			s.speed,
			float(s.km_h_per_unit)
		),
	};
	// No buttons is `&[]`, so the source names neither `ButtonPlan` nor `Command`.
	let buttons = if plan.buttons.is_empty() { "&[]" } else { "&BUTTONS" };
	let standstill = match plan.standstill {
		None => "None".to_string(),
		Some(Standstill::Speed { channel }) => format!("Some(Standstill::Speed({channel}))"),
	};
	let index = |v: Option<u16>| v.map_or_else(|| "None".to_string(), |v| format!("Some({v})"));
	let _ = writeln!(
		out,
		"pub static PLAN: Plan = Plan {{ vin: {:?}, language: {:?}, units: &UNITS, channels: &CHANNELS, pages: &PAGES, alarms: &ALARMS, stalk: {stalk}, stopwatch: {stopwatch}, buttons: {buttons}, standstill: {standstill} }};",
		plan.vin, plan.language
	);
	let _ = writeln!(out);
	let _ = writeln!(out, "static UNITS: [Unit; {}] = [", plan.units.len());
	// Numbered: a channel names its unit by this index.
	for (at, u) in plan.units.iter().enumerate() {
		let _ = writeln!(
			out,
			"\tUnit {{ request: 0x{:03X}, response: 0x{:03X}, part_number: {:?} }}, // {at}",
			u.request, u.response, u.part_number
		);
	}
	let _ = writeln!(out, "];");
	let _ = writeln!(out);
	let _ = writeln!(out, "static CHANNELS: [Channel; {}] = [", plan.channels.len());
	for (c, unit) in plan.channels.iter().zip(&joins.units) {
		let _ = writeln!(
			out,
			"\tChannel {{ unit: {}, did: 0x{:04X}, bit_offset: {}, bit_length: {}, signed: {}, big_endian: {}, factor: {}, offset: {}, decimals: {}, unit_text: {:?}, label: {:?}, proven: {}, hz: {}, setpoint: {}, zero_at_rest: {} }},",
			unit,
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
			},
			c.zero_at_rest
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
			Page::Chart {
				title, channel, min, max, ..
			} => {
				let _ = writeln!(
					out,
					"\tPage::Chart {{ title: {title:?}, channel: {channel}, slot: {}, min: {}, max: {}, volts: {} }},",
					joins.slots[i].expect("a chart page has a slot"),
					float(*min),
					float(*max),
					index(joins.volts[i])
				);
			}
			Page::Values { title, .. } => {
				let _ = writeln!(
					out,
					"\tPage::Values {{ title: {title:?}, cells: &CELLS_{i}, volts: {} }},",
					index(joins.volts[i])
				);
			}
		}
	}
	let _ = writeln!(out, "];");
	let _ = writeln!(out);
	let bound = |b: &Option<Bound>| match b {
		Some(b) => format!("Some(Bound {{ at: {}, off: {} }})", float(b.at), float(b.off)),
		None => "None".to_string(),
	};
	let _ = writeln!(out, "static ALARMS: [Alarm; {}] = [", plan.alarms.len());
	for a in &plan.alarms {
		let rule = match &a.rule {
			AlarmRule::Bounds { max, min } => format!("Rule::Bounds {{ max: {}, min: {} }}", bound(max), bound(min)),
			AlarmRule::Drift {
				specified,
				percent,
				off_percent,
				hold_ms,
				min_setpoint,
			} => format!(
				"Rule::Drift {{ specified: ChannelId({specified}), percent: {}, off_percent: {}, hold_ms: {hold_ms}, min_setpoint: {} }}",
				float(*percent),
				float(*off_percent),
				float(*min_setpoint)
			),
		};
		let _ = writeln!(
			out,
			"\tAlarm {{ channel: ChannelId({}), page: PageId({}), rule: {rule} }},",
			a.channel, a.page
		);
	}
	let _ = writeln!(out, "];");
	if let Some(s) = &plan.stalk {
		// `i32::MIN` / `i32::MAX` by name: an unbounded end is a limit, not a number anyone wrote.
		let bound = |v: i32| match v {
			i32::MIN => "i32::MIN".to_string(),
			i32::MAX => "i32::MAX".to_string(),
			v => v.to_string(),
		};
		for (name, states) in [
			("STALK_ROCKER", &s.rocker_states),
			("STALK_SWITCH", &s.switch_states),
			("STALK_CRUISE", &s.cruise_states),
		] {
			let _ = writeln!(out);
			let _ = writeln!(out, "static {name}: [Band; {}] = [", states.len());
			for state in states {
				let _ = writeln!(
					out,
					"\tBand {{ lower: {}, upper: {} }}, // {:?}",
					bound(state.lower),
					bound(state.upper),
					state.name
				);
			}
			let _ = writeln!(out, "];");
		}
	}
	if let Some(s) = &plan.stopwatch {
		let list: Vec<String> = s.marks.iter().map(u16::to_string).collect();
		let _ = writeln!(out);
		let _ = writeln!(out, "static MARKS: [u16; {}] = [{}];", s.marks.len(), list.join(", "));
	}
	if !plan.buttons.is_empty() {
		let _ = writeln!(out);
		let _ = writeln!(out, "static BUTTONS: [ButtonPlan; {}] = [", plan.buttons.len());
		for b in &plan.buttons {
			let _ = writeln!(out, "\tButtonPlan {{ pin: {}, action: Command::{:?} }},", b.pin, b.action);
		}
		let _ = writeln!(out, "];");
	}
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
	/// car's record of its units, the project's cache and proven rows, the name
	/// table, and the settings and glossary that decide the labels' language
	/// and wording.
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
	/// What each unit said about itself, out of the car's record: what the catalogs were
	/// looked up by.
	pub units: Vec<UnitIdentity>,
	pub store: CatalogStore,
	/// The project, naming channels in `config.toml`'s language, as `watch` does — not
	/// necessarily the plan's, which [`build`] labels in its own.
	pub extracted: Extracted,
}

/// A car's build input, read and checked, and the car's record of its units — everything
/// a build needs that is not the project. Read first, before anything slow: `dev dash build`
/// reads the VCDS registry for the units on record before it builds, and a typo in the
/// input or a VIN that does not match must be refused before that read, not after it.
#[derive(Debug)]
pub struct Inputs {
	pub vin: String,
	pub parsed: Input,
	/// `~/.vagcan/dash/<VIN>/`, where the outputs go.
	pub dir: PathBuf,
	pub input_path: PathBuf,
	/// `~/.vagcan/cars/<VIN>/units.json`.
	pub record: PathBuf,
	/// What each unit said about itself, out of the car's record.
	pub units: Vec<UnitIdentity>,
	/// Whether the caller has just tried to read the units' channels
	/// (`dev dash build` does, through `registry::ensure`): a unit still unread
	/// after that is not one the same command should be told to run.
	pub read_attempted: bool,
}

/// Read and check the input and the car's record.
///
/// `input` defaults to `dash.toml` under `~/.vagcan/dash/<VIN>/`. The units are the
/// car's record (`~/.vagcan/cars/<VIN>/units.json`), which `watch`, `measure` and
/// `units --identify` write; with none the build refuses and says which command to
/// connect to the car with.
pub fn read_inputs(vin: &str, input: Option<&Path>) -> anyhow::Result<Inputs> {
	use anyhow::Context as _;
	let dir = crate::datadir::dash_dir(vin)?;
	let input_path = input.map(Path::to_path_buf).unwrap_or_else(|| dir.join("dash.toml"));
	let text = match std::fs::read_to_string(&input_path) {
		Ok(text) => text,
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(missing_input(vin, &input_path, input.is_some())),
		Err(e) => return Err(e).with_context(|| format!("reading the build input {}", input_path.display())),
	};
	let parsed = parse_input(&text)?;
	if !parsed.vin.eq_ignore_ascii_case(vin.trim()) {
		anyhow::bail!("{} is for VIN {} but the build asked for {vin}", input_path.display(), parsed.vin);
	}
	let record = crate::datadir::units_record(vin)?;
	let units = crate::units::read_record(&record)?.ok_or_else(|| no_record(vin, &record))?;
	Ok(Inputs {
		vin: vin.to_string(),
		parsed,
		dir,
		input_path,
		record,
		units,
		read_attempted: false,
	})
}

/// [`build_for_car`] short of writing anything: read the input, the car's record of
/// its units and the project, and build. For a command that reads the plan and keeps
/// nothing (`vagcan dev recording dash`).
///
/// Every channel the project describes is `declared`: the record holds what the units
/// are, nothing about which identifiers they answer.
pub fn resolve_for_car(vin: &str, input: Option<&Path>) -> anyhow::Result<Resolved> {
	resolve(read_inputs(vin, input)?)
}

/// [`resolve_for_car`] on inputs already read: the project, the check that the plan's
/// units have had their channels read, and the build.
///
/// **A unit the plan uses whose channels a VCDS read has yet to bring is a refusal,
/// not a build.** The project's VCDS channels come per unit, the first time a command
/// with the car — or `dev dash build`, offline — reads them
/// ([`crate::registry::unread`]); a plan built for a car before that read would refuse
/// every channel of those units as "not declared", which is a false diagnosis of the
/// input, and the firmware's build script would print it (found in review,
/// 2026-09-28). So the build says the truth instead: which units, and what reads them.
/// Only the units the input references ([`referenced_units`]): a unit the plan never
/// reads — asleep at the first read, a new version after a software update, one named
/// once with `--did` — must not block the build (found in the next review), least of
/// all with the installation gone; it is a note.
pub fn resolve(inputs: Inputs) -> anyhow::Result<Resolved> {
	let Inputs {
		vin,
		parsed,
		dir,
		input_path,
		record,
		units,
		read_attempted,
	} = inputs;
	let stores = crate::project::stores()?;
	let store = CatalogStore::open(stores.measurements_dir());
	let extracted = crate::extracted::open(&stores).for_car(Some(vin.as_str()));
	let referenced = referenced_units(&parsed);
	let (used, unused): (Vec<UnitIdentity>, Vec<UnitIdentity>) = units.iter().cloned().partition(|u| referenced.contains(&u.request));
	let (used_note, unused_note) = {
		let described = crate::registry::described_by(&extracted);
		let used = refuse_unread(&vin, read_attempted, crate::registry::unread(&stores.vcds, &used, &described)?)?;
		(
			used,
			unread_note(&vin, read_attempted, crate::registry::unread(&stores.vcds, &unused, &described)?),
		)
	};
	let language = crate::config::language(&crate::config::load());
	let mut built = build(&parsed, &store, &extracted, &units, language)?;
	built.notes.extend(used_note);
	built.notes.extend(unused_note);
	let mut inputs = vec![input_path, record];
	inputs.extend(stores.caches());
	inputs.extend([
		stores.measurements_dir(),
		stores.vcds.names(),
		stores.vcds.registry_log(),
		crate::config::path()?,
		crate::glossary::path()?,
	]);
	Ok(Resolved {
		built,
		dir,
		inputs,
		units,
		store,
		extracted,
	})
}

/// The units a build input reads: its channels and their specified values, the stalk's
/// read and its cruise status, the stopwatch's speed, the strip's voltage. A page names a channel
/// from that list, so the channels are the whole of it — what has to have had its
/// channels read before the plan can be built, and nothing else.
pub fn referenced_units(input: &Input) -> std::collections::BTreeSet<u16> {
	let mut units: std::collections::BTreeSet<u16> = input
		.channels
		.iter()
		.flat_map(|c| std::iter::once(c.reference.request()).chain(c.setpoint.as_ref().map(Reference::request)))
		.collect();
	if let Some(stalk) = &input.stalk {
		units.insert(stalk.request);
		units.insert(stalk.cruise.request());
	}
	if let Some(stopwatch) = &input.stopwatch {
		units.insert(stopwatch.speed.request());
	}
	if let Some(strip) = &input.strip {
		units.insert(strip.volts.request());
	}
	units
}

/// "the channels of N control unit(s) of this car have not been read from the VCDS
/// installation (names)": the subject of the refusal and of the note.
fn unread_subject(units: &std::collections::BTreeSet<(String, String)>) -> String {
	let names: Vec<&str> = units.iter().map(|(odx, _)| odx.as_str()).collect();
	format!(
		"the channels of {} control unit{} of this car have not been read from the VCDS installation ({})",
		units.len(),
		if units.len() == 1 { "" } else { "s" },
		names.join(", ")
	)
}

/// What reads the channels of units still unread, for the refusal and the note:
/// `dev dash build`, unless that is the command speaking — it has just tried, and
/// the line it printed says what stopped the read.
fn what_reads_them(vin: &str, read_attempted: bool, said_where: &str) -> String {
	match read_attempted {
		true => format!("{said_where} what stopped the read"),
		false => format!("`vagcan dev dash build {vin}` reads them once, offline, or a command with the car does"),
	}
}

/// Where `dev dash build`'s read said what stopped it, from where the refusal is
/// printed — right under it — and from where the note is: after the plan's lines,
/// twenty-odd of them (found in review, 2026-09-28).
const ABOVE_THE_REFUSAL: &str = "the lines above say";
const ABOVE_THE_NOTE: &str = "the first lines of this output say";

/// The refusal behind [`resolve`]: the plan's units whose channels have not been read,
/// and what reads them. Nothing when there are none.
///
/// Units only an older version of this tool read are no reason to refuse: their rows are
/// in the cache, and the plan is built from them — the firmware's build script, too, after
/// a [`crate::registry::READ_VERSION`] bump. The note that comes back says so; a command
/// with the car reads them again once.
fn refuse_unread(vin: &str, read_attempted: bool, unread: crate::registry::Unread) -> anyhow::Result<Option<String>> {
	use crate::registry::Unread;
	match unread {
		Unread::Nothing => Ok(None),
		Unread::Outdated { units, .. } => Ok(Some(outdated_note(vin, &units))),
		Unread::Readable { units, .. } => anyhow::bail!("{} — {}", unread_subject(&units), what_reads_them(vin, read_attempted, ABOVE_THE_REFUSAL)),
		Unread::Unreadable { units, why } => anyhow::bail!("{}, and {why}", unread_subject(&units)),
	}
}

/// The note behind [`resolve`] for the units the plan does not use: their channels are
/// not read yet, and why — the build goes ahead without them, and says so once.
fn unread_note(vin: &str, read_attempted: bool, unread: crate::registry::Unread) -> Option<String> {
	use crate::registry::Unread;
	match unread {
		Unread::Nothing => None,
		Unread::Outdated { units, .. } => Some(outdated_note(vin, &units)),
		Unread::Readable { units, .. } => Some(format!(
			"{}, which the plan does not use; {}",
			unread_subject(&units),
			what_reads_them(vin, read_attempted, ABOVE_THE_NOTE)
		)),
		Unread::Unreadable { units, why } => Some(format!("{}, which the plan does not use, and {why}", unread_subject(&units))),
	}
}

/// What a build says of units an older version of this tool read: built from the rows the
/// cache holds, and what reads them again.
fn outdated_note(vin: &str, units: &std::collections::BTreeSet<(String, String)>) -> String {
	format!(
		"{} {} read by an older version of this tool: built from the channels the cache holds; \
		 `vagcan dev dash build {vin}`, or a command with the car, reads {} again once",
		units.len(),
		if units.len() == 1 { "control unit was" } else { "control units were" },
		if units.len() == 1 { "it" } else { "them" }
	)
}

/// The whole command: read the input, the car's record of its units and the
/// project, build, write `plan.json` and `plan.rs` under `~/.vagcan/dash/<VIN>/`.
///
/// `input` defaults to `dash.toml` in that directory. The firmware's build
/// script calls this too, so `cargo build` of the firmware *is* the plan build.
pub fn build_for_car(vin: &str, input: Option<&Path>) -> anyhow::Result<Written> {
	build_inputs(read_inputs(vin, input)?)
}

/// [`build_for_car`] on inputs already read — for `dev dash build`, which reads the
/// registry for the units on record between the two.
pub fn build_inputs(inputs: Inputs) -> anyhow::Result<Written> {
	use anyhow::Context as _;
	let Resolved { built, dir, inputs, .. } = resolve(inputs)?;
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
	/// `plan.json` read back, to check what it holds: nothing outside the tests reads it.
	fn from_json(text: &str) -> Result<Plan, serde_json::Error> {
		serde_json::from_str(text)
	}

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

	#[test]
	fn the_units_a_plan_reads_are_its_channels_setpoints_stalk_and_stopwatch_and_nothing_else() {
		// Found in review (2026-09-28): the unread-channels refusal checked every
		// recorded unit, so a unit the plan never reads — asleep at the first
		// read, a new version after an update, named once with `--did` — blocked
		// every build, and with the installation gone there was no way out.
		let parse = |text: &str| Reference::parse(text).unwrap();
		let input = Input {
			vin: "TESTVIN0000000001".into(),
			language: None,
			channels: vec![ChannelInput {
				setpoint: Some(parse("02:IDE00190")),
				..ChannelInput::bare(parse("01:IDE00025"))
			}],
			pages: Vec::new(),
			stalk: Some(StalkInput {
				request: 0x70C,
				did: 0x1105,
				rocker: "r".into(),
				switch: "s".into(),
				next: "n".into(),
				previous: "p".into(),
				measure: "m".into(),
				switch_off: "off".into(),
				cruise: parse("713:IDE00001"),
				cruise_off: "off".into(),
			}),
			stopwatch: Some(StopwatchInput {
				speed: parse("7E1:F40D"),
				km_h_per_unit: 1.0,
				marks: Vec::new(),
			}),
			buttons: Vec::new(),
			strip: Some(StripInput {
				volts: parse("710:IDE00018"),
			}),
		};
		assert_eq!(referenced_units(&input), [ENGINE, GEARBOX, 0x70C, 0x710, 0x713].into());
		let smallest = self::input(&["01:F423"], &values_page(&["01:F423"]));
		assert_eq!(referenced_units(&smallest), [ENGINE].into());

		// The plan's units are refused; the rest are a note, with the cause.
		use crate::registry::Unread;
		let units = |names: &[&str]| names.iter().map(|n| (n.to_string(), "001".to_string())).collect();
		assert_eq!(refuse_unread("VIN", false, Unread::Nothing).unwrap(), None);
		assert_eq!(unread_note("VIN", false, Unread::Nothing), None);
		let readable = Unread::Readable {
			units: units(&["EV_Sixteenth"]),
			root: PathBuf::from("/vcds"),
		};
		let refusal = refuse_unread("VIN", false, readable.clone()).unwrap_err().to_string();
		assert!(
			refusal.starts_with("the channels of 1 control unit of this car have not been read"),
			"{refusal}"
		);
		assert!(
			refusal.contains("(EV_Sixteenth)") && refusal.contains("`vagcan dev dash build VIN`"),
			"{refusal}"
		);
		let note = unread_note("VIN", false, readable.clone()).unwrap();
		assert!(note.contains("(EV_Sixteenth), which the plan does not use"), "{note}");
		assert!(note.contains("`vagcan dev dash build VIN`"), "{note}");
		// `dev dash build` itself has just tried to read them: it is not told to
		// run itself (found in review, 2026-09-28).
		let refusal = refuse_unread("VIN", true, readable.clone()).unwrap_err().to_string();
		assert!(refusal.ends_with("the lines above say what stopped the read"), "{refusal}");
		assert!(!refusal.contains("dev dash build"), "{refusal}");
		// The note comes after the plan's lines, so it points at the top of the output.
		let note = unread_note("VIN", true, readable).unwrap();
		assert!(
			note.ends_with("which the plan does not use; the first lines of this output say what stopped the read"),
			"{note}"
		);
		// Units only an older read tried: the build goes ahead on the cache's rows, and says
		// so — the firmware's build after a READ_VERSION bump must not refuse (found in
		// review, 2026-10-20).
		let outdated = Unread::Outdated {
			units: units(&["EV_Gatew", "EV_Brake"]),
			root: PathBuf::from("/vcds"),
		};
		let note = refuse_unread("VIN", false, outdated.clone()).unwrap().expect("a note");
		assert!(note.starts_with("2 control units were read by an older version of this tool"), "{note}");
		assert!(note.contains("built from the channels the cache holds"), "{note}");
		assert_eq!(unread_note("VIN", false, outdated), Some(note));
		let gone = Unread::Unreadable {
			units: units(&["EV_A", "EV_B"]),
			why: "the VCDS installation to read from is not on this machine any more".into(),
		};
		let refusal = refuse_unread("VIN", false, gone.clone()).unwrap_err().to_string();
		assert!(
			refusal.contains("2 control units") && refusal.contains("not on this machine any more"),
			"{refusal}"
		);
		let note = unread_note("VIN", true, gone).unwrap();
		assert!(
			note.contains("(EV_A, EV_B), which the plan does not use, and the VCDS installation"),
			"{note}"
		);
	}

	#[test]
	fn a_survey_key_is_not_read() {
		// `survey =` named the file the units were read from, before they came from the car's
		// record. It is no key of this file, so the strict check refuses it by line.
		let text = "vin = \"TESTVIN0000000001\"\nlanguage = \"en\"\nsurvey = \"/somewhere/survey.jsonl\"\n[[channel]]\nref = \"01:IDE00025\"\n[[page]]\nkind = \"values\"\ntitle = \"T\"\ncells = [\"01:IDE00025\"]\n";
		let Error::Parse(why) = parse_input(text).unwrap_err() else {
			panic!("not a parse refusal");
		};
		assert!(why.contains("line 3"), "{why}");
		assert!(why.contains("\"survey\""), "{why}");
		// Without it the same input parses.
		assert!(parse_input(&text.replace("survey = \"/somewhere/survey.jsonl\"\n", "")).is_ok());
	}

	fn values_page(cells: &[&str]) -> String {
		values_page_titled("T", cells)
	}

	fn values_page_titled(title: &str, cells: &[&str]) -> String {
		let list: Vec<String> = cells.iter().map(|c| format!("\"{c}\"")).collect();
		format!("[[page]]\nkind = \"values\"\ntitle = \"{title}\"\ncells = [{}]\n", list.join(", "))
	}

	/// No alarm keys on any of [`build_with_alarms`]'s channels.
	const NO_ALARMS: [&str; 3] = ["", "", ""];

	/// Three neutral channels on one unit, each followed by its line of `keys` (alarm keys,
	/// `\n`-separated); values pages `A` (channels 0 and 1) and `B` (channel 2), a chart of
	/// channel 0; then whatever the test appends.
	fn build_with_alarms(keys: [&str; 3], extra: &str) -> Result<Built, Error> {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![
					reading(0x1001, "One", "IDE00001", 0, 8, false, true, 1.0, 0.0),
					reading(0x1002, "Two", "IDE00002", 0, 8, false, true, 1.0, 0.0),
					Reading {
						unit: Some("°C".to_string()),
						..reading(0x1003, "Three", "IDE00003", 0, 16, true, true, 0.1, 0.0)
					},
				],
			)],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let line = |keys: &str| if keys.is_empty() { String::new() } else { format!("{keys}\n") };
		let text = format!(
			"vin = \"TESTVIN0000000001\"\n[[channel]]\nref = \"01:IDE00001\"\n{}[[channel]]\nref = \"01:IDE00002\"\n{}[[channel]]\nref = \"01:IDE00003\"\n{}{}{}[[page]]\nkind = \"chart\"\ncell = \"01:IDE00001\"\nmin = 0\nmax = 10\n{extra}",
			line(keys[0]),
			line(keys[1]),
			line(keys[2]),
			values_page_titled("A", &["01:IDE00001", "01:IDE00002"]),
			values_page_titled("B", &["01:IDE00003"]),
		);
		build(
			&parse_input(&text)?,
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
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

	/// The drift keys the tests build on, for the channel paired with a setpoint.
	fn drift_alarm(percent: f64, off_percent: f64) -> String {
		format!("drift_percent = {percent:?}\ndrift_off_percent = {off_percent:?}\ndrift_hold_ms = 1000\ndrift_min_setpoint = 0.5\n")
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
				channel: 0,
				page: 0,
				rule: AlarmRule::Drift {
					specified: 1,
					percent: 10.0,
					off_percent: 6.0,
					hold_ms: 1000,
					min_setpoint: 0.5,
				},
			}]
		);
		assert!(
			built.notes.iter().any(
				|n| n == "alarm: 01:IDE00191 10% or more from its specified value for 1000 ms, off at or under 6%, nothing said under 0.5 → page 1 \"A\""
			),
			"{:?}",
			built.notes
		);
		let json = built.plan.to_json();
		assert_eq!(from_json(&json).unwrap(), built.plan);
		assert!(json.contains("\"kind\": \"drift\""), "{json}");
		let rust = to_rust(&built.plan);
		assert!(
			rust.contains(
				"Alarm { channel: ChannelId(0), page: PageId(0), rule: Rule::Drift { specified: ChannelId(1), percent: 10.0, off_percent: 6.0, hold_ms: 1000, min_setpoint: 0.5 } },"
			),
			"{rust}"
		);
	}

	#[test]
	fn a_drift_rule_over_a_channel_with_no_specified_value_is_refused() {
		let why = build_with_setpoint(&format!("[[channel]]\nref = \"01:IDE00191\"\n{}", drift_alarm(10.0, 6.0))).unwrap_err();
		assert_eq!(
			why.to_string(),
			"01:IDE00191: drift_percent needs a setpoint — the drift is measured against what the unit asked for"
		);
	}

	#[test]
	fn a_drift_rule_that_does_not_go_off_under_its_percent_is_refused() {
		let why = build_with_setpoint(&format!(
			"[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\n{}",
			drift_alarm(10.0, 10.0)
		))
		.unwrap_err()
		.to_string();
		assert!(
			why.ends_with(
				"[[channel]] 1: drift_off_percent 10 is not under drift_percent 10 — the alarm goes off once the drift is back at drift_off_percent"
			),
			"{why}"
		);
	}

	/// `[[alarm]]` is gone (owner, 2026-10-19): a file that still has it is refused with its own
	/// numbers written as the channel keys that replace it — TOML that parses as it is printed —
	/// and, where the rules' order is not the channels', the `alarm_priority` that keeps it.
	#[test]
	fn an_alarm_table_is_refused_with_its_rewrite_on_the_channels() {
		let channels = ["01:IDE00191", "01:IDE01775", "01:IDE01776", "01:IDE00155"]
			.map(|c| format!("[[channel]]\nref = \"{c}\"\n"))
			.concat();
		let text = format!(
			"vin = \"TESTVIN0000000001\"\n{channels}[[page]]\nkind = \"values\"\ntitle = \"A\"\ncells = [\"01:IDE00191\"]\n\
			[[alarm]]\nchannels = [\"01:IDE01775\", \"01:IDE01776\"]\npage = \"MISFIRE\"\ndirection = \"above\"\ntrip = 5\nrelease = 3\n\
			[[alarm]]\nchannels = [\"01:IDE00155\"]\npage = \"RETARD\"\ndirection = \"below\"\ntrip = -6.0\nrelease = -4.5\n\
			[[alarm]]\nkind = \"drift\"\nchannels = [\"01:IDE00191\"]\npage = \"MAIN\"\npercent = 10\nrelease_percent = 5\nhold_ms = 2000\nmin_setpoint = 1.3\n"
		);
		let why = parse_input(&text).unwrap_err().to_string();
		assert!(
			why.starts_with("dash.toml: line 14: [[alarm]] is gone: an alarm is now keys of the [[channel]] it watches."),
			"{why}"
		);
		assert!(why.contains("`page` is gone"), "{why}");
		assert!(
			why.contains("\n[[channel]]\nref = \"01:IDE00155\"\nalarm_min = -6.0\nalarm_min_off = -4.5\nalarm_priority = 2\n"),
			"one key a line: {why}"
		);
		assert!(!why.contains("A threshold rule's"), "every rule was read: no rule in words: {why}");
		// The printed blocks are TOML: pasted under a `vin` and a page, they parse to the keys.
		let (_, rest) = why.split_once("same ref:\n").expect("the blocks follow");
		let (blocks, after) = rest.split_once("\nalarm_priority keeps").expect("the order is said");
		let pasted = format!("vin = \"X\"\n{blocks}\n[[page]]\nkind = \"values\"\ncells = [\"01:IDE00191\"]\n");
		let input = parse_input(&pasted).unwrap_or_else(|e| panic!("{e}\n{pasted}"));
		let refs: Vec<String> = input.channels.iter().map(|c| c.reference.to_string()).collect();
		assert_eq!(refs, ["01:IDE01775", "01:IDE01776", "01:IDE00155", "01:IDE00191"], "in the rules' order");
		assert_eq!(input.channels[0].alarm_max, Some(BoundInput { at: 5.0, off: Some(3.0) }));
		assert_eq!(input.channels[2].alarm_min, Some(BoundInput { at: -6.0, off: Some(-4.5) }));
		assert_eq!(
			input.channels[3].drift,
			Some(DriftInput {
				percent: 10.0,
				off_percent: 5.0,
				hold_ms: 2000,
				min_setpoint: 1.3
			})
		);
		// The drift channel comes first in the file and was the last rule: each channel takes its
		// rule's number, and with it the old order — no channel moves.
		let priorities: Vec<Option<u32>> = input.channels.iter().map(|c| c.alarm_priority).collect();
		assert_eq!(priorities, [Some(1), Some(1), Some(2), Some(3)]);
		assert!(after.contains("without alarm_priority the order in the file decides"), "{after}");
		// In the rules' order already, no priority is written.
		let ordered = text.replace(
			&channels,
			&["01:IDE01775", "01:IDE01776", "01:IDE00155", "01:IDE00191"]
				.map(|c| format!("[[channel]]\nref = \"{c}\"\n"))
				.concat(),
		);
		assert!(!parse_input(&ordered).unwrap_err().to_string().contains("alarm_priority"));
		// One it cannot read is said in words, as is a single `[alarm]`.
		let odd = text.replace("direction = \"below\"", "direction = \"sideways\"");
		assert!(parse_input(&odd).unwrap_err().to_string().contains("A threshold rule's trip and"));
		let single = "vin = \"X\"\n[[channel]]\nref = \"01:IDE00001\"\n[alarm]\npage = \"T\"\n";
		let why = parse_input(single).unwrap_err().to_string();
		assert!(why.contains("Delete every [[alarm]] table.\nA threshold rule's"), "{why}");
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
	fn a_plan_whose_rules_are_all_drift_does_not_import_bound() {
		// The firmware lints the generated plan with `-D warnings`, so an unused import is a
		// build that fails.
		let built = build_with_setpoint(&format!(
			"[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\n{}",
			drift_alarm(10.0, 6.0)
		))
		.unwrap();
		let rust = to_rust(&built.plan);
		assert!(rust.contains("use vag_dash_render::alarm::{Alarm, ChannelId, PageId, Rule};"), "{rust}");
		assert!(!rust.contains("Bound"), "{rust}");
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
	fn an_alarm_finds_its_page_by_the_row_whatever_the_page_spells_it() {
		let built = build_with_setpoint(
			"[[channel]]\nref = \"01:IDE00191\"\nalarm_max = 2.5\nalarm_max_off = 2.3\n[[page]]\nkind = \"values\"\ntitle = \"B\"\ncells = [\"01:202A\"]\n",
		)
		.unwrap();
		assert_eq!(built.plan.channels.len(), 1, "one row, one channel");
		assert!(
			built
				.plan
				.pages
				.iter()
				.any(|p| matches!(p, Page::Values { title, cells, .. } if title == "B" && cells == &vec![0]))
		);
		assert_eq!(
			(built.plan.alarms[0].channel, built.plan.alarms[0].page),
			(0, 0),
			"the first page that shows it"
		);
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
		use vag_dash_render::alarm::{Bound as DeviceBound, ChannelId, PageId, Rule};
		use vag_dash_render::plan::Page as DevicePage;
		let built = build_with_alarms(["", "alarm_min = -2.0\nalarm_min_off = -1.5", ""], "").unwrap();
		let device = built.plan.to_device();
		let rust = to_rust(&built.plan);

		assert_eq!((device.vin, device.language), ("TESTVIN0000000001", "en"));
		assert_eq!(device.units.len(), 1);
		assert_eq!((device.units[0].request, device.units[0].part_number), (ENGINE, "PART1"));
		assert_eq!(device.channels.len(), 3);
		for (c, d) in built.plan.channels.iter().zip(device.channels) {
			// The unit by its index, the board's join: the plan's units name the request id.
			assert_eq!(device.unit(d).request, c.unit);
			assert_eq!((d.did, d.bit_offset, d.bit_length), (c.did, c.bit_offset, c.bit_length));
			assert_eq!((d.factor, d.offset, d.hz), (c.factor as f32, c.offset as f32, c.hz as f32));
			assert_eq!((d.label, d.unit_text, d.decimals), (c.label.as_str(), c.unit_text.as_str(), c.decimals));
			// The source says the same numbers the memory holds.
			assert!(rust.contains(&format!("did: 0x{:04X}, bit_offset: {}", d.did, d.bit_offset)), "{rust}");
		}
		assert_eq!(
			device.pages,
			[
				DevicePage::Values {
					title: "A",
					cells: &[0, 1],
					volts: None
				},
				DevicePage::Values {
					title: "B",
					cells: &[2],
					volts: None
				},
				DevicePage::Chart {
					title: "",
					channel: 0,
					slot: 0,
					min: 0.0,
					max: 10.0,
					volts: None
				},
			]
		);
		assert!(device.joins_hold(), "every index the board takes names something");
		assert_eq!(device.alarms.len(), 1);
		assert_eq!((device.alarms[0].channel, device.alarms[0].page), (ChannelId(1), PageId(0)));
		assert_eq!(
			device.alarms[0].rule,
			Rule::Bounds {
				max: None,
				min: Some(DeviceBound { at: -2.0, off: -1.5 }),
			}
		);
		assert!(device.watched(1) && !device.watched(0), "the board's own question answers from it");
	}

	#[test]
	fn a_drift_rule_in_memory_pairs_the_same_specified_value_as_the_source() {
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
		assert_eq!(device.alarms[0].channel, ChannelId(actual));
		assert_eq!(
			device.alarms[0].rule,
			Rule::Drift {
				specified: ChannelId(specified),
				percent: 10.0,
				off_percent: 6.0,
				hold_ms: 1000,
				min_setpoint: 0.5
			}
		);
		assert!(device.watched(specified) && device.watched(actual));
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
			Language::En,
		)
		.unwrap();
		let json = built.plan.to_json();
		let back = from_json(&json).unwrap();
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
			Language::En,
		)
		.unwrap();
		let c = &built.plan.channels[0];
		assert_eq!(c.unit, GEARBOX);
		let device = built.plan.to_device().channels[0];
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
				cells: vec![0, 1],
				volts: true
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
			Language::En,
		)
		.unwrap();
		let c = &built.plan.channels[0];
		assert!(c.proven);
		assert_eq!(c.factor, 0.002);
		assert_eq!(c.label, "Boost (driven)");
		assert!(built.notes[0].contains("at 2 Hz, measured on a car (IDE00191)"), "{}", built.notes[0]);
		assert!(built.advice.is_empty(), "how a channel resolved is no advice: {:?}", built.advice);
		// The same as a row of `-v`'s channel table, and a line of the firmware build's log only.
		assert_eq!(built.channels.len(), 1);
		assert_eq!(built.channels[0].line(), built.notes[0]);
		assert_eq!(
			built.channels[0].cells(),
			["Boost (driven)", "01:202A", "202A@0/16", "u BE", "0.002", "0", "2", "measured IDE00191"]
		);
		assert!(!built.prose().any(|n| *n == built.notes[0]));
	}

	/// What each unit's catalog held and which rows a `ref` was chosen from: on stderr at `-vv`,
	/// and not even formatted below it.
	#[test]
	fn the_lookups_are_wire_lines_and_nothing_else_is_said() {
		use crate::verbosity::Capture;
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x202A, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let store = CatalogStore::open(here.path().join("none"));
		let run = |level| {
			let capture = Capture::start(level);
			build(
				&input(&["01:IDE00191"], &values_page(&["01:IDE00191"])),
				&store,
				&extracted,
				&[identity(ENGINE, "PART1", "EV_Test")],
				Language::En,
			)
			.unwrap();
			capture.finish()
		};
		for level in [0, 1] {
			assert_eq!(run(level), crate::verbosity::Captured::default(), "level {level}");
		}
		let wire = run(2);
		assert_eq!(wire.stdout, "");
		let lines: Vec<&str> = wire.stderr.lines().collect();
		assert_eq!(lines.len(), 2, "{lines:?}");
		assert!(lines[0].starts_with("catalog 01: part PART1, odx EV_Test 001004 → "), "{lines:?}");
		assert!(lines[0].ends_with(" rows, 0 measured on a car"), "{lines:?}");
		assert!(lines[1].starts_with("lookup 01:IDE00191: 1 of "), "{lines:?}");
		assert!(lines[1].ends_with(" rows — 202A@0 Boost linear declared"), "{lines:?}");
	}

	#[test]
	fn a_lookup_that_finds_nothing_says_how_many_rows_it_looked_among() {
		let reference = Reference::parse("01:IDE99999").unwrap();
		assert_eq!(
			candidate_lookup(&reference, &[], 12),
			"lookup 01:IDE99999: none of the 12 rows on its unit"
		);
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
			levels: vec![vag_data_labels::Level::point(0, "no"), vag_data_labels::Level::point(1, "yes")],
		};
		let value = reading(0xF405, "Engine Coolant Temperature", "IDE00025", 0, 8, false, true, 1.0, -40.0);
		let extracted = extracted_with(here.path(), &[("EV_Test_001", vec![flag, value])], &[]);
		let store = CatalogStore::open(here.path().join("proven"));
		let built = build(
			&input(&["01:IDE00025"], &values_page(&["01:IDE00025"])),
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test")],
			Language::En,
		)
		.unwrap();
		assert_eq!(built.plan.channels.len(), 1);
		// The line `docs/dash/dash-toml.md` decodes under "Reading the build's output".
		assert_eq!(
			built.notes[0],
			"Engine Coolant Temperature ← 01:IDE00025 F405@0/8 u BE ×1 -40 at 2 Hz, from the project's data (IDE00025)"
		);
		assert_eq!(
			(
				built.plan.channels[0].did,
				built.plan.channels[0].bit_length,
				built.plan.channels[0].offset
			),
			(0xF405, 8, -40.0)
		);
	}

	/// Three ways to name nothing on a recorded unit, each said apart, each with what to do
	/// next (UX review, 2026-10-19: all three read "the car's variant does not declare this
	/// channel and nothing has proven it").
	#[test]
	fn an_unknown_id_a_name_and_a_unit_without_channels_are_three_refusals() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![
					reading(0xF405, "Engine Coolant Temperature", "IDE00025", 0, 8, false, true, 1.0, -40.0),
					reading(0x202A, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0),
				],
			)],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let units = [identity(ENGINE, "PART1", "EV_Test"), identity(GEARBOX, "PART2", "EV_Nothing")];
		let refuse = |r: &str| build(&input(&[r], &values_page(&[r])), &store, &extracted, &units, Language::En).unwrap_err();

		let unknown = refuse("01:IDE99999");
		assert_eq!(unknown, Error::Undeclared(Reference::parse("01:IDE99999").unwrap()));
		let said = unknown.to_string();
		assert!(said.starts_with("01:IDE99999: unit 01 has no channel with the id IDE99999"), "{said}");
		assert!(said.contains(FIND_IDS), "the next step: {said}");
		// The identifier form, in its own words.
		let at = refuse("01:1234@8");
		assert_eq!(at, Error::Undeclared(Reference::parse("01:1234@8").unwrap()));
		assert!(
			at.to_string()
				.starts_with("01:1234@8: unit 01 has no channel at identifier 1234, bit 8 in this car's project data"),
			"{at}"
		);

		let name = refuse("01:Engine Coolant Temperature");
		assert_eq!(
			name,
			Error::NameNotId {
				reference: Reference::parse("01:Engine Coolant Temperature").unwrap(),
				ids: vec!["01:IDE00025".to_string()],
				not_linear: false,
			}
		);
		assert_eq!(
			name.to_string(),
			"01:Engine Coolant Temperature: \"Engine Coolant Temperature\" is the channel's name — write its id: 01:IDE00025"
		);
		// Not the case of the project's spelling, and not a name of this car's: still a name.
		assert!(matches!(refuse("01:engine coolant temperature"), Error::NameNotId { ids, .. } if ids == ["01:IDE00025"]));
		// A text with a space no row is named: a name, or an id typed wrong — not "a name" for sure.
		let stranger = refuse("01:Oil Level");
		assert!(
			matches!(&stranger, Error::NameNotId { ids, not_linear: false, .. } if ids.is_empty()),
			"{stranger}"
		);
		assert_eq!(
			stranger.to_string(),
			format!("01:Oil Level: \"Oil Level\" is a name or a mistyped id, and dash.toml takes a channel's id — {FIND_IDS}")
		);

		let empty = refuse("02:IDE00191");
		assert_eq!(empty, Error::NoChannels(Reference::parse("02:IDE00191").unwrap()));
		assert!(
			empty
				.to_string()
				.contains("unit 02 is in the car's record, but the project data has no channels for its variant"),
			"{empty}"
		);

		// The three say three different things.
		let texts = [unknown.to_string(), name.to_string(), empty.to_string()];
		assert!(texts[0] != texts[1] && texts[1] != texts[2] && texts[0] != texts[2]);
	}

	/// A name's suggestions are what a numeric cell takes, each in a spelling the build reads as
	/// that row (review, 2026-10-19): no enumeration, and no text id two linear rows share.
	#[test]
	fn a_name_suggests_only_linear_rows_spelled_the_way_the_build_takes_them() {
		let here = tempfile::tempdir().unwrap();
		let enumeration = |did: u16, name: &str, text_id: &str| Reading {
			unit: None,
			scaling: Scaling::Enum {
				levels: vec![Level::point(0, "off"), Level::point(1, "on")],
			},
			..reading(did, name, text_id, 0, 8, false, true, 1.0, 0.0)
		};
		let extracted = extracted_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![
					// One name on an enumeration and a linear row sharing a text id: the linear
					// one, by that id — the build's pick of the two.
					enumeration(0x1003, "Gear", "IDE00003"),
					reading(0x1006, "Gear", "IDE00003", 0, 8, false, true, 1.0, 0.0),
					// One name and one text id on two linear rows: the id names neither, so each
					// by identifier.
					reading(0x1007, "Load", "IDE00007", 0, 8, false, true, 1.0, 0.0),
					reading(0x1008, "Load", "IDE00007", 0, 8, false, true, 1.0, 0.0),
					// A name only an enumeration carries.
					enumeration(0x1009, "Door State", "IDE00009"),
				],
			)],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let units = [identity(ENGINE, "PART1", "EV_Test")];
		let refuse = |r: &str| build(&input(&[r], &values_page(&[r])), &store, &extracted, &units, Language::En).unwrap_err();

		assert!(matches!(refuse("01:Gear"), Error::NameNotId { ids, .. } if ids == ["01:IDE00003"]));
		assert!(matches!(refuse("01:Load"), Error::NameNotId { ids, .. } if ids == ["01:1007", "01:1008"]));
		// Every suggestion builds, as the very row it was offered for.
		for (spelled, did) in [("01:IDE00003", 0x1006), ("01:1007", 0x1007), ("01:1008", 0x1008)] {
			let built =
				build(&input(&[spelled], &values_page(&[spelled])), &store, &extracted, &units, Language::En).unwrap_or_else(|e| panic!("{spelled}: {e}"));
			assert_eq!(built.plan.channels[0].did, did, "{spelled}");
		}
		let door = refuse("01:Door State");
		assert!(
			matches!(&door, Error::NameNotId { ids, not_linear: true, .. } if ids.is_empty()),
			"{door}"
		);
		assert!(
			door
				.to_string()
				.starts_with("01:Door State: \"Door State\" is the name of a channel the board cannot show"),
			"{door}"
		);
	}

	#[test]
	fn a_missing_build_input_says_what_to_write_and_where_without_an_errno() {
		let here = tempfile::tempdir().unwrap();
		let path = here.path().join("dash.toml");
		// `--input` names a file that is not there: a path typed wrong, nothing to start from.
		let given = format!("{:#}", read_inputs("TESTVIN0000000001", Some(&path)).unwrap_err());
		assert_eq!(given, format!("no file at {}", path.display()));
		// The car's own dash.toml is not there yet: what to write.
		let why = crate::report::render(&missing_input("TESTVIN0000000001", &path, false), 0, crate::report::Colour::Never);
		assert!(why.starts_with("error: no dash.toml for TESTVIN0000000001 yet\n\n"), "{why}");
		assert!(why.contains(&path.display().to_string()), "where: {why}");
		assert!(
			why.contains("help: lay out its pages with `vagcan dev dash edit TESTVIN0000000001`"),
			"what: {why}"
		);
		assert!(why.contains(REFERENCE_DOC), "{why}");
		// No example file inside an error: the smallest one is `--help`'s.
		assert!(!why.contains("[[channel]]"), "{why}");
		assert!(!why.contains("os error") && !why.contains("No such file"), "no errno: {why}");
	}

	#[test]
	fn the_smallest_input_builds_and_is_the_one_the_readme_shows() {
		let text = SMALLEST_INPUT.replace("<VIN>", "TESTVIN0000000001");
		let parsed = parse_input(&text).unwrap();
		assert_eq!(parsed.channels.len(), 1);
		assert_eq!(parsed.pages.len(), 1);
		let readme = include_str!("../../../../README.md");
		assert!(
			readme.contains(&format!("```toml\n{SMALLEST_INPUT}```")),
			"README's \"Building the dash\" shows SMALLEST_INPUT verbatim"
		);
		assert!(
			std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../"))
				.join(REFERENCE_DOC)
				.is_file()
		);
	}

	#[test]
	fn a_list_written_with_one_pair_of_brackets_is_told_to_have_two() {
		for (name, text) in [
			(
				"channel",
				"vin = \"V\"\n[channel]\nref = \"01:IDE00025\"\n[[page]]\nkind = \"values\"\ncells = [\"01:IDE00025\"]\n",
			),
			(
				"page",
				"vin = \"V\"\n[[channel]]\nref = \"01:IDE00025\"\n[page]\nkind = \"values\"\ncells = [\"01:IDE00025\"]\n",
			),
			(
				"button",
				"vin = \"V\"\n[[channel]]\nref = \"01:IDE00025\"\n[[page]]\nkind = \"values\"\ncells = [\"01:IDE00025\"]\n[button]\npin = 3\naction = \"next\"\n",
			),
		] {
			let Error::Parse(why) = parse_input(text).unwrap_err() else {
				panic!("not a parse refusal");
			};
			assert!(
				why.contains(&format!("[{name}] has one pair of brackets — write [[{name}]], two pairs")),
				"{why}"
			);
			assert!(why.contains("line "), "it points at the line: {why}");
		}
	}

	#[test]
	fn the_record_refusal_names_the_commands_that_write_it_in_one_line() {
		let err = no_record("TESTVIN0000000001", Path::new("/home/x/.vagcan/cars/TESTVIN0000000001/units.json"));
		assert!(err.to_string().ends_with("units.json"), "{err}");
		let said = crate::report::render(&err, 0, crate::report::Colour::Never);
		assert!(said.starts_with("error: no record of TESTVIN0000000001's control units at "), "{said}");
		let help = said.lines().find(|l| l.starts_with("help: ")).expect("a help line");
		assert!(help.contains("`vagcan units --identify`") && help.contains("`vagcan watch`"), "{said}");
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
			Language::En,
		)
		.unwrap_err();
		assert_eq!(err, Error::UnknownUnit(GEARBOX), "a unit the car's record never saw");
		assert!(err.to_string().contains("vagcan units --identify"), "and what writes the record: {err}");
	}

	#[test]
	fn a_scaling_the_device_cannot_apply_is_refused() {
		let here = tempfile::tempdir().unwrap();
		let mut gear = reading(0x3816, "Gear", "IDE00010", 0, 8, false, true, 1.0, 0.0);
		gear.scaling = Scaling::Enum {
			levels: vec![vag_data_labels::Level::point(1, "N"), vag_data_labels::Level::point(2, "1")],
		};
		let extracted = extracted_with(here.path(), &[("EV_Test_001", vec![gear])], &[]);
		let store = CatalogStore::open(here.path().join("proven"));
		let err = build(
			&input(&["02:IDE00010"], &values_page(&["02:IDE00010"])),
			&store,
			&extracted,
			&[identity(GEARBOX, "PART2", "EV_Test")],
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
			Language::En,
		)
		.unwrap_err();
		assert!(matches!(err, Error::PageRefersToUnknown { page: 1, .. }), "{err}");
		let err = build(
			&input(&["01:IDE00191"], "[[page]]\nkind = \"chart\"\ncell = \"01:IDE00191\"\nmin = 2\nmax = 1\n"),
			&store,
			&extracted,
			&units,
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
			Language::En,
		)
		.unwrap();
		let rust = to_rust(&built.plan);
		assert!(rust.contains("pub static PLAN: Plan"), "{rust}");
		assert!(rust.contains("language: \"ru\""), "{rust}");
		assert!(rust.contains("did: 0xF405, bit_offset: 0, bit_length: 8, signed: false, big_endian: true, factor: 1.0, offset: -40.0, decimals: 0, unit_text: \"°C\", label: \"ОЖ\", proven: false, hz: 2.0"), "{rust}");
		assert!(rust.contains("static CELLS_0: [u16; 1] = [0];"), "{rust}");
		assert!(rust.contains("Page::Values { title: \"T\", cells: &CELLS_0, volts: None }"), "{rust}");
		assert!(rust.contains("Channel { unit: 0, did: 0xF405,"), "a unit by its index: {rust}");
		assert!(
			rust.contains("Unit { request: 0x7E0, response: 0x7E8, part_number: \"PART1\" }, // 0"),
			"{rust}"
		);
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
			Language::En,
		)
		.unwrap();
		assert!(built.plan.channels[0].proven);
		assert_eq!(built.plan.channels[0].factor, 0.002);
	}

	/// A standard OBD-II row builds — the standard mandates it — and the log says the car
	/// may still not carry it; a row with no text id is not therefore the standard's.
	#[test]
	fn a_standard_obd_row_builds_and_the_log_says_so() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x2029, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let units = [identity(ENGINE, "PART1", "EV_Test")];
		let built = build(&input(&["01:F423"], &values_page(&["01:F423"])), &store, &extracted, &units, Language::En).unwrap();
		assert_eq!(built.plan.channels[0].did, 0xF423);
		assert!(
			built
				.notes
				.iter()
				.any(|n| n.starts_with("01:F423: a standard OBD-II row") && n.contains("may still not carry") && !n.contains("record")),
			"{:?}",
			built.notes
		);

		// A row with no text id is not therefore the standard's: a VCDS row
		// gives up an id ODIS gives another field, and one from an install whose
		// text table is shut has none at all.
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x2029, "Boost", "", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let built = build(&input(&["01:2029"], &values_page(&["01:2029"])), &store, &extracted, &units, Language::En).unwrap();
		assert!(!built.notes.iter().any(|n| n.contains("standard OBD-II")), "{:?}", built.notes);
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
			Language::En,
		)
		.unwrap();
		assert_eq!(built.plan.channels.iter().map(|c| c.hz).collect::<Vec<_>>(), [10.0, DEFAULT_HZ]);
		let rust = to_rust(&built.plan);
		assert!(rust.contains("proven: false, hz: 10.0, setpoint: None, zero_at_rest: false }"), "{rust}");
		assert!(rust.contains("proven: false, hz: 2.0, setpoint: None, zero_at_rest: false }"), "{rust}");
		assert!(built.notes[0].contains("at 10 Hz"), "{}", built.notes[0]);
	}

	#[test]
	fn a_rate_that_is_not_a_positive_number_within_the_ceiling_is_refused() {
		// Past 50 Hz, the guard's floor of 20 ms a read, which the panel keeps too.
		for bad in ["0", "-1", "50.5", "100", "\"fast\"", "nan"] {
			let text = format!(
				"vin = \"X\"\n[[channel]]\nref = \"01:IDE00025\"\nhz = {bad}\n{}",
				values_page(&["01:IDE00025"])
			);
			let err = parse_input(&text).unwrap_err();
			assert!(err.to_string().contains("hz must be"), "{bad}: {err}");
		}
		let text = format!(
			"vin = \"X\"\n[[channel]]\nref = \"01:IDE00025\"\nhz = 50\n{}",
			values_page(&["01:IDE00025"])
		);
		assert_eq!(parse_input(&text).unwrap().channels[0].hz, Some(MAX_HZ));
	}

	#[test]
	fn alarms_reach_both_outputs_in_the_channels_order_by_plan_index() {
		let built = build_with_alarms(
			[
				"alarm_min = 0\nalarm_min_off = 1",
				"alarm_max = 10\nalarm_max_off = 8",
				"alarm_max = 105\nalarm_min = 40",
			],
			"",
		)
		.unwrap();
		assert_eq!(
			built.plan.alarms,
			vec![
				Alarm {
					channel: 0,
					page: 0,
					rule: AlarmRule::Bounds {
						max: None,
						min: Some(Bound { at: 0.0, off: 1.0 }),
					},
				},
				Alarm {
					channel: 1,
					page: 0,
					rule: AlarmRule::Bounds {
						max: Some(Bound { at: 10.0, off: 8.0 }),
						min: None,
					},
				},
				Alarm {
					channel: 2,
					page: 1,
					rule: AlarmRule::Bounds {
						max: Some(Bound { at: 105.0, off: 99.8 }),
						min: Some(Bound { at: 40.0, off: 42.0 }),
					},
				},
			],
			"the channels' order is the priority; 5 % inside each bound by default"
		);
		for note in [
			"alarm: 01:IDE00002 at or above 10, off at or under 8 → page 1 \"A\"",
			"alarm: 01:IDE00003 at or above 105, off at or under 99.8 (its default); at or below 40, off at or above 42 (its default) → page 2 \"B\"",
		] {
			assert!(built.notes.iter().any(|n| n == note), "{note}: {:?}", built.notes);
		}

		let json = built.plan.to_json();
		assert_eq!(from_json(&json).unwrap(), built.plan);
		assert!(json.contains("\"kind\": \"bounds\""), "{json}");

		let rust = to_rust(&built.plan);
		assert!(
			rust.contains("use vag_dash_render::alarm::{Alarm, Bound, ChannelId, PageId, Rule};"),
			"{rust}"
		);
		assert!(
			rust.contains("alarms: &ALARMS, stalk: None, stopwatch: None, buttons: &[], standstill: None }"),
			"{rust}"
		);
		assert!(rust.contains("static ALARMS: [Alarm; 3] = ["), "{rust}");
		assert!(
			rust.contains("\tAlarm { channel: ChannelId(1), page: PageId(0), rule: Rule::Bounds { max: Some(Bound { at: 10.0, off: 8.0 }), min: None } },"),
			"{rust}"
		);

		// A plan with no alarms imports nothing unused.
		let mut none = built.plan.clone();
		none.alarms.clear();
		let rust = to_rust(&none);
		assert!(rust.contains("use vag_dash_render::alarm::Alarm;\n"), "{rust}");
		assert!(rust.contains("static ALARMS: [Alarm; 0] = ["), "{rust}");
		assert!(!rust.contains("ChannelId"), "{rust}");
	}

	#[test]
	fn an_alarm_the_board_could_not_honour_is_refused_and_the_message_says_why() {
		let refused = |keys: [&str; 3]| build_with_alarms(keys, "").unwrap_err().to_string();
		// Read when the input is: by line and channel.
		let parse = |keys: &str| {
			let why = refused(["", keys, ""]);
			let (_, why) = why.split_once("[[channel]] 2: ").unwrap_or_else(|| panic!("{why}"));
			why.to_string()
		};
		assert_eq!(
			parse("alarm_max_off = 3"),
			"alarm_max_off needs alarm_max: it is the value the alarm_max alarm goes off at"
		);
		assert_eq!(
			parse("alarm_min_off = 3"),
			"alarm_min_off needs alarm_min: it is the value the alarm_min alarm goes off at"
		);
		assert_eq!(
			parse("alarm_max = 10\nalarm_max_off = 12"),
			"alarm_max_off 12 is not under alarm_max 10 — the alarm goes off once the value is back at alarm_max_off"
		);
		assert_eq!(
			parse("alarm_max = 10\nalarm_max_off = 10"),
			"alarm_max_off 10 is not under alarm_max 10 — the alarm goes off once the value is back at alarm_max_off",
			"an off value at the bound is no off value"
		);
		assert_eq!(
			parse("alarm_max = 100.000001\nalarm_max_off = 100.0"),
			"alarm_max_off 100 is not under alarm_max 100 — the alarm goes off once the value is back at alarm_max_off",
			"apart in the file, one value in the board's f32"
		);
		assert_eq!(
			parse("alarm_min = 0\nalarm_min_off = -1"),
			"alarm_min_off -1 is not over alarm_min 0 — the alarm goes off once the value is back at alarm_min_off"
		);
		assert_eq!(parse("alarm_max = \"hot\""), "alarm_max must be a number, not a string");
		assert_eq!(
			parse("alarm_min = 1e40"),
			format!("alarm_min 1e40 is not a number the board's 32-bit float holds (within ±{:e})", f32::MAX)
		);
		assert_eq!(
			parse("drift_percent = 10"),
			"a drift alarm takes all four of drift_percent, drift_off_percent, drift_hold_ms, drift_min_setpoint — missing drift_off_percent, drift_hold_ms, drift_min_setpoint"
		);
		assert_eq!(
			parse("drift_hold_ms = 10"),
			"a drift alarm takes all four of drift_percent, drift_off_percent, drift_hold_ms, drift_min_setpoint — missing drift_percent, drift_off_percent, drift_min_setpoint"
		);
		assert_eq!(
			parse("drift_percent = 10\ndrift_off_percent = 1e-50\ndrift_hold_ms = 0\ndrift_min_setpoint = 0"),
			"drift_off_percent 1e-50 is too small for the board, which would hold it as 0"
		);
		assert_eq!(
			parse("drift_percent = 10\ndrift_off_percent = 5\ndrift_hold_ms = -1\ndrift_min_setpoint = 0"),
			"drift_hold_ms must be whole milliseconds, 0 or more"
		);
		assert_eq!(
			parse("drift_percent = 10\ndrift_off_percent = 5\ndrift_hold_ms = 0\ndrift_min_setpoint = -1"),
			"drift_min_setpoint -1 is below zero"
		);
		// Resolved: the bounds against each other, with the defaults in.
		assert_eq!(
			refused(["", "alarm_max = 10\nalarm_min = 9", ""]),
			"01:IDE00002: alarm_min_off 10 (its default) is not under alarm_max_off 9 (its default) — no value would be off for both; write the two so that alarm_min_off is under alarm_max_off"
		);
		assert_eq!(
			refused(["", "alarm_max = 10\nalarm_max_off = 5\nalarm_min = 2\nalarm_min_off = 6", ""]),
			"01:IDE00002: alarm_min_off 6 is not under alarm_max_off 5 — no value would be off for both; write the two so that alarm_min_off is under alarm_max_off"
		);
		build_with_alarms(["", "alarm_max = 10\nalarm_max_off = 6\nalarm_min = 2\nalarm_min_off = 5", ""], "").unwrap();
	}

	/// The off value an alarm without one goes off at: 5 % of the bound inside it, never less than
	/// one step of the channel's resolution — what one count of its raw value is worth — and not of
	/// its display (owner, 2026-10-19).
	#[test]
	fn the_default_off_value_is_five_percent_inside_and_never_under_one_step_of_the_resolution() {
		let off = |keys: &str, which: usize| -> (f64, f64) {
			let built = build_with_alarms(["", "", keys], "").unwrap();
			match &built.plan.alarms[0].rule {
				AlarmRule::Bounds { max, min } => {
					let b = [max, min][which].expect("the bound is there");
					(b.at, b.off)
				}
				other => panic!("{other:?}"),
			}
		};
		// Channel 2 scales by 0.1: its step is 0.1.
		assert_eq!(off("alarm_max = 125", 0), (125.0, 118.8), "118.75, to the step's one place");
		assert_eq!(off("alarm_max = -20", 0), (-20.0, -21.0), "5 % of the magnitude, inside");
		assert_eq!(off("alarm_min = 40", 1), (40.0, 42.0));
		assert_eq!(off("alarm_min = -6", 1), (-6.0, -5.7));
		assert_eq!(off("alarm_max = 1", 0), (1.0, 0.9), "5 % is 0.05, under one step");
		assert_eq!(off("alarm_min = 0", 1), (0.0, 0.1), "a bound at zero is one step");
		// The resolution, not the display: shown without decimals, the channel still steps by 0.1.
		assert_eq!(off("decimals = 0\nalarm_max = 1", 0), (1.0, 0.9));
		// Rounded, so the build's line says a number: 1.33 + 0.1 is 1.4300000000000002.
		assert_eq!(off("alarm_min = 1.33", 1), (1.33, 1.4));
		let built = build_with_alarms(["", "", "alarm_min = 1.33"], "").unwrap();
		assert!(
			built
				.notes
				.iter()
				.any(|n| n.contains("at or below 1.33, off at or above 1.4 (its default)")),
			"{:?}",
			built.notes
		);
		// A channel that counts by one steps by one, however many decimals it is shown with.
		for keys in ["alarm_max = 5", "decimals = 2\nalarm_max = 5"] {
			let built = build_with_alarms([keys, "", ""], "").unwrap();
			assert!(
				matches!(&built.plan.alarms[0].rule, AlarmRule::Bounds { max: Some(Bound { at, off }), .. } if (*at, *off) == (5.0, 4.0)),
				"{keys}: {:?}",
				built.plan.alarms
			);
		}
	}

	/// The bus lines of a build: the estimate's own line first, then one per page and a run.
	fn bus_lines(built: &Built) -> Vec<&str> {
		built.notes.iter().filter(|n| n.starts_with("bus: ")).map(String::as_str).collect()
	}

	/// The bus lines are an estimate, the board's planner run over the plan
	/// (`.archive/tasks/done/dash/24-bus-time-check.md`): what the estimate is, then each page's
	/// share of the bus. The pages not on the glass are read once a second, and their share counts.
	#[test]
	fn the_bus_lines_estimate_each_page_on_the_glass() {
		let built = build_with_alarms(["alarm_max = 5\nhz = 10", "alarm_max = 5\nhz = 2", ""], "").unwrap();
		assert_eq!(
			bus_lines(&built),
			[
				"bus: an estimate at 34 ms an exchange, an upper bound from a BLE recording on the reference car; it refuses nothing",
				"bus: page 1 \"A\" ~34 %",
				"bus: page 2 \"B\" ~34 %",
				"bus: page 3 ~34 %",
			]
		);
		let quiet = build_with_alarms(NO_ALARMS, "").unwrap();
		assert_eq!(
			&bus_lines(&quiet)[1..],
			[r#"bus: page 1 "A" ~7 %"#, r#"bus: page 2 "B" ~7 %"#, "bus: page 3 ~7 %"]
		);
	}

	/// Three channels of one unit at 50 Hz: one exchange carries all three, and fifty exchanges a
	/// second is more than a bus of 34 ms an exchange makes. Built all the same (2026-10-20): the
	/// line says what the estimate would thin.
	#[test]
	fn a_page_the_estimate_would_thin_is_built_and_its_reads_said() {
		let built = build_with_alarms(["alarm_max = 5\nhz = 50", "alarm_max = 5\nhz = 50", "alarm_max = 5\nhz = 50"], "").unwrap();
		assert_eq!(
			bus_lines(&built),
			[
				"bus: an estimate at 34 ms an exchange, an upper bound from a BLE recording on the reference car; it refuses nothing",
				"bus: page 1 \"A\" ~100 % — reads 01:IDE00001 One 29.4/s of 50, 01:IDE00002 Two 29.4/s of 50, 01:IDE00003 Three 29.4/s of 50",
				"bus: page 2 \"B\" ~100 % — reads 01:IDE00001 One 29.4/s of 50, 01:IDE00002 Two 29.4/s of 50, 01:IDE00003 Three 29.4/s of 50",
				"bus: page 3 ~100 % — reads 01:IDE00001 One 29.4/s of 50, 01:IDE00002 Two 29.4/s of 50, 01:IDE00003 Three 29.4/s of 50",
			]
		);
		// The same as rows, and every page among the over-budget ones.
		let reads = "01:IDE00001 One 29.4/s of 50, 01:IDE00002 Two 29.4/s of 50, 01:IDE00003 Three 29.4/s of 50";
		let row = |page: &str| BusRow {
			page: page.into(),
			load: "~100 %".into(),
			short_of: reads.into(),
		};
		assert_eq!(built.bus, [row("1 \"A\""), row("2 \"B\""), row("3")]);
		assert_eq!(built.over_budget, [OverBudget::Page(1), OverBudget::Page(2), OverBudget::Page(3)]);
		// Each row's line is in the log for the firmware build, and not in what `-v` prints as lines.
		assert!(built.tabled.iter().all(|t| built.notes.contains(t)));
		assert!(
			!built.prose().any(|n| n.starts_with("bus: page")),
			"{:?}",
			built.prose().collect::<Vec<_>>()
		);
		assert!(built.prose().any(|n| n.starts_with("bus: an estimate")));
		let calm = build_with_alarms(NO_ALARMS, "").unwrap();
		assert!(calm.over_budget.is_empty(), "{:?}", calm.over_budget);
	}

	/// With the lever, each page is estimated with cruise off — the lever at 20 a second — and on,
	/// at 2; a stopwatch run reads it at its own rate whatever the gate, and has one share.
	#[test]
	fn the_bus_lines_say_cruise_off_and_on_and_a_stopwatch_run() {
		let built = build_with_lever(&format!("{LEVER}{WATCH}{STRIP}")).unwrap();
		assert_eq!(
			bus_lines(&built),
			[
				"bus: an estimate at 34 ms an exchange, an upper bound from a BLE recording on the reference car; it refuses nothing; the lever is read 20 times a second with cruise off, 2 with it on",
				"bus: page 1 \"T\" cruise off ~85 %; cruise on ~24 %",
				"bus: a stopwatch run ~100 % — reads 01:IDE00010 Speed 14.4/s of 50",
			]
		);
		let row = |page: &str, load: &str, short_of: &str| BusRow {
			page: page.into(),
			load: load.into(),
			short_of: short_of.into(),
		};
		assert_eq!(
			built.bus,
			[
				row("1 \"T\" cruise off", "~85 %", ""),
				row("1 \"T\" cruise on", "~24 %", ""),
				row("stopwatch", "~100 %", "01:IDE00010 Speed 14.4/s of 50"),
			]
		);
		assert_eq!(built.over_budget, [OverBudget::Stopwatch]);
	}

	/// Shaped like the owner's plan, which runs on the car: eight full values pages, a strip, the
	/// lever, and six channels with an alarm at 5 Hz. The lever alone is 20 exchanges a second on a
	/// unit of its own, and at 34 ms the estimate thins it — and the plan builds (2026-10-20).
	#[test]
	fn a_plan_shaped_like_the_owners_builds_and_says_what_the_estimate_thins() {
		let rows = (0..6u16)
			.map(|i| {
				(
					ENGINE,
					reading(0x4101 + i, "Alarmed", &format!("IDE0010{i}"), 0, 8, false, true, 1.0, 0.0),
				)
			})
			.collect();
		let mut input: String = (0..6)
			.map(|i| format!("[[channel]]\nref = \"01:IDE0010{i}\"\nhz = 5\nalarm_max = 5\n"))
			.collect();
		let all: Vec<String> = (0..6).map(|i| format!("01:IDE0010{i}")).chain(["01:IDE00001".to_string()]).collect();
		for page in 0..7 {
			let cells: Vec<&str> = (0..4).map(|k| all[(page + k) % all.len()].as_str()).collect();
			input.push_str(&values_page_titled(&format!("P{page}"), &cells));
		}
		input.push_str(STRIP);
		input.push_str(LEVER);
		let built = build_with_lever_and(&input, Extra { rows, ..Extra::default() }).unwrap();
		assert_eq!(built.plan.pages.len(), MAX_PAGES);
		assert_eq!(
			bus_lines(&built),
			[
				"bus: an estimate at 34 ms an exchange, an upper bound from a BLE recording on the reference car; it refuses nothing; the lever is read 20 times a second with cruise off, 2 with it on",
				"bus: page 1 \"T\" cruise off ~88 % — reads the lever 18.9/s of 20; cruise on ~31 %",
				"bus: page 2 \"P0\" cruise off ~85 %; cruise on ~24 %",
				"bus: page 3 \"P1\" cruise off ~85 %; cruise on ~24 %",
				"bus: page 4 \"P2\" cruise off ~85 %; cruise on ~24 %",
				"bus: page 5 \"P3\" cruise off ~88 % — reads the lever 18.9/s of 20; cruise on ~31 %",
				"bus: page 6 \"P4\" cruise off ~88 % — reads the lever 18.9/s of 20; cruise on ~31 %",
				"bus: page 7 \"P5\" cruise off ~88 % — reads the lever 18.9/s of 20; cruise on ~31 %",
				"bus: page 8 \"P6\" cruise off ~88 % — reads the lever 18.9/s of 20; cruise on ~31 %",
			]
		);
	}
	/// Six alarm channels of one unit at 50 Hz beside the lever: one exchange carries the six, fifty
	/// a second, so cruise on thins them too — each run says its own reads.
	#[test]
	fn each_cruise_run_says_its_own_reads() {
		let rows = (0..6u16)
			.map(|i| {
				(
					ENGINE,
					reading(0x4101 + i, "Alarmed", &format!("IDE0010{i}"), 0, 8, false, true, 1.0, 0.0),
				)
			})
			.collect();
		let mut input: String = (0..6)
			.map(|i| format!("[[channel]]\nref = \"01:IDE0010{i}\"\nhz = 50\nalarm_max = 5\n"))
			.collect();
		input.push_str(&values_page_titled("A", &["01:IDE00100", "01:IDE00101", "01:IDE00102", "01:IDE00103"]));
		input.push_str(&values_page_titled("B", &["01:IDE00104", "01:IDE00105"]));
		input.push_str(LEVER);
		let built = build_with_lever_and(&input, Extra { rows, ..Extra::default() }).unwrap();
		let six = |per_s: &str| {
			(0..6)
				.map(|i| format!("01:IDE0010{i} Alarmed {per_s}/s of 50"))
				.collect::<Vec<_>>()
				.join(", ")
		};
		assert_eq!(
			bus_lines(&built)[1],
			format!(
				"bus: page 1 \"T\" cruise off ~100 % — reads {}, the lever 14.7/s of 20; cruise on ~100 % — reads {}",
				six("14.7"),
				six("27.4")
			)
		);
	}

	/// `alarm_priority` orders the alarms (owner, 2026-10-19): with it first, ascending; ties and
	/// the channels without it in the file's order.
	#[test]
	fn alarm_priority_orders_the_alarms_before_the_file_does() {
		let order = |keys: [&str; 3]| -> Vec<u16> { build_with_alarms(keys, "").unwrap().plan.alarms.iter().map(|a| a.channel).collect() };
		assert_eq!(order(["alarm_max = 5", "alarm_max = 5", "alarm_max = 5"]), [0, 1, 2], "the file's order");
		assert_eq!(
			order(["alarm_max = 5", "alarm_max = 5\nalarm_priority = 2", "alarm_max = 5\nalarm_priority = 1"]),
			[2, 1, 0]
		);
		assert_eq!(
			order(["alarm_max = 5\nalarm_priority = 3", "alarm_max = 5", "alarm_max = 5\nalarm_priority = 3"]),
			[0, 2, 1],
			"a tie keeps the file's order, and no priority comes last"
		);
		let built = build_with_alarms(["", "", "alarm_max = 5\nalarm_priority = 1"], "").unwrap();
		assert!(built.notes.iter().any(|n| n.ends_with("→ page 2 \"B\", priority 1")), "{:?}", built.notes);
		let refused = |keys: &str| {
			let why = build_with_alarms(["", keys, ""], "").unwrap_err().to_string();
			why.split_once("[[channel]] 2: ").map(|(_, why)| why.to_string()).unwrap_or(why)
		};
		assert_eq!(
			refused("alarm_max = 5\nalarm_priority = 0"),
			"alarm_priority 0 is not a whole number, 1 or more"
		);
		assert_eq!(
			refused("alarm_max = 5\nalarm_priority = -2"),
			"alarm_priority -2 is not a whole number, 1 or more"
		);
		assert_eq!(
			refused("alarm_max = 5\nalarm_priority = 1.5"),
			"alarm_priority must be a whole number, 1 or more, not a float"
		);
		assert_eq!(
			refused("alarm_priority = 1"),
			"alarm_priority needs an alarm on the channel: alarm_max, alarm_min or drift_percent"
		);
	}

	/// An alarm raises its page by plan index, and the board holds `MAX_PAGES`: a plan
	/// with more is one whose alarm could raise a page the glass never shows.
	#[test]
	fn more_pages_than_the_board_holds_are_refused() {
		// The fixture has three pages of its own.
		let extra = |n: usize| values_page_titled("P", &["01:IDE00001"]).repeat(n);
		assert_eq!(
			build_with_alarms(NO_ALARMS, &extra(MAX_PAGES - 2)).unwrap_err().to_string(),
			format!("{} [[page]] tables, and the board holds at most 8", MAX_PAGES + 1)
		);
		assert_eq!(build_with_alarms(NO_ALARMS, &extra(MAX_PAGES - 3)).unwrap().plan.pages.len(), MAX_PAGES);
	}

	/// One language for the plan (owner, 2026-09-27): `dash.toml`'s `language` picks the board's
	/// own words **and** the glossary column every label is taken from; without it both follow
	/// `config.toml`. Before, labels followed `config.toml` whatever the plan said.
	#[test]
	fn the_labels_are_in_the_plans_language_whatever_the_settings_say() {
		let here = tempfile::tempdir().unwrap();
		// Synthetic wording: `IDE00002` is written in Russian only, `IDE00003` in neither.
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
			&[("IDE00003", "Three, as the label files say")],
		)
		.with_glossary("text_id,en,ru\nIDE00001,Coolant,ОЖ\nIDE00002,,Масло\n")
		// Opened as `config.toml` says, the way `resolve_for_car` opens it: in Russian.
		.in_language(Language::Ru);
		let store = CatalogStore::open(here.path().join("proven"));
		let built = |language: &str| {
			let text = format!(
				"vin = \"TESTVIN0000000001\"\n{language}[[channel]]\nref = \"01:IDE00001\"\n[[channel]]\nref = \"01:IDE00002\"\n[[channel]]\nref = \"01:IDE00003\"\n{}",
				values_page(&["01:IDE00001", "01:IDE00002", "01:IDE00003"])
			);
			let plan = build(
				&parse_input(&text).unwrap(),
				&store,
				&extracted,
				&[identity(ENGINE, "PART1", "EV_Test")],
				Language::Ru,
			)
			.unwrap()
			.plan;
			(plan.language.clone(), plan.channels.iter().map(|c| c.label.clone()).collect::<Vec<_>>())
		};
		assert_eq!(
			built("language = \"en\"\n"),
			(
				"en".to_string(),
				vec![
					"Coolant".to_string(),
					// Not the Russian cell: a column the glossary leaves blank falls through to the
					// project's wording, never to another language's.
					"Two".to_string(),
					"Three, as the label files say".to_string()
				]
			)
		);
		assert_eq!(
			built(""),
			(
				"ru".to_string(),
				vec!["ОЖ".to_string(), "Масло".to_string(), "Three, as the label files say".to_string()]
			),
			"no `language`: the settings' language, for the labels and the board alike"
		);
		assert_eq!(extracted.language(), Language::Ru, "the caller's project is left as it was");
	}

	/// A setpoint with no `[[channel]]` of its own is read with its channel and never offered as
	/// a cell (`todo/dash/18`, owner 2026-09-27): no page or stopwatch may name it, in either
	/// spelling.
	#[test]
	fn a_setpoint_without_a_channel_of_its_own_is_never_a_cell() {
		let paired = "[[channel]]\nref = \"01:IDE00191\"\nsetpoint = \"01:IDE00190\"\nhz = 50\n";
		for spelled in ["01:IDE00190", "01:2029"] {
			let why = build_with_setpoint(&format!("{paired}{}", values_page_titled("B", &[spelled])))
				.unwrap_err()
				.to_string();
			assert_eq!(
				why,
				format!("page #1: {spelled} is a setpoint with no [[channel]] of its own — declare it as a [[channel]] to show it")
			);
			let why = build_with_setpoint(&format!(
				"{paired}[stopwatch]\nspeed = \"{spelled}\"\nkm_h_per_unit = 0.0\nmarks = [60]\n"
			))
			.unwrap_err()
			.to_string();
			assert_eq!(
				why,
				format!("[stopwatch] speed {spelled} is a setpoint with no [[channel]] of its own — declare it as a [[channel]] to show it")
			);
		}
		// Declared as a `[[channel]]` too, it is a channel like any other.
		let declared = format!("{paired}[[channel]]\nref = \"01:IDE00190\"\nhz = 50\nalarm_max = 2.0\nalarm_max_off = 1.0\n");
		let built = build_with_setpoint(&format!(
			"{declared}{}[stopwatch]\nspeed = \"01:IDE00190\"\nkm_h_per_unit = 0.0\nmarks = [60]\n",
			values_page_titled("B", &["01:IDE00190"])
		))
		.unwrap();
		assert_eq!(built.plan.alarms[0].channel, 1);
		assert_eq!(built.plan.stopwatch.map(|s| s.speed), Some(1));
	}

	/// The board draws a chart in `f32`: a scale apart in the file and one value there draws no
	/// trace at all, and one whose span overflows draws every value on the floor.
	#[test]
	fn a_chart_scale_is_compared_as_the_board_holds_it() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[("EV_Test_001", vec![reading(0x2029, "Boost", "IDE00191", 0, 16, false, true, 0.001, 0.0)])],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		let chart = |min: &str, max: &str| {
			build(
				&input(
					&["01:IDE00191"],
					&format!("[[page]]\nkind = \"chart\"\ncell = \"01:IDE00191\"\nmin = {min}\nmax = {max}\n"),
				),
				&store,
				&extracted,
				&[identity(ENGINE, "PART1", "EV_Test")],
				Language::En,
			)
		};
		assert_eq!(
			chart("100.000001", "100.000002").unwrap_err().to_string(),
			"page #1: min 100 is not below max 100",
			"apart in the file, one value in the board's f32"
		);
		assert_eq!(
			chart("-3e38", "3e38").unwrap_err().to_string(),
			"page #1: min -3e38 and max 3e38 are further apart than the board's 32-bit float holds"
		);
		assert_eq!(chart("2", "1").unwrap_err().to_string(), "page #1: min 2 is not below max 1");
		chart("0.9", "2.1").unwrap();
	}

	/// A factor or offset past what an `f32` holds is an `inf` in the generated source, and the
	/// firmware's build fails on it far from here.
	#[test]
	fn a_scaling_the_boards_f32_cannot_hold_is_refused() {
		let here = tempfile::tempdir().unwrap();
		let extracted = extracted_with(
			here.path(),
			&[(
				"EV_Test_001",
				vec![
					reading(0x2029, "Huge", "IDE00191", 0, 16, false, true, 1e39, 0.0),
					reading(0x202A, "Far", "IDE00192", 0, 16, false, true, 1.0, -1e39),
					reading(0x202B, "Vanishing", "IDE00193", 0, 16, false, true, 1e-46, 0.0),
				],
			)],
			&[],
		);
		let store = CatalogStore::open(here.path().join("proven"));
		for id in ["01:IDE00191", "01:IDE00192", "01:IDE00193"] {
			let err = build(
				&input(&[id], &values_page(&[id])),
				&store,
				&extracted,
				&[identity(ENGINE, "PART1", "EV_Test")],
				Language::En,
			)
			.unwrap_err();
			assert_eq!(err, Error::NotFinite(Reference::parse(id).unwrap()), "{id}");
			assert_eq!(
				err.to_string(),
				format!("{id}: its scaling is not a finite number the board's 32-bit float holds")
			);
		}
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

	/// The lever's unit and identifier in the fixture, made up for it: no car's column, no
	/// car's identifier. What the fixture keeps is only the shape a lever has — several
	/// enumerated fields in one answer, a byte each. The code path takes none of these: every
	/// one comes from `[stalk]` and the project.
	const STALK_UNIT: u16 = 0x75A;
	const STALK_DID: u16 = 0x4C21;

	fn state_reading(did: u16, name: &str, text_id: &str, bit_offset: u32, bit_length: u32, levels: Vec<Level>) -> Reading {
		Reading {
			scaling: Scaling::Enum { levels },
			unit: None,
			..reading(did, name, text_id, bit_offset, bit_length, false, true, 1.0, 0.0)
		}
	}

	/// A neutral ladder: states tiling a byte in equal bands, as a switch read as a voltage has
	/// states tiling its range. The bands are arithmetic, not any car's.
	fn ladder(names: &[&str]) -> Vec<Level> {
		let n = names.len() as i32;
		names
			.iter()
			.enumerate()
			.map(|(i, name)| {
				let i = i as i32;
				Level::range(i * 256 / n, (i + 1) * 256 / n - 1, *name)
			})
			.collect()
	}

	const LEVER: &str = "[stalk]\nread = \"75A:4C21\"\nrocker = \"Rocker\"\nswitch = \"Switch\"\nnext = \"plus\"\nprevious = \"minus\"\nmeasure = \"limit\"\nswitch_off = \"off\"\ncruise = \"01:2001\"\ncruise_off = \"off\"\n";
	const WATCH: &str = "[stopwatch]\nspeed = \"01:IDE00010\"\nkm_h_per_unit = 0.0\nmarks = [60, 100]\n";

	/// What a lever test adds to the fixture of [`build_with_lever_in`].
	#[derive(Default)]
	struct Extra {
		/// More rows, each on [`ENGINE`] or on [`STALK_UNIT`].
		rows: Vec<(u16, Reading)>,
		/// Lines under each of the fixture's three `[[channel]]`s.
		keys: [&'static str; 3],
	}

	/// An engine with a speed, a speed with an offset, a cruise status and a quantity; a
	/// steering column with a rocker, a switch and a voltage in one identifier. `input` is
	/// the whole `[stalk]` / `[stopwatch]` part.
	fn build_with_lever_in(dir: &Path, input: &str, extra: Extra, cache_written: impl FnOnce(&Path)) -> Result<Built, Error> {
		let more = |unit: u16| extra.rows.iter().filter(move |(u, _)| *u == unit).map(|(_, r)| r.clone());
		let extracted = extracted_with(
			dir,
			&[
				(
					"EV_Test_001",
					vec![
						reading(0x1001, "One", "IDE00001", 0, 8, false, true, 1.0, 0.0),
						reading(0x3001, "Speed", "IDE00010", 0, 16, false, true, 1.0, 0.0),
						reading(0x3002, "Offset speed", "IDE00011", 0, 16, false, true, 1.0, -40.0),
						reading(0x3003, "Reverse speed", "IDE00012", 0, 16, false, true, -1.0, 0.0),
						reading(0x3004, "Still speed", "IDE00013", 0, 16, false, true, 0.0, 0.0),
						state_reading(
							0x2001,
							"Cruise status",
							"IDE00020",
							0,
							16,
							vec![Level::point(0, "off"), Level::point(1, "standby"), Level::point(2, "passive")],
						),
						reading(0x2002, "Quantity", "IDE00021", 0, 16, false, true, 1.0, 0.0),
					]
					.into_iter()
					.chain(more(ENGINE))
					.collect(),
				),
				(
					"EV_Stalk_001",
					vec![
						reading(STALK_DID, "Voltage", "", 0, 8, false, true, 0.1, 0.0),
						state_reading(STALK_DID, "Rocker", "", 16, 8, ladder(&["rest", "plus", "minus", "limit", "open"])),
						state_reading(STALK_DID, "Switch", "", 24, 8, ladder(&["open", "off", "on", "shorted"])),
						// What the matching rule is for: a catch-all listed first, bands out of
						// order, two that overlap, a second unbounded one.
						state_reading(
							STALK_DID,
							"Messy",
							"",
							32,
							8,
							vec![
								Level::range(i32::MIN, i32::MAX, "any"),
								Level::range(200, 255, "plus"),
								Level::range(0, 50, "minus"),
								Level::range(40, 60, "limit"),
								Level::point(100, "rest"),
								Level::range(i32::MIN, -10, "below"),
								Level::range(120, 130, "twice"),
								Level::range(140, 150, "twice"),
							],
						),
					]
					.into_iter()
					.chain(more(STALK_UNIT))
					.collect(),
				),
			],
			&[],
		);
		cache_written(&dir.join("cache.sqlite"));
		let store = CatalogStore::open(dir.join("proven"));
		let [one, speed, volts] = extra.keys.map(|keys| if keys.is_empty() { String::new() } else { format!("{keys}\n") });
		let text = format!(
			"vin = \"TESTVIN0000000001\"\n[[channel]]\nref = \"01:IDE00001\"\n{one}[[channel]]\nref = \"01:IDE00010\"\nhz = 50\n{speed}[[channel]]\nref = \"01:IDE00011\"\n{volts}{}{input}",
			values_page(&["01:IDE00001"])
		);
		build(
			&parse_input(&text)?,
			&store,
			&extracted,
			&[identity(ENGINE, "PART1", "EV_Test"), identity(STALK_UNIT, "PART2", "EV_Stalk")],
			Language::En,
		)
	}

	fn build_with_lever(input: &str) -> Result<Built, Error> {
		build_with_lever_and(input, Extra::default())
	}

	fn build_with_lever_and(input: &str, extra: Extra) -> Result<Built, Error> {
		let here = tempfile::tempdir().unwrap();
		build_with_lever_in(here.path(), input, extra, |_| {})
	}

	/// Owner, 2026-10-05: the ESC's acceleration reads the slope while the car stands.
	#[test]
	fn zero_at_rest_reaches_both_plans_and_needs_a_standstill_source() {
		let flagged = "[[channel]]\nref = \"01:IDE00013\"\nzero_at_rest = true\n";
		let built = build_with_lever(&format!("{WATCH}{flagged}")).unwrap();
		let zeroed: Vec<u16> = built.plan.channels.iter().filter(|c| c.zero_at_rest).map(|c| c.did).collect();
		assert_eq!(zeroed, [0x3004], "the flagged channel, and no other");
		assert!(to_rust(&built.plan).contains("zero_at_rest: true"), "the firmware's plan carries it");
		let device = built.plan.to_device();
		assert!(device.channels.iter().any(|c| c.did == 0x3004 && c.zero_at_rest), "and the replay's");

		let refused = build_with_lever(flagged).unwrap_err().to_string();
		assert!(
			refused.contains("zero_at_rest needs a source for whether the car stands — today a [stopwatch]"),
			"{refused}"
		);
	}

	/// `todo/dash/26`: whether the car stands is one fact of the plan, today the stopwatch's speed
	/// reading 0 — in both plans, and in `plan.json` as a kind a later source can sit beside.
	#[test]
	fn the_plan_takes_its_standstill_from_the_stopwatchs_speed_today() {
		use vag_dash_render::plan::Standstill as DeviceStandstill;
		let built = build_with_lever(WATCH).unwrap();
		assert_eq!(built.plan.standstill, Some(Standstill::Speed { channel: 1 }), "the stopwatch's speed");
		assert_eq!(built.plan.to_device().standstill, Some(DeviceStandstill::Speed(1)));
		let rust = to_rust(&built.plan);
		assert!(rust.contains("standstill: Some(Standstill::Speed(1)) };"), "{rust}");
		let json = built.plan.to_json();
		assert!(
			json.contains("\"standstill\": {\n    \"kind\": \"speed\",\n    \"channel\": 1\n  }"),
			"{json}"
		);
		assert_eq!(from_json(&json).unwrap(), built.plan);

		// No table of its own yet: a later source is a `[standstill]`, refused until it exists.
		let text = format!(
			"vin = \"X\"\n[[channel]]\nref = \"01:IDE00025\"\n{}[standstill]\nspeed = \"01:IDE00025\"\n",
			values_page(&["01:IDE00025"])
		);
		assert!(parse_input(&text).is_err(), "[standstill] is not a table yet");

		let none = build_with_lever("").unwrap();
		assert_eq!(none.plan.standstill, None, "no stopwatch, no source");
		assert!(to_rust(&none.plan).contains("standstill: None };"));
		assert!(!none.plan.to_json().contains("standstill"));
	}

	/// The fixture's voltage: a `[[channel]]` (`01:IDE00011`) on no page of its own, a titled
	/// chart of another channel with the voltage off, and a values page with it as a cell.
	const STRIP: &str = "[strip]\nvolts = \"01:IDE00011\"\n";
	const SPEED_CHART: &str = "[[page]]\nkind = \"chart\"\ntitle = \"Speed\"\ncell = \"01:IDE00010\"\nmin = 0\nmax = 250\nvolts = false\n";
	const VOLTS_PAGE: &str = "[[page]]\nkind = \"values\"\ntitle = \"Volts\"\ncells = [\"01:IDE00011\", \"01:IDE00001\"]\n";

	/// Owner, 2026-10-19: the battery voltage in a strip over the pages, beside their titles. The
	/// generator decides on which pages (`todo/dash/27`): the board reads each page's own.
	#[test]
	fn the_strip_reaches_both_plans_with_the_pages_titles_and_their_volts() {
		let built = build_with_lever(&format!("{STRIP}{SPEED_CHART}{VOLTS_PAGE}")).unwrap();
		assert_eq!(built.plan.strip, Some(Strip { volts: 2 }), "the [[channel]] it names");
		assert_eq!(
			built.plan.pages[1],
			Page::Chart {
				title: "Speed".into(),
				channel: 1,
				min: 0.0,
				max: 250.0,
				volts: false
			},
			"a chart has a title now"
		);
		assert!(
			built.plan.pages.iter().filter(|p| !matches!(p, Page::Chart { .. })).all(Page::volts),
			"on unless a page says off"
		);

		let rust = to_rust(&built.plan);
		for wanted in [
			"Page::Values { title: \"T\", cells: &CELLS_0, volts: Some(2) },",
			"Page::Chart { title: \"Speed\", channel: 1, slot: 0, min: 0.0, max: 250.0, volts: None },",
			"Page::Values { title: \"Volts\", cells: &CELLS_2, volts: None },",
		] {
			assert!(rust.contains(wanted), "{wanted}\n{rust}");
		}
		let device = built.plan.to_device();
		let shown: Vec<Option<u16>> = device.pages.iter().map(vag_dash_render::plan::Page::volts).collect();
		assert_eq!(
			shown,
			[Some(2), None, None],
			"page 1 shows it; the chart has it off; page 3 has it as a cell"
		);
		assert_eq!(device.pages.iter().find_map(vag_dash_render::plan::Page::volts), Some(2));
		assert_eq!(device.strip(1, |_| None).title, "Speed");

		assert!(built.notes.iter().any(|n| n == "strip: volts 01:IDE00011 on page 1"), "{:?}", built.notes);
		assert!(
			!built.advice.iter().any(|n| n.ends_with("on page 1")),
			"where the strip is shown is no advice"
		);
		assert!(
			advised(&built, "strip: volts 01:IDE00011 reads in \"°C\", and the strip writes it as V"),
			"the fixture's rows are all °C: {:?}",
			built.notes
		);
		let json = built.plan.to_json();
		assert_eq!(from_json(&json).unwrap(), built.plan);
	}

	#[test]
	fn without_a_strip_the_plan_has_none_and_a_pages_volts_is_refused() {
		let built = build_with_lever("").unwrap();
		assert_eq!(built.plan.strip, None);
		assert!(built.plan.to_device().pages.iter().all(|p| p.volts().is_none()));
		assert!(to_rust(&built.plan).contains("cells: &CELLS_0, volts: None },"));
		assert!(!built.notes.iter().any(|n| n.starts_with("strip:")), "{:?}", built.notes);

		for volts in ["true", "false"] {
			let page = format!("[[page]]\nkind = \"values\"\ncells = [\"01:IDE00001\"]\nvolts = {volts}\n");
			let why = build_with_lever(&page).unwrap_err().to_string();
			assert_eq!(
				why, "dash.toml: page #2: volts needs a [strip] — its volts is the channel the voltage is read from",
				"{volts}"
			);
		}
	}

	#[test]
	fn the_strips_volts_is_a_channel_of_the_input() {
		let why = build_with_lever("[strip]\nvolts = \"01:IDE00013\"\n").unwrap_err().to_string();
		assert_eq!(why, "[strip] volts 01:IDE00013 is not in the [[channel]] list");
		// The same row spelled by its identifier is the same channel.
		let built = build_with_lever("[strip]\nvolts = \"01:3002\"\n").unwrap();
		assert_eq!(built.plan.strip, Some(Strip { volts: 2 }));
	}

	#[test]
	fn the_strip_is_one_table_with_volts_and_a_pages_volts_is_true_or_false() {
		let text = |extra: &str| {
			format!(
				"vin = \"TESTVIN0000000001\"\n[[channel]]\nref = \"01:IDE00001\"\n{}{extra}",
				values_page(&["01:IDE00001"])
			)
		};
		assert_eq!(
			parse_input(&text("[strip]\n")).unwrap_err().to_string(),
			"dash.toml: [strip] volts is missing or not a string"
		);
		assert_eq!(
			parse_input(&text("[[strip]]\nvolts = \"01:IDE00001\"\n")).unwrap_err().to_string(),
			"dash.toml: strip must be one [strip] table"
		);
		let parsed = parse_input(&text("[strip]\nvolts = \"01:IDE00001\"\n")).unwrap();
		assert_eq!(
			parsed.strip,
			Some(StripInput {
				volts: Reference::parse("01:IDE00001").unwrap()
			})
		);
		let why = parse_input(&text("").replace("cells =", "volts = \"yes\"\ncells ="))
			.unwrap_err()
			.to_string();
		assert!(why.ends_with("[[page]] 1: volts must be true or false, not a string"), "{why}");
	}

	/// `todo/dash/27`: a channel shown only on a chart page may carry an alarm — it raises the
	/// chart, whose header the board inverts. A values page with it as a cell still comes first.
	#[test]
	fn an_alarm_on_a_chart_only_channel_raises_its_chart_and_a_values_page_comes_first() {
		let speed_alarm = || Extra {
			keys: ["", "alarm_max = 200", ""],
			..Extra::default()
		};
		let built = build_with_lever_and(&format!("{SPEED_CHART}{STRIP}"), speed_alarm()).unwrap();
		assert_eq!((built.plan.alarms[0].channel, built.plan.alarms[0].page), (1, 1), "the chart page");
		assert!(
			built
				.notes
				.iter()
				.any(|n| n.starts_with("alarm: 01:IDE00010 at or above 200") && n.ends_with("→ page 2 \"Speed\"")),
			"{:?}",
			built.notes
		);
		let shown_too = "[[page]]\nkind = \"values\"\ncells = [\"01:IDE00010\"]\n";
		let built = build_with_lever_and(&format!("{SPEED_CHART}{shown_too}{STRIP}"), speed_alarm()).unwrap();
		assert_eq!(built.plan.alarms[0].page, 2, "the values page, after the chart in the file");

		let why = build_with_lever_and(STRIP, speed_alarm()).unwrap_err().to_string();
		assert_eq!(
			why,
			"01:IDE00010: alarm_max needs a page that shows the channel — the alarm raises the first values page with it as a cell, else its chart page, and no [[page]] has it"
		);
	}

	#[test]
	fn zero_at_rest_is_true_or_false() {
		let text = "vin = \"TESTVIN0000000001\"\n[[channel]]\nref = \"01:IDE00001\"\nzero_at_rest = \"yes\"\n";
		let why = parse_input(text).unwrap_err().to_string();
		assert!(why.contains("zero_at_rest is not true or false"), "{why}");
	}

	#[test]
	fn the_lever_resolves_its_texts_to_the_projects_intervals_and_joins_the_plan_on_no_page() {
		let built = build_with_lever(&format!("{LEVER}{WATCH}")).unwrap();
		let plan = &built.plan;
		let stalk = plan.stalk.as_ref().expect("a stalk");
		assert_eq!((stalk.rocker, stalk.switch, stalk.cruise), (3, 4, 5), "after the owner's three channels");
		let rocker = &plan.channels[3];
		assert_eq!(
			(rocker.unit, rocker.did, rocker.bit_offset, rocker.bit_length),
			(STALK_UNIT, STALK_DID, 16, 8)
		);
		assert_eq!((rocker.factor, rocker.offset), (1.0, 0.0), "a state is looked up by its raw value");
		assert_eq!(plan.channels[4].bit_offset, 24, "the switch, from the same answer");
		assert_eq!((plan.channels[5].did, plan.channels[5].bit_length), (0x2001, 16));
		assert_eq!(
			(stalk.next, stalk.previous, stalk.measure, stalk.switch_off, stalk.cruise_off),
			(1, 2, 3, 1, 0)
		);
		assert_eq!(
			stalk.rocker_states[1],
			State {
				lower: 51,
				upper: 101,
				name: "plus".to_string()
			},
			"the interval, not its lower end"
		);
		assert!(plan.pages.iter().all(|p| match p {
			Page::Values { cells, .. } => cells.iter().all(|c| *c < 3),
			Page::Chart { channel, .. } => *channel < 3,
		}));
		assert!(
			plan.units.iter().any(|u| u.request == STALK_UNIT && u.part_number == "PART2"),
			"the column is checked like any unit"
		);
		let stopwatch = plan.stopwatch.as_ref().expect("a stopwatch");
		assert_eq!(
			(stopwatch.speed, stopwatch.km_h_per_unit, stopwatch.marks.as_slice()),
			(1, 0.0, &[60, 100][..])
		);
		assert!(built.notes.iter().any(|n| n.contains("not measured")), "{:?}", built.notes);
		assert!(built.advice.iter().any(|n| n.contains("not measured")), "{:?}", built.advice);
	}

	#[test]
	fn the_boards_rule_for_a_state_is_the_laptops() {
		let built = build_with_lever(&format!("{LEVER}{WATCH}")).unwrap();
		let device = built.plan.to_device();
		let stalk = device.stalk.expect("carried to the board");
		let laptop = &built.plan.stalk.as_ref().unwrap().rocker_states;
		let levels: Vec<Level> = laptop.iter().map(|s| Level::range(s.lower, s.upper, s.name.clone())).collect();
		for raw in -5..=300 {
			let board = vag_dash_render::plan::state_of(stalk.rocker_states, i64::from(raw)).map(|s| usize::from(s.0));
			assert_eq!(board, vag_data_labels::catalog::level_for(&levels, raw), "raw {raw}");
		}
		assert_eq!(stalk.states.measure, vag_dash_render::stalk::StateIndex(3));
		assert_eq!(device.stopwatch.map(|s| (s.speed, s.marks)), Some((1, &[60u16, 100][..])));

		// A ladder where the order of trying matters: any rule but the shared one fails here.
		let built = build_with_lever(&format!("{}{WATCH}", LEVER.replacen("\"Rocker\"", "\"Messy\"", 1))).unwrap();
		let device = built.plan.to_device();
		let bands = device.stalk.expect("carried to the board").rocker_states;
		let laptop = &built.plan.stalk.as_ref().unwrap().rocker_states;
		let levels: Vec<Level> = laptop.iter().map(|s| Level::range(s.lower, s.upper, s.name.clone())).collect();
		for raw in [
			i32::MIN,
			i32::MIN + 1,
			-11,
			-10,
			-9,
			-1,
			0,
			39,
			40,
			45,
			50,
			51,
			60,
			61,
			99,
			100,
			101,
			199,
			200,
			255,
			256,
			i32::MAX,
		] {
			let board = vag_dash_render::plan::state_of(bands, i64::from(raw)).map(|s| usize::from(s.0));
			assert_eq!(board, vag_data_labels::catalog::level_for(&levels, raw), "raw {raw}");
		}
		let name = |raw: i64| vag_dash_render::plan::state_of(bands, raw).map(|s| laptop[usize::from(s.0)].name.as_str());
		assert_eq!(
			(name(45), name(1_000), name(-20), name(100)),
			(Some("minus"), Some("any"), Some("any"), Some("rest")),
			"bounded first in table order, then the unbounded ones in theirs — `any` before `below`"
		);
	}

	#[test]
	fn a_button_named_by_a_state_the_field_gives_twice_is_refused() {
		let lever = LEVER.replacen("\"Rocker\"", "\"Messy\"", 1).replacen("\"plus\"", "\"twice\"", 1);
		let why = build_with_lever(&lever).unwrap_err().to_string();
		assert_eq!(
			why,
			"[stalk] next: \"twice\" names 2 bands of \"Messy\" (120–130, 140–150) — the board takes a button as one band, so \
			 this state cannot be used yet"
		);
		// An off state is not a button, and the message does not call it one; a point is its
		// value alone, an unbounded end says which way it runs.
		let split = Extra {
			rows: vec![(
				STALK_UNIT,
				state_reading(
					STALK_DID,
					"Split",
					"",
					40,
					8,
					vec![
						Level::range(i32::MIN, -1, "off"),
						Level::point(7, "off"),
						Level::range(200, i32::MAX, "off"),
						Level::point(8, "on"),
					],
				),
			)],
			..Extra::default()
		};
		let why = build_with_lever_and(&LEVER.replacen("\"Switch\"", "\"Split\"", 1), split)
			.unwrap_err()
			.to_string();
		assert_eq!(
			why,
			"[stalk] switch_off: \"off\" names 3 bands of \"Split\" (up to -1, 7, 200 and up) — the board takes an off state \
			 as one band, so this state cannot be used yet"
		);
		assert_eq!(band_text(&Level::range(i32::MIN, i32::MAX, "any")), "any value");
	}

	/// Rows whose ODIS names carry a space at one end, as nine enumerated fields of one real
	/// project do: a rocker with a trailing one, a switch with a leading one, states with both,
	/// and a cruise status whose off state ends in one.
	fn spaced() -> Extra {
		Extra {
			rows: vec![
				(
					STALK_UNIT,
					state_reading(STALK_DID, "Spaced ", "", 40, 8, ladder(&[" up", "down ", " idle ", "rest"])),
				),
				(STALK_UNIT, state_reading(STALK_DID, " Lead", "", 48, 8, ladder(&["off ", " on"]))),
				(
					ENGINE,
					state_reading(
						0x2003,
						"Cruise spaced",
						"IDE00022",
						0,
						8,
						vec![Level::point(0, " off "), Level::point(1, "on")],
					),
				),
			],
			..Extra::default()
		}
	}

	const SPACED: &str = "[stalk]\nread = \"75A:4C21\"\nrocker = \"Spaced \"\nswitch = \"Lead\"\nnext = \"up\"\nprevious = \"down\"\nmeasure = \"idle\"\nswitch_off = \"off\"\ncruise = \"01:2003\"\ncruise_off = \"off\"\n";

	/// `dash.toml` is trimmed when it is read, so a name is compared trimmed on the project's
	/// side too — or a field the project spells `"Rocker "` could never be named at all.
	#[test]
	fn a_name_the_project_spells_with_a_space_at_an_end_is_matched_trimmed() {
		let built = build_with_lever_and(SPACED, spaced()).unwrap();
		let stalk = built.plan.stalk.as_ref().expect("a stalk");
		let (rocker, switch) = (
			&built.plan.channels[usize::from(stalk.rocker)],
			&built.plan.channels[usize::from(stalk.switch)],
		);
		assert_eq!((rocker.bit_offset, switch.bit_offset), (40, 48));
		assert_eq!(
			(stalk.next, stalk.previous, stalk.measure, stalk.switch_off, stalk.cruise_off),
			(0, 1, 2, 0, 0)
		);
		// What the owner is shown to choose from is what they can type: trimmed.
		let why = build_with_lever_and(&SPACED.replacen("\"Spaced \"", "\"Spice\"", 1), spaced())
			.unwrap_err()
			.to_string();
		assert!(
			why.contains("\"Spaced\"") && why.contains("\"Lead\"") && !why.contains("\"Spaced \""),
			"{why}"
		);
		let why = build_with_lever_and(&SPACED.replacen("\"up\"", "\"upp\"", 1), spaced())
			.unwrap_err()
			.to_string();
		assert!(why.contains("its states are \"up\", \"down\", \"idle\", \"rest\""), "{why}");
	}

	/// Two names the project tells apart only by a space are one name once trimmed.
	#[test]
	fn two_fields_or_two_states_equal_once_trimmed_are_refused() {
		let twin = Extra {
			rows: vec![(STALK_UNIT, state_reading(STALK_DID, "Rocker ", "", 40, 8, ladder(&["a", "b"])))],
			..Extra::default()
		};
		let why = build_with_lever_and(LEVER, twin).unwrap_err().to_string();
		assert!(why.contains("rocker: 75A:4C21 has 2 fields named \"Rocker\""), "{why}");
		let twin = Extra {
			rows: vec![(
				STALK_UNIT,
				state_reading(STALK_DID, "Twin", "", 40, 8, ladder(&["plus", "plus ", "minus", "limit"])),
			)],
			..Extra::default()
		};
		let why = build_with_lever_and(&LEVER.replacen("\"Rocker\"", "\"Twin\"", 1), twin)
			.unwrap_err()
			.to_string();
		assert!(why.contains("next: \"plus\" names 2 bands of \"Twin\" (0–63, 64–127)"), "{why}");
	}

	/// The refusals only a second row reaches, each one: the helper's fixture has one row
	/// per name, so none of them was ever exercised (review, 2026-09-27).
	#[test]
	fn the_lever_refusals_a_second_row_decides_are_each_reached() {
		// A unit the car's record has nothing about: the column's, then the cruise status's.
		let why = build_with_lever(&LEVER.replacen("75A:4C21", "75B:4C21", 1)).unwrap_err();
		assert_eq!(why, Error::UnknownUnit(0x75B));
		let why = build_with_lever(&LEVER.replacen("01:2001", "02:2001", 1)).unwrap_err();
		assert_eq!(why, Error::UnknownUnit(GEARBOX));

		// A field name the identifier gives twice, exactly.
		let twice = Extra {
			rows: vec![(STALK_UNIT, state_reading(STALK_DID, "Switch", "", 40, 8, ladder(&["off", "on"])))],
			..Extra::default()
		};
		let why = build_with_lever_and(LEVER, twice).unwrap_err().to_string();
		assert!(why.contains("switch: 75A:4C21 has 2 fields named \"Switch\""), "{why}");

		// Two enumerated rows under one text id: which one is the cruise status is not told.
		let why = build_with_lever_and(&LEVER.replacen("01:2001", "01:IDE00020", 1), second_cruise_status()).unwrap_err();
		assert!(
			matches!(&why, Error::Ambiguous(r, rows) if *r == Reference::parse("01:IDE00020").unwrap() && rows.len() == 2),
			"{why:?}"
		);
	}

	/// A second enumerated row under the cruise status's text id, on another identifier.
	fn second_cruise_status() -> Extra {
		Extra {
			rows: vec![(
				ENGINE,
				state_reading(
					0x2005,
					"Cruise status",
					"IDE00020",
					0,
					8,
					vec![Level::point(0, "off"), Level::point(1, "on")],
				),
			)],
			..Extra::default()
		}
	}

	/// The message of an ambiguous cruise status says what to write instead, the way a
	/// channel's does: each row's identifier and bit offset.
	#[test]
	fn an_ambiguous_cruise_status_lists_what_to_write_instead() {
		let why = build_with_lever_and(&LEVER.replacen("01:2001", "01:IDE00020", 1), second_cruise_status())
			.unwrap_err()
			.to_string();
		assert!(
			why.ends_with("name one by identifier and bit offset: 2001@0 Cruise status, 2005@0 Cruise status"),
			"{why}"
		);
		// What it says to write builds.
		let lever = LEVER.replacen("01:2001", "01:2005@0", 1);
		let built = build_with_lever_and(&lever, second_cruise_status()).unwrap();
		let cruise = &built.plan.channels[usize::from(built.plan.stalk.unwrap().cruise)];
		assert_eq!(cruise.did, 0x2005);
	}

	/// The gate wants two witnesses that cruise is off: the switch, and the engine's own
	/// status. A `cruise` read out of the lever's own answer is one unit saying it twice.
	#[test]
	fn a_cruise_status_inside_the_levers_own_identifier_is_refused() {
		let inside = Extra {
			rows: vec![(
				STALK_UNIT,
				state_reading(STALK_DID, "Also cruise", "IDE00030", 56, 8, ladder(&["off", "on"])),
			)],
			..Extra::default()
		};
		for cruise in ["75A:4C21@24", "75A:IDE00030"] {
			let lever = LEVER.replacen("\"01:2001\"", &format!("{cruise:?}"), 1);
			let why = build_with_lever_and(
				&lever,
				Extra {
					rows: inside.rows.clone(),
					..Extra::default()
				},
			)
			.unwrap_err()
			.to_string();
			assert!(
				why.starts_with(&format!("[stalk] cruise {cruise} is in read's own identifier 75A:4C21")),
				"{cruise}: {why}"
			);
		}
		// Another identifier on the same unit is another answer.
		let beside = Extra {
			rows: vec![(STALK_UNIT, state_reading(0x4C22, "Beside", "", 0, 8, ladder(&["off", "on"])))],
			..Extra::default()
		};
		build_with_lever_and(&LEVER.replacen("01:2001", "75A:4C22", 1), beside).expect("another answer");
	}

	/// The generated source of a plan with a lever, a stopwatch, buttons and a strip, checked in and
	/// compiled by `tests/generated_plan.rs` under `deny(warnings)`: CI builds the firmware on
	/// an empty plan, so nothing else ever compiles the `[stalk]` / `[stopwatch]` /
	/// `[[button]]` half of `to_rust`. Built from this module's made-up fixture.
	///
	/// `BLESS=1 cargo test -p vag-cli-core generated_source` rewrites it; review the diff, then
	/// **run the tests again**: `tests/generated_plan.rs` is compiled in the same run as the
	/// rewrite, before it, and still sees the old file.
	#[test]
	fn the_generated_source_of_a_lever_plan_is_the_one_checked_in() {
		// The rocker on the ladder with unbounded states, so their literals are compiled too.
		let lever = LEVER.replacen("\"Rocker\"", "\"Messy\"", 1);
		let built = build_with_lever(&format!(
			"{lever}{}{EVERY_ACTION}{STRIP}{SPEED_CHART}",
			WATCH.replacen("0.0", "0.0271", 1)
		))
		.unwrap();
		let rust = to_rust(&built.plan);
		// The generator's header names a VIN and says not to commit; the fixture says what it is.
		let body = rust.splitn(3, '\n').nth(2).expect("a header of two lines");
		let text = format!("{GENERATED_HEADER}{body}");
		let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lever_plan.rs");
		if blesses(std::env::var_os("BLESS").as_deref()) {
			std::fs::write(&path, &text).unwrap();
		}
		let checked_in = std::fs::read_to_string(&path).unwrap_or_default();
		assert!(
			checked_in == text,
			"{} is not what `to_rust` writes now — run `BLESS=1 cargo test -p vag-cli-core generated_source`, review the diff, \
			 and run the tests again: tests/generated_plan.rs compiled the old file",
			path.display()
		);
	}

	/// The plan the id tests turn: a lever, a stopwatch, buttons, a strip and a chart.
	fn every_part() -> Plan {
		build_with_lever(&format!("{LEVER}{WATCH}{EVERY_ACTION}{STRIP}{SPEED_CHART}"))
			.unwrap()
			.plan
	}

	#[test]
	fn a_plan_has_the_same_id_however_often_it_is_built_and_says_it_in_both_outputs() {
		let (one, two) = (every_part(), every_part());
		assert_eq!(plan_id(&one), plan_id(&two));
		let id = plan_id(&one);
		let rust = to_rust(&one);
		assert!(rust.ends_with(&format!("pub const PLAN_ID: u64 = 0x{id:016x};\n")), "{rust}");
		// The id is the hash of what precedes it, so anyone can check it from the file.
		let above = rust.rsplit_once("\n///").unwrap().0;
		assert_eq!(fnv1a64(above.as_bytes()), id);
		let json: serde_json::Value = serde_json::from_str(&one.to_json()).unwrap();
		assert_eq!(json["id"], format!("{id:016x}"));
		assert_eq!(from_json(&one.to_json()).unwrap(), one, "the id is derived, not read back");
	}

	#[test]
	fn anything_the_board_runs_differently_on_changes_the_id() {
		let base = every_part();
		let id = plan_id(&base);
		type Change = (&'static str, fn(&mut Plan));
		let changes: [Change; 13] = [
			("a page title", |p| match &mut p.pages[0] {
				Page::Values { title, .. } | Page::Chart { title, .. } => title.push('!'),
			}),
			("the pages' order", |p| p.pages.reverse()),
			("a page's voltage", |p| match &mut p.pages[0] {
				Page::Values { volts, .. } | Page::Chart { volts, .. } => *volts = !*volts,
			}),
			("a channel's rate", |p| p.channels[0].hz += 1.0),
			("a channel's scaling", |p| p.channels[0].factor *= 2.0),
			("a channel's label", |p| p.channels[0].label.push('x')),
			("a unit's part number", |p| p.units[0].part_number.push('X')),
			("the stopwatch's marks", |p| p.stopwatch.as_mut().unwrap().marks.push(130)),
			("a button's action", |p| p.buttons.swap(0, 1)),
			("the strip", |p| p.strip = None),
			("the standstill", |p| p.standstill = None),
			("the lever's states", |p| p.stalk.as_mut().unwrap().rocker_states[0].upper -= 1),
			("the language", |p| p.language = "ru".to_string()),
		];
		for (what, change) in changes {
			let mut changed = base.clone();
			change(&mut changed);
			assert_ne!(plan_id(&changed), id, "{what} changed and the id did not");
		}
		let mut alarmed = base.clone();
		alarmed.alarms.push(Alarm {
			channel: 0,
			page: 0,
			rule: AlarmRule::Bounds {
				max: Some(Bound { at: 100.0, off: 95.0 }),
				min: None,
			},
		});
		assert_ne!(plan_id(&alarmed), id, "an alarm added");
		let mut tripped = alarmed.clone();
		if let AlarmRule::Bounds { max: Some(bound), .. } = &mut tripped.alarms[0].rule {
			bound.at = 101.0;
		}
		assert_ne!(plan_id(&tripped), plan_id(&alarmed), "an alarm's bound");
	}

	/// The firmware's CI build has no car and is built on this plan (`vag-dash-fw`'s `build.rs`).
	/// Its id is pinned: it moves only when the generator spells every plan's source another
	/// way — after which a board reflashed with an unchanged `dash.toml` says, once, that its
	/// settings were saved against another plan.
	#[test]
	fn the_empty_plan_has_an_id_of_its_own() {
		let empty = Plan {
			vin: String::new(),
			language: "en".to_string(),
			units: Vec::new(),
			channels: Vec::new(),
			pages: Vec::new(),
			alarms: Vec::new(),
			stalk: None,
			stopwatch: None,
			buttons: Vec::new(),
			strip: None,
			standstill: None,
		};
		assert_eq!(format!("{:016x}", plan_id(&empty)), "edfbdbaf3754546a");
		assert_ne!(plan_id(&empty), plan_id(&every_part()));
	}

	#[test]
	fn fnv1a64_is_the_published_function() {
		// The reference vectors of the FNV specification (draft-eastlake-fnv).
		assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
		assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
		assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
	}

	/// Whether `BLESS` asks for the golden file to be rewritten: `1` and nothing else, so a
	/// `BLESS=0` left in a shell never rewrites it.
	fn blesses(value: Option<&std::ffi::OsStr>) -> bool {
		value == Some(std::ffi::OsStr::new("1"))
	}

	#[test]
	fn only_bless_1_rewrites_the_golden_file() {
		use std::ffi::OsStr;
		assert!(blesses(Some(OsStr::new("1"))));
		for no in [
			None,
			Some(OsStr::new("0")),
			Some(OsStr::new("")),
			Some(OsStr::new("true")),
			Some(OsStr::new("1 ")),
		] {
			assert!(!blesses(no), "BLESS={no:?} rewrote the golden file");
		}
	}

	const GENERATED_HEADER: &str = "// `to_rust` on the test fixture of `src/dash.rs` (a lever, a stopwatch, buttons and a strip),\n// not on any car's data. Compiled by `tests/generated_plan.rs`. Do not edit by hand: rewrite it with\n// `BLESS=1 cargo test -p vag-cli-core generated_source`, then run the tests again.\n";

	#[test]
	fn the_lever_and_the_stopwatch_reach_plan_json_and_the_rust_source() {
		let built = build_with_lever(&format!("{LEVER}{WATCH}")).unwrap();
		let json = built.plan.to_json();
		assert_eq!(from_json(&json).unwrap(), built.plan);
		assert!(json.contains("\"name\": \"limit\""), "a person reads the states by name: {json}");
		let rust = to_rust(&built.plan);
		for wanted in [
			"use vag_dash_render::plan::{Band, Channel, Page, Plan, StalkPlan, Standstill, StopwatchPlan, Unit};",
			"use vag_dash_render::stalk::{StateIndex, States};",
			"stalk: Some(StalkPlan { rocker: 3, switch: 4, cruise: 5, rocker_states: &STALK_ROCKER, switch_states: &STALK_SWITCH, cruise_states: &STALK_CRUISE, states: States { next: StateIndex(1), previous: StateIndex(2), measure: StateIndex(3), switch_off: StateIndex(1), cruise_off: StateIndex(0) } })",
			"stopwatch: Some(StopwatchPlan { speed: 1, km_h_per_unit: 0.0, marks: &MARKS })",
			"static STALK_ROCKER: [Band; 5] = [",
			"\tBand { lower: 51, upper: 101 }, // \"plus\"",
			"static MARKS: [u16; 2] = [60, 100];",
		] {
			assert!(rust.contains(wanted), "{wanted}\n{rust}");
		}
		// A plan without either says so, and names nothing it does not use.
		let bare = build_with_lever("").unwrap();
		let rust = to_rust(&bare.plan);
		assert!(rust.contains("stalk: None, stopwatch: None, buttons: &[], standstill: None };"), "{rust}");
		assert!(
			rust.contains("use vag_dash_render::plan::{Channel, Page, Plan, Unit};") && !rust.contains("States"),
			"{rust}"
		);
		assert!(!bare.plan.to_json().contains("stalk"), "an old reader sees the plan it knew");
	}

	#[test]
	fn an_unbounded_state_is_written_by_its_limit() {
		let mut built = build_with_lever(&format!("{LEVER}{WATCH}")).unwrap();
		let stalk = built.plan.stalk.as_mut().unwrap();
		stalk.cruise_states.push(State {
			lower: 3,
			upper: i32::MAX,
			name: "anything above".to_string(),
		});
		assert!(to_rust(&built.plan).contains("Band { lower: 3, upper: i32::MAX }"));
	}

	#[test]
	fn a_plan_json_from_before_the_lever_reads_without_one() {
		let built = build_with_lever("").unwrap();
		let back = from_json(&built.plan.to_json()).unwrap();
		assert_eq!((back.stalk, back.stopwatch), (None, None));
	}

	/// A button on each free pin, one of each action.
	const EVERY_ACTION: &str =
		"[[button]]\npin = 3\naction = \"next\"\n[[button]]\npin = 4\naction = \"previous\"\n[[button]]\npin = 5\naction = \"stopwatch\"\n";

	/// A minimal input — one channel, one page — and whatever the test appends.
	fn input_with(extra: &str) -> Result<Input, Error> {
		parse_input(&format!(
			"vin = \"TESTVIN0000000001\"\n[[channel]]\nref = \"01:IDE00001\"\n{}{extra}",
			values_page(&["01:IDE00001"])
		))
	}

	fn button(pin: &str, action: &str) -> String {
		format!("[[button]]\npin = {pin}\naction = {action}\n")
	}

	#[test]
	fn buttons_parse_in_the_files_order_and_two_may_share_an_action() {
		let input = input_with(&[button("5", "\"previous\""), button("3", "\" stopwatch \""), button("4", "\"previous\"")].concat()).unwrap();
		assert_eq!(
			input.buttons,
			[
				Button {
					pin: 5,
					action: Command::Previous
				},
				Button {
					pin: 3,
					action: Command::Stopwatch
				},
				Button {
					pin: 4,
					action: Command::Previous
				},
			],
			"an action is trimmed, like every string"
		);
		assert!(input_with("").unwrap().buttons.is_empty(), "no [[button]] is no buttons");
	}

	#[test]
	fn every_button_refusal_says_what_is_wrong() {
		let refused = |extra: &str| input_with(extra).unwrap_err().to_string();
		assert_eq!(
			refused("[button]\npin = 3\naction = \"next\"\n"),
			"dash.toml: line 8: [button] has one pair of brackets — write [[button]], two pairs: the file holds a list of them"
		);
		assert_eq!(
			refused(&["3", "4", "5", "3"].map(|pin| button(pin, "\"next\"")).concat()),
			"dash.toml: 4 [[button]] tables, and the board has 3 pins free for one: 3, 4 and 5"
		);
		let no_pin = "dash.toml: button #1 needs pin, a whole number: 3, 4 or 5";
		assert_eq!(refused("[[button]]\naction = \"next\"\n"), no_pin);
		assert_eq!(refused(&button("\"3\"", "\"next\"")), no_pin, "a string is not a pin");
		assert_eq!(refused(&button("3.0", "\"next\"")), no_pin, "nor a float");
		// Each taken pin says what holds it.
		for (pin, why) in [
			("0", "is the OLED's D/C"),
			("1", "is the CAN transceiver's RX"),
			("2", "is a strapping pin, read at reset"),
			("6", "is the CAN transceiver's TX"),
			("7", "is the OLED's SDIN"),
			("8", "is the LED's, and a strapping pin"),
			("9", "is the BOOT button's, and a strapping pin"),
			("10", "is the OLED's SCLK"),
			("11", "is not broken out on the SuperMini"),
			("12", "is the SPI flash's"),
			("17", "is the SPI flash's"),
			("18", "is USB's D−"),
			("19", "is USB's D+"),
			("20", "is the OLED's RES"),
			("21", "is the OLED's CS"),
			("22", "is not a GPIO of the ESP32-C3"),
			("-1", "is not a GPIO of the ESP32-C3"),
			("259", "is not a GPIO of the ESP32-C3"),
		] {
			assert_eq!(
				refused(&button(pin, "\"next\"")),
				format!("dash.toml: button #1: pin {pin} {why} — a button goes on pin 3, 4 or 5"),
				"pin {pin}"
			);
		}
		assert_eq!(
			refused(&[button("4", "\"next\""), button("4", "\"previous\"")].concat()),
			"dash.toml: button #2: pin 4 is button #1's already — one button per pin"
		);
		for action in ["\"Next\"", "\"measure\"", "\"\""] {
			let name = action.trim_matches('"');
			assert_eq!(
				refused(&button("3", action)),
				format!("dash.toml: button #1: action \"{name}\" is not \"next\", \"previous\" or \"stopwatch\""),
				"{action}"
			);
		}
		let no_action = "dash.toml: button #1 needs action: \"next\", \"previous\" or \"stopwatch\"";
		assert_eq!(refused("[[button]]\npin = 3\n"), no_action);
		assert_eq!(refused(&button("3", "1")), no_action, "an action is a string");
	}

	#[test]
	fn a_button_may_take_exactly_the_pins_nothing_else_holds() {
		for pin in -2..=40_i64 {
			let free = u8::try_from(pin).is_ok_and(is_button_pin);
			assert_eq!(pin_taken(pin).is_none(), free, "pin {pin}");
		}
		assert_eq!(BUTTON_PINS.len(), MAX_BUTTONS);
	}

	#[test]
	fn buttons_reach_the_plan_its_log_plan_json_the_rust_source_and_the_device() {
		let built = build_with_lever(&format!("{WATCH}{EVERY_ACTION}")).unwrap();
		let wanted = [(3, Command::Next), (4, Command::Previous), (5, Command::Stopwatch)].map(|(pin, action)| Button { pin, action });
		assert_eq!(built.plan.buttons, wanted);
		for line in ["button: pin 3 → next", "button: pin 4 → previous", "button: pin 5 → stopwatch"] {
			assert!(built.notes.iter().any(|n| n == line), "{line}: {:?}", built.notes);
		}
		assert!(
			!built.notes.iter().any(|n| n.starts_with("stopwatch: nothing opens")),
			"a stopwatch button opens the page: {:?}",
			built.notes
		);
		let json = built.plan.to_json();
		assert!(
			json.contains("\"action\": \"stopwatch\""),
			"a person reads an action by its dash.toml word: {json}"
		);
		assert_eq!(from_json(&json).unwrap(), built.plan);
		let rust = to_rust(&built.plan);
		for wanted in [
			"use vag_dash_render::control::Command;",
			"use vag_dash_render::plan::{ButtonPlan, Channel, Page, Plan, Standstill, StopwatchPlan, Unit};",
			"stopwatch: Some(StopwatchPlan { speed: 1, km_h_per_unit: 0.0, marks: &MARKS }), buttons: &BUTTONS, standstill: Some(Standstill::Speed(1)) };",
			"static BUTTONS: [ButtonPlan; 3] = [\n\tButtonPlan { pin: 3, action: Command::Next },\n\tButtonPlan { pin: 4, action: Command::Previous },\n\tButtonPlan { pin: 5, action: Command::Stopwatch },\n];",
		] {
			assert!(rust.contains(wanted), "{wanted}\n{rust}");
		}
		let device = built.plan.to_device();
		assert_eq!(
			device.buttons,
			wanted.map(|b| vag_dash_render::plan::ButtonPlan {
				pin: b.pin,
				action: b.action
			})
		);
	}

	#[test]
	fn a_plan_without_buttons_names_none() {
		let bare = build_with_lever("").unwrap();
		let json = bare.plan.to_json();
		assert!(!json.contains("buttons"), "an old reader sees the plan it knew: {json}");
		assert!(from_json(&json).unwrap().buttons.is_empty());
		let rust = to_rust(&bare.plan);
		assert!(rust.contains("buttons: &[], standstill: None };"), "{rust}");
		assert!(
			!rust.contains("Command") && !rust.contains("ButtonPlan"),
			"nothing imported unused: {rust}"
		);
		assert!(bare.plan.to_device().buttons.is_empty());
	}

	#[test]
	fn a_plan_json_with_an_action_the_board_has_not_is_refused() {
		let mut json = build_with_lever(EVERY_ACTION).unwrap().plan.to_json();
		json = json.replacen("\"previous\"", "\"measure\"", 1);
		let why = from_json(&json).unwrap_err().to_string();
		assert!(why.contains("action \"measure\" is not next, previous or stopwatch"), "{why}");
	}

	#[test]
	fn a_stopwatch_button_without_a_stopwatch_is_a_note_not_a_refusal() {
		let built = build_with_lever(EVERY_ACTION).unwrap();
		assert!(
			built
				.notes
				.iter()
				.any(|n| n == "button: pin 5 is a stopwatch button, and there is no [stopwatch] — a press of it only silences an alarm"),
			"{:?}",
			built.notes
		);
		let without = build_with_lever(&format!("{WATCH}{EVERY_ACTION}")).unwrap();
		assert!(
			!without.notes.iter().any(|n| n.contains("there is no [stopwatch]")),
			"{:?}",
			without.notes
		);
	}

	/// Whether `line` is in the build log, and then as advice — what `dev dash build` says by
	/// default — too: a line of advice is always in the log, and never only there.
	fn advised(built: &Built, line: &str) -> bool {
		let logged = built.notes.iter().any(|n| n == line);
		assert_eq!(built.advice.iter().any(|n| n == line), logged, "{line}: {:?}", built.advice);
		assert!(
			built.advice.iter().all(|a| built.notes.contains(a)),
			"advice outside the log: {:?}",
			built.advice
		);
		logged
	}

	#[test]
	fn a_stopwatch_nothing_opens_is_said() {
		let nothing = "stopwatch: nothing opens the page — give [stalk] a measure, or a [[button]] action = \"stopwatch\"";
		let says = |input: &str| advised(&build_with_lever(input).unwrap(), nothing);
		assert!(says(WATCH), "no lever, no button");
		assert!(says(&format!("{WATCH}{}", button("3", "\"next\""))), "a button that only pages");
		assert!(!says(&format!("{LEVER}{WATCH}")), "the lever's measure opens it");
		assert!(!says(&format!("{WATCH}{}", button("4", "\"stopwatch\""))), "so does a stopwatch button");
		assert!(!says(""), "no stopwatch, nothing to open");
	}

	/// BOOT and RESET page nothing (owner, 2026-09-27): a board flashed with pages and alarms and
	/// no input is one whose page only a bench tool turns, and whose alarm only its channel ends.
	#[test]
	fn a_board_with_nothing_to_turn_its_pages_or_silence_its_alarms_is_said() {
		let turns = "input: no [stalk] and no [[button]] with next or previous — only dashsim or dashcfg's set page turns the page";
		let silences = "input: no [stalk] and no [[button]] — an alarm stays up until its channel answers in range again; only dashsim silences it";
		let said = |input: &str, alarm: bool| {
			let extra = Extra {
				keys: [if alarm { "alarm_max = 10\nalarm_max_off = 5" } else { "" }, "", ""],
				..Extra::default()
			};
			let built = build_with_lever_and(input, extra).unwrap();
			(advised(&built, turns), advised(&built, silences))
		};
		let second = values_page_titled("U", &["01:IDE00001"]);
		assert_eq!(said("", false), (false, false), "one page and no alarm: nothing to turn or silence");
		assert_eq!(said(&second, false), (true, false), "two pages");
		assert_eq!(said("", true), (false, true), "an alarm");
		assert_eq!(said(&second, true), (true, true), "both");
		assert_eq!(said(&format!("{second}{LEVER}"), true), (false, false), "the lever pages and silences");
		for action in ["\"next\"", "\"previous\""] {
			assert_eq!(said(&format!("{second}{}", button("3", action)), true), (false, false), "{action}");
		}
		assert_eq!(
			said(&format!("{second}{WATCH}{}", button("3", "\"stopwatch\"")), true),
			(true, false),
			"a stopwatch button silences an alarm and turns no page"
		);
	}

	#[test]
	fn a_lever_with_no_stopwatch_to_open_is_said() {
		let note = "stalk: there is no [stopwatch] — a press of measure only silences an alarm";
		let says = |input: &str| advised(&build_with_lever(input).unwrap(), note);
		assert!(says(LEVER));
		assert!(!says(&format!("{LEVER}{WATCH}")), "measure opens the stopwatch");
		assert!(!says(""), "no lever");
	}

	#[test]
	fn every_text_the_project_does_not_have_is_refused_by_name() {
		let refused = |from: &str, to: &str| -> String {
			let input = format!("{LEVER}{WATCH}").replacen(from, to, 1);
			build_with_lever(&input).unwrap_err().to_string()
		};
		let why = refused("rocker = \"Rocker\"", "rocker = \"Rocket\"");
		assert!(why.contains("has no field named \"Rocket\"") && why.contains("\"Rocker\""), "{why}");
		let why = refused("rocker = \"Rocker\"", "rocker = \"Voltage\"");
		assert!(why.contains("a quantity, not a list of states"), "{why}");
		let why = refused("next = \"plus\"", "next = \"plu\"");
		assert!(
			why.contains("next: \"plu\" is not a state of \"Rocker\"") && why.contains("\"plus\""),
			"{why}"
		);
		let why = refused("measure = \"limit\"", "measure = \"plus\"");
		assert!(why.contains("same state"), "{why}");
		let why = refused("switch_off = \"off\"", "switch_off = \"Off\"");
		assert!(why.contains("switch_off: \"Off\" is not a state of \"Switch\""), "{why}");
		let why = refused("switch = \"Switch\"", "switch = \"Rocker\"");
		assert!(why.contains("rocker and switch are both"), "{why}");
		let why = refused("read = \"75A:4C21\"", "read = \"75A:4C22\"");
		assert!(why.contains("declares no such identifier"), "{why}");
		let why = refused("cruise = \"01:2001\"", "cruise = \"01:2002\"");
		assert!(why.contains("cruise 01:2002: a quantity, not a list of states"), "{why}");
		let why = refused("cruise = \"01:2001\"", "cruise = \"01:2009\"");
		assert!(why.contains("declares no such field"), "{why}");
		let why = refused("cruise_off = \"off\"", "cruise_off = \"main switch off\"");
		assert!(why.contains("cruise_off: \"main switch off\" is not a state of"), "{why}");
		assert!(why.starts_with("[stalk] "), "{why}");
	}

	#[test]
	fn a_stopwatch_the_plan_cannot_honour_is_refused() {
		let refused = |from: &str, to: &str| -> String { build_with_lever(&WATCH.replacen(from, to, 1)).unwrap_err().to_string() };
		let why = refused("01:IDE00010", "01:IDE00001x");
		assert!(why.contains("is not in the [[channel]] list"), "{why}");
		let why = refused("01:IDE00010", "01:IDE00011");
		assert!(why.contains("offset"), "{why}");
		for (marks, says) in [
			("[0, 100]", "above 0"),
			("[60, 60]", "listed twice"),
			("[20, 40, 60, 80]", "1 to 3"),
			("[]", "1 to 3"),
			("[60.5]", "whole speed"),
		] {
			let why = refused("[60, 100]", marks);
			assert!(why.contains(says), "{marks}: {why}");
		}
		for factor in ["-0.1", "\"fast\"", "nan", "-inf"] {
			let why = refused("0.0", factor);
			assert!(why.contains("km_h_per_unit must be a number at or above 0"), "{factor}: {why}");
		}
		let why = refused("0.0", "1e-50");
		assert!(why.contains("too small for the board"), "{why}");
		// A number, and above 0: what is wrong is that the board's `f32` cannot hold it.
		for factor in ["1e40", "inf"] {
			let why = refused("0.0", factor);
			assert!(
				why.contains(&format!("km_h_per_unit {factor} is too large for the board")),
				"{factor}: {why}"
			);
		}
		// A scaling that reads forward as zero or backwards; appended `[[channel]]`s, read fast.
		for (text_id, factor) in [("IDE00012", "-1"), ("IDE00013", "0")] {
			let watch = WATCH.replacen("IDE00010", text_id, 1);
			let why = build_with_lever(&format!("{watch}[[channel]]\nref = \"01:{text_id}\"\nhz = 50\n"))
				.unwrap_err()
				.to_string();
			assert!(why.contains(&format!("factor {factor} is not above zero")), "{text_id}: {why}");
		}
		// A speed read at the default rate: every run would cross its marks with no time.
		let why = refused("01:IDE00010", "01:IDE00001");
		assert!(
			why.contains("is read every 500 ms") && why.contains("hz of 7.5 or more; 50 is recommended"),
			"{why}"
		);
		let measured = build_with_lever(&WATCH.replacen("0.0", "0.0271", 1)).unwrap();
		assert_eq!(measured.plan.stopwatch.unwrap().km_h_per_unit, 0.0271);
	}

	/// The board reads a channel every `1000 / hz` ms rounded (`Channel::period_ms`), and the
	/// launch fit wants `MIN_FIT_SAMPLES` moving samples in its first `START_FIT_MS` — with one
	/// answer late, three periods in 400 ms: every 133 ms or sooner.
	#[test]
	fn a_speed_the_board_reads_too_seldom_for_a_launch_fit_with_one_answer_late_is_refused() {
		let at = |hz: &str| {
			let another = Extra {
				rows: vec![(ENGINE, reading(0x3005, "Speed too", "IDE00014", 0, 16, false, true, 1.0, 0.0))],
				..Extra::default()
			};
			let watch = WATCH.replacen("IDE00010", "IDE00014", 1);
			build_with_lever_and(&format!("{watch}[[channel]]\nref = \"01:IDE00014\"\nhz = {hz}\n"), another)
		};
		// 5.01 Hz is 199.6 ms, which the board polls as 200: 5 Hz, and two periods fill the
		// window. 7.49 Hz is 133.5 ms, polled as 134: three periods are 402 ms.
		for (hz, every) in [("5.01", 200), ("6", 167), ("7.49", 134)] {
			let why = at(hz).unwrap_err().to_string();
			assert!(
				why.starts_with(&format!("[stopwatch] speed 01:IDE00014 is read every {every} ms (hz = {hz})"))
					&& why.contains("3 samples in its first 400 ms even when one answer is late, so a reading every 133 ms or sooner"),
				"{hz}: {why}"
			);
		}
		// The rate the message names is one the board polls fast enough.
		for hz in ["7.5", "8", "50"] {
			at(hz).unwrap_or_else(|e| panic!("{hz} Hz: {e}"));
		}
	}

	/// The firmware asserts that the stopwatch's silence outlasts two answer timeouts and the
	/// slowest period the speed may be read at, `SLOWEST_SPEED_PERIOD_MS`: that period is the one
	/// this build lets through, not a slower one it refuses (PR #12 review: it was 200 ms against
	/// the build's 133).
	#[test]
	fn the_slowest_speed_the_build_takes_is_the_stopwatchs_slowest_period() {
		use vag_dash_render::stopwatch::SLOWEST_SPEED_PERIOD_MS;
		let builds = |hz: f64| {
			let another = Extra {
				rows: vec![(ENGINE, reading(0x3005, "Speed too", "IDE00014", 0, 16, false, true, 1.0, 0.0))],
				..Extra::default()
			};
			let watch = WATCH.replacen("IDE00010", "IDE00014", 1);
			build_with_lever_and(&format!("{watch}[[channel]]\nref = \"01:IDE00014\"\nhz = {hz:?}\n"), another).is_ok()
		};
		// Around the edge, and at the rates a period of 200 ms is.
		for hz in [7.55, 7.5, 7.49, 7.45, 6.0, 5.01, 5.0, 4.99] {
			let period = u64::from(board_period_ms(hz));
			assert_eq!(builds(hz), period <= SLOWEST_SPEED_PERIOD_MS, "{hz} Hz, read every {period} ms");
		}
	}

	#[test]
	fn a_stalk_read_that_names_a_field_rather_than_an_identifier_is_refused_when_parsed() {
		let why = build_with_lever(&LEVER.replacen("75A:4C21", "75A:4C21@8", 1)).unwrap_err().to_string();
		assert!(why.contains("is not <unit>:<DID>"), "{why}");
		let why = build_with_lever("[[stalk]]\nread = \"75A:4C21\"\n").unwrap_err().to_string();
		assert!(why.contains("one [stalk] table"), "{why}");
	}

	/// A proven row in `measurements/<key>.json`, enumerated, at one field: `raw_form` places
	/// it, and `levels` are its states as the file holds them.
	fn proven_states(did: u16, name: &str, raw_form: RawForm, levels: Vec<Level>) -> MeasurementDef {
		MeasurementDef {
			name: name.to_string().into(),
			unit: "".into(),
			address: ReadId::Uds(did),
			raw_form,
			scaling: Scaling::Enum { levels },
		}
	}

	/// [`build_with_lever_in`] with `proven` written as the unit's catalog, `<key>.json`,
	/// beside the cache.
	fn build_with_proven(input: &str, extra: Extra, key: &str, proven: Vec<MeasurementDef>) -> Result<Built, Error> {
		let here = tempfile::tempdir().unwrap();
		build_with_lever_in(here.path(), input, extra, |cache| {
			let dir = cache.parent().unwrap().join("proven");
			std::fs::create_dir_all(&dir).unwrap();
			std::fs::write(dir.join(format!("{key}.json")), MeasurementCatalog::new(proven).to_json().unwrap()).unwrap();
		})
	}

	/// A proven row wins at its field, and one written before state ranges holds every state
	/// as a single value: where the project gives the field bands, a reading inside one would
	/// be no state at all, and the lever would never press. The same rule as a cache that
	/// predates the bands, for the other place a state can come from.
	#[test]
	fn a_proven_row_whose_states_predate_their_bands_is_refused_naming_its_file() {
		// The rocker's byte, as the declared row places it (bit 16, one byte).
		let byte_2 = RawForm::Int {
			byte_offset: 2,
			byte_length: 1,
			signed: false,
			big_endian: true,
		};
		let points = vec![
			Level::point(20, "rest"),
			Level::point(76, "plus"),
			Level::point(127, "minus"),
			Level::point(178, "limit"),
		];
		let why = build_with_proven(LEVER, Extra::default(), "PART2", vec![proven_states(STALK_DID, "Rocker", byte_2, points)])
			.unwrap_err()
			.to_string();
		assert!(
			why.starts_with("[stalk] rocker: the proven row for \"Rocker\" in ") && why.contains("PART2.json") && why.contains("predates state ranges"),
			"{why}"
		);

		// The cruise status, on a declared ladder, proven as points.
		let ladder_status = Extra {
			rows: vec![(ENGINE, state_reading(0x2006, "Cruise ladder", "IDE00023", 0, 8, ladder(&["off", "on"])))],
			..Extra::default()
		};
		let why = build_with_proven(
			&LEVER.replacen("01:2001", "01:IDE00023", 1),
			ladder_status,
			"PART1",
			vec![proven_states(
				0x2006,
				"Cruise ladder",
				RawForm::U8First,
				vec![Level::point(10, "off"), Level::point(200, "on")],
			)],
		)
		.unwrap_err()
		.to_string();
		assert!(
			why.starts_with("[stalk] cruise: the proven row for \"Cruise ladder\" in ") && why.contains("PART1.json"),
			"{why}"
		);

		// A status the project itself gives as single values is one: proven so, it builds.
		let status = vec![Level::point(0, "off"), Level::point(1, "standby"), Level::point(2, "passive")];
		let built = build_with_proven(
			LEVER,
			Extra::default(),
			"PART1",
			vec![proven_states(0x2001, "Cruise status", RawForm::U16Be, status)],
		)
		.expect("points where the project has points");
		assert!(built.plan.channels[5].proven, "the proven row is the one used");
		// And a proven rocker written with ranges is the proven row, used.
		let ranges = vec![
			Level::range(0, 60, "rest"),
			Level::range(61, 101, "plus"),
			Level::range(102, 152, "minus"),
			Level::range(153, 203, "limit"),
		];
		let built = build_with_proven(LEVER, Extra::default(), "PART2", vec![proven_states(STALK_DID, "Rocker", byte_2, ranges)]).unwrap();
		assert_eq!(built.plan.stalk.unwrap().rocker_states[0].upper, 60);
	}

	/// A cache an older vagcan added the levels' upper ends to in place has every state as
	/// its lower end alone: the rocker would never read as pressed. Refused, with what to do.
	#[test]
	fn a_cache_whose_states_predate_their_bands_is_refused_with_what_to_run() {
		let here = tempfile::tempdir().unwrap();
		build_with_lever_in(here.path(), LEVER, Extra::default(), |_| {}).expect("builds on a cache that has the bands");
		let here = tempfile::tempdir().unwrap();
		let old_shape = |cache: &Path| {
			let conn = rusqlite::Connection::open(cache).unwrap();
			conn.execute("UPDATE reading_level SET upper = NULL", []).unwrap();
		};
		let why = build_with_lever_in(here.path(), LEVER, Extra::default(), old_shape)
			.unwrap_err()
			.to_string();
		assert!(why.contains("run `vagcan setup` again"), "{why}");
	}
}
