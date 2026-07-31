//! Pure rendering of what `doctor` prints — kept free of I/O and hardware so
//! the output formatting is unit-tested with captured identify bytes.

use vag_protocol::EcuIdentity;

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
