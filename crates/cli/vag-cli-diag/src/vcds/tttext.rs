//! `vagcan dev vcds tttext` — read the label files' global text table.
//!
//! Every record of `TTTEXT.ROD`'s `[TXT]` section is enciphered under its own
//! key, and the key is the record's id, so each one reads exactly
//! ([`vag_data_labels::tttext`]). This command prints what it read; `vagcan
//! setup` writes the same reading into a project, through [`names_json`] and
//! [`odx_ids_json`].

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use vag_cli_core::verbosity::{self, NOTES, WIRE};
use vag_data_labels::codes::CodePage;
use vag_data_labels::tttext::{self, Table, Tail};

pub struct Options<'a> {
	pub file: &'a str,
	pub out: Option<&'a str>,
	pub catalog: Option<&'a str>,
}

pub fn run(opts: Options<'_>) -> Result<()> {
	let table = read_file(Path::new(opts.file), CodePage::Windows1252)?;
	vag_cli_core::note!("read {} as Windows-1252", opts.file);

	// The records are the data: on stdout, or in `--out` instead.
	let mut lines = String::new();
	for text in &table.texts {
		let (kind, value) = text.tail.as_ref().map_or(("", ""), |t| (t.kind.as_str(), t.value.as_str()));
		lines.push_str(&format!("{:06}\t{}\t{kind}\t{value}\n", text.id, text.name));
	}
	match opts.out {
		Some(path) => {
			let mut file = std::io::BufWriter::new(std::fs::File::create(path).with_context(|| format!("creating the output file {path:?}"))?);
			file.write_all(lines.as_bytes())?;
			file.flush()?;
			vag_cli_core::say!("wrote {} records to {path}", table.texts.len());
		}
		None => vag_cli_core::out!("{lines}"),
	}

	if let Some(path) = opts.catalog {
		let written = write_names(&table, Path::new(path))?;
		vag_cli_core::say!("wrote {written} names to {path}");
	}
	// On stderr, not after the records: on stdout it would be one more record to
	// whatever reads the tab-separated stream.
	vag_cli_core::say!("{}", summary(&table));
	for (at, (id, plain)) in table.malformed.iter().enumerate() {
		verbosity::say(if at < 10 { NOTES } else { WIRE }, &format!("{id:06} read as {plain:?}"));
	}
	Ok(())
}

/// Read a decrypted, inflated `[TXT]` section from disk.
pub fn read_file(path: &Path, page: CodePage) -> Result<Table> {
	let section = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
	Ok(tttext::read(&section, page))
}

/// What a reading holds, in one line.
pub fn summary(table: &Table) -> String {
	let with_ids = table.texts.iter().filter(|t| t.tail.as_ref().and_then(Tail::odx_id).is_some()).count();
	let mut line = format!("{} records · {with_ids} name an IDE or MAS id", table.texts.len());
	if !table.malformed.is_empty() {
		line.push_str(&format!(" · {} malformed, left out", table.malformed.len()));
	}
	if table.not_records > 0 {
		line.push_str(&format!(" · {} lines not records", table.not_records));
	}
	line
}

/// `names.json`'s text — `{"<6-digit text id>": "<name>"}`, the form `vagcan
/// dev vcds names` searches — and how many names it holds.
///
/// Every record is in it. Nothing is withheld, because nothing is guessed: the
/// solver this replaced gated its readings (no digit, a dozen letters, no name
/// twice), and each of those gates was about the solver's doubt, not about the
/// names. Two records can carry the same text — 3,060 names in the 26.3 table
/// do, under different ids with different tails — and both are kept.
pub fn names_json(table: &Table) -> Result<(String, usize)> {
	let names: BTreeMap<String, String> = table.texts.iter().map(|t| (format!("{:06}", t.id), t.name.clone())).collect();
	Ok((serde_json::to_string_pretty(&names)?, names.len()))
}

/// `odx-ids.json`'s text — `{"<6-digit text id>": "IDE#####" | "MAS#####"}`,
/// which ODX object each record names, for the records whose tail says (kinds
/// `2` and `7`) — and how many ids it holds.
///
/// A key of `RM.rod`'s measurement registry is such a record, and the id is how
/// a VCDS row can join what ODIS and a `dash.toml` call the same measurement.
pub fn odx_ids_json(table: &Table) -> Result<(String, usize)> {
	let ids: BTreeMap<String, String> = table
		.texts
		.iter()
		.filter_map(|t| Some((format!("{:06}", t.id), t.tail.as_ref()?.odx_id()?)))
		.collect();
	Ok((serde_json::to_string_pretty(&ids)?, ids.len()))
}

/// Write [`names_json`] to `path`. Returns how many names.
pub fn write_names(table: &Table, path: &Path) -> Result<usize> {
	let (text, count) = names_json(table)?;
	if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
		std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
	}
	std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
	Ok(count)
}

#[cfg(test)]
mod tests {
	use super::*;

	use vag_data_labels::glyphs::TableAlphabet;

	/// A section the way VCDS writes one, from plaintext records.
	fn section(records: &[(u32, &str)]) -> Vec<u8> {
		records
			.iter()
			.map(|(id, plain)| format!("{id:06},{}\r\n", TableAlphabet::for_key(*id).encipher(plain)))
			.collect::<String>()
			.into_bytes()
	}

	fn table(records: &[(u32, &str)]) -> Table {
		tttext::read(&section(records), CodePage::Windows1252)
	}

	#[test]
	fn names_json_holds_every_record_keyed_by_its_six_digit_id() {
		let here = tempfile::tempdir().unwrap();
		let path = here.path().join("names.json");
		let t = table(&[
			(17, "Invented flap position,2,90001"),
			(910_231, "Invented Shaft Speed Probe,"),
			(13, "Pump 2,1,00042"),
		]);
		assert_eq!(write_names(&t, &path).unwrap(), 3);
		let back: BTreeMap<String, String> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
		assert_eq!(back["000017"], "Invented flap position");
		assert_eq!(back["910231"], "Invented Shaft Speed Probe");
		// Short and with a digit: the solver's gate dropped both kinds.
		assert_eq!(back["000013"], "Pump 2");
	}

	#[test]
	fn two_records_with_one_text_both_stay() {
		// The solver dropped both when two readings agreed, since that was how
		// a digit eaten off the end looked. Read exactly, they are two texts.
		let here = tempfile::tempdir().unwrap();
		let path = here.path().join("names.json");
		let t = table(&[(500_001, "Coolant level,10,00012"), (500_002, "Coolant level,")]);
		assert_eq!(write_names(&t, &path).unwrap(), 2);
	}

	#[test]
	fn odx_ids_hold_only_the_two_known_kinds() {
		let t = table(&[
			(17, "Invented flap position,2,90001"),
			(3, "Low - high,7,01234"),
			(304, "Seat module,9,000B7"),
			(910_231, "Invented Shaft Speed Probe,"),
		]);
		let (text, count) = odx_ids_json(&t).unwrap();
		assert_eq!(count, 2);
		let back: BTreeMap<String, String> = serde_json::from_str(&text).unwrap();
		assert_eq!(
			back,
			BTreeMap::from([
				("000003".to_string(), "MAS01234".to_string()),
				("000017".to_string(), "IDE90001".to_string())
			])
		);
	}

	#[test]
	fn the_summary_says_what_was_left_out() {
		let t = table(&[(17, "Invented flap position,2,90001"), (5, "no separator")]);
		assert_eq!(summary(&t), "1 records · 1 name an IDE or MAS id · 1 malformed, left out");
	}

	/// The research's four cribs, against the owner's VCDS install when it is
	/// here — skipped otherwise, like the proven-row tests: the table is
	/// Ross-Tech's and is not in the checkout, and neither are the cribs' words.
	/// What is pinned is that the whole table reads, and that each crib reads
	/// under its own id as a name with the tail it has. Needs the text table's
	/// key in this machine's project, which `vagcan setup` records.
	#[test]
	fn the_cribs_read_from_the_installed_table() {
		let Some(home) = std::env::var_os("HOME") else { return };
		let rod = Path::new(&home).join("vcds-en/UDS_EV/TTTEXT.ROD");
		let Ok(data) = std::fs::read(&rod) else {
			eprintln!("skipped: no VCDS install at {}", rod.display());
			return;
		};
		// The key is the file's name: nothing cached is needed (2026-10-20).
		let mut keys = vag_data_labels::rod::IvCache::default();
		let section = vag_data_labels::rod::open_section(&data, "TTTEXT.ROD", "TXT", &mut keys, false);
		let text = section
			.as_ref()
			.and_then(|s| s.text.as_ref())
			.expect("the section opens by its file name");
		// The section comes back one byte per char; the reader wants the bytes.
		let bytes: Vec<u8> = text.chars().map(|c| c as u8).collect();
		let t = tttext::read(&bytes, CodePage::Windows1252);
		assert!(t.malformed.is_empty(), "{}", summary(&t));
		let read = |id: u32| t.texts.iter().find(|x| x.id == id).unwrap_or_else(|| panic!("no record {id}"));
		for id in [103_074, 99_967, 100_415, 80] {
			assert!(!read(id).name.trim().is_empty(), "record {id} read as no name");
		}
		assert_eq!(read(80).tail.as_ref().and_then(Tail::odx_id).as_deref(), Some("IDE00594"));
		for id in [103_074, 99_967, 100_415] {
			assert_eq!(read(id).tail, None, "record {id} has no tail");
		}
	}
}
