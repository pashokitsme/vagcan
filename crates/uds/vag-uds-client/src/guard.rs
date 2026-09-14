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
//!   speed counts as moving. Bit 7 of the session byte only suppresses the
//!   answer, so it is masked off first.
//! - **Rate:** at most [`RATE_LIMIT`] counted units in any [`RATE_WINDOW_MS`].
//!   An identifier in a `0x22` request is one unit each, any other request one,
//!   the speed read one. Over the cap the request **waits**
//!   ([`Verdict::WaitUntil`]): slowing is allowed, dropping is forbidden.
//! - **Sweeps:** at most [`MAX_IDENTIFIERS_PER_REQUEST`] identifiers in one
//!   `0x22`; per unit, the set of different identifiers asked may never hold
//!   [`WALK_RUN`] evenly spaced ones (a walk — refused, and `0x22` to that unit
//!   locked for the rest of the connection); at most [`MAX_DISTINCT_IDENTIFIERS`]
//!   different identifiers per unit and [`MAX_UNITS`] units per connection.
//! - **Units:** a unit is a request id and the one response id it answers on. The first
//!   request or subscription that reaches a request id binds its response id for the
//!   connection; the same request id on another response id is refused
//!   ([`Refusal::OtherResponseId`]). Without it a host could name one request id under
//!   every response id and make the board hold state for each.
//! - **Subscriptions:** the board polls one identifier for the host on its own
//!   clock ([`Guard::check_subscribe`]). The identifier counts once toward the
//!   sweep rules when subscribed; neither the subscribe nor the board's polls
//!   count toward the rate cap. At most [`MAX_SUBSCRIPTIONS`] live, none faster
//!   than [`MIN_PERIOD_MS`].
//!
//! One `Guard` per connection; dropping it is the reset. No clock inside — the
//! caller passes milliseconds from any monotonic source.
//!
//! # Two profiles
//!
//! [`Guard::new`] is the radio's, everything above. [`Guard::cable`] is for the
//! board's USB cable, which is trusted more (`CLAUDE.md`: the board guards itself on
//! links that are *not* a cable), the way a CANable on the same laptop is:
//!
//! | rule                                               | radio | cable |
//! |----------------------------------------------------|-------|-------|
//! | service allowlist                                  | yes   | yes   |
//! | `10 02` refused                                    | yes   | yes   |
//! | road speed 0 before another session change         | yes   | yes   |
//! | one response id per request id, [`MAX_UNITS`]      | yes   | yes   |
//! | [`MAX_SUBSCRIPTIONS`], [`MIN_PERIOD_MS`]           | yes   | yes   |
//! | rate cap                                           | yes   | no    |
//! | [`MAX_IDENTIFIERS_PER_REQUEST`]                    | yes   | no    |
//! | walk rule, [`MAX_DISTINCT_IDENTIFIERS`]            | yes   | no    |
//!
//! The per-request identifier cap goes with the rate cap it exists for ("identifiers
//! count, not requests"); a cable host is bounded by ISO-TP's PDU size instead. And
//! with no walk rule and no distinct cap the cable profile keeps no identifiers at
//! all, so its memory is the units map and the subscription slots, as bounded as the
//! radio's.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::vec::Vec;
use core::fmt;

use crate::pdu::READ_ONLY_ALLOWLIST;

/// Counted units allowed in any [`RATE_WINDOW_MS`]. A starting figure, not measured.
pub const RATE_LIMIT: u32 = 20;
/// The sliding window the rate cap is taken over.
pub const RATE_WINDOW_MS: u64 = 10_000;
/// Identifiers allowed in one `0x22` request.
pub const MAX_IDENTIFIERS_PER_REQUEST: usize = 4;
/// Different identifiers one unit may be asked for in one connection. A starting figure.
pub const MAX_DISTINCT_IDENTIFIERS: usize = 32;
/// Evenly spaced identifiers in one unit's asked set that make a walk.
pub const WALK_RUN: usize = 8;
/// Different units one connection may address.
///
/// Bounds the board's memory: per unit, an asked list of at most 32 `u16` (64
/// bytes on the heap — `Vec` grows 4, 8, 16, 32) and a 16-byte entry in a
/// `BTreeMap` whose nodes hold 11 entries in about 210 bytes (leaf) or 260
/// (internal) on the 32-bit board. 64 units in half-full nodes is at most ~13
/// nodes, ~3 KB, plus 64 × 64 = ~4 KB of identifiers: **under 8 KB** worst case.
pub const MAX_UNITS: usize = 64;
/// Live subscriptions one connection may hold.
pub const MAX_SUBSCRIPTIONS: usize = 32;
/// The shortest subscription period: `measure` reads road speed at 50 Hz.
pub const MIN_PERIOD_MS: u16 = 20;

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
	/// Send it (or start polling it), then call [`Guard::forwarded`] (or [`Guard::subscribed`]).
	Forward,
	/// Over the rate cap: ask again at this time (ms).
	WaitUntil(u64),
	/// Read road speed now — [`SPEED_REQUEST`] to [`SPEED_REQUEST_ID`] — and
	/// hand the result to [`Guard::speed`].
	CheckSpeedFirst,
	/// Do not send it; answer the host with the refusal's `Display` text.
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
	/// The connection would address more than [`MAX_UNITS`] units.
	TooManyUnits,
	/// A request id this connection already addressed on another response id. A unit
	/// answers on one; a host that names others is sweeping what the board remembers.
	OtherResponseId { request: u16, answers_on: u16 },
	/// A subscription period under [`MIN_PERIOD_MS`].
	PeriodTooShort,
	/// [`MAX_SUBSCRIPTIONS`] are already live.
	TooManySubscriptions,
}

impl Refusal {
	/// Short, plain English, without figures. The board sends the `Display`
	/// text, which adds them.
	pub fn reason(&self) -> &'static str {
		match self {
			Refusal::Empty => "empty request",
			Refusal::ServiceNotAllowed(_) => "service not allowed: this link only reads",
			Refusal::MalformedSession => "malformed session request",
			Refusal::ProgrammingSession => "programming session is never allowed over this link",
			Refusal::SpeedUnknown => "session change refused: the engine did not report road speed",
			Refusal::Moving(_) => "session change refused: the car is moving",
			Refusal::MalformedIdentifiers => "malformed read request",
			Refusal::TooManyIdentifiers => "too many identifiers in one read",
			Refusal::Walk => "evenly spaced identifiers: reads of this unit locked until reconnect",
			Refusal::Locked => "reads of this unit are locked until reconnect",
			Refusal::TooManyDistinct => "too many different identifiers from this unit",
			Refusal::TooManyUnits => "too many units in one connection",
			Refusal::OtherResponseId { .. } => "this request id already answers on another response id",
			Refusal::PeriodTooShort => "subscription period too short",
			Refusal::TooManySubscriptions => "too many subscriptions",
		}
	}
}

impl fmt::Display for Refusal {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Refusal::ServiceNotAllowed(sid) => write!(f, "service 0x{sid:02X} not allowed: this link only reads"),
			Refusal::MalformedSession => f.write_str("malformed session request: it is 10 and one byte"),
			Refusal::Moving(kmh) => write!(f, "session change refused: the car is moving at {kmh} km/h"),
			Refusal::MalformedIdentifiers => f.write_str("malformed read request: 22 and whole two-byte identifiers"),
			Refusal::TooManyIdentifiers => write!(f, "more than {MAX_IDENTIFIERS_PER_REQUEST} identifiers in one read"),
			Refusal::Walk => write!(f, "{WALK_RUN} evenly spaced identifiers: reads of this unit locked until reconnect"),
			Refusal::TooManyDistinct => write!(f, "more than {MAX_DISTINCT_IDENTIFIERS} different identifiers from this unit"),
			Refusal::TooManyUnits => write!(f, "more than {MAX_UNITS} units in one connection"),
			Refusal::OtherResponseId { request, answers_on } => {
				write!(f, "request id {request:03X} already answers on {answers_on:03X} in this connection")
			}
			Refusal::PeriodTooShort => write!(f, "subscription period under {MIN_PERIOD_MS} ms"),
			Refusal::TooManySubscriptions => write!(f, "more than {MAX_SUBSCRIPTIONS} subscriptions"),
			other => f.write_str(other.reason()),
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

/// Which rules a [`Guard`] holds a link to (module docs, "Two profiles").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Profile {
	/// A host across the radio: every rule.
	#[default]
	Radio,
	/// A host on the board's USB cable: no rate cap and no sweep rules.
	Cable,
}

/// One connection's view of what has been asked.
#[derive(Debug, Default)]
pub struct Guard {
	profile: Profile,
	/// `(when, units)` of everything that counted toward the rate cap, oldest first.
	/// Always empty under [`Profile::Cable`].
	window: VecDeque<(u64, u32)>,
	/// Every unit addressed, at most [`MAX_UNITS`].
	units: BTreeMap<u16, UnitHistory>,
	/// Live subscription ids, at most [`MAX_SUBSCRIPTIONS`].
	subscriptions: Vec<u16>,
}

#[derive(Debug, Default)]
struct UnitHistory {
	/// Every different identifier forwarded or subscribed; at most
	/// [`MAX_DISTINCT_IDENTIFIERS`], so a list is smaller than a set. Always empty
	/// under [`Profile::Cable`], which has no rule that reads it.
	asked: Vec<u16>,
	locked: bool,
	/// The response id this request id answers on, from the first request or
	/// subscription that reached it; any other is refused for the connection.
	response: Option<u16>,
}

impl Guard {
	/// The radio's guard: every rule.
	pub fn new() -> Self {
		Self::default()
	}

	/// The USB cable's guard (module docs, "Two profiles").
	pub fn cable() -> Self {
		Guard {
			profile: Profile::Cable,
			..Self::default()
		}
	}

	pub fn profile(&self) -> Profile {
		self.profile
	}

	/// A fresh guard of the same profile: what a new connection starts from.
	pub fn renewed(&self) -> Self {
		Guard {
			profile: self.profile,
			..Self::default()
		}
	}

	fn radio(&self) -> bool {
		self.profile == Profile::Radio
	}

	/// Decide what to do with one request from the host.
	pub fn check(&mut self, now_ms: u64, request_id: u16, response_id: u16, pdu: &[u8]) -> Verdict {
		let admitted = match self.admit(request_id, response_id, pdu) {
			Ok(admitted) => admitted,
			Err(refusal) => return Verdict::Refuse(refusal),
		};
		if !self.radio() {
			return if admitted.needs_speed {
				Verdict::CheckSpeedFirst
			} else {
				Verdict::Forward
			};
		}
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
	pub fn speed(&mut self, now_ms: u64, request_id: u16, response_id: u16, pdu: &[u8], kmh: Option<u8>) -> Verdict {
		if self.radio() {
			self.window.push_back((now_ms, 1));
		}
		let admitted = match self.admit(request_id, response_id, pdu) {
			Ok(admitted) => admitted,
			Err(refusal) => return Verdict::Refuse(refusal),
		};
		if !admitted.needs_speed {
			return self.check(now_ms, request_id, response_id, pdu);
		}
		match kmh {
			None => Verdict::Refuse(Refusal::SpeedUnknown),
			Some(0) if !self.radio() => Verdict::Forward,
			Some(0) => match self.wait(now_ms, admitted.cost) {
				Some(until) => Verdict::WaitUntil(until),
				None => Verdict::Forward,
			},
			Some(moving) => Verdict::Refuse(Refusal::Moving(moving)),
		}
	}

	/// Record a request that was actually sent. Refused and delayed requests are
	/// never passed here, so they do not count.
	pub fn forwarded(&mut self, now_ms: u64, request_id: u16, response_id: u16, pdu: &[u8]) {
		if !self.radio() {
			self.record(request_id, response_id, &[]);
			return;
		}
		let dids = identifiers(pdu);
		let cost = if pdu.first() == Some(&READ_BY_IDENTIFIER) {
			dids.len() as u32
		} else {
			1
		};
		self.window.push_back((now_ms, cost));
		self.record(request_id, response_id, &dids);
	}

	/// Decide whether the board may poll `did` on `request_id` every `period_ms`.
	///
	/// [`Verdict::Forward`] or [`Verdict::Refuse`], never a wait: a subscribe does
	/// not count toward the rate cap, so a watch page starts at once. Its
	/// identifier counts toward the walk rule and the distinct and unit caps.
	pub fn check_subscribe(&mut self, request_id: u16, response_id: u16, did: u16, period_ms: u16) -> Verdict {
		let checked = if period_ms < MIN_PERIOD_MS {
			Err(Refusal::PeriodTooShort)
		} else if self.subscriptions.len() >= MAX_SUBSCRIPTIONS {
			Err(Refusal::TooManySubscriptions)
		} else {
			self
				.admit_unit(request_id, response_id)
				.and_then(|()| self.admit_reads(request_id, &[did]))
		};
		match checked {
			Ok(()) => Verdict::Forward,
			Err(refusal) => Verdict::Refuse(refusal),
		}
	}

	/// Record a subscription the board started. A live `sub` given again is
	/// replaced, not counted twice.
	pub fn subscribed(&mut self, sub: u16, request_id: u16, response_id: u16, did: u16) {
		if !self.subscriptions.contains(&sub) {
			self.subscriptions.push(sub);
		}
		self.record(request_id, response_id, &[did]);
	}

	/// Free a subscription's slot. Its identifier stays asked for the connection.
	pub fn unsubscribed(&mut self, sub: u16) {
		self.subscriptions.retain(|&live| live != sub);
	}

	/// Note the unit and the identifiers that reached it.
	fn record(&mut self, request_id: u16, response_id: u16, dids: &[u16]) {
		let radio = self.radio();
		let unit = self.units.entry(request_id).or_default();
		unit.response.get_or_insert(response_id);
		if !radio {
			return;
		}
		for did in dids {
			if !unit.asked.contains(did) {
				unit.asked.push(*did);
			}
		}
	}

	/// The rules that do not depend on time. Refusing a walk locks the unit.
	fn admit(&mut self, request_id: u16, response_id: u16, pdu: &[u8]) -> Result<Admitted, Refusal> {
		let (&sid, rest) = pdu.split_first().ok_or(Refusal::Empty)?;
		if !READ_ONLY_ALLOWLIST.contains(&sid) {
			return Err(Refusal::ServiceNotAllowed(sid));
		}
		self.admit_unit(request_id, response_id)?;
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
				self.admit_reads(request_id, &dids)?;
				Ok(Admitted {
					cost: dids.len() as u32,
					needs_speed: false,
				})
			}
			_ => Ok(Admitted { cost: 1, needs_speed: false }),
		}
	}

	/// A unit is a request id and the one response id it answers on. One already
	/// addressed passes on that pair and is refused on any other; a new one needs a free
	/// slot. So [`MAX_UNITS`] bounds pairs, not only request ids.
	fn admit_unit(&self, request_id: u16, response_id: u16) -> Result<(), Refusal> {
		match self.units.get(&request_id) {
			Some(UnitHistory {
				response: Some(answers_on), ..
			}) if *answers_on != response_id => Err(Refusal::OtherResponseId {
				request: request_id,
				answers_on: *answers_on,
			}),
			Some(_) => Ok(()),
			None if self.units.len() >= MAX_UNITS => Err(Refusal::TooManyUnits),
			None => Ok(()),
		}
	}

	/// The sweep rules for identifiers about to be asked of one unit. A unit
	/// gets an entry only once something reaches it or it is locked, so a
	/// refusal costs the board no memory.
	fn admit_reads(&mut self, request_id: u16, dids: &[u16]) -> Result<(), Refusal> {
		if !self.radio() {
			return Ok(());
		}
		let asked = match self.units.get(&request_id) {
			Some(unit) if unit.locked => return Err(Refusal::Locked),
			Some(unit) => unit.asked.as_slice(),
			None => &[],
		};
		if dids.len() > MAX_IDENTIFIERS_PER_REQUEST {
			return Err(Refusal::TooManyIdentifiers);
		}
		let mut after: Vec<u16> = asked.iter().chain(dids).copied().collect();
		after.sort_unstable();
		after.dedup();
		if walks(&after) {
			self.units.entry(request_id).or_default().locked = true;
			return Err(Refusal::Walk);
		}
		if after.len() > MAX_DISTINCT_IDENTIFIERS {
			return Err(Refusal::TooManyDistinct);
		}
		Ok(())
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

/// Whether `sorted` (ascending, no repeats) holds [`WALK_RUN`] identifiers
/// evenly spaced by one non-zero stride. Order of asking cannot matter: this
/// sees only the set.
///
/// Every pair is tried as a run's first two terms and the run extended by
/// binary search; a pair whose stride leaves no room for [`WALK_RUN`] terms
/// below the largest identifier ends its row, since strides only grow along
/// it. `n` is at most [`MAX_DISTINCT_IDENTIFIERS`] + [`MAX_IDENTIFIERS_PER_REQUEST`]
/// = 36, so this is O(n² · WALK_RUN · log n): at most 630 pairs × 6 lookups ×
/// 6 comparisons, about 23 000 comparisons per request.
fn walks(sorted: &[u16]) -> bool {
	let Some(&largest) = sorted.last() else {
		return false;
	};
	let span = WALK_RUN as u32 - 1;
	for (at, &first) in sorted.iter().enumerate() {
		for &second in &sorted[at + 1..] {
			let stride = u32::from(second - first);
			if stride * span > u32::from(largest - first) {
				break;
			}
			// Every term is at most `largest`, so the conversion back cannot wrap.
			let run = (2..=span)
				.take_while(|&k| sorted.binary_search(&((u32::from(first) + k * stride) as u16)).is_ok())
				.count()
				+ 2;
			if run >= WALK_RUN {
				return true;
			}
		}
	}
	false
}

#[cfg(test)]
mod tests {
	use super::*;
	use alloc::format;
	use alloc::string::ToString;
	use alloc::vec;

	const ENGINE: u16 = 0x7E0;
	const GEARBOX: u16 = 0x7E1;
	const GATEWAY: u16 = 0x710;

	/// The response id the tests pair a request id with: synthetic, one per request id.
	fn resp(request_id: u16) -> u16 {
		request_id + 8
	}

	fn rdbi(dids: &[u16]) -> Vec<u8> {
		let mut pdu = vec![0x22];
		for did in dids {
			pdu.extend_from_slice(&did.to_be_bytes());
		}
		pdu
	}

	/// Identifiers no eight of which are evenly spaced: squares hold no four
	/// in arithmetic progression.
	fn squares(count: u16) -> Vec<u16> {
		(1..=count).map(|n| 0x1000 + n * n).collect()
	}

	/// Take one request through the guard the way the board does: wait when
	/// told to, read speed when told to (the engine answering `kmh`), send when
	/// allowed. Each exchange with the car takes 30 ms. Returns the refusal, if any.
	fn drive(guard: &mut Guard, now: &mut u64, request_id: u16, pdu: &[u8], kmh: Option<u8>) -> Result<(), Refusal> {
		for _ in 0..8 {
			let verdict = match guard.check(*now, request_id, resp(request_id), pdu) {
				Verdict::CheckSpeedFirst => {
					*now += 30;
					guard.speed(*now, request_id, resp(request_id), pdu, kmh)
				}
				other => other,
			};
			match verdict {
				Verdict::Forward => {
					guard.forwarded(*now, request_id, resp(request_id), pdu);
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
		assert_eq!(guard.check(now, request_id, resp(request_id), pdu), Verdict::Forward, "{pdu:02X?}");
		guard.forwarded(now, request_id, resp(request_id), pdu);
	}

	fn subscribe(guard: &mut Guard, sub: u16, request_id: u16, did: u16, period_ms: u16) -> Result<(), Refusal> {
		match guard.check_subscribe(request_id, resp(request_id), did, period_ms) {
			Verdict::Forward => {
				guard.subscribed(sub, request_id, resp(request_id), did);
				Ok(())
			}
			Verdict::Refuse(r) => Err(r),
			other => panic!("a subscribe is forwarded or refused, not {other:?}"),
		}
	}

	// --- services -------------------------------------------------------------

	#[test]
	fn an_empty_request_is_refused() {
		assert_eq!(Guard::new().check(0, ENGINE, resp(ENGINE), &[]), Verdict::Refuse(Refusal::Empty));
	}

	#[test]
	fn services_outside_the_allowlist_are_refused() {
		let mut guard = Guard::new();
		for sid in [0x14, 0x2E, 0x27, 0x31, 0x34, 0x11, 0x28, 0x85] {
			assert_eq!(
				guard.check(0, ENGINE, resp(ENGINE), &[sid, 0x00]),
				Verdict::Refuse(Refusal::ServiceNotAllowed(sid))
			);
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
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &[0x10, 0x02]),
			Verdict::Refuse(Refusal::ProgrammingSession)
		);
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &[0x10, 0x82]),
			Verdict::Refuse(Refusal::ProgrammingSession),
			"the suppress-response bit does not disguise it"
		);
		assert_eq!(
			guard.speed(0, ENGINE, resp(ENGINE), &[0x10, 0x02], Some(0)),
			Verdict::Refuse(Refusal::ProgrammingSession),
			"not even on a stationary car"
		);
	}

	#[test]
	fn the_default_session_is_always_allowed_without_a_speed_read() {
		let mut guard = Guard::new();
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &[0x10, 0x01]), Verdict::Forward);
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &[0x10, 0x81]), Verdict::Forward);
	}

	#[test]
	fn a_session_request_that_is_not_two_bytes_is_refused() {
		let mut guard = Guard::new();
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &[0x10]), Verdict::Refuse(Refusal::MalformedSession));
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &[0x10, 0x03, 0x00]),
			Verdict::Refuse(Refusal::MalformedSession)
		);
	}

	#[test]
	fn another_session_change_asks_for_road_speed_first() {
		let mut guard = Guard::new();
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &[0x10, 0x03]), Verdict::CheckSpeedFirst);
		assert_eq!(guard.check(0, 0x713, resp(0x713), &[0x10, 0x03]), Verdict::CheckSpeedFirst);
		assert_eq!(guard.check(0, 0x713, resp(0x713), &[0x10, 0x40]), Verdict::CheckSpeedFirst);
	}

	#[test]
	fn a_session_change_on_a_stationary_car_is_forwarded() {
		assert_eq!(Guard::new().speed(0, 0x713, resp(0x713), &[0x10, 0x03], Some(0)), Verdict::Forward);
	}

	#[test]
	fn a_session_change_on_a_moving_car_is_refused_with_its_speed() {
		assert_eq!(
			Guard::new().speed(0, 0x713, resp(0x713), &[0x10, 0x03], Some(12)),
			Verdict::Refuse(Refusal::Moving(12))
		);
		assert_eq!(Refusal::Moving(12).to_string(), "session change refused: the car is moving at 12 km/h");
	}

	#[test]
	fn a_session_change_without_a_speed_answer_counts_as_moving() {
		let mut guard = Guard::new();
		assert_eq!(
			guard.speed(0, 0x713, resp(0x713), &[0x10, 0x03], None),
			Verdict::Refuse(Refusal::SpeedUnknown)
		);
		// A negative answer decodes to no speed, and is refused the same way.
		assert_eq!(
			guard.speed(0, 0x713, resp(0x713), &[0x10, 0x03], road_speed(&[0x7F, 0x22, 0x31])),
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
		// board read the engine without limit. Each check reserves room for the
		// read and the request, so nineteen reads fill the window as far as a
		// twentieth session change can see.
		let mut guard = Guard::new();
		for _ in 0..RATE_LIMIT - 1 {
			assert_eq!(guard.check(0, 0x713, resp(0x713), &[0x10, 0x03]), Verdict::CheckSpeedFirst);
			assert_eq!(
				guard.speed(0, 0x713, resp(0x713), &[0x10, 0x03], Some(40)),
				Verdict::Refuse(Refusal::Moving(40))
			);
		}
		assert_eq!(guard.check(0, 0x713, resp(0x713), &[0x10, 0x03]), Verdict::WaitUntil(RATE_WINDOW_MS));
	}

	#[test]
	fn a_session_change_waits_until_there_is_room_for_its_speed_read_too() {
		let mut guard = Guard::new();
		for _ in 0..RATE_LIMIT - 1 {
			forward_now(&mut guard, 0, ENGINE, &[0x3E, 0x00]);
		}
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &[0x10, 0x03]), Verdict::WaitUntil(RATE_WINDOW_MS));
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &[0x10, 0x01]),
			Verdict::Forward,
			"one unit still fits"
		);
	}

	// --- rate -----------------------------------------------------------------

	#[test]
	fn the_twenty_first_unit_in_ten_seconds_waits_for_the_oldest_to_expire() {
		let mut guard = Guard::new();
		for i in 0..RATE_LIMIT as u64 {
			forward_now(&mut guard, i * 100, ENGINE, &[0x3E, 0x00]);
		}
		assert_eq!(
			guard.check(2_000, ENGINE, resp(ENGINE), &[0x3E, 0x00]),
			Verdict::WaitUntil(RATE_WINDOW_MS)
		);
		assert_eq!(
			guard.check(9_999, ENGINE, resp(ENGINE), &[0x3E, 0x00]),
			Verdict::WaitUntil(RATE_WINDOW_MS)
		);
		assert_eq!(guard.check(10_000, ENGINE, resp(ENGINE), &[0x3E, 0x00]), Verdict::Forward);
	}

	#[test]
	fn refused_and_delayed_requests_do_not_count_toward_the_rate() {
		let mut guard = Guard::new();
		for _ in 0..100 {
			assert!(matches!(guard.check(0, ENGINE, resp(ENGINE), &[0x2E, 0xF1, 0x90]), Verdict::Refuse(_)));
			// Asked, never sent.
			let _ = guard.check(0, ENGINE, resp(ENGINE), &[0x3E, 0x00]);
		}
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &[0x3E, 0x00]), Verdict::Forward);
	}

	#[test]
	fn identifiers_inside_one_request_each_count_toward_the_rate() {
		let mut guard = Guard::new();
		// Four identifiers, five times: twenty units.
		let batch = rdbi(&[0xF190, 0xF187, 0xF197, 0xF19E]);
		for _ in 0..5 {
			forward_now(&mut guard, 0, ENGINE, &batch);
		}
		assert_eq!(guard.check(0, GEARBOX, resp(GEARBOX), &[0x3E, 0x00]), Verdict::WaitUntil(RATE_WINDOW_MS));
	}

	#[test]
	fn a_request_above_the_remaining_budget_waits_even_if_a_smaller_one_would_fit() {
		let mut guard = Guard::new();
		for i in 0..18u64 {
			forward_now(&mut guard, i, ENGINE, &[0x3E, 0x00]);
		}
		// 18 used: four identifiers need the two oldest gone, at 0 and 1 ms.
		assert_eq!(
			guard.check(18, GEARBOX, resp(GEARBOX), &rdbi(&[0xF190, 0xF187, 0xF197, 0xF19E])),
			Verdict::WaitUntil(10_001)
		);
		assert_eq!(guard.check(18, GEARBOX, resp(GEARBOX), &rdbi(&[0xF190, 0xF187])), Verdict::Forward);
	}

	// --- identifier requests ----------------------------------------------------

	#[test]
	fn a_read_that_is_not_whole_identifiers_is_refused() {
		let mut guard = Guard::new();
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &[0x22]),
			Verdict::Refuse(Refusal::MalformedIdentifiers)
		);
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &[0x22, 0xF1]),
			Verdict::Refuse(Refusal::MalformedIdentifiers)
		);
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &[0x22, 0xF1, 0x90, 0xF1]),
			Verdict::Refuse(Refusal::MalformedIdentifiers)
		);
	}

	#[test]
	fn more_than_four_identifiers_in_one_request_is_refused() {
		let mut guard = Guard::new();
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &rdbi(&[0xF190, 0xF187, 0xF197, 0xF19E, 0xF1A2])),
			Verdict::Refuse(Refusal::TooManyIdentifiers)
		);
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &rdbi(&[0xF190, 0xF187, 0xF197, 0xF19E])),
			Verdict::Forward
		);
	}

	// --- walks ------------------------------------------------------------------

	#[test]
	fn a_run_of_eight_adjacent_identifiers_is_refused_at_the_eighth_in_any_order() {
		let shuffled = [0xF105, 0xF100, 0xF107, 0xF102, 0xF101, 0xF106, 0xF103, 0xF104];
		let mut guard = Guard::new();
		let mut now = 0;
		for did in &shuffled[..WALK_RUN - 1] {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[*did]), None).unwrap();
		}
		assert_eq!(
			guard.check(now, ENGINE, resp(ENGINE), &rdbi(&[shuffled[7]])),
			Verdict::Refuse(Refusal::Walk)
		);
	}

	#[test]
	fn seven_evenly_spaced_identifiers_are_not_a_walk() {
		let mut guard = Guard::new();
		let mut now = 0;
		for did in 0xF100..0xF107 {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[did]), None).unwrap();
		}
	}

	#[test]
	fn padding_and_interleaving_do_not_dodge_the_walk_rule() {
		let mut guard = Guard::new();
		let mut now = 0;
		for did in 0xF100..0xF107 {
			// Repeat each, and put an unrelated identifier between.
			for pdu in [rdbi(&[did]), rdbi(&[0x2029]), rdbi(&[did, did]), rdbi(&[0xF190])] {
				drive(&mut guard, &mut now, ENGINE, &pdu, None).unwrap();
			}
		}
		assert_eq!(drive(&mut guard, &mut now, ENGINE, &rdbi(&[0x2029, 0xF107]), None), Err(Refusal::Walk));
	}

	#[test]
	fn a_stride_two_run_of_eight_is_refused() {
		let mut guard = Guard::new();
		let mut now = 0;
		for n in 0..7u16 {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[0x2000 + 2 * n]), None).unwrap();
		}
		assert_eq!(guard.check(now, ENGINE, resp(ENGINE), &rdbi(&[0x200E])), Verdict::Refuse(Refusal::Walk));
	}

	#[test]
	fn a_wide_stride_run_is_refused_too() {
		let mut guard = Guard::new();
		let mut now = 0;
		for n in (1..8u16).rev() {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[0x0100 + 0x1111 * n]), None).unwrap();
		}
		assert_eq!(guard.check(now, ENGINE, resp(ENGINE), &rdbi(&[0x0100])), Verdict::Refuse(Refusal::Walk));
	}

	#[test]
	fn a_run_completed_inside_one_multi_identifier_request_is_refused() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0xF100, 0xF101, 0xF102, 0xF103]));
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &rdbi(&[0xF107, 0xF105, 0xF104, 0xF106])),
			Verdict::Refuse(Refusal::Walk)
		);
	}

	#[test]
	fn a_few_adjacent_identifiers_polled_for_minutes_pass() {
		// A watch page with 2029, 202A and 202B side by side, and repeats.
		let mut guard = Guard::new();
		let mut now = 0;
		while now < 5 * 60_000 {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[0x2029, 0x202A, 0x202B]), None).unwrap();
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[0x202A]), None).unwrap();
		}
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
		let mut now = 0;
		for did in 0xF100..0xF107 {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[did]), None).unwrap();
			drive(&mut guard, &mut now, GEARBOX, &rdbi(&[did + 1]), None).unwrap();
		}
	}

	#[test]
	fn a_walk_locks_reads_of_that_unit_for_the_rest_of_the_connection() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2000, 0x2001, 0x2002, 0x2003]));
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2004, 0x2005, 0x2006]));
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &rdbi(&[0x2007])), Verdict::Refuse(Refusal::Walk));
		// Any read, however harmless, and however much later.
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &rdbi(&[0xF190])), Verdict::Refuse(Refusal::Locked));
		assert_eq!(
			guard.check(3_600_000, ENGINE, resp(ENGINE), &rdbi(&[0x2000])),
			Verdict::Refuse(Refusal::Locked)
		);
		// Other services to that unit, and reads of other units, are unaffected.
		assert_eq!(guard.check(3_600_000, ENGINE, resp(ENGINE), &[0x19, 0x02, 0xFF]), Verdict::Forward);
		assert_eq!(guard.check(3_600_000, GEARBOX, resp(GEARBOX), &rdbi(&[0xF190])), Verdict::Forward);
	}

	#[test]
	fn a_lock_ends_with_the_connection() {
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2000, 0x2001, 0x2002, 0x2003]));
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2004, 0x2005, 0x2006]));
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &rdbi(&[0x2007])), Verdict::Refuse(Refusal::Walk));
		drop(guard);
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, ENGINE, &rdbi(&[0x2007]));
	}

	#[test]
	fn units_identify_walking_f100_to_f1ff_is_refused_by_its_eighth_identifier() {
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
		assert_eq!(refused_at, Some((WALK_RUN - 1, Refusal::Walk)));
		// Batched four at a time, the second batch completes the run.
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, 0x714, &rdbi(&[0xF100, 0xF101, 0xF102, 0xF103]));
		assert_eq!(
			guard.check(0, 0x714, resp(0x714), &rdbi(&[0xF104, 0xF105, 0xF106, 0xF107])),
			Verdict::Refuse(Refusal::Walk)
		);
	}

	// --- distinct identifiers and units ---------------------------------------------

	#[test]
	fn the_thirty_third_distinct_identifier_of_a_unit_is_refused_but_repeats_are_not() {
		let mut guard = Guard::new();
		let mut now = 0;
		let dids = squares(33);
		for did in &dids[..32] {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[*did]), None).unwrap();
		}
		assert_eq!(
			guard.check(now, ENGINE, resp(ENGINE), &rdbi(&[dids[32]])),
			Verdict::Refuse(Refusal::TooManyDistinct)
		);
		drive(&mut guard, &mut now, ENGINE, &rdbi(&[dids[3]]), None).expect("a repeat is not a new identifier");
		drive(&mut guard, &mut now, GEARBOX, &rdbi(&[dids[32]]), None).expect("the cap is per unit");
	}

	#[test]
	fn a_request_that_would_cross_the_distinct_cap_is_refused_whole() {
		let mut guard = Guard::new();
		let mut now = 0;
		let dids = squares(33);
		for did in &dids[..31] {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[*did]), None).unwrap();
		}
		assert_eq!(
			guard.check(now, ENGINE, resp(ENGINE), &rdbi(&[dids[31], dids[32]])),
			Verdict::Refuse(Refusal::TooManyDistinct)
		);
		assert_eq!(
			guard.check(now + 2 * RATE_WINDOW_MS, ENGINE, resp(ENGINE), &rdbi(&[dids[31], dids[31]])),
			Verdict::Forward,
			"one new identifier asked twice is one"
		);
	}

	/// A unit answers on one id. A host that pairs a request id it already used with
	/// another response id is not addressing a unit, it is making the board remember
	/// one more thing — and a board that remembers one per response id runs out of heap.
	#[test]
	fn a_request_id_answers_on_one_response_id_per_connection() {
		let mut guard = Guard::new();
		let mut now = 0;
		drive(&mut guard, &mut now, ENGINE, &rdbi(&[0xF190]), None).unwrap();
		let other = resp(ENGINE) + 1;
		let refused = Verdict::Refuse(Refusal::OtherResponseId {
			request: ENGINE,
			answers_on: resp(ENGINE),
		});
		assert_eq!(guard.check(now, ENGINE, other, &rdbi(&[0xF190])), refused);
		assert_eq!(guard.check_subscribe(ENGINE, other, 0xF40D, 100), refused);
		assert_eq!(
			guard.check(now, ENGINE, resp(ENGINE), &rdbi(&[0xF190])),
			Verdict::Forward,
			"the pair in use still passes"
		);

		// A subscription binds the pair as well.
		assert_eq!(guard.check_subscribe(GEARBOX, resp(GEARBOX), 0xF40D, 100), Verdict::Forward);
		guard.subscribed(1, GEARBOX, resp(GEARBOX), 0xF40D);
		assert!(matches!(
			guard.check(now, GEARBOX, 0x123, &[0x3E, 0x00]),
			Verdict::Refuse(Refusal::OtherResponseId { .. })
		));
	}

	/// The attack the rule exists for: one request id, every response id, as fast as the
	/// link carries it. One subscription passes; the rest are refused, and the guard's
	/// own memory does not grow with them.
	#[test]
	fn sweeping_response_ids_under_one_request_id_is_refused_and_costs_no_memory() {
		let mut guard = Guard::new();
		let mut forwarded = 0;
		for response_id in 0..=0x7FF {
			if guard.check_subscribe(ENGINE, response_id, 0xF40D, 100) == Verdict::Forward {
				guard.subscribed(1, ENGINE, response_id, 0xF40D);
				forwarded += 1;
			}
		}
		assert_eq!(forwarded, 1);
		assert_eq!(guard.units.len(), 1);
	}

	#[test]
	fn the_sixty_fifth_unit_of_a_connection_is_refused_whatever_the_service() {
		let mut guard = Guard::new();
		let mut now = 0;
		for unit in 0x700..0x700 + MAX_UNITS as u16 {
			drive(&mut guard, &mut now, unit, &[0x3E, 0x00], None).unwrap();
		}
		let next = 0x700 + MAX_UNITS as u16;
		now += RATE_WINDOW_MS;
		assert_eq!(guard.check(now, next, resp(next), &[0x3E, 0x00]), Verdict::Refuse(Refusal::TooManyUnits));
		assert_eq!(
			guard.check(now, next, resp(next), &rdbi(&[0xF190])),
			Verdict::Refuse(Refusal::TooManyUnits)
		);
		assert_eq!(guard.check(now, next, resp(next), &[0x10, 0x03]), Verdict::Refuse(Refusal::TooManyUnits));
		assert_eq!(
			guard.check_subscribe(next, resp(next), 0xF40D, 20),
			Verdict::Refuse(Refusal::TooManyUnits)
		);
		assert_eq!(
			guard.check(now, 0x700, resp(0x700), &rdbi(&[0xF190])),
			Verdict::Forward,
			"a unit already addressed still is"
		);
		assert!(Refusal::TooManyUnits.to_string().contains("64"));
	}

	#[test]
	fn a_refused_request_does_not_take_a_unit_slot() {
		let mut guard = Guard::new();
		for unit in 0..1000u16 {
			let _ = guard.check(0, unit, resp(unit), &[0x2E, 0xF1, 0x90]);
			let _ = guard.check(0, unit, resp(unit), &rdbi(&[1, 2, 3, 4, 5]));
		}
		assert_eq!(guard.check(0, 0x7FF, resp(0x7FF), &[0x3E, 0x00]), Verdict::Forward);
	}

	#[test]
	fn a_new_guard_starts_from_nothing() {
		let mut guard = Guard::new();
		let mut now = 0;
		for did in squares(32) {
			drive(&mut guard, &mut now, ENGINE, &rdbi(&[did]), None).unwrap();
		}
		now += 2 * RATE_WINDOW_MS;
		forward_now(&mut guard, now, GEARBOX, &rdbi(&[0x2000, 0x2001, 0x2002, 0x2003]));
		forward_now(&mut guard, now, GEARBOX, &rdbi(&[0x2004, 0x2005, 0x2006]));
		assert_eq!(guard.check(now, GEARBOX, resp(GEARBOX), &rdbi(&[0x2007])), Verdict::Refuse(Refusal::Walk));
		for _ in 0..RATE_LIMIT - 7 {
			forward_now(&mut guard, now, 0x713, &[0x3E, 0x00]);
		}
		assert!(matches!(guard.check(now, 0x713, resp(0x713), &[0x3E, 0x00]), Verdict::WaitUntil(_)));
		assert_eq!(
			guard.check(now, ENGINE, resp(ENGINE), &rdbi(&[0x4000])),
			Verdict::Refuse(Refusal::TooManyDistinct)
		);
		for sub in 0..MAX_SUBSCRIPTIONS as u16 {
			subscribe(&mut guard, sub, 0x714, 0x2029, 100).unwrap();
		}

		let mut guard = Guard::new();
		assert_eq!(guard.check(now, GEARBOX, resp(GEARBOX), &rdbi(&[0xF190])), Verdict::Forward, "lock gone");
		assert_eq!(
			guard.check(now, ENGINE, resp(ENGINE), &rdbi(&[0x4000])),
			Verdict::Forward,
			"distinct count gone"
		);
		for _ in 0..RATE_LIMIT {
			forward_now(&mut guard, now, 0x713, &[0x3E, 0x00]);
		}
		subscribe(&mut guard, 99, 0x714, 0x2029, 100).expect("slots gone");
	}

	#[test]
	fn reasons_carry_no_figures_and_the_display_text_does() {
		let all = [
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
			Refusal::TooManyUnits,
			Refusal::PeriodTooShort,
			Refusal::TooManySubscriptions,
		];
		for refusal in all {
			let reason = refusal.reason();
			assert!(!reason.is_empty() && reason.len() <= 80, "{refusal:?}: {reason:?}");
			assert!(!reason.chars().any(|c| c.is_ascii_digit()), "{refusal:?}: {reason:?}");
			let text = refusal.to_string();
			assert!(!text.is_empty() && text.len() <= 100, "{refusal:?}: {text:?}");
		}
		for (refusal, figure) in [
			(Refusal::TooManyIdentifiers, MAX_IDENTIFIERS_PER_REQUEST.to_string()),
			(Refusal::Walk, WALK_RUN.to_string()),
			(Refusal::TooManyDistinct, MAX_DISTINCT_IDENTIFIERS.to_string()),
			(Refusal::TooManyUnits, MAX_UNITS.to_string()),
			(Refusal::PeriodTooShort, MIN_PERIOD_MS.to_string()),
			(Refusal::TooManySubscriptions, MAX_SUBSCRIPTIONS.to_string()),
			(Refusal::ServiceNotAllowed(0x2E), "0x2E".to_string()),
			(
				Refusal::OtherResponseId {
					request: 0x7E0,
					answers_on: 0x7E8,
				},
				"7E8".to_string(),
			),
		] {
			assert!(refusal.to_string().contains(&figure), "{refusal:?}: {refusal} lacks {figure}");
		}
		assert_eq!(format!("{}", Refusal::Locked), Refusal::Locked.reason());
	}

	// --- subscriptions ------------------------------------------------------------

	#[test]
	fn a_subscription_is_accepted_at_once_and_never_waits() {
		let mut guard = Guard::new();
		for _ in 0..RATE_LIMIT {
			forward_now(&mut guard, 0, ENGINE, &[0x3E, 0x00]);
		}
		assert_eq!(
			guard.check_subscribe(GEARBOX, resp(GEARBOX), 0xF40D, 20),
			Verdict::Forward,
			"a full rate window does not delay it"
		);
	}

	#[test]
	fn a_period_under_twenty_milliseconds_is_refused() {
		let mut guard = Guard::new();
		assert_eq!(
			guard.check_subscribe(GEARBOX, resp(GEARBOX), 0xF40D, 19),
			Verdict::Refuse(Refusal::PeriodTooShort)
		);
		assert_eq!(
			guard.check_subscribe(GEARBOX, resp(GEARBOX), 0xF40D, 0),
			Verdict::Refuse(Refusal::PeriodTooShort)
		);
		assert_eq!(guard.check_subscribe(GEARBOX, resp(GEARBOX), 0xF40D, MIN_PERIOD_MS), Verdict::Forward);
	}

	#[test]
	fn the_thirty_third_live_subscription_is_refused_and_an_unsubscribe_frees_a_slot() {
		let mut guard = Guard::new();
		for sub in 0..MAX_SUBSCRIPTIONS as u16 {
			subscribe(&mut guard, sub, [ENGINE, GEARBOX][usize::from(sub % 2)], 0xF40D, 100).unwrap();
		}
		assert_eq!(subscribe(&mut guard, 100, ENGINE, 0xF40D, 100), Err(Refusal::TooManySubscriptions));
		guard.unsubscribed(5);
		subscribe(&mut guard, 100, ENGINE, 0xF40D, 100).expect("the freed slot");
		assert_eq!(subscribe(&mut guard, 101, ENGINE, 0xF40D, 100), Err(Refusal::TooManySubscriptions));
		guard.unsubscribed(5);
		assert_eq!(
			subscribe(&mut guard, 101, ENGINE, 0xF40D, 100),
			Err(Refusal::TooManySubscriptions),
			"unsubscribing a dead id frees nothing"
		);
	}

	#[test]
	fn a_live_subscription_given_again_is_replaced_not_counted_twice() {
		let mut guard = Guard::new();
		for sub in 0..MAX_SUBSCRIPTIONS as u16 {
			subscribe(&mut guard, sub, ENGINE, 0x2029, 100).unwrap();
		}
		guard.unsubscribed(0);
		subscribe(&mut guard, 1, ENGINE, 0x202A, 100).unwrap();
		subscribe(&mut guard, 0, ENGINE, 0x202A, 100).expect("sub 1 was replaced, so slot 0 is still free");
	}

	#[test]
	fn subscribing_the_same_identifier_again_is_not_a_new_distinct_identifier() {
		let mut guard = Guard::new();
		let dids = squares(32);
		for (sub, did) in dids.iter().enumerate() {
			subscribe(&mut guard, sub as u16, ENGINE, *did, 100).unwrap();
		}
		for sub in 0..MAX_SUBSCRIPTIONS as u16 {
			guard.unsubscribed(sub);
		}
		subscribe(&mut guard, 0, ENGINE, dids[7], 100).expect("already asked");
		assert_eq!(
			guard.check_subscribe(ENGINE, resp(ENGINE), 0x4000, 100),
			Verdict::Refuse(Refusal::TooManyDistinct)
		);
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &rdbi(&[0x4000])),
			Verdict::Refuse(Refusal::TooManyDistinct),
			"subscribed identifiers count for reads too"
		);
	}

	#[test]
	fn subscriptions_count_toward_the_walk_rule_and_a_locked_unit_refuses_them() {
		let mut guard = Guard::new();
		for (sub, did) in (0xF100..0xF107).enumerate() {
			subscribe(&mut guard, sub as u16, ENGINE, did, 100).unwrap();
		}
		assert_eq!(guard.check_subscribe(ENGINE, resp(ENGINE), 0xF107, 100), Verdict::Refuse(Refusal::Walk));
		assert_eq!(guard.check_subscribe(ENGINE, resp(ENGINE), 0xF190, 100), Verdict::Refuse(Refusal::Locked));
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &rdbi(&[0xF190])), Verdict::Refuse(Refusal::Locked));

		// And reads followed by a subscribe complete a walk the same way.
		let mut guard = Guard::new();
		forward_now(&mut guard, 0, GEARBOX, &rdbi(&[0x2000, 0x2001, 0x2002, 0x2003]));
		forward_now(&mut guard, 0, GEARBOX, &rdbi(&[0x2004, 0x2005, 0x2006]));
		assert_eq!(guard.check_subscribe(GEARBOX, resp(GEARBOX), 0x2007, 100), Verdict::Refuse(Refusal::Walk));
	}

	#[test]
	fn a_measure_session_counts_only_its_subscribe() {
		// `vagcan measure`: road speed from the gearbox at 50 Hz for a minute.
		let mut guard = Guard::new();
		subscribe(&mut guard, 1, GEARBOX, 0xF40D, 20).unwrap();
		// The board polls 3000 times on its own; none of it reaches the guard,
		// so the rate window is untouched at any point of the run...
		let mut now = 0;
		while now < 60_000 {
			now += 20;
		}
		for _ in 0..RATE_LIMIT {
			forward_now(&mut guard, now, 0x713, &[0x3E, 0x00]);
		}
		// ...and the gearbox has been asked exactly one identifier: 31 more fit.
		guard.unsubscribed(1);
		for did in squares(31) {
			drive(&mut guard, &mut now, GEARBOX, &rdbi(&[did]), None).unwrap();
		}
		assert_eq!(
			guard.check(now, GEARBOX, resp(GEARBOX), &rdbi(&[0x4000])),
			Verdict::Refuse(Refusal::TooManyDistinct)
		);
	}

	#[test]
	fn a_watch_session_starts_at_once_and_runs_for_minutes() {
		// `vagcan watch` over BLE: twenty channels across the engine and the
		// gearbox, subscribed at t = 0 at 100–500 ms each.
		let engine = [0x2029, 0x202A, 0x202B, 0x202F, 0x206E, 0xF405, 0xF40C, 0xF40D, 0xF410, 0x0281];
		let gearbox = [0x028D, 0xF40D, 0x2203, 0x2210, 0x2211, 0x3862, 0x38BC, 0x0600, 0x1030, 0x1D4A];
		let mut guard = Guard::new();
		let mut sub = 0u16;
		for (unit, dids) in [(ENGINE, engine), (GEARBOX, gearbox)] {
			for did in dids {
				let period = 100 + (sub % 5) * 100;
				subscribe(&mut guard, sub, unit, did, period).unwrap_or_else(|r| panic!("{unit:03X} {did:04X}: {r}"));
				sub += 1;
			}
		}
		// Five minutes: the board polls; the host keeps the session alive and
		// once a minute flips to another page and back.
		let mut now = 0;
		while now < 5 * 60_000 {
			drive(&mut guard, &mut now, ENGINE, &[0x3E, 0x00], None).unwrap();
			if now % 60_000 < 2_000 {
				for s in 0..sub {
					guard.unsubscribed(s);
				}
				let mut s = 0u16;
				for (unit, dids) in [(ENGINE, engine), (GEARBOX, gearbox)] {
					for did in dids {
						subscribe(&mut guard, s, unit, did, 100 + (s % 5) * 100).unwrap();
						s += 1;
					}
				}
			}
			now += 2_000;
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
		// The scheduler groups them by unit, one request each.
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

	// --- the cable profile ----------------------------------------------------------

	#[test]
	fn profiles_are_what_they_are_built_as_and_survive_renewal() {
		assert_eq!(Guard::new().profile(), Profile::Radio);
		assert_eq!(Guard::cable().profile(), Profile::Cable);
		assert_eq!(Guard::cable().renewed().profile(), Profile::Cable);
		assert_eq!(Guard::new().renewed().profile(), Profile::Radio);
	}

	#[test]
	fn the_cable_keeps_the_allowlist_and_refuses_the_programming_session() {
		let mut guard = Guard::cable();
		for sid in [0x14, 0x2E, 0x27, 0x31, 0x34, 0x11, 0x28, 0x85] {
			assert_eq!(
				guard.check(0, ENGINE, resp(ENGINE), &[sid, 0x00]),
				Verdict::Refuse(Refusal::ServiceNotAllowed(sid))
			);
		}
		for session in [0x02, 0x82] {
			assert_eq!(
				guard.check(0, ENGINE, resp(ENGINE), &[0x10, session]),
				Verdict::Refuse(Refusal::ProgrammingSession)
			);
		}
		assert_eq!(
			guard.speed(0, ENGINE, resp(ENGINE), &[0x10, 0x02], Some(0)),
			Verdict::Refuse(Refusal::ProgrammingSession)
		);
		assert_eq!(guard.check(0, ENGINE, resp(ENGINE), &[]), Verdict::Refuse(Refusal::Empty));
		assert_eq!(
			guard.check(0, ENGINE, resp(ENGINE), &[0x22, 0xF1]),
			Verdict::Refuse(Refusal::MalformedIdentifiers)
		);
	}

	#[test]
	fn the_cable_keeps_the_speed_gate_on_session_changes() {
		let mut guard = Guard::cable();
		assert_eq!(guard.check(0, 0x713, resp(0x713), &[0x10, 0x03]), Verdict::CheckSpeedFirst);
		assert_eq!(
			guard.speed(0, 0x713, resp(0x713), &[0x10, 0x03], Some(30)),
			Verdict::Refuse(Refusal::Moving(30))
		);
		assert_eq!(
			guard.speed(0, 0x713, resp(0x713), &[0x10, 0x03], None),
			Verdict::Refuse(Refusal::SpeedUnknown)
		);
		assert_eq!(guard.speed(0, 0x713, resp(0x713), &[0x10, 0x03], Some(0)), Verdict::Forward);
		assert_eq!(
			guard.check(0, 0x713, resp(0x713), &[0x10, 0x01]),
			Verdict::Forward,
			"default needs no speed"
		);
	}

	#[test]
	fn the_cable_has_no_rate_cap() {
		let mut guard = Guard::cable();
		for _ in 0..10 * RATE_LIMIT {
			forward_now(&mut guard, 0, ENGINE, &rdbi(&[0xF190, 0xF187, 0xF197, 0xF19E]));
		}
		for _ in 0..10 * RATE_LIMIT {
			assert_eq!(guard.check(0, 0x713, resp(0x713), &[0x10, 0x03]), Verdict::CheckSpeedFirst);
			assert_eq!(guard.speed(0, 0x713, resp(0x713), &[0x10, 0x03], Some(0)), Verdict::Forward);
			guard.forwarded(0, 0x713, resp(0x713), &[0x10, 0x03]);
		}
		assert!(guard.window.is_empty(), "nothing counted, so nothing kept");
	}

	#[test]
	fn the_cable_has_no_walk_rule_no_distinct_cap_and_no_per_request_cap() {
		let mut guard = Guard::cable();
		// `units --identify <unit>`: all of F100–F1FF, one at a time and then in one request.
		for did in 0xF100u16..=0xF1FF {
			forward_now(&mut guard, 0, 0x714, &rdbi(&[did]));
		}
		let all: Vec<u16> = (0xF100u16..=0xF1FF).collect();
		forward_now(&mut guard, 0, 0x714, &rdbi(&all));
		assert_eq!(guard.check_subscribe(0x714, resp(0x714), 0xF1FF, MIN_PERIOD_MS), Verdict::Forward);
		assert!(
			guard.units.values().all(|unit| unit.asked.is_empty() && !unit.locked),
			"no identifier is kept: memory is units and slots only"
		);
	}

	#[test]
	fn the_cable_keeps_the_memory_bounds() {
		let mut guard = Guard::cable();
		for unit in 0x700..0x700 + MAX_UNITS as u16 {
			forward_now(&mut guard, 0, unit, &[0x3E, 0x00]);
		}
		let next = 0x700 + MAX_UNITS as u16;
		assert_eq!(guard.check(0, next, resp(next), &[0x3E, 0x00]), Verdict::Refuse(Refusal::TooManyUnits));
		assert!(matches!(
			guard.check(0, 0x700, 0x123, &[0x3E, 0x00]),
			Verdict::Refuse(Refusal::OtherResponseId { .. })
		));

		let mut guard = Guard::cable();
		assert_eq!(
			guard.check_subscribe(ENGINE, resp(ENGINE), 0xF40D, MIN_PERIOD_MS - 1),
			Verdict::Refuse(Refusal::PeriodTooShort)
		);
		for sub in 0..MAX_SUBSCRIPTIONS as u16 {
			subscribe(&mut guard, sub, ENGINE, 0x2000 + sub, 100).unwrap();
		}
		assert_eq!(subscribe(&mut guard, 100, ENGINE, 0x2100, 100), Err(Refusal::TooManySubscriptions));
	}
}
