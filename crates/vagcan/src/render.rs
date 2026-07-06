//! Pure rendering of what `doctor` prints — kept free of I/O and hardware so
//! the output formatting is unit-tested with captured identify bytes.

use vag_hex::CableIdentity;
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
pub fn render_identity(id: &CableIdentity) -> String {
    let firmware = id.firmware.as_deref().unwrap_or("(no printable identity)");
    let raw = if id.raw.is_empty() {
        "(empty)".to_string()
    } else {
        id.raw
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!("cable identity:\n  firmware: {firmware}\n  raw:      {raw}")
}

/// Real captured f3-channel `b8` blocks (the 16-byte enciphered payloads) from
/// `reading-ecus.pcapng` — see `research/vag-hex-framing.md` "Link cipher".
const F3_TESTER_PRESENT: [u8; 16] = [
    0xf3, 0x83, 0x44, 0xdd, 0x7c, 0x5f, 0x00, 0x97, 0x99, 0xf6, 0xda, 0x7c, 0x9c, 0x3a, 0x00, 0xfc,
];
const F3_RDBI: [u8; 16] = [
    0xf3, 0x9f, 0x44, 0xdd, 0x7c, 0x5f, 0x01, 0x8b, 0xed, 0xae, 0xda, 0x7c, 0x9c, 0x3a, 0xfb, 0xfd,
];

/// PoC #2: recover the f3 channel keystream from the TesterPresent frame's
/// known plaintext, then decode a *different* frame of the same channel — real
/// captured car data, decode-only (no key derivation, see `SCOPE-BOUNDARY.md`).
pub fn render_decode_demo() -> String {
    // TesterPresent UDS region (off6..=13): PCI 0x02, SID 0x3E, sub 0x00, 0x00 pad.
    let mut crib = [None; 16];
    for (i, p) in [(6, 0x02u8), (7, 0x3E), (8, 0x00), (9, 0x00), (10, 0x00), (11, 0x00), (12, 0x00), (13, 0x00)] {
        crib[i] = Some(p);
    }
    let ks = vag_hex::link::recover_keystream(&F3_TESTER_PRESENT, &crib);
    let tp = vag_hex::link::decode_diag_frame(&F3_TESTER_PRESENT, &ks);
    let rdbi = vag_hex::link::decode_diag_frame(&F3_RDBI, &ks);

    let mut out = String::from(
        "link-cipher decode demo (real capture reading-ecus.pcapng, f3 channel)\n\
         keystream recovered from the TesterPresent frame's known plaintext, then\n\
         applied to decode this channel's frames to UDS:\n",
    );
    out.push_str(&render_uds_line("b8 frame 1", tp.as_ref().map(|s| s.uds.as_slice())));
    out.push_str(&render_uds_line("b8 frame 2", rdbi.as_ref().map(|s| s.uds.as_slice())));
    out
}

fn render_uds_line(label: &str, uds: Option<&[u8]>) -> String {
    match uds {
        Some(bytes) => {
            let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
            format!("  {label} -> UDS {hex}  ({})\n", uds_name(bytes))
        }
        None => format!("  {label} -> (not a single-frame UDS block)\n"),
    }
}

/// Human name for a UDS PDU by its service id, for the demo output.
fn uds_name(uds: &[u8]) -> &'static str {
    match uds.first() {
        Some(0x3E) => "TesterPresent",
        Some(0x22) => "ReadDataByIdentifier",
        Some(0x19) => "ReadDTCInformation",
        Some(0x10) => "DiagnosticSessionControl",
        _ => "unknown service",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real captured identify reply payload (bytes after the `0x04`
    /// opcode): `"ROSSTECH"` + NUL padding + version bytes. Same ground truth
    /// as `vag-hex`'s handshake tests (`research/vag-hex-framing.md` §4).
    const IDENTIFY_DATA: [u8; 16] = [
        0x52, 0x4f, 0x53, 0x53, 0x54, 0x45, 0x43, 0x48, // "ROSSTECH"
        0x00, 0x00, 0x00, // NUL padding
        0xa8, 0x9d, 0x01, 0x00, 0x09, // version bytes
    ];

    #[test]
    fn renders_captured_rosstech_identity() {
        let id = CableIdentity {
            firmware: Some("ROSSTECH a89d010009".into()),
            raw: IDENTIFY_DATA.to_vec(),
        };

        let out = render_identity(&id);

        assert_eq!(
            out,
            "cable identity:\n\
             \x20 firmware: ROSSTECH a89d010009\n\
             \x20 raw:      52 4f 53 53 54 45 43 48 00 00 00 a8 9d 01 00 09"
        );
    }

    #[test]
    fn renders_missing_firmware_as_placeholder() {
        let id = CableIdentity {
            firmware: None,
            raw: vec![0x00, 0xff],
        };

        let out = render_identity(&id);

        assert_eq!(
            out,
            "cable identity:\n\
             \x20 firmware: (no printable identity)\n\
             \x20 raw:      00 ff"
        );
    }

    #[test]
    fn decode_demo_recovers_and_decodes_real_uds() {
        let out = render_decode_demo();
        // f3 TesterPresent decodes to 3E 00; RDBI (same recovered keystream) to 22 74 58.
        assert!(out.contains("UDS 3e 00  (TesterPresent)"), "got:\n{out}");
        assert!(
            out.contains("UDS 22 74 58  (ReadDataByIdentifier)"),
            "got:\n{out}"
        );
    }

    #[test]
    fn renders_info_report_with_two_sections() {
        let engine = EcuIdentity {
            vin: Some("XW8AD4NE9JH008917".into()),
            part_number: Some("8V0906264H".into()),
            hw_number: Some("8V0906264".into()),
            sw_version: Some("0004".into()),
            component: Some("R4 1.8l TFSI".into()),
            serial: Some("VWZZZ7Z0K1234567".into()),
            coding: Some(vec![0x01, 0x2A, 0x00, 0x04]),
        };
        // Gearbox partially answered: no serial, no coding.
        let gearbox = EcuIdentity {
            part_number: Some("0CW300043".into()),
            hw_number: Some("0CW927769".into()),
            sw_version: Some("6002".into()),
            component: Some("DSG7 DQ200".into()),
            ..Default::default()
        };

        let out = render_info(Some("XW8AD4NE9JH008917"), &engine, &gearbox);

        assert_eq!(
            out,
            "VIN: XW8AD4NE9JH008917\n\
             \n\
             Engine (01):\n  \
             part number: 8V0906264H\n  \
             hw number:   8V0906264\n  \
             sw version:  0004\n  \
             component:   R4 1.8l TFSI\n  \
             serial:      VWZZZ7Z0K1234567\n  \
             coding:      01 2a 00 04\n\
             \n\
             Gearbox (02):\n  \
             part number: 0CW300043\n  \
             hw number:   0CW927769\n  \
             sw version:  6002\n  \
             component:   DSG7 DQ200\n  \
             serial:      —\n  \
             coding:      —\n"
        );
    }

    #[test]
    fn renders_empty_raw_without_trailing_space() {
        let id = CableIdentity {
            firmware: Some("ROSSTECH".into()),
            raw: vec![],
        };

        let out = render_identity(&id);

        assert_eq!(
            out,
            "cable identity:\n\
             \x20 firmware: ROSSTECH\n\
             \x20 raw:      (empty)"
        );
    }
}
