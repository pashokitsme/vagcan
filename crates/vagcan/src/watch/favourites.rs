//! The handful of channels one person watches every time they drive.
//!
//! With a survey loaded, `watch` offers thousands of channels across fifteen
//! control units. Nearly all of a session is spent finding the same six of them
//! again, and nothing about that selection survived the run — the tool asked the
//! same question every drive and never learned the answer. `f` on the selection
//! screen is the answer being written down.
//!
//! **Stored in `~/.vagcan/config.toml`, keyed by VIN**, beside the other
//! settings a person edits by hand:
//!
//! ```toml
//! [favourites]
//! XW8AD4NE9JH008917 = ["7E0:202A:0", "7E1:380A:0"]
//! ```
//!
//! Keyed by VIN and not by project, for the reason that has not changed:
//! `data/<project>/` holds what is true of a *platform*, and `SK37X` covers
//! every Octavia III, Karoq and Kodiaq there is. Which identifiers a unit
//! answers is a fact about that unit as built, coded and installed in **this**
//! car, and a favourite is also a person's choice about the car in front of
//! them — stored per platform it would follow somebody onto a car that has never
//! answered that identifier.
//!
//! What did change is the file. One `favourites.json` per car put a person's
//! settings in as many places as they had cars, none of them the file they would
//! think to open; the marks are settings, and the settings live in one place.
//! Old per-car files are read once and folded in — see [`migrate`].
//!
//! The cost is stated rather than hidden: a car that will not report a VIN has
//! no key to store under, and a replay has no car at all. Both keep their
//! favourites for the length of the run and lose them at the end.
//!
//! A key is `<request>:<did>:<bit>`, all hex — see [`parse_key`].

use std::collections::BTreeSet;

use super::plan::Key;

/// What the per-car file used to be called, for [`migrate`].
pub const LEGACY_FILE: &str = "favourites.json";

/// Read a car's favourites, or none at all.
///
/// `None` for a car that reported no VIN and for a replay, which has no car:
/// there is no key to store under, and inventing one would file one car's
/// choices where another car reads them. The run still works; the marks simply
/// do not outlive it.
pub fn load(vin: Option<&str>) -> BTreeSet<Key> {
	let Some(vin) = vin else { return BTreeSet::new() };
	migrate(vin);
	from_list(&crate::config::favourites(&crate::config::load(), vin))
}

/// Write them back, and say what went wrong rather than throwing it away.
///
/// Called on every `f`, not once at the end: `watch` is quit with `q` on a
/// terminal in a car park and killed with a closed lid at least as often, and a
/// mark that only survives a tidy exit is a mark that does not survive.
///
/// Read-modify-write of the whole document, so a favourite saved mid-drive
/// cannot cost somebody the project they had set or a note they had left
/// themselves.
pub fn save(vin: Option<&str>, favourites: &BTreeSet<Key>) -> Result<(), String> {
	let Some(vin) = vin else {
		return Err("this run has no car to keep favourites under — they last until it ends".to_string());
	};
	let mut document = crate::config::load();
	let keys: Vec<String> = favourites.iter().map(|key| render_key(*key)).collect();
	crate::config::set_favourites(&mut document, vin, &keys);
	crate::config::save(&document).map_err(|why| format!("could not save favourites: {why}"))
}

/// Fold a pre-`config.toml` `favourites.json` into the settings, once.
///
/// Best-effort, and additive: the old file's marks are **merged** with whatever
/// the settings already hold rather than replacing them, because the two could
/// both have been written and neither is more recent by construction. The old
/// file is removed only after the settings are written.
pub fn migrate(vin: &str) {
	let Ok(dir) = crate::datadir::car_dir(vin) else { return };
	let legacy = dir.join(LEGACY_FILE);
	let Ok(text) = std::fs::read_to_string(&legacy) else { return };
	let mut marks = parse_legacy(&text);
	if marks.is_empty() {
		let _ = std::fs::remove_file(&legacy);
		return;
	}
	let mut document = crate::config::load();
	marks.extend(from_list(&crate::config::favourites(&document, vin)));
	let keys: Vec<String> = marks.iter().map(|key| render_key(*key)).collect();
	crate::config::set_favourites(&mut document, vin, &keys);
	if crate::config::save(&document).is_ok() {
		let _ = std::fs::remove_file(&legacy);
	}
}

/// The old per-car file's text as marks.
pub fn parse_legacy(text: &str) -> BTreeSet<Key> {
	let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
		return BTreeSet::new();
	};
	let Some(list) = value.get("favourites").and_then(|v| v.as_array()) else {
		return BTreeSet::new();
	};
	list.iter().filter_map(|entry| entry.as_str().and_then(parse_key)).collect()
}

/// A list of written keys as marks, dropping any this build cannot read.
pub fn from_list(keys: &[String]) -> BTreeSet<Key> {
	keys.iter().filter_map(|key| parse_key(key)).collect()
}

/// `"7E0:202A:2"` as the channel it names, or `None` when it is not one.
///
/// Every part is hex, and the unit is required: a bare identifier is refused
/// rather than assumed to be the engine's, because this is written by the tool
/// and read back by it, and guessing a control unit would silently move
/// somebody's mark elsewhere.
///
/// **The third part is the bit the field starts at, and a key written before it
/// existed has two parts.** Those read as bit 0, which is what they meant: back
/// then a channel was an identifier, and the only field anything could show was
/// the one at the start of the response.
pub fn parse_key(key: &str) -> Option<Key> {
	let mut parts = key.split(':');
	let request = u16::from_str_radix(parts.next()?.trim(), 16).ok()?;
	let did = u16::from_str_radix(parts.next()?.trim(), 16).ok()?;
	let offset = match parts.next() {
		Some(text) => u32::from_str_radix(text.trim(), 16).ok()?,
		None => 0,
	};
	// A fourth part is not a key this tool ever wrote.
	match parts.next() {
		Some(_) => None,
		None => Some((request, did, offset)),
	}
}

/// One mark, as the settings file spells it.
pub fn render_key((request, did, offset): Key) -> String {
	format!("{request:03X}:{did:04X}:{offset:X}")
}

#[cfg(test)]
mod tests {
	use super::*;

	fn set(keys: &[Key]) -> BTreeSet<Key> {
		keys.iter().copied().collect()
	}

	#[test]
	fn a_mark_is_written_and_read_back_as_the_same_channel() {
		// Where the marks are *stored* is `crate::config`'s business and is
		// tested there, against a temporary file. What belongs here is the
		// vocabulary: a key names a unit, an identifier and a field, and two
		// fields of one identifier are two different marks.
		let marks = set(&[(0x7E0, 0x202A, 0), (0x7E0, 0x202A, 16), (0x74B, 0x02BD, 0)]);
		let written: Vec<String> = marks.iter().map(|key| render_key(*key)).collect();

		// Sorted, because a `BTreeSet` is — a file that reshuffled itself on
		// every save is one nobody can diff or keep in a backup.
		assert_eq!(written, ["74B:02BD:0", "7E0:202A:0", "7E0:202A:10"]);
		assert_eq!(from_list(&written), marks);
	}

	#[test]
	fn a_key_this_build_cannot_read_costs_itself_and_nothing_else() {
		// These are a convenience. Failing a drive over a note about which rows
		// to tick is the wrong trade, so an unreadable one is dropped and the
		// rest stand.
		let written: Vec<String> = ["7E0:202A:0", "nonsense", "202A", "7E0:", ":202A", "7E0:202A:0:1"]
			.iter()
			.map(|s| s.to_string())
			.collect();
		assert_eq!(from_list(&written), set(&[(0x7E0, 0x202A, 0)]));
	}

	#[test]
	fn a_two_part_key_means_the_field_at_the_start_of_the_response() {
		// What every key written before a channel could be narrower than an
		// identifier meant, and the only reading that does not silently move
		// somebody's mark.
		assert_eq!(parse_key("7E0:202A"), Some((0x7E0, 0x202A, 0)));
	}

	#[test]
	fn the_old_per_car_file_still_reads() {
		// It has to, or a migration would quietly drop the marks it was written
		// to carry over.
		let marks = parse_legacy(r#"{"favourites": ["7E0:202A", "7E1:3816:0", "nonsense"]}"#);
		assert_eq!(marks, set(&[(0x7E0, 0x202A, 0), (0x7E1, 0x3816, 0)]));

		for broken in ["", "not json", "[]", "{}", r#"{"favourites": "7E0:202A"}"#] {
			assert!(parse_legacy(broken).is_empty(), "{broken:?}");
		}
	}

	#[test]
	fn nothing_here_writes_into_the_owners_own_settings() {
		// The rule this module broke the day it moved into `config.toml`: a
		// test that named a VIN saved a favourite into `~/.vagcan/config.toml`
		// on whoever ran `cargo test`. `save` is the only writer, it needs a
		// VIN, and no test may give it one.
		let source = include_str!("favourites.rs");
		let tests = source.split("mod tests").nth(1).expect("this module has tests");
		assert!(
			!tests.contains("save(Some("),
			"a test names a VIN to save under, which writes into the owner's real settings"
		);
	}

	#[test]
	fn a_run_with_no_car_has_nowhere_to_keep_them_and_says_so() {
		// A replay addresses nothing and a car that will not report a VIN has no
		// key to store under. Neither is an error; both are a mark that lasts
		// until the run ends, and the message has to say which.
		let why = save(None, &set(&[(0x7E0, 0x202A, 0)])).unwrap_err();
		assert!(why.contains("no car"), "{why}");
		assert!(load(None).is_empty());
	}
}
