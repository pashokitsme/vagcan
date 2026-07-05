//! Transport-agnostic UDS PDU encoding/decoding shared by the sync and async
//! clients. Pure functions over byte slices — no I/O, no timing.

use std::time::Duration;

use crate::dtc::RawDtc;
use crate::uds::UdsError;

/// How long each client waits for one response PDU.
pub(crate) const RESPONSE_TIMEOUT: Duration = Duration::from_millis(2000);
/// Max NRC 0x78 (responsePending) replies tolerated before giving up.
pub(crate) const MAX_PENDING: usize = 30;
/// Services this stack will ever emit. Everything else → `UdsError::Forbidden`.
const READ_ONLY_ALLOWLIST: &[u8] = &[0x10, 0x19, 0x22, 0x3E];

/// Encode `[sid, payload...]`, rejecting services outside the read-only allowlist.
pub(crate) fn encode_request(sid: u8, payload: &[u8]) -> Result<Vec<u8>, UdsError> {
    if !READ_ONLY_ALLOWLIST.contains(&sid) {
        return Err(UdsError::Forbidden(sid));
    }
    let mut req = Vec::with_capacity(1 + payload.len());
    req.push(sid);
    req.extend_from_slice(payload);
    Ok(req)
}

/// One classified response PDU.
pub(crate) enum Classified {
    /// NRC 0x78 — the ECU asks for more time; read again without re-sending.
    Pending,
    /// Positive response: the bytes after the echoed SID.
    Data(Vec<u8>),
}

/// Classify one response PDU for a request with service `sid`.
pub(crate) fn classify_response(sid: u8, resp: &[u8]) -> Result<Classified, UdsError> {
    let first = *resp
        .first()
        .ok_or_else(|| UdsError::Malformed("empty response".into()))?;
    if first == 0x7F {
        // Negative: [0x7F, sid, nrc]
        let nrc = *resp
            .get(2)
            .ok_or_else(|| UdsError::Malformed("short negative response".into()))?;
        if nrc == 0x78 {
            return Ok(Classified::Pending);
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
    Ok(Classified::Data(resp[1..].to_vec()))
}

/// The two big-endian payload bytes of a DID.
pub(crate) fn did_bytes(did: u16) -> [u8; 2] {
    [(did >> 8) as u8, (did & 0xFF) as u8]
}

/// Validate the RDBI DID echo and return the data bytes after it.
pub(crate) fn parse_rdbi_response(did: u16, resp: &[u8]) -> Result<Vec<u8>, UdsError> {
    let echoed = resp
        .get(0..2)
        .ok_or_else(|| UdsError::Malformed("RDBI response missing DID echo".into()))?;
    if echoed != did_bytes(did) {
        return Err(UdsError::Malformed(format!(
            "RDBI DID echo mismatch: got {echoed:02X?}, want 0x{did:04X}"
        )));
    }
    Ok(resp[2..].to_vec())
}

/// Parse a ReadDTCInformation 0x02 response body (after SID strip):
/// `0x02 <availability mask> [code(3) status(1)]*`.
pub(crate) fn parse_dtc_response(resp: &[u8]) -> Result<Vec<RawDtc>, UdsError> {
    if resp.len() < 2 || resp[0] != 0x02 {
        return Err(UdsError::Malformed("bad ReadDTCInformation 0x02 response".into()));
    }
    let entries = &resp[2..];
    if entries.len() % 4 != 0 {
        return Err(UdsError::Malformed("DTC entries not a multiple of 4 bytes".into()));
    }
    let mut out = Vec::with_capacity(entries.len() / 4);
    for chunk in entries.chunks_exact(4) {
        out.push(RawDtc { code: [chunk[0], chunk[1], chunk[2]], status: chunk[3] });
    }
    Ok(out)
}
