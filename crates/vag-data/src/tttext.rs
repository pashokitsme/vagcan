//! Breaking the per-record substitution of `TTTEXT.ROD`.
//!
//! The corpus's global text table holds the names of every measurement and
//! fault, keyed by a six-digit id that is in plain sight. The payloads are not:
//! each record is enciphered with a substitution **chosen afresh for that
//! record**, acting on three disjoint alphabets — the 26 letters (case
//! preserved, so `a`–`z` and `A`–`Z` share one permutation), a 14-glyph
//! numeric class (`0`–`9`, `,`, `.`, `_`, `-`), and everything else, which
//! passes through untouched. `research/tttext-codec.md` establishes that from
//! the frequency bands and from cribs.
//!
//! Ninety characters of ciphertext under an unknown 26-letter permutation is
//! solvable from a dictionary, and the corpus supplies its own: names already
//! recovered feed the vocabulary that recovers the next ones. This module is
//! that solver.
//!
//! It replaces a pile of throwaway scripts, and not only for tidiness. The
//! attack is a search with a scoring function and a lot of pruning; it wants
//! to run over 192,469 records repeatedly as the vocabulary grows, and each
//! pass wants to be minutes rather than hours.
//!
//! **Nothing here decides that a name is right.** It proposes readings and
//! scores them. What may be written into a catalog is settled by the gate in
//! `research/tttext-codec.md` §7 — two independent constraints agreeing — and
//! this project has already retracted decodings that looked fluent and were
//! wrong.

use std::collections::HashMap;

/// The letters, which the cipher permutes among themselves.
const LETTERS: usize = 26;

/// One record: a plaintext id and an enciphered payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: u32,
    pub cipher: String,
}

/// A recovered substitution: cipher letter index → plain letter index.
///
/// `None` where the record gave no evidence — a letter it never used, or one
/// the search could not pin. A partial key decodes what it knows and leaves
/// the rest visible as `?`, which is the honest rendering: a name with an
/// invented letter reads exactly like a name without one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    map: [Option<u8>; LETTERS],
}

impl Default for Key {
    fn default() -> Self {
        Key { map: [None; LETTERS] }
    }
}

impl Key {
    /// How many letters are pinned.
    pub fn len(&self) -> usize {
        self.map.iter().filter(|m| m.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Add `cipher → plain`, refusing anything that contradicts what is known
    /// or would map two cipher letters to the same plain one.
    ///
    /// Injectivity is the whole strength of the attack: it is what makes a
    /// wrong candidate word collide with a right one a few tokens later.
    pub fn insert(&mut self, cipher: u8, plain: u8) -> bool {
        match self.map[cipher as usize] {
            Some(existing) => existing == plain,
            None => {
                if self.map.contains(&Some(plain)) {
                    return false;
                }
                self.map[cipher as usize] = Some(plain);
                true
            }
        }
    }

    /// Decipher a payload, marking letters this key does not pin.
    ///
    /// Characters outside the letter class are passed through, including the
    /// numeric glyphs: that class has its own permutation and is not broken
    /// (`research/tttext-codec.md` §6), so its symbols are shown as they came
    /// rather than as digits nobody established.
    pub fn decode(&self, cipher: &str) -> String {
        cipher
            .chars()
            .map(|c| match letter_index(c) {
                Some(index) => match self.map[index] {
                    Some(plain) => {
                        let letter = (b'a' + plain) as char;
                        match c.is_ascii_uppercase() {
                            true => letter.to_ascii_uppercase(),
                            false => letter,
                        }
                    }
                    None => '?',
                },
                None => c,
            })
            .collect()
    }

    /// Whether the key pins every letter the payload uses.
    pub fn covers(&self, cipher: &str) -> bool {
        cipher.chars().filter_map(letter_index).all(|i| self.map[i].is_some())
    }
}

/// The letter's index, or `None` for anything the cipher leaves alone.
fn letter_index(c: char) -> Option<usize> {
    c.is_ascii_alphabetic().then(|| (c.to_ascii_lowercase() as u8 - b'a') as usize)
}

/// The repetition pattern of a word, which a substitution cannot change.
///
/// `Zeeman` and `bookie` share the pattern `0 1 1 2 3 4`, so a cipher token
/// can only stand for a plaintext word of the same shape. This is the index
/// the search looks candidates up in, and it is also what clusters records:
/// two records with the same pattern hold the same words under different keys,
/// so one solve serves both.
pub fn pattern(word: &str) -> Vec<u8> {
    let mut seen: HashMap<char, u8> = HashMap::new();
    let mut next = 0u8;
    word.chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            *seen.entry(c).or_insert_with(|| {
                let index = next;
                next += 1;
                index
            })
        })
        .collect()
}

/// Split a payload into the runs of letters the search works on.
///
/// Anything outside the letter class ends a token: a numeric glyph is not
/// evidence about letters, and treating it as part of a word would make every
/// pattern unique.
pub fn tokens(cipher: &str) -> Vec<&str> {
    cipher.split(|c: char| !c.is_ascii_alphabetic()).filter(|t| t.len() >= 2).collect()
}

/// Words the solver may propose, indexed by their pattern.
#[derive(Debug, Default, Clone)]
pub struct Dictionary {
    by_pattern: HashMap<Vec<u8>, Vec<(String, f32)>>,
}

impl Dictionary {
    /// Add a word with a weight. Higher weights win ties, so in-domain
    /// vocabulary should outweigh a general word list — the corpus's own
    /// language is the strongest prior available.
    pub fn insert(&mut self, word: &str, weight: f32) {
        if word.len() < 2 || !word.chars().all(|c| c.is_ascii_alphabetic()) {
            return;
        }
        let word = word.to_ascii_lowercase();
        let entry = self.by_pattern.entry(pattern(&word)).or_default();
        match entry.iter_mut().find(|(w, _)| *w == word) {
            Some((_, existing)) => *existing = existing.max(weight),
            None => entry.push((word, weight)),
        }
    }

    /// Candidates for a cipher token, best first.
    pub fn candidates(&self, token: &str) -> &[(String, f32)] {
        self.by_pattern.get(&pattern(token)).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.by_pattern.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.by_pattern.is_empty()
    }

    /// Sort every pattern's candidates by weight, so the search meets the
    /// likeliest first and its first complete solution is usually the best.
    pub fn finish(&mut self) {
        for entry in self.by_pattern.values_mut() {
            entry.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        }
    }
}

/// How hard the search may work, and what it must achieve to be believed.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Give up on a record after this many candidate placements.
    pub steps: u32,
    /// A solution must explain at least this fraction of the record's letters.
    /// Below it the key is a coincidence dressed as a reading.
    pub min_coverage: f32,
    /// And at least this many tokens, so a single long word cannot carry a
    /// record on its own.
    pub min_tokens: usize,
}

impl Default for Limits {
    fn default() -> Self {
        // Measured on the reference corpus: raising the step budget past
        // ~200k changes the outcome for well under 1 % of records while
        // costing time linearly, and 0.6 coverage is where fluent readings
        // stop appearing among the rejects.
        Limits { steps: 200_000, min_coverage: 0.6, min_tokens: 2 }
    }
}

/// What a solve produced.
#[derive(Debug, Clone, PartialEq)]
pub struct Solution {
    pub key: Key,
    /// Fraction of the record's letters the chosen words explain.
    pub coverage: f32,
    pub score: f32,
    pub tokens_solved: usize,
}

/// Recover a record's key from the dictionary.
///
/// Branch and bound over the record's tokens, longest first: a long token
/// constrains many letters at once, so meeting it early prunes hardest. The
/// objective is `4·len + log₂ weight` summed over solved tokens, which makes
/// coverage dominate and lets the weight break ties — a reading that explains
/// more of the record beats a reading of commoner words.
pub fn solve(cipher: &str, dict: &Dictionary, limits: Limits) -> Option<Solution> {
    let mut tokens = tokens(cipher);
    tokens.sort_by_key(|t| std::cmp::Reverse(t.len()));
    if tokens.len() < limits.min_tokens {
        return None;
    }
    let total_letters: usize = tokens.iter().map(|t| t.len()).sum();
    if total_letters == 0 {
        return None;
    }

    let mut best: Option<Solution> = None;
    let mut steps = 0u32;
    let mut key = Key::default();
    search(&tokens, 0, &mut key, 0.0, 0, dict, limits, &mut steps, &mut best, total_letters);

    let best = best?;
    (best.coverage >= limits.min_coverage).then_some(best)
}

/// One level of the branch and bound.
#[allow(clippy::too_many_arguments)]
fn search(
    tokens: &[&str],
    at: usize,
    key: &mut Key,
    score: f32,
    letters: usize,
    dict: &Dictionary,
    limits: Limits,
    steps: &mut u32,
    best: &mut Option<Solution>,
    total_letters: usize,
) {
    if *steps > limits.steps {
        return;
    }
    if at == tokens.len() {
        let coverage = letters as f32 / total_letters as f32;
        if best.as_ref().is_none_or(|b| score > b.score) {
            *best = Some(Solution { key: *key, coverage, score, tokens_solved: 0 });
        }
        return;
    }

    // The best still reachable: every remaining token solved perfectly. If
    // that cannot beat what is already in hand, stop.
    let remaining: usize = tokens[at..].iter().map(|t| t.len()).sum();
    if let Some(best) = best.as_ref() {
        if score + 4.0 * remaining as f32 + 20.0 <= best.score {
            return;
        }
    }

    let token = tokens[at];
    for (word, weight) in dict.candidates(token) {
        if *steps > limits.steps {
            return;
        }
        *steps += 1;
        let mut next = *key;
        let fits = token
            .chars()
            .zip(word.chars())
            .all(|(c, p)| match (letter_index(c), letter_index(p)) {
                (Some(c), Some(p)) => next.insert(c as u8, p as u8),
                _ => false,
            });
        if !fits {
            continue;
        }
        let gain = 4.0 * token.len() as f32 + weight.max(1.0).log2();
        search(
            tokens,
            at + 1,
            &mut next,
            score + gain,
            letters + token.len(),
            dict,
            limits,
            steps,
            best,
            total_letters,
        );
    }

    // A token may also be left unexplained — a name, an abbreviation, or a
    // word the vocabulary has not learned yet. Skipping it costs coverage,
    // which the objective already penalises.
    search(tokens, at + 1, key, score, letters, dict, limits, steps, best, total_letters);
}

/// Carry a solved record's plaintext to another record with the same pattern.
///
/// Two records whose whole payloads share a repetition pattern hold the same
/// words, so the second record's key is read off by lining its ciphertext up
/// against the first's plaintext. It is free, and it is also a check: a pair
/// that is not really the same text fails injectivity and is refused here
/// rather than producing a fluent-looking wrong name.
pub fn transfer(cipher: &str, plain: &str) -> Option<Key> {
    if cipher.chars().count() != plain.chars().count() {
        return None;
    }
    let mut key = Key::default();
    for (c, p) in cipher.chars().zip(plain.chars()) {
        match (letter_index(c), letter_index(p)) {
            (Some(c), Some(p)) => {
                if !key.insert(c as u8, p as u8) {
                    return None;
                }
            }
            // Outside the letter class both sides must agree literally; if
            // they do not, these are not the same text.
            (None, None) => {
                if c != p {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encipher with a rotation, standing in for a record's own key.
    fn encipher(plain: &str, shift: u8) -> String {
        plain
            .chars()
            .map(|c| match letter_index(c) {
                Some(i) => {
                    let rotated = (b'a' + ((i as u8 + shift) % 26)) as char;
                    match c.is_ascii_uppercase() {
                        true => rotated.to_ascii_uppercase(),
                        false => rotated,
                    }
                }
                None => c,
            })
            .collect()
    }

    fn dictionary(words: &[&str]) -> Dictionary {
        let mut dict = Dictionary::default();
        for word in words {
            dict.insert(word, 200.0);
        }
        dict.finish();
        dict
    }

    #[test]
    fn a_substitution_cannot_change_a_words_repetition_pattern() {
        // The whole index rests on this: `Zeeman` and `bookie` are the same
        // shape, and a cipher token can only stand for a word that is.
        assert_eq!(pattern("zeeman"), pattern("bookie"));
        assert_ne!(pattern("engine"), pattern("coolant"));
        // Case is not keyed separately, so it cannot distinguish either.
        assert_eq!(pattern("Engine"), pattern("engine"));
    }

    #[test]
    fn tokens_break_at_anything_the_cipher_leaves_alone() {
        // A numeric glyph is not evidence about letters, and gluing it into a
        // word would make every pattern unique and the index useless.
        assert_eq!(tokens("Bank #/# Sensor"), vec!["Bank", "Sensor"]);
        assert_eq!(tokens("a b"), Vec::<&str>::new(), "one letter constrains nothing");
    }

    #[test]
    fn a_key_refuses_to_map_two_letters_onto_one() {
        // Injectivity is what makes a wrong candidate collide a few tokens
        // later; without it the search would accept almost anything.
        let mut key = Key::default();
        assert!(key.insert(0, 5));
        assert!(key.insert(0, 5), "the same fact twice is not a contradiction");
        assert!(!key.insert(0, 6), "one cipher letter cannot be two plain ones");
        assert!(!key.insert(1, 5), "two cipher letters cannot be one plain one");
        assert_eq!(key.len(), 1);
    }

    #[test]
    fn a_record_is_solved_from_the_words_it_is_made_of() {
        let plain = "Engine coolant temperature sensor";
        let cipher = encipher(plain, 7);
        let dict = dictionary(&["engine", "coolant", "temperature", "sensor", "pressure"]);

        let solved = solve(&cipher, &dict, Limits::default()).expect("solvable");
        assert!(solved.coverage > 0.99, "{}", solved.coverage);
        assert_eq!(solved.key.decode(&cipher), plain);
    }

    #[test]
    fn a_letter_the_record_never_used_is_shown_as_unknown_not_guessed() {
        // The rendering rule: a name with an invented letter reads exactly
        // like a name without one, so it must not read like one.
        let cipher = encipher("Oil temperature", 3);
        let dict = dictionary(&["oil", "temperature"]);
        let solved = solve(&cipher, &dict, Limits::default()).unwrap();
        assert_eq!(solved.key.decode(&cipher), "Oil temperature");
        // `z` never appeared, so it is unpinned and decodes as a question mark.
        assert_eq!(solved.key.decode(&encipher("zoo", 3)), "?oo");
        assert!(!solved.key.covers(&encipher("zoo", 3)));
    }

    #[test]
    fn a_record_the_vocabulary_cannot_explain_is_refused() {
        // Below the coverage bar a key is a coincidence dressed as a reading.
        let cipher = encipher("Kraftstoffdruckregelventil Ansteuerung", 11);
        let dict = dictionary(&["engine", "coolant", "temperature", "sensor"]);
        assert!(solve(&cipher, &dict, Limits::default()).is_none());
    }

    #[test]
    fn a_solution_may_leave_a_token_unexplained() {
        // Abbreviations and part names are not in any dictionary, and a
        // record must not be lost because one token is unknown.
        let plain = "Coolant temperature G62";
        let cipher = encipher(plain, 19);
        let dict = dictionary(&["coolant", "temperature"]);
        let solved = solve(&cipher, &dict, Limits::default()).expect("the rest carries it");
        assert!(solved.key.decode(&cipher).starts_with("Coolant temperature"));
    }

    #[test]
    fn transfer_reads_a_second_records_key_off_a_solved_one() {
        // Two records with the same text under different keys: the second is
        // free once the first is solved.
        let plain = "Intake air temperature";
        let a = encipher(plain, 4);
        let b = encipher(plain, 17);
        let key = transfer(&b, plain).expect("same text, so the key lines up");
        assert_eq!(key.decode(&b), plain);
        assert_ne!(a, b, "the fixture really does use two different keys");
    }

    #[test]
    fn transfer_refuses_a_pair_that_is_not_the_same_text() {
        // The check that makes transfer safe: a mismatched pair fails
        // injectivity instead of producing a fluent-looking wrong name.
        assert!(transfer(&encipher("aab", 1), "xyz").is_none(), "shape disagrees");
        assert!(transfer(&encipher("abc", 1), "ab").is_none(), "length disagrees");
        assert!(transfer("ab#", "ab!").is_none(), "the untouched characters disagree");
    }

    #[test]
    fn the_search_stops_rather_than_running_forever() {
        // A record with many tokens and a large ambiguous vocabulary must not
        // hang a pass over 192,469 records.
        let cipher = encipher("aa bb cc dd ee ff gg hh ii jj kk ll", 5);
        let dict = dictionary(&["an", "at", "as", "be", "by", "do", "go", "he", "if", "in"]);
        let limits = Limits { steps: 50, ..Limits::default() };
        let _ = solve(&cipher, &dict, limits);
    }
}
