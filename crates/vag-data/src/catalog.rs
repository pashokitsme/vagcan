//! A data-driven measurement catalog: [`MeasurementDef`] rows that join a UDS
//! read address to its raw byte form, scaling, unit and name — the model the
//! roadmap (`todo/README.md` §M3, `research/labels/rod-labels.md` §5) sketches for
//! turning `UDS 22 <DID>` responses into `name = value unit`.
//!
//! ## Provenance — why this catalog is hand-seeded, not machine-built
//! The *intended* source of these rows is a decoded engine `.rod`: its `MWB`
//! list (`<text-id>,<code>` rows — see [`crate::mwb`]) joined to the global
//! `STRUC`/`TTDOP` tables for the read DID + byte spec + scaling + unit, and to
//! `TTTEXT` for the name. That automatic path is **blocked**: the `STRUC`/`DOP`
//! records are a proven base-14 packed codec whose **field segmentation is not
//! reversed** (`research/labels/rod-labels.md` §2–§3), and — newly established here by
//! crossing the owner's engine-running capture crib (real valid DIDs) against
//! the decoded `STRUC` table — **the read DID is not stored in `STRUC` at all**
//! in any tested encoding (u16 BE/LE or a base-14 field at any offset), so the
//! `code → STRUC-id → DID` chain the roadmap hypothesised does not hold as
//! written. See the module tests and `research/labels/rod-labels.md` for the evidence.
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
        /// `(raw value, what it means)`, in whatever order reads best.
        levels: Vec<(i32, String)>,
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
            Scaling::Enum { levels } => levels
                .iter()
                .find(|(value, _)| *value == raw)
                .map(|(_, name)| name.clone()),
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
/// against the owner's engine-running capture (`research/labels/rod-labels.md` §4.0a):
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
    pub fn for_unit(
        store: &CatalogStore,
        part_number: Option<&str>,
        odx_name: Option<&str>,
    ) -> Self {
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

    /// The reference car's catalogs, read from the repository's store.
    ///
    /// These files are evidence, not a shipped table: they are keyed by the
    /// part number each control unit reports, and the tests below check that
    /// the numbers still reproduce the readings that justified them.
    fn reference_store() -> CatalogStore {
        CatalogStore::open(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../catalogs/vehicles"),
        )
    }

    fn reference_engine() -> Vec<MeasurementDef> {
        reference_store().load("8V0906264H").expect("the reference engine catalog is present")
    }

    fn reference_gearbox() -> Vec<MeasurementDef> {
        reference_store().load("0CW300041G").expect("the reference gearbox catalog is present")
    }

    fn reference_cluster() -> Vec<MeasurementDef> {
        reference_store().load("5E0920740D").expect("the reference cluster catalog is present")
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
    fn a_discrete_state_reports_its_name_and_refuses_to_extrapolate() {
        // The gear as this car encodes it: code = gear + 1, and reverse sits
        // at 0x0C where no arithmetic reaches it.
        let def = MeasurementDef {
            name: Cow::Borrowed("Selected gear"),
            unit: Cow::Borrowed(""),
            address: ReadId::Uds(0x3816),
            raw_form: RawForm::U8First,
            scaling: Scaling::Enum {
                levels: vec![
                    (0x00, "not engaged".to_string()),
                    (0x03, "2".to_string()),
                    (0x0C, "R".to_string()),
                ],
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

    #[test]
    fn a_measured_quantity_still_describes_itself_with_its_unit() {
        let rpm = &reference_engine()[0];
        assert_eq!(rpm.describe(&[0x02, 0xBD]).as_deref(), Some("701 /min"));
    }

    #[test]
    fn catalogs_are_data_on_disk_keyed_by_what_a_unit_calls_itself() {
        // Nothing car-specific is compiled in. A unit is looked up by the part
        // number it reports, so a car this project has never seen gets an
        // empty catalog instead of another car's scalings.
        assert_eq!(reference_engine().len(), 3, "engine rows come from the file");
        assert_eq!(reference_gearbox().len(), 12, "gearbox rows come from the file");
        assert_eq!(reference_cluster().len(), 8, "cluster rows come from the file");
        // Padding and spacing in a reported part number must not matter —
        // control units pad these strings to a fixed width.
        assert_eq!(reference_store().for_unit(Some("5E0 920 740 D "), None).len(), 8);
        assert!(reference_store().for_unit(Some("NOSUCHPART"), None).is_empty());
        // The odometer reproduces the exact reading that justified it.
        assert_eq!(reference_cluster()[0].interpret(&[0x03, 0x3F, 0x18]), Some(212_760.0));
    }

    /// One row per cluster DID: `watch` keeps a single channel per (unit, DID)
    /// and the last definition wins, so a duplicate would silently shadow.
    #[test]
    fn the_cluster_catalog_has_one_row_per_identifier() {
        let mut dids: Vec<u16> = reference_cluster()
            .iter()
            .map(|d| match d.address {
                ReadId::Uds(did) => did,
            })
            .collect();
        let before = dids.len();
        dids.sort_unstable();
        dids.dedup();
        assert_eq!(dids.len(), before);
    }

    /// The metre-resolution odometer, pinned to the three readings that proved
    /// it: `22B8 / 1000` truncates to the `2203` odometer at every snapshot of
    /// the 2026-08-01/02 surveys, across a drive that moved both.
    ///
    /// A single wrong byte order, width or factor breaks all three at once —
    /// `U24Be` would read 212 805 188 as 13 300 262, and little-endian as
    /// 1 143 705 356.
    #[test]
    fn the_fine_odometer_agrees_with_the_kilometre_odometer_at_every_snapshot() {
        let cluster = reference_cluster();
        let coarse = &cluster[0];
        let fine = &cluster[1];
        assert_eq!(fine.raw_form, RawForm::U32Be);
        for (km, metres) in [
            ([0x03u8, 0x3F, 0x45], [0x0Cu8, 0xAF, 0x26, 0x44]), // parked
            ([0x03, 0x3F, 0x45], [0x0C, 0xAF, 0x26, 0x95]),     // driving, sweep 1
            ([0x03, 0x3F, 0x4A], [0x0C, 0xAF, 0x39, 0x8D]),     // driving, sweep 2
        ] {
            let coarse_km = coarse.interpret(&km).expect("the odometer reads");
            let fine_m = fine.interpret(&metres).expect("the fine odometer reads");
            assert_eq!((fine_m / 1000.0).floor(), coarse_km);
        }
        // 212 805.188 km, and the drive moved it 4 856 m while the kilometre
        // odometer stepped by 5.
        assert_eq!(fine.interpret(&[0x0C, 0xAF, 0x26, 0x44]), Some(212_805_188.0));
        assert_eq!(fine.unit, "m");
    }

    /// The car's own clock, as five identifiers that reassemble into the two
    /// block identifiers `2216` (`hh mm ss`) and `2217` (`yyyy mm dd`).
    ///
    /// Values are the reference car's parked survey: the cluster displayed
    /// 23:51:32 on the car's calendar day 2026-07-28, which landed within
    /// 4 s of the host clock's own reading of when that identifier was read.
    #[test]
    fn the_clock_rows_read_the_time_the_cluster_displayed() {
        let cluster = reference_cluster();
        let by_did = |did: u16| {
            cluster
                .iter()
                .find(|d| d.address == ReadId::Uds(did))
                .unwrap_or_else(|| panic!("the cluster set carries {did:#06X}"))
                .clone()
        };
        // 2216 read `17 33 20`, and 2238/2239 read its first two bytes.
        assert_eq!(by_did(0x2238).interpret(&[0x17]), Some(23.0));
        assert_eq!(by_did(0x2239).interpret(&[0x33]), Some(51.0));
        // 2217 read `07EA 07 1C`, and 223A/223B/223C read its three fields.
        assert_eq!(by_did(0x223A).interpret(&[0x07, 0xEA]), Some(2026.0));
        assert_eq!(by_did(0x223B).interpret(&[0x07]), Some(7.0));
        assert_eq!(by_did(0x223C).interpret(&[0x1C]), Some(28.0));
        // Little-endian would have read the year as 60 167, not 2026.
        assert_eq!(by_did(0x223A).raw_form, RawForm::U16Be);
    }

    /// Road speed, `22D2`: proven by timing against a VCDS log in
    /// `research/car/other-ecus.md` §4.1 over 0–5 km/h, and corroborated at 53 km/h
    /// by the 2026-08-02 driving surveys — where the metre odometer says the
    /// car covered 4 856 m in the 497 s between the two cluster reads, a mean
    /// of 35 km/h that brackets the 5 and 53 km/h read at the ends.
    ///
    /// A ×0.01 factor would make that drive 49 m long, and ×10 would make it
    /// 49 km; both are refuted by the odometer in the same response set.
    #[test]
    fn the_cluster_road_speed_is_kilometres_per_hour_at_factor_one() {
        let speed = reference_cluster()
            .into_iter()
            .find(|d| d.address == ReadId::Uds(0x22D2))
            .expect("the cluster set carries road speed");
        assert_eq!(speed.interpret(&[0x00, 0x00]), Some(0.0));
        assert_eq!(speed.interpret(&[0x00, 0x05]), Some(5.0));
        assert_eq!(speed.interpret(&[0x00, 0x35]), Some(53.0));
        assert_eq!(speed.unit, "km/h");
    }

    #[test]
    fn the_proven_rows_reproduce_the_values_the_car_displayed() {
        // Spot values lifted straight from the 2026-08-01 session, so the
        // catalog cannot drift away from the evidence that justified it.
        let engine = reference_engine();
        let rpm = &engine[0];
        assert_eq!(rpm.interpret(&[0x02, 0xBD]), Some(701.0));

        // Boost came out at exactly ×0.001 bar over two big-endian bytes.
        let boost = &engine[2];
        assert_eq!(boost.interpret(&[0x03, 0xDF]), Some(0.991));
        assert_eq!(boost.unit, "bar");

        // The gearbox reports little-endian: `B2 02` is 690 /min, not 45570.
        let gearbox = reference_gearbox();
        let input = &gearbox[0];
        assert_eq!(input.interpret(&[0xB2, 0x02]), Some(690.0));
        assert_eq!(input.interpret(&[0xCC, 0x08]), Some(2252.0));
        assert_eq!(input.interpret(&[0x7A, 0x0E]), Some(3706.0));
    }

    #[test]
    fn the_gear_row_matches_the_evidence_that_proved_it() {
        // Proven by ratio arithmetic against the already-proven shaft speeds:
        // from standstill the code stepped 02→03→…→08 in strict order, seven
        // consecutive codes on a seven-speed box, so gear = code − 1. Reverse
        // sits at 0C, outside any arithmetic.
        let gear = reference_gearbox()
            .into_iter()
            .find(|d| d.address == ReadId::Uds(0x3816))
            .expect("the gearbox set carries the gear");
        assert_eq!(gear.describe(&[0x02]).as_deref(), Some("1"));
        assert_eq!(gear.describe(&[0x08]).as_deref(), Some("7"));
        assert_eq!(gear.describe(&[0x0C]).as_deref(), Some("R"));
        assert_eq!(gear.describe(&[0x00]).as_deref(), Some("not engaged"));
        // 0x01 was never observed; it is not "gear 0".
        assert_eq!(gear.describe(&[0x01]), None);
    }

    #[test]
    fn the_two_units_disagree_about_f40d_and_the_catalogs_keep_them_apart() {
        // On the engine F40D is the OBD-II mirror: one byte of km/h. On the
        // gearbox it is two little-endian bytes at x0.01. Merging the sets
        // would silently make one of them wrong.
        assert!(!reference_engine().iter().any(|d| d.address == ReadId::Uds(0xF40D)));
        let gearbox_speed = reference_gearbox()
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
        let mut cat = MeasurementCatalog::for_unit(&reference_store(), Some("8V0906264H"), None);
        cat.defs.extend(ignition_angle());
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
