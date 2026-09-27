//! The board's fault count on the host (`todo/dash/20`, phase 2): when it starts, when it waits
//! for the stopwatch, what it asks the planner, what it says on USB and what the panel shows.
//!
//! `crates/dash/vag-dash-fw/src/faults.rs` compiled as it is — the firmware cannot be built for
//! the host — and driven through the planner the board runs, `Budget::board()`, against a
//! fake car.

#[path = "../../../../crates/dash/vag-dash-fw/src/faults.rs"]
mod faults;

use std::collections::BTreeMap;

use faults::{Count, DEADLINE_MS, Found, Hold, Line, Now, START_MS, bus_on};
use vag_dash_render::Faults;
use vag_uds_client::faultcount::{Failed, MAX_UNITS, UnitTally, Why};
use vag_uds_client::schedule::{Answer, Budget, Class, Delivery, Next, Outgoing, Planner, Unit};

/// A car behind the planner: the gateway's answer to its list, and each unit's to `19 02 08`.
/// A unit not listed here does not answer.
struct Car {
	gateway: Answer,
	units: BTreeMap<u16, Answer>,
}

impl Car {
	fn listing(listed: &[u16]) -> Self {
		Car {
			gateway: Answer::Pdu(list_answer(listed)),
			units: BTreeMap::new(),
		}
	}

	/// `request` answers with these `(code, status)` records.
	fn codes(mut self, request: u16, codes: &[([u8; 3], u8)]) -> Self {
		let mut pdu = vec![0x59, 0x02, 0xFF];
		for (code, status) in codes {
			pdu.extend_from_slice(code);
			pdu.push(*status);
		}
		self.units.insert(request, Answer::Pdu(pdu));
		self
	}

	fn answer(&self, unit: Unit, pdu: &[u8]) -> Answer {
		if pdu.first() == Some(&0x22) {
			assert_eq!(unit.request, 0x710, "only the gateway is asked for its list");
			return self.gateway.clone();
		}
		self.units.get(&unit.request).cloned().unwrap_or(Answer::NoAnswer)
	}
}

/// `62 2A 26` and a 32-byte bitmap with a bit for each id.
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

/// One exchange as the bus task ran it.
#[derive(Debug, Clone)]
struct Sent {
	unit: Unit,
	pdu: Vec<u8>,
	/// What the count gave the exchange as its own deadline.
	deadline_ms: Option<u64>,
}

/// The bus task's loop in miniature: the count's turn before each `due`, one exchange at a
/// time, every delivery offered to the count.
struct Bench {
	planner: Planner,
	count: Count,
	clock: u64,
	said: Vec<String>,
	sent: Vec<Sent>,
}

impl Bench {
	fn new() -> Self {
		Bench {
			planner: Planner::new(Budget::board()),
			count: Count::new(),
			clock: 0,
			said: Vec::new(),
			sent: Vec::new(),
		}
	}

	/// The count's turn with the board's stopwatch up or not.
	fn step(&mut self, bus_on: bool, stopwatch: bool) {
		self.step_held(bus_on, stopwatch.then_some(Hold::Stopwatch));
	}

	/// The count's turn, with whatever holds it up.
	fn step_held(&mut self, bus_on: bool, hold: Option<Hold>) {
		let Bench {
			planner, count, said, clock, ..
		} = self;
		let now = Now { ms: *clock, bus_on, hold };
		count.step(now, planner, &mut |line: &Line<'_>| said.push(line.to_string()));
	}

	/// `panel_bus` giving up the exchange on the bus for adapter mode: the count hears of it
	/// first, then the planner is answered `BusError` and its deliveries go round.
	fn give_up(&mut self, out: &Outgoing) {
		let flying = self.planner.flying_raw();
		let Bench {
			planner, count, said, clock, ..
		} = self;
		count.given_up(flying, &mut |line: &Line<'_>| said.push(line.to_string()));
		for delivery in planner.answered(*clock, out.token, Answer::BusError) {
			// Nobody's now: `remote` drops it.
			assert!(count.take(delivery, &mut |line: &Line<'_>| said.push(line.to_string())).is_some());
		}
	}

	/// The planner's next exchange, if one is due by now or by its next slot, answered by `car`
	/// after `held_ms` and its deliveries given to the count, which must take them all.
	fn exchange(&mut self, car: &Car, held_ms: u64) -> Option<Sent> {
		self.exchange_timed(car, &|_| held_ms)
	}

	/// [`Bench::exchange`], each unit answering after the time `held` gives for its request id.
	fn exchange_timed(&mut self, car: &Car, held: &dyn Fn(u16) -> u64) -> Option<Sent> {
		let out = self.due()?;
		let sent = Sent {
			unit: out.unit,
			pdu: out.pdu.clone(),
			deadline_ms: self.count.deadline_ms(self.planner.flying_raw()),
		};
		self.sent.push(sent.clone());
		self.clock += held(out.unit.request);
		let answer = car.answer(out.unit, &out.pdu);
		let Bench {
			planner, count, said, clock, ..
		} = self;
		for delivery in planner.answered(*clock, out.token, answer) {
			if let Some(other) = count.take(delivery, &mut |line: &Line<'_>| said.push(line.to_string())) {
				panic!("not the count's: {other:?}");
			}
		}
		Some(sent)
	}

	/// What the planner sends next, the clock moved to its slot; `None` when nothing will come
	/// due by time alone.
	fn due(&mut self) -> Option<Outgoing> {
		loop {
			match self.planner.due(self.clock) {
				Next::Send(out) => return Some(out),
				Next::Idle { until_ms: Some(until) } if until > self.clock => self.clock = until,
				Next::Idle { .. } => return None,
			}
		}
	}

	/// Steps and exchanges, every unit answering in `held_ms`, until the count has found
	/// something; the number of exchanges it took.
	fn run(&mut self, car: &Car, held_ms: u64) -> usize {
		self.run_timed(car, &|_| held_ms)
	}

	/// [`Bench::run`], each unit answering after the time `held` gives for its request id.
	fn run_timed(&mut self, car: &Car, held: &dyn Fn(u16) -> u64) -> usize {
		let mut n = 0;
		while self.count.found().is_none() {
			self.step(true, false);
			assert!(self.exchange_timed(car, held).is_some(), "the count stalled after {n} exchanges");
			n += 1;
			assert!(n < 300, "the count never ends");
		}
		n
	}
}

/// The reference car's shape: a list with the two ids that share one, a bit past VW's block,
/// codes on two units, one silent unit, and one — at `70C` — that asks for time (`78`) and is
/// still searching when the count's deadline cuts its exchange: the shell says `StillPending`.
fn reference_like() -> Car {
	let mut car = Car::listing(&[0x70C, 0x714, 0x776, 0x777])
		.codes(0x7E0, &[([0x01, 0x02, 0x03], 0x09), ([0x04, 0x05, 0x06], 0x08)])
		.codes(0x710, &[])
		.codes(0x714, &[([0x07, 0x08, 0x09], 0x2C)]);
	car.units.insert(0x70C, Answer::StillPending);
	if let Answer::Pdu(list) = &mut car.gateway {
		list[3 + (0x7C0 - 0x700) / 8] |= 1;
	}
	car
}

#[test]
fn nothing_is_asked_before_ten_seconds_after_boot_or_before_a_plan_unit_answers() {
	let car = reference_like();
	let mut bench = Bench::new();
	bench.clock = START_MS - 1;
	bench.step(true, false);
	assert!(bench.exchange(&car, 5).is_none(), "not before ten seconds");
	bench.clock = START_MS;
	bench.step(false, false);
	assert!(bench.exchange(&car, 5).is_none(), "not while no plan unit has answered");
	assert!(bench.said.is_empty(), "{:?}", bench.said);

	// A board on permanent +12 V: the ignition comes on a minute later.
	bench.clock = 70_000;
	bench.step(true, false);
	assert_eq!(bench.said, ["faults: counting the car's stored codes — the gateway's list first"]);
	let first = bench.exchange(&car, 5).expect("the gateway asked");
	assert_eq!(
		first.unit,
		Unit {
			request: 0x710,
			response: 0x77A
		}
	);
	assert_eq!(first.pdu, [0x22, 0x2A, 0x26]);
}

#[test]
fn a_plan_with_no_units_starts_at_ten_seconds_and_one_with_units_once_one_answers() {
	assert!(bus_on(0, 0), "a plan with no units has none to wait for");
	assert!(!bus_on(3, 0));
	assert!(bus_on(3, 1));
	assert_eq!(START_MS, 10_000);
}

#[test]
fn the_start_waits_for_the_stopwatch_to_close() {
	let car = reference_like();
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.step(true, true);
	assert!(bench.exchange(&car, 5).is_none());
	assert!(bench.said.is_empty(), "nothing starts under the stopwatch: {:?}", bench.said);
	bench.clock += 30_000;
	bench.step(true, false);
	assert!(bench.exchange(&car, 5).is_some());
}

#[test]
fn a_whole_count_through_the_planner_reads_what_vagcan_faults_counts_and_says_each_unit() {
	let car = reference_like();
	let mut bench = Bench::new();
	bench.clock = START_MS;
	// The gearbox is silent for the board's answer timeout; the unit at 70C holds the bus until
	// the count's deadline; the rest answer in 5 ms.
	bench.run_timed(&car, &|request| match request {
		0x7E1 => 500,
		0x70C => DEADLINE_MS,
		_ => 5,
	});

	// The gateway once, then the walk: the three the list cannot hold, then the list, less the
	// two that share an id — each asked `19 02 08`, and nothing else ever.
	let asked: Vec<(u16, Vec<u8>)> = bench.sent.iter().map(|s| (s.unit.request, s.pdu.clone())).collect();
	assert_eq!(
		asked,
		[
			(0x710, vec![0x22, 0x2A, 0x26]),
			(0x7E0, vec![0x19, 0x02, 0x08]),
			(0x7E1, vec![0x19, 0x02, 0x08]),
			(0x710, vec![0x19, 0x02, 0x08]),
			(0x70C, vec![0x19, 0x02, 0x08]),
			(0x714, vec![0x19, 0x02, 0x08]),
		]
	);
	for sent in &bench.sent {
		assert_eq!(sent.deadline_ms, Some(DEADLINE_MS), "{:03X}: the count's own deadline", sent.unit.request);
	}

	// Stored = confirmed (bit 3), failing now = confirmed and failing (bit 0): 7E0 two stored, one
	// of them failing now; 714 one stored (0x2C: bits 2, 3, 5).
	let found = bench.count.found().expect("counted");
	assert_eq!(found, Found::Counted { stored: 3, failing_now: 1 });
	assert_eq!(
		found.badge(),
		Faults::Counted {
			stored: 3,
			failing_now: true
		}
	);
	let took = bench.clock - START_MS;
	assert_eq!(
		bench.said,
		[
			"faults: counting the car's stored codes — the gateway's list first".to_string(),
			"faults: the list set 1 bit past 7BF — not decoded, not asked".to_string(),
			"faults: 776, 777 skipped — each shares an id with a unit walked".to_string(),
			"faults: 7E0 2 stored, 1 failing now".to_string(),
			"faults: 7E1 not counted — no answer".to_string(),
			"faults: 70C not counted — asked for time (78), no answer in 2 s".to_string(),
			"faults: 714 1 stored, 0 failing now".to_string(),
			format!(
				"faults: 3 stored, 1 failing now; 3 of 5 units answered in {}.{} s",
				took / 1000,
				took % 1000 / 100
			),
		]
	);

	// Once per boot: nothing is asked again, whatever comes.
	bench.clock += 60_000;
	bench.step(true, false);
	assert!(bench.exchange(&car, 5).is_none());
}

#[test]
fn only_one_request_of_the_count_is_ever_with_the_planner() {
	let car = Car::listing(&[0x70C, 0x714]);
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.step(true, false);
	// Asked again and again before the answer: the one request, not a second.
	for _ in 0..5 {
		bench.step(true, false);
	}
	assert!(bench.exchange(&car, 5).is_some());
	assert!(bench.exchange(&car, 5).is_none(), "one request was queued, not six");
}

#[test]
fn the_count_waits_while_the_stopwatch_is_up_and_goes_on_where_it_stopped() {
	let car = Car::listing(&[0x70C]).codes(0x7E0, &[([0, 1, 2], 0x08)]);
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.step(true, false);
	bench.exchange(&car, 5).expect("the gateway");
	bench.step(true, false);
	// The engine's request waits in the planner; the stopwatch opens before it goes out.
	bench.step(true, true);
	assert!(bench.exchange(&car, 5).is_none(), "taken back: nothing of the count's is due");
	for _ in 0..3 {
		bench.clock += 1_000;
		bench.step(true, true);
		assert!(bench.exchange(&car, 5).is_none(), "nothing starts while the stopwatch is up");
	}
	bench.clock += 1_000;
	bench.step(true, false);
	let next = bench.exchange(&car, 5).expect("going on");
	assert_eq!(next.unit.request, 0x7E0, "where it stopped: the engine is still the next");
	assert_eq!(
		bench.said[1..],
		[
			"faults: waiting while the stopwatch is up".to_string(),
			"faults: counting on where it stopped".to_string(),
			"faults: 7E0 1 stored, 0 failing now".to_string(),
		],
		"said once each way"
	);
	bench.run(&car, 5);
	assert_eq!(bench.count.found(), Some(Found::Counted { stored: 1, failing_now: 0 }));
	// The count's time leaves the 4 s it waited out: it is what car check 2 reads.
	let last = bench.said.last().unwrap();
	assert!(last.ends_with(", not counting 4.0 s paused"), "{last}");
}

#[test]
fn an_exchange_already_out_when_the_stopwatch_opens_runs_to_its_end_and_nothing_follows_it() {
	let car = Car::listing(&[0x70C]).codes(0x7E0, &[([0, 1, 2], 0x08)]);
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.step(true, false);
	bench.exchange(&car, 5).expect("the gateway");
	bench.step(true, false);
	// On the bus when the stopwatch opens: what is on the bus cannot be recalled.
	let out = bench.due().expect("the engine's request is due");
	bench.step(true, true);
	assert_eq!(
		bench.count.deadline_ms(bench.planner.flying_raw()),
		Some(DEADLINE_MS),
		"still the count's, and still bounded"
	);
	bench.clock += 40;
	let Bench {
		planner, count, said, clock, ..
	} = &mut bench;
	for delivery in planner.answered(*clock, out.token, car.answer(out.unit, &out.pdu)) {
		assert!(count.take(delivery, &mut |line: &Line<'_>| said.push(line.to_string())).is_none());
	}
	assert!(bench.said.contains(&"faults: 7E0 1 stored, 0 failing now".to_string()));
	bench.step(true, true);
	assert!(bench.exchange(&car, 5).is_none(), "and the next waits for the stopwatch");
}

#[test]
fn a_count_exchange_has_its_own_deadline_and_no_other_exchange_does() {
	let mut bench = Bench::new();
	assert_eq!(bench.count.deadline_ms(None), None);
	let host = bench
		.planner
		.exchange(
			0,
			Class::Remote,
			Unit {
				request: 0x7E0,
				response: 0x7E8,
			},
			vec![0x19, 0x02, 0xFF],
		)
		.expect("allowed");
	assert_eq!(
		bench.count.deadline_ms(Some(host)),
		None,
		"a host's fault read keeps the board's deadlines"
	);
	bench.clock = START_MS;
	bench.step(true, false);
	assert_eq!(bench.count.deadline_ms(Some(host)), None);
	assert_eq!(DEADLINE_MS, 2_000);
}

#[test]
fn a_gateway_with_no_list_is_a_question_mark_said_once_and_never_asked_again() {
	for (answer, held, text) in [
		(Answer::NoAnswer, 500, "no answer"),
		(Answer::StillPending, DEADLINE_MS, "asked for time (78), no answer in 2 s"),
		(Answer::BusError, 0, "bus error"),
		(Answer::Pdu(vec![0x7F, 0x22, 0x31]), 5, "refused, NRC 31"),
		(Answer::Pdu(vec![0x62, 0x04, 0xA3, 0x01]), 5, "answer did not parse"),
	] {
		let car = Car {
			gateway: answer.clone(),
			units: BTreeMap::new(),
		};
		let mut bench = Bench::new();
		bench.clock = START_MS;
		assert_eq!(bench.run(&car, held), 1, "{answer:?}: the gateway alone");
		assert_eq!(bench.count.found(), Some(Found::Failed));
		assert_eq!(Found::Failed.badge(), Faults::Failed);
		assert_eq!(
			bench.said.last().map(String::as_str),
			Some(format!("faults: the gateway gave no list ({text}) — badge ?").as_str())
		);
		bench.clock += 60_000;
		bench.step(true, false);
		assert!(bench.exchange(&car, 5).is_none(), "{answer:?}: no retry");
	}
}

#[test]
fn ids_past_vws_block_and_ids_of_it_no_unit_can_answer_on_are_said_apart() {
	let mut car = Car::listing(&[0x7A0, 0x7B3]);
	if let Answer::Pdu(list) = &mut car.gateway {
		list[3 + (0x7E0 - 0x700) / 8] |= 1;
	}
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.run(&car, 5);
	assert_eq!(
		bench.said[1..3],
		[
			"faults: the list set 1 bit past 7BF — not decoded, not asked".to_string(),
			"faults: the list names 2 ids past 795 — no answer id fits 11 bits, not asked".to_string(),
		]
	);
	assert!(bench.sent.iter().all(|s| ![0x7A0, 0x7B3].contains(&s.unit.request)), "neither asked");
}

#[test]
fn a_list_longer_than_the_board_may_walk_is_a_question_mark_and_nothing_is_asked() {
	// Sixty-two plain ids and the three: sixty-five, one past `MAX_UNITS`.
	let listed: Vec<u16> = (0x700u16..0x76A).filter(|id| *id != 0x710).take(MAX_UNITS - 2).collect();
	let car = Car::listing(&listed);
	let mut bench = Bench::new();
	bench.clock = START_MS;
	assert_eq!(bench.run(&car, 5), 1, "the gateway alone");
	assert_eq!(bench.count.found(), Some(Found::Failed));
	assert_eq!(
		bench.said.last().map(String::as_str),
		Some("faults: the walk would ask 65 units, more than 64 — not a car's list, badge ?")
	);
}

#[test]
fn an_exchange_given_up_for_adapter_mode_is_asked_again_when_the_panel_is_back() {
	// Review, 2026-09-27: the give-up was fed to the count as the car's answer — for the
	// gateway a `?` for the whole boot, for a unit its codes left out. It is the board's doing,
	// not the car's: the same request goes out again once the panel is back.
	let car = Car::listing(&[]).codes(0x7E0, &[([0, 1, 2], 0x08)]).codes(0x7E1, &[]).codes(0x710, &[]);
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.step(true, false);
	let gateway = bench.due().expect("the gateway's list");
	bench.give_up(&gateway);
	assert_eq!(bench.count.found(), None, "no `?` for the board's own doing");
	bench.clock += 30_000;
	bench.step(true, false);
	let again = bench.exchange(&car, 5).expect("asked again");
	assert_eq!((again.unit.request, again.pdu.clone()), (0x710, vec![0x22, 0x2A, 0x26]));

	bench.step(true, false);
	let engine = bench.due().expect("the engine's request");
	assert_eq!(engine.unit.request, 0x7E0);
	bench.give_up(&engine);
	bench.step(true, false);
	let again = bench.exchange(&car, 5).expect("asked again");
	assert_eq!(again.unit.request, 0x7E0, "the same unit, not the next");
	bench.run(&car, 5);
	assert_eq!(bench.count.found(), Some(Found::Counted { stored: 1, failing_now: 0 }));
	assert_eq!(
		bench
			.said
			.iter()
			.filter(|l| *l == "faults: the board turned adapter — the same request again when the panel is back")
			.count(),
		2
	);
	assert!(!bench.said.iter().any(|l| l.contains("bus error")), "{:?}", bench.said);
}

#[test]
fn nobody_answering_is_a_question_mark_not_no_faults() {
	// Review, 2026-09-27: a list from the gateway and not one unit of the walk answering was
	// counted as 0 stored — no badge, the picture of a car with no faults.
	let car = Car::listing(&[0x70C]);
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.run(&car, 5);
	assert_eq!(bench.count.found(), Some(Found::Failed));
	assert_eq!(Found::Failed.badge(), Faults::Failed);
	assert_eq!(bench.said.last().map(String::as_str), Some("faults: none of 4 units answered — badge ?"));
}

#[test]
fn the_count_waits_while_a_host_holds_the_boards_timing_channel() {
	// A laptop's `vagcan measure` through the board is a stopwatch too: a 2 s exchange of the
	// count's would be a gap in its run (review, 2026-09-27; flagged to the owner).
	let car = Car::listing(&[0x70C]).codes(0x7E0, &[([0, 1, 2], 0x08)]);
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.step_held(true, Some(Hold::HostTiming));
	assert!(bench.exchange(&car, 5).is_none(), "no start while a host times a run");
	assert!(bench.said.is_empty(), "{:?}", bench.said);
	bench.step_held(true, None);
	bench.exchange(&car, 5).expect("the gateway");
	bench.step_held(true, None);
	bench.step_held(true, Some(Hold::HostTiming));
	assert!(bench.exchange(&car, 5).is_none(), "taken back");
	// The host's run ends and the board's stopwatch opens: still waiting, and said why.
	bench.step_held(true, Some(Hold::Stopwatch));
	assert!(bench.exchange(&car, 5).is_none());
	bench.step_held(true, None);
	let next = bench.exchange(&car, 5).expect("going on");
	assert_eq!(next.unit.request, 0x7E0);
	assert_eq!(
		bench.said[1..4],
		[
			"faults: waiting while a host times a run on the board's timing channel".to_string(),
			"faults: waiting while the stopwatch is up".to_string(),
			"faults: counting on where it stopped".to_string(),
		]
	);
}

#[test]
fn a_hosts_answer_during_the_count_goes_to_the_host() {
	// Mutation that survived review: without the `ReqId` check the count took any raw answer.
	let car = Car::listing(&[]).codes(0x7E0, &[([0, 1, 2], 0x08)]);
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.step(true, false);
	let host = bench
		.planner
		.exchange(
			bench.clock,
			Class::Remote,
			Unit {
				request: 0x7E0,
				response: 0x7E8,
			},
			vec![0x19, 0x02, 0xFF],
		)
		.expect("allowed");
	// The host's `Remote` goes before the count's `Background`.
	let out = bench.due().expect("the host's request");
	assert_eq!(out.pdu, [0x19, 0x02, 0xFF]);
	assert_eq!(bench.count.deadline_ms(bench.planner.flying_raw()), None);
	let Bench {
		planner, count, said, clock, ..
	} = &mut bench;
	for delivery in planner.answered(*clock + 5, out.token, Answer::Pdu(vec![0x59, 0x02, 0xFF, 0, 1, 2, 0x08])) {
		let theirs = count.take(delivery, &mut |line: &Line<'_>| said.push(line.to_string()));
		assert!(matches!(theirs, Some(Delivery::Raw { req, .. }) if req == host), "{theirs:?}");
	}
	assert_eq!(bench.said.len(), 1, "only the start: {:?}", bench.said);
	let gateway = bench.exchange(&car, 5).expect("the count's own request is still there");
	assert_eq!(gateway.unit.request, 0x710);
}

#[test]
fn once_it_has_found_something_the_count_holds_no_heap() {
	// Review, 2026-09-27: the tally — 12 B an answering unit, 4 B a walked one — was kept for
	// the whole boot, about 1 KB at 64 units.
	let car = Car::listing(&[0x70C, 0x714, 0x746]).codes(0x7E0, &[([0, 1, 2], 0x08)]);
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.step(true, false);
	bench.exchange(&car, 5).expect("the gateway");
	let walking = std::mem::take(&mut bench.count);
	assert!(meter::freed_by(|| drop(walking)) > 0, "a count under way holds its walk");

	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.run(&car, 5);
	assert!(bench.count.found().is_some());
	let done = std::mem::take(&mut bench.count);
	assert_eq!(meter::freed_by(|| drop(done)), 0, "and one that has found something none");
}

/// The test binary's allocator: the system's, counting what each thread frees, so a test sees
/// what one drop gives back while the others run beside it.
mod meter {
	use std::alloc::{GlobalAlloc, Layout, System};
	use std::cell::Cell;

	thread_local! {
		static FREED: Cell<usize> = const { Cell::new(0) };
	}

	struct Meter;

	// SAFETY: every call is passed to `System` unchanged; the counter is a const-initialised
	// thread-local `Cell` with no destructor, which never allocates.
	unsafe impl GlobalAlloc for Meter {
		unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
			// SAFETY: the caller's contract, passed on.
			unsafe { System.alloc(layout) }
		}

		unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
			let _ = FREED.try_with(|n| n.set(n.get() + layout.size()));
			// SAFETY: the caller's contract, passed on.
			unsafe { System.dealloc(ptr, layout) }
		}
	}

	#[global_allocator]
	static METER: Meter = Meter;

	/// Bytes this thread freed while `f` ran.
	pub fn freed_by(f: impl FnOnce()) -> usize {
		let before = FREED.with(Cell::get);
		f();
		FREED.with(Cell::get) - before
	}
}

#[test]
fn a_count_of_zero_is_counted_and_draws_nothing() {
	let car = Car::listing(&[0x70C])
		.codes(0x7E0, &[])
		.codes(0x7E1, &[])
		.codes(0x710, &[])
		.codes(0x70C, &[]);
	let mut bench = Bench::new();
	bench.clock = START_MS;
	bench.run(&car, 5);
	let found = bench.count.found().expect("counted");
	assert_eq!(
		found.badge(),
		Faults::Counted {
			stored: 0,
			failing_now: false
		}
	);
	assert!(
		bench
			.said
			.last()
			.unwrap()
			.starts_with("faults: 0 stored, 0 failing now; 4 of 4 units answered in ")
	);
}

#[test]
fn the_board_wakes_for_the_start_and_for_nothing_after_it() {
	let car = Car::listing(&[]);
	let mut bench = Bench::new();
	assert_eq!(bench.count.wake_ms(0), Some(START_MS));
	assert_eq!(bench.count.wake_ms(START_MS - 1), Some(START_MS));
	// Past it with no plan unit answering yet: a moment gone by would wake the bus task at once,
	// for ever. What starts the count now comes with a part check's answer.
	assert_eq!(bench.count.wake_ms(START_MS), None);
	assert_eq!(bench.count.wake_ms(60_000), None);
	bench.clock = START_MS;
	bench.run(&car, 5);
	assert_eq!(bench.count.wake_ms(0), None, "started: nothing to wake for");
}

#[test]
fn what_the_board_says_word_for_word() {
	let lines = [
		(
			Line::Codes(UnitTally {
				request: 0x7E0,
				stored: 2,
				failing_now: 1,
			}),
			"faults: 7E0 2 stored, 1 failing now",
		),
		(
			Line::NotCounted {
				request: 0x7E1,
				why: Why::StillPending,
			},
			"faults: 7E1 not counted — asked for time (78), no answer in 2 s",
		),
		(
			Line::NotCounted {
				request: 0x7E1,
				why: Why::NoAnswer,
			},
			"faults: 7E1 not counted — no answer",
		),
		(
			Line::NotCounted {
				request: 0x746,
				why: Why::Refused(0x22),
			},
			"faults: 746 not counted — refused, NRC 22",
		),
		(Line::PastBlock(2), "faults: the list set 2 bits past 7BF — not decoded, not asked"),
		(
			Line::Unaddressable(1),
			"faults: the list names 1 id past 795 — no answer id fits 11 bits, not asked",
		),
		(
			Line::Counted {
				stored: 9,
				failing_now: 1,
				answered: 17,
				asked: 18,
				took_ms: 1_450,
				paused_ms: 0,
			},
			"faults: 9 stored, 1 failing now; 17 of 18 units answered in 1.4 s",
		),
		(
			Line::TooMany(65),
			"faults: the walk would ask 65 units, more than 64 — not a car's list, badge ?",
		),
		(Line::NoneAnswered(18), "faults: none of 18 units answered — badge ?"),
		(
			Line::Counted {
				stored: 9,
				failing_now: 1,
				answered: 17,
				asked: 18,
				took_ms: 1_450,
				paused_ms: 12_340,
			},
			"faults: 9 stored, 1 failing now; 17 of 18 units answered in 1.4 s, not counting 12.3 s paused",
		),
		(Line::Waiting(Hold::Stopwatch), "faults: waiting while the stopwatch is up"),
		(
			Line::Waiting(Hold::HostTiming),
			"faults: waiting while a host times a run on the board's timing channel",
		),
		(Line::Resumed, "faults: counting on where it stopped"),
		(
			Line::GivenUp,
			"faults: the board turned adapter — the same request again when the panel is back",
		),
	];
	for (line, text) in lines {
		assert_eq!(line.to_string(), text);
	}
	let skipped = [
		Failed {
			request: 0x776,
			why: Why::SharedId,
		},
		Failed {
			request: 0x7E1,
			why: Why::NoAnswer,
		},
		Failed {
			request: 0x777,
			why: Why::SharedId,
		},
	];
	assert_eq!(
		Line::Skipped(&skipped).to_string(),
		"faults: 776, 777 skipped — each shares an id with a unit walked"
	);
	assert_eq!(
		Line::Skipped(&skipped[..2]).to_string(),
		"faults: 776 skipped — shares an id with a unit walked",
		"one skipped, said as one"
	);
}

#[test]
fn state_says_stored_and_failing_now_or_a_question_mark_and_fits_the_line() {
	// `9/1` beside `page=0/3` read as nine of one (review, 2026-09-27).
	let counted = Found::Counted { stored: 9, failing_now: 1 };
	assert_eq!(format!(" {counted}"), " faults=9 failing=1");
	assert_eq!(format!(" {}", Found::Failed), " faults=?");
	let widest = Found::Counted {
		stored: u32::MAX,
		failing_now: u32::MAX,
	};
	assert_eq!(format!(" {widest}").len(), faults::STATE_LONGEST);
}

#[test]
fn what_the_panel_reads_every_frame_is_twelve_bytes() {
	// `FAULTS` in the firmware: one static, copied out once a frame.
	assert_eq!(std::mem::size_of::<Option<Found>>(), 12);
}
