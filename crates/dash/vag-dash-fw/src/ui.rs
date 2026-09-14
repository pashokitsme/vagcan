//! The button and the LED's vocabulary.
//!
//! The button itself — debounce, long press, and the gate that makes one
//! press one press — is [`vag_dash_render::button`], re-exported here. It
//! lives there and not here for the reason the alarm machine does: this crate
//! cannot be built for the host, so nothing in it is tested by CI, and a state
//! machine over a clock is exactly the thing that wants a synthetic clock. What
//! stays here is what only the firmware has a use for.

pub use vag_dash_render::button::{Button, DEBOUNCE_MS, LONG_PRESS_MS, PRESS_GAP_MS, Press};

/// What the radio is doing, which is also what the LED is saying.
///
/// BLE is always on (owner, 2026-09-13/14): the board advertises from boot and
/// again after every disconnect, with no button to press. There is no
/// access-control story in being visible; the board's guard is what bounds
/// what a stranger in range can ask of the car (`todo/dash/16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Visibility {
	/// Not advertising: before the radio is up, or while advertising could not
	/// be started and is being retried.
	Dark = 0,
	/// Advertising, waiting for a client.
	Advertising = 1,
	/// A client is connected.
	Connected = 2,
}
