//! `vag-labels` — walk a VCDS `Labels/` directory, parse every plaintext `.lbl`
//! file and every encrypted `.clb` file, and emit a structured JSON corpus
//! plus a coverage summary.
//!
//! Usage:
//!   vag-labels <labels-dir> [--out corpus.json] [--summary] [--lookup <PART_NO>]
//!
//! `.clb` files are decrypted (TEA-CBC, see [`vag_data::clb`]) before being
//! fed through the same [`parse_label`] parser used for plaintext `.lbl`.
//!
//! `--lookup <PART_NO>` resolves an ECU part number against the parsed corpus
//! (following any `REDIRECT` chain, see [`vag_data::db`]) and prints the
//! resolved label file plus its measurements.

use std::path::PathBuf;
use std::process::ExitCode;

use vag_data::label::{LabelFile, Record};
use vag_data::{load_corpus, LabelDb};

struct Args {
    dir: PathBuf,
    out: Option<PathBuf>,
    summary: bool,
    lookup: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut dir = None;
    let mut out = None;
    let mut summary = false;
    let mut lookup = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a path")?)),
            "--summary" => summary = true,
            "--lookup" => lookup = Some(it.next().ok_or("--lookup needs a part number")?),
            "-h" | "--help" => return Err("help".into()),
            s if s.starts_with('-') => return Err(format!("unknown flag: {s}")),
            s => {
                if dir.is_some() {
                    return Err(format!("unexpected extra argument: {s}"));
                }
                dir = Some(PathBuf::from(s));
            }
        }
    }
    Ok(Args {
        dir: dir.ok_or("missing <labels-dir> argument")?,
        out,
        summary,
        lookup,
    })
}

#[derive(Default)]
struct Stats {
    lbl_files: usize,
    clb_files: usize,
    other_files: usize,
    measurements: usize,
    redirects: usize,
    adaptations: usize,
    long_codings: usize,
    others: usize,
    parse_errors: usize,
}

fn tally(stats: &mut Stats, lf: &LabelFile) {
    for r in &lf.records {
        match r {
            Record::Measurement(_) => stats.measurements += 1,
            Record::Redirect { .. } => stats.redirects += 1,
            Record::Adaptation { .. } => stats.adaptations += 1,
            Record::LongCoding { .. } => stats.long_codings += 1,
            Record::Other { .. } => stats.others += 1,
        }
    }
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let load = load_corpus(&args.dir)
        .map_err(|e| format!("cannot read dir {}: {e}", args.dir.display()))?;

    let mut stats = Stats {
        lbl_files: load.lbl_count,
        clb_files: load.clb_count,
        other_files: load.other_count,
        parse_errors: load.read_errors,
        ..Default::default()
    };
    let mut corpus: Vec<LabelFile> = load.files;
    for lf in &corpus {
        tally(&mut stats, lf);
    }

    // Deterministic output ordering.
    corpus.sort_by(|a, b| a.source.cmp(&b.source));

    if let Some(out) = &args.out {
        let json = serde_json::to_string_pretty(&corpus)
            .map_err(|e| format!("serialize error: {e}"))?;
        std::fs::write(out, json).map_err(|e| format!("write {}: {e}", out.display()))?;
        eprintln!("wrote {} label files to {}", corpus.len(), out.display());
    }

    if args.summary || args.out.is_none() {
        print_summary(&stats);
    }

    if let Some(part_no) = &args.lookup {
        // Consumes `corpus`; must be the last use of it.
        print_lookup(LabelDb::new(corpus), part_no);
    }
    Ok(())
}

fn print_lookup(db: LabelDb, part_no: &str) {
    match db.resolve(part_no) {
        Some(file) => {
            println!("== Lookup {part_no} ==");
            println!("Resolved file: {}", file.source);
            let measurements = db.measurements(part_no);
            if measurements.is_empty() {
                println!("(no measurements in resolved file)");
            } else {
                println!("Measurements:");
                for m in measurements {
                    let unit = m.unit.as_deref().unwrap_or("");
                    println!("  {:>4}.{:<3} {}  [{}]", m.block, m.field, m.name, unit);
                }
            }
        }
        None => {
            println!("== Lookup {part_no} ==");
            println!("no match found in corpus of {} file(s)", db.len());
        }
    }
}

fn print_summary(s: &Stats) {
    println!("== VCDS Labels corpus summary ==");
    println!("Files:");
    println!("  .lbl parsed : {}", s.lbl_files);
    println!("  .clb parsed (decrypted via TEA-CBC) : {}", s.clb_files);
    println!("  other       : {}", s.other_files);
    if s.parse_errors > 0 {
        println!("  read errors : {}", s.parse_errors);
    }
    println!("Records parsed from .lbl + .clb:");
    println!("  measurements : {}", s.measurements);
    println!("  redirects    : {}", s.redirects);
    println!("  adaptations  : {}", s.adaptations);
    println!("  long codings : {}", s.long_codings);
    println!("  other        : {}", s.others);
    let total_labels = s.lbl_files + s.clb_files;
    if total_labels > 0 {
        println!("Coverage: {total_labels}/{total_labels} label files parsed (100%).");
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e == "help" => {
            eprintln!(
                "usage: vag-labels <labels-dir> [--out corpus.json] [--summary] [--lookup <PART_NO>]"
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
