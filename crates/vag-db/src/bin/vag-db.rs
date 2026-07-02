//! `vag-db` — build and query a SQLite cache of the parsed VCDS label corpus.
//!
//! Usage:
//!   vag-db build  <labels-dir> <out.sqlite>
//!   vag-db lookup <db.sqlite>  <part-no>
//!   vag-db stats  <db.sqlite>
//!
//! `build` walks `<labels-dir>` (parsing every `.lbl` and decrypting+parsing
//! every `.clb`, see [`vag_data::load_corpus`]) and writes the parsed corpus
//! to a SQLite file, overwriting any existing tables of the same name.
//! `lookup` loads that cache into a [`vag_data::LabelDb`] and resolves a part
//! number exactly like `vag-labels --lookup`. `stats` prints row counts per
//! table.

use std::path::PathBuf;
use std::process::ExitCode;

use vag_db::build_db;

const USAGE: &str = "usage:\n  \
    vag-db build  <labels-dir> <out.sqlite>\n  \
    vag-db lookup <db.sqlite>  <part-no>\n  \
    vag-db stats  <db.sqlite>";

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().ok_or_else(|| USAGE.to_string())?;

    match cmd.as_str() {
        "build" => {
            let labels_dir =
                PathBuf::from(args.next().ok_or("build needs <labels-dir> <out.sqlite>")?);
            let out = PathBuf::from(args.next().ok_or("build needs <out.sqlite>")?);
            let stats =
                build_db(&labels_dir, &out).map_err(|e| format!("build failed: {e}"))?;
            println!("wrote {} label file(s) to {}", stats.files, out.display());
            println!("  measurements : {}", stats.measurements);
            println!("  redirects    : {}", stats.redirects);
            println!("  adaptations  : {}", stats.adaptations);
            println!("  long codings : {}", stats.long_codings);
            Ok(())
        }
        "lookup" => {
            let db_path = PathBuf::from(args.next().ok_or("lookup needs <db.sqlite> <part-no>")?);
            let part_no = args.next().ok_or("lookup needs <part-no>")?;
            let db = vag_db::load_db(&db_path).map_err(|e| format!("load failed: {e}"))?;
            print_lookup(&db, &part_no);
            Ok(())
        }
        "stats" => {
            let db_path = PathBuf::from(args.next().ok_or("stats needs <db.sqlite>")?);
            print_stats(&db_path)
        }
        "-h" | "--help" => {
            eprintln!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown subcommand: {other}\n{USAGE}")),
    }
}

/// Mirrors `vag-labels --lookup`'s output: resolved file name + measurements.
fn print_lookup(db: &vag_data::LabelDb, part_no: &str) {
    match db.resolve(part_no) {
        Some(file) => {
            println!("== Lookup {part_no} ==");
            println!("Resolved file: {}", file.source);
            let measurements = db.measurements(part_no);
            let total = measurements.len();
            let named: Vec<_> = measurements
                .into_iter()
                .filter(|m| !m.name.trim().is_empty())
                .collect();
            if named.is_empty() {
                if total == 0 {
                    println!("(no measurements in resolved file)");
                } else {
                    println!("({total} measurements, all empty placeholders)");
                }
            } else {
                println!("Measurements:");
                for m in named {
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

fn print_stats(db_path: &std::path::Path) -> Result<(), String> {
    let conn = rusqlite::Connection::open(db_path)
        .map_err(|e| format!("open {}: {e}", db_path.display()))?;
    let count = |table: &str| -> Result<i64, String> {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row.get(0))
            .map_err(|e| format!("count {table}: {e}"))
    };
    println!("== vag-db stats: {} ==", db_path.display());
    println!("  label_file  : {}", count("label_file")?);
    println!("  measurement : {}", count("measurement")?);
    println!("  redirect    : {}", count("redirect")?);
    println!("  adaptation  : {}", count("adaptation")?);
    println!("  long_coding : {}", count("long_coding")?);
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) if e == USAGE => {
            eprintln!("{USAGE}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
