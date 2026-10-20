//! Label files loading: walk a VCDS `Labels/` directory, parse every plaintext
//! `.lbl` file and decrypt+parse every encrypted `.clb` file into a `Vec<LabelFile>`.
//!
//! Shared by the `vagcan dev vcds dump` binary (JSON/summary/lookup CLI) and the
//! `vag-data-db` crate (SQLite cache builder), so both stay in sync on how the
//! label files are walked and parsed.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::clb::decrypt_clb;
use crate::label::{LabelFile, parse_label};
use crate::redirect::{Redirects, Resolved};

/// Outcome of walking a labels directory.
pub struct LabelFileLoad {
	/// Parsed `.lbl` files and decrypted-then-parsed `.clb` files.
	pub files: Vec<LabelFile>,
	pub lbl_count: usize,
	pub clb_count: usize,
	pub other_count: usize,
	/// Files that matched a known extension but could not be read (skipped,
	/// not fatal).
	pub read_errors: usize,
}

fn has_ext(path: &Path, ext: &str) -> bool {
	path
		.extension()
		.and_then(|e| e.to_str())
		.map(|e| e.eq_ignore_ascii_case(ext))
		.unwrap_or(false)
}

fn file_name_or(path: &Path, fallback: &str) -> String {
	path.file_name().and_then(|n| n.to_str()).unwrap_or(fallback).to_string()
}

/// Walk `dir`, parse every `.lbl`, decrypt+parse every `.clb`, into label files.
/// Non-file entries and other extensions are counted, not parsed. Read errors
/// are counted (and the file skipped), never fatal.
///
/// The files are read, decrypted and parsed on rayon's pool, three thousand of
/// them, and come back in the directory's own order — the order a loop gave —
/// so the cache built from them holds the same rows under the same ids.
pub fn load_label_files(dir: &Path) -> io::Result<LabelFileLoad> {
	use rayon::prelude::*;

	/// One entry, as far as the listing says.
	enum Kind {
		Lbl,
		Clb,
		Other,
		NotAFile,
	}
	let paths: Vec<PathBuf> = std::fs::read_dir(dir)?.map(|entry| entry.map(|e| e.path())).collect::<io::Result<_>>()?;
	let read: Vec<(Kind, Option<LabelFile>)> = paths
		.par_iter()
		.map(|path| {
			let kind = match () {
				_ if !path.is_file() => Kind::NotAFile,
				_ if has_ext(path, "lbl") => Kind::Lbl,
				_ if has_ext(path, "clb") => Kind::Clb,
				_ => Kind::Other,
			};
			let parsed = match kind {
				Kind::Lbl | Kind::Clb => match std::fs::read(path) {
					Ok(bytes) => {
						let name = file_name_or(path, "<?>");
						Some(match kind {
							Kind::Clb => parse_label(name, &decrypt_clb(&bytes)),
							_ => parse_label(name, &bytes),
						})
					}
					Err(e) => {
						let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("warn: cannot read {}: {e}\n", path.display()));
						None
					}
				},
				Kind::Other | Kind::NotAFile => None,
			};
			(kind, parsed)
		})
		.collect();

	let mut load = LabelFileLoad {
		files: Vec::new(),
		lbl_count: 0,
		clb_count: 0,
		other_count: 0,
		read_errors: 0,
	};
	for (kind, parsed) in read {
		match kind {
			Kind::Lbl => load.lbl_count += 1,
			Kind::Clb => load.clb_count += 1,
			Kind::Other => load.other_count += 1,
			Kind::NotAFile => continue,
		}
		match parsed {
			Some(file) => load.files.push(file),
			None if !matches!(kind, Kind::Other) => load.read_errors += 1,
			None => {}
		}
	}
	Ok(load)
}

/// Outcome of a recursive scan (see [`scan_label_files`]).
pub struct LabelScan {
	/// Parsed `.lbl` + decrypted-then-parsed `.clb` files, from the whole tree.
	pub files: Vec<LabelFile>,
	pub lbl_count: usize,
	pub clb_count: usize,
	/// `.rod` files found. NOT parsed (the ODX crypto/inflate pipeline lives
	/// elsewhere) — counted only, so `vagcan dev vcds labels` can report label files size.
	pub rod_count: usize,
	pub other_count: usize,
	pub read_errors: usize,
}

/// Find the `.rod` files whose name matches an ODX identifier.
///
/// Control units name their own description file: reading identifier `F19E`
/// off the car yields e.g. `EV_ECM18TFS0208V0906264H`, and the corresponding
/// file is `EV_ECM18TFS0208V0906264H.rod`. That turns label selection from a
/// part-number guess into a lookup the car itself answers.
///
/// Matching ignores case and the extension, and tolerates the trailing NUL and
/// space padding VW puts in the identifier value. Several hits are possible
/// (localised corpora keep per-language copies), so all are returned, sorted.
pub fn find_rod_by_odx_name(root: &Path, odx_name: &str) -> io::Result<Vec<PathBuf>> {
	let wanted = odx_name.trim_end_matches(['\0', ' ']).to_ascii_uppercase();
	if wanted.is_empty() {
		return Ok(Vec::new());
	}
	let mut hits = Vec::new();
	collect_rod_matches(root, &wanted, &mut hits);
	hits.sort();
	Ok(hits)
}

/// How closely a `.rod` name answers a control unit's own identification, in the
/// order VCDS tries names — best first.
///
/// VCDS builds a unit's file name from what the unit reports (`F19E`, the first
/// three digits `vvv` of `F1A2`) and from the car's project (`SK37`, from the VIN
/// through `chassis.clb`, [`crate::chassis`]), and takes the first name that is a
/// file or a redirect source in `ReDir.rod`:
///
/// 1. `<name>_<vvv>_<project>`
/// 2. `<name>_<vvv>`
/// 3. `<name>_<project>`
/// 4. `<name>`
///
/// The evidence is `ReDir.rod` itself (`research/vcds-registry/README.md` §6b): it
/// redirects 154 `<name>_<vvv>` and 59 `<name>_<project>` names although `<name>`
/// is there, which would be dead lines if the bare name came first. Nothing in it
/// orders 2 against 3 — no line becomes unreachable either way — so that pair is
/// the likelier order, not a measured one.
///
/// The last two ranks are this tool's, not VCDS's: names VCDS would not try for
/// this car, offered when nothing it would try is there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OdxMatch {
	/// `<name>_<vvv>_<project>`, the project the car's.
	VersionProject,
	/// `<name>_<vvv>`.
	Version,
	/// `<name>_<project>`, the project the car's.
	Project,
	/// The name itself, no suffix.
	Exact,
	/// `<name>_<vvv>_<tag>`, the tag not the car's project — or no project known:
	/// the right version under another localisation.
	Localised,
	/// `<name>_<something else>` — the right family, an unconfirmed variant.
	Family,
}

impl OdxMatch {
	/// Whether VCDS itself tries a name of this rank for the car ([`OdxMatch`]'s first four).
	pub fn is_vcds_lookup(self) -> bool {
		matches!(self, Self::VersionProject | Self::Version | Self::Project | Self::Exact)
	}
}

/// Find the `.rod` files for a control unit from what the unit itself reports.
///
/// [`find_rod_by_odx_name`] matches the stem exactly, which finds only those
/// units whose ODX name happens to carry no suffix. The label files spell the rest
/// as `<odx name>_<vvv>`, `<odx name>_<project>` and `<odx name>_<vvv>_<project>`,
/// where `vvv` is the **first three digits of `F1A2`**: `F19E =
/// EV_Brake1UDSContiMK100ESP` with `F1A2 = 036010` is
/// `EV_Brake1UDSContiMK100ESP_036.rod`. Both halves come off the car, so this
/// stays a lookup the vehicle answers rather than a table about one vehicle
/// (`.archive/research/labels/fault-naming-hop.md` §10.4).
///
/// **The order** is [`OdxMatch`]'s: rank first, the car's project inside the rank
/// (`lookup.projects`), then a file before a redirect of the same rank, then by
/// path. The ranks VCDS tries name one name each, so the first candidate is the
/// name VCDS opens; the two ranks after them are guesses, offered in turn.
///
/// **Redirects.** VCDS takes a name that is a redirect source in `ReDir.rod`
/// ([`crate::redirect`]) as it takes a file: each source is ranked by the same
/// rule and brings its target at the source's rank, marked [`UnitRod::via`]. A
/// source that is itself a file name is not followed — the file is a candidate
/// already. That case does not occur in the one installation measured (no
/// source exists as a file there), so it is a choice: what the installation ships
/// under the name is what it opens.
///
/// **No data.** A redirect to `EMPTY` at a rank VCDS tries is VCDS saying it has
/// nothing for this unit. When it ranks ahead of every file, the answer is
/// [`UnitRods::nothing`] and no file at all, rather than a sibling variant guessed
/// in its place. One at a guessed rank — another project's, another version's —
/// says nothing about this car and is ignored.
///
/// `version` may be empty or short, in which case no candidate carries a version;
/// with no project, no candidate carries one either.
pub fn find_rod_by_odx_variant(root: &Path, odx_name: &str, version: &str, lookup: Lookup<'_>) -> io::Result<UnitRods> {
	Ok(RodIndex::scan(root).find(odx_name, version, lookup))
}

/// Every `.rod` file of an installation by upper-cased stem: the walk
/// [`find_rod_by_odx_variant`] makes, kept for a caller that looks up many units.
#[derive(Debug, Clone, Default)]
pub struct RodIndex {
	by_stem: HashMap<String, Vec<PathBuf>>,
}

impl RodIndex {
	/// Walk `root`; an unreadable subtree is skipped.
	pub fn scan(root: &Path) -> Self {
		let mut rods = Vec::new();
		collect_rods(root, &mut rods);
		let mut by_stem: HashMap<String, Vec<PathBuf>> = HashMap::new();
		for path in rods {
			if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
				by_stem.entry(stem.to_ascii_uppercase()).or_default().push(path);
			}
		}
		Self { by_stem }
	}

	/// [`find_rod_by_odx_variant`] on this index.
	pub fn find(&self, odx_name: &str, version: &str, lookup: Lookup<'_>) -> UnitRods {
		let Some((wanted, version)) = normalise(odx_name, version) else {
			return UnitRods::default();
		};
		let version = version.as_deref();
		let projects = projects_upper(lookup.projects);
		let by_stem = &self.by_stem;
		let mut files = Vec::new();
		for (stem, paths) in by_stem {
			if let Some(how) = rank(stem, &wanted, version, &projects) {
				files.extend(paths.iter().map(|path| UnitRod {
					how,
					path: path.clone(),
					via: None,
				}));
			}
		}
		let mut nothing: Option<(OdxMatch, String)> = None;
		for (source, upper) in lookup.redirects.sources() {
			let Some(how) = rank(upper, &wanted, version, &projects) else {
				continue;
			};
			if by_stem.contains_key(upper) {
				continue;
			}
			match lookup.redirects.resolve(upper, |stem| by_stem.contains_key(stem)) {
				Resolved::File(target) => files.extend(by_stem[&target.to_ascii_uppercase()].iter().map(|path| UnitRod {
					how,
					path: path.clone(),
					via: Some(source.to_string()),
				})),
				Resolved::Nothing if how.is_vcds_lookup() => {
					let key = (how, source.to_string());
					if nothing.as_ref().is_none_or(|held| key < *held) {
						nothing = Some(key);
					}
				}
				// A target this installation does not have, or a chain that does not
				// end, brings no file. VCDS stops there ("Redir Fail"); this lookup
				// offers what comes next, which the caller says is not the unit's own.
				_ => {}
			}
		}
		files.sort_by(|a, b| (a.how, a.via.is_some(), &a.path).cmp(&(b.how, b.via.is_some(), &b.path)));
		if let Some((how, source)) = nothing
			&& files.first().is_none_or(|best| how < best.how)
		{
			return UnitRods {
				files: Vec::new(),
				nothing: Some(source),
			};
		}
		UnitRods { files, nothing: None }
	}
}

/// What, beside the unit's own name, decides which file is its: the
/// installation's redirect table and the car's project codes (`SK37`).
#[derive(Debug, Clone, Copy)]
pub struct Lookup<'a> {
	pub redirects: &'a Redirects,
	pub projects: &'a [String],
}

impl Lookup<'_> {
	/// By the unit's own name alone: no redirects, no project.
	pub fn direct() -> Lookup<'static> {
		Lookup {
			redirects: &crate::redirect::NONE,
			projects: &[],
		}
	}
}

/// One file a unit's lookup offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitRod {
	pub how: OdxMatch,
	pub path: PathBuf,
	/// The redirect source this file stands in for, as `ReDir.rod` spells it
	/// (`EV_BCMMQB_017`); `None` for a file of the unit's own name.
	pub via: Option<String>,
}

impl UnitRod {
	/// The name the unit's data goes by: the redirect source, or the file stem.
	/// A unit's rows are joined to it by [`odx_match`], so a redirected unit's
	/// rows carry the source's name, not the target's.
	pub fn name(&self) -> &str {
		self
			.via
			.as_deref()
			.or_else(|| self.path.file_stem().and_then(|s| s.to_str()))
			.unwrap_or_default()
	}
}

/// `<source> → <file> (VCDS ReDir)`: how every output names a redirected file.
pub fn redirect_line(source: &str, file: &Path) -> String {
	let file = file.file_name().and_then(|n| n.to_str()).unwrap_or_default();
	format!("{source} → {file} (VCDS ReDir)")
}

/// A unit's candidate files, best first, or VCDS's word that it has none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnitRods {
	pub files: Vec<UnitRod>,
	/// The redirect source that says `EMPTY` for this unit, when it outranks
	/// every file: VCDS has no data for it. `files` is then empty.
	pub nothing: Option<String>,
}

/// How one candidate name answers a control unit's own identification, with no
/// filesystem involved.
///
/// The same rule [`find_rod_by_odx_variant`] applies to `.rod` stems, exposed so
/// a caller holding names from somewhere else can ask it too. It has two callers
/// and one implementation on purpose: an ODIS project's ECU variants are spelled
/// the same way — `EV_ECM18TFS0208V0906264H_001` is `<odx name>_<vvv>` — and the
/// project's cached VCDS rows go by the names the lookup chose, so a unit on the
/// car joins to them by exactly the rule that joins it to a label file. A second
/// copy of this rule would be free to drift from the first, and the failure would
/// be a car matched to the wrong control unit's data.
///
/// `odx_name` is the unit's `F19E` and `version` its `F1A2`, both as the car
/// answers them — trailing NULs and padding, full length; `projects` the car's
/// project codes, empty when not known. Normalising here rather than at the call
/// site is the point: a caller that trimmed differently would be reimplementing
/// the half of the rule it was trying to reuse.
///
/// `None` means `candidate` is not this unit's at all.
pub fn odx_match(candidate: &str, odx_name: &str, version: &str, projects: &[String]) -> Option<OdxMatch> {
	let (wanted, version) = normalise(odx_name, version)?;
	rank(
		&candidate.trim_end_matches(['\0', ' ']).to_ascii_uppercase(),
		&wanted,
		version.as_deref(),
		&projects_upper(projects),
	)
}

/// Bring a car's `F19E`/`F1A2` to the form the rule compares against.
///
/// `None` when there is no name to match; a `None` version means only that no
/// candidate can carry a version, not that nothing matches.
fn normalise(odx_name: &str, version: &str) -> Option<(String, Option<String>)> {
	let wanted = odx_name.trim_end_matches(['\0', ' ']).to_ascii_uppercase();
	if wanted.is_empty() {
		return None;
	}
	let version: String = version.trim_end_matches(['\0', ' ']).chars().take(3).collect();
	let version = (version.len() == 3 && version.chars().all(|c| c.is_ascii_digit())).then_some(version);
	Some((wanted, version))
}

/// Project codes as the rule compares them: upper-cased, blanks dropped.
fn projects_upper(projects: &[String]) -> Vec<String> {
	projects.iter().map(|p| p.trim().to_ascii_uppercase()).filter(|p| !p.is_empty()).collect()
}

/// The rule itself, on already-normalised inputs: [`OdxMatch`]'s order.
///
/// One implementation, so the directory walk, the redirect table and a caller's
/// own list of names cannot answer differently.
fn rank(candidate: &str, wanted: &str, version: Option<&str>, projects: &[String]) -> Option<OdxMatch> {
	if candidate == wanted {
		return Some(OdxMatch::Exact);
	}
	// Only a `_` starts a variant: `EV_TCMDQ2000210` is a different unit from
	// `EV_TCMDQ200021`, not a variant of it.
	let rest = candidate.strip_prefix(wanted)?.strip_prefix('_')?;
	let ours = |tag: &str| projects.iter().any(|p| p == tag);
	if let Some(tail) = version.and_then(|v| rest.strip_prefix(v)) {
		if tail.is_empty() {
			return Some(OdxMatch::Version);
		}
		if let Some(tag) = tail.strip_prefix('_') {
			return Some(if ours(tag) { OdxMatch::VersionProject } else { OdxMatch::Localised });
		}
	}
	Some(if ours(rest) { OdxMatch::Project } else { OdxMatch::Family })
}

/// Whether a directory entry is a directory, from what the listing already says.
///
/// `Path::is_dir` asks the file system again, once per entry: a unit's lookup
/// walks `UDS_EV`'s sixteen thousand files, and that second call per file was
/// most of what each unit cost a registry read once its keys came free. A link
/// is still followed, as `is_dir` would.
pub(crate) fn is_dir(entry: &std::fs::DirEntry) -> bool {
	match entry.file_type() {
		Ok(kind) if kind.is_symlink() => entry.path().is_dir(),
		Ok(kind) => kind.is_dir(),
		Err(_) => entry.path().is_dir(),
	}
}

/// Every `.rod` file under `dir`, recursively; an unreadable subtree is skipped.
fn collect_rods(dir: &Path, out: &mut Vec<PathBuf>) {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return;
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if is_dir(&entry) {
			collect_rods(&path, out);
		} else if has_ext(&path, "rod") {
			out.push(path);
		}
	}
}

fn collect_rod_matches(dir: &Path, wanted: &str, hits: &mut Vec<PathBuf>) {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return; // unreadable subtree: skip, never fatal (as elsewhere here)
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if is_dir(&entry) {
			collect_rod_matches(&path, wanted, hits);
			continue;
		}
		let is_rod = path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("rod"));
		if !is_rod {
			continue;
		}
		if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
			if stem.to_ascii_uppercase() == wanted {
				hits.push(path);
			}
		}
	}
}

/// Recursively walk `root`, parsing every `.lbl` and decrypting+parsing every
/// `.clb` into label files, and counting `.rod` files (parse not attempted).
///
/// Unlike [`load_label_files`] (single flat dir, `.lbl`/`.clb` only — kept as-is for
/// `vag-data-db`), this descends the whole VCDS install tree so a caller can point at
/// the install root and get `.lbl`/`.clb` (under `Labels/`) and `.rod` (under
/// `UDS_EV/`) in one pass. Unreadable dirs/files are counted as errors, skipped,
/// never fatal.
pub fn scan_label_files(root: &Path) -> io::Result<LabelScan> {
	let mut scan = LabelScan {
		files: Vec::new(),
		lbl_count: 0,
		clb_count: 0,
		rod_count: 0,
		other_count: 0,
		read_errors: 0,
	};
	scan_dir(root, &mut scan);
	Ok(scan)
}

/// Walk one directory level, recursing into subdirectories. Errors reading a
/// directory are counted and the subtree skipped (never fatal).
fn scan_dir(dir: &Path, scan: &mut LabelScan) {
	let entries = match std::fs::read_dir(dir) {
		Ok(e) => e,
		Err(e) => {
			let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("warn: cannot read dir {}: {e}\n", dir.display()));
			scan.read_errors += 1;
			return;
		}
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if path.is_dir() {
			scan_dir(&path, scan);
		} else if path.is_file() {
			scan_file(&path, scan);
		}
	}
}

/// Classify + (for `.lbl`/`.clb`) parse one file into the scan.
fn scan_file(path: &Path, scan: &mut LabelScan) {
	if has_ext(path, "lbl") {
		scan.lbl_count += 1;
		match std::fs::read(path) {
			Ok(bytes) => scan.files.push(parse_label(file_name_or(path, "<?>"), &bytes)),
			Err(e) => {
				let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("warn: cannot read {}: {e}\n", path.display()));
				scan.read_errors += 1;
			}
		}
	} else if has_ext(path, "clb") {
		scan.clb_count += 1;
		match std::fs::read(path) {
			Ok(bytes) => {
				let decoded = decrypt_clb(&bytes);
				scan.files.push(parse_label(file_name_or(path, "<?>"), &decoded));
			}
			Err(e) => {
				let _ = std::io::Write::write_fmt(&mut std::io::stderr(), format_args!("warn: cannot read {}: {e}\n", path.display()));
				scan.read_errors += 1;
			}
		}
	} else if has_ext(path, "rod") {
		scan.rod_count += 1;
	} else {
		scan.other_count += 1;
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Same synthetic 80-byte `.clb` fixture used in `clb.rs`'s tests
	/// (TEA-CBC-encrypted with `KEY_CLB`, `w7 = 7`) — no proprietary data.
	const FIXTURE_HEX: &str = "002738e02cf98f11742ee0b6f41102c2e55c4890aa526e2753a9263c7947f8b656f3467dc8f892f6c03a000a00202d7dc10402a81d837c41c4b66f69b6b50479e421595f5f5c20f4d6edd2d07b99000a";

	fn hex_decode(s: &str) -> Vec<u8> {
		assert_eq!(s.len() % 2, 0);
		(0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
	}

	/// A unique-per-test-run temp dir under the system temp dir, cleaned up
	/// on drop.
	struct TempDir(std::path::PathBuf);

	impl TempDir {
		fn new(tag: &str) -> Self {
			let path = std::env::temp_dir().join(format!("vag-data-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
			std::fs::create_dir_all(&path).unwrap();
			TempDir(path)
		}
	}

	impl Drop for TempDir {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir_all(&self.0);
		}
	}

	#[test]
	fn load_label_files_parses_lbl_and_clb_and_counts_other() {
		let dir = TempDir::new("label-files-test");

		std::fs::write(dir.0.join("plain.lbl"), b"001,1,Engine Speed,,Range: 0...6500 RPM").unwrap();

		let clb_bytes = hex_decode(FIXTURE_HEX);
		std::fs::write(dir.0.join("fixture.clb"), &clb_bytes).unwrap();

		std::fs::write(dir.0.join("readme.txt"), b"not a label file").unwrap();

		let load = load_label_files(&dir.0).expect("load_label_files should succeed");

		assert_eq!(load.lbl_count, 1);
		assert_eq!(load.clb_count, 1);
		assert_eq!(load.other_count, 1);
		assert_eq!(load.read_errors, 0);
		assert_eq!(load.files.len(), 2);

		let plain = load.files.iter().find(|f| f.source == "plain.lbl").expect("plain.lbl present");
		assert_eq!(plain.records.len(), 1);

		let fixture = load.files.iter().find(|f| f.source == "fixture.clb").expect("fixture.clb present");
		assert_eq!(fixture.records.len(), 2);
	}

	#[test]
	fn scan_label_files_recurses_and_counts_lbl_clb_rod() {
		// Mirror the real install layout: .lbl/.clb under Labels/, .rod under
		// UDS_EV/, a stray file at the root. scan_label_files must descend both.
		let dir = TempDir::new("scan-test");
		let labels = dir.0.join("Labels");
		let uds = dir.0.join("UDS_EV");
		std::fs::create_dir_all(&labels).unwrap();
		std::fs::create_dir_all(&uds).unwrap();

		std::fs::write(labels.join("plain.lbl"), b"001,1,Engine Speed,,Range: 0...6500 RPM").unwrap();
		std::fs::write(labels.join("fixture.clb"), hex_decode(FIXTURE_HEX)).unwrap();
		// .rod files are counted, not parsed — content is irrelevant here.
		std::fs::write(uds.join("STRUC.rod"), b"\x00\x01\x02not-really-odx").unwrap();
		std::fs::write(uds.join("TTTEXT.ROD"), b"\x00rod two").unwrap();
		std::fs::write(dir.0.join("readme.txt"), b"stray").unwrap();

		let scan = scan_label_files(&dir.0).expect("scan_label_files should succeed");

		assert_eq!(scan.lbl_count, 1);
		assert_eq!(scan.clb_count, 1);
		assert_eq!(scan.rod_count, 2, ".ROD is matched case-insensitively");
		assert_eq!(scan.other_count, 1);
		assert_eq!(scan.read_errors, 0);
		// Only .lbl + .clb are parsed into files; .rod is a bare count.
		assert_eq!(scan.files.len(), 2);
		assert!(scan.files.iter().any(|f| f.source == "plain.lbl"));
		assert!(scan.files.iter().any(|f| f.source == "fixture.clb"));
	}
}

#[cfg(test)]
mod odx_lookup_tests {
	use super::*;
	use crate::redirect::NOTHING;
	use crate::redirect::testkit::table;

	/// The lookup by the unit's own name, as `(rank, file)` pairs.
	fn walk(root: &Path, odx: &str, version: &str) -> Vec<(OdxMatch, PathBuf)> {
		let found = find_rod_by_odx_variant(root, odx, version, Lookup::direct()).unwrap();
		assert_eq!(found.nothing, None, "no redirects, no word from them");
		found
			.files
			.into_iter()
			.inspect(|f| assert_eq!(f.via, None))
			.map(|f| (f.how, f.path))
			.collect()
	}

	/// The lookup with a redirect table and the car's projects, as
	/// `(rank, file name, source)`, and the `EMPTY` source if that is the answer.
	type Found = (Vec<(OdxMatch, String, Option<String>)>, Option<String>);

	fn redirected(root: &Path, lines: &[(u32, &str, &str)], odx: &str, version: &str, projects: &[&str]) -> Found {
		let redirects = Redirects::parse(&table(lines));
		let projects: Vec<String> = projects.iter().map(|p| p.to_string()).collect();
		let found = find_rod_by_odx_variant(
			root,
			odx,
			version,
			Lookup {
				redirects: &redirects,
				projects: &projects,
			},
		)
		.unwrap();
		let files = found
			.files
			.into_iter()
			.map(|f| (f.how, f.path.file_name().unwrap().to_str().unwrap().to_string(), f.via))
			.collect();
		(files, found.nothing)
	}

	// The names below are invented. What the reference car's body control
	// module resolves to is checked against the installation, not here.

	#[test]
	fn a_unit_with_no_file_of_its_own_finds_the_one_its_redirect_names() {
		let root = label_files_at(&["EV_InvBOSCH_017.rod", "EV_InvBOSCH_016_VW37.rod", "EV_Unrelated_017.rod"]);
		let lines = [
			(101, "EV_InvMQB_017", "EV_InvBOSCH_017"),
			(102, "EV_InvMQB_016_SK37", "EV_InvBOSCH_016_VW37"),
			(103, "EV_InvMQBX_017", "EV_Unrelated_017"),
		];
		let (files, nothing) = redirected(&root, &lines, "EV_InvMQB\0", "017001", &[]);
		assert_eq!(nothing, None);
		assert_eq!(
			files,
			vec![
				(OdxMatch::Version, "EV_InvBOSCH_017.rod".into(), Some("EV_InvMQB_017".into())),
				(OdxMatch::Family, "EV_InvBOSCH_016_VW37.rod".into(), Some("EV_InvMQB_016_SK37".into())),
			],
			"ranked by the source's name, by the one rule; a longer name is another unit"
		);
		// Another localisation of the version, for a source as for a file: offered, after
		// anything VCDS would try, and first with the car's project.
		let (files, _) = redirected(&root, &lines, "EV_InvMQB", "016001", &[]);
		assert_eq!(files[0].0, OdxMatch::Localised);
		let (files, _) = redirected(&root, &lines, "EV_InvMQB", "016001", &["SK37"]);
		assert_eq!(files[0].0, OdxMatch::VersionProject);
		assert_eq!(files[0].2.as_deref(), Some("EV_InvMQB_016_SK37"));
	}

	#[test]
	fn a_file_and_a_redirect_at_one_rank_put_the_file_first_and_a_better_redirect_wins() {
		let root = label_files_at(&["EV_InvMQB_017_VW48.rod", "EV_InvMQB_012.rod", "EV_InvBOSCH_017.rod"]);
		let mut lines = vec![(101, "EV_InvMQB_017_AU37", "EV_InvBOSCH_017"), (104, "EV_InvMQB_012", "EV_InvBOSCH_017")];
		let names = |lines: &[(u32, &str, &str)]| -> Vec<(OdxMatch, String, bool)> {
			let (files, _) = redirected(&root, lines, "EV_InvMQB", "017001", &[]);
			files.into_iter().map(|(how, file, via)| (how, file, via.is_some())).collect()
		};
		assert_eq!(
			names(&lines),
			vec![
				(OdxMatch::Localised, "EV_InvMQB_017_VW48.rod".into(), false),
				(OdxMatch::Localised, "EV_InvBOSCH_017.rod".into(), true),
				(OdxMatch::Family, "EV_InvMQB_012.rod".into(), false),
			],
			"one rank: the file first; a source that is a file is the file, not followed"
		);
		lines.push((105, "EV_InvMQB_017", "EV_InvBOSCH_017"));
		assert_eq!(
			names(&lines)[0],
			(OdxMatch::Version, "EV_InvBOSCH_017.rod".into(), true),
			"a redirect of the version beats another localisation's file"
		);
	}

	/// The order VCDS tries names in, on the shapes of the lines that show it: a
	/// versioned name redirected while the bare file is there, a bare-name redirect
	/// beside a versioned one, and a project's name redirected beside the bare file.
	#[test]
	fn the_version_and_the_project_come_before_the_bare_name() {
		let root = label_files_at(&[
			"EV_InvACC.rod",
			"EV_InvACC49X_002.rod",
			"EV_InvCONTI.rod",
			"EV_InvBOSCH_017.rod",
			"EV_InvAirba010.rod",
			"EV_InvAirba10RB4_AU35.rod",
		]);
		let lines = [
			(501, "EV_InvACC_002", "EV_InvACC49X_002"),
			(502, "EV_InvBody", "EV_InvCONTI"),
			(503, "EV_InvBody_017", "EV_InvBOSCH_017"),
			(504, "EV_InvAirba010_AU35", "EV_InvAirba10RB4_AU35"),
		];
		let first = |odx: &str, version: &str, projects: &[&str]| redirected(&root, &lines, odx, version, projects).0[0].clone();
		assert_eq!(
			first("EV_InvACC", "002010", &[]),
			(OdxMatch::Version, "EV_InvACC49X_002.rod".into(), Some("EV_InvACC_002".into()))
		);
		assert_eq!(first("EV_InvACC", "003010", &[]).1, "EV_InvACC.rod", "another version: the bare file");
		assert_eq!(
			first("EV_InvBody", "017001", &[]),
			(OdxMatch::Version, "EV_InvBOSCH_017.rod".into(), Some("EV_InvBody_017".into())),
			"never the bare name's unit from another maker"
		);
		assert_eq!(
			first("EV_InvAirba010", "001001", &["AU35"]),
			(OdxMatch::Project, "EV_InvAirba10RB4_AU35.rod".into(), Some("EV_InvAirba010_AU35".into()))
		);
		assert_eq!(
			first("EV_InvAirba010", "001001", &[]).1,
			"EV_InvAirba010.rod",
			"no project: the bare file"
		);
	}

	#[test]
	fn empty_is_no_data_where_it_outranks_every_file_and_nothing_where_it_does_not() {
		let root = label_files_at(&["EV_InvMQB_012.rod", "EV_InvBOSCH_018.rod"]);
		let lines = [
			(201, "EV_InvMQB_017", NOTHING),
			(202, "EV_InvMQB_018", "EV_InvBOSCH_018"),
			(203, "EV_InvMQB_019_SK37", NOTHING),
			(204, "EV_InvMQB_014", NOTHING),
		];
		// The version the car reports is EMPTY: no data, and no sibling guessed.
		let (files, nothing) = redirected(&root, &lines, "EV_InvMQB", "017001", &[]);
		assert_eq!((files.len(), nothing.as_deref()), (0, Some("EV_InvMQB_017")));
		// Another version that has a file: the EMPTY siblings are only family.
		let (files, nothing) = redirected(&root, &lines, "EV_InvMQB", "018001", &[]);
		assert_eq!(nothing, None);
		assert_eq!(files[0].1, "EV_InvBOSCH_018.rod");
		// A version nothing names at all: family files, and a family EMPTY says nothing.
		let (files, nothing) = redirected(&root, &lines, "EV_InvMQB", "030001", &[]);
		assert_eq!(nothing, None);
		assert!(files.iter().all(|(how, ..)| *how == OdxMatch::Family));
		// Another project's EMPTY is not this car's answer; the car's own project's is.
		let (_, nothing) = redirected(&root, &lines, "EV_InvMQB", "019001", &[]);
		assert_eq!(nothing, None, "no project known: SK37's EMPTY is another localisation");
		let (_, nothing) = redirected(&root, &lines, "EV_InvMQB", "019001", &["VW48"]);
		assert_eq!(nothing, None);
		let (files, nothing) = redirected(&root, &lines, "EV_InvMQB", "019001", &["SK37"]);
		assert_eq!((files.len(), nothing.as_deref()), (0, Some("EV_InvMQB_019_SK37")));
	}

	#[test]
	fn the_cars_project_picks_among_sources_of_one_rank() {
		let root = label_files_at(&["EV_InvBOSCH_016_AU37.rod", "EV_InvBOSCH_016_VW37.rod"]);
		let lines = [
			(301, "EV_InvMQB_016_AU37", "EV_InvBOSCH_016_AU37"),
			(302, "EV_InvMQB_016_SK37", "EV_InvBOSCH_016_VW37"),
			(303, "EV_InvMQB_016_SK38P", NOTHING),
		];
		let first = |projects: &[&str]| redirected(&root, &lines, "EV_InvMQB", "016001", projects).0[0].2.clone();
		assert_eq!(first(&[]).as_deref(), Some("EV_InvMQB_016_AU37"), "no project: by name");
		assert_eq!(first(&["SK37"]).as_deref(), Some("EV_InvMQB_016_SK37"), "the project's source first");
		assert_eq!(
			first(&["ZZ99"]).as_deref(),
			Some("EV_InvMQB_016_AU37"),
			"a project nothing carries: by name"
		);
		// The car's project says EMPTY: that outranks another project's file.
		let (files, nothing) = redirected(&root, &lines, "EV_InvMQB", "016001", &["SK38P"]);
		assert_eq!((files.len(), nothing.as_deref()), (0, Some("EV_InvMQB_016_SK38P")));
		// `SK38` is not `SK38P`: a suffix is a whole word.
		let (_, nothing) = redirected(&root, &lines, "EV_InvMQB", "016001", &["SK38"]);
		assert_eq!(nothing, None);
	}

	#[test]
	fn a_missing_target_or_a_loop_brings_no_file() {
		let root = label_files_at(&["EV_Elsewhere_001.rod"]);
		let lines = [
			(401, "EV_InvMQB_017", "EV_InvBOSCH_017"),
			(402, "EV_InvMQB_018", "EV_InvLoop_018"),
			(403, "EV_InvLoop_018", "EV_InvMQB_018"),
		];
		for version in ["017001", "018001"] {
			assert_eq!(redirected(&root, &lines, "EV_InvMQB", version, &[]), (vec![], None), "{version}");
		}
	}

	/// Build a throwaway label files tree under the OS temp dir.
	///
	/// The directory is named after the files themselves. Naming it after
	/// their combined *length* — as this once did — gives two tests with
	/// equally long names the same tree, and since tests run in parallel each
	/// deletes the other's fixture at random.
	fn label_files_at(files: &[&str]) -> PathBuf {
		// The name has to distinguish `.ROD` from `.rod`, and a path cannot:
		// this filesystem is case-insensitive, so two tests differing only in
		// case shared a directory and deleted each other's fixtures. A hash of
		// the exact strings does distinguish them.
		let joined = files.join("_");
		let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
		for byte in joined.as_bytes() {
			hash = (hash ^ *byte as u64).wrapping_mul(0x1000_0000_01b3);
		}
		let key: String = joined.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
		let root = std::env::temp_dir().join(format!("vagcan-odx-{key}-{hash:016x}"));
		let _ = std::fs::remove_dir_all(&root);
		std::fs::create_dir_all(root.join("UDS_EV")).unwrap();
		for name in files {
			std::fs::write(root.join("UDS_EV").join(name), b"x").unwrap();
		}
		root
	}

	#[test]
	fn a_control_units_own_odx_name_finds_its_file() {
		// The name is what F19E returns on the reference car, padding included.
		let root = label_files_at(&["EV_ECM18TFS0208V0906264H.rod", "EV_TCMDQ200021.rod"]);
		let hits = find_rod_by_odx_name(&root, "EV_ECM18TFS0208V0906264H\0").unwrap();
		assert_eq!(hits.len(), 1);
		assert!(hits[0].ends_with("EV_ECM18TFS0208V0906264H.rod"), "{hits:?}");
	}

	#[test]
	fn case_and_extension_spelling_do_not_matter() {
		// VCDS installs ship both `.rod` and `.ROD`.
		let root = label_files_at(&["EV_TCMDQ200021.ROD"]);
		let hits = find_rod_by_odx_name(&root, "ev_tcmdq200021").unwrap();
		assert_eq!(hits.len(), 1);
	}

	#[test]
	fn a_name_the_label_files_do_not_have_finds_nothing() {
		let root = label_files_at(&["EV_TCMDQ200021.rod"]);
		assert!(find_rod_by_odx_name(&root, "EV_ECM_NOT_INSTALLED").unwrap().is_empty());
		// An empty identifier must not match every file in the tree.
		assert!(find_rod_by_odx_name(&root, "   ").unwrap().is_empty());
	}

	#[test]
	fn a_prefix_is_not_a_match() {
		// EV_TCMDQ200021 must not be answered by EV_TCMDQ2000210.
		let root = label_files_at(&["EV_TCMDQ2000210.rod"]);
		assert!(find_rod_by_odx_name(&root, "EV_TCMDQ200021").unwrap().is_empty());
	}

	#[test]
	fn f1a2_picks_the_variant_out_of_the_family() {
		// Both identifiers as the reference car's ESP reports them.
		let root = label_files_at(&[
			"EV_Brake1UDSContiMK100ESP_035.rod",
			"EV_Brake1UDSContiMK100ESP_036.rod",
			"EV_Brake1UDSContiMK100ESP_037.rod",
		]);
		let hits = walk(&root, "EV_Brake1UDSContiMK100ESP", "036010");
		assert_eq!(hits.len(), 3, "the whole family is offered: {hits:?}");
		assert_eq!(hits[0].0, OdxMatch::Version);
		assert!(hits[0].1.ends_with("EV_Brake1UDSContiMK100ESP_036.rod"), "{hits:?}");
		assert!(hits[1..].iter().all(|(m, _)| *m == OdxMatch::Family));
	}

	#[test]
	fn another_localisation_of_the_version_is_offered_after_what_vcds_would_try() {
		let root = label_files_at(&[
			"EV_EPHVA14AU3700000_009_SK37.rod",
			"EV_EPHVA14AU3700000_009_VW48.rod",
			"EV_EPHVA14AU3700000_VW26.rod",
		]);
		let hits = walk(&root, "EV_EPHVA14AU3700000", "009029");
		assert_eq!(hits.iter().filter(|(m, _)| *m == OdxMatch::Localised).count(), 2);
		assert_eq!(hits.iter().filter(|(m, _)| *m == OdxMatch::Family).count(), 1);
	}

	#[test]
	fn an_unsuffixed_name_ranks_after_the_version_and_the_project_and_before_the_guesses() {
		let stems = [
			"EV_SMLSVALEOMQBLRH",
			"EV_SMLSVALEOMQBLRH_001",
			"EV_SMLSVALEOMQBLRH_SK37",
			"EV_SMLSVALEOMQBLRH_001_SK37",
			"EV_SMLSVALEOMQBLRH_001_VW48",
			"EV_SMLSVALEOMQBLRH_002",
		];
		let files: Vec<String> = stems.iter().map(|s| format!("{s}.rod")).collect();
		let root = label_files_at(&files.iter().map(String::as_str).collect::<Vec<_>>());
		let order = |projects: &[&str]| -> Vec<(OdxMatch, String)> {
			let (files, _) = redirected(&root, &[], "EV_SMLSVALEOMQBLRH\0", "001007", projects);
			files
				.into_iter()
				.map(|(how, file, _)| (how, file.trim_end_matches(".rod").to_string()))
				.collect()
		};
		let at = |how, i: usize| (how, stems[i].to_string());
		assert_eq!(
			order(&["SK37"]),
			vec![
				at(OdxMatch::VersionProject, 3),
				at(OdxMatch::Version, 1),
				at(OdxMatch::Project, 2),
				at(OdxMatch::Exact, 0),
				at(OdxMatch::Localised, 4),
				at(OdxMatch::Family, 5),
			]
		);
		// No project known: the project's names are another localisation and a
		// family member, and the bare name comes right after the version.
		assert_eq!(
			order(&[]),
			vec![
				at(OdxMatch::Version, 1),
				at(OdxMatch::Exact, 0),
				at(OdxMatch::Localised, 3),
				at(OdxMatch::Localised, 4),
				at(OdxMatch::Family, 5),
				at(OdxMatch::Family, 2),
			]
		);
	}

	#[test]
	fn a_variant_lookup_does_not_widen_into_a_prefix_match() {
		// Same rule as the exact lookup: a longer name is a different unit.
		let root = label_files_at(&["EV_TCMDQ2000210.rod", "EV_TCMDQ2000210_001.rod"]);
		assert!(walk(&root, "EV_TCMDQ200021", "001001").is_empty());
	}

	#[test]
	fn a_version_that_is_not_three_digits_ranks_nothing() {
		let root = label_files_at(&["EV_GATEWNF_SK37.rod", "EV_GATEWNF_013.rod"]);
		// No F1A2 read (or a short one): the family is still offered, unranked.
		let hits = walk(&root, "EV_GatewNF", "");
		assert_eq!(hits.len(), 2);
		assert!(hits.iter().all(|(m, _)| *m == OdxMatch::Family));
		// An empty ODX name must not match the whole tree, as for the exact lookup.
		assert!(walk(&root, "  \0", "013020").is_empty());
	}

	/// The rule with no filesystem, which is how a caller holding names from
	/// somewhere else asks it.
	#[test]
	fn odx_match_ranks_a_bare_name() {
		let m = |c: &str| odx_match(c, "EV_Brake1UDSContiMK100ESP", "036010", &[]);
		assert_eq!(m("EV_Brake1UDSContiMK100ESP"), Some(OdxMatch::Exact));
		assert_eq!(m("EV_Brake1UDSContiMK100ESP_036"), Some(OdxMatch::Version));
		assert_eq!(m("EV_Brake1UDSContiMK100ESP_036_SK37"), Some(OdxMatch::Localised), "no project known");
		let sk37 = ["SK37".to_string()];
		assert_eq!(
			odx_match("EV_Brake1UDSContiMK100ESP_036_SK37", "EV_Brake1UDSContiMK100ESP", "036010", &sk37),
			Some(OdxMatch::VersionProject)
		);
		assert_eq!(
			odx_match("EV_Brake1UDSContiMK100ESP_sk37", "EV_Brake1UDSContiMK100ESP", "036010", &sk37),
			Some(OdxMatch::Project)
		);
		assert_eq!(m("EV_Brake1UDSContiMK100ESP_014"), Some(OdxMatch::Family));
		assert_eq!(m("EV_SomethingElse"), None);
		// A longer name is a different unit, not a variant of this one.
		assert_eq!(m("EV_Brake1UDSContiMK100ESPX"), None);
		assert_eq!(m("EV_Brake1UDSContiMK100ESPX_036"), None);
	}

	/// An ODIS project spells its ECU variants the same way, which is why this
	/// rule is exported rather than copied: `EV_ECM18TFS0208V0906264H_001` is
	/// `<odx name>_<vvv>`, and a unit on the car joins to a variant by exactly
	/// the rule that joins it to a label file.
	#[test]
	fn odx_match_joins_a_car_to_an_odis_variant() {
		// What the car answers, padding and all.
		let (name, version) = ("EV_ECM18TFS0208V0906264H\0\0", "001004  ");
		let variants = [
			"EV_ECM18TFS0208V0906264H_001",
			"EV_ECM18TFS0208V0906264H_002",
			"EV_ECM20TDI01105L906022BN_007",
		];
		let ranked: Vec<_> = variants.iter().map(|v| (odx_match(v, name, version, &[]), *v)).collect();
		assert_eq!(ranked[0].0, Some(OdxMatch::Version), "the version the car reports is the best match");
		assert_eq!(ranked[1].0, Some(OdxMatch::Family), "a sibling version is the right family, unconfirmed");
		assert_eq!(ranked[2].0, None, "another engine is not this unit at all");
	}

	/// Padding and case are normalised inside the rule, not at the call site.
	/// A caller that trimmed differently would be reimplementing the half of it
	/// they were trying to reuse.
	#[test]
	fn odx_match_normalises_what_the_car_pads() {
		assert_eq!(odx_match("ev_gatewnf_013", "EV_GatewNF\0\0 ", "013020\0", &[]), Some(OdxMatch::Version));
		// No name to match is no match, never a match against everything.
		assert_eq!(odx_match("EV_Anything", " \0", "013020", &[]), None);
	}

	/// The exported rule and the directory walk are one implementation, so they
	/// cannot answer differently. This is the assertion that keeps them so.
	#[test]
	fn odx_match_agrees_with_the_directory_walk() {
		let stems = [
			"EV_Brake1UDSContiMK100ESP_036",
			"EV_Brake1UDSContiMK100ESP_036_SK37",
			"EV_Brake1UDSContiMK100ESP_014",
			"EV_TCMDQ200021_001",
		];
		let files: Vec<String> = stems.iter().map(|s| format!("{s}.rod")).collect();
		let root = label_files_at(&files.iter().map(String::as_str).collect::<Vec<_>>());
		let walked = walk(&root, "EV_Brake1UDSContiMK100ESP", "036010");
		for (how, path) in &walked {
			let stem = path.file_stem().and_then(|s| s.to_str()).expect("a stem");
			assert_eq!(
				odx_match(stem, "EV_Brake1UDSContiMK100ESP", "036010", &[]),
				Some(*how),
				"{stem} ranked differently by the two callers"
			);
		}
		// And what the walk did not return, the rule does not match either.
		assert_eq!(walked.len(), 3);
		assert_eq!(odx_match("EV_TCMDQ200021_001", "EV_Brake1UDSContiMK100ESP", "036010", &[]), None);
	}
}
