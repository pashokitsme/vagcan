//! A data-driven measurement catalog: [`MeasurementDef`] rows that join a UDS
//! read address to its raw byte form, scaling, unit and name — the model the
//! roadmap (`todo/README.md` §M3, `.archive/research/labels/rod-labels.md` §5) sketches for
//! turning `UDS 22 <DID>` responses into `name = value unit`.
//!
//! ## Provenance — why this catalog is hand-seeded, not machine-built
//! The *intended* source of these rows is a decoded engine `.rod`: its `MWB`
//! list (`<text-id>,<code>` rows — see [`crate::mwb`]) joined to the global
//! `STRUC`/`TTDOP`/`MUX` tables for the byte spec + scaling + unit, and to
//! `TTTEXT` for the name. The **codec is now fully decoded** — the per-table
//! substitution of [`crate::glyphs`] reads `STRUC`/`MUX`/`TTDOP` at 100 %
//! coverage, and the scalings this project proved by driving (`0.4`, `0.01`,
//! `0.001`, `1.0`, …) are all present in the label files. So the earlier "base-14,
//! field segmentation not reversed" blocker is gone (`.archive/research/labels/scaling-audit.md`).
//!
//! Two things still block the *automatic* path, and both were re-confirmed under
//! that correct decode (not the retired base-14 one):
//! - **The read DID is not in the label files.** Re-running the DID search over the
//!   correctly-decoded fields reproduces `rod-labels.md` §4.0c's chance-level
//!   negative; `label-linkage.md` §3 / `tttext2.md` §6.2a add that the per-ECU
//!   payload has no per-ECU degree of freedom to hold it. The label files never says
//!   which DID a measurement is read at.
//! - **The measurement→structure join is unproven.** A car's ADVMB measurement
//!   text-ids are not name-reachable in the global tables, and where a name is
//!   present it resolves to the *self-test* DOP with a different scaling (engine
//!   speed `×0.25` in MUX vs the proven ADVMB `×1`) — a name-join is a trap. The
//!   per-ECU `code → structure id` edge stays refuted (`scaling-audit.md` §4).
//!
//! Therefore this catalog is currently seeded **only with rows proven
//! empirically** from the owner's capture (see [`crate::measure`]): nothing is
//! read out of the label files, and no scaling slope is invented. As more
//! measurements are validated against the capture/CSV crib, they are added here
//! as data rows — the extensible foundation the roadmap calls for (add a
//! parameter = add a row, never a new match arm).

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::measure::{LinearScale, RawForm};

/// How a measurement value is addressed on the ECU. Today only UDS
/// `ReadDataByIdentifier` (service `0x22`, a 16-bit DID) is modelled; group
/// reads (`RecordLocalId`) can be added as a sibling variant when needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReadId {
	/// UDS `ReadDataByIdentifier` DID (the `62 <hi> <lo>` echo identifies it).
	Uds(u16),
}

/// A measurement's raw→engineering scaling knowledge. Kept as an explicit enum
/// so a **partially** reversed measurement is representable without fabricating
/// the missing part — the honest state for this car, where the ignition zero
/// point is proven but the slope is not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Scaling {
	/// A fully-proven linear COMPU method: `value = raw * factor + offset`.
	Linear(LinearScale),
	/// A discrete state: each raw value means one thing, and there is no
	/// scale between them.
	///
	/// A gear, a selector position or a switch is not a quantity. Forcing one
	/// into [`Scaling::Linear`] produces confident nonsense — on this car the
	/// gear code is `gear + 1`, so `factor 1, offset −1` would report the
	/// reverse-gear code `0C` as "gear 11" and neutral as "gear −1", across a
	/// third of a recording. Anything not listed is reported as unknown rather
	/// than extrapolated.
	Enum {
		/// Each interval of raw values and what it means. A reading takes the
		/// **first** level that holds it, in this order — see [`Level`].
		levels: Vec<Level>,
	},
	/// Only a single `(raw, value)` point is proven; the slope is **not** yet
	/// reversed. Interpreting any other raw value would be a guess, so it is
	/// deliberately not attempted (see [`MeasurementDef::interpret`]).
	Anchor {
		/// The proven raw integer (already read per the measurement's
		/// [`RawForm`]).
		raw: i32,
		/// The engineering value VCDS displays for that raw.
		value: f64,
	},
}

/// One state of a [`Scaling::Enum`]: an interval of raw values, both ends
/// included, and what any value in it means.
///
/// **An interval, not a point**, because that is what an ODX `TEXTTAB` says: each
/// `COMPU-SCALE` carries a coded lower *and* upper limit. Most are one value wide
/// (`lower == upper`), and the rest tile a range — a switch read as an analogue
/// voltage answers anywhere inside its band, with a count or two of noise, and
/// matching the lower bound alone matched none of those readings. A level that
/// is one value wide is a [`Level::point`], and behaves exactly as the
/// `(raw, name)` pairs this type replaced.
///
/// **First match wins, in table order.** ODX expects a text table's intervals not
/// to overlap, and the reference project (548,887 scales in 172,240 tables) has no
/// overlap at all — so on real data the order decides nothing. It is the rule
/// anyway, rather than a refusal at parse time, because refusing would cost a
/// whole channel its every name over one bad row, and because it is what a
/// lookup of point levels always did. Order is not sorted: 3,965 of those
/// tables list their intervals out of order, and they mean the same unsorted.
/// One exception to table order: a level with an unbounded end is tried after
/// every bounded one — see [`level_for`], which is the whole rule.
///
/// The bounds are private so that `lower <= upper` always holds; see
/// [`Level::range`] for what a reversed pair becomes.
///
/// On disk a point is `[raw, "name"]` — the shape every catalog file had before
/// intervals were kept, so old files read unchanged and a catalog with no
/// interval in it is still readable by an older build — and an interval is
/// `[lower, upper, "name"]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "LevelForm", into = "LevelForm")]
pub struct Level {
	lower: i32,
	upper: i32,
	name: String,
}

impl Level {
	/// A level exactly one raw value wide.
	pub fn point(raw: i32, name: impl Into<String>) -> Level {
		Level {
			lower: raw,
			upper: raw,
			name: name.into(),
		}
	}

	/// A level covering `lower..=upper`.
	///
	/// A reversed pair (`upper < lower`) is taken as the point at `lower`: that
	/// is the key this project read off every text table before it kept the
	/// upper bound, so a malformed row means what it always meant rather than
	/// matching nothing — or, swapped, a range nobody wrote.
	pub fn range(lower: i32, upper: i32, name: impl Into<String>) -> Level {
		Level {
			lower,
			upper: upper.max(lower),
			name: name.into(),
		}
	}

	/// The lowest raw value this level answers to — the one to send when the
	/// level has to be turned back into bytes.
	pub fn lower(&self) -> i32 {
		self.lower
	}

	/// The highest raw value this level answers to.
	pub fn upper(&self) -> i32 {
		self.upper
	}

	/// What the state is called.
	pub fn name(&self) -> &str {
		&self.name
	}

	/// Whether an end of this level is unbounded: `i32::MIN` below or `i32::MAX`
	/// above, which is how an ODX limit of kind `INFINITE` is kept. See
	/// [`level_for`] for what that changes.
	///
	/// Decided by the value, so any bound equal to `i32::MIN` or `i32::MAX` counts —
	/// a closed limit written as that value, and an unsigned upper limit past
	/// `i32::MAX` capped to it, included — and such a level is tried after every
	/// bounded one. On valid ODX data, whose levels do not overlap, the order in
	/// which levels are tried changes no answer, so this is harmless there.
	pub fn is_unbounded(&self) -> bool {
		self.lower == i32::MIN || self.upper == i32::MAX
	}

	/// Whether a raw reading falls inside this level, both ends included.
	pub fn contains(&self, raw: i32) -> bool {
		(self.lower..=self.upper).contains(&raw)
	}
}

/// Which level names a raw reading: its index in `levels`, or `None`.
///
/// **The one matching rule** — [`MeasurementDef::describe`] and the replay that
/// turns a name back into bytes both go through it, so the two cannot disagree
/// about which level a value belongs to. Bounded levels are tried first, in table
/// order; a level with an unbounded end ([`Level::is_unbounded`] — an ODX limit of
/// kind `INFINITE`) only after all of them, in table order. An unbounded level is
/// a catch-all, and listed first it would otherwise hide every level after it.
pub fn level_for(levels: &[Level], raw: i32) -> Option<usize> {
	let bounded = |unbounded: bool| levels.iter().enumerate().filter(move |(_, level)| level.is_unbounded() == unbounded);
	bounded(false)
		.chain(bounded(true))
		.find(|(_, level)| level.contains(raw))
		.map(|(at, _)| at)
}

/// How a [`Level`] is written in a catalog file: `[raw, "name"]` for a point,
/// `[lower, upper, "name"]` for an interval.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum LevelForm {
	Point(i32, String),
	Range(i32, i32, String),
}

impl From<LevelForm> for Level {
	fn from(form: LevelForm) -> Level {
		match form {
			LevelForm::Point(raw, name) => Level::point(raw, name),
			LevelForm::Range(lower, upper, name) => Level::range(lower, upper, name),
		}
	}
}

impl From<Level> for LevelForm {
	fn from(level: Level) -> LevelForm {
		match level.lower == level.upper {
			true => LevelForm::Point(level.lower, level.name),
			false => LevelForm::Range(level.lower, level.upper, level.name),
		}
	}
}

/// One catalog row: a fully-described (or honestly partially-described)
/// measurement. Strings are [`Cow`] so a compile-time-seeded row borrows a
/// `&'static str` for free while a config- or crib-loaded row owns a `String` —
/// the same type serves both the hand-proven constants and the future
/// `.rod`/capture-driven catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasurementDef {
	/// Human name (as VCDS displays it).
	pub name: Cow<'static, str>,
	/// Engineering unit (`"°"`, `"/min"`, `"°C"`, …); empty if dimensionless.
	pub unit: Cow<'static, str>,
	/// How the value is read from the ECU.
	pub address: ReadId,
	/// How to extract the raw integer from the RDBI response data bytes.
	pub raw_form: RawForm,
	/// Raw→engineering scaling (possibly only an anchor — see [`Scaling`]).
	pub scaling: Scaling,
}

impl MeasurementDef {
	/// Interpret an RDBI response's **data bytes** (everything after the
	/// `62 <DID hi> <DID lo>` echo) into an engineering value.
	///
	/// Returns `None` when `data` is too short for [`Self::raw_form`], or when
	/// the scaling is only an [`Scaling::Anchor`] and the observed raw differs
	/// from the proven anchor point (the slope being unknown, no honest value
	/// can be produced). A fully [`Scaling::Linear`] row always converts.
	pub fn interpret(&self, data: &[u8]) -> Option<f64> {
		let raw = self.raw_form.read(data)?;
		match &self.scaling {
			Scaling::Linear(s) => Some(s.apply(raw)),
			Scaling::Anchor { raw: a, value } => (raw == *a).then_some(*value),
			// A state has no numeric value; ask for its name instead.
			Scaling::Enum { .. } => None,
		}
	}

	/// What to show a person: the name of a state, or the value and its unit.
	///
	/// `None` when the bytes cannot be read at all, or when they carry a state
	/// this definition does not know — an unlisted code is reported as unknown,
	/// never guessed at.
	pub fn describe(&self, data: &[u8]) -> Option<String> {
		let raw = self.raw_form.read(data)?;
		match &self.scaling {
			Scaling::Enum { levels } => level_for(levels, raw).map(|at| levels[at].name.clone()),
			_ => {
				let value = self.interpret(data)?;
				Some(if self.unit.is_empty() {
					round(value)
				} else {
					format!("{} {}", round(value), self.unit)
				})
			}
		}
	}
}

/// Format a measured value for a person to read.
///
/// Three decimals, and no trailing zeros. The finest scaling this project has
/// proven is ×0.001 (boost pressure, in bar), so three decimals is exactly the
/// resolution the car reports and a fourth would be arithmetic rather than
/// measurement. Whole numbers print whole: an odometer is not `212805.000`.
fn round(value: f64) -> String {
	let text = format!("{value:.3}");
	match text.contains('.') {
		true => text.trim_end_matches('0').trim_end_matches('.').to_string(),
		false => text,
	}
}

/// The engine-ECU **ignition-angle family**, the one measurement group proven
/// against the owner's engine-running capture (`.archive/research/labels/rod-labels.md` §4.0a):
/// each DID returns raw `0x5555` (big-endian `u16`) for a displayed **0.00°**,
/// cross-validated four ways. The per-cylinder pairing of these four DIDs to
/// `IDE00155/156/157/158` is **not individually determined** (all four read a
/// constant `0.00°` over the capture), so the name is the family name, not a
/// cylinder number, and the slope is left as an [`Scaling::Anchor`] (unproven).
pub fn ignition_angle() -> Vec<MeasurementDef> {
	[0xA058u16, 0xA059, 0xA05E, 0xA05F].into_iter().map(ignition_def).collect()
}

/// Build a proven ignition-angle catalog row for one DID.
fn ignition_def(did: u16) -> MeasurementDef {
	MeasurementDef {
		name: Cow::Borrowed("Ignition angle"),
		unit: Cow::Borrowed(crate::measure::IGNITION_ANGLE_UNIT),
		address: ReadId::Uds(did),
		raw_form: RawForm::U16Be,
		scaling: Scaling::Anchor {
			raw: crate::measure::IGNITION_ANGLE_ZERO_RAW as i32,
			value: 0.0,
		},
	}
}

/// Measurement definitions on disk, one file per control unit, named after
/// what that unit calls itself.
///
/// A scaling is a property of a *control unit*, not of a project: `0x202A` is
/// boost pressure on engine `8V0906264H` and means nothing in particular on
/// another. So catalogs are keyed by the unit's own part number (`F187`) or
/// ODX file name (`F19E`) — both of which any VAG car reports about itself —
/// and are loaded at run time from a directory rather than compiled in. A car
/// this project has never seen simply finds no file and reads raw bytes,
/// which is the honest outcome; it never gets another car's numbers applied to
/// it.
#[derive(Debug, Clone)]
pub struct CatalogStore {
	dir: std::path::PathBuf,
}

impl CatalogStore {
	/// The directory being read, so a caller can list what is on offer
	/// without duplicating the path handling.
	pub fn dir(&self) -> &std::path::Path {
		&self.dir
	}

	pub fn open(dir: impl Into<std::path::PathBuf>) -> Self {
		CatalogStore { dir: dir.into() }
	}

	/// The rows known for a unit, given what it reported about itself.
	///
	/// The part number is tried first and the ODX name second; a unit that
	/// reports neither, or reports something with no file, gets nothing. Keys
	/// are matched with spaces and padding removed, because control units pad
	/// these strings to a fixed width.
	pub fn for_unit(&self, part_number: Option<&str>, odx_name: Option<&str>) -> Vec<MeasurementDef> {
		[part_number, odx_name]
			.into_iter()
			.flatten()
			.filter_map(|key| self.load(key))
			.next()
			.unwrap_or_default()
	}

	/// Load one catalog by key. `None` when there is no such file or it does
	/// not parse — a broken catalog must not be silently half-applied.
	pub fn load(&self, key: &str) -> Option<Vec<MeasurementDef>> {
		let key = Self::normalise(key);
		if key.is_empty() {
			return None;
		}
		let text = std::fs::read_to_string(self.dir.join(format!("{key}.json"))).ok()?;
		match MeasurementCatalog::from_json(&text) {
			Ok(catalog) => Some(catalog.defs),
			Err(e) => {
				eprintln!("catalog {key}.json is not readable: {e}");
				None
			}
		}
	}

	/// A control unit reports `"5E0920740D "` with trailing padding, and part
	/// numbers are written with spaces as often as without.
	fn normalise(key: &str) -> String {
		key.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_uppercase()
	}
}

/// A selectable set of measurement definitions — the user's chosen catalog.
///
/// Serializable so it round-trips to a config file (`load`/`save`): the user
/// picks which measurements to read, and a crib-derived or `.rod`-derived
/// catalog persists as data, never code.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MeasurementCatalog {
	pub defs: Vec<MeasurementDef>,
}

impl MeasurementCatalog {
	/// A catalog holding the given definitions.
	pub fn new(defs: Vec<MeasurementDef>) -> Self {
		MeasurementCatalog { defs }
	}

	/// The rows for one control unit, from a store on disk plus the families
	/// this project proved by measurement rather than by label.
	///
	/// Nothing car-specific is compiled in: the store answers by the unit's
	/// own part number or ODX name, so a car the project has never seen gets
	/// an empty catalog rather than another car's scalings.
	pub fn for_unit(store: &CatalogStore, part_number: Option<&str>, odx_name: Option<&str>) -> Self {
		MeasurementCatalog::new(store.for_unit(part_number, odx_name))
	}

	pub fn len(&self) -> usize {
		self.defs.len()
	}

	pub fn is_empty(&self) -> bool {
		self.defs.is_empty()
	}

	/// Serialize the catalog to pretty JSON (the config format).
	pub fn to_json(&self) -> Result<String, serde_json::Error> {
		serde_json::to_string_pretty(self)
	}

	/// Parse a catalog from JSON (a saved config).
	pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
		serde_json::from_str(s)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A tiny catalog written to a temp directory, so the store's own mechanism
	/// — reading a file, keying it by part number, and refusing to merge two
	/// units — can be tested without shipping any car's proven rows. A real
	/// vehicle's catalogs live under `~/.vagcan/data/measured` after a drive, not
	/// in the repository; asserting their contents is a job for the machine that
	/// measured them.
	fn synthetic_store() -> (tempfile::TempDir, CatalogStore) {
		let dir = tempfile::tempdir().expect("a temp dir");
		let write = |part: &str, defs: Vec<MeasurementDef>| {
			let cat = MeasurementCatalog::new(defs);
			std::fs::write(dir.path().join(format!("{part}.json")), cat.to_json().unwrap()).unwrap();
		};
		let linear = |name: &str, did: u16, factor: f64| MeasurementDef {
			name: Cow::Owned(name.to_string()),
			unit: Cow::Borrowed("x"),
			address: ReadId::Uds(did),
			raw_form: RawForm::U8First,
			scaling: Scaling::Linear(LinearScale { factor, offset: 0.0 }),
		};
		// Two units that both answer 0xF40D, at different factors, to prove the
		// sets are kept apart rather than merged.
		write("AAA111", vec![linear("engine speed", 0xF40D, 1.0)]);
		write("BBB222", vec![linear("road speed", 0xF40D, 2.0)]);
		let store = CatalogStore::open(dir.path());
		(dir, store)
	}

	#[test]
	fn the_store_reads_a_catalog_off_disk_keyed_by_the_part_number() {
		let (_dir, store) = synthetic_store();
		assert_eq!(store.load("AAA111").expect("the file is there").len(), 1);
		// Padding and spacing in a reported part number must not matter —
		// control units pad these strings to a fixed width.
		assert_eq!(store.for_unit(Some("AAA 111 "), None).len(), 1);
		// A part this store has never seen gets nothing, not another unit's rows.
		assert!(store.for_unit(Some("NOSUCHPART"), None).is_empty());
	}

	#[test]
	fn the_same_identifier_on_two_units_is_kept_apart() {
		// The property that made per-unit files necessary: 0xF40D means one byte
		// of one thing on the first unit and another factor on the second.
		// Merging the sets would silently make one of them wrong.
		let (_dir, store) = synthetic_store();
		let a = store.load("AAA111").unwrap();
		let b = store.load("BBB222").unwrap();
		assert_eq!(a[0].interpret(&[0x0A]), Some(10.0));
		assert_eq!(b[0].interpret(&[0x0A]), Some(20.0));
	}

	#[test]
	fn linear_row_interprets_every_raw() {
		// The reusable machinery: a fully-known linear row converts any raw.
		let def = MeasurementDef {
			name: Cow::Borrowed("Coolant temp"),
			unit: Cow::Borrowed("°C"),
			address: ReadId::Uds(0x1234),
			raw_form: RawForm::U8First,
			scaling: Scaling::Linear(LinearScale { factor: 0.75, offset: -48.0 }),
		};
		// 0x80 * 0.75 - 48 = 48.0
		assert_eq!(def.interpret(&[0x80]), Some(48.0));
		assert_eq!(def.interpret(&[0x00]), Some(-48.0));
		// too short for the form
		assert_eq!(
			MeasurementDef {
				raw_form: RawForm::U16Be,
				..def
			}
			.interpret(&[0x01]),
			None
		);
	}

	#[test]
	fn anchor_row_only_converts_the_proven_point() {
		// Honest partial knowledge: the anchor row yields the proven value at
		// the proven raw, and refuses (None) elsewhere — no invented slope.
		let defs = ignition_angle();
		let def = &defs[0];
		// The exact captured data bytes after the `62 A0 58` echo.
		assert_eq!(def.interpret(&[0x55, 0x55]), Some(0.0));
		// Any other raw: slope unknown -> no guess.
		assert_eq!(def.interpret(&[0x57, 0xE9]), None);
		assert_eq!(def.interpret(&[0x00]), None); // too short
	}

	#[test]
	fn ignition_family_is_the_four_proven_dids_at_the_zero_point() {
		// Cross-check the catalog against the proven crib: exactly the four
		// ignition DIDs, each U16Be, each mapping raw 0x5555 -> 0.00°.
		let defs = ignition_angle();
		let dids: Vec<u16> = defs
			.iter()
			.map(|d| match d.address {
				ReadId::Uds(did) => did,
			})
			.collect();
		assert_eq!(dids, vec![0xA058, 0xA059, 0xA05E, 0xA05F]);
		assert_eq!(dids, crate::measure::IGNITION_ANGLE_ZERO_DIDS.to_vec());
		for def in &defs {
			assert_eq!(def.unit, "°");
			assert_eq!(def.raw_form, RawForm::U16Be);
			// The captured raw 0x5555 -> displayed 0.00°.
			assert_eq!(def.interpret(&[0x55, 0x55]), Some(0.0));
		}
	}

	#[test]
	fn a_discrete_state_reports_its_name_and_refuses_to_extrapolate() {
		// The gear as this car encodes it: code = gear + 1, and reverse sits
		// at 0x0C where no arithmetic reaches it.
		let def = MeasurementDef {
			name: Cow::Borrowed("Selected gear"),
			unit: Cow::Borrowed(""),
			address: ReadId::Uds(0x3816),
			raw_form: RawForm::U8First,
			scaling: Scaling::Enum {
				levels: vec![Level::point(0x00, "not engaged"), Level::point(0x03, "2"), Level::point(0x0C, "R")],
			},
		};
		assert_eq!(def.describe(&[0x03]).as_deref(), Some("2"));
		assert_eq!(def.describe(&[0x0C]).as_deref(), Some("R"));
		assert_eq!(def.describe(&[0x00]).as_deref(), Some("not engaged"));
		// A code not in the table is unknown, not extrapolated.
		assert_eq!(def.describe(&[0x09]), None);
		// And a state is never a number.
		assert_eq!(def.interpret(&[0x03]), None);
	}

	/// A state read as an analogue voltage: every level is a band, with gaps a
	/// reading can fall into. Synthetic numbers, no car's table.
	fn banded() -> MeasurementDef {
		MeasurementDef {
			name: Cow::Borrowed("Switch"),
			unit: Cow::Borrowed(""),
			address: ReadId::Uds(0x1000),
			raw_form: RawForm::U8First,
			scaling: Scaling::Enum {
				levels: vec![
					Level::range(10, 19, "low"),
					// Listed out of order, as real tables are: order is not a key.
					Level::range(40, 49, "high"),
					Level::range(20, 29, "middle"),
					Level::point(60, "exactly sixty"),
				],
			},
		}
	}

	#[test]
	fn a_reading_inside_an_interval_takes_that_levels_name() {
		let def = banded();
		for (raw, name) in [
			(10, "low"),
			(14, "low"),
			(19, "low"),
			(20, "middle"),
			(27, "middle"),
			(40, "high"),
			(49, "high"),
		] {
			assert_eq!(def.describe(&[raw]).as_deref(), Some(name), "raw {raw}");
		}
		// A band is a name, never a number.
		assert_eq!(def.interpret(&[14]), None);
	}

	#[test]
	fn a_point_level_still_matches_exactly_and_only_itself() {
		let def = banded();
		assert_eq!(def.describe(&[60]).as_deref(), Some("exactly sixty"));
		assert_eq!(def.describe(&[59]), None);
		assert_eq!(def.describe(&[61]), None);
	}

	#[test]
	fn a_reading_between_intervals_is_unknown() {
		let def = banded();
		for raw in [0, 9, 30, 39, 50, 255] {
			assert_eq!(def.describe(&[raw]), None, "raw {raw} is in no interval");
		}
	}

	#[test]
	fn overlapping_intervals_answer_with_the_first_in_table_order() {
		let def = MeasurementDef {
			scaling: Scaling::Enum {
				levels: vec![Level::range(0, 10, "first"), Level::range(5, 15, "second")],
			},
			..banded()
		};
		assert_eq!(def.describe(&[5]).as_deref(), Some("first"));
		assert_eq!(def.describe(&[10]).as_deref(), Some("first"));
		assert_eq!(def.describe(&[11]).as_deref(), Some("second"));
	}

	#[test]
	fn a_level_with_an_unbounded_end_is_matched_after_every_bounded_one() {
		// An ODX limit of kind INFINITE is a catch-all; listed first, first-match
		// would let it hide every level after it. Bounded levels are tried first,
		// in table order, then the unbounded ones, in table order.
		let def = MeasurementDef {
			scaling: Scaling::Enum {
				levels: vec![
					Level::range(i32::MIN, 100, "at most 100"),
					Level::point(5, "five"),
					Level::range(200, i32::MAX, "at least 200"),
					Level::range(150, 300, "mid"),
				],
			},
			..banded()
		};
		assert_eq!(def.describe(&[5]).as_deref(), Some("five"));
		assert_eq!(def.describe(&[50]).as_deref(), Some("at most 100"));
		assert_eq!(def.describe(&[250]).as_deref(), Some("mid"));
		assert_eq!(def.describe(&[255]).as_deref(), Some("mid"));
		assert_eq!(def.describe(&[120]), None);
		let def = MeasurementDef {
			raw_form: RawForm::U16Be,
			..def
		};
		assert_eq!(def.describe(&[0x01, 0x90]).as_deref(), Some("at least 200"));
		let Scaling::Enum { levels } = &def.scaling else { unreachable!() };
		assert_eq!(level_for(levels, 250), Some(3));
		assert_eq!(level_for(levels, 400), Some(2));
	}

	#[test]
	fn a_reversed_interval_is_its_lower_bound_alone() {
		let level = Level::range(30, 20, "backwards");
		assert_eq!((level.lower(), level.upper()), (30, 30));
		assert!(level.contains(30));
		assert!(!level.contains(25));
	}

	#[test]
	fn a_catalog_written_before_intervals_reads_every_level_as_a_point() {
		// The shape every `measurements/` file had: `[raw, "name"]` pairs.
		let old = r#"{ "defs": [ { "name": "Gear", "unit": "", "address": { "Uds": 4096 }, "raw_form": "U8First",
			"scaling": { "Enum": { "levels": [[0, "none"], [3, "two"]] } } } ] }"#;
		let catalog = MeasurementCatalog::from_json(old).expect("an old catalog still reads");
		assert_eq!(
			catalog.defs[0].scaling,
			Scaling::Enum {
				levels: vec![Level::point(0, "none"), Level::point(3, "two")]
			}
		);
		assert_eq!(catalog.defs[0].describe(&[3]).as_deref(), Some("two"));
		assert_eq!(catalog.defs[0].describe(&[2]), None);
	}

	#[test]
	fn an_interval_round_trips_and_a_point_is_written_the_old_way() {
		let catalog = MeasurementCatalog::new(vec![banded()]);
		let json = catalog.to_json().expect("serialises");
		assert_eq!(MeasurementCatalog::from_json(&json).expect("reads back"), catalog);
		// A point is still `[raw, "name"]`, so a file with no interval in it is
		// one an older build reads too.
		let compact: String = json.chars().filter(|c| !c.is_whitespace()).collect();
		assert!(compact.contains(r#"[60,"exactlysixty"]"#), "{compact}");
		assert!(compact.contains(r#"[10,19,"low"]"#), "{compact}");
	}

	#[test]
	fn values_are_rounded_to_the_resolution_the_car_reports() {
		// ×0.001 is the finest scaling proven on this car, so a fourth decimal
		// would be arithmetic rather than measurement — and a whole number
		// prints whole, because an odometer is not 212805.000.
		assert_eq!(round(1.0005678), "1.001");
		assert_eq!(round(0.991), "0.991");
		assert_eq!(round(212_805.0), "212805");
		assert_eq!(round(90.5), "90.5");
		assert_eq!(round(-0.25), "-0.25");
		// The case that made this necessary: a division that lands just off.
		assert_eq!(round(717.4999999999999), "717.5");
		assert_eq!(round(1.0 / 3.0), "0.333");
	}

	/// A `measurements/` file exactly as the seven-variant `RawForm` wrote it,
	/// byte for byte, before [`RawForm::Int`] existed. Kept as a literal so the
	/// guard below cannot drift with the code it is guarding: regenerating it
	/// from the current enum would test that today agrees with today.
	const A_FILE_WRITTEN_BEFORE_THE_WIDENING: &str = r#"{
  "defs": [
    { "name": "one", "unit": "x", "address": { "Uds": 4096 },
      "raw_form": "U8First",  "scaling": { "Linear": { "factor": 1.0, "offset": 0.0 } } },
    { "name": "two", "unit": "x", "address": { "Uds": 4097 },
      "raw_form": "U8Second", "scaling": { "Linear": { "factor": 1.0, "offset": 0.0 } } },
    { "name": "three", "unit": "x", "address": { "Uds": 4098 },
      "raw_form": "U16Be",    "scaling": { "Linear": { "factor": 1.0, "offset": 0.0 } } },
    { "name": "four", "unit": "x", "address": { "Uds": 4099 },
      "raw_form": "U16Le",    "scaling": { "Linear": { "factor": 1.0, "offset": 0.0 } } },
    { "name": "five", "unit": "x", "address": { "Uds": 4100 },
      "raw_form": "I16Be",    "scaling": { "Linear": { "factor": 1.0, "offset": 0.0 } } },
    { "name": "six", "unit": "x", "address": { "Uds": 4101 },
      "raw_form": "U24Be",    "scaling": { "Linear": { "factor": 1.0, "offset": 0.0 } } },
    { "name": "seven", "unit": "x", "address": { "Uds": 4102 },
      "raw_form": "U32Be",    "scaling": { "Linear": { "factor": 1.0, "offset": 0.0 } } }
  ]
}"#;

	#[test]
	fn a_file_written_before_the_widening_still_reads_and_still_means_the_same() {
		// The rows under `~/.vagcan/data/<project>/measurements/` were proven by
		// driving a car and can be recreated by nothing else. Widening `RawForm`
		// by *renaming* its variants — `U16Be` becoming a struct form — would
		// have made every one of those files unparseable, silently, on the next
		// run. So the seven names are load-bearing file format, and this is the
		// test that says so.
		let back = MeasurementCatalog::from_json(A_FILE_WRITTEN_BEFORE_THE_WIDENING).expect("an old file still parses");
		let forms: Vec<RawForm> = back.defs.iter().map(|d| d.raw_form).collect();
		assert_eq!(
			forms,
			vec![
				RawForm::U8First,
				RawForm::U8Second,
				RawForm::U16Be,
				RawForm::U16Le,
				RawForm::I16Be,
				RawForm::U24Be,
				RawForm::U32Be
			]
		);
		// And they still decode what they always decoded — the cluster's odometer
		// bytes read as 24 and as 32 bits, and a negative through `I16Be`.
		assert_eq!(back.defs[5].interpret(&[0x03, 0x3F, 0x18]), Some(212_760.0));
		assert_eq!(back.defs[6].interpret(&[0x0C, 0xAF, 0x39, 0x8D]), Some(212_810_125.0));
		assert_eq!(back.defs[4].interpret(&[0xFF, 0xFE]), Some(-2.0));

		// A row derived from a project file today lands on the same name, so it
		// compares equal to the row on disk rather than shadowing it.
		assert_eq!(RawForm::for_field(0, 16, false, true), Some(back.defs[2].raw_form));
	}

	#[test]
	fn a_row_that_needs_the_general_form_survives_the_file_it_is_written_to() {
		// The other direction: the shapes that were unsayable until now have to
		// round-trip through the same file, or they are expressible in memory
		// and lost on save.
		let def = |name: &str, did: u16, raw_form: RawForm| MeasurementDef {
			name: Cow::Owned(name.to_string()),
			unit: Cow::Borrowed("bar"),
			address: ReadId::Uds(did),
			raw_form,
			scaling: Scaling::Linear(LinearScale { factor: 0.001, offset: 0.0 }),
		};
		let cat = MeasurementCatalog::new(vec![
			def(
				"signed word, little end first",
				0x1000,
				RawForm::Int {
					byte_offset: 0,
					byte_length: 2,
					signed: true,
					big_endian: false,
				},
			),
			def(
				"a word four bytes in",
				0x1001,
				RawForm::Int {
					byte_offset: 4,
					byte_length: 2,
					signed: false,
					big_endian: true,
				},
			),
		]);
		let back = MeasurementCatalog::from_json(&cat.to_json().expect("serialize")).expect("deserialize");
		assert_eq!(back, cat);
		// -208 * 0.001 and 0x1234 * 0.001, from the bytes the measure tests
		// hand-computed; through `describe` because a raw `f64` compare is a
		// test of binary floating point rather than of the decoder.
		assert_eq!(back.defs[0].describe(&[0x30, 0xFF]).as_deref(), Some("-0.208 bar"));
		assert_eq!(back.defs[1].describe(&[0, 0, 0, 0, 0x12, 0x34]).as_deref(), Some("4.66 bar"));
	}

	#[test]
	fn catalog_round_trips_through_json_config() {
		// The user-facing config: a catalog (a proven anchor family plus a fully
		// linear row) survives save→load byte-for-byte, so config selection is
		// pure data. Seeded from synthetic defs rather than a car's file — this
		// is a test of the serialization, not of any vehicle.
		let mut cat = MeasurementCatalog::new(ignition_angle());
		cat.defs.push(MeasurementDef {
			name: Cow::Owned("Engine RPM".to_string()),
			unit: Cow::Owned("/min".to_string()),
			address: ReadId::Uds(0xF40C),
			raw_form: RawForm::U16Be,
			scaling: Scaling::Linear(LinearScale { factor: 0.25, offset: 0.0 }),
		});

		let json = cat.to_json().expect("serialize");
		let back = MeasurementCatalog::from_json(&json).expect("deserialize");

		assert_eq!(back, cat);
		assert_eq!(back.len(), 5); // 4 ignition + 1 RPM
		// The linear row interprets a real raw after the round-trip.
		let rpm = &back.defs[4];
		assert_eq!(rpm.interpret(&[0x0B, 0x34]), Some(717.0)); // 0x0B34=2868 *0.25
	}
}
