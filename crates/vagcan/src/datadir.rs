//! Finding the data files, wherever the command was run from.
//!
//! Catalogs, recovered names and per-car tables all live in `catalogs/`. Every
//! default path used to be written relative to the working directory, so the
//! tool worked from the repository root and quietly did nothing anywhere else
//! — `watch` in particular showed an empty screen and left the impression the
//! car was at fault.
//!
//! The rule here: a path the user typed is used exactly as typed. A *default*
//! is looked for next to the working directory, then upwards from it, then
//! next to the executable — the three places the data actually is, whether
//! the tool is run from a checkout, from a subdirectory of one, or from an
//! installed copy.

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
///   config.json                       settings that are not about one car
///   cars/
///     1.8l-R4-TFSI-XW8AD4NE9JH008917/ one directory per car
///       car.json                      mass, tyre, measured road load
///       measures/2026-08-04-1241.json  one saved session per file
///       reports/                       surveys, fault dumps, whatever is kept
/// ```
///
/// One dot-directory rather than each platform's convention.
/// `dirs::config_dir` would scatter this across `~/.config` and
/// `~/Library/Application Support`, which is right for an application bundle
/// and wrong for a tool whose files a person opens, reads and edits by hand.
///
/// Not `catalogs/`. That is a checked-in corpus keyed by part number, where a
/// measurement proven on one `0CW300041G` is true of every `0CW300041G` in the
/// world. Everything here is the opposite in kind — a personal identifier, one
/// owner's answers, one physical car — and this repository already works to
/// keep VINs out of git.
///
/// Deliberately not built on [`resolve`]. That walks parent directories looking
/// for something that already exists, which is right for *reading* the corpus
/// and wrong for *writing*: it would put a car's files in whichever checkout
/// the shell happened to be standing in.
pub fn vagcan_dir() -> anyhow::Result<PathBuf> {
    vagcan_dir_in(dirs::home_dir())
}

/// Everything this tool keeps about one car.
///
/// The directory is named for what the car said about itself and then its VIN:
/// `1.8l-R4-TFSI-XW8AD4NE9JH008917`. The VIN alone is unambiguous and
/// unreadable; the description alone is neither unique nor stable. Together the
/// owner of two cars can tell them apart at a glance, and the tool can still
/// find the right one by matching the tail.
///
/// **The VIN identifies the car; the description only decorates the folder.**
/// A directory whose name already ends in this VIN *is* this car's directory,
/// whatever the caller happens to know about the car this time — because the
/// callers do not know the same things. `measure setup` has the engine's
/// component string in hand and `measure` did not ask for it, and naming the
/// folder from each in turn gave one car two directories: the car file in one,
/// the sessions in the other, and `--full` refusing on a car that had been set
/// up. Matching the tail is what the paragraph above always promised.
///
/// `description` is what a control unit reported — the engine's component
/// string, usually. No make or model appears here, because a car does not
/// broadcast one and this tool does not invent data it was not given.
pub fn car_dir(vin: &str, description: Option<&str>) -> anyhow::Result<PathBuf> {
    let cars = vagcan_dir()?.join("cars");
    let folder = car_folder_in(&cars, vin, description)?;
    Ok(cars.join(folder))
}

/// Where a car's saved measurement sessions go.
// Called once `measure` writes sessions; the directory layout is settled here so
// that the command does not invent its own.
#[allow(dead_code)]
pub fn measures_dir(vin: &str, description: Option<&str>) -> anyhow::Result<PathBuf> {
    Ok(car_dir(vin, description)?.join("measures"))
}

/// Where the readings a car is asked for end up when nobody named a file — a
/// survey, a fault dump. They belong to the car, not to the working directory
/// the command happened to be run from.
// Called once `survey` defaults its output here — `todo/README.md` carries that
// item; the layout is settled now so the command does not invent its own.
#[allow(dead_code)]
pub fn reports_dir(vin: &str, description: Option<&str>) -> anyhow::Result<PathBuf> {
    Ok(car_dir(vin, description)?.join("reports"))
}

/// The folder name for one car, safe to write and readable to a person.
///
/// The VIN arrives from the bus and the description does too, so neither is
/// trusted as a path: a unit answering with a separator or a `..` would
/// otherwise choose where this tool writes. Anything that is not a letter, a
/// digit or `.` becomes a hyphen, runs of hyphens collapse, and the VIN must
/// still be the ISO 3779 alphabet after that.
fn car_folder(vin: &str, description: Option<&str>) -> anyhow::Result<String> {
    let vin = vin.trim();
    if vin.is_empty() || !vin.chars().all(|c| c.is_ascii_alphanumeric()) {
        anyhow::bail!("{vin:?} is not a VIN this tool will turn into a directory name");
    }
    let mut name = String::new();
    if let Some(text) = description {
        name = slug(text);
        if !name.is_empty() {
            name.push('-');
        }
    }
    name.push_str(vin);
    Ok(name)
}

/// The folder this car already has, or the one it would be given.
///
/// The rule behind [`car_dir`], with `cars/` passed in so it can be tested
/// without writing into the owner's own — the same reason [`vagcan_dir_in`]
/// takes a home directory.
///
/// **An existing folder is never renamed to match a new description.** A rename
/// under a running tool is the one operation here that can lose data: `watch`
/// may have a file open in that directory, a second `vagcan` may be writing a
/// session into it, and every path any of them resolved earlier stops pointing
/// at anything. The folder name is decoration and the VIN is identity, so a
/// mismatch costs a slightly stale name and nothing else. Someone who wants the
/// nicer name can `mv` the directory, and this function will follow it.
///
/// Reachable from the crate so that a caller's tests can assert what a car gets
/// *named* without asking what it is *called now* — the second question needs
/// the owner's real `~/.vagcan` and answers differently on a car that already
/// has a directory, which is the whole point of this function.
pub(crate) fn car_folder_in(
    cars: &Path,
    vin: &str,
    description: Option<&str>,
) -> anyhow::Result<String> {
    let wanted = car_folder(vin, description)?;
    Ok(existing_folder(cars, &wanted, vin).unwrap_or(wanted))
}

/// A directory this VIN already has, whatever it happens to be called.
///
/// The names are `<slug>-<VIN>` or the bare VIN, so the tail is matched on the
/// whole VIN with its separator: a VIN is fixed-length and one car's must never
/// match another's.
///
/// More than one is the state this bug left behind on cars that were set up
/// before it was fixed, so the order is decided rather than left to the
/// filesystem: the folder holding the car file is the car — that file is what
/// makes a car *described*, and losing sight of it is the failure being fixed —
/// then the name that was asked for, then the first by name so that two runs
/// with nothing to choose between them still agree.
fn existing_folder(cars: &Path, wanted: &str, vin: &str) -> Option<String> {
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
        .or_else(|| found.iter().find(|name| *name == wanted))
        .or_else(|| found.first())
        .cloned()
}

/// Readable, and safe as one path component.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '.' {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_matches(['-', '.']).to_string()
}

/// The rule behind [`vagcan_dir`], with the home directory passed in so it can
/// be tested without a process-wide `set_var` that the other tests would race.
fn vagcan_dir_in(home: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    let home = home.filter(|p| p.is_absolute()).ok_or_else(|| {
        anyhow::anyhow!("no home directory to write to — set HOME, or pass an explicit path")
    })?;
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
            let path = std::env::temp_dir().join(format!(
                "vagcan-datadir-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
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
        let folder = car_folder_in(&cars.0, vin, Some("1.8l R4 TFSI")).unwrap();
        assert_eq!(folder, vin);
        std::fs::create_dir_all(cars.0.join(&folder)).unwrap();
        assert_eq!(std::fs::read_dir(&cars.0).unwrap().count(), 1, "one car, one directory");
    }

    #[test]
    fn a_directory_is_never_renamed_to_match_a_later_description() {
        // A rename under a running tool can lose data; a stale folder name
        // cannot. The description decorates, the VIN identifies.
        let cars = TempDir::new("rename");
        let vin = "XW8AD4NE9JH008917";
        cars.car("old-name-XW8AD4NE9JH008917");
        assert_eq!(
            car_folder_in(&cars.0, vin, Some("Škoda Octavia III")).unwrap(),
            "old-name-XW8AD4NE9JH008917"
        );
    }

    #[test]
    fn a_car_with_no_directory_yet_is_named_for_what_it_said_about_itself() {
        let cars = TempDir::new("first");
        assert_eq!(
            car_folder_in(&cars.0, "XW8AD4NE9JH008917", Some("1.8l R4 TFSI")).unwrap(),
            "1.8l-R4-TFSI-XW8AD4NE9JH008917"
        );
        // And a `cars/` that does not exist yet is simply a car nobody has met.
        assert_eq!(
            car_folder_in(&cars.0.join("not-created"), "XW8AD4NE9JH008917", None).unwrap(),
            "XW8AD4NE9JH008917"
        );
    }

    #[test]
    fn only_the_whole_vin_at_the_tail_is_another_car() {
        // A VIN is fixed-length, so the tail is matched with its separator: a
        // shorter one must never adopt a longer one's directory.
        let cars = TempDir::new("tail");
        cars.car("1.8l-R4-TFSI-XW8AD4NE9JH008917");
        assert_eq!(
            car_folder_in(&cars.0, "XW8AD4NE9JH008918", None).unwrap(),
            "XW8AD4NE9JH008918"
        );
        assert_eq!(car_folder_in(&cars.0, "JH008917", None).unwrap(), "JH008917");
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
        assert_eq!(car_folder_in(&cars.0, vin, None).unwrap(), "1.8l-R4-TFSI-XW8AD4NE9JH008917");
        assert_eq!(
            car_folder_in(&cars.0, vin, Some("1.8l R4 TFSI")).unwrap(),
            "1.8l-R4-TFSI-XW8AD4NE9JH008917"
        );
    }

    #[test]
    fn a_path_that_exists_where_it_was_typed_is_used_as_typed() {
        // The user's own argument is never second-guessed.
        let here = std::env::current_dir().unwrap();
        let name = here.join("Cargo.toml");
        assert_eq!(resolve(name.to_str().unwrap()), name);
    }

    #[test]
    fn a_default_is_found_from_a_subdirectory() {
        // The failure this module exists for: `catalogs/…` resolves from the
        // repository root and from anywhere inside it, because that is where
        // the data is regardless of where the shell happens to be.
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let deep = root.join("crates/vagcan/src");
        let found = deep
            .canonicalize()
            .map(|dir| {
                let mut roots = Vec::new();
                push_with_parents(&mut roots, &dir);
                roots.iter().any(|r| r.join("catalogs").exists())
            })
            .unwrap_or(false);
        assert!(found, "catalogs/ must be reachable by walking up from a source directory");
    }

    #[test]
    fn a_missing_file_comes_back_unchanged_so_the_error_names_it() {
        assert_eq!(resolve("catalogs/definitely-not-here.json").to_str(), Some("catalogs/definitely-not-here.json"));
    }

    #[test]
    fn a_car_file_never_lands_inside_the_repository() {
        // The whole point of the writer-side sibling: a VIN-keyed file must not
        // end up in a checkout, one `git add` away from being published.
        let dir = car_dir("XW8AD4NE9JH008917", Some("1.8l R4 TFSI"))
            .expect("a home directory exists in any environment that runs tests");
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        assert!(!dir.starts_with(&repo), "{dir:?} is inside {repo:?}");
        assert!(!dir.iter().any(|part| part == "catalogs"), "{dir:?} is in the shared corpus");
    }

    #[test]
    fn everything_this_tool_writes_lives_in_one_dot_directory() {
        let dir = vagcan_dir_in(Some(PathBuf::from("/home/someone"))).unwrap();
        assert_eq!(dir, Path::new("/home/someone/.vagcan"));
    }

    #[test]
    fn a_car_folder_reads_as_the_car_and_ends_in_its_vin() {
        // Both halves earn their place: the VIN is unambiguous and unreadable,
        // the description is readable and neither unique nor stable.
        assert_eq!(
            car_folder("XW8AD4NE9JH008917", Some("1.8l R4 TFSI")).unwrap(),
            "1.8l-R4-TFSI-XW8AD4NE9JH008917"
        );
        // A car that would not describe itself still gets a directory.
        assert_eq!(
            car_folder("XW8AD4NE9JH008917", None).unwrap(),
            "XW8AD4NE9JH008917"
        );
    }

    #[test]
    fn nothing_off_the_bus_gets_to_choose_where_this_tool_writes() {
        // A unit is free to answer with anything at all, including a path.
        assert!(car_folder("../../etc", Some("x")).is_err());
        assert!(car_folder("XW8AD4NE9JH00 8917", None).is_err());
        assert!(car_folder("", None).is_err());
        let folder = car_folder("XW8AD4NE9JH008917", Some("../../etc/passwd")).unwrap();
        assert!(!folder.contains('/'), "{folder}");
        assert!(!folder.contains(".."), "{folder}");
    }

    #[test]
    fn a_cars_files_all_live_under_that_car() {
        // Sessions and reports belong to the car they were read from, not to
        // the working directory a command happened to be run in.
        let vin = "XW8AD4NE9JH008917";
        let car = car_dir(vin, Some("1.8l R4 TFSI")).unwrap();
        assert_eq!(measures_dir(vin, Some("1.8l R4 TFSI")).unwrap(), car.join("measures"));
        assert_eq!(reports_dir(vin, Some("1.8l R4 TFSI")).unwrap(), car.join("reports"));
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
