//! The one multiplexer this reader reads: a variant's description of its fault
//! records — the extended data a `19 06` answer carries (`research/odis-dtc/freeze-frames.md`).
//!
//! ```text
//! MCD_DB_PARAMETER_MULTIPLEXER  "MUX_DTCExtenDataRecor", by short name through the layers
//!  ├ u32                         the case structures' byte position in the multiplexer
//!  ├ 0x004C switch key           bit, byte, the key's DOP (UINTHEX1Byte: the record number)
//!  ├ DB_CASES                    u32 count, then each:
//!  │  └ DB_CASE                  lower DB_LIMIT, upper DB_LIMIT, names, the structure
//!  └ DB_DEFAULT_CASE             names, the structure for any other record number
//! ```
//!
//! **Why this is not on the refusal list's wrong side.** `DB_CASE`, `DB_CASES` and
//! `DB_DEFAULT_CASE` stay refused for every other caller: [`super::load`] still answers
//! `Refused` for them and [`super::nested_any`] still errors on one, so a multiplexed
//! *measurement* is skipped as before. The list was written to keep "adaptation/coding write
//! cases" unread (the project spec of 2026-08-07). This multiplexer describes read-only fault
//! records, its selector is the record number the answer itself carries, and the owner
//! approved reading it, and it alone (2026-10-21). So the three are read **here**, inline,
//! by a function no dispatcher reaches: only [`crate::odis::Project::extended_records`] calls
//! it, on an object it found by this multiplexer's short name.

use super::super::Error;
use super::super::compu;
use super::super::object::Stream;
use super::{Ref, code, reference};

/// The short name a variant's layers resolve to its extended-data multiplexer.
///
/// VW's converter's name, declared by the protocol layer's own
/// `STRUC_DTCExtenDataRecor` and overridden per variant (171 objects in the reference
/// project's 43 pools); looked up in the layers, not assumed.
pub const EXTENDED_DATA_MULTIPLEXER: &str = "MUX_DTCExtenDataRecor";

/// The switch key's own type code.
const SWITCH_KEY: u16 = 0x004C;

/// One case: the record numbers it covers, both ends included, and its layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Case {
	/// `None` for the default case, and for a case whose limits are the key's text rather
	/// than its number ([`Case::text`]).
	pub range: Option<(i32, i32)>,
	/// A case keyed by the physical value of a key whose DOP is a text table — two
	/// headlamp variants of the reference project write `"Standard DTC Information"` where
	/// the others write `1`. The caller turns it into a number through that table.
	pub text: Option<String>,
	pub short_name: Option<String>,
	pub long_name: Option<String>,
	/// The case's structure.
	pub structure: Option<Ref>,
}

/// A variant's extended-data multiplexer.
#[derive(Debug, Clone, PartialEq)]
pub struct Multiplexer {
	/// Where the case structures start, in bytes from the multiplexer's start — the record
	/// number's byte. 1 on 169 of the reference project's 171; 0 on the two DQ200
	/// gearboxes, whose structures count the record number's byte themselves.
	pub case_start: u32,
	/// The switch key's byte and bit position, and its DOP.
	pub key_byte: u32,
	pub key_bit: u8,
	pub key_dop: Option<Ref>,
	/// The numbered cases, in file order, then the default case if there is one.
	pub cases: Vec<Case>,
}

/// Read an `MCD_DB_PARAMETER_MULTIPLEXER` that is an extended-data multiplexer.
///
/// Its cases are read inline here; see the module note for why only here.
pub fn multiplexer(stream: &mut Stream<'_>) -> Result<Multiplexer, Error> {
	let case_start = stream.u32()?;
	let (key_bit, key_byte, key_dop) = match inline(stream)? {
		Some(SWITCH_KEY) => {
			let bit = stream.u8()?;
			let byte = stream.u32()?;
			(bit, byte, Some(reference(stream, false, false)?))
		}
		Some(other) => return Err(unexpected(other, SWITCH_KEY)),
		None => (0, 0, None),
	};
	let mut cases = Vec::new();
	match inline(stream)? {
		Some(code::DB_CASES) => {
			let count = stream.count32()?;
			for _ in 0..count {
				match inline(stream)? {
					Some(code::DB_CASE) => cases.push(case(stream)?),
					Some(other) => return Err(unexpected(other, code::DB_CASE)),
					None => {}
				}
			}
		}
		Some(other) => return Err(unexpected(other, code::DB_CASES)),
		None => {}
	}
	match inline(stream)? {
		Some(code::DB_DEFAULT_CASE) => cases.push(named_case(stream, None, None)?),
		Some(other) => return Err(unexpected(other, code::DB_DEFAULT_CASE)),
		None => {}
	}
	Ok(Multiplexer {
		case_start,
		key_byte,
		key_bit,
		key_dop,
		cases,
	})
}

/// The type code of an inline sub-object, if its presence flag says there is one — read
/// here without [`super::nested_any`]'s refusal, which is the point of this module.
fn inline(stream: &mut Stream<'_>) -> Result<Option<u16>, Error> {
	if !stream.flag()? {
		return Ok(None);
	}
	Ok(Some(stream.u16()?))
}

fn unexpected(found: u16, expected: u16) -> Error {
	Error::Format(format!(
		"an extended-data multiplexer holds type {found:#06x} where {expected:#06x} was expected"
	))
}

/// A `DB_CASE`: its two limits, then what a default case also has.
fn case(stream: &mut Stream<'_>) -> Result<Case, Error> {
	let mut limit = || -> Result<Option<compu::Limit>, Error> {
		match inline(stream)? {
			Some(code::DB_LIMIT) => Ok(Some(compu::limit(stream)?)),
			Some(other) => Err(unexpected(other, code::DB_LIMIT)),
			None => Ok(None),
		}
	};
	let (lower, upper) = (limit()?, limit()?);
	let number = |l: &Option<compu::Limit>| l.as_ref().and_then(compu::closed_bound);
	let range = match (number(&lower), number(&upper)) {
		(Some(lower), Some(upper)) => Some((lower, upper.max(lower))),
		(Some(one), None) | (None, Some(one)) => Some((one, one)),
		(None, None) => None,
	};
	let text = match range {
		Some(_) => None,
		None => lower.as_ref().or(upper.as_ref()).and_then(compu::limit_text),
	};
	if range.is_none() && text.is_none() {
		return Err(Error::Format("an extended-data case has no record number".into()));
	}
	named_case(stream, range, text)
}

/// The fields every case has: its names, four strings nobody reads, its structure.
fn named_case(stream: &mut Stream<'_>, range: Option<(i32, i32)>, text: Option<String>) -> Result<Case, Error> {
	let short_name = stream.ascii()?.map(str::to_owned);
	let long_name = stream.unicode()?.map(str::to_owned);
	for _ in 0..4 {
		// Description, and three ids: all four empty on every case of the reference
		// project's 171 multiplexers; read as hashes so a filled one cannot desynchronise.
		let _ = stream.u32()?;
	}
	let structure = if stream.flag()? { Some(reference(stream, false, false)?) } else { None };
	Ok(Case {
		range,
		text,
		short_name,
		long_name,
		structure,
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::odis::hash;
	use crate::odis::object::{END, Stream};
	use crate::odis::strings::{Pool, Strings};

	/// The byte-builder the other loaders' tests use, kept local as theirs is.
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
		/// The "an inline object follows" flag plus its type code.
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

	/// A closed limit written as the decimal text a case carries.
	fn limit(b: &mut Bytes, value: &str) {
		const UNICODE2STRING: u8 = 0x0E;
		const CLOSED: u8 = 2;
		b.some(code::DB_LIMIT).u8(UNICODE2STRING).u(Some(value)).u8(CLOSED);
	}

	/// A case's names, four empty strings and its structure.
	fn names_and_structure(b: &mut Bytes, short: &str, long: &str, structure: &str) {
		b.a(Some(short)).u(Some(long));
		for _ in 0..4 {
			b.u32(0);
		}
		b.u8(1).a(Some(structure)).a(Some("0.0.0@POOL.bv"));
	}

	/// An invented extended-data multiplexer: cases 1 and 112, then a default.
	fn multiplexer_bytes(case_start: u32) -> (Vec<u8>, Strings) {
		let mut b = Bytes::default();
		b.u32(case_start);
		b.some(SWITCH_KEY).u8(0).u32(0).a(Some("DOP_UINTHEX1Byte")).a(Some("0.0.0@POOL.sd"));
		b.some(code::DB_CASES).u32(2);
		for (number, short, structure) in [("1", "Case_Info", "STRUC_Info"), ("112", "Case_Own", "STRUC_Own")] {
			b.some(code::DB_CASE);
			limit(&mut b, number);
			limit(&mut b, number);
			names_and_structure(&mut b, short, "a case", structure);
		}
		b.some(code::DB_DEFAULT_CASE);
		names_and_structure(&mut b, "Case_Unknown", "unknown freeze frame", "STRUC_Unknown");
		b.done(code::MCD_DB_PARAMETER_MULTIPLEXER)
	}

	#[test]
	fn the_cases_are_record_numbers_and_the_default_comes_last() {
		let (body, strings) = multiplexer_bytes(1);
		let (type_code, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		assert_eq!(type_code, code::MCD_DB_PARAMETER_MULTIPLEXER);
		let mux = multiplexer(&mut stream).expect("the multiplexer reads");
		stream.end().expect("every field read lands on the terminator");
		assert_eq!((mux.case_start, mux.key_byte, mux.key_bit), (1, 0, 0));
		assert_eq!(mux.key_dop.as_ref().and_then(|r| r.object.as_deref()), Some("DOP_UINTHEX1Byte"));
		let ranges: Vec<Option<(i32, i32)>> = mux.cases.iter().map(|c| c.range).collect();
		assert_eq!(ranges, vec![Some((1, 1)), Some((112, 112)), None]);
		assert_eq!(mux.cases[2].long_name.as_deref(), Some("unknown freeze frame"));
		assert_eq!(
			mux.cases[1].structure.as_ref().map(|r| (r.object.as_deref(), r.pool.as_deref())),
			Some((Some("STRUC_Own"), Some("0.0.0@POOL.bv")))
		);
	}

	#[test]
	fn a_case_start_of_zero_is_kept_for_the_caller_to_shift_by() {
		let (body, strings) = multiplexer_bytes(0);
		let (_, mut stream) = Stream::open(&body, &strings).unwrap();
		assert_eq!(multiplexer(&mut stream).unwrap().case_start, 0);
	}

	#[test]
	fn the_case_family_stays_refused_everywhere_else() {
		// Read here, inline, and nowhere else: the dispatcher still refuses all three, and
		// has no loader for a multiplexer, so a multiplexed measurement cannot reach them.
		for type_code in [code::DB_CASE, code::DB_CASES, code::DB_DEFAULT_CASE] {
			assert!(super::super::refused(type_code));
		}
		let (body, strings) = multiplexer_bytes(1);
		let (type_code, mut stream) = Stream::open(&body, &strings).unwrap();
		assert_eq!(
			super::super::load(type_code, &mut stream).unwrap(),
			super::super::Outcome::Unsupported(code::MCD_DB_PARAMETER_MULTIPLEXER)
		);
	}

	#[test]
	fn an_object_of_another_shape_is_an_error_not_a_guess() {
		let mut b = Bytes::default();
		b.u32(1);
		b.some(code::DB_LIMIT);
		let (body, strings) = b.done(code::MCD_DB_PARAMETER_MULTIPLEXER);
		let (_, mut stream) = Stream::open(&body, &strings).unwrap();
		assert!(matches!(multiplexer(&mut stream), Err(Error::Format(_))));
	}
}
