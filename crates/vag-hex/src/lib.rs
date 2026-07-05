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
//! see `research/SCOPE-BOUNDARY.md`). [`link`] carries the DECODE side of the
//! `0xb8`/`0xb7` diagnostic link cipher: [`decrypt_block`] (per-channel XOR),
//! [`recover_keystream`] from known-plaintext, and [`decode_diag_frame`] yielding
//! the inner single-frame UDS PDU — decode only, using a per-session keystream
//! recovered from the capture's UDS known-plaintext (the AES session key is
//! auth-derived and out of scope; see `research/clb-crack/link_cipher.py`). Still
//! pending hardware/capture: wiring this decode + multiframe ISO-TP reassembly
//! into an `AsyncIsoTpTransport` impl on [`CableHandle`] (the `frame::encode`/
//! `decode` seams stay gated until then).

// Without the `d2xx` backend feature (and outside tests), the dedicated-thread
// byte-pipe bridge has no caller and is deliberately dormant scaffolding — do
// not warn on it in that configuration.
#![cfg_attr(not(any(feature = "d2xx", test)), allow(dead_code))]

pub mod actor;
pub mod drive;
pub mod error;
pub mod frame;
pub mod init;
pub mod link;
pub mod probe;
pub mod usb;

pub use actor::{CableActor, CableHandle, spawn};
pub use drive::{AUTH39_BLOCK, DriveReport, drive_session, drive_session_sweep};
pub use error::HexError;
pub use frame::{Frame, MARKER_CABLE, MARKER_HOST};
pub use link::{
    IsoTpReassembler, KS_F3, UdsSlice, decode_diag_frame, decrypt_block, encode_f3_request,
    encode_request, f3_trailer, paired_off14, recover_keystream,
};
pub use init::{CableIdentity, HANDSHAKE_TIMEOUT, handshake};
pub use probe::{BRINGUP, ProbeReport, probe_open};
pub use usb::{Backend, CableInfo, D2xxBackend, FTDI_VID, list_cables};
