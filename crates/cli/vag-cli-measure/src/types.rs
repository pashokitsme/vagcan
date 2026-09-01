//! The two shapes a channel can take through a run, and nothing else.
//!
//! A run is a set of channels sampled at different instants, so a channel is
//! **columnar** — its own values against its own timestamps. That is not a
//! storage optimisation: identifiers are polled in batches and no two channels
//! share a clock, and a layout that implied they did has already cost this
//! project one wrong proof (`todo/README.md`, the gear evidence).
//!
//! Numbers and states are separate types because they behave differently
//! between samples. A speed between two readings is interpolated; a gear
//! between two readings is whatever it was, and interpolating it would invent
//! half a gear.

/// Seconds from the run's own zero. Negative before the launch, since the ring
/// buffer keeps what happened before it.
pub type Seconds = f64;

/// A measured quantity: values with the times they were read at.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
	pub t: Vec<Seconds>,
	pub v: Vec<f64>,
}

/// A discrete state — the engaged gear, the selector lever.
///
/// Values are the **labels** the catalog gives, never the codes behind them.
/// The codes are neither contiguous nor ordered by ratio, and two of them are
/// not gears at all; reading them as numbers is the mistake this project
/// already made once, when `gear + 1` reported reverse as "gear 11".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct States {
	pub t: Vec<Seconds>,
	pub v: Vec<String>,
}

impl Track {
	pub fn push(&mut self, t: Seconds, v: f64) {
		self.t.push(t);
		self.v.push(v);
	}

	pub fn len(&self) -> usize {
		self.t.len()
	}

	pub fn is_empty(&self) -> bool {
		self.t.is_empty()
	}

	/// The value at an arbitrary time, linearly interpolated.
	///
	/// `None` outside the span rather than the nearest end: a derived figure
	/// computed from an extrapolated input is not a measurement, and the
	/// caller has to be able to tell the difference.
	pub fn at(&self, t: Seconds) -> Option<f64> {
		let last = self.t.last().copied()?;
		if t < *self.t.first()? || t > last {
			return None;
		}
		match self.t.binary_search_by(|probe| probe.total_cmp(&t)) {
			Ok(i) => Some(self.v[i]),
			Err(0) => Some(self.v[0]),
			Err(i) => {
				let (t0, t1) = (self.t[i - 1], self.t[i]);
				let (v0, v1) = (self.v[i - 1], self.v[i]);
				let span = t1 - t0;
				if span <= 0.0 {
					return Some(v1);
				}
				Some(v0 + (v1 - v0) * (t - t0) / span)
			}
		}
	}

	/// When the value first rises past `target` at or after `after`.
	///
	/// Interpolated between the bracketing samples, not rounded to the nearer
	/// one: at 20 Hz a whole sample is 50 ms, which is most of what separates
	/// two attempts at the same mark.
	pub fn crossing(&self, target: f64, after: Seconds) -> Option<Seconds> {
		for i in 1..self.len() {
			if self.t[i] < after {
				continue;
			}
			let (v0, v1) = (self.v[i - 1], self.v[i]);
			if v0 < target && v1 >= target {
				let (t0, t1) = (self.t[i - 1], self.t[i]);
				let rise = v1 - v0;
				if rise <= 0.0 {
					return Some(t1);
				}
				return Some(t0 + (t1 - t0) * (target - v0) / rise);
			}
		}
		None
	}

	/// The samples between two times, endpoints included.
	pub fn window(&self, from: Seconds, to: Seconds) -> Track {
		let mut out = Track::default();
		for i in 0..self.len() {
			if self.t[i] >= from && self.t[i] <= to {
				out.push(self.t[i], self.v[i]);
			}
		}
		out
	}
}

impl States {
	pub fn push(&mut self, t: Seconds, v: impl Into<String>) {
		self.t.push(t);
		self.v.push(v.into());
	}

	pub fn len(&self) -> usize {
		self.t.len()
	}

	/// Whether nothing has been recorded yet.
	///
	/// Beside `len` because this type crossed a crate boundary when `measure`
	/// became its own crate: a public `len` with no `is_empty` is a rough edge
	/// for every caller that is not this file.
	pub fn is_empty(&self) -> bool {
		self.len() == 0
	}

	/// What was in force at `t` — a step function held until the next reading.
	/// `None` before the first sample, because nothing was established yet.
	pub fn at(&self, t: Seconds) -> Option<&str> {
		let mut found = None;
		for i in 0..self.len() {
			if self.t[i] <= t {
				found = Some(self.v[i].as_str());
			} else {
				break;
			}
		}
		found
	}

	/// Every change of label, in order, as `(when, from, to)`.
	///
	/// Repeated readings of the same label are not changes: a channel polled at
	/// 20 Hz reports the same gear twenty times a second.
	pub fn transitions(&self) -> Vec<(Seconds, String, String)> {
		let mut out = Vec::new();
		for i in 1..self.len() {
			if self.v[i] != self.v[i - 1] {
				out.push((self.t[i], self.v[i - 1].clone(), self.v[i].clone()));
			}
		}
		out
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ramp() -> Track {
		// 0 to 10 m/s over one second, sampled unevenly on purpose.
		let mut t = Track::default();
		for (at, v) in [(0.0, 0.0), (0.23, 2.3), (0.51, 5.1), (0.74, 7.4), (1.0, 10.0)] {
			t.push(at, v);
		}
		t
	}

	#[test]
	fn a_value_between_samples_is_interpolated_not_rounded() {
		let track = ramp();
		// Halfway between 0.51 and 0.74 the ramp is at 6.25; the nearer sample
		// would say 5.1 or 7.4, and at 20 Hz that error is most of what
		// separates two attempts at the same mark.
		assert!((track.at(0.625).unwrap() - 6.25).abs() < 1e-9);
		assert_eq!(track.at(0.0), Some(0.0));
		assert_eq!(track.at(1.0), Some(10.0));
	}

	#[test]
	fn outside_the_span_there_is_no_value_rather_than_the_nearest_one() {
		let track = ramp();
		assert_eq!(track.at(-0.1), None);
		assert_eq!(track.at(1.1), None);
	}

	#[test]
	fn a_crossing_is_interpolated_and_ignores_anything_before_the_start() {
		let track = ramp();
		let at = track.crossing(5.0, 0.0).unwrap();
		assert!((at - 0.5).abs() < 1e-9, "{at}");
		// A crossing that already happened is not this run's crossing.
		assert!(track.crossing(5.0, 0.6).is_none());
	}

	#[test]
	fn a_falling_channel_never_reports_a_crossing() {
		let mut track = Track::default();
		for (at, v) in [(0.0, 10.0), (0.5, 5.0), (1.0, 0.0)] {
			track.push(at, v);
		}
		assert_eq!(track.crossing(5.0, 0.0), None);
	}

	#[test]
	fn a_state_is_held_until_the_next_reading_and_never_interpolated() {
		let mut gears = States::default();
		gears.push(0.0, "1");
		gears.push(1.0, "2");
		assert_eq!(gears.at(-0.1), None, "nothing was established yet");
		assert_eq!(gears.at(0.5), Some("1"), "held, not half-way to 2");
		assert_eq!(gears.at(5.0), Some("2"));
	}

	#[test]
	fn repeated_readings_of_the_same_state_are_not_transitions() {
		let mut gears = States::default();
		for (at, g) in [(0.0, "1"), (0.05, "1"), (0.1, "1"), (0.15, "2"), (0.2, "2")] {
			gears.push(at, g);
		}
		let changes = gears.transitions();
		assert_eq!(changes.len(), 1);
		assert_eq!(changes[0].0, 0.15);
		assert_eq!((changes[0].1.as_str(), changes[0].2.as_str()), ("1", "2"));
	}

	#[test]
	fn a_window_keeps_its_endpoints() {
		let track = ramp();
		let w = track.window(0.23, 0.74);
		assert_eq!(w.len(), 3);
		assert_eq!(w.t.first(), Some(&0.23));
		assert_eq!(w.t.last(), Some(&0.74));
	}
}
