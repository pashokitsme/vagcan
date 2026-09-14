//! [`Bus::start_remote`](super::Bus::start_remote): the same [`Bus`](super::Bus) and
//! [`Subscription`](super::Subscription) handles over a byte pipe to the dash board.
//!
//! The board runs the planner and the guard (`todo/dash/16-uds-over-ble.md`), so this
//! task plans nothing: it turns each command into a link message
//! ([`vag_uds_transport::link`]) and each message back into what a handle waits for.
//!
//! | a handle asks                              | on the pipe                                 | back                     |
//! |--------------------------------------------|---------------------------------------------|--------------------------|
//! | [`subscribe`](super::Bus::subscribe)       | `Subscribe { sub, ids, did, period_ms }`     | a `Reading` per poll      |
//! | dropping the subscription                  | `Unsubscribe { sub }`                        | —                        |
//! | [`read_once`](super::Bus::read_once)       | `Request` of `22 did`                        | its `Answer`, read as a `Reading` is |
//! | [`exchange`](super::Bus::exchange)         | `Request` of the whole PDU                   | its `Answer`             |
//!
//! - **Requests go out one at a time**, in the order asked, each with its own `seq`; the
//!   board takes them one at a time anyway. An answer whose `seq` is not the one out is
//!   late for a request already given up on, and dropped.
//! - **The pipe is read first.** What it already holds is taken in before the commands
//!   waiting, and again after every write — a write over BLE takes tens of milliseconds,
//!   and a board streaming readings does not wait for the host to finish writing.
//! - **[`Class`](super::Class) and [`Budget`](super::Budget) are not sent.** The link has
//!   no field for either: the board classes all remote work itself, as its own class.
//! - **A period under [`MIN_PERIOD_MS`] is raised to it**, the guard's floor, rather than
//!   sent to be refused.
//! - **At most [`MAX_SUBSCRIPTIONS`] live**, the guard's cap: the next one is refused
//!   here, with that reason, rather than sent to be refused.
//! - **A refusal by the board ends a subscription** — on the board it has already ended
//!   (`vag_uds_client::remote`). The subscriber gets one [`Miss::BusError`] and then the
//!   end of its stream. The planner's [`Miss`] has no variant for a refusal by the link,
//!   and it is shared with the board, whose session matches it exhaustively. An
//!   exchange's refusal is [`ExchangeError::Refused`], carrying the board's words.
//! - **What is to be said is said when the bus closes**, not when it happens: a line on
//!   stderr in the middle of a run lands on top of `watch`'s or `measure`'s full screen
//!   and stays there. The cable task keeps its own note the same way.
//! - **The board's text** — `dashcfg`'s state line over BLE, the image's log lines over
//!   USB — shares the pipe and is ignored. So is a HelloReply: the Hello was asked by
//!   whoever opened the pipe, before the bus started.
//! - **When the link breaks** — the pipe closes, a write fails, or a frame does not
//!   reassemble, which on a link that guarantees delivery means a chunk was lost — every
//!   subscription gets a last [`Miss::BusError`] and ends, whatever is waiting fails,
//!   every later command fails with the reason, and [`Bus::closed`](super::Bus::closed)
//!   says it, so a consumer can show why its streams ended.
//!
//! # Time
//!
//! A `Reading` carries `at_ms`, the board's clock when the unit's answer reached the
//! board. It is put on the bus clock with one offset, fixed at the first reading of a
//! live subscription that is an answer or a no-answer:
//! `secs = host_secs_at_first + (at_ms − at_ms_first) / 1000`. A refusal is stamped with
//! the board's own now rather than an answer's arrival, so it does not fix the offset.
//! Differences between readings are the board's, taken where the bus is, and those are
//! what `measure` times a run from; the radio's latency moves only the offset. The offset
//! is never corrected, so a reading can be stamped a few milliseconds past
//! [`Bus::secs`](super::Bus::secs) when the first one came through the radio slower than
//! a later one. An `Answer` carries no board time: a one-shot read or an exchange is
//! stamped when it arrived here.
//!
//! # Deadlines
//!
//! A request waits its caller's deadline plus [`REMOTE_GRACE`]; see there.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::task::Poll;
use std::time::Duration;

use tokio::sync::mpsc::{self, error::TryRecvError};
use tokio::time::Instant;
use vag_uds_client::guard::{MAX_SUBSCRIPTIONS, MIN_PERIOD_MS};
use vag_uds_transport::TransportError;
use vag_uds_transport::link::{self, LinkError, Message, Outcome, Piece, Pipe, Reassembler};

use super::task::{answers, sleep_until};
use super::{At, Carrier, Command, ExchangeError, Miss, OnceReply, READ_DEADLINE, REMOTE_GRACE, RawReply, Sample, Unit};

/// ReadDataByIdentifier, its positive answer, and a negative answer's first byte (ISO 14229-1).
const READ: u8 = 0x22;
const READ_POSITIVE: u8 = 0x62;
const NEGATIVE: u8 = 0x7F;

/// A subscription the board holds for this host.
struct Sub {
	unit: Unit,
	did: u16,
	to: mpsc::UnboundedSender<Sample>,
}

/// A request not yet on the pipe, or out on it.
enum Ask {
	Once {
		unit: Unit,
		did: u16,
		to: OnceReply,
	},
	Raw {
		unit: Unit,
		pdu: Vec<u8>,
		timeout: Duration,
		to: RawReply,
	},
}

/// The request out on the pipe.
struct Flying {
	seq: u8,
	deadline: Instant,
	ask: Ask,
}

/// What woke the task.
enum Event {
	Chunk(Option<Vec<u8>>),
	Command(Option<Command>),
	Deadline,
}

/// Everything the task holds besides the pipe.
struct Remote {
	/// The board's name, for what is said when something goes wrong.
	peer: String,
	/// What the pipe runs on, for the same.
	carrier: Carrier,
	started: Instant,
	/// Live subscriptions by the id on the wire.
	subs: HashMap<u16, Sub>,
	/// A handle's own key for its subscription, to the id on the wire.
	wire: HashMap<u64, u16>,
	/// The wire id tried next. Counted up, so an id just freed is not handed out again
	/// while readings for it may still be on their way.
	next_sub: u16,
	asks: VecDeque<Ask>,
	flying: Option<Flying>,
	next_seq: u8,
	/// The anchoring reading's board time and its arrival on the bus clock.
	clock: Option<(u32, f64)>,
	/// Frames to write before waiting again.
	outbox: Vec<Vec<u8>>,
	/// What to tell the person when the bus closes, each line once.
	notes: Vec<String>,
}

impl Drop for Remote {
	/// Said once, when the bus closes: in the middle of a run it would land on top of
	/// whatever the screen is drawing.
	fn drop(&mut self) {
		for line in &self.notes {
			eprintln!("{line}");
		}
	}
}

/// Run until every handle is gone or one asks for a shutdown. Returning drops the pipe.
///
/// When the link breaks, its reason goes into `closed` for [`Bus::closed`](super::Bus::closed).
pub(super) async fn run<P: Pipe>(
	mut pipe: P,
	peer: String,
	carrier: Carrier,
	mut inbox: mpsc::UnboundedReceiver<Command>,
	started: Instant,
	closed: Arc<OnceLock<String>>,
) {
	let mut state = Remote {
		peer,
		carrier,
		started,
		subs: HashMap::new(),
		wire: HashMap::new(),
		next_sub: 0,
		asks: VecDeque::new(),
		flying: None,
		next_seq: 0,
		clock: None,
		outbox: Vec::new(),
		notes: Vec::new(),
	};
	let mut reassembler = Reassembler::new();
	let broken = 'live: loop {
		// What the pipe already holds comes first: the board does not wait for the host.
		if let Some(broken) = state.take_in(&mut pipe, &mut reassembler).await {
			break broken;
		}
		// Then everything already asked, before the next request is put out.
		loop {
			match inbox.try_recv() {
				Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => return state.part(&mut pipe).await,
				Ok(command) => state.apply(command),
				Err(TryRecvError::Empty) => break,
			}
		}
		state.launch();
		for frame in std::mem::take(&mut state.outbox) {
			if let Err(failed) = pipe.write(&frame).await {
				break 'live format!("writing to {} over {} failed: {failed}", state.peer, state.carrier);
			}
			// A write over BLE takes tens of milliseconds; what arrived meanwhile is taken
			// in before the next one.
			if let Some(broken) = state.take_in(&mut pipe, &mut reassembler).await {
				break 'live broken;
			}
		}
		let deadline = state.flying.as_ref().map(|f| f.deadline);
		let event = tokio::select! {
			biased;
			chunk = pipe.read() => Event::Chunk(chunk),
			command = inbox.recv() => Event::Command(command),
			() = sleep_until(deadline) => Event::Deadline,
		};
		match event {
			Event::Chunk(chunk) => {
				if let Some(broken) = state.chunk(chunk, &mut reassembler) {
					break broken;
				}
			}
			Event::Command(None | Some(Command::Shutdown)) => return state.part(&mut pipe).await,
			Event::Command(Some(command)) => state.apply(command),
			Event::Deadline => state.expire(),
		}
	};
	drop(pipe);
	let _ = closed.set(broken.clone());
	state.broke(&broken);
	while let Some(command) = inbox.recv().await {
		match command {
			Command::Shutdown => return,
			command => state.refuse(command, &broken),
		}
	}
}

/// `future`'s output if it is ready now, without waiting for it.
async fn ready<F: Future>(future: F) -> Option<F::Output> {
	let mut future = std::pin::pin!(future);
	std::future::poll_fn(|cx| {
		Poll::Ready(match future.as_mut().poll(cx) {
			Poll::Ready(output) => Some(output),
			Poll::Pending => None,
		})
	})
	.await
}

/// How long a bus over the cable waits to put its parting Hello on the pipe.
const PART_WAIT: Duration = Duration::from_millis(100);

impl Remote {
	/// The bus is ending with the link up — every handle dropped, a shutdown, or a
	/// command dropped on Ctrl-C. Over the cable it says Hello once more: the board takes
	/// a Hello as the start of a new session on that carrier and closes the old one, so a
	/// host that exits with the cable in does not leave its subscriptions polling the car.
	/// Best effort and bounded. Over BLE the disconnect itself closes the session. A host
	/// killed outright (`kill -9`) sends nothing; its session lasts until the next host's
	/// Hello, a stalled write on the board, or the cable.
	async fn part<P: Pipe>(&self, pipe: &mut P) {
		if self.carrier != Carrier::Usb {
			return;
		}
		if let Ok(hello) = link::encode(&Message::Hello) {
			let _ = tokio::time::timeout(PART_WAIT, pipe.write(&hello)).await;
		}
	}

	fn at(&self, arrived: Instant) -> At {
		let since = arrived.saturating_duration_since(self.started);
		At {
			ms: u64::try_from(since.as_millis()).unwrap_or(u64::MAX),
			secs: since.as_secs_f64(),
		}
	}

	/// A board time on the bus clock (module docs, "Time"). `anchors` says whether this
	/// reading may fix the offset; before one has, a board time is its arrival.
	fn board_at(&mut self, at_ms: u32, arrived: Instant, anchors: bool) -> At {
		let arrived = self.at(arrived);
		if anchors && self.clock.is_none() {
			self.clock = Some((at_ms, arrived.secs));
		}
		let Some((first_ms, first_secs)) = self.clock else {
			return arrived;
		};
		let secs = (first_secs + (f64::from(at_ms) - f64::from(first_ms)) / 1000.0).max(0.0);
		At {
			ms: (secs * 1000.0) as u64,
			secs,
		}
	}

	fn note(&mut self, line: String) {
		if !self.notes.contains(&line) {
			self.notes.push(line);
		}
	}

	fn send(&mut self, message: &Message) -> Result<(), LinkError> {
		self.outbox.push(link::encode(message)?);
		Ok(())
	}

	/// Every chunk the pipe already holds, taken in without waiting. `Some(why)` when the
	/// link broke.
	async fn take_in<P: Pipe>(&mut self, pipe: &mut P, reassembler: &mut Reassembler) -> Option<String> {
		while let Some(chunk) = ready(pipe.read()).await {
			if let Some(broken) = self.chunk(chunk, reassembler) {
				return Some(broken);
			}
		}
		None
	}

	/// One chunk off the pipe, or its end. `Some(why)` when the link broke.
	fn chunk(&mut self, chunk: Option<Vec<u8>>, reassembler: &mut Reassembler) -> Option<String> {
		let Some(chunk) = chunk else {
			return Some(format!("the {} connection to {} dropped", self.carrier, self.peer));
		};
		// The arrival, before anything else is done with it.
		let arrived = Instant::now();
		for piece in reassembler.push(&chunk) {
			match piece {
				Piece::Message(message) => self.heard(message, arrived),
				// The settings protocol's.
				Piece::Text(_) => {}
				// BLE and USB both guarantee delivery and integrity, so a frame that does not
				// reassemble is a chunk lost on this side — a notification channel
				// (`vag_dash_ble::pump`) or a FIFO that overflowed — and nothing after it can
				// be trusted to be what it looks like.
				Piece::Error(why) => {
					return Some(format!(
						"the {} link to {} lost data ({why}), so what it sends can no longer be trusted",
						self.carrier, self.peer
					));
				}
			}
		}
		None
	}

	fn apply(&mut self, command: Command) {
		match command {
			Command::Subscribe {
				key,
				unit,
				did,
				period_ms,
				to,
				..
			} => self.subscribe(key, unit, did, period_ms, to),
			Command::Unsubscribe { key } => {
				if let Some(sub) = self.wire.remove(&key) {
					self.subs.remove(&sub);
					// Two bytes and an id: nothing about it can fail to encode.
					let _ = self.send(&Message::Unsubscribe { sub });
				}
			}
			Command::ReadOnce { unit, did, to, .. } => self.asks.push_back(Ask::Once { unit, did, to }),
			// The board checks the allowlist too; this is the cable's answer, given as early.
			Command::Exchange { unit, pdu, timeout, to, .. } => match vag_uds_client::check_read_only(&pdu) {
				Ok(()) => self.asks.push_back(Ask::Raw { unit, pdu, timeout, to }),
				Err(why) => {
					let _ = to.send(Err(ExchangeError::Forbidden(why)));
				}
			},
			Command::Shutdown => {}
		}
	}

	fn subscribe(&mut self, key: u64, unit: Unit, did: u16, period_ms: u32, to: mpsc::UnboundedSender<Sample>) {
		let now = self.at(Instant::now());
		if self.subs.len() >= MAX_SUBSCRIPTIONS {
			let why = format!("the dash board holds at most {MAX_SUBSCRIPTIONS} subscriptions at once");
			return self.end(unit, did, now, &to, &why);
		}
		let sub = self.free_sub();
		let asked = link::Subscribe {
			sub,
			request_id: unit.request,
			response_id: unit.response,
			did,
			period_ms: u16::try_from(period_ms).unwrap_or(u16::MAX).max(MIN_PERIOD_MS),
		};
		match self.send(&Message::Subscribe(asked)) {
			Ok(()) => {
				self.subs.insert(sub, Sub { unit, did, to });
				self.wire.insert(key, sub);
			}
			Err(why) => self.end(unit, did, now, &to, &why.to_string()),
		}
	}

	fn free_sub(&mut self) -> u16 {
		// At most MAX_SUBSCRIPTIONS ids are taken, so this ends within that many steps.
		loop {
			let sub = self.next_sub;
			self.next_sub = self.next_sub.wrapping_add(1);
			if !self.subs.contains_key(&sub) {
				return sub;
			}
		}
	}

	/// A subscription that will not be read: one miss now, why at the close. Its stream
	/// ends when the caller lets go of `to`.
	fn end(&mut self, unit: Unit, did: u16, at: At, to: &mpsc::UnboundedSender<Sample>, why: &str) {
		self.note(format!("{}: {:03X} {did:04X} was not read: {why}", self.peer, unit.request));
		let _ = to.send(Sample {
			unit,
			did,
			at,
			value: Err(Miss::BusError),
		});
	}

	/// Put the next request on the pipe, when none is out.
	fn launch(&mut self) {
		while self.flying.is_none() {
			let Some(ask) = self.asks.pop_front() else { return };
			let seq = self.next_seq;
			self.next_seq = seq.wrapping_add(1);
			let (unit, pdu, wait) = match &ask {
				Ask::Once { unit, did, .. } => (*unit, read_request(*did), READ_DEADLINE),
				Ask::Raw { unit, pdu, timeout, .. } => (*unit, pdu.clone(), *timeout),
			};
			let request = Message::Request(link::Request {
				seq,
				request_id: unit.request,
				response_id: unit.response,
				pdu,
			});
			match self.send(&request) {
				Ok(()) => {
					self.flying = Some(Flying {
						seq,
						deadline: Instant::now() + wait + REMOTE_GRACE,
						ask,
					});
				}
				Err(why) => match ask {
					Ask::Once { to, .. } => {
						let _ = to.send(Err(Miss::BusError));
					}
					Ask::Raw { to, .. } => {
						let _ = to.send(Err(ExchangeError::Link(TransportError::Protocol(why.to_string()))));
					}
				},
			}
		}
	}

	fn heard(&mut self, message: Message, arrived: Instant) {
		match message {
			Message::Reading(reading) => self.reading(reading, arrived),
			Message::Answer(answer) => self.answer(answer, arrived),
			// Only a host sends these.
			Message::Request(_) | Message::Subscribe(_) | Message::Unsubscribe { .. } | Message::Hello => {}
			// Asked for before the bus starts, by whoever opened the pipe; nothing here waits for it.
			Message::HelloReply(_) => {}
		}
	}

	fn reading(&mut self, reading: link::Reading, arrived: Instant) {
		// Late for a subscription already dropped: it reaches nobody and sets no clock.
		let Some(sub) = self.subs.get(&reading.sub) else { return };
		let (unit, did) = (sub.unit, sub.did);
		let anchors = matches!(reading.outcome, Outcome::Pdu(_) | Outcome::NoAnswer);
		let at = self.board_at(reading.at_ms, arrived, anchors);
		let value = match reading.outcome {
			Outcome::Refused(why) => {
				if let Some(sub) = self.subs.remove(&reading.sub) {
					self.wire.retain(|_, wire| *wire != reading.sub);
					self.end(unit, did, at, &sub.to, &format!("refused by the dash board: {why}"));
				}
				return;
			}
			outcome => read_value(did, outcome),
		};
		if let Some(sub) = self.subs.get(&reading.sub) {
			let _ = sub.to.send(Sample { unit, did, at, value });
		}
	}

	fn answer(&mut self, answer: link::Answer, arrived: Instant) {
		let Some(flying) = self.flying.take_if(|f| f.seq == answer.seq) else {
			return;
		};
		let at = self.at(arrived);
		match flying.ask {
			Ask::Once { unit, did, to } => {
				let value = match answer.outcome {
					Outcome::Refused(why) => {
						self.note(format!(
							"{}: {:03X} {did:04X} was not read: refused by the dash board: {why}",
							self.peer, unit.request
						));
						Err(Miss::BusError)
					}
					outcome => read_value(did, outcome),
				};
				let _ = to.send(value.map(|data| (data, at)));
			}
			Ask::Raw { pdu: asked, to, .. } => {
				let result = match answer.outcome {
					// The board matches answers to requests itself; this is the cable's own
					// rule applied once more, so another request's bytes never pass for these.
					Outcome::Pdu(pdu) if !answers(&asked, &pdu) => Err(ExchangeError::Link(TransportError::Protocol(format!(
						"{} answered the request with bytes that do not answer it",
						self.peer
					)))),
					Outcome::Pdu(pdu) => Ok((pdu, at)),
					// Status 1 to a request that suppressed its positive response is the board
					// saying no refusal came: what was asked for, as on a cable.
					Outcome::NoAnswer if vag_uds_client::schedule::expects_no_answer(&asked) => Ok((Vec::new(), at)),
					Outcome::NoAnswer => Err(ExchangeError::NoAnswer),
					Outcome::Refused(why) => Err(ExchangeError::Refused(why)),
					Outcome::BusError(why) => Err(ExchangeError::Link(TransportError::Io(format!("{}: {why}", self.peer)))),
				};
				let _ = to.send(result);
			}
		}
	}

	/// The request out has waited its whole deadline: it is no answer.
	fn expire(&mut self) {
		let now = Instant::now();
		let Some(flying) = self.flying.take_if(|f| f.deadline <= now) else {
			return;
		};
		match flying.ask {
			Ask::Once { to, .. } => {
				let _ = to.send(Err(Miss::NoAnswer));
			}
			Ask::Raw { to, .. } => {
				let _ = to.send(Err(ExchangeError::NoAnswer));
			}
		}
	}

	/// The link broke for `why`: every subscription gets a last miss and ends, and
	/// everything waiting fails.
	fn broke(&mut self, why: &str) {
		let now = self.at(Instant::now());
		self.wire.clear();
		for (_, sub) in self.subs.drain() {
			let _ = sub.to.send(Sample {
				unit: sub.unit,
				did: sub.did,
				at: now,
				value: Err(Miss::BusError),
			});
		}
		let waiting: Vec<Ask> = self.flying.take().map(|f| f.ask).into_iter().chain(self.asks.drain(..)).collect();
		for ask in waiting {
			match ask {
				Ask::Once { to, .. } => {
					let _ = to.send(Err(Miss::BusError));
				}
				Ask::Raw { to, .. } => {
					let _ = to.send(Err(ExchangeError::Link(TransportError::Io(why.to_string()))));
				}
			}
		}
	}

	/// A command after the link broke for `why`.
	fn refuse(&self, command: Command, why: &str) {
		match command {
			// Dropping its sender here ends the new subscription's stream at once.
			Command::Subscribe { .. } | Command::Unsubscribe { .. } | Command::Shutdown => {}
			Command::ReadOnce { to, .. } => {
				let _ = to.send(Err(Miss::BusError));
			}
			Command::Exchange { to, .. } => {
				let _ = to.send(Err(ExchangeError::Link(TransportError::Io(why.to_string()))));
			}
		}
	}
}

fn read_request(did: u16) -> Vec<u8> {
	let [hi, lo] = did.to_be_bytes();
	vec![READ, hi, lo]
}

/// A read's outcome as the board sent it: the record with its `62 hi lo` echo taken off,
/// or why there is none. The unit's own refusal is `7F 22 nrc`, as on a cable.
fn read_value(did: u16, outcome: Outcome) -> Result<Vec<u8>, Miss> {
	match outcome {
		Outcome::Pdu(pdu) => match pdu.as_slice() {
			[READ_POSITIVE, hi, lo, data @ ..] if u16::from_be_bytes([*hi, *lo]) == did => Ok(data.to_vec()),
			[NEGATIVE, READ, nrc, ..] => Err(Miss::Refused(*nrc)),
			_ => Err(Miss::Malformed),
		},
		Outcome::NoAnswer => Err(Miss::NoAnswer),
		Outcome::Refused(_) | Outcome::BusError(_) => Err(Miss::BusError),
	}
}

#[cfg(test)]
mod tests;
