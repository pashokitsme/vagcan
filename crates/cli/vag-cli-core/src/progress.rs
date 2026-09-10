//! Saying what the tool is waiting for.
//!
//! Reading a car is slow in places that look like nothing happening: finding
//! which control units exist means asking fifteen addresses in turn, and a
//! unit that answers takes a moment while one that is absent takes its whole
//! deadline. Several seconds of a blank terminal reads as a hang.
//!
//! Two shapes, because there are two situations. Before the full-screen view
//! opens there is only a terminal, so progress goes on one line that rewrites
//! itself and then gets out of the way. Inside the view it belongs in the
//! footer beside the keys, which is where a reader's eye already is.
//!
//! Neither is decoration: both say **what** is being waited for, because
//! "working…" and a blank screen carry the same information.
//!
//! ## The line drives itself
//!
//! [`Line`] used to redraw only when its caller called [`Line::update`], so the
//! spinner glyph advanced once per unit of work and froze in between — a
//! variant that took a second to walk showed a second of a still frame, which
//! is the hang the widget exists to deny. Now one thread ticks every
//! [`TICK`] and redraws whatever message was last set, and `update` only swaps
//! the message. The caller's loop still supplies the counter text; the
//! animation no longer depends on how often it does.
//!
//! That is also what makes the line usable from a parallel loop: a
//! [`Reporter`] is a cloneable handle onto the same message, so a worker on
//! any thread can say what it just finished, and the one ticker draws it.

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// How long a wait has to last before it is worth reporting.
///
/// Below this the report is the only thing anybody sees: a line that appears
/// and vanishes in a tenth of a second is noise, and on a car that answers
/// promptly the whole operation is over before a reader could focus on it.
/// Above it, silence is the thing that misleads.
pub const THRESHOLD: Duration = Duration::from_millis(500);

/// The frames of the spinner, in order.
///
/// Braille dots rather than `|/-\` — they animate in place without the width
/// changing, so the text after them does not jitter.
const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Pick the frame for a step. Exposed so the full-screen view can spin the
/// same way without owning a line of its own.
pub fn frame(step: u64) -> char {
	FRAMES[(step as usize) % FRAMES.len()]
}

/// How often a self-driven line redraws.
///
/// Fast enough to look alive, slow enough that the thread doing it costs
/// nothing next to the work it is reporting on.
const TICK: Duration = Duration::from_millis(80);

/// Past this the elapsed time joins the message on a [`Spinner`]: it is what
/// tells somebody watching a three-minute search that it is three minutes in
/// and not stuck.
const SHOW_ELAPSED_AFTER: Duration = Duration::from_secs(5);

/// What the line is showing, and what it has drawn — everything the ticker
/// and the callers share, under one lock.
///
/// The drawing itself happens under that lock too, which is what keeps a
/// redraw from the ticker and an `update` from the caller from interleaving
/// their bytes on the same terminal line.
#[derive(Debug, Default)]
struct State {
	/// The message last set. Empty until the first `update`.
	message: String,
	/// Which frame the spinner is on. Advances on every draw and every update,
	/// so the animation matches how long the wait has actually been.
	step: u64,
	/// How many columns the last draw covered, so a shorter message can pad
	/// over the tail of a longer one.
	width: usize,
	/// Whether anything is on screen, so `finish` knows if there is a line
	/// to clear.
	drawn: bool,
	/// Whether the ticker may draw. Off until the first `update`, off again
	/// after `finish`: a line that was cleared must stay cleared until the
	/// caller has something new to say, or a notice written under it would be
	/// overdrawn on the next tick.
	active: bool,
}

/// What the ticker and every [`Reporter`] hold in common.
#[derive(Debug)]
struct Shared {
	state: Mutex<State>,
	/// Whether stderr is a terminal. A rewriting `\r` line is meaningless once
	/// it is redirected to a file — every frame becomes another line of scroll,
	/// and a copy of 23 000 files leaves half a megabyte of spinner. Off a
	/// terminal the report draws nothing.
	tty: bool,
	/// When the operation began. Nothing is written until it has been running
	/// longer than [`THRESHOLD`].
	started: Instant,
	/// Whether to append the elapsed time once the wait is long — what a
	/// [`Spinner`] wants and a counter line does not.
	elapsed: bool,
	/// Tells the ticker to stop.
	stop: AtomicBool,
}

impl Shared {
	/// Whether drawing is allowed at all right now: on a terminal, and past
	/// the threshold.
	fn may_draw(&self) -> bool {
		self.tty && self.started.elapsed() >= THRESHOLD
	}

	/// Redraw the current message on stderr, if drawing is allowed and the
	/// line is active. Advances the spinner either way, so the first frame
	/// drawn is not always the first one.
	fn draw(&self, state: &mut State) {
		state.step += 1;
		if !state.active || !self.may_draw() {
			return;
		}
		let secs = self.started.elapsed().as_secs();
		let text = match self.elapsed && self.started.elapsed() >= SHOW_ELAPSED_AFTER {
			true => format!("{} {} — {secs}s", frame(state.step), state.message),
			false => format!("{} {}", frame(state.step), state.message),
		};
		let mut err = std::io::stderr();
		// Pad to the previous width so a shorter message does not leave the
		// tail of a longer one behind it.
		let pad = state.width.saturating_sub(text.chars().count());
		let _ = write!(err, "\r{text}{:pad$}", "", pad = pad);
		let _ = err.flush();
		state.width = text.chars().count();
		state.drawn = true;
	}

	/// Clear the line, leaving the terminal as it was found, and stop drawing
	/// until the next message.
	fn finish(&self, state: &mut State) {
		state.active = false;
		if !state.drawn {
			return;
		}
		let mut err = std::io::stderr();
		let _ = write!(err, "\r{:width$}\r", "", width = state.width);
		let _ = err.flush();
		state.drawn = false;
		state.width = 0;
	}
}

/// A one-line progress report for the plain-terminal case.
///
/// Writes to stderr, so a caller's real output can still be piped somewhere
/// while this is on screen.
///
/// Owns the ticker thread: it starts on construction and is joined on drop,
/// after which the line it drew is cleared — so an early return or an error
/// cannot leave it spinning.
pub struct Line {
	reporter: Reporter,
	worker: Option<JoinHandle<()>>,
}

/// A handle onto a [`Line`]'s message that any thread can hold.
///
/// Cloned off a line and handed to workers, so a loop that fans out across
/// threads can still say which item it just finished. Setting a message
/// through a reporter is exactly [`Line::update`].
#[derive(Clone, Debug)]
pub struct Reporter {
	shared: Arc<Shared>,
}

impl Default for Line {
	fn default() -> Self {
		Line::new()
	}
}

impl Line {
	pub fn new() -> Self {
		// **stderr**, and deliberately not the question
		// [`crate::ui::can_ask`] asks. That one is "is there a person at
		// stdin to answer a menu"; this is "may I draw a line and erase it
		// again". They disagree routinely — `vagcan dev survey </dev/null`
		// still deserves a spinner, and `vagcan dev survey 2>log` must not have
		// one written into the file — so the two must not be merged.
		Line::with(std::io::stderr().is_terminal(), false)
	}

	/// Build a line, choosing whether it may draw and whether it shows the
	/// elapsed time. The ticker is only started where it could ever draw.
	fn with(tty: bool, elapsed: bool) -> Line {
		let shared = Arc::new(Shared {
			state: Mutex::new(State::default()),
			tty,
			started: Instant::now(),
			elapsed,
			stop: AtomicBool::new(false),
		});
		let worker = tty.then(|| {
			let shared = Arc::clone(&shared);
			thread::spawn(move || {
				while !shared.stop.load(Ordering::Relaxed) {
					thread::sleep(TICK);
					if let Ok(mut state) = shared.state.lock() {
						shared.draw(&mut state);
					}
				}
			})
		});
		Line {
			reporter: Reporter { shared },
			worker,
		}
	}

	/// A handle any thread can set the message through.
	pub fn reporter(&self) -> Reporter {
		self.reporter.clone()
	}

	/// Redraw with a new message, once the operation has run long enough to
	/// be worth reporting.
	///
	/// Calls before [`THRESHOLD`] still advance the spinner, so the first
	/// frame drawn is not always the first one — the animation matches how
	/// long the wait has actually been.
	pub fn update(&mut self, message: &str) {
		self.reporter.set(message);
	}

	/// Say something that must still be on screen a minute later.
	///
	/// **A safety message must never be written to a surface that erases
	/// itself.** This one does: [`update`](Line::update) returns to the start of
	/// the line with `\r` and pads over whatever was there, and
	/// [`finish`](Line::finish) blanks it outright. A warning that goes up
	/// during a sweep and then goes out again is how a run that has already
	/// provoked a control unit carries on regardless.
	///
	/// So the progress line is cleared **first**, and the message is written
	/// whole and newline-terminated onto the line after it, where the next
	/// redraw cannot reach back. Nothing about this is decoration: it is the
	/// difference between a warning and a flicker.
	pub fn notice(&mut self, message: &str) {
		self.notice_to(&mut std::io::stderr(), message);
	}

	/// The same, against any sink, so the ordering can be tested without a
	/// terminal.
	fn notice_to(&mut self, out: &mut impl Write, message: &str) {
		// Clearing first is the whole invariant. Written the other way round,
		// `finish` would blank the first line of the message. And the line
		// stays inactive until the next `update`, so the ticker cannot draw
		// over the notice in between.
		self.finish();
		let _ = writeln!(out, "{}", message.trim_end_matches('\n'));
		let _ = out.flush();
	}

	/// Clear the line, leaving the terminal as it was found.
	///
	/// Called by [`Drop`] too, so an error path cannot leave a half-written
	/// progress line above whatever is printed next.
	pub fn finish(&mut self) {
		if let Ok(mut state) = self.reporter.shared.state.lock() {
			self.reporter.shared.finish(&mut state);
		}
	}

	/// A look at the state, for the tests.
	#[cfg(test)]
	fn state(&self) -> std::sync::MutexGuard<'_, State> {
		self.reporter.shared.state.lock().expect("the state lock is not poisoned")
	}
}

impl Reporter {
	/// Set the message and redraw at once, when drawing is allowed.
	pub fn set(&self, message: &str) {
		if let Ok(mut state) = self.shared.state.lock() {
			state.message.clear();
			state.message.push_str(message);
			state.active = true;
			self.shared.draw(&mut state);
		}
	}
}

impl Drop for Line {
	fn drop(&mut self) {
		self.reporter.shared.stop.store(true, Ordering::Relaxed);
		if let Some(worker) = self.worker.take() {
			let _ = worker.join();
		}
		// After the ticker is gone, so nothing can redraw what this clears.
		self.finish();
	}
}

/// A spinner for one blocking call that does not come back.
///
/// [`Line`] is fed by its caller, which is right where there is a loop to feed
/// it from. The slowest things here are not loops: recovering one `.rod`
/// section key is about three minutes inside a single call, and parsing three
/// thousand label files is one more. This is a [`Line`] with its message set
/// once, left to tick, and — past a few seconds — the elapsed time appended,
/// because that is what tells somebody watching a three-minute search that it
/// is three minutes in and not stuck.
///
/// Held as a guard: it starts on construction and clears the line when it goes
/// out of scope, so an early return or an error cannot leave it spinning.
pub struct Spinner {
	_line: Line,
}

impl Spinner {
	/// Say what is being waited for. Drops below [`THRESHOLD`] draw nothing, and
	/// nothing is ever drawn off a terminal, both by way of [`Line`].
	pub fn new(message: impl Into<String>) -> Spinner {
		let mut line = Line::with(std::io::stderr().is_terminal(), true);
		line.update(&message.into());
		Spinner { _line: line }
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A line that behaves as if on a terminal, past the threshold, but with
	/// no ticker thread — so what is on screen is exactly what the calls put
	/// there and a test can reason about it.
	///
	/// `tty` is what would start the ticker, so it is built off and switched
	/// on afterwards; `started` is moved back so the threshold has passed.
	fn drawing_line() -> Line {
		on_terminal(Line::with(false, false))
	}

	/// Rebuild a line's shared state as a terminal past the threshold.
	fn on_terminal(line: Line) -> Line {
		let state = std::mem::take(&mut *line.state());
		let shared = Arc::new(Shared {
			state: Mutex::new(state),
			tty: true,
			started: Instant::now() - THRESHOLD - Duration::from_millis(1),
			elapsed: line.reporter.shared.elapsed,
			stop: AtomicBool::new(false),
		});
		Line {
			reporter: Reporter { shared },
			worker: None,
		}
	}

	#[test]
	fn the_spinner_advances_and_wraps() {
		assert_eq!(frame(0), FRAMES[0]);
		assert_eq!(frame(1), FRAMES[1]);
		assert_eq!(frame(FRAMES.len() as u64), FRAMES[0], "it comes back round");
	}

	#[test]
	fn a_line_that_drew_nothing_has_nothing_to_clear() {
		// `finish` runs on drop, including on an error path that never
		// reported progress; clearing then would erase somebody else's line.
		let mut line = Line::with(false, false);
		line.finish();
		assert!(!line.state().drawn);
		assert_eq!(line.state().width, 0);
	}

	#[test]
	fn a_wait_shorter_than_the_threshold_is_never_drawn() {
		// On a car that answers promptly the whole operation is over before a
		// reader could focus on the line, and a report that flashes past is
		// worse than none.
		let mut line = Line::with(true, false);
		for _ in 0..5 {
			line.update("identifying control units");
		}
		// Five updates plus however many ticks the thread managed; the ticks
		// count too, since each is a frame the animation would have shown.
		assert!(!line.state().drawn, "nothing on screen yet");
		assert!(line.state().step >= 5, "but the spinner tracked the calls");
	}

	#[test]
	fn off_a_terminal_nothing_is_ever_drawn() {
		// Redirected to a file, a rewriting line turns every frame into scroll —
		// a 23 000-file copy left half a megabyte of spinner. The spinner still
		// tracks the calls; it just never writes. And no ticker is started at
		// all: there is nothing for it to draw on.
		let mut line = Line::with(false, false);
		assert!(line.worker.is_none(), "a non-terminal starts no ticker");
		for _ in 0..5 {
			line.update("copying");
		}
		assert!(!line.state().drawn, "a non-terminal must stay clean");
		assert_eq!(line.state().step, 5, "but the calls were still counted");
	}

	#[test]
	fn a_wait_past_the_threshold_is_drawn() {
		let mut line = drawing_line();
		line.update("identifying control units");
		assert!(line.state().drawn);
		line.finish();
	}

	#[test]
	fn a_tick_redraws_without_a_new_message() {
		// The defect this version exists for: the glyph froze between calls
		// to `update`, so one slow item looked like a hang. A tick with no new
		// message still advances the frame and redraws.
		let mut line = drawing_line();
		line.update("312 of 717 — EV_ECM18TFS0208V0906264H");
		// Two `state()` calls in one expression would hold the lock twice and
		// wait on themselves; take the guard once.
		let (step, width) = {
			let state = line.state();
			(state.step, state.width)
		};
		{
			let mut state = line.state();
			line.reporter.shared.draw(&mut state);
		}
		assert_eq!(line.state().step, step + 1, "the frame advanced");
		assert_eq!(line.state().width, width, "and the same message was redrawn");
		assert!(line.state().drawn);
		line.finish();
	}

	#[test]
	fn a_tick_after_finish_draws_nothing_until_the_next_message() {
		// `finish` is what a notice relies on: the line is cleared, the notice
		// is written under it, and the ticker must not come back and draw over
		// the notice. It may draw again once the caller has said something new.
		let mut line = drawing_line();
		line.update("sweeping");
		line.finish();
		{
			let mut state = line.state();
			line.reporter.shared.draw(&mut state);
		}
		assert!(!line.state().drawn, "a cleared line stays cleared on a tick");
		line.update("sweeping 713");
		assert!(line.state().drawn, "and comes back on the next message");
		line.finish();
	}

	#[test]
	fn a_reporter_sets_the_same_message_the_line_shows() {
		// What a parallel loop holds: a handle per worker, all onto one line.
		let mut line = drawing_line();
		let reporter = line.reporter();
		let other = std::thread::spawn(move || reporter.set("from another thread"));
		other.join().expect("the worker set its message");
		assert_eq!(line.state().message, "from another thread");
		assert!(line.state().drawn);
		line.finish();
	}

	#[test]
	fn a_warning_is_not_written_to_the_line_that_erases_itself() {
		// The defect this guards against: a message shown during a
		// sweep shares the rewriting progress line and is gone at the
		// next redraw — "it showed an error, and then it went out". A notice
		// clears the line first and then writes where nothing rewrites.
		let mut line = drawing_line();
		line.update("sweeping 712 — unit 5 of 15");
		assert!(line.state().drawn, "there is a progress line up");

		let mut out: Vec<u8> = Vec::new();
		line.notice_to(&mut out, "STOPPED: control unit 44 stopped answering");

		assert!(!line.state().drawn, "the progress line was cleared before the notice");
		assert_eq!(line.state().width, 0, "so the next redraw has nothing to overwrite");
		assert!(!line.state().active, "and the ticker is held off until the next message");
		let text = String::from_utf8(out).unwrap();
		assert!(text.starts_with("STOPPED"), "{text:?}");
		assert!(text.ends_with('\n'), "a notice is a whole line, not a fragment: {text:?}");
		assert!(!text.contains('\r'), "nothing in a notice returns to the start of a line: {text:?}");
	}

	#[test]
	fn a_notice_survives_a_redraw_that_follows_it() {
		// The failure in one assertion: whatever the spinner does next, it must
		// not be able to touch what the notice said.
		let mut line = drawing_line();
		let mut out: Vec<u8> = Vec::new();
		line.notice_to(&mut out, "STOPPED");
		let after_notice = out.len();
		// The next redraw goes to stderr, not here — but the invariant that
		// makes that safe is the width, and it is zero.
		line.update("sweeping 713");
		assert_eq!(out.len(), after_notice, "the redraw wrote nothing over the notice");
		line.finish();
	}

	#[test]
	fn the_line_remembers_how_much_it_must_overwrite() {
		// A short message after a long one must cover the tail of the long
		// one, or the screen keeps characters nobody wrote.
		let mut line = drawing_line();
		line.update("identifying control units 12 of 15");
		let long = line.state().width;
		line.update("done");
		assert!(long > line.state().width, "the fixture really does shorten");
		assert!(line.state().drawn);
		line.finish();
		assert!(!line.state().drawn);
	}

	#[test]
	fn a_spinner_is_a_line_that_shows_how_long_it_has_been() {
		// Past a few seconds the elapsed time is the useful part of a spinner:
		// it is what says a three-minute search is three minutes in, not stuck.
		let spinner = Spinner::new("parsing the label files");
		assert!(spinner._line.reporter.shared.elapsed, "a spinner asks for the elapsed time");
		assert_eq!(spinner._line.state().message, "parsing the label files");
	}
}
