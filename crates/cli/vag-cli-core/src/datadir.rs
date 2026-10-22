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
//! shape. A path the user typed is used exactly as typed: a relative one is
//! relative to the working directory, as for any command.

use std::path::{Path, PathBuf};

/// What a described car has in its directory, and nothing else does.
///
/// Named here because this module decides the layout — `measure::carfile` joins
/// the same name onto [`car_dir`].
pub const CAR_FILE: &str = "car.json";

/// This tool's own directory, `~/.vagcan`.
///
/// ```text
/// ~/.vagcan/
///   rod/                              raw VCDS files read at run time, shared
///                                     across every project: the .rod files and
///                                     this build's fault text
///   vcds/                             the machine's one VCDS installation, read for
///                                     every car — [`vcds_dir`]
///     cache.sqlite                      its label files and registry rows, and its
///                                       `source` row; the schema a project's cache has
///     names.json                        text id -> name
///     odx-ids.json                      text id -> the IDE/MAS id that text names
///     rod-keys.json                     recovered .rod section keys
///     registry.json                     which installation the channels were read
///                                       from, and what came of each unit — `crate::registry`
///   data/
///     SK37X/                          one ODIS project per *platform* — `crate::project`
///       cache.sqlite                    its channels and fault text, queryable, and its
///                                       `source` row
///       names-odis.json                 text id -> name, from the ODIS project
///       measurements/                   proven-on-car rows, one file per part number
///   config.toml                       settings, and the marks `watch` keeps:
///                                     which project a bare command means, which
///                                     language names are shown in, favourites by VIN
///   names.csv                         the owner's own wording, by text id
///   cars/
///     XW8AD4NE9JH008917/              one directory per car, named for its VIN
///       units.json                    its control units, as each said what it is
///                                     (`crate::units`), written by `watch`, `measure`
///                                     and `units --identify`
///       car.json                      mass, tyre, measured road load
///       measures/2026-08-04-1241.json one saved session per file
///   dash/
///     XW8AD4NE9JH008917/              one car's dash — [`dash_dir`]
///       dash.toml                     its pages and channels, written by hand
///       plan.json, plan.rs            what `vagcan dev dash build` writes from it
///       editor/                       what `vagcan dev dash edit` serves — [`dash_editor_dir`]
/// ```
///
/// **`data/` and `cars/` are keyed differently on purpose.** A project is keyed
/// by VW's own project id (`SK37X`) and that names a **platform**, not a car —
/// spec §4.1: `SK37X` covers every Octavia III, Karoq and Kodiaq. That is why
/// the proven rows sit there: a proven scaling is a property of a *part number*,
/// true of every car carrying that part. `cars/` is keyed by the VIN the car
/// itself answers and holds what is true of one car and no other — its car
/// file, its drives, the record of its units.
///
/// **Every directory under `data/` is a project**, whatever it is called, and
/// nothing else there is read. A machine with no project has nothing to read
/// from, and the remedy is `vagcan setup`.
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
/// (`.archive/research/labels/rod-labels.md` §4.0c) and nothing but a car can recreate
/// them. That is the one distinction a reader of the tree needs.
///
/// ## Overrides: [`ROOT_ENV`], and a test build
///
/// `VAGCAN_HOME`, when set, **is** that directory — what `~/.vagcan` holds, under
/// another name — and HOME is not asked. It must be absolute: a relative one would
/// follow the shell, as a relative HOME would. For a second set of data on one
/// machine, and for a process a test starts (`.env(ROOT_ENV, …)`), which must never
/// read the person's own.
///
/// **A test build never looks at HOME.** Built for tests — this crate's own, or another
/// crate's through the `test-root` feature its dev-dependency turns on — the default is
/// one temporary directory per process ([`test_root`]). Since `cache.sqlite` is upgraded
/// in place when it is opened, a test that reached `~/.vagcan` was writing the person's
/// data: `cargo test --workspace` upgraded the reference machine's cache (2026-10-20).
/// One switch here covers every test, including the ones nobody thought reached it.
pub fn vagcan_dir() -> anyhow::Result<PathBuf> {
	if let Some(root) = std::env::var_os(ROOT_ENV).filter(|root| !root.is_empty()) {
		let root = PathBuf::from(root);
		if !root.is_absolute() {
			use crate::report::Advice as _;
			return Err(
				anyhow::anyhow!("{ROOT_ENV}={} is not an absolute path", root.display())
					.note("a relative one would mean another directory from every folder")
					.help(format!("set {ROOT_ENV} to an absolute path, or unset it")),
			);
		}
		return Ok(root);
	}
	default_root()
}

/// `~/.vagcan`, out of HOME.
#[cfg(not(any(test, feature = "test-root")))]
fn default_root() -> anyhow::Result<PathBuf> {
	vagcan_dir_in(dirs::home_dir())
}

/// A test build's, which is never HOME's ([`test_root`]).
#[cfg(any(test, feature = "test-root"))]
fn default_root() -> anyhow::Result<PathBuf> {
	Ok(test_root().to_path_buf())
}

/// The line a binary built for tests says at startup, or `None` for a normal build.
///
/// `cargo test` leaves a `target/debug/vagcan` built with `test-root`, and that binary
/// reads a temporary directory, not `~/.vagcan`. Run by hand, it must say so rather than
/// report a machine with no data.
pub fn test_build_note() -> Option<String> {
	if !cfg!(any(test, feature = "test-root")) {
		return None;
	}
	let dir = vagcan_dir().map(|dir| dir.display().to_string()).unwrap_or_else(|e| format!("({e})"));
	Some(format!("note  test build: data dir {dir}, not ~/.vagcan"))
}

/// The environment variable that names the directory `~/.vagcan` stands for.
pub const ROOT_ENV: &str = "VAGCAN_HOME";

/// A test build's `~/.vagcan`: one empty temporary directory for the life of the process,
/// created on first use. Every test in one binary shares it, as they shared HOME before;
/// a test that needs its own data writes it into a directory of its own.
///
/// All of them under one parent, `vagcan-test-roots/<pid>/`, which the first test of each
/// process clears of the roots older than [`TEST_ROOT_AGE`]: a process leaves its root
/// behind — nothing runs at a test binary's exit — and a process still running has not
/// been running that long, so another binary's live root is never taken from under it.
#[cfg(any(test, feature = "test-root"))]
pub fn test_root() -> &'static Path {
	static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
	ROOT.get_or_init(|| {
		let parent = std::env::temp_dir().join("vagcan-test-roots");
		remove_old_roots(&parent, TEST_ROOT_AGE);
		let root = parent.join(std::process::id().to_string()).join(".vagcan");
		let _ = std::fs::remove_dir_all(&root);
		std::fs::create_dir_all(&root).expect("a temporary directory for a test build's ~/.vagcan");
		root
	})
}

/// How old a test build's root is before the next test process removes it.
#[cfg(any(test, feature = "test-root"))]
const TEST_ROOT_AGE: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Remove the roots under `parent` older than `age`, best effort: a root another process
/// is removing, or one that will not go, is left for the next run.
#[cfg(any(test, feature = "test-root"))]
fn remove_old_roots(parent: &Path, age: std::time::Duration) {
	let Ok(entries) = std::fs::read_dir(parent) else { return };
	for entry in entries.flatten() {
		let old = entry
			.metadata()
			.and_then(|meta| meta.modified())
			.ok()
			.and_then(|at| at.elapsed().ok())
			.is_some_and(|elapsed| elapsed > age);
		if old {
			let _ = std::fs::remove_dir_all(entry.path());
		}
	}
}

/// Where the projects live — `~/.vagcan/data/`, one directory per platform.
///
/// **The directory is called `data/`, not `projects/`.** It is what a person
/// opens looking for their car's data. The function keeps the name of the
/// *role* because that is what a caller means by it.
///
/// Named for what the *source* calls the vehicle, not for its VIN: an ODIS
/// project ships its own identifier (`SK37X`) and a VCDS-only project is named
/// by the person, and neither of them has ever seen a VIN. `crate::project`
/// owns what is inside one.
pub fn projects_dir() -> anyhow::Result<PathBuf> {
	Ok(vagcan_dir()?.join("data"))
}

/// The machine's VCDS installation, as `setup` read it — `~/.vagcan/vcds/`.
///
/// **One per machine, not one per project.** A VCDS installation describes every VAG car,
/// so its rows serve whichever car is connected; filed under a platform, a second car of
/// another platform needed a second copy of the same installation. A project is an ODIS
/// project's, and nothing else. `crate::project::vcds` opens it.
pub fn vcds_dir() -> anyhow::Result<PathBuf> {
	Ok(vagcan_dir()?.join("vcds"))
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

/// A path as a person reads it: under the home directory, `~/…`; anywhere else as it is.
pub fn tilde(path: &Path) -> String {
	match std::env::var_os("HOME").map(PathBuf::from) {
		Some(home) if !home.as_os_str().is_empty() => match path.strip_prefix(&home) {
			Ok(rest) if rest.as_os_str().is_empty() => "~".to_string(),
			Ok(rest) => format!("~/{}", rest.display()),
			Err(_) => path.display().to_string(),
		},
		_ => path.display().to_string(),
	}
}

/// A path the user gave, or this tool's own default for it.
///
/// What somebody typed is used exactly as typed. A default is `~/.vagcan`'s and
/// nothing else, because a default that followed the working directory is what
/// made these commands work in a checkout and nowhere else.
pub fn or_default(given: Option<&str>, default: impl FnOnce() -> anyhow::Result<PathBuf>) -> anyhow::Result<PathBuf> {
	match given {
		Some(path) => Ok(PathBuf::from(path)),
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
pub fn car_dir(vin: &str) -> anyhow::Result<PathBuf> {
	Ok(cars_dir()?.join(car_folder(vin)?))
}

/// Every car this machine has met — `~/.vagcan/cars/`, one directory each.
///
/// For a reader that wants all of them at once, which `setup` does: the units
/// of every car recorded here are what its VCDS registry step reads channels
/// for. [`car_dir`] is the way to one car.
pub fn cars_dir() -> anyhow::Result<PathBuf> {
	Ok(vagcan_dir()?.join("cars"))
}

/// Where a car's saved measurement sessions go.
pub fn measures_dir(vin: &str) -> anyhow::Result<PathBuf> {
	Ok(car_dir(vin)?.join("measures"))
}

/// What a car's control units said about themselves, one file per car.
///
/// Named here for the reason [`CAR_FILE`] is: the commands that write it
/// (`watch`, `measure`, `units`) and the ones that read it (`setup`, the dash
/// build) have to agree on the name or the record is written where nothing
/// reads it. `crate::units` owns what is inside.
pub const UNITS_FILE: &str = "units.json";

/// The record of a car's control units — `~/.vagcan/cars/<VIN>/units.json`.
///
/// **Per car, not per part number.** Which units a car has, and what each of
/// them is, is a fact about that car; `data/` is the opposite — a project
/// describes a platform. So this sits beside the car file, keyed by VIN, and
/// the VIN is checked as [`car_dir`] checks it: a unit's answer is not trusted
/// as a path.
pub fn units_record(vin: &str) -> anyhow::Result<PathBuf> {
	Ok(car_dir(vin)?.join(UNITS_FILE))
}

/// Replace a file's contents in one step, or leave it as it was.
///
/// The record of a car's units and a project's registry log are both read
/// by one command while another may be writing them — `watch` on the car and
/// `dev dash build` at the desk. A plain `write` truncates first and fills
/// afterwards, and a reader in between sees an empty or half-written file as
/// "no units". So the bytes go to a temporary file beside the target, which is
/// then renamed over it: on Unix a rename replaces atomically, and a reader
/// sees the old file or the new one and never the gap.
///
/// The temporary name carries the process id and a counter, so two writers in
/// one directory — two commands, or two threads of one — never share a
/// temporary file. A write that fails leaves no temporary file behind.
pub fn replace_file(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
	use anyhow::Context as _;
	static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
	let dir = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
	std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
	let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
	let temporary = dir.join(format!(
		".{name}.{}.{}.tmp",
		std::process::id(),
		COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
	));
	let written = std::fs::write(&temporary, bytes)
		.with_context(|| format!("writing {}", temporary.display()))
		.and_then(|()| std::fs::rename(&temporary, path).with_context(|| format!("replacing {}", path.display())));
	if written.is_err() {
		let _ = std::fs::remove_file(&temporary);
	}
	written
}

/// [`replace_file`] for a file a person keeps — `dash.toml`, which `dev dash edit` saves: one
/// that is a link is replaced where it points, the link left as it is; the file keeps its
/// permissions; and its bytes are on disk before the new name takes over.
///
/// A rename over a link replaces the link with a plain file and leaves its target with the old
/// text, so the target is resolved first and the temporary file is staged beside it, in the
/// directory the rename happens in.
pub fn replace_through_links(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
	use anyhow::Context as _;
	use std::io::Write as _;
	static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
	let target = match std::fs::canonicalize(path) {
		Ok(target) => target,
		// A link to a file not made yet is followed to where the file goes.
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => match std::fs::read_link(path) {
			Ok(to) => path.parent().unwrap_or(Path::new(".")).join(to),
			Err(_) => path.to_path_buf(),
		},
		Err(e) => return Err(e).with_context(|| format!("resolving {}", path.display())),
	};
	let dir = target.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
	std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
	let kept = std::fs::metadata(&target).ok().map(|m| m.permissions());
	let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("file");
	let temporary = dir.join(format!(
		".{name}.{}.{}.tmp",
		std::process::id(),
		COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
	));
	let written = (|| {
		let mut file = std::fs::File::create(&temporary).with_context(|| format!("creating {}", temporary.display()))?;
		file.write_all(bytes).with_context(|| format!("writing {}", temporary.display()))?;
		if let Some(permissions) = kept {
			file
				.set_permissions(permissions)
				.with_context(|| format!("setting the permissions of {}", temporary.display()))?;
		}
		file.sync_all().with_context(|| format!("writing {} to disk", temporary.display()))?;
		std::fs::rename(&temporary, &target).with_context(|| format!("replacing {}", target.display()))
	})();
	if written.is_err() {
		let _ = std::fs::remove_file(&temporary);
	}
	written
}

/// Copy a file into place in one step, or leave the destination as it was.
///
/// `setup`'s step 1 copies the `.rod` files into the shared pool, and skips a
/// copy whose destination is newer than its source. A copy cut short — Ctrl-C
/// is acted on at once now — would leave a truncated file under the final name,
/// newer than its source, which the next run then takes for current until
/// `--refresh` (found in review, 2026-09-28). So the bytes go to a staging
/// name beside the destination and are renamed over it, as [`replace_file`]
/// does: the final name holds a whole file or nothing.
pub fn copy_file(from: &Path, to: &Path) -> anyhow::Result<u64> {
	copy_file_with(|from, to| std::fs::copy(from, to), from, to)
}

/// Remove the staging files a [`copy_file`] cut short left in `dir` — exactly
/// the form it writes, `.<name>.<pid>.copying`, and nothing else. How many went.
///
/// A kill mid-copy leaves one per kill, each under its pid, and nothing reads
/// them (found in review, 2026-09-28); `setup`'s step 1 sweeps them before it
/// copies. A dotfile of any other shape is somebody else's and is left alone.
pub fn remove_stale_copies(dir: &Path) -> usize {
	let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
	entries
		.flatten()
		.filter(|entry| {
			let name = entry.file_name();
			let Some(name) = name.to_str() else { return false };
			is_stale_copy(name) && entry.path().is_file()
		})
		.filter(|entry| std::fs::remove_file(entry.path()).is_ok())
		.count()
}

/// Whether a file name is one [`copy_file`] stages under: `.<name>.<pid>.copying`.
fn is_stale_copy(name: &str) -> bool {
	let Some(rest) = name.strip_prefix('.').and_then(|n| n.strip_suffix(".copying")) else {
		return false;
	};
	match rest.rsplit_once('.') {
		Some((name, pid)) => !name.is_empty() && !pid.is_empty() && pid.bytes().all(|b| b.is_ascii_digit()),
		None => false,
	}
}

/// The rule behind [`copy_file`], with the copying itself passed in — so a copy
/// that stops halfway can be staged in a test without stopping the test.
fn copy_file_with(copy: impl FnOnce(&Path, &Path) -> std::io::Result<u64>, from: &Path, to: &Path) -> anyhow::Result<u64> {
	use anyhow::Context as _;
	let dir = to.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
	std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
	let name = to.file_name().and_then(|n| n.to_str()).unwrap_or("file");
	let staging = dir.join(format!(".{name}.{}.copying", std::process::id()));
	let copied = copy(from, &staging)
		.with_context(|| format!("copying {} to {}", from.display(), to.display()))
		.and_then(|n| {
			std::fs::rename(&staging, to)
				.with_context(|| format!("replacing {}", to.display()))
				.map(|()| n)
		});
	if copied.is_err() {
		let _ = std::fs::remove_file(&staging);
	}
	copied
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

/// What `vagcan dev dash edit` serves for a car — `~/.vagcan/dash/<VIN>/editor/`: the page, the
/// renderer as WebAssembly, the car's channels (`channels.json`, kept between runs while what it
/// was made from is unchanged). The layout of its `dash.toml` is answered from the file itself,
/// at the moment the page asks, and is not kept here.
///
/// Beside the plan and not in the checkout, for the plan's reason: `channels.json` is the
/// project's label data resolved for one car. Everything in it is rebuilt by the next run.
pub fn dash_editor_dir(vin: &str) -> anyhow::Result<PathBuf> {
	Ok(dash_dir(vin)?.join("editor"))
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

/// The rule behind [`vagcan_dir`], with the home directory passed in so it can
/// be tested without a process-wide `set_var` that the other tests would race.
/// Unused in another crate's test build, whose default is [`test_root`].
#[cfg_attr(all(feature = "test-root", not(test)), allow(dead_code))]
fn vagcan_dir_in(home: Option<PathBuf>) -> anyhow::Result<PathBuf> {
	let home = home.filter(|p| p.is_absolute()).ok_or_else(|| {
		use crate::report::Advice as _;
		anyhow::anyhow!("no home directory to write to").help("set HOME, or pass an explicit path")
	})?;
	Ok(home.join(".vagcan"))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A unique-per-test temp dir, cleaned up on drop — the shape the rest of
	/// this crate's file tests use. Nothing here may write into the owner's
	/// own `~/.vagcan`.
	struct TempDir(PathBuf);

	impl TempDir {
		fn new(tag: &str) -> TempDir {
			let path = std::env::temp_dir().join(format!("vagcan-datadir-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
			let _ = std::fs::remove_dir_all(&path);
			std::fs::create_dir_all(&path).unwrap();
			TempDir(path)
		}
	}

	impl Drop for TempDir {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir_all(&self.0);
		}
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
		for path in [
			projects_dir().unwrap(),
			rod_pool_dir().unwrap(),
			vcds_dir().unwrap(),
			crate::config::path().unwrap(),
		] {
			assert!(!path.starts_with(&repo), "{path:?} is inside the checkout");
			assert!(path.starts_with(&home), "{path:?} is outside ~/.vagcan");
		}
	}

	#[test]
	fn a_path_the_user_gave_beats_the_default_and_is_not_second_guessed() {
		let typed = or_default(Some("Cargo.toml"), || unreachable!("the default was consulted")).unwrap();
		assert_eq!(typed, Path::new("Cargo.toml"));
		// A relative path is the working directory's, found or not: nothing walks up looking.
		let typed = or_default(Some("crates/nowhere.json"), || unreachable!("the default was consulted")).unwrap();
		assert_eq!(typed, Path::new("crates/nowhere.json"));
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
		assert_eq!(units_record(vin).unwrap(), car.join(UNITS_FILE));
	}

	#[test]
	fn a_cars_units_are_recorded_beside_its_car_file_and_never_in_the_checkout() {
		// Which units a car has is a fact about that car, so the record is keyed
		// the way everything else about a car is — by VIN, under `cars/`.
		let vin = "XW8AD4NE9JH008917";
		assert_eq!(units_record(vin).unwrap(), car_dir(vin).unwrap().join(UNITS_FILE));
		assert_ne!(units_record(vin).unwrap(), units_record("XW8AD4NE9JH008918").unwrap());
		assert!(car_dir(vin).unwrap().starts_with(cars_dir().unwrap()));
		let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
		assert!(!units_record(vin).unwrap().starts_with(&repo));
		// A VIN off the bus never chooses where this tool writes.
		for not_a_vin in ["../../etc", "XW8AD4NE9JH00 8917", ""] {
			assert!(units_record(not_a_vin).is_err(), "{not_a_vin:?} was accepted");
		}
	}

	#[test]
	fn replacing_a_file_leaves_no_temporary_behind_and_the_old_contents_until_the_new_are_whole() {
		let dir = TempDir::new("replace");
		let path = dir.0.join("units.json");
		replace_file(&path, b"first").unwrap();
		assert_eq!(std::fs::read(&path).unwrap(), b"first");
		replace_file(&path, b"second").unwrap();
		assert_eq!(std::fs::read(&path).unwrap(), b"second");
		let left: Vec<_> = std::fs::read_dir(&dir.0).unwrap().flatten().map(|e| e.file_name()).collect();
		assert_eq!(left, vec![std::ffi::OsString::from("units.json")], "a temporary file was left: {left:?}");
		// The directory is made on the way, as a first record of a car needs.
		let deep = dir.0.join("new-car").join("units.json");
		replace_file(&deep, b"{}").unwrap();
		assert_eq!(std::fs::read(&deep).unwrap(), b"{}");
	}

	#[test]
	fn a_reader_never_sees_a_torn_file_while_writers_replace_it() {
		// The property the temporary-and-rename exists for. Several writers
		// replace one file with whole contents of different lengths while a
		// reader reads it as fast as it can: every read is one of the whole
		// contents, never empty and never a prefix. A plain `write` fails this
		// within a few hundred reads.
		let dir = TempDir::new("torn");
		let path = dir.0.join("units.json");
		let contents: Vec<Vec<u8>> = (1..=4).map(|n| vec![b'a' + n as u8; 4096 * n]).collect();
		replace_file(&path, &contents[0]).unwrap();
		let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
		let writers: Vec<_> = contents
			.iter()
			.map(|bytes| {
				let (path, bytes, stop) = (path.clone(), bytes.clone(), stop.clone());
				std::thread::spawn(move || {
					while !stop.load(std::sync::atomic::Ordering::Relaxed) {
						replace_file(&path, &bytes).unwrap();
					}
				})
			})
			.collect();
		let mut reads = 0;
		for _ in 0..2000 {
			let seen = std::fs::read(&path).unwrap();
			assert!(contents.contains(&seen), "a torn read of {} bytes", seen.len());
			reads += 1;
		}
		stop.store(true, std::sync::atomic::Ordering::Relaxed);
		for writer in writers {
			writer.join().unwrap();
		}
		assert_eq!(reads, 2000);
		let left = std::fs::read_dir(&dir.0).unwrap().flatten().count();
		assert_eq!(left, 1, "temporary files were left behind");
	}

	#[test]
	fn a_test_process_clears_the_roots_old_processes_left_and_no_live_one() {
		// Each test process left a `vagcan-test-root-<pid>` behind (review of #37). They
		// share one parent now, and a root older than any test process lives is removed.
		let parent = TempDir::new("roots");
		let (old, live) = (parent.0.join("111"), parent.0.join("222"));
		for dir in [&old, &live] {
			std::fs::create_dir_all(dir.join(".vagcan").join("data")).unwrap();
		}
		let long_ago = std::time::SystemTime::now() - 2 * TEST_ROOT_AGE;
		std::fs::File::open(&old).unwrap().set_modified(long_ago).unwrap();
		remove_old_roots(&parent.0, TEST_ROOT_AGE);
		assert!(!old.exists(), "a root an ended process left stayed");
		assert!(live.join(".vagcan").is_dir(), "a live process's root was taken");
		// And this process's own sits under the one parent.
		assert!(test_root().starts_with(std::env::temp_dir().join("vagcan-test-roots")));
	}

	#[test]
	fn a_test_build_has_a_note_naming_its_data_dir() {
		let note = test_build_note().expect("this is a test build");
		assert_eq!(
			note,
			format!("note  test build: data dir {}, not ~/.vagcan", vagcan_dir().unwrap().display())
		);
	}

	#[test]
	fn a_test_build_never_resolves_to_the_vagcan_dir_home_points_at() {
		// Opening a cache can upgrade it: a test that resolved HOME's `~/.vagcan` wrote into
		// the person's data (2026-10-20). Unless `VAGCAN_HOME` names one, a test build's is
		// a temporary directory of its own.
		if std::env::var_os(ROOT_ENV).is_some() {
			return;
		}
		let dir = vagcan_dir().unwrap();
		assert_eq!(dir, test_root());
		assert!(dir.is_dir());
		assert!(dir.starts_with(std::env::temp_dir()), "{dir:?}");
		if let Some(home) = dirs::home_dir() {
			assert!(!dir.starts_with(home.join(".vagcan")), "{dir:?} is the person's own");
		}
		assert!(projects_dir().unwrap().starts_with(&dir));
	}

	#[test]
	fn a_relative_home_is_ignored_because_it_would_follow_the_shell() {
		// Resolving against the working directory is the same "wherever the
		// shell is standing" failure this module exists to avoid.
		assert!(vagcan_dir_in(Some(PathBuf::from("relative/home"))).is_err());
	}

	#[test]
	fn with_nowhere_to_write_the_error_says_what_to_set() {
		let err = crate::report::render(&vagcan_dir_in(None).unwrap_err(), 0, crate::report::Colour::Never);
		assert!(err.contains("HOME"), "{err}");
	}

	#[test]
	fn a_copy_cut_short_leaves_nothing_under_the_final_name() {
		// Found in review (2026-09-28): `setup`'s step 1 copied the `.rod` files
		// straight to their final names, so a Ctrl-C mid-copy left a truncated
		// file newer than its source, which the next run took for current until
		// `--refresh`. A copy that stops halfway, as an interrupted one does,
		// must leave the final name empty — and no staging file either.
		let dir = tempfile::tempdir().unwrap();
		let src = dir.path().join("EV_X.rod");
		let bytes: Vec<u8> = (0..4000u32).map(|n| (n % 251) as u8).collect();
		std::fs::write(&src, &bytes).unwrap();
		let pool = dir.path().join("pool");
		std::fs::create_dir_all(&pool).unwrap();
		let dst = pool.join("EV_X.rod");
		let cut_short = |from: &Path, to: &Path| -> std::io::Result<u64> {
			let bytes = std::fs::read(from)?;
			std::fs::write(to, &bytes[..bytes.len() / 2])?;
			Err(std::io::Error::other("cut short"))
		};
		let err = copy_file_with(cut_short, &src, &dst).unwrap_err().to_string();
		assert!(err.contains("EV_X.rod"), "{err}");
		assert!(!dst.exists(), "a truncated file under the final name reads as current next time");
		assert_eq!(std::fs::read_dir(&pool).unwrap().count(), 0, "no staging file left behind");
		// The whole copy lands under the final name, whole, and nothing else is left.
		assert_eq!(copy_file(&src, &dst).unwrap(), bytes.len() as u64);
		assert_eq!(std::fs::read(&dst).unwrap(), bytes);
		assert_eq!(std::fs::read_dir(&pool).unwrap().count(), 1);
		// And a copy over an older file replaces it whole, as `setup` does on `--refresh`.
		std::fs::write(&src, b"newer").unwrap();
		copy_file(&src, &dst).unwrap();
		assert_eq!(std::fs::read(&dst).unwrap(), b"newer");
	}

	#[test]
	fn the_staging_files_a_kill_left_are_removed_and_nothing_else_is() {
		// One per kill, each under its pid (found in review, 2026-09-28). Only
		// the exact form `copy_file` writes goes; every other name in the pool —
		// a copied file, a dotfile of another shape, a staging name with no pid
		// — stays.
		let dir = tempfile::tempdir().unwrap();
		let pool = dir.path().join("rod");
		std::fs::create_dir_all(&pool).unwrap();
		for name in [
			".EV_A.rod.35611.copying",
			".EV_B.rod.7.copying",
			"EV_A.rod",
			".DS_Store",
			".EV_C.rod.copying",
			".EV_D.rod.12x.copying",
			"..1.copying",
			"EV_E.rod.35611.copying",
		] {
			std::fs::write(pool.join(name), b"x").unwrap();
		}
		std::fs::create_dir_all(pool.join(".dir.1.copying")).unwrap();
		assert_eq!(remove_stale_copies(&pool), 2);
		let mut left: Vec<String> = std::fs::read_dir(&pool)
			.unwrap()
			.flatten()
			.map(|e| e.file_name().to_string_lossy().into_owned())
			.collect();
		left.sort();
		assert_eq!(
			left,
			vec![
				"..1.copying",
				".DS_Store",
				".EV_C.rod.copying",
				".EV_D.rod.12x.copying",
				".dir.1.copying",
				"EV_A.rod",
				"EV_E.rod.35611.copying"
			]
		);
		assert_eq!(remove_stale_copies(&dir.path().join("nowhere")), 0, "no pool is nothing to sweep");
		// The name a real copy stages under is one this recognises.
		assert!(is_stale_copy(&format!(".EV_X.rod.{}.copying", std::process::id())));
	}

	#[cfg(unix)]
	#[test]
	fn a_file_kept_by_hand_is_replaced_through_its_link_with_its_permissions() {
		use std::os::unix::fs::PermissionsExt as _;
		let here = tempfile::tempdir().unwrap();
		let target = here.path().join("elsewhere").join("dash.toml");
		std::fs::create_dir(target.parent().unwrap()).unwrap();
		std::fs::write(&target, "old").unwrap();
		std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
		let link = here.path().join("dash.toml");
		std::os::unix::fs::symlink(&target, &link).unwrap();
		replace_through_links(&link, b"new").unwrap();
		assert!(
			std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
			"the link stays a link"
		);
		assert_eq!(std::fs::read_to_string(&target).unwrap(), "new", "its target holds the new text");
		assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
		let left: Vec<_> = std::fs::read_dir(target.parent().unwrap())
			.unwrap()
			.flatten()
			.map(|e| e.file_name())
			.collect();
		assert_eq!(left, ["dash.toml"], "no temporary file left behind");
		// A plain file, and one not there yet.
		let plain = here.path().join("plain.toml");
		std::fs::write(&plain, "old").unwrap();
		std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o640)).unwrap();
		replace_through_links(&plain, b"new").unwrap();
		assert_eq!(std::fs::metadata(&plain).unwrap().permissions().mode() & 0o777, 0o640);
		replace_through_links(&here.path().join("fresh.toml"), b"made").unwrap();
		assert_eq!(std::fs::read_to_string(here.path().join("fresh.toml")).unwrap(), "made");
	}
}
