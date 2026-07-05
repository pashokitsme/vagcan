//! End-to-end, hardware-free: a UDS VIN read (ReadDataByIdentifier 0xF190)
//! through `IsoTpCan<SlcanBackend<duplex>>`, with a fake ECU on the far end of
//! the in-memory stream speaking slcan + ISO-TP.

use std::time::Duration;
use vag_can::{IsoTpCan, SlcanBackend};
use vag_transport::AsyncIsoTpTransport;

const VIN: &[u8] = b"WVWZZZ1KZAW000001";

#[tokio::test]
async fn uds_vin_read_over_slcan_duplex() {
    let (tester_side, ecu_side) = tokio::io::duplex(1024);
    let mut iso = IsoTpCan::for_ecu(SlcanBackend::new(tester_side), 0);

    // Fake ECU: uses the slcan backend directly as its bus access.
    let ecu = tokio::spawn(async move {
        use vag_can::CanBackend;
        let mut bus = SlcanBackend::new(ecu_side);
        let t = Duration::from_secs(1);

        // Expect the single-frame request 22 F1 90.
        let (id, data) = bus.recv_frame(t).await.unwrap();
        assert_eq!(id, 0x7E0);
        assert_eq!(&data[..4], &[0x03, 0x22, 0xF1, 0x90]);

        // Respond multi-frame: 62 F1 90 + 17-byte VIN = 20 bytes.
        let mut pdu = vec![0x62, 0xF1, 0x90];
        pdu.extend_from_slice(VIN);
        assert_eq!(pdu.len(), 20);

        let mut ff = vec![0x10, 0x14];
        ff.extend_from_slice(&pdu[..6]);
        bus.send_frame(0x7E8, &ff).await.unwrap();

        // Wait for the tester's flow control (CTS).
        let (fc_id, fc) = bus.recv_frame(t).await.unwrap();
        assert_eq!(fc_id, 0x7E0);
        assert_eq!(fc[0], 0x30);

        let mut cf1 = vec![0x21];
        cf1.extend_from_slice(&pdu[6..13]);
        bus.send_frame(0x7E8, &cf1).await.unwrap();
        let mut cf2 = vec![0x22];
        cf2.extend_from_slice(&pdu[13..20]);
        bus.send_frame(0x7E8, &cf2).await.unwrap();
    });

    iso.send(&[0x22, 0xF1, 0x90]).await.unwrap();
    let resp = iso.recv(Duration::from_secs(1)).await.unwrap();

    assert_eq!(&resp[..3], &[0x62, 0xF1, 0x90]);
    assert_eq!(&resp[3..], VIN);
    ecu.await.unwrap();
}
