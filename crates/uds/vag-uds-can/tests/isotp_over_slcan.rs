//! End-to-end, hardware-free: a UDS VIN read (ReadDataByIdentifier 0xF190)
//! through `IsoTpCan<SlcanBackend<duplex>>`, with a fake ECU on the far end of
//! the in-memory stream speaking slcan + ISO-TP.

use std::time::Duration;
use vag_uds_can::{IsoTpCan, SlcanBackend};
use vag_uds_transport::AsyncIsoTpTransport;

const VIN: &[u8] = b"WVWZZZ1KZAW000001";

#[tokio::test]
async fn uds_vin_read_over_slcan_duplex() {
	let (tester_side, ecu_side) = tokio::io::duplex(1024);
	let mut iso = IsoTpCan::for_ecu(SlcanBackend::new(tester_side), 0);

	// Fake ECU: uses the slcan backend directly as its bus access.
	let ecu = tokio::spawn(async move {
		use vag_uds_can::CanBackend;
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

/// The laptop's path, the same rule: the tail of an earlier answer — consecutive frames the
/// tester stopped waiting for — comes before the ECU's answer to this request, and is ignored
/// (ISO 15765-2) rather than failing the exchange.
#[tokio::test]
async fn an_earlier_answers_tail_before_this_answer_is_ignored_over_slcan() {
	let (tester_side, ecu_side) = tokio::io::duplex(1024);
	let mut iso = IsoTpCan::for_ecu(SlcanBackend::new(tester_side), 0);
	let ecu = tokio::spawn(async move {
		use vag_uds_can::CanBackend;
		let mut bus = SlcanBackend::new(ecu_side);
		let (id, _) = bus.recv_frame(Duration::from_secs(1)).await.unwrap();
		assert_eq!(id, 0x7E0);
		bus.send_frame(0x7E8, &[0x23, 1, 2, 3, 4, 5, 6, 7]).await.unwrap();
		bus.send_frame(0x7E8, &[0x24, 8, 9, 10, 11, 12, 13, 14]).await.unwrap();
		bus.send_frame(0x7E8, &[0x04, 0x62, 0xF1, 0x87, b'P', 0, 0, 0]).await.unwrap();
	});
	iso.send(&[0x22, 0xF1, 0x87]).await.unwrap();
	let resp = iso.recv(Duration::from_secs(1)).await.unwrap();
	assert_eq!(resp, [0x62, 0xF1, 0x87, b'P']);
	ecu.await.unwrap();
}

/// The send side over slcan: a leftover consecutive frame of an earlier answer, arriving while
/// the tester waits for the flow control of its own multi-frame request, is ignored.
#[tokio::test]
async fn an_earlier_answers_tail_before_the_flow_control_is_ignored_over_slcan() {
	let (tester_side, ecu_side) = tokio::io::duplex(1024);
	let mut iso = IsoTpCan::for_ecu(SlcanBackend::new(tester_side), 0);
	let request: Vec<u8> = vec![0x22, 0x10, 0x00, 0x10, 0x01, 0x10, 0x02, 0x10, 0x03];
	let expected = request.clone();
	let ecu = tokio::spawn(async move {
		use vag_uds_can::CanBackend;
		let mut bus = SlcanBackend::new(ecu_side);
		let t = Duration::from_secs(1);
		let (_, ff) = bus.recv_frame(t).await.unwrap();
		assert_eq!(&ff[..2], &[0x10, 0x09]);
		bus.send_frame(0x7E8, &[0x23, 1, 2, 3, 4, 5, 6, 7]).await.unwrap();
		bus.send_frame(0x7E8, &[0x30, 0x00, 0x00, 0, 0, 0, 0, 0]).await.unwrap();
		let (_, cf) = bus.recv_frame(t).await.unwrap();
		assert_eq!(cf[0], 0x21);
		let mut got = ff[2..8].to_vec();
		got.extend_from_slice(&cf[1..4]);
		assert_eq!(got, expected);
	});
	iso.send(&request).await.unwrap();
	ecu.await.unwrap();
}
