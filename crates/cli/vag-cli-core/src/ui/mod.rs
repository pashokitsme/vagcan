//! What the commands draw with.
//!
//! What is here is what more than one command draws with: the terminal guard
//! every full-screen command and the picker enter through, and the file chooser
//! `measure view` and `recording` both offer when a path was left off.

use std::io::Write as _;

pub mod bars;
pub mod chart;
pub mod menu;
pub mod picker;
pub mod term;

/// Whether this process may stop and ask a person a question.
///
/// One predicate, because "is there somebody at the keyboard" is one question
/// and it was being asked in six places in two crates — [`menu`], [`picker`],
/// `setup`'s file search and `faults`' offer to unseal — each spelling out
/// `std::io::stdin().is_terminal()` by hand. Spelled once, it can be read once.
///
/// **The reactions to a `false` differ on purpose, and must keep differing.**
/// What is shared is the fact, not what to do about it:
///
/// | asked from | when nobody is there | why |
/// |---|---|---|
/// | [`menu::Asker::ask`], [`picker::Chooser::choose`], [`picker::Chooser::confirm`] | refuse, naming the argument that needs no menu | a list nobody can see is a hang with no explanation, and there is no answer to fall back on |
/// | [`menu::Asker::line`] | take the default, silently | a question with a default already has an answer, and a redirected stdin is the empty line that takes it |
/// | `setup`'s file search, `faults`' offer to unseal | skip the question, print what to run | the step has something else to report, and blocking would be the only failure |
///
/// Folding those three into one reaction would break `vagcan setup
/// /path/to/VCDS </dev/null`, which works precisely because the questions with
/// defaults answer themselves while the ones without refuse.
///
/// **Not the same question as a progress bar's.** [`crate::progress::Line`]
/// asks whether *stderr* is a terminal — "may I draw and erase a line" — which
/// is true of a run whose stdin is a pipe and false of one whose output is
/// redirected. The two go opposite ways on the same command often enough that
/// merging them would be a bug, not a simplification.
pub fn can_ask() -> bool {
	std::io::IsTerminal::is_terminal(&std::io::stdin())
}

/// Repaint a block of `lines` in place, over whatever the last call left.
///
/// `drawn` is how many lines are on screen from the previous call and is
/// updated to how many are now. Zero means nothing has been drawn yet, so
/// nothing is cleared — which is also what makes the first call correct.
///
/// **Here rather than in [`menu`] and [`picker`], which each had a byte-identical
/// copy.** Two screens that scroll the terminal differently is a defect nobody
/// would choose, and there is no version of "draw a list and move the cursor
/// back over it" that is specific to what the list holds. `\r\n` and not `\n`:
/// both callers are inside [`term`]'s raw mode, where a bare newline moves down
/// without returning to column one.
pub fn redraw(out: &mut impl std::io::Write, drawn: &mut usize, lines: &[String]) -> anyhow::Result<()> {
	if *drawn > 0 {
		crossterm::execute!(
			out,
			crossterm::cursor::MoveToPreviousLine(*drawn as u16),
			crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown)
		)?;
	}
	for line in lines {
		write!(out, "{line}\r\n")?;
	}
	out.flush()?;
	*drawn = lines.len();
	Ok(())
}

/// Take a block of `drawn` lines back off the screen.
///
/// Called on every way out of both screens — the answer, the quit, and the `?`
/// — so that the next line of output lands where it belongs rather than under a
/// stale list. See [`redraw`] for why it lives here.
pub fn erase(out: &mut impl std::io::Write, drawn: usize) -> anyhow::Result<()> {
	if drawn > 0 {
		crossterm::execute!(
			out,
			crossterm::cursor::MoveToPreviousLine(drawn as u16),
			crossterm::terminal::Clear(crossterm::terminal::ClearType::FromCursorDown)
		)?;
		out.flush()?;
	}
	Ok(())
}

/// The screen a person actually uses: a list on stdout, keys off the terminal.
///
/// **One type for both screens, where there were two.** [`menu`] and [`picker`]
/// each had a `Console` holding one `String`, a `new` that wrapped it and a
/// `say` that wrote a line — byte for byte the same, differing only in which
/// trait the rest of the file went on to implement. The traits still differ and
/// still live in their own modules; the thing they are implemented for does not
/// have to.
pub struct Console {
	/// What to tell someone whose stdin is a pipe: the command line that answers
	/// the question without anybody at a keyboard. Passed in because neither
	/// screen knows which command it is serving — `vagcan setup /path/to/VCDS`
	/// for the menu, `vagcan measure view FILE.json` for the picker.
	pub(crate) instead: String,
}

impl Console {
	/// `instead` is that command line, complete enough to paste.
	pub fn new(instead: impl Into<String>) -> Console {
		Console { instead: instead.into() }
	}

	/// Say one line. Both traits below declare a `say`, and both are this.
	pub(crate) fn say_line(&mut self, line: &str) -> anyhow::Result<()> {
		let mut out = std::io::stdout();
		writeln!(out, "{line}")?;
		out.flush()?;
		Ok(())
	}
}
