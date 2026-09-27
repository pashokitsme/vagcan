//! The board's one fault count (`todo/dash/20`): when it starts, when it waits, what it asks
//! the planner, what it says on USB, and what the panel and `state` show.
//!
//! [`vag_uds_client::faultcount`] is the count itself — the gateway's installation list, the
//! walk, each unit's stored codes — with no clock and no bus. This is the board's shell round
//! it, pure too, like [`crate::saving`]: the firmware cannot be built for the host, so the
//! decisions live here, where `research/dash/host/tests/fault_count.rs` compiles this file as it
//! is and drives it through the planner the board runs. The bus task holds one [`Count`], gives
//! it a turn before every [`Planner::due`], offers it every delivery, and does what it says.
//!
//! * **Once, [`START_MS`] after boot, and once a plan unit has answered** its part check,
//!   matched or not: a unit that answers is a bus that is on. A board on permanent +12 V with
//!   the ignition off counts once the ignition comes on; a plan with no units starts at 10 s
//!   ([`bus_on`]). Never again that boot, whatever it found: a count that failed is not retried.
//! * **Through the planner, as the part checks are:** each request is one [`Class::Background`]
//!   exchange, and one at a time. The panel and a host keep their turns; nothing here owns the
//!   bus. Two reads only — `22 2A26` once, `19 02 08` a unit — and no session, ever.
//! * **Its own deadline**, [`DEADLINE_MS`] from the send, `78`s included (owner, 2026-09-27):
//!   a unit that has not answered by then is not counted. The board's other exchanges keep
//!   theirs (`PENDING_DEADLINE`, 10 s, in the firmware).
//! * **Not while the stopwatch is up** (owner, 2026-09-27): no request of the count's starts,
//!   one waiting in the planner is taken back, and the walk goes on where it stopped once the
//!   stopwatch closes. One already on the bus runs to its end — at most [`DEADLINE_MS`], longer
//!   than the stopwatch's silence (`stopwatch::SILENCE_MS`), and harmless: the mode's turn resets
//!   the stopwatch, and it arms only on a second of standstill answers, none of which can come
//!   while the count's exchange holds the bus. So no run is armed while one is out.
//! * **Published** once it ends ([`Count::found`]): the badge ([`Found::badge`]) and `state`'s
//!   `faults=` ([`Found`]'s `Display`). Nothing before the end: a count half done is no count.

use core::fmt;

use vag_dash_render::Faults;
use vag_uds_client::address;
use vag_uds_client::faultcount::{Failed, FaultCount, MAX_UNITS, Outcome, Step, UnitTally, Why};
use vag_uds_client::schedule::{Answer, Class, Delivery, Planner, ReqId};

/// The board's clock at which the count may start: ten seconds after boot (owner, 2026-09-26).
pub const START_MS: u64 = 10_000;

/// How long one exchange of the count's may hold the bus, from its send, `78`s (response
/// pending) included (owner, 2026-09-27). Past it the unit is not counted.
pub const DEADLINE_MS: u64 = 2_000;

/// The longest `faults=` `state` carries, its leading space included: both counts at
/// `u32::MAX`. For the firmware's check of the whole line against the characteristic.
pub const STATE_LONGEST: usize = " faults=".len() + 2 * "4294967295".len() + "/".len();

/// Whether the bus is on as far as the count cares: the plan has no units to wait for, or at
/// least one of them has answered its part check.
pub fn bus_on(plan_units: usize, answered: usize) -> bool {
	plan_units == 0 || answered > 0
}

/// How long the next wait of an exchange may be under a deadline of `limit_ms` from its send,
/// `held_ms` after it: `wait_ms`, or what is left of the deadline where that is less. `None`
/// once nothing is left — the exchange is over, with no answer.
pub fn within(limit_ms: u64, held_ms: u64, wait_ms: u64) -> Option<u64> {
	let left = limit_ms.checked_sub(held_ms).filter(|left| *left > 0)?;
	Some(wait_ms.min(left))
}

/// What the bus task knows at one turn of the count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
	/// The board's clock.
	pub ms: u64,
	/// [`bus_on`].
	pub bus_on: bool,
	/// The stopwatch is up (its mode is on, whatever holds the glass).
	pub stopwatch: bool,
}

/// What the count found, for the panel and `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
	/// Stored codes across every unit that answered, and of those the ones failing now.
	Counted { stored: u32, failing_now: u32 },
	/// The gateway gave no list, or a list longer than the board may walk.
	Failed,
}

impl Found {
	/// The panel's badge: the count, inverted while a code fails now; `?` for a count that
	/// failed.
	pub fn badge(self) -> Faults {
		match self {
			Found::Counted { stored, failing_now } => Faults::Counted {
				stored,
				failing_now: failing_now > 0,
			},
			Found::Failed => Faults::Failed,
		}
	}
}

/// `state`'s value: `9/1`, stored and failing now, or `?`.
impl fmt::Display for Found {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Found::Counted { stored, failing_now } => write!(f, "{stored}/{failing_now}"),
			Found::Failed => f.write_str("?"),
		}
	}
}

/// One line for the USB log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'a> {
	/// The count starts.
	Started,
	/// The stopwatch opened with the walk under way.
	Paused,
	/// It closed: the walk goes on.
	Resumed,
	/// A unit that answered with stored codes.
	Codes(UnitTally),
	/// A unit left out, and how long its exchange held the bus.
	NotCounted { request: u16, why: Why, held_ms: u64 },
	/// The listed ids not asked because each shares an id with a unit walked: every
	/// [`Why::SharedId`] among these.
	Skipped(&'a [Failed]),
	/// Ids the list named that no unit can answer on — past VW's block, or in it past `0x795` —
	/// counted and not asked ([`Tally::unaddressable`](vag_uds_client::faultcount::Tally::unaddressable)).
	Unaddressable(u32),
	/// The end of a count.
	Counted {
		stored: u32,
		failing_now: u32,
		answered: usize,
		asked: usize,
		took_ms: u64,
	},
	/// The gateway gave no list, and how long its exchange held the bus.
	NoList { why: Why, held_ms: u64 },
	/// The walk would ask this many units — the list's and the three it cannot hold — more than
	/// [`MAX_UNITS`].
	TooMany(usize),
}

/// Why a unit, or the gateway's list, is not in the count, in words. No answer after the
/// whole of [`DEADLINE_MS`] is a unit that asked for more time (`78`) and never gave the
/// answer; before it, a unit that did not answer at all.
struct Because {
	why: Why,
	held_ms: u64,
}

impl fmt::Display for Because {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self.why {
			Why::NoAnswer if self.held_ms >= DEADLINE_MS => write!(f, "no answer in {} s", DEADLINE_MS / 1000),
			Why::NoAnswer => f.write_str("no answer"),
			Why::BusError => f.write_str("bus error"),
			Why::Refused(nrc) => write!(f, "refused, NRC {nrc:02X}"),
			Why::Malformed => f.write_str("answer did not parse"),
			Why::SharedId => f.write_str("shares an id with a unit walked"),
		}
	}
}

impl fmt::Display for Line<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("faults: ")?;
		match *self {
			Line::Started => f.write_str("counting the car's stored codes — the gateway's list first"),
			Line::Paused => f.write_str("waiting while the stopwatch is up"),
			Line::Resumed => f.write_str("the stopwatch is closed — counting on"),
			Line::Codes(unit) => write!(f, "{:03X} {} stored, {} failing now", unit.request, unit.stored, unit.failing_now),
			Line::NotCounted { request, why, held_ms } => write!(f, "{request:03X} not counted — {}", Because { why, held_ms }),
			Line::Skipped(failed) => {
				let skipped = failed.iter().filter(|f| f.why == Why::SharedId);
				for (i, unit) in skipped.enumerate() {
					if i > 0 {
						f.write_str(", ")?;
					}
					write!(f, "{:03X}", unit.request)?;
				}
				f.write_str(" skipped — each shares an id with a unit walked")
			}
			Line::Unaddressable(ids) => write!(
				f,
				"the list names {ids} {} past {:03X}, which no unit can answer on — not asked",
				if ids == 1 { "id" } else { "ids" },
				address::VW_LAST_ADDRESSABLE
			),
			Line::Counted {
				stored,
				failing_now,
				answered,
				asked,
				took_ms,
			} => write!(
				f,
				"{stored} stored, {failing_now} failing now; {answered} of {asked} units answered in {}.{} s",
				took_ms / 1000,
				took_ms % 1000 / 100
			),
			Line::NoList { why, held_ms } => write!(f, "the gateway gave no list ({}) — badge ?", Because { why, held_ms }),
			Line::TooMany(units) => write!(f, "the walk would ask {units} units, more than {MAX_UNITS} — not a car's list, badge ?"),
		}
	}
}

/// The board's fault count, from its start to what it found.
#[derive(Debug)]
pub struct Count {
	count: FaultCount,
	/// When it started, on the board's clock; `None` before.
	started_ms: Option<u64>,
	/// The exchange waiting in the planner or on the bus for the request the count asks now.
	asked: Option<ReqId>,
	/// The stopwatch holds the walk up, and that has been said.
	paused: bool,
	/// The walk's own lines — the skips, the bits past the block — have been said.
	walk_said: bool,
	/// Entries of the tally's `read` and `failed` already said.
	said_read: usize,
	said_failed: usize,
	/// What it found, once it is over.
	found: Option<Found>,
}

impl Default for Count {
	fn default() -> Self {
		Self::new()
	}
}

impl Count {
	/// A count not started.
	pub fn new() -> Self {
		Count {
			count: FaultCount::new(),
			started_ms: None,
			asked: None,
			paused: false,
			walk_said: false,
			said_read: 0,
			said_failed: 0,
			found: None,
		}
	}

	/// The count's turn, before the bus task asks the planner what is due: start when it may,
	/// queue the next request, or — the stopwatch up — take back the one waiting. Called with
	/// nothing of the count's on the bus, as the bus task does; one that is out is left to run.
	pub fn step(&mut self, now: Now, planner: &mut Planner, say: &mut impl FnMut(&Line<'_>)) {
		if self.found.is_some() {
			return;
		}
		if self.started_ms.is_none() {
			if now.ms < START_MS || !now.bus_on || now.stopwatch {
				return;
			}
			self.started_ms = Some(now.ms);
			say(&Line::Started);
		}
		if now.stopwatch {
			// A request still waiting in the planner comes back; one already on the bus cannot.
			if self.asked.is_some_and(|req| planner.cancel(req)) {
				self.asked = None;
			}
			if !self.paused {
				self.paused = true;
				say(&Line::Paused);
			}
			return;
		}
		if self.paused {
			self.paused = false;
			say(&Line::Resumed);
		}
		if self.asked.is_some() {
			return;
		}
		// Over is `found`, set with the last answer: there is always a request here.
		let Step::Ask { unit, pdu } = self.count.next() else {
			return;
		};
		match planner.exchange(now.ms, Class::Background, unit, pdu) {
			Ok(req) => self.asked = Some(req),
			// Never for the two reads the count sends. Were the planner to refuse one, the unit
			// is left out as a bus error and the walk goes on.
			Err(_) => self.heard(Answer::BusError, 0, now.ms, say),
		}
	}

	/// A delivery, if it is the count's answer: fed to the count, and what it says said. Any
	/// other comes back for its owner.
	pub fn take(&mut self, delivery: Delivery, say: &mut impl FnMut(&Line<'_>)) -> Option<Delivery> {
		match delivery {
			Delivery::Raw {
				req, answer, sent_ms, at_ms, ..
			} if self.asked == Some(req) => {
				self.asked = None;
				self.heard(answer, at_ms.saturating_sub(sent_ms), at_ms, say);
				None
			}
			other => Some(other),
		}
	}

	/// The deadline of the exchange on the bus, if it is the count's: [`DEADLINE_MS`].
	pub fn deadline_ms(&self, flying: Option<ReqId>) -> Option<u64> {
		(flying.is_some() && flying == self.asked).then_some(DEADLINE_MS)
	}

	/// When the bus task has to look again for the count's sake, at `now_ms`: at [`START_MS`],
	/// while that is still to come and the count has not started. Never a moment gone by, which
	/// would wake it at once, for ever. Everything else that moves the count comes with a wake of
	/// its own: a part check's answer, the stopwatch closing, the panel back from adapter mode.
	pub fn wake_ms(&self, now_ms: u64) -> Option<u64> {
		(self.started_ms.is_none() && now_ms < START_MS).then_some(START_MS)
	}

	/// What it found; `None` until it is over.
	pub fn found(&self) -> Option<Found> {
		self.found
	}

	/// One answer for the request the count asked, `held_ms` on the bus, arrived at `now_ms`.
	fn heard(&mut self, answer: Answer, held_ms: u64, now_ms: u64, say: &mut impl FnMut(&Line<'_>)) {
		self.count.answered(answer);
		if let Some(tally) = self.count.tally() {
			// The list is in: what the walk left out of it, once.
			if !self.walk_said {
				self.walk_said = true;
				if tally.unaddressable > 0 {
					say(&Line::Unaddressable(tally.unaddressable));
				}
				if tally.failed.iter().any(|f| f.why == Why::SharedId) {
					say(&Line::Skipped(&tally.failed));
				}
				self.said_failed = tally.failed.len();
			}
			// Then each unit as it is heard from: one answer, one unit.
			for unit in &tally.read[self.said_read..] {
				if unit.stored > 0 {
					say(&Line::Codes(*unit));
				}
			}
			for failed in &tally.failed[self.said_failed..] {
				say(&Line::NotCounted {
					request: failed.request,
					why: failed.why,
					held_ms,
				});
			}
			self.said_read = tally.read.len();
			self.said_failed = tally.failed.len();
		}
		let Some(outcome) = self.count.outcome() else {
			return;
		};
		let took_ms = now_ms.saturating_sub(self.started_ms.unwrap_or(now_ms));
		match outcome {
			Outcome::Counted(tally) => {
				let (stored, failing_now) = (tally.stored(), tally.failing_now());
				say(&Line::Counted {
					stored,
					failing_now,
					answered: tally.units_read(),
					asked: tally.units_read() + tally.failed.iter().filter(|f| f.why != Why::SharedId).count(),
					took_ms,
				});
				self.found = Some(Found::Counted { stored, failing_now });
			}
			Outcome::NoList(why) => {
				say(&Line::NoList { why: *why, held_ms });
				self.found = Some(Found::Failed);
			}
			Outcome::TooMany { units } => {
				say(&Line::TooMany(*units));
				self.found = Some(Found::Failed);
			}
		}
	}
}
