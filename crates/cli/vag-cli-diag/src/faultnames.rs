//! Turning a fault number into VW's own words for it.
//!
//! The chain and its evidence are `.archive/research/labels/fault-naming-hop.md`; the decoders
//! are [`vag_data_labels::dtc`] and [`vag_data_labels::CodesDb`]. This module is what holds
//! the three files open at once and answers one question per fault:
//!
//! ```text
//! the unit's F19E/F1A2 ──▶ its own .rod [DTC]  ──▶ a row of UDS_EV/RD.rod
//! the fault number     ──▶ that row's table    ──┘
//!                                              ──▶ a Codes.dat key ──▶ the text
//! ```
//!
//! It exists in the CLI rather than in `vag-data-labels` because it is policy: which
//! files to open, in what order, and — the part that matters — **what to say
//! when the chain breaks**. Every break has its own answer here, because
//! "047120" with a reason beside it is a result and "047120 — Fuel Pump" is a
//! wrong one. `.archive/research/labels/fault-naming-hop.md` has stayed at zero wrong answers
//! through five passes and this module is where that is kept.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use vag_data_labels::CodesDb;
use vag_data_labels::codes::sae_code;
use vag_data_labels::dtc::{self, DtcRegistry, UnitCatalogue, UnitLookup};
use vag_data_labels::label_files::{Lookup, redirect_line};
use vag_data_labels::redirect::{self, RedirectLoad, Redirects};
use vag_data_labels::rod::IvCache;

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
/// an installation the reader named still reports what it could not open.
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
	/// The installation's `ReDir.rod`, opened once: empty when it has none or it did not open,
	/// and every unit is then looked up by its own name alone.
	redirects: Redirects,
	/// Why `ReDir.rod` did not open, for the lines above a listing; `None` when it opened
	/// or there is none.
	redirect_note: Option<String>,
	/// The project codes of the cars recorded on this machine, per unit
	/// ([`crate::registry::recorded_projects`]): `faults` reads no VIN of its own.
	projects: BTreeMap<(String, String), Vec<String>>,
	registry: DtcRegistry,
	codes: CodesDb,
	/// Per-unit catalogues, memoised — a fault read re-reads the same unit's file
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
                 vagcan dev vcds rod --cache {} <path to {}>",
				root.display(),
				dtc::REGISTRY_FILE,
				iv_cache.display(),
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
		// With the cached keys only, as every file here: the search is `setup`'s.
		let what = "units VCDS reads through another unit's file show codes only";
		let (redirects, redirect_note) = match redirect::load_redirects(root, &mut cache.clone(), false) {
			RedirectLoad::Found { redirects, .. } => (redirects, None),
			RedirectLoad::NoFile => (Redirects::default(), None),
			RedirectLoad::Locked { file } => (
				Redirects::default(),
				Some(format!("{} is still sealed: {what} — `vagcan setup` opens it", file.display())),
			),
			RedirectLoad::Shifted { file } => (
				Redirects::default(),
				Some(format!("{} opens only inside VCDS itself: {what}", file.display())),
			),
		};
		let projects = crate::project::current()
			.map(|project| crate::registry::recorded_projects(&project))
			.unwrap_or_default();
		Ok(Self {
			root: root.to_path_buf(),
			cache,
			redirects,
			redirect_note,
			projects,
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

	/// Why `ReDir.rod` did not open, when it did not.
	pub fn redirect_note(&self) -> Option<&str> {
		self.redirect_note.as_deref()
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
		// The project of the recorded cars that carry this unit: `faults` reads no VIN. Two
		// cars of two projects with one unit offer both projects' names first, in turn.
		let projects = self.projects.get(&(odx_name.to_string(), version.to_string()));
		let lookup = Lookup {
			redirects: &self.redirects,
			projects: projects.map_or(&[][..], Vec::as_slice),
		};
		self
			.units
			.entry(key)
			.or_insert_with(|| dtc::unit_catalogue(root, odx_name, version, lookup, cache, registry))
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

/// Why a unit's faults are not in the VCDS text, in one line for the console — or, for a
/// catalogue `ReDir.rod` chose, which file it came from.
///
/// `instead` is what the codes are shown with: `codes only`, or the ODIS
/// project's text where the project has the unit and VCDS was only asked first.
pub fn unit_note(lookup: &UnitLookup, instead: &str) -> Option<String> {
	match lookup {
		UnitLookup::Found {
			file, via, sealed_before, ..
		} => {
			let from = match via {
				Some(source) => redirect_line(source, file),
				None => file_name(file).to_string(),
			};
			match sealed_before {
				// Never another variant's text as this unit's without saying so.
				Some(sealed) => Some(format!(
					"the fault catalogue in {} is still sealed — VCDS text from {from}, another variant, in its place",
					file_name(sealed)
				)),
				None => via.as_ref().map(|_| format!("VCDS text from {from}")),
			}
		}
		UnitLookup::NoData { source } => Some(format!("VCDS has no data for this unit: ReDir.rod sends {source} to EMPTY — {instead}")),
		UnitLookup::NoFile => Some(format!("the label files have no ODX file of the name this unit gives — {instead}")),
		UnitLookup::NoSection { candidates } => Some(format!(
			"none of the {candidates} ODX files of this family carries a fault catalogue — {instead}"
		)),
		UnitLookup::Locked { file } => Some(format!(
			"the fault catalogue in {} is still sealed — {instead}",
			file.file_name().and_then(|n| n.to_str()).unwrap_or_default()
		)),
		UnitLookup::Mismatched { file, listed, distinct } => Some(format!(
			"{} lists {listed} faults that land on only {distinct} registry rows, \
             so it is not the same vintage as this RD.rod — refused, {instead}",
			file.file_name().and_then(|n| n.to_str()).unwrap_or_default()
		)),
	}
}

/// A path's file name, for a line that names a file.
fn file_name(path: &Path) -> &str {
	path.file_name().and_then(|n| n.to_str()).unwrap_or_default()
}

/// Offer to open the fault catalogues this car needs and nobody has opened yet.
///
/// VW encrypts each `.rod` section under a key that is not in the file, and
/// recovering one is a search: **about three minutes per file** on an M4,
/// measured rather than estimated. The answer is then cached for good, so the
/// price is one-off per car, not per run.
///
/// That is too long to spend without asking, and too useful to leave as a
/// command printed under each unit for somebody to copy out — a car with four
/// sealed catalogues printed it four times. So it is asked once, at the end,
/// for all of them together.
///
/// Never asks when stdin is not a terminal: a script gets the command it would
/// have to run, and no prompt nobody is there to answer.
pub fn offer_to_unseal(files: &[PathBuf], cache: &Path) -> Result<()> {
	if files.is_empty() {
		return Ok(());
	}
	let names: Vec<&str> = files.iter().map(|f| f.file_name().and_then(|f| f.to_str()).unwrap_or_default()).collect();
	// What is sealed and the command that opens it, on stderr: a hint, not data.
	vag_cli_core::say!(
		"sealed   {} — `vagcan dev vcds rod --cache {} <file>` (a search of minutes per file)",
		names.join(", "),
		cache.display()
	);
	vag_cli_core::note!("opening a sealed catalogue recovers the words for its codes: a key search, once — the key is cached");
	if !crate::ui::can_ask() {
		return Ok(());
	}
	if !unseal_now(&mut crate::ui::Console::new(String::new())) {
		return Ok(());
	}
	unseal_each(files, |file| {
		crate::vcds::rod::run(&file.to_string_lossy(), true, Some(&cache.to_string_lossy()), None)
	})?;
	vag_cli_core::say!("unsealed: `vagcan faults` names those codes now");
	Ok(())
}

/// Open each catalogue with `open` — `dev vcds rod`, whose tables are its data — with what
/// it prints sent to stderr: `faults > out.txt` holds the fault table alone.
fn unseal_each(files: &[PathBuf], mut open: impl FnMut(&Path) -> Result<()>) -> Result<()> {
	let n = files.len();
	for (at, file) in files.iter().enumerate() {
		vag_cli_core::say!("[{}/{n}] {}", at + 1, file.file_name().and_then(|f| f.to_str()).unwrap_or_default());
		vag_cli_core::verbosity::data_on_stderr(|| open(file))?;
	}
	Ok(())
}

/// The question, as the person reads it. `[y/N]` in the text: the answer is a
/// letter and the default is the empty line.
const UNSEAL_QUESTION: &str = "Unseal them now? [y/N]";

/// Ask whether to start the key search. Anything short of a clear yes is a no —
/// an empty line, Ctrl-D, a read that failed — because the price is minutes.
///
/// Through [`crate::ui::menu::Asker::line`], the one way a question is put to a
/// person, rather than a `print!` and a `read_line` of its own: that is where the
/// progress lines are held off the question (`crate::progress::hold`). This one's
/// own `print!` was overdrawn by the walk's still-ticking line, and `faults`
/// looked hung on "unit 18 of 18" while it waited for an answer to a question
/// nobody could see (2026-10-20).
fn unseal_now(io: &mut impl crate::ui::menu::Asker) -> bool {
	io.line(UNSEAL_QUESTION, "")
		.is_ok_and(|typed| matches!(typed.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::ui::menu::{Answer, Scripted};

	#[test]
	fn what_unsealing_prints_stays_off_the_fault_tables_stdout() {
		// `faults > out.txt`, the offer taken: `dev vcds rod`'s tables went into the file
		// after the fault table (review of PR #38).
		let capture = vag_cli_core::verbosity::Capture::start(0);
		unseal_each(&[PathBuf::from("EV_X.rod")], |_| {
			vag_cli_core::outln!("tag  state  bytes");
			Ok(())
		})
		.unwrap();
		vag_cli_core::outln!("the command's own data");
		let said = capture.finish();
		assert_eq!(said.stdout, "the command's own data\n");
		assert!(said.stderr.contains("[1/1] EV_X.rod\ntag  state  bytes\n"), "{said:?}");
	}

	#[test]
	fn the_offer_to_unseal_is_asked_through_the_one_question_helper() {
		// The helper is what holds the progress lines off the question; a
		// `print!` of its own is what left this one under a spinner.
		let mut io = Scripted::new(vec![Answer::Type("y".into())]);
		assert!(unseal_now(&mut io));
		assert_eq!(io.typed, vec![(UNSEAL_QUESTION.to_string(), String::new())]);
		for typed in ["", "n", "later"] {
			assert!(!unseal_now(&mut Scripted::new(vec![Answer::Type(typed.into())])), "{typed:?}");
		}
		assert!(!unseal_now(&mut Scripted::new(vec![Answer::Quit])), "Ctrl-D is a no");
	}

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
		assert!(unit_note(&UnitLookup::NoFile, "codes only").unwrap().contains("no ODX file"));
		let none = unit_note(
			&UnitLookup::NoData {
				source: "EV_InvMQB_018".into(),
			},
			"codes only",
		)
		.unwrap();
		assert!(none.contains("no data") && none.contains("EV_InvMQB_018"), "{none}");
		// A redirected catalogue names itself, in the one line every output uses;
		// one of the unit's own name says nothing.
		let found = |via: Option<&str>| UnitLookup::Found {
			file: PathBuf::from("/vcds/UDS_EV/EV_InvBOSCH_017.rod"),
			via: via.map(str::to_string),
			sealed_before: None,
			catalogue: UnitCatalogue::default(),
		};
		// A better variant still sealed: the text standing in is said to be another's.
		let standing_in = UnitLookup::Found {
			file: PathBuf::from("/vcds/UDS_EV/EV_InvBOSCH_017.rod"),
			via: Some("EV_InvMQB_017".into()),
			sealed_before: Some(PathBuf::from("/vcds/UDS_EV/EV_InvBOSCH_016_VW37.rod")),
			catalogue: UnitCatalogue::default(),
		};
		assert_eq!(
			unit_note(&standing_in, "codes only").as_deref(),
			Some(
				"the fault catalogue in EV_InvBOSCH_016_VW37.rod is still sealed — VCDS text from \
				 EV_InvMQB_017 → EV_InvBOSCH_017.rod (VCDS ReDir), another variant, in its place"
			)
		);
		assert_eq!(
			unit_note(&found(Some("EV_InvMQB_017")), "codes only").as_deref(),
			Some("VCDS text from EV_InvMQB_017 → EV_InvBOSCH_017.rod (VCDS ReDir)")
		);
		assert_eq!(unit_note(&found(None), "codes only"), None);
		assert!(unit_note(&UnitLookup::NoSection { candidates: 14 }, "codes only").unwrap().contains("14"));
		let sealed = UnitLookup::Locked {
			file: PathBuf::from("EV_X.rod"),
		};
		assert!(unit_note(&sealed, "codes only").unwrap().contains("EV_X.rod"));
		// Where the ODIS project has the unit, its codes are not numbers alone,
		// and the note says what they are shown with instead.
		let note = unit_note(&sealed, "the ODIS project's text").unwrap();
		assert!(note.ends_with("sealed — the ODIS project's text"), "{note}");
	}
}
