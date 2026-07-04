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

/// Async ISO-TP channel: the seam the async UDS client rides.
///
/// In the connection-actor model, a `CableActor` owns the physical byte pipe;
/// each cheaply-cloned handle (e.g. a per-ECU `CableHandle` channel) implements
/// this trait by forwarding PDUs to the actor over a bounded mpsc and awaiting
/// the reply on a oneshot. Consumers (uds-async) use STATIC dispatch
/// (`T: AsyncIsoTpTransport`) — no `dyn`, no `async_trait`.
// Static dispatch only, so callers that spawn add their own `Send` bounds on
// the returned futures; the auto "async fn in public trait" lint is not useful
// at this seam.
#[allow(async_fn_in_trait)]
pub trait AsyncIsoTpTransport: Send {
    /// Send one whole ISO-TP PDU.
    async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError>;
    /// Receive one whole ISO-TP PDU, waiting at most `timeout`.
    async fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError>;
}
