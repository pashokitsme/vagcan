//! UDS client + ISO-TP + unit addressing.
//!
//! **Runs on the board**, minus two modules. `--no-default-features` builds
//! this crate `no_std` (`alloc` only) and drops [`address`] (which reads the
//! filesystem) and [`read`] (which decodes a measurement against a `vag-data`
//! catalog, and is the only reason this crate depends on `vag-data` at all).
//! The board executes a plan with the scaling already baked in, so it needs
//! neither.
#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

#[cfg(feature = "std")]
pub mod address;
pub mod dtc;
pub mod gateway;
pub mod identity;
pub mod isotp;
mod pdu;
#[cfg(feature = "std")]
pub mod read;
pub mod uds;
pub mod uds_async;
#[cfg(feature = "std")]
pub use address::UnitAddress;
pub use dtc::RawDtc;
pub use identity::EcuIdentity;
pub use isotp::SoftwareIsoTp;
#[cfg(feature = "std")]
pub use read::{Reading, UdsReadExt};
pub use uds::{UdsClient, UdsError};
pub use uds_async::AsyncUdsClient;
