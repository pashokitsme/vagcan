//! `vagcan vcds labels` — inventory + lookup over a VCDS label/ODX directory tree.
//!
//! Points at a VCDS install root (or any subtree), recursively counts the
//! `.lbl` / `.clb` / `.rod` files it holds, and answers two lookups:
//!
//! - `--part <PART_NO>` — resolve an ECU part number through the `REDIRECT`
//!   chain to its terminal label file and list that file's measurements
//!   (the same resolution `vagcan info`'s label path uses).
//! - `--block <N> [--field <F>]` — a cross-label-file scan: every label file that
//!   defines measuring block `N` (optionally narrowed to field `F`), with the
//!   measurement's name and unit.
//!
//! No brand/model dimension: VCDS label files are keyed by part number, not by
//! vehicle make/model, so there is nothing to group by. Rendering is factored
//! into pure `render_*` helpers so the formatting is unit-tested without a disk.

use std::path::{Path, PathBuf};

use anyhow::Context;
use vag_data::{LabelDb, LabelScan, Measurement, scan_label_files};

/// Where a parsed label file set is cached between runs.
///
/// Parsing every `.lbl` and decrypting every `.clb` in a VCDS install is the
/// slow part of every lookup, and the label files do not change between runs.
///
/// One file, `~/.vagcan/data/extracted/cache.sqlite`, beside the other two things
/// `vagcan setup` recovers from an installation. It used to be one file per
/// label file directory, named after the flattened path, which kept the English and
/// the Russian install apart at the cost of a directory nobody could read. The
/// same property is kept more cheaply inside the cache itself: it records which
/// directory it was built from, and a cache built from another one is rebuilt
/// rather than trusted.
fn cache_path() -> PathBuf {
	crate::datadir::label_cache()
		// With nowhere to write, the working directory is a poor cache but a
		// better failure than refusing to read label files at all.
		.unwrap_or_else(|_| PathBuf::from("cache.sqlite"))
}

/// The directory the label files are actually in.
///
/// `--labels` is documented as taking a VCDS install root *or* any directory
/// below it, and people point it at the root, because that is what they have.
/// The loader reads one directory level, so a root — where the labels sit in
/// `Labels/` and the ODX files in `UDS_EV/` — used to cache nothing at all and
/// say so as "cached 0 label files", which reads as an empty label files rather than
/// as a wrong path.
///
/// So the directory is located rather than assumed: the one given if it holds
/// label files, otherwise the first child that does. Two levels is enough for
/// every layout Ross-Tech ships and shallow enough not to wander into a home
/// directory.
fn label_dir_under(given: &Path) -> anyhow::Result<PathBuf> {
	fn holds_labels(dir: &Path) -> bool {
		std::fs::read_dir(dir).is_ok_and(|entries| {
			entries.flatten().any(|e| {
				matches!(
					e.path().extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).as_deref(),
					Some("lbl") | Some("clb")
				)
			})
		})
	}

	if holds_labels(given) {
		return Ok(given.to_path_buf());
	}
	let mut children: Vec<PathBuf> = std::fs::read_dir(given)
		.with_context(|| format!("reading {}", given.display()))?
		.flatten()
		.map(|e| e.path())
		.filter(|p| p.is_dir())
		.collect();
	children.sort();
	for child in &children {
		if holds_labels(child) {
			eprintln!("using {} — the label files are there, not in {}", child.display(), given.display());
			return Ok(child.clone());
		}
	}
	anyhow::bail!(
		"no label files under {} — expected a VCDS install root (with a Labels directory) \
         or the Labels directory itself",
		given.display()
	)
}

/// Whether a directory holds label files this tool could load.
///
/// The same shape [`load_cached`] needs — a `Labels/` of `.lbl`/`.clb`, or a
/// directory that already is one — asked cheaply so the `~/.vagcan` default can
/// degrade to "no label files" instead of an error on a machine that has not run
/// `vagcan setup`. Only used for that default: a directory the user named is
/// loaded outright, so a wrong path still reports itself.
pub fn has_label_files(dir: &Path) -> bool {
	label_dir_under(dir).is_ok()
}

/// Whether the cache on disk can be believed for this label file directory.
///
/// Two questions, and an mtime only answers the first. **Is it stale?** — the
/// file must be newer than the directory it was built from. **Is it even about
/// these label files?** — one cache file now serves every install, so a cache built
/// from the Russian tree is not stale for the English one, it is wrong, and its
/// mtime says nothing about that. Hence the source directory the cache carries.
fn cache_is_current(cache: &Path, dir: &Path) -> bool {
	// A cache that does not say what it holds cannot be vouched for — that is
	// every cache written before this was recorded, and it rebuilds once.
	if vag_db::source_of(cache).as_deref() != Some(dir.to_string_lossy().as_ref()) {
		return false;
	}
	match (std::fs::metadata(cache), std::fs::metadata(dir)) {
		(Ok(c), Ok(d)) => match (c.modified(), d.modified()) {
			(Ok(c), Ok(d)) => c >= d,
			_ => false,
		},
		_ => false,
	}
}

/// Load label files, using the SQLite cache when it is usable and building it when
/// it is not.
///
/// A stale cache is worse than none, so it is only trusted when
/// [`cache_is_current`] says so. `refresh` forces a rebuild regardless.
pub fn load_cached(dir: &Path, refresh: bool) -> anyhow::Result<LabelDb> {
	let dir = &label_dir_under(dir)?;
	let cache = cache_path();
	let fresh = !refresh && cache_is_current(&cache, dir);
	if fresh {
		match vag_db::load_db(&cache) {
			Ok(db) => return Ok(db),
			// A corrupt or half-written cache is a reason to rebuild, not to
			// fail the command.
			Err(e) => eprintln!("cache {} unusable ({e}) — rebuilding", cache.display()),
		}
	}

	if let Some(parent) = cache.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("creating the cache directory {}", parent.display()))?;
	}
	let stats = vag_db::build_db(dir, &cache).map_err(|e| anyhow::anyhow!("building the label cache from {}: {e}", dir.display()))?;
	eprintln!(
		"cached {} label files ({} measurements) in {}",
		stats.files,
		stats.measurements,
		cache.display()
	);
	vag_db::load_db(&cache).map_err(|e| anyhow::anyhow!("reading the label cache: {e}"))
}

/// Hand the label files' unit numbering to the address layer.
///
/// `vag-protocol` cannot read a label file — it is the protocol layer, and
/// label files are not a protocol — so the numbering is pushed in from here, where
/// both crates are already in scope. What crosses the seam is plain numbers and
/// strings.
///
/// Names only: no label file states which CAN id a number is answered on (see
/// `vag_protocol::address`), so this tier fills in *what* `44` is and leaves
/// *where to reach it* to the car, to the user's override file, or to the
/// built-in fallback.
pub fn install_unit_numbers(db: &LabelDb) {
	vag_protocol::address::install(db.unit_numbers().iter().map(|(number, name)| vag_protocol::address::UnitNumber {
		number: *number,
		request: None,
		name: Some(name.clone()),
	}));
}

/// Entry point for the `labels` subcommand. Loads the label files once, prints the
/// summary, then runs whichever lookup(s) were requested.
pub fn labels_cmd(dir: &str, part: Option<&str>, block: Option<u16>, field: Option<u8>, refresh: bool) -> anyhow::Result<()> {
	// The inventory is what someone asking *nothing* wants; with a lookup on
	// the command line it is six lines of noise before the answer.
	if part.is_none() && block.is_none() {
		let scan = scan_label_files(Path::new(dir)).with_context(|| format!("scanning label files under {dir:?}"))?;
		print!("{}", render_summary(&scan));
	}

	if part.is_some() || block.is_some() {
		let db = load_cached(Path::new(dir), refresh)?;
		install_unit_numbers(&db);
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
fn render_summary(scan: &LabelScan) -> String {
	let mut out = String::new();
	let total_files = scan.lbl_count + scan.clb_count + scan.rod_count;
	out.push_str("== VCDS label files ==\n");
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
			// Which unit this part number *is*, in the label files' own numbering —
			// the answer `vagcan units --identify` needs to name a unit `44`
			// rather than `712`.
			if let Some(unit) = db.unit_for_part(part_no) {
				out.push_str(&format!("Control unit: {:02X}  {}\n", unit.address, unit.name));
			}
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
		None => out.push_str(&format!("no match in {} parsed label file(s)\n", db.len())),
	}
	out
}

/// Render `--block <N> [--field <F>]`: every file defining that block.
fn render_block_lookup(db: &LabelDb, block: u16, field: Option<u8>) -> String {
	let scope = match field {
		Some(f) => format!("block {block} field {f}"),
		None => format!("block {block}"),
	};
	let mut out = format!("== Lookup {scope} (whole label_files) ==\n");
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
pub fn resolve_odx(dir: &str, odx_name: &str, cache_path: &Path) -> anyhow::Result<()> {
	use vag_data::rod::{IvCache, RodStatus, decode_rod_recover};

	let hits = vag_data::find_rod_by_odx_name(std::path::Path::new(dir), odx_name)?;
	if hits.is_empty() {
		println!(
			"No label file named {odx_name:?} under {dir}.\n\n\
             The control unit names this file itself, so the label files are either incomplete or \
             pointed at the wrong directory — pass the VCDS install root."
		);
		return Ok(());
	}

	// Recovered initialisation vectors come from the cache only. The search
	// that fills it costs minutes of every core and lives in a separate tool
	// (`cargo run -p vagcan -- vcds rod <file.rod>`),
	// so reading a car never links it.
	let mut cache = IvCache::load(cache_path);
	for path in &hits {
		println!("{}", path.display());
		let bytes = match std::fs::read(path) {
			Ok(b) => b,
			Err(e) => {
				println!("  cannot read: {e}");
				continue;
			}
		};
		let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
		let sections = decode_rod_recover(&bytes, name, &mut cache, false);
		if sections.is_empty() {
			println!("  no sections found");
			continue;
		}
		for section in &sections {
			let state = match section.status {
				RodStatus::Tea => "decrypted",
				RodStatus::Zlib => "decrypted + inflated",
				// No vector in the cache for this section, and this binary
				// cannot search for one.
				RodStatus::Undecodable => {
					"encrypted (recover with: cargo run -p vagcan \
                     -- vcds rod <file.rod>)"
				}
				// Pointing at the recovery command here would waste an hour of
				// every core: the search cannot start on this file at all.
				RodStatus::SearchDeclined => {
					"encrypted, and the key search has no crib on this file \
                     (see research/labels/tttext2.md) — not damaged, not yet openable"
				}
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

	fn scan_with(files: Vec<vag_data::LabelFile>, lbl: usize, clb: usize, rod: usize) -> LabelScan {
		LabelScan {
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
		let file = parse_label("022-906-032-C.LBL", b"001,1,Engine Speed,,Range: 0...6500 RPM");
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
	fn block_lookup_spans_all_label_files_and_field_narrows() {
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
