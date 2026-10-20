//! `vagcan dev vcds rod` — open a `.rod` container.
//!
//! VW ships ODX label files with every section encrypted, and a section whose
//! `product` field is nonzero cannot be decrypted from the file alone: five
//! bytes of its first-block IV are missing and have to be searched for. The
//! search costs about a minute of every core per section, which is why nothing
//! in the live path runs it — the live path reads the *answer* out of the
//! project's `rod-keys.json`. This command is where that answer comes from.
//!
//! Was the `vag-rod` binary. With `run_crack` off it reads the cache and reports
//! what it could not open rather than pretending the section is empty.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use vag_cli_core::table::{self, Table};
use vag_cli_core::verbosity::{self, NOTES, WIRE};
use vag_data_labels::rod::{IvCache, RodSection, RodStatus, Search, decode_rod_with};

/// The key cache for `path`: `cache` when given, `<path>.ivcache.json` beside it otherwise.
fn cache_path(path: &str, cache: Option<&str>) -> PathBuf {
	cache.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("{path}.ivcache.json")))
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
	let file_name = Path::new(path)
		.file_name()
		.map(|s| s.to_string_lossy().into_owned())
		.unwrap_or_else(|| path.to_string());
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
		std::fs::create_dir_all(dir).with_context(|| format!("creating the dump directory {dir:?}"))?;
		for s in &sections {
			if let Some(text) = &s.text {
				// Latin-1 out, matching the way the section was read in: the
				// text tables carry high bytes (umlauts, `°`, `µ`) that are
				// plaintext evidence, and re-encoding them as UTF-8 would
				// change the byte offsets every later tool counts on.
				let bytes: Vec<u8> = text.chars().map(|c| c as u8).collect();
				let out = Path::new(dir).join(format!("{}.bin", s.tag));
				std::fs::write(&out, &bytes).with_context(|| format!("writing {}", out.display()))?;
			}
		}
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

/// How a section decoded, in one word, as `rod` and `labels --odx` print it: `tea`
/// (decrypted), `zlib` (decrypted and inflated), `sealed` (no key), `no crib` (no key,
/// and none can be searched for).
pub fn state(section: &RodSection) -> &'static str {
	match (&section.status, &section.text) {
		// Two different pieces of news, and printing one word for both is what left
		// `TTTEXT2.ROD` looking unreadable for four writeups.
		(RodStatus::SearchDeclined, _) => "no crib",
		(RodStatus::Undecodable, _) | (_, None) => "sealed",
		(RodStatus::Tea, Some(_)) => "tea",
		(RodStatus::Zlib, Some(_)) => "zlib",
	}
}

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

/// What [`run`] prints on stdout: the file and its cache, a table of sections, the
/// summary line.
fn render(path: &str, cache: &Path, sections: &[RodSection], keys: &[Key]) -> String {
	let mut out = table::pairs(&[("file", path.to_string()), ("cache", cache.display().to_string())]);
	out.push('\n');
	let mut rows = Table::new(&["tag", "state", "bytes", "key", "preview"]).right(2);
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
			Key::Cached(k) => verbosity::hex(k),
			Key::Recovered(k) => format!("{} new", verbosity::hex(k)),
		};
		let bytes = s.text.as_ref().map_or(0, |t| t.len());
		rows.row(vec![s.tag.clone(), state(s).to_string(), bytes.to_string(), key, preview]);
	}
	out.push_str(&rows.render(table::terminal_width()));
	let open = sections.iter().filter(|s| s.text.is_some()).count();
	let recovered = keys.iter().filter(|k| matches!(k, Key::Recovered(_))).count();
	let mut summary = format!("{open}/{} sections open · {recovered} keys recovered", sections.len());
	let declined = sections.iter().filter(|s| s.status == RodStatus::SearchDeclined).count();
	if declined > 0 {
		summary.push_str(&format!(" · {declined} no crib"));
	}
	out.push_str(&summary);
	out.push('\n');
	out
}

/// What [`run`] says on stderr, by level.
fn notes(opened: &Opened, run_crack: bool, dump: Option<&str>, keys: &[Key]) -> Vec<(u8, String)> {
	let mut notes = vec![(
		NOTES,
		format!("{} keys in {} before this run", opened.before.len(), opened.cache.display()),
	)];
	let sealed = opened.sections.iter().filter(|s| state(s) == "sealed").count();
	if sealed > 0 && !run_crack {
		notes.push((
			NOTES,
			format!("{sealed} sealed: without --no-crack the key is searched for, about 3 min per section"),
		));
	}
	if sealed > 0 && run_crack {
		notes.push((NOTES, format!("{sealed} sealed: the key search found nothing")));
	}
	for s in opened.sections.iter().filter(|s| s.status == RodStatus::SearchDeclined) {
		notes.push((
			NOTES,
			format!(
				"[{}] no crib: the file XORs a per-file mask over the first-block IV after [CMP], so its key cannot be searched for; \
				 not damaged, it opens once the key is known (.archive/research/labels/tttext2.md)",
				s.tag
			),
		));
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
		let written = opened.sections.iter().filter(|s| s.text.is_some()).count();
		notes.push((0, format!("wrote {written} sections to {dir}/<TAG>.bin")));
	}
	notes
}

/// `vagcan dev vcds rod`: open a file and list its sections.
pub fn run(path: &str, run_crack: bool, cache: Option<&str>, dump: Option<&str>) -> Result<()> {
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
			"file   X.rod\n\
			 cache  /k/rod-keys.json\n\
			 \n\
			 tag  state    bytes  key                 preview\n\
			 CMP  tea          6                      9377.4\n\
			 TXT  zlib        10  18 BE F0 88 21 new  000003,abc\n\
			 INC  sealed       0\n\
			 MWB  no crib      0\n\
			 2/4 sections open · 1 keys recovered · 1 no crib\n"
		);
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
