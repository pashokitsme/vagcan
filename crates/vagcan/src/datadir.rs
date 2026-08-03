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

/// Where this tool keeps its own configuration — one directory, `~/.config/vagcan`.
///
/// The car files live under `cars/` inside it: one per VIN, holding what an
/// owner typed about their car and what the coastdown measured on it.
///
/// Not `catalogs/`. That is a checked-in corpus keyed by part number, where a
/// measurement proven on one `0CW300041G` is true of every `0CW300041G` in the
/// world. A car file is the opposite in kind — a personal identifier, one
/// owner's answers, one physical car on one day — and this repository already
/// works to keep VINs out of git.
///
/// Deliberately not built on [`resolve`]. That walks parent directories looking
/// for something that already exists, which is right for *reading* the corpus
/// and wrong for *writing*: it would put a car file in whichever checkout the
/// shell happened to be standing in.
///
/// `~/.config` on every platform rather than each system's own convention. The
/// `dirs` crate would give `~/Library/Application Support` on macOS, which is
/// where a Mac application belongs and not where someone who edits their tools'
/// config files by hand will look for it. `XDG_CONFIG_HOME` is honoured when it
/// is absolute; a relative one is ignored, since resolving it against the
/// working directory is exactly the "wherever the shell is standing" failure
/// this function exists to avoid.
pub fn config_dir() -> anyhow::Result<PathBuf> {
    config_dir_from(std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from), dirs::home_dir())
}

/// Where the per-VIN car files live.
pub fn car_dir() -> anyhow::Result<PathBuf> {
    Ok(config_dir()?.join("cars"))
}

/// The rule behind [`config_dir`], with the environment passed in so it can be
/// tested without a process-wide `set_var` that the other tests would race.
fn config_dir_from(config_home: Option<PathBuf>, home: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(base) = config_home.filter(|p| p.is_absolute()) {
        return Ok(base.join("vagcan"));
    }
    let home = home.filter(|p| p.is_absolute()).ok_or_else(|| {
        anyhow::anyhow!(
            "no home directory to write to — set HOME or XDG_CONFIG_HOME, or pass an explicit path"
        )
    })?;
    Ok(home.join(".config").join("vagcan"))
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
        let dir = car_dir().expect("a home directory exists in any environment that runs tests");
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
        assert!(!dir.starts_with(&repo), "{dir:?} is inside {repo:?}");
        assert!(!dir.iter().any(|part| part == "catalogs"), "{dir:?} is in the shared corpus");
    }

    #[test]
    fn a_relative_config_home_is_ignored_because_it_would_follow_the_shell() {
        let dir =
            config_dir_from(Some(PathBuf::from("relative/config")), Some(PathBuf::from("/home/someone")))
                .unwrap();
        assert_eq!(dir, Path::new("/home/someone/.config/vagcan"));
    }

    #[test]
    fn an_absolute_config_home_wins() {
        let dir =
            config_dir_from(Some(PathBuf::from("/etc/xdg")), Some(PathBuf::from("/home/someone")))
                .unwrap();
        assert_eq!(dir, Path::new("/etc/xdg/vagcan"));
    }

    #[test]
    fn every_platform_gets_the_same_dot_config_path() {
        // Not each system's own convention: `dirs` would put this in
        // `~/Library/Application Support` on a Mac, which is right for an
        // application bundle and wrong for a file someone edits by hand.
        let dir = config_dir_from(None, Some(PathBuf::from("/Users/someone"))).unwrap();
        assert_eq!(dir, Path::new("/Users/someone/.config/vagcan"));
        assert_eq!(
            config_dir_from(None, Some(PathBuf::from("/home/someone"))).unwrap(),
            Path::new("/home/someone/.config/vagcan")
        );
    }

    #[test]
    fn with_nowhere_to_write_the_error_says_what_to_set() {
        let err = config_dir_from(None, None).unwrap_err().to_string();
        assert!(err.contains("HOME") && err.contains("XDG_CONFIG_HOME"), "{err}");
    }
}
