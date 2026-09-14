//! The plan: everything the device knows about the car, resolved on the laptop.
//!
//! `todo/dash/01-plan-format.md` decides the shape and this module is that
//! shape, in the form the firmware can hold — `&'static` everything, no
//! allocation, no lookup. The generator (`vag_cli_core::dash`) writes a
//! `static PLAN: Plan = Plan { … }` from the catalogs, the firmware
//! `include!`s it, and at run time does exactly one thing per channel: send
//! `0x22`, take the bits, multiply, draw. Nothing here resolves anything.
//!
//! Two things differ from the task's sketch, both because of what already
//! exists on the board: pages point at channels **by index** rather than
//! carrying them, because `vag_dash_fw::config::Page` stores cells as indices
//! into the flashed plan and has since before there was one; and every channel
//! carries its unit's request id rather than the unit carrying its channels,
//! because the poll loop walks *units* (one conversation at a time — see
//! `vag-cli-core/src/plan.rs`) and asks each for its channels.
//!
//! Why this lives in the renderer crate and not the firmware: the firmware is
//! not a workspace member and cannot be built for the host, so anything only
//! there is untested by CI. [`Channel::decode`] is the one piece of arithmetic
//! between the bus and the glass, and the little-endian row that motivates its
//! byte-order flag (`0x380A`, 690 /min read as 45570 by a reader that assumed
//! big-endian) is exactly the bug a host test catches for free.

use crate::alarm::{Alarm, ChannelId};

/// The whole interface between the laptop and the device.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plan {
	/// Baked so the firmware can check where it is. A plan built for one car
	/// asked of another answers plausibly and wrongly; `05` refuses to poll on a
	/// mismatch.
	pub vin: &'static str,
	/// `"en"` or `"ru"` — the language every [`Channel::label`] is already in.
	pub language: &'static str,
	/// The control units the plan reads, in the order they are polled.
	pub units: &'static [Unit],
	/// Every value the plan can put on screen. Pages and the device's own
	/// configuration refer to these by index.
	pub channels: &'static [Channel],
	pub pages: &'static [Page],
	/// The owner's `[[alarm]]` rules, in priority order. A [`ChannelId`] is an
	/// index into [`Plan::channels`] and a [`PageId`](crate::alarm::PageId) an index
	/// into [`Plan::pages`]: an image is built for one plan, so an index is a name
	/// that cannot drift. The generator has checked each rule — its page is a values
	/// page holding every channel it watches, and its release is on the far side of
	/// its trip — and carries at most [`MAX_ALARMS`](crate::alarm::MAX_ALARMS).
	pub alarms: &'static [Alarm<'static>],
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
	/// Something came back that did not parse. It was an answer, so the scheduler does
	/// not back the unit off; ask again no sooner than the backoff's cap.
	RetryLater,
}

/// One value: where it is on the bus, how to cut it out, how to scale it, and
/// what to call it. Already rendered — there is nothing left to look up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Channel {
	/// Request id of the unit that owns it — a key into [`Plan::units`].
	pub unit: u16,
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
	/// In [`Plan::language`], ten characters at most for a four-column page.
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
	pub period_ms: u32,
}

/// What one page shows. Indices are into [`Plan::channels`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Page {
	/// One channel, large, with its recent history. `min`/`max` are the chart's
	/// fixed range — never autoscaled, see `02`.
	Chart { channel: u16, min: f32, max: f32 },
	/// Up to four columns, small label over a large number.
	Values { title: &'static str, cells: &'static [u16] },
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

	/// Every chart page, in plan order, numbered — what the panel feeds a
	/// sample to each frame.
	pub fn charts(&self) -> impl Iterator<Item = Chart> + '_ {
		self
			.pages
			.iter()
			.filter_map(|page| match page {
				Page::Chart { channel, min, max } => Some((*channel, *min, *max)),
				_ => None,
			})
			.enumerate()
			.map(|(slot, (channel, min, max))| Chart { slot, channel, min, max })
	}

	/// The chart the plan gives a channel, if it gives one. A channel with
	/// two chart pages is one chart — the first — because one channel has one
	/// history.
	pub fn chart(&self, index: u16) -> Option<Chart> {
		self.charts().find(|chart| chart.channel == index)
	}

	/// The unit a channel is read from.
	pub fn unit_of(&self, channel: &Channel) -> Option<&'static Unit> {
		self.units.iter().find(|u| u.request == channel.unit)
	}

	/// How every channel worth reading is to be read, in plan order: the ones
	/// `shown` and the ones an alarm watches at their own rate, the ones only
	/// `listed` (on some page, not the one on the glass) no faster than
	/// [`HIDDEN_PERIOD_MS`], and a channel on no page not at all. A chart samples
	/// its channel every frame whether or not it is shown, so the caller lists
	/// every page's cells, charts included. An alarm's channels are the plan's
	/// business, not the caller's: they are foreground on every page, so no page
	/// switch can drop the reading that would trip the rule.
	pub fn rates<'a>(&'a self, shown: &'a [u16], listed: &'a [u16]) -> impl Iterator<Item = Rate> + 'a {
		self.channels.iter().enumerate().filter_map(move |(i, channel)| {
			let index = i as u16;
			let own = channel.period_ms();
			if shown.contains(&index) || self.watched(index) {
				Some(Rate {
					channel: index,
					foreground: true,
					period_ms: own,
				})
			} else if listed.contains(&index) {
				Some(Rate {
					channel: index,
					foreground: false,
					period_ms: own.max(HIDDEN_PERIOD_MS),
				})
			} else {
				None
			}
		})
	}

	/// Whether any alarm watches the channel at `index`.
	pub fn watched(&self, index: u16) -> bool {
		self.alarms.iter().any(|alarm| alarm.channels.contains(&ChannelId(index)))
	}

	/// The channels one unit owns, in plan order — what one addressed
	/// conversation asks for before the backend is handed to the next unit.
	pub fn channels_of(&self, unit: &Unit) -> impl Iterator<Item = (u16, &'static Channel)> {
		let request = unit.request;
		self
			.channels
			.iter()
			.enumerate()
			.filter_map(move |(i, c)| (c.unit == request).then_some((i as u16, c)))
	}
}

impl Unit {
	/// The car check (`05`): what this unit's answer to `F187` means for polling it.
	///
	/// `F187` carries the number padded — one trailing space on the reference car, and a
	/// NUL is the other thing a fixed-width field is padded with — so the padding is
	/// trimmed before comparing, exactly as the survey the plan was built from trimmed it.
	pub fn check_part(&self, answer: PartAnswer<'_>) -> PartCheck {
		match answer {
			PartAnswer::Data(data) => match core::str::from_utf8(data) {
				Ok(reported) if reported.trim_end_matches([' ', '\0']) == self.part_number => PartCheck::Matched,
				_ => PartCheck::Mismatch,
			},
			PartAnswer::Refused(_) => PartCheck::Mismatch,
			PartAnswer::NoAnswer | PartAnswer::BusError => PartCheck::Absent,
			PartAnswer::Malformed => PartCheck::RetryLater,
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
			unit: 0x7E0,
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
		Page::Values { title: "a", cells: &[0, 1] },
		Page::Chart {
			channel: 1,
			min: 0.9,
			max: 2.1,
		},
		Page::Values { title: "b", cells: &[2] },
		Page::Chart {
			channel: 2,
			min: 0.0,
			max: 100.0,
		},
	];
	const PLAN: Plan = Plan {
		vin: "",
		language: "en",
		units: &[],
		channels: &CHANNELS,
		pages: &PAGES,
		alarms: &[],
	};

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
		assert_eq!(PLAN.chart(1), Some(boost));
		assert_eq!(PLAN.chart(2), Some(load));
		let all: std::vec::Vec<Chart> = PLAN.charts().collect();
		assert_eq!(all, [boost, load], "slots are dense and in plan order");
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
		use crate::alarm::{Direction, PageId, Rule};
		const FAST: Channel = Channel {
			hz: 10.0,
			..channel(0, 8, false, true, 1.0, 0.0)
		};
		const CHANNELS: [Channel; 3] = [FAST, FAST, channel(0, 8, false, true, 1.0, 0.0)];
		static WATCHED: [ChannelId; 1] = [ChannelId(1)];
		static ALARMS: [Alarm<'static>; 1] = [Alarm {
			channels: &WATCHED,
			page: PageId(2),
			rule: Rule::Threshold {
				trip: 10.0,
				release: 8.0,
				direction: Direction::Above,
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
	fn a_channel_without_a_chart_page_has_no_chart() {
		assert_eq!(PLAN.chart(0), None);
		assert_eq!(PLAN.chart(7), None);
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
				unit: 0x7E1,
				..channel(0, 8, false, true, 1.0, 0.0)
			},
			channel(8, 8, false, true, 1.0, 0.0),
		];
		static CELLS: [u16; 2] = [0, 2];
		static PAGES: [Page; 1] = [Page::Values { title: "", cells: &CELLS }];
		static PLAN: Plan = Plan {
			vin: "VIN",
			language: "en",
			units: &UNITS,
			channels: &CHANNELS,
			pages: &PAGES,
			alarms: &[],
		};
		let engine: std::vec::Vec<u16> = PLAN.channels_of(&UNITS[0]).map(|(i, _)| i).collect();
		assert_eq!(engine, [0, 2]);
		assert_eq!(PLAN.channel(1).map(|c| c.unit), Some(0x7E1));
		assert_eq!(PLAN.channel(9), None);
		assert_eq!(PLAN.unit_of(&CHANNELS[1]).map(|u| u.response), Some(0x7E9));
	}
}
