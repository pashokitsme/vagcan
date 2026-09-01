//! Moving `~/.vagcan/data/` into the project store, once.
//!
//! Before projects existed there was one `data/extracted/` and one
//! `data/measured/`, because there was one source and it described one car.
//! Spec §6 turns that into the first project: the `.rod` files and the fault
//! text join the shared pool at `~/.vagcan/rod/`, everything else lands under
//! `~/.vagcan/data/<id>/`.
//!
//! **`measured/` is why this module is careful.** Those rows were proven by
//! driving a car — the label files provably cannot supply them
//! (`research/labels/rod-labels.md` §4.0c) — and nothing but another drive can
//! recreate one. So every file is **copied, verified, and only then removed**,
//! in that order and per file: a run interrupted half way leaves both copies,
//! which is untidy, and never leaves neither, which would be unrecoverable.
//! A second run is a no-op because the first left nothing behind to move.
//!
//! **`Labels/` is copied nowhere and deleted by nobody.** D4 drops the
//! `.lbl`/`.clb` files from what this tool keeps — `cache.sqlite` is their
//! surviving representation — but "no longer read" is not "safe to delete on
//! somebody's behalf", and a hundred megabytes of a person's files is not this
//! command's to throw away. It stays where it is, and the report says it can go.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Below this, a copy is verified by reading both files back and comparing every
/// byte; above it, by length alone.
///
/// The split is honest about what it buys. `measured/` files are a few kilobytes
/// each and are the ones that cannot be recreated, so they get the real check. A
/// forty-megabyte `.rod` gets the cheap one, because reading 122 MB twice to
/// re-derive what `fs::copy` already reported would cost a minute to re-prove
/// something a failed copy would have raised as an error.
const FULL_COMPARE: u64 = 4 << 20;

/// The one file under `measured/` that stays where it is.
///
/// `vag_uds_client::address::OVERRIDE_PATH` names it, from a crate with no idea
/// what a project is. See the note in [`plan`].
const OVERRIDE_FILE: &str = "unit-numbers.json";

/// The old layout, still on disk.
#[derive(Debug, Clone)]
pub struct Old {
	pub extracted: PathBuf,
	pub measured: PathBuf,
}

impl Old {
	/// How many proven-on-car files are waiting to move.
	///
	/// The one number worth putting in front of somebody before the move,
	/// because it is the only class here that no re-parse can reproduce — spec
	/// §4.5's "the only data proven on the actual car". Everything else in
	/// `data/` is extracted from a VCDS installation and can be extracted again.
	///
	/// Zero is the ordinary answer: a machine that has run `setup` but never
	/// calibrated a car has nothing here, and nothing irreversible is at stake.
	pub fn proven(&self) -> usize {
		plan(self).iter().filter(|(_, kind, _)| *kind == Kind::Measured).count()
	}

	/// How many files in total would move.
	pub fn files(&self) -> usize {
		plan(self).len()
	}
}

/// What one migration did, for the run's closing report.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
	/// Raw VCDS files that went into the shared pool.
	pub pooled: usize,
	/// Proven-on-car files that went into the project.
	pub measured: usize,
	/// Derived files — the cache, the names, the keys.
	pub derived: usize,
	/// Files left where they were because the destination already held
	/// something of that name with different content. Nothing is overwritten
	/// and nothing is removed.
	pub conflicted: Vec<PathBuf>,
	/// `Labels/`, if it is still there: read by nothing now, deleted by nobody.
	pub left_behind: Option<PathBuf>,
}

impl Report {
	/// Whether anything at all moved.
	pub fn moved(&self) -> usize {
		self.pooled + self.measured + self.derived
	}
}

/// Move projects out of `~/.vagcan/projects/` and into `~/.vagcan/data/`.
///
/// **A second, smaller move, and one this project inflicted on itself.** The
/// store lived at `projects/<id>/` for exactly as long as it took the owner to
/// say the directory should be `data/`. A build that ran before the rename left
/// a real project — an 88 MB cache, a names file, possibly proven rows — under a
/// parent nothing looks in any more, and the next command reports that no car
/// has been set up.
///
/// Unlike [`run`] this asks nothing and reports nothing when there is nothing to
/// do. There is no question to ask: the project keeps its own name, only its
/// parent changes, so there is no car it could be filed under by mistake.
///
/// `rename` rather than copy-verify-remove, deliberately, and it is the safer of
/// the two here: within one filesystem it is atomic, so the directory is either
/// at the old path or the new one and never half at both. A destination that
/// already exists is left alone rather than merged — two directories of one name
/// is a state a person has to look at, not one this should resolve by guessing.
pub fn relocate_projects() -> Result<()> {
	let vagcan = crate::datadir::vagcan_dir()?;
	relocate_in(&vagcan.join("projects"), &crate::datadir::projects_dir()?)
}

/// The rule behind [`relocate_projects`], with both parents passed in.
fn relocate_in(from: &Path, to: &Path) -> Result<()> {
	let Ok(entries) = std::fs::read_dir(from) else {
		// No such directory: every machine that never ran a build from the few
		// hours the old name existed.
		return Ok(());
	};
	let mut moved = 0usize;
	for entry in entries.flatten() {
		let path = entry.path();
		if !path.is_dir() {
			continue;
		}
		let Some(name) = entry.file_name().into_string().ok().filter(|n| crate::project::folder_name(n).is_ok()) else {
			continue;
		};
		let target = to.join(&name);
		if target.exists() {
			// Both exist. Saying so and doing nothing is the only honest move:
			// merging them could put two builds' rows in one cache, and picking
			// one would discard the other's proven rows.
			eprintln!(
				"both {} and {} exist — leaving them alone; the one to keep is yours to choose",
				path.display(),
				target.display()
			);
			continue;
		}
		std::fs::create_dir_all(to).with_context(|| format!("creating {}", to.display()))?;
		std::fs::rename(&path, &target).with_context(|| format!("moving {} to {}", path.display(), target.display()))?;
		moved += 1;
	}
	if moved > 0 {
		// Said once, because a person who upgrades and finds their car under a
		// new path deserves to know why — and the path is last, since its
		// length is not knowable when the sentence is written.
		eprintln!("moved {moved} project(s) into {}", to.display());
	}
	// Only if it is empty, and `remove_dir` failing is the check.
	let _ = std::fs::remove_dir(from);
	Ok(())
}

/// The old layout, if there is still anything in it worth moving.
///
/// **Keyed on `extracted`/`measured`, never on `data/` existing** (spec §6).
/// `data/` is where the projects live now, so its existence says nothing; the
/// two pre-project directories are named, and only they are looked in.
///
/// Stricter than "does either exist", deliberately: it answers *is there
/// anything left to move*. A finished migration leaves `extracted/Labels/`
/// behind on purpose, and a rule keyed on existence would offer to migrate
/// again on every run for ever.
pub fn pending() -> Result<Option<Old>> {
	Ok(pending_in(&crate::datadir::vagcan_dir()?))
}

/// The rule behind [`pending`], with `~/.vagcan` passed in so it can be tested
/// without touching the owner's own.
fn pending_in(vagcan: &Path) -> Option<Old> {
	let old = Old {
		extracted: vagcan.join("data").join("extracted"),
		measured: vagcan.join("data").join("measured"),
	};
	// Asked as "is there anything left to move", not "does `data/` exist": a
	// finished migration leaves `Labels/` behind, and a `data/` that holds only
	// that must not offer to run again.
	(!plan(&old).is_empty()).then_some(old)
}

/// Where one file is going, and which counter it lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
	/// The shared `~/.vagcan/rod/` pool: a property of a VCDS build.
	Pooled,
	/// `data/<id>/measurements/`: proven on a car, irreplaceable.
	Measured,
	/// `data/<id>/`: the cache, the names, the keys.
	Derived,
}

/// Every file the migration would move, and what it is.
///
/// Built before anything is written, the way `setup`'s copy plan is, so the
/// question "is there anything left to do" has one answer used by both
/// [`pending_in`] and [`run_into`] rather than two that can disagree.
fn plan(old: &Old) -> Vec<(PathBuf, Kind, PathBuf)> {
	let mut plan = Vec::new();

	// The derived three, by name, straight into the project.
	for name in ["cache.sqlite", "names.json", "rod-keys.json"] {
		let src = old.extracted.join(name);
		if src.is_file() {
			plan.push((src, Kind::Derived, PathBuf::from(name)));
		}
	}

	// Every `.rod` and this build's fault text, wherever under `extracted/`
	// they sit, flattened into the shared pool. Flattened because that is what
	// the pool is: `dtc::find_named` and `find_rod_by_odx_name` search by name
	// and never by path, so the directory a file sat in was never load-bearing.
	let mut raw = Vec::new();
	walk(&old.extracted, &mut raw);
	for src in raw {
		let Some(name) = src.file_name().and_then(|n| n.to_str()) else {
			continue;
		};
		let rod = name.to_ascii_lowercase().ends_with(".rod");
		let codes = crate::setup::CODES_FILES.iter().any(|known| name.eq_ignore_ascii_case(known));
		if rod || codes {
			plan.push((src.clone(), Kind::Pooled, PathBuf::from(name)));
		}
	}

	// The proven rows, keeping their layout under `measurements/`.
	let mut proven = Vec::new();
	walk(&old.measured, &mut proven);
	for src in proven {
		let Ok(rel) = src.strip_prefix(&old.measured) else { continue };
		// **Except the one file this tool does not own.** `unit-numbers.json` is
		// not a proven measurement row at all — it is the hand-written
		// number-to-CAN-id override, and `vag_uds_client::address::OVERRIDE_PATH`
		// reads it by a fixed path from a crate that cannot know which project
		// this is. Moving it would silently ignore what somebody wrote down,
		// which is the worst of the outcomes available: their car would go on
		// answering, with the wrong addresses, and nothing would say why.
		if rel.as_os_str().to_string_lossy().ends_with(OVERRIDE_FILE) {
			continue;
		}
		plan.push((src.clone(), Kind::Measured, rel.to_path_buf()));
	}

	plan
}

/// Every file under `dir`, recursively. A directory that is not there is not an
/// error — it is a machine that never had one.
fn walk(dir: &Path, into: &mut Vec<PathBuf>) {
	let Ok(entries) = std::fs::read_dir(dir) else { return };
	for entry in entries.flatten() {
		let path = entry.path();
		match path.is_dir() {
			true => walk(&path, into),
			false => into.push(path),
		}
	}
}

/// Move the old layout into `project`, copying before removing anything.
pub fn run(old: &Old, project: &crate::project::Project) -> Result<Report> {
	run_into(old, &project.dir, &crate::project::rod_pool()?)
}

/// The rule behind [`run`], with both destinations passed in so the whole move
/// can be exercised in a temporary directory.
fn run_into(old: &Old, project_dir: &Path, rod_pool: &Path) -> Result<Report> {
	let mut report = Report::default();
	for (src, kind, rel) in plan(old) {
		let dst = match kind {
			Kind::Pooled => rod_pool.join(&rel),
			Kind::Measured => project_dir.join("measurements").join(&rel),
			Kind::Derived => project_dir.join(&rel),
		};
		match carry(&src, &dst)? {
			true => match kind {
				Kind::Pooled => report.pooled += 1,
				Kind::Measured => report.measured += 1,
				Kind::Derived => report.derived += 1,
			},
			false => report.conflicted.push(src),
		}
	}

	// Whatever is now empty, tidied away. Only ever empty directories: a
	// `remove_dir` that finds anything in it fails, which is the check.
	prune(&old.measured);
	prune(&old.extracted);

	let labels = old.extracted.join("Labels");
	if labels.is_dir() {
		report.left_behind = Some(labels);
	} else {
		prune(old.extracted.parent().unwrap_or(&old.extracted));
	}
	Ok(report)
}

/// Copy one file, verify it arrived, then remove the original.
///
/// Returns `false` when the destination already holds something else of that
/// name — the shared pool is shared, and two VCDS builds can ship a same-named
/// `.rod` with different bytes. Nothing is overwritten and nothing is removed in
/// that case; the caller reports it and the file stays where it is.
fn carry(src: &Path, dst: &Path) -> Result<bool> {
	if dst.exists() {
		// Already there from an earlier partial run, or from the other build.
		// The same file is not a conflict, it is a job already done.
		return match same(src, dst)? {
			true => {
				std::fs::remove_file(src).with_context(|| format!("removing {}", src.display()))?;
				Ok(true)
			}
			false => Ok(false),
		};
	}
	if let Some(parent) = dst.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
	}
	std::fs::copy(src, dst).with_context(|| format!("copying {} to {}", src.display(), dst.display()))?;
	// Verified before the original goes, never after, and never on the strength
	// of `fs::copy` returning `Ok` alone: these are files a drive proved, and
	// the cost of checking is nothing beside the cost of being wrong.
	anyhow::ensure!(
		same(src, dst)?,
		"{} did not arrive intact at {} — the original is untouched",
		src.display(),
		dst.display()
	);
	std::fs::remove_file(src).with_context(|| format!("removing {}", src.display()))?;
	Ok(true)
}

/// Whether two files hold the same thing.
///
/// Every byte for anything small enough to read twice cheaply — which is every
/// file that cannot be recreated — and the length alone for the rest. See
/// [`FULL_COMPARE`].
fn same(a: &Path, b: &Path) -> Result<bool> {
	let (ma, mb) = (
		std::fs::metadata(a).with_context(|| format!("reading {}", a.display()))?,
		std::fs::metadata(b).with_context(|| format!("reading {}", b.display()))?,
	);
	if ma.len() != mb.len() {
		return Ok(false);
	}
	if ma.len() > FULL_COMPARE {
		return Ok(true);
	}
	Ok(std::fs::read(a)? == std::fs::read(b)?)
}

/// Remove a directory if it is empty, and its now-empty parents up to `data/`.
///
/// `remove_dir` rather than `remove_dir_all` throughout: it fails on a directory
/// that still holds something, which is exactly the guard wanted here.
fn prune(dir: &Path) {
	let mut at = dir.to_path_buf();
	for _ in 0..3 {
		if std::fs::remove_dir(&at).is_err() {
			return;
		}
		let Some(parent) = at.parent() else { return };
		at = parent.to_path_buf();
	}
}

/// What to tell somebody whose data has just moved.
pub fn describe(report: &Report, project: &crate::project::Project) -> String {
	use std::fmt::Write as _;
	let mut out = format!(
		"Moved the data from before projects existed into `{}`:\n  \
         {} raw VCDS files into the shared pool\n  \
         {} proven-on-car files into {}\n  \
         {} derived files into {}\n",
		project.id,
		report.pooled,
		report.measured,
		project.measurements_dir().display(),
		report.derived,
		project.dir.display()
	);
	for path in &report.conflicted {
		let _ = writeln!(
			out,
			"  left where it was, something else of that name is already in the pool: {}",
			path.display()
		);
	}
	if let Some(labels) = &report.left_behind {
		let _ = writeln!(
			out,
			"  {} is left where it is. Nothing reads it now — the label cache is what\n  \
             survives of it — so it can be deleted, but that is yours to do, not this\n  \
             command's.",
			labels.display()
		);
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A `~/.vagcan` as an earlier build left it. **Every byte here is
	/// synthetic** — no test reads a real installation, a real ODIS project, or
	/// the owner's own `~/.vagcan`.
	fn old_layout(vagcan: &Path) -> Old {
		let extracted = vagcan.join("data").join("extracted");
		let measured = vagcan.join("data").join("measured");
		for (rel, bytes) in [
			("cache.sqlite", b"a label cache".as_slice()),
			("names.json", b"{}"),
			("rod-keys.json", b"{}"),
			("Codes.dat", b"fault text"),
			("UDS_EV/RD.rod", b"the registry"),
			("UDS_EV/EV_ECM.rod", b"one unit's own file"),
			("Labels/part.lbl", b"001,1,Engine Speed,,"),
		] {
			let path = extracted.join(rel);
			std::fs::create_dir_all(path.parent().unwrap()).unwrap();
			std::fs::write(path, bytes).unwrap();
		}
		for (rel, bytes) in [("04E-906-027-AH.json", b"{\"rows\":[]}".as_slice()), ("unit-numbers.json", b"{}")] {
			std::fs::create_dir_all(&measured).unwrap();
			std::fs::write(measured.join(rel), bytes).unwrap();
		}
		Old { extracted, measured }
	}

	fn project(root: &Path) -> crate::project::Project {
		let dir = root.join("projects").join("SK37X");
		std::fs::create_dir_all(&dir).unwrap();
		crate::project::Project { id: "SK37X".into(), dir }
	}

	#[test]
	fn the_hand_written_address_override_is_left_exactly_where_it_is_read_from() {
		// `vag_uds_client` reads it by a fixed path, from a crate that cannot know
		// which project this is. Moving it would leave somebody's own pairings
		// silently ignored — their car answering at the wrong addresses with
		// nothing to say why.
		let here = tempfile::tempdir().unwrap();
		let old = old_layout(here.path());
		let p = project(here.path());
		run_into(&old, &p.dir, &here.path().join("rod")).unwrap();
		assert!(
			old.measured.join(OVERRIDE_FILE).is_file(),
			"the override moved out from under vag-uds-client"
		);
		assert!(!p.measurements_dir().join(OVERRIDE_FILE).exists());
		// And the path it is read from is still the one this module leaves alone.
		let owned = std::path::Path::new(vag_uds_client::address::OVERRIDE_PATH);
		assert!(owned.ends_with(OVERRIDE_FILE), "{owned:?}");
	}

	#[test]
	fn every_file_arrives_where_the_new_layout_expects_it() {
		let here = tempfile::tempdir().unwrap();
		let old = old_layout(here.path());
		let p = project(here.path());
		let pool = here.path().join("rod");

		let report = run_into(&old, &p.dir, &pool).unwrap();
		assert_eq!((report.pooled, report.measured, report.derived), (3, 1, 3), "{report:?}");

		// The raw files, flat: nothing searches them by path.
		for name in ["RD.rod", "EV_ECM.rod", "Codes.dat"] {
			assert!(pool.join(name).is_file(), "{name} is not in the pool");
		}
		for name in ["cache.sqlite", "names.json", "rod-keys.json"] {
			assert!(p.dir.join(name).is_file(), "{name} is not in the project");
		}
		// The proven rows keep their layout — `vag_uds_client::address` reads one
		// of them by a relative path it owns.
		assert!(p.measurements_dir().join("04E-906-027-AH.json").is_file());
		assert_eq!(std::fs::read(p.dir.join("cache.sqlite")).unwrap(), b"a label cache");
	}

	#[test]
	fn nothing_is_deleted_that_was_not_copied_first() {
		// The rule this module exists for. `measured/` holds rows proven by
		// driving a car and nothing but another drive can recreate one.
		let here = tempfile::tempdir().unwrap();
		let old = old_layout(here.path());
		let p = project(here.path());
		let pool = here.path().join("rod");

		let before = std::fs::read(old.measured.join("04E-906-027-AH.json")).unwrap();
		run_into(&old, &p.dir, &pool).unwrap();
		let after = std::fs::read(p.measurements_dir().join("04E-906-027-AH.json")).unwrap();
		assert_eq!(before, after, "a proven row changed on the way across");
	}

	#[test]
	fn a_second_run_is_a_no_op_rather_than_a_second_move() {
		let here = tempfile::tempdir().unwrap();
		let old = old_layout(here.path());
		let p = project(here.path());
		let pool = here.path().join("rod");

		let first = run_into(&old, &p.dir, &pool).unwrap();
		assert!(first.moved() > 0);
		let second = run_into(&old, &p.dir, &pool).unwrap();
		assert_eq!(second.moved(), 0, "{second:?}");
		// And nothing offers to run it a third time.
		assert!(pending_in(here.path()).is_none());
	}

	#[test]
	fn labels_is_left_where_it_is_rather_than_deleted_on_somebody_s_behalf() {
		// D4 drops the .lbl/.clb from what this tool keeps, and "no longer read"
		// is not "safe to throw away" — a hundred megabytes of a person's files
		// is not this command's to delete.
		let here = tempfile::tempdir().unwrap();
		let old = old_layout(here.path());
		let p = project(here.path());
		let report = run_into(&old, &p.dir, &here.path().join("rod")).unwrap();
		assert_eq!(report.left_behind.as_deref(), Some(old.extracted.join("Labels").as_path()));
		assert!(old.extracted.join("Labels/part.lbl").is_file(), "it deleted the label files");
		let said = describe(&report, &p);
		assert!(said.contains("Labels"), "{said}");
		assert!(said.contains("yours to do"), "{said}");
	}

	#[test]
	fn a_pool_that_already_holds_another_builds_file_of_that_name_keeps_both() {
		// The pool is shared, and two VCDS builds ship a same-named `.rod` with
		// different bytes. Overwriting would put one build's file under the
		// other's key recovery; removing the source would lose it outright.
		let here = tempfile::tempdir().unwrap();
		let old = old_layout(here.path());
		let p = project(here.path());
		let pool = here.path().join("rod");
		std::fs::create_dir_all(&pool).unwrap();
		std::fs::write(pool.join("RD.rod"), b"another build's registry").unwrap();

		let report = run_into(&old, &p.dir, &pool).unwrap();
		assert_eq!(report.conflicted, [old.extracted.join("UDS_EV/RD.rod")], "{report:?}");
		assert_eq!(
			std::fs::read(pool.join("RD.rod")).unwrap(),
			b"another build's registry",
			"it overwrote the pool"
		);
		assert!(old.extracted.join("UDS_EV/RD.rod").is_file(), "it removed the source it could not carry");
	}

	#[test]
	fn a_file_already_carried_by_an_interrupted_run_is_finished_rather_than_refused() {
		// A run cut off between the copy and the remove leaves both copies. The
		// next run has to recognise its own work.
		let here = tempfile::tempdir().unwrap();
		let old = old_layout(here.path());
		let p = project(here.path());
		let pool = here.path().join("rod");
		std::fs::create_dir_all(&pool).unwrap();
		std::fs::copy(old.extracted.join("UDS_EV/RD.rod"), pool.join("RD.rod")).unwrap();

		let report = run_into(&old, &p.dir, &pool).unwrap();
		assert!(report.conflicted.is_empty(), "{report:?}");
		assert_eq!(report.pooled, 3);
	}

	#[test]
	fn what_is_at_stake_is_counted_before_anything_moves() {
		// The number `setup` puts in front of somebody before asking whose car
		// this data is. Only the proven rows are irreplaceable; the rest can be
		// extracted from a VCDS installation again.
		let here = tempfile::tempdir().unwrap();
		let old = old_layout(here.path());
		assert_eq!(old.proven(), 1, "the override is not a proven row and does not move");
		assert_eq!(old.files(), 7);

		// A machine that ran setup but never calibrated a car has nothing
		// irreversible at stake, and the question can say so.
		std::fs::remove_file(old.measured.join("04E-906-027-AH.json")).unwrap();
		assert_eq!(old.proven(), 0);
		assert!(old.files() > 0, "there is still plenty to move, just nothing unrepeatable");
	}

	#[test]
	fn a_project_left_under_the_old_parent_is_moved_rather_than_orphaned() {
		// The rename from `projects/` to `data/` stranded a real project — an
		// 88 MB cache and proven rows — under a parent nothing looks in, and
		// the next command reported that no car had been set up.
		let here = tempfile::tempdir().unwrap();
		let (from, to) = (here.path().join("projects"), here.path().join("data"));
		std::fs::create_dir_all(from.join("SK37X").join("measurements")).unwrap();
		std::fs::write(from.join("SK37X").join("cache.sqlite"), b"the cache").unwrap();
		std::fs::write(from.join("SK37X").join("measurements").join("04E.json"), b"proven").unwrap();

		relocate_in(&from, &to).unwrap();
		assert_eq!(std::fs::read(to.join("SK37X").join("cache.sqlite")).unwrap(), b"the cache");
		assert_eq!(std::fs::read(to.join("SK37X").join("measurements").join("04E.json")).unwrap(), b"proven");
		assert!(!from.exists(), "the empty old parent is tidied away");
		// And a second run has nothing to do.
		relocate_in(&from, &to).unwrap();
		assert!(to.join("SK37X").is_dir());
	}

	#[test]
	fn a_project_that_exists_under_both_parents_is_left_for_a_person_to_settle() {
		// Merging could put two builds' rows in one cache; picking one would
		// discard the other's proven rows. Neither is this command's to decide.
		let here = tempfile::tempdir().unwrap();
		let (from, to) = (here.path().join("projects"), here.path().join("data"));
		std::fs::create_dir_all(from.join("SK37X")).unwrap();
		std::fs::create_dir_all(to.join("SK37X")).unwrap();
		std::fs::write(from.join("SK37X").join("cache.sqlite"), b"old").unwrap();
		std::fs::write(to.join("SK37X").join("cache.sqlite"), b"new").unwrap();

		relocate_in(&from, &to).unwrap();
		assert_eq!(std::fs::read(from.join("SK37X").join("cache.sqlite")).unwrap(), b"old");
		assert_eq!(std::fs::read(to.join("SK37X").join("cache.sqlite")).unwrap(), b"new");
	}

	#[test]
	fn a_machine_that_never_had_the_old_parent_is_untouched() {
		let here = tempfile::tempdir().unwrap();
		relocate_in(&here.path().join("projects"), &here.path().join("data")).unwrap();
		assert!(!here.path().join("data").exists(), "it created a directory for nothing");
	}

	#[test]
	fn a_machine_that_never_had_the_old_layout_has_nothing_to_migrate() {
		let here = tempfile::tempdir().unwrap();
		assert!(pending_in(here.path()).is_none());
		// And one that has it does.
		old_layout(here.path());
		assert!(pending_in(here.path()).is_some());
	}
}
