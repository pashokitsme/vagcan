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
