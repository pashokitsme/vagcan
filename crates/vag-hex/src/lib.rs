//! `vag-hex` — transport for the physical clone HEX cable (VAG25.3).
//!
//! Drives the cable's own USB/serial protocol directly so `vagcan` can talk to
//! the car with no VCDS and no loader. The connection actor in [`actor`] owns
//! the byte pipe and will (once the link cipher is ported) expose
//! [`vag_transport::AsyncIsoTpTransport`], the seam the async UDS client rides.
//!
//! **Status:** the wire framing, the byte-pipe backend, and the connection
//! actor are implemented. [`frame`] carries the capture-confirmed flat `S/M`
//! frame (`frame_encode`/`frame_decode`/`take_frame`) — see
//! `research/vag-hex-framing.md`. [`usb`] carries the async [`Backend`] trait
//! and the [`D2xxBackend`] (blocking FTDI D2XX handle on a dedicated thread,
//! bridged to async by channels). [`actor`] carries the [`CableActor`] +
//! cheap-clone [`CableHandle`]: one tokio task owns the byte pipe and
//! multiplexes N concurrent plaintext frame requests over the single link
//! ([`spawn`]). [`init`] drives the PLAINTEXT open handshake ([`handshake`] →
//! [`CableIdentity`]: `0x02` probe + `0x04` identify → "ROSSTECH" + version);
//! it stops at plaintext identify (the `0xb0..0xb6` auth burst is out of scope,
//! see `research/SCOPE-BOUNDARY.md`). Still pending hardware/capture: the
//! encrypted diagnostic UDS transport (`frame::encode`/`decode` — needs
//! the per-channel link keystream schedule, reversed in research but not yet
//! ported; see `research/clb-crack/link_cipher.py`), which lands as an
//! `AsyncIsoTpTransport` impl on [`CableHandle`].

// Without the `d2xx` backend feature (and outside tests), the dedicated-thread
// byte-pipe bridge has no caller and is deliberately dormant scaffolding — do
// not warn on it in that configuration.
#![cfg_attr(not(any(feature = "d2xx", test)), allow(dead_code))]

pub mod actor;
pub mod error;
pub mod frame;
pub mod init;
pub mod usb;

pub use actor::{CableActor, CableHandle, spawn};
pub use error::HexError;
pub use frame::{Frame, MARKER_CABLE, MARKER_HOST};
pub use init::{CableIdentity, HANDSHAKE_TIMEOUT, handshake};
pub use usb::{Backend, CableInfo, D2xxBackend, FTDI_VID, list_cables};
