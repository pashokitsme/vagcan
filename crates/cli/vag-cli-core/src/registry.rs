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
//! fifteen units took five minutes on the reference machine — until 2026-10-20, when the keys
//! turned out to be the file names ([`vag_data_labels::rod::name_product`]): the fifteen read in
//! a fraction of a second now, and a whole installation is still a lot of work for units no car
//! here has. Only the car can say which units
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
//! before the log existed, from its VCDS source in the cache ([`Installation`]).
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
use vag_data_labels::redirect::{self, RedirectLoad, Redirects};
use vag_data_labels::registry::{self, RegistryLoad, RowError, UnitMeasurements, Words};
use vag_data_labels::rod::{IvCache, KeyCost, RodStatus, key_cost, open_section};
use vag_data_labels::ttdop::{self, TablesLoad};
use vag_data_labels::unit_strings::{self, UnitsLoad};

use crate::plan::UnitIdentity;
use crate::project::Project;

mod others;

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

/// The fault text store, one file in the install root beside `Labels/`.
///
/// Same story as [`TEXT_TABLES`]: `Codes.dat` in the English build,
/// `Code-RUS.dat` in the Russian one.
pub const CODES_FILES: &[&str] = &["Codes.dat", "Code-RUS.dat"];

/// The language a VCDS build is in, as ISO 639-2, from the name of its fault
/// text file or its text table — the only place the build says. Recorded on the
/// source the same way an ODIS project's `<LANGUAGE>` is, and read off the pool
/// by `faults`, so the two can be compared.
pub fn codes_language(file_name: &str) -> Option<&'static str> {
	match file_name {
		"Codes.dat" | "TTTEXT.ROD" => Some("eng"),
		"Code-RUS.dat" | "TTText-RUS.rod" => Some("rus"),
		_ => None,
	}
}

/// Which language build a directory holds, named by the file that says so.
///
/// No marker is written for this: a directory that contains `Code-RUS.dat` and
/// `TTText-RUS.rod` *is* the Russian build, and a note beside it saying so
/// would be one more thing to keep in step with what is actually there. Asked
/// of an installation and of the pool `setup` copies it into, which holds one
/// build at a time (`setup` clears it when the build changes).
pub fn build_of(dir: &Path) -> Option<&'static str> {
	CODES_FILES
		.iter()
		.find(|name| dir.join(name).is_file())
		.or_else(|| TEXT_TABLES.iter().find(|name| dir.join(ODX_DIR).join(name).is_file()))
		.copied()
}

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
pub(crate) fn source_of(root: &Path) -> String {
	label_dir_under(root)
		.unwrap_or_else(|_| root.to_path_buf())
		.to_string_lossy()
		.into_owned()
}

/// The VCDS installation root a recorded label directory sits in: the directory itself or
/// one of its two parents, whichever holds [`ODX_DIR`] — the root `setup` takes, two levels
/// at most, as [`label_dir_under`] looks down. `None` when none is on disk.
pub fn installation_holding(labels: &Path) -> Option<PathBuf> {
	labels.ancestors().take(3).find(|dir| dir.join(ODX_DIR).is_dir()).map(Path::to_path_buf)
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
	/// `read`, `no file or redirect`, `no data`, `no list`, `key not found`, `shifted`,
	/// `unresolved`, `mismatched`, or `registry: …` when the registry itself did not open.
	/// `no file` and `no file; ReDir unread` are words of a lookup that did not ask
	/// `ReDir.rod` ([`settled`]).
	pub outcome: String,
	/// The car's project the unit was looked up with (`SK37`), when one was known: a unit
	/// two cars of two projects carry is read once per project.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub project: Option<String>,
	/// The variant name this lookup's rows are stored under — a redirect's source or the
	/// file stem — when its list was read: what a car of this project joins its rows by
	/// ([`car_join`]), so it is never handed another lookup's rows that tie at its rank.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub stem: Option<String>,
}

/// How a car joins its units to the VCDS rows the registry read stored for it: its project
/// (`SK37`, `None` when its model code is not in `chassis.clb`) and, per unit
/// `(F19E, F1A2)`, the stem the lookup made for that project chose ([`Tried::stem`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CarJoin {
	pub project: Option<String>,
	pub stems: BTreeMap<(String, String), String>,
}

/// [`CarJoin`] for a car, out of the project's log and the `chassis.clb` of the
/// installation it names: the car's project from its VIN, and that project's stems.
///
/// A car with no VIN has no project and is never recorded, so it joins by the stems of
/// the lookups made with no project — those of recorded cars whose model code the table
/// does not list. A unit no such car carries has none, and joins by the rank rule.
///
/// `None` with no log, or for a VIN with no table to say its project — the installation
/// gone: the join is then by the rank rule alone, since the stems of a car of no known
/// project are not this car's if it has one.
pub fn car_join(project: &Project, vin: Option<&str>) -> Option<CarJoin> {
	let log = Log::load(project).ok()??;
	let car = match vin {
		Some(vin) => vag_data_labels::chassis::load_projects(&log.installation)?
			.of_vin(vin)
			.map(str::to_string),
		None => None,
	};
	Some(CarJoin {
		stems: stems_for(&log, car.as_deref()),
		project: car,
	})
}

/// The stems the log names for one car project's lookups (`None`: a car of no known
/// project), per unit.
fn stems_for(log: &Log, project: Option<&str>) -> BTreeMap<(String, String), String> {
	log
		.units
		.iter()
		.filter(|t| t.project.as_deref() == project)
		.filter_map(|t| Some(((t.odx_name.clone(), t.odx_version.clone()), t.stem.clone()?)))
		.collect()
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
	/// The [`READ_VERSION`] of the read that wrote it; `0` for a log from before the field.
	#[serde(default)]
	pub read_version: u32,
	#[serde(default)]
	pub units: Vec<Tried>,
}

/// What a read finds and how, as far as the log is concerned. **Bump it to make every
/// recorded unit be read once more:** a log written by an older read counts as having tried
/// nothing ([`Log::is_current`]), so the next car command or `setup` reads every unit again,
/// with no step for anybody to run.
///
/// 1 (2026-10-20): units looked up in VCDS's own order — version and project before the
/// bare name — and through `ReDir.rod`. A machine read before then holds the files the old
/// order picked (`EV_GatewNF_SK26` where VCDS reads `EV_GatewNF_SK37 → EV_GatewNF_VW37`).
///
/// 2 (2026-10-20): each lookup logs the stem its rows are stored under ([`Tried::stem`]),
/// which a car joins by; a version-1 log has none, and its cars would join by rank.
///
/// 3 (2026-10-20): the other registries — every unit's adaptation, basic-setting,
/// output-test, freeze-frame and XPL rows, and their MUX cases, read with its channels
/// (`registry_row`) and stored under the same stem. A read of 2 wrote none.
pub const READ_VERSION: u32 = 3;

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

	/// A log with nothing tried, written by this version of the read.
	pub fn new(installation: PathBuf) -> Log {
		Log {
			installation,
			read_version: READ_VERSION,
			units: Vec::new(),
		}
	}

	/// Whether the read that wrote this log is this one ([`READ_VERSION`]). An older one
	/// counts as having tried nothing.
	pub fn is_current(&self) -> bool {
		self.read_version >= READ_VERSION
	}

	/// Whether this unit has been tried with the recorded installation, for any project,
	/// whatever came of it — see [`Self::tried_with`] for the outcomes that do not count.
	pub fn tried(&self, odx_name: &str, odx_version: &str) -> bool {
		self
			.units
			.iter()
			.any(|t| t.odx_name == odx_name && t.odx_version == odx_version && settled(t, false))
	}

	/// Whether this unit has been tried for this car project (`None`: a car whose project is
	/// not known), whatever came of it — except a "no file" that did not ask `ReDir.rod`:
	/// [`BEFORE_REDIRECTS`] always, and [`REDIRECTS_UNREAD`] when `redirects_open`, the table
	/// opening now ([`redirects_open`]). One still shut is tried: a read would search for the
	/// same key again, minutes, on every car command.
	pub fn tried_with(&self, odx_name: &str, odx_version: &str, project: Option<&str>, redirects_open: bool) -> bool {
		self
			.units
			.iter()
			.any(|t| t.odx_name == odx_name && t.odx_version == odx_version && t.project.as_deref() == project && settled(t, redirects_open))
	}
}

/// Whether what a unit's line in the log says stands, or the unit is worth another read.
fn settled(tried: &Tried, redirects_open: bool) -> bool {
	match tried.outcome.as_str() {
		BEFORE_REDIRECTS => false,
		REDIRECTS_UNREAD => !redirects_open,
		_ => true,
	}
}

/// The word a log written before `ReDir.rod` was followed kept for a unit with no file of
/// its own name. Such a unit may be redirected, so it is read again ([`settled`]).
const BEFORE_REDIRECTS: &str = "no file";

/// The word for a unit with no file of its name when `ReDir.rod` did not open: read again
/// once the table opens, and not before ([`settled`]).
const REDIRECTS_UNREAD: &str = "no file; ReDir unread";

/// The word for a unit with no file of its name and no redirect to one.
const NO_FILE: &str = "no file or redirect";

/// Record the installation without reading anything: for a `setup` run on a
/// machine no car has been recorded on yet, so that the first `watch` knows
/// where to read from.
pub fn record_installation(project: &Project, root: &Path) -> Result<()> {
	Log {
		installation: installation_path(root),
		read_version: READ_VERSION,
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
/// — as the cache keeps a source. One that cannot be resolved is kept as given.
fn installation_path(root: &Path) -> PathBuf {
	std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

/// The word the log keeps for what a unit's list came to.
fn outcome(found: &UnitMeasurements) -> &'static str {
	match found {
		UnitMeasurements::Found { .. } => "read",
		UnitMeasurements::NoFile => NO_FILE,
		UnitMeasurements::NoData { .. } => "no data",
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
	/// Unit `at` (from 0) of `of` is the next whose line is said: `name` is its
	/// ODX name. Its list is found already; a [`Progress::done`] follows for
	/// every one.
	fn unit(&mut self, at: usize, of: usize, name: &str) {
		let _ = (at, of, name);
	}
	/// A handle the units' lookups, running side by side, can count through on
	/// the line the last [`Progress::waiting`] drew. `None` draws no count.
	fn reporter(&self) -> Option<crate::progress::Reporter> {
		None
	}
}

/// The key cache, saved after every table and as each unit's list is found, so
/// an interrupted run keeps what it found, and a save that failed said once rather
/// than dropped. A file under its own name needs no key, so on a genuine
/// installation there is nothing to save.
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
	/// Rows written from the other registries — adaptations, basic settings,
	/// output tests, freeze frames, XPL bits, MUX cases ([`others`]).
	pub other_rows: usize,
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
		let mut detail = format!(
			"{} channel{} for {} of {} unit{}",
			self.written,
			plural(self.written),
			self.read,
			self.asked,
			plural(self.asked)
		);
		if self.not_channels > 0 {
			let (row, channel) = match self.not_channels {
				1 => ("row is", "a channel"),
				_ => ("rows are", "channels"),
			};
			detail.push_str(&format!("; {} listed {row} not {channel}", self.not_channels));
		}
		if self.unread > 0 {
			detail.push_str(&format!("; {} listed row{} did not read", self.unread, plural(self.unread)));
		}
		if self.other_rows > 0 {
			detail.push_str(&format!(
				"; {} rows of adaptations, basic settings, output tests and freeze frames",
				self.other_rows
			));
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
///
/// `None` when the project holds no VCDS source for this installation — never read by
/// `setup`, or dropped with `setup --drop` while this ran — and nothing was written.
pub fn clear(root: &Path, project: &Project) -> Result<Option<String>> {
	let held = vag_data_db::reading_count_of(&project.cache(), vag_data_db::VCDS).unwrap_or(0);
	if vag_data_db::put_all_vcds_readings(&project.cache(), &source_of(root), std::iter::empty())?.is_none() {
		return Ok(None);
	}
	// The other registries' rows go with the channels: they are the same
	// installation's, read in the same run.
	vag_data_db::put_all_vcds_registry_rows(&project.cache(), &source_of(root), std::iter::empty())?;
	Ok(Some(match held {
		0 => String::new(),
		n => format!(
			"\n    This run removed the {n} channel{} an earlier run read from a VCDS registry.",
			if n == 1 { "" } else { "s" }
		),
	}))
}

/// What a read says when the project's VCDS source is not there to write into.
const SOURCE_GONE: &str = "the project holds no source for this VCDS installation — dropped while this ran, or \
                           never read by `vagcan setup` — so nothing was written";

/// Take back the log a read just saved, when its source turned out to be gone: the log
/// names an installation the project no longer holds, and kept, it would send the next
/// read back to it. `--drop` removes it for the same reason.
fn forget_log(project: &Project) {
	match std::fs::remove_file(project.registry_log()) {
		Ok(()) => {}
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
		Err(e) => crate::say!("{} not removed: {e}", project.registry_log().display()),
	}
}

/// Read the registry for `units` — `(F19E, F1A2)` each — out of the
/// installation at `root`, write their channels into the project's cache, and
/// log what came of every unit.
///
/// Every key found is saved after the table it opened, and a unit's as that
/// unit is done, so a run that is interrupted leaves the next one less to search
/// for.
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
		read_version: READ_VERSION,
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
					project: None,
					stem: None,
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
			let Some(removed) = clear(root, project)? else {
				forget_log(project);
				return Ok(Read::NoRegistry {
					why: format!("{why}; {SOURCE_GONE}"),
					removed: String::new(),
				});
			};
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
				let n = tables.unattributed();
				let (rows, they) = if n == 1 { ("row", "it holds") } else { ("rows", "they hold") };
				notes.push(format!(
					"{n} {rows} of TTDOP.rod name{} no table: the states {they} are left out",
					if n == 1 { "s" } else { "" }
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
			notes.push(match other {
				UnitsLoad::NoFile => "this installation has no UNIT.ROD: channels show no unit of measure".to_string(),
				_ => format!("UNIT.ROD did not open ({}): channels show no unit of measure", units_why(&other)),
			});
			unit_strings::UnitStrings::default()
		}
	};
	keys.save();
	progress.waiting("opening ReDir.rod");
	let (redirects, redirects_read) = redirects(root, &mut keys.cache, &mut notes);
	keys.save();
	let projects = projects(root);
	// A unit two recorded cars of two projects carry is two lookups, since VCDS picks its
	// file by the project, and so is one a car of no known project carries beside them;
	// each lookup's rows go by the name it chose.
	let lookups: Vec<(&String, &String, Option<String>)> = units
		.iter()
		.flat_map(|(odx, version)| projects_of(&projects, odx, version).into_iter().map(move |car| (odx, version, car)))
		.collect();
	// The other registries, read for the same units in the same run.
	let mut others = others::Others::open(root, &mut keys, &mut notes, progress);

	// A unit the project's ODIS rows describe is not a gap when this
	// installation has nothing for it: its channels are there already.
	let extracted = crate::extracted::open(project);
	let mut variants: BTreeMap<String, Vec<vag_data_labels::odis::Reading>> = BTreeMap::new();
	let (mut read, mut not_channels, mut unread, mut dropped, mut lossy) = (0usize, 0usize, 0usize, 0usize, 0usize);
	// Every unit's list is found at once, on rayon's pool: each is a walk of the
	// installation and a file or two, and one after another they were most of
	// what this read cost once the keys came from the file names. What came of
	// each is then said in order, as before.
	// Each unit's keys are saved as that unit finishes, so a Ctrl-C keeps the
	// keys of every unit done so far, and the line counts the units done.
	progress.waiting(&format!("finding the measurement lists — 0 of {} units", lookups.len()));
	let line = progress.reporter();
	let car_projects: Vec<Vec<String>> = lookups.iter().map(|(_, _, car)| car.iter().cloned().collect()).collect();
	let wanted: Vec<(&str, &str, vag_data_labels::label_files::Lookup<'_>)> = lookups
		.iter()
		.zip(&car_projects)
		.map(|((odx, version, _), projects)| {
			let lookup = vag_data_labels::label_files::Lookup {
				redirects: &redirects,
				projects,
			};
			(odx.as_str(), version.as_str(), lookup)
		})
		.collect();
	let base = keys.cache.clone();
	let done = std::sync::atomic::AtomicUsize::new(0);
	let shared = std::sync::Mutex::new(&mut keys);
	let lists = registry::units_measurements(root, &wanted, &base, true, &registry, &|found| {
		let finished = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
		if !found.is_empty()
			&& let Ok(mut keys) = shared.lock()
		{
			keys.cache.absorb(found);
			keys.save();
		}
		if let Some(line) = &line {
			line.set(&format!("finding the measurement lists — {finished} of {} units", lookups.len()));
		}
	});
	keys.save();
	// The rest of each unit's work, side by side again: its other lists, from the
	// file its measurement list was found in. Keys saved as each unit finishes.
	progress.waiting(&format!("reading the other registries — 0 of {} units", lookups.len()));
	let line = progress.reporter();
	let base = keys.cache.clone();
	let done = std::sync::atomic::AtomicUsize::new(0);
	let shared = std::sync::Mutex::new(&mut keys);
	let other_lists = others.read_units(root, &wanted, &lists, &registry, &base, &|found| {
		let finished = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
		if !found.is_empty()
			&& let Ok(mut keys) = shared.lock()
		{
			keys.cache.absorb(found);
			keys.save();
		}
		if let Some(line) = &line {
			line.set(&format!("reading the other registries — {finished} of {} units", lookups.len()));
		}
	});
	keys.save();
	for (at, (((odx, version, car), found), other)) in lookups.iter().zip(lists).zip(other_lists).enumerate() {
		let (odx, version) = (*odx, *version);
		// The list is found already; this numbers the unit's line.
		progress.unit(at, lookups.len(), odx);
		crate::wire!(
			"vcds registry: {odx} {version}{}: {}",
			car.as_deref().map(|p| format!(" ({p})")).unwrap_or_default(),
			outcome(&found)
		);
		log.units.push(Tried {
			odx_name: odx.clone(),
			odx_version: version.clone(),
			outcome: match (&found, redirects_read) {
				(UnitMeasurements::NoFile, false) => REDIRECTS_UNREAD,
				_ => outcome(&found),
			}
			.to_string(),
			project: car.clone(),
			stem: None,
		});
		// Whatever came of its measurement list: a unit whose `[MWB]` is shut may
		// still list adaptations. Its rows go under the name its channels would, and a
		// unit with no channels logs that name, so a car joins its other rows by it too.
		let other_stem = others.absorb(other, &words, &units_of, &tables);
		if !matches!(found, UnitMeasurements::Found { .. })
			&& let Some(tried) = log.units.last_mut()
		{
			tried.stem = other_stem;
		}
		let UnitMeasurements::Found { file, rows, via, .. } = found else {
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
		// A redirected unit's rows go by the redirect's source, the name a unit is joined to
		// them by (`extracted::best_variants`); its target is another unit's name.
		let stem = via
			.clone()
			.unwrap_or_else(|| file.file_stem().and_then(|s| s.to_str()).unwrap_or(odx).to_string());
		// What this car project's lookup joins by ([`car_join`]).
		if let Some(tried) = log.units.last_mut() {
			tried.stem = Some(stem.clone());
		}
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
		// A line per unit, in order, whatever order their lists were found in.
		let through = via
			.as_deref()
			.map(|source| format!(", {}", vag_data_labels::label_files::redirect_line(source, &file)))
			.unwrap_or_default();
		progress.done(&format!("{odx}: {} channel{}{through}", channels.len(), plural(channels.len())));
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
	if others.unread > 0 {
		notes.push(format!(
			"{} listed row{} of the other registries did not read",
			others.unread,
			plural(others.unread)
		));
	}
	if others.shut > 0 {
		notes.push(format!(
			"{} list{} of adaptations, basic settings, output tests, freeze frames or XPL bits did not open: \
			 a key not found, or a file only VCDS itself opens",
			others.shut,
			plural(others.shut)
		));
	}
	if let Some(unsaved) = &keys.unsaved {
		notes.push(format!("{unsaved} ({})", keys.path.display()));
	}

	let batch: Vec<(&str, &[vag_data_labels::odis::Reading])> = variants.iter().map(|(v, r)| (v.as_str(), r.as_slice())).collect();
	let (written, other_rows) = match vag_data_db::put_all_vcds_readings(&project.cache(), &source_of(root), batch)? {
		Some(written) => {
			// Into the same source, under the same rule: `None` is a source dropped
			// between the two writes, and its rows went with it.
			let other_rows = vag_data_db::put_all_vcds_registry_rows(
				&project.cache(),
				&source_of(root),
				others.rows.iter().map(|(v, r)| (v.as_str(), r.as_slice())),
			)?
			.unwrap_or(0);
			// After the rows and never before: see `Log`.
			if let Err(e) = log.save(project) {
				notes.push(format!(
					"what was tried was not written down ({e:#}): the next run tries every unit again"
				));
			}
			(written, other_rows)
		}
		None => {
			notes.push(SOURCE_GONE.to_string());
			(0, 0)
		}
	};
	Ok(Read::Done(Summary {
		asked: lookups.len(),
		read,
		written,
		not_channels,
		unread,
		other_rows,
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
	let unnamed = "channels show their reading id, not a name";
	for name in TEXT_TABLES {
		let file = root.join(ODX_DIR).join(name);
		let Ok(bytes) = std::fs::read(&file) else { continue };
		let page = text_page(name);
		let text = open_section(&bytes, name, "TXT", cache, true)
			.filter(|s| matches!(s.status, RodStatus::Tea | RodStatus::Zlib))
			.and_then(|s| s.text);
		if let Some(text) = text {
			// One byte to one char out of the container; the code page is
			// applied to the bytes, as the names step does.
			let raw: Vec<u8> = text.chars().map(|c| c as u32 as u8).collect();
			return (Words::from_table(&vag_data_labels::tttext::read(&raw, page)), page);
		}
		let why = match key_cost(&bytes, "TXT") {
			Some(KeyCost::AnchorSweep) => OPENS_ONLY_IN_VCDS,
			_ => "the search for its key found nothing",
		};
		notes.push(format!("{name} did not open ({why}): {unnamed}"));
		return (Words::default(), page);
	}
	notes.push(format!("none of {TEXT_TABLES:?} is in {}: {unnamed}", root.join(ODX_DIR).display()));
	(Words::default(), CodePage::Windows1252)
}

/// The installation's redirect table, opened once for every unit of the read, and whether
/// it was read: `false` when it did not open, which is said in `notes` — every unit is then
/// looked up by its own name alone, and one with no file is logged [`REDIRECTS_UNREAD`]. An
/// installation with no table at all has nothing to read, and counts as read. Lines of it
/// that did not read are said too.
fn redirects(root: &Path, cache: &mut IvCache, notes: &mut Vec<String>) -> (Redirects, bool) {
	let what = "units VCDS reads through another unit's file are looked up by their own name alone";
	match redirect::load_redirects(root, cache, true) {
		RedirectLoad::Found { redirects, .. } => {
			if redirects.unread() > 0 {
				notes.push(format!(
					"{} lines of ReDir.rod did not read: those redirects are not followed",
					redirects.unread()
				));
			}
			(redirects, true)
		}
		RedirectLoad::NoFile => (Redirects::default(), true),
		RedirectLoad::Locked { .. } => {
			notes.push(format!("ReDir.rod did not open (the search for its key found nothing): {what}"));
			(Redirects::default(), false)
		}
		RedirectLoad::Shifted { .. } => {
			notes.push(format!("ReDir.rod did not open ({OPENS_ONLY_IN_VCDS}): {what}"));
			(Redirects::default(), false)
		}
	}
}

/// Whether the installation's `ReDir.rod` opens now with the keys `setup` keeps, searching
/// for nothing: what decides whether a unit logged [`REDIRECTS_UNREAD`] is worth another
/// read ([`Log::tried_with`]). One that still does not open is not — a read would search
/// for the same key, minutes, on every car command. No table at all opens nothing.
fn redirects_open(root: &Path, project: &Project) -> bool {
	let mut cache = IvCache::load(&project.rod_keys());
	matches!(redirect::load_redirects(root, &mut cache, false), RedirectLoad::Found { .. })
}

/// Per unit `(F19E, F1A2)`, the projects it is looked up with ([`projects`]).
type Projects = BTreeMap<(String, String), Vec<Option<String>>>;

/// The projects a unit is looked up with: those of the cars recorded on this machine that
/// carry it (`SK37`, a car's VIN through the installation's `chassis.clb`,
/// [`vag_data_labels::chassis`]), and `None` for a carrying car whose project is not known —
/// its model code is not in the table, or the installation has no table. A unit no
/// recorded car carries is absent, and looked up with no project ([`projects_of`]).
fn projects(root: &Path) -> Projects {
	let mut out: Projects = BTreeMap::new();
	let table = vag_data_labels::chassis::load_projects(root);
	for car in crate::units::recorded_cars().unwrap_or_default() {
		let project = table.as_ref().and_then(|t| t.of_vin(&car.vin)).map(str::to_string);
		for unit in car.units.iter().filter_map(key) {
			let held = out.entry(unit).or_default();
			if !held.contains(&project) {
				held.push(project.clone());
			}
		}
	}
	out
}

/// One unit's projects out of [`projects`]: one lookup each, and a single one with no
/// project for a unit no recorded car carries.
fn projects_of(projects: &Projects, odx: &str, version: &str) -> Vec<Option<String>> {
	projects
		.get(&(odx.to_string(), version.to_string()))
		.filter(|of| !of.is_empty())
		.cloned()
		.unwrap_or_else(|| vec![None])
}

/// The project codes of the recorded cars, per unit they carry, for a project's logged
/// installation: what `faults` looks a unit's file up with when it has no VIN of its own.
/// Empty with no log, or the installation gone.
pub fn recorded_projects(project: &Project) -> BTreeMap<(String, String), Vec<String>> {
	match Log::load(project) {
		Ok(Some(log)) => projects(&log.installation)
			.into_iter()
			.map(|(unit, of)| (unit, of.into_iter().flatten().collect()))
			.collect(),
		_ => BTreeMap::new(),
	}
}

/// Why a shifted file did not open, in words a reader needs: its mask is the
/// answer the VCDS interface gives to a challenge (cable command `0x45`), which
/// nothing offline computes, and `setup` never runs the hours-long search for it
/// (`.archive/research/labels/tttext2.md` §3.3b).
const OPENS_ONLY_IN_VCDS: &str = "it opens only inside VCDS itself";

fn registry_why(load: &RegistryLoad) -> String {
	match load {
		RegistryLoad::NoFile => "there is no RM.rod in this installation".to_string(),
		RegistryLoad::Locked { file } => format!("the search for {}'s key found nothing", file.display()),
		RegistryLoad::Shifted { file } => format!("{}: {OPENS_ONLY_IN_VCDS}", file.display()),
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
		TablesLoad::Shifted { .. } => OPENS_ONLY_IN_VCDS.to_string(),
		TablesLoad::Found { .. } => String::new(),
	}
}

fn units_why(load: &UnitsLoad) -> String {
	match load {
		UnitsLoad::NoFile => "there is none in this installation".to_string(),
		UnitsLoad::Locked { .. } => "the search for its key found nothing".to_string(),
		UnitsLoad::Shifted { .. } => OPENS_ONLY_IN_VCDS.to_string(),
		UnitsLoad::Found { .. } => String::new(),
	}
}

/// What this installation lacks for a unit, without the advice — for a unit
/// the ODIS project already describes, where there is nothing to do.
fn unit_gap(outcome: &UnitMeasurements) -> &'static str {
	match outcome {
		UnitMeasurements::Found { .. } => "",
		UnitMeasurements::NoFile => "no file of that name in this installation",
		UnitMeasurements::NoData { .. } => "VCDS has no data for it (ReDir.rod sends it to EMPTY)",
		UnitMeasurements::NoList { .. } => "none of its files lists measurements",
		UnitMeasurements::Locked { .. } => "the search for its list's key found nothing",
		UnitMeasurements::Shifted { .. } => "its list opens only inside VCDS itself",
		UnitMeasurements::Unresolved { .. } => "it includes a store this installation does not have",
		UnitMeasurements::Mismatched { .. } => "its list is another install's file",
	}
}

/// Why a unit brought no channels, in a line.
fn unit_why(outcome: &UnitMeasurements) -> String {
	match outcome {
		UnitMeasurements::Found { .. } => String::new(),
		UnitMeasurements::NoFile => "no file of that name in this installation; an ODIS project that describes the unit brings its channels".to_string(),
		UnitMeasurements::NoData { source } => {
			format!("VCDS has no data for it: ReDir.rod sends {source} to EMPTY; an ODIS project that describes the unit brings its channels")
		}
		UnitMeasurements::NoList { candidates: 1 } => "its one file lists no measurements".to_string(),
		UnitMeasurements::NoList { candidates } => format!("none of its {candidates} files lists measurements"),
		UnitMeasurements::Locked { file, section } => {
			format!("the search for the key of [{section}] in {} found nothing", file.display())
		}
		UnitMeasurements::Shifted { file, section } => format!(
			"[{section}] in {} opens only inside VCDS itself; an ODIS project has this unit",
			file.display()
		),
		UnitMeasurements::Unresolved { file } => format!("{} includes a store this installation does not have", file.display()),
		UnitMeasurements::Mismatched { file, listed, outside } => {
			format!(
				"{} lists {outside} of {listed} row{} the registry does not have — another install's file",
				file.display(),
				plural(*listed)
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
///
/// `projects` are the projects a read looks each unit up with ([`projects`]), when the
/// installation is at hand: a unit is pending until it is tried for **each** of them, so a
/// second car of another project with the same unit gets its own read. It is the very map
/// the read expands its lookups from, so what one read logs is what the next asks for.
/// With no installation at hand, any project's line counts. `redirects_open` is whether
/// `ReDir.rod` opens now ([`Log::tried_with`]).
///
/// A log an older read wrote ([`READ_VERSION`]) has tried nothing: with the installation at
/// hand every unit is pending, described or not, so the next read covers them all once.
/// Without it nothing can be read again, and the log stands as it is.
fn pending(
	log: &Log,
	identities: &[UnitIdentity],
	described: impl Fn(&str, &str) -> bool,
	projects: Option<&Projects>,
	redirects_open: bool,
) -> BTreeSet<(String, String)> {
	let stale = !log.is_current() && projects.is_some();
	identities
		.iter()
		.filter_map(key)
		.filter(|(odx, version)| {
			if stale {
				return true;
			}
			if described(odx, version) {
				return false;
			}
			match projects {
				Some(projects) => projects_of(projects, odx, version)
					.iter()
					.any(|project| !log.tried_with(odx, version, project.as_deref(), redirects_open)),
				None => !log.tried(odx, version),
			}
		})
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
/// said nothing (found in review, 2026-09-28) — so the project's VCDS source in the
/// cache, which every `setup` has written, is the fallback: it is the installation
/// when its path is absolute and holds `UDS_EV`. Absolute, because a relative path in
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
/// VCDS sources in the cache, **newest first** ([`crate::project::sources_of_kind`]).
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
	/// Every unit was tried, but by an older read ([`READ_VERSION`]): the cache holds what
	/// it found, and `units` are worth reading again from the installation at `root`. Not a
	/// gap — [`ensure`] reads them once; a build goes ahead on the rows there are.
	Outdated { units: BTreeSet<(String, String)>, root: PathBuf },
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
		read_version: READ_VERSION,
		units: Vec::new(),
	});
	// The projects the read would look each unit up with, and whether a "no file" logged
	// while ReDir.rod was shut is worth a read now — asked only when there is such a line.
	let (projects, open) = match &where_from {
		Installation::At(root) => {
			let open = untried.units.iter().any(|t| t.outcome == REDIRECTS_UNREAD) && redirects_open(root, project);
			(Some(projects(root)), open)
		}
		_ => (None, false),
	};
	// The units never read, as if the log were this read's; then, when there are none, the
	// ones only an older read tried. The second is no gap: their rows are in the cache.
	let current = Log {
		read_version: untried.read_version.max(READ_VERSION),
		..untried.clone()
	};
	let units = pending(&current, identities, &described, projects.as_ref(), open);
	if units.is_empty() {
		let again = pending(&untried, identities, &described, projects.as_ref(), open);
		return Ok(match where_from {
			Installation::At(root) if !again.is_empty() => Unread::Outdated { units: again, root },
			_ => Unread::Nothing,
		});
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
		BEFORE_REDIRECTS | REDIRECTS_UNREAD | NO_FILE => unit_gap(&UnitMeasurements::NoFile).to_string(),
		"no data" => unit_gap(&UnitMeasurements::NoData { source: String::new() }).to_string(),
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
/// not been tried for are read now — the first time only — so the caller's
/// channels then include them. The read is over every unit recorded on this
/// machine plus these (the rows are replaced per kind), and the caller reloads its
/// channels afterwards.
///
/// What it says: one status line at level 0, since the read can be minutes of key
/// searching, and a spinner on a terminal; at `-v` a line per unit as it is done,
/// what the read came to and what it could not read ([`announce`], [`Quietly`]).
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
		crate::say!("the channels of this car's units were not read from the VCDS installation: {e:#}");
	}
}

/// [`ensure`] off the async runtime's worker threads: the read is minutes of
/// CPU when a key has to be searched for, and a worker that long blocked would
/// starve the bus task beside it — and, on the main task, sit between Ctrl-C
/// and the `select!` that acts on it.
pub async fn ensure_async(identities: Vec<UnitIdentity>) {
	if let Err(e) = tokio::task::spawn_blocking(move || ensure(&identities)).await {
		crate::say!("the channels of this car's units were not read from the VCDS installation: {e}");
	}
}

fn try_ensure(identities: &[UnitIdentity]) -> Result<()> {
	// No project is a machine `setup` has not run on; the commands say so
	// themselves, in `missing`'s words, when they find no channels.
	let Ok(project) = crate::project::current() else {
		return Ok(());
	};
	let extracted = crate::extracted::open(&project);
	let (wanted, root, outdated) = match unread(&project, identities, described_by(&extracted))? {
		Unread::Nothing => return Ok(()),
		Unread::Unreadable { units, why } => {
			crate::say!("{}", unreadable(units.len(), &why));
			return Ok(());
		}
		Unread::Readable { units, root } => (units, root, false),
		// An older read tried them: read again once, since what it found may not be what
		// this read finds.
		Unread::Outdated { units, root } => (units, root, true),
	};
	// Every unit recorded on this machine, plus these: the rows are replaced
	// per kind, so a read of these alone would drop every other car's. Loaded
	// before anything is announced, and a record that does not read skips the
	// read rather than lose those rows.
	let cars = match crate::units::recorded_cars() {
		Ok(cars) => cars,
		Err(e) => {
			crate::say!("{}", unreadable(wanted.len(), &format!("they were not read: {e:#}")));
			crate::note!("{WHY_EVERY_CAR}");
			return Ok(());
		}
	};
	let mut units: BTreeSet<(String, String)> = cars.iter().flat_map(|car| car.units.iter()).filter_map(key).collect();
	units.extend(wanted.iter().cloned());
	match outdated {
		true => crate::say!("{}", announce_again(units.len())),
		false => crate::say!("{}", announce(units.len(), wanted.len())),
	}
	crate::note!("vcds registry: {}", root.display());
	let mut progress = Quietly {
		spinner: None,
		said: Vec::new(),
	};
	let outcome = read(&root, &project, &units, &mut progress)?;
	let said = std::mem::take(&mut progress.said);
	drop(progress);
	match outcome {
		Read::Done(summary) => {
			crate::note!("vcds registry: {}", summary.detail());
			// What the read could not do, beyond what the unit lines said already:
			// a table that did not open, a list that lost its first row, keys not
			// saved.
			for note in summary.notes.iter().filter(|note| !said.contains(note)) {
				crate::note!("vcds registry: {note}");
			}
		}
		// What failed, at every level: the channels the line above promised are not coming.
		Read::NoRegistry { why, removed } => {
			crate::say!("the VCDS registry did not open: {why}");
			if !removed.is_empty() {
				crate::note!("vcds registry: {}", removed.trim());
			}
		}
	}
	Ok(())
}

/// The one level-0 line before a read: how many units it covers — every recorded
/// one, since the rows are replaced at once — how many of them are this car's
/// and new, and what the wait can cost.
fn announce(units: usize, wanted: usize) -> String {
	format!(
		"reading the VCDS registry: {units} unit{} ({wanted} of this car new), the first time only",
		plural(units)
	)
}

/// The line for a read made again because an older version of this tool made the last one.
fn announce_again(units: usize) -> String {
	format!(
		"reading the VCDS registry again: {units} unit{}, once, since this version finds their files differently",
		plural(units)
	)
}

/// The line for units of this car whose channels cannot be read: how many, and why
/// — with what to type, where there is something to type.
fn unreadable(n: usize, why: &str) -> String {
	format!("{} {} no channels yet: {why}", units_of_this_car(n), if n == 1 { "has" } else { "have" })
}

/// Why a record of another car that does not read stops the read, at `-v`.
const WHY_EVERY_CAR: &str =
	"vcds registry: a read replaces every car's VCDS channels at once, and one made without that car's units would drop its rows";

/// [`ensure`]'s progress: a spinner while a table opens or a list is found, and
/// at `-v` a line per unit as each is done — the same cause and remedy `setup`
/// gives, since a unit VCDS has no file for is said here once and never searched
/// for again.
struct Quietly {
	spinner: Option<crate::progress::Spinner>,
	/// The unit lines said, so the summary's notes do not say them again.
	said: Vec<String>,
}

impl Progress for Quietly {
	fn waiting(&mut self, what: &str) {
		crate::wire!("vcds registry: {what}");
		match &mut self.spinner {
			Some(spinner) => spinner.update(&format!("reading the VCDS registry — {what}")),
			None => self.spinner = Some(crate::progress::Spinner::new(format!("reading the VCDS registry — {what}"))),
		}
	}

	fn done(&mut self, line: &str) {
		// Cleared before the line is written, or the line lands on the spinner's.
		self.spinner = None;
		crate::note!("vcds registry: {line}");
		self.said.push(line.to_string());
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_build_says_its_language_by_the_files_it_ships() {
		// The pool `faults` reads holds one build; which language it is comes
		// from the files in it, not from the source rows of every build ever set up.
		let base = std::env::temp_dir().join(format!("vagcan-build-of-{}-{:?}", std::process::id(), std::thread::current().id()));
		let _ = std::fs::remove_dir_all(&base);
		std::fs::create_dir_all(base.join(ODX_DIR)).unwrap();
		assert_eq!(build_of(&base), None, "an empty directory is no build");
		std::fs::write(base.join(ODX_DIR).join("TTText-RUS.rod"), b"x").unwrap();
		assert_eq!(build_of(&base).and_then(codes_language), Some("rus"), "the text table alone says it");
		std::fs::write(base.join("Codes.dat"), b"x").unwrap();
		assert_eq!(build_of(&base), Some("Codes.dat"), "the fault text file is asked first");
		assert_eq!(build_of(&base).and_then(codes_language), Some("eng"));
		assert_eq!(codes_language("Code-RUS.dat"), Some("rus"));
		assert_eq!(codes_language("Codes-XYZ.dat"), None);
		let _ = std::fs::remove_dir_all(&base);
	}

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
			project: None,
			stem: None,
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
			UnitMeasurements::NoData { source: "EV_X_017".into() },
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
			read_version: READ_VERSION,
			units: vec![tried("EV_A", "001007", "read"), tried("EV_B", "017001", NO_FILE)],
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
			read_version: READ_VERSION,
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
		let read = &read[..read.find("for (at, (((odx, version, car), found), other))").expect("the unit loop")];
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
			read_version: READ_VERSION,
			units: vec![tried("EV_Tried", "001", NO_FILE)],
		};
		let identities = [
			unit(0x7E0, Some("EV_Tried"), Some("001")),
			unit(0x7E1, Some("EV_Odis"), Some("002")),
			unit(0x713, Some("EV_New"), Some("003")),
			unit(0x714, Some("EV_New"), Some("003")),
			unit(0x715, Some("EV_NoVersion"), None),
			unit(0x746, None, None),
		];
		let wanted = pending(&log, &identities, |odx, _| odx == "EV_Odis", None, false);
		let expected: BTreeSet<(String, String)> = [("EV_New".to_string(), "003".to_string()), ("EV_NoVersion".to_string(), String::new())].into();
		assert_eq!(wanted, expected, "tried, described and nameless units are left out; a unit twice is one");
		// The same unit at another version is another list to find.
		let wanted = pending(&log, &[unit(0x7E0, Some("EV_Tried"), Some("002"))], |_, _| false, None, false);
		assert_eq!(wanted.len(), 1);
		// "No file" from before redirects were followed is tried again: the unit may be one
		// `ReDir.rod` sends to another unit's file. Today's words are not.
		let before = Log {
			installation: PathBuf::from("/vcds"),
			read_version: READ_VERSION,
			units: vec![
				tried("EV_Tried", "001", BEFORE_REDIRECTS),
				tried("EV_Empty", "001", "no data"),
				tried("EV_Unread", "001", REDIRECTS_UNREAD),
				tried("EV_Asked", "001", NO_FILE),
			],
		};
		let identities = [
			unit(0x7E0, Some("EV_Tried"), Some("001")),
			unit(0x7E1, Some("EV_Empty"), Some("001")),
			unit(0x7E2, Some("EV_Unread"), Some("001")),
			unit(0x7E3, Some("EV_Asked"), Some("001")),
		];
		let no_cars = BTreeMap::new();
		let names = |set: BTreeSet<(String, String)>| set.into_iter().map(|(odx, _)| odx).collect::<Vec<_>>();
		assert_eq!(
			names(pending(&before, &identities, |_, _| false, Some(&no_cars), true)),
			["EV_Tried", "EV_Unread"],
			"a lookup that did not ask ReDir.rod is retried once the table opens; one that asked is not"
		);
		// The table still shut: the unit is not read again. A read would search for the same
		// key, minutes, on every car command (found in review, 2026-10-20).
		assert_eq!(names(pending(&before, &identities, |_, _| false, Some(&no_cars), false)), ["EV_Tried"]);
		assert_eq!(names(pending(&before, &identities, |_, _| false, None, true)), ["EV_Tried"]);
		assert_eq!(gap_of(REDIRECTS_UNREAD).as_deref(), Some(unit_gap(&UnitMeasurements::NoFile)));
	}

	/// Two recorded cars of two projects carry one unit: each project is a lookup of its own,
	/// and a line for one does not stand for the other. A car of no known project beside
	/// them is a lookup with no project (found in review, 2026-10-20).
	#[test]
	fn a_unit_is_pending_until_it_is_tried_for_every_project_that_carries_it() {
		let ac = ("EV_InvAC".to_string(), "006145".to_string());
		let tried_for = |project: Option<&str>| Tried {
			project: project.map(str::to_string),
			..tried("EV_InvAC", "006145", "read")
		};
		let log = |units: Vec<Tried>| Log {
			units,
			..Log::new(PathBuf::from("/vcds"))
		};
		let identities = [unit(0x708, Some("EV_InvAC"), Some("006145"))];
		let two: BTreeMap<_, _> = [(ac.clone(), vec![Some("SK37".to_string()), Some("VW48".to_string())])].into();
		let sk37_only = log(vec![tried_for(Some("SK37"))]);
		assert_eq!(
			pending(&sk37_only, &identities, |_, _| false, Some(&two), false).len(),
			1,
			"VW48 not read"
		);
		let both = log(vec![tried_for(Some("SK37")), tried_for(Some("VW48"))]);
		assert!(pending(&both, &identities, |_, _| false, Some(&two), false).is_empty());
		// A car whose model code the table does not list, beside the SK37 car.
		let with_unknown: BTreeMap<_, _> = [(ac.clone(), vec![Some("SK37".to_string()), None])].into();
		assert_eq!(pending(&sk37_only, &identities, |_, _| false, Some(&with_unknown), false).len(), 1);
		assert_eq!(projects_of(&with_unknown, &ac.0, &ac.1), [Some("SK37".to_string()), None]);
		// A unit no recorded car carries is one lookup, with no project.
		assert_eq!(projects_of(&with_unknown, "EV_Other", "001"), [None]);
		let unknown = log(vec![tried_for(None)]);
		assert!(pending(&unknown, &identities, |_, _| false, Some(&BTreeMap::new()), false).is_empty());
	}

	/// Each car project's lookup keeps the stem it stored, and a car joins by its own
	/// project's: a car of no known project gets the no-project lookup's stem, never the
	/// SK37 car's that ties with it.
	#[test]
	fn a_car_joins_by_the_stem_its_own_projects_lookup_stored() {
		let line = |project: Option<&str>, stem: Option<&str>| Tried {
			project: project.map(str::to_string),
			stem: stem.map(str::to_string),
			..tried("EV_InvACC", "006145", "read")
		};
		let log = Log {
			units: vec![
				line(Some("SK37"), Some("EV_InvACC_006_SK37")),
				line(None, Some("EV_InvACC_006_VW27")),
				Tried {
					stem: None,
					..tried("EV_InvGone", "001001", NO_FILE)
				},
			],
			..Log::new(PathBuf::from("/vcds"))
		};
		let unit = ("EV_InvACC".to_string(), "006145".to_string());
		assert_eq!(stems_for(&log, Some("SK37")), [(unit.clone(), "EV_InvACC_006_SK37".to_string())].into());
		assert_eq!(stems_for(&log, None), [(unit, "EV_InvACC_006_VW27".to_string())].into());
		assert!(stems_for(&log, Some("VW48")).is_empty(), "a project nothing was read for joins by rank");
		// A log from before the field reads with no stem.
		let old: Tried = serde_json::from_str(r#"{"odx_name": "EV_A", "outcome": "read"}"#).unwrap();
		assert_eq!(old.stem, None);
	}

	/// The read and the pending check take the cars' projects from the recorded cars, and in a
	/// test build those are the test root's, never the ones under HOME: `recorded_cars` goes
	/// through `datadir`, whose test default is [`crate::datadir::test_root`].
	#[test]
	fn the_recorded_cars_a_test_reads_are_the_test_roots() {
		let cars = crate::datadir::cars_dir().unwrap();
		assert!(cars.starts_with(crate::datadir::test_root()), "{}", cars.display());
		if let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
			assert!(!cars.starts_with(PathBuf::from(home).join(".vagcan")), "{}", cars.display());
		}
	}

	/// A log an older read wrote has tried nothing: every unit is read once more with the
	/// installation at hand, described by ODIS or not — and with the installation gone the
	/// log stands, since nothing could be read again.
	#[test]
	fn a_log_an_older_read_wrote_makes_every_unit_pending_once() {
		let mut log = Log {
			units: vec![tried("EV_InvGatew", "013020", "read")],
			..Log::new(PathBuf::from("/vcds"))
		};
		let identities = [unit(0x710, Some("EV_InvGatew"), Some("013020"))];
		let described = |_: &str, _: &str| true;
		let no_cars = BTreeMap::new();
		assert!(
			pending(&log, &identities, described, Some(&no_cars), false).is_empty(),
			"current: nothing to read"
		);
		log.read_version = READ_VERSION - 1;
		assert_eq!(pending(&log, &identities, described, Some(&no_cars), false).len(), 1);
		assert!(
			pending(&log, &identities, described, None, false).is_empty(),
			"no installation: the log stands"
		);
		// A log from before the field reads as version 0.
		let old: Log = serde_json::from_str(r#"{"installation": "/vcds", "units": []}"#).unwrap();
		assert!(!old.is_current());
		assert!(Log::new(PathBuf::new()).is_current());
	}

	#[test]
	fn a_log_written_before_the_other_registries_brings_one_more_read_and_then_none() {
		// Version 2 read channels and no `registry_row`. Every unit of the car, read and
		// ODIS-described as they are, is read once more; the log that read writes is
		// current, and the next `ensure` reads nothing — whatever the installation has
		// of the other registries, so one with none of them is not read forever.
		assert_eq!(READ_VERSION, 3, "the other registries are version 3");
		let dir = tempfile::tempdir().unwrap();
		let project = project(dir.path());
		std::fs::create_dir_all(&project.dir).unwrap();
		let root = install_at(dir.path());
		let identities = [unit(0x7E0, Some("EV_Engine"), Some("001")), unit(0x713, Some("EV_Brake"), Some("002"))];
		let mut log = Log {
			units: vec![tried("EV_Engine", "001", "read"), tried("EV_Brake", "002", "read")],
			..Log::new(root.clone())
		};
		log.read_version = 2;
		log.save(&project).unwrap();
		let all: BTreeSet<(String, String)> = [("EV_Brake".to_string(), "002".to_string()), ("EV_Engine".to_string(), "001".to_string())].into();
		// Outdated, not unread: a build goes ahead on the rows the cache has (#43), and
		// `ensure` reads them once.
		assert_eq!(
			unread(&project, &identities, |_, _| true).unwrap(),
			Unread::Outdated {
				units: all,
				root: root.clone()
			}
		);
		log.read_version = READ_VERSION;
		log.save(&project).unwrap();
		assert_eq!(unread(&project, &identities, |_, _| true).unwrap(), Unread::Nothing);
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
		// the VCDS source in the cache stands in when it can be used, and when
		// it cannot the line says what to type.
		let dir = tempfile::tempdir().unwrap();
		let root = install_at(dir.path());
		let logged = Log {
			installation: root.clone(),
			read_version: READ_VERSION,
			units: Vec::new(),
		};
		assert_eq!(installation(Some(&logged), &[]), Installation::At(root.clone()), "the log's, first");
		let gone = Log {
			installation: dir.path().join("moved-away"),
			read_version: READ_VERSION,
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
			read_version: READ_VERSION,
			units: vec![tried("EV_Engine", "001", "read"), tried("EV_Brake", "002", NO_FILE)],
		}
		.save(&project)
		.unwrap();
		assert_eq!(unread(&project, &identities, |_, _| false).unwrap(), Unread::Nothing);
		// The same log written by an older read: every unit was tried, so nothing is a gap —
		// a build goes ahead on the rows in the cache — and both are worth reading again.
		let mut old = Log::load(&project).unwrap().unwrap();
		old.read_version = READ_VERSION - 1;
		old.save(&project).unwrap();
		let both: BTreeSet<(String, String)> = [("EV_Brake".to_string(), "002".to_string()), ("EV_Engine".to_string(), "001".to_string())].into();
		assert_eq!(
			unread(&project, &identities, |_, _| false).unwrap(),
			Unread::Outdated {
				units: both,
				root: root.clone()
			}
		);
		// A unit no read ever tried, beside them: that one is the gap, refused as before.
		old.units.retain(|t| t.odx_name == "EV_Engine");
		old.save(&project).unwrap();
		assert_eq!(
			unread(&project, &identities, |_, _| false).unwrap(),
			Unread::Readable {
				units: [("EV_Brake".to_string(), "002".to_string())].into(),
				root: root.clone()
			}
		);
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
		// The installation is one of the project's sources, as setup's label step leaves it.
		vag_data_db::record_language(&project.cache(), vag_data_db::VCDS, &source_of(&root), "eng").unwrap();
		let outcome = read(&root, &project, &units, &mut Nothing).unwrap();
		let Read::NoRegistry { why, removed } = outcome else {
			panic!("{outcome:?}");
		};
		assert!(why.contains("RM.rod"), "{why}");
		assert_eq!(removed, "", "nothing had been read before");
		let log = Log::load(&project).unwrap().expect("the log is written");
		assert_eq!(log.installation, root.canonicalize().unwrap(), "resolved, as the cache keeps a source");
		assert_eq!(
			log.units,
			vec![tried("EV_A", "001", "registry: no file"), tried("EV_B", "", "registry: no file")]
		);
		assert!(log.tried("EV_A", "001"), "so it is not tried again until setup reads an installation");
		// And the cache exists, empty of VCDS rows, as `clear` leaves it.
		assert_eq!(vag_data_db::reading_count_of(&project.cache(), vag_data_db::VCDS).unwrap(), 0);
	}

	#[test]
	fn a_read_whose_source_is_gone_writes_no_rows_no_source_and_no_log() {
		// Review of #37: a `watch` read in flight when `setup --drop vcds` committed wrote
		// its rows, made the source again and rewrote `registry.json`. Both paths of a read
		// — the registry not opening, and the rows written — find the source gone.
		let dir = tempfile::tempdir().unwrap();
		let root = dir.path().join("not-an-install");
		std::fs::create_dir_all(&root).unwrap();
		let project = project(dir.path());
		std::fs::create_dir_all(&project.dir).unwrap();
		// The project holds an ODIS source and no VCDS one: the VCDS one was dropped.
		vag_data_db::record_language(&project.cache(), vag_data_db::ODIS, "/x/SK37X", "deu").unwrap();
		let units: BTreeSet<(String, String)> = [("EV_A".to_string(), "001".to_string())].into();
		struct Nothing;
		impl Progress for Nothing {
			fn waiting(&mut self, _: &str) {}
			fn done(&mut self, _: &str) {}
		}
		let outcome = read(&root, &project, &units, &mut Nothing).unwrap();
		let Read::NoRegistry { why, .. } = outcome else {
			panic!("{outcome:?}");
		};
		assert!(why.contains("nothing was written"), "{why}");
		assert!(!project.registry_log().exists(), "the log names a source the project does not hold");
		let kinds: Vec<String> = crate::project::sources(&project).unwrap().into_iter().map(|s| s.kind).collect();
		assert_eq!(kinds, ["odis"], "the read made a VCDS source");
		assert_eq!(clear(&root, &project).unwrap(), None);
	}

	#[test]
	fn the_detail_names_every_count_that_is_not_zero_and_only_those() {
		let mut summary = Summary {
			asked: 14,
			read: 13,
			written: 2310,
			not_channels: 0,
			unread: 0,
			other_rows: 0,
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
		summary.other_rows = 412;
		assert!(
			summary
				.detail()
				.contains("; 412 rows of adaptations, basic settings, output tests and freeze frames"),
			"{}",
			summary.detail()
		);
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
