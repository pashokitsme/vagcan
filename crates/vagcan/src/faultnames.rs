//! Turning a fault number into VW's own words for it.
//!
//! The chain and its evidence are `research/labels/fault-naming-hop.md`; the decoders
//! are [`vag_data::dtc`] and [`vag_data::CodesDb`]. This module is what holds
//! the three files open at once and answers one question per fault:
//!
//! ```text
//! the unit's F19E/F1A2 ──▶ its own .rod [DTC]  ──▶ a row of UDS_EV/RD.rod
//! the fault number     ──▶ that row's table    ──┘
//!                                              ──▶ a Codes.dat key ──▶ the text
//! ```
//!
//! It exists in the CLI rather than in `vag-data` because it is policy: which
//! files to open, in what order, and — the part that matters — **what to say
//! when the chain breaks**. Every break has its own answer here, because
//! "047120" with a reason beside it is a result and "047120 — Fuel Pump" is a
//! wrong one. `research/labels/fault-naming-hop.md` has stayed at zero wrong answers
//! through five passes and this module is where that is kept.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use vag_data::CodesDb;
use vag_data::codes::sae_code;
use vag_data::dtc::{self, DtcRegistry, UnitCatalogue, UnitLookup};
use vag_data::rod::IvCache;

/// The fault text store's own file name inside a VCDS installation.
///
/// One name per language build — see [`crate::setup::CODES_FILES`], which owns
/// the list. Looked up in that order, so an install carrying both is read the
/// same way twice running.
fn find_codes(root: &Path) -> Option<std::path::PathBuf> {
	crate::setup::CODES_FILES.iter().find_map(|name| dtc::find_named(root, name))
}

/// Whether a directory holds the two files fault naming cannot start without —
/// the registry and the text store.
///
/// Cheap: two breadth-first name searches, no decode, resolved exactly the way
/// [`Namer::open`] will. It tells the ordinary "`vagcan setup` has not copied
/// the labels in yet" (show the codes as numbers, point at setup) apart from a
/// dir the user named that is genuinely broken (a real error worth surfacing) —
/// so the default `~/.vagcan` path can degrade quietly while an explicit
/// `--labels` still reports what it could not open.
pub fn has_fault_labels(root: &Path) -> bool {
	dtc::find_named(root, dtc::REGISTRY_FILE).is_some() && find_codes(root).is_some()
}

/// What one fault resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Naming {
	/// VW's own text, and the code VCDS prints beside it.
	Named {
		text: String,
		sae: Option<String>,
		failure_type: Option<u8>,
	},
	/// The unit's catalogue does not list this code. Its file is from a
	/// different vintage than the label files' `RD.rod`, or than the unit's own
	/// software.
	NotListed,
	/// The registry row is there and does not have a row's shape.
	Unreadable,
	/// The row named a `Codes.dat` key this `Codes.dat` does not have — 1.0 %
	/// of the registry's rows, label files/text-file vintage mismatch.
	NoText { codes_key: u32 },
}

impl Naming {
	/// One line for the console, or `None` when there is nothing to add to the
	/// number itself.
	pub fn line(&self) -> Option<String> {
		match self {
			Self::Named { text, sae, failure_type } => {
				let code = match (sae, failure_type) {
					(Some(sae), Some(ftb)) => format!("{sae} {ftb:02X}  "),
					(Some(sae), None) => format!("{sae}  "),
					_ => String::new(),
				};
				Some(format!("{code}{text}"))
			}
			Self::NotListed => Some("not in this unit's fault catalogue".to_string()),
			Self::Unreadable => Some("the registry row for it is malformed".to_string()),
			Self::NoText { codes_key } => Some(format!("named {codes_key}, which this Codes.dat does not carry")),
		}
	}
}

/// The three files the chain needs, held open.
pub struct Namer {
	root: PathBuf,
	cache: IvCache,
	registry: DtcRegistry,
	codes: CodesDb,
	/// Per-unit catalogues, memoised — a survey re-reads the same unit's file
	/// once per fault otherwise, and the file is a decrypt and an inflate.
	units: BTreeMap<(String, String), UnitLookup>,
}

impl Namer {
	/// Open a VCDS installation for naming.
	///
	/// `root` is the install root or any directory above the files: `RD.rod`
	/// and `Codes.dat` are searched for by name, since label files copied out of
	/// an installation keeps the layout but not always the root.
	pub fn open(root: &Path, iv_cache: &Path) -> Result<Self> {
		// No keys at all is not "the registry did not decode" — it is that
		// nobody has run `vagcan setup` on this machine, and the reader needs
		// that named rather than a decode failure they cannot act on.
		if !iv_cache.is_file() {
			anyhow::bail!(crate::missing::no_label_data("The .rod section keys", "naming faults", iv_cache));
		}
		let cache = IvCache::load(iv_cache);
		let (_, registry) = dtc::load_registry(root, &cache).with_context(|| {
			format!(
				"no readable fault registry under {}\n\n\
                 The chain needs UDS_EV/{} and the key for its [DTC] section. \
                 The key lives in {} — if the file is there and the key is not, \
                 recover it with:\n    \
                 cargo run --release -p vagcan -- vcds rod <path to {}>",
				root.display(),
				dtc::REGISTRY_FILE,
				iv_cache.display(),
				dtc::REGISTRY_FILE,
			)
		})?;
		let codes_path = find_codes(root).with_context(|| {
			format!(
				"none of {:?} under {} — the fault text ships in the VCDS install root, beside Labels/",
				crate::setup::CODES_FILES,
				root.display()
			)
		})?;
		let codes = CodesDb::parse(&std::fs::read(&codes_path).with_context(|| format!("reading {}", codes_path.display()))?);
		Ok(Self {
			root: root.to_path_buf(),
			cache,
			registry,
			codes,
			units: BTreeMap::new(),
		})
	}

	/// How many rows the registry has — the one number that says the label files
	/// opened.
	pub fn registry_rows(&self) -> usize {
		self.registry.len()
	}

	pub fn codes_texts(&self) -> usize {
		self.codes.len()
	}

	/// The catalogue for a unit that named its own ODX file.
	///
	/// Cloned rather than borrowed: opening the catalogue needs `&mut self`
	/// for the memo, naming a fault needs `&self`, and a caller holding a
	/// borrow of the first cannot do the second. A catalogue is a few hundred
	/// integers and the file decode behind it is what the memo saves.
	pub fn unit(&mut self, odx_name: &str, version: &str) -> UnitLookup {
		let key = (odx_name.to_string(), version.to_string());
		let (root, cache, registry) = (&self.root, &self.cache, &self.registry);
		self
			.units
			.entry(key)
			.or_insert_with(|| dtc::unit_catalogue(root, odx_name, version, cache, registry))
			.clone()
	}

	/// Name one code against a catalogue already obtained from [`Namer::unit`].
	pub fn name(&self, catalogue: &UnitCatalogue, code: [u8; 3]) -> Naming {
		let raw = u32::from_be_bytes([0, code[0], code[1], code[2]]);
		let Some(index) = catalogue.row_of(raw) else { return Naming::NotListed };
		let Some(row) = self.registry.row(index) else { return Naming::NotListed };
		// The catalogue is keyed by the row's own table key, so this holds by
		// construction — asserted rather than assumed because if it ever did
		// not, the answer would be another fault's name.
		debug_assert_eq!(row.key, raw);
		let Some(name) = dtc::read_row(&row) else { return Naming::Unreadable };
		match self.codes.get(name.codes_key) {
			Some(text) => Naming::Named {
				text: text.to_string(),
				sae: sae_code(name.codes_key),
				failure_type: name.failure_type,
			},
			None => Naming::NoText { codes_key: name.codes_key },
		}
	}
}

/// Why a unit's faults are being shown as numbers, in one line for the console.
pub fn unit_note(lookup: &UnitLookup) -> Option<String> {
	match lookup {
		UnitLookup::Found { .. } => None,
		UnitLookup::NoFile => Some("the label files have no ODX file of the name this unit gives — codes only".to_string()),
		UnitLookup::NoSection { candidates } => Some(format!(
			"none of the {candidates} ODX files of this family carries a fault catalogue — codes only"
		)),
		UnitLookup::Locked { file } => Some(format!(
			"the fault catalogue in {} is still sealed — recover its key with \
             `vagcan vcds rod` — codes only",
			file.file_name().and_then(|n| n.to_str()).unwrap_or_default()
		)),
		UnitLookup::Mismatched { file, listed, distinct } => Some(format!(
			"{} lists {listed} faults that land on only {distinct} registry rows, \
             so it is not the same vintage as this RD.rod — refused, codes only",
			file.file_name().and_then(|n| n.to_str()).unwrap_or_default()
		)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_named_fault_reads_as_vcds_prints_it() {
		// 047120 on this car's steering column: VCDS printed
		// "291104 - Датчик температуры подогрева рулевого колеса / B1455 01".
		let naming = Naming::Named {
			text: "Temperature Sensor for Heated Steering Wheel".to_string(),
			sae: Some("B1455".to_string()),
			failure_type: Some(0x01),
		};
		assert_eq!(naming.line().unwrap(), "B1455 01  Temperature Sensor for Heated Steering Wheel");
	}

	#[test]
	fn every_break_in_the_chain_says_which_break_it_was() {
		// The rule this module exists for: a fault that cannot be named comes
		// back as a reason, never as a plausible name.
		for naming in [Naming::NotListed, Naming::Unreadable, Naming::NoText { codes_key: 140_975 }] {
			let line = naming.line().expect("a break is still something to say");
			assert!(!line.is_empty());
		}
		assert!(Naming::NoText { codes_key: 140_975 }.line().unwrap().contains("140975"));
	}

	#[test]
	fn fault_labels_are_present_only_once_both_files_are_there() {
		// The gate that decides whether `faults` names or degrades: the registry
		// and the text store, resolved by name the way `Namer::open` does. One
		// of the two missing is still "not ready" — a registry with no texts
		// names nothing.
		let base = std::env::temp_dir().join(format!("vagcan-faultnames-{}-{:?}", std::process::id(), std::thread::current().id()));
		let _ = std::fs::remove_dir_all(&base);
		let odx = base.join("UDS_EV");
		std::fs::create_dir_all(&odx).unwrap();
		assert!(!has_fault_labels(&base), "empty dir has no labels");
		std::fs::write(odx.join(dtc::REGISTRY_FILE), b"x").unwrap();
		assert!(!has_fault_labels(&base), "registry alone is not enough");
		std::fs::write(base.join(crate::setup::CODES_FILES[0]), b"x").unwrap();
		assert!(has_fault_labels(&base), "both files present — found by name, at any depth");
		let _ = std::fs::remove_dir_all(&base);
	}

	#[test]
	fn a_units_missing_catalogue_is_explained_rather_than_hidden() {
		// On the reference car the body control module's F19E is EV_BCMMQB and
		// no file in the English label files start with that name.
		assert!(unit_note(&UnitLookup::NoFile).unwrap().contains("no ODX file"));
		assert!(unit_note(&UnitLookup::NoSection { candidates: 14 }).unwrap().contains("14"));
		assert!(
			unit_note(&UnitLookup::Locked {
				file: PathBuf::from("EV_X.rod")
			})
			.unwrap()
			.contains("EV_X.rod")
		);
	}
}
