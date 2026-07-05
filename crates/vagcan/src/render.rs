//! Pure rendering of what `doctor` prints — kept free of I/O and hardware so
//! the output formatting is unit-tested with captured identify bytes.

use vag_hex::CableIdentity;

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
