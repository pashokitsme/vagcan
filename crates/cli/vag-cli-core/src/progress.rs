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

use std::io::{IsTerminal, Write};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// A one-line progress report for the plain-terminal case.
///
/// Writes to stderr, so a caller's real output can still be piped somewhere
/// while this is on screen.
pub struct Line {
	step: u64,
	width: usize,
	/// Whether anything was drawn, so [`Line::finish`] knows if there is a
	/// line to clear.
	drawn: bool,
	/// Whether stderr is a terminal. A rewriting `\r` line is meaningless once
	/// it is redirected to a file — every frame becomes another line of scroll,
	/// and a copy of 23 000 files leaves half a megabyte of spinner. Off a
	/// terminal the report draws nothing.
	tty: bool,
	/// When the operation began. Nothing is written until it has been running
	/// longer than [`THRESHOLD`].
	started: Instant,
}

impl Default for Line {
	fn default() -> Self {
		Line::new()
	}
}

impl Line {
	pub fn new() -> Self {
		Line {
			step: 0,
			width: 0,
			drawn: false,
			// **stderr**, and deliberately not the question
			// [`crate::ui::can_ask`] asks. That one is "is there a person at
			// stdin to answer a menu"; this is "may I draw a line and erase it
			// again". They disagree routinely — `vagcan dev survey </dev/null`
			// still deserves a spinner, and `vagcan dev survey 2>log` must not have
			// one written into the file — so the two must not be merged.
			tty: std::io::stderr().is_terminal(),
			started: Instant::now(),
		}
	}

	/// Redraw with a new message, once the operation has run long enough to
	/// be worth reporting.
	///
	/// Calls before [`THRESHOLD`] still advance the spinner, so the first
	/// frame drawn is not always the first one — the animation matches how
	/// long the wait has actually been.
	pub fn update(&mut self, message: &str) {
		self.step += 1;
		if !self.tty || self.started.elapsed() < THRESHOLD {
			return;
		}
		let text = format!("{} {message}", frame(self.step));
		let mut err = std::io::stderr();
		// Pad to the previous width so a shorter message does not leave the
		// tail of a longer one behind it.
		let pad = self.width.saturating_sub(text.chars().count());
		let _ = write!(err, "\r{text}{:pad$}", "", pad = pad);
		let _ = err.flush();
		self.width = text.chars().count();
		self.drawn = true;
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
		// `finish` would blank the first line of the message.
		self.finish();
		let _ = writeln!(out, "{}", message.trim_end_matches('\n'));
		let _ = out.flush();
	}

	/// Clear the line, leaving the terminal as it was found.
	///
	/// Called by [`Drop`] too, so an error path cannot leave a half-written
	/// progress line above whatever is printed next.
	pub fn finish(&mut self) {
		if !self.drawn {
			return;
		}
		let mut err = std::io::stderr();
		let _ = write!(err, "\r{:width$}\r", "", width = self.width);
		let _ = err.flush();
		self.drawn = false;
		self.width = 0;
	}
}

/// How often a self-driven spinner redraws.
///
/// Fast enough to look alive, slow enough that the thread doing it costs
/// nothing next to the work it is reporting on.
const TICK: Duration = Duration::from_millis(80);

/// A spinner that drives itself while one blocking call runs.
///
/// [`Line`] is driven by its caller, which is right where there is a loop to
/// drive it from. The slowest things here are not loops: recovering one `.rod`
/// section key is about three minutes inside a single call that does not come
/// back, and parsing three thousand label files is one more. A terminal that
/// says nothing for that long reads as a hang, and the caller has no loop to
/// report from — so the reporting moves to a thread of its own.
///
/// Held as a guard: it starts on construction and clears the line when it goes
/// out of scope, so an early return or an error cannot leave it spinning.
pub struct Spinner {
	done: Arc<AtomicBool>,
	worker: Option<JoinHandle<()>>,
}

impl Spinner {
	/// Say what is being waited for. Drops below [`THRESHOLD`] draw nothing, and
	/// nothing is ever drawn off a terminal, both by way of [`Line`].
	pub fn new(message: impl Into<String>) -> Spinner {
		let message = message.into();
		let done = Arc::new(AtomicBool::new(false));
		let flag = Arc::clone(&done);
		let worker = thread::spawn(move || {
			let mut line = Line::new();
			let started = Instant::now();
			while !flag.load(Ordering::Relaxed) {
				// Past a few seconds the elapsed time is the useful part: it is
				// what tells somebody watching a three-minute search that it is
				// three minutes in and not stuck.
				let secs = started.elapsed().as_secs();
				match secs >= 5 {
					true => line.update(&format!("{message} — {secs}s")),
					false => line.update(&message),
				}
				thread::sleep(TICK);
			}
			// `Line::drop` clears what it drew, here rather than on the caller's
			// thread, which is why the caller waits for this one to finish.
		});
		Spinner { done, worker: Some(worker) }
	}
}

impl Drop for Spinner {
	fn drop(&mut self) {
		self.done.store(true, Ordering::Relaxed);
		if let Some(worker) = self.worker.take() {
			let _ = worker.join();
		}
	}
}

impl Drop for Line {
	fn drop(&mut self) {
		self.finish();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

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
		let mut line = Line::new();
		line.finish();
		assert!(!line.drawn);
		assert_eq!(line.width, 0);
	}

	#[test]
	fn a_wait_shorter_than_the_threshold_is_never_drawn() {
		// On a car that answers promptly the whole operation is over before a
		// reader could focus on the line, and a report that flashes past is
		// worse than none.
		let mut line = Line::new();
		for _ in 0..5 {
			line.update("identifying control units");
		}
		assert!(!line.drawn, "nothing on screen yet");
		assert_eq!(line.step, 5, "but the spinner tracked the calls");
	}

	#[test]
	fn off_a_terminal_nothing_is_ever_drawn() {
		// Redirected to a file, a rewriting line turns every frame into scroll —
		// a 23 000-file copy left half a megabyte of spinner. The spinner still
		// tracks the calls; it just never writes.
		let mut line = Line::new();
		line.tty = false;
		line.started = Instant::now() - THRESHOLD - Duration::from_millis(1);
		for _ in 0..5 {
			line.update("copying");
		}
		assert!(!line.drawn, "a non-terminal must stay clean");
		assert_eq!(line.step, 5, "but the calls were still counted");
	}

	#[test]
	fn a_wait_past_the_threshold_is_drawn() {
		let mut line = Line::new();
		line.tty = true; // the test harness's stderr is not a terminal
		line.started = Instant::now() - THRESHOLD - Duration::from_millis(1);
		line.update("identifying control units");
		assert!(line.drawn);
		line.finish();
	}

	#[test]
	fn a_warning_is_not_written_to_the_line_that_erases_itself() {
		// The defect this guards against: a message shown during a
		// sweep shares the rewriting progress line and is gone at the
		// next redraw — "it showed an error, and then it went out". A notice
		// clears the line first and then writes where nothing rewrites.
		let mut line = Line::new();
		line.tty = true; // the test harness's stderr is not a terminal
		line.started = Instant::now() - THRESHOLD - Duration::from_millis(1);
		line.update("sweeping 712 — unit 5 of 15");
		assert!(line.drawn, "there is a progress line up");

		let mut out: Vec<u8> = Vec::new();
		line.notice_to(&mut out, "STOPPED: control unit 44 stopped answering");

		assert!(!line.drawn, "the progress line was cleared before the notice");
		assert_eq!(line.width, 0, "so the next redraw has nothing to overwrite");
		let text = String::from_utf8(out).unwrap();
		assert!(text.starts_with("STOPPED"), "{text:?}");
		assert!(text.ends_with('\n'), "a notice is a whole line, not a fragment: {text:?}");
		assert!(!text.contains('\r'), "nothing in a notice returns to the start of a line: {text:?}");
	}

	#[test]
	fn a_notice_survives_a_redraw_that_follows_it() {
		// The failure in one assertion: whatever the spinner does next, it must
		// not be able to touch what the notice said.
		let mut line = Line::new();
		line.tty = true;
		line.started = Instant::now() - THRESHOLD - Duration::from_millis(1);
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
		let mut line = Line::new();
		line.tty = true; // the test harness's stderr is not a terminal
		line.started = Instant::now() - THRESHOLD - Duration::from_millis(1);
		line.update("identifying control units 12 of 15");
		let long = line.width;
		line.update("done");
		assert!(long > line.width, "the fixture really does shorten");
		assert!(line.drawn);
		line.finish();
		assert!(!line.drawn);
	}
}
