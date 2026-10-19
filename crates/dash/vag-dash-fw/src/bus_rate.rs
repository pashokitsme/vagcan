//! How many exchanges the board's bus makes, counted where they happen: the number the build's
//! bus-time estimate (`vag-cli-core`'s `dash/bus_time.rs`) takes as a constant, measured on the
//! car instead (`.archive/tasks/done/dash/24-bus-time-check.md`).
//!
//! The panel's bus task tells [`BusRate`] of every exchange that ends — its unit, when it went
//! out, when it ended, whether the unit answered — and closes a window every [`WINDOW_MS`]. A
//! closed [`Window`] is said on the USB console ([`ConsoleLine`]) and kept for the `state` line
//! ([`StateKeys`]): exchanges a second, and how long an answered one took, request out to
//! answer in — the per-exchange time of this car's units, from a drive.
//!
//! Pure, like [`crate::exchange`]: `research/dash/host/tests/bus_rate.rs` compiles this file as
//! it is. Small on purpose: a few counters, [`UNITS`] of them by unit.

use core::fmt;

/// How long a window counts, in milliseconds.
pub const WINDOW_MS: u64 = 10_000;
/// The units counted one by one, in the order they first end an exchange in a window; the rest
/// are summed as `others`. A plan reads a handful of units; the fault count asks more.
pub const UNITS: usize = 6;

/// One unit's exchanges in a window. `request` 0 is a slot not taken.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UnitCount {
	pub request: u16,
	pub exchanges: u16,
}

/// A window of the bus, closed or still counting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Window {
	/// How long it counted.
	pub ms: u32,
	/// Exchanges that ended in it, answered or not.
	pub exchanges: u32,
	/// Of those, the ones a unit answered.
	pub answered: u32,
	/// Request out to the end, every exchange.
	pub busy_ms: u32,
	/// Request out to the answer, the answered ones.
	pub answered_ms: u32,
	pub units: [UnitCount; UNITS],
	/// Exchanges of units past the first [`UNITS`].
	pub others: u16,
}

impl Window {
	/// Exchanges a second, in tenths.
	pub fn per_s_tenths(&self) -> u32 {
		tenths_per_s(self.exchanges, self.ms)
	}

	/// How long an answered exchange took on average, in tenths of a millisecond; `None` with
	/// none answered.
	pub fn answered_ms_tenths(&self) -> Option<u32> {
		(self.answered > 0).then(|| (u64::from(self.answered_ms) * 10 / u64::from(self.answered)) as u32)
	}

	/// The share of the window the bus was in an exchange, in percent, at most 100.
	pub fn busy_percent(&self) -> u32 {
		(u64::from(self.busy_ms) * 100 / u64::from(self.ms.max(1))).min(100) as u32
	}
}

/// `count` in `ms`, as tenths of one a second.
fn tenths_per_s(count: u32, ms: u32) -> u32 {
	(u64::from(count) * 10_000 / u64::from(ms.max(1))).min(u64::from(u32::MAX)) as u32
}

/// The window being counted.
#[derive(Debug, Clone, Copy)]
pub struct BusRate {
	since_ms: u64,
	open: Window,
}

impl BusRate {
	pub const fn new(now_ms: u64) -> Self {
		BusRate {
			since_ms: now_ms,
			open: Window {
				ms: 0,
				exchanges: 0,
				answered: 0,
				busy_ms: 0,
				answered_ms: 0,
				units: [UnitCount { request: 0, exchanges: 0 }; UNITS],
				others: 0,
			},
		}
	}

	/// One exchange ended: on unit `request`, sent at `sent_ms`, ended at `at_ms`, answered or
	/// not. Counted in the window it ends in.
	pub fn ended(&mut self, request: u16, sent_ms: u64, at_ms: u64, answered: bool) {
		let took = at_ms.saturating_sub(sent_ms).min(u64::from(u32::MAX)) as u32;
		let open = &mut self.open;
		open.exchanges = open.exchanges.saturating_add(1);
		open.busy_ms = open.busy_ms.saturating_add(took);
		if answered {
			open.answered = open.answered.saturating_add(1);
			open.answered_ms = open.answered_ms.saturating_add(took);
		}
		match open.units.iter_mut().find(|u| u.request == request || u.request == 0) {
			Some(unit) => {
				unit.request = request;
				unit.exchanges = unit.exchanges.saturating_add(1);
			}
			None => open.others = open.others.saturating_add(1),
		}
	}

	/// When the window being counted closes.
	pub fn closes_at_ms(&self) -> u64 {
		self.since_ms + WINDOW_MS
	}

	/// The window, once [`WINDOW_MS`] have passed: closed at `now_ms` and a new one begun there.
	pub fn tick(&mut self, now_ms: u64) -> Option<Window> {
		if now_ms < self.closes_at_ms() {
			return None;
		}
		let mut closed = self.open;
		closed.ms = now_ms.saturating_sub(self.since_ms).min(u64::from(u32::MAX)) as u32;
		*self = BusRate::new(now_ms);
		Some(closed)
	}
}

/// A count in tenths, as `29.4`; at most `9999.9`.
struct Tenths(u32);

impl fmt::Display for Tenths {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let v = self.0.min(99_999);
		write!(f, "{}.{}", v / 10, v % 10)
	}
}

/// The `state` line's keys: ` exch=29.4/s exch_ms=34.0` — exchanges a second, and how long an
/// answered one took — ` exch_ms=-` with none answered, ` exch=- exch_ms=-` before a window has
/// closed. `dashcfg` skips a key it does not know.
pub struct StateKeys(pub Option<Window>);

/// The longest [`StateKeys`] writes.
pub const STATE_LONGEST: usize = " exch=9999.9/s exch_ms=9999.9".len();

impl fmt::Display for StateKeys {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let Some(window) = self.0 else {
			return write!(f, " exch=- exch_ms=-");
		};
		write!(f, " exch={}/s", Tenths(window.per_s_tenths()))?;
		match window.answered_ms_tenths() {
			Some(ms) => write!(f, " exch_ms={}", Tenths(ms)),
			None => write!(f, " exch_ms=-"),
		}
	}
}

/// The console's line for a closed window:
/// `bus: 29.5 exchanges/s, 29.4 answered in 34.0 ms each, 100 % busy — 7E0 19.6/s, 75A 9.8/s`.
pub struct ConsoleLine<'a>(pub &'a Window);

impl fmt::Display for ConsoleLine<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		let w = self.0;
		write!(
			f,
			"bus: {} exchanges/s, {} answered",
			Tenths(w.per_s_tenths()),
			Tenths(tenths_per_s(w.answered, w.ms))
		)?;
		if let Some(ms) = w.answered_ms_tenths() {
			write!(f, " in {} ms each", Tenths(ms))?;
		}
		write!(f, ", {} % busy", w.busy_percent())?;
		let mut first = true;
		for unit in w.units.iter().filter(|u| u.request != 0) {
			let sep = if first { " — " } else { ", " };
			first = false;
			write!(f, "{sep}{:03X} {}/s", unit.request, Tenths(tenths_per_s(u32::from(unit.exchanges), w.ms)))?;
		}
		if w.others > 0 {
			write!(f, ", others {}/s", Tenths(tenths_per_s(u32::from(w.others), w.ms)))?;
		}
		Ok(())
	}
}
