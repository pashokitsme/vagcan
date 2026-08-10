//! UDS measurement scaling: turn a `ReadDataByIdentifier` response's raw data
//! bytes into an engineering value + unit.
//!
//! ## Provenance — an engine-running capture, empirically
//! The scaling constants here are derived **empirically**, by pairing decoded
//! raw UDS with VCDS's own displayed values, from ENGINE-RUNNING captures of the
//! owner's Škoda Octavia 1.8 TSI (`research/dumps/`, all gitignored — USB traces
//! `capture-w-logs.pcapng` and `coolant-rpm-speed.pcapng` plus their VCDS ADVMB
//! logs `logs-engine.CSV` / `logs-dsg.CSV` / `coolant-rpm-speed.CSV`). The link
//! cipher is decoded per channel; each measurement DID's raw time-series is
//! aligned to a logged measurement by curve shape (cross-correlation) and fitted
//! by least squares. Tooling: `research/clb-crack/measure_{series,ttp,final}.py`
//! (first capture) and `measure_{coolant,fit,overlay,channels,probe}.py` (the
//! second, wide-rev capture).
//!
//! ## What is PROVEN (and shipped)
//! The **ignition-angle zero point**: DIDs [`IGNITION_ANGLE_ZERO_DIDS`] each
//! return raw `0x5555` (big-endian `u16`) for a displayed value of **0.00°**.
//! This is cross-validated four independent ways — the four DIDs read a constant
//! `0x5555` for the entire capture while the four constant ignition-angle
//! channels VCDS logged (`IDE00155/156/157/158`) read a constant `0.00°` over the
//! same window. It fixes the COMPU **zero point** of the ignition-angle method.
//!
//! ## What is NOT proven (deliberately not shipped — no forced fits)
//! - **The ignition-angle SLOPE.** The four proven DIDs are constant at `0x5555`
//!   for the whole session, so they pin the offset but carry no gradient. The one
//!   varying ignition-angle DID (`0xA051` ↔ `IDE00149`) shape-matches only loosely
//!   (best `|r| ≈ 0.86`, non-monotonic raw→° relation, `R² ≈ 0.73`) — not a clean
//!   linear fit, so no `(factor, offset)` is asserted for it.
//! - **RPM and vehicle speed.** No decodable DID in either engine-running capture
//!   tracks either with a proof-grade fit. This was re-tested with the exact
//!   capture the first pass prescribed — a **single ECU (Engine 01, `8V0 906 264 H`)
//!   polled through a wide, sustained rev (`IDE00405` = 784 → 3807 /min) with a
//!   tight ~1.4 s ADVMB log** (`research/dumps/coolant-rpm-speed.{pcapng,CSV}`,
//!   gitignored). The wide rev is present in the log, yet **no polled DID carries
//!   it**: at the single true capture→log lag (≈ 52 s, pinned by the drive-away
//!   window), RPM correlates with *nothing* (`|r| < 0.5` for every DID×form). The
//!   only decodable RDBI DIDs on the two TP-crib channels are
//!   `{7410,7419,7444,7450,7458,82D4,A03B,A0EF}` — the 2-byte ones (`A03B`≈`0x56xx`,
//!   `A0EF`≈`0x55xx`, `7458` idles at `0x55`) sit in the ignition-angle 0x5555 band
//!   and are near-constant or bidirectional, i.e. engine-internal angle/throttle
//!   signals, **not** the logged RPM/speed/coolant. High per-pair `|r|` shows up
//!   only at *inconsistent, per-measurement* lags (RPM's best fits scatter across
//!   lags 34–90 s) — the signature of spurious window-matching, not tracking. So
//!   the ADVMB display values are computed from raw the decodable channels do not
//!   expose; a further capture cannot settle this by rev range alone. See
//!   `research/labels/rod-labels.md §4` for the full negative.
//! - **Coolant temp.** Same capture: `IDE00025` rises 99 → 104 °C (slow, monotonic);
//!   the only slowly-drifting DID (`7450`) *falls* `0xDE → 0xC5` and anti-correlates
//!   (`r ≈ −0.66`), and the standard `raw·0.75 − 48` maps it to 118 → 99 °C (wrong
//!   direction and magnitude) — so `7450` is a different, cooling temperature, not
//!   the logged coolant. No clean fit.
//!
//! [`LinearScale`] + [`RawForm`] are the reusable runtime machinery (mirroring
//! the `MeasurementDef`/`Compu::Linear` model sketched in `research/labels/rod-labels.md
//! §5`); car-specific `(factor, offset)` rows drop in here as they are proven.

/// How to read an integer out of an RDBI response's data bytes (the bytes after
/// the `62 <DID hi> <DID lo>` echo). VAG measurements are 1- or 2-byte; both byte
/// orders occur, so the interpretation is part of each measurement's definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RawForm {
	/// First data byte as unsigned 8-bit.
	U8First,
	/// Second data byte as unsigned 8-bit.
	U8Second,
	/// Two data bytes, unsigned 16-bit big-endian (`data[0] << 8 | data[1]`).
	U16Be,
	/// Two data bytes, unsigned 16-bit little-endian (`data[1] << 8 | data[0]`).
	U16Le,
	/// Two data bytes, signed 16-bit big-endian.
	I16Be,
	/// Three data bytes, unsigned 24-bit big-endian. An odometer in kilometres
	/// needs more than 16 bits and does not warrant 32.
	U24Be,
	/// Four data bytes, unsigned 32-bit big-endian. Needed by counters that
	/// genuinely exceed 24 bits — the reference cluster's metre-resolution
	/// odometer reads 212 810 125, which 24 bits cannot hold.
	///
	/// The carrier is `i32`, so a value above [`i32::MAX`] reads as `None`
	/// rather than as a negative number. That ceiling is 2 147 483 647, i.e.
	/// 2.1 million kilometres for a metre counter — beyond any odometer — and
	/// refusing is the honest answer for anything that does exceed it.
	U32Be,
	/// A whole-byte integer **anywhere** in the response, in either byte order
	/// and either sign — the general case the seven variants above are the
	/// hand-proven special cases of.
	///
	/// The seven exist because they were written one at a time, as a drive
	/// proved each. Joined to a VW ODIS project that describes every field by
	/// `(bit offset, bit length, signed, byte order)`, that vocabulary turned
	/// out to be the binding constraint rather than the car: of 6 669 channels
	/// the reference car's fifteen units describe, only 1 691 could be said at
	/// all. 146 of the losses were a signed 16-bit **little**-endian value, 51 a
	/// 32-bit little-endian one, and the rest sat at a byte offset above 1 —
	/// none of them exotic, all of them unsayable.
	///
	/// Adding this variant rather than replacing the seven is deliberate:
	/// `measurements/*.json` rows are serialized by name (`"U16Be"`), they are
	/// proven by driving a car and nothing else can recreate them, and a
	/// rewrite of the vocabulary would make every one of those files
	/// unreadable. So the old names keep meaning exactly what they meant, and
	/// [`RawForm::for_field`] still answers with one of them wherever one fits —
	/// a row derived today compares equal to the row a drive wrote last year.
	Int {
		/// Bytes into the data, counted from the first byte after the DID echo.
		byte_offset: u8,
		/// How many bytes wide, 1 to 4. Wider than 4 has no `i32` to land in and
		/// is refused by [`RawForm::read`] rather than truncated.
		byte_length: u8,
		/// Whether the bytes are a two's-complement signed quantity.
		signed: bool,
		/// Whether the most significant byte comes first.
		big_endian: bool,
	},
	/// A field narrower than a byte, or one that does not start on a byte
	/// boundary — a one-bit flag, a 3-bit selector, a 5-bit counter.
	///
	/// 175,606 of the reference project's channels are this shape and 155,426 of
	/// them are single bits, so refusing them cost more channels than every other
	/// limitation of this vocabulary put together.
	///
	/// **Only within one byte.** A field that starts mid-byte and runs into the
	/// next needs a byte-order rule this project has no evidence for, and 97% of
	/// them do not, so [`RawForm::for_field`] refuses the rest rather than
	/// guessing at 3%.
	Bits {
		/// Bits into the data, counted **as ODX counts them**: `byte * 8 + bit`,
		/// where `bit` is the position of the field's *least significant* bit
		/// inside that byte, 0 being the LSB.
		///
		/// That is ASAM MCD-2D's definition of `BIT-POSITION`, and it is also
		/// what the reference car's own answers say. Two dumps of it, one parked
		/// and one driving (`research/dumps/survey-parked.jsonl` and
		/// `survey-driving-20260802-0322.jsonl`), settle the within-byte order
		/// twice over:
		///
		/// - `710/2A49` `engine_running` reads 0 parked and 1 driving counted
		///   from the LSB, and 0 both times counted from the MSB.
		/// - `7E0/20A4` carries eight flags named `cylinder 1` … `cylinder 8`.
		///   Counted from the LSB, cylinders 1–4 change between the two dumps and
		///   5–8 stay zero. Counted from the MSB it is the other way round — on
		///   an engine with four cylinders.
		bit_offset: u32,
		/// How many bits, 1 to 8.
		bit_length: u8,
		/// Whether the bits are a two's-complement signed quantity, sign bit at
		/// the top of the field.
		signed: bool,
	},
}

impl RawForm {
	/// Where in the response this form starts, in **bits** after the DID echo.
	///
	/// **This is what tells two channels of one identifier apart.** A `0x22`
	/// response carries as many fields as the unit chose to put in it — the
	/// reference car's fifteen units describe 3,963 sayable fields across 2,011
	/// identifiers — and everything that keyed a channel by its identifier alone
	/// could hold one of them and dropped the rest. Bits rather than bytes
	/// because a byte can hold eight flags, and eight channels that shared a key
	/// would be the same loss one level down.
	pub fn bit_offset(self) -> u32 {
		match self {
			RawForm::U8First | RawForm::U16Be | RawForm::U16Le | RawForm::I16Be | RawForm::U24Be | RawForm::U32Be => 0,
			RawForm::U8Second => 8,
			RawForm::Int { byte_offset, .. } => u32::from(byte_offset) * 8,
			RawForm::Bits { bit_offset, .. } => bit_offset,
		}
	}

	/// Extract the raw integer from `data` (the response bytes after the DID
	/// echo). Returns `None` if `data` is too short for this form.
	pub fn read(self, data: &[u8]) -> Option<i32> {
		match self {
			RawForm::U8First => data.first().map(|&b| b as i32),
			RawForm::U8Second => data.get(1).map(|&b| b as i32),
			RawForm::U16Be => match data {
				[hi, lo, ..] => Some(((*hi as i32) << 8) | *lo as i32),
				_ => None,
			},
			RawForm::U24Be => match data {
				[hi, mid, lo, ..] => Some(((*hi as i32) << 16) | ((*mid as i32) << 8) | *lo as i32),
				_ => None,
			},
			RawForm::U16Le => match data {
				[lo, hi, ..] => Some(((*hi as i32) << 8) | *lo as i32),
				_ => None,
			},
			RawForm::I16Be => match data {
				[hi, lo, ..] => Some((((*hi as u16) << 8 | *lo as u16) as i16) as i32),
				_ => None,
			},
			RawForm::U32Be => match data {
				[a, b, c, d, ..] => i32::try_from(u32::from_be_bytes([*a, *b, *c, *d])).ok(),
				_ => None,
			},
			RawForm::Bits {
				bit_offset,
				bit_length,
				signed,
			} => {
				// Within one byte by construction — `for_field` refuses anything
				// that crosses — so this is a shift and a mask, and the sign
				// extension is from the field's own width.
				if bit_length == 0 || bit_length > 8 {
					return None;
				}
				let byte = *data.get((bit_offset / 8) as usize)?;
				let shift = bit_offset % 8;
				if u32::from(bit_length) + shift > 8 {
					return None;
				}
				let mask = if bit_length == 8 { 0xFFu32 } else { (1u32 << bit_length) - 1 };
				let raw = (u32::from(byte) >> shift) & mask;
				Some(match signed && bit_length > 1 && raw >> (bit_length - 1) != 0 {
					true => raw as i32 - (1i32 << bit_length),
					false => raw as i32,
				})
			}
			RawForm::Int {
				byte_offset,
				byte_length,
				signed,
				big_endian,
			} => {
				// A width the carrier cannot hold is refused, not clipped: four
				// bytes is what an `i32` has room for, and a zero-byte field is
				// not a value at all.
				if byte_length == 0 || byte_length as usize > 4 {
					return None;
				}
				let start = byte_offset as usize;
				let field = data.get(start..start + byte_length as usize)?;
				// Sign-extend from the field's own width, so a one-byte −2 is −2
				// and not 254. Unsigned stays unsigned, and a four-byte unsigned
				// value above `i32::MAX` has no honest answer — the same rule
				// `U32Be` has kept since the cluster's metre odometer needed it.
				let mut wide: u32 = 0;
				match big_endian {
					true => field.iter().for_each(|&b| wide = (wide << 8) | b as u32),
					false => field.iter().rev().for_each(|&b| wide = (wide << 8) | b as u32),
				}
				match signed {
					true => {
						let shift = 32 - 8 * byte_length as u32;
						Some(((wide << shift) as i32) >> shift)
					}
					false => i32::try_from(wide).ok(),
				}
			}
		}
	}

	/// The one form that says a field described as `(bit offset, bit length,
	/// signed, byte order)` — a VW ODIS project's own terms — or `None` when
	/// this vocabulary cannot say it exactly.
	///
	/// **One answer per shape, and the old names win where they fit.** A shape
	/// one of the seven original variants names comes back as that variant, so
	/// a row derived from a project file compares equal to the row a drive
	/// proved and wrote to disk; everything else comes back as [`RawForm::Int`].
	/// Two spellings of one shape would make `raw_form == RawForm::U16Le` a
	/// coin toss for every caller that asks it.
	///
	/// `None` is the honest answer for a field that does not start on a byte or
	/// does not fill whole bytes — a one-bit flag, a 3-bit field at bit 19.
	/// Approximating one produces a confident wrong number, which is worse than
	/// the raw bytes a reader gets instead.
	pub fn for_field(bit_offset: u32, bit_length: u32, signed: bool, big_endian: bool) -> Option<RawForm> {
		if bit_offset % 8 != 0 || bit_length % 8 != 0 {
			// A field inside one byte. Anything crossing a boundary needs a rule
			// for which end the bits continue from, and this project has no
			// evidence for one — 97% of them do not cross, so the rest are
			// refused rather than guessed at.
			if bit_length == 0 || bit_length > 8 || (bit_offset % 8) + bit_length > 8 {
				return None;
			}
			return Some(RawForm::Bits {
				bit_offset,
				bit_length: bit_length as u8,
				signed,
			});
		}
		let (byte_offset, byte_length) = (bit_offset / 8, bit_length / 8);
		if byte_length == 0 || byte_length > 4 {
			return None;
		}
		// The shapes that already had a name keep it, so nothing that reads a
		// `measurements/` file has to learn a second spelling of them. Byte
		// order is immaterial to a single byte, which is why the `U8` arms
		// ignore it.
		Some(match (byte_offset, byte_length, signed, big_endian) {
			(0, 1, false, _) => RawForm::U8First,
			(1, 1, false, _) => RawForm::U8Second,
			(0, 2, false, true) => RawForm::U16Be,
			(0, 2, false, false) => RawForm::U16Le,
			(0, 2, true, true) => RawForm::I16Be,
			(0, 3, false, true) => RawForm::U24Be,
			(0, 4, false, true) => RawForm::U32Be,
			_ => RawForm::Int {
				byte_offset: u8::try_from(byte_offset).ok()?,
				byte_length: byte_length as u8,
				signed,
				big_endian,
			},
		})
	}
}

/// A linear COMPU-METHOD: `engineering = raw * factor + offset`. This is the VAG
/// default (RPM ≈ raw·0.25, speed ≈ raw·0.01, temp ≈ raw·0.75 − 48, …). Non-linear
/// / table methods are not modelled yet (none is proven for this car).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LinearScale {
	/// Multiplier applied to the raw integer.
	pub factor: f64,
	/// Additive offset.
	pub offset: f64,
}

impl LinearScale {
	/// Apply the scaling to a raw integer.
	pub fn apply(self, raw: i32) -> f64 {
		raw as f64 * self.factor + self.offset
	}

	/// Read `data` per `form` and apply the scaling. `None` if `data` is too
	/// short for `form`.
	pub fn apply_bytes(self, form: RawForm, data: &[u8]) -> Option<f64> {
		form.read(data).map(|r| self.apply(r))
	}
}

/// Unit string of the ignition-angle measurements (degrees crank).
pub const IGNITION_ANGLE_UNIT: &str = "°";

/// The raw value (read as [`RawForm::U16Be`]) that the ignition-angle DIDs return
/// for a displayed **0.00°**. Cross-validated against VCDS's ADVMB log.
pub const IGNITION_ANGLE_ZERO_RAW: u16 = 0x5555;

/// Engine-ECU RDBI DIDs proven to belong to the ignition-angle family (unit
/// `°`), each observed returning [`IGNITION_ANGLE_ZERO_RAW`] = **0.00°** for the
/// whole engine-running capture. They match the four constant ignition-angle
/// channels VCDS logged (`IDE00155/156/157/158`); the exact one-to-one DID↔IDE
/// pairing is not individually determined (all four are constant `0.00°`), so
/// only set membership + the zero point are asserted.
pub const IGNITION_ANGLE_ZERO_DIDS: &[u16] = &[0xA058, 0xA059, 0xA05E, 0xA05F];

#[cfg(test)]
mod u24_tests {
	use super::*;

	#[test]
	fn a_24_bit_reading_recovers_the_cars_odometer_exactly() {
		// The instrument cluster answered 0x03 0x3F 0x18 while a VCDS log
		// recorded 212760 km at the same moment. An exact hit on a six-figure
		// value is not something a wrong byte order or width reaches.
		assert_eq!(RawForm::U24Be.read(&[0x03, 0x3F, 0x18]), Some(212_760));
		// Trailing bytes are ignored, missing ones are not invented.
		assert_eq!(RawForm::U24Be.read(&[0x03, 0x3F, 0x18, 0xFF]), Some(212_760));
		assert_eq!(RawForm::U24Be.read(&[0x03, 0x3F]), None);
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn raw_form_reads_each_interpretation() {
		let d = [0x57u8, 0xE9];
		assert_eq!(RawForm::U8First.read(&d), Some(0x57));
		assert_eq!(RawForm::U8Second.read(&d), Some(0xE9));
		assert_eq!(RawForm::U16Be.read(&d), Some(0x57E9));
		assert_eq!(RawForm::U16Le.read(&d), Some(0xE957));
		assert_eq!(RawForm::I16Be.read(&[0xFF, 0xFE]), Some(-2));
		assert_eq!(RawForm::U16Be.read(&[0x01]), None);
		assert_eq!(RawForm::U8Second.read(&[0x01]), None);
	}

	#[test]
	fn a_thirty_two_bit_form_reads_wide_counters_and_refuses_to_go_negative() {
		// The reference cluster's metre odometer: 212 810 125 m. Read as 24
		// bits it would be 11 483 021, so the width is not cosmetic.
		assert_eq!(RawForm::U32Be.read(&[0x0C, 0xAF, 0x39, 0x8D]), Some(212_810_125));
		assert_eq!(RawForm::U24Be.read(&[0x0C, 0xAF, 0x39, 0x8D]), Some(0x0CAF39));
		assert_eq!(RawForm::U32Be.read(&[0x7F, 0xFF, 0xFF, 0xFF]), Some(i32::MAX));
		// Above i32::MAX there is no honest answer, so there is no answer —
		// never a value that has silently wrapped to negative.
		assert_eq!(RawForm::U32Be.read(&[0x80, 0x00, 0x00, 0x00]), None);
		assert_eq!(RawForm::U32Be.read(&[0x01, 0x02, 0x03]), None);
	}

	#[test]
	fn a_general_integer_reads_a_field_anywhere_in_the_response() {
		let word = |signed, big_endian| RawForm::Int {
			byte_offset: 0,
			byte_length: 2,
			signed,
			big_endian,
		};
		let quad = |signed, big_endian| RawForm::Int {
			byte_offset: 0,
			byte_length: 4,
			signed,
			big_endian,
		};
		// A signed 16-bit LITTLE-endian value — the one shape whose absence cost
		// 146 channels on the reference car's gearbox. Bytes 0x30 0xFF little-end
		// first are 0xFF30, which as i16 is -208. Read big-endian they would be
		// 0x30FF = +12543: a wrong byte order here is silently wrong, and wrong in
		// exactly one direction.
		assert_eq!(word(true, false).read(&[0x30, 0xFF]), Some(-208));
		assert_eq!(word(true, true).read(&[0x30, 0xFF]), Some(12543));
		// The proven little-endian register, expressed the general way: the
		// gearbox's 0x380A read 690 /min from these bytes, 45570 read the other
		// way round (`research/labels/rod-labels.md:433`).
		assert_eq!(word(false, false).read(&[0xB2, 0x02]), Some(690));
		assert_eq!(RawForm::U16Le.read(&[0xB2, 0x02]), Some(690));

		// A 32-bit LITTLE-endian counter: the reference cluster's metre odometer
		// with its bytes the other way round.
		assert_eq!(quad(false, false).read(&[0x8D, 0x39, 0xAF, 0x0C]), Some(212_810_125));
		assert_eq!(quad(false, false).read(&[0xFF, 0xFF, 0xFF, 0x7F]), Some(i32::MAX));
		// Above i32::MAX there is no honest answer, exactly as for `U32Be` — never
		// a value that has silently wrapped negative.
		assert_eq!(quad(false, false).read(&[0x00, 0x00, 0x00, 0x80]), None);
		// Signed, the same four bytes are a number the carrier does hold.
		assert_eq!(quad(true, false).read(&[0x00, 0x00, 0x00, 0x80]), Some(i32::MIN));
	}

	#[test]
	fn a_field_past_the_second_byte_is_read_where_it_actually_is() {
		use RawForm::Int;
		// Everything the old vocabulary could say lived at byte 0 or byte 1, so a
		// response carrying several fields could only ever yield its first. These
		// six bytes hold three 16-bit values; the third is not reachable by any
		// of the seven original forms.
		let data = [0x00, 0x01, 0x02, 0x03, 0x12, 0x34];
		let at = |byte_offset| {
			Int {
				byte_offset,
				byte_length: 2,
				signed: false,
				big_endian: true,
			}
			.read(&data)
		};
		assert_eq!(at(0), Some(0x0001));
		assert_eq!(at(2), Some(0x0203));
		assert_eq!(at(4), Some(0x1234));
		// A single byte deep into the response, and a signed one.
		assert_eq!(
			Int {
				byte_offset: 5,
				byte_length: 1,
				signed: false,
				big_endian: true
			}
			.read(&data),
			Some(0x34)
		);
		assert_eq!(
			Int {
				byte_offset: 3,
				byte_length: 1,
				signed: true,
				big_endian: true
			}
			.read(&[0x00, 0x00, 0x00, 0xFE]),
			Some(-2)
		);
		// A field that runs off the end is not invented, and neither is one whose
		// offset is past the end entirely.
		assert_eq!(at(5), None);
		assert_eq!(at(6), None);
		// A width this carrier cannot hold is refused rather than truncated: an
		// `i32` has no room for five bytes, and a zero-byte field is not a value.
		assert_eq!(
			Int {
				byte_offset: 0,
				byte_length: 5,
				signed: false,
				big_endian: true
			}
			.read(&[1, 2, 3, 4, 5, 6]),
			None
		);
		assert_eq!(
			Int {
				byte_offset: 0,
				byte_length: 0,
				signed: false,
				big_endian: true
			}
			.read(&data),
			None
		);
	}

	#[test]
	fn a_described_field_gets_the_one_form_that_says_it() {
		// The seven original variants stay canonical for the shapes they name, so
		// a row written before this widening still compares equal to a row
		// derived now. Everything else becomes the general form.
		let f = RawForm::for_field;
		assert_eq!(f(0, 8, false, true), Some(RawForm::U8First));
		assert_eq!(f(8, 8, false, false), Some(RawForm::U8Second));
		assert_eq!(f(0, 16, false, true), Some(RawForm::U16Be));
		assert_eq!(f(0, 16, false, false), Some(RawForm::U16Le));
		assert_eq!(f(0, 16, true, true), Some(RawForm::I16Be));
		assert_eq!(f(0, 24, false, true), Some(RawForm::U24Be));
		assert_eq!(f(0, 32, false, true), Some(RawForm::U32Be));
		// The three shapes the reference car's channel count was losing.
		assert_eq!(
			f(0, 16, true, false),
			Some(RawForm::Int {
				byte_offset: 0,
				byte_length: 2,
				signed: true,
				big_endian: false
			})
		);
		assert_eq!(
			f(0, 32, false, false),
			Some(RawForm::Int {
				byte_offset: 0,
				byte_length: 4,
				signed: false,
				big_endian: false
			})
		);
		assert_eq!(
			f(16, 16, false, true),
			Some(RawForm::Int {
				byte_offset: 2,
				byte_length: 2,
				signed: false,
				big_endian: true
			})
		);
		// A field inside one byte is a bit field, wherever in the byte it sits.
		assert_eq!(
			f(19, 3, false, true),
			Some(RawForm::Bits {
				bit_offset: 19,
				bit_length: 3,
				signed: false
			})
		);
		assert_eq!(
			f(8, 1, false, true),
			Some(RawForm::Bits {
				bit_offset: 8,
				bit_length: 1,
				signed: false
			})
		);
		// A field that crosses a byte boundary needs a rule for which end the
		// bits continue from, and nothing here has evidence for one.
		assert_eq!(f(0, 12, false, true), None);
		assert_eq!(f(6, 4, false, true), None);
		// And a field too wide for the carrier is refused rather than clipped.
		assert_eq!(f(0, 64, false, true), None);
	}

	#[test]
	fn a_bit_field_is_counted_from_the_least_significant_bit() {
		// The direction is not a convention this project picked. It is ASAM
		// MCD-2D's, and the reference car's own answers agree twice over — see
		// the note on `RawForm::Bits`, which names the two dumps and the two
		// channels that settle it.
		//
		// `0b1010_0110` at byte 1 of the response.
		let data = [0x00, 0xA6];
		let bit = |offset, length| {
			RawForm::Bits {
				bit_offset: offset,
				bit_length: length,
				signed: false,
			}
			.read(&data)
		};
		assert_eq!(bit(8, 1), Some(0), "bit 0 is the low end of the byte");
		assert_eq!(bit(9, 1), Some(1));
		assert_eq!(bit(10, 1), Some(1));
		assert_eq!(bit(15, 1), Some(1), "and bit 7 is the high end");
		// Wider fields read from the same end.
		assert_eq!(bit(8, 4), Some(0b0110));
		assert_eq!(bit(12, 4), Some(0b1010));

		// Signed, sign bit at the top of the field's own width. Bits 5..7 of
		// `0b1010_0110` are `0b101`, which in three bits is −3 and not 5.
		let signed = RawForm::Bits {
			bit_offset: 13,
			bit_length: 3,
			signed: true,
		};
		assert_eq!(signed.read(&data), Some(-3));
		// The same bits unsigned stay 5, so the sign is the definition's and
		// never the value's.
		assert_eq!(bit(13, 3), Some(5));

		// A response too short for the byte the field is in has no value in it,
		// rather than a zero that looks like a reading.
		assert_eq!(bit(24, 1), None);
	}

	#[test]
	fn linear_scale_arithmetic() {
		// The machinery itself: a textbook VAG coolant-temp style scale.
		let temp = LinearScale { factor: 0.75, offset: -48.0 };
		assert!((temp.apply(0x80) - 48.0).abs() < 1e-9); // 128*0.75-48 = 48.0
		// And through raw bytes (single-byte form).
		assert!((temp.apply_bytes(RawForm::U8First, &[0x80]).unwrap() - 48.0).abs() < 1e-9);
	}

	#[test]
	fn ignition_zero_point_matches_capture_and_log() {
		// Captured raw data bytes (after the `62 A0 xx` echo) for every ignition-
		// angle-family DID, for the whole engine-running session, are these two
		// literal bytes; VCDS displayed 0.00° for the matching logged channels.
		// (Bytes/values are the crib, not the gitignored capture itself.)
		let captured_raw: [u8; 2] = [0x55, 0x55];
		let vcds_displayed_deg = 0.00_f64;

		let raw = RawForm::U16Be.read(&captured_raw).unwrap();
		assert_eq!(raw as u16, IGNITION_ANGLE_ZERO_RAW);

		// The proven COMPU zero point: this raw maps to 0.00° for each DID. The
		// slope is unproven, but ANY linear scale through this zero point (i.e.
		// offset = -factor*0x5555) reproduces the displayed value here.
		for &did in IGNITION_ANGLE_ZERO_DIDS {
			assert!((0xA058..=0xA05F).contains(&did));
			let factor = 0.01; // arbitrary; the zero point is what is asserted
			let scale = LinearScale {
				factor,
				offset: -factor * IGNITION_ANGLE_ZERO_RAW as f64,
			};
			let got = scale.apply(raw);
			assert!(
				(got - vcds_displayed_deg).abs() < 1e-6,
				"DID {did:#06X}: raw {raw:#06X} -> {got}, expected {vcds_displayed_deg}"
			);
		}
	}
}
