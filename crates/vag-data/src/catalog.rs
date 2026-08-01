//! A data-driven measurement catalog: [`MeasurementDef`] rows that join a UDS
//! read address to its raw byte form, scaling, unit and name — the model the
//! roadmap (`todo/README.md` §M3, `research/rod-labels.md` §5) sketches for
//! turning `UDS 22 <DID>` responses into `name = value unit`.
//!
//! ## Provenance — why this catalog is hand-seeded, not machine-built
//! The *intended* source of these rows is a decoded engine `.rod`: its `MWB`
//! list (`<text-id>,<code>` rows — see [`crate::mwb`]) joined to the global
//! `STRUC`/`TTDOP` tables for the read DID + byte spec + scaling + unit, and to
//! `TTTEXT` for the name. That automatic path is **blocked**: the `STRUC`/`DOP`
//! records are a proven base-14 packed codec whose **field segmentation is not
//! reversed** (`research/rod-labels.md` §2–§3), and — newly established here by
//! crossing the owner's engine-running capture crib (real valid DIDs) against
//! the decoded `STRUC` table — **the read DID is not stored in `STRUC` at all**
//! in any tested encoding (u16 BE/LE or a base-14 field at any offset), so the
//! `code → STRUC-id → DID` chain the roadmap hypothesised does not hold as
//! written. See the module tests and `research/rod-labels.md` for the evidence.
//!
//! Therefore this catalog is currently seeded **only with rows proven
//! empirically** from the owner's capture (see [`crate::measure`]): nothing is
//! read out of the unreversed codec, and no scaling slope is invented. As more
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
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Scaling {
    /// A fully-proven linear COMPU method: `value = raw * factor + offset`.
    Linear(LinearScale),
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
        match self.scaling {
            Scaling::Linear(s) => Some(s.apply(raw)),
            Scaling::Anchor { raw: a, value } => (raw == a).then_some(value),
        }
    }
}

/// The engine-ECU **ignition-angle family**, the one measurement group proven
/// against the owner's engine-running capture (`research/rod-labels.md` §4.0a):
/// each DID returns raw `0x5555` (big-endian `u16`) for a displayed **0.00°**,
/// cross-validated four ways. The per-cylinder pairing of these four DIDs to
/// `IDE00155/156/157/158` is **not individually determined** (all four read a
/// constant `0.00°` over the capture), so the name is the family name, not a
/// cylinder number, and the slope is left as an [`Scaling::Anchor`] (unproven).
pub fn ignition_angle() -> Vec<MeasurementDef> {
    [0xA058u16, 0xA059, 0xA05E, 0xA05F]
        .into_iter()
        .map(ignition_def)
        .collect()
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

/// Measurements proven against the car on 2026-08-01 by crossing a passive CAN
/// capture with a simultaneous VCDS log (`vagcan analyse`).
///
/// Each row fitted with `R² = 1.00000` — an exact linear relation, not a
/// correlation. Engine speed (`206E`) comes from the first, 19.7-second session
/// and rests on 14–16 matched samples spanning a real rev; the two boost
/// pressures (`2029`/`202A`) come from the later 16-minute capture, over ~100
/// matched points each (`research/rod-labels.md` §4.3a). They are specific to
/// this vehicle's control units (engine `8V0906264H`, gearbox `0CW300041G`);
/// another car's identifiers must be proven the same way rather than assumed.
///
/// The coolant fit from the same sessions is the load-bearing one for
/// confidence: it produced `raw − 40`, exactly the standard OBD-II PID 05
/// formula. Nothing in the pipeline knows that formula, so recovering it from
/// the data validates the alignment and the fitting end to end. It lives in
/// [`crate::obd`] with the rest of the standard family, not here.
pub fn proven_measurements() -> Vec<MeasurementDef> {
    proven_engine()
}

/// The shipped catalogs, embedded so the binary needs no data directory.
///
/// These are **files, not literals**: measurement definitions are data (see
/// `catalogs/README.md`), and a new measurement is a new row there rather than
/// a new branch here.
const ENGINE_CATALOG: &str = include_str!("../../../catalogs/engine-01.json");
const GEARBOX_CATALOG: &str = include_str!("../../../catalogs/gearbox-02.json");

/// Engine (address 01) rows that are NOT part of the OBD-II standard set —
/// VW's own identifiers, which only the capture crib could supply. Combine
/// with [`crate::obd::PIDS`] for the standard parameters.
pub fn proven_engine() -> Vec<MeasurementDef> {
    MeasurementCatalog::from_json(ENGINE_CATALOG)
        .expect("the shipped engine catalog is valid JSON")
        .defs
}

/// Gearbox (address 02) rows.
///
/// Kept separate from the engine set on purpose: **`F40D` means different
/// things on the two units** — one byte of km/h on the engine (the OBD-II
/// mirror), two little-endian bytes at ×0.01 here. A single combined catalog
/// could only be wrong for one of them.
pub fn proven_gearbox() -> Vec<MeasurementDef> {
    MeasurementCatalog::from_json(GEARBOX_CATALOG)
        .expect("the shipped gearbox catalog is valid JSON")
        .defs
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

    /// The catalog seeded with everything proven on the engine so far. This is
    /// the baseline a fresh install ships with; the gearbox has its own set
    /// (see [`proven_gearbox`]) because the two units disagree on `F40D`.
    pub fn seeded() -> Self {
        let mut defs = proven_engine();
        defs.extend(ignition_angle());
        MeasurementCatalog::new(defs)
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
        assert_eq!(MeasurementDef {
            raw_form: RawForm::U16Be, ..def
        }.interpret(&[0x01]), None);
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
    fn the_shipped_catalogs_are_loadable_data_not_code() {
        // The definitions live in catalogs/*.json. If they ever moved back
        // into Rust literals this test would still pass, so it also checks the
        // count matches the files as shipped.
        assert_eq!(proven_engine().len(), 3, "engine rows come from the file");
        assert_eq!(proven_gearbox().len(), 10, "gearbox rows come from the file");
    }

    #[test]
    fn the_proven_rows_reproduce_the_values_the_car_displayed() {
        // Spot values lifted straight from the 2026-08-01 session, so the
        // catalog cannot drift away from the evidence that justified it.
        let engine = proven_engine();
        let rpm = &engine[0];
        assert_eq!(rpm.interpret(&[0x02, 0xBD]), Some(701.0));

        // Boost came out at exactly ×0.001 bar over two big-endian bytes.
        let boost = &engine[2];
        assert_eq!(boost.interpret(&[0x03, 0xDF]), Some(0.991));
        assert_eq!(boost.unit, "bar");

        // The gearbox reports little-endian: `B2 02` is 690 /min, not 45570.
        let gearbox = proven_gearbox();
        let input = &gearbox[0];
        assert_eq!(input.interpret(&[0xB2, 0x02]), Some(690.0));
        assert_eq!(input.interpret(&[0xCC, 0x08]), Some(2252.0));
        assert_eq!(input.interpret(&[0x7A, 0x0E]), Some(3706.0));
    }

    #[test]
    fn the_two_units_disagree_about_f40d_and_the_catalogs_keep_them_apart() {
        // On the engine F40D is the OBD-II mirror: one byte of km/h. On the
        // gearbox it is two little-endian bytes at x0.01. Merging the sets
        // would silently make one of them wrong.
        assert!(!proven_engine().iter().any(|d| d.address == ReadId::Uds(0xF40D)));
        let gearbox_speed = proven_gearbox()
            .into_iter()
            .find(|d| d.address == ReadId::Uds(0xF40D))
            .expect("the gearbox set carries its own F40D");
        assert_eq!(gearbox_speed.raw_form, RawForm::U16Le);
        assert_eq!(gearbox_speed.interpret(&[0x0A, 0x1E]), Some(76.9)); // 0x1E0A = 7690
    }

    #[test]
    fn catalog_round_trips_through_json_config() {
        // The user-facing config: a catalog (mix of proven anchor + a fully
        // linear row) survives save→load byte-for-byte, so config selection is
        // pure data.
        let mut cat = MeasurementCatalog::seeded();
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
        assert_eq!(back.len(), 8); // 3 engine + 4 ignition + 1 RPM
        // The linear row interprets a real raw after the round-trip.
        let rpm = &back.defs[7];
        assert_eq!(rpm.interpret(&[0x0B, 0x34]), Some(717.0)); // 0x0B34=2868 *0.25
    }
}
