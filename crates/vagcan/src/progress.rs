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

use std::io::Write;

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
}

impl Default for Line {
    fn default() -> Self {
        Line::new()
    }
}

impl Line {
    pub fn new() -> Self {
        Line { step: 0, width: 0, drawn: false }
    }

    /// Redraw with a new message.
    pub fn update(&mut self, message: &str) {
        let text = format!("{} {message}", frame(self.step));
        self.step += 1;
        let mut err = std::io::stderr();
        // Pad to the previous width so a shorter message does not leave the
        // tail of a longer one behind it.
        let pad = self.width.saturating_sub(text.chars().count());
        let _ = write!(err, "\r{text}{:pad$}", "", pad = pad);
        let _ = err.flush();
        self.width = text.chars().count();
        self.drawn = true;
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
    fn the_line_remembers_how_much_it_must_overwrite() {
        // A short message after a long one must cover the tail of the long
        // one, or the screen keeps characters nobody wrote.
        let mut line = Line::new();
        line.update("identifying control units 12 of 15");
        let long = line.width;
        line.update("done");
        assert!(long > line.width, "the fixture really does shorten");
        assert!(line.drawn);
        line.finish();
        assert!(!line.drawn);
    }
}
