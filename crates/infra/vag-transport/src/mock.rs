use crate::traits::RawCanTransport;
use crate::{CanFrame, TransportError};
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::time::Duration;

#[cfg(test)]
use crate::CanId;

#[derive(Debug, Clone)]
pub enum ScriptStep {
	/// Assert the next frame the code-under-test sends equals this frame.
	ExpectSend(CanFrame),
	/// The next `recv_frame` returns this frame.
	Reply(CanFrame),
}

/// Deterministic mock: replays a scripted sequence of expected sends and canned replies.
pub struct ScriptedCan {
	steps: VecDeque<ScriptStep>,
	sent: Vec<CanFrame>,
}

impl ScriptedCan {
	pub fn new(steps: Vec<ScriptStep>) -> Self {
		ScriptedCan {
			steps: steps.into(),
			sent: Vec::new(),
		}
	}
	pub fn sent(&self) -> &[CanFrame] {
		&self.sent
	}
}

impl RawCanTransport for ScriptedCan {
	fn send_frame(&mut self, frame: &CanFrame) -> Result<(), TransportError> {
		match self.steps.pop_front() {
			Some(ScriptStep::ExpectSend(expected)) => {
				assert_eq!(*frame, expected, "unexpected frame sent by code under test");
				self.sent.push(frame.clone());
				Ok(())
			}
			other => panic!("send_frame called but next script step was {other:?}"),
		}
	}

	fn recv_frame(&mut self, _timeout: Duration) -> Result<CanFrame, TransportError> {
		match self.steps.pop_front() {
			Some(ScriptStep::Reply(frame)) => Ok(frame),
			None => Err(TransportError::Timeout),
			other => panic!("recv_frame called but next script step was {other:?}"),
		}
	}
}

/// Deterministic async mock: scripted request→response PDU pairs, so upper
/// layers (uds-async, cable-actor) can be tested with no hardware.
///
/// `send` must match the next scripted request (panics with a diff otherwise)
/// and queues its paired response; `recv` returns the next queued response, or
/// `TransportError::Timeout` when nothing is pending.
#[cfg(any(test, feature = "test-util"))]
pub struct MockAsyncTransport {
	script: VecDeque<(Vec<u8>, Vec<u8>)>,
	pending: VecDeque<Vec<u8>>,
	sent: Vec<Vec<u8>>,
}

#[cfg(any(test, feature = "test-util"))]
impl MockAsyncTransport {
	/// `script`: ordered (expected request PDU, canned response PDU) pairs.
	pub fn new(script: Vec<(Vec<u8>, Vec<u8>)>) -> Self {
		MockAsyncTransport {
			script: script.into(),
			pending: VecDeque::new(),
			sent: Vec::new(),
		}
	}

	/// Every PDU the code under test has sent, in order.
	pub fn sent(&self) -> &[Vec<u8>] {
		&self.sent
	}

	/// True when the whole script was consumed and no response is still pending.
	pub fn is_exhausted(&self) -> bool {
		self.script.is_empty() && self.pending.is_empty()
	}
}

#[cfg(any(test, feature = "test-util"))]
impl crate::traits::AsyncIsoTpTransport for MockAsyncTransport {
	async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError> {
		match self.script.pop_front() {
			Some((expected, response)) => {
				assert_eq!(pdu, expected.as_slice(), "unexpected PDU sent by code under test");
				self.sent.push(pdu.to_vec());
				self.pending.push_back(response);
				Ok(())
			}
			None => panic!("send called but the script is exhausted (pdu: {pdu:02X?})"),
		}
	}

	async fn recv(&mut self, _timeout: Duration) -> Result<Vec<u8>, TransportError> {
		self.pending.pop_front().ok_or(TransportError::Timeout)
	}
}

#[cfg(test)]
mod async_tests {
	use super::*;
	use crate::traits::AsyncIsoTpTransport;

	#[tokio::test]
	async fn mock_replays_scripted_request_response_pair() {
		let req = vec![0x22, 0xF1, 0x90];
		let resp = vec![0x62, 0xF1, 0x90, b'W', b'V', b'W'];
		let mut t = MockAsyncTransport::new(vec![(req.clone(), resp.clone())]);

		t.send(&req).await.unwrap();
		let got = t.recv(Duration::from_millis(10)).await.unwrap();

		assert_eq!(got, resp);
		assert_eq!(t.sent(), &[req]);
	}

	#[tokio::test]
	async fn recv_with_empty_script_times_out() {
		let mut t = MockAsyncTransport::new(vec![]);
		let err = t.recv(Duration::from_millis(5)).await.unwrap_err();
		assert!(matches!(err, TransportError::Timeout), "got {err:?}");
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn scripted_can_expects_send_then_replies() {
		let tx = CanFrame::new(CanId::Standard(0x7E0), vec![0x02, 0x3E, 0x00]);
		let rx = CanFrame::new(CanId::Standard(0x7E8), vec![0x02, 0x7E, 0x00]);
		let mut can = ScriptedCan::new(vec![ScriptStep::ExpectSend(tx.clone()), ScriptStep::Reply(rx.clone())]);
		can.send_frame(&tx).unwrap();
		let got = can.recv_frame(Duration::from_millis(10)).unwrap();
		assert_eq!(got, rx);
		assert_eq!(can.sent(), &[tx]);
	}
}
