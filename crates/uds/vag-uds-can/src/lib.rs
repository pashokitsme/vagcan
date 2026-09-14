//! Generic CAN transport — the fallback path that bypasses the HEX cable's
//! encrypted link entirely: UDS-over-ISO-TP-over-CAN through a plain USB-CAN
//! adapter (slcan/LAWICEL first, since the host is macOS).
//!
//! Plugs into the same [`vag_uds_transport::AsyncIsoTpTransport`] seam the rest of
//! the stack consumes, so `vagcan info` works over it unchanged.
//!
//! **Runs on the board**, minus the two host-only modules. With
//! `--no-default-features` the crate is `no_std` + `alloc` and keeps
//! [`backend`] (the [`CanBackend`] seam an ESP32-C3 TWAI driver implements),
//! [`isotp`] (the segmentation state machine, byte-for-byte the same one the
//! laptop runs) and [`link`] (the [`UnitLink`] seam a command addresses one unit
//! at a time through), and [`filter`] (which answer ids the controller hands up,
//! and when that has to move). [`slcan`] and [`sniff`] drop out: a serial port and a map
//! of whole-bus traffic are not things the board has.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod backend;
#[cfg(feature = "std")]
pub mod board;
pub mod error;
pub mod filter;
pub mod isotp;
pub mod link;
#[cfg(feature = "std")]
pub mod slcan;
#[cfg(feature = "std")]
pub mod sniff;
mod time;

pub use backend::{CAN_EFF_FLAG, CAN_EFF_MASK, CAN_SFF_MASK, CanBackend, from_raw_id, to_raw_id};
#[cfg(feature = "slcan")]
pub use board::SerialPipe;
#[cfg(feature = "std")]
pub use board::{HANDSHAKE_WAIT, StreamPipe};
pub use error::CanError;
pub use filter::{FilterFollower, StandardFilter};
pub use isotp::IsoTpCan;
pub use link::UnitLink;
#[cfg(feature = "slcan")]
pub use slcan::{AdapterInfo, BOARD_PROBE_WAIT, BOARD_USB, SerialSlcan, classify_usb, list_adapters, probe_board};
#[cfg(feature = "std")]
pub use slcan::{BoardAnswer, SlcanBackend, SlcanBitrate, SlcanMode};
#[cfg(feature = "std")]
pub use sniff::{DEFAULT_ASSEMBLY_TIMEOUT, IsoTpSniffer, SnifferPdu};
