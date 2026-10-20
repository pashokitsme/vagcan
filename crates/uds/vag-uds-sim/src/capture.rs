//! A profile from a capture of a real conversation (`vagcan dev sniff --out`, or any
//! `vag-uds-capture` file): the car plays back what it was heard to say.
//!
//! Frames are put back together into whole PDUs per CAN id, each answer is paired with the
//! request before it on its unit, and every answer becomes the profile's: an identifier's
//! reads become its value — a series when they changed, timed from the first frame — and
//! fault lists and records become the unit's faults. Nothing is added that the capture
//! did not hear. What cannot be taken over is counted: a read of several identifiers at
//! once, whose answer does not say where one identifier's bytes end.

use std::collections::BTreeMap;

use vag_uds_capture::{CapturePayload, CaptureRecord};
use vag_uds_client::UnitAddress;
use vag_uds_transport::CanId;

use crate::profile::{DidSpec, DidTable, FaultSpec, Hex, Id, Point, Profile, UnitSpec};

/// What a capture gave, beside the profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Taken {
	pub frames: usize,
	pub units: usize,
	pub identifiers: usize,
	pub series: usize,
	pub faults: usize,
	/// Answers to a read of several identifiers, which do not split by themselves.
	pub multi_reads: usize,
	/// Answers with no request before them on their unit.
	pub unpaired: usize,
}

/// Put frames back into PDUs, one id at a time.
#[derive(Default)]
struct Reassembly {
	partial: BTreeMap<u16, (usize, Vec<u8>, u64)>,
}

impl Reassembly {
	/// The whole PDU this frame finishes, with the time of its first frame.
	fn push(&mut self, id: u16, at_us: u64, data: &[u8]) -> Option<(u64, Vec<u8>)> {
		let pci = *data.first()?;
		match pci >> 4 {
			0x0 => {
				let len = usize::from(pci & 0x0F);
				self.partial.remove(&id);
				data.get(1..1 + len).filter(|_| len > 0).map(|pdu| (at_us, pdu.to_vec()))
			}
			0x1 => {
				let len = (usize::from(pci & 0x0F) << 8) | usize::from(*data.get(1)?);
				self.partial.insert(id, (len, data.get(2..)?.to_vec(), at_us));
				None
			}
			0x2 => {
				let (len, buf, _) = self.partial.get_mut(&id)?;
				let take = (*len - buf.len()).min(data.len() - 1);
				buf.extend_from_slice(&data[1..1 + take]);
				if buf.len() >= *len {
					let (len, mut buf, at) = self.partial.remove(&id)?;
					buf.truncate(len);
					return Some((at, buf));
				}
				None
			}
			_ => None,
		}
	}
}

/// Whether a PDU is a request: UDS requests carry service ids below `0x40`, answers
/// `0x40` more, and refusals `7F`.
fn is_request(pdu: &[u8]) -> bool {
	matches!(pdu.first(), Some(0x01..=0x3F | 0x80..=0xBF))
}

#[derive(Default)]
struct Heard {
	reads: BTreeMap<u16, Vec<(u64, Vec<u8>)>>,
	refused: BTreeMap<u16, u8>,
	faults: BTreeMap<[u8; 3], u8>,
	extended: BTreeMap<[u8; 3], Vec<u8>>,
	snapshot: BTreeMap<[u8; 3], Vec<u8>>,
	supported: Vec<[u8; 3]>,
	answered: bool,
}

/// The profile a capture describes.
pub fn profile(records: &[CaptureRecord], about: &str) -> (Profile, Taken) {
	let mut taken = Taken::default();
	let mut parts = Reassembly::default();
	let mut asked: BTreeMap<u16, Vec<u8>> = BTreeMap::new();
	let mut heard: BTreeMap<u16, Heard> = BTreeMap::new();
	let start = records
		.iter()
		.find(|r| matches!(r.payload, CapturePayload::CanFrame { .. }))
		.map_or(0, |r| r.ts_us);
	for record in records {
		let CapturePayload::CanFrame {
			id: CanId::Standard(id),
			data,
		} = &record.payload
		else {
			continue;
		};
		taken.frames += 1;
		let Some((at_us, pdu)) = parts.push(*id, record.ts_us, data) else {
			continue;
		};
		if is_request(&pdu) && UnitAddress::from_request(*id).is_some() {
			asked.insert(*id, pdu);
			continue;
		}
		// An answer: on the answer id of the unit asked last on its request id.
		let Some(unit) = asked
			.keys()
			.copied()
			.find(|r| UnitAddress::from_request(*r).is_some_and(|a| a.response == *id))
		else {
			taken.unpaired += 1;
			continue;
		};
		if pdu.get(..1) == Some(&[0x7F]) && pdu.get(2) == Some(&0x78) {
			continue;
		}
		let request = asked[&unit].clone();
		let t = (at_us.saturating_sub(start)) / 1000;
		take(heard.entry(unit).or_default(), &request, &pdu, t, &mut taken);
	}

	let mut units = Vec::new();
	for (request, heard) in heard.into_iter().filter(|(_, h)| h.answered) {
		let mut did = BTreeMap::new();
		for (number, reads) in heard.reads {
			taken.identifiers += 1;
			let same = reads.windows(2).all(|w| w[0].1 == w[1].1);
			let spec = match same {
				true => DidSpec::Value(Hex(reads[0].1.clone())),
				false => {
					taken.series += 1;
					// Only the changes: a point that repeats the one before says nothing.
					let mut points: Vec<Point> = Vec::new();
					for (at_ms, value) in reads {
						if points.last().is_none_or(|p| p.value.0 != value) {
							points.push(Point { at_ms, value: Hex(value) });
						}
					}
					DidSpec::Full(DidTable {
						series: Some(points),
						..DidTable::default()
					})
				}
			};
			did.insert(format!("{number:04X}"), spec);
		}
		for (number, nrc) in heard.refused {
			let key = format!("{number:04X}");
			if !did.contains_key(&key) && nrc != 0x31 {
				did.insert(
					key,
					DidSpec::Full(DidTable {
						nrc: Some(nrc),
						..DidTable::default()
					}),
				);
			}
		}
		taken.faults += heard.faults.len();
		let faults = heard
			.faults
			.iter()
			.map(|(code, status)| FaultSpec {
				code: Hex(code.to_vec()),
				status: *status,
				extended: heard.extended.get(code).cloned().map(Hex),
				snapshot: heard.snapshot.get(code).cloned().map(Hex),
			})
			.collect();
		let supported = heard
			.supported
			.iter()
			.filter(|code| !heard.faults.contains_key(*code))
			.map(|code| Hex(code.to_vec()))
			.collect();
		taken.units += 1;
		units.push(UnitSpec {
			request: Id(request),
			did,
			faults,
			supported,
			..UnitSpec::default()
		});
	}
	let profile = Profile {
		about: Some(about.to_string()),
		units,
		..Profile::default()
	};
	(profile, taken)
}

/// What one answer to one request says about the unit.
fn take(unit: &mut Heard, request: &[u8], answer: &[u8], t: u64, taken: &mut Taken) {
	unit.answered = true;
	match (request, answer) {
		([0x22, hi, lo], [0x62, ahi, alo, data @ ..]) if (hi, lo) == (ahi, alo) => {
			unit.reads.entry(u16::from_be_bytes([*hi, *lo])).or_default().push((t, data.to_vec()));
		}
		([0x22, ..], [0x62, ..]) => taken.multi_reads += 1,
		([0x22, hi, lo], [0x7F, 0x22, nrc]) => {
			unit.refused.insert(u16::from_be_bytes([*hi, *lo]), *nrc);
		}
		([0x19, 0x02, ..], [0x59, 0x02, _avail, list @ ..]) => {
			unit.faults = list.chunks_exact(4).map(|c| ([c[0], c[1], c[2]], c[3])).collect();
		}
		([0x19, 0x0A, ..], [0x59, 0x0A, _avail, list @ ..]) => {
			unit.supported = list.chunks_exact(4).map(|c| [c[0], c[1], c[2]]).collect();
		}
		([0x19, sub @ (0x04 | 0x06), ..], [0x59, asub, a, b, c, _status, records @ ..]) if sub == asub => {
			let into = if *sub == 0x04 { &mut unit.snapshot } else { &mut unit.extended };
			into.insert([*a, *b, *c], records.to_vec());
		}
		_ => {}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use vag_uds_capture::Direction;

	fn frame(ts_us: u64, id: u16, data: &[u8]) -> CaptureRecord {
		let mut padded = data.to_vec();
		padded.resize(8, 0xAA);
		CaptureRecord {
			ts_us,
			dir: Direction::Rx,
			payload: CapturePayload::CanFrame {
				id: CanId::Standard(id),
				data: padded,
			},
		}
	}

	#[test]
	fn reads_become_values_and_changing_reads_a_series() {
		let records = vec![
			frame(1_000_000, 0x7E0, &[0x03, 0x22, 0xF4, 0x0D]),
			frame(1_010_000, 0x7E8, &[0x04, 0x62, 0xF4, 0x0D, 0x00]),
			frame(1_100_000, 0x7E0, &[0x03, 0x22, 0xF1, 0x87]),
			frame(1_110_000, 0x7E8, &[0x03, 0x7F, 0x22, 0x78]),
			frame(1_120_000, 0x7E8, &[0x10, 0x0A, 0x62, 0xF1, 0x87, b'P', b'A', b'R']),
			frame(1_121_000, 0x7E0, &[0x30, 0x00, 0x00]),
			frame(1_122_000, 0x7E8, &[0x21, b'T', b'-', b'1', b'2']),
			frame(2_000_000, 0x7E0, &[0x03, 0x22, 0xF4, 0x0D]),
			frame(2_010_000, 0x7E8, &[0x04, 0x62, 0xF4, 0x0D, 0x1E]),
			frame(2_100_000, 0x7E0, &[0x03, 0x22, 0x01, 0x00]),
			frame(2_110_000, 0x7E8, &[0x03, 0x7F, 0x22, 0x22]),
			// A unit that never answers is not in the profile.
			frame(2_200_000, 0x713, &[0x03, 0x22, 0xF1, 0x87]),
		];
		let (profile, taken) = profile(&records, "test");
		assert_eq!(profile.units.len(), 1);
		let engine = &profile.units[0];
		assert_eq!(engine.request, Id(0x7E0));
		assert_eq!(engine.did["F187"], DidSpec::Value(Hex(b"PART-12".to_vec())));
		assert_eq!(
			engine.did["F40D"],
			DidSpec::Full(DidTable {
				series: Some(vec![
					Point {
						at_ms: 10,
						value: Hex(vec![0x00])
					},
					Point {
						at_ms: 1010,
						value: Hex(vec![0x1E])
					}
				]),
				..DidTable::default()
			})
		);
		assert_eq!(
			engine.did["0100"],
			DidSpec::Full(DidTable {
				nrc: Some(0x22),
				..DidTable::default()
			})
		);
		assert_eq!((taken.units, taken.identifiers, taken.series), (1, 2, 1));
		// And it is a profile the sim runs.
		profile.resolve().unwrap();
	}

	#[test]
	fn fault_lists_and_their_records_become_faults() {
		let records = vec![
			frame(0, 0x713, &[0x03, 0x19, 0x02, 0x0C]),
			frame(10, 0x77D, &[0x07, 0x59, 0x02, 0xFF, 0x04, 0x71, 0x20, 0x08]),
			frame(20, 0x713, &[0x06, 0x19, 0x06, 0x04, 0x71, 0x20, 0xFF]),
			frame(30, 0x77D, &[0x07, 0x59, 0x06, 0x04, 0x71, 0x20, 0x08, 0x01]),
		];
		let (profile, taken) = profile(&records, "test");
		assert_eq!(taken.faults, 1);
		assert_eq!(
			profile.units[0].faults,
			vec![FaultSpec {
				code: Hex(vec![0x04, 0x71, 0x20]),
				status: 0x08,
				extended: Some(Hex(vec![0x01])),
				snapshot: None
			}]
		);
	}

	#[test]
	fn a_read_of_several_identifiers_is_counted_not_guessed() {
		let records = vec![
			frame(0, 0x7E0, &[0x05, 0x22, 0xF4, 0x0D, 0xF4, 0x05]),
			frame(10, 0x7E8, &[0x07, 0x62, 0xF4, 0x0D, 0x00, 0xF4, 0x05, 0x50]),
		];
		let (profile, taken) = profile(&records, "test");
		assert_eq!(taken.multi_reads, 1);
		assert!(profile.units[0].did.is_empty());
	}
}
