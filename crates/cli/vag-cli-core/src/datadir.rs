//! Finding the data files, wherever the command was run from.
//!
//! **Nothing this tool reads lives in the checkout any more.** Every default
//! used to be a path relative to the working directory — `catalogs/…` — which
//! meant the tool worked from the repository root and quietly did nothing
//! anywhere else, and after `cargo install` did nothing at all. It also meant
//! that two kinds of file nobody may publish, one derived from a proprietary
//! VCDS installation and one measured off somebody's own car, sat inside a
//! repository one `git add` away from being shared.
//!
//! So there is one directory, `~/.vagcan`, and [`vagcan_dir`] describes its
//! shape. The rule for a path the user typed is unchanged: it is used exactly
//! as typed, and [`resolve`] is what a *relative* one is looked up through.

use std::path::{Path, PathBuf};

/// How far up to look. A checkout is a handful of levels deep at most, and
/// walking to the filesystem root risks matching some unrelated `catalogs/`.
const MAX_PARENTS: usize = 6;

/// What a described car has in its directory, and nothing else does.
///
/// Named here because this module decides the layout — `measure::carfile` joins
/// the same name onto [`car_dir`] — and because [`existing_folder`] has to know
/// which of two directories for one VIN is the car.
pub const CAR_FILE: &str = "car.json";

/// Resolve a default data path, or return it unchanged if it exists as given.
///
/// Returns the path as-is when nothing is found, so the caller's own error
/// message still names something the user can act on.
pub fn resolve(relative: &str) -> PathBuf {
	let given = Path::new(relative);
	if given.exists() {
		return given.to_path_buf();
	}
	for base in search_roots() {
		let candidate = base.join(relative);
		if candidate.exists() {
			return candidate;
		}
	}
	given.to_path_buf()
}

/// This tool's own directory, `~/.vagcan`.
///
/// ```text
/// ~/.vagcan/
///   rod/                              raw VCDS files read at run time, shared
///                                     across every project: the .rod files and
///                                     this build's fault text
///   data/
///     SK37X/                          one directory per *platform* — `crate::project`
///       cache.sqlite                    the label and ODIS rows, queryable
///       names.json                      text id -> name
///       rod-keys.json                   recovered .rod section keys
///       measurements/                   proven-on-car rows, one file per part number
///       sources.json                    where this project's data came from
///     extracted/                      the layout from before projects existed,
///     measured/                       moved into a project on the next `setup`
///                                     run and then gone — see `crate::migrate`
///   config.toml                       settings, and the marks `watch` keeps:
///                                     which project a bare command means, which
///                                     language names are shown in, favourites by VIN
///   names.csv                         the owner's own wording, by text id
///   cars/
///     XW8AD4NE9JH008917/              one directory per car, named for its VIN
///       car.json                      mass, tyre, measured road load
///       measures/2026-08-04-1241.json one saved session per file
///       reports/                      surveys, fault dumps, whatever is kept
/// ```
///
/// **`data/` and `cars/` are keyed differently on purpose.** A project is keyed
/// by VW's own project id (`SK37X`) and that names a **platform**, not a car —
/// spec §4.1: `SK37X` covers every Octavia III, Karoq and Kodiaq. That is why
/// the proven rows sit there: a proven scaling is a property of a *part number*,
/// true of every car carrying that part. `cars/` is keyed by the VIN the car
/// itself answers and holds what is true of one car and no other — its car
/// file, its drives, its survey.
///
/// **`data/` holds both the old layout and the new one, and they are told apart
/// by name.** `extracted` and `measured` are the pre-project directories;
/// anything else under `data/` is a project. Two rules enforce that rather than
/// assume it: [`crate::migrate::pending`] keys on those two names existing
/// rather than on `data/` existing, which is now always true, and
/// [`crate::project::folder_name`] refuses both as project names.
///
/// One dot-directory rather than each platform's convention.
/// `dirs::config_dir` would scatter this across `~/.config` and
/// `~/Library/Application Support`, which is right for an application bundle
/// and wrong for a tool whose files a person opens, reads and edits by hand.
///
/// A project's `cache.sqlite`, `names.json` and `rod-keys.json` are all
/// rebuildable from the source they came from, in minutes. Its `measurements/`
/// holds the `(identifier, raw form, factor, offset)` rows this project proved
/// on a vehicle; the label files provably cannot supply those
/// (`research/labels/rod-labels.md` §4.0c) and nothing but a car can recreate
/// them. That is the one distinction a reader of the tree needs, and why
/// `crate::migrate` copies before it removes.
///
/// Deliberately not built on [`resolve`]. That walks parent directories looking
/// for something that already exists, which is right for a path someone typed
/// and wrong for the tool's own directory: it would put a car's files in
/// whichever checkout the shell happened to be standing in.
pub fn vagcan_dir() -> anyhow::Result<PathBuf> {
	vagcan_dir_in(dirs::home_dir())
}

/// Where the projects live — `~/.vagcan/data/`, one directory per platform.
///
/// **The directory is called `data/`, not `projects/`.** It is what a person
/// opens looking for their car's data, and it is also where the pre-project
/// `extracted/` and `measured/` sat — so the two layouts are siblings now, and
/// the note on [`vagcan_dir`] says how they are told apart. The function keeps
/// the name of the *role* because that is what a caller means by it.
///
/// Named for what the *source* calls the vehicle, not for its VIN: an ODIS
/// project ships its own identifier (`SK37X`) and a VCDS-only project is named
/// by the person, and neither of them has ever seen a VIN. `crate::project`
/// owns what is inside one.
pub fn projects_dir() -> anyhow::Result<PathBuf> {
	Ok(vagcan_dir()?.join("data"))
}

/// The raw VCDS files every project reads at run time, kept once.
///
/// A `.rod` file is a property of a VCDS **build**, not of a car — the same
/// `TTTEXT.ROD` byte for byte serves every project parsed from that build — so
/// a copy per project would be tens of megabytes each to hold identical bytes.
/// `crate::project::rod_pool` is what creates it.
pub fn rod_pool_dir() -> anyhow::Result<PathBuf> {
	Ok(vagcan_dir()?.join("rod"))
}

/// A path the user gave, or this tool's own default for it.
///
/// The two halves are treated differently on purpose. What somebody typed is
/// used exactly as typed — [`resolve`] only fills in a relative path from the
/// places data is actually kept. A default is `~/.vagcan`'s and nothing else,
/// because a default that followed the working directory is what made these
/// commands work in a checkout and nowhere else.
pub fn or_default(given: Option<&str>, default: impl FnOnce() -> anyhow::Result<PathBuf>) -> anyhow::Result<PathBuf> {
	match given {
		Some(path) => Ok(resolve(path)),
		None => default(),
	}
}

/// Everything this tool keeps about one car, under its VIN and nothing else.
///
/// **The VIN is the whole name.** It is the one thing that identifies a car,
/// every unit that answers `F190` agrees on it, and it is the same seventeen
/// characters on the windscreen, the logbook and the invoice — so it is what a
/// person searching for their own files will actually type.
///
/// A readable half used to be prefixed — `1.8l-R4-TFSI-XW8AD4NE9JH008917`, from
/// whatever the engine called itself. It bought little and cost a real bug:
/// `measure setup` had the component string in hand and `measure` had not asked
/// for it, so one car got two directories, the car file in one and the sessions
/// in the other, and `--full` refused on a car that had been set up. A name
/// assembled from what each caller happens to know is not a name.
///
/// Directories named the old way are still found and still used — see
/// [`car_folder_in`]. Nothing is renamed.
pub fn car_dir(vin: &str) -> anyhow::Result<PathBuf> {
	let cars = vagcan_dir()?.join("cars");
	let folder = car_folder_in(&cars, vin)?;
	Ok(cars.join(folder))
}

/// Where a car's saved measurement sessions go.
pub fn measures_dir(vin: &str) -> anyhow::Result<PathBuf> {
	Ok(car_dir(vin)?.join("measures"))
}

/// The whole-car survey a car keeps for itself.
///
/// Named here for the same reason [`CAR_FILE`] is: two commands have to agree
/// on it or the cache is written where nothing reads it.
pub const SURVEY_FILE: &str = "survey.jsonl";

/// The survey `vagcan dev survey` last recorded off this car.
///
/// **Per car, not per part number.** Which identifiers a control unit answers
/// is a fact about that unit as it is built, coded and installed in *this* car;
/// `catalogs/` is the opposite — a proven scaling for a part number is true of
/// every car carrying that part. So this belongs beside the car file, keyed by
/// VIN, and never in the checkout.
///
/// It exists so that `watch` can offer every identifier the car answers without
/// the user having to remember a file name from an eight-minute sweep they ran
/// last week.
pub fn survey_cache(vin: &str) -> anyhow::Result<PathBuf> {
	Ok(car_dir(vin)?.join(SURVEY_FILE))
}

/// Where a car's dash plan is built — `~/.vagcan/dash/<VIN>/`: the hand-written
/// `dash.toml`, and the `plan.json` and `plan.rs` `vagcan dev dash build` writes
/// from it.
///
/// Keyed by VIN like `cars/`, because a plan is for one car — it carries the
/// part numbers the firmware checks itself against — and it is never in the
/// checkout, because everything in it is derived from VW's data.
pub fn dash_dir(vin: &str) -> anyhow::Result<PathBuf> {
	Ok(vagcan_dir()?.join("dash").join(car_folder(vin)?))
}

/// The VIN, checked hard enough to be a directory name.
///
/// It arrives from the bus, so it is not trusted as a path: a unit answering
/// with a separator or a `..` would otherwise choose where this tool writes.
/// Nothing is sanitised into shape — a VIN that is not the ISO 3779 alphabet is
/// refused, because a mangled one would silently file a car under a name no
/// later run could reproduce.
fn car_folder(vin: &str) -> anyhow::Result<String> {
	let vin = vin.trim();
	if vin.is_empty() || !vin.chars().all(|c| c.is_ascii_alphanumeric()) {
		anyhow::bail!("{vin:?} is not a VIN this tool will turn into a directory name");
	}
	Ok(vin.to_string())
}

/// The folder this car already has, or the one it would be given.
///
/// The rule behind [`car_dir`], with `cars/` passed in so it can be tested
/// without writing into the owner's own — the same reason [`vagcan_dir_in`]
/// takes a home directory.
///
/// **A directory named the old way is used, not renamed.** Anyone who ran this
/// tool before the name became the bare VIN has a `1.8l-R4-TFSI-<VIN>` holding
/// their car file and their drives, and a rename under a running tool is the
/// one operation here that can lose data: `watch` may have a file open in that
/// directory, a second `vagcan` may be writing a session into it, and every
/// path either of them resolved earlier stops pointing at anything. So the tail
/// is still matched, the old directory still wins, and `mv` still works for
/// anyone who wants the tidier name — this function will follow it.
pub fn car_folder_in(cars: &Path, vin: &str) -> anyhow::Result<String> {
	let wanted = car_folder(vin)?;
	Ok(existing_folder(cars, &wanted).unwrap_or(wanted))
}

/// A directory this VIN already has, whatever it happens to be called.
///
/// The names are the bare VIN or, from before, `<slug>-<VIN>`, so the tail is
/// matched on the whole VIN with its separator: a VIN is fixed-length and one
/// car's must never match another's.
///
/// More than one is the state the two-directory bug left behind, so the order
/// is decided rather than left to the filesystem: the folder holding the car
/// file is the car — that file is what makes a car *described*, and losing
/// sight of it is the failure being fixed — then the bare VIN, which is what a
/// fresh run would create, then the first by name so that two runs with nothing
/// to choose between them still agree.
fn existing_folder(cars: &Path, vin: &str) -> Option<String> {
	let tail = format!("-{vin}");
	let Ok(entries) = std::fs::read_dir(cars) else { return None };
	let mut found: Vec<String> = entries
		.flatten()
		.filter(|entry| entry.path().is_dir())
		.filter_map(|entry| entry.file_name().into_string().ok())
		.filter(|name| name == vin || name.ends_with(&tail))
		.collect();
	found.sort();
	found
		.iter()
		.find(|name| cars.join(name).join(CAR_FILE).is_file())
		.or_else(|| found.iter().find(|name| *name == vin))
		.or_else(|| found.first())
		.cloned()
}

/// The rule behind [`vagcan_dir`], with the home directory passed in so it can
/// be tested without a process-wide `set_var` that the other tests would race.
fn vagcan_dir_in(home: Option<PathBuf>) -> anyhow::Result<PathBuf> {
	let home = home
		.filter(|p| p.is_absolute())
		.ok_or_else(|| anyhow::anyhow!("no home directory to write to — set HOME, or pass an explicit path"))?;
	Ok(home.join(".vagcan"))
}

/// Where to look, in order: the working directory and its parents, then the
/// directory holding the executable and its parents (a `target/debug` build
/// sits three levels below the checkout root).
fn search_roots() -> Vec<PathBuf> {
	let mut roots = Vec::new();
	if let Ok(cwd) = std::env::current_dir() {
		push_with_parents(&mut roots, &cwd);
	}
	if let Ok(exe) = std::env::current_exe() {
		if let Some(dir) = exe.parent() {
			push_with_parents(&mut roots, dir);
		}
	}
	roots
}

fn push_with_parents(roots: &mut Vec<PathBuf>, from: &Path) {
	let mut at = Some(from);
	for _ in 0..=MAX_PARENTS {
		let Some(dir) = at else { break };
		if !roots.iter().any(|r| r == dir) {
			roots.push(dir.to_path_buf());
		}
		at = dir.parent();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A unique-per-test temp dir, cleaned up on drop — the shape the rest of
	/// this crate's file tests use. Nothing here may write into the owner's
	/// own `~/.vagcan`, which is the whole reason [`car_folder_in`] takes the
	/// `cars/` directory as an argument.
	struct TempDir(PathBuf);

	impl TempDir {
		fn new(tag: &str) -> TempDir {
			let path = std::env::temp_dir().join(format!("vagcan-datadir-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
			let _ = std::fs::remove_dir_all(&path);
			std::fs::create_dir_all(&path).unwrap();
			TempDir(path)
		}

		/// A car directory as some earlier run left it.
		fn car(&self, name: &str) -> PathBuf {
			let path = self.0.join(name);
			std::fs::create_dir_all(&path).unwrap();
			path
		}
	}

	impl Drop for TempDir {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir_all(&self.0);
		}
	}

	#[test]
	fn a_car_that_already_has_a_directory_does_not_get_a_second_one() {
		// The reported bug: `measure` named the folder from the VIN alone,
		// `measure setup` named it from the VIN and the engine's component
		// string, and one car ended up with the car file in one directory and
		// its sessions in the other.
		let cars = TempDir::new("second");
		let vin = "XW8AD4NE9JH008917";
		cars.car(vin);
		let folder = car_folder_in(&cars.0, vin).unwrap();
		assert_eq!(folder, vin);
		std::fs::create_dir_all(cars.0.join(&folder)).unwrap();
		assert_eq!(std::fs::read_dir(&cars.0).unwrap().count(), 1, "one car, one directory");
	}

	#[test]
	fn a_directory_from_before_the_rename_is_used_rather_than_orphaned() {
		// Anyone who ran this tool before the folder name became the bare VIN
		// has their car file and their drives under the old name. A rename
		// under a running tool can lose a drive somebody is recording; a stale
		// folder name costs nothing.
		let cars = TempDir::new("rename");
		let vin = "XW8AD4NE9JH008917";
		cars.car("old-name-XW8AD4NE9JH008917");
		assert_eq!(car_folder_in(&cars.0, vin).unwrap(), "old-name-XW8AD4NE9JH008917");
	}

	#[test]
	fn a_car_with_no_directory_yet_is_named_for_its_vin() {
		let cars = TempDir::new("first");
		assert_eq!(car_folder_in(&cars.0, "XW8AD4NE9JH008917").unwrap(), "XW8AD4NE9JH008917");
		// And a `cars/` that does not exist yet is simply a car nobody has met.
		assert_eq!(
			car_folder_in(&cars.0.join("not-created"), "XW8AD4NE9JH008917").unwrap(),
			"XW8AD4NE9JH008917"
		);
	}

	#[test]
	fn only_the_whole_vin_at_the_tail_is_another_car() {
		// A VIN is fixed-length, so the tail is matched with its separator: a
		// shorter one must never adopt a longer one's directory.
		let cars = TempDir::new("tail");
		cars.car("1.8l-R4-TFSI-XW8AD4NE9JH008917");
		assert_eq!(car_folder_in(&cars.0, "XW8AD4NE9JH008918").unwrap(), "XW8AD4NE9JH008918");
		assert_eq!(car_folder_in(&cars.0, "JH008917").unwrap(), "JH008917");
	}

	#[test]
	fn when_one_car_has_two_directories_the_one_holding_the_car_file_wins() {
		// The state this bug left on cars set up before it was fixed. Both
		// callers have to land on the described car, whichever name each of
		// them would have chosen, or `--full` goes on refusing.
		let cars = TempDir::new("two");
		let vin = "XW8AD4NE9JH008917";
		cars.car(vin);
		let described = cars.car("1.8l-R4-TFSI-XW8AD4NE9JH008917");
		std::fs::write(described.join(CAR_FILE), "{}").unwrap();
		assert_eq!(car_folder_in(&cars.0, vin).unwrap(), "1.8l-R4-TFSI-XW8AD4NE9JH008917");
		assert_eq!(car_folder_in(&cars.0, vin).unwrap(), "1.8l-R4-TFSI-XW8AD4NE9JH008917");
	}

	#[test]
	fn a_path_that_exists_where_it_was_typed_is_used_as_typed() {
		// The user's own argument is never second-guessed.
		let here = std::env::current_dir().unwrap();
		let name = here.join("Cargo.toml");
		assert_eq!(resolve(name.to_str().unwrap()), name);
	}

	#[test]
	fn a_relative_path_someone_typed_is_found_from_a_subdirectory() {
		// What `resolve` is still for. It is no longer how any *default* is
		// found — those are all under `~/.vagcan` now — but a path typed at a
		// shell standing three directories deep still has to mean what it
		// looks like it means.
		let deep = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
		let found = deep
			.canonicalize()
			.map(|dir| {
				let mut roots = Vec::new();
				push_with_parents(&mut roots, &dir);
				roots.iter().any(|r| r.join("crates").exists())
			})
			.unwrap_or(false);
		assert!(found, "crates/ must be reachable by walking up from a source directory");
	}

	#[test]
	fn a_missing_file_comes_back_unchanged_so_the_error_names_it() {
		assert_eq!(
			resolve("nowhere/definitely-not-here.json").to_str(),
			Some("nowhere/definitely-not-here.json")
		);
	}

	#[test]
	fn a_car_file_never_lands_inside_the_repository() {
		// A VIN-keyed file must not end up in a checkout, one `git add` away
		// from being published.
		let dir = car_dir("XW8AD4NE9JH008917").expect("a home directory exists in any environment that runs tests");
		let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
		assert!(!dir.starts_with(&repo), "{dir:?} is inside {repo:?}");
	}

	#[test]
	fn everything_this_tool_writes_lives_in_one_dot_directory() {
		let dir = vagcan_dir_in(Some(PathBuf::from("/home/someone"))).unwrap();
		assert_eq!(dir, Path::new("/home/someone/.vagcan"));
	}

	#[test]
	fn nothing_the_tool_reads_by_default_is_looked_for_in_a_checkout() {
		// The failure this layout exists for. Every one of these used to be a
		// path relative to the working directory, so the tool worked from the
		// repository root and nowhere else — and after `cargo install` there is
		// no repository root at all.
		let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
		let home = vagcan_dir().unwrap();
		for path in [projects_dir().unwrap(), rod_pool_dir().unwrap(), crate::config::path().unwrap()] {
			assert!(!path.starts_with(&repo), "{path:?} is inside the checkout");
			assert!(path.starts_with(&home), "{path:?} is outside ~/.vagcan");
		}
	}

	#[test]
	fn the_one_path_this_crate_does_not_own_is_still_under_the_dot_directory() {
		// `vag_uds_client` reads the number-to-id override itself, by a path it
		// owns and this crate cannot change without a file somebody wrote by
		// hand being silently ignored. `crate::migrate` leaves it exactly where
		// it is for that reason; all that is asserted here is that it is still
		// inside `~/.vagcan` and not in a checkout.
		let owned = Path::new(vag_uds_client::address::OVERRIDE_PATH);
		assert!(owned.starts_with(".vagcan"), "{owned:?}");
		assert!(!owned.is_absolute(), "it is joined onto the home directory: {owned:?}");
	}

	#[test]
	fn a_path_the_user_gave_beats_the_default_and_is_not_second_guessed() {
		let typed = or_default(Some("Cargo.toml"), || unreachable!("the default was consulted")).unwrap();
		assert!(typed.ends_with("Cargo.toml"), "{typed:?}");
		assert_eq!(or_default(None, crate::config::path).unwrap(), crate::config::path().unwrap());
	}

	#[test]
	fn a_car_folder_is_its_vin_and_nothing_else() {
		// The readable prefix is gone. It was assembled from whatever the
		// caller happened to know about the car, which is how one car came to
		// have two directories; and the VIN is what a person looking for their
		// own files reads off the windscreen anyway.
		assert_eq!(car_folder("XW8AD4NE9JH008917").unwrap(), "XW8AD4NE9JH008917");
		assert_eq!(car_folder("  XW8AD4NE9JH008917  ").unwrap(), "XW8AD4NE9JH008917");
	}

	#[test]
	fn nothing_off_the_bus_gets_to_choose_where_this_tool_writes() {
		// A unit is free to answer with anything at all, including a path. A
		// VIN that is not the ISO 3779 alphabet is refused rather than
		// sanitised: a mangled one would file the car under a name no later run
		// could reproduce, which loses it as surely as writing outside the
		// directory would.
		for not_a_vin in ["../../etc", "XW8AD4NE9JH00 8917", "", "XW8/AD4", "a\\b"] {
			assert!(car_folder(not_a_vin).is_err(), "{not_a_vin:?} was accepted");
		}
	}

	#[test]
	fn a_cars_files_all_live_under_that_car() {
		// Sessions belong to the car they were read from, not to the working
		// directory a command happened to be run in.
		let vin = "XW8AD4NE9JH008917";
		let car = car_dir(vin).unwrap();
		assert_eq!(measures_dir(vin).unwrap(), car.join("measures"));
		assert_eq!(survey_cache(vin).unwrap(), car.join(SURVEY_FILE));
	}

	#[test]
	fn a_survey_is_cached_per_car_because_identifiers_are_per_car() {
		// Which identifiers a unit answers is a fact about that unit in that
		// car, so the cache is keyed the same way everything else about a car
		// is — by VIN, outside the checkout.
		let a = survey_cache("XW8AD4NE9JH008917").unwrap();
		let b = survey_cache("XW8AD4NE9JH008918").unwrap();
		assert_ne!(a, b);
		assert!(a.ends_with("XW8AD4NE9JH008917/survey.jsonl"), "{a:?}");
		let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
		assert!(!a.starts_with(&repo), "{a:?} is inside the checkout");
		// A VIN off the bus never chooses where this tool writes.
		assert!(survey_cache("../../etc").is_err());
	}

	#[test]
	fn a_relative_home_is_ignored_because_it_would_follow_the_shell() {
		// Resolving against the working directory is the same "wherever the
		// shell is standing" failure this module exists to avoid.
		assert!(vagcan_dir_in(Some(PathBuf::from("relative/home"))).is_err());
	}

	#[test]
	fn with_nowhere_to_write_the_error_says_what_to_set() {
		let err = vagcan_dir_in(None).unwrap_err().to_string();
		assert!(err.contains("HOME"), "{err}");
	}
}
