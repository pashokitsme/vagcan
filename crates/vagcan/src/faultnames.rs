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
use vag_data::codes::sae_code;
use vag_data::dtc::{self, DtcRegistry, UnitCatalogue, UnitLookup};
use vag_data::rod::IvCache;
use vag_data::CodesDb;

/// `Codes.dat`'s own file name inside a VCDS installation.
const CODES_FILE: &str = "Codes.dat";

/// What one fault resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Naming {
    /// VW's own text, and the code VCDS prints beside it.
    Named { text: String, sae: Option<String>, failure_type: Option<u8> },
    /// The unit's catalogue does not list this code. Its file is from a
    /// different vintage than the corpus's `RD.rod`, or than the unit's own
    /// software.
    NotListed,
    /// The registry row is there and does not have a row's shape.
    Unreadable,
    /// The row named a `Codes.dat` key this `Codes.dat` does not have — 1.0 %
    /// of the registry's rows, a corpus/text-file vintage mismatch.
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
            Self::NoText { codes_key } => {
                Some(format!("named {codes_key}, which this Codes.dat does not carry"))
            }
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
    /// and `Codes.dat` are searched for by name, since a corpus copied out of
    /// an installation keeps the layout but not always the root.
    pub fn open(root: &Path, iv_cache: &Path) -> Result<Self> {
        // No keys at all is not "the registry did not decode" — it is that
        // nobody has run `vagcan setup` on this machine, and the reader needs
        // that named rather than a decode failure they cannot act on.
        if !iv_cache.is_file() {
            anyhow::bail!(crate::missing::no_label_data(
                "The .rod section keys",
                "naming faults",
                iv_cache
            ));
        }
        let cache = IvCache::load(iv_cache);
        let (_, registry) = dtc::load_registry(root, &cache).with_context(|| {
            format!(
                "no readable fault registry under {}\n\n\
                 The chain needs UDS_EV/{} and the key for its [DTC] section. \
                 The key lives in {} — if the file is there and the key is not, \
                 recover it with:\n    \
                 cargo run --release -p vagcan --features rod-crack -- vcds rod <path to {}>",
                root.display(),
                dtc::REGISTRY_FILE,
                iv_cache.display(),
                dtc::REGISTRY_FILE,
            )
        })?;
        let codes_path = dtc::find_named(root, CODES_FILE).with_context(|| {
            format!("no {CODES_FILE} under {} — it ships in the VCDS install root, beside Labels/", root.display())
        })?;
        let codes = CodesDb::parse(
            &std::fs::read(&codes_path).with_context(|| format!("reading {}", codes_path.display()))?,
        );
        Ok(Self { root: root.to_path_buf(), cache, registry, codes, units: BTreeMap::new() })
    }

    /// How many rows the registry has — the one number that says the corpus
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
        self.units
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
        UnitLookup::NoFile => {
            Some("the corpus has no ODX file of the name this unit gives — codes only".to_string())
        }
        UnitLookup::NoSection { candidates } => Some(format!(
            "none of the {candidates} ODX files of this family carries a fault catalogue — codes only"
        )),
        UnitLookup::Locked { file } => Some(format!(
            "the fault catalogue in {} is still sealed — recover its key with \
             `vagcan vcds rod --features rod-crack` — codes only",
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
        assert_eq!(
            naming.line().unwrap(),
            "B1455 01  Temperature Sensor for Heated Steering Wheel"
        );
    }

    #[test]
    fn every_break_in_the_chain_says_which_break_it_was() {
        // The rule this module exists for: a fault that cannot be named comes
        // back as a reason, never as a plausible name.
        for naming in [
            Naming::NotListed,
            Naming::Unreadable,
            Naming::NoText { codes_key: 140_975 },
        ] {
            let line = naming.line().expect("a break is still something to say");
            assert!(!line.is_empty());
        }
        assert!(Naming::NoText { codes_key: 140_975 }.line().unwrap().contains("140975"));
    }

    #[test]
    fn a_units_missing_catalogue_is_explained_rather_than_hidden() {
        // On the reference car the body control module's F19E is EV_BCMMQB and
        // no file in the English corpus starts with that name.
        assert!(unit_note(&UnitLookup::NoFile).unwrap().contains("no ODX file"));
        assert!(unit_note(&UnitLookup::NoSection { candidates: 14 }).unwrap().contains("14"));
        assert!(unit_note(&UnitLookup::Locked { file: PathBuf::from("EV_X.rod") })
            .unwrap()
            .contains("EV_X.rod"));
    }
}
