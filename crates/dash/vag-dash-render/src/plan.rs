//! The plan: everything the device knows about the car, resolved on the laptop.
//!
//! `todo/dash/01-plan-format.md` decides the shape and this module is that
//! shape, in the form the firmware can hold — `&'static` everything, no
//! allocation, no lookup. The generator (`vag_cli_core::dash`) writes a
//! `static PLAN: Plan = Plan { … }` from the catalogs, the firmware
//! `include!`s it, and at run time does exactly one thing per channel: send
//! `0x22`, take the bits, multiply, draw. Nothing here resolves anything.
//!
//! **The board indexes; it never searches** (`todo/dash/27`, archived). Every join the board
//! needs is resolved by the generator and written as an index: a page points at its channels by
//! index into [`Plan::channels`], a channel at its unit by index into [`Plan::units`], a page
//! names the channel its strip's voltage is read from ([`Page::volts`]), and a chart page the
//! slot of its history ([`Page::Chart`]'s `slot`). [`Plan::joins_hold`] checks every one of
//! them when the image is built.
//!
//! Why this lives in the renderer crate and not the firmware: the firmware is
//! not a workspace member and cannot be built for the host, so anything only
//! there is untested by CI. [`Channel::decode`] is the one piece of arithmetic
//! between the bus and the glass, and the little-endian row that motivates its
//! byte-order flag (`0x380A`, 690 /min read as 45570 by a reader that assumed
//! big-endian) is exactly the bug a host test catches for free.

use crate::alarm::{Alarm, ChannelId, Rule};
use crate::control::{Command, MAX_BUTTONS, is_button_pin};
use crate::frame::{Strip, Volts};
use crate::stalk::{Read, StateIndex, States};

/// The whole interface between the laptop and the device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
	/// Baked so the firmware can check where it is. A plan built for one car
	/// asked of another answers plausibly and wrongly; `05` refuses to poll on a
	/// mismatch.
	pub vin: &'static str,
	/// `"en"` or `"ru"` — the board's own words, and the language every [`Channel::label`] was
	/// looked up in: `dash.toml`'s `language`, else `config.toml`'s.
	pub language: &'static str,
	/// The control units the plan reads, in the order they are polled.
	pub units: &'static [Unit],
	/// Every value the plan can put on screen. Pages and the device's own
	/// configuration refer to these by index.
	pub channels: &'static [Channel],
	pub pages: &'static [Page],
	/// The alarms the owner's `[[channel]]`s carry — `alarm_max`, `alarm_min`, `drift_*` — one
	/// rule per channel and kind, in priority order: the channels' order in `dash.toml`, a
	/// channel's bounds before its drift. A [`ChannelId`] is an index into [`Plan::channels`]
	/// and a [`PageId`](crate::alarm::PageId) an index into [`Plan::pages`]: an image is built
	/// for one plan, so an index is a name that cannot drift. The generator has checked each
	/// rule — its page is the first values page that shows the channel, else its chart page, and
	/// its off values are on the inside of its bounds; no count caps them.
	pub alarms: &'static [Alarm],
	/// The cruise lever as buttons (`todo/dash/19`), where the owner's `[stalk]` names one.
	pub stalk: Option<StalkPlan>,
	/// The stopwatch page, where the owner's `[stopwatch]` names a speed channel.
	pub stopwatch: Option<StopwatchPlan>,
	/// The owner's `[[button]]`s, in the file's order: at most
	/// [`MAX_BUTTONS`](crate::control::MAX_BUTTONS), each on a pin of its own.
	pub buttons: &'static [ButtonPlan],
	/// Where the plan learns that the car stands ([`Plan::stands`]), if anywhere: what a
	/// `zero_at_rest` channel asks. `None` where the plan has no source, and then no channel is
	/// `zero_at_rest` — the generator refuses one.
	pub standstill: Option<Standstill>,
}

/// Where "the car stands" comes from: one fact of the plan, decided once per frame
/// ([`Plan::stands`]), not a feature of the stopwatch (`todo/dash/26`, archived).
///
/// One kind today. A later one — a unit's own standstill verdict, an enumerated state such as
/// the parking aid's — is another variant, and needs the car first: whether it answers, and how
/// long after the wheels stop it changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Standstill {
	/// A speed channel, by plan channel index: the car stands while it reads exactly 0
	/// ([`stands_at`]) — the stopwatch's own test, on the stopwatch's own speed today.
	Speed(u16),
}

/// Whether a speed reading says the car stands: exactly 0. The stopwatch arms on it
/// ([`Stopwatch::sample`](crate::stopwatch::Stopwatch::sample)) and [`Standstill::Speed`] decides
/// by it, so the two never disagree about one reading. Exact, because a speed channel's zero is
/// a raw zero — the generator refuses a stopwatch speed with an offset in its scaling.
pub fn stands_at(speed: f32) -> bool {
	speed == 0.0
}

impl Standstill {
	/// Whether the car stands now, `value_of` giving a channel's current reading: `None` where
	/// the source has no current answer — no answer is no claim either way.
	pub fn stands(&self, value_of: impl Fn(u16) -> Option<f32>) -> Option<bool> {
		match *self {
			Standstill::Speed(channel) => value_of(channel).map(stands_at),
		}
	}

	/// The channel the fact is read from.
	pub const fn channel(&self) -> u16 {
		match *self {
			Standstill::Speed(channel) => channel,
		}
	}
}

/// A `[[button]]`: a button on one of the board's free pins, wired to GND, and the one
/// command a press of it gives ([`PinButton`](crate::control::PinButton)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ButtonPlan {
	/// The GPIO, one of [`BUTTON_PINS`](crate::control::BUTTON_PINS).
	pub pin: u8,
	pub action: Command,
}

/// One state of an enumerated field: an interval of raw values, both ends included —
/// an ODIS `TEXTTAB` level as `vag_data_labels::catalog::Level` keeps it, without its
/// name. A field's states are carried in the project's own order, and a state's place in
/// that list is its [`StateIndex`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
	pub lower: i32,
	pub upper: i32,
}

impl Band {
	/// An end at `i32::MIN` or `i32::MAX` — an ODX limit of kind `INFINITE`.
	pub const fn is_unbounded(&self) -> bool {
		self.lower == i32::MIN || self.upper == i32::MAX
	}

	pub fn contains(&self, raw: i64) -> bool {
		i64::from(self.lower) <= raw && raw <= i64::from(self.upper)
	}
}

/// Which state a raw reading is: `vag_data_labels::catalog::level_for`, the one matching
/// rule, on the board — bounded states first in table order, then unbounded ones, first
/// match wins. `None` where no state holds it.
pub fn state_of(bands: &[Band], raw: i64) -> Option<StateIndex> {
	let tried = |unbounded: bool| bands.iter().enumerate().filter(move |(_, band)| band.is_unbounded() == unbounded);
	tried(false)
		.chain(tried(true))
		.find(|(_, band)| band.contains(raw))
		.map(|(at, _)| StateIndex(at as u16))
}

/// The cruise lever, resolved: three fields as plan channels, each with its states, and
/// which of those states mean something here. Nothing in it names a car: the generator
/// wrote the indices from the owner's `[stalk]` texts and the project's levels.
///
/// The rocker and the switch are fields of **one identifier**, so one answer carries both:
/// the board subscribes the rocker's channel alone, and reads the switch out of the same
/// bytes. The cruise status is on another unit and read on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StalkPlan {
	/// Plan channel indices.
	pub rocker: u16,
	pub switch: u16,
	pub cruise: u16,
	pub rocker_states: &'static [Band],
	pub switch_states: &'static [Band],
	pub cruise_states: &'static [Band],
	pub states: States,
}

impl StalkPlan {
	/// One read of the lever from raw field values: the rocker and the switch from one
	/// answer, the cruise status as last read.
	pub fn read(&self, rocker: Option<i64>, switch: Option<i64>, cruise: Option<i64>) -> Read {
		Read {
			rocker: rocker.and_then(|raw| state_of(self.rocker_states, raw)),
			switch: switch.and_then(|raw| state_of(self.switch_states, raw)),
			cruise: cruise.and_then(|raw| state_of(self.cruise_states, raw)),
		}
	}
}

/// The stopwatch page, resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StopwatchPlan {
	/// The speed channel, a plan channel index.
	pub speed: u16,
	/// km/h per unit of the channel's value, measured on the car; `0.0` where it has not been.
	pub km_h_per_unit: f32,
	/// In km/h, in the owner's order; at most [`MAX_MARKS`](crate::stopwatch::MAX_MARKS).
	pub marks: &'static [u16],
}

/// How often the lever is read with the gate open: a 50 ms button (`todo/dash/19`).
pub const LEVER_OPEN_PERIOD_MS: u32 = 50;
/// With the gate closed: often enough to see it open.
pub const LEVER_CLOSED_PERIOD_MS: u32 = 500;
/// While the stopwatch is up: a press of about 0.2 s spans two reads (hold longer to be sure),
/// so `measure` can end it.
pub const LEVER_STOPWATCH_PERIOD_MS: u32 = 100;
/// The cruise status, the gate's second witness.
pub const CRUISE_PERIOD_MS: u32 = 200;

/// What the panel is doing, for [`Plan::rates_in`]: the lever's gate and the stopwatch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mode {
	/// The lever is ours ([`Stalk::gate_open`](crate::stalk::Stalk::gate_open)).
	pub gate_open: bool,
	pub stopwatch: Timing,
	/// The rocker's unit answers as the plan's — its part number matched — so the lever can be
	/// read. The cruise status serves nothing but a lever read, so without it it is not read.
	pub lever: bool,
}

/// Where the stopwatch is, as the bus sees it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Timing {
	/// The mode is off.
	#[default]
	Off,
	/// The mode is on: the speed is read at its own rate, for the standstill that arms it.
	Up,
	/// Armed or running: the speed keeps its rate and everything a page shows drops to the
	/// background — only an alarm's channels keep theirs.
	Timing,
}

/// One control unit and how to address it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unit {
	/// The CAN id requests go out on — `0x7E0` engine, `0x7E1` gearbox.
	pub request: u16,
	/// The id it answers on. Stored, not derived: the two id blocks on a VW
	/// have different rules (`vag_uds_client::address`), and the board has no
	/// business knowing either.
	pub response: u16,
	/// `F187`, as the unit reported it when the plan was built. What the
	/// firmware compares against at start-up.
	pub part_number: &'static str,
}

/// What came back when a unit was asked its part number (`F187`), as
/// [`Unit::check_part`] needs it — the scheduler's own types stay out of this crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartAnswer<'a> {
	/// The record's bytes, identifier echo stripped.
	Data(&'a [u8]),
	/// A negative response, by its NRC.
	Refused(u8),
	/// Nothing within the deadline.
	NoAnswer,
	/// The bus failed under the request.
	BusError,
	/// An answer that is not a response to what was asked.
	Malformed,
	/// The unit was heard from, busy, and did not answer in time.
	Busy,
}

/// What a part-number answer makes of a unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartCheck {
	/// The number the plan was built against: poll the unit.
	Matched,
	/// A definite answer that is not that number — another number, not text, or a
	/// refusal to say. Never polled this run: the plan's identifiers would answer,
	/// plausibly, about a unit they were not resolved for, and a unit that will not say
	/// what it is cannot be checked.
	Mismatch,
	/// Not there (yet): ask again, as often as the scheduler's backoff allows.
	Absent,
	/// Something came back that did not parse, the unit said it is busy (`21`), or it was heard
	/// and did not answer in time ([`PartAnswer::Busy`]). An answer, or a unit heard from: the
	/// scheduler does not back the unit off for it — for a `Busy`, it does as for silence, but for
	/// a unit a run is timing only from the second in a row;
	/// ask again no sooner than the backoff's cap.
	RetryLater,
}

/// ISO 14229-1's busyRepeatRequest: the unit is busy, and asks to be asked again. It says
/// nothing of what the unit is.
const BUSY_REPEAT_REQUEST: u8 = 0x21;

/// One value: where it is on the bus, how to cut it out, how to scale it, and
/// what to call it. Already rendered — there is nothing left to look up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Channel {
	/// The unit that owns it, by index into [`Plan::units`] ([`Plan::unit`]).
	pub unit: u8,
	/// The identifier `0x22` asks for.
	pub did: u16,
	/// Bits into the positive response, counted after the three-byte
	/// `62 <hi> <lo>` header, as ODX counts them: `byte * 8 + bit`, `bit` being
	/// the position of the field's least significant bit within that byte.
	pub bit_offset: u32,
	pub bit_length: u32,
	pub signed: bool,
	/// Whether the bytes run most-significant first. **Stored, never assumed.**
	pub big_endian: bool,
	/// `value = raw * factor + offset`. Only linear scalings exist here: a
	/// channel whose scaling is an enum or an unreversed anchor cannot be
	/// multiplied and the generator refuses it rather than guessing.
	pub factor: f32,
	pub offset: f32,
	/// Places after the point on the panel.
	pub decimals: u8,
	/// `"°C"`, `"bar"` — or empty for a count.
	pub unit_text: &'static str,
	/// Ten characters at most for a four-column page. The owner's wording — `dash.toml`'s
	/// `label`, or the glossary's [`Plan::language`] column — else the project's, in whatever
	/// language the project wrote it.
	pub label: &'static str,
	/// Whether a drive on a car established this scaling, as opposed to a
	/// catalog declaring it. Carried so the device can say which is which;
	/// it changes nothing about how the value is read.
	pub proven: bool,
	/// Readings a second while a page showing it is on the glass — the owner's
	/// `hz` in `dash.toml`, 2 when it gives none. Never derived on the board.
	pub hz: f32,
	/// The channel holding what the unit asked for, by its index into
	/// [`Plan::channels`] — `None` where the plan pairs this one with nothing
	/// (`todo/dash/18-setpoints-and-drift.md`).
	///
	/// Both are on the same unit and due together, so the planner asks for them in one `22`
	/// and the difference is between two numbers from the same moment.
	pub setpoint: Option<u16>,
	/// Shown as 0 while the car stands: `dash.toml`'s `zero_at_rest` (owner, 2026-10-05), for
	/// an acceleration sensor that reads the road's slope as acceleration when the car is
	/// still. "Stands" is the plan's one fact ([`Plan::stands`]); the plan generator refuses the
	/// flag in a plan with no [`Plan::standstill`]. It changes what the glass shows, not what
	/// alarms or charts watch.
	pub zero_at_rest: bool,
}

/// `value` rounded down to `decimals` places, so a formatter that rounds to nearest prints it
/// as it is: 99.97 with 0 decimals is `99`, not `100`.
pub fn round_down(value: f32, decimals: u8) -> f32 {
	let scale = [1.0f32, 10.0, 100.0, 1000.0][usize::from(decimals.min(3))];
	// The scaled value is close to a whole number already where it should land on one:
	// 100.0 * 10 is 1000, but 0.3 * 10 is 2.9999998 in f32 and would floor to 2.
	let scaled = value * scale;
	let nearest = round(scaled);
	let floored = if (scaled - nearest).abs() < 1e-3 { nearest } else { floor(scaled) };
	floored / scale
}

/// `f32::floor` and `f32::round` are `std`; this crate is `no_std`.
fn floor(x: f32) -> f32 {
	let t = x as i64 as f32;
	if t > x { t - 1.0 } else { t }
}

fn round(x: f32) -> f32 {
	floor(x + 0.5)
}

/// The slowest a channel on no visible page is read: once a second, or its own
/// rate if that is slower (owner, 2026-09-14, `todo/dash/14` §2).
pub const HIDDEN_PERIOD_MS: u32 = 1000;

/// How the panel wants one channel read right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
	/// Index into [`Plan::channels`].
	pub channel: u16,
	/// Read at its own rate, ahead of other work: it is on the page on the glass,
	/// or an alarm watches it — and an alarm watches whatever page is up, so its
	/// channels are never demoted. Otherwise only on pages not shown: read at
	/// [`HIDDEN_PERIOD_MS`] at most, last.
	pub foreground: bool,
	/// Read as a stopwatch's clock — the scheduler's `Class::Timing` — ahead of a host's reads
	/// and of the foreground past its floor, so a host's load cannot thin it. Only the
	/// stopwatch's speed while a run is up ([`Timing::Timing`]), which is foreground as well.
	pub timing: bool,
	pub period_ms: u32,
}

/// The most cells a values page holds — four columns on the panel. The generator refuses a page
/// with more, and an alarm marks at most this many cells on the page it raises.
pub const MAX_VALUE_CELLS: usize = 4;

/// What one page shows. Indices are into [`Plan::channels`].
///
/// Both kinds carry the strip's two settings: the `title` drawn in it (and the name an alarm
/// raises the page by), and the channel the strip reads its voltage from on this page
/// (`volts`) — `None` where the plan has no `[strip]`, the owner wrote `volts = false`, or the
/// page shows that channel itself already, as a cell or as its chart: the same number twice on
/// one page says nothing the once does not. The generator decided it; the board reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
	/// One channel, large, with its recent history. `min`/`max` are the chart's
	/// fixed range — never autoscaled, see `02`. `slot` is the chart's place among the plan's
	/// chart pages, `0..chart_count()`: the index of its [`History`](crate::history::History)
	/// in an array sized by [`Plan::chart_count`].
	Chart {
		title: &'static str,
		channel: u16,
		slot: u8,
		min: f32,
		max: f32,
		volts: Option<u16>,
	},
	/// Up to four columns, small label over a large number.
	Values {
		title: &'static str,
		cells: &'static [u16],
		volts: Option<u16>,
	},
}

impl Page {
	/// The title the strip shows; empty for none.
	pub const fn title(&self) -> &'static str {
		match self {
			Page::Chart { title, .. } | Page::Values { title, .. } => title,
		}
	}

	/// The channel the strip over this page reads its voltage from, if it shows one here.
	pub const fn volts(&self) -> Option<u16> {
		match self {
			Page::Chart { volts, .. } | Page::Values { volts, .. } => *volts,
		}
	}

	/// Whether the page shows the channel at `index` itself: as a cell, or as its chart.
	pub fn shows(&self, index: u16) -> bool {
		match self {
			Page::Chart { channel, .. } => *channel == index,
			Page::Values { cells, .. } => cells.contains(&index),
		}
	}

	/// Every channel the page reads for the glass: what it shows ([`pages::cells`](crate::pages::cells)
	/// — a chart's channel too, which samples every frame) and the voltage its strip shows. What
	/// the bus lists for the page, on the board and in the replay.
	pub fn reads(&self) -> impl Iterator<Item = u16> + '_ {
		crate::pages::cells(self).iter().copied().chain(self.volts())
	}
}

/// One chart the plan carries: where its history lives and what its axis is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Chart {
	/// The chart's position among the plan's chart pages, `0..chart_count()`
	/// — the index of its [`History`](crate::history::History) in an array
	/// sized by [`Plan::chart_count`].
	pub slot: usize,
	/// The channel it charts, as an index into [`Plan::channels`].
	pub channel: u16,
	pub min: f32,
	pub max: f32,
}

impl Plan {
	/// The channel a page cell or a configuration index refers to.
	pub fn channel(&self, index: u16) -> Option<&'static Channel> {
		self.channels.get(usize::from(index))
	}

	/// The unit a channel is read from: its index, as the generator wrote it. In a plan whose
	/// [`Plan::joins_hold`] every channel's unit is one of its units, and the firmware refuses
	/// to build on one that does not.
	pub fn unit(&self, channel: &Channel) -> &'static Unit {
		&self.units[usize::from(channel.unit)]
	}

	/// Whether the car stands now, as the plan's [`Plan::standstill`] says, `value_of` giving a
	/// channel's current reading: decided once per frame, and what every `zero_at_rest` cell
	/// of that frame is shown by. `None` with no source, or no current answer from it.
	pub fn stands(&self, value_of: impl Fn(u16) -> Option<f32>) -> Option<bool> {
		self.standstill?.stands(value_of)
	}

	/// What a cell shows for channel `index` whose reading is `value`, given whether the car
	/// stands ([`Plan::stands`], this frame's). Two things change it, both the owner's
	/// (2026-10-05):
	///
	/// - **The stopwatch's speed is shown rounded down** to its decimals. A mark counts when
	///   the speed reaches it, so a speed rounded up showed `100` at 99.5 while the run had
	///   not reached 100 yet, and it looked as if the mark were missed.
	/// - **A `zero_at_rest` channel shows 0 while the car stands.** Not knowing whether it
	///   stands changes nothing.
	pub fn shown(&self, index: u16, value: Option<f32>, stands: Option<bool>) -> Option<f32> {
		let value = value?;
		let Some(channel) = self.channel(index) else {
			return Some(value);
		};
		if self.stopwatch.is_some_and(|stopwatch| stopwatch.speed == index) {
			return Some(round_down(value, channel.decimals));
		}
		if channel.zero_at_rest && stands == Some(true) {
			return Some(0.0);
		}
		Some(value)
	}

	/// The strip's channel, where any page's strip shows it — what the generator's `bus:` line
	/// counts beside the alarms, as read on every page.
	pub fn strip_listed(&self) -> Option<u16> {
		self.pages.iter().find_map(Page::volts)
	}

	/// What the strip over page `page` shows, `value_of` giving a channel's current reading:
	/// the page's title and its voltage ([`Page::volts`]). A page the plan does not have
	/// — the stopwatch's — has no title or voltage ([`Strip::NONE`]). The board's panel and
	/// `dev recording dash` both draw it from here.
	pub fn strip(&self, page: usize, value_of: impl Fn(u16) -> Option<f32>) -> Strip<'static> {
		let Some(at) = self.pages.get(page) else {
			return Strip::NONE;
		};
		Strip {
			title: at.title(),
			volts: match at.volts() {
				Some(index) => Volts::Shown(value_of(index)),
				None => Volts::Hidden,
			},
		}
	}

	/// How many chart pages the plan has — how many histories the panel
	/// keeps. `const` so an array can be sized by it: the set of channels a
	/// chart can show is fixed when the image is built, because a chart is a
	/// fixed range and only the plan has one.
	pub const fn chart_count(&self) -> usize {
		// A `while` and an index because iterators are not `const`.
		let mut count = 0;
		let mut i = 0;
		while i < self.pages.len() {
			if let Page::Chart { .. } = self.pages[i] {
				count += 1;
			}
			i += 1;
		}
		count
	}

	/// Whether every index the generator wrote is one the board can index by: each channel's
	/// unit is one of [`Plan::units`]; each channel's setpoint, each index a page names — its
	/// cells, its chart's channel, its strip's voltage — the lever's rocker, switch and cruise
	/// status, the stopwatch's speed, the standstill's source and each alarm's channel and
	/// specified value are channels of the plan; each alarm's page is one of its pages; and the
	/// n-th chart page holds slot n, so the slots are `0..chart_count()`, each once. `const`, so
	/// the firmware refuses to build on a plan that fails it: an index past its table would be
	/// a panic on the car.
	pub const fn joins_hold(&self) -> bool {
		let channels = self.channels.len();
		// A `const fn` calls no closure.
		const fn on(index: u16, channels: usize) -> bool {
			(index as usize) < channels
		}
		// `while` and indices, because iterators are not `const`.
		let mut i = 0;
		while i < channels {
			if self.channels[i].unit as usize >= self.units.len() {
				return false;
			}
			if let Some(setpoint) = self.channels[i].setpoint
				&& !on(setpoint, channels)
			{
				return false;
			}
			i += 1;
		}
		if let Some(stalk) = &self.stalk
			&& !(on(stalk.rocker, channels) && on(stalk.switch, channels) && on(stalk.cruise, channels))
		{
			return false;
		}
		if let Some(stopwatch) = &self.stopwatch
			&& !on(stopwatch.speed, channels)
		{
			return false;
		}
		let mut a = 0;
		while a < self.alarms.len() {
			let alarm = &self.alarms[a];
			if !on(alarm.channel.0, channels) || alarm.page.0 as usize >= self.pages.len() {
				return false;
			}
			if let Rule::Drift { specified, .. } = alarm.rule
				&& !on(specified.0, channels)
			{
				return false;
			}
			a += 1;
		}
		let mut slot = 0;
		let mut p = 0;
		while p < self.pages.len() {
			let (volts, cells) = match &self.pages[p] {
				Page::Chart {
					channel, slot: at, volts, ..
				} => {
					if *at as usize != slot {
						return false;
					}
					slot += 1;
					(*volts, core::slice::from_ref(channel))
				}
				Page::Values { cells, volts, .. } => (*volts, *cells),
			};
			if let Some(volts) = volts
				&& !on(volts, channels)
			{
				return false;
			}
			let mut c = 0;
			while c < cells.len() {
				if !on(cells[c], channels) {
					return false;
				}
				c += 1;
			}
			p += 1;
		}
		match self.standstill {
			Some(standstill) => on(standstill.channel(), channels),
			None => true,
		}
	}

	/// Whether the plan's `[[button]]`s are ones the board can wire: at most
	/// [`MAX_BUTTONS`], each on one of [`BUTTON_PINS`](crate::control::BUTTON_PINS), no pin
	/// twice. The generator refuses anything else; `const`, so the firmware refuses it again
	/// when the image is built, where a button that silently did nothing would be the failure.
	pub const fn buttons_fit(&self) -> bool {
		if self.buttons.len() > MAX_BUTTONS {
			return false;
		}
		// `while` and indices, because iterators are not `const`.
		let mut i = 0;
		while i < self.buttons.len() {
			if !is_button_pin(self.buttons[i].pin) {
				return false;
			}
			let mut j = 0;
			while j < i {
				if self.buttons[j].pin == self.buttons[i].pin {
					return false;
				}
				j += 1;
			}
			i += 1;
		}
		true
	}

	/// Every chart page, in plan order, with its slot — what the panel feeds a sample to each
	/// frame.
	pub fn charts(&self) -> impl Iterator<Item = Chart> + '_ {
		self.pages.iter().filter_map(|page| match *page {
			Page::Chart { channel, slot, min, max, .. } => Some(Chart {
				slot: usize::from(slot),
				channel,
				min,
				max,
			}),
			Page::Values { .. } => None,
		})
	}

	/// How every channel worth reading is to be read, in plan order: the ones
	/// `shown` and the ones an alarm watches at their own rate, the ones only
	/// `listed` (on some page, not the one on the glass) no faster than
	/// [`HIDDEN_PERIOD_MS`], and a channel on no page not at all. A chart samples
	/// its channel every frame whether or not it is shown, so the caller lists
	/// every page's reads, charts included ([`Page::reads`]). An alarm's channels are the
	/// plan's business, not the caller's: they are foreground on every page, so no page
	/// switch can drop the reading that would trip the rule.
	pub fn rates<'a>(&'a self, shown: &'a [u16], listed: &'a [u16]) -> impl Iterator<Item = Rate> + 'a {
		self.rates_in(shown, listed, Mode::default())
	}

	/// [`Plan::rates`] with the lever and the stopwatch in it (`todo/dash/19`).
	///
	/// - The rocker's channel is read at [`LEVER_OPEN_PERIOD_MS`] with the gate open,
	///   [`LEVER_CLOSED_PERIOD_MS`] with it closed, [`LEVER_STOPWATCH_PERIOD_MS`] while the
	///   stopwatch is up; the switch's is never subscribed — it is in the rocker's answer.
	/// - The cruise status at [`CRUISE_PERIOD_MS`], while the rocker's unit answers as the
	///   plan's ([`Mode::lever`]) — it only ever opens the lever's gate, and a lever that cannot
	///   be read has none to open.
	/// - The stopwatch's speed at its own rate while the mode is on; while it is armed or
	///   running it is read as the stopwatch's clock ([`Rate::timing`]), and every other
	///   channel a page shows drops to the background. As foreground a host's reads took its
	///   slots once the lever had spent the panel's floor, and every run aborted (PR #12
	///   review); the scheduler keeps the panel's floor ahead of a clock on the board, so the
	///   lever and an alarm's channels keep theirs.
	///
	/// The rocker and the cruise status are foreground whatever the page: a press is a press
	/// on any. An alarm's channels keep their rate through all of it.
	pub fn rates_in<'a>(&'a self, shown: &'a [u16], listed: &'a [u16], mode: Mode) -> impl Iterator<Item = Rate> + 'a {
		self.channels.iter().enumerate().filter_map(move |(i, channel)| {
			let index = i as u16;
			let own = channel.period_ms();
			let foreground = |period_ms| {
				Some(Rate {
					channel: index,
					foreground: true,
					timing: false,
					period_ms,
				})
			};
			if let Some(stalk) = &self.stalk {
				if index == stalk.switch {
					return None;
				}
				if index == stalk.rocker {
					return foreground(match (mode.stopwatch, mode.gate_open) {
						(Timing::Off, true) => LEVER_OPEN_PERIOD_MS,
						(Timing::Off, false) => LEVER_CLOSED_PERIOD_MS,
						(Timing::Up | Timing::Timing, _) => LEVER_STOPWATCH_PERIOD_MS,
					});
				}
				if index == stalk.cruise {
					return if mode.lever { foreground(CRUISE_PERIOD_MS) } else { None };
				}
			}
			let speed = mode.stopwatch != Timing::Off && self.stopwatch.is_some_and(|s| s.speed == index);
			// Timing, what a page shows is not what matters: the run is.
			let on_glass = self.need(index, shown);
			let timing = mode.stopwatch == Timing::Timing;
			if speed && timing {
				Some(Rate {
					channel: index,
					foreground: true,
					timing: true,
					period_ms: own,
				})
			} else if speed || self.watched(index) {
				foreground(own)
			} else if let Some(period_ms) = on_glass.filter(|_| !timing) {
				foreground(period_ms)
			} else {
				let needed = match (on_glass, self.need(index, listed)) {
					(Some(a), Some(b)) => Some(a.min(b)),
					(a, b) => a.or(b),
				};
				needed.map(|period_ms| Rate {
					channel: index,
					foreground: false,
					timing: false,
					period_ms: period_ms.max(HIDDEN_PERIOD_MS),
				})
			}
		})
	}

	/// Whether any alarm watches the channel at `index`, either as a reading or as the
	/// specified value a drift rule measures against.
	pub fn watched(&self, index: u16) -> bool {
		self.alarms.iter().any(|alarm| {
			alarm.channel == ChannelId(index)
				|| match alarm.rule {
					Rule::Bounds { .. } => false,
					Rule::Drift { specified, .. } => specified == ChannelId(index),
				}
		})
	}

	/// How often the channels in `set` need the channel at `index` read, `None` where they do
	/// not need it at all. At its own rate where it is one of them, or the specified value of
	/// one (`todo/dash/18` §2) — the cell draws the two together. Where it is only the
	/// standstill's source, which a `zero_at_rest` channel in `set` cannot be shown without, no
	/// faster than the fastest of those: a 10 Hz cell zeroed at rest is served by its source at
	/// 10 Hz, whatever the source's own `hz`.
	fn need(&self, index: u16, set: &[u16]) -> Option<u32> {
		let own = self.channels[usize::from(index)].period_ms();
		let in_set = |i: usize| set.contains(&(i as u16));
		if set.contains(&index) || self.channels.iter().enumerate().any(|(i, c)| c.setpoint == Some(index) && in_set(i)) {
			return Some(own);
		}
		if !self.standstill.is_some_and(|s| s.channel() == index) {
			return None;
		}
		self
			.channels
			.iter()
			.enumerate()
			.filter(|&(i, c)| c.zero_at_rest && in_set(i))
			.map(|(_, c)| c.period_ms())
			.min()
			.map(|fastest| own.max(fastest))
	}

	/// The channels of the unit at `unit` (an index into [`Plan::units`]), in plan order — what
	/// one addressed conversation asks for before the backend is handed to the next unit.
	pub fn channels_of(&self, unit: usize) -> impl Iterator<Item = (u16, &'static Channel)> {
		self
			.channels
			.iter()
			.enumerate()
			.filter_map(move |(i, c)| (usize::from(c.unit) == unit).then_some((i as u16, c)))
	}
}
impl Unit {
	/// The car check (`05`): what this unit's answer to `F187` means for polling it.
	///
	/// `F187` carries the number padded — one trailing space on the reference car, and a
	/// NUL is the other thing a fixed-width field is padded with — so the padding is
	/// trimmed before comparing, exactly as the record the plan was built from trimmed it.
	pub fn check_part(&self, answer: PartAnswer<'_>) -> PartCheck {
		match answer {
			PartAnswer::Data(data) => match core::str::from_utf8(data) {
				Ok(reported) if reported.trim_end_matches([' ', '\0']) == self.part_number => PartCheck::Matched,
				_ => PartCheck::Mismatch,
			},
			// Busy — searching its fault memory for the board's fault count, say: ask again.
			PartAnswer::Refused(BUSY_REPEAT_REQUEST) => PartCheck::RetryLater,
			PartAnswer::Refused(_) => PartCheck::Mismatch,
			PartAnswer::NoAnswer | PartAnswer::BusError => PartCheck::Absent,
			PartAnswer::Malformed => PartCheck::RetryLater,
			// Heard from, no answer in time: there, busy — ask again.
			PartAnswer::Busy => PartCheck::RetryLater,
		}
	}
}

impl Channel {
	/// Milliseconds between two readings at [`Channel::hz`], never below one. A
	/// rate that is not a positive number — only a hand-edited plan could hold
	/// one, the generator refuses it — reads as 2 Hz rather than as a flood.
	pub fn period_ms(&self) -> u32 {
		const FALLBACK_MS: u32 = 500;
		if !(self.hz.is_finite() && self.hz > 0.0) {
			return FALLBACK_MS;
		}
		// `+ 0.5` and a cast: `f32::round` is not in `core`. The cast saturates.
		((1000.0 / self.hz + 0.5) as u32).max(1)
	}

	/// Cut the raw integer out of a positive response's data bytes (everything
	/// after the `62 <hi> <lo>` echo) and scale it.
	///
	/// `None` when the response is too short, when the field is wider than 32
	/// bits, or when a sub-byte field would cross a byte boundary — the ODX
	/// vocabulary has no byte-order rule for that case and this project has
	/// no evidence for one, so it is refused rather than guessed (the same
	/// line `RawForm::for_field` draws on the laptop).
	pub fn decode(&self, data: &[u8]) -> Option<f32> {
		let raw = self.raw(data)?;
		Some(raw as f32 * self.factor + self.offset)
	}

	/// The raw integer, before scaling.
	pub fn raw(&self, data: &[u8]) -> Option<i64> {
		let length = self.bit_length;
		if length == 0 || length > 32 {
			return None;
		}
		let byte = (self.bit_offset / 8) as usize;
		let shift = self.bit_offset % 8;

		let value: u64 = if shift == 0 && length % 8 == 0 {
			// Whole bytes, in the stored order.
			let bytes = (length / 8) as usize;
			let field = data.get(byte..byte + bytes)?;
			let mut acc = 0u64;
			if self.big_endian {
				for b in field {
					acc = (acc << 8) | u64::from(*b);
				}
			} else {
				for b in field.iter().rev() {
					acc = (acc << 8) | u64::from(*b);
				}
			}
			acc
		} else {
			// A field inside one byte.
			if length + shift > 8 {
				return None;
			}
			let mask = (1u64 << length) - 1;
			(u64::from(*data.get(byte)?) >> shift) & mask
		};

		// The host's reader (`RawForm::read`) refuses an unsigned 32-bit value
		// above `i32::MAX` and does not sign-extend a single bit; the board
		// agrees with it on both, so the same catalog row never reads
		// differently on the glass and in `watch`.
		if !self.signed && length == 32 && value > i32::MAX as u64 {
			return None;
		}
		Some(if self.signed && length > 1 {
			// Sign-extend from the field's own width.
			let sign = 1u64 << (length - 1);
			if value & sign != 0 {
				(value as i64) - (1i64 << length)
			} else {
				value as i64
			}
		} else {
			value as i64
		})
	}
}

#[cfg(test)]
mod tests {
	extern crate std;

	use super::*;

	const fn channel(bit_offset: u32, bit_length: u32, signed: bool, big_endian: bool, factor: f32, offset: f32) -> Channel {
		Channel {
			unit: 0,
			did: 0x0000,
			bit_offset,
			bit_length,
			signed,
			big_endian,
			factor,
			offset,
			decimals: 0,
			unit_text: "",
			label: "",
			proven: false,
			hz: 2.0,
			setpoint: None,
			zero_at_rest: false,
		}
	}

	/// The row the byte-order column exists for: gearbox `0x380A` is `u16`
	/// little-endian, and 690 /min arrives as `B2 02`.
	#[test]
	fn little_endian_u16_reads_690_not_45570() {
		let c = channel(0, 16, false, false, 1.0, 0.0);
		assert_eq!(c.raw(&[0xB2, 0x02]), Some(690));
		let wrong = channel(0, 16, false, true, 1.0, 0.0);
		assert_eq!(wrong.raw(&[0xB2, 0x02]), Some(45570));
	}

	/// OBD-II coolant: one byte, `A - 40`. `0x72` is 74 °C.
	#[test]
	fn u8_with_offset() {
		let c = channel(0, 8, false, true, 1.0, -40.0);
		assert_eq!(c.decode(&[0x72]), Some(74.0));
	}

	/// Boost `0x202A`: `u16` big-endian ×0.001 bar. `03DF` is 0.991.
	#[test]
	fn u16_big_endian_scaled() {
		let c = channel(0, 16, false, true, 0.001, 0.0);
		let v = c.decode(&[0x03, 0xDF]).unwrap();
		assert!((v - 0.991).abs() < 1e-6, "{v}");
	}

	/// Oil temperature as the catalog declares it: tenths of a kelvin.
	#[test]
	fn kelvin_tenths_to_celsius() {
		let c = channel(0, 16, false, true, 0.1, -273.14);
		// 3531 → 353.1 K → 79.96 °C
		let v = c.decode(&[0x0D, 0xCB]).unwrap();
		assert!((v - 79.96).abs() < 1e-3, "{v}");
	}

	const CHANNELS: [Channel; 3] = [
		channel(0, 8, false, true, 1.0, 0.0),
		channel(0, 8, false, true, 1.0, 0.0),
		channel(0, 8, false, true, 1.0, 0.0),
	];
	const PAGES: [Page; 4] = [
		Page::Values {
			title: "a",
			cells: &[0, 1],
			volts: None,
		},
		Page::Chart {
			title: "",
			channel: 1,
			slot: 0,
			min: 0.9,
			max: 2.1,
			volts: None,
		},
		Page::Values {
			title: "b",
			cells: &[2],
			volts: None,
		},
		Page::Chart {
			title: "",
			channel: 2,
			slot: 1,
			min: 0.0,
			max: 100.0,
			volts: None,
		},
	];
	const PLAN: Plan = Plan {
		vin: "",
		language: "en",
		units: &[PARTED],
		channels: &CHANNELS,
		pages: &PAGES,
		alarms: &[],
		stalk: None,
		stopwatch: None,
		buttons: &[],
		standstill: None,
	};

	/// A speed (0 decimals, the stopwatch's), an acceleration that is zeroed at rest, and a
	/// temperature that is neither.
	const SHOWN_CHANNELS: [Channel; 3] = [
		Channel {
			decimals: 0,
			..channel(0, 16, false, false, 0.01, 0.0)
		},
		Channel {
			decimals: 1,
			zero_at_rest: true,
			..channel(0, 16, false, true, 0.03125, -16.0)
		},
		Channel {
			decimals: 0,
			..channel(0, 8, false, true, 1.0, -40.0)
		},
	];
	const SHOWN_PLAN: Plan = Plan {
		vin: "",
		language: "en",
		units: &[PARTED],
		channels: &SHOWN_CHANNELS,
		pages: &[],
		alarms: &[],
		stalk: None,
		stopwatch: Some(StopwatchPlan {
			speed: 0,
			km_h_per_unit: 1.0,
			marks: &[60, 100],
		}),
		buttons: &[],
		standstill: Some(Standstill::Speed(0)),
	};

	/// Owner, 2026-10-05: the glass said `100` and the run did not finish. The mark counts at
	/// 100.00; rounded to nearest, 99.5 already showed `100`.
	#[test]
	fn the_stopwatch_speed_shows_100_only_once_it_is_100() {
		assert_eq!(SHOWN_PLAN.shown(0, Some(99.97), Some(false)), Some(99.0));
		assert_eq!(SHOWN_PLAN.shown(0, Some(99.5), Some(false)), Some(99.0));
		assert_eq!(SHOWN_PLAN.shown(0, Some(100.0), Some(false)), Some(100.0));
		assert_eq!(SHOWN_PLAN.shown(0, Some(100.4), Some(false)), Some(100.0));
		assert_eq!(SHOWN_PLAN.shown(0, None, None), None, "no answer stays no answer");
	}

	#[test]
	fn rounding_down_does_not_lose_a_value_that_is_already_whole_in_f32() {
		// 0.3 * 10 is 2.9999998 in f32: a plain floor would show 0.2.
		assert_eq!(round_down(0.3, 1), 0.3);
		assert_eq!(round_down(1.29, 1), 1.2);
		assert_eq!(round_down(-0.25, 1), -0.3, "down, not towards zero");
		assert_eq!(round_down(7.0, 0), 7.0);
	}

	/// Owner, 2026-10-05: standing, the ESC's acceleration read −0.2 and 0.6 m/s² — the slope.
	#[test]
	fn a_zero_at_rest_channel_shows_0_only_while_the_car_stands() {
		assert_eq!(SHOWN_PLAN.shown(1, Some(0.6), Some(true)), Some(0.0));
		assert_eq!(SHOWN_PLAN.shown(1, Some(-0.2), Some(true)), Some(0.0));
		assert_eq!(SHOWN_PLAN.shown(1, Some(2.5), Some(false)), Some(2.5), "rolling, it is the sensor's");
		assert_eq!(
			SHOWN_PLAN.shown(1, Some(0.6), None),
			Some(0.6),
			"not knowing is no claim that the car stands"
		);
		assert_eq!(SHOWN_PLAN.shown(1, None, Some(true)), None, "no answer is not a zero");
	}

	/// `todo/dash/26`: one fact of the plan, from its source, decided once per frame — today the
	/// stopwatch's speed reading exactly 0, the test the stopwatch arms on.
	#[test]
	fn the_car_stands_while_the_standstill_source_reads_exactly_0() {
		let speed = |v: Option<f32>| move |index: u16| if index == 0 { v } else { None };
		assert_eq!(SHOWN_PLAN.stands(speed(Some(0.0))), Some(true));
		assert_eq!(SHOWN_PLAN.stands(speed(Some(0.01))), Some(false), "a crawl is moving");
		assert_eq!(SHOWN_PLAN.stands(speed(None)), None, "no answer, no fact");
		assert_eq!(PLAN.stands(|_| Some(0.0)), None, "no source, no fact");
		assert!(stands_at(0.0) && stands_at(-0.0) && !stands_at(1.0));
	}

	#[test]
	fn other_channels_are_shown_as_read() {
		assert_eq!(SHOWN_PLAN.shown(2, Some(87.6), Some(true)), Some(87.6));
		assert_eq!(PLAN.shown(0, Some(99.97), Some(true)), Some(99.97), "no stopwatch, nothing rounded down");
	}

	/// A `zero_at_rest` channel cannot be shown without knowing whether the car stands: the
	/// standstill's source is read with it, as a specified value is with its channel — at its
	/// own rate with it on the glass, in the background with it on a page not shown, and not at
	/// all with it on no page.
	#[test]
	fn the_standstill_source_is_read_whenever_a_zero_at_rest_channel_is() {
		let plan = Plan {
			stopwatch: None,
			..SHOWN_PLAN
		};
		let rates = |shown: &[u16], listed: &[u16]| -> std::vec::Vec<(u16, bool, u32)> {
			plan.rates(shown, listed).map(|r| (r.channel, r.foreground, r.period_ms)).collect()
		};
		assert_eq!(
			rates(&[1], &[1]),
			[(0, true, 500), (1, true, 500)],
			"on the glass, the speed comes with it"
		);
		assert_eq!(
			rates(&[2], &[1, 2]),
			[(0, false, 1000), (1, false, 1000), (2, true, 500)],
			"and goes to the background with it"
		);
		assert_eq!(rates(&[2], &[2]), [(2, true, 500)], "on no page, neither is read");
	}

	/// Review of `refactor/dash-plan-joins`: a 50 Hz stopwatch speed on no page, pulled onto the
	/// glass only to zero a 10 Hz acceleration, is read at 10 Hz — every 100 ms, not every 20.
	/// A cell of the page, or the stopwatch's own, it keeps its own rate; and it is never read
	/// faster than its own `hz` for the sake of a faster cell.
	#[test]
	fn the_standstill_source_is_read_no_faster_than_the_cell_it_zeroes() {
		let mut channels = SHOWN_CHANNELS;
		channels[0].hz = 50.0;
		channels[1].hz = 10.0;
		let plan = Plan {
			channels: std::vec::Vec::leak(channels.to_vec()),
			..SHOWN_PLAN
		};
		let rates = |shown: &[u16], listed: &[u16], stopwatch: Timing| -> std::vec::Vec<(u16, bool, u32)> {
			let mode = Mode {
				stopwatch,
				..Mode::default()
			};
			plan
				.rates_in(shown, listed, mode)
				.map(|r| (r.channel, r.foreground, r.period_ms))
				.collect()
		};
		assert_eq!(
			rates(&[1], &[1], Timing::Off),
			[(0, true, 100), (1, true, 100)],
			"only zeroing the cell: at the cell's rate"
		);
		assert_eq!(
			rates(&[0, 1], &[0, 1], Timing::Off),
			[(0, true, 20), (1, true, 100)],
			"a cell of the page itself: its own"
		);
		assert_eq!(
			rates(&[1], &[1], Timing::Up),
			[(0, true, 20), (1, true, 100)],
			"the stopwatch's speed: its own"
		);
		assert_eq!(
			rates(&[2], &[1, 2], Timing::Off),
			[(0, false, 1000), (1, false, 1000), (2, true, 500)],
			"in the background, as the cell is"
		);
		let mut slow = channels;
		slow[0].hz = 2.0;
		let slow = Plan {
			channels: std::vec::Vec::leak(slow.to_vec()),
			..plan
		};
		assert_eq!(
			slow.rates(&[1], &[1]).map(|r| (r.channel, r.period_ms)).collect::<std::vec::Vec<_>>(),
			[(0, 500), (1, 100)],
			"never faster than its own hz"
		);
	}

	#[test]
	fn buttons_fit_the_board_on_its_free_pins_each_once_at_most_three() {
		const fn button(pin: u8) -> ButtonPlan {
			ButtonPlan { pin, action: Command::Next }
		}
		// A plan's slices are `'static`; a test's are leaked to be.
		let fits = |buttons: &[ButtonPlan]| {
			Plan {
				buttons: std::vec::Vec::leak(buttons.to_vec()),
				..PLAN
			}
			.buttons_fit()
		};
		// Known when the image is built, which is where the firmware asks.
		const { assert!(PLAN.buttons_fit(), "no buttons fit") };
		assert!(fits(&[button(3), button(4), button(5)]));
		assert!(fits(&[button(5)]));
		assert!(!fits(&[button(9)]), "BOOT is not a button's");
		assert!(!fits(&[button(2)]));
		assert!(!fits(&[button(3), button(4), button(3)]), "a pin twice");
		assert!(!fits(&[button(3), button(4), button(5), button(3)]), "four");
	}

	#[test]
	fn chart_count_is_a_constant_the_panel_can_size_an_array_by() {
		const N: usize = PLAN.chart_count();
		assert_eq!(N, 2);
		let _sized: [u8; N] = [0; N];
	}

	#[test]
	fn a_chart_is_numbered_among_chart_pages_and_carries_its_range() {
		let boost = Chart {
			slot: 0,
			channel: 1,
			min: 0.9,
			max: 2.1,
		};
		let load = Chart {
			slot: 1,
			channel: 2,
			min: 0.0,
			max: 100.0,
		};
		let all: std::vec::Vec<Chart> = PLAN.charts().collect();
		assert_eq!(all, [boost, load], "slots are dense and in plan order");
	}

	/// `todo/dash/27`: the generator writes every join as an index, and the image refuses to
	/// build on one that points past its table.
	#[test]
	fn the_joins_hold_only_where_every_index_names_something_and_the_slots_are_dense() {
		const { assert!(PLAN.joins_hold() && SHOWN_PLAN.joins_hold()) };
		// A test's slices are leaked to be `'static`, as a plan's are.
		let leak = |pages: &[Page]| -> &'static [Page] { std::vec::Vec::leak(pages.to_vec()) };
		let with_pages = |pages: &[Page]| Plan { pages: leak(pages), ..PLAN };
		let chart = |slot: u8, volts: Option<u16>| Page::Chart {
			title: "",
			channel: 1,
			slot,
			min: 0.0,
			max: 1.0,
			volts,
		};
		assert!(with_pages(&[chart(0, Some(2))]).joins_hold());
		assert!(!with_pages(&[chart(1, None)]).joins_hold(), "a slot past the first chart's");
		assert!(!with_pages(&[chart(0, None), chart(0, None)]).joins_hold(), "one slot twice");
		assert!(!with_pages(&[chart(0, Some(3))]).joins_hold(), "a voltage on no channel");
		let cells = |cells: &'static [u16]| Page::Values {
			title: "",
			cells,
			volts: None,
		};
		assert!(!with_pages(&[cells(&[0, 3])]).joins_hold(), "a cell on no channel");
		let unitless = Plan { units: &[], ..PLAN };
		assert!(!unitless.joins_hold(), "a channel's unit past the units");
		let far = Plan {
			standstill: Some(Standstill::Speed(3)),
			..PLAN
		};
		assert!(!far.joins_hold(), "a standstill on no channel");
	}

	#[test]
	fn a_page_reads_what_it_shows_and_its_strips_voltage() {
		let chart = Page::Chart {
			title: "",
			channel: 1,
			slot: 0,
			min: 0.0,
			max: 1.0,
			volts: Some(4),
		};
		let read: std::vec::Vec<u16> = chart.reads().collect();
		assert_eq!(read, [1, 4]);
		let read: std::vec::Vec<u16> = PAGES[0].reads().collect();
		assert_eq!(read, [0, 1], "no voltage on it, none read for it");
	}

	const PARTED: Unit = Unit {
		request: 0x7E0,
		response: 0x7E8,
		part_number: "PART1",
	};

	/// The car check (`05`): what an answer to `F187` makes of a unit.
	#[test]
	fn a_part_number_matches_after_its_padding_and_anything_definite_else_is_a_mismatch() {
		assert_eq!(PARTED.check_part(PartAnswer::Data(b"PART1")), PartCheck::Matched);
		assert_eq!(PARTED.check_part(PartAnswer::Data(b"PART1 ")), PartCheck::Matched, "a space pads it");
		assert_eq!(PARTED.check_part(PartAnswer::Data(b"PART1\0\0")), PartCheck::Matched, "so does a NUL");
		assert_eq!(PARTED.check_part(PartAnswer::Data(b"PART2")), PartCheck::Mismatch);
		assert_eq!(PARTED.check_part(PartAnswer::Data(&[0xFF, 0xFE])), PartCheck::Mismatch, "not text");
	}

	/// A unit that refuses `F187` cannot be checked against the plan, and a unit that
	/// cannot be checked is not polled: asking again changes nothing it would say.
	#[test]
	fn a_refused_part_number_is_a_mismatch_not_a_retry() {
		assert_eq!(PARTED.check_part(PartAnswer::Refused(0x31)), PartCheck::Mismatch);
		assert_eq!(PARTED.check_part(PartAnswer::Refused(0x22)), PartCheck::Mismatch);
	}

	/// `21` (busyRepeatRequest, ISO 14229-1) says nothing of what the unit is, only that it
	/// is busy — searching its fault memory for the board's own count, say (`todo/dash/20`).
	/// Asked again after the backoff's cap; a busy unit is never the wrong part for the boot.
	#[test]
	fn a_busy_unit_is_asked_its_part_number_again_not_declared_a_mismatch() {
		assert_eq!(PARTED.check_part(PartAnswer::Refused(0x21)), PartCheck::RetryLater);
		// Heard from, and no answer in time (`Miss::Busy`): the same.
		assert_eq!(PARTED.check_part(PartAnswer::Busy), PartCheck::RetryLater);
	}

	/// Silence is a unit that is not there yet (ignition off): asked again, at the pace
	/// the scheduler's backoff sets. An answer that did not parse was an answer, which
	/// resets that backoff — so it waits on its own, no faster than the backoff's cap.
	#[test]
	fn silence_is_asked_again_and_a_garbled_answer_later() {
		assert_eq!(PARTED.check_part(PartAnswer::NoAnswer), PartCheck::Absent);
		assert_eq!(PARTED.check_part(PartAnswer::BusError), PartCheck::Absent);
		assert_eq!(PARTED.check_part(PartAnswer::Malformed), PartCheck::RetryLater);
	}

	#[test]
	fn a_period_is_the_rate_inverted_and_a_nonsense_rate_is_two_hertz() {
		let at = |hz| Channel {
			hz,
			..channel(0, 8, false, true, 1.0, 0.0)
		};
		assert_eq!(at(2.0).period_ms(), 500);
		assert_eq!(at(10.0).period_ms(), 100);
		assert_eq!(at(0.5).period_ms(), 2000);
		assert_eq!(at(3.0).period_ms(), 333);
		assert_eq!(at(100_000.0).period_ms(), 1, "never zero");
		assert_eq!(at(0.0).period_ms(), 500);
		assert_eq!(at(-1.0).period_ms(), 500);
		assert_eq!(at(f32::NAN).period_ms(), 500);
	}

	#[test]
	fn the_shown_page_reads_at_its_rates_hidden_pages_at_one_hertz_at_most_and_the_rest_not_at_all() {
		const FAST: Channel = Channel {
			hz: 10.0,
			..channel(0, 8, false, true, 1.0, 0.0)
		};
		const SLOW: Channel = Channel {
			hz: 0.25,
			..channel(0, 8, false, true, 1.0, 0.0)
		};
		const MIXED: [Channel; 4] = [FAST, FAST, SLOW, channel(0, 8, false, true, 1.0, 0.0)];
		let plan = Plan { channels: &MIXED, ..PLAN };
		let rates = |shown: &[u16], listed: &[u16]| -> std::vec::Vec<(u16, bool, u32)> {
			plan.rates(shown, listed).map(|r| (r.channel, r.foreground, r.period_ms)).collect()
		};
		assert_eq!(
			rates(&[0], &[0, 1, 2]),
			[(0, true, 100), (1, false, 1000), (2, false, 4000)],
			"a hidden channel no faster than 1 Hz, a slower one at its own rate, channel 3 on no page not read"
		);
		assert_eq!(
			rates(&[1, 3], &[0, 1, 2]),
			[(0, false, 1000), (1, true, 100), (2, false, 4000), (3, true, 500)],
			"switching pages swaps who is shown"
		);
	}

	#[test]
	fn a_channel_an_alarm_watches_is_read_at_its_own_rate_whatever_page_is_up() {
		use crate::alarm::{Bound, PageId, Rule};
		const FAST: Channel = Channel {
			hz: 10.0,
			..channel(0, 8, false, true, 1.0, 0.0)
		};
		const CHANNELS: [Channel; 3] = [FAST, FAST, channel(0, 8, false, true, 1.0, 0.0)];
		static ALARMS: [Alarm; 1] = [Alarm {
			channel: ChannelId(1),
			page: PageId(2),
			rule: Rule::Bounds {
				max: Some(Bound { at: 10.0, off: 8.0 }),
				min: None,
			},
		}];
		let plan = Plan {
			channels: &CHANNELS,
			alarms: &ALARMS,
			..PLAN
		};
		let rates = |shown: &[u16], listed: &[u16]| -> std::vec::Vec<(u16, bool, u32)> {
			plan.rates(shown, listed).map(|r| (r.channel, r.foreground, r.period_ms)).collect()
		};
		assert!(plan.watched(1) && !plan.watched(0) && !plan.watched(2));
		assert_eq!(
			rates(&[0], &[0, 1, 2]),
			[(0, true, 100), (1, true, 100), (2, false, 1000)],
			"channel 1 is on a hidden page and still at 10 Hz, ahead of other work"
		);
		assert_eq!(
			rates(&[2], &[0, 1, 2]),
			[(0, false, 1000), (1, true, 100), (2, true, 500)],
			"a page switch demotes what was shown and not what the alarm watches"
		);
		assert_eq!(rates(&[], &[]), [(1, true, 100)], "watched even with no page asking for it");
	}

	#[test]
	fn a_specified_value_is_read_exactly_when_the_channel_it_explains_is() {
		const FAST: Channel = Channel {
			hz: 10.0,
			..channel(0, 8, false, true, 1.0, 0.0)
		};
		// Channel 0 is paired with channel 1, which is on no page of its own.
		const PAIRED: Channel = Channel { setpoint: Some(1), ..FAST };
		const CHANNELS: [Channel; 3] = [PAIRED, FAST, channel(0, 8, false, true, 1.0, 0.0)];
		let plan = Plan { channels: &CHANNELS, ..PLAN };
		let rates = |shown: &[u16], listed: &[u16]| -> std::vec::Vec<(u16, bool, u32)> {
			plan.rates(shown, listed).map(|r| (r.channel, r.foreground, r.period_ms)).collect()
		};
		assert_eq!(
			rates(&[0], &[0, 2]),
			[(0, true, 100), (1, true, 100), (2, false, 1000)],
			"the specified value comes up with the channel it explains, at the same rate"
		);
		assert_eq!(
			rates(&[2], &[0, 2]),
			[(0, false, 1000), (1, false, 1000), (2, true, 500)],
			"and goes to the background with it"
		);
		assert_eq!(rates(&[2], &[2]), [(2, true, 500)], "a pair on no page at all is not read");
	}

	#[test]
	fn signed_i16_both_orders() {
		let be = channel(0, 16, true, true, 1.0, 0.0);
		assert_eq!(be.raw(&[0xFF, 0xFE]), Some(-2));
		let le = channel(0, 16, true, false, 1.0, 0.0);
		assert_eq!(le.raw(&[0xFE, 0xFF]), Some(-2));
	}

	#[test]
	fn second_byte_field() {
		let c = channel(8, 8, false, true, 1.0, 0.0);
		assert_eq!(c.raw(&[0x11, 0x22, 0x33]), Some(0x22));
	}

	/// A three-bit selector at bit 3 of the second byte, and the byte after
	/// it is not consulted.
	#[test]
	fn sub_byte_field_keeps_its_offset_in_bits() {
		let c = channel(8 + 3, 3, false, true, 1.0, 0.0);
		// 0b0011_1000 → bits 3..6 = 0b111
		assert_eq!(c.raw(&[0x00, 0b0011_1000, 0xFF]), Some(7));
		let one_bit = channel(7, 1, false, true, 1.0, 0.0);
		assert_eq!(one_bit.raw(&[0x80]), Some(1));
		assert_eq!(one_bit.raw(&[0x7F]), Some(0));
	}

	#[test]
	fn signed_sub_byte_field_sign_extends_from_its_own_width() {
		let c = channel(0, 4, true, true, 1.0, 0.0);
		assert_eq!(c.raw(&[0x0F]), Some(-1));
		assert_eq!(c.raw(&[0x07]), Some(7));
	}

	#[test]
	fn refuses_what_it_cannot_say() {
		assert_eq!(channel(0, 16, false, true, 1.0, 0.0).raw(&[0x01]), None, "too short");
		assert_eq!(channel(6, 4, false, true, 1.0, 0.0).raw(&[0xFF, 0xFF]), None, "crosses a byte");
		assert_eq!(channel(0, 40, false, true, 1.0, 0.0).raw(&[0; 8]), None, "wider than 32 bits");
		assert_eq!(channel(0, 0, false, true, 1.0, 0.0).raw(&[0; 8]), None, "empty");
	}

	/// Where the host refuses, the board refuses: an unsigned 32-bit field
	/// past `i32::MAX`, and a one-bit "signed" flag that is just a flag.
	#[test]
	fn agrees_with_the_hosts_reader_at_the_edges() {
		let wide = channel(0, 32, false, true, 1.0, 0.0);
		assert_eq!(wide.raw(&[0xFF; 4]), None);
		assert_eq!(wide.raw(&[0x7F, 0xFF, 0xFF, 0xFF]), Some(i32::MAX as i64));
		let flag = channel(0, 1, true, true, 1.0, 0.0);
		assert_eq!(flag.raw(&[0x01]), Some(1));
	}

	#[test]
	fn u32_big_endian_fits() {
		let c = channel(0, 32, false, true, 1.0, 0.0);
		assert_eq!(c.raw(&[0x0C, 0xAF, 0x3A, 0x8D]), Some(0x0CAF_3A8D));
	}

	#[test]
	fn plan_lookups() {
		static UNITS: [Unit; 2] = [
			Unit {
				request: 0x7E0,
				response: 0x7E8,
				part_number: "A",
			},
			Unit {
				request: 0x7E1,
				response: 0x7E9,
				part_number: "B",
			},
		];
		static CHANNELS: [Channel; 3] = [
			channel(0, 8, false, true, 1.0, 0.0),
			Channel {
				unit: 1,
				..channel(0, 8, false, true, 1.0, 0.0)
			},
			channel(8, 8, false, true, 1.0, 0.0),
		];
		static CELLS: [u16; 2] = [0, 2];
		static PAGES: [Page; 1] = [Page::Values {
			title: "",
			cells: &CELLS,
			volts: None,
		}];
		static PLAN: Plan = Plan {
			vin: "VIN",
			language: "en",
			units: &UNITS,
			channels: &CHANNELS,
			pages: &PAGES,
			alarms: &[],
			stalk: None,
			stopwatch: None,
			buttons: &[],
			standstill: None,
		};
		let engine: std::vec::Vec<u16> = PLAN.channels_of(0).map(|(i, _)| i).collect();
		assert_eq!(engine, [0, 2]);
		let gearbox: std::vec::Vec<u16> = PLAN.channels_of(1).map(|(i, _)| i).collect();
		assert_eq!(gearbox, [1]);
		assert_eq!(PLAN.channel(9), None);
		assert_eq!(PLAN.unit(&CHANNELS[1]).response, 0x7E9, "the channel's unit, by its index");
		assert!(PLAN.joins_hold());
	}

	/// A neutral ladder in the shape an ODIS text table has: bounded bands in table order,
	/// one listed out of order, and a catch-all with an unbounded end listed first.
	const LADDER: [Band; 5] = [
		Band {
			lower: i32::MIN,
			upper: i32::MAX,
		},
		Band { lower: 0, upper: 49 },
		Band { lower: 100, upper: 149 },
		Band { lower: 50, upper: 99 },
		Band { lower: 150, upper: 150 },
	];

	#[test]
	fn a_raw_reading_takes_the_first_bounded_state_that_holds_it_and_the_catch_all_last() {
		assert_eq!(state_of(&LADDER, 0), Some(StateIndex(1)));
		assert_eq!(state_of(&LADDER, 49), Some(StateIndex(1)), "both ends are in");
		assert_eq!(state_of(&LADDER, 72), Some(StateIndex(3)), "table order, not sorted order");
		assert_eq!(state_of(&LADDER, 150), Some(StateIndex(4)), "a point");
		assert_eq!(
			state_of(&LADDER, 151),
			Some(StateIndex(0)),
			"the unbounded one only when nothing else holds it"
		);
		assert_eq!(state_of(&LADDER[1..], 151), None, "and without it, nothing does");
		assert_eq!(state_of(&LADDER[1..], i64::from(i32::MAX) + 1), None, "past what a band can hold");
	}

	/// Channels 0 and 1 are the driver's; 2 the rocker, 3 the switch (both of one identifier),
	/// 4 the cruise status, 5 the stopwatch's speed at 50 Hz, 6 a channel an alarm watches.
	fn lever_plan() -> Plan {
		use crate::alarm::{Bound, PageId};
		const FAST: Channel = Channel {
			hz: 10.0,
			..channel(0, 8, false, true, 1.0, 0.0)
		};
		static CHANNELS: [Channel; 7] = [FAST, FAST, FAST, FAST, FAST, Channel { hz: 50.0, ..FAST }, FAST];
		static ALARMS: [Alarm; 1] = [Alarm {
			channel: ChannelId(6),
			page: PageId(0),
			rule: Rule::Bounds {
				max: Some(Bound { at: 10.0, off: 8.0 }),
				min: None,
			},
		}];
		static MARKS: [u16; 2] = [60, 100];
		Plan {
			channels: &CHANNELS,
			alarms: &ALARMS,
			stalk: Some(StalkPlan {
				rocker: 2,
				switch: 3,
				cruise: 4,
				rocker_states: &LADDER,
				switch_states: &LADDER,
				cruise_states: &LADDER,
				states: States {
					next: StateIndex(1),
					previous: StateIndex(2),
					measure: StateIndex(3),
					switch_off: StateIndex(1),
					cruise_off: StateIndex(1),
				},
			}),
			stopwatch: Some(StopwatchPlan {
				speed: 5,
				km_h_per_unit: 0.1,
				marks: &MARKS,
			}),
			..PLAN
		}
	}

	/// Review of `refactor/dash-plan-joins`: the lever's, the stopwatch's, a setpoint's and an
	/// alarm's indices are ones the board indexes by too (`lever_unit` reads the rocker's channel
	/// directly), so each is held to the plan's tables like a page's.
	#[test]
	fn the_joins_hold_for_the_lever_the_stopwatch_setpoints_and_alarms() {
		use crate::alarm::{Bound, PageId};
		let plan = lever_plan();
		assert!(plan.joins_hold());
		let past = plan.channels.len() as u16;
		let stalk = plan.stalk.unwrap();
		for (what, bad) in [
			("a rocker on no channel", StalkPlan { rocker: past, ..stalk }),
			("a switch on no channel", StalkPlan { switch: past, ..stalk }),
			("a cruise status on no channel", StalkPlan { cruise: past, ..stalk }),
		] {
			assert!(!Plan { stalk: Some(bad), ..plan }.joins_hold(), "{what}");
		}
		let stopwatch = StopwatchPlan {
			speed: past,
			..plan.stopwatch.unwrap()
		};
		assert!(
			!Plan {
				stopwatch: Some(stopwatch),
				..plan
			}
			.joins_hold(),
			"a stopwatch speed on no channel"
		);
		let mut channels = plan.channels.to_vec();
		channels[0].setpoint = Some(past);
		let channels: &'static [Channel] = std::vec::Vec::leak(channels);
		assert!(!Plan { channels, ..plan }.joins_hold(), "a setpoint on no channel");
		let bound = Some(Bound { at: 10.0, off: 8.0 });
		let pages = plan.pages.len() as u16;
		let alarm = |alarm: Alarm| Plan {
			alarms: std::vec::Vec::leak(std::vec![alarm]),
			..plan
		};
		assert!(alarm(Alarm::bounds(ChannelId(6), PageId(0), bound, None)).joins_hold());
		assert!(
			!alarm(Alarm::bounds(ChannelId(past), PageId(0), bound, None)).joins_hold(),
			"an alarm on no channel"
		);
		assert!(
			!alarm(Alarm::bounds(ChannelId(6), PageId(pages), bound, None)).joins_hold(),
			"an alarm on no page"
		);
		assert!(alarm(Alarm::drift(ChannelId(6), ChannelId(1), PageId(0), 10.0, 6.0, 1_000, 0.5)).joins_hold());
		assert!(
			!alarm(Alarm::drift(ChannelId(6), ChannelId(past), PageId(0), 10.0, 6.0, 1_000, 0.5)).joins_hold(),
			"a specified value on no channel"
		);
	}

	#[test]
	fn the_lever_is_read_at_the_gates_rate_its_switch_never_and_the_cruise_status_at_five_hertz() {
		let plan = lever_plan();
		let rates = |mode: Mode| -> std::vec::Vec<(u16, bool, u32)> {
			plan
				.rates_in(&[0, 1], &[0, 1], mode)
				.map(|r| (r.channel, r.foreground, r.period_ms))
				.collect()
		};
		let closed = Mode {
			lever: true,
			..Mode::default()
		};
		let open = Mode { gate_open: true, ..closed };
		assert_eq!(
			rates(closed),
			[(0, true, 100), (1, true, 100), (2, true, 500), (4, true, 200), (6, true, 100)],
			"gate closed: 2 Hz, enough to see it open; the switch is in the rocker's answer"
		);
		// The rocker's unit not the plan's, or not answering: no lever read can use the cruise
		// status, so it is not read (PR #12 review).
		let unmatched = Mode { lever: false, ..closed };
		assert!(!rates(unmatched).iter().any(|r| r.0 == 4), "{:?}", rates(unmatched));
		assert_eq!(rates(open)[2], (2, true, 50), "gate open: a 50 ms button");
		for timing in [Timing::Up, Timing::Timing] {
			let up = Mode {
				gate_open: true,
				stopwatch: timing,
				lever: true,
			};
			assert!(
				rates(up).contains(&(2, true, 100)),
				"{timing:?}: 10 Hz while the stopwatch is up, gate or not"
			);
		}
		// On no page at all the lever is still read: a press is a press on any.
		let bare: std::vec::Vec<u16> = plan.rates_in(&[], &[], closed).map(|r| r.channel).collect();
		assert_eq!(bare, [2, 4, 6]);
	}

	#[test]
	fn the_stopwatchs_speed_is_read_at_its_own_rate_and_while_timing_nothing_a_page_shows_is() {
		let plan = lever_plan();
		let rates = |shown: &[u16], mode: Mode| -> std::vec::Vec<(u16, bool, u32)> {
			plan
				.rates_in(shown, &[0, 1], mode)
				.map(|r| (r.channel, r.foreground, r.period_ms))
				.collect()
		};
		let off = rates(&[0, 1], Mode::default());
		assert!(
			!off.iter().any(|r| r.0 == 5),
			"the mode off, the speed is on no page and not read: {off:?}"
		);
		// The stopwatch on the glass: no page is shown, the speed is.
		let up = rates(
			&[],
			Mode {
				stopwatch: Timing::Up,
				lever: true,
				..Mode::default()
			},
		);
		assert_eq!(
			up,
			[
				(0, false, 1000),
				(1, false, 1000),
				(2, true, 100),
				(4, true, 200),
				(5, true, 20),
				(6, true, 100)
			]
		);
		// An alarm took the glass during a run: its page's cells drop to the background, its
		// watched channel does not, and the speed keeps its rate.
		let timing = rates(
			&[0, 6],
			Mode {
				stopwatch: Timing::Timing,
				lever: true,
				..Mode::default()
			},
		);
		assert_eq!(
			timing,
			[
				(0, false, 1000),
				(1, false, 1000),
				(2, true, 100),
				(4, true, 200),
				(5, true, 20),
				(6, true, 100)
			]
		);
		// The same page with the stopwatch merely up keeps its cells.
		assert!(
			rates(
				&[0, 6],
				Mode {
					stopwatch: Timing::Up,
					lever: true,
					..Mode::default()
				}
			)
			.contains(&(0, true, 100))
		);
		// A run up, the speed is read as the stopwatch's clock, and nothing else is: a host's
		// reads must not thin it (PR #12 review). Merely up, it waits for a standstill at the
		// panel's own rank.
		let clock = |stopwatch: Timing| -> std::vec::Vec<u16> {
			let mode = Mode {
				stopwatch,
				lever: true,
				..Mode::default()
			};
			plan.rates_in(&[0, 6], &[0, 1], mode).filter(|r| r.timing).map(|r| r.channel).collect()
		};
		assert_eq!(clock(Timing::Timing), [5]);
		assert_eq!(clock(Timing::Up), [] as [u16; 0]);
		assert_eq!(clock(Timing::Off), [] as [u16; 0]);
	}

	#[test]
	fn a_lever_read_is_its_three_fields_through_their_own_states() {
		let stalk = lever_plan().stalk.unwrap();
		let read = stalk.read(Some(72), Some(10), None);
		assert_eq!(
			read,
			Read {
				rocker: Some(StateIndex(3)),
				switch: Some(StateIndex(1)),
				cruise: None
			}
		);
	}

	/// Channel 0 is the battery voltage, resolved as the generator resolves it: page 0 shows it
	/// as a cell and page 1 charts it, so neither strip shows it; page 2 shows other channels
	/// with the voltage on; page 3 has it off; page 4 is a chart of another channel with a title.
	fn strip_plan() -> Plan {
		static PAGES: [Page; 5] = [
			Page::Values {
				title: "BATTERY",
				cells: &[0, 1],
				volts: None,
			},
			Page::Chart {
				title: "",
				channel: 0,
				slot: 0,
				min: 10.0,
				max: 15.0,
				volts: None,
			},
			Page::Values {
				title: "",
				cells: &[1, 2],
				volts: Some(0),
			},
			Page::Values {
				title: "QUIET",
				cells: &[2],
				volts: None,
			},
			Page::Chart {
				title: "BOOST",
				channel: 1,
				slot: 1,
				min: 0.0,
				max: 2.5,
				volts: Some(0),
			},
		];
		Plan { pages: &PAGES, ..PLAN }
	}

	#[test]
	fn the_strips_channel_is_listed_where_a_page_shows_it_there() {
		let plan = strip_plan();
		assert!(plan.joins_hold());
		assert_eq!(plan.strip_listed(), Some(0));
		assert_eq!(PLAN.strip_listed(), None, "no [strip], no voltage anywhere");
		let quiet = Plan {
			pages: &strip_plan().pages[..2],
			..strip_plan()
		};
		assert_eq!(
			quiet.strip_listed(),
			None,
			"no page shows it in the strip: it is read as its pages read it"
		);
	}
	#[test]
	fn the_strip_of_a_page_is_its_title_and_its_voltage_as_read() {
		let plan = strip_plan();
		let read = |index: u16| (index == 0).then_some(14.2);
		assert_eq!(
			plan.strip(2, read),
			Strip {
				title: "",
				volts: Volts::Shown(Some(14.2))
			}
		);
		assert_eq!(
			plan.strip(2, |_| None),
			Strip {
				title: "",
				volts: Volts::Shown(None)
			},
			"no answer is shown as none, not hidden"
		);
		assert_eq!(
			plan.strip(4, read),
			Strip {
				title: "BOOST",
				volts: Volts::Shown(Some(14.2))
			},
			"a chart's title"
		);
		assert_eq!(
			plan.strip(0, read),
			Strip {
				title: "BATTERY",
				volts: Volts::Hidden
			}
		);
		assert_eq!(plan.strip(1, read), Strip::NONE, "no title, and the voltage is the chart");
		assert_eq!(plan.strip(9, read), Strip::NONE, "the stopwatch's page is no plan page");
	}
}
