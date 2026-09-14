//! One host's UDS session with the board, across a link that is not a cable.
//!
//! The board's side of `todo/dash/16-uds-over-ble.md`, with no I/O: link
//! [`Message`]s from the host go in, [`Guard`] decides, the [`Planner`] puts
//! what passes on the bus between the panel's own reads, and link messages for
//! the host come out. The firmware owns the radio, the clock and the planner's
//! lock; everything that can be decided without them is decided here and tested
//! on the laptop.
//!
//! # Requests
//!
//! One at a time, in arrival order. A [`Request`] passes [`Guard::check`]:
//!
//! - [`Verdict::WaitUntil`] — held until [`Session::wake_at`] (slowing is
//!   allowed, dropping is not), then checked again;
//! - [`Verdict::CheckSpeedFirst`] — a fresh [`SPEED_REQUEST`] goes to the
//!   engine as [`Class::Timing`], so it is the next thing on the bus; its answer
//!   ([`Session::answered`]) goes to [`Guard::speed`], and what that says is
//!   done;
//! - [`Verdict::Refuse`] — an [`Answer`] with [`Outcome::Refused`] and the
//!   refusal's text, and nothing reaches the bus;
//! - [`Verdict::Forward`] — [`Planner::exchange`] as [`Class::Remote`],
//!   [`Guard::forwarded`], and the unit's answer comes back as an [`Answer`].
//!
//! # Subscriptions
//!
//! Handled as they arrive, not behind a request that is waiting: a `measure`
//! session subscribing while a slow read is out should not wait for it.
//! [`Guard::check_subscribe`] then [`Planner::subscribe`] — as [`Class::Remote`], or as
//! [`Class::Timing`] when the host marked it [`Priority::Timing`];
//! a refusal is a [`Reading`] with [`Outcome::Refused`]. Every planner delivery
//! for a live subscription becomes a [`Reading`] ([`Session::deliver`]) stamped
//! with the moment the answer arrived. A subscription given again under a live
//! `sub` replaces it.
//!
//! When the guard locks a unit (a walk), every live subscription of this
//! session to that unit ends with a [`Reading`] saying so. [`Session::close`]
//! — the connection is gone — ends all of them: a dead consumer takes its
//! subscriptions with it.
//!
//! # Why a host may hold a timing subscription
//!
//! `measure` times a run from one speed channel at 50 Hz. As [`Class::Remote`] it waits
//! behind everything else once the planner is at its ceiling, and on the bench it came
//! at 10 Hz (2026-09-14). [`Class::Timing`] is never thinned, so the guard alone bounds
//! what a host takes that way: at most
//! [`MAX_TIMING_SUBSCRIPTIONS`](crate::guard::MAX_TIMING_SUBSCRIPTIONS) per connection,
//! polled no faster than [`MIN_PERIOD_MS`](crate::guard::MIN_PERIOD_MS) — one channel,
//! at most 50 of the planner's 100 exchanges a second. The panel's floor of 25 fits in
//! what is left, the ceiling is the
//! planner's to hold whatever is asked, and every other subscription of the host stays
//! `Remote`: slowed when the bus is short, never dropped.
//!
//! The bound is per connection, and the board runs a radio session and a cable session
//! side by side: a timing subscription on each, to different identifiers, is 100 a second
//! and the panel waits behind them while both last (`todo/dash/16`, open).

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use vag_uds_transport::link::{Answer, Message, Outcome, Priority, Reading, Request, Subscribe};

use crate::guard::{Guard, Refusal, SPEED_REQUEST, SPEED_REQUEST_ID, SPEED_RESPONSE_ID, Verdict, road_speed};
use crate::schedule::{self, Class, Delivery, Miss, Planner, ReqId, SubId, Unit};

/// ReadDataByIdentifier's positive response and a negative response's first byte (ISO 14229-1).
const RDBI: u8 = 0x22;
const RDBI_POSITIVE: u8 = 0x62;
const NEGATIVE: u8 = 0x7F;

/// The engine, where road speed is read (ISO 15765-4's first address pair).
const SPEED_UNIT: Unit = Unit {
	request: SPEED_REQUEST_ID,
	response: SPEED_RESPONSE_ID,
};

/// One connection's session. Dropping it without [`Session::close`] leaves its
/// subscriptions in the planner, so the shell closes it on disconnect.
#[derive(Debug, Default)]
pub struct Session {
	guard: Guard,
	/// The host's subscription ids and what they are in the planner.
	subs: BTreeMap<u16, Live>,
	/// Requests not yet begun, oldest first.
	queue: VecDeque<Request>,
	/// The request being dealt with.
	current: Option<Current>,
}

#[derive(Debug, Clone, Copy)]
struct Live {
	id: SubId,
	request_id: u16,
}

#[derive(Debug)]
enum Current {
	/// Over the rate cap; checked again at `until_ms`.
	Waiting { request: Request, until_ms: u64 },
	/// The speed read `req` is out.
	Speed { request: Request, req: ReqId },
	/// The request itself is out as `req`; `sid` is its service.
	Forwarded { seq: u8, sid: u8, req: ReqId },
}

impl Session {
	/// A session across the radio: [`Guard::new`].
	pub fn new() -> Self {
		Self::default()
	}

	/// A session held to `guard`'s profile — [`Guard::cable`] for the board's USB cable.
	pub fn with_guard(guard: Guard) -> Self {
		Session {
			guard: guard.renewed(),
			..Self::default()
		}
	}

	/// The PDU bytes this session holds for requests not yet handed to the planner: what a
	/// host that floods large requests pins here. A shell bounds it by not reading the
	/// link while it is high; the one request out is the planner's, at most `MAX_PDU`.
	pub fn queued_bytes(&self) -> usize {
		let current = match &self.current {
			Some(Current::Waiting { request, .. } | Current::Speed { request, .. }) => request.pdu.len(),
			Some(Current::Forwarded { .. }) | None => 0,
		};
		current + self.queue.iter().map(|request| request.pdu.len()).sum::<usize>()
	}

	/// Whether a host holds anything here: a live subscription, or a request queued or
	/// out. A carrier that also takes other protocols (the USB cable's slcan lines)
	/// does not let them in while this is so.
	pub fn is_active(&self) -> bool {
		!self.subs.is_empty() || self.queued() > 0
	}

	/// Requests waiting behind the current one. A shell that stops reading the
	/// link while this is high slows the host down instead of queueing without end.
	pub fn queued(&self) -> usize {
		self.queue.len() + usize::from(self.current.is_some())
	}

	/// The time the request that is waiting out the rate cap may be checked again.
	pub fn wake_at(&self) -> Option<u64> {
		match self.current {
			Some(Current::Waiting { until_ms, .. }) => Some(until_ms),
			_ => None,
		}
	}

	/// The planner subscriptions this session owns: every delivery for one of them is
	/// this session's host's, and a delivery for none of them is nobody's.
	pub fn subscriptions(&self) -> impl Iterator<Item = SubId> + '_ {
		self.subs.values().map(|live| live.id)
	}

	/// The planner exchange whose [`Delivery::Raw`] this session waits for.
	pub fn awaiting(&self) -> Option<ReqId> {
		match self.current {
			Some(Current::Speed { req, .. } | Current::Forwarded { req, .. }) => Some(req),
			_ => None,
		}
	}

	/// Take one message from the host. Subscriptions are acted on now; a request
	/// is queued and begun by [`Session::poll`]. Messages only the board sends
	/// (`Answer`, `Reading`) mean nothing from the host and are ignored.
	pub fn push(&mut self, now_ms: u64, planner: &mut Planner, message: Message) -> Vec<Message> {
		let mut out = Vec::new();
		match message {
			Message::Request(request) => self.queue.push_back(request),
			Message::Subscribe(s) => self.subscribe(now_ms, planner, s, &mut out),
			Message::Unsubscribe { sub } => self.unsubscribe(planner, sub),
			// A Hello is the shell's to answer: only it knows the image, and it closes the
			// session first ([`Session::close`]).
			Message::Answer(_) | Message::Reading(_) | Message::Hello | Message::HelloReply(_) => {}
		}
		out.extend(self.poll(now_ms, planner));
		out
	}

	/// Take one message from the host while the board puts nothing on the bus — the
	/// `dash` image in its adapter mode, where the USB cable's host drives the pair itself.
	///
	/// A request is answered [`Outcome::Refused`] with `reason`, a subscription gets one
	/// refused [`Reading`], and neither reaches the guard or the planner: nothing is
	/// counted, nothing is queued to go out later. A subscription given again under a
	/// live `sub` ends the live one first, as [`Session::push`] would replace it. An
	/// unsubscribe is honoured, since it only frees.
	pub fn push_refused(&mut self, now_ms: u64, planner: &mut Planner, message: Message, reason: &str) -> Vec<Message> {
		match message {
			Message::Request(request) => vec![Message::Answer(Answer {
				seq: request.seq,
				outcome: Outcome::Refused(String::from(reason)),
			})],
			Message::Subscribe(s) => {
				self.unsubscribe(planner, s.sub);
				vec![Message::Reading(Reading {
					sub: s.sub,
					at_ms: now_ms as u32,
					outcome: Outcome::Refused(String::from(reason)),
				})]
			}
			Message::Unsubscribe { sub } => {
				self.unsubscribe(planner, sub);
				Vec::new()
			}
			Message::Answer(_) | Message::Reading(_) | Message::Hello | Message::HelloReply(_) => Vec::new(),
		}
	}

	/// The board stops putting anything on the bus: every request this session's host
	/// still waits for is answered [`Outcome::Refused`] with `reason`, in the order they
	/// came — the one being dealt with first. An exchange handed to the planner is
	/// cancelled if it has not gone out; one already on the bus finds nobody when it is
	/// answered. Subscriptions stay: the planner keeps them and they resume when the bus
	/// does, the way the panel's own do.
	pub fn refuse_pending(&mut self, planner: &mut Planner, reason: &str) -> Vec<Message> {
		if let Some(req) = self.awaiting() {
			planner.cancel(req);
		}
		let current = self.current.take().map(|current| match current {
			Current::Waiting { request, .. } | Current::Speed { request, .. } => request.seq,
			Current::Forwarded { seq, .. } => seq,
		});
		current
			.into_iter()
			.chain(self.queue.drain(..).map(|request| request.seq))
			.map(|seq| {
				Message::Answer(Answer {
					seq,
					outcome: Outcome::Refused(String::from(reason)),
				})
			})
			.collect()
	}

	/// Move the request queue as far as it goes at `now_ms`.
	pub fn poll(&mut self, now_ms: u64, planner: &mut Planner) -> Vec<Message> {
		let mut out = Vec::new();
		loop {
			let (request, verdict) = match self.current.take() {
				None => match self.queue.pop_front() {
					Some(request) => {
						let verdict = self.guard.check(now_ms, request.request_id, request.response_id, &request.pdu);
						(request, verdict)
					}
					None => break,
				},
				Some(Current::Waiting { request, until_ms }) if until_ms <= now_ms => {
					let verdict = self.guard.check(now_ms, request.request_id, request.response_id, &request.pdu);
					(request, verdict)
				}
				Some(blocked) => {
					self.current = Some(blocked);
					break;
				}
			};
			self.act(now_ms, planner, request, verdict, &mut out);
		}
		out
	}

	/// The answer to the exchange [`Session::awaiting`] named; anything else is
	/// ignored. `now_ms` is the moment the answer arrived.
	pub fn answered(&mut self, now_ms: u64, planner: &mut Planner, req: ReqId, answer: &schedule::Answer) -> Vec<Message> {
		let mut out = Vec::new();
		match self.current.take() {
			Some(Current::Speed { request, req: out_req }) if out_req == req => {
				let kmh = match answer {
					schedule::Answer::Pdu(pdu) => road_speed(pdu),
					_ => None,
				};
				let verdict = self.guard.speed(now_ms, request.request_id, request.response_id, &request.pdu, kmh);
				self.act(now_ms, planner, request, verdict, &mut out);
			}
			Some(Current::Forwarded { seq, sid, req: out_req }) if out_req == req => {
				out.push(Message::Answer(Answer {
					seq,
					outcome: outcome_of(sid, answer),
				}));
			}
			other => self.current = other,
		}
		out.extend(self.poll(now_ms, planner));
		out
	}

	/// The [`Reading`] a planner delivery makes for this session's host, if the
	/// delivery is for one of its live subscriptions.
	pub fn deliver(&self, delivery: &Delivery) -> Option<Message> {
		let (sub, at_ms, outcome) = match delivery {
			Delivery::Reading { sub, did, data, at_ms, .. } => {
				let mut pdu = Vec::with_capacity(3 + data.len());
				pdu.push(RDBI_POSITIVE);
				pdu.extend_from_slice(&did.to_be_bytes());
				pdu.extend_from_slice(data);
				(*sub, *at_ms, Outcome::Pdu(pdu))
			}
			Delivery::Missed { sub, why, at_ms, .. } => (*sub, *at_ms, missed(*why)),
			Delivery::Once { .. } | Delivery::Raw { .. } => return None,
		};
		let host_sub = self.subs.iter().find(|(_, live)| live.id == sub).map(|(host, _)| *host)?;
		Some(Message::Reading(Reading {
			sub: host_sub,
			at_ms: at_ms as u32,
			outcome,
		}))
	}

	/// The connection is gone: every subscription leaves the planner, nothing
	/// queued is begun, and the exchange handed to the planner is cancelled if it has not
	/// gone out ([`Planner::cancel`]). One already on the bus cannot be recalled; its
	/// answer finds nobody.
	pub fn close(&mut self, planner: &mut Planner) {
		for (_, live) in core::mem::take(&mut self.subs) {
			planner.unsubscribe(live.id);
		}
		if let Some(req) = self.awaiting() {
			planner.cancel(req);
		}
		self.queue.clear();
		self.current = None;
		self.guard = self.guard.renewed();
	}

	fn act(&mut self, now_ms: u64, planner: &mut Planner, request: Request, verdict: Verdict, out: &mut Vec<Message>) {
		match verdict {
			Verdict::WaitUntil(until_ms) => self.current = Some(Current::Waiting { request, until_ms }),
			Verdict::CheckSpeedFirst => {
				let req = planner
					.exchange(now_ms, Class::Timing, SPEED_UNIT, SPEED_REQUEST.to_vec())
					.expect("a read is inside the allowlist");
				self.current = Some(Current::Speed { request, req });
			}
			Verdict::Refuse(refusal) => {
				if refusal == Refusal::Walk {
					self.end_unit(now_ms, planner, request.request_id, out);
				}
				out.push(Message::Answer(Answer {
					seq: request.seq,
					outcome: Outcome::Refused(refusal.to_string()),
				}));
			}
			Verdict::Forward => {
				let unit = Unit {
					request: request.request_id,
					response: request.response_id,
				};
				match planner.exchange(now_ms, Class::Remote, unit, request.pdu.clone()) {
					Ok(req) => {
						self.guard.forwarded(now_ms, request.request_id, request.response_id, &request.pdu);
						self.current = Some(Current::Forwarded {
							seq: request.seq,
							sid: request.pdu[0],
							req,
						});
					}
					// The guard's allowlist is the planner's; this is the second lock on one door.
					Err(e) => out.push(Message::Answer(Answer {
						seq: request.seq,
						outcome: Outcome::Refused(e.to_string()),
					})),
				}
			}
		}
	}

	fn subscribe(&mut self, now_ms: u64, planner: &mut Planner, s: Subscribe, out: &mut Vec<Message>) {
		// Given again, a live id is replaced: the old one goes first, so it does not
		// hold the slot the new one needs.
		self.unsubscribe(planner, s.sub);
		match self.guard.check_subscribe(s.request_id, s.response_id, s.did, s.period_ms, s.priority) {
			Verdict::Forward => {
				let unit = Unit {
					request: s.request_id,
					response: s.response_id,
				};
				let id = planner.subscribe(now_ms, class_of(s.priority), unit, s.did, u32::from(s.period_ms), None);
				self.guard.subscribed(s.sub, s.request_id, s.response_id, s.did, s.priority);
				self.subs.insert(
					s.sub,
					Live {
						id,
						request_id: s.request_id,
					},
				);
			}
			Verdict::Refuse(refusal) => {
				if refusal == Refusal::Walk {
					self.end_unit(now_ms, planner, s.request_id, out);
				}
				out.push(refused_reading(s.sub, now_ms, refusal));
			}
			// `check_subscribe` never waits and never needs speed: a subscription is only a read.
			Verdict::WaitUntil(_) | Verdict::CheckSpeedFirst => out.push(refused_reading(s.sub, now_ms, Refusal::MalformedIdentifiers)),
		}
	}

	fn unsubscribe(&mut self, planner: &mut Planner, sub: u16) {
		if let Some(live) = self.subs.remove(&sub) {
			planner.unsubscribe(live.id);
			self.guard.unsubscribed(sub);
		}
	}

	/// The guard locked `request_id`: its subscriptions of this session end, each said.
	fn end_unit(&mut self, now_ms: u64, planner: &mut Planner, request_id: u16, out: &mut Vec<Message>) {
		let ended: Vec<u16> = self.subs.iter().filter(|(_, l)| l.request_id == request_id).map(|(s, _)| *s).collect();
		for sub in ended {
			self.unsubscribe(planner, sub);
			out.push(refused_reading(sub, now_ms, Refusal::Locked));
		}
	}
}

/// The planner class a host's subscription is polled as (module docs, "Why a host may
/// hold a timing subscription").
fn class_of(priority: Priority) -> Class {
	match priority {
		Priority::Normal => Class::Remote,
		Priority::Timing => Class::Timing,
	}
}

/// A planner answer to a request with service `sid`, as the link carries it. A
/// refusal the shell reported by its NRC alone is rebuilt as `7F sid nrc`, which is
/// all a negative response is.
fn outcome_of(sid: u8, answer: &schedule::Answer) -> Outcome {
	match answer {
		schedule::Answer::Pdu(pdu) if pdu.is_empty() => Outcome::BusError(String::from("the unit's answer was empty")),
		schedule::Answer::Pdu(pdu) => Outcome::Pdu(pdu.clone()),
		schedule::Answer::Refused(nrc) => Outcome::Pdu(vec![NEGATIVE, sid, *nrc]),
		schedule::Answer::NoAnswer => Outcome::NoAnswer,
		schedule::Answer::BusError => Outcome::BusError(String::from("the bus failed under the request")),
		// The request suppressed its positive response and no refusal came: status 1, no
		// answer — which is what was asked for (`link`'s status table says so).
		schedule::Answer::NotExpected => Outcome::NoAnswer,
	}
}

/// A subscriber's missed reading as the link carries it.
///
/// A refusal is the unit's own negative response, rebuilt from its NRC — the
/// planner keeps only the code, and `7F 22 nrc` is all a negative response to a
/// read ever was. An identifier left out of a multi-identifier answer is no
/// answer for that identifier; an answer that did not parse is reported as a
/// failure with its reason, not passed on as data.
fn missed(why: Miss) -> Outcome {
	match why {
		Miss::NoAnswer | Miss::Absent => Outcome::NoAnswer,
		Miss::Refused(nrc) => Outcome::Pdu(vec![NEGATIVE, RDBI, nrc]),
		Miss::BusError => Outcome::BusError(String::from("the bus failed under the request")),
		Miss::Malformed => Outcome::BusError(String::from("the unit's answer did not parse")),
	}
}

fn refused_reading(sub: u16, now_ms: u64, refusal: Refusal) -> Message {
	Message::Reading(Reading {
		sub,
		at_ms: now_ms as u32,
		outcome: Outcome::Refused(refusal.to_string()),
	})
}

#[cfg(test)]
mod tests;
