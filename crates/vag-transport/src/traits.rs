use std::time::Duration;
use crate::{CanFrame, TransportError};

/// Raw CAN frame I/O. Implemented by real adapters and by mocks.
pub trait RawCanTransport {
    fn send_frame(&mut self, frame: &CanFrame) -> Result<(), TransportError>;
    fn recv_frame(&mut self, timeout: Duration) -> Result<CanFrame, TransportError>;
}

/// A single ISO-TP channel bound (at construction) to one ECU's tx/rx addressing.
/// Sends/receives whole ISO-TP PDUs; the UDS client depends only on this.
pub trait IsoTpTransport {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError>;
    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError>;
}
