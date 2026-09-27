//! UDS client + ISO-TP + unit addressing.
//!
//! **Runs on the board**, minus one module and a half. `--no-default-features` builds
//! this crate `no_std` (`alloc` only) and drops [`read`] (which decodes a measurement
//! against a `vag-data-labels` catalog, and is the only reason this crate depends on
//! `vag-data-labels` at all) and the half of [`address`] that reads the filesystem —
//! the short-number table. The addressing rule itself stays: the board reads the
//! faults of every unit the gateway lists ([`faultcount`]), and needs each one's
//! response id. The board executes a plan with the scaling already baked in, so it
//! needs nothing else of either.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod address;
pub mod console;
pub mod dtc;
pub mod faultcount;
pub mod gateway;
pub mod guard;
pub mod identity;
pub mod isotp;
mod pdu;
#[cfg(feature = "std")]
pub mod read;
pub mod remote;
pub mod schedule;
pub mod uds;
pub mod uds_async;
pub use address::UnitAddress;
pub use dtc::RawDtc;
pub use identity::EcuIdentity;
pub use isotp::SoftwareIsoTp;
pub use pdu::check_read_only;
#[cfg(feature = "std")]
pub use read::{Reading, UdsReadExt};
pub use uds::{UdsClient, UdsError};
pub use uds_async::AsyncUdsClient;
