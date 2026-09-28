//! Reading `TTTEXT.ROD`'s `[TXT]` section — VCDS's global text table — exactly.
//!
//! Every record is `NNNNNN,<payload>`. The id is plaintext; the payload is
//! enciphered under the alphabet the C runtime's `srand(id)` generates, the same
//! generator the fault registry's tables use ([`TableAlphabet::for_key`]).
//! Letters (case kept) and the digit class `0-9 , . - _` go through it and
//! everything else passes through, so a record reads by lookup: no dictionary,
//! no search, nothing guessed, digits included.
//!
//! A record's plaintext is `<name>,` or `<name>,<kind>,<value>`. The tail says
//! which ODX object the text is the name of: kind `2` is an `IDE#####`, kind `7`
//! a `MAS#####` ([`Tail::odx_id`]). Other kinds occur — `0`, `1`, `6`, `8`–`11`,
//! `A`–`E` — and are kept as read, not interpreted.
//!
//! Evidence, `research/vcds-registry/README.md` §0–§1: all 195,910 records of the
//! 26.3 table are well-formed under `srand(id)`, against 6.6 % under
//! `srand(id + 1)`; four names VCDS prints in its own logs as `ENG######` read
//! back verbatim. This replaced a dictionary solver
//! (`.archive/research/labels/tttext-codec.md`, wrong in its §5 and §6) that read
//! about half the records and none of their digits.

use rayon::prelude::*;

use crate::codes::CodePage;
use crate::glyphs::TableAlphabet;

/// One record of the text table, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Text {
	/// The record's id — what `RM.rod`'s rows and VCDS's `ENG######` refer to,
	/// and what a project's `names.json` is keyed by.
	pub id: u32,
	pub name: String,
	/// `<kind>,<value>`, when the record carries one.
	pub tail: Option<Tail>,
}

/// A record's `<kind>,<value>` tail, as read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tail {
	pub kind: String,
	pub value: String,
}

impl Tail {
	/// The ODX id this text is the name of, for the two kinds that are
	/// established: `2` → `IDE#####`, `7` → `MAS#####`.
	///
	/// 2,279 of an ODIS project's 2,280 `IDE` ids and 2,749 of its 2,810 `MAS`
	/// ids are found in the table this way (README §0). Any other kind, or a
	/// value that is not five digits, is `None`: read, but not known to be an
	/// ODX id.
	pub fn odx_id(&self) -> Option<String> {
		let prefix = match self.kind.as_str() {
			"2" => "IDE",
			"7" => "MAS",
			_ => return None,
		};
		let five_digits = self.value.len() == 5 && self.value.bytes().all(|b| b.is_ascii_digit());
		five_digits.then(|| format!("{prefix}{}", self.value))
	}
}

/// What reading a section found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Table {
	/// Every record that read into a name, in file order.
	pub texts: Vec<Text>,
	/// Records whose plaintext is neither `<name>,` nor `<name>,<kind>,<value>`:
	/// the id and what it read as. Reported, not guessed at — the 26.3 table
	/// has none, so one here says the section or the key is not what this
	/// reader was built on.
	pub malformed: Vec<(u32, String)>,
	/// Lines with no `<id>,` in front: not records at all.
	pub not_records: usize,
}

/// Read a decrypted, inflated `[TXT]` section.
///
/// `page` is the code page the section's high bytes are in. The cipher leaves
/// them alone, so they are plaintext: in the English build `0x96` is an en dash,
/// which ISO 8859-1 would turn into a C1 control.
///
/// Records are read on rayon's pool and come back in file order.
pub fn read(section: &[u8], page: CodePage) -> Table {
	let lines: Vec<&[u8]> = section
		.split(|b| *b == b'\n')
		.map(|line| line.strip_suffix(b"\r").unwrap_or(line))
		.filter(|line| !line.is_empty())
		.collect();
	let read: Vec<Option<Result<Text, (u32, String)>>> = lines.par_iter().map(|line| record(line, page)).collect();

	let mut table = Table::default();
	for outcome in read {
		match outcome {
			Some(Ok(text)) => table.texts.push(text),
			Some(Err(malformed)) => table.malformed.push(malformed),
			None => table.not_records += 1,
		}
	}
	table
}

/// One line: `None` when it is not a record, else the record read or refused.
fn record(line: &[u8], page: CodePage) -> Option<Result<Text, (u32, String)>> {
	let comma = line.iter().position(|b| *b == b',')?;
	let (digits, payload) = (&line[..comma], &line[comma + 1..]);
	if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
		return None;
	}
	let id: u32 = std::str::from_utf8(digits).ok()?.parse().ok()?;
	let plain = TableAlphabet::for_key(id).decipher(&page.decode(payload));
	Some(parse(id, plain))
}

/// Split a record's plaintext into its name and its tail.
///
/// From the right, so a comma inside a name cannot move the tail.
fn parse(id: u32, plain: String) -> Result<Text, (u32, String)> {
	if let Some(name) = plain.strip_suffix(',') {
		return Ok(Text {
			id,
			name: name.to_string(),
			tail: None,
		});
	}
	let mut fields = plain.rsplitn(3, ',');
	let (Some(value), Some(kind), Some(name)) = (fields.next(), fields.next(), fields.next()) else {
		return Err((id, plain));
	};
	let token = |field: &str| !field.is_empty() && field.bytes().all(|b| b.is_ascii_alphanumeric());
	if !token(kind) || !token(value) {
		return Err((id, plain));
	}
	Ok(Text {
		id,
		name: name.to_string(),
		tail: Some(Tail {
			kind: kind.to_string(),
			value: value.to_string(),
		}),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A section line the way VCDS writes one: the id in plain sight, the
	/// payload under the record's own key, CRLF at the end.
	fn line(id: u32, plain: &str) -> String {
		format!("{id:06},{}\r\n", TableAlphabet::for_key(id).encipher(plain))
	}

	/// Invented records in the shapes of the research's four cribs: three names
	/// with no tail, one with digits in it, and one whose tail is an `IDE`. The
	/// cribs themselves are Ross-Tech's words; `vag-cli-diag`'s gated test reads
	/// them from the installed table.
	const RECORDS: [(u32, &str); 4] = [
		(910_231, "Invented Shaft Speed Probe,"),
		(905_517, "Synthetic Wheel Speed Probe,"),
		(907_042, "X042 Invented Timer Manual,"),
		(17, "Invented flap position,2,90001"),
	];

	#[test]
	fn every_record_reads_under_its_own_id() {
		let section: String = RECORDS.iter().map(|(id, plain)| line(*id, plain)).collect();
		let table = read(section.as_bytes(), CodePage::Windows1252);
		assert_eq!(table.malformed, Vec::new());
		assert_eq!(table.not_records, 0);
		let names: Vec<(u32, &str)> = table.texts.iter().map(|t| (t.id, t.name.as_str())).collect();
		assert_eq!(
			names,
			vec![
				(910_231, "Invented Shaft Speed Probe"),
				(905_517, "Synthetic Wheel Speed Probe"),
				(907_042, "X042 Invented Timer Manual"),
				(17, "Invented flap position"),
			],
			"in file order, digits included"
		);
	}

	#[test]
	fn a_tail_parses_into_its_kind_and_value_and_the_two_known_kinds_into_an_odx_id() {
		let section = [
			line(17, "Invented flap position,2,90001"),
			line(3, "Low - high,7,01234"),
			line(304, "Seat module,9,000B7"),
			line(26, "Invented Shaft Speed Probe,"),
		]
		.concat();
		let table = read(section.as_bytes(), CodePage::Windows1252);
		let tails: Vec<(u32, Option<&Tail>, Option<String>)> = table
			.texts
			.iter()
			.map(|t| (t.id, t.tail.as_ref(), t.tail.as_ref().and_then(Tail::odx_id)))
			.collect();
		let tail = |kind: &str, value: &str| Tail {
			kind: kind.into(),
			value: value.into(),
		};
		assert_eq!(tails[0], (17, Some(&tail("2", "90001")), Some("IDE90001".into())));
		assert_eq!(tails[1], (3, Some(&tail("7", "01234")), Some("MAS01234".into())));
		// Read, letters and all, but not an ODX id this reader knows.
		assert_eq!(tails[2], (304, Some(&tail("9", "000B7")), None));
		assert_eq!(tails[3], (26, None, None));
	}

	#[test]
	fn only_a_five_digit_value_makes_an_odx_id() {
		let odx = |kind: &str, value: &str| {
			Tail {
				kind: kind.into(),
				value: value.into(),
			}
			.odx_id()
		};
		assert_eq!(odx("2", "00022"), Some("IDE00022".into()));
		assert_eq!(odx("2", "0022"), None);
		assert_eq!(odx("7", "0002A"), None);
		assert_eq!(odx("10", "00497"), None, "kind 10 is read, not interpreted");
	}

	#[test]
	fn a_comma_inside_a_name_does_not_move_the_tail() {
		let section = [line(7, "Boost pressure, actual,2,00022"), line(8, "Boost pressure, actual,")].concat();
		let table = read(section.as_bytes(), CodePage::Windows1252);
		assert_eq!(table.texts[0].name, "Boost pressure, actual");
		assert_eq!(table.texts[0].tail.as_ref().and_then(Tail::odx_id).as_deref(), Some("IDE00022"));
		assert_eq!(table.texts[1].name, "Boost pressure, actual");
		assert_eq!(table.texts[1].tail, None);
	}

	#[test]
	fn a_high_byte_is_read_in_the_sections_code_page() {
		// `0x96` is an en dash in Windows-1252; the cipher leaves it alone.
		let mut section = format!("{:06},", 4242).into_bytes();
		let alphabet = TableAlphabet::for_key(4242);
		section.extend(alphabet.encipher("invented stage 1 ").bytes());
		section.push(0x96);
		section.extend(alphabet.encipher(" on,7,00077").bytes());
		section.extend(b"\r\n");
		let table = read(&section, CodePage::Windows1252);
		assert_eq!(table.texts[0].name, "invented stage 1 \u{2013} on");
	}

	#[test]
	fn a_record_of_neither_shape_is_reported_not_guessed() {
		let section = [
			line(5, "no separator at all"),
			line(6, "a,,90001"),
			line(9, "a,2,00 1"),
			line(10, "fine,"),
		]
		.concat();
		let table = read(section.as_bytes(), CodePage::Windows1252);
		let ids: Vec<u32> = table.malformed.iter().map(|(id, _)| *id).collect();
		assert_eq!(ids, vec![5, 6, 9]);
		assert_eq!(table.malformed[0].1, "no separator at all", "what it read as is kept for the report");
		assert_eq!(table.texts.len(), 1);
	}

	#[test]
	fn a_line_without_an_id_is_not_a_record() {
		let section = format!("not a record\r\n,no id\r\n12a,x,\r\n\r\n{}", line(11, "fine,"));
		let table = read(section.as_bytes(), CodePage::Windows1252);
		assert_eq!(table.not_records, 3, "the blank line is not counted");
		assert_eq!(table.texts.len(), 1);
		assert!(table.malformed.is_empty());
	}

	#[test]
	fn the_neighbouring_key_does_not_read() {
		// The control the research measured: under `srand(id + 1)` a record is
		// noise. Enciphered for 18 and read as 17, the record must not come back.
		let mut section = line(18, "Invented flap position,2,90001");
		section.replace_range(..6, "000017");
		let table = read(section.as_bytes(), CodePage::Windows1252);
		assert!(table.texts.iter().all(|t| t.name != "Invented flap position"), "{table:?}");
	}
}
