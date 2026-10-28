//! How a failed command says so: cargo's shape, for `vagcan` and `dashcfg`.
//!
//! ```text
//! error: no USB-CAN adapter found
//!
//! help: plug one in and check `vagcan devices`
//! help: over Bluetooth, add `--ble`
//! ```
//!
//! - `error:` is the error's own message, one short line saying what failed.
//! - `caused by:` lines follow it, one per cause in the `anyhow` chain. At level 0 only a cause
//!   that says something the lines above it do not; at `-v` every one.
//! - A blank line, then `note:` lines (a reason the reader needs in order to act) and `help:`
//!   lines (one action each, the command to type in backticks).
//!
//! **Hints are typed, never parsed out of prose** (owner, 2026-10-21). A call site attaches them
//! with [`Advice::help`] and [`Advice::note`], on an `anyhow::Error` or on a `Result`:
//!
//! ```
//! use vag_cli_core::report::Advice as _;
//! let err = anyhow::anyhow!("no USB-CAN adapter found").help("plug one in and check `vagcan devices`");
//! assert_eq!(err.to_string(), "no USB-CAN adapter found");
//! ```
//!
//! The hints ride in a [`Hinted`] wrapper that is transparent everywhere else: its `Display` is
//! the wrapped error's, `{:#}` and the chain are unchanged, so a test or a step that prints the
//! error sees the same text it did before. The one thing the wrapper changes is
//! `anyhow::Error::downcast_ref`, which sees the wrapper and not the type inside it: a typed
//! error that is looked for by type (`NoLabelData`, `NotARecord`) is found with [`find`].

use std::fmt;
use std::io::IsTerminal as _;

/// One line under an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
	/// A reason the reader needs in order to act.
	Note(String),
	/// One action, with the command to type in backticks.
	Help(String),
}

/// An error and the hints attached to it. Created by [`Advice`]; read by [`render`].
///
/// Transparent: `Display` (plain and `{:#}`) is the inner error's, and `source` is the inner
/// error's first cause, so the chain reads as if the wrapper were not there.
pub struct Hinted {
	inner: anyhow::Error,
	hints: Vec<Hint>,
}

impl Hinted {
	/// The hints, in the order they were attached.
	pub fn hints(&self) -> &[Hint] {
		&self.hints
	}
}

impl fmt::Display for Hinted {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// Only the inner error's own message: the chain is walked by whoever formats it, through
		// `source`, so writing `{:#}` here would say every cause twice.
		fmt::Display::fmt(&self.inner.to_string(), f)
	}
}

impl fmt::Debug for Hinted {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		fmt::Debug::fmt(&self.inner, f)
	}
}

impl std::error::Error for Hinted {
	fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
		self.inner.source()
	}
}

/// `.help("…")` and `.note("…")` on an error, or on a `Result` that may hold one.
pub trait Advice: Sized {
	type Out;
	/// Attach a `help:` line.
	fn help(self, text: impl fmt::Display) -> Self::Out;
	/// Attach a `note:` line.
	fn note(self, text: impl fmt::Display) -> Self::Out;
}

fn attach(mut err: anyhow::Error, hint: Hint) -> anyhow::Error {
	// Onto the wrapper already at the top, so `.note(…).help(…)` is one wrapper, not two.
	if let Some(hinted) = err.downcast_mut::<Hinted>() {
		hinted.hints.push(hint);
		return err;
	}
	anyhow::Error::new(Hinted {
		inner: err,
		hints: vec![hint],
	})
}

impl Advice for anyhow::Error {
	type Out = anyhow::Error;

	fn help(self, text: impl fmt::Display) -> anyhow::Error {
		attach(self, Hint::Help(text.to_string()))
	}

	fn note(self, text: impl fmt::Display) -> anyhow::Error {
		attach(self, Hint::Note(text.to_string()))
	}
}

impl<T, E: Into<anyhow::Error>> Advice for Result<T, E> {
	type Out = Result<T, anyhow::Error>;

	fn help(self, text: impl fmt::Display) -> Self::Out {
		self.map_err(|e| e.into().help(text))
	}

	fn note(self, text: impl fmt::Display) -> Self::Out {
		self.map_err(|e| e.into().note(text))
	}
}

/// The error of type `T` anywhere in `err` — its top, a context, any cause — looking inside
/// every [`Hinted`] wrapper: what `err.downcast_ref::<T>()` would answer had no hint been
/// attached, and what a walk of `err.chain()` would.
pub fn find<T: std::error::Error + Send + Sync + 'static>(err: &anyhow::Error) -> Option<&T> {
	err
		.downcast_ref::<T>()
		.or_else(|| err.chain().find_map(|e| e.downcast_ref::<T>()))
		.or_else(|| {
			err
				.chain()
				.filter_map(|e| e.downcast_ref::<Hinted>())
				.find_map(|hinted| find::<T>(&hinted.inner))
		})
}

/// Whether any error of type `T` in `err`, inside [`Hinted`] wrappers too, satisfies `test`.
pub fn any<T: std::error::Error + 'static>(err: &anyhow::Error, test: &dyn Fn(&T) -> bool) -> bool {
	err.chain().any(|e| e.downcast_ref::<T>().is_some_and(test))
		|| err
			.chain()
			.filter_map(|e| e.downcast_ref::<Hinted>())
			.any(|hinted| any(&hinted.inner, test))
}

/// Every hint attached anywhere in `err`'s chain, outermost first.
pub fn hints(err: &anyhow::Error) -> Vec<Hint> {
	err
		.chain()
		.filter_map(|e| e.downcast_ref::<Hinted>())
		.flat_map(|h| h.hints.iter().cloned())
		.collect()
}

/// `err` with its message, its causes and its hints, and without its types: what a site
/// returns when no caller may recognise the error by downcasting it, and the hints must
/// still reach the reader.
pub fn untyped(err: &anyhow::Error) -> anyhow::Error {
	hints(err).into_iter().fold(anyhow::anyhow!("{err:#}"), |out, hint| match hint {
		Hint::Note(text) => out.note(text),
		Hint::Help(text) => out.help(text),
	})
}

/// The report without its `error: ` label: the message, then a blank line and the hints,
/// for a screen that shows an error under a heading of its own.
pub fn unlabelled(err: &anyhow::Error) -> String {
	let shown = render(err, 0, Colour::Never);
	shown.strip_prefix("error: ").unwrap_or(&shown).trim_end().to_string()
}

/// Whether the report is drawn in colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Colour {
	Never,
	Always,
}

impl Colour {
	/// Colour when stderr is a terminal and `NO_COLOR` is unset or empty (no-color.org).
	pub fn for_stderr() -> Colour {
		let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
		Colour::on(std::io::stderr().is_terminal(), no_color)
	}

	/// Colour on a terminal, unless `NO_COLOR` asks for none.
	fn on(terminal: bool, no_color: bool) -> Colour {
		match terminal && !no_color {
			true => Colour::Always,
			false => Colour::Never,
		}
	}
}

/// cargo's styles: `error` bold red, `warning` bold yellow, `note` bold green, `help` bold cyan.
const RED: &str = "\x1b[1;31m";
const YELLOW: &str = "\x1b[1;33m";
const GREEN: &str = "\x1b[1;32m";
const CYAN: &str = "\x1b[1;36m";
const RESET: &str = "\x1b[0m";

/// The labels a line on stderr may start with, and their styles.
const LABELS: [(&str, &str); 4] = [("error", RED), ("warning", YELLOW), ("note", GREEN), ("help", CYAN)];

/// `text` with the label each of its lines starts with — `warning: …`, `note: …` — drawn as
/// [`render`] draws an error's: every labelled line on stderr looks the same, whichever
/// command said it.
pub fn paint(text: &str, colour: Colour) -> std::borrow::Cow<'_, str> {
	let labelled = |line: &str| {
		LABELS
			.iter()
			.find(|(label, _)| line.strip_prefix(label).is_some_and(|rest| rest.starts_with(": ")))
	};
	if colour == Colour::Never || !text.split_inclusive('\n').any(|l| labelled(l).is_some()) {
		return text.into();
	}
	let mut out = String::with_capacity(text.len() + 16);
	for line in text.split_inclusive('\n') {
		match labelled(line) {
			Some((label, style)) => out.push_str(&format!("{style}{label}:{RESET}{}", &line[label.len() + 1..])),
			None => out.push_str(line),
		}
	}
	out.into()
}

/// `label:` and `text` as one line or more, continuation lines indented under the text.
fn line(out: &mut String, label: &str, style: &str, text: &str, colour: Colour) {
	match colour {
		Colour::Always => out.push_str(&format!("{style}{label}:{RESET}")),
		Colour::Never => out.push_str(&format!("{label}:")),
	}
	let indent = " ".repeat(label.len() + 2);
	for (n, piece) in text.lines().enumerate() {
		match (n, piece.is_empty()) {
			(0, _) => out.push(' '),
			(_, true) => {}
			(_, false) => out.push_str(&indent),
		}
		out.push_str(piece);
		out.push('\n');
	}
	if text.lines().next().is_none() {
		out.push('\n');
	}
}

/// The report for `err` at verbosity `level`, every line ending in `\n`.
pub fn render(err: &anyhow::Error, level: u8, colour: Colour) -> String {
	let mut out = String::new();
	let headline = err.to_string();
	line(&mut out, "error", RED, &headline, colour);
	// The causes the reader has not already read: a message that quotes its cause with
	// `{e:#}` has said it, and a wrapper says its inner error's message again.
	let mut said = vec![headline];
	for cause in err.chain().skip(1) {
		let text = cause.to_string();
		let new = !said.iter().any(|s| s.contains(&text));
		if new || level >= crate::verbosity::NOTES {
			line(&mut out, "caused by", RED, &text, Colour::Never);
		}
		said.push(text);
	}
	let hints = hints(err);
	if !hints.is_empty() {
		out.push('\n');
	}
	for hint in hints.iter().filter(|h| matches!(h, Hint::Note(_))) {
		if let Hint::Note(text) = hint {
			line(&mut out, "note", GREEN, text, colour);
		}
	}
	for hint in &hints {
		if let Hint::Help(text) = hint {
			line(&mut out, "help", CYAN, text, colour);
		}
	}
	out
}

/// Report `err` on stderr, at the process's verbosity, in colour on a terminal.
///
/// Written, not `eprintln!`ed, and the write's result ignored: `eprintln!` panics when stderr
/// will not take the bytes — a full disk under `2>log`, `2>/dev/full` — and the panic's own
/// message then has nowhere to go either, which ended a run with exit 101 and nothing said
/// (#38).
pub fn report(err: &anyhow::Error) {
	use std::io::Write as _;
	let text = render(err, crate::verbosity::level(), Colour::for_stderr());
	let _ = std::io::stderr().lock().write_all(text.as_bytes());
}

#[cfg(test)]
mod tests {
	use super::{Advice as _, Colour, Hint, find, hints, render};
	use anyhow::{Context as _, anyhow};

	#[test]
	fn an_untyped_copy_keeps_the_message_and_every_hint() {
		#[derive(Debug)]
		struct Typed;
		impl std::fmt::Display for Typed {
			fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
				f.write_str("typed")
			}
		}
		impl std::error::Error for Typed {}
		let err = anyhow::Error::new(Typed).context("outer").note("why").help("do this");
		let copy = super::untyped(&err);
		assert!(find::<Typed>(&copy).is_none(), "the type is gone");
		assert_eq!(render(&copy, 0, Colour::Never), "error: outer: typed\n\nnote: why\nhelp: do this\n");
		assert_eq!(super::unlabelled(&err), "outer\ncaused by: typed\n\nnote: why\nhelp: do this");
	}

	#[test]
	fn an_error_with_no_hints_is_one_line() {
		let err = anyhow!("no USB-CAN adapter found");
		assert_eq!(render(&err, 0, Colour::Never), "error: no USB-CAN adapter found\n");
	}

	#[test]
	fn help_lines_follow_a_blank_line_in_the_order_given() {
		let err = anyhow!("no USB-CAN adapter found")
			.help("plug one in and check `vagcan devices`")
			.help("over Bluetooth, add `--ble`");
		assert_eq!(
			render(&err, 0, Colour::Never),
			"error: no USB-CAN adapter found\n\
			 \n\
			 help: plug one in and check `vagcan devices`\n\
			 help: over Bluetooth, add `--ble`\n"
		);
	}

	#[test]
	fn notes_come_before_help_whatever_the_order_they_were_attached_in() {
		let err = anyhow!("the car is moving (42 km/h)")
			.help("stop the car and run `vagcan adaptations 713` again")
			.note("adaptations are read only on a parked car");
		assert_eq!(
			render(&err, 0, Colour::Never),
			"error: the car is moving (42 km/h)\n\
			 \n\
			 note: adaptations are read only on a parked car\n\
			 help: stop the car and run `vagcan adaptations 713` again\n"
		);
	}

	#[test]
	fn a_cause_is_shown_when_it_adds_something_and_at_v_when_it_does_not() {
		let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
		let err = Err::<(), _>(io).context("reading /x/cache.sqlite").unwrap_err();
		assert_eq!(
			render(&err, 0, Colour::Never),
			"error: reading /x/cache.sqlite\ncaused by: permission denied\n"
		);
		// The message already quotes its cause: said once at level 0, the chain at `-v`.
		let io = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "permission denied");
		let err = Err::<(), _>(io).context("reading /x/cache.sqlite: permission denied").unwrap_err();
		assert_eq!(render(&err, 0, Colour::Never), "error: reading /x/cache.sqlite: permission denied\n");
		assert_eq!(
			render(&err, 1, Colour::Never),
			"error: reading /x/cache.sqlite: permission denied\ncaused by: permission denied\n"
		);
	}

	#[test]
	fn a_chain_with_hints_on_an_inner_error_keeps_both() {
		let inner = anyhow!("no cache for SK37X").help("run `vagcan setup`");
		let err = Err::<(), _>(inner)
			.context("opening the label cache")
			.note("the cache is made by `setup`")
			.unwrap_err();
		assert_eq!(
			render(&err, 0, Colour::Never),
			"error: opening the label cache\n\
			 caused by: no cache for SK37X\n\
			 \n\
			 note: the cache is made by `setup`\n\
			 help: run `vagcan setup`\n"
		);
	}

	#[test]
	fn colour_is_carried_by_the_labels_alone_and_only_when_asked() {
		let err = anyhow!("x").note("why").help("do");
		let painted = render(&err, 0, Colour::Always);
		assert_eq!(
			painted,
			"\x1b[1;31merror:\x1b[0m x\n\n\x1b[1;32mnote:\x1b[0m why\n\x1b[1;36mhelp:\x1b[0m do\n"
		);
		assert!(!render(&err, 0, Colour::Never).contains('\x1b'));
	}

	#[test]
	fn a_said_line_is_labelled_in_the_reports_colours() {
		assert_eq!(
			super::paint("warning: 713 not read\nnote: x\n", Colour::Always),
			"\x1b[1;33mwarning:\x1b[0m 713 not read\n\x1b[1;32mnote:\x1b[0m x\n"
		);
		assert_eq!(
			super::paint("  note: indented, not a label\n", Colour::Always),
			"  note: indented, not a label\n"
		);
		assert_eq!(super::paint("notes: not a label\n", Colour::Always), "notes: not a label\n");
		assert_eq!(super::paint("warning: x\n", Colour::Never), "warning: x\n");
	}

	#[test]
	fn colour_only_on_a_terminal_and_never_under_no_color() {
		assert_eq!(Colour::on(true, false), Colour::Always);
		assert_eq!(Colour::on(false, false), Colour::Never, "stderr into a file or a pipe");
		assert_eq!(Colour::on(true, true), Colour::Never, "NO_COLOR");
	}

	#[test]
	fn a_multi_line_hint_is_indented_under_its_text() {
		let err = anyhow!("several CAN adapters found").help("name one with `--device`:\n/dev/a\n/dev/b");
		assert_eq!(
			render(&err, 0, Colour::Never),
			"error: several CAN adapters found\n\nhelp: name one with `--device`:\n      /dev/a\n      /dev/b\n"
		);
	}

	#[test]
	fn the_wrapper_is_invisible_to_display_and_the_chain() {
		let io = std::io::Error::other("disk full");
		let err = Err::<(), _>(io).context("writing x").help("free some space").unwrap_err();
		assert_eq!(err.to_string(), "writing x");
		assert_eq!(format!("{err:#}"), "writing x: disk full");
		assert_eq!(err.chain().count(), 2);
		assert_eq!(hints(&err), vec![Hint::Help("free some space".into())]);
	}

	#[derive(Debug)]
	struct Typed;
	impl std::fmt::Display for Typed {
		fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
			f.write_str("typed")
		}
	}
	impl std::error::Error for Typed {}

	#[test]
	fn a_typed_error_is_found_through_hints_and_context() {
		let err = anyhow::Error::new(Typed).help("h").context("outer").help("again");
		assert!(err.downcast_ref::<Typed>().is_none(), "what `find` exists for");
		assert!(find::<Typed>(&err).is_some());
		assert!(find::<Typed>(&anyhow::Error::new(Typed)).is_some());
		assert!(find::<std::io::Error>(&err).is_none());
	}

	#[test]
	fn an_io_error_under_a_hint_is_still_seen() {
		let pipe = std::io::Error::from(std::io::ErrorKind::BrokenPipe);
		let err = anyhow::Error::new(pipe).help("h").context("writing");
		assert!(super::any::<std::io::Error>(&err, &|e| e.kind() == std::io::ErrorKind::BrokenPipe));
		assert!(!super::any::<std::io::Error>(&err, &|e| e.kind() == std::io::ErrorKind::NotFound));
		assert!(find::<std::io::Error>(&err).is_some());
	}
}
