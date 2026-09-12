//! What a chart draws: the last `N` samples of one channel, oldest first.
//!
//! The renderer's chart takes a slice and draws one column per sample. This is
//! the thing that owns that slice, and the rule it enforces is the one the
//! 2026-09-13 defect broke: **the history is kept whether or not the chart is
//! on the glass.** The firmware used to sample only while the chart page was
//! shown and to start over on every entry, so the page came up empty and took
//! seconds to fill — or, flipped past quickly, never filled at all. A history
//! that is fed every frame regardless is full when the page arrives.
//!
//! It is fed per **frame**, not per poll result, and that is a choice: a
//! column is then a fixed slice of time, so the window printed in the chart's
//! header (`samples × frame period`) is true, and a channel that answers at
//! whatever rate the bus allows still draws on a straight time axis. The cost
//! is one `f32` copy per frame per chart, from a store the panel task already
//! reads.
//!
//! A history is of **one channel**. Feeding it another starts it over,
//! because a trace that joins two channels says something neither of them
//! said, and this is where the rule lives rather than in every caller. A frame
//! with **no value** also ends the trace — a unit that went quiet for five
//! seconds and came back would otherwise be drawn as one continuous line
//! across the silence.
//!
//! `N` is the panel width: the chart draws the newest `plot_w` columns and
//! `plot_w` is never wider than the panel, so `N` samples never underfeed it.
//! One history is `N × 4` bytes plus a length and a channel.

/// The last `N` values of one channel, oldest first.
pub struct History<const N: usize> {
	samples: [f32; N],
	len: usize,
	/// Whose values these are, once anything has been pushed.
	channel: Option<u16>,
}

impl<const N: usize> History<N> {
	pub const fn new() -> Self {
		Self {
			samples: [0.0; N],
			len: 0,
			channel: None,
		}
	}

	/// One frame's sample of `channel`. `None` — the channel did not answer,
	/// or its last answer is stale — ends the trace.
	pub fn push(&mut self, channel: u16, value: Option<f32>) {
		if self.channel != Some(channel) {
			self.channel = Some(channel);
			self.len = 0;
		}
		let Some(value) = value else {
			self.len = 0;
			return;
		};
		if N == 0 {
			return;
		}
		if self.len < N {
			self.samples[self.len] = value;
			self.len += 1;
		} else {
			// Full: the oldest column falls off the left, as it does on the
			// glass. A memmove of `N × 4` bytes once a frame is nothing.
			self.samples.copy_within(1.., 0);
			self.samples[N - 1] = value;
		}
	}

	/// Oldest first, at most `N`. What goes straight into
	/// [`Frame::Chart::samples`](crate::Frame::Chart).
	pub fn samples(&self) -> &[f32] {
		&self.samples[..self.len]
	}

	/// The channel this is a history of, once anything has been pushed.
	pub fn channel(&self) -> Option<u16> {
		self.channel
	}
}

impl<const N: usize> Default for History<N> {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const BOOST: u16 = 1;
	const RPM: u16 = 2;

	#[test]
	fn it_starts_empty() {
		let history = History::<8>::new();
		assert!(history.samples().is_empty());
		assert_eq!(history.channel(), None);
	}

	#[test]
	fn it_keeps_samples_oldest_first_until_full() {
		let mut history = History::<8>::new();
		for i in 0..5 {
			history.push(BOOST, Some(i as f32));
		}
		assert_eq!(history.samples(), &[0.0, 1.0, 2.0, 3.0, 4.0]);
		assert_eq!(history.channel(), Some(BOOST));
	}

	#[test]
	fn once_full_it_keeps_the_newest_n() {
		let mut history = History::<4>::new();
		for i in 0..10 {
			history.push(BOOST, Some(i as f32));
		}
		assert_eq!(history.samples(), &[6.0, 7.0, 8.0, 9.0]);
	}

	#[test]
	fn it_survives_frames_that_did_not_draw_it() {
		// The page switch is the caller's business: as long as it keeps
		// feeding, nothing here knows or cares which page was on the glass.
		let mut history = History::<8>::new();
		for i in 0..3 {
			history.push(BOOST, Some(i as f32));
		}
		// ...three frames on a values page, still fed...
		for i in 3..6 {
			history.push(BOOST, Some(i as f32));
		}
		// ...and back: six samples, not three and not zero.
		assert_eq!(history.samples(), &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
	}

	#[test]
	fn a_different_channel_starts_it_over() {
		let mut history = History::<8>::new();
		for i in 0..5 {
			history.push(BOOST, Some(i as f32));
		}
		history.push(RPM, Some(900.0));
		assert_eq!(history.samples(), &[900.0]);
		assert_eq!(history.channel(), Some(RPM));
	}

	#[test]
	fn a_missing_value_ends_the_trace() {
		let mut history = History::<8>::new();
		for i in 0..5 {
			history.push(BOOST, Some(i as f32));
		}
		history.push(BOOST, None);
		assert!(history.samples().is_empty(), "silence is not a point on the line");
		history.push(BOOST, Some(7.0));
		assert_eq!(history.samples(), &[7.0], "and the next value starts a new one");
	}

	#[test]
	fn a_zero_width_history_takes_pushes_and_holds_nothing() {
		let mut history = History::<0>::new();
		history.push(BOOST, Some(1.0));
		history.push(BOOST, Some(2.0));
		assert!(history.samples().is_empty());
	}
}
