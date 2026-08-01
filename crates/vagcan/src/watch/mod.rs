//! `vagcan watch` — a live view of the car, configured from inside.
//!
//! Values are picked on a selection screen rather than by flags, and several
//! control units appear together: each is addressed in turn over the one
//! serial link and they share a single table. The catalogs cover the engine,
//! the gearbox and the instrument cluster; `--survey` adds every identifier a
//! `vagcan survey` run found on any other unit, shown as raw bytes.
//!
//! The previous version drew with carriage returns, which only works on a
//! terminal that honours them — piped or resized, it left a trail of new lines
//! instead of updating one. A full-screen renderer has no such failure mode,
//! and it can also show a name in full instead of eliding it to fit a column.

pub mod plan;

use std::io;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};

use plan::Channel;

/// Which screen has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Live,
    Select,
}

/// One line of the live table: a measurement, and its specified counterpart
/// when the unit publishes one.
struct DisplayRow<'a> {
    label: String,
    actual: Option<&'a Channel>,
    specified: Option<&'a Channel>,
}

impl<'a> DisplayRow<'a> {
    /// Either half — they always agree on unit and control unit.
    fn any(&self) -> &'a Channel {
        self.actual.or(self.specified).expect("a row holds at least one channel")
    }
}

/// Everything the UI needs to draw itself.
pub struct App {
    pub channels: Vec<Channel>,
    /// Latest response body per `(request id, did)`, with when it arrived.
    pub latest: std::collections::BTreeMap<(u16, u16), (f64, Vec<u8>)>,
    screen: Screen,
    cursor: usize,
    /// Completed poll cycles, and when the run started.
    cycles: u64,
    started: Instant,
    status: String,
}

impl App {
    pub fn new(channels: Vec<Channel>) -> Self {
        App {
            channels,
            latest: std::collections::BTreeMap::new(),
            screen: Screen::Live,
            cursor: 0,
            cycles: 0,
            started: Instant::now(),
            status: String::new(),
        }
    }

    /// Rows currently on screen, in the order the plan polls them.
    fn shown(&self) -> Vec<&Channel> {
        let mut v: Vec<&Channel> = self.channels.iter().filter(|c| c.selected).collect();
        v.sort_by_key(|c| (c.request, c.did));
        v
    }

    /// What one line of the table shows.
    ///
    /// Usually one channel. When a unit publishes a quantity as both what it
    /// asked for and what it got, the two share a line: the number that matters
    /// is the gap between them, and two lines apart it has to be computed by
    /// eye.
    fn rows(&self) -> Vec<DisplayRow<'_>> {
        let shown = self.shown();
        let mut out: Vec<DisplayRow> = Vec::new();
        for c in shown {
            let paired = c.def.as_ref().and_then(|d| plan::split_role(&d.name));
            let Some((base, role)) = paired else {
                out.push(DisplayRow { label: c.label(), actual: Some(c), specified: None });
                continue;
            };
            // Same base name on the same unit — a pair from another control
            // unit is a different quantity that happens to share a name.
            let slot = out.iter_mut().find(|r| {
                r.label == base
                    && r.any().request == c.request
                    && match role {
                        plan::Role::Actual => r.actual.is_none(),
                        plan::Role::Specified => r.specified.is_none(),
                    }
            });
            match (slot, role) {
                (Some(row), plan::Role::Actual) => row.actual = Some(c),
                (Some(row), plan::Role::Specified) => row.specified = Some(c),
                (None, plan::Role::Actual) => out.push(DisplayRow {
                    label: base.to_string(),
                    actual: Some(c),
                    specified: None,
                }),
                (None, plan::Role::Specified) => out.push(DisplayRow {
                    label: base.to_string(),
                    actual: None,
                    specified: Some(c),
                }),
            }
        }
        out
    }

    /// The text for a row's value cell, and how old the reading is.
    fn value_of(&self, row: &DisplayRow) -> (String, String) {
        let read = |c: Option<&Channel>| {
            c.and_then(|c| self.latest.get(&(c.request, c.did)).map(|(t, d)| (c.render(d), *t)))
        };
        match (read(row.actual), read(row.specified)) {
            (Some((a, t)), Some((s, u))) => {
                // Both halves carry the unit; printing it twice on one line
                // reads as two different quantities.
                let unit = row.any().unit_of_measure();
                let a = match unit.is_empty() {
                    false => a.strip_suffix(unit).map(|t| t.trim_end().to_string()).unwrap_or(a),
                    true => a,
                };
                (format!("{a} / {s}"), self.age(t.min(u)))
            }
            (Some((a, t)), None) => (a, self.age(t)),
            (None, Some((s, t))) => (format!("— / {s}"), self.age(t)),
            (None, None) => ("—".to_string(), String::new()),
        }
    }

    fn age(&self, at: f64) -> String {
        format!("{:.1}s", self.started.elapsed().as_secs_f64() - at)
    }

    fn poll_rate(&self) -> f64 {
        let secs = self.started.elapsed().as_secs_f64();
        if secs <= 0.0 { 0.0 } else { self.cycles as f64 / secs }
    }
}

/// Draw the live table.
///
/// Column widths come from the content, so a long name is shown in full rather
/// than elided — the whole reason for moving off a single scrolling line.
fn draw_live(frame: &mut Frame, app: &App) {
    let shown = app.rows();
    let rows: Vec<Row> = shown
        .iter()
        .map(|r| {
            let (value, age) = app.value_of(r);
            let c = r.any();
            // A pair is addressed by two identifiers; showing both keeps the
            // line honest about where the numbers came from.
            let dids = match (r.actual, r.specified) {
                (Some(a), Some(s)) => format!("{:04X}/{:04X}", a.did, s.did),
                _ => format!("{:04X}", c.did),
            };
            Row::new(vec![
                Cell::from(c.unit()),
                Cell::from(dids),
                Cell::from(r.label.clone()),
                Cell::from(value).style(Style::default().add_modifier(Modifier::BOLD)),
                Cell::from(c.unit_of_measure().to_string()),
                Cell::from(age).style(Style::default().fg(Color::DarkGray)),
            ])
        })
        .collect();

    let name_w = shown.iter().map(|r| r.label.len()).max().unwrap_or(4).max(11) as u16;
    let did_w = shown
        .iter()
        .map(|r| if r.actual.is_some() && r.specified.is_some() { 9 } else { 4 })
        .max()
        .unwrap_or(4) as u16;
    let value_w = shown
        .iter()
        .map(|r| app.value_of(r).0.len())
        .max()
        .unwrap_or(8)
        .max(14) as u16;

    let layout = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(frame.area());
    let table = Table::new(
        rows,
        [
            Constraint::Length(4),
            Constraint::Length(did_w),
            Constraint::Length(name_w),
            Constraint::Length(value_w),
            Constraint::Length(9),
            Constraint::Length(6),
        ],
    )
    .header(
        Row::new(vec!["ECU", "DID", "Measurement", "Actual / specified", "Unit", "Age"])
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    )
    .block(Block::default().borders(Borders::ALL).title(" vagcan watch "));
    frame.render_widget(table, layout[0]);

    let help = format!(
        " {:.1} Hz · {} of {} shown · [c] configure  [q] quit {}",
        app.poll_rate(),
        shown.len(),
        app.channels.len(),
        app.status
    );
    frame.render_widget(
        Paragraph::new(help).block(Block::default().borders(Borders::ALL)),
        layout[1],
    );
}

/// Draw the selection screen.
fn draw_select(frame: &mut Frame, app: &App) {
    let rows: Vec<Row> = app
        .channels
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mark = if c.selected { "[x]" } else { "[ ]" };
            let style = if i == app.cursor {
                Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(vec![
                Cell::from(mark),
                Cell::from(c.unit()),
                Cell::from(format!("{:04X}", c.did)),
                Cell::from(c.label()),
                Cell::from(c.unit_of_measure().to_string()),
            ])
            .style(style)
        })
        .collect();

    let name_w = app.channels.iter().map(|c| c.label().len()).max().unwrap_or(4) as u16;
    let layout = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(frame.area());
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(5),
            Constraint::Length(name_w),
            Constraint::Length(9),
        ],
    )
    .block(Block::default().borders(Borders::ALL).title(" choose what to show "));
    frame.render_widget(table, layout[0]);
    frame.render_widget(
        Paragraph::new(" [space] toggle  [↑↓] move  [a] all  [n] none  [enter] back ")
            .block(Block::default().borders(Borders::ALL)),
        layout[1],
    );
}

/// Handle one key. Returns false when the user asked to quit.
fn on_key(app: &mut App, code: KeyCode) -> bool {
    match app.screen {
        Screen::Live => match code {
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Char('c') => app.screen = Screen::Select,
            _ => {}
        },
        Screen::Select => match code {
            KeyCode::Char('q') => return false,
            KeyCode::Enter | KeyCode::Esc | KeyCode::Char('c') => app.screen = Screen::Live,
            KeyCode::Up => app.cursor = app.cursor.saturating_sub(1),
            KeyCode::Down => {
                if app.cursor + 1 < app.channels.len() {
                    app.cursor += 1;
                }
            }
            KeyCode::Char(' ') => {
                if let Some(c) = app.channels.get_mut(app.cursor) {
                    c.selected = !c.selected;
                }
            }
            KeyCode::Char('a') => app.channels.iter_mut().for_each(|c| c.selected = true),
            KeyCode::Char('n') => app.channels.iter_mut().for_each(|c| c.selected = false),
            _ => {}
        },
    }
    true
}

/// Run the live view against a real adapter.
pub async fn run(
    device_path: &str,
    baud: u32,
    preselect: &[(u16, u16)],
    hz: f64,
    out: Option<&str>,
    survey: Option<&str>,
    catalogs: &str,
) -> Result<()> {
    use std::io::Write as _;
    use vag_can::{IsoTpCan, SlcanBackend, SlcanBitrate, SlcanMode};
    use vag_protocol::AsyncUdsClient;
    use vag_transport::CanId;

    let store = vag_data::catalog::CatalogStore::open(catalogs);
    let survey_text = match survey {
        Some(path) => Some(
            std::fs::read_to_string(path)
                .with_context(|| format!("reading the survey {path:?}"))?,
        ),
        None => None,
    };

    // Which scalings apply is decided by what each unit says it is, never by
    // its address. A survey already asked; without one, the units to be polled
    // are asked directly below.
    let mut identities = match &survey_text {
        Some(text) => plan::identities_from_survey(text),
        None => Vec::new(),
    };

    let mut adapter =
        SlcanBackend::open_mode(device_path, baud, SlcanBitrate::Rate500k, SlcanMode::Normal)
            .await
            .with_context(|| format!("opening the adapter at {device_path}"))?;

    // Any unit that will be polled but was not in the survey is asked for its
    // identification block now — two reads, once, at startup.
    let mut wanted: Vec<u16> = preselect.iter().map(|(request, _)| *request).collect();
    wanted.push(plan::ENGINE);
    wanted.sort_unstable();
    wanted.dedup();
    for request in wanted {
        if identities.iter().any(|i| i.request == request) {
            continue;
        }
        let Some(address) = vag_protocol::address::UnitAddress::from_request(request) else {
            continue;
        };
        let mut uds = AsyncUdsClient::new(IsoTpCan::new(
            adapter,
            CanId::Standard(address.request),
            CanId::Standard(address.response),
        ));
        let text = |data: Option<Vec<u8>>| {
            data.map(|b| String::from_utf8_lossy(&b).trim_end_matches(['\0', ' ']).to_string())
                .filter(|s| !s.is_empty())
        };
        let part = text(uds.read_data_by_identifier(0xF187).await.ok());
        let odx = text(uds.read_data_by_identifier(0xF19E).await.ok());
        identities.push(plan::UnitIdentity { request, part_number: part, odx_name: odx });
        adapter = uds.into_transport().into_backend();
    }

    let mut channels = plan::available(&store, &identities);
    if let Some(text) = &survey_text {
        // Everything a survey found becomes watchable, on every unit — which
        // is the only way the units outside the catalogs get on screen at all.
        channels = plan::with_survey(channels, text);
    }
    for (request, did) in preselect {
        match channels.iter_mut().find(|c| c.request == *request && c.did == *did) {
            Some(c) => c.selected = true,
            None => channels.push(Channel {
                request: *request,
                did: *did,
                def: None,
                selected: true,
            }),
        }
    }
    if !channels.iter().any(|c| c.selected) {
        for c in channels.iter_mut().filter(|c| c.request == plan::ENGINE).take(8) {
            c.selected = true;
        }
    }
    let mut backend = Some(adapter);

    let mut sink = match out {
        Some(path) => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("creating {path:?}"))?;
            Some(std::io::BufWriter::new(file))
        }
        None => None,
    };
    let mut header_written = false;

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new(channels);
    let period = Duration::from_secs_f64(1.0 / hz.max(0.1));
    let result = loop {
        terminal.draw(|f| match app.screen {
            Screen::Live => draw_live(f, &app),
            Screen::Select => draw_select(f, &app),
        })?;

        // Drain the keyboard without blocking the poll loop. `q` here has to
        // leave the loop entirely, not just this drain — otherwise the key is
        // swallowed and a whole poll cycle runs before the quit takes effect.
        let mut quit = false;
        while event::poll(Duration::from_millis(0))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press && !on_key(&mut app, k.code) {
                    quit = true;
                    break;
                }
            }
        }
        if quit {
            break Ok(());
        }

        let cycle = Instant::now();
        for batch in plan::plan(&app.channels) {
            let Some(b) = backend.take() else { break };
            // Each unit is addressed by the rule its id block uses: the
            // cluster answers on 0x77E, not on 0x7E0 + 16, which is what
            // treating the unit number as an ISO index used to produce.
            let Some(address) = vag_protocol::address::UnitAddress::from_request(batch.request)
            else {
                backend = Some(b);
                continue;
            };
            let channel = IsoTpCan::new(
                b,
                CanId::Standard(address.request),
                CanId::Standard(address.response),
            );
            let mut uds = AsyncUdsClient::new(channel);
            let answer = if batch.dids.len() == 1 {
                uds.read_data_by_identifier(batch.dids[0])
                    .await
                    .map(|d| vec![(batch.dids[0], d)])
            } else {
                uds.read_data_by_identifiers(&batch.dids).await.map(|payload| {
                    crate::analyse::split_records(&payload, &batch.dids).unwrap_or_default()
                })
            };
            let at = app.started.elapsed().as_secs_f64();
            if let Ok(records) = answer {
                for (did, data) in records {
                    app.latest.insert((batch.request, did), (at, data));
                }
            }
            backend = Some(uds.into_transport().into_backend());
        }
        app.cycles += 1;

        if let Some(w) = sink.as_mut() {
            let shown = app.shown();
            if !header_written {
                // A raw column is marked, because a four-digit hex value and a
                // four-digit decimal are the same string — the reader cannot
                // tell them apart from the value alone.
                let cols: Vec<String> = shown
                    .iter()
                    .map(|c| {
                        let name = if c.def.is_some() {
                            c.label()
                        } else {
                            format!("{}_raw", c.label())
                        };
                        format!("{name}_t_s,{name}")
                    })
                    .collect();
                writeln!(w, "t_s,{}", cols.join(","))?;
                header_written = true;
            }
            let cells: Vec<String> = shown
                .iter()
                .map(|c| match app.latest.get(&(c.request, c.did)) {
                    Some((t, data)) => {
                        let v = match c.def.as_ref().and_then(|d| d.interpret(data)) {
                            Some(v) => format!("{v}"),
                            None => data.iter().map(|b| format!("{b:02X}")).collect(),
                        };
                        format!("{t:.3},{v}")
                    }
                    None => ",".to_string(),
                })
                .collect();
            writeln!(w, "{:.3},{}", app.started.elapsed().as_secs_f64(), cells.join(","))?;
        }

        // A key pressed during the poll should not wait a whole cycle.
        let mut quit = false;
        while let Some(rest) = period.checked_sub(cycle.elapsed()) {
            if !event::poll(rest.min(Duration::from_millis(50)))? {
                if cycle.elapsed() >= period {
                    break;
                }
                continue;
            }
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press && !on_key(&mut app, k.code) {
                    quit = true;
                    break;
                }
            }
        }
        if quit {
            break Ok(());
        }
    };

    disable_raw_mode()?;
    crossterm::execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    if let Some(w) = sink.as_mut() {
        w.flush()?;
    }
    println!(
        "{} cycles in {:.1}s — {:.1} Hz",
        app.cycles,
        app.started.elapsed().as_secs_f64(),
        app.poll_rate()
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference car's catalogs and identities — test fixtures, not a
    /// table the code carries.
    fn reference_channels() -> Vec<Channel> {
        let store = vag_data::catalog::CatalogStore::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../catalogs/vehicles"),
        );
        let ident = |request, part: &str| plan::UnitIdentity {
            request,
            part_number: Some(part.to_string()),
            odx_name: None,
        };
        plan::available(
            &store,
            &[
                ident(0x7E0, "8V0906264H"),
                ident(0x7E1, "0CW300041G"),
                ident(0x714, "5E0920740D"),
            ],
        )
    }

    fn app() -> App {
        let mut channels = reference_channels();
        channels[0].selected = true;
        channels[1].selected = true;
        App::new(channels)
    }

    #[test]
    fn the_configure_key_switches_screens_and_q_stops() {
        let mut a = app();
        assert_eq!(a.screen, Screen::Live);
        assert!(on_key(&mut a, KeyCode::Char('c')));
        assert_eq!(a.screen, Screen::Select);
        assert!(on_key(&mut a, KeyCode::Enter));
        assert_eq!(a.screen, Screen::Live);
        assert!(!on_key(&mut a, KeyCode::Char('q')));
    }

    #[test]
    fn toggling_changes_what_is_polled_without_a_restart() {
        let mut a = app();
        let before = plan::plan(&a.channels).len();
        a.screen = Screen::Select;
        a.cursor = 5;
        on_key(&mut a, KeyCode::Char(' '));
        assert!(a.channels[5].selected);
        // Selecting more can only add work, never remove it.
        assert!(plan::plan(&a.channels).len() >= before);

        on_key(&mut a, KeyCode::Char('n'));
        assert!(plan::plan(&a.channels).is_empty(), "none selected polls nothing");
        on_key(&mut a, KeyCode::Char('a'));
        assert!(!plan::plan(&a.channels).is_empty());
    }

    #[test]
    fn the_cursor_stays_inside_the_list() {
        let mut a = app();
        a.screen = Screen::Select;
        on_key(&mut a, KeyCode::Up);
        assert_eq!(a.cursor, 0, "cannot run off the top");
        a.cursor = a.channels.len() - 1;
        on_key(&mut a, KeyCode::Down);
        assert_eq!(a.cursor, a.channels.len() - 1, "cannot run off the bottom");
    }

    #[test]
    fn a_specified_value_shares_a_line_with_its_actual() {
        // Boost is published twice: 2029 is what the engine asked for, 202A is
        // what it got. Side by side the gap is readable at a glance.
        let mut a = App::new(reference_channels());
        for c in a.channels.iter_mut() {
            if c.request == 0x7E0 && (c.did == 0x2029 || c.did == 0x202A) {
                c.selected = true;
            }
        }
        let rows = a.rows();
        assert_eq!(rows.len(), 1, "one line, not two");
        assert_eq!(rows[0].label, "Boost pressure");
        assert_eq!(rows[0].actual.unwrap().did, 0x202A);
        assert_eq!(rows[0].specified.unwrap().did, 0x2029);

        // Before either has answered the line says so rather than showing a
        // stale or invented number.
        assert_eq!(a.value_of(&rows[0]).0, "—");
        a.latest.insert((0x7E0, 0x202A), (1.0, vec![0x03, 0xE8]));
        a.latest.insert((0x7E0, 0x2029), (1.0, vec![0x03, 0xF2]));
        let rows = a.rows();
        assert_eq!(a.value_of(&rows[0]).0, "1 / 1.01 bar");
    }

    #[test]
    fn half_a_pair_still_draws_a_line() {
        // Selecting only the actual value must not hide it waiting for a
        // partner that was never asked for.
        let mut a = App::new(reference_channels());
        for c in a.channels.iter_mut() {
            if c.request == 0x7E0 && c.did == 0x202A {
                c.selected = true;
            }
        }
        let rows = a.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Boost pressure");
        assert!(rows[0].specified.is_none());
        a.latest.insert((0x7E0, 0x202A), (1.0, vec![0x03, 0xE8]));
        let rows = a.rows();
        assert_eq!(a.value_of(&rows[0]).0, "1 bar");
    }

    #[test]
    fn rows_are_ordered_the_same_way_the_plan_polls_them() {
        // A table whose rows move between cycles cannot be read.
        let mut a = app();
        a.channels.iter_mut().for_each(|c| c.selected = true);
        let order: Vec<(u16, u16)> = a.shown().iter().map(|c| (c.request, c.did)).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted);
    }
}
