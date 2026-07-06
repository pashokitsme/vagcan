//! High-level ECU identification reader — the `vagcan info` core.
//!
//! Sits ABOVE [`AsyncUdsClient`]: it issues a fixed set of read-only
//! ReadDataByIdentifier (0x22) requests and folds the answers into one
//! [`EcuIdentity`]. Transport-generic (`T: AsyncIsoTpTransport`), so the exact
//! same reader runs against an in-memory mock in tests and a real CAN adapter
//! on the car — no hardware-specific code leaks in here.
//!
//! Read-only by construction: only 0x22 is ever emitted (the allowlist in
//! `pdu` would reject anything else anyway). Every DID is read independently —
//! an ECU that does not implement one identifier simply leaves that field
//! `None`; it never aborts the whole read.

use vag_transport::AsyncIsoTpTransport;

use crate::AsyncUdsClient;

/// Data identifiers we read for the identification block. ASCII-valued unless
/// noted. Values mirror the VW/UDS standard identifiers VCDS surfaces.
mod did {
    /// VIN (ISO 14229 standard identifier). 17 ASCII chars.
    pub const VIN: u16 = 0xF190;
    /// VW spare part number (e.g. `8V0906264H`).
    pub const PART_NUMBER: u16 = 0xF187;
    /// VW ECU hardware number.
    pub const HW_NUMBER: u16 = 0xF191;
    /// VW application software version number.
    pub const SW_VERSION: u16 = 0xF189;
    /// VW system name / component description (e.g. `R4 1.8l TFSI`).
    pub const COMPONENT: u16 = 0xF197;
    /// ECU serial number.
    pub const SERIAL: u16 = 0xF18C;
    /// VW coding value — raw bytes, NOT ASCII.
    pub const CODING: u16 = 0x0600;
}

/// One ECU's identification block. Every field is optional: a DID the ECU does
/// not answer stays `None`. ASCII fields are already trimmed (see
/// [`trim_ascii`]); `coding` is kept as raw bytes for hex rendering.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EcuIdentity {
    pub vin: Option<String>,
    pub part_number: Option<String>, // F187
    pub hw_number: Option<String>,   // F191
    pub sw_version: Option<String>,  // F189
    pub component: Option<String>,   // F197
    pub serial: Option<String>,      // F18C
    pub coding: Option<Vec<u8>>,     // 0600
}

/// Decode a VW ASCII identifier value: lossy UTF-8, then strip trailing NUL
/// padding, spaces, and control bytes. Interior spaces are kept — VW part
/// numbers such as `8V0 906 264 H` carry meaningful spaces.
fn trim_ascii(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(|c: char| c == ' ' || c.is_control())
        .to_string()
}

/// Read one ASCII-valued DID, mapping any failure (negative response, timeout,
/// malformed reply) to `None`.
async fn read_ascii<T: AsyncIsoTpTransport>(
    uds: &mut AsyncUdsClient<T>,
    did: u16,
) -> Option<String> {
    match uds.read_data_by_identifier(did).await {
        Ok(bytes) => Some(trim_ascii(&bytes)),
        Err(_) => None,
    }
}

/// Read the identification DID set from one ECU. Each DID is read
/// independently: a DID the ECU does not support (any `UdsError`) becomes
/// `None`, never aborts the whole read. Never issues a write.
pub async fn read_identity<T: AsyncIsoTpTransport>(uds: &mut AsyncUdsClient<T>) -> EcuIdentity {
    EcuIdentity {
        vin: read_ascii(uds, did::VIN).await,
        part_number: read_ascii(uds, did::PART_NUMBER).await,
        hw_number: read_ascii(uds, did::HW_NUMBER).await,
        sw_version: read_ascii(uds, did::SW_VERSION).await,
        component: read_ascii(uds, did::COMPONENT).await,
        serial: read_ascii(uds, did::SERIAL).await,
        // Coding is raw bytes (VW coding), kept verbatim for hex rendering.
        coding: uds.read_data_by_identifier(did::CODING).await.ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vag_transport::MockAsyncTransport;

    /// RDBI request PDU for `did`: `0x22 <hi> <lo>`.
    fn req(did: u16) -> Vec<u8> {
        vec![0x22, (did >> 8) as u8, (did & 0xFF) as u8]
    }

    /// Positive RDBI response PDU: `0x62 <hi> <lo> <data…>`.
    fn resp(did: u16, data: &[u8]) -> Vec<u8> {
        let mut v = vec![0x62, (did >> 8) as u8, (did & 0xFF) as u8];
        v.extend_from_slice(data);
        v
    }

    /// The DID order `read_identity` emits — the script must match it exactly.
    const ORDER: [u16; 7] = [
        did::VIN,
        did::PART_NUMBER,
        did::HW_NUMBER,
        did::SW_VERSION,
        did::COMPONENT,
        did::SERIAL,
        did::CODING,
    ];

    #[tokio::test]
    async fn golden_identity_from_real_autoscan_values() {
        // Owner's real Škoda Octavia Auto-Scan values (engine, address 01).
        // NOTE: F187 is modelled here WITHOUT interior spaces (`8V0906264H`);
        // whether the live wire pads it with spaces is an assumption to confirm
        // against a real read (see the reader's module docs).
        let vin = b"XW8AD4NE9JH008917";
        let part = b"8V0906264H";
        let hw = b"8V0906264";
        let sw = b"0004";
        let component = b"R4 1.8l TFSI    "; // trailing pad → trimmed
        let serial = b"VWZZZ7Z0K1234567";
        let coding = [0x01, 0x2A, 0x00, 0x04];

        let script = vec![
            (req(ORDER[0]), resp(ORDER[0], vin)),
            (req(ORDER[1]), resp(ORDER[1], part)),
            (req(ORDER[2]), resp(ORDER[2], hw)),
            (req(ORDER[3]), resp(ORDER[3], sw)),
            (req(ORDER[4]), resp(ORDER[4], component)),
            (req(ORDER[5]), resp(ORDER[5], serial)),
            (req(ORDER[6]), resp(ORDER[6], &coding)),
        ];
        let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

        let id = read_identity(&mut uds).await;

        assert_eq!(id.vin.as_deref(), Some("XW8AD4NE9JH008917"));
        assert_eq!(id.vin.as_deref().map(str::len), Some(17));
        assert_eq!(id.part_number.as_deref(), Some("8V0906264H"));
        assert_eq!(id.hw_number.as_deref(), Some("8V0906264"));
        assert_eq!(id.sw_version.as_deref(), Some("0004"));
        assert_eq!(id.component.as_deref(), Some("R4 1.8l TFSI"));
        assert_eq!(id.serial.as_deref(), Some("VWZZZ7Z0K1234567"));
        assert_eq!(id.coding.as_deref(), Some([0x01, 0x2A, 0x00, 0x04].as_slice()));
        assert!(uds.into_transport().is_exhausted(), "whole script consumed");
    }

    #[tokio::test]
    async fn unsupported_did_becomes_none_and_rest_still_populate() {
        // The HW-number DID answers with a negative response (requestOutOfRange);
        // every other DID succeeds. Only `hw_number` must end up `None`.
        let vin = b"XW8AD4NE9JH008917";
        let script = vec![
            (req(did::VIN), resp(did::VIN, vin)),
            (req(did::PART_NUMBER), resp(did::PART_NUMBER, b"8V0906264H")),
            // Negative response: [0x7F, sid, nrc=0x31 requestOutOfRange].
            (req(did::HW_NUMBER), vec![0x7F, 0x22, 0x31]),
            (req(did::SW_VERSION), resp(did::SW_VERSION, b"0004")),
            (req(did::COMPONENT), resp(did::COMPONENT, b"R4 1.8l TFSI")),
            (req(did::SERIAL), resp(did::SERIAL, b"VWZZZ7Z0K1234567")),
            (req(did::CODING), resp(did::CODING, &[0x01, 0x2A])),
        ];
        let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

        let id = read_identity(&mut uds).await;

        assert_eq!(id.hw_number, None, "unsupported DID → None");
        assert_eq!(id.vin.as_deref(), Some("XW8AD4NE9JH008917"));
        assert_eq!(id.sw_version.as_deref(), Some("0004"));
        assert_eq!(id.coding.as_deref(), Some([0x01, 0x2A].as_slice()));
    }

    #[tokio::test]
    async fn coding_did_failure_leaves_coding_none() {
        // Only the coding DID (last) fails via a transport timeout (empty script
        // slot → recv times out). Everything ASCII still populates.
        let script = vec![
            (req(did::VIN), resp(did::VIN, b"XW8AD4NE9JH008917")),
            (req(did::PART_NUMBER), resp(did::PART_NUMBER, b"8V0906264H")),
            (req(did::HW_NUMBER), resp(did::HW_NUMBER, b"8V0906264")),
            (req(did::SW_VERSION), resp(did::SW_VERSION, b"0004")),
            (req(did::COMPONENT), resp(did::COMPONENT, b"R4 1.8l TFSI")),
            (req(did::SERIAL), resp(did::SERIAL, b"VWZZZ7Z0K1234567")),
            // Coding DID: an empty response PDU → Malformed → None.
            (req(did::CODING), vec![]),
        ];
        let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

        let id = read_identity(&mut uds).await;

        assert_eq!(id.coding, None);
        assert_eq!(id.serial.as_deref(), Some("VWZZZ7Z0K1234567"));
    }

    #[test]
    fn trim_strips_trailing_pad_but_keeps_interior_spaces() {
        // Trailing NUL + spaces gone; interior spaces preserved.
        assert_eq!(trim_ascii(b"8V0 906 264 H\0\0  "), "8V0 906 264 H");
        assert_eq!(trim_ascii(b"XW8AD4NE9JH008917"), "XW8AD4NE9JH008917");
        // Pure padding trims to empty, not a panic.
        assert_eq!(trim_ascii(b"\0\0\0"), "");
        assert_eq!(trim_ascii(b""), "");
    }
}
