//! What a host across a radio may ask of the car, enforced by the board.
//!
//! The dash board is always visible over BLE and does no pairing, so whoever is
//! on the other end is not trusted: every request it relays passes through a
//! [`Guard`] first. A check the host makes is a courtesy, never the enforcement.
//! The rules are `todo/dash/16-uds-over-ble.md`, "The board's own guards":
//!
//! - **Services:** only the read-only allowlist `0x22 0x19 0x10 0x3E` (the same
//!   list the client refuses to leave).
//! - **Sessions:** `10 02` (programming) is refused outright; `10 01` (default)
//!   is always allowed, being the safe direction; any other session change is
//!   forwarded only after the board has just read road speed `0` from the engine
//!   (`22 F40D` on `7E0`/`7E8`). No answer, a negative answer, or any other
//!   speed counts as moving.
//! - **Rate:** at most [`RATE_LIMIT`] counted units in any [`RATE_WINDOW_MS`].
//!   An identifier in a `0x22` request is one unit each, any other request one.
//!   Over the cap the request **waits** ([`Verdict::WaitUntil`]); it is slowed,
//!   never dropped.
//! - **Sweeps:** at most [`MAX_IDENTIFIERS_PER_REQUEST`] identifiers in one
//!   `0x22`; three identifiers in a row to one unit that step by the same
//!   non-zero stride (`n, n+1, n+2` or `n, n+k, n+2k`) are a walk, refused, and
//!   lock `0x22` to that unit for the rest of the connection; at most
//!   [`MAX_DISTINCT_IDENTIFIERS`] different identifiers per unit per connection.
//!
//! One `Guard` per connection; dropping it is the reset. No clock inside — the
//! caller passes milliseconds from any monotonic source.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;

use crate::pdu::READ_ONLY_ALLOWLIST;

/// Counted units allowed in any [`RATE_WINDOW_MS`]. A starting figure, not measured.
pub const RATE_LIMIT: u32 = 20;
/// The sliding window the rate cap is taken over.
pub const RATE_WINDOW_MS: u64 = 10_000;
/// Identifiers allowed in one `0x22` request.
pub const MAX_IDENTIFIERS_PER_REQUEST: usize = 4;
/// Different identifiers one unit may be asked for in one connection. A starting figure.
pub const MAX_DISTINCT_IDENTIFIERS: usize = 32;

/// The engine's request id on the ISO 15765-4 address block.
pub const SPEED_REQUEST_ID: u16 = 0x7E0;
/// The engine's response id on the ISO 15765-4 address block.
pub const SPEED_RESPONSE_ID: u16 = 0x7E8;
/// Road speed: SAE J1979 PID `0x0D` at its UDS mirror `0xF400 + PID`, one byte of km/h.
pub const SPEED_REQUEST: [u8; 3] = [0x22, 0xF4, 0x0D];

const SESSION_CONTROL: u8 = 0x10;
const READ_BY_IDENTIFIER: u8 = 0x22;
const DEFAULT_SESSION: u8 = 0x01;
const PROGRAMMING_SESSION: u8 = 0x02;
/// ISO 14229-1: bit 7 of a subfunction only suppresses the positive response.
const SUPPRESS_POSITIVE_RESPONSE: u8 = 0x80;

/// What the board does with one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
	/// Send it, then call [`Guard::forwarded`].
	Forward,
	/// Over the rate cap: ask again at this time (ms).
	WaitUntil(u64),
	/// Read road speed now — [`SPEED_REQUEST`] to [`SPEED_REQUEST_ID`] — and
	/// hand the result to [`Guard::speed`].
	CheckSpeedFirst,
	/// Do not send it; answer the host with [`Refusal::reason`].
	Refuse(Refusal),
}

/// Why a request was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
	/// No bytes at all.
	Empty,
	/// A service outside the read-only allowlist.
	ServiceNotAllowed(u8),
	/// A session request that is not exactly `10 xx`.
	MalformedSession,
	/// `10 02`.
	ProgrammingSession,
	/// The engine gave no road speed, or a negative answer.
	SpeedUnknown,
	/// The engine reported this speed, in km/h.
	Moving(u8),
	/// A `0x22` request that is not `22` and one or more whole identifiers.
	MalformedIdentifiers,
	/// More than [`MAX_IDENTIFIERS_PER_REQUEST`] identifiers in one request.
	TooManyIdentifiers,
	/// The request completes a walk; `0x22` to this unit is now locked.
	Walk,
	/// `0x22` to this unit was locked by an earlier walk.
	Locked,
	/// The unit would be asked for more than [`MAX_DISTINCT_IDENTIFIERS`] different identifiers.
	TooManyDistinct,
}

impl Refusal {
	/// Short, plain English, for the host to print.
	pub fn reason(&self) -> &'static str {
		match self {
			Refusal::Empty => "empty request",
			Refusal::ServiceNotAllowed(_) => "service not allowed: this link only reads",
			Refusal::MalformedSession => "a session request is two bytes",
			Refusal::ProgrammingSession => "programming session is never allowed over this link",
			Refusal::SpeedUnknown => "session change refused: the engine did not report road speed",
			Refusal::Moving(_) => "session change refused: the car is moving",
			Refusal::MalformedIdentifiers => "a read needs whole two-byte identifiers",
			Refusal::TooManyIdentifiers => "more than 4 identifiers in one read",
			Refusal::Walk => "identifiers asked in a row: reads of this unit locked until reconnect",
			Refusal::Locked => "reads of this unit are locked until reconnect",
			Refusal::TooManyDistinct => "more than 32 different identifiers from this unit",
		}
	}
}

/// Decode an engine's answer to [`SPEED_REQUEST`]: `62 F4 0D xx` is `xx` km/h,
/// anything else — a negative answer, a short or long one — is `None`. Pass the
/// final answer, after any `7F 22 78` (response pending).
pub fn road_speed(answer: &[u8]) -> Option<u8> {
	match answer {
		[0x62, id_hi, id_lo, kmh] if [*id_hi, *id_lo] == SPEED_REQUEST[1..] => Some(*kmh),
		_ => None,
	}
}

/// What the static rules made of a request that passed them.
struct Admitted {
	/// Units it counts toward the rate cap.
	cost: u32,
	/// Whether it needs a speed read first.
	needs_speed: bool,
}

/// One connection's view of what has been asked.
#[derive(Debug, Default)]
pub struct Guard {
	/// `(when, units)` of everything that reached the car, oldest first.
	window: VecDeque<(u64, u32)>,
	units: BTreeMap<u16, UnitHistory>,
}

#[derive(Debug, Default)]
struct UnitHistory {
	/// The last two identifiers forwarded to this unit, oldest first.
	recent: Vec<u16>,
	/// Every different identifier forwarded to this unit; at most
	/// [`MAX_DISTINCT_IDENTIFIERS`], so a list is smaller than a set.
	asked: Vec<u16>,
	locked: bool,
}

impl Guard {
	pub fn new() -> Self {
		Self::default()
	}

	/// Decide what to do with one request from the host.
	pub fn check(&mut self, now_ms: u64, request_id: u16, pdu: &[u8]) -> Verdict {
		let admitted = match self.admit(request_id, pdu) {
			Ok(admitted) => admitted,
			Err(refusal) => return Verdict::Refuse(refusal),
		};
		// A speed read is a request too: there has to be room for it and for the
		// session change it clears.
		let need = admitted.cost + u32::from(admitted.needs_speed);
		match self.wait(now_ms, need) {
			Some(until) => Verdict::WaitUntil(until),
			None if admitted.needs_speed => Verdict::CheckSpeedFirst,
			None => Verdict::Forward,
		}
	}

	/// The road speed the guard asked for: `Some(kmh)` from a positive answer
	/// (see [`road_speed`]), `None` for no answer or a negative one.
	///
	/// Call it after [`Guard::check`] answered [`Verdict::CheckSpeedFirst`], with
	/// the read made just then. The read itself reached the car, so it counts one
	/// unit toward the rate cap whatever is decided.
	pub fn speed(&mut self, now_ms: u64, request_id: u16, pdu: &[u8], kmh: Option<u8>) -> Verdict {
		self.window.push_back((now_ms, 1));
		let admitted = match self.admit(request_id, pdu) {
			Ok(admitted) => admitted,
			Err(refusal) => return Verdict::Refuse(refusal),
		};
		if !admitted.needs_speed {
			return self.check(now_ms, request_id, pdu);
		}
		match kmh {
			None => Verdict::Refuse(Refusal::SpeedUnknown),
			Some(0) => match self.wait(now_ms, admitted.cost) {
				Some(until) => Verdict::WaitUntil(until),
				None => Verdict::Forward,
			},
			Some(moving) => Verdict::Refuse(Refusal::Moving(moving)),
		}
	}

	/// Record a request that was actually sent. Refused and delayed requests are
	/// never passed here, so they do not count.
	pub fn forwarded(&mut self, now_ms: u64, request_id: u16, pdu: &[u8]) {
		if pdu.first() != Some(&READ_BY_IDENTIFIER) {
			self.window.push_back((now_ms, 1));
			return;
		}
		let dids = identifiers(pdu);
		self.window.push_back((now_ms, dids.len() as u32));
		let unit = self.units.entry(request_id).or_default();
		for did in dids {
			if !unit.asked.contains(&did) {
				unit.asked.push(did);
			}
			unit.recent.push(did);
		}
		let excess = unit.recent.len().saturating_sub(2);
		unit.recent.drain(..excess);
	}

	/// The rules that do not depend on time. Refusing a walk locks the unit.
	fn admit(&mut self, request_id: u16, pdu: &[u8]) -> Result<Admitted, Refusal> {
		let (&sid, rest) = pdu.split_first().ok_or(Refusal::Empty)?;
		if !READ_ONLY_ALLOWLIST.contains(&sid) {
			return Err(Refusal::ServiceNotAllowed(sid));
		}
		match sid {
			SESSION_CONTROL => {
				let [session] = rest else {
					return Err(Refusal::MalformedSession);
				};
				match session & !SUPPRESS_POSITIVE_RESPONSE {
					PROGRAMMING_SESSION => Err(Refusal::ProgrammingSession),
					DEFAULT_SESSION => Ok(Admitted { cost: 1, needs_speed: false }),
					_ => Ok(Admitted { cost: 1, needs_speed: true }),
				}
			}
			READ_BY_IDENTIFIER => {
				if rest.is_empty() || rest.len() % 2 != 0 {
					return Err(Refusal::MalformedIdentifiers);
				}
				let dids = identifiers(pdu);
				// A unit gets an entry only once something is forwarded to it or it is
				// locked, so refusals cost the board no memory.
				let (recent, asked) = match self.units.get(&request_id) {
					Some(unit) if unit.locked => return Err(Refusal::Locked),
					Some(unit) => (unit.recent.as_slice(), unit.asked.as_slice()),
					None => (&[][..], &[][..]),
				};
				if dids.len() > MAX_IDENTIFIERS_PER_REQUEST {
					return Err(Refusal::TooManyIdentifiers);
				}
				if walks(recent, &dids) {
					self.units.entry(request_id).or_default().locked = true;
					return Err(Refusal::Walk);
				}
				let mut fresh: Vec<u16> = dids.iter().copied().filter(|did| !asked.contains(did)).collect();
				fresh.sort_unstable();
				fresh.dedup();
				if asked.len() + fresh.len() > MAX_DISTINCT_IDENTIFIERS {
					return Err(Refusal::TooManyDistinct);
				}
				Ok(Admitted {
					cost: dids.len() as u32,
					needs_speed: false,
				})
			}
			_ => Ok(Admitted { cost: 1, needs_speed: false }),
		}
	}

	/// When `need` more units fit under the cap, or `None` if they fit now.
	fn wait(&mut self, now_ms: u64, need: u32) -> Option<u64> {
		let expiry = |at: u64| at.saturating_add(RATE_WINDOW_MS);
		// `retain` rather than popping the front: after a clock that stepped back,
		// an expired entry can sit behind a live one and must not go on counting.
		self.window.retain(|&(at, _)| expiry(at) > now_ms);
		let used: u32 = self.window.iter().map(|&(_, units)| units).sum();
		if used + need <= RATE_LIMIT {
			return None;
		}
		let mut left = used;
		for &(at, units) in &self.window {
			left -= units;
			if left + need <= RATE_LIMIT {
				return Some(expiry(at));
			}
		}
		// Unreachable while a request costs at most RATE_LIMIT; the empty window is the answer.
		self.window.back().map(|&(at, _)| expiry(at))
	}
}

/// The identifiers of a `0x22` request, in order. Empty for anything else.
fn identifiers(pdu: &[u8]) -> Vec<u16> {
	match pdu.split_first() {
		Some((&READ_BY_IDENTIFIER, rest)) => rest.chunks_exact(2).map(|pair| u16::from_be_bytes([pair[0], pair[1]])).collect(),
		_ => Vec::new(),
	}
}

/// Whether asking `next` after `recent` makes any three identifiers in a row
/// step by one fixed, non-zero stride.
fn walks(recent: &[u16], next: &[u16]) -> bool {
	let order: Vec<i32> = recent.iter().chain(next).map(|&did| i32::from(did)).collect();
	order.windows(3).any(|w| w[1] - w[0] == w[2] - w[1] && w[1] != w[0])
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::vec;

	const ENGINE: u16 = 0x7E0;
	const GEARBOX: u16 = 0x7E1;
	const GATEWAY: u16 = 0x710;

	fn rdbi(dids: &[u16]) -> Vec<u8> {
		let mut pdu = vec![0x22];
		for did in dids {
			pdu.extend_from_slice(&did.to_be_bytes());
		}
		pdu
	}

	/// Take one request through the guard the way the board does: wait when
	/// told to, read speed when told to (the engine answering `kmh`), send when
	/// allowed. Each exchange with the car takes 30 ms. Returns the refusal, if any.
	fn drive(guard: &mut Guard, now: &mut u64, request_id: u16, pdu: &[u8], kmh: Option<u8>) -> Result<(), Refusal> {
		for _ in 0..8 {
			let verdict = match guard.check(*now, request_id, pdu) {
				Verdict::CheckSpeedFirst => {
					*now += 30;
					guard.speed(*now, request_id, pdu, kmh)
				}
				other => other,
			};
			match verdict {
				Verdict::Forward => {
					guard.forwarded(*now, request_id, pdu);
					*now += 30;
					return Ok(());
				}
				Verdict::WaitUntil(t) => {
					assert!(t > *now, "a wait must be in the future: {t} at {now}");
					*now = t;
				}
				Verdict::Refuse(r) => return Err(r),
				Verdict::CheckSpeedFirst => panic!("speed() asked for speed again"),
			}
		}
		panic!("request {pdu:02X?} to {request_id:03X} never went out");
	}

	fn forward_now(guard: &mut Guard, now: u64, request_id: u16, pdu: &[u8]) {
		assert_eq!(guard.check(now, request_id, pdu), Verdict::Forward, "{pdu:02X?}");
		guard.forwarded(now, request_id, pdu);
	}

	// --- services -------------------------------------------------------------

	#[test]
	fn an_empty_request_is_refused() {
		assert_eq!(Guard::new().check(0, ENGINE, &[]), Verdict::Refuse(Refusal::Empty));
	}

	#[test]
	fn services_outside_the_allowlist_are_refused() {
		let mut guard = Guard::new();
		for sid in [0x14, 0x2E, 0x27, 0x31, 0x34, 0x11, 0x28, 0x85] {
			assert_eq!(guard.check(0, ENGINE, &[sid, 0x00]), Verdict::Refuse(Refusal::ServiceNotAllowed(sid)));
		}
	}

	#[test]
	fn fault_reads_and_tester_present_pass_on_allowlist_and_rate_alone() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &[0x19, 0x02, 0xFF]);
		forward_now(&mut guard, 0, ENGINE, &[0x19, 0x06, 0x00, 0x01, 0x29, 0xFF]);
		forward_now(&mut guard, 0, ENGINE, &[0x3E, 0x00]);
	}

	// --- sessions -------------------------------------------------------------

	#[test]
	fn the_programming_session_is_refused_whatever_the_car_is_doing() {
		let mut guard = Guard::new();
		assert_eq!(guard.check(0, ENGINE, &[0x10, 0x02]), Verdict::Refuse(Refusal::ProgrammingSession));
		assert_eq!(
			guard.check(0, ENGINE, &[0x10, 0x82]),
			Verdict::Refuse(Refusal::ProgrammingSession),
			"the suppress-response bit does not disguise it"
		);
		assert_eq!(
			guard.speed(0, ENGINE, &[0x10, 0x02], Some(0)),
			Verdict::Refuse(Refusal::ProgrammingSession),
			"not even on a stationary car"
		);
	}

	#[test]
	fn the_default_session_is_always_allowed_without_a_speed_read() {
		let mut guard = Guard::new();
		assert_eq!(guard.check(0, ENGINE, &[0x10, 0x01]), Verdict::Forward);
		assert_eq!(guard.check(0, ENGINE, &[0x10, 0x81]), Verdict::Forward);
	}

	#[test]
	fn a_session_request_that_is_not_two_bytes_is_refused() {
		let mut guard = Guard::new();
		assert_eq!(guard.check(0, ENGINE, &[0x10]), Verdict::Refuse(Refusal::MalformedSession));
		assert_eq!(guard.check(0, ENGINE, &[0x10, 0x03, 0x00]), Verdict::Refuse(Refusal::MalformedSession));
	}

	#[test]
	fn another_session_change_asks_for_road_speed_first() {
		let mut guard = Guard::new();
		assert_eq!(guard.check(0, ENGINE, &[0x10, 0x03]), Verdict::CheckSpeedFirst);
		assert_eq!(guard.check(0, 0x713, &[0x10, 0x03]), Verdict::CheckSpeedFirst);
		assert_eq!(guard.check(0, 0x713, &[0x10, 0x40]), Verdict::CheckSpeedFirst);
	}

	#[test]
	fn a_session_change_on_a_stationary_car_is_forwarded() {
		assert_eq!(Guard::new().speed(0, 0x713, &[0x10, 0x03], Some(0)), Verdict::Forward);
	}

	#[test]
	fn a_session_change_on_a_moving_car_is_refused_with_its_speed() {
		assert_eq!(
			Guard::new().speed(0, 0x713, &[0x10, 0x03], Some(12)),
			Verdict::Refuse(Refusal::Moving(12))
		);
	}

	#[test]
	fn a_session_change_without_a_speed_answer_counts_as_moving() {
		let mut guard = Guard::new();
		assert_eq!(guard.speed(0, 0x713, &[0x10, 0x03], None), Verdict::Refuse(Refusal::SpeedUnknown));
		// A negative answer decodes to no speed, and is refused the same way.
		assert_eq!(
			guard.speed(0, 0x713, &[0x10, 0x03], road_speed(&[0x7F, 0x22, 0x31])),
			Verdict::Refuse(Refusal::SpeedUnknown)
		);
	}

	#[test]
	fn moving_and_unknown_speed_are_told_apart_in_the_reason() {
		assert_ne!(Refusal::Moving(5).reason(), Refusal::SpeedUnknown.reason());
		assert!(Refusal::SpeedUnknown.reason().contains("speed"));
		assert!(Refusal::Moving(5).reason().contains("moving"));
	}

	#[test]
	fn road_speed_decodes_only_the_positive_answer_to_f40d() {
		assert_eq!(road_speed(&[0x62, 0xF4, 0x0D, 0x00]), Some(0));
		assert_eq!(road_speed(&[0x62, 0xF4, 0x0D, 57]), Some(57));
		assert_eq!(road_speed(&[0x7F, 0x22, 0x31]), None, "negative answer");
		assert_eq!(road_speed(&[0x62, 0xF4, 0x0C, 0x00]), None, "another identifier");
		assert_eq!(road_speed(&[0x62, 0xF4, 0x0D]), None, "no value");
		assert_eq!(road_speed(&[0x62, 0xF4, 0x0D, 0x00, 0x00]), None, "not the one-byte parameter");
		assert_eq!(road_speed(&[]), None);
	}

	#[test]
	fn the_speed_read_counts_toward_the_rate_cap() {
		// The read reaches the car whether or not the session change is then
		// refused, so a host repeating a refused session change cannot make the
		// board read the engine without limit.
		let mut guard = Guard::new();
		// Each check reserves room for the read and the request, so nineteen
		// reads fill the window as far as a twentieth session change can see.
		for _ in 0..RATE_LIMIT - 1 {
			assert_eq!(guard.check(0, 0x713, &[0x10, 0x03]), Verdict::CheckSpeedFirst);
			assert_eq!(guard.speed(0, 0x713, &[0x10, 0x03], Some(40)), Verdict::Refuse(Refusal::Moving(40)));
		}
		assert_eq!(guard.check(0, 0x713, &[0x10, 0x03]), Verdict::WaitUntil(RATE_WINDOW_MS));
	}

	#[test]
	fn a_session_change_waits_until_there_is_room_for_its_speed_read_too() {
		let mut guard = Guard::new();
		for _ in 0..RATE_LIMIT - 1 {
			forward_now(&mut guard, 0, ENGINE, &[0x3E, 0x00]);
		}
		assert_eq!(guard.check(0, ENGINE, &[0x10, 0x03]), Verdict::WaitUntil(RATE_WINDOW_MS));
		assert_eq!(guard.check(0, ENGINE, &[0x10, 0x01]), Verdict::Forward, "one unit still fits");
	}

	// --- rate -----------------------------------------------------------------

	#[test]
	fn the_twenty_first_unit_in_ten_seconds_waits_for_the_oldest_to_expire() {
		let mut guard = Guard::new();
		for i in 0..RATE_LIMIT as u64 {
			forward_now(&mut guard, i * 100, ENGINE, &[0x3E, 0x00]);
		}
		assert_eq!(guard.check(2_000, ENGINE, &[0x3E, 0x00]), Verdict::WaitUntil(RATE_WINDOW_MS));
		assert_eq!(guard.check(9_999, ENGINE, &[0x3E, 0x00]), Verdict::WaitUntil(RATE_WINDOW_MS));
		assert_eq!(guard.check(10_000, ENGINE, &[0x3E, 0x00]), Verdict::Forward);
	}

	#[test]
	fn refused_and_delayed_requests_do_not_count_toward_the_rate() {
		let mut guard = Guard::new();
		for _ in 0..100 {
			assert!(matches!(guard.check(0, ENGINE, &[0x2E, 0xF1, 0x90]), Verdict::Refuse(_)));
			// Asked, never sent.
			let _ = guard.check(0, ENGINE, &[0x3E, 0x00]);
		}
		assert_eq!(guard.check(0, ENGINE, &[0x3E, 0x00]), Verdict::Forward);
	}

	#[test]
	fn identifiers_inside_one_request_each_count_toward_the_rate() {
		let mut guard = Guard::new();
		// Four identifiers that do not walk, five times: twenty units.
		let batch = rdbi(&[0xF190, 0xF187, 0xF197, 0xF19E]);
		for _ in 0..5 {
			forward_now(&mut guard, 0, ENGINE, &batch);
		}
		assert_eq!(guard.check(0, GEARBOX, &[0x3E, 0x00]), Verdict::WaitUntil(RATE_WINDOW_MS));
	}

	#[test]
	fn a_request_above_the_remaining_budget_waits_even_if_a_smaller_one_would_fit() {
		let mut guard = Guard::new();
		for i in 0..18u64 {
			forward_now(&mut guard, i, ENGINE, &[0x3E, 0x00]);
		}
		// 18 used: four identifiers need the two oldest gone, at 0 and 1 ms.
		assert_eq!(
			guard.check(18, GEARBOX, &rdbi(&[0xF190, 0xF187, 0xF197, 0xF19E])),
			Verdict::WaitUntil(10_001)
		);
		assert_eq!(guard.check(18, GEARBOX, &rdbi(&[0xF190, 0xF187])), Verdict::Forward);
	}

	// --- identifier requests ----------------------------------------------------

	#[test]
	fn a_read_that_is_not_whole_identifiers_is_refused() {
		let mut guard = Guard::new();
		assert_eq!(guard.check(0, ENGINE, &[0x22]), Verdict::Refuse(Refusal::MalformedIdentifiers));
		assert_eq!(guard.check(0, ENGINE, &[0x22, 0xF1]), Verdict::Refuse(Refusal::MalformedIdentifiers));
		assert_eq!(
			guard.check(0, ENGINE, &[0x22, 0xF1, 0x90, 0xF1]),
			Verdict::Refuse(Refusal::MalformedIdentifiers)
		);
	}

	#[test]
	fn more_than_four_identifiers_in_one_request_is_refused() {
		let mut guard = Guard::new();
		assert_eq!(
			guard.check(0, ENGINE, &rdbi(&[0xF190, 0xF187, 0xF197, 0xF19E, 0xF1A2])),
			Verdict::Refuse(Refusal::TooManyIdentifiers)
		);
		assert_eq!(guard.check(0, ENGINE, &rdbi(&[0xF190, 0xF187, 0xF197, 0xF19E])), Verdict::Forward);
	}

	#[test]
	fn a_consecutive_walk_is_refused_at_its_third_identifier() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2000]));
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2001]));
		assert_eq!(guard.check(0, ENGINE, &rdbi(&[0x2002])), Verdict::Refuse(Refusal::Walk));
	}

	#[test]
	fn a_downward_walk_is_a_walk_too() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2002]));
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2001]));
		assert_eq!(guard.check(0, ENGINE, &rdbi(&[0x2000])), Verdict::Refuse(Refusal::Walk));
	}

	#[test]
	fn a_fixed_stride_walk_is_refused() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2000]));
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2010]));
		assert_eq!(guard.check(0, ENGINE, &rdbi(&[0x2020])), Verdict::Refuse(Refusal::Walk));
	}

	#[test]
	fn a_walk_inside_one_multi_identifier_request_is_refused() {
		let mut guard = Guard::new();
		assert_eq!(guard.check(0, ENGINE, &rdbi(&[0xF100, 0xF101, 0xF102])), Verdict::Refuse(Refusal::Walk));
		let mut guard = Guard::new();
		assert_eq!(
			guard.check(0, ENGINE, &rdbi(&[0xF1A0, 0xF100, 0xF102, 0xF104])),
			Verdict::Refuse(Refusal::Walk)
		);
	}

	#[test]
	fn a_walk_that_spans_two_requests_is_refused() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0xF190, 0xF100, 0xF102]));
		assert_eq!(guard.check(0, ENGINE, &rdbi(&[0xF104])), Verdict::Refuse(Refusal::Walk));
	}

	#[test]
	fn the_same_identifier_again_and_again_is_not_a_walk() {
		let mut guard = Guard::new();
		let mut now = 0;
		for _ in 0..50 {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[0x202A]), None).unwrap();
		}
	}

	#[test]
	fn walks_are_counted_per_unit() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2000]));
		forward_now(&mut guard, 0, GEARBOX, &rdbi(&[0x2001]));
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2002]));
		forward_now(&mut guard, 0, GEARBOX, &rdbi(&[0x2003]));
	}

	#[test]
	fn a_walk_locks_reads_of_that_unit_for_the_rest_of_the_connection() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2000]));
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2001]));
		assert_eq!(guard.check(0, ENGINE, &rdbi(&[0x2002])), Verdict::Refuse(Refusal::Walk));
		// Any read, however harmless, and however much later.
		assert_eq!(guard.check(0, ENGINE, &rdbi(&[0xF190])), Verdict::Refuse(Refusal::Locked));
		assert_eq!(guard.check(3_600_000, ENGINE, &rdbi(&[0x2000])), Verdict::Refuse(Refusal::Locked));
		// Other services to that unit, and reads of other units, are unaffected.
		assert_eq!(guard.check(3_600_000, ENGINE, &[0x19, 0x02, 0xFF]), Verdict::Forward);
		assert_eq!(guard.check(3_600_000, GEARBOX, &rdbi(&[0xF190])), Verdict::Forward);
	}

	#[test]
	fn a_lock_ends_with_the_connection() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2000]));
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2001]));
		assert_eq!(guard.check(0, ENGINE, &rdbi(&[0x2002])), Verdict::Refuse(Refusal::Walk));
		drop(guard);
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2002]));
	}

	#[test]
	fn units_identify_walking_f100_to_f1ff_is_refused_at_its_third_identifier() {
		// `vagcan units --identify <unit>` reads the whole F100–F1FF range.
		let mut guard = Guard::new();
		let mut now = 0;
		let mut refused_at = None;
		for (at, did) in (0xF100u16..=0xF1FF).enumerate() {
			if let Err(refusal) = drive(&mut guard, &mut now, 0x714, &rdbi(&[did]), None) {
				refused_at = Some((at, refusal));
				break;
			}
		}
		assert_eq!(refused_at, Some((2, Refusal::Walk)));
		// Batched the way a presence test batches, it fails at once too.
		let mut guard = Guard::new();
		assert_eq!(
			guard.check(0, 0x714, &rdbi(&[0xF100, 0xF101, 0xF102, 0xF103])),
			Verdict::Refuse(Refusal::Walk)
		);
	}

	#[test]
	fn the_thirty_third_distinct_identifier_of_a_unit_is_refused_but_repeats_are_not() {
		let mut guard = Guard::new();
		let mut now = 0;
		// Squares never step by a fixed stride.
		let dids: Vec<u16> = (1..=33u16).map(|n| 0x1000 + n * n).collect();
		for did in &dids[..32] {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[*did]), None).unwrap();
		}
		assert_eq!(guard.check(now, ENGINE, &rdbi(&[dids[32]])), Verdict::Refuse(Refusal::TooManyDistinct));
		drive(&mut guard, &mut now, ENGINE, &rdbi(&[dids[3]]), None).expect("a repeat is not a new identifier");
		drive(&mut guard, &mut now, GEARBOX, &rdbi(&[dids[32]]), None).expect("the cap is per unit");
	}

	#[test]
	fn a_request_that_would_cross_the_distinct_cap_is_refused_whole() {
		let mut guard = Guard::new();
		let mut now = 0;
		let dids: Vec<u16> = (1..=33u16).map(|n| 0x1000 + n * n).collect();
		for did in &dids[..31] {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[*did]), None).unwrap();
		}
		assert_eq!(
			guard.check(now, ENGINE, &rdbi(&[dids[31], dids[32]])),
			Verdict::Refuse(Refusal::TooManyDistinct)
		);
		assert_eq!(
			guard.check(now + 2 * RATE_WINDOW_MS, ENGINE, &rdbi(&[dids[31], dids[31]])),
			Verdict::Forward,
			"one new identifier asked twice is one"
		);
	}

	#[test]
	fn a_new_guard_starts_from_nothing() {
		let mut guard = Guard::new();
		let mut now = 0;
		let dids: Vec<u16> = (1..=32u16).map(|n| 0x1000 + n * n).collect();
		for did in &dids {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[*did]), None).unwrap();
		}
		now += 2 * RATE_WINDOW_MS;
		forward_now(&mut guard, now, GEARBOX, &rdbi(&[0x2000]));
		forward_now(&mut guard, now, GEARBOX, &rdbi(&[0x2001]));
		assert_eq!(guard.check(now, GEARBOX, &rdbi(&[0x2002])), Verdict::Refuse(Refusal::Walk));
		for _ in 0..RATE_LIMIT - 2 {
			forward_now(&mut guard, now, 0x713, &[0x3E, 0x00]);
		}
		assert!(matches!(guard.check(now, 0x713, &[0x3E, 0x00]), Verdict::WaitUntil(_)));
		assert_eq!(guard.check(now, ENGINE, &rdbi(&[0x4000])), Verdict::Refuse(Refusal::TooManyDistinct));

		let mut guard = Guard::new();
		assert_eq!(guard.check(now, GEARBOX, &rdbi(&[0xF190])), Verdict::Forward, "lock gone");
		assert_eq!(guard.check(now, ENGINE, &rdbi(&[0x4000])), Verdict::Forward, "distinct count gone");
		for _ in 0..RATE_LIMIT {
			forward_now(&mut guard, now, 0x713, &[0x3E, 0x00]);
		}
	}

	#[test]
	fn every_refusal_has_a_short_reason() {
		for refusal in [
			Refusal::Empty,
			Refusal::ServiceNotAllowed(0x2E),
			Refusal::MalformedSession,
			Refusal::ProgrammingSession,
			Refusal::SpeedUnknown,
			Refusal::Moving(1),
			Refusal::MalformedIdentifiers,
			Refusal::TooManyIdentifiers,
			Refusal::Walk,
			Refusal::Locked,
			Refusal::TooManyDistinct,
		] {
			let reason = refusal.reason();
			assert!(!reason.is_empty() && reason.len() <= 80, "{refusal:?}: {reason:?}");
		}
	}

	// --- what the tool really sends -----------------------------------------------

	/// `identity::read_identity`'s identifiers, in the order it asks them.
	fn identity_reads() -> Vec<u16> {
		use crate::identity::did;
		vec![
			did::VIN,
			did::PART_NUMBER,
			did::HW_NUMBER,
			did::SW_VERSION,
			did::COMPONENT,
			did::SERIAL,
			did::CODING,
		]
	}

	#[test]
	fn vagcan_info_passes_every_limit() {
		// `info`: `read_identity` on the engine, then on the gearbox, one
		// identifier per request.
		let mut guard = Guard::new();
		let mut now = 0;
		for unit in [ENGINE, GEARBOX] {
			for did in identity_reads() {
				drive(&mut guard, &mut now, unit, &rdbi(&[did]), None).unwrap_or_else(|r| panic!("{unit:03X} {did:04X}: {r:?}"));
			}
		}
	}

	/// What `vagcan faults` sends to one unit (`vag-cli-diag/src/faults.rs::run`):
	/// its component name, every code by status mask, its own "now"; and where
	/// it has codes to show, the two identifiers naming its description file and
	/// the extended data of each code.
	fn fault_reads(codes: &[[u8; 3]]) -> Vec<Vec<u8>> {
		let mut out = vec![rdbi(&[0xF197]), vec![0x19, 0x02, 0xFF], rdbi(&[crate::dtc::UnitStamp::DID])];
		if !codes.is_empty() {
			out.push(rdbi(&[0xF19E]));
			out.push(rdbi(&[0xF1A2]));
			for code in codes {
				out.push(vec![0x19, 0x06, code[0], code[1], code[2], 0xFF]);
			}
		}
		out
	}

	/// The request ids `faults` visits on the reference car: the engine, the
	/// gearbox and the gateway, then the gateway's installation list (see
	/// `gateway.rs`'s test).
	fn fault_order() -> Vec<u16> {
		let mut ids = vec![ENGINE, GEARBOX, GATEWAY];
		ids.extend([
			0x700, 0x70A, 0x70C, 0x70E, 0x712, 0x713, 0x714, 0x715, 0x746, 0x74A, 0x74B, 0x767, 0x773, 0x776, 0x777,
		]);
		ids
	}

	#[test]
	fn vagcan_faults_passes_every_limit() {
		let mut guard = Guard::new();
		let mut now = 0;
		drive(&mut guard, &mut now, GATEWAY, &rdbi(&[crate::gateway::INSTALLATION_LIST]), None).unwrap();
		for (at, unit) in fault_order().into_iter().enumerate() {
			// Every third unit has stored codes, one of them two.
			let codes: &[[u8; 3]] = match at % 3 {
				0 => &[[0x00, 0x01, 0x29], [0x04, 0x71, 0x20]],
				1 => &[],
				_ => &[[0x00, 0x02, 0x97]],
			};
			for pdu in fault_reads(codes) {
				drive(&mut guard, &mut now, unit, &pdu, None).unwrap_or_else(|r| panic!("{unit:03X} {pdu:02X?}: {r:?}"));
			}
		}
	}

	#[test]
	fn vagcan_faults_extended_passes_on_a_stationary_car_and_is_refused_on_a_moving_one() {
		let mut guard = Guard::new();
		let mut now = 0;
		for unit in fault_order() {
			drive(&mut guard, &mut now, unit, &[0x10, 0x03], Some(0)).unwrap();
			for pdu in fault_reads(&[[0x00, 0x01, 0x29]]) {
				drive(&mut guard, &mut now, unit, &pdu, Some(0)).unwrap();
			}
		}
		let mut guard = Guard::new();
		assert_eq!(drive(&mut guard, &mut now, 0x713, &[0x10, 0x03], Some(30)), Err(Refusal::Moving(30)));
		assert_eq!(drive(&mut guard, &mut now, 0x713, &[0x10, 0x03], None), Err(Refusal::SpeedUnknown));
	}

	#[test]
	fn a_watch_page_of_four_channels_polled_repeatedly_passes_slowed_not_refused() {
		// The reference car's dash page: engine coolant F405, boost 202A, oil
		// 202F on the engine; control module temperature 028D on the gearbox.
		// `plan::plan` groups them by unit, one request each.
		let page = [(ENGINE, rdbi(&[0xF405, 0x202A, 0x202F])), (GEARBOX, rdbi(&[0x028D]))];
		let mut guard = Guard::new();
		let mut now = 0;
		for _ in 0..30 {
			for (unit, pdu) in &page {
				drive(&mut guard, &mut now, *unit, pdu, None).unwrap();
			}
		}
		// Four units a cycle, 120 in all, twenty per ten seconds: the cap has to
		// stretch the poll over at least fifty seconds, and needs no more than
		// one extra window to do it.
		assert!(now >= 50_000, "the cap slowed the poll: {now} ms");
		assert!(now < 60_000 + RATE_WINDOW_MS, "and did not stall it: {now} ms");
	}
}
