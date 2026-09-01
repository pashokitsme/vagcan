//! Census an ODIS project pool and dump raw objects — the tool that found the
//! layer-inheritance bug, kept because the next format question will need it.
//!
//! **What it is for.** Answering "what is actually in this pool, and what do the
//! bytes of one object look like", without adding a loader first. It is how
//! `EV_DCUDriveSideEWMAXCONT_006` was found sitting in the project with a layer
//! that declares no services and one parent — see `odis-format.md` §4.1.
//!
//! **What it expects.** An **extracted ODIS project directory** — the folder
//! holding `0.0.0@*.db` / `.key` pairs and `AStringData.data.gz`, e.g.
//! `~/Downloads/SK37X`. Not an archive, not a parent directory.
//!
//! **What it writes.** Nothing. Everything goes to stdout: a type-code census
//! per matching pool, and with a type given, the first two objects of that type
//! as hex, ASCII, and every four-byte word that resolves to a pool string —
//! which is what makes an untagged positional record readable at all.
//!
//! ```text
//! cargo run -p vag-data-labels --example odis_pool -- <project dir> [pool substring] [type hex]
//! ```

use vag_data_labels::odis::{keyfile, object, pool, strings};

fn main() {
	let mut args = std::env::args().skip(1);
	let Some(dir) = args.next() else {
		eprintln!("usage: odis_pool <extracted ODIS project dir> [pool substring] [type hex]");
		std::process::exit(2);
	};
	let want_pool = args.next().unwrap_or_default();
	let want_type = args.next().and_then(|t| u16::from_str_radix(t.trim_start_matches("0x"), 16).ok());
	let dir = std::path::Path::new(&dir);
	let strings = strings::Strings::open(dir).expect("the string pools open");

	for entry in std::fs::read_dir(dir).expect("the directory reads") {
		let path = entry.expect("an entry").path();
		let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
			continue;
		};
		let Some(id) = name.strip_suffix(".key") else { continue };
		if !id.contains(&want_pool) {
			continue;
		}
		println!("== {id}");
		let key = keyfile::KeyFile::open(&path).expect("the .key opens");
		let db = pool::Pool::open(&dir.join(format!("{id}.db"))).expect("the .db opens");
		let mut census: std::collections::BTreeMap<u16, usize> = Default::default();
		let mut shown = 0;
		for record in key.records().expect("the .key parses") {
			let Ok(locator) = pool::Locator::parse(&record.data) else { continue };
			let Ok(bytes) = db.member(&locator) else { continue };
			let Ok(code) = object::type_code(&bytes) else { continue };
			*census.entry(code).or_default() += 1;
			if want_type != Some(code) || shown >= 2 {
				continue;
			}
			shown += 1;
			println!("\n-- {code:#06x}, {} bytes", bytes.len());
			for (n, chunk) in bytes.chunks(24).enumerate() {
				let hex: String = chunk.iter().map(|b| format!("{b:02X} ")).collect();
				let txt: String = chunk.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }).collect();
				println!("   {:04}  {hex:<73}{txt}", n * 24);
			}
			// Most fields are a four-byte hash into the ASCII pool, so resolving
			// every window is what turns an untagged record into field names.
			let named: Vec<(usize, String)> = (0..bytes.len().saturating_sub(3))
				.filter_map(|i| {
					let h = u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
					strings.ascii.get(h).filter(|s| s.len() > 3).map(|s| (i, s.to_owned()))
				})
				.collect();
			if !named.is_empty() {
				println!("   strings: {named:?}");
			}
		}
		if want_type.is_none() {
			for (code, n) in &census {
				println!("  {code:#06x}  {n}");
			}
		}
	}
}
