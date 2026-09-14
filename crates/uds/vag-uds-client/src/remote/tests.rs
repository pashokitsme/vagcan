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
	board.session.close(&mut board.planner);
	assert_eq!(board.planner.units_held(), 0);
}

/// A host that leaves takes its queued request with it: nothing it asked reaches the car
/// after the connection is gone.
#[test]
fn closing_cancels_the_request_the_planner_has_not_sent() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	board.hear(request(1, GATEWAY, &[0x22, 0xF1, 0x87]));
	assert!(board.session.awaiting().is_some(), "handed to the planner");
	board.session.close(&mut board.planner);
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
/// 10 Hz. Marked timing, it keeps 50 Hz beside fifteen normal channels and the panel;
/// the panel keeps everything it asks (under its floor), the normal channels are slowed
/// and not dropped, and no second holds more than the ceiling. Under the default budget,
/// and under one identifier per request, where every read is an exchange of its own; on
/// either carrier's guard.
#[test]
fn a_timing_subscription_keeps_fifty_hertz_beside_fifteen_normal_ones_and_a_panel() {
	const MINUTE_MS: u64 = 60_000;
	const PANEL_PERIOD_MS: u32 = 500;
	const GEARBOX: Unit = Unit {
		request: 0x7E1,
		response: 0x7E9,
	};
	let single = Budget {
		max_dids_per_request: 1,
		..Budget::default()
	};
	for budget in [Budget::default(), single] {
		for guard in [Guard::new(), Guard::cable()] {
			let mut board = Board::new(Bus::Answering { kmh: 0 });
			board.planner = Planner::new(budget);
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
			let label = format!("{budget:?}, {:?}", board.session.guard.profile());

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
			assert!(speed >= 45 * 60, "{label}: the timing channel got {speed} readings in a minute");

			for did in &panel {
				let reads = board
					.sent
					.iter()
					.filter(|(_, o)| o.unit == GATEWAY && dids_of(&o.pdu).contains(did))
					.count();
				let asked = (MINUTE_MS / u64::from(PANEL_PERIOD_MS)) as usize;
				assert!(reads + 1 >= asked, "{label}: panel {did:04X} read {reads} times of {asked}");
			}

			for n in 0..15u16 {
				let got = board.readings(10 + n).len();
				assert!(got >= 60, "{label}: normal channel {n} got {got} readings in a minute");
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

/// One stopwatch at a time on the board: a timing subscription held over the radio refuses
/// one over the cable, and the cable gets it — and is polled — once the radio's session
/// closes. It frees the same way when its holder gives its id again as normal, or
/// unsubscribes.
#[test]
fn the_boards_one_timing_channel_is_held_across_connections_until_its_holder_lets_go() {
	let mut board = Board::new(Bus::Answering { kmh: 0 });
	let mut usb = Session::with_guard(Guard::cable());

	board.hear(timing(1, ENGINE, 0xF40D, 20));
	assert!(board.to_host.is_empty(), "the radio takes the channel: {:?}", board.to_host);
	let out = usb.push(0, &mut board.planner, timing(7, GATEWAY, 0x1000, 20));
	let why = refusal_of(&out, 7).expect("the cable is refused while the radio holds it");
	assert_eq!(why, "another client holds the board's timing channel");
	assert_eq!(usb.subscriptions().count(), 0);
	let out = usb.push(0, &mut board.planner, subscribe(8, GATEWAY, 0x1001, 100));
	assert_eq!(refusal_of(&out, 8), None, "a normal subscription is not held to it");

	board.session.close(&mut board.planner);
	let out = usb.push(0, &mut board.planner, timing(7, GATEWAY, 0x1000, 20));
	assert_eq!(refusal_of(&out, 7), None, "the radio's session closed, so the channel is free");
	board.run_until(200);
	let polled = board
		.sent
		.iter()
		.filter(|(_, o)| o.unit == GATEWAY && dids_of(&o.pdu).contains(&0x1000))
		.count();
	assert!(polled >= 9, "the cable's timing channel is polled: {polled} in 200 ms");

	board.hear(timing(1, ENGINE, 0xF40D, 20));
	let last = board.readings(1).pop().expect("a reading for the radio");
	assert_eq!(refused(&last.1), "another client holds the board's timing channel");

	let now = board.now;
	let out = usb.push(now, &mut board.planner, subscribe(7, GATEWAY, 0x1000, 20));
	assert_eq!(refusal_of(&out, 7), None);
	board.to_host.clear();
	board.hear(timing(1, ENGINE, 0xF40D, 20));
	assert!(board.to_host.is_empty(), "given again as normal, the cable let go: {:?}", board.to_host);

	board.hear(Message::Unsubscribe { sub: 1 });
	let out = usb.push(now, &mut board.planner, timing(7, GATEWAY, 0x1000, 20));
	assert_eq!(refusal_of(&out, 7), None, "unsubscribed, the radio let go");
	usb.close(&mut board.planner);
	assert_eq!(board.planner.timing_subscriptions(), 0);
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
	cable.session.close(&mut cable.planner);
	cable.to_host.clear();
	assert_eq!(walk(&mut cable), 0, "still the cable's guard after a close");
}
