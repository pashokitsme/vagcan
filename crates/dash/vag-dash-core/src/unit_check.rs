//! One plan unit's part check, as the board runs it: what is known of the unit, when it is
//! asked its part number again, and when a matched unit is taken for silent.
//!
//! The firmware's `PanelReads` holds one [`UnitCheck`] per plan unit and does what it says: on
//! an `F187` answer ([`UnitCheck::on_part`], fed by
//! [`Unit::check_part`](vag_dash_render::plan::Unit::check_part)), on a reading of one of the
//! unit's channels ([`UnitCheck::on_reading`]) and on a missed one ([`UnitCheck::on_miss`]). The
//! answer is an [`Action`]: subscribe the unit's channels, ask its part number now, drop its
//! subscriptions and ask, or nothing — a retry it keeps itself and hands back through
//! [`UnitCheck::take_due`].
//!
//! **Silence is [`SILENT_AFTER`] reads in a row, not one.** A unit taken for silent loses its
//! subscriptions and is asked one thing, its part number, until it answers, so that a bus gone
//! quiet is asked one thing per unit. One unanswered read did that until 2026-10-20, and it was
//! too few: one answer later than the board's timeout
//! ([`RESPONSE_TIMEOUT_MS`](crate::exchange::RESPONSE_TIMEOUT_MS)) struck off a unit that is
//! there, and that answer's refusal, still to come, was then taken as the refusal of the part
//! number asked next ([`PartCheck::Refused`] is the other half of that fix). What counts is an
//! exchange, not a delivery: one batch of identifiers that went unanswered hands every
//! subscriber of it a miss, all at the same `at_ms`, and that is one read. Anything the unit
//! said — a reading, a refusal, an identifier left out, an answer that did not parse, a late
//! one — starts the count over; a bus error says nothing of the unit and leaves the count as it
//! is. Waiting costs nothing the planner does not already pace: it backs a unit off after every
//! unanswered read (250 ms, doubling to its cap), so three reads of a silent unit hold the bus
//! as three reads of its part number would. A channel's value is no value from its first miss.
//!
//! **A retry waits the backoff's cap** ([`Budget::backoff_cap_ms`]): a refusal, a garbled answer
//! and a record with no number in it are answers, which reset the planner's backoff, so the pace
//! is kept here — a unit that keeps refusing is asked its part number once per cap, never in a
//! loop.
//!
//! Held per plan unit for the whole run, so it is packed: the board's clock as a wrapping `u32`
//! of milliseconds (a retry is never more than the cap away, and two misses of one exchange end
//! at the same millisecond), and the rest in bytes. 12 bytes a unit.

use vag_dash_render::plan::{PartAnswer, PartCheck};
use vag_uds_client::schedule::{Budget, Miss};

/// How many of a matched unit's reads in a row may go unanswered before it is taken for silent.
pub const SILENT_AFTER: u8 = 3;

/// What the part check has established about one unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
	/// Asked, not answered yet.
	Pending,
	/// Asked, no answer — or the answer had no part number in it, or the unit was busy; asked
	/// again. A matched unit that went silent comes back here.
	Absent,
	/// Answered with the part number the plan was built against: its channels are subscribed.
	Matched,
	/// Refuses `F187`, at boot or after it matched ([`PartCheck::Refused`]): not polled, and
	/// asked its part number again after the backoff's cap until it names the planned part.
	Refusing,
	/// Named another part number. Never polled again this run.
	Mismatch,
}

/// What the caller is to do after a [`UnitCheck`] event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
	/// Nothing now. A retry, if one is due later, is kept by the check: [`UnitCheck::take_due`].
	None,
	/// The unit matched: subscribe its channels.
	Subscribe,
	/// Ask its part number now; the planner's backoff paces a silent unit.
	Ask,
	/// The unit went silent: drop its subscriptions, clear its values, and ask its part number.
	DropAndAsk,
}

/// [`UnitCheck`]'s flags.
const MATCHED_BEFORE: u8 = 1 << 0;
const MISS_AT: u8 = 1 << 1;
const RETRY: u8 = 1 << 2;

/// One plan unit's part check. See the [module](self).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitCheck {
	/// When the last miss that counted ended (with [`MISS_AT`]): the misses of one exchange share it.
	miss_at_ms: u32,
	/// When the part number is to be asked again (with [`RETRY`]).
	retry_at_ms: u32,
	check: Check,
	/// Unanswered exchanges in a row while matched.
	missed: u8,
	flags: u8,
}

impl Default for UnitCheck {
	fn default() -> Self {
		Self::NEW
	}
}

impl UnitCheck {
	/// A unit not asked yet.
	pub const NEW: UnitCheck = UnitCheck {
		miss_at_ms: 0,
		retry_at_ms: 0,
		check: Check::Pending,
		missed: 0,
		flags: 0,
	};

	/// What is known of the unit.
	pub fn check(&self) -> Check {
		self.check
	}

	/// Whether the unit has matched this run: what the board's line about a refusal names.
	pub fn matched_before(&self) -> bool {
		self.flags & MATCHED_BEFORE != 0
	}

	/// The unit's answer to `F187`, judged by
	/// [`Unit::check_part`](vag_dash_render::plan::Unit::check_part), at `now_ms` on the board's
	/// clock.
	pub fn on_part(&mut self, part: PartCheck, now_ms: u64) -> Action {
		self.flags &= !RETRY;
		match part {
			PartCheck::Matched => {
				self.check = Check::Matched;
				self.flags |= MATCHED_BEFORE;
				self.clear_misses();
				Action::Subscribe
			}
			PartCheck::Mismatch => {
				self.check = Check::Mismatch;
				Action::None
			}
			PartCheck::Absent => {
				self.check = Check::Absent;
				Action::Ask
			}
			PartCheck::RetryLater => {
				self.check = Check::Absent;
				self.retry_after_cap(now_ms);
				Action::None
			}
			PartCheck::Refused => {
				self.check = Check::Refusing;
				self.retry_after_cap(now_ms);
				Action::None
			}
		}
	}

	/// A reading of one of the unit's channels came, whatever it decodes to: the unit is there.
	pub fn on_reading(&mut self, at_ms: u64) -> Action {
		let _ = at_ms;
		self.clear_misses();
		Action::None
	}

	/// A reading of one of the unit's channels did not come, for `why`, at `at_ms`.
	/// [`Action::DropAndAsk`] once, at the [`SILENT_AFTER`]th unanswered exchange in a row of a
	/// matched unit.
	pub fn on_miss(&mut self, why: Miss, at_ms: u64) -> Action {
		if self.check != Check::Matched {
			return Action::None;
		}
		let at = at_ms as u32;
		match why {
			Miss::NoAnswer if self.flags & MISS_AT != 0 && self.miss_at_ms == at => Action::None,
			Miss::NoAnswer => {
				self.miss_at_ms = at;
				self.flags |= MISS_AT;
				self.missed = self.missed.saturating_add(1);
				if self.missed < SILENT_AFTER {
					return Action::None;
				}
				self.clear_misses();
				self.check = Check::Absent;
				Action::DropAndAsk
			}
			Miss::BusError => Action::None,
			Miss::Refused(_) | Miss::Absent | Miss::Malformed | Miss::Busy => {
				self.clear_misses();
				Action::None
			}
		}
	}

	/// Whether the part number is due to be asked again at `now_ms`; once true, the retry is
	/// taken and the caller asks.
	pub fn take_due(&mut self, now_ms: u64) -> bool {
		let due = self.retry_in_ms(now_ms) == Some(0);
		if due {
			self.flags &= !RETRY;
		}
		due
	}

	/// How long from `now_ms` until the part number is asked again, `Some(0)` if that is now;
	/// `None` with no retry waiting. For the bus task's sleep.
	pub fn retry_in_ms(&self, now_ms: u64) -> Option<u64> {
		if self.flags & RETRY == 0 {
			return None;
		}
		// Wrapping: the retry is never more than the cap away, far inside half the `u32` range.
		let left = self.retry_at_ms.wrapping_sub(now_ms as u32) as i32;
		Some(left.max(0) as u64)
	}

	fn retry_after_cap(&mut self, now_ms: u64) {
		self.retry_at_ms = (now_ms as u32).wrapping_add(Budget::board().backoff_cap_ms);
		self.flags |= RETRY;
	}

	fn clear_misses(&mut self) {
		self.missed = 0;
		self.flags &= !MISS_AT;
	}
}

/// The planner's result of an `F187` read, as [`Unit::check_part`](vag_dash_render::plan::Unit::check_part)
/// takes it.
pub fn part_answer<'a>(result: Result<&'a [u8], &Miss>) -> PartAnswer<'a> {
	match result {
		Ok(data) => PartAnswer::Data(data),
		Err(Miss::Refused(nrc)) => PartAnswer::Refused(*nrc),
		Err(Miss::NoAnswer) => PartAnswer::NoAnswer,
		Err(Miss::BusError) => PartAnswer::BusError,
		Err(Miss::Malformed | Miss::Absent) => PartAnswer::Malformed,
		Err(Miss::Busy) => PartAnswer::Busy,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const CAP: u64 = 2_000;

	fn matched() -> UnitCheck {
		let mut u = UnitCheck::NEW;
		assert_eq!(u.on_part(PartCheck::Matched, 0), Action::Subscribe);
		u
	}

	#[test]
	fn the_cap_is_the_boards() {
		assert_eq!(u64::from(Budget::board().backoff_cap_ms), CAP);
	}

	#[test]
	fn it_is_packed() {
		assert_eq!(core::mem::size_of::<UnitCheck>(), 12);
	}

	#[test]
	fn two_unanswered_reads_then_an_answer_leave_the_unit_there() {
		let mut u = matched();
		assert_eq!(u.on_miss(Miss::NoAnswer, 500), Action::None);
		assert_eq!(u.on_miss(Miss::NoAnswer, 1_250), Action::None);
		assert_eq!(u.on_reading(1_300), Action::None);
		assert_eq!(u.on_miss(Miss::NoAnswer, 1_800), Action::None, "the count started over");
		assert_eq!(u.on_miss(Miss::NoAnswer, 2_550), Action::None);
		assert_eq!(u.check(), Check::Matched);
		assert_eq!(u.on_miss(Miss::NoAnswer, 3_550), Action::DropAndAsk, "three in a row since the answer");
		assert_eq!(u.check(), Check::Absent);
	}

	#[test]
	fn the_third_unanswered_read_in_a_row_is_silence_said_once() {
		let mut u = matched();
		assert_eq!(u.on_miss(Miss::NoAnswer, 500), Action::None);
		assert_eq!(u.on_miss(Miss::NoAnswer, 1_250), Action::None);
		assert_eq!(u.on_miss(Miss::NoAnswer, 2_250), Action::DropAndAsk);
		assert_eq!(u.on_miss(Miss::NoAnswer, 3_250), Action::None, "no longer matched");
	}

	/// A batch of identifiers that went unanswered is one exchange: its misses all end at the
	/// same moment, and count once.
	#[test]
	fn the_misses_of_one_exchange_count_once() {
		let mut u = matched();
		for _ in 0..8 {
			assert_eq!(u.on_miss(Miss::NoAnswer, 500), Action::None);
		}
		assert_eq!(u.on_miss(Miss::NoAnswer, 1_250), Action::None);
		assert_eq!(u.on_miss(Miss::NoAnswer, 2_250), Action::DropAndAsk);
	}

	/// A refusal, an identifier left out, an answer that did not parse, a late one: the unit
	/// spoke, and the count starts over. A bus error says nothing of the unit either way.
	#[test]
	fn anything_the_unit_said_starts_the_count_over_and_a_bus_error_is_neither() {
		for spoke in [Miss::Refused(0x22), Miss::Absent, Miss::Malformed, Miss::Busy] {
			let mut u = matched();
			u.on_miss(Miss::NoAnswer, 500);
			u.on_miss(Miss::NoAnswer, 1_250);
			assert_eq!(u.on_miss(spoke, 1_300), Action::None, "{spoke:?}");
			assert_eq!(u.on_miss(Miss::NoAnswer, 1_800), Action::None, "{spoke:?} started it over");
		}
		let mut u = matched();
		u.on_miss(Miss::NoAnswer, 500);
		assert_eq!(u.on_miss(Miss::BusError, 600), Action::None);
		u.on_miss(Miss::NoAnswer, 1_250);
		assert_eq!(
			u.on_miss(Miss::NoAnswer, 2_250),
			Action::DropAndAsk,
			"the bus error neither counted nor reset"
		);
	}

	#[test]
	fn a_unit_that_matches_again_is_counted_afresh() {
		let mut u = matched();
		u.on_miss(Miss::NoAnswer, 500);
		u.on_miss(Miss::NoAnswer, 1_250);
		assert_eq!(u.on_part(PartCheck::Matched, 1_300), Action::Subscribe);
		assert_eq!(u.on_miss(Miss::NoAnswer, 2_250), Action::None);
		assert!(u.matched_before());
	}

	/// A refusal and a record with no number wait the cap, once; silence is asked at once (the
	/// planner paces it); a mismatch is never asked again.
	#[test]
	fn a_retry_waits_the_cap_and_is_taken_once() {
		for (part, check) in [(PartCheck::Refused, Check::Refusing), (PartCheck::RetryLater, Check::Absent)] {
			let mut u = UnitCheck::NEW;
			assert_eq!(u.on_part(part, 1_000), Action::None, "{part:?}");
			assert_eq!(u.check(), check);
			assert!(!u.matched_before());
			assert_eq!(u.retry_in_ms(1_000), Some(CAP));
			assert!(!u.take_due(1_000 + CAP - 1), "{part:?}: not before the cap");
			assert!(u.take_due(1_000 + CAP), "{part:?}: at the cap");
			assert!(!u.take_due(1_000 + CAP + 1), "{part:?}: taken once");
			assert_eq!(u.retry_in_ms(1_000 + CAP + 1), None);
		}
		let mut u = UnitCheck::NEW;
		assert_eq!(u.on_part(PartCheck::Absent, 1_000), Action::Ask);
		assert_eq!(u.retry_in_ms(1_000), None);
		assert_eq!(u.on_part(PartCheck::Mismatch, 1_500), Action::None);
		assert_eq!(u.check(), Check::Mismatch);
		assert_eq!(u.retry_in_ms(1_500), None);
	}

	#[test]
	fn a_refusal_after_a_match_is_named_so() {
		let mut u = matched();
		assert_eq!(u.on_part(PartCheck::Refused, 5_000), Action::None);
		assert_eq!(u.check(), Check::Refusing);
		assert!(u.matched_before());
		assert_eq!(u.on_miss(Miss::NoAnswer, 5_500), Action::None, "not matched now: no reads to miss");
	}

	/// The board's clock past `u32::MAX` milliseconds (49.7 days): the retry is still the cap away.
	#[test]
	fn a_retry_across_the_clocks_wrap_is_on_time() {
		let now = u64::from(u32::MAX) - 500;
		let mut u = UnitCheck::NEW;
		u.on_part(PartCheck::Refused, now);
		assert_eq!(u.retry_in_ms(now), Some(CAP));
		assert!(!u.take_due(now + CAP - 1));
		assert!(u.take_due(now + CAP));
	}

	#[test]
	fn a_part_answer_is_the_planners_result_as_the_check_takes_it() {
		assert_eq!(part_answer(Ok(b"X")), PartAnswer::Data(b"X"));
		assert_eq!(part_answer(Err(&Miss::Refused(0x22))), PartAnswer::Refused(0x22));
		assert_eq!(part_answer(Err(&Miss::NoAnswer)), PartAnswer::NoAnswer);
		assert_eq!(part_answer(Err(&Miss::BusError)), PartAnswer::BusError);
		assert_eq!(part_answer(Err(&Miss::Absent)), PartAnswer::Malformed);
		assert_eq!(part_answer(Err(&Miss::Malformed)), PartAnswer::Malformed);
		assert_eq!(part_answer(Err(&Miss::Busy)), PartAnswer::Busy);
	}
}
