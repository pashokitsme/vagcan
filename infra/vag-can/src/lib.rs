//! Generic CAN transport — the fallback path that bypasses the HEX cable's
//! encrypted link entirely: UDS-over-ISO-TP-over-CAN through a plain USB-CAN
//! adapter (slcan/LAWICEL first, since the host is macOS).
//!
//! Plugs into the same [`vag_transport::AsyncIsoTpTransport`] seam the rest of
//! the stack consumes, so `vagcan info` works over it unchanged.

pub mod backend;
pub mod error;
pub mod isotp;
pub mod slcan;
pub mod sniff;

pub use backend::{CAN_EFF_FLAG, CAN_EFF_MASK, CAN_SFF_MASK, CanBackend, from_raw_id, to_raw_id};
pub use error::CanError;
pub use isotp::IsoTpCan;
#[cfg(feature = "slcan")]
pub use slcan::{AdapterInfo, SerialSlcan, list_adapters};
pub use slcan::{SlcanBackend, SlcanBitrate, SlcanMode};
pub use sniff::{DEFAULT_ASSEMBLY_TIMEOUT, IsoTpSniffer, SnifferPdu};
