//! `vagcan vcds names` — search the measurement names recovered from the label
//! label_files.
//!
//! `~/.vagcan/data/extracted/names.json` holds the names `vagcan setup` recovered by
//! breaking `TTTEXT.ROD`'s per-record substitution cipher
//! (`research/labels/tttext-codec.md`). They are keyed by the label files' own text id,
//! **not** by data identifier: the join from a name to the identifier that
//! carries it was shown to be structurally absent from the label files
//! (`research/labels/label-linkage.md` §3), and no amount of decryption puts it back.
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

/// Run the command (see the module docs).
pub fn run(needle: &str, limit: usize, path: &std::path::Path) -> Result<()> {
	// The ordinary reason this file is absent is that nobody has run `vagcan
	// setup` yet, and "No such file or directory" leaves a reader with nothing
	// to do about it. The catalog cannot ship with the tool — it is derived
	// from Ross-Tech's product — so a fresh checkout not having it is expected,
	// and saying which command makes it is the whole job here.
	if !path.is_file() {
		anyhow::bail!(crate::missing::no_label_data("The measurement names", "`vagcan vcds names`", path));
	}
	let text = std::fs::read_to_string(path).with_context(|| format!("reading the names catalog {}", path.display()))?;
	let catalog: serde_json::Value = serde_json::from_str(&text).context("parsing the names catalog")?;
	let hits = search(&catalog, needle);

	if hits.is_empty() {
		println!(
			"No name in the label files contain {needle:?}.\n\n\
             The catalog is one car's label_files, in English, and it is a list of \n\
             names only — a name it lacks may still exist on another model."
		);
		return Ok(());
	}

	let shown = hits.len().min(limit);
	for (id, text) in hits.iter().take(shown) {
		println!("  {id}  {text}");
	}
	if hits.len() > shown {
		println!("\n… {} more — showing {shown}, raise it with --limit", hits.len() - shown);
	}
	println!(
		"\n{} of {} names matched.\n\n\
         These are names, not addresses: the label files do not record which data \n\
         identifier carries which name, so a match here is a hypothesis to test \n\
         against the car, not an identification.",
		hits.len(),
		catalog.as_object().map(|m| m.len()).unwrap_or(0)
	);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn catalog() -> serde_json::Value {
		serde_json::json!({
				"000080": "Absolute intake pressure",
				"000097": "ACC specified acceleration",
				"012389": "Button for rear lid unlocking in rear lid",
		})
	}

	#[test]
	fn a_search_is_case_insensitive_and_matches_anywhere_in_the_name() {
		let c = catalog();
		assert_eq!(search(&c, "intake"), vec![("000080", "Absolute intake pressure")]);
		assert_eq!(search(&c, "ABSOLUTE"), vec![("000080", "Absolute intake pressure")]);
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
		let err = run("boost", 40, std::path::Path::new("/definitely/not/here/names.json"))
			.unwrap_err()
			.to_string();
		assert!(err.contains("vagcan setup"), "{err}");
		assert!(err.contains("/definitely/not/here/names.json"), "{err}");
	}
}
