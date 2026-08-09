//! The handful of channels one person watches every time they drive.
//!
//! With a survey loaded, `watch` offers 2,751 channels across fifteen control
//! units. Nearly all of a session is spent finding the same six of them again,
//! and nothing about that selection survived the run — the tool asked the same
//! question every drive and never learned the answer. `f` on the selection
//! screen is the answer being written down.
//!
//! **They live under the car, not under the project** —
//! `~/.vagcan/cars/<VIN>/favourites.json`. The two stores are keyed differently
//! on purpose (`crate::datadir`): `data/<project>/` holds what is true of a
//! *platform*, and a proven scaling is one of those, because it is a property
//! of a part number that every car carrying that part shares. A favourite is
//! neither. It is a set of `(request id, identifier)` pairs, and which
//! identifiers a control unit answers is a fact about that unit as it is built,
//! coded and installed in **this** car — the same reasoning that already puts
//! `survey.jsonl` beside the car file. It is also a person's choice about the
//! car in front of them, and `SK37X` covers every Octavia III, Karoq and Kodiaq
//! there is; a favourite stored there would follow somebody onto a car that has
//! never answered that identifier.
//!
//! The cost of that choice is stated rather than hidden: a car that will not
//! report a VIN has nowhere of its own to write, and a replay has no car at
//! all. Both keep their favourites for the length of the run and lose them at
//! the end — see [`path_for`], which is the only thing that decides where a
//! file goes.
//!
//! The file is small, plain and hand-editable, like everything else under
//! `~/.vagcan`:
//!
//! ```json
//! { "favourites": ["7E0:202A", "7E1:3816"] }
//! ```
//!
//! An object rather than a bare array so a later version can put something
//! beside it without every older file becoming unreadable.

use std::collections::BTreeSet;
use std::path::PathBuf;

/// What the file is called, beside the car file and the survey.
pub const FILE: &str = "favourites.json";

/// Where this car's favourites go, when it said which car it is.
///
/// `None` for a car that reported no VIN and for a replay, which has no car:
/// there is no per-car directory to write into, and inventing one — under the
/// working directory, or under a project — would file one car's choices where
/// another car reads them. The run still works; the marks simply do not
/// outlive it.
pub fn path_for(vin: Option<&str>) -> Option<PathBuf> {
	let vin = vin?;
	crate::datadir::car_dir(vin).ok().map(|dir| dir.join(FILE))
}

/// Read the favourites out of a file's text.
///
/// Anything unreadable is no favourites rather than an error. This file is a
/// convenience, and losing a drive because a note about which rows to tick
/// would not parse is the wrong trade — the same judgement `crate::project`
/// makes about `config.json`.
pub fn parse(text: &str) -> BTreeSet<(u16, u16)> {
	let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
		return BTreeSet::new();
	};
	let Some(list) = value.get("favourites").and_then(|v| v.as_array()) else {
		return BTreeSet::new();
	};
	list.iter().filter_map(|entry| entry.as_str().and_then(parse_key)).collect()
}

/// `"7E0:202A"` as the pair it names, or `None` when it is not one.
///
/// Both halves are hex and both are required. A bare identifier is refused
/// rather than assumed to be the engine's: this file is written by the tool and
/// read back by it, and guessing a control unit would silently move somebody's
/// mark to another unit.
fn parse_key(key: &str) -> Option<(u16, u16)> {
	let (request, did) = key.split_once(':')?;
	Some((u16::from_str_radix(request.trim(), 16).ok()?, u16::from_str_radix(did.trim(), 16).ok()?))
}

/// The file's whole text, ready to write.
///
/// Ordered, because a `BTreeSet` is, and because a file that reshuffles itself
/// on every save is a file nobody can diff or keep in a backup.
pub fn render(favourites: &BTreeSet<(u16, u16)>) -> String {
	let keys: Vec<String> = favourites.iter().map(|(request, did)| format!("{request:03X}:{did:04X}")).collect();
	serde_json::to_string_pretty(&serde_json::json!({ "favourites": keys })).unwrap_or_else(|_| "{}".to_string())
}

/// Read a car's favourites, or none at all.
pub fn load(path: Option<&std::path::Path>) -> BTreeSet<(u16, u16)> {
	path
		.and_then(|path| std::fs::read_to_string(path).ok())
		.map(|text| parse(&text))
		.unwrap_or_default()
}

/// Write them back, and say what went wrong rather than throwing it away.
///
/// Called on every `f`, not once at the end: `watch` is quit with `q` on a
/// terminal in a car park and killed with a closed lid at least as often, and a
/// mark that only survives a tidy exit is a mark that does not survive.
pub fn save(path: Option<&std::path::Path>, favourites: &BTreeSet<(u16, u16)>) -> Result<(), String> {
	let Some(path) = path else {
		return Err("this run has no car to keep favourites under — they last until it ends".to_string());
	};
	if let Some(parent) = path.parent() {
		std::fs::create_dir_all(parent).map_err(|why| format!("could not create {}: {why}", parent.display()))?;
	}
	// The path goes last, because it is as long as somebody's home directory
	// makes it and a sentence continuing after it is a sentence that gets cut.
	std::fs::write(path, render(favourites)).map_err(|why| format!("could not save favourites: {why}: {}", path.display()))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn set(pairs: &[(u16, u16)]) -> BTreeSet<(u16, u16)> {
		pairs.iter().copied().collect()
	}

	#[test]
	fn what_is_written_is_what_comes_back() {
		let marks = set(&[(0x7E0, 0x202A), (0x7E1, 0x3816), (0x74B, 0x02BD)]);
		assert_eq!(parse(&render(&marks)), marks);
		// And the text is the plain, hand-editable thing the rest of ~/.vagcan
		// is, keyed the way this tool writes identifiers everywhere else.
		let text = render(&marks);
		assert!(text.contains("\"7E0:202A\""), "{text}");
		assert!(text.contains("\"74B:02BD\""), "{text}");
	}

	#[test]
	fn a_file_that_will_not_parse_is_no_favourites_rather_than_a_lost_drive() {
		// This file is a convenience. Failing the run over it would take `watch`
		// off a car for the sake of a note about which rows to tick.
		for broken in ["", "not json", "[]", "{}", r#"{"favourites": "7E0:202A"}"#] {
			assert!(parse(broken).is_empty(), "{broken:?}");
		}
		// A malformed entry costs itself and nothing else.
		let mixed = parse(r#"{"favourites": ["7E0:202A", "nonsense", "202A", "7E0:", ":202A"]}"#);
		assert_eq!(mixed, set(&[(0x7E0, 0x202A)]));
	}

	#[test]
	fn a_run_with_no_car_has_nowhere_to_keep_them_and_says_so() {
		// A replay addresses nothing and a car that will not report a VIN has no
		// directory of its own. Neither is an error; both are a mark that lasts
		// until the run ends, and the message has to say which.
		assert_eq!(path_for(None), None);
		let why = save(None, &set(&[(0x7E0, 0x202A)])).unwrap_err();
		assert!(why.contains("no car"), "{why}");
		assert!(load(None).is_empty());
	}

	#[test]
	fn favourites_are_kept_under_the_car_and_never_in_a_checkout() {
		// A favourite is a set of identifiers, and which identifiers a unit
		// answers is a fact about that unit in *this* car — the same reason
		// `survey.jsonl` sits beside the car file rather than under a project,
		// which covers every Octavia III, Karoq and Kodiaq there is.
		let path = path_for(Some("XW8AD4NE9JH008917")).expect("a home directory exists wherever tests run");
		assert!(path.ends_with(format!("XW8AD4NE9JH008917/{FILE}")), "{path:?}");
		let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap();
		assert!(!path.starts_with(&repo), "{path:?} is inside the checkout");
		// Two cars never share one file, and nothing off the bus chooses where
		// this tool writes.
		assert_ne!(path, path_for(Some("XW8AD4NE9JH008918")).unwrap());
		assert_eq!(path_for(Some("../../etc")), None);
	}

	#[test]
	fn saving_names_the_file_it_could_not_write() {
		// A message that interpolates a path ends with it: this one is as long
		// as somebody's home directory makes it, and anything after it is what
		// gets cut off the end of a line.
		let here = tempfile::tempdir().unwrap();
		let path = here.path().join("nested/deeper").join(FILE);
		save(Some(&path), &set(&[(0x7E0, 0x202A)])).expect("it creates what it needs");
		assert_eq!(load(Some(&path)), set(&[(0x7E0, 0x202A)]));

		// A directory where the file should be is the ordinary shape of an
		// unwritable path, and the answer names it.
		let blocked = here.path().join("blocked");
		std::fs::create_dir_all(&blocked).unwrap();
		let why = save(Some(&blocked), &set(&[(0x7E0, 0x202A)])).unwrap_err();
		assert!(why.ends_with(&blocked.display().to_string()), "{why}");
	}
}
