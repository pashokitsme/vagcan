//! The value store: the last thing the car said about each plan channel, and when — and how
//! long that may stay on the glass.
//!
//! The board's bus task writes a [`Slot`] per answer and its panel task reads them each frame;
//! the laptop's replay writes them from a recording. A value the bus stopped refreshing does
//! not stay on the glass looking current: past its [`fresh_for_ms`] it is no value, and the
//! panel draws a dash.
//!
//! Times are microseconds of the caller's clock — the board's `Instant` (embassy's 1 MHz tick),
//! the recording's own time on a laptop — so the board's comparisons are tick for tick what
//! they were before the store moved here.

/// How old a value may be and still be shown. A shown channel is read several times a second
/// and a hidden one once; a value nobody has refreshed for five seconds is a bus that went
/// quiet, and the panel should say so rather than hold the last number.
pub const STALE_MS: u64 = 5_000;

/// How long a value of a channel read every `period_ms` may be shown: three periods of a
/// channel read slower than [`STALE_MS`] allows, else [`STALE_MS`].
pub const fn fresh_for_ms(period_ms: u32) -> u64 {
	let three = period_ms as u64 * 3;
	if three > STALE_MS { three } else { STALE_MS }
}

/// One channel's last reading.
///
/// `value` is what the panel draws; `None` is a dash, never a zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot {
	value: Option<f32>,
	/// When `value` was heard. Read only with a value: a slot with none is no value whenever
	/// it is asked.
	at_us: u64,
	fresh_for_us: u64,
}

impl Slot {
	/// Nothing heard yet.
	pub const EMPTY: Slot = Slot {
		value: None,
		at_us: 0,
		fresh_for_us: STALE_MS * 1000,
	};

	/// A reading heard at `at_us`, to be shown for `fresh_for_ms` after ([`fresh_for_ms`]).
	/// `None` — a read that missed, an answer that did not decode — is no value at once, not
	/// the last value aging out.
	pub const fn heard(value: Option<f32>, at_us: u64, fresh_for_ms: u64) -> Slot {
		Slot {
			value,
			at_us,
			fresh_for_us: fresh_for_ms * 1000,
		}
	}

	/// The value at `now_us`, unless it is older than it may be shown.
	pub fn current(&self, now_us: u64) -> Option<f32> {
		self.within(now_us, self.fresh_for_us)
	}

	/// The value at `now_us` if it was heard at most `within_us` before, whatever it may be
	/// shown for — the lever's witnesses ask their own age of it.
	pub fn within(&self, now_us: u64, within_us: u64) -> Option<f32> {
		let value = self.value?;
		(now_us.saturating_sub(self.at_us) <= within_us).then_some(value)
	}
}

impl Default for Slot {
	fn default() -> Self {
		Slot::EMPTY
	}
}

/// Channel `index`'s value in `slots` at `now_us`: `None` for a channel the store has no slot
/// for, one never heard, one that missed, and one older than it may be shown.
pub fn current(slots: &[Slot], index: u16, now_us: u64) -> Option<f32> {
	slots.get(usize::from(index))?.current(now_us)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_value_is_shown_for_its_fresh_for_and_not_a_tick_longer() {
		let slot = Slot::heard(Some(4.2), 1_000_000, STALE_MS);
		assert_eq!(slot.current(1_000_000), Some(4.2));
		assert_eq!(slot.current(1_000_000 + STALE_MS * 1000), Some(4.2), "exactly STALE_MS old is shown");
		assert_eq!(slot.current(1_000_000 + STALE_MS * 1000 + 1), None, "a microsecond past is not");
		// A clock behind the reading — a slot written after the frame's `now` was taken — is
		// no age at all, not a wrap.
		assert_eq!(slot.current(999_000), Some(4.2));
	}

	#[test]
	fn a_slow_channel_is_shown_for_three_of_its_periods_a_fast_one_for_stale() {
		assert_eq!(fresh_for_ms(0), STALE_MS);
		assert_eq!(fresh_for_ms(100), STALE_MS);
		assert_eq!(fresh_for_ms(1_000), STALE_MS, "the hidden rate: three periods are less than STALE");
		assert_eq!(fresh_for_ms(1_666), STALE_MS);
		assert_eq!(fresh_for_ms(1_667), 5_001);
		assert_eq!(fresh_for_ms(4_000), 12_000);
	}

	#[test]
	fn a_miss_is_no_value_at_once_and_nothing_heard_is_none() {
		assert_eq!(Slot::heard(None, 1_000, STALE_MS).current(1_000), None);
		assert_eq!(Slot::EMPTY.current(0), None);
		assert_eq!(Slot::default(), Slot::EMPTY);
		assert_eq!(current(&[Slot::heard(Some(1.0), 0, STALE_MS)], 1, 0), None, "no slot for it");
		assert_eq!(current(&[Slot::heard(Some(1.0), 0, STALE_MS)], 0, 0), Some(1.0));
	}

	#[test]
	fn within_asks_its_own_age_whatever_the_slot_may_be_shown_for() {
		let slot = Slot::heard(Some(1.0), 10_000, STALE_MS);
		assert_eq!(slot.within(10_000 + 600_000, 600_000), Some(1.0));
		assert_eq!(slot.within(10_000 + 600_001, 600_000), None);
		assert_eq!(Slot::heard(None, 10_000, STALE_MS).within(10_000, 600_000), None);
	}
}
