//! Pure rendering of what `doctor` prints — kept free of I/O and hardware so
//! the output formatting is unit-tested with captured identify bytes.

use vag_data::obd::Unconverted;
use vag_protocol::{EcuIdentity, Reading};

/// Placeholder for an identification field the ECU did not return.
const MISSING: &str = "—";

/// Render the full `vagcan info` report: the VIN once, then a labelled section
/// each for the Engine (address 01) and Gearbox (address 02). Pure formatting —
/// no I/O — so it is unit-tested against fixture identities.
pub fn render_info(vin: Option<&str>, engine: &EcuIdentity, gearbox: &EcuIdentity) -> String {
    let mut out = String::new();
    out.push_str(&format!("VIN: {}\n", vin.unwrap_or(MISSING)));
    out.push('\n');
    out.push_str(&render_ecu_section("Engine (01)", engine));
    out.push('\n');
    out.push_str(&render_ecu_section("Gearbox (02)", gearbox));
    out
}

/// One ECU's identification block, e.g.
/// ```text
/// Engine (01):
///   part number: 8V0906264H
///   ...
/// ```
fn render_ecu_section(label: &str, id: &EcuIdentity) -> String {
    let coding = id
        .coding
        .as_deref()
        .map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" "))
        .unwrap_or_else(|| MISSING.to_string());
    format!(
        "{label}:\n  \
         part number: {}\n  \
         hw number:   {}\n  \
         sw version:  {}\n  \
         component:   {}\n  \
         serial:      {}\n  \
         coding:      {}\n",
        id.part_number.as_deref().unwrap_or(MISSING),
        id.hw_number.as_deref().unwrap_or(MISSING),
        id.sw_version.as_deref().unwrap_or(MISSING),
        id.component.as_deref().unwrap_or(MISSING),
        id.serial.as_deref().unwrap_or(MISSING),
        coding,
    )
}

/// Render a [`CableIdentity`] into the multi-line block `vagcan doctor`
/// prints: the firmware string plus the raw identify payload as hex.
/// What to print when a control unit answers nothing at all.
fn nothing_answered() -> String {
    "The car did not answer.\n\n\
     Check, in this order:\n  \
     - the ignition is on\n  \
     - OBD-II pin 6 → CAN-H, pin 14 → CAN-L, pin 5 → GND\n  \
     - the adapter's termination jumper is OFF\n  \
     - the adapter really has a serial node: `vagcan devices`\n\n\
     Note that a silent bus is normal here: this platform's diagnostic line carries almost \
     no traffic until something queries it."
        .to_string()
}

/// Public wrapper (see [`nothing_answered`]).
pub fn render_nothing_answered() -> String {
    nothing_answered()
}

/// The noun for a count: `1 change`, `2 changes`.
///
/// Only regular plurals, because every count in this tool is of something with
/// one — identifiers, columns, changes, control units.
pub fn plural(n: usize, singular: &str) -> String {
    if n == 1 {
        singular.to_string()
    } else {
        format!("{singular}s")
    }
}

/// Bytes for a person to read: `0B 34`.
///
/// Everything shown on a screen uses this. The gap is what makes a byte
/// countable against a datasheet or a VCDS window, which is what anybody
/// looking at raw bytes is doing.
pub fn hex_spaced(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// Bytes for a file: `0B34`.
///
/// Separate from [`hex_spaced`] rather than a parameter of it, because this one
/// is a **format on disk**. `survey` writes it and `survey --diff` compares the
/// strings as text, so adding a separator here would report every identifier in
/// an existing pair of dumps as having moved. The two are not the same function
/// with a flag; they have different readers and only one of them can change.
pub fn hex_packed(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}

/// One `vagcan sensors` row: name, value, and a unit only when there is one.
///
/// Whether a row has a unit is a property of the measurement, not of the value
/// it happens to be carrying — so the unit column is decided by the definition.
/// The earlier version decided it by asking whether the value was a whole
/// number as well, which printed `Warm-ups since codes cleared  4.00 ` with a
/// trailing space for a reading of 4.5 and for the whole numbers it did catch
/// printed no unit but still no trailing space by luck rather than by rule.
/// Decimals are dropped for a unitless whole number: a count of warm-ups is not
/// a measurement to two decimal places.
pub fn render_sensor_row(reading: &Reading, width: usize) -> String {
    let name = &reading.name;
    let Some(value) = reading.value else {
        // The identifier answered but the bytes did not fit the form; the unit
        // belongs to the value that could not be formed, so it is not printed.
        return format!("  {name:<width$}  {:>10} (raw)", hex_spaced(&reading.raw));
    };
    if reading.unit.is_empty() {
        let text = match value.fract() == 0.0 {
            true => format!("{value:>10.0}"),
            false => format!("{value:>10.2}"),
        };
        return format!("  {name:<width$}  {text}");
    }
    format!("  {name:<width$}  {value:>10.2} {}", reading.unit)
}

/// One line of `vagcan sensors`: either a reading the standard conversion is
/// established for, or the bytes a unit answered with and the reason the tool
/// will not turn them into a number.
pub enum SensorLine {
    /// The conversion holds here (see [`vag_data::obd::conversion_for`]).
    Converted(Reading),
    /// It does not, so only the identifier and its bytes are shown. Nothing is
    /// named: the J1979 name would assert the very thing that is in doubt.
    Unconverted { did: u16, bytes: Vec<u8>, why: Unconverted },
}

/// The whole `vagcan sensors` report.
///
/// Converted rows first, then — under a heading that says why — the identifiers
/// whose bytes the tool refuses to interpret. The refusal is spelt out rather
/// than left as a silent omission: a user who sees `F405` answer on the climate
/// unit and no row for it deserves to know that the tool read it and would not
/// convert it.
pub fn render_sensors(unit_label: &str, lines: &[SensorLine]) -> String {
    let width = lines
        .iter()
        .filter_map(|l| match l {
            SensorLine::Converted(r) => Some(r.name.len()),
            SensorLine::Unconverted { .. } => None,
        })
        .max()
        .unwrap_or(0);

    let mut out = String::new();
    let mut converted = 0usize;
    for line in lines {
        if let SensorLine::Converted(r) = line {
            converted += 1;
            out.push_str(&render_sensor_row(r, width));
            out.push('\n');
        }
    }

    let refused: Vec<&SensorLine> =
        lines.iter().filter(|l| matches!(l, SensorLine::Unconverted { .. })).collect();
    if !refused.is_empty() {
        if converted > 0 {
            out.push('\n');
        }
        let off_block = refused
            .iter()
            .any(|l| matches!(l, SensorLine::Unconverted { why: Unconverted::NotAnEmissionsUnit, .. }));
        if off_block {
            out.push_str(&format!(
                "Control unit {unit_label} is not one ISO 15765-4 addresses for emissions\n\
                 diagnostics (0x7E0..0x7E7), so nothing establishes that its F4xx\n\
                 identifiers are the SAE J1979 parameters. On the reference car they are\n\
                 not. Bytes only:\n\n"
            ));
        } else {
            out.push_str(
                "Answered, but not at the width SAE J1979 defines — so these are not\n\
                 those parameters. Bytes only:\n\n",
            );
        }
        for line in refused {
            let SensorLine::Unconverted { did, bytes, why } = line else { continue };
            let note = match why {
                Unconverted::NotAnEmissionsUnit => String::new(),
                Unconverted::WrongWidth { expected, got } => {
                    format!("   ({got} {}, standard says {expected})", plural(*got, "byte"))
                }
            };
            out.push_str(&format!("  {did:04X}  {:>10}{note}\n", hex_spaced(bytes)));
        }
    }
    out.push_str(&format!("\n{converted} of {} standard sensors converted", lines.len()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(part: &str) -> EcuIdentity {
        EcuIdentity {
            vin: Some("XW8AD4NE9JH008917".to_string()),
            part_number: Some(part.to_string()),
            hw_number: Some("06K907425B".to_string()),
            sw_version: Some("0005".to_string()),
            component: Some("1.8l R4 TFSI".to_string()),
            serial: None,
            coding: Some(vec![0x0c, 0x25]),
        }
    }

    #[test]
    fn the_report_shows_the_real_cars_values() {
        // The values are the reference car's, read over CAN on 2026-08-01.
        let engine = identity("8V0906264H");
        let gearbox = EcuIdentity {
            part_number: Some("0CW300041G".to_string()),
            component: Some("GSG DQ200G2_M".to_string()),
            serial: Some("CU501702277773".to_string()),
            ..identity("0CW300041G")
        };
        let text = render_info(Some("XW8AD4NE9JH008917"), &engine, &gearbox);
        assert!(text.contains("XW8AD4NE9JH008917"), "{text}");
        assert!(text.contains("8V0906264H"), "{text}");
        assert!(text.contains("GSG DQ200G2_M"), "{text}");
        assert!(text.contains("CU501702277773"), "{text}");
    }

    #[test]
    fn a_missing_field_reads_as_absent_not_as_empty() {
        let mut engine = identity("8V0906264H");
        engine.serial = None;
        let text = render_info(None, &engine, &engine);
        assert!(text.contains(MISSING), "an absent field is marked, not blank: {text}");
    }

    fn reading(name: &str, unit: &str, value: Option<f64>) -> Reading {
        Reading {
            name: name.to_string(),
            unit: unit.to_string(),
            value,
            raw: vec![0x04],
        }
    }

    #[test]
    fn a_count_prints_as_an_integer_with_no_unit_column() {
        // PID 30 on the reference engine: a count, no unit. It used to print
        // `4.00 ` — two decimals it cannot have, and a trailing space where a
        // unit would have been.
        let row = render_sensor_row(&reading("Warm-ups since codes cleared", "", Some(4.0)), 28);
        assert_eq!(row, "  Warm-ups since codes cleared           4");
        assert_eq!(row.trim_end(), row, "no trailing space where a unit would be");

        // Unitless and NOT whole: still no unit column, decimals kept because
        // they are real.
        let row = render_sensor_row(&reading("Something", "", Some(4.5)), 9);
        assert_eq!(row, "  Something        4.50");
        assert_eq!(row.trim_end(), row);
    }

    #[test]
    fn a_measurement_keeps_its_unit_and_its_decimals() {
        let row = render_sensor_row(&reading("Coolant temperature", "°C", Some(74.0)), 19);
        assert_eq!(row, "  Coolant temperature       74.00 °C");
    }

    #[test]
    fn bytes_that_did_not_fit_the_form_are_shown_as_bytes() {
        let mut r = reading("Odd one", "km/h", None);
        r.raw = vec![0xAB, 0xCD];
        let row = render_sensor_row(&r, 7);
        assert!(row.contains("AB CD (raw)"), "{row}");
        // No unit on a value that was never formed — it would claim the bytes
        // had been understood.
        assert!(!row.contains("km/h"), "{row}");
    }

    /// The climate unit's own bytes, from `research/dumps/survey-parked.jsonl`
    /// (unit `0x746`, `5E0907044AM`).
    #[test]
    fn a_unit_off_the_iso_block_gets_bytes_and_a_reason_never_a_value() {
        let lines = vec![
            SensorLine::Unconverted {
                did: 0xF405,
                bytes: vec![0x57],
                why: Unconverted::NotAnEmissionsUnit,
            },
            SensorLine::Unconverted {
                did: 0xF40C,
                bytes: vec![0x00],
                why: Unconverted::NotAnEmissionsUnit,
            },
        ];
        let text = render_sensors("746", &lines);
        assert!(text.contains("F405          57"), "{text}");
        assert!(text.contains("ISO 15765-4"), "{text}");
        assert!(text.contains("0 of 2 standard sensors converted"), "{text}");
        // 0x57 is 87; as PID 05 that would print 47.00 °C. It must not appear,
        // nor may the parameter be named — the name is the claim in doubt.
        assert!(!text.contains("47.00"), "{text}");
        assert!(!text.contains("Coolant"), "{text}");
        assert!(!text.contains("°C"), "{text}");
    }

    #[test]
    fn a_wrong_width_row_says_so_and_the_good_rows_still_print() {
        // Engine coolant, which is established, alongside the gearbox's
        // two-byte F40D, which is not PID 0D.
        let lines = vec![
            SensorLine::Converted(reading("Coolant temperature", "°C", Some(89.0))),
            SensorLine::Unconverted {
                did: 0xF40D,
                bytes: vec![0x9C, 0x02],
                why: Unconverted::WrongWidth { expected: 1, got: 2 },
            },
        ];
        let text = render_sensors("02", &lines);
        assert!(text.contains("Coolant temperature       89.00 °C"), "{text}");
        assert!(text.contains("F40D       9C 02"), "{text}");
        assert!(text.contains("2 bytes, standard says 1"), "{text}");
        assert!(text.contains("1 of 2 standard sensors converted"), "{text}");
        // The first byte read as km/h would be 156. Never printed.
        assert!(!text.contains("156"), "{text}");
    }

    #[test]
    fn the_silence_message_names_the_checks_in_order() {
        let text = nothing_answered();
        assert!(text.contains("ignition"), "{text}");
        assert!(text.contains("pin 6"), "{text}");
        assert!(text.contains("vagcan devices"), "{text}");
        // Silence is not proof of a fault on this platform — say so.
        assert!(text.contains("silent bus is normal"), "{text}");
    }
}
