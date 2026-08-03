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
///       races/2026-08-04-1241.json     one saved session per file
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
/// `description` is what a control unit reported — the engine's component
/// string, usually. No make or model appears here, because a car does not
/// broadcast one and this tool does not invent data it was not given.
pub fn car_dir(vin: &str, description: Option<&str>) -> anyhow::Result<PathBuf> {
    Ok(vagcan_dir()?.join("cars").join(car_folder(vin, description)?))
}

/// Where a car's saved race sessions go.
// Called once `race` writes sessions; the directory layout is settled here so
// that the command does not invent its own.
#[allow(dead_code)]
pub fn races_dir(vin: &str, description: Option<&str>) -> anyhow::Result<PathBuf> {
    Ok(car_dir(vin, description)?.join("races"))
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
        assert_eq!(races_dir(vin, Some("1.8l R4 TFSI")).unwrap(), car.join("races"));
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
