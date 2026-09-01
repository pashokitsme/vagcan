//! When a channel nobody is looking at goes wrong, it takes the screen.
//!
//! An alarm watches channels that are **not on the page being shown**, and when
//! one crosses a threshold it replaces whatever is up with the page that
//! explains why. That is the whole rule; everything below is about it not being
//! annoying, because an alarm that flickers or that cannot be dismissed is one
//! the driver learns to ignore, and then it may as well not exist.
//!
//! Three mechanisms, all of them required (`todo/dash/04-alarms.md`):
//!
//! 1. **Hysteresis.** It fires at `trip` and clears only past `release`, further
//!    back. A single threshold with a value hovering on it swaps the screen at
//!    the poll rate.
//! 2. **A hold after the release** — [`HOLD_MS`]. The view stays up that long
//!    after the value comes back inside, so a two-poll excursion is still
//!    readable.
//! 3. **It hands back to where you were.** The caller passes the page it *would*
//!    be showing on every poll, and that is what returns — not page one, and not
//!    a page this module remembers, which would be a second copy of a fact the
//!    caller already owns.
//!
//! And one escape hatch: **the button silences the episode**. A genuinely
//! misfiring engine would otherwise freeze the display for the rest of the
//! drive. Silence lasts until the value releases; a fresh crossing after that
//! arms the rule again, so the escape is per-episode and not a permanent
//! switch-off nobody remembers flipping.
//!
//! Nothing here reads a clock. Every entry point takes `now_ms`, because the
//! two callers are `embassy_time` on the board and `std::time` on a laptop, and
//! because a machine you can hand a synthetic clock is a machine you can test —
//! which is the only way the 2.5 second hold is ever exercised.
//!
//! What comes out is a [`Shown`]: a page, and at most one channel to draw
//! inverted. Inverting *the offending cell* rather than the panel is the point
//! of the view — see [`Cell::alarm`](crate::Cell::alarm).

/// How long the alarm view stays up after the value comes back inside.
///
/// The owner's number, 2026-08-20. Long enough to read a four-cylinder page,
/// short enough that a clean engine never keeps the screen.
pub const HOLD_MS: u64 = 2_500;

/// A page of the plan, by identity.
///
/// A number the plan assigns and never reuses, not a position in a list: pages
/// are added and reordered, and "hand back to where you were" has to survive
/// that. Nothing in this crate interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageId(pub u16);

/// A channel of the plan, by identity.
///
/// Same argument as [`PageId`], plus a sharper one: the polled set is the union
/// of the current page's channels and every rule's, so a channel's position in
/// the readings changes as the driver pages around. Only the name is stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelId(pub u16);

/// One channel's most recent answer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
	pub channel: ChannelId,
	/// `None` where the channel has not answered — which is not a reading of
	/// zero, and is treated as no evidence in either direction.
	pub value: Option<f32>,
}

impl Reading {
	pub const fn new(channel: ChannelId, value: Option<f32>) -> Self {
		Reading { channel, value }
	}
}

/// Which way a reading has to go to be wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
	/// Fires at or below `trip` — ignition retard, oil pressure.
	Below,
	/// Fires at or above `trip` — intake temperature, misfire counts.
	Above,
}

impl Direction {
	/// Is `a` further out than `b`, in this direction? Used to pick the one cell
	/// worth inverting out of the four a rule watches.
	fn worse(self, a: f32, b: f32) -> bool {
		match self {
			Direction::Below => a < b,
			Direction::Above => a > b,
		}
	}
}

/// One rule: some channels, a page that explains them, and two thresholds.
pub struct Alarm<'a> {
	/// Watched whether or not they are on the screen. Borrowed, because the plan
	/// outlives the machine and this crate has no allocator.
	pub channels: &'a [ChannelId],
	/// The page to raise. It is expected to contain the watched channels — that
	/// is what makes the takeover an explanation rather than an interruption —
	/// but nothing here checks it, because pages live in the plan.
	pub page: PageId,
	/// Fires at this value, in `direction`.
	pub trip: f32,
	/// Clears only once past this one, back the other way.
	pub release: f32,
	pub direction: Direction,
}

impl<'a> Alarm<'a> {
	/// A rule that fires when a reading falls to `trip` and clears above
	/// `release` — so `release` is the larger of the two.
	pub fn below(channels: &'a [ChannelId], page: PageId, trip: f32, release: f32) -> Self {
		debug_assert!(
			release > trip,
			"a Below alarm releases above where it trips, or it has no hysteresis at all"
		);
		Alarm {
			channels,
			page,
			trip,
			release,
			direction: Direction::Below,
		}
	}

	/// A rule that fires when a reading rises to `trip` and clears below
	/// `release` — so `release` is the smaller of the two.
	pub fn above(channels: &'a [ChannelId], page: PageId, trip: f32, release: f32) -> Self {
		debug_assert!(
			release < trip,
			"an Above alarm releases below where it trips, or it has no hysteresis at all"
		);
		Alarm {
			channels,
			page,
			trip,
			release,
			direction: Direction::Above,
		}
	}

	fn trips(&self, v: f32) -> bool {
		match self.direction {
			Direction::Below => v <= self.trip,
			Direction::Above => v >= self.trip,
		}
	}

	fn releases(&self, v: f32) -> bool {
		match self.direction {
			Direction::Below => v > self.release,
			Direction::Above => v < self.release,
		}
	}

	/// The rule's worst answering channel this poll, if any of them answered.
	///
	/// The extremum decides both questions at once and that is not a shortcut:
	/// *any* channel past `trip` fires the rule, which is the worst one being
	/// past it; *every* channel has to be past `release` to clear it, which is
	/// the worst one being past it. Ties keep the earlier channel, so a page of
	/// four identical readings always highlights the same cell.
	fn worst(&self, readings: &[Reading]) -> Option<(ChannelId, f32)> {
		let mut worst: Option<(ChannelId, f32)> = None;
		for want in self.channels {
			let Some(v) = value_of(*want, readings) else { continue };
			match worst {
				Some((_, best)) if !self.direction.worse(v, best) => {}
				_ => worst = Some((*want, v)),
			}
		}
		worst
	}
}

/// A channel not in the readings reads as unanswered, which is the truth: a
/// plan that forgot to poll it knows exactly as much about it as a car that
/// declined to answer.
fn value_of(channel: ChannelId, readings: &[Reading]) -> Option<f32> {
	readings.iter().find(|r| r.channel == channel).and_then(|r| r.value)
}

/// Where one rule is in its episode.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Episode {
	/// Nothing wrong, armed.
	Clear,
	/// Out of bounds now. `offender` is the cell to invert and follows the worst
	/// channel while it lasts.
	Firing { offender: ChannelId },
	/// Back inside, still on the screen until `until_ms`. The offender is frozen
	/// at whoever it last was, so the view does not end with nothing highlighted.
	Holding { offender: ChannelId, until_ms: u64 },
	/// Dismissed by the button. Still polled — a release is what re-arms it.
	Silenced,
}

impl Episode {
	fn showing(self) -> Option<ChannelId> {
		match self {
			Episode::Firing { offender } | Episode::Holding { offender, .. } => Some(offender),
			Episode::Clear | Episode::Silenced => None,
		}
	}
}

/// What belongs on the glass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
	pub page: PageId,
	/// The channel to draw inverted, and by construction `Some` exactly when the
	/// page is up because of an alarm. The caller maps it to a cell and calls
	/// [`Cell::alarmed`](crate::Cell::alarmed); if the page does not contain it,
	/// nothing is inverted and that is a plan bug, not a render one.
	pub offending: Option<ChannelId>,
}

impl Shown {
	/// An ordinary screen, nothing wrong.
	pub const fn page(page: PageId) -> Self {
		Shown { page, offending: None }
	}
}

/// The answer to one [`Alarms::poll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Update {
	pub shown: Shown,
	/// Did the screen just change? The board redraws on this rather than every
	/// poll: pushing 2 KB over SPI ten times a second to show the same picture
	/// costs power for nothing.
	///
	/// It is about *which* picture, not what is in it — the values move on their
	/// own and the caller redraws for those. The first poll always reports
	/// `true`, because before it there was nothing on the glass.
	pub changed: bool,
}

/// What a short press did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
	/// No alarm was showing, so the press means what it normally means.
	NextPage,
	/// An alarm was showing and this press ended that episode. The caller does
	/// not page: silencing *is* what the driver asked for.
	Silenced,
}

/// The rules, their state, and what they decided last.
///
/// `N` is fixed at the type: the plan's alarms are known when it is built and
/// this crate has no allocator. Rules are in **priority order** — when two fire
/// in the same poll the earlier one takes the screen, and the tie is broken by
/// the plan rather than by whichever the loop happened to see first.
pub struct Alarms<'a, const N: usize> {
	rules: [Alarm<'a>; N],
	episodes: [Episode; N],
	/// Which rule owns the screen, from the last poll. What a press acts on.
	showing: Option<usize>,
	/// The last decision, for `changed`.
	last: Option<Shown>,
}

impl<'a, const N: usize> Alarms<'a, N> {
	pub fn new(rules: [Alarm<'a>; N]) -> Self {
		Alarms {
			rules,
			episodes: [Episode::Clear; N],
			showing: None,
			last: None,
		}
	}

	/// Every channel any rule watches, silenced ones included.
	///
	/// A silenced rule is still polled, and that is the whole reason silence is
	/// bounded: the release it is waiting for cannot be seen if nobody asks. The
	/// caller unions this with the current page's channels to build the request
	/// set — twelve where a page alone would be four — and splits it at whatever
	/// batch limit the ECU turns out to accept. Duplicates are not removed here;
	/// there is nowhere to remember them without an allocator, and the caller is
	/// already walking the union.
	pub fn watched(&self) -> impl Iterator<Item = ChannelId> + '_ {
		self.rules.iter().flat_map(|r| r.channels.iter().copied())
	}

	/// A short press.
	///
	/// The device has one button, so this gesture is modal: while an alarm is
	/// showing it silences that episode and returns [`Press::Silenced`];
	/// otherwise it means what it normally means and the caller pages on. The
	/// caller does not need to ask which case it is first — that is exactly the
	/// question this answers, and asking separately is how the two get out of
	/// step.
	///
	/// One press ends one episode: if a second rule is also out, it takes the
	/// screen on the next poll and wants its own press, because it is a
	/// different thing to be told.
	pub fn press(&mut self) -> Press {
		match self.showing.take() {
			Some(i) => {
				self.episodes[i] = Episode::Silenced;
				Press::Silenced
			}
			None => Press::NextPage,
		}
	}

	/// Advance every rule and say what belongs on the glass.
	///
	/// `page` is what the caller would show if nothing were wrong — the page the
	/// driver paged to. It is passed every poll rather than remembered here so
	/// that "back to where you were" cannot drift out of step with the caller's
	/// own idea of where that is.
	pub fn poll(&mut self, page: PageId, readings: &[Reading], now_ms: u64) -> Update {
		for i in 0..N {
			self.episodes[i] = step(&self.rules[i], self.episodes[i], readings, now_ms);
		}

		self.showing = (0..N).find(|&i| self.episodes[i].showing().is_some());
		let shown = match self.showing {
			Some(i) => Shown {
				page: self.rules[i].page,
				offending: self.episodes[i].showing(),
			},
			None => Shown::page(page),
		};

		let changed = self.last != Some(shown);
		self.last = Some(shown);
		Update { shown, changed }
	}
}

/// One rule, one poll. Pulled out of the loop because it is the state machine
/// and everything around it is bookkeeping.
fn step(rule: &Alarm<'_>, episode: Episode, readings: &[Reading], now_ms: u64) -> Episode {
	let worst = rule.worst(readings);
	match episode {
		Episode::Clear => match worst {
			Some((c, v)) if rule.trips(v) => Episode::Firing { offender: c },
			_ => Episode::Clear,
		},
		Episode::Firing { offender } => match worst {
			// The offender follows the engine: a worse cylinder is the one worth
			// pointing at, even mid-episode.
			Some((c, v)) if rule.trips(v) => Episode::Firing { offender: c },
			Some((_, v)) if rule.releases(v) => Episode::Holding {
				offender,
				until_ms: now_ms.saturating_add(HOLD_MS),
			},
			// Inside the hysteresis band, or nothing answered. Neither of those is
			// news, so the screen does not move.
			_ => Episode::Firing { offender },
		},
		Episode::Holding { offender, until_ms } => match worst {
			// Out again before the hold expired: the same episode continues, so it
			// does not re-announce itself and the driver's silence still applies.
			Some((c, v)) if rule.trips(v) => Episode::Firing { offender: c },
			_ if now_ms >= until_ms => Episode::Clear,
			_ => Episode::Holding { offender, until_ms },
		},
		// Silence ends at the release and not before — including when the channel
		// stops answering, which is why silence is bounded by evidence rather than
		// by a timer. A unit that has dropped off the bus stays silenced, and that
		// is the right answer: it is not saying the engine is fine.
		Episode::Silenced => match worst {
			Some((_, v)) if rule.releases(v) => Episode::Clear,
			_ => Episode::Silenced,
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The four ignition-retard channels of `04`, by identifier rather than by
	/// position: what a rule watches is named, not counted, because the polled
	/// set is a union whose order changes with the page.
	const RETARD: [ChannelId; 4] = [ChannelId(0x200A), ChannelId(0x200B), ChannelId(0x200C), ChannelId(0x200D)];
	const BOOST: [ChannelId; 1] = [ChannelId(0x2005)];

	const RETARD_PAGE: PageId = PageId(7);
	const BOOST_PAGE: PageId = PageId(9);
	/// Where the driver was before any of this happened.
	const WAS_SHOWING: PageId = PageId(3);

	fn retard() -> Alarm<'static> {
		Alarm::below(&RETARD, RETARD_PAGE, -2.0, -1.5)
	}

	fn boost() -> Alarm<'static> {
		Alarm::above(&BOOST, BOOST_PAGE, 2.2, 2.0)
	}

	/// Four cylinders' worth of readings, in channel order.
	fn cyls(v: [f32; 4]) -> [Reading; 4] {
		[
			Reading::new(RETARD[0], Some(v[0])),
			Reading::new(RETARD[1], Some(v[1])),
			Reading::new(RETARD[2], Some(v[2])),
			Reading::new(RETARD[3], Some(v[3])),
		]
	}

	/// The same, with cylinder 2 the only interesting one.
	fn only(v: f32) -> [Reading; 4] {
		cyls([0.0, v, 0.0, 0.0])
	}

	#[test]
	fn hysteresis_takes_the_screen_once_where_one_threshold_would_take_it_ten_times() {
		// A retard sitting exactly on -2.0 and breathing either side of it. With a
		// single threshold this is a screen swapping at the poll rate, which is the
		// failure the whole rule exists to avoid.
		let mut alarms = Alarms::new([retard()]);
		let series = [-2.0, -1.9, -2.1, -1.95, -2.0, -1.6, -2.4, -1.7];
		let mut takeovers = 0;
		for (tick, v) in series.iter().enumerate() {
			let update = alarms.poll(WAS_SHOWING, &only(*v), tick as u64 * 100);
			if update.changed && update.shown.page == RETARD_PAGE {
				takeovers += 1;
			}
			assert_eq!(update.shown.page, RETARD_PAGE, "the episode never ends inside the band");
		}
		assert_eq!(takeovers, 1, "one episode, not eight");
	}

	#[test]
	fn it_releases_at_the_release_value_and_not_at_the_trip_value() {
		let mut alarms = Alarms::new([retard()]);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-2.0), 0).shown.page, RETARD_PAGE);
		// Back above the trip point, but still inside the band: not released, so the
		// hold has not even started and the view stays up however long we wait.
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-1.9), 1_000).shown.page, RETARD_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-1.9), 100_000).shown.page, RETARD_PAGE);
		// Past the release value the hold starts, and only then does it run out.
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-1.4), 100_000).shown.page, RETARD_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-1.4), 100_000 + HOLD_MS).shown.page, WAS_SHOWING);
	}

	#[test]
	fn the_view_stays_up_for_the_hold_and_then_hands_back_to_where_you_were() {
		let mut alarms = Alarms::new([retard()]);
		alarms.poll(WAS_SHOWING, &only(-3.0), 0);
		let released_at = 500;
		assert_eq!(alarms.poll(WAS_SHOWING, &only(0.0), released_at).shown.page, RETARD_PAGE);
		// One millisecond short of the hold, it is still the alarm's screen.
		let almost = alarms.poll(WAS_SHOWING, &only(0.0), released_at + HOLD_MS - 1);
		assert_eq!(almost.shown.page, RETARD_PAGE);
		assert!(!almost.changed);
		// And at the hold it hands back — to the page that was showing, by identity.
		let back = alarms.poll(WAS_SHOWING, &only(0.0), released_at + HOLD_MS);
		assert_eq!(back.shown, Shown::page(WAS_SHOWING));
		assert!(back.changed, "the glass has to be redrawn");
	}

	#[test]
	fn it_hands_back_to_the_page_that_was_showing_and_never_to_page_one() {
		let mut alarms = Alarms::new([retard()]);
		// A different page from the one the previous test used: what comes back is
		// whatever the caller says is current, which is the only definition of
		// "where you were" that survives the driver having paged around.
		let elsewhere = PageId(11);
		alarms.poll(elsewhere, &only(-3.0), 0);
		alarms.poll(elsewhere, &only(0.0), 10);
		assert_eq!(alarms.poll(elsewhere, &only(0.0), 10 + HOLD_MS).shown.page, elsewhere);
	}

	#[test]
	fn a_press_silences_the_episode_and_the_screen_goes_back_at_once() {
		let mut alarms = Alarms::new([retard()]);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-3.0), 0).shown.page, RETARD_PAGE);
		assert_eq!(alarms.press(), Press::Silenced);
		let after = alarms.poll(WAS_SHOWING, &only(-3.0), 100);
		assert_eq!(after.shown, Shown::page(WAS_SHOWING));
		assert!(after.changed);
	}

	#[test]
	fn a_silenced_alarm_stays_silent_while_the_value_is_still_out() {
		// The engine is genuinely misfiring. Without this the display is one frozen
		// screen for the rest of the drive.
		let mut alarms = Alarms::new([retard()]);
		alarms.poll(WAS_SHOWING, &only(-3.0), 0);
		alarms.press();
		for tick in 1..200u64 {
			let update = alarms.poll(WAS_SHOWING, &only(-3.0 - tick as f32), tick * 100);
			assert_eq!(update.shown, Shown::page(WAS_SHOWING), "silenced at t={}", tick * 100);
		}
	}

	#[test]
	fn a_fresh_crossing_after_a_release_arms_it_again() {
		let mut alarms = Alarms::new([retard()]);
		alarms.poll(WAS_SHOWING, &only(-3.0), 0);
		alarms.press();
		// Silence is not deafness: the release is still watched, and it re-arms.
		assert_eq!(alarms.poll(WAS_SHOWING, &only(0.0), 1_000).shown.page, WAS_SHOWING);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-2.5), 2_000).shown.page, RETARD_PAGE);
	}

	#[test]
	fn two_alarms_crossing_in_the_same_poll_resolve_by_priority_not_by_luck() {
		let mut alarms = Alarms::new([retard(), boost()]);
		let readings = [
			Reading::new(RETARD[0], Some(0.0)),
			Reading::new(RETARD[1], Some(-4.0)),
			Reading::new(RETARD[2], Some(0.0)),
			Reading::new(RETARD[3], Some(0.0)),
			Reading::new(BOOST[0], Some(2.5)),
		];
		let first = alarms.poll(WAS_SHOWING, &readings, 0);
		assert_eq!(first.shown.page, RETARD_PAGE, "the plan lists the rules in priority order");
		// And it is stable: the same input a second later does not swap them.
		assert!(!alarms.poll(WAS_SHOWING, &readings, 100).changed);
	}

	#[test]
	fn silencing_the_showing_alarm_lets_the_one_behind_it_through() {
		let mut alarms = Alarms::new([retard(), boost()]);
		let readings = [
			Reading::new(RETARD[0], Some(-4.0)),
			Reading::new(RETARD[1], Some(0.0)),
			Reading::new(RETARD[2], Some(0.0)),
			Reading::new(RETARD[3], Some(0.0)),
			Reading::new(BOOST[0], Some(2.5)),
		];
		alarms.poll(WAS_SHOWING, &readings, 0);
		assert_eq!(alarms.press(), Press::Silenced);
		// One press, one episode: the second alarm is a different thing to say.
		assert_eq!(alarms.poll(WAS_SHOWING, &readings, 100).shown.page, BOOST_PAGE);
		assert_eq!(alarms.press(), Press::Silenced);
		assert_eq!(alarms.poll(WAS_SHOWING, &readings, 200).shown, Shown::page(WAS_SHOWING));
	}

	#[test]
	fn the_offending_cell_is_the_worst_channel_and_not_the_first_one_over() {
		let mut alarms = Alarms::new([retard()]);
		let update = alarms.poll(WAS_SHOWING, &cyls([-2.1, -0.4, -3.9, -2.2]), 0);
		assert_eq!(update.shown.offending, Some(RETARD[2]));
		// It follows the engine while the episode runs...
		let update = alarms.poll(WAS_SHOWING, &cyls([-4.5, -0.4, -3.9, -2.2]), 100);
		assert_eq!(update.shown.offending, Some(RETARD[0]));
		assert!(update.changed, "a different cylinder is a different picture");
		// ...and freezes on the last offender once the values come back inside, so
		// the hold does not end on a page with nothing highlighted.
		let update = alarms.poll(WAS_SHOWING, &cyls([0.0, 0.0, 0.0, 0.0]), 200);
		assert_eq!(update.shown.offending, Some(RETARD[0]));
	}

	#[test]
	fn an_alarm_can_fire_above_a_threshold_as_well_as_below_one() {
		let mut alarms = Alarms::new([boost()]);
		let up = [Reading::new(BOOST[0], Some(2.3))];
		let band = [Reading::new(BOOST[0], Some(2.1))];
		let down = [Reading::new(BOOST[0], Some(1.9))];
		assert_eq!(alarms.poll(WAS_SHOWING, &up, 0).shown.page, BOOST_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &band, 100).shown.page, BOOST_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &down, 200).shown.page, BOOST_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &down, 200 + HOLD_MS).shown.page, WAS_SHOWING);
	}

	#[test]
	fn a_channel_that_stops_answering_neither_trips_nor_releases() {
		let mut alarms = Alarms::new([retard()]);
		let silence = [Reading::new(RETARD[0], None)];
		// Nothing on the bus is not a reading of zero, so it cannot fire...
		assert_eq!(alarms.poll(WAS_SHOWING, &silence, 0).shown, Shown::page(WAS_SHOWING));
		// ...and it cannot end an episode either: a unit that drops out mid-alarm
		// has not told us the engine is fine. The button is the way out.
		alarms.poll(WAS_SHOWING, &only(-3.0), 100);
		assert_eq!(alarms.poll(WAS_SHOWING, &silence, 100 + HOLD_MS * 4).shown.page, RETARD_PAGE);
		alarms.press();
		assert_eq!(alarms.poll(WAS_SHOWING, &silence, 100 + HOLD_MS * 5).shown, Shown::page(WAS_SHOWING));
	}

	#[test]
	fn a_press_with_no_alarm_showing_is_a_page_turn() {
		let mut alarms = Alarms::new([retard()]);
		alarms.poll(WAS_SHOWING, &only(0.0), 0);
		assert_eq!(alarms.press(), Press::NextPage);
	}

	#[test]
	fn nothing_moving_reports_nothing_changed_after_the_first_look() {
		let mut alarms = Alarms::new([retard()]);
		// The first poll always changes: there was nothing on the glass before it.
		assert!(alarms.poll(WAS_SHOWING, &only(0.0), 0).changed);
		assert!(!alarms.poll(WAS_SHOWING, &only(0.0), 100).changed);
		assert!(!alarms.poll(WAS_SHOWING, &only(-0.9), 200).changed);
		// Paging is the caller's business, and it is still a change of screen.
		assert!(alarms.poll(PageId(4), &only(0.0), 300).changed);
	}

	#[test]
	fn every_rules_channels_are_watched_even_while_it_is_silenced() {
		let mut alarms = Alarms::new([retard(), boost()]);
		alarms.poll(WAS_SHOWING, &only(-3.0), 0);
		alarms.press();
		let watched: [ChannelId; 5] = core::array::from_fn(|i| alarms.watched().nth(i).unwrap());
		assert_eq!(watched, [RETARD[0], RETARD[1], RETARD[2], RETARD[3], BOOST[0]]);
		assert_eq!(
			alarms.watched().count(),
			5,
			"a silenced rule is still polled — release is what re-arms it"
		);
	}
}
