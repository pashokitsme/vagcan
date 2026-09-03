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

impl Plan {
	/// The channel a page cell or a configuration index refers to.
	pub fn channel(&self, index: u16) -> Option<&'static Channel> {
		self.channels.get(usize::from(index))
	}

	/// The unit a channel is read from.
	pub fn unit_of(&self, channel: &Channel) -> Option<&'static Unit> {
		self.units.iter().find(|u| u.request == channel.unit)
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

impl Channel {
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
		};
		let engine: std::vec::Vec<u16> = PLAN.channels_of(&UNITS[0]).map(|(i, _)| i).collect();
		assert_eq!(engine, [0, 2]);
		assert_eq!(PLAN.channel(1).map(|c| c.unit), Some(0x7E1));
		assert_eq!(PLAN.channel(9), None);
		assert_eq!(PLAN.unit_of(&CHANNELS[1]).map(|u| u.response), Some(0x7E9));
	}
}
