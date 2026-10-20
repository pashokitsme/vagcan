//! The car's side of ISO-TP (ISO 15765-2): requests reassembled from frames, answers cut
//! into frames, flow control both ways, every unit on its own ids.
//!
//! Its own implementation, not `vag_uds_can::IsoTpCan` run backwards: the sim is the
//! client's counterpart in a test, and a defect shared by both ends would pass between
//! them unseen. It is a state machine over a clock it is handed, so every timing is a
//! test with no sleep in it.

use std::collections::VecDeque;
use std::time::Duration;

use crate::profile::{BusSettings, Car};
use crate::server::{Reply, Server, Why};

/// How long a sent first frame waits for the tester's flow control (ISO 15765-2 N_Bs).
const FLOW_CONTROL_WAIT: Duration = Duration::from_millis(1000);
/// How long a request's consecutive frames may be apart (ISO 15765-2 N_Cr).
const CONSECUTIVE_WAIT: Duration = Duration::from_millis(1000);

/// One classic CAN frame, eight bytes, padded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
	pub id: u16,
	pub data: [u8; 8],
}

/// What happened, for the sim's log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
	/// A whole request reached a unit.
	Request { unit: u16, pdu: Vec<u8> },
	/// A whole answer left a unit, on `id`, its answer id: its first frame went out.
	Answer { unit: u16, id: u16, pdu: Vec<u8> },
	/// A unit said nothing to a request, and why.
	Silent { unit: u16, why: Why },
	/// A frame that broke the protocol, or a transfer given up.
	Protocol { unit: u16, what: String },
}

/// Every unit of the car on one bus.
pub struct Bus {
	server: Server,
	settings: BusSettings,
	ends: Vec<End>,
	/// When the channel opened: a series' clock.
	opened: Duration,
	pub events: Vec<Event>,
}

/// One unit's ISO-TP state.
#[derive(Default)]
struct End {
	request: u16,
	response: u16,
	rx: Option<Rx>,
	/// Whole answers waiting their turn, each with when it may go.
	outbox: VecDeque<(Duration, Vec<u8>)>,
	tx: Option<Tx>,
}

struct Rx {
	len: usize,
	data: Vec<u8>,
	next_seq: u8,
	/// Consecutive frames left before the sim sends flow control again; `None` is all of them.
	left_in_block: Option<u8>,
	deadline: Duration,
}

struct Tx {
	pdu: Vec<u8>,
	offset: usize,
	seq: u8,
	state: TxState,
}

enum TxState {
	AwaitFlowControl {
		until: Duration,
	},
	Sending {
		next: Duration,
		left_in_block: Option<u8>,
		gap: Duration,
	},
}

impl Bus {
	pub fn new(car: Car) -> Bus {
		let ends = car
			.units
			.iter()
			.map(|u| End {
				request: u.address.request,
				response: u.address.response,
				..End::default()
			})
			.collect();
		let settings = car.bus;
		Bus {
			server: Server::new(car),
			settings,
			ends,
			opened: Duration::ZERO,
			events: Vec::new(),
		}
	}

	pub fn car(&self) -> &Car {
		self.server.car()
	}

	/// A channel opened at `now`: every unit starts over, and nothing half sent goes on.
	pub fn reset(&mut self, now: Duration) {
		self.server.reset();
		self.opened = now;
		for end in &mut self.ends {
			end.rx = None;
			end.outbox.clear();
			end.tx = None;
		}
	}

	/// A frame from the tester. Frames on ids no unit is asked on are another node's
	/// traffic and are ignored, as a unit ignores them.
	pub fn receive(&mut self, now: Duration, id: u16, data: &[u8], out: &mut Vec<Frame>) {
		let Some(at) = self.ends.iter().position(|e| e.request == id) else {
			return;
		};
		let Some(&pci) = data.first() else { return };
		match pci >> 4 {
			0x0 => {
				let len = usize::from(pci & 0x0F);
				match data.get(1..1 + len) {
					Some(pdu) if len > 0 => {
						let pdu = pdu.to_vec();
						self.ends[at].rx = None;
						self.request(at, now, pdu);
					}
					_ => self.protocol(at, format!("single frame of length {len} in {} bytes", data.len())),
				}
			}
			0x1 => {
				let len = (usize::from(pci & 0x0F) << 8) | usize::from(*data.get(1).unwrap_or(&0));
				if len <= 7 || data.len() < 8 {
					self.protocol(at, format!("first frame of length {len} in {} bytes", data.len()));
					return;
				}
				let block = self.settings.block_size;
				self.ends[at].rx = Some(Rx {
					len,
					data: data[2..8].to_vec(),
					next_seq: 1,
					left_in_block: (block > 0).then_some(block),
					deadline: now + CONSECUTIVE_WAIT,
				});
				out.push(self.flow_control(at));
			}
			0x2 => self.consecutive(at, now, pci, data, out),
			0x3 => self.flow_control_from_tester(at, now, pci, data),
			_ => self.protocol(at, format!("unknown frame type {pci:02X}")),
		}
	}

	fn consecutive(&mut self, at: usize, now: Duration, pci: u8, data: &[u8], out: &mut Vec<Frame>) {
		let Some(rx) = self.ends[at].rx.as_mut() else {
			// ISO 15765-2: a consecutive frame with no reception under way is ignored.
			self.protocol(at, "a consecutive frame with no request under way, ignored".into());
			return;
		};
		if pci & 0x0F != rx.next_seq {
			let what = format!("consecutive frame {} where {} was due; request dropped", pci & 0x0F, rx.next_seq);
			self.ends[at].rx = None;
			self.protocol(at, what);
			return;
		}
		let take = (rx.len - rx.data.len()).min(7).min(data.len().saturating_sub(1));
		rx.data.extend_from_slice(&data[1..1 + take]);
		rx.next_seq = (rx.next_seq + 1) & 0x0F;
		rx.deadline = now + CONSECUTIVE_WAIT;
		if rx.data.len() >= rx.len {
			let pdu = self.ends[at].rx.take().map(|rx| rx.data).unwrap_or_default();
			self.request(at, now, pdu);
			return;
		}
		if let Some(left) = rx.left_in_block.as_mut() {
			*left -= 1;
			if *left == 0 {
				*left = self.settings.block_size;
				out.push(self.flow_control(at));
			}
		}
	}

	fn flow_control_from_tester(&mut self, at: usize, now: Duration, pci: u8, data: &[u8]) {
		let Some(tx) = self.ends[at].tx.as_mut() else {
			self.protocol(at, "a flow control with no answer under way, ignored".into());
			return;
		};
		if !matches!(tx.state, TxState::AwaitFlowControl { .. }) {
			self.protocol(at, "a flow control while sending, ignored".into());
			return;
		}
		match pci & 0x0F {
			0x0 => {
				let block = data.get(1).copied().unwrap_or(0);
				let st_min = data.get(2).copied().unwrap_or(0);
				tx.state = TxState::Sending {
					next: now,
					left_in_block: (block > 0).then_some(block),
					gap: separation(st_min),
				};
			}
			0x1 => {
				tx.state = TxState::AwaitFlowControl {
					until: now + FLOW_CONTROL_WAIT,
				}
			}
			status => {
				self.ends[at].tx = None;
				self.protocol(at, format!("flow status {status:X} from the tester; answer dropped"));
			}
		}
	}

	/// A whole request: hand it to the unit and queue what it answers.
	fn request(&mut self, at: usize, now: Duration, pdu: Vec<u8>) {
		let unit = self.ends[at].request;
		self.events.push(Event::Request { unit, pdu: pdu.clone() });
		match self.server.answer(at, now.saturating_sub(self.opened), &pdu) {
			Reply::Silent(why) => self.events.push(Event::Silent { unit, why }),
			// Behind anything still queued: an answer that is late stays late, as a real
			// unit's does, and the tester has to tell it from the next one.
			Reply::Answers(answers) => self.ends[at].outbox.extend(answers.into_iter().map(|(after, pdu)| (now + after, pdu))),
		}
	}

	/// Everything due by `now`, in order.
	pub fn poll(&mut self, now: Duration, out: &mut Vec<Frame>) {
		for at in 0..self.ends.len() {
			if let Some(rx) = &self.ends[at].rx
				&& now >= rx.deadline
			{
				self.ends[at].rx = None;
				self.protocol(at, "the rest of a request never came; dropped".into());
			}
			loop {
				match self.step(at, now) {
					Some(frame) => out.push(frame),
					None => break,
				}
			}
		}
	}

	/// The next frame unit `at` sends at `now`, if any.
	fn step(&mut self, at: usize, now: Duration) -> Option<Frame> {
		let padding = self.settings.padding;
		let end = &mut self.ends[at];
		let response = end.response;
		if end.tx.is_none() {
			let due = end.outbox.front().is_some_and(|(when, _)| *when <= now);
			if !due {
				return None;
			}
			let (_, pdu) = end.outbox.pop_front()?;
			let unit = end.request;
			self.events.push(Event::Answer { unit, id: response, pdu: pdu.clone() });
			if pdu.len() <= 7 {
				let mut data = vec![pdu.len() as u8];
				data.extend_from_slice(&pdu);
				return Some(frame(response, &data, padding));
			}
			let len = pdu.len();
			let mut data = vec![0x10 | ((len >> 8) as u8 & 0x0F), len as u8];
			data.extend_from_slice(&pdu[..6]);
			self.ends[at].tx = Some(Tx {
				pdu,
				offset: 6,
				seq: 1,
				state: TxState::AwaitFlowControl {
					until: now + FLOW_CONTROL_WAIT,
				},
			});
			return Some(frame(response, &data, padding));
		}
		let tx = end.tx.as_mut()?;
		match &mut tx.state {
			TxState::AwaitFlowControl { until } => {
				if now >= *until {
					end.tx = None;
					self.protocol(at, "no flow control from the tester; answer dropped".into());
				}
				None
			}
			TxState::Sending { next, left_in_block, gap } => {
				if now < *next {
					return None;
				}
				let stop = (tx.offset + 7).min(tx.pdu.len());
				let mut data = vec![0x20 | tx.seq];
				data.extend_from_slice(&tx.pdu[tx.offset..stop]);
				tx.offset = stop;
				tx.seq = (tx.seq + 1) & 0x0F;
				let sent = frame(response, &data, padding);
				if tx.offset >= tx.pdu.len() {
					end.tx = None;
					return Some(sent);
				}
				*next = now + *gap;
				if let Some(left) = left_in_block {
					*left -= 1;
					if *left == 0 {
						tx.state = TxState::AwaitFlowControl {
							until: now + FLOW_CONTROL_WAIT,
						};
					}
				}
				Some(sent)
			}
		}
	}

	/// When [`Self::poll`] next has something to do, if ever.
	pub fn next_due(&self) -> Option<Duration> {
		self
			.ends
			.iter()
			.flat_map(|end| {
				let tx = end.tx.as_ref().map(|tx| match tx.state {
					TxState::AwaitFlowControl { until } => until,
					TxState::Sending { next, .. } => next,
				});
				let queued = match end.tx {
					None => end.outbox.front().map(|(when, _)| *when),
					Some(_) => None,
				};
				[tx, queued, end.rx.as_ref().map(|rx| rx.deadline)]
			})
			.flatten()
			.min()
	}

	fn flow_control(&self, at: usize) -> Frame {
		let s = self.settings;
		frame(self.ends[at].response, &[0x30, s.block_size, s.st_min], s.padding)
	}

	fn protocol(&mut self, at: usize, what: String) {
		let unit = self.ends[at].request;
		self.events.push(Event::Protocol { unit, what });
	}
}

fn frame(id: u16, data: &[u8], padding: u8) -> Frame {
	let mut out = [padding; 8];
	out[..data.len()].copy_from_slice(data);
	Frame { id, data: out }
}

/// An STmin byte as a gap (ISO 15765-2 §9.6.5.4); a reserved value is the longest gap.
fn separation(st_min: u8) -> Duration {
	match st_min {
		0x00..=0x7F => Duration::from_millis(u64::from(st_min)),
		0xF1..=0xF9 => Duration::from_micros(u64::from(st_min - 0xF0) * 100),
		_ => Duration::from_millis(0x7F),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::profile::Profile;

	fn bus(text: &str) -> Bus {
		Bus::new(Profile::parse(text).unwrap().resolve().unwrap())
	}

	const ENGINE: &str = "[[unit]]\nrequest = \"7E0\"\nvin = \"SIMVIN00000000001\"\n[unit.did]\nF40D = \"00\"\n";

	fn ms(n: u64) -> Duration {
		Duration::from_millis(n)
	}

	/// Send `frames` at `now` and poll: what came back.
	fn exchange(bus: &mut Bus, now: Duration, frames: &[(u16, &[u8])]) -> Vec<Frame> {
		let mut out = Vec::new();
		for (id, data) in frames {
			bus.receive(now, *id, data, &mut out);
		}
		bus.poll(now, &mut out);
		out
	}

	#[test]
	fn a_short_answer_is_one_single_frame_on_the_answer_id() {
		let mut bus = bus(ENGINE);
		let out = exchange(&mut bus, ms(0), &[(0x7E0, &[0x03, 0x22, 0xF4, 0x0D, 0, 0, 0, 0])]);
		assert_eq!(
			out,
			vec![Frame {
				id: 0x7E8,
				data: [0x04, 0x62, 0xF4, 0x0D, 0x00, 0, 0, 0]
			}]
		);
	}

	#[test]
	fn a_long_answer_waits_for_flow_control_and_honours_its_block_size_and_gap() {
		let mut bus = bus("[[unit]]\nrequest = \"7E0\"\n[unit.did]\n0100 = { text = \"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123\" }\n");
		let first = exchange(&mut bus, ms(0), &[(0x7E0, &[0x03, 0x22, 0x01, 0x00])]);
		// 62 01 00 and 30 bytes = 33: a first frame, and nothing more until flow control.
		assert_eq!(
			first,
			vec![Frame {
				id: 0x7E8,
				data: [0x10, 33, 0x62, 0x01, 0x00, b'A', b'B', b'C']
			}]
		);
		assert!(exchange(&mut bus, ms(10), &[]).is_empty());
		// Block size 1: one consecutive frame, then wait for the next flow control.
		let next = exchange(&mut bus, ms(10), &[(0x7E0, &[0x30, 0x01, 0x05])]);
		assert_eq!(
			next,
			vec![Frame {
				id: 0x7E8,
				data: [0x21, b'D', b'E', b'F', b'G', b'H', b'I', b'J']
			}]
		);
		assert!(exchange(&mut bus, ms(20), &[]).is_empty(), "waits for the block's flow control");
		// Block size 0, STmin 5 ms: the rest, 5 ms apart.
		assert_eq!(exchange(&mut bus, ms(20), &[(0x7E0, &[0x30, 0x00, 0x05])]).len(), 1);
		assert!(exchange(&mut bus, ms(24), &[]).is_empty(), "the gap holds the next back");
		assert_eq!(exchange(&mut bus, ms(25), &[])[0].data[0], 0x23);
		let last = exchange(&mut bus, ms(30), &[]);
		assert_eq!(
			last,
			vec![Frame {
				id: 0x7E8,
				data: [0x24, b'Y', b'Z', b'0', b'1', b'2', b'3', 0]
			}]
		);
		assert_eq!(bus.next_due(), None);
	}

	#[test]
	fn an_answer_with_no_flow_control_is_given_up() {
		let mut bus = bus(ENGINE);
		exchange(&mut bus, ms(0), &[(0x7E0, &[0x03, 0x22, 0xF1, 0x90])]);
		assert_eq!(bus.next_due(), Some(ms(1000)));
		assert!(exchange(&mut bus, ms(1000), &[]).is_empty());
		assert!(
			bus
				.events
				.iter()
				.any(|e| matches!(e, Event::Protocol { what, .. } if what.starts_with("no flow control")))
		);
		assert_eq!(bus.next_due(), None);
	}

	#[test]
	fn a_long_request_gets_the_profiles_flow_control_and_is_reassembled() {
		let mut bus = bus(&format!("[bus]\nblock_size = 1\nst_min = 2\npadding = 0xAA\n{ENGINE}"));
		// 22, F40D six times, F190: fifteen bytes, a first frame and two consecutive frames.
		let fc = exchange(&mut bus, ms(0), &[(0x7E0, &[0x10, 0x0F, 0x22, 0xF4, 0x0D, 0xF4, 0x0D, 0xF4])]);
		assert_eq!(fc, vec![Frame { id: 0x7E8, data: [0x30, 0x01, 0x02, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA] }]);
		let fc = exchange(&mut bus, ms(1), &[(0x7E0, &[0x21, 0x0D, 0xF4, 0x0D, 0xF4, 0x0D, 0xF4, 0x0D])]);
		assert_eq!(fc.len(), 1, "another flow control after a block of one");
		let answer = exchange(&mut bus, ms(2), &[(0x7E0, &[0x22, 0xF1, 0x90, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA])]);
		// 62, F40D 00 six times, F190 and seventeen bytes.
		assert_eq!(answer[0].data[..2], [0x10, 1 + 6 * 3 + 2 + 17]);
		let mut asked = vec![0x22];
		for _ in 0..6 {
			asked.extend_from_slice(&[0xF4, 0x0D]);
		}
		asked.extend_from_slice(&[0xF1, 0x90]);
		assert!(matches!(&bus.events[0], Event::Request { pdu, .. } if *pdu == asked));
	}

	#[test]
	fn a_consecutive_frame_out_of_order_drops_the_request() {
		let mut bus = bus(ENGINE);
		exchange(&mut bus, ms(0), &[(0x7E0, &[0x10, 0x09, 0x22, 0xF4, 0x0D, 0xF4, 0x0D, 0xF4])]);
		let out = exchange(&mut bus, ms(1), &[(0x7E0, &[0x22, 0x0D, 0xF1, 0x90, 0, 0, 0, 0])]);
		assert!(out.is_empty());
		assert!(
			bus
				.events
				.iter()
				.any(|e| matches!(e, Event::Protocol { what, .. } if what.contains("request dropped")))
		);
	}

	#[test]
	fn other_ids_and_absent_units_say_nothing() {
		let mut bus = bus(ENGINE);
		assert!(exchange(&mut bus, ms(0), &[(0x7E1, &[0x03, 0x22, 0xF4, 0x0D]), (0x123, &[0x01, 0x3E])]).is_empty());
		assert!(bus.events.is_empty());
	}

	#[test]
	fn pending_answers_go_out_at_their_times_and_a_reset_drops_them() {
		let mut bus = bus("[[unit]]\nrequest = \"7E0\"\n[unit.did]\n0100 = { value = \"01\", pending = 1, pending_ms = 100 }\n");
		let first = exchange(&mut bus, ms(0), &[(0x7E0, &[0x03, 0x22, 0x01, 0x00])]);
		assert_eq!(first[0].data[..4], [0x03, 0x7F, 0x22, 0x78]);
		assert_eq!(bus.next_due(), Some(ms(100)));
		let last = exchange(&mut bus, ms(100), &[]);
		assert_eq!(last[0].data[..5], [0x04, 0x62, 0x01, 0x00, 0x01]);

		exchange(&mut bus, ms(200), &[(0x7E0, &[0x03, 0x22, 0x01, 0x00])]);
		bus.reset(ms(250));
		assert!(exchange(&mut bus, ms(300), &[]).is_empty());
	}

	#[test]
	fn a_series_is_read_on_the_clock_of_the_channel() {
		let mut bus =
			bus("[[unit]]\nrequest = \"7E0\"\n[unit.did]\nF40D = { series = [ { at_ms = 0, value = \"00\" }, { at_ms = 500, value = \"1E\" } ] }\n");
		bus.reset(ms(10_000));
		let before = exchange(&mut bus, ms(10_400), &[(0x7E0, &[0x03, 0x22, 0xF4, 0x0D])]);
		assert_eq!(before[0].data[4], 0x00);
		let after = exchange(&mut bus, ms(10_500), &[(0x7E0, &[0x03, 0x22, 0xF4, 0x0D])]);
		assert_eq!(after[0].data[4], 0x1E);
	}
}
