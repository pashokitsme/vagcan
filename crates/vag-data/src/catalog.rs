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

use crate::measure::{LinearScale, RawForm};

/// How a measurement value is addressed on the ECU. Today only UDS
/// `ReadDataByIdentifier` (service `0x22`, a 16-bit DID) is modelled; group
/// reads (`RecordLocalId`) can be added as a sibling variant when needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadId {
    /// UDS `ReadDataByIdentifier` DID (the `62 <hi> <lo>` echo identifies it).
    Uds(u16),
}

/// A measurement's raw→engineering scaling knowledge. Kept as an explicit enum
/// so a **partially** reversed measurement is representable without fabricating
/// the missing part — the honest state for this car, where the ignition zero
/// point is proven but the slope is not.
#[derive(Debug, Clone, Copy, PartialEq)]
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
/// measurement. `'static` strings because the seeded rows are compile-time
/// constants; a future `.rod`-driven loader would own `String`s instead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasurementDef {
    /// Human name (as VCDS displays it).
    pub name: &'static str,
    /// Engineering unit (`"°"`, `"/min"`, `"°C"`, …); empty if dimensionless.
    pub unit: &'static str,
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
pub const IGNITION_ANGLE: &[MeasurementDef] = &[
    ignition_def(0xA058),
    ignition_def(0xA059),
    ignition_def(0xA05E),
    ignition_def(0xA05F),
];

/// Build a proven ignition-angle catalog row for one DID (const-fn so
/// [`IGNITION_ANGLE`] is a compile-time constant).
const fn ignition_def(did: u16) -> MeasurementDef {
    MeasurementDef {
        name: "Ignition angle",
        unit: crate::measure::IGNITION_ANGLE_UNIT,
        address: ReadId::Uds(did),
        raw_form: RawForm::U16Be,
        scaling: Scaling::Anchor {
            raw: crate::measure::IGNITION_ANGLE_ZERO_RAW as i32,
            value: 0.0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_row_interprets_every_raw() {
        // The reusable machinery: a fully-known linear row converts any raw.
        let def = MeasurementDef {
            name: "Coolant temp",
            unit: "°C",
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
        let def = IGNITION_ANGLE[0];
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
        let dids: Vec<u16> = IGNITION_ANGLE
            .iter()
            .map(|d| match d.address {
                ReadId::Uds(did) => did,
            })
            .collect();
        assert_eq!(dids, vec![0xA058, 0xA059, 0xA05E, 0xA05F]);
        assert_eq!(dids, crate::measure::IGNITION_ANGLE_ZERO_DIDS.to_vec());
        for def in IGNITION_ANGLE {
            assert_eq!(def.unit, "°");
            assert_eq!(def.raw_form, RawForm::U16Be);
            // The captured raw 0x5555 -> displayed 0.00°.
            assert_eq!(def.interpret(&[0x55, 0x55]), Some(0.0));
        }
    }
}
