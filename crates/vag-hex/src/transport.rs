//! Wire the cable into the existing transport seam.
//!
//! `HexCable` implements [`vag_transport::IsoTpTransport`], the same trait the
//! `vag-protocol` UDS client already consumes. So the proven UDS/ISO-TP stack
//! runs unchanged the moment `frame`/`init` are pinned from the capture — this
//! layer only maps cable frames to/from whole ISO-TP PDUs.

use std::time::Duration;

use vag_transport::{IsoTpTransport, TransportError};

use crate::error::HexError;
use crate::frame;
use crate::init::{self, CableIdentity};
use crate::usb::{self, Backend, BytePipe};

/// Configuration for opening the cable.
#[derive(Debug, Clone)]
pub struct HexConfig {
    pub backend: Backend,
    pub read_timeout: Duration,
    pub write_timeout: Duration,
}

/// An open HEX cable, usable as an ISO-TP transport by the UDS client.
pub struct HexCable {
    pipe: Box<dyn BytePipe>,
    identity: CableIdentity,
    read_timeout: Duration,
}

impl HexCable {
    /// Open the cable and run the init handshake.
    pub fn open(cfg: HexConfig) -> Result<Self, HexError> {
        let mut pipe = usb::open(&cfg.backend)?;
        let identity = init::handshake(pipe.as_mut())?;
        Ok(HexCable {
            pipe,
            identity,
            read_timeout: cfg.read_timeout,
        })
    }

    /// Cable identity recovered during init (for diagnostics / `doctor`).
    pub fn identity(&self) -> &CableIdentity {
        &self.identity
    }
}

impl IsoTpTransport for HexCable {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        let wire = frame::encode(data).map_err(TransportError::from)?;
        self.pipe.write(&wire).map_err(TransportError::from)
    }

    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        // Real impl accumulates bytes until frame::decode yields a full PDU,
        // bounded by `timeout`. Placeholder reads once, then decodes.
        let _ = timeout;
        let mut buf = [0u8; 512];
        let n = self
            .pipe
            .read(&mut buf, self.read_timeout)
            .map_err(TransportError::from)?;
        let (pdu, _consumed) = frame::decode(&buf[..n]).map_err(TransportError::from)?;
        Ok(pdu)
    }
}
