//! Reading the numbers inside a `.rod` table.
//!
//! Numeric fields in these tables are written in a per-table substitution
//! alphabet: ten glyphs drawn from `0-9`, `.`, `-` and `_`, in an order that
//! differs from table to table. Every earlier attempt on it — frequency
//! analysis, known-plaintext from the car, the two-character code beside each
//! row — failed, and `research/label-linkage.md` §2.4 recorded it as the
//! blocker it was.
//!
//! **The substitution is order-revealing, and the rows are sorted.** Rows in a
//! table appear in ascending order of their *plaintext*, so for any two
//! consecutive rows the first position where they differ says which of two
//! glyphs is the smaller digit. Collect that over a table and the constraints
//! order the whole alphabet; no plaintext is needed anywhere.
//!
//! Evidence, each with the value that would have refuted it
//! (`research/whole-car-survey.md` §3):
//!
//! * the constraint graph is acyclic in **10 916 of 10 916** tables of the
//!   global fault registry — the same rows shuffled give 7 258 cycles;
//! * 14 958 values decoded from 680 independently-keyed tables collapse to
//!   **2 080 distinct** numbers, where random per-table maps give 10 392, and
//!   one value is reached identically by 42 different keys;
//! * of 2 143 decoded values read as 24-bit fault codes, **66.5 % end in
//!   `0xF0`** and 95 % in `0xF0..0xF7`, against 0.2 % for random maps.

use std::collections::{BTreeMap, BTreeSet};

/// The glyphs a numeric field can be written with. Anything else in a row is
/// structure — a separator or a field the substitution does not cover.
pub const NUMERIC_GLYPHS: &[char] = &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '.', '-', '_'];

/// A recovered alphabet: glyph → digit.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct DigitOrder {
    map: BTreeMap<char, u8>,
}

impl DigitOrder {
    /// The digit a glyph stands for.
    pub fn digit(&self, glyph: char) -> Option<u8> {
        self.map.get(&glyph).copied()
    }

    /// How many glyphs are pinned. Ten is a complete alphabet; fewer means the
    /// table did not exercise the rest.
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Read one enciphered numeric field.
    ///
    /// `None` if any glyph is unaccounted for — a partly-decoded number is a
    /// wrong number, and this project has a rule about those.
    pub fn decode(&self, text: &str) -> Option<u64> {
        let mut out = 0u64;
        for glyph in text.chars() {
            out = out.checked_mul(10)?.checked_add(self.digit(glyph)? as u64)?;
        }
        (!text.is_empty()).then_some(out)
    }
}

/// Recover a table's alphabet from the order its rows are stored in.
///
/// `rows` must be in file order, and `ignore` lists glyphs that are structure
/// rather than digits in this table — the field separator above all, which
/// otherwise contributes constraints from a position that is not a digit.
///
/// Returns `None` when the constraints do not pin all ten digits: a partial
/// order would decode some rows and silently mis-decode others.
pub fn digit_order(rows: &[&str], ignore: &[char]) -> Option<DigitOrder> {
    let mut greater: BTreeMap<char, BTreeSet<char>> = BTreeMap::new();
    let mut seen: BTreeSet<char> = BTreeSet::new();

    for pair in rows.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        // The first position where consecutive rows differ orders those two
        // glyphs: the rows are sorted, so the earlier row's glyph is smaller.
        let Some((x, y)) = a.chars().zip(b.chars()).find(|(x, y)| x != y) else {
            continue;
        };
        if ignore.contains(&x) || ignore.contains(&y) {
            continue;
        }
        if !NUMERIC_GLYPHS.contains(&x) || !NUMERIC_GLYPHS.contains(&y) {
            continue;
        }
        seen.insert(x);
        seen.insert(y);
        greater.entry(x).or_default().insert(y);
    }

    if seen.len() != 10 {
        return None;
    }

    // Kahn's algorithm. A cycle means the rows were not sorted the way this
    // rests on, and the answer would be an invention.
    let mut incoming: BTreeMap<char, usize> = seen.iter().map(|g| (*g, 0)).collect();
    for targets in greater.values() {
        for target in targets {
            *incoming.entry(*target).or_default() += 1;
        }
    }
    let mut ready: Vec<char> = incoming
        .iter()
        .filter(|(_, n)| **n == 0)
        .map(|(g, _)| *g)
        .collect();
    let mut order = Vec::new();
    while let Some(glyph) = ready.pop() {
        order.push(glyph);
        if let Some(targets) = greater.get(&glyph) {
            for target in targets {
                let count = incoming.get_mut(target).expect("every target was seen");
                *count -= 1;
                if *count == 0 {
                    ready.push(*target);
                }
            }
        }
        // More than one glyph ready at once means the order is not total, and
        // the digits it would assign are a guess between them.
        if ready.len() > 1 {
            return None;
        }
    }
    if order.len() != 10 {
        return None;
    }

    Some(DigitOrder {
        map: order.iter().enumerate().map(|(digit, glyph)| (*glyph, digit as u8)).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A table whose alphabet is the one recovered from the global registry's
    /// fault-531 table: `0 . - 8 3 2 1 5 7 4` standing for `0123456789`.
    ///
    /// The rows are the ten one-digit values in ascending plaintext order,
    /// which is how such a table appears in the file, and is exactly the
    /// evidence the method uses.
    fn table_531() -> Vec<&'static str> {
        vec!["0", ".", "-", "8", "3", "2", "1", "5", "7", "4"]
    }

    #[test]
    fn the_order_of_the_rows_gives_the_order_of_the_digits() {
        let order = digit_order(&table_531(), &[]).expect("ten glyphs, one total order");
        assert_eq!(order.len(), 10);
        assert_eq!(order.digit('0'), Some(0));
        assert_eq!(order.digit('.'), Some(1));
        assert_eq!(order.digit('-'), Some(2));
        assert_eq!(order.digit('4'), Some(9));
    }

    #[test]
    fn the_recovered_alphabet_reproduces_the_worked_values() {
        // Three values decoded by hand from this table when the method was
        // first established. If the mapping drifts, these stop matching.
        let order = digit_order(&table_531(), &[]).unwrap();
        assert_eq!(order.decode(".0374730"), Some(10_489_840));
        assert_eq!(order.decode("4527503"), Some(9_758_704));
        assert_eq!(order.decode(".-0238"), Some(120_543));
    }

    #[test]
    fn a_glyph_the_table_never_ordered_makes_the_answer_none() {
        // Refusing beats decoding eight digits of ten: a number with one
        // wrong digit reads exactly like a right one.
        let order = digit_order(&table_531(), &[]).unwrap();
        assert_eq!(order.decode("0.9"), None, "9 is not in this table's alphabet");
        assert_eq!(order.decode(""), None);
    }

    #[test]
    fn a_table_that_does_not_pin_ten_digits_is_refused() {
        // Two rows cannot order ten glyphs, and guessing the rest would be an
        // invention with no evidence behind it.
        assert!(digit_order(&["00000", "0000."], &[]).is_none());
        assert!(digit_order(&[], &[]).is_none());
    }

    #[test]
    fn rows_out_of_order_are_refused_rather_than_answered() {
        // The whole method rests on the rows being sorted. Contradictory
        // constraints make the graph cyclic, and that must come back as "no
        // answer" — the check that failed 7 258 times on shuffled rows of the
        // real file and never once on the file as shipped.
        let cyclic = vec!["0", ".", "-", "0", ".", "-", "8", "3", "2", "1", "5", "7", "4"];
        assert!(digit_order(&cyclic, &[]).is_none());
    }

    #[test]
    fn an_ignored_glyph_contributes_no_constraint() {
        // The field separator sits among the digits and would otherwise order
        // something that is not one. Ignoring a glyph must remove it from the
        // alphabet entirely, not merely from the output.
        let rows = table_531();
        assert!(digit_order(&rows, &[]).is_some());
        assert!(
            digit_order(&rows, &['.']).is_none(),
            "with one glyph ignored the table pins nine digits, which is not an answer"
        );
    }
}
