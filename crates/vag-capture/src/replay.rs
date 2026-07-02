use std::collections::VecDeque;
use std::time::Duration;
use vag_transport::{CanFrame, RawCanTransport, TransportError};
use crate::record::{CapturePayload, CaptureRecord, Direction};

/// Replays a capture as a RawCanTransport: Rx records are handed to `recv_frame`,
/// Tx records are asserted against `send_frame`. CableBytes payloads are ignored.
pub struct ReplayCan {
    queue: VecDeque<CaptureRecord>,
}

impl ReplayCan {
    pub fn new(records: Vec<CaptureRecord>) -> Self {
        let queue = records
            .into_iter()
            .filter(|r| matches!(r.payload, CapturePayload::CanFrame { .. }))
            .collect();
        ReplayCan { queue }
    }
}

fn as_frame(rec: &CaptureRecord) -> CanFrame {
    match &rec.payload {
        CapturePayload::CanFrame { id, data } => CanFrame::new(*id, data.clone()),
        CapturePayload::CableBytes { .. } => unreachable!("filtered in new()"),
    }
}

impl RawCanTransport for ReplayCan {
    fn send_frame(&mut self, frame: &CanFrame) -> Result<(), TransportError> {
        match self.queue.front() {
            Some(rec) if rec.dir == Direction::Tx => {
                let expected = as_frame(rec);
                if *frame != expected {
                    return Err(TransportError::Protocol(format!(
                        "replay mismatch: sent {frame:?}, capture expected {expected:?}"
                    )));
                }
                self.queue.pop_front();
                Ok(())
            }
            _ => Err(TransportError::Protocol(
                "replay: send_frame with no matching Tx record".into(),
            )),
        }
    }

    fn recv_frame(&mut self, _timeout: Duration) -> Result<CanFrame, TransportError> {
        // Skip any leading Tx records that were not consumed (defensive), then take next Rx.
        while let Some(rec) = self.queue.front() {
            if rec.dir == Direction::Rx {
                let frame = as_frame(rec);
                self.queue.pop_front();
                return Ok(frame);
            }
            break;
        }
        Err(TransportError::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vag_transport::CanId;

    fn tx(id: u16, data: Vec<u8>) -> CaptureRecord {
        CaptureRecord { ts_us: 0, dir: Direction::Tx, payload: CapturePayload::CanFrame { id: CanId::Standard(id), data } }
    }
    fn rx(id: u16, data: Vec<u8>) -> CaptureRecord {
        CaptureRecord { ts_us: 0, dir: Direction::Rx, payload: CapturePayload::CanFrame { id: CanId::Standard(id), data } }
    }

    #[test]
    fn replays_tx_then_rx_in_order() {
        let mut can = ReplayCan::new(vec![
            tx(0x7E0, vec![0x02, 0x10, 0x03]),
            rx(0x7E8, vec![0x06, 0x50, 0x03]),
        ]);
        can.send_frame(&CanFrame::new(CanId::Standard(0x7E0), vec![0x02, 0x10, 0x03])).unwrap();
        let got = can.recv_frame(Duration::from_millis(1)).unwrap();
        assert_eq!(got, CanFrame::new(CanId::Standard(0x7E8), vec![0x06, 0x50, 0x03]));
    }

    #[test]
    fn send_mismatch_is_protocol_error() {
        let mut can = ReplayCan::new(vec![tx(0x7E0, vec![0x01])]);
        let err = can.send_frame(&CanFrame::new(CanId::Standard(0x7E0), vec![0x99])).unwrap_err();
        assert!(matches!(err, TransportError::Protocol(_)));
    }
}
