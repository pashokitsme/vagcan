//! The session against a real planner and a simulated bus.
//!
//! Identifiers and record bytes are synthetic, except road speed's `22 F40D` on
//! `7E0`, which is the protocol's (SAE J1979 over ISO 15765-4) and what the guard
//! reads.

use alloc::vec;
use alloc::vec::Vec;

use super::*;
use crate::guard::{RATE_LIMIT, RATE_WINDOW_MS};
use crate::schedule::{Answer as BusAnswer, Budget, Next, Outgoing};

const ENGINE: Unit = SPEED_UNIT;
const GATEWAY: Unit = Unit {
	request: 0x710,
	response: 0x77A,
};

/// What the simulated bus answers.
#[derive(Clone, Copy)]
enum Bus {
	/// Nobody on the pair: every request times out.
	Silent,
	/// Every unit answers every read with one byte, and road speed with this.
	Answering { kmh: u8 },
}

impl Bus {
	fn answer(self, out: &Outgoing) -> BusAnswer {
		match self {
			Bus::Silent => BusAnswer::NoAnswer,
			Bus::Answering { kmh } => match out.pdu.as_slice() {
				[0x22, 0xF4, 0x0D] => BusAnswer::Pdu(vec![0x62, 0xF4, 0x0D, kmh]),
				[0x22, rest @ ..] => {
					let mut pdu = vec![0x62];
					for did in rest.chunks_exact(2) {
						pdu.extend_from_slice(did);
						pdu.push(0x2A);
					}
					BusAnswer::Pdu(pdu)
				}
				[sid, rest @ ..] => {
					let mut pdu = vec![sid + 0x40];
					pdu.extend_from_slice(rest);
					BusAnswer::Pdu(pdu)
				}
				[] => BusAnswer::BusError,
			},
		}
	}
}

/// The firmware's loop, on a clock that only moves when told to.
struct Board {
	planner: Planner,
	session: Session,
	bus: Bus,
	now: u64,
	sent: Vec<(u64, Outgoing)>,
	to_host: Vec<Message>,
}

impl Board {
	fn new(bus: Bus) -> Self {
		Board {
			planner: Planner::new(Budget::default()),
			session: Session::new(),
			bus,
			now: 0,
			sent: Vec::new(),
			to_host: Vec::new(),
		}
	}

	fn hear(&mut self, message: Message) {
		let out = self.session.push(self.now, &mut self.planner, message);
		self.to_host.extend(out);
	}

	/// Run the bus and the session until `end`, as the shell does.
	fn run_until(&mut self, end: u64) {
		while self.now < end {
			let out = self.session.poll(self.now, &mut self.planner);
			self.to_host.extend(out);
			match self.planner.due(self.now) {
				Next::Send(out) => {
					let answer = self.bus.answer(&out);
					self.sent.push((self.now, out.clone()));
					self.now += 5;
					for delivery in self.planner.answered(self.now, out.token, answer) {
						match &delivery {
							Delivery::Raw { req, answer, at_ms, .. } if self.session.awaiting() == Some(*req) => {
								let out = self.session.answered(*at_ms, &mut self.planner, *req, answer);
								self.to_host.extend(out);
							}
							other => self.to_host.extend(self.session.deliver(other)),
						}
					}
				}
				Next::Idle { until_ms } => {
					let wake = [until_ms, self.session.wake_at()].into_iter().flatten().min();
					self.now = wake.unwrap_or(end).clamp(self.now + 1, end);
				}
			}
		}
	}

	fn pdus_sent(&self) -> Vec<Vec<u8>> {
		self.sent.iter().map(|(_, o)| o.pdu.clone()).collect()
	}

	fn answers(&self) -> Vec<(u8, Outcome)> {
		self
			.to_host
			.iter()
			.filter_map(|m| match m {
				Message::Answer(a) => Some((a.seq, a.outcome.clone())),
				_ => None,
			})
			.collect()
	}

	fn readings(&self, sub: u16) -> Vec<(u32, Outcome)> {
		self
			.to_host
			.iter()
			.filter_map(|m| match m {
				Message::Reading(r) if r.sub == sub => Some((r.at_ms, r.outcome.clone())),
				_ => None,
			})
			.collect()
	}
}

fn request(seq: u8, unit: Unit, pdu: &[u8]) -> Message {
	Message::Request(Request {
		seq,
		request_id: unit.request,
		response_id: unit.response,
		pdu: pdu.to_vec(),
	})
}

fn subscribe(sub: u16, unit: Unit, did: u16, period_ms: u16) -> Message {
	Message::Subscribe(Subscribe {
		sub,
		request_id: unit.request,
		response_id: unit.response,
		did,
		period_ms,
	})
}

fn refused(outcome: &Outcome) -> &str {
	match outcome {
		Outcome::Refused(text) => text,
		other => panic!("expected a refusal, got {other:?}"),
	}
}

#[test]
fn a_read_is_forwarded_and_its_answer_comes_back_under_its_seq() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(request(7, ENGINE, &[0x22, 0xF1, 0x90]));
	board.run_until(100);
	assert_eq!(board.sent.len(), 1);
	assert_eq!(board.sent[0].1.unit, ENGINE);
	assert_eq!(board.pdus_sent(), [vec![0x22, 0xF1, 0x90]]);
	assert_eq!(board.answers(), [(7, Outcome::Pdu(vec![0x62, 0xF1, 0x90, 0x2A]))]);
}

#[test]
fn silence_is_no_answer_and_a_unit_outside_the_plan_is_asked_on_its_own_ids() {
	let mut board = Board::new(Bus::Silent);
	board.hear(request(1, GATEWAY, &[0x22, 0xF1, 0x87]));
	board.run_until(100);
	assert_eq!(board.sent[0].1.unit, GATEWAY);
	assert_eq!(board.answers(), [(1, Outcome::NoAnswer)]);
}

#[test]
fn a_write_and_a_programming_session_are_refused_and_nothing_reaches_the_bus() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(request(1, ENGINE, &[0x2E, 0xF1, 0x90, 0x00]));
	board.hear(request(2, ENGINE, &[0x10, 0x02]));
	board.hear(request(3, ENGINE, &[0x10, 0x82]));
	board.run_until(1000);
	assert!(board.sent.is_empty(), "{:02X?}", board.pdus_sent());
	let answers = board.answers();
	assert_eq!(answers.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(), [1, 2, 3]);
	assert!(refused(&answers[0].1).contains("0x2E"), "{answers:?}");
	assert!(refused(&answers[1].1).contains("programming"), "{answers:?}");
	assert!(refused(&answers[2].1).contains("programming"), "{answers:?}");
}

#[test]
fn an_extended_session_reads_road_speed_first_and_silence_is_moving() {
	let mut board = Board::new(Bus::Silent);
	board.hear(request(4, GATEWAY, &[0x10, 0x03]));
	board.run_until(1000);
	assert_eq!(board.pdus_sent(), [vec![0x22, 0xF4, 0x0D]], "the speed read only, to the engine");
	assert_eq!(board.sent[0].1.unit, ENGINE);
	let answers = board.answers();
	assert_eq!(answers.len(), 1);
	assert!(refused(&answers[0].1).contains("did not report road speed"), "{answers:?}");
}

#[test]
fn an_extended_session_on_a_moving_car_is_refused_and_on_a_standing_one_forwarded() {
	let mut moving = Board::new(Bus::Answering { kmh: 30 });
	moving.hear(request(1, GATEWAY, &[0x10, 0x03]));
	moving.run_until(1000);
	assert_eq!(moving.pdus_sent(), [vec![0x22, 0xF4, 0x0D]]);
	assert!(refused(&moving.answers()[0].1).contains("30 km/h"));

	let mut standing = Board::new(Bus::Answering { kmh: 0 });
	standing.hear(request(2, GATEWAY, &[0x10, 0x03]));
	standing.run_until(1000);
	assert_eq!(standing.pdus_sent(), [vec![0x22, 0xF4, 0x0D], vec![0x10, 0x03]]);
	assert_eq!(standing.sent[1].1.unit, GATEWAY);
	assert_eq!(standing.answers(), [(2, Outcome::Pdu(vec![0x50, 0x03]))]);
}

#[test]
fn the_speed_read_goes_ahead_of_the_panels_reads() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	let a = board.planner.subscribe(0, Class::Foreground, ENGINE, 0x1000, 10, None);
	let b = board.planner.subscribe(0, Class::Foreground, GATEWAY, 0x1001, 10, None);
	board.run_until(50);
	board.hear(request(1, GATEWAY, &[0x10, 0x03]));
	let before = board.sent.len();
	board.run_until(80);
	assert_eq!(board.sent[before].1.pdu, [0x22, 0xF4, 0x0D], "{:02X?}", &board.pdus_sent()[before..]);
	board.planner.unsubscribe(a);
	board.planner.unsubscribe(b);
}

#[test]
fn requests_are_answered_one_at_a_time_in_order() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(request(1, ENGINE, &[0x22, 0x10, 0x00]));
	board.hear(request(2, GATEWAY, &[0x22, 0x10, 0x01]));
	assert_eq!(board.session.queued(), 2);
	board.run_until(100);
	assert_eq!(board.answers().iter().map(|(seq, _)| *seq).collect::<Vec<_>>(), [1, 2]);
	assert!(board.sent[0].0 < board.sent[1].0);
	assert_eq!(board.session.queued(), 0);
}

#[test]
fn over_the_rate_cap_a_request_waits_and_is_then_sent_never_dropped() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	for seq in 0..=RATE_LIMIT as u8 {
		// One identifier over and over: a walk is not what this is about.
		board.hear(request(seq, ENGINE, &[0x22, 0x10, 0x00]));
	}
	board.run_until(RATE_WINDOW_MS - 1);
	assert_eq!(board.answers().len(), RATE_LIMIT as usize, "the last one waits out the window");
	assert!(board.session.wake_at().is_some());
	board.run_until(RATE_WINDOW_MS + 1000);
	assert_eq!(board.answers().len(), RATE_LIMIT as usize + 1);
	assert!(board.answers().iter().all(|(_, o)| matches!(o, Outcome::Pdu(_))));
}

#[test]
fn a_subscription_streams_whole_positive_pdus_stamped_on_arrival_until_unsubscribed() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(subscribe(9, ENGINE, 0xF40D, 100));
	board.run_until(1000);
	let readings = board.readings(9);
	assert_eq!(readings.len(), 10, "{readings:?}");
	assert!(readings.iter().all(|(_, o)| *o == Outcome::Pdu(vec![0x62, 0xF4, 0x0D, 0])));
	let arrivals: Vec<u64> = board.sent.iter().map(|(t, _)| t + 5).collect();
	assert_eq!(readings.iter().map(|(at, _)| u64::from(*at)).collect::<Vec<_>>(), arrivals);

	board.hear(Message::Unsubscribe { sub: 9 });
	let sent = board.sent.len();
	board.run_until(3000);
	assert_eq!(board.sent.len(), sent, "nothing is read for nobody");
	assert_eq!(board.readings(9).len(), 10);
}

#[test]
fn a_silent_or_refusing_unit_reads_as_no_answer_or_its_negative_response() {
	let mut board = Board::new(Bus::Silent);
	board.hear(subscribe(1, ENGINE, 0x1000, 100));
	board.run_until(300);
	assert!(board.readings(1).iter().all(|(_, o)| *o == Outcome::NoAnswer));
	assert!(!board.readings(1).is_empty());

	let session = Session {
		subs: [(
			5,
			Live {
				id: SubId(77),
				request_id: 0x7E0,
			},
		)]
		.into_iter()
		.collect(),
		..Session::new()
	};
	let refused = Delivery::Missed {
		sub: SubId(77),
		unit: ENGINE,
		did: 0x1000,
		why: Miss::Refused(0x31),
		at_ms: 12,
	};
	assert_eq!(
		session.deliver(&refused),
		Some(Message::Reading(Reading {
			sub: 5,
			at_ms: 12,
			outcome: Outcome::Pdu(vec![0x7F, 0x22, 0x31])
		}))
	);
	let someone_elses = Delivery::Missed {
		sub: SubId(78),
		unit: ENGINE,
		did: 0x1000,
		why: Miss::NoAnswer,
		at_ms: 12,
	};
	assert_eq!(session.deliver(&someone_elses), None);
}

#[test]
fn a_subscription_the_guard_refuses_is_a_refused_reading_and_nothing_is_polled() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(subscribe(3, ENGINE, 0xF40D, 5));
	board.run_until(500);
	assert!(board.sent.is_empty());
	let readings = board.readings(3);
	assert_eq!(readings.len(), 1);
	assert!(refused(&readings[0].1).contains("period"), "{readings:?}");
}

#[test]
fn a_walk_locks_the_unit_and_ends_its_subscriptions_but_not_other_units() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(subscribe(1, ENGINE, 0x2000, 100));
	board.hear(subscribe(2, GATEWAY, 0x2000, 100));
	board.hear(request(1, ENGINE, &[0x22, 0x20, 0x01, 0x20, 0x02, 0x20, 0x03]));
	board.run_until(100);
	board.hear(request(2, ENGINE, &[0x22, 0x20, 0x04, 0x20, 0x05, 0x20, 0x06, 0x20, 0x07]));
	board.run_until(200);
	let answers = board.answers();
	assert!(matches!(answers[0].1, Outcome::Pdu(_)));
	assert!(refused(&answers[1].1).contains("evenly spaced"), "{answers:?}");
	let last = board.readings(1).pop().unwrap();
	assert!(refused(&last.1).contains("locked"), "{last:?}");

	let before = board.sent.len();
	board.run_until(2000);
	assert!(
		board.sent[before..].iter().all(|(_, o)| o.unit == GATEWAY),
		"the engine is no longer polled"
	);
	assert!(board.sent[before..].len() >= 10, "the gateway still is");
}

#[test]
fn closing_ends_every_subscription_and_forgets_the_queue() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(subscribe(1, ENGINE, 0x1000, 50));
	board.hear(subscribe(2, GATEWAY, 0x1000, 50));
	board.run_until(200);
	board.hear(request(1, ENGINE, &[0x10, 0x03]));
	board.session.close(&mut board.planner);
	let before = board.sent.len();
	board.run_until(2000);
	let after: Vec<&[u8]> = board.sent[before..].iter().map(|(_, o)| o.pdu.as_slice()).collect();
	assert!(after.len() <= 1, "at most the exchange already handed over: {after:02X?}");
	assert_eq!(board.session.queued(), 0);
	assert_eq!(board.session.awaiting(), None);
}

#[test]
fn a_replaced_subscription_polls_once_at_the_new_period() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(subscribe(1, ENGINE, 0x1000, 100));
	board.hear(subscribe(1, ENGINE, 0x1000, 500));
	board.run_until(1000);
	assert_eq!(board.sent.len(), 2, "{:?}", board.sent.iter().map(|(t, _)| *t).collect::<Vec<_>>());
}
