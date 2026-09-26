//! What is on the glass: the page the driver chose, unless an alarm took it.
//!
//! Three things meet here — the page cursor, the plan's alarms, and the one
//! button — and the firmware only feeds them: the time, the latest value of a
//! channel, a press. It lives in this crate for the reason [`alarm`](crate::alarm)
//! and [`button`](crate::button) do: the firmware cannot be built for the host,
//! and the rules between the three are exactly what a person on the car would
//! otherwise report as "the button did nothing".
//!
//! **The cursor is not stored here.** It is the board's configuration — saved to
//! flash, set over BLE, reported in `state` — and a second copy would drift from
//! it. [`Screen::frame`] reads it and [`Screen::press`] moves it, both through
//! the caller's own value.
//!
//! **The page on the glass is what the bus reads in the foreground.** During a
//! takeover that is the alarm's page, not the cursor's: its cells are the ones
//! being looked at. [`Glass::page_changed`] is when that set moves — a takeover
//! and a hand-back as much as a page turn.
//!
//! **The adapter screen runs no alarms.** While the board is a plain CAN adapter
//! the planner sends nothing, so nothing is read and nothing could trip; and the
//! glass shows the adapter's counters, so a press there has no alarm to silence
//! and turns the page as it always has.

use crate::alarm::{Alarm, Alarms, ChannelId, Highlight, PageId, Press, Shown};
use crate::pages;
use crate::stalk::Lever;

/// What one frame did to the glass that is worth one line in the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
	/// Rule `rule` — its place in the plan — took the glass, from the driver's
	/// page or from another rule.
	Took { rule: usize },
	/// The alarm's hold ran out, and the glass is back on the driver's page.
	Over,
	/// A press silenced the alarm, and the glass is back on the driver's page —
	/// with the value possibly still out, which is why it is not [`Change::Over`].
	Silenced,
}

/// One frame's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glass {
	/// The page to draw: an index into the plan's pages, which are the board's.
	pub page: u8,
	/// The channel the alarm on the glass points at — what the log names. Whether its
	/// cell is inverted on this frame is [`Glass::inverted`].
	pub offending: Option<ChannelId>,
	/// How the offending cell is drawn: blinking from the takeover while the rule fires,
	/// steady through the hold after the release. `Some` exactly when `offending` is.
	pub highlight: Option<Highlight>,
	/// The channel whose cell is drawn inverted **on this frame**: the offending one
	/// on the inverted half of a blink ([`BLINK_MS`](crate::alarm::BLINK_MS)) and all
	/// through the hold, otherwise none.
	pub inverted: Option<ChannelId>,
	/// The page differs from the one the last frame drew — the first frame, a
	/// page turn, a takeover, a hand-back, the first frame after the adapter
	/// screen. The channels read in the foreground follow the page on the glass,
	/// so this is when they move.
	pub page_changed: bool,
	/// The alarm's page is not among the `pages` the board holds, so the driver's
	/// page is drawn instead. The generator refuses such a plan; this keeps the
	/// glass live if one is flashed anyway rather than freezing it.
	pub missed: Option<PageId>,
	pub change: Option<Change>,
}

/// What a press of the cruise lever did (`todo/dash/19`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
	/// The cursor moved to the next or the previous page.
	Paged,
	/// An alarm held the glass, and the press silenced it — whichever way the
	/// lever went, as the button's short press does there.
	Silenced,
	/// The stopwatch took the glass, from whatever page the cursor is on.
	StopwatchOn,
	/// The stopwatch left the glass, back to the page the cursor is on — the one it
	/// came from, since nothing moves the cursor while it is up.
	StopwatchOff,
	/// Nothing: a page turn while the stopwatch is up, or the adapter screen.
	Ignored,
}

/// The plan's alarms and what the glass showed last.
///
/// `N` is the plan's number of rules, fixed when the image is built.
pub struct Screen<'a, const N: usize> {
	alarms: Alarms<'a, N>,
	/// The last frame was the adapter's, not a page.
	adapter: bool,
	/// The page the last page frame drew: `None` before the first and after the
	/// adapter screen.
	drawn: Option<u8>,
	/// The rule whose alarm the last frame showed.
	rule: Option<usize>,
	/// A press silenced an alarm since the last frame.
	silenced: bool,
	/// The stopwatch mode is on. Not a page: the cursor stays where it was.
	stopwatch: bool,
	/// How many times the stopwatch mode has turned on or off, wrapping.
	turns: u16,
}

impl<'a, const N: usize> Screen<'a, N> {
	/// The plan's rules, all `N` of them, in priority order.
	///
	/// # Panics
	///
	/// When `rules` is not `N` long: `N` is sized from the same plan, so a
	/// mismatch is an image built wrong, not a car behaving oddly.
	pub fn new(rules: &'a [Alarm<'a>]) -> Self {
		assert_eq!(rules.len(), N, "the screen is sized for the plan's alarms");
		Screen {
			alarms: Alarms::new(core::array::from_fn(|i| rules[i])),
			adapter: false,
			drawn: None,
			rule: None,
			silenced: false,
			stopwatch: false,
			turns: 0,
		}
	}

	/// One frame of a page. `cursor` is the page the driver paged to and `pages`
	/// how many pages the board holds; `value_of` answers the latest value of a
	/// plan channel by index, `None` where it is stale or never answered.
	pub fn frame(&mut self, cursor: u8, pages: u8, now_ms: u64, value_of: impl Fn(u16) -> Option<f32>) -> Glass {
		self.adapter = false;
		let shown = self
			.alarms
			.poll_with(PageId(u16::from(cursor)), |channel: ChannelId| value_of(channel.0), now_ms)
			.shown;
		let rule = self.alarms.showing();

		// With no alarm up the shown page is the cursor, which is the caller's to
		// keep in range; an alarm's page is the plan's, and is checked here.
		let (page, shown, missed) = match u8::try_from(shown.page.0) {
			Ok(page) if rule.is_none() || page < pages => (page, shown, None),
			_ => (cursor, Shown::page(PageId(u16::from(cursor))), Some(shown.page)),
		};

		let change = match (self.rule, rule) {
			(before, Some(now)) if before != Some(now) => Some(Change::Took { rule: now }),
			(Some(_), None) if self.silenced => Some(Change::Silenced),
			(Some(_), None) => Some(Change::Over),
			_ => None,
		};
		self.rule = rule;
		self.silenced = false;

		let page_changed = self.drawn != Some(page);
		self.drawn = Some(page);
		Glass {
			page,
			offending: shown.offending,
			highlight: shown.highlight,
			inverted: shown.inverted(now_ms),
			page_changed,
			missed,
			change,
		}
	}

	/// One frame of the adapter screen, in place of [`Screen::frame`]. The alarms
	/// are not polled: their state waits, and the first page frame after resumes
	/// it — as a changed page, since the glass showed no page meanwhile.
	///
	/// The stopwatch mode ends here. The adapter's screen is not the stopwatch,
	/// nothing is read meanwhile, and a run left open across it would be timed from
	/// a launch minutes old; [`Screen::stopwatch_turns`] counts the mode ending.
	pub fn adapter(&mut self) {
		self.adapter = true;
		self.drawn = None;
		self.set_stopwatch(false);
		self.alarms.glass_lost();
	}

	/// A short press, with `pages` the number of pages the cursor runs over.
	///
	/// While an alarm owns the glass the press silences it and the cursor stays;
	/// otherwise the cursor moves on, wrapping. What it did is returned, so the
	/// caller can say so.
	///
	/// The button keeps its job while the stopwatch is up (`todo/dash/19`): the
	/// press turns the page, and with a page on the glass the stopwatch is off.
	/// That makes it the bench's way out, where no lever is read, and the way out
	/// on a car whose `measure` state was named wrong. [`Screen::stopwatch_turns`]
	/// counts the mode ending, whichever way it ended.
	pub fn press(&mut self, cursor: &mut u8, pages: u8) -> Press {
		let press = if self.adapter { Press::NextPage } else { self.alarms.press() };
		match press {
			Press::NextPage => {
				self.set_stopwatch(false);
				*cursor = pages::next(*cursor, pages);
			}
			Press::Silenced => self.silenced = true,
		}
		press
	}

	/// A press of the cruise lever, with `pages` the number of pages the cursor
	/// runs over. Alarms first: while one holds the glass any press silences it.
	/// Otherwise `next` and `previous` turn the page, wrapping, and `measure`
	/// switches the stopwatch on or off; while the stopwatch is up the page turns
	/// are ignored, so leaving it lands on the page it came from.
	///
	/// On the adapter screen nothing is polled, so no lever is read; a press that
	/// arrives anyway is ignored.
	pub fn lever(&mut self, lever: Lever, cursor: &mut u8, pages: u8) -> Action {
		if self.adapter {
			return Action::Ignored;
		}
		if self.alarms.press() == Press::Silenced {
			self.silenced = true;
			return Action::Silenced;
		}
		match (lever, self.stopwatch) {
			(Lever::Measure, false) => {
				self.set_stopwatch(true);
				Action::StopwatchOn
			}
			(Lever::Measure, true) => {
				self.set_stopwatch(false);
				Action::StopwatchOff
			}
			(Lever::Next | Lever::Previous, true) => Action::Ignored,
			(Lever::Next, false) => {
				*cursor = pages::next(*cursor, pages);
				Action::Paged
			}
			(Lever::Previous, false) => {
				*cursor = pages::previous(*cursor, pages);
				Action::Paged
			}
		}
	}

	/// Whether an alarm holds the glass — where a press, the button's or the lever's, silences.
	pub fn alarm_showing(&self) -> bool {
		self.alarms.showing().is_some()
	}

	/// Whether the stopwatch mode is on.
	pub fn stopwatch(&self) -> bool {
		self.stopwatch
	}

	/// How many times the stopwatch mode has turned on or off, wrapping. A run belongs
	/// to one stretch of the mode: whoever holds the stopwatch hands it this count
	/// (`Stopwatch::follow`), and a count it has not seen resets it — at the turn, not
	/// on the next frame, so an off and an on between two frames still start over.
	pub fn stopwatch_turns(&self) -> u16 {
		self.turns
	}

	fn set_stopwatch(&mut self, on: bool) {
		if self.stopwatch != on {
			self.stopwatch = on;
			self.turns = self.turns.wrapping_add(1);
		}
	}

	/// Whether the stopwatch is what the glass shows: the mode is on and no alarm
	/// holds the glass. The caller draws the stopwatch page in place of
	/// [`Glass::page`] exactly then; alarms take the glass during the stopwatch too.
	pub fn stopwatch_on_glass(&self) -> bool {
		self.stopwatch && self.alarms.showing().is_none()
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::alarm::{BLINK_MS, Direction, HOLD_MS, Rule};
	use crate::plan::{Channel, Page, Plan};

	/// Pages: 0 and 1 are the driver's, 2 explains `HIGH`, 3 explains `LOW`.
	const PAGES: u8 = 4;
	static HIGH_CHANNELS: [ChannelId; 2] = [ChannelId(4), ChannelId(5)];
	static LOW_CHANNELS: [ChannelId; 1] = [ChannelId(6)];
	/// Neutral numbers: a rule that fires high and one that fires low.
	static RULES: [Alarm<'static>; 2] = [
		Alarm {
			channels: &HIGH_CHANNELS,
			page: PageId(2),
			rule: Rule::Threshold {
				trip: 10.0,
				release: 8.0,
				direction: Direction::Above,
			},
		},
		Alarm {
			channels: &LOW_CHANNELS,
			page: PageId(3),
			rule: Rule::Threshold {
				trip: 0.0,
				release: 1.0,
				direction: Direction::Below,
			},
		},
	];

	/// The latest value of each of seven channels, as the board's store holds it.
	#[derive(Clone, Copy)]
	struct Car([Option<f32>; 7]);

	impl Car {
		fn calm() -> Self {
			Car([Some(0.0), Some(0.0), Some(0.0), Some(0.0), Some(5.0), Some(5.0), Some(5.0)])
		}
		fn with(mut self, channel: usize, value: Option<f32>) -> Self {
			self.0[channel] = value;
			self
		}
		fn value_of(self) -> impl Fn(u16) -> Option<f32> {
			move |i| self.0.get(usize::from(i)).copied().flatten()
		}
	}

	fn screen() -> Screen<'static, 2> {
		Screen::new(&RULES)
	}

	#[test]
	fn a_channel_on_a_hidden_page_takes_the_screen_with_its_page_and_inverts_its_cell() {
		let mut screen = screen();
		let cursor = 0;
		let first = screen.frame(cursor, PAGES, 0, Car::calm().value_of());
		assert_eq!((first.page, first.offending, first.change), (0, None, None));
		assert!(first.page_changed, "nothing was on the glass before");
		// Channel 5 is on page 2, which nobody is looking at.
		let took = screen.frame(cursor, PAGES, 200, Car::calm().with(5, Some(12.0)).value_of());
		assert_eq!(took.page, 2, "the page that explains it");
		assert_eq!(took.offending, Some(ChannelId(5)), "the cell to invert, by plan index");
		assert!(took.page_changed, "a takeover moves what the bus reads in the foreground");
		assert_eq!(took.change, Some(Change::Took { rule: 0 }));
		let still = screen.frame(cursor, PAGES, 300, Car::calm().with(5, Some(12.0)).value_of());
		assert!(!still.page_changed && still.change.is_none());
		// Back inside, the hold runs out and the glass returns to the cursor.
		screen.frame(cursor, PAGES, 400, Car::calm().value_of());
		let back = screen.frame(cursor, PAGES, 400 + HOLD_MS, Car::calm().value_of());
		assert_eq!((back.page, back.offending, back.change), (0, None, Some(Change::Over)));
		assert!(back.page_changed, "so does the hand-back");
	}

	#[test]
	fn the_page_on_the_glass_is_the_one_read_in_the_foreground_during_a_takeover() {
		const CH: Channel = Channel {
			unit: 0,
			did: 0,
			bit_offset: 0,
			bit_length: 8,
			signed: false,
			big_endian: true,
			factor: 1.0,
			offset: 0.0,
			decimals: 0,
			unit_text: "",
			label: "",
			proven: false,
			hz: 10.0,
			setpoint: None,
		};
		static CHANNELS: [Channel; 7] = [CH; 7];
		// The alarm page shows channel 3, which no rule watches, beside its two.
		static PLAN_PAGES: [Page; 4] = [
			Page::Values { title: "a", cells: &[0, 1] },
			Page::Values { title: "b", cells: &[2] },
			Page::Values {
				title: "high",
				cells: &[3, 4, 5],
			},
			Page::Values { title: "low", cells: &[6] },
		];
		let plan = Plan {
			vin: "",
			language: "en",
			units: &[],
			channels: &CHANNELS,
			pages: &PLAN_PAGES,
			alarms: &RULES,
			stalk: None,
			stopwatch: None,
		};
		let cells = |page: u8| match plan.pages[usize::from(page)] {
			Page::Values { cells, .. } => cells,
			Page::Chart { .. } => unreachable!("the fixture has none"),
		};
		let listed = [0, 1, 2, 3, 4, 5, 6];
		let foreground = |page: u8| -> std::vec::Vec<u16> { plan.rates(cells(page), &listed).filter(|r| r.foreground).map(|r| r.channel).collect() };

		let mut screen = screen();
		let glass = screen.frame(0, PAGES, 0, Car::calm().value_of());
		assert_eq!(foreground(glass.page), [0, 1, 4, 5, 6], "the driver's page and every watched channel");
		let glass = screen.frame(0, PAGES, 200, Car::calm().with(4, Some(15.0)).value_of());
		assert!(glass.page_changed);
		assert_eq!(
			foreground(glass.page),
			[3, 4, 5, 6],
			"the alarm page's unwatched cell is read at its rate; the cursor page, not drawn, is not"
		);
	}

	#[test]
	fn an_alarm_page_the_board_does_not_hold_draws_the_drivers_page_rather_than_freezing() {
		let mut screen = screen();
		let out = Car::calm().with(5, Some(12.0));
		// A board holding two pages, and a rule raising page 2.
		let glass = screen.frame(1, 2, 0, out.value_of());
		assert_eq!((glass.page, glass.offending, glass.missed), (1, None, Some(PageId(2))));
		assert_eq!(screen.frame(1, 2, 200, out.value_of()).page, 1);
		// It is still an episode: the button silences it.
		assert_eq!(screen.press(&mut 1, 2), Press::Silenced);
		assert_eq!(screen.frame(1, 2, 400, out.value_of()).missed, None);
	}

	#[test]
	fn a_press_silences_the_episode_and_a_release_arms_the_rule_again() {
		let mut screen = screen();
		let mut cursor = 1;
		let out = Car::calm().with(4, Some(11.0));
		assert_eq!(screen.frame(cursor, PAGES, 0, out.value_of()).page, 2);
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced);
		assert_eq!(cursor, 1, "silencing is not a page turn");
		// Still out, and silent: the driver's page, said as silenced and not as over.
		let silenced = screen.frame(cursor, PAGES, 200, out.value_of());
		assert_eq!((silenced.page, silenced.change), (1, Some(Change::Silenced)));
		assert_eq!(screen.frame(cursor, PAGES, 400, out.value_of()).change, None);
		// A release re-arms without taking the screen...
		assert_eq!(screen.frame(cursor, PAGES, 600, Car::calm().value_of()).page, 1);
		// ...and the next crossing takes it again.
		let again = screen.frame(cursor, PAGES, 800, out.value_of());
		assert_eq!((again.page, again.change), (2, Some(Change::Took { rule: 0 })));
	}

	#[test]
	fn the_cursor_moves_only_on_a_page_turn_and_the_hold_hands_back_to_where_it_is_now() {
		let mut screen = screen();
		let mut cursor = 0;
		screen.frame(cursor, PAGES, 0, Car::calm().value_of());
		assert_eq!(screen.press(&mut cursor, PAGES), Press::NextPage);
		assert_eq!(cursor, 1);
		cursor = 3;
		assert_eq!(screen.press(&mut cursor, PAGES), Press::NextPage);
		assert_eq!(cursor, 0, "it wraps");

		let out = Car::calm().with(6, Some(-1.0));
		assert_eq!(screen.frame(cursor, PAGES, 200, out.value_of()).page, 3);
		// Set over BLE while the alarm is up: not a press, and not undone by it.
		cursor = 1;
		assert_eq!(screen.frame(cursor, PAGES, 400, Car::calm().value_of()).page, 3, "holding");
		assert_eq!(screen.frame(cursor, PAGES, 400 + HOLD_MS, Car::calm().value_of()).page, 1);
		assert_eq!(screen.press(&mut cursor, PAGES), Press::NextPage);
		assert_eq!(cursor, 2);
	}

	#[test]
	fn two_rules_out_at_once_take_the_screen_in_plan_order_and_each_is_said() {
		let mut screen = screen();
		let mut cursor = 0;
		let both = Car::calm().with(4, Some(20.0)).with(6, Some(-5.0));
		let first = screen.frame(cursor, PAGES, 0, both.value_of());
		assert_eq!(
			(first.page, first.change),
			(2, Some(Change::Took { rule: 0 })),
			"the first rule in the plan"
		);
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced);
		let second = screen.frame(cursor, PAGES, 200, both.value_of());
		assert_eq!(
			(second.page, second.change),
			(3, Some(Change::Took { rule: 1 })),
			"the one behind it is a takeover of its own"
		);
		assert!(second.page_changed);
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced);
		let back = screen.frame(cursor, PAGES, 400, both.value_of());
		assert_eq!(
			(back.page, back.change),
			(0, Some(Change::Silenced)),
			"both still out: silenced, not over"
		);
		assert_eq!(cursor, 0);
	}

	#[test]
	fn a_stale_channel_neither_trips_nor_releases() {
		let mut screen = screen();
		let mut cursor = 0;
		let stale = Car::calm().with(6, None);
		assert_eq!(screen.frame(cursor, PAGES, 0, stale.value_of()).page, 0, "no value is not a low one");
		assert_eq!(screen.frame(cursor, PAGES, 200, Car::calm().with(6, Some(-1.0)).value_of()).page, 3);
		// Gone quiet mid-episode: long past the hold, still up.
		assert_eq!(screen.frame(cursor, PAGES, 200 + HOLD_MS * 4, stale.value_of()).page, 3);
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced, "the button is the way out");
		assert_eq!(screen.frame(cursor, PAGES, 200 + HOLD_MS * 5, stale.value_of()).page, 0);
	}

	#[test]
	fn the_adapter_screen_runs_no_alarms_and_a_press_there_turns_the_page() {
		let mut screen = screen();
		let mut cursor = 0;
		let out = Car::calm().with(4, Some(11.0));
		assert_eq!(screen.frame(cursor, PAGES, 0, out.value_of()).page, 2);
		// The board becomes an adapter: its screen, not the alarm's.
		screen.adapter();
		assert_eq!(screen.press(&mut cursor, PAGES), Press::NextPage, "nothing on the glass to silence");
		assert_eq!(cursor, 1);
		screen.adapter();
		// Back to the panel with the value still out: the episode was never silenced.
		let back = screen.frame(cursor, PAGES, 60_000, out.value_of());
		assert_eq!((back.page, back.change), (2, None), "the same episode, not a new takeover");
		assert!(back.page_changed, "the glass showed the adapter a frame ago");
		assert!(!screen.frame(cursor, PAGES, 60_200, out.value_of()).page_changed);
	}

	#[test]
	fn back_from_the_adapter_screen_the_alarm_cell_blinks_from_its_inverted_half() {
		// Fired at 0; the adapter screen until 10.7 s; back at 10.8 s, where the clock alone
		// is in a plain half ((10800 / 400) % 2 = 1). The first frame back is inverted.
		let mut screen = screen();
		let out = Car::calm().with(5, Some(12.0));
		screen.frame(0, PAGES, 0, out.value_of());
		screen.adapter();
		let back = screen.frame(0, PAGES, 10_800, out.value_of());
		assert_eq!(back.change, None, "the same episode, not a new takeover");
		assert_eq!(back.inverted, Some(ChannelId(5)));
		assert_eq!(blinks(&mut screen, out, [11_000, 11_200, 11_400], 5), [true, false, false]);
	}

	/// Which frames of `times` draw `channel`'s cell inverted, with `car` as the store.
	fn blinks(screen: &mut Screen<'static, 2>, car: Car, times: impl IntoIterator<Item = u64>, channel: u16) -> std::vec::Vec<bool> {
		times
			.into_iter()
			.map(|t| screen.frame(0, PAGES, t, car.value_of()).inverted == Some(ChannelId(channel)))
			.collect()
	}

	#[test]
	fn the_offending_cell_blinks_while_out_holds_steady_after_the_release_and_then_is_gone() {
		let mut screen = screen();
		let out = Car::calm().with(5, Some(12.0));
		// The board's frames, 200 ms apart: two inverted, two plain, while the value is out.
		for t in [0, 200, 400, 600] {
			let glass = screen.frame(0, PAGES, t, out.value_of());
			assert_eq!(
				(glass.page, glass.offending),
				(2, Some(ChannelId(5))),
				"the page and what it points at stay"
			);
			assert!(matches!(glass.highlight, Some(Highlight::Blinking { .. })));
		}
		assert_eq!(
			blinks(&mut screen, out, (800..2_400).step_by(200), 5),
			[true, true, false, false, true, true, false, false]
		);
		// Back inside: inverted on every frame of the hold, whatever the clock's half.
		let glass = screen.frame(0, PAGES, 2_400, Car::calm().value_of());
		assert_eq!(glass.highlight, Some(Highlight::Steady));
		let hold = blinks(&mut screen, Car::calm(), (2_400..2_400 + HOLD_MS).step_by(200), 5);
		assert!(hold.iter().all(|&inverted| inverted), "{hold:?}");
		// And handed back: nothing inverted, nothing pointed at.
		let back = screen.frame(0, PAGES, 2_400 + HOLD_MS, Car::calm().value_of());
		assert_eq!((back.page, back.offending, back.highlight, back.inverted), (0, None, None, None));
	}

	#[test]
	fn a_silenced_alarm_inverts_nothing() {
		let mut screen = screen();
		let mut cursor = 0;
		let out = Car::calm().with(4, Some(11.0));
		screen.frame(cursor, PAGES, 0, out.value_of());
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced);
		for t in (200..=2_000).step_by(200) {
			let glass = screen.frame(cursor, PAGES, t, out.value_of());
			assert_eq!((glass.offending, glass.highlight, glass.inverted), (None, None, None), "t={t}");
		}
	}

	#[test]
	fn a_second_rule_taking_over_blinks_its_own_cell() {
		let mut screen = screen();
		let mut cursor = 0;
		let both = Car::calm().with(4, Some(20.0)).with(6, Some(-5.0));
		assert_eq!(screen.frame(cursor, PAGES, 0, both.value_of()).inverted, Some(ChannelId(4)));
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced);
		// The second rule has been out since 0, behind the first. It takes the glass at
		// 600 ms — the plain half of a blink counted from when it fired — and its own cell is
		// inverted on that first frame: the phase starts when it takes the glass.
		let glass = screen.frame(cursor, PAGES, 600, both.value_of());
		assert_eq!((glass.page, glass.offending, glass.inverted), (3, Some(ChannelId(6)), Some(ChannelId(6))));
		assert!(matches!(glass.highlight, Some(Highlight::Blinking { .. })));
		assert_eq!(blinks(&mut screen, both, (800..=1_600).step_by(200), 6), [true, false, false, true, true]);
		assert!(
			(0..=1_400)
				.step_by(200)
				.all(|t| screen.frame(cursor, PAGES, t + 1_800, both.value_of()).inverted != Some(ChannelId(4))),
			"the silenced rule's cell is not inverted meanwhile"
		);
	}

	#[test]
	fn a_takeover_of_the_page_already_up_is_seen_on_its_first_frame() {
		// The driver is on the rule's own page, so the page does not change: the cell is
		// the only news, and it is inverted at once whatever the clock.
		let mut screen = screen();
		for t in [0, 200, 400] {
			assert_eq!(screen.frame(2, PAGES, t, Car::calm().value_of()).inverted, None);
		}
		let took = screen.frame(2, PAGES, 500, Car::calm().with(5, Some(12.0)).value_of());
		assert_eq!((took.page, took.page_changed), (2, false));
		assert_eq!(took.change, Some(Change::Took { rule: 0 }));
		assert_eq!(took.inverted, Some(ChannelId(5)));
	}

	#[test]
	fn frames_off_the_beat_still_show_both_halves_and_hold_neither_past_a_half_and_a_frame() {
		// The board frames every 200 ms plus however long a frame takes to draw, so its
		// frames drift across the clock's halves. Whatever the period up to half a blink
		// and whatever the start, each half is seen, and no picture lasts longer than one
		// half and the frame after it.
		let jittered = [190, 230, 260, 210, 200, 240];
		let mut schedules: std::vec::Vec<std::vec::Vec<u64>> = std::vec::Vec::new();
		for period in [150, 200, 201, 213, 230, 250, 300, 399, 400] {
			for start in [0, 1, 199, 200, 399] {
				schedules.push((0..60).map(|k| start + k * period).collect());
			}
		}
		schedules.push(
			jittered
				.iter()
				.cycle()
				.take(60)
				.scan(0, |t, gap| Some(core::mem::replace(t, *t + gap)))
				.collect(),
		);
		for times in schedules {
			let mut screen = screen();
			let out = Car::calm().with(5, Some(12.0));
			let seen = blinks(&mut screen, out, times.iter().copied(), 5);
			assert!(seen.contains(&true) && seen.contains(&false), "{times:?}");
			let longest_gap = times.windows(2).map(|w| w[1] - w[0]).max().unwrap_or(0);
			let mut run_from = times[0];
			for i in 1..times.len() {
				if seen[i] != seen[i - 1] {
					let shown_for = times[i] - run_from;
					assert!(shown_for < BLINK_MS + longest_gap, "{shown_for} ms the same at {times:?}");
					run_from = times[i];
				}
			}
		}
	}

	#[test]
	fn the_plain_half_of_a_blink_draws_exactly_the_page_with_nothing_inverted() {
		use crate::{Board, Cell, Frame, Links, PANEL, Theme, draw_with};
		use embedded_graphics::pixelcolor::BinaryColor;
		use embedded_graphics_simulator::SimulatorDisplay;

		// The alarm page's two cells, composed as the board's panel task composes them.
		let draw = |car: Car, inverted: Option<ChannelId>| {
			let cells = [4u16, 5].map(|i| {
				let cell = Cell::new(if i == 4 { "A" } else { "B" }, car.value_of()(i), "", 1);
				if inverted == Some(ChannelId(i)) { cell.alarmed() } else { cell }
			});
			let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);
			let board = Board {
				links: Links::NONE,
				rates: None,
			};
			draw_with(&Frame::Values { cells: &cells }, &board, &Theme::bold_mono(), &mut display);
			display
		};
		let mut screen = screen();
		let out = Car::calm().with(5, Some(12.0));
		let plain = draw(out, None);
		let alarmed = draw(out, Some(ChannelId(5)));
		assert!(plain != alarmed, "the inverted half is a different picture");

		let on = screen.frame(0, PAGES, 0, out.value_of());
		assert!(draw(out, on.inverted) == alarmed, "the inverted half");
		let off = screen.frame(0, PAGES, BLINK_MS, out.value_of());
		assert_eq!(off.page, 2, "still the alarm's page");
		assert!(draw(out, off.inverted) == plain, "the plain half is the plain page, pixel for pixel");
	}

	#[test]
	fn the_lever_turns_the_page_both_ways_and_wraps() {
		let mut screen = screen();
		let mut cursor = 0;
		screen.frame(cursor, PAGES, 0, Car::calm().value_of());
		assert_eq!(screen.lever(Lever::Previous, &mut cursor, PAGES), Action::Paged);
		assert_eq!(cursor, PAGES - 1, "back from the first is the last");
		assert_eq!(screen.lever(Lever::Next, &mut cursor, PAGES), Action::Paged);
		assert_eq!(cursor, 0, "on from the last is the first");
		assert_eq!(screen.lever(Lever::Next, &mut cursor, PAGES), Action::Paged);
		assert_eq!(screen.frame(cursor, PAGES, 200, Car::calm().value_of()).page, 1);
	}

	#[test]
	fn measure_takes_the_glass_from_any_page_and_leaves_it_back_where_it_was() {
		let mut screen = screen();
		let mut cursor = 1;
		screen.frame(cursor, PAGES, 0, Car::calm().value_of());
		assert!(!screen.stopwatch() && !screen.stopwatch_on_glass());
		assert_eq!(screen.lever(Lever::Measure, &mut cursor, PAGES), Action::StopwatchOn);
		assert!(screen.stopwatch() && screen.stopwatch_on_glass());
		// The lever's page turns do nothing while it is up; the cursor stays.
		assert_eq!(screen.lever(Lever::Next, &mut cursor, PAGES), Action::Ignored);
		assert_eq!(screen.lever(Lever::Previous, &mut cursor, PAGES), Action::Ignored);
		assert_eq!(cursor, 1);
		let glass = screen.frame(cursor, PAGES, 200, Car::calm().value_of());
		assert_eq!((glass.page, glass.change), (1, None), "the cursor's page, under the stopwatch");
		assert_eq!(screen.lever(Lever::Measure, &mut cursor, PAGES), Action::StopwatchOff);
		assert!(!screen.stopwatch() && !screen.stopwatch_on_glass());
		assert_eq!(cursor, 1, "back to the page it came from");
	}

	#[test]
	fn a_lever_press_while_an_alarm_holds_the_glass_silences_it_and_does_nothing_else() {
		for lever in [Lever::Next, Lever::Previous, Lever::Measure] {
			let mut screen = screen();
			let mut cursor = 1;
			let out = Car::calm().with(4, Some(11.0));
			assert_eq!(screen.frame(cursor, PAGES, 0, out.value_of()).page, 2);
			assert_eq!(screen.lever(lever, &mut cursor, PAGES), Action::Silenced, "{lever:?}");
			assert_eq!(cursor, 1, "{lever:?} did not page");
			assert!(!screen.stopwatch(), "{lever:?} did not switch the stopwatch on");
			let silenced = screen.frame(cursor, PAGES, 200, out.value_of());
			assert_eq!((silenced.page, silenced.change), (1, Some(Change::Silenced)), "{lever:?}");
		}
	}

	#[test]
	fn an_alarm_takes_the_glass_from_the_stopwatch_and_hands_it_back() {
		let mut screen = screen();
		let mut cursor = 0;
		screen.frame(cursor, PAGES, 0, Car::calm().value_of());
		screen.lever(Lever::Measure, &mut cursor, PAGES);
		let out = Car::calm().with(6, Some(-1.0));
		let took = screen.frame(cursor, PAGES, 200, out.value_of());
		assert_eq!((took.page, took.change), (3, Some(Change::Took { rule: 1 })));
		assert!(screen.stopwatch() && !screen.stopwatch_on_glass(), "the alarm's page is drawn");
		assert!(screen.alarm_showing());
		// A press silences the alarm, and the stopwatch is on the glass again.
		assert_eq!(screen.lever(Lever::Measure, &mut cursor, PAGES), Action::Silenced);
		assert!(screen.stopwatch_on_glass() && !screen.alarm_showing());
		// Silenced, a measure press is a measure press again.
		screen.frame(cursor, PAGES, 400, out.value_of());
		assert_eq!(screen.lever(Lever::Measure, &mut cursor, PAGES), Action::StopwatchOff);
		assert_eq!(cursor, 0);
	}

	#[test]
	fn the_buttons_short_press_keeps_its_job_and_a_page_on_the_glass_ends_the_stopwatch() {
		let mut screen = screen();
		let mut cursor = 2;
		screen.frame(cursor, PAGES, 0, Car::calm().value_of());
		screen.lever(Lever::Measure, &mut cursor, PAGES);
		assert_eq!(screen.press(&mut cursor, PAGES), Press::NextPage);
		assert_eq!(cursor, 3, "the next page, as always");
		assert!(!screen.stopwatch(), "a page is on the glass, so the stopwatch is not");
		// With an alarm up during the stopwatch the press silences, and the stopwatch stays.
		screen.lever(Lever::Measure, &mut cursor, PAGES);
		screen.frame(cursor, PAGES, 200, Car::calm().with(4, Some(11.0)).value_of());
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced);
		assert!(screen.stopwatch_on_glass());
		assert_eq!(cursor, 3);
	}

	#[test]
	fn the_adapter_screen_ends_the_stopwatch_so_a_run_does_not_survive_it() {
		use crate::stopwatch::{Event, Stopwatch};
		static MARKS: [u16; 2] = [60, 100];
		let mut screen = screen();
		let mut watch = Stopwatch::new(&MARKS, 1.0);
		let mut cursor = 0;
		// As the firmware runs it: the stopwatch follows the mode's turns before every
		// sample; the speed is fed regardless.
		let step = |screen: &mut Screen<'static, 2>, watch: &mut Stopwatch<'_>, kmh: f32, now_ms: u64| {
			watch.follow(screen.stopwatch_turns());
			watch.sample(Some(kmh), now_ms)
		};
		screen.frame(cursor, PAGES, 0, Car::calm().value_of());
		assert_eq!(screen.lever(Lever::Measure, &mut cursor, PAGES), Action::StopwatchOn);
		step(&mut screen, &mut watch, 0.0, 0);
		assert_eq!(step(&mut screen, &mut watch, 0.0, 1_000), Some(Event::Armed));
		assert_eq!(step(&mut screen, &mut watch, 10.0, 1_100), Some(Event::Started));
		step(&mut screen, &mut watch, 40.0, 2_000);
		// Five minutes as a plain adapter: no page, no stopwatch on the glass.
		screen.adapter();
		assert!(
			!screen.stopwatch() && !screen.stopwatch_on_glass(),
			"the adapter's screen, not the stopwatch"
		);
		step(&mut screen, &mut watch, 40.0, 2_100);
		// Back, at 120 km/h: that is not a 0-100 from a launch five minutes ago.
		screen.frame(cursor, PAGES, 302_000, Car::calm().value_of());
		assert_eq!(step(&mut screen, &mut watch, 120.0, 302_000), None);
		assert_eq!(step(&mut screen, &mut watch, 121.0, 302_100), None);
		assert_eq!(watch.run(), None, "the run the adapter interrupted is dropped, not finished");
		assert!(!screen.stopwatch(), "the mode stays off until measure is pressed again");
	}

	#[test]
	fn the_adapter_screen_ignores_the_lever() {
		let mut screen = screen();
		let mut cursor = 0;
		screen.adapter();
		for lever in [Lever::Next, Lever::Previous, Lever::Measure] {
			assert_eq!(screen.lever(lever, &mut cursor, PAGES), Action::Ignored);
		}
		assert_eq!(cursor, 0);
		assert!(!screen.stopwatch());
	}

	#[test]
	fn a_plan_without_alarms_only_turns_pages() {
		let mut screen: Screen<'static, 0> = Screen::new(&[]);
		let mut cursor = 0;
		let glass = screen.frame(cursor, 2, 0, |_| Some(1e9));
		assert_eq!((glass.page, glass.offending, glass.missed, glass.change), (0, None, None, None));
		assert_eq!(screen.press(&mut cursor, 2), Press::NextPage);
		assert_eq!(cursor, 1);
	}
}
