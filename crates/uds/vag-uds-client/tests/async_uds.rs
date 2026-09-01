//! Async UDS client over `AsyncIsoTpTransport`, tested with no hardware via
//! `MockAsyncTransport` (vag-uds-transport `test-util` feature).

use std::collections::VecDeque;
use std::time::Duration;

use vag_uds_client::{AsyncUdsClient, RawDtc, UdsError};
use vag_uds_transport::{AsyncIsoTpTransport, MockAsyncTransport, TransportError};

#[tokio::test]
async fn rdbi_reads_vin_from_scripted_mock() {
	let vin = b"WVWZZZ1KZAW000001";
	let mut resp = vec![0x62, 0xF1, 0x90];
	resp.extend_from_slice(vin);
	let mock = MockAsyncTransport::new(vec![(vec![0x22, 0xF1, 0x90], resp)]);

	let mut uds = AsyncUdsClient::new(mock);
	let data = uds.read_data_by_identifier(0xF190).await.unwrap();

	assert_eq!(data, vin);
	assert!(uds.transport().is_exhausted());
}

#[tokio::test]
async fn rdbi_rejects_wrong_did_echo() {
	let mock = MockAsyncTransport::new(vec![(
		vec![0x22, 0xF1, 0x90],
		vec![0x62, 0xF1, 0x91, 0x00], // wrong DID echoed
	)]);

	let mut uds = AsyncUdsClient::new(mock);
	let err = uds.read_data_by_identifier(0xF190).await.unwrap_err();

	assert!(matches!(err, UdsError::Malformed(_)), "got {err:?}");
}

#[tokio::test]
async fn read_dtcs_by_status_mask_round_trip() {
	let mock = MockAsyncTransport::new(vec![(
		vec![0x19, 0x02, 0xFF],
		vec![
			0x59, 0x02, 0xFF, // +SID, subfn echo, availability mask
			0x11, 0x22, 0x33, 0x08, // DTC 1
			0x44, 0x55, 0x66, 0x2F, // DTC 2
		],
	)]);

	let mut uds = AsyncUdsClient::new(mock);
	let dtcs = uds.read_dtcs_by_status_mask(0xFF).await.unwrap();

	assert_eq!(
		dtcs,
		vec![
			RawDtc {
				code: [0x11, 0x22, 0x33],
				status: 0x08
			},
			RawDtc {
				code: [0x44, 0x55, 0x66],
				status: 0x2F
			},
		]
	);
}

#[tokio::test]
async fn read_dtcs_empty_list() {
	let mock = MockAsyncTransport::new(vec![(vec![0x19, 0x02, 0xFF], vec![0x59, 0x02, 0xFF])]);

	let mut uds = AsyncUdsClient::new(mock);
	let dtcs = uds.read_dtcs_by_status_mask(0xFF).await.unwrap();

	assert!(dtcs.is_empty());
}

#[tokio::test]
async fn write_service_is_forbidden_without_touching_transport() {
	let mock = MockAsyncTransport::new(vec![]); // any send would panic the mock

	let mut uds = AsyncUdsClient::new(mock);
	let err = uds.request(0x2E, &[0xF1, 0x90, 0x01]).await.unwrap_err();

	assert!(matches!(err, UdsError::Forbidden(0x2E)), "got {err:?}");
	assert!(uds.transport().sent().is_empty(), "transport must not be touched");
}

#[tokio::test]
async fn tester_present_round_trip() {
	let mock = MockAsyncTransport::new(vec![(vec![0x3E, 0x00], vec![0x7E, 0x00])]);

	let mut uds = AsyncUdsClient::new(mock);
	uds.tester_present().await.unwrap();

	assert!(uds.transport().is_exhausted());
}

#[tokio::test]
async fn start_session_round_trip() {
	let mock = MockAsyncTransport::new(vec![(vec![0x10, 0x03], vec![0x50, 0x03, 0x00, 0x32, 0x01, 0xF4])]);

	let mut uds = AsyncUdsClient::new(mock);
	uds.start_session(0x03).await.unwrap();
}

#[tokio::test]
async fn negative_response_surfaces_nrc() {
	let mock = MockAsyncTransport::new(vec![(
		vec![0x22, 0x00, 0x00],
		vec![0x7F, 0x22, 0x31], // requestOutOfRange
	)]);

	let mut uds = AsyncUdsClient::new(mock);
	let err = uds.request(0x22, &[0x00, 0x00]).await.unwrap_err();

	assert!(matches!(err, UdsError::NegativeResponse { sid: 0x22, nrc: 0x31 }), "got {err:?}");
}

/// Local mock able to deliver several PDUs for one send — `MockAsyncTransport`
/// pairs each send with exactly one response, which can't script responsePending.
struct MultiReplyMock {
	replies: VecDeque<Vec<u8>>,
	sent: Vec<Vec<u8>>,
}

impl AsyncIsoTpTransport for MultiReplyMock {
	async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError> {
		self.sent.push(pdu.to_vec());
		Ok(())
	}
	async fn recv(&mut self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
		self.replies.pop_front().ok_or(TransportError::Timeout)
	}
}

#[tokio::test]
async fn response_pending_then_final() {
	let mock = MultiReplyMock {
		replies: VecDeque::from(vec![
			vec![0x7F, 0x22, 0x78], // responsePending
			vec![0x7F, 0x22, 0x78], // responsePending
			vec![0x62, 0xF1, 0x90, 0xAB],
		]),
		sent: Vec::new(),
	};

	let mut uds = AsyncUdsClient::new(mock);
	let data = uds.request(0x22, &[0xF1, 0x90]).await.unwrap();

	assert_eq!(data, vec![0xF1, 0x90, 0xAB]);
	assert_eq!(uds.transport().sent.len(), 1, "pending must not re-send the request");
}
