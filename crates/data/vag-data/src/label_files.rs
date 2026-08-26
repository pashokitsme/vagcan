//! Label files loading: walk a VCDS `Labels/` directory, parse every plaintext
//! `.lbl` file and decrypt+parse every encrypted `.clb` file into a `Vec<LabelFile>`.
//!
//! Shared by the `vagcan vcds dump` binary (JSON/summary/lookup CLI) and the
//! `vag-db` crate (SQLite cache builder), so both stay in sync on how the
//! label files are walked and parsed.

use std::io;
use std::path::{Path, PathBuf};

use crate::clb::decrypt_clb;
use crate::label::{LabelFile, parse_label};

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
pub fn load_label_files(dir: &Path) -> io::Result<LabelFileLoad> {
	let entries = std::fs::read_dir(dir)?;

	let mut files = Vec::new();
	let mut lbl_count = 0;
	let mut clb_count = 0;
	let mut other_count = 0;
	let mut read_errors = 0;

	for entry in entries {
		let entry = entry?;
		let path = entry.path();
		if !path.is_file() {
			continue;
		}
		if has_ext(&path, "lbl") {
			lbl_count += 1;
			match std::fs::read(&path) {
				Ok(bytes) => {
					let name = file_name_or(&path, "<?>");
					files.push(parse_label(name, &bytes));
				}
				Err(e) => {
					eprintln!("warn: cannot read {}: {e}", path.display());
					read_errors += 1;
				}
			}
		} else if has_ext(&path, "clb") {
			clb_count += 1;
			match std::fs::read(&path) {
				Ok(bytes) => {
					let name = file_name_or(&path, "<?>");
					let decoded = decrypt_clb(&bytes);
					files.push(parse_label(name, &decoded));
				}
				Err(e) => {
					eprintln!("warn: cannot read {}: {e}", path.display());
					read_errors += 1;
				}
			}
		} else {
			other_count += 1;
		}
	}

	Ok(LabelFileLoad {
		files,
		lbl_count,
		clb_count,
		other_count,
		read_errors,
	})
}

/// Outcome of a recursive scan (see [`scan_label_files`]).
pub struct LabelScan {
	/// Parsed `.lbl` + decrypted-then-parsed `.clb` files, from the whole tree.
	pub files: Vec<LabelFile>,
	pub lbl_count: usize,
	pub clb_count: usize,
	/// `.rod` files found. NOT parsed (the ODX crypto/inflate pipeline lives
	/// elsewhere) — counted only, so `vagcan vcds labels` can report label files size.
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

/// How closely a `.rod` file name answers a control unit's own identification.
///
/// Ordered best-first: `Exact` beats `Version` beats `Family`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OdxMatch {
	/// The stem *is* the ODX name — the unit's file has no variant suffix.
	Exact,
	/// `<odx name>_<version>` or `<odx name>_<version>_<localisation>`, where
	/// `<version>` is the leading three digits of the unit's `F1A2`.
	Version,
	/// `<odx name>_<something else>` — the right family, an unconfirmed variant.
	Family,
}

/// Find the `.rod` files for a control unit from what the unit itself reports.
///
/// [`find_rod_by_odx_name`] matches the stem exactly, which finds only those
/// units whose ODX name happens to carry no suffix — two of the fifteen on the
/// reference car. The label files spell the rest as `<odx name>_<vvv>` and
/// `<odx name>_<vvv>_<localisation>`, where `vvv` is the **first three digits of
/// `F1A2`** and the localisation is a brand/platform tag (`SK37`, `VW48`, …):
/// `F19E = EV_Brake1UDSContiMK100ESP` with `F1A2 = 036010` is
/// `EV_Brake1UDSContiMK100ESP_036.rod`. Both halves come off the car, so this
/// stays a lookup the vehicle answers rather than a table about one vehicle
/// (`research/labels/fault-naming-hop.md` §10.4).
///
/// Returns every candidate paired with how it matched, best match first. More
/// than one is normal and is not an error — a family keeps one file per
/// localisation, and only the caller knows whether it can tell them apart.
/// `version` may be empty or short, in which case no candidate ranks `Version`.
pub fn find_rod_by_odx_variant(root: &Path, odx_name: &str, version: &str) -> io::Result<Vec<(OdxMatch, PathBuf)>> {
	let Some((wanted, version)) = normalise(odx_name, version) else {
		return Ok(Vec::new());
	};
	let mut hits = Vec::new();
	collect_rod_family(root, &wanted, version.as_deref(), &mut hits);
	hits.sort();
	Ok(hits)
}

/// How one candidate name answers a control unit's own identification, with no
/// filesystem involved.
///
/// The same rule [`find_rod_by_odx_variant`] applies to `.rod` stems, exposed so
/// a caller holding names from somewhere else can ask it too. It has two callers
/// and one implementation on purpose: an ODIS project's ECU variants are spelled
/// the same way — `EV_ECM18TFS0208V0906264H_001` is `<odx name>_<vvv>` — so a
/// unit on the car joins to a variant by exactly the rule that joins it to a
/// label file. A second copy of this rule would be free to drift from the first,
/// and the failure would be a car matched to the wrong control unit's data.
///
/// `odx_name` is the unit's `F19E` and `version` its `F1A2`, both as the car
/// answers them — trailing NULs and padding, full length. Normalising here
/// rather than at the call site is the point: a caller that trimmed differently
/// would be reimplementing the half of the rule it was trying to reuse.
///
/// `None` means `candidate` is not this unit's at all.
pub fn odx_match(candidate: &str, odx_name: &str, version: &str) -> Option<OdxMatch> {
	let (wanted, version) = normalise(odx_name, version)?;
	rank(&candidate.trim_end_matches(['\0', ' ']).to_ascii_uppercase(), &wanted, version.as_deref())
}

/// Bring a car's `F19E`/`F1A2` to the form the rule compares against.
///
/// `None` when there is no name to match; a `None` version means only that no
/// candidate can rank [`OdxMatch::Version`], not that nothing matches.
fn normalise(odx_name: &str, version: &str) -> Option<(String, Option<String>)> {
	let wanted = odx_name.trim_end_matches(['\0', ' ']).to_ascii_uppercase();
	if wanted.is_empty() {
		return None;
	}
	let version: String = version.trim_end_matches(['\0', ' ']).chars().take(3).collect();
	let version = (version.len() == 3 && version.chars().all(|c| c.is_ascii_digit())).then_some(version);
	Some((wanted, version))
}

/// The rule itself, on already-normalised inputs.
///
/// One implementation, so the directory walk and a caller's own list of names
/// cannot answer differently.
fn rank(candidate: &str, wanted: &str, version: Option<&str>) -> Option<OdxMatch> {
	if candidate == wanted {
		return Some(OdxMatch::Exact);
	}
	// Only a `_` starts a variant: `EV_TCMDQ2000210` is a different unit from
	// `EV_TCMDQ200021`, not a variant of it.
	let rest = candidate.strip_prefix(wanted)?.strip_prefix('_')?;
	let versioned = version.is_some_and(|v| rest.strip_prefix(v).is_some_and(|tail| tail.is_empty() || tail.starts_with('_')));
	Some(if versioned { OdxMatch::Version } else { OdxMatch::Family })
}

fn collect_rod_family(dir: &Path, wanted: &str, version: Option<&str>, hits: &mut Vec<(OdxMatch, PathBuf)>) {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return;
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if path.is_dir() {
			collect_rod_family(&path, wanted, version, hits);
			continue;
		}
		if !path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("rod")) {
			continue;
		}
		let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
			continue;
		};
		if let Some(how) = rank(&stem.to_ascii_uppercase(), wanted, version) {
			hits.push((how, path));
		}
	}
}

fn collect_rod_matches(dir: &Path, wanted: &str, hits: &mut Vec<PathBuf>) {
	let Ok(entries) = std::fs::read_dir(dir) else {
		return; // unreadable subtree: skip, never fatal (as elsewhere here)
	};
	for entry in entries.flatten() {
		let path = entry.path();
		if path.is_dir() {
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
/// `vag-db`), this descends the whole VCDS install tree so a caller can point at
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
			eprintln!("warn: cannot read dir {}: {e}", dir.display());
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
				eprintln!("warn: cannot read {}: {e}", path.display());
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
				eprintln!("warn: cannot read {}: {e}", path.display());
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
		let hits = find_rod_by_odx_variant(&root, "EV_Brake1UDSContiMK100ESP", "036010").unwrap();
		assert_eq!(hits.len(), 3, "the whole family is offered: {hits:?}");
		assert_eq!(hits[0].0, OdxMatch::Version);
		assert!(hits[0].1.ends_with("EV_Brake1UDSContiMK100ESP_036.rod"), "{hits:?}");
		assert!(hits[1..].iter().all(|(m, _)| *m == OdxMatch::Family));
	}

	#[test]
	fn the_localisation_suffix_rides_along_with_the_version() {
		let root = label_files_at(&[
			"EV_EPHVA14AU3700000_009_SK37.rod",
			"EV_EPHVA14AU3700000_009_VW48.rod",
			"EV_EPHVA14AU3700000_VW26.rod",
		]);
		let hits = find_rod_by_odx_variant(&root, "EV_EPHVA14AU3700000", "009029").unwrap();
		assert_eq!(hits.iter().filter(|(m, _)| *m == OdxMatch::Version).count(), 2);
		assert_eq!(hits.iter().filter(|(m, _)| *m == OdxMatch::Family).count(), 1);
	}

	#[test]
	fn an_unsuffixed_name_still_matches_exactly_and_ranks_first() {
		let root = label_files_at(&["EV_SMLSVALEOMQBLRH.rod", "EV_SMLSVALEOMQBLRH_002.rod"]);
		let hits = find_rod_by_odx_variant(&root, "EV_SMLSVALEOMQBLRH\0", "001007").unwrap();
		assert_eq!(hits[0].0, OdxMatch::Exact);
		assert!(hits[0].1.ends_with("EV_SMLSVALEOMQBLRH.rod"), "{hits:?}");
	}

	#[test]
	fn a_variant_lookup_does_not_widen_into_a_prefix_match() {
		// Same rule as the exact lookup: a longer name is a different unit.
		let root = label_files_at(&["EV_TCMDQ2000210.rod", "EV_TCMDQ2000210_001.rod"]);
		assert!(find_rod_by_odx_variant(&root, "EV_TCMDQ200021", "001001").unwrap().is_empty());
	}

	#[test]
	fn a_version_that_is_not_three_digits_ranks_nothing() {
		let root = label_files_at(&["EV_GATEWNF_SK37.rod", "EV_GATEWNF_013.rod"]);
		// No F1A2 read (or a short one): the family is still offered, unranked.
		let hits = find_rod_by_odx_variant(&root, "EV_GatewNF", "").unwrap();
		assert_eq!(hits.len(), 2);
		assert!(hits.iter().all(|(m, _)| *m == OdxMatch::Family));
		// An empty ODX name must not match the whole tree, as for the exact lookup.
		assert!(find_rod_by_odx_variant(&root, "  \0", "013020").unwrap().is_empty());
	}

	/// The rule with no filesystem, which is how a caller holding names from
	/// somewhere else asks it.
	#[test]
	fn odx_match_ranks_a_bare_name() {
		let m = |c: &str| odx_match(c, "EV_Brake1UDSContiMK100ESP", "036010");
		assert_eq!(m("EV_Brake1UDSContiMK100ESP"), Some(OdxMatch::Exact));
		assert_eq!(m("EV_Brake1UDSContiMK100ESP_036"), Some(OdxMatch::Version));
		assert_eq!(m("EV_Brake1UDSContiMK100ESP_036_SK37"), Some(OdxMatch::Version));
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
		let ranked: Vec<_> = variants.iter().map(|v| (odx_match(v, name, version), *v)).collect();
		assert_eq!(ranked[0].0, Some(OdxMatch::Version), "the version the car reports is the best match");
		assert_eq!(ranked[1].0, Some(OdxMatch::Family), "a sibling version is the right family, unconfirmed");
		assert_eq!(ranked[2].0, None, "another engine is not this unit at all");
	}

	/// Padding and case are normalised inside the rule, not at the call site.
	/// A caller that trimmed differently would be reimplementing the half of it
	/// they were trying to reuse.
	#[test]
	fn odx_match_normalises_what_the_car_pads() {
		assert_eq!(odx_match("ev_gatewnf_013", "EV_GatewNF\0\0 ", "013020\0"), Some(OdxMatch::Version));
		// No name to match is no match, never a match against everything.
		assert_eq!(odx_match("EV_Anything", " \0", "013020"), None);
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
		let walked = find_rod_by_odx_variant(&root, "EV_Brake1UDSContiMK100ESP", "036010").unwrap();
		for (how, path) in &walked {
			let stem = path.file_stem().and_then(|s| s.to_str()).expect("a stem");
			assert_eq!(
				odx_match(stem, "EV_Brake1UDSContiMK100ESP", "036010"),
				Some(*how),
				"{stem} ranked differently by the two callers"
			);
		}
		// And what the walk did not return, the rule does not match either.
		assert_eq!(walked.len(), 3);
		assert_eq!(odx_match("EV_TCMDQ200021_001", "EV_Brake1UDSContiMK100ESP", "036010"), None);
	}
}
