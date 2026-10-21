//! Switching the terminal over to draw on, and the promise to switch it back.
//!
//! Four commands take the terminal away from the shell for a while: the picker
//! draws a list where the cursor already is, `measure` and `watch` take a screen
//! of their own, and `watch` also wants the mouse. Each of them used to switch
//! what it needed on by hand and switch it back on the way out, which works for
//! exactly one path through the code — the one that returns `Ok`. A `?` in
//! between (a serial write that failed, a file that would not open) skipped the
//! restore and left a shell with no echo and no cursor, which reads to the
//! person at the keyboard as a tool that broke their terminal. On `measure` that
//! happens in the middle of a drive.
//!
//! So the restore hangs off `Drop`, the way [`picker`](crate::ui::picker) already
//! did it: `?` unwinds through this, and so does a panic.
//!
//! **Nothing is put back that was not switched on.** A [`Guard`] holds the list
//! of switches it actually got on, appended to as each one succeeds, and `Drop`
//! walks that list backwards. There is no way to build one that restores a
//! switch it never entered, and a failure part-way through entering leaves the
//! ones already on to be undone and the rest untouched.
//!
//! **The terminal is behind [`Terminal`]** for the reason `picker` puts its
//! input behind `Chooser`: the part worth testing — that everything comes back
//! off, in the reverse order, through an error and through a panic — is the part
//! a real terminal makes untestable, and `cargo test` has no terminal to switch.
//!
//! What to say when the switch fails is the *caller's*, not this module's. A
//! command that cannot have a screen has something else to offer instead —
//! `measure` prints a line per cycle, `watch` has `--for SECONDS` — and that
//! sentence is the useful half of the failure.

use std::sync::atomic::{AtomicU8, Ordering};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::{cursor, event, execute, terminal};

/// One thing switched on for as long as a command draws, and switched back off
/// after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
	/// Single keys instead of lines: no echo, no line buffering. Leaving this
	/// on is what makes a shell look broken.
	Raw,
	/// The cursor out of the way of something drawn over it.
	Cursor,
	/// A screen of its own, so whatever the shell had on it comes back after.
	Alternate,
	/// Clicks and scrolls as events, instead of as the terminal's own selection.
	Mouse,
}

/// The terminal, as a guard needs it: switches that go on and come back off.
///
/// Two methods rather than one per switch, so that `Drop` can undo a list
/// without knowing what is in it.
pub trait Terminal {
	fn on(&mut self, switch: Switch) -> Result<()>;
	fn off(&mut self, switch: Switch) -> Result<()>;
}

/// The terminal a person is sitting at. The cursor is switched on the stream that draws:
/// stderr for the in-place menus ([`in_place`]), stdout for a full screen — so a log a
/// stream is redirected to gets no cursor escapes from the other one.
pub struct Crossterm {
	cursor_on_stderr: bool,
}

impl Crossterm {
	fn cursor(&self, command: impl crossterm::Command) -> Result<()> {
		match self.cursor_on_stderr {
			true => execute!(std::io::stderr(), command)?,
			false => execute!(std::io::stdout(), command)?,
		}
		Ok(())
	}
}

impl Terminal for Crossterm {
	fn on(&mut self, switch: Switch) -> Result<()> {
		// Once a Ctrl-C is ending the process, the terminal has been or is being given back
		// from the handler's thread: a screen that starts now would outlive it.
		if crate::interrupt::ending() {
			return Err(crate::interrupt::Interrupted.into());
		}
		let mut out = std::io::stdout();
		match switch {
			Switch::Raw => terminal::enable_raw_mode()?,
			Switch::Cursor => self.cursor(cursor::Hide)?,
			Switch::Alternate => execute!(out, terminal::EnterAlternateScreen)?,
			Switch::Mouse => execute!(out, event::EnableMouseCapture)?,
		}
		ON.fetch_or(bit(switch, self.cursor_on_stderr), Ordering::SeqCst);
		// One that raced in — on before the flag, listed after the give-back read the list —
		// is undone here. Listed first and checked second, so one of the two always sees it.
		if crate::interrupt::ending() {
			let _ = self.off(switch);
			return Err(crate::interrupt::Interrupted.into());
		}
		Ok(())
	}

	fn off(&mut self, switch: Switch) -> Result<()> {
		// Taken off the list first, and switched off only by whoever took it: a Ctrl-C's
		// [`give_back`] on its own thread and this guard's `Drop` may both get here.
		let bit = bit(switch, self.cursor_on_stderr);
		if ON.fetch_and(!bit, Ordering::SeqCst) & bit == 0 {
			return Ok(());
		}
		let mut out = std::io::stdout();
		match switch {
			Switch::Raw => terminal::disable_raw_mode()?,
			Switch::Cursor => self.cursor(cursor::Show)?,
			Switch::Alternate => execute!(out, terminal::LeaveAlternateScreen)?,
			Switch::Mouse => execute!(out, event::DisableMouseCapture)?,
		}
		Ok(())
	}
}

/// What is switched on on the real terminal right now, one bit per switch (the cursor
/// once per stream), for [`give_back`]: a Ctrl-C ends the process from another thread,
/// and no guard's `Drop` runs then.
static ON: AtomicU8 = AtomicU8::new(0);

fn bit(switch: Switch, cursor_on_stderr: bool) -> u8 {
	match switch {
		Switch::Raw => 1,
		Switch::Cursor if cursor_on_stderr => 2,
		Switch::Cursor => 4,
		Switch::Alternate => 8,
		Switch::Mouse => 16,
	}
}

/// Raw mode alone switched off, if it is on: line mode back without writing a byte, so a
/// terminal that has stopped taking bytes cannot hold it up. What the handler does first
/// on its way to the exit, before [`give_back`] writes its escapes.
pub fn give_back_raw() {
	let _ = Crossterm { cursor_on_stderr: false }.off(Switch::Raw);
}

/// Everything still switched on, switched off, in the order a guard would: for a
/// Ctrl-C, which ends the process from its own thread while a menu or a screen may be
/// up ([`crate::interrupt`]). The cursor comes back on a full screen too, which
/// ratatui hid and would have shown again on its own way out.
pub fn give_back() {
	let had_screen = ON.load(Ordering::SeqCst) & bit(Switch::Alternate, false) != 0;
	for (switch, cursor_on_stderr) in [
		(Switch::Mouse, false),
		(Switch::Alternate, false),
		(Switch::Cursor, false),
		(Switch::Cursor, true),
		(Switch::Raw, false),
	] {
		let _ = Crossterm { cursor_on_stderr }.off(switch);
	}
	if had_screen {
		let _ = execute!(std::io::stdout(), cursor::Show);
	}
}

/// Whether a key is Ctrl-C. Raw mode turns the terminal's own interrupt off, so in a
/// menu or a full screen the keystroke that ends every other command arrives as `c`
/// with Control held; every raw-mode loop asks this first, or there is no Ctrl-C there.
/// A menu hands it to [`crate::interrupt::raise`] at once; a full screen quits the way
/// its own quit key does, so what it was recording is closed properly, and raises it
/// after — the run ends with 130 either way.
pub fn interrupts(key: &KeyEvent) -> bool {
	key.kind != KeyEventKind::Release && key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c' | 'C'))
}

/// What a command needs the terminal to be while it draws.
///
/// Built rather than named, because the three shapes in this crate are three
/// different terminals and a single one with flags would be a shape nobody
/// asked for. The order things are listed in is the order they are switched on,
/// and therefore the reverse of the order they come back off.
#[derive(Clone, Debug)]
pub struct Wanted {
	switches: Vec<Switch>,
	/// Whether what draws is on stderr — the in-place menus — rather than stdout.
	on_stderr: bool,
}

/// A list drawn where the cursor already is — raw keys and no cursor, and
/// **no** alternate screen.
///
/// The lines a picker prints while it works (what was taken, what was deleted)
/// belong to the shell it was started from and should still be there
/// afterwards, so it deliberately leaves its output where the next line goes.
///
/// Raw mode first and the cursor second: the guard exists as soon as the first
/// one is on, so a failure hiding the cursor still hands raw mode back.
pub fn in_place() -> Wanted {
	Wanted {
		switches: vec![Switch::Raw, Switch::Cursor],
		on_stderr: true,
	}
}

/// A screen of its own — raw keys on the alternate screen.
///
/// The cursor is not in the list, and that is deliberate: the drawing here is
/// ratatui's, `Terminal::draw` hides the cursor itself on any frame that does
/// not place one, and ratatui's own `Drop` shows it again. Hiding it a second
/// time here would mean showing it on a terminal this guard never hid it on.
pub fn full_screen() -> Wanted {
	Wanted {
		switches: vec![Switch::Raw, Switch::Alternate],
		on_stderr: false,
	}
}

impl Wanted {
	/// Clicks and scrolls as well as keys.
	#[must_use]
	pub fn with_mouse(mut self) -> Wanted {
		self.switches.push(Switch::Mouse);
		self
	}

	/// The cursor hidden too, for a screen not drawn through ratatui — which hides and
	/// shows it itself, and is why [`full_screen`] leaves it alone.
	#[must_use]
	pub fn hiding_cursor(mut self) -> Wanted {
		self.switches.push(Switch::Cursor);
		self
	}

	/// Switch it all on, and hand back the promise to switch it back off.
	///
	/// The error is crossterm's, unwrapped: what to tell somebody with no
	/// terminal depends on what the command would have done with one, so the
	/// sentence is added by the caller.
	pub fn enter(self) -> Result<Guard<Crossterm>> {
		let terminal = Crossterm {
			cursor_on_stderr: self.on_stderr,
		};
		self.enter_on(terminal)
	}

	/// The same, against something that is not a terminal.
	fn enter_on<T: Terminal>(self, terminal: T) -> Result<Guard<T>> {
		// The guard owns the list before anything is on it, so that a `?` below
		// unwinds through a `Drop` that knows how far it got.
		let mut guard = Guard {
			terminal,
			entered: Vec::new(),
		};
		for switch in self.switches {
			guard.terminal.on(switch)?;
			guard.entered.push(switch);
		}
		Ok(guard)
	}
}

/// The terminal, switched over, and the promise to give it back.
///
/// Held in a binding that lives as long as the drawing does — `let _screen =`,
/// not `let _ =`, which drops it on the spot.
pub struct Guard<T: Terminal = Crossterm> {
	terminal: T,
	/// Every switch that actually went on, in the order it did.
	entered: Vec<Switch>,
}

impl<T: Terminal> Drop for Guard<T> {
	fn drop(&mut self) {
		// Backwards: the alternate screen goes away before raw mode does, so
		// the shell's own screen comes back in the state it was left in. And
		// nothing here can be reported — this runs while an error or a panic is
		// already on its way out, and it must not replace it.
		while let Some(switch) = self.entered.pop() {
			let _ = self.terminal.off(switch);
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::sync::{Arc, Mutex};

	#[test]
	fn ctrl_c_is_an_interrupt_and_nothing_else_is() {
		use crossterm::event::KeyEventState;
		let key = |code, modifiers, kind| KeyEvent {
			code,
			modifiers,
			kind,
			state: KeyEventState::NONE,
		};
		let ctrl = KeyModifiers::CONTROL;
		// What a terminal in raw mode sends for Ctrl-C (0x03), and with Shift held or
		// Caps Lock on; a held key repeats.
		assert!(interrupts(&key(KeyCode::Char('c'), ctrl, KeyEventKind::Press)));
		assert!(interrupts(&key(KeyCode::Char('C'), ctrl | KeyModifiers::SHIFT, KeyEventKind::Press)));
		assert!(interrupts(&key(KeyCode::Char('c'), ctrl, KeyEventKind::Repeat)));
		// The release Windows sends after each press is not a second Ctrl-C.
		assert!(!interrupts(&key(KeyCode::Char('c'), ctrl, KeyEventKind::Release)));
		// `c` alone is a command's own key: `watch` opens its channel list on it,
		// `measure` cancels a run.
		assert!(!interrupts(&key(KeyCode::Char('c'), KeyModifiers::NONE, KeyEventKind::Press)));
		assert!(!interrupts(&key(KeyCode::Char('c'), KeyModifiers::ALT, KeyEventKind::Press)));
		assert!(!interrupts(&key(KeyCode::Char('d'), ctrl, KeyEventKind::Press)));
		assert!(!interrupts(&key(KeyCode::Esc, ctrl, KeyEventKind::Press)));
	}

	#[test]
	fn the_cursor_is_switched_on_the_stream_that_draws() {
		// A menu draws on stderr, a full screen on stdout: a cursor escape on the other
		// stream lands in whatever file that one is redirected to (review of PR #38).
		assert!(in_place().on_stderr);
		assert!(!full_screen().on_stderr);
		assert!(!full_screen().hiding_cursor().on_stderr);
	}

	/// What a terminal was told to do, in order.
	#[derive(Clone, Copy, Debug, PartialEq, Eq)]
	enum Step {
		On(Switch),
		Off(Switch),
	}

	/// The terminal the tests use: everything it was told comes back out to be
	/// asserted against, and it can be told to refuse one switch.
	///
	/// The log is shared rather than owned because a guard swallows its
	/// terminal, and what a guard did on the way out is the whole question.
	struct Recording {
		steps: Arc<Mutex<Vec<Step>>>,
		refuses: Option<Switch>,
	}

	impl Recording {
		fn new() -> (Recording, Arc<Mutex<Vec<Step>>>) {
			let steps = Arc::new(Mutex::new(Vec::new()));
			(
				Recording {
					steps: Arc::clone(&steps),
					refuses: None,
				},
				steps,
			)
		}

		fn refusing(switch: Switch) -> (Recording, Arc<Mutex<Vec<Step>>>) {
			let (mut term, steps) = Recording::new();
			term.refuses = Some(switch);
			(term, steps)
		}
	}

	impl Terminal for Recording {
		fn on(&mut self, switch: Switch) -> Result<()> {
			if self.refuses == Some(switch) {
				anyhow::bail!("this terminal will not go into {switch:?}");
			}
			self.steps.lock().unwrap().push(Step::On(switch));
			Ok(())
		}

		fn off(&mut self, switch: Switch) -> Result<()> {
			self.steps.lock().unwrap().push(Step::Off(switch));
			Ok(())
		}
	}

	fn log(steps: &Arc<Mutex<Vec<Step>>>) -> Vec<Step> {
		steps.lock().unwrap().clone()
	}

	#[test]
	fn a_screen_drawn_without_ratatui_hides_the_cursor_and_gives_it_back() {
		// Ratatui hides and shows the cursor itself; a screen drawn by hand has nobody
		// else to do it, and a cursor blinking in the middle of the panel is the result.
		let (term, steps) = Recording::new();
		drop(full_screen().hiding_cursor().enter_on(term).unwrap());
		assert_eq!(
			log(&steps),
			[
				Step::On(Switch::Raw),
				Step::On(Switch::Alternate),
				Step::On(Switch::Cursor),
				Step::Off(Switch::Cursor),
				Step::Off(Switch::Alternate),
				Step::Off(Switch::Raw),
			]
		);
	}

	#[test]
	fn everything_switched_on_comes_back_off_in_the_reverse_order() {
		let (term, steps) = Recording::new();
		drop(full_screen().with_mouse().enter_on(term).unwrap());
		assert_eq!(
			log(&steps),
			[
				Step::On(Switch::Raw),
				Step::On(Switch::Alternate),
				Step::On(Switch::Mouse),
				Step::Off(Switch::Mouse),
				Step::Off(Switch::Alternate),
				Step::Off(Switch::Raw),
			]
		);
	}

	#[test]
	fn a_question_mark_between_two_switches_still_hands_back_the_first() {
		// The defect this module exists for, one step earlier: entering is
		// itself several steps, and the ones already on have to come off.
		let (term, steps) = Recording::refusing(Switch::Alternate);
		let refused = full_screen().with_mouse().enter_on(term);
		assert!(refused.is_err(), "the terminal refused, so there is no guard");
		assert_eq!(log(&steps), [Step::On(Switch::Raw), Step::Off(Switch::Raw)]);
	}

	#[test]
	fn nothing_is_put_back_that_never_went_on() {
		// A guard that switches off what it never switched on is worse than no
		// guard: it turns somebody else's mouse capture off, or shows a cursor
		// the command above this one hid.
		let (term, steps) = Recording::refusing(Switch::Raw);
		assert!(in_place().enter_on(term).is_err());
		assert!(log(&steps).is_empty(), "{:?}", log(&steps));
	}

	#[test]
	fn a_question_mark_inside_the_screen_hands_the_terminal_back() {
		let (term, steps) = Recording::new();
		let drew = || -> Result<()> {
			let _screen = full_screen().enter_on(term)?;
			anyhow::bail!("the serial write failed half way through a drive");
		};
		assert!(drew().is_err());
		assert_eq!(
			log(&steps),
			[
				Step::On(Switch::Raw),
				Step::On(Switch::Alternate),
				Step::Off(Switch::Alternate),
				Step::Off(Switch::Raw),
			]
		);
	}

	#[test]
	fn a_panic_inside_the_screen_hands_the_terminal_back() {
		let (term, steps) = Recording::new();
		// The hook is muted for the length of the panic: a backtrace printed in
		// the middle of `cargo test` reads as a failure, and this one is the
		// test working.
		let previous = std::panic::take_hook();
		std::panic::set_hook(Box::new(|_| {}));
		let panicked = std::panic::catch_unwind(move || {
			let _screen = full_screen().with_mouse().enter_on(term).unwrap();
			panic!("an index off the end of a chart");
		});
		std::panic::set_hook(previous);
		assert!(panicked.is_err());
		assert_eq!(
			log(&steps),
			[
				Step::On(Switch::Raw),
				Step::On(Switch::Alternate),
				Step::On(Switch::Mouse),
				Step::Off(Switch::Mouse),
				Step::Off(Switch::Alternate),
				Step::Off(Switch::Raw),
			]
		);
	}

	#[test]
	fn a_list_drawn_in_place_never_takes_a_screen_of_its_own() {
		// `picker`'s lines belong to the shell it was started from — an
		// alternate screen would take them away at the moment it exits.
		let (term, steps) = Recording::new();
		drop(in_place().enter_on(term).unwrap());
		assert_eq!(
			log(&steps),
			[
				Step::On(Switch::Raw),
				Step::On(Switch::Cursor),
				Step::Off(Switch::Cursor),
				Step::Off(Switch::Raw),
			]
		);
	}

	#[test]
	fn the_cursor_is_hidden_after_raw_mode_and_not_before_it() {
		// So that a terminal which refuses to hide the cursor still gets raw
		// mode back — the ordering `picker::RawMode` was written for.
		let (term, steps) = Recording::refusing(Switch::Cursor);
		assert!(in_place().enter_on(term).is_err());
		assert_eq!(log(&steps), [Step::On(Switch::Raw), Step::Off(Switch::Raw)]);
	}

	#[test]
	fn a_full_screen_does_not_promise_to_show_a_cursor_it_never_hid() {
		// ratatui's `Terminal::draw` hides it and ratatui's `Drop` shows it
		// again. Doing it here as well would mean showing a cursor on a
		// terminal this guard never touched.
		let (term, steps) = Recording::new();
		drop(full_screen().enter_on(term).unwrap());
		assert!(
			!log(&steps)
				.iter()
				.any(|step| matches!(step, Step::On(Switch::Cursor) | Step::Off(Switch::Cursor))),
			"{:?}",
			log(&steps)
		);
	}
}
