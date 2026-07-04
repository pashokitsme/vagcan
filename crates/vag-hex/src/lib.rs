//! `vag-hex` — transport for the physical clone HEX cable (VAG25.3).
//!
//! Drives the cable's own USB/serial protocol directly so `vagcan` can talk to
//! the car with no VCDS and no loader. Implements [`vag_transport::IsoTpTransport`],
//! the seam the `vag-protocol` UDS client already consumes, so the existing
//! (tested) UDS/ISO-TP stack works unchanged once the cable protocol is pinned.
//!
//! **Status:** the wire framing is recovered and implemented. [`frame`] carries
//! the capture-confirmed flat `S/M` frame (`frame_encode`/`frame_decode`/
//! `take_frame`) — see `research/vag-hex-framing.md`. Still pending hardware/
//! capture: [`usb`] (FTDI D2XX byte pipe), [`init`] (open-time handshake), and
//! the encrypted diagnostic UDS transport (`frame::encode`/`decode` — needs the
//! per-channel link keystream schedule, reversed in research but not yet ported;
//! see `research/clb-crack/link_cipher.py`). The module seams, public API, and
//! error mapping are final.

pub mod error;
pub mod frame;
pub mod init;
pub mod transport;
pub mod usb;

pub use error::HexError;
pub use frame::{Frame, MARKER_CABLE, MARKER_HOST};
pub use init::CableIdentity;
pub use transport::{HexCable, HexConfig};
pub use usb::Backend;
