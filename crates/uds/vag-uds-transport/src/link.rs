//! UDS over a byte pipe: the framing between the host and the dash board.
//!
//! The pipe is BLE NUS now and may be the USB cable later. It already carries
//! `dashcfg`'s plain-text commands (`state`, `set brightness 3`…) and the board's
//! text notifications, and neither ever starts with a NUL byte. So a framed
//! message starts with one:
//!
//! ```text
//! 0x00  marker
//! u8    type
//! u16   body length, little-endian
//! body
//! ```
//!
//! Every multi-byte field is little-endian.
//!
//! | type   | name        | direction    | body                                                               |
//! |--------|-------------|--------------|--------------------------------------------------------------------|
//! | `0x01` | Request     | host → board | `seq u8, request_id u16, response_id u16, pdu…`                    |
//! | `0x02` | Answer      | board → host | `seq u8, status u8, payload…`                                      |
//! | `0x03` | Subscribe   | host → board | `sub u16, request_id u16, response_id u16, did u16, period_ms u16` |
//! | `0x04` | Unsubscribe | host → board | `sub u16`                                                          |
//! | `0x05` | Reading     | board → host | `sub u16, at_ms u32, status u8, payload…`                          |
//!
//! Status, in an Answer and a Reading alike: `0` the payload is the unit's
//! answer PDU, `1` no answer (timeout, no payload), `2` refused by the board
//! (payload: a short UTF-8 reason), `3` bus error (payload: a UTF-8 reason).
//!
//! A subscription asks the board to read `did` (`22 did`) every `period_ms` on
//! its own clock and send each result as a Reading: timing an acceleration run
//! or polling a watch page over a radio link only works if the timestamps are
//! taken where the bus is. `at_ms` is the board's clock, milliseconds since
//! boot, at the moment the answer arrived. The period floor is the board's
//! guard's to enforce, not the codec's. A one-shot read is a Request.
//!
//! BLE's link layer guarantees delivery and integrity, so there is no checksum.
//! What the link does *not* keep is message boundaries: a write or a
//! notification is cut at the MTU, so [`Reassembler`] takes arbitrary chunks and
//! gives back whole messages.
//!
//! No clocks and no I/O here: `no_std` + `alloc`, the same code on the board and
//! on the laptop.

use alloc::string::String;
use alloc::vec::Vec;

/// First byte of every framed message. Text on the same pipe never starts with it.
pub const MARKER: u8 = 0x00;
/// Marker, type and the two length bytes.
pub const HEADER_LEN: usize = 4;
/// The largest body the reassembler accepts. The largest body this link sends is a
/// Reading's, 7 + [`MAX_PDU`] bytes; anything past this cap is not a message.
pub const MAX_BODY: usize = 4200;
/// The largest PDU ISO-TP carries: its length field is 12 bits (ISO 15765-2).
pub const MAX_PDU: usize = 4095;
/// Largest 11-bit CAN identifier.
const MAX_STANDARD_ID: u16 = 0x7FF;

const TYPE_REQUEST: u8 = 0x01;
const TYPE_ANSWER: u8 = 0x02;
const TYPE_SUBSCRIBE: u8 = 0x03;
const TYPE_UNSUBSCRIBE: u8 = 0x04;
const TYPE_READING: u8 = 0x05;

const STATUS_PDU: u8 = 0;
const STATUS_NO_ANSWER: u8 = 1;
const STATUS_REFUSED: u8 = 2;
const STATUS_BUS_ERROR: u8 = 3;

/// One UDS request for the board to put on the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
	/// Chosen by the host, echoed in the [`Answer`].
	pub seq: u8,
	/// 11-bit CAN id the board sends on.
	pub request_id: u16,
	/// 11-bit CAN id the board listens on.
	pub response_id: u16,
	/// The whole UDS PDU, `1..=MAX_PDU` bytes.
	pub pdu: Vec<u8>,
}

/// What became of one [`Request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answer {
	/// The request's `seq`.
	pub seq: u8,
	pub outcome: Outcome,
}

/// Poll one identifier on the board's clock, until [`Message::Unsubscribe`] or disconnect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subscribe {
	/// Chosen by the host, carried by every [`Reading`].
	pub sub: u16,
	/// 11-bit CAN id the board sends on.
	pub request_id: u16,
	/// 11-bit CAN id the board listens on.
	pub response_id: u16,
	/// The identifier read: the board sends `22 did`.
	pub did: u16,
	pub period_ms: u16,
}

/// One result of a [`Subscribe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
	pub sub: u16,
	/// The board's clock, ms since boot, when the answer arrived.
	pub at_ms: u32,
	pub outcome: Outcome,
}

/// The status byte of an [`Answer`] or a [`Reading`] and what it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
	/// The unit's answer PDU, `1..=MAX_PDU` bytes.
	Pdu(Vec<u8>),
	/// Nothing came back in time.
	NoAnswer,
	/// The board would not send it; why, in plain words.
	Refused(String),
	/// The bus failed; why, in plain words.
	BusError(String),
}

/// One framed message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
	Request(Request),
	Answer(Answer),
	Subscribe(Subscribe),
	Unsubscribe { sub: u16 },
	Reading(Reading),
}

/// Why a message could not be encoded or was not accepted.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
	#[error("message body of {0} bytes is over the {MAX_BODY}-byte cap")]
	Oversize(usize),
	#[error("unknown message type 0x{0:02X}")]
	UnknownType(u8),
	#[error("malformed message: {0}")]
	Malformed(&'static str),
}

/// What [`Reassembler::push`] recovered from the pipe, in the order it arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
	/// A whole framed message.
	Message(Message),
	/// Bytes that are not framed: the text protocol. Runs to the next NUL or the
	/// end of the chunk, whichever is first — text is one command per write, so a
	/// text piece never continues into the next chunk.
	Text(Vec<u8>),
	/// A frame that was rejected. The reassembler has already resynchronised.
	Error(LinkError),
}

/// Encode a message into one frame, ready for [`chunks`].
///
/// Refuses what the other end would reject: a PDU that is empty or over
/// [`MAX_PDU`], a CAN id over 11 bits, a body over [`MAX_BODY`].
pub fn encode(message: &Message) -> Result<Vec<u8>, LinkError> {
	check(message)?;
	let kind = match message {
		Message::Request(_) => TYPE_REQUEST,
		Message::Answer(_) => TYPE_ANSWER,
		Message::Subscribe(_) => TYPE_SUBSCRIBE,
		Message::Unsubscribe { .. } => TYPE_UNSUBSCRIBE,
		Message::Reading(_) => TYPE_READING,
	};
	// The length is filled in once the body is written.
	let mut out = alloc::vec![MARKER, kind, 0, 0];
	match message {
		Message::Request(r) => {
			out.push(r.seq);
			out.extend_from_slice(&r.request_id.to_le_bytes());
			out.extend_from_slice(&r.response_id.to_le_bytes());
			out.extend_from_slice(&r.pdu);
		}
		Message::Answer(a) => {
			out.push(a.seq);
			put_outcome(&mut out, &a.outcome);
		}
		Message::Subscribe(s) => {
			for field in [s.sub, s.request_id, s.response_id, s.did, s.period_ms] {
				out.extend_from_slice(&field.to_le_bytes());
			}
		}
		Message::Unsubscribe { sub } => out.extend_from_slice(&sub.to_le_bytes()),
		Message::Reading(r) => {
			out.extend_from_slice(&r.sub.to_le_bytes());
			out.extend_from_slice(&r.at_ms.to_le_bytes());
			put_outcome(&mut out, &r.outcome);
		}
	}
	let body_len = out.len() - HEADER_LEN;
	if body_len > MAX_BODY {
		return Err(LinkError::Oversize(body_len));
	}
	out[2..HEADER_LEN].copy_from_slice(&(body_len as u16).to_le_bytes());
	Ok(out)
}

/// A status byte and its payload.
fn put_outcome(out: &mut Vec<u8>, outcome: &Outcome) {
	match outcome {
		Outcome::Pdu(pdu) => {
			out.push(STATUS_PDU);
			out.extend_from_slice(pdu);
		}
		Outcome::NoAnswer => out.push(STATUS_NO_ANSWER),
		Outcome::Refused(reason) => {
			out.push(STATUS_REFUSED);
			out.extend_from_slice(reason.as_bytes());
		}
		Outcome::BusError(reason) => {
			out.push(STATUS_BUS_ERROR);
			out.extend_from_slice(reason.as_bytes());
		}
	}
}

/// The rules both ends hold a message to, beyond its framing.
fn check(message: &Message) -> Result<(), LinkError> {
	let ids = |request_id: u16, response_id: u16| {
		if request_id > MAX_STANDARD_ID || response_id > MAX_STANDARD_ID {
			return Err(LinkError::Malformed("CAN id over 11 bits"));
		}
		Ok(())
	};
	let pdu = match message {
		Message::Request(r) => {
			ids(r.request_id, r.response_id)?;
			&r.pdu
		}
		Message::Subscribe(s) => return ids(s.request_id, s.response_id),
		Message::Answer(Answer {
			outcome: Outcome::Pdu(pdu), ..
		})
		| Message::Reading(Reading {
			outcome: Outcome::Pdu(pdu), ..
		}) => pdu,
		Message::Answer(_) | Message::Reading(_) | Message::Unsubscribe { .. } => return Ok(()),
	};
	match pdu.len() {
		0 => Err(LinkError::Malformed("empty PDU")),
		n if n > MAX_PDU => Err(LinkError::Malformed("PDU over ISO-TP's 4095 bytes")),
		_ => Ok(()),
	}
}

/// A status byte and its payload, back.
fn take_outcome(status: u8, payload: &[u8]) -> Result<Outcome, LinkError> {
	let reason = || {
		core::str::from_utf8(payload)
			.map(String::from)
			.map_err(|_| LinkError::Malformed("reason is not UTF-8"))
	};
	Ok(match status {
		STATUS_PDU => Outcome::Pdu(payload.to_vec()),
		STATUS_NO_ANSWER if payload.is_empty() => Outcome::NoAnswer,
		STATUS_NO_ANSWER => return Err(LinkError::Malformed("no-answer status with a payload")),
		STATUS_REFUSED => Outcome::Refused(reason()?),
		STATUS_BUS_ERROR => Outcome::BusError(reason()?),
		_ => return Err(LinkError::Malformed("unknown status")),
	})
}

/// Decode the body of a frame whose type is already known to be one of ours.
fn decode(kind: u8, body: &[u8]) -> Result<Message, LinkError> {
	let message = match kind {
		TYPE_REQUEST => {
			let [seq, a, b, c, d, pdu @ ..] = body else {
				return Err(LinkError::Malformed("request shorter than its fields"));
			};
			Message::Request(Request {
				seq: *seq,
				request_id: u16::from_le_bytes([*a, *b]),
				response_id: u16::from_le_bytes([*c, *d]),
				pdu: pdu.to_vec(),
			})
		}
		TYPE_ANSWER => {
			let [seq, status, payload @ ..] = body else {
				return Err(LinkError::Malformed("answer shorter than its fields"));
			};
			Message::Answer(Answer {
				seq: *seq,
				outcome: take_outcome(*status, payload)?,
			})
		}
		TYPE_SUBSCRIBE => {
			let [a, b, c, d, e, f, g, h, i, j] = body else {
				return Err(LinkError::Malformed("subscribe is ten bytes"));
			};
			Message::Subscribe(Subscribe {
				sub: u16::from_le_bytes([*a, *b]),
				request_id: u16::from_le_bytes([*c, *d]),
				response_id: u16::from_le_bytes([*e, *f]),
				did: u16::from_le_bytes([*g, *h]),
				period_ms: u16::from_le_bytes([*i, *j]),
			})
		}
		TYPE_UNSUBSCRIBE => {
			let [a, b] = body else {
				return Err(LinkError::Malformed("unsubscribe is two bytes"));
			};
			Message::Unsubscribe {
				sub: u16::from_le_bytes([*a, *b]),
			}
		}
		TYPE_READING => {
			let [s0, s1, t0, t1, t2, t3, status, payload @ ..] = body else {
				return Err(LinkError::Malformed("reading shorter than its fields"));
			};
			Message::Reading(Reading {
				sub: u16::from_le_bytes([*s0, *s1]),
				at_ms: u32::from_le_bytes([*t0, *t1, *t2, *t3]),
				outcome: take_outcome(*status, payload)?,
			})
		}
		other => return Err(LinkError::UnknownType(other)),
	};
	check(&message)?;
	Ok(message)
}
/// Split an encoded frame into pieces of at most `max` bytes (a zero `max` is taken as 1).
pub fn chunks(bytes: &[u8], max: usize) -> impl Iterator<Item = &[u8]> {
	bytes.chunks(max.max(1))
}

/// Turns chunks cut at arbitrary points back into messages and text.
///
/// Resynchronisation: a frame whose header is not trusted — an unknown type or
/// a length over [`MAX_BODY`] — gives [`Piece::Error`] and the rest of that
/// chunk is dropped, so the next chunk starts clean. A frame whose header was
/// fine but whose body does not decode gives [`Piece::Error`] and costs only
/// that frame. A frame that never finishes cannot be told from a slow one
/// without a clock: the owner calls [`Reassembler::reset`] when the connection
/// drops.
#[derive(Debug, Default)]
pub struct Reassembler {
	/// The frame in progress, header included. Empty between frames.
	partial: Vec<u8>,
}

impl Reassembler {
	pub fn new() -> Self {
		Self::default()
	}

	/// Forget any frame in progress.
	pub fn reset(&mut self) {
		self.partial.clear();
	}

	/// Feed one chunk; get back everything it completed.
	pub fn push(&mut self, chunk: &[u8]) -> Vec<Piece> {
		let mut out = Vec::new();
		let mut at = 0;
		while at < chunk.len() {
			if self.partial.is_empty() && chunk[at] != MARKER {
				let end = chunk[at..].iter().position(|&b| b == MARKER).map_or(chunk.len(), |p| at + p);
				out.push(Piece::Text(chunk[at..end].to_vec()));
				at = end;
				continue;
			}
			if self.partial.len() < HEADER_LEN {
				let take = (HEADER_LEN - self.partial.len()).min(chunk.len() - at);
				self.partial.extend_from_slice(&chunk[at..at + take]);
				at += take;
				if self.partial.len() < HEADER_LEN {
					break;
				}
				let kind = self.partial[1];
				let len = self.body_len();
				let untrusted = if !(TYPE_REQUEST..=TYPE_READING).contains(&kind) {
					Some(LinkError::UnknownType(kind))
				} else if len > MAX_BODY {
					Some(LinkError::Oversize(len))
				} else {
					None
				};
				if let Some(error) = untrusted {
					// The length cannot be trusted, so neither can anything after it
					// in this chunk.
					out.push(Piece::Error(error));
					self.partial.clear();
					break;
				}
			}
			let whole = HEADER_LEN + self.body_len();
			let take = (whole - self.partial.len()).min(chunk.len() - at);
			self.partial.extend_from_slice(&chunk[at..at + take]);
			at += take;
			if self.partial.len() == whole {
				out.push(match decode(self.partial[1], &self.partial[HEADER_LEN..]) {
					Ok(message) => Piece::Message(message),
					Err(error) => Piece::Error(error),
				});
				self.partial.clear();
			}
		}
		out
	}

	/// The body length in the header of the frame in progress.
	fn body_len(&self) -> usize {
		u16::from_le_bytes([self.partial[2], self.partial[3]]) as usize
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::vec;

	fn request() -> Message {
		Message::Request(Request {
			seq: 7,
			request_id: 0x7E0,
			response_id: 0x7E8,
			pdu: vec![0x22, 0xF1, 0x90],
		})
	}

	fn answer(outcome: Outcome) -> Message {
		Message::Answer(Answer { seq: 7, outcome })
	}

	fn one_message(pieces: Vec<Piece>) -> Message {
		match pieces.as_slice() {
			[Piece::Message(m)] => m.clone(),
			other => panic!("expected one message, got {other:?}"),
		}
	}

	#[test]
	fn a_request_is_encoded_byte_for_byte_as_the_wire_format_says() {
		let bytes = encode(&request()).unwrap();
		assert_eq!(bytes, vec![0x00, 0x01, 0x08, 0x00, 7, 0xE0, 0x07, 0xE8, 0x07, 0x22, 0xF1, 0x90]);
	}

	#[test]
	fn an_answer_is_encoded_byte_for_byte_as_the_wire_format_says() {
		assert_eq!(
			encode(&answer(Outcome::Pdu(vec![0x62, 0xF1, 0x90, b'X']))).unwrap(),
			vec![0x00, 0x02, 0x06, 0x00, 7, 0, 0x62, 0xF1, 0x90, b'X']
		);
		assert_eq!(encode(&answer(Outcome::NoAnswer)).unwrap(), vec![0x00, 0x02, 0x02, 0x00, 7, 1]);
		assert_eq!(
			encode(&answer(Outcome::Refused("no".into()))).unwrap(),
			vec![0x00, 0x02, 0x04, 0x00, 7, 2, b'n', b'o']
		);
		assert_eq!(
			encode(&answer(Outcome::BusError("off".into()))).unwrap(),
			vec![0x00, 0x02, 0x05, 0x00, 7, 3, b'o', b'f', b'f']
		);
	}

	#[test]
	fn every_message_kind_survives_a_round_trip() {
		let messages = [
			request(),
			answer(Outcome::Pdu(vec![0x62, 0xF1, 0x90, 1, 2, 3])),
			answer(Outcome::NoAnswer),
			answer(Outcome::Refused("the car is moving".into())),
			answer(Outcome::BusError("bus off".into())),
			answer(Outcome::Refused(String::new())),
		];
		for message in messages {
			let mut r = Reassembler::new();
			assert_eq!(one_message(r.push(&encode(&message).unwrap())), message);
		}
	}

	#[test]
	fn a_largest_pdu_round_trips() {
		let big = Message::Request(Request {
			seq: 255,
			request_id: 0x7FF,
			response_id: 0x000,
			pdu: (0..MAX_PDU).map(|i| i as u8).collect(),
		});
		let mut r = Reassembler::new();
		assert_eq!(one_message(r.push(&encode(&big).unwrap())), big);
	}

	#[test]
	fn a_frame_split_at_every_byte_boundary_reassembles() {
		let frame = encode(&answer(Outcome::Pdu(vec![0x59, 0x02, 0xFF, 0x00, 0x01, 0x29, 0x08]))).unwrap();
		for cut in 0..=frame.len() {
			let mut r = Reassembler::new();
			let mut pieces = r.push(&frame[..cut]);
			pieces.extend(r.push(&frame[cut..]));
			assert_eq!(
				one_message(pieces),
				answer(Outcome::Pdu(vec![0x59, 0x02, 0xFF, 0x00, 0x01, 0x29, 0x08])),
				"cut at {cut}"
			);
		}
	}

	#[test]
	fn a_frame_fed_one_byte_at_a_time_reassembles() {
		let frame = encode(&request()).unwrap();
		let mut r = Reassembler::new();
		let mut pieces = Vec::new();
		for byte in &frame {
			pieces.extend(r.push(core::slice::from_ref(byte)));
		}
		assert_eq!(one_message(pieces), request());
	}

	#[test]
	fn chunks_cut_at_the_mtu_reassemble() {
		let message = answer(Outcome::Pdu((0..600).map(|i| i as u8).collect()));
		let frame = encode(&message).unwrap();
		let pieces: Vec<&[u8]> = chunks(&frame, 244).collect();
		assert_eq!(pieces.len(), 3);
		assert!(pieces.iter().all(|p| p.len() <= 244));
		let mut r = Reassembler::new();
		let mut out = Vec::new();
		for p in pieces {
			out.extend(r.push(p));
		}
		assert_eq!(one_message(out), message);
	}

	fn subscribe() -> Message {
		Message::Subscribe(Subscribe {
			sub: 0x0102,
			request_id: 0x7E1,
			response_id: 0x7E9,
			did: 0xF40D,
			period_ms: 20,
		})
	}

	fn reading(outcome: Outcome) -> Message {
		Message::Reading(Reading {
			sub: 0x0102,
			at_ms: 0x0A0B_0C0D,
			outcome,
		})
	}

	/// One of every message kind and status.
	fn every_kind() -> Vec<Message> {
		vec![
			request(),
			answer(Outcome::Pdu(vec![0x62, 0xF1, 0x90, 1, 2, 3])),
			answer(Outcome::NoAnswer),
			answer(Outcome::Refused("the car is moving".into())),
			answer(Outcome::BusError("bus off".into())),
			subscribe(),
			Message::Unsubscribe { sub: 0xBEEF },
			reading(Outcome::Pdu(vec![0x62, 0xF4, 0x0D, 57])),
			reading(Outcome::NoAnswer),
			reading(Outcome::Refused("8 evenly spaced identifiers".into())),
			reading(Outcome::BusError("bus off".into())),
		]
	}

	#[test]
	fn subscribe_unsubscribe_and_reading_are_encoded_byte_for_byte() {
		assert_eq!(
			encode(&subscribe()).unwrap(),
			vec![0x00, 0x03, 0x0A, 0x00, 0x02, 0x01, 0xE1, 0x07, 0xE9, 0x07, 0x0D, 0xF4, 0x14, 0x00]
		);
		assert_eq!(
			encode(&Message::Unsubscribe { sub: 0xBEEF }).unwrap(),
			vec![0x00, 0x04, 0x02, 0x00, 0xEF, 0xBE]
		);
		assert_eq!(
			encode(&reading(Outcome::Pdu(vec![0x62, 0xF4, 0x0D, 57]))).unwrap(),
			vec![0x00, 0x05, 0x0B, 0x00, 0x02, 0x01, 0x0D, 0x0C, 0x0B, 0x0A, 0, 0x62, 0xF4, 0x0D, 57]
		);
		assert_eq!(
			encode(&reading(Outcome::NoAnswer)).unwrap(),
			vec![0x00, 0x05, 0x07, 0x00, 0x02, 0x01, 0x0D, 0x0C, 0x0B, 0x0A, 1]
		);
		assert_eq!(
			encode(&reading(Outcome::Refused("no".into()))).unwrap(),
			vec![0x00, 0x05, 0x09, 0x00, 0x02, 0x01, 0x0D, 0x0C, 0x0B, 0x0A, 2, b'n', b'o']
		);
	}

	#[test]
	fn every_kind_survives_a_round_trip_split_at_every_byte() {
		for message in every_kind() {
			let frame = encode(&message).unwrap();
			for cut in 0..=frame.len() {
				let mut r = Reassembler::new();
				let mut pieces = r.push(&frame[..cut]);
				pieces.extend(r.push(&frame[cut..]));
				assert_eq!(one_message(pieces), message, "cut at {cut}");
			}
		}
	}

	#[test]
	fn a_stream_of_every_kind_cut_at_a_small_mtu_reassembles_in_order() {
		let mut stream = Vec::new();
		for message in every_kind() {
			stream.extend(encode(&message).unwrap());
			stream.extend_from_slice(b"state");
		}
		let mut r = Reassembler::new();
		let mut messages = Vec::new();
		let mut text = Vec::new();
		for chunk in chunks(&stream, 7) {
			for piece in r.push(chunk) {
				match piece {
					Piece::Message(m) => messages.push(m),
					Piece::Text(t) => text.extend(t),
					Piece::Error(e) => panic!("{e}"),
				}
			}
		}
		assert_eq!(messages, every_kind());
		assert_eq!(text, b"state".repeat(every_kind().len()));
	}

	#[test]
	fn the_period_floor_is_not_the_codecs_business() {
		let fast = Message::Subscribe(Subscribe {
			sub: 1,
			request_id: 0x7E1,
			response_id: 0x7E9,
			did: 0xF40D,
			period_ms: 0,
		});
		let mut r = Reassembler::new();
		assert_eq!(one_message(r.push(&encode(&fast).unwrap())), fast);
	}

	#[test]
	fn malformed_subscriptions_and_readings_cost_only_their_own_frame() {
		let mut bytes = Vec::new();
		// A subscribe one byte short, and one byte long.
		bytes.extend_from_slice(&[0x00, 0x03, 0x09, 0x00, 1, 0, 0xE1, 0x07, 0xE9, 0x07, 0x0D, 0xF4, 0x14]);
		bytes.extend_from_slice(&[0x00, 0x03, 0x0B, 0x00, 1, 0, 0xE1, 0x07, 0xE9, 0x07, 0x0D, 0xF4, 0x14, 0x00, 0x00]);
		// A subscribe with a response id over 11 bits.
		bytes.extend_from_slice(&[0x00, 0x03, 0x0A, 0x00, 1, 0, 0xE1, 0x07, 0x00, 0x08, 0x0D, 0xF4, 0x14, 0x00]);
		// An unsubscribe of three bytes.
		bytes.extend_from_slice(&[0x00, 0x04, 0x03, 0x00, 1, 0, 0]);
		// A reading without its status.
		bytes.extend_from_slice(&[0x00, 0x05, 0x06, 0x00, 1, 0, 0, 0, 0, 0]);
		// A reading whose data is empty, and one with an unknown status.
		bytes.extend_from_slice(&[0x00, 0x05, 0x07, 0x00, 1, 0, 0, 0, 0, 0, 0]);
		bytes.extend_from_slice(&[0x00, 0x05, 0x07, 0x00, 1, 0, 0, 0, 0, 0, 7]);
		bytes.extend(encode(&subscribe()).unwrap());
		let mut r = Reassembler::new();
		let pieces = r.push(&bytes);
		assert_eq!(pieces.len(), 8, "{pieces:?}");
		assert!(
			pieces[..7].iter().all(|p| matches!(p, Piece::Error(LinkError::Malformed(_)))),
			"{pieces:?}"
		);
		assert_eq!(pieces[7], Piece::Message(subscribe()));
	}

	#[test]
	fn the_encoder_refuses_a_bad_subscription_or_reading() {
		let bad_id = Message::Subscribe(Subscribe {
			sub: 0,
			request_id: 0x800,
			response_id: 0x7E8,
			did: 0xF40D,
			period_ms: 20,
		});
		assert!(matches!(encode(&bad_id), Err(LinkError::Malformed(_))));
		assert!(matches!(encode(&reading(Outcome::Pdu(vec![]))), Err(LinkError::Malformed(_))));
		assert!(matches!(
			encode(&reading(Outcome::Refused("x".repeat(MAX_BODY)))),
			Err(LinkError::Oversize(_))
		));
	}

	#[test]
	fn a_zero_chunk_size_still_makes_progress() {
		assert_eq!(chunks(&[1, 2, 3], 0).count(), 3);
	}

	#[test]
	fn two_messages_in_one_chunk_come_out_in_order() {
		let mut bytes = encode(&request()).unwrap();
		bytes.extend(encode(&answer(Outcome::NoAnswer)).unwrap());
		let mut r = Reassembler::new();
		assert_eq!(r.push(&bytes), vec![Piece::Message(request()), Piece::Message(answer(Outcome::NoAnswer))]);
	}

	#[test]
	fn text_on_its_own_is_passed_through() {
		let mut r = Reassembler::new();
		assert_eq!(r.push(b"set brightness 3"), vec![Piece::Text(b"set brightness 3".to_vec())]);
	}

	#[test]
	fn text_between_messages_is_routed_as_text() {
		let mut bytes = encode(&request()).unwrap();
		bytes.extend_from_slice(b"state");
		bytes.extend(encode(&answer(Outcome::NoAnswer)).unwrap());
		let mut r = Reassembler::new();
		assert_eq!(
			r.push(&bytes),
			vec![
				Piece::Message(request()),
				Piece::Text(b"state".to_vec()),
				Piece::Message(answer(Outcome::NoAnswer))
			]
		);
	}

	#[test]
	fn a_chunk_that_continues_a_frame_is_not_mistaken_for_text() {
		// The second half of a frame starts with a non-NUL byte; it belongs to
		// the frame, not to the text protocol.
		let frame = encode(&request()).unwrap();
		let mut r = Reassembler::new();
		assert!(r.push(&frame[..5]).is_empty());
		assert_eq!(one_message(r.push(&frame[5..])), request());
	}

	#[test]
	fn an_oversize_length_is_rejected_and_the_next_chunk_starts_clean() {
		let mut r = Reassembler::new();
		// Length 4201, then bytes that would have been its body.
		let pieces = r.push(&[0x00, 0x01, 0x69, 0x10, 0xAA, 0xBB]);
		assert_eq!(pieces, vec![Piece::Error(LinkError::Oversize(MAX_BODY + 1))]);
		assert_eq!(one_message(r.push(&encode(&request()).unwrap())), request());
	}

	#[test]
	fn an_oversize_message_is_refused_by_the_encoder() {
		let too_big = Message::Answer(Answer {
			seq: 0,
			outcome: Outcome::Refused("x".repeat(MAX_BODY)),
		});
		assert_eq!(encode(&too_big), Err(LinkError::Oversize(MAX_BODY + 2)));
	}

	#[test]
	fn the_encoder_refuses_what_the_decoder_would_reject() {
		let with = |request_id: u16, response_id: u16, pdu: Vec<u8>| {
			encode(&Message::Request(Request {
				seq: 0,
				request_id,
				response_id,
				pdu,
			}))
		};
		assert!(matches!(with(0x7E0, 0x7E8, vec![]), Err(LinkError::Malformed(_))), "empty pdu");
		assert!(
			matches!(with(0x800, 0x7E8, vec![0x3E, 0x00]), Err(LinkError::Malformed(_))),
			"29-bit request id"
		);
		assert!(
			matches!(with(0x7E0, 0x800, vec![0x3E, 0x00]), Err(LinkError::Malformed(_))),
			"29-bit response id"
		);
		assert!(
			matches!(with(0x7E0, 0x7E8, vec![0; MAX_PDU + 1]), Err(LinkError::Malformed(_))),
			"pdu over ISO-TP's limit"
		);
		assert!(
			matches!(encode(&answer(Outcome::Pdu(vec![]))), Err(LinkError::Malformed(_))),
			"empty answer"
		);
	}

	#[test]
	fn an_unknown_type_is_rejected_and_the_next_chunk_starts_clean() {
		let mut r = Reassembler::new();
		assert_eq!(r.push(&[0x00, 0x7A, 0x02, 0x00, 1, 2]), vec![Piece::Error(LinkError::UnknownType(0x7A))]);
		assert_eq!(one_message(r.push(&encode(&request()).unwrap())), request());
	}

	#[test]
	fn a_malformed_body_costs_only_its_own_frame() {
		let mut bytes = Vec::new();
		// A request body too short to hold its ids.
		bytes.extend_from_slice(&[0x00, 0x01, 0x03, 0x00, 1, 0xE0, 0x07]);
		// An answer with an unknown status.
		bytes.extend_from_slice(&[0x00, 0x02, 0x02, 0x00, 1, 9]);
		// A refusal whose reason is not UTF-8.
		bytes.extend_from_slice(&[0x00, 0x02, 0x03, 0x00, 1, 2, 0xFF]);
		// "No answer" carrying a payload.
		bytes.extend_from_slice(&[0x00, 0x02, 0x03, 0x00, 1, 1, 0xAA]);
		// A request with an 11-bit id out of range.
		bytes.extend_from_slice(&[0x00, 0x01, 0x06, 0x00, 1, 0x00, 0x08, 0xE8, 0x07, 0x3E]);
		// A request with no PDU.
		bytes.extend_from_slice(&[0x00, 0x01, 0x05, 0x00, 1, 0xE0, 0x07, 0xE8, 0x07]);
		// An answer PDU that is empty.
		bytes.extend_from_slice(&[0x00, 0x02, 0x02, 0x00, 1, 0]);
		bytes.extend(encode(&request()).unwrap());
		let mut r = Reassembler::new();
		let pieces = r.push(&bytes);
		assert_eq!(pieces.len(), 8, "{pieces:?}");
		assert!(
			pieces[..7].iter().all(|p| matches!(p, Piece::Error(LinkError::Malformed(_)))),
			"{pieces:?}"
		);
		assert_eq!(pieces[7], Piece::Message(request()));
	}

	#[test]
	fn a_truncated_frame_is_dropped_by_reset() {
		let frame = encode(&request()).unwrap();
		let mut r = Reassembler::new();
		assert!(r.push(&frame[..6]).is_empty());
		r.reset();
		assert_eq!(one_message(r.push(&frame)), request());
	}

	#[test]
	fn garbage_after_a_rejected_header_does_not_poison_later_chunks() {
		let mut r = Reassembler::new();
		assert_eq!(r.push(&[0x00, 0xEE, 0xFF, 0xFF, 0x00, 0x00, 0x00]).len(), 1);
		assert_eq!(r.push(b"get"), vec![Piece::Text(b"get".to_vec())]);
		assert_eq!(one_message(r.push(&encode(&request()).unwrap())), request());
	}
}
