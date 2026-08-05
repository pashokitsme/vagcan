//! What `watch` remembers, so that there is something to draw a chart from.
//!
//! Until this module existed `watch` kept exactly one body per identifier —
//! `App::latest`, the last answer and when it arrived. That is everything a
//! table of current values needs and nothing a chart needs: a chart is not a
//! rendering change, it is a buffer, a policy for trimming it, and the memory
//! that policy implies.
//!
//! One channel's samples are its own. Identifiers are polled in batches and no
//! two channels share a clock, and a layout that implied they did has already
//! cost this project one wrong proof (`todo/README.md`, the gear evidence). So
//! each channel carries its own `(seconds, value)` pairs, exactly as
//! `measure`'s `Track` does — and deliberately not by importing `Track`, which
//! is a numerics type that interpolates and finds crossings and belongs with
//! the physics that needs those. Nothing here interpolates anything.

use std::collections::BTreeMap;
use std::collections::VecDeque;

/// A channel, as `watch` addresses one: the unit's request id and the
/// identifier.
pub type Key = (u16, u16);

/// How much of the past a chart keeps: a fixed **time** window, and not a fixed
/// number of samples.
///
/// The two candidates are a count of samples and a span of seconds, and what
/// decides between them is that `watch`'s poll rate is not a constant. One
/// identifier on one unit answers at tens of hertz; thirty identifiers across
/// four units cost a request per batch and every one of those units' deadlines,
/// and the same screen then cycles once or twice a second. A buffer of N
/// samples is therefore a window of unknown length — the same six hundred
/// points are thirteen seconds of one run and ten minutes of the next — and,
/// worse, its length changes under the reader the moment somebody presses `c`
/// and marks two more channels. A time axis whose extent moves with the
/// selection is one nobody can read. A span of seconds means the same thing in
/// every run and on every car.
///
/// What the choice costs is that the memory is a rate times a window rather
/// than a constant. At the fastest rate this tool has polled at, a minute is a
/// few thousand points per channel, and the channels are the ones somebody
/// selected rather than the thousand a survey puts on offer — a few hundred
/// kilobytes at the top end, for a program that already holds a survey in
/// memory.
///
/// A minute because `watch` is read while it runs: long enough to hold a gear
/// change, an overrun and the recovery from it, short enough that the trace of
/// the last few seconds is not compressed into a stripe. It is printed on the
/// screen next to the chart — a window nobody can see is a chart nobody can
/// read.
pub const WINDOW_SECONDS: f64 = 60.0;

/// Bounded `(seconds, value)` buffers, one per channel.
///
/// Only channels the caller pushes to have any, and only numbers reach it: a
/// channel whose scaling this project has not proven is shown as raw bytes and
/// there is nothing in it to plot. The caller enforces that, because the caller
/// is what holds the scalings.
pub struct History {
    /// The trim policy, in seconds. Carried rather than read from the constant
    /// so a test can state a window in its own terms instead of generating a
    /// minute of samples to observe one.
    window: f64,
    /// A deque per channel, oldest at the front. A deque and not a `Vec`
    /// because trimming happens on every sample and both ends move: dropping
    /// the front of a `Vec` moves everything behind it, thousands of points at
    /// tens of hertz, to save nothing.
    tracks: BTreeMap<Key, VecDeque<(f64, f64)>>,
}

impl History {
    pub fn new(window: f64) -> Self {
        History { window, tracks: BTreeMap::new() }
    }

    /// Record one reading, and trim that channel to the window as it goes.
    ///
    /// **Trimmed before the sample is appended**, not after, and that ordering
    /// is the whole of how a clock that ran backwards is survived: on a replay
    /// the playhead wraps at the end of the recording and `←` seeks backwards
    /// through it. Appending first would leave a point from t=50 sitting behind
    /// a point from t=5, and a buffer out of time order draws a line that goes
    /// back on itself.
    ///
    /// A second reading at a time already recorded **replaces** the first
    /// rather than joining it. A paused replay redraws twenty times a second
    /// with the playhead where it was and hands back that same instant every
    /// time, and a buffer that accepted all of them would grow without bound
    /// with the clock stopped — the one way past a window of seconds, and the
    /// one this had.
    pub fn push(&mut self, key: Key, t: f64, v: f64) {
        let window = self.window;
        let track = self.tracks.entry(key).or_default();
        trim_track(track, t, window);
        // Everything later than `t` is gone by now, so this is the equality.
        if track.back().is_some_and(|(last, _)| *last >= t) {
            track.pop_back();
        }
        track.push_back((t, v));
    }

    /// Drop everything outside the window on every channel, and release the
    /// channels left with nothing.
    ///
    /// [`push`](Self::push) already bounds the channel it writes to, so this is
    /// about the ones nobody is writing to any more: deselect a channel and it
    /// stops being polled, and its last minute would otherwise sit in memory
    /// for the rest of the run and reappear, an hour stale, if it were ever
    /// selected again.
    pub fn trim(&mut self, now: f64) {
        let window = self.window;
        for track in self.tracks.values_mut() {
            trim_track(track, now, window);
        }
        self.tracks.retain(|_, track| !track.is_empty());
    }

    /// One channel's window, oldest first. Empty for a channel that has
    /// answered nothing — which is every channel on the first cycle of a run.
    pub fn points(&self, key: Key) -> Vec<(f64, f64)> {
        self.tracks.get(&key).map(|t| t.iter().copied().collect()).unwrap_or_default()
    }
}

/// Keep `now - window ..= now`, from a buffer held in time order.
///
/// Both ends, because both ends move. The front is the trim policy; the back is
/// a clock that went backwards, which on a replay is an ordinary keypress and
/// not a fault.
fn trim_track(track: &mut VecDeque<(f64, f64)>, now: f64, window: f64) {
    while track.front().is_some_and(|(t, _)| *t < now - window) {
        track.pop_front();
    }
    while track.back().is_some_and(|(t, _)| *t > now) {
        track.pop_back();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Key = (0x7E0, 0x2029);
    const B: Key = (0x7E1, 0x380A);

    #[test]
    fn a_run_of_minutes_keeps_a_window_of_seconds() {
        // The reason the buffer exists at all: `watch` is left running, and an
        // untrimmed buffer at tens of hertz grows for as long as it is.
        let mut history = History::new(10.0);
        for i in 0..=100 {
            history.push(A, i as f64, i as f64);
        }
        let points = history.points(A);
        assert_eq!(points.first().map(|p| p.0), Some(90.0));
        assert_eq!(points.last().map(|p| p.0), Some(100.0));
        assert_eq!(points.len(), 11);
    }

    #[test]
    fn a_clock_that_went_backwards_takes_the_future_with_it() {
        // On a replay the clock is the playhead: it wraps at the end of the
        // recording and `←` seeks backwards through it. Samples from after the
        // playhead are not history, and a buffer that kept them would draw a
        // line that doubles back on itself.
        let mut history = History::new(10.0);
        for i in 0..=50 {
            history.push(A, i as f64, i as f64);
        }
        history.push(A, 5.0, 5.0);
        let points = history.points(A);
        assert_eq!(points.last(), Some(&(5.0, 5.0)));
        assert!(points.iter().all(|p| p.0 <= 5.0), "{points:?}");
        assert!(points.windows(2).all(|w| w[0].0 <= w[1].0), "still in time order: {points:?}");
    }

    #[test]
    fn a_channel_nobody_polls_any_more_is_released_rather_than_held_for_the_run() {
        // Deselecting a channel stops it being polled. Its last minute must not
        // sit in memory for the rest of the drive, and must not reappear as a
        // line of stale points if it is ever selected again.
        let mut history = History::new(10.0);
        history.push(A, 1.0, 1.0);
        history.push(B, 1.0, 1.0);
        for i in 2..30 {
            history.push(B, i as f64, 1.0);
            history.trim(i as f64);
        }
        assert!(history.points(A).is_empty(), "{:?}", history.points(A));
        assert!(!history.points(B).is_empty());
    }

    #[test]
    fn a_clock_that_has_stopped_does_not_grow_the_buffer() {
        // A paused replay redraws twenty times a second with the playhead where
        // it was, and every one of those redraws hands the same instant back.
        // Held that way for a few minutes it is thousands of points at one
        // time — an unbounded buffer reached without the clock moving at all,
        // which is the exact thing the window is here to prevent.
        let mut history = History::new(10.0);
        for _ in 0..1000 {
            history.push(A, 4.0, 42.0);
        }
        assert_eq!(history.points(A), vec![(4.0, 42.0)]);
    }

    #[test]
    fn a_channel_that_has_answered_nothing_has_no_points_rather_than_a_zero() {
        // Every channel looks like this on the first cycle of every run, and a
        // point invented to fill the gap would be a reading the car never gave.
        assert!(History::new(10.0).points(A).is_empty());
    }
}
