//! One button, and what it means.
//!
//! The device has a single button because configuration moved to BLE, so the
//! button carries exactly two gestures and one of them is modal:
//!
//! | gesture | normally | while an alarm is showing |
//! |---|---|---|
//! | short press | next page | silence this episode |
//! | held 3 s | start advertising for configuration | same |
//!
//! Nothing here touches hardware. It is a state machine over a clock and a
//! level, so when this moves into `vag-dash` it can be tested against a
//! synthetic clock exactly like the alarm machine in `04`.

/// How long the button must be held to count as a long press. Longer than a
/// fumble, shorter than annoying.
pub const LONG_PRESS_MS: u64 = 3_000;

/// A level must hold this long before it is believed. Mechanical buttons
/// bounce for a millisecond or two; ten is comfortable and still invisible.
pub const DEBOUNCE_MS: u64 = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
	Short,
	Long,
}

/// Debounces a level and classifies presses.
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
	/// not then also produce a short one.
	consumed: bool,
}

impl Button {
	pub const fn new() -> Self {
		Self {
			pressed: false,
			candidate: false,
			candidate_since_ms: 0,
			pressed_since_ms: 0,
			consumed: false,
		}
	}

	/// Feed the raw level and the current time. Call it faster than the
	/// debounce interval; returns an event at most once per call.
	pub fn poll(&mut self, level_pressed: bool, now_ms: u64) -> Option<Press> {
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
			self.consumed = true;
			return Some(Press::Long);
		}
		None
	}
}

impl Default for Button {
	fn default() -> Self {
		Self::new()
	}
}

/// What the device is doing about being configurable, which is also what the
/// LED is saying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Visibility {
	/// Not advertising. There is nothing on the air to connect to, which is
	/// the entire security model: reaching this device requires standing next
	/// to it and pressing the button.
	Dark = 0,
	/// Advertising, waiting for a client, on a bounded window.
	Advertising = 1,
	/// A client is connected.
	Connected = 2,
}

/// How long advertising stays up with nobody connecting.
pub const ADVERTISE_WINDOW_SECS: u64 = 180;
