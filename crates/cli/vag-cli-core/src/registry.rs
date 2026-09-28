//! The car's channels, out of a VCDS installation's measurement registry — read where they
//! are needed.
//!
//! A unit's `.rod` lists 1-based rows of the global registry `RM.rod`, and each row carries
//! the DID, the bit layout and the scaling (`research/vcds-registry/README.md`; the reader is
//! [`vag_data_labels::registry`]). Everything here is read **from the installation**, not from
//! the shared pool `setup`'s other steps copy into: a unit's list and the registry it points
//! into mean something only together, and the pool can hold files from two builds under the
//! same names.
//!
//! **Which units.** An installation cannot be read for every unit up front: `UDS_EV` holds
//! twelve thousand unit files, each with its own section keys, and the keys for one car's
//! fifteen units took five minutes on the reference machine. Only the car can say which units
//! it has — so the list is the record `watch`, `measure` and `units --identify` write
//! (`~/.vagcan/cars/<VIN>/units.json`, [`crate::units`]), and the read happens in two places:
//! `setup`'s step 5, for the units of every car recorded on this machine, and [`ensure`],
//! which those three commands call with the car and `dev dash build` calls offline, so a car
//! connected for the first time after `setup` gets its channels without another command.
//!
//! **What was tried is written down**, per project, in `registry.json` ([`Log`]): which
//! installation, and for each unit what came of it — read, no file, shifted, key not found.
//! A unit the installation has no file for is not searched for again on every run, and
//! [`ensure`] knows where the installation is without asking — or, for a project set up
//! before the log existed, from the VCDS entry of its `sources.json` ([`Installation`]).
//! Every read is over every recorded unit at once, because the rows are replaced per kind
//! ([`vag_data_db::put_all_vcds_readings`], `Replace::Kind`): with the keys cached the known
//! units cost seconds, and the last installation read is the one whose rows the project holds.
//! [`unread`] is the one rule for what a read would still bring: [`ensure`] reads it, and the
//! dash build refuses to build a plan for a car with units it has not been made for.
//!
//! What a read could not do, it says: a table that did not open, a row that did not read, a
//! list that lost its first row. A number on its own reads as everything there was. `setup`
//! turns that into its report; [`ensure`] says one line.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use vag_data_labels::codes::CodePage;
use vag_data_labels::registry::{self, RegistryLoad, RowError, UnitMeasurements, Words};
use vag_data_labels::rod::{IvCache, KeyCost, RodStatus, decode_rod_recover, key_cost, recover_classic_key};
use vag_data_labels::ttdop::{self, TablesLoad};
use vag_data_labels::unit_strings::{self, UnitsLoad};

use crate::plan::UnitIdentity;
use crate::project::Project;

/// Where a VCDS installation keeps the ODX files, relative to its root.
///
/// A property of Ross-Tech's layout, not of any car. Its presence is what says an
/// installation is still on the machine.
pub const ODX_DIR: &str = "UDS_EV";

/// The global text table every measurement name comes out of.
///
/// **Ross-Tech names it per language build**, and that is not a detail: the
/// English one ships `TTTEXT.ROD`, the Russian one `TTText-RUS.rod`. Matching
/// only the English spelling left anyone who chose Russian with an install that
/// recovered no names at all and never said why. Nothing suggests the list is
/// closed, so a name nobody here has seen is a question to ask, not a verdict.
pub const TEXT_TABLES: &[&str] = &["TTTEXT.ROD", "TTText-RUS.rod"];

/// The code page a text table's high bytes are in, from its name — the only
/// place the build says, as for the fault text.
///
/// The English table is Windows-1252: its `0x96` is an en dash. The Russian
/// one has never been read — it is shifted, and refused before this is
/// asked — so 1251 is the page its build's `Code-RUS.dat` is in, not a page
/// anybody has seen it use.
pub fn text_page(file_name: &str) -> CodePage {
	match file_name {
		"TTText-RUS.rod" => CodePage::Windows1251,
		_ => CodePage::Windows1252,
	}
}

/// The directory the label files are actually in.
///
/// The loader reads one directory level, and what `vagcan setup` is pointed at
/// is an install root — labels in `Labels/`, ODX files in `UDS_EV/` — so
/// pointing it straight at the root would cache nothing and report "cached 0
/// label files", which reads as empty rather than as the wrong level.
///
/// So the directory is located rather than assumed: the one given if it holds
/// label files, otherwise the first child that does. That also picks
/// `Labels/RUS/` out of a Russian build, where nothing sits at the top level.
/// Two levels is enough for every layout Ross-Tech ships and shallow enough not
/// to wander into a home directory.
///
/// Here rather than in `diag`'s label tooling because the registry rows are
/// written under the same source string as the label files ([`source_of`]):
/// an install is one source with one language, whichever of its files a row
/// came from.
pub fn label_dir_under(given: &Path) -> Result<PathBuf> {
	fn holds_labels(dir: &Path) -> bool {
		std::fs::read_dir(dir).is_ok_and(|entries| {
			entries.flatten().any(|e| {
				matches!(
					e.path().extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref(),
					Some("lbl") | Some("clb")
				)
			})
		})
	}

	if holds_labels(given) {
		return Ok(given.to_path_buf());
	}
	let mut children: Vec<PathBuf> = std::fs::read_dir(given)
		.with_context(|| format!("reading {}", given.display()))?
		.flatten()
		.map(|e| e.path())
		.filter(|p| p.is_dir())
		.collect();
	children.sort();
	for child in &children {
		if holds_labels(child) {
			// Silently: this is the layout of what `vagcan setup` reads, not a
			// choice the reader made or can act on, and it was being announced
			// on every single run.
			return Ok(child.clone());
		}
	}
	anyhow::bail!(
		"no label files under {} — expected a VCDS install root (with a Labels directory) \
         or the Labels directory itself",
		given.display()
	)
}

/// Where an install's registry rows are written: under the source its label
/// files were written under, so an install is one source with one language,
/// whichever of its files a row came from.
fn source_of(root: &Path) -> String {
	label_dir_under(root)
		.unwrap_or_else(|_| root.to_path_buf())
		.to_string_lossy()
		.into_owned()
}

// ---------------------------------------------------------------------------
// The log: which installation, which units, what came of each
// ---------------------------------------------------------------------------

/// One unit as the log holds it: its `F19E`, its `F1A2`, and what the read came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tried {
	pub odx_name: String,
	#[serde(default)]
	pub odx_version: String,
	/// `read`, `no file`, `no list`, `key not found`, `shifted`, `unresolved`,
	/// `mismatched`, or `registry: …` when the registry itself did not open.
	pub outcome: String,
}

/// A project's `registry.json`: the installation its VCDS channels were read
/// from, and every unit tried with it.
///
/// Replaced whole by every read, because every read covers every unit
/// recorded on this machine and its rows replace the last installation's:
/// what the log says was tried with *this* installation is what the cache
/// holds. Written after the rows, never before — a log that said "read" for
/// rows an interrupted run never wrote would keep those units from ever being
/// read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Log {
	pub installation: PathBuf,
	#[serde(default)]
	pub units: Vec<Tried>,
}

impl Log {
	/// The project's log, or `None` when no read has been made into it — an
	/// ODIS-only project, or one set up before this file existed.
	pub fn load(project: &Project) -> Result<Option<Log>> {
		let path = project.registry_log();
		let text = match std::fs::read_to_string(&path) {
			Ok(text) => text,
			Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
			Err(e) => return Err(anyhow::Error::new(e).context(format!("reading {}", path.display()))),
		};
		serde_json::from_str(&text).map(Some).with_context(|| {
			format!(
				"{} is not a registry log this tool wrote — move it aside, and setup writes a fresh one",
				path.display()
			)
		})
	}

	pub fn save(&self, project: &Project) -> Result<()> {
		let mut text = serde_json::to_string_pretty(self).context("encoding the registry log")?;
		text.push('\n');
		crate::datadir::replace_file(&project.registry_log(), text.as_bytes())
	}

	/// Whether this unit has been tried with the recorded installation, whatever came of it.
	pub fn tried(&self, odx_name: &str, odx_version: &str) -> bool {
		self.units.iter().any(|t| t.odx_name == odx_name && t.odx_version == odx_version)
	}
}

/// Record the installation without reading anything: for a `setup` run on a
/// machine no car has been recorded on yet, so that the first `watch` knows
/// where to read from.
pub fn record_installation(project: &Project, root: &Path) -> Result<()> {
	Log {
		installation: installation_path(root),
		units: Vec::new(),
	}
	.save(project)
}

/// [`record_installation`] for a `setup` whose registry step did not run: the
/// log names this run's installation afterwards, and what was tried is kept
/// when that is the installation already logged. Rewriting it whole there
/// made every unit count as unread — the rows still in the cache, the dash
/// build refusing, `ensure` announcing fifteen units with no channels (found
/// in review, 2026-09-28) — for a step that read nothing and changed nothing.
pub fn note_installation(project: &Project, root: &Path) -> Result<()> {
	let installation = installation_path(root);
	// A log that does not read is no log: this is the step that did not
	// complete already, and a second unreadable file must not turn its report
	// into an abort (found in review, 2026-09-28). The installation is recorded
	// over it, as for a project that never had one.
	if let Some(log) = Log::load(project).ok().flatten()
		&& log.installation == installation
	{
		return Ok(());
	}
	record_installation(project, root)
}

/// The installation as the log names it: resolved, so a path typed relative to
/// where `setup` ran still means the same directory from wherever `watch` runs
/// — as `sources.json` keeps it. One that cannot be resolved is kept as given.
fn installation_path(root: &Path) -> PathBuf {
	std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// The word the log keeps for what a unit's list came to.
fn outcome(found: &UnitMeasurements) -> &'static str {
	match found {
		UnitMeasurements::Found { .. } => "read",
		UnitMeasurements::NoFile => "no file",
		UnitMeasurements::NoList { .. } => "no list",
		UnitMeasurements::Locked { .. } => "key not found",
		UnitMeasurements::Shifted { .. } => "shifted",
		UnitMeasurements::Unresolved { .. } => "unresolved",
		UnitMeasurements::Mismatched { .. } => "mismatched",
	}
}

// ---------------------------------------------------------------------------
// The read
// ---------------------------------------------------------------------------

/// Where a read says what it is doing. `setup` prints a line per unit under its
/// step header and spins while a table opens; [`ensure`] keeps one spinner and
/// says nothing per unit.
pub trait Progress {
	/// A wait inside one call: a table opening, a unit's list being found —
	/// minutes, when a key has to be searched for.
	fn waiting(&mut self, what: &str);
	/// One unit is done: what it came to, in a line.
	fn done(&mut self, line: &str);
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

/// What a read that got as far as the units came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
	/// Units asked for.
	pub asked: usize,
	/// Units whose list opened.
	pub read: usize,
	/// Channels written into the project's cache.
	pub written: usize,
	/// Listed rows that are not channels: raw bytes, text, a formula not decoded.
	pub not_channels: usize,
	/// Listed rows that did not read.
	pub unread: usize,
	/// Keys this run searched for and found.
	pub keys_found: usize,
	/// Whether the keys found were saved — `None` when they were.
	pub keys_unsaved: Option<String>,
	/// What could not be read, one line each; empty when everything was.
	pub notes: Vec<String>,
	/// The cache the rows went into.
	pub cache: PathBuf,
}

impl Summary {
	/// The sentence a report gives for the read: what was written, for how many
	/// units, and what the listed rows that brought nothing were.
	pub fn detail(&self) -> String {
		let mut detail = format!("{} channels for {} of {} unit{}", self.written, self.read, self.asked, plural(self.asked));
		if self.not_channels > 0 {
			detail.push_str(&format!(
				"; {} listed rows are not channels (raw bytes, text, a type or OBD formula not decoded, a text table without words)",
				self.not_channels
			));
		}
		if self.unread > 0 {
			detail.push_str(&format!("; {} listed rows did not read", self.unread));
		}
		if self.keys_found > 0 {
			let kept = match self.keys_unsaved {
				None => "cached",
				Some(_) => "not saved",
			};
			detail.push_str(&format!("; {} key{} found, {kept}", self.keys_found, plural(self.keys_found)));
		}
		detail
	}
}

/// What a read came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Read {
	/// `RM.rod` did not open, so nothing could be read; `removed` says what rows
	/// an earlier read had left, now cleared.
	NoRegistry {
		why: String,
		removed: String,
	},
	Done(Summary),
}

/// Leave no registry rows behind when this installation gives none: every row
/// comes from the last installation read, as the label files do, and rows from
/// another one beside this one's label files would be two builds in one project.
///
/// Says in words what it removed, for the step's reason: channels an earlier
/// run read disappearing without a word is how a working dash stops building.
pub fn clear(root: &Path, project: &Project) -> Result<String> {
	let held = vag_data_db::reading_count_of(&project.cache(), vag_data_db::VCDS).unwrap_or(0);
	vag_data_db::put_all_vcds_readings(&project.cache(), &source_of(root), std::iter::empty())?;
	Ok(match held {
		0 => String::new(),
		n => format!("\n    This run removed the {n} channels an earlier run read from a VCDS registry."),
	})
}

/// Read the registry for `units` — `(F19E, F1A2)` each — out of the
/// installation at `root`, write their channels into the project's cache, and
/// log what came of every unit.
///
/// Every key found is saved as it is found, so a run that is interrupted
/// leaves the next one less to search for.
pub fn read(root: &Path, project: &Project, units: &BTreeSet<(String, String)>, progress: &mut impl Progress) -> Result<Read> {
	let _reading = Reading::start();
	let path = project.rod_keys();
	let cache = IvCache::load(&path);
	let mut keys = Keys {
		path: &path,
		held: cache.len(),
		cache,
		unsaved: None,
	};
	let mut notes = Vec::new();
	let mut log = Log {
		installation: installation_path(root),
		units: Vec::new(),
	};

	progress.waiting("opening RM.rod");
	let opened = registry::load_registry(root, &mut keys.cache, true);
	keys.save();
	let registry = match opened {
		RegistryLoad::Found { registry, .. } => registry,
		other => {
			// Tried, all of them, with what stopped them: a registry that does
			// not open is not searched for again on every `watch`.
			let word = format!("registry: {}", registry_word(&other));
			log.units = units
				.iter()
				.map(|(odx, version)| Tried {
					odx_name: odx.clone(),
					odx_version: version.clone(),
					outcome: word.clone(),
				})
				.collect();
			// The log before the rows go, on this path: a kill between the two
			// would otherwise leave the rows gone and the old log still saying
			// "read" for them (found in review, 2026-09-28). A log that cannot
			// be written is said, as on the other path; it never stops a read,
			// and the next run tries again.
			let mut why = registry_why(&other);
			if let Err(e) = log.save(project) {
				why.push_str(&format!("; what was tried was not written down ({e:#})"));
			}
			let removed = clear(root, project)?;
			return Ok(Read::NoRegistry { why, removed });
		}
	};
	progress.waiting("opening TTDOP.rod");
	let tables = ttdop::load_text_tables(root, &mut keys.cache, true);
	// After each table, not after the three: a key found for TTDOP.rod that
	// waited in memory for UNIT.ROD's search to end was lost to a Ctrl-C during
	// that search, and the notice that the tables' keys are cached was untrue
	// (found in review, 2026-09-28).
	keys.save();
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
	progress.waiting("opening the text table");
	let (words, page) = words(root, &mut keys.cache, &mut notes);
	keys.save();
	progress.waiting("opening UNIT.ROD");
	let units_of = unit_strings::load_unit_strings(root, &mut keys.cache, true, page);
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
	for (at, (odx, version)) in units.iter().enumerate() {
		// "unit N of M": the searches can take minutes each, and a count is what
		// tells somebody watching how much of the wait is left.
		progress.waiting(&format!("{odx}: finding its measurement list — unit {} of {}", at + 1, units.len()));
		let found = registry::unit_measurements(root, odx, version, &mut keys.cache, true, &registry);
		keys.save();
		log.units.push(Tried {
			odx_name: odx.clone(),
			odx_version: version.clone(),
			outcome: outcome(&found).to_string(),
		});
		let UnitMeasurements::Found { file, rows, .. } = found else {
			if !extracted.for_unit_from(vag_data_db::ODIS, Some(odx), Some(version)).is_empty() {
				progress.done(&format!("{odx}: {}; the ODIS project describes it", unit_gap(&found)));
				continue;
			}
			let why = format!("{odx}: {}", unit_why(&found));
			progress.done(&why);
			notes.push(why);
			continue;
		};
		read += 1;
		let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or(odx).to_string();
		// Two cars can carry one unit at two versions that come to the same
		// file. Its rows are written once.
		if variants.contains_key(&stem) {
			progress.done(&format!("{odx} {version}: the same list as above"));
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
		progress.done(&format!("{odx}: {} channels", channels.len()));
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
	// After the rows and never before: see `Log`.
	if let Err(e) = log.save(project) {
		notes.push(format!(
			"what was tried was not written down ({e:#}): the next run tries every unit again"
		));
	}
	Ok(Read::Done(Summary {
		asked: units.len(),
		read,
		written,
		not_channels,
		unread,
		keys_found: keys.found(),
		keys_unsaved: keys.unsaved.clone(),
		notes,
		cache: project.cache(),
	}))
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

/// [`registry_why`] in the log's words.
fn registry_word(load: &RegistryLoad) -> &'static str {
	match load {
		RegistryLoad::NoFile => "no file",
		RegistryLoad::Locked { .. } => "key not found",
		RegistryLoad::Shifted { .. } => "shifted",
		RegistryLoad::Found { .. } => "read",
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

// ---------------------------------------------------------------------------
// Reading for the car in front of the tool
// ---------------------------------------------------------------------------

/// The `(F19E, F1A2)` a unit is looked up by; `None` for a unit that did not
/// say what it is, which no installation can be read for.
fn key(unit: &UnitIdentity) -> Option<(String, String)> {
	Some((unit.odx_name.clone()?, unit.odx_version.clone().unwrap_or_default()))
}

/// The units of this car the installation has not been tried for and nothing
/// else describes — what would make a read worth its minutes.
///
/// `described` is whether an ODIS variant in the project describes the unit,
/// passed in so the rule can be tested without a cache.
fn pending(log: &Log, identities: &[UnitIdentity], described: impl Fn(&str, &str) -> bool) -> BTreeSet<(String, String)> {
	identities
		.iter()
		.filter_map(key)
		.filter(|(odx, version)| !log.tried(odx, version) && !described(odx, version))
		.collect()
}

/// Whether an ODIS variant in the project describes a unit: the `described` a
/// caller with a cache passes to [`unread`].
pub fn described_by(extracted: &crate::extracted::Extracted) -> impl Fn(&str, &str) -> bool + '_ {
	move |odx, version| !extracted.for_unit_from(vag_data_db::ODIS, Some(odx), Some(version)).is_empty()
}

/// What to type to have the installation read again, in every line that says
/// it cannot be.
const RERUN_SETUP: &str = "re-run `vagcan setup <VCDS installation>`";

/// Where a project's VCDS channels are read from.
///
/// The log names it once a read has been made. A project set up **before the log
/// existed** has none, and with nothing else to go on `ensure` did nothing and
/// said nothing (found in review, 2026-09-28) — so `sources.json`, which every
/// `setup` has written, is the fallback: its VCDS entry is the installation when
/// its path is absolute and holds `UDS_EV`. Absolute, because a relative path in
/// there was true in the directory `setup` ran in and nowhere else, and this tool
/// resolves nothing against the working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Installation {
	/// On this machine, `UDS_EV` in place: a read can be made from it.
	At(PathBuf),
	/// The project names one, and it cannot be read from — gone, or recorded
	/// relative to where `setup` ran. `why` says which, and what to type.
	Unusable { why: String },
	/// The project never read a VCDS installation and names none: ODIS alone.
	None,
}

/// The rule behind [`Installation`], on the log (if any) and the project's
/// `sources.json` VCDS entries, **newest first** ([`crate::project::sources_of_kind`]).
/// Only the newest counts: its rows are the ones the project holds, and an older
/// entry is an installation whose rows they replaced.
fn installation(log: Option<&Log>, vcds_sources: &[String]) -> Installation {
	if let Some(log) = log {
		let root = &log.installation;
		if root.join(ODX_DIR).is_dir() {
			return Installation::At(root.clone());
		}
		return Installation::Unusable {
			why: format!(
				"the VCDS installation to read from is not on this machine any more: {} — {RERUN_SETUP}",
				root.display()
			),
		};
	}
	let Some(named) = vcds_sources.first() else {
		return Installation::None;
	};
	let root = Path::new(named);
	if root.is_absolute() && root.join(ODX_DIR).is_dir() {
		return Installation::At(root.to_path_buf());
	}
	Installation::Unusable {
		why: format!(
			"the project was set up from a VCDS installation before this tool wrote down where it is, and its own note of it \
			 ({named}) is not a directory on this machine holding {ODX_DIR} — {RERUN_SETUP}"
		),
	}
}

/// Whether a registry read is running on this process, for the Ctrl-C handler:
/// it cannot see the spinner a blocking read is drawing, but it can end the
/// line and say what an interrupted key search leaves behind.
pub fn reading() -> bool {
	READING.load(std::sync::atomic::Ordering::Relaxed)
}

static READING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// [`READING`] set for the life of one [`read`], however it returns.
struct Reading;

impl Reading {
	fn start() -> Reading {
		READING.store(true, std::sync::atomic::Ordering::Relaxed);
		Reading
	}
}

impl Drop for Reading {
	fn drop(&mut self) {
		READING.store(false, std::sync::atomic::Ordering::Relaxed);
	}
}

/// The channels of this car's units a VCDS read could still bring, and where from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unread {
	/// Nothing a read would bring: every unit tried with the installation or
	/// described by the project — or the project has no VCDS installation.
	Nothing,
	/// `units` have not been read, and the installation is at `root`.
	Readable { units: BTreeSet<(String, String)>, root: PathBuf },
	/// `units` have not been read, and cannot be: `why`.
	Unreadable { units: BTreeSet<(String, String)>, why: String },
}

/// Which of this car's units the project's VCDS installation has not been tried
/// for, and whether it can be — the question [`ensure`] acts on and the dash
/// build refuses on.
///
/// `described` is whether an ODIS variant describes a unit ([`described_by`]).
/// A `registry.json` that does not read is an error, said by the caller.
pub fn unread(project: &Project, identities: &[UnitIdentity], described: impl Fn(&str, &str) -> bool) -> Result<Unread> {
	let log = Log::load(project)?;
	let where_from = installation(log.as_ref(), &crate::project::sources_of_kind(project, vag_data_db::VCDS));
	if where_from == Installation::None {
		return Ok(Unread::Nothing);
	}
	let untried = log.unwrap_or_else(|| Log {
		installation: PathBuf::new(),
		units: Vec::new(),
	});
	let units = pending(&untried, identities, described);
	if units.is_empty() {
		return Ok(Unread::Nothing);
	}
	Ok(match where_from {
		Installation::At(root) => Unread::Readable { units, root },
		Installation::Unusable { why } => Unread::Unreadable { units, why },
		Installation::None => Unread::Nothing,
	})
}

/// "{n} control unit(s) of this car" — the subject of every line about them.
fn units_of_this_car(n: usize) -> String {
	format!("{n} control unit{} of this car", plural(n))
}

/// Why a unit of this car has no VCDS channels, per unit, out of the project's
/// log — for `watch`'s summary, which names the units nothing describes and
/// owes the reader the cause the read logged for each (found in review,
/// 2026-09-28). Keyed by request id; a unit the log has no line for, or whose
/// list was read, has no entry. Empty when there is no project or no log, or
/// the log does not read: the summary is not the place to say so.
pub fn causes(identities: &[UnitIdentity]) -> BTreeMap<u16, String> {
	let Ok(project) = crate::project::current() else {
		return BTreeMap::new();
	};
	let Ok(Some(log)) = Log::load(&project) else {
		return BTreeMap::new();
	};
	identities
		.iter()
		.filter_map(|unit| {
			let (odx, version) = key(unit)?;
			let tried = log.units.iter().find(|t| t.odx_name == odx && t.odx_version == version)?;
			Some((unit.request, format!("{odx}: {}", gap_of(&tried.outcome)?)))
		})
		.collect()
}

/// The gap behind a word the log keeps ([`outcome`]), in the words `setup`
/// printed for it — `None` for a list that was read.
fn gap_of(outcome: &str) -> Option<String> {
	Some(match outcome {
		"read" => return None,
		"no file" => unit_gap(&UnitMeasurements::NoFile).to_string(),
		"no list" => "none of its files lists measurements".to_string(),
		"key not found" => "the search for its list's key found nothing".to_string(),
		"shifted" => unit_gap(&UnitMeasurements::Shifted {
			file: PathBuf::new(),
			section: String::new(),
		})
		.to_string(),
		"unresolved" => unit_gap(&UnitMeasurements::Unresolved { file: PathBuf::new() }).to_string(),
		"mismatched" => unit_gap(&UnitMeasurements::Mismatched {
			file: PathBuf::new(),
			listed: 0,
			outside: 0,
		})
		.to_string(),
		other => match other.strip_prefix("registry: ") {
			Some(why) => format!("the registry did not open ({why})"),
			None => other.to_string(),
		},
	})
}

/// Read the channels of the units of this car that nothing has read yet.
///
/// For `watch`, `measure`, `units --identify` and the dash build, right after
/// they know the car's units: when the project was set up from a VCDS
/// installation, the units no ODIS variant describes and that installation has
/// not been tried for are read now — one line, a spinner, a line per unit as
/// it is done, the first time only — so the caller's channels then include
/// them. The read is over every unit recorded on this machine plus these (the
/// rows are replaced per kind), and the caller reloads its channels afterwards.
///
/// Nothing to say when there is nothing to do: no project, a project with no
/// VCDS installation, every unit tried or described already. An installation
/// that cannot be read from is one line saying what to type, and no read. A
/// record of another car that does not read is one line and no read either: a
/// read replaces every car's VCDS rows, and one made without that car's units
/// would drop its rows. Nothing here stops the command: a read that fails is
/// one line.
pub fn ensure(identities: &[UnitIdentity]) {
	if let Err(e) = try_ensure(identities) {
		eprintln!("the channels of this car's units were not read from the VCDS installation: {e:#}");
	}
}

/// [`ensure`] off the async runtime's worker threads: the read is minutes of
/// CPU when a key has to be searched for, and a worker that long blocked would
/// starve the bus task beside it — and, on the main task, sit between Ctrl-C
/// and the `select!` that acts on it.
pub async fn ensure_async(identities: Vec<UnitIdentity>) {
	if let Err(e) = tokio::task::spawn_blocking(move || ensure(&identities)).await {
		eprintln!("the channels of this car's units were not read from the VCDS installation: {e}");
	}
}

fn try_ensure(identities: &[UnitIdentity]) -> Result<()> {
	// No project is a machine `setup` has not run on; the commands say so
	// themselves, in `missing`'s words, when they find no channels.
	let Ok(project) = crate::project::current() else {
		return Ok(());
	};
	let extracted = crate::extracted::open(&project);
	let (wanted, root) = match unread(&project, identities, described_by(&extracted))? {
		Unread::Nothing => return Ok(()),
		Unread::Unreadable { units, why } => {
			eprintln!(
				"{} {} no channels yet, and {why}",
				units_of_this_car(units.len()),
				if units.len() == 1 { "has" } else { "have" }
			);
			return Ok(());
		}
		Unread::Readable { units, root } => (units, root),
	};
	// Every unit recorded on this machine, plus these: the rows are replaced
	// per kind, so a read of these alone would drop every other car's. Loaded
	// before anything is announced, and a record that does not read skips the
	// read rather than lose those rows.
	let cars = match crate::units::recorded_cars() {
		Ok(cars) => cars,
		Err(e) => {
			eprintln!(
				"{} {} no channels yet, and they were not read: {e:#}.\n  A read replaces every car's VCDS channels at once, and one made without that car's units would drop its rows.",
				units_of_this_car(wanted.len()),
				if wanted.len() == 1 { "has" } else { "have" }
			);
			return Ok(());
		}
	};
	let mut units: BTreeSet<(String, String)> = cars.iter().flat_map(|car| car.units.iter()).filter_map(key).collect();
	units.extend(wanted.iter().cloned());
	// Both numbers: what this car still lacks, and how many units the read
	// covers — every recorded one, since the rows are replaced at once.
	eprintln!(
		"Reading the VCDS registry for {} recorded control unit{} — {} of this car not read yet; the first time only, and a key not cached yet is searched for, minutes.",
		units.len(),
		plural(units.len()),
		wanted.len()
	);
	let mut progress = Quietly {
		spinner: None,
		said: Vec::new(),
	};
	let outcome = read(&root, &project, &units, &mut progress)?;
	let said = std::mem::take(&mut progress.said);
	drop(progress);
	match outcome {
		Read::Done(summary) => {
			eprintln!("  {}", summary.detail());
			// What the read could not do, beyond what the unit lines said already:
			// a table that did not open, a list that lost its first row, keys not
			// saved.
			for note in summary.notes.iter().filter(|note| !said.contains(note)) {
				eprintln!("  {note}");
			}
		}
		Read::NoRegistry { why, removed } => eprintln!("  the registry did not open: {why}{removed}"),
	}
	Ok(())
}

/// [`ensure`]'s progress: a spinner while a table opens or a list is found, and
/// a line per unit on stderr as each is done — the same cause and remedy
/// `setup` prints, since a unit VCDS has no file for is said here once and
/// never searched for again.
struct Quietly {
	spinner: Option<crate::progress::Spinner>,
	/// The unit lines printed, so the summary's notes do not say them again.
	said: Vec<String>,
}

impl Progress for Quietly {
	fn waiting(&mut self, what: &str) {
		match &mut self.spinner {
			Some(spinner) => spinner.update(&format!("reading the VCDS registry — {what}")),
			None => self.spinner = Some(crate::progress::Spinner::new(format!("reading the VCDS registry — {what}"))),
		}
	}

	fn done(&mut self, line: &str) {
		// Cleared before the line is printed, or the line lands on the spinner's.
		self.spinner = None;
		eprintln!("  {line}");
		self.said.push(line.to_string());
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn unit(request: u16, odx: Option<&str>, version: Option<&str>) -> UnitIdentity {
		UnitIdentity {
			request,
			part_number: Some("PART".into()),
			odx_name: odx.map(str::to_string),
			odx_version: version.map(str::to_string),
			component: None,
		}
	}

	fn tried(odx: &str, version: &str, outcome: &str) -> Tried {
		Tried {
			odx_name: odx.into(),
			odx_version: version.into(),
			outcome: outcome.into(),
		}
	}

	/// A project in a throwaway directory: nothing here touches the owner's own.
	fn project(dir: &Path) -> Project {
		Project {
			id: "TEST".into(),
			dir: dir.join("TEST"),
		}
	}

	#[test]
	fn every_reason_a_unit_brings_no_channels_is_said_and_logged_in_a_word() {
		let file = PathBuf::from("EV_X.rod");
		let outcomes = [
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
		];
		let mut words = BTreeSet::new();
		for outcome_ in &outcomes {
			assert!(!unit_why(outcome_).is_empty(), "{outcome_:?}");
			assert!(!unit_gap(outcome_).is_empty(), "{outcome_:?}");
			assert!(words.insert(outcome(outcome_)), "two outcomes share the word {:?}", outcome(outcome_));
		}
		assert!(unit_why(&UnitMeasurements::Shifted { file, section: "INC".into() }).contains("ODIS"));
		assert!(unit_why(&UnitMeasurements::NoFile).contains("ODIS"), "and what covers it instead");
		assert!(!words.contains("read"), "read is the word for a list that opened");
		// And every word the log keeps reads back as the gap it stood for, so
		// `watch`'s summary can say per unit what `setup` said.
		for outcome_ in &outcomes {
			let word = outcome(outcome_);
			assert_eq!(gap_of(word).as_deref(), Some(unit_gap(outcome_)), "{word}");
		}
		assert_eq!(gap_of("read"), None);
		assert_eq!(gap_of("registry: no file").as_deref(), Some("the registry did not open (no file)"));
	}

	#[test]
	fn every_table_that_does_not_open_is_said_with_why() {
		let file = PathBuf::from("X.rod");
		for load in [
			TablesLoad::NoFile,
			TablesLoad::Locked { file: file.clone() },
			TablesLoad::Shifted { file: file.clone() },
		] {
			assert!(!tables_why(&load).is_empty(), "{load:?}");
		}
		for load in [
			UnitsLoad::NoFile,
			UnitsLoad::Locked { file: file.clone() },
			UnitsLoad::Shifted { file: file.clone() },
		] {
			assert!(!units_why(&load).is_empty(), "{load:?}");
		}
		for load in [
			RegistryLoad::NoFile,
			RegistryLoad::Locked { file: file.clone() },
			RegistryLoad::Shifted { file },
		] {
			assert!(!registry_why(&load).is_empty(), "{load:?}");
			assert_ne!(registry_word(&load), "read", "{load:?}");
		}
	}

	#[test]
	fn the_log_is_read_back_as_written_and_knows_what_was_tried() {
		let dir = tempfile::tempdir().unwrap();
		let project = project(dir.path());
		assert_eq!(Log::load(&project).unwrap(), None, "no log is a project that never read an installation");
		let log = Log {
			installation: PathBuf::from("/somewhere/vcds"),
			units: vec![tried("EV_A", "001007", "read"), tried("EV_B", "017001", "no file")],
		};
		log.save(&project).unwrap();
		assert_eq!(Log::load(&project).unwrap(), Some(log.clone()));
		assert!(log.tried("EV_B", "017001"), "tried, whatever came of it");
		assert!(!log.tried("EV_B", "017002"), "the version is part of what was tried");
		assert!(!log.tried("EV_C", ""));
		// `setup` on a machine no car has been recorded on records where the
		// installation is and nothing else.
		record_installation(&project, Path::new("/elsewhere/vcds")).unwrap();
		let fresh = Log::load(&project).unwrap().unwrap();
		assert_eq!(fresh.installation, PathBuf::from("/elsewhere/vcds"));
		assert!(fresh.units.is_empty());
	}

	#[test]
	fn a_setup_whose_registry_step_did_not_run_keeps_what_was_tried_with_the_same_installation() {
		// Found in review (2026-09-28): the failed step rewrote the log with no
		// unit tried even for the installation already logged, so every unit
		// then counted as unread. Kept for the same installation; a different
		// one is recorded afresh, since nothing was tried with it.
		let dir = tempfile::tempdir().unwrap();
		let project = project(dir.path());
		let same = install_at(dir.path());
		Log {
			installation: same.canonicalize().unwrap(),
			units: vec![tried("EV_A", "001007", "read"), tried("EV_B", "017001", "no file")],
		}
		.save(&project)
		.unwrap();
		note_installation(&project, &same).unwrap();
		assert_eq!(
			Log::load(&project).unwrap().unwrap().units.len(),
			2,
			"the same installation: what was tried stands"
		);
		let other = dir.path().join("another-vcds");
		std::fs::create_dir_all(other.join(ODX_DIR)).unwrap();
		note_installation(&project, &other).unwrap();
		let log = Log::load(&project).unwrap().unwrap();
		assert_eq!(log.installation, other.canonicalize().unwrap(), "another installation is recorded");
		assert!(log.units.is_empty(), "and nothing was tried with it");
		// No log at all: recorded, as `setup` with no car does.
		std::fs::remove_file(project.registry_log()).unwrap();
		note_installation(&project, &same).unwrap();
		assert_eq!(Log::load(&project).unwrap().unwrap().installation, same.canonicalize().unwrap());
		// The double fault: a log that does not read either. Not an abort — the
		// step's report is what says a file is bad — and the installation is
		// recorded over it (found in review, 2026-09-28).
		std::fs::write(project.registry_log(), "not json").unwrap();
		assert!(Log::load(&project).is_err(), "the fixture is unreadable");
		note_installation(&project, &other).unwrap();
		let log = Log::load(&project).unwrap().expect("recorded over the unreadable log");
		assert_eq!(log.installation, other.canonicalize().unwrap());
		assert!(log.units.is_empty());
	}

	#[test]
	fn every_tables_keys_are_saved_before_the_next_tables_search_begins() {
		// Found in review (2026-09-28): the three tables' keys were saved together
		// after UNIT.ROD, so a Ctrl-C during that search lost a key found for
		// TTDOP.rod minutes earlier, and the Ctrl-C notice that the tables' keys
		// are cached was untrue. Pinned against the source, as `setup`'s step
		// order is: a run with keys to search for takes minutes per table.
		let body = include_str!("registry.rs");
		let read = &body[body.find("pub fn read(").expect("read is here")..];
		// Up to the unit loop, and no further: past it the text runs on into
		// this test, whose own list names every load again, and the last
		// table's search would then reach the per-unit saves and pass on them.
		let read = &read[..read.find("for (at, (odx, version))").expect("the unit loop")];
		let loads = ["load_registry(", "load_text_tables(", "= words(", "load_unit_strings("];
		let mut from = 0;
		for load in loads {
			let at = from + read[from..].find(load).unwrap_or_else(|| panic!("{load} is gone"));
			let next = loads
				.iter()
				.filter_map(|other| read[at + load.len()..].find(other).map(|n| at + load.len() + n))
				.min()
				.unwrap_or(read.len());
			assert!(
				read[at..next].contains("keys.save();"),
				"no keys.save() between {load} and the search after it"
			);
			from = at + load.len();
		}
	}

	#[test]
	fn a_read_is_worth_making_only_for_units_untried_with_this_installation_and_described_by_nothing() {
		let log = Log {
			installation: PathBuf::from("/vcds"),
			units: vec![tried("EV_Tried", "001", "no file")],
		};
		let identities = [
			unit(0x7E0, Some("EV_Tried"), Some("001")),
			unit(0x7E1, Some("EV_Odis"), Some("002")),
			unit(0x713, Some("EV_New"), Some("003")),
			unit(0x714, Some("EV_New"), Some("003")),
			unit(0x715, Some("EV_NoVersion"), None),
			unit(0x746, None, None),
		];
		let wanted = pending(&log, &identities, |odx, _| odx == "EV_Odis");
		let expected: BTreeSet<(String, String)> = [("EV_New".to_string(), "003".to_string()), ("EV_NoVersion".to_string(), String::new())].into();
		assert_eq!(wanted, expected, "tried, described and nameless units are left out; a unit twice is one");
		// The same unit at another version is another list to find.
		let wanted = pending(&log, &[unit(0x7E0, Some("EV_Tried"), Some("002"))], |_, _| false);
		assert_eq!(wanted.len(), 1);
	}

	/// A directory that passes for an installation: `UDS_EV` in place, nothing in it.
	fn install_at(dir: &Path) -> PathBuf {
		let root = dir.join("vcds");
		std::fs::create_dir_all(root.join(ODX_DIR)).unwrap();
		root
	}

	#[test]
	fn the_installation_is_the_logs_or_else_the_absolute_vcds_source_holding_udsev() {
		// Found in review (2026-09-28): a project set up before `registry.json`
		// existed has no log, and `ensure` did nothing and said nothing — so
		// `sources.json`'s VCDS entry stands in when it can be used, and when
		// it cannot the line says what to type.
		let dir = tempfile::tempdir().unwrap();
		let root = install_at(dir.path());
		let logged = Log {
			installation: root.clone(),
			units: Vec::new(),
		};
		assert_eq!(installation(Some(&logged), &[]), Installation::At(root.clone()), "the log's, first");
		let gone = Log {
			installation: dir.path().join("moved-away"),
			units: Vec::new(),
		};
		let Installation::Unusable { why } = installation(Some(&gone), &[root.display().to_string()]) else {
			panic!("a logged installation that is gone is not replaced by a source entry");
		};
		assert!(why.contains("not on this machine any more"), "{why}");
		assert!(why.contains("moved-away") && why.contains("vagcan setup <VCDS installation>"), "{why}");
		// No log: the newest source (first in the list), when absolute and
		// holding UDS_EV. An older entry is an installation whose rows the newest
		// replaced, so it is never fallen back on (found in review, 2026-09-28).
		assert_eq!(installation(None, &[root.display().to_string()]), Installation::At(root.clone()));
		assert_eq!(
			installation(None, &[root.display().to_string(), "vendor/vcds-en".to_string()]),
			Installation::At(root.clone()),
			"the newest entry"
		);
		let Installation::Unusable { why } = installation(None, &["vendor/vcds-en".to_string(), root.display().to_string()]) else {
			panic!("a relative newest entry cannot be used, whatever an older one says, and is never resolved against the working directory");
		};
		assert!(
			why.contains("vendor/vcds-en") && why.contains("vagcan setup <VCDS installation>"),
			"{why}"
		);
		let Installation::Unusable { why } = installation(None, &[dir.path().join("gone").display().to_string()]) else {
			panic!("an absolute entry with no UDS_EV cannot be used");
		};
		assert!(why.contains(ODX_DIR), "{why}");
		// No log and no VCDS source: an ODIS-only project has nothing to read.
		assert_eq!(installation(None, &[]), Installation::None);
	}

	#[test]
	fn what_is_unread_is_asked_of_the_log_or_of_the_sources_and_of_what_odis_describes() {
		let dir = tempfile::tempdir().unwrap();
		let project = project(dir.path());
		std::fs::create_dir_all(&project.dir).unwrap();
		let root = install_at(dir.path());
		let identities = [
			unit(0x7E0, Some("EV_Engine"), Some("001")),
			unit(0x713, Some("EV_Brake"), Some("002")),
			unit(0x746, None, None),
		];
		// No log, no sources: ODIS alone — nothing a VCDS read would bring.
		assert_eq!(unread(&project, &identities, |_, _| false).unwrap(), Unread::Nothing);
		// The owner's pair project: a relative VCDS entry beside the ODIS one,
		// and every unit ODIS-described. Silent — no line, no read.
		crate::project::record_source(
			&project,
			crate::project::SourceEntry {
				kind: vag_data_db::VCDS,
				path: "vendor/vcds-en".into(),
				version: None,
				detail: None,
			},
		)
		.unwrap();
		// `record_source` stamps the moment; the entry below has to be the newer one.
		std::thread::sleep(std::time::Duration::from_millis(5));
		assert_eq!(unread(&project, &identities, |_, _| true).unwrap(), Unread::Nothing);
		// The same entry with a unit nothing describes: unreadable, with what to type.
		let Unread::Unreadable { units, why } = unread(&project, &identities, |odx, _| odx == "EV_Engine").unwrap() else {
			panic!("a relative entry cannot be read from");
		};
		assert_eq!(units, [("EV_Brake".to_string(), "002".to_string())].into());
		assert!(why.contains("vagcan setup <VCDS installation>"), "{why}");
		// A VCDS-only project set up before the log existed, its entry absolute
		// and the installation in place: readable from that entry.
		crate::project::record_source(
			&project,
			crate::project::SourceEntry {
				kind: vag_data_db::VCDS,
				path: root.display().to_string(),
				version: None,
				detail: None,
			},
		)
		.unwrap();
		let expected: BTreeSet<(String, String)> = [("EV_Brake".to_string(), "002".to_string()), ("EV_Engine".to_string(), "001".to_string())].into();
		assert_eq!(
			unread(&project, &identities, |_, _| false).unwrap(),
			Unread::Readable {
				units: expected,
				// Resolved, as `record_source` writes a path.
				root: root.canonicalize().unwrap()
			},
			"the nameless unit is never pending"
		);
		// Once the log says what was tried, the sources are not consulted.
		Log {
			installation: root.clone(),
			units: vec![tried("EV_Engine", "001", "read"), tried("EV_Brake", "002", "no file")],
		}
		.save(&project)
		.unwrap();
		assert_eq!(unread(&project, &identities, |_, _| false).unwrap(), Unread::Nothing);
		// A log that does not read is an error, not a silence.
		std::fs::write(project.registry_log(), "not json").unwrap();
		let err = unread(&project, &identities, |_, _| false).unwrap_err().to_string();
		assert!(err.contains("registry.json"), "{err}");
	}

	#[test]
	fn a_registry_that_does_not_open_is_said_and_every_unit_is_logged_as_stopped_by_it() {
		// A root with no RM.rod, as an install that is not one. Nothing is read,
		// the project's log still names the installation and every unit asked
		// for — with the registry's own reason — so `ensure` does not search for
		// the same thing on every run.
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path().join("not-an-install");
		std::fs::create_dir_all(&root).unwrap();
		let project = project(dir.path());
		std::fs::create_dir_all(&project.dir).unwrap();
		let units: BTreeSet<(String, String)> = [("EV_A".to_string(), "001".to_string()), ("EV_B".to_string(), String::new())].into();
		struct Nothing;
		impl Progress for Nothing {
			fn waiting(&mut self, _: &str) {}
			fn done(&mut self, _: &str) {}
		}
		let outcome = read(&root, &project, &units, &mut Nothing).unwrap();
		let Read::NoRegistry { why, removed } = outcome else {
			panic!("{outcome:?}");
		};
		assert!(why.contains("RM.rod"), "{why}");
		assert_eq!(removed, "", "nothing had been read before");
		let log = Log::load(&project).unwrap().expect("the log is written");
		assert_eq!(log.installation, root.canonicalize().unwrap(), "resolved, as sources.json keeps a path");
		assert_eq!(
			log.units,
			vec![tried("EV_A", "001", "registry: no file"), tried("EV_B", "", "registry: no file")]
		);
		assert!(log.tried("EV_A", "001"), "so it is not tried again until setup reads an installation");
		// And the cache exists, empty of VCDS rows, as `clear` leaves it.
		assert_eq!(vag_data_db::reading_count_of(&project.cache(), vag_data_db::VCDS).unwrap(), 0);
	}

	#[test]
	fn the_detail_names_every_count_that_is_not_zero_and_only_those() {
		let mut summary = Summary {
			asked: 14,
			read: 13,
			written: 2310,
			not_channels: 0,
			unread: 0,
			keys_found: 0,
			keys_unsaved: None,
			notes: Vec::new(),
			cache: PathBuf::from("cache.sqlite"),
		};
		assert_eq!(summary.detail(), "2310 channels for 13 of 14 units");
		summary.not_channels = 5;
		summary.unread = 2;
		summary.keys_found = 1;
		let detail = summary.detail();
		assert!(detail.contains("5 listed rows are not channels"), "{detail}");
		assert!(detail.contains("2 listed rows did not read"), "{detail}");
		assert!(detail.ends_with("1 key found, cached"), "{detail}");
		summary.keys_unsaved = Some("not saved".into());
		assert!(summary.detail().ends_with("1 key found, not saved"));
	}

	#[test]
	fn the_label_directory_is_the_root_or_the_first_child_holding_label_files() {
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path();
		std::fs::create_dir_all(root.join("UDS_EV")).unwrap();
		std::fs::create_dir_all(root.join("Labels")).unwrap();
		assert!(label_dir_under(root).is_err(), "no label files anywhere yet");
		std::fs::write(root.join("Labels").join("part.lbl"), b"").unwrap();
		assert_eq!(label_dir_under(root).unwrap(), root.join("Labels"));
		assert_eq!(
			label_dir_under(&root.join("Labels")).unwrap(),
			root.join("Labels"),
			"the directory itself is fine too"
		);
		assert_eq!(text_page("TTText-RUS.rod"), CodePage::Windows1251);
		assert_eq!(text_page("TTTEXT.ROD"), CodePage::Windows1252);
	}
}
