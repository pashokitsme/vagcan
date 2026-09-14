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

use crate::alarm::{Alarm, Alarms, ChannelId, PageId, Press};
use crate::pages;

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
	/// The channel whose cell is drawn inverted.
	pub offending: Option<ChannelId>,
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
		let (page, offending, missed) = match u8::try_from(shown.page.0) {
			Ok(page) if rule.is_none() || page < pages => (page, shown.offending, None),
			_ => (cursor, None, Some(shown.page)),
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
			offending,
			page_changed,
			missed,
			change,
		}
	}

	/// One frame of the adapter screen, in place of [`Screen::frame`]. The alarms
	/// are not polled: their state waits, and the first page frame after resumes
	/// it — as a changed page, since the glass showed no page meanwhile.
	pub fn adapter(&mut self) {
		self.adapter = true;
		self.drawn = None;
	}

	/// A short press, with `pages` the number of pages the cursor runs over.
	///
	/// While an alarm owns the glass the press silences it and the cursor stays;
	/// otherwise the cursor moves on, wrapping. What it did is returned, so the
	/// caller can say so.
	pub fn press(&mut self, cursor: &mut u8, pages: u8) -> Press {
		let press = if self.adapter { Press::NextPage } else { self.alarms.press() };
		match press {
			Press::NextPage => *cursor = pages::next(*cursor, pages),
			Press::Silenced => self.silenced = true,
		}
		press
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::alarm::{Direction, HOLD_MS};
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
			trip: 10.0,
			release: 8.0,
			direction: Direction::Above,
		},
		Alarm {
			channels: &LOW_CHANNELS,
			page: PageId(3),
			trip: 0.0,
			release: 1.0,
			direction: Direction::Below,
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
	fn a_plan_without_alarms_only_turns_pages() {
		let mut screen: Screen<'static, 0> = Screen::new(&[]);
		let mut cursor = 0;
		let glass = screen.frame(cursor, 2, 0, |_| Some(1e9));
		assert_eq!((glass.page, glass.offending, glass.missed, glass.change), (0, None, None, None));
		assert_eq!(screen.press(&mut cursor, 2), Press::NextPage);
		assert_eq!(cursor, 1);
	}
}
