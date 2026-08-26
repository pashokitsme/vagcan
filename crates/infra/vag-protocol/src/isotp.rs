use std::time::Duration;
use vag_transport::{CanFrame, CanId, IsoTpTransport, RawCanTransport, TransportError};

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Software ISO-TP (ISO 15765-2) over a raw CAN transport, one ECU channel.
pub struct SoftwareIsoTp<T: RawCanTransport> {
	inner: T,
	tx: CanId,
	rx: CanId,
}

impl<T: RawCanTransport> SoftwareIsoTp<T> {
	pub fn new(inner: T, tx: CanId, rx: CanId) -> Self {
		SoftwareIsoTp { inner, tx, rx }
	}

	fn pad8(mut data: Vec<u8>) -> Vec<u8> {
		while data.len() < 8 {
			data.push(0x00);
		}
		data
	}
}

impl<T: RawCanTransport> IsoTpTransport for SoftwareIsoTp<T> {
	fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
		if data.len() <= 7 {
			let mut frame = Vec::with_capacity(8);
			frame.push(data.len() as u8);
			frame.extend_from_slice(data);
			return self.inner.send_frame(&CanFrame::new(self.tx, Self::pad8(frame)));
		}
		if data.len() > 4095 {
			return Err(TransportError::Unsupported("payload exceeds 4095 bytes"));
		}
		// First Frame: PCI = 0x1 (high nibble) + 12-bit length; 6 payload bytes follow.
		let len = data.len() as u16;
		let mut ff = Vec::with_capacity(8);
		ff.push(0x10 | ((len >> 8) as u8 & 0x0F));
		ff.push((len & 0xFF) as u8);
		ff.extend_from_slice(&data[..6]);
		self.inner.send_frame(&CanFrame::new(self.tx, ff))?;

		// Wait for Flow Control (ContinueToSend). Block size / STmin ignored in P0.
		let fc = self.inner.recv_frame(DEFAULT_TIMEOUT)?;
		let fc_pci = *fc.data.first().ok_or_else(|| TransportError::Protocol("empty FC".into()))?;
		if fc_pci >> 4 != 0x3 {
			return Err(TransportError::Protocol("expected flow control frame".into()));
		}
		if fc_pci & 0x0F != 0x0 {
			return Err(TransportError::Protocol("flow status not ContinueToSend".into()));
		}

		// Consecutive Frames: PCI = 0x2 + sequence (1..15 wrapping), 7 payload bytes each.
		let mut seq: u8 = 1;
		let mut offset = 6;
		while offset < data.len() {
			let end = (offset + 7).min(data.len());
			let mut cf = Vec::with_capacity(8);
			cf.push(0x20 | (seq & 0x0F));
			cf.extend_from_slice(&data[offset..end]);
			self.inner.send_frame(&CanFrame::new(self.tx, Self::pad8(cf)))?;
			offset = end;
			seq = (seq + 1) & 0x0F;
		}
		Ok(())
	}

	fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
		let frame = self.inner.recv_frame(timeout)?;
		if frame.id != self.rx {
			return Err(TransportError::Protocol(format!("unexpected rx id {:?}, want {:?}", frame.id, self.rx)));
		}
		let pci = *frame.data.first().ok_or_else(|| TransportError::Protocol("empty frame".into()))?;
		match pci >> 4 {
			0 => {
				let len = (pci & 0x0F) as usize;
				let body = frame
					.data
					.get(1..1 + len)
					.ok_or_else(|| TransportError::Protocol("single frame length exceeds data".into()))?;
				Ok(body.to_vec())
			}
			1 => {
				// First Frame: 12-bit length, 6 data bytes here.
				let len_low = *frame
					.data
					.get(1)
					.ok_or_else(|| TransportError::Protocol("malformed first frame".into()))?;
				let len = (((pci & 0x0F) as usize) << 8) | (len_low as usize);
				if len <= 7 {
					return Err(TransportError::Protocol("first frame with length <= 7".into()));
				}
				let mut out: Vec<u8> = frame
					.data
					.get(2..8)
					.ok_or_else(|| TransportError::Protocol("malformed first frame".into()))?
					.to_vec();

				// Send Flow Control: ContinueToSend, block size 0, STmin 0.
				let fc = CanFrame::new(self.tx, Self::pad8(vec![0x30, 0x00, 0x00]));
				self.inner.send_frame(&fc)?;

				// Collect Consecutive Frames.
				let mut expected_seq: u8 = 1;
				while out.len() < len {
					let cf = self.inner.recv_frame(timeout)?;
					if cf.id != self.rx {
						return Err(TransportError::Protocol("CF from unexpected id".into()));
					}
					let cf_pci = *cf.data.first().ok_or_else(|| TransportError::Protocol("empty CF".into()))?;
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
					let remaining = len - out.len();
					let take = remaining.min(7);
					let payload = cf
						.data
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
	use vag_transport::{ScriptStep, ScriptedCan};

	const TX: CanId = CanId::Standard(0x7E0);
	const RX: CanId = CanId::Standard(0x7E8);

	#[test]
	fn sends_single_frame_padded_to_8() {
		let expected = CanFrame::new(TX, vec![0x02, 0x10, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00]);
		let can = ScriptedCan::new(vec![ScriptStep::ExpectSend(expected)]);
		let mut iso = SoftwareIsoTp::new(can, TX, RX);
		iso.send(&[0x10, 0x03]).unwrap();
	}

	#[test]
	fn receives_single_frame() {
		let reply = CanFrame::new(RX, vec![0x02, 0x50, 0x03, 0, 0, 0, 0, 0]);
		let can = ScriptedCan::new(vec![ScriptStep::Reply(reply)]);
		let mut iso = SoftwareIsoTp::new(can, TX, RX);
		let got = iso.recv(Duration::from_millis(10)).unwrap();
		assert_eq!(got, vec![0x50, 0x03]);
	}

	#[test]
	fn sends_multi_frame_ff_then_cfs_after_flow_control() {
		// 10-byte payload -> FF carries 6 bytes, then 1 CF carries remaining 4.
		let payload: Vec<u8> = (0..10).collect();

		let ff = CanFrame::new(TX, vec![0x10, 0x0A, 0, 1, 2, 3, 4, 5]);
		let fc = CanFrame::new(RX, vec![0x30, 0x00, 0x00, 0, 0, 0, 0, 0]); // CTS, bs=0, stmin=0
		let cf = CanFrame::new(TX, vec![0x21, 6, 7, 8, 9, 0x00, 0x00, 0x00]);

		let can = ScriptedCan::new(vec![ScriptStep::ExpectSend(ff), ScriptStep::Reply(fc), ScriptStep::ExpectSend(cf)]);
		let mut iso = SoftwareIsoTp::new(can, TX, RX);
		iso.send(&payload).unwrap();
	}

	#[test]
	fn receives_multi_frame_sends_flow_control() {
		// 10-byte response: FF (len=10, 6 bytes) then CF with remaining 4 bytes.
		let ff = CanFrame::new(RX, vec![0x10, 0x0A, 0x50, 0x03, 1, 2, 3, 4]);
		let fc = CanFrame::new(TX, vec![0x30, 0x00, 0x00, 0, 0, 0, 0, 0]);
		let cf = CanFrame::new(RX, vec![0x21, 5, 6, 7, 8, 0, 0, 0]);

		let can = ScriptedCan::new(vec![ScriptStep::Reply(ff), ScriptStep::ExpectSend(fc), ScriptStep::Reply(cf)]);
		let mut iso = SoftwareIsoTp::new(can, TX, RX);
		let got = iso.recv(Duration::from_millis(50)).unwrap();
		assert_eq!(got, vec![0x50, 0x03, 1, 2, 3, 4, 5, 6, 7, 8]);
	}

	#[test]
	fn truncated_first_frame_errors_instead_of_panicking() {
		// Claims length 10 (0x0A) but only carries 1 payload byte instead of 6.
		let ff = CanFrame::new(RX, vec![0x10, 0x0A, 0x50]);
		let can = ScriptedCan::new(vec![ScriptStep::Reply(ff)]);
		let mut iso = SoftwareIsoTp::new(can, TX, RX);
		let err = iso.recv(Duration::from_millis(10)).unwrap_err();
		assert!(matches!(err, TransportError::Protocol(_)));
	}
}
