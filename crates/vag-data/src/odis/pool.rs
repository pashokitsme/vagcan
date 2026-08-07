//! A `.db` file: concatenated zlib members, located by what the `.key` tree
//! stores against an object's name.
//!
//! A `.db` has no index and no framing of its own — it is nothing but zlib
//! streams laid end to end (`research/labels/odis-crib.md` §2 found them by
//! scanning for `78 9c`). What says where one starts is the paired `.key`
//! file: every leaf's data is a [`Locator`], a `(position, compressed size,
//! decompressed size)` triple.
//!
//! The triple is stored in one of three widths, chosen by how big the numbers
//! are, and the width is carried by the record's length alone: 6 bytes means
//! the two sizes are `u8`, 8 means `u16`, 12 means `u32`. The position is
//! always a little-endian `u32`. On the reference project's engine pool the
//! census is 552,223 six-byte, 24,569 eight-byte and one twelve-byte record —
//! so all three widths are real and none may be dropped.

use std::path::Path;

use super::Error;

/// Where one object's bytes are in a `.db` file, and how big they are on both
/// sides of the inflate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Locator {
	/// Byte offset of the zlib member in the `.db` file.
	pub position: u32,
	/// Length of the zlib member.
	pub compressed: u32,
	/// Length the member must inflate to. Checked, not trusted — see
	/// [`Pool::member`].
	pub decompressed: u32,
}

/// One `.db` file, held whole in memory.
///
/// A pool is read start to finish (every object in it is wanted, or none is),
/// so one read beats a seek per member. The largest in the reference project
/// is under a megabyte.
#[derive(Debug)]
pub struct Pool {
	bytes: Vec<u8>,
}

impl Locator {
	/// Decode a `.key` leaf's data.
	///
	/// A record of any other length is a `Format` error rather than a
	/// best-effort read: the width *is* the length, so a length nobody defined
	/// means the bytes are not a locator and guessing at them would seek into
	/// the middle of some other object.
	pub fn parse(data: &[u8]) -> Result<Locator, Error> {
		let position = u32::from_le_bytes(
			data
				.get(..4)
				.and_then(|b| <[u8; 4]>::try_from(b).ok())
				.ok_or_else(|| Error::Format(format!("a locator is {} bytes, too short to hold a position", data.len())))?,
		);
		let (compressed, decompressed) = match data.len() {
			6 => (u32::from(data[4]), u32::from(data[5])),
			8 => (
				u32::from(u16::from_le_bytes([data[4], data[5]])),
				u32::from(u16::from_le_bytes([data[6], data[7]])),
			),
			12 => (
				u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
				u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
			),
			n => return Err(Error::Format(format!("a locator is {n} bytes; only 6, 8 and 12 are defined"))),
		};
		Ok(Locator {
			position,
			compressed,
			decompressed,
		})
	}
}

impl Pool {
	/// Read a `.db` file.
	pub fn open(path: &Path) -> Result<Pool, Error> {
		Ok(Pool {
			bytes: std::fs::read(path).map_err(Error::Io)?,
		})
	}

	/// Take an already-read `.db` file.
	pub fn from_bytes(bytes: Vec<u8>) -> Pool {
		Pool { bytes }
	}

	/// Inflate the member a locator points at.
	///
	/// The declared decompressed size is verified against what actually came
	/// out. That check is the point: a `.db` truncated mid-member still
	/// inflates to *something* — a prefix — and a reader that trusted the
	/// length prefix would go on to parse that prefix as a whole object and
	/// report a control unit's measurements from half a record.
	pub fn member(&self, locator: &Locator) -> Result<Vec<u8>, Error> {
		let start = locator.position as usize;
		let end = start
			.checked_add(locator.compressed as usize)
			.ok_or_else(|| Error::Format(format!("a member at {start} of {} bytes overflows", locator.compressed)))?;
		let stream = self.bytes.get(start..end).ok_or_else(|| {
			Error::Format(format!(
				"a member at {start}..{end} runs past the end of a {}-byte pool",
				self.bytes.len()
			))
		})?;
		let out =
			miniz_oxide::inflate::decompress_to_vec_zlib(stream).map_err(|e| Error::Format(format!("the member at {start} does not inflate: {e:?}")))?;
		if out.len() != locator.decompressed as usize {
			return Err(Error::Format(format!(
				"the member at {start} inflates to {} bytes, not the {} its locator declares",
				out.len(),
				locator.decompressed
			)));
		}
		Ok(out)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Build a `.db` from object bodies, returning it with a locator each.
	fn pool_of(bodies: &[&[u8]]) -> (Pool, Vec<Locator>) {
		let mut bytes = Vec::new();
		let mut locators = Vec::new();
		for body in bodies {
			let member = miniz_oxide::deflate::compress_to_vec_zlib(body, 6);
			locators.push(Locator {
				position: bytes.len() as u32,
				compressed: member.len() as u32,
				decompressed: body.len() as u32,
			});
			bytes.extend_from_slice(&member);
		}
		(Pool::from_bytes(bytes), locators)
	}

	#[test]
	fn a_member_round_trips() {
		let first = b"\x2c\x00 the first object".as_slice();
		let second = b"\xbe\x00 the second, longer object".as_slice();
		let (pool, locators) = pool_of(&[first, second]);
		assert_eq!(pool.member(&locators[0]).expect("a well-formed member inflates"), first);
		assert_eq!(pool.member(&locators[1]).expect("a well-formed member inflates"), second);
	}

	#[test]
	fn a_declared_size_that_disagrees_is_refused() {
		let (pool, mut locators) = pool_of(&[b"the object"]);
		locators[0].decompressed += 1;
		let err = pool.member(&locators[0]).expect_err("a size that disagrees must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_truncated_pool_is_refused_rather_than_half_read() {
		let (pool, locators) = pool_of(&[b"an object long enough to be worth truncating"]);
		let mut bytes = pool.bytes;
		bytes.truncate(bytes.len() - 4);
		let truncated = Pool::from_bytes(bytes);
		// The locator now points past the end, or the stream no longer
		// inflates whole. Either way it must not come back as a short object.
		let err = truncated.member(&locators[0]).expect_err("a truncated member must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_member_running_past_the_end_is_refused() {
		let (pool, mut locators) = pool_of(&[b"the object"]);
		locators[0].position = 10_000;
		let err = pool.member(&locators[0]).expect_err("a member past the end must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn bytes_that_are_not_zlib_are_refused() {
		let pool = Pool::from_bytes(b"not a zlib stream at all, just text".to_vec());
		let locator = Locator {
			position: 0,
			compressed: 20,
			decompressed: 20,
		};
		let err = pool.member(&locator).expect_err("non-zlib bytes must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn all_three_locator_widths_decode() {
		// 6 bytes: u8 sizes.
		assert_eq!(
			Locator::parse(&[0x10, 0, 0, 0, 0x2a, 0x50]).expect("six bytes is a locator"),
			Locator {
				position: 0x10,
				compressed: 0x2a,
				decompressed: 0x50
			}
		);
		// 8 bytes: u16 sizes.
		assert_eq!(
			Locator::parse(&[0x10, 0x20, 0, 0, 0x34, 0x12, 0x78, 0x56]).expect("eight bytes is a locator"),
			Locator {
				position: 0x2010,
				compressed: 0x1234,
				decompressed: 0x5678
			}
		);
		// 12 bytes: u32 sizes.
		assert_eq!(
			Locator::parse(&[1, 0, 0, 0, 2, 0, 0, 0, 3, 0, 0, 0]).expect("twelve bytes is a locator"),
			Locator {
				position: 1,
				compressed: 2,
				decompressed: 3
			}
		);
	}

	#[test]
	fn a_locator_of_an_undefined_width_is_refused() {
		for width in [0usize, 4, 5, 7, 9, 16] {
			let data = vec![0u8; width];
			let err = Locator::parse(&data).expect_err("only 6, 8 and 12 are defined");
			assert!(matches!(err, Error::Format(_)), "width {width} gave {err:?}");
		}
	}
}
