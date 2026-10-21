//! A stored fault's freeze frame: the conditions a unit recorded with it, out of the
//! `19 06 <DTC> FF` answer `faults` already reads for its mileage and date.
//!
//! The answer is records back to back, each a number and its bytes, with no lengths
//! ([`vag_uds_client::dtc::split_records`]). VW's protocol layer fixes records `0x01` and
//! `0x02` for every unit (priority, occurrences; aging counter, mileage, time stamp —
//! [`vag_uds_client::dtc::STANDARD_DTC_INFORMATION`]); the rest are the unit's own.
//!
//! **Two sources, each complete on its own, never mixed** ([`Source`]; CLAUDE.md, "ODIS and
//! VCDS are independent sources"). Both are stored by `setup` in the cache's `registry_row`
//! ([`vag_data_db::FreezeField`]), each under its own source, and a layout is built from one
//! kind of row alone:
//!
//! * **ODIS** — the variant's extended-data multiplexer
//!   (`vag_data_labels::odis::Project::extended_records`): every case, the standard records
//!   among them, with the length the project proves and each field's layout and scaling.
//! * **VCDS** — `RF.rod [FFMUX]`: a row's key is the record number (`999` the default case),
//!   its lead 100 plus the record's length in 897 of `RF.rod`'s 1,028 records — used only where
//!   the fields end exactly there; a type-11 row is a reserved field VCDS does not show. VCDS
//!   holds no rows for records `0x01`/`0x02`: those are the UDS framing's.
//!
//! What establishes it, with numbers: `research/odis-dtc/freeze-frames.md`.
//!
//! **Nothing is guessed.** A record no row describes, or one whose length is not proven,
//! takes the rest of the answer and is shown as bytes; so is a field whose type has no
//! decoding here (multiplexed, raw bytes) or whose layout [`RawForm`] cannot say. An answer
//! that does not split on its description, or whose standard records fail the unit's own
//! stamp, is shown as bytes after record `0x02` ([`Frame`]).
//!

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

/// Which description a layout is read from: one, and only one, per layout — the run's one
/// source: an ODIS project's extended-data multiplexers, or a VCDS install's `RF.rod`.
pub use vag_cli_core::source::Source;

/// One unit's extended-data records, as one source describes them.
#[derive(Debug, Clone)]
pub struct Layout {
	source: Source,
	records: BTreeMap<u32, Record>,
}

#[derive(Debug, Clone, Default)]
struct Record {
	/// The last number a range case covers; `None` for a case of one number.
	upper: Option<u32>,
	/// The record's length in bytes, where its rows prove one.
	length: Option<usize>,
	fields: Vec<FreezeField>,
}

impl Layout {
	/// Group one source's rows for a unit by record.
	pub fn new(fields: Vec<FreezeField>, source: Source) -> Layout {
		let mut records: BTreeMap<u32, Record> = BTreeMap::new();
		for field in fields {
			let record = records.entry(field.record).or_default();
			record.upper = record.upper.or(field.record_upper);
			record.fields.push(field);
		}
		for record in records.values_mut() {
			record.length = proven_length(&record.fields, source);
		}
		Layout { source, records }
	}

	/// The source the layout is read from.
	pub fn source(&self) -> Source {
		self.source
	}

	/// Record `number`'s own description — its case, or a range case that covers it — but
	/// not the default case's.
	fn own(&self, number: u8) -> Option<&Record> {
		let number = u32::from(number);
		self.records.get(&number).or_else(|| {
			self
				.records
				.range(..=number)
				.rev()
				.find(|(lower, r)| **lower != DEFAULT_RECORD && r.upper.is_some_and(|upper| (**lower..=upper).contains(&number)))
				.map(|(_, r)| r)
		})
	}

	/// The description of record `number`: its own, else the unit's default record's.
	fn record(&self, number: u8) -> Option<&Record> {
		self.own(number).or_else(|| self.records.get(&DEFAULT_RECORD))
	}

	/// How long record `number` is: the unit's own description's where the source has one
	/// for that very number (an ODIS variant describes the standard records too, the camera
	/// variant's `0x02` as twelve bytes), else the protocol's for the two standard records,
	/// else the unit's rows', `None` where nothing proves one.
	pub fn length_of(&self, number: u8) -> Option<usize> {
		match self.own(number) {
			Some(record) if self.source == Source::Odis || standard_length(number).is_none() => record.length,
			_ => standard_length(number).or_else(|| self.record(number)?.length),
		}
	}
}

/// A record's length, when its rows prove one.
///
/// **VCDS:** their one lead, less 100, **equal** to where its last field ends — the 897 of
/// `RF.rod`'s 1,028 records whose two readings agree. Rows that disagree on the lead, a field
/// past it, fields ending short of it (33 records whose tail nothing describes) and a lead of
/// 200 or more prove nothing. **ODIS:** the lead `setup` wrote is the project's own proof —
/// its declared size, or where its last field ends (`Project::extended_records`).
fn proven_length(fields: &[FreezeField], source: Source) -> Option<usize> {
	let lead = fields.first()?.lead?;
	if fields.iter().any(|f| f.lead != Some(lead)) || lead <= 100 {
		return None;
	}
	if source == Source::Odis {
		return Some((lead - 100) as usize);
	}
	if lead >= 200 {
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
	// The check of the standard records applies where they came out the standard's shape;
	// a description that lays them out otherwise (an ODIS variant's twelve-byte `0x02`) is
	// read by its own fields instead.
	let standard_whole = [STANDARD_DTC_INFORMATION, STANDARD_FREEZE_FRAME].iter().all(|n| {
		records
			.iter()
			.any(|r| r.number == *n && r.extent == Extent::Whole && Some(r.data.len()) == standard_length(*n))
	});
	// A source that lays a standard record out otherwise than the standard (an ODIS variant's
	// twelve-byte `0x02`) has no stamp check to catch a misaligned split, and a default case of
	// fixed length reads on through any bytes. The records after it are then read only when
	// they end exactly where the answer does, every one whole.
	let redefined = [STANDARD_DTC_INFORMATION, STANDARD_FREEZE_FRAME]
		.iter()
		.any(|n| layout.own(*n).is_some() && layout.length_of(*n) != standard_length(*n));
	let standard = |n: u8| matches!(n, STANDARD_DTC_INFORMATION | STANDARD_FREEZE_FRAME);
	let tail_whole = records.iter().filter(|r| !standard(r.number)).all(|r| r.extent == Extent::Whole);
	let why = match (short, standard_whole && context.is_none(), now) {
		(true, _, _) => Some("the answer does not split on this variant's layout"),
		(_, true, None) => Some("the unit gave no stamp to check its standard records by"),
		(_, true, Some(_)) => Some("its standard records fail the unit's own stamp"),
		_ if redefined && !tail_whole => Some("the answer does not end where this variant's layout says"),
		_ => None,
	};
	if why.is_some() {
		// Split again on the standard records alone — as this layout gives their lengths
		// where it redefines them — and leave everything after them as bytes.
		let records = match redefined {
			true => split_records(answer, |n| if standard(n) { layout.length_of(n) } else { None }),
			false => standard_only(),
		};
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
		// A standard record the source describes itself, laid out otherwise than the
		// standard, is read by that description; any other goes unread without the check.
		let described_standard = standard && frame.misframed.is_none() && Some(record.data.len()) != standard_length(record.number);
		if standard && !described_standard {
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
		let described = match standard {
			true => layout.and_then(|l| l.own(record.number)),
			false => layout.and_then(|l| l.record(record.number)),
		};
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
		return "(width not known, not decoded)".to_string();
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

/// The fewest columns [`render`] wraps a value into, beside its key.
const NARROWEST_VALUE: usize = 20;

/// A freeze frame as `key  value unit` lines under `indent`, the keys padded to one column.
///
/// The one rendering, so it goes under a code in whichever layout lists codes. A line that
/// would run past `width` keeps its value in the value column and wraps it there: the
/// bytes or the reading first, a reason in parentheses after it on a line of its own —
/// nothing is cut. With under [`NARROWEST_VALUE`] columns left for the value, it goes on
/// the next line, under the key, further in.
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
		let lead = format!("{indent}{}{}  ", line.key, " ".repeat(pad));
		let one = format!("{lead}{}", line.value);
		let lead_len = lead.chars().count();
		match width {
			Some(w) if one.chars().count() > w && w.saturating_sub(lead_len) >= NARROWEST_VALUE => {
				let room = w - lead_len;
				// The reason is the value's last parenthesis: it wraps on its own.
				let (reading, reason) = match line.value.rfind(" (") {
					Some(at) if line.value.ends_with(')') => (&line.value[..at], &line.value[at + 1..]),
					_ => (line.value.as_str(), ""),
				};
				let mut pieces = vag_cli_core::table::wrap(reading, room);
				pieces.extend(vag_cli_core::table::wrap(reason, room));
				let under = " ".repeat(lead_len);
				for (at, piece) in pieces.iter().enumerate() {
					out.push_str(if at == 0 { &lead } else { &under });
					out.push_str(piece);
					out.push('\n');
				}
			}
			Some(w) if one.chars().count() > w => out.push_str(&format!("{indent}{}\n{indent}    {}\n", line.key, line.value)),
			_ => {
				out.push_str(&one);
				out.push('\n');
			}
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
			record_upper: None,
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
		Layout::new(
			vec![
				field(112, 104, LINEAR, (0, 16), Some((0.25, 0.0)), "/min", "Speed"),
				field(112, 104, LINEAR, (16, 8), Some((0.1, 0.0)), "V", "Voltage"),
				flag,
				field(112, 104, RESERVED, (25, 7), None, "", "Reserved"),
				field(DEFAULT_RECORD, 101, IDENTITY, (0, 8), Some((1.0, 0.0)), "", "Counter"),
			],
			Source::Vcds,
		)
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
		let layout = Layout::new(vec![field(113, 101, IDENTITY, (0, 8), Some((1.0, 0.0)), "", "One byte")], Source::Vcds);
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
		assert_eq!(proven_length(&fits, Source::Vcds), Some(3));
		let short = [field(5, 104, LINEAR, (0, 24), Some((1.0, 0.0)), "", "a")];
		assert_eq!(proven_length(&short, Source::Vcds), None, "a tail nothing describes");
		let big = [field(5, 203, LINEAR, (0, 24), Some((1.0, 0.0)), "", "a")];
		assert_eq!(proven_length(&big, Source::Vcds), None, "a lead of 200 or more");
		let over = [field(5, 103, LINEAR, (16, 16), Some((1.0, 0.0)), "", "a")];
		assert_eq!(proven_length(&over, Source::Vcds), None, "a field past the lead");
		// VCDS's one-row "dynamic" record: a reserved field of no length, lead 203.
		let mut dynamic = field(113, 203, RESERVED, (0, 0), None, "", "");
		dynamic.bit_length = None;
		assert_eq!(proven_length(&[dynamic], Source::Vcds), None);
		let split = [
			field(5, 103, LINEAR, (0, 8), Some((1.0, 0.0)), "", "a"),
			field(5, 104, LINEAR, (8, 8), Some((1.0, 0.0)), "", "b"),
		];
		assert_eq!(proven_length(&split, Source::Vcds), None, "two leads for one record");
	}

	/// An ODIS-only layout as `setup` stores a variant's multiplexer: record 0x01, a record
	/// 0x02 laid out otherwise than the standard (twelve bytes, a reserved byte first, as one
	/// camera variant has it), and an own record 0x70. Invented.
	fn odis_layout() -> Layout {
		let mut fields = vec![
			field(1, 102, IDENTITY, (0, 8), Some((1.0, 0.0)), "", "Sim priority"),
			field(1, 102, IDENTITY, (8, 8), Some((1.0, 0.0)), "", "Sim count"),
			field(2, 112, RESERVED, (0, 8), None, "", "Sim reserved"),
			field(2, 112, IDENTITY, (8, 8), Some((1.0, 0.0)), "", "Sim aging"),
			field(2, 112, IDENTITY, (16, 24), Some((1.0, 0.0)), "km", "Sim mileage"),
			field(112, 103, LINEAR, (0, 16), Some((0.25, 0.0)), "1/min", "Sim engine speed"),
			field(112, 103, LINEAR, (16, 8), Some((0.1, 0.0)), "V", "Sim supply voltage"),
		];
		for f in &mut fields {
			f.record_upper = None;
		}
		Layout::new(fields, Source::Odis)
	}

	#[test]
	fn an_odis_layout_decodes_a_unit_on_its_own_its_standard_records_included() {
		// The ODIS path alone: the record lengths are the project's, so record 0x02 is split
		// as twelve bytes, read by its own fields, and the record after it lines up.
		let layout = odis_layout();
		let mut data = vec![0x02, 0x05, 0x02, 0x00, 0xB4, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0, 0];
		data.extend_from_slice(&[0x70, 0x0B, 0xB8, 0x7B]);
		let answer = DtcExtendedData { record: 0x01, data };
		let frame = split(&answer, Some(&layout), Some(&now()));
		assert!(frame.context.is_none(), "not the standard's shape, so not the standard's reading");
		assert_eq!(
			keyed(&lines(&frame, Some(&layout), "")),
			vec![
				(
					"record 01",
					"02 05 (standard record, not checked against the unit's own stamp, not decoded)"
				),
				("Sim aging", "180"),
				("Sim mileage", "4096 km"),
				("Sim engine speed", "750 1/min"),
				("Sim supply voltage", "12.3 V"),
			]
		);
	}

	#[test]
	fn a_redefined_record_02_decodes_the_rest_only_where_it_ends_with_the_answer() {
		// The camera case: the layout's record 0x02 is twelve bytes, the unit's answer is the
		// standard ten. Split on the layout, 0x02 swallows the next record's number, a stray
		// byte reads as a default record of two bytes, and nothing comes up short — but the
		// split does not end on a whole record, so nothing after 0x02 is decoded.
		let mut fields = odis_layout().records.into_values().flat_map(|r| r.fields).collect::<Vec<_>>();
		fields.push(field(DEFAULT_RECORD, 102, IDENTITY, (0, 16), Some((1.0, 0.0)), "", "Sim value"));
		let layout = Layout::new(fields, Source::Odis);
		let answer = answer(&[0x70, 0x0B, 0xB8, 0x01, 0x02, 0x71, 0xCC]);
		let frame = split(&answer, Some(&layout), Some(&now()));
		let lines = lines(&frame, Some(&layout), "");
		assert!(lines.iter().all(|l| l.key != "Sim value"), "a stray byte decoded: {lines:?}");
		let tail = lines.last().unwrap();
		assert!(
			tail
				.value
				.ends_with("(the answer does not end where this variant's layout says, not decoded)"),
			"{lines:?}"
		);
		// The same layout over an answer that does end with a whole record is read.
		let exact = DtcExtendedData {
			record: 0x01,
			data: vec![0x02, 0x05, 0x02, 0, 0xB4, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0, 0, 0x75, 0x01, 0x02],
		};
		let frame = split(&exact, Some(&layout), Some(&now()));
		assert!(lines_of(&frame, &layout).contains(&("Sim value".to_string(), "258".to_string())));
	}

	fn lines_of(frame: &Frame<'_>, layout: &Layout) -> Vec<(String, String)> {
		lines(frame, Some(layout), "").into_iter().map(|l| (l.key, l.value)).collect()
	}

	#[test]
	fn an_odis_layout_of_the_standard_shape_reads_like_the_standard() {
		let mut fields = odis_layout().records.into_values().flat_map(|r| r.fields).collect::<Vec<_>>();
		fields.retain(|f| f.record != 2);
		fields.push(field(2, 110, IDENTITY, (0, 8), Some((1.0, 0.0)), "", "Sim aging"));
		let layout = Layout::new(fields, Source::Odis);
		let answer = answer(&[0x70, 0x0B, 0xB8, 0x7B]);
		let frame = split(&answer, Some(&layout), Some(&now()));
		assert!(frame.context.is_some());
		assert_eq!(
			keyed(&lines(&frame, Some(&layout), ""))[2..],
			[("Sim engine speed", "750 1/min"), ("Sim supply voltage", "12.3 V")]
		);
	}

	#[test]
	fn a_source_is_named_by_its_cache_kind_and_nothing_else() {
		for source in [Source::Odis, Source::Vcds] {
			assert_eq!(Source::of_kind(source.kind()), Some(source));
		}
		assert_eq!(Source::of_kind("other"), None);
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
		// Wrapped in the value column, the reason after the bytes on lines of its own.
		assert_eq!(
			render(&long, "    ", Some(40)),
			"    Voltage  0B B8 7B 01 71 AA 0B B8 7B\n\
			 \x20            01 71 AA\n\
			 \x20            (not described, not\n\
			 \x20            decoded)\n"
		);
		// The bytes fit beside the key: only the reason goes under them.
		let record = vec![Line::new("record 70", "AA BB (no layout for this variant, not decoded)")];
		assert_eq!(
			render(&record, &" ".repeat(13), Some(70)),
			"             record 70  AA BB\n\
			 \x20                       (no layout for this variant, not decoded)\n"
		);
		// With too little room for a value beside its key, the value goes under the key.
		assert_eq!(
			render(&long, "    ", Some(24)),
			"    Voltage\n        0B B8 7B 01 71 AA 0B B8 7B 01 71 AA (not described, not decoded)\n"
		);
	}
}
