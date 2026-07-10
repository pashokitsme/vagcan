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

/// One `STRUC` record: a structure id plus its still-encoded payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrucRecord {
    /// The decimal structure id (`000001..=001623` in the owner's corpus).
    pub id: u16,
    /// The packed 14-symbol payload (everything after the first comma), decoded
    /// only to bytes/text — the field codec is NOT applied (see module docs).
    pub encoded: String,
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
    fn skips_malformed_lines() {
        let text = "garbage line\n000005,-0-----2-2\nno-comma-here\n\n007,X\n";
        let t = StrucTable::parse(text);
        // Only the two well-formed `id,payload` lines survive.
        assert_eq!(t.len(), 2);
        assert_eq!(t.rows(5).next().unwrap().encoded, "-0-----2-2");
        assert_eq!(t.rows(7).next().unwrap().encoded, "X");
    }
}
