//! What the commands draw with.
//!
//! What is here is what more than one command draws with: the terminal guard
//! every full-screen command and the picker enter through, and the file chooser
//! `measure view` and `recording` both offer when a path was left off.

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
