//! `~/.vagcan/data/<id>/` — one platform's data, whatever it was learned from.
//!
//! There are two sources — a VCDS installation and an extracted ODIS-Service
//! project — and they describe the same car from different directions, so the
//! split that matters is not *which parser produced this* but *which car is
//! this about*. A project is that: a directory holding everything known about
//! one car, into which a second source is **added** rather than swapped
//! (design §5).
//!
//! ```text
//! ~/.vagcan/
//!   rod/                    shared across every project — the raw VCDS files
//!                           read at run time (.rod, the fault text). A property
//!                           of a VCDS *build*, not of a car, so one copy serves
//!                           every project parsed from it.
//!   data/
//!     SK37X/
//!       cache.sqlite        the label/ODIS rows, queryable, and the sources they
//!                           came from (its `source` table)
//!       names.json          text id -> name
//!       odx-ids.json        text id -> the IDE/MAS id that text names
//!       rod-keys.json       recovered .rod section keys — per project, because a
//!                           key is a property of one file's *bytes* and two VCDS
//!                           builds ship a same-named .rod with different content
//!       measurements/       proven-on-car rows, one file per part number
//!   config.toml             which project a bare command means
//! ```
//!
//! **A project is a platform, not a car** (spec §4.1). `SK37X` is VW's own
//! project id and it covers every Octavia III, Karoq and Kodiaq — which is why
//! the proven rows live here: a proven scaling is a property of a *part number*,
//! true of every car carrying that part. `cars/<VIN>/` goes on holding what is
//! true of one car and no other.
//!
//! **`measurements/` is the one directory here that nothing can recreate.** Every
//! other file is extracted from somebody else's — a re-parse reproduces it. Those
//! rows were proven by driving a car (`.archive/research/labels/rod-labels.md` §4.0c), and
//! that is why nothing in this module ever deletes a project.
//!
//! **Nothing here invents a name for a car.** An ODIS project names itself and
//! [`vag_data_labels::odis::Project::id`] reads it; a VCDS-only project is named by the
//! person. This module's whole contribution to naming is [`folder_name`], which
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

/// One car's store on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
	pub id: String,
	pub dir: PathBuf,
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
	let env = std::env::var(PROJECT_ENV).ok();
	let configured = configured()?;
	current_in(
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
fn current_in(projects: &Path, flag: Option<&str>, env: Option<&str>, covering: Option<&str>, configured: Option<&str>) -> Result<Project> {
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
		return Ok(Project { id: name, dir });
	}
	match list_in(projects).as_slice() {
		// "Run `vagcan setup` first", for a store that is now per car rather
		// than one directory. The words are `missing`'s, not this module's:
		// this is the same shortage `vcds names` and `faults` report, met one
		// step earlier, and a reader who sees it here and then there must not
		// be given two accounts of it.
		// The `NoLabelData` itself, not its text: `vag_cli_diag::rescue`
		// recognises the shortage by downcasting to it, and a rendered string
		// would leave it matching on prose.
		[] => Err(
			crate::missing::NoLabelData::new("no project has been set up")
				.looked_in(projects)
				.into_error(),
		),
		// One project is not a choice, so it is not asked about.
		[only] => Ok(Project {
			id: only.clone(),
			dir: projects.join(only),
		}),
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
		bail!("{id:?} already names a folder — the one above, or the one it is in");
	}
	if id.chars().count() > MAX_ID {
		bail!(
			"{id:?} is {} characters, and {MAX_ID} is the most a project name may be",
			id.chars().count()
		);
	}
	if let Some(bad) = id.chars().find(|c| !allowed(*c)) {
		bail!(
			"{id:?} cannot be a project name — {bad:?} is not a character a folder under \
			 ~/.vagcan/data/ may hold. Letters, digits, `-`, `_` and `.` and nothing else."
		);
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
