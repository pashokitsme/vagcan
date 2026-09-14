//! [`Bus`]: the laptop's shell over the bus scheduler, and the only way a command
//! talks to the car.
//!
//! Design: `todo/dash/14-one-bus-three-clients.md` §2, "subscriptions, not a priority
//! queue" (owner, 2026-09-14). The decisions live in
//! [`vag_uds_client::schedule::Planner`], which has no clock, no bus and no runtime;
//! this module is the tokio half of it:
//!
//! - **One task owns the link.** [`Bus::start`] hands the opened adapter to it and
//!   gets back a cheap [`Clone`] handle. Nothing else ever holds the link, and the
//!   planner never has two requests out, so there is exactly one exchange in flight.
//! - **Handles talk to the task over a channel.** [`Bus::subscribe`] returns a
//!   [`Subscription`] whose [`next`](Subscription::next) yields every reading of one
//!   `(unit, identifier)` at the period asked for, and whose `Drop` unsubscribes.
//!   [`Bus::read_once`] reads one identifier once; [`Bus::exchange`] sends one whole
//!   request and returns the answer as it came.
//! - **Every command generic over [`UnitLink`](vag_uds_can::UnitLink) runs through it
//!   unchanged**: a `Bus` is a link (`channel.rs`), whose exchanges queue as
//!   [`Class::Foreground`] raw requests.
//!
//! # Time
//!
//! The task keeps one monotonic origin, the moment it was started. Every [`Sample`]
//! is stamped with the instant its answer was received — taken as the link hands the
//! PDU back, before anything else is done with it — as whole milliseconds for the
//! planner and as seconds for whoever times something from it (`measure` does).
//!
//! # Why a blocking-pool thread
//!
//! The task is generic over the link, and the futures of an `async fn` in a trait
//! cannot be proven `Send` from inside generic code on stable Rust, so it cannot be
//! given to `tokio::spawn`. It runs instead on a thread of the runtime's blocking pool,
//! driven by [`Handle::block_on`](tokio::runtime::Handle::block_on): the same runtime,
//! the same timers and I/O drivers, and only the link itself has to be `Send`.

mod channel;
mod remote;
mod task;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

pub use channel::BusChannel;
pub use vag_uds_client::schedule::{Budget, Class, Miss, Unit};
use vag_uds_transport::TransportError;

/// How long a scheduled read waits for its answer.
///
/// Ten times the default server response time ISO 14229-2 gives a unit (P2, 50 ms):
/// room for the USB hop and a multi-frame answer, while a unit that is not there costs
/// half a second rather than two. A unit that needs longer says `7F 22 78` and gets
/// [`PENDING_WAIT`]. A raw [`Bus::exchange_within`] waits what its caller asks.
pub const READ_DEADLINE: Duration = Duration::from_millis(500);

/// How long the task waits after a `7F xx 78` (response pending) for the next answer.
/// ISO 14229-2's default P2*server_max, 5 s.
pub const PENDING_WAIT: Duration = Duration::from_secs(5);

/// How long a request that suppressed its positive response (`3E 80`, `10 81`) waits for
/// the refusal that is its only possible answer: three times ISO 14229-2's default
/// P2server_max (50 ms), room for a unit behind the gateway — the board's own figure
/// (`vag-dash-fw`'s `SUPPRESSED_WAIT`). Silence after it is
/// [`Answer::NotExpected`](vag_uds_client::schedule::Answer::NotExpected), which a raw
/// exchange's caller gets as a success with no data.
pub const SUPPRESSED_WAIT: Duration = Duration::from_millis(150);

/// How long a one-shot read ([`Bus::read_once`], [`Bus::read_all`]) on a bus under `budget`
/// waits, queue and answer together, before its caller gets [`Miss::NoAnswer`]:
///
/// - [`Budget::starve_after_ms`], the longest it waits behind a timing channel before it
///   goes ahead of it;
/// - [`READ_DEADLINE`], for the exchange already out when it does;
/// - [`PENDING_WAIT`], for its own unit saying `78` before it answers.
///
/// A bus over the dash board adds [`REMOTE_GRACE`]. The task bounds each exchange on its
/// own; this bounds the caller, whatever holds the task up — a queue in front of the read,
/// a unit that keeps saying `78`, a backend that does not honour its deadline. A read given
/// up on may still go out later; its answer then reaches nobody.
pub fn once_deadline(budget: &Budget) -> Duration {
	Duration::from_millis(u64::from(budget.starve_after_ms)) + READ_DEADLINE + PENDING_WAIT
}

/// How many `7F xx 78` in a row one request may be answered with before the unit counts
/// as not answering: the async UDS client's own limit, so an exchange through the bus
/// gives up exactly where it did talking to the link directly.
pub const MAX_PENDING: usize = 30;

/// What a request over the dash board ([`Bus::start_remote`]) waits past its caller's
/// own deadline.
///
/// The board answers every request, but may hold one a long time first, and the
/// caller's deadline never reaches it — the link has no field for one:
///
/// - up to [`RATE_WINDOW_MS`](vag_uds_client::guard::RATE_WINDOW_MS), 10 s, waiting out
///   the guard's rate cap: over it the board delays a request and never refuses it;
/// - up to 10 s more for the unit: the board waits out `7F xx 78` itself up to twice
///   [`PENDING_WAIT`] (the firmware's pending cap), and a unit that does not answer is
///   backed off from before it is asked — a request to one came back as no answer after
///   5.8 s on the bench (2026-09-14);
/// - 5 s for the rest: the speed read a session change waits for, the panel's own reads
///   ahead of it, and the radio.
///
/// Past this the board or the radio has gone quiet, and the request counts as no answer.
/// Generous on purpose: giving up on a request the board still holds only puts the next
/// one behind it.
pub const REMOTE_GRACE: Duration = Duration::from_millis(vag_uds_client::guard::RATE_WINDOW_MS + 15_000);

/// What a bus over the dash board ([`Bus::start_remote`]) runs on. The framing is the
/// same on both; only what is said when the link breaks differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Carrier {
	/// Bluetooth LE, the board's UART service.
	Ble,
	/// The board's USB cable, with its `dash` image running.
	Usb,
}

impl std::fmt::Display for Carrier {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Carrier::Ble => "BLE",
			Carrier::Usb => "USB",
		})
	}
}

/// When a sample arrived, on the bus's own clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct At {
	/// Whole milliseconds since the bus started — what the planner was told.
	pub ms: u64,
	/// Seconds since the bus started, to the resolution of the clock.
	pub secs: f64,
}

/// One delivery to a subscriber: a reading, or the reading that did not come.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
	pub unit: Unit,
	pub did: u16,
	pub at: At,
	/// The record's bytes, identifier echo stripped; or why there are none this time.
	pub value: Result<Vec<u8>, Miss>,
	/// `Some` on the last sample of a subscription that ends while its consumer still
	/// holds it, with why, in words for a person: the dash board refused it ("refused by
	/// the dash board — the car is moving"), or the link to the board broke. Its `value`
	/// is then [`Miss::BusError`], and the stream ends after it. A subscription on a
	/// cable never ends this way; it ends only with the bus.
	pub ended: Option<String>,
}

/// Why a raw exchange came back without an answer.
#[derive(Debug)]
pub enum ExchangeError {
	/// Refused before it was queued: the service is outside the read-only allowlist.
	Forbidden(vag_uds_client::UdsError),
	/// The unit did not answer within the deadline.
	NoAnswer,
	/// The link failed under the request.
	Link(TransportError),
	/// The dash board would not put it on the bus, and said why (a bus over the board
	/// only; see `remote.rs`).
	Refused(String),
	/// The bus has shut down.
	Closed,
}

impl std::fmt::Display for ExchangeError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			ExchangeError::Forbidden(why) => write!(f, "{why}"),
			ExchangeError::NoAnswer => write!(f, "no answer"),
			ExchangeError::Link(why) => write!(f, "{why}"),
			ExchangeError::Refused(why) => f.write_str(&refused_by_board(why)),
			ExchangeError::Closed => write!(f, "the bus has shut down"),
		}
	}
}

impl std::error::Error for ExchangeError {}

/// A refusal by the dash board in words, one spelling wherever it surfaces: a
/// subscription's end, a one-shot read's note, an exchange's error.
fn refused_by_board(why: &str) -> String {
	format!("refused by the dash board — {why}")
}

/// Where a one-shot read's result goes.
type OnceReply = oneshot::Sender<Result<(Vec<u8>, At), Miss>>;
/// Where a raw exchange's answer goes.
type RawReply = oneshot::Sender<Result<(Vec<u8>, At), ExchangeError>>;

/// What a handle asks of the task.
enum Command {
	Subscribe {
		key: u64,
		class: Class,
		unit: Unit,
		did: u16,
		period_ms: u32,
		record_len: Option<u16>,
		to: mpsc::UnboundedSender<Sample>,
	},
	Unsubscribe {
		key: u64,
	},
	ReadOnce {
		class: Class,
		unit: Unit,
		did: u16,
		to: OnceReply,
	},
	Exchange {
		class: Class,
		unit: Unit,
		pdu: Vec<u8>,
		timeout: Duration,
		to: RawReply,
	},
	Shutdown,
}

/// A handle on the one task that owns the link. Cheap to clone; see the module docs.
///
/// The task ends — and drops the link, closing the port — when every `Bus` and every
/// [`Subscription`] is gone, or at [`Bus::shutdown`].
#[derive(Clone)]
pub struct Bus {
	commands: mpsc::UnboundedSender<Command>,
	started: Instant,
	keys: Arc<AtomicU64>,
	/// Why the link broke under the task, once it has (see [`Bus::closed`]).
	closed: Arc<std::sync::OnceLock<String>>,
	/// How long a one-shot read waits: [`once_deadline`] of the planner's budget, plus
	/// [`REMOTE_GRACE`] over the dash board.
	once_wait: Duration,
}

impl Bus {
	/// Hand `link` to a new scheduler task running under `budget`, and return a handle.
	///
	/// Must be called inside a tokio runtime. A cable uses [`Budget::default`].
	pub fn start<L: vag_uds_can::UnitLink + Send + 'static>(link: L, budget: Budget) -> Bus {
		let (commands, inbox) = mpsc::unbounded_channel();
		let started = Instant::now();
		let runtime = tokio::runtime::Handle::current();
		let once_wait = once_deadline(&budget);
		tokio::task::spawn_blocking(move || runtime.block_on(task::run(link, budget, inbox, started)));
		Bus {
			commands,
			started,
			keys: Arc::new(AtomicU64::new(0)),
			closed: Arc::new(std::sync::OnceLock::new()),
			once_wait,
		}
	}

	/// The same handles over a byte pipe to the dash board, which runs the planner and
	/// its guard itself: this task only forwards (`remote.rs`). `peer` names the board, and
	/// `carrier` what the pipe runs on, in what is said when it refuses something or the
	/// connection drops.
	///
	/// Must be called inside a tokio runtime. Runs on a blocking-pool thread for the
	/// reason [`Bus::start`] does.
	pub fn start_remote<P: vag_uds_transport::link::Pipe + Send + 'static>(pipe: P, peer: &str, carrier: Carrier) -> Bus {
		let (commands, inbox) = mpsc::unbounded_channel();
		let started = Instant::now();
		let runtime = tokio::runtime::Handle::current();
		let peer = peer.to_string();
		let closed = Arc::new(std::sync::OnceLock::new());
		let told = closed.clone();
		tokio::task::spawn_blocking(move || runtime.block_on(remote::run(pipe, peer, carrier, inbox, started, told)));
		Bus {
			commands,
			started,
			keys: Arc::new(AtomicU64::new(0)),
			closed,
			// The board plans under its own budget.
			once_wait: once_deadline(&Budget::board()) + REMOTE_GRACE,
		}
	}

	/// Why the link broke under the bus, once it has — "the BLE connection to vagcan-dash
	/// dropped" — so a consumer whose subscriptions ended can say why rather than only
	/// that they did. `None` while the link is up, and for a cable, whose task does not
	/// outlive its link.
	pub fn closed(&self) -> Option<String> {
		self.closed.get().cloned()
	}

	/// Seconds since the bus started: the clock every [`At::secs`] is on.
	pub fn secs(&self) -> f64 {
		self.started.elapsed().as_secs_f64()
	}

	/// Deliver `did` of `unit` every `period` until the returned handle is dropped. The
	/// first reading is due at once.
	///
	/// `record_len` is the record's length in bytes when it is known, which lets a
	/// multi-identifier answer be cut without a search.
	pub fn subscribe(&self, class: Class, unit: Unit, did: u16, period: Duration, record_len: Option<u16>) -> Subscription {
		let key = self.keys.fetch_add(1, Ordering::Relaxed);
		let (to, rx) = mpsc::unbounded_channel();
		let period_ms = u32::try_from(period.as_millis()).unwrap_or(u32::MAX).max(1);
		// A bus that has shut down leaves `rx` with no sender, which is what `next` says.
		let _ = self.commands.send(Command::Subscribe {
			key,
			class,
			unit,
			did,
			period_ms,
			record_len,
			to,
		});
		Subscription {
			key,
			unit,
			did,
			rx,
			commands: self.commands.clone(),
			ended: None,
		}
	}

	/// Read `did` of `unit` once. It rides with anything of that unit due soon. A read that
	/// has not come back within [`once_deadline`] of the budget (plus [`REMOTE_GRACE`] over
	/// the board) is [`Miss::NoAnswer`].
	pub async fn read_once(&self, class: Class, unit: Unit, did: u16) -> Result<(Vec<u8>, At), Miss> {
		self.read_all(class, &[(unit, did)]).await.pop().unwrap_or(Err(Miss::BusError))
	}

	/// Read several identifiers once each. All are asked before any is waited for, so
	/// those of one unit ride in one request; the results come back in the order asked.
	/// One deadline for all of them, as for [`read_once`](Self::read_once).
	pub async fn read_all(&self, class: Class, reads: &[(Unit, u16)]) -> Vec<Result<(Vec<u8>, At), Miss>> {
		let until = Instant::now() + self.once_wait;
		let waiting: Vec<_> = reads
			.iter()
			.map(|&(unit, did)| {
				let (to, rx) = oneshot::channel();
				self.commands.send(Command::ReadOnce { class, unit, did, to }).ok().map(|()| rx)
			})
			.collect();
		let mut out = Vec::with_capacity(waiting.len());
		for rx in waiting {
			out.push(match rx {
				Some(rx) => match tokio::time::timeout_at(until, rx).await {
					Ok(answered) => answered.unwrap_or(Err(Miss::BusError)),
					Err(_elapsed) => Err(Miss::NoAnswer),
				},
				None => Err(Miss::BusError),
			});
		}
		out
	}

	/// Send one whole UDS request to `unit` and return its answer as it came — a negative
	/// response included — waiting [`vag_uds_client`]'s own default deadline. A request that
	/// suppressed its positive response and was not refused comes back as an empty answer,
	/// after [`SUPPRESSED_WAIT`] on a cable.
	pub async fn exchange(&self, class: Class, unit: Unit, pdu: Vec<u8>) -> Result<(Vec<u8>, At), ExchangeError> {
		self.exchange_within(class, unit, pdu, DEFAULT_EXCHANGE_DEADLINE).await
	}

	/// The same, with the deadline for the first answer chosen by the caller.
	pub async fn exchange_within(&self, class: Class, unit: Unit, pdu: Vec<u8>, timeout: Duration) -> Result<(Vec<u8>, At), ExchangeError> {
		let (to, rx) = oneshot::channel();
		let asked = Command::Exchange {
			class,
			unit,
			pdu,
			timeout,
			to,
		};
		if self.commands.send(asked).is_err() {
			return Err(ExchangeError::Closed);
		}
		rx.await.unwrap_or(Err(ExchangeError::Closed))
	}

	/// Stop the task now, whoever else holds a handle, and let go of the link. An
	/// exchange in flight is abandoned; every waiting consumer is told the bus closed.
	pub fn shutdown(&self) {
		let _ = self.commands.send(Command::Shutdown);
	}
}

/// What `exchange` waits for a first answer: the UDS client's default, 2 s.
const DEFAULT_EXCHANGE_DEADLINE: Duration = Duration::from_millis(2000);

/// One live subscription. Dropping it unsubscribes.
pub struct Subscription {
	key: u64,
	unit: Unit,
	did: u16,
	rx: mpsc::UnboundedReceiver<Sample>,
	commands: mpsc::UnboundedSender<Command>,
	/// Why it ended, once its last sample ([`Sample::ended`]) has been delivered.
	ended: Option<String>,
}

impl Subscription {
	/// The next reading or miss, in arrival order. `None` once the subscription has ended
	/// ([`ended`](Self::ended) says why, when there is a reason) or the bus has shut down.
	pub async fn next(&mut self) -> Option<Sample> {
		std::future::poll_fn(|cx| self.poll_next(cx)).await
	}

	/// [`next`](Self::next) as a poll, for waiting on several at once ([`next_of`]).
	pub fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<Sample>> {
		let polled = self.rx.poll_recv(cx);
		if let Poll::Ready(Some(Sample { ended: Some(why), .. })) = &polled {
			self.ended = Some(why.clone());
		}
		polled
	}

	/// Why this subscription ended while it was held, once its last sample has been
	/// delivered: the words [`Sample::ended`] carried. `None` while it is live.
	pub fn ended(&self) -> Option<&str> {
		self.ended.as_deref()
	}

	pub fn unit(&self) -> Unit {
		self.unit
	}

	pub fn did(&self) -> u16 {
		self.did
	}
}

impl Drop for Subscription {
	fn drop(&mut self) {
		// Never blocks: the channel is unbounded, and a bus already gone needs no word.
		let _ = self.commands.send(Command::Unsubscribe { key: self.key });
	}
}

/// The next sample from any of `subs`, with the index of the one it came from.
///
/// `cursor` is where the next look starts, and it is the caller's to keep from one call
/// to the next: it moves past whichever subscription just delivered, so one busy
/// subscription cannot keep the rest waiting. Any value is valid, including after the
/// slice has changed length.
///
/// Waits forever on an empty slice, so it can sit in a `select!` beside a keyboard while
/// nothing is selected; `None` once every subscription's bus has shut down.
pub async fn next_of(subs: &mut [Subscription], cursor: &mut usize) -> Option<(usize, Sample)> {
	if subs.is_empty() {
		return std::future::pending().await;
	}
	std::future::poll_fn(|cx| {
		let n = subs.len();
		let start = *cursor % n;
		let mut closed = 0;
		for step in 0..n {
			let i = (start + step) % n;
			match subs[i].poll_next(cx) {
				Poll::Ready(Some(sample)) => {
					*cursor = (i + 1) % n;
					return Poll::Ready(Some((i, sample)));
				}
				Poll::Ready(None) => closed += 1,
				Poll::Pending => {}
			}
		}
		match closed == n {
			true => Poll::Ready(None),
			false => Poll::Pending,
		}
	})
	.await
}
