//! Identification: which ECU variants a project describes, and how each one
//! says what it can do.
//!
//! Three types carry it, and the way they join is the reason this module is
//! separate from [`super::measurement`]:
//!
//! - `DB_PROJECT_DATA` — one per base-variant pool, under the fixed ObjectID
//!   [`PROJECT_DATA_ID`]. It names the base variant and every ECU variant
//!   derived from it. This is where [`super::super::Project::variants`] gets
//!   its list: no scan, one object per pool.
//! - `MCD_DB_ECU_VARIANT` — one control unit's exact software identity, and
//!   the matching patterns that recognise it on a car.
//! - `DB_LAYER_DATA` — a variant's index of every service, data object
//!   property and table it can reach, including inherited ones. The
//!   measurement chain starts here.
//!
//! ## Finding a variant's layer data without an access key
//! `ODIS-project-explorer` reaches an ECU variant's layer data through
//! `ecu.location_refs[0].access_key.layer_data_object_id` — that is, by parsing
//! an `MCD_ACCESS_KEY`, which is on this project's permanent refusal list
//! (`SAFETY.md`, design §2). So [`layer_data`] is reached the other way: a
//! pool's layer-data objects are found by type, and the one whose
//! [`LayerData::variant`] names the wanted variant is its own. Same answer, and
//! no access key is ever parsed.
//!
//! That is also why [`ecu`] stops at the location references' access keys and
//! reports [`super::super::Error::Refused`]: an ECU's location list is the only
//! place an access key appears inline in this chain, and reading past one is not
//! possible — nothing in the stream says how long it is.

use super::super::Error;
use super::super::object::Stream;
use super::measurement;
use super::{
	Ref, code, diag_com_reference_map, name_list, named_references, nested, nested_any, reference, reference_map, skip_location_reference,
	string_vector_map,
};

/// The ObjectID every base-variant pool stores its project data under.
///
/// A generated name, not a car's: VW's MCD converter writes it into every pool
/// it emits, which is what makes a pool's variant list a single lookup instead
/// of a scan.
pub const PROJECT_DATA_ID: &str = "#RtGen_DB_PROJECT_DATA";

/// The ObjectID a pool stores its *own* layer's data under. An ECU variant's
/// layer data lives in the same pool under a generated name that this reader
/// does not try to predict — see the module note.
pub const LAYER_DATA_ID: &str = "#RtGen_DB_LAYER_DATA";

/// Which kind of layer a `DB_LAYER_DATA` belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
	/// A base variant: the family, shared by every variant derived from it.
	BaseVariant,
	/// One ECU variant.
	EcuVariant,
	/// A functional group.
	FunctionalGroup,
	/// A multiple-ECU job.
	MultipleEcuJob,
	/// A protocol — `UDSOnCAN` and its parents.
	Protocol,
}

impl Layer {
	/// Read the two-byte `MCDLocationType`.
	fn read(stream: &mut Stream<'_>) -> Result<Layer, Error> {
		Ok(match stream.u16()? {
			0x0101 => Layer::BaseVariant,
			0x0102 => Layer::EcuVariant,
			0x0103 => Layer::FunctionalGroup,
			0x0104 => Layer::MultipleEcuJob,
			0x0105 => Layer::Protocol,
			other => return Err(Error::Format(format!("layer type {other:#06x} is not one of the five ODX defines"))),
		})
	}
}

/// `DB_PROJECT_DATA`: what a base-variant pool holds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProjectData {
	/// The base variant this pool is for.
	pub base_variant: Ref,
	/// `(name, reference)` for every ECU variant derived from it.
	pub ecu_variants: Vec<(Option<String>, Ref)>,
}

/// `MCD_DB_ECU_VARIANT`: one control unit's exact software identity.
#[derive(Debug, Clone, PartialEq)]
pub struct EcuVariant {
	/// The base variant it derives from.
	pub base_variant: Ref,
	/// How many matching patterns recognise it. The patterns themselves each
	/// name a `ReadDataByIdentifier` call and an expected answer; they are
	/// counted rather than kept, because this reader identifies a car from
	/// what it already reads (`F187`, `F19E`), not by replaying VW's patterns.
	pub patterns: usize,
	/// The unit's short name.
	pub short_name: Option<String>,
	/// The unit's human name.
	pub long_name: Option<String>,
}

/// `DB_LAYER_DATA`: one layer's index of everything reachable from it.
#[derive(Debug, Clone, PartialEq)]
pub struct LayerData {
	/// Which kind of layer this is.
	pub layer: Layer,
	/// The variant, base variant or functional group it belongs to.
	pub variant: Ref,
	/// `(short name, reference)` for every diagnostic service. The measurement
	/// chain starts by looking up [`RDBI_MEASUREMENT`] here.
	pub services: Vec<(Option<String>, Ref)>,
	/// `(ObjectID, reference)` for every data object property, which is how a
	/// reference that omits its pool is resolved.
	pub properties: Vec<(Option<String>, Ref)>,
	/// `(ObjectID, reference)` for every table.
	pub tables: Vec<(Option<String>, Ref)>,
	/// The pools this layer inherits from, nearest first. Empty when the tail
	/// was not followed — see [`LayerData::complete`].
	pub parents: Vec<String>,
	/// Whether the whole object was read, terminator included.
	///
	/// `false` means the head — everything up to and including the table index,
	/// which is all the measurement chain needs — was read, and the tail was
	/// not. 34 of the reference project's 663 layer-data objects end in a shape
	/// this reader cannot follow, and refusing those would cost 34 control
	/// units their measurements to protect bookkeeping nobody reads. The
	/// terminator check is skipped for exactly those objects and nothing else;
	/// [`super::load`] honours this flag.
	pub complete: bool,
}

/// The short name of the service that reads measurements.
///
/// Not a car-specific constant: it is VW's own generated name for the
/// `ReadDataByIdentifier` measurement service, written by the MCD converter
/// into every project it emits, and it is looked *up* in a layer's own service
/// index rather than assumed to exist. A variant that does not list it simply
/// has no measurements.
pub const RDBI_MEASUREMENT: &str = "DiagnServi_ReadDataByIdentMeasuValue";

/// Read a `DB_PROJECT_DATA`.
///
/// It opens with a list of location references whose access keys are stepped
/// over, not parsed ([`super::skip_access_key`]) — the reference project's
/// engine pool has 1,209 of them in front of the variant list, so there is no
/// reading a car without walking past them.
///
/// It ends with a list of **nested `DB_PROJECT_DATA` objects**, one per ECU
/// variant, each holding that variant's own location references. The recursion
/// is real and observed: refusing to follow it leaves 77,988 bytes unread and
/// the terminator check fails.
pub fn project_data(stream: &mut Stream<'_>) -> Result<ProjectData, Error> {
	let count = stream.count()?;
	for _ in 0..count {
		skip_location_reference(stream)?;
	}
	let _functional_group = reference(stream, true, false)?;
	let base_variant = reference(stream, true, false)?;
	let ecu_variants = named_references(stream)?;
	let _ecu_variant = reference(stream, true, false)?;
	let _string1 = stream.ascii()?;
	let _string2 = stream.ascii()?;
	let _string3 = stream.ascii()?;
	let _functional_groups = name_list(stream)?;
	let nested_count = stream.count()?;
	for _ in 0..nested_count {
		nested(stream, code::DB_PROJECT_DATA, |stream| project_data(stream).map(|_| ()))?;
	}
	Ok(ProjectData { base_variant, ecu_variants })
}

/// Read an `MCD_DB_ECU_VARIANT`.
pub fn ecu_variant(stream: &mut Stream<'_>) -> Result<EcuVariant, Error> {
	let base_variant = reference(stream, false, false)?;
	let patterns = nested(stream, code::MCD_DB_MATCHING_PATTERNS, matching_patterns)?.unwrap_or(0);
	let (short_name, long_name) = ecu(stream)?;
	Ok(EcuVariant {
		base_variant,
		patterns,
		short_name,
		long_name,
	})
}

/// Read an `MCD_DB_MATCHING_PATTERNS` collection, returning how many there are.
///
/// The patterns are read in full — the field order gives no way to skip them —
/// but nothing is kept. Identifying a car is this tool's own job, done from
/// `F187` and `F19E` off the car itself (`vagcan info`), not by replaying VW's
/// recognition rules.
fn matching_patterns(stream: &mut Stream<'_>) -> Result<usize, Error> {
	let count = stream.count32()?;
	for _ in 0..count {
		let _one = nested(stream, code::MCD_DB_MATCHING_PATTERN, matching_pattern)?;
	}
	Ok(count)
}

/// Read an `MCD_DB_MATCHING_PATTERN`.
fn matching_pattern(stream: &mut Stream<'_>) -> Result<(), Error> {
	nested(stream, code::MCD_DB_MATCHING_PARAMETERS, matching_parameters)?;
	Ok(())
}

/// Read an `MCD_DB_MATCHING_PARAMETERS` collection.
fn matching_parameters(stream: &mut Stream<'_>) -> Result<(), Error> {
	let count = stream.count32()?;
	for _ in 0..count {
		nested(stream, code::MCD_DB_MATCHING_PARAMETER, matching_parameter)?;
	}
	Ok(())
}

/// Read an `MCD_DB_MATCHING_PARAMETER`: which service to call, which response
/// field to look at, and what it must say.
fn matching_parameter(stream: &mut Stream<'_>) -> Result<(), Error> {
	let _primitive = super::diag_com_reference(stream)?;
	let _has_path = stream.flag()?;
	let _response_parameter = stream.ascii()?;
	let _expected = stream.unicode()?;
	Ok(())
}

/// Read the `MCD_DB_ECU` fields an ECU variant and a base variant both end on.
///
/// Returns the short and long name. Its location references carry access keys,
/// which are stepped over rather than parsed — see [`super::skip_access_key`]
/// for why that is the enforcement of the refusal and not a hole in it.
fn ecu(stream: &mut Stream<'_>) -> Result<(Option<String>, Option<String>), Error> {
	let short_name = stream.ascii()?.map(str::to_owned);
	let long_name = stream.unicode()?.map(str::to_owned);
	let _description = stream.unicode()?;
	let _reserved = stream.ascii()?;
	let _long_name_id = stream.ascii()?;
	let _description_id = stream.ascii()?;
	let count = stream.count()?;
	for _ in 0..count {
		let _name = stream.ascii()?;
		skip_location_reference(stream)?;
	}
	Ok((short_name, long_name))
}

/// Read a `DB_LAYER_DATA`.
///
/// The longest field order in the format, and the one where a wrong width is
/// least visible — nearly every field is a map of pooled names, so a misread
/// count consumes plausible-looking bytes for a long time before anything
/// fails. The terminator check at the end of the member is what catches it.
pub fn layer_data(stream: &mut Stream<'_>) -> Result<LayerData, Error> {
	let mut head = layer_head(stream)?;
	// The tail is inheritance and protocol bookkeeping. It is read so the
	// terminator can vouch for the head, and a tail that cannot be followed
	// costs that vouching for one object rather than the object itself.
	let mark = stream.mark();
	match layer_tail(stream) {
		Ok(parents) => head.parents = parents,
		Err(_) => {
			stream.rewind(mark);
			head.complete = false;
		}
	}
	Ok(head)
}

/// The part of a layer every caller needs: what it is, whose it is, and its
/// indexes of services, data object properties and tables.
fn layer_head(stream: &mut Stream<'_>) -> Result<LayerData, Error> {
	let _layer_id = stream.ascii()?;
	let _unknown = stream.ascii()?;
	let _protocol_type = stream.ascii()?;
	let _protocol_stack = stream.ascii()?;
	let _com_param_spec_pool = stream.ascii()?;

	let layer = Layer::read(stream)?;
	let variant = match layer {
		Layer::BaseVariant | Layer::EcuVariant | Layer::FunctionalGroup => reference(stream, false, false)?,
		// A protocol or a multiple-ECU job layer names nothing.
		Layer::Protocol | Layer::MultipleEcuJob => Ref::default(),
	};

	let services = diag_com_reference_map(stream)?;
	let _dtc_properties = name_list(stream)?;
	let properties = reference_map(stream, false)?;
	let tables = reference_map(stream, true)?;
	Ok(LayerData {
		layer,
		variant,
		services,
		properties,
		tables,
		parents: Vec::new(),
		complete: true,
	})
}

/// Everything after the table index, ending on the member's terminator.
///
/// Returns the parent pools. Any failure here is caught by [`layer_data`]; the
/// terminator is consumed here rather than by [`super::load`] precisely so that
/// "the tail was followed" and "the object ended where it should" are one
/// question with one answer.
fn layer_tail(stream: &mut Stream<'_>) -> Result<Vec<String>, Error> {
	let _requests = reference_map(stream, false)?;
	let _global_negative_responses = reference_map(stream, false)?;
	let _functional_classes = reference_map(stream, false)?;

	let count = stream.count()?;
	for _ in 0..count {
		let _name = stream.ascii()?;
		diag_com_reference_map(stream)?;
	}

	// Three maps the reference implementation has only ever seen empty. They
	// are read rather than assumed away, because an entry in one would shift
	// everything after it and the terminator check would then be the only
	// evidence anything went wrong.
	for _ in 0..3 {
		reference_map(stream, false)?;
	}

	// Environment-data descriptions: freeze-frame layouts, one nested object
	// each. Fault data rather than measurement data, so they are walked for
	// alignment and kept by nobody — but they do occur (26 of the reference
	// project's 663 layer-data objects carry one), and refusing them would
	// refuse those layers' measurements along with them.
	let count = stream.count()?;
	for _ in 0..count {
		let _key = stream.ascii()?;
		nested_any(stream, |_, stream| env_data_desc(stream))?;
	}

	let parents = name_list(stream)?;
	let _shared_data_parents = name_list(stream)?;
	for _ in 0..4 {
		string_vector_map(stream)?;
	}
	let _unit_groups = reference_map(stream, false)?;
	let _units = reference_map(stream, false)?;

	// Protocol parameters — timings and addressing, each a whole nested object.
	let count = stream.count()?;
	for _ in 0..count {
		nested_any(stream, |code, stream| measurement::protocol_parameter(stream, code))?;
	}
	let _unknown = stream.u8()?;
	if stream.u8()? != 0 {
		return Err(Error::Format(
			"a layer carries special data groups, a shape this reader has never seen".into(),
		));
	}
	let _diag_com_objects = reference_map(stream, false)?;
	stream.end()?;
	Ok(parents)
}

/// Read an `MCD_DB_ENV_DATA_DESC`: a freeze frame's layout.
///
/// Walked, not kept. Its shape is here only so that a layer which has one can
/// still hand over its measurement services.
fn env_data_desc(stream: &mut Stream<'_>) -> Result<(), Error> {
	let _reserved = stream.unicode()?;
	let _long_name = stream.unicode()?;
	let _short_name = stream.ascii()?;
	for _ in 0..3 {
		let _also = stream.ascii()?;
	}
	let count = stream.count()?;
	for _ in 0..count {
		let _key = stream.u32()?;
		let _name = stream.ascii()?;
	}
	// The reference implementation notes this flag is read twice, and the files
	// agree: an outer presence byte, then the usual nested-object one.
	if stream.flag()? {
		nested_any(stream, |code, stream| measurement::parameter(stream, code).map(|_| ()))?;
	}
	let count = stream.count()?;
	for _ in 0..count {
		let _name = stream.ascii()?;
		nested_any(stream, |code, stream| measurement::parameter(stream, code).map(|_| ()))?;
		let inner = stream.count32()?;
		for _ in 0..inner {
			let _value = stream.u32()?;
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::odis::hash;
	use crate::odis::object::{END, Stream};
	use crate::odis::strings::{Pool, Strings};

	/// The same byte-builder [`super::measurement`]'s tests use, kept local so
	/// each module's fixtures read as the type they are testing.
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
		fn a(&mut self, s: Option<&str>) -> &mut Self {
			match s {
				None => self.u32(0),
				Some(s) => {
					self.ascii.push(s.to_owned());
					self.u32(hash::of_bytes(s.as_bytes()))
				}
			}
		}
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
		/// An empty map or list, whose count is two bytes.
		fn empty_map(&mut self) -> &mut Self {
			self.u16(0)
		}
		/// The "a nested object follows" flag plus its type code.
		fn some(&mut self, type_code: u16) -> &mut Self {
			self.u8(1).u16(type_code)
		}
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
			(
				body,
				Strings {
					ascii: Pool::parse_ascii(&a).expect("a synthesised pool parses"),
					unicode: Pool::parse_utf16(&u).expect("a synthesised pool parses"),
				},
			)
		}
	}

	/// A `DbObjectReference` with a third name — the bare `load_reference` shape.
	fn reference_bytes(b: &mut Bytes, object: Option<&str>, pool: Option<&str>) {
		b.a(object).a(pool).a(None);
	}

	/// The two-name shape a *named* reference collection uses. Getting this
	/// wrong by one field is what made every real `.bv` pool unreadable.
	fn named_reference_bytes(b: &mut Bytes, name: &str, object: Option<&str>, pool: Option<&str>) {
		b.a(Some(name)).a(object).a(pool);
	}

	/// One location reference carrying `keys` access keys, each of the fixed
	/// 34-byte shape this reader steps over without parsing.
	fn location_reference_bytes(b: &mut Bytes, keys: u8) {
		b.a(Some("Loc")).a(Some("EV_Test")).u8(keys);
		for _ in 0..keys {
			b.u16(code::MCD_ACCESS_KEY);
			for _ in 0..7 {
				b.a(None);
			}
			b.u16(0x0102); // eECU_VARIANT
			b.a(None);
		}
	}

	/// The tail every `DB_PROJECT_DATA` ends on, with `nested` sub-objects.
	fn project_data_tail(b: &mut Bytes, nested_variants: usize) {
		reference_bytes(b, None, None); // the trailing ECU variant reference
		b.a(None).a(None).a(None); // three names
		b.empty_map(); // no functional groups
		b.u16(nested_variants as u16);
		for _ in 0..nested_variants {
			b.some(code::DB_PROJECT_DATA);
			// A whole nested project data: its own location references, its own
			// references, its own (empty) tail.
			b.u16(1);
			location_reference_bytes(b, 1);
			reference_bytes(b, None, None);
			reference_bytes(b, Some("BV_Test"), Some("0.0.0@BV_Test.bv"));
			b.u16(0);
			project_data_tail(b, 0);
		}
	}

	#[test]
	fn project_data_lists_a_pools_variants() {
		let mut b = Bytes::default();
		// Two location references, both carrying an access key — the shape
		// every real base-variant pool opens with.
		b.u16(2);
		location_reference_bytes(&mut b, 1);
		location_reference_bytes(&mut b, 1);
		reference_bytes(&mut b, Some("FG_AllUDSSyste"), Some("0.0.0@FG_AllUDSSyste.fg"));
		reference_bytes(&mut b, Some("BV_EnginContrModul1UDS"), Some("0.0.0@BV_EnginContrModul1UDS.bv"));
		b.u16(2);
		for name in ["EV_ECM18TFS0208V0906264H", "EV_ECM20TDI01105L906022BN"] {
			named_reference_bytes(&mut b, name, Some(name), Some("0.0.0@BV_EnginContrModul1UDS.bv"));
		}
		project_data_tail(&mut b, 1);
		let (body, strings) = b.done(code::DB_PROJECT_DATA);

		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let data = project_data(&mut stream).expect("project data parses");
		stream.end().expect("the loader lands on the terminator");
		assert_eq!(data.base_variant.object.as_deref(), Some("BV_EnginContrModul1UDS"));
		assert_eq!(data.ecu_variants.len(), 2);
		assert_eq!(data.ecu_variants[0].0.as_deref(), Some("EV_ECM18TFS0208V0906264H"));
		assert_eq!(data.ecu_variants[0].1.object.as_deref(), Some("EV_ECM18TFS0208V0906264H"));
		assert_eq!(data.ecu_variants[1].1.pool.as_deref(), Some("0.0.0@BV_EnginContrModul1UDS.bv"));
	}

	/// A named-reference collection carries two names per entry, not three.
	///
	/// The regression this exists for: with three, entry *n*'s object id is
	/// entry *n-1*'s pool id, and after 402 entries the cursor is deep inside
	/// somebody else's access keys. It looked like a working parse right up to
	/// the terminator.
	#[test]
	fn a_named_reference_carries_two_names_not_three() {
		let mut b = Bytes::default();
		b.u16(2);
		named_reference_bytes(&mut b, "EV_A", Some("EV_A"), Some("POOL"));
		named_reference_bytes(&mut b, "EV_B", Some("EV_B"), Some("POOL"));
		let (body, strings) = b.done(0x0001);
		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let refs = named_references(&mut stream).expect("a collection parses");
		stream.end().expect("two names per entry lands on the terminator");
		assert_eq!(refs[0].1.object.as_deref(), Some("EV_A"));
		assert_eq!(refs[1].0.as_deref(), Some("EV_B"));
		assert_eq!(refs[1].1.pool.as_deref(), Some("POOL"));
	}

	#[test]
	fn an_ecu_variant_names_itself_and_its_base() {
		let mut b = Bytes::default();
		b.a(Some("BV_EnginContrModul1UDS")).a(Some("0.0.0@BV_EnginContrModul1UDS.bv")); // the base variant reference
		b.u8(0); // no matching patterns
		b.a(Some("EV_ECM18TFS0208V0906264H"))
			.u(Some("Motorsteuergerät"))
			.u(None)
			.a(None)
			.a(None)
			.a(None);
		b.empty_map(); // no location references
		let (body, strings) = b.done(code::MCD_DB_ECU_VARIANT);

		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let variant = ecu_variant(&mut stream).expect("an ECU variant parses");
		stream.end().expect("the loader lands on the terminator");
		assert_eq!(variant.short_name.as_deref(), Some("EV_ECM18TFS0208V0906264H"));
		assert_eq!(variant.long_name.as_deref(), Some("Motorsteuergerät"));
		assert_eq!(variant.base_variant.object.as_deref(), Some("BV_EnginContrModul1UDS"));
		assert_eq!(variant.patterns, 0);
	}

	/// An ECU's location list carries access keys, and the variant is still
	/// read: the key is stepped over, never parsed.
	///
	/// The pairing that keeps this honest is in `loaders::mod`: dispatching
	/// `MCD_ACCESS_KEY` still yields `Outcome::Refused`, so nothing builds one.
	/// Stepping over bytes is not parsing them, and refusing to step was an
	/// inability to read the car rather than a safety property.
	#[test]
	fn an_access_key_is_stepped_over_not_parsed() {
		let mut b = Bytes::default();
		b.a(Some("BV_Engin")).a(Some("0.0.0@BV_Engin.bv"));
		b.u8(0); // no matching patterns
		b.a(Some("EV_ECM")).u(Some("Motorsteuergerät")).u(None).a(None).a(None).a(None);
		b.u16(1); // one location reference, name then the reference itself
		b.a(Some("Loc"));
		b.a(Some("EV_ECM")).a(Some("0.0.0@BV_Engin.bv")).u8(1);
		b.u16(code::MCD_ACCESS_KEY);
		for _ in 0..7 {
			b.a(None);
		}
		b.u16(0x0102).a(None);
		let (body, strings) = b.done(code::MCD_DB_ECU_VARIANT);

		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let variant = ecu_variant(&mut stream).expect("an access key must not stop the parse");
		stream.end().expect("stepping over the key lands on the terminator");
		assert_eq!(variant.short_name.as_deref(), Some("EV_ECM"));
		assert_eq!(variant.long_name.as_deref(), Some("Motorsteuergerät"));

		// And the type itself is still refused wherever it is dispatched.
		let body = code::MCD_ACCESS_KEY.to_le_bytes().to_vec();
		let (type_code, mut stream) = Stream::open(&body, &strings).expect("a two-byte head is a type code");
		assert_eq!(
			super::super::load(type_code, &mut stream).expect("refusing is not an error"),
			super::super::Outcome::Refused
		);
	}

	#[test]
	fn layer_data_indexes_a_variants_services() {
		let mut b = Bytes::default();
		b.a(Some("EV_ECM18TFS0208V0906264H")).a(None).a(Some("UDS")).a(Some("UDSOnCAN")).a(None);
		b.u16(0x0102); // eECU_VARIANT
		b.a(Some("EV_ECM18TFS0208V0906264H")).a(Some("0.0.0@BV_EnginContrModul1UDS.bv"));

		// One service: a DbDiagComObjectReference keyed by its short name.
		b.u16(1);
		b.a(Some(RDBI_MEASUREMENT));
		b.a(Some("DiagnServi_ReadDataByIdentMeasuValue")).a(Some("0.0.0@BL_LIBECM.sd")).u8(0);
		b.u8(0).u16(0x0C83).u8(0);

		b.empty_map(); // no DTC properties
		// One data object property, one table, then four empty maps.
		b.u16(1).a(Some("DOP_2Byte")).a(Some("DOP_2Byte")).a(Some("0.0.0@BL_LIBECM.sd"));
		b.u16(1).a(Some("TAB_Measu")).a(Some("TAB_Measu")).a(Some("0.0.0@BL_LIBECM.sd")).u8(0);
		b.empty_map().empty_map().empty_map();
		b.empty_map(); // no functional-class data primitives
		b.empty_map().empty_map().empty_map(); // the three always-empty maps
		b.empty_map(); // no environment-data descriptions
		b.u16(1).a(Some("0.0.0@BL_LIBECM.sd")); // one parent layer
		b.empty_map(); // no shared-data parents
		b.empty_map().empty_map().empty_map().empty_map(); // four string-vector maps
		b.empty_map().empty_map(); // unit groups, units
		b.empty_map(); // no protocol parameters
		b.u8(0).u8(0); // the trailing byte, and no special data groups
		b.empty_map(); // the final diag-com map
		let (body, strings) = b.done(code::DB_LAYER_DATA);

		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let layer = layer_data(&mut stream).expect("layer data parses");
		assert!(layer.complete, "a whole tail must report itself complete");
		assert_eq!(stream.remaining(), 0, "the loader consumes its own terminator");
		assert_eq!(layer.layer, Layer::EcuVariant);
		assert_eq!(layer.variant.object.as_deref(), Some("EV_ECM18TFS0208V0906264H"));
		assert_eq!(layer.services.len(), 1);
		assert_eq!(layer.services[0].0.as_deref(), Some(RDBI_MEASUREMENT));
		assert_eq!(layer.services[0].1.pool.as_deref(), Some("0.0.0@BL_LIBECM.sd"));
		assert_eq!(layer.properties.len(), 1);
		assert_eq!(layer.tables.len(), 1);
		assert_eq!(layer.parents, vec!["0.0.0@BL_LIBECM.sd".to_owned()]);
	}

	/// A tail this reader cannot follow costs the tail, not the layer.
	///
	/// 34 of the reference project's 663 layer-data objects end in a shape that
	/// does not parse. Every one of them still names its services, which is all
	/// the measurement chain wants — so the head is kept, `complete` says the
	/// terminator was not reached, and the 34 control units keep their channels.
	#[test]
	fn a_tail_that_cannot_be_followed_still_yields_the_services() {
		let mut b = Bytes::default();
		b.a(Some("EV_Test")).a(None).a(Some("UDS")).a(Some("UDSOnCAN")).a(None);
		b.u16(0x0102);
		b.a(Some("EV_Test")).a(Some("0.0.0@BV_Test.bv"));
		b.u16(1);
		b.a(Some(RDBI_MEASUREMENT));
		b.a(Some("SVC")).a(Some("0.0.0@BL_Test.sd")).u8(0);
		b.u8(0).u16(0x0C83).u8(0);
		b.empty_map(); // no DTC properties
		b.empty_map().empty_map(); // no property or table index
		// And then a tail of nonsense, which no amount of field order recovers.
		b.u8(0xEE).u8(0xEE).u8(0xEE).u8(0xEE);
		let (body, strings) = b.done(code::DB_LAYER_DATA);

		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let layer = layer_data(&mut stream).expect("a bad tail must not cost the head");
		assert!(!layer.complete, "an unfollowed tail must say so");
		assert_eq!(layer.services.len(), 1);
		assert_eq!(layer.services[0].0.as_deref(), Some(RDBI_MEASUREMENT));
		assert!(layer.parents.is_empty(), "an unfollowed tail yields no parents");
	}
}
