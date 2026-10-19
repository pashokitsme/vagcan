//! When a channel goes wrong, it takes the screen.
//!
//! An alarm is part of a channel (owner, 2026-10-19): a channel may carry an upper bound, a
//! lower one, or both — `dash.toml`'s `alarm_max` and `alarm_min`, each with the value it goes
//! off at — and a channel paired with a specified value may carry a drift rule. The board
//! watches every such channel in the background, on every page, and when one crosses it raises
//! the **first values page that shows the channel** and inverts its cell — or, for a channel no
//! values page shows, its chart page, whose header is inverted. That is the whole rule; everything below is about it not being annoying, because an alarm that flickers or
//! that cannot be dismissed is one the driver learns to ignore, and then it may as well not
//! exist.
//!
//! Three mechanisms, all of them required (`todo/dash/04-alarms.md`):
//!
//! 1. **An "off" value apart from the bound.** It fires at the bound and goes off only once the
//!    value is back past its off value, further inside. A single threshold with a value hovering
//!    on it swaps the screen at the poll rate.
//! 2. **A hold after it goes off** — [`HOLD_MS`]. The view stays up that long after the value
//!    comes back inside, so a two-poll excursion is still readable.
//! 3. **It hands back to where you were.** The caller passes the page it *would* be showing on
//!    every poll, and that is what returns — not page one, and not a page this module
//!    remembers, which would be a second copy of a fact the caller already owns.
//!
//! And one escape hatch: **a press silences what is on the glass** — any command, from a
//! `[[button]]`, the lever or `dashsim` ([`Screen::command`](crate::screen::Screen::command)).
//! A board with none of them cannot silence, and waits for the value to come back. A genuinely
//! misfiring engine would otherwise freeze the display for the rest of the drive. Silence lasts
//! until the value goes off; a fresh crossing after that arms the rule again, so the escape is
//! per-episode and not a permanent switch-off nobody remembers flipping.
//!
//! # Several at once
//!
//! Every rule is one channel's and has its own episode; the rules are in **priority order**,
//! which is the order of the channels in `dash.toml` (a channel's bounds before its drift).
//! What the glass shows when several are out:
//!
//! - **The page is the first showing rule's** — the highest priority among the episodes that
//!   are firing or holding. A page raised by a lower rule never replaces it.
//! - **On that page every channel it shows whose rule is showing is inverted**, each as its own
//!   episode says (blinking while out, steady through the hold) — four cylinders out at once are
//!   four inverted cells, not one. That includes a channel whose own alarm raises another page:
//!   it is on the glass, so it is marked there and silenced there. A channel out that the page
//!   does not show waits.
//! - **A press silences every episode on the glass**, the page the driver is looking at: what
//!   they see is what they dismiss. A rule out on another page then takes the glass on the next
//!   poll and wants its own press, because it is a different thing to be told.
//!
//! The old `[[alarm]]` rule watched several channels as one episode; one rule per channel keeps
//! its semantics on a page — one takeover, one press, the worst cells marked — and adds that a
//! channel newly out on a page nobody silenced is news of its own.
//!
//! Nothing here reads a clock. Every entry point takes `now_ms`, because the two callers are
//! `embassy_time` on the board and `std::time` on a laptop, and because a machine you can hand
//! a synthetic clock is a machine you can test — which is the only way the 2.5 second hold is
//! ever exercised.
//!
//! What comes out is a [`Shown`]: a page, and the cells on it to draw inverted — blinking
//! ([`BLINK_MS`]) while the value is out, steady through the hold. Inverting *the offending
//! cell* rather than the panel is the point of the view — see [`Cell::alarm`](crate::Cell::alarm).

use crate::plan::{MAX_VALUE_CELLS, Page};

/// How long the alarm view stays up after the value comes back inside.
///
/// The owner's number, 2026-08-20. Long enough to read a four-cylinder page,
/// short enough that a clean engine never keeps the screen.
pub const HOLD_MS: u64 = 2_500;

/// Half a blink of the offending cell: inverted this long, plain this long, while the
/// value is past the bound.
///
/// The owner's number, 2026-09-26: a cell that is merely inverted did not say "look here"
/// clearly enough. The phase counts from the episode's own start — `((now_ms − from) / BLINK_MS)
/// % 2`, inverted when it is 0 — so the first frame of an episode is always inverted, even when
/// the driver is already on the alarm's page and the cell is the only thing that changes.
/// `from` is the episode's own state, not a timer: the moment it fired, fired again out of the
/// hold, or its page took the glass.
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

/// One bound of a channel: where it fires, and where it goes off again.
///
/// An upper bound fires at or above `at` and goes off at or under `off`, so `off < at`; a
/// lower one fires at or below `at` and goes off at or above `off`, so `off > at`. The
/// generator has checked which side `off` is on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bound {
	pub at: f32,
	pub off: f32,
}

/// What a rule watches for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rule {
	/// The reading outside its bounds — `dash.toml`'s `alarm_max` and `alarm_min`, either or
	/// both. With both, `min.off < max.off`: there is a value that is off for both.
	Bounds { max: Option<Bound>, min: Option<Bound> },
	/// The reading too far from what its control unit asked for
	/// (`todo/dash/18-setpoints-and-drift.md` §4).
	Drift {
		/// The channel holding the specified value: the plan pairs them, so the rule reads both
		/// halves through the same lookup the rest does.
		specified: ChannelId,
		/// Fires when `|actual − specified|` reaches this share of `|specified|`, in percent.
		percent: f32,
		/// Goes off at or under this share.
		off_percent: f32,
		/// And only once it has been past `percent` this long without a break. A
		/// turbocharger lags its own setpoint on every throttle stab; without this the rule
		/// fires on every gear change.
		hold_ms: u64,
		/// Below this specified value the rule says nothing: a percentage of nearly zero is
		/// noise, not drift.
		min_setpoint: f32,
	},
}

/// One rule: a channel, the page that shows it, and what counts as wrong.
///
/// Plain data, so a plan can carry it as a `static`: the generator writes the fields
/// directly, having already checked what [`Alarm::bounds`] and [`Alarm::drift`] assert.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alarm {
	/// Watched whether or not it is on the screen.
	pub channel: ChannelId,
	/// The page to raise: the first values page of the plan that shows the channel, else its
	/// chart page — that is what makes the takeover an explanation rather than an interruption. The generator
	/// finds it, because pages live in the plan.
	pub page: PageId,
	pub rule: Rule,
}

impl Alarm {
	/// A channel with an upper bound, a lower one, or both.
	pub fn bounds(channel: ChannelId, page: PageId, max: Option<Bound>, min: Option<Bound>) -> Self {
		debug_assert!(max.is_some() || min.is_some(), "a bounds rule has a bound");
		debug_assert!(max.is_none_or(|b| b.off < b.at), "an upper bound goes off under where it fires");
		debug_assert!(min.is_none_or(|b| b.off > b.at), "a lower bound goes off over where it fires");
		Alarm {
			channel,
			page,
			rule: Rule::Bounds { max, min },
		}
	}

	/// A channel that fires when it has been `percent` or more away from what its unit asked
	/// for, for `hold_ms` without a break, and goes off at or under `off_percent`.
	pub fn drift(channel: ChannelId, specified: ChannelId, page: PageId, percent: f32, off_percent: f32, hold_ms: u64, min_setpoint: f32) -> Self {
		debug_assert!(off_percent < percent, "a drift rule goes off under where it fires");
		Alarm {
			channel,
			page,
			rule: Rule::Drift {
				specified,
				percent,
				off_percent,
				hold_ms,
				min_setpoint,
			},
		}
	}

	/// How long a reading has to stay wrong before the rule fires. Zero for a bound: a
	/// temperature past its limit is past it now.
	fn hold_ms(&self) -> u64 {
		match self.rule {
			Rule::Bounds { .. } => 0,
			Rule::Drift { hold_ms, .. } => hold_ms,
		}
	}

	fn trips(&self, v: f32) -> bool {
		match self.rule {
			Rule::Bounds { max, min } => max.is_some_and(|b| v >= b.at) || min.is_some_and(|b| v <= b.at),
			// `v` is the share of the specified value the channel is away from it.
			Rule::Drift { percent, .. } => v >= percent,
		}
	}

	/// Whether the reading is back inside every off value — where the episode ends.
	fn goes_off(&self, v: f32) -> bool {
		match self.rule {
			Rule::Bounds { max, min } => max.is_none_or(|b| v <= b.off) && min.is_none_or(|b| v >= b.off),
			Rule::Drift { off_percent, .. } => v <= off_percent,
		}
	}

	/// What the rule compares: the reading itself, or how far it is from the value its unit
	/// asked for, as a share of that value.
	///
	/// `None` where there is no evidence — the channel or its specified value has not
	/// answered — and where the specified value is under `min_setpoint`, because a percentage
	/// of nearly zero says nothing about the engine.
	fn reading(&self, value_of: &impl Fn(ChannelId) -> Option<f32>) -> Option<f32> {
		match self.rule {
			Rule::Bounds { .. } => value_of(self.channel),
			Rule::Drift { specified, min_setpoint, .. } => {
				let actual = value_of(self.channel)?;
				let wanted = value_of(specified)?;
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
	Rising { since_ms: u64 },
	/// Out of bounds now. The blink counts from `blink_from_ms`: when it fired, fired again
	/// out of the hold, or its page took the glass.
	///
	/// Still `Firing`, and so still blinking, when the value is back inside the bound but not
	/// yet at its off value, and when the channel stops answering: neither ends it.
	/// Deliberate (controller, 2026-09-26), not a missed case.
	Firing { blink_from_ms: u64 },
	/// Back inside, still on the screen until `until_ms`.
	Holding { until_ms: u64 },
	/// Dismissed by a press. Still polled — the value going off is what re-arms it.
	Silenced,
}

impl Episode {
	/// How the episode's cell is drawn — `None` while nothing is on the glass.
	fn showing(self) -> Option<Highlight> {
		match self {
			Episode::Firing { blink_from_ms } => Some(Highlight::Blinking { from_ms: blink_from_ms }),
			Episode::Holding { .. } => Some(Highlight::Steady),
			Episode::Clear | Episode::Rising { .. } | Episode::Silenced => None,
		}
	}
}

/// One rule's state, opaque: what the caller sizes for the plan — one per rule, all
/// [`Slot::CLEAR`] at the start ([`Alarms::new`]). On the board an array as long as the plan's
/// rules, on a laptop a `Vec`: the plan says how many, so nothing caps them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slot(Episode);

impl Slot {
	/// A rule with nothing wrong, armed.
	pub const CLEAR: Slot = Slot(Episode::Clear);
}

/// How the offending cell is drawn, which says where its value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
	/// The episode is firing: inverted and plain by turns, [`BLINK_MS`] each, counted
	/// from `from_ms` so its first frame is inverted. That includes a value back inside its
	/// bound but not yet at its off value, and a channel gone quiet — the alarm is not off.
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

/// One cell the alarm on the glass points at, and how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark {
	pub channel: ChannelId,
	pub highlight: Highlight,
}

/// The channels whose cells are drawn inverted on one frame: at most one per cell of a values
/// page, since every mark is a channel the page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Inverted([Option<ChannelId>; MAX_VALUE_CELLS]);

impl Inverted {
	/// Nothing inverted.
	pub const NONE: Inverted = Inverted([None; MAX_VALUE_CELLS]);

	/// Whether the cell of `channel` is drawn inverted.
	pub fn contains(&self, channel: ChannelId) -> bool {
		self.0.contains(&Some(channel))
	}

	/// The inverted channels, in priority order.
	pub fn iter(&self) -> impl Iterator<Item = ChannelId> + '_ {
		self.0.iter().flatten().copied()
	}

	pub fn is_empty(&self) -> bool {
		self.0.iter().all(Option::is_none)
	}
}

/// What belongs on the glass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown {
	pub page: PageId,
	/// The cells the alarm on the glass points at, in priority order — `marks[0]` is the
	/// leading rule's, the one that raised the page — and by construction some exactly when
	/// the page is up because of an alarm. A channel the page does not contain is drawn
	/// nowhere, and that is a plan bug, not a render one.
	pub marks: [Option<Mark>; MAX_VALUE_CELLS],
}

impl Shown {
	/// An ordinary screen, nothing wrong.
	pub const fn page(page: PageId) -> Self {
		Shown {
			page,
			marks: [None; MAX_VALUE_CELLS],
		}
	}

	/// The leading rule's channel: what the log names.
	pub fn offending(&self) -> Option<ChannelId> {
		self.marks[0].map(|m| m.channel)
	}

	/// How the leading rule's cell is drawn.
	pub fn highlight(&self) -> Option<Highlight> {
		self.marks[0].map(|m| m.highlight)
	}

	/// The channels whose cells are drawn inverted on a frame at `now_ms`: each marked one on
	/// the inverted half of its blink and all through its hold. The caller maps each to its cell
	/// and calls [`Cell::alarmed`](crate::Cell::alarmed).
	pub fn inverted(&self, now_ms: u64) -> Inverted {
		let mut out = Inverted::NONE;
		for (slot, mark) in out.0.iter_mut().zip(&self.marks) {
			*slot = mark.filter(|m| m.highlight.inverted(now_ms)).map(|m| m.channel);
		}
		out
	}

	/// The page and the channels pointed at — what [`Update::changed`] compares.
	fn picture(&self) -> (PageId, [Option<ChannelId>; MAX_VALUE_CELLS]) {
		(self.page, self.marks.map(|m| m.map(|m| m.channel)))
	}
}

/// The answer to one [`Alarms::poll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Update {
	pub shown: Shown,
	/// Did the page, or the cells it points at, just change? Not how those cells are
	/// drawn — the blink and the steady hold are [`Shown::inverted`]'s, per frame —
	/// and not the values, which move on their own. The first poll always reports
	/// `true`, because before it there was nothing on the glass.
	///
	/// Nothing outside the tests reads it: the board redraws every frame
	/// unconditionally, and [`Screen`](crate::screen::Screen) keeps its own
	/// `page_changed` for when the foreground channels move.
	pub changed: bool,
}

/// What a press did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Press {
	/// No alarm was showing, so the press means what it normally means.
	NextPage,
	/// An alarm was showing and this press ended every episode on the glass. The caller does
	/// not page: silencing *is* what the driver asked for.
	Silenced,
}

/// The rules, their state, and what they decided last.
///
/// `S` holds one [`Slot`] per rule — an array on the board, sized by the plan when the image is
/// built; a `Vec` on a laptop. Rules are in **priority order** — when two fire in the same poll
/// the earlier one takes the screen, and the tie is broken by the plan rather than by whichever
/// the loop happened to see first.
pub struct Alarms<'a, S> {
	rules: &'a [Alarm],
	/// The plan's pages: which channels the page on the glass shows ([`Page::shows`]).
	pages: &'a [Page],
	episodes: S,
	/// Which rule leads on the glass, from the last poll: the first showing one.
	showing: Option<usize>,
	/// The page the alarms hold, from the last poll. What a press acts on.
	page: Option<PageId>,
	/// The last decision, for `changed`.
	last: Option<Shown>,
}

impl<'a, S: AsRef<[Slot]> + AsMut<[Slot]>> Alarms<'a, S> {
	/// The plan's rules and pages, and one slot for each rule.
	///
	/// # Panics
	///
	/// When `slots` is not as long as `rules`: both are sized from the same plan, so a
	/// mismatch is an image built wrong, not a car behaving oddly.
	pub fn new(rules: &'a [Alarm], pages: &'a [Page], slots: S) -> Self {
		assert_eq!(slots.as_ref().len(), rules.len(), "one slot per alarm rule");
		Alarms {
			rules,
			pages,
			episodes: slots,
			showing: None,
			page: None,
			last: None,
		}
	}

	/// Which rule leads on the glass as of the last poll, by its place in the plan.
	pub fn showing(&self) -> Option<usize> {
		self.showing
	}

	/// The page the alarms hold as of the last poll.
	pub fn page(&self) -> Option<PageId> {
		self.page
	}

	/// The glass showed something else meanwhile (the adapter screen): the next poll
	/// counts whichever alarm is up as taking it anew, so its blink starts inverted
	/// rather than wherever the clock has reached since it last showed.
	pub fn glass_lost(&mut self) {
		self.showing = None;
		self.page = None;
	}

	/// Every channel any rule watches, silenced ones included.
	///
	/// A silenced rule is still polled, and that is the whole reason silence is
	/// bounded: the value going off cannot be seen if nobody asks. Duplicates are not
	/// removed here; there is nowhere to remember them without an allocator.
	pub fn watched(&self) -> impl Iterator<Item = ChannelId> + '_ {
		self.rules.iter().flat_map(|r| {
			// A drift rule needs both halves of its pair: the specified value is as much a
			// reading as the actual one, and a rule cannot see what nobody asks for.
			let specified = match r.rule {
				Rule::Bounds { .. } => None,
				Rule::Drift { specified, .. } => Some(specified),
			};
			core::iter::once(r.channel).chain(specified)
		})
	}

	/// A press — any command, whichever input gave it.
	///
	/// Every press is modal: while an alarm is showing it silences every episode on the
	/// glass's page and returns [`Press::Silenced`]; otherwise it means what it normally means
	/// and the caller carries it out. The caller does not need to ask which case it is first —
	/// that is exactly the question this answers, and asking separately is how the two get out
	/// of step.
	///
	/// A rule out on another page is not silenced: it takes the screen on the next poll and
	/// wants its own press, because it is a different thing to be told.
	pub fn press(&mut self) -> Press {
		self.showing = None;
		let Some(page) = self.page.take() else {
			return Press::NextPage;
		};
		let (rules, pages) = (self.rules, self.pages);
		let episodes = self.episodes.as_mut();
		for (rule, episode) in rules.iter().zip(episodes.iter_mut()) {
			if on(rule, page, pages) && episode.0.showing().is_some() {
				episode.0 = Episode::Silenced;
			}
		}
		Press::Silenced
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
		let (rules, pages) = (self.rules, self.pages);
		let episodes = self.episodes.as_mut();
		for (rule, episode) in rules.iter().zip(episodes.iter_mut()) {
			episode.0 = step(rule, episode.0, &value_of, now_ms);
		}

		let lead = episodes.iter().position(|e| e.0.showing().is_some());
		let held = lead.map(|i| rules[i].page);
		// A page taking the glass — from the driver's, or from another alarm's — has had its
		// rules firing behind it for however long: their blink starts now, inverted first, like
		// any takeover's.
		if let Some(at) = held.filter(|_| held != self.page) {
			for (rule, episode) in rules.iter().zip(episodes.iter_mut()) {
				if on(rule, at, pages)
					&& let Episode::Firing { blink_from_ms } = &mut episode.0
				{
					*blink_from_ms = now_ms;
				}
			}
		}
		self.showing = lead;
		self.page = held;

		let shown = match held {
			None => Shown::page(page),
			Some(at) => {
				let mut shown = Shown::page(at);
				let showing = rules.iter().zip(episodes.iter()).filter(|(rule, _)| on(rule, at, pages));
				for (rule, highlight) in showing.filter_map(|(rule, e)| Some((rule, e.0.showing()?))) {
					mark(&mut shown.marks, rule.channel, highlight);
				}
				shown
			}
		};

		// Which page, and which cells on it — not how they are drawn this moment.
		let changed = self.last.is_none_or(|last| last.picture() != shown.picture());
		self.last = Some(shown);
		Update { shown, changed }
	}
}

/// Whether `rule` is on page `at` of `pages`: the page it raises, or one that shows its channel
/// as well. A page the plan does not have shows only what raises it.
fn on(rule: &Alarm, at: PageId, pages: &[Page]) -> bool {
	rule.page == at || pages.get(usize::from(at.0)).is_some_and(|page| page.shows(rule.channel.0))
}

/// Mark `channel` on the glass, once: a channel with two rules showing — its bounds and its
/// drift — is one cell, blinking while either is out.
fn mark(marks: &mut [Option<Mark>], channel: ChannelId, highlight: Highlight) {
	for slot in marks.iter_mut() {
		match slot {
			Some(m) if m.channel == channel => {
				if let (Highlight::Steady, Highlight::Blinking { .. }) = (m.highlight, highlight) {
					m.highlight = highlight;
				}
				return;
			}
			Some(_) => {}
			None => {
				*slot = Some(Mark { channel, highlight });
				return;
			}
		}
	}
}

/// One rule, one poll. Pulled out of the loop because it is the state machine
/// and everything around it is bookkeeping.
fn step(rule: &Alarm, episode: Episode, value_of: &impl Fn(ChannelId) -> Option<f32>, now_ms: u64) -> Episode {
	let reading = rule.reading(value_of);
	let out = reading.is_some_and(|v| rule.trips(v));
	let off = reading.is_some_and(|v| rule.goes_off(v));
	// A rule with no hold fires the moment it trips; one with a hold starts counting.
	let start = || match rule.hold_ms() {
		0 => Episode::Firing { blink_from_ms: now_ms },
		_ => Episode::Rising { since_ms: now_ms },
	};
	match episode {
		Episode::Clear if out => start(),
		Episode::Clear => Episode::Clear,
		// Counting. The count is kept while the channel is still out; it came back, or stopped
		// answering, or the specified value fell under the floor — the count ends: a hold that
		// survived the gaps between what it was counting would not be a hold, and a transient
		// is what this state exists to swallow.
		Episode::Rising { since_ms } if out && now_ms.saturating_sub(since_ms) >= rule.hold_ms() => Episode::Firing { blink_from_ms: now_ms },
		Episode::Rising { since_ms } if out => Episode::Rising { since_ms },
		Episode::Rising { .. } => Episode::Clear,
		Episode::Firing { blink_from_ms } if out => Episode::Firing { blink_from_ms },
		Episode::Firing { .. } if off => Episode::Holding {
			until_ms: now_ms.saturating_add(HOLD_MS),
		},
		// Inside the bound but not at its off value, or nothing answered. Neither of those is
		// news, so the screen does not move.
		Episode::Firing { blink_from_ms } => Episode::Firing { blink_from_ms },
		// Out again before the hold expired: the same episode continues, so it does not
		// re-announce itself and the driver's silence still applies. A rule with a hold does not
		// have to earn it a second time inside one episode. The blink starts over here, inverted
		// first, as it does at a takeover.
		Episode::Holding { .. } if out => Episode::Firing { blink_from_ms: now_ms },
		Episode::Holding { until_ms } if now_ms >= until_ms => Episode::Clear,
		Episode::Holding { until_ms } => Episode::Holding { until_ms },
		// Silence ends when the value goes off and not before — including when the channel
		// stops answering, which is why silence is bounded by evidence rather than by a timer.
		// A unit that has dropped off the bus stays silenced, and that is the right answer: it
		// is not saying the engine is fine.
		Episode::Silenced if off => Episode::Clear,
		Episode::Silenced => Episode::Silenced,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::vec::Vec;

	/// Four channels with a lower bound each, all on one page, and a fifth with an upper bound
	/// on another. Neutral numbers throughout — no channel, identifier or threshold of any car.
	const GROUP: [ChannelId; 4] = [ChannelId(0), ChannelId(1), ChannelId(2), ChannelId(3)];
	const SINGLE: ChannelId = ChannelId(4);

	const GROUP_PAGE: PageId = PageId(2);
	const SINGLE_PAGE: PageId = PageId(3);
	/// Where the driver was before any of this happened.
	const WAS_SHOWING: PageId = PageId(1);
	/// A reading well clear of the group's off value.
	const CLEAR: f32 = 20.0;

	/// Fires at or below 0, off at or above 5.
	fn low(channel: ChannelId) -> Alarm {
		Alarm::bounds(channel, GROUP_PAGE, None, Some(Bound { at: 0.0, off: 5.0 }))
	}

	/// The group's four rules, then the single one: fires at or above 10, off at or under 8.
	fn rules() -> Vec<Alarm> {
		let mut rules: Vec<Alarm> = GROUP.iter().map(|c| low(*c)).collect();
		rules.push(Alarm::bounds(SINGLE, SINGLE_PAGE, Some(Bound { at: 10.0, off: 8.0 }), None));
		rules
	}

	fn leak(rules: Vec<Alarm>) -> &'static [Alarm] {
		std::vec::Vec::leak(rules)
	}

	/// The pages the fixtures' rules raise: each shows its rules' channels and nothing else.
	static PAGES: [Page; 5] = [
		Page::Values {
			title: "",
			cells: &[],
			volts: None,
		},
		Page::Values {
			title: "",
			cells: &[],
			volts: None,
		},
		Page::Values {
			title: "",
			cells: &[0, 1, 2, 3],
			volts: None,
		},
		Page::Values {
			title: "",
			cells: &[4],
			volts: None,
		},
		Page::Values {
			title: "",
			cells: &[10, 11],
			volts: None,
		},
	];

	/// A machine over `rules`.
	fn alarms(rules: &'static [Alarm]) -> Alarms<'static, Vec<Slot>> {
		Alarms::new(rules, &PAGES, std::vec![Slot::CLEAR; rules.len()])
	}

	/// The group's rules alone.
	fn group() -> Alarms<'static, Vec<Slot>> {
		alarms(leak(GROUP.iter().map(|c| low(*c)).collect()))
	}

	/// Every rule.
	fn all() -> Alarms<'static, Vec<Slot>> {
		alarms(leak(rules()))
	}

	fn marked(shown: &Shown) -> Vec<ChannelId> {
		shown.marks.iter().flatten().map(|m| m.channel).collect()
	}

	// --- drift: a channel against what its unit asked for ------------------------------

	/// Two channels, each paired with the specified value the plan resolved for it.
	const DRIFTING: [ChannelId; 2] = [ChannelId(10), ChannelId(11)];
	const SPECIFIED: [ChannelId; 2] = [ChannelId(20), ChannelId(21)];
	const DRIFT_PAGE: PageId = PageId(4);
	/// Over 10 % for a second fires it; at or under 6 % it goes off; a specified value under
	/// 0.5 is not worth a percentage.
	fn drift_of(i: usize) -> Alarm {
		Alarm::drift(DRIFTING[i], SPECIFIED[i], DRIFT_PAGE, 10.0, 6.0, 1_000, 0.5)
	}

	fn drift() -> Alarms<'static, Vec<Slot>> {
		alarms(leak(std::vec![drift_of(0), drift_of(1)]))
	}

	/// One drifting channel and its specified value; the second pair says nothing.
	fn pair(actual: Option<f32>, specified: Option<f32>) -> [Reading; 2] {
		[Reading::new(DRIFTING[0], actual), Reading::new(SPECIFIED[0], specified)]
	}

	/// Both pairs, each against a specified value of 2.0.
	fn both(a: f32, b: f32) -> [Reading; 4] {
		[
			Reading::new(DRIFTING[0], Some(a)),
			Reading::new(SPECIFIED[0], Some(2.0)),
			Reading::new(DRIFTING[1], Some(b)),
			Reading::new(SPECIFIED[1], Some(2.0)),
		]
	}

	#[test]
	fn a_drift_rule_is_polled_with_both_halves_of_its_pair() {
		let alarms = drift();
		let watched: Vec<ChannelId> = alarms.watched().collect();
		assert_eq!(watched, [DRIFTING[0], SPECIFIED[0], DRIFTING[1], SPECIFIED[1]]);
	}

	#[test]
	fn a_drift_rule_fires_only_once_it_has_held() {
		let mut alarms = drift();
		// 2.2 against 2.0 is 10 %, which trips — but not yet.
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 0).shown, Shown::page(WAS_SHOWING));
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 999).shown, Shown::page(WAS_SHOWING));
		let up = alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_000);
		assert_eq!(up.shown.page, DRIFT_PAGE, "a second of drift is the rule's own condition");
		assert_eq!(up.shown.offending(), Some(DRIFTING[0]));
	}

	#[test]
	fn a_transient_never_fires_and_starts_the_count_over() {
		let mut alarms = drift();
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
		let mut alarms = drift();
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
		assert_eq!(marked(&up.shown), [DRIFTING[1]]);
	}

	#[test]
	fn two_channels_out_on_one_page_both_fire_and_both_are_marked() {
		// Both are out the whole second; which one is worse changes every poll. Each rule counts
		// its own drift.
		let mut alarms = drift();
		let mut fired = None;
		for step in 0..=20u64 {
			// 12 % and 13 % out, swapping places each poll.
			let (a, b) = if step % 2 == 0 { (2.26, 2.24) } else { (2.24, 2.26) };
			let up = alarms.poll(WAS_SHOWING, &both(a, b), step * 100);
			if up.shown.page == DRIFT_PAGE && fired.is_none() {
				fired = Some(step * 100);
				assert_eq!(marked(&up.shown), [DRIFTING[0], DRIFTING[1]]);
			}
		}
		assert_eq!(fired, Some(1_000), "it fires on the hold");
	}

	#[test]
	fn a_hold_does_not_survive_a_gap_in_the_evidence() {
		let mut alarms = drift();
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

		// The same for a value back inside but not yet off meanwhile, and for a pair that stops
		// answering.
		for quiet in [pair(Some(2.16), Some(2.0)), pair(None, None)] {
			let mut alarms = drift();
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
		let mut alarms = drift();
		// 0.4 asked for, 0.1 delivered: 75 % out, and meaningless.
		for now in [0, 1_000, 5_000] {
			assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(0.1), Some(0.4)), now).shown, Shown::page(WAS_SHOWING));
		}
	}

	#[test]
	fn a_pair_that_has_not_answered_says_nothing() {
		let mut alarms = drift();
		for readings in [pair(None, Some(2.0)), pair(Some(2.2), None), pair(None, None)] {
			assert_eq!(alarms.poll(WAS_SHOWING, &readings, 5_000).shown, Shown::page(WAS_SHOWING));
		}
	}

	#[test]
	fn a_drift_rule_goes_off_at_its_off_share_and_holds_the_view_first() {
		let mut alarms = drift();
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 0);
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_000).shown.page, DRIFT_PAGE);
		// 2.1 against 2.0 is 5 %, under the off share.
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
	fn a_press_silences_a_drift_episode_until_it_goes_off() {
		let mut alarms = drift();
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 0);
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_000);
		assert_eq!(alarms.press(), Press::Silenced);
		assert_eq!(
			alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 2_000).shown,
			Shown::page(WAS_SHOWING)
		);
		// Off, so the rule is armed again — and has to hold once more.
		alarms.poll(WAS_SHOWING, &pair(Some(2.0), Some(2.0)), 3_000);
		alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 4_000);
		assert_eq!(alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 5_000).shown.page, DRIFT_PAGE);
	}

	// --- bounds -------------------------------------------------------------------------

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
	fn an_off_value_takes_the_screen_once_where_one_threshold_would_take_it_ten_times() {
		// A value sitting exactly on the bound and breathing either side of it, never at the
		// off value. With a single threshold this is a screen swapping at the poll rate, which
		// is the failure the whole rule exists to avoid.
		let mut alarms = group();
		let series = [0.0, 1.0, -1.0, 0.5, 0.0, 4.0, -4.0, 3.0];
		let mut takeovers = 0;
		for (tick, v) in series.iter().enumerate() {
			let update = alarms.poll(WAS_SHOWING, &only(*v), tick as u64 * 100);
			if update.changed && update.shown.page == GROUP_PAGE {
				takeovers += 1;
			}
			assert_eq!(update.shown.page, GROUP_PAGE, "the episode never ends short of the off value");
		}
		assert_eq!(takeovers, 1, "one episode, not eight");
	}

	#[test]
	fn it_goes_off_at_the_off_value_and_not_at_the_bound() {
		let mut alarms = group();
		assert_eq!(alarms.poll(WAS_SHOWING, &only(0.0), 0).shown.page, GROUP_PAGE);
		// Back above the bound, short of the off value: not off, so the hold has not even
		// started and the view stays up however long we wait.
		assert_eq!(alarms.poll(WAS_SHOWING, &only(1.0), 1_000).shown.page, GROUP_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(4.9), 100_000).shown.page, GROUP_PAGE);
		// At the off value the hold starts, and only then does it run out.
		assert_eq!(alarms.poll(WAS_SHOWING, &only(5.0), 100_000).shown.page, GROUP_PAGE);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(5.0), 100_000 + HOLD_MS).shown.page, WAS_SHOWING);
	}

	#[test]
	fn a_channel_with_both_bounds_fires_past_either_and_goes_off_between_the_off_values() {
		// Out above 10, off at or under 8; out below 0, off at or above 2.
		static BOTH: [Alarm; 1] = [Alarm {
			channel: ChannelId(4),
			page: SINGLE_PAGE,
			rule: Rule::Bounds {
				max: Some(Bound { at: 10.0, off: 8.0 }),
				min: Some(Bound { at: 0.0, off: 2.0 }),
			},
		}];
		let at = |v: f32| [Reading::new(ChannelId(4), Some(v))];
		for (out, between) in [(10.0, 9.0), (-1.0, 1.0)] {
			let mut alarms = alarms(&BOTH);
			assert_eq!(alarms.poll(WAS_SHOWING, &at(5.0), 0).shown.page, WAS_SHOWING);
			assert_eq!(alarms.poll(WAS_SHOWING, &at(out), 100).shown.page, SINGLE_PAGE, "out at {out}");
			assert_eq!(
				alarms.poll(WAS_SHOWING, &at(between), 100_000).shown.highlight(),
				Some(Highlight::Blinking { from_ms: 100 }),
				"{between} is inside the bound but not at its off value"
			);
			assert_eq!(alarms.poll(WAS_SHOWING, &at(5.0), 200_000).shown.highlight(), Some(Highlight::Steady));
			assert_eq!(alarms.poll(WAS_SHOWING, &at(5.0), 200_000 + HOLD_MS).shown.page, WAS_SHOWING);
		}
		// From one bound straight past the other is the same episode.
		let mut alarms = alarms(&BOTH);
		alarms.poll(WAS_SHOWING, &at(12.0), 0);
		let across = alarms.poll(WAS_SHOWING, &at(-3.0), 100);
		assert_eq!((across.shown.page, across.changed), (SINGLE_PAGE, false));
	}

	#[test]
	fn the_view_stays_up_for_the_hold_and_then_hands_back_to_where_you_were() {
		let mut alarms = group();
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		let off_at = 500;
		assert_eq!(alarms.poll(WAS_SHOWING, &only(CLEAR), off_at).shown.page, GROUP_PAGE);
		// One millisecond short of the hold, it is still the alarm's screen.
		let almost = alarms.poll(WAS_SHOWING, &only(CLEAR), off_at + HOLD_MS - 1);
		assert_eq!(almost.shown.page, GROUP_PAGE);
		assert!(!almost.changed);
		// And at the hold it hands back — to the page that was showing, by identity.
		let back = alarms.poll(WAS_SHOWING, &only(CLEAR), off_at + HOLD_MS);
		assert_eq!(back.shown, Shown::page(WAS_SHOWING));
		assert!(back.changed, "the glass has to be redrawn");
	}

	#[test]
	fn it_hands_back_to_the_page_that_was_showing_and_never_to_page_one() {
		let mut alarms = group();
		// What comes back is whatever the caller says is current, which is the only definition
		// of "where you were" that survives the driver having paged around.
		let elsewhere = PageId(5);
		alarms.poll(elsewhere, &only(-10.0), 0);
		alarms.poll(elsewhere, &only(CLEAR), 10);
		assert_eq!(alarms.poll(elsewhere, &only(CLEAR), 10 + HOLD_MS).shown.page, elsewhere);
	}

	#[test]
	fn a_press_silences_the_episode_and_the_screen_goes_back_at_once() {
		let mut alarms = group();
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-10.0), 0).shown.page, GROUP_PAGE);
		assert_eq!(alarms.showing(), Some(1));
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
		let mut alarms = group();
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		alarms.press();
		for tick in 1..200u64 {
			let update = alarms.poll(WAS_SHOWING, &only(-10.0 - tick as f32), tick * 100);
			assert_eq!(update.shown, Shown::page(WAS_SHOWING), "silenced at t={}", tick * 100);
		}
	}

	#[test]
	fn a_fresh_crossing_after_it_goes_off_arms_it_again() {
		let mut alarms = group();
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		alarms.press();
		// Silence is not deafness: the value is still watched, and going off re-arms.
		assert_eq!(alarms.poll(WAS_SHOWING, &only(CLEAR), 1_000).shown.page, WAS_SHOWING);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(-5.0), 2_000).shown.page, GROUP_PAGE);
	}

	#[test]
	fn two_alarms_crossing_in_the_same_poll_resolve_by_priority_not_by_luck() {
		let mut alarms = all();
		let mut readings = only(-20.0).to_vec();
		readings.push(Reading::new(SINGLE, Some(12.0)));
		let first = alarms.poll(WAS_SHOWING, &readings, 0);
		assert_eq!(first.shown.page, GROUP_PAGE, "the plan lists the rules in priority order");
		assert_eq!(marked(&first.shown), [GROUP[1]], "the other page's channel is not drawn here");
		// And it is stable: the same input a second later does not swap them.
		assert!(!alarms.poll(WAS_SHOWING, &readings, 100).changed);
	}

	#[test]
	fn a_lower_rule_out_first_gives_the_glass_to_a_higher_one_out_later() {
		let mut alarms = all();
		let calm = only(CLEAR);
		let mut single_out = calm.to_vec();
		single_out.push(Reading::new(SINGLE, Some(12.0)));
		assert_eq!(alarms.poll(WAS_SHOWING, &single_out, 0).shown.page, SINGLE_PAGE);
		let mut both_out = only(-1.0).to_vec();
		both_out.push(Reading::new(SINGLE, Some(12.0)));
		let took = alarms.poll(WAS_SHOWING, &both_out, 100);
		assert_eq!((took.shown.page, took.changed), (GROUP_PAGE, true), "priority, not who was first");
	}

	#[test]
	fn silencing_the_glass_lets_the_page_behind_it_through() {
		let mut alarms = all();
		let mut readings = cells([-20.0, -3.0, CLEAR, CLEAR]).to_vec();
		readings.push(Reading::new(SINGLE, Some(12.0)));
		let up = alarms.poll(WAS_SHOWING, &readings, 0);
		assert_eq!(marked(&up.shown), [GROUP[0], GROUP[1]]);
		// One press silences both cells the driver sees...
		assert_eq!(alarms.press(), Press::Silenced);
		// ...and not the other page's alarm: it is a different thing to say.
		assert_eq!(alarms.poll(WAS_SHOWING, &readings, 100).shown.page, SINGLE_PAGE);
		assert_eq!(alarms.showing(), Some(4));
		assert_eq!(alarms.press(), Press::Silenced);
		assert_eq!(alarms.poll(WAS_SHOWING, &readings, 200).shown, Shown::page(WAS_SHOWING));
	}

	#[test]
	fn every_channel_out_on_the_page_is_marked_in_priority_order_and_each_ends_on_its_own() {
		let mut alarms = group();
		let update = alarms.poll(WAS_SHOWING, &cells([-1.0, 16.0, -19.0, -2.0]), 0);
		assert_eq!(update.shown.page, GROUP_PAGE);
		assert_eq!(marked(&update.shown), [GROUP[0], GROUP[2], GROUP[3]]);
		assert_eq!(update.shown.offending(), Some(GROUP[0]), "the leading rule's");
		// A cell going out on the page already up is a different picture, not a takeover.
		let update = alarms.poll(WAS_SHOWING, &cells([-1.0, -5.0, -19.0, -2.0]), 100);
		assert_eq!(marked(&update.shown), [GROUP[0], GROUP[1], GROUP[2], GROUP[3]]);
		assert!(update.changed);
		// The first one off holds steady, the others still blink.
		let update = alarms.poll(WAS_SHOWING, &cells([CLEAR, -5.0, -19.0, -2.0]), 200);
		assert_eq!(update.shown.marks[0].map(|m| m.highlight), Some(Highlight::Steady));
		assert!(
			update.shown.marks[1..]
				.iter()
				.flatten()
				.all(|m| matches!(m.highlight, Highlight::Blinking { .. }))
		);
		// Its hold over, it drops off the page while the rest keep it up.
		let update = alarms.poll(WAS_SHOWING, &cells([CLEAR, -5.0, -19.0, -2.0]), 200 + HOLD_MS);
		assert_eq!(marked(&update.shown), [GROUP[1], GROUP[2], GROUP[3]]);
		assert_eq!((update.shown.page, alarms.showing()), (GROUP_PAGE, Some(1)));
	}

	#[test]
	fn a_channel_with_two_rules_is_one_mark_blinking_while_either_is_out() {
		// Channel 4's bounds and its drift, both raising one page.
		static TWO: [Alarm; 2] = [
			Alarm {
				channel: ChannelId(4),
				page: SINGLE_PAGE,
				rule: Rule::Bounds {
					max: Some(Bound { at: 10.0, off: 8.0 }),
					min: None,
				},
			},
			Alarm {
				channel: ChannelId(4),
				page: SINGLE_PAGE,
				rule: Rule::Drift {
					specified: ChannelId(5),
					percent: 10.0,
					off_percent: 5.0,
					hold_ms: 0,
					min_setpoint: 0.5,
				},
			},
		];
		let mut alarms = alarms(&TWO);
		let at = |v: f32, s: f32| [Reading::new(ChannelId(4), Some(v)), Reading::new(ChannelId(5), Some(s))];
		let up = alarms.poll(WAS_SHOWING, &at(11.0, 11.0), 0);
		assert_eq!(marked(&up.shown), [ChannelId(4)], "past its bound only");
		// Back under the off value (steady), and 50 % from its setpoint (out).
		let up = alarms.poll(WAS_SHOWING, &at(6.0, 4.0), 100);
		assert_eq!(marked(&up.shown), [ChannelId(4)]);
		assert!(matches!(up.shown.highlight(), Some(Highlight::Blinking { .. })));
	}

	// --- the highlight: blinking while out, steady in the hold -------------------------

	fn blinking(highlight: Option<Highlight>) -> bool {
		matches!(highlight, Some(Highlight::Blinking { .. }))
	}

	#[test]
	fn while_out_the_offending_cell_blinks_from_the_takeover() {
		let mut alarms = group();
		alarms.poll(WAS_SHOWING, &only(CLEAR), 0);
		// Out from 700 ms, which is no boundary of the clock's: the blink starts there.
		// Frames 200 ms apart, the board's period: two inverted, two plain.
		let took = 700;
		let mut seen = Vec::new();
		for t in (took..=took + 1_400).step_by(200) {
			let shown = alarms.poll(WAS_SHOWING, &only(-10.0), t).shown;
			assert!(blinking(shown.highlight()), "out at t={t}");
			assert_eq!(shown.offending(), Some(GROUP[1]), "what is pointed at does not blink");
			seen.push(shown.inverted(t).contains(GROUP[1]));
		}
		assert_eq!(seen, [true, true, false, false, true, true, false, false]);
		// The halves are BLINK_MS from the takeover, to the millisecond.
		let shown = alarms.poll(WAS_SHOWING, &only(-10.0), took + 1_600).shown;
		assert!(shown.inverted(took + BLINK_MS - 1).contains(GROUP[1]));
		assert!(shown.inverted(took + BLINK_MS).is_empty());
		assert!(shown.inverted(took + BLINK_MS * 2 - 1).is_empty());
		assert!(shown.inverted(took + BLINK_MS * 2).contains(GROUP[1]));
	}

	#[test]
	fn the_first_frame_of_a_takeover_is_inverted_whenever_it_comes() {
		// Nothing else on the glass says "this one" when the driver is already on the alarm's
		// page: the first frame has to.
		for took in [1, 250, 399, 400, 401, 777, 1_234, 10_001] {
			let mut alarms = group();
			alarms.poll(WAS_SHOWING, &only(CLEAR), 0);
			let shown = alarms.poll(WAS_SHOWING, &only(-10.0), took).shown;
			assert!(shown.inverted(took).contains(GROUP[1]), "took at {took}");
		}
	}

	#[test]
	fn a_page_taking_the_glass_starts_every_cell_on_it_inverted() {
		// The group's channel 1 has been firing behind the single page; once the single one is
		// silenced the group's page takes the glass, and its cell blinks from that moment.
		let mut alarms = all();
		let mut readings = only(CLEAR).to_vec();
		readings.push(Reading::new(SINGLE, Some(12.0)));
		alarms.poll(WAS_SHOWING, &readings, 0);
		let mut readings = only(-10.0).to_vec();
		readings.push(Reading::new(SINGLE, Some(12.0)));
		// Out from 100, behind nothing: the group outranks, so it takes the glass at once.
		let took = alarms.poll(WAS_SHOWING, &readings, 100).shown;
		assert_eq!(took.page, GROUP_PAGE);
		assert!(took.inverted(100).contains(GROUP[1]));
		// Silenced; the single page takes the glass at 500, a plain half counted from when it
		// fired (500 / 400 is odd), and its first frame is inverted all the same.
		alarms.press();
		let back = alarms.poll(WAS_SHOWING, &readings, 500).shown;
		assert_eq!(back.page, SINGLE_PAGE);
		assert!(back.inverted(500).contains(SINGLE), "from its own takeover, not from 0");
	}

	#[test]
	fn the_cell_keeps_blinking_short_of_the_off_value_and_while_its_channel_is_quiet() {
		// Neither ends it, so the episode is still `Firing`, and its phase runs on.
		let mut alarms = group();
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		// Back above the bound, short of the off value.
		let band = alarms.poll(WAS_SHOWING, &only(1.0), BLINK_MS).shown;
		assert!(blinking(band.highlight()));
		assert!(band.inverted(BLINK_MS).is_empty(), "the phase from the takeover, not restarted");
		// Nothing answering at all.
		let quiet = alarms.poll(WAS_SHOWING, &[], BLINK_MS * 2).shown;
		assert!(blinking(quiet.highlight()));
		assert!(quiet.inverted(BLINK_MS * 2).contains(GROUP[1]));
	}

	#[test]
	fn through_the_hold_the_cell_is_steady_and_after_the_hand_back_nothing_is() {
		let mut alarms = group();
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		let off_at = 500;
		let off = alarms.poll(WAS_SHOWING, &only(CLEAR), off_at);
		assert_eq!(off.shown.highlight(), Some(Highlight::Steady));
		assert!(!off.changed, "the same page and the same cell");
		// Every frame of the hold, whichever half of a blink the clock is in.
		for t in (off_at..off_at + HOLD_MS).step_by(100) {
			let shown = alarms.poll(WAS_SHOWING, &only(CLEAR), t).shown;
			assert_eq!(shown.highlight(), Some(Highlight::Steady), "holding at t={t}");
			assert!(shown.inverted(t).contains(GROUP[1]), "inverted at t={t}");
		}
		let back = alarms.poll(WAS_SHOWING, &only(CLEAR), off_at + HOLD_MS).shown;
		assert_eq!(back, Shown::page(WAS_SHOWING));
		assert!(back.inverted(off_at + HOLD_MS).is_empty());
	}

	#[test]
	fn out_again_inside_the_hold_blinks_again_from_that_moment() {
		let mut alarms = group();
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		assert_eq!(alarms.poll(WAS_SHOWING, &only(CLEAR), 100).shown.highlight(), Some(Highlight::Steady));
		let again_at = 1_300;
		let again = alarms.poll(WAS_SHOWING, &only(-10.0), again_at).shown;
		assert!(blinking(again.highlight()));
		assert!(again.inverted(again_at).contains(GROUP[1]), "inverted first, as at any takeover");
		assert!(again.inverted(again_at + BLINK_MS).is_empty());
	}

	#[test]
	fn a_silenced_episode_highlights_nothing() {
		let mut alarms = group();
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		alarms.press();
		for t in (100..=2_000).step_by(100) {
			let shown = alarms.poll(WAS_SHOWING, &only(-10.0), t).shown;
			assert_eq!((shown.offending(), shown.highlight()), (None, None), "t={t}");
			assert!(shown.inverted(t).is_empty());
		}
	}

	#[test]
	fn a_drift_rule_counting_its_hold_highlights_nothing_and_blinks_once_it_fires() {
		let mut alarms = drift();
		let counting = alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 200).shown;
		assert_eq!(counting.highlight(), None);
		assert!(counting.inverted(200).is_empty());
		// A second after it started counting: the blink starts when it fires, not before.
		let fired = alarms.poll(WAS_SHOWING, &pair(Some(2.2), Some(2.0)), 1_200).shown;
		assert!(blinking(fired.highlight()));
		assert!(fired.inverted(1_200).contains(DRIFTING[0]));
	}

	#[test]
	fn an_alarm_can_fire_above_a_bound_as_well_as_below_one() {
		let mut alarms = all();
		let at = |v: f32| [Reading::new(SINGLE, Some(v))];
		assert_eq!(alarms.poll(WAS_SHOWING, &at(10.0), 0).shown.page, SINGLE_PAGE, "at the bound");
		assert_eq!(alarms.poll(WAS_SHOWING, &at(9.0), 100).shown.highlight().map(blinking_one), Some(true));
		assert_eq!(
			alarms.poll(WAS_SHOWING, &at(8.0), 200).shown.highlight(),
			Some(Highlight::Steady),
			"at the off value"
		);
		assert_eq!(alarms.poll(WAS_SHOWING, &at(8.0), 200 + HOLD_MS).shown.page, WAS_SHOWING);
	}

	fn blinking_one(h: Highlight) -> bool {
		matches!(h, Highlight::Blinking { .. })
	}

	#[test]
	fn a_channel_that_stops_answering_neither_trips_nor_goes_off() {
		let mut alarms = group();
		let silence = [Reading::new(GROUP[0], None)];
		// Nothing on the bus is not a reading of zero, so it cannot fire...
		assert_eq!(alarms.poll(WAS_SHOWING, &silence, 0).shown, Shown::page(WAS_SHOWING));
		// ...and it cannot end an episode either: a unit that drops out mid-alarm
		// has not said the value is fine. A press is the way out.
		alarms.poll(WAS_SHOWING, &only(-10.0), 100);
		assert_eq!(alarms.poll(WAS_SHOWING, &silence, 100 + HOLD_MS * 4).shown.page, GROUP_PAGE);
		alarms.press();
		assert_eq!(alarms.poll(WAS_SHOWING, &silence, 100 + HOLD_MS * 5).shown, Shown::page(WAS_SHOWING));
	}

	#[test]
	fn a_press_with_no_alarm_showing_is_a_page_turn() {
		let mut alarms = group();
		alarms.poll(WAS_SHOWING, &only(CLEAR), 0);
		assert_eq!(alarms.press(), Press::NextPage);
	}

	#[test]
	fn nothing_moving_reports_nothing_changed_after_the_first_look() {
		let mut alarms = group();
		// The first poll always changes: there was nothing on the glass before it.
		assert!(alarms.poll(WAS_SHOWING, &only(CLEAR), 0).changed);
		assert!(!alarms.poll(WAS_SHOWING, &only(CLEAR), 100).changed);
		assert!(!alarms.poll(WAS_SHOWING, &only(11.0), 200).changed);
		// Paging is the caller's business, and it is still a change of screen.
		assert!(alarms.poll(PageId(4), &only(CLEAR), 300).changed);
	}

	#[test]
	fn every_rules_channel_is_watched_even_while_it_is_silenced() {
		let mut alarms = all();
		alarms.poll(WAS_SHOWING, &only(-10.0), 0);
		alarms.press();
		let watched: Vec<ChannelId> = alarms.watched().collect();
		assert_eq!(
			watched,
			[GROUP[0], GROUP[1], GROUP[2], GROUP[3], SINGLE],
			"a silenced rule is still polled — going off is what re-arms it"
		);
	}

	#[test]
	#[should_panic(expected = "one slot per alarm rule")]
	fn the_slots_are_sized_for_the_rules() {
		let _ = Alarms::new(leak(rules()), &PAGES, std::vec![Slot::CLEAR; 2]);
	}

	/// A channel out on the page another alarm raised is on the glass: it is marked there, and
	/// the press that silences the page silences it too (review, 2026-10-19). Here coolant is on
	/// the driver's page 0 and on page 2; oil only on page 2, and oil comes first.
	#[test]
	fn a_channel_out_on_the_page_up_is_marked_there_even_when_its_own_alarm_raises_another() {
		static BOTH: [Page; 3] = [
			Page::Values {
				title: "",
				cells: &[0],
				volts: None,
			},
			Page::Values {
				title: "",
				cells: &[],
				volts: None,
			},
			Page::Values {
				title: "",
				cells: &[1, 0],
				volts: None,
			},
		];
		const HOT: Rule = Rule::Bounds {
			max: Some(Bound { at: 10.0, off: 8.0 }),
			min: None,
		};
		static RULES: [Alarm; 2] = [
			Alarm {
				channel: ChannelId(1),
				page: PageId(2),
				rule: HOT,
			},
			Alarm {
				channel: ChannelId(0),
				page: PageId(0),
				rule: HOT,
			},
		];
		let mut alarms = Alarms::new(&RULES, &BOTH, std::vec![Slot::CLEAR; 2]);
		let out = [Reading::new(ChannelId(0), Some(12.0)), Reading::new(ChannelId(1), Some(12.0))];
		let up = alarms.poll(PageId(1), &out, 0).shown;
		assert_eq!(up.page, PageId(2));
		assert_eq!(marked(&up), [ChannelId(1), ChannelId(0)], "coolant is on the glass too");
		assert!(up.inverted(0).contains(ChannelId(0)));
		// One press for both: coolant does not come back on its own page for a second one.
		assert_eq!(alarms.press(), Press::Silenced);
		assert_eq!(alarms.poll(PageId(1), &out, 100).shown, Shown::page(PageId(1)));
		// Without the other alarm up, coolant raises its own page, as ever.
		let mut alarms = Alarms::new(&RULES, &BOTH, std::vec![Slot::CLEAR; 2]);
		let coolant = [Reading::new(ChannelId(0), Some(12.0)), Reading::new(ChannelId(1), Some(0.0))];
		assert_eq!(alarms.poll(PageId(1), &coolant, 0).shown.page, PageId(0));
	}
}
