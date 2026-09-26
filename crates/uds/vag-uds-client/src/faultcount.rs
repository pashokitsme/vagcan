//! Counting the codes a car has stored — the board's fault badge (`todo/dash/20`).
//!
//! What `vagcan faults` reads, without the words: the gateway's installation list
//! ([`gateway::INSTALLATION_LIST`]), the three units it cannot list, in the one order
//! every whole-car walk uses ([`gateway::walk_order`]), and each unit's confirmed codes
//! (`19 02`). Two services, both reads — `0x22` once, `0x19` once a unit — and **no
//! session**: every unit is read in the one it is in, so nothing here changes how a unit
//! behaves and nothing here has to be refused on a moving car.
//!
//! **Sans-IO**, as [`crate::schedule`] is: no clock, no bus. [`FaultCount::next`] says
//! what to send; the shell sends it and hands the answer to [`FaultCount::answered`].
//! One request at a time, in order, until `next` says [`Step::Done`]. On the board each
//! request is one low-priority exchange through the planner, so the panel keeps its reads
//! and a host's requests keep their turn; nothing here owns the bus.
//!
//! What is counted is what `vagcan faults` calls stored and failing now:
//!
//! * **stored** — a code with [`dtc::CONFIRMED`] set. The request asks for exactly those
//!   (status mask `0x08`): ISO 14229-1 reports a code when its status ANDed with the mask
//!   is non-zero, so the answer is the confirmed codes and nothing else — three on the
//!   reference car's body control module, where mask `0xFF` answers 508 (`vagcan
//!   faults` module docs). A unit that ignored the mask would still be counted right: the
//!   bit is checked on every code that comes back.
//! * **failing now** — a stored code with [`dtc::FAILED_NOW`] set too.
//!
//! A unit that does not answer, refuses or answers something that does not parse is left
//! out of the count and named in [`Tally::failed`]; the count is of the units that
//! answered. When the gateway gives no list there is nothing to walk: the outcome is
//! [`Outcome::NoList`], and the board shows no badge.
//!
//! Three guards the laptop's `faults` does not have, because the board runs this at every
//! boot with nobody watching:
//!
//! * **Only VW's block is decoded** ([`gateway::VW_BLOCK_BYTES`]): an answer's length
//!   never decides how much is allocated. Bits past it are counted in
//!   [`Tally::outside_block`].
//! * **A walk of more than [`MAX_UNITS`] is refused** ([`Outcome::TooMany`]): past that it
//!   is a sweep of the block, not a car.
//! * **A listed id that shares an id with a unit already walked is skipped**
//!   ([`Why::SharedId`]): the reference car lists `776` and `777`, which are `70C`'s and
//!   `70D`'s answer ids and would answer on the engine's and the gearbox's request ids.

use alloc::vec::Vec;

use crate::address::UnitAddress;
use crate::dtc::{self, RawDtc};
use crate::gateway;
use crate::pdu::{self, Classified};
use crate::schedule::{Answer, Unit};
use crate::uds::UdsError;

/// ReadDTCInformation.
const READ_DTC: u8 = 0x19;
/// ReadDTCInformation's reportDTCByStatusMask.
const BY_STATUS_MASK: u8 = 0x02;
/// ReadDataByIdentifier.
const READ_DATA: u8 = 0x22;
/// ISO 14229-1: the negative response that asks for more time. The shell waits it out;
/// one that reaches this module is taken as a refusal, as the planner takes it.
const RESPONSE_PENDING: u8 = 0x78;

/// The status mask the units are asked with: the stored codes, and only those.
pub const STORED_MASK: u8 = dtc::CONFIRMED;

/// The most units one count asks. A car lists fifteen to twenty (the reference car:
/// fifteen, eighteen with the three the list never holds). A list that makes the walk
/// longer than this is not a car's but the block's — a gateway answering garbage, or a
/// bitmap with every bit set — and asking every id of it is a sweep of the block with
/// nobody watching, which `CLAUDE.md` guards as it guards `survey`. Refused whole:
/// [`Outcome::TooMany`], no badge.
pub const MAX_UNITS: usize = 40;

/// What to do next.
#[derive(Debug, PartialEq, Eq)]
pub enum Step<'a> {
	/// Send `pdu` to `unit` and hand the answer to [`FaultCount::answered`].
	Ask { unit: Unit, pdu: Vec<u8> },
	/// The count is over.
	Done(&'a Outcome),
}

/// How a count ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
	/// The gateway gave no installation list, so no unit was asked.
	NoList(Why),
	/// The list named more than [`MAX_UNITS`] units to walk, so none was asked.
	TooMany { units: usize },
	/// Every unit of the walk was asked.
	Counted(Tally),
}

/// Why a unit — or the gateway's list — is not in the count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
	/// Nothing came back in the transport's deadline.
	NoAnswer,
	/// The request could not be put on the bus, or the bus failed under it.
	BusError,
	/// A negative response, by its NRC.
	Refused(u8),
	/// An answer that is not a response to what was asked.
	Malformed,
	/// A listed id that is another walked unit's answer id, or whose own answer id is
	/// another walked unit's request id: asking it would put one conversation on another's
	/// ids. Not asked.
	SharedId,
}

/// One unit that answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitTally {
	/// The unit's request id.
	pub request: u16,
	/// Its stored codes.
	pub stored: u32,
	/// Of those, the ones failing now.
	pub failing_now: u32,
}

/// One unit left out of the count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Failed {
	/// The unit's request id.
	pub request: u16,
	pub why: Why,
}

/// The count: what each unit that answered holds, and who did not answer.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tally {
	/// The units that answered, in the order they were asked.
	pub read: Vec<UnitTally>,
	/// The units left out, in the order they were met.
	pub failed: Vec<Failed>,
	/// Bits the list set past VW's block ([`gateway::VW_BLOCK_BYTES`]): counted, never
	/// decoded, never asked.
	pub outside_block: u32,
}

impl Tally {
	/// Stored codes across every unit that answered.
	pub fn stored(&self) -> u32 {
		self.read.iter().map(|u| u.stored).fold(0, u32::saturating_add)
	}

	/// Of those, the ones failing now.
	pub fn failing_now(&self) -> u32 {
		self.read.iter().map(|u| u.failing_now).fold(0, u32::saturating_add)
	}

	/// How many units answered.
	pub fn units_read(&self) -> usize {
		self.read.len()
	}
}

/// One fault count, from the gateway's list to the last unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultCount {
	state: State,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
	/// The installation list is asked for.
	Gateway,
	/// `walk[at]` is asked for its stored codes.
	Units {
		walk: Vec<Unit>,
		at: usize,
		tally: Tally,
	},
	Done(Outcome),
}

impl Default for FaultCount {
	fn default() -> Self {
		Self::new()
	}
}

impl FaultCount {
	/// A count that has asked nothing yet: its first step is the gateway's list.
	pub fn new() -> Self {
		FaultCount { state: State::Gateway }
	}

	/// What to send now, or the outcome. Asking twice without an answer between gives
	/// the same request.
	pub fn next(&self) -> Step<'_> {
		match &self.state {
			State::Gateway => {
				let [high, low] = pdu::did_bytes(gateway::INSTALLATION_LIST);
				Step::Ask {
					unit: address(gateway::GATEWAY).expect("the gateway is in VW's block"),
					pdu: Vec::from([READ_DATA, high, low]),
				}
			}
			State::Units { walk, at, .. } => Step::Ask {
				unit: walk[*at],
				pdu: Vec::from([READ_DTC, BY_STATUS_MASK, STORED_MASK]),
			},
			State::Done(outcome) => Step::Done(outcome),
		}
	}

	/// The answer to the request [`next`](Self::next) gave. Ignored once the count is
	/// done.
	pub fn answered(&mut self, answer: Answer) {
		let state = core::mem::replace(&mut self.state, State::Gateway);
		self.state = match state {
			State::Gateway => match installation_list(answer) {
				Ok((listed, outside_block)) => Self::walk(&listed, outside_block),
				Err(why) => State::Done(Outcome::NoList(why)),
			},
			State::Units { walk, at, mut tally } => {
				let request = walk[at].request;
				match stored_codes(answer) {
					Ok(codes) => tally.read.push(tally_of(request, &codes)),
					Err(why) => tally.failed.push(Failed { request, why }),
				}
				Self::advance(walk, at + 1, tally)
			}
			done @ State::Done(_) => done,
		};
	}

	/// The walk over what the gateway listed, in [`gateway::walk_order`]: a listed id that
	/// would share an id with a unit already in the walk is named and not asked, and a walk
	/// longer than [`MAX_UNITS`] is refused whole.
	///
	/// The walk order puts the three units the list cannot hold first and the listed ids
	/// after them in ascending order, and an answer id is always above its request id
	/// (`+8`, `+0x6A`), so a unit is met before the id it answers on: the first of a
	/// clashing pair is the one kept. The laptop's `faults` asks every listed id as it
	/// stands (`todo/dash/20`).
	fn walk(listed: &[u16], outside_block: u32) -> State {
		let mut tally = Tally {
			outside_block,
			..Tally::default()
		};
		let mut walk: Vec<Unit> = Vec::new();
		for request in gateway::walk_order(listed) {
			// Every id here is in a block — the three, and the bytes of VW's block — so the
			// rule always answers; were it not to, the id is outside and counted so.
			let Some(unit) = address(request) else {
				tally.outside_block = tally.outside_block.saturating_add(1);
				continue;
			};
			let clashes = walk.iter().any(|kept| kept.response == unit.request || kept.request == unit.response);
			if clashes {
				tally.failed.push(Failed { request, why: Why::SharedId });
			} else {
				walk.push(unit);
			}
		}
		if walk.len() > MAX_UNITS {
			return State::Done(Outcome::TooMany { units: walk.len() });
		}
		Self::advance(walk, 0, tally)
	}

	/// `walk[at]` next, or the end.
	fn advance(walk: Vec<Unit>, at: usize, tally: Tally) -> State {
		if at < walk.len() {
			State::Units { walk, at, tally }
		} else {
			State::Done(Outcome::Counted(tally))
		}
	}
}

/// The unit a request id is answered by, by its block's rule.
fn address(request: u16) -> Option<Unit> {
	UnitAddress::from_request(request).map(|a| Unit {
		request: a.request,
		response: a.response,
	})
}

/// The bytes after the echoed service of a positive answer to `sid`, or why there are
/// none.
fn positive(sid: u8, answer: Answer) -> Result<Vec<u8>, Why> {
	match answer {
		Answer::Pdu(pdu) => match pdu::classify_response(sid, &pdu) {
			Ok(Classified::Data(data)) => Ok(data),
			Ok(Classified::Pending) => Err(Why::Refused(RESPONSE_PENDING)),
			Err(UdsError::NegativeResponse { nrc, .. }) => Err(Why::Refused(nrc)),
			Err(_) => Err(Why::Malformed),
		},
		Answer::Refused(nrc) => Err(Why::Refused(nrc)),
		// A read always expects an answer; nothing coming back is silence.
		Answer::NoAnswer | Answer::NotExpected => Err(Why::NoAnswer),
		Answer::BusError => Err(Why::BusError),
	}
}

/// The ids of VW's block the gateway's answer lists, and how many bits it set past that
/// block.
///
/// **Only the block's bytes are decoded**, whatever the answer's length: ISO-TP carries
/// up to 4095 bytes, and a whole answer decoded is up to 32,736 ids — over 130 KB of heap
/// on a board with 72 KB, at every boot. The block's 24 bytes are 192 ids at most; the
/// rest is counted, not allocated.
fn installation_list(answer: Answer) -> Result<(Vec<u16>, u32), Why> {
	let data = positive(READ_DATA, answer)?;
	let bitmap = data.strip_prefix(&pdu::did_bytes(gateway::INSTALLATION_LIST)).ok_or(Why::Malformed)?;
	let (block, past) = bitmap.split_at(bitmap.len().min(gateway::VW_BLOCK_BYTES));
	let outside = past.iter().map(|byte| byte.count_ones()).fold(0, u32::saturating_add);
	Ok((gateway::decode_installation_list(block), outside))
}

/// The codes a unit's `19 02` answer holds.
fn stored_codes(answer: Answer) -> Result<Vec<RawDtc>, Why> {
	let data = positive(READ_DTC, answer)?;
	pdu::parse_dtc_list(&data, BY_STATUS_MASK).map_err(|_| Why::Malformed)
}

/// One unit's stored codes and, of those, the ones failing now — the bits checked on
/// every code, whatever mask the unit honoured.
fn tally_of(request: u16, codes: &[RawDtc]) -> UnitTally {
	let stored = codes.iter().filter(|c| c.status & dtc::CONFIRMED != 0);
	UnitTally {
		request,
		stored: stored.clone().count() as u32,
		failing_now: stored.filter(|c| c.status & dtc::FAILED_NOW != 0).count() as u32,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::collections::BTreeMap;
	use alloc::vec;

	/// How a fake unit answers whatever it is asked.
	#[derive(Clone)]
	enum Reply {
		/// A positive `59 02 FF` answer holding these `(code, status)` records.
		Codes(Vec<([u8; 3], u8)>),
		/// Exactly this answer.
		Answer(Answer),
	}

	/// A car behind a fake link: the gateway's reply to its list, and each unit's reply
	/// to `19 02`. A unit not in `units` does not answer.
	struct Car {
		gateway: Answer,
		units: BTreeMap<u16, Reply>,
	}

	impl Car {
		fn listing(listed: &[u16]) -> Self {
			Car {
				gateway: Answer::Pdu(list_answer(listed)),
				units: BTreeMap::new(),
			}
		}

		fn with(mut self, request: u16, reply: Reply) -> Self {
			self.units.insert(request, reply);
			self
		}

		fn answer(&self, unit: Unit, pdu: &[u8]) -> Answer {
			if unit.request == gateway::GATEWAY && pdu.first() == Some(&READ_DATA) {
				return self.gateway.clone();
			}
			match self.units.get(&unit.request) {
				None => Answer::NoAnswer,
				Some(Reply::Answer(answer)) => answer.clone(),
				Some(Reply::Codes(codes)) => {
					let mut out = vec![0x59, 0x02, 0xFF];
					for (code, status) in codes {
						out.extend_from_slice(code);
						out.push(*status);
					}
					Answer::Pdu(out)
				}
			}
		}
	}

	/// `62 2A 26` and a bitmap with a bit for each id.
	fn list_answer(listed: &[u16]) -> Vec<u8> {
		let mut out = vec![0x62, 0x2A, 0x26];
		let mut bitmap = [0u8; 32];
		for id in listed {
			let n = usize::from(id - 0x700);
			bitmap[n / 8] |= 1 << (n % 8);
		}
		out.extend_from_slice(&bitmap);
		out
	}

	/// Run a count to its end against `car`: the outcome, and every request in order.
	fn run(car: &Car) -> (Outcome, Vec<(Unit, Vec<u8>)>) {
		let mut count = FaultCount::new();
		let mut asked = Vec::new();
		loop {
			match count.next() {
				Step::Ask { unit, pdu } => {
					let answer = car.answer(unit, &pdu);
					asked.push((unit, pdu));
					count.answered(answer);
					assert!(asked.len() < 300, "the count never ends");
				}
				Step::Done(outcome) => return (outcome.clone(), asked),
			}
		}
	}

	fn counted(outcome: Outcome) -> Tally {
		match outcome {
			Outcome::Counted(tally) => tally,
			other => panic!("not counted: {other:?}"),
		}
	}

	fn unit(request: u16) -> Unit {
		let a = UnitAddress::from_request(request).unwrap();
		Unit {
			request: a.request,
			response: a.response,
		}
	}

	#[test]
	fn the_gateway_is_asked_first_for_its_installation_list() {
		let count = FaultCount::new();
		assert_eq!(
			count.next(),
			Step::Ask {
				unit: Unit {
					request: 0x710,
					response: 0x77A
				},
				pdu: vec![0x22, 0x2A, 0x26]
			}
		);
		// Asked again before an answer: the same request, not the next one.
		assert_eq!(count.next(), count.next());
	}

	#[test]
	fn every_unit_of_the_walk_is_asked_for_its_stored_codes_in_order() {
		let (_, asked) = run(&Car::listing(&[0x70C, 0x714]));
		let units: Vec<Unit> = asked.iter().skip(1).map(|(u, _)| *u).collect();
		assert_eq!(units, vec![unit(0x7E0), unit(0x7E1), unit(0x710), unit(0x70C), unit(0x714)]);
		// Each by its block's rule: the engine on 7E8, the cluster on 77E.
		assert_eq!(units[0].response, 0x7E8);
		assert_eq!(units[4].response, 0x77E);
		for (_, pdu) in asked.iter().skip(1) {
			assert_eq!(pdu, &vec![0x19, 0x02, 0x08]);
		}
	}

	#[test]
	fn only_reads_are_ever_sent_and_no_session_is_entered() {
		let car = Car::listing(&[0x70C, 0x714, 0x746]).with(0x7E0, Reply::Codes(vec![([0, 1, 2], 0x09)]));
		let (_, asked) = run(&car);
		for (_, pdu) in &asked {
			assert!(crate::check_read_only(pdu).is_ok(), "{pdu:02X?}");
			assert!(matches!(pdu[0], 0x22 | 0x19), "{pdu:02X?} is neither a read nor a fault read");
		}
		assert_eq!(asked.iter().filter(|(_, pdu)| pdu[0] == 0x22).count(), 1, "the list is read once");
	}

	#[test]
	fn stored_is_confirmed_and_failing_now_is_confirmed_and_failing() {
		let car = Car::listing(&[0x714])
			.with(0x7E0, Reply::Codes(vec![([0, 1, 0x29], 0x08), ([0, 2, 0], 0x09), ([0, 3, 0], 0x2F)]))
			.with(0x7E1, Reply::Codes(vec![]))
			.with(0x710, Reply::Codes(vec![]))
			// A unit that ignored the mask: a code only pending, one failing but not
			// confirmed. Neither is stored, so neither is counted.
			.with(0x714, Reply::Codes(vec![([4, 0x71, 0x20], 0x04), ([1, 1, 1], 0x01), ([2, 2, 2], 0x88)]));
		let tally = counted(run(&car).0);
		assert_eq!(tally.stored(), 4);
		assert_eq!(tally.failing_now(), 2);
		assert_eq!(tally.units_read(), 4);
		assert!(tally.failed.is_empty());
		assert_eq!(
			tally.read[0],
			UnitTally {
				request: 0x7E0,
				stored: 3,
				failing_now: 2
			}
		);
		assert_eq!(
			tally.read[3],
			UnitTally {
				request: 0x714,
				stored: 1,
				failing_now: 0
			}
		);
	}

	#[test]
	fn a_unit_that_does_not_answer_is_skipped_and_named() {
		let car = Car::listing(&[0x714, 0x746])
			.with(0x7E0, Reply::Codes(vec![([0, 1, 0], 0x08)]))
			.with(0x7E1, Reply::Answer(Answer::Pdu(vec![0x7F, 0x19, 0x11])))
			.with(0x710, Reply::Answer(Answer::BusError))
			.with(0x714, Reply::Answer(Answer::Pdu(vec![0x59, 0x02, 0xFF, 0x00, 0x01])))
			.with(0x746, Reply::Answer(Answer::Refused(0x22)));
		let (outcome, asked) = run(&car);
		assert_eq!(asked.len(), 6, "every unit asked once, none twice");
		let tally = counted(outcome);
		assert_eq!(tally.stored(), 1);
		assert_eq!(tally.units_read(), 1);
		assert_eq!(
			tally.failed,
			vec![
				Failed {
					request: 0x7E1,
					why: Why::Refused(0x11)
				},
				Failed {
					request: 0x710,
					why: Why::BusError
				},
				Failed {
					request: 0x714,
					why: Why::Malformed
				},
				Failed {
					request: 0x746,
					why: Why::Refused(0x22)
				},
			]
		);
	}

	#[test]
	fn silence_a_pending_that_leaks_and_the_wrong_answer_are_each_named() {
		let car = Car::listing(&[])
			.with(0x7E1, Reply::Answer(Answer::Pdu(vec![0x7F, 0x19, 0x78])))
			// The answer to another subfunction.
			.with(0x710, Reply::Answer(Answer::Pdu(vec![0x59, 0x0A, 0xFF])));
		let tally = counted(run(&car).0);
		assert_eq!(
			tally.failed,
			vec![
				Failed {
					request: 0x7E0,
					why: Why::NoAnswer
				},
				Failed {
					request: 0x7E1,
					why: Why::Refused(0x78)
				},
				Failed {
					request: 0x710,
					why: Why::Malformed
				},
			]
		);
		assert_eq!(tally.stored(), 0);
		assert_eq!(tally.units_read(), 0);
	}

	#[test]
	fn a_gateway_with_no_list_ends_the_count_before_any_unit_is_asked() {
		for (answer, why) in [
			(Answer::NoAnswer, Why::NoAnswer),
			(Answer::BusError, Why::BusError),
			(Answer::Pdu(vec![0x7F, 0x22, 0x31]), Why::Refused(0x31)),
			(Answer::Refused(0x33), Why::Refused(0x33)),
			// The list of another identifier.
			(Answer::Pdu(vec![0x62, 0x04, 0xA3, 0x01]), Why::Malformed),
			(Answer::Pdu(vec![0x62]), Why::Malformed),
		] {
			let car = Car {
				gateway: answer.clone(),
				units: BTreeMap::new(),
			};
			let (outcome, asked) = run(&car);
			assert_eq!(outcome, Outcome::NoList(why), "{answer:?}");
			assert_eq!(asked.len(), 1, "{answer:?}: no unit asked");
		}
	}

	#[test]
	fn an_empty_list_still_reads_the_three_units_it_cannot_hold() {
		let (outcome, asked) = run(&Car::listing(&[]));
		assert_eq!(asked.len(), 4);
		assert_eq!(counted(outcome).failed.len(), 3, "none of them answers this car");
	}

	#[test]
	fn bits_past_vws_block_are_counted_and_never_decoded() {
		// 0x7C0 is in neither block, and 0x7E0's bit would name the engine by a
		// second road: past byte 24 nothing is an id this walk addresses by VW's rule.
		let mut gateway = list_answer(&[0x714]);
		gateway[3 + (0x7C0 - 0x700) / 8] |= 1;
		gateway[3 + (0x7E0 - 0x700) / 8] |= 1;
		let car = Car {
			gateway: Answer::Pdu(gateway),
			units: BTreeMap::new(),
		};
		let (outcome, asked) = run(&car);
		let units: Vec<u16> = asked.iter().skip(1).map(|(u, _)| u.request).collect();
		assert_eq!(units, vec![0x7E0, 0x7E1, 0x710, 0x714], "each once, nothing past the block");
		let tally = counted(outcome);
		assert_eq!(tally.outside_block, 2);
		assert_eq!(tally.failed.len(), 4, "the four silent units, and nothing else named");
	}

	#[test]
	fn an_answer_as_long_as_iso_tp_allows_decodes_no_more_than_the_block() {
		// 4095 bytes is the longest PDU ISO-TP carries: decoded whole, 32,736 ids — over
		// 130 KB of heap on a board that has 72 KB, every boot. Only VW's block is read:
		// 192 ids at most, and a list that long is refused as a sweep (below).
		let mut pdu = vec![0x62, 0x2A, 0x26];
		pdu.resize(4095, 0xFF);
		let car = Car {
			gateway: Answer::Pdu(pdu),
			units: BTreeMap::new(),
		};
		let (outcome, asked) = run(&car);
		assert_eq!(asked.len(), 1, "no unit asked");
		match outcome {
			Outcome::TooMany { units } => assert!(units > MAX_UNITS && units <= 192 + 3, "{units}"),
			other => panic!("not refused: {other:?}"),
		}
		assert_eq!(gateway::VW_BLOCK_BYTES, 24, "0x700..=0x7BF, a bit a unit");
	}

	/// `n` listed ids of VW's block with no id another's response, and not the gateway.
	fn plain_ids(n: usize) -> Vec<u16> {
		(0x700u16..0x76A).filter(|id| *id != gateway::GATEWAY).take(n).collect()
	}

	#[test]
	fn a_walk_of_more_than_forty_units_is_refused_as_a_sweep() {
		// Forty: three the list cannot hold and thirty-seven listed.
		let (outcome, asked) = run(&Car::listing(&plain_ids(MAX_UNITS - 3)));
		assert_eq!(counted(outcome).failed.len(), MAX_UNITS, "forty asked, all silent here");
		assert_eq!(asked.len(), 1 + MAX_UNITS);

		// One more, and nothing is asked past the list.
		let (outcome, asked) = run(&Car::listing(&plain_ids(MAX_UNITS - 2)));
		assert_eq!(outcome, Outcome::TooMany { units: MAX_UNITS + 1 });
		assert_eq!(asked.len(), 1);
	}

	#[test]
	fn a_listed_id_that_is_another_units_response_id_is_skipped_and_named() {
		// 0x776 is 0x70C's answer id: asked, it would be sent on an id 0x70C answers on.
		// 0x777, with 0x70D not listed, is nobody's answer id here — but its own would be
		// 0x7E1, the gearbox's request: it would be listened for on an id the gearbox is
		// asked on. Both are the pairs the reference car lists (`gateway` module docs).
		let car = Car::listing(&[0x70C, 0x776, 0x777]);
		let (outcome, asked) = run(&car);
		let units: Vec<u16> = asked.iter().skip(1).map(|(u, _)| u.request).collect();
		assert_eq!(units, vec![0x7E0, 0x7E1, 0x710, 0x70C]);
		let tally = counted(outcome);
		for request in [0x776, 0x777] {
			assert!(tally.failed.contains(&Failed { request, why: Why::SharedId }), "{request:03X}");
		}
		// And no request goes out on, or listens on, an id another unit of the walk uses.
		for (unit, _) in &asked {
			for (other, _) in &asked {
				if unit != other {
					assert!(unit.request != other.response && unit.response != other.request, "{unit:?} / {other:?}");
				}
			}
		}
	}

	#[test]
	fn an_answer_after_the_end_changes_nothing() {
		let car = Car::listing(&[]).with(0x7E0, Reply::Codes(vec![([0, 1, 0], 0x08)]));
		let mut count = FaultCount::new();
		while let Step::Ask { unit, pdu } = count.next() {
			count.answered(car.answer(unit, &pdu));
		}
		let before = count.clone();
		count.answered(Answer::Pdu(vec![0x59, 0x02, 0xFF, 9, 9, 9, 0x08]));
		assert_eq!(count, before);
		let Step::Done(Outcome::Counted(tally)) = count.next() else {
			panic!("not done")
		};
		assert_eq!(tally.stored(), 1);
	}
}
