//! The button and the LED's vocabulary.
//!
//! The button itself — debounce, long press, and the gate that makes one
//! press one press — is [`vag_dash_render::button`], re-exported here. It
//! lives there and not here for the reason the alarm machine does: this crate
//! cannot be built for the host, so nothing in it is tested by CI, and a state
//! machine over a clock is exactly the thing that wants a synthetic clock. What
//! stays here is what only the firmware has a use for.

pub use vag_dash_render::button::{Button, DEBOUNCE_MS, LONG_PRESS_MS, PRESS_GAP_MS, Press};

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
