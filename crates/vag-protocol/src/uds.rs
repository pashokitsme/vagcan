use std::time::Duration;
use vag_transport::{IsoTpTransport, TransportError};

const RESPONSE_TIMEOUT: Duration = Duration::from_millis(2000);
const MAX_PENDING: usize = 30;
const READ_ONLY_ALLOWLIST: &[u8] = &[0x10, 0x19, 0x22, 0x3E];

#[derive(thiserror::Error, Debug)]
pub enum UdsError {
    #[error("transport: {0}")]
    Transport(#[from] TransportError),
    #[error("negative response: sid=0x{sid:02X} nrc=0x{nrc:02X}")]
    NegativeResponse { sid: u8, nrc: u8 },
    #[error("malformed response: {0}")]
    Malformed(String),
    #[error("service 0x{0:02X} blocked by read-only allowlist")]
    Forbidden(u8),
}

pub struct UdsClient<C: IsoTpTransport> {
    channel: C,
}

impl<C: IsoTpTransport> UdsClient<C> {
    pub fn new(channel: C) -> Self {
        UdsClient { channel }
    }

    /// Send a UDS request; return response bytes after the echoed SID.
    pub fn request(&mut self, sid: u8, payload: &[u8]) -> Result<Vec<u8>, UdsError> {
        if !READ_ONLY_ALLOWLIST.contains(&sid) {
            return Err(UdsError::Forbidden(sid));
        }
        let mut req = Vec::with_capacity(1 + payload.len());
        req.push(sid);
        req.extend_from_slice(payload);
        self.channel.send(&req)?;

        for _ in 0..MAX_PENDING {
            let resp = self.channel.recv(RESPONSE_TIMEOUT)?;
            let first = *resp.first().ok_or_else(|| UdsError::Malformed("empty response".into()))?;
            if first == 0x7F {
                // Negative: [0x7F, sid, nrc]
                let nrc = *resp.get(2).ok_or_else(|| UdsError::Malformed("short negative response".into()))?;
                if nrc == 0x78 {
                    // responsePending: read again.
                    continue;
                }
                let echoed = *resp.get(1).unwrap_or(&sid);
                return Err(UdsError::NegativeResponse { sid: echoed, nrc });
            }
            if first != sid + 0x40 {
                return Err(UdsError::Malformed(format!(
                    "response SID 0x{first:02X} does not match request 0x{:02X}",
                    sid + 0x40
                )));
            }
            return Ok(resp[1..].to_vec());
        }
        Err(UdsError::Malformed("too many responsePending replies".into()))
    }

    pub fn read_data_by_identifier(&mut self, did: u16) -> Result<Vec<u8>, UdsError> {
        let resp = self.request(0x22, &[(did >> 8) as u8, (did & 0xFF) as u8])?;
        let echoed = resp
            .get(0..2)
            .ok_or_else(|| UdsError::Malformed("RDBI response missing DID echo".into()))?;
        if echoed != [(did >> 8) as u8, (did & 0xFF) as u8] {
            return Err(UdsError::Malformed(format!(
                "RDBI DID echo mismatch: got {echoed:02X?}, want 0x{did:04X}"
            )));
        }
        Ok(resp[2..].to_vec())
    }

    pub fn tester_present(&mut self) -> Result<(), UdsError> {
        self.request(0x3E, &[0x00])?;
        Ok(())
    }

    pub fn start_session(&mut self, session: u8) -> Result<(), UdsError> {
        self.request(0x10, &[session])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Mock IsoTp channel: canned responses, records sent PDUs.
    struct MockChannel {
        replies: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
    }
    impl MockChannel {
        fn new(replies: Vec<Vec<u8>>) -> Self {
            MockChannel { replies: replies.into(), sent: Vec::new() }
        }
    }
    impl IsoTpTransport for MockChannel {
        fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
            self.sent.push(data.to_vec());
            Ok(())
        }
        fn recv(&mut self, _t: Duration) -> Result<Vec<u8>, TransportError> {
            self.replies.pop_front().ok_or(TransportError::Timeout)
        }
    }

    #[test]
    fn positive_response_strips_sid() {
        let ch = MockChannel::new(vec![vec![0x62, 0xF1, 0x90, 0xAB]]); // resp to 0x22
        let mut uds = UdsClient::new(ch);
        let data = uds.request(0x22, &[0xF1, 0x90]).unwrap();
        assert_eq!(data, vec![0xF1, 0x90, 0xAB]);
    }

    #[test]
    fn negative_response_surfaces_nrc() {
        let ch = MockChannel::new(vec![vec![0x7F, 0x22, 0x31]]); // requestOutOfRange
        let mut uds = UdsClient::new(ch);
        let err = uds.request(0x22, &[0x00, 0x00]).unwrap_err();
        assert!(matches!(err, UdsError::NegativeResponse { sid: 0x22, nrc: 0x31 }));
    }

    #[test]
    fn response_pending_then_final() {
        let ch = MockChannel::new(vec![
            vec![0x7F, 0x22, 0x78], // pending
            vec![0x7F, 0x22, 0x78], // pending
            vec![0x62, 0xF1, 0x90], // final positive
        ]);
        let mut uds = UdsClient::new(ch);
        let data = uds.request(0x22, &[0xF1, 0x90]).unwrap();
        assert_eq!(data, vec![0xF1, 0x90]);
    }

    #[test]
    fn write_service_is_forbidden() {
        let ch = MockChannel::new(vec![]);
        let mut uds = UdsClient::new(ch);
        let err = uds.request(0x2E, &[0xF1, 0x90, 0x01]).unwrap_err();
        assert!(matches!(err, UdsError::Forbidden(0x2E)));
    }

    #[test]
    fn rdbi_validates_did_echo_and_returns_payload() {
        // Request DID 0xF190; response 0x62 F1 90 <data...>
        let ch = MockChannel::new(vec![vec![0x62, 0xF1, 0x90, b'W', b'V', b'W']]);
        let mut uds = UdsClient::new(ch);
        let data = uds.read_data_by_identifier(0xF190).unwrap();
        assert_eq!(data, vec![b'W', b'V', b'W']);
    }

    #[test]
    fn rdbi_rejects_wrong_did_echo() {
        let ch = MockChannel::new(vec![vec![0x62, 0xF1, 0x91, 0x00]]); // wrong DID echoed
        let mut uds = UdsClient::new(ch);
        let err = uds.read_data_by_identifier(0xF190).unwrap_err();
        assert!(matches!(err, UdsError::Malformed(_)));
    }

    #[test]
    fn tester_present_ok() {
        let ch = MockChannel::new(vec![vec![0x7E, 0x00]]);
        let mut uds = UdsClient::new(ch);
        uds.tester_present().unwrap();
    }

    #[test]
    fn start_session_ok() {
        let ch = MockChannel::new(vec![vec![0x50, 0x03, 0, 0x32, 0x01, 0xF4]]);
        let mut uds = UdsClient::new(ch);
        uds.start_session(0x03).unwrap();
    }
}
