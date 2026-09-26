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
use crate::watch::replay::{Column, Recording};

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
			(Some(_), _) => {}
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
/// gives one: `None` for a channel with no column, and for one whose column turns out not
/// to be on the plan's scaling, which is said in `notes`.
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
				// An empty cell: `watch` had heard nothing for this channel yet.
				let Some(cell) = cells.get(column).and_then(|c| c.as_deref()) else {
					continue;
				};
				let at = recording.read_at.get(row).and_then(|r| r.get(column).copied().flatten()).unwrap_or(*t);
				let value = match source {
					Source::Raw(_) => hex_bytes(cell).and_then(|bytes| board.decode(&bytes)),
					// Not a number is an answer `watch` could not convert, which it writes as
					// its bytes: the unit answered, and nothing decodes out of it.
					Source::Converted(_) => match cell.parse::<f64>() {
						Err(_) => None,
						Ok(v) => match on_the_boards_scale(v, owned, board) {
							Some(v) => Some(v),
							None => {
								notes.push(format!(
									"{}: the recording's {cell} is not a value of the plan's scaling (×{} {:+}) — recorded with another; not used",
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

/// A converted value as the board would have computed it: back to the raw integer through
/// the plan's own scaling, then `raw × factor + offset` in `f32`. `None` when the value is
/// not a whole raw value on that scaling.
fn on_the_boards_scale(value: f64, owned: &PlanChannel, board: &DeviceChannel) -> Option<f32> {
	if owned.factor == 0.0 || !value.is_finite() {
		return None;
	}
	let raw = (value - owned.offset) / owned.factor;
	let whole = raw.round();
	if (raw - whole).abs() > 1e-6 * whole.abs().max(1.0) {
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
		assert!(matched.notes.is_empty(), "{:?}", matched.notes);
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
		// Per-column times, a repeated value on the next row, a value `watch` could not
		// convert, and a raw answer decoded by the plan's layout (0xFF38 = -200 → -2.00).
		let recording = columns("t_s,One_t_s,One,01/1004_raw\n0.100,0.050,-2.3,FF38\n0.200,0.050,-2.3,\n0.300,0.250,0B,0064\n");
		let matched = match_columns(&recording.columns, &offered, &Answered::default(), &owned);
		let mut notes = Vec::new();
		let series = series(&recording, &matched.sources, &owned, &device, &mut notes);
		assert!(notes.is_empty(), "{notes:?}");
		assert_eq!(series[0], Some(vec![(50, Some(-230.0f32 * 0.01)), (250, None)]));
		assert_eq!(series[1], Some(vec![(100, Some(-2.0)), (300, Some(1.0))]));
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
		assert!(notes[0].contains("not a value of the plan's scaling"), "{notes:?}");
	}
}
