use std::time::Duration;
use crate::{CanFrame, TransportError};
use crate::traits::RawCanTransport;

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
    steps: std::collections::VecDeque<ScriptStep>,
    sent: Vec<CanFrame>,
}

impl ScriptedCan {
    pub fn new(steps: Vec<ScriptStep>) -> Self {
        ScriptedCan { steps: steps.into(), sent: Vec::new() }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_can_expects_send_then_replies() {
        let tx = CanFrame::new(CanId::Standard(0x7E0), vec![0x02, 0x3E, 0x00]);
        let rx = CanFrame::new(CanId::Standard(0x7E8), vec![0x02, 0x7E, 0x00]);
        let mut can = ScriptedCan::new(vec![
            ScriptStep::ExpectSend(tx.clone()),
            ScriptStep::Reply(rx.clone()),
        ]);
        can.send_frame(&tx).unwrap();
        let got = can.recv_frame(Duration::from_millis(10)).unwrap();
        assert_eq!(got, rx);
        assert_eq!(can.sent(), &[tx]);
    }
}
