//! `vagcan watch` — a live view of the car, configured from inside.
//!
//! Values are picked on a selection screen rather than by flags, and several
//! control units appear together: each is addressed in turn over the one
//! serial link and they share a single table.
//!
//! **Nothing has to be run before this.** The channels come from what the car
//! says it is — each unit's `F187`/`F19E`/`F1A2` resolved against this
//! machine's project and proven catalogs — and the unit list comes off the
//! gateway, live, every run. An installed project declares thousands of
//! channels per car; asking somebody to go and rediscover them by sweeping the
//! vehicle was eight minutes spent learning what the project already said.
//!
//! A sweep is still the only thing that reaches a unit **no** source describes,
//! because there the identifiers have to be guessed at rather than looked up.
//! That case is offered — one question, on a terminal, with what it costs said
//! plainly — and never taken without an answer: asking a control unit
//! identifiers nothing declares it answers is a fuzz test of its diagnostic
//! server, which on a unit the car is relying on is not a small event
//!
//! Whatever a sweep found is kept per car — see [`crate::datadir::survey_cache`]
//! — and loaded with no flag at all, which puts every identifier it saw on
//! offer as raw bytes. `--survey FILE` still wins over the cache.
//!
//! The previous version drew with carriage returns, which only works on a
//! terminal that honours them — piped or resized, it left a trail of new lines
//! instead of updating one. A full-screen renderer has no such failure mode,
//! and it can also show a name in full instead of eliding it to fit a column.

pub mod favourites;
pub mod history;
pub mod replay;

use std::fmt::Write as _;
use std::io;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::Rect;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState};

use crate::plan::Channel;
use crate::ui::chart;
use crate::ui::term;

/// How many lines the chart draws, however many are marked for it.
///
/// The widget puts three lines and two scales on a page and says why, so six is
/// two full pages when the marked channels agree on their units and six pages
/// when none of them do. That is the number of times `←`/`→` can be pressed
/// before a reader is lost, and it is the reason there is a cap at all: with
/// thirty selected channels — an ordinary `watch` session — the pages run to
/// fifteen and paging stops being navigation.
///
/// The widget is not told about it. Which series, in what order, is the
/// caller's half of that seam, and a cap passed inwards would be the beginning
/// of the configuration language `docs/superpowers/specs/2026-08-05-architecture-design.md`
/// §5 refuses.
const CHART_CHANNELS: usize = 6;

/// Whether a channel can ever put a number on a chart.
///
/// Two ways it cannot, and they are different facts about the car. A channel
/// with no proven scaling is shown as raw bytes: there is no float in it, only
/// a byte string, and `watch`'s whole purpose is to show those so they can be
/// found. A state has a definition and still has no number — the gear codes are
/// neither contiguous nor ordered by ratio, and two of them are not gears at
/// all, so a line drawn through them would be a picture of the encoding rather
/// than of the car.
///
/// Either way the channel keeps its row in the table. It is the chart that
/// declines it, and it says so.
fn plottable(channel: &Channel) -> bool {
	match &channel.def {
		None => false,
		Some(def) => !matches!(def.scaling, vag_data_labels::catalog::Scaling::Enum { .. }),
	}
}

/// What the chart is drawing, and what it would not draw.
///
/// The two exclusions are counted rather than dropped quietly, because a driver
/// who marks a channel and sees nothing appear concludes the tool is broken.
/// They are said on `watch`'s own line under the chart and not through the
/// widget: an `excluded` parameter on `plot` would be a parameter that exists
/// for one caller, and this is a sentence about *this* screen's selection.
struct Charted {
	series: Vec<chart::Series>,
	/// Marked, and with nothing a chart can hold — see [`plottable`].
	no_number: usize,
	/// Marked, plottable, and past [`CHART_CHANNELS`].
	over_cap: usize,
}

/// Which screen has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
	Live,
	Select,
	/// Which of the watched channels the chart draws — a third screen rather
	/// than a corner of the second one.
	///
	/// The mark was only reachable from `Select`, which lists every channel the
	/// car has: to change one line of a chart somebody had to leave the chart,
	/// find the row among two thousand, and come back to see what they had
	/// done. Here the list is the handful already on the table and the chart
	/// stays on screen underneath it, so a press and its effect are visible at
	/// once — which is the whole of "нельзя выбрать что отображать на графике".
	Series,
	/// The settings, edited where they are used.
	///
	/// A rate and a `show_key` are found by trying them on a car — which is
	/// precisely the moment a command-line flag is not to hand and a text editor
	/// is two windows away. What is changed here is written to
	/// `~/.vagcan/config.toml` as it is changed, the same as a favourite.
	Settings,
}

/// One line of the settings screen.
///
/// An enum rather than an index into a list of closures: the screen has to draw
/// the current value beside each row, and a row that can be drawn but not
/// applied — or the reverse — is the way a settings screen silently stops
/// working.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Setting {
	/// How often the car is asked, in hertz.
	Rate,
	/// Whether each row ends with the channel's own key.
	ShowKey,
}

impl Setting {
	const ALL: [Setting; 2] = [Setting::Rate, Setting::ShowKey];

	fn label(self) -> &'static str {
		match self {
			Setting::Rate => "Poll rate",
			Setting::ShowKey => "Key at the end of each row",
		}
	}

	fn note(self) -> &'static str {
		match self {
			Setting::Rate => "how often the car is asked — the rate actually achieved is in the footer",
			Setting::ShowKey => "the text id to look up in names.csv, or the identifier where there is none",
		}
	}
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

	/// What to write in the key column: the text id this row's name came from.
	///
	/// A pair shows both halves' ids, because they are two lines of
	/// `names.csv` and renaming one does not rename the other. A channel with
	/// no id at all shows nothing rather than a dash — there is no line to
	/// write, and a placeholder would suggest there is.
	fn key_text(&self) -> String {
		let ids: Vec<&str> = [self.actual, self.specified]
			.into_iter()
			.flatten()
			.filter_map(|c| c.text_id.as_deref())
			.collect();
		let mut seen: Vec<&str> = Vec::new();
		for id in ids {
			if !seen.contains(&id) {
				seen.push(id);
			}
		}
		seen.join("/")
	}
}

/// How many rows the selection screen is holding back, and why.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Hidden {
	/// Nothing anywhere has a name for these.
	unnamed: usize,
	/// A survey asked this car for these and it said nothing.
	silent: usize,
}

impl Hidden {
	fn total(self) -> usize {
		self.unnamed + self.silent
	}

	/// The footer sentence, or `None` when nothing is being held back.
	///
	/// One sentence whichever reasons apply, because two lines about a filter
	/// is more screen than the filter is worth. It always ends with the key,
	/// since the count is only useful to somebody who can act on it.
	fn sentence(self) -> Option<String> {
		let reason = match (self.unnamed, self.silent) {
			(0, 0) => return None,
			(n, 0) => format!("{n} with no name anywhere"),
			(0, n) => format!("{n} this car does not answer"),
			(u, s) => format!("{u} with no name anywhere and {s} this car does not answer"),
		};
		Some(format!("{reason} are hidden — [u] shows them"))
	}
}

/// Everything the UI needs to draw itself.
pub struct App {
	pub channels: Vec<Channel>,
	/// Latest response body per `(request id, did)`, with when it arrived.
	pub latest: std::collections::BTreeMap<(u16, u16), (f64, Vec<u8>)>,
	/// The last [`history::WINDOW_SECONDS`] of every channel that answered with
	/// a number, which is what the chart is drawn from. `latest` cannot serve:
	/// it is one body per identifier and a chart is a shape over time.
	history: history::History,
	/// Which channels the chart holds — a second choice over the same list,
	/// because what belongs on a table of thirty values and what belongs on a
	/// chart of three lines are different questions.
	charted: std::collections::BTreeSet<crate::plan::Key>,
	/// The channels this car's owner marked with `f`, read from and written
	/// back to [`favourites::path_for`].
	///
	/// They are offered first on the selection screen, they survive the
	/// nameless-identifier filter, and they are what a run starts with — see
	/// [`favourites`].
	favourites: std::collections::BTreeSet<crate::plan::Key>,
	/// Which car they are kept under. `None` on a replay and on a car that
	/// would not say which car it is: the marks then last until the run ends,
	/// which is what [`App::note`] says at the moment somebody makes one.
	favourites_vin: Option<String>,
	/// One line about the last thing this screen did that did not work — a
	/// favourite that could not be written. Empty the rest of the time, which
	/// is nearly always.
	note: String,
	/// Whether the chart has the bottom of the live screen.
	chart_shown: bool,
	/// Which page of overlaid lines is up, as `chart::pages` divides them.
	chart_page: usize,
	screen: Screen,
	cursor: usize,
	/// Substring the selection screen is narrowed to. With a survey loaded
	/// there are over a thousand candidates, and stepping through them one
	/// arrow at a time is not a way to find anything.
	filter: String,
	/// True while the filter is being typed, so letters go into it instead of
	/// triggering `a`/`n`/`q`.
	typing_filter: bool,
	/// Whether the rows held back for either reason are on the list.
	///
	/// Two reasons, and both are the same complaint — a list two thirds of which
	/// cannot be used buries the third that can:
	///
	/// - **Nothing can name it.** On the reference car 787 of 2,751 channels have
	///   no name anywhere and their label is the identifier they already sit
	///   beside.
	/// - **The car was asked for it and said nothing.** A project describes a
	///   vehicle family and no one car is all of it, so a named row can sit there
	///   unable to produce a value — worse than a nameless one, because it looks
	///   like it works. How common that is on any given car is not something this
	///   comment should claim: the first attempt put it at 1,746 of 2,251 on the
	///   reference car and the real figure was 38, the rest being identifiers the
	///   sweep never asked. Hence [`crate::plan::Answered`], which will not call anything
	///   silent without a record of what was put to the unit.
	///
	/// Neither is dropped. A nameless identifier is precisely what somebody
	/// hunting a new measurement is looking for, a silent one may answer in a
	/// state the survey was not taken in, and `u` puts both back. A selected one
	/// is always on the list whatever this says, or `--did` could name a channel
	/// that could then never be unticked.
	show_hidden: bool,
	/// Which settings row the cursor is on.
	settings_cursor: usize,
	/// How often the car is asked, in hertz. Seeded from the settings and from
	/// `--hz`, and changed on the settings screen while the car is answering.
	hz: f64,
	/// Whether each row ends with the channel's own key.
	show_key: bool,
	/// What this car was seen to answer, when a survey has been loaded.
	///
	/// Empty means nothing is known, and nothing is then filtered on those
	/// grounds — see [`crate::plan::Answered::saw`], which distinguishes "asked and
	/// silent" from "never asked" precisely so this cannot hide a unit nobody
	/// swept.
	answered: crate::plan::Answered,
	/// Scroll position of the selection list. Without one, everything past the
	/// bottom of the terminal is unreachable.
	select_state: TableState,
	/// Which row of the chart-lines screen the cursor is on, as an index into
	/// [`App::shown_indices`]. Its own, not `cursor`: that one belongs to a
	/// list of every channel the car has, and this one to the handful being
	/// watched.
	series_cursor: usize,
	series_state: TableState,
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
			history: history::History::new(history::WINDOW_SECONDS),
			charted: std::collections::BTreeSet::new(),
			favourites: std::collections::BTreeSet::new(),
			favourites_vin: None,
			note: String::new(),
			chart_shown: false,
			chart_page: 0,
			screen: Screen::Live,
			cursor: 0,
			filter: String::new(),
			typing_filter: false,
			show_hidden: false,
			settings_cursor: 0,
			hz: crate::config::DEFAULT_HZ,
			show_key: false,
			answered: crate::plan::Answered::default(),
			select_state: TableState::default(),
			series_cursor: 0,
			series_state: TableState::default(),
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
		if let Some(index) = self
			.tabs()
			.iter()
			.position(|r| self.channels.iter().any(|c| c.request == *r && c.selected))
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
		let address = vag_uds_client::address::UnitAddress::from_request(request)
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
		let mut out: Vec<usize> = self
			.channels
			.iter()
			.enumerate()
			.filter(|(_, c)| unit.is_none_or(|u| c.request == u))
			.filter(|(_, c)| self.show_hidden || self.usable(c) || self.kept(c))
			.filter(|(_, c)| {
				needle.is_empty()
					|| c.label().to_lowercase().contains(&needle)
					|| format!("{:04x}", c.did).contains(&needle)
					|| c.unit().to_lowercase().contains(&needle)
			})
			.map(|(i, _)| i)
			.collect();
		// Favourites first, and otherwise the order the plan polls in. A person
		// with a handful of channels they watch every drive should not have to
		// find them again in two thousand rows.
		out.sort_by_key(|i| (!self.favourite(*i), *i));
		out
	}

	/// Whether this channel is worth a row on the default list.
	///
	/// Both halves have to hold: something has to be able to name it, *and* the
	/// car must not have been asked for it and stayed silent. The second is the
	/// half added after the ODIS import — a project declares far more than any
	/// one car answers, and a named channel that never returns a value spends a
	/// row saying nothing.
	fn usable(&self, channel: &Channel) -> bool {
		channel.is_named() && self.answered.saw(channel.request, channel.did) != Some(false)
	}

	/// Whether this channel stays on the list however little describes it.
	///
	/// Two ways: it is already being watched, and hiding a ticked row would
	/// leave `--did`'s channels impossible to untick; or somebody marked it a
	/// favourite, which is a person saying "this one" about the very row a
	/// nameless-identifier filter is designed to remove.
	fn kept(&self, channel: &Channel) -> bool {
		let key = channel.key();
		channel.selected || self.favourites.contains(&key) || self.charted.contains(&key)
	}

	/// Whether the channel at this index is one of the favourites.
	fn favourite(&self, index: usize) -> bool {
		self.channels.get(index).is_some_and(|c| self.favourites.contains(&c.key()))
	}

	/// How many of the open tab's channels this screen is holding back, split by
	/// why.
	///
	/// Counted rather than left implicit: a list that silently drops two rows
	/// in three is its own defect, so the footer says how many went and which
	/// key brings them back. Split by reason because the two are answered
	/// differently — a nameless row wants a better name source, a silent one
	/// wants the car in a different state, or wants nothing at all.
	fn hidden(&self) -> Hidden {
		if self.show_hidden {
			return Hidden::default();
		}
		let unit = self.open_unit();
		let mut out = Hidden::default();
		for c in self.channels.iter().filter(|c| unit.is_none_or(|u| c.request == u)) {
			if self.usable(c) || self.kept(c) {
				continue;
			}
			// A row can fail both tests; it is counted under the one a reader can
			// do something about first. Silence is the stronger statement — the
			// car was asked — so it wins over a missing name.
			match self.answered.saw(c.request, c.did) {
				Some(false) => out.silent += 1,
				_ => out.unnamed += 1,
			}
		}
		out
	}

	/// Rows currently on screen, in the order the plan polls them.
	/// Every selected channel, from every unit.
	///
	/// The live screen is not filtered by the open unit: the point of choosing
	/// measurements from several control units is to watch them together. The
	/// unit list belongs to the configure screen, where the choosing happens.
	fn shown(&self) -> Vec<&Channel> {
		self.shown_indices().into_iter().map(|i| &self.channels[i]).collect()
	}

	/// The same rows, as indices — for a screen that has to change one.
	///
	/// One ordering, defined here, so the chart's list and the table cannot
	/// disagree about which row is the third one.
	fn shown_indices(&self) -> Vec<usize> {
		let mut v: Vec<usize> = self.channels.iter().enumerate().filter(|(_, c)| c.selected).map(|(i, _)| i).collect();
		v.sort_by_key(|i| (self.channels[*i].request, self.channels[*i].did));
		v
	}

	/// The channels the chart actually draws, in the order it draws them.
	///
	/// Not the same set as `charted`: a mark with no number in it never reaches
	/// a line, and beyond [`CHART_CHANNELS`] there is no room for one. Both are
	/// said out loud on the screen that makes the marks, because a mark that
	/// silently does nothing is what makes a chart look broken.
	fn drawn(&self) -> Vec<crate::plan::Key> {
		self
			.shown()
			.into_iter()
			.filter(|c| self.charted.contains(&c.key()) && plottable(c))
			.take(CHART_CHANNELS)
			.map(|c| c.key())
			.collect()
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
			let paired = c.def.as_ref().and_then(|d| crate::plan::split_role(&d.name));
			let Some((base, role)) = paired else {
				out.push(DisplayRow {
					label: c.label(),
					actual: Some(c),
					specified: None,
				});
				continue;
			};
			// Same base name on the same unit — a pair from another control
			// unit is a different quantity that happens to share a name.
			let slot = out.iter_mut().find(|r| {
				r.label == base
					&& r.any().request == c.request
					&& match role {
						crate::plan::Role::Actual => r.actual.is_none(),
						crate::plan::Role::Specified => r.specified.is_none(),
					}
			});
			match (slot, role) {
				(Some(row), crate::plan::Role::Actual) => row.actual = Some(c),
				(Some(row), crate::plan::Role::Specified) => row.specified = Some(c),
				(None, crate::plan::Role::Actual) => out.push(DisplayRow {
					label: base.to_string(),
					actual: Some(c),
					specified: None,
				}),
				(None, crate::plan::Role::Specified) => out.push(DisplayRow {
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
		let read = |c: Option<&Channel>| c.and_then(|c| self.latest.get(&(c.request, c.did)).map(|(t, d)| (c.render(d), *t)));
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

	/// Take in one answer: the body for the table, and a number for the chart
	/// when there is honestly one to take.
	///
	/// Both live loops go through here rather than writing `latest` themselves,
	/// so that a recording drawn back through the same screen cannot end up
	/// with a different idea of what was measured than the car did.
	fn observe(&mut self, request: u16, did: u16, at: f64, data: Vec<u8>) {
		// **One response, several channels.** A control unit packs as many
		// fields into a `0x22` answer as it likes, and each is a line of its
		// own; taking the first channel that matched the identifier gave every
		// one of them the first field's number.
		//
		// `interpret` is the one thing that decides whether there is a value,
		// and it declines exactly what must be declined: bytes too short for the
		// form, a state, and an anchored row away from its anchor, where the
		// slope is unknown and no honest value exists.
		let seen: Vec<(crate::plan::Key, f64)> = self
			.channels
			.iter()
			.filter(|c| c.request == request && c.did == did)
			.filter_map(|c| c.def.as_ref().and_then(|def| def.interpret(&data)).map(|value| (c.key(), value)))
			.collect();
		for (key, value) in seen {
			self.history.push(key, at, value);
		}
		self.latest.insert((request, did), (at, data));
	}

	/// The lines the chart is to draw, in the order the table shows them.
	///
	/// Selection order, so that `←`/`→` means the same thing on the next cycle
	/// as it did on this one. `pages` is a pure function of what it is handed
	/// and cannot do better than the order it gets.
	fn charted(&self) -> Charted {
		let marked: Vec<&Channel> = self.shown().into_iter().filter(|c| self.charted.contains(&c.key())).collect();
		let no_number = marked.iter().filter(|c| !plottable(c)).count();
		let drawable: Vec<&Channel> = marked.iter().copied().filter(|c| plottable(c)).collect();
		let over_cap = drawable.len().saturating_sub(CHART_CHANNELS);
		// Two units on one chart mean two lines that can be called the same
		// thing — an engine speed on the engine and on the gearbox — and a key
		// that names two lines alike explains neither. With one unit the prefix
		// would be noise on every line.
		let units: std::collections::BTreeSet<u16> = drawable.iter().take(CHART_CHANNELS).map(|c| c.request).collect();
		let series = drawable
			.iter()
			.take(CHART_CHANNELS)
			.map(|c| chart::Series {
				label: match units.len() > 1 {
					true => format!("{} {}", c.unit(), c.label()),
					false => c.label(),
				},
				unit: c.unit_of_measure().to_string(),
				points: self.history.points(c.key()),
				// Everything on this screen came off the bus. `watch` reads and
				// does not compute, and the day it does the distinction is
				// already drawn here.
				origin: chart::Origin::Bus,
			})
			.collect();
		Charted { series, no_number, over_cap }
	}

	/// Show or hide the chart, seeding it the first time from what is already
	/// on screen.
	///
	/// A chart that opens empty on the one key that opens it teaches nobody
	/// that it exists, and the marks are one keypress each to change
	/// afterwards. What it seeds from is the table's own order, capped at what
	/// the chart draws and skipping anything with no number in it.
	fn toggle_chart(&mut self) {
		self.chart_shown = !self.chart_shown;
		if !self.chart_shown || !self.charted.is_empty() {
			return;
		}
		let seed: Vec<crate::plan::Key> = self
			.shown()
			.into_iter()
			.filter(|c| plottable(c))
			.take(CHART_CHANNELS)
			.map(|c| c.key())
			.collect();
		self.charted.extend(seed);
		self.remember_charted();
	}

	/// Change one setting, and write it down.
	///
	/// **Written on every press**, for the reason a favourite is: this screen is
	/// used in a car park with the lid about to close, and a preference that
	/// only survives a tidy exit is one that does not survive.
	///
	/// The rate steps rather than scales — a person tuning it is asking "a bit
	/// faster" and not "twice as fast", and doubling walks straight off the top
	/// of the useful range in three presses.
	fn nudge_setting(&mut self, setting: Setting, up: bool) {
		let mut document = crate::config::load();
		match setting {
			Setting::Rate => {
				let step = if up { RATE_STEP } else { -RATE_STEP };
				self.hz = (self.hz + step).clamp(crate::config::MIN_HZ, crate::config::MAX_HZ);
				crate::config::set_hz(&mut document, self.hz);
			}
			Setting::ShowKey => {
				self.show_key = !self.show_key;
				crate::config::set_show_key(&mut document, self.show_key);
			}
		}
		self.note = match crate::config::save(&document) {
			Ok(()) => String::new(),
			Err(why) => format!("could not save the setting: {why}"),
		};
	}

	/// What one setting currently reads as.
	fn setting_value(&self, setting: Setting) -> String {
		match setting {
			Setting::Rate => format!("{:.1} Hz", self.hz),
			Setting::ShowKey => match self.show_key {
				true => "shown".to_string(),
				false => "hidden".to_string(),
			},
		}
	}

	/// Mark or unmark the channel under the cursor as a favourite, and write
	/// the file.
	///
	/// **Written on every press, not once at the end.** `watch` is quit with
	/// `q` in a car park about as often as it is killed by a closed lid, and a
	/// mark that only survives a tidy exit is a mark that does not survive.
	///
	/// Marking selects it too, for the reason [`Self::toggle_charted`] does:
	/// "watch this every drive" that leaves the row unticked would be a mark
	/// with no effect on the run somebody made it during.
	fn toggle_favourite(&mut self, index: usize) {
		let Some(channel) = self.channels.get_mut(index) else { return };
		let key = channel.key();
		if self.favourites.remove(&key) {
			// Unfavouriting is not deselecting: somebody who no longer wants a
			// row *next* time is not asking for it to vanish from this screen.
		} else {
			self.favourites.insert(key);
			channel.selected = true;
		}
		self.note = match favourites::save(self.favourites_vin.as_deref(), &self.favourites) {
			Ok(()) => String::new(),
			Err(why) => why,
		};
	}

	/// Tick whatever this car's favourites are, and say how many were found.
	///
	/// A favourite the car does not offer is not an error and not a warning:
	/// which identifiers a unit answers changes with a software update, and a
	/// mark for one that has gone simply has nothing to tick.
	fn select_favourites(&mut self) -> usize {
		let mut found = 0;
		for channel in self.channels.iter_mut() {
			if self.favourites.contains(&channel.key()) {
				channel.selected = true;
				found += 1;
			}
		}
		found
	}

	/// Mark or unmark the channel under the cursor.
	///
	/// Marking selects it too: a line the poll loop is not feeding can never
	/// have a point in it, and would sit in the key with nothing under it for
	/// the whole run.
	fn toggle_charted(&mut self, index: usize) {
		let Some(channel) = self.channels.get_mut(index) else { return };
		let key = channel.key();
		if !self.charted.remove(&key) {
			self.charted.insert(key);
			channel.selected = true;
		}
		self.chart_page = 0;
		self.remember_charted();
	}

	/// Write the chart's lines down, for the same reason a favourite is written
	/// on every press: choosing three lines out of two thousand channels is the
	/// work, and doing it again next drive is the work again.
	fn remember_charted(&mut self) {
		let Some(vin) = self.favourites_vin.clone() else { return };
		let mut document = crate::config::load();
		let keys: Vec<String> = self.charted.iter().map(|key| favourites::render_key(*key)).collect();
		crate::config::set_charted(&mut document, &vin, &keys);
		self.note = match crate::config::save(&document) {
			Ok(()) => String::new(),
			Err(why) => format!("could not save the chart lines: {why}"),
		};
	}

	/// Drop chart marks for channels that are no longer polled.
	///
	/// Called after anything that can deselect. A marked channel that stopped
	/// being polled draws a line that stops dead and then, a minute later,
	/// nothing — and looks exactly like a control unit that went quiet.
	fn prune_charted(&mut self) {
		let selected: std::collections::BTreeSet<crate::plan::Key> = self.channels.iter().filter(|c| c.selected).map(|c| c.key()).collect();
		let before = self.charted.len();
		self.charted.retain(|key| selected.contains(key));
		if self.charted.len() != before {
			self.chart_page = 0;
		}
	}

	/// Step to the next or previous page of lines, wrapping.
	///
	/// The count comes from the widget, because how many series share a page is
	/// the widget's decision and a second opinion here would be a second answer.
	fn step_chart(&mut self, forward: bool) {
		let pages = chart::pages(&self.charted().series).len();
		if pages == 0 {
			return;
		}
		self.chart_page = match forward {
			true => (self.chart_page + 1) % pages,
			false => (self.chart_page + pages - 1) % pages,
		};
	}

	fn age(&self, at: f64) -> String {
		format!("{:.1}s", (self.clock - at).max(0.0))
	}

	fn poll_rate(&self) -> f64 {
		let secs = self.started.elapsed().as_secs_f64();
		if secs <= 0.0 { 0.0 } else { self.cycles as f64 / secs }
	}
}

/// How much one press moves the poll rate.
const RATE_STEP: f64 = 0.5;

/// Draw the live table.
///
/// Column widths come from the content, so a long name is shown in full rather
/// than elided — the whole reason for moving off a single scrolling line.
/// What each unit called itself, for the tab labels.
///
/// The component string when the unit gave one, else its part number — both
/// come from the unit, so a tab never carries a name this project made up.
fn unit_names(identities: &[crate::plan::UnitIdentity]) -> Vec<(u16, String)> {
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
	let split = Layout::horizontal([Constraint::Length(width), Constraint::Min(20)]).split(area);

	let rows: Vec<Row> = units
		.iter()
		.map(|request| {
			let selected = app.channels.iter().filter(|c| c.request == *request && c.selected).count();
			let available = app.channels.iter().filter(|c| c.request == *request).count();
			Row::new(vec![Cell::from(app.tab_label(*request)), Cell::from(format!("{selected}/{available}"))])
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

/// Where the chart goes, and what stands in for it before anything is marked.
///
/// Every decision inside the frame is [`chart::plot`]'s and every stroke is
/// [`chart::draw`]'s — the paging, the fold, the key, the palette, which line
/// comes off when the terminal is narrow. What is left here is the sentence for
/// the empty case, which is about this screen's selection rather than about a
/// plot, which is exactly why `plot` hands that case back to its caller.
fn draw_chart(frame: &mut Frame, page: usize, charted: &Charted, area: Rect) {
	let Some(plotted) = chart::plot(&charted.series, page, area.width) else {
		frame.render_widget(
			Block::default()
				.borders(Borders::ALL)
				.title(" chart — nothing marked: [c] configure, then [g] on a measurement "),
			area,
		);
		return;
	};
	chart::draw(frame, &plotted, area);
}

/// What `watch` has to say about its own chart, which the widget cannot.
///
/// The window, because a chart whose extent is a secret is a chart nobody can
/// read; and the two ways a marked channel does not reach the screen, because a
/// driver who marks one and sees nothing appear concludes the tool is broken.
///
/// It goes on a line of its own under the chart rather than into the footer
/// with the key hints. Not taste: the footer is one row inside a border and
/// these sentences are as long as the selection makes them, so in there the
/// last of them is the one that gets clipped — and the one that gets clipped is
/// the one that says what is missing.
fn chart_note(charted: &Charted) -> String {
	let mut note = format!(" last {:.0}s", history::WINDOW_SECONDS);
	if charted.over_cap > 0 {
		note.push_str(&format!(" · {} more marked than it draws", charted.over_cap));
	}
	if charted.no_number > 0 {
		note.push_str(&format!(" · {} marked with no proven number", charted.no_number));
	}
	note
}

fn draw_live(frame: &mut Frame, app: &mut App) {
	// Built before anything is drawn, because the chart is what decides how
	// much of the screen the table gets.
	let charted = app.chart_shown.then(|| app.charted());
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
	// Built before the layout, because how many rows it wraps to is what the
	// footer's height has to be. It was one row and did not wrap: on a replay
	// at eighty columns the playback keys ran off the end of it, taking `[q]
	// quit` with them.
	let help = format!(
		" {rate}{} of {} shown · [tab] unit  [c] configure  [g] chart  [s] lines  [,] settings  [q] quit{}{waiting}",
		app.rows().len(),
		app.channels.iter().filter(|c| c.selected).count(),
		app.status
	);
	let hint_rows = wrapped_height(&help, frame.area().width.saturating_sub(2)) + 2;
	let layout = match charted.is_some() {
		// A chart in a few lines is a smear, and a chart that takes the screen
		// leaves no table to read the numbers off. Two fifths is the split that
		// keeps both readable at the 24 rows a terminal is still allowed to be.
		true => Layout::vertical([Constraint::Min(3), Constraint::Percentage(40), Constraint::Length(hint_rows)]).split(frame.area()),
		false => Layout::vertical([Constraint::Min(3), Constraint::Length(hint_rows)]).split(frame.area()),
	};
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
		let mut cells = vec![
			Cell::from(c.unit()),
			Cell::from(dids),
			Cell::from(r.label.clone()),
			Cell::from(value).style(Style::default().add_modifier(Modifier::BOLD)),
			Cell::from(c.unit_of_measure().to_string()),
			Cell::from(age).style(Style::default().fg(Color::DarkGray)),
		];
		// Last, and only when asked for: it is the line to look up in
		// names.csv, which nobody needs while the car is moving and everybody
		// needs the moment a row reads badly.
		if app.show_key {
			cells.push(Cell::from(r.key_text()).style(Style::default().fg(Color::DarkGray)));
		}
		rows.push(Row::new(cells));
	}

	let heading_w = shown
		.iter()
		.map(|r| app.unit_heading(r.any().request).chars().count() + 1)
		.max()
		.unwrap_or(0);
	let name_w = shown.iter().map(|r| r.label.len()).chain([heading_w]).max().unwrap_or(4).max(11) as u16;
	let did_w = shown
		.iter()
		.map(|r| if r.actual.is_some() && r.specified.is_some() { 9 } else { 4 })
		.max()
		.unwrap_or(4) as u16;
	let value_w = shown.iter().map(|r| app.value_of(r).0.len()).max().unwrap_or(8).max(14) as u16;

	// The key column is as wide as the widest id on screen and no wider: a
	// fixed width would either clip `IDE00022` or leave a gap on a table where
	// no row has one.
	let key_w = match app.show_key {
		true => shown.iter().map(|r| r.key_text().chars().count()).max().unwrap_or(0).max(3) as u16,
		false => 0,
	};
	let mut widths = vec![
		Constraint::Length(4),
		Constraint::Length(did_w),
		Constraint::Length(name_w),
		Constraint::Length(value_w),
		Constraint::Length(9),
		Constraint::Length(6),
	];
	let mut headings = vec!["ECU", "DID", "Measurement", "Actual / specified", "Unit", "Age"];
	if app.show_key {
		widths.push(Constraint::Length(key_w));
		headings.push("Key");
	}
	let table = Table::new(rows, widths)
		.header(Row::new(headings).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
		.block(Block::default().borders(Borders::ALL).title(" vagcan watch "));
	frame.render_widget(table, table_area);

	if let Some(charted) = &charted {
		let split = Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).split(layout[1]);
		draw_chart(frame, app.chart_page, charted, split[0]);
		frame.render_widget(Paragraph::new(chart_note(charted)).style(Style::default().fg(Color::DarkGray)), split[1]);
	}

	frame.render_widget(
		Paragraph::new(help)
			.wrap(ratatui::widgets::Wrap { trim: false })
			.block(Block::default().borders(Borders::ALL)),
		*layout.last().expect("the footer is the last row of the layout"),
	);
}

/// What a row of the chart-lines screen says about itself, after the mark.
///
/// Three states and they are three different facts, which is why none of them
/// is left to be inferred from an empty chart: a channel with no proven number
/// can never be a line, a marked one past the cap is waiting for room, and the
/// rest are on screen right now.
fn series_note(app: &App, channel: &Channel) -> &'static str {
	let key = channel.key();
	if !plottable(channel) {
		// It keeps its row on the table either way; it is the chart that
		// declines it, and this is where it says so.
		return "no number";
	}
	if !app.charted.contains(&key) {
		return "";
	}
	match app.drawn().contains(&key) {
		true => "drawn",
		false => "no room",
	}
}

/// The chart-lines screen: the watched channels, and which of them are lines.
///
/// The chart itself stays on screen underneath, which is the point — the
/// previous way to change a line was to leave the chart, find the channel among
/// every one the car has, and come back to see what had happened.
/// Draw the settings screen.
///
/// Deliberately plain: three columns, and the note beside each row saying what
/// the setting is *for*. A settings screen whose rows are only labels makes
/// somebody guess, and the guess is made while sitting in a car.
fn draw_settings(frame: &mut Frame, app: &mut App) {
	let help = concat!(" [↑↓] move  [←→] change  [space] toggle\n", " [c] configure  [,]/[esc] back  [q] quit");
	let hint_rows = wrapped_height(help, frame.area().width.saturating_sub(2)) + 2;
	let layout = Layout::vertical([Constraint::Min(3), Constraint::Length(hint_rows)]).split(frame.area());
	let at = app.settings_cursor.min(Setting::ALL.len() - 1);

	let rows: Vec<Row> = Setting::ALL
		.iter()
		.map(|setting| {
			Row::new(vec![
				Cell::from(setting.label()),
				Cell::from(app.setting_value(*setting)).style(Style::default().fg(Color::Cyan)),
				Cell::from(setting.note()).style(Style::default().fg(Color::DarkGray)),
			])
		})
		.collect();

	let fixed: u16 = 28 + 1 + 9 + 1;
	let note_w = layout[0].width.saturating_sub(fixed).max(10);
	let table = Table::new(rows, [Constraint::Length(28), Constraint::Length(9), Constraint::Length(note_w)])
		.row_highlight_style(Style::default().add_modifier(Modifier::REVERSED))
		.block(
			Block::default()
				.borders(Borders::ALL)
				.title(format!(" settings — kept in {} ", short_config_path())),
		);
	let mut state = TableState::default();
	state.select(Some(at));
	frame.render_stateful_widget(table, layout[0], &mut state);
	let mut hints = help.to_string();
	if !app.note.is_empty() {
		hints.push('\n');
		hints.push(' ');
		hints.push_str(&app.note);
	}
	frame.render_widget(
		Paragraph::new(hints)
			.wrap(ratatui::widgets::Wrap { trim: false })
			.block(Block::default().borders(Borders::ALL)),
		layout[1],
	);
}

/// The settings file, written short enough for a title bar.
///
/// The home directory is replaced by `~` rather than printed: on this machine
/// it is a third of the width of the box, and the part that matters is the file.
fn short_config_path() -> String {
	let Ok(path) = crate::config::path() else {
		return "~/.vagcan/config.toml".to_string();
	};
	let text = path.display().to_string();
	match dirs::home_dir().map(|home| home.display().to_string()) {
		Some(home) if text.starts_with(&home) => text.replacen(&home, "~", 1),
		_ => text,
	}
}

fn draw_series(frame: &mut Frame, app: &mut App) {
	let charted = app.charted();
	// Two lines of hints, because they do not fit one at eighty columns, and
	// the block is sized from them for the reason the selection screen's is:
	// a footer that clips is a footer that clips the last sentence on it.
	let help = concat!(
		" [space] draw  [↑↓] move  [←→] page  [a] all  [n] none\n",
		" [c] configure  [s]/[esc] back  [q] quit"
	);
	let hint_rows = wrapped_height(help, frame.area().width.saturating_sub(2)) + 2;
	let layout = Layout::vertical([Constraint::Min(3), Constraint::Percentage(40), Constraint::Length(hint_rows)]).split(frame.area());
	let shown = app.shown_indices();
	let drawn = app.drawn();

	let rows: Vec<Row> = shown
		.iter()
		.map(|i| {
			let c = &app.channels[*i];
			let marked = app.charted.contains(&c.key());
			let note = series_note(app, c);
			Row::new(vec![
				Cell::from(if marked { "[x]" } else { "[ ]" }),
				Cell::from(c.unit()),
				Cell::from(format!("{:04X}", c.did)),
				Cell::from(c.label()),
				Cell::from(c.unit_of_measure().to_string()),
				Cell::from(note).style(Style::default().fg(match note {
					"drawn" => Color::Cyan,
					"" => Color::Reset,
					_ => Color::DarkGray,
				})),
			])
		})
		.collect();

	// The same arithmetic as the selection screen, and for the same failure:
	// one long ODIS name must not push the note off the right of an 80-column
	// terminal, because the note is the half that says why nothing appeared.
	let fixed: u16 = 3 + 1 + 4 + 1 + 5 + 1 + 1 + 9 + 1 + 9;
	let room = layout[0].width.saturating_sub(fixed).max(11);
	let name_w = shown
		.iter()
		.map(|i| app.channels[*i].label().chars().count() as u16)
		.max()
		.unwrap_or(11)
		.clamp(11, room);
	let title = format!(
		" chart lines — {} drawn of {} marked, {CHART_CHANNELS} at most ",
		drawn.len(),
		app.charted.len()
	);
	let table = Table::new(
		rows,
		[
			Constraint::Length(3),
			Constraint::Length(4),
			Constraint::Length(5),
			Constraint::Length(name_w),
			Constraint::Length(9),
			Constraint::Length(9),
		],
	)
	.row_highlight_style(Style::default().bg(Color::Blue).add_modifier(Modifier::BOLD))
	.block(Block::default().borders(Borders::ALL).title(title));

	app
		.series_state
		.select((!shown.is_empty()).then_some(app.series_cursor.min(shown.len().saturating_sub(1))));
	frame.render_stateful_widget(table, layout[0], &mut app.series_state);

	let split = Layout::vertical([Constraint::Min(5), Constraint::Length(1)]).split(layout[1]);
	draw_chart(frame, app.chart_page, &charted, split[0]);
	frame.render_widget(Paragraph::new(chart_note(&charted)).style(Style::default().fg(Color::DarkGray)), split[1]);
	frame.render_widget(
		Paragraph::new(help)
			.wrap(ratatui::widgets::Wrap { trim: false })
			.block(Block::default().borders(Borders::ALL)),
		layout[2],
	);
}

/// The selection screen's title: how much of the car is on this list.
///
/// Three numbers, because there are three ways a channel is not on screen and
/// a person who cannot tell them apart cannot act on any of them: this tab
/// rather than another, a filter they typed, and the rows nothing can name.
/// Pure so the wording can be asserted without a terminal.
fn select_title(app: &App, shown: usize) -> String {
	let mut title = format!(" choose what to show — {shown}");
	if !app.filter.is_empty() {
		let _ = write!(title, " of {} matching {:?}", app.channels.len(), app.filter);
	}
	let hidden = app.hidden();
	if hidden.total() > 0 {
		let _ = write!(title, " · {} hidden", hidden.total());
	}
	title.push(' ');
	title
}

/// The keys this screen has, in the order somebody reaches for them.
///
/// It says how to bring the hidden rows back *where they went missing*, not in
/// a manual: a list that quietly drops two rows in three is its own defect, and
/// the sentence undoing it has to be on the same screen as the shortening.
fn select_keys(app: &App) -> String {
	// Broken into lines here rather than left to the wrap. This screen has the
	// most keys in the tool, its hints have not fitted one row since the chart
	// mark landed, and a wrap chosen by the widget put the break in the middle
	// of `[a] all` — a key spelled across two rows is a key nobody reads.
	let mut keys = String::from(concat!(
		" [space]/click toggle  [f] favourite  [g] chart  [u] hidden\n",
		" [/] filter  [↑↓ pgup/pgdn] move  [tab] unit  [a] all  [n] none\n",
		" [,] settings  [enter] back"
	));
	if let Some(sentence) = app.hidden().sentence() {
		let _ = write!(keys, "\n {sentence}");
	} else if app.show_hidden {
		let _ = write!(keys, "\n [u] hides the rows with no name and the ones this car does not answer");
	}
	if !app.note.is_empty() {
		let _ = write!(keys, "\n {}", app.note);
	}
	keys.push(' ');
	keys
}

/// How many screen rows a paragraph takes once it has been wrapped.
///
/// The hint block is sized from this rather than from a constant. It was a
/// constant — four rows, two of them border — and every sentence added to the
/// screen since has been silently cut off the bottom: first `[enter] back`, the
/// key that leaves the screen, and then the line saying how many rows are
/// hidden and which key brings them back. A footer that drops the sentence
/// explaining a shortened list is worse than no sentence.
///
/// Greedy on whitespace, which is what `ratatui`'s `Wrap` does. It may
/// over-count where a word lands exactly on the edge; a spare row is a blank
/// line and a row too few is a lost sentence.
fn wrapped_height(text: &str, width: u16) -> u16 {
	let width = width.max(1) as usize;
	let mut rows = 0u16;
	for line in text.lines() {
		let mut used = 0usize;
		let mut on_line = 1u16;
		for word in line.split(' ') {
			let word = word.chars().count();
			if used == 0 {
				used = word;
			} else if used + 1 + word <= width {
				used += 1 + word;
			} else {
				on_line += 1;
				used = word;
			}
			// A single word longer than the screen wraps inside itself.
			while used > width {
				on_line += 1;
				used -= width;
			}
		}
		rows += on_line;
	}
	rows.max(1)
}

/// Draw the selection screen.
fn draw_select(frame: &mut Frame, app: &mut App) {
	// Two rows for the hints, and they wrap. This screen has the most keys of
	// any in the tool and its hint line was already longer than a hundred
	// columns before the chart mark was added to it — which meant `[enter]
	// back`, the key that leaves the screen, was clipped off the end of it on
	// an ordinary terminal.
	let help = match app.typing_filter {
		true => format!(" filter: {}▏ [enter] apply  [esc] clear ", app.filter),
		false => select_keys(app),
	};
	// The hints take the rows they need. A fixed height is how `[enter] back`
	// and then the hidden-row count each fell off the bottom of this screen.
	let hint_rows = wrapped_height(&help, frame.area().width.saturating_sub(2)) + 2;
	let layout = Layout::vertical([Constraint::Min(3), Constraint::Length(hint_rows)]).split(frame.area());
	let table_area = draw_units(frame, app, layout[0]);
	let visible = app.visible();
	let rows: Vec<Row> = visible
		.iter()
		.map(|i| {
			let c = &app.channels[*i];
			let mark = if c.selected { "[x]" } else { "[ ]" };
			// The chart mark is a word rather than a second box: two boxes side
			// by side on one row is a puzzle, and this one is the rarer choice
			// of the two.
			let charted = match app.charted.contains(&c.key()) {
				true => "chart",
				false => "",
			};
			Row::new(vec![
				Cell::from(mark),
				// A favourite is a person's own mark, so it is a glyph and not
				// a word: it has to be findable by eye down a long column.
				Cell::from(if app.favourite(*i) { "★" } else { "" }).style(Style::default().fg(Color::Yellow)),
				// No control unit column. This list is one unit's — the tab
				// filters it — and the unit is named in the list beside it. At
				// eighty columns those five characters are the difference
				// between `Engine_temperature` and `Engine_temperat`.
				Cell::from(format!("{:04X}", c.did)),
				Cell::from(c.label()),
				Cell::from(c.unit_of_measure().to_string()),
				Cell::from(charted).style(Style::default().fg(Color::Cyan)),
			])
		})
		.collect();

	// Everything but the name has a fixed width, so the name takes what is
	// left of the screen. Taking the longest label of every channel instead —
	// which is what this did — pushed the unit and the chart mark off the
	// right of an 80-column terminal as soon as one ODIS name ran long.
	let fixed: u16 = 3 + 1 + 1 + 1 + 5 + 1 + 1 + 9 + 1 + 5;
	let room = table_area.width.saturating_sub(fixed).max(11);
	let name_w = visible
		.iter()
		.map(|i| app.channels[*i].label().chars().count() as u16)
		.max()
		.unwrap_or(11)
		.clamp(11, room);
	let title = select_title(app, visible.len());
	let table = Table::new(
		rows,
		[
			Constraint::Length(3),
			Constraint::Length(1),
			Constraint::Length(5),
			Constraint::Length(name_w),
			Constraint::Length(9),
			Constraint::Length(5),
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

	frame.render_widget(
		Paragraph::new(help)
			.wrap(ratatui::widgets::Wrap { trim: false })
			.block(Block::default().borders(Borders::ALL)),
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
				// A click deselects exactly as `space` does, so it drops a
				// chart mark exactly as `space` does.
				app.prune_charted();
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
		// `Esc` quits here where in `measure` it cancels a run. The divergence
		// is deliberate and predates the chart: a stopwatch needs a cheap
		// "throw this one away" and `watch` has nothing to throw away.
		Screen::Live => match code {
			KeyCode::Char('q') | KeyCode::Esc => return false,
			KeyCode::Char('c') => app.screen = Screen::Select,
			KeyCode::Char(',') => app.screen = Screen::Settings,
			KeyCode::Char('g') => app.toggle_chart(),
			// Choosing the lines implies wanting to see them, so the chart
			// comes up with the list — a screen for picking what a hidden
			// chart draws would be picking in the dark.
			KeyCode::Char('s') => {
				if !app.chart_shown {
					app.toggle_chart();
				}
				app.screen = Screen::Series;
			}
			// `←`/`→` are the chart's own advertised keys — its bottom border
			// prints `←→ chart` when there is more than one page — so they
			// belong to it, and to nothing while it is down.
			KeyCode::Left if app.chart_shown => app.step_chart(false),
			KeyCode::Right if app.chart_shown => app.step_chart(true),
			_ => {}
		},
		Screen::Series => {
			let shown = app.shown_indices();
			let at = app.series_cursor.min(shown.len().saturating_sub(1));
			let step = |app: &mut App, to: usize| app.series_cursor = to.min(shown.len().saturating_sub(1));
			match code {
				KeyCode::Char('q') => return false,
				KeyCode::Char('s') | KeyCode::Esc | KeyCode::Enter => app.screen = Screen::Live,
				KeyCode::Char('c') => app.screen = Screen::Select,
				KeyCode::Up => step(app, at.saturating_sub(1)),
				KeyCode::Down => step(app, at + 1),
				KeyCode::PageUp => step(app, at.saturating_sub(10)),
				KeyCode::PageDown => step(app, at + 10),
				KeyCode::Home => step(app, 0),
				KeyCode::End => step(app, shown.len().saturating_sub(1)),
				// `←`/`→` are the chart's own advertised keys — its bottom
				// border prints `←→ chart` — and the chart is on this screen.
				KeyCode::Left => app.step_chart(false),
				KeyCode::Right => app.step_chart(true),
				KeyCode::Char(' ') => {
					if let Some(i) = shown.get(at) {
						app.toggle_charted(*i);
					}
				}
				// `a` and `n` act on what is on this screen, which is what is
				// being watched — never on every channel the car has.
				KeyCode::Char('a') => {
					for i in &shown {
						let key = app.channels[*i].key();
						if plottable(&app.channels[*i]) {
							app.charted.insert(key);
						}
					}
					app.chart_page = 0;
					app.remember_charted();
				}
				KeyCode::Char('n') => {
					app.charted.clear();
					app.chart_page = 0;
					app.remember_charted();
				}
				_ => {}
			}
		}
		Screen::Settings => {
			let at = app.settings_cursor.min(Setting::ALL.len() - 1);
			match code {
				KeyCode::Char('q') => return false,
				KeyCode::Char(',') | KeyCode::Esc | KeyCode::Enter => app.screen = Screen::Live,
				KeyCode::Char('c') => app.screen = Screen::Select,
				KeyCode::Up => app.settings_cursor = at.saturating_sub(1),
				KeyCode::Down => app.settings_cursor = (at + 1).min(Setting::ALL.len() - 1),
				// One key for "more" and one for "less", and `space` for the
				// settings that are on or off. A rate has no meaningful toggle
				// and a flag has no meaningful step, so each row takes the keys
				// its own kind of value has.
				KeyCode::Left => app.nudge_setting(Setting::ALL[at], false),
				KeyCode::Right => app.nudge_setting(Setting::ALL[at], true),
				KeyCode::Char(' ') => app.nudge_setting(Setting::ALL[at], true),
				_ => {}
			}
		}
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
				KeyCode::Char('f') => app.toggle_favourite(app.cursor),
				KeyCode::Char('g') => app.toggle_charted(app.cursor),
				KeyCode::Char(',') => app.screen = Screen::Settings,
				// Everything held back, on the list again: the rows nothing can
				// name, and the ones this car answered nothing to. A person
				// hunting a measurement nobody has proven wants the first, and a
				// person who thinks the survey was taken in the wrong state wants
				// the second. The footer is where they learn the key exists.
				KeyCode::Char('u') => {
					app.show_hidden = !app.show_hidden;
					// The cursor may have been standing on a row that just went
					// away; land it somewhere that still exists.
					if !app.visible().contains(&app.cursor) {
						if let Some(first) = app.visible().first() {
							app.cursor = *first;
						}
					}
				}
				_ => {}
			}
			// Anything above can have deselected a channel, and a chart mark
			// outliving the poll that feeds it is a line that stops dead.
			app.prune_charted();
		}
	}
	true
}

/// Play a recorded drive back through the same screen, with no car.
///
/// A separate loop from the live one, deliberately: see `replay`'s module
/// docs. Nothing here opens a port or addresses a control unit.
pub async fn run_recording(recording_path: &str, catalogs: &str, survey: Option<&str>, speed: f64) -> Result<()> {
	let csv = std::fs::read_to_string(recording_path).with_context(|| format!("reading the recording {recording_path:?}"))?;
	let recording = replay::Recording::parse(&csv).map_err(|e| anyhow::anyhow!("{recording_path}: {e}"))?;

	let store = vag_data_labels::catalog::CatalogStore::open(catalogs);
	// A recording carries no identification block, so the catalogs are offered
	// for every unit this project has one for. On a replay that is honest:
	// nothing is being addressed, and a column only appears if it matched.
	let mut identities: Vec<crate::plan::UnitIdentity> = Vec::new();
	let survey_text = match survey {
		Some(path) => Some(std::fs::read_to_string(path).with_context(|| format!("reading the survey {path:?}"))?),
		None => None,
	};
	if let Some(text) = &survey_text {
		identities = crate::plan::identities_from_survey(text);
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
				identities.push(crate::plan::UnitIdentity {
					request: crate::plan::ENGINE,
					part_number: Some(part.to_string()),
					odx_name: None,
					odx_version: None,
					component: None,
				});
			}
		}
	}

	// Bound rather than built twice: the coverage line names the project these
	// very rows came out of, and a second `current()` could answer differently.
	let extracted = crate::extracted::current();
	let mut channels = crate::plan::available(&store, &extracted, &identities);
	// Everything the survey found, exactly as the live view folds it in. A
	// replay is what this interface is *shown* with, and without this it showed
	// a tidier tool than the one that exists: 1,964 channels of a car that
	// answers 2,751, and none of the units no project describes at all.
	if let Some(text) = &survey_text {
		channels = crate::plan::with_survey(channels, text);
	}
	// A recording does not say which unit each column came from. Columns that
	// match a known measurement keep its unit; the rest are attributed to the
	// engine's id, which is a label on a screen and addresses nothing — no
	// request is ever sent in this mode.
	let resolved = replay::resolve(&recording.columns, &mut channels, crate::plan::ENGINE);
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

	let screen = term::full_screen()
		.with_mouse()
		.enter()
		.map_err(|e| anyhow::anyhow!("`watch` needs an interactive terminal (it draws a full-screen view): {e}"))?;
	let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

	let mut app = App::new(channels);
	// The same filter the live view gets, for the same reason: a replay is what
	// this interface is shown with, and a demonstration that quietly offers two
	// thousand channels the car never answers is showing a different tool.
	if let Some(text) = &survey_text {
		app.answered = crate::plan::answered_from_survey(text);
	}
	app.open_first_populated();
	app.units = match named_by_survey {
		true => unit_names(&identities),
		false => {
			let file = std::path::Path::new(recording_path)
				.file_name()
				.map(|n| n.to_string_lossy().to_string())
				.unwrap_or_else(|| recording_path.to_string());
			vec![(crate::plan::ENGINE, file)]
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
				let Some(cell) = cells.get(*column).and_then(|c| c.as_ref()) else {
					continue;
				};
				let channel = &app.channels[hit.channel];
				let (request, did) = (channel.request, channel.did);
				if let Some(bytes) = replay::cell_to_bytes(cell, channel, hit.raw) {
					app.observe(request, did, playhead, bytes);
				}
			}
		}
		app.clock = playhead;
		// The clock here is the playhead, which wraps at the end of the
		// recording and jumps when somebody seeks. `History` is written to
		// survive that; this is where it happens.
		app.history.trim(playhead);
		// `←`/`→` seek, and they page the chart while it is up. One key cannot
		// mean two things at once, so the hint says which of the two it means
		// now rather than advertising a key that will not do what it says.
		let seek = match app.chart_shown {
			true => "  [g] chart off to seek",
			false => "  [←→] seek",
		};
		app.status = format!(
			" · [space] pause{seek}  [+-] speed · {:.0}/{:.0}s ×{speed:.2}{}",
			playhead,
			duration,
			if paused { " PAUSED" } else { "" }
		);
		app.cycles += 1;

		terminal.draw(|f| match app.screen {
			Screen::Live => draw_live(f, &mut app),
			Screen::Select => draw_select(f, &mut app),
			Screen::Series => draw_series(f, &mut app),
			Screen::Settings => draw_settings(f, &mut app),
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
							// Seeking only while the chart is down. With it up
							// the arrows are the chart's, which is what its own
							// border says they are.
							KeyCode::Left if !app.chart_shown => {
								playhead = (playhead - 10.0).max(0.0);
								continue;
							}
							KeyCode::Right if !app.chart_shown => {
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

	// The line below belongs on the screen this was started from, so the
	// alternate screen goes before it rather than at the end of the function.
	// ratatui's `Drop` shows the cursor it hid while drawing, so it goes first.
	drop(terminal);
	drop(screen);
	println!(
		"replayed {recording_path} — {:.0}s of driving, {} columns, {} of which ever changed",
		duration,
		recording.columns.len(),
		moved.len()
	);
	result
}

/// Run the live view against a real adapter.
/// Read one batch of identifiers and record the answer against the clock.
///
/// Shared by the full-screen view and the plain-console one so the two cannot
/// drift: whatever a recording means, it means the same thing in both.
async fn poll_batch<B: vag_uds_can::CanBackend>(app: &mut App, backend: &mut Option<B>, batch: &crate::plan::Batch) {
	let (at, outcome) = crate::plan::read_batch(backend, batch, app.started).await;
	let records = match outcome {
		// Nothing was sent, so nothing about the clock has moved on either.
		crate::plan::BatchOutcome::Unaddressable => return,
		// The clock still advances: the wait happened, and a row that keeps
		// its old value has to be seen ageing.
		crate::plan::BatchOutcome::NoAnswer => Vec::new(),
		crate::plan::BatchOutcome::Answered(records) => records,
	};
	app.clock = at;
	for (did, data) in records {
		app.observe(batch.request, did, at, data);
	}
	// Sweeping every channel and not only the ones just answered: a channel
	// that was deselected mid-run stops being polled, and its last minute would
	// otherwise sit in memory for the rest of the drive.
	app.history.trim(at);
}

/// One CSV row of whatever is selected, writing the header first.
///
/// A raw column is marked, because a four-digit hex value and a four-digit
/// decimal are the same string — the reader cannot tell them apart from the
/// value alone. Every value carries its own time, because identifiers are
/// polled in batches and columns are up to a cycle apart.
fn write_row<W: std::io::Write>(w: &mut W, app: &App, header_written: &mut bool) -> Result<()> {
	let shown = app.shown();
	if !*header_written {
		let cols: Vec<String> = shown
			.iter()
			.map(|c| {
				let name = if c.def.is_some() { c.label() } else { format!("{}_raw", c.label()) };
				format!("{name}_t_s,{name}")
			})
			.collect();
		writeln!(w, "t_s,{}", cols.join(","))?;
		*header_written = true;
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
	Ok(())
}

/// Where the identifiers beyond the proven catalogs are coming from.
///
/// The catalogs cover three control units of this car's fifteen. Everything the
/// other twelve answer is watchable only because some sweep wrote down what
/// they answered — so which sweep that was is worth naming on screen, and worth
/// saying when there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
enum SurveySource {
	/// `--survey FILE`: the user was explicit, so nothing overrides it.
	Given(String),
	/// The survey this car cached the last time it was swept.
	Cached(std::path::PathBuf),
	/// Nothing to load. `cache` is where one would go, when the car said which
	/// car it is; `None` when it did not, in which case there is no per-car
	/// path to name and the advice has to be the `--out`/`--survey` pair.
	Missing { cache: Option<std::path::PathBuf> },
}

/// Decide which survey a run uses.
///
/// Split out from [`run`] because the precedence is the whole point and it is
/// two lines of it: a file the user named beats a file this tool wrote for
/// itself, always, and a cache that is not there is not an error.
fn choose_survey(given: Option<&str>, cache: Option<std::path::PathBuf>) -> SurveySource {
	if let Some(path) = given {
		return SurveySource::Given(path.to_string());
	}
	match cache {
		Some(path) if path.is_file() => SurveySource::Cached(path),
		cache => SurveySource::Missing { cache },
	}
}

/// What answered, what of it can be shown, and what to do about the rest.
///
/// Printed before the screen takes over. The count and the list used to
/// disagree — eleven units counted, three listed — because the count came from
/// what identified itself and the list from what had catalog rows. They are one
/// set here, and the units with nothing to show are a line of their own, since
/// "the tool cannot name this unit's identifiers" and "the tool did not find
/// this unit" look identical on screen and are not the same problem.
/// The control units this car has that nothing on this machine describes.
///
/// **The one definition of "covered", and it is per unit rather than per car.**
/// A project describes an ECU variant, and a car is a set of variants: on the
/// reference car's fifteen units an installed project reaches thirteen. Asking
/// "does a project cover this car" of the whole vehicle would answer no for a
/// car that is thirteen-fifteenths described, and then advise a sweep of all
/// fifteen — twelve of which the project already declares in full.
///
/// It costs nothing to ask. Every input is already in hand: `channels` is what
/// [`crate::plan::available`] resolved out of the project and the catalogs,
/// plus whatever survey was loaded, and `identities` is what the gateway walk
/// already read. Nothing here goes back to the car.
fn silent_units(identities: &[crate::plan::UnitIdentity], channels: &[Channel]) -> Vec<u16> {
	identities
		.iter()
		.map(|i| i.request)
		.filter(|request| !channels.iter().any(|c| c.request == *request))
		.collect()
}

fn coverage_report(
	identities: &[crate::plan::UnitIdentity],
	channels: &[Channel],
	catalogs: &str,
	source: &SurveySource,
	answered: &crate::plan::Answered,
	project: Option<&str>,
) -> String {
	let list = |units: &[u16]| units.iter().map(|r| format!("{r:03X}")).collect::<Vec<_>>().join(" ");
	let units: Vec<u16> = identities.iter().map(|i| i.request).collect();
	let any = |request: u16, f: &dyn Fn(&Channel) -> bool| channels.iter().any(|c| c.request == request && f(c));
	// Three states, not two. A channel can be named and scaled and still never
	// have been confirmed on this car — an ODIS compu formula or an OBD-II
	// standard parameter — and calling that "proven" would report somebody
	// else's arithmetic as this project's measurement.
	let proven: Vec<u16> = units
		.iter()
		.copied()
		.filter(|r| any(*r, &|c: &Channel| c.def.is_some() && c.proven))
		.collect();
	let named: Vec<u16> = units
		.iter()
		.copied()
		.filter(|r| any(*r, &|c: &Channel| c.def.is_some() && !c.proven) && !proven.contains(r))
		.collect();
	let raw: Vec<u16> = units.iter().copied().filter(|r| any(*r, &|c: &Channel| c.def.is_none())).collect();
	let silent = silent_units(identities, channels);

	// "answered" would be a claim about *this* run, and with a cached survey
	// loaded it is not one: those units answered the sweep that wrote the
	// cache, and this run took its word for it rather than paying a probe per
	// unit again. What is true either way is that the car has them.
	let mut out = format!(
		"{} control {} on this car: {}\n",
		units.len(),
		crate::render::plural(units.len(), "unit"),
		list(&units)
	);
	if !proven.is_empty() {
		out.push_str(&format!("  measurements proven on a car: {}\n", list(&proven)));
	}
	if !named.is_empty() {
		// Named and scaled without a drive — the whole point of reading an ODIS
		// project — but said in words that do not claim more than that.
		out.push_str(&format!(
			"  named and scaled from this project's source data, not yet confirmed on a car: {}\n",
			list(&named)
		));
	}
	if proven.is_empty() && named.is_empty() && !units.is_empty() {
		// Not one unit of this car has a catalog. That is the ordinary state of
		// every car but the one this project was developed on, and it is worth
		// a paragraph rather than a silence: everything on the screen will be
		// hex, and the reason is that nobody has driven this car with the tool
		// recording yet.
		out.push_str(&crate::missing::no_catalog("This car", std::path::Path::new(catalogs)));
	}
	if !raw.is_empty() {
		let from = match source {
			SurveySource::Given(path) => path.clone(),
			SurveySource::Cached(path) => path.display().to_string(),
			// Nothing was loaded, so nothing can be raw-only; kept total
			// rather than reached-for so a later change cannot make it lie.
			SurveySource::Missing { .. } => "an earlier sweep".to_string(),
		};
		out.push_str(&format!("  raw identifiers from {from}: {}\n", list(&raw)));
		// Why they are raw, and what turns them into numbers. Without this the
		// screen is a wall of hex with no way to learn that it is fixable —
		// and the fix is a drive, not a `setup`, which is the distinction a
		// reader has no way to guess.
		let unproven = channels.iter().filter(|c| c.def.is_none()).count();
		for line in crate::missing::raw_channels_note(unproven).lines() {
			out.push_str(&format!("  {line}\n"));
		}
	}
	// How much of what the source data declares this particular car does not
	// have. Worth saying out loud rather than leaving as a shorter list: a
	// reader who is not told will count the rows, find fewer than `setup`
	// reported, and conclude the import was lost. It counts only channels a
	// survey actually put to the unit, so a car with no survey — or one whose
	// survey did not record its range — gets no sentence rather than a wrong one.
	let unanswered = channels.iter().filter(|c| answered.saw(c.request, c.did) == Some(false)).count();
	if unanswered > 0 {
		out.push_str(&format!(
			"  {unanswered} declared {} asked and answered nothing — held off the list, [u] shows them.\n",
			crate::render::plural(unanswered, "channel")
		));
	}
	if !silent.is_empty() {
		// Every line that carries a list or a path ends with it: these are as
		// long as the car makes them, and a hard wrap placed before one lands
		// in a different place on every car.
		out.push_str(&format!("nothing to show for {}\n", list(&silent)));
		// **Which shortage this is, in the words that pick the right fix.** A
		// project that is installed and does not reach these units is not the
		// same state as no project at all, and the two have different next
		// steps — one of them is `vagcan setup` and the other is not. What is
		// deliberately absent from both is an instruction to sweep: that is
		// offered below, as a question, because it is a fuzz test.
		out.push_str(&match project {
			Some(id) => format!("  — they answer, but neither the project {id} nor any catalog in {catalogs} \n    describes their identifiers.\n"),
			None => format!(
				"  — they answer, but no catalog in {catalogs} matches their part numbers, \n                     and no project is set up that might describe them.\n"
			),
		});
		if !matches!(source, SurveySource::Missing { .. }) {
			out.push_str("  The survey in use does not cover them either.\n");
		}
		let spec = silent.iter().map(|r| format!("{r:03X}")).collect::<Vec<_>>().join(",");
		out.push_str(&sweep_offer(&silent, project, &spec));
	}
	out
}

/// What sweeping the units nothing describes would be, said before it is asked.
///
/// **This is the one place `watch` still talks about a survey, and it is the
/// only place it is still true.** Everywhere else the channels come from what
/// the project declares, resolved off what the car reports about itself, with
/// no sweep in the picture at all. A unit no source describes is the exception:
/// there is nothing to look the identifiers up in, so the only way to learn
/// them is to ask for identifiers nothing declares — which is a fuzz test of
/// that unit's diagnostic server.
///
/// So the paragraph prices it rather than recommending it, and where a cheaper
/// answer exists it is named first: a machine with no project at all may be one
/// `vagcan setup` away from describing these units without asking the car
/// anything.
fn sweep_offer(silent: &[u16], project: Option<&str>, spec: &str) -> String {
	let n = silent.len();
	let mut out = format!(
		"\n{n} control {} on this car — {} — {} nothing on this machine describes.\n",
		crate::render::plural(n, "unit"),
		silent.iter().map(|r| format!("{r:03X}")).collect::<Vec<_>>().join(" "),
		if n == 1 { "is one" } else { "are ones" }
	);
	if project.is_none() {
		// The cheap answer first. A project describes a whole platform without
		// the car being asked anything, and somebody who sweeps instead spends
		// minutes of fuzzing to rediscover part of what a parse would have
		// handed them.
		out.push_str(
			"\nNo project is set up here. A VCDS installation or an extracted ODIS project \n             may describe them outright — `vagcan setup /path/to/VCDS` — and that asks the \n             car nothing at all. It is worth trying before anything below.\n",
		);
	}
	out.push_str(&format!(
		"\nThe other way is to ask those units identifiers nothing declares they answer:\n    \
         vagcan dev survey --only {spec} --blind {spec}\n\n\
         That is a fuzz test of their diagnostic servers and the most invasive thing this \n         tool does. Each request takes a path through firmware that may never have been \n         exercised, and a path with a defect in it crashes the server, which on a control \n         unit the car is relying on is not a small event. It is refused outright while the \n         car is moving, and it takes a few minutes per unit.\n\n\
         What it buys: the answers are filed under this car, and every later `watch` puts \n         those identifiers on offer as raw bytes with no flag.\n"
	));
	out
}

/// How the values are shown.
///
/// Two modes rather than one flag, because "which view" and "for how long" are
/// separate questions and encoding them in a single `Option<f64>` needs a
/// sentinel for "plain, indefinitely" — the sentinel was infinity, and
/// `Duration::from_secs_f64` panics on it.
pub enum View {
	/// The full-screen view. Needs a terminal.
	FullScreen,
	/// CSV, no screen and no keyboard. `None` runs until interrupted.
	Plain(Option<Duration>),
}

pub struct Options<'a> {
	pub preselect: &'a [(u16, u16)],
	pub hz: f64,
	pub out: Option<&'a str>,
	pub survey: Option<&'a str>,
	pub catalogs: &'a str,
	pub view: View,
}

pub async fn run(device_path: &str, baud: u32, opts: Options<'_>) -> Result<()> {
	let Options {
		preselect,
		hz,
		out,
		survey,
		catalogs,
		view,
	} = opts;
	use std::io::Write as _;
	use vag_uds_can::{SlcanBackend, SlcanBitrate, SlcanMode};

	// Argument checking first: the adapter is a single-user resource, and
	// holding it open while failing on a typo blocks the next attempt. That
	// means the recording is created here too, not once the car is answering —
	// an unwritable --out path is the same typo as an unreadable --survey one.
	let store = vag_data_labels::catalog::CatalogStore::open(catalogs);
	// Named surveys are read here, before the adapter: an unreadable one is a
	// typo, and a typo should not cost the port. The car's own cache cannot be
	// — it is found by VIN, and the VIN comes off the car.
	let given_text = match survey {
		Some(path) => Some(std::fs::read_to_string(path).with_context(|| format!("reading the survey {path:?}"))?),
		None => None,
	};
	let mut sink = match out {
		Some(path) => {
			let file = std::fs::File::create(path).with_context(|| format!("creating {path:?}"))?;
			Some(std::io::BufWriter::new(file))
		}
		None => None,
	};

	let mut adapter = SlcanBackend::open_mode(device_path, baud, SlcanBitrate::Rate500k, SlcanMode::Normal)
		.await
		.with_context(|| crate::device::open_failure(device_path))?;

	// Which car this is, so its own survey can be found. One identifier read,
	// and a car that will not say simply has no cache — everything below still
	// works, with the catalogs alone.
	let mut progress = crate::progress::Line::new();
	progress.update("reading the vehicle identification number");
	let (back, vin) = crate::units::read_vin(adapter).await;
	adapter = back;

	// The whole reason a car keeps a survey: without one, the twelve units no
	// catalog covers have nothing on screen, and the only way to see them was
	// to remember the file name of a sweep run some other day. The cache is
	// loaded with no flag; `--survey` still overrides it.
	let source = choose_survey(survey, vin.as_deref().and_then(|vin| crate::datadir::survey_cache(vin).ok()));
	let survey_text = match &source {
		// Already read above, before the port was taken.
		SurveySource::Given(_) => given_text,
		SurveySource::Cached(path) => Some(std::fs::read_to_string(path).with_context(|| format!("reading the survey {}", path.display()))?),
		SurveySource::Missing { .. } => None,
	};

	// Which scalings apply is decided by what each unit says it is, never by
	// its address. A survey already asked; without one, the units to be polled
	// are asked directly below.
	let mut identities = match &survey_text {
		Some(text) => crate::plan::identities_from_survey(text),
		None => Vec::new(),
	};

	// Which units the car has, and what each of them is. Without this the view
	// would only ever show the engine, because a unit with no identity
	// contributes no channels and so no tab — which is what "switching between
	// units does nothing" looked like. The walk lives in `crate::units`,
	// because `measure` makes the same one.
	let mut wanted: Vec<u16> = preselect.iter().map(|(request, _)| *request).collect();
	wanted.push(crate::plan::ENGINE);
	// A survey already asked every unit it visited for its identification
	// block, so those are not asked again; everything else still is.
	let (back, found) = crate::units::identify(adapter, &wanted, &identities, &mut progress).await;
	adapter = back;
	identities.extend(found);

	progress.finish();
	// Bound rather than built twice: the coverage line names the project these
	// very rows came out of, and a second `current()` could answer differently.
	let extracted = crate::extracted::current();
	let mut channels = crate::plan::available(&store, &extracted, &identities);
	// What the car was seen to answer, so the selection screen can hold back the
	// channels this project declares and this vehicle does not have. Empty
	// without a survey, and nothing is then filtered on those grounds.
	let mut answered = crate::plan::Answered::default();
	if let Some(text) = &survey_text {
		// Everything a survey found becomes watchable, on every unit — which
		// is the only way the units outside the catalogs get on screen at all.
		channels = crate::plan::with_survey(channels, text);
		answered = crate::plan::answered_from_survey(text);
	}
	// Say what the car has and what of it can be shown, before the screen takes
	// over. A unit that identified itself but has no catalog contributes no
	// measurements and so no tab — which looks like the tool failing to find
	// it, and is worth distinguishing from that. Reported after the survey is
	// folded in, or it would describe a screen nobody is about to see.
	//
	// On stderr, because in the plain-console view stdout is the CSV: a
	// paragraph of prose in front of the header is not something a reader of
	// that stream can be asked to skip. It is still the terminal either way.
	eprint!(
		"{}",
		coverage_report(&identities, &channels, catalogs, &source, &answered, extracted.project())
	);
	for (request, did) in preselect {
		// **Every** field of that identifier, not the first one. `--did
		// 01:2029` names an identifier, one request reads all of it, and
		// selecting one field of the answer while dropping the others would
		// cost nothing on the bus and lose channels on the screen.
		let mut found = false;
		for channel in channels.iter_mut().filter(|c| c.request == *request && c.did == *did) {
			channel.selected = true;
			found = true;
		}
		if !found {
			channels.push(Channel {
				request: *request,
				did: *did,
				def: None,
				named: None,
				proven: false,
				text_id: None,
				selected: true,
			});
		}
	}
	let mut backend = Some(adapter);
	let mut header_written = false;

	let mut app = App::new(channels);
	app.answered = answered;
	{
		// The settings this screen can change, read once before it appears.
		let settings = crate::config::load();
		app.hz = crate::config::hz(&settings);
		app.show_key = crate::config::show_key(&settings);
	}
	// The flag wins for one run; the setting is what the screen changes and
	// what the next run starts from. `--hz` left at its own default means
	// nobody asked for a rate, so the setting stands.
	if (hz - crate::config::DEFAULT_HZ).abs() > f64::EPSILON {
		app.hz = hz.clamp(crate::config::MIN_HZ, crate::config::MAX_HZ);
	}
	// This car's own marks, if it has any: the handful somebody watches every
	// drive, ticked before the screen appears. They come after `--did`, which
	// is a person being explicit about this one run, and before the basics,
	// which are only a guess at what anybody wants.
	app.favourites_vin = vin.clone();
	app.favourites = favourites::load(app.favourites_vin.as_deref());
	// The chart's lines are the same kind of choice as a favourite and are kept
	// the same way: per car, because which identifiers a unit answers is a fact
	// about the unit as installed in this one.
	app.charted = favourites::from_list(&crate::config::charted(&crate::config::load(), vin.as_deref().unwrap_or_default()))
		.into_iter()
		.collect();
	let favourites = app.select_favourites();
	if favourites > 0 {
		eprintln!(
			"{favourites} of your {} favourites are on this car — press `f` on the selection screen to change them",
			app.favourites.len()
		);
	}
	if !app.channels.iter().any(|c| c.selected) {
		crate::plan::select_basics(&mut app.channels);
	}
	app.open_first_populated();
	app.units = unit_names(&identities);
	// Read from the app each cycle rather than computed once: the settings
	// screen changes it while the car is answering, and a period captured at
	// startup would leave the new rate showing and the old one polling.
	let period = |app: &App| Duration::from_secs_f64(1.0 / app.hz.max(crate::config::MIN_HZ));

	// No terminal wanted: a script, a pipe, or an agent that cannot press a
	// key. Same poll loop, no drawing and no input — and with no `--out` the
	// samples go to stdout, so they can be read directly instead of through a
	// file nobody asked for.
	if let View::Plain(duration) = view {
		let mut sink: Box<dyn std::io::Write> = match sink {
			Some(file) => Box::new(file),
			None => Box::new(io::stdout().lock()),
		};
		// `checked_add`, not `+`: a duration a caller is free to name can be
		// large enough to overflow the instant, and panicking there would do it
		// with the adapter open and the car on the bus. Too far away to reach
		// is the same as no deadline at all.
		let deadline = duration.and_then(|d| Instant::now().checked_add(d));
		while deadline.is_none_or(|d| Instant::now() < d) {
			let cycle = Instant::now();
			for batch in crate::plan::plan(&app.channels) {
				poll_batch(&mut app, &mut backend, &batch).await;
			}
			app.cycles += 1;
			write_row(&mut sink, &app, &mut header_written)?;
			// Flushed every cycle: a reader watching the pipe should see the
			// samples as they happen, not in one burst when the run ends.
			sink.flush()?;
			if let Some(rest) = period(&app).checked_sub(cycle.elapsed()) {
				tokio::time::sleep(rest).await;
			}
		}
		eprintln!("{} cycles over {:.1} s", app.cycles, app.started.elapsed().as_secs_f64());
		return Ok(());
	}

	// A full-screen view needs a terminal; without one crossterm fails with a
	// bare errno that says nothing about why.
	let screen = term::full_screen().with_mouse().enter().map_err(|e| {
		anyhow::anyhow!(
			"`watch` needs an interactive terminal (it draws a full-screen view). \
             Without one, use `--for SECONDS`, which polls the same channels and \
             writes CSV: {e}"
		)
	})?;
	let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
	let result = loop {
		terminal.draw(|f| match app.screen {
			Screen::Live => draw_live(f, &mut app),
			Screen::Select => draw_select(f, &mut app),
			Screen::Series => draw_series(f, &mut app),
			Screen::Settings => draw_settings(f, &mut app),
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
		for batch in crate::plan::plan(&app.channels) {
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
			// Redraw before the request, so the footer says which unit is
			// being waited on. A batch can take as long as that unit's
			// deadline, and a still screen during it reads as a hang.
			app.waiting = app.slow.contains(&batch.request).then_some(batch.request);
			terminal.draw(|f| match app.screen {
				Screen::Live => draw_live(f, &mut app),
				Screen::Select => draw_select(f, &mut app),
				Screen::Series => draw_series(f, &mut app),
				Screen::Settings => draw_settings(f, &mut app),
			})?;
			let asked = Instant::now();
			poll_batch(&mut app, &mut backend, &batch).await;
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
			write_row(w, &app, &mut header_written)?;
		}

		// A key pressed during the poll should not wait a whole cycle.
		let mut quit = false;
		while let Some(rest) = period(&app).checked_sub(cycle.elapsed()) {
			if !event::poll(rest.min(Duration::from_millis(50)))? {
				if cycle.elapsed() >= period(&app) {
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

	// Off the alternate screen before the summary is printed, for the same
	// reason as in the replay path above.
	drop(terminal);
	drop(screen);
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

	/// The reference car's own proven rows, when this machine has any.
	///
	/// They used to be committed under `catalogs/vehicles/` and are now one
	/// owner's measured data under `~/.vagcan/data/<id>/measurements`, like
	/// everybody
	/// else's — nothing measured on a vehicle lives in the checkout any more.
	/// So a machine that has never calibrated a car has nothing to assert
	/// against, and these tests say so rather than failing over data they were
	/// never entitled to assume.
	fn measured_rows() -> Option<std::path::PathBuf> {
		let dir = crate::project::current().ok()?.measurements_dir();
		let any = std::fs::read_dir(&dir)
			.ok()?
			.flatten()
			.any(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"));
		any.then_some(dir)
	}

	/// Give up on a test that needs rows this machine has not got.
	macro_rules! need_rows {
		() => {
			match measured_rows() {
				Some(dir) => dir,
				None => {
					eprintln!(
						"skipped: no proven rows in this machine's project — \
                         drive and calibrate a car to get some"
					);
					return;
				}
			}
		};
	}

	/// The reference car's rows and identities — a fixture, not a table the
	/// code carries.
	fn reference_channels(dir: std::path::PathBuf) -> Vec<Channel> {
		let store = vag_data_labels::catalog::CatalogStore::open(dir);
		let ident = |request, part: &str| crate::plan::UnitIdentity {
			request,
			part_number: Some(part.to_string()),
			odx_name: None,
			odx_version: None,
			component: None,
		};
		crate::plan::available(
			&store,
			&crate::extracted::Extracted::none(),
			&[ident(0x7E0, "8V0906264H"), ident(0x7E1, "0CW300041G"), ident(0x714, "5E0920740D")],
		)
	}

	/// Open the tab that holds `request`, so a test about rows is not really
	/// a test about which tab happens to be first.
	fn open(app: &mut App, request: u16) {
		app.tab = app.tabs().iter().position(|r| *r == request).expect("the unit has a tab");
	}

	fn app(dir: std::path::PathBuf) -> App {
		let mut channels = reference_channels(dir);
		channels[0].selected = true;
		channels[1].selected = true;
		App::new(channels)
	}

	/// The reference car's fifteen units, as `vagcan units` lists them — a
	/// fixture, not a table the code carries. Only three have catalogs.
	fn reference_identities() -> Vec<crate::plan::UnitIdentity> {
		let ident = |request, part: Option<&str>| crate::plan::UnitIdentity {
			request,
			part_number: part.map(str::to_string),
			odx_name: None,
			odx_version: None,
			component: None,
		};
		vec![
			ident(0x700, None),
			ident(0x70A, None),
			ident(0x70C, None),
			ident(0x70E, None),
			ident(0x712, None),
			ident(0x713, None),
			ident(0x714, Some("5E0920740D")),
			ident(0x715, None),
			ident(0x746, None),
			ident(0x74A, None),
			ident(0x74B, None),
			ident(0x767, None),
			ident(0x773, None),
			ident(0x7E0, Some("8V0906264H")),
			ident(0x7E1, Some("0CW300041G")),
		]
	}

	fn store(dir: std::path::PathBuf) -> vag_data_labels::catalog::CatalogStore {
		vag_data_labels::catalog::CatalogStore::open(dir)
	}

	#[test]
	fn the_count_and_the_list_name_the_same_control_units() {
		// The reported defect: "11 control units answered: 714 7E0 7E1" — a
		// count of everything that answered set against a list of the three
		// with catalogs. Whatever else the summary says, those two must agree.
		let identities = reference_identities();
		let channels = crate::plan::available(&store(need_rows!()), &crate::extracted::Extracted::none(), &identities);
		let text = coverage_report(
			&identities,
			&channels,
			"catalogs/vehicles",
			&SurveySource::Missing { cache: None },
			&crate::plan::Answered::default(),
			None,
		);
		let first = text.lines().next().unwrap().to_string();
		let (count, listed) = first.split_once(':').unwrap();
		let count: usize = count.split_whitespace().next().unwrap().parse().unwrap();
		assert_eq!(count, identities.len());
		assert_eq!(listed.split_whitespace().count(), count, "{first}");
	}

	#[test]
	fn without_a_survey_the_summary_names_the_one_command_that_fixes_it() {
		// A unit with no catalog and no survey has nothing on screen at all,
		// and the tool has to say which single command changes that.
		let identities = reference_identities();
		let channels = crate::plan::available(&store(need_rows!()), &crate::extracted::Extracted::none(), &identities);
		let text = coverage_report(
			&identities,
			&channels,
			"catalogs/vehicles",
			&SurveySource::Missing {
				cache: Some(std::path::PathBuf::from("/somewhere/survey.jsonl")),
			},
			&crate::plan::Answered::default(),
			None,
		);
		assert!(text.contains("vagcan dev survey"), "{text}");
		assert!(text.contains("713"), "the unit with nothing to show is named: {text}");
		// The identifiers a survey would offer are raw bytes and are said to
		// be — this project does not invent a scaling for them.
		assert!(text.contains("raw"), "{text}");
	}

	#[test]
	fn a_cached_survey_puts_every_unit_on_offer_and_the_summary_says_where_from() {
		// The point of the cache: with one on disk, no unit is left with
		// nothing to show and no flag was needed to get there.
		let identities = reference_identities();
		// A sweep answers on every unit, including the twelve no catalog
		// covers — one identifier each is enough to make the point.
		let survey: String = identities
			.iter()
			.map(|i| {
				format!(
					"{{\"request\":\"{:03X}\",\"dids\":[{{\"did\":\"1001\",\"data\":\"0224\"}}]}}\n",
					i.request
				)
			})
			.collect();
		let channels = crate::plan::with_survey(
			crate::plan::available(&store(need_rows!()), &crate::extracted::Extracted::none(), &identities),
			&survey,
		);
		assert!(channels.iter().any(|c| c.request == 0x713), "the sweep's units are watchable");
		let cache = std::path::PathBuf::from("/somewhere/survey.jsonl");
		let text = coverage_report(
			&identities,
			&channels,
			"catalogs/vehicles",
			&SurveySource::Cached(cache),
			&crate::plan::Answered::default(),
			None,
		);
		assert!(text.contains("survey.jsonl"), "{text}");
		assert!(text.contains("713"), "{text}");
		// No unit is left with nothing on screen, so there is nothing to
		// advise about and no advice.
		assert!(!text.contains("nothing to show"), "{text}");
		assert!(!text.contains("vagcan dev survey\n"), "{text}");
	}

	#[test]
	fn a_screen_of_hex_says_why_it_is_hex_and_what_turns_it_into_numbers() {
		// The reported gap: twelve of fifteen units show raw bytes, the tool
		// tags each value `(raw)`, and nothing anywhere says the scaling is
		// missing rather than the car being odd — let alone that a drive fixes
		// it. Said once, in the summary, not per row: this is read at an open
		// driver's door.
		let identities = reference_identities();
		let survey: String = identities
			.iter()
			.map(|i| {
				format!(
					"{{\"request\":\"{:03X}\",\"dids\":[{{\"did\":\"1001\",\"data\":\"0224\"}}]}}\n",
					i.request
				)
			})
			.collect();
		let channels = crate::plan::with_survey(
			crate::plan::available(&store(need_rows!()), &crate::extracted::Extracted::none(), &identities),
			&survey,
		);
		let text = coverage_report(
			&identities,
			&channels,
			"/x/data",
			&SurveySource::Cached(std::path::PathBuf::from("/somewhere/survey.jsonl")),
			&crate::plan::Answered::default(),
			None,
		);
		assert!(text.contains("no proven scaling for this car yet"), "{text}");
		assert!(text.contains("recording calibrate"), "{text}");
		// And never the other shortage's fix as an instruction: a scaling is
		// not in any label files, so pointing at `setup` here sends a reader
		// nowhere.
		assert!(!text.contains("vagcan setup /path"), "{text}");
	}

	#[test]
	fn the_report_accounts_for_the_declared_channels_this_car_does_not_have() {
		// Otherwise the filter looks like a loss. `setup` says it imported
		// hundreds of thousands of channels, the list shows a fraction, and
		// nothing on screen connects the two — so the import reads as broken
		// when what actually happened is that this car is not that project.
		let identities = reference_identities();
		let channels = crate::plan::available(&store(need_rows!()), &crate::extracted::Extracted::none(), &identities);
		let one = channels.first().expect("the reference store has channels").clone();
		// One channel asked and answered, and the rest of that unit asked and
		// silent.
		let survey = format!(
			"{{\"request\":\"{:03X}\",\"dids\":[{{\"did\":\"{:04X}\",\"data\":\"00\"}}]}}\n",
			one.request, one.did
		);
		let answered = crate::plan::answered_from_survey(&survey);
		let quiet = channels.iter().filter(|c| answered.saw(c.request, c.did) == Some(false)).count();
		assert!(quiet > 0, "the fixture has to have something to hold back");

		let text = coverage_report(
			&identities,
			&channels,
			"catalogs/vehicles",
			&SurveySource::Cached(std::path::PathBuf::from("/somewhere/survey.jsonl")),
			&answered,
			None,
		);
		assert!(text.contains(&format!("{quiet} declared")), "{text}");
		assert!(text.contains("answered nothing"), "{text}");
		assert!(text.contains("[u] shows them"), "the key that undoes it is on the same page: {text}");

		// And nothing of the sort is claimed when no survey was loaded: the
		// sentence is about identifiers somebody asked for.
		let text = coverage_report(
			&identities,
			&channels,
			"catalogs/vehicles",
			&SurveySource::Missing { cache: None },
			&crate::plan::Answered::default(),
			None,
		);
		assert!(!text.contains("answered nothing"), "{text}");
	}

	#[test]
	fn a_car_with_no_catalog_at_all_is_told_what_makes_one() {
		// The screen is about to be entirely hex, and silence would read as the
		// car being unusual rather than as a step nobody has taken yet.
		//
		// The units here are deliberately outside `0x7E0..0x7E7`: a car whose
		// engine sits in the ISO block always has the standard OBD-II rows,
		// which are SAE J1979's rather than anybody's measurement, so `proven`
		// is not empty there however uncalibrated the car is.
		let ident = |request| crate::plan::UnitIdentity {
			request,
			part_number: Some(format!("{request:03X}0000000")),
			odx_name: None,
			odx_version: None,
			component: None,
		};
		let identities = vec![ident(0x714), ident(0x713), ident(0x70C)];
		let empty = vag_data_labels::catalog::CatalogStore::open("/definitely/not/here");
		let channels = crate::plan::available(&empty, &crate::extracted::Extracted::none(), &identities);
		let text = coverage_report(
			&identities,
			&channels,
			"/x/data/measured",
			&SurveySource::Missing { cache: None },
			&crate::plan::Answered::default(),
			None,
		);
		assert!(text.contains("no proven measurement rows"), "{text}");
		assert!(text.contains("/x/data/measured"), "{text}");
		assert!(text.contains("vagcan dev recording calibrate"), "{text}");
		assert!(text.contains("not something `vagcan setup` can fix"), "{text}");
	}

	#[test]
	fn a_named_survey_wins_over_the_one_this_car_cached() {
		// `--survey FILE` is the user being explicit; a cache must never
		// silently override it.
		let dir = std::env::temp_dir().join(format!("vagcan-watch-survey-{}-{:?}", std::process::id(), std::thread::current().id()));
		std::fs::create_dir_all(&dir).unwrap();
		let cache = dir.join("survey.jsonl");
		std::fs::write(&cache, "{}\n").unwrap();

		assert!(matches!(
				choose_survey(Some("named.jsonl"), Some(cache.clone())),
				SurveySource::Given(ref path) if path == "named.jsonl"
		));
		assert!(matches!(
				choose_survey(None, Some(cache.clone())),
				SurveySource::Cached(ref path) if *path == cache
		));
		// A car that has never been swept, and a car that would not say which
		// car it is, both come out as nothing to load.
		std::fs::remove_file(&cache).unwrap();
		assert!(matches!(
			choose_survey(None, Some(cache.clone())),
			SurveySource::Missing { cache: Some(_) }
		));
		assert!(matches!(choose_survey(None, None), SurveySource::Missing { cache: None }));
		let _ = std::fs::remove_dir_all(&dir);
	}

	#[test]
	fn the_configure_key_switches_screens_and_q_stops() {
		let mut a = app(need_rows!());
		assert_eq!(a.screen, Screen::Live);
		assert!(on_key(&mut a, KeyCode::Char('c')));
		assert_eq!(a.screen, Screen::Select);
		assert!(on_key(&mut a, KeyCode::Enter));
		assert_eq!(a.screen, Screen::Live);
		assert!(!on_key(&mut a, KeyCode::Char('q')));
	}

	#[test]
	fn toggling_changes_what_is_polled_without_a_restart() {
		let mut a = app(need_rows!());
		let before = crate::plan::plan(&a.channels).len();
		a.screen = Screen::Select;
		open(&mut a, 0x7E0);
		a.cursor = a.visible()[5];
		let at = a.cursor;
		on_key(&mut a, KeyCode::Char(' '));
		assert!(a.channels[at].selected);
		// Selecting more can only add work, never remove it.
		assert!(crate::plan::plan(&a.channels).len() >= before);

		// `a` and `n` act on the open tab, so clearing every tab clears the
		// plan — and polling never follows the tab, only the selection.
		for _ in 0..a.tabs().len() {
			on_key(&mut a, KeyCode::Char('n'));
			step_tab(&mut a, true);
		}
		assert!(crate::plan::plan(&a.channels).is_empty(), "none selected polls nothing");
		on_key(&mut a, KeyCode::Char('a'));
		assert!(!crate::plan::plan(&a.channels).is_empty());
	}

	#[test]
	fn the_cursor_stays_inside_the_list() {
		let mut a = app(need_rows!());
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
		let mut a = app(need_rows!());
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
		let mut a = app(need_rows!());
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
		let hidden_selected = a.channels.iter().enumerate().filter(|(i, c)| c.selected && !visible.contains(i)).count();
		assert_eq!(hidden_selected, 2, "only the two the fixture pre-selected");
	}

	#[test]
	fn typing_a_filter_does_not_trigger_the_command_keys() {
		// `n` clears the selection; typing "engine" must not.
		let mut a = app(need_rows!());
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
		let mut a = App::new(reference_channels(need_rows!()));
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
		let mut a = App::new(reference_channels(need_rows!()));
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
		let mut a = App::new(reference_channels(need_rows!()));
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
		let mut a = App::new(reference_channels(need_rows!()));
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
		let mut a = App::new(reference_channels(need_rows!()));
		assert!(a.slow.is_empty());
		assert_eq!(a.slow.contains(&0x7E0).then_some(0x7E0), None);
		a.slow.insert(0x7E0);
		assert_eq!(a.slow.contains(&0x7E0).then_some(0x7E0), Some(0x7E0));
	}

	#[test]
	fn the_live_table_says_which_unit_each_group_came_from() {
		// Values from several units on one screen are unreadable without it.
		let mut a = App::new(reference_channels(need_rows!()));
		a.units = vec![(0x7E0, "1.8l R4 TFSI".to_string())];
		assert_eq!(a.unit_heading(0x7E0), "01 1.8l R4 TFSI");
		assert_eq!(a.unit_heading(0x714), "17", "a unit that said nothing is not named");
	}

	#[test]
	fn the_view_opens_on_a_unit_that_has_something_to_show() {
		// Tabs are in id order and the lowest id is rarely the interesting
		// one; opening there shows an empty table at the moment a person
		// first sees the tool.
		let mut a = App::new(reference_channels(need_rows!()));
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
		let mut a = App::new(reference_channels(need_rows!()));
		a.units = vec![(0x7E0, "1.8l R4 TFSI".to_string())];
		assert_eq!(a.tab_label(0x7E0), "01 1.8l R4 TFSI");
		// A unit that said nothing goes by its number, not by an invented name.
		assert_eq!(a.tab_label(0x714), "17");
		assert_eq!(a.tab_label(0x713), "713");
	}

	#[test]
	fn the_cursor_moves_with_the_tab_it_belongs_to() {
		// Leaving it behind makes the arrow keys walk a row nobody can see.
		let mut a = App::new(reference_channels(need_rows!()));
		a.screen = Screen::Select;
		step_tab(&mut a, true);
		let visible = a.visible();
		assert!(visible.contains(&a.cursor), "{:?} not in the open tab", a.cursor);
	}

	/// A channel with a scaling this project proved, for the chart tests. A
	/// fixture: the numbers are a linear scale and a unit string, not a row
	/// copied out of one car's catalog.
	fn proven(request: u16, did: u16, name: &'static str, unit: &'static str) -> Channel {
		use std::borrow::Cow;
		use vag_data_labels::catalog::{ReadId, Scaling};
		use vag_data_labels::measure::{LinearScale, RawForm};
		Channel {
			request,
			did,
			def: Some(vag_data_labels::catalog::MeasurementDef {
				name: Cow::Borrowed(name),
				unit: Cow::Borrowed(unit),
				address: ReadId::Uds(did),
				raw_form: RawForm::U16Be,
				scaling: Scaling::Linear(LinearScale { factor: 0.001, offset: 0.0 }),
			}),
			named: None,
			proven: true,
			text_id: None,
			selected: true,
		}
	}

	/// A state — a gear, a selector lever. It has a definition and it has no
	/// number: the codes are neither ordered nor contiguous.
	fn state(request: u16, did: u16, name: &'static str) -> Channel {
		use std::borrow::Cow;
		use vag_data_labels::catalog::{ReadId, Scaling};
		use vag_data_labels::measure::RawForm;
		Channel {
			request,
			did,
			def: Some(vag_data_labels::catalog::MeasurementDef {
				name: Cow::Borrowed(name),
				unit: Cow::Borrowed(""),
				address: ReadId::Uds(did),
				raw_form: RawForm::U8First,
				scaling: Scaling::Enum {
					levels: vec![(5, "4".to_string()), (12, "R".to_string())],
				},
			}),
			named: None,
			proven: true,
			text_id: None,
			selected: true,
		}
	}

	fn raw(request: u16, did: u16) -> Channel {
		Channel {
			request,
			did,
			def: None,
			named: None,
			proven: false,
			text_id: None,
			selected: true,
		}
	}

	/// The live screen as a person sees it.
	fn live_text(app: &mut App, w: u16, h: u16) -> String {
		use ratatui::Terminal;
		use ratatui::backend::TestBackend;
		let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
		terminal.draw(|frame| draw_live(frame, app)).unwrap();
		let buffer = terminal.backend().buffer().clone();
		(0..h)
			.map(|y| (0..w).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>())
			.collect::<Vec<_>>()
			.join("\n")
	}

	fn select_text(app: &mut App, w: u16, h: u16) -> String {
		use ratatui::Terminal;
		use ratatui::backend::TestBackend;
		let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
		terminal.draw(|frame| draw_select(frame, app)).unwrap();
		let buffer = terminal.backend().buffer().clone();
		(0..h)
			.map(|y| (0..w).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>())
			.collect::<Vec<_>>()
			.join("\n")
	}

	/// The live screen with a chart on it, at every size a terminal is still
	/// allowed to be.
	#[test]
	#[ignore = "not an assertion — prints the screen so a person can read it"]
	fn show() {
		for (w, h) in [(120u16, 40u16), (100, 30), (80, 24)] {
			let mut a = App::new(vec![
				proven(0x7E0, 0x202A, "Boost pressure", "bar"),
				proven(0x7E0, 0x206E, "Engine speed", "/min"),
				raw(0x7E0, 0x38F0),
			]);
			a.units = vec![(0x7E0, "1.8l R4 TFSI".to_string())];
			for (i, did) in [0x202Au16, 0x206E].iter().enumerate() {
				a.charted.insert((0x7E0, *did, 0));
				for step in 0..40u16 {
					let value = 1000 + step * 40 * (i as u16 + 1);
					a.observe(0x7E0, *did, step as f64 * 0.25, value.to_be_bytes().to_vec());
				}
			}
			a.charted.insert((0x7E0, 0x38F0, 0));
			a.observe(0x7E0, 0x38F0, 9.75, vec![0x0B, 0x34]);
			a.clock = 9.75;
			a.chart_shown = true;
			println!("\n=== {w}×{h} ===");
			println!("{}", live_text(&mut a, w, h));
			a.screen = Screen::Select;
			println!("{}", select_text(&mut a, w, h));
		}
	}

	#[test]
	fn a_channel_with_no_proven_scaling_stays_raw_and_unplotted() {
		// The whole of `watch`'s reason for existing is to show the identifiers
		// nobody has proven yet, so the chart has to be able to say no to one.
		// There is no float in a raw channel — there is a byte string — and
		// inventing one is a mistake this project has caught itself at before.
		let mut a = App::new(vec![proven(0x7E0, 0x202A, "Boost pressure", "bar"), raw(0x7E0, 0x38F0)]);
		a.observe(0x7E0, 0x202A, 1.0, vec![0x03, 0xE8]);
		a.observe(0x7E0, 0x38F0, 1.0, vec![0x0B, 0x34]);
		assert_eq!(a.history.points((0x7E0, 0x202A, 0)), vec![(1.0, 1.0)]);
		assert!(a.history.points((0x7E0, 0x38F0, 0)).is_empty());
		// The table still shows it, because that is what `watch` is for.
		assert_eq!(a.latest.get(&(0x7E0, 0x38F0)).map(|(_, d)| d.clone()), Some(vec![0x0B, 0x34]));

		// A state has a definition and still no number: the gear codes are
		// neither ordered nor contiguous, so a line through them would draw the
		// encoding rather than the car.
		let mut a = App::new(vec![state(0x7E1, 0x3816, "Selected gear")]);
		a.observe(0x7E1, 0x3816, 1.0, vec![0x05]);
		assert!(a.history.points((0x7E1, 0x3816, 0)).is_empty());
	}

	#[test]
	fn marking_a_channel_for_the_chart_also_puts_it_on_the_poll_plan() {
		// A line that is charted but not polled can never have a point in it,
		// and would be a name in the key with nothing under it forever.
		let mut a = App::new(vec![Channel {
			text_id: None,
			selected: false,
			..proven(0x7E0, 0x202A, "Boost pressure", "bar")
		}]);
		a.screen = Screen::Select;
		a.cursor = 0;
		assert!(crate::plan::plan(&a.channels).is_empty());
		on_key(&mut a, KeyCode::Char('g'));
		assert!(a.charted.contains(&(0x7E0, 0x202A, 0)));
		assert!(a.channels[0].selected, "marking for the chart selects it");
		assert!(!crate::plan::plan(&a.channels).is_empty());

		// And unselecting it takes the chart mark with it, rather than leaving
		// a line the loop has stopped feeding.
		on_key(&mut a, KeyCode::Char(' '));
		assert!(!a.channels[0].selected);
		assert!(a.charted.is_empty(), "{:?}", a.charted);
	}

	#[test]
	fn the_chart_draws_what_was_marked_and_says_what_it_would_not_draw() {
		// A screen that drops a series has to say it dropped it, and the two
		// reasons it can drop one are different sentences: too many marked, and
		// marked with nothing to plot.
		let mut channels: Vec<Channel> = (0..8).map(|i| proven(0x7E0, 0x2000 + i, "boost", "bar")).collect();
		channels.push(raw(0x7E0, 0x38F0));
		let mut a = App::new(channels);
		for c in &a.channels {
			a.charted.insert(c.key());
		}
		let charted = a.charted();
		assert_eq!(charted.series.len(), CHART_CHANNELS);
		assert_eq!(charted.over_cap, 8 - CHART_CHANNELS);
		assert_eq!(charted.no_number, 1);

		// Nothing unmarked is on it, whatever else is selected.
		let mut a = App::new(vec![
			proven(0x7E0, 0x2029, "Boost pressure, specified", "bar"),
			proven(0x7E0, 0x202A, "Boost pressure, actual", "bar"),
		]);
		a.charted.insert((0x7E0, 0x202A, 0));
		let charted = a.charted();
		assert_eq!(charted.series.len(), 1);
		assert_eq!(charted.series[0].label, "Boost pressure, actual");
		assert_eq!(charted.series[0].unit, "bar");
		// Everything `watch` draws came off the bus; it computes nothing.
		assert_eq!(charted.series[0].origin, crate::ui::chart::Origin::Bus);
	}

	#[test]
	fn a_line_from_a_second_control_unit_says_which_unit_it_came_from() {
		// Two units publish an engine speed and a shaft speed under names that
		// can read alike, and a key that names two lines the same way is a key
		// that explains nothing.
		let mut a = App::new(vec![
			proven(0x7E0, 0x206E, "Engine speed", "/min"),
			proven(0x7E1, 0x380A, "Engine speed", "/min"),
		]);
		a.charted.insert((0x7E0, 0x206E, 0));
		let one = a.charted();
		assert_eq!(one.series[0].label, "Engine speed", "one unit needs no prefix");
		a.charted.insert((0x7E1, 0x380A, 0));
		let two = a.charted();
		assert_eq!(two.series[0].label, "01 Engine speed");
		assert_eq!(two.series[1].label, "02 Engine speed");
	}

	#[test]
	fn opening_the_chart_with_nothing_marked_seeds_it_from_what_is_on_screen() {
		// The one key that opens the chart must not open an empty one: a
		// feature whose first press shows nothing teaches nobody it exists.
		// What it seeds from is what is already on the screen, in the order the
		// table shows it, and no more than the chart draws.
		let mut channels: Vec<Channel> = (0..9).map(|i| proven(0x7E0, 0x2000 + i, "boost", "bar")).collect();
		channels.push(raw(0x7E0, 0x38F0));
		let mut a = App::new(channels);
		assert!(!a.chart_shown);
		on_key(&mut a, KeyCode::Char('g'));
		assert!(a.chart_shown);
		assert_eq!(a.charted.len(), CHART_CHANNELS);
		assert!(!a.charted.contains(&(0x7E0, 0x38F0, 0)), "nothing raw is seeded");

		// Hiding and showing it again keeps the marks: the seed is for an empty
		// selection, not for every press.
		on_key(&mut a, KeyCode::Char('g'));
		a.charted.remove(&(0x7E0, 0x2000, 0));
		on_key(&mut a, KeyCode::Char('g'));
		assert_eq!(a.charted.len(), CHART_CHANNELS - 1);
	}

	#[test]
	fn the_arrow_keys_page_the_chart_only_while_it_is_up_and_escape_still_quits() {
		// `←`/`→` are the widget's own advertised keys, so they page the chart —
		// and they do nothing at all while there is no chart, because a key
		// advertised and inert is worse than no key.
		//
		// `Esc` quits here where in `measure` it cancels a run. That divergence
		// is deliberate: a stopwatch needs a cheap "throw this one away" and
		// `watch` has nothing to throw away.
		let mut a = App::new(vec![
			proven(0x7E0, 0x2029, "boost", "bar"),
			proven(0x7E0, 0x206E, "engine speed", "/min"),
			proven(0x7E0, 0x2000, "coolant", "°C"),
		]);
		for c in &a.channels {
			a.charted.insert(c.key());
		}
		on_key(&mut a, KeyCode::Right);
		assert_eq!(a.chart_page, 0, "nothing to page while the chart is down");

		a.chart_shown = true;
		// Three units over two pages, because two scales is all one page holds
		// — the widget's rule, and this side does not get a second opinion on
		// it.
		on_key(&mut a, KeyCode::Right);
		assert_eq!(a.chart_page, 1);
		on_key(&mut a, KeyCode::Right);
		assert_eq!(a.chart_page, 0, "wraps rather than running off the end");
		on_key(&mut a, KeyCode::Left);
		assert_eq!(a.chart_page, 1, "and wraps backwards too");

		// Changing what is marked renumbers the pages, so the page index goes
		// back to the first one. The key prints `1/3`, which is what says so.
		a.screen = Screen::Select;
		a.cursor = 0;
		on_key(&mut a, KeyCode::Char('g'));
		assert_eq!(a.chart_page, 0);

		assert!(!on_key(&mut a, KeyCode::Char('q')));
		let mut a = App::new(vec![proven(0x7E0, 0x2029, "boost", "bar")]);
		assert!(!on_key(&mut a, KeyCode::Esc));
	}

	#[test]
	fn the_chart_names_its_lines_its_axis_and_the_window_it_covers() {
		// "Я не сразу понял что означает график": a frame with two bare numbers
		// in it is not something a chart can be read from. The window is this
		// screen's own sentence — a chart whose extent is a secret is a chart
		// nobody can read — so `watch` prints it where `watch` prints its other
		// sentences.
		let mut a = App::new(vec![
			proven(0x7E0, 0x202A, "Boost pressure", "bar"),
			proven(0x7E0, 0x206E, "Engine speed", "/min"),
			raw(0x7E0, 0x38F0),
		]);
		for (i, did) in [0x202Au16, 0x206E].iter().enumerate() {
			a.charted.insert((0x7E0, *did, 0));
			for step in 0..5 {
				let raw_value = 1000 + step * 500 * (i as u16 + 1);
				a.observe(0x7E0, *did, step as f64 * 0.2, raw_value.to_be_bytes().to_vec());
			}
		}
		a.charted.insert((0x7E0, 0x38F0, 0));
		a.chart_shown = true;

		// Every size a terminal is still allowed to be. The table and the chart
		// share the screen, so a chart that only reads at 120 columns is one
		// that does not read.
		for (w, h) in [(120u16, 40u16), (100, 30), (80, 24)] {
			let text = live_text(&mut a, w, h);
			assert!(text.contains("Boost pressure"), "{w}×{h} does not name the line:\n{text}");
			assert!(text.contains("Engine speed"), "{w}×{h} plots one line, not two:\n{text}");
			assert!(text.contains("time"), "{w}×{h} does not say what it is over:\n{text}");
			// The folded line's own numbers are not on the axis, so they are in
			// the key: a curve whose axis is nowhere is decoration, not data.
			assert!(text.contains("/min]"), "{w}×{h} folds a line and hides its scale:\n{text}");
			assert!(text.contains("last 60s"), "{w}×{h} keeps the window a secret:\n{text}");
			assert!(text.contains("[g] chart"), "{w}×{h}:\n{text}");
			// The marked channel with nothing to plot is accounted for rather
			// than silently missing, or a driver concludes the tool is broken.
			assert!(text.contains("no proven number"), "{w}×{h}:\n{text}");
		}

		// With the chart down the table has the screen to itself.
		a.chart_shown = false;
		let text = live_text(&mut a, 110, 30);
		assert!(!text.contains("/min]"), "{text}");
		assert!(!text.contains("last 60s"), "no chart, so no window to describe: {text}");
		assert!(text.contains("[g] chart"), "the key is still offered: {text}");
	}

	#[test]
	fn nothing_marked_says_which_two_keys_fix_it_rather_than_drawing_an_empty_frame() {
		// What an empty chart should say is a sentence about this screen, which
		// is why `chart::plot` hands the empty case back to its caller.
		let mut a = App::new(vec![raw(0x7E0, 0x38F0)]);
		a.chart_shown = true;
		let text = live_text(&mut a, 110, 30);
		assert!(text.contains("nothing marked"), "{text}");
	}

	/// The chart-lines screen as a person sees it.
	fn series_text(app: &mut App, w: u16, h: u16) -> String {
		use ratatui::Terminal;
		use ratatui::backend::TestBackend;
		let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
		terminal.draw(|frame| draw_series(frame, app)).unwrap();
		let buffer = terminal.backend().buffer().clone();
		(0..h)
			.map(|y| (0..w).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>())
			.collect::<Vec<_>>()
			.join("\n")
	}

	fn settings_text(app: &mut App, w: u16, h: u16) -> String {
		use ratatui::Terminal;
		use ratatui::backend::TestBackend;
		let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
		terminal.draw(|frame| draw_settings(frame, app)).unwrap();
		let buffer = terminal.backend().buffer().clone();
		(0..h)
			.map(|y| (0..w).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>())
			.collect::<Vec<_>>()
			.join("\n")
	}

	/// A channel with a name and one with nothing but its address — the two
	/// kinds the selection screen now tells apart.
	fn unselected(channel: Channel) -> Channel {
		Channel {
			text_id: None,
			selected: false,
			..channel
		}
	}

	/// A survey line for one unit: what it was asked, and what answered.
	///
	/// `asked` is not optional here, and that is the point — a line without it
	/// supports no verdict about what a car lacks, so a fixture that omitted it
	/// would be testing the filter with the filter switched off.
	fn surveyed(request: u16, asked: &[&str], dids: &[u16]) -> String {
		let entries: Vec<String> = dids.iter().map(|d| format!("{{\"did\":\"{d:04X}\",\"data\":\"00\"}}")).collect();
		let asked: Vec<String> = asked.iter().map(|r| format!("\"{r}\"")).collect();
		format!(
			"{{\"request\":\"{request:03X}\",\"asked\":[{}],\"dids\":[{}]}}\n",
			asked.join(","),
			entries.join(",")
		)
	}

	#[test]
	fn a_named_channel_this_car_does_not_answer_is_held_back_and_says_so() {
		// Why the filter exists: a project describes a vehicle family, no one
		// car is all of it, and a named row that can never produce a value is
		// worse than a nameless one because it looks like it works.
		//
		// The fixture says what it was asked. That is not decoration — the
		// first version of this feature inferred it, and on the only real
		// survey in existence the inference was wrong for 1,708 of the 1,746
		// channels it hid.
		let mut a = App::new(vec![
			unselected(proven(0x713, 0x1001, "Brake pressure", "bar")),
			unselected(proven(0x713, 0x1002, "Declared but silent", "bar")),
		]);
		a.answered = crate::plan::answered_from_survey(&surveyed(0x713, &["1001-1002"], &[0x1001]));
		a.screen = Screen::Select;

		assert_eq!(a.visible().len(), 1, "only the one the car answered");
		assert_eq!(a.hidden(), Hidden { unnamed: 0, silent: 1 });

		let text = select_text(&mut a, 80, 14);
		assert!(text.contains("Brake pressure"), "{text}");
		assert!(!text.contains("Declared but silent"), "the silent row is off the list:\n{text}");
		// Named for what it is. "unnamed" would be a lie about a row that has a
		// perfectly good name and no value behind it.
		assert!(text.contains("1 this car does not answer are hidden"), "{text}");

		on_key(&mut a, KeyCode::Char('u'));
		assert_eq!(
			a.visible().len(),
			2,
			"[u] brings it back — the survey may have caught the car in the wrong state"
		);
	}

	#[test]
	fn a_unit_no_survey_visited_keeps_every_row_it_has() {
		// The trap this filter has to avoid: silence is only evidence about a
		// unit somebody actually asked. The safe habit is surveying one
		// unit at a time, so a survey covering the brakes alone is normal — and
		// reading it as "the gearbox answers nothing" would take a whole control
		// unit off the screen on the strength of never having looked at it.
		let mut a = App::new(vec![
			unselected(proven(0x713, 0x1001, "Brake pressure", "bar")),
			unselected(proven(0x7E1, 0x380A, "Engine speed", "/min")),
		]);
		a.answered = crate::plan::answered_from_survey(&surveyed(0x713, &["1001-1002"], &[0x1001]));
		a.screen = Screen::Select;

		// One tab per unit, so each is checked on its own tab rather than by a
		// single count that the tab filter would have shortened anyway.
		for (tab, request) in a.tabs().into_iter().enumerate() {
			a.tab = tab;
			assert_eq!(a.visible().len(), 1, "unit {request:03X} keeps its row");
			assert_eq!(a.hidden(), Hidden::default(), "unit {request:03X} holds nothing back");
		}
		assert_eq!(
			a.answered.saw(0x7E1, 0x380A),
			None,
			"never asked is not the same answer as asked and silent"
		);
		assert_eq!(a.answered.saw(0x713, 0x1002), Some(false), "inside what the brakes were asked");
	}

	#[test]
	fn with_no_survey_at_all_nothing_is_filtered_for_silence() {
		// The default on a car nobody has surveyed. An empty `Answered` must
		// mean "nothing is known", never "nothing answers".
		let mut a = App::new(vec![unselected(proven(0x713, 0x1001, "Brake pressure", "bar"))]);
		a.screen = Screen::Select;
		assert_eq!(a.visible().len(), 1);
		assert_eq!(a.hidden(), Hidden::default());
	}

	#[test]
	fn both_reasons_are_counted_apart_and_read_as_one_sentence() {
		// They are answered differently — a nameless row wants a better name
		// source, a silent one wants the car in another state — so a single
		// number would tell a reader nothing about what to do next.
		let mut a = App::new(vec![
			unselected(proven(0x713, 0x1001, "Brake pressure", "bar")),
			unselected(proven(0x713, 0x1002, "Declared but silent", "bar")),
			unselected(raw(0x713, 0x1003)),
		]);
		a.answered = crate::plan::answered_from_survey(&surveyed(0x713, &["1001-1003"], &[0x1001, 0x1003]));
		a.screen = Screen::Select;

		assert_eq!(a.hidden(), Hidden { unnamed: 1, silent: 1 });
		let text = select_text(&mut a, 80, 14);
		assert!(
			text.contains("1 with no name anywhere and 1 this car does not answer are hidden"),
			"{text}"
		);
		assert!(text.contains("2 hidden"), "the title counts them together: {text}");
	}

	#[test]
	fn the_rows_nothing_can_name_are_hidden_and_the_screen_says_how_many() {
		// The reported defect: 2,751 rows, and on a unit no project describes
		// every one of them reads `74B/02BD` — the identifier printed beside
		// itself. They are hidden rather than dropped, because a nameless
		// identifier is exactly what somebody hunting a new measurement wants.
		let mut a = App::new(vec![
			unselected(proven(0x74B, 0x0102, "Bonnet open", "")),
			unselected(raw(0x74B, 0x02BD)),
			unselected(raw(0x74B, 0x02C1)),
			unselected(raw(0x74B, 0x1948)),
		]);
		a.screen = Screen::Select;
		assert_eq!(a.visible().len(), 1, "only the one anything describes");
		assert_eq!(a.hidden().unnamed, 3);

		let text = select_text(&mut a, 80, 14);
		assert!(text.contains("Bonnet open"), "{text}");
		assert!(!text.contains("74B/02BD"), "the nameless row is off the list:\n{text}");
		// A silently shortened list is its own bug, so the count and the key
		// that undoes it are on the same screen as the shortening.
		assert!(text.contains("3 hidden"), "{text}");
		assert!(text.contains("3 with no name anywhere are hidden"), "{text}");
		assert!(text.contains("[u] shows them"), "{text}");

		on_key(&mut a, KeyCode::Char('u'));
		assert_eq!(a.visible().len(), 4);
		assert_eq!(a.hidden().total(), 0, "nothing is held back once they are asked for");
		let text = select_text(&mut a, 80, 14);
		assert!(text.contains("74B/02BD"), "{text}");
		assert!(!text.contains("are hidden"), "no row is being held back now:\n{text}");
		assert!(text.contains("[u] hides"), "the key still says what it does now: {text}");
	}

	#[test]
	fn a_watched_row_stays_on_the_list_however_little_describes_it() {
		// `--did 74B:02BD` names a channel nothing describes. Hiding it would
		// leave a ticked row that could never be unticked, and a chart mark on
		// a line nobody could find again.
		let mut a = App::new(vec![raw(0x74B, 0x02BD), unselected(raw(0x74B, 0x02C1))]);
		a.screen = Screen::Select;
		assert_eq!(a.visible(), vec![0], "the selected one, and only it");
		assert_eq!(a.hidden().unnamed, 1);

		// A favourite is a person saying "this one" about precisely the row the
		// filter is built to remove, so it outranks the filter too.
		let mut a = App::new(vec![unselected(raw(0x74B, 0x02BD)), unselected(raw(0x74B, 0x02C1))]);
		a.favourites.insert((0x74B, 0x02BD, 0));
		assert_eq!(a.visible(), vec![0]);
	}

	#[test]
	fn favourites_are_offered_first_and_are_what_a_run_starts_with() {
		// The reported cost: the same handful re-found among thousands of rows
		// every drive. `f` writes the answer down; the next run ticks it.
		let mut a = App::new(vec![
			unselected(proven(0x7E0, 0x2000, "Coolant", "°C")),
			unselected(proven(0x7E0, 0x202A, "Boost pressure", "bar")),
			unselected(proven(0x7E0, 0x206E, "Engine speed", "/min")),
		]);
		a.screen = Screen::Select;
		assert_eq!(a.visible(), vec![0, 1, 2]);

		a.cursor = 2;
		on_key(&mut a, KeyCode::Char('f'));
		assert!(a.favourites.contains(&(0x7E0, 0x206E, 0)));
		assert!(a.channels[2].selected, "marking it for every drive marks it for this one");
		assert_eq!(a.visible(), vec![2, 0, 1], "the favourite is offered first");
		let text = select_text(&mut a, 80, 12);
		assert!(text.contains("★"), "the mark is visible where it was made:\n{text}");

		// And pressing it again takes the mark off without hiding the row —
		// "not next time" is not "not now".
		on_key(&mut a, KeyCode::Char('f'));
		assert!(a.favourites.is_empty());
		assert!(a.channels[2].selected);
	}

	#[test]
	fn a_mark_made_this_run_is_ticked_on_the_next_one() {
		// The point of writing them down. Where they are stored is
		// `crate::config`'s business and is tested there against a temporary
		// file — a test here must never write into the owner's own settings.
		let mut a = App::new(vec![
			unselected(proven(0x7E0, 0x202A, "Boost pressure", "bar")),
			unselected(proven(0x7E1, 0x3816, "Selected gear", "")),
		]);
		// **No VIN, deliberately.** Saving goes to `~/.vagcan/config.toml`, and
		// a test that set one would write into the owner's own settings — which
		// is exactly what happened the first time this was written. The mark
		// still lands; only the writing is declined, and the note says so.
		a.favourites_vin = None;
		a.screen = Screen::Select;
		a.cursor = 1;
		on_key(&mut a, KeyCode::Char('f'));
		assert_eq!(a.favourites.len(), 1);
		assert!(a.note.contains("no car"), "{}", a.note);

		// Through the written form and back, which is what a second run does.
		let written: Vec<String> = a.favourites.iter().map(|key| favourites::render_key(*key)).collect();
		let mut next = App::new(vec![
			unselected(proven(0x7E0, 0x202A, "Boost pressure", "bar")),
			unselected(proven(0x7E1, 0x3816, "Selected gear", "")),
		]);
		next.favourites = favourites::from_list(&written);
		assert_eq!(next.select_favourites(), 1);
		assert!(next.channels[1].selected);
		assert!(!next.channels[0].selected, "only what was marked");
	}

	#[test]
	fn the_settings_screen_shows_what_each_setting_is_and_what_it_reads() {
		// A settings screen whose rows are only labels makes somebody guess,
		// and the guess is made sitting in a car. Every row carries its current
		// value and a note saying what it is for.
		let mut a = App::new(vec![unselected(proven(0x7E0, 0x202A, "Boost pressure", "bar"))]);
		a.screen = Screen::Settings;
		let text = settings_text(&mut a, 100, 12);
		assert!(text.contains("Poll rate"), "{text}");
		assert!(text.contains("10.0 Hz"), "{text}");
		assert!(text.contains("Key at the end of each row"), "{text}");
		assert!(text.contains("hidden"), "{text}");
		assert!(text.contains("names.csv"), "the note says where the key is used: {text}");
		// And the file it writes to, so nobody has to be told where it lives.
		assert!(text.contains("config.toml"), "{text}");
	}

	#[test]
	fn the_rate_steps_and_stops_at_the_ends_rather_than_running_off_them() {
		// Zero is a screen that never updates and a thousand is the same screen
		// with the bus full of requests the car will not answer any sooner.
		let mut a = App::new(vec![unselected(proven(0x7E0, 0x202A, "Boost pressure", "bar"))]);
		a.screen = Screen::Settings;
		a.hz = crate::config::MIN_HZ;
		// No VIN and no writable settings needed: the value moves in the app,
		// and saving is what `crate::config` is tested for.
		a.nudge_setting(Setting::Rate, false);
		assert_eq!(a.hz, crate::config::MIN_HZ, "it does not go below the floor");
		a.nudge_setting(Setting::Rate, true);
		assert!(a.hz > crate::config::MIN_HZ);
		a.hz = crate::config::MAX_HZ;
		a.nudge_setting(Setting::Rate, true);
		assert_eq!(a.hz, crate::config::MAX_HZ, "nor above the ceiling");
	}

	#[test]
	fn the_key_column_appears_only_when_it_is_asked_for() {
		// Four columns of identifier are noise while driving, and the one thing
		// anybody needs the moment a row reads badly. So it is a setting, and
		// it shows the *text id* — the identifier is already its own column, so
		// repeating it would add nothing to look up.
		let mut named = proven(0x7E0, 0x202A, "Boost pressure", "bar");
		named.text_id = Some("IDE00022".into());
		let mut a = App::new(vec![Channel { selected: true, ..named }]);
		a.screen = Screen::Live;

		let text = live_text(&mut a, 100, 12);
		assert!(!text.contains("IDE00022"), "off by default:\n{text}");

		a.show_key = true;
		let text = live_text(&mut a, 100, 12);
		assert!(text.contains("IDE00022"), "{text}");
		assert!(text.contains("Key"), "and the column says what it is: {text}");
	}

	#[test]
	fn a_run_with_no_car_says_the_favourite_will_not_outlive_it() {
		// A replay addresses nothing, and a car that will not report a VIN has
		// no directory of its own. Silently doing nothing would be the worse
		// answer of the two available.
		let mut a = App::new(vec![unselected(proven(0x7E0, 0x202A, "Boost pressure", "bar"))]);
		a.screen = Screen::Select;
		on_key(&mut a, KeyCode::Char('f'));
		assert!(a.favourites.contains(&(0x7E0, 0x202A, 0)), "it still works for this run");
		assert!(a.note.contains("no car"), "{}", a.note);
		let text = select_text(&mut a, 80, 14);
		assert!(text.contains("no car"), "{text}");
	}

	#[test]
	fn the_chart_lines_are_chosen_beside_the_chart_rather_than_two_screens_away() {
		// "Графики оказались неюзабельными, т.к. нельзя выбрать что отображать":
		// the mark was only reachable from the list of every channel the car
		// has. Here the list is what is being watched, and the chart is on the
		// same screen, so a press and its effect are visible at once.
		let mut a = App::new(vec![
			proven(0x7E0, 0x202A, "Boost pressure", "bar"),
			proven(0x7E0, 0x206E, "Engine speed", "/min"),
			state(0x7E1, 0x3816, "Selected gear"),
		]);
		assert_eq!(a.screen, Screen::Live);
		on_key(&mut a, KeyCode::Char('s'));
		assert_eq!(a.screen, Screen::Series);
		assert!(a.chart_shown, "picking lines for a hidden chart is picking in the dark");

		// Opening the chart seeded it from the table, so both numbers are on
		// it; the state is not, and never can be.
		assert_eq!(a.drawn(), vec![(0x7E0, 0x202A, 0), (0x7E0, 0x206E, 0)]);
		on_key(&mut a, KeyCode::Char('n'));
		assert!(a.drawn().is_empty());
		on_key(&mut a, KeyCode::Down);
		on_key(&mut a, KeyCode::Char(' '));
		assert_eq!(a.drawn(), vec![(0x7E0, 0x206E, 0)], "one press, one line");

		// `a` takes everything it can draw and nothing it cannot.
		on_key(&mut a, KeyCode::Char('a'));
		assert_eq!(a.drawn(), vec![(0x7E0, 0x202A, 0), (0x7E0, 0x206E, 0)]);
		assert!(!a.charted.contains(&(0x7E1, 0x3816, 0)), "a state has no number to plot");

		on_key(&mut a, KeyCode::Esc);
		assert_eq!(a.screen, Screen::Live);
		assert!(a.chart_shown, "leaving the list does not put the chart away");
	}

	#[test]
	fn the_chart_lines_screen_says_why_a_marked_channel_is_not_a_line() {
		// A mark that silently does nothing is what makes a chart look broken.
		// Two ways it can: nothing to plot, and no room left.
		let mut channels: Vec<Channel> = (0..8).map(|i| proven(0x7E0, 0x2000 + i, "boost", "bar")).collect();
		channels.push(state(0x7E1, 0x3816, "Selected gear"));
		let mut a = App::new(channels);
		for c in &a.channels {
			a.charted.insert(c.key());
		}
		a.screen = Screen::Series;
		a.chart_shown = true;

		// Every size a terminal is still allowed to be: the note is the half
		// that explains an empty-looking chart, so it may never be the half
		// that falls off the right-hand edge.
		for (w, h) in [(120u16, 40u16), (100, 30), (80, 24)] {
			let text = series_text(&mut a, w, h);
			assert!(text.contains("chart lines"), "{w}×{h}:\n{text}");
			assert!(text.contains("6 drawn of 9 marked"), "{w}×{h}:\n{text}");
			assert!(text.contains("drawn"), "{w}×{h}:\n{text}");
			assert!(text.contains("no room"), "{w}×{h} hides why two marks did nothing:\n{text}");
			assert!(text.contains("[space] draw"), "{w}×{h}:\n{text}");
		}
		// The third reason is a property of the channel rather than of the
		// screen, and it is asserted where it is decided: at 24 rows the ninth
		// row of a nine-row list is below the fold, and a test that only looked
		// at a tall terminal would be testing the scroll.
		assert_eq!(series_note(&a, &a.channels[0]), "drawn");
		assert_eq!(series_note(&a, &a.channels[7]), "no room");
		assert_eq!(series_note(&a, &a.channels[8]), "no number");
	}

	#[test]
	fn the_live_footer_keeps_every_key_it_advertises_at_eighty_columns() {
		// It was one row and did not wrap. A replay adds its playback keys to
		// the same line, so at eighty columns `[q] quit` — and then the pause
		// and speed keys behind it — were simply not on the screen, which on a
		// real terminal looks like a line that ends rather than one that was
		// cut.
		let mut a = App::new(vec![proven(0x7E0, 0x202A, "Boost pressure", "bar")]);
		a.live = false;
		a.chart_shown = true;
		a.status = " · [space] pause  [g] chart off to seek  [+-] speed · 0/180s ×1.00".to_string();
		let text = live_text(&mut a, 80, 24);
		for line in text.lines() {
			assert_eq!(line.chars().count(), 80, "a line is not the width of the screen:\n{text}");
		}
		for key in ["[s] lines", "[q] quit", "[space] pause", "[+-] speed"] {
			assert!(text.contains(key), "80 columns hides {key}:\n{text}");
		}
	}

	#[test]
	fn nothing_on_either_list_overruns_eighty_columns() {
		// Both defects this session were only visible on a real terminal: a
		// line wider than the screen vanishes rather than wrapping, so a
		// rendered buffer is asserted against the width it was given.
		let long = "Brake_pedal_information_plausibility_and_then_some_more_words_still";
		let mut a = App::new(vec![
			proven(0x7E0, 0x202A, long, "bar"),
			proven(0x7E0, 0x206E, "Engine speed", "/min"),
			unselected(raw(0x7E0, 0x38F0)),
		]);
		a.chart_shown = true;
		a.screen = Screen::Select;
		for text in [select_text(&mut a, 80, 24), series_text(&mut a, 80, 24)] {
			for line in text.lines() {
				assert_eq!(line.chars().count(), 80, "a line is not the width of the screen:\n{text}");
			}
			// A name long enough to fill the screen must not push the columns
			// after it off the edge — they are the ones a reader acts on.
			assert!(text.contains("206E"), "{text}");
			assert!(text.contains("/min"), "{text}");
		}
		// And every key of both screens is still on them at that width.
		for key in ["[u] hidden", "[f] favourite", "[a] all", "[enter] back"] {
			assert!(select_text(&mut a, 80, 24).contains(key), "80 columns hides {key}");
		}
		a.screen = Screen::Series;
		for key in ["[space] draw", "[←→] page", "[s]/[esc] back"] {
			assert!(series_text(&mut a, 80, 24).contains(key), "80 columns hides {key}");
		}
	}

	#[test]
	fn the_selection_screen_shows_which_channels_the_chart_holds() {
		// The mark is a second choice over the same list — what is on the table
		// and what is on the chart are different questions — so it needs to be
		// visible on the screen where it is made.
		let mut a = App::new(vec![
			proven(0x7E0, 0x2029, "Boost pressure", "bar"),
			proven(0x7E0, 0x206E, "Engine speed", "/min"),
		]);
		a.charted.insert((0x7E0, 0x206E, 0));
		a.screen = Screen::Select;
		let text = select_text(&mut a, 110, 12);
		let charted_row = text
			.lines()
			.find(|l| l.contains("Engine speed"))
			.expect("the marked row is on screen")
			.to_string();
		let plain_row = text.lines().find(|l| l.contains("Boost pressure")).expect("the other row").to_string();
		assert!(charted_row.contains("chart"), "{charted_row:?}");
		assert!(!plain_row.contains("chart"), "{plain_row:?}");

		// And every key this screen has is on it, at the narrowest a terminal
		// is allowed to be. The hints outgrew one row when the chart mark was
		// added and `[enter] back` — the key that leaves the screen — fell off
		// the end of the line.
		for (w, h) in [(120u16, 40u16), (100, 30), (80, 24)] {
			let text = select_text(&mut a, w, h);
			for key in ["[g] chart", "[a] all", "[n] none", "[enter] back"] {
				assert!(text.contains(key), "{w}×{h} hides {key}:\n{text}");
			}
		}
	}

	#[test]
	fn rows_are_ordered_the_same_way_the_plan_polls_them() {
		// A table whose rows move between cycles cannot be read.
		let mut a = app(need_rows!());
		a.channels.iter_mut().for_each(|c| c.selected = true);
		let order: Vec<(u16, u16)> = a.shown().iter().map(|c| (c.request, c.did)).collect();
		let mut sorted = order.clone();
		sorted.sort();
		assert_eq!(order, sorted);
	}
}
