//! `vagcan vcds tttext` — recover names from the corpus's global text table.
//!
//! Was the `vag-tttext` binary. Every record of `TTTEXT.ROD`'s `[TXT]` section
//! is enciphered under its own substitution, so there is no single key to find:
//! the attack is dictionary-driven and bootstraps, with words read off records
//! it solves becoming vocabulary for the next pass.
//!
//! Only records read with **no unresolved letter** are output. A partial reading
//! is not a weaker result — it is an unmarked guess, and a name with a guessed
//! letter reads exactly like a name without one.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;

use anyhow::{Context, Result};

use vag_data::tttext::{self, Dictionary, Limits, Record};

/// How far the best reading of a token must outweigh the runner-up before the
/// completion step will pin its letters. A likelihood ratio, not a proof: at
/// 20x a token with a real second reading is left unpinned and the record is
/// dropped, which is the direction to err in.
const MARGIN: f32 = 20.0;

/// How many letters a reading must have before it is worth keeping.
///
/// `research/labels/tttext-codec.md` §7. Under a dozen there is too little
/// evidence for the vocabulary check to mean anything, and the acronyms that
/// dominate below it are not names.
const MIN_LETTERS: usize = 12;

pub struct Options<'a> {
    pub file: &'a str,
    pub words: &'a [String],
    pub names: Option<&'a str>,
    pub out: Option<&'a str>,
    pub catalog: Option<&'a str>,
    pub partial: Option<&'a str>,
    pub passes: usize,
    pub steps: Option<u32>,
    pub check: usize,
    pub gated: bool,
}

pub fn run(opts: Options<'_>) -> Result<()> {
    let mut limits = Limits::default();
    if let Some(steps) = opts.steps {
        limits.steps = steps;
    }

    let text = std::fs::read(opts.file).with_context(|| format!("reading {:?}", opts.file))?;
    let records = parse(&text);
    eprintln!("{} records", records.len());

    // The vocabulary. In-domain words outweigh a general list heavily: the
    // corpus's own language is the strongest prior available, and a general
    // dictionary is there to catch the rest.
    let mut dict = Dictionary::default();
    let mut known: HashSet<String> = HashSet::new();
    if let Some(path) = opts.names {
        for word in words_of_json(path) {
            known.insert(word.clone());
            dict.insert(&word, 400.0);
        }
        eprintln!("{} words from names already recovered", known.len());
    }
    for spec in opts.words {
        // `FILE` or `FILE:WEIGHT`. The weight is the prior: the corpus's own
        // label files are in-domain and must outrank a general word list, or
        // the search prefers an English rarity to the term the corpus uses.
        let (path, weight) =
            match spec.rsplit_once(':').and_then(|(p, w)| Some((p, w.parse().ok()?))) {
                Some((path, weight)) => (path, weight),
                None => (spec.as_str(), 8.0f32),
            };
        let files = word_files(std::path::Path::new(path));
        if files.is_empty() {
            eprintln!("skipping {path}: nothing readable there");
            continue;
        }
        let before = known.len();
        for file in &files {
            // Read as bytes, not as a string. A VCDS `Labels/` directory is
            // Latin-1 and holds umlauts, so `read_to_string` refuses the whole
            // file — which silently cost the run its entire in-domain
            // vocabulary and left an English word list deciding what VW calls
            // things. Only the ASCII letters are wanted anyway.
            let Ok(bytes) = std::fs::read(file) else { continue };
            for word in bytes.split(|b| !b.is_ascii_alphabetic()) {
                if word.len() < 2 {
                    continue;
                }
                let word = String::from_utf8_lossy(word).to_ascii_lowercase();
                if known.insert(word.clone()) {
                    dict.insert(&word, weight);
                }
            }
        }
        eprintln!(
            "{} words from {path} ({} file(s)) at weight {weight}",
            known.len() - before,
            files.len()
        );
    }
    dict.finish();
    anyhow::ensure!(!dict.is_empty(), "no vocabulary: pass --names and/or --words");

    // The vocabulary as it stands *before* the bootstrap, kept for the catalog
    // gate. This is not a micro-optimisation, it is the difference between a
    // check and a tautology: the passes below feed words read off solved
    // records back into the dictionary at weight 300, so a wrong decode teaches
    // the gate its own misreadings and then passes them. Measured on the
    // reference corpus, gating against the grown dictionary let
    // `Ejzc xjpx dyjje agrope acpcj cgijfbc` through as a name; against the
    // seed it is six words nothing has ever attested and the record is dropped.
    let seed_dict = dict.clone();
    let seed_words = known.clone();

    // Records sharing the repetition pattern of their *letter runs* hold the
    // same words under different keys, so one solve serves the cluster. The
    // pattern of the whole payload would not do: the trailing numeric fields
    // differ between records that carry identical text, which splits the
    // cluster and throws away the free members.
    let mut clusters: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        let tokens = tttext::tokens(&record.cipher);
        if tokens.len() >= 2 {
            clusters.entry(tttext::pattern(&tokens.join("|"))).or_default().push(index);
        }
    }
    eprintln!("{} clusters with something to solve", clusters.len());

    let mut solved: HashMap<usize, String> = HashMap::new();
    let mut partials: Vec<(usize, String)> = Vec::new();
    for pass in 1..=opts.passes {
        let mut learned = 0usize;
        let mut fresh = 0usize;
        let mut partial = 0usize;
        partials.clear();

        for members in clusters.values() {
            // A cluster already read needs no second look.
            if members.iter().any(|m| solved.contains_key(m)) {
                continue;
            }
            let leader = members[0];
            let cipher = &records[leader].cipher;
            let Some(solution) = tttext::solve(cipher, &dict, limits) else { continue };
            // The search stops at its best-scoring reading, which routinely
            // leaves a token unexplained; the letters the rest of the record
            // pins are evidence it did not have at the time, so re-filter that
            // token against them before giving the record up.
            let key = match solution.key.covers(cipher) {
                true => solution.key,
                false => tttext::complete(cipher, &solution.key, &dict, MARGIN),
            };
            if !key.covers(cipher) {
                partial += 1;
                partials.push((leader, key.decode(cipher)));
                continue;
            }
            let plain = key.decode(cipher);
            fresh += 1;
            solved.insert(leader, plain.clone());

            // Every other member is free — and checked: a member whose tokens
            // do not line up letter for letter is not the same text and is
            // dropped. Only the letter runs are compared, because that is what
            // the cluster claims is shared.
            let words: Vec<&str> = tttext::tokens(&plain);
            for member in &members[1..] {
                let cipher = &records[*member].cipher;
                let Some(key) = transfer_tokens(cipher, &words) else { continue };
                if key.covers(cipher) {
                    solved.insert(*member, key.decode(cipher));
                }
            }

            // Words this reading contributes, for the next pass.
            for word in plain.split(|c: char| !c.is_ascii_alphabetic()) {
                if word.len() >= 3 && known.insert(word.to_ascii_lowercase()) {
                    dict.insert(word, 300.0);
                    learned += 1;
                }
            }
        }
        dict.finish();
        eprintln!(
            "pass {pass}: {fresh} clusters read ({} records), {learned} new words, \
             {partial} rejected for unresolved letters",
            solved.len()
        );
        if learned == 0 {
            break;
        }
    }

    if opts.check > 0 {
        recheck(&records, &clusters, &solved, &dict, limits, opts.check, opts.gated, &known);
    }

    if let Some(path) = opts.partial {
        let mut sink = std::io::BufWriter::new(
            std::fs::File::create(path)
                .with_context(|| format!("creating the partial output file {path:?}"))?,
        );
        for (index, plain) in &partials {
            writeln!(sink, "{:06}\t{plain}", records[*index].id)?;
        }
        eprintln!("{} partial readings written to {path}", partials.len());
    }

    if let Some(path) = opts.catalog {
        write_catalog(path, &records, &solved, &seed_dict, &seed_words)?;
    }

    // Readings go to stdout when nobody said where to put them — unless a
    // catalog was asked for, in which case the gated names *are* the answer and
    // a hundred thousand ungated lines scrolling past them is not a default
    // anyone wants.
    let mut sink: Option<Box<dyn Write>> = match (opts.out, opts.catalog) {
        (Some(path), _) => Some(Box::new(std::io::BufWriter::new(
            std::fs::File::create(path)
                .with_context(|| format!("creating the output file {path:?}"))?,
        ))),
        (None, Some(_)) => None,
        (None, None) => Some(Box::new(std::io::stdout().lock())),
    };
    let mut ordered: Vec<(u32, &String)> =
        solved.iter().map(|(index, plain)| (records[*index].id, plain)).collect();
    ordered.sort_unstable();
    if let Some(sink) = sink.as_mut() {
        for (id, plain) in &ordered {
            writeln!(sink, "{id:06}\t{plain}")?;
        }
    }
    eprintln!(
        "{} of {} records read ({:.1} %)",
        ordered.len(),
        records.len(),
        100.0 * ordered.len() as f32 / records.len() as f32
    );
    Ok(())
}

/// Validation, and it is not optional.
///
/// Within a cluster the member's cipher pattern equals the leader's *plaintext*
/// pattern by construction, so [`transfer_tokens`] can never fail injectivity —
/// it is free, but it is not a check. The check is to solve a member from
/// scratch, under its own key and its own search path, and see whether it says
/// the same thing. Two plaintexts can share a repetition pattern; this is what
/// would catch it.
#[allow(clippy::too_many_arguments)]
fn recheck(
    records: &[Record],
    clusters: &BTreeMap<Vec<u8>, Vec<usize>>,
    solved: &HashMap<usize, String>,
    dict: &Dictionary,
    limits: Limits,
    check: usize,
    gated: bool,
    known: &HashSet<String>,
) {
    let mut members: Vec<(usize, &String)> = Vec::new();
    for group in clusters.values() {
        for member in &group[1..] {
            match solved.get(member) {
                // Only readings a catalog would consider. Sampling every
                // transferred member measures the transfer over acronym soup
                // that no gate would ship, which is not the number that matters
                // and is not what the published 599/600 was measured on.
                Some(plain) if !gated || shippable(plain, known) => members.push((*member, plain)),
                _ => {}
            }
        }
    }
    // A fixed stride over the sorted list rather than a random sample:
    // reproducible, and it cannot be accused of picking the easy ones.
    let stride = (members.len() / check.max(1)).max(1);
    let (mut agree, mut disagree, mut unusable) = (0usize, 0usize, 0usize);
    let mut examples: Vec<(u32, String, String)> = Vec::new();
    for (index, transferred) in members.iter().step_by(stride).take(check) {
        let cipher = &records[*index].cipher;
        let Some(solution) = tttext::solve(cipher, dict, limits) else {
            unusable += 1;
            continue;
        };
        let key = tttext::complete(cipher, &solution.key, dict, MARGIN);
        if !key.covers(cipher) {
            unusable += 1;
            continue;
        }
        let fresh = key.decode(cipher);
        if fresh.eq_ignore_ascii_case(transferred) {
            agree += 1;
        } else {
            disagree += 1;
            if examples.len() < 20 {
                examples.push((records[*index].id, (*transferred).clone(), fresh));
            }
        }
    }
    eprintln!(
        "independent re-solve of transferred members: agree {agree} disagree {disagree} \
         unusable {unusable}"
    );
    for (id, transferred, fresh) in &examples {
        eprintln!("  {id:06} transferred {transferred:?} != re-solved {fresh:?}");
    }
}

/// Every file a `--words` argument names: the file itself, or a directory's
/// worth.
///
/// A directory is the ordinary case now that `vagcan setup` passes the VCDS
/// `Labels/` tree straight in. The recursion is what makes that work on an
/// install root as well as on `Labels/` itself.
fn word_files(at: &std::path::Path) -> Vec<std::path::PathBuf> {
    if at.is_file() {
        return vec![at.to_path_buf()];
    }
    let Ok(entries) = std::fs::read_dir(at) else { return Vec::new() };
    let mut out: Vec<std::path::PathBuf> = Vec::new();
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            dirs.push(path);
        } else {
            out.push(path);
        }
    }
    // Sorted, so two runs over one corpus build the same dictionary and read
    // the same names out of it.
    out.sort();
    dirs.sort();
    for dir in dirs {
        out.extend(word_files(&dir));
    }
    out
}

/// Write the readings that clear §7's gate as a `{"<text id>": "<name>"}`
/// catalog.
///
/// **The gate is the product, not the decode.** 61 % of the section decodes;
/// what a name catalog may contain is far less than that, because a fluent
/// wrong reading is indistinguishable from a right one at the point of use.
/// `research/labels/tttext-codec.md` §7 states the filters and this applies
/// them: the framing rule, no unresolved letter, no digit, at least
/// [`MIN_LETTERS`] letters, every word of length ≥ 3 a word the vocabulary
/// knows, the [`MARGIN`] ambiguity check per token, and no name twice.
fn write_catalog(
    path: &str,
    records: &[Record],
    solved: &HashMap<usize, String>,
    dict: &Dictionary,
    known: &HashSet<String>,
) -> Result<()> {
    // By name first: two records reading the same way is the signature of a
    // truncated enumeration (`… of cylinder 4` losing its digit), so both go.
    let mut by_name: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for (index, plain) in solved {
        let Some(name) = gated_name(&records[*index].cipher, plain, dict, known) else { continue };
        by_name.entry(name).or_default().push(records[*index].id);
    }

    let mut catalog: BTreeMap<String, String> = BTreeMap::new();
    let mut duplicated = 0usize;
    for (name, ids) in by_name {
        if ids.len() > 1 {
            duplicated += ids.len();
            continue;
        }
        catalog.insert(format!("{:06}", ids[0]), name);
    }

    if let Some(parent) = std::path::Path::new(path).parent().filter(|p| !p.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, serde_json::to_string_pretty(&catalog)?)
        .with_context(|| format!("writing the name catalog {path:?}"))?;
    eprintln!(
        "{} names written to {path} ({} dropped for reading the same as another record)",
        catalog.len(),
        duplicated
    );
    Ok(())
}

/// The name a record contributes to the catalog, or nothing.
///
/// Nothing is the ordinary answer. A partial reading is not a weaker result but
/// an unmarked guess, and the same is true of an ambiguous one — see
/// [`unambiguous`].
fn gated_name(
    cipher: &str,
    plain: &str,
    dict: &Dictionary,
    known: &HashSet<String>,
) -> Option<String> {
    if plain.contains('?') {
        return None;
    }
    let name = framed(plain)?;
    // Digits are not recovered at all (§6): the glyph class that carries them
    // is unbroken, so a name containing one contains a guess.
    if name.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    if name.chars().filter(|c| c.is_ascii_alphabetic()).count() < MIN_LETTERS {
        return None;
    }
    if !name
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| w.len() >= 3)
        .all(|w| known.contains(&w.to_ascii_lowercase()))
    {
        return None;
    }
    unambiguous(cipher, plain, dict, MARGIN).then_some(name)
}

/// The name, with the record's trailing field run removed.
///
/// A record is `<name><sep><digit><sep><number>` and the tail is inside the
/// glyph class, so it survives the decode as noise. Stripping the trailing run
/// of non-letters is right — unless the *name itself* ended in a digit, in
/// which case the digit is inside the run and gets eaten, silently turning
/// `… of cylinder 4` into `… of cylinder`. The guard is that the run's first
/// character must recur later in it: that is what makes it a separator rather
/// than the end of the name.
fn framed(plain: &str) -> Option<String> {
    let cut = plain.trim_end().len()
        - plain
            .trim_end()
            .chars()
            .rev()
            .take_while(|c| !c.is_ascii_alphabetic() && *c != ' ')
            .map(char::len_utf8)
            .sum::<usize>();
    let (name, run) = plain.trim_end().split_at(cut);
    let mut run = run.chars();
    let first = run.next()?;
    run.any(|c| c == first).then(|| name.trim_end().to_string())
}

/// Whether every word of the reading beats its best alternative by `margin`.
///
/// This is §7's ambiguity filter and it is the one that matters. `Hill bytes to
/// maintain backward compatibility` is fluent, dictionary-clean and stable
/// across keys, and the word is `Fill`: a letter occurring once in a record is
/// pinned by nothing but the dictionary. So each token is re-read against every
/// word of its pattern class that the *rest of the record* allows, and a token
/// whose winner does not outweigh the runner-up by `margin` costs the record.
///
/// It is deliberately re-derived here rather than trusted from the solve. The
/// search stops at its best-scoring reading and has no opinion about how close
/// second place was.
fn unambiguous(cipher: &str, plain: &str, dict: &Dictionary, margin: f32) -> bool {
    let ciphered = tttext::tokens(cipher);
    let read = tttext::tokens(plain);
    if ciphered.len() != read.len() {
        return false;
    }
    for (at, token) in ciphered.iter().enumerate() {
        // Short tokens are not dictionary-checked anywhere in §7 — there is no
        // vocabulary at two letters, only noise.
        if token.len() < 3 {
            continue;
        }
        let Some(rest) = pinned_by_others(&ciphered, &read, at) else { return false };
        let (mut chosen, mut runner_up) = (0.0f32, 0.0f32);
        for (word, weight) in dict.candidates(token) {
            if !fits(token, word, rest) {
                continue;
            }
            if word.eq_ignore_ascii_case(read[at]) {
                chosen = chosen.max(*weight);
            } else {
                runner_up = runner_up.max(*weight);
            }
        }
        // A reading the dictionary does not contain at all cannot be defended;
        // one the dictionary ranks below an alternative is a coin toss.
        if chosen == 0.0 || (runner_up > 0.0 && chosen < margin * runner_up) {
            return false;
        }
    }
    true
}

/// The letter mapping every token *except* `at` forces.
fn pinned_by_others(ciphered: &[&str], read: &[&str], at: usize) -> Option<tttext::Key> {
    let mut key = tttext::Key::default();
    for (index, token) in ciphered.iter().enumerate() {
        if index == at {
            continue;
        }
        for (c, p) in token.bytes().zip(read[index].bytes()) {
            let (c, p) = (c.to_ascii_lowercase(), p.to_ascii_lowercase());
            if !c.is_ascii_lowercase() || !p.is_ascii_lowercase() || !key.insert(c - b'a', p - b'a')
            {
                return None;
            }
        }
    }
    Some(key)
}

/// Whether `word` can be this token's reading given the letters already pinned.
fn fits(token: &str, word: &str, mut pinned: tttext::Key) -> bool {
    token.len() == word.len()
        && token.bytes().zip(word.bytes()).all(|(c, p)| {
            let (c, p) = (c.to_ascii_lowercase(), p.to_ascii_lowercase());
            c.is_ascii_lowercase() && p.is_ascii_lowercase() && pinned.insert(c - b'a', p - b'a')
        })
}

/// Whether a reading is the kind a catalog would consider at all.
///
/// Three of the five filters of `research/labels/tttext-codec.md` §7: enough letters
/// to be sure of, no unresolved letter, and every word of length >= 3 a word
/// the vocabulary knows. The other two — the ambiguity margin and the framing
/// rule — are not reimplemented here; this is a sampling filter, not the gate.
fn shippable(plain: &str, known: &HashSet<String>) -> bool {
    if plain.contains('?') {
        return false;
    }
    if plain.chars().filter(|c| c.is_ascii_alphabetic()).count() < 12 {
        return false;
    }
    plain
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| w.len() >= 3)
        .all(|w| known.contains(&w.to_ascii_lowercase()))
}

/// Read a cluster member's key off the leader's plaintext words.
///
/// The members share only their letter runs — the trailing numeric fields
/// differ — so the alignment is token by token. A member whose tokens do not
/// line up injectively is not the same text and gets no key at all, which is
/// the check that makes the transfer safe rather than merely free.
fn transfer_tokens(cipher: &str, words: &[&str]) -> Option<tttext::Key> {
    let tokens = tttext::tokens(cipher);
    if tokens.len() != words.len() {
        return None;
    }
    let mut key = tttext::Key::default();
    for (token, word) in tokens.iter().zip(words) {
        if token.len() != word.len() {
            return None;
        }
        for (c, p) in token.bytes().zip(word.bytes()) {
            let (c, p) = (c.to_ascii_lowercase(), p.to_ascii_lowercase());
            if !c.is_ascii_lowercase() || !p.is_ascii_lowercase() {
                return None;
            }
            if !key.insert(c - b'a', p - b'a') {
                return None;
            }
        }
    }
    Some(key)
}

/// Split the section into records. The id is plaintext; everything after the
/// first comma is the enciphered payload.
fn parse(bytes: &[u8]) -> Vec<Record> {
    // Latin-1, not UTF-8. The section carries 19 distinct high bytes — the
    // umlauts, `°`, `µ` — and they are *pass-through*: outside every enciphered
    // class, so they are plaintext evidence. `from_utf8_lossy` would fold all
    // of them onto one replacement character, which merges records that differ
    // only in which umlaut they use and silently deletes the `°C` crib.
    let text: String = bytes.iter().map(|b| char::from(*b)).collect();
    text.lines()
        .filter_map(|line| {
            let (id, cipher) = line.split_once(',')?;
            Some(Record { id: id.trim().parse().ok()?, cipher: cipher.to_string() })
        })
        .collect()
}

/// Words from a `{"id": "name"}` catalog.
fn words_of_json(path: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else { return Vec::new() };
    let Some(map) = value.as_object() else { return Vec::new() };
    let mut out = Vec::new();
    for name in map.values().filter_map(|v| v.as_str()) {
        for word in name.split(|c: char| !c.is_ascii_alphabetic()) {
            if word.len() >= 2 {
                out.push(word.to_ascii_lowercase());
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_keeps_its_high_bytes() {
        // `°` is 0xB0 and is pass-through, so it survives the cipher and is a
        // crib. Reading the section as UTF-8 would destroy it.
        let records = parse(b"000123,abc \xb0C\n");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id, 123);
        assert!(records[0].cipher.contains('\u{b0}'), "{:?}", records[0].cipher);
    }

    #[test]
    fn a_line_without_an_id_is_dropped_rather_than_guessed() {
        assert!(parse(b"not a record\n").is_empty());
    }

    #[test]
    fn the_trailing_field_run_is_cut_only_where_it_is_really_a_separator() {
        // `<name><sep><digit><sep><number>`: the separator recurs, so the run
        // is the record's tail and the name is what precedes it.
        assert_eq!(framed("Intake air temperature-4-23513").as_deref(), Some("Intake air temperature"));
        assert_eq!(framed("Engine speed.5.245,1").as_deref(), Some("Engine speed"));
        // A name that genuinely ends in a digit is the failure this guards:
        // the digit is inside the run, and cutting it would ship
        // `… of cylinder` six times over. One non-recurring character is not a
        // separator, so the record is dropped instead.
        assert_eq!(framed("Pressure of cylinder 4"), None);
        assert_eq!(framed("Nothing after the letters"), None);
    }

    #[test]
    fn a_word_pinned_by_nothing_but_the_dictionary_costs_the_record() {
        // The documented case: `Hill` and `Fill` differ in one letter that the
        // record uses once, so the rest of the record has no opinion and the
        // reading is a coin toss. Both are ordinary words, so the vocabulary
        // check passes it and only the margin catches it.
        let mut dict = Dictionary::default();
        for word in ["hill", "fill", "bytes", "backward"] {
            dict.insert(word, 100.0);
        }
        dict.finish();
        // A cipher whose first token maps to `hill` under a key that the other
        // tokens fix; `q`→`h`/`f` is the letter nothing else pins.
        assert!(!unambiguous("qill bytes", "hill bytes", &dict, MARGIN));
        // The same record with one reading far ahead of the other survives.
        let mut skewed = Dictionary::default();
        skewed.insert("hill", 1000.0);
        skewed.insert("fill", 1.0);
        skewed.insert("bytes", 100.0);
        skewed.finish();
        assert!(unambiguous("qill bytes", "hill bytes", &skewed, MARGIN));
    }

    #[test]
    fn the_gate_keeps_a_clean_reading_and_drops_the_four_kinds_of_unclean_one() {
        let mut dict = Dictionary::default();
        let mut known = HashSet::new();
        for word in ["intake", "air", "temperature", "bank"] {
            dict.insert(word, 100.0);
            known.insert(word.to_string());
        }
        dict.finish();
        let keep = |cipher: &str, plain: &str| gated_name(cipher, plain, &dict, &known);

        // Nothing in this reading is guessed: every letter resolved, every word
        // known, one candidate per pattern, and a real trailing field run.
        assert_eq!(
            keep("intake air temperature-4-2", "intake air temperature-4-2").as_deref(),
            Some("intake air temperature")
        );
        // An unresolved letter.
        assert_eq!(keep("intake air temper?ture-4-2", "intake air temper?ture-4-2"), None);
        // A word the vocabulary does not have.
        assert_eq!(keep("intake air tempxrature-4-2", "intake air tempxrature-4-2"), None);
        // Too few letters to be sure of.
        assert_eq!(keep("air bank-4-2", "air bank-4-2"), None);
        // No trailing run at all, so the framing cannot be shown to be clean.
        assert_eq!(keep("intake air temperature", "intake air temperature"), None);
    }

    #[test]
    fn a_directory_of_word_files_is_read_whole() {
        // `vagcan setup` hands the VCDS `Labels/` tree straight in, and the
        // corpus's own language is the strongest prior the attack has.
        let dir = std::env::temp_dir().join(format!("vagcan-words-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("a.lbl"), "one").unwrap();
        std::fs::write(dir.join("nested/b.lbl"), "two").unwrap();
        let files = word_files(&dir);
        assert_eq!(files.len(), 2, "{files:?}");
        assert_eq!(word_files(&dir.join("a.lbl")).len(), 1);
        assert!(word_files(&dir.join("absent")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
