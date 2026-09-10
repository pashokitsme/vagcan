//! A read-only reader for a `.key` file: Peter Graf's PBL B+Tree.
//!
//! Ported from the read paths of `pblkf.c` (MIT-licensed C, read for the
//! algorithm — no code copied and no library linked). Every write path was
//! ignored on purpose: **this module has no insert, delete, split or merge**,
//! and there is no `&mut self` anywhere on [`KeyFile`]. A `.key` file belongs
//! to VW's tooling and is never modified by this tool.
//!
//! ## The layout
//! The file is a flat array of 4096-byte blocks, block 0 being the root. Each
//! block opens with a 13-byte header:
//!
//! ```text
//! byte  0     level     u8    0 = leaf, higher = inner node, 255 = overflow data
//! bytes 1-4   nblock    i32BE next block at this level (0 = none)
//! bytes 5-8   pblock    i32BE previous block at this level
//! bytes 9-10  nentries  u16BE how many items the block holds
//! bytes 11-12 free      u16BE offset of the first free byte
//! ```
//!
//! Items grow forward from byte 13; their offsets are stored backward from the
//! end of the block as 2-byte big-endian slots, item `i` at `4096 - 2*(i+1)`.
//! An item is `keylen`, `keycommon`, a variable-length integer, then the last
//! `keylen - keycommon` bytes of the key — the first `keycommon` bytes are
//! shared with the *previous item on the same block* and are not stored again.
//! That prefix compression is why keys must be expanded in slot order and why
//! a block cannot be read from the middle.
//!
//! The variable-length integer is `datablock` on an inner node and `datalen` on
//! a leaf, and on a leaf the data follows inline. PBL also has an overflow path
//! for data longer than [`INLINE_DATA_MAX`], and it is provably unused here:
//! every value VW stores is a 6-, 8- or 12-byte locator into the matching `.db`
//! file. The path is still *refused* rather than ignored, so a file that did
//! use it fails loudly instead of returning wrong bytes.
//!
//! ## The pseudo-item
//! PBL inserts a `keylen == 0` item as the very first record of every file, to
//! hold its magic string. It sorts before any real key and is skipped here —
//! it is bookkeeping, not an object.

use std::path::Path;

use super::Error;

/// Every block is exactly this many bytes. PBL's `PBLDATASIZE`.
pub const BLOCK: usize = 4096;

/// Bytes of header before the first item. PBL's `PBLHEADERSIZE`.
const HEADER: usize = 13;

/// Data longer than this lives on its own overflow block instead of inline.
/// PBL's `PBLDATALENGTH`. VW never crosses it — the values are locators.
const INLINE_DATA_MAX: u32 = 1024;

/// A `.key` file, held whole in memory.
///
/// A key file is at most a few hundred kilobytes even for the largest pool, and
/// reading it once beats seeking per lookup — the traversal touches a block per
/// level and then walks the whole leaf chain.
#[derive(Debug)]
pub struct KeyFile {
	bytes: Vec<u8>,
}

/// One record of the tree: the four-byte key (an [`super::hash`] of an
/// ObjectID) and the bytes stored against it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
	/// The raw key. PBL keys are arbitrary byte strings; VW's are always four
	/// bytes, but nothing here assumes that.
	pub key: Vec<u8>,
	/// The record's data, inline from the leaf.
	pub data: Vec<u8>,
}

/// A parsed block header plus the block's bytes.
struct Block<'a> {
	bytes: &'a [u8],
	level: u8,
	/// Next block at this level; `0` means there is none. Block 0 is the root,
	/// so it can never legitimately be a successor.
	next: u32,
	entries: usize,
}

/// What an item points at, which depends on the block's level.
enum Target<'a> {
	/// An inner node's item: the child block to descend into.
	Child(u32),
	/// A leaf's item: its data, borrowed from the block.
	Data(&'a [u8]),
}

impl KeyFile {
	/// Read a `.key` file.
	pub fn open(path: &Path) -> Result<KeyFile, Error> {
		KeyFile::from_bytes(std::fs::read(path).map_err(Error::Io)?)
	}

	/// Take an already-read `.key` file.
	///
	/// The length must be a whole number of blocks and there must be at least
	/// one, because block 0 is the root and a file without it has no tree.
	pub fn from_bytes(bytes: Vec<u8>) -> Result<KeyFile, Error> {
		if bytes.is_empty() || bytes.len() % BLOCK != 0 {
			return Err(Error::Format(format!(
				"key file is {} bytes, not a whole number of {BLOCK}-byte blocks",
				bytes.len()
			)));
		}
		Ok(KeyFile { bytes })
	}

	/// Every record in the tree, in key order.
	///
	/// Descends to the leftmost leaf and then follows the leaf chain, which is
	/// what PBL's own `pblKfFirst`/`pblKfNext` do. The pseudo-item is skipped.
	pub fn records(&self) -> Result<Vec<Record>, Error> {
		let mut out = Vec::new();
		self.each_record(|key, data| {
			out.push(Record {
				key: key.to_vec(),
				data: data.to_vec(),
			});
			Ok(())
		})?;
		Ok(out)
	}

	/// Visit every record in key order without building a list.
	///
	/// The same walk as [`KeyFile::records`] — leftmost leaf, then the leaf
	/// chain — handed to a closure one `(key, data)` at a time, both borrowed
	/// from the block. This is what an index over a whole pool is built with:
	/// the reference project's engine pool holds 576,793 records, and a list
	/// of owned pairs for each of them is a quarter of a million allocations
	/// that the index would discard on the spot.
	///
	/// Each block is expanded in **one** pass. `keycommon` is relative to the
	/// item before, so walking the slots in order and carrying the previous
	/// key makes every item cost its own bytes — see [`Block::step`].
	pub fn each_record(&self, mut visit: impl FnMut(&[u8], &[u8]) -> Result<(), Error>) -> Result<(), Error> {
		let mut at = self.leftmost_leaf()?;
		// A corrupt `next` chain could loop; a block can be visited at most
		// once, so the block count bounds the walk.
		let mut budget = self.blocks();
		let mut key = Vec::new();
		loop {
			let block = self.block(at)?;
			key.clear();
			for i in 0..block.entries {
				let target = block.step(i, &mut key)?;
				if key.is_empty() {
					continue; // PBL's magic pseudo-item.
				}
				match target {
					Target::Data(data) => visit(&key, data)?,
					Target::Child(_) => return Err(Error::Format(format!("block {at} claims level 0 but its items point at child blocks"))),
				}
			}
			if block.next == 0 {
				return Ok(());
			}
			budget = budget.checked_sub(1).ok_or_else(|| Error::Format("the leaf chain loops".into()))?;
			at = block.next;
		}
	}

	/// The data stored against `key`, or `None` if the tree does not hold it.
	///
	/// Descends the tree rather than scanning: at each inner node the child
	/// taken is the last item whose key is `<= key`, keys compared as byte
	/// strings, which is the ordering PBL builds the tree under.
	pub fn find(&self, key: &[u8]) -> Result<Option<Vec<u8>>, Error> {
		let mut at = 0u32;
		let mut budget = self.blocks();
		let mut item_key = Vec::new();
		loop {
			let block = self.block(at)?;
			item_key.clear();
			if block.level == 0 {
				for i in 0..block.entries {
					let target = block.step(i, &mut item_key)?;
					if item_key == key {
						return match target {
							Target::Data(data) => Ok(Some(data.to_vec())),
							Target::Child(_) => Err(Error::Format(format!("block {at} claims level 0 but its items point at child blocks"))),
						};
					}
				}
				return Ok(None);
			}
			// Inner node: take the rightmost child whose separator is <= key.
			// Item 0 always has an empty key, so a candidate always exists.
			let mut child = None;
			for i in 0..block.entries {
				let target = block.step(i, &mut item_key)?;
				if item_key.as_slice() > key {
					break;
				}
				match target {
					Target::Child(block_no) => child = Some(block_no),
					Target::Data(_) => {
						return Err(Error::Format(format!(
							"block {at} claims level {} but its items carry inline data",
							block.level
						)));
					}
				}
			}
			let Some(next) = child else {
				return Ok(None);
			};
			budget = budget.checked_sub(1).ok_or_else(|| Error::Format("the tree descent loops".into()))?;
			at = next;
		}
	}

	/// How many blocks the file holds.
	fn blocks(&self) -> usize {
		self.bytes.len() / BLOCK
	}

	/// Follow item 0 down from the root until a level-0 block is reached.
	fn leftmost_leaf(&self) -> Result<u32, Error> {
		let mut at = 0u32;
		let mut budget = self.blocks();
		loop {
			let block = self.block(at)?;
			if block.level == 0 {
				return Ok(at);
			}
			let Target::Child(next) = block.step(0, &mut Vec::new())? else {
				return Err(Error::Format(format!(
					"block {at} claims level {} but its first item carries inline data",
					block.level
				)));
			};
			budget = budget.checked_sub(1).ok_or_else(|| Error::Format("the tree descent loops".into()))?;
			at = next;
		}
	}

	/// Parse block `no`'s header.
	fn block(&self, no: u32) -> Result<Block<'_>, Error> {
		let start = (no as usize)
			.checked_mul(BLOCK)
			.ok_or_else(|| Error::Format(format!("block number {no} overflows")))?;
		let bytes = self
			.bytes
			.get(start..start + BLOCK)
			.ok_or_else(|| Error::Format(format!("block {no} is past the end of a {}-block file", self.blocks())))?;
		let level = bytes[0];
		let next = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
		let entries = usize::from(u16::from_be_bytes([bytes[9], bytes[10]]));
		// Each item costs a 2-byte slot at the end plus at least 5 bytes of
		// item (PBL's PBL_MINIMAL_ITEM_SIZE), so a block physically cannot
		// hold more than this. A header claiming more is a corrupt file, and
		// trusting it would read slots out of the item area.
		let ceiling = (BLOCK - HEADER) / (2 + 3);
		if entries > ceiling {
			return Err(Error::Format(format!(
				"block {no} claims {entries} entries, more than the {ceiling} a {BLOCK}-byte block can hold"
			)));
		}
		Ok(Block { bytes, level, next, entries })
	}
}

impl Block<'_> {
	/// Expand item `index`'s key into `key` and say what the item points at.
	///
	/// `key` must hold item `index - 1`'s key on the way in — or nothing, for
	/// item 0 — because `keycommon` is relative to the immediate predecessor
	/// and only the suffix past it is stored. Walking a block's slots in order
	/// and carrying the key between calls therefore reads the block in one
	/// pass, each item costing its own bytes.
	///
	/// That is the cost model of this whole file. The version of this that
	/// re-expanded from slot 0 on every call was `O(n²)` per block and, under a
	/// leaf scanned item by item, `O(n³)` per lookup; the measurement chain
	/// looks up a few million objects by key, and that one function was 96%
	/// of a three-and-a-half-minute `vagcan setup` (profiled 2026-09-10).
	fn step(&self, index: usize, key: &mut Vec<u8>) -> Result<Target<'_>, Error> {
		let at = self.slot(index)?;
		let (keylen, keycommon) = (usize::from(*self.byte(at)?), usize::from(*self.byte(at + 1)?));
		if keycommon > keylen {
			return Err(Error::Format(format!(
				"item {index} shares {keycommon} bytes with a predecessor but is only {keylen} bytes long"
			)));
		}
		if keycommon > key.len() {
			return Err(Error::Format(format!(
				"item {index} shares {keycommon} bytes with a predecessor that is only {} bytes long",
				key.len()
			)));
		}
		let (value, used) = varint(self.bytes, at + 2)?;
		let stored = keylen - keycommon;
		let suffix_at = at + 2 + used;
		let suffix = self
			.bytes
			.get(suffix_at..suffix_at + stored)
			.ok_or_else(|| Error::Format(format!("item {index}'s key runs past the end of its block")))?;
		key.truncate(keycommon);
		key.extend_from_slice(suffix);

		if self.level > 0 {
			return Ok(Target::Child(value));
		}
		if value > INLINE_DATA_MAX {
			// PBL's overflow chain. VW stores only 6/8/12-byte locators, so
			// this cannot happen in a project — and if it ever did, saying
			// so beats handing back the block bytes that follow.
			return Err(Error::Format(format!(
				"item {index} stores {value} bytes on an overflow block, which this reader does not follow"
			)));
		}
		let data_at = suffix_at + stored;
		let data = self
			.bytes
			.get(data_at..data_at + value as usize)
			.ok_or_else(|| Error::Format(format!("item {index}'s data runs past the end of its block")))?;
		Ok(Target::Data(data))
	}

	/// The byte offset of item `index`, read from the backward slot array.
	fn slot(&self, index: usize) -> Result<usize, Error> {
		if index >= self.entries {
			return Err(Error::Format(format!("item {index} asked for on a block holding {}", self.entries)));
		}
		let at = BLOCK - 2 * (index + 1);
		let offset = usize::from(u16::from_be_bytes([self.bytes[at], self.bytes[at + 1]]));
		// Items live strictly between the header and the slot array, and the
		// slot array is the last `2 * entries` bytes of the block. Checking
		// only against this slot's own position would leave an item free to
		// start inside a later slot, which reads slot bytes as item bytes.
		if offset < HEADER || offset >= BLOCK - 2 * self.entries {
			return Err(Error::Format(format!(
				"item {index} claims to start at byte {offset}, outside its block's item area"
			)));
		}
		Ok(offset)
	}

	/// One byte of the block, bounds-checked.
	fn byte(&self, at: usize) -> Result<&u8, Error> {
		self
			.bytes
			.get(at)
			.ok_or_else(|| Error::Format("an item runs past the end of its block".into()))
	}
}

/// Read PBL's self-describing variable-length integer at `at`.
///
/// The first byte's high bits say how many bytes the value occupies: `0xxxxxxx`
/// is one byte, `10xxxxxx` two, `110xxxxx` three, `1110xxxx` four, and `1111`
/// means a plain four-byte big-endian value follows. The payload bits of the
/// first byte are the value's high bits in the first four forms. This is
/// `pbl_VarBufToLong`.
///
/// Returns the value and how many bytes it took.
fn varint(bytes: &[u8], at: usize) -> Result<(u32, usize), Error> {
	let short = || Error::Format(format!("a variable-length integer at byte {at} runs past the end of its block"));
	let first = u32::from(*bytes.get(at).ok_or_else(short)?);
	let byte = |i: usize| -> Result<u32, Error> { Ok(u32::from(*bytes.get(at + i).ok_or_else(short)?)) };
	if first & 0x80 == 0 {
		return Ok((first, 1));
	}
	if first & 0x40 == 0 {
		return Ok((((first & 0x3f) << 8) | byte(1)?, 2));
	}
	if first & 0x20 == 0 {
		return Ok((((first & 0x1f) << 16) | (byte(1)? << 8) | byte(2)?, 3));
	}
	if first & 0x10 == 0 {
		return Ok((((first & 0x0f) << 24) | (byte(1)? << 16) | (byte(2)? << 8) | byte(3)?, 4));
	}
	Ok(((byte(1)? << 24) | (byte(2)? << 16) | (byte(3)? << 8) | byte(4)?, 5))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Encode a value the way `pbl_LongToVarBuf` does — the writer this
	/// module's [`varint`] is the reader for.
	fn put_varint(out: &mut Vec<u8>, value: u32) {
		match value {
			0..=0x7f => out.push(value as u8),
			0x80..=0x3fff => out.extend_from_slice(&[((value >> 8) | 0x80) as u8, value as u8]),
			0x4000..=0x1f_ffff => out.extend_from_slice(&[((value >> 16) | 0xc0) as u8, (value >> 8) as u8, value as u8]),
			0x20_0000..=0x0fff_ffff => out.extend_from_slice(&[((value >> 24) | 0xe0) as u8, (value >> 16) as u8, (value >> 8) as u8, value as u8]),
			_ => {
				out.push(0xf0);
				out.extend_from_slice(&value.to_be_bytes());
			}
		}
	}

	/// Lay out one block from items given as `(key, target)`, applying the
	/// prefix compression the format uses. `level` picks how the varint is
	/// read back: `0` makes it a data length, higher a child block number.
	fn block(level: u8, next: u32, items: &[(&[u8], Target<'_>)]) -> Vec<u8> {
		let mut out = vec![0u8; BLOCK];
		out[0] = level;
		out[1..5].copy_from_slice(&next.to_be_bytes());
		out[9..11].copy_from_slice(&(items.len() as u16).to_be_bytes());
		let mut at = HEADER;
		let mut prev: &[u8] = b"";
		for (i, (key, target)) in items.iter().enumerate() {
			let common = key.iter().zip(prev.iter()).take_while(|(a, b)| a == b).count().min(255);
			let mut item = vec![key.len() as u8, common as u8];
			match target {
				Target::Child(no) => {
					put_varint(&mut item, *no);
					item.extend_from_slice(&key[common..]);
				}
				Target::Data(data) => {
					put_varint(&mut item, data.len() as u32);
					item.extend_from_slice(&key[common..]);
					item.extend_from_slice(data);
				}
			}
			out[at..at + item.len()].copy_from_slice(&item);
			let slot = BLOCK - 2 * (i + 1);
			out[slot..slot + 2].copy_from_slice(&(at as u16).to_be_bytes());
			at += item.len();
			prev = key;
		}
		out[11..13].copy_from_slice(&(at as u16).to_be_bytes());
		out
	}

	/// PBL's `keylen == 0` magic pseudo-item, present as record 0 of every file.
	fn pseudo() -> (&'static [u8], Target<'static>) {
		(b"", Target::Data(b"1.00 Peter's B Tree\0"))
	}

	#[test]
	fn varint_round_trips_every_width() {
		for value in [
			0u32,
			1,
			0x7f,
			0x80,
			0x3fff,
			0x4000,
			0x1f_ffff,
			0x20_0000,
			0x0fff_ffff,
			0x1000_0000,
			u32::MAX,
		] {
			let mut buf = Vec::new();
			put_varint(&mut buf, value);
			let (read, used) = varint(&buf, 0).expect("a well-formed varint reads");
			assert_eq!((read, used), (value, buf.len()), "round trip of {value:#x}");
		}
	}

	#[test]
	fn a_varint_running_past_the_buffer_is_an_error() {
		// A two-byte form with only its first byte present.
		let err = varint(&[0x80], 0).expect_err("a truncated varint must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn one_leaf_block_yields_its_records() {
		let file = block(
			0,
			0,
			&[
				pseudo(),
				(b"\x01\x00\x00\x00", Target::Data(&[1, 2, 3])),
				(b"\x02\x00\x00\x00", Target::Data(&[4, 5])),
			],
		);
		let kf = KeyFile::from_bytes(file).expect("one block is a whole file");
		let records = kf.records().expect("a well-formed block reads");
		assert_eq!(records.len(), 2, "the magic pseudo-item must not be reported as a record");
		assert_eq!(
			records[0],
			Record {
				key: b"\x01\x00\x00\x00".to_vec(),
				data: vec![1, 2, 3]
			}
		);
		assert_eq!(
			records[1],
			Record {
				key: b"\x02\x00\x00\x00".to_vec(),
				data: vec![4, 5]
			}
		);
	}

	#[test]
	fn successive_keys_are_prefix_compressed() {
		// "New Haven" then "New York": the second stores only "York" and a
		// keycommon of 4. Expanding it wrongly is the classic failure here.
		let file = block(
			0,
			0,
			&[
				pseudo(),
				(b"New Haven", Target::Data(&[1])),
				(b"New York", Target::Data(&[2])),
				(b"New Yorker", Target::Data(&[3])),
			],
		);
		let kf = KeyFile::from_bytes(file).expect("one block is a whole file");
		let keys: Vec<Vec<u8>> = kf.records().expect("a well-formed block reads").into_iter().map(|r| r.key).collect();
		assert_eq!(keys, vec![b"New Haven".to_vec(), b"New York".to_vec(), b"New Yorker".to_vec()]);
	}

	/// Root over two leaves, chained by `nblock` — the shape `records` walks.
	fn two_level_tree() -> KeyFile {
		let root = block(1, 0, &[(b"", Target::Child(1)), (b"\x02\x00\x00\x00", Target::Child(2))]);
		let leaf_a = block(0, 2, &[pseudo(), (b"\x01\x00\x00\x00", Target::Data(&[0xaa]))]);
		let leaf_b = block(
			0,
			0,
			&[(b"\x02\x00\x00\x00", Target::Data(&[0xbb])), (b"\x03\x00\x00\x00", Target::Data(&[0xcc]))],
		);
		let mut bytes = root;
		bytes.extend_from_slice(&leaf_a);
		bytes.extend_from_slice(&leaf_b);
		KeyFile::from_bytes(bytes).expect("three blocks is a whole file")
	}

	#[test]
	fn a_two_level_tree_walks_its_leaf_chain() {
		let kf = two_level_tree();
		let records = kf.records().expect("a well-formed tree reads");
		let keys: Vec<Vec<u8>> = records.iter().map(|r| r.key.clone()).collect();
		assert_eq!(
			keys,
			vec![b"\x01\x00\x00\x00".to_vec(), b"\x02\x00\x00\x00".to_vec(), b"\x03\x00\x00\x00".to_vec()]
		);
	}

	#[test]
	fn find_descends_to_the_right_leaf() {
		let kf = two_level_tree();
		assert_eq!(kf.find(b"\x01\x00\x00\x00").expect("a well-formed tree reads"), Some(vec![0xaa]));
		assert_eq!(kf.find(b"\x03\x00\x00\x00").expect("a well-formed tree reads"), Some(vec![0xcc]));
	}

	#[test]
	fn find_on_an_absent_key_is_none_not_an_error() {
		let kf = two_level_tree();
		// Between the two leaves' ranges, and past the end of the last one.
		assert_eq!(kf.find(b"\x01\x80\x00\x00").expect("a well-formed tree reads"), None);
		assert_eq!(kf.find(b"\xff\xff\xff\xff").expect("a well-formed tree reads"), None);
	}

	#[test]
	fn a_block_claiming_more_entries_than_it_can_hold_is_refused() {
		let mut file = block(0, 0, &[pseudo(), (b"\x01\x00\x00\x00", Target::Data(&[1]))]);
		file[9..11].copy_from_slice(&4000u16.to_be_bytes());
		let kf = KeyFile::from_bytes(file).expect("one block is a whole file");
		let err = kf.records().expect_err("an impossible entry count must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_slot_pointing_outside_the_item_area_is_refused() {
		let mut file = block(0, 0, &[pseudo(), (b"\x01\x00\x00\x00", Target::Data(&[1]))]);
		// Point item 1's slot into the slot array itself, where an item's bytes
		// would be read out of the offsets rather than out of the item area.
		let slot = BLOCK - 4;
		file[slot..slot + 2].copy_from_slice(&((BLOCK - 2) as u16).to_be_bytes());
		let kf = KeyFile::from_bytes(file).expect("one block is a whole file");
		let err = kf.records().expect_err("a slot inside the slot array must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_slot_pointing_into_the_header_is_refused() {
		let mut file = block(0, 0, &[pseudo(), (b"\x01\x00\x00\x00", Target::Data(&[1]))]);
		let slot = BLOCK - 4;
		file[slot..slot + 2].copy_from_slice(&5u16.to_be_bytes());
		let kf = KeyFile::from_bytes(file).expect("one block is a whole file");
		let err = kf.records().expect_err("a slot inside the header must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	#[test]
	fn a_file_that_is_not_whole_blocks_is_refused() {
		assert!(matches!(KeyFile::from_bytes(vec![0; 100]), Err(Error::Format(_))));
		assert!(matches!(KeyFile::from_bytes(Vec::new()), Err(Error::Format(_))));
	}

	#[test]
	fn a_child_pointer_past_the_end_of_the_file_is_refused() {
		let root = block(1, 0, &[(b"", Target::Child(99))]);
		let kf = KeyFile::from_bytes(root).expect("one block is a whole file");
		let err = kf.records().expect_err("a dangling child must be refused");
		assert!(matches!(err, Error::Format(_)), "got {err:?}");
	}

	/// The whole public surface, exercised through a shared reference.
	///
	/// This is the executable form of the module's central claim: reading a
	/// `.key` file needs no `&mut` anywhere, so there is no place an insert or
	/// a delete could be added without changing a signature. It is paired with
	/// the byte-for-byte check below, which catches interior mutability too.
	fn read_only_surface(kf: &KeyFile) {
		let _ = kf.records().expect("a well-formed tree reads");
		let _ = kf.find(b"\x01\x00\x00\x00").expect("a well-formed tree reads");
	}

	#[test]
	fn traversal_never_modifies_the_file() {
		let root = block(1, 0, &[(b"", Target::Child(1)), (b"\x02\x00\x00\x00", Target::Child(2))]);
		let leaf_a = block(0, 2, &[pseudo(), (b"\x01\x00\x00\x00", Target::Data(&[0xaa]))]);
		let leaf_b = block(0, 0, &[(b"\x02\x00\x00\x00", Target::Data(&[0xbb]))]);
		let mut bytes = root;
		bytes.extend_from_slice(&leaf_a);
		bytes.extend_from_slice(&leaf_b);

		let before = bytes.clone();
		let kf = KeyFile::from_bytes(bytes).expect("three blocks is a whole file");
		read_only_surface(&kf);
		read_only_surface(&kf);
		assert_eq!(kf.bytes, before, "traversing a key file must leave its bytes untouched");
	}
}
