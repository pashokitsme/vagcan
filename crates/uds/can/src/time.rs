//! One clock, two runtimes.
//!
//! `isotp.rs` needs exactly three things to hold a deadline: "now", "now plus a
//! `Duration`", and "how much of that is left". On the host those come from
//! `tokio::time`; on the board from `embassy_time`. Everything above this
//! module speaks [`core::time::Duration`], which is the same type in both
//! worlds, so the ISO-TP state machine never learns which runtime it is on.

#[cfg(any(test, not(feature = "std")))]
use core::time::Duration;

pub use imp::{Instant, sleep};

/// Longest wait this module will represent, in microseconds — a bit over an
/// hour. `embassy_time::Duration::from_micros` scales by the tick rate *before*
/// dividing, so an unclamped `u64` overflows and a very long wait silently
/// becomes a very short one. Nothing in ISO-TP comes near this: the longest
/// constant in the protocol is N_Bs, one second.
#[cfg(any(test, not(feature = "std")))]
const MAX_MICROS: u64 = u32::MAX as u64;

/// A `Duration` as whole microseconds, clamped so the conversion above cannot
/// wrap. Lives outside both `imp` modules so the host test suite covers it.
#[cfg(any(test, not(feature = "std")))]
fn clamp_micros(d: Duration) -> u64 {
	u64::try_from(d.as_micros()).unwrap_or(MAX_MICROS).min(MAX_MICROS)
}

#[cfg(feature = "std")]
mod imp {
	use core::time::Duration;

	/// tokio's `Instant` already has `now`, `+ Duration` and
	/// `saturating_duration_since`, so the host side is a plain alias.
	pub type Instant = tokio::time::Instant;

	pub async fn sleep(d: Duration) {
		tokio::time::sleep(d).await;
	}
}

#[cfg(not(feature = "std"))]
mod imp {
	use core::ops::Add;
	use core::time::Duration;

	use super::clamp_micros;

	/// `core::time::Duration` -> `embassy_time::Duration`.
	///
	/// `from_micros` already rounds **up** to whole ticks, which is what a
	/// deadline wants: a tick coarser than a microsecond must not round a wait
	/// down to zero and turn it into a spin.
	fn to_embassy(d: Duration) -> embassy_time::Duration {
		embassy_time::Duration::from_micros(clamp_micros(d))
	}

	/// A monotonic instant on the board, with the same three operations the
	/// host's `tokio::time::Instant` offers.
	#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
	pub struct Instant(embassy_time::Instant);

	impl Instant {
		pub fn now() -> Self {
			Instant(embassy_time::Instant::now())
		}

		/// Time from `earlier` to `self`, or zero if `earlier` is not earlier.
		pub fn saturating_duration_since(self, earlier: Instant) -> Duration {
			match self.0.checked_duration_since(earlier.0) {
				Some(d) => Duration::from_micros(d.as_micros()),
				None => Duration::ZERO,
			}
		}
	}

	impl Add<Duration> for Instant {
		type Output = Instant;
		fn add(self, rhs: Duration) -> Instant {
			Instant(self.0 + to_embassy(rhs))
		}
	}

	pub async fn sleep(d: Duration) {
		embassy_time::Timer::after(to_embassy(d)).await;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_isotp_deadline_converts_exactly() {
		// The constants the state machine actually uses.
		assert_eq!(clamp_micros(Duration::from_millis(1000)), 1_000_000, "N_Bs");
		assert_eq!(clamp_micros(Duration::from_millis(0x7F)), 127_000, "largest STmin");
		assert_eq!(clamp_micros(Duration::from_micros(100)), 100, "smallest STmin");
		assert_eq!(clamp_micros(Duration::ZERO), 0);
	}

	#[test]
	fn absurd_durations_clamp_instead_of_wrapping() {
		// A wrap here would turn "wait forever" into "do not wait", which the
		// ISO-TP loop would read as an immediate timeout.
		assert_eq!(clamp_micros(Duration::MAX), MAX_MICROS);
		assert_eq!(clamp_micros(Duration::from_secs(u64::from(u32::MAX))), MAX_MICROS);
	}

	#[tokio::test(start_paused = true)]
	async fn a_deadline_expires_and_stops_being_in_the_future() {
		let deadline = Instant::now() + Duration::from_millis(50);
		assert_eq!(deadline.saturating_duration_since(Instant::now()), Duration::from_millis(50));
		sleep(Duration::from_millis(60)).await;
		assert_eq!(
			deadline.saturating_duration_since(Instant::now()),
			Duration::ZERO,
			"past deadlines saturate to zero, never wrap"
		);
	}
}
