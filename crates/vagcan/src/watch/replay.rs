//! Playing a recorded drive back through the live view.
//!
//! `watch --out` writes what it saw, second by second. This reads one of those
//! files back and drives the same screen from it, with no adapter and no car —
//! so the interface can be tried, shown or changed by someone who is nowhere
//! near a vehicle.
//!
//! It is a **separate loop** from the live one on purpose. The live path holds
//! a serial port, addresses control units and asks them thousands of questions;
//! this project has already learned what that can cost (`SAFETY.md`), and the
//! way to keep a verified path verified is not to thread a second mode through
//! it. The two share what is safe to share — the screen, the key handling, the
//! channel model — and nothing else.
//!
//! What a recording holds is a *display value* per column, not the bytes that
//! produced it. Bytes are recovered where that is exact: a column marked `_raw`
//! is hex and converts directly, and a converted column can be inverted through
//! the linear scaling that produced it. A column that cannot be inverted is
//! dropped with a note rather than shown as an approximation.

use std::collections::BTreeMap;

use vag_data::catalog::Scaling;
use vag_data::measure::RawForm;

use super::plan::Channel;

/// One recorded run: the columns it holds and the samples in time order.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
	/// Column headings, in file order, with the `_raw` marker stripped and
	/// remembered.
	pub columns: Vec<Column>,
	/// `(seconds from the start, one cell per column)`.
	pub samples: Vec<(f64, Vec<Option<String>>)>,
}

/// A column of a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
	/// The heading as written, minus the `_raw` suffix.
	pub name: String,
	/// Whether the cells are raw bytes in hex rather than converted values.
	pub raw: bool,
}

/// How long the recording runs, in seconds.
impl Recording {
	pub fn duration(&self) -> f64 {
		match (self.samples.first(), self.samples.last()) {
			(Some((first, _)), Some((last, _))) => last - first,
			_ => 0.0,
		}
	}

	/// The last sample at or before `t` seconds from the start.
	///
	/// Holds the previous value rather than interpolating: these are readings
	/// from a bus, and a value between two of them was never on the car.
	pub fn at(&self, t: f64) -> Option<&(f64, Vec<Option<String>>)> {
		let start = self.samples.first()?.0;
		self.samples.iter().rfind(|(at, _)| *at - start <= t)
	}

	/// Parse a `watch --out` recording.
	///
	/// The layout is `t_s` and then either `name` or `name_t_s,name` pairs —
	/// the per-column timestamp is written when the value came from a
	/// different moment than the row. Both are accepted; the per-column time
	/// is ignored here, because a replay is driven by one clock.
	pub fn parse(csv: &str) -> Result<Recording, String> {
		let mut lines = csv.lines().filter(|l| !l.trim().is_empty());
		let header = lines.next().ok_or("the recording is empty")?;
		let headings: Vec<&str> = header.split(',').collect();
		if headings.first().map(|h| h.trim()) != Some("t_s") {
			return Err("not a `watch --out` recording: the first column is not t_s".into());
		}

		// Walk the header the same way the writer built it, so a `name_t_s`
		// column is recognised as the partner of the one after it rather than
		// becoming a channel of its own.
		let mut columns = Vec::new();
		let mut cells: Vec<usize> = Vec::new();
		let mut i = 1;
		while i < headings.len() {
			let paired = headings
				.get(i)
				.zip(headings.get(i + 1))
				.is_some_and(|(t, v)| t.strip_suffix("_t_s") == Some(*v));
			let at = if paired { i + 1 } else { i };
			let heading = headings[at].trim();
			let (name, raw) = match heading.strip_suffix("_raw") {
				Some(base) => (base.to_string(), true),
				None => (heading.to_string(), false),
			};
			columns.push(Column { name, raw });
			cells.push(at);
			i += if paired { 2 } else { 1 };
		}
		if columns.is_empty() {
			return Err("the recording has no value columns".into());
		}

		let mut samples = Vec::new();
		for line in lines {
			let row: Vec<&str> = line.split(',').collect();
			let Some(Ok(t)) = row.first().map(|c| c.trim().parse::<f64>()) else {
				continue;
			};
			let values = cells
				.iter()
				.map(|at| row.get(*at).map(|c| c.trim()).filter(|c| !c.is_empty()).map(str::to_string))
				.collect();
			samples.push((t, values));
		}
		if samples.is_empty() {
			return Err("the recording has no samples".into());
		}
		Ok(Recording { columns, samples })
	}
}

/// Which channel each column feeds, matching against the catalogs and adding
/// channels for the rest.
///
/// A column is matched by the name the writer would have given it: the
/// measurement's name when one is known, otherwise `unit/DID`. A bare `0102`
/// is matched by identifier, which is how older recordings were written.
///
/// A column that matches nothing is **not dropped**: it becomes a channel with
/// no definition and shows its bytes tagged `(raw)`, which is exactly what the
/// live view does with an identifier nobody has proven. A recording is mostly
/// such columns, and hiding them would make the replay a demo of a different,
/// tidier tool than the one that exists.
pub fn resolve(columns: &[Column], channels: &mut Vec<Channel>, request: u16) -> BTreeMap<usize, Resolved> {
	let mut out = BTreeMap::new();
	for (index, column) in columns.iter().enumerate() {
		let by_label = channels.iter().position(|c| c.label() == column.name);
		let did = u16::from_str_radix(&column.name, 16).ok();
		if let Some(channel) = by_label {
			out.insert(index, Resolved { channel, raw: column.raw });
			continue;
		}
		// Only a column that names an identifier can be one; a heading that is
		// neither a known measurement nor a DID is not a reading.
		let Some(did) = did else { continue };
		// **The heading decides the format here.** A writer that had a
		// definition wrote the measurement's name and a converted value; a
		// bare identifier means it had none, so the cells are bytes — even
		// when this build has since proven a scaling for that identifier and
		// resolves the column to a named channel. Reading such a column as
		// converted was how the gear selector replayed as a blank.
		let channel = match channels.iter().position(|c| c.did == did) {
			Some(channel) => channel,
			None => {
				channels.push(Channel {
					request,
					did,
					def: None,
					proven: false,
					selected: false,
				});
				channels.len() - 1
			}
		};
		out.insert(index, Resolved { channel, raw: true });
	}
	out
}

/// A column matched to a channel, and how its cells are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
	pub channel: usize,
	/// True when the cells are bytes in hex rather than converted values.
	pub raw: bool,
}

/// Which columns actually moved during the recording.
///
/// A column that read the same bytes from start to finish proves nothing —
/// the rule this project applies to every measurement it has tried to
/// establish — and a screen full of them is a poor first impression of what
/// the tool does. The rest stay available behind the selection screen.
pub fn columns_that_moved(recording: &Recording) -> Vec<usize> {
	(0..recording.columns.len())
		.filter(|column| {
			let mut seen: Option<&str> = None;
			recording.samples.iter().any(|(_, cells)| {
				let Some(cell) = cells.get(*column).and_then(|c| c.as_deref()) else {
					return false;
				};
				match seen {
					Some(first) => first != cell,
					None => {
						seen = Some(cell);
						false
					}
				}
			})
		})
		.collect()
}

/// Turn a recorded cell back into the bytes that produced it.
///
/// Exact or nothing: a raw cell is hex and converts directly, and a converted
/// cell is inverted through the linear scaling it came from. A discrete state
/// or a non-linear scaling cannot be inverted, and returns `None` rather than
/// a number that looks like a reading and is not one.
pub fn cell_to_bytes(cell: &str, channel: &Channel, raw: bool) -> Option<Vec<u8>> {
	// The `_raw` marker settles it when present. When it is absent the channel
	// does: a column for an identifier with no proven scaling cannot have had
	// a converted value written for it, so its cells are bytes. Recordings
	// made before the marker existed are entirely of that kind.
	if raw || channel.def.is_none() {
		return hex_bytes(cell);
	}
	let def = channel.def.as_ref()?;
	let count = match &def.scaling {
		Scaling::Linear(scale) if scale.factor != 0.0 => {
			let value: f64 = cell.parse().ok()?;
			((value - scale.offset) / scale.factor).round()
		}
		// A discrete state inverts exactly by looking its name up in the same
		// table that produced it — `D` came from one code and no other. This is
		// a lookup, not an estimate, so gear and selector replay faithfully.
		Scaling::Enum { levels } => levels.iter().find(|(_, name)| name == cell).map(|(code, _)| *code as f64)?,
		// An anchor fixes one point and leaves the slope unproven; there is no
		// line to invert, and inventing one would put a number on screen that
		// was never measured.
		_ => return None,
	};
	if !count.is_finite() || count < 0.0 {
		return None;
	}
	encode(count as u64, def.raw_form)
}

/// Lay an integer out the way a control unit would have sent it.
fn encode(count: u64, form: RawForm) -> Option<Vec<u8>> {
	let be = |width: usize| -> Option<Vec<u8>> {
		let bytes = count.to_be_bytes();
		(count < 1u64 << (8 * width)).then(|| bytes[8 - width..].to_vec())
	};
	match form {
		RawForm::U8First => be(1),
		// The value lives in the second byte; the first was never recorded, so
		// it is filled with zero and the reader takes the one it wants.
		RawForm::U8Second => be(1).map(|b| vec![0, b[0]]),
		RawForm::U16Be => be(2),
		RawForm::U16Le => be(2).map(|mut b| {
			b.reverse();
			b
		}),
		RawForm::I16Be => be(2),
		RawForm::U24Be => be(3),
		RawForm::U32Be => be(4),
	}
}

fn hex_bytes(text: &str) -> Option<Vec<u8>> {
	if text.is_empty() || text.len() % 2 != 0 {
		return None;
	}
	(0..text.len() / 2)
		.map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok())
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::borrow::Cow;
	use vag_data::catalog::{MeasurementDef, ReadId};
	use vag_data::measure::LinearScale;

	fn rpm_channel() -> Channel {
		Channel {
			request: 0x7E1,
			did: 0x380A,
			def: Some(MeasurementDef {
				name: Cow::Borrowed("Input shaft speed"),
				unit: Cow::Borrowed("/min"),
				address: ReadId::Uds(0x380A),
				raw_form: RawForm::U16Le,
				scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 }),
			}),
			proven: true,
			selected: false,
		}
	}

	#[test]
	fn a_recording_parses_into_columns_and_samples() {
		let csv = "t_s,Input shaft speed,0102_raw\n0.000,690,0B34\n0.100,700,0C40\n";
		let recording = Recording::parse(csv).unwrap();
		assert_eq!(recording.columns.len(), 2);
		assert_eq!(
			recording.columns[0],
			Column {
				name: "Input shaft speed".into(),
				raw: false
			}
		);
		assert_eq!(
			recording.columns[1],
			Column {
				name: "0102".into(),
				raw: true
			}
		);
		assert_eq!(recording.samples.len(), 2);
		assert!((recording.duration() - 0.1).abs() < 1e-9);
	}

	#[test]
	fn per_column_timestamps_are_recognised_as_partners_not_as_channels() {
		// The writer emits `name_t_s,name` when the value came from a
		// different moment than the row; reading that as two columns would
		// put a clock on screen as if it were a measurement.
		let csv = "t_s,Boost_t_s,Boost\n0.000,0.000,1.01\n0.100,0.090,1.02\n";
		let recording = Recording::parse(csv).unwrap();
		assert_eq!(recording.columns.len(), 1);
		assert_eq!(recording.columns[0].name, "Boost");
		assert_eq!(recording.samples[1].1, vec![Some("1.02".to_string())]);
	}

	#[test]
	fn a_file_that_is_not_a_recording_says_so() {
		assert!(Recording::parse("").is_err());
		assert!(Recording::parse("did,data\nF187,3856\n").is_err());
		assert!(Recording::parse("t_s,Boost\n").is_err(), "a header with no samples");
	}

	#[test]
	fn playback_holds_the_last_reading_rather_than_inventing_one_between() {
		// A value between two samples was never on the bus.
		let csv = "t_s,Boost_raw\n0.000,0B34\n1.000,0C40\n2.000,0D00\n";
		let recording = Recording::parse(csv).unwrap();
		assert_eq!(recording.at(0.0).unwrap().1[0].as_deref(), Some("0B34"));
		assert_eq!(recording.at(0.9).unwrap().1[0].as_deref(), Some("0B34"));
		assert_eq!(recording.at(1.0).unwrap().1[0].as_deref(), Some("0C40"));
		assert_eq!(recording.at(99.0).unwrap().1[0].as_deref(), Some("0D00"));
	}

	#[test]
	fn a_raw_cell_converts_straight_back_to_bytes() {
		let channel = rpm_channel();
		assert_eq!(cell_to_bytes("0B34", &channel, true), Some(vec![0x0B, 0x34]));
		assert_eq!(cell_to_bytes("0B3", &channel, true), None, "half a byte is not a reading");
		assert_eq!(cell_to_bytes("ZZ", &channel, true), None);
	}

	#[test]
	fn a_converted_cell_is_inverted_through_the_scaling_that_produced_it() {
		// Round trip against the real thing: the gearbox reports its input
		// shaft little-endian, so 690 /min was `B2 02` on the wire.
		let channel = rpm_channel();
		let bytes = cell_to_bytes("690", &channel, false).unwrap();
		assert_eq!(bytes, vec![0xB2, 0x02]);
		assert_eq!(channel.def.as_ref().unwrap().interpret(&bytes), Some(690.0));
	}

	#[test]
	fn only_the_columns_that_moved_are_offered_first() {
		// Measured on the reference recording: 29 of its 104 columns carry
		// anything at all, and the 75 that read zero throughout would fill the
		// screen before the first interesting row.
		let csv = "t_s,Moving_raw,Stuck_raw,Empty_raw\n                   0.0,0B34,0000,\n                   1.0,0C40,0000,\n";
		let recording = Recording::parse(csv).unwrap();
		assert_eq!(columns_that_moved(&recording), vec![0]);
	}

	#[test]
	fn a_column_headed_by_an_identifier_stays_bytes_even_once_the_row_is_proven() {
		// The recording was written before this project proved 0x3809, so its
		// heading is the bare identifier and its cells are bytes. Resolving it
		// to the now-named channel must not make the replay read those bytes
		// as a state name — that is how the selector replayed as a blank.
		let mut channels = vec![Channel {
			did: 0x3809,
			def: Some(MeasurementDef {
				name: Cow::Borrowed("Selector lever"),
				raw_form: RawForm::U8First,
				scaling: Scaling::Enum {
					levels: vec![(5, "D".into())],
				},
				..rpm_channel().def.unwrap()
			}),
			..rpm_channel()
		}];
		let columns = vec![Column {
			name: "3809".into(),
			raw: false,
		}];
		let resolved = resolve(&columns, &mut channels, 0x7E1);
		assert!(resolved[&0].raw, "the heading says bytes, whatever the catalog now knows");
		assert_eq!(cell_to_bytes("05", &channels[0], resolved[&0].raw), Some(vec![0x05]));
	}

	#[test]
	fn an_unmarked_column_for_an_unproven_identifier_is_read_as_bytes() {
		// Recordings written before the `_raw` marker existed have bare hex
		// under a bare identifier. Insisting on the marker would replay them
		// as an empty screen — which is how this was found.
		let unknown = Channel { def: None, ..rpm_channel() };
		assert_eq!(cell_to_bytes("0B34", &unknown, false), Some(vec![0x0B, 0x34]));
		assert_eq!(cell_to_bytes("nope", &unknown, false), None);
	}

	#[test]
	fn a_discrete_state_inverts_by_name_because_that_is_a_lookup_not_a_guess() {
		// `R` came from one code and no other, so a recorded gear replays as
		// the byte the gearbox actually sent.
		let gear = Channel {
			def: Some(MeasurementDef {
				raw_form: RawForm::U8First,
				scaling: Scaling::Enum {
					levels: vec![(5, "4".into()), (0x0C, "R".into())],
				},
				..rpm_channel().def.unwrap()
			}),
			..rpm_channel()
		};
		assert_eq!(cell_to_bytes("R", &gear, false), Some(vec![0x0C]));
		assert_eq!(cell_to_bytes("4", &gear, false), Some(vec![0x05]));
		// A state the table does not list is not invented.
		assert_eq!(cell_to_bytes("N", &gear, false), None);
	}

	#[test]
	fn a_value_that_cannot_be_inverted_exactly_is_refused() {
		// An anchor fixes one point and leaves the slope unproven: there is no
		// line to invert, so nothing is shown rather than something made up.
		let anchored = Channel {
			def: Some(MeasurementDef {
				scaling: Scaling::Anchor { raw: 0x5555, value: 0.0 },
				..rpm_channel().def.unwrap()
			}),
			..rpm_channel()
		};
		assert_eq!(cell_to_bytes("0", &anchored, false), None);
		// And a value the form cannot hold is not truncated into one it can.
		let channel = rpm_channel();
		assert_eq!(cell_to_bytes("70000", &channel, false), None);
		assert_eq!(cell_to_bytes("-5", &channel, false), None);
	}

	#[test]
	fn columns_resolve_to_channels_by_name_or_by_identifier() {
		let mut channels = vec![
			rpm_channel(),
			Channel {
				did: 0x0102,
				def: None,
				..rpm_channel()
			},
		];
		let columns = vec![
			Column {
				name: "Input shaft speed".into(),
				raw: false,
			},
			Column {
				name: "0102".into(),
				raw: true,
			},
			Column {
				name: "Nothing here".into(),
				raw: false,
			},
		];
		let resolved = resolve(&columns, &mut channels, 0x7E1);
		assert_eq!(resolved[&0], Resolved { channel: 0, raw: false }, "by measurement name");
		assert_eq!(resolved[&1], Resolved { channel: 1, raw: true }, "by identifier");
		assert_eq!(resolved.get(&2), None, "a heading that is not an identifier is not a reading");
		assert_eq!(channels.len(), 2, "nothing was invented for it");
	}

	#[test]
	fn an_unproven_identifier_becomes_a_raw_channel_rather_than_vanishing() {
		// A recording is mostly columns nobody has proven. Dropping them would
		// replay a tidier tool than the one that exists.
		let mut channels = vec![rpm_channel()];
		let columns = vec![Column {
			name: "0306".into(),
			raw: true,
		}];
		let resolved = resolve(&columns, &mut channels, 0x7E1);
		assert_eq!(resolved[&0], Resolved { channel: 1, raw: true });
		assert_eq!(channels[1].did, 0x0306);
		assert!(channels[1].def.is_none());
		assert_eq!(channels[1].render(&[0x0B, 0x34]), "0B34 (raw)");
	}
}
