//! Ctrl-C, for every command: heard on a thread of its own, so it ends the process
//! whatever the command is doing.
//!
//! **Why a thread, and not a branch of the command's `select!`.** Until 2026-10-21
//! `vagcan` raced the command against `tokio::signal::ctrl_c()` on the main task. Polling
//! that future installs tokio's SIGINT handler, so the default "SIGINT ends the process"
//! is gone from then on — and the branch only runs when the main task is polled. A
//! command that blocks the thread (a label parse, a SQLite build, a rayon key search, a
//! `read_line` at a question, blocking file IO) never gives the task back, so the signal
//! was taken and nothing acted on it. `select!` polls its branches in a random order, so
//! the same command either died on the default disposition (the handler not installed
//! yet) or ignored Ctrl-C for good — about half the runs each, measured with a pty.
//!
//! Here the handler is installed before the command starts, on its own OS thread with its
//! own one-thread runtime, so nothing the command does can keep it from running. On the
//! first Ctrl-C it:
//!
//! 1. tells the command ([`requested`]): the main task's `select!` (see [`run`]) drops the
//!    command when it is next polled, and every bus handle with it — each bus task ends
//!    and drops its link, and a serial link closes its channel on the way
//!    (`SlcanBackend::closing_on_drop`), which is what takes a dash board out of the
//!    adapter mode `--slcan` put it in;
//! 2. on yet another thread, stops every progress line, hands the terminal back
//!    ([`term::give_back`](crate::ui::term::give_back)) — once the command is gone, or
//!    after a moment when it is blocked and cannot go — and says what an interrupted
//!    registry read leaves behind — another thread because each of those writes to a
//!    terminal, and a terminal that has stopped taking bytes must not hold up the exit;
//! 3. waits [`GRACE`] for the links to close, then ends the process with 130, the
//!    shell's code for a SIGINT — by `exit`, not by returning: the bus tasks run on the
//!    blocking pool, whose shutdown would also wait for anything else parked there. A
//!    second Ctrl-C in that time ends it at once.
//!
//! A command blocked on its thread at that moment is not dropped — nothing can drop a
//! future that is not being polled; `measure` holds a bus across its questions. So the
//! bus tasks do not wait for it: each waits on [`ends`] beside its work, on its own
//! blocking-pool thread, and returns — dropping its link, close command and all — the
//! moment the process is ending, whatever the command's thread is doing.
//!
//! Right before the exit, on every path, the terminal is given back once more: raw mode
//! first, then the rest. A screen that switched on after the first give-back (a replay
//! still loading when Ctrl-C came) is refused once the process is ending, and undone if
//! it raced in ([`term`](crate::ui::term)).
//!
//! **Raw mode is the other half.** A menu or a full screen turns the terminal's own
//! interrupt off, and Ctrl-C arrives as a key. [`term::interrupts`](crate::ui::term::interrupts)
//! recognises it; a menu hands it here through [`raise`], a full screen quits on it.

use std::future::Future;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use tokio::sync::Notify;

/// How long an interrupted command's links get to close before the process ends.
pub const GRACE: Duration = Duration::from_millis(300);

/// The exit code of a process a Ctrl-C ended: 128 + SIGINT, as a shell reports it.
pub const EXIT_CODE: u8 = 130;

/// What the first Ctrl-C does. A second one always ends the process at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum First {
	/// Ends the process after [`GRACE`]: every command but one.
	Ends,
	/// Only tells the command ([`requested`]), which stops itself: `dev sniff` prints the
	/// summary of what it saw.
	Stops,
}

/// Set by the first Ctrl-C, whatever [`First`] says.
static REQUESTED: AtomicBool = AtomicBool::new(false);
/// Set by the first Ctrl-C when it ends the process: from then on nothing the command
/// returns is reported, and the handler's exit is the process's.
static ENDING: AtomicBool = AtomicBool::new(false);
/// Wakes every [`requested`].
static TOLD: Notify = Notify::const_new();
/// A Ctrl-C read as a key in raw mode, handed to the handler ([`raise`]).
static RAISED: Notify = Notify::const_new();
/// What the first Ctrl-C does in this process: 0 until [`install`] has a listener
/// running, then a [`First`].
static POLICY: AtomicU8 = AtomicU8::new(0);
/// Set by the first [`install`], whether or not its listener starts.
static INSTALLING: AtomicBool = AtomicBool::new(false);
/// Set by [`run`] once the interrupted command is gone: dropped, or finished anyway.
static GONE: AtomicBool = AtomicBool::new(false);
/// How long the terminal is left to the command's own guards before it is taken back.
const GONE_WITHIN: Duration = Duration::from_millis(50);

/// Ctrl-C, as an error: what a menu returns when it was pressed there. The process is
/// ending by then; the error only carries the command out of its way.
#[derive(Debug)]
pub struct Interrupted;

impl std::fmt::Display for Interrupted {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("interrupted")
	}
}

impl std::error::Error for Interrupted {}

/// Install the handler, once per process, before the command starts. The signal is
/// registered before this returns, so a Ctrl-C a moment later is not lost.
///
/// A process that cannot have a signal handler (no thread, no runtime) keeps the
/// default disposition, which still ends it; that is said under `-v` and not refused.
pub fn install(first: First) {
	if INSTALLING.swap(true, Ordering::SeqCst) {
		return;
	}
	// The policy is set only once a listener runs: with none, a key's [`raise`] must not
	// mark the process as ending, or [`run`] would wait for an exit nobody makes.
	match spawn(first) {
		Ok(()) => POLICY.store(code(first), Ordering::SeqCst),
		Err(why) => crate::note!("Ctrl-C: no handler of its own ({why}); it ends the process without closing the links"),
	}
}

fn code(first: First) -> u8 {
	match first {
		First::Ends => 1,
		First::Stops => 2,
	}
}

/// The installed handler's policy, if one is listening.
fn policy() -> Option<First> {
	match POLICY.load(Ordering::SeqCst) {
		1 => Some(First::Ends),
		2 => Some(First::Stops),
		_ => None,
	}
}

fn spawn(first: First) -> std::io::Result<()> {
	let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
	let signals = {
		let _inside = runtime.enter();
		Signals::new()?
	};
	std::thread::Builder::new()
		.name("ctrl-c".into())
		.spawn(move || runtime.block_on(listen(first, signals)))?;
	Ok(())
}

async fn listen(first: First, mut signals: Signals) {
	next(&mut signals).await;
	heard(Some(first));
	if first == First::Ends {
		// Detached: the exit below does not wait for it, and must not.
		let _ = std::thread::Builder::new().name("ctrl-c-goodbye".into()).spawn(goodbye);
		tokio::select! {
			() = tokio::time::sleep(GRACE) => {}
			() = next(&mut signals) => {}
		}
	} else {
		next(&mut signals).await;
	}
	// On every way out, a second Ctrl-C and `dev sniff`'s included, and after a screen
	// that switched on late: raw mode first, which writes nothing and so cannot be held
	// up by a terminal that takes no bytes, then everything else still on.
	crate::ui::term::give_back_raw();
	crate::ui::term::give_back();
	std::process::exit(i32::from(EXIT_CODE));
}

/// The first Ctrl-C, from the signal or a key: the flags set and the command told.
/// Called by [`raise`] on the loop's own thread too, so the flags are up before the
/// loop's error reaches [`run`]; doing it twice changes nothing. Ending is set only
/// with a listener that will make the exit.
fn heard(first: Option<First>) {
	REQUESTED.store(true, Ordering::SeqCst);
	if first == Some(First::Ends) {
		ENDING.store(true, Ordering::SeqCst);
	}
	TOLD.notify_waiters();
}

/// The next Ctrl-C: the signal, or the key a raw-mode loop read ([`raise`]).
async fn next(signals: &mut Signals) {
	tokio::select! {
		() = signals.recv() => {}
		() = RAISED.notified() => {}
	}
}

/// What the screen is left with. A spinner a blocking read is drawing cannot hear the
/// signal and would be left frozen mid-line, ticking on until the process ends: stop
/// every line drawing, give the terminal back, end the line on screen, and where a key
/// search was running say what it leaves behind — every key found is saved as it is
/// found, so the next run does not search for it again.
fn goodbye() {
	crate::progress::silence();
	// A command that is being polled is dropped at once, and its own guards give the
	// terminal back in order; giving it back from here first would let its last frame land
	// on the shell's screen. A command blocked on its thread never gets there, and is not
	// waited for longer than this.
	let waiting = std::time::Instant::now();
	while !GONE.load(Ordering::SeqCst) && waiting.elapsed() < GONE_WITHIN {
		std::thread::sleep(Duration::from_millis(1));
	}
	crate::ui::term::give_back();
	if std::io::IsTerminal::is_terminal(&std::io::stderr()) {
		crate::say!();
	}
	if crate::registry::reading() {
		// Saved after the tables and after each unit — not after each key, so a unit
		// that needs two searches and was cut short between them is searched for again.
		crate::say!("interrupted: the VCDS registry keys found so far are cached; the next run goes on from them");
	}
}

/// Whether Ctrl-C has been pressed in this process.
pub fn is_requested() -> bool {
	REQUESTED.load(Ordering::SeqCst)
}

/// Whether the process is ending on a Ctrl-C: whatever the command returns from now on
/// is not the process's outcome.
pub fn ending() -> bool {
	ENDING.load(Ordering::SeqCst)
}

/// Resolves on the first Ctrl-C — at once if it has been pressed already.
pub async fn requested() {
	loop {
		let told = TOLD.notified();
		tokio::pin!(told);
		// Registered before the flag is read, so a Ctrl-C between the two is not missed.
		told.as_mut().enable();
		if is_requested() {
			return;
		}
		told.await;
	}
}

/// A Ctrl-C a raw-mode loop read as a key, handed to the handler as if it were the
/// signal. The error is for the loop to return, so the command stops on its way out.
pub fn raise() -> anyhow::Error {
	heard(policy());
	RAISED.notify_one();
	anyhow::Error::new(Interrupted)
}

/// Resolves once the process is ending on a Ctrl-C — at once if it is already. What a
/// bus task waits on beside its work, so it drops its link on its own thread even while
/// the command's thread is blocked (a question waiting for a line) and cannot drop it.
pub async fn ends() {
	loop {
		let told = TOLD.notified();
		tokio::pin!(told);
		told.as_mut().enable();
		if ending() {
			return;
		}
		told.await;
	}
}

/// The state a Ctrl-C leaves behind, with no handler to end the process: for a test of
/// what reacts to it, in a test binary of its own (the flags stay set for its life).
#[cfg(any(test, feature = "test-util"))]
pub fn end_for_test() {
	REQUESTED.store(true, Ordering::SeqCst);
	ENDING.store(true, Ordering::SeqCst);
	TOLD.notify_waiters();
}

/// Whether an error is a Ctrl-C's ([`raise`]) rather than the command's own.
pub fn is_interrupted(err: &anyhow::Error) -> bool {
	err.is::<Interrupted>()
}

/// Run `command` until it finishes or Ctrl-C is pressed, whichever comes first.
///
/// On Ctrl-C the command is dropped here, with every bus handle it holds (module
/// note), and this never returns: the handler ends the process. Once the process is
/// ending, a command that finished anyway — or failed because a menu returned
/// [`Interrupted`] — does not return either, so its outcome is never reported over the
/// interrupt's.
pub async fn run<F: Future>(command: F) -> F::Output {
	let out = tokio::select! {
		out = command => out,
		// `select!` drops every branch's future before it runs the one that won.
		() = requested() => {
			GONE.store(true, Ordering::SeqCst);
			std::future::pending().await
		}
	};
	if ending() {
		GONE.store(true, Ordering::SeqCst);
		std::future::pending::<()>().await;
	}
	out
}

/// The signal, on whichever platform this is.
struct Signals {
	#[cfg(unix)]
	inner: tokio::signal::unix::Signal,
	#[cfg(windows)]
	inner: tokio::signal::windows::CtrlC,
}

impl Signals {
	fn new() -> std::io::Result<Signals> {
		Ok(Signals {
			#[cfg(unix)]
			inner: tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?,
			#[cfg(windows)]
			inner: tokio::signal::windows::ctrl_c()?,
		})
	}

	/// The next signal. A stream that has ended (the runtime is going away) never yields
	/// again, rather than reading as a Ctrl-C.
	async fn recv(&mut self) {
		if self.inner.recv().await.is_none() {
			std::future::pending::<()>().await;
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use anyhow::Context as _;
	use std::sync::Arc;

	/// Set when it is dropped: a bus handle, as far as this test is concerned.
	struct Link(Arc<AtomicBool>);

	impl Drop for Link {
		fn drop(&mut self) {
			self.0.store(true, Ordering::SeqCst);
		}
	}

	#[tokio::test]
	async fn a_ctrl_c_drops_the_command_it_interrupts() {
		// What keeps `--slcan`'s promise: the command, and every link it holds, is dropped
		// on the main task while the handler waits out the grace — a link writes its close
		// command on its way out. The only test in this binary that presses Ctrl-C: the flag
		// it sets stays set for the life of the process. No handler is installed, so nothing
		// here ends the process.
		let closed = Arc::new(AtomicBool::new(false));
		let link = Link(Arc::clone(&closed));
		let command = async move {
			let _link = link;
			std::future::pending::<()>().await;
		};
		let running = tokio::spawn(run(command));
		tokio::task::yield_now().await;
		assert!(!closed.load(Ordering::SeqCst), "dropped before Ctrl-C");
		let raised = raise();
		assert!(
			is_interrupted(&Err::<(), _>(raised).context("choosing a source").unwrap_err()),
			"found through a context"
		);
		tokio::time::timeout(Duration::from_secs(1), async {
			while !closed.load(Ordering::SeqCst) {
				tokio::task::yield_now().await;
			}
		})
		.await
		.expect("the command was not dropped");
		assert!(GONE.load(Ordering::SeqCst));
		assert!(is_requested());
		// `run` itself never returns: the process is meant to end under it.
		assert!(!running.is_finished());
		running.abort();
	}
}
