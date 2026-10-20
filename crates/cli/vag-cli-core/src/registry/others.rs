//! The rows a read of the measurement registry brings along from VCDS's other
//! registries: adaptations, basic settings, output tests, freeze frames, the
//! `[XPL]` bits, and the MUX cases a row of type 6 defers to
//! ([`vag_data_labels::registries`]).
//!
//! **Stored, not acted on.** Nothing reads these rows back yet; a later reader may
//! show freeze frames through `0x19` and adaptation values through `0x22`. Basic
//! settings and output tests are names only — their services (`0x31`, `0x2F`) are
//! outside the allowlist, and nothing in this tool may ever start one.
//!
//! Read for the same units, from the same installation, in the same run as the
//! channels: a unit's lists and the registries they point into mean something
//! only together.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use vag_data_labels::label_files::{Lookup, UnitRod};
use vag_data_labels::registries::{self, Described, Entry, Load, Mux, Registry, Section};
use vag_data_labels::registry::{UnitMeasurements, Words, list_in};
use vag_data_labels::rod::IvCache;
use vag_data_labels::ttdop::TextTables;
use vag_data_labels::unit_strings::UnitStrings;

/// The registries of one installation, as far as they opened.
#[derive(Default)]
pub(super) struct Others {
	opened: Vec<(Section, Registry)>,
	mux: Option<Mux>,
	/// The rows read, by unit file stem.
	pub(super) rows: BTreeMap<String, Vec<Described>>,
	/// MUX keys already written per stem, so a key two rows name is written once.
	muxed: BTreeSet<(String, u32)>,
	/// Listed rows that did not read.
	pub(super) unread: usize,
	/// Lists that are there and did not open: a key not found, a shifted file.
	pub(super) shut: usize,
}

/// What one unit's lists came to, before it is put to words.
#[derive(Default)]
pub(super) struct Unit {
	/// The name its rows go by — the unit file's stem, or the redirect's source
	/// ([`UnitRod::name`]); `None` for a unit with no file.
	stem: Option<String>,
	sections: Vec<(Section, Vec<Entry>)>,
	/// The MUX keys its rows of type 6 name, its `[MWB]` rows' among them.
	muxes: Vec<u32>,
	unread: usize,
	shut: usize,
}

impl Others {
	/// Open every registry, saying in `notes` which did not open and why. The keys
	/// are saved after each, so a key searched for is kept however the run ends.
	pub(super) fn open(root: &Path, keys: &mut super::Keys<'_>, notes: &mut Vec<String>, progress: &mut impl super::Progress) -> Others {
		let mut others = Others::default();
		for section in Section::ALL {
			progress.waiting(&format!("opening {}", section.file()));
			let load = registries::load(root, section, &mut keys.cache, true);
			keys.save();
			match load {
				Load::Found { registry, .. } => others.opened.push((section, registry)),
				other => notes.push(format!("{} did not open ({}): {}", section.file(), why(&other), lost(section))),
			}
		}
		progress.waiting(&format!("opening {}", registries::MUX_FILE));
		let load = registries::load_mux(root, &mut keys.cache, true);
		keys.save();
		match load {
			Load::Found { registry, .. } => others.mux = Some(registry),
			other => notes.push(format!(
				"{} did not open ({}): the cases of multiplexed rows are left out",
				registries::MUX_FILE,
				why(&other)
			)),
		}
		others
	}

	/// Read every unit's lists at once, on rayon's pool, in the order given: the
	/// rest of each unit's work after its `[MWB]` list, so it costs wall time only
	/// as the slowest unit does. `lists` is what each unit's measurement list came
	/// to: the other lists are read from the file it was found in, and its rows of
	/// type 6 bring their MUX cases; without one the unit's best-matching file is
	/// read. Each unit searches with its own copy of `cache` and hands `finished`
	/// only the keys it found, as [`vag_data_labels::registry::units_measurements`]
	/// does.
	pub(super) fn read_units(
		&self,
		root: &Path,
		units: &[(&str, &str, Lookup<'_>)],
		lists: &[UnitMeasurements],
		rm: &vag_data_labels::MeasurementRegistry,
		cache: &IvCache,
		finished: &(dyn Fn(IvCache) + Sync),
	) -> Vec<Unit> {
		use rayon::prelude::*;
		units
			.par_iter()
			.zip(lists.par_iter())
			.map(|(unit, found)| {
				let mut own = cache.clone();
				let unit = self.read_unit(root, *unit, &mut own, found, rm);
				finished(own.since(cache));
				unit
			})
			.collect()
	}

	/// One unit's lists: the registry rows each selects, not yet put to words.
	/// `(odx, version, lookup)` is the unit as [`super::read`] looked it up.
	fn read_unit(
		&self,
		root: &Path,
		(odx, version, lookup): (&str, &str, Lookup<'_>),
		cache: &mut IvCache,
		found: &UnitMeasurements,
		rm: &vag_data_labels::MeasurementRegistry,
	) -> Unit {
		let mut unit = Unit::default();
		// The file, and the redirect it came through: the name the unit's rows go by,
		// as its channels' do.
		let file = match found {
			UnitMeasurements::Found { file, via, .. } => UnitRod {
				how: vag_data_labels::label_files::OdxMatch::Exact,
				path: file.clone(),
				via: via.clone(),
			},
			// VCDS's own word that it has no data for the unit.
			UnitMeasurements::NoData { .. } => return unit,
			_ => match vag_data_labels::find_rod_by_odx_variant(root, odx, version, lookup)
				.ok()
				.and_then(|found| found.files.into_iter().next())
			{
				Some(file) => file,
				None => return unit,
			},
		};
		if let UnitMeasurements::Found { rows, .. } = found {
			unit.muxes.extend(
				rows
					.rows
					.iter()
					.filter_map(|&n| rm.row(n))
					.filter_map(|row| registries::mux_of_measurement(&row)),
			);
		}
		for (section, registry) in &self.opened {
			let listed = match list_in(&file, cache, true, section.tag(), registry.len()) {
				UnitMeasurements::Found { rows, .. } => rows.rows,
				UnitMeasurements::Locked { .. } | UnitMeasurements::Shifted { .. } | UnitMeasurements::Mismatched { .. } => {
					unit.shut += 1;
					continue;
				}
				_ => continue,
			};
			let mut entries = Vec::new();
			for n in listed {
				let Some(row) = registry.row(n) else { continue };
				match registries::read_entry(*section, n, &row) {
					Ok(entry) => {
						if entry.value.kind == Some(registries::MULTIPLEXED)
							&& let Some(key) = entry.value.table
						{
							unit.muxes.push(key);
						}
						entries.push(entry);
					}
					// RX opens each list with a one-field header row, and RF ends
					// with a one-field sentinel: neither is a row of the registry.
					Err(registries::RowError::Shape { fields: 1 }) => {}
					Err(_) => unit.unread += 1,
				}
			}
			unit.sections.push((*section, entries));
		}
		unit.stem = Some(file.name().to_string()).filter(|name| !name.is_empty());
		unit
	}

	/// Take one unit's rows, in the order the units are said: put to words, with
	/// their MUX cases, under the unit's file stem. Two units that come to one file
	/// are written once.
	pub(super) fn absorb(&mut self, unit: Unit, words: &Words, units: &UnitStrings, tables: &TextTables) {
		self.unread += unit.unread;
		self.shut += unit.shut;
		let Some(stem) = unit.stem else { return };
		for (section, entries) in unit.sections {
			if self.rows.get(&stem).is_some_and(|rows| rows.iter().any(|d| d.section == section.tag())) {
				continue;
			}
			let described = entries
				.into_iter()
				.map(|entry| registries::describe(section.tag(), entry, section.key_is_a_record(), words, units, tables));
			self.rows.entry(stem.clone()).or_default().extend(described);
		}
		let Some(mux) = &self.mux else { return };
		for key in unit.muxes {
			if !self.muxed.insert((stem.clone(), key)) {
				continue;
			}
			let (cases, unread) = mux.cases(key);
			self.unread += unread;
			let rows = self.rows.entry(stem.clone()).or_default();
			rows.extend(cases.into_iter().map(|mut case| {
				case.identifier = Some(key);
				registries::describe(registries::MUX_SECTION, case, false, words, units, tables)
			}));
		}
	}
}

fn why<T>(load: &Load<T>) -> &'static str {
	match load {
		Load::NoFile => "there is none in this installation",
		Load::Locked { .. } => "the search for its key found nothing",
		Load::Shifted { .. } => super::OPENS_ONLY_IN_VCDS,
		Load::Found { .. } => "",
	}
}

fn lost(section: Section) -> &'static str {
	match section {
		Section::Adaptation => "no adaptation rows",
		Section::BasicSetting => "no basic-setting names",
		Section::OutputTest => "no output-test names",
		Section::FreezeFrame => "no freeze-frame fields",
		Section::Xpl => "no XPL rows",
	}
}
