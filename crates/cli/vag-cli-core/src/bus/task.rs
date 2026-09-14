//! The task that owns the link: the loop around `due` and `answered`.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc::{self, error::TryRecvError};
use tokio::time::Instant;
use vag_uds_can::UnitLink;
use vag_uds_client::schedule::{Answer, Budget, Delivery, Miss, Next, Planner, ReqId, SubId, Unit};
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

use super::{At, Command, ExchangeError, MAX_PENDING, OnceReply, PENDING_WAIT, READ_DEADLINE, RawReply, Sample};

/// A negative response's first byte, and the NRC that means "response pending"
/// (ISO 14229-1).
const NEGATIVE: u8 = 0x7F;
const RESPONSE_PENDING: u8 = 0x78;

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
}

/// What came back for one request, and when.
struct Heard {
	answer: Answer,
	at: Instant,
	/// The link's own error behind an [`Answer::BusError`], for a raw exchange's caller.
	error: Option<TransportError>,
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
				let deliveries = state.planner.answered(state.ms(heard.at), out.token, heard.answer);
				state.route(deliveries, heard.at, heard.error);
			}
			Next::Idle { until_ms } => {
				let wake = until_ms.map(|ms| started + Duration::from_millis(ms));
				tokio::select! {
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

async fn sleep_until(wake: Option<Instant>) {
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
			let _ = to.send(Sample { unit, did, at, value });
		}
	}
}

/// Address `unit`, put `pdu` on the link, and take the final answer back.
///
/// Not cancel-safe: dropped mid-exchange, it leaves the slot empty. The task drops it
/// only on its way out.
async fn talk<L: UnitLink>(slot: &mut Option<L>, unit: Unit, pdu: &[u8], timeout: Duration) -> Heard {
	let Some(link) = slot.take() else {
		return Heard {
			answer: Answer::BusError,
			at: Instant::now(),
			error: Some(TransportError::Disconnected),
		};
	};
	let mut channel = link.to_unit(CanId::Standard(unit.request), CanId::Standard(unit.response));
	let heard = exchange(&mut channel, pdu, timeout).await;
	*slot = Some(L::release(channel));
	heard
}

/// One request and its answer, with any number of `7F xx 78` up to [`MAX_PENDING`]
/// waited out here: the planner is never told about a pending answer.
async fn exchange<C: AsyncIsoTpTransport>(channel: &mut C, pdu: &[u8], timeout: Duration) -> Heard {
	let failed = |error: TransportError| Heard {
		answer: match error {
			TransportError::Timeout => Answer::NoAnswer,
			_ => Answer::BusError,
		},
		at: Instant::now(),
		error: Some(error),
	};
	if let Err(why) = channel.send(pdu).await {
		return failed(why);
	}
	let mut wait = timeout;
	for _ in 0..=MAX_PENDING {
		match channel.recv(wait).await {
			Ok(answer) => {
				// The arrival, before anything else is done with the answer.
				let at = Instant::now();
				if is_pending(&answer) {
					wait = PENDING_WAIT;
					continue;
				}
				return Heard {
					answer: Answer::Pdu(answer),
					at,
					error: None,
				};
			}
			Err(why) => return failed(why),
		}
	}
	Heard {
		answer: Answer::NoAnswer,
		at: Instant::now(),
		error: None,
	}
}

fn is_pending(answer: &[u8]) -> bool {
	matches!(answer, [NEGATIVE, _, RESPONSE_PENDING, ..])
}
