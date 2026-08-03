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
//! **The table carries every value; the chart carries one.** Ten rows are
//! readable and ten series are not, so one series is drawn at a time, switched
//! with the arrow keys and named in its own border.
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
/// A launch-based mark carries a trailing `+` here and its interval waits for
/// the results table: there is room for one number on this screen and no time to
/// read a second.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkRow {
    pub name: String,
    /// `None` until it closes, drawn as a placeholder rather than left blank —
    /// a gap reads as a mark that was not asked for.
    pub seconds: Option<Seconds>,
    pub from_launch: bool,
}

impl MarkRow {
    fn value(&self) -> String {
        match (self.seconds, self.from_launch) {
            (Some(seconds), true) => format!("{seconds:.1}+ s"),
            (Some(seconds), false) => format!("{seconds:.2} s"),
            (None, _) => "·".to_string(),
        }
    }
}

/// One series the chart can show.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    /// What the border calls it.
    pub label: String,
    pub points: Track,
    /// A derived series' running end is causal by construction, and the border
    /// says so rather than leaving it to be assumed.
    pub causal: bool,
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
    /// How many series there are to walk. Kept here rather than passed in, so
    /// that a caller cannot hold the keyboard state and the count at once and
    /// have them disagree.
    pub charts: usize,
    /// Set by the first `q` with unsaved runs, cleared by anything else. Two
    /// keystrokes to throw away a drive, one to keep it.
    quit_armed: bool,
    /// Set by `s`. The write itself is deferred out of the key handler, so that
    /// a file is never created between two batches of one cycle.
    save_requested: bool,
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
}

/// What a keystroke asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Nothing,
    /// Hand this to the state machine.
    Session(session::Command),
    /// Write the session out, then tell the state machine it is saved.
    Save,
    Quit,
    /// `q` with unsaved runs: the message to put on screen, and no quit.
    Refuse(String),
}

/// Handle one key.
///
/// `Esc` cancels a run here and quits in `watch`. That is a deliberate
/// divergence rather than an oversight — a stopwatch needs a cheap "throw this
/// one away" and `watch` has nothing to throw away — and it is written down so
/// that it stays deliberate.
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
        KeyCode::Esc => Action::Session(session::Command::Cancel),
        KeyCode::Char('s') => Action::Save,
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
    " [p] pause trigger  [esc] cancel run  [s] save  [←→] series  [q] quit";

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
            Paragraph::new(status_line(screen)).block(Block::default().borders(Borders::ALL)),
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
        Paragraph::new(status_line(screen)).block(Block::default().borders(Borders::ALL)),
        outer[3],
    );
}

/// The footer: the achieved rate, whether a file is open, and the keys.
fn status_line(screen: &Screen) -> String {
    if let Some(warning) = &screen.warning {
        return warning.clone();
    }
    let rate = screen.hz.map(|hz| format!("{hz:.1} Hz · ")).unwrap_or_default();
    let file =
        screen.file.as_deref().map(|path| format!("  ·  writing {path}")).unwrap_or_default();
    format!(" {rate}{}{file}", HINTS.trim_start())
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

/// One series, named in its own border.
///
/// Drawn from the accumulated run buffer rather than from the last point alone,
/// so the shape of the run is visible while it is happening.
fn draw_chart(frame: &mut Frame, screen: &Screen, area: Rect) {
    let Some(series) = screen.series.get(screen.chart) else {
        frame.render_widget(Block::default().borders(Borders::ALL), area);
        return;
    };
    let points: Vec<(f64, f64)> =
        (0..series.points.len()).map(|i| (series.points.t[i], series.points.v[i])).collect();
    let bounds = |values: &[f64]| {
        let low = values.iter().copied().fold(f64::INFINITY, f64::min);
        let high = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        match low.is_finite() && high.is_finite() && high > low {
            true => [low, high],
            // A flat or empty series still needs an axis, or the chart draws
            // nothing and looks like a failure to read the car.
            false => [0.0, 1.0],
        }
    };
    let x = bounds(&series.points.t);
    let y = bounds(&series.points.v);

    let causal = if series.causal { " (trailing)" } else { "" };
    let title = format!(" {}{causal} ── ← → to change ", series.label);
    let data = [Dataset::default()
        .marker(Marker::Braille)
        .graph_type(GraphType::Line)
        .style(Style::default().fg(Color::Cyan))
        .data(&points)];
    let chart = Chart::new(data.to_vec())
        .block(Block::default().borders(Borders::ALL).title(title))
        .x_axis(
            Axis::default()
                .bounds(x)
                .labels([format!("{:.0}s", x[0]), format!("{:.0}s", x[1])]),
        )
        .y_axis(
            Axis::default()
                .bounds(y)
                .labels([format!("{:.1}", y[0]), format!("{:.1}", y[1])]),
        );
    frame.render_widget(chart, area);
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
    fn a_launch_mark_carries_a_plus_on_screen_and_an_open_one_a_placeholder() {
        // There is room for one number here and no time to read a second; the
        // interval waits for the results table.
        let closed = MarkRow { name: "0-10".into(), seconds: Some(1.04), from_launch: true };
        assert_eq!(closed.value(), "1.0+ s");
        let rolling = MarkRow { name: "50-100".into(), seconds: Some(3.24), from_launch: false };
        assert_eq!(rolling.value(), "3.24 s");
        let open = MarkRow { name: "0-100".into(), seconds: None, from_launch: true };
        assert_eq!(open.value(), "·");
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
        assert!(line.contains("0-10 1.0+ s"), "{line}");
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
                Series { label: "speed".into(), points: speed, causal: false },
                // A series with nothing in it yet, which is every series for the
                // first cycle of every run.
                Series { label: "accel".into(), points: Track::default(), causal: true },
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
