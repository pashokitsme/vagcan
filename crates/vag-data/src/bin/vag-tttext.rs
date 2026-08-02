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
    let mut passes = 4usize;
    let mut limits = Limits::default();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--words" => word_files.extend(args.next()),
            "--names" => names_file = args.next(),
            "--out" => out = args.next(),
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

    // Records sharing a whole-payload pattern hold the same text under
    // different keys, so one solve serves the cluster and every other member
    // is had by transfer.
    let mut clusters: BTreeMap<Vec<u8>, Vec<usize>> = BTreeMap::new();
    for (index, record) in records.iter().enumerate() {
        if tttext::tokens(&record.cipher).len() >= 2 {
            clusters.entry(tttext::pattern(&record.cipher)).or_default().push(index);
        }
    }
    eprintln!("{} clusters with something to solve", clusters.len());

    let mut solved: HashMap<usize, String> = HashMap::new();
    for pass in 1..=passes {
        let mut learned = 0usize;
        let mut fresh = 0usize;
        let mut partial = 0usize;

        for members in clusters.values() {
            // A cluster already read needs no second look.
            if members.iter().any(|m| solved.contains_key(m)) {
                continue;
            }
            let leader = members[0];
            let cipher = &records[leader].cipher;
            let Some(solution) = tttext::solve(cipher, &dict, limits) else { continue };
            if !solution.key.covers(cipher) {
                partial += 1;
                continue;
            }
            let plain = solution.key.decode(cipher);
            fresh += 1;
            solved.insert(leader, plain.clone());

            // Every other member is free — and checked: a member whose key
            // does not line up is not the same text and is dropped.
            for member in &members[1..] {
                if tttext::transfer(&records[*member].cipher, &plain).is_some() {
                    solved.insert(*member, plain.clone());
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

/// Split the section into records. The id is plaintext; everything after the
/// first comma is the enciphered payload.
fn parse(bytes: &[u8]) -> Vec<Record> {
    String::from_utf8_lossy(bytes)
        .lines()
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
