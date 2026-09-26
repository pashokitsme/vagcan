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
//! inverted — blinking ([`BLINK_MS`]) while the value is out, steady through the
//! hold. Inverting *the offending cell* rather than the panel is the point of the
//! view — see [`Cell::alarm`](crate::Cell::alarm).

/// How long the alarm view stays up after the value comes back inside.
///
/// The owner's number, 2026-08-20. Long enough to read a four-cylinder page,
/// short enough that a clean engine never keeps the screen.
pub const HOLD_MS: u64 = 2_500;

/// Half a blink of the offending cell: inverted this long, plain this long, while the
/// value is past the trip.
///
/// The owner's number, 2026-09-26: a cell that is merely inverted did not say "look here"
/// clearly enough. The phase counts from the takeover — `((now_ms − from) / BLINK_MS) % 2`,
/// inverted when it is 0 — so the first frame of an episode is always inverted, even when the
/// driver is already on the rule's page and the cell is the only thing that changes. `from`
/// is the episode's own state, not a timer: the moment it fired, fired again out of the
/// hold, or took the glass from another rule.
///
/// **The caller's frames must be at most `BLINK_MS / 2` apart.** Every half then holds at
/// least one frame even when drawing a frame takes as long again as the wait between two,
/// so the cell is never drawn the same for longer than one half and a frame. At a period of
/// an even multiple of `BLINK_MS` the frames alias onto one half and the blink is lost — the
/// board's panel (`FRAME_MS`, 200 ms: two frames on, two off) and the host replay assert it.
pub const BLINK_MS: u64 = 400;

/// A page of the plan: its index into [`Plan::pages`](crate::plan::Plan::pages).
///
/// An image is built for one plan, so the index names the same page for the
/// life of the image, and the generator only writes one the board holds
/// ([`MAX_PAGES`](crate::pages::MAX_PAGES)). Nothing in this module interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageId(pub u16);

/// A channel of the plan: its index into [`Plan::channels`](crate::plan::Plan::channels).
///
/// The index into the plan, not a position in the readings: those are the union
/// of the current page's channels and every rule's, so where a channel sits in
/// them changes as the driver pages around. The plan index does not.
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

/// The most rules one plan may carry.
///
/// Every rule's channels are read at their own rate all the time, whatever page
/// is up, so each rule is a standing cost on the bus budget; and one press ends
/// one episode, so a driver with more rules out at once than this presses more
/// than they read. The generator refuses a `dash.toml` with more.
pub const MAX_ALARMS: usize = 4;

/// What a rule watches for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rule<'a> {
	/// A reading past a threshold, in one direction, with hysteresis.
	Threshold {
		/// Fires at this value, in `direction`.
		trip: f32,
		/// Clears only once past this one, back the other way.
		release: f32,
		direction: Direction,
	},
	/// A reading too far from what its control unit asked for
	/// (`todo/dash/18-setpoints-and-drift.md` §4).
	Drift {
		/// The specified value of each watched channel, in the same order: the plan pairs
		/// them, so the rule reads both halves through the same lookup the rest does.
		specified: &'a [ChannelId],
		/// Fires when `|actual − specified|` passes this share of `|specified|`.
		percent: f32,
		/// Clears under this share.
		release_percent: f32,
		/// And only once it has been past `percent` this long without a break. A
		/// turbocharger lags its own setpoint on every throttle stab; without this the rule
		/// fires on every gear change.
		hold_ms: u64,
		/// Below this specified value the rule says nothing: a percentage of nearly zero is
		/// noise, not drift.
		min_setpoint: f32,
	},
}

/// One rule: some channels, a page that explains them, and what counts as wrong.
///
/// Plain data, so a plan can carry it as a `static`: the generator writes the
/// fields directly, having already checked what [`Alarm::below`] and
/// [`Alarm::above`] assert.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alarm<'a> {
	/// Watched whether or not they are on the screen. Borrowed, because the plan
	/// outlives the machine and this crate has no allocator.
	pub channels: &'a [ChannelId],
	/// The page to raise. It contains the watched channels — that is what makes
	/// the takeover an explanation rather than an interruption — and the plan
	/// generator is what checks it, because pages live in the plan.
	pub page: PageId,
	pub rule: Rule<'a>,
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
			rule: Rule::Threshold {
				trip,
				release,
				direction: Direction::Below,
			},
		}
	}

	/// A rule that fires when a channel has been more than `percent` away from what its unit
	/// asked for, for `hold_ms` without a break, and clears under `release_percent`.
	pub fn drift(
		channels: &'a [ChannelId],
		specified: &'a [ChannelId],
		page: PageId,
		percent: f32,
		release_percent: f32,
		hold_ms: u64,
		min_setpoint: f32,
	) -> Self {
		debug_assert!(
			specified.len() == channels.len(),
			"a drift rule pairs every channel it watches with a specified value"
		);
		debug_assert!(
			release_percent < percent,
			"a drift alarm releases under where it trips, or it has no hysteresis at all"
		);
		Alarm {
			channels,
			page,
			rule: Rule::Drift {
				specified,
				percent,
				release_percent,
				hold_ms,
				min_setpoint,
			},
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
			rule: Rule::Threshold {
				trip,
				release,
				direction: Direction::Above,
			},
		}
	}

	/// How long a reading has to stay wrong before the rule fires. Zero for a threshold: a
	/// temperature past its limit is past it now.
	fn hold_ms(&self) -> u64 {
		match self.rule {
			Rule::Threshold { .. } => 0,
			Rule::Drift { hold_ms, .. } => hold_ms,
		}
	}

	fn trips(&self, v: f32) -> bool {
		match self.rule {
			Rule::Threshold { trip, direction, .. } => match direction {
				Direction::Below => v <= trip,
				Direction::Above => v >= trip,
			},
			// `v` is the share of the specified value the channel is away from it.
			Rule::Drift { percent, .. } => v >= percent,
		}
	}

	fn releases(&self, v: f32) -> bool {
		match self.rule {
			Rule::Threshold { release, direction, .. } => match direction {
				Direction::Below => v > release,
				Direction::Above => v < release,
			},
			Rule::Drift { release_percent, .. } => v < release_percent,
		}
	}

	/// The rule's worst answering channel this poll, if any of them answered.
	///
	/// The extremum decides both questions at once and that is not a shortcut:
	/// *any* channel past `trip` fires the rule, which is the worst one being
	/// past it; *every* channel has to be past `release` to clear it, which is
	/// the worst one being past it. Ties keep the earlier channel, so a page of
	/// four identical readings always highlights the same cell.
	fn worst(&self, value_of: &impl Fn(ChannelId) -> Option<f32>) -> Option<(ChannelId, f32)> {
		let mut worst: Option<(ChannelId, f32)> = None;
		for (i, want) in self.channels.iter().enumerate() {
			let Some(v) = self.reading(i, *want, value_of) else { continue };
			let further = match self.rule {
				Rule::Threshold { direction, .. } => direction,
				// The further from the specified value, the worse — whichever side it is on.
				Rule::Drift { .. } => Direction::Above,
			};
			match worst {
				Some((_, best)) if !further.worse(v, best) => {}
				_ => worst = Some((*want, v)),
			}
		}
		worst
	}

	/// What the rule compares for one channel of its own, by name rather than by rank.
	fn reading_of(&self, channel: ChannelId, value_of: &impl Fn(ChannelId) -> Option<f32>) -> Option<f32> {
		let i = self.channels.iter().position(|c| *c == channel)?;
		self.reading(i, channel, value_of)
	}

	/// What the rule compares: the reading itself, or how far it is from the value its unit
	/// asked for, as a share of that value.
	///
	/// `None` where there is no evidence — the channel or its specified value has not
	/// answered — and where the specified value is under `min_setpoint`, because a percentage
	/// of nearly zero says nothing about the engine.
	fn reading(&self, i: usize, channel: ChannelId, value_of: &impl Fn(ChannelId) -> Option<f32>) -> Option<f32> {
		match self.rule {
			Rule::Threshold { .. } => value_of(channel),
			Rule::Drift { specified, min_setpoint, .. } => {
				let actual = value_of(channel)?;
				let wanted = value_of(*specified.get(i)?)?;
				let magnitude = abs(wanted);
				if magnitude < min_setpoint || magnitude == 0.0 {
					return None;
				}
				Some(abs(actual - wanted) / magnitude * 100.0)
			}
		}
	}
}

/// `f32::abs` is in `std`; this crate is `no_std` and needs the three lines.
fn abs(v: f32) -> f32 {
	if v < 0.0 { -v } else { v }
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
	/// Out of bounds, but not yet for as long as the rule asks. Nothing is on the screen
	/// (`todo/dash/18` §4: a turbo lags its setpoint on every throttle stab).
	Rising { offender: ChannelId, since_ms: u64 },
	/// Out of bounds now. `offender` is the cell to invert and follows the worst
	/// channel while it lasts. The episode's blink counts from `blink_from_ms`: when it
	/// fired, fired again out of the hold, or took the glass from another rule.
	///
	/// Still `Firing`, and so still blinking, when the value is back inside the
	/// hysteresis band but not past the release, and when its channels stop answering:
	/// neither is a release. Deliberate (controller, 2026-09-26), not a missed case.
	Firing { offender: ChannelId, blink_from_ms: u64 },
	/// Back inside, still on the screen until `until_ms`. The offender is frozen
	/// at whoever it last was, so the view does not end with nothing highlighted.
	Holding { offender: ChannelId, until_ms: u64 },
	/// Dismissed by the button. Still polled — a release is what re-arms it.
	Silenced,
}

impl Episode {
	/// The cell the episode points at, and how — `None` while nothing is on the glass.
	fn showing(self) -> Option<(ChannelId, Highlight)> {
		match self {
			Episode::Firing { offender, blink_from_ms } => Some((offender, Highlight::Blinking { from_ms: blink_from_ms })),
			Episode::Holding { offender, .. } => Some((offender, Highlight::Steady)),
			Episode::Clear | Episode::Rising { .. } | Episode::Silenced => None,
		}
	}
}

/// How the offending cell is drawn, which says where its value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
	/// The episode is firing: inverted and plain by turns, [`BLINK_MS`] each, counted
	/// from `from_ms` so its first frame is inverted. That includes a value sitting in
	/// the hysteresis band and channels gone quiet — the alarm has not released.
	Blinking { from_ms: u64 },
	/// Back inside, in the [`HOLD_MS`] before the hand-back: inverted and still, so the
	/// driver sees the value has come back before the page goes.
	Steady,
}

impl Highlight {
	/// Whether the cell is drawn inverted on a frame at `now_ms`.
	pub fn inverted(self, now_ms: u64) -> bool {
		match self {
			// The first half of every blink inverted, from the moment the blink started.
			Highlight::Blinking { from_ms } => (now_ms.saturating_sub(from_ms) / BLINK_MS) % 2 == 0,
			Highlight::Steady => true,
		}
	}
}

/// What belongs on the glass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
	pub page: PageId,
	/// The channel the alarm points at, and by construction `Some` exactly when
	/// the page is up because of an alarm. Which frames draw it inverted is
	/// [`Shown::inverted`]; if the page does not contain it, nothing is inverted
	/// and that is a plan bug, not a render one.
	pub offending: Option<ChannelId>,
	/// How that cell is drawn: `Some` exactly when `offending` is.
	pub highlight: Option<Highlight>,
}

impl Shown {
	/// An ordinary screen, nothing wrong.
	pub const fn page(page: PageId) -> Self {
		Shown {
			page,
			offending: None,
			highlight: None,
		}
	}

	/// The channel whose cell is drawn inverted on a frame at `now_ms`: the offending one
	/// on the inverted half of a blink and all through the hold, otherwise none. The caller
	/// maps it to a cell and calls [`Cell::alarmed`](crate::Cell::alarmed).
	pub fn inverted(&self, now_ms: u64) -> Option<ChannelId> {
		self.offending.filter(|_| self.highlight.is_some_and(|h| h.inverted(now_ms)))
	}
}

/// The answer to one [`Alarms::poll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Update {
	pub shown: Shown,
	/// Did the page, or the cell it points at, just change? Not how that cell is
	/// drawn — the blink and the steady hold are [`Shown::inverted`]'s, per frame —
	/// and not the values, which move on their own. The first poll always reports
	/// `true`, because before it there was nothing on the glass.
	///
	/// Nothing outside the tests reads it: the board redraws every frame
	/// unconditionally, and [`Screen`](crate::screen::Screen) keeps its own
	/// `page_changed` for when the foreground channels move.
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

	/// Which rule owns the glass as of the last poll, by its place in the plan.
	pub fn showing(&self) -> Option<usize> {
		self.showing
	}

	/// The glass showed something else meanwhile (the adapter screen): the next poll
	/// counts whichever alarm is up as taking it anew, so its blink starts inverted
	/// rather than wherever the clock has reached since it last showed.
	pub fn glass_lost(&mut self) {
		self.showing = None;
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
		self.rules.iter().flat_map(|r| {
			// A drift rule needs both halves of every pair: the specified value is as much a
			// reading as the actual one, and a rule cannot see what nobody asks for.
			let specified = match r.rule {
				Rule::Threshold { .. } => [].as_slice(),
				Rule::Drift { specified, .. } => specified,
			};
			r.channels.iter().chain(specified.iter()).copied()
		})
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
		self.poll_with(page, |channel| value_of(channel, readings), now_ms)
	}

	/// [`Alarms::poll`], with the readings as a lookup rather than a list: what the
	/// board has is a store indexed by channel, and copying the watched part of it
	/// into a slice first would need a buffer this crate has no allocator for.
	/// `None` is the same no-evidence it is in a [`Reading`].
	pub fn poll_with(&mut self, page: PageId, value_of: impl Fn(ChannelId) -> Option<f32>, now_ms: u64) -> Update {
		for i in 0..N {
			self.episodes[i] = step(&self.rules[i], self.episodes[i], &value_of, now_ms);
		}

		let first = (0..N).find(|&i| self.episodes[i].showing().is_some());
		// A rule taking the glass from another has been firing behind it for however long:
		// its blink starts now, inverted first, like any takeover's.
		if let Some(i) = first.filter(|_| first != self.showing) {
			if let Episode::Firing { blink_from_ms, .. } = &mut self.episodes[i] {
				*blink_from_ms = now_ms;
			}
		}
		self.showing = first;
		let up = first.and_then(|i| Some((i, self.episodes[i].showing()?)));
		let shown = match up {
			Some((i, (offender, highlight))) => Shown {
				page: self.rules[i].page,
				offending: Some(offender),
				highlight: Some(highlight),
			},
			None => Shown::page(page),
		};

		// Which page, and which cell on it — not how the cell is drawn this moment.
		let changed = self.last.is_none_or(|last| (last.page, last.offending) != (shown.page, shown.offending));
		self.last = Some(shown);
		Update { shown, changed }
	}
}

/// One rule, one poll. Pulled out of the loop because it is the state machine
/// and everything around it is bookkeeping.
fn step(rule: &Alarm<'_>, episode: Episode, value_of: &impl Fn(ChannelId) -> Option<f32>, now_ms: u64) -> Episode {
	let worst = rule.worst(value_of);
	// A rule with no hold fires the moment it trips; one with a hold starts counting.
	let start_at = |c: ChannelId, at: u64| match rule.hold_ms() {
		0 => Episode::Firing {
			offender: c,
			blink_from_ms: at,
		},
		_ => Episode::Rising { offender: c, since_ms: at },
	};
	let start = |c: ChannelId| start_at(c, now_ms);
	match episode {
		Episode::Clear => match worst {
			Some((c, v)) if rule.trips(v) => start(c),
			_ => Episode::Clear,
		},
		// Counting, for **one channel**: a rule watching four cylinders must not add a second
		// of one to a second of another, and a channel that comes back and goes out again
		// starts over.
		Episode::Rising { offender, since_ms } => {
			// The count belongs to the channel that started it, and is kept while **that
			// channel** is still out — not while it is still the worst. Two cylinders trading
			// places every poll are both out the whole time, and a rule that restarted on
			// every swap would never fire (review, 2026-09-15).
			let still_out = rule.reading_of(offender, value_of).is_some_and(|v| rule.trips(v));
			match (still_out, worst) {
				(true, _) if now_ms.saturating_sub(since_ms) >= rule.hold_ms() => Episode::Firing {
					// Once it fires, the cell to invert is the worst one, as everywhere else.
					offender: worst.map_or(offender, |(c, _)| c),
					blink_from_ms: now_ms,
				},
				(true, _) => Episode::Rising { offender, since_ms },
				// It came back, or stopped answering, and somebody else is out: that channel's
				// own count starts here rather than inheriting this one's seconds.
				(false, Some((c, v))) if rule.trips(v) => start_at(c, now_ms),
				// Nobody is out, or there is no evidence at all — nothing answered, or the
				// specified value is under the floor. The count ends: a hold that survived the
				// gaps between what it was counting would not be a hold, and a transient is
				// what this state exists to swallow.
				_ => Episode::Clear,
			}
		}
		Episode::Firing { offender, blink_from_ms } => match worst {
			// The offender follows the engine: a worse cylinder is the one worth
			// pointing at, even mid-episode.
			Some((c, v)) if rule.trips(v) => Episode::Firing { offender: c, blink_from_ms },
			Some((_, v)) if rule.releases(v) => Episode::Holding {
				offender,
				until_ms: now_ms.saturating_add(HOLD_MS),
			},
			// Inside the hysteresis band, or nothing answered. Neither of those is
			// news, so the screen does not move.
			_ => Episode::Firing { offender, blink_from_ms },
		},
		Episode::Holding { offender, until_ms } => match worst {
			// Out again before the hold expired: the same episode continues, so it
			// does not re-announce itself and the driver's silence still applies. A rule with
			// a hold does not have to earn it a second time inside one episode. The blink
			// starts over here, inverted first, as it does at a takeover.
			Some((c, v)) if rule.trips(v) => Episode::Firing {
				offender: c,
				blink_from_ms: now_ms,
			},
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

	/// The four channels one rule watches, by plan index: what a rule watches is
	/// named, not counted by where it sits in the readings, because the polled set
	/// is a union whose order changes with the page. Neutral numbers throughout —
	/// no channel, identifier or threshold of any car.
	const GROUP: [ChannelId; 4] = [ChannelId(0), ChannelId(1), ChannelId(2), ChannelId(3)];
	const SINGLE: [ChannelId; 1] = [ChannelId(4)];

	const GROUP_PAGE: PageId = PageId(2);
	const SINGLE_PAGE: PageId = PageId(3);
	/// Where the driver was before any of this happened.
	const WAS_SHOWING: PageId = PageId(1);
	/// A reading well clear of the group rule's release.
	const CLEAR: f32 = 20.0;

	/// Fires at or below 0, clears above 5.
	fn group() -> Alarm<'static> {
		Alarm::below(&GROUP, GROUP_PAGE, 0.0, 5.0)
	}

	/// Fires at or above 10, clears below 8.
	fn single() -> Alarm<'static> {
		Alarm::above(&SINGLE, SINGLE_PAGE, 10.0, 8.0)
	}

	// --- drift: a channel against what its unit asked for ------------------------------

	/// Two channels, each paired with the specified value the plan resolved for it.
	const DRIFTING: [ChannelId; 2] = [ChannelId(10), ChannelId(11)];
	const SPECIFIED: [ChannelId; 2] = [ChannelId(20), ChannelId(21)];
	const DRIFT_PAGE: PageId = PageId(4);
	/// Over 10 % for a second fires it; under 6 % clears it; a specified value under 0.5 is
	/// not worth a percentage.
	fn drift() -> Alarm<'static> {
		Alarm::drift(&DRIFTING, &SPECIFIED, DRIFT_PAGE, 10.0, 6.0, 1_000, 0.5)
	}

	/// One drifting channel and its specified value; the second pair says nothing.
	fn pair(actual: Option<f32>, specified: Option<f32>) -> [Reading; 2] {
		[Reading::new(DRIFTING[0], actual), Reading::new(SPECIFIED[0], specified)]
	}

	#[test]
	fn a_drift_rule_is_polled_with_both_halves_of_every_pair() {
		let alarms = Alarms::new([drift()]);
		let watched: std::vec::Vec<ChannelId> = alarms.watched().collect();
		for channel in DRIFTING.iter().chain(SPECIFIED.iter()) {
			assert!(watched.contains(channel), "{channel:?} is polled");
		}
	}

	#[test]
	fn a_drift_rule_fires_only_once_it_has_held() {
		let mut alarms = Alarms::new([drift()]);
		// 2.2 against 2.0 is 10 %, which trips — but not yet.
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 0).shown, Shown::page(WAS_SHOWING));
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 999).shown, Shown::page(WAS_SHOWING));
		let up = alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_000);
		assert_eq!(up.shown.page, DRIFT_PAGE, "a second of drift is the rule's own condition");
		assert_eq!(up.shown.offending, Some(DRIFTING[0]));
	}

	#[test]
	fn a_transient_never_fires_and_starts_the_count_over() {
		let mut alarms = Alarms::new([drift()]);
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 0);
		// Back inside before the second is up: a throttle stab, not drift.
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.0), Some(2.0)), 500).shown, Shown::page(WAS_SHOWING));
		// Out again: the count is the new one's, so 1 100 is too early and 1 600 is not.
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 600);
		assert_eq!(
			alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_100).shown,
			Shown::page(WAS_SHOWING)
		);
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_600).shown.page, DRIFT_PAGE);
	}

	#[test]
	fn a_hold_belongs_to_one_channel_and_is_not_handed_over() {
		let mut alarms = Alarms::new([drift()]);
		let both = |a: f32, b: f32| {
			[
				Reading::new(DRIFTING[0], Some(a)),
				Reading::new(SPECIFIED[0], Some(2.0)),
				Reading::new(DRIFTING[1], Some(b)),
				Reading::new(SPECIFIED[1], Some(2.0)),
			]
		};
		// The first channel is out for 900 ms, then comes back as the second goes out.
		alarms.poll(WAS_SHOWING, &both(2.2, 2.0), 0);
		alarms.poll(WAS_SHOWING, &both(2.2, 2.0), 900);
		assert_eq!(
			alarms.poll(WAS_SHOWING, &both(2.0, 2.2), 1_000).shown,
			Shown::page(WAS_SHOWING),
			"the second channel does not inherit the first one's second"
		);
		// It earns its own, from where it started.
		assert_eq!(alarms.poll(WAS_SHOWING, &both(2.0, 2.2), 1_999).shown, Shown::page(WAS_SHOWING));
		let up = alarms.poll(WAS_SHOWING, &both(2.0, 2.2), 2_000);
		assert_eq!(up.shown.page, DRIFT_PAGE);
		assert_eq!(up.shown.offending, Some(DRIFTING[1]));
	}

	#[test]
	fn two_channels_taking_turns_at_being_worst_still_fire() {
		// Both are out the whole second; which one is worse changes every poll. The rule
		// counts the drift, not the ranking.
		let mut alarms = Alarms::new([drift()]);
		let readings = |a: f32, b: f32| {
			[
				Reading::new(DRIFTING[0], Some(a)),
				Reading::new(SPECIFIED[0], Some(2.0)),
				Reading::new(DRIFTING[1], Some(b)),
				Reading::new(SPECIFIED[1], Some(2.0)),
			]
		};
		let mut fired = None;
		for step in 0..=20u64 {
			// 12 % and 13 % out, swapping places each poll.
			let (a, b) = if step % 2 == 0 { (2.26, 2.24) } else { (2.24, 2.26) };
			let up = alarms.poll(WAS_SHOWING, &readings(a, b), step * 100);
			if up.shown.page == DRIFT_PAGE && fired.is_none() {
				fired = Some(step * 100);
			}
		}
		assert_eq!(fired, Some(1_000), "it fires on the hold, whoever is worst that poll");
	}

	#[test]
	fn a_hold_does_not_survive_a_gap_in_the_evidence() {
		let mut alarms = Alarms::new([drift()]);
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 0);
		// Half a minute with the specified value under the floor: no evidence either way, and
		// the count is not kept across it.
		alarms.poll(WAS_SHOWING, &pair(Some(0.1), Some(0.4)), 30_000);
		assert_eq!(
			alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 60_000).shown,
			Shown::page(WAS_SHOWING),
			"one sample after the gap is not a second of drift"
		);
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 61_000).shown.page, DRIFT_PAGE);

		// The same for a value that sits in the hysteresis band meanwhile, and for a pair that
		// stops answering.
		for quiet in [pair(Some(2.16), Some(2.0)), pair(None, None)] {
			let mut alarms = Alarms::new([drift()]);
			alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 0);
			alarms.poll(WAS_SHOWING, &quiet, 500);
			assert_eq!(
				alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_000).shown,
				Shown::page(WAS_SHOWING)
			);
		}
	}

	#[test]
	fn a_specified_value_under_the_floor_says_nothing() {
		let mut alarms = Alarms::new([drift()]);
		// 0.4 asked for, 0.1 delivered: 75 % out, and meaningless.
		for now in [0, 1_000, 5_000] {
			assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(0.1), Some(0.4)), now).shown, Shown::page(WAS_SHOWING));
		}
	}

	#[test]
	fn a_pair_that_has_not_answered_says_nothing() {
		let mut alarms = Alarms::new([drift()]);
		for readings in [pair(None, Some(2.0)), pair(Some(2.2), None), pair(None, None)] {
			assert_eq!(alarms.poll(WAS_SHOWING, &readings, 5_000).shown, Shown::page(WAS_SHOWING));
		}
	}

	#[test]
	fn a_drift_rule_clears_under_its_release_and_holds_the_view_first() {
		let mut alarms = Alarms::new([drift()]);
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 0);
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_000).shown.page, DRIFT_PAGE);
		// 2.1 against 2.0 is 5 %, under the release.
		assert_eq!(
			alarms.poll(WAS_SHOWING, &pair(Some(2.1), Some(2.0)), 1_100).shown.page,
			DRIFT_PAGE,
			"the view is held so the driver can read it"
		);
		assert_eq!(
			alarms.poll(WAS_SHOWING, &pair(Some(2.1), Some(2.0)), 1_100 + HOLD_MS).shown,
			Shown::page(WAS_SHOWING)
		);
	}

	#[test]
	fn the_offending_cell_is_the_one_furthest_from_what_it_was_asked_for() {
		let mut alarms = Alarms::new([drift()]);
		// The first is 10 % out, the second 25 %: the second is the one to point at, either
		// way round.
		let readings = [
			Reading::new(DRIFTING[0], Some(2.2)),
			Reading::new(SPECIFIED[0], Some(2.0)),
			Reading::new(DRIFTING[1], Some(2.5)),
			Reading::new(SPECIFIED[1], Some(2.0)),
		];
		alarms.poll(WAS_SHOWING, &readings, 0);
		let up = alarms.poll(WAS_SHOWING, &readings, 1_000);
		assert_eq!(up.shown.offending, Some(DRIFTING[1]));
	}

	#[test]
	fn a_press_silences_a_drift_episode_until_it_releases() {
		let mut alarms = Alarms::new([drift()]);
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 0);
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_000);
		assert_eq!(alarms.press(), Press::Silenced);
		assert_eq!(
			alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 2_000).shown,
			Shown::page(WAS_SHOWING)
		);
		// Released, so the rule is armed again — and has to hold once more.
		alarms.poll(WAS_SHOWING, &pair(Some(2.0), Some(2.0)), 3_000);
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 4_000);
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 5_000).shown.page, DRIFT_PAGE);
	}

	/// The group's four readings, in channel order.
	fn cells(v: [f32; 4]) -> [Reading; 4] {
		[
			Reading::new(GROUP[0], Some(v[0])),
			Reading::new(GROUP[1], Some(v[1])),
			Reading::new(GROUP[2], Some(v[2])),
			Reading::new(GROUP[3], Some(v[3])),
		]
	}

	/// The same, with channel 1 the only interesting one.
	fn only(v: f32) -> [Reading; 4] {
		cells([CLEAR, v, CLEAR, CLEAR])
	}

	#[test]
	fn hysteresis_takes_the_screen_once_where_one_threshold_would_take_it_ten_times() {
		// A value sitting exactly on the trip point and breathing either side of it,
		// never past the release. With a single threshold this is a screen swapping
		// at the poll rate, which is the failure the whole rule exists to avoid.
		let mut alarms = Alarms::new([group()]);
		let series = [0.0, 1.0, -1.0, 0.5, 0.0, 4.0, -4.0, 3.0];
		let mut takeovers = 0;
		for (tick, v) in series.iter().enumerate() {
			let update = alarms.poll(WAS_SHOWING, &only(*v), tick as u64 * 100);
			if update.changed && update.shown.page == GROUP_PAGE {
				takeovers += 1;
			}
			assert_eq!(update.shown.page, GROUP_PAGE, "the episode never ends inside the band");
		}
		assert_eq!(takeovers, 1, "one episode, not eight");
	}

	#[test]
	fn it_releases_at_the_release_value_and_not_at_the_trip_value() {
		let mut alarms = Alarms::new([group()]);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(0.0), 0).shown.page, GROUP_PAGE);
		// Back above the trip point, but still inside the band: not released, so the
		// hold has not even started and the view stays up however long we wait.
		assert_eq!(alarms.poll(WAS_SHOWING, &only(1.0), 1_000).shown.page, GROUP_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(1.0), 100_000).shown.page, GROUP_PAGE);
		// Past the release value the hold starts, and only then does it run out.
		assert_eq!(alarms.poll(WAS_SHOWING, &only(6.0), 100_000).shown.page, GROUP_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(6.0), 100_000 + HOLD_MS).shown.page, WAS_SHOWING);
	}

	#[test]
	fn the_view_stays_up_for_the_hold_and_then_hands_back_to_where_you_were() {
		let mut alarms = Alarms::new([group()]);
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		let released_at = 500;
		assert_eq!(alarms.poll(WAS_SHOWING, &only(CLEAR), released_at).shown.page, GROUP_PAGE);
		// One millisecond short of the hold, it is still the alarm's screen.
		let almost = alarms.poll(WAS_SHOWING, &only(CLEAR), released_at + HOLD_MS - 1);
		assert_eq!(almost.shown.page, GROUP_PAGE);
		assert!(!almost.changed);
		// And at the hold it hands back — to the page that was showing, by identity.
		let back = alarms.poll(WAS_SHOWING, &only(CLEAR), released_at + HOLD_MS);
		assert_eq!(back.shown, Shown::page(WAS_SHOWING));
		assert!(back.changed, "the glass has to be redrawn");
	}

	#[test]
	fn it_hands_back_to_the_page_that_was_showing_and_never_to_page_one() {
		let mut alarms = Alarms::new([group()]);
		// A different page from the one the previous test used: what comes back is
		// whatever the caller says is current, which is the only definition of
		// "where you were" that survives the driver having paged around.
		let elsewhere = PageId(5);
		alarms.poll(elsewhere, &only(-10.0), 0);
		alarms.poll(elsewhere, &only(CLEAR), 10);
		assert_eq!(alarms.poll(elsewhere, &only(CLEAR), 10 + HOLD_MS).shown.page, elsewhere);
	}

	#[test]
	fn a_press_silences_the_episode_and_the_screen_goes_back_at_once() {
		let mut alarms = Alarms::new([group()]);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-10.0), 0).shown.page, GROUP_PAGE);
		assert_eq!(alarms.showing(), Some(0));
		assert_eq!(alarms.press(), Press::Silenced);
		let after = alarms.poll(WAS_SHOWING, &only(-10.0), 100);
		assert_eq!(after.shown, Shown::page(WAS_SHOWING));
		assert_eq!(alarms.showing(), None);
		assert!(after.changed);
	}

	#[test]
	fn a_silenced_alarm_stays_silent_while_the_value_is_still_out() {
		// A fault that does not go away. Without this the display is one frozen
		// screen for the rest of the drive.
		let mut alarms = Alarms::new([group()]);
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		alarms.press();
		for tick in 1..200u64 {
			let update = alarms.poll(WAS_SHOWING, &only(-10.0 - tick as f32), tick * 100);
			assert_eq!(update.shown, Shown::page(WAS_SHOWING), "silenced at t={}", tick * 100);
		}
	}

	#[test]
	fn a_fresh_crossing_after_a_release_arms_it_again() {
		let mut alarms = Alarms::new([group()]);
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		alarms.press();
		// Silence is not deafness: the release is still watched, and it re-arms.
		assert_eq!(alarms.poll(WAS_SHOWING, &only(CLEAR), 1_000).shown.page, WAS_SHOWING);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-5.0), 2_000).shown.page, GROUP_PAGE);
	}

	#[test]
	fn two_alarms_crossing_in_the_same_poll_resolve_by_priority_not_by_luck() {
		let mut alarms = Alarms::new([group(), single()]);
		let readings = [
			Reading::new(GROUP[0], Some(CLEAR)),
			Reading::new(GROUP[1], Some(-20.0)),
			Reading::new(GROUP[2], Some(CLEAR)),
			Reading::new(GROUP[3], Some(CLEAR)),
			Reading::new(SINGLE[0], Some(12.0)),
		];
		let first = alarms.poll(WAS_SHOWING, &readings, 0);
		assert_eq!(first.shown.page, GROUP_PAGE, "the plan lists the rules in priority order");
		// And it is stable: the same input a second later does not swap them.
		assert!(!alarms.poll(WAS_SHOWING, &readings, 100).changed);
	}

	#[test]
	fn silencing_the_showing_alarm_lets_the_one_behind_it_through() {
		let mut alarms = Alarms::new([group(), single()]);
		let readings = [
			Reading::new(GROUP[0], Some(-20.0)),
			Reading::new(GROUP[1], Some(CLEAR)),
			Reading::new(GROUP[2], Some(CLEAR)),
			Reading::new(GROUP[3], Some(CLEAR)),
			Reading::new(SINGLE[0], Some(12.0)),
		];
		alarms.poll(WAS_SHOWING, &readings, 0);
		assert_eq!(alarms.press(), Press::Silenced);
		// One press, one episode: the second alarm is a different thing to say.
		assert_eq!(alarms.poll(WAS_SHOWING, &readings, 100).shown.page, SINGLE_PAGE);
		assert_eq!(alarms.showing(), Some(1));
		assert_eq!(alarms.press(), Press::Silenced);
		assert_eq!(alarms.poll(WAS_SHOWING, &readings, 200).shown, Shown::page(WAS_SHOWING));
	}

	#[test]
	fn the_offending_cell_is_the_worst_channel_and_not_the_first_one_over() {
		let mut alarms = Alarms::new([group()]);
		let update = alarms.poll(WAS_SHOWING, &cells([-1.0, 16.0, -19.0, -2.0]), 0);
		assert_eq!(update.shown.offending, Some(GROUP[2]));
		// It follows the value while the episode runs...
		let update = alarms.poll(WAS_SHOWING, &cells([-25.0, 16.0, -19.0, -2.0]), 100);
		assert_eq!(update.shown.offending, Some(GROUP[0]));
		assert!(update.changed, "a different cell is a different picture");
		// ...and freezes on the last offender once the values come back inside, so
		// the hold does not end on a page with nothing highlighted.
		let update = alarms.poll(WAS_SHOWING, &cells([CLEAR; 4]), 200);
		assert_eq!(update.shown.offending, Some(GROUP[0]));
	}

	// --- the highlight: blinking while out, steady in the hold -------------------------

	fn blinking(highlight: Option<Highlight>) -> bool {
		matches!(highlight, Some(Highlight::Blinking { .. }))
	}

	#[test]
	fn while_out_the_offending_cell_blinks_from_the_takeover() {
		let mut alarms = Alarms::new([group()]);
		alarms.poll(WAS_SHOWING, &only(CLEAR), 0);
		// Out from 700 ms, which is no boundary of the clock's: the blink starts there.
		// Frames 200 ms apart, the board's period: two inverted, two plain.
		let took = 700;
		let mut seen = std::vec::Vec::new();
		for t in (took..=took + 1_400).step_by(200) {
			let shown = alarms.poll(WAS_SHOWING, &only(-10.0), t).shown;
			assert!(blinking(shown.highlight), "out at t={t}");
			assert_eq!(shown.offending, Some(GROUP[1]), "what is pointed at does not blink");
			seen.push(shown.inverted(t) == Some(GROUP[1]));
		}
		assert_eq!(seen, [true, true, false, false, true, true, false, false]);
		// The halves are BLINK_MS from the takeover, to the millisecond.
		let shown = alarms.poll(WAS_SHOWING, &only(-10.0), took + 1_600).shown;
		assert_eq!(shown.inverted(took + BLINK_MS - 1), Some(GROUP[1]));
		assert_eq!(shown.inverted(took + BLINK_MS), None);
		assert_eq!(shown.inverted(took + BLINK_MS * 2 - 1), None);
		assert_eq!(shown.inverted(took + BLINK_MS * 2), Some(GROUP[1]));
	}

	#[test]
	fn the_first_frame_of_a_takeover_is_inverted_whenever_it_comes() {
		// Nothing else on the glass says "this one" when the driver is already on the rule's
		// page: the first frame has to.
		for took in [1, 250, 399, 400, 401, 777, 1_234, 10_001] {
			let mut alarms = Alarms::new([group()]);
			alarms.poll(WAS_SHOWING, &only(CLEAR), 0);
			let shown = alarms.poll(WAS_SHOWING, &only(-10.0), took).shown;
			assert_eq!(shown.inverted(took), Some(GROUP[1]), "took at {took}");
		}
	}

	#[test]
	fn the_cell_keeps_blinking_in_the_band_and_while_its_channel_is_quiet() {
		// Neither is a release, so the episode is still `Firing`, and its phase runs on.
		let mut alarms = Alarms::new([group()]);
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		// Back above the trip, not past the release.
		let band = alarms.poll(WAS_SHOWING, &only(1.0), BLINK_MS).shown;
		assert!(blinking(band.highlight));
		assert_eq!(band.inverted(BLINK_MS), None, "the phase from the takeover, not restarted");
		// Nothing answering at all.
		let quiet = alarms.poll(WAS_SHOWING, &[], BLINK_MS * 2).shown;
		assert!(blinking(quiet.highlight));
		assert_eq!(quiet.inverted(BLINK_MS * 2), Some(GROUP[1]));
	}

	#[test]
	fn through_the_hold_the_cell_is_steady_and_after_the_hand_back_nothing_is() {
		let mut alarms = Alarms::new([group()]);
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		let released_at = 500;
		let release = alarms.poll(WAS_SHOWING, &only(CLEAR), released_at);
		assert_eq!(release.shown.highlight, Some(Highlight::Steady));
		assert!(!release.changed, "the same page and the same cell");
		// Every frame of the hold, whichever half of a blink the clock is in.
		for t in (released_at..released_at + HOLD_MS).step_by(100) {
			let shown = alarms.poll(WAS_SHOWING, &only(CLEAR), t).shown;
			assert_eq!(shown.highlight, Some(Highlight::Steady), "holding at t={t}");
			assert_eq!(shown.inverted(t), Some(GROUP[1]), "inverted at t={t}");
		}
		let back = alarms.poll(WAS_SHOWING, &only(CLEAR), released_at + HOLD_MS).shown;
		assert_eq!(back, Shown::page(WAS_SHOWING));
		assert_eq!(back.inverted(released_at + HOLD_MS), None);
	}

	#[test]
	fn out_again_inside_the_hold_blinks_again_from_that_moment() {
		let mut alarms = Alarms::new([group()]);
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(CLEAR), 100).shown.highlight, Some(Highlight::Steady));
		let again_at = 1_300;
		let again = alarms.poll(WAS_SHOWING, &only(-10.0), again_at).shown;
		assert!(blinking(again.highlight));
		assert_eq!(again.inverted(again_at), Some(GROUP[1]), "inverted first, as at any takeover");
		assert_eq!(again.inverted(again_at + BLINK_MS), None);
	}

	#[test]
	fn a_silenced_episode_highlights_nothing() {
		let mut alarms = Alarms::new([group()]);
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		alarms.press();
		for t in (100..=2_000).step_by(100) {
			let shown = alarms.poll(WAS_SHOWING, &only(-10.0), t).shown;
			assert_eq!((shown.offending, shown.highlight, shown.inverted(t)), (None, None, None), "t={t}");
		}
	}

	#[test]
	fn a_drift_rule_counting_its_hold_highlights_nothing_and_blinks_once_it_fires() {
		let mut alarms = Alarms::new([drift()]);
		let counting = alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 200).shown;
		assert_eq!((counting.highlight, counting.inverted(200)), (None, None));
		// A second after it started counting: the blink starts when it fires, not before.
		let fired = alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_200).shown;
		assert!(blinking(fired.highlight));
		assert_eq!(fired.inverted(1_200), Some(DRIFTING[0]));
	}

	#[test]
	fn an_alarm_can_fire_above_a_threshold_as_well_as_below_one() {
		let mut alarms = Alarms::new([single()]);
		let up = [Reading::new(SINGLE[0], Some(11.0))];
		let band = [Reading::new(SINGLE[0], Some(9.0))];
		let down = [Reading::new(SINGLE[0], Some(7.0))];
		assert_eq!(alarms.poll(WAS_SHOWING, &up, 0).shown.page, SINGLE_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &band, 100).shown.page, SINGLE_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &down, 200).shown.page, SINGLE_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &down, 200 + HOLD_MS).shown.page, WAS_SHOWING);
	}

	#[test]
	fn a_channel_that_stops_answering_neither_trips_nor_releases() {
		let mut alarms = Alarms::new([group()]);
		let silence = [Reading::new(GROUP[0], None)];
		// Nothing on the bus is not a reading of zero, so it cannot fire...
		assert_eq!(alarms.poll(WAS_SHOWING, &silence, 0).shown, Shown::page(WAS_SHOWING));
		// ...and it cannot end an episode either: a unit that drops out mid-alarm
		// has not said the value is fine. The button is the way out.
		alarms.poll(WAS_SHOWING, &only(-10.0), 100);
		assert_eq!(alarms.poll(WAS_SHOWING, &silence, 100 + HOLD_MS * 4).shown.page, GROUP_PAGE);
		alarms.press();
		assert_eq!(alarms.poll(WAS_SHOWING, &silence, 100 + HOLD_MS * 5).shown, Shown::page(WAS_SHOWING));
	}

	#[test]
	fn a_press_with_no_alarm_showing_is_a_page_turn() {
		let mut alarms = Alarms::new([group()]);
		alarms.poll(WAS_SHOWING, &only(CLEAR), 0);
		assert_eq!(alarms.press(), Press::NextPage);
	}

	#[test]
	fn nothing_moving_reports_nothing_changed_after_the_first_look() {
		let mut alarms = Alarms::new([group()]);
		// The first poll always changes: there was nothing on the glass before it.
		assert!(alarms.poll(WAS_SHOWING, &only(CLEAR), 0).changed);
		assert!(!alarms.poll(WAS_SHOWING, &only(CLEAR), 100).changed);
		assert!(!alarms.poll(WAS_SHOWING, &only(11.0), 200).changed);
		// Paging is the caller's business, and it is still a change of screen.
		assert!(alarms.poll(PageId(4), &only(CLEAR), 300).changed);
	}

	#[test]
	fn every_rules_channels_are_watched_even_while_it_is_silenced() {
		let mut alarms = Alarms::new([group(), single()]);
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		alarms.press();
		let watched: [ChannelId; 5] = core::array::from_fn(|i| alarms.watched().nth(i).unwrap());
		assert_eq!(watched, [GROUP[0], GROUP[1], GROUP[2], GROUP[3], SINGLE[0]]);
		assert_eq!(
			alarms.watched().count(),
			5,
			"a silenced rule is still polled — release is what re-arms it"
		);
	}
}
