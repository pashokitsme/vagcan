//! `UNIT.ROD`: the engineering units a registry row's `f6` names.
//!
//! ```text
//! registry row, f6 = unit id ──▶ UDS_EV/UNIT.ROD [UNT] ──▶ <id>,<unit>, the unit under srand(id)
//! ```
//!
//! 250 units in VCDS 26.3 — `%`, `°C`, `/min`, `bar`, `mm`, `km`, `km/h`, `m/s`
//! among them. The cipher covers letters and digits and leaves `°`, `%` and `/`
//! as they are, which is how [`TableAlphabet::decipher`] reads a text; the high
//! bytes are in the build's code page, as `TTTEXT`'s are.
//!
//! The Russian build keeps its units in `Unit-RUS.rod`, in the shifted regime:
//! not readable offline (`research/vcds-registry/README.md` §6a).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::codes::CodePage;
use crate::dtc::find_named;
use crate::glyphs::TableAlphabet;
use crate::rod::{IvCache, KeyCost, RodStatus, decode_rod_recover, key_cost, recover_classic_key};

/// The unit table's own file name, inside a VCDS install.
pub const UNITS_FILE: &str = "UNIT.ROD";

/// The section the units are read from.
pub const UNIT_SECTION: &str = "UNT";

/// Every engineering unit of an install, by id.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitStrings {
	units: BTreeMap<u32, String>,
	/// Lines with no `<id>,` in front. Counted, not guessed at.
	unread: usize,
}

impl UnitStrings {
	/// Parse a decoded `[UNT]` section, its bytes in the build's code page.
	pub fn parse(section: &[u8], page: CodePage) -> Self {
		let mut out = Self::default();
		for line in section.split(|b| *b == b'\n').map(|line| line.strip_suffix(b"\r").unwrap_or(line)) {
			if line.is_empty() {
				continue;
			}
			let read = line.iter().position(|b| *b == b',').and_then(|comma| {
				let id: u32 = std::str::from_utf8(&line[..comma]).ok()?.parse().ok()?;
				Some((id, TableAlphabet::for_key(id).decipher(&page.decode(&line[comma + 1..]))))
			});
			match read {
				Some((id, unit)) => {
					out.units.insert(id, unit);
				}
				None => out.unread += 1,
			}
		}
		out
	}

	/// The unit an id names, `None` when the table has no such id.
	pub fn get(&self, id: u32) -> Option<&str> {
		self.units.get(&id).map(String::as_str)
	}

	/// How many lines did not read.
	pub fn unread(&self) -> usize {
		self.unread
	}

	pub fn len(&self) -> usize {
		self.units.len()
	}

	pub fn is_empty(&self) -> bool {
		self.units.is_empty()
	}
}

/// What opening the unit table came to.
#[derive(Debug, Clone)]
pub enum UnitsLoad {
	Found {
		file: PathBuf,
		units: UnitStrings,
	},
	/// No `UNIT.ROD` in the install — the Russian build has none.
	NoFile,
	/// The key is not cached: one classic search, about 92 CPU-s on 26.3.
	Locked {
		file: PathBuf,
	},
	/// Shifted in this install.
	Shifted {
		file: PathBuf,
	},
}

/// Open the unit table of a VCDS install. With `crack`, a classic section
/// whose key is not cached is searched for; a shifted one never is.
pub fn load_unit_strings(root: &Path, cache: &mut IvCache, crack: bool, page: CodePage) -> UnitsLoad {
	let Some(file) = find_named(root, UNITS_FILE) else {
		return UnitsLoad::NoFile;
	};
	let Ok(bytes) = std::fs::read(&file) else { return UnitsLoad::NoFile };
	let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
	if crack {
		recover_classic_key(&bytes, &name, UNIT_SECTION, cache);
	}
	let section = decode_rod_recover(&bytes, &name, cache, false)
		.into_iter()
		.find(|s| s.tag == UNIT_SECTION);
	match section {
		Some(s) if matches!(s.status, RodStatus::Tea | RodStatus::Zlib) && s.text.is_some() => {
			// The container reads a section one byte to one char; the code page
			// is applied here, to the bytes, as `TTTEXT`'s reader does.
			let raw: Vec<u8> = s.text.as_deref().unwrap_or_default().chars().map(|c| c as u32 as u8).collect();
			UnitsLoad::Found {
				units: UnitStrings::parse(&raw, page),
				file,
			}
		}
		Some(s) if s.status == RodStatus::SearchDeclined => UnitsLoad::Shifted { file },
		_ => match key_cost(&bytes, UNIT_SECTION) {
			Some(KeyCost::AnchorSweep) => UnitsLoad::Shifted { file },
			_ => UnitsLoad::Locked { file },
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::rod::testkit::section;

	/// A section line as VCDS writes one, the unit in the build's code page.
	fn line(id: u32, unit: &str) -> Vec<u8> {
		let mut out = format!("{id:06},").into_bytes();
		out.extend(TableAlphabet::for_key(id).encipher(unit).chars().map(|c| c as u32 as u8));
		out.extend_from_slice(b"\r\n");
		out
	}

	/// The ids are invented: which id VCDS gives which unit is Ross-Tech's table.
	#[test]
	fn a_unit_reads_through_its_ids_alphabet_with_the_signs_left_alone() {
		let section = [
			line(901, "%"),
			line(902, "°C"),
			line(903, "/min"),
			line(904, "km/h"),
			b"nonsense\r\n".to_vec(),
		]
		.concat();
		let units = UnitStrings::parse(&section, CodePage::Windows1252);
		assert_eq!(units.get(902), Some("°C"));
		assert_eq!(units.get(903), Some("/min"));
		assert_eq!(units.get(904), Some("km/h"));
		assert_eq!(units.get(901), Some("%"));
		assert_eq!(units.get(905), None);
		assert_eq!(units.unread(), 1);
	}

	#[test]
	fn the_units_open_from_an_install() {
		let root = std::env::temp_dir().join(format!("vagcan-units-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&root);
		std::fs::create_dir_all(root.join("UDS_EV")).unwrap();
		let text = [line(902, "°C"), line(903, "/min")].concat();
		std::fs::write(root.join("UDS_EV").join(UNITS_FILE), section(UNIT_SECTION, &text, true, None)).unwrap();
		let opened = load_unit_strings(&root, &mut IvCache::default(), false, CodePage::Windows1252);
		let _ = std::fs::remove_dir_all(&root);
		let UnitsLoad::Found { units, .. } = opened else {
			panic!("did not open: {opened:?}")
		};
		assert_eq!((units.get(902), units.get(903)), (Some("°C"), Some("/min")));
	}
}
