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
use super::{Ref, code, diag_com_reference_map, name_list, named_references, nested, reference, reference_map, string_vector_map};

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
	/// The pools this layer inherits from, nearest first.
	pub parents: Vec<String>,
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
pub fn project_data(stream: &mut Stream<'_>) -> Result<ProjectData, Error> {
	// A list of location references, each naming a pool and carrying access
	// keys. The keys are refused, and this list is not needed for anything —
	// but its length has to be read to stay aligned, and an entry that
	// actually carries a key stops the parse.
	let count = stream.count()?;
	for _ in 0..count {
		let _object = stream.ascii()?;
		let _pool = stream.ascii()?;
		if stream.u8()? != 0 {
			return Err(Error::Refused("MCD_ACCESS_KEY"));
		}
	}
	let _functional_group = reference(stream, true, false)?;
	let base_variant = reference(stream, true, false)?;
	let ecu_variants = named_references(stream)?;
	let _ecu_variant = reference(stream, true, false)?;
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
/// Returns the short and long name. The location references are read for
/// alignment; an entry carrying an access key stops the parse, because an
/// access key is refused and there is no way to read past one.
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
		let _object = stream.ascii()?;
		let _pool = stream.ascii()?;
		if stream.u8()? != 0 {
			return Err(Error::Refused("MCD_ACCESS_KEY"));
		}
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

	// Environment-data descriptions, each a whole nested object. Fault data,
	// not measurement data — read for alignment, kept by nobody yet.
	let count = stream.count()?;
	if count > 0 {
		return Err(Error::Format(format!(
			"a layer carries {count} environment-data descriptions, which this reader does not yet parse"
		)));
	}

	let parents = name_list(stream)?;
	let _shared_data_parents = name_list(stream)?;
	for _ in 0..4 {
		string_vector_map(stream)?;
	}
	let _unit_groups = reference_map(stream, false)?;
	let _units = reference_map(stream, false)?;

	Ok(LayerData {
		layer,
		variant,
		services,
		properties,
		tables,
		parents,
	})
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
		/// An empty map or list of `n`-byte count width.
		fn empty_map(&mut self) -> &mut Self {
			self.u16(0)
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

	/// A `DbObjectReference` with a third name.
	fn reference_bytes(b: &mut Bytes, object: Option<&str>, pool: Option<&str>) {
		b.a(object).a(pool).a(None);
	}

	#[test]
	fn project_data_lists_a_pools_variants() {
		let mut b = Bytes::default();
		b.empty_map(); // no location references
		reference_bytes(&mut b, Some("FG_AllUDSSyste"), Some("0.0.0@FG_AllUDSSyste.fg"));
		reference_bytes(&mut b, Some("BV_EnginContrModul1UDS"), Some("0.0.0@BV_EnginContrModul1UDS.bv"));
		b.u16(2);
		for name in ["EV_ECM18TFS0208V0906264H", "EV_ECM20TDI01105L906022BN"] {
			b.a(Some(name));
			reference_bytes(&mut b, Some(name), Some("0.0.0@BV_EnginContrModul1UDS.bv"));
		}
		reference_bytes(&mut b, None, None);
		let (body, strings) = b.done(code::DB_PROJECT_DATA);

		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let data = project_data(&mut stream).expect("project data parses");
		stream.end().expect("the loader lands on the terminator");
		assert_eq!(data.base_variant.object.as_deref(), Some("BV_EnginContrModul1UDS"));
		assert_eq!(data.ecu_variants.len(), 2);
		assert_eq!(data.ecu_variants[0].0.as_deref(), Some("EV_ECM18TFS0208V0906264H"));
		assert_eq!(data.ecu_variants[1].1.pool.as_deref(), Some("0.0.0@BV_EnginContrModul1UDS.bv"));
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

	/// An ECU whose location list carries an access key stops the parse and
	/// names what stopped it. This is the refusal list reaching into the
	/// identification chain, and it is the reason [`layer_data`] is found by
	/// scanning rather than through the key the reference implementation uses.
	#[test]
	fn an_access_key_in_a_location_reference_is_refused() {
		let mut b = Bytes::default();
		b.a(Some("BV_Engin")).a(Some("0.0.0@BV_Engin.bv"));
		b.u8(0);
		b.a(Some("EV_ECM")).u(None).u(None).a(None).a(None).a(None);
		b.u16(1); // one location reference…
		b.a(Some("Loc")).a(Some("EV_ECM")).a(Some("0.0.0@BV_Engin.bv")).u8(1); // …carrying a key
		let (body, strings) = b.done(code::MCD_DB_ECU_VARIANT);

		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let err = ecu_variant(&mut stream).expect_err("an access key must stop the parse");
		assert!(matches!(err, Error::Refused("MCD_ACCESS_KEY")), "got {err:?}");
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
		let (body, strings) = b.done(code::DB_LAYER_DATA);

		let (_, mut stream) = Stream::open(&body, &strings).expect("a well-formed object opens");
		let layer = layer_data(&mut stream).expect("layer data parses");
		stream.end().expect("the loader lands on the terminator");
		assert_eq!(layer.layer, Layer::EcuVariant);
		assert_eq!(layer.variant.object.as_deref(), Some("EV_ECM18TFS0208V0906264H"));
		assert_eq!(layer.services.len(), 1);
		assert_eq!(layer.services[0].0.as_deref(), Some(RDBI_MEASUREMENT));
		assert_eq!(layer.services[0].1.pool.as_deref(), Some("0.0.0@BL_LIBECM.sd"));
		assert_eq!(layer.properties.len(), 1);
		assert_eq!(layer.tables.len(), 1);
		assert_eq!(layer.parents, vec!["0.0.0@BL_LIBECM.sd".to_owned()]);
	}
}
