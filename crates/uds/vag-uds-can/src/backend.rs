use alloc::vec::Vec;
use core::time::Duration;
use vag_uds_transport::{CanId, MaybeSend};

use crate::CanError;

/// Bit 31 set marks a 29-bit (extended) id in the raw `u32` representation,
/// mirroring the SocketCAN `CAN_EFF_FLAG` convention.
pub const CAN_EFF_FLAG: u32 = 0x8000_0000;
/// Mask for an 11-bit (standard) id.
pub const CAN_SFF_MASK: u32 = 0x0000_07FF;
/// Mask for a 29-bit (extended) id.
pub const CAN_EFF_MASK: u32 = 0x1FFF_FFFF;

/// Convert a typed [`CanId`] into the raw `u32` form used by [`CanBackend`].
pub fn to_raw_id(id: CanId) -> u32 {
	match id {
		CanId::Standard(v) => u32::from(v) & CAN_SFF_MASK,
		CanId::Extended(v) => (v & CAN_EFF_MASK) | CAN_EFF_FLAG,
	}
}

/// Convert a raw `u32` id back into the typed [`CanId`] form.
pub fn from_raw_id(raw: u32) -> CanId {
	if raw & CAN_EFF_FLAG != 0 {
		CanId::Extended(raw & CAN_EFF_MASK)
	} else {
		CanId::Standard((raw & CAN_SFF_MASK) as u16)
	}
}

/// Raw classic-CAN frame I/O over some adapter (slcan serial, socketcan, mock).
///
/// Ids are raw `u32`s: 11-bit standard by default, or 29-bit extended when
/// [`CAN_EFF_FLAG`] (bit 31) is set. Static dispatch only — consumers take
/// `B: CanBackend`, no `dyn`. [`MaybeSend`] is `Send` on the host and nothing
/// on the board, where esp-hal's async peripherals are pinned to one core.
#[allow(async_fn_in_trait)] // static-dispatch seam; callers add Send bounds as needed
pub trait CanBackend: MaybeSend {
	/// Transmit one classic CAN frame (`data` must be <= 8 bytes).
	async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), CanError>;
	/// Receive the next CAN frame, waiting at most `timeout`.
	async fn recv_frame(&mut self, timeout: Duration) -> Result<(u32, Vec<u8>), CanError>;
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn raw_id_roundtrip_standard_and_extended() {
		let std = CanId::Standard(0x7E0);
		let ext = CanId::Extended(0x18DA_10F1);
		assert_eq!(to_raw_id(std), 0x7E0);
		assert_eq!(to_raw_id(ext), 0x18DA_10F1 | CAN_EFF_FLAG);
		assert_eq!(from_raw_id(to_raw_id(std)), std);
		assert_eq!(from_raw_id(to_raw_id(ext)), ext);
	}

	#[test]
	fn extended_id_below_0x800_stays_extended() {
		// An extended id numerically <= 0x7FF is still a distinct wire id.
		let ext = CanId::Extended(0x123);
		assert_eq!(from_raw_id(to_raw_id(ext)), ext);
	}
}
