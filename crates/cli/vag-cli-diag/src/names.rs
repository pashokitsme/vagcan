//! `vagcan dev vcds names` — search the names in the label files' global text
//! table.
//!
//! A project's `names.json` holds every record of `TTTEXT.ROD`, which `vagcan
//! setup` reads exactly — each record under its own key
//! (`research/vcds-registry/README.md`). They are keyed by the label files' own
//! text id, **not** by data identifier. The label files do say which identifier
//! carries a name — in the `RM.rod` registry, which `setup` reads into a
//! project's channels — but `names.json` holds the names alone, so this list
//! cannot.
//!
//! So this command cannot name a scan result for you. What it can do is answer
//! "does this car's label files have a name that sounds like the thing I am
//! looking at" — which is how a sweep result gets a hypothesis worth testing
//! on the car. The pairing still has to be earned by making the value move.

use anyhow::{Context, Result};

/// Every name whose text contains `needle`, case-insensitively, with its text
/// id.
pub fn search<'a>(catalog: &'a serde_json::Value, needle: &str) -> Vec<(&'a str, &'a str)> {
	let needle = needle.to_lowercase();
	let Some(map) = catalog.as_object() else { return Vec::new() };
	let mut out: Vec<(&str, &str)> = map
		.iter()
		.filter_map(|(id, value)| {
			let text = value.as_str()?;
			text.to_lowercase().contains(&needle).then_some((id.as_str(), text))
		})
		.collect();
	// By id, so two runs of the same search list the same way.
	out.sort_unstable();
	out
}

/// One names file to search, and which source wrote it.
pub struct NamesFile {
	/// `VCDS` or `ODIS`, shown beside a hit when more than one file is searched.
	pub source: &'static str,
	pub path: std::path::PathBuf,
}

/// The files `names` searches when none is named: the project's `names.json`
/// (a VCDS installation's) and `names-odis.json` (an ODIS project's), whichever
/// `setup` wrote. A project set up from ODIS alone has only the second.
pub fn project_files(project: &crate::project::Project) -> Vec<NamesFile> {
	vec![
		NamesFile {
			source: "VCDS",
			path: project.names(),
		},
		NamesFile {
			source: "ODIS",
			path: project.odis_names(),
		},
	]
}

/// Run the command (see the module docs) over every file of `files` that is
/// there.
pub fn run(needle: &str, limit: usize, files: &[NamesFile]) -> Result<()> {
	// The ordinary reason no file is there is that nobody has run `vagcan
	// setup` yet, and "No such file or directory" leaves a reader with nothing
	// to do about it. The names cannot ship with the tool — they are derived
	// from Ross-Tech's and VW's data — so a fresh checkout not having them is
	// expected, and saying which command makes them is the whole job here.
	let present: Vec<&NamesFile> = files.iter().filter(|f| f.path.is_file()).collect();
	if present.is_empty() {
		let headline = "The measurement names are not on this machine, and `vagcan dev vcds names` needs them.";
		let missing = crate::missing::NoLabelData::new(headline);
		anyhow::bail!(match files {
			[one] => missing.looked_for(&one.path),
			_ => missing.looked_in(files.first().and_then(|f| f.path.parent()).unwrap_or(std::path::Path::new("."))),
		});
	}
	let mut catalogs = Vec::new();
	for file in &present {
		let text = std::fs::read_to_string(&file.path).with_context(|| format!("reading the names file {}", file.path.display()))?;
		let catalog: serde_json::Value = serde_json::from_str(&text).with_context(|| format!("parsing the names file {}", file.path.display()))?;
		catalogs.push((file.source, catalog));
	}
	let tagged = catalogs.len() > 1;
	let hits: Vec<(&str, &str, &str)> = catalogs
		.iter()
		.flat_map(|(source, catalog)| search(catalog, needle).into_iter().map(move |(id, text)| (*source, id, text)))
		.collect();
	let total: usize = catalogs.iter().map(|(_, c)| c.as_object().map_or(0, |m| m.len())).sum();

	if hits.is_empty() {
		println!(
			"No name contains {needle:?}.\n\n\
             The names are those of the VCDS installation or ODIS project this project was\n\
             set up from. Another build may have a name this one lacks."
		);
		return Ok(());
	}

	let shown = hits.len().min(limit);
	for (source, id, text) in hits.iter().take(shown) {
		if tagged {
			println!("  {source}  {id}  {text}");
		} else {
			println!("  {id}  {text}");
		}
	}
	if hits.len() > shown {
		println!("\n… {} more — showing {shown}, raise it with --limit", hits.len() - shown);
	}
	println!(
		"\n{} of {total} names matched.\n\n\
         These are names, not addresses: this list does not say which data\n\
         identifier carries which name, so a match is a guess to test on the car.",
		hits.len(),
	);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Invented names: the label files' own are Ross-Tech's.
	fn catalog() -> serde_json::Value {
		serde_json::json!({
				"000017": "Synthetic manifold pressure",
				"000097": "Cruise target distance",
				"012389": "Lid switch in the rear lid",
		})
	}

	#[test]
	fn a_search_is_case_insensitive_and_matches_anywhere_in_the_name() {
		let c = catalog();
		assert_eq!(search(&c, "manifold"), vec![("000017", "Synthetic manifold pressure")]);
		assert_eq!(search(&c, "SYNTHETIC"), vec![("000017", "Synthetic manifold pressure")]);
		assert_eq!(search(&c, "pressure").len(), 1);
		assert!(search(&c, "boost").is_empty(), "no invented synonyms");
	}

	#[test]
	fn results_are_ordered_so_two_runs_agree() {
		let c = catalog();
		let hits = search(&c, "e");
		let mut sorted = hits.clone();
		sorted.sort_unstable();
		assert_eq!(hits, sorted);
	}

	#[test]
	fn a_catalog_nobody_has_recovered_yet_is_a_message_not_an_errno() {
		// This used to assert 17,009 names against a file committed to the
		// repository. The file is derived from a VCDS installation and no
		// longer ships, so the interesting behaviour is what happens on a
		// machine where `vagcan setup` has not been run — which is every fresh
		// checkout.
		let one = [NamesFile {
			source: "VCDS",
			path: "/definitely/not/here/names.json".into(),
		}];
		let err = run("boost", 40, &one).unwrap_err().to_string();
		assert!(err.contains("vagcan setup"), "{err}");
		assert!(err.contains("/definitely/not/here/names.json"), "{err}");
	}

	#[test]
	fn a_project_set_up_from_odis_alone_is_searched_through_its_odis_names() {
		// `setup` from an ODIS project writes `names-odis.json` and no
		// `names.json`; the search used to refuse such a project.
		let dir = std::env::temp_dir().join(format!("vagcan-names-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&dir);
		std::fs::create_dir_all(&dir).unwrap();
		let project = crate::project::Project {
			id: "X".into(),
			dir: dir.clone(),
		};
		assert!(
			run("boost", 40, &project_files(&project)).is_err(),
			"neither file: still the setup message"
		);
		std::fs::write(project.odis_names(), catalog().to_string()).unwrap();
		run("manifold", 40, &project_files(&project)).expect("the ODIS names alone are enough");
		std::fs::write(project.names(), catalog().to_string()).unwrap();
		run("manifold", 40, &project_files(&project)).expect("both files are searched");
		let _ = std::fs::remove_dir_all(&dir);
	}
}
