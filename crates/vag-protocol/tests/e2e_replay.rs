use vag_capture::{CapturePayload, CaptureRecord, Direction, ReplayCan};
use vag_protocol::{SoftwareIsoTp, UdsClient};
use vag_transport::CanId;

fn tx(id: u16, data: Vec<u8>) -> CaptureRecord {
    CaptureRecord { ts_us: 0, dir: Direction::Tx, payload: CapturePayload::CanFrame { id: CanId::Standard(id), data } }
}
fn rx(id: u16, data: Vec<u8>) -> CaptureRecord {
    CaptureRecord { ts_us: 0, dir: Direction::Rx, payload: CapturePayload::CanFrame { id: CanId::Standard(id), data } }
}

/// Full stack: UdsClient -> SoftwareIsoTp -> ReplayCan.
/// Scenario: RDBI 0xF190 (VIN-like), multi-frame 17-byte response.
#[test]
fn rdbi_multiframe_over_replay() {
    // Request is a single frame: 0x03 0x22 0xF1 0x90 (padded).
    // Response is 20 bytes after SID: 0x62 F1 90 + 17 chars = 20 bytes -> multi-frame.
    // FF declares length 20: 0x10 0x14, then 6 bytes (0x62 F1 90 W V W)
    // FC from tester: 0x30 00 00
    // CFs carry the rest.
    let payload: Vec<u8> = {
        let mut v = vec![0x62, 0xF1, 0x90];
        v.extend_from_slice(b"WVWZZZ1KZAW000001"); // 17 bytes -> total 20
        v
    };
    assert_eq!(payload.len(), 20);

    let ff = rx(0x7E8, vec![0x10, 0x14, payload[0], payload[1], payload[2], payload[3], payload[4], payload[5]]);
    let fc = tx(0x7E0, vec![0x30, 0x00, 0x00, 0, 0, 0, 0, 0]);
    let cf1 = rx(0x7E8, vec![0x21, payload[6], payload[7], payload[8], payload[9], payload[10], payload[11], payload[12]]);
    let cf2 = rx(0x7E8, vec![0x22, payload[13], payload[14], payload[15], payload[16], payload[17], payload[18], payload[19]]);

    let req = tx(0x7E0, vec![0x03, 0x22, 0xF1, 0x90, 0, 0, 0, 0]);

    let records = vec![req, ff, fc, cf1, cf2];
    let can = ReplayCan::new(records);
    let iso = SoftwareIsoTp::new(can, CanId::Standard(0x7E0), CanId::Standard(0x7E8));
    let mut uds = UdsClient::new(iso);

    let data = uds.read_data_by_identifier(0xF190).unwrap();
    assert_eq!(&data, b"WVWZZZ1KZAW000001");
}
