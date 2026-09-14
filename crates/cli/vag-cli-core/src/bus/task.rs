//! The task that owns the link: the loop around `due` and `answered`.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc::{self, error::TryRecvError};
use tokio::time::Instant;
use vag_uds_can::UnitLink;
use vag_uds_client::schedule::{Answer, Budget, Delivery, Miss, Next, Planner, ReqId, SubId, Unit, expects_no_answer};
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

use super::{At, Command, ExchangeError, MAX_PENDING, OnceReply, PENDING_WAIT, READ_DEADLINE, RawReply, SUPPRESSED_WAIT, Sample};

/// A negative response's first byte, and the NRC that means "response pending"
/// (ISO 14229-1).
const NEGATIVE: u8 = 0x7F;
const RESPONSE_PENDING: u8 = 0x78;
/// What a positive response adds to the request's service id (ISO 14229-1).
const POSITIVE_OFFSET: u8 = 0x40;
/// The services on the read-only allowlist whose answers echo something of the
/// request (ISO 14229-1): ReadDataByIdentifier echoes the identifier, and
/// DiagnosticSessionControl, ReadDTCInformation and TesterPresent their sub-function.
const RDBI: u8 = 0x22;
const SESSION: u8 = 0x10;
const DTC: u8 = 0x19;
const TESTER_PRESENT: u8 = 0x3E;
/// The sub-function bit that asks the server to suppress its positive response; the
/// response echoes the sub-function without it (ISO 14229-1).
const SUPPRESS_POSITIVE: u8 = 0x80;

/// A raw exchange somebody is waiting on.
struct Waiting {
	timeout: Duration,
	to: RawReply,
}

/// Everything the task holds besides the link.
struct State {
	planner: Planner,
	started: Instant,
	subs: HashMap<SubId, mpsc::UnboundedSender<Sample>>,
	/// A handle's own key for its subscription, to the planner's id.
	keys: HashMap<u64, SubId>,
	onces: HashMap<ReqId, OnceReply>,
	raws: HashMap<ReqId, Waiting>,
	/// Answers the link handed over that did not answer the request out at the time —
	/// late answers to earlier ones — over the life of the task.
	discarded: usize,
}

impl Drop for State {
	/// Said once, when the bus closes, and only when there were any: in the middle of a
	/// run this would land on top of whatever the screen is drawing.
	fn drop(&mut self) {
		if let Some(note) = late_note(self.discarded) {
			eprintln!("{note}");
		}
	}
}

/// What the closing note says about `discarded` late answers: nothing for none.
pub(super) fn late_note(discarded: usize) -> Option<String> {
	match discarded {
		0 => None,
		1 => Some("1 late answer from a control unit ignored".to_string()),
		n => Some(format!("{n} late answers from control units ignored")),
	}
}

/// What came back for one request, and when.
struct Heard {
	answer: Answer,
	at: Instant,
	/// The link's own error behind an [`Answer::BusError`], for a raw exchange's caller.
	error: Option<TransportError>,
	/// Answers received and not taken, because they answered some other request.
	discarded: usize,
}

/// Run until every handle is gone or one asks for a shutdown. Returning drops the link.
pub(super) async fn run<L: UnitLink>(link: L, budget: Budget, mut inbox: mpsc::UnboundedReceiver<Command>, started: Instant) {
	let mut state = State {
		planner: Planner::new(budget),
		started,
		subs: HashMap::new(),
		keys: HashMap::new(),
		onces: HashMap::new(),
		raws: HashMap::new(),
		discarded: 0,
	};
	let mut link = Some(link);
	loop {
		// Everything already asked goes into the planner before the next slot is filled.
		loop {
			match inbox.try_recv() {
				Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => return,
				Ok(command) => state.apply(command),
				Err(TryRecvError::Empty) => break,
			}
		}
		match state.planner.due(state.ms(Instant::now())) {
			Next::Send(out) => {
				let timeout = state
					.planner
					.flying_raw()
					.and_then(|req| state.raws.get(&req))
					.map_or(READ_DEADLINE, |w| w.timeout);
				let heard = {
					let exchange = talk(&mut link, out.unit, &out.pdu, timeout);
					tokio::pin!(exchange);
					loop {
						tokio::select! {
							biased;
							// Taken in while the exchange is out, so a handle that goes
							// away mid-exchange stops the task at once rather than after.
							command = inbox.recv() => match command {
								None | Some(Command::Shutdown) => return,
								Some(command) => state.apply(command),
							},
							heard = &mut exchange => break heard,
						}
					}
				};
				state.discarded += heard.discarded;
				let deliveries = state.planner.answered(state.ms(heard.at), out.token, heard.answer);
				state.route(deliveries, heard.at, heard.error);
			}
			Next::Idle { until_ms } => {
				let wake = until_ms.map(|ms| started + Duration::from_millis(ms));
				tokio::select! {
					biased;
					// A command first, as in the exchange above: a slot that comes due
					// at the same moment waits one pass, a handle that went away does not.
					command = inbox.recv() => match command {
						None | Some(Command::Shutdown) => return,
						Some(command) => state.apply(command),
					},
					() = sleep_until(wake) => {}
				}
			}
		}
	}
}

/// Wake at `wake`, or never. Shared with the remote task.
pub(super) async fn sleep_until(wake: Option<Instant>) {
	match wake {
		Some(at) => tokio::time::sleep_until(at).await,
		None => std::future::pending().await,
	}
}

impl State {
	fn ms(&self, at: Instant) -> u64 {
		u64::try_from(at.saturating_duration_since(self.started).as_millis()).unwrap_or(u64::MAX)
	}

	fn at(&self, at: Instant) -> At {
		At {
			ms: self.ms(at),
			secs: at.saturating_duration_since(self.started).as_secs_f64(),
		}
	}

	fn apply(&mut self, command: Command) {
		let now = self.ms(Instant::now());
		match command {
			Command::Subscribe {
				key,
				class,
				unit,
				did,
				period_ms,
				record_len,
				to,
			} => {
				let id = self.planner.subscribe(now, class, unit, did, period_ms, record_len);
				self.subs.insert(id, to);
				self.keys.insert(key, id);
			}
			Command::Unsubscribe { key } => {
				if let Some(id) = self.keys.remove(&key) {
					self.planner.unsubscribe(id);
					self.subs.remove(&id);
				}
			}
			Command::ReadOnce { class, unit, did, to } => {
				let id = self.planner.read_once(now, class, unit, did);
				self.onces.insert(id, to);
			}
			Command::Exchange {
				class,
				unit,
				pdu,
				timeout,
				to,
			} => match self.planner.exchange(now, class, unit, pdu) {
				Ok(id) => {
					self.raws.insert(id, Waiting { timeout, to });
				}
				Err(why) => {
					let _ = to.send(Err(ExchangeError::Forbidden(why)));
				}
			},
			// Handled by the loop, which is the only thing that can stop.
			Command::Shutdown => {}
		}
	}

	/// Hand every delivery to whoever is waiting for it. A consumer that has gone is
	/// skipped: its unsubscribe is already on its way.
	fn route(&mut self, deliveries: Vec<Delivery>, arrived: Instant, mut error: Option<TransportError>) {
		let at = self.at(arrived);
		for delivery in deliveries {
			match delivery {
				Delivery::Reading { sub, unit, did, data, .. } => self.to_subscriber(sub, unit, did, at, Ok(data)),
				Delivery::Missed { sub, unit, did, why, .. } => self.to_subscriber(sub, unit, did, at, Err(why)),
				Delivery::Once { req, result, .. } => {
					if let Some(to) = self.onces.remove(&req) {
						let _ = to.send(result.map(|data| (data, at)));
					}
				}
				Delivery::Raw { req, unit, answer, .. } => {
					let Some(waiting) = self.raws.remove(&req) else { continue };
					let result = match answer {
						Answer::Pdu(pdu) => Ok((pdu, at)),
						// The shell hands every answer over as a PDU, so this is never
						// produced here; kept total so a refusal is still a refusal.
						Answer::Refused(nrc) => Ok((vec![NEGATIVE, 0, nrc], at)),
						// Silence after a request that suppressed its positive response is
						// what was asked for: a success with nothing in it, as the remote
						// bus reports it too.
						Answer::NotExpected => Ok((Vec::new(), at)),
						Answer::NoAnswer => Err(ExchangeError::NoAnswer),
						Answer::BusError => {
							Err(ExchangeError::Link(error.take().unwrap_or_else(|| {
								TransportError::Io(format!("the link failed talking to {:03X}", unit.request))
							})))
						}
					};
					let _ = waiting.to.send(result);
				}
			}
		}
	}

	fn to_subscriber(&self, sub: SubId, unit: Unit, did: u16, at: At, value: Result<Vec<u8>, Miss>) {
		if let Some(to) = self.subs.get(&sub) {
			let _ = to.send(Sample {
				unit,
				did,
				at,
				value,
				ended: None,
			});
		}
	}
}

/// Address `unit`, put `pdu` on the link, and take the final answer back.
///
/// What the link already holds is thrown away first ([`UnitLink::discard_stale`]): a late
/// answer to an identical request echoes this one's identifier, and [`answers`] cannot
/// tell it from the real one.
///
/// Not cancel-safe: dropped mid-exchange, it leaves the slot empty. The task drops it
/// only on its way out.
async fn talk<L: UnitLink>(slot: &mut Option<L>, unit: Unit, pdu: &[u8], timeout: Duration) -> Heard {
	let Some(mut link) = slot.take() else {
		return Heard {
			answer: Answer::BusError,
			at: Instant::now(),
			error: Some(TransportError::Disconnected),
			discarded: 0,
		};
	};
	link.discard_stale().await;
	let mut channel = link.to_unit(CanId::Standard(unit.request), CanId::Standard(unit.response));
	let heard = exchange(&mut channel, pdu, timeout).await;
	*slot = Some(L::release(channel));
	heard
}

/// One request and its answer.
///
/// Any number of `7F xx 78` up to [`MAX_PENDING`] is waited out here: the planner is
/// never told about a pending answer. An answer that does not answer *this* request
/// ([`answers`]) — one that arrived after an earlier request stopped waiting for it —
/// is discarded, and the wait goes on for what is left of the deadline, not a new one.
async fn exchange<C: AsyncIsoTpTransport>(channel: &mut C, pdu: &[u8], timeout: Duration) -> Heard {
	let mut discarded = 0;
	// A request that suppressed its positive response is answered only by a refusal, so
	// it waits [`SUPPRESSED_WAIT`] for one rather than the caller's whole deadline, and
	// silence is what it asked for. Once the unit says `78` it owes a final answer after
	// all (ISO 14229-1), and silence from then on is no answer.
	let mut none_expected = expects_no_answer(pdu);
	if let Err(why) = channel.send(pdu).await {
		return failed(why, discarded, none_expected);
	}
	// The first wait is the whole deadline, and so is the first after a `78`; only a
	// discarded answer leaves less of it.
	let first = if none_expected { SUPPRESSED_WAIT } else { timeout };
	let mut deadline = Instant::now() + first;
	let mut wait = first;
	let mut pending = 0;
	loop {
		if wait.is_zero() {
			return failed(TransportError::Timeout, discarded, none_expected);
		}
		let answer = match channel.recv(wait).await {
			Ok(answer) => answer,
			Err(why) => return failed(why, discarded, none_expected),
		};
		// The arrival, before anything else is done with the answer.
		let at = Instant::now();
		if !answers(pdu, &answer) {
			discarded += 1;
			wait = deadline.saturating_duration_since(at);
			continue;
		}
		if is_pending(&answer) {
			none_expected = false;
			pending += 1;
			if pending > MAX_PENDING {
				return failed(TransportError::Timeout, discarded, none_expected);
			}
			deadline = at + PENDING_WAIT;
			wait = PENDING_WAIT;
			continue;
		}
		return Heard {
			answer: Answer::Pdu(answer),
			at,
			error: None,
			discarded,
		};
	}
}

/// A request that got no answer: [`Answer::NotExpected`] when it asked for none and none
/// came, otherwise no answer or a link failure.
fn failed(error: TransportError, discarded: usize, none_expected: bool) -> Heard {
	Heard {
		answer: match error {
			TransportError::Timeout if none_expected => Answer::NotExpected,
			TransportError::Timeout => Answer::NoAnswer,
			_ => Answer::BusError,
		},
		at: Instant::now(),
		error: Some(error),
		discarded,
	}
}

/// Whether `response` answers `request`.
///
/// A link can hand over an answer that arrived after its own request stopped waiting
/// for it, and taking it for the next request would put one identifier's bytes under
/// another's name. So, by ISO 14229-1: a positive response's service id is the
/// request's plus `0x40`, and a negative one is `7F <request's service id> <NRC>`; a
/// `22` answer starts with the first identifier asked; `10`, `19` and `3E` echo their
/// sub-function, without the suppress-positive-response bit.
pub(super) fn answers(request: &[u8], response: &[u8]) -> bool {
	let Some(&sid) = request.first() else {
		return false;
	};
	match response {
		[NEGATIVE, echoed, _, ..] => *echoed == sid,
		[NEGATIVE, ..] => false,
		[positive, rest @ ..] if *positive == sid.wrapping_add(POSITIVE_OFFSET) => match sid {
			RDBI => request.get(1..3).is_some_and(|did| rest.get(..2) == Some(did)),
			SESSION | DTC | TESTER_PRESENT => request.get(1).is_none_or(|sub| rest.first() == Some(&(sub & !SUPPRESS_POSITIVE))),
			_ => true,
		},
		_ => false,
	}
}

fn is_pending(answer: &[u8]) -> bool {
	matches!(answer, [NEGATIVE, _, RESPONSE_PENDING, ..])
}
