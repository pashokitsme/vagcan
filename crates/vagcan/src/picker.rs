//! Choosing a file when the command was not given one.
//!
//! Several commands take a path that the person running them has to find first:
//! a session under a car, a recording `watch --out` wrote, a `.rod` inside a
//! VCDS installation. Finding it means leaving the tool, listing a directory
//! whose layout only this tool knows, and pasting the answer back. This module
//! is the alternative — a list, the arrow keys, and the thing itself.
//!
//! Nothing here knows what is being picked. It offers *entries of a directory*,
//! described well enough to tell apart and sorted by name, and hands back the
//! path. A level is a [`Level`]; two levels are two of them, and the level below
//! looks inside whatever the level above picked. `←` and `→` walk between them,
//! so a person who went into the wrong car can back out to the cars and in
//! again without losing the row they were on.
//!
//! **The input is behind [`Chooser`]** for the reason `measure::setup` puts its
//! interview behind a trait: the part worth testing — the order, the detail
//! beside each name, what an empty directory says, what a delete does to the
//! list — is the part a terminal makes untestable. [`Console`] is the person's;
//! `Scripted` is the tests'.
//!
//! A chooser reports a [`Decision`] and does not act on it. Deleting is done by
//! [`pick_path`], where a test can watch it happen against a temporary
//! directory with no terminal anywhere. The two exceptions are the two things
//! that *are* output — saying a line and revealing a path in the file manager —
//! and those stay on the chooser because that is what a chooser is for.
//!
//! **A pipe is not a terminal.** Every offline command in this tool works with
//! its output redirected, and a prompt that blocks on a stdin nobody is typing
//! into is a hang with no explanation. Raw mode makes that stricter rather than
//! looser: the terminal is checked *before* it is switched, and it is switched
//! back by a guard that runs on every way out of the list, error and panic
//! included.
//!
//! No new dependency: `crossterm` is already what `measure` and `watch` read
//! their keys with. There is no alternate screen — this is a list of eighteen
//! files, and the lines it prints while working (what was taken, what was
//! deleted) should still be on the screen afterwards.

// Not every level shape and helper here has a caller yet — the commands that
// will are wired separately, and a module written a command ahead of its
// callers is the shape `datadir` already uses for the same reason.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::Attribute;
use crossterm::terminal::{Clear, ClearType};
use crossterm::{cursor, execute, terminal};

/// One thing that can be picked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// What the caller gets back.
    pub path: PathBuf,
    /// What it is called, and what the list is ordered by.
    pub name: String,
    /// What is shown beside the name. Eighteen timestamps that look alike are
    /// not a choice, so a file carries its size and the day it was written and
    /// a directory carries how much is in it.
    pub detail: String,
}

/// Which entries of a directory belong on the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Files,
    Directories,
}

/// Which end of the list comes first.
///
/// Names sort as text, so a directory of timestamps sorts oldest first — right
/// for a car, whose name means nothing in order, and unkind for a shelf of
/// recordings where the one wanted is nearly always the last drive. Which it is
/// is the caller's to say rather than a rule hidden in here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    ByName,
    ByNameReversed,
}

/// One level of the choice: what is being picked, and out of what.
#[derive(Clone, Debug)]
pub struct Level<'a> {
    /// The singular noun for one entry — "car", "session", "recording". It is
    /// what the prompt asks for and what the refusals talk about, and it gains
    /// a plain `-s` in the plural, so a noun that does not is a noun to reword.
    pub what: &'a str,
    pub kind: Kind,
    pub order: Order,
    /// Only names ending in this, when it is not empty: `.json`, `.rod`.
    pub ending: &'a str,
    /// Where this level looks, relative to what the level above picked. Empty
    /// is that directory itself; `measures` is the subdirectory under a car.
    pub within: &'a str,
    /// What puts something here, for the level that finds nothing. An empty
    /// directory is the ordinary state before the first drive, and a person who
    /// is told only that it is empty has been told nothing they can act on.
    pub filled_by: &'a str,
}

impl<'a> Level<'a> {
    /// Pick a file. `what` is the singular noun for one of them.
    pub fn files(what: &'a str) -> Level<'a> {
        Level {
            what,
            kind: Kind::Files,
            order: Order::ByName,
            ending: "",
            within: "",
            filled_by: "",
        }
    }

    /// Pick a directory.
    pub fn directories(what: &'a str) -> Level<'a> {
        Level { kind: Kind::Directories, ..Level::files(what) }
    }

    /// Offer only names ending in `suffix`.
    #[must_use]
    pub fn ending(self, suffix: &'a str) -> Level<'a> {
        Level { ending: suffix, ..self }
    }

    /// Look in this subdirectory of whatever the level above picked.
    #[must_use]
    pub fn within(self, subdirectory: &'a str) -> Level<'a> {
        Level { within: subdirectory, ..self }
    }

    /// The command that would have put something here.
    #[must_use]
    pub fn filled_by(self, command: &'a str) -> Level<'a> {
        Level { filled_by: command, ..self }
    }

    /// Start at the far end of the alphabet — the newest, where the names are
    /// timestamps.
    #[must_use]
    pub fn newest_first(self) -> Level<'a> {
        Level { order: Order::ByNameReversed, ..self }
    }
}

/// What the person did to the list.
///
/// Every one of these is an *answer*, which is why an index is no longer enough
/// to carry one: "this one" and "never mind" were only ever two of five. The
/// index in the destructive and revealing ones is the row it was aimed at,
/// because a key acts on the highlight and the chooser is the only thing that
/// knows where the highlight is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    /// Take this row: `Enter`, or `→` on a level with one below it.
    Take(usize),
    /// Delete the file on this row — after asking, and only where a level
    /// offers files.
    Delete(usize),
    /// Put this row's path on the clipboard — `o`. The path is what a person
    /// actually wants: it pastes into a `cd`, an editor, a message.
    Copy(usize),
    /// Show this row where this system shows files — `O`.
    Reveal(usize),
    /// Up a level — `←`. At the top level there is nothing above, so it leaves.
    Back,
    /// Leave without picking anything — `q`, `Esc`, `Ctrl-C`.
    Quit,
}

/// Where the answer comes from.
///
/// Four things, which is what a list of files needs: ask which one, ask a
/// yes/no, say a line, show a path. `Err` from any of them is the end of the
/// conversation — a stdin that is not a terminal, or one that closed.
pub trait Chooser {
    /// Show `choices` with row `at` highlighted, and report what was done.
    ///
    /// The whole [`Level`] comes in because the keys depend on it: `d` belongs
    /// to a list of files and not to a list of cars, and the legend at the
    /// bottom has to say so.
    fn choose(&mut self, level: &Level<'_>, choices: &[Choice], at: usize) -> Result<Decision>;

    /// Ask before something irreversible. Anything short of a clear yes is a no.
    fn confirm(&mut self, question: &str) -> Result<bool>;

    /// Say one line: what was taken, what was deleted, why it was not.
    fn say(&mut self, line: &str) -> Result<()>;

    /// Put `path` on the clipboard, saying whether it got there.
    fn copy(&mut self, path: &Path) -> Result<()>;

    /// Show `path` where this system shows files.
    fn reveal(&mut self, path: &Path) -> Result<()>;
}

/// The chooser a person uses: a list on stdout, arrow keys off the terminal.
pub struct Console {
    /// What to tell someone whose stdin is a pipe — the argument that says the
    /// same thing without anybody at a keyboard. Passed in because the picker
    /// does not know which command it is serving.
    instead: String,
}

impl Console {
    /// `instead` is the command line that needs no picker, complete enough to
    /// paste: `vagcan measure view PATH`.
    pub fn new(instead: impl Into<String>) -> Console {
        Console { instead: instead.into() }
    }
}

impl Chooser for Console {
    fn choose(&mut self, level: &Level<'_>, choices: &[Choice], at: usize) -> Result<Decision> {
        // Before raw mode, not after: switching a terminal that is not there is
        // an errno, and this is the sentence that actually helps.
        if !std::io::stdin().is_terminal() {
            bail!(no_terminal(level.what, &self.instead));
        }
        let mut at = at.min(choices.len().saturating_sub(1));
        let mut out = std::io::stdout();
        // Everything below this line can leave through a `?` or a panic, and
        // both of those have to hand the terminal back the way they found it.
        let _raw = RawMode::enter()?;
        let mut drawn = 0usize;
        let decision = loop {
            // A terminal that answers 0 does not know how big it is — a pty
            // opened without a window size, which is what a pipe on the far end
            // gives you. Guessing 80x24 shows a list; believing the 0 shows one
            // row of it.
            let (width, height) = terminal::size().unwrap_or((0, 0));
            let lines = screen(
                level,
                choices,
                at,
                if height == 0 { 24 } else { height },
                if width == 0 { 80 } else { width },
            );
            redraw(&mut out, &mut drawn, &lines)?;
            let Event::Key(key) = event::read()? else { continue };
            // Windows sends a release for every press; acting on both moves the
            // highlight twice per keystroke.
            if key.kind != KeyEventKind::Press {
                continue;
            }
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            match key.code {
                KeyCode::Up => at = at.saturating_sub(1),
                KeyCode::Down => at = (at + 1).min(choices.len().saturating_sub(1)),
                KeyCode::Home => at = 0,
                KeyCode::End => at = choices.len().saturating_sub(1),
                // `→` on a file has nothing below it to go into, so it means
                // the same as Enter rather than nothing at all.
                KeyCode::Enter | KeyCode::Right => break Decision::Take(at),
                KeyCode::Left => break Decision::Back,
                KeyCode::Char('d') if level.kind == Kind::Files => break Decision::Delete(at),
                // Lower case for the thing wanted ten times as often, and the
                // shifted one for the thing that takes over the screen.
                KeyCode::Char('o') => break Decision::Copy(at),
                KeyCode::Char('O') => break Decision::Reveal(at),
                KeyCode::Char('q') | KeyCode::Esc => break Decision::Quit,
                // Raw mode swallows the interrupt, so the key has to be honoured
                // by hand or there is no way out of here.
                KeyCode::Char('c') if ctrl => break Decision::Quit,
                _ => {}
            }
        };
        // The list has done its job; leave the screen where the next line of
        // output belongs rather than under a stale list.
        erase(&mut out, drawn)?;
        Ok(decision)
    }

    fn confirm(&mut self, question: &str) -> Result<bool> {
        // Asked outside raw mode, so the answer echoes and a backspace works.
        if !std::io::stdin().is_terminal() {
            bail!(no_terminal("file to delete", &self.instead));
        }
        let mut out = std::io::stdout();
        write!(out, "{question}")?;
        out.flush()?;
        let mut line = String::new();
        // Zero bytes is Ctrl-D: no answer, and no answer to a question about
        // deleting evidence is a no.
        if std::io::stdin().read_line(&mut line)? == 0 {
            writeln!(out)?;
            return Ok(false);
        }
        Ok(matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes"))
    }

    fn say(&mut self, line: &str) -> Result<()> {
        let mut out = std::io::stdout();
        writeln!(out, "{line}")?;
        out.flush()?;
        Ok(())
    }

    fn copy(&mut self, path: &Path) -> Result<()> {
        let line = copied(path);
        self.say(&line)
    }

    fn reveal(&mut self, path: &Path) -> Result<()> {
        let line = revealed(path);
        self.say(&line)
    }
}

/// Raw mode, and the promise to give it back.
///
/// The list reads single keys, which means the terminal stops echoing and stops
/// line-buffering. Leaving it that way turns a shell into something that looks
/// broken, so the restore hangs off `Drop` rather than off remembering to call
/// it: `?` in the middle of the loop unwinds through this, and so does a panic.
struct RawMode;

impl RawMode {
    fn enter() -> Result<RawMode> {
        terminal::enable_raw_mode()?;
        let guard = RawMode;
        // Hidden after the guard exists, so that a failure here still restores.
        execute!(std::io::stdout(), cursor::Hide)?;
        Ok(guard)
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = execute!(std::io::stdout(), cursor::Show);
        let _ = terminal::disable_raw_mode();
    }
}

/// Put `lines` where the last draw was, and remember how tall it is.
///
/// Relative movement, not absolute: the terminal scrolls when the list reaches
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

/// Take the list back off the screen.
fn erase(out: &mut impl std::io::Write, drawn: usize) -> Result<()> {
    if drawn > 0 {
        execute!(out, cursor::MoveToPreviousLine(drawn as u16), Clear(ClearType::FromCursorDown))?;
        out.flush()?;
    }
    Ok(())
}

/// The chooser the tests use: the decisions go in, and everything it was shown
/// or asked to do comes back out to be asserted against.
///
/// Lives beside [`Console`] rather than in this module's own tests so that any
/// command's tests can drive its picker without a terminal.
#[cfg(test)]
pub struct Scripted {
    decisions: std::collections::VecDeque<Decision>,
    confirms: std::collections::VecDeque<bool>,
    /// Every list it was shown: the noun, then the names in the order they were
    /// offered.
    pub seen: Vec<(String, Vec<String>)>,
    /// The row that was highlighted when each of those lists appeared — the
    /// only way to tell that going back landed somebody where they left.
    pub highlights: Vec<usize>,
    /// Every line it was told to say.
    pub said: Vec<String>,
    /// Every path it was asked to show in the file manager.
    pub copied: Vec<PathBuf>,
    pub revealed: Vec<PathBuf>,
}

#[cfg(test)]
impl Scripted {
    pub fn new(decisions: impl IntoIterator<Item = Decision>) -> Scripted {
        Scripted {
            decisions: decisions.into_iter().collect(),
            confirms: std::collections::VecDeque::new(),
            seen: Vec::new(),
            highlights: Vec::new(),
            said: Vec::new(),
            copied: Vec::new(),
            revealed: Vec::new(),
        }
    }

    /// The answers to the yes/no questions, in the order they will be asked.
    #[must_use]
    pub fn confirming(mut self, answers: impl IntoIterator<Item = bool>) -> Scripted {
        self.confirms = answers.into_iter().collect();
        self
    }

    /// The names of the last list it was shown, in the order they were offered.
    pub fn last_names(&self) -> Vec<String> {
        self.seen.last().map(|(_, names)| names.clone()).unwrap_or_default()
    }

    /// Everything it was told to say, as one blob to look for a phrase in.
    pub fn all_said(&self) -> String {
        self.said.join("\n")
    }
}

#[cfg(test)]
impl Chooser for Scripted {
    fn choose(&mut self, level: &Level<'_>, choices: &[Choice], at: usize) -> Result<Decision> {
        self.seen.push((
            level.what.to_string(),
            choices.iter().map(|c| c.name.clone()).collect(),
        ));
        self.highlights.push(at);
        match self.decisions.pop_front() {
            Some(decision) => Ok(decision),
            None => bail!("the script ran out while choosing a {}", level.what),
        }
    }

    fn confirm(&mut self, question: &str) -> Result<bool> {
        self.said.push(question.to_string());
        match self.confirms.pop_front() {
            Some(answer) => Ok(answer),
            None => bail!("the script ran out while being asked {question:?}"),
        }
    }

    fn say(&mut self, line: &str) -> Result<()> {
        self.said.push(line.to_string());
        Ok(())
    }

    fn copy(&mut self, path: &Path) -> Result<()> {
        self.copied.push(path.to_path_buf());
        Ok(())
    }

    fn reveal(&mut self, path: &Path) -> Result<()> {
        self.revealed.push(path.to_path_buf());
        Ok(())
    }
}

/// Everything at `dir` that this level offers, described and in order.
///
/// A directory that cannot be read is one with nothing in it: for a picker the
/// two are the same answer, and [`nothing_here`] is a better thing to say about
/// either than an errno.
pub fn entries(dir: &Path, level: &Level<'_>) -> Vec<Choice> {
    let Ok(listing) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut choices: Vec<Choice> =
        listing.flatten().filter_map(|entry| describe(&entry.path(), level)).collect();
    choices.sort_by(|a, b| a.name.cmp(&b.name));
    if level.order == Order::ByNameReversed {
        choices.reverse();
    }
    choices
}

/// Pick one entry of `dir`, or `None` if the person left without one.
///
/// Nothing to pick from is an `Err` rather than a `None`: a command that
/// silently does nothing is indistinguishable from one that failed, and the
/// message is the useful half of this module for anyone before their first
/// drive.
pub fn pick(io: &mut impl Chooser, dir: &Path, level: &Level<'_>) -> Result<Option<PathBuf>> {
    let mut at = 0;
    // Nothing is above a lone level, so `←` and `q` mean the same thing here.
    match walk(io, dir, level, &mut at, false)? {
        Step::Down(path) => Ok(Some(path)),
        Step::Back | Step::Quit => Ok(None),
    }
}

/// Pick down a tree, one level at a time: a car, then a session in it.
///
/// `←` from a level below the first goes back up rather than giving up — the
/// wrong car is a thing to notice at the moment its sessions appear, and
/// starting the command again for it would be a poor answer. `←` at the first
/// level, and `q` anywhere, is "never mind", and `None`.
///
/// The row somebody was on is remembered per level, so a trip up to swap cars
/// and back down again does not cost them their place.
pub fn pick_path(
    io: &mut impl Chooser,
    root: &Path,
    levels: &[Level<'_>],
) -> Result<Option<PathBuf>> {
    let mut chosen: Vec<PathBuf> = Vec::new();
    let mut marks: Vec<usize> = vec![0; levels.len()];
    let mut at = 0;
    // A level reached by going back has to be asked rather than answered for:
    // the one-choice shortcut below would otherwise bounce somebody straight
    // out of the list they just asked to see.
    let mut from_below = false;
    while let Some(level) = levels.get(at) {
        // The level above, not the deepest thing picked so far: going back
        // leaves the deeper pick in place so that coming forward again can
        // notice it is the same one, and `last()` would then have this level
        // looking inside its own child.
        let parent = match at.checked_sub(1) {
            Some(above) => chosen[above].clone(),
            None => root.to_path_buf(),
        };
        let dir = if level.within.is_empty() { parent } else { parent.join(level.within) };
        let mut mark = marks[at];
        let step = walk(io, &dir, level, &mut mark, from_below)?;
        marks[at] = mark;
        match step {
            Step::Down(path) => {
                // A different car means the list below is a different list, and
                // the row somebody stopped on in the old one means nothing in it.
                if chosen.get(at) != Some(&path) {
                    for mark in marks.iter_mut().skip(at + 1) {
                        *mark = 0;
                    }
                }
                chosen.truncate(at);
                chosen.push(path);
                at += 1;
                from_below = false;
            }
            Step::Back if at == 0 => return Ok(None),
            Step::Back => {
                at -= 1;
                from_below = true;
            }
            Step::Quit => return Ok(None),
        }
    }
    Ok(chosen.pop())
}

/// Where one level ended.
enum Step {
    Down(PathBuf),
    Back,
    Quit,
}

/// One level, until it is left: ask, act, ask again.
///
/// `at` comes in as the row to highlight and goes out as the row that was last
/// touched, which is what makes a round trip through the level above keep its
/// place. Deleting and revealing come back here rather than ending the level —
/// they are things done *to* a list, and the list is still the question.
fn walk(
    io: &mut impl Chooser,
    dir: &Path,
    level: &Level<'_>,
    at: &mut usize,
    from_below: bool,
) -> Result<Step> {
    let mut first = true;
    loop {
        let choices = entries(dir, level);
        if choices.is_empty() {
            // Empty on arrival is the state before the first drive and worth an
            // error. Empty because the last file was just deleted is not a
            // fault at all — say so and go back up to where there is something.
            if first {
                bail!(nothing_here(dir, level));
            }
            io.say(&nothing_here(dir, level))?;
            return Ok(Step::Back);
        }
        // One choice is not a question. Every car directory is named for its
        // VIN, and most people have one car — asking them to confirm the only
        // possible answer is ceremony. Only on arrival, though: a list that has
        // just been cut down to one by a delete is still a list.
        if first && !from_below && choices.len() == 1 {
            let only = &choices[0];
            io.say(&format!(
                "the only {}: {} ({})",
                level.what, only.name, only.detail
            ))?;
            *at = 0;
            return Ok(Step::Down(only.path.clone()));
        }
        if *at >= choices.len() {
            *at = choices.len() - 1;
        }
        first = false;
        let decision = io.choose(level, &choices, *at)?;
        // A chooser that names a row outside the list has named nothing; ask
        // again rather than index into a panic.
        let aimed = match decision {
            Decision::Take(row) | Decision::Delete(row) | Decision::Copy(row)
            | Decision::Reveal(row) => {
                match choices.get(row) {
                    Some(choice) => {
                        *at = row;
                        choice.clone()
                    }
                    None => continue,
                }
            }
            Decision::Back => return Ok(Step::Back),
            Decision::Quit => return Ok(Step::Quit),
        };
        match decision {
            Decision::Take(_) => return Ok(Step::Down(aimed.path)),
            Decision::Copy(_) => io.copy(&aimed.path)?,
            Decision::Reveal(_) => io.reveal(&aimed.path)?,
            Decision::Delete(_) => delete(io, level, &aimed)?,
            Decision::Back | Decision::Quit => unreachable!("answered above"),
        }
    }
}

/// Delete the file a row names — after saying exactly what will be lost.
///
/// This is the one thing in this module that changes anything, and what it
/// changes is evidence: a session is a drive that cannot be recorded again. So
/// it is bounded on every side — one file, named in full, in a list of files,
/// never through a link, and always with an answer about what happened.
fn delete(io: &mut impl Chooser, level: &Level<'_>, choice: &Choice) -> Result<()> {
    // A directory level lists cars, and a car directory is a whole history. `d`
    // is one key away from `s`, and there is no undo behind it.
    if level.kind != Kind::Files {
        io.say(&format!(
            "`d` deletes one file. {} is a {}, and a whole {} is not something this list \
             will remove.",
            choice.name, level.what, level.what
        ))?;
        return Ok(());
    }
    // `symlink_metadata` looks at the entry, not at what it points at. A link
    // in a directory of sessions is not a session, and following one is how a
    // delete reaches somewhere nobody was looking.
    match std::fs::symlink_metadata(&choice.path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            io.say(&format!(
                "{} is a link, not a file in this folder — it is left alone rather than \
                 followed out of it.",
                choice.path.display()
            ))?;
            return Ok(());
        }
        Ok(meta) if !meta.is_file() => {
            io.say(&format!("{} is not a file — left alone.", choice.path.display()))?;
            return Ok(());
        }
        Ok(_) => {}
        Err(why) => {
            io.say(&format!("could not look at {}: {why}", choice.path.display()))?;
            return Ok(());
        }
    }
    let question = format!(
        "delete {} ({})?\nA {} cannot be recorded again. [y/N] ",
        choice.path.display(),
        choice.detail,
        level.what
    );
    if !io.confirm(&question)? {
        io.say(&format!("kept {}", choice.path.display()))?;
        return Ok(());
    }
    match std::fs::remove_file(&choice.path) {
        Ok(()) => io.say(&format!("deleted {}", choice.path.display()))?,
        // The errno alone is useless in a list of eighteen alike names, so the
        // file is named beside it.
        Err(why) => io.say(&format!("could not delete {}: {why}", choice.path.display()))?,
    }
    Ok(())
}

/// What `o` does: the path onto the clipboard, and onto the screen either way.
///
/// The path is what a person is actually after — it pastes into a `cd`, an
/// editor, a message to somebody. `pbcopy` on macOS, `wl-copy` or `xclip`
/// where they exist; where none does, the path is printed and that is the whole
/// of the answer rather than a failure. It is written to the copier's stdin,
/// never spliced into a command line: these names come out of a directory named
/// after a VIN read off the bus.
fn copied(path: &Path) -> String {
    let target = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let shown = target.display();
    match to_clipboard(&target) {
        Some(with) => format!("{shown}\ncopied to the clipboard (by {with})"),
        None => format!("{shown}\n(nothing on this system copies for me — select the line above)"),
    }
}

/// The first clipboard program that takes the path, and its name.
fn to_clipboard(path: &Path) -> Option<&'static str> {
    use std::io::Write as _;
    use std::process::{Command, Stdio};
    for (program, args) in
        [("pbcopy", &[][..]), ("wl-copy", &[]), ("xclip", &["-selection", "clipboard"])]
    {
        let Ok(mut child) = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut pipe| pipe.write_all(path.as_os_str().as_encoded_bytes()).is_ok());
        if child.wait().is_ok_and(|status| status.success()) && wrote {
            return Some(program);
        }
    }
    None
}

/// What `O` honestly does.
///
/// Not `cd`: a child process cannot move its parent shell, which is a property
/// of how processes work and not a gap to paper over. The nearest true thing on
/// macOS is Finder's reveal, which puts the folder on screen with the file
/// selected; everywhere else the honest answer is to print the folder so it can
/// be pasted.
fn revealed(path: &Path) -> String {
    let folder = folder_line(path);
    if !cfg!(target_os = "macos") {
        return folder;
    }
    // The path is an argument, never a fragment of a command line: these names
    // come from a directory named after a VIN read off the bus, and nothing off
    // the bus gets to choose what this tool runs. No shell is spawned, so there
    // is nothing to quote for.
    let target = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match std::process::Command::new("/usr/bin/open").args(reveal_args(&target)).status() {
        Ok(status) if status.success() => format!("revealed in Finder: {}", target.display()),
        Ok(status) => format!("Finder did not open ({status}). {folder}"),
        Err(why) => format!("could not reveal {}: {why}\n{folder}", target.display()),
    }
}

/// The fallback, and the half of the answer that is true everywhere: the folder
/// itself, spelled out so it can be pasted into whatever the person uses.
fn folder_line(path: &Path) -> String {
    format!(
        "the folder is {}\n(a program cannot change the directory of the shell that started it)",
        path.parent().unwrap_or(path).display()
    )
}

/// The arguments of the reveal, as values.
///
/// Split out so a test can prove the path stays one argument no matter what the
/// file is called.
fn reveal_args(path: &Path) -> Vec<std::ffi::OsString> {
    vec![std::ffi::OsString::from("-R"), path.as_os_str().to_os_string()]
}

/// One entry, or `None` when it is not the kind being picked.
///
/// Names beginning with a dot are left out: `.DS_Store` is not a session, and
/// nothing this tool writes is hidden.
fn describe(path: &Path, level: &Level<'_>) -> Option<Choice> {
    let name = path.file_name()?.to_str()?.to_string();
    if name.starts_with('.') {
        return None;
    }
    let meta = std::fs::metadata(path).ok()?;
    let detail = match level.kind {
        Kind::Directories => {
            if !meta.is_dir() {
                return None;
            }
            held(path)
        }
        Kind::Files => {
            if !meta.is_file() || !name.ends_with(level.ending) {
                return None;
            }
            match modified(&meta) {
                Some(day) => format!("{}, {day}", size(meta.len())),
                None => size(meta.len()),
            }
        }
    };
    Some(Choice { path: path.to_path_buf(), name, detail })
}

/// How much is in a directory, for the person deciding whether it is the one.
fn held(dir: &Path) -> String {
    match std::fs::read_dir(dir).map(Iterator::count) {
        Ok(0) | Err(_) => "empty".to_string(),
        Ok(1) => "1 entry".to_string(),
        Ok(n) => format!("{n} entries"),
    }
}

/// A size in the units the number is readable in. Decimal, because the labels
/// say so.
fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} B"),
        1_000..1_000_000 => format!("{:.0} kB", bytes as f64 / 1_000.0),
        _ => format!("{:.1} MB", bytes as f64 / 1_000_000.0),
    }
}

/// When it was written, in the owner's own time zone — the day they would
/// remember driving.
fn modified(meta: &std::fs::Metadata) -> Option<String> {
    let at: chrono::DateTime<chrono::Local> = meta.modified().ok()?.into();
    Some(at.format("%Y-%m-%d %H:%M").to_string())
}

/// Which slice of a long list is on screen, given where the highlight is.
///
/// The highlight is kept in the middle when there is list on both sides of it,
/// and the window stops at the ends rather than scrolling past them.
fn window(len: usize, at: usize, room: usize) -> (usize, usize) {
    if len <= room {
        return (0, len);
    }
    let start = at.saturating_sub(room / 2).min(len - room);
    (start, room)
}

/// The list as a person sees it, one string per screen line.
///
/// Pure, and given its own size rather than asking the terminal for one: what
/// this draws is most of what this module is, and none of it should need a
/// terminal to test.
fn screen(
    level: &Level<'_>,
    choices: &[Choice],
    at: usize,
    height: u16,
    width: u16,
) -> Vec<String> {
    let width = (width as usize).max(20);
    // A title and a legend, and whatever is left over is list.
    let room = (height as usize).saturating_sub(2).max(1);
    let truncated = choices.len() > room;
    // When the list does not fit, one of its rows goes to saying so.
    let room = if truncated { room.saturating_sub(1).max(1) } else { room };
    let (start, count) = window(choices.len(), at, room);

    let mut lines = vec![clip(&format!("pick a {} ({})", level.what, choices.len()), width)];
    let name_width = choices.iter().map(|c| c.name.chars().count()).max().unwrap_or(0).min(40);
    for (row, choice) in choices.iter().enumerate().skip(start).take(count) {
        let text = clip(
            &format!(
                "{} {:<name_width$}  {}",
                if row == at { "❯" } else { " " },
                choice.name,
                choice.detail
            ),
            width,
        );
        // Reverse video rather than a colour: it survives every theme, and the
        // marker in front of it survives a terminal with no attributes at all.
        lines.push(match row == at {
            true => format!("{}{text}{}", Attribute::Reverse, Attribute::Reset),
            false => text,
        });
    }
    if truncated {
        lines.push(clip(
            &format!("  … {}-{} of {}", start + 1, start + count, choices.len()),
            width,
        ));
    }
    lines.push(clip(&keys(level), width));
    lines
}

/// The legend. A key nobody can discover is not a feature.
fn keys(level: &Level<'_>) -> String {
    let mut out = String::from("↑↓ move   ⏎ open");
    if level.kind == Kind::Directories {
        let _ = write!(out, "   → into");
    }
    let _ = write!(out, "   ← back");
    if level.kind == Kind::Files {
        let _ = write!(out, "   d delete");
    }
    // Never "cd": see `copied` and `revealed`. The word on screen is the word
    // for what actually happens.
    let _ = write!(
        out,
        "   o copy path   O {}   q quit",
        match cfg!(target_os = "macos") {
            true => "in Finder",
            false => "show folder",
        }
    );
    out
}

/// Cut a line to the width of the screen.
///
/// A line that wraps counts as two rows, and the redraw moves the cursor back
/// by the number of rows it thinks it wrote — so one wrapped name would smear
/// the list up the screen.
fn clip(line: &str, width: usize) -> String {
    match line.chars().count() > width {
        true => line.chars().take(width.saturating_sub(1)).collect::<String>() + "…",
        false => line.to_string(),
    }
}

/// Nothing to pick from — which before the first drive is the ordinary state of
/// affairs and not a fault, so it says what would have put something here.
fn nothing_here(dir: &Path, level: &Level<'_>) -> String {
    let mut out = format!("no {}s to choose from in {}", level.what, dir.display());
    if !level.filled_by.is_empty() {
        let _ = write!(out, "\n    {}", level.filled_by);
    }
    out
}

/// There is nobody at a keyboard, so the question cannot be asked.
///
/// A picker that blocks on a redirected stdin is a hang with no explanation, and
/// the thing the person wanted is one argument away.
fn no_terminal(what: &str, instead: &str) -> String {
    format!(
        "there is no terminal to choose a {what} at — stdin is redirected, so nobody would \
         see the list.\nName it on the command line instead:\n    {instead}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique-per-test temp dir, cleaned up on drop — the shape the rest of
    /// this crate's file tests use. Nothing here may write inside a checkout,
    /// and nothing here may touch a real `~/.vagcan`.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let path = std::env::temp_dir().join(format!(
                "vagcan-picker-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }

        fn file(&self, name: &str, bytes: usize) -> PathBuf {
            let path = self.0.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, "x".repeat(bytes)).unwrap();
            path
        }

        fn dir(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::create_dir_all(&path).unwrap();
            path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(choices: &[Choice]) -> Vec<&str> {
        choices.iter().map(|c| c.name.as_str()).collect()
    }

    /// Two cars with two sessions each: enough that no level answers itself.
    fn two_cars(tag: &str) -> TempDir {
        let cars = TempDir::new(tag);
        cars.file("XW8-first/measures/2026-08-04-1241.json", 10);
        cars.file("XW8-first/measures/2026-08-05-0900.json", 10);
        cars.file("XW8-second/measures/2026-07-19-0902.json", 10);
        cars.file("XW8-second/measures/2026-08-01-1800.json", 10);
        cars
    }

    fn car_then_session<'a>() -> [Level<'a>; 2] {
        [Level::directories("car"), Level::files("session").within("measures").ending(".json")]
    }

    #[test]
    fn a_list_is_ordered_by_name() {
        let dir = TempDir::new("order");
        for name in ["2026-08-04-1241.json", "2026-07-19-0902.json", "2026-08-04-0033.json"] {
            dir.file(name, 10);
        }
        assert_eq!(
            names(&entries(&dir.0, &Level::files("session"))),
            ["2026-07-19-0902.json", "2026-08-04-0033.json", "2026-08-04-1241.json"]
        );
    }

    #[test]
    fn a_list_of_timestamps_can_be_asked_for_newest_first() {
        // Names sort oldest first, and the drive somebody wants is nearly
        // always the last one — but that is the caller's judgement to make.
        let dir = TempDir::new("newest");
        for name in ["2026-07-19-0902.json", "2026-08-04-1241.json"] {
            dir.file(name, 10);
        }
        assert_eq!(
            names(&entries(&dir.0, &Level::files("recording").newest_first())),
            ["2026-08-04-1241.json", "2026-07-19-0902.json"]
        );
    }

    #[test]
    fn every_file_on_the_list_carries_its_size_and_the_day_it_was_written() {
        let dir = TempDir::new("detail");
        dir.file("2026-08-04-1241.json", 4_200);
        let listed = entries(&dir.0, &Level::files("session"));
        let detail = &listed[0].detail;
        assert!(detail.starts_with("4 kB, "), "{detail}");
        // Enough of a date to tell one drive from another, not a full timestamp.
        assert_eq!(detail.split(", ").nth(1).unwrap().len(), "2026-08-04 12:41".len(), "{detail}");
    }

    #[test]
    fn a_directory_says_how_much_is_in_it() {
        let dir = TempDir::new("held");
        dir.dir("empty-one");
        dir.file("full-one/car.json", 10);
        dir.file("full-one/note.txt", 10);
        let listed = entries(&dir.0, &Level::directories("car"));
        assert_eq!(names(&listed), ["empty-one", "full-one"]);
        assert_eq!(listed[0].detail, "empty");
        assert_eq!(listed[1].detail, "2 entries");
    }

    #[test]
    fn only_the_kind_that_was_asked_for_is_offered() {
        let dir = TempDir::new("kind");
        dir.file("2026-08-04.json", 10);
        dir.file("2026-08-04.html", 10);
        dir.file(".DS_Store", 10);
        dir.dir("measures");
        assert_eq!(
            names(&entries(&dir.0, &Level::files("session").ending(".json"))),
            ["2026-08-04.json"]
        );
        assert_eq!(names(&entries(&dir.0, &Level::directories("car"))), ["measures"]);
    }

    #[test]
    fn nothing_to_choose_from_says_what_would_have_recorded_one() {
        // The state before the first drive. "No such file" is a worse answer.
        let dir = TempDir::new("empty");
        let level = Level::files("recording")
            .ending(".csv")
            .filled_by("vagcan watch --out drive.csv   records one");
        let mut io = Scripted::new([]);
        let why = pick(&mut io, &dir.0, &level).unwrap_err().to_string();
        assert!(why.contains("no recordings"), "{why}");
        assert!(why.contains("vagcan watch --out"), "{why}");
        assert!(io.seen.is_empty(), "nobody was asked to choose from an empty list");
    }

    #[test]
    fn a_directory_that_does_not_exist_yet_reads_as_an_empty_one() {
        // Before the first drive there is no `measures/` at all, and that is
        // the same state as an empty one, not an error about a missing path.
        let dir = TempDir::new("missing");
        let level = Level::files("session").filled_by("vagcan measure");
        let why = pick(&mut Scripted::new([]), &dir.0.join("measures"), &level)
            .unwrap_err()
            .to_string();
        assert!(why.contains("no sessions"), "{why}");
        assert!(why.contains("measures"), "the path is named: {why}");
    }

    #[test]
    fn picking_a_car_then_a_session_looks_inside_the_car_that_was_picked() {
        let cars = two_cars("nested");
        let mut io = Scripted::new([Decision::Take(1), Decision::Take(0)]);
        let picked = pick_path(&mut io, &cars.0, &car_then_session()).unwrap();
        assert_eq!(picked, Some(cars.0.join("XW8-second/measures/2026-07-19-0902.json")));
        assert_eq!(io.seen[0].0, "car");
        assert_eq!(io.last_names(), ["2026-07-19-0902.json", "2026-08-01-1800.json"]);
    }

    #[test]
    fn the_left_arrow_from_the_sessions_offers_the_cars_again() {
        // The wrong car is a thing you notice when its sessions appear, and
        // starting the command over would be a poor way to say so.
        let cars = two_cars("back");
        let mut io = Scripted::new([
            Decision::Take(0),
            Decision::Back,
            Decision::Take(1),
            Decision::Take(0),
        ]);
        let picked = pick_path(&mut io, &cars.0, &car_then_session()).unwrap();
        assert_eq!(picked, Some(cars.0.join("XW8-second/measures/2026-07-19-0902.json")));
        let asked: Vec<&str> = io.seen.iter().map(|(what, _)| what.as_str()).collect();
        assert_eq!(asked, ["car", "session", "car", "session"]);
    }

    #[test]
    fn the_left_arrow_at_the_first_level_picks_nothing() {
        let cars = two_cars("never-mind");
        let picked =
            pick_path(&mut Scripted::new([Decision::Back]), &cars.0, &car_then_session()).unwrap();
        assert_eq!(picked, None);
    }

    #[test]
    fn quitting_from_a_level_below_the_first_picks_nothing_rather_than_going_up() {
        // `←` is one level; `q` is the whole thing. They are different answers.
        let cars = two_cars("quit");
        let mut io = Scripted::new([Decision::Take(0), Decision::Quit]);
        assert_eq!(pick_path(&mut io, &cars.0, &car_then_session()).unwrap(), None);
        assert_eq!(io.seen.len(), 2, "it did not go back up to ask again");
    }

    #[test]
    fn the_highlighted_row_survives_a_trip_up_to_the_cars_and_back_down() {
        // Somebody going back to swap cars and forward again should not have to
        // find their place a second time.
        let cars = two_cars("highlight");
        let mut io = Scripted::new([
            Decision::Take(1),
            Decision::Back,
            Decision::Take(1),
            Decision::Take(0),
        ]);
        pick_path(&mut io, &cars.0, &car_then_session()).unwrap();
        // car(0) → session(0) → car again, highlighted where it was left.
        assert_eq!(io.highlights, [0, 0, 1, 0]);
    }

    #[test]
    fn a_level_with_one_choice_is_taken_without_asking_and_said_out_loud() {
        // After every car directory is named for its VIN, one car is the common
        // case, and a list of one is not a question worth asking.
        let cars = TempDir::new("only");
        cars.file("XW8-only/measures/2026-08-04-1241.json", 10);
        cars.file("XW8-only/measures/2026-08-05-0900.json", 10);
        let mut io = Scripted::new([Decision::Take(0)]);
        let picked = pick_path(&mut io, &cars.0, &car_then_session()).unwrap();
        assert_eq!(picked, Some(cars.0.join("XW8-only/measures/2026-08-04-1241.json")));
        let asked: Vec<&str> = io.seen.iter().map(|(what, _)| what.as_str()).collect();
        assert_eq!(asked, ["session"], "the one car was never put to a vote");
        assert!(io.all_said().contains("the only car: XW8-only"), "{:?}", io.said);
    }

    #[test]
    fn going_back_to_a_level_with_one_choice_shows_it_rather_than_leaving() {
        // Otherwise `←` from the sessions of the only car would drop straight
        // out of the picker, and the list somebody asked for never appears.
        let cars = TempDir::new("only-back");
        cars.file("XW8-only/measures/2026-08-04-1241.json", 10);
        cars.file("XW8-only/measures/2026-08-05-0900.json", 10);
        let mut io = Scripted::new([Decision::Back, Decision::Quit]);
        assert_eq!(pick_path(&mut io, &cars.0, &car_then_session()).unwrap(), None);
        let asked: Vec<&str> = io.seen.iter().map(|(what, _)| what.as_str()).collect();
        assert_eq!(asked, ["session", "car"]);
        assert_eq!(io.last_names(), ["XW8-only"]);
    }

    #[test]
    fn deleting_a_session_removes_the_file_and_the_list_comes_back_without_it() {
        let cars = TempDir::new("delete");
        let doomed = cars.file("XW8-only/measures/2026-08-04-1241.json", 10);
        cars.file("XW8-only/measures/2026-08-05-0900.json", 10);
        let mut io = Scripted::new([Decision::Delete(0), Decision::Take(0)]).confirming([true]);
        let picked = pick_path(&mut io, &cars.0, &car_then_session()).unwrap();
        assert!(!doomed.exists(), "the file is gone");
        assert_eq!(picked, Some(cars.0.join("XW8-only/measures/2026-08-05-0900.json")));
        assert_eq!(io.last_names(), ["2026-08-05-0900.json"], "the list refreshed");
        assert!(io.all_said().contains(&format!("deleted {}", doomed.display())), "{:?}", io.said);
    }

    #[test]
    fn a_delete_asks_first_and_names_the_whole_path_and_what_is_lost() {
        let cars = TempDir::new("delete-asks");
        let kept = cars.file("XW8-only/measures/2026-08-04-1241.json", 4_200);
        cars.file("XW8-only/measures/2026-08-05-0900.json", 10);
        let mut io = Scripted::new([Decision::Delete(0), Decision::Quit]).confirming([false]);
        pick_path(&mut io, &cars.0, &car_then_session()).unwrap();
        assert!(kept.exists(), "a no keeps the file");
        let asked = io.all_said();
        assert!(asked.contains(&kept.display().to_string()), "{asked}");
        assert!(asked.contains("4 kB"), "it says what is about to be lost: {asked}");
        assert!(asked.contains("cannot be recorded again"), "{asked}");
        assert!(asked.contains(&format!("kept {}", kept.display())), "{asked}");
    }

    #[test]
    fn deleting_the_last_row_leaves_the_highlight_on_the_new_last_row() {
        // A naive index would sit one past the end of the shortened list.
        let cars = TempDir::new("delete-last");
        for name in ["a.json", "b.json", "c.json"] {
            cars.file(&format!("XW8-only/measures/{name}"), 10);
        }
        let mut io = Scripted::new([Decision::Delete(2), Decision::Quit]).confirming([true]);
        pick_path(&mut io, &cars.0, &car_then_session()).unwrap();
        assert_eq!(io.last_names(), ["a.json", "b.json"]);
        assert_eq!(io.highlights.last(), Some(&1));
    }

    #[test]
    fn deleting_the_only_session_left_says_so_and_goes_back_up_to_the_cars() {
        // An empty list is an error on arrival and an ordinary outcome after a
        // delete — the person did it on purpose and knows what is missing.
        let cars = TempDir::new("delete-all");
        cars.file("XW8-only/measures/a.json", 10);
        cars.file("XW8-only/measures/b.json", 10);
        let mut io = Scripted::new([Decision::Delete(0), Decision::Delete(0), Decision::Quit])
            .confirming([true, true]);
        assert_eq!(pick_path(&mut io, &cars.0, &car_then_session()).unwrap(), None);
        assert!(io.all_said().contains("no sessions to choose from"), "{:?}", io.said);
        let asked: Vec<&str> = io.seen.iter().map(|(what, _)| what.as_str()).collect();
        assert_eq!(asked, ["session", "session", "car"], "it went back up rather than erroring");
    }

    #[test]
    fn a_car_cannot_be_deleted_because_a_car_holds_a_whole_history() {
        // `d` on a directory level is one keystroke from `s`, and behind it is
        // every drive ever recorded for that car.
        let cars = two_cars("delete-car");
        let mut io = Scripted::new([Decision::Delete(0), Decision::Quit]);
        assert_eq!(pick_path(&mut io, &cars.0, &car_then_session()).unwrap(), None);
        assert!(cars.0.join("XW8-first").exists(), "the car is untouched");
        assert!(io.all_said().contains("`d` deletes one file"), "{:?}", io.said);
    }

    #[test]
    fn a_delete_that_the_filesystem_refuses_names_the_file_it_could_not_remove() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let cars = TempDir::new("delete-denied");
            let doomed = cars.file("XW8-only/measures/a.json", 10);
            cars.file("XW8-only/measures/b.json", 10);
            let measures = cars.0.join("XW8-only/measures");
            // A directory nobody may write is a directory nothing may be
            // unlinked from — the ordinary shape of a permission error.
            std::fs::set_permissions(&measures, std::fs::Permissions::from_mode(0o555)).unwrap();
            let mut io = Scripted::new([Decision::Delete(0), Decision::Quit]).confirming([true]);
            let walked = pick_path(&mut io, &cars.0, &car_then_session());
            std::fs::set_permissions(&measures, std::fs::Permissions::from_mode(0o755)).unwrap();
            walked.unwrap();
            assert!(doomed.exists(), "it is still there");
            let said = io.all_said();
            assert!(said.contains(&format!("could not delete {}", doomed.display())), "{said}");
            assert!(said.contains("denied") || said.contains("permission"), "{said}");
        }
    }

    #[test]
    fn a_link_is_left_alone_rather_than_followed_out_of_the_folder() {
        #[cfg(unix)]
        {
            let cars = TempDir::new("delete-link");
            let outside = cars.file("elsewhere.json", 10);
            cars.dir("XW8-only/measures");
            let link = cars.0.join("XW8-only/measures/a-link.json");
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            cars.file("XW8-only/measures/b.json", 10);
            let mut io = Scripted::new([Decision::Delete(0), Decision::Quit]).confirming([]);
            pick_path(&mut io, &cars.0, &car_then_session()).unwrap();
            assert!(outside.exists(), "what the link pointed at is untouched");
            assert!(link.symlink_metadata().is_ok(), "and so is the link");
            assert!(io.all_said().contains("is a link"), "{:?}", io.said);
        }
    }

    #[test]
    fn revealing_hands_over_the_path_of_the_highlighted_row() {
        let cars = two_cars("reveal");
        let mut io = Scripted::new([Decision::Reveal(1), Decision::Quit]);
        pick_path(&mut io, &cars.0, &car_then_session()).unwrap();
        assert_eq!(io.revealed, [cars.0.join("XW8-second")]);
        assert_eq!(io.seen.len(), 2, "revealing does not end the list");
    }

    #[test]
    fn copying_hands_over_the_path_and_leaves_the_list_up() {
        // `o` is the one wanted often — a path pastes into a `cd`, an editor, a
        // message — so it is the unshifted key, and like a reveal it answers the
        // question without ending the list.
        let cars = two_cars("copy");
        let mut io = Scripted::new([Decision::Copy(1), Decision::Quit]);
        pick_path(&mut io, &cars.0, &car_then_session()).unwrap();
        assert_eq!(io.copied, [cars.0.join("XW8-second")]);
        assert!(io.revealed.is_empty(), "copying is not revealing");
        assert_eq!(io.seen.len(), 2, "copying does not end the list");
    }

    #[test]
    fn the_legend_names_both_keys_and_promises_neither_a_cd_nor_a_copy_it_cannot_do() {
        let legend = keys(&Level::files("session"));
        assert!(legend.contains("o copy path"), "{legend}");
        assert!(legend.contains("O "), "the shifted key is discoverable too: {legend}");
        assert!(!legend.contains("cd"), "{legend}");
    }

    #[test]
    fn a_path_that_could_not_be_copied_is_still_shown() {
        // A system with no clipboard program is not a failure: the path on the
        // screen is the whole of the answer, and saying so beats an error.
        let line = copied(Path::new("/cars/XW8/measures/a.json"));
        assert!(line.contains("/cars/XW8/measures/a.json"), "{line}");
        assert!(
            line.contains("copied to the clipboard") || line.contains("select the line above"),
            "{line}"
        );
    }

    #[test]
    fn a_reveal_never_lets_a_file_name_choose_what_runs() {
        // The names here come from a directory named after a VIN read off the
        // bus. Nothing off the bus gets to be part of a command line.
        let path = Path::new("/tmp/cars/XW8; rm -rf ~/`whoami`/measures/a.json");
        let args = reveal_args(path);
        assert_eq!(args.len(), 2, "a flag and a path, and nothing parsed out of the path");
        assert_eq!(args[1], path.as_os_str());
    }

    #[test]
    fn revealing_says_where_the_folder_is_rather_than_pretending_to_cd() {
        // A child process cannot move its parent shell, so neither the screen
        // nor the legend may promise a `cd`. Tested on the words rather than by
        // calling `revealed`, which would put a Finder window on somebody's
        // screen in the middle of `cargo test`.
        let line = folder_line(Path::new("/cars/XW8/measures/a.json"));
        assert!(line.contains("the folder is /cars/XW8/measures"), "{line}");
        assert!(line.contains("cannot change the directory"), "{line}");
        assert!(!keys(&Level::files("session")).contains("cd"), "the legend does not say cd");
    }

    #[test]
    fn with_no_terminal_the_refusal_names_the_argument_to_pass_instead() {
        // A prompt on a redirected stdin is a hang nobody can diagnose.
        let why = no_terminal("session", "vagcan measure view PATH");
        assert!(why.contains("no terminal"), "{why}");
        assert!(why.contains("vagcan measure view PATH"), "{why}");
    }

    #[test]
    fn the_list_a_person_sees_marks_one_row_and_shows_the_keys_that_work_on_it() {
        let dir = TempDir::new("render");
        dir.file("2026-08-04-1241.json", 4_200);
        dir.file("2026-08-05-0900.json", 10);
        let level = Level::files("session");
        let drawn = screen(&level, &entries(&dir.0, &level), 1, 24, 80).join("\n");
        assert!(drawn.contains("pick a session (2)"), "{drawn}");
        assert!(drawn.contains("  2026-08-04-1241.json  4 kB, "), "{drawn}");
        assert!(drawn.contains("❯ 2026-08-05-0900.json"), "the second row is the one: {drawn}");
        assert!(drawn.contains("↑↓ move"), "{drawn}");
        assert!(drawn.contains("d delete"), "a file can be deleted: {drawn}");
        assert!(drawn.contains("← back"), "{drawn}");
        assert!(drawn.contains("q quit"), "{drawn}");
    }

    #[test]
    fn a_list_of_cars_offers_going_into_one_and_does_not_offer_deleting_one() {
        let dir = TempDir::new("render-cars");
        dir.dir("XW8-first");
        let level = Level::directories("car");
        let drawn = screen(&level, &entries(&dir.0, &level), 0, 24, 80).join("\n");
        assert!(drawn.contains("→ into"), "{drawn}");
        assert!(!drawn.contains("d delete"), "a car is not deletable: {drawn}");
    }

    #[test]
    fn a_list_taller_than_the_screen_scrolls_to_keep_the_highlight_in_view() {
        let dir = TempDir::new("render-tall");
        for n in 0..30 {
            dir.file(&format!("2026-08-{:02}-1200.json", n + 1), 10);
        }
        let level = Level::files("session");
        let choices = entries(&dir.0, &level);
        let drawn = screen(&level, &choices, 25, 12, 80);
        // Title, rows, the "how far in" line, the legend — and nothing that
        // would wrap past the bottom and smear the next redraw.
        assert!(drawn.len() <= 12, "{} lines in 12 rows", drawn.len());
        let body = drawn.join("\n");
        assert!(body.contains("❯ 2026-08-26-1200.json"), "{body}");
        assert!(body.contains("of 30"), "it says how far into the list this is: {body}");
    }

    #[test]
    fn a_name_wider_than_the_screen_is_cut_rather_than_wrapped() {
        // A wrapped line is two rows on screen and one row in the redraw's
        // count, which walks the list up the terminal a line at a time.
        let cut = clip(&"x".repeat(200), 40);
        assert_eq!(cut.chars().count(), 40);
        assert!(cut.ends_with('…'));
    }
}
