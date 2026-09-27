//! One exchange's waits, as the bus task runs them (`transact` in `dash.rs`): how long to wait
//! for the next answer PDU, what each PDU on the unit's answer id is to this request, and how
//! the exchange ends without an answer.
//!
//! Pure, like [`crate::saving`]: the firmware cannot be built for the host, so the rules live
//! here, where `research/dash/host/tests/exchange_waits.rs` compiles this file as it is.
//! Times are milliseconds from the exchange's start, before its send.
//!
//! * **The first answer** within [`Timeouts::answer_ms`] of the send, or
//!   [`Timeouts::suppressed_ms`] for a request that suppressed its positive response.
//! * **`7F sid 78`** (response pending) asks for more time: each one waits
//!   [`Timeouts::pending_wait_ms`] more, all of them together no longer than
//!   [`Timeouts::pending_deadline_ms`] from the first.
//! * **A PDU that answers another request** — another service's, another identifier's,
//!   another sub-function's, or a refusal naming another service
//!   ([`vag_uds_client::schedule::answers`], the laptop's rule too) — is a late answer to an
//!   earlier exchange its consumer stopped waiting for, arriving inside this one: dropped, and
//!   the wait goes on within the time it had (review of `todo/dash/20`, 2026-09-27). Taken as
//!   this exchange's answer, the fault count's late `59 02 …` became the next part-number read's
//!   answer, a part number that did not parse. Only an identical request can still take one.
//! * **An exchange with a deadline of its own** — the fault count's, 2 s from the exchange's
//!   start, the send included, `78`s included — has every wait cut to it.
//! * **How it ends without an answer** is what was heard on the unit's answer id, not what ended
//!   the wait: nothing at all is [`Ended::Silent`], an absent unit; a `78`, or a late answer to
//!   an earlier request, is [`Ended::Busy`] — the unit is there and busy, and the planner must
//!   not take it for silence, which marks it absent and drops its readers (review rounds 1 and
//!   2, 2026-09-27). Every exchange, not only the fault count's.

use vag_uds_client::schedule::{answers, expects_no_answer};

/// ISO 14229-1: a negative response, and the NRC that asks for more time.
const NEGATIVE: u8 = 0x7F;
const RESPONSE_PENDING: u8 = 0x78;

/// The board's waits, in milliseconds (`dash.rs` keeps them, with their reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timeouts {
	/// The first answer PDU, all its frames: `RESPONSE_TIMEOUT`.
	pub answer_ms: u64,
	/// The refusal a request that suppressed its positive response may get: `SUPPRESSED_WAIT`.
	pub suppressed_ms: u64,
	/// The next answer after a `78`: `PENDING_WAIT`, ISO 14229-2's P2*.
	pub pending_wait_ms: u64,
	/// Every `78` together, from the first: `PENDING_DEADLINE`.
	pub pending_deadline_ms: u64,
}

/// What one PDU on the unit's answer id is to this exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Heard {
	/// Its answer — positive, or a refusal — for the planner.
	Answer,
	/// `7F sid 78`: wait again.
	Pending,
	/// A late answer to another request: dropped; wait on within the time left.
	Stray,
}

/// How an exchange ended with no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
	/// Nothing was heard on the unit's answer id: silence, an absent unit.
	Silent,
	/// The unit was heard — it asked for time (`78`), or only sent late answers to earlier
	/// requests (`asked_for_time` false) — and did not answer this request in time.
	Busy { asked_for_time: bool },
}

/// One exchange's waits.
#[derive(Debug, Clone)]
pub struct Waits {
	/// The request's first bytes — its service, and its sub-function or first identifier: what
	/// [`answers`] and [`expects_no_answer`] read of it.
	request: [u8; 3],
	request_len: usize,
	timeouts: Timeouts,
	limit_ms: Option<u64>,
	/// When the wait under way ends, uncut by the limit.
	wait_end_ms: u64,
	/// When the first `78` came.
	pending_since_ms: Option<u64>,
	strays: u32,
	stray_head: [u8; 3],
	stray_len: usize,
}

impl Waits {
	/// The waits of `request`, with the exchange's own deadline if it has one.
	pub fn new(request: &[u8], timeouts: Timeouts, limit_ms: Option<u64>) -> Self {
		let mut head = [0u8; 3];
		let request_len = request.len().min(3);
		head[..request_len].copy_from_slice(&request[..request_len]);
		let mut waits = Waits {
			request: head,
			request_len,
			timeouts,
			limit_ms,
			wait_end_ms: 0,
			pending_since_ms: None,
			strays: 0,
			stray_head: [0; 3],
			stray_len: 0,
		};
		waits.sent(0);
		waits
	}

	/// How long the send may take: `deadline_ms`, cut to the exchange's own deadline.
	pub fn send_wait(&self, deadline_ms: u64) -> Option<u64> {
		self.cut(0, deadline_ms)
	}

	/// The send is done at `now_ms`: the first answer's wait starts.
	pub fn sent(&mut self, now_ms: u64) {
		// A request that suppressed its positive response is answered only by a refusal, and a
		// refusal comes within P2: waiting the full timeout for silence would hold the bus for
		// nothing.
		let first = if expects_no_answer(&self.request[..self.request_len]) {
			self.timeouts.suppressed_ms
		} else {
			self.timeouts.answer_ms
		};
		self.wait_end_ms = now_ms.saturating_add(first);
	}

	/// How long to wait for the next PDU at `now_ms`; `None` once the wait under way is over.
	pub fn next_wait(&self, now_ms: u64) -> Option<u64> {
		let wait = self.wait_end_ms.checked_sub(now_ms).filter(|w| *w > 0)?;
		self.cut(now_ms, wait)
	}

	/// A PDU came at `now_ms`: what it is to this exchange.
	pub fn heard(&mut self, pdu: &[u8], now_ms: u64) -> Heard {
		if !answers(&self.request[..self.request_len], pdu) {
			if self.strays == 0 {
				self.stray_len = pdu.len().min(self.stray_head.len());
				self.stray_head[..self.stray_len].copy_from_slice(&pdu[..self.stray_len]);
			}
			self.strays = self.strays.saturating_add(1);
			return Heard::Stray;
		}
		// `answers` held: a `7F` here names this request's service.
		if !matches!(pdu, [NEGATIVE, _, RESPONSE_PENDING, ..]) {
			return Heard::Answer;
		}
		let since = *self.pending_since_ms.get_or_insert(now_ms);
		let left = since.saturating_add(self.timeouts.pending_deadline_ms).saturating_sub(now_ms);
		self.wait_end_ms = now_ms.saturating_add(left.min(self.timeouts.pending_wait_ms));
		Heard::Pending
	}

	/// How the exchange ended, its last wait over with no answer: busy if anything was heard on
	/// the unit's answer id during it, silent otherwise.
	pub fn ended(&self) -> Ended {
		let asked_for_time = self.pending_since_ms.is_some();
		if asked_for_time || self.strays > 0 {
			Ended::Busy { asked_for_time }
		} else {
			Ended::Silent
		}
	}

	/// How many PDUs were dropped as late answers to another request.
	pub fn strays(&self) -> u32 {
		self.strays
	}

	/// The first bytes of the first of them.
	pub fn first_stray(&self) -> Option<&[u8]> {
		(self.strays > 0).then(|| &self.stray_head[..self.stray_len])
	}

	/// `wait_ms` from `now_ms`, cut to the exchange's own deadline; `None` when that is past.
	fn cut(&self, now_ms: u64, wait_ms: u64) -> Option<u64> {
		match self.limit_ms {
			None => Some(wait_ms),
			Some(limit) => limit.checked_sub(now_ms).filter(|left| *left > 0).map(|left| wait_ms.min(left)),
		}
	}
}
