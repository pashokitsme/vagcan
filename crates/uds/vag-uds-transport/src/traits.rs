use crate::{CanFrame, TransportError};
use alloc::vec::Vec;
use core::time::Duration;

/// `Send` where `Send` means something, and nothing where it does not.
///
/// The host runs these transports under tokio, which may move a task between
/// worker threads, so a backend there has to be [`Send`]. The board runs them
/// under embassy on one core, and esp-hal's async drivers are deliberately
/// **not** `Send` — an async peripheral is tied to the core whose interrupt
/// handler wakes it, and the type system is where that is enforced.
///
/// So the bound is a feature of the platform, not of the protocol. Under
/// `std` this is exactly `Send`; without it, it is satisfied by everything.
#[cfg(feature = "std")]
pub trait MaybeSend: Send {}
#[cfg(feature = "std")]
impl<T: Send + ?Sized> MaybeSend for T {}

#[cfg(not(feature = "std"))]
pub trait MaybeSend {}
#[cfg(not(feature = "std"))]
impl<T: ?Sized> MaybeSend for T {}

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
/// One implementor owns the physical link outright — `IsoTpCan` over a
/// `CanBackend` — and is addressed to one unit at a time; talking to another
/// unit is unwrapping the backend and wrapping it again (see
/// `vag-cli-core/src/plan.rs::read_batch`). There is no actor and no shared
/// handle: the one that existed served the HEX clone and went with it.
/// Consumers (uds-async) use STATIC dispatch (`T: AsyncIsoTpTransport`) — no
/// `dyn`, no `async_trait`.
// Static dispatch only, so callers that spawn add their own `Send` bounds on
// the returned futures; the auto "async fn in public trait" lint is not useful
// at this seam.
#[allow(async_fn_in_trait)]
pub trait AsyncIsoTpTransport: MaybeSend {
	/// Send one whole ISO-TP PDU.
	async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError>;
	/// Receive one whole ISO-TP PDU, waiting at most `timeout`.
	async fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError>;
}
