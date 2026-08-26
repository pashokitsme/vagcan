//! The object stream: what an inflated `.db` member holds.
//!
//! A member opens with a two-byte little-endian **type code** and then runs
//! straight into that type's fields, in a fixed order, with no tags and no
//! lengths — the type code is the only self-description in the whole stream.
//! Read a field of the wrong width and everything after it is garbage that
//! still parses, which is why every loader in [`super::loaders`] is a literal
//! transcription of a field order and why [`Stream::end`] exists: reaching the
//! terminator exactly is the only evidence a loader read the right shape.
//!
//! (The design document calls these "tagged fields". They are not; the tag is
//! the object's, not the field's. The consequence is the important part: there
//! is no way to skip a field you do not understand.)
//!
//! ## The terminator
//! An object's fields are followed by three bytes. Two forms occur in the
//! reference project's engine pool — `23 3E 00` (`#>\0`, 407,974 members) and
//! `23 3C 00` (`#<\0`, 168,819, exactly the `MCD_DB_TABLE_PARAMETER` count).
//! `ODIS-project-explorer` only documents the first. Both are accepted here.
//!
//! Only the **outermost** object of a member is terminated. A nested one — a
//! compu method inside a data object property, say — runs straight into the
//! field that follows it, so a loader that consumed a terminator of its own
//! would eat the next field's first bytes.
//!
//! A terminator is also not always the *last* three bytes of a member: some
//! types append further named sub-streams after it. So [`Stream::end`] asserts
//! that the terminator is where the fields stopped, and says nothing about
//! what follows.
//!
//! ## Strings
//! Most string fields are a four-byte hash into one of the two pools
//! ([`super::strings`]) — `0` meaning "no string", which is why a hash of `0`
//! is illegal ([`super::hash::ZERO_SUBSTITUTE`]). A few are stored inline
//! instead, flagged by the high bit of the same four bytes.

use super::Error;
use super::strings::Strings;

/// The terminator `ODIS-project-explorer` documents, `#>\0`.
pub const END: [u8; 3] = [0x23, 0x3E, 0x00];

/// The second terminator, `#<\0`. Undocumented there, but a third of the
/// reference project's members end on it.
pub const END_ALT: [u8; 3] = [0x23, 0x3C, 0x00];

/// One `MCDValue`: a tagged scalar, used for default values, constants and
/// the limits of a compu scale.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
	/// A pooled Windows-1252 string; `None` when the hash was `0`.
	Ascii(Option<String>),
	/// A pooled UTF-16 string; `None` when the hash was `0`.
	Unicode(Option<String>),
	/// `A_FLOAT32`.
	F32(f32),
	/// `A_FLOAT64`.
	F64(f64),
	/// `A_INT32`.
	I32(i32),
	/// `A_UINT32`.
	U32(u32),
	/// `A_BYTEFIELD` or `A_BITFIELD` — raw bytes either way.
	Bytes(Vec<u8>),
}

/// A cursor over one inflated object.
#[derive(Debug)]
pub struct Stream<'a> {
	data: &'a [u8],
	at: usize,
	strings: &'a Strings,
}

/// The type code at the head of an inflated member, without starting to read
/// its fields.
///
/// This is what makes a whole pool cheap to survey: a member's type can be
/// decided before committing to a loader, so a pool can be scanned for the
/// one object type wanted and everything else left untouched.
pub fn type_code(data: &[u8]) -> Result<u16, Error> {
	let head = data
		.get(..2)
		.ok_or_else(|| Error::Format(format!("an object is {} bytes, too short to hold a type code", data.len())))?;
	Ok(u16::from_le_bytes([head[0], head[1]]))
}

impl<'a> Stream<'a> {
	/// Start reading an inflated member. Returns its type code and a cursor
	/// positioned on the first field.
	pub fn open(data: &'a [u8], strings: &'a Strings) -> Result<(u16, Stream<'a>), Error> {
		Ok((type_code(data)?, Stream { data, at: 2, strings }))
	}

	/// How many bytes are left unread, terminator included.
	pub fn remaining(&self) -> usize {
		self.data.len().saturating_sub(self.at)
	}

	/// Where the cursor is, so a loader can come back to it.
	pub fn mark(&self) -> usize {
		self.at
	}

	/// Put the cursor back where [`Stream::mark`] said.
	///
	/// The one place a loader may move backwards, and it exists for exactly one
	/// case: an object whose tail this reader cannot follow, where the choice is
	/// between abandoning the head it *did* read and rewinding to leave the
	/// stream honest about how far it got. Nothing is re-read; the caller stops.
	pub fn rewind(&mut self, mark: usize) {
		self.at = mark.min(self.data.len());
	}

	/// Consume the terminator that ends an object's fields.
	///
	/// Anything else here means the loader and the file disagree about the
	/// type's shape — the fields were read at the wrong widths and everything
	/// this object reported is suspect. It is the one check that catches that,
	/// so it must never be softened into a search for the next terminator.
	pub fn end(&mut self) -> Result<(), Error> {
		let three = self.bytes(3)?;
		if three == END || three == END_ALT {
			return Ok(());
		}
		Err(Error::Format(format!(
			"an object's fields end at byte {} on {three:02X?}, which is not a terminator",
			self.at - 3
		)))
	}

	/// Take `n` raw bytes.
	pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
		let end = self
			.at
			.checked_add(n)
			.ok_or_else(|| Error::Format(format!("a field of {n} bytes at {} overflows", self.at)))?;
		let slice = self.data.get(self.at..end).ok_or_else(|| {
			Error::Format(format!(
				"a field wants {n} bytes at {}, but the object is {} bytes",
				self.at,
				self.data.len()
			))
		})?;
		self.at = end;
		Ok(slice)
	}

	/// One byte.
	pub fn u8(&mut self) -> Result<u8, Error> {
		Ok(self.bytes(1)?[0])
	}

	/// Two bytes, little-endian. Also how every short enum is stored.
	pub fn u16(&mut self) -> Result<u16, Error> {
		let b = self.bytes(2)?;
		Ok(u16::from_le_bytes([b[0], b[1]]))
	}

	/// Four bytes, little-endian.
	pub fn u32(&mut self) -> Result<u32, Error> {
		let b = self.bytes(4)?;
		Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
	}

	/// Four bytes, little-endian, signed.
	pub fn i32(&mut self) -> Result<i32, Error> {
		Ok(self.u32()? as i32)
	}

	/// Eight bytes, an IEEE-754 double. Every coefficient of a compu method is
	/// one of these.
	pub fn f64(&mut self) -> Result<f64, Error> {
		let b = self.bytes(8)?;
		Ok(f64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
	}

	/// A one-byte boolean.
	///
	/// The writer only ever emits `0` or `1`, so anything else is not a flag —
	/// it is the first byte of a field the loader read at the wrong offset.
	/// Refusing here turns a silent misparse into a located one.
	pub fn flag(&mut self) -> Result<bool, Error> {
		match self.u8()? {
			0 => Ok(false),
			1 => Ok(true),
			other => Err(Error::Format(format!(
				"a flag at byte {} is {other}, neither 0 nor 1 — the fields before it were read at the wrong widths",
				self.at - 1
			))),
		}
	}

	/// A collection count: two bytes.
	pub fn count(&mut self) -> Result<usize, Error> {
		Ok(usize::from(self.u16()?))
	}

	/// A collection count: four bytes. Some collections use this width and
	/// some the two-byte one, per type; there is no rule, only the field order.
	pub fn count32(&mut self) -> Result<usize, Error> {
		// A count is bounded by what is left: each element costs at least a
		// byte, so a count larger than the remaining bytes cannot be honoured
		// and is a misparse rather than a very large collection.
		let count = self.u32()? as usize;
		if count > self.remaining() {
			return Err(Error::Format(format!(
				"a collection at byte {} claims {count} elements with only {} bytes left",
				self.at - 4,
				self.remaining()
			)));
		}
		Ok(count)
	}

	/// A pooled `AStringData` reference: four bytes of hash, `0` for absent.
	pub fn ascii(&mut self) -> Result<Option<&'a str>, Error> {
		let hash = self.u32()?;
		if hash == 0 {
			return Ok(None);
		}
		// A hash the pool does not hold is reported as absent rather than as
		// an error: the pools are shared across a whole project, and a name
		// missing from them is a gap in the project, not a broken object.
		Ok(self.strings.ascii.get(hash))
	}

	/// A pooled `UStringData` reference: four bytes of hash, `0` for absent.
	pub fn unicode(&mut self) -> Result<Option<&'a str>, Error> {
		let hash = self.u32()?;
		if hash == 0 {
			return Ok(None);
		}
		Ok(self.strings.unicode.get(hash))
	}

	/// A string stored inline rather than pooled.
	///
	/// Four bytes: with the high bit set, the low 31 are a byte count and that
	/// many Windows-1252 bytes follow; `0` means absent. Any other value would
	/// be a pool hash in a field the writer never puts one in, so it is
	/// refused rather than looked up.
	pub fn inline_ascii(&mut self) -> Result<Option<String>, Error> {
		let head = self.u32()?;
		if head == 0 {
			return Ok(None);
		}
		if head & 0x8000_0000 == 0 {
			return Err(Error::Format(format!(
				"an inline string at byte {} has no length bit set ({head:#x})",
				self.at - 4
			)));
		}
		let len = (head & 0x7fff_ffff) as usize;
		Ok(Some(super::strings::cp1252(self.bytes(len)?)))
	}

	/// An optional byte field: a flag, then a four-byte length and its bytes.
	pub fn bytefield(&mut self) -> Result<Vec<u8>, Error> {
		if !self.flag()? {
			return Ok(Vec::new());
		}
		let len = self.u32()? as usize;
		Ok(self.bytes(len)?.to_vec())
	}

	/// One `MCDValue`.
	///
	/// A one-byte data type, then a payload whose shape it selects. Type codes
	/// above 18 are the "no type" sentinel and carry no payload; the codes
	/// inside the range that name a width this format never stores
	/// (`A_INT8`/`A_UINT8`, and the 16- and 64-bit integers) are refused,
	/// because a value stored at a width nothing writes means the byte was not
	/// a type code at all.
	pub fn value(&mut self) -> Result<Option<Value>, Error> {
		// MCDDataType, as the kernel numbers it.
		const ASCIISTRING: u8 = 0x01;
		const BITFIELD: u8 = 0x02;
		const BYTEFIELD: u8 = 0x03;
		const FLOAT32: u8 = 0x04;
		const FLOAT64: u8 = 0x05;
		const INT32: u8 = 0x07;
		const UINT32: u8 = 0x0B;
		const UNICODE2STRING: u8 = 0x0E;
		/// Everything past this is the "no type" sentinel.
		const NO_TYPE_ABOVE: u8 = 18;

		let kind = self.u8()?;
		if kind > NO_TYPE_ABOVE {
			return Ok(None);
		}
		Ok(Some(match kind {
			ASCIISTRING => Value::Ascii(self.ascii()?.map(str::to_owned)),
			UNICODE2STRING => Value::Unicode(self.unicode()?.map(str::to_owned)),
			FLOAT32 => {
				let b = self.bytes(4)?;
				Value::F32(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
			}
			FLOAT64 => Value::F64(self.f64()?),
			INT32 => Value::I32(self.i32()?),
			UINT32 => Value::U32(self.u32()?),
			BITFIELD | BYTEFIELD => {
				// Note the width: this length is two bytes, unlike the
				// four-byte one `bytefield` reads. They are different fields.
				if !self.flag()? {
					Value::Bytes(Vec::new())
				} else {
					let len = usize::from(self.u16()?);
					Value::Bytes(self.bytes(len)?.to_vec())
				}
			}
			other => {
				return Err(Error::Format(format!(
					"a value at byte {} has data type {other}, which nothing in this format stores",
					self.at - 1
				)));
			}
		}))
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::odis::hash;
	use crate::odis::strings::Pool;

	/// A `Strings` holding the given ASCII and Unicode strings.
	fn strings(ascii: &[&str], unicode: &[&str]) -> Strings {
		let mut a = Vec::new();
		for s in ascii {
			a.extend_from_slice(&(s.len() as u32).to_le_bytes());
			a.extend_from_slice(s.as_bytes());
		}
		let mut u = Vec::new();
		for s in unicode {
			let units: Vec<u16> = s.encode_utf16().collect();
			u.extend_from_slice(&(units.len() as u32).to_le_bytes());
			for unit in units {
				u.extend_from_slice(&unit.to_le_bytes());
			}
		}
		Strings {
			ascii: Pool::parse_ascii(&a).expect("a synthesised pool parses"),
			unicode: Pool::parse_utf16(&u).expect("a synthesised pool parses"),
		}
	}

	#[test]
	fn the_type_code_leads_the_stream() {
		let empty = strings(&[], &[]);
		let mut body = 0x00BEu16.to_le_bytes().to_vec();
		body.extend_from_slice(&END);
		let (code, mut stream) = Stream::open(&body, &empty).expect("a two-byte head is a type code");
		assert_eq!(code, 0x00BE);
		stream.end().expect("the terminator follows immediately");
	}

	#[test]
	fn an_object_too_short_for_a_type_code_is_refused() {
		let empty = strings(&[], &[]);
		let err = Stream::open(&[0x2c], &empty).expect_err("one byte is not a type code");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn every_scalar_shape_reads_back() {
		let empty = strings(&[], &[]);
		let mut body = 0x0001u16.to_le_bytes().to_vec();
		body.push(0xAB); // u8
		body.extend_from_slice(&0x1234u16.to_le_bytes());
		body.extend_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
		body.extend_from_slice(&(-7i32).to_le_bytes());
		body.extend_from_slice(&0.4f64.to_le_bytes());
		body.push(1); // flag
		body.extend_from_slice(&END);

		let (_, mut stream) = Stream::open(&body, &empty).expect("a well-formed object opens");
		assert_eq!(stream.u8().expect("a byte"), 0xAB);
		assert_eq!(stream.u16().expect("two bytes"), 0x1234);
		assert_eq!(stream.u32().expect("four bytes"), 0xDEAD_BEEF);
		assert_eq!(stream.i32().expect("four signed bytes"), -7);
		assert_eq!(stream.f64().expect("eight bytes"), 0.4);
		assert!(stream.flag().expect("a flag"));
		stream.end().expect("the terminator follows the fields");
	}

	#[test]
	fn pooled_strings_resolve_through_their_hash() {
		let pools = strings(&["EV_ECM18TFS"], &["Motordrehzahl"]);
		let a = hash::of_bytes(b"EV_ECM18TFS");
		let u = hash::of_utf16(&"Motordrehzahl".encode_utf16().collect::<Vec<_>>());
		let mut body = 0x0001u16.to_le_bytes().to_vec();
		body.extend_from_slice(&a.to_le_bytes());
		body.extend_from_slice(&u.to_le_bytes());
		body.extend_from_slice(&0u32.to_le_bytes()); // the "no string" hash
		body.extend_from_slice(&END);

		let (_, mut stream) = Stream::open(&body, &pools).expect("a well-formed object opens");
		assert_eq!(stream.ascii().expect("a pooled name"), Some("EV_ECM18TFS"));
		assert_eq!(stream.unicode().expect("a pooled text"), Some("Motordrehzahl"));
		assert_eq!(stream.ascii().expect("a zero hash"), None, "a hash of 0 means no string, not a lookup");
		stream.end().expect("the terminator follows the fields");
	}

	#[test]
	fn an_inline_string_carries_its_own_length() {
		let empty = strings(&[], &[]);
		let text = b"UDSOnCAN";
		let mut body = 0x0001u16.to_le_bytes().to_vec();
		body.extend_from_slice(&(0x8000_0000u32 | text.len() as u32).to_le_bytes());
		body.extend_from_slice(text);
		body.extend_from_slice(&0u32.to_le_bytes());
		body.extend_from_slice(&END);

		let (_, mut stream) = Stream::open(&body, &empty).expect("a well-formed object opens");
		assert_eq!(stream.inline_ascii().expect("an inline string").as_deref(), Some("UDSOnCAN"));
		assert_eq!(stream.inline_ascii().expect("an absent inline string"), None);
		stream.end().expect("the terminator follows the fields");
	}

	#[test]
	fn an_inline_string_without_its_length_bit_is_refused() {
		let empty = strings(&[], &[]);
		let mut body = 0x0001u16.to_le_bytes().to_vec();
		body.extend_from_slice(&42u32.to_le_bytes());
		let (_, mut stream) = Stream::open(&body, &empty).expect("a well-formed object opens");
		let err = stream.inline_ascii().expect_err("a bare number is not an inline string");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn each_value_shape_reads_back() {
		let pools = strings(&["ppm"], &[]);
		let mut body = 0x0001u16.to_le_bytes().to_vec();
		body.push(0x07); // A_INT32
		body.extend_from_slice(&(-1i32).to_le_bytes());
		body.push(0x05); // A_FLOAT64
		body.extend_from_slice(&2.5f64.to_le_bytes());
		body.push(0x01); // A_ASCIISTRING
		body.extend_from_slice(&hash::of_bytes(b"ppm").to_le_bytes());
		body.push(0x03); // A_BYTEFIELD, present, two-byte length
		body.push(1);
		body.extend_from_slice(&2u16.to_le_bytes());
		body.extend_from_slice(&[0xFF, 0xFF]);
		body.push(0xFF); // above 18: the "no type" sentinel, no payload
		body.extend_from_slice(&END);

		let (_, mut stream) = Stream::open(&body, &pools).expect("a well-formed object opens");
		assert_eq!(stream.value().expect("an int"), Some(Value::I32(-1)));
		assert_eq!(stream.value().expect("a double"), Some(Value::F64(2.5)));
		assert_eq!(stream.value().expect("a string"), Some(Value::Ascii(Some("ppm".into()))));
		assert_eq!(stream.value().expect("a byte field"), Some(Value::Bytes(vec![0xFF, 0xFF])));
		assert_eq!(stream.value().expect("a sentinel"), None);
		stream.end().expect("the terminator follows the fields");
	}

	#[test]
	fn a_value_of_a_width_nothing_stores_is_refused() {
		let empty = strings(&[], &[]);
		for kind in [0x00u8, 0x06, 0x08, 0x0A, 0x0C, 0x09] {
			let mut body = 0x0001u16.to_le_bytes().to_vec();
			body.push(kind);
			body.extend_from_slice(&[0; 8]);
			let (_, mut stream) = Stream::open(&body, &empty).expect("a well-formed object opens");
			let err = stream.value().expect_err("an unstored width must be refused");
			assert!(matches!(err, Error::Format(_)), "type {kind:#x} gave {err:?}");
		}
	}

	#[test]
	fn both_terminators_are_accepted() {
		let empty = strings(&[], &[]);
		for end in [END, END_ALT] {
			let mut body = 0x00ACu16.to_le_bytes().to_vec();
			body.extend_from_slice(&end);
			// Trailing named sub-streams follow the terminator on some types.
			body.extend_from_slice(&[0x41, 0x01, 0x23, 0x3E, 0x01]);
			let (_, mut stream) = Stream::open(&body, &empty).expect("a well-formed object opens");
			stream.end().expect("both terminator forms end an object's fields");
			assert_eq!(stream.remaining(), 5, "what follows a terminator is not this reader's business");
		}
	}

	#[test]
	fn a_stream_missing_its_terminator_is_refused() {
		let empty = strings(&[], &[]);
		let mut body = 0x0001u16.to_le_bytes().to_vec();
		body.extend_from_slice(&[0x00, 0x00, 0x00]);
		let (_, mut stream) = Stream::open(&body, &empty).expect("a well-formed object opens");
		let err = stream.end().expect_err("fields that do not end on a terminator must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_field_running_past_the_buffer_is_refused() {
		let empty = strings(&[], &[]);
		let body = 0x0001u16.to_le_bytes().to_vec();
		let (_, mut stream) = Stream::open(&body, &empty).expect("a well-formed object opens");
		let err = stream.u32().expect_err("there are no bytes left");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");

		// A byte field whose declared length runs off the end.
		let mut body = 0x0001u16.to_le_bytes().to_vec();
		body.push(1);
		body.extend_from_slice(&1000u32.to_le_bytes());
		let (_, mut stream) = Stream::open(&body, &empty).expect("a well-formed object opens");
		let err = stream.bytefield().expect_err("a length past the end must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_flag_that_is_neither_zero_nor_one_is_refused() {
		let empty = strings(&[], &[]);
		let mut body = 0x0001u16.to_le_bytes().to_vec();
		body.push(0x42);
		let (_, mut stream) = Stream::open(&body, &empty).expect("a well-formed object opens");
		let err = stream.flag().expect_err("a flag is 0 or 1");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_collection_count_larger_than_the_object_is_refused() {
		let empty = strings(&[], &[]);
		let mut body = 0x0001u16.to_le_bytes().to_vec();
		body.extend_from_slice(&100_000u32.to_le_bytes());
		body.extend_from_slice(&END);
		let (_, mut stream) = Stream::open(&body, &empty).expect("a well-formed object opens");
		let err = stream.count32().expect_err("an impossible element count must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}
}
