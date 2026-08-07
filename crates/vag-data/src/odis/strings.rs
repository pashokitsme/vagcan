//! The two plaintext string pools an ODIS project ships, and the hash → name
//! table built from them.
//!
//! `research/labels/odis-crib.md` §2 established what these are and that they
//! are not encrypted: `AStringData.data` is 1.1 M short names as `u32` byte
//! count + Windows-1252, `UStringData.data` is 154 k texts as `u32` **character**
//! count + UTF-16LE. Both parse to the last byte in one forward pass, with no
//! index and no framing beyond the length prefix.
//!
//! ## Why the `.idx` files are not read
//! A project ships `AStringData.idx` / `UStringData.idx`, which record the hash
//! VW's tooling assigned to each string. This module ignores them and recomputes
//! the hashes ([`super::hash`]) instead — one less binary format to port, and
//! the reconstruction is exact where it matters: on the reference project all
//! 576,793 keys of the engine pool's `.key` tree resolve against a table built
//! this way. (Ten of the A pool's 1,155,437 strings land on a hash the `.idx`
//! does not list. They are strings that occur twice, where the writer's
//! insertion order and ours disagree about which copy took the base hash and
//! which took the `+ 11` probe. No `.key` entry pointed at one.)
//!
//! The pools are held **inflated in memory** — 73 MB and 15 MB. That is the
//! price of resolving every object name in a project, and it is paid once per
//! [`super::Project`], not once per lookup.

use std::collections::HashMap;
use std::path::Path;

use super::Error;
use super::hash;

/// A parsed string pool: every string it holds, keyed by the hash a `.key`
/// tree or an object stream refers to it by.
#[derive(Debug, Default)]
pub struct Pool {
	by_hash: HashMap<u32, String>,
}

/// Both pools of one project. Kept together because an object stream picks
/// between them field by field — a short name is an A hash, its long name a U
/// hash — and neither is meaningful without the other.
#[derive(Debug, Default)]
pub struct Strings {
	/// `AStringData` — identifiers, short names, ObjectIDs, PoolIDs.
	pub ascii: Pool,
	/// `UStringData` — the human-readable long names and descriptions.
	pub unicode: Pool,
}

/// The A pool's file stem. Both a plain and a `.gz` form occur in the wild.
const ASCII_STEM: &str = "AStringData.data";
/// The U pool's file stem.
const UNICODE_STEM: &str = "UStringData.data";

impl Strings {
	/// Read and parse both pools out of a project directory.
	///
	/// Accepts `<stem>` and `<stem>.gz`, in that order — VW's converter writes
	/// the gzipped form, but a project that has been unpacked by hand (as
	/// `ODIS-project-explorer` does) carries the plain one.
	pub fn open(dir: &Path) -> Result<Strings, Error> {
		Ok(Strings {
			ascii: Pool::parse_ascii(&read_maybe_gz(dir, ASCII_STEM)?)?,
			unicode: Pool::parse_utf16(&read_maybe_gz(dir, UNICODE_STEM)?)?,
		})
	}
}

impl Pool {
	/// Parse an `AStringData.data` body: repeated `u32` byte count + that many
	/// Windows-1252 bytes, to the last byte of the buffer.
	pub fn parse_ascii(data: &[u8]) -> Result<Pool, Error> {
		let mut pool = Pool::default();
		let mut at = 0usize;
		while at < data.len() {
			let len = take_u32(data, &mut at, "ascii string pool")? as usize;
			let bytes = take(data, &mut at, len, "ascii string pool")?;
			pool.insert(hash::of_bytes(bytes), cp1252(bytes));
		}
		Ok(pool)
	}

	/// Parse a `UStringData.data` body: repeated `u32` **character** count +
	/// that many UTF-16LE code units. The count is characters, not bytes —
	/// reading it as bytes desynchronises on the very first string.
	pub fn parse_utf16(data: &[u8]) -> Result<Pool, Error> {
		let mut pool = Pool::default();
		let mut at = 0usize;
		while at < data.len() {
			let chars = take_u32(data, &mut at, "unicode string pool")? as usize;
			let bytes = take(data, &mut at, chars.saturating_mul(2), "unicode string pool")?;
			let units: Vec<u16> = bytes.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
			// Lone surrogates are possible in principle and would not be a
			// reason to reject a whole project, so they become U+FFFD.
			pool.insert(hash::of_utf16(&units), String::from_utf16_lossy(&units));
		}
		Ok(pool)
	}

	/// The string stored under `hash`, if this pool holds one.
	pub fn get(&self, hash: u32) -> Option<&str> {
		self.by_hash.get(&hash).map(String::as_str)
	}

	/// How many strings the pool holds.
	pub fn len(&self) -> usize {
		self.by_hash.len()
	}

	/// Whether the pool is empty.
	pub fn is_empty(&self) -> bool {
		self.by_hash.is_empty()
	}

	/// Every `(hash, string)` this pool holds, in no particular order.
	pub fn iter(&self) -> impl Iterator<Item = (u32, &str)> {
		self.by_hash.iter().map(|(&h, s)| (h, s.as_str()))
	}

	/// Insert at `hash`, probing on collision exactly as the writer did.
	///
	/// A collision here is two *different* strings hashing alike; the same
	/// string appearing twice also collides, and the duplicate then occupies
	/// the probed slot — matching the writer, which does not deduplicate.
	fn insert(&mut self, hash: u32, string: String) {
		let mut at = hash;
		while self.by_hash.contains_key(&at) {
			at = hash::probe(at);
		}
		self.by_hash.insert(at, string);
	}
}

/// Read `dir/stem`, falling back to `dir/stem.gz` and inflating it.
fn read_maybe_gz(dir: &Path, stem: &str) -> Result<Vec<u8>, Error> {
	let plain = dir.join(stem);
	if plain.is_file() {
		return std::fs::read(&plain).map_err(Error::Io);
	}
	let zipped = dir.join(format!("{stem}.gz"));
	if !zipped.is_file() {
		return Err(Error::Missing(format!("{} (neither {} nor {}.gz)", stem, plain.display(), stem)));
	}
	gunzip(&std::fs::read(&zipped).map_err(Error::Io)?)
}

/// Inflate a gzip member (RFC 1952) into its deflate payload.
///
/// `miniz_oxide` has zlib and raw inflate but no gzip wrapper, and the string
/// pools are the only gzip in the whole format — so the ten-byte header is
/// walked here rather than pulling in a dependency for it. The trailer (CRC32
/// and ISIZE) is not checked: the pool has to parse to its last byte anyway,
/// which is a stronger statement about the same bytes.
fn gunzip(bytes: &[u8]) -> Result<Vec<u8>, Error> {
	// RFC 1952 §2.3: ID1 ID2 CM FLG MTIME(4) XFL OS, then the optional extras
	// selected by FLG, then the deflate stream.
	const MAGIC: [u8; 2] = [0x1f, 0x8b];
	const DEFLATE: u8 = 8;
	const FEXTRA: u8 = 1 << 2;
	const FNAME: u8 = 1 << 3;
	const FCOMMENT: u8 = 1 << 4;
	const FHCRC: u8 = 1 << 1;

	if bytes.len() < 10 || bytes[..2] != MAGIC {
		return Err(Error::Format("not a gzip file: bad magic".into()));
	}
	if bytes[2] != DEFLATE {
		return Err(Error::Format(format!("gzip compression method {} is not deflate", bytes[2])));
	}
	let flags = bytes[3];
	let mut at = 10usize;
	if flags & FEXTRA != 0 {
		let len = u16::from_le_bytes([*byte(bytes, at)?, *byte(bytes, at + 1)?]) as usize;
		at = at.saturating_add(2).saturating_add(len);
	}
	for flag in [FNAME, FCOMMENT] {
		if flags & flag != 0 {
			// A NUL-terminated string; the terminator must be present.
			let end = bytes
				.get(at..)
				.and_then(|rest| rest.iter().position(|&b| b == 0))
				.ok_or_else(|| Error::Format("gzip header string is unterminated".into()))?;
			at = at.saturating_add(end).saturating_add(1);
		}
	}
	if flags & FHCRC != 0 {
		at = at.saturating_add(2);
	}
	let payload = bytes
		.get(at..)
		.ok_or_else(|| Error::Format("gzip header runs past the end of the file".into()))?;
	miniz_oxide::inflate::decompress_to_vec(payload).map_err(|e| Error::Format(format!("gzip inflate failed: {e:?}")))
}

/// Decode Windows-1252 into a `String`.
///
/// Bytes `0x00–0x7F` and `0xA0–0xFF` are their own code points; `0x80–0x9F`
/// are the printable characters of the Windows extension, five of which
/// (`0x81 0x8D 0x8F 0x90 0x9D`) the encoding leaves undefined. Undefined bytes
/// map to the matching C1 control, following the WHATWG encoding standard —
/// they do not occur in the reference project, and a name is not worth failing
/// a whole project over.
pub(super) fn cp1252(bytes: &[u8]) -> String {
	/// The `0x80–0x9F` block, in order. `\u{81}` etc. are the undefined slots.
	const HIGH: [char; 32] = [
		'\u{20AC}', '\u{81}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}', '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}',
		'\u{0152}', '\u{8D}', '\u{017D}', '\u{8F}', '\u{90}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
		'\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{9D}', '\u{017E}', '\u{0178}',
	];
	bytes
		.iter()
		.map(|&b| {
			if (0x80..0xA0).contains(&b) {
				HIGH[usize::from(b - 0x80)]
			} else {
				char::from(b)
			}
		})
		.collect()
}

/// Read a little-endian `u32` at `*at`, advancing it. A short buffer is a
/// `Format` error — a pool that ends mid-length is truncated, not empty.
fn take_u32(data: &[u8], at: &mut usize, what: &str) -> Result<u32, Error> {
	let bytes = take(data, at, 4, what)?;
	Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Take `len` bytes at `*at`, advancing it.
fn take<'a>(data: &'a [u8], at: &mut usize, len: usize, what: &str) -> Result<&'a [u8], Error> {
	let end = at
		.checked_add(len)
		.ok_or_else(|| Error::Format(format!("{what}: length {len} overflows")))?;
	let slice = data
		.get(*at..end)
		.ok_or_else(|| Error::Format(format!("{what}: wants {len} bytes at {at}, file ends at {}", data.len())))?;
	*at = end;
	Ok(slice)
}

/// One byte at `at`, or a truncation error.
fn byte(bytes: &[u8], at: usize) -> Result<&u8, Error> {
	bytes.get(at).ok_or_else(|| Error::Format("gzip header is truncated".into()))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Build an `AStringData.data` body from strings, the way VW's writer does.
	fn ascii_pool_bytes(strings: &[&str]) -> Vec<u8> {
		let mut out = Vec::new();
		for s in strings {
			out.extend_from_slice(&(s.len() as u32).to_le_bytes());
			out.extend_from_slice(s.as_bytes());
		}
		out
	}

	/// Build a `UStringData.data` body: character count, then UTF-16LE.
	fn utf16_pool_bytes(strings: &[&str]) -> Vec<u8> {
		let mut out = Vec::new();
		for s in strings {
			let units: Vec<u16> = s.encode_utf16().collect();
			out.extend_from_slice(&(units.len() as u32).to_le_bytes());
			for u in units {
				out.extend_from_slice(&u.to_le_bytes());
			}
		}
		out
	}

	#[test]
	fn an_ascii_pool_parses_to_the_last_byte() {
		let bytes = ascii_pool_bytes(&["EV_ECM", "#RtGen_DB_LAYER_DATA", ""]);
		let pool = Pool::parse_ascii(&bytes).expect("a well-formed pool parses");
		assert_eq!(pool.len(), 3);
		assert_eq!(pool.get(hash::of_bytes(b"EV_ECM")), Some("EV_ECM"));
		assert_eq!(pool.get(hash::of_bytes(b"#RtGen_DB_LAYER_DATA")), Some("#RtGen_DB_LAYER_DATA"));
		assert_eq!(pool.get(hash::of_bytes(b"")), Some(""));
	}

	#[test]
	fn a_utf16_pool_counts_characters_not_bytes() {
		let bytes = utf16_pool_bytes(&["Motordrehzahl", "Öldruck"]);
		let pool = Pool::parse_utf16(&bytes).expect("a well-formed pool parses");
		assert_eq!(pool.len(), 2);
		let units: Vec<u16> = "Öldruck".encode_utf16().collect();
		assert_eq!(pool.get(hash::of_utf16(&units)), Some("Öldruck"));
	}

	#[test]
	fn a_truncated_pool_is_an_error_not_a_panic() {
		// A length prefix that promises more bytes than the file holds.
		let mut bytes = ascii_pool_bytes(&["EV_ECM"]);
		bytes.truncate(bytes.len() - 2);
		let err = Pool::parse_ascii(&bytes).expect_err("a short string must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");

		// A length prefix cut in half.
		let err = Pool::parse_ascii(&[1, 0, 0]).expect_err("a short length must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_length_that_overflows_is_an_error() {
		// u32::MAX bytes are promised and none follow.
		let bytes = [0xff, 0xff, 0xff, 0xff];
		let err = Pool::parse_ascii(&bytes).expect_err("an absurd length must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn colliding_strings_both_survive_at_probed_hashes() {
		// The same string twice: the writer stores both, the second at + 11.
		let bytes = ascii_pool_bytes(&["dup", "dup"]);
		let pool = Pool::parse_ascii(&bytes).expect("duplicates are legal");
		assert_eq!(pool.len(), 2, "a duplicate must not overwrite its twin");
		let base = hash::of_bytes(b"dup");
		assert_eq!(pool.get(base), Some("dup"));
		assert_eq!(pool.get(hash::probe(base)), Some("dup"));
	}

	#[test]
	fn windows_1252_high_bytes_decode() {
		// 0x80 is the euro sign, 0xDF is sharp s, 0x81 is undefined.
		assert_eq!(cp1252(&[0x80, 0xDF, 0x81]), "€ß\u{81}");
	}

	#[test]
	fn gunzip_reads_a_member_with_a_name_field() {
		let payload = b"AStringData contents";
		let deflated = miniz_oxide::deflate::compress_to_vec(payload, 6);
		let mut member = vec![0x1f, 0x8b, 8, 1 << 3, 0, 0, 0, 0, 0, 3];
		member.extend_from_slice(b"AStringData.data\0");
		member.extend_from_slice(&deflated);
		// The trailer is present in a real member and deliberately unread.
		member.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0]);
		assert_eq!(gunzip(&member).expect("a valid member inflates"), payload);
	}

	#[test]
	fn gunzip_refuses_what_is_not_gzip() {
		let err = gunzip(b"not a gzip member at all").expect_err("bad magic must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
		let err = gunzip(&[0x1f, 0x8b]).expect_err("a truncated header must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}
}
