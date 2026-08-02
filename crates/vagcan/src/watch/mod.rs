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
pub mod replay;

use std::io;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::Rect;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

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
    /// Substring the selection screen is narrowed to. With a survey loaded
    /// there are over a thousand candidates, and stepping through them one
    /// arrow at a time is not a way to find anything.
    filter: String,
    /// True while the filter is being typed, so letters go into it instead of
    /// triggering `a`/`n`/`q`.
    typing_filter: bool,
    /// Scroll position of the selection list. Without one, everything past the
    /// bottom of the terminal is unreachable.
    select_state: TableState,
    /// Which control unit's tab is open. A car has fifteen units and over a
    /// thousand identifiers between them; one list of all of it is a list
    /// nobody reads.
    tab: usize,
    /// Where the unit list was drawn, so a click can find a unit.
    unit_area: Option<Rect>,
    /// Where the selection list was drawn, for the same reason.
    list_area: Option<Rect>,
    /// What each unit called itself (`F197`), for the tab labels. A unit that
    /// did not say goes by its number alone rather than an invented name.
    pub units: Vec<(u16, String)>,
    /// Completed poll cycles, and when the run started.
    cycles: u64,
    started: Instant,
    /// The clock a reading's age is measured against. Live, it is time since
    /// the run started; on a replay it is the playhead, because a value
    /// recorded ten minutes into a drive is not ten minutes old.
    clock: f64,
    /// False on a replay, where a poll rate would be the redraw rate and mean
    /// nothing about a car.
    live: bool,
    /// The unit a request is out to, while it is out. Shown in the footer:
    /// a batch can take as long as that unit's deadline, and a still screen
    /// during it reads as a hang.
    waiting: Option<u16>,
    /// Units whose last request took longer than the threshold.
    ///
    /// The screen is drawn before the request is sent — it has to be, since
    /// the await blocks — so whether *this* request will be slow is not yet
    /// knowable. What is knowable is whether the last one to this unit was,
    /// and a unit that timed out once will time out again. So the first slow
    /// answer passes unannounced and the rest are called: over-reporting a
    /// prompt unit would put a spinner on screen at every redraw.
    slow: std::collections::BTreeSet<u16>,
    status: String,
}

impl App {
    pub fn new(channels: Vec<Channel>) -> Self {
        App {
            channels,
            latest: std::collections::BTreeMap::new(),
            screen: Screen::Live,
            cursor: 0,
            filter: String::new(),
            typing_filter: false,
            select_state: TableState::default(),
            tab: 0,
            unit_area: None,
            list_area: None,
            units: Vec::new(),
            cycles: 0,
            started: Instant::now(),
            clock: 0.0,
            live: true,
            waiting: None,
            slow: std::collections::BTreeSet::new(),
            status: String::new(),
        }
    }

    /// The control units on offer, in the order their tabs appear.
    ///
    /// One tab per unit, and a last tab for everything at once — the pile is
    /// still reachable for anyone who wants it, it is just not what opens.
    fn tabs(&self) -> Vec<u16> {
        let mut units: Vec<u16> = self.channels.iter().map(|c| c.request).collect();
        units.sort_unstable();
        units.dedup();
        units
    }

    /// Open the first unit that has anything on screen.
    ///
    /// Tabs are in id order, and the lowest id is not usually the interesting
    /// one — opening on a unit with nothing selected makes the tool look empty
    /// at the moment a person first sees it.
    fn open_first_populated(&mut self) {
        if let Some(index) =
            self.tabs().iter().position(|r| self.channels.iter().any(|c| c.request == *r && c.selected))
        {
            self.tab = index;
            if let Some(first) = self.visible().first() {
                self.cursor = *first;
            }
        }
    }

    /// The unit the open tab shows, or `None` on the "everything" tab.
    fn open_unit(&self) -> Option<u16> {
        self.tabs().get(self.tab).copied()
    }

    /// How a tab is labelled: the unit's short number when this project has
    /// established one, otherwise its request id, and the component string the
    /// unit gave for itself when there is one.
    fn unit_heading(&self, request: u16) -> String {
        self.tab_label(request)
    }

    fn tab_label(&self, request: u16) -> String {
        let address = vag_protocol::address::UnitAddress::from_request(request)
            .map(|a| a.label())
            .unwrap_or_else(|| format!("{request:03X}"));
        match self.units.iter().find(|(id, _)| *id == request) {
            Some((_, name)) => format!("{address} {name}"),
            None => address,
        }
    }

    /// Which channels the selection screen is showing, as indices into
    /// `channels`.
    ///
    /// A filter matches the measurement name, the identifier or the unit, so
    /// `boost`, `202A` and `713` all narrow to something useful.
    fn visible(&self) -> Vec<usize> {
        let unit = self.open_unit();
        let needle = self.filter.to_lowercase();
        self.channels
            .iter()
            .enumerate()
            .filter(|(_, c)| unit.is_none_or(|u| c.request == u))
            .filter(|(_, c)| {
                needle.is_empty()
                    || c.label().to_lowercase().contains(&needle)
                    || format!("{:04x}", c.did).contains(&needle)
                    || c.unit().to_lowercase().contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Rows currently on screen, in the order the plan polls them.
    /// Every selected channel, from every unit.
    ///
    /// The live screen is not filtered by the open unit: the point of choosing
    /// measurements from several control units is to watch them together. The
    /// unit list belongs to the configure screen, where the choosing happens.
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
        format!("{:.1}s", (self.clock - at).max(0.0))
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
/// What each unit called itself, for the tab labels.
///
/// The component string when the unit gave one, else its part number — both
/// come from the unit, so a tab never carries a name this project made up.
fn unit_names(identities: &[plan::UnitIdentity]) -> Vec<(u16, String)> {
    identities
        .iter()
        .filter_map(|i| {
            let name = i.component.clone().or_else(|| i.part_number.clone())?;
            Some((i.request, name))
        })
        .collect()
}

/// Draw the list of control units down the left of the configure screen, and
/// return the area left for that unit's measurements.
///
/// A car has fifteen control units and over a thousand identifiers between
/// them. The list is what makes choosing navigable: which units this car has,
/// how much of each is already on screen, and which one's measurements are
/// listed beside it. Tab moves between them and so does a click — the list
/// looks selectable, so it is.
fn draw_units(frame: &mut Frame, app: &mut App, area: Rect) -> Rect {
    let units = app.tabs();
    // Wide enough for the longest label, within reason: a component string
    // can be long and the table is what the screen is for.
    // Room for the longest label, its `selected/available` count, the column
    // gap and both borders — within reason, because the table is what the
    // screen is for.
    let width = units
        .iter()
        .map(|r| app.tab_label(*r).chars().count() + 11)
        .max()
        .unwrap_or(16)
        .clamp(16, 36) as u16;
    // The list takes what its content needs; the measurements take the rest.
    let split =
        Layout::horizontal([Constraint::Length(width), Constraint::Min(20)]).split(area);

    let rows: Vec<Row> = units
        .iter()
        .map(|request| {
            let selected =
                app.channels.iter().filter(|c| c.request == *request && c.selected).count();
            let available = app.channels.iter().filter(|c| c.request == *request).count();
            Row::new(vec![
                Cell::from(app.tab_label(*request)),
                Cell::from(format!("{selected}/{available}")),
            ])
        })
        .collect();

    let mut state = TableState::default();
    state.select(Some(app.tab.min(units.len().saturating_sub(1))));
    let list = Table::new(rows, [Constraint::Min(6), Constraint::Length(7)])
        .row_highlight_style(Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD))
        .block(Block::default().borders(Borders::ALL).title(" units [tab] "));
    // Remembered so a click can be turned back into a unit.
    app.unit_area = Some(split[0]);
    frame.render_stateful_widget(list, split[0], &mut state);
    split[1]
}

fn draw_live(frame: &mut Frame, app: &mut App) {
    let layout = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(frame.area());
    let table_area = layout[0];
    let shown = app.rows();
    // Grouped by control unit, with a line naming each: values from several
    // units on one screen are unreadable without saying which is which.
    let mut rows: Vec<Row> = Vec::new();
    let mut unit_of_row: Option<u16> = None;
    for r in &shown {
        let c = r.any();
        if unit_of_row != Some(c.request) {
            unit_of_row = Some(c.request);
            // The heading goes in the measurement column, which is the wide
            // one: a table cannot span columns, and in the four-wide first
            // column the unit's name would read `── 0`.
            rows.push(
                Row::new(vec![
                    Cell::from("──"),
                    Cell::from("────"),
                    Cell::from(format!("{} ", app.unit_heading(c.request))),
                    Cell::from("──────────"),
                ])
                .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            );
        }
        let (value, age) = app.value_of(r);
        // A pair is addressed by two identifiers; showing both keeps the
        // line honest about where the numbers came from.
        let dids = match (r.actual, r.specified) {
            (Some(a), Some(s)) => format!("{:04X}/{:04X}", a.did, s.did),
            _ => format!("{:04X}", c.did),
        };
        rows.push(Row::new(vec![
            Cell::from(c.unit()),
            Cell::from(dids),
            Cell::from(r.label.clone()),
            Cell::from(value).style(Style::default().add_modifier(Modifier::BOLD)),
            Cell::from(c.unit_of_measure().to_string()),
            Cell::from(age).style(Style::default().fg(Color::DarkGray)),
        ]));
    }

    let heading_w = shown
        .iter()
        .map(|r| app.unit_heading(r.any().request).chars().count() + 1)
        .max()
        .unwrap_or(0);
    let name_w =
        shown.iter().map(|r| r.label.len()).chain([heading_w]).max().unwrap_or(4).max(11) as u16;
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
    frame.render_widget(table, table_area);

    let rate = match app.live {
        true => format!("{:.1} Hz · ", app.poll_rate()),
        false => String::new(),
    };
    let waiting = match app.waiting {
        Some(request) => {
            format!("  {} reading {}…", crate::progress::frame(app.cycles), app.unit_heading(request))
        }
        None => String::new(),
    };
    let help = format!(
        " {rate}{} of {} shown · [tab] unit  [c] configure  [q] quit{}{waiting}",
        shown.len(),
        app.channels.iter().filter(|c| c.selected).count(),
        app.status
    );
    frame.render_widget(
        Paragraph::new(help).block(Block::default().borders(Borders::ALL)),
        layout[1],
    );
}

/// Draw the selection screen.
fn draw_select(frame: &mut Frame, app: &mut App) {
    let layout = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).split(frame.area());
    let table_area = draw_units(frame, app, layout[0]);
    let visible = app.visible();
    let rows: Vec<Row> = visible
        .iter()
        .map(|i| {
            let c = &app.channels[*i];
            let mark = if c.selected { "[x]" } else { "[ ]" };
            Row::new(vec![
                Cell::from(mark),
                Cell::from(c.unit()),
                Cell::from(format!("{:04X}", c.did)),
                Cell::from(c.label()),
                Cell::from(c.unit_of_measure().to_string()),
            ])
        })
        .collect();

    let name_w = app.channels.iter().map(|c| c.label().len()).max().unwrap_or(4) as u16;
    let title = match app.filter.is_empty() {
        true => format!(" choose what to show — {} available ", app.channels.len()),
        false => format!(
            " choose what to show — {} of {} match {:?} ",
            visible.len(),
            app.channels.len(),
            app.filter
        ),
    };
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(4),
            Constraint::Length(5),
            Constraint::Length(name_w),
            Constraint::Length(9),
        ],
    )
    .row_highlight_style(Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD))
    .block(Block::default().borders(Borders::ALL).title(title));

    // The cursor is an index into `channels`; the table shows the filtered
    // subset, so it is translated — and the state is what makes the list
    // scroll instead of clipping at the bottom of the terminal.
    let row = visible.iter().position(|i| *i == app.cursor);
    app.select_state.select(row);
    // Remember where the list landed, so a click can be turned back into a row.
    app.list_area = Some(table_area);
    frame.render_stateful_widget(table, table_area, &mut app.select_state);

    let help = if app.typing_filter {
        format!(" filter: {}▏ [enter] apply  [esc] clear ", app.filter)
    } else {
        " [space]/click toggle  [↑↓ pgup/pgdn] move  [tab] unit  [/] filter  [a] all  [n] none  \
         [enter] back "
            .to_string()
    };
    frame.render_widget(
        Paragraph::new(help).block(Block::default().borders(Borders::ALL)),
        layout[1],
    );
}

/// Move to the next or previous unit tab, wrapping.
fn step_tab(app: &mut App, forward: bool) {
    let count = app.tabs().len();
    if count == 0 {
        return;
    }
    app.tab = match forward {
        true => (app.tab + 1) % count,
        false => (app.tab + count - 1) % count,
    };
    // The cursor belongs to the tab it is in; leaving it behind makes the
    // arrow keys jump to a row nobody can see.
    if let Some(first) = app.visible().first() {
        app.cursor = *first;
    }
}

/// Handle a mouse event.
///
/// Clicking is the obvious thing to try on a list of checkboxes, and the tool
/// draws them as `[x]`. Supporting it costs one branch; not supporting it
/// leaves a screen that looks clickable and is not. Nothing here can quit —
/// there is no click that means "stop", and inventing one would make a stray
/// click end a recording.
fn on_mouse(app: &mut App, event: crossterm::event::MouseEvent) {
    use crossterm::event::MouseEventKind;
    let (x, y) = (event.column, event.row);
    match event.kind {
        MouseEventKind::Down(_) => {
            // The unit list runs down the right of either screen.
            if let Some(area) = app.unit_area {
                if x >= area.x && x < area.x + area.width && y > area.y {
                    let index = (y - area.y - 1) as usize;
                    if index < app.tabs().len() {
                        app.tab = index;
                        if let Some(first) = app.visible().first() {
                            app.cursor = *first;
                        }
                    }
                    return;
                }
            }
            if app.screen != Screen::Select {
                return;
            }
            // Inside the list, a row is one line below the border, offset by
            // however far the list has been scrolled.
            let Some(area) = app.list_area else { return };
            if y <= area.y || y + 1 >= area.y + area.height {
                return;
            }
            let visible = app.visible();
            let index = (y - area.y - 1) as usize + app.select_state.offset();
            if let Some(channel) = visible.get(index) {
                app.cursor = *channel;
                app.channels[*channel].selected = !app.channels[*channel].selected;
            }
        }
        MouseEventKind::ScrollDown => {
            on_key(app, KeyCode::Down);
        }
        MouseEventKind::ScrollUp => {
            on_key(app, KeyCode::Up);
        }
        _ => {}
    }
}

/// Handle one key. Returns false when the user asked to quit.
fn on_key(app: &mut App, code: KeyCode) -> bool {
    // The tab bar is on both screens, so its keys are handled before either.
    match code {
        KeyCode::Tab => {
            step_tab(app, true);
            return true;
        }
        KeyCode::BackTab => {
            step_tab(app, false);
            return true;
        }
        _ => {}
    }
    match app.screen {
        Screen::Live => match code {
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Char('c') => app.screen = Screen::Select,
            _ => {}
        },
        // While a filter is being typed the letters belong to it, or `n` would
        // clear the selection halfway through typing "engine".
        Screen::Select if app.typing_filter => match code {
            KeyCode::Enter => app.typing_filter = false,
            KeyCode::Esc => {
                app.filter.clear();
                app.typing_filter = false;
            }
            KeyCode::Backspace => {
                app.filter.pop();
            }
            KeyCode::Char(c) => app.filter.push(c),
            _ => {}
        },
        Screen::Select => {
            let visible = app.visible();
            let at = visible.iter().position(|i| *i == app.cursor).unwrap_or(0);
            let step = |app: &mut App, to: usize| {
                if let Some(i) = visible.get(to.min(visible.len().saturating_sub(1))) {
                    app.cursor = *i;
                }
            };
            match code {
                KeyCode::Char('q') => return false,
                KeyCode::Enter | KeyCode::Esc | KeyCode::Char('c') => app.screen = Screen::Live,
                KeyCode::Char('/') => app.typing_filter = true,
                KeyCode::Up => step(app, at.saturating_sub(1)),
                KeyCode::Down => step(app, at + 1),
                KeyCode::PageUp => step(app, at.saturating_sub(10)),
                KeyCode::PageDown => step(app, at + 10),
                KeyCode::Home => step(app, 0),
                KeyCode::End => step(app, visible.len().saturating_sub(1)),
                KeyCode::Char(' ') => {
                    if let Some(c) = app.channels.get_mut(app.cursor) {
                        c.selected = !c.selected;
                    }
                }
                // `all` and `none` act on what is on screen: with a filter up,
                // selecting "all" of a thousand channels would be a mistake
                // nobody meant to make.
                KeyCode::Char('a') => {
                    for i in &visible {
                        app.channels[*i].selected = true;
                    }
                }
                KeyCode::Char('n') => {
                    for i in &visible {
                        app.channels[*i].selected = false;
                    }
                }
                _ => {}
            }
        }
    }
    true
}

/// Play a recorded drive back through the same screen, with no car.
///
/// A separate loop from the live one, deliberately: see `replay`'s module
/// docs. Nothing here opens a port or addresses a control unit.
pub async fn run_recording(
    recording_path: &str,
    catalogs: &str,
    survey: Option<&str>,
    speed: f64,
) -> Result<()> {
    let csv = std::fs::read_to_string(recording_path)
        .with_context(|| format!("reading the recording {recording_path:?}"))?;
    let recording = replay::Recording::parse(&csv)
        .map_err(|e| anyhow::anyhow!("{recording_path}: {e}"))?;

    let store = vag_data::catalog::CatalogStore::open(catalogs);
    // A recording carries no identification block, so the catalogs are offered
    // for every unit this project has one for. On a replay that is honest:
    // nothing is being addressed, and a column only appears if it matched.
    let mut identities: Vec<plan::UnitIdentity> = Vec::new();
    if let Some(path) = survey {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the survey {path:?}"))?;
        identities = plan::identities_from_survey(&text);
    }
    // A recording does not record which unit each column came from. With a
    // survey the real units are known and the tabs are real; without one every
    // catalog is offered under a single tab named after the file, because
    // splitting them into units this build merely happens to have catalogs for
    // would put addresses on screen that the recording never claimed.
    let named_by_survey = !identities.is_empty();
    if !named_by_survey {
        for entry in std::fs::read_dir(store.dir()).into_iter().flatten().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(part) = name.strip_suffix(".json") {
                identities.push(plan::UnitIdentity {
                    request: plan::ENGINE,
                    part_number: Some(part.to_string()),
                    odx_name: None,
                    component: None,
                });
            }
        }
    }

    let mut channels = plan::available(&store, &identities);
    // A recording does not say which unit each column came from. Columns that
    // match a known measurement keep its unit; the rest are attributed to the
    // engine's id, which is a label on a screen and addresses nothing — no
    // request is ever sent in this mode.
    let resolved = replay::resolve(&recording.columns, &mut channels, plan::ENGINE);
    if resolved.is_empty() {
        anyhow::bail!(
            "none of the {} columns in {recording_path} matched a channel this build knows. \n\
             A recording is matched by measurement name or identifier; check that the \n\
             catalogs in {catalogs} are the ones it was recorded with.",
            recording.columns.len()
        );
    }
    // Start with the channels that actually moved. A column that read the same
    // bytes for the whole drive proves nothing — the rule this project applies
    // to its own measurements — and on the reference recording 75 of 104
    // columns are like that. The rest are one keypress away.
    let moved = replay::columns_that_moved(&recording);
    for column in &moved {
        if let Some(hit) = resolved.get(column) {
            channels[hit.channel].selected = true;
        }
    }
    if moved.is_empty() {
        for hit in resolved.values() {
            channels[hit.channel].selected = true;
        }
    }

    enable_raw_mode().map_err(|e| {
        anyhow::anyhow!("`watch` needs an interactive terminal (it draws a full-screen view): {e}")
    })?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen, crossterm::event::EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new(channels);
    app.open_first_populated();
    app.units = match named_by_survey {
        true => unit_names(&identities),
        false => {
            let file = std::path::Path::new(recording_path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| recording_path.to_string());
            vec![(plan::ENGINE, file)]
        }
    };
    app.live = false;
    let duration = recording.duration();
    let mut playhead = 0.0f64;
    let mut paused = false;
    let mut speed = speed.clamp(0.05, 50.0);
    let mut last = Instant::now();

    let result = loop {
        // Advance the playhead by real time, and wrap: a demo that stops after
        // one pass is a demo somebody has to keep restarting.
        let elapsed = last.elapsed().as_secs_f64();
        last = Instant::now();
        if !paused && duration > 0.0 {
            playhead = (playhead + elapsed * speed) % duration;
        }
        if let Some((_, cells)) = recording.at(playhead) {
            for (column, hit) in &resolved {
                let Some(cell) = cells.get(*column).and_then(|c| c.as_ref()) else { continue };
                let channel = &app.channels[hit.channel];
                let (request, did) = (channel.request, channel.did);
                if let Some(bytes) = replay::cell_to_bytes(cell, channel, hit.raw) {
                    app.latest.insert((request, did), (playhead, bytes));
                }
            }
        }
        app.clock = playhead;
        app.status = format!(
            " · [space] pause  [←→] seek  [+-] speed · {:.0}/{:.0}s ×{speed:.2}{}",
            playhead,
            duration,
            if paused { " PAUSED" } else { "" }
        );
        app.cycles += 1;

        terminal.draw(|f| match app.screen {
            Screen::Live => draw_live(f, &mut app),
            Screen::Select => draw_select(f, &mut app),
        })?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Mouse(m) => {
                    on_mouse(&mut app, m);
                    continue;
                }
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    // Playback keys only mean something on the live screen;
                    // on the selection screen they are ordinary input.
                    if app.screen == Screen::Live {
                        match k.code {
                            KeyCode::Char(' ') => {
                                paused = !paused;
                                continue;
                            }
                            KeyCode::Char('+') | KeyCode::Char('=') => {
                                speed = (speed * 2.0).min(50.0);
                                continue;
                            }
                            KeyCode::Char('-') => {
                                speed = (speed / 2.0).max(0.05);
                                continue;
                            }
                            KeyCode::Left => {
                                playhead = (playhead - 10.0).max(0.0);
                                continue;
                            }
                            KeyCode::Right => {
                                playhead = (playhead + 10.0).min(duration);
                                continue;
                            }
                            _ => {}
                        }
                    }
                    if !on_key(&mut app, k.code) {
                        break Ok(());
                    }
                }
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    println!(
        "replayed {recording_path} — {:.0}s of driving, {} columns, {} of which ever changed",
        duration,
        recording.columns.len(),
        moved.len()
    );
    result
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

    // Argument checking first: the adapter is a single-user resource, and
    // holding it open while failing on a typo blocks the next attempt. That
    // means the recording is created here too, not once the car is answering —
    // an unwritable --out path is the same typo as an unreadable --survey one.
    let store = vag_data::catalog::CatalogStore::open(catalogs);
    let survey_text = match survey {
        Some(path) => Some(
            std::fs::read_to_string(path)
                .with_context(|| format!("reading the survey {path:?}"))?,
        ),
        None => None,
    };
    let mut sink = match out {
        Some(path) => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("creating {path:?}"))?;
            Some(std::io::BufWriter::new(file))
        }
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
            .with_context(|| crate::device::open_failure(device_path))?;

    // Which units the car has. Without this the view would only ever show the
    // engine, because a unit with no identity contributes no channels and so
    // no tab — which is what "switching between units does nothing" looked
    // like. One read of the gateway's installation list answers it, the same
    // read `vagcan units` makes; a car whose gateway does not answer falls
    // back to whatever was asked for.
    let mut wanted: Vec<u16> = preselect.iter().map(|(request, _)| *request).collect();
    wanted.push(plan::ENGINE);
    let mut progress = crate::progress::Line::new();
    if survey_text.is_none() {
        progress.update("asking the gateway which control units this car has");
        let gateway = vag_protocol::address::UnitAddress::from_request(0x710)
            .expect("the gateway is in VW's block");
        let mut uds = AsyncUdsClient::new(IsoTpCan::new(
            adapter,
            CanId::Standard(gateway.request),
            CanId::Standard(gateway.response),
        ));
        if let Ok(bitmap) =
            uds.read_data_by_identifier(vag_protocol::gateway::INSTALLATION_LIST).await
        {
            wanted.extend(vag_protocol::gateway::decode_installation_list(&bitmap));
        }
        // The powertrain is never in that list — it lives on the other id
        // block — so it is added rather than discovered.
        wanted.push(0x7E1);
        adapter = uds.into_transport().into_backend();
    }
    wanted.sort_unstable();
    wanted.dedup();
    let total = wanted.len();
    for (at, request) in wanted.into_iter().enumerate() {
        progress.update(&format!("identifying control units — {request:03X}, {} of {total}", at + 1));
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
        // One short probe decides whether the unit is there. A unit that is
        // not costs this deadline once, instead of the full two-second one
        // three times over — fifteen listed addresses at that price is what
        // made startup take several seconds.
        const PROBE: Duration = Duration::from_millis(300);
        let part = text(uds.read_data_by_identifier_within(0xF187, PROBE).await.ok());
        if part.is_none() && request != plan::ENGINE {
            adapter = uds.into_transport().into_backend();
            continue;
        }
        // Identification only — no session change and no sweep. `SAFETY.md`
        // is about what a sweep can provoke; this is not one.
        let component = text(uds.read_data_by_identifier_within(0xF197, PROBE).await.ok());
        let odx = text(uds.read_data_by_identifier_within(0xF19E, PROBE).await.ok());
        identities.push(plan::UnitIdentity {
            request,
            part_number: part,
            odx_name: odx,
            component,
        });
        adapter = uds.into_transport().into_backend();
    }

    progress.finish();
    // Say what the car answered and what could be shown, before the screen
    // takes over. A unit that identified itself but has no catalog contributes
    // no measurements and so no tab — which looks like the tool failing to
    // find it, and is worth distinguishing from that.
    let mut channels = plan::available(&store, &identities);
    {
        let with_rows: Vec<String> = identities
            .iter()
            .filter(|i| channels.iter().any(|c| c.request == i.request))
            .map(|i| format!("{:03X}", i.request))
            .collect();
        let without: Vec<String> = identities
            .iter()
            .filter(|i| !channels.iter().any(|c| c.request == i.request))
            .map(|i| format!("{:03X}", i.request))
            .collect();
        println!("{} control units answered: {}", identities.len(), with_rows.join(" "));
        if !without.is_empty() {
            println!(
                "no measurements known for {} — they answer, but {catalogs} holds no \n\
                 catalog for their part numbers. `vagcan survey --out FILE` then \n\
                 `watch --survey FILE` offers their identifiers as raw bytes.",
                without.join(" ")
            );
        }
    }
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
        plan::select_basics(&mut channels);
    }
    let mut backend = Some(adapter);
    let mut header_written = false;

    // A full-screen view needs a terminal; without one crossterm fails with a
    // bare errno that says nothing about why.
    enable_raw_mode().map_err(|e| {
        anyhow::anyhow!("`watch` needs an interactive terminal (it draws a full-screen view): {e}")
    })?;
    let mut stdout = io::stdout();
    crossterm::execute!(stdout, EnterAlternateScreen, crossterm::event::EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let mut app = App::new(channels);
    app.open_first_populated();
    app.units = unit_names(&identities);
    let period = Duration::from_secs_f64(1.0 / hz.max(0.1));
    let result = loop {
        terminal.draw(|f| match app.screen {
            Screen::Live => draw_live(f, &mut app),
            Screen::Select => draw_select(f, &mut app),
        })?;

        // Drain the keyboard without blocking the poll loop. `q` here has to
        // leave the loop entirely, not just this drain — otherwise the key is
        // swallowed and a whole poll cycle runs before the quit takes effect.
        let mut quit = false;
        while event::poll(Duration::from_millis(0))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if !on_key(&mut app, k.code) {
                        quit = true;
                        break;
                    }
                }
                Event::Mouse(m) => on_mouse(&mut app, m),
                _ => {}
            }
        }
        if quit {
            break Ok(());
        }

        let cycle = Instant::now();
        let mut quit_mid_cycle = false;
        for batch in plan::plan(&app.channels) {
            // Between batches, not only between cycles: a cycle that spans
            // several units takes as long as their timeouts add up to, and a
            // keypress should not wait for that.
            while event::poll(Duration::from_millis(0))? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        if !on_key(&mut app, k.code) {
                            quit_mid_cycle = true;
                        }
                    }
                    Event::Mouse(m) => on_mouse(&mut app, m),
                    _ => {}
                }
            }
            if quit_mid_cycle {
                break;
            }
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
            // Redraw before the request, so the footer says which unit is
            // being waited on. A batch can take as long as that unit's
            // deadline, and a still screen during it reads as a hang.
            app.waiting = app.slow.contains(&batch.request).then_some(batch.request);
            terminal.draw(|f| match app.screen {
                Screen::Live => draw_live(f, &mut app),
                Screen::Select => draw_select(f, &mut app),
            })?;
            let asked = Instant::now();
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
            app.clock = at;
            if let Ok(records) = answer {
                for (did, data) in records {
                    app.latest.insert((batch.request, did), (at, data));
                }
            }
            backend = Some(uds.into_transport().into_backend());
            app.waiting = None;
            // Remember for next time round, so the footer can warn before the
            // wait rather than after it.
            match asked.elapsed() >= crate::progress::THRESHOLD {
                true => app.slow.insert(batch.request),
                false => app.slow.remove(&batch.request),
            };
        }
        if quit_mid_cycle {
            break Ok(());
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
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    if !on_key(&mut app, k.code) {
                        quit = true;
                        break;
                    }
                }
                Event::Mouse(m) => on_mouse(&mut app, m),
                _ => {}
            }
        }
        if quit {
            break Ok(());
        }
    };

    disable_raw_mode()?;
    crossterm::execute!(
        terminal.backend_mut(),
        crossterm::event::DisableMouseCapture,
        LeaveAlternateScreen
    )?;
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
            component: None,
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

    /// Open the tab that holds `request`, so a test about rows is not really
    /// a test about which tab happens to be first.
    fn open(app: &mut App, request: u16) {
        app.tab = app.tabs().iter().position(|r| *r == request).expect("the unit has a tab");
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
        open(&mut a, 0x7E0);
        a.cursor = a.visible()[5];
        let at = a.cursor;
        on_key(&mut a, KeyCode::Char(' '));
        assert!(a.channels[at].selected);
        // Selecting more can only add work, never remove it.
        assert!(plan::plan(&a.channels).len() >= before);

        // `a` and `n` act on the open tab, so clearing every tab clears the
        // plan — and polling never follows the tab, only the selection.
        for _ in 0..a.tabs().len() {
            on_key(&mut a, KeyCode::Char('n'));
            step_tab(&mut a, true);
        }
        assert!(plan::plan(&a.channels).is_empty(), "none selected polls nothing");
        on_key(&mut a, KeyCode::Char('a'));
        assert!(!plan::plan(&a.channels).is_empty());
    }

    #[test]
    fn the_cursor_stays_inside_the_list() {
        let mut a = app();
        a.screen = Screen::Select;
        open(&mut a, 0x7E0);
        let visible = a.visible();
        let (first, last) = (visible[0], *visible.last().unwrap());

        a.cursor = first;
        on_key(&mut a, KeyCode::Up);
        assert_eq!(a.cursor, first, "cannot run off the top of the tab");
        a.cursor = last;
        on_key(&mut a, KeyCode::Down);
        assert_eq!(a.cursor, last, "cannot run off the bottom of the tab");
        // A page jump past the end lands on the last row rather than nowhere.
        on_key(&mut a, KeyCode::PageDown);
        assert_eq!(a.cursor, last);
        on_key(&mut a, KeyCode::Home);
        assert_eq!(a.cursor, first);
        on_key(&mut a, KeyCode::End);
        assert_eq!(a.cursor, last);
    }

    #[test]
    fn a_filter_narrows_the_list_and_the_cursor_follows_it() {
        // With a survey loaded there are over a thousand candidates; stepping
        // to one by arrow key is not a way to find anything.
        let mut a = app();
        a.screen = Screen::Select;
        open(&mut a, 0x7E0);
        let in_tab = a.visible().len();
        assert!(in_tab > 1, "the engine tab has rows to filter");

        on_key(&mut a, KeyCode::Char('/'));
        for c in "boost".chars() {
            on_key(&mut a, KeyCode::Char(c));
        }
        on_key(&mut a, KeyCode::Enter);

        let visible = a.visible();
        assert!(!visible.is_empty() && visible.len() < in_tab, "{}", visible.len());
        assert!(visible.iter().all(|i| a.channels[*i].label().to_lowercase().contains("boost")));

        // Moving now walks the filtered rows, not the hidden ones.
        on_key(&mut a, KeyCode::Home);
        assert_eq!(a.cursor, visible[0]);
        on_key(&mut a, KeyCode::End);
        assert_eq!(a.cursor, *visible.last().unwrap());
    }

    #[test]
    fn select_all_applies_to_what_is_on_screen_not_to_everything() {
        // "All" with a filter up must not silently select a thousand channels
        // the user cannot see.
        let mut a = app();
        a.screen = Screen::Select;
        open(&mut a, 0x7E0);
        on_key(&mut a, KeyCode::Char('/'));
        for c in "boost".chars() {
            on_key(&mut a, KeyCode::Char(c));
        }
        on_key(&mut a, KeyCode::Enter);
        on_key(&mut a, KeyCode::Char('a'));

        let visible = a.visible();
        assert!(visible.iter().all(|i| a.channels[*i].selected));
        let hidden_selected =
            a.channels.iter().enumerate().filter(|(i, c)| c.selected && !visible.contains(i)).count();
        assert_eq!(hidden_selected, 2, "only the two the fixture pre-selected");
    }

    #[test]
    fn typing_a_filter_does_not_trigger_the_command_keys() {
        // `n` clears the selection; typing "engine" must not.
        let mut a = app();
        a.screen = Screen::Select;
        a.channels[3].selected = true;
        on_key(&mut a, KeyCode::Char('/'));
        for c in "engine".chars() {
            on_key(&mut a, KeyCode::Char(c));
        }
        assert!(a.channels[3].selected, "the selection survived typing");
        assert_eq!(a.filter, "engine");
        // Escape abandons the filter rather than the screen.
        on_key(&mut a, KeyCode::Esc);
        assert_eq!(a.filter, "");
        assert_eq!(a.screen, Screen::Select);
    }

    #[test]
    fn a_specified_value_shares_a_line_with_its_actual() {
        // Boost is published twice: 2029 is what the engine asked for, 202A is
        // what it got. Side by side the gap is readable at a glance.
        let mut a = App::new(reference_channels());
        open(&mut a, 0x7E0);
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
        open(&mut a, 0x7E0);
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
    fn the_unit_list_narrows_the_choosing_but_not_the_watching() {
        // The point of choosing measurements from several units is to watch
        // them together, so the live table is never filtered by the open unit.
        // The configure screen's list is.
        let mut a = App::new(reference_channels());
        a.channels.iter_mut().for_each(|c| c.selected = true);
        let units = a.tabs();
        assert!(units.len() > 1, "the reference car has more than one unit: {units:02X?}");

        let on_screen: Vec<u16> = a.shown().iter().map(|c| c.request).collect();
        assert!(on_screen.iter().any(|r| *r == units[0]));
        assert!(on_screen.iter().any(|r| *r == units[1]), "every unit at once");

        // Choosing is per unit, and it moves with the tab.
        let first: Vec<u16> = a.visible().iter().map(|i| a.channels[*i].request).collect();
        assert!(first.iter().all(|r| *r == units[0]));
        step_tab(&mut a, true);
        let second: Vec<u16> = a.visible().iter().map(|i| a.channels[*i].request).collect();
        assert!(second.iter().all(|r| *r == units[1]));

        // And it wraps rather than running off the end.
        for _ in 0..units.len() {
            step_tab(&mut a, true);
        }
        assert_eq!(a.tab, 1);
    }

    #[test]
    fn a_unit_appears_as_one_group_not_scattered_down_the_table() {
        // Separators are written where the unit changes, so a unit that
        // reappeared later would get a second heading and the table would read
        // as if there were two of it.
        let mut a = App::new(reference_channels());
        a.channels.iter_mut().for_each(|c| c.selected = true);
        let order: Vec<u16> = a.rows().iter().map(|r| r.any().request).collect();
        let mut seen: Vec<u16> = Vec::new();
        for request in order {
            if seen.last() != Some(&request) {
                assert!(!seen.contains(&request), "{request:03X} appears twice");
                seen.push(request);
            }
        }
        assert!(seen.len() > 1, "the fixture really does span units: {seen:02X?}");
    }

    #[test]
    fn only_a_unit_that_has_been_slow_is_announced() {
        // The screen is drawn before the request, so whether this one will be
        // slow is not knowable; whether the last one was, is. A prompt unit
        // must not put a spinner on screen at every redraw.
        let mut a = App::new(reference_channels());
        assert!(a.slow.is_empty());
        assert_eq!(a.slow.contains(&0x7E0).then_some(0x7E0), None);
        a.slow.insert(0x7E0);
        assert_eq!(a.slow.contains(&0x7E0).then_some(0x7E0), Some(0x7E0));
    }

    #[test]
    fn the_live_table_says_which_unit_each_group_came_from() {
        // Values from several units on one screen are unreadable without it.
        let mut a = App::new(reference_channels());
        a.units = vec![(0x7E0, "1.8l R4 TFSI".to_string())];
        assert_eq!(a.unit_heading(0x7E0), "01 1.8l R4 TFSI");
        assert_eq!(a.unit_heading(0x714), "17", "a unit that said nothing is not named");
    }

    #[test]
    fn the_view_opens_on_a_unit_that_has_something_to_show() {
        // Tabs are in id order and the lowest id is rarely the interesting
        // one; opening there shows an empty table at the moment a person
        // first sees the tool.
        let mut a = App::new(reference_channels());
        for c in a.channels.iter_mut() {
            if c.request == 0x7E1 {
                c.selected = true;
            }
        }
        a.open_first_populated();
        assert_eq!(a.tabs()[a.tab], 0x7E1);
        assert!(!a.shown().is_empty());
    }

    #[test]
    fn a_tab_is_labelled_by_what_the_unit_said_about_itself() {
        let mut a = App::new(reference_channels());
        a.units = vec![(0x7E0, "1.8l R4 TFSI".to_string())];
        assert_eq!(a.tab_label(0x7E0), "01 1.8l R4 TFSI");
        // A unit that said nothing goes by its number, not by an invented name.
        assert_eq!(a.tab_label(0x714), "17");
        assert_eq!(a.tab_label(0x713), "713");
    }

    #[test]
    fn the_cursor_moves_with_the_tab_it_belongs_to() {
        // Leaving it behind makes the arrow keys walk a row nobody can see.
        let mut a = App::new(reference_channels());
        a.screen = Screen::Select;
        step_tab(&mut a, true);
        let visible = a.visible();
        assert!(visible.contains(&a.cursor), "{:?} not in the open tab", a.cursor);
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
