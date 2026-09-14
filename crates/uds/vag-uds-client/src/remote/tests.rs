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
		// What the board's shell reports for a request that asked for silence and got it.
		if crate::schedule::expects_no_answer(&out.pdu) {
			return BusAnswer::NotExpected;
		}
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
	/// How long the bus takes to answer each request, in ms.
	latency: u64,
	sent: Vec<(u64, Outgoing)>,
	to_host: Vec<Message>,
}

impl Board {
	/// The firmware's budget ([`Budget::board`]); the bus answers in 5 ms.
	fn new(bus: Bus) -> Self {
		Board {
			planner: Planner::new(Budget::board()),
			session: Session::new(),
			bus,
			now: 0,
			latency: 5,
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
					self.now += self.latency;
					for delivery in self.planner.answered(self.now, out.token, answer) {
						match &delivery {
							Delivery::Raw { req, answer, at_ms, .. } if self.session.awaiting() == Some(*req) => {
								let out = self.session.answered(*at_ms, &mut self.planner, *req, answer);
								self.to_host.extend(out);
							}
							other => {
								let msg = self.session.deliver(self.now, &mut self.planner, other);
								self.to_host.extend(msg);
							}
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
	subscribe_as(sub, unit, did, period_ms, Priority::Normal)
}

fn timing(sub: u16, unit: Unit, did: u16, period_ms: u16) -> Message {
	subscribe_as(sub, unit, did, period_ms, Priority::Timing)
}

fn subscribe_as(sub: u16, unit: Unit, did: u16, period_ms: u16, priority: Priority) -> Message {
	Message::Subscribe(Subscribe {
		sub,
		request_id: unit.request,
		response_id: unit.response,
		did,
		period_ms,
		priority,
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

/// PR #2 review (S-F2), the reviewer's proof reversed. With a timing channel on a unit slower
/// than its period, the session change the speed read cleared was queued as Remote and waited
/// behind it for a minute, then went out on a car answering 90 km/h with no fresh read. Now it
/// goes out while its speed reading is fresh, and never later without another one.
#[test]
fn a_speed_cleared_session_change_goes_out_while_the_reading_is_fresh_beside_a_slow_timing_channel() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	// A unit slower than the timing period: 30 ms to answer a 20 ms subscription.
	board.latency = 30;
	board.hear(timing(1, GATEWAY, 0x1000, 20));
	board.run_until(200);
	board.hear(request(9, GATEWAY, &[0x10, 0x03]));
	board.run_until(2_000);
	let speed_at = board.sent.iter().find(|(_, o)| o.pdu == [0x22, 0xF4, 0x0D]).map(|(t, _)| *t);
	let session_at = board.sent.iter().find(|(_, o)| o.pdu == [0x10, 0x03]).map(|(t, _)| *t);
	let fresh = speed_at.zip(session_at).map(|(speed, session)| session - (speed + board.latency));
	assert!(
		fresh.is_some_and(|after| after <= crate::guard::SPEED_FRESH_MS),
		"speed read at {speed_at:?}, 10 03 at {session_at:?}"
	);
	assert_eq!(board.answers(), [(9, Outcome::Pdu(vec![0x50, 0x03]))]);

	// The car drives off; the run ends. Nothing else is ever sent as a session change.
	board.bus = Bus::Answering { kmh: 90 };
	board.run_until(60_000);
	board.hear(Message::Unsubscribe { sub: 1 });
	board.run_until(61_000);
	assert_eq!(board.sent.iter().filter(|(_, o)| o.pdu == [0x10, 0x03]).count(), 1);
}

/// A session change the speed read cleared, that cannot go out within `SPEED_FRESH_MS` of the
/// speed answer — the bus is busy elsewhere — is not sent: the road speed is read again, and
/// on a car now moving it is refused. Whichever of the shell's two loops looks first.
#[test]
fn a_session_change_that_misses_its_fresh_speed_reading_is_not_sent_and_speed_is_read_again() {
	for due_first in [true, false] {
		let mut planner = Planner::new(Budget::board());
		let mut session = Session::new();
		let mut to_host = session.push(0, &mut planner, request(1, GATEWAY, &[0x10, 0x03]));
		let answer = |planner: &mut Planner, session: &mut Session, at: u64, kmh: u8| {
			let Next::Send(out) = planner.due(at - 5) else {
				panic!("nothing to send")
			};
			assert_eq!(out.pdu, [0x22, 0xF4, 0x0D], "due first {due_first}");
			let mut messages = Vec::new();
			for delivery in planner.answered(at, out.token, BusAnswer::Pdu(vec![0x62, 0xF4, 0x0D, kmh])) {
				if let Delivery::Raw { req, answer, .. } = &delivery {
					messages.extend(session.answered(at, planner, *req, answer));
				}
			}
			messages
		};
		to_host.extend(answer(&mut planner, &mut session, 20, 0));

		// The bus was busy elsewhere until one millisecond past the reading's freshness.
		let late = 20 + crate::guard::SPEED_FRESH_MS + 1;
		let sent_late = |planner: &mut Planner| match planner.due(late) {
			Next::Send(out) => Some(out),
			Next::Idle { .. } => None,
		};
		let out = if due_first {
			let out = sent_late(&mut planner);
			to_host.extend(session.poll(late, &mut planner));
			out.or_else(|| sent_late(&mut planner))
		} else {
			to_host.extend(session.poll(late, &mut planner));
			sent_late(&mut planner)
		};
		let out = out.expect("a fresh speed read goes out");
		assert_eq!(out.pdu, [0x22, 0xF4, 0x0D], "due first {due_first}: not the stale session change");

		// Meanwhile the car drove off.
		let at = late + 5;
		for delivery in planner.answered(at, out.token, BusAnswer::Pdu(vec![0x62, 0xF4, 0x0D, 90])) {
			if let Delivery::Raw { req, answer, .. } = &delivery {
				to_host.extend(session.answered(at, &mut planner, *req, answer));
			}
		}
		assert!(matches!(planner.due(at + 100), Next::Idle { .. }), "due first {due_first}");
		let [Message::Answer(answer)] = to_host.as_slice() else {
			panic!("due first {due_first}: {to_host:?}")
		};
		assert!(refused(&answer.outcome).contains("90 km/h"), "{answer:?}");
	}
}

#[test]
fn the_speed_read_goes_ahead_of_the_panels_reads() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	let a = board.planner.subscribe(0, Class::Foreground, ENGINE, 0x1000, 10, None);
	let b = board.planner.subscribe(0, Class::Foreground, GATEWAY, 0x1001, 10, None);
	// A second in, the panel has had its floor. Under it, the board's budget sends the panel
	// first (`Budget::timing_yields_to_floor`), and the speed read waits at most for that.
	board.run_until(1000);
	board.hear(request(1, GATEWAY, &[0x10, 0x03]));
	let before = board.sent.len();
	board.run_until(1030);
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

	let mut planner = Planner::new(Budget::board());
	let mut session = Session {
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
		session.deliver(0, &mut planner, &refused),
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
	assert_eq!(session.deliver(0, &mut planner, &someone_elses), None);
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
	board.session.close(board.now, &mut board.planner);
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
/// The heap attack from across the radio: one request id paired with every response id,
/// subscribed as fast as the link carries it. One passes; the planner and the guard hold
/// one unit, not two thousand.
#[test]
fn subscribing_one_request_id_under_every_response_id_leaves_memory_bounded() {
	let mut board = Board::new(Bus::Silent);
	let mut refused = 0;
	for response_id in 0..=0x7FFu16 {
		board.hear(Message::Subscribe(Subscribe {
			sub: 1,
			request_id: 0x7E0,
			response_id,
			did: 0xF40D,
			period_ms: 100,
			priority: Priority::Normal,
		}));
		refused += board
			.readings(1)
			.iter()
			.filter(|(_, o)| matches!(o, Outcome::Refused(why) if why.contains("answers on")))
			.count();
		board.to_host.clear();
	}
	assert_eq!(refused, 0x7FF, "all but the first response id");
	// `sub: 1` given again replaces the live one before the guard refuses the new
	// pair, so the first subscription is gone too: at most one unit is ever held.
	assert!(board.planner.units_held() <= 1, "{} units held", board.planner.units_held());
	board.session.close(board.now, &mut board.planner);
	assert_eq!(board.planner.units_held(), 0);
}

/// A host that leaves takes its queued request with it: nothing it asked reaches the car
/// after the connection is gone.
#[test]
fn closing_cancels_the_request_the_planner_has_not_sent() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(request(1, GATEWAY, &[0x22, 0xF1, 0x87]));
	assert!(board.session.awaiting().is_some(), "handed to the planner");
	board.session.close(board.now, &mut board.planner);
	board.run_until(2000);
	assert!(board.sent.is_empty(), "{:02X?}", board.pdus_sent());
}

/// `3E 80` answered with the silence it asked for: the host is told no answer came, and
/// the panel reading the same unit keeps its rate.
#[test]
fn a_suppressed_positive_response_is_no_answer_and_the_unit_is_not_backed_off() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	let panel = board.planner.subscribe(0, Class::Foreground, ENGINE, 0x1000, 100, None);
	board.run_until(100);
	board.hear(request(3, ENGINE, &[0x3E, 0x80]));
	board.run_until(1000);
	assert_eq!(board.answers(), [(3, Outcome::NoAnswer)]);
	let panel_reads = board.sent.iter().filter(|(_, o)| o.pdu == [0x22, 0x10, 0x00]).count();
	assert!(panel_reads >= 9, "the panel kept reading: {panel_reads} in 1 s");
	board.planner.unsubscribe(panel);
}

// --- the USB cable's session, and the board's adapter mode -------------------------

const ADAPTER: &str = "the board is in adapter mode";

/// In adapter mode nothing a host asks reaches the guard or the bus; the refusal says why.
#[test]
fn in_adapter_mode_requests_and_subscriptions_are_refused_and_nothing_is_sent() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	let out = board
		.session
		.push_refused(0, &mut board.planner, request(4, ENGINE, &[0x22, 0xF1, 0x90]), ADAPTER);
	let [Message::Answer(answer)] = out.as_slice() else { panic!("{out:?}") };
	assert_eq!((answer.seq, refused(&answer.outcome)), (4, ADAPTER));
	let out = board
		.session
		.push_refused(7, &mut board.planner, subscribe(9, ENGINE, 0x2000, 100), ADAPTER);
	let [Message::Reading(reading)] = out.as_slice() else {
		panic!("{out:?}")
	};
	assert_eq!((reading.sub, reading.at_ms, refused(&reading.outcome)), (9, 7, ADAPTER));
	assert!(board.session.push_refused(0, &mut board.planner, Message::Hello, ADAPTER).is_empty());
	board.run_until(1000);
	assert!(board.sent.is_empty(), "{:02X?}", board.pdus_sent());
	assert!(!board.session.is_active());
}

/// A subscription given again in adapter mode ends the live one, and an unsubscribe still frees.
#[test]
fn in_adapter_mode_a_live_subscription_can_still_be_replaced_or_dropped() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(subscribe(1, ENGINE, 0x2000, 100));
	board.hear(subscribe(2, ENGINE, 0x2001, 100));
	assert_eq!(board.session.subscriptions().count(), 2);
	board
		.session
		.push_refused(0, &mut board.planner, subscribe(1, ENGINE, 0x2002, 100), ADAPTER);
	board
		.session
		.push_refused(0, &mut board.planner, Message::Unsubscribe { sub: 2 }, ADAPTER);
	assert_eq!(board.session.subscriptions().count(), 0);
	board.run_until(1000);
	assert!(board.sent.is_empty(), "{:02X?}", board.pdus_sent());
}

/// Entering adapter mode refuses what the host waits for, in order, and keeps its
/// subscriptions for when the bus comes back.
#[test]
fn refusing_the_pending_answers_each_request_once_and_keeps_the_subscriptions() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(subscribe(5, ENGINE, 0x2000, 100));
	board.hear(request(1, GATEWAY, &[0x22, 0xF1, 0x87]));
	board.hear(request(2, GATEWAY, &[0x22, 0xF1, 0x89]));
	assert_eq!(board.session.queued(), 2);
	let refusals = board.session.refuse_pending(&mut board.planner, ADAPTER);
	board.to_host.extend(refusals);
	let answers = board.answers();
	assert_eq!(answers.len(), 2, "{answers:?}");
	assert_eq!((answers[0].0, refused(&answers[0].1)), (1, ADAPTER));
	assert_eq!((answers[1].0, refused(&answers[1].1)), (2, ADAPTER));
	assert_eq!(board.session.queued(), 0);
	assert!(board.session.is_active(), "the subscription is still held");
	board.run_until(1000);
	assert!(
		board.sent.iter().all(|(_, o)| o.unit == ENGINE),
		"the refused requests never went out: {:02X?}",
		board.pdus_sent()
	);
	assert!(!board.readings(5).is_empty(), "the subscription is read again");
}

#[test]
fn queued_bytes_count_what_waits_here_and_not_what_the_planner_holds() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	assert_eq!(board.session.queued_bytes(), 0);
	let big: Vec<u8> = core::iter::once(0x22)
		.chain((0..200u16).flat_map(|d| (0x2000 + d).to_be_bytes()))
		.collect();
	board.hear(request(1, GATEWAY, &[0x22, 0xF1, 0x87]));
	board.hear(request(2, GATEWAY, &big));
	board.hear(request(3, GATEWAY, &[0x3E, 0x00]));
	// The first is the planner's; the other two wait here.
	assert_eq!(board.session.queued_bytes(), big.len() + 2);
	board.run_until(2000);
	assert_eq!(board.session.queued_bytes(), 0, "{:?}", board.answers());
}

#[test]
fn a_session_is_active_while_it_holds_a_subscription_or_a_request() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	assert!(!board.session.is_active());
	board.hear(subscribe(1, ENGINE, 0x2000, 100));
	assert!(board.session.is_active());
	board.hear(Message::Unsubscribe { sub: 1 });
	assert!(!board.session.is_active());
	board.hear(request(1, ENGINE, &[0x22, 0xF1, 0x90]));
	assert!(board.session.is_active());
	board.run_until(100);
	assert_eq!(board.answers().len(), 1);
	assert!(!board.session.is_active(), "answered, nothing held");
}

// --- timing subscriptions -----------------------------------------------------------

/// The identifiers of a `22` request.
fn dids_of(pdu: &[u8]) -> Vec<u16> {
	pdu[1..].chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect()
}

/// The bench's `measure` over the board (2026-09-14): every host subscription ran as the
/// board's Remote class, the planner sat at its ceiling, and the speed channel came at
/// 10 Hz. Marked timing, beside fifteen normal channels and the panel, under the board's
/// budget: the panel keeps everything it asks (under its floor) at every answer latency,
/// and no second holds more than the ceiling. A bus that answers in 5 ms gives the timing
/// channel its 50 Hz and the normal channels slowed, not dropped. One that answers in 25
/// or 45 ms — slower than the period, as a unit behind the gateway may — leaves the timing
/// read always due, and it gets what the panel leaves. Under one identifier per request
/// too, where every read is an exchange of its own; on either carrier's guard.
#[test]
fn a_timing_subscription_beside_fifteen_normal_ones_leaves_the_panel_its_floor_at_any_latency() {
	const MINUTE_MS: u64 = 60_000;
	const PANEL_PERIOD_MS: u32 = 500;
	const GEARBOX: Unit = Unit {
		request: 0x7E1,
		response: 0x7E9,
	};
	let single = Budget {
		max_dids_per_request: 1,
		..Budget::board()
	};
	let runs = [Budget::board(), single]
		.into_iter()
		.flat_map(|budget| [5, 25, 45].map(|latency| (budget, latency)));
	for (budget, latency) in runs {
		for guard in [Guard::new(), Guard::cable()] {
			let mut board = Board::new(Bus::Answering { kmh: 0 });
			board.planner = Planner::new(budget);
			board.latency = latency;
			board.session = Session::with_guard(guard);
			// The panel: four channels at 2 Hz, on a unit of its own.
			let panel: Vec<u16> = (0..4u16).map(|n| 0x3000 + n * n).collect();
			for did in &panel {
				board.planner.subscribe(0, Class::Foreground, GATEWAY, *did, PANEL_PERIOD_MS, None);
			}
			board.hear(timing(1, ENGINE, 0xF40D, 20));
			// Fifteen normal channels at 50, 75 and 100 ms over two units. Squares: no eight
			// of them evenly spaced, so the walk rule has nothing to say.
			for n in 0..15u16 {
				let unit = if n % 2 == 0 { ENGINE } else { GEARBOX };
				board.hear(subscribe(10 + n, unit, 0x2000 + n * n, 50 + 25 * (n % 3)));
			}
			board.run_until(MINUTE_MS);
			let label = format!("{budget:?}, latency {latency} ms, {:?}", board.session.guard.profile());

			assert!(
				board.to_host.iter().all(|m| !matches!(
					m,
					Message::Reading(Reading {
						outcome: Outcome::Refused(_),
						..
					})
				)),
				"{label}: nothing refused"
			);
			let speed = board.readings(1).iter().filter(|(_, o)| matches!(o, Outcome::Pdu(_))).count();
			// A normal channel starved for `starve_after_ms` goes ahead of the timing one: one
			// send per channel per window at most, fewer where channels share a request.
			let windows = (MINUTE_MS / u64::from(budget.starve_after_ms) + 1) as usize;
			if latency < 20 {
				assert!(speed >= 45 * 60, "{label}: the timing channel got {speed} readings in a minute");
			} else {
				let rest = board.sent.iter().filter(|(_, o)| o.unit != GATEWAY).count();
				assert!(
					speed + 15 * windows >= rest,
					"{label}: the timing channel got {speed} of the {rest} sends the panel left"
				);
			}

			for did in &panel {
				let reads = board
					.sent
					.iter()
					.filter(|(_, o)| o.unit == GATEWAY && dids_of(&o.pdu).contains(did))
					.count();
				let asked = (MINUTE_MS / u64::from(PANEL_PERIOD_MS)) as usize;
				assert!(reads + 1 >= asked, "{label}: panel {did:04X} read {reads} times of {asked}");
			}

			// Slower than the period, the timing read is always due, and the normal channels
			// wait behind it until they starve: then one reading each per window.
			for n in 0..15u16 {
				let got = board.readings(10 + n).len();
				let least = if latency < 20 { 60 } else { windows - 2 };
				assert!(got >= least, "{label}: normal channel {n} got {got} readings in a minute");
			}

			let times: Vec<u64> = board.sent.iter().map(|(t, _)| *t).collect();
			let busiest = (0..times.len())
				.map(|i| times[i..].iter().take_while(|&&t| t < times[i] + 1000).count())
				.max()
				.unwrap_or(0);
			assert!(busiest <= usize::from(budget.ceiling_per_s), "{label}: {busiest} sends in one second");
		}
	}
}

/// One timing subscription per connection, on the radio and the cable alike; a normal one
/// still passes beside it, and unsubscribing or replacing the timing one frees its slot.
#[test]
fn a_second_timing_subscription_is_refused_and_an_unsubscribe_frees_the_slot() {
	for guard in [Guard::new(), Guard::cable()] {
		let mut board = Board::new(Bus::Answering { kmh: 0 });
		board.session = Session::with_guard(guard);
		board.hear(timing(1, ENGINE, 0xF40D, 20));
		board.hear(timing(2, GATEWAY, 0x1000, 100));
		let second = board.readings(2);
		assert_eq!(second.len(), 1, "{second:?}");
		assert!(refused(&second[0].1).contains("timing"), "{second:?}");

		board.hear(subscribe(3, GATEWAY, 0x1000, 100));
		board.hear(timing(1, ENGINE, 0xF40D, 40));
		board.run_until(500);
		assert!(!board.readings(3).is_empty(), "a normal subscription passes beside it");
		assert!(
			board.readings(1).iter().all(|(_, o)| matches!(o, Outcome::Pdu(_))),
			"given again under its own id, the timing subscription is replaced, not refused"
		);

		board.hear(Message::Unsubscribe { sub: 1 });
		board.to_host.clear();
		board.hear(timing(2, GATEWAY, 0x1001, 100));
		let now = board.now;
		board.run_until(now + 500);
		let freed = board.readings(2);
		assert!(!freed.is_empty(), "{freed:?}");
		assert!(freed.iter().all(|(_, o)| matches!(o, Outcome::Pdu(_))), "{freed:?}");
	}
}

/// What a session answered to one subscribe: `Some(reason)` for a refused reading of
/// `sub`, `None` when it was taken without a word.
fn refusal_of(out: &[Message], sub: u16) -> Option<String> {
	match out {
		[] => None,
		[
			Message::Reading(Reading {
				sub: of,
				outcome: Outcome::Refused(why),
				..
			}),
		] if *of == sub => Some(why.clone()),
		other => panic!("{other:?}"),
	}
}

/// One stopwatch at a time on the board, and the cable's owner comes first (S-F3): a timing
/// subscription held over the cable refuses one over the radio, and the radio gets it — and
/// is polled — once the cable lets go, by closing, by giving its id again as normal, or by
/// unsubscribing.
#[test]
fn the_boards_one_timing_channel_is_held_by_the_cable_until_it_lets_go() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	let mut usb = Session::with_guard(Guard::cable());

	assert!(
		usb.push(0, &mut board.planner, timing(7, GATEWAY, 0x1000, 20)).is_empty(),
		"the cable takes it"
	);
	board.hear(timing(1, ENGINE, 0xF40D, 20));
	let last = board.readings(1).pop().expect("the radio is refused while the cable holds it");
	assert_eq!(refused(&last.1), "another client holds the board's timing channel");
	assert_eq!(board.session.subscriptions().count(), 0);
	board.hear(subscribe(2, ENGINE, 0xF40C, 100));
	assert!(board.readings(2).is_empty(), "a normal subscription is not held to it");

	usb.close(board.now, &mut board.planner);
	board.to_host.clear();
	board.hear(timing(1, ENGINE, 0xF40D, 20));
	assert!(board.to_host.is_empty(), "the cable's session closed, so the channel is free");
	board.run_until(200);
	let polled = board
		.sent
		.iter()
		.filter(|(_, o)| o.unit == ENGINE && dids_of(&o.pdu).contains(&0xF40D))
		.count();
	assert!(polled >= 9, "the radio's timing channel is polled: {polled} in 200 ms");

	// The radio holds it now; a cable timing subscription preempts it (S-F3).
	let mut usb = Session::with_guard(Guard::cable());
	assert!(
		usb.push(board.now, &mut board.planner, timing(7, GATEWAY, 0x1000, 20)).is_empty(),
		"the cable takes it back"
	);
	board.run_until(board.now + 100);
	assert_eq!(
		refused(&board.readings(1).pop().unwrap().1),
		"the board's timing channel was taken by the cable host"
	);

	// Given again as normal, the cable lets go, and the radio takes it.
	let now = board.now;
	assert!(usb.push(now, &mut board.planner, subscribe(7, GATEWAY, 0x1000, 20)).is_empty());
	board.to_host.clear();
	board.hear(timing(1, ENGINE, 0xF40D, 20));
	assert!(board.to_host.is_empty(), "given again as normal, the cable let go: {:?}", board.to_host);

	// And an unsubscribe frees it the same way.
	assert!(usb.push(now, &mut board.planner, timing(7, GATEWAY, 0x1000, 20)).len() <= 1);
	board.hear(Message::Unsubscribe { sub: 7 } /* wrong session, no-op */);
	usb.push(now, &mut board.planner, Message::Unsubscribe { sub: 7 });
	board.to_host.clear();
	board.hear(timing(1, ENGINE, 0xF40D, 20));
	assert!(board.to_host.is_empty(), "unsubscribed, the cable let go");
	board.session.close(board.now, &mut board.planner);
	assert_eq!(board.planner.timing_subscriptions(), 0);
}

// --- a subscription reading is bounded (PR #2 review, S-F4) -------------------------------

/// A reading larger than `MAX_READING_BYTES` ends its subscription with a refusal, and a
/// small one streams on. Watch and measure channels are a few bytes, so only a record meant
/// for a one-shot trips it.
#[test]
fn a_reading_over_the_cap_ends_its_subscription_and_a_small_one_streams() {
	use crate::guard::Refusal;

	let big: Vec<u8> = (0..MAX_READING_BYTES as u16).map(|n| n as u8).collect();
	let mut planner = Planner::new(Budget::board());
	let mut session = Session::new();
	// Two subscriptions this session owns: one that will get a big record, one small.
	session.push(0, &mut planner, subscribe(1, ENGINE, 0x2000, 100));
	session.push(0, &mut planner, subscribe(2, ENGINE, 0x2001, 100));
	let big_id = session.subs[&1].id;
	let small_id = session.subs[&2].id;

	let oversized = Delivery::Reading {
		sub: big_id,
		unit: ENGINE,
		did: 0x2000,
		data: big,
		at_ms: 50,
	};
	let msg = session.deliver(60, &mut planner, &oversized).expect("a reading for the host");
	match msg {
		Message::Reading(Reading {
			sub: 1,
			outcome: Outcome::Refused(why),
			..
		}) => {
			assert_eq!(why, Refusal::ReadingTooLarge.to_string())
		}
		other => panic!("{other:?}"),
	}
	assert_eq!(session.subscriptions().count(), 1, "the big subscription ended");
	assert!(!planner.holds(big_id), "and left the planner");

	let small = Delivery::Reading {
		sub: small_id,
		unit: ENGINE,
		did: 0x2001,
		data: vec![0x11, 0x22],
		at_ms: 70,
	};
	let msg = session.deliver(80, &mut planner, &small).expect("a reading");
	assert!(
		matches!(
			msg,
			Message::Reading(Reading {
				sub: 2,
				outcome: Outcome::Pdu(_),
				..
			})
		),
		"{msg:?}"
	);
	let _ = Miss::NoAnswer;
}

// --- the radio guard is the board's, not the connection's (PR #2 review, S-F1) -----------

/// A `22` request for `dids`.
fn rdbi(dids: &[u16]) -> Vec<u8> {
	core::iter::once(0x22).chain(dids.iter().flat_map(|did| did.to_be_bytes())).collect()
}

/// Identifiers of the `22` requests that reached the bus.
fn identifiers_sent(board: &Board) -> usize {
	board
		.pdus_sent()
		.iter()
		.filter(|p| p[0] == 0x22 && p[1..] != [0xF4, 0x0D])
		.map(|p| (p.len() - 1) / 2)
		.sum()
}

/// The reviewer's proof reversed. A Hello closes the session, and the close used to renew
/// the radio guard: all of `F100–F1FF`, four a request with a Hello after each, reached the
/// unit in 1.3 s. The memory now outlives it, and the sweep is refused by its eighth identifier.
#[test]
fn a_hello_between_requests_does_not_reset_the_radio_guard() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	let all: Vec<u16> = (0xF100u16..=0xF1FF).collect();
	for (seq, block) in all.chunks(4).enumerate() {
		board.hear(request(seq as u8, GATEWAY, &rdbi(block)));
		board.run_until(board.now + 20);
		// What the firmware does with a Hello, on either carrier.
		board.session.close(board.now, &mut board.planner);
	}
	let reads = identifiers_sent(&board);
	assert!(reads < crate::guard::WALK_RUN, "{reads} identifiers reached the unit");
	assert!(
		board
			.answers()
			.iter()
			.any(|(_, o)| matches!(o, Outcome::Refused(why) if why.contains("evenly spaced"))),
		"{:?}",
		board.answers()
	);
}

/// A reconnect is a close of the same board's radio session: a unit a walk locked stays
/// locked, and the rate window stays as full as it was.
#[test]
fn a_reconnect_keeps_the_lock_and_the_rate_window() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(request(1, ENGINE, &rdbi(&[0x2000, 0x2001, 0x2002, 0x2003])));
	board.run_until(100);
	board.hear(request(2, ENGINE, &rdbi(&[0x2004, 0x2005, 0x2006, 0x2007])));
	board.run_until(200);
	assert!(refused(&board.answers()[1].1).contains("evenly spaced"), "{:?}", board.answers());

	board.session.close(board.now, &mut board.planner);
	board.to_host.clear();
	board.hear(request(3, ENGINE, &rdbi(&[0xF190])));
	board.run_until(300);
	assert!(refused(&board.answers()[0].1).contains("locked"), "{:?}", board.answers());

	// Four identifiers are in the window; sixteen more fill it.
	for seq in 0..16 {
		board.hear(request(10 + seq, GATEWAY, &[0x3E, 0x00]));
	}
	board.run_until(1_000);
	assert_eq!(board.answers().len(), 17, "{:?}", board.answers());
	board.session.close(board.now, &mut board.planner);
	board.hear(request(30, GATEWAY, &[0x3E, 0x00]));
	board.run_until(2_000);
	assert_eq!(board.answers().len(), 17, "the full window outlived the reconnect");
	board.run_until(RATE_WINDOW_MS + 1_000);
	assert_eq!(board.answers().len(), 18, "and the request went out once it had room");
}

/// The units a radio host addressed count across reconnects, so the board's memory stays
/// bounded by [`MAX_UNITS`](crate::guard::MAX_UNITS) whatever the host does with the link —
/// and a unit ten minutes without a request frees its slot.
#[test]
fn the_units_cap_holds_across_reconnects_and_frees_with_time() {
	use crate::guard::MAX_UNITS;
	let unit = |n: u16| Unit {
		request: 0x600 + n,
		response: 0x700 + n,
	};
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	for n in 0..MAX_UNITS as u16 {
		board.hear(request(n as u8, unit(n), &[0x3E, 0x00]));
		if n % 8 == 7 {
			board.run_until(board.now + RATE_WINDOW_MS);
			board.session.close(board.now, &mut board.planner);
		}
	}
	board.run_until(board.now + RATE_WINDOW_MS);
	assert!(board.answers().iter().all(|(_, o)| matches!(o, Outcome::Pdu(_))), "{:?}", board.answers());
	board.to_host.clear();

	let next = unit(MAX_UNITS as u16);
	board.hear(request(99, next, &[0x3E, 0x00]));
	board.run_until(board.now + 100);
	assert!(refused(&board.answers()[0].1).contains("units"), "{:?}", board.answers());

	board.to_host.clear();
	board.run_until(board.now + 10 * 60_000);
	board.hear(request(100, next, &[0x3E, 0x00]));
	board.run_until(board.now + 100);
	assert!(matches!(board.answers()[..], [(100, Outcome::Pdu(_))]), "{:?}", board.answers());
}

// --- bus time, and the timing channel between carriers (PR #2 review, S-F3) --------------

/// How much of `[from, to)` the exchanges sent at `sent` held, each for `held_ms`.
fn bus_time(sent: &[u64], held_ms: u64, from: u64, to: u64) -> u64 {
	sent.iter().map(|t| (t + held_ms).min(to).saturating_sub((*t).max(from))).sum()
}

/// The reviewer's starvation by bus time: a request to an id nobody answers holds the bus
/// for the whole answer timeout, and a different id each time dodges the planner's per-unit
/// backoff. A radio host is charged the time its exchanges hold the bus: past a quarter of
/// any ten seconds its next request waits. Slowed, never dropped. The cable is not charged.
#[test]
fn a_radio_host_holds_at_most_a_quarter_of_the_bus_by_time_and_the_cable_is_not_held_to_it() {
	const TIMEOUT_MS: u64 = 500;
	const WINDOW_MS: u64 = 10_000;
	let unit = |n: u16| Unit {
		request: 0x600 + n,
		response: 0x680 + n,
	};
	for cable in [false, true] {
		let mut board = Board::new(Bus::Silent);
		board.latency = TIMEOUT_MS;
		if cable {
			board.session = Session::with_guard(Guard::cable());
		}
		for n in 0..60u16 {
			board.hear(request(n as u8, unit(n), &[0x3E, 0x00]));
		}
		board.run_until(200_000);
		assert_eq!(board.answers().len(), 60, "cable {cable}: every request answered");
		assert!(board.answers().iter().all(|(_, o)| *o == Outcome::NoAnswer), "cable {cable}");

		let sent: Vec<u64> = board.sent.iter().map(|(t, _)| *t).collect();
		let busiest = (0..200_000 - WINDOW_MS)
			.step_by(50)
			.map(|from| bus_time(&sent, TIMEOUT_MS, from, from + WINDOW_MS))
			.max()
			.unwrap();
		if cable {
			assert!(busiest > WINDOW_MS / 2, "the cable is not held to the radio's share: {busiest} ms");
		} else {
			assert!(
				busiest <= WINDOW_MS / 4 + TIMEOUT_MS,
				"a quarter of ten seconds, and the exchange that crossed it: {busiest} ms"
			);
		}
	}
}

/// One stopwatch at a time, and the owner's comes first: a timing subscription from the
/// cable takes the board's timing channel from a radio host, whose subscription ends with a
/// reading that says so. A radio host never takes it from the cable.
#[test]
fn a_cable_timing_subscription_takes_the_channel_from_the_radio_and_never_the_other_way() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	let mut usb = Session::with_guard(Guard::cable());
	board.hear(timing(1, ENGINE, 0xF40D, 20));
	board.run_until(100);

	let out = usb.push(board.now, &mut board.planner, timing(7, GATEWAY, 0x1000, 20));
	assert_eq!(refusal_of(&out, 7), None, "the cable takes the channel");
	board.run_until(300);
	let last = board.readings(1).pop().expect("the radio is told");
	assert_eq!(refused(&last.1), "the board's timing channel was taken by the cable host");
	assert_eq!(board.session.subscriptions().count(), 0);
	let before = board.sent.len();
	board.run_until(600);
	assert!(
		board.sent[before..].iter().all(|(_, o)| o.unit == GATEWAY),
		"the radio's channel is no longer polled"
	);
	assert_eq!(board.planner.timing_subscriptions(), 1);

	board.to_host.clear();
	board.hear(timing(1, ENGINE, 0xF40D, 20));
	let readings = board.readings(1);
	assert_eq!(readings.len(), 1, "{readings:?}");
	assert_eq!(refused(&readings[0].1), "another client holds the board's timing channel");
	usb.close(board.now, &mut board.planner);
}

/// The cable's session holds the cable's guard, and keeps it across a close.
#[test]
fn a_cable_session_walks_a_range_the_radio_refuses_and_stays_a_cable_session_after_close() {
	let walk = |board: &mut Board| {
		for (seq, did) in (0xF100u16..0xF110).enumerate() {
			board.hear(request(seq as u8, ENGINE, &[0x22, (did >> 8) as u8, did as u8]));
			let until = board.now + 20;
			board.run_until(until);
		}
		board.answers().iter().filter(|(_, o)| matches!(o, Outcome::Refused(_))).count()
	};
	let mut radio = Board::new(Bus::Answering { kmh: 0 });
	assert!(walk(&mut radio) > 0, "the radio refuses the walk");

	let mut cable = Board::new(Bus::Answering { kmh: 0 });
	cable.session = Session::with_guard(crate::guard::Guard::cable());
	assert_eq!(walk(&mut cable), 0, "{:?}", cable.answers());
	cable.session.close(cable.now, &mut cable.planner);
	cable.to_host.clear();
	assert_eq!(walk(&mut cable), 0, "still the cable's guard after a close");
}
