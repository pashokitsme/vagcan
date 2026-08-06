//! `vagcan vcds rod` — open a `.rod` container.
//!
//! VW ships ODX label files with every section encrypted, and a section whose
//! `product` field is nonzero cannot be decrypted from the file alone: five
//! bytes of its first-block IV are missing and have to be searched for. The
//! search costs about a minute of every core per section, which is why nothing
//! in the live path runs it — the live path reads the *answer* out of
//! `catalogs/rod-iv-cache.json`. This command is where that answer comes from.
//!
//! Was the `vag-rod` binary. With `run_crack` off it reads the cache and reports
//! what it could not open rather than pretending the section is empty.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

use vag_data::rod::{IvCache, RodStatus, decode_rod_recover};

pub fn run(path: &str, run_crack: bool, cache: Option<&str>, dump: Option<&str>) -> Result<()> {
	let data = std::fs::read(path).with_context(|| format!("reading {path:?}"))?;
	let file_name = Path::new(path)
		.file_name()
		.map(|s| s.to_string_lossy().into_owned())
		.unwrap_or_else(|| path.to_string());
	let cache_path = cache.map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("{path}.ivcache.json")));

	let mut cache_data = IvCache::load(&cache_path);
	// Decoding is milliseconds; recovering a key that is not cached yet is about
	// three minutes per blocked section, and it happens inside this one call.
	// Below the spinner's own threshold nothing is drawn, so the fast path stays
	// silent and the slow one stops looking like a hang.
	let sections = {
		let _spinner = crate::progress::Spinner::new(match run_crack {
			true => format!("opening {file_name} — recovering the key of any sealed section"),
			false => format!("opening {file_name}"),
		});
		decode_rod_recover(&data, &file_name, &mut cache_data, run_crack)
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
			"{declined} section(s) marked NO CRIB: no cached key, and this file is one of the \
             40 % that XOR a per-file mask over the first-block IV of every section after \
             [CMP] (research/labels/tttext2.md). Such a section opens fine once its key is known — \
             what it cannot have is a cheap search, because the mask costs a sweep of the \
             deflate anchor against the full candidate space. Not a damaged file."
		);
	}
	Ok(())
}
