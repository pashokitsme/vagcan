//! Wire the cable into the existing transport seam.
//!
//! `HexCable` implements [`vag_transport::IsoTpTransport`], the same trait the
//! `vag-protocol` UDS client already consumes, so the proven UDS/ISO-TP stack
//! runs unchanged once the diagnostic framing is pinned.
//!
//! **Status:** placeholder. The byte pipe is now the async [`crate::usb::Backend`]
//! driven by a dedicated thread; the sync `IsoTpTransport` glue lands with the
//! connection-actor task (`todo/usb-backend/02`, which owns the `CableActor`
//! and the async↔sync bridge). Until then `open` and the diagnostic
//! [`frame::encode`]/[`frame::decode`] seams return [`HexError::Unspecified`].

use std::time::Duration;

use vag_transport::{IsoTpTransport, TransportError};

use crate::error::HexError;
use crate::frame;
use crate::init::{self, CableIdentity};

/// Configuration for opening the cable.
#[derive(Debug, Clone)]
pub struct HexConfig {
    /// FTDI serial to open (`None` = first device found).
    pub serial: Option<String>,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
}

/// An open HEX cable, usable as an ISO-TP transport by the UDS client.
pub struct HexCable {
    identity: CableIdentity,
}

impl HexCable {
    /// Open the cable and run the init handshake.
    ///
    /// Gated: returns [`HexError::Unspecified`] until the connection actor wires
    /// the async backend to this sync transport seam.
    pub fn open(cfg: HexConfig) -> Result<Self, HexError> {
        let _ = cfg;
        let identity = init::handshake()?;
        Ok(HexCable { identity })
    }

    /// Cable identity recovered during init (for diagnostics / `doctor`).
    pub fn identity(&self) -> &CableIdentity {
        &self.identity
    }
}

impl IsoTpTransport for HexCable {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        let _wire = frame::encode(data).map_err(TransportError::from)?;
        Err(TransportError::Protocol(
            "hex cable transport not yet wired to the async backend".into(),
        ))
    }

    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let _ = timeout;
        let (pdu, _consumed) = frame::decode(&[]).map_err(TransportError::from)?;
        Ok(pdu)
    }
}
