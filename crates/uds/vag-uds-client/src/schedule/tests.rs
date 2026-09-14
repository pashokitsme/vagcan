//! The planner's behaviour, against a simulated car and a simulated clock.
//!
//! Identifiers and record bytes here are synthetic: the planner never looks inside a
//! record, so nothing in these tests is a fact about any car.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::uds::UdsError;

const A: Unit = Unit {
	request: 0x7E0,
	response: 0x7E8,
};
const B: Unit = Unit {
	request: 0x7E1,
	response: 0x7E9,
};

fn unit(n: u16) -> Unit {
	Unit {
		request: 0x700 + n,
		response: 0x780 + n,
	}
}

/// How a simulated control unit behaves.
#[derive(Default, Clone)]
struct FakeUnit {
	records: BTreeMap<u16, Vec<u8>>,
	silent: bool,
	refuses_multi: bool,
	refuses_multi_empty: bool,
	/// Refuses multi-identifier requests with this NRC while the count lasts, then answers
	/// them: a transient refusal, or one that says nothing about batching.
	multi_nrc: Option<(u8, core::cell::Cell<u32>)>,
}

impl FakeUnit {
	fn with(records: &[(u16, &[u8])]) -> Self {
		FakeUnit {
			records: records.iter().map(|(d, r)| (*d, r.to_vec())).collect(),
			..FakeUnit::default()
		}
	}
}

#[derive(Default)]
struct Car {
	units: BTreeMap<Unit, FakeUnit>,
}

impl Car {
	fn answer(&self, out: &Outgoing) -> Answer {
		let Some(u) = self.units.get(&out.unit) else {
			return Answer::NoAnswer;
		};
		if u.silent {
			return Answer::NoAnswer;
		}
		let sid = out.pdu[0];
		if sid != 0x22 {
			let mut pdu = vec![sid + 0x40];
			pdu.extend_from_slice(&out.pdu[1..]);
			return Answer::Pdu(pdu);
		}
		let dids = dids_of(&out.pdu);
		if dids.len() > 1 && u.refuses_multi_empty {
			return Answer::Pdu(vec![0x62]);
		}
		if dids.len() > 1 && u.refuses_multi {
			return Answer::Pdu(vec![0x7F, 0x22, 0x13]);
		}
		if let Some((nrc, left)) = u.multi_nrc.as_ref().filter(|(_, left)| dids.len() > 1 && left.get() > 0) {
			left.set(left.get() - 1);
			return Answer::Pdu(vec![0x7F, 0x22, *nrc]);
		}
		if dids.len() == 1 && !u.records.contains_key(&dids[0]) {
			return Answer::Pdu(vec![0x7F, 0x22, 0x31]);
		}
		let mut pdu = vec![0x62];
		for did in dids {
			if let Some(record) = u.records.get(&did) {
				pdu.extend_from_slice(&did.to_be_bytes());
				pdu.extend_from_slice(record);
			}
		}
		Answer::Pdu(pdu)
	}
}

fn dids_of(pdu: &[u8]) -> Vec<u16> {
	assert_eq!(pdu[0], 0x22, "{pdu:02X?}");
	pdu[1..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect()
}

/// A planner driven against a [`Car`] with a fixed answer latency.
struct Sim {
	p: Planner,
	car: Car,
	now: u64,
	latency: u64,
	/// A unit's own answer latency, where it is not `latency`.
	unit_latency: BTreeMap<Unit, u64>,
	sends: Vec<(u64, Outgoing)>,
	got: Vec<Delivery>,
}

impl Sim {
	fn new(budget: Budget, car: Car) -> Self {
		Sim {
			p: Planner::new(budget),
			car,
			now: 0,
			latency: 4,
			unit_latency: BTreeMap::new(),
			sends: Vec::new(),
			got: Vec::new(),
		}
	}

	/// When the one-shot or raw exchange `req` was delivered.
	fn delivered_at(&self, req: ReqId) -> Option<u64> {
		self.got.iter().find_map(|d| match d {
			Delivery::Once { req: r, at_ms, .. } | Delivery::Raw { req: r, at_ms, .. } if *r == req => Some(*at_ms),
			_ => None,
		})
	}

	/// Run the loop a shell runs until `end`, calling `hook` after every step with the
	/// deliveries that step produced.
	fn run_until_with(&mut self, end: u64, mut hook: impl FnMut(&mut Planner, u64, &[Delivery])) {
		while self.now < end {
			match self.p.due(self.now) {
				Next::Send(out) => {
					let answer = self.car.answer(&out);
					self.sends.push((self.now, out.clone()));
					self.now += self.unit_latency.get(&out.unit).copied().unwrap_or(self.latency);
					let d = self.p.answered(self.now, out.token, answer);
					hook(&mut self.p, self.now, &d);
					self.got.extend(d);
				}
				Next::Idle { until_ms: Some(t) } => {
					assert!(t > self.now, "idle until {t} at {} would spin", self.now);
					self.now = t.min(end);
					hook(&mut self.p, self.now, &[]);
				}
				Next::Idle { until_ms: None } => {
					self.now = end;
					hook(&mut self.p, self.now, &[]);
				}
			}
		}
	}

	fn run_until(&mut self, end: u64) {
		self.run_until_with(end, |_, _, _| {});
	}

	fn sends_to(&self, unit: Unit, from: u64, to: u64) -> usize {
		self.sends.iter().filter(|(t, o)| o.unit == unit && *t >= from && *t < to).count()
	}

	fn readings_of(&self, sub: SubId) -> Vec<(Vec<u8>, u64)> {
		self
			.got
			.iter()
			.filter_map(|d| match d {
				Delivery::Reading { sub: s, data, at_ms, .. } if *s == sub => Some((data.clone(), *at_ms)),
				_ => None,
			})
			.collect()
	}

	fn misses_of(&self, sub: SubId) -> Vec<Miss> {
		self
			.got
			.iter()
			.filter_map(|d| match d {
				Delivery::Missed { sub: s, why, .. } if *s == sub => Some(*why),
				_ => None,
			})
			.collect()
	}
}

fn send(next: Next) -> Outgoing {
	match next {
		Next::Send(out) => out,
		other => panic!("expected a send, got {other:?}"),
	}
}

fn car(units: &[(Unit, FakeUnit)]) -> Car {
	Car {
		units: units.iter().cloned().collect(),
	}
}

#[test]
fn two_subscribers_to_one_identifier_share_one_read() {
	let mut sim = Sim::new(Budget::default(), car(&[(A, FakeUnit::with(&[(0x1000, &[7])]))]));
	let one = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
	let two = sim.p.subscribe(0, Class::Background, A, 0x1000, 100, None);
	sim.run_until(1000);

	assert_eq!(sim.sends.len(), 10, "one read per period, not per subscriber");
	assert!(sim.sends.iter().all(|(_, o)| o.pdu == [0x22, 0x10, 0x00]));
	assert_eq!(sim.readings_of(one).len(), 10);
	assert_eq!(sim.readings_of(two), sim.readings_of(one));
}

#[test]
fn unsubscribing_the_faster_subscriber_slows_the_read_to_the_slower_one() {
	let mut sim = Sim::new(Budget::default(), car(&[(A, FakeUnit::with(&[(0x1000, &[7])]))]));
	let fast = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
	let slow = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 500, None);
	sim.run_until(1000);
	assert_eq!(sim.sends_to(A, 0, 1000), 10);

	sim.p.unsubscribe(fast);
	sim.run_until(3000);
	assert_eq!(sim.sends_to(A, 1000, 3000), 4, "every 500 ms once the 100 ms subscriber is gone");
	assert_eq!(sim.readings_of(slow).len(), 14);
	assert_eq!(sim.readings_of(fast).len(), 10, "nothing reaches a subscription after it is gone");
}

#[test]
fn an_unsubscribed_identifier_leaves_future_requests() {
	let mut sim = Sim::new(Budget::default(), car(&[(A, FakeUnit::with(&[(0x1000, &[1]), (0x1001, &[2])]))]));
	sim.p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
	let gone = sim.p.subscribe(0, Class::Foreground, A, 0x1001, 100, None);
	sim.run_until(100);
	assert_eq!(dids_of(&sim.sends[0].1.pdu), vec![0x1000, 0x1001]);

	sim.p.unsubscribe(gone);
	sim.run_until(1000);
	assert!(sim.sends[1..].iter().all(|(_, o)| dids_of(&o.pdu) == [0x1000]), "{:?}", sim.sends);
}

#[test]
fn due_and_nearly_due_identifiers_of_a_unit_go_out_as_one_request() {
	let mut p = Planner::new(Budget::default());
	p.subscribe(0, Class::Foreground, A, 0x1000, 1000, None);
	let first = send(p.due(0));
	p.answered(4, first.token, Answer::Pdu(vec![0x62, 0x10, 0x00, 1]));
	assert_eq!(p.due(800), Next::Idle { until_ms: Some(1000) });

	// 0x1001 comes due at 800; 0x1000 is 200 ms from due, inside a quarter period.
	p.subscribe(800, Class::Foreground, A, 0x1001, 1000, None);
	let out = send(p.due(800));
	assert_eq!(out.pdu, [0x22, 0x10, 0x01, 0x10, 0x00]);
	p.answered(804, out.token, Answer::Pdu(vec![0x62, 0x10, 0x01, 2, 0x10, 0x00, 1]));

	// Pulled forward keeps its phase: 0x1000 is next due at 2000, not at 1800.
	assert_eq!(p.due(1000), Next::Idle { until_ms: Some(1800) });
	let out = send(p.due(1800));
	assert_eq!(out.pdu, [0x22, 0x10, 0x01, 0x10, 0x00]);
}

#[test]
fn a_request_carries_at_most_max_dids() {
	let records: Vec<(u16, Vec<u8>)> = (0..10).map(|i| (0x1000 + i, vec![i as u8])).collect();
	let unit_records: Vec<(u16, &[u8])> = records.iter().map(|(d, r)| (*d, r.as_slice())).collect();
	let mut sim = Sim::new(Budget::default(), car(&[(A, FakeUnit::with(&unit_records))]));
	for (did, _) in &records {
		sim.p.subscribe(0, Class::Foreground, A, *did, 1000, None);
	}
	sim.run_until(500);
	let sizes: Vec<usize> = sim.sends.iter().map(|(_, o)| dids_of(&o.pdu).len()).collect();
	assert_eq!(sizes, vec![8, 2]);
	assert_eq!(sim.got.iter().filter(|d| matches!(d, Delivery::Reading { .. })).count(), 10);
}

#[test]
fn known_record_lengths_split_an_answer_deterministically() {
	// 0x1001's echo sits inside 0x1000's record, so without lengths it reads two ways.
	let fake = FakeUnit::with(&[(0x1000, &[0x10, 0x01, 0xAA]), (0x1001, &[0xBB])]);
	let mut sim = Sim::new(Budget::default(), car(&[(A, fake)]));
	let long = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 1000, Some(3));
	let short = sim.p.subscribe(0, Class::Foreground, A, 0x1001, 1000, Some(1));
	sim.run_until(500);

	assert_eq!(sim.sends.len(), 1, "one request, split by length, nothing retried: {:?}", sim.sends);
	assert_eq!(sim.readings_of(long), vec![(vec![0x10, 0x01, 0xAA], 4)]);
	assert_eq!(sim.readings_of(short), vec![(vec![0xBB], 4)]);
}

#[test]
fn without_lengths_the_answer_is_split_by_search() {
	let fake = FakeUnit::with(&[(0x1000, &[0x01]), (0x1001, &[0x02, 0x03])]);
	let mut sim = Sim::new(Budget::default(), car(&[(A, fake)]));
	let one = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 1000, None);
	let two = sim.p.subscribe(0, Class::Foreground, A, 0x1001, 1000, None);
	sim.run_until(500);

	assert_eq!(sim.sends.len(), 1);
	assert_eq!(sim.readings_of(one), vec![(vec![0x01], 4)]);
	assert_eq!(sim.readings_of(two), vec![(vec![0x02, 0x03], 4)]);
}

#[test]
fn an_answer_that_splits_two_ways_is_retried_singly() {
	let fake = FakeUnit::with(&[(0x1000, &[0x10, 0x01, 0xAA]), (0x1001, &[0xBB])]);
	let mut sim = Sim::new(Budget::default(), car(&[(A, fake)]));
	let long = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 1000, None);
	let short = sim.p.subscribe(0, Class::Foreground, A, 0x1001, 1000, None);
	let once = sim.p.read_once(0, Class::Remote, A, 0x1001);
	sim.run_until(500);

	let shapes: Vec<Vec<u16>> = sim.sends.iter().map(|(_, o)| dids_of(&o.pdu)).collect();
	assert_eq!(shapes.len(), 3, "{shapes:?}");
	assert_eq!(shapes[0].len(), 2);
	assert!(shapes[1..].iter().all(|s| s.len() == 1), "{shapes:?}");
	assert_eq!(sim.readings_of(long).len(), 1);
	assert_eq!(sim.readings_of(long)[0].0, vec![0x10, 0x01, 0xAA]);
	assert_eq!(sim.readings_of(short)[0].0, vec![0xBB]);
	assert!(sim.misses_of(long).is_empty() && sim.misses_of(short).is_empty(), "a retry, not a loss");
	let onces: Vec<_> = sim
		.got
		.iter()
		.filter(|d| matches!(d, Delivery::Once { req, .. } if *req == once))
		.collect();
	assert_eq!(onces.len(), 1);
	assert!(matches!(onces[0], Delivery::Once { result: Ok(data), .. } if *data == [0xBB]));
}

#[test]
fn a_unit_refusing_multi_identifier_requests_is_asked_singly_for_good() {
	let mut fake = FakeUnit::with(&[(0x1000, &[1]), (0x1001, &[2]), (0x1002, &[3])]);
	fake.refuses_multi = true;
	let mut sim = Sim::new(Budget::default(), car(&[(A, fake)]));
	let subs: Vec<SubId> = (0..3).map(|i| sim.p.subscribe(0, Class::Foreground, A, 0x1000 + i, 100, None)).collect();
	sim.run_until(5000);

	let multi: Vec<u64> = sim.sends.iter().filter(|(_, o)| dids_of(&o.pdu).len() > 1).map(|(t, _)| *t).collect();
	assert_eq!(multi, vec![0], "learned once, never re-probed");
	for sub in subs {
		assert!(sim.readings_of(sub).len() >= 49, "{}", sim.readings_of(sub).len());
		assert!(sim.misses_of(sub).is_empty(), "a refused batch is retried, not missed");
	}
}

/// ISO 14229-1 has two refusals that are about the request's shape: `13` (incorrect message
/// length or invalid format) and `14` (response too long). Only they teach single-only; the
/// same NRC for every multi request keeps the unit single-only for good.
#[test]
fn only_a_length_or_format_refusal_teaches_single_only() {
	for nrc in [0x13, 0x14] {
		let mut fake = FakeUnit::with(&[(0x1000, &[1]), (0x1001, &[2])]);
		fake.multi_nrc = Some((nrc, core::cell::Cell::new(u32::MAX)));
		let mut sim = Sim::new(Budget::default(), car(&[(A, fake)]));
		let subs: Vec<SubId> = (0..2).map(|i| sim.p.subscribe(0, Class::Foreground, A, 0x1000 + i, 100, None)).collect();
		sim.run_until(2000);
		let multi = sim.sends.iter().filter(|(_, o)| dids_of(&o.pdu).len() > 1).count();
		assert_eq!(multi, 1, "NRC {nrc:02X}: learned once");
		for sub in subs {
			assert!(sim.readings_of(sub).len() >= 19, "NRC {nrc:02X}");
		}
	}
}

/// Any other NRC to a multi-identifier request — `31` (none of them supported), `21` busy,
/// `22` conditions not correct — says nothing about batching: that batch goes out singly for
/// one round, and the unit is batched again after.
#[test]
fn another_refusal_of_a_multi_request_sends_that_batch_singly_for_one_round() {
	for nrc in [0x21, 0x22, 0x31] {
		let mut fake = FakeUnit::with(&[(0x1000, &[1]), (0x1001, &[2])]);
		fake.multi_nrc = Some((nrc, core::cell::Cell::new(1)));
		let mut sim = Sim::new(Budget::default(), car(&[(A, fake)]));
		let subs: Vec<SubId> = (0..2).map(|i| sim.p.subscribe(0, Class::Foreground, A, 0x1000 + i, 100, None)).collect();
		sim.run_until(2000);
		let shapes: Vec<usize> = sim.sends.iter().map(|(_, o)| dids_of(&o.pdu).len()).collect();
		assert_eq!(shapes[..3], [2, 1, 1], "NRC {nrc:02X}: refused, then that batch singly: {shapes:?}");
		assert!(
			shapes[3..].iter().all(|n| *n == 2),
			"NRC {nrc:02X}: batched again after one round: {shapes:?}"
		);
		for sub in subs {
			assert!(sim.readings_of(sub).len() >= 19, "NRC {nrc:02X}");
			assert!(sim.misses_of(sub).is_empty(), "NRC {nrc:02X}: a retry, not a loss");
		}
	}
}

#[test]
fn a_unit_off_during_its_first_multi_request_is_not_taught_single_only() {
	let mut fake = FakeUnit::with(&[(0x1000, &[1]), (0x1001, &[2])]);
	fake.silent = true;
	let mut sim = Sim::new(Budget::default(), car(&[(A, fake)]));
	let one = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
	let two = sim.p.subscribe(0, Class::Foreground, A, 0x1001, 100, None);
	sim.run_until(600);
	assert!(sim.sends.len() >= 2, "backed off and retried: {:?}", sim.sends);

	// The ignition comes on.
	sim.car.units.get_mut(&A).unwrap().silent = false;
	sim.run_until(5000);
	assert!(
		sim.sends.iter().all(|(_, o)| dids_of(&o.pdu).len() == 2),
		"silence is no evidence against batching: {:?}",
		sim.sends.iter().map(|(t, o)| (*t, dids_of(&o.pdu))).collect::<Vec<_>>()
	);
	assert!(sim.readings_of(one).len() >= 40 && sim.readings_of(two).len() >= 40);
	assert_eq!(sim.misses_of(one).first(), Some(&Miss::NoAnswer));
}

#[test]
fn an_empty_positive_answer_to_a_multi_request_teaches_single_only() {
	let mut fake = FakeUnit::with(&[(0x1000, &[1]), (0x1001, &[2])]);
	fake.refuses_multi_empty = true;
	let mut sim = Sim::new(Budget::default(), car(&[(A, fake)]));
	let one = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
	let two = sim.p.subscribe(0, Class::Foreground, A, 0x1001, 100, None);
	sim.run_until(2000);

	let multi: Vec<u64> = sim.sends.iter().filter(|(_, o)| dids_of(&o.pdu).len() > 1).map(|(t, _)| *t).collect();
	assert_eq!(multi, vec![0]);
	assert!(sim.readings_of(one).len() >= 19 && sim.readings_of(two).len() >= 19);
}

#[test]
fn three_unsplittable_answers_in_a_row_teach_single_only() {
	// 0x1001's echo sits inside 0x1000's record and no length is known: every multi
	// answer reads two ways.
	let fake = FakeUnit::with(&[(0x1000, &[0x10, 0x01, 0xAA]), (0x1001, &[0xBB])]);
	let mut sim = Sim::new(Budget::default(), car(&[(A, fake)]));
	let long = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
	let short = sim.p.subscribe(0, Class::Foreground, A, 0x1001, 100, None);
	sim.run_until(3000);

	let multi = sim.sends.iter().filter(|(_, o)| dids_of(&o.pdu).len() > 1).count();
	assert_eq!(multi, 3, "{:?}", sim.sends.iter().map(|(t, o)| (*t, dids_of(&o.pdu))).collect::<Vec<_>>());
	assert!(sim.readings_of(long).len() >= 29 && sim.readings_of(short).len() >= 29);
	assert!(sim.readings_of(long).iter().all(|(d, _)| *d == [0x10, 0x01, 0xAA]));
}

#[test]
fn a_single_identifier_refusal_misses_that_identifier_and_not_the_unit() {
	let mut sim = Sim::new(Budget::default(), car(&[(A, FakeUnit::with(&[(0x1000, &[1])]))]));
	let refused = sim.p.subscribe(0, Class::Foreground, A, 0x2000, 100, None);
	sim.run_until(1000);
	assert_eq!(sim.sends_to(A, 0, 1000), 10, "no back-off: the unit answered");
	assert_eq!(sim.misses_of(refused), vec![Miss::Refused(0x31); 10]);

	// Batched with one it has, the unsupported one is simply left out of the answer.
	let kept = sim.p.subscribe(1000, Class::Foreground, A, 0x1000, 100, None);
	sim.run_until(1100);
	assert_eq!(dids_of(&sim.sends.last().unwrap().1.pdu).len(), 2);
	assert_eq!(sim.readings_of(kept).len(), 1);
	assert_eq!(sim.misses_of(refused).last(), Some(&Miss::Absent));
}

#[test]
fn one_request_is_in_flight_at_a_time() {
	let mut p = Planner::new(Budget::default());
	p.subscribe(0, Class::Foreground, A, 0x1000, 10, None);
	p.subscribe(0, Class::Foreground, B, 0x1000, 10, None);
	let out = send(p.due(0));
	assert_eq!(p.due(0), Next::Idle { until_ms: None });
	assert_eq!(p.due(500), Next::Idle { until_ms: None }, "however long the answer takes");
	p.answered(500, out.token, Answer::NoAnswer);
	assert!(matches!(p.due(500), Next::Send(_)));
}

#[test]
fn an_answer_with_a_stale_token_changes_nothing() {
	let mut p = Planner::new(Budget::default());
	let sub = p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
	let out = send(p.due(0));
	let wrong = Token(out.token.0 + 1);
	assert!(p.answered(4, wrong, Answer::Pdu(vec![0x62, 0x10, 0x00, 1])).is_empty());
	assert_eq!(p.due(4), Next::Idle { until_ms: None }, "the real request is still in flight");
	let got = p.answered(5, out.token, Answer::Pdu(vec![0x62, 0x10, 0x00, 1]));
	assert_eq!(
		got,
		vec![Delivery::Reading {
			sub,
			unit: A,
			did: 0x1000,
			data: vec![1],
			at_ms: 5
		}]
	);
}

#[test]
fn deliveries_are_stamped_with_the_moment_the_answer_arrived() {
	let mut p = Planner::new(Budget::default());
	p.subscribe(0, Class::Timing, A, 0x1000, 20, None);
	let out = send(p.due(3));
	let got = p.answered(37, out.token, Answer::Pdu(vec![0x62, 0x10, 0x00, 9]));
	assert!(matches!(got[..], [Delivery::Reading { at_ms: 37, .. }]), "{got:?}");
}

#[test]
fn a_one_shot_rides_along_with_a_read_about_to_happen() {
	let mut sim = Sim::new(Budget::default(), car(&[(A, FakeUnit::with(&[(0x1000, &[1])]))]));
	let sub = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 100, None);
	sim.run_until(80);
	assert_eq!(sim.sends.len(), 1);

	let once = sim.p.read_once(80, Class::Remote, A, 0x1000);
	sim.run_until(200);
	assert_eq!(
		sim.sends.iter().map(|(t, _)| *t).collect::<Vec<_>>(),
		vec![0, 80],
		"the 100 ms read went out with the one-shot"
	);
	assert!(
		sim
			.got
			.iter()
			.any(|d| matches!(d, Delivery::Once { req, result: Ok(_), at_ms: 84, .. } if *req == once))
	);
	assert_eq!(sim.readings_of(sub).len(), 2, "and the subscriber had that reading too");
	sim.run_until(201);
	assert_eq!(sim.sends.last().unwrap().0, 200, "on its old phase afterwards");
}

#[test]
fn only_read_only_services_are_accepted_for_a_raw_exchange() {
	let mut p = Planner::new(Budget::default());
	assert!(matches!(
		p.exchange(0, Class::Remote, A, vec![0x2E, 0xF1, 0x90, 0]),
		Err(UdsError::Forbidden(0x2E))
	));
	assert!(matches!(
		p.exchange(0, Class::Remote, A, vec![0x14, 0xFF, 0xFF, 0xFF]),
		Err(UdsError::Forbidden(0x14))
	));
	assert!(p.exchange(0, Class::Remote, A, Vec::new()).is_err());
	assert_eq!(p.due(0), Next::Idle { until_ms: None }, "nothing refused was queued");

	let req = p.exchange(0, Class::Remote, A, vec![0x19, 0x02, 0xFF]).unwrap();
	let out = send(p.due(0));
	assert_eq!(out.pdu, [0x19, 0x02, 0xFF]);
	let got = p.answered(9, out.token, Answer::Refused(0x22));
	assert_eq!(
		got,
		vec![Delivery::Raw {
			req,
			unit: A,
			answer: Answer::Refused(0x22),
			at_ms: 9
		}]
	);
}

#[test]
fn the_shell_can_tell_a_raw_exchange_in_flight_from_a_read() {
	let mut p = Planner::new(Budget::default());
	assert_eq!(p.flying_raw(), None, "nothing in flight");
	let req = p.exchange(0, Class::Foreground, A, vec![0x22, 0xF1, 0x87]).unwrap();
	let out = send(p.due(0));
	assert_eq!(p.flying_raw(), Some(req), "the same bytes a read of F187 would be, and still a raw");
	p.answered(5, out.token, Answer::NoAnswer);
	assert_eq!(p.flying_raw(), None, "answered");

	p.read_once(1000, Class::Foreground, A, 0xF187);
	let out = send(p.due(3000));
	assert_eq!(p.flying_raw(), None, "a read is not a raw");
	p.answered(3001, out.token, Answer::NoAnswer);
}

/// Sends in any `[t, t + 1000)` window, at its worst.
fn busiest_second(times: &[u64]) -> usize {
	times
		.iter()
		.map(|t| times.iter().filter(|u| **u >= *t && **u < t + 1000).count())
		.max()
		.unwrap_or(0)
}

/// Sends to `units` in each whole second from `from_s` to `to_s`.
fn per_second(sim: &Sim, units: &BTreeSet<Unit>, from_s: u64, to_s: u64) -> Vec<usize> {
	(from_s..to_s)
		.map(|s| {
			sim
				.sends
				.iter()
				.filter(|(t, o)| units.contains(&o.unit) && *t >= s * 1000 && *t < (s + 1) * 1000)
				.count()
		})
		.collect()
}

/// A car where every unit in `units` answers `did` with one byte.
fn car_of(units: impl IntoIterator<Item = Unit>, did: u16) -> Car {
	Car {
		units: units.into_iter().map(|u| (u, FakeUnit::with(&[(did, &[0])]))).collect(),
	}
}

/// Keeps `depth` remote raw exchanges queued on `unit`: a laptop reading as fast as it
/// is answered.
fn flood(depth: usize, unit: Unit) -> impl FnMut(&mut Planner, u64, &[Delivery]) {
	let mut outstanding = 0;
	move |p, now, got| {
		outstanding -= got.iter().filter(|d| matches!(d, Delivery::Raw { .. })).count();
		while outstanding < depth {
			p.exchange(now, Class::Remote, unit, vec![0x19, 0x02, 0xFF]).unwrap();
			outstanding += 1;
		}
	}
}

#[test]
fn the_ceiling_holds_in_every_second_of_a_minute() {
	let timing = unit(0x10);
	let fg: Vec<Unit> = (0..4).map(unit).collect();
	let bg: Vec<Unit> = (4..8).map(unit).collect();
	let remote = unit(0x20);
	let mut sim = Sim::new(Budget::default(), car_of(fg.iter().chain(&bg).copied().chain([timing, remote]), 0x1000));
	sim.p.subscribe(0, Class::Timing, timing, 0x1000, 20, None);
	for u in &fg {
		sim.p.subscribe(0, Class::Foreground, *u, 0x1000, 10, None);
	}
	for u in &bg {
		sim.p.subscribe(0, Class::Background, *u, 0x1000, 10, None);
	}
	sim.latency = 1;
	sim.run_until_with(60_000, flood(4, remote));

	let times: Vec<u64> = sim.sends.iter().map(|(t, _)| *t).collect();
	assert!(times.len() > 5_900, "the budget is used, not only respected: {}", times.len());
	assert!(busiest_second(&times) <= 100, "{}", busiest_second(&times));
}

#[test]
fn the_foreground_keeps_its_floor_while_remote_floods() {
	let fg: Vec<Unit> = (0..4).map(unit).collect();
	let remote = unit(0x20);
	let mut sim = Sim::new(Budget::default(), car_of(fg.iter().copied().chain([remote]), 0x1000));
	for u in &fg {
		sim.p.subscribe(0, Class::Foreground, *u, 0x1000, 10, None);
	}
	sim.run_until_with(60_000, flood(4, remote));

	let fg_set: BTreeSet<Unit> = fg.iter().copied().collect();
	let foreground = per_second(&sim, &fg_set, 1, 60);
	assert!(foreground.iter().all(|n| *n >= 25), "{foreground:?}");
	let served = per_second(&sim, &BTreeSet::from([remote]), 1, 60);
	assert!(served.iter().all(|n| *n >= 70), "the laptop is served beside it: {served:?}");
}

#[test]
fn background_is_thinned_first() {
	let fg: Vec<Unit> = (0..8).map(unit).collect();
	let bg: Vec<Unit> = (8..13).map(unit).collect();
	let mut sim = Sim::new(Budget::default(), car_of(fg.iter().chain(&bg).copied(), 0x1000));
	let fg_subs: Vec<SubId> = fg.iter().map(|u| sim.p.subscribe(0, Class::Foreground, *u, 0x1000, 100, None)).collect();
	for u in &bg {
		sim.p.subscribe(0, Class::Background, *u, 0x1000, 100, None);
	}
	sim.run_until(60_000);

	// Asked for: foreground 80/s, background 50/s, against a ceiling of 100.
	for sub in fg_subs {
		assert!(sim.readings_of(sub).len() >= 599, "foreground keeps its whole rate");
	}
	let bg_set: BTreeSet<Unit> = bg.iter().copied().collect();
	let background = per_second(&sim, &bg_set, 1, 60);
	assert!(
		background.iter().all(|n| (15..=21).contains(n)),
		"background gets what is left: {background:?}"
	);
}

/// "Never thinned" in the owner's sense: no timing reading is lost, and one is late only
/// when something starved goes ahead of it — at most once per `starve_after_ms` per starving
/// read, by an exchange or two (PR #2 review, Sched-F1). The background here never gets a
/// slot otherwise, so it starves every five seconds.
#[test]
fn timing_at_50_hz_is_never_thinned() {
	let timing = unit(0x10);
	let fg: Vec<Unit> = (0..4).map(unit).collect();
	let bg: Vec<Unit> = (4..6).map(unit).collect();
	let remote = unit(0x20);
	let mut sim = Sim::new(Budget::default(), car_of(fg.iter().chain(&bg).copied().chain([timing, remote]), 0x1000));
	let speed = sim.p.subscribe(0, Class::Timing, timing, 0x1000, 20, None);
	for u in &fg {
		sim.p.subscribe(0, Class::Foreground, *u, 0x1000, 10, None);
	}
	for u in &bg {
		sim.p.subscribe(0, Class::Background, *u, 0x1000, 10, None);
	}
	sim.run_until_with(60_000, flood(4, remote));

	let readings = sim.readings_of(speed);
	assert!(readings.len() >= 2999, "{}", readings.len());
	let late: Vec<u64> = readings.windows(2).map(|w| w[1].1 - w[0].1).filter(|gap| *gap > 20).collect();
	let starving = bg.len() as u64;
	let windows = 60_000 / u64::from(Budget::default().starve_after_ms) + 1;
	assert!(late.len() as u64 <= starving * windows, "late only for a starved read: {late:?}");
	assert!(late.iter().all(|gap| *gap <= 20 + 2 * (4 + 10)), "and by an exchange or two: {late:?}");
}

/// A timing unit that answers slower than its period is due again the moment it answers.
/// With [`Budget::timing_yields_to_floor`] (the board) the panel under its floor still goes
/// first — all 120 of its reads a minute at 2 Hz, at every latency — and timing gets every
/// other send. Without it (the laptop) timing keeps its place ahead of everything not
/// starved: a unit that answers inside the period leaves the panel gaps, a slower one takes
/// every send but one each time the panel has waited `starve_after_ms`.
#[test]
fn a_slow_timing_unit_yields_to_the_panels_floor_only_where_the_budget_says() {
	let timing = unit(0x10);
	let panel = unit(0x11);
	let cells: Vec<u16> = (0..4).map(|n| 0x2000 + n).collect();
	let records: Vec<(u16, &[u8])> = cells.iter().map(|did| (*did, &[0u8][..])).collect();
	for latency in [5, 25, 45] {
		for yields in [true, false] {
			let budget = Budget {
				timing_yields_to_floor: yields,
				..Budget::default()
			};
			assert_eq!(yields, budget == Budget::board());
			let mut sim = Sim::new(
				budget,
				car(&[(timing, FakeUnit::with(&[(0x1000, &[0])])), (panel, FakeUnit::with(&records))]),
			);
			sim.latency = latency;
			let speed = sim.p.subscribe(0, Class::Timing, timing, 0x1000, 20, Some(1));
			let subs: Vec<SubId> = cells
				.iter()
				.map(|did| sim.p.subscribe(0, Class::Foreground, panel, *did, 500, Some(1)))
				.collect();
			sim.run_until(60_000);

			let label = format!("latency {latency} ms, timing_yields_to_floor {yields}");
			let panel_reads: Vec<usize> = subs.iter().map(|sub| sim.readings_of(*sub).len()).collect();
			let speed_reads = sim.readings_of(speed).len();
			let panel_sends = sim.sends_to(panel, 0, 60_000);
			assert_eq!(
				speed_reads + panel_sends,
				sim.sends.len(),
				"{label}: every send is the panel's or the timing channel's"
			);
			if yields || latency < 20 {
				assert!(
					panel_reads.iter().all(|n| *n >= 120),
					"{label}: the panel keeps its floor: {panel_reads:?}"
				);
			} else {
				let windows = 60_000 / u64::from(budget.starve_after_ms) + 1;
				assert!(
					(1..=windows as usize).contains(&panel_sends),
					"{label}: timing takes every send but the starved panel's, one a window: {panel_sends}"
				);
			}
			if latency < 20 {
				assert!(
					speed_reads >= 2990,
					"{label}: a unit answering inside the period keeps 50 Hz: {speed_reads}"
				);
			}
		}
	}
}

#[test]
fn nothing_is_dropped() {
	let fg: Vec<Unit> = (0..4).map(unit).collect();
	let other = unit(0x30);
	let remote = unit(0x20);
	let mut units: Vec<Unit> = fg.clone();
	units.extend([other, remote]);
	let mut sim = Sim::new(Budget::default(), car_of(units, 0x1000));
	let subs: Vec<SubId> = fg.iter().map(|u| sim.p.subscribe(0, Class::Foreground, *u, 0x1000, 10, None)).collect();

	// Over budget the whole minute, with one-shots and raws of every class arriving.
	let mut asked: Vec<ReqId> = Vec::new();
	let mut next_at = 0;
	let mut n = 0u32;
	let mut flood = flood(4, remote);
	sim.run_until_with(60_000, |p, now, got| {
		flood(p, now, got);
		while now >= next_at {
			next_at += 37;
			n += 1;
			let class = [Class::Timing, Class::Foreground, Class::Remote, Class::Background][(n % 4) as usize];
			asked.push(match n % 3 {
				0 => p.read_once(now, class, other, 0x1000),
				1 => p.read_once(now, class, fg[(n % 4) as usize], 0x1000),
				_ => p.exchange(now, class, other, vec![0x3E, 0x00]).unwrap(),
			});
		}
	});
	// Then the load goes away, and whatever was waiting goes out.
	for sub in subs {
		sim.p.unsubscribe(sub);
	}
	sim.run_until(120_000);

	let mut answered: BTreeMap<ReqId, usize> = BTreeMap::new();
	for d in &sim.got {
		if let Delivery::Once { req, .. } | Delivery::Raw { req, .. } = d {
			*answered.entry(*req).or_default() += 1;
		}
	}
	let remote_raws = answered.len() - asked.iter().filter(|r| answered.contains_key(r)).count();
	assert!(remote_raws > 0);
	assert!(asked.len() > 1500);
	for req in &asked {
		assert_eq!(answered.get(req), Some(&1), "{req:?} answered exactly once");
	}
}

#[test]
fn a_dead_unit_backs_off_without_starving_the_others() {
	let mut dead = FakeUnit::with(&[(0x1000, &[1]), (0x1001, &[2])]);
	dead.silent = true;
	let live = FakeUnit::with(&[(0x1000, &[1]), (0x1001, &[2])]);
	let mut sim = Sim::new(Budget::default(), car(&[(A, dead), (B, live)]));
	let dead_subs: Vec<SubId> = (0..2).map(|i| sim.p.subscribe(0, Class::Foreground, A, 0x1000 + i, 100, None)).collect();
	let live_subs: Vec<SubId> = (0..2).map(|i| sim.p.subscribe(0, Class::Foreground, B, 0x1000 + i, 100, None)).collect();
	sim.run_until(10_000);

	for sub in live_subs {
		assert!(sim.readings_of(sub).len() >= 99, "{}", sim.readings_of(sub).len());
	}
	let tries: Vec<u64> = sim.sends.iter().filter(|(_, o)| o.unit == A).map(|(t, _)| *t).collect();
	assert!(tries.len() <= 10, "backed off, not polled at 10 Hz: {tries:?}");
	let gaps: Vec<u64> = tries.windows(2).map(|w| w[1] - w[0]).collect();
	assert!(gaps.windows(2).all(|g| g[1] + 10 >= g[0]), "the wait only grows: {gaps:?}");
	assert!(gaps.iter().all(|g| *g <= 2000 + 10), "up to the cap: {gaps:?}");
	for sub in dead_subs {
		let misses = sim.misses_of(sub);
		assert!(misses.len() >= 5 && misses.iter().all(|m| *m == Miss::NoAnswer), "{misses:?}");
	}
}

#[test]
fn a_timing_request_is_never_padded() {
	let fake = FakeUnit::with(&[(0x1000, &[1]), (0x1001, &[2]), (0x1002, &[3]), (0x1003, &[4])]);
	let mut sim = Sim::new(Budget::default(), car(&[(A, fake)]));
	let speed = sim.p.subscribe(0, Class::Timing, A, 0x1000, 20, None);
	for did in 0x1001..=0x1003 {
		sim.p.subscribe(0, Class::Foreground, A, did, 20, None);
	}
	// A one-shot of the speed identifier itself still rides with it.
	let once = sim.p.read_once(0, Class::Remote, A, 0x1000);
	sim.run_until(2000);

	for (t, o) in &sim.sends {
		let dids = dids_of(&o.pdu);
		if dids.contains(&0x1000) {
			assert_eq!(dids, vec![0x1000], "at {t}");
		}
	}
	assert!(sim.sends.iter().any(|(_, o)| dids_of(&o.pdu).len() == 3), "the rest still batches");
	assert!(sim.readings_of(speed).len() >= 99);
	assert!(
		sim
			.got
			.iter()
			.any(|d| matches!(d, Delivery::Once { req, result: Ok(_), .. } if *req == once))
	);
}

#[test]
fn a_waiting_read_is_not_starved_forever() {
	let timing = unit(0x10);
	let fg: Vec<Unit> = (0..4).map(unit).collect();
	let slow = unit(0x30);
	let remote = unit(0x20);
	let mut sim = Sim::new(Budget::default(), car_of(fg.iter().copied().chain([timing, slow, remote]), 0x1000));
	sim.p.subscribe(0, Class::Timing, timing, 0x1000, 20, None);
	for u in &fg {
		sim.p.subscribe(0, Class::Foreground, *u, 0x1000, 10, None);
	}
	let background = sim.p.subscribe(0, Class::Background, slow, 0x1000, 100, None);
	sim.run_until_with(60_000, flood(4, remote));

	// Timing takes half the ceiling, the foreground and the laptop want the rest.
	let readings = sim.readings_of(background);
	assert!(readings.len() >= 10, "{}", readings.len());
	let mut at: Vec<u64> = vec![0];
	at.extend(readings.iter().map(|(_, t)| *t));
	let worst = at.windows(2).map(|w| w[1] - w[0]).max().unwrap();
	let starve = Budget::default().starve_after_ms;
	assert!(worst <= u64::from(starve) + 200, "worst wait {worst} ms");
	assert!(busiest_second(&sim.sends.iter().map(|(t, _)| *t).collect::<Vec<_>>()) <= 100);
}

/// PR #2 review (Sched-F1), the reviewer's latency table: `measure` on a unit that answers
/// in 10–30 ms, beside three leading channels at 50 ms on the same unit and two at 100 ms on
/// another, a one-shot and a raw request asked at 5 s. On the laptop they are Foreground and
/// Background; through the board, a host's Remote. A timing read slower than its 20 ms period
/// is always due, and before the fix nothing below it was sent at 21 ms and up: no channel,
/// no one-shot, no raw. Now nothing waits much longer than `starve_after_ms`, and a unit that
/// answers inside the period keeps its timing channel at 45 Hz or better.
#[test]
fn nothing_waits_past_starve_after_behind_a_timing_channel_at_any_latency() {
	const SPEED: u16 = 0xF40D;
	/// The longest subscription period here.
	const PERIOD_MS: u64 = 100;
	const END_MS: u64 = 30_000;
	let leading = [0x2029u16, 0x202A, 0x206E];
	let background = [0x3801u16, 0x3802];
	for (budget, fg, bg) in [
		(Budget::default(), Class::Foreground, Class::Background),
		(Budget::board(), Class::Remote, Class::Remote),
	] {
		for latency in [10, 15, 19, 21, 30] {
			let engine: Vec<(u16, &[u8])> = [SPEED, 0x2001].iter().chain(&leading).map(|d| (*d, &[0u8][..])).collect();
			let gearbox: Vec<(u16, &[u8])> = background.iter().map(|d| (*d, &[0u8][..])).collect();
			let mut sim = Sim::new(budget, car(&[(A, FakeUnit::with(&engine)), (B, FakeUnit::with(&gearbox))]));
			sim.unit_latency.insert(A, latency);
			sim.unit_latency.insert(B, 10);
			let speed = sim.p.subscribe(0, Class::Timing, A, SPEED, 20, None);
			let mut subs: Vec<SubId> = leading.iter().map(|d| sim.p.subscribe(0, fg, A, *d, 50, None)).collect();
			subs.extend(background.iter().map(|d| sim.p.subscribe(0, bg, B, *d, PERIOD_MS as u32, None)));
			sim.run_until(5_000);
			let asked = sim.now;
			let once = sim.p.read_once(asked, fg, A, 0x2001);
			let raw = sim.p.exchange(asked, fg, B, vec![0x19, 0x02, 0xAF]).unwrap();
			sim.run_until(END_MS);

			let label = format!("yields {}, latency {latency} ms", budget.timing_yields_to_floor);
			let starve = u64::from(budget.starve_after_ms);
			for (what, req) in [("one-shot", once), ("raw", raw)] {
				let waited = sim.delivered_at(req).map(|t| t - asked);
				assert!(
					waited.is_some_and(|w| w <= starve + PERIOD_MS),
					"{label}: the {what} was delivered after {waited:?} ms"
				);
			}
			for sub in &subs {
				let mut at = vec![0];
				at.extend(sim.readings_of(*sub).iter().map(|(_, t)| *t));
				at.push(END_MS.max(*at.last().unwrap()));
				let worst = at.windows(2).map(|w| w[1] - w[0]).max().unwrap();
				assert!(worst <= starve + 2 * PERIOD_MS, "{label}: {sub:?} waited {worst} ms for a reading");
			}
			let hz = sim.readings_of(speed).len() as u64 * 1000 / END_MS;
			if latency < 20 {
				assert!(hz >= 45, "{label}: the timing channel ran at {hz} Hz");
			}
		}
	}
}

/// A unit's raw exchanges rank by their own class, not by the one in front: a guard's speed
/// check (Timing) queued behind a host's raw (Remote) on the engine, with a timing channel
/// on the same slow engine, goes out at once — before the fix neither went, ever.
#[test]
fn a_timing_raw_is_not_held_behind_a_waiting_raw_of_its_unit() {
	let engine = FakeUnit::with(&[(0xF40D, &[0])]);
	let panel = FakeUnit::with(&[(0x3801, &[0]), (0x3802, &[0])]);
	for budget in [Budget::default(), Budget::board()] {
		let mut sim = Sim::new(budget, car(&[(A, engine.clone()), (B, panel.clone())]));
		sim.unit_latency.insert(A, 25);
		sim.unit_latency.insert(B, 10);
		for did in [0x3801, 0x3802] {
			sim.p.subscribe(0, Class::Foreground, B, did, 100, None);
		}
		sim.p.subscribe(0, Class::Timing, A, 0xF40D, 20, None);
		sim.run_until(1_000);
		let asked = sim.now;
		let faults = sim.p.exchange(asked, Class::Remote, A, vec![0x19, 0x02, 0xAF]).unwrap();
		let speed = sim.p.exchange(asked + 1, Class::Timing, A, vec![0x22, 0xF4, 0x0D]).unwrap();
		sim.run_until(asked + 10_000);

		let label = format!("yields {}", budget.timing_yields_to_floor);
		let speed_waited = sim.delivered_at(speed).map(|t| t - asked);
		assert!(speed_waited.is_some_and(|w| w <= 100), "{label}: speed check after {speed_waited:?} ms");
		let faults_waited = sim.delivered_at(faults).map(|t| t - asked);
		assert!(
			faults_waited.is_some_and(|w| w <= u64::from(budget.starve_after_ms) + 100),
			"{label}: the raw in front after {faults_waited:?} ms"
		);
	}
}

#[test]
fn a_remote_client_with_20_subscriptions_disconnects() {
	let records: Vec<(u16, Vec<u8>)> = (0..10).map(|i| (0x1000 + i, vec![i as u8])).collect();
	let unit_records: Vec<(u16, &[u8])> = records.iter().map(|(d, r)| (*d, r.as_slice())).collect();
	let mut sim = Sim::new(
		Budget::default(),
		car(&[(A, FakeUnit::with(&unit_records)), (B, FakeUnit::with(&unit_records))]),
	);
	let panel_a = sim.p.subscribe(0, Class::Foreground, A, 0x1000, 500, None);
	let panel_b = sim.p.subscribe(0, Class::Foreground, B, 0x1000, 500, None);
	let client: Vec<SubId> = [A, B]
		.iter()
		.flat_map(|u| records.iter().map(move |(did, _)| (*u, *did)))
		.map(|(u, did)| sim.p.subscribe(0, Class::Remote, u, did, 50, None))
		.collect();
	assert_eq!(client.len(), 20);
	sim.run_until(2000);

	// The link drops: every handle of that client is dropped at once.
	let gone_at = sim.now;
	for sub in client {
		sim.p.unsubscribe(sub);
	}
	sim.run_until(8000);

	let after: Vec<&(u64, Outgoing)> = sim.sends.iter().filter(|(t, _)| *t >= gone_at).collect();
	assert!(after.iter().all(|(_, o)| dids_of(&o.pdu) == [0x1000]), "{after:?}");
	for (u, panel) in [(A, panel_a), (B, panel_b)] {
		assert_eq!(sim.sends_to(u, 3000, 8000), 10, "the panel's 500 ms period, unchanged");
		assert!(sim.readings_of(panel).iter().filter(|(_, t)| *t >= 3000).count() >= 10);
	}
}
