//! The bus scheduler's pure core: who is asked what, when, and who hears the answer.
//!
//! Design: `todo/dash/14-one-bus-three-clients.md` §2, "subscriptions, not a priority
//! queue" (owner, 2026-09-14). One [`Planner`] is the only thing that decides what goes
//! on the bus — on the board under embassy, and on the laptop under tokio for `watch`,
//! `measure` and the one-shot commands. It has **no clock, no bus and no runtime**:
//!
//! - consumers [`subscribe`](Planner::subscribe) to an identifier at a period, ask for
//!   one [`read_once`](Planner::read_once), or queue a raw [`exchange`](Planner::exchange);
//! - the shell asks [`due`](Planner::due) with the time, puts the [`Outgoing`] PDU on the
//!   bus, and hands the result back to [`answered`](Planner::answered) with the time the
//!   answer arrived, which returns the [`Delivery`]s to route to consumers.
//!
//! The shell owns the async side: a `Subscription` handle whose `Drop` calls
//! [`unsubscribe`](Planner::unsubscribe), the one-exchange-at-a-time transport, the clock.
//!
//! # Compression
//!
//! - One read per `(unit, identifier)` however many subscribe; the effective period is
//!   the shortest live one, and every subscriber gets every reading.
//! - A request carries the identifier that triggered it plus every other identifier of
//!   that unit that is due, or due within [`Budget::pull_forward_permille`] of its
//!   period, up to [`Budget::max_dids_per_request`], as one `22 d1 … dn`. A one-shot
//!   is due at once, so it rides with anything of its unit that is near, and a read
//!   pulled forward keeps its phase.
//! - The answer is cut by record length when the plan knows every length
//!   ([`split_by_lengths`]), by the unique-parse search otherwise ([`split_records`]).
//!   An answer that does not read exactly one way is not guessed: its identifiers go
//!   out singly next time.
//! - A unit is single-only for the planner's lifetime once it answers a
//!   multi-identifier request definitely against it: an NRC, an empty positive answer,
//!   or three unsplittable answers in a row while not every record length is known.
//!   Silence never teaches this: no answer is an absent unit, backed off like any other.
//! - A request for a [`Class::Timing`] identifier carries only Timing identifiers
//!   (normally one), and no other request carries one, so the speed answer stays short.
//!
//! # Budget
//!
//! Every send is spaced at least `1000 / ceiling_per_s` ms from the previous one, so no
//! 1000 ms window holds more than the ceiling. When a slot opens, the candidate sent is
//! the best by, in order:
//!
//! 1. [`Class::Timing`] — never thinned.
//! 2. A `Remote` or `Background` item due for longer than [`Budget::starve_after_ms`] —
//!    nothing waits forever.
//! 3. [`Class::Foreground`] while it has had fewer than `foreground_floor_per_s` sends
//!    in the last 1000 ms — the floor.
//! 4. [`Class::Remote`] — waits when the budget is short, never dropped.
//! 5. [`Class::Foreground`] above its floor.
//! 6. [`Class::Background`] — thinned first.
//!
//! and within one rank, the most overdue first. A read shared by several classes ranks
//! as the best of the classes due in it. Nothing is ever dropped: over budget means
//! later. A unit that stops answering is backed off (doubling from
//! [`Budget::backoff_first_ms`] to [`Budget::backoff_cap_ms`]), every subscriber of it is
//! told [`Miss::NoAnswer`] per failed attempt, and the other units keep their slots.
//!
//! # Assumptions the shell must hold
//!
//! - **One [`Outgoing`] in flight.** [`Planner::due`] returns [`Next::Idle`] until the
//!   token it sent is [`answered`](Planner::answered).
//! - **`7F xx 78` (response pending) is the transport's business.** The shell waits it
//!   out and hands over the final answer. A `78` that reaches the planner is taken as a
//!   refusal.
//! - **`now_ms` is monotonic**, and in `answered` it is the moment the answer arrived:
//!   every [`Delivery`] is stamped with it, and `measure` times runs from it.
//! - Rates come from the caller (the plan's `hz`); the planner derives none.

mod planner;
mod split;
#[cfg(test)]
mod tests;

use alloc::vec::Vec;

pub use planner::Planner;
pub use split::{Records, split_by_lengths, split_records};

/// A control unit: the id it is asked on and the id it answers on.
///
/// Both ids, not a unit number: the mapping between them is per id block (ISO 15765-4's
/// `+8` beside VW's own), and it is the shell's to resolve before subscribing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Unit {
	pub request: u16,
	pub response: u16,
}

/// Who a request is for, as the budget sees it.
///
/// Named for how the budget treats a consumer, not for which program it is, so one
/// planner serves both shells; the shell maps its consumers onto them:
///
/// | class | board | laptop |
/// |---|---|---|
/// | `Timing` | the stopwatch's speed channel during a run | `measure` |
/// | `Foreground` | the visible page, the stalk poll | `watch`, `info`, `faults` |
/// | `Remote` | the laptop's PDUs over USB or BLE | — |
/// | `Background` | pages not shown | anything polled for later |
///
/// The order of the variants is the order of precedence, floor aside (see the module
/// docs): `Timing` first, `Background` last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
	/// Never thinned.
	Timing,
	/// Keeps [`Budget::foreground_floor_per_s`] whenever it wants it; above the floor it
	/// yields to `Remote`.
	Foreground,
	/// A client across a link: waits when the budget is short, never dropped.
	Remote,
	/// Thinned first.
	Background,
}

/// The limits one shell runs the planner under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
	/// Exchanges per second, all classes together. Owner, 2026-09-13: 100, half of what
	/// the one conversation could do, so the car's units see a sparse, even trickle.
	pub ceiling_per_s: u16,
	/// What [`Class::Foreground`] keeps whenever it wants it. Owner, 2026-09-14: 25, a
	/// four-cell page.
	pub foreground_floor_per_s: u16,
	/// Identifiers in one `22` request. Default 8, the value the laptop's old `plan::BATCH`
	/// was measured at on the reference car (eight answered, twelve refused); a unit that
	/// accepts fewer is learned as single-only rather than guessed at.
	pub max_dids_per_request: u8,
	/// How near its due time, in thousandths of its period, a read is pulled into a
	/// request its unit is making anyway. 250 = a quarter period.
	pub pull_forward_permille: u16,
	/// The first wait after a unit fails to answer; it doubles per failure.
	pub backoff_first_ms: u32,
	/// The longest wait between attempts on a silent unit. Default 2 s, the firmware's
	/// `DEAD_BUS_GAP` before the planner.
	pub backoff_cap_ms: u32,
	/// A `Remote` or `Background` item due for longer than this ranks just below
	/// `Timing` until it is sent, so nothing waits forever. Owner, 2026-09-14: 5 s.
	pub starve_after_ms: u32,
}

impl Default for Budget {
	fn default() -> Self {
		Budget {
			ceiling_per_s: 100,
			foreground_floor_per_s: 25,
			max_dids_per_request: 8,
			pull_forward_permille: 250,
			backoff_first_ms: 250,
			backoff_cap_ms: 2000,
			starve_after_ms: 5000,
		}
	}
}

/// A live subscription. Unsubscribing it is what dropping the shell's handle does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SubId(pub(crate) u32);

/// A one-shot read or a raw exchange, delivered exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReqId(pub(crate) u32);

/// Names the one request in flight; [`Planner::answered`] takes it back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Token(pub(crate) u32);

/// What [`Planner::due`] says to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
	/// Put this on the bus now.
	Send(Outgoing),
	/// Nothing to send now. Ask again at `until_ms`; `None` means nothing will come due
	/// by time alone — wait for the answer in flight or for a new request.
	Idle { until_ms: Option<u64> },
}

/// One request for the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
	pub token: Token,
	pub unit: Unit,
	/// The whole UDS request PDU, service id first.
	pub pdu: Vec<u8>,
}

/// What came back for an [`Outgoing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
	/// A response PDU, service id first. A negative response (`7F sid nrc`) may come
	/// this way or as [`Answer::Refused`]; the planner treats both alike.
	Pdu(Vec<u8>),
	/// A negative response, by its NRC.
	Refused(u8),
	/// Nothing within the transport's deadline.
	NoAnswer,
	/// The request could not be put on the bus, or the bus failed under it.
	BusError,
}

/// Why a subscriber got no reading, or a one-shot no data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Miss {
	/// The unit did not answer.
	NoAnswer,
	/// The bus failed under the request.
	BusError,
	/// The unit refused the identifier, with this NRC (`31`: not supported).
	Refused(u8),
	/// A multi-identifier answer that left this identifier out — what a unit does with
	/// one it does not support.
	Absent,
	/// An answer that is not a response to what was asked.
	Malformed,
}

/// What [`Planner::answered`] hands to consumers. `at_ms` is the `now_ms` given to
/// `answered`: the moment the answer arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivery {
	/// A subscriber's reading: the record's bytes, identifier echo stripped.
	Reading {
		sub: SubId,
		unit: Unit,
		did: u16,
		data: Vec<u8>,
		at_ms: u64,
	},
	/// A subscriber's reading that did not come.
	Missed {
		sub: SubId,
		unit: Unit,
		did: u16,
		why: Miss,
		at_ms: u64,
	},
	/// A one-shot read's result.
	Once {
		req: ReqId,
		unit: Unit,
		did: u16,
		result: Result<Vec<u8>, Miss>,
		at_ms: u64,
	},
	/// A raw exchange's answer, as it came.
	Raw { req: ReqId, unit: Unit, answer: Answer, at_ms: u64 },
}
