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

use crate::render::plural;
use vag_cli_core::table::{self, Table};
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
		vag_cli_core::say!("wrote {} {} to {out}", label_files.len(), plural(label_files.len(), "label file"));
	}

	vag_cli_core::out!("{}", render_summary(&stats));
	vag_cli_core::note!("read {dir}: .clb decrypted (TEA-CBC) and parsed with the .lbl; other files skipped");
	Ok(())
}

/// What `dump` prints on stdout: the files by kind, the records by kind, one summary line.
///
/// `what  count`, the header every count table in `dev vcds` has (`labels`, `stats`);
/// the record kinds spelled as `stats` names the cache's tables (`long_coding`). The
/// two `other` rows are other files and other records: each table is its own.
fn render_summary(s: &Stats) -> String {
	let mut files = Table::new(&["what", "count"]).right(1);
	for (kind, n) in [(".lbl", s.lbl_files), (".clb", s.clb_files), ("other", s.other_files)] {
		files.row(vec![kind.to_string(), n.to_string()]);
	}
	let mut records = Table::new(&["what", "count"]).right(1);
	for (kind, n) in [
		("measurement", s.measurements),
		("redirect", s.redirects),
		("adaptation", s.adaptations),
		("long_coding", s.long_codings),
		("other", s.others),
	] {
		records.row(vec![kind.to_string(), n.to_string()]);
	}
	let width = table::terminal_width();
	let mut out = files.render(width);
	out.push('\n');
	out.push_str(&records.render(width));
	out.push('\n');
	let parsed = s.lbl_files + s.clb_files;
	out.push_str(&vag_cli_core::style::facts([
		format!("{parsed} {} parsed", plural(parsed, "label file")),
		match s.read_errors {
			0 => String::new(),
			n => format!("{n} unreadable"),
		},
	]));
	out.push('\n');
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_summary_is_two_tables_and_a_line() {
		let clean = Stats {
			lbl_files: 2,
			clb_files: 1,
			measurements: 40,
			..Default::default()
		};
		assert_eq!(
			render_summary(&clean),
			"what   count\n\
			 .lbl       2\n\
			 .clb       1\n\
			 other      0\n\
			 \n\
			 what         count\n\
			 measurement     40\n\
			 redirect         0\n\
			 adaptation       0\n\
			 long_coding      0\n\
			 other            0\n\
			 \n\
			 3 label files parsed\n"
		);
		let one = Stats {
			lbl_files: 1,
			..Default::default()
		};
		assert!(render_summary(&one).ends_with("\n\n1 label file parsed\n"));
	}

	#[test]
	fn the_summary_hides_read_errors_when_there_were_none() {
		// A zero count reads as a warning nobody needs to act on.
		let clean = Stats {
			lbl_files: 2,
			clb_files: 1,
			..Default::default()
		};
		assert!(!render_summary(&clean).contains("unreadable"));
		let broken = Stats { read_errors: 3, ..clean };
		assert!(render_summary(&broken).ends_with("3 label files parsed · 3 unreadable\n"));
	}

	#[test]
	fn an_empty_label_set_claims_no_coverage() {
		// "0/0 parsed (100%)" is the kind of line that makes a broken path look
		// like a working one.
		let text = render_summary(&Stats::default());
		assert!(text.ends_with("0 label files parsed\n"), "{text}");
		assert!(!text.contains('%'), "{text}");
	}
}
