use std::time::Duration;
use vag_transport::{CanFrame, CanId, IsoTpTransport, RawCanTransport, TransportError};

#[allow(dead_code)]
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
        if data.len() > 7 {
            return Err(TransportError::Unsupported("multi-frame send not yet implemented"));
        }
        // Single Frame: PCI byte high nibble = 0 (SF), low nibble = length.
        let mut frame = Vec::with_capacity(8);
        frame.push(data.len() as u8);
        frame.extend_from_slice(data);
        self.inner.send_frame(&CanFrame::new(self.tx, Self::pad8(frame)))
    }

    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let frame = self.inner.recv_frame(timeout)?;
        if frame.id != self.rx {
            return Err(TransportError::Protocol(format!(
                "unexpected rx id {:?}, want {:?}",
                frame.id, self.rx
            )));
        }
        let pci = *frame.data.first().ok_or_else(|| TransportError::Protocol("empty frame".into()))?;
        let kind = pci >> 4;
        match kind {
            0 => {
                let len = (pci & 0x0F) as usize;
                let body = frame.data.get(1..1 + len).ok_or_else(|| {
                    TransportError::Protocol("single frame length exceeds data".into())
                })?;
                Ok(body.to_vec())
            }
            _ => Err(TransportError::Unsupported("multi-frame recv not yet implemented")),
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
}
