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

/// How many `7F xx 78` in a row one request may be answered with before the unit counts
/// as not answering: the async UDS client's own limit, so an exchange through the bus
/// gives up exactly where it did talking to the link directly.
pub const MAX_PENDING: usize = 30;

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
	/// The bus has shut down.
	Closed,
}

impl std::fmt::Display for ExchangeError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			ExchangeError::Forbidden(why) => write!(f, "{why}"),
			ExchangeError::NoAnswer => write!(f, "no answer"),
			ExchangeError::Link(why) => write!(f, "{why}"),
			ExchangeError::Closed => write!(f, "the bus has shut down"),
		}
	}
}

impl std::error::Error for ExchangeError {}

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
}

impl Bus {
	/// Hand `link` to a new scheduler task running under `budget`, and return a handle.
	///
	/// Must be called inside a tokio runtime. A cable uses [`Budget::default`].
	pub fn start<L: vag_uds_can::UnitLink + Send + 'static>(link: L, budget: Budget) -> Bus {
		let (commands, inbox) = mpsc::unbounded_channel();
		let started = Instant::now();
		let runtime = tokio::runtime::Handle::current();
		tokio::task::spawn_blocking(move || runtime.block_on(task::run(link, budget, inbox, started)));
		Bus {
			commands,
			started,
			keys: Arc::new(AtomicU64::new(0)),
		}
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
		}
	}

	/// Read `did` of `unit` once. It rides with anything of that unit due soon.
	pub async fn read_once(&self, class: Class, unit: Unit, did: u16) -> Result<(Vec<u8>, At), Miss> {
		let (to, rx) = oneshot::channel();
		if self.commands.send(Command::ReadOnce { class, unit, did, to }).is_err() {
			return Err(Miss::BusError);
		}
		rx.await.unwrap_or(Err(Miss::BusError))
	}

	/// Send one whole UDS request to `unit` and return its answer as it came — a negative
	/// response included — waiting [`vag_uds_client`]'s own default deadline.
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
}

impl Subscription {
	/// The next reading or miss, in arrival order. `None` once the bus has shut down.
	pub async fn next(&mut self) -> Option<Sample> {
		self.rx.recv().await
	}

	/// [`next`](Self::next) as a poll, for waiting on several at once ([`next_of`]).
	pub fn poll_next(&mut self, cx: &mut Context<'_>) -> Poll<Option<Sample>> {
		self.rx.poll_recv(cx)
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
/// Waits forever on an empty slice, so it can sit in a `select!` beside a keyboard while
/// nothing is selected; `None` once every subscription's bus has shut down.
pub async fn next_of(subs: &mut [Subscription]) -> Option<(usize, Sample)> {
	if subs.is_empty() {
		return std::future::pending().await;
	}
	// Where to start looking, turned each poll so one busy subscription cannot keep the
	// rest waiting.
	let mut start = 0usize;
	std::future::poll_fn(|cx| {
		let n = subs.len();
		let mut closed = 0;
		for step in 0..n {
			let i = (start + step) % n;
			match subs[i].poll_next(cx) {
				Poll::Ready(Some(sample)) => {
					start = (i + 1) % n;
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
