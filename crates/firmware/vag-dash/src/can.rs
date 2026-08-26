//! [`vag_can::CanBackend`] over the ESP32-C3's TWAI controller.
//!
//! This is the whole of the board's CAN-specific code. Everything above it —
//! ISO-TP segmentation, flow control, the UDS client and its allowlist — is the
//! same source the laptop runs, compiled `no_std`; the seam is two async
//! methods, "put these bytes on the bus" and "give me the next frame".
//!
//! Two things this module deliberately does not do:
//!
//! * **It never picks a bit rate, a mode or a pin.** Those come in as a
//!   configured [`Twai`], because "which pins" is a property of the board it is
//!   soldered to and "which mode" is a safety decision (see below), not
//!   something a transport driver gets to assume.
//! * **It adds no service.** The UDS allowlist lives in `vag-protocol` and is
//!   `0x22`, `0x19`, `0x10`, `0x3E`. Nothing here can widen it — this layer
//!   only sees bytes.
//!
//! ## Listen-only is the safe default on a car
//!
//! `SAFETY.md` is the reason [`TwaiBackend::new`] is not the only constructor.
//! [`esp_hal::twai::TwaiMode::ListenOnly`] cannot acknowledge, cannot transmit
//! and cannot disturb a bus; it is what a first bring-up on a real car should
//! use. [`TwaiBackend::self_test`] is the opposite end — a bench mode that
//! hears its own frames, used by the `cantest` binary, and it is the only mode
//! that needs `EspTwaiFrame::new_self_reception`.

use alloc::format;
use alloc::vec::Vec;
use core::time::Duration;
use embassy_time::{Duration as EmbassyDuration, with_timeout};
use embedded_can::Frame as _;
use esp_hal::Async;
use esp_hal::twai::{EspTwaiError, EspTwaiFrame, ExtendedId, Id, StandardId, Twai};
use vag_can::{CAN_EFF_FLAG, CAN_EFF_MASK, CAN_SFF_MASK, CanBackend, CanError};

/// A [`CanBackend`] driving the chip's TWAI peripheral.
pub struct TwaiBackend<'d> {
	twai: Twai<'d, Async>,
	/// Whether outgoing frames are marked for self-reception. Only meaningful
	/// in `TwaiMode::SelfTest`; on a real bus the controller must not hear
	/// itself, or every request would come back as its own answer.
	self_reception: bool,
}

impl<'d> TwaiBackend<'d> {
	/// The ordinary case: frames go out, other people's frames come back.
	///
	/// The [`Twai`] must already be started. Use
	/// [`esp_hal::twai::TwaiMode::ListenOnly`] on a car until there is a reason
	/// not to — a listen-only controller cannot acknowledge and so cannot make
	/// a bus worse.
	pub fn new(twai: Twai<'d, Async>) -> Self {
		TwaiBackend { twai, self_reception: false }
	}

	/// Bench mode: every frame this backend sends is marked for self-reception,
	/// so a controller started in [`esp_hal::twai::TwaiMode::SelfTest`] hears it
	/// come back. Needs no transceiver and no second node.
	pub fn self_test(twai: Twai<'d, Async>) -> Self {
		TwaiBackend { twai, self_reception: true }
	}

	/// Give the peripheral back (to stop it, or to reconfigure the bit rate).
	pub fn into_twai(self) -> Twai<'d, Async> {
		self.twai
	}
}

impl CanBackend for TwaiBackend<'_> {
	async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), CanError> {
		let id = to_twai_id(id)?;
		let frame = if self.self_reception {
			EspTwaiFrame::new_self_reception(id, data)
		} else {
			EspTwaiFrame::new(id, data)
		}
		.ok_or(CanError::Unsupported("classic CAN frame data must be <= 8 bytes"))?;
		self.twai.transmit_async(&frame).await.map_err(twai_error)
	}

	async fn recv_frame(&mut self, timeout: Duration) -> Result<(u32, Vec<u8>), CanError> {
		match with_timeout(to_embassy(timeout), self.twai.receive_async()).await {
			Err(_elapsed) => Err(CanError::Timeout),
			Ok(Err(e)) => Err(twai_error(e)),
			Ok(Ok(frame)) => Ok((from_embedded_id(frame.id()), frame.data().to_vec())),
		}
	}
}

/// Longest wait this converter will represent, in microseconds — a bit over an
/// hour. `embassy_time::Duration::from_micros` scales by the tick rate before
/// dividing, so an unclamped `u64` overflows; no ISO-TP deadline comes close.
const MAX_MICROS: u64 = u32::MAX as u64;

fn to_embassy(d: Duration) -> EmbassyDuration {
	let micros = u64::try_from(d.as_micros()).unwrap_or(MAX_MICROS).min(MAX_MICROS);
	EmbassyDuration::from_micros(micros)
}

/// `vag-can`'s raw id (bit 31 = 29-bit id, SocketCAN's convention) -> TWAI id.
fn to_twai_id(raw: u32) -> Result<Id, CanError> {
	if raw & CAN_EFF_FLAG != 0 {
		ExtendedId::new(raw & CAN_EFF_MASK)
			.map(Id::Extended)
			.ok_or(CanError::Unsupported("extended id out of range"))
	} else {
		StandardId::new((raw & CAN_SFF_MASK) as u16)
			.map(Id::Standard)
			.ok_or(CanError::Unsupported("standard id out of range"))
	}
}

/// The inverse. `EspTwaiFrame` only exposes its id through `embedded_can::Frame`.
fn from_embedded_id(id: embedded_can::Id) -> u32 {
	match id {
		embedded_can::Id::Standard(s) => u32::from(s.as_raw()) & CAN_SFF_MASK,
		embedded_can::Id::Extended(e) => (e.as_raw() & CAN_EFF_MASK) | CAN_EFF_FLAG,
	}
}

/// A bus-off controller is gone until it is restarted, which is a different
/// thing from a frame that did not arrive — keep the two distinguishable.
fn twai_error(e: EspTwaiError) -> CanError {
	match e {
		EspTwaiError::BusOff => CanError::Disconnected,
		other => CanError::Io(format!("twai: {other:?}")),
	}
}
