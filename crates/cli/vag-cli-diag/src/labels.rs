//! `vagcan dev vcds labels` — inventory + lookup over a VCDS label/ODX directory tree.
//!
//! Points at a VCDS install root (or any subtree), counts the `.lbl` / `.clb`
//! files of its label directory and the `.rod` files of the whole tree, and
//! answers two lookups:
//!
//! - `--part <PART_NO>` — resolve an ECU part number through the `REDIRECT`
//!   chain to its terminal label file and list that file's measurements
//!   (the same resolution `vagcan info`'s label path uses).
//! - `--block <N> [--field <F>]` — a cross-label-file scan: every label file that
//!   defines measuring block `N` (optionally narrowed to field `F`), with the
//!   measurement's name and physical unit (`phys`).
//!
//! No brand/model dimension: VCDS label files are keyed by part number, not by
//! vehicle make/model, so there is nothing to group by. Rendering is factored
//! into pure `render_*` helpers so the formatting is unit-tested without a disk.

use std::path::Path;

use anyhow::Context;
use vag_cli_core::table::{self, Table};
use vag_cli_core::verbosity::{self, NOTES, WIRE};
use vag_data_labels::{LabelDb, Measurement, load_label_files};

use crate::render::plural;
use vag_cli_core::marks::facts;

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
	// Whether it is newer is said by the caller's one line: current, or rebuilt.
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
					"label cache {} ({} {}), current for {}",
					cache.display(),
					db.len(),
					plural(db.len(), "label file"),
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
		"label cache {} rebuilt from {}: {} {}, {} {}",
		cache.display(),
		dir.display(),
		stats.files,
		plural(stats.files, "label file"),
		stats.measurements,
		plural(stats.measurements, "measurement")
	);
	vag_data_db::load_db(cache).map_err(|e| anyhow::anyhow!("reading the label cache: {e}"))
}

/// The label files under `dir`, for a lookup, **without writing the VCDS store's cache**.
///
/// The store's cache answers when it was built from these very label files and is
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
		vag_cli_core::note!(
			"label cache {} ({} {}), current for {}",
			project.cache().display(),
			db.len(),
			plural(db.len(), "label file"),
			labels.display()
		);
		return Ok(db);
	}
	let load = {
		let _spinner = crate::progress::Spinner::new(format!("parsing the label files in {}", labels.display()));
		vag_data_labels::load_label_files(&labels).with_context(|| format!("reading the label files in {}", labels.display()))?
	};
	vag_cli_core::note!(
		"label cache not used for {}: {} {} parsed, nothing written",
		labels.display(),
		load.files.len(),
		plural(load.files.len(), "label file")
	);
	Ok(LabelDb::new(load.files))
}

/// Entry point for the `labels` subcommand. Loads the label files once, prints the
/// summary, then runs whichever lookup(s) were requested.
pub fn labels_cmd(dir: &str, part: Option<&str>, block: Option<u16>, field: Option<u8>, refresh: bool) -> anyhow::Result<()> {
	// The inventory is what someone asking *nothing* wants; with a lookup on
	// the command line it is six lines of noise before the answer.
	if part.is_none() && block.is_none() {
		let inventory = inventory(Path::new(dir))?;
		vag_cli_core::out!("{}", render_summary(Path::new(dir), &inventory));
		match &inventory.label_dir {
			Some(labels) => vag_cli_core::note!(
				".lbl/.clb counted and parsed in {}, the directory `dump` reads and the label cache is built from; .clb decrypted (TEA-CBC)",
				labels.display()
			),
			None => vag_cli_core::note!("no directory of .lbl/.clb files under {dir}"),
		}
		vag_cli_core::note!(".rod counted everywhere under {dir}, not parsed (`vagcan dev vcds rod` opens one)");
		for (at, stray) in inventory.strays.iter().enumerate() {
			verbosity::say(
				if at < 10 { NOTES } else { WIRE },
				&format!("not counted: {}, outside the label directory", stray.display()),
			);
		}
		return Ok(());
	}

	let db = look_into(Path::new(dir), crate::project::vcds().ok().as_ref(), refresh)?;
	let mut answers = Vec::new();
	if let Some(part_no) = part {
		answers.push(render_part_lookup(&db, part_no));
		vag_cli_core::note!(
			"part {part_no}: resolved through the REDIRECT chains of {} {}",
			db.len(),
			plural(db.len(), "label file")
		);
	}
	if let Some(b) = block {
		answers.push(render_block_lookup(&db, b, field));
		vag_cli_core::note!("block {b}: looked up in {} {}", db.len(), plural(db.len(), "label file"));
	}
	vag_cli_core::out!("{}", answers.join("\n"));
	Ok(())
}

/// What the inventory counts under a directory.
///
/// `.lbl`/`.clb` in the label directory alone ([`label_dir_under`]), the set `dump` parses
/// and the label cache is built from, so the three print one figure. A VCDS install keeps
/// a label file outside it too (`UDS_EV/` holds one `.clb`); counting the whole tree gave
/// one `.clb` more here than `dump` and `stats` print. Those are `strays`, named at `-v`.
/// `.rod` everywhere under the directory: they live in `UDS_EV/`, not with the labels.
#[derive(Debug, Default, PartialEq)]
struct Inventory {
	label_dir: Option<std::path::PathBuf>,
	lbl: usize,
	clb: usize,
	parsed: usize,
	rod: usize,
	strays: Vec<std::path::PathBuf>,
	read_errors: usize,
}

fn inventory(dir: &Path) -> anyhow::Result<Inventory> {
	anyhow::ensure!(dir.is_dir(), "{} is not a directory", dir.display());
	let mut found = Inventory {
		label_dir: label_dir_under(dir).ok(),
		..Inventory::default()
	};
	if let Some(labels) = &found.label_dir {
		let load = load_label_files(labels).with_context(|| format!("reading the label files in {}", labels.display()))?;
		found.lbl = load.lbl_count;
		found.clb = load.clb_count;
		found.parsed = load.files.len();
		found.read_errors = load.read_errors;
	}
	walk(dir, &mut found);
	found.strays.sort();
	Ok(found)
}

/// Count the `.rod` files under `dir` and collect the `.lbl`/`.clb` outside the label
/// directory. Names only: nothing here is opened.
fn walk(dir: &Path, found: &mut Inventory) {
	let Ok(entries) = std::fs::read_dir(dir) else {
		found.read_errors += 1;
		return;
	};
	for path in entries.flatten().map(|e| e.path()) {
		if path.is_dir() {
			walk(&path, found);
			continue;
		}
		let ext = path.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
		match ext.as_deref() {
			Some("rod") => found.rod += 1,
			Some("lbl" | "clb") if path.parent() != found.label_dir.as_deref() => found.strays.push(path),
			_ => {}
		}
	}
}

/// The inventory as a card, `gh pr view`'s shape: the directory, a meta line of totals,
/// then the file counts (`.lbl` / `.clb` / `.rod`).
fn render_summary(dir: &Path, found: &Inventory) -> String {
	let total = found.lbl + found.clb + found.rod;
	let meta = facts([
		format!("{total} {}", plural(total, "file")),
		format!("{} {} parsed", found.parsed, plural(found.parsed, "label file")),
		match found.read_errors {
			0 => String::new(),
			n => format!("{n} unreadable"),
		},
	]);
	let rows: Vec<(&str, String)> = [(".lbl", found.lbl), (".clb", found.clb), (".rod", found.rod)]
		.into_iter()
		.map(|(kind, n)| (kind, n.to_string()))
		.collect();
	format!(
		"{}\n{meta}\n\n{}",
		vag_cli_core::datadir::tilde(dir),
		table::section("FILES", table::counts(&rows))
	)
}

/// `--part <PART_NO>`: a card — the part and the file it resolves to, the unit it is and how
/// many measurements on the meta line, then the measurements.
fn render_part_lookup(db: &LabelDb, part_no: &str) -> String {
	let Some(file) = db.resolve(part_no) else {
		return format!("{}\n", facts([part_no, "no label file"]));
	};
	let ms = db.measurements(part_no);
	let mut out = format!("{}\n", facts([format!("part {part_no}"), file.source.clone()]));
	// Which unit this part number *is*, in the label files' own numbering — the
	// answer `vagcan units --identify` needs to name a unit `44` rather than `712`.
	let unit = db
		.unit_for_part(part_no)
		.map(|unit| format!("unit {:02X} {}", unit.address, unit.name))
		.unwrap_or_default();
	out.push_str(&facts([unit, format!("{} {}", ms.len(), plural(ms.len(), "measurement"))]));
	out.push('\n');
	if !ms.is_empty() {
		out.push('\n');
		out.push_str(&table::section("MEASUREMENTS", measurement_lines(&ms, 2)));
	}
	out
}

/// `--block <N> [--field <F>]`: grouped records — the count, then each file defining that
/// block as a heading with its definitions indented under it.
fn render_block_lookup(db: &LabelDb, block: u16, field: Option<u8>) -> String {
	let scope = match field {
		Some(f) => format!("block {block} field {f}"),
		None => format!("block {block}"),
	};
	let hits = db.measurements_by_block(block, field);
	let mut out = format!("{} {} of {scope}\n", hits.len(), plural(hits.len(), "definition"));
	let mut at = 0;
	while at < hits.len() {
		let source = hits[at].0;
		let group: Vec<&Measurement> = hits[at..].iter().take_while(|(s, _)| *s == source).map(|(_, m)| *m).collect();
		at += group.len();
		out.push('\n');
		out.push_str(&table::section(source, measurement_lines(&group, 2)));
	}
	out
}

/// Each measurement as one line, `block.field  name · phys`, a name too long for the line
/// wrapped under itself and never cut. `indent`: what the lines will be indented by.
///
/// `phys`, the physical unit, after the name: a label file's unit field can be a sentence
/// (53 characters in one), and it wraps with the name rather than squeezing it.
fn measurement_lines(ms: &[&Measurement], indent: usize) -> Vec<String> {
	let width = table::terminal_width().unwrap_or(table::WIDTH);
	let codes: Vec<String> = ms.iter().map(|m| format!("{}.{}", m.block, m.field)).collect();
	let code_width = codes.iter().map(|c| c.chars().count()).max().unwrap_or(0);
	let room = width.saturating_sub(indent + code_width + 2).max(20);
	let pad = " ".repeat(code_width + 2);
	ms.iter()
		.zip(&codes)
		.map(|(m, code)| {
			let text = facts([m.name.clone(), m.unit.clone().unwrap_or_default()]);
			let mut pieces = table::wrap(&text, room).into_iter();
			let mut line = format!("{code:<code_width$}  {}", pieces.next().unwrap_or_default());
			for more in pieces {
				line.push('\n');
				line.push_str(&pad);
				line.push_str(&more);
			}
			line.trim_end().to_string()
		})
		.collect()
}

/// What `labels --odx` says when no `.rod` answers the name.
///
/// The control unit names this file itself, so either the directory is not a VCDS
/// install or the install lacks the unit's file. Only the first is something to fix
/// here: a directory that holds `UDS_EV/` (or is it) is the install root already, and
/// telling somebody who passed it to pass it is a wrong answer.
fn no_rod(dir: &Path, odx_name: &str) -> Vec<String> {
	let missing = format!("no {odx_name}.rod under {}", dir.display());
	let is_uds_ev = |p: &Path| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.eq_ignore_ascii_case("UDS_EV"));
	let holds_uds_ev = std::fs::read_dir(dir).is_ok_and(|entries| entries.flatten().any(|e| e.path().is_dir() && is_uds_ev(&e.path())));
	if is_uds_ev(dir) || holds_uds_ev {
		return vec![missing];
	}
	vec![missing, "  (pass the VCDS install root, the directory that holds UDS_EV/)".to_string()]
}

/// Resolve the label file a control unit names for itself.
///
/// The unit's `F19E` identifier holds its ODX file name (e.g.
/// `EV_ECM18TFS0208V0906264H`), so selecting the right description file stops
/// being a part-number guess: the car answers the question. Reports what the
/// file contains, marking sections whose payload did not decode rather than
/// implying the whole file was read.
///
/// An exact-name lookup, not [`vag_data_labels::find_rod_by_odx_variant`]'s ranked one: the
/// name given is a file name (`EV_BCMMQB_017`), not a unit's `F19E`/`F1A2` and project. With
/// no file of that name, the redirect of that very name ([`vag_data_labels::redirect::Redirects::target`])
/// is followed, and a bare name the table only has variants of lists them.
pub fn resolve_odx(dir: &str, odx_name: &str, cache_path: &Path) -> anyhow::Result<()> {
	use vag_data_labels::rod::{IvCache, decode_rod_recover};

	use crate::vcds::rod::{NO_CRIB, SECTION_COLUMNS, cipher, state};

	// Recovered initialisation vectors come from the cache only. The search
	// that fills it costs a search of minutes per file and is its own command
	// (`vagcan dev vcds rod --cache <this cache> <file.rod>`), so a lookup
	// never starts one.
	let mut cache = IvCache::load(cache_path);
	vag_cli_core::note!("{} {} in {}", cache.len(), plural(cache.len(), "key"), cache_path.display());
	let root = Path::new(dir);
	let mut hits = vag_data_labels::find_rod_by_odx_name(root, odx_name)?;
	vag_cli_core::note!("{} {} named {odx_name}.rod under {dir}", hits.len(), plural(hits.len(), "file"));
	if hits.is_empty() {
		match redirected(root, odx_name, &mut cache)? {
			// The table below names another file than the one asked for: said, so it is
			// not read as this name's own.
			Redirected::To { line, files } => {
				vag_cli_core::say!("{line}");
				hits = files;
			}
			Redirected::Nothing { source } => {
				vag_cli_core::outln!("{}", facts([odx_name, "EMPTY (VCDS ReDir)"]));
				vag_cli_core::say!("{source} → EMPTY (VCDS ReDir): VCDS has no data for this unit");
				return Ok(());
			}
			Redirected::No { variants } => {
				vag_cli_core::outln!("{}", facts([odx_name, "no .rod file"]));
				for line in no_rod(root, odx_name) {
					vag_cli_core::say!("{line}");
				}
				if !variants.is_empty() {
					vag_cli_core::say!(
						"ReDir.rod redirects {} {} of it; pass one with its version: {}",
						variants.len(),
						plural(variants.len(), "variant"),
						variants.join(", ")
					);
				}
				return Ok(());
			}
		}
	}
	let headers: Vec<String> = ["file"].into_iter().chain(SECTION_COLUMNS).map(str::to_uppercase).collect();
	let headers: Vec<&str> = headers.iter().map(String::as_str).collect();
	let mut table = Table::new(&headers).right(3);
	let (mut open, mut total) = (0usize, 0usize);
	let mut sealed_files = Vec::new();
	for path in &hits {
		let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
		vag_cli_core::note!("reading {}", path.display());
		let bytes = match std::fs::read(path) {
			Ok(b) => b,
			Err(e) => {
				table.row(vec![name.to_string(), String::new(), format!("unreadable: {e}")]);
				continue;
			}
		};
		let sections = decode_rod_recover(&bytes, name, &mut cache, false);
		if sections.is_empty() {
			table.row(vec![name.to_string(), String::new(), "no sections".to_string()]);
			continue;
		}
		for section in &sections {
			vag_cli_core::wire!(
				"{name} [{}]: {:?}, {}",
				section.tag,
				section.status,
				match cache.get(name, &section.tag) {
					Some(key) => format!("key {} from the cache", vag_cli_core::verbosity::hex(&key)),
					None => "no key in the cache".to_string(),
				}
			);
			match cipher(section) {
				// No vector in the cache for this section, and a lookup does not search
				// for one: `rod` does.
				"no key" => {
					vag_cli_core::note!("{name} [{}] no key", section.tag);
					if !sealed_files.contains(path) {
						sealed_files.push(path.clone());
					}
				}
				// Pointing at the recovery command here would waste the search's time:
				// it cannot start on this file at all.
				"no crib" => vag_cli_core::note!("{name} [{}] no crib: {NO_CRIB}", section.tag),
				cipher => vag_cli_core::note!("{name} [{}] {cipher}", section.tag),
			}
			let size = section.text.as_ref().map(|t| t.len()).unwrap_or(0);
			open += usize::from(section.text.is_some());
			total += 1;
			table.row(vec![name.to_string(), section.tag.clone(), state(section).to_string(), size.to_string()]);
		}
	}
	let summary = facts([
		format!("{open} of {total} {} open", plural(total, "section")),
		format!("{} {}", hits.len(), plural(hits.len(), "file")),
	]);
	vag_cli_core::out!("{}", table::list(&summary, &table, table::terminal_width()));
	for path in sealed_files {
		vag_cli_core::say!(
			"  (use \"vagcan dev vcds rod --cache {} {}\" to open its sealed sections)",
			vag_cli_core::datadir::tilde(cache_path),
			vag_cli_core::datadir::tilde(&path)
		);
	}
	Ok(())
}

/// What VCDS's `ReDir.rod` says about a name the installation has no file of.
enum Redirected {
	/// It stands for these files: `line` says so, `source → file (VCDS ReDir)`.
	To { line: String, files: Vec<std::path::PathBuf> },
	/// It is redirected to `EMPTY`: VCDS has no data for the unit.
	Nothing { source: String },
	/// Not redirected; `variants` are the redirected names of its family
	/// (`<name>_<version>…`), for a name given without its version.
	No { variants: Vec<String> },
}

/// Look `odx_name` up in the installation's redirect table. Opened with the
/// cached keys only, as the files are; a table that does not open redirects
/// nothing.
fn redirected(root: &Path, odx_name: &str, cache: &mut vag_data_labels::rod::IvCache) -> anyhow::Result<Redirected> {
	use vag_data_labels::redirect::{Resolved, load_redirects};
	let load = load_redirects(root, cache, false);
	let Some(redirects) = load.redirects() else {
		return Ok(Redirected::No { variants: Vec::new() });
	};
	let name = odx_name.trim_end_matches(['\0', ' ']);
	let has_file = |stem: &str| vag_data_labels::find_rod_by_odx_name(root, stem).is_ok_and(|hits| !hits.is_empty());
	Ok(match redirects.target(name).map(|_| redirects.resolve(name, has_file)) {
		Some(Resolved::File(target)) => {
			let files = vag_data_labels::find_rod_by_odx_name(root, &target)?;
			let line = files
				.first()
				.map(|file| vag_data_labels::label_files::redirect_line(name, file))
				.unwrap_or_default();
			Redirected::To { line, files }
		}
		Some(Resolved::Nothing) => Redirected::Nothing { source: name.to_string() },
		_ => {
			let family = format!("{}_", name.to_ascii_uppercase());
			Redirected::No {
				variants: redirects
					.sources()
					.filter(|(_, upper)| upper.starts_with(&family))
					.map(|(written, _)| written.to_string())
					.collect(),
			}
		}
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use vag_data_labels::parse_label;

	#[test]
	fn summary_reports_all_three_file_kinds_and_total() {
		let found = Inventory {
			lbl: 1181,
			clb: 1854,
			parsed: 3035,
			rod: 16578,
			..Inventory::default()
		};
		assert_eq!(
			render_summary(Path::new("/v"), &found),
			"/v\n\
			 19613 files · 3035 label files parsed\n\
			 \n\
			 FILES\n\
			 \x20 .lbl   1181\n\
			 \x20 .clb   1854\n\
			 \x20 .rod  16578\n"
		);
		let broken = Inventory {
			lbl: 1,
			read_errors: 2,
			..Inventory::default()
		};
		assert!(render_summary(Path::new("/v"), &broken).starts_with("/v\n1 file · 0 label files parsed · 2 unreadable\n"));
	}

	/// A VCDS install keeps one `.clb` in `UDS_EV/`, beside the `.rod` files. The cache
	/// and `dump` read `Labels/` alone, so the inventory counts `Labels/` alone and names
	/// the stray: before, it printed one `.clb` more than they did.
	#[test]
	fn the_inventory_counts_the_label_directory_and_names_what_lies_outside_it() {
		let here = tempfile::tempdir().unwrap();
		let labels = here.path().join("Labels");
		let uds = here.path().join("UDS_EV");
		std::fs::create_dir_all(&labels).unwrap();
		std::fs::create_dir_all(uds.join("deeper")).unwrap();
		std::fs::write(labels.join("A.lbl"), b"001,1,Engine Speed,,").unwrap();
		std::fs::write(labels.join("readme.txt"), b"not a label file").unwrap();
		std::fs::write(uds.join("chassis.clb"), b"stray").unwrap();
		std::fs::write(uds.join("EV_A.rod"), b"").unwrap();
		std::fs::write(uds.join("deeper").join("EV_B.ROD"), b"").unwrap();

		let found = inventory(here.path()).unwrap();
		assert_eq!(
			found,
			Inventory {
				label_dir: Some(labels),
				lbl: 1,
				clb: 0,
				parsed: 1,
				rod: 2,
				strays: vec![uds.join("chassis.clb")],
				read_errors: 0,
			}
		);
		assert!(inventory(&here.path().join("nope")).is_err());
	}

	#[test]
	fn no_rod_offers_the_install_root_only_to_a_directory_that_is_not_one() {
		let here = tempfile::tempdir().unwrap();
		let root = here.path().join("vcds");
		std::fs::create_dir_all(root.join("UDS_EV")).unwrap();
		std::fs::create_dir_all(root.join("Labels")).unwrap();
		let miss = |dir: &Path| format!("no EV_X.rod under {}", dir.display());
		assert_eq!(no_rod(&root, "EV_X"), vec![miss(&root)], "the install root, passed");
		assert_eq!(no_rod(&root.join("UDS_EV"), "EV_X"), vec![miss(&root.join("UDS_EV"))]);
		assert_eq!(
			no_rod(&root.join("Labels"), "EV_X"),
			vec![
				miss(&root.join("Labels")),
				"  (pass the VCDS install root, the directory that holds UDS_EV/)".to_string()
			]
		);
	}

	#[test]
	fn part_lookup_lists_measurements_of_resolved_file() {
		let file = parse_label("022-906-032-C.LBL", b"001,1,Engine Speed,,Range: 0...6500 RPM");
		let db = LabelDb::new(vec![file]);
		assert_eq!(
			render_part_lookup(&db, "022-906-032-C"),
			"part 022-906-032-C · 022-906-032-C.LBL\n\
			 1 measurement\n\
			 \n\
			 MEASUREMENTS\n\
			 \x20 1.1  Engine Speed · RPM\n"
		);
	}

	#[test]
	fn part_lookup_reports_miss() {
		let db = LabelDb::new(vec![parse_label("X.LBL", b"001,1,Irrelevant,,")]);
		assert_eq!(render_part_lookup(&db, "999-999-999-Z"), "999-999-999-Z · no label file\n");
	}

	/// Names are free text: a long physical unit wraps under its name, never cut, and every
	/// line fits 80 columns.
	#[test]
	fn a_long_physical_unit_wraps_and_is_never_cut() {
		let long = "Range: 0...255 = see the workshop manual for the meaning of each bit of the status byte";
		let db = LabelDb::new(vec![parse_label(
			"AAA.LBL",
			format!("002,1,Engine Speed,,Range: 0...6500 RPM\n002,2,Status bits,,{long}").as_bytes(),
		)]);
		let text = render_block_lookup(&db, 2, None);
		assert!(text.contains("\n  2.1  Engine Speed · RPM\n"), "{text}");
		assert!(text.contains("  2.2  Status bits · "), "{text}");
		assert!(text.lines().all(|l| l.chars().count() <= 80), "{text}");
		assert!(text.contains("\n       the status byte\n"), "the rest wraps under the name: {text}");
	}

	#[test]
	fn block_lookup_spans_all_label_files_and_field_narrows() {
		let a = parse_label("AAA.LBL", b"002,1,Engine Speed,,Range: 0...6500 RPM");
		let b = parse_label("BBB.LBL", b"002,1,Vehicle Speed,,Range: 0...300 km/h\n002,2,Coolant,,");
		let db = LabelDb::new(vec![a, b]);

		let all = render_block_lookup(&db, 2, None);
		assert_eq!(
			all,
			"3 definitions of block 2\n\
			 \n\
			 AAA.LBL\n\
			 \x20 2.1  Engine Speed · RPM\n\
			 \n\
			 BBB.LBL\n\
			 \x20 2.1  Vehicle Speed · km/h\n\
			 \x20 2.2  Coolant\n"
		);

		let f1 = render_block_lookup(&db, 2, Some(1));
		assert!(f1.starts_with("2 definitions of block 2 field 1\n"), "{f1}");
		assert!(!f1.contains("Coolant"));

		assert_eq!(render_block_lookup(&db, 42, None), "0 definitions of block 42\n");
	}
}
