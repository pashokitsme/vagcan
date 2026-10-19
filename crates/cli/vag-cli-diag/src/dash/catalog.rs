//! Every channel a car's dash build can resolve — what `dev dash edit` offers to put on a page.
//!
//! The rows are `watch`'s ([`plan::available`]) and every verdict is [`dash::build`]'s own: a
//! row is numeric only when a `[[channel]]` naming it builds, to that very row. So the editor
//! offers nothing a `dash.toml` would refuse, and names each row the way `dash.toml` takes it.
//! Pure: reads its arguments, writes nothing.
//!
//! Moved here from `research/dash/layout-editor/catalog` (2026-10-19). What stayed behind is
//! the research evidence — which identifiers answered in the August surveys and recordings of
//! one car — which is not data a command has for any other.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use vag_cli_core::config::Language;
use vag_cli_core::dash::{self, ChannelInput, Error as BuildError, Input, Reference};
use vag_cli_core::extracted::Extracted;
use vag_cli_core::plan::{self, UnitIdentity};
use vag_data_labels::catalog::{CatalogStore, Scaling};

/// One row of the catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
	/// How `dash.toml` names it: its text id where that names this row alone, its
	/// `<unit>:<DID>[@<bit>]` otherwise.
	pub reference: String,
	/// Every other spelling the build reads as this row.
	pub aliases: Vec<String>,
	/// The unit as every command writes it, `01` or `713`.
	pub unit: String,
	/// What the unit called itself, `F197`.
	pub component: String,
	/// The project's name for the row.
	pub name: String,
	/// What the panel shows without a `label`: the build's, for a numeric row.
	pub label: String,
	pub did: u16,
	pub unit_of_measure: String,
	/// A `[[channel]]` naming it builds.
	pub numeric: bool,
	/// The build's decimals, for a numeric row.
	pub decimals: Option<u8>,
	/// Measured on a car, not only in the project's data.
	pub proven: bool,
	/// Why a row is not numeric: the build's refusal, without the reference it starts with.
	pub refused: Option<String>,
}

/// A `[[channel]]` with nothing but its reference.
fn probe(reference: Reference) -> ChannelInput {
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

fn input_of(vin: &str, language: Option<Language>, refs: &[Reference]) -> Input {
	Input {
		vin: vin.to_string(),
		language,
		channels: refs.iter().cloned().map(probe).collect(),
		pages: Vec::new(),
		stalk: None,
		stopwatch: None,
		buttons: Vec::new(),
		strip: None,
	}
}

/// The reference a refusal is about, when it is about one channel.
fn culprit(e: &BuildError) -> Option<&Reference> {
	match e {
		BuildError::Undeclared(r)
		| BuildError::NoChannels(r)
		| BuildError::Ambiguous(r, _)
		| BuildError::NotLinear(r, _)
		| BuildError::Duplicate(r)
		| BuildError::NotFinite(r)
		| BuildError::Alarm(r, _) => Some(r),
		BuildError::NameNotId { reference, .. } => Some(reference),
		BuildError::SameRow { second, .. } => Some(second),
		_ => None,
	}
}

/// Everything the catalog is made from.
pub struct Sources<'a> {
	pub vin: &'a str,
	/// `dash.toml`'s `language`, where it has one.
	pub language: Option<Language>,
	/// `config.toml`'s, for an input without one.
	pub default_language: Language,
	pub store: &'a CatalogStore,
	pub extracted: &'a Extracted,
	pub units: &'a [UnitIdentity],
}

impl Sources<'_> {
	/// Build `refs` as one input's `[[channel]]`s; a refusal names its channel, which is taken
	/// out and the rest built again. Each reference's verdict, by its index in `refs`.
	fn verdicts(&self, refs: &[Reference]) -> BTreeMap<usize, Result<dash::Channel, String>> {
		let mut verdict = BTreeMap::new();
		let mut pending: Vec<usize> = (0..refs.len()).collect();
		while !pending.is_empty() {
			let these: Vec<Reference> = pending.iter().map(|&i| refs[i].clone()).collect();
			let built = dash::build(
				&input_of(self.vin, self.language, &these),
				self.store,
				self.extracted,
				self.units,
				self.default_language,
			);
			match built {
				// The input's own channels come first in the plan, one each and in order.
				Ok(built) if built.plan.channels.len() >= pending.len() => {
					for (&i, channel) in pending.iter().zip(&built.plan.channels) {
						verdict.insert(i, Ok(channel.clone()));
					}
					break;
				}
				Ok(built) => {
					let why = format!("{} channels built for {} asked", built.plan.channels.len(), pending.len());
					for &i in &pending {
						verdict.insert(i, Err(why.clone()));
					}
					break;
				}
				Err(e) => match culprit(&e).and_then(|r| pending.iter().position(|&i| refs[i] == *r)) {
					Some(at) => {
						let i = pending.remove(at);
						let said = e.to_string();
						let prefix = format!("{}: ", refs[i]);
						verdict.insert(i, Err(said.strip_prefix(&prefix).unwrap_or(&said).to_string()));
					}
					// A refusal of the whole input names nobody: every row left gets it.
					None => {
						for &i in &pending {
							verdict.insert(i, Err(e.to_string()));
						}
						break;
					}
				},
			}
		}
		verdict
	}
}

/// The catalog: every row of the car's units the project describes, sorted by unit, name and
/// reference.
pub fn rows(sources: &Sources<'_>) -> Vec<Row> {
	let language = sources.language.unwrap_or(sources.default_language);
	let respoken;
	let extracted = if sources.extracted.language() == language {
		sources.extracted
	} else {
		respoken = sources.extracted.clone().in_language(language);
		&respoken
	};
	let offered = plan::available(sources.store, extracted, sources.units);
	let described: Vec<&plan::Channel> = offered.iter().filter(|c| c.def.is_some()).collect();
	let linear = |c: &plan::Channel| matches!(c.def.as_ref().map(|d| &d.scaling), Some(Scaling::Linear(_)));
	let bit_offset = |c: &plan::Channel| c.def.as_ref().map_or(0, |d| d.raw_form.bit_offset());

	// How many rows of a unit carry each text id, and how many of those are linear.
	let mut by_id: BTreeMap<(u16, &str), (usize, usize)> = BTreeMap::new();
	for c in &described {
		if let Some(id) = &c.text_id {
			let e = by_id.entry((c.request, id.as_str())).or_default();
			e.0 += 1;
			e.1 += usize::from(linear(c));
		}
	}
	// One reference per row: its text id where that names this row and no other — the rule the
	// build applies (a single linear row wins over enum rows sharing the id; otherwise the id
	// must be the only match) — and `<unit>:<DID>[@bit]` otherwise. A spelling two rows share
	// keeps the row the build resolves it to: the first linear one, or with none linear the
	// first. `plan::available` holds one row per field today, so two rows at one identifier and
	// bit do not reach here; this keeps the build's rule if that changes.
	let mut taken: BTreeMap<String, usize> = BTreeMap::new();
	let mut picked: Vec<(&plan::Channel, Reference)> = Vec::new();
	for c in &described {
		let by_text = c.text_id.as_deref().filter(|id| {
			let (all, lin) = by_id[&(c.request, *id)];
			if linear(c) { lin == 1 } else { all == 1 }
		});
		let field = Reference::Field {
			request: c.request,
			did: c.did,
			bit_offset: bit_offset(c),
		};
		let reference = match by_text {
			Some(id) => Reference::TextId {
				request: c.request,
				text_id: id.to_string(),
			},
			None => field,
		};
		match taken.get(&reference.to_string()) {
			None => {
				taken.insert(reference.to_string(), picked.len());
				picked.push((c, reference));
			}
			Some(&at) if linear(c) && !linear(picked[at].0) => picked[at] = (c, reference),
			Some(_) => {}
		}
	}

	// Linear rows through the build, all at once; anything else is not a number the board can
	// show, which is what the build would say of it one at a time.
	let numeric_refs: Vec<usize> = (0..picked.len()).filter(|&i| linear(picked[i].0)).collect();
	let asked: Vec<Reference> = numeric_refs.iter().map(|&i| picked[i].1.clone()).collect();
	let verdicts = sources.verdicts(&asked);
	let mut verdict: BTreeMap<usize, Result<dash::Channel, String>> =
		numeric_refs.iter().enumerate().map(|(k, &i)| (i, verdicts[&k].clone())).collect();
	for (i, (c, _)) in picked.iter().enumerate() {
		verdict.entry(i).or_insert_with(|| {
			Err(
				match c.def.as_ref().map(|d| &d.scaling) {
					Some(Scaling::Enum { .. }) => "scaling is an enumeration, not linear — the device can multiply and nothing else",
					_ => "scaling is a single proven point with no slope, not linear — the device can multiply and nothing else",
				}
				.to_string(),
			)
		});
	}

	let component: BTreeMap<u16, String> = sources
		.units
		.iter()
		.map(|u| (u.request, u.component.clone().unwrap_or_default()))
		.collect();
	let mut out: Vec<Row> = picked
		.iter()
		.enumerate()
		.map(|(i, (c, reference))| {
			let def = c.def.as_ref().expect("described");
			let built = &verdict[&i];
			// A row built to another read than its own is not this row's verdict.
			let built = match built {
				Ok(ch) if (ch.unit, ch.did, ch.bit_offset) != (c.request, c.did, def.raw_form.bit_offset()) => {
					Err(format!("built as {:04X}@{}, not this row", ch.did, ch.bit_offset))
				}
				other => other.clone(),
			};
			Row {
				reference: reference.to_string(),
				aliases: aliases(reference, c),
				unit: c.unit(),
				component: component.get(&c.request).cloned().unwrap_or_default(),
				name: def.name.to_string(),
				label: built.as_ref().map_or_else(|_| c.label(), |ch| ch.label.clone()),
				did: c.did,
				unit_of_measure: def.unit.to_string(),
				numeric: built.is_ok(),
				decimals: built.as_ref().ok().map(|ch| ch.decimals),
				proven: c.proven,
				refused: built.err(),
			}
		})
		.collect();
	out.sort_by(|a, b| (&a.unit, &a.name, &a.reference).cmp(&(&b.unit, &b.name, &b.reference)));
	out
}

/// Every other spelling the build reads as the row `reference` names: the identifier form of a
/// row named by text id, and each form with the unit's request id where it has a short number.
fn aliases(reference: &Reference, c: &plan::Channel) -> Vec<String> {
	let field = Reference::Field {
		request: c.request,
		did: c.did,
		bit_offset: c.def.as_ref().map_or(0, |d| d.raw_form.bit_offset()),
	};
	let mut forms = vec![reference.to_string()];
	if *reference != field {
		forms.push(field.to_string());
	}
	let short = c.unit();
	let long = format!("{:03X}", c.request);
	let mut out = Vec::new();
	for form in &forms {
		if *form != reference.to_string() {
			out.push(form.clone());
		}
		if short != long
			&& let Some(rest) = form.strip_prefix(&short)
		{
			out.push(format!("{long}{rest}"));
		}
	}
	out
}

/// The rows as the page reads them, `channels.json`.
pub fn to_json(rows: &[Row]) -> Value {
	Value::Array(
		rows
			.iter()
			.map(|r| {
				json!({
					"ref": r.reference,
					"aliases": r.aliases,
					"unit": r.unit,
					"unitName": r.component,
					"name": r.name,
					"label": r.label,
					"did": format!("{:04X}", r.did),
					"uom": r.unit_of_measure,
					"numeric": r.numeric,
					"decimals": r.decimals,
					"proven": r.proven,
					"refused": r.refused,
				})
			})
			.collect(),
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use vag_data_labels::Level;
	use vag_data_labels::odis::Reading;

	fn reading(did: u16, name: &str, text_id: &str, bit_offset: u32, bit_length: u32, factor: f64) -> Reading {
		Reading {
			did,
			name: name.to_string(),
			unit: Some("°C".to_string()),
			bit_offset,
			bit_length,
			signed: false,
			big_endian: true,
			text_id: Some(text_id.to_string()),
			scaling: Scaling::Linear(vag_data_labels::measure::LinearScale { factor, offset: 0.0 }),
		}
	}

	#[test]
	fn every_numeric_row_builds_to_itself_and_the_rest_say_why_not() {
		let here = tempfile::tempdir().unwrap();
		let cache = here.path().join("cache.sqlite");
		let mut flag = reading(0x1003, "Gear", "IDE00003", 0, 8, 1.0);
		flag.unit = None;
		flag.scaling = Scaling::Enum {
			levels: vec![Level::point(1, "N"), Level::point(2, "1")],
		};
		let readings = vec![
			reading(0x1001, "Coolant", "IDE00001", 0, 8, 1.0),
			// One text id on two linear rows: neither is named by it, both by identifier.
			reading(0x1002, "Pair low", "IDE00002", 0, 8, 1.0),
			reading(0x1002, "Pair high", "IDE00002", 8, 8, 1.0),
			flag,
			// A factor the board's f32 holds as 0.
			reading(0x1004, "Tiny", "IDE00004", 0, 8, 1e-60),
			// An enumeration and a linear row at one identifier and bit, neither with a text id:
			// the project's rows reach `watch`'s list as one row per field.
			Reading {
				text_id: None,
				unit: None,
				scaling: Scaling::Enum {
					levels: vec![Level::point(0, "off")],
				},
				..reading(0x1005, "Mode", "", 0, 8, 1.0)
			},
			Reading {
				text_id: None,
				..reading(0x1005, "Pressure", "", 0, 8, 0.5)
			},
		];
		vag_data_db::put_readings(&cache, "/nowhere/TEST", "EV_Test_001", &readings).unwrap();
		let extracted = Extracted::synthetic(cache, Default::default());
		let store = CatalogStore::open(here.path().join("proven"));
		let units = [UnitIdentity {
			request: plan::ENGINE,
			part_number: Some("PART1".to_string()),
			odx_name: Some("EV_Test".to_string()),
			odx_version: Some("001004".to_string()),
			component: Some("ENGINE".to_string()),
		}];
		let sources = Sources {
			vin: "TESTVIN0000000001",
			language: None,
			default_language: Language::En,
			store: &store,
			extracted: &extracted,
			units: &units,
		};
		let rows = rows(&sources);
		let row = |r: &str| rows.iter().find(|x| x.reference == r).unwrap_or_else(|| panic!("{r} in {rows:#?}"));

		let coolant = row("01:IDE00001");
		assert!(coolant.numeric && coolant.refused.is_none(), "{coolant:?}");
		assert_eq!(coolant.aliases, ["7E0:IDE00001", "01:1001", "7E0:1001"]);
		assert_eq!(coolant.component, "ENGINE");
		assert!(row("01:1002").numeric && row("01:1002@8").numeric, "named by identifier and bit");
		let gear = row("01:IDE00003");
		assert!(!gear.numeric && gear.refused.as_deref().unwrap().contains("enumeration"), "{gear:?}");
		let tiny = row("01:IDE00004");
		assert!(!tiny.numeric && tiny.refused.as_deref().unwrap().contains("32-bit float"), "{tiny:?}");
		// Two rows of the project at one field are one row offered, and one spelling.
		assert_eq!(rows.iter().filter(|r| r.reference == "01:1005").count(), 1, "{rows:#?}");
		// Every row says what the build says of its reference alone: numeric ones build (below),
		// and the rest are refused.
		for r in rows.iter().filter(|r| !r.numeric) {
			let built = dash::build(
				&input_of(sources.vin, None, &[Reference::parse(&r.reference).unwrap()]),
				&store,
				&extracted,
				&units,
				Language::En,
			);
			assert!(built.is_err(), "{} is offered as not a number, and builds", r.reference);
		}

		// Each numeric row, alone in a dash.toml, builds — and under every alias too.
		for r in rows.iter().filter(|r| r.numeric) {
			for spelled in std::iter::once(&r.reference).chain(&r.aliases) {
				let built = dash::build(
					&input_of(sources.vin, None, &[Reference::parse(spelled).unwrap()]),
					&store,
					&extracted,
					&units,
					Language::En,
				)
				.unwrap_or_else(|e| panic!("{spelled}: {e}"));
				assert_eq!(built.plan.channels[0].did, r.did, "{spelled}");
			}
		}
		// The engine also carries the standard's OBD-II rows, which `watch` offers too.
		assert!(rows.iter().any(|r| r.reference == "01:F405" && r.numeric), "{rows:#?}");
		let json = to_json(&rows);
		let coolant = json.as_array().unwrap().iter().find(|r| r["ref"] == "01:IDE00001").unwrap();
		assert_eq!(
			coolant,
			&json!({
				"ref": "01:IDE00001", "aliases": ["7E0:IDE00001", "01:1001", "7E0:1001"], "unit": "01", "unitName": "ENGINE",
				"name": "Coolant", "label": "Coolant", "did": "1001", "uom": "°C", "numeric": true, "decimals": 0,
				"proven": false, "refused": null,
			})
		);
	}
}
