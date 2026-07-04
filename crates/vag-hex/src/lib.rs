//! `vag-hex` — transport for the physical clone HEX cable (VAG25.3).
//!
//! Drives the cable's own USB/serial protocol directly so `vagcan` can talk to
//! the car with no VCDS and no loader. Implements [`vag_transport::IsoTpTransport`],
//! the seam the `vag-protocol` UDS client already consumes, so the existing
//! (tested) UDS/ISO-TP stack works unchanged once the cable protocol is pinned.
//!
//! **Status:** scaffold. The two layers carrying the reversed wire format —
//! [`frame`] (cable envelope) and [`init`] (open-time handshake) — are stubs
//! returning [`error::HexError::Unspecified`] until the USB capture defines them
//! (see `research/vag-hex-capture-guide.md`). The module seams, public API, and
//! error mapping are final; only the byte-level bodies are pending.

pub mod error;
pub mod frame;
pub mod init;
pub mod transport;
pub mod usb;

pub use error::HexError;
pub use init::CableIdentity;
pub use transport::{HexCable, HexConfig};
pub use usb::Backend;
