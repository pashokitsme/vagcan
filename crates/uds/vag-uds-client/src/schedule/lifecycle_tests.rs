//! What the planner keeps, for how long, and what it can take back.
//!
//! The board's planner lives as long as the board and is fed by a host across a radio,
//! so everything it holds per unit has to go when nobody wants the unit any more —
//! except what keeps the bus safe (a silent unit's backoff) and what is cheap and
//! bounded (a unit learned to refuse multi-identifier reads). Ids and bytes are synthetic.

use alloc::vec;
use alloc::vec::Vec;

use super::*;

const A: Unit = Unit {
	request: 0x7E0,
	response: 0x7E8,
};

fn send(next: Next) -> Outgoing {
	match next {
		Next::Send(out) => out,
		other => panic!("expected a send, got {other:?}"),
	}
}

/// The attack on the board's heap: one request id, every response id, subscribed and
/// dropped. Nothing is left behind.
#[test]
fn a_unit_nobody_wants_is_forgotten() {
	let mut p = Planner::new(Budget::default());
	for response in 0..=0x7FFu16 {
		let unit = Unit { request: 0x7E0, response };
		let sub = p.subscribe(0, Class::Remote, unit, 0xF40D, 100, None);
		p.unsubscribe(sub);
	}
	assert_eq!(p.units_held(), 0);

	let once = p.read_once(0, Class::Foreground, A, 0xF187);
	let out = send(p.due(0));
	let got = p.answered(5, out.token, Answer::Pdu(vec![0x62, 0xF1, 0x87, 0x41]));
	assert!(matches!(got.as_slice(), [Delivery::Once { req, .. }] if *req == once));
	assert_eq!(p.units_held(), 0, "a one-shot that was answered leaves nothing");

	let raw = p.exchange(10, Class::Remote, A, vec![0x3E, 0x00]).unwrap();
	let out = send(p.due(20));
	assert_eq!(p.units_held(), 1, "a unit in flight is held");
	let got = p.answered(25, out.token, Answer::Pdu(vec![0x7E, 0x00]));
	assert!(matches!(got.as_slice(), [Delivery::Raw { req, .. }] if *req == raw));
	assert_eq!(p.units_held(), 0);
}

/// An exchange with a `not_after_ms` goes out up to that moment and never after: past it
/// the planner does not send it and does not wake for it, and `cancel` takes it back as one
/// that never went out. Another exchange behind it on the same unit is not held up.
#[test]
fn an_exchange_is_never_sent_after_its_not_after_and_cancel_takes_it_back() {
	let mut p = Planner::new(Budget::default());
	let on_time = p.exchange_until(0, Class::Timing, A, vec![0x10, 0x03], 50).unwrap();
	let out = send(p.due(50));
	assert_eq!(out.pdu, [0x10, 0x03], "at its not_after it still goes");
	let got = p.answered(55, out.token, Answer::Pdu(vec![0x50, 0x03]));
	assert!(matches!(got.as_slice(), [Delivery::Raw { req, .. }] if *req == on_time));

	let stale = p.exchange_until(100, Class::Timing, A, vec![0x10, 0x03], 150).unwrap();
	let other = p.exchange(100, Class::Remote, A, vec![0x3E, 0x00]).unwrap();
	let out = send(p.due(151));
	assert_eq!(out.pdu, [0x3E, 0x00], "one past it, only the other exchange goes");
	let got = p.answered(155, out.token, Answer::Pdu(vec![0x7E, 0x00]));
	assert!(matches!(got.as_slice(), [Delivery::Raw { req, .. }] if *req == other));
	assert_eq!(p.due(160), Next::Idle { until_ms: None }, "nothing to send, nothing to wake for");
	assert_eq!(p.units_held(), 1, "held until its owner takes it back");
	assert!(p.cancel(stale), "it never went out");
	assert!(!p.cancel(stale));
	assert_eq!(p.units_held(), 0);
}

/// Forgetting must not undo the backoff: the panel asks a silent unit its part number
/// once, gets "no answer", and asks once more at once — which, if the unit had been
/// forgotten with its backoff, would put a request on the bus every exchange.
#[test]
fn a_silent_unit_asked_once_again_and_again_is_still_backed_off() {
	let mut p = Planner::new(Budget::default());
	let mut now = 0;
	let mut sends = 0;
	p.read_once(now, Class::Foreground, A, 0xF187);
	while now < 10_000 {
		match p.due(now) {
			Next::Send(out) => {
				sends += 1;
				now += 5;
				for d in p.answered(now, out.token, Answer::NoAnswer) {
					if let Delivery::Once { .. } = d {
						p.read_once(now, Class::Foreground, A, 0xF187);
					}
				}
			}
			Next::Idle { until_ms: Some(t) } => now = t,
			Next::Idle { until_ms: None } => break,
		}
	}
	// 250, 500, 1000, 2000, 2000… : a handful in ten seconds, not two thousand.
	assert!(sends <= 9, "{sends} sends in 10 s to a silent unit");
	assert!(p.units_held() <= 1);
}

/// Learned from a definite answer and cheap to keep: a unit that refuses
/// multi-identifier reads is still asked singly after it was forgotten.
#[test]
fn single_only_outlives_the_unit_it_was_learned_on() {
	let mut p = Planner::new(Budget::default());
	let one = p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
	let two = p.subscribe(0, Class::Foreground, A, 0x1001, 100, None);
	let out = send(p.due(0));
	assert_eq!(out.pdu.len(), 5, "asked together first: {:02X?}", out.pdu);
	p.answered(5, out.token, Answer::Pdu(vec![0x7F, 0x22, 0x13]));
	p.unsubscribe(one);
	p.unsubscribe(two);
	let _ = p.due(10);
	assert_eq!(p.units_held(), 0);

	p.subscribe(20, Class::Foreground, A, 0x1000, 100, None);
	p.subscribe(20, Class::Foreground, A, 0x1001, 100, None);
	let out = send(p.due(20));
	assert_eq!(out.pdu.len(), 3, "asked singly: {:02X?}", out.pdu);
}

/// A consumer that is gone takes its queued exchanges with it; one already on the bus
/// cannot be taken back.
#[test]
fn a_queued_exchange_can_be_cancelled_and_one_in_flight_cannot() {
	let mut p = Planner::new(Budget::default());
	let queued = p.exchange(0, Class::Remote, A, vec![0x22, 0xF1, 0x90]).unwrap();
	assert!(p.cancel(queued));
	assert!(!p.cancel(queued), "cancelled once");
	assert_eq!(p.due(0), Next::Idle { until_ms: None });
	assert_eq!(p.units_held(), 0);

	let flying = p.exchange(0, Class::Remote, A, vec![0x22, 0xF1, 0x90]).unwrap();
	let out = send(p.due(0));
	assert!(!p.cancel(flying));
	let got = p.answered(5, out.token, Answer::NoAnswer);
	assert!(matches!(got.as_slice(), [Delivery::Raw { req, .. }] if *req == flying));
}

/// `3E 80` asks the unit to say nothing, and it does. That silence is the answer, not
/// an absent unit: nobody else reading the unit misses a reading, and nothing backs off.
#[test]
fn a_request_that_expects_no_answer_does_not_back_the_unit_off() {
	let mut p = Planner::new(Budget::default());
	let sub = p.subscribe(0, Class::Foreground, A, 0xF40D, 100, None);
	let out = send(p.due(0));
	p.answered(5, out.token, Answer::Pdu(vec![0x62, 0xF4, 0x0D, 0x00]));

	let quiet = p.exchange(10, Class::Remote, A, vec![0x3E, 0x80]).unwrap();
	let out = send(p.due(10));
	assert_eq!(out.pdu, [0x3E, 0x80]);
	let got = p.answered(60, out.token, Answer::NotExpected);
	assert_eq!(got.len(), 1, "{got:?}");
	assert!(matches!(&got[0], Delivery::Raw { req, answer: Answer::NotExpected, .. } if *req == quiet));
	assert!(!got.iter().any(|d| matches!(d, Delivery::Missed { .. })));

	let out = send(p.due(100));
	assert_eq!(out.pdu, [0x22, 0xF4, 0x0D], "the subscriber's next read is on time");
	let got = p.answered(105, out.token, Answer::Pdu(vec![0x62, 0xF4, 0x0D, 0x00]));
	assert!(matches!(got.as_slice(), [Delivery::Reading { sub: s, .. }] if *s == sub));
}

/// The board's fault count ends its own exchange 2 s from its start, and a unit that has asked
/// for more time (`7F 19 78`) is often still searching then (`todo/dash/20`). That unit is there:
/// it is never `NoAnswer`, which the panel takes for an absent unit and answers by dropping its
/// cells and asking its part number again (review round 1). It is backed off as silence backs it
/// off, its readers told `Busy` (round 3) — unless a run is timing it: then one such exchange
/// between answers costs no wait and no reader a sample (round 4: a backoff there pushed the
/// run's speed past the stopwatch's silence), and the second in a row backs it off (round 5).
#[test]
fn a_busy_raw_exchange_backs_the_unit_off_unless_a_run_is_timing_it_and_it_is_the_first() {
	// No run times the unit: backed off from the first, its reader told.
	let mut p = Planner::new(Budget::board());
	let sub = p.subscribe(0, Class::Foreground, A, 0xF40D, 100, None);
	let out = send(p.due(0));
	p.answered(5, out.token, Answer::Pdu(vec![0x62, 0xF4, 0x0D, 0x00]));
	p.exchange(10, Class::Background, A, vec![0x19, 0x02, 0x08]).unwrap();
	let out = send(p.due(10));
	let got = p.answered(2_010, out.token, Answer::Busy { asked_for_time: true });
	assert!(
		matches!(got.as_slice(), [Delivery::Missed { sub: s, why: Miss::Busy, .. }, Delivery::Raw { .. }] if *s == sub),
		"{got:?}"
	);
	let first = u64::from(Budget::board().backoff_first_ms);
	assert!(matches!(p.due(2_010 + first - 1), Next::Idle { .. }), "backed off");

	// A run is timing it: the first costs nothing; the second in a row backs it off.
	let mut p = Planner::new(Budget::board());
	let speed = p.subscribe(0, Class::Timing, A, 0xF40D, 50, None);
	let out = send(p.due(0));
	p.answered(5, out.token, Answer::Pdu(vec![0x62, 0xF4, 0x0D, 0x00]));
	let count = p.exchange(10, Class::Background, A, vec![0x19, 0x02, 0x08]).unwrap();
	let out = send(p.due(50));
	assert_eq!(out.pdu, [0x22, 0xF4, 0x0D], "the run's speed first");
	p.answered(55, out.token, Answer::Pdu(vec![0x62, 0xF4, 0x0D, 0x00]));
	let out = send(p.due(60));
	assert_eq!(out.pdu, [0x19, 0x02, 0x08]);
	let got = p.answered(2_060, out.token, Answer::Busy { asked_for_time: true });
	assert!(
		matches!(got.as_slice(), [Delivery::Raw { req, answer: Answer::Busy { asked_for_time: true }, .. }] if *req == count),
		"{got:?}"
	);
	let out = send(p.due(2_060));
	assert_eq!(out.pdu, [0x22, 0xF4, 0x0D], "the speed's next read goes at once");
	let got = p.answered(2_065, out.token, Answer::Busy { asked_for_time: false });
	assert!(
		matches!(got.as_slice(), [Delivery::Missed { sub: s, why: Miss::Busy, .. }] if *s == speed),
		"the second in a row: {got:?}"
	);
	// Backed off at the step two failures reach, as silence would be.
	assert!(matches!(p.due(2_065 + 2 * first - 1), Next::Idle { .. }), "backed off");
	let out = send(p.due(2_065 + 2 * first));
	let got = p.answered(2_065 + 2 * first + 5, out.token, Answer::Pdu(vec![0x62, 0xF4, 0x0D, 0x00]));
	assert!(
		matches!(got.as_slice(), [Delivery::Reading { sub: s, .. }] if *s == speed),
		"the reader kept"
	);
}

/// A read a busy unit did not answer in time — it asked for time, or only finished an earlier
/// answer (review round 2, 2026-09-27) — is a sample missed from a unit that is there: every
/// reader and one-shot is told `Busy`, never `NoAnswer` — taken for silence, the panel marked the
/// unit absent and dropped its subscriptions, a run's speed with them. No run times this unit, so
/// it is backed off as silence is (review rounds 3 and 5; a timed unit's pass is
/// `a_busy_raw_exchange_backs_the_unit_off_unless_a_run_is_timing_it_and_it_is_the_first` and
/// `research/dash/host/tests/stopwatch_silence.rs`).
#[test]
fn a_read_a_busy_unit_did_not_answer_in_time_is_a_missed_sample_from_a_unit_that_is_there() {
	for asked_for_time in [false, true] {
		let mut p = Planner::new(Budget::default());
		let speed = p.subscribe(0, Class::Foreground, A, 0xF40D, 100, None);
		let other = p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
		let once = p.read_once(0, Class::Foreground, A, 0x1001);
		let out = send(p.due(0));
		assert_eq!(out.pdu.len(), 7, "one request for the three: {:02X?}", out.pdu);
		let got = p.answered(5, out.token, Answer::Busy { asked_for_time });
		for sub in [speed, other] {
			assert!(
				got
					.iter()
					.any(|d| matches!(d, Delivery::Missed { sub: s, why: Miss::Busy, .. } if *s == sub)),
				"{asked_for_time}: {got:?}"
			);
		}
		assert!(!got.iter().any(|d| matches!(d, Delivery::Missed { why: Miss::NoAnswer, .. })));
		assert!(
			got
				.iter()
				.any(|d| matches!(d, Delivery::Once { req, result: Err(Miss::Busy), .. } if *req == once)),
			"{asked_for_time}: {got:?}"
		);
		// No run times the unit: backed off from the first, as silence is.
		assert!(matches!(p.due(100), Next::Idle { .. }), "{asked_for_time}: backed off");
	}
}

/// A late answer to an earlier request on the same ids is no answer to this one: the shell
/// drops it and waits on (review, 2026-09-27). ISO 14229-1: a positive response is the
/// request's service id + `0x40`, a negative one `7F` and the request's service id.
#[test]
fn a_response_answers_its_own_request_and_no_other() {
	let read: &[u8] = &[0x22, 0xF1, 0x87];
	assert!(answers(read, &[0x62, 0xF1, 0x87, b'P']));
	assert!(answers(&[0x22, 0xF1, 0x90, 0xF1, 0x87], &[0x62, 0xF1, 0x90, b'V', 0xF1, 0x87, b'P']));
	assert!(!answers(read, &[0x62, 0xF1, 0x90, b'V']), "another identifier's late answer");
	// Review, 2026-09-27: the gateway's late list is not the part number asked of it next.
	assert!(!answers(read, &[0x62, 0x2A, 0x26, 0xFF, 0x13]), "the gateway's late list");
	// ISO 14229-1 lets a unit leave out an identifier it does not support, the first included:
	// an answer that starts with any identifier asked is this request's (review round 3 — the
	// first-identifier rule dropped `62 20 00 …` to `22 10 00 20 00`, and `2000` was never read).
	let batch: &[u8] = &[0x22, 0x10, 0x00, 0x20, 0x00];
	assert!(answers(batch, &[0x62, 0x20, 0x00, 0x2A]), "the first left out");
	assert!(answers(batch, &[0x62, 0x10, 0x00, 0x01, 0x20, 0x00, 0x2A]));
	assert!(!answers(batch, &[0x62, 0x30, 0x00, 0x2A]), "an identifier not asked");
	assert!(!answers(batch, &[0x62, 0x20]), "half an identifier");
	// A positive answer that carries no record at all can be no late answer to another
	// identifier; the planner judges it (an empty positive teaches it single-only).
	assert!(answers(read, &[0x62]));
	// Negative: `7F <this service> <nrc>`, pending included; the NRC is not optional.
	assert!(answers(read, &[0x7F, 0x22, 0x31]));
	assert!(answers(read, &[0x7F, 0x22, 0x78]), "its own pending");
	assert!(!answers(read, &[0x7F, 0x22]), "a refusal carries its NRC");
	assert!(!answers(read, &[0x7F]), "a refusal of nothing");
	assert!(!answers(read, &[0x59, 0x02, 0xFF, 0, 1, 2, 0x08]), "the count's late answer");
	assert!(!answers(read, &[0x7F, 0x19, 0x78]), "the count's late pending");
	assert!(!answers(read, &[0x7F, 0x19, 0x21]));
	assert!(!answers(read, &[]));
	// Sub-functions are echoed, without the suppress-positive-response bit: the count's late
	// `59 02 …` is no answer to a host's `19 04` or `19 06` (review, 2026-09-27).
	assert!(answers(&[0x19, 0x02, 0x08], &[0x59, 0x02, 0xFF]));
	assert!(!answers(&[0x19, 0x04, 0x01, 0x02, 0x03, 0xFF], &[0x59, 0x02, 0xFF, 0, 1, 2, 0x08]));
	assert!(!answers(&[0x19, 0x06, 0x01, 0x02, 0x03, 0xFF], &[0x59, 0x02, 0xFF, 0, 1, 2, 0x08]));
	assert!(!answers(&[0x19, 0x02, 0xFF], &[0x59, 0x0A]));
	assert!(answers(&[0x10, 0x03], &[0x50, 0x03, 0x00, 0x32, 0x01, 0xF4]));
	assert!(!answers(&[0x10, 0x03], &[0x50, 0x01]));
	assert!(answers(&[0x3E, 0x00], &[0x7E, 0x00]));
	assert!(answers(&[0x3E, 0x80], &[0x7E, 0x00]), "a positive a unit sent anyway is still its own");
	assert!(!answers(&[0x3E, 0x00], &[0x7E]));
	assert!(answers(&[0x3E, 0x80], &[0x7F, 0x3E, 0x12]));
	assert!(!answers(&[], &[0x40]), "no request, nothing answers it");
}

/// The board's shell against the unit that left out the batch's first identifier (review round 3,
/// the safety probe `r3_a_batch_whose_first_did_the_unit_omits`): the answer the unit gives is
/// taken, the identifier it left out is `Absent`, the one it answered read at its rate, and the
/// bus held for its answers, not for 500 ms waits.
#[test]
fn a_batch_whose_first_identifier_the_unit_leaves_out_is_read_through_the_shells_rule() {
	let mut p = Planner::new(Budget::board());
	let unsupported = p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
	let supported = p.subscribe(0, Class::Foreground, A, 0x2000, 100, None);
	let (mut now, mut held, mut read, mut absent) = (0u64, 0u64, 0usize, 0usize);
	while now < 5_000 {
		match p.due(now) {
			Next::Send(out) => {
				// The unit answers what it supports, in the order asked.
				let mut answer = vec![0x62];
				for did in out.pdu[1..].chunks(2) {
					if did == [0x20, 0x00] {
						answer.extend_from_slice(&[0x20, 0x00, 0x2A]);
					}
				}
				// The shell: an answer that is not this request's is dropped, and the exchange ends
				// busy after the answer timeout.
				let (answer, cost) = if answers(&out.pdu, &answer) {
					(Answer::Pdu(answer), 5)
				} else {
					(Answer::Busy { asked_for_time: false }, 500)
				};
				now += cost;
				held += cost;
				for d in p.answered(now, out.token, answer) {
					match d {
						Delivery::Reading { sub, .. } if sub == supported => read += 1,
						Delivery::Missed { sub, why: Miss::Absent, .. } if sub == unsupported => absent += 1,
						_ => {}
					}
				}
			}
			Next::Idle { until_ms } => now = until_ms.unwrap_or(now + 10).max(now + 1),
		}
	}
	assert!(read >= 45, "{read} readings of 2000 in 5 s at 10 Hz");
	assert!(absent > 0, "the identifier left out is told so");
	assert!(held < 1_000, "the bus held {held} ms of 5000");
}

/// Two cells at 10 Hz on each of two units; `A` answers every exchange as `a_answer` after
/// `a_cost` ms, `B` in 5 ms. How many readings `B` gets in `until` ms.
fn beside(a_answer: &Answer, a_cost: u64, until: u64) -> usize {
	beside_with(&mut |_, _| (a_answer.clone(), a_cost), until)
}

/// A healthy unit's answer to `pdu`: every identifier asked, one byte each.
fn every_record(pdu: &[u8]) -> Answer {
	let mut answer = vec![0x62];
	for did in pdu[1..].chunks(2) {
		answer.extend_from_slice(did);
		answer.push(0x2A);
	}
	Answer::Pdu(answer)
}

/// How `A` answers its `n`-th exchange, `pdu`: the answer, and after how many ms.
type Reply<'a> = &'a mut dyn FnMut(usize, &[u8]) -> (Answer, u64);

/// [`beside`], `A` answering its `n`-th exchange as `a(n, pdu)` says, and after that many ms.
fn beside_with(a: Reply<'_>, until: u64) -> usize {
	const B: Unit = Unit {
		request: 0x714,
		response: 0x77E,
	};
	let mut p = Planner::new(Budget::board());
	for did in [0x1001u16, 0x1002] {
		p.subscribe(0, Class::Foreground, A, did, 100, None);
		p.subscribe(0, Class::Foreground, B, did, 100, None);
	}
	let (mut now, mut read, mut a_asked) = (0u64, 0usize, 0usize);
	while now < until {
		match p.due(now) {
			Next::Send(out) => {
				let (answer, cost) = if out.unit == A {
					a_asked += 1;
					a(a_asked, &out.pdu)
				} else {
					(every_record(&out.pdu), 5)
				};
				now += cost;
				read += p
					.answered(now, out.token, answer)
					.iter()
					.filter(|d| matches!(d, Delivery::Reading { unit, .. } if *unit == B))
					.count();
			}
			Next::Idle { until_ms } => now = until_ms.unwrap_or(now + 10).max(now + 1),
		}
	}
	read
}

/// A unit heard but never answering this board — another tester's traffic on its id, or `78`
/// to the end — costs its neighbours what a silent one does (review round 3, the safety probe
/// `r3_busy_forever_vs_silent`): `Busy` backs it off as a non-answer does. It kept the unit's
/// backoff reset, and a healthy unit beside it got 40 readings in 10 s where a silent neighbour
/// left it 144 — with `78` to the 10 s limit, 12 in 60 s against 118. No run times this unit, so
/// no `Busy` of it passes free (round 5): 144 and 144, 118 and 118.
#[test]
fn a_unit_busy_forever_costs_its_neighbours_what_a_silent_one_does() {
	let silent = beside(&Answer::NoAnswer, 500, 10_000);
	let busy = beside(&Answer::Busy { asked_for_time: false }, 500, 10_000);
	assert!(silent >= 100, "{silent}");
	assert!(
		busy >= silent,
		"B got {busy} readings in 10 s beside a busy unit, {silent} beside a silent one"
	);
	let silent = beside(&Answer::NoAnswer, 10_000, 60_000);
	let busy = beside(&Answer::Busy { asked_for_time: true }, 10_000, 60_000);
	assert!(
		busy >= silent,
		"B got {busy} readings in 60 s beside a unit saying 78 to the end, {silent} beside a silent one"
	);
}

/// A unit whose every other exchange ends busy — a second tester's late answers landing in them —
/// and that answers in between is backed off from each `Busy`, as one silent every other time
/// is (review round 5, firmware: with the first `Busy` free, each answer made the next `Busy`
/// free again, the unit never waited, and a healthy neighbour got 66 readings in 10 s against 94
/// beside a silent/answer unit). No run is timing it, so nothing is owed a free pass.
#[test]
fn a_unit_busy_every_other_time_costs_its_neighbours_what_one_silent_every_other_time_does() {
	let alternate = |miss: Answer| move |n: usize, pdu: &[u8]| if n % 2 == 1 { (miss.clone(), 500) } else { (every_record(pdu), 5) };
	let silent = beside_with(&mut alternate(Answer::NoAnswer), 10_000);
	let busy = beside_with(&mut alternate(Answer::Busy { asked_for_time: false }), 10_000);
	assert!(
		busy >= silent,
		"B got {busy} readings in 10 s beside a busy/answer unit, {silent} beside a silent/answer one"
	);
}

/// A unit with no reader that a host asks one raw request at a time, the next as soon as the last
/// is answered: every `Busy` backs it off as silence does, and the backoff climbs 250 → 2000 ms
/// (review round 5, safety: a first `Busy` set no wait, the planner forgot the unit with its
/// count, every attempt was a first `Busy` again, and a healthy neighbour fell from 150 readings
/// in 10 s to 40).
#[test]
fn a_host_asking_a_busy_unit_nobody_reads_again_and_again_is_backed_off_as_for_silence() {
	const C: Unit = Unit {
		request: 0x746,
		response: 0x7B0,
	};
	const B: Unit = Unit {
		request: 0x714,
		response: 0x77E,
	};
	let run = |c_answer: Answer| {
		let mut p = Planner::new(Budget::board());
		for did in [0x1001u16, 0x1002] {
			p.subscribe(0, Class::Foreground, B, did, 100, None);
		}
		p.exchange(0, Class::Remote, C, vec![0x19, 0x02, 0xFF]).unwrap();
		let (mut now, mut read, mut waits, mut answered_at) = (0u64, 0usize, Vec::new(), None::<u64>);
		while now < 10_000 {
			match p.due(now) {
				Next::Send(out) => {
					let (answer, cost) = if out.unit == C {
						if let Some(at) = answered_at {
							waits.push(now - at);
						}
						(c_answer.clone(), 500)
					} else {
						(every_record(&out.pdu), 5)
					};
					now += cost;
					for d in p.answered(now, out.token, answer) {
						match d {
							Delivery::Reading { unit, .. } if unit == B => read += 1,
							// The host asks again at once.
							Delivery::Raw { .. } => {
								answered_at = Some(now);
								p.exchange(now, Class::Remote, C, vec![0x19, 0x02, 0xFF]).unwrap();
							}
							_ => {}
						}
					}
				}
				Next::Idle { until_ms } => now = until_ms.unwrap_or(now + 10).max(now + 1),
			}
		}
		(read, waits)
	};
	let (silent_read, silent_waits) = run(Answer::NoAnswer);
	let (busy_read, busy_waits) = run(Answer::Busy { asked_for_time: false });
	// The backoff's steps, each met (a send slot may add a few ms).
	for (wait, step) in silent_waits.iter().zip([250, 500, 1_000, 2_000, 2_000]) {
		assert!((step..step + 20).contains(wait), "silence waits {silent_waits:?}");
	}
	assert_eq!(busy_waits, silent_waits, "busy waits as silence does");
	assert!(
		busy_read >= silent_read,
		"B got {busy_read} readings beside the busy unit, {silent_read} beside the silent one"
	);
}

#[test]
fn only_a_suppressed_positive_response_expects_no_answer() {
	let expects_none: Vec<&[u8]> = vec![&[0x3E, 0x80], &[0x10, 0x81], &[0x10, 0x83]];
	for pdu in expects_none {
		assert!(expects_no_answer(pdu), "{pdu:02X?}");
	}
	let expects_one: Vec<&[u8]> = vec![
		&[0x3E, 0x00],
		&[0x10, 0x03],
		&[0x22, 0xF1, 0x90],
		&[0x22, 0x80, 0x00],
		&[0x19, 0x82, 0xFF],
		&[0x3E],
		&[],
	];
	for pdu in expects_one {
		assert!(!expects_no_answer(pdu), "{pdu:02X?}");
	}
}
