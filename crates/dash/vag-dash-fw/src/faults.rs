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
//! * **Its own deadline**, [`DEADLINE_MS`] from the exchange's start, the send and `78`s
//!   included (owner, 2026-09-27;
//!   the waits are [`crate::exchange`]'s): a unit that has not answered by then is not
//!   counted. One that asked for time (`78`) is still there, and the planner is told so
//!   (`Answer::Busy`): its readers miss one sample (`Miss::Busy`) and it is not declared
//!   absent; it is backed off as a non-answer is. The board's other exchanges keep their
//!   deadlines (`PENDING_DEADLINE`, 10 s, in the firmware).
//! * **Not while a stopwatch runs** ([`Hold`]): the board's own is up (owner, 2026-09-27), or a
//!   host holds the board's timing channel — a laptop's `vagcan measure` through the board
//!   (review, 2026-09-27, flagged to the owner). No request of the count's starts, one waiting
//!   in the planner is taken back, and the walk goes on where it stopped when nothing holds it;
//!   the time held is said apart from the count's own. One already on the bus runs to its end
//!   — at most [`DEADLINE_MS`], longer than the stopwatch's silence (`stopwatch::SILENCE_MS`),
//!   and harmless: the mode's turn resets the stopwatch, and it arms only on a second of
//!   standstill answers, none of which can come while the count's exchange holds the bus. So no
//!   run is armed while one is out.
//! * **Adapter mode** gives the exchange on the bus up; that is the board's doing, not the
//!   car's answer, and the same request goes out again when the panel is back
//!   ([`Count::given_up`]).
//! * **`?` when there is no count** ([`Found::Failed`]): the gateway gave no list, the list
//!   made a walk longer than [`MAX_UNITS`], or no unit of the walk could be counted. Before the count
//!   ends: nothing, a count half done being no count.
//! * **Published** once it ends ([`Count::found`]): the badge ([`Found::badge`]) and `state`'s
//!   `faults=` ([`State`]'s `Display`, ` faults=-` before). The count's walk and tally are
//!   dropped then.

use core::fmt;

use crate::exchange::RESPONSE_TIMEOUT_MS;
use vag_dash_render::Faults;
use vag_uds_client::address;
use vag_uds_client::faultcount::{Failed, FaultCount, MAX_UNITS, Outcome, Step, UnitTally, Why};
use vag_uds_client::schedule::{Answer, Class, Delivery, Planner, ReqId};

/// The board's clock at which the count may start: ten seconds after boot (owner, 2026-09-26).
pub const START_MS: u64 = 10_000;

/// How long one exchange of the count's may hold the bus, from the exchange's start, the send
/// included, `78`s (response pending) included (owner, 2026-09-27). Past it the unit is not
/// counted.
pub const DEADLINE_MS: u64 = 2_000;

/// The longest of what `state` carries of the count ([`State`]), its leading space included:
/// both counts at `u32::MAX`, the units at `u8::MAX`. For the firmware's check of the whole line
/// against its buffer.
pub const STATE_LONGEST: usize = " faults=4294967295 failing=4294967295 units=255/255".len();

/// Whether the bus is on as far as the count cares: the plan has no units to wait for, or at
/// least one of them has answered its part check.
pub fn bus_on(plan_units: usize, answered: usize) -> bool {
	plan_units == 0 || answered > 0
}

/// What the bus task knows at one turn of the count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
	/// The board's clock.
	pub ms: u64,
	/// [`bus_on`].
	pub bus_on: bool,
	/// What holds the count up now, if anything.
	pub hold: Option<Hold>,
}

/// What holds the count up: a stopwatch whose run a 2 s exchange would cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
	/// The board's stopwatch is up (its mode is on, whatever holds the glass).
	Stopwatch,
	/// A host holds the board's timing channel: a laptop's `vagcan measure` through the board.
	HostTiming,
}

/// What the count found, for the panel and `state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
	/// Stored codes across every unit counted, and of those the ones failing now; how many units
	/// were counted of how many asked — the count is of the units that answered (owner's rule), and
	/// a partial walk shows on the bench (review round 2). Units at most [`MAX_UNITS`], so a byte.
	Counted {
		stored: u32,
		failing_now: u32,
		units_counted: u8,
		units_asked: u8,
	},
	/// No count: the gateway gave no list, a list longer than the board may walk, or no unit
	/// of the walk could be counted.
	Failed,
}

impl Found {
	/// The panel's badge: the count, inverted while a code fails now; `?` for a count that
	/// failed.
	pub fn badge(self) -> Faults {
		match self {
			Found::Counted { stored, failing_now, .. } => Faults::Counted {
				stored,
				failing_now: failing_now > 0,
			},
			Found::Failed => Faults::Failed,
		}
	}
}

/// What `state` says of the count, its leading space included: ` faults=9 failing=1
/// units=17/18` once counted — not `9/1`, which beside `page=0/3` read as nine of one — ` faults=?`
/// when there is no count, and ` faults=-` until it ends, so that no key at all means an image
/// without the count (review rounds 1 and 2, 2026-09-27).
pub struct State(pub Option<Found>);

impl fmt::Display for State {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self.0 {
			Some(Found::Counted {
				stored,
				failing_now,
				units_counted,
				units_asked,
			}) => write!(f, " faults={stored} failing={failing_now} units={units_counted}/{units_asked}"),
			Some(Found::Failed) => f.write_str(" faults=?"),
			None => f.write_str(" faults=-"),
		}
	}
}

/// One line for the USB log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Line<'a> {
	/// The count starts.
	Started,
	/// Something holds the walk up.
	Waiting(Hold),
	/// Nothing does any more: the walk goes on.
	Resumed,
	/// The exchange on the bus was given up for adapter mode.
	GivenUp,
	/// No unit of the walk could be counted, of this many asked.
	NoneCounted(usize),
	/// A unit that answered with stored codes.
	Codes(UnitTally),
	/// A unit left out, and why.
	NotCounted { request: u16, why: Why },
	/// The listed ids not asked because each shares an id with a unit walked: every
	/// [`Why::SharedId`] among these.
	Skipped(&'a [Failed]),
	/// Bits the list set past VW's block, never decoded, never asked
	/// ([`Tally::past_block`](vag_uds_client::faultcount::Tally::past_block)).
	PastBlock(u32),
	/// Ids of VW's block the list named past `0x795`, whose answer id would not fit 11 bits:
	/// not asked ([`Tally::unaddressable`](vag_uds_client::faultcount::Tally::unaddressable)).
	Unaddressable(u32),
	/// The end of a count: its time, and apart from it the time it was held up.
	Counted {
		stored: u32,
		failing_now: u32,
		counted: usize,
		asked: usize,
		took_ms: u64,
		paused_ms: u64,
	},
	/// The gateway gave no list, and why.
	NoList(Why),
	/// The walk would ask this many units — the list's and the three it cannot hold — more than
	/// [`MAX_UNITS`].
	TooMany(usize),
}

/// Why a unit, or the gateway's list, is not in the count, in words.
struct Because(Why);

impl fmt::Display for Because {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self.0 {
			Why::NoAnswer => f.write_str("no answer"),
			// The count's own deadline cut the exchange after the unit's `78`.
			Why::Busy { asked_for_time: true } => write!(f, "asked for time (78), no answer in {} s", DEADLINE_MS / 1000),
			// A late answer does not lengthen the first answer's wait: the window is the board's
			// answer timeout.
			Why::Busy { asked_for_time: false } => write!(
				f,
				"sent only a late answer to an earlier request, none to this one in {}.{} s",
				RESPONSE_TIMEOUT_MS / 1000,
				RESPONSE_TIMEOUT_MS % 1000 / 100
			),
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
			Line::Waiting(Hold::Stopwatch) => f.write_str("waiting while the stopwatch is up"),
			Line::Waiting(Hold::HostTiming) => f.write_str("waiting while a host holds the board's timing channel (vagcan measure)"),
			Line::GivenUp => f.write_str("the board turned adapter — the same request again when the panel is back"),
			Line::NoneCounted(asked) => write!(f, "none of {asked} units could be counted — badge ?"),
			Line::Resumed => f.write_str("counting on where it stopped"),
			Line::Codes(unit) => write!(f, "{:03X} {} stored, {} failing now", unit.request, unit.stored, unit.failing_now),
			Line::NotCounted { request, why } => write!(f, "{request:03X} not counted — {}", Because(why)),
			Line::Skipped(failed) => {
				let skipped = || failed.iter().filter(|f| f.why == Why::SharedId);
				for (i, unit) in skipped().enumerate() {
					if i > 0 {
						f.write_str(", ")?;
					}
					write!(f, "{:03X}", unit.request)?;
				}
				match skipped().count() {
					1 => f.write_str(" skipped — shares an id with a unit walked"),
					_ => f.write_str(" skipped — each shares an id with a unit walked"),
				}
			}
			Line::PastBlock(bits) => write!(
				f,
				"the list set {bits} {} past {:03X} — not decoded, not asked",
				if bits == 1 { "bit" } else { "bits" },
				address::VW_LAST
			),
			Line::Unaddressable(ids) => write!(
				f,
				"the list names {ids} {} past {:03X} — no answer id fits 11 bits, not asked",
				if ids == 1 { "id" } else { "ids" },
				address::VW_LAST_ADDRESSABLE
			),
			Line::Counted {
				stored,
				failing_now,
				counted,
				asked,
				took_ms,
				paused_ms,
			} => {
				write!(
					f,
					"{stored} stored, {failing_now} failing now; {counted} of {asked} units counted in {}.{} s",
					took_ms / 1000,
					took_ms % 1000 / 100
				)?;
				if paused_ms > 0 {
					write!(f, ", not counting {}.{} s paused", paused_ms / 1000, paused_ms % 1000 / 100)?;
				}
				Ok(())
			}
			Line::NoList(why) => write!(f, "the gateway gave no list ({}) — badge ?", Because(why)),
			Line::TooMany(units) => write!(f, "the walk would ask {units} units, more than {MAX_UNITS} — not a car's list, badge ?"),
		}
	}
}

/// The board's fault count, from its start to what it found.
#[derive(Debug)]
pub struct Count {
	/// The count itself; `None` once it has found something, so its walk and tally — about
	/// 1 KB of heap at 64 units — are not kept for the rest of the boot.
	count: Option<FaultCount>,
	/// When it started, on the board's clock; `None` before.
	started_ms: Option<u64>,
	/// The exchange waiting in the planner or on the bus for the request the count asks now.
	asked: Option<ReqId>,
	/// What holds the walk up, as last said.
	held: Option<Hold>,
	/// Since when, and how long it was held up before.
	held_since_ms: Option<u64>,
	paused_ms: u64,
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
			count: Some(FaultCount::new()),
			started_ms: None,
			asked: None,
			held: None,
			held_since_ms: None,
			paused_ms: 0,
			walk_said: false,
			said_read: 0,
			said_failed: 0,
			found: None,
		}
	}

	/// The count's turn, before the bus task asks the planner what is due: start when it may,
	/// queue the next request, or — held up by a stopwatch — take back the one waiting. Called
	/// with nothing of the count's on the bus, as the bus task does; one that is out is left to
	/// run.
	pub fn step(&mut self, now: Now, planner: &mut Planner, say: &mut impl FnMut(&Line<'_>)) {
		if self.found.is_some() {
			return;
		}
		if self.started_ms.is_none() {
			if now.ms < START_MS || !now.bus_on {
				return;
			}
			// Past 10 s with the bus on, only a stopwatch keeps the start back: said once (review
			// round 2 — a silent wait read as a count that never began).
			if let Some(hold) = now.hold {
				if self.held != Some(hold) {
					self.held = Some(hold);
					say(&Line::Waiting(hold));
				}
				return;
			}
			// A wait before the start is no pause of the count's.
			self.held = None;
			self.started_ms = Some(now.ms);
			say(&Line::Started);
		}
		if let Some(hold) = now.hold {
			// A request still waiting in the planner comes back; one already on the bus cannot.
			if self.asked.is_some_and(|req| planner.cancel(req)) {
				self.asked = None;
			}
			if self.held != Some(hold) {
				self.held_since_ms.get_or_insert(now.ms);
				self.held = Some(hold);
				say(&Line::Waiting(hold));
			}
			return;
		}
		if self.held.take().is_some() {
			let since = self.held_since_ms.take().unwrap_or(now.ms);
			self.paused_ms = self.paused_ms.saturating_add(now.ms.saturating_sub(since));
			say(&Line::Resumed);
		}
		if self.asked.is_some() {
			return;
		}
		// Over is `found`, set with the last answer, and the count dropped with it: there is
		// always a request here.
		let Some(Step::Ask { unit, pdu }) = self.count.as_ref().map(FaultCount::next) else {
			return;
		};
		match planner.exchange(now.ms, Class::Background, unit, pdu) {
			Ok(req) => self.asked = Some(req),
			// Never for the two reads the count sends. Were the planner to refuse one, the unit
			// is left out as a bus error and the walk goes on.
			Err(_) => self.heard(Answer::BusError, now.ms, say),
		}
	}

	/// The exchange on the bus (`flying`) was given up for adapter mode, before the planner is
	/// answered. If it is the count's, that is the board's doing, not the car's answer: it is not
	/// fed to the count, and the same request goes out again once the panel is back (review,
	/// 2026-09-27 — it was a `BusError`, for the gateway a `?` for the whole boot). The planner's
	/// `Raw` delivery for it then belongs to nobody.
	pub fn given_up(&mut self, flying: Option<ReqId>, say: &mut impl FnMut(&Line<'_>)) {
		if flying.is_some() && flying == self.asked {
			self.asked = None;
			say(&Line::GivenUp);
		}
	}

	/// A delivery, if it is the count's answer: fed to the count, and what it says said. Any
	/// other comes back for its owner.
	pub fn take(&mut self, delivery: Delivery, say: &mut impl FnMut(&Line<'_>)) -> Option<Delivery> {
		match delivery {
			Delivery::Raw { req, answer, at_ms, .. } if self.asked == Some(req) => {
				self.asked = None;
				self.heard(answer, at_ms, say);
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
	/// its own — a part check's answer, the stopwatch closing, the panel back from adapter mode —
	/// or within the second the bus task looks at the pages anyway (a host's timing channel
	/// freed).
	pub fn wake_ms(&self, now_ms: u64) -> Option<u64> {
		(self.started_ms.is_none() && now_ms < START_MS).then_some(START_MS)
	}

	/// What it found; `None` until it is over.
	pub fn found(&self) -> Option<Found> {
		self.found
	}

	/// One answer for the request the count asked, arrived at `now_ms`.
	fn heard(&mut self, answer: Answer, now_ms: u64, say: &mut impl FnMut(&Line<'_>)) {
		let Some(count) = self.count.as_mut() else {
			return;
		};
		count.answered(answer);
		if let Some(tally) = count.tally() {
			// The list is in: what the walk left out of it, once.
			if !self.walk_said {
				self.walk_said = true;
				if tally.past_block > 0 {
					say(&Line::PastBlock(tally.past_block));
				}
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
				});
			}
			self.said_read = tally.read.len();
			self.said_failed = tally.failed.len();
		}
		let Some(outcome) = count.outcome() else {
			return;
		};
		// Time held up by a stopwatch is not the count's: car check 2 reads this as its time.
		let held_now = self.held_since_ms.map_or(0, |since| now_ms.saturating_sub(since));
		let paused_ms = self.paused_ms.saturating_add(held_now);
		let took_ms = now_ms.saturating_sub(self.started_ms.unwrap_or(now_ms)).saturating_sub(paused_ms);
		let found = match outcome {
			Outcome::Counted(tally) => {
				let asked = tally.units_read() + tally.failed.iter().filter(|f| f.why != Why::SharedId).count();
				// Not one unit of the walk answered: no count, not a car with none (review,
				// 2026-09-27 — owner's answer 4 is there so that nothing looks like "no faults").
				if tally.units_read() == 0 {
					say(&Line::NoneCounted(asked));
					Found::Failed
				} else {
					let (stored, failing_now) = (tally.stored(), tally.failing_now());
					say(&Line::Counted {
						stored,
						failing_now,
						counted: tally.units_read(),
						asked,
						took_ms,
						paused_ms,
					});
					Found::Counted {
						stored,
						failing_now,
						units_counted: tally.units_read().min(usize::from(u8::MAX)) as u8,
						units_asked: asked.min(usize::from(u8::MAX)) as u8,
					}
				}
			}
			Outcome::NoList(why) => {
				say(&Line::NoList(*why));
				Found::Failed
			}
			Outcome::TooMany { units } => {
				say(&Line::TooMany(*units));
				Found::Failed
			}
		};
		self.found = Some(found);
		// Its walk and tally are said; nothing of them is needed again this boot.
		self.count = None;
	}
}
