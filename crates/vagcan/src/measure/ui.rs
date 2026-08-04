//! The live screen: drawing, keys and the tone. No polling happens here.
//!
//! The whole of this module is a function of a [`Screen`] the poll loop fills
//! in each cycle, which is what keeps the loop's scheduling testable and the
//! drawing testable separately — `watch` never split at that seam and its
//! `mod.rs` is the largest file in the crate as a result.
//!
//! Three decisions from the design shape it.
//!
//! **The screen always says which state it is in.** The stopwatch arms, starts
//! and finishes by itself, so without a band across the top the first thing a
//! new user meets is a still screen and no explanation. `WAITING` carries the
//! current speed for one specific reason: arming needs a true zero, and a car
//! creeping at 0.4 km/h would otherwise sit there looking broken.
//!
//! **The table carries every value; the chart carries a few.** Ten rows are
//! readable and ten lines are not, so the chart takes them three at a time on
//! at most two scales, overlaid rather than paged one by one — a driver asked
//! for speed, engine speed, power and acceleration on the same picture, and one
//! series at a time cannot answer "where did it lose the time". What will not
//! fit on a page is one `←`/`→` away, and every line says in the key what it is,
//! what it is measured in, and whether it was read off the bus or worked out
//! here.
//!
//! **A tone marks each closed mark**, because the screen is unreadable at the
//! moment the information arrives. The player is spawned and never waited on: a
//! poll loop that blocks on audio puts the sound ahead of the measurement.

use crossterm::event::KeyCode;
use ratatui::prelude::*;
use ratatui::symbols::Marker;
use ratatui::widgets::{Axis, Block, Borders, Cell, Chart, Dataset, GraphType, Paragraph, Row, Table};

use super::messages;
use super::session::{self, ARMING_HOLD_S};
use super::types::{Seconds, Track};

/// Where the stopwatch stands, with what the band has to say about it.
///
/// Not [`session::State`] itself: three of the bands carry a number the state
/// machine has no reason to keep — the speed that is preventing arming, the run
/// that just finished, the speed a run was abandoned at.
#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Moving, or never yet stopped. The speed is what is keeping it here.
    Waiting { speed_kmh: f64 },
    /// Standing still and counting towards [`ARMING_HOLD_S`].
    Arming { remaining_s: Seconds },
    Armed,
    Running { elapsed_s: Seconds },
    /// A run reached its highest mark. `seconds` is that mark's time.
    Done { seconds: Option<Seconds> },
    /// A run ended early. The marks that closed are kept and named.
    Aborted { at_kmh: f64, kept: Vec<String> },
    Paused,
}

/// The state band, in the copy the design fixes for every state.
///
/// `slow` is the achieved rate once the cadence has collapsed, which appends
/// rather than replaces: the state is still the state, and what changed is how
/// much the times are worth.
pub fn band(phase: &Phase, slow: Option<f64>) -> String {
    let mut out = match phase {
        Phase::Waiting { speed_kmh } => {
            format!("WAITING — come to a full stop to arm      {speed_kmh:.1} km/h")
        }
        Phase::Arming { remaining_s } => format!("ARMING — hold still  {remaining_s:.1} s"),
        Phase::Armed => "ARMED — go when you are ready".to_string(),
        Phase::Running { elapsed_s } => format!("RUN  {elapsed_s:.2} s"),
        Phase::Done { seconds: Some(seconds) } => {
            format!("DONE  {seconds:.2} s — stop completely to arm the next run")
        }
        Phase::Done { seconds: None } => {
            "DONE — stop completely to arm the next run".to_string()
        }
        Phase::Aborted { at_kmh, kept } if kept.is_empty() => {
            format!("ABORTED at {at_kmh:.0} km/h")
        }
        Phase::Aborted { at_kmh, kept } => {
            format!("ABORTED at {at_kmh:.0} km/h — kept {}", kept.join(", "))
        }
        Phase::Paused => "PAUSED — will not arm.  [p] resume".to_string(),
    };
    if let Some(hz) = slow {
        out.push_str(&format!("  SLOW — {hz:.0} Hz, times less certain"));
    }
    out
}

/// Turn the state machine's own state into a band, given what only the loop
/// knows.
pub fn phase_of(
    state: session::State,
    speed_kmh: f64,
    now: Seconds,
    last_run: Option<&Outcome>,
) -> Phase {
    match state {
        session::State::Idle => Phase::Waiting { speed_kmh },
        session::State::Arming { since } => {
            Phase::Arming { remaining_s: (ARMING_HOLD_S - (now - since)).max(0.0) }
        }
        session::State::Armed => Phase::Armed,
        session::State::Running => Phase::Running { elapsed_s: now },
        session::State::Paused => Phase::Paused,
        session::State::Finished => match last_run {
            Some(Outcome::Aborted { at_kmh, kept }) => {
                Phase::Aborted { at_kmh: *at_kmh, kept: kept.clone() }
            }
            Some(Outcome::Done { seconds }) => Phase::Done { seconds: *seconds },
            None => Phase::Done { seconds: None },
        },
    }
}

/// How the last run ended, which is what the band says while the car rolls to a
/// stop.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Done { seconds: Option<Seconds> },
    Aborted { at_kmh: f64, kept: Vec<String> },
}

/// Where a number on the value table came from.
///
/// A column of its own, because a figure that was never on the bus must not
/// look like one that was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The car reported it.
    Bus,
    /// This tool worked it out. The qualifier is the one that matters for
    /// reading it: live acceleration can only be causal, and power is an
    /// estimate.
    Computed(&'static str),
}

impl Origin {
    fn columns(self) -> (&'static str, &'static str) {
        match self {
            Origin::Bus => ("bus", ""),
            Origin::Computed(note) => ("computed", note),
        }
    }
}

/// One line of the value table.
#[derive(Clone, Debug, PartialEq)]
pub struct ValueRow {
    pub name: String,
    pub value: String,
    pub origin: Origin,
}

/// One line of the marks panel.
///
/// A launch-based mark leads with `≈` and its interval waits for the results
/// table: there is room for one number on this screen and no time to read a
/// second.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkRow {
    pub name: String,
    /// `None` until it closes, drawn as a placeholder rather than left blank —
    /// a gap reads as a mark that was not asked for.
    ///
    /// For a launch mark this is the **midpoint of the bracket**, the same
    /// number `report::mark_time` leads with, so the panel and the table can
    /// never disagree about what the run did.
    pub seconds: Option<Seconds>,
    pub from_launch: bool,
}

impl MarkRow {
    /// How a mark is spelled with one column to spell it in.
    ///
    /// The retracted spelling was `1.2+ s`. It is wrong twice over: on the car
    /// it reads as a line that was cut off, and the `+` is the one-signed launch
    /// bias this project withdrew — the launch is *bracketed* between two
    /// estimators that miss it from opposite sides, so a launch time is not
    /// "1.2 or more" and never was.
    ///
    /// `≈` is what is left once the interval will not fit: it says the number is
    /// an estimate without saying which way it leans, and it cannot be read as
    /// "at least". The interval it came from — `6.94 s (6.85 … 7.04)` — is on the
    /// results table a moment later, which is where a driver has time to read
    /// two numbers. Both are printed to hundredths, so the panel's figure is the
    /// table's figure and not a rounder cousin of it.
    fn value(&self) -> String {
        match (self.seconds, self.from_launch) {
            (Some(seconds), true) => format!("≈{seconds:.2} s"),
            (Some(seconds), false) => format!("{seconds:.2} s"),
            (None, _) => "·".to_string(),
        }
    }
}

/// One series the chart can show.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// What the key calls it.
    pub label: String,
    /// What it is measured in — the catalog's own word for a channel it
    /// resolved, or the unit the computation produces for one this tool worked
    /// out. It is what groups lines onto a scale, so a series without one has a
    /// scale of its own by definition.
    pub unit: String,
    pub points: Track,
    /// Where the line came from, the same distinction the value table draws.
    ///
    /// The chart is the one place it would be easiest to drop, and a line whose
    /// origin is not stated is indistinguishable from a measurement. A derived
    /// series' running end is causal by construction, which is what
    /// `Computed("trailing")` says.
    pub origin: Origin,
}

impl Series {
    fn computed(&self) -> bool {
        matches!(self.origin, Origin::Computed(_))
    }

    /// The lowest and highest value in the series, or `None` when there is
    /// nothing to bound — an empty series, or one that has not moved.
    fn span(&self) -> Option<(f64, f64)> {
        let low = self.points.v.iter().copied().fold(f64::INFINITY, f64::min);
        let high = self.points.v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (low.is_finite() && high.is_finite()).then_some((low, high))
    }
}

/// How many lines one chart carries, and how many scales it puts them on.
///
/// The browser page caps a hand-picked set at three series and two Y axes, and
/// says why: rpm at 6480 beside boost at 2.1 bar destroys both scales, and
/// dropping one quietly is worse than saying so. A terminal has a fraction of
/// the room and **no second Y axis to draw on at all** — `ratatui::Chart` has
/// one — so the cap here is the same three lines and a stricter two units: one
/// drawn on the axis in its own numbers, the other folded onto that axis and
/// printed in the key with the range it was folded from. A curve whose axis is
/// not shown would be decoration; the range is that axis, in words.
///
/// What does not fit is one `←`/`→` away, and the key says which page this is.
const MAX_LINES: usize = 3;
const MAX_UNITS: usize = 2;

/// One colour per line, in a fixed order so a series does not change colour from
/// one cycle to the next.
///
/// Colour is how a driver tells three lines apart at a glance. It is not the
/// only way they are told apart — a computed line is drawn with a different
/// marker and named with a different glyph in the key — because a terminal that
/// will not colour, or an eye that will not separate these three, must still be
/// able to read the chart.
const LINE_COLOURS: [Color; MAX_LINES] = [Color::Cyan, Color::Magenta, Color::Yellow];

/// Which series share one chart, in the order [`Series`] came in.
///
/// Greedy and stable: a page takes series until it would exceed [`MAX_LINES`]
/// or [`MAX_UNITS`], then a new page starts. Stable matters more than clever —
/// `←`/`→` has to mean the same thing on the next cycle as it did on this one,
/// and a page that reshuffles itself as a channel starts answering is a page
/// nobody can navigate at 100 km/h.
pub fn pages(series: &[Series]) -> Vec<Vec<usize>> {
    let mut pages: Vec<Vec<usize>> = Vec::new();
    for (i, next) in series.iter().enumerate() {
        let fits = pages.last().is_some_and(|page: &Vec<usize>| {
            if page.len() >= MAX_LINES {
                return false;
            }
            let mut units: Vec<&str> = page.iter().map(|j| series[*j].unit.as_str()).collect();
            units.sort_unstable();
            units.dedup();
            units.contains(&next.unit.as_str()) || units.len() < MAX_UNITS
        });
        match fits {
            true => pages.last_mut().expect("a page to add to").push(i),
            false => pages.push(vec![i]),
        }
    }
    pages
}

/// Everything the screen draws, assembled by the poll loop each cycle.
#[derive(Clone, Debug, Default)]
pub struct Screen {
    pub band: String,
    /// The one-line note about the car file, shown above everything: a user who
    /// spent twenty minutes on a coastdown and then sees no difference concludes
    /// it did nothing.
    pub banner: Option<String>,
    pub rows: Vec<ValueRow>,
    pub marks: Vec<MarkRow>,
    pub series: Vec<Series>,
    /// Which page of overlaid series is up, as [`pages`] divides them.
    pub chart: usize,
    /// The achieved rate, measured. There is no `--hz`, and a rate printed
    /// before the loop ran would be a setting pretending to be a measurement.
    pub hz: Option<f64>,
    /// The file being written, when one is open.
    pub file: Option<String>,
    /// The quit guard, once `q` has been pressed with work outstanding.
    pub warning: Option<String>,
    /// The results table for the run that just ended.
    ///
    /// It takes the whole middle of the screen, and the loop only sets it once
    /// the car is stationary: redrawing a dense table at 100 km/h is exactly
    /// what the rest of this design avoids.
    pub table: Option<String>,
}

/// The keyboard's own state: which chart is up, and whether `q` is armed.
#[derive(Clone, Debug, Default)]
pub struct Controls {
    pub chart: usize,
    /// How many chart pages there are to walk — [`pages`]`(…).len()`, not the
    /// number of series. Kept here rather than passed in, so that a caller
    /// cannot hold the keyboard state and the count at once and have them
    /// disagree.
    pub charts: usize,
    /// Set by the first `q` with unsaved runs, cleared by anything else. Two
    /// keystrokes to throw away a drive, one to keep it.
    quit_armed: bool,
    /// Set by `s`. The write itself is deferred out of the key handler, so that
    /// a file is never created between two batches of one cycle.
    save_requested: bool,
    /// Set by `d`. Deferred for the same reason as a save, and for one more:
    /// what a discard has to reach — the recorded runs and the file — is the
    /// loop's, not the keyboard's.
    discard_requested: bool,
    /// Set by Enter.
    keep_requested: bool,
}

impl Controls {
    /// Note that a save was asked for.
    pub fn ask_save(&mut self) {
        self.save_requested = true;
    }

    /// Whether a save is owed, clearing the request.
    pub fn take_save(&mut self) -> bool {
        std::mem::take(&mut self.save_requested)
    }

    pub fn ask_discard(&mut self) {
        self.discard_requested = true;
    }

    pub fn take_discard(&mut self) -> bool {
        std::mem::take(&mut self.discard_requested)
    }

    pub fn ask_keep(&mut self) {
        self.keep_requested = true;
    }

    pub fn take_keep(&mut self) -> bool {
        std::mem::take(&mut self.keep_requested)
    }
}

/// What a keystroke asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Nothing,
    /// Hand this to the state machine.
    Session(session::Command),
    /// Write the session out, then tell the state machine it is saved.
    Save,
    /// Throw the finished run away. It leaves the session, so it leaves both
    /// what `Save` would write and the count of what is still unsaved.
    Discard,
    /// Keep the finished run and go again: save, then put the screen back the
    /// way the first run found it.
    KeepGoing,
    Quit,
    /// `q` with unsaved runs: the message to put on screen, and no quit.
    Refuse(String),
}

/// What to say when `c` or `Esc` arrives with no run under way.
///
/// A key that does nothing at all reads as a key that was not received, which
/// is precisely how the cancel bug was described from the driver's seat.
pub fn nothing_to_cancel() -> String {
    "nothing to cancel — no run is under way. The stopwatch starts itself when \
     the car moves off from a standstill."
        .to_string()
}

/// What a discard did, and — the part that matters — what it did to the file.
///
/// A run already written to `--out` is on disk by the time anybody decides they
/// did not want it, so "discarded" would be a half-truth on its own: the file
/// is rewritten without it and the message says which file and how many runs
/// are left in it. A run that was never written says that instead, because
/// "rewritten" would be just as misleading the other way.
pub fn discarded(index: usize, rewritten: Option<&str>, left: usize) -> String {
    match rewritten {
        Some(path) => format!(
            "run {index} discarded. It had already been written, so {path} has been \
             rewritten without it — {left} run(s) left in the file."
        ),
        None => format!(
            "run {index} discarded. It was never written anywhere, so there is nothing \
             to undo."
        ),
    }
}

/// `d` with no finished run on screen.
pub fn nothing_to_discard() -> String {
    "nothing to discard — [d] throws away the run whose results are on screen, and \
     none are."
        .to_string()
}

/// Enter with no finished run on screen.
pub fn nothing_to_keep() -> String {
    "nothing to keep yet — [↵] saves the run whose results are on screen and puts the \
     screen back for the next one."
        .to_string()
}

/// Enter, done: the run is on disk and the screen is the one a run starts on.
pub fn kept(path: &str, runs: usize) -> String {
    format!("{runs} run(s) saved to {path}. Ready for the next one.")
}

/// Handle one key.
///
/// `Esc` cancels a run here and quits in `watch`. That is a deliberate
/// divergence rather than an oversight — a stopwatch needs a cheap "throw this
/// one away" and `watch` has nothing to throw away — and it is written down so
/// that it stays deliberate.
///
/// **`c` cancels as well, and it is the one in the hints.** A control a driver
/// needs mid-run must not depend on the terminal agreeing about escape
/// sequences: `Esc` is the prefix of every one of them, so what a parser makes
/// of a lone `0x1B` is a property of the terminal and the crate rather than of
/// this program. It happens to work here — measured, not assumed, see `drain` —
/// and a plain letter costs nothing and cannot stop working.
pub fn on_key(controls: &mut Controls, code: KeyCode, unsaved: usize) -> Action {
    // Any key that is not a second `q` disarms the guard: a driver who pressed
    // `q`, thought better of it and pressed `s` must not then lose the drive to
    // a later stray `q`.
    let armed = std::mem::take(&mut controls.quit_armed);
    let charts = controls.charts;
    match code {
        KeyCode::Char('q') => match unsaved == 0 || armed {
            true => Action::Quit,
            false => {
                controls.quit_armed = true;
                Action::Refuse(messages::unsaved_on_quit(unsaved))
            }
        },
        KeyCode::Char('p') => Action::Session(session::Command::PauseTrigger),
        KeyCode::Esc | KeyCode::Char('c') => Action::Session(session::Command::Cancel),
        KeyCode::Char('s') => Action::Save,
        KeyCode::Char('d') => Action::Discard,
        KeyCode::Enter => Action::KeepGoing,
        KeyCode::Left if charts > 0 => {
            controls.chart = (controls.chart + charts - 1) % charts;
            Action::Nothing
        }
        KeyCode::Right if charts > 0 => {
            controls.chart = (controls.chart + 1) % charts;
            Action::Nothing
        }
        _ => Action::Nothing,
    }
}

/// The key hints, which are the keys the design gives this command and no
/// others.
const HINTS: &str =
    " [p]ause [c]ancel [d]iscard [↵]keep&next [s]ave [←→]chart [q]uit";

/// Draw the whole screen.
pub fn draw(frame: &mut Frame, screen: &Screen) {
    let banner_height = u16::from(screen.banner.is_some()) * 2;
    let outer = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(banner_height),
        Constraint::Min(6),
        Constraint::Length(3),
    ])
    .split(frame.area());

    frame.render_widget(
        Paragraph::new(screen.band.clone())
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        outer[0],
    );
    if let Some(banner) = &screen.banner {
        frame.render_widget(
            Paragraph::new(banner.clone()).style(Style::default().fg(Color::DarkGray)),
            outer[1],
        );
    }

    // Once the car has stopped, the results table is what the screen is for.
    if let Some(table) = &screen.table {
        frame.render_widget(
            Paragraph::new(table.clone())
                .block(Block::default().borders(Borders::ALL).title(" results ")),
            outer[2],
        );
        frame.render_widget(
            Paragraph::new(status_line(screen, outer[3].width.saturating_sub(2) as usize))
                .block(Block::default().borders(Borders::ALL)),
            outer[3],
        );
        return;
    }

    // The marks panel takes what its content needs and the values take the
    // rest: a mark is `0-100` and a value can be `2.06 / 2.15 bar abs`.
    let marks_width = screen
        .marks
        .iter()
        .map(|m| m.name.chars().count() + m.value().chars().count() + 5)
        .max()
        .unwrap_or(12)
        .clamp(12, 24) as u16;
    let middle =
        Layout::horizontal([Constraint::Min(30), Constraint::Length(marks_width)]).split(outer[2]);
    let left = Layout::vertical([
        Constraint::Length(screen.rows.len() as u16 + 2),
        Constraint::Min(3),
    ])
    .split(middle[0]);

    draw_values(frame, screen, left[0]);
    draw_chart(frame, screen, left[1]);
    draw_marks(frame, screen, middle[1]);

    frame.render_widget(
        Paragraph::new(status_line(screen, outer[3].width.saturating_sub(2) as usize))
            .block(Block::default().borders(Borders::ALL)),
        outer[3],
    );
}

/// The footer: the achieved rate, whether a file is open, and the keys.
///
/// The keys are the part that has to survive a narrow terminal — they are the
/// only thing on screen a driver cannot work out for themselves — so the two
/// pieces of running commentary are dropped in turn until what is left fits,
/// rather than letting the line truncate wherever it happens to reach.
fn status_line(screen: &Screen, width: usize) -> String {
    if let Some(warning) = &screen.warning {
        return warning.clone();
    }
    let rate = screen.hz.map(|hz| format!("{hz:.1} Hz · ")).unwrap_or_default();
    let file =
        screen.file.as_deref().map(|path| format!("  ·  writing {path}")).unwrap_or_default();
    let keys = HINTS.trim_start();
    for line in [format!(" {rate}{keys}{file}"), format!(" {rate}{keys}"), format!(" {keys}")] {
        if line.chars().count() <= width {
            return line;
        }
    }
    format!(" {keys}")
}

fn draw_values(frame: &mut Frame, screen: &Screen, area: Rect) {
    let rows: Vec<Row> = screen
        .rows
        .iter()
        .map(|row| {
            let (origin, note) = row.origin.columns();
            Row::new(vec![
                Cell::from(row.name.clone()),
                Cell::from(row.value.clone())
                    .style(Style::default().add_modifier(Modifier::BOLD)),
                Cell::from(origin).style(Style::default().fg(Color::DarkGray)),
                Cell::from(note).style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();
    let name_w = screen.rows.iter().map(|r| r.name.chars().count()).max().unwrap_or(6).max(6);
    let value_w = screen.rows.iter().map(|r| r.value.chars().count()).max().unwrap_or(8).max(8);
    let table = Table::new(
        rows,
        [
            Constraint::Length(name_w as u16),
            Constraint::Length(value_w as u16),
            Constraint::Length(8),
            Constraint::Min(8),
        ],
    )
    .block(Block::default().borders(Borders::ALL).title(" vagcan measure "));
    frame.render_widget(table, area);
}

/// A number as a person reads it off an axis: as many decimals as its size
/// leaves room to mean anything, and no more.
fn tick(value: f64) -> String {
    match value.abs() {
        v if v >= 100.0 => format!("{value:.0}"),
        v if v >= 10.0 => format!("{value:.1}"),
        _ => format!("{value:.2}"),
    }
}

/// One page's lines, overlaid, with the key that makes them readable.
///
/// Drawn from the accumulated run buffer rather than from the last point alone,
/// so the shape of the run is visible while it is happening.
///
/// **The lines have wildly different scales** — 100 km/h against 6000 rpm — so
/// one unit owns the drawn axis and everything else is folded onto it. Folding
/// is what keeps a 6000-rpm line from flattening the speed trace into a stripe
/// along the bottom, and the price of it is that the folded line's numbers are
/// not on the axis. They are in the key instead, as the range the fold came
/// from, because a curve whose axis is nowhere is decoration rather than data.
fn draw_chart(frame: &mut Frame, screen: &Screen, area: Rect) {
    let pages = pages(&screen.series);
    let Some(page) = pages.get(screen.chart.min(pages.len().saturating_sub(1))) else {
        frame.render_widget(
            Block::default().borders(Borders::ALL).title(" chart — nothing read yet "),
            area,
        );
        return;
    };

    // The key has to fit or it is not a key. Lines come off the tail of the page
    // until it does, and the count that went is printed rather than left for the
    // driver to notice: a screen that drops a series must say it dropped it.
    let room = area.width.saturating_sub(2) as usize;
    let mut drawn: Vec<usize> = page.clone();
    let mut dropped = 0usize;
    while drawn.len() > 1 && key_line(screen, &drawn, dropped, pages.len(), screen.chart).width() > room
    {
        drawn.pop();
        dropped += 1;
    }

    // The unit that came first owns the axis; the second is folded onto it.
    let mut units: Vec<&str> = Vec::new();
    for i in &drawn {
        let unit = screen.series[*i].unit.as_str();
        if !units.contains(&unit) {
            units.push(unit);
        }
    }
    let group_span = |unit: &str| {
        drawn
            .iter()
            .filter(|i| screen.series[**i].unit == unit)
            .filter_map(|i| screen.series[*i].span())
            .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
    };
    // A flat or empty group still needs bounds, or the chart draws nothing and
    // looks like a failure to read the car.
    let widen = |span: Option<(f64, f64)>| match span {
        Some((lo, hi)) if hi > lo => (lo, hi),
        Some((lo, _)) => (lo - 0.5, lo + 0.5),
        None => (0.0, 1.0),
    };
    let axis = widen(group_span(units[0]));

    // Time is the same for every line, and it is drawn from the oldest sample
    // kept rather than from the session clock: the buffer is emptied at each
    // launch, so during a run this reads as seconds since the car set off.
    let t0 = drawn
        .iter()
        .filter_map(|i| screen.series[*i].points.t.first().copied())
        .fold(f64::INFINITY, f64::min);
    let t1 = drawn
        .iter()
        .filter_map(|i| screen.series[*i].points.t.last().copied())
        .fold(f64::NEG_INFINITY, f64::max);
    let (t0, t1) = match t0.is_finite() && t1 > t0 {
        true => (t0, t1),
        false => (0.0, 1.0),
    };

    let mut plotted: Vec<Vec<(f64, f64)>> = Vec::new();
    for i in &drawn {
        let series = &screen.series[*i];
        let (lo, hi) = widen(group_span(&series.unit));
        let fold = series.unit != units[0];
        plotted.push(
            (0..series.points.len())
                .map(|n| {
                    let y = series.points.v[n];
                    let y = match fold {
                        true => axis.0 + (y - lo) / (hi - lo) * (axis.1 - axis.0),
                        false => y,
                    };
                    (series.points.t[n] - t0, y)
                })
                .collect(),
        );
    }

    let data: Vec<Dataset> = drawn
        .iter()
        .zip(&plotted)
        .enumerate()
        .map(|(n, (i, points))| {
            let series = &screen.series[*i];
            Dataset::default()
                // A computed line is dotted where a read one is solid, so the
                // distinction survives a terminal that will not colour.
                .marker(match series.computed() {
                    true => Marker::Dot,
                    false => Marker::Braille,
                })
                .graph_type(GraphType::Line)
                .style(Style::default().fg(LINE_COLOURS[n % LINE_COLOURS.len()]))
                .data(points)
        })
        .collect();

    let chart = Chart::new(data)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(key_line(screen, &drawn, dropped, pages.len(), screen.chart))
                .title_bottom(notes_line(screen, &drawn, units.len(), pages.len())),
        )
        .x_axis(
            Axis::default()
                .title("time")
                .bounds([0.0, t1 - t0])
                .labels(["0s".to_string(), format!("{:.1}s", t1 - t0)]),
        )
        .y_axis(
            Axis::default()
                .title(units[0].to_string())
                .bounds([axis.0, axis.1])
                .labels([tick(axis.0), tick(axis.1)]),
        );
    frame.render_widget(chart, area);
}

/// The key: what each line is, in its own colour, with the range of any line
/// that had to be folded onto somebody else's axis.
fn key_line<'a>(
    screen: &Screen,
    drawn: &[usize],
    dropped: usize,
    pages: usize,
    page: usize,
) -> Line<'a> {
    let mut spans = vec![Span::raw(" ")];
    let first_unit = drawn.first().map(|i| screen.series[*i].unit.clone()).unwrap_or_default();
    for (n, i) in drawn.iter().enumerate() {
        let series = &screen.series[*i];
        if n > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
        }
        let mut text = String::new();
        if series.computed() {
            text.push('⋯');
        }
        text.push_str(&series.label);
        // Only a folded line carries a range, which is what makes the range the
        // marker: the axis says everything about the line that owns it.
        if series.unit != first_unit
            && let Some((lo, hi)) = series.span()
        {
            text.push_str(&format!(" [{}…{} {}]", tick(lo), tick(hi), series.unit));
        }
        if let Origin::Computed(note) = series.origin {
            text.push(' ');
            text.push_str(note);
        }
        spans.push(Span::styled(
            text,
            Style::default().fg(LINE_COLOURS[n % LINE_COLOURS.len()]),
        ));
    }
    if dropped > 0 {
        spans.push(Span::styled(
            format!("  +{dropped} no room"),
            Style::default().fg(Color::DarkGray),
        ));
    }
    if pages > 1 {
        spans.push(Span::styled(
            format!("  {}/{pages}", page.min(pages - 1) + 1),
            Style::default().fg(Color::DarkGray),
        ));
    }
    spans.push(Span::raw(" "));
    Line::from(spans)
}

/// The bottom border: what the glyphs on this particular chart mean, and
/// nothing about the ones it is not carrying.
fn notes_line<'a>(screen: &Screen, drawn: &[usize], units: usize, pages: usize) -> Line<'a> {
    let mut notes: Vec<String> = Vec::new();
    if drawn.iter().any(|i| screen.series[*i].computed()) {
        notes.push("⋯ computed".to_string());
    }
    if units > 1 {
        notes.push("[ ] own scale".to_string());
    }
    if pages > 1 {
        notes.push("←→ chart".to_string());
    }
    match notes.is_empty() {
        true => Line::default(),
        false => Line::styled(
            format!(" {} ", notes.join(" · ")),
            Style::default().fg(Color::DarkGray),
        ),
    }
}

fn draw_marks(frame: &mut Frame, screen: &Screen, area: Rect) {
    let rows: Vec<Row> = screen
        .marks
        .iter()
        .map(|mark| {
            let style = match mark.seconds.is_some() {
                true => Style::default().add_modifier(Modifier::BOLD),
                false => Style::default().fg(Color::DarkGray),
            };
            Row::new(vec![
                Cell::from(mark.name.clone()),
                Cell::from(mark.value()).style(style),
            ])
        })
        .collect();
    let name_w = screen.marks.iter().map(|m| m.name.chars().count()).max().unwrap_or(6);
    let table = Table::new(rows, [Constraint::Length(name_w as u16), Constraint::Min(6)])
        .block(Block::default().borders(Borders::ALL).title(" marks "));
    frame.render_widget(table, area);
}

/// The same screen for a pipe, a log or an agent: the band, the values and
/// whatever marks have closed, one line at a time.
///
/// The loop is identical — only the drawing changes — exactly as `watch`'s
/// plain-console mode does it.
pub fn plain_line(screen: &Screen) -> String {
    let values: Vec<String> =
        screen.rows.iter().map(|row| format!("{} {}", row.name, row.value)).collect();
    let marks: Vec<String> = screen
        .marks
        .iter()
        .filter(|mark| mark.seconds.is_some())
        .map(|mark| format!("{} {}", mark.name, mark.value()))
        .collect();
    let mut out = screen.band.clone();
    if !values.is_empty() {
        out.push_str(&format!("  |  {}", values.join("  ")));
    }
    if !marks.is_empty() {
        out.push_str(&format!("  |  {}", marks.join("  ")));
    }
    out
}

/// What just happened, for the ear rather than the eye.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// A mark closed.
    Mark,
    /// The run reached its highest mark.
    Finished,
    /// A run was abandoned, or a coastdown pass was thrown away. This is the
    /// one that matters most: without it a rejected pass is discovered a
    /// kilometre later.
    Rejected,
}

/// Where macOS keeps sounds that are short enough to be information rather than
/// an interruption.
const SOUNDS: &str = "/System/Library/Sounds";

/// The player and its argument, or `None` when nothing should be played.
///
/// Split out from [`play`] so that the choice is testable and the spawn is not:
/// a test can assert that `--quiet` silences every tone and that the three are
/// distinguishable, without a sound card.
fn player(tone: Tone, quiet: bool) -> Option<(&'static str, String)> {
    if quiet || !cfg!(target_os = "macos") {
        return None;
    }
    let file = match tone {
        Tone::Mark => "Tink.aiff",
        Tone::Finished => "Glass.aiff",
        Tone::Rejected => "Basso.aiff",
    };
    Some(("afplay", format!("{SOUNDS}/{file}")))
}

/// Play a tone, and do not wait for it.
///
/// **Spawned, never awaited.** A poll loop that blocks on audio puts the sound
/// ahead of the measurement, and the measurement is the point. Every failure is
/// ignored, including the absence of anything to play with; where there is no
/// player the terminal bell says the same thing in one byte.
pub fn play(tone: Tone, quiet: bool) {
    if quiet {
        return;
    }
    let Some((program, argument)) = player(tone, quiet) else {
        bell();
        return;
    };
    let spawned = std::process::Command::new(program)
        .arg(&argument)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if spawned.is_err() {
        bell();
    }
}

fn bell() {
    use std::io::Write as _;
    let mut err = std::io::stderr();
    let _ = err.write_all(b"\x07");
    let _ = err.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Draw a screen and hand back what a person would see, one line per row.
    ///
    /// Asserting on the rendered text rather than on the widgets is the only
    /// way to catch the things that actually went wrong on the car: a column
    /// too narrow for what it prints, a title truncated to nonsense, a series
    /// silently missing from a legend.
    fn screen_lines(screen: &Screen, w: u16, h: u16) -> Vec<String> {
        let backend = ratatui::backend::TestBackend::new(w, h);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame, screen)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                let line: String =
                    (0..w).map(|x| buffer[(x, y)].symbol().to_string()).collect::<Vec<_>>().concat();
                line.trim_end().to_string()
            })
            .collect()
    }

    fn screen_text(screen: &Screen, w: u16, h: u16) -> String {
        screen_lines(screen, w, h).join("\n")
    }

    /// A run in progress on a car with a profile: the busiest the live screen
    /// gets, which is the case every size has to survive.
    fn demo_screen() -> Screen {
        // Two shapes, not one scaled twice: a speed that rises through the run
        // and an engine speed that saws up and back at each shift. Two curves of
        // the same shape would overlap exactly once folded, and a chart test
        // that cannot tell one line from two proves nothing.
        let ramp = |scale: f64, offset: f64| {
            let mut track = Track::default();
            for i in 0..60 {
                let t = f64::from(i) * 0.05;
                track.push(t, offset + scale * t * t);
            }
            track
        };
        let saw = |low: f64, high: f64| {
            let mut track = Track::default();
            for i in 0..60 {
                let t = f64::from(i) * 0.05;
                track.push(t, low + (high - low) * ((t * 1.2) % 1.0));
            }
            track
        };
        Screen {
            band: band(&Phase::Running { elapsed_s: 4.31 }, None),
            banner: None,
            rows: vec![
                ValueRow { name: "speed".into(), value: "62.4 km/h".into(), origin: Origin::Bus },
                ValueRow { name: "engine".into(), value: "4310 /min".into(), origin: Origin::Bus },
                ValueRow { name: "gear".into(), value: "3".into(), origin: Origin::Bus },
                ValueRow {
                    name: "boost".into(),
                    value: "2.06 / 2.15 bar (act/spec)".into(),
                    origin: Origin::Bus,
                },
                ValueRow {
                    name: "accel".into(),
                    value: "0.41 g".into(),
                    origin: Origin::Computed("trailing"),
                },
                ValueRow {
                    name: "power".into(),
                    value: "108 kW (147 PS)".into(),
                    origin: Origin::Computed("estimate"),
                },
            ],
            marks: vec![
                MarkRow { name: "0-10".into(), seconds: Some(1.04), from_launch: true },
                MarkRow { name: "50-100".into(), seconds: Some(3.24), from_launch: false },
                MarkRow { name: "0-100".into(), seconds: None, from_launch: true },
            ],
            series: vec![
                Series {
                    label: "speed".into(),
                    unit: "km/h".into(),
                    points: ramp(4.0, 0.0),
                    origin: Origin::Bus,
                },
                Series {
                    label: "engine speed".into(),
                    unit: "/min".into(),
                    points: saw(1800.0, 6400.0),
                    origin: Origin::Bus,
                },
                Series {
                    label: "power".into(),
                    unit: "kW".into(),
                    points: ramp(9.0, 4.0),
                    origin: Origin::Computed("estimate"),
                },
                Series {
                    label: "accel".into(),
                    unit: "m/s²".into(),
                    points: ramp(0.2, 3.0),
                    origin: Origin::Computed("trailing"),
                },
            ],
            chart: 0,
            hz: Some(21.4),
            file: Some("drive.json".into()),
            warning: None,
            table: None,
        }
    }

    #[test]
    #[ignore = "not an assertion — prints the screen so a person can read it"]
    fn show() {
        for (w, h) in [(120u16, 40u16), (100, 30), (80, 24)] {
            println!("\n=== {w}×{h} ===");
            println!("{}", screen_text(&demo_screen(), w, h));
            let mut second = demo_screen();
            second.chart = 1;
            println!("--- page 2 ---");
            println!("{}", screen_text(&second, w, h));
            let done = Screen {
                band: band(&Phase::Done { seconds: Some(6.12) }, None),
                table: Some(
                    "  Run 1 — measured\n    0-10    1.04 s (0.94 … 1.13)\n    \
                     0-100   6.12 s (6.03 … 6.38)\n    50-100  3.24 s ± 0.02\n"
                        .into(),
                ),
                hz: Some(21.4),
                file: Some("drive.json".into()),
                ..Screen::default()
            };
            println!("--- after the run ---");
            println!("{}", screen_text(&done, w, h));
        }
    }

    #[test]
    fn every_state_the_machine_can_be_in_has_a_band() {
        // The state machine is otherwise invisible, and the first thing a new
        // user meets is a still screen with nothing explaining it.
        let cases = [
            (Phase::Waiting { speed_kmh: 0.4 }, "WAITING — come to a full stop to arm"),
            (Phase::Arming { remaining_s: 0.6 }, "ARMING — hold still  0.6 s"),
            (Phase::Armed, "ARMED"),
            (Phase::Running { elapsed_s: 4.31 }, "RUN  4.31 s"),
            (Phase::Done { seconds: Some(6.12) }, "DONE  6.12 s"),
            (Phase::Aborted { at_kmh: 82.0, kept: vec!["0-10".into()] }, "ABORTED at 82 km/h"),
            (Phase::Paused, "PAUSED — will not arm."),
        ];
        for (phase, expected) in cases {
            let text = band(&phase, None);
            assert!(text.contains(expected), "{phase:?} → {text:?}");
        }
    }

    #[test]
    fn waiting_shows_the_speed_that_is_keeping_it_waiting() {
        // Arming needs a true zero. A car creeping at 0.4 km/h would otherwise
        // sit there looking broken with nothing on screen to explain it.
        assert!(band(&Phase::Waiting { speed_kmh: 0.4 }, None).contains("0.4 km/h"));
    }

    #[test]
    fn an_aborted_run_names_the_marks_it_kept() {
        // A run that died at 80 still measured 0-60, and the band is where that
        // is said while the car is rolling to a stop.
        let phase = Phase::Aborted {
            at_kmh: 82.0,
            kept: vec!["0-10".into(), "0-25".into(), "0-50".into(), "0-60".into()],
        };
        assert_eq!(band(&phase, None), "ABORTED at 82 km/h — kept 0-10, 0-25, 0-50, 0-60");
    }

    #[test]
    fn a_collapsed_rate_appends_to_the_band_rather_than_replacing_it() {
        // The state is still the state; what changed is what the times are
        // worth.
        let text = band(&Phase::Running { elapsed_s: 4.31 }, Some(6.0));
        assert!(text.starts_with("RUN  4.31 s"), "{text}");
        assert!(text.contains("SLOW — 6 Hz, times less certain"), "{text}");
    }

    #[test]
    fn the_band_follows_the_state_machine_and_the_hold_counts_down() {
        let phase = phase_of(session::State::Arming { since: 10.0 }, 0.0, 10.4, None);
        let Phase::Arming { remaining_s } = phase else { panic!("{phase:?}") };
        assert!((remaining_s - (ARMING_HOLD_S - 0.4)).abs() < 1e-9, "{remaining_s}");
        assert_eq!(
            phase_of(session::State::Idle, 0.4, 1.0, None),
            Phase::Waiting { speed_kmh: 0.4 }
        );
        // Finished reports whichever way the last run ended.
        let aborted = Outcome::Aborted { at_kmh: 82.0, kept: vec!["0-60".into()] };
        assert!(matches!(
            phase_of(session::State::Finished, 0.0, 9.0, Some(&aborted)),
            Phase::Aborted { .. }
        ));
    }

    #[test]
    fn quitting_with_unsaved_runs_does_not_quit_and_a_second_q_discards() {
        // Two keystrokes to throw away a drive, one to keep it.
        let mut controls = Controls { charts: 2, ..Controls::default() };
        let refused = on_key(&mut controls, KeyCode::Char('q'), 4);
        assert!(matches!(refused, Action::Refuse(ref text) if text.contains("[s] save")), "{refused:?}");
        assert_eq!(on_key(&mut controls, KeyCode::Char('q'), 4), Action::Quit);

        // Nothing outstanding, nothing to argue about.
        let mut controls = Controls::default();
        assert_eq!(on_key(&mut controls, KeyCode::Char('q'), 0), Action::Quit);
    }

    #[test]
    fn the_quit_guard_is_disarmed_by_thinking_better_of_it() {
        // `q`, then `s`, then a stray `q` much later must not lose the drive.
        let mut controls = Controls::default();
        assert!(matches!(on_key(&mut controls, KeyCode::Char('q'), 4), Action::Refuse(_)));
        assert_eq!(on_key(&mut controls, KeyCode::Char('s'), 4), Action::Save);
        assert!(matches!(on_key(&mut controls, KeyCode::Char('q'), 4), Action::Refuse(_)));
    }

    #[test]
    fn the_arrow_keys_walk_the_charts_and_wrap() {
        let mut controls = Controls { charts: 3, ..Controls::default() };
        assert_eq!(on_key(&mut controls, KeyCode::Right, 0), Action::Nothing);
        assert_eq!(controls.chart, 1);
        on_key(&mut controls, KeyCode::Left, 0);
        assert_eq!(controls.chart, 0);
        on_key(&mut controls, KeyCode::Left, 0);
        assert_eq!(controls.chart, 2, "wraps rather than running off the end");
        // With nothing to show there is nothing to switch between.
        controls.charts = 0;
        on_key(&mut controls, KeyCode::Right, 0);
        assert_eq!(controls.chart, 2);
    }

    #[test]
    fn escape_cancels_the_run_here_rather_than_quitting() {
        // A deliberate divergence from `watch`: a stopwatch needs a cheap
        // "throw this one away", and `watch` has nothing to throw away.
        let mut controls = Controls { charts: 1, ..Controls::default() };
        assert_eq!(on_key(&mut controls, KeyCode::Esc, 0), Action::Session(session::Command::Cancel));
        assert_eq!(
            on_key(&mut controls, KeyCode::Char('p'), 0),
            Action::Session(session::Command::PauseTrigger)
        );
    }

    #[test]
    fn cancel_has_a_plain_letter_as_well_and_it_is_the_one_on_screen() {
        // A control a driver needs mid-run must not rest on the terminal
        // agreeing about escape sequences. `Esc` keeps working; `c` is the one
        // the hints name, because it is the one that cannot stop working.
        let mut controls = Controls::default();
        assert_eq!(
            on_key(&mut controls, KeyCode::Char('c'), 0),
            Action::Session(session::Command::Cancel)
        );
        assert!(HINTS.contains("[c]ancel"), "{HINTS}");
    }

    #[test]
    fn a_finished_run_can_be_thrown_away_or_kept_and_both_are_deferred() {
        // Neither may happen inside the key handler: what a discard has to
        // reach — the recorded runs and the file — belongs to the loop, and a
        // file must never appear between two batches of one cycle.
        let mut controls = Controls::default();
        assert_eq!(on_key(&mut controls, KeyCode::Char('d'), 0), Action::Discard);
        assert_eq!(on_key(&mut controls, KeyCode::Enter, 0), Action::KeepGoing);
        assert!(HINTS.contains("[d]iscard") && HINTS.contains("[↵]keep&next"), "{HINTS}");

        // And they are on the screen they are for — the one a run ends on,
        // which is the whole of what is up while the driver decides.
        let done = Screen {
            band: band(&Phase::Done { seconds: Some(6.12) }, None),
            table: Some("  Run 1 — measured\n    0-100  6.12 s (6.03 … 6.38)\n".into()),
            hz: Some(21.4),
            ..Screen::default()
        };
        for (w, h) in [(120u16, 40u16), (100, 30), (80, 24)] {
            let text = screen_text(&done, w, h);
            assert!(text.contains("[d]iscard"), "{w}×{h}:\n{text}");
            assert!(text.contains("[↵]keep&next"), "{w}×{h}:\n{text}");
        }
    }

    #[test]
    fn a_discard_says_whether_anything_had_already_been_written() {
        // "Discarded" on its own is a half-truth for a run `--out` already put
        // on disk, and "rewritten" is a half-truth the other way for one that
        // was never written at all.
        let written = discarded(3, Some("drive.json"), 2);
        assert!(written.contains("drive.json") && written.contains("rewritten"), "{written}");
        assert!(written.contains("2 run"), "{written}");
        let never = discarded(3, None, 0);
        assert!(never.contains("never written"), "{never}");
        assert!(!never.contains("rewritten"), "{never}");
    }

    #[test]
    fn a_key_that_needs_a_finished_run_says_so_when_there_is_none() {
        // Silence is what made the cancel bug unreadable from the driver's
        // seat; the same family of keys does not get to repeat it.
        for text in [nothing_to_discard(), nothing_to_keep(), nothing_to_cancel()] {
            assert!(!text.is_empty());
        }
        assert!(nothing_to_discard().contains("[d]"));
        assert!(nothing_to_keep().contains("[↵]"));
    }

    #[test]
    fn the_footer_keeps_the_keys_when_it_has_to_drop_something() {
        // The keys are the only thing on the footer a driver cannot work out
        // for themselves, so the rate and the file name go first.
        let screen = demo_screen();
        let wide = status_line(&screen, 200);
        assert!(wide.contains("21.4 Hz") && wide.contains("drive.json"), "{wide}");
        for width in [60usize, 45, 30] {
            let line = status_line(&screen, width);
            assert!(line.contains("[c]ancel"), "{width}: {line}");
        }
        assert!(!status_line(&screen, 60).contains("drive.json"), "the file goes first");
    }

    #[test]
    fn a_launch_mark_is_an_estimate_on_screen_and_never_a_lower_bound() {
        // There is room for one number here and no time to read a second; the
        // interval waits for the results table.
        let closed = MarkRow { name: "0-10".into(), seconds: Some(1.04), from_launch: true };
        assert_eq!(closed.value(), "≈1.04 s");
        let rolling = MarkRow { name: "50-100".into(), seconds: Some(3.24), from_launch: false };
        assert_eq!(rolling.value(), "3.24 s");
        let open = MarkRow { name: "0-100".into(), seconds: None, from_launch: true };
        assert_eq!(open.value(), "·");
    }

    #[test]
    fn no_mark_is_spelled_as_a_lower_bound_anywhere_on_the_screen() {
        // `1.2+ s` was the retracted one-signed launch model, and on the car it
        // read as a line that had been cut off. Neither may come back by way of
        // a helper nobody re-checked.
        for seconds in [0.0, 1.04, 9.087, 123.4] {
            for from_launch in [true, false] {
                let row = MarkRow { name: "0-100".into(), seconds: Some(seconds), from_launch };
                assert!(!row.value().contains('+'), "{:?}", row.value());
            }
        }
        let screen = demo_screen();
        for (w, h) in [(120u16, 40u16), (100, 30), (80, 24)] {
            let text = screen_text(&screen, w, h);
            assert!(!text.contains("+ s"), "{w}×{h}:\n{text}");
        }
    }

    #[test]
    fn the_marks_panel_prints_every_mark_whole_at_every_size_it_claims_to_fit() {
        // The panel takes its width from what it is about to print, so the two
        // have to be checked against each other rather than assumed to agree.
        let screen = demo_screen();
        for (w, h) in [(120u16, 40u16), (100, 30), (80, 24)] {
            let text = screen_text(&screen, w, h);
            for expected in ["0-10", "≈1.04 s", "50-100", "3.24 s", "0-100"] {
                assert!(text.contains(expected), "{w}×{h} lost {expected}:\n{text}");
            }
        }
    }

    #[test]
    fn a_page_stops_at_three_lines_and_two_scales_and_the_rest_gets_its_own_page() {
        let series = |label: &str, unit: &str| Series {
            label: label.into(),
            unit: unit.into(),
            points: Track::default(),
            origin: Origin::Bus,
        };
        // Four units: two per page, in the order they were offered.
        let four = [
            series("speed", "km/h"),
            series("engine speed", "/min"),
            series("power", "kW"),
            series("accel", "m/s²"),
        ];
        assert_eq!(pages(&four), vec![vec![0, 1], vec![2, 3]]);

        // Series that share a unit share a scale, so three of them are one page
        // — and a fourth starts another even though it costs no new scale.
        let same = [
            series("boost actual", "bar"),
            series("boost specified", "bar"),
            series("boost peak", "bar"),
            series("boost held", "bar"),
        ];
        assert_eq!(pages(&same), vec![vec![0, 1, 2], vec![3]]);

        assert!(pages(&[]).is_empty(), "nothing read is no pages, not one empty one");
    }

    #[test]
    fn the_chart_says_what_it_is_plotting_in_what_unit_and_where_it_came_from() {
        // "Я не сразу понял что означает график": a border reading `speed` and
        // two bare numbers is not enough to read a chart from.
        let screen = demo_screen();
        for (w, h) in [(120u16, 40u16), (100, 30), (80, 24)] {
            let text = screen_text(&screen, w, h);
            assert!(text.contains("speed"), "{w}×{h} does not name the line:\n{text}");
            assert!(text.contains("engine speed"), "{w}×{h} plots one line, not two:\n{text}");
            assert!(text.contains("km/h"), "{w}×{h} drops the axis unit:\n{text}");
            assert!(text.contains("time"), "{w}×{h} does not say what it is over:\n{text}");
            // The folded line's own numbers are not on the axis, so they are in
            // the key: a curve whose axis is nowhere is decoration.
            assert!(text.contains("/min]"), "{w}×{h} folds a line and hides its scale:\n{text}");
            assert!(
                text.contains("own scale"),
                "{w}×{h} folds a line without admitting it:\n{text}"
            );
        }
    }

    #[test]
    fn a_computed_line_never_passes_for_a_measured_one() {
        // Power and live acceleration were never on the bus. The value table
        // says so in a column of its own and the chart must not be the one
        // place the distinction is dropped.
        let mut screen = demo_screen();
        screen.chart = 1;
        for (w, h) in [(120u16, 40u16), (100, 30), (80, 24)] {
            let text = screen_text(&screen, w, h);
            assert!(text.contains("⋯power"), "{w}×{h}:\n{text}");
            assert!(text.contains("⋯ computed"), "{w}×{h} draws it with no key:\n{text}");
            assert!(text.contains("estimate"), "{w}×{h} loses the qualifier:\n{text}");
            assert!(text.contains("trailing"), "{w}×{h} loses the causal note:\n{text}");
        }
    }

    #[test]
    fn a_chart_with_no_room_for_a_line_says_it_dropped_it() {
        // Degrading is allowed; degrading quietly is not.
        let screen = demo_screen();
        let narrow = screen_text(&screen, 44, 20);
        assert!(narrow.contains("no room"), "{narrow}");
        // And the line that is left is still named, so the chart never becomes
        // an unlabelled curve again.
        assert!(narrow.contains("speed"), "{narrow}");
    }

    #[test]
    fn a_row_says_whether_its_number_was_on_the_bus() {
        // A number that was never on the bus must not look like one that was.
        assert_eq!(Origin::Bus.columns(), ("bus", ""));
        assert_eq!(Origin::Computed("trailing").columns(), ("computed", "trailing"));
        assert_eq!(Origin::Computed("estimate").columns(), ("computed", "estimate"));
    }

    #[test]
    fn quiet_silences_every_tone_and_the_three_are_distinguishable() {
        for tone in [Tone::Mark, Tone::Finished, Tone::Rejected] {
            assert_eq!(player(tone, true), None, "--quiet is not a suggestion");
        }
        if cfg!(target_os = "macos") {
            let sounds: Vec<String> = [Tone::Mark, Tone::Finished, Tone::Rejected]
                .into_iter()
                .map(|tone| player(tone, false).expect("macOS has sounds").1)
                .collect();
            assert!(sounds.iter().all(|path| path.starts_with(SOUNDS)), "{sounds:?}");
            let mut unique = sounds.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), 3, "a finished run does not sound like a mark");
        }
    }

    #[test]
    fn a_tone_does_not_block_the_poll_loop() {
        // A poll loop that waits on audio puts the sound ahead of the
        // measurement. Three tones back to back are spawns, not playbacks.
        let started = std::time::Instant::now();
        for tone in [Tone::Mark, Tone::Finished, Tone::Rejected] {
            play(tone, false);
        }
        assert!(started.elapsed().as_millis() < 500, "{:?}", started.elapsed());
    }

    #[test]
    fn without_a_terminal_the_same_screen_becomes_a_line() {
        let screen = Screen {
            band: band(&Phase::Running { elapsed_s: 4.31 }, None),
            rows: vec![ValueRow {
                name: "speed".into(),
                value: "62.4 km/h".into(),
                origin: Origin::Bus,
            }],
            marks: vec![
                MarkRow { name: "0-10".into(), seconds: Some(1.04), from_launch: true },
                MarkRow { name: "0-100".into(), seconds: None, from_launch: true },
            ],
            ..Screen::default()
        };
        let line = plain_line(&screen);
        assert!(line.starts_with("RUN  4.31 s"), "{line}");
        assert!(line.contains("speed 62.4 km/h"), "{line}");
        assert!(line.contains("0-10 ≈1.04 s"), "{line}");
        assert!(!line.contains("0-100"), "an open mark has nothing to report yet: {line}");
    }

    /// Drawing is exercised against a test backend rather than asserted
    /// pixel by pixel: what this catches is a panic — a zero-width constraint, a
    /// chart with no samples, an empty marks panel — on a screen nobody can see
    /// until they are in a car.
    #[test]
    fn the_screen_draws_at_any_size_and_with_nothing_in_it() {
        let mut speed = Track::default();
        for i in 0..40 {
            speed.push(f64::from(i) * 0.05, f64::from(i));
        }
        let full = Screen {
            band: band(&Phase::Running { elapsed_s: 4.31 }, Some(6.0)),
            banner: Some("no car file for XW8 — default mode".into()),
            rows: vec![
                ValueRow { name: "speed".into(), value: "62.4 km/h".into(), origin: Origin::Bus },
                ValueRow {
                    name: "accel".into(),
                    value: "0.41 g".into(),
                    origin: Origin::Computed("trailing"),
                },
            ],
            marks: vec![MarkRow { name: "0-100".into(), seconds: None, from_launch: true }],
            series: vec![
                Series {
                    label: "speed".into(),
                    unit: "km/h".into(),
                    points: speed,
                    origin: Origin::Bus,
                },
                // A series with nothing in it yet, which is every series for the
                // first cycle of every run.
                Series {
                    label: "accel".into(),
                    unit: "m/s²".into(),
                    points: Track::default(),
                    origin: Origin::Computed("trailing"),
                },
            ],
            chart: 1,
            hz: Some(21.4),
            file: Some("drive.json".into()),
            warning: None,
            table: None,
        };
        let stopped = Screen {
            band: band(&Phase::Done { seconds: Some(6.12) }, None),
            table: Some("  Run 1 — measured\n    0-100  6.03 … 6.38 s\n".into()),
            ..Screen::default()
        };
        for screen in [full, stopped, Screen::default()] {
            for (w, h) in [(120u16, 40u16), (40, 12), (20, 6)] {
                let backend = ratatui::backend::TestBackend::new(w, h);
                let mut terminal = Terminal::new(backend).unwrap();
                terminal.draw(|frame| draw(frame, &screen)).unwrap();
            }
        }
    }
}
