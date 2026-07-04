//! Byte-pipe backend: the raw read/write channel to the cable.
//!
//! The cable is an FTDI bridge reachable two ways — D2XX (raw bulk) or a virtual
//! COM port. Both reduce to "write bytes, read bytes", so the rest of the crate
//! depends only on the [`BytePipe`] trait and stays backend-agnostic. Which
//! concrete backend we ship is decided by the capture (VID/PID + D2XX vs VCP).

use std::time::Duration;

use crate::error::HexError;

/// A bidirectional byte channel to the cable. Backends (D2XX, serial, or a
/// captured-bytes replay used in tests) implement this.
pub trait BytePipe {
    /// Write all bytes to the cable.
    fn write(&mut self, bytes: &[u8]) -> Result<(), HexError>;
    /// Read up to `buf.len()` bytes, blocking up to `timeout`. Returns count read.
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> Result<usize, HexError>;
}

/// Which physical backend to open.
#[derive(Debug, Clone)]
pub enum Backend {
    /// FTDI D2XX by serial string (`None` = first device found).
    D2xx { serial: Option<String> },
    /// Virtual COM / serial device path, e.g. `/dev/tty.usbserial-XXXX`.
    Serial { path: String },
}

/// Open the byte pipe for the chosen backend.
///
/// Stub: the concrete D2XX / serial backend is wired once the capture pins the
/// enumeration details. Returns [`HexError::Unspecified`] until then.
pub fn open(_backend: &Backend) -> Result<Box<dyn BytePipe>, HexError> {
    Err(HexError::Unspecified("usb backend (needs capture: D2XX vs VCP, VID/PID)"))
}
