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
		crate::say!("note: reading the {} of the ODIS project {}, the first time only", part.what(), store.id);
		crate::note!("  from {}", crate::datadir::tilde(&folder));
		match read(&folder) {
			Ok(found) => crate::note!("  {found}"),
			Err(e) => return Ensured::Failed(format!("{e:#}")),
		}
	}
	Ensured::Held
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
