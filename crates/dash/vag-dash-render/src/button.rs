//! One button, and what a press of it is.
//!
//! The device has a single button because configuration moved to BLE, so the
//! button carries exactly two gestures and one of them is modal:
//!
//! | gesture | normally | while an alarm is showing |
//! |---|---|---|
//! | short press | next page | silence this episode |
//! | held 3 s | nothing (it opened BLE until BLE became always on, 2026-09-14) | same |
//!
//! Nothing here touches hardware. It is a state machine over a clock and a
//! level, and it lives in this crate for the same reason [`alarm`](crate::alarm)
//! does: the firmware is not a workspace member, so anything that lives there
//! is untested by CI, and a machine you can hand a synthetic clock is a machine
//! you can test.
//!
//! Two ways in, one way out. The GPIO feeds a *level* through [`Button::poll`]
//! and gets debouncing and long-press detection; a remote — `dashsim` typing
//! `BTN S` over the USB line — feeds an already-classified [`Press`] through
//! [`Button::remote`]. Both go through the same gate on the way out: **a press
//! closer than [`PRESS_GAP_MS`] to the last accepted one is not a press.** That
//! gate is what the 2026-09-13 defect was missing — a held space bar in a
//! terminal auto-repeats, the remote path took every repeat at face value, and
//! the page turned a dozen times for one keystroke.

/// How long the button must be held to count as a long press. Longer than a
/// fumble, shorter than annoying.
pub const LONG_PRESS_MS: u64 = 3_000;

/// A level must hold this long before it is believed. Mechanical buttons
/// bounce for a millisecond or two; ten is comfortable and still invisible.
pub const DEBOUNCE_MS: u64 = 10;

/// Two presses closer than this are one press.
///
/// Faster than a person turns pages on purpose, slower than anything that is
/// not a person: a terminal's key repeat (macOS defaults to one every 83 ms
/// after a 500 ms delay), or a contact that chatters on release for longer
/// than [`DEBOUNCE_MS`]. Measured from the last press that was *accepted*, so
/// a key held down still turns a page every quarter second rather than never
/// — a gate that slid with every dropped repeat would lock out a person
/// tapping at a steady rhythm, and "the button stopped working" is the worse
/// failure.
pub const PRESS_GAP_MS: u64 = 250;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
	Short,
	Long,
}

/// Debounces a level, classifies presses, and rate-limits what it lets out.
///
/// A long press fires **while the button is still held**, at the moment the
/// threshold is crossed, rather than on release. Without that there is no way
/// to tell a long press from a stuck button until it ends, and the person
/// holding it gets no feedback that it worked.
pub struct Button {
	/// The level we currently believe, `true` meaning pressed.
	pressed: bool,
	/// The level we are waiting to believe, and since when.
	candidate: bool,
	candidate_since_ms: u64,
	/// When the believed press started.
	pressed_since_ms: u64,
	/// Set once a press has already produced a long event, so the release does
	/// not then also produce a short one. Set from [`Button::poll`] and only
	/// when the gate let the Long out — a hold whose Long the gate dropped is
	/// still a hold, and it has to be able to try again on the next poll.
	consumed: bool,
	/// When the last press of either kind, from either source, was let out.
	last_accepted_ms: Option<u64>,
}

impl Button {
	pub const fn new() -> Self {
		Self {
			pressed: false,
			candidate: false,
			candidate_since_ms: 0,
			pressed_since_ms: 0,
			consumed: false,
			last_accepted_ms: None,
		}
	}

	/// Feed the raw level and the current time. Call it faster than the
	/// debounce interval; returns an event at most once per call.
	pub fn poll(&mut self, level_pressed: bool, now_ms: u64) -> Option<Press> {
		let press = self.classify(level_pressed, now_ms)?;
		let out = self.accept(press, now_ms);
		// A hold is spent by a Long that *came out*, not by one the gate ate.
		// Marking it at the threshold instead lost the whole hold to whatever
		// happened to pass through the gate in the quarter second before it,
		// and the release then produced no Short either. The Long is exempted
		// from the gate on this side rather than inside `accept`, because the
		// gate is what makes a burst of remote `BTN L` one long press.
		if out == Some(Press::Long) {
			self.consumed = true;
		}
		out
	}

	/// Feed a press somebody else already classified — the simulator's
	/// `BTN S` / `BTN L`. It is not debounced, because there is no level to
	/// debounce; it is gated, because a keyboard repeats.
	pub fn remote(&mut self, press: Press, now_ms: u64) -> Option<Press> {
		self.accept(press, now_ms)
	}

	/// The debounce and the long-press threshold, on a level. What comes out
	/// has not been through the gate yet.
	fn classify(&mut self, level_pressed: bool, now_ms: u64) -> Option<Press> {
		if level_pressed != self.candidate {
			self.candidate = level_pressed;
			self.candidate_since_ms = now_ms;
		} else if level_pressed != self.pressed && now_ms.saturating_sub(self.candidate_since_ms) >= DEBOUNCE_MS {
			self.pressed = level_pressed;
			if self.pressed {
				self.pressed_since_ms = now_ms;
				self.consumed = false;
			} else if !self.consumed {
				return Some(Press::Short);
			}
		}

		if self.pressed && !self.consumed && now_ms.saturating_sub(self.pressed_since_ms) >= LONG_PRESS_MS {
			return Some(Press::Long);
		}
		None
	}

	/// The gate: one press per [`PRESS_GAP_MS`], whatever its kind and
	/// wherever it came from.
	fn accept(&mut self, press: Press, now_ms: u64) -> Option<Press> {
		if let Some(last) = self.last_accepted_ms {
			if now_ms.saturating_sub(last) < PRESS_GAP_MS {
				return None;
			}
		}
		self.last_accepted_ms = Some(now_ms);
		Some(press)
	}
}

impl Default for Button {
	fn default() -> Self {
		Self::new()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::vec::Vec;

	/// Hold the level for `ms`, polling every millisecond, and collect what
	/// comes out. `from_ms` is when the hold starts.
	fn hold(button: &mut Button, level: bool, from_ms: u64, ms: u64) -> Vec<Press> {
		let mut out = Vec::new();
		for t in from_ms..from_ms + ms {
			if let Some(p) = button.poll(level, t) {
				out.push(p);
			}
		}
		out
	}

	/// One clean tap: pressed for `ms`, then released and left alone long
	/// enough for the release to be believed. Returns what came out and when
	/// the tap ended.
	fn tap(button: &mut Button, from_ms: u64, ms: u64) -> (Vec<Press>, u64) {
		let mut out = hold(button, true, from_ms, ms);
		let released = hold(button, false, from_ms + ms, DEBOUNCE_MS + 1);
		out.extend(released);
		(out, from_ms + ms + DEBOUNCE_MS + 1)
	}

	#[test]
	fn a_level_shorter_than_the_debounce_is_not_a_press() {
		let mut button = Button::new();
		let (out, _) = tap(&mut button, 0, DEBOUNCE_MS - 1);
		assert!(out.is_empty(), "a bounce is not a press, got {out:?}");
	}

	#[test]
	fn a_tap_is_one_short_press_on_release() {
		let mut button = Button::new();
		let pressed = hold(&mut button, true, 0, 100);
		assert!(pressed.is_empty(), "a short press is reported on release, not while held");
		let released = hold(&mut button, false, 100, DEBOUNCE_MS + 1);
		assert_eq!(released.as_slice(), &[Press::Short]);
	}

	#[test]
	fn a_hold_is_one_long_press_while_still_held_and_nothing_on_release() {
		let mut button = Button::new();
		let held = hold(&mut button, true, 0, LONG_PRESS_MS + DEBOUNCE_MS + 100);
		assert_eq!(held.as_slice(), &[Press::Long], "fires at the threshold, once");
		let released = hold(&mut button, false, LONG_PRESS_MS + DEBOUNCE_MS + 100, DEBOUNCE_MS + 1);
		assert!(released.is_empty(), "the release of a long press is not a short press");
	}

	#[test]
	fn a_long_press_the_gate_drops_fires_once_the_gap_has_passed() {
		// The gate is shared, so something else coming out of it can land close
		// enough to the long-press threshold to swallow the Long. That must cost
		// the press its timing, not the press: the finger is still on the button.
		let mut button = Button::new();
		// The hold starts at 0 and is believed at DEBOUNCE_MS, so the threshold
		// falls at 3010.
		let early = hold(&mut button, true, 0, 2_900);
		assert!(early.is_empty(), "nothing before the threshold, got {early:?}");
		// A page turn from the simulator, 110 ms before it.
		assert_eq!(button.remote(Press::Short, 2_900), Some(Press::Short));
		// The threshold falls inside the gap, and the Long is dropped there...
		let gated = hold(&mut button, true, 2_901, PRESS_GAP_MS - 1);
		assert!(gated.is_empty(), "inside the gap nothing comes out, got {gated:?}");
		// ...and comes out at the first poll past the gap, exactly once, with the
		// button never released.
		let late = hold(&mut button, true, 2_900 + PRESS_GAP_MS, 500);
		assert_eq!(late.as_slice(), &[Press::Long], "the hold is still a hold");
		// And the release of a long press is still not a short press.
		let released = hold(&mut button, false, 3_400 + PRESS_GAP_MS, DEBOUNCE_MS + 1);
		assert!(released.is_empty(), "got {released:?}");
	}

	#[test]
	fn a_burst_of_remote_presses_inside_the_gap_is_one_press() {
		// What a held space bar looks like through `dashsim` on a terminal that
		// does not report key repeat: a press every 83 ms for as long as it is
		// held. Inside one gap that is one press.
		let mut button = Button::new();
		let mut accepted = 0;
		for i in 0..3u64 {
			if button.remote(Press::Short, i * 83).is_some() {
				accepted += 1;
			}
		}
		assert_eq!(accepted, 1, "three repeats in 166 ms are one press");
	}

	#[test]
	fn a_held_key_turns_a_page_every_gap_and_not_never() {
		// Measured from the last *accepted* press: a repeat that lands after
		// the gap is a press again. Eight repeats at 83 ms — 0 to 581 ms — are
		// two presses: the first, and the one at 332 (the first past 250);
		// 581 is 249 after that and still inside.
		let mut button = Button::new();
		let mut accepted = Vec::<u64>::new();
		for i in 0..8u64 {
			let t = i * 83;
			if button.remote(Press::Short, t).is_some() {
				accepted.push(t);
			}
		}
		assert_eq!(accepted.as_slice(), &[0, 332]);
	}

	#[test]
	fn remote_presses_spaced_beyond_the_gap_are_each_a_press() {
		let mut button = Button::new();
		let mut accepted = 0;
		for i in 0..5u64 {
			if button.remote(Press::Short, i * PRESS_GAP_MS).is_some() {
				accepted += 1;
			}
		}
		assert_eq!(accepted, 5, "the gap is exclusive: exactly PRESS_GAP_MS apart is two presses");
	}

	#[test]
	fn a_remote_long_press_stays_a_long_press() {
		let mut button = Button::new();
		assert_eq!(button.remote(Press::Long, 0), Some(Press::Long));
	}

	#[test]
	fn a_burst_of_remote_long_presses_is_one_long_press() {
		// `l` auto-repeats like space does, and a long press toggles
		// advertising: a burst would toggle it back off.
		let mut button = Button::new();
		let mut accepted = 0;
		for i in 0..4u64 {
			if button.remote(Press::Long, i * 83).is_some() {
				accepted += 1;
			}
		}
		assert_eq!(accepted, 1);
	}

	#[test]
	fn the_gate_is_shared_between_the_gpio_and_the_remote() {
		// One button, two ways to press it, one rule. A remote press right
		// after a physical one is the same double press and is dropped; one a
		// gap later is not.
		let mut button = Button::new();
		let (out, ended) = tap(&mut button, 0, 50);
		assert_eq!(out.as_slice(), &[Press::Short]);
		assert_eq!(button.remote(Press::Short, ended + 10), None);
		assert_eq!(button.remote(Press::Short, ended + PRESS_GAP_MS), Some(Press::Short));
	}

	#[test]
	fn physical_taps_spaced_beyond_the_gap_are_each_a_press() {
		let mut button = Button::new();
		let mut presses = 0;
		let mut t = 0;
		for _ in 0..4 {
			let (out, ended) = tap(&mut button, t, 50);
			presses += out.len();
			t = ended + PRESS_GAP_MS;
		}
		assert_eq!(presses, 4);
	}

	#[test]
	fn a_contact_that_chatters_on_release_longer_than_the_debounce_is_still_one_press() {
		// A tired switch: released, makes again for 20 ms, released for good.
		// The debounce believes both edges; the gap does not believe the
		// second press.
		let mut button = Button::new();
		let (first, ended) = tap(&mut button, 0, 50);
		assert_eq!(first.as_slice(), &[Press::Short]);
		let (chatter, _) = tap(&mut button, ended, 20);
		assert!(chatter.is_empty(), "got {chatter:?}");
	}
}
