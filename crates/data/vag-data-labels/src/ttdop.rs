//! `TTDOP.rod`: the text tables a registry row of type 3 names — which raw
//! value means which state.
//!
//! ```text
//! registry row, type 3 ──▶ f1 = table ──▶ UDS_EV/TTDOP.rod [DOP], every row keyed by table
//! each row, in the alphabet the table generates ──▶ <lower>,<upper>,<text id>,
//! ```
//!
//! On the reference gearbox the selected gear is such a table: one state per
//! raw value the gearbox sends, neutral and reverse among them. A state's
//! words are a `TTTEXT` record, named here by its id. Joining the two is the
//! reader of `TTTEXT`'s business, not this one's.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::dtc::find_named;
use crate::glyphs::TableAlphabet;
use crate::rod::{IvCache, KeyCost, RodStatus, decode_rod_recover, key_cost, recover_classic_key};

/// The text tables' own file name, inside a VCDS install.
pub const TEXT_TABLES_FILE: &str = "TTDOP.rod";

/// The section the tables are read from.
pub const DOP_SECTION: &str = "DOP";

/// One state of a text table: every raw value from `lower` to `upper`, both
/// included, reads as the `TTTEXT` record `text_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextLevel {
	pub lower: i64,
	pub upper: i64,
	pub text_id: u32,
}

/// Every text table of an install, by table id, each in file order.
#[derive(Debug, Clone, Default)]
pub struct TextTables {
	tables: BTreeMap<u32, Vec<TextLevel>>,
	/// Rows that did not read as `<lower>,<upper>,<text id>`. Counted rather
	/// than guessed at: a table missing a row is a table with a state unnamed.
	unread: usize,
	/// The tables such a row belongs to, where its table id read.
	lossy: std::collections::BTreeSet<u32>,
	/// Such rows whose table id did not read either.
	unattributed: usize,
}

impl TextTables {
	/// Parse a decoded `[DOP]` section of `TTDOP.rod`.
	pub fn parse(text: &str) -> Self {
		let mut out = Self::default();
		for line in text.split("\r\n").filter(|line| !line.is_empty()) {
			match read_level(line) {
				Some((table, level)) => {
					// Read, and still a state no channel's level can hold: a bound
					// past `i32` is left out where a channel is built from the table
					// (`registry::to_reading`), so it is charged to its table here.
					if i32::try_from(level.lower).is_err() || i32::try_from(level.upper).is_err() {
						out.lossy.insert(table);
					}
					out.tables.entry(table).or_default().push(level)
				}
				None => {
					out.unread += 1;
					match line.split_once(',').and_then(|(table, _)| table.parse().ok()) {
						Some(table) => {
							out.lossy.insert(table);
						}
						None => out.unattributed += 1,
					}
				}
			}
		}
		out
	}

	/// The states of one table, in file order, or `None` if the file has no
	/// such table.
	pub fn table(&self, id: u32) -> Option<&[TextLevel]> {
		self.tables.get(&id).map(Vec::as_slice)
	}

	/// How many rows did not read.
	pub fn unread(&self) -> usize {
		self.unread
	}

	/// Whether one of table `id`'s states is left out: a row that did not read
	/// — in 26.3 a bound written as a 17-byte string, and states whose text id
	/// is `-2` — or a bound past `i32`, which no channel's level holds (236
	/// states in 51 tables of 26.3).
	pub fn lost_states(&self, id: u32) -> bool {
		self.lossy.contains(&id)
	}

	/// How many rows did not read with no table they can be charged to.
	pub fn unattributed(&self) -> usize {
		self.unattributed
	}

	pub fn len(&self) -> usize {
		self.tables.len()
	}

	pub fn is_empty(&self) -> bool {
		self.tables.is_empty()
	}
}

/// Read one `<table>,<payload>` row through the alphabet its table generates.
fn read_level(line: &str) -> Option<(u32, TextLevel)> {
	let (table, payload) = line.split_once(',')?;
	let table: u32 = table.parse().ok()?;
	let alphabet = TableAlphabet::for_key(table);
	let mut fields = payload.split(alphabet.separator());
	let mut next = || alphabet.decode(fields.next()?);
	let lower = bound(&next()?)?;
	let upper = bound(&next()?)?;
	let text_id = next()?.parse().ok()?;
	Some((table, TextLevel { lower, upper, text_id }))
}

/// A state's bound: decimal, or hexadecimal where the row says so with `0x`.
fn bound(text: &str) -> Option<i64> {
	match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
		Some(hex) => i64::from_str_radix(hex, 16).ok(),
		None => text.parse().ok(),
	}
}

/// What opening the text tables came to.
#[derive(Debug, Clone)]
pub enum TablesLoad {
	Found {
		file: PathBuf,
		tables: TextTables,
	},
	/// No `TTDOP.rod` in the install.
	NoFile,
	/// The key is not cached: one classic search, minutes.
	Locked {
		file: PathBuf,
	},
	/// Shifted in this install. It was classic in every install checked; this
	/// arm exists so a future one says so rather than looks empty.
	Shifted {
		file: PathBuf,
	},
}

/// Open the text tables of a VCDS install. With `crack`, a classic section
/// whose key is not cached is searched for; a shifted one never is.
pub fn load_text_tables(root: &Path, cache: &mut IvCache, crack: bool) -> TablesLoad {
	let Some(file) = find_named(root, TEXT_TABLES_FILE) else {
		return TablesLoad::NoFile;
	};
	let Ok(bytes) = std::fs::read(&file) else { return TablesLoad::NoFile };
	let name = file.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
	if crack {
		recover_classic_key(&bytes, &name, DOP_SECTION, cache);
	}
	let section = decode_rod_recover(&bytes, &name, cache, false).into_iter().find(|s| s.tag == DOP_SECTION);
	match section {
		Some(s) if matches!(s.status, RodStatus::Tea | RodStatus::Zlib) && s.text.is_some() => TablesLoad::Found {
			tables: TextTables::parse(s.text.as_deref().unwrap_or_default()),
			file,
		},
		Some(s) if s.status == RodStatus::SearchDeclined => TablesLoad::Shifted { file },
		_ => match key_cost(&bytes, DOP_SECTION) {
			Some(KeyCost::AnchorSweep) => TablesLoad::Shifted { file },
			_ => TablesLoad::Locked { file },
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::rod::testkit::section;

	fn row(table: u32, fields: &str) -> String {
		format!("{table:06},{}", TableAlphabet::for_key(table).encipher(fields))
	}

	#[test]
	fn a_table_reads_as_its_states_in_file_order() {
		let text = [
			row(6051, "1,1,7001,"),
			row(6051, "4,4,7002,"),
			row(6051, "11,11,7003,"),
			row(7, "-40,-1,5,"),
		]
		.join("\r\n");
		let tables = TextTables::parse(&text);
		assert_eq!(tables.len(), 2);
		assert_eq!(
			tables.table(6051).unwrap(),
			&[
				TextLevel {
					lower: 1,
					upper: 1,
					text_id: 7001
				},
				TextLevel {
					lower: 4,
					upper: 4,
					text_id: 7002
				},
				TextLevel {
					lower: 11,
					upper: 11,
					text_id: 7003
				},
			]
		);
		assert_eq!(
			tables.table(7).unwrap(),
			&[TextLevel {
				lower: -40,
				upper: -1,
				text_id: 5
			}],
			"a range, and negative bounds"
		);
		assert_eq!(tables.table(3), None);
		assert_eq!(tables.unread(), 0);
	}

	#[test]
	fn a_row_that_does_not_read_is_charged_to_its_table() {
		// So a caller can say which of *its* rows lost a state, rather than
		// that a table somewhere in the file did — in 26.3 four rows of 17,935
		// tables, none of them a table the reference car's units name.
		let text = [
			row(9, "0,0,7001,"),
			row(9, "1,1,-2,"),
			"x,y".to_string(),
			row(12, "0,0,7002,"),
			row(14, "0x100000000,0x100000000,7003,"),
		]
		.join("\r\n");
		let tables = TextTables::parse(&text);
		assert!(tables.lost_states(9));
		assert!(!tables.lost_states(12));
		// Read, and still a state no channel's level can hold: its bounds pass
		// `i32`, and `registry::to_reading` leaves it out.
		assert!(tables.lost_states(14));
		assert_eq!(tables.unattributed(), 1, "a row whose table does not read either");
		assert_eq!(tables.unread(), 2);
	}

	#[test]
	fn a_bound_written_in_hex_reads_as_hex() {
		// Most rows that did not read wrote a bound as `0x…`: a stated base, not
		// a guess at one.
		let text = [row(9, "0x0A,0x1F,7005,"), row(9, "0X20,32,7006,")].join("\r\n");
		let tables = TextTables::parse(&text);
		assert_eq!(
			tables.table(9).unwrap(),
			&[
				TextLevel {
					lower: 10,
					upper: 31,
					text_id: 7005
				},
				TextLevel {
					lower: 32,
					upper: 32,
					text_id: 7006
				},
			]
		);
		assert_eq!(tables.unread(), 0);
	}

	#[test]
	fn a_row_that_does_not_read_is_counted_not_guessed() {
		let text = [row(1, "0,0,1,"), row(1, "0,0"), "x,y".to_string(), row(1, "0,zz,1,")].join("\r\n");
		let tables = TextTables::parse(&text);
		assert_eq!(tables.table(1).unwrap().len(), 1);
		assert_eq!(tables.unread(), 3);
	}

	#[test]
	fn the_tables_open_from_an_install() {
		let root = std::env::temp_dir().join(format!("vagcan-ttdop-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&root);
		std::fs::create_dir_all(root.join("UDS_EV")).unwrap();
		let text = [row(6051, "1,1,7001,"), row(6051, "11,11,7003,")].join("\r\n");
		std::fs::write(
			root.join("UDS_EV").join(TEXT_TABLES_FILE),
			section(DOP_SECTION, text.as_bytes(), true, None),
		)
		.unwrap();
		let opened = load_text_tables(&root, &mut IvCache::default(), false);
		let _ = std::fs::remove_dir_all(&root);
		let TablesLoad::Found { tables, .. } = opened else {
			panic!("did not open: {opened:?}")
		};
		assert_eq!(tables.table(6051).unwrap().len(), 2);
	}
}
