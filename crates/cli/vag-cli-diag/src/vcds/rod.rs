//! `vagcan dev vcds rod` — open a `.rod` container.
//!
//! VW ships ODX label files with every section encrypted, and a section whose
//! `product` field is nonzero cannot be decrypted from the file alone: five
//! bytes of its first-block IV are missing and have to be searched for. The
//! search costs minutes per file (none for a file under its own name), which is why nothing
//! in the live path runs it — the live path reads the *answer* out of the
//! project's `rod-keys.json`. This command is where that answer comes from.
//!
//! Was the `vag-rod` binary. With `run_crack` off it reads the cache and reports
//! what it could not open rather than pretending the section is empty.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use crate::render::plural;
use vag_cli_core::marks::facts;
use vag_cli_core::verbosity::{self, NOTES, WIRE};
use vag_data_labels::rod::{IvCache, RodSection, RodStatus, Search, decode_rod_with, decode_rod_with_mask};

/// The key cache for `path`: `cache` when given, `<path>.ivcache.json` beside it otherwise.
fn cache_path(path: &str, cache: Option<&str>) -> PathBuf {
	cache.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("{path}.ivcache.json")))
}

/// The file's own name, which the name-derived IV is built from — the last path
/// component, or the whole path when it has none.
fn file_name_of(path: &str) -> String {
	Path::new(path)
		.file_name()
		.map(|s| s.to_string_lossy().into_owned())
		.unwrap_or_else(|| path.to_string())
}

/// Write each decoded section to `dir/<tag>.bin`, Latin-1 out as it was read in
/// (`--dump`). Shared by the key-search path and the `--mask` path.
fn write_dump(dir: &str, sections: &[RodSection]) -> Result<()> {
	std::fs::create_dir_all(dir).with_context(|| format!("creating the dump directory {dir:?}"))?;
	for s in sections {
		if let Some(text) = &s.text {
			// Latin-1 out, matching the way the section was read in: the text
			// tables carry high bytes (umlauts, `°`, `µ`) that are plaintext
			// evidence, and re-encoding them as UTF-8 would change the byte
			// offsets every later tool counts on.
			let bytes: Vec<u8> = text.chars().map(|c| c as u8).collect();
			let out = Path::new(dir).join(format!("{}.bin", s.tag));
			std::fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;
		}
	}
	Ok(())
}

/// Parse the eight-byte IV mask from 16 hex digits (an optional `0x` is allowed).
fn parse_mask(hex: &str) -> Result<[u8; 8]> {
	let hex = hex.strip_prefix("0x").unwrap_or(hex);
	anyhow::ensure!(
		hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
		"the mask must be 16 hex digits (8 bytes), got {hex:?}"
	);
	let mut mask = [0u8; 8];
	for (i, byte) in mask.iter_mut().enumerate() {
		*byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("validated as hex above");
	}
	Ok(mask)
}

/// What one opening read and left behind.
struct Opened {
	sections: Vec<RodSection>,
	/// The keys the cache held before, and after the search and the save.
	before: IvCache,
	after: IvCache,
	file_name: String,
	cache: PathBuf,
}

/// Open `path`'s sections with the cached keys — searching for one not cached
/// yet as far as `search` allows — save the keys, and write each decoded section
/// to `dump` when given. Prints nothing but a spinner: [`run`] prints the
/// listing, and `setup`, which opens files as one of its steps, says only what
/// did not open.
pub fn open(path: &str, search: Search, cache: Option<&str>, dump: Option<&str>) -> Result<Vec<RodSection>> {
	Ok(open_keyed(path, search, cache, dump)?.sections)
}

fn open_keyed(path: &str, search: Search, cache: Option<&str>, dump: Option<&str>) -> Result<Opened> {
	let data = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
	let file_name = file_name_of(path);
	let cache_path = cache_path(path, cache);

	let before = IvCache::load(&cache_path);
	let mut cache_data = before.clone();
	// Decoding is milliseconds, and a file under its own name needs no key. A
	// file under another name costs a search of minutes, a shifted one hours,
	// and either happens inside this one call.
	// Below the spinner's own threshold nothing is drawn, so the fast path stays
	// silent and the slow one stops looking like a hang.
	let sections = {
		let _spinner = crate::progress::Spinner::new(match search {
			Search::Off => format!("opening {file_name}"),
			Search::Classic | Search::All => format!("opening {file_name} — recovering the key of any sealed section"),
		});
		decode_rod_with(&data, &file_name, &mut cache_data, search)
	};
	if let Err(e) = cache_data.save(&cache_path) {
		vag_cli_core::say!("warning: could not write the key cache {}: {e}", cache_path.display());
	}

	if let Some(dir) = dump {
		write_dump(dir, &sections)?;
	}
	Ok(Opened {
		sections,
		before,
		after: cache_data,
		file_name,
		cache: cache_path,
	})
}

/// Why a section did not open, in words — `None` for one that did.
pub fn why_shut(section: &RodSection) -> Option<&'static str> {
	match (&section.status, &section.text) {
		(RodStatus::SearchDeclined, _) => Some("it is shifted, which only VCDS's runtime key opens"),
		(RodStatus::Undecodable, _) | (_, None) => Some("the search for its key found nothing"),
		_ => None,
	}
}

/// Whether a section opened, as `rod` and `labels --odx` print it on stdout: `open` or
/// `sealed`. How it opened, or why not, is [`cipher`], at `-v`.
pub fn state(section: &RodSection) -> &'static str {
	match section.text {
		Some(_) => "open",
		None => "sealed",
	}
}

/// How a section decoded, in one word, as `rod` and `labels --odx` say it at `-v`: `tea`
/// (decrypted), `zlib` (decrypted and inflated), `no key` (sealed: none cached, or the
/// search found none), `no crib` (sealed: no key can be searched for).
pub fn cipher(section: &RodSection) -> &'static str {
	match (&section.status, &section.text) {
		// Two different pieces of news, and printing one word for both is what left
		// `TTTEXT2.ROD` looking unreadable for four writeups.
		(RodStatus::SearchDeclined, _) => "no crib",
		(RodStatus::Undecodable, _) | (_, None) => "no key",
		(RodStatus::Tea, Some(_)) => "tea",
		(RodStatus::Zlib, Some(_)) => "zlib",
	}
}

/// The columns every listing of sections starts with — `rod`'s, and `labels --odx`'s
/// after its `file` — so the two read alike. `bytes` is right-aligned.
pub const SECTION_COLUMNS: [&str; 3] = ["section", "state", "bytes"];

/// What `no crib` means, said once wherever a section is one.
pub const NO_CRIB: &str = "the file XORs a per-file mask over the first-block IV after [CMP], so its key cannot be searched for; \
                           not damaged, it opens once the key is known";

/// A section's key, as the cache holds it after the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Key {
	/// None in the cache: the section needs none, or none was found.
	None,
	Cached([u8; 5]),
	/// Not in the cache before this run.
	Recovered([u8; 5]),
}

fn key_of(before: &IvCache, after: &IvCache, file: &str, tag: &str) -> Key {
	match (before.get(file, tag), after.get(file, tag)) {
		(_, None) => Key::None,
		(Some(_), Some(key)) => Key::Cached(key),
		(None, Some(key)) => Key::Recovered(key),
	}
}

/// What [`run`] prints on stdout: grouped records, rustc's shape — the file and a summary
/// line of facts, then each section as a heading line of its state, size and key, with the
/// start of its text indented under it.
fn render(path: &str, cache: &Path, sections: &[RodSection], keys: &[Key]) -> String {
	let open = sections.iter().filter(|s| s.text.is_some()).count();
	let recovered = keys.iter().filter(|k| matches!(k, Key::Recovered(_))).count();
	let summary = facts([
		format!("{open} of {} {} open", sections.len(), plural(sections.len(), "section")),
		format!("{recovered} {} recovered", plural(recovered, "key")),
		format!("keys in {}", vag_cli_core::datadir::tilde(cache)),
	]);
	format!("{path}\n{summary}\n{}", section_rows(sections, keys))
}

/// What `--mask` prints on stdout: the same per-section rows as [`render`], under
/// a summary that names the mask instead of a key cache. The mask opens the file
/// outright, so there are no per-section keys to show.
fn render_masked(path: &str, mask: [u8; 8], sections: &[RodSection]) -> String {
	let open = sections.iter().filter(|s| s.text.is_some()).count();
	let summary = facts([
		format!("{open} of {} {} open", sections.len(), plural(sections.len(), "section")),
		format!("mask {}", verbosity::hex(&mask)),
	]);
	let keys = vec![Key::None; sections.len()];
	format!("{path}\n{summary}\n{}", section_rows(sections, &keys))
}

/// One heading row per section — state, size and (where there is one) key — with the
/// start of its text indented under it. The body shared by [`render`] and [`render_masked`].
fn section_rows(sections: &[RodSection], keys: &[Key]) -> String {
	let mut out = String::new();
	for (s, key) in sections.iter().zip(keys) {
		let preview: String = s
			.text
			.as_deref()
			.unwrap_or_default()
			.chars()
			.take(60)
			.map(|c| if c.is_control() { '.' } else { c })
			.collect();
		let key = match key {
			Key::None => String::new(),
			Key::Cached(k) => format!("key {}", verbosity::hex(k)),
			Key::Recovered(k) => format!("key {}, recovered", verbosity::hex(k)),
		};
		let bytes = match &s.text {
			Some(text) => format!("{} {}", text.len(), plural(text.len(), "byte")),
			None => String::new(),
		};
		out.push('\n');
		out.push_str(&format!("{}  {}\n", s.tag, facts([state(s).to_string(), bytes, key])));
		if !preview.trim().is_empty() {
			out.push_str(&format!("  {preview}\n"));
		}
	}
	out
}

/// What [`run`] says on stderr, by level.
fn notes(opened: &Opened, run_crack: bool, dump: Option<&str>, keys: &[Key]) -> Vec<(u8, String)> {
	let mut notes = vec![(
		NOTES,
		format!(
			"{} {} in {} before this run",
			opened.before.len(),
			plural(opened.before.len(), "key"),
			opened.cache.display()
		),
	)];
	// `no key` only: a `no crib` section has no key to search for.
	let searchable = opened.sections.iter().filter(|s| cipher(s) == "no key").count();
	if searchable > 0 && !run_crack {
		notes.push((
			NOTES,
			format!("{searchable} sealed: without --no-crack the key is searched for, a search of minutes per file"),
		));
	}
	if searchable > 0 && run_crack {
		notes.push((NOTES, format!("{searchable} sealed: the key search found nothing")));
	}
	for s in &opened.sections {
		let line = match cipher(s) {
			"no crib" => format!("[{}] no crib: {NO_CRIB}", s.tag),
			cipher => format!("[{}] {cipher}", s.tag),
		};
		notes.push((NOTES, line));
	}
	for (s, key) in opened.sections.iter().zip(keys) {
		let key = match key {
			Key::None => "no key in the cache".to_string(),
			Key::Cached(k) => format!("key {} from the cache", verbosity::hex(k)),
			Key::Recovered(k) => format!("key {} recovered this run", verbosity::hex(k)),
		};
		notes.push((WIRE, format!("{} [{}] {:?}, {key}", opened.file_name, s.tag, s.status)));
	}
	if let Some(dir) = dump {
		notes.push((0, wrote(&opened.sections, dir)));
	}
	notes
}

/// The `--dump` line: how many sections went to `dir`, and the file each is in.
fn wrote(sections: &[RodSection], dir: &str) -> String {
	let files: Vec<String> = sections.iter().filter(|s| s.text.is_some()).map(|s| format!("{}.bin", s.tag)).collect();
	let mut line = format!("wrote {} {} to {dir}", files.len(), plural(files.len(), "section"));
	if !files.is_empty() {
		line.push_str(&format!(": {}", files.join(", ")));
	}
	line
}

/// `vagcan dev vcds rod`: open a file and list its sections.
///
/// With `mask`, a shifted file is opened by its known eight-byte IV mask instead of by a
/// key search: no cache is touched, and `run_crack`/`cache` are refused beside it (clap).
pub fn run(path: &str, run_crack: bool, cache: Option<&str>, dump: Option<&str>, mask: Option<&str>) -> Result<()> {
	if let Some(mask) = mask {
		return run_masked(path, parse_mask(mask)?, dump);
	}
	let search = match run_crack {
		true => Search::All,
		false => Search::Off,
	};
	let opened = open_keyed(path, search, cache, dump)?;
	let keys: Vec<Key> = opened
		.sections
		.iter()
		.map(|s| key_of(&opened.before, &opened.after, &opened.file_name, &s.tag))
		.collect();
	vag_cli_core::out!("{}", render(path, &opened.cache, &opened.sections, &keys));
	for (level, text) in notes(&opened, run_crack, dump, &keys) {
		verbosity::say(level, &text);
	}
	Ok(())
}

/// `vagcan dev vcds rod --mask`: open a shifted file by its known IV mask.
fn run_masked(path: &str, mask: [u8; 8], dump: Option<&str>) -> Result<()> {
	let data = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
	let file_name = file_name_of(path);
	let sections = decode_rod_with_mask(&data, &file_name, mask);
	if let Some(dir) = dump {
		write_dump(dir, &sections)?;
	}
	vag_cli_core::out!("{}", render_masked(path, mask, &sections));
	for (level, text) in mask_notes(&file_name, &sections, dump) {
		verbosity::say(level, &text);
	}
	Ok(())
}

/// What [`run_masked`] says on stderr, by level: whether each section opened under the
/// mask, and the per-section cipher at `-vv`. A sealed section means the wrong mask (or a
/// file that is not shifted), not a key that could be searched for.
fn mask_notes(file: &str, sections: &[RodSection], dump: Option<&str>) -> Vec<(u8, String)> {
	let mut notes = Vec::new();
	let sealed = sections.iter().filter(|s| s.text.is_none()).count();
	if sealed > 0 {
		notes.push((
			NOTES,
			format!("{sealed} sealed under this mask — check the mask, the file name, and that the file is shifted"),
		));
	}
	for s in sections {
		notes.push((WIRE, format!("{file} [{}] {:?}", s.tag, s.status)));
	}
	if let Some(dir) = dump {
		notes.push((0, wrote(sections, dir)));
	}
	notes
}

#[cfg(test)]
mod tests {
	use super::*;

	fn section(tag: &str, status: RodStatus, text: Option<&str>) -> RodSection {
		RodSection {
			tag: tag.into(),
			status,
			text: text.map(str::to_owned),
		}
	}

	#[test]
	fn a_mask_is_sixteen_hex_digits_with_an_optional_prefix() {
		let want = [0x5e, 0xe6, 0x10, 0x51, 0x77, 0x19, 0x40, 0x8c];
		assert_eq!(parse_mask("5ee610517719408c").unwrap(), want);
		assert_eq!(parse_mask("0x5EE610517719408C").unwrap(), want);
		// too short, too long, a non-hex digit, the prefix alone.
		for bad in ["", "5ee6", "5ee610517719408c00", "5ee610517719zz8c", "0x"] {
			assert!(parse_mask(bad).is_err(), "{bad:?}");
		}
	}

	/// `--mask` names the mask where the key path names a cache, and shows no per-section
	/// key, because the mask opens the file outright. A sealed section is the wrong mask.
	#[test]
	fn the_masked_listing_names_the_mask_and_shows_no_key_column() {
		let mask = [0x5e, 0xe6, 0x10, 0x51, 0x77, 0x19, 0x40, 0x8c];
		let sections = [
			section("CMP", RodStatus::Tea, Some("12345678,AB")),
			section("MWB", RodStatus::Zlib, Some("000003,abc")),
			section("DTC", RodStatus::Undecodable, None),
		];
		let text = render_masked("X.rod", mask, &sections);
		assert_eq!(
			text,
			"X.rod\n\
			 2 of 3 sections open · mask 5E E6 10 51 77 19 40 8C\n\
			 \n\
			 CMP  open · 11 bytes\n\
			 \x20 12345678,AB\n\
			 \n\
			 MWB  open · 10 bytes\n\
			 \x20 000003,abc\n\
			 \n\
			 DTC  sealed\n"
		);
	}

	#[test]
	fn the_listing_is_the_file_its_cache_one_row_per_section_and_a_summary() {
		let sections = [
			section("CMP", RodStatus::Tea, Some("9377\u{1}4")),
			section("TXT", RodStatus::Zlib, Some("000003,abc")),
			section("INC", RodStatus::Undecodable, None),
			section("MWB", RodStatus::SearchDeclined, None),
		];
		let keys = [Key::None, Key::Recovered([0x18, 0xBE, 0xF0, 0x88, 0x21]), Key::None, Key::None];
		let text = render("X.rod", Path::new("/k/rod-keys.json"), &sections, &keys);
		assert_eq!(
			text,
			"X.rod\n\
			 2 of 4 sections open · 1 key recovered · keys in /k/rod-keys.json\n\
			 \n\
			 CMP  open · 6 bytes\n\
			 \x20 9377.4\n\
			 \n\
			 TXT  open · 10 bytes · key 18 BE F0 88 21, recovered\n\
			 \x20 000003,abc\n\
			 \n\
			 INC  sealed\n\
			 \n\
			 MWB  sealed\n"
		);
		let one = render("X.rod", Path::new("/k"), &sections[..1], &[Key::None]);
		assert!(one.starts_with("X.rod\n1 of 1 section open · 0 keys recovered · keys in /k\n"), "{one}");
	}

	/// The cipher is a note: `tea`/`zlib` for an open section, `no key`/`no crib` for a
	/// sealed one. Nothing points into the checkout, which an installed tool has not got.
	#[test]
	fn the_cipher_of_each_section_is_said_at_v_and_names_no_checkout_path() {
		let sections = vec![
			section("CMP", RodStatus::Tea, Some("x")),
			section("TXT", RodStatus::Zlib, Some("y")),
			section("INC", RodStatus::Undecodable, None),
			section("MWB", RodStatus::SearchDeclined, None),
		];
		let mut before = IvCache::default();
		before.insert("X.rod", "TXT", [1; 5]);
		let opened = Opened {
			sections,
			after: before.clone(),
			before,
			file_name: "X.rod".into(),
			cache: PathBuf::from("/k/rod-keys.json"),
		};
		let keys = [Key::None, Key::Cached([1; 5]), Key::None, Key::None];
		let notes = notes(&opened, false, Some("out"), &keys);
		let at = |level: u8| notes.iter().filter(|(l, _)| *l == level).map(|(_, t)| t.as_str()).collect::<Vec<_>>();
		assert_eq!(
			at(NOTES),
			vec![
				"1 key in /k/rod-keys.json before this run",
				"1 sealed: without --no-crack the key is searched for, a search of minutes per file",
				"[CMP] tea",
				"[TXT] zlib",
				"[INC] no key",
				&format!("[MWB] no crib: {NO_CRIB}"),
			]
		);
		assert_eq!(at(0), vec!["wrote 2 sections to out: CMP.bin, TXT.bin"]);
		assert!(notes.iter().all(|(_, t)| !t.contains(".archive")), "{notes:?}");
		assert_eq!(wrote(&opened.sections[2..], "out"), "wrote 0 sections to out");
		assert_eq!(wrote(&opened.sections[..1], "out"), "wrote 1 section to out: CMP.bin");
	}

	#[test]
	fn a_key_is_new_only_when_the_cache_did_not_hold_it_before() {
		let mut before = IvCache::default();
		before.insert("F.rod", "A", [1; 5]);
		let mut after = before.clone();
		after.insert("F.rod", "B", [2; 5]);
		assert_eq!(key_of(&before, &after, "F.rod", "A"), Key::Cached([1; 5]));
		assert_eq!(key_of(&before, &after, "F.rod", "B"), Key::Recovered([2; 5]));
		assert_eq!(key_of(&before, &after, "F.rod", "C"), Key::None);
	}
}
