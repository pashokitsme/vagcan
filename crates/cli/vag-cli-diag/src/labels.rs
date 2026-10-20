//! `vagcan dev vcds labels` — inventory + lookup over a VCDS label/ODX directory tree.
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

use std::path::Path;

use anyhow::Context;
use vag_cli_core::table::{self, Table};
use vag_data_labels::{LabelDb, LabelScan, Measurement, scan_label_files};

// The label directory locator is `core`'s now: the registry read keys its rows
// by it, and that read runs from `watch` and `measure` as well as from `setup`.
// Same name here, so every `crate::labels::label_dir_under` reads as it did.
pub(crate) use vag_cli_core::registry::label_dir_under;

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
	// In the spelling the cache keys sources by: a directory typed relative, or with a
	// trailing slash, is still the one the cache was built from.
	let wanted = vag_data_db::normalise_dir(&dir.to_string_lossy());
	let source = vag_data_db::source_of(cache);
	if source.as_deref() != Some(wanted.as_str()) {
		vag_cli_core::wire!(
			"label cache {}: built from {}, not from {}",
			cache.display(),
			source.as_deref().unwrap_or("no recorded directory"),
			wanted
		);
		return false;
	}
	let newer = match (std::fs::metadata(cache), std::fs::metadata(dir)) {
		(Ok(c), Ok(d)) => match (c.modified(), d.modified()) {
			(Ok(c), Ok(d)) => c >= d,
			_ => false,
		},
		_ => false,
	};
	vag_cli_core::wire!(
		"label cache {}: built from {}, {}",
		cache.display(),
		dir.display(),
		if newer { "newer than it" } else { "older than it" }
	);
	newer
}

/// Load a project's label rows out of its cache, with nothing to rebuild from.
///
/// **D5, and it is the consequence of D4 rather than a new rule.** The
/// `.lbl`/`.clb` files are no longer kept — `cache.sqlite` is what survives of
/// them — so there is no directory to compare an mtime against and no way to
/// rebuild if the answer came back "stale". A cache whose source directory is
/// gone is therefore **trusted**, not distrusted: the alternative is that every
/// run after somebody deletes their VCDS installation tries to rebuild from
/// nothing and reports a car with no label data at all.
///
/// The freshness rule still applies where it can still do something — see
/// [`load_cached`], which is what a directory somebody *named* goes through.
pub fn load_project(project: &crate::project::Project) -> anyhow::Result<LabelDb> {
	let cache = project.cache();
	anyhow::ensure!(
		cache.is_file(),
		crate::missing::NoLabelData::new(format!("The project `{}` has no label cache.", project.id)).looked_for(&cache)
	);
	vag_data_db::load_db(&cache).map_err(|e| anyhow::anyhow!("reading {}: {e}", cache.display()))
}

/// Whether this project has anything to look a label up in.
///
/// Cheap and non-committal, for the callers that degrade rather than fail: a
/// machine where `vagcan setup` has not run identifies units without names
/// instead of refusing to identify them.
pub fn has_project_labels(project: &crate::project::Project) -> bool {
	project.cache().is_file()
}

/// Load label files, using the SQLite cache when it is usable and building it when
/// it is not.
///
/// A stale cache is worse than none, so it is only trusted when
/// [`cache_is_current`] says so. `refresh` forces a rebuild regardless.
/// `cache` is the file to build into — a project's `cache.sqlite`. It used to
/// be one file per label directory, named after the flattened path, which kept
/// the English and the Russian install apart at the cost of a directory nobody
/// could read. The same property is kept more cheaply inside the cache itself:
/// it records which directory it was built from, and a cache built from another
/// one is rebuilt rather than trusted.
pub fn load_cached(dir: &Path, cache: &Path, refresh: bool) -> anyhow::Result<LabelDb> {
	let dir = &label_dir_under(dir)?;
	let fresh = !refresh && cache_is_current(cache, dir);
	if fresh {
		match vag_data_db::load_db(cache) {
			Ok(db) => {
				vag_cli_core::note!(
					"label cache {} ({} label files), current for {}",
					cache.display(),
					db.len(),
					dir.display()
				);
				return Ok(db);
			}
			// Rebuilding would meet the same refusal: say what to run instead.
			Err(e @ (vag_data_db::Error::OlderCache { .. } | vag_data_db::Error::NotACache { .. })) => anyhow::bail!("{e}"),
			// A corrupt or half-written cache is a reason to rebuild, not to
			// fail the command.
			Err(e) => vag_cli_core::note!("label cache {} unusable ({e}): rebuilding", cache.display()),
		}
	}

	if let Some(parent) = cache.parent() {
		std::fs::create_dir_all(parent).with_context(|| format!("creating the cache directory {}", parent.display()))?;
	}
	// Three thousand files parsed and decrypted inside one call, with nothing to
	// report from: the spinner is the only thing between that and a blank screen.
	let stats = {
		let _spinner = crate::progress::Spinner::new(format!("parsing the label files in {}", dir.display()));
		vag_data_db::build_db(dir, cache).map_err(|e| anyhow::anyhow!("building the label cache from {}: {e}", dir.display()))?
	};
	vag_cli_core::note!(
		"label cache {} rebuilt from {}: {} label files, {} measurements",
		cache.display(),
		dir.display(),
		stats.files,
		stats.measurements
	);
	vag_data_db::load_db(cache).map_err(|e| anyhow::anyhow!("reading the label cache: {e}"))
}

/// Hand the label files' unit numbering to the address layer.
///
/// `vag-uds-client` cannot read a label file — it is the protocol layer, and
/// label files are not a protocol — so the numbering is pushed in from here, where
/// both crates are already in scope. What crosses the seam is plain numbers and
/// strings.
///
/// Names only: no label file states which CAN id a number is answered on (see
/// `vag_uds_client::address`), so this tier fills in *what* `44` is and leaves
/// *where to reach it* to the car or to the built-in fallback.
pub fn install_unit_numbers(db: &LabelDb) {
	vag_uds_client::address::install(db.unit_numbers().iter().map(|(number, name)| vag_uds_client::address::UnitNumber {
		number: *number,
		request: None,
		name: Some(name.clone()),
	}));
}

/// The unit numbers `vagcan units` lists, in force for a command that takes a unit
/// (`faults --unit`, `sensors --unit`, `watch --did`, and `watch`'s Unit column).
///
/// The same two halves the plain listing joins, both already on this machine: the
/// part number each unit gave `units --identify` (the car's record), and the number
/// the label files give that part number. Nothing is asked of the car. A machine with
/// no label files, or no record, keeps the numbers it had — request ids still work.
pub fn install_recorded_units() {
	let Some(project) = crate::project::current().ok().filter(has_project_labels) else {
		return;
	};
	let Ok(db) = load_project(&project) else {
		return;
	};
	install_unit_numbers(&db);
	let cars = crate::units::recorded_cars().unwrap_or_default();
	vag_uds_client::address::install(recorded_pairings(&cars, |part| {
		db.unit_for_part(part).map(|u| (u.address, u.name.clone()))
	}));
}

/// Each recorded unit's number through its part number (`by_part`, the label files),
/// paired with the request id it answered on.
///
/// Over every recorded car, since which one is plugged in is not known before it is
/// asked. Where two records disagree — one number on two ids, or one id under two
/// numbers — neither is used: a guess there addresses another unit than the one named.
fn recorded_pairings(cars: &[crate::units::Recorded], by_part: impl Fn(&str) -> Option<(u8, String)>) -> Vec<vag_uds_client::address::UnitNumber> {
	let mut found: Vec<(u8, u16, String)> = Vec::new();
	for unit in cars.iter().flat_map(|car| &car.units) {
		let Some((number, name)) = unit.part_number.as_deref().and_then(&by_part) else {
			continue;
		};
		if !found.iter().any(|(n, r, _)| *n == number && *r == unit.request) {
			found.push((number, unit.request, name));
		}
	}
	found
		.iter()
		.filter(|(number, request, _)| found.iter().all(|(n, r, _)| (n == number) == (r == request)))
		.map(|(number, request, name)| vag_uds_client::address::UnitNumber {
			number: *number,
			request: Some(*request),
			name: Some(name.clone()),
		})
		.collect()
}

/// The label files under `dir`, for a lookup, **without writing the project's cache**.
///
/// The project's cache answers when it was built from these very label files and is
/// current ([`cache_is_current`]); anything else — another folder, `--refresh` — is
/// parsed in memory and forgotten. It used to be built into the project's cache, and a
/// build replaces the project's label files with the folder's: one lookup against a
/// one-file folder cost a project its installation's labels, and its source with its
/// registry channels (review of #37).
fn look_into(dir: &Path, project: Option<&crate::project::Project>, refresh: bool) -> anyhow::Result<LabelDb> {
	let labels = label_dir_under(dir)?;
	if !refresh
		&& let Some(project) = project
		&& cache_is_current(&project.cache(), &labels)
		&& let Ok(db) = vag_data_db::load_db(&project.cache())
	{
		return Ok(db);
	}
	let load = {
		let _spinner = crate::progress::Spinner::new(format!("parsing the label files in {}", labels.display()));
		vag_data_labels::load_label_files(&labels).with_context(|| format!("reading the label files in {}", labels.display()))?
	};
	Ok(LabelDb::new(load.files))
}

/// Entry point for the `labels` subcommand. Loads the label files once, prints the
/// summary, then runs whichever lookup(s) were requested.
pub fn labels_cmd(dir: &str, part: Option<&str>, block: Option<u16>, field: Option<u8>, refresh: bool) -> anyhow::Result<()> {
	// The inventory is what someone asking *nothing* wants; with a lookup on
	// the command line it is six lines of noise before the answer.
	if part.is_none() && block.is_none() {
		let scan = scan_label_files(Path::new(dir)).with_context(|| format!("scanning label files under {dir:?}"))?;
		vag_cli_core::out!("{}", render_summary(&scan));
		vag_cli_core::note!("scanned {dir}: .clb decrypted (TEA-CBC); .rod counted, not parsed (`vagcan dev vcds rod` opens one)");
		return Ok(());
	}

	let db = look_into(Path::new(dir), crate::project::current().ok().as_ref(), refresh)?;
	install_unit_numbers(&db);
	let mut answers = Vec::new();
	if let Some(part_no) = part {
		answers.push(render_part_lookup(&db, part_no));
		vag_cli_core::note!("part {part_no}: resolved through the REDIRECT chains of {} label files", db.len());
	}
	if let Some(b) = block {
		answers.push(render_block_lookup(&db, b, field));
		vag_cli_core::note!("block {b}: looked up in all {} label files", db.len());
	}
	vag_cli_core::out!("{}", answers.join("\n"));
	Ok(())
}

/// The file counts (`.lbl` / `.clb` / `.rod`) and one summary line.
fn render_summary(scan: &LabelScan) -> String {
	let mut table = Table::new(&["kind", "files"]).right(1);
	for (kind, n) in [(".lbl", scan.lbl_count), (".clb", scan.clb_count), (".rod", scan.rod_count)] {
		table.row(vec![kind.to_string(), n.to_string()]);
	}
	let mut out = table.render(table::terminal_width());
	let total = scan.lbl_count + scan.clb_count + scan.rod_count;
	let mut summary = format!("{total} files · {} .lbl/.clb parsed", scan.files.len());
	if scan.read_errors > 0 {
		summary.push_str(&format!(" · {} unreadable", scan.read_errors));
	}
	out.push_str(&summary);
	out.push('\n');
	out
}

/// `--part <PART_NO>`: the part, the file it resolves to and the unit it is, then that
/// file's measurements.
fn render_part_lookup(db: &LabelDb, part_no: &str) -> String {
	let Some(file) = db.resolve(part_no) else {
		return table::pairs(&[("part", part_no.to_string()), ("file", "none".to_string())]);
	};
	let mut rows = vec![("part", part_no.to_string()), ("file", file.source.clone())];
	// Which unit this part number *is*, in the label files' own numbering — the
	// answer `vagcan units --identify` needs to name a unit `44` rather than `712`.
	if let Some(unit) = db.unit_for_part(part_no) {
		rows.push(("unit", format!("{:02X}  {}", unit.address, unit.name)));
	}
	let mut out = table::pairs(&rows);
	let ms = db.measurements(part_no);
	if !ms.is_empty() {
		out.push('\n');
		let mut table = measurement_table(&[]);
		for m in &ms {
			table.row(measurement_cells(&[], m));
		}
		out.push_str(&table.render(table::terminal_width()));
	}
	out.push_str(&format!("{} measurements\n", ms.len()));
	out
}

/// `--block <N> [--field <F>]`: every file defining that block, and a summary line.
fn render_block_lookup(db: &LabelDb, block: u16, field: Option<u8>) -> String {
	let scope = match field {
		Some(f) => format!("block {block} field {f}"),
		None => format!("block {block}"),
	};
	let hits = db.measurements_by_block(block, field);
	let mut out = String::new();
	if !hits.is_empty() {
		let mut table = measurement_table(&["file"]);
		for (source, m) in &hits {
			table.row(measurement_cells(&[source], m));
		}
		out.push_str(&table.render(table::terminal_width()));
	}
	out.push_str(&format!("{} definitions · {scope}\n", hits.len()));
	out
}

/// The measurement columns, after `leading`: block and field right-aligned, the name last.
fn measurement_table(leading: &[&str]) -> Table {
	let headers: Vec<&str> = leading.iter().copied().chain(["block", "field", "unit", "name"]).collect();
	Table::new(&headers).right(leading.len()).right(leading.len() + 1)
}

fn measurement_cells(leading: &[&str], m: &Measurement) -> Vec<String> {
	let mut cells: Vec<String> = leading.iter().map(|s| s.to_string()).collect();
	cells.extend([
		m.block.to_string(),
		m.field.to_string(),
		m.unit.clone().unwrap_or_default(),
		m.name.clone(),
	]);
	cells
}

/// Resolve the label file a control unit names for itself.
///
/// The unit's `F19E` identifier holds its ODX file name (e.g.
/// `EV_ECM18TFS0208V0906264H`), so selecting the right description file stops
/// being a part-number guess: the car answers the question. Reports what the
/// file contains, marking sections whose payload did not decode rather than
/// implying the whole file was read.
pub fn resolve_odx(dir: &str, odx_name: &str, cache_path: &Path) -> anyhow::Result<()> {
	use vag_data_labels::rod::{IvCache, decode_rod_recover};

	let hits = vag_data_labels::find_rod_by_odx_name(std::path::Path::new(dir), odx_name)?;
	vag_cli_core::note!("{} file(s) named {odx_name}.rod under {dir}", hits.len());
	if hits.is_empty() {
		vag_cli_core::out!("{}", table::pairs(&[("odx", odx_name.to_string()), ("file", "none".to_string())]));
		// The control unit names this file itself, so the label files are either
		// incomplete or this is the wrong directory.
		vag_cli_core::say!("pass the VCDS install root, or a directory above its .rod files");
		return Ok(());
	}

	// Recovered initialisation vectors come from the cache only. The search
	// that fills it costs minutes of every core and is its own command
	// (`vagcan dev vcds rod --cache <this cache> <file.rod>`), so a lookup
	// never starts one.
	let mut cache = IvCache::load(cache_path);
	vag_cli_core::note!("{} keys in {}", cache.len(), cache_path.display());
	let mut table = Table::new(&["file", "section", "bytes", "state"]).right(2);
	let (mut open, mut total) = (0usize, 0usize);
	let mut sealed_files = Vec::new();
	for path in &hits {
		let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
		vag_cli_core::note!("reading {}", path.display());
		let bytes = match std::fs::read(path) {
			Ok(b) => b,
			Err(e) => {
				table.row(vec![name.to_string(), String::new(), String::new(), format!("unreadable: {e}")]);
				continue;
			}
		};
		let sections = decode_rod_recover(&bytes, name, &mut cache, false);
		if sections.is_empty() {
			table.row(vec![name.to_string(), String::new(), String::new(), "no sections".to_string()]);
			continue;
		}
		for section in &sections {
			let state = crate::vcds::rod::state(section);
			vag_cli_core::wire!(
				"{name} [{}]: {:?}, {}",
				section.tag,
				section.status,
				match cache.get(name, &section.tag) {
					Some(key) => format!("key {} from the cache", vag_cli_core::verbosity::hex(&key)),
					None => "no key in the cache".to_string(),
				}
			);
			match state {
				// No vector in the cache for this section, and a lookup does not search
				// for one: `rod` does.
				"sealed" => {
					if !sealed_files.contains(path) {
						sealed_files.push(path.clone());
					}
				}
				// Pointing at the recovery command here would waste an hour of every
				// core: the search cannot start on this file at all.
				"no crib" => vag_cli_core::note!(
					"{name} [{}] no crib: the key search cannot start on this file; not damaged, not yet openable \
					 (.archive/research/labels/tttext2.md)",
					section.tag
				),
				_ => {}
			}
			let size = section.text.as_ref().map(|t| t.len()).unwrap_or(0);
			open += usize::from(section.text.is_some());
			total += 1;
			table.row(vec![name.to_string(), section.tag.clone(), size.to_string(), state.to_string()]);
		}
	}
	vag_cli_core::out!("{}", table.render(table::terminal_width()));
	vag_cli_core::outln!("{open}/{total} sections open · {} files", hits.len());
	for path in sealed_files {
		vag_cli_core::say!(
			"to open its sealed sections: vagcan dev vcds rod --cache {} {}",
			cache_path.display(),
			path.display()
		);
	}
	Ok(())
}
#[cfg(test)]
mod tests {
	use super::*;
	use vag_data_labels::parse_label;

	/// What `units` lists is what `--unit` and `--did` take: a recorded unit's part
	/// number gives its number through the label files, and the record gives the id it
	/// answered on.
	#[test]
	fn a_recorded_part_number_pairs_its_label_file_number_with_the_id_that_answered() {
		use crate::plan::UnitIdentity;
		use crate::units::Recorded;
		let unit = |request: u16, part: Option<&str>| UnitIdentity {
			request,
			part_number: part.map(str::to_owned),
			..UnitIdentity::default()
		};
		let car = |vin: &str, units: Vec<UnitIdentity>| Recorded { vin: vin.to_string(), units };
		let by_part = |part: &str| match part {
			"STEER" => Some((0x44, "Power Steering".to_string())),
			"BRAKE" => Some((0x03, "Brakes".to_string())),
			"OTHER" => Some((0x44, "Power Steering".to_string())),
			_ => None,
		};
		let pairs = |cars: &[Recorded]| -> Vec<(u8, Option<u16>, Option<String>)> {
			recorded_pairings(cars, by_part)
				.into_iter()
				.map(|p| (p.number, p.request, p.name))
				.collect()
		};
		let one = [car(
			"A",
			vec![
				unit(0x712, Some("STEER")),
				unit(0x713, Some("BRAKE")),
				unit(0x70A, Some("NOBODY")),
				unit(0x715, None),
			],
		)];
		assert_eq!(
			pairs(&one),
			vec![
				(0x44, Some(0x712), Some("Power Steering".to_string())),
				(0x03, Some(0x713), Some("Brakes".to_string()))
			],
			"a part the label files do not know, and a unit with no part number, give nothing"
		);
		// Two records agreeing is one pairing.
		let same = [one[0].clone(), car("B", vec![unit(0x712, Some("STEER"))])];
		assert_eq!(pairs(&same), pairs(&one));
		// Two records disagreeing: which car is plugged in is not known before it is
		// asked, so neither pairing is guessed at — by number, or by id.
		let by_number = [one[0].clone(), car("B", vec![unit(0x70C, Some("OTHER"))])];
		assert_eq!(pairs(&by_number), vec![(0x03, Some(0x713), Some("Brakes".to_string()))]);
		let by_id = [one[0].clone(), car("B", vec![unit(0x712, Some("BRAKE"))])];
		assert_eq!(pairs(&by_id), vec![]);
	}

	#[test]
	fn a_cache_whose_label_files_are_gone_is_trusted_rather_than_declared_stale() {
		// D5. The `.lbl`/`.clb` files are not kept any more, so there is nothing
		// to compare an mtime against and nothing to rebuild from if the answer
		// came back "stale". Without this, every run after somebody deletes
		// their VCDS installation tries to rebuild from nothing.
		let here = tempfile::tempdir().unwrap();
		let labels = here.path().join("Labels");
		std::fs::create_dir_all(&labels).unwrap();
		std::fs::write(labels.join("index.lbl"), b"001,1,Engine Speed,(G28),Range: 0...6500 RPM").unwrap();

		let project = crate::project::Project {
			id: "SK37X".into(),
			dir: here.path().join("projects").join("SK37X"),
		};
		std::fs::create_dir_all(&project.dir).unwrap();
		vag_data_db::build_db(&labels, &project.cache()).unwrap();

		// The installation goes, which is the whole point of the copy that
		// preceded it — and the cache still answers.
		std::fs::remove_dir_all(&labels).unwrap();
		assert!(has_project_labels(&project));
		let db = load_project(&project).unwrap();
		assert_eq!(db.measurement("index.lbl", 1, 1).map(|m| m.name.as_str()), Some("Engine Speed"));
	}

	#[test]
	fn a_project_with_no_cache_says_to_run_setup_rather_than_naming_sqlite() {
		let here = tempfile::tempdir().unwrap();
		let project = crate::project::Project {
			id: "SK37X".into(),
			dir: here.path().join("SK37X"),
		};
		assert!(!has_project_labels(&project));
		let why = match load_project(&project) {
			Err(e) => e.to_string(),
			Ok(_) => panic!("a project with no cache loaded one"),
		};
		assert!(why.contains("vagcan setup"), "{why}");
	}

	fn scan_with(files: Vec<vag_data_labels::LabelFile>, lbl: usize, clb: usize, rod: usize) -> LabelScan {
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
		assert_eq!(
			render_summary(&scan),
			"kind  files\n\
			 .lbl   1181\n\
			 .clb   1854\n\
			 .rod  16578\n\
			 19613 files · 0 .lbl/.clb parsed\n"
		);
		let broken = LabelScan {
			read_errors: 2,
			..scan_with(vec![], 1, 0, 0)
		};
		assert!(render_summary(&broken).ends_with("1 files · 0 .lbl/.clb parsed · 2 unreadable\n"));
	}

	#[test]
	fn part_lookup_lists_measurements_of_resolved_file() {
		let file = parse_label("022-906-032-C.LBL", b"001,1,Engine Speed,,Range: 0...6500 RPM");
		let db = LabelDb::new(vec![file]);
		assert_eq!(
			render_part_lookup(&db, "022-906-032-C"),
			"part  022-906-032-C\n\
			 file  022-906-032-C.LBL\n\
			 \n\
			 block  field  unit  name\n\
			 \x20   1      1  RPM   Engine Speed\n\
			 1 measurements\n"
		);
	}

	#[test]
	fn part_lookup_reports_miss() {
		let db = LabelDb::new(vec![parse_label("X.LBL", b"001,1,Irrelevant,,")]);
		assert_eq!(render_part_lookup(&db, "999-999-999-Z"), "part  999-999-999-Z\nfile  none\n");
	}

	#[test]
	fn block_lookup_spans_all_label_files_and_field_narrows() {
		let a = parse_label("AAA.LBL", b"002,1,Engine Speed,,Range: 0...6500 RPM");
		let b = parse_label("BBB.LBL", b"002,1,Vehicle Speed,,Range: 0...300 km/h\n002,2,Coolant,,");
		let db = LabelDb::new(vec![a, b]);

		let all = render_block_lookup(&db, 2, None);
		assert_eq!(
			all,
			"file     block  field  unit  name\n\
			 AAA.LBL      2      1  RPM   Engine Speed\n\
			 BBB.LBL      2      1  km/h  Vehicle Speed\n\
			 BBB.LBL      2      2        Coolant\n\
			 3 definitions · block 2\n"
		);

		let f1 = render_block_lookup(&db, 2, Some(1));
		assert!(f1.ends_with("2 definitions · block 2 field 1\n"), "{f1}");
		assert!(!f1.contains("Coolant"));

		assert_eq!(render_block_lookup(&db, 42, None), "0 definitions · block 42\n");
	}
}
