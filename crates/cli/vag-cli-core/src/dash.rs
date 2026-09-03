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
}

/// One `[[page]]` of the input.
#[derive(Debug, Clone, PartialEq)]
pub enum PageInput {
	Values { title: String, cells: Vec<Reference> },
	Chart { cell: Reference, min: f64, max: f64 },
}

/// The whole input, parsed and nothing more.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
	pub vin: String,
	pub language: Option<Language>,
	pub survey: Option<PathBuf>,
	pub channels: Vec<ChannelInput>,
	pub pages: Vec<PageInput>,
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
			channels.push(ChannelInput { reference, label, decimals });
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
	Ok(Input {
		vin,
		language,
		survey,
		channels,
		pages,
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
			Error::NotAnswered(r) => write!(f, "{r}: the survey asked the unit for this identifier and it did not answer"),
			Error::NotFinite(r) => write!(f, "{r}: its scaling is not a finite number"),
			Error::NoPartNumber(r) => write!(
				f,
				"unit {r:03X}: the survey has no part number (F187) for it, and the firmware checks the unit against the plan by that"
			),
			Error::PageRefersToUnknown { page, reference } => write!(f, "page #{page}: {reference} is not in the [[channel]] list"),
			Error::Page(n, why) => write!(f, "page #{n}: {why}"),
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
	/// Where the row came from: its text id, or the catalog's own name.
	pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Page {
	Chart { channel: u16, min: f64, max: f64 },
	Values { title: String, cells: Vec<u16> },
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
}

impl Plan {
	pub fn to_json(&self) -> String {
		serde_json::to_string_pretty(self).expect("a plan serialises")
	}

	pub fn from_json(text: &str) -> Result<Plan, serde_json::Error> {
		serde_json::from_str(text)
	}
}

/// The plan and the build log that explains every row of it.
#[derive(Debug, Clone, PartialEq)]
pub struct Built {
	pub plan: Plan,
	pub notes: Vec<String>,
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
		let request = wanted.reference.request();
		if index_of.contains_key(&wanted.reference) {
			return Err(Error::Duplicate(wanted.reference.clone()));
		}
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
		notes.push(format!(
			"{label} ← {} {did:04X}@{bit_offset}/{bit_length} {} {}{} ×{factor} {offset:+} {} ({})",
			wanted.reference,
			if signed { "i" } else { "u" },
			if big_endian { "BE" } else { "LE" },
			if bit_length % 8 == 0 { "" } else { " bits" },
			if found.proven { "proven" } else { "declared" },
			source
		));
		let index = channels.len() as u16;
		channels.push(Channel {
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
			source,
		});
		index_of.insert(wanted.reference.clone(), index);
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

	let mut pages = Vec::new();
	for (i, page) in input.pages.iter().enumerate() {
		let n = i + 1;
		let index = |r: &Reference| {
			index_of.get(r).copied().ok_or_else(|| Error::PageRefersToUnknown {
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

	Ok(Built {
		plan: Plan {
			vin: input.vin.clone(),
			language: language.code().to_string(),
			units: plan_units,
			channels,
			pages,
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
	let _ = writeln!(out);
	let _ = writeln!(
		out,
		"pub static PLAN: Plan = Plan {{ vin: {:?}, language: {:?}, units: &UNITS, channels: &CHANNELS, pages: &PAGES }};",
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
			"\tChannel {{ unit: 0x{:03X}, did: 0x{:04X}, bit_offset: {}, bit_length: {}, signed: {}, big_endian: {}, factor: {}, offset: {}, decimals: {}, unit_text: {:?}, label: {:?}, proven: {} }},",
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
			c.proven
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

/// The whole command: read the input, the survey and the project, build,
/// write `plan.json` and `plan.rs` under `~/.vagcan/dash/<VIN>/`.
///
/// `input` defaults to `dash.toml` in that directory. The firmware's build
/// script calls this too, so `cargo build` of the firmware *is* the plan build.
pub fn build_for_car(vin: &str, input: Option<&Path>) -> anyhow::Result<Written> {
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
		input_path.clone(),
		survey_path.clone(),
		project.cache(),
		project.measurements_dir(),
		project.names(),
		crate::config::path()?,
		crate::glossary::path()?,
	];

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
		let list: Vec<String> = cells.iter().map(|c| format!("\"{c}\"")).collect();
		format!("[[page]]\nkind = \"values\"\ntitle = \"T\"\ncells = [{}]\n", list.join(", "))
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
		assert!(rust.contains("did: 0xF405, bit_offset: 0, bit_length: 8, signed: false, big_endian: true, factor: 1.0, offset: -40.0, decimals: 0, unit_text: \"°C\", label: \"ОЖ\", proven: false"), "{rust}");
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
