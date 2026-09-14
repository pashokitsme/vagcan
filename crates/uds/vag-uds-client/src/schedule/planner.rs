//! [`Planner`]: the state behind `due` and `answered`. The rules are in the module docs
//! of [`super`]; this file is how they are kept.

use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;

use super::split::{Records, split_by_lengths, split_records};
use super::{Answer, Budget, Class, Delivery, Miss, Next, Outgoing, ReqId, SubId, Token, Unit};
use crate::pdu;
use crate::uds::UdsError;

/// ReadDataByIdentifier and its positive response (ISO 14229-1).
const RDBI: u8 = 0x22;
const RDBI_POSITIVE: u8 = RDBI + 0x40;
/// A negative response's first byte (ISO 14229-1).
const NEGATIVE: u8 = 0x7F;
/// Consecutive multi-identifier answers that would not split, from a unit whose record
/// lengths are not all known, after which the unit is asked singly for good. Owner,
/// 2026-09-14.
const UNSPLITTABLE_RUN: u8 = 3;

/// How many units learned single-only are remembered after they are forgotten. Past it
/// the knowledge is dropped, not grown: such a unit is asked a multi-identifier request
/// once more and learned again, which costs one refused request.
pub const LEARNED_SINGLE_MAX: usize = 64;

/// The one scheduler every consumer of the bus goes through. See [`super`].
///
/// # What it holds
///
/// A unit's state lives while anything wants the unit: a live read, a queued exchange,
/// the request in flight, or a backoff still running. When none of those is left the
/// unit is forgotten — its identifiers, its unsplittable-answer run, its failure count —
/// so what the planner holds is bounded by what its consumers ask at once, not by how
/// many different units they have ever named. The one thing kept past that is
/// single-only knowledge, in a set of at most [`LEARNED_SINGLE_MAX`] units.
#[derive(Debug)]
pub struct Planner {
	budget: Budget,
	units: BTreeMap<Unit, UnitState>,
	/// Units that refused multi-identifier reads, remembered after they were forgotten.
	learned_single: BTreeSet<Unit>,
	/// Where each live subscription sits, for [`Planner::unsubscribe`].
	subs: BTreeMap<SubId, (Unit, u16)>,
	/// One counter for subscription, request and token ids: never reused.
	last_id: u32,
	flight: Option<Flight>,
	last_send: Option<u64>,
	/// When the foreground's sends of the last 1000 ms went out, for its floor.
	foreground_sends: VecDeque<u64>,
}

/// Everything the planner holds about one control unit.
#[derive(Debug, Default)]
struct UnitState {
	reads: BTreeMap<u16, Read>,
	raws: VecDeque<Raw>,
	/// Learned from a definite answer, never from silence: an NRC or an empty positive
	/// answer to a multi-identifier request, or [`UNSPLITTABLE_RUN`] unsplittable ones.
	single_only: bool,
	/// Multi-identifier answers in a row that would not split.
	unsplittable_run: u8,
	/// Identifiers whose last multi-identifier answer could not be split, or left them
	/// out while a one-shot waited: they go out alone next time.
	singly: BTreeSet<u16>,
	failures: u32,
	/// Not asked again before this.
	retry_at: u64,
}

/// One `(unit, identifier)`: read once for all who want it.
#[derive(Debug, Default)]
struct Read {
	subs: Vec<Sub>,
	onces: Vec<Once>,
	/// When the subscribers want the next reading; meaningless without subscribers.
	next_due: u64,
}

#[derive(Debug)]
struct Sub {
	id: SubId,
	class: Class,
	period_ms: u32,
	record_len: Option<u16>,
}

#[derive(Debug)]
struct Once {
	id: ReqId,
	class: Class,
	since_ms: u64,
}

#[derive(Debug)]
struct Raw {
	id: ReqId,
	class: Class,
	pdu: Vec<u8>,
	since_ms: u64,
}

#[derive(Debug)]
struct Flight {
	token: Token,
	unit: Unit,
	what: Flying,
}

#[derive(Debug)]
enum Flying {
	/// A `22` request: the identifiers in request order, and the one-shots it carries.
	Read {
		dids: Vec<u16>,
		onces: Vec<(u16, Vec<Once>)>,
	},
	Raw(Raw),
}

/// How a candidate for the next slot sorts, lowest first: its [`tier`], when it came
/// due (the most overdue first), then unit, raw before read, and identifier, so that ties
/// break the same way every time.
type Rank = (u8, u64, Unit, u8, u16);

/// What a candidate for the next slot is.
#[derive(Debug, Clone, Copy)]
enum Pick {
	Raw,
	Read(u16),
}

/// How an answer to a `22` request reads.
enum Heard {
	Records(Records),
	/// Positive, but not cut exactly one way (or cut into nothing); whether every
	/// requested record's length was known.
	Unsplittable {
		lengths_known: bool,
	},
	/// Positive and empty, to a multi-identifier request.
	Empty,
	Refused(u8),
	Silent(Miss),
	Malformed,
}

impl Read {
	fn period(&self) -> Option<u32> {
		self.subs.iter().map(|s| s.period_ms).min()
	}

	/// The record length every subscriber agrees on, if they all state the same one.
	fn record_len(&self) -> Option<u16> {
		let first = self.subs.first()?.record_len?;
		self.subs.iter().all(|s| s.record_len == Some(first)).then_some(first)
	}

	/// The earliest moment anything in this read wants to go out.
	fn due(&self) -> Option<u64> {
		let subs = (!self.subs.is_empty()).then_some(self.next_due);
		let onces = self.onces.iter().map(|o| o.since_ms).min();
		subs.into_iter().chain(onces).min()
	}

	/// Whether a stopwatch-grade consumer wants this read. Such a read travels alone.
	fn is_timing(&self) -> bool {
		self.subs.iter().any(|s| s.class == Class::Timing) || self.onces.iter().any(|o| o.class == Class::Timing)
	}

	/// The best class among what is due at `now`.
	fn class_due(&self, now: u64) -> Option<Class> {
		let subs = self.subs.iter().filter(|_| self.next_due <= now).map(|s| s.class);
		let onces = self.onces.iter().filter(|o| o.since_ms <= now).map(|o| o.class);
		subs.chain(onces).min()
	}

	/// Whether this read belongs in a request its unit is making at `now`: something in
	/// it is due, or its subscribers are due within the pull-forward window.
	fn wanted(&self, now: u64, pull_permille: u16) -> bool {
		if self.onces.iter().any(|o| o.since_ms <= now) {
			return true;
		}
		self.period().is_some_and(|p| self.next_due <= now + window(p, pull_permille))
	}

	fn is_empty(&self) -> bool {
		self.subs.is_empty() && self.onces.is_empty()
	}
}

fn window(period_ms: u32, permille: u16) -> u64 {
	u64::from(period_ms) * u64::from(permille) / 1000
}

impl Planner {
	pub fn new(budget: Budget) -> Self {
		Planner {
			budget,
			units: BTreeMap::new(),
			learned_single: BTreeSet::new(),
			subs: BTreeMap::new(),
			last_id: 0,
			flight: None,
			last_send: None,
			foreground_sends: VecDeque::new(),
		}
	}

	fn fresh(&mut self) -> u32 {
		self.last_id = self.last_id.wrapping_add(1);
		self.last_id
	}

	/// Deliver `did` of `unit` to a new subscriber every `period_ms`, until
	/// [`unsubscribe`](Self::unsubscribe). The first reading is due at once.
	///
	/// `record_len` is the record's length in bytes when the plan knows it; a read whose
	/// subscribers disagree on it is treated as unknown.
	pub fn subscribe(&mut self, now_ms: u64, class: Class, unit: Unit, did: u16, period_ms: u32, record_len: Option<u16>) -> SubId {
		let id = SubId(self.fresh());
		let read = self.unit_state(unit).reads.entry(did).or_default();
		read.next_due = if read.subs.is_empty() { now_ms } else { read.next_due.min(now_ms) };
		read.subs.push(Sub {
			id,
			class,
			period_ms: period_ms.max(1),
			record_len,
		});
		self.subs.insert(id, (unit, did));
		id
	}

	/// Stop a subscription. Its identifier leaves future requests once nobody else wants
	/// it; a reading already in flight is not delivered to it.
	///
	/// Cheap enough to call once per handle when a client drops all of its subscriptions
	/// at once: two map lookups and a scan of that identifier's own subscribers, no
	/// allocation, nothing recomputed for other identifiers (a shared read's period is
	/// derived from its live subscribers on use).
	pub fn unsubscribe(&mut self, id: SubId) {
		let Some((unit, did)) = self.subs.remove(&id) else {
			return;
		};
		let Some(state) = self.units.get_mut(&unit) else {
			return;
		};
		if let Some(read) = state.reads.get_mut(&did) {
			read.subs.retain(|s| s.id != id);
			if read.is_empty() {
				state.reads.remove(&did);
				state.singly.remove(&did);
			}
		}
		self.forget_if_idle(unit, None);
	}

	/// The state of `unit`, made if there is none — single-only from the start if that
	/// was learned before the unit was last forgotten.
	fn unit_state(&mut self, unit: Unit) -> &mut UnitState {
		let single_only = self.learned_single.contains(&unit);
		self.units.entry(unit).or_insert_with(|| UnitState {
			single_only,
			..UnitState::default()
		})
	}

	/// Forget `unit` if nothing wants it any more (see [`Planner`]). Without the time a
	/// backoff that was ever set keeps the unit; [`Planner::due`] forgets it once the
	/// backoff has run out.
	fn forget_if_idle(&mut self, unit: Unit, now_ms: Option<u64>) {
		let in_flight = self.flight.as_ref().is_some_and(|f| f.unit == unit);
		let Some(state) = self.units.get(&unit) else {
			return;
		};
		if in_flight || !idle(state, now_ms) {
			return;
		}
		if state.single_only && self.learned_single.len() < LEARNED_SINGLE_MAX {
			self.learned_single.insert(unit);
		}
		self.units.remove(&unit);
	}

	/// Forget every unit nothing wants at `now_ms`. Called with nothing in flight.
	fn forget_idle(&mut self, now_ms: u64) {
		let learned = &mut self.learned_single;
		self.units.retain(|unit, state| {
			if !idle(state, Some(now_ms)) {
				return true;
			}
			if state.single_only && learned.len() < LEARNED_SINGLE_MAX {
				learned.insert(*unit);
			}
			false
		});
	}

	/// Take back an exchange that is still queued, so a consumer that is gone does not
	/// reach the car. `false` for one already in flight, answered, or unknown: what is on
	/// the bus cannot be recalled, and its answer is delivered as usual.
	pub fn cancel(&mut self, req: ReqId) -> bool {
		let found = self
			.units
			.iter()
			.find_map(|(unit, state)| state.raws.iter().position(|raw| raw.id == req).map(|at| (*unit, at)));
		let Some((unit, at)) = found else {
			return false;
		};
		if let Some(state) = self.units.get_mut(&unit) {
			state.raws.remove(at);
		}
		self.forget_if_idle(unit, None);
		true
	}

	/// How many units the planner holds state for.
	#[cfg(test)]
	pub(crate) fn units_held(&self) -> usize {
		self.units.len()
	}

	/// How many live [`Class::Timing`] subscriptions the planner holds, from every
	/// consumer. The board's sessions share one planner and read this to keep one
	/// stopwatch at a time (`remote::Session`).
	pub fn timing_subscriptions(&self) -> usize {
		self
			.units
			.values()
			.flat_map(|state| state.reads.values())
			.flat_map(|read| &read.subs)
			.filter(|sub| sub.class == Class::Timing)
			.count()
	}

	/// Read `did` of `unit` once; the result comes as exactly one [`Delivery::Once`].
	pub fn read_once(&mut self, now_ms: u64, class: Class, unit: Unit, did: u16) -> ReqId {
		let id = ReqId(self.fresh());
		let read = self.unit_state(unit).reads.entry(did).or_default();
		read.onces.push(Once { id, class, since_ms: now_ms });
		id
	}

	/// Send `pdu` to `unit` alone and deliver the answer as it comes, as exactly one
	/// [`Delivery::Raw`]. Refused at the door, and never queued, when its service is
	/// outside the read-only allowlist.
	pub fn exchange(&mut self, now_ms: u64, class: Class, unit: Unit, pdu: Vec<u8>) -> Result<ReqId, UdsError> {
		let (&sid, rest) = pdu.split_first().ok_or_else(|| UdsError::Malformed(String::from("empty request")))?;
		let pdu = pdu::encode_request(sid, rest)?;
		let id = ReqId(self.fresh());
		self.unit_state(unit).raws.push_back(Raw {
			id,
			class,
			pdu,
			since_ms: now_ms,
		});
		Ok(id)
	}

	/// The next request to put on the bus, or when to ask again.
	pub fn due(&mut self, now_ms: u64) -> Next {
		if self.flight.is_some() {
			return Next::Idle { until_ms: None };
		}
		self.forget_idle(now_ms);
		let now = now_ms;
		while self.foreground_sends.front().is_some_and(|t| t + 1000 <= now) {
			self.foreground_sends.pop_front();
		}
		let under_floor = self.foreground_sends.len() < usize::from(self.budget.foreground_floor_per_s);

		let mut best: Option<(Rank, Class, Pick)> = None;
		let mut earliest: Option<u64> = None;
		for (unit, state) in &self.units {
			let raw = state.raws.front().map(|r| (r.since_ms, Some(r.class), Pick::Raw));
			let reads = state
				.reads
				.iter()
				.filter_map(|(did, read)| Some((read.due()?, read.class_due(now), Pick::Read(*did))));
			for (due, class, pick) in raw.into_iter().chain(reads) {
				let ready = due.max(state.retry_at);
				let Some(class) = class.filter(|_| ready <= now) else {
					earliest = Some(earliest.map_or(ready, |e| e.min(ready)));
					continue;
				};
				let (kind, did) = match pick {
					Pick::Raw => (0, 0),
					Pick::Read(did) => (1, did),
				};
				let starved = now.saturating_sub(due) > u64::from(self.budget.starve_after_ms);
				let rank = (
					tier(class, under_floor, starved, self.budget.timing_yields_to_floor),
					due,
					*unit,
					kind,
					did,
				);
				if best.as_ref().is_none_or(|(held, _, _)| rank < *held) {
					best = Some((rank, class, pick));
				}
			}
		}

		let slot = self.last_send.map_or(0, |t| t + spacing(self.budget.ceiling_per_s));
		let Some(((_, _, unit, _, _), class, pick)) = best else {
			return Next::Idle {
				until_ms: earliest.map(|e| e.max(slot)),
			};
		};
		if slot > now {
			return Next::Idle { until_ms: Some(slot) };
		}

		let token = Token(self.fresh());
		self.last_send = Some(now);
		if class == Class::Foreground {
			self.foreground_sends.push_back(now);
		}
		let budget = self.budget;
		let state = self.units.get_mut(&unit).expect("the candidate's unit is held");
		let (pdu, what) = match pick {
			Pick::Raw => {
				let raw = state.raws.pop_front().expect("the candidate raw is queued");
				(raw.pdu.clone(), Flying::Raw(raw))
			}
			Pick::Read(trigger) => {
				let dids = batch(state, trigger, now, &budget);
				let mut pdu = Vec::with_capacity(1 + 2 * dids.len());
				pdu.push(RDBI);
				let mut onces = Vec::new();
				for did in &dids {
					pdu.extend_from_slice(&pdu::did_bytes(*did));
					let read = state.reads.get_mut(did).expect("a batched identifier is held");
					onces.push((*did, core::mem::take(&mut read.onces)));
					match read.period() {
						Some(period) if read.next_due <= now + window(period, budget.pull_forward_permille) => {
							// Keep the phase; a read more than a period late starts a new one.
							let next = read.next_due + u64::from(period);
							read.next_due = if next <= now { now + u64::from(period) } else { next };
						}
						_ => {}
					}
				}
				if dids.len() == 1 {
					state.singly.remove(&dids[0]);
				}
				(pdu, Flying::Read { dids, onces })
			}
		};
		self.flight = Some(Flight { token, unit, what });
		Next::Send(Outgoing { token, unit, pdu })
	}

	/// The raw exchange in flight, if the request [`due`](Self::due) last sent is one.
	///
	/// A read and a raw `22` look alike on the wire, and a shell that lets a consumer
	/// choose how long its own exchange may wait (a presence probe waits far less
	/// than a read) needs to know which of the two it is waiting for.
	pub fn flying_raw(&self) -> Option<ReqId> {
		match &self.flight.as_ref()?.what {
			Flying::Raw(raw) => Some(raw.id),
			Flying::Read { .. } => None,
		}
	}

	/// Take the answer to the request in flight; `now_ms` is the moment it arrived.
	/// An answer for any other token is ignored and returns nothing.
	pub fn answered(&mut self, now_ms: u64, token: Token, answer: Answer) -> Vec<Delivery> {
		let Some(flight) = self.flight.take_if(|f| f.token == token) else {
			return Vec::new();
		};
		let mut out = Vec::new();
		let unit = flight.unit;
		match flight.what {
			Flying::Raw(raw) => {
				match answer {
					Answer::NoAnswer => self.back_off(now_ms, unit, Miss::NoAnswer, &mut out),
					Answer::BusError => self.back_off(now_ms, unit, Miss::BusError, &mut out),
					Answer::Pdu(_) | Answer::Refused(_) => self.heard_from(unit),
					// The silence the request asked for says nothing about the unit either way.
					Answer::NotExpected => {}
				}
				out.push(Delivery::Raw {
					req: raw.id,
					unit,
					answer,
					at_ms: now_ms,
				});
			}
			Flying::Read { dids, onces } => self.answered_read(now_ms, unit, dids, onces, answer, &mut out),
		}
		self.forget_if_idle(unit, Some(now_ms));
		out
	}

	fn answered_read(&mut self, now: u64, unit: Unit, dids: Vec<u16>, onces: Vec<(u16, Vec<Once>)>, answer: Answer, out: &mut Vec<Delivery>) {
		let multi = dids.len() > 1;
		let heard = self.read_answer(unit, &dids, answer);
		match heard {
			Heard::Records(records) => {
				self.heard_from(unit);
				let state = self.units.get_mut(&unit).expect("a unit in flight is held");
				if multi {
					state.unsplittable_run = 0;
				}
				for (did, waiting) in onces {
					let data = records.iter().find(|(d, _)| *d == did).map(|(_, data)| data);
					// Held even if every subscriber left in flight: a one-shot may be waiting.
					// Emptied reads are dropped below.
					let read = state.reads.entry(did).or_default();
					match data {
						Some(data) => {
							for sub in &read.subs {
								out.push(Delivery::Reading {
									sub: sub.id,
									unit,
									did,
									data: data.clone(),
									at_ms: now,
								});
							}
							for once in waiting {
								out.push(Delivery::Once {
									req: once.id,
									unit,
									did,
									result: Ok(data.clone()),
									at_ms: now,
								});
							}
						}
						None => {
							for sub in &read.subs {
								out.push(missed(sub, unit, did, Miss::Absent, now));
							}
							// A one-shot gets a definite answer: asked alone, the unit
							// either reads the identifier or refuses it.
							if !waiting.is_empty() {
								state.singly.insert(did);
								read.onces.extend(waiting);
							}
						}
					}
				}
			}
			Heard::Refused(nrc) if !multi => {
				self.heard_from(unit);
				let state = self.units.get_mut(&unit).expect("a unit in flight is held");
				for (did, waiting) in onces {
					if let Some(read) = state.reads.get(&did) {
						for sub in &read.subs {
							out.push(missed(sub, unit, did, Miss::Refused(nrc), now));
						}
					}
					for once in waiting {
						out.push(Delivery::Once {
							req: once.id,
							unit,
							did,
							result: Err(Miss::Refused(nrc)),
							at_ms: now,
						});
					}
				}
			}
			Heard::Refused(_) => {
				self.heard_from(unit);
				let state = self.units.get_mut(&unit).expect("a unit in flight is held");
				state.single_only = true;
				retry(state, now, onces);
			}
			Heard::Empty => {
				self.heard_from(unit);
				let state = self.units.get_mut(&unit).expect("a unit in flight is held");
				state.single_only = true;
				retry(state, now, onces);
			}
			Heard::Unsplittable { lengths_known } => {
				self.heard_from(unit);
				let state = self.units.get_mut(&unit).expect("a unit in flight is held");
				if !lengths_known {
					state.unsplittable_run = state.unsplittable_run.saturating_add(1);
					state.single_only |= state.unsplittable_run >= UNSPLITTABLE_RUN;
				}
				state.singly.extend(dids.iter().copied());
				retry(state, now, onces);
			}
			// No answer is an absent unit, never evidence against batching.
			Heard::Silent(why) => {
				self.back_off(now, unit, why, out);
				fail_onces(onces, unit, why, now, out);
			}
			Heard::Malformed => {
				self.heard_from(unit);
				let state = &self.units[&unit];
				for did in &dids {
					if let Some(read) = state.reads.get(did) {
						for sub in &read.subs {
							out.push(missed(sub, unit, *did, Miss::Malformed, now));
						}
					}
				}
				fail_onces(onces, unit, Miss::Malformed, now, out);
			}
		}
		let state = self.units.get_mut(&unit).expect("a unit in flight is held");
		state.reads.retain(|_, read| !read.is_empty());
	}

	/// Classify a `22` answer and cut it into records.
	fn read_answer(&self, unit: Unit, dids: &[u16], answer: Answer) -> Heard {
		let pdu = match answer {
			Answer::NoAnswer => return Heard::Silent(Miss::NoAnswer),
			Answer::BusError => return Heard::Silent(Miss::BusError),
			Answer::Refused(nrc) => return Heard::Refused(nrc),
			// A read always expects an answer; a shell that says otherwise is not answering it.
			Answer::NotExpected => return Heard::Malformed,
			Answer::Pdu(pdu) => pdu,
		};
		match pdu.as_slice() {
			// `78` included: the transport should have waited it out (module docs).
			[NEGATIVE, RDBI, nrc, ..] => Heard::Refused(*nrc),
			[RDBI_POSITIVE, payload @ ..] => match dids {
				[did] => match payload.split_first_chunk::<2>() {
					Some((echo, data)) if *echo == pdu::did_bytes(*did) => Heard::Records(alloc::vec![(*did, data.to_vec())]),
					_ => Heard::Malformed,
				},
				_ if payload.is_empty() => Heard::Empty,
				_ => {
					let reads = &self.units[&unit].reads;
					let lengths: Option<Vec<(u16, u16)>> = dids.iter().map(|d| Some((*d, reads.get(d)?.record_len()?))).collect();
					let lengths_known = lengths.is_some();
					let records = lengths
						.and_then(|l| split_by_lengths(payload, &l))
						.or_else(|| split_records(payload, dids))
						.filter(|r| !r.is_empty());
					records.map_or(Heard::Unsplittable { lengths_known }, Heard::Records)
				}
			},
			_ => Heard::Malformed,
		}
	}

	/// The unit answered something: it is alive.
	fn heard_from(&mut self, unit: Unit) {
		if let Some(state) = self.units.get_mut(&unit) {
			state.failures = 0;
			state.retry_at = 0;
		}
	}

	/// The unit did not answer: wait longer before the next attempt, and tell every
	/// subscriber of it that this reading is not coming.
	fn back_off(&mut self, now: u64, unit: Unit, why: Miss, out: &mut Vec<Delivery>) {
		let budget = self.budget;
		let Some(state) = self.units.get_mut(&unit) else {
			return;
		};
		state.failures = state.failures.saturating_add(1);
		let doubling = 1u64 << (state.failures - 1).min(32);
		let wait = u64::from(budget.backoff_first_ms)
			.saturating_mul(doubling)
			.min(u64::from(budget.backoff_cap_ms));
		state.retry_at = now + wait;
		for (did, read) in &state.reads {
			for sub in &read.subs {
				out.push(missed(sub, unit, *did, why, now));
			}
		}
	}
}

/// Pick the identifiers of one request: the trigger, then every other wanted identifier
/// of its unit by how overdue it is, up to the cap — or the trigger alone when the unit
/// is single-only or the trigger must go singly.
///
/// Timing reads never share a request with anything else: a Timing request carries only
/// Timing identifiers (normally one), so the speed answer stays short, and no other
/// request pulls a Timing identifier in and lengthens its answer.
fn batch(state: &UnitState, trigger: u16, now: u64, budget: &Budget) -> Vec<u16> {
	let mut dids = alloc::vec![trigger];
	if state.single_only || state.singly.contains(&trigger) {
		return dids;
	}
	let timing = state.reads.get(&trigger).is_some_and(Read::is_timing);
	let mut others: Vec<(u64, u16)> = state
		.reads
		.iter()
		.filter(|(did, read)| {
			**did != trigger && !state.singly.contains(did) && read.is_timing() == timing && read.wanted(now, budget.pull_forward_permille)
		})
		.filter_map(|(did, read)| Some((read.due()?, *did)))
		.collect();
	others.sort_unstable();
	let room = usize::from(budget.max_dids_per_request.max(1)) - 1;
	dids.extend(others.into_iter().take(room).map(|(_, did)| did));
	dids
}

/// Whether nothing wants a unit: no live read, nothing queued, and no backoff still
/// running — with the time unknown, no backoff ever set.
fn idle(state: &UnitState, now_ms: Option<u64>) -> bool {
	let backoff_over = match now_ms {
		Some(now) => state.retry_at <= now,
		None => state.retry_at == 0,
	};
	state.reads.is_empty() && state.raws.is_empty() && backoff_over
}

/// A batch that will be asked again at once, differently: its one-shots wait on, its
/// subscribers are due now, and nobody is told of a miss.
fn retry(state: &mut UnitState, now: u64, onces: Vec<(u16, Vec<Once>)>) {
	for (did, waiting) in onces {
		let read = state.reads.entry(did).or_default();
		read.onces.extend(waiting);
		if !read.subs.is_empty() {
			read.next_due = read.next_due.min(now);
		}
	}
}

fn fail_onces(onces: Vec<(u16, Vec<Once>)>, unit: Unit, why: Miss, now: u64, out: &mut Vec<Delivery>) {
	for (did, waiting) in onces {
		for once in waiting {
			out.push(Delivery::Once {
				req: once.id,
				unit,
				did,
				result: Err(why),
				at_ms: now,
			});
		}
	}
}

fn missed(sub: &Sub, unit: Unit, did: u16, why: Miss, now: u64) -> Delivery {
	Delivery::Missed {
		sub: sub.id,
		unit,
		did,
		why,
		at_ms: now,
	}
}

/// Precedence of a candidate: lower goes first. See the module docs of [`super`].
///
/// `timing_yields_to_floor` ([`Budget::timing_yields_to_floor`]) moves the foreground under
/// its floor ahead of timing; everything else keeps its order.
fn tier(class: Class, foreground_under_floor: bool, starved: bool, timing_yields_to_floor: bool) -> u8 {
	match class {
		Class::Foreground if foreground_under_floor && timing_yields_to_floor => 0,
		Class::Timing => 1,
		Class::Remote | Class::Background if starved => 2,
		Class::Foreground if foreground_under_floor => 3,
		Class::Remote => 4,
		Class::Foreground => 5,
		Class::Background => 6,
	}
}

/// The least time between two sends that keeps any 1000 ms under `ceiling_per_s`.
fn spacing(ceiling_per_s: u16) -> u64 {
	1000u64.div_ceil(u64::from(ceiling_per_s.max(1)))
}
