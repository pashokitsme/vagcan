//! The board's panel loop, run over a recording instead of a bus.
//!
//! What decides the glass is the board's own code: [`Screen`] (the page cursor, the alarms
//! and the one button), [`Plan::rates`] (what the bus reads, and how often, for the page on
//! the glass) and [`pages::from_plan`] (the pages the board holds). What this module adds is
//! the part of `vag-dash-fw`'s `bin/dash.rs` that is not a library — the frame loop, the value
//! store with its staleness rule, and the cell composition in `panel_task` — mirrored here
//! because the firmware cannot be built for the host. Where a constant is the board's, it
//! says where it lives.
//!
//! **Nothing reads a clock.** Every time is the recording's own, in milliseconds of its
//! `t_s`: the frame loop ticks every [`FRAME_MS`] of recording time, a read happens when the
//! board's schedule would make it, and it takes what the recording holds at that moment.
//! Pacing a terminal is the caller's business, not this module's.

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::DrawTarget;
use vag_dash_render::alarm::{Alarm, BLINK_MS, ChannelId, Direction, MAX_ALARMS, Press, Rule};
use vag_dash_render::button::{Button, Press as ButtonPress};
use vag_dash_render::history::History;
use vag_dash_render::pages::{self, Layout, MAX_PAGES};
use vag_dash_render::plan::{Page, Plan};
use vag_dash_render::screen::{Change, Glass, Screen};
use vag_dash_render::{Board, Cell, Deviation, Frame, Links, Theme, draw_with};

/// Milliseconds between two panel frames — `FRAME_MS` in `vag-dash-fw`'s `bin/dash.rs`.
pub const FRAME_MS: u64 = 200;
// The offending cell blinks in BLINK_MS halves: frames at most BLINK_MS / 2 apart, so every half gets a frame.
const _: () = assert!(FRAME_MS * 2 <= BLINK_MS, "the panel must frame at least twice per half blink");
/// How old a value may be and still be shown — `STALE` in `vag-dash-fw`'s `bin/dash.rs`.
/// Counted from when the recording heard it.
pub const STALE_MS: u64 = 5_000;
/// Cells a stored page may hold — `MAX_CELLS` in `vag-dash-fw`'s `config.rs`.
const MAX_CELLS: usize = 8;
/// The board's framebuffer — `WIDTH` and `HEIGHT` in `vag-dash-fw`'s `panel.rs`.
pub const WIDTH: usize = 256;
pub const HEIGHT: usize = 64;

/// One plan channel's readings, oldest first: the recording time in milliseconds, and the
/// value — `None` where the unit answered with something that does not decode.
pub type Series = Vec<(u64, Option<f32>)>;

/// What one frame did that is worth a line of the log.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
	/// The first frame's page: the driver's, before anything happened.
	Start { page: u8 },
	/// A press turned the page.
	Paged { page: u8 },
	/// A press ended this rule's episode.
	Hushed { rule: Option<usize> },
	/// A rule took the glass: its page, the channel it points at and its value then.
	Took {
		rule: usize,
		page: u8,
		channel: Option<u16>,
		value: Option<f32>,
		/// For a drift rule, what the unit asked for at that moment.
		specified: Option<f32>,
	},
	/// The hold after the release ran out, and the glass is back on `page`.
	Over { rule: Option<usize>, page: u8 },
	/// A press silenced the alarm, and the glass is back on `page`.
	Silenced { rule: Option<usize>, page: u8 },
	/// The rule's page is not one the board holds, so `shown` is drawn instead.
	Missed { rule: Option<usize>, page: u16, shown: u8 },
}

/// Why a press asked for was not made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
	/// Before the first frame or after the last.
	Outside,
	/// Closer than the board's `PRESS_GAP_MS` to the last press made: the board's button
	/// gate takes the two as one.
	TooSoon,
}

/// One frame: its time, what the glass showed, and what happened.
#[derive(Debug, Clone, PartialEq)]
pub struct Tick {
	pub t_ms: u64,
	pub glass: Glass,
	pub events: Vec<Event>,
}

/// One subscription, as the board's bus task holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sub {
	foreground: bool,
	period_ms: u32,
	/// When the next read is due.
	next_ms: u64,
}

/// One slot of the board's value store.
#[derive(Debug, Clone, Copy, Default)]
struct Slot {
	value: Option<f32>,
	at: Option<u64>,
	fresh_for: u64,
}

/// The board's [`Screen`] for however many rules the plan carries: its size is a type
/// parameter on the board, fixed when the image is built, and a number read from a file
/// here.
enum Screens {
	R0(Screen<'static, 0>),
	R1(Screen<'static, 1>),
	R2(Screen<'static, 2>),
	R3(Screen<'static, 3>),
	R4(Screen<'static, 4>),
}

// One arm per rule count the board can hold: a board that holds more needs one more arm.
const _: () = assert!(MAX_ALARMS == 4);

macro_rules! each_screen {
	($screens:expr, $screen:ident => $body:expr) => {
		match $screens {
			Screens::R0($screen) => $body,
			Screens::R1($screen) => $body,
			Screens::R2($screen) => $body,
			Screens::R3($screen) => $body,
			Screens::R4($screen) => $body,
		}
	};
}

impl Screens {
	fn new(rules: &'static [Alarm<'static>]) -> Option<Screens> {
		Some(match rules.len() {
			0 => Screens::R0(Screen::new(rules)),
			1 => Screens::R1(Screen::new(rules)),
			2 => Screens::R2(Screen::new(rules)),
			3 => Screens::R3(Screen::new(rules)),
			4 => Screens::R4(Screen::new(rules)),
			_ => return None,
		})
	}

	fn frame(&mut self, cursor: u8, pages: u8, now_ms: u64, value_of: impl Fn(u16) -> Option<f32>) -> Glass {
		each_screen!(self, screen => screen.frame(cursor, pages, now_ms, value_of))
	}

	fn press(&mut self, cursor: &mut u8, pages: u8) -> Press {
		each_screen!(self, screen => screen.press(cursor, pages))
	}
}

/// The panel, run over one recording.
pub struct Replay {
	plan: &'static Plan,
	/// The board's pages, as its configuration holds them.
	layouts: Vec<Layout<'static>>,
	/// Per plan channel; `None` for one the recording does not carry.
	series: Vec<Option<Series>>,
	/// Short presses, in recording milliseconds, sorted.
	presses: Vec<u64>,
	refused: Vec<(u64, Refusal)>,
	last_frame: u64,
	pressed: usize,
	screen: Screens,
	cursor: u8,
	subs: Vec<Option<Sub>>,
	slots: Vec<Slot>,
	histories: Vec<History<WIDTH>>,
	/// The page the last frame drew — what the bus reads in the foreground.
	drawn: Option<u8>,
	/// The rule on the glass after the last frame.
	rule: Option<usize>,
	missed_said: Option<u16>,
	start: u64,
	now: u64,
	end: u64,
	last: Option<(u64, Glass)>,
}

impl Replay {
	/// `series` is one entry per plan channel; the frames run from `start_ms` to `end_ms` of
	/// the recording. `Err` for a plan with more rules than the board holds — the generator
	/// refuses one.
	pub fn new(plan: &'static Plan, series: Vec<Option<Series>>, mut presses: Vec<u64>, start_ms: u64, end_ms: u64) -> Result<Replay, String> {
		let screen = Screens::new(plan.alarms).ok_or_else(|| format!("{} alarm rules, and the board holds at most {MAX_ALARMS}", plan.alarms.len()))?;
		if series.len() != plan.channels.len() {
			return Err(format!("{} series for {} plan channels", series.len(), plan.channels.len()));
		}
		presses.sort_unstable();
		// The frames run on FRAME_MS from the first row, so the last is not the last row.
		let last_frame = start_ms + end_ms.saturating_sub(start_ms) / FRAME_MS * FRAME_MS;
		// A press is made where the board would take one: inside the frames, and through the
		// button's own gate, which lets none through closer than PRESS_GAP_MS to the last.
		let mut button = Button::new();
		let mut refused = Vec::new();
		presses.retain(|&p| {
			let why = match (start_ms..=last_frame).contains(&p) {
				false => Some(Refusal::Outside),
				true if button.remote(ButtonPress::Short, p).is_none() => Some(Refusal::TooSoon),
				true => None,
			};
			refused.extend(why.map(|why| (p, why)));
			why.is_none()
		});
		Ok(Replay {
			plan,
			layouts: pages::from_plan(plan.pages, MAX_PAGES, MAX_CELLS).collect(),
			series,
			presses,
			refused,
			last_frame,
			pressed: 0,
			screen,
			cursor: 0,
			subs: vec![None; plan.channels.len()],
			slots: vec![Slot::default(); plan.channels.len()],
			histories: (0..plan.chart_count()).map(|_| History::new()).collect(),
			drawn: None,
			rule: None,
			missed_said: None,
			start: start_ms,
			now: start_ms,
			end: end_ms,
			last: None,
		})
	}

	pub fn start_ms(&self) -> u64 {
		self.start
	}

	pub fn end_ms(&self) -> u64 {
		self.end
	}

	/// The time of the last frame: the last row, down to the frame period.
	pub fn last_frame_ms(&self) -> u64 {
		self.last_frame
	}

	/// The presses not made, in time order, and why.
	pub fn refused(&self) -> &[(u64, Refusal)] {
		&self.refused
	}

	/// The next frame, or `None` past the end of the recording.
	pub fn step(&mut self) -> Option<Tick> {
		if self.now > self.end {
			return None;
		}
		let t = self.now;
		self.now += FRAME_MS;
		// The generator refuses more than `MAX_PAGES`, so the count fits.
		let pages = self.layouts.len() as u8;
		let mut events = Vec::new();
		if self.last.is_none() {
			events.push(Event::Start { page: self.cursor });
		}

		// The button task routes a press through the screen between two frames.
		while self.presses.get(self.pressed).is_some_and(|&p| p <= t) {
			self.pressed += 1;
			let showing = self.rule;
			match self.screen.press(&mut self.cursor, pages) {
				Press::NextPage => events.push(Event::Paged { page: self.cursor }),
				Press::Silenced => events.push(Event::Hushed { rule: showing }),
			}
		}

		self.read(t);

		let slots = &self.slots;
		let value_of = |i: u16| current(slots, i, t);
		let glass = self.screen.frame(self.cursor, pages, t, value_of);
		let before = self.rule;
		match glass.change {
			Some(Change::Took { rule }) => {
				self.rule = Some(rule);
				// Said as a miss instead, once, the way the board says it.
				if glass.missed.is_none() {
					let channel = glass.offending.map(|c| c.0);
					events.push(Event::Took {
						rule,
						page: glass.page,
						channel,
						value: channel.and_then(value_of),
						specified: channel.and_then(|c| self.specified_of(rule, c)).and_then(value_of),
					});
				}
			}
			Some(Change::Over) => {
				self.rule = None;
				events.push(Event::Over {
					rule: before,
					page: glass.page,
				});
			}
			Some(Change::Silenced) => {
				self.rule = None;
				events.push(Event::Silenced {
					rule: before,
					page: glass.page,
				});
			}
			None => {}
		}
		let missed = glass.missed.map(|page| page.0);
		if missed != self.missed_said {
			self.missed_said = missed;
			if let Some(page) = missed {
				events.push(Event::Missed {
					rule: self.rule,
					page,
					shown: glass.page,
				});
			}
		}

		// Every chart samples its channel every frame, shown or not.
		for chart in self.plan.charts() {
			self.histories[chart.slot].push(chart.channel, current(&self.slots, chart.channel, t));
		}
		self.drawn = Some(glass.page);
		self.last = Some((t, glass));
		Some(Tick { t_ms: t, glass, events })
	}

	/// The specified value a drift rule pairs with `channel`, if the rule is one.
	fn specified_of(&self, rule: usize, channel: u16) -> Option<u16> {
		let alarm = self.plan.alarms.get(rule)?;
		let Rule::Drift { specified, .. } = alarm.rule else { return None };
		let i = alarm.channels.iter().position(|c| *c == ChannelId(channel))?;
		specified.get(i).map(|c| c.0)
	}

	/// The bus: subscriptions for the page on the glass through [`Plan::rates`], and every
	/// read those make due by `t`, each taking what the recording holds at its own moment.
	fn read(&mut self, t: u64) {
		let glass = self.drawn.unwrap_or(self.cursor);
		let shown: &[u16] = self.layouts.get(usize::from(glass)).map_or(&[], |page| page.cells);
		// Every page's cells, charts included: a chart samples its channel every frame.
		let listed: Vec<u16> = self.layouts.iter().flat_map(|page| page.cells.iter().copied()).collect();
		let mut wanted = vec![None; self.plan.channels.len()];
		for rate in self.plan.rates(shown, &listed) {
			wanted[usize::from(rate.channel)] = Some((rate.foreground, rate.period_ms));
		}
		for (i, wanted) in wanted.into_iter().enumerate() {
			// A moved subscription is a new one, and a new one is read at once.
			self.subs[i] = match (self.subs[i], wanted) {
				(Some(sub), Some((foreground, period_ms))) if sub.foreground == foreground && sub.period_ms == period_ms => Some(sub),
				(_, Some((foreground, period_ms))) => Some(Sub {
					foreground,
					period_ms,
					next_ms: t,
				}),
				(_, None) => None,
			};
			let Some(sub) = self.subs[i].as_mut() else { continue };
			if t < sub.next_ms {
				continue;
			}
			// The frame ticks slower than a fast channel is read: only the last read due by
			// now matters, and it is of its own moment, not the frame's.
			let period = u64::from(sub.period_ms.max(1));
			let at = sub.next_ms + (t - sub.next_ms) / period * period;
			sub.next_ms = at + period;
			// `fresh_for` in the board's bus task: three periods of a slow channel, else STALE.
			let fresh_for = (period * 3).max(STALE_MS);
			// Stamped with when the recording heard it, not when this read happened: a value
			// heard long ago does not start a fresh `fresh_for` by being read again.
			let (heard, value) = self.series[i].as_ref().and_then(|series| answer(series, at)).unwrap_or((at, None));
			self.slots[i] = Slot {
				value,
				at: Some(heard),
				fresh_for,
			};
		}
	}

	/// Draw the last frame as the board's panel task draws it.
	pub fn draw<D: DrawTarget<Color = BinaryColor>>(&self, target: &mut D) {
		let Some((t, glass)) = self.last else { return };
		let Some(layout) = self.layouts.get(usize::from(glass.page)) else {
			return;
		};
		let value_of = |i: u16| current(&self.slots, i, t);
		let cell_of = |index: u16| {
			let cell = match self.plan.channel(index) {
				Some(channel) => {
					let deviation = match channel.setpoint {
						None => Deviation::None,
						Some(specified) => match (value_of(index), value_of(specified)) {
							(Some(actual), Some(wanted)) => Deviation::Value(actual - wanted),
							_ => Deviation::Unknown,
						},
					};
					Cell::new(channel.label, value_of(index), channel.unit_text, channel.decimals).with_deviation(deviation)
				}
				None => Cell::new("?", None, "", 0),
			};
			// Blinking while out, steady in the hold: `Glass::inverted` is this frame's half.
			if glass.inverted == Some(ChannelId(index)) {
				cell.alarmed()
			} else {
				cell
			}
		};
		// Links: nothing is connected to a board in a recording.
		let board = Board {
			links: Links::NONE,
			rates: None,
		};
		let theme = Theme::bold_mono();
		if !layout.chart {
			let cells: Vec<Cell<'_>> = layout.cells.iter().take(4).map(|&index| cell_of(index)).collect();
			draw_with(&Frame::Values { cells: &cells }, &board, &theme, target);
			return;
		}
		let index = layout.cells.first().copied().unwrap_or(0);
		match self.plan.chart(index) {
			Some(chart) => {
				let frame = Frame::Chart {
					cell: cell_of(index),
					min: chart.min,
					max: chart.max,
					samples: self.histories[chart.slot].samples(),
					seconds_per_sample: FRAME_MS as f32 / 1000.0,
				};
				draw_with(&frame, &board, &theme, target);
			}
			// No range, no chart: the value as a one-cell page, as the board does.
			None => {
				draw_with(&Frame::Values { cells: &[cell_of(index)] }, &board, &theme, target);
			}
		}
	}

	/// One event as a line of the log, at `t_ms` of the recording.
	pub fn line(&self, t_ms: u64, event: &Event) -> String {
		let what = match *event {
			Event::Start { page } => format!("start on {}", self.page_name(u16::from(page))),
			Event::Paged { page } => format!("press — {}", self.page_name(u16::from(page))),
			Event::Hushed { rule } => format!("press — silences {}", rule_name(rule)),
			Event::Took {
				rule,
				page,
				channel,
				value,
				specified,
			} => {
				let mut line = format!("{} took the screen — {}", rule_name(Some(rule)), self.page_name(u16::from(page)));
				if let Some(channel) = channel {
					line.push_str(&format!(", {} = {}", self.channel_name(channel), self.value_text(channel, value)));
					if let Some(wanted) = specified {
						line.push_str(&format!(", asked for {}", self.value_text(channel, Some(wanted))));
					}
				}
				if let Some(alarm) = self.plan.alarms.get(rule) {
					line.push_str(&format!(" ({})", rule_text(&alarm.rule)));
				}
				line
			}
			Event::Over { rule, page } => format!("{} over, the hold ran out — back to {}", rule_name(rule), self.page_name(u16::from(page))),
			Event::Silenced { rule, page } => format!("{} silenced — back to {}", rule_name(rule), self.page_name(u16::from(page))),
			Event::Missed { rule, page, shown } => format!(
				"{}: its page {} is not on the board — showing {}",
				rule_name(rule),
				page + 1,
				self.page_name(u16::from(shown))
			),
		};
		format!("{:>9.2} s  {what}", t_ms as f64 / 1000.0)
	}

	fn page_name(&self, page: u16) -> String {
		let number = page + 1;
		match self.plan.pages.get(usize::from(page)) {
			Some(Page::Values { title, .. }) => format!("page {number} \"{title}\""),
			Some(Page::Chart { channel, .. }) => format!("page {number} (chart of {})", self.channel_name(*channel)),
			None => format!("page {number}"),
		}
	}

	fn channel_name(&self, index: u16) -> String {
		match self.plan.channel(index) {
			Some(c) => channel_name(c.label, c.unit, c.did, c.bit_offset),
			None => format!("channel {index}"),
		}
	}

	fn value_text(&self, index: u16, value: Option<f32>) -> String {
		let Some(v) = value else { return "no value".to_string() };
		match self.plan.channel(index) {
			Some(channel) if channel.unit_text.is_empty() => format!("{:.*}", usize::from(channel.decimals), v),
			Some(channel) => format!("{:.*} {}", usize::from(channel.decimals), v, channel.unit_text),
			None => format!("{v}"),
		}
	}
}

/// A plan channel as the log names it: `label (01:200A)`, and `(01:200A@16)` for a field
/// that does not start the answer.
pub fn channel_name(label: &str, unit: u16, did: u16, bit_offset: u32) -> String {
	format!("{label} ({})", address_name(unit, did, bit_offset))
}

/// Where a channel is read: `01:200A`, and `01:200A@16` for a field that does not start
/// the answer — `dash.toml`'s own spelling of a reference.
pub fn address_name(unit: u16, did: u16, bit_offset: u32) -> String {
	let unit = vag_uds_client::address::UnitAddress::from_request(unit).map_or_else(|| format!("{unit:03X}"), |a| a.label());
	match bit_offset {
		0 => format!("{unit}:{did:04X}"),
		bits => format!("{unit}:{did:04X}@{bits}"),
	}
}

/// The value in the board's store, unless it is older than it may be.
fn current(slots: &[Slot], index: u16, t: u64) -> Option<f32> {
	let slot = slots.get(usize::from(index))?;
	let at = slot.at?;
	if t.saturating_sub(at) > slot.fresh_for {
		return None;
	}
	slot.value
}

/// What a read at `at` gets: the recording's last reading by then and when it was heard —
/// a value, or `None` for a miss or an answer that did not decode. `None` when nothing
/// has been heard yet. How long the value is then shown is the store's rule, counted
/// from when it was heard.
///
/// A recording made before 2026-09-26 writes no miss: after a unit went quiet it repeats
/// the last value with its old time. Such a value is shown until `fresh_for` after it was
/// heard, where the board would store `None` at its first read that missed.
fn answer(series: &Series, at: u64) -> Option<(u64, Option<f32>)> {
	let by_then = series.partition_point(|(t, _)| *t <= at);
	series.get(by_then.checked_sub(1)?).copied()
}

fn rule_name(rule: Option<usize>) -> String {
	match rule {
		Some(rule) => format!("alarm #{}", rule + 1),
		None => "alarm".to_string(),
	}
}

fn rule_text(rule: &Rule<'_>) -> String {
	match *rule {
		Rule::Threshold { trip, release, direction } => match direction {
			Direction::Below => format!("trips at ≤ {trip}, releases above {release}"),
			Direction::Above => format!("trips at ≥ {trip}, releases below {release}"),
		},
		Rule::Drift {
			percent,
			release_percent,
			hold_ms,
			..
		} => format!("trips {percent} % off for {hold_ms} ms, releases under {release_percent} %"),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::dashreplay::glass::Canvas;
	use vag_dash_render::alarm::{HOLD_MS, PageId};
	use vag_dash_render::plan::{Channel, Unit};

	const fn channel(did: u16, label: &'static str) -> Channel {
		Channel {
			unit: 0x7E0,
			did,
			bit_offset: 0,
			bit_length: 16,
			signed: true,
			big_endian: true,
			factor: 0.01,
			offset: 0.0,
			decimals: 1,
			unit_text: "",
			label,
			proven: false,
			// Faster than the frames, so every frame reads it.
			hz: 10.0,
			setpoint: None,
		}
	}

	/// Neutral numbers throughout. Page 1 is the driver's with channels 0 and 1; page 2 shows
	/// channels 2 and 3 and is what the one rule raises: channel 2 or 3 at or below -2 trips,
	/// above -1.5 releases.
	static CHANNELS: [Channel; 4] = [channel(0x1001, "A"), channel(0x1002, "B"), channel(0x1003, "C"), channel(0x1004, "D")];
	static PAGES: [Page; 2] = [
		Page::Values {
			title: "MAIN",
			cells: &[0, 1],
		},
		Page::Values {
			title: "WATCH",
			cells: &[2, 3],
		},
	];
	static WATCHED: [ChannelId; 2] = [ChannelId(2), ChannelId(3)];
	static RULES: [Alarm<'static>; 1] = [Alarm {
		channels: &WATCHED,
		page: PageId(1),
		rule: Rule::Threshold {
			trip: -2.0,
			release: -1.5,
			direction: Direction::Below,
		},
	}];
	static UNITS: [Unit; 1] = [Unit {
		request: 0x7E0,
		response: 0x7E8,
		part_number: "PART",
	}];
	static PLAN: Plan = Plan {
		vin: "TESTVIN0000000001",
		language: "en",
		units: &UNITS,
		channels: &CHANNELS,
		pages: &PAGES,
		alarms: &RULES,
		stalk: None,
		stopwatch: None,
	};

	/// A reading every 100 ms from 0 to `end_ms`, of what `value` says at that moment.
	fn every_100ms(end_ms: u64, value: impl Fn(u64) -> f32) -> Option<Series> {
		Some((0..=end_ms / 100).map(|i| (i * 100, Some(value(i * 100)))).collect())
	}

	/// Channels 0, 1 and 3 calm, channel 2 as given.
	fn replay(watched: Option<Series>, presses: &[u64], end_ms: u64) -> Replay {
		let calm = || every_100ms(end_ms, |_| 0.0);
		Replay::new(&PLAN, vec![calm(), calm(), watched, calm()], presses.to_vec(), 0, end_ms).unwrap()
	}

	/// The first frame at or after `t`. Frames are [`FRAME_MS`] apart, so a hold that ends
	/// between two hands back on the later one — as on the board.
	fn first_frame_from(t: u64) -> u64 {
		t.div_ceil(FRAME_MS) * FRAME_MS
	}

	fn run(mut replay: Replay) -> Vec<(u64, Event)> {
		let mut out = Vec::new();
		while let Some(tick) = replay.step() {
			out.extend(tick.events.into_iter().map(|e| (tick.t_ms, e)));
		}
		out
	}

	fn took(events: &[(u64, Event)]) -> Vec<u64> {
		events.iter().filter(|(_, e)| matches!(e, Event::Took { .. })).map(|(t, _)| *t).collect()
	}

	#[test]
	fn a_value_hovering_on_the_trip_takes_the_screen_once() {
		// The flicker case, through the replay: every other frame reads a value past the trip,
		// the ones between sit inside the band and none reaches the release, so one episode
		// lasts the whole recording.
		let hover = every_100ms(20_000, |t| if (t / FRAME_MS) % 2 == 0 { -2.1 } else { -1.9 });
		let mut replay = replay(hover, &[], 20_000);
		let mut events = Vec::new();
		let mut seen = Vec::new();
		while let Some(tick) = replay.step() {
			seen.push(current(&replay.slots, 2, tick.t_ms));
			events.extend(tick.events.into_iter().map(|e| (tick.t_ms, e)));
		}
		assert_eq!(&seen[..4], [Some(-2.1), Some(-1.9), Some(-2.1), Some(-1.9)], "the frames do see it hover");
		assert_eq!(took(&events), [0]);
		assert!(!events.iter().any(|(_, e)| matches!(e, Event::Over { .. })), "{events:?}");
		match events[1].1 {
			Event::Took {
				rule, page, channel, value, ..
			} => assert_eq!((rule, page, channel, value), (0, 1, Some(2), Some(-2.1))),
			other => panic!("{other:?}"),
		}
	}

	#[test]
	fn the_glass_goes_back_after_the_hold_by_recording_time() {
		// Out from 1 s, back inside from 3 s: the hold is counted in the recording's time.
		let excursion = every_100ms(10_000, |t| if (1_000..3_000).contains(&t) { -3.0 } else { 0.0 });
		let mut replay = replay(excursion, &[], 10_000);
		let mut pages = Vec::new();
		let mut events = Vec::new();
		while let Some(tick) = replay.step() {
			pages.push((tick.t_ms, tick.glass.page, tick.glass.offending));
			events.extend(tick.events.into_iter().map(|e| (tick.t_ms, e)));
		}
		assert_eq!(took(&events), [1_000]);
		let back = first_frame_from(3_000 + HOLD_MS);
		assert_eq!(back, 5_600);
		let over: Vec<_> = events.iter().filter(|(_, e)| matches!(e, Event::Over { .. })).collect();
		assert_eq!(over, [&(back, Event::Over { rule: Some(0), page: 0 })]);
		assert!(pages.contains(&(2_800, 1, Some(ChannelId(2)))), "the cell is pointed at while out");
		assert!(pages.contains(&(back - FRAME_MS, 1, Some(ChannelId(2)))), "and through the hold");
		assert!(pages.contains(&(back, 0, None)), "then the driver's page, nothing inverted");
	}

	#[test]
	fn a_press_silences_and_the_next_crossing_after_a_release_takes_the_screen_again() {
		let series = every_100ms(12_000, |t| match t {
			1_000..4_000 => -3.0,
			6_000..8_000 => -3.0,
			_ => 0.0,
		});
		let events = run(replay(series, &[2_000, 11_000], 12_000));
		assert!(events.contains(&(2_000, Event::Hushed { rule: Some(0) })), "{events:?}");
		assert!(events.contains(&(2_000, Event::Silenced { rule: Some(0), page: 0 })), "{events:?}");
		assert_eq!(took(&events), [1_000, 6_000], "still out after the press is silence, not a takeover");
		// The second episode ends by the hold; a press with nothing up turns the page.
		let back = first_frame_from(8_000 + HOLD_MS);
		assert!(events.contains(&(back, Event::Over { rule: Some(0), page: 0 })), "{events:?}");
		assert!(events.contains(&(11_000, Event::Paged { page: 1 })), "{events:?}");
	}

	#[test]
	fn a_channel_the_recording_does_not_carry_never_trips() {
		let events = run(replay(None, &[], 5_000));
		assert_eq!(events, [(0, Event::Start { page: 0 })]);
		// And a channel that stops answering is no evidence either way: out, then silent — with
		// its sibling silent too, since a sibling answering inside would release the rule.
		let quiet: Series = vec![(1_000, Some(-3.0))];
		let calm = every_100ms(30_000, |_| 0.0);
		let replay = Replay::new(&PLAN, vec![calm.clone(), calm, Some(quiet), None], vec![], 0, 30_000).unwrap();
		let events = run(replay);
		assert_eq!(took(&events), [1_000]);
		assert!(
			!events.iter().any(|(_, e)| matches!(e, Event::Over { .. })),
			"a channel that went quiet is not one that came back: {events:?}"
		);
	}

	#[test]
	fn a_press_closer_than_the_boards_gap_to_the_last_one_is_not_a_press() {
		// The board's button gate: 250 ms from the last press it let through.
		let replay = replay(every_100ms(2_000, |_| 0.0), &[1_000, 500, 600, 800], 2_000);
		assert_eq!(replay.refused(), [(600, Refusal::TooSoon), (1_000, Refusal::TooSoon)]);
		let events = run(replay);
		let pages: Vec<_> = events.iter().filter(|(_, e)| matches!(e, Event::Paged { .. })).collect();
		assert_eq!(pages, [&(600, Event::Paged { page: 1 }), &(800, Event::Paged { page: 0 })]);
	}

	#[test]
	fn a_press_is_inside_the_recording_only_up_to_its_last_frame() {
		// Rows from 0.05 s to 1.03 s: frames at 50, 250, … 850 — none at 1.03 s.
		let calm = |_| Some(vec![(50, Some(0.0))]);
		let series = (0..4).map(calm).collect();
		let replay = Replay::new(&PLAN, series, vec![40, 50, 850, 900], 50, 1_030).unwrap();
		assert_eq!(replay.refused(), [(40, Refusal::Outside), (900, Refusal::Outside)]);
		assert_eq!(replay.last_frame_ms(), 850);
		let events = run(replay);
		assert_eq!(events.iter().filter(|(_, e)| matches!(e, Event::Paged { .. })).count(), 2);
	}

	#[test]
	fn a_reading_goes_stale_from_when_it_was_heard_not_from_when_it_was_read() {
		// Channel 0 is heard at 0 and 300 ms and never again, and sits behind a takeover,
		// read once a second. The read at 5.2 s still finds the 300 ms reading; it is
		// 4.9 s old then and must not start a fresh five seconds of its own.
		let heard: Series = vec![(0, Some(5.0)), (300, Some(5.0))];
		let out = every_100ms(10_000, |_| -3.0);
		let calm = every_100ms(10_000, |_| 0.0);
		let mut replay = Replay::new(&PLAN, vec![Some(heard), calm.clone(), out, calm], vec![], 0, 10_000).unwrap();
		let mut seen = Vec::new();
		while let Some(tick) = replay.step() {
			seen.push((tick.t_ms, current(&replay.slots, 0, tick.t_ms)));
		}
		assert!(seen.contains(&(5_200, Some(5.0))), "{seen:?}");
		assert!(seen.contains(&(5_400, None)), "300 ms + STALE_MS has passed: {seen:?}");
		assert!(seen.iter().filter(|(t, _)| *t >= 5_400).all(|(_, v)| v.is_none()), "{seen:?}");
	}

	#[test]
	fn a_read_that_missed_is_no_value_at_once() {
		// The board stores `None` for a miss; so does the replay, not the last value.
		let series: Series = vec![(0, Some(-3.0)), (1_000, None)];
		let mut replay = replay(Some(series), &[], 2_000);
		let mut seen = Vec::new();
		while let Some(tick) = replay.step() {
			seen.push((tick.t_ms, current(&replay.slots, 2, tick.t_ms)));
		}
		assert_eq!(seen[4], (800, Some(-3.0)));
		assert_eq!(seen[5], (1_000, None));
	}

	#[test]
	fn the_page_behind_a_takeover_is_read_at_the_boards_hidden_rate() {
		// Channel 0 counts the milliseconds. The rule's channel is out from the first frame,
		// so from the second frame on the glass shows the rule's page and the driver's page
		// behind it drops to the board's once a second.
		let clock = every_100ms(5_000, |t| t as f32);
		let out = every_100ms(5_000, |_| -3.0);
		let calm = every_100ms(5_000, |_| 0.0);
		let mut replay = Replay::new(&PLAN, vec![clock, calm.clone(), out, calm], vec![], 0, 5_000).unwrap();
		let mut seen = Vec::new();
		while let Some(tick) = replay.step() {
			seen.push((tick.t_ms, current(&replay.slots, 0, tick.t_ms)));
		}
		assert_eq!(seen[0], (0, Some(0.0)));
		assert_eq!(
			&seen[1..7],
			[
				(200, Some(200.0)),
				(400, Some(200.0)),
				(600, Some(200.0)),
				(800, Some(200.0)),
				(1_000, Some(200.0)),
				(1_200, Some(1_200.0))
			]
		);
	}

	#[test]
	fn the_log_names_the_rule_the_page_the_channel_and_the_value() {
		let excursion = every_100ms(10_000, |t| if (1_000..3_000).contains(&t) { -3.0 } else { 0.0 });
		let mut replay = replay(excursion, &[], 10_000);
		let mut lines = Vec::new();
		while let Some(tick) = replay.step() {
			lines.extend(tick.events.iter().map(|e| replay.line(tick.t_ms, e)));
		}
		assert_eq!(
			lines,
			[
				"     0.00 s  start on page 1 \"MAIN\"",
				"     1.00 s  alarm #1 took the screen — page 2 \"WATCH\", C (01:1003) = -3.0 (trips at ≤ -2, releases above -1.5)",
				"     5.60 s  alarm #1 over, the hold ran out — back to page 1 \"MAIN\"",
			]
		);
	}

	/// The panel the replay draws on its last frame, at `end_ms`, with channel 2 out from 1 s.
	fn panel_at(end_ms: u64) -> Canvas {
		let excursion = every_100ms(end_ms, |t| if t >= 1_000 { -3.0 } else { 0.0 });
		let mut replay = replay(excursion, &[], end_ms);
		while replay.step().is_some() {}
		let mut drawn = Canvas::new(WIDTH, HEIGHT);
		replay.draw(&mut drawn);
		drawn
	}

	/// What the board's panel task composes for the alarm page, drawn by the renderer itself.
	fn composed(inverted: bool) -> Canvas {
		let mut expected = Canvas::new(WIDTH, HEIGHT);
		let offending = Cell::new("C", Some(-3.0), "", 1);
		let offending = if inverted { offending.alarmed() } else { offending };
		let cells = [offending, Cell::new("D", Some(0.0), "", 1)];
		let board = Board {
			links: Links::NONE,
			rates: None,
		};
		draw_with(&Frame::Values { cells: &cells }, &board, &Theme::bold_mono(), &mut expected);
		expected
	}

	#[test]
	fn the_panel_is_the_boards_frame_with_the_offending_cell_blinking() {
		let text = crate::dashreplay::glass::half_blocks;
		// Out from 1.0 s, where the blink starts: 1.8 s is in an inverted half, 1.4 s in a plain one.
		assert_eq!(text(&panel_at(1_800)), text(&composed(true)), "the inverted half");
		assert_eq!(text(&panel_at(1_400)), text(&composed(false)), "the plain half is the plain page");
		// And the inversion is really there: the two halves are not the same picture.
		assert_ne!(text(&composed(true)), text(&composed(false)));
	}

	#[test]
	fn the_cell_blinks_while_out_and_holds_steady_through_the_hold() {
		// Out from 1 s, back inside from 3 s; the hold hands back at 5.6 s.
		let excursion = every_100ms(7_000, |t| if (1_000..3_000).contains(&t) { -3.0 } else { 0.0 });
		let mut replay = replay(excursion, &[], 7_000);
		let mut inverted = Vec::new();
		while let Some(tick) = replay.step() {
			inverted.push((tick.t_ms, tick.glass.inverted == Some(ChannelId(2))));
		}
		let out: Vec<bool> = inverted.iter().filter(|(t, _)| (1_000..3_000).contains(t)).map(|&(_, i)| i).collect();
		// Frames at 1.0, 1.2 … 2.8 s, 400 ms halves counted from the takeover: two frames a half.
		assert_eq!(out, [true, true, false, false, true, true, false, false, true, true]);
		let back = first_frame_from(3_000 + HOLD_MS);
		assert!(
			inverted.iter().filter(|(t, _)| (3_000..back).contains(t)).all(|&(_, i)| i),
			"steady through the hold: {inverted:?}"
		);
		assert!(inverted.iter().filter(|(t, _)| *t >= back).all(|&(_, i)| !i), "{inverted:?}");
	}

	#[test]
	fn a_plan_with_more_rules_than_the_board_holds_is_refused() {
		static FIVE: [Alarm<'static>; 5] = [RULES[0], RULES[0], RULES[0], RULES[0], RULES[0]];
		static TOO_MANY: Plan = Plan { alarms: &FIVE, ..PLAN };
		assert!(Replay::new(&TOO_MANY, vec![None; 4], vec![], 0, 1_000).is_err());
	}
}
