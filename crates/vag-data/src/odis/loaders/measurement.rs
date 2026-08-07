//! The measurement chain: from a variant's `ReadDataByIdentifier` service down
//! to the scaling of one channel.
//!
//! Every loader here is a literal transcription of one type's field order.
//! There are no tags in an object stream ([`super::super::object`]), so the
//! order *is* the format: a field read at the wrong width silently shifts
//! everything after it, and the only thing that catches it is the terminator
//! at the end of the whole member. That is why loaders read fields they have
//! no use for — a field skipped is a cursor moved wrong.
//!
//! No loader here consumes a terminator. Only the **outermost** object of a
//! `.db` member is terminated; a nested one runs straight into the field that
//! follows it. [`super::load`] consumes the one terminator there is.
//!
//! ## The chain
//! ```text
//! DB_LAYER_DATA           the variant's index of what it can do
//!  └ MCD_DB_SERVICE       "DiagnServi_ReadDataByIdentMeasuValue"
//!     └ MCD_DB_RESPONSE   the positive response's parameter list
//!        ├ …TABLE_KEY     byte 1 of the response: the DID
//!        │  └ MCD_DB_TABLE → DB_DOP_SIMPLE_BASE (a TEXTTABLE: DID → name)
//!        └ …TABLESTRUCT   byte 3 of the response: the measurement
//!           └ MCD_DB_TABLE → MCD_DB_TABLE_PARAMETER, one row per DID
//!              └ MCD_DB_PARAMETER → MCD_DB_PARAMETER_STRUCTURE
//!                 └ MCD_DB_PARAMETER → DB_DOP_SIMPLE_BASE
//!                    ├ DB_DIAG_CODED_TYPE   where the bits are
//!                    ├ DB_PHYSICAL_TYPE     what they become
//!                    └ DB_COMPU_METHOD      how — see [`super::super::compu`]
//! ```
//!
//! Byte positions 1 and 3 are not this reader's invention and not a property
//! of one car: a UDS `0x22` positive response is `62 <DID hi> <DID lo>` then
//! data, so the identifier is at byte 1 and the payload at byte 3. The file
//! says so too — it is checked, not assumed.

use super::super::Error;
use super::super::compu::{self, Method};
use super::super::object::{Stream, Value};
use super::{Ref, code, named_references, nested, nested_any, reference};

/// `DB_DIAG_CODED_TYPE`'s four length rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodedLength {
	/// The length is in the bits at the head of the parameter.
	Leading,
	/// The length lies between a minimum and a maximum, with a terminator.
	MinMax,
	/// A fixed bit length.
	Standard,
	/// The length is in another parameter.
	ParamInfo,
}

/// The base data types a coded or physical value can have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
	/// A signed 32-bit integer.
	Int32,
	/// An unsigned 32-bit integer.
	Uint32,
	/// A 32-bit float.
	Float32,
	/// A 64-bit float.
	Float64,
	/// A Windows-1252 / ISO-8859 string.
	AsciiString,
	/// A UTF-8 string.
	Utf8String,
	/// A UTF-16 string.
	Unicode2String,
	/// Raw bytes.
	ByteField,
	/// Raw bits.
	BitField,
}

impl DataType {
	/// Read the one-byte `EDbDataType`.
	fn read(stream: &mut Stream<'_>) -> Result<DataType, Error> {
		Ok(match stream.u8()? {
			0 => DataType::Int32,
			1 => DataType::Uint32,
			2 => DataType::Float32,
			3 => DataType::Float64,
			4 => DataType::AsciiString,
			5 => DataType::Utf8String,
			6 => DataType::Unicode2String,
			7 => DataType::ByteField,
			8 => DataType::BitField,
			other => return Err(Error::Format(format!("data type {other} is not one of the nine ODX defines"))),
		})
	}

	/// Read the one-byte `EDbPhysicalDataType`, a narrower enum over the same
	/// names but numbered differently. Reading one as the other is a silent
	/// wrong answer, so they are separate functions on purpose.
	fn read_physical(stream: &mut Stream<'_>) -> Result<DataType, Error> {
		Ok(match stream.u8()? {
			0 => DataType::Int32,
			1 => DataType::Uint32,
			2 => DataType::Float32,
			3 => DataType::Float64,
			4 => DataType::Unicode2String,
			5 => DataType::ByteField,
			other => return Err(Error::Format(format!("physical data type {other} is not one of the six ODX defines"))),
		})
	}

	/// Whether a value of this type is signed, which is what a decoder needs
	/// to know beyond its bit length.
	pub fn is_signed(self) -> bool {
		matches!(self, DataType::Int32)
	}
}

/// `DB_DIAG_CODED_TYPE`: where a value sits in the response bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct CodedType {
	/// Which length rule applies.
	pub length: CodedLength,
	/// The bit length, for every rule but [`CodedLength::MinMax`].
	pub bits: Option<u32>,
	/// The type of the coded value.
	pub base: DataType,
	/// Whether the bytes run most-significant first.
	pub high_low_byte_order: bool,
}

/// `DB_PHYSICAL_TYPE`: what a coded value becomes.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicalType {
	/// The engineering type.
	pub base: DataType,
	/// Digits after the decimal point, when the file says.
	pub precision: Option<u16>,
	/// 2, 8, 10 or 16 — how an integer is meant to be displayed.
	pub radix: u8,
}

/// `DB_DOP_SIMPLE_BASE`: one scalar channel's coded shape and scaling.
#[derive(Debug, Clone, PartialEq)]
pub struct Dop {
	/// The short name, as the file spells it.
	pub short_name: Option<String>,
	/// How the coded value converts. Absent means no conversion is defined.
	pub compu: Option<Method>,
	/// Where the bits are.
	pub coded: Option<CodedType>,
	/// What they become.
	pub physical: Option<PhysicalType>,
	/// The engineering unit, by reference.
	pub unit: Option<Ref>,
}

/// `MCD_DB_PARAMETER` and its four aliases: one field of a request or response.
#[derive(Debug, Clone, PartialEq)]
pub struct Parameter {
	/// Which type code the object actually carried.
	pub type_code: u16,
	/// The human name, from the Unicode pool.
	pub long_name: Option<String>,
	/// The text id of that name — the join to `TTTEXT`.
	pub long_name_id: Option<String>,
	/// The description, from the Unicode pool.
	pub description: Option<String>,
	/// The short name, which is how a table struct names its key.
	pub short_name: Option<String>,
	/// Bits into the byte at [`Parameter::byte_position`].
	pub bit_position: u8,
	/// Bytes into the enclosing PDU or structure, when the file gives one.
	pub byte_position: Option<u32>,
	/// The data object property this field converts through.
	pub dop: Option<Ref>,
	/// The table a `TABLE-KEY` or `TABLE-STRUCT` field indexes.
	pub table: Option<Ref>,
	/// A `TABLE-KEY` whose table is stored inline rather than referenced.
	pub inline_table: Option<Box<Table>>,
	/// The `TABLE-KEY` a `TABLE-STRUCT` is keyed by, named by short name.
	pub key_short_name: Option<String>,
}

/// `MCD_DB_PARAMETER_STRUCTURE`: a channel's internal layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Structure {
	/// The short name.
	pub short_name: Option<String>,
	/// The human name.
	pub long_name: Option<String>,
	/// How many bytes the whole structure occupies.
	pub bytes: u16,
	/// Its fields, in file order.
	pub fields: Vec<Parameter>,
}

/// `MCD_DB_RESPONSE`: what a service expects back.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
	/// The short name.
	pub short_name: Option<String>,
	/// Its parameters, in file order.
	pub parameters: Vec<Parameter>,
}

/// `MCD_DB_SERVICE`: one diagnostic service of a variant.
#[derive(Debug, Clone, PartialEq)]
pub struct Service {
	/// The service's own identifier.
	pub id: Option<String>,
	/// The short name.
	pub short_name: Option<String>,
	/// The request it sends.
	pub request: Option<Ref>,
	/// The positive responses it accepts.
	pub positive_responses: Vec<Ref>,
}

/// `MCD_DB_TABLE`: the set of channels one service can read.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
	/// The short name.
	pub short_name: Option<String>,
	/// `(row key, the row's object)` — the key being the channel's human name,
	/// which is also what the table key's text table maps a DID to.
	pub rows: Vec<(Option<String>, Ref)>,
	/// The data object property that decodes the key, i.e. the DID.
	pub key_dop: Option<Ref>,
}

/// `MCD_DB_TABLE_PARAMETER`: one row of such a table.
#[derive(Debug, Clone, PartialEq)]
pub struct TableRow {
	/// The row's key, matching [`Table::rows`].
	pub key: Option<String>,
	/// The parameter that describes the row's payload.
	pub parameter: Parameter,
}

/// `MCD_DB_UNIT`: an engineering unit.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
	/// The short name.
	pub short_name: Option<String>,
	/// The human name.
	pub long_name: Option<String>,
	/// What a tester actually prints — `°C`, `/min`. This is the one wanted.
	pub display_name: Option<String>,
	/// The text id of [`Unit::long_name`].
	pub long_name_id: Option<String>,
}

/// Read a `DB_DIAG_CODED_TYPE`.
pub fn coded_type(stream: &mut Stream<'_>) -> Result<CodedType, Error> {
	let length = match stream.u8()? {
		0 => CodedLength::Leading,
		1 => CodedLength::MinMax,
		2 => CodedLength::Standard,
		3 => CodedLength::ParamInfo,
		other => return Err(Error::Format(format!("diag coded type {other} is not one of the four ODX defines"))),
	};
	let bits = if length == CodedLength::MinMax {
		let _max = stream.u32()?;
		let _min = stream.u32()?;
		let _termination = stream.u8()?;
		None
	} else {
		Some(stream.u32()?)
	};
	if length == CodedLength::Standard {
		let _bit_mask = stream.bytefield()?;
	}
	let base = DataType::read(stream)?;
	let _encoding = stream.u8()?;
	let high_low_byte_order = stream.flag()?;
	let _condensed = stream.flag()?;
	if length == CodedLength::ParamInfo {
		// The length key is a whole nested parameter.
		let _key = nested_any(stream, |code, stream| parameter(stream, code))?;
	}
	Ok(CodedType {
		length,
		bits,
		base,
		high_low_byte_order,
	})
}

/// Read a `DB_PHYSICAL_TYPE`.
pub fn physical_type(stream: &mut Stream<'_>) -> Result<PhysicalType, Error> {
	let base = DataType::read_physical(stream)?;
	let precision = if stream.flag()? { Some(stream.u16()?) } else { None };
	let radix = stream.u8()?;
	Ok(PhysicalType { base, precision, radix })
}

/// Read a `DB_DOP_SIMPLE_BASE`.
pub fn dop(stream: &mut Stream<'_>) -> Result<Dop, Error> {
	let short_name = stream.ascii()?.map(str::to_owned);
	let compu = nested(stream, code::DB_COMPU_METHOD, compu::method)?;
	let coded = nested(stream, code::DB_DIAG_CODED_TYPE, coded_type)?;
	let physical = nested(stream, code::DB_PHYSICAL_TYPE, physical_type)?;
	// Two index maps, physical → coded and coded → physical, each a count of
	// `(u32, u16)` pairs. They are a lookup accelerator, not information.
	for _ in 0..2 {
		let count = stream.count()?;
		for _ in 0..count {
			let _key = stream.u32()?;
			let _index = stream.u16()?;
		}
	}
	let unit = if stream.flag()? { Some(reference(stream, false, false)?) } else { None };
	let _internal_constraint = if stream.flag()? { Some(reference(stream, false, false)?) } else { None };
	let _physical_constraint = if stream.flag()? { Some(reference(stream, false, false)?) } else { None };
	Ok(Dop {
		short_name,
		compu,
		coded,
		physical,
		unit,
	})
}

/// Read an `MCD_DB_PARAMETER` or any of the four types that extend it.
///
/// `type_code` selects the extra fields that follow the common ones. The
/// common part is identical for all five, which is why one function serves
/// them: `MCD_DB_PARAMETER_SIMPLE` is `MCD_DB_PARAMETER` under another name,
/// and the other three append.
pub fn parameter(stream: &mut Stream<'_>, type_code: u16) -> Result<Parameter, Error> {
	let description = stream.unicode()?.map(str::to_owned);
	let long_name = stream.unicode()?.map(str::to_owned);
	let short_name = stream.ascii()?.map(str::to_owned);
	let _some_id = stream.ascii()?;
	let long_name_id = stream.ascii()?.map(str::to_owned);
	let _unique_object_id = stream.ascii()?;

	let bit_position = stream.u8()?;
	let byte_position_raw = stream.u32()?;

	// One byte of flags decides which of the optional fields follow. Reading
	// it as anything else desynchronises the rest of the object.
	let flags = stream.u8()?;
	const HAS_DEFAULT: u8 = 1 << 0;
	const HAS_SEMANTIC: u8 = 1 << 1;
	const HAS_CODED_TYPE: u8 = 1 << 2;
	const HAS_DOP_REF: u8 = 1 << 3;
	const HAS_DOP_INLINE: u8 = 1 << 4;
	const BYTE_POS_AVAILABLE: u8 = 1 << 5;
	const HAS_SDG: u8 = 1 << 6;

	if flags & HAS_DEFAULT != 0 {
		let _default = stream.value()?;
	}
	let _display_level = stream.u32()?;
	if flags & HAS_SEMANTIC != 0 {
		let _semantic = stream.ascii()?;
	}
	let _sys_param = stream.ascii()?;
	let _parameter_type = stream.u8()?;
	let _layer_id = stream.u8()?;
	if flags & HAS_CODED_TYPE != 0 {
		let _coded = nested(stream, code::DB_DIAG_CODED_TYPE, coded_type)?;
	}
	let dop = if flags & HAS_DOP_REF != 0 {
		Some(reference(stream, false, false)?)
	} else {
		None
	};
	if flags & HAS_DOP_INLINE != 0 {
		// An inline data object property. `ODIS-project-explorer` has never
		// seen one either; saying so beats reading the rest at a wrong offset.
		return Err(Error::Format(
			"a parameter carries an inline data object property, a shape this reader has never seen".into(),
		));
	}
	let byte_position = (flags & BYTE_POS_AVAILABLE != 0).then_some(byte_position_raw);
	if flags & HAS_SDG != 0 {
		return Err(Error::Format(
			"a parameter carries special data groups, a shape this reader has never seen".into(),
		));
	}

	let mut out = Parameter {
		type_code,
		long_name,
		long_name_id,
		description,
		short_name,
		bit_position,
		byte_position,
		dop,
		table: None,
		inline_table: None,
		key_short_name: None,
	};

	match type_code {
		code::MCD_DB_PARAMETER_TABLE_KEY => {
			// The table is either inline or referenced, never both.
			out.inline_table = nested(stream, code::MCD_DB_TABLE, table)?.map(Box::new);
			if out.inline_table.is_none() {
				out.table = Some(super::attributed_reference(stream)?);
			}
			let _is_table_row_reference = stream.flag()?;
			let _string = stream.ascii()?;
		}
		code::MCD_DB_PARAMETER_TABLESTRUCT => {
			out.key_short_name = stream.ascii()?.map(str::to_owned);
			out.table = Some(super::attributed_reference(stream)?);
		}
		code::MCD_DB_MATCHING_REQUEST_PARAMETER => {
			let _request_byte_position = stream.u32()?;
			let _byte_length = stream.u32()?;
		}
		_ => {}
	}
	Ok(out)
}

/// Read an `MCD_DB_PARAMETERS` / `MCD_DB_RESPONSE_PARAMETERS` collection.
pub fn parameters(stream: &mut Stream<'_>) -> Result<Vec<Parameter>, Error> {
	let count = stream.count()?;
	let mut out = Vec::with_capacity(count.min(4096));
	for _ in 0..count {
		if let Some(one) = nested_any(stream, |code, stream| parameter(stream, code))? {
			out.push(one);
		}
	}
	Ok(out)
}

/// Read an `MCD_DB_PARAMETER_STRUCTURE`.
pub fn structure(stream: &mut Stream<'_>) -> Result<Structure, Error> {
	let short_name = stream.ascii()?.map(str::to_owned);
	let long_name = stream.unicode()?.map(str::to_owned);
	let _long_name_id = stream.ascii()?;
	let _description = stream.unicode()?;
	let _description_id = stream.ascii()?;
	let _unique_object_id = stream.ascii()?;
	let bytes = stream.u16()?;
	let fields = nested(stream, code::MCD_DB_PARAMETERS, parameters)?.unwrap_or_default();
	Ok(Structure {
		short_name,
		long_name,
		bytes,
		fields,
	})
}

/// Read an `MCD_DB_RESPONSE`.
pub fn response(stream: &mut Stream<'_>) -> Result<Response, Error> {
	let short_name = stream.ascii()?.map(str::to_owned);
	let _long_name = stream.unicode()?;
	let _description = stream.unicode()?;
	let _unique_object_id = stream.ascii()?;
	let _long_name_id = stream.ascii()?;
	let _reserved = stream.ascii()?;
	let parameters = nested(stream, code::MCD_DB_RESPONSE_PARAMETERS, parameters)?.unwrap_or_default();
	let _response_type = stream.u16()?;
	if stream.flag()? {
		return Err(Error::Format(
			"a response carries special data groups, a shape this reader has never seen".into(),
		));
	}
	Ok(Response { short_name, parameters })
}

/// Read an `MCD_DB_SERVICE`.
///
/// A service is three types deep — `MCD_DB_SERVICE` extends
/// `MCD_DB_DIAG_SERVICE` extends `MCD_DB_DATA_PRIMITIVE` extends
/// `MCD_DB_DIAG_COM_PRIMITIVE` — and the fields of all four are laid down in
/// that order with no marker between them.
pub fn service(stream: &mut Stream<'_>) -> Result<Service, Error> {
	// MCD_DB_SERVICE's own field.
	let _repetition_mode = stream.u8()?;

	// MCD_DB_DIAG_SERVICE: protocol parameter sets, then two enums.
	let count = stream.count()?;
	for _ in 0..count {
		let _name = stream.ascii()?;
		let _protocol_parameters = nested(stream, code::MCD_DB_REQUEST_PARAMETERS, parameters)?;
	}
	let _runtime_mode = stream.u16()?;
	let _is_multiple = stream.flag()?;

	// MCD_DB_DATA_PRIMITIVE.
	if stream.flag()? {
		return Err(Error::Format(
			"a service carries an access level, a shape this reader has never seen".into(),
		));
	}
	let _audience = nested(stream, code::MCD_AUDIENCE, audience)?;
	let _repetition = stream.u8()?;
	let count = stream.count()?;
	for _ in 0..count {
		let _name = stream.ascii()?;
		let _related = super::diag_com_reference(stream)?;
	}
	let status = stream.u8()?;
	if status & 1 != 0 {
		named_references(stream)?;
	}
	if status & 2 != 0 {
		named_references(stream)?;
	}
	if status & 4 != 0 {
		let count = stream.count()?;
		for _ in 0..count {
			let _object_id = stream.u32()?;
		}
	}

	// MCD_DB_DIAG_COM_PRIMITIVE.
	let id = stream.ascii()?.map(str::to_owned);
	let _long_name_id = stream.ascii()?;
	let _unique_object_id = stream.ascii()?;
	let _description = stream.unicode()?;
	let _long_name = stream.unicode()?;
	let short_name = stream.ascii()?.map(str::to_owned);
	let request = if stream.flag()? { Some(reference(stream, false, false)?) } else { None };
	let positive_responses = named_references(stream)?.into_iter().map(|(_, target)| target).collect();
	let _negative_responses = named_references(stream)?;
	let _functional_classes = named_references(stream)?;
	let _semantic = stream.ascii()?;
	let _transmission_mode = stream.u16()?;
	let _is_api_executable = stream.flag()?;
	let _is_no_operation = stream.flag()?;
	let _diagnostic_class = stream.u8()?;
	let _ecu_state_transitions = named_references(stream)?;
	let _ecu_states = named_references(stream)?;
	if stream.flag()? {
		let _bytefield = stream.bytefield()?;
		let _has_path = stream.flag()?;
		let _suppression_parameter = stream.ascii()?;
	}
	Ok(Service {
		id,
		short_name,
		request,
		positive_responses,
	})
}

/// Read an `MCD_AUDIENCE`: five one-byte flags naming who may run something.
fn audience(stream: &mut Stream<'_>) -> Result<(), Error> {
	// Supplier, development, manufacturing, after-sales, after-market.
	for _ in 0..5 {
		let _who = stream.u8()?;
	}
	Ok(())
}

/// Read an `MCD_DB_TABLE`.
pub fn table(stream: &mut Stream<'_>) -> Result<Table, Error> {
	let _reserved = stream.ascii()?;
	let _some_id = stream.ascii()?;
	let _object_id = stream.ascii()?;
	let _description = stream.unicode()?;
	let _long_name = stream.unicode()?;
	let short_name = stream.ascii()?.map(str::to_owned);

	// The row map's key is a Unicode string — the channel's human name — and
	// its value a named reference to the row object.
	let count = stream.count32()?;
	let mut rows = Vec::with_capacity(count.min(8192));
	for _ in 0..count {
		let key = stream.unicode()?.map(str::to_owned);
		let object = stream.ascii()?.map(str::to_owned);
		let pool = stream.ascii()?.map(str::to_owned);
		let _short_name = stream.ascii()?;
		rows.push((key, Ref { object, pool }));
	}

	let _semantic = stream.ascii()?;
	let count = stream.count()?;
	for _ in 0..count {
		let _name = stream.ascii()?;
		let _primitive = super::diag_com_reference(stream)?;
	}
	let key_dop = if stream.flag()? { Some(reference(stream, false, false)?) } else { None };
	if stream.flag()? {
		return Err(Error::Format(
			"a table carries special data groups, a shape this reader has never seen".into(),
		));
	}
	Ok(Table { short_name, rows, key_dop })
}

/// Read an `MCD_DB_TABLE_PARAMETER`.
///
/// The trailing bytes are named sub-streams the kernel writes after the
/// object's own terminator, and [`Stream::end`] already stops at the
/// terminator — so nothing here has to know what they are.
pub fn table_row(stream: &mut Stream<'_>) -> Result<TableRow, Error> {
	let key = stream.unicode()?.map(str::to_owned);
	let _audience = nested(stream, code::MCD_AUDIENCE, audience)?;
	if stream.flag()? {
		return Err(Error::Format(
			"a table row carries disabled audiences, a shape this reader has never seen".into(),
		));
	}
	if stream.flag()? {
		return Err(Error::Format(
			"a table row carries enabled audiences, a shape this reader has never seen".into(),
		));
	}
	// The row's parameter is inlined without a type code of its own — it is
	// always `MCD_DB_PARAMETER`.
	let parameter = parameter(stream, code::MCD_DB_PARAMETER)?;
	Ok(TableRow { key, parameter })
}

/// Read an `MCD_DB_UNIT`.
pub fn unit(stream: &mut Stream<'_>) -> Result<Unit, Error> {
	let short_name = stream.ascii()?.map(str::to_owned);
	let long_name = stream.unicode()?.map(str::to_owned);
	let _description = stream.unicode()?;
	let _unique_object_id = stream.ascii()?;
	let long_name_id = stream.ascii()?.map(str::to_owned);
	let _reserved = stream.ascii()?;
	let display_name = stream.unicode()?.map(str::to_owned);
	let _factor_si_to_unit = stream.f64()?;
	let _offset_si_to_unit = stream.f64()?;
	let _physical_dimension = nested(stream, code::MCD_DB_PHYSICAL_DIMENSION, physical_dimension)?;
	if stream.flag()? {
		named_references(stream)?;
	}
	Ok(Unit {
		short_name,
		long_name,
		display_name,
		long_name_id,
	})
}

/// Read an `MCD_DB_PHYSICAL_DIMENSION`: a unit's seven SI exponents.
fn physical_dimension(stream: &mut Stream<'_>) -> Result<(), Error> {
	let _short_name = stream.ascii()?;
	let _long_name = stream.unicode()?;
	let _description = stream.unicode()?;
	let _unique_object_id = stream.ascii()?;
	let _long_name_id = stream.ascii()?;
	let _reserved = stream.ascii()?;
	// Length, mass, time, current, temperature, molar amount, luminous
	// intensity — the seven SI base dimensions, as signed exponents.
	for _ in 0..7 {
		let _exponent = stream.i32()?;
	}
	Ok(())
}

/// What a `TABLE-KEY`'s text table says: `(DID, the channel's name)`.
///
/// Reached through the table's own key data object property, whose compu
/// method is a `TEXTTAB` mapping each coded DID onto the human name that also
/// keys [`Table::rows`]. Anything not an integer key with a text is dropped
/// rather than guessed at.
pub fn key_levels(dop: &Dop) -> Result<Vec<(u16, String, Option<String>)>, Error> {
	let method = dop
		.compu
		.as_ref()
		.ok_or_else(|| Error::Format("a table key's data object property has no compu method".into()))?;
	if method.category != compu::Category::TextTable {
		return Err(Error::Format(format!(
			"a table key's compu method is {}, not TEXTTABLE",
			method.category.name()
		)));
	}
	let base = method
		.internal_to_phys
		.as_ref()
		.ok_or_else(|| Error::Format("a table key's text table has no coded-to-physical direction".into()))?;
	let mut out = Vec::with_capacity(base.scales.len());
	for scale in &base.scales {
		let Some(raw) = scale.lower_coded.as_ref().and_then(|l| l.value.as_ref()).and_then(as_u16) else {
			continue;
		};
		let Some(name) = scale.constant.as_ref().and_then(as_text) else {
			continue;
		};
		out.push((raw, name, scale.label_id.clone()));
	}
	Ok(out)
}

/// A value as a `u16` DID, when it is one.
fn as_u16(value: &Value) -> Option<u16> {
	match value {
		Value::U32(v) => u16::try_from(*v).ok(),
		Value::I32(v) => u16::try_from(*v).ok(),
		_ => None,
	}
}

/// A value as text, when it is a string.
fn as_text(value: &Value) -> Option<String> {
	match value {
		Value::Unicode(Some(s)) | Value::Ascii(Some(s)) => Some(s.clone()),
		_ => None,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::catalog::Scaling;
	use crate::measure::LinearScale;
	use crate::odis::hash;
	use crate::odis::loaders::{Object, Outcome, load};
	use crate::odis::object::END;
	use crate::odis::strings::{Pool, Strings};

	/// Builds object bytes the way the MCD kernel writes them, so a loader can
	/// be tested against the shape it will actually meet. Deliberately verbose:
	/// a helper that guessed at widths would test the guess, not the loader.
	#[derive(Default)]
	struct Bytes {
		out: Vec<u8>,
		ascii: Vec<String>,
		unicode: Vec<String>,
	}

	impl Bytes {
		fn u8(&mut self, v: u8) -> &mut Self {
			self.out.push(v);
			self
		}
		fn u16(&mut self, v: u16) -> &mut Self {
			self.out.extend_from_slice(&v.to_le_bytes());
			self
		}
		fn u32(&mut self, v: u32) -> &mut Self {
			self.out.extend_from_slice(&v.to_le_bytes());
			self
		}
		fn f64(&mut self, v: f64) -> &mut Self {
			self.out.extend_from_slice(&v.to_le_bytes());
			self
		}
		/// A pooled ASCII reference, registering the string as it goes.
		fn a(&mut self, s: Option<&str>) -> &mut Self {
			match s {
				None => self.u32(0),
				Some(s) => {
					self.ascii.push(s.to_owned());
					self.u32(hash::of_bytes(s.as_bytes()))
				}
			}
		}
		/// A pooled Unicode reference.
		fn u(&mut self, s: Option<&str>) -> &mut Self {
			match s {
				None => self.u32(0),
				Some(s) => {
					self.unicode.push(s.to_owned());
					let units: Vec<u16> = s.encode_utf16().collect();
					self.u32(hash::of_utf16(&units))
				}
			}
		}
		/// The "no nested object" flag.
		fn none(&mut self) -> &mut Self {
			self.u8(0)
		}
		/// The "a nested object follows" flag plus its type code.
		fn some(&mut self, type_code: u16) -> &mut Self {
			self.u8(1).u16(type_code)
		}
		/// An `MCDValue` of no type.
		fn no_value(&mut self) -> &mut Self {
			self.u8(0xFF)
		}
		/// An `MCDValue` holding an unsigned integer.
		fn uint_value(&mut self, v: u32) -> &mut Self {
			self.u8(0x0B).u32(v)
		}
		/// An `MCDValue` holding a pooled Unicode string.
		fn text_value(&mut self, s: &str) -> &mut Self {
			self.u8(0x0E).u(Some(s))
		}

		/// Finish: the terminator, the object bytes, and the pools they need.
		fn done(mut self, type_code: u16) -> (Vec<u8>, Strings) {
			self.out.extend_from_slice(&END);
			let mut body = type_code.to_le_bytes().to_vec();
			body.extend_from_slice(&self.out);

			let mut a = Vec::new();
			for s in &self.ascii {
				a.extend_from_slice(&(s.len() as u32).to_le_bytes());
				a.extend_from_slice(s.as_bytes());
			}
			let mut u = Vec::new();
			for s in &self.unicode {
				let units: Vec<u16> = s.encode_utf16().collect();
				u.extend_from_slice(&(units.len() as u32).to_le_bytes());
				for unit in units {
					u.extend_from_slice(&unit.to_le_bytes());
				}
			}
			let strings = Strings {
				ascii: Pool::parse_ascii(&a).expect("a synthesised pool parses"),
				unicode: Pool::parse_utf16(&u).expect("a synthesised pool parses"),
			};
			(body, strings)
		}
	}

	/// A `DB_LIMIT` holding one unsigned integer, CLOSED.
	fn closed_uint(b: &mut Bytes, v: u32) {
		b.some(code::DB_LIMIT).uint_value(v).u8(2);
	}

	/// A `DB_DIAG_CODED_TYPE`: standard length, `bits` wide, unsigned, big-endian.
	fn standard_uint(b: &mut Bytes, bits: u32) {
		b.some(code::DB_DIAG_CODED_TYPE)
			.u8(2) // eSTANDARD_LENGTH_TYPE
			.u32(bits)
			.u8(0) // no bit mask
			.u8(1) // eDB_UINT32
			.u8(11) // eNONE encoding
			.u8(1) // high-low byte order
			.u8(0); // not condensed
	}

	/// A `DB_PHYSICAL_TYPE`: float64, no precision, decimal.
	fn physical_float(b: &mut Bytes) {
		b.some(code::DB_PHYSICAL_TYPE).u8(3).u8(0).u8(10);
	}

	/// The two index maps a data object property carries, both empty.
	fn empty_index_maps(b: &mut Bytes) {
		b.u16(0).u16(0);
	}

	/// A `DB_COMPU_METHOD` of category `LINEAR` with one scale.
	fn linear_method(b: &mut Bytes, offset: f64, factor: f64, divisor: f64) {
		b.some(code::DB_COMPU_METHOD)
			.u8(1) // eLINEAR
			.none(); // no physical-to-internal direction
		b.some(code::DB_COMPU_BASE);
		b.some(code::DB_COMPU_SCALES).u32(1);
		b.some(code::DB_COMPU_SCALE).a(None); // the scale's label id
		b.none(); // no inverse coefficients
		b.some(code::DB_COMPU_RATIONAL_COEFFS).u8(2).f64(offset).f64(factor).u8(1).f64(divisor);
		b.none().none(); // no physical limits
		b.no_value().no_value().no_value(); // constant, inverse, coded constant
		b.none().none(); // no coded limits
		b.no_value().no_value(); // the base's default and code byte stream
		b.none(); // no code information
		b.no_value(); // the base's inverse value
	}

	#[test]
	fn a_data_object_property_parses_to_its_scaling() {
		// The shape of a real measurement channel: 16 raw bits, converted by
		// (0 + 1 * x) / 20 — one of the slopes this project proved by driving.
		let mut b = Bytes::default();
		b.a(Some("DOP_2ByteUINTLINEA1xPlus0Div20FLOAT2Degre"));
		linear_method(&mut b, 0.0, 1.0, 20.0);
		standard_uint(&mut b, 16);
		physical_float(&mut b);
		empty_index_maps(&mut b);
		b.u8(1).a(Some("UNIT_Degre")).a(None); // the unit reference
		b.none().none(); // no internal or physical constraint
		let (body, strings) = b.done(code::DB_DOP_SIMPLE_BASE);

		let (type_code, mut stream) = crate::odis::object::Stream::open(&body, &strings).expect("a well-formed object opens");
		let outcome = load(type_code, &mut stream).expect("a well-formed data object property parses");
		let Outcome::Object(Object::Dop(dop)) = outcome else {
			panic!("got {outcome:?}")
		};

		assert_eq!(dop.short_name.as_deref(), Some("DOP_2ByteUINTLINEA1xPlus0Div20FLOAT2Degre"));
		let coded = dop.coded.expect("the file gives a coded type");
		assert_eq!(coded.bits, Some(16));
		assert_eq!(coded.base, DataType::Uint32);
		assert!(!coded.base.is_signed());
		assert!(coded.high_low_byte_order, "a UDS payload is big-endian unless the file says otherwise");
		assert_eq!(dop.unit.as_ref().and_then(|u| u.object.as_deref()), Some("UNIT_Degre"));
		let scaling = dop.compu.expect("the file gives a compu method").scaling().expect("LINEAR translates");
		assert_eq!(scaling, Scaling::Linear(LinearScale { factor: 0.05, offset: 0.0 }));

		// Nothing after the terminator: the loader read the object exactly.
		assert_eq!(stream.remaining(), 0, "the loader must land on the terminator, not before or past it");
	}

	#[test]
	fn a_table_key_yields_its_identifiers() {
		// A text table mapping two DIDs onto the names that key the row table.
		let mut b = Bytes::default();
		b.a(Some("DOP_TableKey"));
		b.some(code::DB_COMPU_METHOD).u8(3).none(); // eTEXTTAB, no inverse direction
		b.some(code::DB_COMPU_BASE);
		b.some(code::DB_COMPU_SCALES).u32(2);
		for (did, name) in [(0x380Au32, "Getriebe-Eingangsdrehzahl"), (0x2000, "Motordrehzahl")] {
			b.some(code::DB_COMPU_SCALE).a(Some("000116"));
			b.none().none(); // no coefficients either way
			b.none().none(); // no physical limits
			b.text_value(name).no_value().no_value();
			closed_uint(&mut b, did);
			closed_uint(&mut b, did);
		}
		b.no_value().no_value().none().no_value(); // the base's tail
		standard_uint(&mut b, 16);
		physical_float(&mut b);
		empty_index_maps(&mut b);
		b.none().none().none();
		let (body, strings) = b.done(code::DB_DOP_SIMPLE_BASE);

		let (type_code, mut stream) = crate::odis::object::Stream::open(&body, &strings).expect("a well-formed object opens");
		let Outcome::Object(Object::Dop(dop)) = load(type_code, &mut stream).expect("a table key's property parses") else {
			panic!("expected a data object property")
		};
		assert_eq!(stream.remaining(), 0);

		let levels = key_levels(&dop).expect("a TEXTTABLE yields levels");
		assert_eq!(
			levels,
			vec![
				(0x380A, "Getriebe-Eingangsdrehzahl".to_owned(), Some("000116".to_owned())),
				(0x2000, "Motordrehzahl".to_owned(), Some("000116".to_owned())),
			]
		);
	}

	#[test]
	fn a_parameter_reads_its_position_and_its_property() {
		let mut b = Bytes::default();
		b.u(Some("Die Drehzahl der Getriebeeingangswelle")) // description
			.u(Some("Getriebe-Eingangsdrehzahl")) // long name
			.a(Some("Param_1")) // short name
			.a(None) // some id
			.a(Some("000116")) // long name id
			.a(None); // unique object id
		b.u8(3).u32(2); // bit position 3, byte position 2
		// Flags: a data object property reference, and the byte position is real.
		b.u8((1 << 3) | (1 << 5));
		b.u32(0); // display level
		b.a(None); // sys param
		b.u8(1); // eVALUE
		b.u8(0xFF); // no layer id
		b.a(Some("DOP_2Byte")).a(None); // the property reference
		let (body, strings) = b.done(code::MCD_DB_PARAMETER);

		let (type_code, mut stream) = crate::odis::object::Stream::open(&body, &strings).expect("a well-formed object opens");
		let Outcome::Object(Object::Parameter(p)) = load(type_code, &mut stream).expect("a parameter parses") else {
			panic!("expected a parameter")
		};
		assert_eq!(stream.remaining(), 0);
		assert_eq!(p.long_name.as_deref(), Some("Getriebe-Eingangsdrehzahl"));
		assert_eq!(p.long_name_id.as_deref(), Some("000116"));
		assert_eq!(p.byte_position, Some(2));
		assert_eq!(p.bit_position, 3);
		assert_eq!(p.dop.and_then(|d| d.object), Some("DOP_2Byte".to_owned()));
	}

	/// A parameter with no byte position must report none, not zero. A
	/// measurement placed at byte 0 and a measurement with no stated position
	/// are different things, and conflating them puts a channel at the head of
	/// a response it does not belong in.
	#[test]
	fn a_parameter_without_a_byte_position_reports_none() {
		let mut b = Bytes::default();
		b.u(None).u(None).a(None).a(None).a(None).a(None);
		b.u8(0).u32(7); // a stale byte position the flags do not vouch for
		b.u8(0); // no flags at all
		b.u32(0).a(None).u8(1).u8(0xFF);
		let (body, strings) = b.done(code::MCD_DB_PARAMETER);
		let (type_code, mut stream) = crate::odis::object::Stream::open(&body, &strings).expect("a well-formed object opens");
		let Outcome::Object(Object::Parameter(p)) = load(type_code, &mut stream).expect("a parameter parses") else {
			panic!("expected a parameter")
		};
		assert_eq!(p.byte_position, None);
	}

	#[test]
	fn a_structure_carries_its_fields() {
		let mut b = Bytes::default();
		b.a(Some("STRUC_Drehzahl")).u(Some("Drehzahl")).a(None).u(None).a(None).a(None);
		b.u16(2); // two bytes wide
		b.some(code::MCD_DB_PARAMETERS).u16(1);
		b.some(code::MCD_DB_PARAMETER);
		b.u(None).u(Some("Getriebe-Eingangsdrehzahl")).a(None).a(None).a(Some("000116")).a(None);
		b.u8(0).u32(0);
		b.u8((1 << 3) | (1 << 5));
		b.u32(0).a(None).u8(1).u8(0xFF);
		b.a(Some("DOP_2Byte")).a(None);
		let (body, strings) = b.done(code::MCD_DB_PARAMETER_STRUCTURE);

		let (type_code, mut stream) = crate::odis::object::Stream::open(&body, &strings).expect("a well-formed object opens");
		let Outcome::Object(Object::Structure(s)) = load(type_code, &mut stream).expect("a structure parses") else {
			panic!("expected a structure")
		};
		assert_eq!(stream.remaining(), 0, "a nested parameter must not consume a terminator of its own");
		assert_eq!(s.bytes, 2);
		assert_eq!(s.fields.len(), 1);
		assert_eq!(s.fields[0].long_name.as_deref(), Some("Getriebe-Eingangsdrehzahl"));
	}

	/// The refusal list reaches into the measurement chain: a compu method
	/// whose base names an external code module stops the parse and says which
	/// type stopped it, rather than reading past it.
	#[test]
	fn a_compu_method_carrying_code_information_is_refused() {
		let mut b = Bytes::default();
		b.a(Some("DOP_Coded"));
		b.some(code::DB_COMPU_METHOD).u8(4).none(); // eCOMPUCODE
		b.some(code::DB_COMPU_BASE);
		b.none(); // no scales
		b.no_value().no_value();
		b.u8(1).u16(code::MCD_DB_CODE_INFORMATION); // code information is present
		let (body, strings) = b.done(code::DB_DOP_SIMPLE_BASE);

		let (type_code, mut stream) = crate::odis::object::Stream::open(&body, &strings).expect("a well-formed object opens");
		let err = load(type_code, &mut stream).expect_err("code information must stop the parse");
		assert!(matches!(err, Error::Refused("MCD_DB_CODE_INFORMATION")), "got {err:?}");
	}
}
