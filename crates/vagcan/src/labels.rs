//! `vagcan labels` — inventory + lookup over a VCDS label/ODX directory tree.
//!
//! Points at a VCDS install root (or any subtree), recursively counts the
//! `.lbl` / `.clb` / `.rod` files it holds, and answers two lookups:
//!
//! - `--part <PART_NO>` — resolve an ECU part number through the `REDIRECT`
//!   chain to its terminal label file and list that file's measurements
//!   (the same resolution `vagcan info`'s label path uses).
//! - `--block <N> [--field <F>]` — a cross-corpus scan: every label file that
//!   defines measuring block `N` (optionally narrowed to field `F`), with the
//!   measurement's name and unit.
//!
//! No brand/model dimension: VCDS label files are keyed by part number, not by
//! vehicle make/model, so there is nothing to group by. Rendering is factored
//! into pure `render_*` helpers so the formatting is unit-tested without a disk.

use std::path::Path;

use anyhow::Context;
use vag_data::{scan_corpus, CorpusScan, LabelDb, Measurement};

/// Entry point for the `labels` subcommand. Loads the corpus once, prints the
/// summary, then runs whichever lookup(s) were requested.
pub fn labels_cmd(
    dir: &str,
    part: Option<&str>,
    block: Option<u16>,
    field: Option<u8>,
) -> anyhow::Result<()> {
    let scan = scan_corpus(Path::new(dir))
        .with_context(|| format!("scanning label corpus under {dir:?}"))?;

    print!("{}", render_summary(&scan));

    // The block/part lookups need a LabelDb, which consumes the parsed files.
    if part.is_some() || block.is_some() {
        let db = LabelDb::new(scan.files);
        if let Some(part_no) = part {
            print!("\n{}", render_part_lookup(&db, part_no));
        }
        if let Some(b) = block {
            print!("\n{}", render_block_lookup(&db, b, field));
        }
    }
    Ok(())
}

/// Render the file-count summary (`.lbl` / `.clb` / `.rod` + parsed records).
fn render_summary(scan: &CorpusScan) -> String {
    let mut out = String::new();
    let total_files = scan.lbl_count + scan.clb_count + scan.rod_count;
    out.push_str("== VCDS label corpus ==\n");
    out.push_str(&format!("  .lbl (plaintext)          : {}\n", scan.lbl_count));
    out.push_str(&format!("  .clb (TEA-CBC decrypted)  : {}\n", scan.clb_count));
    out.push_str(&format!("  .rod (ODX, counted only)  : {}\n", scan.rod_count));
    out.push_str(&format!("  total label/ODX files     : {total_files}\n"));
    if scan.read_errors > 0 {
        out.push_str(&format!("  read errors (skipped)     : {}\n", scan.read_errors));
    }
    out.push_str(&format!(
        "Parsed {} .lbl/.clb file(s) into measurement definition sets.\n",
        scan.files.len()
    ));
    out
}

/// Render `--part <PART_NO>`: resolved file + its measurements.
fn render_part_lookup(db: &LabelDb, part_no: &str) -> String {
    let mut out = format!("== Lookup part {part_no} ==\n");
    match db.resolve(part_no) {
        Some(file) => {
            out.push_str(&format!("Resolved file: {}\n", file.source));
            let ms = db.measurements(part_no);
            if ms.is_empty() {
                out.push_str("(no measurements in resolved file)\n");
            } else {
                out.push_str(&format!("Measurements ({}):\n", ms.len()));
                for m in ms {
                    out.push_str(&render_measurement_row(m));
                }
            }
        }
        None => out.push_str(&format!(
            "no match in corpus of {} parsed file(s)\n",
            db.len()
        )),
    }
    out
}

/// Render `--block <N> [--field <F>]`: every file defining that block.
fn render_block_lookup(db: &LabelDb, block: u16, field: Option<u8>) -> String {
    let scope = match field {
        Some(f) => format!("block {block} field {f}"),
        None => format!("block {block}"),
    };
    let mut out = format!("== Lookup {scope} (whole corpus) ==\n");
    let hits = db.measurements_by_block(block, field);
    if hits.is_empty() {
        out.push_str("no label file defines this block\n");
        return out;
    }
    out.push_str(&format!("{} definition(s):\n", hits.len()));
    for (source, m) in hits {
        out.push_str(&format!("  {:<24} {}", source, render_measurement_row(m)));
    }
    out
}

/// One measurement line: `NNN.F  Name  [unit]` (+ trailing newline).
fn render_measurement_row(m: &Measurement) -> String {
    let unit = m.unit.as_deref().unwrap_or("");
    format!("{:>4}.{:<3} {}  [{}]\n", m.block, m.field, m.name, unit)
}

/// Resolve the label file a control unit names for itself.
///
/// The unit's `F19E` identifier holds its ODX file name (e.g.
/// `EV_ECM18TFS0208V0906264H`), so selecting the right description file stops
/// being a part-number guess: the car answers the question. Reports what the
/// file contains, marking sections whose payload did not decode rather than
/// implying the whole file was read.
pub fn resolve_odx(dir: &str, odx_name: &str) -> anyhow::Result<()> {
    use vag_data::rod::{decode_rod, RodStatus};

    let hits = vag_data::find_rod_by_odx_name(std::path::Path::new(dir), odx_name)?;
    if hits.is_empty() {
        println!(
            "No label file named {odx_name:?} under {dir}.\n\n\
             The control unit names this file itself, so the corpus is either incomplete or \
             pointed at the wrong directory — pass the VCDS install root."
        );
        return Ok(());
    }

    for path in &hits {
        println!("{}", path.display());
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                println!("  cannot read: {e}");
                continue;
            }
        };
        let sections = decode_rod(&bytes);
        if sections.is_empty() {
            println!("  no sections found");
            continue;
        }
        for section in &sections {
            let state = match section.status {
                RodStatus::Tea => "decrypted",
                RodStatus::Zlib => "decrypted + inflated",
                RodStatus::Undecodable => "NOT decoded",
            };
            let size = section.text.as_ref().map(|t| t.len()).unwrap_or(0);
            println!("  [{}]  {state}, {size} bytes", section.tag);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vag_data::parse_label;

    fn scan_with(files: Vec<vag_data::LabelFile>, lbl: usize, clb: usize, rod: usize) -> CorpusScan {
        CorpusScan {
            files,
            lbl_count: lbl,
            clb_count: clb,
            rod_count: rod,
            other_count: 0,
            read_errors: 0,
        }
    }

    #[test]
    fn summary_reports_all_three_file_kinds_and_total() {
        let scan = scan_with(vec![], 1181, 1854, 16578);
        let s = render_summary(&scan);
        assert!(s.contains(".lbl (plaintext)          : 1181"));
        assert!(s.contains(".clb (TEA-CBC decrypted)  : 1854"));
        assert!(s.contains(".rod (ODX, counted only)  : 16578"));
        assert!(s.contains("total label/ODX files     : 19613"));
    }

    #[test]
    fn part_lookup_lists_measurements_of_resolved_file() {
        let file = parse_label(
            "022-906-032-C.LBL",
            b"001,1,Engine Speed,,Range: 0...6500 RPM",
        );
        let db = LabelDb::new(vec![file]);
        let s = render_part_lookup(&db, "022-906-032-C");
        assert!(s.contains("Resolved file: 022-906-032-C.LBL"));
        assert!(s.contains("Engine Speed"));
        assert!(s.contains("[RPM]"));
    }

    #[test]
    fn part_lookup_reports_miss() {
        let db = LabelDb::new(vec![parse_label("X.LBL", b"001,1,Irrelevant,,")]);
        let s = render_part_lookup(&db, "999-999-999-Z");
        assert!(s.contains("no match"));
    }

    #[test]
    fn block_lookup_spans_the_corpus_and_field_narrows() {
        let a = parse_label("AAA.LBL", b"002,1,Engine Speed,,Range: 0...6500 RPM");
        let b = parse_label("BBB.LBL", b"002,1,Vehicle Speed,,Range: 0...300 km/h\n002,2,Coolant,,");
        let db = LabelDb::new(vec![a, b]);

        let all = render_block_lookup(&db, 2, None);
        assert!(all.contains("3 definition(s):"));
        assert!(all.contains("AAA.LBL"));
        assert!(all.contains("BBB.LBL"));
        assert!(all.contains("Engine Speed"));
        assert!(all.contains("Coolant"));

        let f1 = render_block_lookup(&db, 2, Some(1));
        assert!(f1.contains("2 definition(s):"));
        assert!(f1.contains("field 1"));
        assert!(!f1.contains("Coolant"));

        let none = render_block_lookup(&db, 42, None);
        assert!(none.contains("no label file defines this block"));
    }
}
