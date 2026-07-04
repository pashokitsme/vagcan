//! Open-time handshake: the fixed exchange VCDS does right after opening the
//! cable, before any car traffic (baud/latency setup, firmware/version query,
//! the cable "hello"). Exact bytes and expected replies come **from the capture**
//! (the trace must start before VCDS launches to record this).

use crate::error::HexError;
use crate::usb::BytePipe;

/// Identity recovered from the cable during init — surfaced by `vagcan doctor`.
#[derive(Debug, Clone, Default)]
pub struct CableIdentity {
    /// Firmware / version string, if the handshake exposes one.
    pub firmware: Option<String>,
    /// Raw identity bytes as returned, for diagnostics.
    pub raw: Vec<u8>,
}

/// Run the handshake over an open byte pipe, driving the cable to "ready".
///
/// Stub: the handshake sequence is defined once the capture pins it. Returns
/// [`HexError::Unspecified`] until then.
pub fn handshake(_pipe: &mut dyn BytePipe) -> Result<CableIdentity, HexError> {
    Err(HexError::Unspecified("init handshake (needs capture: open-time exchange)"))
}
