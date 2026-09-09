//! Fault codes: the data object property that lists them, and each code's
//! identity and text.
//!
//! Two types, and they join the way the measurement chain does
//! (`research/odis-dtc/README.md`):
//!
//! ```text
//! DB_LAYER_DATA                 the variant's index; `dtc_properties` names
//!  └ DB_DOP_DTC                 the DOP, found through `properties` by that name
//!     └ MCD_DB_DIAG_TROUBLE_CODE  one per code: number, display code, text
//! ```
//!
//! Every loader here is a literal transcription of a field order, read
//! against the reference project's 291,346 trouble-code objects and 696 DOPs
//! — every one of the former is 34 bytes, and every one of the latter ends on
//! its terminator under the order below. No terminator is consumed here; see
//! [`super::load`].
//!
//! ## What the number is
//! [`TroubleCode::code`] is the 24-bit number the control unit sends in a
//! `0x19` response, read big-endian as one integer — the same number
//! `vagcan faults` prints in decimal beside the hex. It is **not** the SAE
//! code: on the reference project only 1,515 of 43,378 `(display code, number)`
//! pairs agree with the SAE encoding, and `DTC_17154` displays as `P150B00`.
//! The display code is a separate string the object carries, and the join
//! from the car to the file is on the number alone.

use super::super::Error;
use super::super::compu;
use super::super::object::Stream;
use super::measurement::{self, CodedType, PhysicalType};
use super::{Ref, code, nested, reference};

/// `MCD_DB_DIAG_TROUBLE_CODE`: one fault code, as a variant's fault memory
/// reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TroubleCode {
	/// The ODX `ID` of the DTC element — a supplier's own name for the check,
	/// `RB_DFC_VLCAvl_aVeh_VW.17154`. Absent on the body electronics.
	pub id: Option<String>,
	/// `DTC_<number>` — the short name, generated from the number.
	pub short_name: Option<String>,
	/// What a tester prints beside the text: an SAE-style code with the
	/// failure type appended, `P150B00`, `B12DF29`. A string, not derived.
	pub display_code: Option<String>,
	/// The text, in whichever language the supplier wrote it.
	pub text: Option<String>,
	/// `LEVEL` in ODX terms. 1 to 9 on the reference project; recorded, not
	/// interpreted.
	pub level: u32,
	/// The number the control unit sends — see the module note.
	pub code: u32,
	/// `IS-TEMPORARY`. False on every object of the reference project.
	pub temporary: bool,
	/// The text's identifier — the join key a translated text table would be
	/// keyed by. Equal to the display code on 291,290 of 291,346 objects;
	/// `TXE<display code>` on a few supplier libraries, absent on dummy codes.
	pub text_id: Option<String>,
}

/// `DB_DOP_DTC`: a variant's fault-code table.
#[derive(Debug, Clone, PartialEq)]
pub struct DtcDop {
	/// `(number, reference)` for every code, in file order.
	pub codes: Vec<(u32, Ref)>,
	/// Where the number sits in the response — 24 bits, unsigned,
	/// most-significant first, on every DOP of the reference project.
	pub coded: Option<CodedType>,
	/// What the number becomes: an unsigned integer, displayed in base 16 or 10.
	pub physical: Option<PhysicalType>,
	/// The short name, `DTCDOP_VAGUDS`, which is also the key a layer's
	/// `dtc_properties` and `properties` index it under.
	pub short_name: Option<String>,
	/// The human name, `VAG UDS`.
	pub long_name: Option<String>,
}

/// Read an `MCD_DB_DIAG_TROUBLE_CODE`.
pub fn trouble_code(stream: &mut Stream<'_>) -> Result<TroubleCode, Error> {
	let id = stream.ascii()?.map(str::to_owned);
	let short_name = stream.ascii()?.map(str::to_owned);
	let display_code = stream.ascii()?.map(str::to_owned);
	let text = stream.unicode()?.map(str::to_owned);
	let level = stream.u32()?;
	let code = stream.u32()?;
	let temporary = stream.flag()?;
	let text_id = stream.ascii()?.map(str::to_owned);
	Ok(TroubleCode {
		id,
		short_name,
		display_code,
		text,
		level,
		code,
		temporary,
		text_id,
	})
}

/// Read a `DB_DOP_DTC`.
///
/// A second, always-empty collection follows the code map. Its element shape
/// is unknown because no file has ever filled it; one that does is refused
/// rather than read at a guessed width.
pub fn dtc_dop(stream: &mut Stream<'_>) -> Result<DtcDop, Error> {
	let count = stream.count()?;
	let mut codes = Vec::with_capacity(count.min(4096));
	for _ in 0..count {
		let number = stream.u32()?;
		codes.push((number, reference(stream, false, false)?));
	}
	if stream.count()? != 0 {
		return Err(Error::Format(
			"a fault-code property carries a second collection, a shape this reader has never seen".into(),
		));
	}
	let _compu = nested(stream, code::DB_COMPU_METHOD, compu::method)?;
	let coded = nested(stream, code::DB_DIAG_CODED_TYPE, measurement::coded_type)?;
	let physical = nested(stream, code::DB_PHYSICAL_TYPE, measurement::physical_type)?;
	let short_name = stream.ascii()?.map(str::to_owned);
	let long_name = stream.unicode()?.map(str::to_owned);
	let _description = stream.unicode()?;
	let _reserved = stream.ascii()?;
	let _long_name_id = stream.ascii()?;
	let _description_id = stream.ascii()?;
	Ok(DtcDop {
		codes,
		coded,
		physical,
		short_name,
		long_name,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::odis::hash;
	use crate::odis::object::{END, Stream};
	use crate::odis::strings::{Pool, Strings};

	/// The same byte-builder the other loader modules' tests use, kept local
	/// so each module's fixtures read as the type they are testing.
	#[derive(Default)]
	struct Bytes {
		out: Vec<u8>,
		ascii: Vec<String>,
		unicode: Vec<String>,
	}

	impl Bytes {
		fn u8(&mut self, v: u8) -> &mut Self {
			self.out.push(v);
			self
		}
		fn u16(&mut self, v: u16) -> &mut Self {
			self.out.extend_from_slice(&v.to_le_bytes());
			self
		}
		fn u32(&mut self, v: u32) -> &mut Self {
			self.out.extend_from_slice(&v.to_le_bytes());
			self
		}
		fn a(&mut self, s: Option<&str>) -> &mut Self {
			match s {
				None => self.u32(0),
				Some(s) => {
					self.ascii.push(s.to_owned());
					self.u32(hash::of_bytes(s.as_bytes()))
				}
			}
		}
		fn u(&mut self, s: Option<&str>) -> &mut Self {
			match s {
				None => self.u32(0),
				Some(s) => {
					self.unicode.push(s.to_owned());
					let units: Vec<u16> = s.encode_utf16().collect();
					self.u32(hash::of_utf16(&units))
				}
			}
		}
		fn some(&mut self, type_code: u16) -> &mut Self {
			self.u8(1).u16(type_code)
		}
		fn done(mut self, type_code: u16) -> (Vec<u8>, Strings) {
			self.out.extend_from_slice(&END);
			let mut body = type_code.to_le_bytes().to_vec();
			body.extend_from_slice(&self.out);
			let mut a = Vec::new();
			for s in &self.ascii {
				a.extend_from_slice(&(s.len() as u32).to_le_bytes());
				a.extend_from_slice(s.as_bytes());
			}
			let mut u = Vec::new();
			for s in &self.unicode {
				let units: Vec<u16> = s.encode_utf16().collect();
				u.extend_from_slice(&(units.len() as u32).to_le_bytes());
				for unit in units {
					u.extend_from_slice(&unit.to_le_bytes());
				}
			}
			(
				body,
				Strings {
					ascii: Pool::parse_ascii(&a).expect("a synthesised pool parses"),
					unicode: Pool::parse_utf16(&u).expect("a synthesised pool parses"),
				},
			)
		}
	}

	/// The 34-byte shape every one of the reference project's 291,346
	/// trouble-code objects has, transcribed from `DTC_17154` of the engine
	/// pool: an ODX id, `DTC_<n>`, the display code, the text, a level of 2,
	/// the number 17154, not temporary, and a text id equal to the display code.
	#[test]
	fn a_trouble_code_reads_its_number_display_code_and_text() {
		let mut b = Bytes::default();
		b.a(Some("RB_DFC_VLCAvl_aVeh_VW.17154"))
			.a(Some("DTC_17154"))
			.a(Some("P150B00"))
			.u(Some("Acceleration monitoring\nControl limit exceeded"))
			.u32(2)
			.u32(17154)
			.u8(0)
			.a(Some("P150B00"));
		let (body, strings) = b.done(code::MCD_DB_DIAG_TROUBLE_CODE);
		assert_eq!(body.len(), 34, "the real objects are 34 bytes, terminator included");

		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let dtc = trouble_code(&mut stream).expect("a trouble code parses");
		stream.end().expect("the loader lands on the terminator");
		assert_eq!(
			dtc,
			TroubleCode {
				id: Some("RB_DFC_VLCAvl_aVeh_VW.17154".into()),
				short_name: Some("DTC_17154".into()),
				display_code: Some("P150B00".into()),
				text: Some("Acceleration monitoring\nControl limit exceeded".into()),
				level: 2,
				code: 17154,
				temporary: false,
				text_id: Some("P150B00".into()),
			}
		);
	}

	/// The body control module's codes carry no ODX id, and the number is the
	/// one the unit sends: `DTC_531` is the `00 02 13` the reference car's
	/// body control module stored, which VCDS prints as `0531`.
	#[test]
	fn a_trouble_code_without_an_id_still_names_itself() {
		let mut b = Bytes::default();
		b.a(None)
			.a(Some("DTC_531"))
			.a(Some("B10A600"))
			.u(Some("Fußraumleuchte"))
			.u32(6)
			.u32(0x000213)
			.u8(0)
			.a(Some("B10A600"));
		let (body, strings) = b.done(code::MCD_DB_DIAG_TROUBLE_CODE);
		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let dtc = trouble_code(&mut stream).expect("a trouble code parses");
		stream.end().expect("the loader lands on the terminator");
		assert_eq!(dtc.id, None);
		assert_eq!(dtc.code, 531);
		assert_eq!(dtc.level, 6);
		assert_eq!(dtc.text.as_deref(), Some("Fußraumleuchte"));
	}

	/// The trailing byte is a flag. A value that is neither 0 nor 1 there is
	/// the first byte of something read at the wrong offset, and is refused.
	#[test]
	fn a_trouble_code_with_a_broken_flag_is_refused() {
		let mut b = Bytes::default();
		b.a(None).a(Some("DTC_1")).a(None).u(None).u32(1).u32(1).u8(7).a(None);
		let (body, strings) = b.done(code::MCD_DB_DIAG_TROUBLE_CODE);
		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let err = trouble_code(&mut stream).expect_err("a flag of 7 is a misparse");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	/// The compu method, coded type and physical type every DOP of the
	/// reference project carries: identical, 24 unsigned bits high-low, an
	/// unsigned integer shown in base 16.
	fn dop_types(b: &mut Bytes) {
		b.some(code::DB_COMPU_METHOD).u8(0).u8(0).u8(0);
		b.some(code::DB_DIAG_CODED_TYPE)
			.u8(2) // eSTANDARD_LENGTH_TYPE
			.u32(24)
			.u8(0) // no bit mask
			.u8(1) // eDB_UINT32
			.u8(11) // eNONE encoding
			.u8(1) // high-low byte order
			.u8(0); // not condensed
		b.some(code::DB_PHYSICAL_TYPE).u8(1).u8(0).u8(16);
	}

	/// Transcribed from the smallest DOP of the engine pool, 70 bytes: one
	/// code, then the empty second collection, the three types, and six names
	/// of which the last four are absent.
	#[test]
	fn a_dtc_dop_lists_its_codes_by_number() {
		let mut b = Bytes::default();
		b.u16(2);
		b.u32(5524).a(Some("DTC_EV_Test_DTCDOP_VAGUDS.DTC_5524")).a(Some("0.0.0@BV_Test.bv"));
		b.u32(17154).a(Some("DTC_EV_Test_DTCDOP_VAGUDS.DTC_17154")).a(None);
		b.u16(0);
		dop_types(&mut b);
		b.a(Some("DTCDOP_VAGUDS")).u(Some("VAG UDS")).u(None).a(None).a(None).a(None);
		let (body, strings) = b.done(code::DB_DOP_DTC);

		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let dop = dtc_dop(&mut stream).expect("a fault-code property parses");
		stream.end().expect("the loader lands on the terminator");
		assert_eq!(dop.short_name.as_deref(), Some("DTCDOP_VAGUDS"));
		assert_eq!(dop.long_name.as_deref(), Some("VAG UDS"));
		assert_eq!(dop.codes.len(), 2);
		assert_eq!(dop.codes[0].0, 5524);
		assert_eq!(dop.codes[0].1.object.as_deref(), Some("DTC_EV_Test_DTCDOP_VAGUDS.DTC_5524"));
		assert_eq!(dop.codes[0].1.pool.as_deref(), Some("0.0.0@BV_Test.bv"));
		assert_eq!(dop.codes[1].0, 17154);
		assert_eq!(dop.codes[1].1.pool, None, "a reference may omit its pool");
		let coded = dop.coded.expect("the coded type is there");
		assert_eq!(coded.bits, Some(24));
		assert!(coded.high_low_byte_order);
		assert_eq!(dop.physical.expect("the physical type is there").radix, 16);
	}

	/// The second collection has never been seen non-empty, so its element
	/// shape is unknown; a file that fills it is refused, not guessed at.
	#[test]
	fn a_dtc_dop_with_the_unknown_second_collection_is_refused() {
		let mut b = Bytes::default();
		b.u16(0).u16(1).u32(0).u32(0).u32(0);
		let (body, strings) = b.done(code::DB_DOP_DTC);
		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let err = dtc_dop(&mut stream).expect_err("an unknown collection must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	/// Both types dispatch through `load`, which is what makes them reachable
	/// from a pool walk; before this module they fell to `Unsupported`.
	#[test]
	fn both_types_dispatch_through_load() {
		let mut b = Bytes::default();
		b.a(None).a(Some("DTC_7")).a(None).u(None).u32(1).u32(7).u8(0).a(None);
		let (body, strings) = b.done(code::MCD_DB_DIAG_TROUBLE_CODE);
		let (type_code, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		match super::super::load(type_code, &mut stream).expect("a trouble code loads") {
			super::super::Outcome::Object(super::super::Object::TroubleCode(dtc)) => assert_eq!(dtc.code, 7),
			other => panic!("a trouble code came back as {other:?}"),
		}
		assert_eq!(stream.remaining(), 0, "load consumes the terminator");

		let mut b = Bytes::default();
		b.u16(0).u16(0);
		dop_types(&mut b);
		b.a(Some("DTCDOP_X")).u(None).u(None).a(None).a(None).a(None);
		let (body, strings) = b.done(code::DB_DOP_DTC);
		let (type_code, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		match super::super::load(type_code, &mut stream).expect("a fault-code property loads") {
			super::super::Outcome::Object(super::super::Object::DtcDop(dop)) => assert_eq!(dop.short_name.as_deref(), Some("DTCDOP_X")),
			other => panic!("a fault-code property came back as {other:?}"),
		}
		assert_eq!(stream.remaining(), 0, "load consumes the terminator");
	}
}
