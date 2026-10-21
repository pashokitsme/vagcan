//! What a unit answers to one whole request: the diagnostic server, at the PDU level.
//!
//! Every service the client's allowlist holds is answered — `22`, `19`, `10`, `3E` — and
//! any other with `7F <sid> 11` (service not supported), as a real unit would. The sim does
//! not police the tester: the client's allowlist is what decides what is asked, and the sim
//! answers whatever arrives.

use std::time::Duration;

use crate::profile::{Car, Unit};

/// Negative response codes this module sends (ISO 14229-1 annex A.1).
const SERVICE_NOT_SUPPORTED: u8 = 0x11;
const SUB_FUNCTION_NOT_SUPPORTED: u8 = 0x12;
const INCORRECT_LENGTH: u8 = 0x13;
const RESPONSE_TOO_LONG: u8 = 0x14;
const REQUEST_OUT_OF_RANGE: u8 = 0x31;
const RESPONSE_PENDING: u8 = 0x78;

/// The longest PDU classic ISO-TP carries (ISO 15765-2: a 12-bit length).
const ISOTP_MAX: usize = 4095;

/// The answer to one request: what to send, each after its delay from the request's
/// arrival, or nothing at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
	Silent(Why),
	Answers(Vec<(Duration, Vec<u8>)>),
}

/// Why a unit said nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
	/// The profile has it silent.
	Silent,
	/// It went quiet after `quiet_after` requests or `quiet_after_ms`.
	Quiet,
	/// An identifier asked for went quiet after `silent_after` reads.
	DidQuiet(u16),
	/// `3E 80`: tester present with the answer suppressed.
	Suppressed,
	/// An empty request.
	Empty,
}

/// The units of one car, and what each has been asked since the channel opened.
pub struct Server {
	car: Car,
	state: Vec<UnitState>,
}

#[derive(Default, Clone)]
struct UnitState {
	requests: u32,
	reads: std::collections::BTreeMap<u16, u32>,
}

impl Server {
	pub fn new(car: Car) -> Server {
		let state = vec![UnitState::default(); car.units.len()];
		Server { car, state }
	}

	pub fn car(&self) -> &Car {
		&self.car
	}

	/// Forget what was asked: a new session, a car started again.
	pub fn reset(&mut self) {
		self.state.iter_mut().for_each(|s| *s = UnitState::default());
	}

	/// The answer of unit `unit` (an index into the car's units) to `request`, `since`
	/// after the channel opened.
	pub fn answer(&mut self, unit: usize, since: Duration, request: &[u8]) -> Reply {
		let spec = &self.car.units[unit];
		let state = &mut self.state[unit];
		state.requests += 1;
		if spec.silent {
			return Reply::Silent(Why::Silent);
		}
		let quiet_by_count = spec.quiet_after.is_some_and(|n| state.requests > n);
		let quiet_by_time = spec.quiet_after_ms.is_some_and(|t| since >= t);
		if quiet_by_count || quiet_by_time {
			return Reply::Silent(Why::Quiet);
		}
		let Some(&sid) = request.first() else {
			return Reply::Silent(Why::Empty);
		};
		let body = &request[1..];
		let mut reply = match sid {
			0x22 => read(spec, state, since, body),
			0x19 => dtc(spec, body),
			0x10 => session(spec, body),
			0x3E => match body {
				[0x80] => return Reply::Silent(Why::Suppressed),
				[0x00] => answer(vec![0x7E, 0x00]),
				_ => refuse(sid, SUB_FUNCTION_NOT_SUPPORTED),
			},
			_ => refuse(sid, SERVICE_NOT_SUPPORTED),
		};
		if let Reply::Answers(answers) = &mut reply {
			for (after, _) in answers.iter_mut() {
				*after += spec.delay;
			}
		}
		reply
	}
}

fn answer(pdu: Vec<u8>) -> Reply {
	Reply::Answers(vec![(Duration::ZERO, pdu)])
}

fn refuse(sid: u8, nrc: u8) -> Reply {
	answer(vec![0x7F, sid, nrc])
}

/// `22`: one identifier or several. The answer carries every identifier the unit has, in
/// the order asked; one it lacks is left out, and a request it has none of is refused with
/// `31`. An identifier the profile refuses refuses the whole request with its code; the
/// longest `pending` and `delay_ms` among those asked apply to the answer.
fn read(unit: &Unit, state: &mut UnitState, since: Duration, body: &[u8]) -> Reply {
	if body.is_empty() || body.len() % 2 != 0 {
		return refuse(0x22, INCORRECT_LENGTH);
	}
	let mut pdu = vec![0x62];
	let (mut pending, mut gap, mut delay) = (0u32, Duration::ZERO, Duration::ZERO);
	for pair in body.chunks(2) {
		let number = u16::from_be_bytes([pair[0], pair[1]]);
		let Some(did) = unit.dids.get(&number) else { continue };
		let reads = state.reads.entry(number).or_default();
		*reads += 1;
		if did.silent_after.is_some_and(|n| *reads > n) {
			return Reply::Silent(Why::DidQuiet(number));
		}
		match did.reads.at(since) {
			Ok(bytes) => {
				pdu.extend_from_slice(pair);
				pdu.extend_from_slice(bytes);
			}
			Err(nrc) => return refuse(0x22, nrc),
		}
		if did.pending > pending {
			(pending, gap) = (did.pending, did.pending_gap);
		}
		delay = delay.max(did.delay);
	}
	if pdu.len() == 1 {
		return refuse(0x22, REQUEST_OUT_OF_RANGE);
	}
	if pdu.len() > ISOTP_MAX {
		return refuse(0x22, RESPONSE_TOO_LONG);
	}
	let mut answers: Vec<(Duration, Vec<u8>)> = (0..pending).map(|n| (delay + gap * n, vec![0x7F, 0x22, RESPONSE_PENDING])).collect();
	answers.push((delay + gap * pending, pdu));
	Reply::Answers(answers)
}

/// `19`: the sub-functions a fault reader asks — `01` count, `02` list by status mask,
/// `04` snapshot, `06` extended data, `0A` every supported code. Others are refused `12`.
fn dtc(unit: &Unit, body: &[u8]) -> Reply {
	let avail = unit.dtc_availability;
	let stored = |code: [u8; 3]| unit.faults.iter().find(|f| f.code == code);
	match body {
		[0x01, mask] => {
			let count = unit.faults.iter().filter(|f| f.status & mask != 0).count() as u16;
			// Format 01: ISO 14229-1 DTC format.
			answer(vec![0x59, 0x01, avail, 0x01, (count >> 8) as u8, count as u8])
		}
		[0x02, mask] => {
			let mut pdu = vec![0x59, 0x02, avail];
			for fault in unit.faults.iter().filter(|f| f.status & mask != 0) {
				pdu.extend_from_slice(&fault.code);
				pdu.push(fault.status);
			}
			answer(pdu)
		}
		[0x0A] => {
			let mut pdu = vec![0x59, 0x0A, avail];
			for (code, status) in &unit.supported {
				pdu.extend_from_slice(code);
				pdu.push(*status);
			}
			answer(pdu)
		}
		[sub @ (0x04 | 0x06), a, b, c, _record] => {
			let code = [*a, *b, *c];
			let Some(fault) = stored(code) else {
				return refuse(0x19, REQUEST_OUT_OF_RANGE);
			};
			let mut pdu = vec![0x59, *sub, *a, *b, *c, fault.status];
			let records = if *sub == 0x04 { &fault.snapshot } else { &fault.extended };
			pdu.extend_from_slice(records.as_deref().unwrap_or_default());
			answer(pdu)
		}
		[0x01 | 0x02 | 0x04 | 0x06 | 0x0A, ..] => refuse(0x19, INCORRECT_LENGTH),
		_ => refuse(0x19, SUB_FUNCTION_NOT_SUPPORTED),
	}
}

/// `10`: a session the unit has, with ISO 14229-1's timing record (P2 50 ms, P2* 5 s).
fn session(unit: &Unit, body: &[u8]) -> Reply {
	match body {
		[session] if unit.sessions.contains(&(session & 0x7F)) => match session & 0x80 {
			0 => answer(vec![0x50, *session, 0x00, 0x32, 0x01, 0xF4]),
			_ => Reply::Silent(Why::Suppressed),
		},
		[_] => refuse(0x10, SUB_FUNCTION_NOT_SUPPORTED),
		_ => refuse(0x10, INCORRECT_LENGTH),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::profile::Profile;

	fn server(text: &str) -> Server {
		Server::new(Profile::parse(text).unwrap().resolve().unwrap())
	}

	const T0: Duration = Duration::ZERO;

	fn one(reply: Reply) -> Vec<u8> {
		match reply {
			Reply::Answers(answers) if answers.len() == 1 => answers[0].1.clone(),
			other => panic!("{other:?}"),
		}
	}

	#[test]
	fn a_read_answers_what_the_profile_has_and_refuses_what_it_lacks() {
		let mut s = server("[[unit]]\nrequest = \"7E0\"\npart = \"AB\"\n[unit.did]\nF40D = \"1E\"\n");
		assert_eq!(one(s.answer(0, T0, &[0x22, 0xF1, 0x87])), vec![0x62, 0xF1, 0x87, b'A', b'B']);
		assert_eq!(one(s.answer(0, T0, &[0x22, 0xF1, 0x88])), vec![0x7F, 0x22, 0x31]);
		// Several at once: the ones it has, in the order asked.
		assert_eq!(
			one(s.answer(0, T0, &[0x22, 0xF4, 0x0D, 0xF1, 0x88, 0xF1, 0x87])),
			vec![0x62, 0xF4, 0x0D, 0x1E, 0xF1, 0x87, b'A', b'B']
		);
		assert_eq!(one(s.answer(0, T0, &[0x22, 0xF4])), vec![0x7F, 0x22, 0x13]);
	}

	#[test]
	fn pending_answers_come_first_then_the_answer_after_the_delays() {
		let mut s = server("[[unit]]\nrequest = \"7E0\"\ndelay_ms = 5\n[unit.did]\n0100 = { value = \"01\", pending = 2, pending_ms = 100 }\n");
		let Reply::Answers(answers) = s.answer(0, T0, &[0x22, 0x01, 0x00]) else {
			panic!()
		};
		let ms = |n| Duration::from_millis(n);
		assert_eq!(
			answers,
			vec![
				(ms(5), vec![0x7F, 0x22, 0x78]),
				(ms(105), vec![0x7F, 0x22, 0x78]),
				(ms(205), vec![0x62, 0x01, 0x00, 0x01])
			]
		);
	}

	#[test]
	fn a_refused_identifier_refuses_with_its_code() {
		let mut s = server("[[unit]]\nrequest = \"7E0\"\n[unit.did]\n0100 = { nrc = 0x22 }\n");
		assert_eq!(one(s.answer(0, T0, &[0x22, 0x01, 0x00])), vec![0x7F, 0x22, 0x22]);
	}

	#[test]
	fn silence_after_n_reads_and_quiet_after_n_requests_or_a_time() {
		let mut s = server("[[unit]]\nrequest = \"7E0\"\n[unit.did]\n0100 = { value = \"01\", silent_after = 1 }\n");
		assert!(matches!(s.answer(0, T0, &[0x22, 0x01, 0x00]), Reply::Answers(_)));
		assert_eq!(s.answer(0, T0, &[0x22, 0x01, 0x00]), Reply::Silent(Why::DidQuiet(0x0100)));
		s.reset();
		assert!(
			matches!(s.answer(0, T0, &[0x22, 0x01, 0x00]), Reply::Answers(_)),
			"a new session counts again"
		);

		let mut s = server("[[unit]]\nrequest = \"7E0\"\nquiet_after = 1\n[unit.did]\n0100 = \"01\"\n");
		assert!(matches!(s.answer(0, T0, &[0x22, 0x01, 0x00]), Reply::Answers(_)));
		assert_eq!(s.answer(0, T0, &[0x3E, 0x00]), Reply::Silent(Why::Quiet));

		let mut s = server("[[unit]]\nrequest = \"7E0\"\nquiet_after_ms = 1000\n[unit.did]\n0100 = \"01\"\n");
		assert!(matches!(s.answer(0, Duration::from_millis(999), &[0x22, 0x01, 0x00]), Reply::Answers(_)));
		assert_eq!(s.answer(0, Duration::from_millis(1000), &[0x22, 0x01, 0x00]), Reply::Silent(Why::Quiet));

		let mut s = server("[[unit]]\nrequest = \"70E\"\nsilent = true\n");
		assert_eq!(s.answer(0, T0, &[0x22, 0xF1, 0x87]), Reply::Silent(Why::Silent));
	}

	#[test]
	fn faults_by_mask_with_their_records() {
		let mut s = server(
			"[[unit]]\nrequest = \"7E0\"\nsupported = [\"00 01 07\"]\n\
             [[unit.fault]]\ncode = \"00 01 29\"\nstatus = 0x09\nextended = \"01 06 03\"\nsnapshot = \"01 01 F4 0D 00\"\n\
             [[unit.fault]]\ncode = \"04 71 20\"\nstatus = 0x10\n",
		);
		assert_eq!(one(s.answer(0, T0, &[0x19, 0x02, 0x08])), vec![0x59, 0x02, 0xFF, 0x00, 0x01, 0x29, 0x09]);
		assert_eq!(
			one(s.answer(0, T0, &[0x19, 0x02, 0xFF])),
			vec![0x59, 0x02, 0xFF, 0x00, 0x01, 0x29, 0x09, 0x04, 0x71, 0x20, 0x10]
		);
		assert_eq!(one(s.answer(0, T0, &[0x19, 0x01, 0xFF])), vec![0x59, 0x01, 0xFF, 0x01, 0x00, 0x02]);
		assert_eq!(
			one(s.answer(0, T0, &[0x19, 0x06, 0x00, 0x01, 0x29, 0xFF])),
			vec![0x59, 0x06, 0x00, 0x01, 0x29, 0x09, 0x01, 0x06, 0x03]
		);
		assert_eq!(
			one(s.answer(0, T0, &[0x19, 0x04, 0x00, 0x01, 0x29, 0xFF])),
			vec![0x59, 0x04, 0x00, 0x01, 0x29, 0x09, 0x01, 0x01, 0xF4, 0x0D, 0x00]
		);
		assert_eq!(
			one(s.answer(0, T0, &[0x19, 0x06, 0x04, 0x71, 0x20, 0xFF])),
			vec![0x59, 0x06, 0x04, 0x71, 0x20, 0x10]
		);
		assert_eq!(one(s.answer(0, T0, &[0x19, 0x06, 0x09, 0x09, 0x09, 0xFF])), vec![0x7F, 0x19, 0x31]);
		assert_eq!(
			one(s.answer(0, T0, &[0x19, 0x0A])),
			vec![0x59, 0x0A, 0xFF, 0x00, 0x01, 0x29, 0x09, 0x04, 0x71, 0x20, 0x10, 0x00, 0x01, 0x07, 0x00]
		);
		assert_eq!(one(s.answer(0, T0, &[0x19, 0x14])), vec![0x7F, 0x19, 0x12]);
	}

	#[test]
	fn sessions_tester_present_and_services_outside_the_set() {
		let mut s = server("[[unit]]\nrequest = \"7E0\"\n");
		assert_eq!(one(s.answer(0, T0, &[0x10, 0x03])), vec![0x50, 0x03, 0x00, 0x32, 0x01, 0xF4]);
		assert_eq!(one(s.answer(0, T0, &[0x10, 0x02])), vec![0x7F, 0x10, 0x12]);
		assert_eq!(one(s.answer(0, T0, &[0x3E, 0x00])), vec![0x7E, 0x00]);
		assert_eq!(s.answer(0, T0, &[0x3E, 0x80]), Reply::Silent(Why::Suppressed));
		assert_eq!(one(s.answer(0, T0, &[0x2E, 0xF1, 0x90, 0x00])), vec![0x7F, 0x2E, 0x11]);
	}
}
