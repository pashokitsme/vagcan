//! `STRUC.rod` record table — the decoded-but-not-yet-interpreted structure
//! records.
//!
//! The `STRUC` section of `STRUC.rod` (decrypted + inflated by [`crate::rod`],
//! recovering the `product != 0` first-block IV offline) is a list of records,
//! one per line: `NNNNNN,<encoded>\r\n`, where `NNNNNN` is a zero-padded
//! decimal **structure id** and `<encoded>` is a packed record over a 14-symbol
//! alphabet (`[0-9,._-]`).
//!
//! ## What is proven (and what is NOT)
//! The structure-id field is cleartext decimal and the record framing is exact,
//! so this module parses `STRUC` into `(id, encoded)` rows reliably. The
//! per-record `<encoded>` payload, however, is a **packed field codec that has
//! not yet been reversed**: its 14 symbols are near-uniformly distributed
//! (max-entropy) across the corpus, yet the multiple rows of a single id are
//! near-identical templates differing in only a few positions (an incrementing
//! index / offset sub-field), i.e. the payload is structured/packed, not a
//! strong cipher and not delimited CSV. Extracting the semantic fields
//! (UDS DID, byte spec, scaling factor/offset, unit ref, name ref) requires
//! reversing the STRUC record parser in Ross-Tech's binary, which is not done.
//!
//! Therefore this module deliberately exposes only the **proven** layer: the
//! id-indexed raw records. It does NOT fabricate a `MeasurementDef` with
//! scaling — that would require the unreversed codec. Downstream code can hang
//! a future decoder off [`StrucRecord::encoded`] once the codec is cracked.

use std::collections::BTreeMap;

/// The `STRUC` payload alphabet, **proven from VCDS's own binary**: the literal
/// charset string `"0123456789,.-_"` at `0x1401898b0` in
/// `VCDS-arm64-unpacked.exe`, consumed by the radix-conversion routine
/// `fcn.1400e6f80` (which does `msub …, #0xe` — arithmetic mod 14 — against
/// this charset). So the packed payload is a **base-14** number whose symbol
/// values are: `'0'..'9'` → 0..9, `','` → 10, `'.'` → 11, `'-'` → 12, `'_'` →
/// 13.
pub const STRUC_BASE14_ALPHABET: &[u8; 14] = b"0123456789,.-_";

/// Map one payload symbol to its base-14 digit value (0..=13), or `None` if it
/// is not one of the 14 alphabet symbols.
pub fn base14_value(sym: u8) -> Option<u8> {
    STRUC_BASE14_ALPHABET
        .iter()
        .position(|&c| c == sym)
        .map(|v| v as u8)
}

/// One `STRUC` record: a structure id plus its still-encoded payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrucRecord {
    /// The decimal structure id (`000001..=001623` in the owner's corpus).
    pub id: u16,
    /// The packed 14-symbol payload (everything after the first comma), decoded
    /// only to bytes/text — the field codec is NOT applied (see module docs).
    pub encoded: String,
}

impl StrucRecord {
    /// Decode the packed payload as a **big-endian base-14 big integer** into
    /// its magnitude bytes (most-significant first), using
    /// [`STRUC_BASE14_ALPHABET`]. Returns `None` if any payload byte is outside
    /// the alphabet.
    ///
    /// ## Proven vs. not
    /// The base-14 radix + charset are proven from VCDS's binary (see the
    /// module / [`STRUC_BASE14_ALPHABET`] docs), and this decode is corroborated
    /// empirically: the multiple rows of one structure id decode to a **shared
    /// high-order prefix** (the structure template) plus a varying low-order
    /// tail (the per-channel field). What is **NOT** proven is how this integer
    /// segments into semantic fields — the boundaries and meaning of the DID /
    /// raw byte spec / scaling / unit ref / name ref within it are not yet
    /// reversed. So these bytes are the faithful packed value, **not** decoded
    /// measurement fields; do not read scaling out of them.
    pub fn decode_base14_be(&self) -> Option<Vec<u8>> {
        // Accumulate the magnitude little-endian (base 256), then reverse.
        let mut le: Vec<u8> = Vec::new();
        for &sym in self.encoded.as_bytes() {
            let d = base14_value(sym)?;
            let mut carry = d as u16;
            for b in le.iter_mut() {
                let v = (*b as u16) * 14 + carry;
                *b = (v & 0xff) as u8;
                carry = v >> 8;
            }
            while carry > 0 {
                le.push((carry & 0xff) as u8);
                carry >>= 8;
            }
        }
        if le.is_empty() {
            le.push(0);
        }
        le.reverse();
        Some(le)
    }
}

/// An id-indexed table of `STRUC` records. Separate from [`crate::LabelDb`]
/// (which is block/field-oriented for `.lbl`/`.clb`); this mirrors the ODX
/// structure-id model. Offline decode only.
#[derive(Debug, Clone, Default)]
pub struct StrucTable {
    records: Vec<StrucRecord>,
    /// id -> indices into `records` (a structure id may have multiple rows).
    by_id: BTreeMap<u16, Vec<usize>>,
}

impl StrucTable {
    /// Parse the decoded `STRUC` section plaintext into a table. Lines that are
    /// empty or not of the form `NNNNNN,<payload>` (with a numeric id) are
    /// skipped. Accepts either `\r\n` or `\n` line endings.
    pub fn parse(text: &str) -> Self {
        let mut records = Vec::new();
        let mut by_id: BTreeMap<u16, Vec<usize>> = BTreeMap::new();
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.is_empty() {
                continue;
            }
            let Some((id_str, payload)) = line.split_once(',') else {
                continue;
            };
            // The id is cleartext decimal; require it to parse (rejects any
            // stray non-record line).
            let Ok(id) = id_str.parse::<u16>() else {
                continue;
            };
            let idx = records.len();
            records.push(StrucRecord {
                id,
                encoded: payload.to_string(),
            });
            by_id.entry(id).or_default().push(idx);
        }
        StrucTable { records, by_id }
    }

    /// All records, in file order.
    pub fn records(&self) -> &[StrucRecord] {
        &self.records
    }

    /// The rows for one structure id, in file order (empty if absent).
    pub fn rows(&self, id: u16) -> impl Iterator<Item = &StrucRecord> {
        self.by_id
            .get(&id)
            .into_iter()
            .flatten()
            .map(move |&i| &self.records[i])
    }

    /// The distinct structure ids present, ascending.
    pub fn ids(&self) -> impl Iterator<Item = u16> + '_ {
        self.by_id.keys().copied()
    }

    /// Number of distinct structure ids.
    pub fn distinct_ids(&self) -> usize {
        self.by_id.len()
    }

    /// Total record count.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ids_and_payloads_and_groups_rows() {
        // Synthetic STRUC-shaped plaintext (not proprietary data).
        let text = "000001,2667._-_____5_5\r\n\
                    000001,23497_-_____7_5\r\n\
                    000003,2622223_2_2_27\r\n";
        let t = StrucTable::parse(text);
        assert_eq!(t.len(), 3);
        assert_eq!(t.distinct_ids(), 2);
        assert_eq!(t.ids().collect::<Vec<_>>(), vec![1, 3]);
        let rows1: Vec<_> = t.rows(1).map(|r| r.encoded.as_str()).collect();
        assert_eq!(rows1, vec!["2667._-_____5_5", "23497_-_____7_5"]);
        assert_eq!(t.rows(3).count(), 1);
        assert_eq!(t.rows(999).count(), 0);
    }

    #[test]
    fn base14_alphabet_maps_symbols_to_values() {
        assert_eq!(base14_value(b'0'), Some(0));
        assert_eq!(base14_value(b'9'), Some(9));
        assert_eq!(base14_value(b','), Some(10));
        assert_eq!(base14_value(b'.'), Some(11));
        assert_eq!(base14_value(b'-'), Some(12));
        assert_eq!(base14_value(b'_'), Some(13));
        assert_eq!(base14_value(b'A'), None);
        assert_eq!(base14_value(b' '), None);
    }

    #[test]
    fn decode_base14_be_matches_hand_computed_values() {
        let rec = |p: &str| StrucRecord {
            id: 1,
            encoded: p.to_string(),
        };
        // "10" = 1*14 + 0 = 14
        assert_eq!(rec("10").decode_base14_be(), Some(vec![14]));
        // "100" = 1*196 = 196
        assert_eq!(rec("100").decode_base14_be(), Some(vec![196]));
        // "_" = 13
        assert_eq!(rec("_").decode_base14_be(), Some(vec![13]));
        // ",." = 10*14 + 11 = 151
        assert_eq!(rec(",.").decode_base14_be(), Some(vec![151]));
        // "-_" = 12*14 + 13 = 181
        assert_eq!(rec("-_").decode_base14_be(), Some(vec![181]));
        // multi-byte: "111" = 1*196+1*14+1 = 211 (<256, one byte)
        assert_eq!(rec("111").decode_base14_be(), Some(vec![211]));
        // "1_0" = 196 + 13*14 + 0 = 378 = 0x017a -> [0x01,0x7a]
        assert_eq!(rec("1_0").decode_base14_be(), Some(vec![0x01, 0x7a]));
        // leading zeros contribute nothing
        assert_eq!(rec("0010").decode_base14_be(), Some(vec![14]));
        // empty payload -> zero
        assert_eq!(rec("").decode_base14_be(), Some(vec![0]));
        // out-of-alphabet symbol rejected
        assert_eq!(rec("1A").decode_base14_be(), None);
    }

    #[test]
    fn decode_base14_be_shares_high_order_prefix_across_structure_rows() {
        // Rows of one id share a high-order template, differ in the low-order
        // tail (the empirically-observed per-channel field). Uses the exact
        // bytes from the owner's STRUC.rod id 000147 (first two of eight rows).
        let a = StrucRecord {
            id: 147,
            encoded: ".348588888989808079..980".to_string(),
        }
        .decode_base14_be()
        .unwrap();
        let b = StrucRecord {
            id: 147,
            encoded: ".34858888898080807661,80".to_string(),
        }
        .decode_base14_be()
        .unwrap();
        assert_eq!(a.len(), b.len());
        // Shared high-order prefix (structure template).
        assert_eq!(&a[..6], &b[..6]);
        // Divergent low-order tail (per-channel field).
        assert_ne!(&a[6..], &b[6..]);
    }

    #[test]
    fn crib_dids_are_not_stored_as_u16_in_struc_records() {
        // Supervised-crib result (see research/rod-labels.md §M3 attack): the
        // owner's engine-running capture yields REAL valid measurement DIDs
        // (the ignition-angle family 0xA058/0xA059/0xA05E/0xA05F, proven to
        // return raw 0x5555 = 0.00°). If STRUC held the read DID, it would
        // appear as a u16 (BE or LE) at some byte offset of the decoded record.
        // These are the EXACT decoded STRUC payloads at the ids that a
        // "STRUC-id == IDE-measurement-id" mapping would predict for those
        // channels — and NONE of the crib DIDs appears in any of them. This
        // pins the negative: the DID is not stored in STRUC.
        let crib: [u16; 6] = [0xA058, 0xA059, 0xA05E, 0xA05F, 0xA051, 0xA03B];
        // Real rows from the owner's decoded STRUC.rod (ids 155/156/157/25).
        let payloads = [
            "-5-----4-4-3-11091-8",       // id 155
            "917765655555,5,5_5980_7052", // id 156
            "185278777779797617146227.",  // id 157
            "04000003090.02531_05",       // id 25
        ];
        for p in payloads {
            let bytes = StrucRecord { id: 0, encoded: p.to_string() }
                .decode_base14_be()
                .unwrap();
            for &did in &crib {
                let (hi, lo) = ((did >> 8) as u8, (did & 0xff) as u8);
                for w in bytes.windows(2) {
                    assert!(
                        !(w[0] == hi && w[1] == lo || w[0] == lo && w[1] == hi),
                        "unexpected DID {did:#06X} found in STRUC payload {p:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn skips_malformed_lines() {
        let text = "garbage line\n000005,-0-----2-2\nno-comma-here\n\n007,X\n";
        let t = StrucTable::parse(text);
        // Only the two well-formed `id,payload` lines survive.
        assert_eq!(t.len(), 2);
        assert_eq!(t.rows(5).next().unwrap().encoded, "-0-----2-2");
        assert_eq!(t.rows(7).next().unwrap().encoded, "X");
    }
}
