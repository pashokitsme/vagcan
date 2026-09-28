//! Step 5: the car's channels, out of the VCDS install's measurement registry.
//!
//! A unit's `.rod` lists 1-based rows of the global registry `RM.rod`, and each
//! row carries the DID, the bit layout and the scaling
//! (`research/vcds-registry/README.md`; the reader is
//! [`vag_data_labels::registry`]). Everything here is read **from the install
//! being set up**, not from the shared pool the other steps copy into: a unit's
//! list and the registry it points into mean something only together, and the
//! pool can hold files from two builds under the same names.
//!
//! Which units: the ones this machine's cars answered when surveyed
//! (`~/.vagcan/cars/<VIN>/survey.jsonl`) — the one record, offline, of which
//! control units a car has and what `F19E`/`F1A2` each gave.
//!
//! What the step could not read, it says: a table that did not open, a row that
//! did not read, a list that lost its first row. A number on its own reads as
//! everything there was.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::Result;
use vag_data_labels::codes::CodePage;
use vag_data_labels::registry::{self, RegistryLoad, RowError, UnitMeasurements, Words};
use vag_data_labels::rod::{IvCache, KeyCost, RodStatus, decode_rod_recover, key_cost, recover_classic_key};
use vag_data_labels::ttdop::{self, TablesLoad};
use vag_data_labels::unit_strings::{self, UnitsLoad};

use super::{ODX_DIR, Step, TEXT_TABLES, text_page};
use crate::progress::Spinner;

const WHAT: &str = "channels from the VCDS registry";

/// The `(F19E, F1A2)` of every unit of every car this machine has surveyed,
/// each once, in order.
pub(super) fn surveyed_units() -> Result<BTreeSet<(String, String)>> {
	let cars = crate::datadir::vagcan_dir()?.join("cars");
	let mut out = BTreeSet::new();
	let Ok(entries) = std::fs::read_dir(&cars) else { return Ok(out) };
	for car in entries.flatten() {
		let Ok(survey) = std::fs::read_to_string(car.path().join(crate::datadir::SURVEY_FILE)) else {
			continue;
		};
		for unit in crate::plan::identities_from_survey(&survey) {
			if let Some(odx) = unit.odx_name {
				out.insert((odx, unit.odx_version.unwrap_or_default()));
			}
		}
	}
	Ok(out)
}

/// The key cache, saved after every search so an interrupted run keeps what it
/// found, and a save that failed said once rather than dropped.
struct Keys<'a> {
	path: &'a Path,
	cache: IvCache,
	held: usize,
	unsaved: Option<String>,
}

impl Keys<'_> {
	fn save(&mut self) {
		if let Err(e) = self.cache.save(self.path)
			&& self.unsaved.is_none()
		{
			self.unsaved = Some(format!(
				"the keys found were not saved: {e}.\n    Move that file aside and run setup again, or every run searches for them again"
			));
		}
	}

	/// How many keys this run found.
	fn found(&self) -> usize {
		self.cache.len().saturating_sub(self.held)
	}
}

/// Where an install's registry rows are written: under the source its label
/// files were written under, so an install is one source with one language,
/// whichever of its files a row came from.
fn source_of(root: &Path) -> String {
	crate::labels::label_dir_under(root)
		.unwrap_or_else(|_| root.to_path_buf())
		.to_string_lossy()
		.into_owned()
}

/// Leave no registry rows behind when this installation gives none: every row
/// comes from the last installation read, as the label files do, and rows from
/// another one beside this one's label files would be two builds in one project.
///
/// Says in words what it removed, for the step's reason: channels an earlier
/// run read disappearing without a word is how a working dash stops building.
fn clear(root: &Path, project: &crate::project::Project) -> Result<String> {
	let held = vag_data_db::reading_count_of(&project.cache(), vag_data_db::VCDS).unwrap_or(0);
	vag_data_db::put_all_vcds_readings(&project.cache(), &source_of(root), std::iter::empty())?;
	Ok(match held {
		0 => String::new(),
		n => format!("\n    This run removed the {n} channels an earlier run read from a VCDS registry."),
	})
}

/// Read the registry for `units` out of the install at `root` and write their
/// channels into the project's cache.
pub(super) fn registry_channels(root: &Path, project: &crate::project::Project, units: &BTreeSet<(String, String)>) -> Result<Step> {
	if units.is_empty() {
		println!("[5/5] Channels — no car has been surveyed on this machine, so there are no units to read.");
		let removed = clear(root, project)?;
		return Ok(Step::Pending {
			what: WHAT,
			why: format!(
				"no car has been surveyed on this machine, so there is no list of its units.\n    \
                 Survey the parked car with a cable adapter — it reads every unit, a few\n    \
                 minutes — then run setup again with this installation still in place:\n      \
                 vagcan dev survey\n      \
                 vagcan setup {}\n    \
                 The units are read from {}.{removed}",
				root.display(),
				crate::datadir::vagcan_dir()?
					.join("cars")
					.join("<VIN>")
					.join(crate::datadir::SURVEY_FILE)
					.display()
			),
		});
	}
	println!(
		"[5/5] Channels — reading the VCDS registry for {} unit{}; a key not cached yet is searched for, minutes, once.",
		units.len(),
		plural(units.len())
	);
	let path = project.rod_keys();
	let cache = IvCache::load(&path);
	let mut keys = Keys {
		path: &path,
		held: cache.len(),
		cache,
		unsaved: None,
	};
	let mut notes = Vec::new();

	let registry = {
		let _spinner = Spinner::new("opening RM.rod");
		registry::load_registry(root, &mut keys.cache, true)
	};
	keys.save();
	let registry = match registry {
		RegistryLoad::Found { registry, .. } => registry,
		other => {
			let removed = clear(root, project)?;
			return Ok(Step::Missing {
				what: WHAT,
				why: format!("the registry did not open: {}{removed}", registry_why(&other)),
			});
		}
	};
	let tables = {
		let _spinner = Spinner::new("opening TTDOP.rod");
		ttdop::load_text_tables(root, &mut keys.cache, true)
	};
	let tables = match tables {
		TablesLoad::Found { tables, .. } => {
			// A row charged to no table could be any listed row's; one charged to
			// a table is said below only where a listed row names that table.
			if tables.unattributed() > 0 {
				notes.push(format!(
					"{} rows of TTDOP.rod name no table: the states they hold are left out",
					tables.unattributed()
				));
			}
			tables
		}
		other => {
			notes.push(format!(
				"TTDOP.rod did not open ({}): the rows whose states it names are left out",
				tables_why(&other)
			));
			ttdop::TextTables::default()
		}
	};
	let (words, page) = {
		let _spinner = Spinner::new("opening the text table");
		words(root, &mut keys.cache, &mut notes)
	};
	let units_of = {
		let _spinner = Spinner::new("opening UNIT.ROD");
		unit_strings::load_unit_strings(root, &mut keys.cache, true, page)
	};
	let units_of = match units_of {
		UnitsLoad::Found { units, .. } => units,
		other => {
			notes.push(format!("UNIT.ROD did not open ({}): no channel carries a unit", units_why(&other)));
			unit_strings::UnitStrings::default()
		}
	};
	keys.save();

	// A unit the project's ODIS rows describe is not a gap when this
	// installation has nothing for it: its channels are there already.
	let extracted = crate::extracted::open(project);
	let mut variants: BTreeMap<String, Vec<vag_data_labels::odis::Reading>> = BTreeMap::new();
	let (mut read, mut not_channels, mut unread, mut dropped, mut lossy) = (0usize, 0usize, 0usize, 0usize, 0usize);
	for (odx, version) in units {
		let found = {
			let _spinner = Spinner::new(format!("{odx}: finding its measurement list"));
			registry::unit_measurements(root, odx, version, &mut keys.cache, true, &registry)
		};
		keys.save();
		let UnitMeasurements::Found { file, rows, .. } = found else {
			if !extracted.for_unit_from(vag_data_db::ODIS, Some(odx), Some(version)).is_empty() {
				println!("      {odx}: {}; the ODIS project describes it", unit_gap(&found));
				continue;
			}
			let why = format!("{odx}: {}", unit_why(&found));
			println!("      {why}");
			notes.push(why);
			continue;
		};
		read += 1;
		let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or(odx).to_string();
		// Two cars can carry one unit at two versions that come to the same
		// file. Its rows are written once.
		if variants.contains_key(&stem) {
			println!("      {odx} {version}: the same list as above");
			continue;
		}
		let mut channels = Vec::new();
		for row in rows.rows.iter().filter_map(|&n| registry.row(n)) {
			let row = match registry::read_row(&row) {
				Ok(row) => row,
				Err(RowError::Undecoded(_)) => {
					not_channels += 1;
					continue;
				}
				Err(_) => {
					unread += 1;
					continue;
				}
			};
			if let registry::Kind::Enum { table } = row.kind
				&& tables.lost_states(table)
			{
				lossy += 1;
			}
			match registry::to_reading(&row, &words, &units_of, &tables) {
				Ok(reading) => channels.push(reading),
				Err(_) => not_channels += 1,
			}
		}
		dropped += usize::from(rows.first_row_dropped);
		// Said as each unit is done: the searches can take minutes, and a run that
		// prints nothing for that long reads as hung.
		println!("      {odx}: {} channels", channels.len());
		variants.insert(stem, channels);
	}
	if lossy > 0 {
		notes.push(format!(
			"{lossy} listed row{} name{} a text table with a row that did not read: one of its states is left out",
			plural(lossy),
			if lossy == 1 { "s" } else { "" }
		));
	}
	if dropped > 0 {
		notes.push(format!(
			"{dropped} list{} lost {} first row: in a plain list a key this tool cannot know spoils the\n    \
             first row's number, and nothing else in the list says which row it was",
			plural(dropped),
			if dropped == 1 { "its" } else { "their" }
		));
	}
	if let Some(unsaved) = &keys.unsaved {
		notes.push(format!("{unsaved} ({})", keys.path.display()));
	}

	let batch: Vec<(&str, &[vag_data_labels::odis::Reading])> = variants.iter().map(|(v, r)| (v.as_str(), r.as_slice())).collect();
	let written = vag_data_db::put_all_vcds_readings(&project.cache(), &source_of(root), batch)?;
	let mut detail = format!("{written} channels for {read} of {} unit{}", units.len(), plural(units.len()));
	if not_channels > 0 {
		detail.push_str(&format!(
			"; {not_channels} listed rows are not channels (raw bytes, text, a type or OBD formula not decoded, a text table without words)"
		));
	}
	if unread > 0 {
		detail.push_str(&format!("; {unread} listed rows did not read"));
	}
	if keys.found() > 0 {
		let kept = match keys.unsaved {
			None => "cached",
			Some(_) => "not saved",
		};
		detail.push_str(&format!("; {} key{} found, {kept}", keys.found(), plural(keys.found())));
	}
	Ok(match notes.is_empty() {
		true => Step::Wrote {
			what: WHAT,
			path: project.cache(),
			detail,
		},
		false => Step::Partial {
			what: WHAT,
			path: project.cache(),
			detail,
			why: notes.join("\n    "),
		},
	})
}

fn plural(n: usize) -> &'static str {
	if n == 1 { "" } else { "s" }
}

/// `TTTEXT`'s records by id, and the code page they are in. Empty when the
/// install's text table does not open — the Russian build's is shifted — and
/// then every channel is named by its identifier, with no ODX id, which is
/// said in `notes`.
fn words(root: &Path, cache: &mut IvCache, notes: &mut Vec<String>) -> (Words, CodePage) {
	let unnamed = "channels are named by their identifier, with no ODX id";
	for name in TEXT_TABLES {
		let file = root.join(ODX_DIR).join(name);
		let Ok(bytes) = std::fs::read(&file) else { continue };
		let page = text_page(name);
		recover_classic_key(&bytes, name, "TXT", cache);
		let text = decode_rod_recover(&bytes, name, cache, false)
			.into_iter()
			.find(|s| s.tag == "TXT" && matches!(s.status, RodStatus::Tea | RodStatus::Zlib))
			.and_then(|s| s.text);
		if let Some(text) = text {
			// One byte to one char out of the container; the code page is
			// applied to the bytes, as the names step does.
			let raw: Vec<u8> = text.chars().map(|c| c as u32 as u8).collect();
			return (Words::from_table(&vag_data_labels::tttext::read(&raw, page)), page);
		}
		let why = match key_cost(&bytes, "TXT") {
			Some(KeyCost::AnchorSweep) => "it is shifted, which only VCDS's runtime key opens",
			_ => "the search for its key found nothing",
		};
		notes.push(format!("{name} did not open ({why}): {unnamed}"));
		return (Words::default(), page);
	}
	notes.push(format!("none of {TEXT_TABLES:?} is in {}: {unnamed}", root.join(ODX_DIR).display()));
	(Words::default(), CodePage::Windows1252)
}

fn registry_why(load: &RegistryLoad) -> String {
	match load {
		RegistryLoad::NoFile => "there is no RM.rod in this installation".to_string(),
		RegistryLoad::Locked { file } => format!("the search for {}'s key found nothing", file.display()),
		RegistryLoad::Shifted { file } => format!("{} is a shifted container, which only VCDS's own runtime key opens", file.display()),
		RegistryLoad::Found { .. } => String::new(),
	}
}

fn tables_why(load: &TablesLoad) -> String {
	match load {
		TablesLoad::NoFile => "there is none in this installation".to_string(),
		TablesLoad::Locked { .. } => "the search for its key found nothing".to_string(),
		TablesLoad::Shifted { .. } => "it is shifted, which only VCDS's runtime key opens".to_string(),
		TablesLoad::Found { .. } => String::new(),
	}
}

fn units_why(load: &UnitsLoad) -> String {
	match load {
		UnitsLoad::NoFile => "there is none in this installation".to_string(),
		UnitsLoad::Locked { .. } => "the search for its key found nothing".to_string(),
		UnitsLoad::Shifted { .. } => "it is shifted, which only VCDS's runtime key opens".to_string(),
		UnitsLoad::Found { .. } => String::new(),
	}
}

/// What this installation lacks for a unit, without the advice — for a unit
/// the ODIS project already describes, where there is nothing to do.
fn unit_gap(outcome: &UnitMeasurements) -> &'static str {
	match outcome {
		UnitMeasurements::Found { .. } => "",
		UnitMeasurements::NoFile => "no file of that name in this installation",
		UnitMeasurements::NoList { .. } => "none of its files lists measurements",
		UnitMeasurements::Locked { .. } => "the search for its list's key found nothing",
		UnitMeasurements::Shifted { .. } => "its list is shifted, which only VCDS's runtime key opens",
		UnitMeasurements::Unresolved { .. } => "it includes a store this installation does not have",
		UnitMeasurements::Mismatched { .. } => "its list is another install's file",
	}
}

/// Why a unit brought no channels, in a line.
fn unit_why(outcome: &UnitMeasurements) -> String {
	match outcome {
		UnitMeasurements::Found { .. } => String::new(),
		UnitMeasurements::NoFile => "no file of that name in this installation; an ODIS project that describes the unit brings its channels".to_string(),
		UnitMeasurements::NoList { candidates } => format!("none of its {candidates} files lists measurements"),
		UnitMeasurements::Locked { file, section } => {
			format!("the search for the key of [{section}] in {} found nothing", file.display())
		}
		UnitMeasurements::Shifted { file, section } => format!(
			"[{section}] in {} is shifted — only VCDS's runtime key opens it; an ODIS project has this unit",
			file.display()
		),
		UnitMeasurements::Unresolved { file } => format!("{} includes a store this installation does not have", file.display()),
		UnitMeasurements::Mismatched { file, listed, outside } => {
			format!(
				"{} lists {outside} of {listed} rows the registry does not have — another install's file",
				file.display()
			)
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_reason_a_unit_brings_no_channels_is_said() {
		let file = std::path::PathBuf::from("EV_X.rod");
		for outcome in [
			UnitMeasurements::NoFile,
			UnitMeasurements::NoList { candidates: 2 },
			UnitMeasurements::Locked {
				file: file.clone(),
				section: "MWB".into(),
			},
			UnitMeasurements::Shifted {
				file: file.clone(),
				section: "INC".into(),
			},
			UnitMeasurements::Unresolved { file: file.clone() },
			UnitMeasurements::Mismatched {
				file: file.clone(),
				listed: 9,
				outside: 3,
			},
		] {
			assert!(!unit_why(&outcome).is_empty(), "{outcome:?}");
			assert!(!unit_gap(&outcome).is_empty(), "{outcome:?}");
		}
		assert!(unit_why(&UnitMeasurements::Shifted { file, section: "INC".into() }).contains("ODIS"));
		assert!(unit_why(&UnitMeasurements::NoFile).contains("ODIS"), "and what covers it instead");
	}

	#[test]
	fn every_table_that_does_not_open_is_said_with_why() {
		let file = std::path::PathBuf::from("X.rod");
		for load in [
			TablesLoad::NoFile,
			TablesLoad::Locked { file: file.clone() },
			TablesLoad::Shifted { file: file.clone() },
		] {
			assert!(!tables_why(&load).is_empty(), "{load:?}");
		}
		for load in [UnitsLoad::NoFile, UnitsLoad::Locked { file: file.clone() }, UnitsLoad::Shifted { file }] {
			assert!(!units_why(&load).is_empty(), "{load:?}");
		}
	}
}
