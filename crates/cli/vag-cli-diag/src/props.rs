//! What an ECU will tell you about itself.
//!
//! `vagcan info` prints a fixed passport. This module answers the broader
//! question — *everything* the control unit exposes in its identification
//! range — by sweeping it and naming what comes back.
//!
//! The names are the standardised UDS / VW identifiers. Anything not
//! confidently identified is printed raw rather than given an invented label:
//! a wrong name is worse than no name, because it gets believed.

/// Identification identifiers, named where the meaning is documented.
///
/// Verified present on the reference car (Škoda Octavia III 1.8 TFSI, MQB) —
/// engine `8V0906264H` and gearbox `0CW300041G` both answer most of these.
const KNOWN: &[(u16, &str)] = &[
	(0xF186, "Active diagnostic session"),
	(0xF187, "VW spare part number"),
	(0xF189, "VW software version"),
	(0xF18A, "System supplier"),
	(0xF18C, "ECU serial number"),
	(0xF190, "VIN"),
	(0xF191, "VW hardware number"),
	(0xF192, "Supplier hardware number"),
	(0xF193, "Supplier hardware version"),
	(0xF194, "Supplier software number"),
	(0xF195, "Supplier software version"),
	(0xF197, "System name / engine type"),
	(0xF19E, "ODX label file"),
	(0xF1A2, "ODX file version"),
	(0xF1DF, "Programming state"),
];

/// The identification range worth sweeping: VW puts everything in `F1xx`.
pub const IDENT_RANGE: &str = "F100-F1FF";

/// The documented name of an identifier, if it has one.
pub fn name_of(did: u16) -> Option<&'static str> {
	KNOWN.iter().find(|(d, _)| *d == did).map(|(_, n)| *n)
}

/// One property read off the ECU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
	pub did: u16,
	pub data: Vec<u8>,
}

impl Property {
	/// The value as text, when the bytes are printable ASCII.
	///
	/// VW pads these fields with trailing spaces and NULs (`"8V0906264H "` is
	/// literally what the engine returns), so both are trimmed. Short values
	/// are not treated as text: two bytes of a measurement are printable by
	/// coincidence often enough to mislead.
	pub fn text(&self) -> Option<String> {
		let trimmed: &[u8] = match self.data.iter().position(|&b| b == 0) {
			Some(nul) => &self.data[..nul],
			None => &self.data,
		};
		if trimmed.len() < 3 || !trimmed.iter().all(|&b| (0x20..0x7F).contains(&b)) {
			return None;
		}
		let text = String::from_utf8_lossy(trimmed).trim_end().to_string();
		(!text.is_empty()).then_some(text)
	}

	pub fn hex(&self) -> String {
		crate::render::hex_spaced(&self.data)
	}
}

/// Render the properties of one ECU as a report.
///
/// Named identifiers come first in the documented order so the interesting
/// facts are at the top; the rest follow numerically as raw rows, still worth
/// showing because that is where the undocumented content lives.
pub fn render(ecu_label: &str, props: &[Property]) -> String {
	let mut out = format!("{ecu_label}\n\n");
	if props.is_empty() {
		out.push_str("  (no identifiers answered)\n");
		return out;
	}

	let width = KNOWN.iter().map(|(_, n)| n.len()).max().unwrap_or(0);
	let mut named = Vec::new();
	let mut rest = Vec::new();
	for p in props {
		match name_of(p.did) {
			Some(name) => named.push((name, p)),
			None => rest.push(p),
		}
	}
	named.sort_by_key(|(name, _)| KNOWN.iter().position(|(_, n)| n == name).unwrap_or(usize::MAX));

	for (name, p) in named {
		let value = p.text().unwrap_or_else(|| p.hex());
		out.push_str(&format!("  {name:<width$}  {value}\n"));
	}
	if !rest.is_empty() {
		out.push_str("\n  Undocumented identifiers:\n");
		for p in rest {
			let value = match p.text() {
				Some(t) => format!("{:<24}  \"{t}\"", p.hex_short()),
				None => p.hex_short(),
			};
			out.push_str(&format!("    {:04X}  {value}\n", p.did));
		}
	}
	out
}

impl Property {
	/// Hex, cut for the terminal when the value is long.
	fn hex_short(&self) -> String {
		if self.data.len() <= 12 {
			return self.hex();
		}
		format!("{} … ({} bytes)", crate::render::hex_spaced(&self.data[..12]), self.data.len())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn prop(did: u16, data: &[u8]) -> Property {
		Property { did, data: data.to_vec() }
	}

	#[test]
	fn vw_pads_its_text_fields_and_the_padding_is_trimmed() {
		// Exactly what the reference car returns for F187 / F19E.
		assert_eq!(prop(0xF187, b"8V0906264H ").text().as_deref(), Some("8V0906264H"));
		assert_eq!(
			prop(0xF19E, b"EV_ECM18TFS0208V0906264H\0").text().as_deref(),
			Some("EV_ECM18TFS0208V0906264H")
		);
		assert_eq!(prop(0xF197, b"1.8l R4 TFSI ").text().as_deref(), Some("1.8l R4 TFSI"));
	}

	#[test]
	fn binary_and_tiny_values_are_not_passed_off_as_text() {
		assert_eq!(prop(0xF19A, &[0x00, 0x01, 0xFE, 0x2D]).text(), None);
		// Printable, but two bytes of binary read as text by coincidence.
		assert_eq!(prop(0xA058, &[0x55, 0x55]).text(), None);
		assert_eq!(prop(0xF1DF, &[0x40]).text(), None);
	}

	#[test]
	fn bytes_that_are_not_text_are_printed_a_byte_at_a_time() {
		// A person reads these against a datasheet or a VCDS screen, both of
		// which group by byte. Whatever renders them has to keep the gaps.
		let p = prop(0xF19A, &[0x00, 0x01, 0xFE, 0x2D]);
		assert_eq!(p.hex(), "00 01 FE 2D");
		// Long ones are cut, and the part that is shown is still spaced.
		let long = prop(0xF1AB, &(0..20u8).collect::<Vec<_>>());
		assert_eq!(long.hex_short(), "00 01 02 03 04 05 06 07 08 09 0A 0B … (20 bytes)");
	}

	#[test]
	fn the_report_names_what_is_documented_and_shows_the_rest_raw() {
		let text = render(
			"Engine (01)",
			&[
				prop(0xF1AB, b"00230076007600760318"),
				prop(0xF190, b"XW8AD4NE9JH008917"),
				prop(0xF19E, b"EV_ECM18TFS0208V0906264H\0"),
			],
		);
		// Named rows carry the name, not the number.
		assert!(text.contains("VIN "), "{text}");
		assert!(text.contains("XW8AD4NE9JH008917"), "{text}");
		assert!(text.contains("ODX label file"), "{text}");
		// The label file is the join key to the .rod label files — it must be legible.
		assert!(text.contains("EV_ECM18TFS0208V0906264H"), "{text}");
		// Unknown identifiers are shown, but under an honest heading.
		assert!(text.contains("Undocumented identifiers:"), "{text}");
		assert!(text.contains("F1AB"), "{text}");
		// VIN is documented, so it must NOT appear in the undocumented block.
		let (_, undocumented) = text.split_once("Undocumented").unwrap();
		assert!(!undocumented.contains("F190"), "{text}");
	}

	#[test]
	fn named_rows_follow_the_documented_order_not_the_read_order() {
		let text = render("Engine (01)", &[prop(0xF19E, b"EV_ECM1\0"), prop(0xF187, b"8V0906264H ")]);
		let part = text.find("VW spare part number").unwrap();
		let odx = text.find("ODX label file").unwrap();
		assert!(part < odx, "part number is listed before the ODX name: {text}");
	}

	#[test]
	fn an_ecu_that_answers_nothing_says_so() {
		assert!(render("Gearbox (02)", &[]).contains("no identifiers answered"));
	}
}
