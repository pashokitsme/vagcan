//! What a driver asks of the panel, whichever input asked (`todo/dash/19`, "Input backends").
//!
//! Two kinds of input turn the pages in the car, in any mix, or none: buttons on the pins the
//! owner's `[[button]]`s name, and the cruise lever ([`stalk`](crate::stalk)). On the bench
//! `dashsim` presses over the cable too. Each is a small machine that turns what it reads into
//! a [`Command`]; [`Screen::command`](crate::screen::Screen::command) takes the command and
//! never learns where it came from, so what a press does is decided in one place whatever was
//! pressed.
//!
//! | input | [`Command::Next`] | [`Command::Previous`] | [`Command::Stopwatch`] |
//! |---|---|---|---|
//! | a `[[button]]` | `action = "next"` | `action = "previous"` | `action = "stopwatch"` |
//! | the lever | its `next` state | its `previous` state | its `measure` state |
//! | `dashsim` | `BTN S` | — | — |
//!
//! A pin button has no long press ([`PinButton`]); `dashsim`'s long press asks for nothing
//! ([`remote`]).
//!
//! **The board's own BOOT and RESET buttons are not inputs** (owner, 2026-09-27: they are
//! technical, nothing more). BOOT is `GPIO9`, a strapping pin the ROM reads at reset to
//! choose download mode; the dash image leaves it alone.

use crate::button::{Button, Press};

/// One thing a driver can ask of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
	/// The next page, wrapping.
	Next,
	/// The previous page, wrapping.
	Previous,
	/// The stopwatch page on, or off.
	Stopwatch,
}

impl Command {
	/// Every command, in the order `dash.toml`'s reference lists their names.
	pub const ALL: [Command; 3] = [Command::Next, Command::Previous, Command::Stopwatch];

	/// Its name: a `[[button]]`'s `action` in `dash.toml`, and the word the board's log uses.
	pub const fn name(self) -> &'static str {
		match self {
			Command::Next => "next",
			Command::Previous => "previous",
			Command::Stopwatch => "stopwatch",
		}
	}

	/// The command a `[[button]]`'s `action` names: exactly, lowercase, as [`Command::name`]
	/// spells it.
	pub fn from_name(name: &str) -> Option<Command> {
		Command::ALL.into_iter().find(|command| command.name() == name)
	}
}

/// The pins a `[[button]]` may name. The ESP32-C3 SuperMini breaks out GPIO 0–10, 20 and 21;
/// with this board's wiring (`todo/dash/15-enclosure.md` §3) the OLED holds 0, 7, 10, 20 and
/// 21, the CAN transceiver 1 and 6, the LED 8, the BOOT button 9, and 2 is a strapping pin
/// (8 and 9 are too). GPIO 3, 4 and 5 lost their jobs there (the RS wire, the rail divider,
/// the wake button) and are free. A property of the board and its wiring, not of any car.
pub const BUTTON_PINS: [u8; 3] = [3, 4, 5];

/// At most one button per free pin.
pub const MAX_BUTTONS: usize = BUTTON_PINS.len();

/// Whether a `[[button]]` may be on `pin` — one of [`BUTTON_PINS`]. `const`, so the firmware
/// refuses at compile time a plan that names another.
pub const fn is_button_pin(pin: u8) -> bool {
	let mut i = 0;
	while i < BUTTON_PINS.len() {
		if BUTTON_PINS[i] == pin {
			return true;
		}
		i += 1;
	}
	false
}

/// What a press from `dashsim`, over the cable, asks for: its short press (`BTN S`) is
/// [`Command::Next`], as it always was; its long press (`BTN L`) asks for nothing, and the
/// firmware only says so. Also what `vagcan dev recording dash --press` gives.
pub const fn remote(press: Press) -> Option<Command> {
	match press {
		Press::Short => Some(Command::Next),
		Press::Long => None,
	}
}

/// A button on a pin: one [`Command`], its `[[button]]`'s `action`.
///
/// Wired to GND with the pin's pull-up, so the pin reads low while it is pressed. Debounced by
/// the one button machine there is ([`Button`]): [`DEBOUNCE_MS`](crate::button::DEBOUNCE_MS),
/// and [`PRESS_GAP_MS`](crate::button::PRESS_GAP_MS) between two presses. **It has no long
/// press**: a press is its action, let out when the button is let go — or at
/// [`LONG_PRESS_MS`](crate::button::LONG_PRESS_MS) if it is still held then, so a hold is one
/// press as well, and never repeats.
pub struct PinButton {
	button: Button,
	action: Command,
}

impl PinButton {
	pub const fn new(action: Command) -> Self {
		PinButton {
			button: Button::new(),
			action,
		}
	}

	/// What a press of it asks for.
	pub fn action(&self) -> Command {
		self.action
	}

	/// Feed the level — `true` while pressed — and the time. Call it faster than the debounce
	/// interval; a command comes out at most once per call.
	pub fn poll(&mut self, pressed: bool, now_ms: u64) -> Option<Command> {
		// A long press is a press like any other: the machine tells the two apart, and a pin
		// button has one action for both.
		self.button.poll(pressed, now_ms).map(|_| self.action)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::button::{DEBOUNCE_MS, LONG_PRESS_MS, PRESS_GAP_MS};
	use std::vec::Vec;

	/// Hold the level for `ms`, polling every millisecond from `from_ms`; what came out.
	fn hold(button: &mut PinButton, pressed: bool, from_ms: u64, ms: u64) -> Vec<Command> {
		(from_ms..from_ms + ms).filter_map(|t| button.poll(pressed, t)).collect()
	}

	#[test]
	fn each_name_is_the_dash_toml_word_and_reads_back_exactly() {
		assert_eq!(Command::ALL.map(Command::name), ["next", "previous", "stopwatch"]);
		for command in Command::ALL {
			assert_eq!(Command::from_name(command.name()), Some(command));
		}
		for wrong in ["Next", "NEXT", " next", "next ", "", "prev", "measure", "limit"] {
			assert_eq!(Command::from_name(wrong), None, "{wrong:?}");
		}
	}

	#[test]
	fn a_button_goes_on_gpio_3_4_or_5_and_nowhere_else() {
		let allowed: Vec<u8> = (0..=u8::MAX).filter(|&pin| is_button_pin(pin)).collect();
		assert_eq!(allowed, BUTTON_PINS);
		assert_eq!(MAX_BUTTONS, 3);
	}

	#[test]
	fn dashsims_short_press_is_next_and_its_long_press_asks_for_nothing() {
		assert_eq!(remote(Press::Short), Some(Command::Next));
		assert_eq!(remote(Press::Long), None);
	}

	#[test]
	fn a_clean_press_is_its_action_once_on_release() {
		for action in Command::ALL {
			let mut button = PinButton::new(action);
			assert_eq!(button.action(), action);
			assert!(hold(&mut button, false, 0, 50).is_empty(), "at rest");
			assert!(hold(&mut button, true, 50, 200).is_empty(), "nothing while held");
			assert_eq!(hold(&mut button, false, 250, DEBOUNCE_MS + 1), [action], "on release");
			assert!(hold(&mut button, false, 261, 1_000).is_empty(), "and once");
		}
	}

	#[test]
	fn a_bounce_is_not_a_press_and_a_bouncing_press_is_one() {
		let mut button = PinButton::new(Command::Previous);
		// Shorter than the debounce: a spike on the line.
		assert!(hold(&mut button, true, 0, DEBOUNCE_MS - 1).is_empty());
		assert!(hold(&mut button, false, DEBOUNCE_MS - 1, 100).is_empty(), "no press came of it");
		// A contact that chatters for 6 ms as it closes and again as it opens.
		let mut out = Vec::new();
		for t in 200..206 {
			out.extend(button.poll(t % 2 == 0, t));
		}
		out.extend(hold(&mut button, true, 206, 150));
		for t in 356..362 {
			out.extend(button.poll(t % 2 == 1, t));
		}
		out.extend(hold(&mut button, false, 362, 100));
		assert_eq!(out, [Command::Previous]);
	}

	#[test]
	fn holding_is_one_press_and_never_repeats() {
		let mut button = PinButton::new(Command::Next);
		let held = hold(&mut button, true, 0, 10_000);
		assert_eq!(held, [Command::Next], "one, at the long-press threshold, while still held");
		assert!(hold(&mut button, false, 10_000, 100).is_empty(), "the release is not a second press");
		// Held for less than the threshold, it comes out on release instead.
		let mut button = PinButton::new(Command::Next);
		assert!(hold(&mut button, true, 0, LONG_PRESS_MS - 100).is_empty());
		assert_eq!(hold(&mut button, false, LONG_PRESS_MS - 100, DEBOUNCE_MS + 1), [Command::Next]);
	}

	#[test]
	fn taps_a_gap_apart_are_each_a_press_and_two_buttons_do_not_gate_each_other() {
		let (mut a, mut b) = (PinButton::new(Command::Next), PinButton::new(Command::Stopwatch));
		let mut out = Vec::new();
		let mut t = 0;
		for _ in 0..3 {
			out.extend(hold(&mut a, true, t, 50));
			out.extend(hold(&mut a, false, t + 50, DEBOUNCE_MS + 1));
			// The other button, pressed at the same moments: its own machine, its own gate.
			out.extend(hold(&mut b, true, t, 50));
			out.extend(hold(&mut b, false, t + 50, DEBOUNCE_MS + 1));
			t += 50 + DEBOUNCE_MS + 1 + PRESS_GAP_MS;
		}
		assert_eq!(out, [Command::Next, Command::Stopwatch].repeat(3));
	}
}
