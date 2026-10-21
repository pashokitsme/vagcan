//! A stored fault's freeze frame: the conditions a unit recorded with it, out of the
//! `19 06 <DTC> FF` answer `faults` already reads for its mileage and date.
//!
//! The answer is records back to back, each a number and its bytes, with no lengths
//! ([`vag_uds_client::dtc::split_records`]). VW's protocol layer fixes records `0x01` and
//! `0x02` for every unit (priority, occurrences; aging counter, mileage, time stamp —
//! [`vag_uds_client::dtc::STANDARD_DTC_INFORMATION`]); the rest are the unit's own, and a
//! VCDS install describes them per unit in `RF.rod [FFMUX]`, which `setup` stores in the
//! cache ([`vag_data_db::FreezeField`]). What establishes that, with numbers, is
//! `research/odis-dtc/freeze-frames.md`:
//!
//! * a row's key is the record number — the reference project's ODIS multiplexers list the
//!   same numbers for the same units (steering assist 113–125, both; engine 112–113, both),
//!   and `999` is the multiplexer's default case;
//! * a row's lead is 100 plus the record's length in 897 of `RF.rod`'s 1,028 records, and it
//!   is used as one only where its fields end exactly there;
//! * a type-11 row is a reserved field VCDS does not show.
//!
//! **Nothing is guessed.** A record no row describes, or one whose length is not proven,
//! takes the rest of the answer and is shown as bytes; so is a field whose type has no
//! decoding here (multiplexed, raw bytes) or whose layout [`RawForm`] cannot say. An answer
//! that does not split on its description, or whose standard records fail the unit's own
//! stamp, is shown as bytes after record `0x02` ([`Frame`]).
//!
//! **One source.** Every row read here is a VCDS install's; nothing of an ODIS project's
//! fills a field, a length or a record of it (CLAUDE.md, "ODIS and VCDS are independent
//! sources"). Records `0x01`/`0x02` are the UDS framing's, of neither source.
//!
//! The ODIS project describes the same records more fully, in its extended-data
//! multiplexers — whose cases are `DB_CASE` objects, on the reader's permanent refusal list
//! (`vag_data_labels::odis::loaders::REFUSED`). So the unit's own records are named from VCDS
//! alone.

use std::collections::BTreeMap;

use vag_data_db::FreezeField;
use vag_data_labels::catalog::level_for;
use vag_data_labels::measure::RawForm;
use vag_uds_client::dtc::{
	DYNAMIC_FREEZE_FRAME, DtcExtendedData, ExtendedRecord, Extent, FaultContext, STANDARD_DTC_INFORMATION, STANDARD_FREEZE_FRAME, UnitStamp,
	split_records, standard_length,
};

/// The key of a unit's default record: the layout of a record number it lists no rows for.
pub const DEFAULT_RECORD: u32 = 999;

/// `RM.rod`'s value types this module reads (`vag_data_labels::registries::Value::kind`).
const LINEAR: u8 = 0;
const IDENTITY: u8 = 2;
const TEXT_TABLE: u8 = 3;
/// A field VCDS keeps in its description and never shows: named `Reserved` in every row
/// of the reference car's units, and absent from VCDS's own printout of the same records.
const RESERVED: u8 = 11;

/// One unit's own extended-data records, as its VCDS rows describe them.
#[derive(Debug, Clone, Default)]
pub struct Layout {
	records: BTreeMap<u32, Record>,
}

#[derive(Debug, Clone, Default)]
struct Record {
	/// The record's length in bytes, where its rows prove one.
	length: Option<usize>,
	fields: Vec<FreezeField>,
}

impl Layout {
	/// Group a unit's rows by record.
	pub fn new(fields: Vec<FreezeField>) -> Layout {
		let mut records: BTreeMap<u32, Record> = BTreeMap::new();
		for field in fields {
			records.entry(field.record).or_default().fields.push(field);
		}
		for record in records.values_mut() {
			record.length = proven_length(&record.fields);
		}
		Layout { records }
	}

	/// The description of record `number`: its own rows, else the unit's default record's.
	fn record(&self, number: u8) -> Option<&Record> {
		self.records.get(&u32::from(number)).or_else(|| self.records.get(&DEFAULT_RECORD))
	}

	/// How long record `number` is: the protocol's for the two standard records, the unit's
	/// rows' for its own, `None` where neither proves one.
	pub fn length_of(&self, number: u8) -> Option<usize> {
		standard_length(number).or_else(|| self.record(number)?.length)
	}
}

/// A record's length, when its rows prove one: their one lead, less 100, **equal** to where
/// its last field ends — the 897 of `RF.rod`'s 1,028 records whose two readings agree. Rows
/// that disagree on the lead, a field past it, fields ending short of it (33 records whose
/// tail nothing describes) and a lead of 200 or more prove nothing.
fn proven_length(fields: &[FreezeField]) -> Option<usize> {
	let lead = fields.first()?.lead?;
	if fields.iter().any(|f| f.lead != Some(lead)) || !(101..200).contains(&lead) {
		return None;
	}
	let length = (lead - 100) as usize;
	let extent = fields
		.iter()
		.filter_map(|f| Some(f.bit_offset? + f.bit_length?))
		.max()
		.filter(|bits| *bits > 0)?;
	(extent.div_ceil(8) as usize == length).then_some(length)
}

/// One line of a freeze frame: what it is called, and what it read — the unit is in `value`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
	pub key: String,
	pub value: String,
}

impl Line {
	fn new(key: impl Into<String>, value: impl Into<String>) -> Line {
		Line {
			key: key.into(),
			value: value.into(),
		}
	}
}

/// An answer split into its records, with what the split is worth.
#[derive(Debug, Clone)]
pub struct Frame<'a> {
	records: Vec<ExtendedRecord<'a>>,
	/// The fault's context, where both standard records are whole and pass the unit's stamp
	/// ([`FaultContext::from_records`]).
	pub context: Option<FaultContext>,
	/// The answer did not split on the unit's description: a record came out short, or the
	/// standard records failed their check. Everything after record `0x02` is then bytes —
	/// a default record that matches any number would otherwise read the rest of a
	/// misaligned answer as its fields. `Some` holds why, in words.
	misframed: Option<&'static str>,
}

/// Split an answer into its records and read the standard two, checked against `now`, the
/// unit's own stamp.
///
/// With a layout, a split that ends short or standard records that fail their check mean
/// the layout and the answer disagree, and the answer is split again on the standard
/// records alone ([`Frame::misframed`]).
pub fn split<'a>(answer: &'a DtcExtendedData, layout: Option<&Layout>, now: Option<&UnitStamp>) -> Frame<'a> {
	let standard_only = || split_records(answer, standard_length);
	let Some(layout) = layout else {
		let records = standard_only();
		let context = FaultContext::from_records(&records, now);
		return Frame {
			records,
			context,
			misframed: None,
		};
	};
	let records = split_records(answer, |n| layout.length_of(n));
	let context = FaultContext::from_records(&records, now);
	let short = records.iter().any(|r| r.extent == Extent::Short);
	let standard_whole = [STANDARD_DTC_INFORMATION, STANDARD_FREEZE_FRAME]
		.iter()
		.all(|n| records.iter().any(|r| r.number == *n && r.extent == Extent::Whole));
	let why = match (short, standard_whole && context.is_none(), now) {
		(true, _, _) => Some("the answer does not split on this variant's layout"),
		(_, true, None) => Some("the unit gave no stamp to check its standard records by"),
		(_, true, Some(_)) => Some("its standard records fail the unit's own stamp"),
		_ => None,
	};
	if why.is_some() {
		let records = standard_only();
		let context = FaultContext::from_records(&records, now);
		return Frame {
			records,
			context,
			misframed: why,
		};
	}
	Frame {
		records,
		context,
		misframed: None,
	}
}

/// What a fault's freeze frame says, line by line: the two standard fields no other column
/// shows (priority, aging counter), then the unit's own records. `no_layout` is why a record
/// has no description when `layout` is `None` — "no layout for this variant".
///
/// Occurrences, mileage and the date are the code's own line, not repeated here.
pub fn lines(frame: &Frame<'_>, layout: Option<&Layout>, no_layout: &str) -> Vec<Line> {
	let mut out = Vec::new();
	let context = frame.context.as_ref();
	if let Some(context) = context {
		out.push(Line::new("priority", context.priority.to_string()));
		out.push(Line::new("aging counter", context.aging_counter.to_string()));
	}
	for record in &frame.records {
		let standard = matches!(record.number, STANDARD_DTC_INFORMATION | STANDARD_FREEZE_FRAME);
		if standard && context.is_some() {
			continue;
		}
		if standard {
			let why = "standard record, not checked against the unit's own stamp, not decoded";
			out.push(Line::new(format!("record {:02X}", record.number), raw(record.data, why)));
			continue;
		}
		if let Some(why) = frame.misframed {
			out.push(Line::new(
				format!("record {:02X}", record.number),
				raw(record.data, &format!("{why}, not decoded")),
			));
			continue;
		}
		let described = layout.and_then(|l| l.record(record.number));
		match (described, record.extent) {
			(Some(described), Extent::Whole) => {
				for field in described.fields.iter().filter(|f| f.kind != Some(RESERVED)) {
					out.push(Line::new(field_name(field), field_value(field, record.data)));
				}
			}
			(_, extent) => {
				let why = match extent {
					Extent::Short => "shorter than its description, not decoded".to_string(),
					_ if record.number == DYNAMIC_FREEZE_FRAME => "runs to the end of the answer, not decoded".to_string(),
					_ if described.is_some() => "length not proven, not decoded".to_string(),
					_ if layout.is_none() => format!("{no_layout}, not decoded"),
					_ => "not described, not decoded".to_string(),
				};
				out.push(Line::new(format!("record {:02X}", record.number), raw(record.data, &why)));
			}
		}
	}
	out
}

fn field_name(field: &FreezeField) -> String {
	field.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| "(no name)".to_string())
}

/// A field's value as a person reads it, or its bytes and why they are bytes.
fn field_value(field: &FreezeField, data: &[u8]) -> String {
	let (Some(bit_offset), Some(bit_length)) = (field.bit_offset, field.bit_length) else {
		return "(no layout)".to_string();
	};
	let first = (bit_offset / 8) as usize;
	let last = (bit_offset + bit_length).div_ceil(8) as usize;
	let Some(bytes) = data.get(first..last) else {
		return "(past the end of the record)".to_string();
	};
	let form = RawForm::for_field(bit_offset, bit_length, field.signed, field.big_endian);
	let read = form.and_then(|form| form.read(data));
	let unit = field.unit.as_deref().filter(|u| !u.is_empty());
	match (field.kind, read) {
		(Some(LINEAR | IDENTITY), Some(raw_value)) => match field.scale {
			Some((factor, offset)) => {
				let value = number(raw_value as f64 * factor + offset);
				match unit {
					Some(unit) => format!("{value} {unit}"),
					None => value,
				}
			}
			None => raw(bytes, "no scaling"),
		},
		(Some(TEXT_TABLE), Some(raw_value)) => match level_for(&field.levels, raw_value) {
			Some(at) => field.levels[at].name().to_string(),
			None => format!("{raw_value} (no state named)"),
		},
		(Some(LINEAR | IDENTITY | TEXT_TABLE), None) => raw(bytes, "layout not decoded"),
		(Some(6), _) => raw(bytes, "multiplexed, not decoded"),
		_ => raw(bytes, "raw"),
	}
}

/// Bytes as hex, and why they are shown so.
fn raw(bytes: &[u8], why: &str) -> String {
	if bytes.is_empty() {
		return format!("(no bytes; {why})");
	}
	format!("{} ({why})", crate::render::hex_spaced(bytes))
}

/// Three decimals at most, no trailing zeros: the finest scaling VCDS writes is ×0.001.
fn number(value: f64) -> String {
	let text = format!("{value:.3}");
	let text = match text.contains('.') {
		true => text.trim_end_matches('0').trim_end_matches('.').to_string(),
		false => text,
	};
	if text == "-0" { "0".to_string() } else { text }
}

/// A freeze frame as `key  value unit` lines under `indent`, the keys padded to one column.
///
/// The one rendering, so it goes under a code in whichever layout lists codes. A line that
/// would run past `width` puts its value on the next line, under the key, further in —
/// nothing is cut.
pub fn render(lines: &[Line], indent: &str, width: Option<usize>) -> String {
	const WIDEST_KEY: usize = 32;
	let column = lines
		.iter()
		.map(|l| l.key.chars().count())
		.filter(|n| *n <= WIDEST_KEY)
		.max()
		.unwrap_or(0);
	let mut out = String::new();
	for line in lines {
		let key_len = line.key.chars().count();
		let pad = column.saturating_sub(key_len);
		let one = format!("{indent}{}{}  {}", line.key, " ".repeat(pad), line.value);
		if width.is_none_or(|w| one.chars().count() <= w) {
			out.push_str(&one);
			out.push('\n');
		} else {
			out.push_str(&format!("{indent}{}\n{indent}    {}\n", line.key, line.value));
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A field the way the cache hands it over. Invented.
	fn field(record: u32, lead: u32, kind: u8, bits: (u32, u32), scale: Option<(f64, f64)>, unit: &str, name: &str) -> FreezeField {
		FreezeField {
			record,
			lead: Some(lead),
			kind: Some(kind),
			signed: false,
			big_endian: true,
			bit_offset: Some(bits.0),
			bit_length: Some(bits.1),
			scale,
			unit: (!unit.is_empty()).then(|| unit.to_string()),
			name: Some(name.to_string()),
			levels: Vec::new(),
		}
	}

	/// An invented unit: record 0x70 of four bytes — a speed, a voltage, a two-state flag,
	/// a reserved bit — and a default record of one byte.
	fn layout() -> Layout {
		let mut flag = field(112, 104, TEXT_TABLE, (24, 1), None, "", "Switch");
		flag.levels = vec![vag_data_labels::Level::point(0, "off"), vag_data_labels::Level::point(1, "on")];
		Layout::new(vec![
			field(112, 104, LINEAR, (0, 16), Some((0.25, 0.0)), "/min", "Speed"),
			field(112, 104, LINEAR, (16, 8), Some((0.1, 0.0)), "V", "Voltage"),
			flag,
			field(112, 104, RESERVED, (25, 7), None, "", "Reserved"),
			field(DEFAULT_RECORD, 101, IDENTITY, (0, 8), Some((1.0, 0.0)), "", "Counter"),
		])
	}

	/// `01 <priority> <count>`, `02 <aging> <km:3> <indication> <stamp:5>`, then what follows.
	/// 4096 km, 2026-07-27 00:00:03. Invented.
	fn answer(tail: &[u8]) -> DtcExtendedData {
		let mut data = vec![0x02, 0x05, 0x02, 0xB4, 0x00, 0x10, 0x00, 0x00, 0x00, 0x69, 0xF6, 0x00, 0x03];
		data.extend_from_slice(tail);
		DtcExtendedData { record: 0x01, data }
	}

	/// The unit's own stamp, 100 km on: what the standard records are checked against.
	fn now() -> UnitStamp {
		UnitStamp::parse(&[0x91, 0x00, 0x10, 0x64, 0x00, 0x00, 0x69, 0xF6, 0x10, 0x03]).unwrap()
	}

	fn keyed(lines: &[Line]) -> Vec<(&str, &str)> {
		lines.iter().map(|l| (l.key.as_str(), l.value.as_str())).collect()
	}

	#[test]
	fn a_described_record_is_read_field_by_field_and_the_reserved_one_is_left_out() {
		let layout = layout();
		let answer = answer(&[0x70, 0x0B, 0xB8, 0x7B, 0x01]);
		let frame = split(&answer, Some(&layout), Some(&now()));
		let context = frame.context.expect("both standard records are whole and pass the stamp");
		assert_eq!(
			(context.priority, context.occurrences, context.aging_counter, context.mileage_km),
			(2, 5, 180, 4096)
		);
		assert_eq!(
			keyed(&lines(&frame, Some(&layout), "")),
			vec![
				("priority", "2"),
				("aging counter", "180"),
				("Speed", "750 /min"),
				("Voltage", "12.3 V"),
				("Switch", "on")
			]
		);
	}

	#[test]
	fn a_record_with_no_rows_of_its_own_is_read_by_the_default_one() {
		let layout = layout();
		let answer = answer(&[0x75, 0x2A]);
		let frame = split(&answer, Some(&layout), Some(&now()));
		assert_eq!(keyed(&lines(&frame, Some(&layout), ""))[2..], [("Counter", "42")]);
	}

	#[test]
	fn a_record_nobody_describes_takes_the_rest_as_bytes_and_says_so() {
		// Without a layout the unit's record cannot be split from what follows it: one line
		// of bytes, not two guessed records — and the reason the caller gives.
		let answer = answer(&[0x70, 0x0B, 0xB8, 0x7B, 0x01, 0x71, 0xAA]);
		let frame = split(&answer, None, Some(&now()));
		let lines = lines(&frame, None, "no layout for this variant");
		assert_eq!(
			keyed(&lines)[2..],
			[("record 70", "0B B8 7B 01 71 AA (no layout for this variant, not decoded)")]
		);
	}

	#[test]
	fn a_split_that_ends_short_is_misframed_and_everything_after_record_02_is_bytes() {
		// The default record answers to any number, so a misaligned answer would otherwise
		// read on as default-case fields. A record cut short says the alignment is wrong.
		let layout = layout();
		let answer = answer(&[0x75, 0x2A, 0x70, 0x0B, 0xB8]);
		let frame = split(&answer, Some(&layout), Some(&now()));
		assert!(frame.context.is_some(), "the standard records still read");
		assert_eq!(
			keyed(&lines(&frame, Some(&layout), ""))[2..],
			[(
				"record 75",
				"2A 70 0B B8 (the answer does not split on this variant's layout, not decoded)"
			)]
		);
	}

	#[test]
	fn standard_records_that_fail_the_units_stamp_leave_the_whole_frame_as_bytes() {
		let layout = layout();
		let answer = answer(&[0x70, 0x0B, 0xB8, 0x7B, 0x01]);
		// A stamp behind the fault's mileage: the reading is impossible.
		let behind = UnitStamp::parse(&[0x91, 0x00, 0x0F, 0x00, 0x00, 0x00, 0x69, 0xF6, 0x10, 0x03]).unwrap();
		let frame = split(&answer, Some(&layout), Some(&behind));
		assert!(frame.context.is_none());
		let failed = lines(&frame, Some(&layout), "");
		assert_eq!(failed.len(), 3, "{failed:?}");
		assert!(failed[2].value.ends_with("(its standard records fail the unit's own stamp, not decoded)"));
		// No stamp at all: nothing to check by, said so.
		let frame = split(&answer, Some(&layout), None);
		let unchecked = lines(&frame, Some(&layout), "");
		assert!(unchecked[0].value.contains("standard record, not checked"), "{unchecked:?}");
		assert!(
			unchecked[2]
				.value
				.ends_with("(the unit gave no stamp to check its standard records by, not decoded)")
		);
	}

	#[test]
	fn the_dynamic_record_is_never_split_by_a_layout() {
		let layout = Layout::new(vec![field(113, 101, IDENTITY, (0, 8), Some((1.0, 0.0)), "", "One byte")]);
		let answer = answer(&[0x71, 0x01, 0x02, 0x03]);
		let frame = split(&answer, Some(&layout), Some(&now()));
		assert_eq!(
			keyed(&lines(&frame, Some(&layout), ""))[2..],
			[("record 71", "01 02 03 (runs to the end of the answer, not decoded)")]
		);
	}

	#[test]
	fn a_lead_is_a_length_only_where_the_fields_end_exactly_there() {
		// The rule of research/odis-dtc/freeze-frames.md: 897 of RF.rod's 1,028 records.
		let fits = [field(5, 103, LINEAR, (0, 24), Some((1.0, 0.0)), "", "a")];
		assert_eq!(proven_length(&fits), Some(3));
		let short = [field(5, 104, LINEAR, (0, 24), Some((1.0, 0.0)), "", "a")];
		assert_eq!(proven_length(&short), None, "a tail nothing describes");
		let big = [field(5, 203, LINEAR, (0, 24), Some((1.0, 0.0)), "", "a")];
		assert_eq!(proven_length(&big), None, "a lead of 200 or more");
		let over = [field(5, 103, LINEAR, (16, 16), Some((1.0, 0.0)), "", "a")];
		assert_eq!(proven_length(&over), None, "a field past the lead");
		// VCDS's one-row "dynamic" record: a reserved field of no length, lead 203.
		let mut dynamic = field(113, 203, RESERVED, (0, 0), None, "", "");
		dynamic.bit_length = None;
		assert_eq!(proven_length(&[dynamic]), None);
		let split = [
			field(5, 103, LINEAR, (0, 8), Some((1.0, 0.0)), "", "a"),
			field(5, 104, LINEAR, (8, 8), Some((1.0, 0.0)), "", "b"),
		];
		assert_eq!(proven_length(&split), None, "two leads for one record");
	}

	#[test]
	fn a_type_with_no_decoding_here_is_its_bytes() {
		let mux = field(112, 102, 6, (0, 16), None, "", "Mode");
		assert_eq!(field_value(&mux, &[0x12, 0x34]), "12 34 (multiplexed, not decoded)");
		let mut table = field(112, 101, TEXT_TABLE, (0, 8), None, "", "State");
		table.levels = vec![vag_data_labels::Level::point(0, "idle")];
		assert_eq!(field_value(&table, &[0x07]), "7 (no state named)");
	}

	#[test]
	fn the_block_lines_up_its_values_and_wraps_rather_than_cuts() {
		let lines = vec![Line::new("priority", "2"), Line::new("Voltage", "12.3 V")];
		assert_eq!(render(&lines, "    ", Some(80)), "    priority  2\n    Voltage   12.3 V\n");
		let long = vec![Line::new("Voltage", "0B B8 7B 01 71 AA 0B B8 7B 01 71 AA (not described, not decoded)")];
		assert_eq!(
			render(&long, "    ", Some(40)),
			"    Voltage\n        0B B8 7B 01 71 AA 0B B8 7B 01 71 AA (not described, not decoded)\n"
		);
	}
}
