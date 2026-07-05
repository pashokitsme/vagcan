//! Async UDS client over `vag_transport::AsyncIsoTpTransport`.
//!
//! Mirrors the sync `UdsClient` exactly — same read-only allowlist, same
//! responsePending handling, same parsing — via the shared `pdu` helpers.
//! ISO-TP framing stays BELOW the trait: `AsyncIsoTpTransport` sends/receives
//! whole PDUs (in the connection-actor model the `CableActor` owns segmentation),
//! so this client works purely at the PDU level, like the sync design.

use vag_transport::AsyncIsoTpTransport;

use crate::dtc::RawDtc;
use crate::pdu::{self, Classified, MAX_PENDING, RESPONSE_TIMEOUT};
use crate::uds::UdsError;

/// Async UDS client, generic over the transport (static dispatch, no `dyn`).
pub struct AsyncUdsClient<T: AsyncIsoTpTransport> {
    channel: T,
}

impl<T: AsyncIsoTpTransport> AsyncUdsClient<T> {
    pub fn new(channel: T) -> Self {
        AsyncUdsClient { channel }
    }

    /// Borrow the underlying transport (tests inspect mocks through this).
    pub fn transport(&self) -> &T {
        &self.channel
    }

    /// Consume the client, returning the transport.
    pub fn into_transport(self) -> T {
        self.channel
    }

    /// Send a UDS request; return response bytes after the echoed SID.
    ///
    /// Services outside the read-only allowlist `{0x10, 0x19, 0x22, 0x3E}` are
    /// rejected with [`UdsError::Forbidden`] before touching the transport.
    pub async fn request(&mut self, sid: u8, payload: &[u8]) -> Result<Vec<u8>, UdsError> {
        let req = pdu::encode_request(sid, payload)?;
        self.channel.send(&req).await?;

        for _ in 0..MAX_PENDING {
            let resp = self.channel.recv(RESPONSE_TIMEOUT).await?;
            match pdu::classify_response(sid, &resp)? {
                Classified::Pending => continue, // responsePending: read again.
                Classified::Data(data) => return Ok(data),
            }
        }
        Err(UdsError::Malformed("too many responsePending replies".into()))
    }

    /// ReadDataByIdentifier (0x22): returns the record bytes after the DID echo.
    pub async fn read_data_by_identifier(&mut self, did: u16) -> Result<Vec<u8>, UdsError> {
        let resp = self.request(0x22, &pdu::did_bytes(did)).await?;
        pdu::parse_rdbi_response(did, &resp)
    }

    /// TesterPresent (0x3E 0x00).
    pub async fn tester_present(&mut self) -> Result<(), UdsError> {
        self.request(0x3E, &[0x00]).await?;
        Ok(())
    }

    /// DiagnosticSessionControl (0x10 `session`).
    pub async fn start_session(&mut self, session: u8) -> Result<(), UdsError> {
        self.request(0x10, &[session]).await?;
        Ok(())
    }

    /// ReadDTCInformation (0x19 0x02 `mask`): DTCs matching the status mask.
    pub async fn read_dtcs_by_status_mask(&mut self, mask: u8) -> Result<Vec<RawDtc>, UdsError> {
        let resp = self.request(0x19, &[0x02, mask]).await?;
        pdu::parse_dtc_response(&resp)
    }
}
