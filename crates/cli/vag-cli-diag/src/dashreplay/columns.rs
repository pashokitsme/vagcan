//! Which column of a recording is which channel of the plan — by identity, never by place.
//!
//! `watch --out` heads a column with the channel's name as `watch` shows it, not with its
//! address. So a heading is turned back into identities the way the writer turned
//! identities into headings: every channel `watch` offers for this car is named, and the
//! ones named like the heading are what the column can be. A plan channel takes a column
//! only when that is exactly one identity and it is the plan channel's — unit, identifier
//! and, for a converted value, where in the answer the field starts. A heading two channels
//! of this car share is refused rather than guessed.
//!
//! A column is one of two kinds, and the writer marks which:
//!
//! - **converted** — the number `watch` computed through the catalog row the plan was built
//!   from. It is turned back into the raw integer and scaled the way the board scales it, in
//!   `f32`; a number that is not a whole raw value on the plan's scaling means the recording
//!   was made with another scaling, and the column is not used.
//! - **`_raw`** — the unit's answer as hex, the identifier echo stripped: exactly what the
//!   board's own decoder reads, so it is decoded with the plan's layout.

use std::collections::BTreeSet;

use vag_cli_core::dash::{Channel as PlanChannel, Plan};
use vag_dash_render::plan::{Channel as DeviceChannel, Plan as DevicePlan};

use super::engine::{Series, address_name, channel_name};
use crate::plan::{Answered, Channel as Offered, hex_bytes};
use crate::watch::replay::{Column, Recording, unconverted};

/// Where a plan channel's values are in the recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
	/// A column of converted values, by its place in [`Recording::columns`].
	Converted(usize),
	/// A column of raw answers.
	Raw(usize),
}

/// One plan channel's column, if the recording has one it can be sure of — and what the
/// person should be told about the ones it has not.
#[derive(Debug, Clone, PartialEq)]
pub struct Matched {
	/// One per plan channel, in plan order.
	pub sources: Vec<Option<Source>>,
	pub notes: Vec<String>,
}

/// What a column can be: unit, identifier and — for a converted value — the field's bit
/// offset; a raw column is the whole answer, so `None`.
type Identity = (u16, u16, Option<u32>);

/// Match the recording's columns to the plan's channels. `offered` is every channel `watch`
/// offers for this car, which is what its headings were written from; `answered` is what the
/// car's survey saw it answer — a channel the car was seen not to answer wrote no values, so
/// it is not what a column holds.
pub fn match_columns(columns: &[Column], offered: &[Offered], answered: &Answered, plan: &Plan) -> Matched {
	let meanings: Vec<BTreeSet<Identity>> = columns
		.iter()
		.map(|column| {
			// The writer heads a channel with a definition by its name and writes the value,
			// and one without by its name suffixed `_raw` and writes the bytes.
			let mut can_be: BTreeSet<Identity> = offered
				.iter()
				.filter(|c| c.def.is_some() != column.raw && c.label() == column.name)
				.filter(|c| answered.saw(c.request, c.did) != Some(false))
				.map(|c| match column.raw {
					true => (c.request, c.did, None),
					false => {
						let (request, did, bit_offset) = c.key();
						(request, did, Some(bit_offset))
					}
				})
				.collect();
			// A channel nothing names is headed by its address, which says which it is.
			if column.raw
				&& let Some((request, did)) = address(&column.name)
			{
				can_be.insert((request, did, None));
			}
			can_be
		})
		.collect();

	let mut sources = Vec::with_capacity(plan.channels.len());
	let mut notes = Vec::new();
	let mut missing = Vec::new();
	let mut by_name = Vec::new();
	for channel in &plan.channels {
		let exact = (channel.unit, channel.did, Some(channel.bit_offset));
		let whole = (channel.unit, channel.did, None);
		let hits: Vec<usize> = (0..columns.len())
			.filter(|&i| meanings[i].contains(&exact) || meanings[i].contains(&whole))
			.collect();
		let sure = hits.iter().copied().find(|&i| meanings[i].len() == 1);
		let name = name_of(channel);
		sources.push(sure.map(|i| match columns[i].raw {
			true => Source::Raw(i),
			false => Source::Converted(i),
		}));
		match (sure, hits.first()) {
			// An address heading says its unit; a name says nothing of one.
			(Some(i), _) if columns[i].raw && address(&columns[i].name).is_some() => {}
			(Some(_), _) => by_name.push(name),
			(None, Some(&i)) => {
				// `watch` heads a column with a name and no unit, so a name two channels share is
				// every one of them. The owner's own name for one of them, written before the
				// drive, is what tells them apart.
				let candidates: Vec<String> = meanings[i]
					.iter()
					.map(|&(unit, did, bits)| address_name(unit, did, bits.unwrap_or(0)))
					.collect();
				notes.push(format!(
					"{name}: the column \"{}\" is the name of {} channels of this car ({}), so which one it holds is not known — \
					 not used. A name of its own in ~/.vagcan/names.csv (`vagcan dev glossary`) before the drive tells them apart",
					columns[i].name,
					meanings[i].len(),
					candidates.join(", ")
				));
			}
			(None, None) => missing.push(name),
		}
	}
	if !missing.is_empty() {
		notes.insert(0, format!("not in the recording, so no value and never an alarm: {}", missing.join(", ")));
	}
	// `watch` names a column from the units it knew on the day: its own survey (`--survey`,
	// or the car's cached one) and the units it identified live — the engine, and every
	// `--did` unit that survey lacks. The plan's survey is `dash.toml`'s `survey =` or the
	// cached one. Where the two differ, a unit only `watch` knew could have a channel of the
	// same name, and nothing in the recording says which unit a column is from.
	if !by_name.is_empty() {
		notes.push(format!(
			"matched by name: {}. Checked against the units of the plan's survey; a unit `watch` knew on the day \
			 and that survey does not hold could share a name, and the recording does not say which unit a column is from",
			by_name.join(", ")
		));
	}
	Matched { sources, notes }
}

/// `01/200A` — how `watch` heads a channel nothing names — as a unit and an identifier.
fn address(heading: &str) -> Option<(u16, u16)> {
	let (unit, did) = heading.split_once('/')?;
	let unit = vag_uds_client::address::parse(unit).ok()?;
	let did = u16::from_str_radix(did, 16).ok()?;
	Some((unit.request, did))
}

fn name_of(channel: &PlanChannel) -> String {
	channel_name(&channel.label, channel.unit, channel.did, channel.bit_offset)
}

/// Every plan channel's readings out of the recording, in its own time where the file
/// gives one: `None` for a channel with no column, and for one whose column holds a cell
/// the plan's scaling cannot have produced, which is said in `notes`.
///
/// A reading is what the board's store would hold after it: a value, or `None` where the
/// unit answered with nothing the board decodes. A read that missed is `None` too, as on
/// the board — and a recording says one only by an empty cell whose own `_t_s` holds a
/// time (`watch --out` since 2026-09-26). An empty cell with no time of its own says
/// nothing: not heard yet, or a row between sweeps in a recording older than per-column
/// times, which leaves whole rows empty.
///
/// A converted column holds numbers on the plan's scaling, `0x…` cells and empty ones;
/// anything else drops the whole column (the owner's call, 2026-09-26). `watch` writes a
/// number exactly, so one the scaling cannot have produced was produced by another — or
/// is an older recording's unconverted answer written as bare hex, which nothing tells
/// apart from a number. Neither is salvaged: keeping the cells that happen to fit would
/// put numbers on the glass the board never shows.
pub fn series(recording: &Recording, sources: &[Option<Source>], plan: &Plan, device: &DevicePlan, notes: &mut Vec<String>) -> Vec<Option<Series>> {
	sources
		.iter()
		.enumerate()
		.map(|(k, source)| {
			let source = (*source)?;
			let (owned, board) = (plan.channels.get(k)?, device.channels.get(k)?);
			let column = match source {
				Source::Converted(i) | Source::Raw(i) => i,
			};
			let mut out: Series = Vec::new();
			for (row, (t, cells)) in recording.samples.iter().enumerate() {
				let own_time = recording.read_at.get(row).and_then(|r| r.get(column).copied().flatten());
				let Some(cell) = cells.get(column).and_then(|c| c.as_deref()) else {
					// A time and no value: the read at that time missed.
					if let Some(missed) = own_time {
						out.push((to_ms(missed), None));
					}
					continue;
				};
				let at = own_time.unwrap_or(*t);
				let value = match source {
					Source::Raw(_) => hex_bytes(cell).and_then(|bytes| board.decode(&bytes)),
					// An answer `watch` could not convert, marked: its bytes, through the board's
					// own decoder, which makes of them what the board would — nothing, for none.
					Source::Converted(_) => match unconverted(cell) {
						Some(bytes) => board.decode(&bytes),
						None => match written_number(cell).and_then(|v| on_the_boards_scale(v, owned, board)) {
							Some(value) => Some(value),
							None => {
								notes.push(format!(
									"{}: the recording's {cell:?} is not a number on the plan's scaling (×{} {:+}) — the column was \
									 recorded with another scaling, or it is from a recording made before 2026-09-26, which wrote an \
									 answer it could not convert as bare hex; not used",
									name_of(owned),
									owned.factor,
									owned.offset
								));
								return None;
							}
						},
					},
				};
				out.push((to_ms(at), value));
			}
			// A value repeats on every row until it is read again; one reading is one entry.
			out.sort_by_key(|(t, _)| *t);
			out.dedup_by_key(|(t, _)| *t);
			Some(out)
		})
		.collect()
}

/// A cell as the number `watch --out` wrote, or `None` for one it cannot have written.
///
/// It writes `format!("{v}")` of an `f64`: `Display` prints the shortest decimal that reads
/// back to the value and never an exponent, so below one it starts `0.` and never `0` then
/// another digit (`a_number_as_watch_writes_it_…` pins this). `0100` parses, and is not one.
fn written_number(cell: &str) -> Option<f64> {
	let leading_zero = cell.len() > 1 && cell.starts_with('0') && cell.as_bytes()[1].is_ascii_digit();
	if leading_zero {
		return None;
	}
	cell.parse().ok()
}

/// How far from a whole raw count a recorded value may invert and still be on the plan's
/// scaling. Floating-point error, not a judgement: see [`on_the_boards_scale`].
const RAW_SLACK: f64 = 1e-3;

/// A converted value as the board would have computed it: back to the raw integer through
/// the plan's own scaling, then `raw × factor + offset` in `f32`. `None` when the value is
/// not a whole raw value on that scaling.
///
/// Whole within [`RAW_SLACK`], a fixed amount in raw counts: `watch` writes the value with
/// `{v}`, which reads back exactly, so a value on this scaling inverts to within a few ulps
/// of an integer — at a raw value near 2³² that is around 1e-6. A bound that grew with the
/// value passed every number past half a million on any scaling.
fn on_the_boards_scale(value: f64, owned: &PlanChannel, board: &DeviceChannel) -> Option<f32> {
	if owned.factor == 0.0 || !value.is_finite() {
		return None;
	}
	let raw = (value - owned.offset) / owned.factor;
	let whole = raw.round();
	if (raw - whole).abs() > RAW_SLACK {
		return None;
	}
	Some(whole as f32 * board.factor + board.offset)
}

/// Seconds of the recording's clock as milliseconds.
pub fn to_ms(seconds: f64) -> u64 {
	(seconds * 1000.0).round().max(0.0) as u64
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::borrow::Cow;
	use vag_data_labels::catalog::{MeasurementDef, ReadId, Scaling};
	use vag_data_labels::measure::{LinearScale, RawForm};

	const ENGINE: u16 = 0x7E0;
	const GEARBOX: u16 = 0x7E1;

	/// A channel `watch` offers, named and scaled ×0.01 — every number neutral.
	fn offered(request: u16, did: u16, name: &'static str, raw_form: RawForm) -> Offered {
		Offered {
			request,
			did,
			def: Some(MeasurementDef {
				name: Cow::Borrowed(name),
				unit: Cow::Borrowed(""),
				address: ReadId::Uds(did),
				raw_form,
				scaling: Scaling::Linear(LinearScale { factor: 0.01, offset: 0.0 }),
			}),
			named: None,
			proven: false,
			text_id: None,
			selected: false,
		}
	}

	fn plan_channel(unit: u16, did: u16, bit_offset: u32, label: &str) -> PlanChannel {
		PlanChannel {
			unit,
			did,
			bit_offset,
			bit_length: 16,
			signed: true,
			big_endian: true,
			factor: 0.01,
			offset: 0.0,
			decimals: 2,
			unit_text: String::new(),
			label: label.to_string(),
			proven: false,
			hz: 10.0,
			source: String::new(),
			setpoint: None,
		}
	}

	fn plan(channels: Vec<PlanChannel>) -> Plan {
		Plan {
			vin: "TESTVIN0000000001".into(),
			language: "en".into(),
			units: vec![],
			channels,
			pages: vec![],
			alarms: vec![],
			stalk: None,
			stopwatch: None,
		}
	}

	fn columns(csv: &str) -> Recording {
		Recording::parse(csv).unwrap()
	}

	#[test]
	fn columns_are_matched_by_identity_whatever_order_the_recording_has_them_in() {
		let offered = [
			offered(ENGINE, 0x1001, "One", RawForm::I16Be),
			offered(ENGINE, 0x1002, "Two", RawForm::I16Be),
			offered(ENGINE, 0x1003, "Three", RawForm::I16Be),
		];
		let plan = plan(vec![
			plan_channel(ENGINE, 0x1001, 0, "one"),
			plan_channel(ENGINE, 0x1002, 0, "two"),
			plan_channel(ENGINE, 0x1003, 0, "three"),
		]);
		// Another order, and a column the plan does not have in between.
		let recording = columns("t_s,Three,Other,One,Two\n0.0,1,2,3,4\n");
		let matched = match_columns(&recording.columns, &offered, &Answered::default(), &plan);
		assert_eq!(
			matched.sources,
			[Some(Source::Converted(2)), Some(Source::Converted(3)), Some(Source::Converted(0))]
		);
		assert_eq!(matched.notes.len(), 1, "only the caveat on names: {:?}", matched.notes);
	}

	#[test]
	fn a_match_by_name_says_what_it_was_checked_against() {
		// `watch` heads a column from the units it knew on the day — its survey, and the
		// units it identified live (the engine, every `--did` unit) — which need not be
		// the plan's survey. A unit only it knew could share the name, and nothing here
		// can see that; an address heading names its unit and needs no caveat.
		let offered = [offered(ENGINE, 0x1001, "One", RawForm::I16Be)];
		let plan = plan(vec![plan_channel(ENGINE, 0x1001, 0, "one"), plan_channel(ENGINE, 0x1004, 0, "raw")]);
		let by_name = match_columns(&columns("t_s,One\n0.0,1\n").columns, &offered, &Answered::default(), &plan);
		assert!(
			by_name
				.notes
				.iter()
				.any(|n| n.starts_with("matched by name: one (01:1001).") && n.contains("the plan's survey")),
			"{:?}",
			by_name.notes
		);
		let by_address = match_columns(&columns("t_s,01/1004_raw\n0.0,0001\n").columns, &offered, &Answered::default(), &plan);
		assert_eq!(by_address.sources, [None, Some(Source::Raw(0))]);
		assert!(!by_address.notes.iter().any(|n| n.contains("matched by name")), "{:?}", by_address.notes);
	}

	#[test]
	fn a_field_is_its_own_identity_and_a_raw_answer_serves_every_field_of_it() {
		// Two fields of one identifier: the converted column is the one at its bit offset.
		let offered = [
			offered(ENGINE, 0x1001, "Low", RawForm::I16Be),
			offered(
				ENGINE,
				0x1001,
				"High",
				RawForm::Int {
					byte_offset: 2,
					byte_length: 2,
					signed: true,
					big_endian: true,
				},
			),
		];
		let plan = plan(vec![
			plan_channel(ENGINE, 0x1001, 16, "high"),
			plan_channel(ENGINE, 0x1001, 0, "low"),
			plan_channel(ENGINE, 0x1004, 0, "raw a"),
			plan_channel(ENGINE, 0x1004, 16, "raw b"),
		]);
		let recording = columns("t_s,Low,High,01/1004_raw\n0.0,1,2,00010002\n");
		let matched = match_columns(&recording.columns, &offered, &Answered::default(), &plan);
		assert_eq!(
			matched.sources,
			[
				Some(Source::Converted(1)),
				Some(Source::Converted(0)),
				Some(Source::Raw(2)),
				Some(Source::Raw(2))
			]
		);
	}

	#[test]
	fn a_heading_two_channels_share_is_not_guessed_and_a_missing_channel_is_said() {
		// One name on two units: `watch` writes no unit into the heading.
		let offered = [
			offered(ENGINE, 0x1001, "Speed", RawForm::I16Be),
			offered(GEARBOX, 0x2001, "Speed", RawForm::I16Be),
		];
		let plan = plan(vec![plan_channel(ENGINE, 0x1001, 0, "speed"), plan_channel(ENGINE, 0x1009, 0, "absent")]);
		let recording = columns("t_s,Speed\n0.0,1\n");
		let matched = match_columns(&recording.columns, &offered, &Answered::default(), &plan);
		assert_eq!(matched.sources, [None, None]);
		assert_eq!(matched.notes.len(), 2, "{:?}", matched.notes);
		assert!(matched.notes[0].starts_with("not in the recording") && matched.notes[0].contains("absent (01:1009)"));
		assert!(
			matched.notes[1].contains("\"Speed\" is the name of 2 channels of this car (01:1001, 02:2001)"),
			"{:?}",
			matched.notes
		);

		// The car's survey asked the gearbox for that identifier and got nothing: it wrote no
		// values, so the column is the engine's.
		let survey = r#"{"request":"7E1","dids":[{"did":"2002"}],"asked":["2000-20FF"]}"#;
		let answered = crate::plan::answered_from_survey(survey);
		let matched = match_columns(&recording.columns, &offered, &answered, &plan);
		assert_eq!(matched.sources, [Some(Source::Converted(0)), None]);
	}

	#[test]
	fn values_come_out_on_the_boards_scale_at_their_own_read_time() {
		let offered = [offered(ENGINE, 0x1001, "One", RawForm::I16Be)];
		let owned = plan(vec![plan_channel(ENGINE, 0x1001, 0, "one"), plan_channel(ENGINE, 0x1004, 0, "raw")]);
		let device = owned.to_device();
		// Per-column times; a value repeated on the next row; a read that missed (a time and
		// no value); a column not heard yet (neither); an answer `watch` could not convert,
		// marked, decoded by the board's own decoder, and a marked empty answer; and a raw
		// answer by the plan's layout (0xFF38 = -200 → -2.00). The raw column has no time of
		// its own, so its empty cells say nothing.
		let recording = columns(
			"t_s,One_t_s,One,01/1004_raw\n0.100,0.050,-2.3,FF38\n0.200,0.050,-2.3,\n0.300,0.250,0x0064,0064\n0.400,0.350,,0064\n\
			 0.500,0.450,0x05,\n0.600,,,\n0.700,0.650,0x,\n",
		);
		let matched = match_columns(&recording.columns, &offered, &Answered::default(), &owned);
		let mut notes = Vec::new();
		let series = series(&recording, &matched.sources, &owned, &device, &mut notes);
		assert!(notes.is_empty(), "{notes:?}");
		assert_eq!(
			series[0],
			Some(vec![
				(50, Some(-230.0f32 * 0.01)),
				(250, Some(1.0)),
				(350, None),
				(450, None),
				(650, None)
			]),
			"0x05 is one byte where the field is two, 0x no byte: the board makes nothing of either"
		);
		assert_eq!(series[1], Some(vec![(100, Some(-2.0)), (300, Some(1.0)), (400, Some(1.0))]));
	}

	#[test]
	fn an_empty_cell_with_no_time_of_its_own_is_not_a_miss() {
		// Recordings older than per-column times leave whole rows empty between sweeps —
		// `research/dumps/drive-gear.csv` has hundreds, interleaved. Reading those as misses
		// put a dash on the panel every other row.
		let offered = [offered(ENGINE, 0x1001, "A", RawForm::I16Be)];
		let owned = plan(vec![plan_channel(ENGINE, 0x1001, 0, "a")]);
		let device = owned.to_device();
		let read = |csv: &str| {
			let recording = columns(csv);
			let matched = match_columns(&recording.columns, &offered, &Answered::default(), &owned);
			series(&recording, &matched.sources, &owned, &device, &mut Vec::new())
		};
		assert_eq!(read("t_s,A\n0.0,5\n0.1,\n0.2,5\n"), [Some(vec![(0, Some(5.0)), (200, Some(5.0))])]);
		// That file's shape: a named column and a bare identifier, then empty rows.
		assert_eq!(
			read("t_s,A,0102\n0.000,806,00\n0.776,1029,00\n341.004,,\n354.027,,\n367.056,1100,00\n"),
			[Some(vec![(0, Some(806.0)), (776, Some(1029.0)), (367_056, Some(1100.0))])]
		);
	}

	/// One channel planned `factor` over a field `bits` wide, and what `series` makes of a
	/// one-column recording of it.
	fn read_on(factor: f64, bits: u32, csv: &str) -> (Vec<Option<Series>>, Vec<String>) {
		let offered = [offered(ENGINE, 0x1001, "One", RawForm::I16Be)];
		let mut channel = plan_channel(ENGINE, 0x1001, 0, "one");
		channel.factor = factor;
		channel.bit_length = bits;
		let owned = plan(vec![channel]);
		let device = owned.to_device();
		let recording = columns(csv);
		let matched = match_columns(&recording.columns, &offered, &Answered::default(), &owned);
		let mut notes = Vec::new();
		let series = series(&recording, &matched.sources, &owned, &device, &mut notes);
		(series, notes)
	}

	#[test]
	fn any_cell_off_the_plans_scaling_drops_the_column() {
		// `watch` writes a number with `{v}`, which reads back exactly: a number the plan's
		// scaling cannot have produced was produced by another, or is not a number at all.
		// Planned ×0.75 and recorded ×1, keeping the cells that happen to fit would show 3
		// and 6 where the board shows 2.25 and 4.5, or 1002 where it shows 751.5. Nothing is
		// salvaged: the owner's call, 2026-09-26.
		for (bits, csv) in [
			(16, "t_s,One\n0.0,3\n0.1,4\n0.2,6\n"),
			(16, "t_s,One\n0.0,3\n0.1,10\n0.2,6\n"),
			(16, "t_s,One\n0.0,1000\n0.1,1001\n0.2,1002\n"),
			(32, "t_s,One\n0.0,3\n0.1,1000\n0.2,6\n"),
		] {
			let (series, notes) = read_on(0.75, bits, csv);
			assert_eq!(series, [None], "{bits}-bit: {csv:?}");
			assert_eq!(notes.len(), 1, "{notes:?}");
			assert!(notes[0].contains("not a number on the plan's scaling (×0.75 +0)"), "{notes:?}");
		}
	}

	#[test]
	fn an_old_recordings_bare_hex_drops_the_column_and_says_why() {
		// Before 2026-09-26 `watch --out` wrote an answer it could not convert as bare hex in
		// the converted column. Nothing tells all of that from a number, so a column holding
		// any is not used — and the note says it may be that, not only another scaling.
		for cell in ["0B", "01", "0B34", "0100"] {
			let (series, notes) = read_on(0.01, 16, &format!("t_s,One\n0.0,-2.3\n0.1,{cell}\n"));
			assert_eq!(series, [None], "{cell}");
			assert!(notes[0].contains("bare hex") && notes[0].contains("before 2026-09-26"), "{notes:?}");
		}
		// Numbers that fit are the values the board computes.
		let (series, notes) = read_on(0.01, 16, "t_s,One\n0.0,-2.3\n0.3,1000\n0.4,0\n0.5,0.5\n");
		assert!(notes.is_empty(), "{notes:?}");
		assert_eq!(
			series[0],
			Some(vec![
				(0, Some(-230.0f32 * 0.01)),
				(300, Some(100_000.0f32 * 0.01)),
				(400, Some(0.0)),
				(500, Some(50.0f32 * 0.01))
			])
		);
	}

	#[test]
	fn a_number_as_watch_writes_it_never_has_an_exponent_or_a_leading_zero_before_a_digit() {
		// `watch --out` writes `format!("{v}")` of an `f64`. `Display` for a float prints
		// the shortest decimal that reads back to it and never an exponent (unlike `{:e}`
		// or `Debug` for very large and very small values), so a number starts `0.` below
		// one and never `0` followed by another digit. This pins why `0100` is not a number
		// `watch` writes, though it parses as one.
		assert_eq!(format!("{}", 1e21f64), "1000000000000000000000");
		assert_eq!(format!("{}", 1e-7f64), "0.0000001");
		assert_eq!(format!("{}", 0.5f64), "0.5");
		assert_eq!(format!("{}", -0.0f64), "-0");
		assert_eq!(format!("{}", 0.0f64), "0");
		assert_eq!(format!("{}", 100.0f64), "100");
	}

	#[test]
	fn a_large_raw_value_fits_only_its_own_scaling() {
		// A tolerance that grew with the raw value passed everything past half a million:
		// 32-bit, planned ×0.75, recorded ×1, `1000000` is raw 1333333.33 and was kept as
		// 999999.75 where the board shows 750000.
		let (series, notes) = read_on(0.75, 32, "t_s,One\n0.0,1000000\n");
		assert_eq!(series, [None]);
		assert!(notes[0].contains("not a number on the plan's scaling"), "{notes:?}");
	}

	#[test]
	fn a_correct_column_with_raw_values_near_the_top_of_32_bits_is_kept() {
		// What `watch` writes is `{v}` of `raw × factor + offset` in f64, which reads back
		// exactly; inverting it is off by a few ulps at most, far under the bound — with a
		// factor no power of two divides and an offset.
		for (factor, offset) in [(0.1, -40.0), (0.75, -40.0), (0.001, 273.15)] {
			let mut owned = plan_channel(ENGINE, 0x1001, 0, "one");
			(owned.factor, owned.offset, owned.bit_length, owned.signed) = (factor, offset, 32, false);
			let board = plan(vec![owned.clone()]).to_device().channels[0];
			for raw in [u64::from(u32::MAX), u64::from(u32::MAX) - 1, 4_000_000_001, 2_147_483_647, 1, 0] {
				let written = format!("{}", raw as f64 * factor + offset);
				let read: f64 = written.parse().unwrap();
				let off = ((read - offset) / factor - raw as f64).abs();
				assert!(
					off < RAW_SLACK / 100.0,
					"×{factor} {offset:+}: raw {raw} inverts {off} off — the bound is too tight"
				);
				assert_eq!(
					on_the_boards_scale(read, &owned, &board),
					Some(raw as f32 * board.factor + board.offset),
					"×{factor} {offset:+}: raw {raw} written as {written}"
				);
			}
		}
	}

	#[test]
	fn a_column_recorded_on_another_scaling_is_not_used() {
		let offered = [offered(ENGINE, 0x1001, "One", RawForm::I16Be)];
		let owned = plan(vec![plan_channel(ENGINE, 0x1001, 0, "one")]);
		let device = owned.to_device();
		let recording = columns("t_s,One\n0.0,-2.305\n");
		let matched = match_columns(&recording.columns, &offered, &Answered::default(), &owned);
		let mut notes = Vec::new();
		assert_eq!(series(&recording, &matched.sources, &owned, &device, &mut notes), [None]);
		assert!(notes[0].contains("not a number on the plan's scaling"), "{notes:?}");
	}
}
