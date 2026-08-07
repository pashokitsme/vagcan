//! Which object types this reader parses, which it refuses, and the dispatch
//! between them.
//!
//! The ODX schema `ODIS-project-explorer` documents has 84 types. Most are
//! irrelevant to a read-only tool, and a handful are worse than irrelevant.
//!
//! ## The refusal list is the safety rule, in code
//! `SAFETY.md` and the design's §2 name ten types that are **never** parsed
//! into anything executable, regardless of how easy parsing them would be:
//! flash jobs, access keys, single-ECU jobs, session start/stop, the
//! adaptation/coding `CASE` family and code information. They describe
//! flashing, access-key handshakes and write cases; the UDS allowlist is
//! `0x22`, `0x19`, `0x10`, `0x3E` and stays that way.
//!
//! [`refused`] answers for all ten by type code, [`load`] returns
//! [`Outcome::Refused`] for them before dispatch is even reached, and
//! `Refused` is a **unit variant** — it cannot carry a parsed payload, so
//! there is nothing for a caller to reach into even by mistake. Extending it
//! with a field would be the wrong shape; the name is available separately
//! from [`refused_type_name`] for messages.
//!
//! This is permanent, not a scoping decision for one pass. A loader for any
//! type on the list is out of scope for this project.
//!
//! ## The `CASE` family, and what refusing it costs
//! `DB_CASE`/`DB_CASES`/`DB_DEFAULT_CASE` are on the list *and* are inline
//! sub-objects of `MCD_DB_PARAMETER_MULTIPLEXER`, which is a measurement type.
//! So a multiplexed channel cannot be read, and is skipped rather than
//! reported wrong. That is the trade the rule asks for: on the reference
//! project's engine pool a multiplexer is 69 objects against 46,230 simple
//! data object properties, and no channel at all beats a channel decoded
//! through a code path that also knows how to describe a write.
//!
//! ## The access key, and the route taken around it
//! `MCD_ACCESS_KEY` is refused, and `ODIS-project-explorer` reaches an ECU
//! variant's layer data *through* one — `ecu.location_refs[0].access_key
//! .layer_data_object_id`. This reader does not: it finds the layer data by
//! scanning its pool for `DB_LAYER_DATA` objects and matching the one whose
//! `ecu_variant_ref` names the variant. Same answer, and no access key is ever
//! parsed. See [`identity::layer_data`].

use super::Error;
use super::object::Stream;

/// The type codes this reader knows, as the MCD kernel numbers them.
///
/// Only the types reached by the measurement and identification chains, plus
/// every type on the refusal list, are named. A code not here is
/// [`Outcome::Unsupported`] — a fact about this reader, distinct from
/// [`Outcome::Refused`], which is a decision about the format.
pub mod code {
	/// `DB_CASE` — a multiplexer's switch case. **Refused.**
	pub const DB_CASE: u16 = 0x0003;
	/// `DB_CASES` — a collection of them. **Refused.**
	pub const DB_CASES: u16 = 0x0004;
	/// `DB_COMPU_BASE` — one direction of a computation method.
	pub const DB_COMPU_BASE: u16 = 0x0005;
	/// `DB_COMPU_METHOD` — how a coded value becomes a physical one.
	pub const DB_COMPU_METHOD: u16 = 0x000A;
	/// `DB_COMPU_RATIONAL_COEFFS` — the numerator/denominator of a scale.
	pub const DB_COMPU_RATIONAL_COEFFS: u16 = 0x0014;
	/// `DB_COMPU_SCALE` — one interval of a computation method.
	pub const DB_COMPU_SCALE: u16 = 0x0019;
	/// `DB_COMPU_SCALES` — a collection of them.
	pub const DB_COMPU_SCALES: u16 = 0x001E;
	/// `DB_DEFAULT_CASE` — a multiplexer's fallback case. **Refused.**
	pub const DB_DEFAULT_CASE: u16 = 0x0020;
	/// `DB_DIAG_CODED_TYPE` — how a value sits in the response bytes.
	pub const DB_DIAG_CODED_TYPE: u16 = 0x0023;
	/// `DB_DOP_DTC` — a fault-code data object property.
	pub const DB_DOP_DTC: u16 = 0x0028;
	/// `DB_DOP_SIMPLE_BASE` — the scaling of one scalar channel.
	pub const DB_DOP_SIMPLE_BASE: u16 = 0x002C;
	/// `DB_LAYER_DATA` — a variant's index of everything it can do.
	pub const DB_LAYER_DATA: u16 = 0x0031;
	/// `DB_PROJECT_DATA` — a pool's index of the variants it holds.
	pub const DB_PROJECT_DATA: u16 = 0x0033;
	/// `DB_LIMIT` — one end of a value range.
	pub const DB_LIMIT: u16 = 0x0037;
	/// `DB_MATCHING_PARAMETER` — one identifier a variant pattern reads.
	pub const DB_MATCHING_PARAMETER: u16 = 0x0038;
	/// `DB_MATCHING_PARAMETERS` — a collection of them.
	pub const DB_MATCHING_PARAMETERS: u16 = 0x0039;
	/// `DB_PHYSICAL_TYPE` — the engineering type a value converts to.
	pub const DB_PHYSICAL_TYPE: u16 = 0x003C;
	/// `MCD_DB_CODE_INFORMATION` — an external code module. **Refused.**
	pub const MCD_DB_CODE_INFORMATION: u16 = 0x0041;
	/// `MCD_DB_CODE_INFORMATIONS` — a collection of them. **Refused.**
	pub const MCD_DB_CODE_INFORMATIONS: u16 = 0x0042;
	/// `MCD_ACCESS_KEY` — a security-access handshake. **Refused.**
	pub const MCD_ACCESS_KEY: u16 = 0x004D;
	/// `MCD_DB_DIAG_TROUBLE_CODE` — one fault code's identity.
	pub const MCD_DB_DIAG_TROUBLE_CODE: u16 = 0x0057;
	/// `MCD_DB_ECU_BASE_VARIANT` — the family a variant belongs to.
	pub const MCD_DB_ECU_BASE_VARIANT: u16 = 0x005A;
	/// `MCD_DB_ECU_VARIANT` — one control unit's exact software identity.
	pub const MCD_DB_ECU_VARIANT: u16 = 0x005C;
	/// `MCD_DB_REQUEST` — the bytes a service sends.
	pub const MCD_DB_REQUEST: u16 = 0x0078;
	/// `MCD_DB_REQUEST_PARAMETERS` — its parameter list.
	pub const MCD_DB_REQUEST_PARAMETERS: u16 = 0x0079;
	/// `MCD_DB_RESPONSE` — the bytes a service expects back.
	pub const MCD_DB_RESPONSE: u16 = 0x0091;
	/// `MCD_DB_RESPONSE_PARAMETERS` — its parameter list.
	pub const MCD_DB_RESPONSE_PARAMETERS: u16 = 0x0092;
	/// `MCD_DB_PARAMETER_MULTIPLEXER` — a channel whose shape varies.
	pub const MCD_DB_PARAMETER_MULTIPLEXER: u16 = 0x00A0;
	/// `MCD_DB_PARAMETER` — one field of a request or a response.
	pub const MCD_DB_PARAMETER: u16 = 0x00A4;
	/// `MCD_DB_PARAMETER_SIMPLE` — the same shape under another name.
	pub const MCD_DB_PARAMETER_SIMPLE: u16 = 0x00A5;
	/// `MCD_DB_MATCHING_REQUEST_PARAMETER` — an echo of the request.
	pub const MCD_DB_MATCHING_REQUEST_PARAMETER: u16 = 0x00A7;
	/// `MCD_DB_PARAMETER_STRUCT_FIELD` — a field of a structure.
	pub const MCD_DB_PARAMETER_STRUCT_FIELD: u16 = 0x00A8;
	/// `MCD_DB_PARAMETER_STRUCTURE` — a channel's internal layout.
	pub const MCD_DB_PARAMETER_STRUCTURE: u16 = 0x00AA;
	/// `MCD_DB_TABLE` — the set of channels one service can read.
	pub const MCD_DB_TABLE: u16 = 0x00AB;
	/// `MCD_DB_TABLE_PARAMETER` — one row of it.
	pub const MCD_DB_TABLE_PARAMETER: u16 = 0x00AC;
	/// `MCD_DB_PARAMETER_TABLESTRUCT` — the response field that is a row.
	pub const MCD_DB_PARAMETER_TABLESTRUCT: u16 = 0x00B0;
	/// `MCD_DB_PARAMETER_TABLE_KEY` — the response field that picks the row.
	pub const MCD_DB_PARAMETER_TABLE_KEY: u16 = 0x00B2;
	/// `MCD_DB_SERVICE` — one diagnostic service of a variant.
	pub const MCD_DB_SERVICE: u16 = 0x00BE;
	/// `MCD_DB_SINGLE_ECU_JOB` — a scripted procedure. **Refused.**
	pub const MCD_DB_SINGLE_ECU_JOB: u16 = 0x00BF;
	/// `MCD_DB_FLASH_JOB` — reprogramming. **Refused.**
	pub const MCD_DB_FLASH_JOB: u16 = 0x00F8;
	/// `MCD_DB_UNIT` — an engineering unit.
	pub const MCD_DB_UNIT: u16 = 0x0102;
	/// `MCD_DB_STARTCOMMUNICATION` — session control. **Refused.**
	pub const MCD_DB_STARTCOMMUNICATION: u16 = 0x0107;
	/// `MCD_DB_STOPCOMMUNICATION` — session control. **Refused.**
	pub const MCD_DB_STOPCOMMUNICATION: u16 = 0x0108;
	/// `MCD_DB_PHYSICAL_DIMENSION` — a unit's SI exponents.
	pub const MCD_DB_PHYSICAL_DIMENSION: u16 = 0x010C;
	/// `MCD_DB_ECU` — a control unit's name and where it lives.
	pub const MCD_DB_ECU: u16 = 0x010D;
}

/// The ten types this project will never parse into anything executable, with
/// their names.
///
/// Kept as one table so the rule can be read in one place and tested as a
/// whole.
pub const REFUSED: [(u16, &str); 10] = [
	(code::DB_CASE, "DB_CASE"),
	(code::DB_CASES, "DB_CASES"),
	(code::DB_DEFAULT_CASE, "DB_DEFAULT_CASE"),
	(code::MCD_DB_CODE_INFORMATION, "MCD_DB_CODE_INFORMATION"),
	(code::MCD_DB_CODE_INFORMATIONS, "MCD_DB_CODE_INFORMATIONS"),
	(code::MCD_ACCESS_KEY, "MCD_ACCESS_KEY"),
	(code::MCD_DB_SINGLE_ECU_JOB, "MCD_DB_SINGLE_ECU_JOB"),
	(code::MCD_DB_FLASH_JOB, "MCD_DB_FLASH_JOB"),
	(code::MCD_DB_STARTCOMMUNICATION, "MCD_DB_STARTCOMMUNICATION"),
	(code::MCD_DB_STOPCOMMUNICATION, "MCD_DB_STOPCOMMUNICATION"),
];

/// Whether a type code is on the permanent refusal list.
pub fn refused(type_code: u16) -> bool {
	REFUSED.iter().any(|&(code, _)| code == type_code)
}

/// The name of a refused type, for saying *what* was refused.
///
/// Deliberately separate from [`Outcome::Refused`], which carries nothing: a
/// message may name the type, but no parsed value ever escapes.
pub fn refused_type_name(type_code: u16) -> Option<&'static str> {
	REFUSED.iter().find(|&&(code, _)| code == type_code).map(|&(_, name)| name)
}

/// What came of asking for an object.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
	/// A parsed object.
	Object(Object),
	/// The type is on the permanent refusal list.
	///
	/// A **unit variant**, on purpose. There is no payload here and there
	/// never will be — a refused type is not parsed at all, so there is
	/// nothing to hand back and nothing a caller could reach into.
	Refused,
	/// This reader has no loader for the type. Unlike [`Outcome::Refused`],
	/// that is a gap rather than a decision, and it carries the code so it can
	/// be reported and, if it ever matters, filled.
	Unsupported(u16),
}

/// A parsed object, one variant per type this reader loads.
#[derive(Debug, Clone, PartialEq)]
pub enum Object {}

/// Parse one inflated member.
///
/// The type code decides everything: a refused type is not read at all — the
/// stream is left exactly where it was — an unknown one is reported as such,
/// and a known one is handed to the loader that transcribes its field order.
pub fn load(type_code: u16, _stream: &mut Stream<'_>) -> Result<Outcome, Error> {
	if refused(type_code) {
		return Ok(Outcome::Refused);
	}
	Ok(Outcome::Unsupported(type_code))
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::odis::object::Stream;
	use crate::odis::strings::Strings;

	/// The list exactly as `SAFETY.md` and the design's §2 write it. Spelled
	/// out here rather than derived from [`REFUSED`], so that deleting an
	/// entry from the table fails this test instead of quietly shrinking it.
	const THE_RULE: [&str; 10] = [
		"MCD_DB_FLASH_JOB",
		"MCD_ACCESS_KEY",
		"MCD_DB_SINGLE_ECU_JOB",
		"MCD_DB_STARTCOMMUNICATION",
		"MCD_DB_STOPCOMMUNICATION",
		"DB_CASE",
		"DB_CASES",
		"DB_DEFAULT_CASE",
		"MCD_DB_CODE_INFORMATION",
		"MCD_DB_CODE_INFORMATIONS",
	];

	/// The executable form of the project's central safety rule.
	///
	/// Every name on the list resolves to a type code, that code is refused,
	/// and refusing it yields `Outcome::Refused` and nothing else. This test
	/// must not be weakened: if a future type needs parsing it does not go on
	/// this list, and nothing on this list comes off it.
	#[test]
	fn every_name_on_the_refusal_list_is_refused() {
		let strings = Strings::default();
		for name in THE_RULE {
			let &(type_code, _) = REFUSED
				.iter()
				.find(|&&(_, listed)| listed == name)
				.unwrap_or_else(|| panic!("{name} is missing from the refusal table"));
			assert!(refused(type_code), "{name} ({type_code:#06x}) must be refused");
			assert_eq!(refused_type_name(type_code), Some(name));

			let body = vec![0u8; 64];
			let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
			assert_eq!(
				load(type_code, &mut stream).expect("refusing is not an error"),
				Outcome::Refused,
				"{name} must yield Refused"
			);
		}
	}

	/// `Refused` carries nothing, so there is no parsed payload to reach for.
	///
	/// Checked structurally: the variant is constructible with no arguments
	/// and equal to itself, which is only true of a unit variant. Giving it a
	/// field would stop this compiling.
	#[test]
	fn refused_carries_no_payload() {
		let refusal = Outcome::Refused;
		assert_eq!(refusal, Outcome::Refused);
		assert!(matches!(refusal, Outcome::Refused));
	}

	/// A refused type is not read at all — the cursor is where it started.
	/// A refusal that fell through into dispatch would move it.
	#[test]
	fn refusing_does_not_touch_the_stream() {
		let strings = Strings::default();
		let body = vec![0u8; 32];
		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let before = stream.remaining();
		assert_eq!(
			load(code::MCD_ACCESS_KEY, &mut stream).expect("refusing is not an error"),
			Outcome::Refused
		);
		assert_eq!(stream.remaining(), before, "a refused object must not be read even a byte deep");
	}

	/// The table has exactly ten entries, names the same ten types the rule
	/// does, and repeats no type code.
	#[test]
	fn the_refusal_list_is_exactly_the_rule() {
		assert_eq!(REFUSED.len(), THE_RULE.len());
		let mut names: Vec<&str> = REFUSED.iter().map(|&(_, n)| n).collect();
		names.sort_unstable();
		let mut expected: Vec<&str> = THE_RULE.to_vec();
		expected.sort_unstable();
		assert_eq!(names, expected, "the table and the rule must name the same ten types");

		let mut codes: Vec<u16> = REFUSED.iter().map(|&(c, _)| c).collect();
		codes.sort_unstable();
		codes.dedup();
		assert_eq!(codes.len(), REFUSED.len(), "no type code may appear twice");
	}

	/// A type nobody has written a loader for is `Unsupported`, not `Refused`.
	/// The two say different things and must not be conflated: one is a gap in
	/// this reader, the other a decision about the format.
	#[test]
	fn an_unknown_type_is_unsupported_not_refused() {
		let strings = Strings::default();
		let body = vec![0u8; 16];
		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		assert_eq!(
			load(0xFFFF, &mut stream).expect("an unknown type is not an error"),
			Outcome::Unsupported(0xFFFF)
		);
	}
}
