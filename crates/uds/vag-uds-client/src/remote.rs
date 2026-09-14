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
//!   [`Guard::forwarded`], and the unit's answer comes back as an [`Answer`]. A session
//!   change the speed read cleared goes as [`Class::Timing`] instead, through
//!   [`Planner::exchange_until`]: if it has not gone out within
//!   [`SPEED_FRESH_MS`] of the speed answer, it is taken back and checked again from the
//!   start, which reads speed again. A reading of 0 is only true for a moment.
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
//! subscriptions with it. It does not end what the radio guard remembers: the board
//! keeps one radio session and closes it, so a Hello or a reconnect starts with the
//! locks, the rate window and the units the last connection left (`guard` module docs,
//! "Memory").
//!
//! # Why a host may hold a timing subscription
//!
//! `measure` times a run from one speed channel at 50 Hz. As [`Class::Remote`] it waits
//! behind everything else once the planner is at its ceiling, and on the bench it came
//! at 10 Hz (2026-09-14). [`Class::Timing`] goes ahead of a host's other work, so how many
//! timing subscriptions hosts hold is bounded by two rules:
//!
//! - **per connection**, the guard's
//!   [`MAX_TIMING_SUBSCRIPTIONS`](crate::guard::MAX_TIMING_SUBSCRIPTIONS), polled no
//!   faster than [`MIN_PERIOD_MS`](crate::guard::MIN_PERIOD_MS);
//! - **for the whole board, one timing channel** (one stopwatch at a time, 2026-09-14).
//!   The board runs a radio session and a cable session side by side on one planner, so a
//!   timing subscription is forwarded only while that planner holds no other
//!   ([`Planner::timing_subscriptions`]); otherwise it is refused with
//!   [`Refusal::TimingChannelHeld`] — except that the **cable's owner comes first**
//!   (S-F3): a cable timing subscription preempts a radio-held one
//!   ([`Planner::preempt_timing`]), and the radio session learns of it on its next poll
//!   ([`Session::drain_taken_timing`]) and ends its subscription with
//!   [`Refusal::TimingChannelTaken`]. A radio host never preempts the cable. The channel
//!   frees when its holder unsubscribes, gives its id again as normal, or its session
//!   closes — a disconnect, a Hello, a stalled writer.
//!
//! # Bus time
//!
//! A request to an id nobody answers holds the bus for the whole answer timeout, and a
//! different id each time dodges the planner's per-unit backoff, so a stranger over the
//! radio could deny the panel and the cable host the bus (S-F3). Each radio session is
//! charged the time its exchanges hold the bus ([`Session::charge_bus_time`], measured
//! send to final answer, `78` waits included); past
//! [`RADIO_BUS_SHARE_PERMILLE`](crate::guard::RADIO_BUS_SHARE_PERMILLE) of a sliding
//! [`RATE_WINDOW_MS`](crate::guard::RATE_WINDOW_MS) its next request waits
//! ([`Session::bus_time_wait`]) — back-pressure, nothing dropped. The cable is not charged.
//!
//! Neither bounds bus time. The planner caps sends, not how long a unit takes to answer,
//! and a timing read on a unit slower than its period is due again the moment it answers.
//! So on the board the panel's floor goes ahead of a host's timing channel
//! ([`Budget::board`](crate::schedule::Budget::board), `timing_yields_to_floor`): the
//! panel keeps its floor, and the timing channel gets what is left — 50 a second from a
//! unit that answers in a few milliseconds, less from a slow one. A host's other
//! subscriptions stay `Remote` and get what the timing channel leaves: slowed when the bus
//! is short, and with a slow timing unit one reading each time they have waited the
//! planner's `starve_after_ms`.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use vag_uds_transport::link::{Answer, Message, Outcome, Priority, Reading, Request, Subscribe};

use crate::guard::{
	Guard, Profile, RADIO_BUS_SHARE_PERMILLE, RATE_WINDOW_MS, Refusal, SPEED_FRESH_MS, SPEED_REQUEST, SPEED_REQUEST_ID, SPEED_RESPONSE_ID, Verdict,
	road_speed,
};
use crate::schedule::{self, Class, Delivery, Miss, Planner, ReqId, SubId, Unit};

/// ReadDataByIdentifier's positive response and a negative response's first byte (ISO 14229-1).
const RDBI: u8 = 0x22;
const RDBI_POSITIVE: u8 = 0x62;
const NEGATIVE: u8 = 0x7F;

/// The largest a subscription reading may be — the board delivers it and its subscription
/// ends past this (S-F4, `Session::deliver`). A watch cell or `measure`'s speed is a few
/// bytes; the board's reading queue holds a bounded number of them, so a big record would
/// starve the heap. 512 bytes leaves any real channel room to spare. A big record is a
/// one-shot's job, over a raw exchange, which is not capped.
pub const MAX_READING_BYTES: usize = 512;

/// The engine, where road speed is read (ISO 15765-4's first address pair).
const SPEED_UNIT: Unit = Unit {
	request: SPEED_REQUEST_ID,
	response: SPEED_RESPONSE_ID,
};

/// One host's session. The board keeps one for the radio for its whole life, closed and
/// used again on every disconnect and Hello — that is what makes the radio guard's memory
/// the board's (`guard` module docs, "Memory") — and one for the cable. Dropping it without
/// [`Session::close`] leaves its subscriptions in the planner, so the shell closes it on
/// disconnect.
#[derive(Debug, Default)]
pub struct Session {
	guard: Guard,
	/// The host's subscription ids and what they are in the planner.
	subs: BTreeMap<u16, Live>,
	/// Requests not yet begun, oldest first.
	queue: VecDeque<Request>,
	/// The request being dealt with.
	current: Option<Current>,
	/// `(when it left the bus, how long it held it)` of this host's exchanges, oldest
	/// first, for the radio's bus-time share (`Guard::RADIO_BUS_SHARE_PERMILLE`). Always
	/// empty on the cable, which is not held to it.
	bus_time: VecDeque<(u64, u64)>,
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
	/// The speed read `req` is out; it went to the planner at `since_ms`.
	Speed { request: Request, req: ReqId, since_ms: u64 },
	/// The request itself is out as `req`; `sid` is its service. `fresh` for a session
	/// change a speed read cleared; it went to the planner at `since_ms`.
	Forwarded {
		seq: u8,
		sid: u8,
		req: ReqId,
		fresh: Option<Fresh>,
		since_ms: u64,
	},
}

/// A session change handed to the planner while its road speed reading is fresh.
#[derive(Debug)]
struct Fresh {
	/// Kept to be checked again if it does not go out in time.
	request: Request,
	/// The planner's `not_after_ms`: the speed answer's arrival plus [`SPEED_FRESH_MS`].
	not_after_ms: u64,
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
			Some(
				Current::Waiting { request, .. }
				| Current::Speed { request, .. }
				| Current::Forwarded {
					fresh: Some(Fresh { request, .. }),
					..
				},
			) => request.pdu.len(),
			Some(Current::Forwarded { fresh: None, .. }) | None => 0,
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

	/// When [`Session::poll`] has something to do by time alone: the request waiting out the
	/// rate cap may be checked again, or a session change whose speed reading has gone stale
	/// is taken back if it has not gone out.
	pub fn wake_at(&self) -> Option<u64> {
		match &self.current {
			Some(Current::Waiting { until_ms, .. }) => Some(*until_ms),
			Some(Current::Forwarded {
				fresh: Some(Fresh { not_after_ms, .. }),
				..
			}) => Some(not_after_ms + 1),
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
			Message::Unsubscribe { sub } => self.unsubscribe(now_ms, planner, sub),
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
				self.unsubscribe(now_ms, planner, s.sub);
				vec![Message::Reading(Reading {
					sub: s.sub,
					at_ms: now_ms as u32,
					outcome: Outcome::Refused(String::from(reason)),
				})]
			}
			Message::Unsubscribe { sub } => {
				self.unsubscribe(now_ms, planner, sub);
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

	/// Move the request queue as far as it goes at `now_ms`. A radio host over its bus-time
	/// share ([`Session::bus_time_wait`]) has its next request held here first, whatever the
	/// guard would say.
	pub fn poll(&mut self, now_ms: u64, planner: &mut Planner) -> Vec<Message> {
		let mut out = Vec::new();
		self.drain_taken_timing(now_ms, planner, &mut out);
		loop {
			// A fresh start (a queued request, or one done waiting) is held while the host is
			// over its bus-time share; a session change resumed from its stale reading is not
			// a new start — it reads speed again, and that read is charged when it answers.
			let request = match self.current.take() {
				None => match self.queue.pop_front() {
					Some(request) => request,
					None => break,
				},
				Some(Current::Waiting { request, until_ms }) if until_ms <= now_ms => request,
				Some(Current::Forwarded {
					seq,
					sid,
					req,
					fresh: Some(fresh),
					since_ms,
				}) if fresh.not_after_ms < now_ms => {
					if !planner.cancel(req) {
						// It went out while the reading was fresh: its answer is coming.
						self.current = Some(Current::Forwarded {
							seq,
							sid,
							req,
							fresh: None,
							since_ms,
						});
						break;
					}
					// It did not, and the planner will not send it now: start again, speed read
					// and all.
					let request = fresh.request;
					let verdict = self.guard.check(now_ms, request.request_id, request.response_id, &request.pdu);
					self.act(now_ms, planner, request, verdict, false, &mut out);
					continue;
				}
				Some(blocked) => {
					self.current = Some(blocked);
					break;
				}
			};
			if let Some(until_ms) = self.bus_time_wait(now_ms) {
				self.current = Some(Current::Waiting { request, until_ms });
				break;
			}
			let verdict = self.guard.check(now_ms, request.request_id, request.response_id, &request.pdu);
			self.act(now_ms, planner, request, verdict, false, &mut out);
		}
		out
	}

	/// The answer to the exchange [`Session::awaiting`] named; anything else is
	/// ignored. `now_ms` is the moment the answer arrived.
	pub fn answered(&mut self, now_ms: u64, planner: &mut Planner, req: ReqId, answer: &schedule::Answer) -> Vec<Message> {
		let mut out = Vec::new();
		match self.current.take() {
			Some(Current::Speed {
				request,
				req: out_req,
				since_ms,
			}) if out_req == req => {
				self.charge_bus_time(now_ms, since_ms);
				let kmh = match answer {
					schedule::Answer::Pdu(pdu) => road_speed(pdu),
					_ => None,
				};
				let verdict = self.guard.speed(now_ms, request.request_id, request.response_id, &request.pdu, kmh);
				self.act(now_ms, planner, request, verdict, true, &mut out);
			}
			Some(Current::Forwarded {
				seq,
				sid,
				req: out_req,
				since_ms,
				..
			}) if out_req == req => {
				self.charge_bus_time(now_ms, since_ms);
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
	///
	/// A reading larger than [`MAX_READING_BYTES`] is not passed on: a subscription is for
	/// small, frequent records (a watch cell, `measure`'s speed), and a big one would sit in
	/// the board's bounded reading queue and starve the heap (S-F4). It comes back as a
	/// failed reading and the subscription **ends** — a big record is a one-shot's job, over
	/// a raw exchange, which keeps its full size.
	pub fn deliver(&mut self, now_ms: u64, planner: &mut Planner, delivery: &Delivery) -> Option<Message> {
		let (sub, at_ms, outcome) = match delivery {
			Delivery::Reading { sub, did, data, at_ms, .. } => {
				let host_sub = self.host_sub(*sub)?;
				if 3 + data.len() > MAX_READING_BYTES {
					self.unsubscribe(now_ms, planner, host_sub);
					return Some(refused_reading(host_sub, *at_ms, Refusal::ReadingTooLarge));
				}
				let mut pdu = Vec::with_capacity(3 + data.len());
				pdu.push(RDBI_POSITIVE);
				pdu.extend_from_slice(&did.to_be_bytes());
				pdu.extend_from_slice(data);
				(*sub, *at_ms, Outcome::Pdu(pdu))
			}
			Delivery::Missed { sub, why, at_ms, .. } => (*sub, *at_ms, missed(*why)),
			Delivery::Once { .. } | Delivery::Raw { .. } => return None,
		};
		let host_sub = self.host_sub(sub)?;
		Some(Message::Reading(Reading {
			sub: host_sub,
			at_ms: at_ms as u32,
			outcome,
		}))
	}

	/// The host's id for one of this session's planner subscriptions.
	fn host_sub(&self, id: SubId) -> Option<u16> {
		self.subs.iter().find(|(_, live)| live.id == id).map(|(host, _)| *host)
	}

	/// The connection is gone, or a Hello starts it over: every subscription leaves the
	/// planner, nothing queued is begun, and the exchange handed to the planner is cancelled
	/// if it has not gone out ([`Planner::cancel`]). One already on the bus cannot be
	/// recalled; its answer finds nobody. The guard frees the subscription slots
	/// ([`Guard::close`]): over the radio its memory stays, on the cable it is the reset.
	pub fn close(&mut self, now_ms: u64, planner: &mut Planner) {
		for (_, live) in core::mem::take(&mut self.subs) {
			planner.unsubscribe(live.id);
		}
		if let Some(req) = self.awaiting() {
			planner.cancel(req);
		}
		self.queue.clear();
		self.current = None;
		self.bus_time.clear();
		self.guard.close(now_ms);
	}

	/// Do what the guard decided. `speed_cleared`: the verdict follows a speed read that
	/// answered just now, at `now_ms` — a forward then has [`SPEED_FRESH_MS`] to go out.
	fn act(&mut self, now_ms: u64, planner: &mut Planner, request: Request, verdict: Verdict, speed_cleared: bool, out: &mut Vec<Message>) {
		match verdict {
			Verdict::WaitUntil(until_ms) => self.current = Some(Current::Waiting { request, until_ms }),
			Verdict::CheckSpeedFirst => {
				let req = planner
					.exchange(now_ms, Class::Timing, SPEED_UNIT, SPEED_REQUEST.to_vec())
					.expect("a read is inside the allowlist");
				self.current = Some(Current::Speed {
					request,
					req,
					since_ms: now_ms,
				});
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
				// A session change goes while the car is known to stand: as the next Timing
				// exchange, and never after its reading goes stale.
				let not_after_ms = speed_cleared.then_some(now_ms + SPEED_FRESH_MS);
				let queued = match not_after_ms {
					Some(not_after_ms) => planner.exchange_until(now_ms, Class::Timing, unit, request.pdu.clone(), not_after_ms),
					None => planner.exchange(now_ms, Class::Remote, unit, request.pdu.clone()),
				};
				match queued {
					Ok(req) => {
						self.guard.forwarded(now_ms, request.request_id, request.response_id, &request.pdu);
						let (seq, sid) = (request.seq, request.pdu[0]);
						let fresh = not_after_ms.map(|not_after_ms| Fresh { request, not_after_ms });
						self.current = Some(Current::Forwarded {
							seq,
							sid,
							req,
							fresh,
							since_ms: now_ms,
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
		self.unsubscribe(now_ms, planner, s.sub);
		// One stopwatch at a time on the board (module docs): if the planner every session
		// shares already holds a timing subscription, and it is not this one (which went
		// above), the cable takes it from whoever holds it (S-F3) and the radio is refused.
		let held = s.priority == Priority::Timing && planner.timing_subscriptions() > 0;
		let cable = self.guard.profile() == Profile::Cable;
		let verdict = match self
			.guard
			.check_subscribe(now_ms, s.request_id, s.response_id, s.did, s.period_ms, s.priority)
		{
			Verdict::Forward if held && !cable => Verdict::Refuse(Refusal::TimingChannelHeld),
			verdict => verdict,
		};
		match verdict {
			Verdict::Forward => {
				let unit = Unit {
					request: s.request_id,
					response: s.response_id,
				};
				let id = planner.subscribe(now_ms, class_of(s.priority), unit, s.did, u32::from(s.period_ms), None);
				// The cable took the channel: end every other timing subscription in the planner.
				// Its holder — the radio session — learns of it on its next poll
				// ([`Session::drain_taken_timing`]) and tells its host.
				if held && cable {
					planner.preempt_timing(id);
				}
				self.guard.subscribed(now_ms, s.sub, s.request_id, s.response_id, s.did, s.priority);
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

	fn unsubscribe(&mut self, now_ms: u64, planner: &mut Planner, sub: u16) {
		if let Some(live) = self.subs.remove(&sub) {
			planner.unsubscribe(live.id);
			self.guard.unsubscribed(now_ms, sub);
		}
	}

	/// A subscription of this session the planner no longer holds was preempted — the cable
	/// took the board's one timing channel (S-F3). Each ends with a [`Reading`] saying so.
	/// (Only preemption drops a live subscription silently; every other end goes through
	/// [`Session::unsubscribe`], which takes it out of `subs` too.)
	fn drain_taken_timing(&mut self, now_ms: u64, planner: &mut Planner, out: &mut Vec<Message>) {
		let taken: Vec<u16> = self
			.subs
			.iter()
			.filter(|(_, live)| !planner.holds(live.id))
			.map(|(sub, _)| *sub)
			.collect();
		for sub in taken {
			self.unsubscribe(now_ms, planner, sub);
			out.push(refused_reading(sub, now_ms, Refusal::TimingChannelTaken));
		}
	}

	/// Charge the bus the time an exchange held it — sent to the planner at `since_ms`,
	/// answered now — for the radio's bus-time share. Nothing on the cable.
	fn charge_bus_time(&mut self, now_ms: u64, since_ms: u64) {
		if self.guard.profile() == Profile::Radio {
			self.bus_time.push_back((now_ms, now_ms.saturating_sub(since_ms)));
		}
	}

	/// When the radio host's next request may start: `None` while it holds under
	/// [`RADIO_BUS_SHARE_PERMILLE`] of the bus over the last [`RATE_WINDOW_MS`], else the
	/// moment enough of its held time ages out. Never gates the cable.
	fn bus_time_wait(&mut self, now_ms: u64) -> Option<u64> {
		if self.guard.profile() != Profile::Radio {
			return None;
		}
		let expiry = |at: u64| at.saturating_add(RATE_WINDOW_MS);
		self.bus_time.retain(|&(at, _)| expiry(at) > now_ms);
		let share = RATE_WINDOW_MS * RADIO_BUS_SHARE_PERMILLE / 1000;
		let used: u64 = self.bus_time.iter().map(|&(_, held)| held).sum();
		if used < share {
			return None;
		}
		let mut left = used;
		for &(at, held) in &self.bus_time {
			left -= held;
			if left < share {
				return Some(expiry(at));
			}
		}
		self.bus_time.back().map(|&(at, _)| expiry(at))
	}

	/// The guard locked `request_id`: its subscriptions of this session end, each said.
	fn end_unit(&mut self, now_ms: u64, planner: &mut Planner, request_id: u16, out: &mut Vec<Message>) {
		let ended: Vec<u16> = self.subs.iter().filter(|(_, l)| l.request_id == request_id).map(|(s, _)| *s).collect();
		for sub in ended {
			self.unsubscribe(now_ms, planner, sub);
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
