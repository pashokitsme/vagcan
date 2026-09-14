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
//! **The adapter screen runs no alarms.** While the board is a plain CAN adapter
//! the planner sends nothing, so nothing is read and nothing could trip; and the
//! glass shows the adapter's counters, so a press there has no alarm to silence
//! and turns the page as it always has.

use crate::alarm::{Alarm, Alarms, ChannelId, PageId, Press, Update};
use crate::pages;

/// The plan's alarms and what the glass showed last.
///
/// `N` is the plan's number of rules, fixed when the image is built.
pub struct Screen<'a, const N: usize> {
	alarms: Alarms<'a, N>,
	/// The last frame was the adapter's, not a page.
	adapter: bool,
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
		}
	}

	/// One frame of a page. `cursor` is the page the driver paged to, an index
	/// into the plan's pages; `value_of` answers the latest value of a plan
	/// channel by index, `None` where it is stale or never answered.
	///
	/// What comes back is the page to draw and the channel whose cell to invert.
	/// `changed` is also true on the first frame after the adapter screen, whose
	/// picture was not a page at all.
	pub fn frame(&mut self, cursor: u8, now_ms: u64, value_of: impl Fn(u16) -> Option<f32>) -> Update {
		let from_adapter = core::mem::replace(&mut self.adapter, false);
		let mut update = self
			.alarms
			.poll_with(PageId(u16::from(cursor)), |channel: ChannelId| value_of(channel.0), now_ms);
		update.changed |= from_adapter;
		update
	}

	/// One frame of the adapter screen, in place of [`Screen::frame`]. The alarms
	/// are not polled: their state waits, and the first page frame after resumes
	/// it.
	pub fn adapter(&mut self) {
		self.adapter = true;
	}

	/// A short press, with `pages` the number of pages the cursor runs over.
	///
	/// While an alarm owns the glass the press silences it and the cursor stays;
	/// otherwise the cursor moves on, wrapping. What it did is returned, so the
	/// caller can say so.
	pub fn press(&mut self, cursor: &mut u8, pages: u8) -> Press {
		let press = if self.adapter { Press::NextPage } else { self.alarms.press() };
		if press == Press::NextPage {
			*cursor = pages::next(*cursor, pages);
		}
		press
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::alarm::{Direction, HOLD_MS, Shown};

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
		assert_eq!(screen.frame(cursor, 0, Car::calm().value_of()).shown, Shown::page(PageId(0)));
		// Channel 5 is on page 2, which nobody is looking at.
		let update = screen.frame(cursor, 200, Car::calm().with(5, Some(12.0)).value_of());
		assert_eq!(update.shown.page, PageId(2), "the page that explains it");
		assert_eq!(update.shown.offending, Some(ChannelId(5)), "the cell to invert, by plan index");
		assert!(update.changed);
		// Back inside, the hold runs out and the glass returns to the cursor.
		screen.frame(cursor, 400, Car::calm().value_of());
		let back = screen.frame(cursor, 400 + HOLD_MS, Car::calm().value_of());
		assert_eq!(back.shown, Shown::page(PageId(0)));
	}

	#[test]
	fn a_press_silences_the_episode_and_a_release_arms_the_rule_again() {
		let mut screen = screen();
		let mut cursor = 1;
		let out = Car::calm().with(4, Some(11.0));
		assert_eq!(screen.frame(cursor, 0, out.value_of()).shown.page, PageId(2));
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced);
		assert_eq!(cursor, 1, "silencing is not a page turn");
		// Still out, and silent: the driver's page.
		assert_eq!(screen.frame(cursor, 200, out.value_of()).shown, Shown::page(PageId(1)));
		assert_eq!(screen.frame(cursor, 400, out.value_of()).shown, Shown::page(PageId(1)));
		// A release re-arms without taking the screen...
		assert_eq!(screen.frame(cursor, 600, Car::calm().value_of()).shown, Shown::page(PageId(1)));
		// ...and the next crossing takes it again.
		assert_eq!(screen.frame(cursor, 800, out.value_of()).shown.page, PageId(2));
	}

	#[test]
	fn the_cursor_moves_only_on_a_page_turn_and_the_hold_hands_back_to_where_it_is_now() {
		let mut screen = screen();
		let mut cursor = 0;
		screen.frame(cursor, 0, Car::calm().value_of());
		assert_eq!(screen.press(&mut cursor, PAGES), Press::NextPage);
		assert_eq!(cursor, 1);
		cursor = 3;
		assert_eq!(screen.press(&mut cursor, PAGES), Press::NextPage);
		assert_eq!(cursor, 0, "it wraps");

		let out = Car::calm().with(6, Some(-1.0));
		assert_eq!(screen.frame(cursor, 200, out.value_of()).shown.page, PageId(3));
		// Set over BLE while the alarm is up: not a press, and not undone by it.
		cursor = 1;
		assert_eq!(screen.frame(cursor, 400, Car::calm().value_of()).shown.page, PageId(3), "holding");
		assert_eq!(screen.frame(cursor, 400 + HOLD_MS, Car::calm().value_of()).shown, Shown::page(PageId(1)));
		assert_eq!(screen.press(&mut cursor, PAGES), Press::NextPage);
		assert_eq!(cursor, 2);
	}

	#[test]
	fn two_rules_out_at_once_take_the_screen_in_plan_order_one_press_each() {
		let mut screen = screen();
		let mut cursor = 0;
		let both = Car::calm().with(4, Some(20.0)).with(6, Some(-5.0));
		assert_eq!(
			screen.frame(cursor, 0, both.value_of()).shown.page,
			PageId(2),
			"the first rule in the plan"
		);
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced);
		assert_eq!(screen.frame(cursor, 200, both.value_of()).shown.page, PageId(3), "then the one behind it");
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced);
		assert_eq!(screen.frame(cursor, 400, both.value_of()).shown, Shown::page(PageId(0)));
		assert_eq!(cursor, 0);
	}

	#[test]
	fn a_stale_channel_neither_trips_nor_releases() {
		let mut screen = screen();
		let mut cursor = 0;
		let stale = Car::calm().with(6, None);
		assert_eq!(
			screen.frame(cursor, 0, stale.value_of()).shown,
			Shown::page(PageId(0)),
			"no value is not a low one"
		);
		assert_eq!(
			screen.frame(cursor, 200, Car::calm().with(6, Some(-1.0)).value_of()).shown.page,
			PageId(3)
		);
		// Gone quiet mid-episode: long past the hold, still up.
		assert_eq!(screen.frame(cursor, 200 + HOLD_MS * 4, stale.value_of()).shown.page, PageId(3));
		assert_eq!(screen.press(&mut cursor, PAGES), Press::Silenced, "the button is the way out");
		assert_eq!(screen.frame(cursor, 200 + HOLD_MS * 5, stale.value_of()).shown, Shown::page(PageId(0)));
	}

	#[test]
	fn the_adapter_screen_runs_no_alarms_and_a_press_there_turns_the_page() {
		let mut screen = screen();
		let mut cursor = 0;
		let out = Car::calm().with(4, Some(11.0));
		assert_eq!(screen.frame(cursor, 0, out.value_of()).shown.page, PageId(2));
		// The board becomes an adapter: its screen, not the alarm's.
		screen.adapter();
		assert_eq!(screen.press(&mut cursor, PAGES), Press::NextPage, "nothing on the glass to silence");
		assert_eq!(cursor, 1);
		screen.adapter();
		// Back to the panel with the value still out: the episode was never silenced.
		let update = screen.frame(cursor, 60_000, out.value_of());
		assert_eq!(update.shown.page, PageId(2));
		assert!(update.changed, "the glass showed the adapter a frame ago");
		assert!(!screen.frame(cursor, 60_200, out.value_of()).changed);
	}

	#[test]
	fn a_plan_without_alarms_only_turns_pages() {
		let mut screen: Screen<'static, 0> = Screen::new(&[]);
		let mut cursor = 0;
		let update = screen.frame(cursor, 0, |_| Some(1e9));
		assert_eq!(update.shown, Shown::page(PageId(0)));
		assert_eq!(screen.press(&mut cursor, 2), Press::NextPage);
		assert_eq!(cursor, 1);
	}
}
