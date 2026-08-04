//! Decoder for Ross-Tech VCDS `.rod` files (`UDS_EV/` UDS/ODX data).
//!
//! `.rod` uses the same TEA cipher family as `.clb` (see [`crate::tea`]), but
//! wraps records in an ASCII section-tag container (`[TAG]\r\n<payload>\r\n
//! [/TAG]\r\n`) and layers zlib compression on top of some sections'
//! plaintext. This module only DECODES sections to raw text; it does not
//! interpret the content.
//!
//! ## Scope
//! - `MWB` section rows are `<6-digit measurement id>,<code>` — a UDS/ODX
//!   measurement INDEX, not a human-readable name. Human names live in
//!   `TTTEXT.ROD` (same cipher family) and require joining on those IDs; that
//!   TTText layer, plus the per-record `product` term needed for records
//!   where it isn't zero (needs a runtime dump to recover), are a documented
//!   FUTURE step and are NOT built here.
//! - `.rod` is NOT ingested into [`crate::LabelDb`] / the SQLite corpus: its
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
    /// In **40 % of the corpus** they do not — those files carry a per-file XOR
    /// on the first-block IV of every section after `[CMP]`
    /// (`research/tttext2.md`).
    ///
    /// Reported apart from [`RodStatus::Undecodable`] because the two say
    /// opposite things. A file that is undecodable has been tried; one that is
    /// declined has not, and reading the second as the first is how
    /// `TTTEXT2.ROD` sat "uncrackable" through four writeups that named it as
    /// the next thing to open.
    SearchDeclined,
    /// Could not be decoded: bad framing, a first block needing the
    /// (unavailable) nonzero per-record `product` term, a short/misaligned
    /// cipher, or an inflate failure.
    Undecodable,
}

/// One `[TAG]...[/TAG]` section of a `.rod` file, decoded (or not).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RodSection {
    pub tag: String,
    pub status: RodStatus,
    pub text: Option<String>,
}

/// IV for a `.rod` record's first TEA-CBC block, in the `product = 0` case
/// (correct whenever the per-record string is empty/NUL — the common case).
/// `tag` must be at least 3 bytes (all real section tags are 2-4 uppercase
/// ASCII letters, so this always holds for tags accepted by the framing
/// scanner below).
fn rod_block0_iv(tag: &[u8]) -> [u8; 8] {
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
/// corpus: `BTYPE = 2` (dynamic Huffman), `HLIT ≤ 29`, either `BFINAL`.
///
/// Sixty values, and they matter because a shifted file (`research/tttext2.md`
/// §3.3) destroys exactly this byte. The layout is RFC 1951 §3.2.7:
/// `BFINAL | BTYPE << 1 | HLIT << 3`. Stored blocks and fixed-Huffman blocks
/// are excluded deliberately — no section in the corpus uses either, and
/// admitting them would double a search that is already the expensive part.
fn deflate_anchors() -> impl Iterator<Item = u8> {
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
/// `research/tttext2.md` §3.3, given the recovered `iv[3..8]`.
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
/// Also returns the file's `D[2]` — the third byte of the mask, which the
/// winning anchor pins. It is worth carrying because the mask is a property of
/// the **file**: knowing it turns the sixty-anchor sweep into a single known
/// anchor for every other section of the same file, and on the *search* side
/// that is a 60× saving on hours, not on milliseconds.
fn decode_shifted(
    tag_str: &str,
    tag: &[u8],
    sc: &SectionCipher<'_>,
    iv3to8: [u8; 5],
) -> Option<(RodSection, u8)> {
    let first: [u8; 8] = sc.cipher.get(0..8)?.try_into().ok()?;
    let t = crate::tea::tea_decrypt_block(first, &KEY_ROD);
    let mut iv = [0u8; 8];
    iv[0] = t[0] ^ 0x78;
    iv[1] = t[1] ^ 0xda;
    iv[3..8].copy_from_slice(&iv3to8);
    let model2 = rod_block0_iv(tag)[2];
    deflate_anchors().find_map(|d0| {
        iv[2] = t[2] ^ d0;
        inflate_with_iv(sc, iv).map(|bytes| {
            let section = RodSection {
                tag: tag_str.to_string(),
                status: RodStatus::Zlib,
                text: Some(decode_latin1(&bytes)),
            };
            (section, iv[2] ^ model2)
        })
    })
}

/// The deflate anchor for a shifted section, once the file's `D[2]` is known.
///
/// `plaintext[2] = t[2] ^ IV[2]` and `IV[2] = IV_model[2] ^ D[2]`, so the byte
/// the searcher would otherwise have to guess sixty times is simple arithmetic.
///
/// Only the search has a use for it, and the search is behind `rod-crack`.
#[cfg(feature = "rod-crack")]
fn anchor_from_shift(tag: &[u8], cipher: &[u8], shift2: u8) -> Option<u8> {
    let first: [u8; 8] = cipher.get(0..8)?.try_into().ok()?;
    let t = crate::tea::tea_decrypt_block(first, &KEY_ROD);
    Some(t[2] ^ rod_block0_iv(tag)[2] ^ shift2)
}

/// Decode raw Latin-1 bytes into a `String`, one byte per `char`. Matches
/// `label::parse_label`'s internal decoding, and `clb`'s test helper of the
/// same name.
fn decode_latin1(bytes: &[u8]) -> String {
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
fn find_next_tag(data: &[u8], from: usize) -> Option<(Vec<u8>, usize)> {
    let mut i = from;
    while i < data.len() {
        if data[i] == b'[' {
            let tag_start = i + 1;
            let mut j = tag_start;
            while j < data.len() && data[j].is_ascii_uppercase() && j - tag_start < 8 {
                j += 1;
            }
            let tag_len = j - tag_start;
            if (2..=8).contains(&tag_len)
                && j + 2 < data.len()
                && data[j] == b']'
                && data[j + 1] == b'\r'
                && data[j + 2] == b'\n'
            {
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
fn find_close(data: &[u8], from: usize, tag: &[u8]) -> Option<(usize, usize)> {
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
struct SectionCipher<'a> {
    /// zlib-compressed (`read1` flag clear) vs plain TEA.
    compressed: bool,
    /// Declared decompressed / plaintext length.
    plainlen: usize,
    /// The `storedlen`-byte ciphertext (already length/alignment-validated).
    cipher: &'a [u8],
}

/// Parse the standard 6-byte `.rod` section header (two BE24 ints) and return
/// the framed ciphertext slice, or `None` if the framing is malformed.
fn parse_section_cipher(payload: &[u8]) -> Option<SectionCipher<'_>> {
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
/// and `[/TAG]` markers) into a [`RodSection`], using the `product = 0` IV.
/// Sections whose per-record `product` term is nonzero come back
/// `Undecodable`; use `decode_rod_recover` (feature `rod-crack`) to recover
/// those offline.
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

/// Decode a `.rod` file into its sections. Sections whose first block needs
/// the (unavailable) nonzero per-record `product` term, or that are
/// otherwise malformed (bad framing, short/misaligned cipher, inflate
/// failure), come back `Undecodable` with `text: None`. Never panics on
/// malformed input.
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
// Recovered-IV cache + `product != 0` recovery (feature `rod-crack`)
// ---------------------------------------------------------------------------

/// A persistent cache of recovered first-block `iv[3..8]` values, keyed by
/// `(file, tag)`. Recovering a `product != 0` section is CPU-heavy (~1 min),
/// so the result is cached to disk to make repeat runs instant. Serialized as
/// a JSON object of `"<file>\t<tag>" -> [5 bytes]`.
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
    /// unreadable.
    pub fn load(path: &std::path::Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Persist the cache to `path`.
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(path, serde_json::to_vec_pretty(self).unwrap_or_default())
    }

    pub fn get(&self, file: &str, tag: &str) -> Option<[u8; 5]> {
        self.entries.get(&Self::key(file, tag)).copied()
    }

    pub fn insert(&mut self, file: &str, tag: &str, iv3to8: [u8; 5]) {
        self.entries.insert(Self::key(file, tag), iv3to8);
    }
}

/// The brute-force IV recovery, compiled only into the tool that needs it.
///
/// Recovery costs about a minute of every core per section, and nothing in the
/// live path can use it: reading a car needs the *answer*, which is a five-byte
/// value in `catalogs/rod-iv-cache.json`, not the search that produced it. So
/// the search ships in `vagcan vcds rod` (`--features rod-crack`) and the CLI
/// links only the cache lookup.
#[cfg(feature = "rod-crack")]
mod crack;

/// Recover the first-block `iv[3..8]` for a single `product != 0` zlib section,
/// given its raw framed payload (the bytes between `[TAG]` and `[/TAG]`).
/// Returns `None` if the section is not a compressed section, is malformed, or
/// the search fails. CPU-heavy (multithreaded brute force, ~1 min per section).
#[cfg(feature = "rod-crack")]
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
/// A section that answers `false` here is not corrupt: 40 % of the corpus
/// carries a per-file XOR on this IV, and one of those files was opened by
/// deriving it (`research/tttext2.md`). Nothing in this crate recovers that
/// XOR yet, which is exactly why the two cases are reported apart.
fn search_has_a_crib(tag: &[u8], cipher: &[u8]) -> bool {
    if tag.len() < 3 || cipher.len() < 8 {
        return false;
    }
    let Ok(first) = <[u8; 8]>::try_from(&cipher[0..8]) else { return false };
    let t = crate::tea::tea_decrypt_block(first, &KEY_ROD);
    let iv = rod_block0_iv(tag);
    t[0] ^ iv[0] == 0x78 && t[1] ^ iv[1] == 0xda
}

/// Like [`decode_rod`], but for every section that fails the `product = 0`
/// decode AND is a zlib section, recover the missing `iv[3..8]` offline
/// (brute force) and retry. Recovered values are read from / written to
/// `cache` under the key `(file, tag)`; pass the file's display name as
/// `file`. Set `run_crack = false` to only use already-cached values; without
/// the `rod-crack` feature that is the only behaviour available, and the flag
/// is accepted so callers do not need two spellings.
pub fn decode_rod_recover(
    data: &[u8],
    file: &str,
    cache: &mut IvCache,
    run_crack: bool,
) -> Vec<RodSection> {
    let mut sections = Vec::new();
    let mut pos = 0usize;
    // The IV shift is a property of the file, so the first shifted section to
    // open hands its third mask byte to every section after it — which is what
    // spares the rest of the file its own sweep. Only the search consumes it,
    // so without `rod-crack` it does not exist at all rather than sitting there
    // written and unread.
    #[cfg(feature = "rod-crack")]
    let mut shift2: Option<u8> = None;
    while let Some((tag, payload_start)) = find_next_tag(data, pos) {
        match find_close(data, payload_start, &tag) {
            Some((payload_end, next_pos)) => {
                let payload = &data[payload_start..payload_end];
                let tag_str = decode_latin1(&tag);
                let mut section = decode_section(&tag, payload);
                // Only compressed sections can be blocked by a nonzero product.
                if section.status == RodStatus::Undecodable && tag.len() >= 3 {
                    if let Some(sc) = parse_section_cipher(payload) {
                        if sc.compressed {
                            let recovered = cache.get(file, &tag_str).or_else(|| {
                                #[cfg(feature = "rod-crack")]
                                if run_crack {
                                    let anchor = shift2
                                        .and_then(|d2| anchor_from_shift(&tag, sc.cipher, d2));
                                    let iv = crack::recover_iv3to8(
                                        &tag, sc.cipher, sc.plainlen, anchor,
                                    );
                                    if let Some(v) = iv {
                                        cache.insert(file, &tag_str, v);
                                    }
                                    return iv;
                                }
                                let _ = run_crack;
                                None
                            });
                            match recovered {
                                // A classic section's IV[0..3] is tag-derived;
                                // a shifted one's is not, and has to be read
                                // back off the known plaintext instead.
                                Some(iv3to8) if search_has_a_crib(&tag, sc.cipher) => {
                                    let iv = rod_block0_iv_recovered(&tag, iv3to8);
                                    section = decode_with_iv(&tag_str, &sc, iv);
                                }
                                Some(iv3to8) => {
                                    if let Some((s, d2)) =
                                        decode_shifted(&tag_str, &tag, &sc, iv3to8)
                                    {
                                        section = s;
                                        #[cfg(feature = "rod-crack")]
                                        {
                                            shift2 = Some(d2);
                                        }
                                        let _ = d2;
                                    }
                                }
                                // Say which of the two happened. A search that
                                // ran and lost and a search that never started
                                // are not the same news, and only one of them
                                // means the section is beyond this tooling.
                                None if !search_has_a_crib(&tag, sc.cipher) => {
                                    section.status = RodStatus::SearchDeclined;
                                }
                                None => {}
                            }
                        }
                    }
                }
                sections.push(section);
                pos = next_pos;
            }
            None => {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Self-contained synthetic 94-byte `.rod` fixture: TEA-CBC-encrypted
    /// (with `KEY_ROD` + the embedded `MT`/`KS` tables, `product = 0`) one
    /// uncompressed `CMP` section and one zlib-compressed `MWB` section.
    /// Reproducible from the embedded tables — not proprietary data.
    const FIXTURE_HEX: &str = "5b434d505d0d0a80001000000ab75c46a2db4dfe23fd889ce32ef7e0280d0a5b2f434d505d0d0a5b4d57425d0d0a00002000001690c062e9b4ddca7d5e070d008f36c71d9b12ac9a75629c79da399231ecaaed810d0a5b2f4d57425d0d0a";

    fn hex_decode(s: &str) -> Vec<u8> {
        assert_eq!(s.len() % 2, 0);
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
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
        // which is the shape 40 % of the real corpus has, where the IV carries
        // a per-file XOR nobody had noticed. Reporting that as `Undecodable`
        // reads as "this file is broken", and it is not.
        let mut data = hex_decode(FIXTURE_HEX);
        let at = data.windows(5).position(|w| w == b"[MWB]").unwrap() + 7 + 6;
        data[at] ^= 0xff;

        let mut cache = IvCache::default();
        let sections = decode_rod_recover(&data, "fixture", &mut cache, false);
        let mwb = sections.iter().find(|s| s.tag == "MWB").unwrap();
        assert_eq!(mwb.status, RodStatus::SearchDeclined, "{sections:?}");

        // And the untouched file still reports the case where a search would
        // run: the two must not collapse back into one word.
        let clean = decode_rod_recover(&hex_decode(FIXTURE_HEX), "fixture", &mut cache, false);
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

    // --- the per-file IV shift (`research/tttext2.md`) ----------------------

    /// Build a zlib section encrypted under an IV that has been XORed with a
    /// per-file mask, the way 40 % of the corpus is.
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
        // The distinction the whole of `research/tttext2.md` turns on: the
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

        let (opened, d2) =
            decode_shifted("MWB", tag, &sc, iv3to8).expect("the anchor sweep must find it");
        assert_eq!(opened.status, RodStatus::Zlib);
        assert_eq!(opened.text.unwrap().len(), plain.len());
        // The sweep also hands back the file's third mask byte, which is what
        // spares every later section of the same file its own sixty searches.
        assert_eq!(d2, mask[2]);
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
        assert!(decode_shifted("MWB", tag, &sc, wrong).is_none());
    }

    // What it spares is a search, and the search is behind `rod-crack`.
    #[cfg(feature = "rod-crack")]
    #[test]
    fn one_opened_section_spares_the_rest_of_the_file_their_own_sweep() {
        // The mask belongs to the file, not the section. Two sections under
        // one mask: the third mask byte learned from the first must give the
        // second its deflate anchor outright, which is what turns 60 full-space
        // searches into 1 for every section after the first.
        let mask = [0x5e, 0xe6, 0x10, 0x51, 0x77, 0x19, 0x40, 0x8c];
        let (pa, _, iv_a, _) = shifted_section(b"MWB", mask);
        let (pb, _, _, anchor_b) = shifted_section(b"DTC", mask);

        let sc_a = parse_section_cipher(&pa).unwrap();
        let (_, d2) = decode_shifted("MWB", b"MWB", &sc_a, iv_a).expect("first section opens");

        let sc_b = parse_section_cipher(&pb).unwrap();
        assert_eq!(anchor_from_shift(b"DTC", sc_b.cipher, d2), Some(anchor_b));
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
    fn iv_cache_round_trips_through_json() {
        let mut cache = IvCache::default();
        cache.insert("STRUC.rod", "STRUC", [0x9d, 0x69, 0x92, 0x24, 0x29]);
        let json = serde_json::to_vec(&cache).unwrap();
        let back: IvCache = serde_json::from_slice(&json).unwrap();
        assert_eq!(
            back.get("STRUC.rod", "STRUC"),
            Some([0x9d, 0x69, 0x92, 0x24, 0x29])
        );
        assert_eq!(back.get("STRUC.rod", "MWB"), None);
    }

    // --- Stage 1: end-to-end offline crack on a synthetic blocked section ---

    /// TEA encrypt one 8-byte block (inverse of [`crate::tea::tea_decrypt_block`]).
    fn tea_encrypt_block(block: [u8; 8], key: &[u32; 4]) -> [u8; 8] {
        let mut v0 = u32::from_le_bytes(block[0..4].try_into().unwrap());
        let mut v1 = u32::from_le_bytes(block[4..8].try_into().unwrap());
        let mut s = 0u32;
        for _ in 0..32 {
            s = s.wrapping_add(crate::tea::DELTA);
            v0 = v0.wrapping_add(
                (v1 << 4).wrapping_add(key[0]) ^ v1.wrapping_add(s) ^ (v1 >> 5).wrapping_add(key[1]),
            );
            v1 = v1.wrapping_add(
                (v0 << 4).wrapping_add(key[2]) ^ v0.wrapping_add(s) ^ (v0 >> 5).wrapping_add(key[3]),
            );
        }
        let mut out = [0u8; 8];
        out[0..4].copy_from_slice(&v0.to_le_bytes());
        out[4..8].copy_from_slice(&v1.to_le_bytes());
        out
    }

    fn tea_cbc_encrypt(plain: &[u8], key: &[u32; 4], iv: [u8; 8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plain.len());
        let mut prev = iv;
        for block in plain.chunks_exact(8) {
            let mut x = [0u8; 8];
            for i in 0..8 {
                x[i] = block[i] ^ prev[i];
            }
            let c = tea_encrypt_block(x, key);
            out.extend_from_slice(&c);
            prev = c;
        }
        out
    }

    /// Build the full first-block IV for a chosen (nonzero) 5-byte product,
    /// mirroring `rod_block0_iv` but with a nonzero seed tail — so the produced
    /// `iv[3..8]` is guaranteed to lie in the reachable candidate space.
    #[cfg(feature = "rod-crack")]
    fn iv_for_product(tag: &[u8], product5: [u8; 5]) -> [u8; 8] {
        let m = tag[1] as usize;
        let mut seed = [0u8; 8];
        seed[..3].copy_from_slice(&tag[..3]);
        seed[3..8].copy_from_slice(&product5);
        let mut iv = [0u8; 8];
        for i in 0..8 {
            let s = seed[i].wrapping_add(KS[(m * (i + 2)) & 0xff]);
            iv[i] = s.wrapping_mul(MT[OFF_ROD[i]]);
        }
        iv
    }

    /// Build a synthetic `product != 0` zlib section, then prove the offline
    /// cracker recovers the exact `iv[3..8]` and the section decodes.
    // Both of these exercise the brute force itself, so they build only with
    // the feature that compiles it.
    #[cfg(feature = "rod-crack")]
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
        // the `vagcan vcds rod` acceptance run performs on the real STRUC.rod.
        let true_tail = [iv[3], iv[4], iv[5], iv[6], iv[7]];
        assert!(crack::confirm_iv3to8(tag, &cipher, plain.len(), true_tail));
        let wrong_tail = [
            iv[3] ^ 0xff,
            iv[4],
            iv[5],
            iv[6],
            iv[7],
        ];
        assert!(!crack::confirm_iv3to8(tag, &cipher, plain.len(), wrong_tail));

        // And the splice + decode path yields the exact plaintext.
        let sc = parse_section_cipher(&payload).unwrap();
        let fixed = decode_with_iv("MWB", &sc, rod_block0_iv_recovered(tag, true_tail));
        assert_eq!(fixed.status, RodStatus::Zlib);
        assert_eq!(fixed.text.unwrap().len(), plain.len());
    }

    /// The full multithreaded brute force, on the same synthetic section.
    /// `#[ignore]`d because it sweeps a ~2^36 space (minutes). Run with
    /// `cargo test -p vag-data --lib -- --ignored recovers_via_full_search`.
    #[cfg(feature = "rod-crack")]
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
        let recovered =
            crack::recover_iv3to8(tag, &cipher, plain.len(), None)
                .expect("full search should find it");
        assert_eq!(recovered, [iv[3], iv[4], iv[5], iv[6], iv[7]]);
    }
}
