//! A refused part number, which used to strike a unit off for the boot (found by a code read on
//! 2026-10-20, the likely cause of a car's ESC (`713`) showing dashes boot after boot; reproduced
//! here, not yet seen on the car). Ids and bytes are synthetic. Three ways in: a matched unit that
//! goes quiet and whose late refusal is taken as `F187`'s, a unit that refuses `F187` at boot
//! (with `22` while it starts up, say), and one that answers it with no number yet.
//!
//! What the board did: one read of the unit's identifier went unanswered for the answer
//! timeout, so the unit was taken for silent, its subscriptions were dropped and `F187` asked
//! again. The unit's refusal of the identifier read — `7F 22 22`, later than the timeout — then
//! came while `22 F1 87` was waiting, and a negative response names only the service:
//! [`answers`] takes it as `F187`'s ([`Waits::heard`] is the shell's rule). A refused part
//! number was a mismatch, and a mismatch is never polled again this run.
//!
//! What it does now: the unit is taken for silent only after [`SILENT_AFTER`] unanswered reads
//! in a row, and a refused `F187` or one with no number in it — before the unit's first match or
//! after it — is asked again after the backoff's cap, not struck off.
//!
//! Every decision here is the board's own: [`UnitCheck`] is the type `PanelReads` in
//! `vag-dash-fw`'s `dash.rs` holds per unit, fed by [`Unit::check_part`](PlanUnit::check_part)
//! through [`part_answer`]. What this test adds is the planner's side — which delivery goes to
//! which call, and doing the [`Action`] — as `PanelReads::take` does it.

use vag_dash_core::exchange::{Heard, RESPONSE_TIMEOUT_MS, Timeouts, Waits};
use vag_dash_core::unit_check::{Action, Check, SILENT_AFTER, UnitCheck, part_answer};
use vag_dash_render::plan::Unit as PlanUnit;
use vag_uds_client::schedule::{Answer, Budget, Class, Delivery, Next, Planner, ReqId, SubId, Unit, answers};

const UNIT: Unit = Unit {
	request: 0x7E1,
	response: 0x7E9,
};
const PLANNED: PlanUnit = PlanUnit {
	request: 0x7E1,
	response: 0x7E9,
	part_number: "PART1",
};
const DID: u16 = 0x3800;
const PART_NUMBER: u16 = 0xF187;
const PERIOD_MS: u32 = 100;
const TIMEOUTS: Timeouts = Timeouts {
	answer_ms: RESPONSE_TIMEOUT_MS,
	tail_ms: 10_000,
	suppressed_ms: 150,
	pending_wait_ms: 5_000,
	pending_deadline_ms: 10_000,
};

/// The board's panel for one unit with one channel: the unit's check, its subscription and its
/// part-number read out.
struct Panel {
	unit: UnitCheck,
	sub: Option<SubId>,
	part_read: Option<ReqId>,
}

impl Panel {
	fn new(p: &mut Planner) -> Self {
		let mut panel = Panel {
			unit: UnitCheck::NEW,
			sub: None,
			part_read: None,
		};
		panel.ask(p, 0);
		panel
	}

	fn ask(&mut self, p: &mut Planner, now: u64) {
		self.part_read = Some(p.read_once(now, Class::Foreground, UNIT, PART_NUMBER));
	}

	fn retry_when_due(&mut self, p: &mut Planner, now: u64) {
		if self.unit.take_due(now) {
			self.ask(p, now);
		}
	}

	/// When the bus loop wakes next: the planner's time or the unit's retry, whichever is first.
	fn wake(&self, now: u64, until_ms: Option<u64>) -> u64 {
		let retry = self.unit.retry_in_ms(now).map(|left| now + left);
		[until_ms, retry].into_iter().flatten().min().unwrap_or(now + 10).max(now + 1)
	}

	fn act(&mut self, p: &mut Planner, action: Action, now: u64) {
		match action {
			Action::None => {}
			Action::Subscribe => self.sub = Some(p.subscribe(now, Class::Foreground, UNIT, DID, PERIOD_MS, None)),
			Action::Ask => self.ask(p, now),
			Action::DropAndAsk => {
				p.unsubscribe(self.sub.take().expect("a matched unit is subscribed"));
				self.ask(p, now);
			}
		}
	}

	fn take(&mut self, p: &mut Planner, delivery: Delivery) {
		let (action, at_ms) = match delivery {
			Delivery::Once { req, result, at_ms, .. } if Some(req) == self.part_read => {
				self.part_read = None;
				let part = PLANNED.check_part(part_answer(result.as_deref()));
				(self.unit.on_part(part, at_ms), at_ms)
			}
			Delivery::Reading { sub, at_ms, .. } if Some(sub) == self.sub => (self.unit.on_reading(at_ms), at_ms),
			Delivery::Missed { sub, why, at_ms, .. } if Some(sub) == self.sub => (self.unit.on_miss(why, at_ms), at_ms),
			other => panic!("not the panel's: {other:?}"),
		};
		self.act(p, action, at_ms);
	}
}

fn positive(did: u16, data: &[u8]) -> Vec<u8> {
	let mut pdu = vec![0x62];
	pdu.extend_from_slice(&did.to_be_bytes());
	pdu.extend_from_slice(data);
	pdu
}

/// The whole story, through the board's planner: matched, quiet for three reads, the late
/// refusal taken as `F187`'s, asked again after the cap, matched, read again.
#[test]
fn a_matched_unit_that_goes_quiet_and_refuses_its_part_number_late_is_asked_again_and_read_again() {
	let mut p = Planner::new(Budget::board());
	let mut panel = Panel::new(&mut p);
	let cap = u64::from(Budget::board().backoff_cap_ms);
	// What the unit does: answers, then is quiet for `SILENT_AFTER` reads of the identifier,
	// then its refusal of one of them comes — late, while `F187` is out — then it refuses `F187`
	// once more on its own, then answers again.
	#[derive(Debug, PartialEq)]
	enum Phase {
		Answering,
		Quiet(u8),
		LateRefusal,
		Refusing,
		Back,
	}
	let mut phase = Phase::Answering;
	let (mut now, mut readings_back, mut part_sends) = (0u64, 0u32, Vec::new());
	let mut late_refusal_taken = false;
	while now < 30_000 && readings_back < 3 {
		panel.retry_when_due(&mut p, now);
		match p.due(now) {
			Next::Send(out) => {
				let is_part = out.pdu == [0x22, 0xF1, 0x87];
				if is_part {
					part_sends.push(now);
				}
				let (answer, cost) = match (&phase, is_part) {
					(Phase::Answering | Phase::Back, true) => (Answer::Pdu(positive(PART_NUMBER, b"PART1 ")), 10),
					(Phase::Answering, false) => (Answer::Pdu(positive(DID, &[0x2A])), 10),
					(Phase::Quiet(_), false) => (Answer::NoAnswer, RESPONSE_TIMEOUT_MS),
					(Phase::LateRefusal, true) => {
						// The shell's own rule takes the identifier read's late refusal as this
						// request's answer: nothing in it says which `22` it refuses.
						let late = [0x7F, 0x22, 0x22];
						let mut waits = Waits::new(&out.pdu, TIMEOUTS, None);
						assert_eq!(waits.heard(&late, 300), Heard::Answer, "a late 7F 22 is any 22's answer");
						assert!(answers(&out.pdu, &late));
						late_refusal_taken = true;
						(Answer::Pdu(late.to_vec()), 300)
					}
					(Phase::Refusing, true) => (Answer::Pdu(vec![0x7F, 0x22, 0x22]), 10),
					(Phase::Back, false) => (Answer::Pdu(positive(DID, &[0x2B])), 10),
					(phase, is_part) => panic!("{phase:?}: unexpected request (part number: {is_part}) {:02X?}", out.pdu),
				};
				now += cost;
				for delivery in p.answered(now, out.token, answer) {
					if phase == Phase::Back && matches!(delivery, Delivery::Reading { .. }) {
						readings_back += 1;
					}
					panel.take(&mut p, delivery);
				}
				phase = match phase {
					Phase::Answering if panel.unit.check() == Check::Matched && now > 1_000 => Phase::Quiet(0),
					Phase::Quiet(n) => {
						let n = n + 1;
						if n < SILENT_AFTER {
							assert_eq!(panel.unit.check(), Check::Matched, "{n} unanswered read(s) are not silence");
							assert!(panel.sub.is_some_and(|sub| p.holds(sub)), "and the unit keeps its reads");
							Phase::Quiet(n)
						} else {
							assert_eq!(panel.unit.check(), Check::Absent, "the {n}th is");
							assert!(panel.sub.is_none());
							Phase::LateRefusal
						}
					}
					Phase::LateRefusal if late_refusal_taken => {
						assert_eq!(panel.unit.check(), Check::Refusing, "a matched unit's refusal is not a mismatch");
						assert!(panel.unit.matched_before(), "and the board says it matched before");
						Phase::Refusing
					}
					Phase::Refusing if is_part => {
						assert_eq!(panel.unit.check(), Check::Refusing, "a second refusal: asked again still");
						Phase::Back
					}
					phase => phase,
				};
			}
			Next::Idle { until_ms } => now = panel.wake(now, until_ms),
		}
	}
	assert_eq!(readings_back, 3, "the unit is read again after it names the planned part");
	assert_eq!(panel.unit.check(), Check::Matched);
	// No tight loop: after each refusal the part number waits the backoff's cap.
	let after_refusals = &part_sends[part_sends.len() - 2..];
	let refusals_at = part_sends[part_sends.len() - 3];
	assert!(after_refusals[0] >= refusals_at + cap, "{part_sends:?}");
	assert!(after_refusals[1] >= after_refusals[0] + cap, "{part_sends:?}");
}

/// A unit that does not name the planned part from boot for `refusals` asks — `answer` each
/// time — and then does: asked again once per cap, nothing but `F187` read from it until then,
/// then matched and read. The asks' times.
fn named_on_the_last_ask(answer: &[u8], refusals: usize, waiting: Check) -> Vec<u64> {
	let mut p = Planner::new(Budget::board());
	let mut panel = Panel::new(&mut p);
	let (mut now, mut readings, mut part_sends) = (0u64, 0u32, Vec::new());
	while now < 30_000 && readings < 3 {
		panel.retry_when_due(&mut p, now);
		match p.due(now) {
			Next::Send(out) => {
				let is_part = out.pdu == [0x22, 0xF1, 0x87];
				let reply = if is_part {
					part_sends.push(now);
					if part_sends.len() <= refusals {
						Answer::Pdu(answer.to_vec())
					} else {
						Answer::Pdu(positive(PART_NUMBER, b"PART1 "))
					}
				} else {
					assert_eq!(panel.unit.check(), Check::Matched, "nothing but F187 before the match: {:02X?}", out.pdu);
					Answer::Pdu(positive(DID, &[0x2A]))
				};
				now += 10;
				for delivery in p.answered(now, out.token, reply) {
					if matches!(delivery, Delivery::Reading { .. }) {
						readings += 1;
					}
					panel.take(&mut p, delivery);
				}
				if is_part && part_sends.len() <= refusals {
					assert_eq!(panel.unit.check(), waiting, "{answer:02X?} is not a mismatch");
					assert!(!panel.unit.matched_before());
				}
			}
			Next::Idle { until_ms } => now = panel.wake(now, until_ms),
		}
	}
	assert_eq!(readings, 3, "{answer:02X?}: read once it names the planned part ({part_sends:?})");
	assert_eq!(panel.unit.check(), Check::Matched);
	assert_eq!(part_sends.len(), refusals + 1, "{part_sends:?}");
	part_sends
}

/// A unit that refuses `F187` from boot — `7F 22 22`, conditionsNotCorrect while it starts up —
/// and names the planned part on the third ask: asked again once per cap.
#[test]
fn a_unit_that_refuses_its_part_number_at_boot_is_asked_again_and_read_once_it_matches() {
	let cap = u64::from(Budget::board().backoff_cap_ms);
	let asks = named_on_the_last_ask(&[0x7F, 0x22, 0x22], 2, Check::Refusing);
	for pair in asks.windows(2) {
		assert!(pair[1] >= pair[0] + cap, "no faster than the cap: {asks:?}");
	}
}

/// A unit that answers `F187` with no number in it — nothing, padding, or an unwritten field's
/// `FF`s — is starting up, not another part: asked again once per cap.
#[test]
fn a_unit_that_answers_its_part_number_with_no_number_is_asked_again_and_read_once_it_matches() {
	let cap = u64::from(Budget::board().backoff_cap_ms);
	let mut ff = positive(PART_NUMBER, &[]);
	ff.extend_from_slice(&[0xFF; 11]);
	for blank in [positive(PART_NUMBER, &[]), positive(PART_NUMBER, b"\0\0\0\0  "), ff] {
		let asks = named_on_the_last_ask(&blank, 2, Check::Absent);
		for pair in asks.windows(2) {
			assert!(pair[1] >= pair[0] + cap, "{blank:02X?}: no faster than the cap: {asks:?}");
		}
	}
}

/// A matched unit that misses two reads in three — late answers on a busy bus — is never taken
/// for silent: each answer starts the count over, and its part number is never asked again.
#[test]
fn a_unit_that_answers_one_read_in_three_keeps_its_reads() {
	let mut p = Planner::new(Budget::board());
	let mut panel = Panel::new(&mut p);
	let (mut now, mut reads, mut part_sends) = (0u64, 0u32, 0u32);
	while now < 20_000 {
		panel.retry_when_due(&mut p, now);
		match p.due(now) {
			Next::Send(out) => {
				let (answer, cost) = if out.pdu == [0x22, 0xF1, 0x87] {
					part_sends += 1;
					(Answer::Pdu(positive(PART_NUMBER, b"PART1 ")), 10)
				} else {
					reads += 1;
					match reads % u32::from(SILENT_AFTER) {
						0 => (Answer::Pdu(positive(DID, &[0x2A])), 10),
						_ => (Answer::NoAnswer, RESPONSE_TIMEOUT_MS),
					}
				};
				now += cost;
				for delivery in p.answered(now, out.token, answer) {
					panel.take(&mut p, delivery);
				}
				assert_eq!(panel.unit.check(), Check::Matched, "read {reads} at {now} ms");
			}
			Next::Idle { until_ms } => now = panel.wake(now, until_ms),
		}
	}
	assert!(reads > 3 * u32::from(SILENT_AFTER), "{reads} reads");
	assert_eq!(part_sends, 1, "the part number asked once, at boot");
}

/// Another part number, at boot or after, is the one thing that strikes a unit off.
#[test]
fn a_unit_that_names_another_part_is_not_polled() {
	let mut p = Planner::new(Budget::board());
	let mut panel = Panel::new(&mut p);
	let out = match p.due(0) {
		Next::Send(out) => out,
		other => panic!("expected F187, got {other:?}"),
	};
	for delivery in p.answered(10, out.token, Answer::Pdu(positive(PART_NUMBER, b"PART2 "))) {
		panel.take(&mut p, delivery);
	}
	assert_eq!(panel.unit.check(), Check::Mismatch);
	assert_eq!(panel.unit.retry_in_ms(10), None);
	assert!(matches!(p.due(10_000), Next::Idle { .. }), "and nothing more is asked");
}
