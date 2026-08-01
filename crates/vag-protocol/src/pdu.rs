//! Transport-agnostic UDS PDU encoding/decoding shared by the sync and async
//! clients. Pure functions over byte slices — no I/O, no timing.

use std::time::Duration;

use crate::dtc::{DtcExtendedData, DtcSnapshot, RawDtc};
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
    parse_dtc_list(resp, 0x02)
}

/// The same framing for any subfunction that answers with a status mask
/// followed by `[code(3) status(1)]` records — `0x02` by status mask and
/// `0x0A` for the unit's whole supported list.
pub(crate) fn parse_dtc_list(resp: &[u8], subfunction: u8) -> Result<Vec<RawDtc>, UdsError> {
    if resp.len() < 2 || resp[0] != subfunction {
        return Err(UdsError::Malformed("bad ReadDTCInformation response".into()));
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

/// Parse a ReadDTCInformation 0x04 response body (after SID strip):
/// `0x04 code(3) status(1) [record(1) count(1) [did(2) data]*]*`.
///
/// The per-identifier lengths are **not** in the response — a reader is
/// expected to know them from the unit's own description. Without that, the
/// only honest parse is: record number, then the remaining bytes of that
/// record. So the identifiers are read where the count makes them
/// unambiguous, and the rest is kept whole.
pub(crate) fn parse_dtc_snapshot(resp: &[u8]) -> Result<Vec<DtcSnapshot>, UdsError> {
    if resp.len() < 5 || resp[0] != 0x04 {
        return Err(UdsError::Malformed("bad ReadDTCInformation 0x04 response".into()));
    }
    // resp[1..4] is the DTC the caller asked about, resp[4] its status.
    let mut out = Vec::new();
    let mut at = 5;
    while at + 1 < resp.len() {
        let record = resp[at];
        let count = resp[at + 1] as usize;
        at += 2;
        let mut values = Vec::new();
        for _ in 0..count {
            if at + 2 > resp.len() {
                break;
            }
            let did = u16::from_be_bytes([resp[at], resp[at + 1]]);
            at += 2;
            // Everything left belongs to this record; without per-identifier
            // lengths there is no way to split further, and inventing a split
            // would misreport the values.
            let rest = resp[at..].to_vec();
            at = resp.len();
            values.push((did, rest));
        }
        out.push(DtcSnapshot { record, values });
    }
    Ok(out)
}

/// Parse a ReadDTCInformation 0x06 response body (after SID strip):
/// `0x06 code(3) status(1) [record(1) data]*`.
///
/// Record boundaries are per-unit, so each record is returned whole rather
/// than split into fields this project has not established.
pub(crate) fn parse_dtc_extended(resp: &[u8]) -> Result<Vec<DtcExtendedData>, UdsError> {
    if resp.len() < 5 || resp[0] != 0x06 {
        return Err(UdsError::Malformed("bad ReadDTCInformation 0x06 response".into()));
    }
    let mut out = Vec::new();
    let mut at = 5;
    while at < resp.len() {
        let record = resp[at];
        at += 1;
        let data = resp[at..].to_vec();
        at = resp.len();
        out.push(DtcExtendedData { record, data });
    }
    Ok(out)
}

#[cfg(test)]
mod dtc_detail_tests {
    use super::*;

    #[test]
    fn a_snapshot_keeps_its_record_number_and_the_identifier_it_captured() {
        // 0x04, the code asked about, its status, then record 01 holding one
        // captured identifier 0x1234.
        let resp = [0x04, 0x00, 0x01, 0x07, 0x08, 0x01, 0x01, 0x12, 0x34, 0xAA, 0xBB];
        let snaps = parse_dtc_snapshot(&resp).unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].record, 0x01);
        assert_eq!(snaps[0].values, vec![(0x1234, vec![0xAA, 0xBB])]);
    }

    #[test]
    fn extended_data_is_returned_whole_rather_than_split_into_guessed_fields() {
        // Record boundaries inside extended data are per-unit. Splitting them
        // on a guess would report an occurrence count nobody established.
        let resp = [0x06, 0x00, 0x01, 0x07, 0x08, 0x01, 0x03, 0x00, 0x12];
        let ext = parse_dtc_extended(&resp).unwrap();
        assert_eq!(ext.len(), 1);
        assert_eq!(ext[0].record, 0x01);
        assert_eq!(ext[0].data, vec![0x03, 0x00, 0x12]);
    }

    #[test]
    fn a_response_for_the_wrong_subfunction_is_rejected() {
        assert!(parse_dtc_snapshot(&[0x06, 0, 0, 0, 0]).is_err());
        assert!(parse_dtc_extended(&[0x04, 0, 0, 0, 0]).is_err());
        assert!(parse_dtc_snapshot(&[0x04]).is_err());
    }
}
