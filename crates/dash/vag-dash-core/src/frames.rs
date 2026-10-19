//! One frame of the panel, without the chip: what the board's panel task does each
//! [`FRAME_MS`], given the store's values and the time — and what the laptop's replay does at
//! each frame of a recording, by the same code.
//!
//! A frame is three steps, in this order, because the board takes its locks between them:
//!
//! 1. [`Frames::glass`]: the screen's answer — the page to draw, an alarm's takeover, the
//!    cells inverted this frame — over the store's values at the frame's time.
//! 2. [`Frames::step`]: what the frame moves — every chart samples its channel, shown or not,
//!    and a page an alarm names that the plan does not have is said once.
//! 3. [`Frames::picture`]: the page composed — its cells with what the plan makes of their
//!    values, its strip, or the stopwatch's row — ready to draw on any target with a
//!    [`Board`] of the caller's.
//!
//! What stays with the caller is what only it has: the clock, the locks, the links and the
//! fault count in the [`Board`], the log, the adapter screen, the stopwatch's machine and the
//! glass.

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::DrawTarget;
use vag_dash_render::alarm::{BLINK_MS, ChannelId, Slot as AlarmSlot};
use vag_dash_render::history::History;
use vag_dash_render::plan::{MAX_VALUE_CELLS, Page, Plan};
use vag_dash_render::render::Report;
use vag_dash_render::screen::{Glass, Screen};
use vag_dash_render::stopwatch::{self, Labels, MAX_MARKS, Stopwatch, Words};
use vag_dash_render::{Board, Cell, Deviation, Frame, Strip, Theme, draw_page};

use crate::panel::WIDTH;
use crate::values::{self, Slot};

/// How often the panel draws — and, because a chart's column is one frame, how much time one
/// column holds. Five a second: fast enough to look live over a terminal, slow enough that the
/// encoding never becomes the bottleneck.
pub const FRAME_MS: u64 = 200;
// An alarm's cell blinks in halves of `BLINK_MS` from its takeover: two frames a half, so a
// frame that takes as long to draw as the wait before it still lands in every half.
const _: () = assert!(FRAME_MS * 2 <= BLINK_MS, "the panel must frame at least twice per half blink");

/// What one chart column stands for, in seconds.
const SECONDS_PER_SAMPLE: f32 = FRAME_MS as f32 / 1000.0;

/// The stopwatch on the glass this frame: its machine as it stands, and the last finished run
/// the settings keep, `(mark in km/h, seconds)`.
#[derive(Clone, Copy)]
pub struct StopwatchView<'a> {
	pub watch: Stopwatch<'a>,
	pub saved: &'a [(u16, f32)],
}

/// What a frame moved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
	/// The page an alarm names that the plan does not have, on the first frame of the miss —
	/// for the caller to say once; `None` on every other frame.
	pub missed: Option<u16>,
}

/// The frame loop's own state: a history per chart, which page miss was said, and the
/// stopwatch page's words.
///
/// `H` holds the histories, one per chart of the plan in its order: a static array on the
/// board, which keeps their kilobytes out of the task arena; a `Vec` on a laptop.
pub struct Frames<H> {
	plan: &'static Plan,
	histories: H,
	missed_said: Option<u16>,
	words: Words,
	labels: Labels,
}

impl<H: AsRef<[History<WIDTH>]> + AsMut<[History<WIDTH>]>> Frames<H> {
	/// # Panics
	///
	/// When `histories` is not one per chart of `plan`: both are sized from the same plan (the
	/// board's array by `CHART_COUNT`), so a mismatch is an image built wrong.
	pub fn new(plan: &'static Plan, histories: H) -> Self {
		assert_eq!(histories.as_ref().len(), plan.chart_count(), "one history per chart of the plan");
		Frames {
			plan,
			histories,
			missed_said: None,
			words: Words::of(plan.language),
			labels: Labels::new(plan.stopwatch.map_or(&[][..], |stopwatch| stopwatch.marks)),
		}
	}

	pub fn histories(&self) -> &[History<WIDTH>] {
		self.histories.as_ref()
	}

	/// The screen's answer for the frame at `now_us`: the page cursor `cursor`, the plan's
	/// alarms over `slots` as they are at that moment ([`values::current`]).
	pub fn glass<S>(&self, screen: &mut Screen<'_, S>, cursor: u8, slots: &[Slot], now_us: u64) -> Glass
	where
		S: AsRef<[AlarmSlot]> + AsMut<[AlarmSlot]>,
	{
		// The generator refuses more than `MAX_PAGES`, so the count fits.
		let pages = self.plan.pages.len() as u8;
		screen.frame(cursor, pages, now_us / 1000, |index| values::current(slots, index, now_us))
	}

	/// What the frame `glass` moves: every chart samples its channel at `now_us`, whether or
	/// not a chart is on the glass, so a chart page comes up with its last `WIDTH × FRAME_MS`
	/// already drawn; and a page miss is said on its first frame. `None`, moving nothing, when
	/// the plan has no page `glass` names — the screen never answers one, and the board then
	/// draws nothing that frame.
	pub fn step(&mut self, glass: &Glass, slots: &[Slot], now_us: u64) -> Option<Step> {
		self.plan.pages.get(usize::from(glass.page))?;
		let missed = glass.missed.map(|page| page.0);
		let said = (missed != self.missed_said).then_some(missed).flatten();
		self.missed_said = missed;
		let histories = self.histories.as_mut();
		for chart in self.plan.charts() {
			histories[chart.slot].push(chart.channel, values::current(slots, chart.channel, now_us));
		}
		Some(Step { missed: said })
	}

	/// The page `glass` names, composed from `slots` at `now_us`; the stopwatch's row instead
	/// when `stopwatch` is on the glass. `None` when the plan has no such page.
	///
	/// A cell's value is what the plan shows of it ([`Plan::shown`]: the stopwatch's speed
	/// rounded down, a `zero_at_rest` channel 0 while the car stands, decided once for the
	/// frame), with its difference from a paired specified value; a channel the plan cannot
	/// name draws as `?` rather than vanishing; every channel an alarm on the glass points at
	/// is inverted on the frames [`Glass::inverted`] names.
	pub fn picture(&self, glass: &Glass, slots: &[Slot], now_us: u64, stopwatch: Option<StopwatchView<'_>>) -> Option<Picture<'_>> {
		let page = self.plan.pages.get(usize::from(glass.page))?;
		let plan = self.plan;
		let value_of = |index: u16| values::current(slots, index, now_us);
		if let Some(view) = stopwatch {
			let speed = plan
				.stopwatch
				.and_then(|stopwatch| value_of(stopwatch.speed).map(|value| value * stopwatch.km_h_per_unit));
			let (cells, count) = stopwatch::cells(&view.watch, speed, view.saved, &self.words, &self.labels);
			return Some(Picture {
				strip: Strip::NONE,
				body: Body::Stopwatch { cells, count },
			});
		}
		let strip = plan.strip(usize::from(glass.page), value_of);
		let stands = plan.stands(value_of);
		let cell_of = |index: u16| {
			let cell = match plan.channel(index) {
				Some(channel) => {
					// One of the two missing is `Unknown`, which keeps the line and draws a dash.
					let deviation = match channel.setpoint {
						None => Deviation::None,
						Some(specified) => match (value_of(index), value_of(specified)) {
							(Some(actual), Some(wanted)) => Deviation::Value(actual - wanted),
							_ => Deviation::Unknown,
						},
					};
					let shown = plan.shown(index, value_of(index), stands);
					Cell::new(channel.label, shown, channel.unit_text, channel.decimals).with_deviation(deviation)
				}
				None => Cell::new("?", None, "", 0),
			};
			if glass.inverted.contains(ChannelId(index)) {
				cell.alarmed()
			} else {
				cell
			}
		};
		let body = match *page {
			Page::Values { cells: indices, .. } => {
				let mut cells = heapless::Vec::new();
				for &index in indices.iter().take(MAX_VALUE_CELLS) {
					let _ = cells.push(cell_of(index));
				}
				Body::Values(cells)
			}
			// Its range and its history's slot are the page's, as the generator joined them
			// (`Plan::joins_hold` checks the slot when the image is built).
			Page::Chart { channel, slot, min, max, .. } => Body::Chart {
				cell: cell_of(channel),
				min,
				max,
				samples: self.histories.as_ref()[usize::from(slot)].samples(),
			},
		};
		Some(Picture { strip, body })
	}
}

/// One frame's page, composed: what [`Frames::picture`] answers.
pub struct Picture<'a> {
	/// The page's title and voltage across the top ([`Plan::strip`]); neither over the
	/// stopwatch's row.
	pub strip: Strip<'static>,
	body: Body<'a>,
}

enum Body<'a> {
	Values(heapless::Vec<Cell<'a>, MAX_VALUE_CELLS>),
	Chart {
		cell: Cell<'a>,
		min: f32,
		max: f32,
		samples: &'a [f32],
	},
	Stopwatch {
		cells: [Cell<'a>; 1 + MAX_MARKS],
		count: usize,
	},
}

impl Picture<'_> {
	/// The page as the renderer takes it.
	pub fn frame(&self) -> Frame<'_> {
		match &self.body {
			Body::Values(cells) => Frame::Values { cells },
			Body::Chart { cell, min, max, samples } => Frame::Chart {
				cell: *cell,
				min: *min,
				max: *max,
				samples,
				seconds_per_sample: SECONDS_PER_SAMPLE,
			},
			Body::Stopwatch { cells, count } => Frame::Values { cells: &cells[..*count] },
		}
	}

	/// Whether this is the stopwatch's row rather than a page of the plan.
	pub fn is_stopwatch(&self) -> bool {
		matches!(self.body, Body::Stopwatch { .. })
	}

	/// Draws the frame on `target` under its strip, with `board`'s links and fault count in it —
	/// the stopwatch's row under a strip with no title or voltage, which is `draw_with`'s. What
	/// the renderer had to compromise is the answer.
	pub fn draw<D: DrawTarget<Color = BinaryColor>>(&self, board: &Board, theme: &Theme, target: &mut D) -> Report {
		draw_page(&self.frame(), &self.strip, board, theme, target)
	}
}

#[cfg(test)]
mod tests {
	extern crate std;
	use super::*;
	use crate::panel::Framebuffer;
	use crate::values::{STALE_MS, fresh_for_ms};
	use std::vec;
	use std::vec::Vec;
	use vag_dash_render::alarm::{Alarm, Bound, PageId, Rule};
	use vag_dash_render::plan::{Channel, Standstill, StopwatchPlan, Unit};
	use vag_dash_render::{Links, Volts, draw_with};

	const fn channel(did: u16, label: &'static str) -> Channel {
		Channel {
			unit: 0,
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
			hz: 10.0,
			setpoint: None,
			zero_at_rest: false,
		}
	}

	/// Neutral numbers. Channel 0 and 1 on the driver's page; 2 is out at or below -2 and
	/// raises page 2; 3 is charted on page 3; 4 is the speed, on no page; 5 is the strip's
	/// voltage; 6 is what 1 is asked to be; 7 reads 0 at a standstill.
	static CHANNELS: [Channel; 8] = [
		channel(0x1001, "A"),
		Channel {
			setpoint: Some(6),
			..channel(0x1002, "B")
		},
		channel(0x1003, "C"),
		channel(0x1004, "D"),
		channel(0x1005, "SPEED"),
		channel(0x1006, "V"),
		channel(0x1007, "B SET"),
		Channel {
			zero_at_rest: true,
			..channel(0x1008, "G")
		},
	];
	static PAGES: [Page; 3] = [
		Page::Values {
			title: "MAIN",
			cells: &[0, 1, 7, 99, 2],
			volts: Some(5),
		},
		Page::Values {
			title: "WATCH",
			cells: &[2, 3],
			volts: None,
		},
		Page::Chart {
			title: "D",
			channel: 3,
			slot: 0,
			min: -5.0,
			max: 5.0,
			volts: None,
		},
	];
	static ALARMS: [Alarm; 1] = [Alarm {
		channel: ChannelId(2),
		page: PageId(1),
		rule: Rule::Bounds {
			max: None,
			min: Some(Bound { at: -2.0, off: -1.5 }),
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
		alarms: &ALARMS,
		stalk: None,
		stopwatch: Some(StopwatchPlan {
			speed: 4,
			km_h_per_unit: 1.0,
			marks: &[100],
		}),
		buttons: &[],
		standstill: Some(Standstill::Speed(4)),
	};

	type Board1 = Screen<'static, Vec<AlarmSlot>>;

	fn screen() -> Board1 {
		Screen::new(PLAN.alarms, PLAN.pages, vec![AlarmSlot::CLEAR; PLAN.alarms.len()], true)
	}

	fn frames() -> Frames<Vec<History<WIDTH>>> {
		Frames::new(&PLAN, (0..PLAN.chart_count()).map(|_| History::new()).collect())
	}

	/// Every channel heard at `at_ms` with `value(index)`, read at the board's hidden rate.
	fn heard(at_ms: u64, value: impl Fn(usize) -> Option<f32>) -> Vec<Slot> {
		(0..CHANNELS.len())
			.map(|i| Slot::heard(value(i), at_ms * 1000, fresh_for_ms(1_000)))
			.collect()
	}

	fn values(picture: &Picture<'_>) -> Vec<Cell<'static>> {
		match picture.frame() {
			Frame::Values { cells } => cells.iter().map(|c| Cell { label: "", unit: "", ..*c }).collect(),
			_ => panic!("not a values page"),
		}
	}

	#[test]
	fn the_boards_numbers() {
		assert_eq!((FRAME_MS, STALE_MS), (200, 5_000));
		assert_eq!(SECONDS_PER_SAMPLE, 0.2, "a chart column is one frame");
	}

	#[test]
	fn a_frame_reads_the_store_at_its_own_time_and_a_stale_value_is_a_dash() {
		let frames = frames();
		let slots = heard(0, |i| (i == 0).then_some(12.5));
		let at = |now_us: u64| {
			let glass = frames.glass(&mut screen(), 0, &slots, now_us);
			values(&frames.picture(&glass, &slots, now_us, None).unwrap())[0].value
		};
		assert_eq!(at(STALE_MS * 1000), Some(12.5), "STALE old is shown");
		assert_eq!(at(STALE_MS * 1000 + 1), None, "a microsecond past is a dash");
	}

	#[test]
	fn the_page_is_composed_as_the_board_draws_it() {
		let (mut screen, frames) = (screen(), frames());
		// Moving: B is 0.6 against the 0.5 asked, G its sensor's 0.3; channel 99 has no name.
		let slots = heard(0, |i| {
			[Some(1.0), Some(0.6), Some(0.3), None, Some(12.0), Some(14.2), Some(0.5), Some(0.3)][i]
		});
		let glass = frames.glass(&mut screen, 0, &slots, 0);
		let picture = frames.picture(&glass, &slots, 0, None).unwrap();
		assert!(!picture.is_stopwatch());
		assert_eq!(
			picture.strip,
			Strip {
				title: "MAIN",
				volts: Volts::Shown(Some(14.2))
			}
		);
		let Frame::Values { cells } = picture.frame() else { panic!() };
		assert_eq!(cells.len(), MAX_VALUE_CELLS, "a page's fifth cell is not drawn");
		assert_eq!(cells[0], Cell::new("A", Some(1.0), "", 1));
		assert_eq!(cells[1].label, "B");
		let Deviation::Value(difference) = cells[1].deviation else {
			panic!("{:?}", cells[1])
		};
		assert!((difference - 0.1).abs() < 1e-6, "{difference}");
		assert_eq!(cells[2], Cell::new("G", Some(0.3), "", 1), "rolling: the sensor's");
		assert_eq!(cells[3], Cell::new("?", None, "", 0), "a channel the plan cannot name");

		// Standing: G is 0, and B without what it was asked for is an unknown difference.
		let slots = heard(0, |i| [Some(1.0), Some(0.6), Some(0.3), None, Some(0.0), None, None, Some(0.3)][i]);
		let picture = frames.picture(&glass, &slots, 0, None).unwrap();
		let cells = values(&picture);
		assert_eq!((cells[1].deviation, cells[2].value), (Deviation::Unknown, Some(0.0)));
		assert_eq!(picture.strip.volts, Volts::Shown(None), "the voltage's dash");
	}

	#[test]
	fn an_alarm_takes_the_glass_and_its_cell_blinks_two_frames_a_half() {
		let (mut screen, mut frames) = (screen(), frames());
		let mut seen = Vec::new();
		for frame in 0..20u64 {
			let t_ms = frame * FRAME_MS;
			let slots = heard(t_ms, |i| match i {
				2 if t_ms >= 1_000 => Some(-3.0),
				_ => Some(0.0),
			});
			let glass = frames.glass(&mut screen, 0, &slots, t_ms * 1000);
			frames.step(&glass, &slots, t_ms * 1000).unwrap();
			let picture = frames.picture(&glass, &slots, t_ms * 1000, None).unwrap();
			let cells = values(&picture);
			seen.push((t_ms, glass.page, cells[0].alarm));
		}
		assert!(seen.iter().filter(|(t, ..)| *t < 1_000).all(|&(_, page, _)| page == 0));
		let out: Vec<bool> = seen
			.iter()
			.filter(|(t, ..)| *t >= 1_000)
			.map(|&(_, page, inverted)| page == 1 && inverted)
			.collect();
		assert_eq!(
			out[..8],
			[true, true, false, false, true, true, false, false],
			"BLINK_MS halves from the takeover"
		);
		assert!(seen.iter().filter(|(t, ..)| *t >= 1_000).all(|&(_, page, _)| page == 1));
	}

	#[test]
	fn every_chart_samples_every_frame_shown_or_not_and_a_stale_value_ends_its_trace() {
		let (mut screen, mut frames) = (screen(), frames());
		for frame in 0..5u64 {
			let now_us = frame * FRAME_MS * 1000;
			let slots = heard(0, |i| (i == 3).then_some(frame as f32));
			let glass = frames.glass(&mut screen, 0, &slots, now_us);
			assert_eq!(glass.page, 0, "the chart is not on the glass");
			assert_eq!(frames.step(&glass, &slots, now_us), Some(Step { missed: None }));
		}
		assert_eq!(frames.histories()[0].samples(), [0.0, 1.0, 2.0, 3.0, 4.0]);

		// On the glass, the chart draws that history at one frame a column.
		let slots = heard(0, |i| (i == 3).then_some(4.0));
		let glass = frames.glass(&mut screen, 2, &slots, 1_000_000);
		assert_eq!(glass.page, 2);
		{
			let picture = frames.picture(&glass, &slots, 1_000_000, None).unwrap();
			let Frame::Chart {
				cell,
				min,
				max,
				samples,
				seconds_per_sample,
			} = picture.frame()
			else {
				panic!()
			};
			assert_eq!(
				(cell, min, max, samples, seconds_per_sample),
				(Cell::new("D", Some(4.0), "", 1), -5.0, 5.0, &[0.0, 1.0, 2.0, 3.0, 4.0][..], 0.2)
			);
		}

		// Heard at 0 and never again: past its five seconds the trace ends.
		let late = (STALE_MS + 1) * 1000;
		let glass = frames.glass(&mut screen, 2, &slots, late);
		frames.step(&glass, &slots, late).unwrap();
		assert!(frames.histories()[0].samples().is_empty());
	}

	#[test]
	fn a_page_the_plan_does_not_have_moves_nothing_and_a_miss_is_said_once() {
		let (mut screen, mut frames) = (screen(), frames());
		let slots = heard(0, |_| Some(0.0));
		let glass = frames.glass(&mut screen, 0, &slots, 0);
		let nowhere = Glass { page: 9, ..glass };
		assert_eq!(frames.step(&nowhere, &slots, 0), None);
		assert!(frames.histories()[0].samples().is_empty(), "nothing sampled");
		assert!(frames.picture(&nowhere, &slots, 0, None).is_none());

		let missed = Glass {
			missed: Some(PageId(7)),
			..glass
		};
		let said: Vec<_> = [missed, missed, glass, missed]
			.iter()
			.map(|g| frames.step(g, &slots, 0).unwrap().missed)
			.collect();
		assert_eq!(said, [Some(7), None, None, Some(7)], "on the first frame of each miss");
	}

	#[test]
	fn the_stopwatch_row_draws_under_an_empty_strip_with_the_speed_in_km_h() {
		let (mut screen, frames) = (screen(), frames());
		let slots = heard(0, |i| (i == 4).then_some(42.7));
		let glass = frames.glass(&mut screen, 0, &slots, 0);
		let watch = Stopwatch::new(&[100], 1.0);
		let saved = [(100, 9.87)];
		let picture = frames.picture(&glass, &slots, 0, Some(StopwatchView { watch, saved: &saved })).unwrap();
		assert!(picture.is_stopwatch());
		assert_eq!(picture.strip, Strip::NONE);
		let words = Words::of("en");
		let labels = Labels::new(&[100]);
		let (want, count) = stopwatch::cells(&watch, Some(42.7), &saved, &words, &labels);
		let Frame::Values { cells } = picture.frame() else { panic!() };
		assert_eq!(cells, &want[..count]);

		// Drawn as the board draws it: under a strip with the board's links and no title.
		let board = Board {
			links: Links { usb: true, ble: false },
			..Board::default()
		};
		let (mut drawn, mut expected) = (Framebuffer::new(), Framebuffer::new());
		picture.draw(&board, &Theme::bold_mono(), &mut drawn);
		draw_with(&Frame::Values { cells: &want[..count] }, &board, &Theme::bold_mono(), &mut expected);
		assert!(drawn.rows().eq(expected.rows()));
	}

	#[test]
	fn a_page_draws_under_its_strip() {
		let (mut screen, frames) = (screen(), frames());
		let slots = heard(0, |i| Some(i as f32));
		let glass = frames.glass(&mut screen, 1, &slots, 0);
		let picture = frames.picture(&glass, &slots, 0, None).unwrap();
		let (mut drawn, mut expected) = (Framebuffer::new(), Framebuffer::new());
		let report = picture.draw(&Board::default(), &Theme::bold_mono(), &mut drawn);
		let cells = [Cell::new("C", Some(2.0), "", 1), Cell::new("D", Some(3.0), "", 1)];
		let strip = Strip {
			title: "WATCH",
			volts: Volts::Hidden,
		};
		assert_eq!(
			report,
			draw_page(
				&Frame::Values { cells: &cells },
				&strip,
				&Board::default(),
				&Theme::bold_mono(),
				&mut expected
			)
		);
		assert!(drawn.rows().eq(expected.rows()));
	}

	#[test]
	#[should_panic(expected = "one history per chart")]
	fn histories_are_one_per_chart() {
		let _ = Frames::new(&PLAN, Vec::<History<WIDTH>>::new());
	}
}
