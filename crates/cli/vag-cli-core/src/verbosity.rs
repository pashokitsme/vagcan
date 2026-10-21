//! How much a command says besides its answer: `-v` / `--verbose`, counted.
//!
//! | level | flag   | what it adds, on stderr                                                   |
//! |-------|--------|---------------------------------------------------------------------------|
//! | 0     |        | nothing: the data, one summary line, errors, the questions it must ask    |
//! | 1     | `-v`   | where the data came from and how much of it, which link, why a row is     |
//! |       |        | unnamed or skipped, the caveats, timings, per-unit notes                  |
//! | 2     | `-vv`  | the wire: every request and answer, `78` pending, timeouts, late answers, |
//! |       |        | the scheduler's batching, label-file lookups                              |
//!
//! **Data goes to stdout and nothing else does** (owner, 2026-10-20: "так и надо, во всех
//! командах"): `vagcan faults > out.txt` holds the table and the summary, and every note,
//! status line, prompt and spinner is on stderr. So the level never changes stdout — the
//! one invariant the command tests pin.
//!
//! Set once by the binary after it parses its flags ([`set`]); read anywhere. Every write
//! goes through [`crate::progress`]'s screen lock, so a spinner on the terminal is cleared
//! before a line is written and comes back under it on its next tick — a note, a data row
//! and a spinner never share a line.
//!
//! The macros format lazily: `wire!` on every request costs nothing at level 0.

use std::io::Write as _;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// `-v` / `--verbose`, counted: declared once, global on `vagcan`, so it is taken before or
/// after the subcommand.
#[derive(Clone, Copy, Debug, Default, clap::Args)]
pub struct VerboseArg {
	/// More on stderr. -v: where the data came from, why a row is unnamed or skipped,
	/// caveats and timings; -vv: also every request and answer on the bus.
	///
	/// Data stays on stdout and does not change with it.
	//
	// Listed after the command's own flags, with the other globals.
	#[arg(short, long, action = clap::ArgAction::Count, global = true, display_order = 902)]
	pub verbose: u8,
}

/// `-v`: provenance, notes, caveats, timings.
pub const NOTES: u8 = 1;
/// `-vv`: requests, answers and the scheduler's decisions.
pub const WIRE: u8 = 2;

static LEVEL: AtomicU8 = AtomicU8::new(0);

/// Whether a full-screen view owns the terminal: stderr lines would draw over it.
static HELD: AtomicBool = AtomicBool::new(false);

/// Set the process's level, from the counted `-v`. Called once, before the command runs.
pub fn set(level: u8) {
	LEVEL.store(level, Ordering::Relaxed);
}

/// The process's level.
pub fn level() -> u8 {
	LEVEL.load(Ordering::Relaxed)
}

/// Whether lines of `level` are written.
pub fn at(level: u8) -> bool {
	self::level() >= level
}

/// Where a line goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
	Out,
	Err,
}

/// What a test captured instead of the process's stdout and stderr ([`Capture`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Captured {
	pub stdout: String,
	pub stderr: String,
}

/// Where writes go while a [`Capture`] is running: `None` is the real stdout and stderr.
static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// A running capture: what it holds, and whose writes it takes — one thread's, or with
/// `None` every thread's.
struct Sink {
	captured: Captured,
	owner: Option<std::thread::ThreadId>,
}

/// Write `text` to `stream` as it is (no newline added), past any spinner on screen.
///
/// The error is the stream's own — stdout's `BrokenPipe` when what reads it has gone
/// (`watch … | head -3`). A failed stdout write is remembered ([`stdout_gone`]).
fn write(stream: Stream, text: &str) -> std::io::Result<()> {
	let stream = match stream {
		Stream::Out if DATA_ON_STDERR.load(Ordering::Relaxed) => Stream::Err,
		other => other,
	};
	if let Ok(mut sink) = SINK.lock()
		&& let Some(sink) = sink.as_mut()
		&& sink.owner.is_none_or(|owner| owner == std::thread::current().id())
	{
		match stream {
			Stream::Out => sink.captured.stdout.push_str(text),
			Stream::Err => sink.captured.stderr.push_str(text),
		}
		return Ok(());
	}
	let mut result = Ok(());
	crate::progress::above(|| {
		result = match stream {
			Stream::Out => to_stdout(&mut std::io::stdout().lock(), text),
			Stream::Err => {
				let mut err = std::io::stderr().lock();
				err.write_all(text.as_bytes()).and_then(|()| err.flush())
			}
		};
	});
	result
}

/// A data write to `out`, the process's stdout but for a test: its error is returned, and
/// remembered for [`stdout_gone`].
fn to_stdout(out: &mut impl std::io::Write, text: &str) -> std::io::Result<()> {
	let result = out.write_all(text.as_bytes()).and_then(|()| out.flush());
	if result.is_err() {
		STDOUT_GONE.store(true, Ordering::Relaxed);
	}
	result
}

/// Whether `err` is, anywhere in its chain, stdout's reader having gone (`BrokenPipe`): the
/// normal end of `vagcan watch … | head`, said as nothing and ended with success.
pub fn is_broken_pipe(err: &anyhow::Error) -> bool {
	// Through `report::any`: a hint attached to the write's error hides it from `chain()`.
	crate::report::any::<std::io::Error>(err, &|e| e.kind() == std::io::ErrorKind::BrokenPipe)
}

/// Whether a write to stdout has failed — the reader of a pipe went away. A command that
/// streams data for as long as it runs stops on it rather than polling a car for nobody.
pub fn stdout_gone() -> bool {
	STDOUT_GONE.load(Ordering::Relaxed)
}

static STDOUT_GONE: AtomicBool = AtomicBool::new(false);

/// While set, data goes to stderr: what another command prints inside one whose stdout is
/// its own data (`faults` unsealing a catalogue runs `dev vcds rod`). See [`data_on_stderr`].
static DATA_ON_STDERR: AtomicBool = AtomicBool::new(false);

/// Run `f` with every data line it writes sent to stderr, so the stdout of the command
/// around it holds that command's data alone.
pub fn data_on_stderr<R>(f: impl FnOnce() -> R) -> R {
	let was = DATA_ON_STDERR.swap(true, Ordering::Relaxed);
	let result = f();
	DATA_ON_STDERR.store(was, Ordering::Relaxed);
	result
}

/// Data, on stdout: what the command was asked for. Written as given. A failed write is
/// not this caller's to handle — it is remembered for [`stdout_gone`] — but a writer that
/// streams takes [`Stdout`], which returns it.
pub fn out(text: &str) {
	let _ = write(Stream::Out, text);
}

/// Data, on stdout, and a newline.
pub fn outln(text: &str) {
	let mut line = String::with_capacity(text.len() + 1);
	line.push_str(text);
	line.push('\n');
	let _ = write(Stream::Out, &line);
}

/// Stdout as an [`std::io::Write`], through [`out`]'s path: the sink for a writer of data
/// that takes one (a CSV a command streams), so it is the same stdout every other data line
/// goes to — past the spinner, and captured in a test. Its errors are stdout's, so a
/// `BrokenPipe` ends the stream that hit it.
pub struct Stdout;

impl std::io::Write for Stdout {
	fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
		// `fmt` hands a writer whole `str` pieces, so a character is never split here.
		write(Stream::Out, &String::from_utf8_lossy(buf))?;
		Ok(buf.len())
	}

	fn flush(&mut self) -> std::io::Result<()> {
		Ok(())
	}
}

/// A line on stderr at `level`: 0 for what every run says (the adapter used, a refusal, a
/// question), [`NOTES`] and [`WIRE`] for the rest. Dropped while a full-screen view holds
/// the terminal ([`hold`]), unless stderr is not that terminal.
pub fn say(level: u8, text: &str) {
	if !at(level) {
		return;
	}
	if level > 0
		&& let Some(kept) = KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).as_mut()
	{
		kept.push((level, text.to_string()));
		return;
	}
	if HELD.load(Ordering::Relaxed) && level > 0 && std::io::IsTerminal::is_terminal(&std::io::stderr()) {
		return;
	}
	let mut line = String::with_capacity(text.len() + 1);
	line.push_str(text);
	line.push('\n');
	let _ = write(Stream::Err, &line);
}

/// The `-v` and `-vv` lines kept back while a command's step runs ([`keep_details`]).
static KEPT: Mutex<Option<Vec<(u8, String)>>> = Mutex::new(None);

/// From now on keep every `-v` and `-vv` line instead of writing it, until
/// [`kept_details`]: for a command that says a step's details under the step's own line,
/// which it can only print once the step is done. Level-0 lines are written as ever.
pub fn keep_details() {
	*KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Vec::new());
}

/// The lines kept since [`keep_details`], with their levels, and writing as before.
pub fn kept_details() -> Vec<(u8, String)> {
	KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take().unwrap_or_default()
}

/// A question's text on stderr, with no newline: the answer is typed after it.
pub fn ask(text: &str) {
	let _ = write(Stream::Err, text);
}

/// While a full-screen view draws on the terminal, notes and wire lines are not written
/// to it — they would land on top of the view. Writing to a redirected stderr
/// (`2>trace.log`) goes on. Released when the guard drops.
pub fn hold() -> Held {
	HELD.store(true, Ordering::Relaxed);
	Held(())
}

/// The guard [`hold`] returns.
pub struct Held(());

impl Drop for Held {
	fn drop(&mut self) {
		HELD.store(false, Ordering::Relaxed);
	}
}

/// Data on stdout, as `println!` writes it.
#[macro_export]
macro_rules! outln {
	() => { $crate::verbosity::outln("") };
	($($arg:tt)*) => { $crate::verbosity::outln(&format!($($arg)*)) };
}

/// Data on stdout, as `print!` writes it.
#[macro_export]
macro_rules! out {
	($($arg:tt)*) => { $crate::verbosity::out(&format!($($arg)*)) };
}

/// A level-0 line on stderr: what every run says besides its data.
#[macro_export]
macro_rules! say {
	() => { $crate::verbosity::say(0, "") };
	($($arg:tt)*) => { $crate::verbosity::say(0, &format!($($arg)*)) };
}

/// A `-v` line on stderr; not formatted at all below that level.
#[macro_export]
macro_rules! note {
	($($arg:tt)*) => {
		if $crate::verbosity::at($crate::verbosity::NOTES) {
			$crate::verbosity::say($crate::verbosity::NOTES, &format!($($arg)*))
		}
	};
}

/// A `-vv` line on stderr; not formatted at all below that level.
#[macro_export]
macro_rules! wire {
	($($arg:tt)*) => {
		if $crate::verbosity::at($crate::verbosity::WIRE) {
			$crate::verbosity::say($crate::verbosity::WIRE, &format!($($arg)*))
		}
	};
}

/// Bytes on a wire line: `22 F1 87`.
pub fn hex(bytes: &[u8]) -> String {
	let mut text = String::with_capacity(bytes.len() * 3);
	for (at, byte) in bytes.iter().enumerate() {
		if at > 0 {
			text.push(' ');
		}
		text.push_str(&format!("{byte:02X}"));
	}
	text
}

/// Everything written, at `level`, between [`Capture::start`] and [`Capture::finish`] —
/// on this thread, or from any thread with [`Capture::start_all`] — kept instead of
/// written. One at a time in the process: a second start waits for the first to finish.
///
/// The level is the process's, so another test running beside a capture writes at its
/// level too — to the real stderr, never into the capture, unless the capture takes every
/// thread: `start_all` is for a test binary of one test.
#[cfg(any(test, feature = "test-util"))]
pub struct Capture {
	_one: std::sync::MutexGuard<'static, ()>,
	before: u8,
}

#[cfg(any(test, feature = "test-util"))]
static ONE_CAPTURE: Mutex<()> = Mutex::new(());

#[cfg(any(test, feature = "test-util"))]
impl Capture {
	/// What this thread writes.
	pub fn start(level: u8) -> Capture {
		Capture::begin(level, Some(std::thread::current().id()))
	}

	/// What every thread writes: a bus task's `-vv` lines too.
	pub fn start_all(level: u8) -> Capture {
		Capture::begin(level, None)
	}

	fn begin(level: u8, owner: Option<std::thread::ThreadId>) -> Capture {
		let one = ONE_CAPTURE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
		let before = self::level();
		set(level);
		*SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Sink {
			captured: Captured::default(),
			owner,
		});
		Capture { _one: one, before }
	}

	pub fn finish(self) -> Captured {
		SINK
			.lock()
			.unwrap_or_else(|poisoned| poisoned.into_inner())
			.take()
			.map(|sink| sink.captured)
			.unwrap_or_default()
	}
}

/// Ends the capture however the test ends, a panic included: writes go to the real
/// stdout and stderr again, at the level before.
#[cfg(any(test, feature = "test-util"))]
impl Drop for Capture {
	fn drop(&mut self) {
		SINK.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
		set(self.before);
	}
}

/// The split every command keeps, checked over one command run at each level: `runs[n]`
/// is what it wrote at level `n`, from 0 up.
///
/// - **The level never changes stdout**: every run's stdout is the first one's.
/// - **A level only adds**: every line on stderr at one level is on it at the next.
/// - **Data is never on stderr and a note never on stdout**: no non-blank line of stdout
///   is a line of any run's stderr.
///
/// Panics with `what` and the line at fault.
#[cfg(any(test, feature = "test-util"))]
pub fn assert_split(what: &str, runs: &[Captured]) {
	let Some(first) = runs.first() else { return };
	let lines = |text: &str| -> Vec<String> {
		text
			.lines()
			.map(str::trim_end)
			.filter(|l| !l.trim().is_empty())
			.map(str::to_owned)
			.collect()
	};
	for (level, run) in runs.iter().enumerate() {
		assert_eq!(run.stdout, first.stdout, "{what}: stdout at level {level} differs from level 0");
	}
	for pair in runs.windows(2) {
		let more = lines(&pair[1].stderr);
		for line in lines(&pair[0].stderr) {
			assert!(more.contains(&line), "{what}: {line:?} on stderr at one level and not at the next");
		}
	}
	let data = lines(&first.stdout);
	for (level, run) in runs.iter().enumerate() {
		for line in lines(&run.stderr) {
			assert!(!data.contains(&line), "{what}: {line:?} is on stdout and on stderr at level {level}");
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_split_is_checked_line_by_line() {
		let run = |out: &str, err: &str| Captured {
			stdout: out.into(),
			stderr: err.into(),
		};
		assert_split("good", &[run("a\nb\n", "adapter\n"), run("a\nb\n", "adapter\nnote\n")]);
		let refused = |runs: Vec<Captured>| std::panic::catch_unwind(|| assert_split("bad", &runs)).is_err();
		assert!(refused(vec![run("a\n", ""), run("a\nnote\n", "")]), "a note on stdout");
		assert!(refused(vec![run("a\n", "a\n")]), "a data line on stderr");
		assert!(refused(vec![run("a\n", "x\n"), run("a\n", "y\n")]), "a level that took a line away");
	}

	#[test]
	fn data_goes_to_stdout_and_every_level_of_note_to_stderr() {
		let run = |level| {
			let capture = Capture::start(level);
			crate::outln!("unit  id   name");
			crate::say!("adapter  /dev/x");
			crate::note!("names from the project");
			crate::wire!("→ 7E0  22 F1 87");
			capture.finish()
		};
		let quiet = run(0);
		assert_eq!(quiet.stdout, "unit  id   name\n");
		assert_eq!(quiet.stderr, "adapter  /dev/x\n");
		let verbose = run(1);
		assert_eq!(verbose.stdout, quiet.stdout, "the level never changes stdout");
		assert_eq!(verbose.stderr, "adapter  /dev/x\nnames from the project\n");
		let wire = run(2);
		assert_eq!(wire.stdout, quiet.stdout);
		assert!(wire.stderr.ends_with("→ 7E0  22 F1 87\n"), "{wire:?}");
	}

	#[test]
	fn a_closed_pipe_is_an_error_the_streaming_writer_returns() {
		// `watch --did … | head -3` ended on EPIPE and let go of the bus; with the error
		// swallowed it polled the car until --for or Ctrl-C (review of PR #38).
		struct Closed;
		impl std::io::Write for Closed {
			fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
				Err(std::io::ErrorKind::BrokenPipe.into())
			}
			fn flush(&mut self) -> std::io::Result<()> {
				Ok(())
			}
		}
		let was = stdout_gone();
		let said = to_stdout(&mut Closed, "0.1,2,3");
		assert_eq!(said.map_err(|e| e.kind()), Err(std::io::ErrorKind::BrokenPipe));
		assert!(stdout_gone(), "remembered for a loop that writes through outln!");
		STDOUT_GONE.store(was, Ordering::Relaxed);
	}

	#[test]
	fn a_broken_pipe_is_known_through_the_error_chain() {
		let pipe = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::BrokenPipe)).context("writing a row");
		assert!(is_broken_pipe(&pipe));
		assert!(!is_broken_pipe(&anyhow::anyhow!("the link to the car closed")));
		let other = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
		assert!(!is_broken_pipe(&other));
	}

	#[test]
	fn data_inside_another_command_goes_to_stderr() {
		let capture = Capture::start(0);
		data_on_stderr(|| crate::outln!("tag  state"));
		crate::outln!("data");
		let said = capture.finish();
		assert_eq!((said.stdout.as_str(), said.stderr.as_str()), ("data\n", "tag  state\n"));
	}

	#[test]
	fn a_note_below_the_level_is_never_formatted() {
		struct Loud;
		impl std::fmt::Display for Loud {
			fn fmt(&self, _: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				panic!("formatted at level 0")
			}
		}
		let capture = Capture::start(0);
		crate::note!("{}", Loud);
		crate::wire!("{}", Loud);
		assert_eq!(capture.finish(), Captured::default());
	}

	#[test]
	fn bytes_are_spaced_pairs() {
		assert_eq!(hex(&[0x22, 0xF1, 0x87]), "22 F1 87");
		assert_eq!(hex(&[]), "");
	}
}
