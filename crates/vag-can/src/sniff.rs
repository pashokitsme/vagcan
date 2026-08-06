//! Passive ISO-TP (15765-2) reassembly — reading somebody else's conversation.
//!
//! [`crate::IsoTpCan`] is an *active* transport: it sends a request and awaits
//! the matching response, driving flow control as it goes. A sniffer has no
//! such luxury. It sees an interleaved stream of frames from every participant
//! on the bus, cannot ask for a retransmission, and must never transmit — so it
//! needs its own state machine.
//!
//! This is the component that turns VCDS's **group reads** into readable PDUs.
//! Those responses span multiple frames, which is exactly why they never
//! decoded from the earlier USB captures (`research/labels/rod-labels.md` §4.0b) and
//! why RPM / vehicle speed / coolant could not be fitted.
//!
//! No I/O and no clock of its own: [`IsoTpSniffer::observe_at`] takes the
//! observation time, so the whole state machine is deterministic under test.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// A complete ISO-TP protocol data unit recovered from the bus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnifferPdu {
	/// The CAN id it arrived on (raw form — bit 31 marks a 29-bit id).
	pub id: u32,
	/// The reassembled service payload.
	pub data: Vec<u8>,
	/// How many CAN frames it took: 1 for a single frame, more for a
	/// reassembled multi-frame message.
	pub frames: usize,
}

/// How long an incomplete multi-frame message is kept before it is abandoned.
/// ISO 15765-2's N_Cr (consecutive frame timeout) is 1 s; allow some slack for
/// a busy bus and a host-side observer.
pub const DEFAULT_ASSEMBLY_TIMEOUT: Duration = Duration::from_secs(2);

/// A multi-frame message being assembled on one CAN id.
#[derive(Debug)]
struct Partial {
	data: Vec<u8>,
	expected: usize,
	/// Sequence number the next consecutive frame must carry (wraps at 16).
	next_seq: u8,
	frames: usize,
	last_seen: Instant,
}

/// Passive ISO-TP reassembler. Feed it every frame seen on the bus; it hands
/// back PDUs as they complete.
///
/// Each CAN id is assembled independently, so a tester and several ECUs talking
/// over each other are separated without any configuration.
#[derive(Debug)]
pub struct IsoTpSniffer {
	in_flight: HashMap<u32, Partial>,
	timeout: Duration,
	dropped: usize,
}

impl Default for IsoTpSniffer {
	fn default() -> Self {
		IsoTpSniffer::new()
	}
}

impl IsoTpSniffer {
	pub fn new() -> Self {
		IsoTpSniffer::with_timeout(DEFAULT_ASSEMBLY_TIMEOUT)
	}

	pub fn with_timeout(timeout: Duration) -> Self {
		IsoTpSniffer {
			in_flight: HashMap::new(),
			timeout,
			dropped: 0,
		}
	}

	/// How many partial messages were abandoned: a missing or out-of-order
	/// consecutive frame, a new message starting on top of an unfinished one,
	/// or an assembly that went stale. A non-zero count means the capture has
	/// holes — worth printing at the end of a session.
	pub fn dropped(&self) -> usize {
		self.dropped
	}

	/// Observe one CAN frame, stamped with the current time.
	pub fn observe(&mut self, id: u32, data: &[u8]) -> Option<SnifferPdu> {
		self.observe_at(id, data, Instant::now())
	}

	/// Observe one CAN frame at an explicit time (the testable entry point).
	///
	/// Returns the PDU when this frame completed one. Frames that are not
	/// ISO-TP (or are flow control, which is the tester's business) are ignored.
	pub fn observe_at(&mut self, id: u32, data: &[u8], now: Instant) -> Option<SnifferPdu> {
		self.expire(now);

		let pci = *data.first()?;
		match pci >> 4 {
			0x0 => self.single_frame(id, data),
			0x1 => {
				self.first_frame(id, data, now);
				None
			}
			0x2 => self.consecutive_frame(id, data, now),
			// Flow control: the tester's half of the handshake, carries no data.
			_ => None,
		}
	}

	fn single_frame(&mut self, id: u32, data: &[u8]) -> Option<SnifferPdu> {
		let len = (data[0] & 0x0F) as usize;
		// Length 0 is the CAN-FD escape form; this bus is classic CAN.
		if len == 0 || len > data.len() - 1 {
			// Not a frame we can read, so it is not evidence of anything —
			// notably not a reason to throw away an assembly in progress.
			return None;
		}
		// ISO 15765-2: a single frame terminates any assembly in flight on that
		// id. Keeping the partial would let a consecutive frame from the *next*
		// message — whose first frame we then missed — be stitched onto it and
		// emitted as a PDU that was never on the bus.
		if self.in_flight.remove(&id).is_some() {
			self.dropped += 1;
		}
		Some(SnifferPdu {
			id,
			data: data[1..1 + len].to_vec(),
			frames: 1,
		})
	}

	fn first_frame(&mut self, id: u32, data: &[u8], now: Instant) {
		if data.len() < 2 {
			return;
		}
		let expected = ((data[0] & 0x0F) as usize) << 8 | data[1] as usize;
		if expected <= 7 {
			// A message that short would have been a single frame; treat the
			// frame as noise rather than starting an assembly that can never
			// be satisfied.
			return;
		}
		// A first frame on an id that was mid-assembly means the previous
		// message will never finish.
		if self.in_flight.contains_key(&id) {
			self.dropped += 1;
		}
		self.in_flight.insert(
			id,
			Partial {
				data: data[2..].to_vec(),
				expected,
				next_seq: 1,
				frames: 1,
				last_seen: now,
			},
		);
	}

	/// Known residual hole, deliberately not chased: if a first frame is lost
	/// while an older assembly on the same id is still within the timeout, that
	/// message's `seq == 1` grafts onto the stale partial. The two cases are
	/// indistinguishable in-band — a legitimate `FF, CF1` pair looks exactly the
	/// same, and the gap between them is bounded only by the tester's flow
	/// control (N_Bs, block size, STmin), so no timing rule separates them
	/// without discarding real messages on a busy bus. It takes two losses at
	/// once — a first frame dropped on an id whose previous message was itself
	/// left unfinished — and the result is a PDU truncated or extended to the
	/// stale header's declared length, which the decoder above sees as garbage
	/// rather than as a plausible reading.
	fn consecutive_frame(&mut self, id: u32, data: &[u8], now: Instant) -> Option<SnifferPdu> {
		let seq = data[0] & 0x0F;
		let partial = self.in_flight.get_mut(&id)?;

		if seq != partial.next_seq {
			// A gap. We cannot ask for a retransmission, so the message is
			// lost — dropping it beats emitting silently corrupted bytes.
			self.in_flight.remove(&id);
			self.dropped += 1;
			return None;
		}

		let want = partial.expected - partial.data.len();
		let take = want.min(data.len() - 1);
		partial.data.extend_from_slice(&data[1..1 + take]);
		partial.next_seq = (partial.next_seq + 1) & 0x0F;
		partial.frames += 1;
		partial.last_seen = now;

		if partial.data.len() >= partial.expected {
			let done = self.in_flight.remove(&id)?;
			return Some(SnifferPdu {
				id,
				data: done.data,
				frames: done.frames,
			});
		}
		None
	}

	/// Abandon assemblies whose last frame is older than the timeout.
	fn expire(&mut self, now: Instant) {
		let timeout = self.timeout;
		let before = self.in_flight.len();
		self.in_flight.retain(|_, p| now.duration_since(p.last_seen) < timeout);
		self.dropped += before - self.in_flight.len();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A fixed clock base; tests advance it explicitly.
	fn t0() -> Instant {
		Instant::now()
	}

	#[test]
	fn single_frame_emits_immediately() {
		let mut s = IsoTpSniffer::new();
		// `03 22 F1 90` = a 3-byte PDU: RDBI of DID F190, padded to 8.
		let pdu = s
			.observe_at(0x7E0, &[0x03, 0x22, 0xF1, 0x90, 0, 0, 0, 0], t0())
			.expect("single frame completes at once");
		assert_eq!(pdu.data, vec![0x22, 0xF1, 0x90]);
		assert_eq!(pdu.frames, 1);
		assert_eq!(s.dropped(), 0);
	}

	#[test]
	fn multi_frame_response_is_reassembled_and_trimmed_to_length() {
		// The shape of a VCDS group read: 17 bytes over three frames, the last
		// of which is padded out to 8 with 0xAA.
		let mut s = IsoTpSniffer::new();
		let now = t0();
		assert_eq!(s.observe_at(0x7E8, &[0x10, 0x11, 0x62, 0xF1, 0x90, 1, 2, 3], now), None);
		assert_eq!(s.observe_at(0x7E8, &[0x21, 4, 5, 6, 7, 8, 9, 10], now), None);
		let pdu = s
			.observe_at(0x7E8, &[0x22, 11, 12, 13, 14, 0xAA, 0xAA, 0xAA], now)
			.expect("third frame completes the message");

		assert_eq!(pdu.frames, 3);
		assert_eq!(pdu.data.len(), 0x11, "trimmed to the declared length");
		assert_eq!(&pdu.data[..3], &[0x62, 0xF1, 0x90]);
		// The trailing padding of the last frame must not leak into the PDU.
		assert_eq!(pdu.data.last(), Some(&14));
	}

	#[test]
	fn two_ecus_talking_at_once_are_assembled_independently() {
		let mut s = IsoTpSniffer::new();
		let now = t0();
		// Engine and gearbox responses interleave frame by frame.
		s.observe_at(0x7E8, &[0x10, 0x09, 0xE1, 1, 2, 3, 4, 5], now);
		s.observe_at(0x7E9, &[0x10, 0x09, 0xE2, 9, 8, 7, 6, 5], now);
		let a = s.observe_at(0x7E8, &[0x21, 6, 7, 0, 0, 0, 0, 0], now).expect("engine done");
		let b = s.observe_at(0x7E9, &[0x21, 4, 3, 0, 0, 0, 0, 0], now).expect("gearbox done");

		assert_eq!(a.id, 0x7E8);
		assert_eq!(a.data, vec![0xE1, 1, 2, 3, 4, 5, 6, 7, 0][..9].to_vec());
		assert_eq!(b.id, 0x7E9);
		assert_eq!(b.data, vec![0xE2, 9, 8, 7, 6, 5, 4, 3, 0][..9].to_vec());
		assert_eq!(s.dropped(), 0);
	}

	#[test]
	fn a_missing_consecutive_frame_drops_the_message() {
		let mut s = IsoTpSniffer::new();
		let now = t0();
		s.observe_at(0x7E8, &[0x10, 0x13, 0x62, 0xF1, 0x90, 1, 2, 3], now);
		// Sequence jumps 1 -> 2: frame 1 was missed.
		assert_eq!(s.observe_at(0x7E8, &[0x22, 4, 5, 6, 7, 8, 9, 10], now), None);
		assert_eq!(s.dropped(), 1, "no silently corrupted PDU is emitted");

		// Later frames of the dead message do not resurrect it.
		assert_eq!(s.observe_at(0x7E8, &[0x23, 11, 12, 13, 14, 0, 0, 0], now), None);
	}

	#[test]
	fn a_new_first_frame_abandons_the_unfinished_one() {
		let mut s = IsoTpSniffer::new();
		let now = t0();
		s.observe_at(0x7E8, &[0x10, 0x13, 0x62, 0xF1, 0x90, 1, 2, 3], now);
		s.observe_at(0x7E8, &[0x10, 0x09, 0x62, 0xA0, 0x58, 0x55, 0x55, 0], now);
		assert_eq!(s.dropped(), 1);

		let pdu = s.observe_at(0x7E8, &[0x21, 0, 0, 0, 0, 0, 0, 0], now).expect("second finishes");
		assert_eq!(pdu.data.len(), 9);
		assert_eq!(&pdu.data[..3], &[0x62, 0xA0, 0x58]);
	}

	#[test]
	fn a_single_frame_abandons_the_unfinished_message_on_its_id() {
		// ISO 15765-2: a single frame terminates whatever was being assembled
		// on that id. Leaving the partial alive lets a later consecutive frame
		// — which belongs to a message we never saw the start of — be stitched
		// onto it, producing a PDU that was never on the bus.
		let mut s = IsoTpSniffer::new();
		let now = t0();
		s.observe_at(0x7E8, &[0x10, 0x13, 0x62, 0xF1, 0x90, 1, 2, 3], now);

		let pdu = s
			.observe_at(0x7E8, &[0x03, 0x7F, 0x22, 0x78, 0, 0, 0, 0], now)
			.expect("the single frame is still emitted");
		assert_eq!(pdu.data, vec![0x7F, 0x22, 0x78]);
		assert_eq!(s.dropped(), 1, "the interrupted assembly is accounted for");

		// The continuation of the dead message must not resurrect it.
		assert_eq!(s.observe_at(0x7E8, &[0x21, 4, 5, 6, 7, 8, 9, 10], now), None);
		assert_eq!(s.dropped(), 1, "nothing new was lost — it was already gone");
	}

	#[test]
	fn a_malformed_single_frame_leaves_the_assembly_alone() {
		// Only a well-formed single frame is evidence that the ECU moved on.
		// A frame we cannot even parse is more likely line noise than a new
		// message, and killing a good assembly over it would lose real data.
		let mut s = IsoTpSniffer::new();
		let now = t0();
		s.observe_at(0x7E8, &[0x10, 0x09, 0x62, 0xA0, 0x58, 0x55, 0x55, 0], now);
		assert_eq!(s.observe_at(0x7E8, &[0x07, 0x22, 0xF1], now), None);
		assert_eq!(s.dropped(), 0);

		let pdu = s.observe_at(0x7E8, &[0x21, 0, 0, 0, 0, 0, 0, 0], now).expect("still assembling");
		assert_eq!(pdu.data.len(), 9);
	}

	#[test]
	fn a_stale_assembly_is_abandoned() {
		let mut s = IsoTpSniffer::with_timeout(Duration::from_secs(2));
		let now = t0();
		s.observe_at(0x7E8, &[0x10, 0x13, 0x62, 0xF1, 0x90, 1, 2, 3], now);
		// Three seconds later the continuation is far too late to trust.
		let late = now + Duration::from_secs(3);
		assert_eq!(s.observe_at(0x7E8, &[0x21, 4, 5, 6, 7, 8, 9, 10], late), None);
		assert_eq!(s.dropped(), 1);
	}

	#[test]
	fn flow_control_and_junk_are_ignored() {
		let mut s = IsoTpSniffer::new();
		let now = t0();
		// Flow control from the tester.
		assert_eq!(s.observe_at(0x7E0, &[0x30, 0x00, 0x00, 0, 0, 0, 0, 0], now), None);
		// Empty frame.
		assert_eq!(s.observe_at(0x7E0, &[], now), None);
		// Single frame claiming more data than the frame holds.
		assert_eq!(s.observe_at(0x7E0, &[0x07, 0x22, 0xF1], now), None);
		// CAN-FD escape single frame (length nibble 0) — not this bus.
		assert_eq!(s.observe_at(0x7E0, &[0x00, 0x10, 0x22, 0xF1, 0x90, 0, 0, 0], now), None);
		assert_eq!(s.dropped(), 0);
	}

	#[test]
	fn consecutive_frame_without_a_first_frame_is_ignored() {
		// Sniffing starts mid-conversation all the time.
		let mut s = IsoTpSniffer::new();
		assert_eq!(s.observe_at(0x7E8, &[0x21, 1, 2, 3, 4, 5, 6, 7], t0()), None);
		assert_eq!(s.dropped(), 0, "never saw the start, so nothing was lost");
	}

	#[test]
	fn sequence_number_wraps_past_fifteen() {
		// A long response (>16 frames) exercises the 4-bit sequence wrap.
		let mut s = IsoTpSniffer::new();
		let now = t0();
		let total = 130usize;
		s.observe_at(0x7E8, &[0x10, total as u8, 1, 2, 3, 4, 5, 6], now);
		let mut seq = 1u8;
		let mut pdu = None;
		while pdu.is_none() {
			let frame = [0x20 | seq, 9, 9, 9, 9, 9, 9, 9];
			pdu = s.observe_at(0x7E8, &frame, now);
			seq = (seq + 1) & 0x0F;
		}
		let pdu = pdu.unwrap();
		assert_eq!(pdu.data.len(), total);
		assert_eq!(s.dropped(), 0);
	}
}
