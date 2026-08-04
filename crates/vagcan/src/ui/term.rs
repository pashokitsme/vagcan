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

use anyhow::Result;
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

/// The terminal a person is sitting at.
pub struct Crossterm;

impl Terminal for Crossterm {
    fn on(&mut self, switch: Switch) -> Result<()> {
        let mut out = std::io::stdout();
        match switch {
            Switch::Raw => terminal::enable_raw_mode()?,
            Switch::Cursor => execute!(out, cursor::Hide)?,
            Switch::Alternate => execute!(out, terminal::EnterAlternateScreen)?,
            Switch::Mouse => execute!(out, event::EnableMouseCapture)?,
        }
        Ok(())
    }

    fn off(&mut self, switch: Switch) -> Result<()> {
        let mut out = std::io::stdout();
        match switch {
            Switch::Raw => terminal::disable_raw_mode()?,
            Switch::Cursor => execute!(out, cursor::Show)?,
            Switch::Alternate => execute!(out, terminal::LeaveAlternateScreen)?,
            Switch::Mouse => execute!(out, event::DisableMouseCapture)?,
        }
        Ok(())
    }
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
    Wanted { switches: vec![Switch::Raw, Switch::Cursor] }
}

/// A screen of its own — raw keys on the alternate screen.
///
/// The cursor is not in the list, and that is deliberate: the drawing here is
/// ratatui's, `Terminal::draw` hides the cursor itself on any frame that does
/// not place one, and ratatui's own `Drop` shows it again. Hiding it a second
/// time here would mean showing it on a terminal this guard never hid it on.
pub fn full_screen() -> Wanted {
    Wanted { switches: vec![Switch::Raw, Switch::Alternate] }
}

impl Wanted {
    /// Clicks and scrolls as well as keys.
    #[must_use]
    pub fn with_mouse(mut self) -> Wanted {
        self.switches.push(Switch::Mouse);
        self
    }

    /// Switch it all on, and hand back the promise to switch it back off.
    ///
    /// The error is crossterm's, unwrapped: what to tell somebody with no
    /// terminal depends on what the command would have done with one, so the
    /// sentence is added by the caller.
    pub fn enter(self) -> Result<Guard<Crossterm>> {
        self.enter_on(Crossterm)
    }

    /// The same, against something that is not a terminal.
    fn enter_on<T: Terminal>(self, terminal: T) -> Result<Guard<T>> {
        // The guard owns the list before anything is on it, so that a `?` below
        // unwinds through a `Drop` that knows how far it got.
        let mut guard = Guard { terminal, entered: Vec::new() };
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
            (Recording { steps: Arc::clone(&steps), refuses: None }, steps)
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
            !log(&steps).iter().any(|step| matches!(
                step,
                Step::On(Switch::Cursor) | Step::Off(Switch::Cursor)
            )),
            "{:?}",
            log(&steps)
        );
    }
}
