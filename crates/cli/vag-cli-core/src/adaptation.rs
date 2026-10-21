//! A unit's adaptation channels: which identifiers hold them, where each field sits in
//! the answer, and how its bytes read.
//!
//! Two sources describe them, as they do the channels: an ODIS
//! project's adaptation read (`DiagnServi_ReadDataByIdentCalibData`, cached in
//! `adaptation_channel`), then a VCDS installation's `RA.rod [ADP]` rows (`registry_row`,
//! `research/vcds-registry/README.md` §9.1). A run reads one of the two, never both mixed
//! (CLAUDE.md, "ODIS and VCDS are independent sources"):
//! [`crate::extracted::Extracted::adaptations`] picks it; this module is the row and its
//! rendering.
//!
//! **Read-only.** An adaptation's value is read with `0x22` on its DID, which is all
//! this describes. Nothing here can say how to write one.

use vag_data_labels::catalog::{MeasurementDef, ReadId, Scaling};
use vag_data_labels::measure::{LinearScale, RawForm};

/// `RM.rod`'s value types a VCDS adaptation row is read by (`flags & 63`,
/// `vag_data_labels::registries::Value::kind`). The rest are shown as bytes.
const LINEAR: u8 = 0;
const IDENTITY: u8 = 2;
const TEXT_TABLE: u8 = 3;
const ASCII: u8 = 8;

/// One field of one adaptation identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct Adaptation {
	/// The identifier `0x22` reads it with.
	pub did: u16,
	/// Bits into the answer after `62 <DID>`.
	pub bit_offset: u32,
	/// How many bits; `None` where the source gives no length: to the end of the answer.
	pub bit_length: Option<u32>,
	/// What it is called: the glossary's or the label files' wording for its text id,
	/// else its source's own name.
	pub name: String,
	/// The engineering unit; empty when it has none.
	pub unit: String,
	pub text_id: Option<String>,
	pub form: Form,
	/// Which kind of source described it ([`vag_data_db::ODIS`], [`vag_data_db::VCDS`]).
	pub source: &'static str,
	/// The source's variant the row came from: an ODIS variant or a VCDS file stem.
	pub variant: String,
}

/// How a field's bytes become what is shown.
#[derive(Debug, Clone, PartialEq)]
pub enum Form {
	/// A number or a state, as a channel reads.
	Value(MeasurementDef),
	/// Text, as the bytes say it.
	Text,
	/// No reading the source describes: the bytes, in hex.
	Bytes,
}

impl Adaptation {
	/// An ODIS adaptation row. A field no [`RawForm`] can say exactly — wider than four
	/// bytes, or crossing a byte inside — is shown as its bytes.
	pub fn from_odis(reading: &vag_data_labels::odis::Reading, variant: &str) -> Adaptation {
		let form = match RawForm::for_field(reading.bit_offset, reading.bit_length, reading.signed, reading.big_endian) {
			Some(raw_form) => Form::Value(MeasurementDef {
				name: reading.name.clone().into(),
				unit: String::new().into(),
				address: ReadId::Uds(reading.did),
				raw_form,
				scaling: reading.scaling.clone(),
			}),
			None => Form::Bytes,
		};
		Adaptation {
			did: reading.did,
			bit_offset: reading.bit_offset,
			bit_length: Some(reading.bit_length),
			name: reading.name.clone(),
			unit: reading.unit.clone().unwrap_or_default(),
			text_id: reading.text_id.clone(),
			form,
			source: vag_data_db::ODIS,
			variant: variant.to_string(),
		}
	}

	/// A VCDS `RA.rod` row. `None` for one with no identifier that fits a DID.
	pub fn from_vcds(entry: &vag_data_db::RegistryEntry, variant: &str) -> Option<Adaptation> {
		let did = u16::try_from(entry.identifier?).ok()?;
		let bit_offset = entry.bit_offset.unwrap_or(0);
		let scaling = match (entry.kind, entry.scale) {
			(Some(LINEAR | IDENTITY), Some((factor, offset))) => Some(Scaling::Linear(LinearScale { factor, offset })),
			(Some(TEXT_TABLE), _) if !entry.levels.is_empty() => Some(Scaling::Enum {
				levels: entry.levels.clone(),
			}),
			_ => None,
		};
		let raw_form = entry
			.bit_length
			.and_then(|length| RawForm::for_field(bit_offset, length, entry.signed, entry.big_endian));
		let name = entry.name.clone().unwrap_or_else(|| format!("{did:04X}"));
		let form = match (scaling, raw_form, entry.kind) {
			(Some(scaling), Some(raw_form), _) => Form::Value(MeasurementDef {
				name: name.clone().into(),
				unit: String::new().into(),
				address: ReadId::Uds(did),
				raw_form,
				scaling,
			}),
			(_, _, Some(ASCII)) => Form::Text,
			_ => Form::Bytes,
		};
		Some(Adaptation {
			did,
			bit_offset,
			bit_length: entry.bit_length,
			name,
			unit: entry.unit.clone().unwrap_or_default(),
			text_id: entry.text_id.clone(),
			form,
			source: vag_data_db::VCDS,
			variant: variant.to_string(),
		})
	}

	/// Whether two rows describe the same field: the same DID, starting at the same bit.
	pub fn same_field(&self, other: &Adaptation) -> bool {
		self.did == other.did && self.bit_offset == other.bit_offset
	}

	/// The field's bytes in an answer's data (after `62 <DID>`), when it starts and ends on
	/// a byte and the answer is long enough.
	fn bytes<'a>(&self, data: &'a [u8]) -> Option<&'a [u8]> {
		if self.bit_offset % 8 != 0 {
			return None;
		}
		let start = (self.bit_offset / 8) as usize;
		let end = match self.bit_length {
			Some(length) if length % 8 == 0 => start + (length / 8) as usize,
			Some(_) => return None,
			None => data.len(),
		};
		data.get(start..end)
	}

	/// What to show for this field, given the unit's answer to its DID.
	///
	/// A state the source does not list is shown by its number, never guessed at; bytes
	/// that will not read as the source describes are shown as bytes.
	pub fn value(&self, data: &[u8]) -> String {
		match &self.form {
			Form::Value(def) => match def.describe(data) {
				Some(text) => text,
				None => match def.raw_form.read(data) {
					Some(raw) => format!("{raw} (not a listed state)"),
					None => SHORT.to_string(),
				},
			},
			Form::Text => match self.bytes(data) {
				Some(bytes) => String::from_utf8_lossy(bytes).trim_end_matches(['\0', ' ']).to_string(),
				None => self.undecoded(data),
			},
			Form::Bytes => match self.bytes(data) {
				Some(bytes) => crate::verbosity::hex(bytes),
				None => self.undecoded(data),
			},
		}
	}

	/// A field with no decoded form that does not start and end on a byte: where its bits
	/// sit, and the bytes that cover them, in hex — `(3 bits at bit 9, not decoded: 02)`.
	/// [`SHORT`] only when the answer does not reach the field.
	fn undecoded(&self, data: &[u8]) -> String {
		let first = (self.bit_offset / 8) as usize;
		let (covering, length) = match self.bit_length {
			Some(length) => (data.get(first..(self.bit_offset + length).div_ceil(8) as usize), format!("{length} bits")),
			None => (data.get(first..).filter(|rest| !rest.is_empty()), "the rest".to_string()),
		};
		match covering {
			Some(bytes) => format!("({length} at bit {}, not decoded: {})", self.bit_offset, crate::verbosity::hex(bytes)),
			None => SHORT.to_string(),
		}
	}
}

impl Adaptation {
	/// [`Self::value`] with the unit after it, where the value is a number the unit
	/// belongs to: a state's name, text and bytes take none.
	pub fn shown(&self, data: &[u8]) -> String {
		let value = self.value(data);
		let numeric = matches!(&self.form, Form::Value(def) if !matches!(def.scaling, Scaling::Enum { .. })) && def_reads(&self.form, data);
		match numeric && !self.unit.is_empty() {
			true => format!("{value} {}", self.unit),
			false => value,
		}
	}
}

/// Whether a value form turns these bytes into a number at all.
fn def_reads(form: &Form, data: &[u8]) -> bool {
	matches!(form, Form::Value(def) if def.interpret(data).is_some())
}

/// What a field shows when the answer does not reach it.
const SHORT: &str = "(answer too short)";

/// The identifiers to ask for these rows, each once, in order: the whole bounded list a
/// read of them makes.
pub fn identifiers(rows: &[Adaptation]) -> Vec<u16> {
	let mut dids: Vec<u16> = rows.iter().map(|a| a.did).collect();
	dids.sort_unstable();
	dids.dedup();
	dids
}

#[cfg(test)]
mod tests {
	use super::*;
	use vag_data_labels::Level;

	// Every row here is invented.

	fn odis(did: u16, bit_offset: u32, bit_length: u32, scaling: Scaling, text_id: &str) -> Adaptation {
		let reading = vag_data_labels::odis::Reading {
			did,
			name: format!("odis {did:04X}"),
			unit: Some("s".into()),
			bit_offset,
			bit_length,
			signed: false,
			big_endian: true,
			scaling,
			text_id: Some(text_id.into()),
		};
		Adaptation::from_odis(&reading, "EV_Test")
	}

	fn vcds(did: u32, kind: u8, bit_offset: u32, bit_length: Option<u32>) -> vag_data_db::RegistryEntry {
		vag_data_db::RegistryEntry {
			identifier: Some(did),
			ordinal: None,
			kind: Some(kind),
			signed: false,
			big_endian: true,
			bit_offset: Some(bit_offset),
			bit_length,
			scale: Some((0.5, 1.0)),
			unit: Some("%".into()),
			name: Some(format!("vcds {did:04X}")),
			text_id: Some(format!("IDE{did:05}")),
			levels: vec![Level::point(0, "off"), Level::point(1, "on")],
		}
	}

	fn linear() -> Scaling {
		Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 })
	}

	#[test]
	fn a_field_reads_as_its_source_describes_it() {
		let states = Scaling::Enum {
			levels: vec![Level::point(0, "aus"), Level::point(1, "ein")],
		};
		// A one-bit state at bit 1 of the second byte.
		let flag = odis(0x0550, 9, 1, states, "IDE1");
		assert_eq!(flag.value(&[0x00, 0b0000_0010]), "ein");
		assert_eq!(flag.value(&[0x00, 0x00]), "aus");
		// A state's name takes no unit; a number takes its own.
		assert_eq!(flag.shown(&[0x00, 0x00]), "aus");
		assert_eq!(odis(0x0552, 0, 16, linear(), "IDE3").shown(&[0x01, 0x00]), "256 s");
		assert_eq!(odis(0x0552, 0, 16, linear(), "IDE3").shown(&[0x01]), SHORT);
		// A state the source does not list is its number.
		let enumerated = odis(
			0x0551,
			0,
			8,
			Scaling::Enum {
				levels: vec![Level::point(0, "aus")],
			},
			"IDE2",
		);
		assert_eq!(enumerated.value(&[7]), "7 (not a listed state)");
		// A number, without its unit: the unit is its own column.
		assert_eq!(odis(0x0552, 0, 16, linear(), "IDE3").value(&[0x01, 0x00]), "256");
		// A field the answer does not reach.
		assert_eq!(odis(0x0552, 0, 16, linear(), "IDE3").value(&[0x01]), SHORT);
		// Wider than four bytes: the bytes.
		assert_eq!(odis(0x0553, 0, 48, linear(), "IDE4").value(b"ABCDEF"), "41 42 43 44 45 46");
	}

	#[test]
	fn a_vcds_row_reads_by_its_type() {
		let linear = Adaptation::from_vcds(&vcds(0x1075, LINEAR, 0, Some(8)), "EV_X").unwrap();
		assert_eq!((linear.value(&[10]).as_str(), linear.unit.as_str()), ("6", "%"));
		let table = Adaptation::from_vcds(&vcds(0x1076, TEXT_TABLE, 0, Some(8)), "EV_X").unwrap();
		assert_eq!(table.value(&[1]), "on");
		let text = Adaptation::from_vcds(&vcds(0x1077, ASCII, 8, None), "EV_X").unwrap();
		assert_eq!(text.value(b"xAB12 \0"), "AB12");
		let raw = Adaptation::from_vcds(&vcds(0x1078, 7, 0, Some(16)), "EV_X").unwrap();
		assert_eq!(raw.value(&[0xDE, 0xAD]), "DE AD");
		// No identifier that is a DID: no row.
		assert!(Adaptation::from_vcds(&vcds(0x1_0000, LINEAR, 0, Some(8)), "EV_X").is_none());
	}

	/// A field off the byte grid with no form to decode it is not "too short" on a full
	/// answer: it says where its bits are and shows the bytes that hold them.
	#[test]
	fn a_field_off_the_byte_grid_shows_its_bits_not_a_short_answer() {
		let flags = Adaptation::from_vcds(&vcds(0x1079, 7, 9, Some(3)), "EV_X").unwrap();
		assert_eq!(flags.form, Form::Bytes);
		assert_eq!(flags.value(&[0x00, 0b0000_1110]), "(3 bits at bit 9, not decoded: 0E)");
		// Across a byte boundary: both bytes.
		let across = Adaptation::from_vcds(&vcds(0x107A, 7, 6, Some(4)), "EV_X").unwrap();
		assert_eq!(across.value(&[0xC0, 0x03, 0xFF]), "(4 bits at bit 6, not decoded: C0 03)");
		// The answer does not reach it: that is a short answer.
		assert_eq!(flags.value(&[0x00]), SHORT);
		// No length, off the grid: from its byte to the end.
		let rest = Adaptation::from_vcds(&vcds(0x107B, 7, 4, None), "EV_X").unwrap();
		assert_eq!(rest.value(&[0xAB, 0xCD]), "(the rest at bit 4, not decoded: AB CD)");
	}

	#[test]
	fn the_identifiers_are_each_asked_once_in_order() {
		let rows = [
			odis(0x0600, 8, 8, linear(), "IDE2"),
			odis(0x0500, 0, 8, linear(), "IDE1"),
			odis(0x0600, 0, 8, linear(), "IDE3"),
		];
		assert_eq!(identifiers(&rows), [0x0500, 0x0600]);
	}
}
