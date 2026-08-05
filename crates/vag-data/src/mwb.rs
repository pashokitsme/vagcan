//! Engine `MWB` (Measuring Value Blocks) rows from an engine `.rod` file.
//!
//! An engine `.rod`'s `MWB` section (decrypted + inflated by [`crate::rod`],
//! recovering the `product != 0` first-block IV offline) lists the ECU's
//! measurements, one per line: `NNNNNN,<code>\r\n`, where `NNNNNN` is a decimal
//! **text-id** (a pointer into the `TTTEXT` name table — the measurement's human
//! name) and `<code>` is a short **2-character code**.
//!
//! ## What is proven (and what is NOT)
//! The row framing is exact and the two fields are cleartext, so [`parse_mwb`]
//! reliably yields `(text_id, code)`. The text-id → name join (via `TTTEXT`) is
//! a straightforward decrypt-only lookup.
//!
//! The **2-char code → structure/scaling** link is **NOT reversed**. Its
//! character set is exactly the 40 symbols [`MWB_CODE_SYMBOLS`]
//! (`0-9 A-Z , . - _`), and a base-40 reading has the right ceiling (40² = 1600,
//! vs `STRUC.rod`'s 1623 structure ids), but **base-40 `code → STRUC-id` is
//! unproven**: on the owner-corpus MWB list it lands in-range only ~3σ above the
//! chance baseline (≈188/221 vs ≈168 expected, across every alphabet
//! order/endianness/offset), the code does not echo into the mapped STRUC
//! record, and no base-40 charset/arithmetic exists in VCDS's binary (the `#0x28`
//! = 40 constants there are struct-field offsets, e.g. in the TTDOP/MUX loader
//! `fcn.140028e28`, not a radix). So this module deliberately exposes only the
//! proven row parse; it does **not** provide a `code → STRUC-id` function, which
//! would be an invented mapping. See `research/labels/rod-labels.md` for the full
//! table graph (MWB → STRUC → TTDOP/DOP, all base-14 packed).

/// The 40 distinct symbols observed in `MWB` 2-char codes (`0-9 A-Z , . - _`).
/// The **set** is proven (it is exactly the symbols that appear); the base-40
/// **ordering/radix** that would turn a code into a table index is NOT proven,
/// so this is exposed as an unordered reference set, not an alphabet.
pub const MWB_CODE_SYMBOLS: &[u8; 40] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ,.-_";

/// One engine `MWB` row: a `TTTEXT` name pointer plus the still-opaque 2-char
/// code. `text_id` is `u32` because engine text-ids exceed `u16` (observed up
/// to ~152,526).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MwbEntry {
    /// Decimal text-id: a pointer into the `TTTEXT` name table (the name).
    pub text_id: u32,
    /// The 2-character code (structure/scaling reference — mapping unproven).
    pub code: String,
}

/// Parse a decoded engine `MWB` section into its rows. Lines that are empty or
/// not of the form `NNNNNN,<code>` (numeric text-id) are skipped. Accepts `\r\n`
/// or `\n` line endings.
pub fn parse_mwb(text: &str) -> Vec<MwbEntry> {
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let Some((id_str, code)) = line.split_once(',') else {
            continue;
        };
        let Ok(text_id) = id_str.parse::<u32>() else {
            continue;
        };
        out.push(MwbEntry {
            text_id,
            code: code.to_string(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_id_and_code() {
        // Exact rows from the decoded VW48 MWB section (not proprietary data).
        let text = "043439,4.\r\n043900,_5\r\n095490,23\r\n011809,B_\r\n";
        let rows = parse_mwb(text);
        assert_eq!(
            rows,
            vec![
                MwbEntry { text_id: 43439, code: "4.".into() },
                MwbEntry { text_id: 43900, code: "_5".into() },
                MwbEntry { text_id: 95490, code: "23".into() },
                MwbEntry { text_id: 11809, code: "B_".into() },
            ]
        );
    }

    #[test]
    fn text_id_exceeding_u16_is_kept() {
        // Engine text-ids exceed u16::MAX (65535); must not truncate/overflow.
        let rows = parse_mwb("152526,ZB\n");
        assert_eq!(rows, vec![MwbEntry { text_id: 152526, code: "ZB".into() }]);
    }

    #[test]
    fn skips_malformed_lines() {
        let rows = parse_mwb("junk\n\n042989,3Q\nno-comma\n");
        assert_eq!(rows, vec![MwbEntry { text_id: 42989, code: "3Q".into() }]);
    }

    #[test]
    fn code_symbol_set_is_the_expected_40() {
        assert_eq!(MWB_CODE_SYMBOLS.len(), 40);
        // digits + A-Z + the four base-14 punctuation symbols, all distinct.
        let mut seen = std::collections::BTreeSet::new();
        for &b in MWB_CODE_SYMBOLS {
            assert!(seen.insert(b), "duplicate symbol {}", b as char);
        }
        for &b in b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ,.-_" {
            assert!(seen.contains(&b));
        }
    }
}
