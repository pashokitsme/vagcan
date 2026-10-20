//! Decoder for Ross-Tech VCDS `.rod` files (`UDS_EV/` UDS/ODX data).
//!
//! `.rod` uses the same TEA cipher family as `.clb` (see [`crate::tea`]), but
//! wraps records in an ASCII section-tag container (`[TAG]\r\n<payload>\r\n
//! [/TAG]\r\n`) and layers zlib compression on top of some sections'
//! plaintext. This module only DECODES sections to raw text; it does not
//! interpret the content.
//!
//! ## Scope
//! - `MWB` section rows are `<row>,<code>`: a 1-based row number into the
//!   global measurement registry `RM.rod`, where the row's DID, layout and
//!   scaling are. [`crate::registry`] follows that join; this module only
//!   opens the containers.
//! - `.rod` is NOT ingested into [`crate::LabelDb`] / the SQLite label files: its
//!   ID-indexed data model doesn't fit the block/field `.lbl`/`.clb` model.
//!   [`decode_rod`] is a standalone decoder.

use crate::tea::tea_cbc_decrypt;

pub(crate) const KEY_ROD: [u32; 4] = [0x029b_76a4, 0xcb6d_b50a, 0x7139_5d29, 0x0dbc_09c2];
pub(crate) const OFF_ROD: [usize; 8] = [0x07, 0xca, 0x22, 0x99, 0x3e, 0x88, 0xc3, 0x76];

pub(crate) static MT: &[u8; 256] = include_bytes!("rod_mt.bin");
pub(crate) static KS: &[u8; 256] = include_bytes!("rod_ks.bin");

/// How a [`RodSection`]'s payload was decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RodStatus {
	/// Decrypted (TEA-CBC) but not compressed.
	Tea,
	/// Decrypted then zlib-inflated.
	Zlib,
	/// The key search never ran, because its crib is absent.
	///
	/// Recovering a blocked first block is a known-plaintext attack and the
	/// known plaintext is the zlib header: the first two bytes must decrypt to
	/// `78 da` under the tag-derived IV, or there is nothing to search against.
	/// In **40 % of the label files** they do not — those files carry a per-file XOR
	/// on the first-block IV of every section after `[CMP]`
	/// (`.archive/research/labels/tttext2.md`).
	///
	/// Reported apart from [`RodStatus::Undecodable`] because the two say
	/// opposite things. A file that is undecodable has been tried; one that is
	/// declined has not, and reading the second as the first is how
	/// `TTTEXT2.ROD` sat "uncrackable" through four writeups that named it as
	/// the next thing to open.
	SearchDeclined,
	/// Could not be decoded: bad framing, a key the file's name does not give
	/// and nothing cached (a file under another name, or [`decode_rod`], which
	/// knows no name), a short/misaligned cipher, or an inflate failure.
	Undecodable,
}

/// One `[TAG]...[/TAG]` section of a `.rod` file, decoded (or not).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RodSection {
	pub tag: String,
	pub status: RodStatus,
	pub text: Option<String>,
}

/// IV for a `.rod` record's first TEA-CBC block, in the `product = 0` case —
/// a file with no name, which is what [`decode_rod`] decodes as.
/// `IV[0..3]` comes from the tag alone and is the same for every `product`,
/// which is what the search and the shift test lean on.
/// `tag` must be at least 3 bytes (all real section tags are 2-4 uppercase
/// ASCII letters, so this always holds for tags accepted by the framing
/// scanner below).
pub(crate) fn rod_block0_iv(tag: &[u8]) -> [u8; 8] {
	let m = tag[1] as usize;
	// seed = tag[0:3] ++ 5 zero bytes (product = 0)
	let mut seed = [0u8; 8];
	seed[..3].copy_from_slice(&tag[..3]);
	let mut s = [0u8; 8];
	for i in 0..8 {
		s[i] = seed[i].wrapping_add(KS[(m * (i + 2)) & 0xff]);
	}
	let mut iv = [0u8; 8];
	for i in 0..8 {
		iv[i] = s[i].wrapping_mul(MT[OFF_ROD[i]]);
	}
	iv
}

/// The first-block IV for a section under a given 5-byte `product`: the seed is
/// the tag's first three bytes and then those five, each byte shifted by `KS`
/// and multiplied by `MT` (`.archive/research/labels/rod-labels.md` §1.1).
/// With five zero bytes this is [`rod_block0_iv`].
pub(crate) fn rod_iv(tag: &[u8], product5: [u8; 5]) -> [u8; 8] {
	let m = tag[1] as usize;
	let mut seed = [0u8; 8];
	seed[..3].copy_from_slice(&tag[..3]);
	seed[3..].copy_from_slice(&product5);
	std::array::from_fn(|i| seed[i].wrapping_add(KS[(m * (i + 2)) & 0xff]).wrapping_mul(MT[OFF_ROD[i]]))
}

/// The `product` VCDS seeds every section of a file with, from the file's name.
///
/// **It is the file name** (2026-10-20). What `rod-labels.md` §1.1 called a
/// runtime record string is the name VCDS opened the file under, cut at its
/// first `.` — so `EV_ECM20TFS.rod` gives `EV_ECM20TFS`, and `TTText-RUS.rod`
/// gives `TTTEXT-RUS`. The stem is upper-cased, and the product is its first
/// character times the product of all of them: the accumulator starts at the
/// first character rather than at one, so that character counts twice. The
/// characters are signed, as VCDS multiplies them; every name in an
/// installation is ASCII, where that changes nothing. A name with an empty
/// stem has product 0.
///
/// Checked against the 57 keys brute force had recovered for the reference
/// install (57 of 57), and on files brute force never opened. A path is
/// accepted: only its last component counts.
pub fn name_product(file: &str) -> u64 {
	let base = file.rsplit(['/', '\\']).next().unwrap_or(file);
	let stem = base.split('.').next().unwrap_or_default().as_bytes();
	let signed = |c: u8| c.to_ascii_uppercase() as i8 as i64 as u64;
	match stem.first() {
		Some(&first) => stem.iter().fold(signed(first), |product, &c| product.wrapping_mul(signed(c))),
		None => 0,
	}
}

/// The low five bytes of a `product`, as the IV seed takes them.
fn product5(product: u64) -> [u8; 5] {
	let bytes = product.to_le_bytes();
	[bytes[0], bytes[1], bytes[2], bytes[3], bytes[4]]
}

/// The first-block IV of section `tag` in the file named `file`, before any
/// per-file mask (`.archive/research/labels/tttext2.md` §3.3a).
///
/// VCDS also adds a byte `C` to every seed byte when the file is `TTTEXT` or
/// `UNIT` (tttext2.md §3.3a). It is zero in every install checked: both files
/// open with this IV.
pub(crate) fn rod_named_iv(tag: &[u8], file: &str) -> [u8; 8] {
	rod_iv(tag, product5(name_product(file)))
}

/// Build the first-block IV for a section whose per-record `product` term is
/// nonzero, given the recovered raw `iv[3..8]` (5 bytes, e.g. from the
/// `crack` module or a runtime dump). `iv[0..3]` is always tag-derived (it does
/// not depend on `product`), so we reuse the `product = 0` derivation for those
/// three bytes and splice the recovered five over the (wrong, product-0) tail.
fn rod_block0_iv_recovered(tag: &[u8], iv3to8: [u8; 5]) -> [u8; 8] {
	let mut iv = rod_block0_iv(tag);
	iv[3..8].copy_from_slice(&iv3to8);
	iv
}

/// Every value deflate byte 0 can take at the head of a zlib stream in this
/// label files: `BTYPE = 2` (dynamic Huffman), `HLIT ≤ 29`, either `BFINAL`.
///
/// Sixty values, and they matter because a shifted file (`.archive/research/labels/tttext2.md`
/// §3.3) destroys exactly this byte. The layout is RFC 1951 §3.2.7:
/// `BFINAL | BTYPE << 1 | HLIT << 3`. Stored and fixed-Huffman first blocks are
/// excluded, which **is a known gap, not a property of the data**: this comment
/// used to claim no section used either, and a census of the whole corpus
/// refuted it — **1,559 of 22,107 classic sections (7.1 %) open with a fixed
/// block** (`0x33`/`0xb3`), so roughly a thousand shifted sections cannot be
/// opened by this sweep at all (`.archive/research/labels/tttext2.md` §5). Admitting them
/// doubles a search that is already the expensive part, which is why the
/// widening lives behind the research driver's `--all-btypes` rather than here
/// — but the reason is cost, and it was never that the blocks do not occur.
pub(crate) fn deflate_anchors() -> impl Iterator<Item = u8> {
	(0..=29u8).flat_map(|hlit| [0b100 | (hlit << 3), 0b101 | (hlit << 3)])
}

/// Decrypt and inflate a section under `iv`, insisting the result is exactly
/// the declared length.
///
/// The length check is what makes the anchor sweep below safe: a wrong anchor
/// almost always fails to inflate at all, but "almost always" is not a decoder
/// guarantee, and a stream that inflates to the wrong size is a wrong stream.
fn inflate_with_iv(sc: &SectionCipher<'_>, iv: [u8; 8]) -> Option<Vec<u8>> {
	let dec = tea_cbc_decrypt(sc.cipher, &KEY_ROD, iv);
	let out = miniz_oxide::inflate::decompress_to_vec_zlib(&dec).ok()?;
	(out.len() == sc.plainlen).then_some(out)
}

/// Decode a compressed section whose first-block IV carries the per-file XOR of
/// `.archive/research/labels/tttext2.md` §3.3, given the recovered `iv[3..8]`.
///
/// Two of the three shifted bytes cost nothing, because the plaintext there is
/// known: `iv[0] = t[0] ^ 0x78` and `iv[1] = t[1] ^ 0xda`. The third sits under
/// deflate byte 0, which nothing pins — so it is swept over
/// [`deflate_anchors`] and the value that inflates to the declared length wins.
///
/// This is cheap (sixty decrypt-and-inflate attempts, most of which die in the
/// Huffman header) and it is the reason the key cache needs no new format: a
/// shifted section is openable from the same five bytes as a classic one.
///
/// Also returns the section's whole first-block IV. It is worth carrying because
/// the mask is a property of the **file**: the IV XOR the name's IV is the mask,
/// all eight bytes of it, and with it every other section of the same file opens
/// with no search at all.
fn decode_shifted(tag_str: &str, sc: &SectionCipher<'_>, iv3to8: [u8; 5]) -> Option<(RodSection, [u8; 8])> {
	let first: [u8; 8] = sc.cipher.get(0..8)?.try_into().ok()?;
	let t = crate::tea::tea_decrypt_block(first, &KEY_ROD);
	let mut iv = [0u8; 8];
	iv[0] = t[0] ^ 0x78;
	iv[1] = t[1] ^ 0xda;
	iv[3..8].copy_from_slice(&iv3to8);
	deflate_anchors().find_map(|d0| {
		iv[2] = t[2] ^ d0;
		inflate_with_iv(sc, iv).map(|bytes| {
			let section = RodSection {
				tag: tag_str.to_string(),
				status: RodStatus::Zlib,
				text: Some(decode_latin1(&bytes)),
			};
			(section, iv)
		})
	})
}

/// Decode raw Latin-1 bytes into a `String`, one byte per `char`. Matches
/// `label::parse_label`'s internal decoding, and `clb`'s test helper of the
/// same name.
pub(crate) fn decode_latin1(bytes: &[u8]) -> String {
	bytes.iter().map(|&b| b as char).collect()
}

fn be24(b: &[u8]) -> u32 {
	((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32)
}

/// Scan `data` from `from` for the next well-formed section opener: `[` +
/// 2-8 uppercase ASCII letters + `]\r\n`. Returns the tag bytes and the
/// index of the first payload byte (right after the opener).
///
/// The upper bound was widened from 4 to 8 to admit the 5-letter `STRUC`
/// container tag (`STRUC.rod`); all shorter tags (`CMP`, `MWB`, `TXT`, …)
/// still match.
pub(crate) fn find_next_tag(data: &[u8], from: usize) -> Option<(Vec<u8>, usize)> {
	let mut i = from;
	while i < data.len() {
		if data[i] == b'[' {
			let tag_start = i + 1;
			let mut j = tag_start;
			while j < data.len() && data[j].is_ascii_uppercase() && j - tag_start < 8 {
				j += 1;
			}
			let tag_len = j - tag_start;
			if (2..=8).contains(&tag_len) && j + 2 < data.len() && data[j] == b']' && data[j + 1] == b'\r' && data[j + 2] == b'\n' {
				return Some((data[tag_start..j].to_vec(), j + 3));
			}
		}
		i += 1;
	}
	None
}

/// Find `\r\n[/TAG]\r\n` at or after `from`. Returns `(payload_end,
/// next_scan_pos)`: `payload_end` is the index right before the closing
/// marker's leading `\r\n`, `next_scan_pos` is the index right after the
/// whole closing marker.
pub(crate) fn find_close(data: &[u8], from: usize, tag: &[u8]) -> Option<(usize, usize)> {
	let mut marker = Vec::with_capacity(tag.len() + 6);
	marker.extend_from_slice(b"\r\n[/");
	marker.extend_from_slice(tag);
	marker.extend_from_slice(b"]\r\n");
	let hay = &data[from..];
	if marker.is_empty() || marker.len() > hay.len() {
		return None;
	}
	let idx = hay.windows(marker.len()).position(|w| w == marker.as_slice())?;
	let payload_end = from + idx;
	Some((payload_end, payload_end + marker.len()))
}

/// A parsed `.rod` section header + the ciphertext slice it frames.
pub(crate) struct SectionCipher<'a> {
	/// zlib-compressed (`read1` flag clear) vs plain TEA.
	pub(crate) compressed: bool,
	/// Declared decompressed / plaintext length.
	pub(crate) plainlen: usize,
	/// The `storedlen`-byte ciphertext (already length/alignment-validated).
	pub(crate) cipher: &'a [u8],
}

/// Parse the standard 6-byte `.rod` section header (two BE24 ints) and return
/// the framed ciphertext slice, or `None` if the framing is malformed.
pub(crate) fn parse_section_cipher(payload: &[u8]) -> Option<SectionCipher<'_>> {
	if payload.len() < 6 {
		return None;
	}
	let read1 = be24(&payload[0..3]);
	let storedlen = (read1 & 0x7f_ffff) as usize;
	let compressed = (read1 & 0x80_0000) == 0;
	let plainlen = be24(&payload[3..6]) as usize;
	if storedlen % 8 != 0 || payload.len() < 6 + storedlen {
		return None;
	}
	Some(SectionCipher {
		compressed,
		plainlen,
		cipher: &payload[6..6 + storedlen],
	})
}

/// Decode a section's ciphertext with an explicit first-block IV.
fn decode_with_iv(tag_str: &str, sc: &SectionCipher<'_>, iv: [u8; 8]) -> RodSection {
	let undecodable = || RodSection {
		tag: tag_str.to_string(),
		status: RodStatus::Undecodable,
		text: None,
	};
	let dec = tea_cbc_decrypt(sc.cipher, &KEY_ROD, iv);
	if !sc.compressed {
		if sc.plainlen > dec.len() {
			return undecodable();
		}
		RodSection {
			tag: tag_str.to_string(),
			status: RodStatus::Tea,
			text: Some(decode_latin1(&dec[..sc.plainlen])),
		}
	} else {
		match miniz_oxide::inflate::decompress_to_vec_zlib(&dec) {
			Ok(bytes) => RodSection {
				tag: tag_str.to_string(),
				status: RodStatus::Zlib,
				text: Some(decode_latin1(&bytes)),
			},
			Err(_) => undecodable(),
		}
	}
}

/// Decode one section's raw payload bytes (as captured between the `[TAG]`
/// and `[/TAG]` markers) into a [`RodSection`], using the `product = 0` IV —
/// the key of a file with no name. A real file's sections are keyed by its
/// name; [`open_section`] and [`decode_rod_recover`] take it.
fn decode_section(tag: &[u8], payload: &[u8]) -> RodSection {
	let tag_str = decode_latin1(tag);
	if tag.len() < 3 {
		return RodSection {
			tag: tag_str,
			status: RodStatus::Undecodable,
			text: None,
		};
	}
	match parse_section_cipher(payload) {
		Some(sc) => decode_with_iv(&tag_str, &sc, rod_block0_iv(tag)),
		None => RodSection {
			tag: tag_str,
			status: RodStatus::Undecodable,
			text: None,
		},
	}
}

/// Decode a `.rod` file into its sections **as a file with no name**
/// (`product = 0`). Every real file's key is its name ([`name_product`]), so
/// for a file off disk use [`decode_rod_recover`] or [`open_section`], which
/// take it; here its compressed sections come back `Undecodable` with
/// `text: None`, as do malformed ones (bad framing, short/misaligned cipher,
/// inflate failure). Never panics on malformed input.
pub fn decode_rod(data: &[u8]) -> Vec<RodSection> {
	let mut sections = Vec::new();
	let mut pos = 0usize;
	while let Some((tag, payload_start)) = find_next_tag(data, pos) {
		match find_close(data, payload_start, &tag) {
			Some((payload_end, next_pos)) => {
				let payload = &data[payload_start..payload_end];
				sections.push(decode_section(&tag, payload));
				pos = next_pos;
			}
			None => {
				// Unterminated section (truncated file / bad framing): we
				// can't tell where it ends, so emit it as Undecodable and
				// stop scanning rather than guess.
				sections.push(RodSection {
					tag: decode_latin1(&tag),
					status: RodStatus::Undecodable,
					text: None,
				});
				break;
			}
		}
	}
	sections
}

// ---------------------------------------------------------------------------
// Recovered-IV cache + `product != 0` recovery
// ---------------------------------------------------------------------------

/// A persistent cache of recovered first-block `iv[3..8]` values, keyed by
/// `(file, tag)`. It holds only keys the file name does not give
/// ([`name_product`]): a shifted file's — one per file is enough, it opens the
/// rest — and a section under a name that is not its own. Each cost a search of
/// minutes to hours, so it is cached to disk. Serialized as a JSON object of
/// `"<file>\t<tag>" -> [5 bytes]`. Caches written before the name was known
/// also hold classic keys; those are still read, and never needed.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct IvCache {
	#[serde(flatten)]
	entries: std::collections::BTreeMap<String, [u8; 5]>,
}

impl IvCache {
	fn key(file: &str, tag: &str) -> String {
		format!("{file}\t{tag}")
	}

	/// Load a cache from `path`, or an empty cache if it does not exist / is
	/// unreadable — which [`Self::save`] then refuses to write back over it.
	pub fn load(path: &std::path::Path) -> Self {
		std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
	}

	/// Persist the cache to `path`, whole or not at all.
	///
	/// Every key in it cost minutes of every core, and a file that does not read
	/// loads as an empty cache: saving that over it would erase them all. So a
	/// file that is there and cannot be read, or is not a key cache, is left
	/// alone and the save refused. The new file is written beside the old one and
	/// renamed over it, so an interrupted save leaves the old one whole.
	pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
		let unusable = match std::fs::read(path) {
			Ok(bytes) => !bytes.is_empty() && serde_json::from_slice::<Self>(&bytes).is_err(),
			Err(e) => e.kind() != std::io::ErrorKind::NotFound,
		};
		if unusable {
			return Err(std::io::Error::new(
				std::io::ErrorKind::InvalidData,
				format!("{} is there and does not read as a key cache, so it is left as it is", path.display()),
			));
		}
		let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
		let staged = path.with_file_name(format!(".{name}.saving"));
		std::fs::write(&staged, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
		std::fs::rename(&staged, path)
	}

	/// How many keys the cache holds.
	pub fn len(&self) -> usize {
		self.entries.len()
	}

	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}

	pub fn get(&self, file: &str, tag: &str) -> Option<[u8; 5]> {
		self.entries.get(&Self::key(file, tag)).copied()
	}

	pub fn insert(&mut self, file: &str, tag: &str, iv3to8: [u8; 5]) {
		self.entries.insert(Self::key(file, tag), iv3to8);
	}

	/// Take in every key `other` holds, its key winning where both hold one.
	pub fn absorb(&mut self, other: IvCache) {
		self.entries.extend(other.entries);
	}

	/// The keys this cache holds that `base` does not, or holds otherwise: what a
	/// copy handed to a parallel lookup found. Taking in only these keeps one
	/// lookup's stale copy of a key from undoing another's fresh one.
	pub fn since(&self, base: &IvCache) -> IvCache {
		IvCache {
			entries: self
				.entries
				.iter()
				.filter(|(k, v)| base.entries.get(*k) != Some(*v))
				.map(|(k, v)| (k.clone(), *v))
				.collect(),
		}
	}
}

/// The brute-force IV recovery.
///
/// Recovery costs about a minute of every core per section, and nothing in the
/// live path can use it: reading a car needs the *answer*, which is a five-byte
/// value in the project's `rod-keys.json`, not the search that produced it. So
/// the search ships in `vagcan dev vcds rod` and the CLI links only the cache lookup
/// on the live path.
pub(crate) mod crack;

/// Recover the first-block `iv[3..8]` for a single `product != 0` zlib section,
/// given its raw framed payload (the bytes between `[TAG]` and `[/TAG]`).
/// Returns `None` if the section is not a compressed section, is malformed, or
/// the search fails. CPU-heavy (multithreaded brute force, ~1 min per section).
pub fn recover_zlib_iv3to8(tag: &[u8], payload: &[u8]) -> Option<[u8; 5]> {
	if tag.len() < 3 {
		return None;
	}
	let sc = parse_section_cipher(payload)?;
	if !sc.compressed {
		return None;
	}
	crack::recover_iv3to8(tag, sc.cipher, sc.plainlen, None)
}

/// Whether the key search has anything to search against.
///
/// The search is a known-plaintext attack and the known plaintext is the zlib
/// header. `iv[0..3]` is derived from the section tag alone and does not depend
/// on the missing `product`, so the first two plaintext bytes can be checked
/// before any work is done — and if they are not `78 da`, the crib is absent
/// and the search declines rather than fails.
///
/// A section that answers `false` here is not corrupt: 40 % of the label files
/// carries a per-file XOR on this IV (`.archive/research/labels/tttext2.md`). One key of such
/// a file gives the whole XOR and opens the rest of it ([`regime`]), but the key
/// itself costs the anchor sweep, which is why the two cases are reported apart.
fn search_has_a_crib(tag: &[u8], cipher: &[u8]) -> bool {
	if tag.len() < 3 || cipher.len() < 8 {
		return false;
	}
	let Ok(first) = <[u8; 8]>::try_from(&cipher[0..8]) else { return false };
	let t = crate::tea::tea_decrypt_block(first, &KEY_ROD);
	let iv = rod_block0_iv(tag);
	t[0] ^ iv[0] == 0x78 && t[1] ^ iv[1] == 0xda
}

/// What it would cost to open a section whose key is not cached yet.
///
/// The difference is not a matter of degree. A **classic** section's IV is
/// tag-derived and exact, so `plaintext[0..2]` reads `78 da` and deflate byte 0
/// comes free: one search over the multiplicatively-reduced candidate sets, and
/// a minute or two. A **shifted** file XORs a runtime mask over the finished IV
/// (`.archive/research/labels/tttext2.md` §3.3), which destroys both the anchor and the
/// reduction — so the only route is every legal anchor against the full space,
/// sixty searches wide, and that is hours to days rather than minutes.
///
/// Since the key is known to be the file name (2026-10-20, [`name_product`]), a
/// classic section under its own name needs no search at all; `OneSearch` is
/// what one renamed, or from another build than its name says, would cost.
///
/// Worth asking *before* starting, because the two look identical while running:
/// a spinner that will stop in ninety seconds and one that will not stop today
/// are the same spinner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyCost {
	/// Tag-derived IV is exact — one search, minutes.
	OneSearch,
	/// The file masks its IV — sixty full-space searches, hours at best.
	AnchorSweep,
}

/// What opening `want_tag` in this container would cost, or `None` if the
/// section is absent or uncompressed.
pub fn key_cost(data: &[u8], want_tag: &str) -> Option<KeyCost> {
	let mut pos = 0usize;
	while let Some((tag, payload_start)) = find_next_tag(data, pos) {
		let (payload_end, next_pos) = find_close(data, payload_start, &tag)?;
		if decode_latin1(&tag).trim() == want_tag {
			let payload = &data[payload_start..payload_end];
			let sc = parse_section_cipher(payload)?;
			if !sc.compressed || sc.cipher.len() < 8 {
				return None;
			}
			// The zlib magic is plaintext, so it survives an exact IV and not a
			// masked one — which is the whole test.
			return Some(match search_has_a_crib(&tag, sc.cipher) {
				true => KeyCost::OneSearch,
				false => KeyCost::AnchorSweep,
			});
		}
		pos = next_pos;
	}
	None
}

/// Whether `iv3to8` opens a classic section.
///
/// Asked of a key before it is trusted: the cache is keyed by file name and tag,
/// and every VCDS release re-encrypts its files under the same names, so a key
/// cached from one build is wrong for the next — and a wrong key does not fail
/// loudly, it just leaves the section shut.
fn key_opens(tag: &[u8], sc: &SectionCipher<'_>, iv3to8: [u8; 5]) -> bool {
	let section = decode_with_iv(&decode_latin1(tag), sc, rod_block0_iv_recovered(tag, iv3to8));
	matches!(section.status, RodStatus::Zlib | RodStatus::Tea)
}

/// One `[TAG]…[/TAG]` span of a container: its tag, and its payload — `None`
/// when the file ends before the section does.
struct Framed<'a> {
	tag: Vec<u8>,
	payload: Option<&'a [u8]>,
}

/// Every section of a container, in file order. An unterminated section is the
/// last one: where it ends cannot be told, so nothing after it is guessed at.
fn frame(data: &[u8]) -> Vec<Framed<'_>> {
	let mut out = Vec::new();
	let mut pos = 0usize;
	while let Some((tag, payload_start)) = find_next_tag(data, pos) {
		match find_close(data, payload_start, &tag) {
			Some((payload_end, next_pos)) => {
				out.push(Framed {
					tag,
					payload: Some(&data[payload_start..payload_end]),
				});
				pos = next_pos;
			}
			None => {
				out.push(Framed { tag, payload: None });
				break;
			}
		}
	}
	out
}

/// Which brute force a call may start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Search {
	/// None: the name's key and the cache only.
	Off,
	/// A classic section whose name does not give its key: one search, minutes.
	Classic,
	/// That, and the anchor sweep a shifted file's first section needs: hours.
	All,
}

/// Which IV regime a file is in (`.archive/research/labels/tttext2.md` §3).
enum Regime {
	/// Every section's IV is the name's ([`rod_named_iv`]).
	Classic,
	/// Every section but `[CMP]` has the name's IV XOR this mask.
	Shifted([u8; 8]),
	/// Shifted, and no section's key is known, so neither is the mask.
	ShiftedUnknown,
}

/// The section's cipher, when its framing reads and its tag is long enough to
/// derive an IV from.
fn cipher_of<'a>(framed: &Framed<'a>) -> Option<SectionCipher<'a>> {
	match framed.tag.len() >= 3 {
		true => parse_section_cipher(framed.payload?),
		false => None,
	}
}

/// The regime of a file, and its mask when that can be had.
///
/// The test is the zlib magic of any compressed section after `[CMP]` under the
/// name's IV: a classic file shows `78 da`, a shifted one does not. A shifted
/// file's mask is the whole IV of one opened section XOR the name's IV — from a
/// key cached for any of its sections, or, under [`Search::All`], from a search
/// for the smallest one. One key opens the whole file.
fn regime(sections: &[Framed<'_>], file: &str, cache: &mut IvCache, search: Search) -> Regime {
	let compressed: Vec<(&[u8], SectionCipher<'_>)> = sections
		.iter()
		.filter(|f| f.tag != b"CMP")
		.filter_map(|f| cipher_of(f).map(|sc| (f.tag.as_slice(), sc)))
		.filter(|(_, sc)| sc.compressed && sc.cipher.len() >= 8)
		.collect();
	if compressed.is_empty() || compressed.iter().any(|(tag, sc)| search_has_a_crib(tag, sc.cipher)) {
		return Regime::Classic;
	}
	let mask = |tag: &[u8], iv: [u8; 8]| -> [u8; 8] {
		let named = rod_named_iv(tag, file);
		std::array::from_fn(|i| iv[i] ^ named[i])
	};
	for (tag, sc) in &compressed {
		let tag_str = decode_latin1(tag);
		if let Some(iv3to8) = cache.get(file, &tag_str)
			&& let Some((_, iv)) = decode_shifted(&tag_str, sc, iv3to8)
		{
			return Regime::Shifted(mask(tag, iv));
		}
	}
	// A key cached for this file that no longer opens it is another build's, and
	// it is not searched past: the sweep is hours, and a cached key never started
	// one before the name was known either.
	let keyed = compressed.iter().any(|(tag, _)| cache.get(file, &decode_latin1(tag)).is_some());
	if search == Search::All && !keyed {
		let mut by_size: Vec<&(&[u8], SectionCipher<'_>)> = compressed.iter().collect();
		by_size.sort_by_key(|(_, sc)| sc.cipher.len());
		for (tag, sc) in by_size {
			let tag_str = decode_latin1(tag);
			if let Some(iv3to8) = crack::recover_iv3to8(tag, sc.cipher, sc.plainlen, None)
				&& let Some((_, iv)) = decode_shifted(&tag_str, sc, iv3to8)
			{
				cache.insert(file, &tag_str, iv3to8);
				return Regime::Shifted(mask(tag, iv));
			}
		}
	}
	Regime::ShiftedUnknown
}

/// Open one framed section of `file`, in the file's regime.
///
/// The name's IV first — with the file's mask over it when the file is shifted
/// and the mask is known — which opens every section of a classic file and
/// every section of a shifted one once a single key of it is known. Then, for a
/// compressed section that still did not open:
///
/// * a crib present (classic regime, but not this name's key: a file renamed, or
///   another build's under the same name) — the cached key, if it opens this
///   file, else one search under [`Search::Classic`] and up;
/// * no crib (shifted, mask unknown) — a key cached for this very section, else
///   [`RodStatus::SearchDeclined`].
fn open_framed(framed: &Framed<'_>, file: &str, regime: &Regime, cache: &mut IvCache, search: Search) -> RodSection {
	let tag = framed.tag.as_slice();
	let tag_str = decode_latin1(tag);
	let undecodable = |tag_str: String| RodSection {
		tag: tag_str,
		status: RodStatus::Undecodable,
		text: None,
	};
	let Some(sc) = cipher_of(framed) else {
		return undecodable(tag_str);
	};
	let named = rod_named_iv(tag, file);
	let iv = match regime {
		// `[CMP]` is exempt from the mask (tttext2.md §3.3a).
		Regime::Shifted(mask) if tag != b"CMP" => std::array::from_fn(|i| named[i] ^ mask[i]),
		_ => named,
	};
	let section = decode_with_iv(&tag_str, &sc, iv);
	if section.status != RodStatus::Undecodable || !sc.compressed {
		return section;
	}
	if search_has_a_crib(tag, sc.cipher) {
		// A cached key is used only if it opens this file — see [`key_opens`].
		if let Some(iv3to8) = cache.get(file, &tag_str).filter(|&k| key_opens(tag, &sc, k)) {
			return decode_with_iv(&tag_str, &sc, rod_block0_iv_recovered(tag, iv3to8));
		}
		if search != Search::Off
			&& let Some(iv3to8) = crack::recover_iv3to8(tag, sc.cipher, sc.plainlen, None)
		{
			cache.insert(file, &tag_str, iv3to8);
			return decode_with_iv(&tag_str, &sc, rod_block0_iv_recovered(tag, iv3to8));
		}
		return section;
	}
	// A classic IV[0..3] is tag-derived; a shifted one's is not, and has to be
	// read back off the known plaintext instead.
	let cached = cache.get(file, &tag_str);
	if let Some(iv3to8) = cached
		&& let Some((opened, _)) = decode_shifted(&tag_str, &sc, iv3to8)
	{
		return opened;
	}
	// The mask is known and still did not open this section: the file is under
	// a name not its own, whose IV differs from the true one per tag past byte 2.
	// Bytes 0..3 of an IV come from the tag alone, so the mask's are right and
	// the anchor follows from them — one search with the anchor known, not sixty.
	if let Regime::Shifted(mask) = regime
		&& search == Search::All
		&& cached.is_none()
		&& let Some(first) = sc.cipher.get(0..8).and_then(|b| <[u8; 8]>::try_from(b).ok())
	{
		let anchor = crate::tea::tea_decrypt_block(first, &KEY_ROD)[2] ^ named[2] ^ mask[2];
		if let Some(iv3to8) = crack::recover_iv3to8(tag, sc.cipher, sc.plainlen, Some(anchor))
			&& let Some((opened, _)) = decode_shifted(&tag_str, &sc, iv3to8)
		{
			cache.insert(file, &tag_str, iv3to8);
			return opened;
		}
	}
	// Say which of the two happened. A search that ran and lost and a search
	// that never started are not the same news, and only one of them means the
	// section is beyond this tooling.
	RodSection {
		status: RodStatus::SearchDeclined,
		..section
	}
}

/// Open the one section `want_tag` of a container, or `None` when it has none.
///
/// `file` is the file's name: the key of every section comes from it
/// ([`name_product`]). With `crack`, a classic section the name does not open is
/// searched for (minutes, recorded in `cache`); a shifted file is never searched
/// here, and opens only when a key of one of its sections is cached.
///
/// Decodes the one section and no other, which is what a caller after one table
/// of a multi-megabyte file wants.
pub fn open_section(data: &[u8], file: &str, want_tag: &str, cache: &mut IvCache, crack: bool) -> Option<RodSection> {
	let sections = frame(data);
	let wanted = sections.iter().find(|f| f.tag == want_tag.as_bytes())?;
	let search = match crack {
		true => Search::Classic,
		false => Search::Off,
	};
	let regime = regime(&sections, file, cache, Search::Off);
	Some(open_framed(wanted, file, &regime, cache, search))
}

/// Recover the key of **one** classic section into `cache`, and nothing else.
///
/// [`decode_rod_recover`] with `run_crack` searches every blocked section of a
/// file, and one file can hold a classic section beside a shifted one, whose
/// search is hours to days ([`KeyCost::AnchorSweep`]) — a registry research run
/// lost twenty minutes of every core that way. A caller that needs one section
/// asks for that one: a classic section gets its one search, minutes; a shifted
/// one is never searched here.
///
/// `true` when the section opens afterwards with no search: it opens under its
/// file's name, its key was already cached, or it has just been recovered.
/// `false` when the section is absent, shifted with no key known, or its search
/// found nothing.
pub fn recover_classic_key(data: &[u8], file: &str, want_tag: &str, cache: &mut IvCache) -> bool {
	open_section(data, file, want_tag, cache, true).is_some_and(|s| matches!(s.status, RodStatus::Tea | RodStatus::Zlib))
}

/// Decode every section of a `.rod` file named `file`.
///
/// Every key comes from the name ([`name_product`]); a shifted file's mask from
/// one key of it in `cache`. With `run_crack`, what the name does not open is
/// searched for: a classic section's key (minutes), and a shifted file's first
/// section (hours), whose key then opens the rest of the file. Recovered keys
/// are written to `cache` under `(file, tag)`. Never panics on malformed input.
pub fn decode_rod_recover(data: &[u8], file: &str, cache: &mut IvCache, run_crack: bool) -> Vec<RodSection> {
	let sections = frame(data);
	let search = match run_crack {
		true => Search::All,
		false => Search::Off,
	};
	let regime = regime(&sections, file, cache, search);
	sections.iter().map(|framed| open_framed(framed, file, &regime, cache, search)).collect()
}

/// Building `.rod` files for tests, with no Ross-Tech data in them.
///
/// The tool only ever reads these files; a test that needs one writes it. Each
/// section is framed as the real ones are, its first block under a chosen
/// product: the one its file's name gives ([`named`] — it opens with no key, as
/// a real file does), zero (`None` — a file with no name), or any other (it
/// opens only with that product's key: a file under a name not its own).
#[cfg(test)]
pub(crate) mod testkit {
	use super::{KEY_ROD, name_product, product5, rod_iv};
	use crate::tea::tea_cbc_encrypt;

	/// The full first-block IV for a chosen 5-byte product.
	pub(crate) fn iv_for_product(tag: &[u8], product5: [u8; 5]) -> [u8; 8] {
		rod_iv(tag, product5)
	}

	/// The product the file `file` keys its sections with, for [`section`].
	pub(crate) fn named(file: &str) -> Option<[u8; 5]> {
		Some(product5(name_product(file)))
	}

	/// One section, framed: `[TAG]\r\n`, the two BE24 lengths, the cipher,
	/// `\r\n[/TAG]\r\n`. `compressed` makes it a zlib section (header forced to
	/// `78 da`, the `.rod` convention); otherwise it is plain TEA.
	pub(crate) fn section(tag: &str, plain: &[u8], compressed: bool, product: Option<[u8; 5]>) -> Vec<u8> {
		masked_section(tag, plain, compressed, product, [0; 8])
	}

	/// [`section`] with `mask` XORed over the first-block IV, the way a shifted
	/// file's sections after `[CMP]` are (`.archive/research/labels/tttext2.md` §3.3a).
	pub(crate) fn masked_section(tag: &str, plain: &[u8], compressed: bool, product: Option<[u8; 5]>, mask: [u8; 8]) -> Vec<u8> {
		let unmasked = rod_iv(tag.as_bytes(), product.unwrap_or_default());
		let iv: [u8; 8] = std::array::from_fn(|i| unmasked[i] ^ mask[i]);
		let mut body = match compressed {
			true => {
				let mut z = miniz_oxide::deflate::compress_to_vec_zlib(plain, 9);
				z[0] = 0x78;
				z[1] = 0xda;
				z
			}
			false => plain.to_vec(),
		};
		while body.len() % 8 != 0 {
			body.push(0);
		}
		let cipher = tea_cbc_encrypt(&body, &KEY_ROD, iv);
		let read1 = cipher.len() as u32 | if compressed { 0 } else { 0x80_0000 };
		let mut out = format!("[{tag}]\r\n").into_bytes();
		out.extend_from_slice(&read1.to_be_bytes()[1..]);
		out.extend_from_slice(&(plain.len() as u32).to_be_bytes()[1..]);
		out.extend_from_slice(&cipher);
		out.extend_from_slice(format!("\r\n[/{tag}]\r\n").as_bytes());
		out
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::tea::tea_cbc_encrypt;
	use testkit::iv_for_product;

	/// Self-contained synthetic 94-byte `.rod` fixture: TEA-CBC-encrypted
	/// (with `KEY_ROD` + the embedded `MT`/`KS` tables, `product = 0`) one
	/// uncompressed `CMP` section and one zlib-compressed `MWB` section.
	/// Reproducible from the embedded tables — not proprietary data.
	const FIXTURE_HEX: &str = "5b434d505d0d0a80001000000ab75c46a2db4dfe23fd889ce32ef7e0280d0a5b2f434d505d0d0a5b4d57425d0d0a00002000001690c062e9b4ddca7d5e070d008f36c71d9b12ac9a75629c79da399231ecaaed810d0a5b2f4d57425d0d0a";

	fn hex_decode(s: &str) -> Vec<u8> {
		assert_eq!(s.len() % 2, 0);
		(0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
	}

	#[test]
	fn decodes_fixture_cmp_and_mwb_sections() {
		let data = hex_decode(FIXTURE_HEX);
		let sections = decode_rod(&data);
		assert_eq!(
			sections,
			vec![
				RodSection {
					tag: "CMP".to_string(),
					status: RodStatus::Tea,
					text: Some("776939,ABC".to_string()),
				},
				RodSection {
					tag: "MWB".to_string(),
					status: RodStatus::Zlib,
					text: Some("043439,X.\r\n043900,Y5\r\n".to_string()),
				},
			]
		);
	}

	#[test]
	fn a_section_the_search_cannot_start_on_says_so_instead_of_undecodable() {
		// The search is a known-plaintext attack whose crib is the zlib header.
		// Corrupt the compressed section's first block and the crib is gone —
		// which is the shape 40 % of the real label files have, where the IV carries
		// a per-file XOR nobody had noticed. Reporting that as `Undecodable`
		// reads as "this file is broken", and it is not.
		let mut data = hex_decode(FIXTURE_HEX);
		let at = data.windows(5).position(|w| w == b"[MWB]").unwrap() + 7 + 6;
		data[at] ^= 0xff;

		// The fixture is sealed under product 0, which is the key of a file with no
		// name.
		let mut cache = IvCache::default();
		let sections = decode_rod_recover(&data, "", &mut cache, false);
		let mwb = sections.iter().find(|s| s.tag == "MWB").unwrap();
		assert_eq!(mwb.status, RodStatus::SearchDeclined, "{sections:?}");

		// And the untouched file still reports the case where a search would
		// run: the two must not collapse back into one word.
		let clean = decode_rod_recover(&hex_decode(FIXTURE_HEX), "", &mut cache, false);
		assert_eq!(clean.iter().find(|s| s.tag == "MWB").unwrap().status, RodStatus::Zlib);
	}

	#[test]
	fn truncated_section_is_undecodable_and_does_not_panic() {
		// Well-framed but the payload is far too short to hold even the
		// 6-byte read1/read2 header.
		let data = b"[CMP]\r\nAB\r\n[/CMP]\r\n".to_vec();
		let sections = decode_rod(&data);
		assert_eq!(sections.len(), 1);
		assert_eq!(sections[0].tag, "CMP");
		assert_eq!(sections[0].status, RodStatus::Undecodable);
		assert_eq!(sections[0].text, None);
	}

	#[test]
	fn garbage_input_does_not_panic() {
		let data: Vec<u8> = (0..=255u8).collect();
		let sections = decode_rod(&data);
		// No well-formed section framing in this garbage; just must not panic.
		assert!(sections.is_empty());
	}

	#[test]
	fn unterminated_section_is_undecodable_and_does_not_panic() {
		// Opener present, but the file is truncated before the matching
		// [/TAG] close ever appears.
		let data = b"[CMP]\r\nsome bytes but no closing tag".to_vec();
		let sections = decode_rod(&data);
		assert_eq!(sections.len(), 1);
		assert_eq!(sections[0].tag, "CMP");
		assert_eq!(sections[0].status, RodStatus::Undecodable);
		assert_eq!(sections[0].text, None);
	}

	// --- Stage 1: 5-letter STRUC tag framing -------------------------------

	#[test]
	fn recognises_five_letter_struc_tag() {
		// The framing scanner used to cap tags at 4 letters; `STRUC` is 5.
		// Payload here is deliberately too short to decode, but the *tag* must
		// now be recognised (previously the whole `[STRUC]` opener was skipped).
		let data = b"[STRUC]\r\nAB\r\n[/STRUC]\r\n".to_vec();
		let sections = decode_rod(&data);
		assert_eq!(sections.len(), 1);
		assert_eq!(sections[0].tag, "STRUC");
	}

	#[test]
	fn iv_splice_keeps_tag_prefix_and_overwrites_tail() {
		let tag = b"MWB";
		let base = rod_block0_iv(tag);
		let spliced = rod_block0_iv_recovered(tag, [1, 2, 3, 4, 5]);
		assert_eq!(&spliced[0..3], &base[0..3]); // tag-derived prefix intact
		assert_eq!(&spliced[3..8], &[1, 2, 3, 4, 5]); // recovered tail
	}

	// --- the per-file IV shift (`.archive/research/labels/tttext2.md`) ----------------------

	/// Build a zlib section encrypted under an IV that has been XORed with a
	/// per-file mask, the way 40 % of the label files are.
	fn shifted_section(tag: &[u8], mask: [u8; 8]) -> (Vec<u8>, Vec<u8>, [u8; 5], u8) {
		// Skewed text, so miniz emits a dynamic-Huffman block (BTYPE 2).
		let plain: Vec<u8> = (0..4096u32)
			.map(|i| 0x20 + ((i.wrapping_mul(7).wrapping_add(i / 13)) % 60) as u8)
			.collect();
		let mut z = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 9);
		z[0] = 0x78;
		z[1] = 0xda;
		while z.len() % 8 != 0 {
			z.push(0);
		}
		let mut iv = rod_block0_iv(tag);
		for i in 0..8 {
			iv[i] ^= mask[i];
		}
		let cipher = tea_cbc_encrypt(&z, &KEY_ROD, iv);
		let mut payload = Vec::new();
		payload.extend_from_slice(&(cipher.len() as u32).to_be_bytes()[1..]); // read1
		payload.extend_from_slice(&(plain.len() as u32).to_be_bytes()[1..]); // read2
		payload.extend_from_slice(&cipher);
		(payload, plain, iv[3..8].try_into().unwrap(), z[2])
	}

	#[test]
	fn a_shifted_section_presents_no_crib_and_is_not_called_undecodable() {
		// The distinction the whole of `.archive/research/labels/tttext2.md` turns on: the
		// search cannot start here, which is not the same as the section being
		// beyond reach. A mask over the IV destroys the known plaintext.
		let tag = b"MWB";
		let (payload, _, _, _) = shifted_section(tag, [0x5e, 0xe6, 0x10, 1, 2, 3, 4, 5]);
		let sc = parse_section_cipher(&payload).unwrap();
		assert!(!search_has_a_crib(tag, sc.cipher));
		assert_eq!(decode_section(tag, &payload).status, RodStatus::Undecodable);
	}

	#[test]
	fn a_shifted_section_opens_from_the_same_five_bytes_a_classic_one_needs() {
		// The mask is eight bytes, but only three of them cost anything to
		// undo: two are read straight off the zlib magic and the third is
		// swept. So the key cache needs no new format.
		let tag = b"MWB";
		let mask = [0x5e, 0xe6, 0x10, 0x51, 0x77, 0x19, 0x40, 0x8c];
		let (payload, plain, iv3to8, _) = shifted_section(tag, mask);
		let sc = parse_section_cipher(&payload).unwrap();

		let (opened, iv) = decode_shifted("MWB", &sc, iv3to8).expect("the anchor sweep must find it");
		assert_eq!(opened.status, RodStatus::Zlib);
		assert_eq!(opened.text.unwrap().len(), plain.len());
		// The sweep also hands back the whole IV, and with it the file's whole
		// mask, which is what spares every later section of the same file a search.
		let model = rod_block0_iv(tag);
		assert_eq!(iv, std::array::from_fn(|i| model[i] ^ mask[i]));
	}

	#[test]
	fn the_shifted_path_refuses_a_wrong_key_rather_than_inventing_a_section() {
		// A wrong tail must not be rescued by one of the sixty anchors: the
		// sweep is only safe because the inflate has to land on the declared
		// length exactly.
		let tag = b"MWB";
		let mask = [0x5e, 0xe6, 0x10, 0x51, 0x77, 0x19, 0x40, 0x8c];
		let (payload, _, iv3to8, _) = shifted_section(tag, mask);
		let sc = parse_section_cipher(&payload).unwrap();

		let mut wrong = iv3to8;
		wrong[0] ^= 0xff;
		assert!(decode_shifted("MWB", &sc, wrong).is_none());
	}

	// What it spares is a search.
	#[test]
	fn one_key_of_a_shifted_file_opens_every_other_section_of_it() {
		// The mask belongs to the file, not the section, and it is all eight bytes
		// of the IV: one key of the file gives the whole of it, and every other
		// section opens with no search — the plain-TEA ones whole, first eight
		// bytes and all. `[CMP]` is exempt from the mask.
		let file = "EV_MadeUpShifted_042.rod";
		let mask = [0x5e, 0xe6, 0x10, 0x51, 0x77, 0x19, 0x40, 0x8c];
		let plain: Vec<u8> = (0..4096u32)
			.map(|i| 0x20 + ((i.wrapping_mul(7).wrapping_add(i / 13)) % 60) as u8)
			.collect();
		let data = [
			testkit::section("CMP", b"012345,AB\r\n", false, testkit::named(file)),
			testkit::masked_section("MWB", &plain, true, testkit::named(file), mask),
			testkit::masked_section("DTC", &plain[..2048], true, testkit::named(file), mask),
			testkit::masked_section("INC", b"000017,QRSTUV\r\n", false, testkit::named(file), mask),
		]
		.concat();

		// No key known: shifted, and said so rather than called undecodable.
		let mut cache = IvCache::default();
		let shut = decode_rod_recover(&data, file, &mut cache, false);
		assert_eq!(shut[0].text.as_deref(), Some("012345,AB\r\n"), "[CMP] is not masked");
		assert_eq!(shut[1].status, RodStatus::SearchDeclined);
		assert_eq!(shut[2].status, RodStatus::SearchDeclined);

		// One section's key, as the search would have found it, and the rest opens.
		let named = rod_named_iv(b"MWB", file);
		cache.insert(file, "MWB", std::array::from_fn(|k| named[k + 3] ^ mask[k + 3]));
		let opened = decode_rod_recover(&data, file, &mut cache, false);
		assert_eq!(opened[1].text.as_ref().map(String::len), Some(plain.len()));
		assert_eq!(opened[2].status, RodStatus::Zlib);
		assert_eq!(opened[2].text.as_ref().map(String::len), Some(2048));
		assert_eq!(opened[3].text.as_deref(), Some("000017,QRSTUV\r\n"), "plain TEA, whole");
		assert_eq!(cache.len(), 1, "nothing the name gives is cached");
		// And one section asked for alone opens the same way.
		let dtc = open_section(&data, file, "DTC", &mut cache, false).unwrap();
		assert_eq!(dtc.status, RodStatus::Zlib);
	}

	#[test]
	fn a_stale_key_of_a_shifted_file_does_not_start_the_sweep() {
		// Another build's key under the same name: it does not open, and the sweep
		// it would take is hours. A cached key never started one before the name
		// was known, and does not now — the section says it is shifted.
		let file = "EV_MadeUpShifted_043.rod";
		let mask = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
		let data = testkit::masked_section("MWB", &[b'7'; 3000], true, testkit::named(file), mask);
		let mut cache = IvCache::default();
		cache.insert(file, "MWB", [1, 2, 3, 4, 5]);
		let started = std::time::Instant::now();
		let opened = decode_rod_recover(&data, file, &mut cache, true);
		assert!(started.elapsed() < std::time::Duration::from_secs(2), "searched");
		assert_eq!(opened[0].status, RodStatus::SearchDeclined);
		assert_eq!(cache.get(file, "MWB"), Some([1, 2, 3, 4, 5]), "left as it was");
	}

	#[test]
	fn a_copy_gives_back_only_the_keys_it_changed() {
		// So one parallel lookup's untouched copy of a stale key cannot undo the
		// fresh one another lookup found.
		let mut base = IvCache::default();
		base.insert("A.rod", "MWB", [1; 5]);
		base.insert("B.rod", "MWB", [2; 5]);
		let mut fresh = base.clone();
		fresh.insert("A.rod", "MWB", [9; 5]);
		fresh.insert("C.rod", "DTC", [3; 5]);
		let untouched = base.clone();
		let (fresh, untouched) = (fresh.since(&base), untouched.since(&base));
		assert_eq!((fresh.len(), untouched.len()), (2, 0));
		base.absorb(fresh);
		base.absorb(untouched);
		assert_eq!(base.get("A.rod", "MWB"), Some([9; 5]));
		assert_eq!(base.get("B.rod", "MWB"), Some([2; 5]));
		assert_eq!(base.get("C.rod", "DTC"), Some([3; 5]));
	}

	// --- the key is the file name (2026-10-20) --------------------------------

	#[test]
	fn a_product_of_zero_is_the_tag_derived_iv() {
		// What every earlier decode used, and what a file with no name gets.
		for tag in [&b"CMP"[..], b"MWB", b"TXT", b"DTC", b"STRUC", b"FFMUX"] {
			assert_eq!(rod_iv(tag, [0; 5]), rod_block0_iv(tag), "{tag:?}");
			assert_eq!(rod_named_iv(tag, ""), rod_block0_iv(tag), "{tag:?}");
			assert_eq!(rod_named_iv(tag, ".rod"), rod_block0_iv(tag), "an empty stem");
		}
		assert_eq!(name_product(""), 0);
	}

	#[test]
	fn the_product_is_the_upper_cased_stem_with_its_first_character_twice() {
		let ab = 65u64 * 65 * 66;
		assert_eq!(name_product("AB"), ab);
		assert_eq!(name_product("ab.rod"), ab, "upper-cased, cut at the first dot");
		assert_eq!(name_product("Ab.x.rod"), ab);
		assert_eq!(name_product("/some/dir/aB.ROD"), ab, "the last path component");
		assert_eq!(name_product(r"C:\VCDS\UDS_EV\AB.rod"), ab);
		// Long names wrap, as a 64-bit multiply does.
		let stem = "EV_MADEUPNAME_0123456789";
		let first = stem.as_bytes()[0] as u64;
		let expected = stem.bytes().fold(first, |p, c| p.wrapping_mul(c as u64));
		assert_eq!(name_product("EV_MadeUpName_0123456789.rod"), expected);
		// Only the low five bytes reach the IV.
		let iv = rod_named_iv(b"MWB", "ab.rod");
		assert_eq!(iv, rod_iv(b"MWB", [ab as u8, (ab >> 8) as u8, (ab >> 16) as u8, 0, 0]));
	}

	#[test]
	fn a_classic_file_opens_by_its_name_alone_and_by_no_other() {
		let file = "EV_MadeUpUnit_017.rod";
		let plain: Vec<u8> = (0..2000u32).map(|i| b'0' + (i % 10) as u8).collect();
		let data = [
			testkit::section("CMP", b"017001,XY\r\n", false, testkit::named(file)),
			testkit::section("MWB", &plain, true, testkit::named(file)),
			testkit::section("INC", b"000017,QRSTUV\r\n", false, testkit::named(file)),
		]
		.concat();
		let mut cache = IvCache::default();
		let opened = decode_rod_recover(&data, file, &mut cache, false);
		assert_eq!(opened[0].text.as_deref(), Some("017001,XY\r\n"));
		assert_eq!(opened[1].status, RodStatus::Zlib);
		assert_eq!(opened[1].text.as_deref().map(str::as_bytes), Some(&plain[..]));
		assert_eq!(
			opened[2].text.as_deref(),
			Some("000017,QRSTUV\r\n"),
			"a plain-TEA section's first eight bytes are right too"
		);
		assert!(cache.is_empty(), "nothing to search for, nothing to cache");
		assert!(recover_classic_key(&data, file, "MWB", &mut cache));

		// Under another name the key is another, and the section stays shut —
		// a classic one, whose search would run.
		let renamed = decode_rod_recover(&data, "EV_Renamed.rod", &mut cache, false);
		assert_eq!(renamed[1].status, RodStatus::Undecodable);
		assert_ne!(renamed[2].text.as_deref(), Some("000017,QRSTUV\r\n"));
	}
	#[test]
	fn the_anchors_are_exactly_the_dynamic_huffman_headers() {
		let anchors: Vec<u8> = deflate_anchors().collect();
		assert_eq!(anchors.len(), 60);
		for a in &anchors {
			assert_eq!((a >> 1) & 0b11, 2, "BTYPE must be dynamic Huffman");
			assert!(a >> 3 <= 29, "HLIT + 257 must not exceed 286 literals");
		}
		// A classic section's real anchor has to be in the set, or the sweep
		// would miss the very streams it is modelled on. `STRUC.rod`'s is 0x8c.
		assert!(anchors.contains(&0x8c));
	}

	// --- Stage 1: recovered-IV cache round-trip ----------------------------

	#[test]
	fn a_cached_key_is_trusted_only_if_it_opens_the_section() {
		// Every VCDS release re-encrypts its files under the same names, and the
		// cache is keyed by name: a key from the last build must not be taken
		// for this one's.
		let product = [1, 2, 3, 4, 5];
		let framed = testkit::section("MWB", b"000001,00\r\n", true, Some(product));
		let (tag, payload_start) = find_next_tag(&framed, 0).unwrap();
		let (payload_end, _) = find_close(&framed, payload_start, &tag).unwrap();
		let sc = parse_section_cipher(&framed[payload_start..payload_end]).unwrap();
		let right: [u8; 5] = iv_for_product(&tag, product)[3..8].try_into().unwrap();
		let stale = [right[0] ^ 1, right[1], right[2], right[3], right[4]];
		assert!(key_opens(&tag, &sc, right));
		assert!(!key_opens(&tag, &sc, stale));

		let mut cache = IvCache::default();
		cache.insert("EV_X.rod", "MWB", right);
		assert!(
			recover_classic_key(&framed, "EV_X.rod", "MWB", &mut cache),
			"the right key opens it, no search"
		);
		// And with the key right, the whole-file decode opens it too.
		let opened = decode_rod_recover(&framed, "EV_X.rod", &mut cache, false);
		assert_eq!(opened[0].status, RodStatus::Zlib);
	}

	#[test]
	fn iv_cache_round_trips_through_json() {
		let mut cache = IvCache::default();
		cache.insert("STRUC.rod", "STRUC", [0x9d, 0x69, 0x92, 0x24, 0x29]);
		let json = serde_json::to_vec(&cache).unwrap();
		let back: IvCache = serde_json::from_slice(&json).unwrap();
		assert_eq!(back.get("STRUC.rod", "STRUC"), Some([0x9d, 0x69, 0x92, 0x24, 0x29]));
		assert_eq!(back.get("STRUC.rod", "MWB"), None);
	}

	#[test]
	fn a_key_file_that_does_not_read_is_never_written_over() {
		// It loads as an empty cache, and saving that back would erase every key
		// it held — each one minutes of every core to find again.
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("rod-keys.json");
		std::fs::write(&path, b"{ torn").unwrap();
		let mut cache = IvCache::load(&path);
		assert_eq!(cache.get("X.rod", "MWB"), None, "it loads as empty");
		cache.insert("X.rod", "MWB", [1, 2, 3, 4, 5]);
		assert!(cache.save(&path).is_err());
		assert_eq!(std::fs::read(&path).unwrap(), b"{ torn", "left as it was");

		// A file that reads, or none at all, is replaced whole, with nothing left beside it.
		std::fs::remove_file(&path).unwrap();
		cache.save(&path).unwrap();
		cache.insert("Y.rod", "INC", [6, 7, 8, 9, 10]);
		cache.save(&path).unwrap();
		let back = IvCache::load(&path);
		assert_eq!(
			(back.get("X.rod", "MWB"), back.get("Y.rod", "INC")),
			(Some([1, 2, 3, 4, 5]), Some([6, 7, 8, 9, 10]))
		);
		assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
	}

	#[cfg(unix)]
	#[test]
	fn a_key_file_that_cannot_be_read_is_never_written_over() {
		// It loads as an empty cache just as a torn one does, and renaming a new
		// file over it would lose its keys the same way.
		use std::os::unix::fs::PermissionsExt;
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("rod-keys.json");
		let mut held = IvCache::default();
		held.insert("X.rod", "MWB", [1, 2, 3, 4, 5]);
		held.save(&path).unwrap();
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
		if std::fs::read(&path).is_ok() {
			// Running as a user no permission stops: nothing to test here.
			return;
		}
		let refused = IvCache::load(&path).save(&path);
		std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
		assert!(refused.is_err());
		assert_eq!(
			IvCache::load(&path).get("X.rod", "MWB"),
			Some([1, 2, 3, 4, 5]),
			"the keys are still there"
		);
	}

	// --- Stage 1: end-to-end offline crack on a synthetic blocked section ---

	/// Build a synthetic `product != 0` zlib section, then prove the offline
	/// cracker recovers the exact `iv[3..8]` and the section decodes.
	// Both of these exercise the brute force itself.
	#[test]
	fn recovers_product_blocked_zlib_section() {
		// ~4 KB of skewed-alphabet text -> miniz emits a dynamic-Huffman block,
		// which the header oracle needs (BTYPE == 2).
		let plain: Vec<u8> = (0..4096u32)
			.map(|i| 0x20 + ((i.wrapping_mul(7).wrapping_add(i / 13)) % 60) as u8)
			.collect();
		let mut z = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 9);
		// Force the zlib header to `78 da` (the `.rod` convention the recovery
		// asserts). Both 78 9c and 78 da are FCHECK-valid; miniz ignores it.
		z[0] = 0x78;
		z[1] = 0xda;
		// Pad the cipher input to an 8-byte boundary (trailing bytes after the
		// zlib stream are ignored by inflate).
		while z.len() % 8 != 0 {
			z.push(0);
		}

		let tag = b"MWB";
		let secret_product = [0x11u8, 0x22, 0x33, 0x44, 0x55];
		let iv = iv_for_product(tag, secret_product);
		// Sanity: iv[0..3] must equal the product-0 tag prefix (product-independent).
		assert_eq!(&iv[0..3], &rod_block0_iv(tag)[0..3]);
		assert_ne!(&iv[3..8], &rod_block0_iv(tag)[3..8]); // genuinely blocked

		let cipher = tea_cbc_encrypt(&z, &KEY_ROD, iv);

		// The product-0 decode must FAIL (this is the blocker).
		let mut payload = Vec::new();
		payload.extend_from_slice(&(cipher.len() as u32).to_be_bytes()[1..]); // read1 (flag clear)
		payload.extend_from_slice(&(plain.len() as u32).to_be_bytes()[1..]); // read2
		payload.extend_from_slice(&cipher);
		let blocked = decode_section(tag, &payload);
		assert_eq!(blocked.status, RodStatus::Undecodable);

		// The recovery ORACLE confirms the true iv[3..8] and rejects a wrong
		// one — this exercises the whole plumbing (TEA block, CBC tail,
		// header oracle, inflate) without the multi-minute 2^36 search, which
		// the `vagcan dev vcds rod` acceptance run performs on the real STRUC.rod.
		let true_tail = [iv[3], iv[4], iv[5], iv[6], iv[7]];
		assert!(crack::confirm_iv3to8(tag, &cipher, plain.len(), true_tail));
		let wrong_tail = [iv[3] ^ 0xff, iv[4], iv[5], iv[6], iv[7]];
		assert!(!crack::confirm_iv3to8(tag, &cipher, plain.len(), wrong_tail));

		// And the splice + decode path yields the exact plaintext.
		let sc = parse_section_cipher(&payload).unwrap();
		let fixed = decode_with_iv("MWB", &sc, rod_block0_iv_recovered(tag, true_tail));
		assert_eq!(fixed.status, RodStatus::Zlib);
		assert_eq!(fixed.text.unwrap().len(), plain.len());
	}

	/// The full multithreaded brute force, on the same synthetic section.
	/// `#[ignore]`d because it sweeps a ~2^36 space (minutes). Run with
	/// `cargo test -p vag-data-labels --lib -- --ignored recovers_via_full_search`.
	#[test]
	#[ignore = "multi-minute brute force; run explicitly"]
	fn recovers_via_full_search() {
		let plain: Vec<u8> = (0..4096u32)
			.map(|i| 0x20 + ((i.wrapping_mul(7).wrapping_add(i / 13)) % 60) as u8)
			.collect();
		let mut z = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 9);
		z[0] = 0x78;
		z[1] = 0xda;
		while z.len() % 8 != 0 {
			z.push(0);
		}
		let tag = b"MWB";
		let iv = iv_for_product(tag, [0x11, 0x22, 0x33, 0x44, 0x55]);
		let cipher = tea_cbc_encrypt(&z, &KEY_ROD, iv);
		let recovered = crack::recover_iv3to8(tag, &cipher, plain.len(), None).expect("full search should find it");
		assert_eq!(recovered, [iv[3], iv[4], iv[5], iv[6], iv[7]]);
	}
}
