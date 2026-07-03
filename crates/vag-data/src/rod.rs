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

const KEY_ROD: [u32; 4] = [0x029b_76a4, 0xcb6d_b50a, 0x7139_5d29, 0x0dbc_09c2];
const OFF_ROD: [usize; 8] = [0x07, 0xca, 0x22, 0x99, 0x3e, 0x88, 0xc3, 0x76];

static MT: &[u8; 256] = include_bytes!("rod_mt.bin");
static KS: &[u8; 256] = include_bytes!("rod_ks.bin");

/// How a [`RodSection`]'s payload was decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RodStatus {
    /// Decrypted (TEA-CBC) but not compressed.
    Tea,
    /// Decrypted then zlib-inflated.
    Zlib,
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
/// 2-4 uppercase ASCII letters + `]\r\n`. Returns the tag bytes and the
/// index of the first payload byte (right after the opener).
fn find_next_tag(data: &[u8], from: usize) -> Option<(Vec<u8>, usize)> {
    let mut i = from;
    while i < data.len() {
        if data[i] == b'[' {
            let tag_start = i + 1;
            let mut j = tag_start;
            while j < data.len() && data[j].is_ascii_uppercase() && j - tag_start < 4 {
                j += 1;
            }
            let tag_len = j - tag_start;
            if (2..=4).contains(&tag_len)
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

/// Decode one section's raw payload bytes (as captured between the `[TAG]`
/// and `[/TAG]` markers) into a [`RodSection`].
fn decode_section(tag: &[u8], payload: &[u8]) -> RodSection {
    let tag_str = decode_latin1(tag);
    let undecodable = || RodSection {
        tag: tag_str.clone(),
        status: RodStatus::Undecodable,
        text: None,
    };

    if tag.len() < 3 || payload.len() < 6 {
        return undecodable();
    }

    let read1 = be24(&payload[0..3]);
    let storedlen = (read1 & 0x7f_ffff) as usize;
    let compressed = (read1 & 0x80_0000) == 0;
    let read2 = be24(&payload[3..6]);
    let plainlen = read2 as usize;

    if storedlen % 8 != 0 || payload.len() < 6 + storedlen {
        return undecodable();
    }
    let cipher = &payload[6..6 + storedlen];
    let iv = rod_block0_iv(tag);
    let dec = tea_cbc_decrypt(cipher, &KEY_ROD, iv);

    if !compressed {
        if plainlen > dec.len() {
            return undecodable();
        }
        RodSection {
            tag: tag_str,
            status: RodStatus::Tea,
            text: Some(decode_latin1(&dec[..plainlen])),
        }
    } else {
        match miniz_oxide::inflate::decompress_to_vec_zlib(&dec) {
            Ok(bytes) => RodSection {
                tag: tag_str,
                status: RodStatus::Zlib,
                text: Some(decode_latin1(&bytes)),
            },
            Err(_) => undecodable(),
        }
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
}
