//! VCDS's redirect table: the unit names it has no file of its own for, and the
//! file that serves each.
//!
//! A control unit names its own description file (`F19E`, with `F1A2` picking
//! the variant — [`crate::label_files::find_rod_by_odx_variant`]), and for some
//! units the installation simply has no file of that name. VCDS still reads
//! them, because `UDS_EV/ReDir.rod` says which file stands in. On the reference
//! car that is the body control module: `F19E = EV_BCMMQB`, `F1A2 = 017001`, no
//! `EV_BCMMQB*.rod` in the install, and the table's line
//! `EV_BCMMQB_017,EV_BCMBOSCH_017`.
//!
//! The container is an ordinary `.rod`, opened with the same keys as every
//! other ([`crate::rod::decode_rod_recover`], the project's `rod-keys.json`). Its
//! `[DIR]` section is one line per redirect, `<key>,<glyphs>` and CRLF, and each
//! line is its own table: the decimal key generates the alphabet
//! ([`TableAlphabet::for_key`]) the rest of the line is written in, which reads
//! back as `<source name>,<target name>`.
//!
//! A target of [`NOTHING`] (`EMPTY`) is VCDS saying it has nothing for that
//! unit. That is a definite answer, and a different one from "no file": the
//! installation is not missing a file, it has decided there is no data, and no
//! sibling variant should be guessed in its place.
//!
//! `research/vcds-registry/README.md` §ReDir is the evidence and what was
//! measured on one installation; nothing here assumes those measurements hold
//! for another — a chain is followed (to [`MAX_HOPS`]), and a target the
//! installation does not have is no file rather than an error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::dtc::find_named;
use crate::glyphs::TableAlphabet;
use crate::registry::{SectionRead, read_section};
use crate::rod::IvCache;

/// The redirect table's file name inside a VCDS installation.
pub const REDIRECT_FILE: &str = "ReDir.rod";

/// The section holding the redirect lines.
pub const DIR_SECTION: &str = "DIR";

/// The target VCDS writes for a unit it has no data for.
pub const NOTHING: &str = "EMPTY";

/// How many redirects one name may pass through before it counts as a loop.
///
/// No target is itself redirected in the one installation measured, so one hop
/// is all that was seen; the limit only has to be larger than any real chain
/// and small enough that a cycle ends at once.
pub const MAX_HOPS: usize = 8;

/// Where one redirect points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
	/// A file stem, as the table spells it.
	File(String),
	/// [`NOTHING`]: VCDS has no data for this unit.
	Nothing,
}

/// What a name comes to once its redirects are followed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
	/// A file stem the installation has.
	File(String),
	/// VCDS has no data for the unit ([`NOTHING`]).
	Nothing,
	/// The chain ends at a name the installation has no file for and that is
	/// not redirected either.
	Missing(String),
	/// The chain does not end within [`MAX_HOPS`]: a cycle, or a table this
	/// reader does not understand.
	Loop,
}

/// The redirect table of one installation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redirects {
	/// Upper-cased source → (source as written, target).
	by_source: BTreeMap<String, (String, Target)>,
	/// Lines that did not read as `<key>,<source>,<target>`.
	unread: usize,
}

/// No redirects at all: the lookup by the unit's own name alone.
pub(crate) static NONE: Redirects = Redirects {
	by_source: BTreeMap::new(),
	unread: 0,
};

impl Redirects {
	/// Parse a decoded `[DIR]` section.
	///
	/// A line that does not read is counted ([`Self::unread`]) and skipped,
	/// never guessed at. A plain-TEA section's first eight bytes can be wrong
	/// (a nonzero per-record `product`), which spoils its first line; that line
	/// then fails here like any other.
	pub fn parse(text: &str) -> Self {
		let mut out = Self::default();
		for line in text.split("\r\n").filter(|line| !line.is_empty()) {
			match read_line(line) {
				Some((source, target)) => {
					out.by_source.insert(source.to_ascii_uppercase(), (source, target));
				}
				None => out.unread += 1,
			}
		}
		out
	}

	/// How many redirects the table holds.
	pub fn len(&self) -> usize {
		self.by_source.len()
	}

	pub fn is_empty(&self) -> bool {
		self.by_source.is_empty()
	}

	/// Lines of the section that did not read.
	pub fn unread(&self) -> usize {
		self.unread
	}

	/// Every source name: as written, and upper-cased for comparing.
	pub fn sources(&self) -> impl Iterator<Item = (&str, &str)> {
		self.by_source.iter().map(|(upper, (written, _))| (written.as_str(), upper.as_str()))
	}

	/// Where `source` points, one hop, or `None` when it is not redirected.
	pub fn target(&self, source: &str) -> Option<&Target> {
		self.by_source.get(&source.to_ascii_uppercase()).map(|(_, target)| target)
	}

	/// Follow `source`'s redirects to a file the installation has.
	///
	/// `has_file` answers whether the installation has a file of an upper-cased
	/// stem. A file wins over a redirect of the same name at every hop — the
	/// same choice [`crate::label_files::find_rod_by_odx_variant`] makes for the
	/// unit's own name — so a chain stops at the first name that is a file.
	pub fn resolve(&self, source: &str, has_file: impl Fn(&str) -> bool) -> Resolved {
		let mut at = source.to_ascii_uppercase();
		for _ in 0..MAX_HOPS {
			let Some((_, target)) = self.by_source.get(&at) else {
				return Resolved::Missing(at);
			};
			let name = match target {
				Target::Nothing => return Resolved::Nothing,
				Target::File(name) => name,
			};
			let upper = name.to_ascii_uppercase();
			if has_file(&upper) {
				return Resolved::File(name.clone());
			}
			if !self.by_source.contains_key(&upper) {
				return Resolved::Missing(name.clone());
			}
			at = upper;
		}
		Resolved::Loop
	}
}

/// One `[DIR]` line: the key in plaintext, then `<source>,<target>` in the
/// alphabet the key generates.
fn read_line(line: &str) -> Option<(String, Target)> {
	let (key, glyphs) = line.split_once(',')?;
	let key: u32 = key.parse().ok()?;
	let plain = TableAlphabet::for_key(key).decode(glyphs)?;
	let (source, target) = plain.split_once(',')?;
	if source.is_empty() || target.is_empty() || target.contains(',') {
		return None;
	}
	let target = match target {
		NOTHING => Target::Nothing,
		name => Target::File(name.to_string()),
	};
	Some((source.to_string(), target))
}

/// What opening the redirect table came to.
#[derive(Debug, Clone)]
pub enum RedirectLoad {
	Found {
		file: PathBuf,
		redirects: Redirects,
	},
	/// No `ReDir.rod` in the installation: every unit is looked up by its own
	/// name alone, as before the table was read.
	NoFile,
	/// `[DIR]`'s key is not cached.
	Locked {
		file: PathBuf,
	},
	/// The file is shifted in this installation: only VCDS's runtime key opens it.
	Shifted {
		file: PathBuf,
	},
}

impl RedirectLoad {
	/// The table, or none when it did not open.
	pub fn redirects(&self) -> Option<&Redirects> {
		match self {
			Self::Found { redirects, .. } => Some(redirects),
			_ => None,
		}
	}
}

/// Open the redirect table of a VCDS installation.
///
/// Once per installation and read: the section is a megabyte of zlib and
/// eighteen thousand lines in VCDS 26.3, a few tens of milliseconds, and a
/// caller resolving many units holds the [`Redirects`] rather than opening it
/// per unit. With `crack`, a classic section whose key is not cached is searched
/// for and recorded in `cache`, as for `RM.rod`.
pub fn load_redirects(root: &Path, cache: &mut IvCache, crack: bool) -> RedirectLoad {
	let Some(file) = find_named(root, REDIRECT_FILE) else {
		return RedirectLoad::NoFile;
	};
	match read_section(&file, DIR_SECTION, cache, crack) {
		SectionRead::Text { text, .. } => RedirectLoad::Found {
			redirects: Redirects::parse(&text),
			file,
		},
		SectionRead::Shifted => RedirectLoad::Shifted { file },
		SectionRead::Locked | SectionRead::Absent => RedirectLoad::Locked { file },
	}
}

#[cfg(test)]
pub(crate) mod testkit {
	use crate::glyphs::TableAlphabet;

	/// One `[DIR]` line as VCDS writes it: the key in plaintext, the names in
	/// the key's alphabet. The names in the tests are invented.
	pub(crate) fn line(key: u32, source: &str, target: &str) -> String {
		format!("{key:06},{}", TableAlphabet::for_key(key).encipher(&format!("{source},{target}")))
	}

	/// A `[DIR]` section's text.
	pub(crate) fn table(lines: &[(u32, &str, &str)]) -> String {
		lines.iter().map(|(k, s, t)| line(*k, s, t) + "\r\n").collect()
	}
}

#[cfg(test)]
mod tests {
	use super::testkit::{line, table};
	use super::*;
	use crate::rod::testkit::{named, section};

	#[test]
	fn a_line_reads_through_the_alphabet_its_own_key_generates() {
		let text = table(&[
			(512_301, "EV_InventedMQB_017", "EV_InventedBOSCH_017"),
			(77, "EV_Other_003_SK37", NOTHING),
		]);
		assert!(!text.contains("EV_Invented"), "the fixture really is enciphered: {text}");
		let redirects = Redirects::parse(&text);
		assert_eq!(redirects.len(), 2);
		assert_eq!(redirects.unread(), 0);
		assert_eq!(
			redirects.target("ev_inventedmqb_017"),
			Some(&Target::File("EV_InventedBOSCH_017".into())),
			"looked up case-insensitively, kept as written"
		);
		assert_eq!(redirects.target("EV_Other_003_SK37"), Some(&Target::Nothing));
		assert_eq!(redirects.target("EV_Absent"), None);
	}

	#[test]
	fn the_key_is_the_lines_own_and_a_line_that_does_not_read_is_counted() {
		// The same glyphs under the neighbouring key are another text: never the
		// names they were written as.
		let good = line(512_301, "EV_InventedMQB_017", "EV_InventedBOSCH_017");
		let glyphs = good.split_once(',').unwrap().1;
		let wrong = Redirects::parse(&format!("512302,{glyphs}"));
		assert_eq!(wrong.target("EV_InventedMQB_017"), None, "{wrong:?}");
		let redirects = Redirects::parse(&format!("{good}\r\nnot a line\r\n000017\r\n"));
		assert_eq!(redirects.len(), 1, "{redirects:?}");
		assert_eq!(redirects.unread(), 2);
	}

	#[test]
	fn a_chain_is_followed_and_ends_at_a_file_nothing_a_missing_name_or_a_loop() {
		let redirects = Redirects::parse(&table(&[
			(1, "EV_A_001", "EV_B_001"),
			(2, "EV_B_001", "EV_C_001"),
			(3, "EV_D_001", NOTHING),
			(4, "EV_E_001", "EV_NotThere_001"),
			(5, "EV_X_001", "EV_Y_001"),
			(6, "EV_Y_001", "EV_X_001"),
		]));
		let files = ["EV_C_001"];
		let has = |stem: &str| files.contains(&stem);
		assert_eq!(redirects.resolve("EV_A_001", has), Resolved::File("EV_C_001".into()), "two hops");
		assert_eq!(redirects.resolve("EV_D_001", has), Resolved::Nothing);
		assert_eq!(redirects.resolve("EV_E_001", has), Resolved::Missing("EV_NotThere_001".into()));
		assert_eq!(redirects.resolve("EV_X_001", has), Resolved::Loop, "a cycle ends");
		// A target that is a file is taken as the file, even when it is also
		// redirected: the file of a name wins over its redirect.
		let both = |stem: &str| stem == "EV_B_001";
		assert_eq!(redirects.resolve("EV_A_001", both), Resolved::File("EV_B_001".into()));
	}

	/// An installation with a `ReDir.rod` built the way VCDS frames one.
	fn install(name: &str, dir: Vec<u8>) -> tempfile::TempDir {
		let root = tempfile::Builder::new().prefix(name).tempdir().unwrap();
		std::fs::create_dir_all(root.path().join("UDS_EV")).unwrap();
		let mut file = section("CMP", b"invented-header", false, named(REDIRECT_FILE));
		file.extend(dir);
		std::fs::write(root.path().join("UDS_EV").join(REDIRECT_FILE), file).unwrap();
		root
	}

	#[test]
	fn the_table_opens_out_of_its_container() {
		let text = table(&[(9001, "EV_InventedMQB_017", "EV_InventedBOSCH_017")]);
		let root = install("redir-open", section(DIR_SECTION, text.as_bytes(), true, named(REDIRECT_FILE)));
		let mut cache = IvCache::default();
		let RedirectLoad::Found { redirects, file } = load_redirects(root.path(), &mut cache, false) else {
			panic!("the table opens");
		};
		assert!(file.ends_with("UDS_EV/ReDir.rod"));
		assert_eq!(redirects.len(), 1);
	}

	#[test]
	fn a_table_whose_key_is_not_cached_is_locked_and_no_table_is_no_file() {
		let text = table(&[(9001, "EV_InventedMQB_017", "EV_InventedBOSCH_017")]);
		let root = install("redir-locked", section(DIR_SECTION, text.as_bytes(), true, Some([1, 2, 3, 4, 5])));
		let mut cache = IvCache::default();
		assert!(matches!(load_redirects(root.path(), &mut cache, false), RedirectLoad::Locked { .. }));
		let empty = tempfile::tempdir().unwrap();
		assert!(matches!(load_redirects(empty.path(), &mut cache, false), RedirectLoad::NoFile));
	}
}
