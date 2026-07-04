//! Open-time handshake: the fixed exchange VCDS does right after opening the
//! cable, before any car traffic (baud/latency setup, firmware/version query,
//! the cable "hello"). Exact bytes and expected replies come **from the capture**
//! (the trace must start before VCDS launches to record this).

use crate::error::HexError;

/// Identity recovered from the cable during init — surfaced by `vagcan doctor`.
#[derive(Debug, Clone, Default)]
pub struct CableIdentity {
    /// Firmware / version string, if the handshake exposes one.
    pub firmware: Option<String>,
    /// Raw identity bytes as returned, for diagnostics.
    pub raw: Vec<u8>,
}

/// Run the open-time handshake, driving the cable to "ready".
///
/// Stub: the handshake sequence is defined once the capture pins it, and will
/// run over the async [`crate::usb::Backend`] (a later task). Returns
/// [`HexError::Unspecified`] until then.
pub fn handshake() -> Result<CableIdentity, HexError> {
    Err(HexError::Unspecified("init handshake (needs capture: open-time exchange)"))
}
