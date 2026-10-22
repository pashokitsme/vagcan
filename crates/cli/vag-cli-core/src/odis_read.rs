//! What `setup` did not read of an ODIS project, read the first time a run needs it.
//!
//! A project read by an older vagcan lacks what a newer one keeps — its unit names
//! (`unit_name`), its adaptation channels (`adaptation_channel`). Nobody runs a command to
//! feed the tool (owner, 2026-09-28: «паразитный, никто этим заморачиваться не будет»), so the
//! run that needs one reads it from the project's folder, which the cache records
//! (`source.dir`), writes it into the cache and goes on; every later run finds it there. Only
//! when that folder is gone is there something to say, and it says what is missing.
//!
//! **One reader at a time.** The read takes the project's lock (`.odis-read.lock`, an
//! exclusive SQLite transaction, as the VCDS store's migration does) and asks again under it
//! whether anything is unread: two processes started together read once, and the second finds
//! the first one's rows.
//!
//! **A read that fails is not tried on every run.** Its reason is written beside the cache
//! (`.odis-read.json`) with the folder's modification time; later runs give that reason without
//! reading again, until the folder changes — then it is read once more, and says it is a
//! retry, not a first time.

use std::path::{Path, PathBuf};

use crate::project::Project;

/// What a project lacks and can be read for later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
	/// The units' names and numbers ([`vag_data_labels::odis::Project::unit_names`]).
	UnitNames,
	/// The variants' adaptation channels ([`vag_data_labels::odis::Project::adaptations`]).
	Adaptations,
}

impl Part {
	/// What it is, in a sentence: `unit names`, `adaptation channels`.
	pub fn what(self) -> &'static str {
		match self {
			Part::UnitNames => "unit names",
			Part::Adaptations => "adaptation channels",
		}
	}

	/// Its key in the record of failed reads.
	fn key(self) -> &'static str {
		match self {
			Part::UnitNames => "unit_names",
			Part::Adaptations => "adaptations",
		}
	}

	/// The ODIS sources of the cache that lack this part, by folder.
	fn unread(self, cache: &Path) -> Result<Vec<String>, vag_data_db::Error> {
		match self {
			Part::UnitNames => vag_data_db::odis_sources_without_unit_names(cache),
			Part::Adaptations => vag_data_db::odis_sources_without_adaptations(cache),
		}
	}
}

/// How [`ensure`] left a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ensured {
	/// Nothing was missing, or it is read now.
	Held,
	/// Never read, and the folder it would be read from is not on this machine.
	Gone(PathBuf),
	/// The read failed: why.
	Failed(String),
}

/// Make sure `store`'s cache holds `part`, reading it from the project's folder when it does
/// not. Says on stderr that it reads, once; at `-v` from where and what it found.
pub fn ensure(store: &Project, part: Part) -> Ensured {
	ensure_with(store, part, |dir| read(store, part, dir))
}

/// [`ensure`] off the async runtime's worker threads, as `registry::ensure_async` is: opening
/// a project and walking it is seconds of CPU.
pub async fn ensure_async(store: Project, part: Part) -> Ensured {
	match tokio::task::spawn_blocking(move || ensure(&store, part)).await {
		Ok(ensured) => ensured,
		Err(e) => Ensured::Failed(e.to_string()),
	}
}

/// [`ensure`] with the read itself passed in: `read(folder)` writes the part into the cache
/// and says in a few words what it found.
fn ensure_with(store: &Project, part: Part, read: impl Fn(&Path) -> anyhow::Result<String>) -> Ensured {
	let cache = store.cache();
	match part.unread(&cache) {
		Ok(unread) if unread.is_empty() => return Ensured::Held,
		Ok(_) => {}
		// A cache that will not answer is said by whoever reads it next, in its own words.
		Err(_) => return Ensured::Held,
	}
	let _lock = match vag_data_db::exclusive(&store.dir.join(".odis-read.lock"), std::time::Duration::from_secs(120)) {
		Ok(lock) => lock,
		Err(e) => return Ensured::Failed(format!("the project is busy: {e}")),
	};
	// Asked again under the lock: another process may have read it while this one waited.
	let unread = match part.unread(&cache) {
		Ok(unread) => unread,
		Err(e) => return Ensured::Failed(e.to_string()),
	};
	for dir in unread {
		let folder = PathBuf::from(&dir);
		if !folder.is_dir() {
			return Ensured::Gone(folder);
		}
		let changed = changed_at(&folder);
		let failed = failure(store, part).filter(|f| f.dir == dir);
		match &failed {
			// Failed before, and nothing about the folder has changed since: the same answer.
			Some(f) if f.changed == changed => return Ensured::Failed(f.why.clone()),
			Some(_) => crate::say!(
				"note: reading the {} of the ODIS project {} again: its folder changed since the read failed",
				part.what(),
				store.id
			),
			None => crate::say!("note: reading the {} of the ODIS project {}, the first time only", part.what(), store.id),
		}
		crate::note!("  from {}", crate::datadir::tilde(&folder));
		match read(&folder) {
			Ok(found) => {
				crate::note!("  {found}");
				forget_failure(store, part);
			}
			Err(e) => {
				let why = format!("{e:#}");
				remember_failure(
					store,
					part,
					Failure {
						dir,
						changed,
						why: why.clone(),
					},
				);
				return Ensured::Failed(why);
			}
		}
	}
	Ensured::Held
}

/// One failed read, as `.odis-read.json` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Failure {
	/// The folder it was read from, as the cache names it.
	dir: String,
	/// The folder's modification time then, in nanoseconds; a read is tried again when it moves.
	changed: String,
	why: String,
}

/// The record of failed reads beside a project's cache.
fn failures_file(store: &Project) -> PathBuf {
	store.dir.join(".odis-read.json")
}

fn failures(store: &Project) -> std::collections::BTreeMap<String, Failure> {
	std::fs::read_to_string(failures_file(store))
		.ok()
		.and_then(|text| serde_json::from_str(&text).ok())
		.unwrap_or_default()
}

fn failure(store: &Project, part: Part) -> Option<Failure> {
	failures(store).remove(part.key())
}

/// Why `part` of `store` did not read the last time it was tried, while it is still unread.
pub fn last_failure(store: &Project, part: Part) -> Option<String> {
	failure(store, part).map(|f| f.why)
}

/// Written whole; a record that cannot be written costs only a read tried again next time.
fn write_failures(store: &Project, all: &std::collections::BTreeMap<String, Failure>) {
	let path = failures_file(store);
	match all.is_empty() {
		true => {
			let _ = std::fs::remove_file(path);
		}
		false => {
			if let Ok(text) = serde_json::to_string_pretty(all) {
				let _ = std::fs::write(path, text);
			}
		}
	}
}

fn remember_failure(store: &Project, part: Part, failed: Failure) {
	let mut all = failures(store);
	all.insert(part.key().to_string(), failed);
	write_failures(store, &all);
}

fn forget_failure(store: &Project, part: Part) {
	let mut all = failures(store);
	if all.remove(part.key()).is_some() {
		write_failures(store, &all);
	}
}

/// A folder's modification time, in nanoseconds since the epoch; empty when it will not say.
fn changed_at(folder: &Path) -> String {
	std::fs::metadata(folder)
		.and_then(|m| m.modified())
		.ok()
		.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
		.map(|d| d.as_nanos().to_string())
		.unwrap_or_default()
}

/// Open the project at `dir` and write `part` of it into `store`'s cache, under the folder
/// the cache knows it by.
fn read(store: &Project, part: Part, dir: &Path) -> anyhow::Result<String> {
	let odis = vag_data_labels::odis::Project::open(dir).map_err(|e| anyhow::anyhow!("{e}"))?;
	let source = dir.to_string_lossy();
	match part {
		Part::UnitNames => {
			let named = odis.unit_names().map_err(|e| anyhow::anyhow!("{e}"))?;
			vag_data_db::put_unit_names(&store.cache(), &source, &named.units).map_err(|e| anyhow::anyhow!("{e}"))?;
			if named.skipped > 0 {
				crate::say!(
					"warning: {} {} of the ODIS project {} did not read; the units behind them go unnamed",
					named.skipped,
					if named.skipped == 1 { "logical link" } else { "logical links" },
					store.id
				);
			}
			Ok(format!("{} unit names for {} variants", links(&named.units), named.units.len()))
		}
		Part::Adaptations => {
			use rayon::prelude::*;
			let variants = odis.variants().map_err(|e| anyhow::anyhow!("{e}"))?;
			// A variant whose chain does not read costs itself, as in `setup`.
			let read: Vec<(String, Vec<vag_data_labels::odis::Reading>)> = variants
				.par_iter()
				.filter_map(|variant| match odis.adaptations(variant) {
					Ok(rows) if !rows.is_empty() => Some((variant.name.clone(), rows)),
					_ => None,
				})
				.collect();
			let landed = vag_data_db::put_all_adaptations(&store.cache(), &source, read.iter().map(|(v, rows)| (v.as_str(), rows.as_slice())))
				.map_err(|e| anyhow::anyhow!("{e}"))?;
			Ok(format!("{landed} adaptation channels of {} variants", read.len()))
		}
	}
}

/// How many logical links the rows name.
fn links(units: &[vag_data_labels::odis::UnitName]) -> usize {
	let mut links: Vec<&str> = units.iter().map(|u| u.link.as_str()).collect();
	links.sort_unstable();
	links.dedup();
	links.len()
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::atomic::{AtomicUsize, Ordering};

	/// A project read before unit names and adaptations were kept, from `folder`.
	fn unread_project(here: &Path, folder: &Path) -> Project {
		let store = Project {
			id: "SIM00".into(),
			dir: here.join("data").join("SIM00"),
		};
		std::fs::create_dir_all(&store.dir).unwrap();
		vag_data_db::record_language(&store.cache(), vag_data_db::ODIS, &folder.to_string_lossy(), "eng").unwrap();
		store
	}

	/// A read that writes made-up names under `folder`, and counts itself.
	fn reader<'a>(store: &'a Project, calls: &'a AtomicUsize) -> impl Fn(&Path) -> anyhow::Result<String> + 'a {
		move |folder| {
			calls.fetch_add(1, Ordering::SeqCst);
			let row = vag_data_labels::odis::UnitName {
				variant: "EV_SimDash_001".into(),
				base_variant: "BV_SimDash".into(),
				link: "LL_SimDash".into(),
				name: Some("Sim Dash".into()),
				address: Some(0x17),
			};
			vag_data_db::put_unit_names(&store.cache(), &folder.to_string_lossy(), &[row])?;
			Ok("1 unit name".into())
		}
	}

	#[test]
	fn a_part_never_read_is_read_from_the_projects_folder_once() {
		let here = tempfile::tempdir().unwrap();
		let folder = here.path().join("SIM00-odis");
		std::fs::create_dir_all(&folder).unwrap();
		let store = unread_project(here.path(), &folder);
		let calls = AtomicUsize::new(0);
		assert_eq!(ensure_with(&store, Part::UnitNames, reader(&store, &calls)), Ensured::Held);
		assert_eq!(calls.load(Ordering::SeqCst), 1);
		assert_eq!(vag_data_db::unit_names_of(&store.cache()).unwrap().len(), 1);
		// Read: the next run reads nothing.
		assert_eq!(ensure_with(&store, Part::UnitNames, reader(&store, &calls)), Ensured::Held);
		assert_eq!(calls.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn two_runs_at_once_read_once() {
		let here = tempfile::tempdir().unwrap();
		let folder = here.path().join("SIM00-odis");
		std::fs::create_dir_all(&folder).unwrap();
		let store = unread_project(here.path(), &folder);
		let calls = AtomicUsize::new(0);
		std::thread::scope(|s| {
			for _ in 0..4 {
				s.spawn(|| assert_eq!(ensure_with(&store, Part::UnitNames, reader(&store, &calls)), Ensured::Held));
			}
		});
		assert_eq!(calls.load(Ordering::SeqCst), 1);
	}

	#[test]
	fn a_folder_that_is_gone_is_said_and_nothing_is_read() {
		let here = tempfile::tempdir().unwrap();
		let folder = here.path().join("moved-away");
		let store = unread_project(here.path(), &folder);
		let calls = AtomicUsize::new(0);
		assert_eq!(ensure_with(&store, Part::UnitNames, reader(&store, &calls)), Ensured::Gone(folder));
		assert_eq!(calls.load(Ordering::SeqCst), 0);
	}

	#[test]
	fn a_read_that_fails_says_why() {
		let here = tempfile::tempdir().unwrap();
		let folder = here.path().join("SIM00-odis");
		std::fs::create_dir_all(&folder).unwrap();
		let store = unread_project(here.path(), &folder);
		// The folder is there and holds no project: the real read says so.
		match ensure(&store, Part::UnitNames) {
			Ensured::Failed(why) => assert!(!why.is_empty()),
			other => panic!("{other:?}"),
		}
		assert_eq!(
			vag_data_db::odis_sources_without_unit_names(&store.cache()).unwrap().len(),
			1,
			"still unread"
		);
	}

	/// A read that fails is tried once, and then not again until its folder changes — and the
	/// retry is not a "first time".
	#[test]
	fn a_read_that_failed_is_not_tried_again_until_the_folder_changes() {
		let here = tempfile::tempdir().unwrap();
		let folder = here.path().join("SIM00-odis");
		std::fs::create_dir_all(&folder).unwrap();
		let store = unread_project(here.path(), &folder);
		let calls = AtomicUsize::new(0);
		let failing = |_: &Path| -> anyhow::Result<String> {
			calls.fetch_add(1, Ordering::SeqCst);
			anyhow::bail!("no project here")
		};
		let failed = Ensured::Failed("no project here".into());
		assert_eq!(ensure_with(&store, Part::UnitNames, failing), failed);
		assert_eq!(ensure_with(&store, Part::UnitNames, failing), failed, "the same reason, given again");
		assert_eq!(calls.load(Ordering::SeqCst), 1, "not read again");
		assert_eq!(last_failure(&store, Part::UnitNames).as_deref(), Some("no project here"));
		// The folder changes: one more read.
		std::thread::sleep(std::time::Duration::from_millis(10));
		std::fs::write(folder.join("AStringData.data"), b"").unwrap();
		assert_eq!(ensure_with(&store, Part::UnitNames, failing), failed);
		assert_eq!(calls.load(Ordering::SeqCst), 2);
		// And a read that works clears the record.
		std::fs::write(folder.join("UStringData.data"), b"").unwrap();
		assert_eq!(ensure_with(&store, Part::UnitNames, reader(&store, &calls)), Ensured::Held);
		assert_eq!(last_failure(&store, Part::UnitNames), None);
	}

	#[test]
	fn the_adaptations_are_one_more_part_of_the_same_rule() {
		let here = tempfile::tempdir().unwrap();
		let folder = here.path().join("moved-away");
		let store = unread_project(here.path(), &folder);
		assert_eq!(ensure(&store, Part::Adaptations), Ensured::Gone(folder));
		vag_data_db::put_all_adaptations(&store.cache(), &here.path().join("moved-away").to_string_lossy(), std::iter::empty()).unwrap();
		assert_eq!(ensure(&store, Part::Adaptations), Ensured::Held, "read, none found");
	}
}
