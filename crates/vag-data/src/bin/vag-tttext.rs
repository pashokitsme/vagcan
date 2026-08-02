//! Recover names from the corpus's global text table.
//!
//! ```text
//! vag-tttext <TXT.bin> [--words FILE]... [--names catalogs/names-uds.json]
//!                      [--out FILE] [--passes N] [--steps N]
//! ```
//!
//! `TXT.bin` is the decrypted, inflated `[TXT]` section of `TTTEXT.ROD` —
//! `NNNNNN,<enciphered payload>` per line. Each payload is under its own
//! substitution (`vag_data::tttext`), so the attack is dictionary-driven and
//! bootstraps: words read off records it solves become vocabulary for the next
//! pass, and passes run until nothing new is learned.
//!
//! Output is `id\tplaintext` for every record solved with **no unresolved
//! letter**. Partial readings are counted and dropped: a name with a guessed
//! letter reads exactly like a name without one.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;

use vag_data::tttext::{self, Dictionary, Limits, Record};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut input = None;
    let mut word_files: Vec<String> = Vec::new();
    let mut names_file = None;
    let mut out = None;
    let mut partial_out = None;
    let mut check = 0usize;
    let mut gated = false;
    let mut passes = 4usize;
    let mut limits = Limits::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--words" => word_files.extend(args.next()),
            "--names" => names_file = args.next(),
            "--out" => out = args.next(),
            "--partial" => partial_out = args.next(),
            "--gated" => gated = true,
            "--check" => check = args.next().and_then(|v| v.parse().ok()).unwrap_or(check),
            "--passes" => passes = args.next().and_then(|v| v.parse().ok()).unwrap_or(passes),
            "--steps" => {
                limits.steps = args.next().and_then(|v| v.parse().ok()).unwrap_or(limits.steps)
            }
            _ => input = Some(arg),
        }
    }
    let Some(input) = input else {
        eprintln!(
            "usage: vag-tttext <TXT.bin> [--words FILE]... [--names FILE] [--out FILE] \
             [--passes N] [--steps N]"
        );
        std::process::exit(2);
    };

    let text = std::fs::read(&input).unwrap_or_else(|e| {
        eprintln!("reading {input}: {e}");
        std::process::exit(1);
    });
    let records = parse(&text);
    eprintln!("{} records", records.len());

    // The vocabulary. In-domain words outweigh a general list heavily: the
    // corpus's own language is the strongest prior available, and a general
    // dictionary is there to catch the rest.
    let mut dict = Dictionary::default();
    let mut known: HashSet<String> = HashSet::new();
    if let Some(path) = &names_file {
        for word in words_of_json(path) {
            known.insert(word.clone());
            dict.insert(&word, 400.0);
        }
        eprintln!("{} words from names already recovered", known.len());
    }
    for spec in &word_files {
        // `FILE` or `FILE:WEIGHT`. The weight is the prior: the corpus's own
        // label files are in-domain and must outrank a general word list, or
        // the search prefers an English rarity to the term the corpus uses.
        let (path, weight) = match spec.rsplit_once(':').and_then(|(p, w)| Some((p, w.parse().ok()?)))
        {
            Some((path, weight)) => (path, weight),
            None => (spec.as_str(), 8.0f32),
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            eprintln!("skipping {path}: unreadable");
            continue;
        };
        let before = known.len();
        for word in text.split(|c: char| !c.is_ascii_alphabetic()) {
            if word.len() >= 2 && known.insert(word.to_ascii_lowercase()) {
                dict.insert(word, weight);
            }
        }
        eprintln!("{} words from {path} at weight {weight}", known.len() - before);
    }
    dict.finish();
    if dict.is_empty() {
        eprintln!("no vocabulary: pass --names and/or --words");
        std::process::exit(1);
    }

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
    for pass in 1..=passes {
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

    // Validation, and it is not optional. Within a cluster the member's
    // cipher pattern equals the leader's *plaintext* pattern by construction,
    // so `transfer_tokens` can never fail injectivity — it is free, but it is
    // not a check. The check is to solve a member from scratch, under its own
    // key and its own search path, and see whether it says the same thing.
    // Two plaintexts can share a repetition pattern; this is what would catch
    // it.
    if check > 0 {
        let mut members: Vec<(usize, &String)> = Vec::new();
        for group in clusters.values() {
            for member in &group[1..] {
                match solved.get(member) {
                    // Only readings a catalog would consider. Sampling every
                    // transferred member measures the transfer over acronym
                    // soup that no gate would ship, which is not the number
                    // that matters and is not what the published 599/600 was
                    // measured on.
                    Some(plain) if !gated || shippable(plain, &known) => {
                        members.push((*member, plain))
                    }
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
            let Some(solution) = tttext::solve(cipher, &dict, limits) else {
                unusable += 1;
                continue;
            };
            let key = tttext::complete(cipher, &solution.key, &dict, MARGIN);
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
        eprintln!("independent re-solve of transferred members: agree {agree} disagree {disagree} unusable {unusable}");
        for (id, transferred, fresh) in &examples {
            eprintln!("  {id:06} transferred {transferred:?} != re-solved {fresh:?}");
        }
    }

    if let Some(path) = &partial_out {
        let mut sink = std::io::BufWriter::new(
            std::fs::File::create(path).expect("creating the partial output file"),
        );
        for (index, plain) in &partials {
            let _ = writeln!(sink, "{:06}\t{plain}", records[*index].id);
        }
        eprintln!("{} partial readings written to {path}", partials.len());
    }

    let mut sink: Box<dyn Write> = match &out {
        Some(path) => Box::new(std::io::BufWriter::new(
            std::fs::File::create(path).expect("creating the output file"),
        )),
        None => Box::new(std::io::stdout().lock()),
    };
    let mut ordered: Vec<(u32, &String)> =
        solved.iter().map(|(index, plain)| (records[*index].id, plain)).collect();
    ordered.sort_unstable();
    for (id, plain) in &ordered {
        let _ = writeln!(sink, "{id:06}\t{plain}");
    }
    eprintln!(
        "{} of {} records read ({:.1} %)",
        ordered.len(),
        records.len(),
        100.0 * ordered.len() as f32 / records.len() as f32
    );
}

/// Whether a reading is the kind a catalog would consider at all.
///
/// Three of the five filters of `research/tttext-codec.md` §7: enough letters
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

/// How far the best reading of a token must outweigh the runner-up before the
/// completion step will pin its letters. A likelihood ratio, not a proof: at
/// 20x a token with a real second reading is left unpinned and the record is
/// dropped, which is the direction to err in.
const MARGIN: f32 = 20.0;

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
