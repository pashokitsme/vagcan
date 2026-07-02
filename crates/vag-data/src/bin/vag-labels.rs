//! `vag-labels` — walk a VCDS `Labels/` directory, parse every plaintext `.lbl`
//! file, and emit a structured JSON corpus plus a coverage summary.
//!
//! Usage:
//!   vag-labels <labels-dir> [--out corpus.json] [--summary]
//!
//! Only `.lbl` (plaintext) files are parsed. `.clb` files are counted and
//! reported as "not yet decoded" so coverage gaps are explicit.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vag_data::label::{parse_label, LabelFile, Record};

struct Args {
    dir: PathBuf,
    out: Option<PathBuf>,
    summary: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut dir = None;
    let mut out = None;
    let mut summary = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().ok_or("--out needs a path")?)),
            "--summary" => summary = true,
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
    })
}

fn has_ext(path: &Path, ext: &str) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case(ext))
        .unwrap_or(false)
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
    let entries = std::fs::read_dir(&args.dir)
        .map_err(|e| format!("cannot read dir {}: {e}", args.dir.display()))?;

    let mut stats = Stats::default();
    let mut corpus: Vec<LabelFile> = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| format!("dir entry error: {e}"))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if has_ext(&path, "lbl") {
            stats.lbl_files += 1;
            match std::fs::read(&path) {
                Ok(bytes) => {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("<?>")
                        .to_string();
                    let lf = parse_label(name, &bytes);
                    tally(&mut stats, &lf);
                    corpus.push(lf);
                }
                Err(e) => {
                    eprintln!("warn: cannot read {}: {e}", path.display());
                    stats.parse_errors += 1;
                }
            }
        } else if has_ext(&path, "clb") {
            stats.clb_files += 1;
        } else {
            stats.other_files += 1;
        }
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
    Ok(())
}

fn print_summary(s: &Stats) {
    println!("== VCDS Labels corpus summary ==");
    println!("Files:");
    println!("  .lbl parsed : {}", s.lbl_files);
    println!("  .clb (NOT decoded yet — fixed-XOR compiled format) : {}", s.clb_files);
    println!("  other       : {}", s.other_files);
    if s.parse_errors > 0 {
        println!("  read errors : {}", s.parse_errors);
    }
    println!("Records parsed from .lbl:");
    println!("  measurements : {}", s.measurements);
    println!("  redirects    : {}", s.redirects);
    println!("  adaptations  : {}", s.adaptations);
    println!("  long codings : {}", s.long_codings);
    println!("  other        : {}", s.others);
    let total_labels = s.lbl_files + s.clb_files;
    if let Some(pct) = (100 * s.lbl_files).checked_div(total_labels) {
        println!(
            "Coverage: {}/{} label files are plaintext ({pct}%); {} need .clb decode.",
            s.lbl_files, total_labels, s.clb_files
        );
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e == "help" => {
            eprintln!("usage: vag-labels <labels-dir> [--out corpus.json] [--summary]");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
