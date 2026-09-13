//! [`UnitLink`] from outside the crate, which is where the BLE link will be.
//!
//! An integration test is its own crate, so `PduLink` below is in exactly the
//! position a BLE transport in another crate is: a local type implementing
//! `UnitLink` directly, beside the blanket impl every `CanBackend` gets. That
//! this file compiles is the coherence half; the tests are the contract — one
//! generic exchange, run over both kinds of link, addresses the unit it was
//! told to and hands the link back.

use std::collections::VecDeque;
use std::time::Duration;

use vag_uds_can::{CanBackend, CanError, UnitLink};
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

/// A link that carries whole PDUs and never sees a frame.
#[derive(Default)]
struct PduLink {
	/// Every PDU sent, with the request id it was addressed to.
	sent: Vec<(CanId, Vec<u8>)>,
	/// Answers handed out in order, each tagged with the id it came from.
	answers: VecDeque<(CanId, Vec<u8>)>,
}

/// `PduLink` while addressed to one unit.
struct PduChannel {
	link: PduLink,
	request: CanId,
	response: CanId,
}

impl UnitLink for PduLink {
	type Channel = PduChannel;

	fn to_unit(self, request: CanId, response: CanId) -> PduChannel {
		PduChannel {
			link: self,
			request,
			response,
		}
	}

	fn release(channel: PduChannel) -> PduLink {
		channel.link
	}
}

impl AsyncIsoTpTransport for PduChannel {
	async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError> {
		self.link.sent.push((self.request, pdu.to_vec()));
		Ok(())
	}

	async fn recv(&mut self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
		// An answer from another unit is not this channel's to take.
		match self.link.answers.pop_front() {
			Some((from, pdu)) if from == self.response => Ok(pdu),
			_ => Err(TransportError::Timeout),
		}
	}
}

/// A CAN bus with one unit on it that answers every single frame with a fixed one.
#[derive(Default)]
struct EchoBus {
	sent: Vec<(u32, Vec<u8>)>,
	pending: VecDeque<(u32, Vec<u8>)>,
}

impl CanBackend for EchoBus {
	async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), CanError> {
		self.sent.push((id, data.to_vec()));
		// `62 F1 90 01`, as a single frame on the response id eight above.
		self.pending.push_back((id + 8, vec![0x04, 0x62, 0xF1, 0x90, 0x01, 0, 0, 0]));
		Ok(())
	}

	async fn recv_frame(&mut self, _timeout: Duration) -> Result<(u32, Vec<u8>), CanError> {
		self.pending.pop_front().ok_or(CanError::Timeout)
	}
}

/// What a command does: address one unit, exchange, let go.
async fn exchange<L: UnitLink>(link: L, request: u16, pdu: &[u8]) -> (L, Result<Vec<u8>, TransportError>) {
	let mut channel = link.to_unit(CanId::Standard(request), CanId::Standard(request + 8));
	let answer = match channel.send(pdu).await {
		Ok(()) => channel.recv(Duration::from_millis(10)).await,
		Err(e) => Err(e),
	};
	(L::release(channel), answer)
}

#[tokio::test]
async fn a_pdu_link_is_addressed_per_unit_and_handed_back() {
	let link = PduLink {
		answers: VecDeque::from([(CanId::Standard(0x7E8), vec![0x62, 0xF1, 0x90, 0x01])]),
		..PduLink::default()
	};
	let (link, answer) = exchange(link, 0x7E0, &[0x22, 0xF1, 0x90]).await;
	assert_eq!(answer.unwrap(), [0x62, 0xF1, 0x90, 0x01]);

	// The same value, re-addressed: nothing is queued for 0x7E9, so it times out.
	let (link, answer) = exchange(link, 0x7E1, &[0x22, 0xF1, 0x90]).await;
	assert!(matches!(answer, Err(TransportError::Timeout)), "{answer:?}");

	assert_eq!(
		link.sent,
		[
			(CanId::Standard(0x7E0), vec![0x22, 0xF1, 0x90]),
			(CanId::Standard(0x7E1), vec![0x22, 0xF1, 0x90])
		]
	);
}

#[tokio::test]
async fn a_can_backend_is_a_link_through_isotp() {
	let (bus, answer) = exchange(EchoBus::default(), 0x7E0, &[0x22, 0xF1, 0x90]).await;
	assert_eq!(answer.unwrap(), [0x62, 0xF1, 0x90, 0x01]);
	// Released back to the bare backend: the frame it sent is ISO-TP's single
	// frame, padded to eight bytes, on the request id.
	assert_eq!(bus.sent, [(0x7E0, vec![0x03, 0x22, 0xF1, 0x90, 0, 0, 0, 0])]);
}
