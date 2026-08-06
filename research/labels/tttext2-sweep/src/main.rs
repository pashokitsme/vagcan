//! `TTTEXT2.ROD` — the 60-anchor, full-space key sweep, with a log.
//!
//! `research/labels/tttext2.md` §4.2 prescribes it: the file is *shifted* (a
//! per-file XOR over the first-block IV, §3.3), so deflate byte 0 — the
//! searcher's anchor — is destroyed, the multiplicative reduction of `iv[3..8]`
//! is invalid, and the only route is every legal anchor against the full 2⁴⁰
//! space. The shipped `vagcan vcds rod --features rod-crack` already does
//! exactly that, in one silent call; what it does not do is say how far it got,
//! which is the difference between a negative result and a timeout dressed as
//! one.
//!
//! So this driver does not re-implement anything. It `include!`s the shipped
//! decoder and searcher verbatim and drives them one anchor at a time, printing
//! and check-pointing after each. If the shipped searcher is wrong, this is
//! wrong in the same way — which is the point.
//!
//! ```text
//! anchors <dir>            deflate byte 0 over every *classic* section — the
//!                          prior that orders the sweep
//! probe <file>             what the container holds, and classic vs shifted
//! sweep <file> <tag>       the sweep itself; --order lists anchors to try first
//! ```

use std::io::Write as _;
use std::time::Instant;

// The shipped TEA and `.rod` code, compiled into this crate as ordinary
// modules pointed at the real files. Not a copy and not a text transform: the
// searcher `mod rod` declares (`rod/crack.rs`) resolves natively beside it, and
// `include_bytes!("rod_mt.bin")` resolves next to its own source, as it should.
// If the shipped searcher is wrong, this driver is wrong in the same way —
// which is the point.
#[path = "../../../../crates/vag-data/src/tea.rs"]
mod tea;

#[path = "../../../../crates/vag-data/src/rod/mod.rs"]
mod rod;

mod raw;

fn read(path: &str) -> Vec<u8> {
	std::fs::read(path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

/// `plaintext[0..3]` of a section's first block under the tag-derived IV.
fn model_prefix(tag: &str, cipher: &[u8]) -> [u8; 3] {
	let t = raw::first_block(cipher);
	let iv = raw::model_iv(tag.as_bytes());
	[t[0] ^ iv[0], t[1] ^ iv[1], t[2] ^ iv[2]]
}

// ---------------------------------------------------------------- `anchors`

/// Histogram of deflate byte 0 over every *classic* compressed section in a
/// directory.
///
/// The sweep has to try all 60 anchors, but nothing says it has to try them in
/// numeric order, and the corpus knows which ones VCDS's compressor actually
/// emits: for a classic section `plaintext[2]` is exact, free, and is precisely
/// the byte a shifted section hides. Ordering by that frequency does not change
/// what the sweep covers — only when it is likely to stop.
fn cmd_anchors(dir: &str) {
	let mut hist = [0u64; 256];
	// Sections at least this big get their own histogram: the anchor encodes
	// HLIT, which tracks how many distinct literals the block used, so a 4 MB
	// text section is a better prior for another 4 MB text section than a
	// 300-byte one is.
	const BIG: usize = 1 << 20;
	let mut big = [0u64; 256];
	let (mut files, mut classic, mut shifted) = (0u64, 0u64, 0u64);
	// The largest classic sections, kept whole: with only a handful of them the
	// histogram is too coarse to read, and the question "what does VCDS's
	// compressor emit for a multi-megabyte text table" is answered by looking
	// at the multi-megabyte text tables one by one.
	let mut biggest: Vec<(usize, u8, String, String)> = Vec::new();
	let mut entries: Vec<_> = std::fs::read_dir(dir)
		.unwrap_or_else(|e| panic!("reading {dir}: {e}"))
		.filter_map(|e| e.ok())
		.map(|e| e.path())
		.collect();
	entries.sort();
	for p in entries {
		if !p.is_file() {
			continue;
		}
		let Ok(data) = std::fs::read(&p) else { continue };
		files += 1;
		for s in raw::framed(&data) {
			if !s.compressed || s.cipher.len() < 8 {
				continue;
			}
			let pre = model_prefix(&s.tag, &s.cipher);
			if pre[0] == 0x78 && pre[1] == 0xda {
				classic += 1;
				hist[pre[2] as usize] += 1;
				if s.plainlen >= BIG {
					big[pre[2] as usize] += 1;
				}
				if s.plainlen >= 65_536 {
					let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
					biggest.push((s.plainlen, pre[2], name, s.tag.clone()));
				}
			} else {
				shifted += 1;
			}
		}
	}
	eprintln!("{files} files, {classic} classic compressed sections, {shifted} shifted");
	let mut order: Vec<usize> = (0..256).filter(|&i| hist[i] > 0).collect();
	order.sort_by_key(|&i| std::cmp::Reverse(hist[i]));
	println!("# anchor  count  count(plain >= 1 MiB)  BFINAL  BTYPE  HLIT");
	for i in order {
		let a = i as u8;
		println!(
			"0x{a:02x} {:>8} {:>8}   {}      {}    {}",
			hist[i],
			big[i],
			a & 1,
			(a >> 1) & 3,
			257 + (a >> 3) as u16
		);
	}
	biggest.sort_by_key(|(n, ..)| std::cmp::Reverse(*n));
	println!("\n# the largest classic sections, and the anchor each one actually uses");
	println!("#     plain  anchor  BFINAL BTYPE HLIT  file [tag]");
	for (n, a, file, tag) in biggest.iter().take(25) {
		println!(
			"{n:>10}   0x{a:02x}      {}     {}   {:>3}  {file} [{tag}]",
			a & 1,
			(a >> 1) & 3,
			257 + (a >> 3) as u16
		);
	}

	// The line the sweep consumes: every legal anchor, most-frequent first,
	// with the ones the corpus never emits at the back rather than dropped.
	let anchors = raw::anchors();
	let mut ranked = anchors.clone();
	ranked.sort_by_key(|&a| (std::cmp::Reverse(big[a as usize]), std::cmp::Reverse(hist[a as usize]), a));
	println!("\norder={}", ranked.iter().map(|a| format!("{a:#04x}")).collect::<Vec<_>>().join(","));
	assert_eq!(anchors.len(), 60);
}

// ----------------------------------------------------------------- `shifts`

/// Every shifted file's `D[0:2]`, with what else the file offers.
///
/// `tttext2.md` §3.3 sampled 349 files and found `D` uniform, from which it
/// concluded — correctly — that no *rule* generates it. That leaves a different
/// question the sample was too small to ask: does `D` **repeat**? The mask is a
/// runtime global, and a runtime global is not obliged to be redrawn per file.
/// If `TTTEXT2.ROD`'s `dd c0` also belongs to a file whose text sections pin its
/// anchor (§3.4), that file is a 35-minute crack and — if `D` is shared in full,
/// not just in its first two bytes — it hands over `iv[3..8]` outright.
///
/// The columns are what decides whether such a twin is cheap: `text` counts
/// uncompressed sections that look like `<6-digit id>,<2-char code>` records
/// after `[CMP]`, which is exactly what §3.4 narrows `D[2]` with, and `zsmall`
/// is the smallest compressed section, which is what a search would run on.
fn cmd_shifts(dir: &str) {
	let mut entries: Vec<_> = std::fs::read_dir(dir)
		.unwrap_or_else(|e| panic!("reading {dir}: {e}"))
		.filter_map(|e| e.ok())
		.map(|e| e.path())
		.collect();
	entries.sort();
	println!("# D16\tfile\tzsections\tzsmallest\ttextsections");
	for p in entries {
		if !p.is_file() {
			continue;
		}
		let Ok(data) = std::fs::read(&p) else { continue };
		let secs = raw::framed(&data);
		let mut d16: Option<u16> = None;
		let mut zn = 0usize;
		let mut zsmall = usize::MAX;
		let mut text = 0usize;
		for s in &secs {
			if s.cipher.len() < 8 {
				continue;
			}
			let pre = model_prefix(&s.tag, &s.cipher);
			if s.compressed {
				if pre[0] == 0x78 && pre[1] == 0xda {
					continue;
				}
				zn += 1;
				zsmall = zsmall.min(s.cipher.len());
				let d = u16::from_be_bytes([pre[0] ^ 0x78, pre[1] ^ 0xda]);
				// A file whose sections disagree here would break the "one mask
				// per file" model outright, so say so rather than pick one.
				if let Some(prev) = d16
					&& prev != d
				{
					eprintln!("{}: sections disagree on D ({prev:04x} vs {d:04x})", p.display());
				}
				d16 = Some(d);
			} else if s.tag != "CMP" && s.plainlen >= 11 && s.plainlen % 11 == 0 {
				// §3.4's shape: 9 record bytes + CRLF, every record the same
				// width. Cheap and only used to rank candidates.
				text += 1;
			}
		}
		if let Some(d) = d16 {
			println!(
				"{d:04x}\t{}\t{zn}\t{}\t{text}",
				p.file_name().unwrap_or_default().to_string_lossy(),
				if zsmall == usize::MAX { 0 } else { zsmall }
			);
		}
	}
}

// ------------------------------------------------------------------ `probe`

fn cmd_probe(path: &str) {
	let data = read(path);
	println!("{path}: {} bytes", data.len());
	for s in raw::framed(&data) {
		let pre = model_prefix(&s.tag, &s.cipher);
		let kind = if s.compressed { "zlib" } else { "tea " };
		let regime = match (s.compressed, pre[0] == 0x78 && pre[1] == 0xda) {
			(true, true) => "classic (anchor free)".to_string(),
			(true, false) => format!("SHIFTED   D[0:2] = {:02x} {:02x}", pre[0] ^ 0x78, pre[1] ^ 0xda),
			(false, _) => "uncompressed".to_string(),
		};
		println!(
			"  [{:<8}] {kind}  cipher {:>9}  plain {:>9}  model plaintext[0:3] = {:02x} {:02x} {:02x}  {regime}",
			s.tag,
			s.cipher.len(),
			s.plainlen,
			pre[0],
			pre[1],
			pre[2]
		);
	}
}

// ------------------------------------------------------------------ `sweep`

/// One anchor, one full-space search, one line of log.
///
/// Calls the shipped `crack::recover_iv3to8` with `known_anchor = Some(d0)`;
/// on a shifted section that is exactly one search over the full candidate
/// sets, which is what §3.5 says is required.
fn cmd_sweep(path: &str, want_tag: &str, order: Option<&str>, out: &str, all_btypes: bool) {
	let data = read(path);
	let sections = raw::framed(&data);
	let s = sections
		.iter()
		.find(|s| s.tag == want_tag)
		.unwrap_or_else(|| panic!("no [{want_tag}] section in {path}"));
	assert!(s.compressed, "[{want_tag}] is not a compressed section");
	let pre = model_prefix(&s.tag, &s.cipher);
	let shifted = !(pre[0] == 0x78 && pre[1] == 0xda);
	println!(
		"{path} [{}]: cipher {} B, plain {} B, model plaintext[0:3] = {:02x} {:02x} {:02x}, {}",
		s.tag,
		s.cipher.len(),
		s.plainlen,
		pre[0],
		pre[1],
		pre[2],
		if shifted { "SHIFTED" } else { "classic" }
	);

	// The shipped anchor set is the 60 dynamic-Huffman headers, on the stated
	// grounds that "no section in the corpus uses either" stored or fixed
	// Huffman. The census says otherwise — 1,561 of 22,107 classic sections
	// open with a *fixed* block — so `--all-btypes` widens the universe to
	// every first byte a deflate stream can legally have: BTYPE=2 with
	// HLIT ≤ 29 (60), BTYPE=1 where the remaining five bits are compressed
	// data and therefore free (64), and BTYPE=0 where they are padding that
	// RFC 1951 requires to be zero (2). Every big section in the corpus is
	// dynamic, so this is the fallback, not the first pass.
	let all: Vec<u8> = match all_btypes {
		false => raw::anchors(),
		true => {
			let mut v = raw::anchors();
			v.extend((0..=255u8).filter(|b| b & 0x06 == 0x02));
			v.extend([0x00u8, 0x01]);
			v
		}
	};
	let list: Vec<u8> = match order {
		Some(spec) => {
			let mut v: Vec<u8> = spec
				.split(',')
				.filter(|t| !t.is_empty())
				.map(|t| {
					let t = t.trim();
					let t = t.strip_prefix("0x").unwrap_or(t);
					u8::from_str_radix(t, 16).unwrap_or_else(|e| panic!("bad anchor {t:?}: {e}"))
				})
				.collect();
			// An order is a permutation, never a filter: anything the caller
			// left out is appended, so "how far did it get" stays answerable.
			for a in &all {
				if !v.contains(a) {
					v.push(*a);
				}
			}
			assert_eq!(v.len(), all.len(), "the order must be a permutation of the {} anchors", all.len());
			v
		}
		None => all.clone(),
	};
	for a in &list {
		assert!(all.contains(a), "{a:#04x} is not a legal deflate anchor");
	}

	let mut log = std::fs::File::create(out).unwrap_or_else(|e| panic!("creating {out}: {e}"));
	let t0 = Instant::now();
	for (n, d0) in list.iter().enumerate() {
		let started = Instant::now();
		eprint!(
			"[{:2}/{}] anchor {d0:#04x} (BFINAL={} BTYPE={} HLIT={}) … ",
			n + 1,
			list.len(),
			d0 & 1,
			(d0 >> 1) & 3,
			257 + (d0 >> 3) as u16
		);
		std::io::stderr().flush().ok();
		let hit = rod::crack::recover_iv3to8(want_tag.as_bytes(), &s.cipher, s.plainlen, Some(*d0));
		let secs = started.elapsed().as_secs_f64();
		match hit {
			Some(iv) => {
				let line = format!(
					"HIT anchor={d0:#04x} iv3to8={} elapsed={secs:.1}s total={:.1}s\n",
					iv.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
					t0.elapsed().as_secs_f64()
				);
				eprint!("{line}");
				log.write_all(line.as_bytes()).ok();
				// Prove it here rather than trusting the searcher's own oracle:
				// rebuild the whole first-block IV the way the shipped
				// `decode_shifted` does and inflate the entire section.
				let t = raw::first_block(&s.cipher);
				let mut iv8 = [0u8; 8];
				iv8[0] = t[0] ^ 0x78;
				iv8[1] = t[1] ^ 0xda;
				iv8[2] = t[2] ^ d0;
				iv8[3..8].copy_from_slice(&iv);
				let dec = raw::cbc(&s.cipher, iv8);
				match miniz_oxide::inflate::decompress_to_vec_zlib(&dec) {
					Ok(bytes) => {
						let model2 = raw::model_iv(want_tag.as_bytes())[2];
						let d = [
							iv8[0] ^ raw::model_iv(want_tag.as_bytes())[0],
							iv8[1] ^ raw::model_iv(want_tag.as_bytes())[1],
							iv8[2] ^ model2,
						];
						let line = format!(
							"INFLATED {} bytes (declared {}) D[0:3]={:02x} {:02x} {:02x} iv={}\n",
							bytes.len(),
							s.plainlen,
							d[0],
							d[1],
							d[2],
							iv8.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
						);
						eprint!("{line}");
						log.write_all(line.as_bytes()).ok();
						let blob = format!("{out}.{want_tag}.bin");
						std::fs::write(&blob, &bytes).unwrap_or_else(|e| panic!("writing {blob}: {e}"));
						eprintln!("wrote {blob}");
					}
					Err(e) => {
						let line = format!("HIT BUT INFLATE FAILED: {e:?}\n");
						eprint!("{line}");
						log.write_all(line.as_bytes()).ok();
					}
				}
				return;
			}
			None => {
				let line = format!(
					"miss anchor={d0:#04x} elapsed={secs:.1}s total={:.1}s ({}/{} done)\n",
					t0.elapsed().as_secs_f64(),
					n + 1,
					list.len()
				);
				eprint!("{line}");
				log.write_all(line.as_bytes()).ok();
				log.flush().ok();
			}
		}
	}
	let line = format!("EXHAUSTED all {} anchors, no key. total={:.1}s\n", list.len(), t0.elapsed().as_secs_f64());
	eprint!("{line}");
	log.write_all(line.as_bytes()).ok();
}

// -------------------------------------------------------------- `tailkraft`
//
// Scratch analysis for the key-search speed work: the code-length-code entries
// live at fixed absolute bit offsets (17 + 3i), and everything from bit 48 on
// is the *exact* CBC tail. So for each HCLEN the entries wholly inside the tail
// have known lengths and a known Kraft contribution — a constant the search
// could subtract from the budget the five guess bytes are allowed to spend.
fn cmd_tailkraft(path: &str) {
	let data = read(path);
	for s in raw::framed(&data) {
		if !s.compressed || s.cipher.len() < 16 {
			continue;
		}
		let first: [u8; 8] = s.cipher[0..8].try_into().unwrap();
		let tail = raw::cbc(&s.cipher[8..], first); // CBC tail: bytes 6.. of the deflate stream
		let bit = |b: usize| -> u32 {
			let i = b / 8;
			if i < 6 {
				return 9; // guess/anchor territory
			}
			match tail.get(i - 6) {
				Some(v) => ((v >> (b % 8)) & 1) as u32,
				None => 9,
			}
		};
		print!("[{:<6}] ", s.tag);
		for h in 4..=19usize {
			// entries i with 17+3i >= 48, i.e. i >= 11, are pure tail
			let mut w = 0u32;
			let mut ok = true;
			for i in 11..h {
				let (b0, b1, b2) = (bit(17 + 3 * i), bit(18 + 3 * i), bit(19 + 3 * i));
				if b0 > 1 || b1 > 1 || b2 > 1 {
					ok = false;
					break;
				}
				let l = b0 | (b1 << 1) | (b2 << 2);
				if l > 0 {
					w += 1 << (7 - l);
				}
			}
			if !ok {
				print!(" h{h}=?");
			} else {
				print!(" h{h}={w}");
			}
		}
		println!();

// ---------------------------------------------------------------- `classic`

/// Time the shipped *classic* search (reduced candidate sets, no anchor sweep)
/// on one section — the path that `vagcan vcds rod` runs on a `product != 0`
/// classic file. Calls `recover_iv3to8` with `known_anchor = None`; on a classic
/// section that goes straight to `search_anchor(full_sets = false)`, so it times
/// the shipped searcher directly without the whole-file plumbing.
fn cmd_classic(path: &str, want_tag: &str) {
	let data = read(path);
	let sections = raw::framed(&data);
	let s = sections
		.iter()
		.find(|s| s.tag == want_tag)
		.unwrap_or_else(|| panic!("no [{want_tag}] section in {path}"));
	assert!(s.compressed, "[{want_tag}] is not a compressed section");
	let pre = model_prefix(&s.tag, &s.cipher);
	let classic = pre[0] == 0x78 && pre[1] == 0xda;
	println!(
		"{path} [{}]: cipher {} B, plain {} B, {}",
		s.tag,
		s.cipher.len(),
		s.plainlen,
		if classic { "classic" } else { "SHIFTED" }
	);
	let t0 = Instant::now();
	let hit = rod::crack::recover_iv3to8(want_tag.as_bytes(), &s.cipher, s.plainlen, None);
	let secs = t0.elapsed().as_secs_f64();
	match hit {
		Some(iv) => println!(
			"HIT iv3to8={} in {secs:.2}s",
			iv.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
		),
		None => println!("MISS in {secs:.2}s"),
	}
}

fn main() {
	let args: Vec<String> = std::env::args().collect();
	match args.get(1).map(String::as_str) {
		Some("tailkraft") => cmd_tailkraft(&args[2]),
		Some("anchors") => cmd_anchors(&args[2]),
		Some("classic") => cmd_classic(&args[2], &args[3]),
		Some("shifts") => cmd_shifts(&args[2]),
		Some("probe") => cmd_probe(&args[2]),
		Some("sweep") => {
			let path = &args[2];
			let tag = &args[3];
			let mut order = None;
			let mut out = "sweep.log".to_string();
			let mut all_btypes = false;
			let mut i = 4;
			while i < args.len() {
				match args[i].as_str() {
					"--order" => {
						order = Some(args[i + 1].clone());
						i += 2;
					}
					"--out" => {
						out = args[i + 1].clone();
						i += 2;
					}
					"--all-btypes" => {
						all_btypes = true;
						i += 1;
					}
					other => panic!("unknown flag {other}"),
				}
			}
			cmd_sweep(path, tag, order.as_deref(), &out, all_btypes);
		}
		_ => {
			eprintln!(
				"usage:\n  tttext2_sweep anchors <UDS_EV dir>\n  tttext2_sweep probe <file.rod>\n  \
				 tttext2_sweep sweep <file.rod> <TAG> [--order 0x8c,0x4d,…] [--out sweep.log]"
			);
			std::process::exit(2);
		}
	}
}
