//! Choosing a file when the command was not given one.
//!
//! Several commands take a path that the person running them has to find first:
//! a session under a car, a recording `watch --out` wrote, a `.rod` inside a
//! VCDS installation. Finding it means leaving the tool, listing a directory
//! whose layout only this tool knows, and pasting the answer back. This module
//! is the alternative — a list, a number, and the thing itself.
//!
//! Nothing here knows what is being picked. It offers *entries of a directory*,
//! described well enough to tell apart and sorted by name, and hands back the
//! path. A level is a [`Level`]; two levels are two of them, and the level below
//! looks inside whatever the level above picked. Two picks from one list are two
//! calls.
//!
//! **The input is behind [`Chooser`]** for the reason `measure::setup` puts its
//! interview behind a trait: the part worth testing — the order, the detail
//! beside each name, what an empty directory says, what backing out does — is
//! the part a terminal makes untestable. [`Console`] is the person's; `Scripted`
//! is the tests'.
//!
//! **A pipe is not a terminal.** Every offline command in this tool works with
//! its output redirected, and a prompt that blocks on a stdin nobody is typing
//! into is a hang with no explanation. Without a terminal this refuses, and the
//! refusal names the argument that says the same thing on the command line.
//!
//! No new dependency: a numbered list on stdin is a `println!` and a
//! `read_line`, and `crossterm` — which is already here — would buy arrow keys
//! at the cost of raw mode, an alternate screen and a restore path on every
//! error, none of which a list of eighteen files needs.

// Nothing calls this yet — the commands that will are wired separately, and a
// module written a command ahead of its callers is the shape `datadir` already
// uses for the same reason.
#![allow(dead_code)]

use std::fmt::Write as _;
use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

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

/// Where the answer comes from.
///
/// One method, because a picker is one question asked repeatedly. `Ok(None)` is
/// a back-out — "not this list" — and is not a failure; `Err` is the end of the
/// conversation, a stdin that is not a terminal or one that closed.
pub trait Chooser {
    /// Show the choices and read one back, as an index into `choices`.
    ///
    /// `what` is the singular noun the list is of, so an implementation can put
    /// it in its own words.
    fn choose(&mut self, what: &str, choices: &[Choice]) -> Result<Option<usize>>;
}

/// The chooser a person uses: a numbered list on stdout, a number on stdin.
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
    fn choose(&mut self, what: &str, choices: &[Choice]) -> Result<Option<usize>> {
        if !std::io::stdin().is_terminal() {
            bail!(no_terminal(what, &self.instead));
        }
        let mut out = std::io::stdout();
        // The list once, the question as often as it takes: a mistyped number
        // should not push what it was chosen from off the screen.
        write!(out, "{}", list(what, choices))?;
        loop {
            out.flush()?;
            let mut line = String::new();
            // Zero bytes read is Ctrl-D, which here is "never mind" rather than
            // the error it is in `measure setup`: nothing is being agreed to by
            // default, so there is nothing a silent end of input could agree to.
            if std::io::stdin().read_line(&mut line)? == 0 {
                writeln!(out)?;
                return Ok(None);
            }
            let answer = line.trim();
            if answer.is_empty() {
                return Ok(None);
            }
            match answer.parse::<usize>() {
                Ok(n) if (1..=choices.len()).contains(&n) => return Ok(Some(n - 1)),
                _ => write!(
                    out,
                    "  {answer:?} is not one of 1-{}. which {what}? ",
                    choices.len()
                )?,
            }
        }
    }
}

/// The chooser the tests use: the picks go in, and everything it was shown
/// comes back out to be asserted against.
///
/// Lives beside [`Console`] rather than in this module's own tests so that any
/// command's tests can drive its picker without a terminal.
#[cfg(test)]
pub struct Scripted {
    picks: std::collections::VecDeque<Option<usize>>,
    /// Every list it was shown: the noun, then the names in the order they were
    /// offered.
    pub seen: Vec<(String, Vec<String>)>,
}

#[cfg(test)]
impl Scripted {
    /// `picks` are one-based, as a person types them; `None` is backing out.
    pub fn new(picks: impl IntoIterator<Item = Option<usize>>) -> Scripted {
        Scripted {
            picks: picks.into_iter().map(|p| p.map(|n| n - 1)).collect(),
            seen: Vec::new(),
        }
    }

    /// The names of the last list it was shown, in the order they were offered.
    pub fn last_names(&self) -> Vec<String> {
        self.seen.last().map(|(_, names)| names.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
impl Chooser for Scripted {
    fn choose(&mut self, what: &str, choices: &[Choice]) -> Result<Option<usize>> {
        self.seen
            .push((what.to_string(), choices.iter().map(|c| c.name.clone()).collect()));
        match self.picks.pop_front() {
            Some(pick) => Ok(pick),
            None => bail!("the script ran out while choosing a {what}"),
        }
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

/// Pick one entry of `dir`, or `None` if the person backed out.
///
/// Nothing to pick from is an `Err` rather than a `None`: a command that
/// silently does nothing is indistinguishable from one that failed, and the
/// message is the useful half of this module for anyone before their first
/// drive.
pub fn pick(io: &mut impl Chooser, dir: &Path, level: &Level<'_>) -> Result<Option<PathBuf>> {
    let choices = entries(dir, level);
    if choices.is_empty() {
        bail!(nothing_here(dir, level));
    }
    // A `Console` never answers outside the list. A chooser that does has
    // picked nothing, which is the same as backing out.
    Ok(io.choose(level.what, &choices)?.and_then(|at| choices.get(at).map(|c| c.path.clone())))
}

/// Pick down a tree, one level at a time: a car, then a session in it.
///
/// Backing out of a level below the first goes back up rather than giving up —
/// the wrong car is a thing to notice at the moment its sessions appear, and
/// starting the command again for it would be a poor answer. Backing out of the
/// first level is "never mind", and `None`.
pub fn pick_path(
    io: &mut impl Chooser,
    root: &Path,
    levels: &[Level<'_>],
) -> Result<Option<PathBuf>> {
    let mut chosen: Vec<PathBuf> = Vec::new();
    let mut at = 0;
    while let Some(level) = levels.get(at) {
        let parent = chosen.last().cloned().unwrap_or_else(|| root.to_path_buf());
        let dir = if level.within.is_empty() { parent } else { parent.join(level.within) };
        match pick(io, &dir, level)? {
            Some(path) => {
                chosen.push(path);
                at += 1;
            }
            None if at == 0 => return Ok(None),
            None => {
                chosen.pop();
                at -= 1;
            }
        }
    }
    Ok(chosen.pop())
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

/// The list as a person sees it.
fn list(what: &str, choices: &[Choice]) -> String {
    let width = choices.iter().map(|c| c.name.chars().count()).max().unwrap_or(0);
    let mut out = format!("\n{what}s:\n");
    for (at, choice) in choices.iter().enumerate() {
        let _ = writeln!(out, "  {:>3}  {:<width$}  {}", at + 1, choice.name, choice.detail);
    }
    let _ = write!(out, "\nwhich {what}? [1-{}, Enter to go back] ", choices.len());
    out
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
    /// this crate's file tests use. Nothing here may write inside a checkout.
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
        let cars = TempDir::new("nested");
        cars.file("XW8-first/measures/2026-08-04-1241.json", 10);
        cars.file("XW8-second/measures/2026-07-19-0902.json", 10);
        cars.file("XW8-second/measures/2026-08-01-1800.json", 10);
        let levels = [
            Level::directories("car"),
            Level::files("session").within("measures").ending(".json"),
        ];
        let mut io = Scripted::new([Some(2), Some(1)]);
        let picked = pick_path(&mut io, &cars.0, &levels).unwrap();
        assert_eq!(picked, Some(cars.0.join("XW8-second/measures/2026-07-19-0902.json")));
        assert_eq!(io.seen[0].0, "car");
        assert_eq!(io.last_names(), ["2026-07-19-0902.json", "2026-08-01-1800.json"]);
    }

    #[test]
    fn backing_out_of_the_second_level_offers_the_first_again() {
        // The wrong car is a thing you notice when its sessions appear, and
        // starting the command over would be a poor way to say so.
        let cars = TempDir::new("back");
        cars.file("XW8-first/measures/2026-08-04-1241.json", 10);
        cars.file("XW8-second/measures/2026-07-19-0902.json", 10);
        let levels =
            [Level::directories("car"), Level::files("session").within("measures")];
        let mut io = Scripted::new([Some(1), None, Some(2), Some(1)]);
        let picked = pick_path(&mut io, &cars.0, &levels).unwrap();
        assert_eq!(picked, Some(cars.0.join("XW8-second/measures/2026-07-19-0902.json")));
        let asked: Vec<&str> = io.seen.iter().map(|(what, _)| what.as_str()).collect();
        assert_eq!(asked, ["car", "session", "car", "session"]);
    }

    #[test]
    fn backing_out_of_the_first_level_picks_nothing() {
        let cars = TempDir::new("never-mind");
        cars.file("XW8-first/measures/2026-08-04-1241.json", 10);
        let levels = [Level::directories("car"), Level::files("session").within("measures")];
        assert_eq!(pick_path(&mut Scripted::new([None]), &cars.0, &levels).unwrap(), None);
    }

    #[test]
    fn one_list_can_be_picked_from_twice() {
        // `survey --diff` compares two dumps, and both come off the same list.
        let dir = TempDir::new("twice");
        dir.file("driving.jsonl", 10);
        dir.file("parked.jsonl", 10);
        let level = Level::files("survey").ending(".jsonl");
        let mut io = Scripted::new([Some(2), Some(1)]);
        let before = pick(&mut io, &dir.0, &level).unwrap();
        let after = pick(&mut io, &dir.0, &level).unwrap();
        assert_eq!(before, Some(dir.0.join("parked.jsonl")));
        assert_eq!(after, Some(dir.0.join("driving.jsonl")));
    }

    #[test]
    fn with_no_terminal_the_refusal_names_the_argument_to_pass_instead() {
        // A prompt on a redirected stdin is a hang nobody can diagnose.
        let why = no_terminal("session", "vagcan measure view PATH");
        assert!(why.contains("no terminal"), "{why}");
        assert!(why.contains("vagcan measure view PATH"), "{why}");
    }

    #[test]
    fn the_list_a_person_sees_numbers_every_choice_and_says_how_to_leave() {
        let dir = TempDir::new("render");
        dir.file("2026-08-04-1241.json", 4_200);
        let shown = list("session", &entries(&dir.0, &Level::files("session")));
        assert!(shown.contains("  1  2026-08-04-1241.json  4 kB, "), "{shown}");
        assert!(shown.contains("which session? [1-1, Enter to go back]"), "{shown}");
    }
}
