//! Choosing one of a handful of things the command already knows about.
//!
//! [`picker`](crate::ui::picker) offers the entries of a directory; this offers
//! a list somebody wrote out in advance — the three things `setup` can learn a
//! car from, and the one line each needs to be told apart by. The two are the
//! same idea and deliberately not the same module: a directory listing is
//! discovered, sorted, and long enough to scroll, and a menu is none of those.
//! A list long enough to need a window is `picker`'s; this one draws every
//! option it was handed and has no window at all.
//!
//! **The input is behind [`Asker`]** for the reason `picker` puts its own behind
//! `Chooser`: the part worth testing here — which option comes first, what its
//! one line of detail says, what a refusal names, what an empty list does — is
//! exactly the part a terminal makes untestable. [`Console`] is the person's;
//! `Scripted` is the tests'.
//!
//! **A pipe is not a terminal, and the two questions answer that differently.**
//! A menu with nobody at the keyboard is a hang with no explanation, so
//! [`Asker::ask`] refuses *before* raw mode is entered and names the command
//! line that needs no menu — `picker`'s rule, not a new one. [`Asker::line`]
//! does the opposite and takes its default without printing or blocking,
//! because a default is an answer and a redirected stdin is precisely the empty
//! line that takes it. That is what keeps `vagcan setup /path/to/VCDS
//! </dev/null` working after a menu lands in front of it: the path answers the
//! only question that had no default, and the rest answer themselves.
//!
//! No new dependency: `crossterm` is already what `picker`, `measure` and
//! `watch` read their keys with, and the terminal is switched over by
//! [`term`](crate::ui::term)'s guard, which hands it back on every way out —
//! error and panic included.

use std::fmt::Write as _;
use std::io::{IsTerminal, Write as _};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::Attribute;
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, execute, terminal};

use crate::ui::term;

/// One line of a menu: what it is, and one line saying what choosing it does.
///
/// The detail is not decoration. "ODIS project" is three words a Škoda owner
/// has never met, and the line beside it is the whole of what tells them
/// whether they have one.
pub struct Item<'a> {
	pub label: &'a str,
	pub detail: &'a str,
}

/// Where a menu's answer comes from.
///
/// Three things, which is what a fixed list of options needs: ask which one,
/// ask for a word, say a line. `Err` from any of them ends the conversation.
pub trait Asker {
	/// Show `items` under `question` with row `at` highlighted; `None` is a quit.
	fn ask(&mut self, question: &str, items: &[Item<'_>], at: usize) -> Result<Option<usize>>;

	/// A free-text answer, with `default` taken on an empty line.
	///
	/// An empty `default` means there is no answer to fall back on, and the
	/// empty string the caller then gets back is the person saying "never mind".
	fn line(&mut self, question: &str, default: &str) -> Result<String>;

	/// Say one line: what was chosen, why something was refused.
	fn say(&mut self, line: &str) -> Result<()>;
}

/// The menu a person uses: the options on stdout, arrow keys off the terminal.
pub struct Console {
	/// What to tell someone whose stdin is a pipe — the command line that needs
	/// no menu. Passed in because this module does not know which command it is
	/// serving, the same reason `picker::Console` is built with one.
	instead: String,
}

impl Console {
	/// `instead` is the command line that answers the question without a menu,
	/// complete enough to paste: `vagcan setup /path/to/VCDS`.
	pub fn new(instead: impl Into<String>) -> Console {
		Console { instead: instead.into() }
	}
}

impl Asker for Console {
	fn ask(&mut self, question: &str, items: &[Item<'_>], at: usize) -> Result<Option<usize>> {
		// Before the terminal check, not after: a menu with no options is a
		// mistake in the caller, and it is one whether anybody is watching or
		// not. Drawing it would offer a person an empty screen and wait for a
		// key that means nothing.
		if items.is_empty() {
			bail!(nothing_to_choose(question));
		}
		// Before raw mode, not after: switching a terminal that is not there is
		// an errno, and this is the sentence that actually helps.
		if !std::io::stdin().is_terminal() {
			bail!(no_terminal(&self.instead));
		}
		let mut at = at.min(items.len() - 1);
		let mut out = std::io::stdout();
		// Everything below this line can leave through a `?` or a panic, and both
		// have to hand the terminal back the way they found it.
		let _raw = term::in_place().enter()?;
		let mut drawn = 0usize;
		let answer = loop {
			// A terminal that answers 0 does not know how wide it is — a pty with
			// no window size. Guessing 80 shows the menu; believing the 0 shows a
			// column of ellipses.
			let (width, _) = terminal::size().unwrap_or((0, 0));
			let lines = screen(question, items, at, if width == 0 { 80 } else { width });
			redraw(&mut out, &mut drawn, &lines)?;
			let Event::Key(key) = event::read()? else { continue };
			// Windows sends a release for every press; acting on both moves the
			// highlight twice per keystroke.
			if key.kind != KeyEventKind::Press {
				continue;
			}
			let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
			match key.code {
				KeyCode::Up => at = moved(at, items.len(), false),
				KeyCode::Down => at = moved(at, items.len(), true),
				KeyCode::Home => at = 0,
				KeyCode::End => at = items.len() - 1,
				// `→` has nothing below it to go into on a menu, so it means the
				// same as Enter rather than nothing at all.
				KeyCode::Enter | KeyCode::Right => break Some(at),
				// The digits are not a shortcut for people who know the menu —
				// they are the way in on a terminal whose arrow keys do not
				// arrive at all, which is every serial console and half of what
				// `ssh` lands on. The legend says so.
				KeyCode::Char(digit) if picked(digit, items.len()).is_some() => break picked(digit, items.len()),
				KeyCode::Char('q') | KeyCode::Esc => break None,
				// Raw mode swallows the interrupt, so the key has to be honoured
				// by hand or there is no way out of here.
				KeyCode::Char('c') if ctrl => break None,
				_ => {}
			}
		};
		// The menu has done its job; leave the screen where the next line of
		// output belongs rather than under a stale list.
		erase(&mut out, drawn)?;
		Ok(answer)
	}

	fn line(&mut self, question: &str, default: &str) -> Result<String> {
		// Not an error, unlike `ask`. A question with a default has an answer
		// already, and a redirected stdin is the empty line that takes it — so a
		// script gets the default silently rather than a refusal it cannot act
		// on. Nothing is printed either: there is nobody to read a prompt, and
		// the caller says what it settled on afterwards.
		if !std::io::stdin().is_terminal() {
			return Ok(default.to_string());
		}
		// Asked outside raw mode, so the answer echoes and a backspace works.
		let mut out = std::io::stdout();
		write!(out, "{}", prompt(question, default))?;
		out.flush()?;
		let mut typed = String::new();
		// Zero bytes is Ctrl-D: no answer, and no answer is the default.
		if std::io::stdin().read_line(&mut typed)? == 0 {
			writeln!(out)?;
			return Ok(default.to_string());
		}
		Ok(answer(&typed, default))
	}

	fn say(&mut self, line: &str) -> Result<()> {
		let mut out = std::io::stdout();
		writeln!(out, "{line}")?;
		out.flush()?;
		Ok(())
	}
}

/// Which row a digit names, when the menu is short enough for digits to reach.
///
/// `1` is the first row: the screen numbers from one because people do, and
/// nothing outside this function ever sees the digit.
fn picked(digit: char, len: usize) -> Option<usize> {
	let row = digit.to_digit(10)?.checked_sub(1)? as usize;
	(row < len && len <= 9).then_some(row)
}

/// Where the highlight goes next.
///
/// It wraps, where `picker`'s clamps. A menu is three rows with no window and
/// both ends on screen at once, so the distance from the last option back to
/// the first is one keystroke and a person who overshoots the bottom means the
/// top. A directory of two hundred recordings is the opposite case, and that is
/// why the rule lives here rather than in a module both share.
fn moved(at: usize, len: usize, down: bool) -> usize {
	if len == 0 {
		return 0;
	}
	match down {
		true => (at + 1) % len,
		false => (at + len - 1) % len,
	}
}

/// Put `lines` where the last draw was, and remember how tall it is.
///
/// Relative movement, not absolute: the terminal scrolls when the menu reaches
/// the bottom, and a remembered absolute row would then point at the wrong
/// place. Every line ends `\r\n` because raw mode does not return the carriage.
fn redraw(out: &mut impl std::io::Write, drawn: &mut usize, lines: &[String]) -> Result<()> {
	if *drawn > 0 {
		execute!(out, cursor::MoveToPreviousLine(*drawn as u16), Clear(ClearType::FromCursorDown))?;
	}
	for line in lines {
		write!(out, "{line}\r\n")?;
	}
	out.flush()?;
	*drawn = lines.len();
	Ok(())
}

/// Take the menu back off the screen.
fn erase(out: &mut impl std::io::Write, drawn: usize) -> Result<()> {
	if drawn > 0 {
		execute!(out, cursor::MoveToPreviousLine(drawn as u16), Clear(ClearType::FromCursorDown))?;
		out.flush()?;
	}
	Ok(())
}

/// The menu as a person sees it, one string per screen line.
///
/// Pure, and given its own width rather than asking the terminal for one: what
/// this draws is most of what this module is, and none of it should need a
/// terminal to test. `pub(crate)` so that a caller can measure its own copy
/// against a terminal width instead of guessing at this function's arithmetic —
/// a detail line one column too long is cut to `…` and nobody finds out.
pub(crate) fn screen(question: &str, items: &[Item<'_>], at: usize, width: u16) -> Vec<String> {
	let width = (width as usize).max(20);
	let mut lines = vec![clip(&format!("? {question}"), width)];
	// The details line up under each other, or three sentences at three
	// different indents read as three unrelated things.
	let label_width = items.iter().map(|i| i.label.chars().count()).max().unwrap_or(0).min(24);
	for (row, item) in items.iter().enumerate() {
		let text = clip(
			&format!("{} {:<label_width$}  {}", if row == at { "❯" } else { " " }, item.label, item.detail),
			width,
		);
		// Reverse video rather than a colour: it survives every theme, and the
		// marker in front of it survives a terminal with no attributes at all.
		lines.push(match row == at {
			true => format!("{}{text}{}", Attribute::Reverse, Attribute::Reset),
			false => text,
		});
	}
	lines.push(clip(&keys(items.len()), width));
	lines
}

/// The legend. A key nobody can discover is not a feature.
fn keys(len: usize) -> String {
	let mut out = String::from("↑↓ move   ⏎ choose");
	// Offered only where a digit reaches a row, so the legend never names a key
	// that does nothing.
	if (2..=9).contains(&len) {
		let _ = write!(out, "   1-{len} pick");
	}
	let _ = write!(out, "   q quit");
	out
}

/// Cut a line to the width of the screen.
///
/// A line that wraps counts as two rows on screen and one in the redraw's
/// count, which walks the menu up the terminal a line at a time.
fn clip(line: &str, width: usize) -> String {
	match line.chars().count() > width {
		true => line.chars().take(width.saturating_sub(1)).collect::<String>() + "…",
		false => line.to_string(),
	}
}

/// The prompt a typed answer is asked behind, with the default it will take.
fn prompt(question: &str, default: &str) -> String {
	match default.is_empty() {
		true => format!("{question} "),
		false => format!("{question} [{default}] "),
	}
}

/// What a typed line means: itself, or the default when nothing was typed.
fn answer(typed: &str, default: &str) -> String {
	match typed.trim() {
		"" => default.to_string(),
		text => text.to_string(),
	}
}

/// A menu with nothing on it.
///
/// Always a mistake in the caller rather than a state a person can be in, so it
/// says which question came up empty instead of asking them to do something.
fn nothing_to_choose(question: &str) -> String {
	format!("nothing to choose from for {question:?} — the menu was built with no options, which is a bug in vagcan")
}

/// There is nobody at a keyboard, so the menu cannot be shown.
///
/// A prompt on a redirected stdin is a hang with no explanation, and the thing
/// the person wanted is one argument away.
fn no_terminal(instead: &str) -> String {
	format!(
		"there is no terminal to choose at — stdin is redirected, so nobody would see the \
         menu.\nSay which one on the command line instead:\n    {instead}"
	)
}

/// One scripted answer, in the order the questions come.
///
/// Lives beside [`Console`] rather than in this module's own tests so that any
/// command's tests can drive its menu without a terminal.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
	/// Take this row of the menu.
	Pick(usize),
	/// Type this at a free-text question. The empty string is an empty line,
	/// which takes the default.
	Type(String),
	/// Leave without choosing.
	Quit,
}

/// The asker the tests use: the answers go in, and everything it showed or was
/// told to say comes back out to be asserted against.
#[cfg(test)]
pub struct Scripted {
	answers: std::collections::VecDeque<Answer>,
	/// Every menu it was shown: the question, then each option's label and its
	/// line of detail, in the order they were offered.
	pub seen: Vec<(String, Vec<(String, String)>)>,
	/// The row highlighted when each of those menus appeared.
	pub highlights: Vec<usize>,
	/// Every free-text question, with the default that was offered for it.
	pub typed: Vec<(String, String)>,
	/// Every line it was told to say.
	pub said: Vec<String>,
}

#[cfg(test)]
impl Scripted {
	pub fn new(answers: Vec<Answer>) -> Scripted {
		Scripted {
			answers: answers.into(),
			seen: Vec::new(),
			highlights: Vec::new(),
			typed: Vec::new(),
			said: Vec::new(),
		}
	}

	/// The labels of the last menu it was shown, in the order they were offered.
	pub fn last_labels(&self) -> Vec<String> {
		self
			.seen
			.last()
			.map(|(_, items)| items.iter().map(|(label, _)| label.clone()).collect())
			.unwrap_or_default()
	}

	/// Every option of the last menu as one blob to look for a phrase in —
	/// labels and details together, since the detail is half the copy.
	pub fn last_menu(&self) -> String {
		self
			.seen
			.last()
			.map(|(question, items)| {
				let mut out = question.clone();
				for (label, detail) in items {
					let _ = write!(out, "\n{label}  {detail}");
				}
				out
			})
			.unwrap_or_default()
	}

	/// Everything it was told to say, as one blob to look for a phrase in.
	pub fn all_said(&self) -> String {
		self.said.join("\n")
	}

	/// The defaults it was offered, in order — the answer to "what does it
	/// suggest when the person just presses Enter".
	pub fn defaults(&self) -> Vec<String> {
		self.typed.iter().map(|(_, default)| default.clone()).collect()
	}
}

#[cfg(test)]
impl Asker for Scripted {
	fn ask(&mut self, question: &str, items: &[Item<'_>], at: usize) -> Result<Option<usize>> {
		if items.is_empty() {
			bail!(nothing_to_choose(question));
		}
		self.seen.push((
			question.to_string(),
			items.iter().map(|i| (i.label.to_string(), i.detail.to_string())).collect(),
		));
		self.highlights.push(at);
		match self.answers.pop_front() {
			Some(Answer::Pick(row)) => Ok(Some(row)),
			Some(Answer::Quit) => Ok(None),
			Some(Answer::Type(text)) => bail!("the script typed {text:?} at the menu {question:?}"),
			None => bail!("the script ran out at the menu {question:?}"),
		}
	}

	fn line(&mut self, question: &str, default: &str) -> Result<String> {
		self.typed.push((question.to_string(), default.to_string()));
		match self.answers.pop_front() {
			Some(Answer::Type(text)) => Ok(answer(&text, default)),
			Some(Answer::Quit) => Ok(String::new()),
			Some(Answer::Pick(row)) => bail!("the script picked row {row} at the question {question:?}"),
			None => bail!("the script ran out at the question {question:?}"),
		}
	}

	fn say(&mut self, line: &str) -> Result<()> {
		self.said.push(line.to_string());
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// One drawn line with the highlight's escape bytes taken out — what is
	/// actually on the screen, in columns.
	fn plain(line: &str) -> String {
		line
			.replace(&Attribute::Reverse.to_string(), "")
			.replace(&Attribute::Reset.to_string(), "")
	}

	fn three<'a>() -> [Item<'a>; 3] {
		[
			Item {
				label: "VCDS installation",
				detail: "the folder holding Labels/ and UDS_EV/",
			},
			Item {
				label: "ODIS project",
				detail: "an extracted ODIS-Service project folder",
			},
			Item {
				label: "Download VCDS",
				detail: "fetch Ross-Tech's installer, about 90 MB",
			},
		]
	}

	#[test]
	fn a_menu_shows_every_option_with_the_line_that_tells_them_apart() {
		// The label alone is not the offer: "ODIS project" is three words most
		// owners have never met, and the detail is what says whether they have
		// one.
		let drawn = screen("What should vagcan learn this car from?", &three(), 0, 80).join("\n");
		assert!(drawn.contains("? What should vagcan learn this car from?"), "{drawn}");
		assert!(drawn.contains("❯ VCDS installation"), "the first row is the one: {drawn}");
		assert!(drawn.contains("an extracted ODIS-Service project folder"), "{drawn}");
		assert!(drawn.contains("about 90 MB"), "{drawn}");
	}

	#[test]
	fn the_details_line_up_under_one_another() {
		// Three sentences at three indents read as three unrelated things. The
		// highlighted row carries escape bytes that take no columns, so it is
		// measured without them.
		let drawn = screen("q", &three(), 0, 80);
		let at: Vec<usize> = drawn[1..4]
			.iter()
			.map(|line| plain(line))
			.map(|line| {
				let byte = line.find("the folder").or(line.find("an extracted")).or(line.find("fetch")).unwrap();
				// Columns, not bytes: `❯` is one column and three bytes, so the
				// highlighted row would otherwise look two places further along.
				line[..byte].chars().count()
			})
			.collect();
		assert_eq!(at[0], at[1], "{drawn:?}");
		assert_eq!(at[1], at[2], "{drawn:?}");
	}

	#[test]
	fn the_legend_names_every_key_that_works_and_none_that_does_not() {
		assert!(keys(3).contains("↑↓ move"), "{}", keys(3));
		assert!(keys(3).contains("⏎ choose"), "{}", keys(3));
		assert!(keys(3).contains("1-3 pick"), "{}", keys(3));
		assert!(keys(3).contains("q quit"), "{}", keys(3));
		// A menu of one has no digit worth naming, and one of twelve has digits
		// that cannot reach every row.
		assert!(!keys(1).contains("pick"), "{}", keys(1));
		assert!(!keys(12).contains("pick"), "{}", keys(12));
	}

	#[test]
	fn a_digit_names_the_row_a_person_would_count_to() {
		// People count menus from one. Nothing outside `picked` sees the digit.
		assert_eq!(picked('1', 3), Some(0));
		assert_eq!(picked('3', 3), Some(2));
		assert_eq!(picked('4', 3), None, "past the end of the menu");
		assert_eq!(picked('0', 3), None, "there is no row zero on screen");
		assert_eq!(picked('q', 3), None);
		assert_eq!(picked('5', 12), None, "a menu digits cannot cover has no digit keys at all");
	}

	#[test]
	fn the_highlight_wraps_at_both_ends() {
		// Three rows, both ends on screen at once: overshooting the bottom means
		// the top, and one keystroke should get there.
		assert_eq!(moved(2, 3, true), 0);
		assert_eq!(moved(0, 3, false), 2);
		assert_eq!(moved(0, 3, true), 1);
		assert_eq!(moved(1, 3, false), 0);
		assert_eq!(moved(0, 1, true), 0, "a menu of one goes nowhere");
	}

	#[test]
	fn a_menu_with_no_options_is_refused_rather_than_drawn() {
		// A person offered an empty screen and asked to press a key has been
		// asked nothing. It is the caller that is wrong, and the message says so
		// rather than telling them to go and do something.
		let why = Console::new("vagcan setup /path/to/VCDS")
			.ask("What should vagcan learn this car from?", &[], 0)
			.unwrap_err()
			.to_string();
		assert!(why.contains("nothing to choose from"), "{why}");
		assert!(why.contains("What should vagcan learn this car from?"), "{why}");
		assert!(why.contains("bug in vagcan"), "{why}");
	}

	#[test]
	fn with_no_terminal_the_refusal_names_the_command_that_needs_no_menu() {
		// A prompt on a redirected stdin is a hang nobody can diagnose.
		let why = no_terminal("vagcan setup /path/to/VCDS");
		assert!(why.contains("no terminal"), "{why}");
		assert!(why.contains("vagcan setup /path/to/VCDS"), "{why}");
	}

	#[test]
	fn a_question_with_no_terminal_takes_its_default_instead_of_refusing() {
		// The opposite rule to `ask`, and the reason `vagcan setup PATH` keeps
		// working under `</dev/null`: a default is already an answer, so a
		// redirected stdin is the empty line that takes it.
		let taken = Console::new("vagcan setup /path/to/VCDS")
			.line("What should this project be called?", "default")
			.unwrap();
		assert_eq!(taken, "default", "cargo test has no terminal, which is the case under test");
	}

	#[test]
	fn an_empty_line_takes_the_default_and_anything_else_is_the_answer() {
		assert_eq!(answer("", "default"), "default");
		assert_eq!(answer("   \n", "default"), "default");
		assert_eq!(answer(" SK37X \n", "default"), "SK37X");
	}

	#[test]
	fn the_prompt_shows_the_default_that_pressing_enter_would_take() {
		assert_eq!(
			prompt("What should this project be called?", "SK37X"),
			"What should this project be called? [SK37X] "
		);
		// Nothing to fall back on, so nothing is promised.
		assert_eq!(prompt("Where is it?", ""), "Where is it? ");
	}

	#[test]
	fn a_scripted_menu_answers_in_the_order_the_questions_come() {
		let mut io = Scripted::new(vec![Answer::Pick(1), Answer::Type("SK37X".to_string())]);
		assert_eq!(io.ask("which?", &three(), 0).unwrap(), Some(1));
		assert_eq!(io.line("called?", "default").unwrap(), "SK37X");
		assert_eq!(io.last_labels(), ["VCDS installation", "ODIS project", "Download VCDS"]);
		assert_eq!(io.highlights, [0]);
		assert_eq!(io.defaults(), ["default"]);
	}

	#[test]
	fn a_quit_is_a_quit_and_not_a_row() {
		let mut io = Scripted::new(vec![Answer::Quit]);
		assert_eq!(io.ask("which?", &three(), 0).unwrap(), None);
	}

	#[test]
	fn a_scripted_empty_line_takes_the_default_the_way_a_typed_one_does() {
		// So a test can say "they just pressed Enter" without knowing what the
		// default turned out to be.
		let mut io = Scripted::new(vec![Answer::Type(String::new())]);
		assert_eq!(io.line("called?", "SK37X").unwrap(), "SK37X");
	}

	#[test]
	fn the_highlight_starts_where_the_caller_asked() {
		let mut io = Scripted::new(vec![Answer::Pick(2)]);
		io.ask("which?", &three(), 2).unwrap();
		assert_eq!(io.highlights, [2]);
		let drawn = screen("which?", &three(), 2, 80).join("\n");
		assert!(drawn.contains("❯ Download VCDS"), "{drawn}");
	}

	#[test]
	fn a_line_wider_than_the_screen_is_cut_rather_than_wrapped() {
		// A wrapped line is two rows on screen and one row in the redraw's
		// count, which walks the menu up the terminal a line at a time.
		let items = [Item {
			label: "VCDS installation",
			detail: &"x".repeat(200),
		}];
		for line in screen("q", &items, 0, 40) {
			// The highlight adds escape bytes that are not printed columns, so
			// the row that carries them is measured without them.
			let printed = line
				.replace(&Attribute::Reverse.to_string(), "")
				.replace(&Attribute::Reset.to_string(), "");
			assert!(printed.chars().count() <= 40, "{printed:?}");
		}
	}
}
