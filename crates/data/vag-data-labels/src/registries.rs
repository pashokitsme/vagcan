//! VCDS's other global registries: the rows a unit selects for its adaptations,
//! basic settings, output tests, freeze frames and `[XPL]` bit lists, and the
//! multiplexer cases some of those rows defer to.
//!
//! ```text
//! the unit's .rod ──▶ [ADP] [GES] [SOT] [FFMUX] [XPL]   (or the IV_… store its [INC] names
//!                         │                              by the letter A, G, S, F, X)
//!                         ▼  "<n>,<code>": row n counted from 1, as [MWB] into RM.rod
//!          RA.rod [ADP]  RG.rod [GES]  RS.rod [SOT]  RF.rod [FFMUX]  RX.rod [XPL]
//!                         │  "<key>,<payload>", the payload in the key's alphabet
//!                         ▼
//!          a value description in RM.rod's shape: text table, type, scaling, unit,
//!          byte, bit, length, name — after one or two fields of each registry's own
//! a row of type 6 ──▶ MUX.rod [MUX], key = the row's table field ──▶ cases by raw range
//! ```
//!
//! `research/vcds-registry/README.md` §9 is the evidence, column by column, joined
//! against a VW ODIS project on the reference car's fourteen units. A column whose
//! meaning was not established is not decoded: it stays in [`Entry::raw`].
//!
//! **What these rows are for, and what they are not.** The tool reads and never
//! writes (CLAUDE.md, Safety). An adaptation's value is read with `0x22`, and a
//! freeze frame with `0x19`; those two registries describe data a later reader may
//! ask for. A basic setting is a routine (`0x31`) and an output test an
//! input/output control (`0x2F`): both services are outside the allowlist and
//! stay outside it, so their rows are kept as names and nothing more — nothing in
//! this tool may ever start one. Nothing here sends a request of any kind.
//!
//! The framing and the cipher are [`crate::registry`]'s: the same
//! [`DtcRegistry`] rows, the same per-key [`TableAlphabet`], the same unit-list
//! chain ([`crate::registry::list_in`]). Every file is read from one install.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::dtc::{DtcRegistry, DtcRow, find_named};
use crate::glyphs::TableAlphabet;
use crate::registry::{SectionRead, read_section};
use crate::rod::IvCache;

/// A registry a unit selects rows of, by the section both carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Section {
	/// `RA.rod [ADP]`: adaptation channels, read with `0x22`.
	Adaptation,
	/// `RG.rod [GES]`: basic settings — routines, `0x31`, never started here.
	BasicSetting,
	/// `RS.rod [SOT]`: output tests — input/output control, `0x2F`, never sent here.
	OutputTest,
	/// `RF.rod [FFMUX]`: the fields of a fault's environment record, read with `0x19`.
	FreezeFrame,
	/// `RX.rod [XPL]`: named bits of a byte string with no identifier of its own.
	/// The names are a unit's coding options; which bytes they describe is not
	/// established (§9.5).
	Xpl,
}

impl Section {
	pub const ALL: [Section; 5] = [
		Section::Adaptation,
		Section::BasicSetting,
		Section::OutputTest,
		Section::FreezeFrame,
		Section::Xpl,
	];

	/// The section tag, in the unit's file and in the registry alike.
	pub fn tag(self) -> &'static str {
		match self {
			Section::Adaptation => "ADP",
			Section::BasicSetting => "GES",
			Section::OutputTest => "SOT",
			Section::FreezeFrame => "FFMUX",
			Section::Xpl => "XPL",
		}
	}

	/// The registry's file name inside `UDS_EV`.
	pub fn file(self) -> &'static str {
		match self {
			Section::Adaptation => "RA.rod",
			Section::BasicSetting => "RG.rod",
			Section::OutputTest => "RS.rod",
			Section::FreezeFrame => "RF.rod",
			Section::Xpl => "RX.rod",
		}
	}

	fn shape(self) -> Shape {
		match self {
			Section::Adaptation => Shape::Identified { fields: 15 },
			Section::BasicSetting | Section::OutputTest => Shape::Identified { fields: 14 },
			Section::Xpl => Shape::Identified { fields: 13 },
			Section::FreezeFrame => Shape::Record,
		}
	}

	/// Whether a row's key is the `TTTEXT` record of what the row describes, as in
	/// `RM.rod`. In `RF.rod` it is not: its keys are 3 to 147 (and two sentinels),
	/// numbers no name record has.
	pub fn key_is_a_record(self) -> bool {
		self != Section::FreezeFrame
	}
}

/// The section `MUX.rod` holds its cases in, and the file.
pub const MUX_SECTION: &str = "MUX";
pub const MUX_FILE: &str = "MUX.rod";

/// The value type `f2 & 63` (`RM.rod`'s numbering) that defers to `MUX.rod`: its
/// table field is a MUX key. 23,547 of 23,547 such rows in `RM.rod` name one.
pub const MULTIPLEXED: u8 = 6;
/// The value type whose table field is a constant (`0x040000`) rather than a table.
pub const CONSTANT: u8 = 10;

const TYPE_MASK: u64 = 63;
const LITTLE_ENDIAN: u64 = 64;
const SIGNED: u64 = 128;
const TEXT_TABLE: u8 = 3;

/// Where a registry's fields sit.
#[derive(Debug, Clone, Copy)]
enum Shape {
	/// `<ordinal>,<identifier>,` then the value description: RA, RG, RS, RX.
	Identified { fields: usize },
	/// `<100 + record length>,` then the value description: RF, 11 fields.
	Record,
	/// MUX: four fields not decoded, the selector's width, the case's range, then
	/// the value description; 17 fields.
	Mux,
	/// RM: `<DID>,` then the value description; 12 or 13 fields.
	Measurement,
}

impl Shape {
	/// The index the value description starts at.
	fn base(self) -> usize {
		match self {
			Shape::Identified { .. } => 2,
			Shape::Record | Shape::Measurement => 1,
			Shape::Mux => 7,
		}
	}

	fn fits(self, fields: usize) -> bool {
		match self {
			Shape::Identified { fields: n } => fields == n,
			Shape::Record => fields == 11,
			Shape::Mux => fields == 17,
			Shape::Measurement => (12..=13).contains(&fields),
		}
	}
}

/// One registry's rows, in file order: a unit's list points into the order.
#[derive(Debug, Clone, Default)]
pub struct Registry {
	rows: DtcRegistry,
}

impl Registry {
	/// Parse a decoded section of the registry's file.
	pub fn parse(text: &str) -> Self {
		Self {
			rows: DtcRegistry::parse(text),
		}
	}

	/// A row by its 1-based number, as a unit's list counts.
	pub fn row(&self, index: usize) -> Option<DtcRow<'_>> {
		self.rows.row(index)
	}

	pub fn len(&self) -> usize {
		self.rows.len()
	}

	pub fn is_empty(&self) -> bool {
		self.rows.is_empty()
	}
}

/// A MUX case's raw range, both ends included, or the default case (`D,D`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
	Default,
	Range { lower: i64, upper: i64 },
}

/// What one registry row says about a value, in `RM.rod`'s field order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Value {
	/// `flags & 63`: 0 linear, 2 identity, 3 text table, 6 multiplexed, 7 raw
	/// bytes, 8 ASCII, 10 constant; the others are not decoded.
	pub kind: Option<u8>,
	/// Flag bit 7.
	pub signed: bool,
	/// Flag bit 6 clear.
	pub big_endian: bool,
	/// `byte * 8 + bit`, when both are there.
	pub bit_offset: Option<u32>,
	pub bit_length: Option<u32>,
	/// `(factor, offset)` for types 0 and 2: `(raw · num + off) / den`.
	pub scale: Option<(f64, f64)>,
	/// The table field as a number, for type 3 (a `TTDOP` table) and type 6 (a
	/// MUX key). For other types it is not decoded and stays in [`Entry::raw`].
	pub table: Option<u32>,
	/// The table field as written, for type 10: the constant (`0x040000`, `0`).
	pub constant: Option<String>,
	/// `UNIT.ROD`'s id.
	pub unit_id: Option<u32>,
	/// The `TTTEXT` record of the name VCDS shows.
	pub name_id: Option<u32>,
}

/// One row of one registry, read.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
	/// The 1-based row number in its registry.
	pub row: usize,
	pub key: u32,
	/// RA: the DID `0x22` reads; RG: the routine id; RS: the input/output DID. RX
	/// leaves it empty in every row; RF and MUX have no such field.
	pub identifier: Option<u32>,
	/// RA, RG, RS, RX: the field's 1-based place among the fields of one
	/// identifier, empty where the identifier has one field.
	pub ordinal: Option<u32>,
	/// RF: the first field, as written. In 917 of 1,001 of `RF.rod`'s records it is 100
	/// plus the record's length in bytes (`research/vcds-registry/README.md` §9.4), which
	/// is not enough to read it as that: it is kept, not decoded.
	pub lead: Option<u32>,
	/// MUX: which raw values of the selector this case is for.
	pub case: Option<Case>,
	pub value: Value,
	/// Every field, deciphered, comma-joined: the columns not decoded live here.
	pub raw: String,
}

/// Why a row was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowError {
	/// Not the registry's field count. RX opens each list with a one-field
	/// header row (key `999999`) and RF ends with one; both land here.
	Shape { fields: usize },
	/// A field holds a glyph the key's alphabet lacks.
	Glyph { field: usize },
}

/// Read row `index` of `section`'s registry.
pub fn read_entry(section: Section, index: usize, row: &DtcRow<'_>) -> Result<Entry, RowError> {
	read(section.shape(), index, row)
}

/// Read row `index` of `MUX.rod`.
pub fn read_mux(index: usize, row: &DtcRow<'_>) -> Result<Entry, RowError> {
	read(Shape::Mux, index, row)
}

/// The MUX key an `RM.rod` row defers to: its table field, when its type is
/// [`MULTIPLEXED`]. `None` for every other row and for one that does not read.
pub fn mux_of_measurement(row: &DtcRow<'_>) -> Option<u32> {
	let entry = read(Shape::Measurement, 0, row).ok()?;
	(entry.value.kind == Some(MULTIPLEXED)).then_some(entry.value.table).flatten()
}

fn read(shape: Shape, index: usize, row: &DtcRow<'_>) -> Result<Entry, RowError> {
	let alphabet = TableAlphabet::for_key(row.key);
	let glyphs: Vec<&str> = row.payload.split(alphabet.separator()).collect();
	if !shape.fits(glyphs.len()) {
		return Err(RowError::Shape { fields: glyphs.len() });
	}
	let mut fields = Vec::with_capacity(glyphs.len());
	for (field, text) in glyphs.iter().enumerate() {
		fields.push(match *text {
			"" => String::new(),
			text => alphabet.decode(text).ok_or(RowError::Glyph { field })?,
		});
	}
	let text = |i: usize| fields.get(i).map(String::as_str).filter(|t| !t.is_empty());
	let whole = |i: usize| text(i).and_then(|t| t.parse::<u64>().ok());
	let small = |i: usize| whole(i).and_then(|n| u32::try_from(n).ok());
	let real = |i: usize| text(i).and_then(|t| t.parse::<f64>().ok());

	let b = shape.base();
	let flags = whole(b + 1);
	let kind = flags.map(|f| (f & TYPE_MASK) as u8);
	let scale = match kind {
		Some(0 | 2) => match (real(b + 2), real(b + 3), real(b + 4)) {
			(offset, Some(numerator), Some(denominator)) if denominator != 0.0 => Some((numerator / denominator, offset.unwrap_or(0.0) / denominator)),
			(None, None, None) if kind == Some(2) => Some((1.0, 0.0)),
			_ => None,
		},
		_ => None,
	};
	let bit_offset = match (small(b + 6), small(b + 7)) {
		(Some(byte), Some(bit)) => byte.checked_mul(8).and_then(|o| o.checked_add(bit)),
		_ => None,
	};
	let value = Value {
		kind,
		signed: flags.is_some_and(|f| f & SIGNED != 0),
		big_endian: flags.is_some_and(|f| f & LITTLE_ENDIAN == 0),
		bit_offset,
		bit_length: small(b + 8),
		scale,
		table: matches!(kind, Some(TEXT_TABLE | MULTIPLEXED)).then(|| small(b)).flatten(),
		constant: (kind == Some(CONSTANT)).then(|| text(b).map(str::to_string)).flatten(),
		unit_id: small(b + 5),
		name_id: small(b + 9),
	};
	let (identifier, ordinal, lead, case) = match shape {
		Shape::Identified { .. } => (small(1), small(0), None, None),
		Shape::Record => (None, None, small(0), None),
		Shape::Mux => {
			let bound = |i: usize| text(i).and_then(|t| t.parse::<i64>().ok());
			let case = match (text(5), text(6)) {
				(Some("D"), Some("D")) => Some(Case::Default),
				_ => bound(5).zip(bound(6)).map(|(lower, upper)| Case::Range { lower, upper }),
			};
			(None, None, None, case)
		}
		Shape::Measurement => (small(0), None, None, None),
	};
	Ok(Entry {
		row: index,
		key: row.key,
		identifier,
		ordinal,
		lead,
		case,
		value,
		raw: fields.join(","),
	})
}

/// `MUX.rod`'s cases, by MUX key, in file order.
#[derive(Debug, Clone, Default)]
pub struct Mux {
	rows: DtcRegistry,
	by_key: BTreeMap<u32, Vec<usize>>,
}

impl Mux {
	pub fn parse(text: &str) -> Self {
		let rows = DtcRegistry::parse(text);
		let mut by_key: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
		for index in 1..=rows.len() {
			if let Some(row) = rows.row(index) {
				by_key.entry(row.key).or_default().push(index);
			}
		}
		Self { rows, by_key }
	}

	/// The cases of one MUX key, and how many of its rows did not read.
	pub fn cases(&self, key: u32) -> (Vec<Entry>, usize) {
		let mut unread = 0;
		let mut out = Vec::new();
		for &index in self.by_key.get(&key).map(Vec::as_slice).unwrap_or_default() {
			match self.rows.row(index).map(|row| read_mux(index, &row)) {
				Some(Ok(entry)) => out.push(entry),
				_ => unread += 1,
			}
		}
		(out, unread)
	}

	pub fn len(&self) -> usize {
		self.rows.len()
	}

	pub fn is_empty(&self) -> bool {
		self.rows.is_empty()
	}
}

/// What opening one of these files came to.
#[derive(Debug, Clone)]
pub enum Load<T> {
	Found {
		file: PathBuf,
		registry: T,
	},
	NoFile,
	/// A classic section whose key is not cached.
	Locked {
		file: PathBuf,
	},
	/// A shifted section: only VCDS's runtime key opens it.
	Shifted {
		file: PathBuf,
	},
}

/// Open `section`'s registry in the install at `root`.
pub fn load(root: &Path, section: Section, cache: &mut IvCache, crack: bool) -> Load<Registry> {
	open(root, section.file(), section.tag(), cache, crack, Registry::parse)
}

/// Open `MUX.rod`.
pub fn load_mux(root: &Path, cache: &mut IvCache, crack: bool) -> Load<Mux> {
	open(root, MUX_FILE, MUX_SECTION, cache, crack, Mux::parse)
}

fn open<T>(root: &Path, name: &str, tag: &str, cache: &mut IvCache, crack: bool, parse: impl Fn(&str) -> T) -> Load<T> {
	let Some(file) = find_named(root, name) else {
		return Load::NoFile;
	};
	match read_section(&file, tag, cache, crack) {
		SectionRead::Text { text, .. } => Load::Found {
			registry: parse(&text),
			file,
		},
		SectionRead::Shifted => Load::Shifted { file },
		SectionRead::Locked | SectionRead::Absent => Load::Locked { file },
	}
}

/// An entry with the words it is shown with, as the cache keeps it.
#[derive(Debug, Clone, PartialEq)]
pub struct Described {
	pub section: &'static str,
	pub entry: Entry,
	/// `f`'s name record, else the key's when the key is a name record.
	pub name: Option<String>,
	/// The ODX id (`IDE#####`, `MAS#####`) of the name record, else of the key.
	pub text_id: Option<String>,
	pub unit: Option<String>,
	/// A text table's states that have words.
	pub levels: Vec<crate::Level>,
}

/// Put words to an entry: its name, ODX id, unit and text-table states.
///
/// `key_is_a_record` says whether the row's key may stand in for a missing name
/// ([`Section::key_is_a_record`]; never for MUX).
pub fn describe(
	section: &'static str,
	entry: Entry,
	key_is_a_record: bool,
	words: &crate::registry::Words,
	units: &crate::unit_strings::UnitStrings,
	tables: &crate::ttdop::TextTables,
) -> Described {
	let key = key_is_a_record.then_some(entry.key);
	let name = entry
		.value
		.name_id
		.and_then(|id| words.name(id))
		.or_else(|| key.and_then(|k| words.name(k)))
		.map(str::to_string);
	let text_id = entry
		.value
		.name_id
		.and_then(|id| words.odx_id(id))
		.or_else(|| key.and_then(|k| words.odx_id(k)))
		.map(str::to_string);
	let unit = entry
		.value
		.unit_id
		.and_then(|id| units.get(id))
		.filter(|u| !u.is_empty())
		.map(str::to_string);
	let levels = match (entry.value.kind, entry.value.table) {
		(Some(TEXT_TABLE), Some(table)) => tables
			.table(table)
			.unwrap_or_default()
			.iter()
			.filter_map(|state| {
				let (lower, upper) = (i32::try_from(state.lower).ok()?, i32::try_from(state.upper).ok()?);
				Some(crate::Level::range(lower, upper, words.name(state.text_id)?))
			})
			.collect(),
		_ => Vec::new(),
	};
	Described {
		section,
		entry,
		name,
		text_id,
		unit,
		levels,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::rod::testkit::{named, section};

	// Every row below is invented — keys, identifiers, tables, records. The
	// registries are Ross-Tech's and stay out of the repository.

	fn line(key: u32, fields: &str) -> String {
		format!("{key:06},{}", TableAlphabet::for_key(key).encipher(fields))
	}

	fn entry(section: Section, key: u32, fields: &str) -> Result<Entry, RowError> {
		let text = line(key, fields);
		let registry = Registry::parse(&text);
		read_entry(section, 1, &registry.row(1).unwrap())
	}

	#[test]
	fn an_adaptation_row_is_an_ordinal_an_identifier_and_a_value_in_rms_order() {
		let e = entry(Section::Adaptation, 930_101, "2,4213,2,0,-50,1,1,3,1,4,8,930102,9,40ABCD,0033").unwrap();
		assert_eq!((e.ordinal, e.identifier), (Some(2), Some(0x1075)));
		assert_eq!(e.value.kind, Some(0));
		assert_eq!(e.value.scale, Some((1.0, -50.0)));
		assert_eq!((e.value.bit_offset, e.value.bit_length), (Some(12), Some(8)), "byte 1, bit 4");
		assert_eq!((e.value.unit_id, e.value.name_id), (Some(3), Some(930_102)));
		assert!(e.value.big_endian && !e.value.signed);
		assert_eq!(e.value.table, None, "a linear row's table field is not decoded");
		assert_eq!(e.raw, "2,4213,2,0,-50,1,1,3,1,4,8,930102,9,40ABCD,0033", "the undecoded columns are kept");
		// One field of a one-field identifier has no ordinal.
		let e = entry(Section::Adaptation, 930_103, ",4214,543,3,,,,,0,0,1,,,40ABCE,0002").unwrap();
		assert_eq!((e.ordinal, e.value.kind, e.value.table), (None, Some(3), Some(543)));
		assert_eq!(e.value.scale, None);
	}

	#[test]
	fn the_flags_say_byte_order_and_sign_and_a_constant_is_kept_as_written() {
		let e = entry(Section::BasicSetting, 930_111, ",791,0x040000,10,,,,,0,0,24,930112,,20ABCD").unwrap();
		assert_eq!(e.identifier, Some(791));
		assert_eq!(e.value.kind, Some(CONSTANT));
		assert_eq!(e.value.constant.as_deref(), Some("0x040000"));
		assert_eq!(e.value.table, None);
		let e = entry(Section::OutputTest, 930_121, "3,1106,,194,,,,,2,0,16,930122,1,10ABCD").unwrap();
		assert!(e.value.signed && !e.value.big_endian, "194 = identity, little-endian, signed");
		assert_eq!(e.value.scale, Some((1.0, 0.0)));
		assert_eq!(e.ordinal, Some(3));
	}

	#[test]
	fn a_freeze_frame_row_keeps_its_first_field_as_written_and_has_no_identifier() {
		let e = entry(Section::FreezeFrame, 114, "114,2,0,0,0.1,1,9,0,0,8,930131").unwrap();
		assert_eq!((e.identifier, e.ordinal, e.lead), (None, None, Some(114)));
		assert_eq!(e.value.scale, Some((0.1, 0.0)));
		assert_eq!(e.value.name_id, Some(930_131));
		// A field deferring to the multiplexer names its key in the table field.
		let e = entry(Section::FreezeFrame, 115, "103,2049,6,,,,,1,0,16,930132").unwrap();
		assert_eq!((e.value.kind, e.value.table), (Some(MULTIPLEXED), Some(2049)));
	}

	#[test]
	fn a_row_of_another_shape_is_refused_not_read() {
		// The one-field header RX opens a list with, and RF's sentinel.
		assert_eq!(entry(Section::Xpl, 999_999, "2").unwrap_err(), RowError::Shape { fields: 1 });
		assert_eq!(entry(Section::Adaptation, 7, ",4213,2,0,0").unwrap_err(), RowError::Shape { fields: 5 });
		let e = entry(Section::Xpl, 930_141, "4,,334,3,,,,,0,3,1,,0").unwrap();
		assert_eq!((e.ordinal, e.identifier, e.value.bit_offset), (Some(4), None, Some(3)));
	}

	#[test]
	fn mux_cases_are_ranges_or_the_default_and_an_rm_row_of_type_six_names_its_key() {
		let text = [
			line(41, "0,0,0,0,16,D,D,,39,,,,,0,0,,930151"),
			line(41, "0,0,0,0,16,0,1023,4,0,0,0.005,1,9,0,0,16,930152"),
			line(41, "0,0,0,0,16,65535,65535,2049,3,,,,,0,0,16,930152"),
			line(42, "0,0,0,0,8,-128,126,1,128,0,0.5,1,3,0,0,8,"),
		]
		.join("\r\n");
		let mux = Mux::parse(&text);
		let (cases, unread) = mux.cases(41);
		assert_eq!(unread, 0);
		assert_eq!(
			cases.iter().map(|c| c.case).collect::<Vec<_>>(),
			[
				Some(Case::Default),
				Some(Case::Range { lower: 0, upper: 1023 }),
				Some(Case::Range { lower: 65535, upper: 65535 })
			]
		);
		assert_eq!(cases[1].value.scale, Some((0.005, 0.0)));
		assert_eq!(cases[2].value.table, Some(2049));
		assert_eq!(mux.cases(42).0[0].case, Some(Case::Range { lower: -128, upper: 126 }));
		assert!(mux.cases(43).0.is_empty());

		let rm = Registry::parse(&[line(5, "8200,41,6,,,,,0,0,16,,80ABCD,0010"), line(6, "8201,41,3,,,,,0,0,8,,80ABCD,0033")].join("\r\n"));
		assert_eq!(mux_of_measurement(&rm.row(1).unwrap()), Some(41));
		assert_eq!(mux_of_measurement(&rm.row(2).unwrap()), None, "a text table is not multiplexed");
	}

	#[test]
	fn a_described_entry_takes_its_name_record_then_its_key_and_never_a_freeze_frames_key() {
		let words = crate::registry::Words::from_table(&crate::tttext::Table {
			texts: [(930_161, "Invented channel", Some(("2", "90161"))), (7001, "Invented state", None)]
				.iter()
				.map(|(id, name, tail)| crate::tttext::Text {
					id: *id,
					name: name.to_string(),
					tail: tail.map(|(kind, value)| crate::tttext::Tail {
						kind: kind.to_string(),
						value: value.to_string(),
					}),
				})
				.collect(),
			..Default::default()
		});
		let tables = crate::ttdop::TextTables::parse(&format!("006051,{}", TableAlphabet::for_key(6051).encipher("1,1,7001,")));
		let units = crate::unit_strings::UnitStrings::default();
		let e = entry(Section::Adaptation, 930_161, ",4215,6051,3,,,,,0,0,8,,,40ABCD,0033").unwrap();
		let d = describe("ADP", e, true, &words, &units, &tables);
		assert_eq!(d.name.as_deref(), Some("Invented channel"), "the key, with no name record");
		assert_eq!(d.text_id.as_deref(), Some("IDE90161"));
		assert_eq!(d.levels, vec![crate::Level::range(1, 1, "Invented state")]);
		let e = entry(Section::FreezeFrame, 930_161, "101,,2,,,,,0,0,8,").unwrap();
		let d = describe("FFMUX", e, Section::FreezeFrame.key_is_a_record(), &words, &units, &tables);
		assert_eq!((d.name, d.text_id), (None, None), "an RF key is not a name record");
	}

	#[test]
	fn a_registry_opens_from_its_file_and_a_unit_list_from_its_store() {
		let root = std::env::temp_dir().join(format!("vagcan-registries-{}", std::process::id()));
		let _ = std::fs::remove_dir_all(&root);
		std::fs::create_dir_all(root.join("UDS_EV")).unwrap();
		let write = |name: &str, sections: &[Vec<u8>]| std::fs::write(root.join("UDS_EV").join(name), sections.concat()).unwrap();
		let rows = [line(11, ",4096,,2,,,,,0,0,8,,,40ABCD,0033"), line(12, ",4097,,2,,,,,0,0,8,,,40ABCE,0033")].join("\r\n");
		write("RA.rod", &[section("ADP", rows.as_bytes(), true, named("RA.rod"))]);
		write(
			"IV_EV_UnitR_A3.rod",
			&[section("ADP", b"000002,00\r\n", true, named("IV_EV_UnitR_A3.rod"))],
		);
		write(
			"EV_UnitR.rod",
			&[section(
				"INC",
				format!("{}\r\n", line(123_456, "IV_EV_UnitR_A3")).as_bytes(),
				true,
				named("EV_UnitR.rod"),
			)],
		);
		let mut cache = IvCache::default();
		let Load::Found { registry, .. } = load(&root, Section::Adaptation, &mut cache, false) else {
			panic!("RA.rod did not open")
		};
		let unit = crate::label_files::UnitRod {
			how: crate::label_files::OdxMatch::Exact,
			path: root.join("UDS_EV").join("EV_UnitR.rod"),
			via: None,
		};
		let found = crate::registry::list_in(&unit, &mut cache, false, "ADP", registry.len());
		let crate::registry::UnitMeasurements::Found { rows, store, .. } = found else {
			panic!("{found:?}")
		};
		assert_eq!(store.unwrap().file_name().unwrap(), "IV_EV_UnitR_A3.rod", "the store whose letter is A");
		let e = read_entry(Section::Adaptation, rows.rows[0], &registry.row(rows.rows[0]).unwrap()).unwrap();
		assert_eq!(e.identifier, Some(4097), "row 2 counted from 1");
		assert!(matches!(load_mux(&root, &mut cache, false), Load::NoFile));
		let _ = std::fs::remove_dir_all(&root);
	}
}
