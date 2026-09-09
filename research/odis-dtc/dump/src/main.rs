//! Throwaway: dump objects of one type from one ODIS pool, with every
//! four-byte window that resolves against a string pool annotated.
//!
//! Research tooling, not shipped. Reads a project the owner has on disk; the
//! checkout carries none. Usage:
//!
//! ```text
//! odis-dtc-dump <project dir> <pool id> census
//! odis-dtc-dump <project dir> <pool id> <type code hex> [limit] [ObjectID substring]
//! odis-dtc-dump <project dir> <pool id> object <ObjectID>
//! odis-dtc-dump <project dir> <pool id> smallest <type code hex>
//! odis-dtc-dump <project dir> <pool id> tail <type code hex> <bytes>
//! odis-dtc-dump <project dir> <pool id> dtcstat
//! odis-dtc-dump <project dir> all dtcstat        (every pool)
//! ```
use std::collections::BTreeMap;

use vag_data_labels::odis::{keyfile, object, pool, strings};

struct Opened {
	key: keyfile::KeyFile,
	db: pool::Pool,
}

fn open(dir: &std::path::Path, pool_id: &str) -> Opened {
	Opened {
		key: keyfile::KeyFile::open(&dir.join(format!("{pool_id}.key"))).expect("the .key opens"),
		db: pool::Pool::open(&dir.join(format!("{pool_id}.db"))).expect("the .db opens"),
	}
}

fn each(opened: &Opened, mut f: impl FnMut(&[u8], &[u8])) {
	for record in opened.key.records().expect("the tree walks") {
		let Ok(locator) = pool::Locator::parse(&record.data) else { continue };
		let Ok(bytes) = opened.db.member(&locator) else { continue };
		f(&record.key, &bytes);
	}
}

fn main() {
	let args: Vec<String> = std::env::args().collect();
	let dir = std::path::Path::new(&args[1]);
	let pool_id = &args[2];
	let strings = strings::Strings::open(dir).expect("the string pools open");
	let mode = args[3].as_str();

	if mode == "dtcstat" {
		let pools: Vec<String> = if pool_id == "all" {
			let mut v: Vec<String> = std::fs::read_dir(dir)
				.unwrap()
				.filter_map(|e| e.ok())
				.filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".key")).map(str::to_owned))
				.collect();
			v.sort();
			v
		} else {
			vec![pool_id.clone()]
		};
		dtcstat(&strings, dir, &pools);
		return;
	}

	if mode == "validate" {
		validate(dir, &args[4..]);
		return;
	}
	if mode == "dopstat" {
		let pools: Vec<String> = if pool_id == "all" {
			let mut v: Vec<String> = std::fs::read_dir(dir)
				.unwrap()
				.filter_map(|e| e.ok())
				.filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".key")).map(str::to_owned))
				.collect();
			v.sort();
			v
		} else {
			vec![pool_id.clone()]
		};
		dopstat(&strings, dir, &pools);
		return;
	}

	let opened = open(dir, pool_id);
	if mode == "object" {
		let id = &args[4];
		let hash = strings.ascii.hash_of(id).expect("the ObjectID is in the pool");
		let data = opened
			.key
			.find(&hash.to_le_bytes())
			.expect("the tree reads")
			.expect("the ObjectID is in the tree");
		let bytes = opened
			.db
			.member(&pool::Locator::parse(&data).expect("a locator"))
			.expect("the member inflates");
		dump(&strings, id, &bytes, 0);
		return;
	}
	if mode == "smallest" || mode == "tail" {
		let wanted = u16::from_str_radix(&args[4], 16).expect("a hex type code");
		let tail: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
		let mut best: Option<(String, Vec<u8>)> = None;
		each(&opened, |key, bytes| {
			if !matches!(object::type_code(bytes), Ok(code) if code == wanted) {
				return;
			}
			let h = u32::from_le_bytes([key[0], key[1], key[2], key[3]]);
			let id = strings.ascii.get(h).unwrap_or("?").to_owned();
			let take = match &best {
				None => true,
				Some((_, b)) => mode == "smallest" && bytes.len() < b.len(),
			};
			if take {
				best = Some((id, bytes.to_vec()));
			}
		});
		let (id, bytes) = best.expect("one object of the type");
		let from = if mode == "tail" { bytes.len().saturating_sub(tail) } else { 0 };
		dump(&strings, &id, &bytes, from);
		return;
	}

	let wanted: Option<u16> = (mode != "census").then(|| u16::from_str_radix(mode, 16).expect("a hex type code"));
	let limit: usize = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(usize::MAX);
	let filter = args.get(5).cloned();
	let mut census: BTreeMap<u16, (usize, usize, usize)> = BTreeMap::new();
	let mut shown = 0usize;
	each(&opened, |key, bytes| {
		let Ok(code) = object::type_code(bytes) else { return };
		let e = census.entry(code).or_insert((0, usize::MAX, 0));
		e.0 += 1;
		e.1 = e.1.min(bytes.len());
		e.2 = e.2.max(bytes.len());
		if wanted == Some(code) && shown < limit {
			let h = u32::from_le_bytes([key[0], key[1], key[2], key[3]]);
			let id = strings.ascii.get(h).unwrap_or("?").to_owned();
			if let Some(f) = &filter
				&& !id.contains(f.as_str())
			{
				return;
			}
			dump(&strings, &id, bytes, 0);
			shown += 1;
		}
	});
	if wanted.is_none() {
		for (code, (n, min, max)) in census {
			println!("{code:#06x}  {n:>8}  len {min}..{max}");
		}
	}
}

/// Run `Project::faults` over every variant and report; then look up the
/// codes given as `<variant prefix>:<decimal code>` pairs.
fn validate(dir: &std::path::Path, lookups: &[String]) {
	use vag_data_labels::odis::{Error, Project};
	let started = std::time::Instant::now();
	let project = Project::open(dir).expect("the project opens");
	println!(
		"project {} version {:?} language {:?}",
		project.id(),
		project.version(),
		project.language()
	);
	let variants = project.variants().expect("variants");
	let (mut with, mut codes, mut empty, mut refused, mut failed) = (0usize, 0usize, 0usize, 0usize, 0usize);
	let mut failures: Vec<String> = Vec::new();
	let mut dops: BTreeMap<String, usize> = BTreeMap::new();
	let mut levels: BTreeMap<u32, usize> = BTreeMap::new();
	let mut temporary = 0usize;
	let mut all: Vec<(String, Vec<vag_data_labels::odis::Fault>)> = Vec::new();
	for v in &variants {
		match project.faults(v) {
			Ok(rows) if rows.is_empty() => empty += 1,
			Ok(rows) => {
				with += 1;
				codes += rows.len();
				for r in &rows {
					*dops.entry(r.dop.clone()).or_default() += 1;
					*levels.entry(r.level).or_default() += 1;
					if r.temporary {
						temporary += 1;
					}
				}
				all.push((v.name.clone(), rows));
			}
			Err(Error::Refused(_)) => refused += 1,
			Err(e) => {
				failed += 1;
				if failures.len() < 10 {
					failures.push(format!("{}: {e}", v.name));
				}
			}
		}
	}
	println!(
		"{} variants: {with} with faults ({codes} codes), {empty} with none, {refused} refused, {failed} failed, in {:.1?}",
		variants.len(),
		started.elapsed()
	);
	for f in &failures {
		println!("  failed: {f}");
	}
	println!("by table: {dops:?}");
	println!("by level: {levels:?}; temporary: {temporary}");
	for lookup in lookups {
		let (prefix, code) = lookup.split_once(':').expect("prefix:code");
		let code: u32 = code.parse().expect("a decimal code");
		let mut hits: Vec<String> = Vec::new();
		for (variant, rows) in &all {
			if !variant.starts_with(prefix) {
				continue;
			}
			for r in rows.iter().filter(|r| r.code == code) {
				hits.push(format!(
					"{variant} {:?} level {} text_id {:?}: {:?}",
					r.display_code.as_deref().unwrap_or("-"),
					r.level,
					r.text_id,
					r.text.as_deref().unwrap_or("-")
				));
			}
		}
		hits.sort();
		hits.dedup();
		println!("{lookup} ({code:#08x}): {} hits", hits.len());
		for h in hits.iter().take(6) {
			println!("    {h}");
		}
	}
}

/// Statistics over every `DB_DOP_DTC` (0x0028) object: what follows the map.
fn dopstat(strings: &strings::Strings, dir: &std::path::Path, pools: &[String]) {
	let mut total = 0usize;
	let mut after_map: BTreeMap<String, usize> = BTreeMap::new();
	let mut entry_pool_is_own = 0usize;
	let mut entry_pool_other = 0usize;
	let mut entry_object_missing = 0usize;
	let mut second_count: BTreeMap<u16, usize> = BTreeMap::new();
	let mut samples: Vec<String> = Vec::new();
	for pool_id in pools {
		let opened = open(dir, pool_id);
		each(&opened, |key, bytes| {
			if !matches!(object::type_code(bytes), Ok(0x0028)) {
				return;
			}
			total += 1;
			let w = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
			let count = u16::from_le_bytes([bytes[2], bytes[3]]) as usize;
			let mut at = 4;
			for _ in 0..count {
				let obj = strings.ascii.get(w(at + 4));
				let pool = strings.ascii.get(w(at + 8));
				if obj.is_none() {
					entry_object_missing += 1;
				}
				if pool == Some(pool_id.as_str()) {
					entry_pool_is_own += 1;
				} else {
					entry_pool_other += 1;
				}
				at += 12;
			}
			let second = u16::from_le_bytes([bytes[at], bytes[at + 1]]);
			*second_count.entry(second).or_default() += 1;
			// Everything after the map, with the two name hashes masked so the
			// pattern is the shape rather than the names.
			let mut rest: Vec<String> = bytes[at..].iter().map(|b| format!("{b:02x}")).collect();
			for i in at..bytes.len().saturating_sub(3) {
				let v = w(i);
				if v != 0 && (strings.ascii.get(v).is_some() || strings.unicode.get(v).is_some()) {
					for j in 0..4 {
						rest[i - at + j] = "NN".into();
					}
				}
			}
			let pattern = rest.join(" ");
			let n = after_map.entry(pattern.clone()).or_default();
			*n += 1;
			if *n == 1 && samples.len() < 6 {
				let h = u32::from_le_bytes([key[0], key[1], key[2], key[3]]);
				samples.push(format!("{}: {pattern}", strings.ascii.get(h).unwrap_or("?")));
			}
		});
	}
	println!("DOP_DTC objects: {total}");
	println!("map entries: pool is own pool {entry_pool_is_own}, other pool {entry_pool_other}, object name unresolved {entry_object_missing}");
	println!("second count: {second_count:?}");
	println!("distinct tails after the map: {}", after_map.len());
	for (pattern, n) in &after_map {
		println!("  {n:>6}  {pattern}");
	}
	for s in &samples {
		println!("  first of: {s}");
	}
}

/// Statistics over every `MCD_DB_DIAG_TROUBLE_CODE` (0x0057) object.
fn dtcstat(strings: &strings::Strings, dir: &std::path::Path, pools: &[String]) {
	let mut total = 0usize;
	let mut lengths: BTreeMap<usize, usize> = BTreeMap::new();
	let mut mid: BTreeMap<Vec<u8>, usize> = BTreeMap::new(); // bytes 0x12..0x16
	let mut byte1a: BTreeMap<u8, usize> = BTreeMap::new();
	let mut tail_eq_display = 0usize;
	let mut code_eq_shortname = 0usize;
	let mut shortname_not_dtc = 0usize;
	let mut no_text = 0usize;
	let mut no_display = 0usize;
	let mut display_shape: BTreeMap<String, usize> = BTreeMap::new();
	let mut german = 0usize;
	let mut english = 0usize;
	let mut other = 0usize;
	let mut samples: Vec<String> = Vec::new();
	let mut code_by_display: BTreeMap<String, std::collections::BTreeSet<u32>> = BTreeMap::new();
	let mut texts_per_code: BTreeMap<u32, std::collections::BTreeSet<String>> = BTreeMap::new();
	let mut per_pool: Vec<(String, usize)> = Vec::new();
	for pool_id in pools {
		let opened = open(dir, pool_id);
		let mut here = 0usize;
		each(&opened, |_, bytes| {
			if !matches!(object::type_code(bytes), Ok(0x0057)) {
				return;
			}
			total += 1;
			here += 1;
			*lengths.entry(bytes.len()).or_default() += 1;
			if bytes.len() < 34 {
				return;
			}
			let w = |at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
			*mid.entry(bytes[0x12..0x16].to_vec()).or_default() += 1;
			*byte1a.entry(bytes[0x1a]).or_default() += 1;
			if w(0x0a) == w(0x1b) {
				tail_eq_display += 1;
			} else if samples.len() < 40 {
				samples.push(format!(
					"0x1b differs in {pool_id}: display {:?} vs tail {:?} / {:?}",
					strings.ascii.get(w(0x0a)),
					strings.ascii.get(w(0x1b)),
					strings.unicode.get(w(0x1b))
				));
			}
			let short = strings.ascii.get(w(0x06)).unwrap_or("");
			let code = w(0x16);
			match short.strip_prefix("DTC_").and_then(|s| s.parse::<u32>().ok()) {
				Some(n) if n == code => code_eq_shortname += 1,
				_ => shortname_not_dtc += 1,
			}
			let display = strings.ascii.get(w(0x0a)).unwrap_or("");
			if display.is_empty() {
				no_display += 1;
			} else {
				let shape: String = display
					.chars()
					.map(|c| {
						if c.is_ascii_digit() {
							'9'
						} else if c.is_ascii_alphabetic() {
							'A'
						} else {
							c
						}
					})
					.collect();
				*display_shape.entry(shape).or_default() += 1;
				code_by_display.entry(display.to_owned()).or_default().insert(code);
			}
			let text = strings.unicode.get(w(0x0e)).unwrap_or("");
			if text.is_empty() {
				no_text += 1;
			} else {
				texts_per_code.entry(code).or_default().insert(text.to_owned());
				let lower = text.to_lowercase();
				let de = [
					"ä",
					"ö",
					"ü",
					"ß",
					" nach ",
					"kurzschluss",
					"unterbrechung",
					"steuergerät",
					"signal ",
					"fehler",
					"unplausibel",
					"defekt",
					" zu ",
					"keine ",
				];
				let en = [
					" to ",
					"circuit",
					"sensor",
					"short",
					"open",
					"high",
					"low",
					"malfunction",
					"control",
					"system",
					"not ",
					"fault",
					"invalid",
					"signal",
				];
				let d = de.iter().filter(|k| lower.contains(*k)).count();
				let e = en.iter().filter(|k| lower.contains(*k)).count();
				if d > e {
					german += 1;
				} else if e > d {
					english += 1;
				} else {
					other += 1;
					if samples.len() < 12 {
						samples.push(format!("{pool_id}: {display} {text:?}"));
					}
				}
			}
		});
		per_pool.push((pool_id.clone(), here));
	}
	println!("DTC objects: {total}");
	println!("lengths: {lengths:?}");
	println!("bytes 0x12..0x16: {mid:?}");
	println!("byte 0x1a: {byte1a:?}");
	println!("hash@0x1b == hash@0x0a: {tail_eq_display}");
	println!("u32@0x16 == number in short name DTC_<n>: {code_eq_shortname}; not: {shortname_not_dtc}");
	println!("no text: {no_text}; no display code: {no_display}");
	println!("display shapes: {display_shape:?}");
	println!("text language guess: german {german}, english {english}, undecided {other}");
	for s in &samples {
		println!("  undecided sample: {s}");
	}
	let multi: Vec<_> = code_by_display.iter().filter(|(_, codes)| codes.len() > 1).take(8).collect();
	println!(
		"display codes mapping to >1 raw code: {} e.g. {multi:?}",
		code_by_display.values().filter(|c| c.len() > 1).count()
	);
	let codes_multi_text = texts_per_code.values().filter(|t| t.len() > 1).count();
	println!(
		"raw codes with >1 distinct text across the project: {codes_multi_text} of {}",
		texts_per_code.len()
	);
	if let Some((code, texts)) = texts_per_code.iter().find(|(_, t)| t.len() > 1) {
		println!("  e.g. {code:#08x}: {texts:?}");
	}
	// The display code against the raw number: is P150B00 == 0x150B00?
	let mut display_is_hex = 0usize;
	let mut display_not_hex = 0usize;
	let mut mismatch_samples = Vec::new();
	for (display, codes) in &code_by_display {
		for code in codes {
			let letter = display.chars().next().unwrap_or('?');
			let prefix = match letter {
				'P' => 0x0,
				'C' => 0x4,
				'B' => 0x8,
				'U' => 0xC,
				_ => 0x10,
			};
			let rest = u32::from_str_radix(&display[1..], 16).ok();
			let expected = rest.map(|r| (prefix << 20) | r);
			if expected == Some(*code) {
				display_is_hex += 1;
			} else {
				display_not_hex += 1;
				if mismatch_samples.len() < 10 {
					mismatch_samples.push(format!("{display} vs {code:#08x}"));
				}
			}
		}
	}
	println!("display code == SAE encoding of raw code: {display_is_hex}; not: {display_not_hex} e.g. {mismatch_samples:?}");
	per_pool.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
	for (p, n) in per_pool.iter().take(12) {
		println!("  {n:>8}  {p}");
	}
}

fn dump(strings: &strings::Strings, id: &str, bytes: &[u8], from: usize) {
	println!("=== {id}  ({} bytes, from {from:#x})", bytes.len());
	let hex: Vec<String> = bytes[from..].iter().map(|b| format!("{b:02x}")).collect();
	for (i, chunk) in hex.chunks(16).enumerate() {
		println!("  {:04x}: {}", from + i * 16, chunk.join(" "));
	}
	for at in from..bytes.len().saturating_sub(3) {
		let w = u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
		if w == 0 {
			continue;
		}
		if let Some(s) = strings.ascii.get(w) {
			println!("  @{at:04x} A {w:#010x} {s:?}");
		}
		if let Some(s) = strings.unicode.get(w) {
			println!("  @{at:04x} U {w:#010x} {s:?}");
		}
	}
}
