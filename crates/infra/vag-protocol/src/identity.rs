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
//!
//! The identifier set and the padding behaviour were verified against the
//! reference car on 2026-08-01 (see the golden tests): VW pads its text fields
//! with a trailing space or NUL, part numbers carry no interior spaces, and the
//! engine does not implement the serial-number identifier at all.

use vag_transport::AsyncIsoTpTransport;

use crate::AsyncUdsClient;

/// Data identifiers we read for the identification block. ASCII-valued unless
/// noted. Values mirror the VW/UDS standard identifiers VCDS surfaces.
///
/// Public because a caller that wants one field has no business paying for
/// seven reads to get it, and because the alternative — each caller writing
/// `0xF190` again — is how the same number ends up spelled three ways.
pub mod did {
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

impl EcuIdentity {
	/// True when the control unit answered nothing at all — every field is
	/// absent. Distinguishes "this ECU is not present / not reachable" from
	/// "present, but sparse", which matters for what the CLI should print.
	pub fn is_empty(&self) -> bool {
		self.vin.is_none()
			&& self.part_number.is_none()
			&& self.hw_number.is_none()
			&& self.sw_version.is_none()
			&& self.component.is_none()
			&& self.serial.is_none()
			&& self.coding.is_none()
	}
}

/// Read one ASCII-valued DID, mapping any failure (negative response, timeout,
/// malformed reply) to `None`.
async fn read_ascii<T: AsyncIsoTpTransport>(uds: &mut AsyncUdsClient<T>, did: u16) -> Option<String> {
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
	async fn golden_engine_identity_is_the_bytes_the_car_really_sends() {
		// Captured off the reference car (Škoda Octavia III 1.8 TFSI, engine at
		// address 01) on 2026-08-01 — these are the wire bytes, not a model of
		// them. Note what the car actually does: F187 and F191 carry ONE
		// trailing space and no interior spaces, F197 is "1.8l R4 TFSI" in that
		// word order, and the engine does not implement F18C at all.
		let vin = b"XW8AD4NE9JH008917";
		let part = b"8V0906264H ";
		let hw = b"06K907425B ";
		let sw = b"0005";
		let component = b"1.8l R4 TFSI ";
		let coding = [0x0c, 0x25, 0x00, 0x12, 0x23, 0x24, 0x04, 0x0b, 0x00, 0x00];

		let script = vec![
			(req(ORDER[0]), resp(ORDER[0], vin)),
			(req(ORDER[1]), resp(ORDER[1], part)),
			(req(ORDER[2]), resp(ORDER[2], hw)),
			(req(ORDER[3]), resp(ORDER[3], sw)),
			(req(ORDER[4]), resp(ORDER[4], component)),
			// The engine refuses the serial-number identifier.
			(req(ORDER[5]), vec![0x7F, 0x22, 0x31]),
			(req(ORDER[6]), resp(ORDER[6], &coding)),
		];
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

		let id = read_identity(&mut uds).await;

		assert_eq!(id.vin.as_deref(), Some("XW8AD4NE9JH008917"));
		assert_eq!(id.vin.as_deref().map(str::len), Some(17));
		assert_eq!(id.part_number.as_deref(), Some("8V0906264H"), "trailing pad trimmed");
		assert_eq!(id.hw_number.as_deref(), Some("06K907425B"));
		assert_eq!(id.sw_version.as_deref(), Some("0005"));
		assert_eq!(id.component.as_deref(), Some("1.8l R4 TFSI"));
		assert_eq!(id.serial, None, "the engine does not implement F18C");
		assert_eq!(id.coding.as_deref(), Some(coding.as_slice()));
		assert!(!id.is_empty());
		assert!(uds.into_transport().is_exhausted(), "whole script consumed");
	}

	#[tokio::test]
	async fn golden_gearbox_identity_is_the_bytes_the_car_really_sends() {
		// Same session, gearbox at address 02 (DQ200). Unlike the engine it
		// does answer F18C, and pads it with a NUL rather than a space — both
		// paddings have to trim.
		let script = vec![
			(req(ORDER[0]), resp(ORDER[0], b"XW8AD4NE9JH008917")),
			(req(ORDER[1]), resp(ORDER[1], b"0CW300041G ")),
			(req(ORDER[2]), resp(ORDER[2], b"0AM927769E ")),
			(req(ORDER[3]), resp(ORDER[3], b"1003")),
			(req(ORDER[4]), resp(ORDER[4], b"GSG DQ200G2_M")),
			(req(ORDER[5]), resp(ORDER[5], b"CU501702277773\0")),
			(req(ORDER[6]), resp(ORDER[6], &[0x00, 0x14])),
		];
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

		let id = read_identity(&mut uds).await;

		assert_eq!(id.part_number.as_deref(), Some("0CW300041G"));
		assert_eq!(id.hw_number.as_deref(), Some("0AM927769E"));
		assert_eq!(id.sw_version.as_deref(), Some("1003"));
		assert_eq!(id.component.as_deref(), Some("GSG DQ200G2_M"), "no padding on this one");
		assert_eq!(id.serial.as_deref(), Some("CU501702277773"), "NUL padding trimmed");
		assert_eq!(id.coding.as_deref(), Some([0x00, 0x14].as_slice()));
	}

	#[tokio::test]
	async fn an_ecu_that_answers_nothing_is_reported_as_empty() {
		// Every identifier refused — the shape of reading an address the car
		// does not have. Must be distinguishable from a sparse answer.
		let script: Vec<(Vec<u8>, Vec<u8>)> = ORDER.iter().map(|d| (req(*d), vec![0x7F, 0x22, 0x31])).collect();
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

		let id = read_identity(&mut uds).await;

		assert!(id.is_empty(), "nothing answered → empty");
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
			(req(did::SW_VERSION), resp(did::SW_VERSION, b"0005")),
			(req(did::COMPONENT), resp(did::COMPONENT, b"R4 1.8l TFSI")),
			(req(did::SERIAL), resp(did::SERIAL, b"CU501702277773")),
			(req(did::CODING), resp(did::CODING, &[0x01, 0x2A])),
		];
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

		let id = read_identity(&mut uds).await;

		assert_eq!(id.hw_number, None, "unsupported DID → None");
		assert_eq!(id.vin.as_deref(), Some("XW8AD4NE9JH008917"));
		assert_eq!(id.sw_version.as_deref(), Some("0005"));
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
			(req(did::SW_VERSION), resp(did::SW_VERSION, b"0005")),
			(req(did::COMPONENT), resp(did::COMPONENT, b"R4 1.8l TFSI")),
			(req(did::SERIAL), resp(did::SERIAL, b"CU501702277773")),
			// Coding DID: an empty response PDU → Malformed → None.
			(req(did::CODING), vec![]),
		];
		let mut uds = AsyncUdsClient::new(MockAsyncTransport::new(script));

		let id = read_identity(&mut uds).await;

		assert_eq!(id.coding, None);
		assert_eq!(id.serial.as_deref(), Some("CU501702277773"));
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
