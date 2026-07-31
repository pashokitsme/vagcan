//! Standard OBD-II sensors, read through their UDS mirrors.
//!
//! A VAG control unit exposes the legislated OBD-II parameters at
//! `0xF400 + PID`: reading data identifier `F405` returns what OBD-II mode 01
//! PID `05` would. The conversions are the public SAE J1979 ones, so this whole
//! family is decodable without reverse-engineering anything.
//!
//! **This is not assumed — it is measured.** Five of these were proven
//! independently by crossing a passive CAN capture with a simultaneous VCDS log
//! (`vagcan analyse`, 2026-08-01), and every one came out exactly as the
//! standard defines, including a two-byte pressure with a ×10 factor:
//!
//! | DID | fitted from the car | J1979 |
//! |---|---|---|
//! | `F405` | `raw − 40` °C | `A − 40` |
//! | `F40D` | `raw` km/h | `A` |
//! | `F40F` | `raw − 40` °C | `A − 40` |
//! | `F423` | `raw × 10` kPa | `(256A+B) × 10` |
//! | `F446` | `raw − 40` °C | `A − 40` |
//!
//! A test in this module pins that agreement, so the table cannot drift away
//! from the evidence that justifies trusting it.
//!
//! Only parameters with a **linear** conversion are listed. Bitfields (which
//! PIDs are supported, which monitors are ready), enumerations (fuel type, OBD
//! standard) and the multi-field lambda parameters are deliberately absent
//! rather than forced into a scale factor.

use std::borrow::Cow;

use crate::catalog::{MeasurementDef, ReadId, Scaling};
use crate::measure::{LinearScale, RawForm};

/// The UDS data identifier a mode-01 PID is mirrored at.
pub const fn did_for_pid(pid: u8) -> u16 {
    0xF400 | pid as u16
}

/// One standard parameter: how to read it and what it means.
pub struct ObdPid {
    pub pid: u8,
    pub name: &'static str,
    pub unit: &'static str,
    pub form: RawForm,
    pub factor: f64,
    pub offset: f64,
}

/// The linear mode-01 parameters, as defined by SAE J1979.
///
/// `A` is the first data byte and `B` the second, matching the standard's own
/// notation: `U8First` is `A`, `U16Be` is `256A + B`.
pub const PIDS: &[ObdPid] = &[
    ObdPid { pid: 0x04, name: "Calculated engine load", unit: "%", form: RawForm::U8First, factor: 100.0 / 255.0, offset: 0.0 },
    ObdPid { pid: 0x05, name: "Coolant temperature", unit: "°C", form: RawForm::U8First, factor: 1.0, offset: -40.0 },
    ObdPid { pid: 0x06, name: "Short term fuel trim, bank 1", unit: "%", form: RawForm::U8First, factor: 100.0 / 128.0, offset: -100.0 },
    ObdPid { pid: 0x07, name: "Long term fuel trim, bank 1", unit: "%", form: RawForm::U8First, factor: 100.0 / 128.0, offset: -100.0 },
    ObdPid { pid: 0x0B, name: "Intake manifold absolute pressure", unit: "kPa", form: RawForm::U8First, factor: 1.0, offset: 0.0 },
    ObdPid { pid: 0x0C, name: "Engine speed", unit: "/min", form: RawForm::U16Be, factor: 0.25, offset: 0.0 },
    ObdPid { pid: 0x0D, name: "Vehicle speed", unit: "km/h", form: RawForm::U8First, factor: 1.0, offset: 0.0 },
    ObdPid { pid: 0x0E, name: "Timing advance", unit: "°", form: RawForm::U8First, factor: 0.5, offset: -64.0 },
    ObdPid { pid: 0x0F, name: "Intake air temperature", unit: "°C", form: RawForm::U8First, factor: 1.0, offset: -40.0 },
    ObdPid { pid: 0x10, name: "Mass air flow", unit: "g/s", form: RawForm::U16Be, factor: 0.01, offset: 0.0 },
    ObdPid { pid: 0x11, name: "Throttle position", unit: "%", form: RawForm::U8First, factor: 100.0 / 255.0, offset: 0.0 },
    ObdPid { pid: 0x1F, name: "Run time since engine start", unit: "s", form: RawForm::U16Be, factor: 1.0, offset: 0.0 },
    ObdPid { pid: 0x21, name: "Distance with warning lamp on", unit: "km", form: RawForm::U16Be, factor: 1.0, offset: 0.0 },
    ObdPid { pid: 0x23, name: "Fuel rail gauge pressure", unit: "kPa", form: RawForm::U16Be, factor: 10.0, offset: 0.0 },
    ObdPid { pid: 0x2E, name: "Commanded evaporative purge", unit: "%", form: RawForm::U8First, factor: 100.0 / 255.0, offset: 0.0 },
    ObdPid { pid: 0x2F, name: "Fuel tank level", unit: "%", form: RawForm::U8First, factor: 100.0 / 255.0, offset: 0.0 },
    ObdPid { pid: 0x30, name: "Warm-ups since codes cleared", unit: "", form: RawForm::U8First, factor: 1.0, offset: 0.0 },
    ObdPid { pid: 0x31, name: "Distance since codes cleared", unit: "km", form: RawForm::U16Be, factor: 1.0, offset: 0.0 },
    ObdPid { pid: 0x33, name: "Absolute barometric pressure", unit: "kPa", form: RawForm::U8First, factor: 1.0, offset: 0.0 },
    ObdPid { pid: 0x3C, name: "Catalyst temperature, bank 1 sensor 1", unit: "°C", form: RawForm::U16Be, factor: 0.1, offset: -40.0 },
    ObdPid { pid: 0x3D, name: "Catalyst temperature, bank 2 sensor 1", unit: "°C", form: RawForm::U16Be, factor: 0.1, offset: -40.0 },
    ObdPid { pid: 0x3E, name: "Catalyst temperature, bank 1 sensor 2", unit: "°C", form: RawForm::U16Be, factor: 0.1, offset: -40.0 },
    ObdPid { pid: 0x42, name: "Control module voltage", unit: "V", form: RawForm::U16Be, factor: 0.001, offset: 0.0 },
    ObdPid { pid: 0x43, name: "Absolute load", unit: "%", form: RawForm::U16Be, factor: 100.0 / 255.0, offset: 0.0 },
    ObdPid { pid: 0x45, name: "Relative throttle position", unit: "%", form: RawForm::U8First, factor: 100.0 / 255.0, offset: 0.0 },
    ObdPid { pid: 0x46, name: "Ambient air temperature", unit: "°C", form: RawForm::U8First, factor: 1.0, offset: -40.0 },
    ObdPid { pid: 0x47, name: "Absolute throttle position B", unit: "%", form: RawForm::U8First, factor: 100.0 / 255.0, offset: 0.0 },
    ObdPid { pid: 0x49, name: "Accelerator pedal position D", unit: "%", form: RawForm::U8First, factor: 100.0 / 255.0, offset: 0.0 },
    ObdPid { pid: 0x4A, name: "Accelerator pedal position E", unit: "%", form: RawForm::U8First, factor: 100.0 / 255.0, offset: 0.0 },
    ObdPid { pid: 0x4C, name: "Commanded throttle actuator", unit: "%", form: RawForm::U8First, factor: 100.0 / 255.0, offset: 0.0 },
    ObdPid { pid: 0x5C, name: "Engine oil temperature", unit: "°C", form: RawForm::U8First, factor: 1.0, offset: -40.0 },
    ObdPid { pid: 0x5E, name: "Engine fuel rate", unit: "L/h", form: RawForm::U16Be, factor: 0.05, offset: 0.0 },
];

/// Look a parameter up by its PID.
pub fn pid(pid: u8) -> Option<&'static ObdPid> {
    PIDS.iter().find(|p| p.pid == pid)
}

impl ObdPid {
    /// The catalog row for reading this parameter over UDS.
    pub fn to_def(&self) -> MeasurementDef {
        MeasurementDef {
            name: Cow::Borrowed(self.name),
            unit: Cow::Borrowed(self.unit),
            address: ReadId::Uds(did_for_pid(self.pid)),
            raw_form: self.form,
            scaling: Scaling::Linear(LinearScale { factor: self.factor, offset: self.offset }),
        }
    }
}

/// Catalog rows for whichever parameters a control unit actually implements.
///
/// Feed it the identifiers a sweep found (`vagcan scan`); only the standard
/// linear parameters among them are returned.
pub fn catalog_for(supported_dids: &[u16]) -> Vec<MeasurementDef> {
    PIDS.iter()
        .filter(|p| supported_dids.contains(&did_for_pid(p.pid)))
        .map(|p| p.to_def())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pids_are_mirrored_at_f400_plus_the_pid() {
        assert_eq!(did_for_pid(0x05), 0xF405);
        assert_eq!(did_for_pid(0x0D), 0xF40D);
        assert_eq!(did_for_pid(0x46), 0xF446);
    }

    #[test]
    fn the_table_agrees_with_what_the_car_proved_independently() {
        // Five rows were fitted from a live capture against a VCDS log with
        // R² = 1.00000; the table must reproduce them exactly. If a future edit
        // breaks one of these, the table has drifted from the evidence.
        let coolant = pid(0x05).unwrap().to_def();
        assert_eq!(coolant.interpret(&[0x72]), Some(74.0)); // raw 0x72 = 114 → 74 °C
        assert_eq!(coolant.raw_form, RawForm::U8First);

        let speed = pid(0x0D).unwrap().to_def();
        assert_eq!(speed.interpret(&[114]), Some(114.0)); // the drive peaked here

        let intake = pid(0x0F).unwrap().to_def();
        assert_eq!(intake.interpret(&[0x69]), Some(65.0)); // 105 − 40

        let ambient = pid(0x46).unwrap().to_def();
        assert_eq!(ambient.interpret(&[0x3E]), Some(22.0)); // 62 − 40

        // The one that is neither a temperature nor a single byte: the fit gave
        // ×10 kPa over two big-endian bytes, exactly as J1979 defines PID 23.
        let rail = pid(0x23).unwrap().to_def();
        assert_eq!(rail.interpret(&[0x03, 0xA4]), Some(9320.0)); // 932 × 10
        assert_eq!(rail.raw_form, RawForm::U16Be);
    }

    #[test]
    fn engine_speed_uses_the_quarter_rpm_resolution() {
        // PID 0C is (256A + B) / 4, which is why it can report fractions.
        let rpm = pid(0x0C).unwrap().to_def();
        assert_eq!(rpm.interpret(&[0x0B, 0x34]), Some(717.0));
    }

    #[test]
    fn percentage_parameters_span_zero_to_one_hundred() {
        let load = pid(0x04).unwrap().to_def();
        assert_eq!(load.interpret(&[0x00]), Some(0.0));
        assert_eq!(load.interpret(&[0xFF]), Some(100.0));

        // Fuel trims are centred on zero, not on 50 %.
        let trim = pid(0x06).unwrap().to_def();
        assert_eq!(trim.interpret(&[128]), Some(0.0));
        assert_eq!(trim.interpret(&[0]), Some(-100.0));
    }

    #[test]
    fn only_supported_identifiers_make_it_into_a_catalog() {
        // The identifiers a sweep found on the reference engine, plus one the
        // table does not model (PID 13 is a bitfield) and one it does not have.
        let found = [0xF405u16, 0xF40C, 0xF413, 0xF446, 0x206E];
        let defs = catalog_for(&found);

        let dids: Vec<u16> = defs
            .iter()
            .map(|d| match d.address {
                ReadId::Uds(did) => did,
            })
            .collect();
        assert_eq!(dids, vec![0xF405, 0xF40C, 0xF446]);
        // A VW-specific identifier is not an OBD parameter and must not appear.
        assert!(!dids.contains(&0x206E));
        // Neither may a bitfield be dressed up as a scaled value.
        assert!(!dids.contains(&0xF413));
    }
}
