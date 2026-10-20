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

use vag_data_labels::rod::{IvCache, RodSection, RodStatus, Search, decode_rod_with};

/// Open `path`'s sections by the file's name and the cached keys — searching for
/// what they do not open as far as `search` allows — save the keys, and write each decoded section
/// to `dump` when given. Prints nothing but a spinner: [`run`] prints the
/// listing, and `setup`, which opens files as one of its steps, says only what
/// did not open.
pub fn open(path: &str, search: Search, cache: Option<&str>, dump: Option<&str>) -> Result<Vec<RodSection>> {
	let data = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
	let file_name = Path::new(path)
		.file_name()
		.map(|s| s.to_string_lossy().into_owned())
		.unwrap_or_else(|| path.to_string());
	let cache_path = cache_path_of(path, cache);

	let mut cache_data = IvCache::load(&cache_path);
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
		eprintln!("warning: could not write the key cache {}: {e}", cache_path.display());
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
	Ok(sections)
}

/// The key cache `open` reads: the one named, else one beside the file.
fn cache_path_of(path: &str, cache: Option<&str>) -> PathBuf {
	cache.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("{path}.ivcache.json")))
}

/// Why a section did not open, in words — `None` for one that did.
pub fn why_shut(section: &RodSection) -> Option<&'static str> {
	match (&section.status, &section.text) {
		(RodStatus::SearchDeclined, _) => Some("it is shifted: its mask is the VCDS interface's answer, and no key cached opens it"),
		(RodStatus::Undecodable, _) | (_, None) => Some("the search for its key found nothing"),
		_ => None,
	}
}

/// `vagcan dev vcds rod`: open a file and list its sections.
pub fn run(path: &str, run_crack: bool, cache: Option<&str>, dump: Option<&str>) -> Result<()> {
	let search = match run_crack {
		true => Search::All,
		false => Search::Off,
	};
	let sections = open(path, search, cache, dump)?;
	println!("{path}: {} section(s)", sections.len());
	let mut ok = 0usize;
	for s in &sections {
		let (status, size, preview) = match (&s.status, &s.text) {
			// Two different pieces of news, and printing one word for both is
			// what left `TTTEXT2.ROD` looking unreadable for four writeups.
			(RodStatus::SearchDeclined, _) => ("NO CRIB".to_string(), 0usize, String::new()),
			(RodStatus::Undecodable, _) | (_, None) => ("UNDECODABLE".to_string(), 0usize, String::new()),
			(st, Some(t)) => {
				ok += 1;
				let label = match st {
					RodStatus::Tea => "tea",
					RodStatus::Zlib => "zlib",
					RodStatus::Undecodable | RodStatus::SearchDeclined => unreachable!(),
				};
				let preview: String = t.chars().take(60).map(|c| if c.is_control() { '.' } else { c }).collect();
				(label.to_string(), t.len(), preview)
			}
		};
		println!("  [{:<8}] {:>4}  {:>9} bytes  {}", s.tag, status, size, preview);
	}
	println!("decoded {ok}/{} section(s)", sections.len());
	let declined = sections.iter().filter(|s| s.status == RodStatus::SearchDeclined).count();
	if declined > 0 {
		println!(
			"{declined} section(s) marked NO CRIB: no cached key opens it, and this file is one of the \
             40 % that XOR a per-file mask over the first-block IV of every section after \
             [CMP] (.archive/research/labels/tttext2.md). Such a section opens fine once its key is known — \
             what it cannot have is a cheap search, because the mask costs a sweep of the \
             deflate anchor against the full candidate space. Not a damaged file."
		);
		// A key cached for a section that it does not open is another build's
		// under the same name, and it is what keeps the search from running.
		let held = IvCache::load(&cache_path_of(path, cache));
		let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
		for s in sections.iter().filter(|s| s.status == RodStatus::SearchDeclined) {
			if held.get(&name, &s.tag).is_some() {
				println!(
					"[{}]: the key cached for it does not open it — another build's. Delete the entry \"{name}\\t{}\" from {}, \
                     or run without --no-crack to search past it.",
					s.tag,
					s.tag,
					cache_path_of(path, cache).display()
				);
			}
		}
	}
	Ok(())
}
