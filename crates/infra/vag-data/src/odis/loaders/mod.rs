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

pub mod identity;
pub mod measurement;

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
	/// `MCD_DB_PARAMETERS` — a bare parameter list, as a structure holds one.
	pub const MCD_DB_PARAMETERS: u16 = 0x006D;
	/// `MCD_AUDIENCE` — who is allowed to run something.
	pub const MCD_AUDIENCE: u16 = 0x0115;
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
	/// `MCD_DB_MATCHING_PATTERNS` — the ways to recognise one ECU variant.
	pub const MCD_DB_MATCHING_PATTERNS: u16 = 0x0201;
	/// `MCD_DB_MATCHING_PATTERN` — one of them.
	pub const MCD_DB_MATCHING_PATTERN: u16 = 0x0202;
	/// `MCD_DB_MATCHING_PARAMETERS` — the identifiers a pattern reads.
	pub const MCD_DB_MATCHING_PARAMETERS: u16 = 0x0039;
	/// `MCD_DB_MATCHING_PARAMETER` — one of them.
	pub const MCD_DB_MATCHING_PARAMETER: u16 = 0x0038;
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
///
/// `Object` is 264 bytes against `Unsupported`'s two, and clippy would have it
/// boxed. It is not, deliberately: `Object` is what nearly every one of the
/// 310,734 readings in a whole-project parse comes back as, so boxing puts an
/// allocation on the common path to shrink a variant that occurs twice in 717
/// ECU variants. The cost is a wide return value moved on the stack, which is
/// the cheaper half of that trade.
#[allow(clippy::large_enum_variant)]
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

/// A reference to another object, by pool and name.
///
/// Either half can be absent. A missing `pool` is the common case and means
/// "look this name up in the layer data's own index" — the writer omits the
/// pool when the target is reachable from the referring variant's inheritance
/// chain.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Ref {
	/// The target's ObjectID.
	pub object: Option<String>,
	/// The PoolID it lives in, when the writer bothered to say.
	pub pool: Option<String>,
}

/// Read a `DbObjectReference`: ObjectID, PoolID, and optionally a third name
/// and a string vector, both of which vary by call site.
pub fn reference(stream: &mut Stream<'_>, third: bool, strings: bool) -> Result<Ref, Error> {
	let object = stream.ascii()?.map(str::to_owned);
	let pool = stream.ascii()?.map(str::to_owned);
	if third {
		let _also = stream.ascii()?;
	}
	if strings {
		string_list(stream)?;
	}
	Ok(Ref { object, pool })
}

/// Read a `DbAttributedObjectReference`: a reference plus a byte-counted list
/// of attribute names.
pub fn attributed_reference(stream: &mut Stream<'_>) -> Result<Ref, Error> {
	let object = stream.ascii()?.map(str::to_owned);
	let pool = stream.ascii()?.map(str::to_owned);
	string_list(stream)?;
	Ok(Ref { object, pool })
}

/// Read a `DbDiagComObjectReference`: an attributed reference, a number, an
/// object-type enum, and an optional string vector.
pub fn diag_com_reference(stream: &mut Stream<'_>) -> Result<Ref, Error> {
	let target = attributed_reference(stream)?;
	let _number = stream.u8()?;
	let _mcd_object_type = stream.u16()?;
	if stream.flag()? {
		let count = stream.count()?;
		for _ in 0..count {
			let _name = stream.ascii()?;
		}
	}
	Ok(target)
}

/// Read a byte-counted list of pooled names, discarding them.
fn string_list(stream: &mut Stream<'_>) -> Result<(), Error> {
	let count = stream.u8()?;
	for _ in 0..count {
		let _name = stream.ascii()?;
	}
	Ok(())
}

/// Read a `DbNamedObjectReferences` collection: `(name, reference)` pairs.
///
/// The reference here carries **two** names, not three. `load_reference`'s own
/// default is three, and transcribing that default into this collection was the
/// defect that made every `.bv` pool unreadable: the extra four bytes per entry
/// walked the cursor off the end of a 402-entry list and into the access keys
/// beyond it. The reference implementation passes `third_string=False` here,
/// and the real files agree — read three names and entry *n*'s object id is
/// entry *n-1*'s pool id.
pub fn named_references(stream: &mut Stream<'_>) -> Result<Vec<(Option<String>, Ref)>, Error> {
	let count = stream.count()?;
	let mut out = Vec::with_capacity(count.min(1024));
	for _ in 0..count {
		let name = stream.ascii()?.map(str::to_owned);
		out.push((name, reference(stream, false, false)?));
	}
	Ok(out)
}

/// Read a map from a pooled name to a reference.
pub fn reference_map(stream: &mut Stream<'_>, strings_in_reference: bool) -> Result<Vec<(Option<String>, Ref)>, Error> {
	let count = stream.count()?;
	let mut out = Vec::with_capacity(count.min(4096));
	for _ in 0..count {
		let key = stream.ascii()?.map(str::to_owned);
		out.push((key, reference(stream, false, strings_in_reference)?));
	}
	Ok(out)
}

/// Read a map from a pooled name to a `DbDiagComObjectReference`.
pub fn diag_com_reference_map(stream: &mut Stream<'_>) -> Result<Vec<(Option<String>, Ref)>, Error> {
	let count = stream.count()?;
	let mut out = Vec::with_capacity(count.min(4096));
	for _ in 0..count {
		let key = stream.ascii()?.map(str::to_owned);
		out.push((key, diag_com_reference(stream)?));
	}
	Ok(out)
}

/// Read a map from a pooled name to a list of pooled names, discarding it.
pub fn string_vector_map(stream: &mut Stream<'_>) -> Result<(), Error> {
	let count = stream.count()?;
	for _ in 0..count {
		let _key = stream.ascii()?;
		let inner = stream.count()?;
		for _ in 0..inner {
			let _name = stream.ascii()?;
		}
	}
	Ok(())
}

/// Read a list of pooled names.
pub fn name_list(stream: &mut Stream<'_>) -> Result<Vec<String>, Error> {
	let count = stream.count()?;
	let mut out = Vec::with_capacity(count.min(4096));
	for _ in 0..count {
		if let Some(name) = stream.ascii()? {
			out.push(name.to_owned());
		}
	}
	Ok(out)
}

/// How many bytes an `MCD_ACCESS_KEY` occupies after its two-byte type code.
///
/// Seven pooled names, a two-byte location type, and one more pooled name:
/// `7*4 + 2 + 4`. A fixed shape with no length field, which is why the number
/// has to be written down rather than read.
const ACCESS_KEY_BYTES: usize = 34;

/// Step over an access key without parsing it.
///
/// **This is not a loader and must never become one.** It returns `()`. It
/// builds nothing, keeps nothing, and hands nothing back — the only thing it
/// changes is the cursor. `MCD_ACCESS_KEY` stays on [`REFUSED`], [`load`] still
/// answers [`Outcome::Refused`] for it, and no security-access handshake ever
/// reaches a caller.
///
/// It exists because the refusal cannot be enforced by stopping. Every one of
/// the reference project's 54 base-variant pools embeds access keys inside
/// `DB_PROJECT_DATA` — the object that holds the ECU variant list — and inside
/// every `MCD_DB_ECU`. Refusing to move past them refuses the variant list too,
/// which is not a safety property, just an inability to read the car. The object
/// stream carries no lengths ([`super::object`]), so "move past" has to be
/// spelled as a byte count.
pub fn skip_access_key(stream: &mut Stream<'_>) -> Result<(), Error> {
	let found = stream.u16()?;
	if found != code::MCD_ACCESS_KEY {
		return Err(Error::Format(format!(
			"an access key slot holds type {found:#06x}, not {:#06x}",
			code::MCD_ACCESS_KEY
		)));
	}
	stream.bytes(ACCESS_KEY_BYTES)?;
	Ok(())
}

/// Step over a location reference's list of access keys.
///
/// A location reference is `(ObjectID, PoolID, count)` then that many access
/// keys. Both `DB_PROJECT_DATA` and `MCD_DB_ECU` carry a list of these.
pub fn skip_location_reference(stream: &mut Stream<'_>) -> Result<(), Error> {
	let _object = stream.ascii()?;
	let _pool = stream.ascii()?;
	let keys = stream.u8()?;
	for _ in 0..keys {
		skip_access_key(stream)?;
	}
	Ok(())
}

/// Read an optionally-present nested object of a known type.
///
/// The slot is a flag, then — if set — a whole object: its own two-byte type
/// code, its fields, and its own terminator. `load` is responsible for the
/// fields and the terminator; this is responsible for the flag, the code and
/// the refusal check.
pub fn nested<T>(stream: &mut Stream<'_>, expect: u16, load: impl FnOnce(&mut Stream<'_>) -> Result<T, Error>) -> Result<Option<T>, Error> {
	nested_any(stream, |found, stream| {
		if found != expect {
			return Err(Error::Format(format!(
				"a nested object is type {found:#06x} where {expect:#06x} was expected"
			)));
		}
		load(stream)
	})
}

/// Read an optionally-present nested object whose type is not fixed.
///
/// Several slots are polymorphic — a parameter can be any of five types, a
/// data object property any of three — so the type code decides which loader
/// runs. A code on [`REFUSED`] stops the parse with [`Error::Refused`]: the
/// object cannot be skipped (nothing in the stream says how long it is) and it
/// will not be parsed, so the only honest outcome is to stop and say which
/// type stopped it.
pub fn nested_any<T>(stream: &mut Stream<'_>, load: impl FnOnce(u16, &mut Stream<'_>) -> Result<T, Error>) -> Result<Option<T>, Error> {
	if !stream.flag()? {
		return Ok(None);
	}
	let found = stream.u16()?;
	if let Some(name) = refused_type_name(found) {
		return Err(Error::Refused(name));
	}
	load(found, stream).map(Some)
}

/// A parsed object, one variant per type this reader loads.
#[derive(Debug, Clone, PartialEq)]
pub enum Object {
	/// `DB_DOP_SIMPLE_BASE` — a scalar channel's coded shape and scaling.
	Dop(measurement::Dop),
	/// `MCD_DB_PARAMETER` and its four aliases.
	Parameter(measurement::Parameter),
	/// `MCD_DB_PARAMETER_STRUCTURE` — a channel's internal layout.
	Structure(measurement::Structure),
	/// `MCD_DB_RESPONSE` — a service's positive response.
	Response(measurement::Response),
	/// `MCD_DB_SERVICE` — one diagnostic service.
	Service(measurement::Service),
	/// `MCD_DB_TABLE` — the set of channels a service can read.
	Table(measurement::Table),
	/// `MCD_DB_TABLE_PARAMETER` — one row of such a table.
	TableRow(measurement::TableRow),
	/// `MCD_DB_UNIT` — an engineering unit.
	Unit(measurement::Unit),
	/// `DB_PROJECT_DATA` — a pool's index of the variants it holds.
	ProjectData(identity::ProjectData),
	/// `DB_LAYER_DATA` — one layer's index of what it can reach.
	LayerData(identity::LayerData),
	/// `MCD_DB_ECU_VARIANT` — one control unit's exact software identity.
	EcuVariant(identity::EcuVariant),
}

/// Parse one inflated member.
///
/// The type code decides everything: a refused type is not read at all — the
/// stream is left exactly where it was — an unknown one is reported as such,
/// and a known one is handed to the loader that transcribes its field order.
///
/// The terminator is consumed **here**, once, for the member as a whole.
/// Nested objects do not carry one: only the outermost object in a `.db`
/// member is terminated, and a loader that consumed a terminator of its own
/// would eat the first bytes of the field after it.
pub fn load(type_code: u16, stream: &mut Stream<'_>) -> Result<Outcome, Error> {
	if refused(type_code) {
		return Ok(Outcome::Refused);
	}
	let object = match type_code {
		code::DB_DOP_SIMPLE_BASE => Object::Dop(measurement::dop(stream)?),
		code::MCD_DB_PARAMETER
		| code::MCD_DB_PARAMETER_SIMPLE
		| code::MCD_DB_PARAMETER_TABLE_KEY
		| code::MCD_DB_PARAMETER_TABLESTRUCT
		| code::MCD_DB_MATCHING_REQUEST_PARAMETER => Object::Parameter(measurement::parameter(stream, type_code)?),
		code::MCD_DB_PARAMETER_STRUCTURE => Object::Structure(measurement::structure(stream)?),
		code::MCD_DB_RESPONSE => Object::Response(measurement::response(stream)?),
		code::MCD_DB_SERVICE => Object::Service(measurement::service(stream)?),
		code::MCD_DB_TABLE => Object::Table(measurement::table(stream)?),
		code::MCD_DB_TABLE_PARAMETER => Object::TableRow(measurement::table_row(stream)?),
		code::MCD_DB_UNIT => Object::Unit(measurement::unit(stream)?),
		code::DB_PROJECT_DATA => Object::ProjectData(identity::project_data(stream)?),
		// A layer whose tail this reader cannot follow consumed its own
		// terminator inside the loader, or deliberately did not reach one.
		code::DB_LAYER_DATA => {
			let layer = identity::layer_data(stream)?;
			return Ok(Outcome::Object(Object::LayerData(layer)));
		}
		code::MCD_DB_ECU_VARIANT => Object::EcuVariant(identity::ecu_variant(stream)?),
		other => return Ok(Outcome::Unsupported(other)),
	};
	stream.end()?;
	Ok(Outcome::Object(object))
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
