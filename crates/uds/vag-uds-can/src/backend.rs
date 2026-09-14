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

	/// Throw away every frame the backend can hand over without waiting, and say how many
	/// there were — called between two exchanges, before a request goes out.
	///
	/// Between exchanges nothing should be waiting, and sometimes something is: an answer
	/// that arrived after its request stopped waiting for it. If that request is repeated,
	/// the late answer echoes the same identifier and passes for the new one's, and every
	/// read of that unit then runs one answer behind. The board sweeps its receive queue
	/// the same way before each request (`vag-dash-fw`'s `TwaiBackend::drain`).
	///
	/// "Without waiting" is each [`recv_frame`](Self::recv_frame) polled once: a frame
	/// already queued — in the backend, or in the port's buffer — comes back on that poll,
	/// and the first that would wait ends the sweep, dropped unfinished. The timeout
	/// handed to it is never waited out; it is non-zero only so that a backend which
	/// checks for an expired deadline before it reads (slcan does) still reads.
	///
	/// The sweep runs until the backend would wait, not for a number of frames. The slcan
	/// cable has no acceptance filter, so between two exchanges its port holds the whole
	/// bus's traffic, and a late answer can sit behind hundreds of foreign frames (PR #2
	/// review round 2, S2-F2). It stops early only at [`MAX_DISCARD_TIME`] or
	/// [`MAX_DISCARDED`] frames, so a bus that queues frames as fast as they are swept
	/// costs one bounded sweep.
	async fn discard_queued(&mut self) -> usize {
		let started = crate::time::Instant::now();
		let mut discarded = 0;
		while discarded < MAX_DISCARDED && crate::time::Instant::now().saturating_duration_since(started) < MAX_DISCARD_TIME {
			let mut next = core::pin::pin!(self.recv_frame(Duration::from_millis(1)));
			let now = core::future::poll_fn(|cx| {
				core::task::Poll::Ready(match core::future::Future::poll(next.as_mut(), cx) {
					core::task::Poll::Ready(frame) => Some(frame),
					core::task::Poll::Pending => None,
				})
			})
			.await;
			match now {
				Some(Ok(_)) => discarded += 1,
				// Nothing waiting, or an error that describes a moment already past: the
				// exchange that follows reports a link that is still failing.
				Some(Err(_)) | None => break,
			}
		}
		discarded
	}
}

/// The longest one [`CanBackend::discard_queued`] sweep runs. A frame already queued is
/// taken in microseconds, so this clears thousands; it is under the 10 ms the planner's
/// ceiling of 100 exchanges a second leaves between two sends.
pub const MAX_DISCARD_TIME: Duration = Duration::from_millis(5);

/// The most frames one [`CanBackend::discard_queued`] sweep throws away: the bound for a
/// clock that does not move while the sweep runs. 4096 slcan lines are about 100 kB, more
/// than a serial port buffers.
pub const MAX_DISCARDED: usize = 4096;

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

	/// A backend holding `queued` frames, whose receive waits once they are gone.
	struct Queued {
		queued: usize,
		sent: usize,
	}

	impl CanBackend for Queued {
		async fn send_frame(&mut self, _id: u32, _data: &[u8]) -> Result<(), CanError> {
			self.sent += 1;
			Ok(())
		}

		async fn recv_frame(&mut self, timeout: Duration) -> Result<(u32, Vec<u8>), CanError> {
			if self.queued == 0 {
				tokio::time::sleep(timeout).await;
				return Err(CanError::Timeout);
			}
			self.queued -= 1;
			Ok((0x7E8, alloc::vec![0x02, 0x7E, 0x00]))
		}
	}

	#[tokio::test(start_paused = true)]
	async fn the_sweep_takes_what_is_queued_and_never_waits_for_more() {
		let mut backend = Queued { queued: 3, sent: 0 };
		let started = tokio::time::Instant::now();
		assert_eq!(backend.discard_queued().await, 3);
		assert_eq!(backend.discard_queued().await, 0, "nothing left, and nothing waited for");
		assert_eq!(started.elapsed(), Duration::ZERO, "the paused clock never moved");
		assert_eq!(backend.sent, 0, "a sweep sends nothing");
		let mut busy = Queued {
			queued: MAX_DISCARDED * 2,
			sent: 0,
		};
		assert_eq!(busy.discard_queued().await, MAX_DISCARDED, "a bus that never falls quiet is swept once");
	}

	/// A backend holding these frames in order, whose receive waits once they are gone —
	/// what the slcan port holds between exchanges: the cable has no acceptance filter.
	struct Port {
		frames: alloc::collections::VecDeque<(u32, Vec<u8>)>,
	}

	impl CanBackend for Port {
		async fn send_frame(&mut self, _id: u32, _data: &[u8]) -> Result<(), CanError> {
			Ok(())
		}

		async fn recv_frame(&mut self, timeout: Duration) -> Result<(u32, Vec<u8>), CanError> {
			match self.frames.pop_front() {
				Some(frame) => Ok(frame),
				None => {
					tokio::time::sleep(timeout).await;
					Err(CanError::Timeout)
				}
			}
		}
	}

	/// PR #2 review round 2 (S2-F2): a late answer that landed behind more of the whole bus's
	/// traffic than one sweep took was still queued after it, and the next identical request's
	/// channel, skipping the foreign ids, took it for its own answer.
	#[tokio::test(start_paused = true)]
	async fn a_late_answer_behind_the_whole_buss_traffic_is_swept_with_it() {
		use vag_uds_transport::AsyncIsoTpTransport;
		let mut frames: alloc::collections::VecDeque<(u32, Vec<u8>)> = (0..500).map(|n| (0x280 + (n % 16), alloc::vec![0u8; 8])).collect();
		// The late answer to `22 F190` on the engine's pair.
		frames.push_back((0x7E8, alloc::vec![0x04, 0x62, 0xF1, 0x90, 0x2A, 0, 0, 0]));
		let mut port = Port { frames };
		let swept = port.discard_queued().await;
		let mut channel = crate::IsoTpCan::new(port, CanId::Standard(0x7E0), CanId::Standard(0x7E8));
		let heard = channel.recv(Duration::from_millis(50)).await;
		assert!(heard.is_err(), "the stale answer survived a sweep of {swept}: {heard:02X?}");
		assert_eq!(swept, 501, "every frame queued, the late answer last");

		let mut empty = channel.into_backend();
		let started = tokio::time::Instant::now();
		assert_eq!(empty.discard_queued().await, 0);
		assert_eq!(started.elapsed(), Duration::ZERO, "an empty port is swept without waiting");
	}

	/// A bus that queues a frame for every one swept, each taking a millisecond to hand over.
	struct NeverQuiet;

	impl CanBackend for NeverQuiet {
		async fn send_frame(&mut self, _id: u32, _data: &[u8]) -> Result<(), CanError> {
			Ok(())
		}

		async fn recv_frame(&mut self, _timeout: Duration) -> Result<(u32, Vec<u8>), CanError> {
			std::thread::sleep(Duration::from_millis(1));
			Ok((0x280, alloc::vec![0u8; 8]))
		}
	}

	#[tokio::test]
	async fn a_bus_that_never_falls_quiet_is_swept_for_a_bounded_time() {
		let started = std::time::Instant::now();
		let swept = NeverQuiet.discard_queued().await;
		let took = started.elapsed();
		assert!(swept > 0 && swept < MAX_DISCARDED, "swept {swept}");
		assert!(took >= MAX_DISCARD_TIME, "stopped after {took:?}, before the bound");
		assert!(took < MAX_DISCARD_TIME * 20, "a sweep of {swept} took {took:?}");
	}

	#[test]
	fn extended_id_below_0x800_stays_extended() {
		// An extended id numerically <= 0x7FF is still a distinct wire id.
		let ext = CanId::Extended(0x123);
		assert_eq!(from_raw_id(to_raw_id(ext)), ext);
	}
}
