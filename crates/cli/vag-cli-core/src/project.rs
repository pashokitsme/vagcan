//! `~/.vagcan/data/<id>/` — one ODIS project's data — and `~/.vagcan/vcds/`, the
//! machine's one VCDS installation.
//!
//! There are two sources, and they are kept by what they describe. An extracted
//! ODIS-Service project describes one **platform** (`SK37X`: every Octavia III, Karoq
//! and Kodiaq), so it is a project, and a machine may hold several. A VCDS installation
//! describes **every** VAG car, so there is one per machine, read for whichever car is
//! connected ([`vcds`]). A command resolves both ([`stores`]) — the car's ODIS project when
//! there is one, and the VCDS store — and then reads **one** of them: the run's source
//! ([`crate::source::run_source`]). The two are never mixed.
//!
//! ```text
//! ~/.vagcan/
//!   rod/                    shared — the raw VCDS files read at run time (.rod, the
//!                           fault text). A property of a VCDS *build*, not of a car.
//!   vcds/                   the VCDS installation `setup` read last
//!     cache.sqlite          its label files and registry rows, and its source row
//!     names.json            text id -> name
//!     odx-ids.json          text id -> the IDE/MAS id that text names
//!     rod-keys.json         recovered .rod section keys
//!     registry.json         which installation the channels came from, per unit
//!   data/
//!     SK37X/
//!       cache.sqlite        the ODIS rows, queryable, and its source row
//!       names-odis.json     text id -> name, pooled from the ODIS project
//!       measurements/       proven-on-car rows, one file per part number
//!   config.toml             which project a bare command means
//! ```
//!
//! The two caches have one schema and are read by one set of code paths: the VCDS store
//! is a [`Project`] whose directory is `vcds/`. VCDS rows an older vagcan wrote into a
//! project's cache are moved into the store the first time either is opened
//! ([`migrate_in`]).
//!
//! **A project is a platform, not a car** (spec §4.1) — which is why the proven rows
//! live in one: a proven scaling is a property of a *part number*, true of every car
//! carrying that part. `cars/<VIN>/` goes on holding what is true of one car and no other.
//!
//! **`measurements/` is the one directory here that nothing can recreate.** Every
//! other file is extracted from somebody else's — a re-parse reproduces it. Those
//! rows were proven by driving a car (`.archive/research/labels/rod-labels.md` §4.0c), and
//! that is why nothing in this module ever deletes a project.
//!
//! **Nothing here invents a name for a car.** An ODIS project names itself and
//! [`vag_data_labels::odis::Project::id`] reads it, or the person does. This module's
//! whole contribution to naming is [`folder_name`], which
//! refuses one that would not be a single child of `data/` — the same rule
//! `datadir::car_folder` applies to a VIN off the bus, and for the same reason:
//! `--project ../../etc` is a path, not a name.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::report::Advice as _;

/// The most a project name may be. A directory name, not a sentence.
const MAX_ID: usize = 64;

/// The environment variable that names the project for one run (D6).
pub const PROJECT_ENV: &str = "VAGCAN_PROJECT";

/// The `--project` flag's answer, for the length of the process.
///
/// A global rather than an argument threaded through every command, for the
/// reason `vag_uds_client::address::install` is one: the flag is parsed once at
/// the top and consulted from a dozen leaves, and passing it down would touch
/// every signature in between to say nothing new.
static SELECTED: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Remember what `--project` said, before any command runs.
pub fn select(id: &str) {
	let _ = SELECTED.set(id.to_owned());
}

/// One store on disk: an ODIS project under `data/`, or the machine's VCDS store
/// ([`vcds`]). Both have the same files under the same names; which of them a store
/// actually holds follows from the source read into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
	pub id: String,
	pub dir: PathBuf,
}

/// Where a command's sources are: the ODIS project this run is about, when there is one, and
/// the machine's VCDS store, which is there whether or not `setup` has read anything into it.
/// A run reads one of the two ([`crate::source::Run`]), never both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stores {
	pub project: Option<Project>,
	pub vcds: Project,
}

impl Stores {
	/// The cache a source of `kind` is kept in: an ODIS project's, or the VCDS store's.
	/// `None` for ODIS with no project.
	pub fn cache_of(&self, kind: &str) -> Option<PathBuf> {
		match kind {
			vag_data_db::VCDS => Some(self.vcds.cache()),
			_ => self.project.as_ref().map(Project::cache),
		}
	}

	/// Where the rows proven on a car are read from: the project's `measurements/`, or
	/// with no project the VCDS store's — the place a VCDS-only project kept them. Nothing
	/// writes either today; a directory that is not there holds no row.
	pub fn measurements_dir(&self) -> PathBuf {
		self.project.as_ref().unwrap_or(&self.vcds).measurements_dir()
	}

	/// The project's id when there is one, else the VCDS store's: what a sentence about
	/// this run's data names.
	pub fn id(&self) -> &str {
		self.project.as_ref().map_or(self.vcds.id.as_str(), |p| p.id.as_str())
	}
}

/// What [`Project::id`] of the VCDS store is, in a sentence.
pub const VCDS_STORE: &str = "VCDS";

/// The machine's VCDS store — `~/.vagcan/vcds/` — whether or not anything is in it.
///
/// Opening it is when VCDS rows an older vagcan left in a project's cache move into it,
/// once per process ([`migrate_in`]).
pub fn vcds() -> Result<Project> {
	let store = vcds_at(&crate::datadir::vcds_dir()?);
	migrate_once();
	Ok(store)
}

/// The VCDS store kept in `dir`.
pub fn vcds_at(dir: &Path) -> Project {
	Project {
		id: VCDS_STORE.to_string(),
		dir: dir.to_path_buf(),
	}
}

/// Whether the VCDS store holds an installation: a source of its kind in its cache. A cache
/// that will not answer is said once and counted as none.
pub fn holds_vcds(store: &Project) -> bool {
	match has_source(store, vag_data_db::VCDS) {
		Ok(held) => held,
		Err(e) => {
			say_unreadable(store, &e, "it is read as holding no VCDS installation");
			false
		}
	}
}

/// What this run reads: [`selected`]'s project, and the VCDS store.
///
/// An error where [`selected`] has one — a project named that is not there, several and
/// none chosen — and where there is nothing at all: no project and no VCDS installation,
/// which is the [`crate::missing::NoLabelData`] a car command offers `setup` for.
pub fn stores() -> Result<Stores> {
	let project = selected()?;
	let vcds = vcds()?;
	if project.is_none() && !holds_vcds(&vcds) {
		return Err(
			crate::missing::NoLabelData::new("nothing has been set up")
				.looked_in(&crate::datadir::vagcan_dir()?)
				.into_error(),
		);
	}
	Ok(Stores { project, vcds })
}

/// [`stores`], or the VCDS store alone where no project resolves — for the readers that
/// degrade rather than fail: a project named wrongly is said once by the command that
/// needs one, and the VCDS rows are worth reading meanwhile.
pub fn stores_or_vcds() -> Option<Stores> {
	match stores() {
		Ok(stores) => Some(stores),
		Err(_) => vcds().ok().map(|vcds| Stores { project: None, vcds }),
	}
}

impl Project {
	/// The label and reading rows, from either source (D1).
	pub fn cache(&self) -> PathBuf {
		self.dir.join("cache.sqlite")
	}

	/// Text id → name, every record of a VCDS installation's `TTTEXT.ROD`.
	pub fn names(&self) -> PathBuf {
		self.dir.join("names.json")
	}

	/// Text id → the ODX id (`IDE#####`, `MAS#####`) that text is the name of,
	/// for the `TTTEXT.ROD` records whose tail says. Written beside
	/// `names.json` by the same read.
	///
	/// A separate file because the two answer different questions: `names.json`
	/// is what a text id says, and this is how a VCDS text joins what ODIS and
	/// a `dash.toml` call the same measurement.
	pub fn odx_ids(&self) -> PathBuf {
		self.dir.join("odx-ids.json")
	}

	/// The recovered `.rod` section keys.
	pub fn rod_keys(&self) -> PathBuf {
		self.dir.join("rod-keys.json")
	}

	/// Which VCDS installation this project's registry channels were read
	/// from, and what came of each unit tried — `crate::registry::Log`.
	pub fn registry_log(&self) -> PathBuf {
		self.dir.join("registry.json")
	}

	/// The proven-on-car rows, one file per part number.
	pub fn measurements_dir(&self) -> PathBuf {
		self.dir.join("measurements")
	}

	/// The provenance log older vagcans kept beside the cache. The `source` table in
	/// [`Project::cache`] is the list of sources now; this file is read once, into it,
	/// and removed ([`sources`]).
	fn legacy_sources(&self) -> PathBuf {
		self.dir.join("sources.json")
	}

	/// Text id → the wording an ODIS project uses for it, pooled.
	///
	/// **Kept apart from `names.json`, which is VCDS's.** They are not
	/// interchangeable, and merging them cost real wording on a real car: an
	/// ODIS reading's own name is the parameter's name *in that ECU variant*,
	/// while the pooled entry is the generic text for the id. Folded together,
	/// `Total_Physical_Wakeup_Events_Counter` and
	/// `Total_Logical_Wakeup_Events_Counter` both became
	/// `Total_CarWakeup_Events_Counter` — two live channels labelled
	/// identically, told apart only by the identifier beside them.
	///
	/// So `names.json` holds what a VCDS installation recovered and nothing
	/// else, and this holds what an ODIS project pooled. A reader naming a
	/// channel wants the first, or the row's own name; the second is a
	/// searchable index of what a text id means, which is a different question.
	pub fn odis_names(&self) -> PathBuf {
		self.dir.join("names-odis.json")
	}
}

/// What `setup` records about one source it read, on that source's row in the
/// cache ([`record_source`]).
///
/// So a person can answer "where did this project's data come from", the way
/// `git log` answers it for code (`vagcan setup --list`), and so a command can ask
/// which kinds of source a project holds ([`has_source`]).
#[derive(Debug, Clone)]
pub struct SourceEntry {
	/// `"vcds"` or `"odis"`.
	pub kind: &'static str,
	/// The folder `setup` was pointed at: a VCDS installation's root, an ODIS
	/// project's folder.
	pub path: String,
	/// Whatever the source calls its own version: a VCDS build's fault-text file
	/// name, or `VWMCD_ProjectVersionInfo` out of `DatabaseVersionInfo.txt`.
	pub version: Option<String>,
	/// One line for anything else worth keeping — the ODIS project's own name,
	/// how many rows landed.
	pub detail: Option<String>,
}

/// Open a project, creating its directory if this is the first source into it.
///
/// **The only thing in this crate that creates a project directory**, so that
/// there is one place to look when a car turns up filed under two names. The
/// caller's obligation is ordering, not location: an ODIS project names itself
/// and that name has to be in hand *before* this is called — see the note on
/// [`crate::setup::source::project_id`].
pub fn open_or_create(id: &str) -> Result<Project> {
	open_or_create_in(&crate::datadir::projects_dir()?, id)
}

/// The rule behind [`open_or_create`], with `data/` passed in so it can be
/// tested without writing into the owner's own.
fn open_or_create_in(projects: &Path, id: &str) -> Result<Project> {
	let name = folder_name(id)?;
	let dir = projects.join(&name);
	std::fs::create_dir_all(&dir).with_context(|| format!("creating the project directory {}", dir.display()))?;
	Ok(Project { id: name, dir })
}

/// Every project on disk, in name order.
pub fn list() -> Result<Vec<String>> {
	Ok(list_in(&crate::datadir::projects_dir()?))
}

/// The rule behind [`list`]. A `data/` that does not exist is not an error —
/// it is a machine where `vagcan setup` has not run, which is the ordinary state
/// of a fresh clone.
fn list_in(projects: &Path) -> Vec<String> {
	let Ok(entries) = std::fs::read_dir(projects) else { return Vec::new() };
	let mut found: Vec<String> = entries
		.flatten()
		.filter(|entry| entry.path().is_dir())
		.filter_map(|entry| entry.file_name().into_string().ok())
		// A directory somebody dropped in by hand under a name this tool would
		// never have written is not a project; listing it would offer a choice
		// that cannot then be selected.
		.filter(|name| folder_name(name).is_ok())
		.collect();
	found.sort();
	found
}

/// The project this run is about.
///
/// In order: `--project`, then `VAGCAN_PROJECT`, then **the project that covers
/// the car in front of the tool**, then `config.toml`, then the only project on
/// disk if there is exactly one. Everything that names a project must name one
/// that already exists — a typo in a flag must not quietly create an empty store
/// and then report that the car has no data.
///
/// **The third step is a seam with nothing plugged into it yet.** Spec §4.1
/// makes a project a *platform*, not a car: `SK37X` covers every Octavia III,
/// Karoq and Kodiaq, so connecting a car has to land on the project covering it
/// rather than on whichever one was last set up. The matching is not built —
/// owner A is establishing how a project declares its own coverage, out of its
/// `.vi` pool (`0.0.0@VI_SK37X.vi.db`), which is the source of truth precisely
/// because a project that describes itself needs no table of type codes compiled
/// into this binary. When that lands, [`covering`] is what fills this in; until
/// then it answers `None` and the order below is what it always was.
///
/// It sits **below** the flag and the environment variable and **above**
/// `config.toml` on purpose. The two overrides are a person saying "this one,
/// I mean it", and nothing read off a bus outranks that. `config.toml` is only a
/// note about the last `setup`, and a car actually present says more than a note
/// does.
pub fn current() -> Result<Project> {
	let projects = crate::datadir::projects_dir()?;
	match selected()? {
		Some(project) => Ok(project),
		None => Err(
			crate::missing::NoLabelData::new("no project has been set up")
				.looked_in(&projects)
				.into_error(),
		),
	}
}

/// [`current`]'s project, or `None` where no project is set up at all — a machine with
/// a VCDS installation alone, or with nothing. An error for everything else
/// [`current`] refuses: a project named that is not there, several and none chosen.
pub fn selected() -> Result<Option<Project>> {
	migrate_once();
	let projects = crate::datadir::projects_dir()?;
	let env = std::env::var(PROJECT_ENV).ok();
	let configured = configured()?;
	selected_in(
		&projects,
		SELECTED.get().map(String::as_str),
		env.as_deref(),
		covering().as_deref(),
		configured.as_deref(),
	)
}

/// The project covering the car in front of the tool, once that can be asked.
///
/// Always `None` today, and deliberately a function rather than a `todo!()` or a
/// commented-out line: it is the one place the answer will come from, it is
/// named from [`current`] so the ordering is already decided and tested, and a
/// reader looking for "where does the car pick its project" finds it here rather
/// than having to be told.
///
/// What goes in it needs two things this crate does not have yet: what the car
/// answered about itself, and a way to ask an installed project whether it
/// covers that. Neither is invented here — the second is `vag_data_labels::odis`'s to
/// provide off the `.vi` pool, and the first arrives from whichever command
/// opened the adapter. The likely shape is the idiom this crate already uses
/// twice ([`select`], `vag_uds_client::address::install`): whoever reads the car
/// installs what it said, and this reads it back.
fn covering() -> Option<String> {
	None
}

/// The rule behind [`current`], with every input passed in.
#[cfg(test)]
fn current_in(projects: &Path, flag: Option<&str>, env: Option<&str>, covering: Option<&str>, configured: Option<&str>) -> Result<Project> {
	match selected_in(projects, flag, env, covering, configured)? {
		Some(project) => Ok(project),
		None => Err(
			crate::missing::NoLabelData::new("no project has been set up")
				.looked_in(projects)
				.into_error(),
		),
	}
}

/// The rule behind [`selected`], with every input passed in.
fn selected_in(projects: &Path, flag: Option<&str>, env: Option<&str>, covering: Option<&str>, configured: Option<&str>) -> Result<Option<Project>> {
	for (given, whose) in [
		(flag, "--project"),
		(env, PROJECT_ENV),
		(covering, "the project covering the car that is connected"),
		(configured, "the project in ~/.vagcan/config.toml"),
	] {
		let Some(id) = given.map(str::trim).filter(|id| !id.is_empty()) else {
			continue;
		};
		let name = folder_name(id).with_context(|| format!("{whose} named {id:?}"))?;
		let dir = projects.join(&name);
		if !dir.is_dir() {
			return Err(anyhow::anyhow!("{whose} names the project {id:?}, and there is no {}", dir.display()).note(which_there_are(projects)));
		}
		return Ok(Some(Project { id: name, dir }));
	}
	match list_in(projects).as_slice() {
		// No project: a machine with a VCDS installation alone, or with nothing. Which
		// of the two is the caller's to say — [`current`] and [`stores`] say it in
		// `missing`'s words, as `NoLabelData` itself: `vag_cli_diag::rescue` recognises
		// the shortage by downcasting to it.
		[] => Ok(None),
		// One project is not a choice, so it is not asked about.
		[only] => Ok(Some(Project {
			id: only.clone(),
			dir: projects.join(only),
		})),
		_ => Err(
			anyhow::anyhow!("more than one project is set up, and nothing says which")
				.note(which_there_are(projects))
				.help("say which with `vagcan --project <id> …`")
				.help("or set it for good: run `vagcan setup` on that project's source again"),
		),
	}
}

/// The projects there actually are, for a message about one there is not.
fn which_there_are(projects: &Path) -> String {
	match list_in(projects).as_slice() {
		[] => format!("nothing is set up under {}", projects.display()),
		found => format!("set up here: {}", found.join(", ")),
	}
}

/// The shared pool of raw VCDS files, created if this is the first one in.
///
/// Shared rather than per project because a `.rod` file is a property of a VCDS
/// **build**, not of a car: the same `TTTEXT.ROD` byte for byte serves every
/// project parsed from that build, and a copy per car would be tens of megabytes
/// each to say the same thing.
pub fn rod_pool() -> Result<PathBuf> {
	let dir = crate::datadir::rod_pool_dir()?;
	std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
	Ok(dir)
}

/// Record one source on its row in the project's cache: where `setup` was pointed,
/// what the source calls its version, one line more, and the time — the row is made
/// if this is the first thing written for it.
///
/// One row per source, not one per run: the *same* source read again is a reread,
/// and its row is restamped where it stands. "The same" is the kind and the
/// directory under the spelling rule the cache keys sources by
/// ([`vag_data_db::normalise_dir`]), so `SK37X` and `SK37X/` are one source. A VCDS
/// installation is keyed by its `Labels` directory, as its rows are
/// ([`crate::registry::label_dir_under`]); `path` is kept as its root.
pub fn record_source(p: &Project, entry: SourceEntry) -> Result<()> {
	adopt_legacy(p);
	let root = vag_data_db::normalise_dir(&entry.path);
	let meta = vag_data_db::Meta {
		// Stored resolved, as the cache stores a directory: a path as typed is only
		// true in the directory it was typed in.
		root: Some(root.clone()),
		version: entry.version,
		detail: entry.detail,
		parsed_at: None,
	};
	std::fs::create_dir_all(&p.dir).with_context(|| format!("creating {}", p.dir.display()))?;
	vag_data_db::record_meta(&p.cache(), entry.kind, &cache_dir(entry.kind, &root), &meta)
		.map_err(|e| anyhow::anyhow!("recording {} in {}: {e}", entry.path, p.cache().display()))
}

/// The directory the cache keys a source of `kind` at `path` by: a VCDS
/// installation's `Labels`, anything else as given.
fn cache_dir(kind: &str, path: &str) -> String {
	match kind {
		vag_data_db::VCDS => crate::registry::source_of(Path::new(path)),
		_ => path.to_string(),
	}
}

/// Every source this project holds, oldest first: the `source` table of its cache.
///
/// Empty when there is no cache — a project nothing has been read into. A cache that
/// will not open is an error, said by the caller.
///
/// A `sources.json` an older vagcan left beside the cache is read into it first,
/// once ([`adopt_legacy`]).
pub fn sources(project: &Project) -> Result<Vec<vag_data_db::Source>> {
	let cache = project.cache();
	if !cache.is_file() {
		return Ok(Vec::new());
	}
	adopt_legacy(project);
	vag_data_db::sources(&cache).map_err(|e| anyhow::anyhow!("reading the sources of {}: {e}", cache.display()))
}

/// The project's sources, or none when they cannot be read — for the questions where
/// "none" is the cautious answer: whether a folder is a reread, where a registry read
/// would come from. Not for [`has_source`].
fn sources_or_none(project: &Project) -> Vec<vag_data_db::Source> {
	sources(project).unwrap_or_default()
}

/// Whether this very source — this kind, this directory under any spelling — is
/// already in the project. Reading it again is then a reread: its rows are
/// replaced, and [`record_source`] restamps its row.
pub fn records_source(project: &Project, kind: &str, path: &str) -> bool {
	let root = vag_data_db::normalise_dir(path);
	let dir = cache_dir(kind, &root);
	sources_or_none(project)
		.iter()
		.any(|s| s.kind == kind && (s.dir == dir || s.meta.root.as_deref() == Some(root.as_str())))
}

/// Whether a source of this kind has been read into this project.
///
/// A file's *content* cannot say which parser produced it, and `names.json` on a
/// project that has only ever seen ODIS runs is ODIS wording under a name that
/// promises VCDS's. The `source` table is what knows. `Ok(false)` when there is no
/// cache: nothing has been read into the project.
///
/// A stale row — a relative path an older vagcan left, which nothing resolves — is
/// not a source of its kind: it can be read from nowhere, and counting it kept a
/// dropped installation's text in use.
///
/// **A cache that will not answer is an `Err`, not a "no"**: one an older vagcan
/// wrote, or one a running `setup` holds locked. Taken as "no source", it turned
/// every fault into a bare number with nothing said. The caller decides what that
/// means for it, and says so once ([`say_unreadable`]).
pub fn has_source(project: &Project, kind: &str) -> Result<bool> {
	Ok(sources(project)?.iter().any(|s| s.kind == kind && !s.is_stale()))
}

/// Say on stderr, once per process, that a project's sources did not read and what
/// is assumed instead. Every command that asks [`has_source`] may meet the same
/// error, and one line is the whole of what a reader needs.
pub fn say_unreadable(project: &Project, why: &anyhow::Error, instead: &str) {
	static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
	if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
		crate::say!("note: the sources of {} did not read ({why:#}); {instead}", project.id);
	}
}

/// Where every source of this kind was read from, **newest read first** — for a
/// VCDS installation its root, which `crate::registry::Installation` reads a unit's
/// channels from when the project's `registry.json` does not name one.
///
/// By `parsed_at`, then by id: a row with no stamp — one an older vagcan wrote and
/// no note of it said when — counts as the oldest. A VCDS row with no root recorded
/// names the installation holding its `Labels` directory
/// ([`crate::registry::installation_holding`]), or the directory itself; a relative
/// one is given as it is, and nothing resolves it.
pub fn sources_of_kind(project: &Project, kind: &str) -> Vec<String> {
	let mut rows: Vec<vag_data_db::Source> = sources_or_none(project).into_iter().filter(|s| s.kind == kind).collect();
	rows.sort_by(|a, b| b.meta.parsed_at.cmp(&a.meta.parsed_at).then(b.id.cmp(&a.id)));
	rows
		.into_iter()
		.map(|s| match (&s.meta.root, s.is_stale()) {
			(Some(root), _) => root.clone(),
			(None, false) if kind == vag_data_db::VCDS => crate::registry::installation_holding(Path::new(&s.dir))
				.map(|root| root.display().to_string())
				.unwrap_or(s.dir),
			(None, _) => s.dir,
		})
		.collect()
}

/// Read a `sources.json` an older vagcan left beside the cache into the cache's
/// `source` table, and remove the file.
///
/// The file was a second list of the same sources and the two disagreed: an entry
/// deleted from it left the source's rows in the cache, still read. So the table
/// wins on *which* sources there are, and the file only fills in what a row does not
/// say yet — where `setup` was pointed, the version, the time
/// ([`vag_data_db::adopt_meta`]). An entry with no row is a source the cache does not
/// hold, and adds none. Entries under a relative path, or for a VCDS installation
/// whose `Labels` row is not under it, match nothing.
///
/// Best effort, and said on stderr when it happened: a file that does not read is
/// removed all the same, since nothing reads it any more — but a cache that will not
/// take the write keeps the file for the next run.
fn adopt_legacy(project: &Project) {
	let path = project.legacy_sources();
	if !path.is_file() || !project.cache().is_file() {
		return;
	}
	let entries = std::fs::read_to_string(&path)
		.ok()
		.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
		.and_then(|value| value.get("sources").and_then(|s| s.as_array()).cloned())
		.unwrap_or_default();
	let Ok(rows) = vag_data_db::sources(&project.cache()) else {
		return;
	};
	for entry in &entries {
		let (Some(kind), Some(given)) = (entry["kind"].as_str(), entry["path"].as_str()) else {
			continue;
		};
		if !Path::new(given).is_absolute() {
			continue;
		}
		let root = vag_data_db::normalise_dir(given);
		let row = rows.iter().find(|s| {
			s.kind == kind
				&& !s.is_stale()
				&& match kind {
					vag_data_db::VCDS => Path::new(&s.dir).starts_with(&root),
					_ => s.dir == root,
				}
		});
		let Some(row) = row else { continue };
		let text = |key: &str| entry[key].as_str().map(str::to_string);
		let meta = vag_data_db::Meta {
			root: Some(root.clone()),
			version: text("version"),
			detail: text("detail"),
			parsed_at: text("parsed_at"),
		};
		if vag_data_db::adopt_meta(&project.cache(), row.id, &meta).is_err() {
			return;
		}
	}
	if std::fs::remove_file(&path).is_ok() {
		crate::say!("note: {} read into {} and removed", path.display(), project.cache().display());
	}
}

/// The files a VCDS installation's read leaves beside its rows, which move with them.
fn vcds_files(store: &Project) -> [PathBuf; 4] {
	[store.names(), store.odx_ids(), store.rod_keys(), store.registry_log()]
}

/// The lock every migration of the store holds from its first look to its last write.
fn migration_lock(store: &Project) -> PathBuf {
	store.dir.join(".migrate.lock")
}

/// A move in progress, beside the store: written before the winner's files are staged, and
/// removed once they are in place and the move is recorded. What it holds — the
/// installation moved, which of its files the project had, the record entry — is what the
/// next run needs to finish a move stopped after its rows committed ([`recover_move`]).
fn move_marker(store: &Project) -> PathBuf {
	store.dir.join(".migrating.json")
}

/// Where a file of the store is staged while a move is in progress: `.names.json.migrating`.
fn staging_of(store: &Project, file: &Path) -> PathBuf {
	let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
	store.dir.join(format!(".{name}.migrating"))
}

/// A project whose VCDS rows are being taken out: written before its files go and kept
/// until [`remove_if_empty`] has had its say, so a run stopped after the rows went — during
/// the VACUUM, say — still removes a project left empty.
fn taken_out_marker(project: &Project) -> PathBuf {
	project.dir.join(".vcds-taken-out")
}

/// What a migration left a record of, beside the store: `migrated.json`, one entry per
/// migration that moved or dropped anything — when, from which projects, which
/// installation was kept and which were replaced. Notes on stderr are lost to a kill or a
/// build script; this is not. `setup --list -v` shows it.
pub fn migration_record(store: &Project) -> PathBuf {
	store.dir.join("migrated.json")
}

/// What one migration did: the lines to say, and the projects it emptied and removed.
#[derive(Debug, Default)]
pub struct Migrated {
	/// One line each, in the words of a step: `moved the installation's data from … to …`.
	/// A line that starts `warning:` is one as it stands.
	pub notes: Vec<String>,
	/// The projects that held a VCDS installation alone and are gone with it.
	pub removed: Vec<String>,
}

/// Whether the notes of this process's migration are kept for its command to say, rather
/// than said when it happens ([`hold_migration_notes`]).
static HOLD_NOTES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static HELD_NOTES: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Keep this process's migration notes for [`take_migration_notes`] instead of saying them
/// when they happen: `setup` says them as steps under its heading.
pub fn hold_migration_notes() {
	HOLD_NOTES.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// The notes [`hold_migration_notes`] kept, once.
pub fn take_migration_notes() -> Vec<String> {
	std::mem::take(&mut *HELD_NOTES.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

/// [`migrate_in`] over this machine's own `data/` and VCDS store, once per process; its
/// notes said on stderr, or kept for the command ([`hold_migration_notes`]). A project it
/// removed is no longer what `config.toml` names.
fn migrate_once() {
	static DONE: std::sync::Once = std::sync::Once::new();
	DONE.call_once(|| {
		let (Ok(projects), Ok(store)) = (crate::datadir::projects_dir(), crate::datadir::vcds_dir()) else {
			return;
		};
		let migrated = migrate_in(&projects, &vcds_at(&store));
		if let Ok(Some(named)) = configured()
			&& migrated.removed.contains(&named)
		{
			let _ = crate::config::clear_project();
		}
		if HOLD_NOTES.load(std::sync::atomic::Ordering::Relaxed) {
			HELD_NOTES.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).extend(migrated.notes);
			return;
		}
		for note in migrated.notes {
			match note.starts_with("warning:") {
				true => crate::say!("{note}"),
				false => crate::say!("note: {note}"),
			}
		}
	});
}

/// One project's VCDS installation, as [`migrate_in`] weighs it.
struct Held {
	project: Project,
	/// When it was read: the newest of its VCDS rows that a path resolves. `None` is older
	/// than any stamp.
	parsed_at: Option<String>,
	/// Where it was read from, to name it.
	path: String,
}

/// The newest VCDS installation a store's or a project's cache holds: its stamp and its
/// path. `None` when it holds none; an error when its sources do not read.
fn newest_vcds(store: &Project) -> Result<Option<(Option<String>, String)>> {
	let rows = sources(store)?;
	let vcds: Vec<&vag_data_db::Source> = rows.iter().filter(|s| s.kind == vag_data_db::VCDS).collect();
	let newest = vcds
		.iter()
		.filter(|s| !s.is_stale())
		.max_by(|a, b| a.meta.parsed_at.cmp(&b.meta.parsed_at))
		.or(vcds.first())
		.copied();
	Ok(newest.map(|s| (s.meta.parsed_at.clone(), s.path().to_string())))
}

/// Whether anything is there to migrate: a project's cache holding VCDS rows, or a
/// project holding a VCDS file the store lacks. Asked without the lock, to keep every open
/// after the first free of it; [`migrate_in`] asks again under it.
fn anything_to_migrate(projects: &Path) -> bool {
	list_in(projects).into_iter().any(|id| {
		let project = Project { dir: projects.join(&id), id };
		let holds_vcds = project.cache().is_file() && newest_vcds(&project).map_or(true, |held| held.is_some());
		holds_vcds || taken_out_marker(&project).is_file() || vcds_files(&project).iter().any(|file| file.is_file())
	})
}

/// Move the VCDS installation an older vagcan read into a project into the machine's
/// VCDS store, with its files, and take every project's VCDS rows out of its cache. What
/// it did, as notes; nothing when no project holds VCDS rows, which is every open after
/// the first.
///
/// **One at a time.** The whole of it runs under one lock between processes
/// ([`vag_data_db::exclusive`] on [`migration_lock`]): two commands started together — two
/// firmware builds, `watch` beside `faults` — once both decided to move, and the second
/// copied the first's emptied project over the full store.
///
/// **Which installation stays** is the one read last, by `parsed_at` — the store's own
/// included — since a second VCDS installation read replaces the first. Every other is
/// named, the store's own among them. Every project keeps its ODIS rows; a project that
/// held VCDS alone is removed once empty, its `measurements/` moved into the store.
///
/// **Crash-safe in its order**: the winner's files are staged in the store, its rows
/// copied in one transaction counted before it commits ([`vag_data_db::copy_kind`]), and
/// only then are the files renamed into place; then the projects' files go, and last their
/// rows ([`vag_data_db::drop_kind`]), each only while the store holds an installation read
/// no earlier than the project's. Stopped anywhere, the next open finishes it.
pub fn migrate_in(projects: &Path, store: &Project) -> Migrated {
	let mut out = Migrated::default();
	if !anything_to_migrate(projects) && !move_marker(store).is_file() {
		return out;
	}
	let _lock = match vag_data_db::exclusive(&migration_lock(store), std::time::Duration::from_secs(120)) {
		Ok(lock) => lock,
		Err(e) => {
			out.notes.push(format!(
				"warning: the VCDS store is busy ({e}); the VCDS rows in the projects stay there until the next run"
			));
			return out;
		}
	};
	// A move a stopped run began is finished, or undone, before anything is weighed.
	if let Some(note) = recover_move(store) {
		out.notes.push(note);
	}
	let mut held: Vec<Held> = Vec::new();
	let mut bare: Vec<Project> = Vec::new();
	for id in list_in(projects) {
		let project = Project { dir: projects.join(&id), id };
		if !project.cache().is_file() {
			bare.push(project);
			continue;
		}
		match newest_vcds(&project) {
			Ok(Some((parsed_at, path))) => held.push(Held { project, parsed_at, path }),
			Ok(None) => bare.push(project),
			Err(e) => out.notes.push(format!(
				"warning: the sources of {} did not read ({e:#}); its VCDS rows, if any, stay there",
				project.id
			)),
		}
	}
	// Newest first; a tie goes to the project first in name order.
	held.sort_by(|a, b| b.parsed_at.cmp(&a.parsed_at).then(a.project.id.cmp(&b.project.id)));
	let in_store = match store.cache().is_file() {
		false => None,
		true => match newest_vcds(store) {
			Ok(found) => found,
			Err(e) => {
				out.notes.push(format!(
					"warning: the VCDS store {} did not read ({e:#}); the VCDS rows in the projects stay there",
					crate::datadir::tilde(&store.cache())
				));
				return out;
			}
		},
	};
	// What this migration does, decided and recorded before anything moves: the record
	// goes out with the move ([`move_into`]) or, with nothing to move, before any project
	// loses a row — a stop after either keeps it.
	let mut record = serde_json::Map::new();
	let mut said = Vec::new();
	let winner = held.first().filter(|w| in_store.as_ref().is_none_or(|(stamp, _)| w.parsed_at > *stamp));
	if let Some(winner) = winner {
		record.insert(
			"moved".into(),
			serde_json::json!({ "from": winner.project.dir, "installation": winner.path, "read": winner.parsed_at }),
		);
	}
	// Every installation not kept, the store's own among them when this one replaced it.
	let mut replaced: Vec<serde_json::Value> = Vec::new();
	if winner.is_some()
		&& let Some((stamp, path)) = &in_store
	{
		said.push(format!(
			"replaced the installation {} the store held, read {}",
			crate::datadir::tilde(Path::new(path)),
			stamp.as_deref().unwrap_or("at an unknown time")
		));
		replaced.push(serde_json::json!({ "from": store.dir, "installation": path, "read": stamp }));
	}
	for h in held.iter().skip(usize::from(winner.is_some())) {
		// The store's own, stamped alike: the copy a stopped run already made.
		if in_store.as_ref().is_some_and(|(stamp, _)| *stamp == h.parsed_at) {
			continue;
		}
		said.push(format!(
			"dropped the installation {} from {}, read {}: the one read last is kept",
			crate::datadir::tilde(Path::new(&h.path)),
			h.project.id,
			h.parsed_at.as_deref().unwrap_or("at an unknown time")
		));
		replaced.push(serde_json::json!({ "from": h.project.dir, "installation": h.path, "read": h.parsed_at }));
	}
	if !replaced.is_empty() {
		record.insert("replaced".into(), serde_json::Value::Array(replaced));
	}
	if !record.is_empty() {
		record.insert("at".into(), serde_json::Value::String(chrono::Utc::now().to_rfc3339()));
	}
	match winner {
		Some(winner) => {
			if let Err(e) = move_into(&winner.project, store, &record) {
				out.notes.push(format!(
					"warning: the VCDS installation in {} did not move to {} ({e:#}); it stays there until the next run",
					crate::datadir::tilde(&winner.project.dir),
					crate::datadir::tilde(&store.dir)
				));
				return out;
			}
			out.notes.push(format!(
				"moved the installation's data from {} to {}",
				crate::datadir::tilde(&winner.project.dir),
				crate::datadir::tilde(&store.dir)
			));
		}
		None if !record.is_empty() => {
			if let Err(e) = append_record(store, serde_json::Value::Object(record)) {
				out.notes.push(format!("warning: the record of this migration was not written ({e:#})"));
			}
		}
		None => {}
	}
	out.notes.extend(said);
	// Under the same lock, and only while the store holds an installation read no earlier
	// than the project's: what is taken out is in the store.
	let kept = match store.cache().is_file() {
		true => newest_vcds(store).ok().flatten(),
		false => None,
	};
	for h in &held {
		if kept.as_ref().is_none_or(|(stamp, _)| *stamp < h.parsed_at) {
			out.notes.push(format!(
				"warning: the VCDS rows in {} stay there: the store holds no installation read since",
				crate::datadir::tilde(&h.project.cache())
			));
			continue;
		}
		// The store holds this very installation (stamped alike): a file of it the store
		// lacks is the project's to give — a move stopped before its files were in place
		// left them only in the project. Until the store has it, the project keeps it all.
		if kept.as_ref().is_some_and(|(stamp, _)| *stamp == h.parsed_at)
			&& let Some(missing) = give_missing_files(&h.project, store)
		{
			out.notes.push(format!(
				"warning: the VCDS rows in {} stay there: {} did not reach the store",
				crate::datadir::tilde(&h.project.cache()),
				crate::datadir::tilde(&missing)
			));
			continue;
		}
		match take_out(&h.project) {
			Ok(vacuumed) => {
				if let Err(e) = vacuumed {
					out.notes.push(format!(
						"note: {} keeps the space its VCDS rows held ({e}); they are gone",
						crate::datadir::tilde(&h.project.cache())
					));
				}
				if let Some(note) = settle_taken_out(&h.project, store, &mut out.removed) {
					out.notes.push(note);
				}
			}
			Err(e) => out.notes.push(format!(
				"warning: the VCDS rows in {} were not removed ({e:#}); the next run removes them",
				crate::datadir::tilde(&h.project.cache())
			)),
		}
	}
	// The files a project kept with no VCDS rows — `--drop vcds` keeps the keys and the
	// names — go to the store where it lacks them, and out of the project.
	for project in &bare {
		if let Some(note) = adopt_files(project, store) {
			out.notes.push(note);
		}
		// Its VCDS rows went in a run stopped before it was settled: an empty project is
		// removed now, as that run would have.
		if taken_out_marker(project).is_file()
			&& let Some(note) = settle_taken_out(project, store, &mut out.removed)
		{
			out.notes.push(note);
		}
	}
	out
}

/// After a project's VCDS rows went: [`remove_if_empty`], and the project's marker gone
/// once nothing is left to retry — the project removed, or kept for its other sources.
fn settle_taken_out(project: &Project, store: &Project, removed: &mut Vec<String>) -> Option<String> {
	let note = remove_if_empty(project, store, removed);
	let retry = note.as_deref().is_some_and(|n| n.starts_with("warning:"));
	if project.dir.is_dir() && !retry {
		let _ = std::fs::remove_file(taken_out_marker(project));
	}
	note
}

/// Copy into the store each VCDS file of `project` the store lacks. The first that does not
/// arrive is returned, and the rest are not tried.
fn give_missing_files(project: &Project, store: &Project) -> Option<PathBuf> {
	for (from, to) in vcds_files(project).into_iter().zip(vcds_files(store)) {
		if from.is_file() && !to.is_file() && crate::datadir::copy_file(&from, &to).is_err() {
			return Some(to);
		}
	}
	None
}

/// Finish, or undo, a move a stopped run began ([`move_marker`]). Its rows committed — the
/// store's newest installation is the one the marker names — the staged files are renamed
/// into place and the move recorded; otherwise the staged files are removed and the next
/// step makes the move again. Staged files with no marker (a vagcan before the marker) are
/// removed: the project still holds what they copied, and gives it again
/// ([`give_missing_files`]).
fn recover_move(store: &Project) -> Option<String> {
	let marker = move_marker(store);
	let Some(moving) = std::fs::read_to_string(&marker)
		.ok()
		.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
	else {
		let _ = std::fs::remove_file(&marker);
		for file in vcds_files(store) {
			let _ = std::fs::remove_file(staging_of(store, &file));
		}
		return None;
	};
	let committed = store.cache().is_file()
		&& newest_vcds(store)
			.ok()
			.flatten()
			.is_some_and(|(stamp, path)| stamp.as_deref() == moving["read"].as_str() && path == moving["installation"].as_str().unwrap_or_default());
	if !committed {
		for file in vcds_files(store) {
			let _ = std::fs::remove_file(staging_of(store, &file));
		}
		let _ = std::fs::remove_file(&marker);
		return None;
	}
	Some(match finish_move(store, &moving) {
		Ok(()) => format!(
			"finished moving the installation {} into {}",
			crate::datadir::tilde(Path::new(moving["installation"].as_str().unwrap_or("?"))),
			crate::datadir::tilde(&store.dir)
		),
		Err(e) => format!(
			"warning: a stopped move into {} is not finished ({e:#}); the next run finishes it",
			crate::datadir::tilde(&store.dir)
		),
	})
}

/// The last half of a move, after its rows committed: each staged file renamed into place,
/// a file the project did not have removed from the store, the move recorded once, the
/// marker gone. Every step is one a second pass makes again safely.
fn finish_move(store: &Project, moving: &serde_json::Value) -> Result<()> {
	let had: Vec<&str> = moving["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()).collect();
	for to in vcds_files(store) {
		let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
		let staging = staging_of(store, &to);
		match had.contains(&name.as_str()) {
			true if staging.is_file() => std::fs::rename(&staging, &to).with_context(|| format!("replacing {}", to.display()))?,
			// Renamed already, by the run that stopped.
			true => {}
			false => match std::fs::remove_file(&to) {
				Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
					return Err(e).with_context(|| format!("removing {}", to.display()));
				}
				_ => {}
			},
		}
	}
	if let Some(entry) = moving.get("record").filter(|r| r.as_object().is_some_and(|r| !r.is_empty())) {
		let recorded = std::fs::read_to_string(migration_record(store))
			.ok()
			.and_then(|text| serde_json::from_str::<Vec<serde_json::Value>>(&text).ok())
			.is_some_and(|entries| entries.iter().any(|e| e["at"] == entry["at"]));
		if !recorded {
			append_record(store, entry.clone())?;
		}
	}
	std::fs::remove_file(move_marker(store)).with_context(|| format!("removing {}", move_marker(store).display()))
}

/// Add one entry to [`migration_record`].
fn append_record(store: &Project, entry: serde_json::Value) -> Result<()> {
	let path = migration_record(store);
	let mut entries: Vec<serde_json::Value> = std::fs::read_to_string(&path)
		.ok()
		.and_then(|text| serde_json::from_str(&text).ok())
		.unwrap_or_default();
	entries.push(entry);
	crate::datadir::replace_file(&path, serde_json::to_string_pretty(&entries)?.as_bytes())
}

/// The winner's half of [`migrate_in`]: the move marked, its files staged in the store,
/// its rows copied, and only then the files renamed into place and `record` recorded. A
/// file it lacks is removed from the store, where it would describe the installation
/// replaced. A copy that fails leaves the store's files and rows as they were; a run
/// stopped after the copy committed is finished by the next ([`recover_move`]).
fn move_into(project: &Project, store: &Project, record: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
	let moving = begin_move(project, store, record)?;
	finish_move(store, &moving)
}

/// [`move_into`] up to its rows committed: the marker written, the files staged, the rows
/// copied. What it returns is the marker's content, for [`finish_move`].
fn begin_move(project: &Project, store: &Project, record: &serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value> {
	std::fs::create_dir_all(&store.dir).with_context(|| format!("creating {}", store.dir.display()))?;
	let (read, installation) = newest_vcds(project)?.with_context(|| format!("{} holds no VCDS installation", project.cache().display()))?;
	let pairs: Vec<(PathBuf, PathBuf)> = vcds_files(project).into_iter().zip(vcds_files(store)).collect();
	let had: Vec<String> = pairs
		.iter()
		.filter(|(from, _)| from.is_file())
		.filter_map(|(_, to)| to.file_name().map(|n| n.to_string_lossy().into_owned()))
		.collect();
	let moving = serde_json::json!({ "installation": installation, "read": read, "files": had, "record": record });
	crate::datadir::replace_file(&move_marker(store), serde_json::to_string_pretty(&moving)?.as_bytes())?;
	let undo = || {
		for (_, to) in &pairs {
			let _ = std::fs::remove_file(staging_of(store, to));
		}
		let _ = std::fs::remove_file(move_marker(store));
	};
	for (from, to) in pairs.iter().filter(|(from, _)| from.is_file()) {
		if let Err(e) = crate::datadir::copy_file(from, &staging_of(store, to)) {
			undo();
			return Err(e);
		}
	}
	if let Err(e) = vag_data_db::copy_kind(&project.cache(), &store.cache(), vag_data_db::VCDS) {
		undo();
		anyhow::bail!(
			"copying the VCDS rows of {} to {}: {e}",
			project.cache().display(),
			store.cache().display()
		);
	}
	Ok(moving)
}

/// The projects' half of [`migrate_in`]: the project marked ([`taken_out_marker`]), its VCDS
/// files, then its VCDS rows. In that order, so a stop between the two leaves rows to find,
/// and the next run finishes it. The cache's VACUUM after them is its own result: one that
/// fails leaves the rows gone, and is a note.
fn take_out(project: &Project) -> Result<std::result::Result<(), vag_data_db::Error>> {
	std::fs::write(taken_out_marker(project), b"").with_context(|| format!("writing {}", taken_out_marker(project).display()))?;
	for file in vcds_files(project) {
		match std::fs::remove_file(&file) {
			Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
				return Err(e).with_context(|| format!("removing {}", file.display()));
			}
			_ => {}
		}
	}
	let gone = vag_data_db::drop_kind(&project.cache(), vag_data_db::VCDS)
		.map_err(|e| anyhow::anyhow!("removing the VCDS rows of {}: {e}", project.cache().display()))?;
	Ok(match gone.total() {
		0 => Ok(()),
		_ => vag_data_db::vacuum(&project.cache()),
	})
}

/// A project emptied by the migration — it held a VCDS installation and nothing else — is
/// no project: its `measurements/` move into the store's, and its directory goes. `None`,
/// and nothing touched, while it holds a source or a file the migration does not know.
fn remove_if_empty(project: &Project, store: &Project, removed: &mut Vec<String>) -> Option<String> {
	if !sources(project).is_ok_and(|rows| rows.is_empty()) {
		return None;
	}
	let known = |name: &str| {
		name == "cache.sqlite" || name.starts_with("cache.sqlite-") || name == "measurements" || name == "names-odis.json" || name == ".vcds-taken-out"
	};
	let entries: Vec<std::fs::DirEntry> = std::fs::read_dir(&project.dir).ok()?.flatten().collect();
	if !entries.iter().all(|e| known(&e.file_name().to_string_lossy())) {
		return None;
	}
	let measurements = project.measurements_dir();
	let mut moved = 0usize;
	if let Ok(files) = std::fs::read_dir(&measurements) {
		let into = store.measurements_dir();
		if std::fs::create_dir_all(&into).is_err() {
			return None;
		}
		for file in files.flatten() {
			let to = into.join(file.file_name());
			if to.exists() || crate::datadir::copy_file(&file.path(), &to).is_err() {
				// Kept where it is rather than lost: the project stays.
				return Some(format!(
					"warning: {} stays: {} was not moved",
					crate::datadir::tilde(&project.dir),
					crate::datadir::tilde(&file.path())
				));
			}
			moved += 1;
		}
	}
	std::fs::remove_dir_all(&project.dir).ok()?;
	removed.push(project.id.clone());
	Some(match moved {
		0 => format!("removed the project {}, which held the VCDS installation alone", project.id),
		n => format!(
			"removed the project {}, which held the VCDS installation alone; its {n} measured {} moved to {}",
			project.id,
			if n == 1 { "row file" } else { "row files" },
			crate::datadir::tilde(&store.measurements_dir())
		),
	})
}

/// A project's VCDS files with no VCDS rows beside them: each moved into the store where
/// it lacks one — the keys merged, the store's own winning — and removed from the project.
fn adopt_files(project: &Project, store: &Project) -> Option<String> {
	let mut moved = Vec::new();
	for (from, to) in vcds_files(project).into_iter().zip(vcds_files(store)) {
		if !from.is_file() {
			continue;
		}
		let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
		let taken = match (to.is_file(), name == "rod-keys.json") {
			(false, _) => std::fs::create_dir_all(&store.dir).is_ok() && crate::datadir::copy_file(&from, &to).is_ok(),
			(true, true) => merge_keys(&from, &to).is_ok(),
			(true, false) => true,
		};
		if taken && std::fs::remove_file(&from).is_ok() {
			moved.push(name);
		}
	}
	(!moved.is_empty()).then(|| {
		format!(
			"moved {} from {} to {}",
			moved.join(crate::marks::SEP),
			crate::datadir::tilde(&project.dir),
			crate::datadir::tilde(&store.dir)
		)
	})
}

/// Fold the keys in `from` into `to`: what `to` holds already wins.
fn merge_keys(from: &Path, to: &Path) -> Result<()> {
	let read = |path: &Path| -> serde_json::Map<String, serde_json::Value> {
		std::fs::read_to_string(path)
			.ok()
			.and_then(|text| serde_json::from_str(&text).ok())
			.unwrap_or_default()
	};
	let mut keys = read(to);
	for (key, value) in read(from) {
		keys.entry(key).or_insert(value);
	}
	crate::datadir::replace_file(to, serde_json::to_string_pretty(&keys)?.as_bytes())
}
/// What `config.toml` says this machine's project is, if it says.
pub fn configured() -> Result<Option<String>> {
	Ok(crate::config::project(&crate::config::load()))
}

/// Write down which project a bare command means from now on.
///
/// Read-modify-write of the whole settings document rather than an overwrite —
/// see [`crate::config`]: that file holds the language, the favourites and
/// whatever somebody has written in it by hand, and this owns exactly one key.
pub fn remember(id: &str) -> Result<()> {
	let name = folder_name(id)?;
	crate::config::set_project(&name)
}

/// A project id, checked hard enough to be one child of `data/`.
///
/// Nothing is sanitised into shape. A name is typed by a person or read out of a
/// project's own `index.xml`, and quietly mangling either one files a car under
/// a name no later run would reproduce — which loses it as surely as writing
/// outside the directory would. `datadir::car_folder` refuses a VIN off the bus
/// for the same reason, in the same words.
pub fn folder_name(id: &str) -> Result<String> {
	let id = id.trim();
	if id.is_empty() {
		bail!("a project name with nothing in it is not a name");
	}
	if id == "." || id == ".." {
		bail!("{id:?} already names a folder: the one above, or the one it is in");
	}
	if id.chars().count() > MAX_ID {
		bail!(
			"{id:?} is {} characters, and {MAX_ID} is the most a project name may be",
			id.chars().count()
		);
	}
	if let Some(bad) = id.chars().find(|c| !allowed(*c)) {
		bail!("{id:?} holds {bad:?}, and a project name is letters, digits, `-`, `_` and `.`");
	}
	Ok(id.to_owned())
}

/// Whether one character may be in a project name.
///
/// Public because `setup::source` asks the same question interactively and used
/// to carry its own copy — with the doc comment here saying the two "match",
/// which is a promise a comment cannot keep. One name is legal or it is not,
/// and the command line and the menu must not be able to disagree about which.
pub fn allowed(c: char) -> bool {
	c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A throwaway `data/` — nothing here may write into the owner's own,
	/// which is why every rule above has an `_in` form.
	fn temp() -> tempfile::TempDir {
		tempfile::tempdir().expect("a temporary directory")
	}

	#[test]
	fn a_project_holds_every_file_one_car_needs_and_all_of_them_under_itself() {
		let here = temp();
		let p = open_or_create_in(here.path(), "SK37X").unwrap();
		assert_eq!(p.id, "SK37X");
		assert!(p.dir.is_dir(), "the directory is created on first open");
		for path in [p.cache(), p.names(), p.odx_ids(), p.rod_keys(), p.measurements_dir(), p.registry_log()] {
			assert_eq!(path.parent(), Some(p.dir.as_path()), "{path:?} is not in the project");
		}
	}

	#[test]
	fn opening_the_same_project_twice_is_the_same_project() {
		// Design §5: a second source is added to a project, not given its own.
		let here = temp();
		let first = open_or_create_in(here.path(), "SK37X").unwrap();
		let second = open_or_create_in(here.path(), "SK37X").unwrap();
		assert_eq!(first, second);
		assert_eq!(list_in(here.path()), ["SK37X"]);
	}

	#[test]
	fn a_name_that_is_a_path_never_gets_to_choose_where_this_tool_writes() {
		// `--project` is an argument, and an argument is not trusted more than a
		// VIN off the bus is. Sanitising instead would file a car under a name
		// no later run could reproduce.
		for not_a_name in ["../../etc", "a/b", "a\\b", "", "   ", ".", "..", "my car", "SK37X!"] {
			assert!(folder_name(not_a_name).is_err(), "{not_a_name:?} was accepted");
		}
		let here = temp();
		assert!(open_or_create_in(here.path(), "../escape").is_err());
		assert!(!here.path().parent().unwrap().join("escape").exists(), "it wrote outside data/");
	}

	#[test]
	fn every_directory_under_data_is_a_project_whatever_it_is_called() {
		// `extracted` and `measured` were the layout before projects existed,
		// and that layout is gone (2026-10-20). Nothing reserves the two names
		// any more and nothing walks them: a directory under `data/` is a
		// project, and a file there is nothing at all.
		for name in ["extracted", "measured", "Extracted", "MEASURED", "SK37X"] {
			assert_eq!(folder_name(name).unwrap(), name);
		}
		let here = temp();
		std::fs::create_dir_all(here.path().join("extracted")).unwrap();
		std::fs::write(here.path().join("extracted").join("names.json"), b"{}").unwrap();
		std::fs::write(here.path().join("measured"), b"not a directory").unwrap();
		assert_eq!(list_in(here.path()), ["extracted"]);
		// One project is not a choice, so a bare command lands on it, and a flag
		// names it like any other.
		assert_eq!(current_in(here.path(), None, None, None, None).unwrap().id, "extracted");
		assert_eq!(current_in(here.path(), Some("extracted"), None, None, None).unwrap().id, "extracted");
		// A second project beside it makes two, and a bare command asks.
		open_or_create_in(here.path(), "SK37X").unwrap();
		assert_eq!(list_in(here.path()), ["SK37X", "extracted"]);
		assert!(current_in(here.path(), None, None, None, None).is_err());
	}

	#[test]
	fn a_name_the_picker_would_offer_is_a_name_the_store_accepts() {
		// The two halves of one rule live in two modules — `setup::source` asks
		// and this stores — so a name that survives the prompt must survive here
		// or a person is told yes and then no.
		for name in ["SK37X", "default", "SK-37X-copy", "a.b_c-1", &"x".repeat(MAX_ID)] {
			assert_eq!(folder_name(name).unwrap(), name);
		}
		assert!(folder_name(&"x".repeat(MAX_ID + 1)).is_err());
	}

	#[test]
	fn nothing_set_up_says_to_run_setup_rather_than_naming_a_file() {
		let here = temp();
		let why = current_in(here.path(), None, None, None, None).unwrap_err();
		let why = crate::report::render(&why, 0, crate::report::Colour::Never);
		assert!(why.contains("vagcan setup"), "{why}");
	}

	#[test]
	fn one_project_is_not_a_choice_and_is_not_asked_about() {
		let here = temp();
		open_or_create_in(here.path(), "SK37X").unwrap();
		assert_eq!(current_in(here.path(), None, None, None, None).unwrap().id, "SK37X");
	}

	#[test]
	fn with_several_projects_and_nothing_selected_it_names_them_and_says_how_to_choose() {
		let here = temp();
		open_or_create_in(here.path(), "SK37X").unwrap();
		open_or_create_in(here.path(), "default").unwrap();
		let why = current_in(here.path(), None, None, None, None).unwrap_err();
		let why = crate::report::render(&why, 0, crate::report::Colour::Never);
		assert!(why.contains("SK37X") && why.contains("default"), "{why}");
		assert!(why.contains("--project"), "{why}");
	}

	#[test]
	fn the_flag_beats_the_environment_beats_the_config_file() {
		// D6's order, and the reason it is that order: a flag is this run, an
		// environment variable is this shell, a config file is this machine.
		let here = temp();
		for id in ["flagged", "envd", "configured"] {
			open_or_create_in(here.path(), id).unwrap();
		}
		assert_eq!(
			current_in(here.path(), Some("flagged"), Some("envd"), None, Some("configured"))
				.unwrap()
				.id,
			"flagged"
		);
		assert_eq!(current_in(here.path(), None, Some("envd"), None, Some("configured")).unwrap().id, "envd");
		assert_eq!(current_in(here.path(), None, None, None, Some("configured")).unwrap().id, "configured");
	}

	#[test]
	fn a_connected_car_outranks_the_config_file_and_loses_to_the_two_overrides() {
		// Spec §4.1: a project is a platform, and connecting a car has to land
		// on the project covering it rather than on whichever one was last set
		// up. The matching is owner A's and not built — the ordering it will
		// plug into is decided and pinned here, so landing it is a one-line
		// change to `covering()` rather than a decision made under pressure.
		let here = temp();
		for id in ["flagged", "envd", "onthebus", "configured"] {
			open_or_create_in(here.path(), id).unwrap();
		}
		// A car present says more than a note about the last `setup` does.
		assert_eq!(
			current_in(here.path(), None, None, Some("onthebus"), Some("configured")).unwrap().id,
			"onthebus"
		);
		// It says less than a person who typed which one they meant.
		assert_eq!(
			current_in(here.path(), None, Some("envd"), Some("onthebus"), Some("configured"))
				.unwrap()
				.id,
			"envd"
		);
		assert_eq!(
			current_in(here.path(), Some("flagged"), None, Some("onthebus"), Some("configured"))
				.unwrap()
				.id,
			"flagged"
		);
	}

	#[test]
	fn a_car_whose_project_is_not_installed_says_which_car_it_could_not_place() {
		// The message has to name the step, or "there is no …/SK37X" reads as a
		// typo somebody made rather than as a project they have not set up.
		let here = temp();
		open_or_create_in(here.path(), "AU21X").unwrap();
		let why = crate::report::render(
			&current_in(here.path(), None, None, Some("SK37X"), None).unwrap_err(),
			0,
			crate::report::Colour::Never,
		);
		assert!(why.contains("covering the car that is connected"), "{why}");
		assert!(why.contains("SK37X") && why.contains("AU21X"), "{why}");
	}

	#[test]
	fn a_selected_project_that_is_not_there_is_an_error_and_not_a_new_one() {
		// A typo in a flag must not create an empty store and then report that
		// the car has no data — the second message would be true and useless.
		let here = temp();
		open_or_create_in(here.path(), "SK37X").unwrap();
		let why = crate::report::render(
			&current_in(here.path(), Some("SK37Y"), None, None, None).unwrap_err(),
			0,
			crate::report::Colour::Never,
		);
		assert!(why.contains("SK37Y"), "{why}");
		assert!(why.contains("SK37X"), "it names the ones that are there: {why}");
		assert!(!here.path().join("SK37Y").exists(), "it created the project it was complaining about");
	}

	#[test]
	fn a_directory_nobody_could_have_created_is_not_offered_as_a_project() {
		let here = temp();
		std::fs::create_dir_all(here.path().join("not a project")).unwrap();
		open_or_create_in(here.path(), "SK37X").unwrap();
		assert_eq!(list_in(here.path()), ["SK37X"]);
	}

	#[test]
	fn every_source_is_kept_rather_than_the_last_one() {
		// Two sources merge into one cache.sqlite and the rows stop saying which
		// run wrote them. A log that kept only the last would answer the one
		// question it exists for with half the truth.
		let here = temp();
		let p = open_or_create_in(here.path(), "SK37X").unwrap();
		record_source(
			&p,
			SourceEntry {
				kind: "vcds",
				path: "/Applications/VCDS".into(),
				version: Some("Codes.dat".into()),
				detail: None,
			},
		)
		.unwrap();
		record_source(
			&p,
			SourceEntry {
				kind: "odis",
				path: "/Users/x/Downloads/SK37X".into(),
				version: Some("2610.2.688".into()),
				detail: Some("54 variants".into()),
			},
		)
		.unwrap();

		let rows = sources(&p).unwrap();
		assert_eq!(rows.len(), 2, "{rows:?}");
		assert_eq!(rows[0].kind, "vcds");
		assert_eq!(rows[1].meta.version.as_deref(), Some("2610.2.688"));
		assert_eq!(rows[1].meta.detail.as_deref(), Some("54 variants"));
		assert!(rows[1].meta.parsed_at.as_deref().is_some_and(|t| t.contains('T')), "{rows:?}");
		assert!(!p.legacy_sources().exists(), "no second list is written beside the cache");
	}

	#[test]
	fn reading_the_same_source_again_replaces_its_entry_rather_than_appending_a_copy() {
		// The defect: every `setup` run appended, so a project set up three times
		// from one folder listed that folder three times. A second read of a
		// source is a reread — the entry is that source's latest, in one row —
		// and a trailing slash is the same folder, as it is in `cache.sqlite`.
		let here = temp();
		let project_dir = here.path().join("SK37X-download");
		std::fs::create_dir_all(&project_dir).unwrap();
		let p = open_or_create_in(here.path(), "SK37X").unwrap();
		let entry = |path: String, version: &str| SourceEntry {
			kind: "odis",
			path,
			version: Some(version.into()),
			detail: None,
		};
		let vcds = SourceEntry {
			kind: "vcds",
			path: "/Applications/VCDS".into(),
			version: None,
			detail: None,
		};
		record_source(&p, entry(project_dir.display().to_string(), "2610.2.688")).unwrap();
		record_source(&p, vcds).unwrap();
		record_source(&p, entry(format!("{}/", project_dir.display()), "2610.2.700")).unwrap();

		let rows = sources(&p).unwrap();
		assert_eq!(rows.len(), 2, "one folder read twice is one source: {rows:?}");
		let odis: Vec<&vag_data_db::Source> = rows.iter().filter(|row| row.kind == "odis").collect();
		assert_eq!(odis.len(), 1, "{rows:?}");
		assert_eq!(odis[0].meta.version.as_deref(), Some("2610.2.700"), "the row is the latest read");
		assert!(rows.iter().any(|row| row.kind == "vcds"), "another source's row stays");

		// And it is known to be there before it is read a third time.
		assert!(records_source(&p, "odis", &format!("{}/", project_dir.display())));
		assert!(records_source(&p, "vcds", "/Applications/VCDS"));
		assert!(!records_source(&p, "odis", "/Applications/VCDS"), "the kind is part of the source");
		assert!(!records_source(&p, "vcds", "/Applications/VCDS-other"));
	}

	#[test]
	fn the_sources_of_a_kind_come_newest_first_whatever_their_place_in_the_file() {
		// A reread lands where its entry was, so the file's order is the order
		// sources were first read in; what the registry needs is the one read
		// last (found in review, 2026-09-28).
		let here = temp();
		let p = open_or_create_in(here.path(), "SK37X").unwrap();
		let vcds = |path: &str| SourceEntry {
			kind: "vcds",
			path: path.into(),
			version: None,
			detail: None,
		};
		record_source(&p, vcds("/Applications/VCDS-old")).unwrap();
		std::thread::sleep(std::time::Duration::from_millis(5));
		record_source(&p, vcds("/Applications/VCDS-new")).unwrap();
		std::thread::sleep(std::time::Duration::from_millis(5));
		// The old one read again: its entry stays first in the file, and is the newest.
		record_source(&p, vcds("/Applications/VCDS-old")).unwrap();
		assert_eq!(
			sources_of_kind(&p, "vcds"),
			vec!["/Applications/VCDS-old".to_string(), "/Applications/VCDS-new".to_string()]
		);
		assert!(sources_of_kind(&p, "odis").is_empty());
		assert!(has_source(&p, "vcds").unwrap() && !has_source(&p, "odis").unwrap());
	}

	#[test]
	fn a_source_given_as_a_relative_path_is_recorded_where_it_resolves() {
		// `vagcan setup SK37X` from `~/Downloads` stored `"path": "SK37X"`, and
		// the next setup — from anywhere else — resolved that against its own
		// directory, missed the entry and appended a second copy of the folder.
		let here = temp();
		let p = open_or_create_in(here.path(), "SK37X").unwrap();
		record_source(
			&p,
			SourceEntry {
				kind: "odis",
				path: ".".into(),
				version: None,
				detail: None,
			},
		)
		.unwrap();
		let rows = sources(&p).unwrap();
		assert!(!rows[0].is_stale(), "stored as typed: {rows:?}");
		assert!(Path::new(rows[0].path()).is_absolute(), "{rows:?}");
	}

	/// A project as an older vagcan left it: a cache, and a `sources.json` beside it
	/// naming the VCDS installation by its root while the cache keys it by `Labels`.
	fn with_legacy_log(here: &Path) -> (Project, PathBuf, PathBuf) {
		let p = open_or_create_in(here, "SK37X").unwrap();
		let install = here.join("vcds-en");
		let odis = here.join("Downloads").join("SK37X");
		std::fs::create_dir_all(install.join("Labels")).unwrap();
		std::fs::write(install.join("Labels").join("index.lbl"), b"001,1,x,,").unwrap();
		std::fs::create_dir_all(&odis).unwrap();
		let (install, odis) = (install.canonicalize().unwrap(), odis.canonicalize().unwrap());
		vag_data_db::record_language(&p.cache(), vag_data_db::VCDS, &install.join("Labels").to_string_lossy(), "eng").unwrap();
		vag_data_db::record_language(&p.cache(), vag_data_db::ODIS, &odis.to_string_lossy(), "deu").unwrap();
		let log = serde_json::json!({ "sources": [
			{ "kind": "vcds", "path": install, "version": "Codes.dat", "parsed_at": "2026-10-20T07:26:16.756324+00:00" },
			{ "kind": "odis", "path": "/nowhere/else", "version": "2610.2.688" },
			{ "kind": "vcds", "path": "vendor/vcds-en" },
		]});
		std::fs::write(p.legacy_sources(), log.to_string()).unwrap();
		(p, install, odis)
	}

	#[test]
	fn a_sources_json_left_beside_the_cache_is_read_into_it_once_and_removed() {
		// The defect this replaces: the owner deleted the ODIS entry from `sources.json`
		// and `faults` went on naming codes from ODIS, because the rows were in the
		// cache. The cache's table is the list now; the file only fills in what a row
		// does not say, and a source it names that the cache does not hold adds none.
		let here = temp();
		let (p, install, odis) = with_legacy_log(here.path());
		let rows = sources(&p).unwrap();
		assert!(!p.legacy_sources().exists(), "read once and removed");
		assert_eq!(rows.len(), 2, "the file adds no source and removes none: {rows:?}");
		let vcds = rows.iter().find(|s| s.kind == "vcds").unwrap();
		assert_eq!(
			vcds.path(),
			install.to_string_lossy(),
			"the root the file kept, beside the cache's Labels"
		);
		assert_eq!(vcds.meta.version.as_deref(), Some("Codes.dat"));
		assert_eq!(vcds.meta.parsed_at.as_deref(), Some("2026-10-20T07:26:16.756324+00:00"));
		let odis_row = rows.iter().find(|s| s.kind == "odis").unwrap();
		assert_eq!(odis_row.dir, odis.to_string_lossy(), "the ODIS source the file had lost is still there");
		assert_eq!(
			odis_row.meta,
			vag_data_db::Meta::default(),
			"and nothing of another folder's entry is put on it"
		);
		assert!(has_source(&p, "odis").unwrap() && has_source(&p, "vcds").unwrap());
		// The installation the registry falls back on is the root, from the table alone.
		assert_eq!(sources_of_kind(&p, "vcds"), vec![install.display().to_string()]);
	}

	#[test]
	fn a_vcds_source_with_no_root_recorded_names_the_installation_holding_its_labels() {
		// A cache whose `sources.json` was deleted by hand: the row has only `Labels`.
		let here = temp();
		let p = open_or_create_in(here.path(), "SK37X").unwrap();
		let install = here.path().join("vcds-en");
		std::fs::create_dir_all(install.join(crate::registry::ODX_DIR)).unwrap();
		std::fs::create_dir_all(install.join("Labels")).unwrap();
		let install = install.canonicalize().unwrap();
		let labels = install.join("Labels");
		vag_data_db::record_language(&p.cache(), vag_data_db::VCDS, &labels.to_string_lossy(), "eng").unwrap();
		assert_eq!(sources_of_kind(&p, "vcds"), vec![install.display().to_string()]);
		// And one under a relative path is given as it is, for the caller to refuse.
		vag_data_db::record_language(&p.cache(), vag_data_db::ODIS, "vendor/odis", "deu").unwrap();
		assert_eq!(sources_of_kind(&p, "odis"), vec!["vendor/odis".to_string()]);
	}

	/// A project cache at today's schema holding both sources, as the reference machine's
	/// was: an ODIS project's channels and faults, and a VCDS installation's label files,
	/// registry channels (one with states) and the files its read leaves beside the cache.
	/// The VCDS root is `<here>/<install>`.
	fn with_both(here: &Path, id: &str, install: &str) -> (Project, PathBuf) {
		use vag_data_labels::{Level, Scaling};
		let p = open_or_create_in(&here.join("data"), id).unwrap();
		let root = here.join(install);
		let labels = root.join("Labels");
		std::fs::create_dir_all(&labels).unwrap();
		std::fs::write(labels.join("index.lbl"), b"001,1,Engine Speed,(G28),Range: 0...6500 RPM\n002,2,Coolant,,").unwrap();
		let root = root.canonicalize().unwrap();
		let labels = root.join("Labels");
		let odis = here.join("Downloads").join(id);
		std::fs::create_dir_all(&odis).unwrap();
		let odis = odis.canonicalize().unwrap().display().to_string();
		let reading = |did: u16, scaling: Scaling| vag_data_labels::odis::Reading {
			did,
			name: format!("r{did}"),
			unit: None,
			bit_offset: 0,
			bit_length: 8,
			signed: false,
			big_endian: true,
			scaling,
			text_id: Some(format!("IDE{did:05}")),
		};
		let linear = Scaling::Linear(vag_data_labels::measure::LinearScale { factor: 1.0, offset: 0.0 });
		let states = Scaling::Enum {
			levels: vec![Level::point(0, "off"), Level::point(1, "on")],
		};
		let odis_rows = [reading(1, linear.clone()), reading(2, states.clone())];
		vag_data_db::put_all_readings(&p.cache(), &odis, [("EV_ECM", &odis_rows[..])]).unwrap();
		// Schema 5: the project's adaptation channels and its `adaptations_at`, ODIS's alone.
		vag_data_db::put_all_adaptations(&p.cache(), &odis, [("EV_ECM", &odis_rows[..1])]).unwrap();
		let fault = vag_data_labels::odis::Fault {
			dop: "DTCDOP_VAGUDS".into(),
			code: 297,
			display_code: Some("B1168F2".into()),
			text: Some("brake".into()),
			text_id: None,
			short_name: None,
			level: 1,
			temporary: false,
		};
		vag_data_db::put_all_faults(&p.cache(), &odis, [("EV_ECM", std::slice::from_ref(&fault))]).unwrap();
		// #53: ODIS's freeze-frame multiplexer is `registry_row` rows under the ODIS source.
		let odis_frame = vag_data_labels::odis::EnvironmentRecord {
			range: Some((0x70, 0x70)),
			name: None,
			length: Some(1),
			fields: vec![vag_data_labels::odis::EnvironmentField {
				name: Some("odis speed".into()),
				text_id: None,
				unit: None,
				bit_offset: Some(0),
				bit_length: Some(8),
				signed: false,
				big_endian: true,
				scaling: Some(linear.clone()),
				constant: false,
			}],
		};
		vag_data_db::put_all_odis_freeze_frames(&p.cache(), &odis, [("EV_ECM", std::slice::from_ref(&odis_frame))]).unwrap();
		record_source(
			&p,
			SourceEntry {
				kind: vag_data_db::ODIS,
				path: odis.clone(),
				version: None,
				detail: None,
			},
		)
		.unwrap();
		vag_data_db::build_db(&labels, &p.cache()).unwrap();
		record_source(
			&p,
			SourceEntry {
				kind: vag_data_db::VCDS,
				path: root.display().to_string(),
				version: Some("Codes.dat".into()),
				detail: None,
			},
		)
		.unwrap();
		let vcds_rows = [reading(3, linear), reading(4, states)];
		vag_data_db::put_all_vcds_readings(&p.cache(), &labels.to_string_lossy(), [("EV_ECM_001", &vcds_rows[..])])
			.unwrap()
			.unwrap();
		// VCDS's own freeze-frame row (`RF.rod [FFMUX]`), in the same table as ODIS's.
		let vcds_frame = vag_data_labels::registries::Described {
			section: vag_data_db::FREEZE_FRAME_SECTION,
			entry: vag_data_labels::registries::Entry {
				row: 1,
				key: 0x70,
				identifier: None,
				ordinal: None,
				lead: Some(104),
				case: None,
				value: vag_data_labels::registries::Value {
					kind: Some(0),
					big_endian: true,
					bit_offset: Some(0),
					bit_length: Some(8),
					..Default::default()
				},
				raw: String::new(),
			},
			name: Some("vcds speed".into()),
			text_id: None,
			unit: None,
			levels: Vec::new(),
		};
		vag_data_db::put_all_vcds_registry_rows(&p.cache(), &labels.to_string_lossy(), [("EV_ECM_001", std::slice::from_ref(&vcds_frame))]).unwrap();
		for (file, text) in [
			(p.names(), format!(r#"{{"000001": "{install}"}}"#)),
			(p.odx_ids(), r#"{"000001": "IDE00003"}"#.to_string()),
			(p.rod_keys(), "{}".to_string()),
			(
				p.registry_log(),
				format!(r#"{{"installation": "{}", "read_version": 3, "units": []}}"#, root.display()),
			),
		] {
			std::fs::write(file, text).unwrap();
		}
		(p, root)
	}

	/// Rows per kind of table in a cache: label files, VCDS channels, ODIS channels, faults.
	fn counts(cache: &Path) -> (u64, u64, u64, u64) {
		let (by_source, _) = vag_data_db::source_rows(cache).unwrap();
		let sources = vag_data_db::sources(cache).unwrap();
		let of = |kind: &str| -> vag_data_db::Rows {
			let mut rows = vag_data_db::Rows::default();
			for s in sources.iter().filter(|s| s.kind == kind) {
				let held = by_source.get(&s.id).copied().unwrap_or_default();
				rows.label_files += held.label_files;
				rows.readings += held.readings;
				rows.faults += held.faults;
			}
			rows
		};
		let (vcds, odis) = (of(vag_data_db::VCDS), of(vag_data_db::ODIS));
		(vcds.label_files, vcds.readings, odis.readings, odis.faults)
	}

	#[test]
	fn a_project_holding_both_sources_gives_its_vcds_half_to_the_machine_store() {
		// The reference machine: SK37X with ODIS (2) and VCDS (3) in one v3 cache.
		let here = temp();
		let (p, root) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));
		let before = counts(&p.cache());
		assert_eq!(before, (1, 2, 2, 1));
		let source = sources(&p).unwrap().into_iter().find(|s| s.kind == vag_data_db::VCDS).unwrap();

		let notes = migrate_in(&here.path().join("data"), &store).notes;
		assert_eq!(notes.len(), 1, "{notes:?}");
		assert!(notes[0].starts_with("moved the installation's data from "), "{notes:?}");
		let _ = &root;

		// The store holds the installation, its rows and its files, as the project held them.
		let moved = sources(&store).unwrap();
		assert_eq!(moved.len(), 1, "{moved:?}");
		assert_eq!(
			(moved[0].kind.as_str(), &moved[0].dir, &moved[0].meta),
			(vag_data_db::VCDS, &source.dir, &source.meta)
		);
		assert_eq!(counts(&store.cache()), (1, 2, 0, 0));
		assert_eq!(vag_data_db::load_db(&store.cache()).unwrap().len(), 1, "the label file reads back whole");
		let channels = vag_data_db::readings_of_kind(&store.cache(), vag_data_db::VCDS, "EV_ECM_001").unwrap();
		assert_eq!(channels.len(), 2);
		assert!(
			channels
				.iter()
				.any(|r| matches!(&r.scaling, vag_data_labels::Scaling::Enum { levels } if levels.len() == 2)),
			"the states came along"
		);
		assert!(std::fs::read_to_string(store.names()).unwrap().contains("vcds-en"));
		for file in vcds_files(&store) {
			assert!(file.is_file(), "{file:?} did not move");
		}
		// The project keeps its ODIS rows, its adaptations among them, and nothing of VCDS.
		assert_eq!(counts(&p.cache()), (0, 0, 2, 1));
		assert_eq!(vag_data_db::adaptation_variants(&p.cache()).unwrap(), ["EV_ECM"]);
		assert!(vag_data_db::adaptation_variants(&store.cache()).unwrap().is_empty());
		assert!(
			vag_data_db::odis_sources_without_adaptations(&p.cache()).unwrap().is_empty(),
			"adaptations_at stayed"
		);
		// `registry_row` holds both kinds since #53: ODIS's freeze-frame rows stay in the
		// project, VCDS's go to the store, and neither side gets the other's.
		let frames = |cache: &Path, kind: &str| vag_data_db::freeze_frame_variants(cache, kind).unwrap();
		assert_eq!(frames(&p.cache(), vag_data_db::ODIS), ["EV_ECM"]);
		assert!(frames(&p.cache(), vag_data_db::VCDS).is_empty());
		assert_eq!(frames(&store.cache(), vag_data_db::VCDS), ["EV_ECM_001"]);
		assert!(frames(&store.cache(), vag_data_db::ODIS).is_empty());
		assert!(vag_data_db::registry_row_counts(&store.cache(), vag_data_db::ODIS).unwrap().is_empty());
		assert!(vag_data_db::registry_row_counts(&p.cache(), vag_data_db::VCDS).unwrap().is_empty());
		assert!(!has_source(&p, vag_data_db::VCDS).unwrap() && has_source(&p, vag_data_db::ODIS).unwrap());
		for file in vcds_files(&p) {
			assert!(!file.exists(), "{file:?} stayed in the project");
		}
		// Once: the next open finds nothing to move, and says nothing.
		assert!(migrate_in(&here.path().join("data"), &store).notes.is_empty());
		assert_eq!(counts(&store.cache()), (1, 2, 0, 0));
	}

	#[test]
	fn of_two_projects_with_vcds_the_installation_read_last_is_kept_and_the_other_named_once() {
		let here = temp();
		let (older, old_root) = with_both(here.path(), "AU21X", "vcds-ru");
		std::thread::sleep(std::time::Duration::from_millis(5));
		let (newer, new_root) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));

		let notes = migrate_in(&here.path().join("data"), &store).notes;
		assert_eq!(notes.len(), 2, "{notes:?}");
		assert!(notes[0].starts_with("moved the installation's data from "), "{notes:?}");
		assert!(
			notes[1].contains(&crate::datadir::tilde(&old_root)) && notes[1].contains("AU21X"),
			"{notes:?}"
		);
		let kept = sources(&store).unwrap();
		assert_eq!(kept.len(), 1);
		assert_eq!(kept[0].path(), new_root.display().to_string());
		assert!(
			std::fs::read_to_string(store.names()).unwrap().contains("vcds-en"),
			"the files are the kept one's"
		);
		for p in [&older, &newer] {
			assert_eq!(counts(&p.cache()), (0, 0, 2, 1), "{}", p.id);
			assert!(!p.names().exists());
		}
	}

	#[test]
	fn an_installation_read_into_the_store_after_a_project_keeps_its_place() {
		// A project whose VCDS was read before the store's: the store's stays, and the
		// project's rows go with one note.
		let here = temp();
		let (p, old_root) = with_both(here.path(), "SK37X", "vcds-old");
		std::thread::sleep(std::time::Duration::from_millis(5));
		let (other, _) = with_both(here.path(), "SCRATCH", "vcds-new");
		let store = vcds_at(&here.path().join("vcds"));
		move_into(&other, &store, &Default::default()).unwrap();
		std::fs::remove_dir_all(&other.dir).unwrap();
		let notes = migrate_in(&here.path().join("data"), &store).notes;
		assert_eq!(notes.len(), 1, "{notes:?}");
		assert!(notes[0].contains(&crate::datadir::tilde(&old_root)), "{notes:?}");
		assert!(sources(&store).unwrap()[0].path().ends_with("vcds-new"));
		assert_eq!(counts(&p.cache()), (0, 0, 2, 1));
	}

	/// Whether any staged file or a move marker is left in the store.
	fn leftovers_in(store: &Project) -> Vec<String> {
		std::fs::read_dir(&store.dir)
			.unwrap()
			.flatten()
			.map(|e| e.file_name().to_string_lossy().into_owned())
			.filter(|name| name.ends_with(".migrating") || name == ".migrating.json")
			.collect()
	}

	#[test]
	fn a_move_stopped_after_its_rows_committed_puts_its_files_in_place_and_records_it() {
		// Review of #50: a kill after `copy_kind` committed and before the renames left the
		// store with the rows and only staged files. The next open saw the stamps alike, took
		// the project's four files out, and they lived on only as files nothing reads.
		let here = temp();
		let (older, old_root) = with_both(here.path(), "OLD", "vcds-old");
		let store = vcds_at(&here.path().join("vcds"));
		move_into(&older, &store, &Default::default()).unwrap();
		std::fs::remove_dir_all(&older.dir).unwrap();
		std::thread::sleep(std::time::Duration::from_millis(5));
		let (p, _) = with_both(here.path(), "SK37X", "vcds-new");
		let files: Vec<Vec<u8>> = vcds_files(&p).iter().map(|f| std::fs::read(f).unwrap()).collect();
		// The record a migration makes of this move: the store's own installation replaced.
		let mut record = serde_json::Map::new();
		record.insert("at".into(), "2026-10-21T08:00:00+00:00".into());
		record.insert("moved".into(), serde_json::json!({ "from": p.dir }));
		record.insert("replaced".into(), serde_json::json!([{ "installation": old_root }]));
		// Killed here: rows committed, files staged, nothing renamed.
		begin_move(&p, &store, &record).unwrap();
		assert_eq!(leftovers_in(&store).len(), 5, "four staged files and the marker");

		let notes = migrate_in(&here.path().join("data"), &store).notes;
		assert!(notes.iter().any(|n| n.starts_with("finished moving the installation ")), "{notes:?}");
		for (file, bytes) in vcds_files(&store).iter().zip(&files) {
			assert_eq!(&std::fs::read(file).unwrap(), bytes, "{file:?} is not the moved installation's");
		}
		assert!(leftovers_in(&store).is_empty(), "{:?}", leftovers_in(&store));
		assert_eq!((counts(&store.cache()), counts(&p.cache())), ((1, 2, 0, 0), (0, 0, 2, 1)));
		for file in vcds_files(&p) {
			assert!(!file.exists(), "{file:?} stayed in the project");
		}
		// The record of the replaced installation survived the kill, once.
		let entries = || -> Vec<serde_json::Value> { serde_json::from_str(&std::fs::read_to_string(migration_record(&store)).unwrap()).unwrap() };
		assert_eq!(entries().len(), 1, "{:?}", entries());
		assert!(entries()[0]["replaced"][0]["installation"].as_str().unwrap().ends_with("vcds-old"));
		assert!(migrate_in(&here.path().join("data"), &store).notes.is_empty());
		assert_eq!(entries().len(), 1, "recorded twice");

		// Killed before the rows committed: the staged files are undone, and the move is
		// made again from the project, which still holds everything.
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));
		begin_move(&p, &store, &Default::default()).unwrap();
		vag_data_db::drop_kind(&store.cache(), vag_data_db::VCDS).unwrap();
		let notes = migrate_in(&here.path().join("data"), &store).notes;
		assert!(notes.iter().any(|n| n.starts_with("moved the installation's data")), "{notes:?}");
		assert!(leftovers_in(&store).is_empty());
		assert_eq!((counts(&store.cache()), counts(&p.cache())), ((1, 2, 0, 0), (0, 0, 2, 1)));
		assert!(vcds_files(&store).iter().all(|f| f.is_file()));

		// The same stop by a vagcan that wrote no marker: the project gives its files again.
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));
		begin_move(&p, &store, &Default::default()).unwrap();
		std::fs::remove_file(move_marker(&store)).unwrap();
		migrate_in(&here.path().join("data"), &store);
		assert!(leftovers_in(&store).is_empty());
		assert!(vcds_files(&store).iter().all(|f| f.is_file()), "the store lacks the installation's files");
		assert_eq!(counts(&p.cache()), (0, 0, 2, 1));
	}

	#[test]
	fn a_project_keeps_its_vcds_while_the_store_lacks_a_file_of_it() {
		// The store holds this installation's rows, and one of its files cannot reach it:
		// nothing is taken out of the project.
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));
		begin_move(&p, &store, &Default::default()).unwrap();
		std::fs::remove_file(move_marker(&store)).unwrap();
		// Where the names would go, something no file can be copied over.
		std::fs::create_dir_all(store.names()).unwrap();
		let notes = migrate_in(&here.path().join("data"), &store).notes;
		assert!(notes.iter().any(|n| n.contains("did not reach the store")), "{notes:?}");
		assert_eq!(counts(&p.cache()), (1, 2, 2, 1), "the project lost rows");
		assert!(p.names().is_file(), "the project lost its names");
	}

	#[test]
	fn a_project_emptied_by_a_stopped_run_is_still_removed() {
		// Review of #50: the project was removed only right after its rows went, so a kill
		// after the drop (during the VACUUM, say) stranded an empty project for good, its
		// measured rows never moved.
		let here = temp();
		let (p, _) = with_both(here.path(), "default", "vcds-en");
		vag_data_db::drop_kind(&p.cache(), vag_data_db::ODIS).unwrap();
		std::fs::create_dir_all(p.measurements_dir()).unwrap();
		std::fs::write(p.measurements_dir().join("5Q0614517AQ.json"), b"{}").unwrap();
		let store = vcds_at(&here.path().join("vcds"));
		move_into(&p, &store, &Default::default()).unwrap();
		// Killed here: the rows gone, the project not yet settled.
		take_out(&p).unwrap().unwrap();
		assert!(taken_out_marker(&p).is_file());
		let migrated = migrate_in(&here.path().join("data"), &store);
		assert_eq!(migrated.removed, ["default"], "{:?}", migrated.notes);
		assert!(!p.dir.exists());
		assert!(store.measurements_dir().join("5Q0614517AQ.json").is_file(), "its measured rows were lost");
		// A project with ODIS rows beside them stays, and loses the marker.
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));
		move_into(&p, &store, &Default::default()).unwrap();
		take_out(&p).unwrap().unwrap();
		assert!(migrate_in(&here.path().join("data"), &store).notes.is_empty());
		assert!(p.dir.is_dir() && !taken_out_marker(&p).exists());
	}

	#[test]
	fn a_migration_stopped_anywhere_is_finished_by_the_next_open() {
		// After the store's commit, before the project lost anything: the store has it,
		// stamped alike, so only the project's half is done — silently.
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));
		move_into(&p, &store, &Default::default()).unwrap();
		assert!(migrate_in(&here.path().join("data"), &store).notes.is_empty());
		assert_eq!((counts(&store.cache()), counts(&p.cache())), ((1, 2, 0, 0), (0, 0, 2, 1)));
		assert!(store.names().is_file() && !p.names().exists());

		// After the project's files went, before its rows: the same.
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));
		move_into(&p, &store, &Default::default()).unwrap();
		for file in vcds_files(&p) {
			std::fs::remove_file(file).unwrap();
		}
		assert!(migrate_in(&here.path().join("data"), &store).notes.is_empty());
		assert_eq!((counts(&store.cache()), counts(&p.cache())), ((1, 2, 0, 0), (0, 0, 2, 1)));
		assert!(store.registry_log().is_file(), "the store keeps what was copied");

		// After the files were copied and before the rows committed: the store holds no
		// rows, so the copy is made again, and only then does the project lose anything.
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));
		std::fs::create_dir_all(&store.dir).unwrap();
		std::fs::copy(p.names(), store.names()).unwrap();
		let notes = migrate_in(&here.path().join("data"), &store).notes;
		assert_eq!(notes.len(), 1, "{notes:?}");
		assert_eq!((counts(&store.cache()), counts(&p.cache())), ((1, 2, 0, 0), (0, 0, 2, 1)));

		// A store that will not take the rows: nothing leaves the project.
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));
		std::fs::create_dir_all(&store.dir).unwrap();
		std::fs::write(store.cache(), b"this is not an SQLite file, and it is longer than a header").unwrap();
		let notes = migrate_in(&here.path().join("data"), &store).notes;
		assert_eq!(notes.len(), 1, "{notes:?}");
		assert!(notes[0].contains("stay"), "{notes:?}");
		assert_eq!(counts(&p.cache()), (1, 2, 2, 1), "the project lost rows the store does not hold");
		assert!(p.names().is_file());
	}

	#[test]
	fn migrations_run_at_once_take_turns_and_lose_no_row() {
		// Review of #50: two processes opening the store together both decided to move;
		// one emptied the project, and the other then copied that empty project over the
		// full store. Here several migrations at once, many times over: every time the
		// store holds the installation whole and the project none of it.
		const THREADS: usize = 6;
		const ROUNDS: usize = 25;
		let mut intact = 0;
		for _ in 0..ROUNDS {
			let here = temp();
			let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
			let data = here.path().join("data");
			let store = vcds_at(&here.path().join("vcds"));
			let moved: usize = std::thread::scope(|scope| {
				let runs: Vec<_> = (0..THREADS)
					.map(|_| scope.spawn(|| migrate_in(&data, &store).notes.iter().filter(|n| n.starts_with("moved")).count()))
					.collect();
				runs.into_iter().map(|run| run.join().unwrap()).sum()
			});
			assert_eq!(moved, 1, "the installation moved once, not once per migration");
			assert_eq!(counts(&store.cache()), (1, 2, 0, 0), "the store lost rows");
			assert_eq!(counts(&p.cache()), (0, 0, 2, 1), "the project kept VCDS rows or lost ODIS ones");
			assert_eq!(sources(&store).unwrap().len(), 1);
			intact += 1;
		}
		assert_eq!(intact, ROUNDS);
	}

	#[test]
	fn a_copy_from_a_cache_with_no_vcds_source_is_refused_and_the_store_kept() {
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		let store = vcds_at(&here.path().join("vcds"));
		move_into(&p, &store, &Default::default()).unwrap();
		take_out(&p).unwrap().unwrap();
		let why = vag_data_db::copy_kind(&p.cache(), &store.cache(), vag_data_db::VCDS)
			.unwrap_err()
			.to_string();
		assert!(why.contains("holds no vcds source"), "{why}");
		assert_eq!(counts(&store.cache()), (1, 2, 0, 0), "the refused copy took the store's rows");
	}

	#[test]
	fn a_store_installation_a_project_replaces_is_named_and_recorded() {
		let here = temp();
		let (older, old_root) = with_both(here.path(), "OLD", "vcds-old");
		let store = vcds_at(&here.path().join("vcds"));
		move_into(&older, &store, &Default::default()).unwrap();
		std::fs::remove_dir_all(&older.dir).unwrap();
		std::thread::sleep(std::time::Duration::from_millis(5));
		let (_newer, _) = with_both(here.path(), "SK37X", "vcds-new");
		let notes = migrate_in(&here.path().join("data"), &store).notes;
		assert!(notes[0].starts_with("moved the installation's data from "), "{notes:?}");
		assert!(
			notes
				.iter()
				.any(|n| n.starts_with("replaced the installation ") && n.contains(&crate::datadir::tilde(&old_root))),
			"the store's own was dropped unnamed: {notes:?}"
		);
		let record: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(migration_record(&store)).unwrap()).unwrap();
		let entry = &record[0];
		assert!(
			entry["at"].is_string() && entry["moved"]["from"].as_str().unwrap().ends_with("SK37X"),
			"{record}"
		);
		assert!(entry["replaced"][0]["installation"].as_str().unwrap().ends_with("vcds-old"), "{record}");
	}

	#[test]
	fn a_project_that_held_vcds_alone_goes_and_its_measured_rows_move_with_it() {
		let here = temp();
		let (p, _) = with_both(here.path(), "default", "vcds-en");
		// VCDS alone: its ODIS half dropped, as a VCDS-only project was.
		vag_data_db::drop_kind(&p.cache(), vag_data_db::ODIS).unwrap();
		std::fs::create_dir_all(p.measurements_dir()).unwrap();
		std::fs::write(p.measurements_dir().join("5Q0614517AQ.json"), b"{}").unwrap();
		let store = vcds_at(&here.path().join("vcds"));
		let migrated = migrate_in(&here.path().join("data"), &store);
		assert_eq!(migrated.removed, ["default"], "{:?}", migrated.notes);
		assert!(!p.dir.exists(), "the empty project stayed");
		assert!(store.measurements_dir().join("5Q0614517AQ.json").is_file(), "its measured rows were lost");
		assert!(
			migrated.notes.iter().any(|n| n.starts_with("removed the project default")),
			"{:?}",
			migrated.notes
		);
		assert_eq!(list_in(&here.path().join("data")), Vec::<String>::new());
	}

	#[test]
	fn the_files_a_dropped_installation_left_in_a_project_go_to_the_store() {
		// `--drop vcds` kept the keys and the names in the project: with no VCDS rows to
		// migrate they stayed there for good, and the next setup searched the keys again.
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		vag_data_db::drop_kind(&p.cache(), vag_data_db::VCDS).unwrap();
		std::fs::remove_file(p.registry_log()).unwrap();
		std::fs::write(p.rod_keys(), r#"{"RD.rod:ABC": "1"}"#).unwrap();
		let store = vcds_at(&here.path().join("vcds"));
		std::fs::create_dir_all(&store.dir).unwrap();
		std::fs::write(store.rod_keys(), r#"{"MUX.rod:DEF": "2"}"#).unwrap();
		let notes = migrate_in(&here.path().join("data"), &store).notes;
		assert!(notes.iter().any(|n| n.starts_with("moved ")), "{notes:?}");
		let keys: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(store.rod_keys()).unwrap()).unwrap();
		assert!(keys["RD.rod:ABC"].is_string() && keys["MUX.rod:DEF"].is_string(), "merged: {keys}");
		assert!(store.names().is_file() && store.odx_ids().is_file());
		for file in vcds_files(&p) {
			assert!(!file.exists(), "{file:?} stayed in the project");
		}
		assert!(p.dir.is_dir(), "the project holds ODIS rows and stays");
	}

	#[test]
	fn a_copy_that_fails_leaves_the_stores_files_and_rows_as_they_were() {
		let here = temp();
		let (older, _) = with_both(here.path(), "OLD", "vcds-old");
		let store = vcds_at(&here.path().join("vcds"));
		move_into(&older, &store, &Default::default()).unwrap();
		let names_before = std::fs::read(store.names()).unwrap();
		// A project whose cache holds no VCDS source: the copy is refused.
		take_out(&older).unwrap().unwrap();
		std::fs::write(older.names(), b"{\"000001\": \"other\"}").unwrap();
		assert!(move_into(&older, &store, &Default::default()).is_err());
		assert_eq!(std::fs::read(store.names()).unwrap(), names_before, "a file landed before its rows");
		assert_eq!(counts(&store.cache()), (1, 2, 0, 0));
		assert!(
			std::fs::read_dir(&store.dir)
				.unwrap()
				.flatten()
				.all(|e| !e.file_name().to_string_lossy().ends_with(".migrating")),
			"a staged file was left"
		);
	}

	#[test]
	fn a_project_with_odis_alone_is_left_as_it_was() {
		let here = temp();
		let (p, _) = with_both(here.path(), "SK37X", "vcds-en");
		take_out(&p).unwrap().unwrap();
		let store = vcds_at(&here.path().join("vcds"));
		assert!(migrate_in(&here.path().join("data"), &store).notes.is_empty());
		assert!(!store.cache().exists(), "a store was made with nothing to put in it");
		assert_eq!(counts(&p.cache()), (0, 0, 2, 1));
	}

	#[test]
	fn with_no_project_a_vcds_store_is_what_the_run_reads() {
		// `selected` is `None` with no project, and that is no error: the VCDS store
		// stands alone. `current` still refuses, for the commands that need a project.
		let here = temp();
		assert_eq!(selected_in(here.path(), None, None, None, None).unwrap(), None);
		assert!(current_in(here.path(), None, None, None, None).is_err());
		// A project named that is not there is still an error, store or none.
		assert!(selected_in(here.path(), Some("SK37X"), None, None, None).is_err());
		let stores = Stores {
			project: None,
			vcds: vcds_at(&here.path().join("vcds")),
		};
		assert_eq!(stores.cache_of(vag_data_db::ODIS), None);
		assert_eq!(stores.cache_of(vag_data_db::VCDS), Some(here.path().join("vcds").join("cache.sqlite")));
		assert!(
			crate::source::Run::of(&stores, crate::source::Source::Odis).is_none(),
			"no project, no ODIS run"
		);
		assert_eq!(stores.id(), VCDS_STORE);
	}

	// The two tests that used to sit here — that remembering a project leaves
	// the rest of the settings alone, and that a settings file which will not
	// parse is no answer rather than a failure — moved to `crate::config` with
	// the file itself. They are the same assertions about the same behaviour;
	// what changed is which module owns reading and writing it.

	#[test]
	fn nothing_a_project_holds_lands_in_the_checkout() {
		// The rule `datadir` exists for, asked of the new layout too.
		let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
		let home = crate::datadir::vagcan_dir().unwrap();
		for path in [crate::datadir::projects_dir().unwrap(), crate::datadir::rod_pool_dir().unwrap()] {
			assert!(!path.starts_with(&repo), "{path:?} is inside the checkout");
			assert!(path.starts_with(&home), "{path:?} is outside ~/.vagcan");
		}
	}
}
