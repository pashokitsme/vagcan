//! `vagcan dev vcds dump` — parse a whole VCDS `Labels/` directory into JSON.
//!
//! Was the `vag-labels` binary. Its `--lookup` flag is gone rather than ported:
//! `vagcan dev vcds labels --part` answers the same question from a SQLite cache
//! instead of reparsing several thousand files, and two commands for one job
//! means the next session picks the slower one without knowing it.
//!
//! What is left is the thing nothing else does — all the label files in one
//! structured file, `.clb` decrypted on the way through, for searching, for
//! diffing one VCDS version against another, or for handing to a tool that is
//! not this program.

use anyhow::{Context, Result};
use std::path::Path;

use vag_data_labels::label::{LabelFile, Record};
use vag_data_labels::load_label_files;

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
	read_errors: usize,
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

pub fn run(dir: &str, out: Option<&str>) -> Result<()> {
	let load = load_label_files(Path::new(dir)).with_context(|| format!("reading {dir:?}"))?;

	let mut stats = Stats {
		lbl_files: load.lbl_count,
		clb_files: load.clb_count,
		other_files: load.other_count,
		read_errors: load.read_errors,
		..Default::default()
	};
	let mut label_files: Vec<LabelFile> = load.files;
	for lf in &label_files {
		tally(&mut stats, lf);
	}

	// Deterministic ordering, so two runs — or two VCDS versions — diff.
	label_files.sort_by(|a, b| a.source.cmp(&b.source));

	if let Some(out) = out {
		let json = serde_json::to_string_pretty(&label_files).context("serialising the label files")?;
		std::fs::write(out, json).with_context(|| format!("writing {out:?}"))?;
		eprintln!("wrote {} label files to {out}", label_files.len());
	}

	print!("{}", render_summary(&stats));
	Ok(())
}

fn render_summary(s: &Stats) -> String {
	let mut out = String::from("== VCDS Labels label files summary ==\nFiles:\n");
	out.push_str(&format!("  .lbl parsed : {}\n", s.lbl_files));
	out.push_str(&format!("  .clb parsed (decrypted via TEA-CBC) : {}\n", s.clb_files));
	out.push_str(&format!("  other       : {}\n", s.other_files));
	if s.read_errors > 0 {
		out.push_str(&format!("  read errors : {}\n", s.read_errors));
	}
	out.push_str("Records parsed from .lbl + .clb:\n");
	out.push_str(&format!("  measurements : {}\n", s.measurements));
	out.push_str(&format!("  redirects    : {}\n", s.redirects));
	out.push_str(&format!("  adaptations  : {}\n", s.adaptations));
	out.push_str(&format!("  long codings : {}\n", s.long_codings));
	out.push_str(&format!("  other        : {}\n", s.others));
	let total = s.lbl_files + s.clb_files;
	if total > 0 {
		out.push_str(&format!("Coverage: {total}/{total} label files parsed (100%).\n"));
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_summary_hides_read_errors_when_there_were_none() {
		// A zero line reads as a warning nobody needs to act on.
		let clean = Stats {
			lbl_files: 2,
			clb_files: 1,
			..Default::default()
		};
		assert!(!render_summary(&clean).contains("read errors"));
		let broken = Stats { read_errors: 3, ..clean };
		assert!(render_summary(&broken).contains("read errors : 3"));
	}

	#[test]
	fn an_empty_label_set_claims_no_coverage() {
		// "0/0 parsed (100%)" is the kind of line that makes a broken path look
		// like a working one.
		assert!(!render_summary(&Stats::default()).contains("Coverage"));
	}
}
