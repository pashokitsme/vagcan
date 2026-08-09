//! `~/.vagcan/data/<id>/` — one platform's data, whatever it was learned from.
//!
//! There used to be one `data/extracted/`, because there was one source: a VCDS
//! installation. There are two now — a VCDS installation and an extracted
//! ODIS-Service project — and they describe the same car from different
//! directions, so the split that matters is no longer *which parser produced
//! this* but *which car is this about*. A project is that: a directory holding
//! everything known about one car, into which a second source is **added**
//! rather than swapped (design §5).
//!
//! ```text
//! ~/.vagcan/
//!   rod/                    shared across every project — the raw VCDS files
//!                           read at run time (.rod, the fault text). A property
//!                           of a VCDS *build*, not of a car, so one copy serves
//!                           every project parsed from it.
//!   data/
//!     SK37X/
//!       cache.sqlite        the label/ODIS rows, queryable
//!       names.json          text id -> name
//!       rod-keys.json       recovered .rod section keys — per project, because a
//!                           key is a property of one file's *bytes* and two VCDS
//!                           builds ship a same-named .rod with different content
//!       measurements/       proven-on-car rows, one file per part number
//!       sources.json        where this project's data came from
//!     extracted/            the layout from before projects existed — siblings of
//!     measured/             the projects now, and told apart from them by name
//!   config.json             which project a bare command means
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
//! rows were proven by driving a car (`research/labels/rod-labels.md` §4.0c), and
//! that is why the migration in [`crate::datadir`] copies before it removes and
//! why nothing in this module ever deletes a project.
//!
//! **Nothing here invents a name for a car.** An ODIS project names itself and
//! [`vag_data::odis::Project::id`] reads it; a VCDS-only project is named by the
//! person. This module's whole contribution to naming is [`folder_name`], which
//! refuses one that would not be a single child of `data/` — the same rule
//! `datadir::car_folder` applies to a VIN off the bus, and for the same reason:
//! `--project ../../etc` is a path, not a name.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// The most a project name may be. A directory name, not a sentence.
const MAX_ID: usize = 64;

/// The two names the pre-project layout used, which a project may not take.
///
/// `~/.vagcan/data/` holds both layouts now (spec §6): `extracted` and
/// `measured` are the old directories and anything else under it is a project.
/// VW's own names cannot collide with these — they are `SK37X`, `AU21X` and the
/// like — so refusing them costs a user nothing, and it stops a project from
/// being mistaken for the layout it replaced. Compared case-insensitively,
/// because the filesystem this runs on is.
const RESERVED: [&str; 2] = ["extracted", "measured"];

/// The environment variable that names the project for one run (D6).
pub const PROJECT_ENV: &str = "VAGCAN_PROJECT";

/// The `--project` flag's answer, for the length of the process.
///
/// A global rather than an argument threaded through every command, for the
/// reason `vag_protocol::address::install` is one: the flag is parsed once at
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

	/// Text id → name, the union of what TTTEXT and an ODIS project know.
	pub fn names(&self) -> PathBuf {
		self.dir.join("names.json")
	}

	/// The recovered `.rod` section keys.
	pub fn rod_keys(&self) -> PathBuf {
		self.dir.join("rod-keys.json")
	}

	/// The proven-on-car rows, one file per part number.
	pub fn measurements_dir(&self) -> PathBuf {
		self.dir.join("measurements")
	}

	/// The provenance log (design §4.4).
	pub fn sources(&self) -> PathBuf {
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

/// One line of a project's provenance log.
///
/// Read by nothing at run time. It exists so a person can answer "where did this
/// project's data come from", the way `git log` answers it for code — which is a
/// real question once two sources have merged into one `cache.sqlite` and the
/// rows no longer say which run put them there.
#[derive(Debug, Clone)]
pub struct SourceEntry {
	/// `"vcds"` or `"odis"`.
	pub kind: &'static str,
	/// The directory it was read from, as it was on the day.
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
/// tested without writing into the owner's own — the same reason
/// `datadir::car_folder_in` takes `cars/`.
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
/// the car in front of the tool**, then `config.json`, then the only project on
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
/// `config.json` on purpose. The two overrides are a person saying "this one,
/// I mean it", and nothing read off a bus outranks that. `config.json` is only a
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

/// Where to tell somebody to put a file they have just produced, in words.
///
/// **A literal path in a printed instruction is a promise, and this one moved.**
/// `~/.vagcan/data/measured/` was correct for as long as there was one store; it
/// is now one directory per platform, and the old text told a person on a real
/// car to put a proven row somewhere that no longer exists. So the instruction
/// is resolved rather than written out: the actual directory when a project
/// resolves, and the shape plus what to do about it when none does.
///
/// Returned as a `String` because every caller is building a sentence, and a
/// `Result` here would make each of them decide what to say when there is no
/// project — which is the question this answers once.
pub fn measurements_hint() -> String {
	match current() {
		Ok(project) => project.measurements_dir().display().to_string(),
		// Naming the shape is more use than naming nothing: it says a project
		// is involved, and the sentence after it says how to get one.
		Err(_) => "~/.vagcan/data/<project>/measurements/ (run `vagcan setup` first — it makes the project)".to_string(),
	}
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
/// covers that. Neither is invented here — the second is `vag_data::odis`'s to
/// provide off the `.vi` pool, and the first arrives from whichever command
/// opened the adapter. The likely shape is the idiom this crate already uses
/// twice ([`select`], `vag_protocol::address::install`): whoever reads the car
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
		(configured, "the project in ~/.vagcan/config.json"),
	] {
		let Some(id) = given.map(str::trim).filter(|id| !id.is_empty()) else {
			continue;
		};
		let name = folder_name(id).with_context(|| format!("{whose} named {id:?}"))?;
		let dir = projects.join(&name);
		if !dir.is_dir() {
			bail!(
				"{whose} names the project {id:?}, and there is no {}.\n\n\
				 {}",
				dir.display(),
				which_there_are(projects)
			);
		}
		return Ok(Project { id: name, dir });
	}
	match list_in(projects).as_slice() {
		// The existing "run `vagcan setup` first", reworded for a store that is
		// now per car rather than one directory.
		[] => bail!(
			"no car has been set up yet — there is nothing under {}.\n\n\
			 Run `vagcan setup` and point it at a VCDS installation or an ODIS project.",
			projects.display()
		),
		// One project is not a choice, so it is not asked about.
		[only] => Ok(Project {
			id: only.clone(),
			dir: projects.join(only),
		}),
		_ => bail!(
			"more than one car is set up, and nothing says which this is about.\n\n\
			 {}\n\n\
			 Say which:  vagcan --project <id> …\n\
			 or set it for good by running `vagcan setup` against that car again.",
			which_there_are(projects)
		),
	}
}

/// The projects there actually are, for a message about one there is not.
fn which_there_are(projects: &Path) -> String {
	match list_in(projects).as_slice() {
		[] => format!("Nothing is set up under {}.", projects.display()),
		found => format!("Set up here: {}", found.join(", ")),
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

/// Append one entry to a project's provenance log.
///
/// Appends rather than replaces: two sources merge into one project, and a log
/// that kept only the last would answer the question it exists for with half the
/// truth. An unreadable existing file is started over rather than failing the
/// run — nothing reads this at run time, and losing a `setup` to a corrupt note
/// about a previous one would be the wrong trade.
pub fn record_source(p: &Project, entry: SourceEntry) -> Result<()> {
	let path = p.sources();
	let mut sources = std::fs::read_to_string(&path)
		.ok()
		.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
		.and_then(|value| value.get("sources").and_then(|s| s.as_array()).cloned())
		.unwrap_or_default();

	let mut row = serde_json::Map::new();
	row.insert("kind".into(), entry.kind.into());
	row.insert("path".into(), entry.path.into());
	if let Some(version) = entry.version {
		row.insert("version".into(), version.into());
	}
	if let Some(detail) = entry.detail {
		row.insert("detail".into(), detail.into());
	}
	row.insert("parsed_at".into(), chrono::Utc::now().to_rfc3339().into());
	sources.push(serde_json::Value::Object(row));

	let document = serde_json::json!({ "sources": sources });
	std::fs::create_dir_all(&p.dir).with_context(|| format!("creating {}", p.dir.display()))?;
	std::fs::write(&path, serde_json::to_string_pretty(&document)?).with_context(|| format!("writing {}", path.display()))?;
	Ok(())
}

/// Whether a source of this kind has ever been read into this project.
///
/// Asked of `sources.json`, which is why that log exists — design §4.4 says
/// nothing reads it at run time, and this is the exception that earned itself:
/// a file's *content* cannot say which parser produced it, and `names.json` on
/// a project that has only ever seen ODIS runs is ODIS wording under a name
/// that promises VCDS's. The log is the only thing on disk that knows.
///
/// `false` when the log is missing or unreadable — a project that cannot say
/// where its data came from gets the cautious answer, which is to trust nothing
/// it did not have to.
pub fn has_source(project: &Project, kind: &str) -> bool {
	let Ok(text) = std::fs::read_to_string(project.sources()) else {
		return false;
	};
	let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
		return false;
	};
	value["sources"]
		.as_array()
		.is_some_and(|rows| rows.iter().any(|row| row["kind"].as_str() == Some(kind)))
}

/// What `config.json` says this machine's project is, if it says.
pub fn configured() -> Result<Option<String>> {
	Ok(configured_in(&crate::datadir::config_file()?))
}

fn configured_in(config: &Path) -> Option<String> {
	let text = std::fs::read_to_string(config).ok()?;
	let value: serde_json::Value = serde_json::from_str(&text).ok()?;
	value.get("project")?.as_str().map(str::to_owned)
}

/// Write down which project a bare command means from now on.
///
/// Read-modify-write rather than overwrite: `config.json` is "settings that are
/// not about one car" and this owns exactly one key of it, so anything else in
/// there — now or later — has to survive being written by this.
pub fn remember(id: &str) -> Result<()> {
	remember_in(&crate::datadir::config_file()?, id)
}

fn remember_in(config: &Path, id: &str) -> Result<()> {
	let name = folder_name(id)?;
	let mut document = std::fs::read_to_string(config)
		.ok()
		.and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
		.and_then(|value| value.as_object().cloned())
		.unwrap_or_default();
	document.insert("project".into(), name.into());
	if let Some(parent) = config.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
	}
	std::fs::write(config, serde_json::to_string_pretty(&serde_json::Value::Object(document))?)
		.with_context(|| format!("writing {}", config.display()))?;
	Ok(())
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
	if RESERVED.iter().any(|reserved| id.eq_ignore_ascii_case(reserved)) {
		bail!(
			"{id:?} is what vagcan called part of its layout before it kept one directory per car, \
			 and that directory still sits beside the projects in ~/.vagcan/data/. Pick another name — \
			 VW's own are like `SK37X`."
		);
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

/// Whether one character may be in a project name. Matches
/// `setup::source`'s rule, which is the interactive half of the same question.
fn allowed(c: char) -> bool {
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
		for path in [p.cache(), p.names(), p.rod_keys(), p.measurements_dir(), p.sources()] {
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
	fn the_layout_this_replaced_cannot_be_taken_as_a_project_name() {
		// Spec §6. `data/` holds both layouts now, so `extracted` and `measured`
		// are siblings of the projects rather than the whole of it. A project
		// under either name would be mistaken for the layout it replaced — and
		// worse, `list` would offer it and `migrate` would still be walking it.
		for taken in ["extracted", "measured", "Extracted", "MEASURED"] {
			assert!(folder_name(taken).is_err(), "{taken:?} was accepted");
		}
		// It costs a user nothing: VW's own names cannot collide with them.
		for vw in ["SK37X", "AU21X", "VW37X"] {
			assert!(folder_name(vw).is_ok(), "{vw:?} was refused");
		}
	}

	#[test]
	fn the_old_layout_is_never_listed_as_a_project_it_sits_beside() {
		// The consequence of the two living in one directory: `list` reads
		// `data/`, and without the reserved names it would report the migration
		// source as a car to choose between.
		let here = temp();
		for name in ["extracted", "measured"] {
			std::fs::create_dir_all(here.path().join(name)).unwrap();
		}
		open_or_create_in(here.path(), "SK37X").unwrap();
		assert_eq!(list_in(here.path()), ["SK37X"]);
		// And with the old layout still there, one project is still one project
		// — so a bare command resolves without asking.
		assert_eq!(current_in(here.path(), None, None, None, None).unwrap().id, "SK37X");
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
		let why = current_in(here.path(), None, None, None, None).unwrap_err().to_string();
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
		let why = current_in(here.path(), None, None, None, None).unwrap_err().to_string();
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
		let why = current_in(here.path(), None, None, Some("SK37X"), None).unwrap_err().to_string();
		assert!(why.contains("covering the car that is connected"), "{why}");
		assert!(why.contains("SK37X") && why.contains("AU21X"), "{why}");
	}

	#[test]
	fn a_selected_project_that_is_not_there_is_an_error_and_not_a_new_one() {
		// A typo in a flag must not create an empty store and then report that
		// the car has no data — the second message would be true and useless.
		let here = temp();
		open_or_create_in(here.path(), "SK37X").unwrap();
		let why = current_in(here.path(), Some("SK37Y"), None, None, None).unwrap_err().to_string();
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

		let text = std::fs::read_to_string(p.sources()).unwrap();
		let value: serde_json::Value = serde_json::from_str(&text).unwrap();
		let rows = value["sources"].as_array().unwrap();
		assert_eq!(rows.len(), 2, "{text}");
		assert_eq!(rows[0]["kind"], "vcds");
		assert_eq!(rows[1]["version"], "2610.2.688");
		assert!(rows[1]["parsed_at"].as_str().is_some_and(|t| t.contains('T')), "{text}");
	}

	#[test]
	fn remembering_a_project_leaves_the_rest_of_the_config_alone() {
		// `config.json` is "settings that are not about one car"; this owns one
		// key of it, so whatever else is in there has to survive being written.
		let here = temp();
		let config = here.path().join("config.json");
		std::fs::write(&config, r#"{"something-else": 7}"#).unwrap();
		remember_in(&config, "SK37X").unwrap();
		assert_eq!(configured_in(&config).as_deref(), Some("SK37X"));
		let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&config).unwrap()).unwrap();
		assert_eq!(value["something-else"], 7);
	}

	#[test]
	fn a_config_that_is_not_json_is_no_answer_rather_than_a_failure() {
		let here = temp();
		let config = here.path().join("config.json");
		std::fs::write(&config, "not json at all").unwrap();
		assert_eq!(configured_in(&config), None);
		// And it still remembers, because a run that cannot record which project
		// it set up would ask again for ever.
		remember_in(&config, "SK37X").unwrap();
		assert_eq!(configured_in(&config).as_deref(), Some("SK37X"));
	}

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
