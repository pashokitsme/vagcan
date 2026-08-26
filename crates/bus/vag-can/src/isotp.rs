use std::time::Duration;
use tokio::time::Instant;
use vag_transport::{AsyncIsoTpTransport, CanId, TransportError};

use crate::backend::{CanBackend, to_raw_id};

/// Frame padding byte (classic CAN diagnostic frames are padded to 8 bytes).
const PAD: u8 = 0x00;
/// N_Bs: how long to wait for the ECU's flow-control frame after a First Frame.
const FC_TIMEOUT: Duration = Duration::from_millis(1000);
/// How many FC.WAIT frames we tolerate before giving up (ISO 15765-2 N_WFTmax).
const MAX_FC_WAIT: usize = 8;

/// One ISO-TP (ISO 15765-2) channel to a single ECU over a raw CAN backend.
///
/// Implements [`AsyncIsoTpTransport`], so the async UDS client rides it
/// unchanged. Classic CAN only (<= 4095-byte PDUs), frames padded to 8 bytes.
/// Frames from other CAN ids are skipped, not treated as errors — this is a
/// shared bus.
pub struct IsoTpCan<B: CanBackend> {
	backend: B,
	tx: u32,
	rx: u32,
}

impl<B: CanBackend> IsoTpCan<B> {
	/// Channel with explicit tester (`tx`) and ECU (`rx`) ids.
	pub fn new(backend: B, tx: CanId, rx: CanId) -> Self {
		IsoTpCan {
			backend,
			tx: to_raw_id(tx),
			rx: to_raw_id(rx),
		}
	}

	/// UDS physical addressing for ECU index `n`: tester `0x7E0+n`, ECU `0x7E8+n`.
	pub fn for_ecu(backend: B, n: u8) -> Self {
		IsoTpCan {
			backend,
			tx: 0x7E0 + u32::from(n),
			rx: 0x7E8 + u32::from(n),
		}
	}

	/// Consume the channel, returning the backend.
	pub fn into_backend(self) -> B {
		self.backend
	}

	fn pad8(mut frame: Vec<u8>) -> Vec<u8> {
		frame.resize(8, PAD);
		frame
	}

	/// Next frame from our ECU (`rx` id), skipping unrelated bus traffic,
	/// bounded by `deadline`.
	async fn recv_own(&mut self, deadline: Instant) -> Result<Vec<u8>, TransportError> {
		loop {
			let remaining = deadline.saturating_duration_since(Instant::now());
			if remaining.is_zero() {
				return Err(TransportError::Timeout);
			}
			let (id, data) = self.backend.recv_frame(remaining).await?;
			if id == self.rx {
				return Ok(data);
			}
		}
	}

	/// Wait for a flow-control frame; returns `(block_size, stmin)` on CTS.
	async fn wait_flow_control(&mut self) -> Result<(u8, u8), TransportError> {
		for _ in 0..=MAX_FC_WAIT {
			let deadline = Instant::now() + FC_TIMEOUT;
			let data = self.recv_own(deadline).await?;
			let pci = *data.first().ok_or_else(|| TransportError::Protocol("empty flow control frame".into()))?;
			if pci >> 4 != 0x3 {
				return Err(TransportError::Protocol("expected flow control frame".into()));
			}
			match pci & 0x0F {
				0x0 => {
					let bs = data.get(1).copied().unwrap_or(0);
					let stmin = data.get(2).copied().unwrap_or(0);
					return Ok((bs, stmin));
				}
				0x1 => continue, // FC.WAIT: sender must keep waiting
				0x2 => return Err(TransportError::Protocol("flow control: buffer overflow".into())),
				fs => {
					return Err(TransportError::Protocol(format!("invalid flow status {fs:#x}")));
				}
			}
		}
		Err(TransportError::Protocol("too many FC.WAIT frames".into()))
	}
}

/// STmin byte -> minimum gap between consecutive frames (ISO 15765-2 §9.6.5.4).
fn stmin_gap(stmin: u8) -> Duration {
	match stmin {
		0x00..=0x7F => Duration::from_millis(u64::from(stmin)),
		0xF1..=0xF9 => Duration::from_micros(u64::from(stmin - 0xF0) * 100),
		// Reserved values: be conservative, use the maximum.
		_ => Duration::from_millis(0x7F),
	}
}

impl<B: CanBackend> AsyncIsoTpTransport for IsoTpCan<B> {
	async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError> {
		// Single Frame: PCI 0x0N (N = length), fits in 7 bytes.
		if pdu.len() <= 7 {
			let mut frame = Vec::with_capacity(8);
			frame.push(pdu.len() as u8);
			frame.extend_from_slice(pdu);
			self.backend.send_frame(self.tx, &Self::pad8(frame)).await?;
			return Ok(());
		}
		if pdu.len() > 4095 {
			return Err(TransportError::Unsupported("payload exceeds 4095 bytes"));
		}

		// First Frame: PCI 0x1 + 12-bit length, then 6 payload bytes.
		let len = pdu.len() as u16;
		let mut ff = Vec::with_capacity(8);
		ff.push(0x10 | ((len >> 8) as u8 & 0x0F));
		ff.push((len & 0xFF) as u8);
		ff.extend_from_slice(&pdu[..6]);
		self.backend.send_frame(self.tx, &ff).await?;

		let (mut bs, mut stmin) = self.wait_flow_control().await?;

		// Consecutive Frames: PCI 0x2 + wrapping sequence, 7 payload bytes each.
		let mut seq: u8 = 1;
		let mut offset = 6;
		let mut sent_in_block: u8 = 0;
		while offset < pdu.len() {
			let gap = stmin_gap(stmin);
			if !gap.is_zero() {
				tokio::time::sleep(gap).await;
			}
			let end = (offset + 7).min(pdu.len());
			let mut cf = Vec::with_capacity(8);
			cf.push(0x20 | (seq & 0x0F));
			cf.extend_from_slice(&pdu[offset..end]);
			self.backend.send_frame(self.tx, &Self::pad8(cf)).await?;
			offset = end;
			seq = (seq + 1) & 0x0F;
			sent_in_block += 1;
			if bs != 0 && sent_in_block == bs && offset < pdu.len() {
				(bs, stmin) = self.wait_flow_control().await?;
				sent_in_block = 0;
			}
		}
		Ok(())
	}

	async fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
		let deadline = Instant::now() + timeout;
		let frame = self.recv_own(deadline).await?;
		let pci = *frame.first().ok_or_else(|| TransportError::Protocol("empty frame".into()))?;
		match pci >> 4 {
			// Single Frame.
			0x0 => {
				let len = (pci & 0x0F) as usize;
				let body = frame
					.get(1..1 + len)
					.ok_or_else(|| TransportError::Protocol("single frame length exceeds data".into()))?;
				Ok(body.to_vec())
			}
			// First Frame: 12-bit length, 6 data bytes here.
			0x1 => {
				let len_low = *frame.get(1).ok_or_else(|| TransportError::Protocol("malformed first frame".into()))?;
				let len = (((pci & 0x0F) as usize) << 8) | usize::from(len_low);
				if len <= 7 {
					return Err(TransportError::Protocol("first frame with length <= 7".into()));
				}
				let mut out: Vec<u8> = frame
					.get(2..8)
					.ok_or_else(|| TransportError::Protocol("malformed first frame".into()))?
					.to_vec();

				// Flow Control: ContinueToSend, block size 0 (send all), STmin 0.
				let fc = Self::pad8(vec![0x30, 0x00, 0x00]);
				self.backend.send_frame(self.tx, &fc).await?;

				let mut expected_seq: u8 = 1;
				while out.len() < len {
					let cf = self.recv_own(deadline).await?;
					let cf_pci = *cf.first().ok_or_else(|| TransportError::Protocol("empty consecutive frame".into()))?;
					if cf_pci >> 4 != 0x2 {
						return Err(TransportError::Protocol("expected consecutive frame".into()));
					}
					if cf_pci & 0x0F != expected_seq {
						return Err(TransportError::Protocol(format!(
							"CF sequence mismatch: got {}, want {}",
							cf_pci & 0x0F,
							expected_seq
						)));
					}
					let take = (len - out.len()).min(7);
					let payload = cf
						.get(1..1 + take)
						.ok_or_else(|| TransportError::Protocol("malformed consecutive frame".into()))?;
					out.extend_from_slice(payload);
					expected_seq = (expected_seq + 1) & 0x0F;
				}
				Ok(out)
			}
			_ => Err(TransportError::Protocol("unexpected PCI in first frame position".into())),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::CanError;
	use std::collections::VecDeque;

	const TX: u32 = 0x7E0;
	const RX: u32 = 0x7E8;

	/// In-memory mock: records sent frames, replays a queue of incoming frames.
	struct MockCan {
		sent: Vec<(u32, Vec<u8>)>,
		replies: VecDeque<(u32, Vec<u8>)>,
	}

	impl MockCan {
		fn new(replies: Vec<(u32, Vec<u8>)>) -> Self {
			MockCan {
				sent: Vec::new(),
				replies: replies.into(),
			}
		}
	}

	impl CanBackend for MockCan {
		async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), CanError> {
			self.sent.push((id, data.to_vec()));
			Ok(())
		}
		async fn recv_frame(&mut self, _timeout: Duration) -> Result<(u32, Vec<u8>), CanError> {
			self.replies.pop_front().ok_or(CanError::Timeout)
		}
	}

	fn channel(replies: Vec<(u32, Vec<u8>)>) -> IsoTpCan<MockCan> {
		IsoTpCan::for_ecu(MockCan::new(replies), 0)
	}

	#[tokio::test]
	async fn single_frame_request_emits_one_padded_frame() {
		let mut iso = channel(vec![]);
		iso.send(&[0x10, 0x03]).await.unwrap();
		let sent = &iso.into_backend().sent;
		assert_eq!(sent.len(), 1);
		assert_eq!(sent[0].0, TX);
		assert_eq!(sent[0].1, vec![0x02, 0x10, 0x03, 0, 0, 0, 0, 0]);
	}

	#[tokio::test]
	async fn single_frame_response_returns_pdu() {
		let mut iso = channel(vec![(RX, vec![0x02, 0x50, 0x03, 0, 0, 0, 0, 0])]);
		let got = iso.recv(Duration::from_millis(50)).await.unwrap();
		assert_eq!(got, vec![0x50, 0x03]);
	}

	#[tokio::test]
	async fn recv_skips_frames_from_other_ids() {
		let mut iso = channel(vec![
			(0x5A0, vec![0x02, 0xAA, 0xBB, 0, 0, 0, 0, 0]), // chatter on the bus
			(RX, vec![0x02, 0x50, 0x03, 0, 0, 0, 0, 0]),
		]);
		let got = iso.recv(Duration::from_millis(50)).await.unwrap();
		assert_eq!(got, vec![0x50, 0x03]);
	}

	#[tokio::test]
	async fn multi_frame_send_waits_for_flow_control() {
		// 10-byte payload -> FF (6 bytes) + FC in + 1 CF (4 bytes).
		let payload: Vec<u8> = (0..10).collect();
		let mut iso = channel(vec![(RX, vec![0x30, 0x00, 0x00, 0, 0, 0, 0, 0])]);
		iso.send(&payload).await.unwrap();
		let sent = iso.into_backend().sent;
		assert_eq!(
			sent,
			vec![(TX, vec![0x10, 0x0A, 0, 1, 2, 3, 4, 5]), (TX, vec![0x21, 6, 7, 8, 9, 0, 0, 0]),]
		);
	}

	#[tokio::test]
	async fn multi_frame_send_honors_block_size() {
		// 27-byte payload -> FF (6) + 3 CFs (7 each). Block size 2 means a
		// second FC must be consumed after 2 CFs.
		let payload: Vec<u8> = (0..27).collect();
		let mut iso = channel(vec![
			(RX, vec![0x30, 0x02, 0x00, 0, 0, 0, 0, 0]),
			(RX, vec![0x30, 0x02, 0x00, 0, 0, 0, 0, 0]),
		]);
		iso.send(&payload).await.unwrap();
		let backend = iso.into_backend();
		assert_eq!(backend.sent.len(), 4, "FF + 3 CFs");
		assert!(backend.replies.is_empty(), "both FCs consumed");
		assert_eq!(backend.sent[3].1[0], 0x23, "third CF sequence number");
	}

	#[tokio::test]
	async fn multi_frame_send_respects_wait_flow_status() {
		let payload: Vec<u8> = (0..10).collect();
		let mut iso = channel(vec![
			(RX, vec![0x31, 0x00, 0x00, 0, 0, 0, 0, 0]), // FC.WAIT
			(RX, vec![0x30, 0x00, 0x00, 0, 0, 0, 0, 0]), // FC.CTS
		]);
		iso.send(&payload).await.unwrap();
		assert_eq!(iso.into_backend().sent.len(), 2, "FF + 1 CF");
	}

	#[tokio::test]
	async fn multi_frame_send_errors_on_overflow_flow_status() {
		let payload: Vec<u8> = (0..10).collect();
		let mut iso = channel(vec![(RX, vec![0x32, 0x00, 0x00, 0, 0, 0, 0, 0])]);
		let err = iso.send(&payload).await.unwrap_err();
		assert!(matches!(err, TransportError::Protocol(_)), "got {err:?}");
	}

	#[tokio::test]
	async fn oversized_pdu_is_rejected() {
		let mut iso = channel(vec![]);
		let err = iso.send(&vec![0u8; 4096]).await.unwrap_err();
		assert!(matches!(err, TransportError::Unsupported(_)), "got {err:?}");
	}

	#[tokio::test]
	async fn multi_frame_response_reassembles_and_sends_flow_control() {
		// 20-byte response: FF (6 bytes) + 2 CFs (7 each).
		let pdu: Vec<u8> = (0..20).collect();
		let mut iso = channel(vec![
			(RX, {
				let mut ff = vec![0x10, 0x14];
				ff.extend_from_slice(&pdu[..6]);
				ff
			}),
			(RX, {
				let mut cf = vec![0x21];
				cf.extend_from_slice(&pdu[6..13]);
				cf
			}),
			(RX, {
				let mut cf = vec![0x22];
				cf.extend_from_slice(&pdu[13..20]);
				cf
			}),
		]);
		let got = iso.recv(Duration::from_millis(50)).await.unwrap();
		assert_eq!(got, pdu);
		let sent = iso.into_backend().sent;
		assert_eq!(sent, vec![(TX, vec![0x30, 0x00, 0x00, 0, 0, 0, 0, 0])], "FC.CTS emitted");
	}

	#[tokio::test]
	async fn consecutive_frame_sequence_mismatch_errors() {
		let mut iso = channel(vec![
			(RX, vec![0x10, 0x0A, 0, 1, 2, 3, 4, 5]),
			(RX, vec![0x22, 6, 7, 8, 9, 0, 0, 0]), // seq 2, expected 1
		]);
		let err = iso.recv(Duration::from_millis(50)).await.unwrap_err();
		assert!(matches!(err, TransportError::Protocol(_)), "got {err:?}");
	}

	#[tokio::test]
	async fn recv_times_out_when_bus_is_silent() {
		let mut iso = channel(vec![]);
		let err = iso.recv(Duration::from_millis(5)).await.unwrap_err();
		assert!(matches!(err, TransportError::Timeout), "got {err:?}");
	}

	#[tokio::test]
	async fn truncated_first_frame_errors_instead_of_panicking() {
		// Claims 10 bytes but carries only 1.
		let mut iso = channel(vec![(RX, vec![0x10, 0x0A, 0x50])]);
		let err = iso.recv(Duration::from_millis(50)).await.unwrap_err();
		assert!(matches!(err, TransportError::Protocol(_)), "got {err:?}");
	}

	#[test]
	fn for_ecu_offsets_default_uds_ids() {
		let iso = IsoTpCan::for_ecu(MockCan::new(vec![]), 3);
		assert_eq!(iso.tx, 0x7E3);
		assert_eq!(iso.rx, 0x7EB);
	}

	#[test]
	fn new_accepts_extended_ids() {
		let iso = IsoTpCan::new(MockCan::new(vec![]), CanId::Extended(0x18DA_10F1), CanId::Extended(0x18DA_F110));
		assert_eq!(iso.tx, to_raw_id(CanId::Extended(0x18DA_10F1)));
		assert_eq!(iso.rx, to_raw_id(CanId::Extended(0x18DA_F110)));
	}
}
