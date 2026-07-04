//! `vag-hex` — transport for the physical clone HEX cable (VAG25.3).
//!
//! Drives the cable's own USB/serial protocol directly so `vagcan` can talk to
//! the car with no VCDS and no loader. Implements [`vag_transport::IsoTpTransport`],
//! the seam the `vag-protocol` UDS client already consumes, so the existing
//! (tested) UDS/ISO-TP stack works unchanged once the cable protocol is pinned.
//!
//! **Status:** the wire framing and the byte-pipe backend are implemented.
//! [`frame`] carries the capture-confirmed flat `S/M` frame (`frame_encode`/
//! `frame_decode`/`take_frame`) — see `research/vag-hex-framing.md`. [`usb`]
//! carries the async [`Backend`] trait and the [`D2xxBackend`] (blocking FTDI
//! D2XX handle on a dedicated thread, bridged to async by channels). Still
//! pending hardware/capture: [`init`] (open-time handshake), the async↔sync
//! actor glue in [`transport`], and the encrypted diagnostic UDS transport
//! (`frame::encode`/`decode` — needs the per-channel link keystream schedule,
//! reversed in research but not yet ported; see
//! `research/clb-crack/link_cipher.py`).

// Without the `d2xx` backend feature (and outside tests), the dedicated-thread
// byte-pipe bridge has no caller and is deliberately dormant scaffolding — do
// not warn on it in that configuration.
#![cfg_attr(not(any(feature = "d2xx", test)), allow(dead_code))]

pub mod error;
pub mod frame;
pub mod init;
pub mod transport;
pub mod usb;

pub use error::HexError;
pub use frame::{Frame, MARKER_CABLE, MARKER_HOST};
pub use init::CableIdentity;
pub use transport::{HexCable, HexConfig};
pub use usb::{Backend, CableInfo, D2xxBackend, FTDI_VID, list_cables};
