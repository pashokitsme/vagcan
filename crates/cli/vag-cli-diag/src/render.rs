//! Pure rendering of what the car commands print — kept free of I/O and hardware so
//! the output formatting is unit-tested with captured identify bytes.
//!
//! Each renderer returns the data (stdout) apart from its notes (stderr, at a level):
//! [`vag_cli_core::verbosity`] has the split.

use vag_cli_core::marks::{SEP, facts};
use vag_cli_core::table::{Table, section};
use vag_data_labels::obd::Unconverted;
use vag_uds_client::{EcuIdentity, Reading};

/// One unit of the `vagcan info` card: which section it is, how it is addressed, and what
/// it answered.
pub struct InfoUnit<'a> {
	/// The section's title: `ENGINE`, `GEARBOX`.
	pub title: &'a str,
	/// Its short number, `01`.
	pub label: String,
	/// Its request id, `7E0`.
	pub request: u16,
	pub identity: &'a EcuIdentity,
	/// The ODX file it names for itself (F19E).
	pub odx: Option<String>,
}

/// The `vagcan info` card, `gh pr view`'s shape: the car on the first line, a meta line of
/// facts, then a section per unit with what it said indented under it. A unit that answered
/// nothing says so in its section.
///
/// ```text
/// XW8AD4NE9JH008917 · SK37
/// 15 control units · read 2026-10-21 14:02
///
/// ENGINE  01 · 7E0
///   1.8l R4 TFSI
///   part 8V0906264H · software 0008 · EV_ECM18TFS0208V0906264H
/// ```
pub fn render_info(vin: Option<&str>, project: Option<&str>, meta: &str, units: &[InfoUnit<'_>]) -> String {
	let mut out = facts([vin.unwrap_or("VIN not read"), project.unwrap_or_default()]);
	out.push('\n');
	if !meta.is_empty() {
		out.push_str(meta);
		out.push('\n');
	}
	for unit in units {
		out.push('\n');
		let title = format!("{}  {}", unit.title, facts([unit.label.clone(), format!("{:03X}", unit.request)]));
		out.push_str(&section(&title, info_lines(unit)));
	}
	out
}

/// What one unit said, as the lines of its section: its own name, then its numbers on one
/// line, then its serial and coding on another; `no answer` when it said nothing.
fn info_lines(unit: &InfoUnit<'_>) -> Vec<String> {
	let id = unit.identity;
	if id.is_empty() && unit.odx.is_none() {
		return vec!["no answer".to_string()];
	}
	let tagged = |tag: &str, value: &Option<String>| value.as_ref().map(|v| format!("{tag} {v}")).unwrap_or_default();
	let mut lines = Vec::new();
	if let Some(component) = &id.component {
		lines.push(component.clone());
	}
	lines.push(facts([
		tagged("part", &id.part_number),
		tagged("hardware", &id.hw_number),
		tagged("software", &id.sw_version),
		unit.odx.clone().unwrap_or_default(),
	]));
	lines.push(facts([
		tagged("serial", &id.serial),
		tagged("coding", &id.coding.as_deref().map(hex_spaced)),
	]));
	lines.retain(|line| !line.is_empty());
	lines
}

/// What is said when a control unit answers nothing at all, on stderr: what failed and
/// what to check at level 0, in the order somebody at a car can work through it; why a
/// quiet bus is not itself a fault at `-v`.
pub fn nothing_answered() -> Vec<(u8, String)> {
	vec![
		(0, "no answer from the car".to_string()),
		(
			0,
			format!(
				"check: {}",
				vag_cli_core::marks::facts([
					"ignition on",
					"OBD-II pin 6 CAN-H, pin 14 CAN-L, pin 5 GND",
					"adapter termination off",
					"`vagcan devices`"
				])
			),
		),
		(
			1,
			"a quiet bus is normal here: this platform's diagnostic line carries almost nothing until it is asked".to_string(),
		),
	]
}

/// [`nothing_answered`], said.
pub fn say_nothing_answered() {
	for (level, line) in nothing_answered() {
		vag_cli_core::verbosity::say(level, &line);
	}
}

/// What is said, on stderr, when a unit answered every identifier asked of it with a
/// refusal: it is on the bus and talking, so nothing about the wiring is in question — it
/// serves none of them.
pub fn refused_all(unit_label: &str, asked: usize, what: &str) -> Vec<(u8, String)> {
	vec![
		(0, format!("unit {unit_label} refused all {asked} {what}")),
		(
			1,
			"it answered each one with a negative response: the unit is there, and serves none of them".to_string(),
		),
	]
}

/// What is said, on stderr, when a unit gave nothing back to the first requests of a read
/// and the rest were not asked ([`crate::anomaly::Monitor::absent`]).
pub fn absent(unit_label: &str, silences: usize) -> Vec<(u8, String)> {
	vec![
		(0, format!("unit {unit_label} does not answer")),
		(0, "  (use \"vagcan units --identify\" to see which units answer)".to_string()),
		(1, format!("its first {silences} requests went unanswered, so the rest were not asked")),
	]
}

/// Lines with their levels, said.
pub fn say_all(lines: &[(u8, String)]) {
	for (level, line) in lines {
		vag_cli_core::verbosity::say(*level, line);
	}
}

/// The noun for a count: `1 change`, `2 changes`.
///
/// Only regular plurals, because every count in this tool is of something with
/// one — identifiers, columns, changes, control units.
pub fn plural(n: usize, singular: &str) -> String {
	vag_cli_core::table::plural(n, singular)
}

/// Bytes for a person to read: `0B 34`.
///
/// Everything shown on a screen uses this. The gap is what makes a byte
/// countable against a datasheet or a VCDS window, which is what anybody
/// looking at raw bytes is doing.
pub fn hex_spaced(bytes: &[u8]) -> String {
	bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// A reading's value and unit cells: decimals dropped for a unitless whole number (a count
/// of warm-ups is not a measurement to two places), the bytes and `raw` for an answer that
/// did not fit the form — no unit on a value that was never formed.
///
/// Whether a row has a unit is a property of the measurement, not of the value it happens
/// to carry, so the unit cell is decided by the definition.
pub fn sensor_cells(reading: &Reading) -> (String, String) {
	let Some(value) = reading.value else {
		return (hex_spaced(&reading.raw), "raw".to_string());
	};
	if reading.unit.is_empty() {
		let text = match value.fract() == 0.0 {
			true => format!("{value:.0}"),
			false => format!("{value:.2}"),
		};
		return (text, String::new());
	}
	(format!("{value:.2}"), reading.unit.clone())
}

/// One line of `vagcan sensors`: either a reading the standard conversion is
/// established for, or the bytes a unit answered with and the reason the tool
/// will not turn them into a number.
pub enum SensorLine {
	/// The conversion holds here (see [`vag_data_labels::obd::conversion_for`]); `did` is
	/// the identifier it was read from.
	Converted { did: u16, reading: Reading },
	/// It does not, so only the identifier and its bytes are shown. Nothing is
	/// named: the J1979 name would assert the very thing that is in doubt.
	Unconverted { did: u16, bytes: Vec<u8>, why: Unconverted },
}

/// The whole `vagcan sensors` report: the data, and the notes for `-v`.
///
/// A list, `gh pr list`'s shape: a summary line — how many of the `asked` parameters the
/// unit answered, how many of those were converted — then one table, converted rows first:
/// `DID`, `VALUE` (the physical unit in it: `40.00 °C` — `unit` is the control unit
/// everywhere in this tool), `NAME`. An identifier whose bytes the tool refuses to interpret
/// is a row too, its bytes and `raw` as the value and the reason in short where the name
/// would be — the J1979 name would assert the very thing in doubt. Why such rows are not
/// converted, in words, is a note.
pub fn render_sensors(unit_label: &str, asked: usize, lines: &[SensorLine]) -> (String, Vec<String>) {
	let mut table = Table::new(&["DID", "VALUE", "NAME"]).right(1);
	// Every unit padded to the widest, `raw` included, so the numbers right-align on their
	// last digit whatever unit follows them: `40.00 °C  ` over `1000.00 /min`.
	let unit_width = lines
		.iter()
		.map(|l| match l {
			SensorLine::Converted { reading, .. } => sensor_cells(reading).1.chars().count(),
			SensorLine::Unconverted { .. } => "raw".len(),
		})
		.max()
		.unwrap_or(0);
	let with_unit = |value: String, unit: &str| match unit_width {
		0 => value,
		w => format!("{value} {unit:<w$}"),
	};
	let mut converted = 0usize;
	for line in lines {
		if let SensorLine::Converted { did, reading } = line {
			converted += 1;
			let (value, unit) = sensor_cells(reading);
			table.row(vec![format!("{did:04X}"), with_unit(value, &unit), reading.name.clone()]);
		}
	}
	let mut notes = Vec::new();
	let mut off_block = false;
	for line in lines {
		let SensorLine::Unconverted { did, bytes, why } = line else { continue };
		let reason = match why {
			Unconverted::NotAnEmissionsUnit => {
				off_block = true;
				"not J1979: unit outside 7E0–7E7".to_string()
			}
			Unconverted::WrongWidth { expected, got } => format!("not J1979: {got} {}, standard {expected}", plural(*got, "byte")),
		};
		table.row(vec![format!("{did:04X}"), with_unit(hex_spaced(bytes), "raw"), reason]);
	}
	if off_block {
		notes.push(format!(
			"unit {unit_label} is not one ISO 15765-4 addresses for emissions diagnostics (7E0–7E7): nothing establishes \
			 that its F4xx identifiers are the SAE J1979 parameters, and on the reference car they are not — bytes only"
		));
	}
	if lines.iter().any(|l| {
		matches!(
			l,
			SensorLine::Unconverted {
				why: Unconverted::WrongWidth { .. },
				..
			}
		)
	}) {
		notes.push("a row answered at another width than SAE J1979 defines is not that parameter — bytes only".to_string());
	}
	let summary = format!(
		"{} of {asked} standard parameters answered by unit {unit_label}{SEP}{converted} converted",
		lines.len()
	);
	(vag_cli_core::table::list(&summary, &table, vag_cli_core::table::terminal_width()), notes)
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
		let units = [
			InfoUnit {
				title: "ENGINE",
				label: "01".into(),
				request: 0x7E0,
				identity: &engine,
				odx: Some("EV_ECM18TFS0208V0906264H".into()),
			},
			InfoUnit {
				title: "GEARBOX",
				label: "02".into(),
				request: 0x7E1,
				identity: &gearbox,
				odx: None,
			},
		];
		let text = render_info(
			Some("XW8AD4NE9JH008917"),
			Some("SK37"),
			"15 control units · read 2026-10-21 14:02",
			&units,
		);
		assert_eq!(
			text,
			"XW8AD4NE9JH008917 · SK37\n\
			 15 control units · read 2026-10-21 14:02\n\
			 \n\
			 ENGINE  01 · 7E0\n\
			 \x20 1.8l R4 TFSI\n\
			 \x20 part 8V0906264H · hardware 06K907425B · software 0005 · EV_ECM18TFS0208V0906264H\n\
			 \x20 coding 0C 25\n\
			 \n\
			 GEARBOX  02 · 7E1\n\
			 \x20 GSG DQ200G2_M\n\
			 \x20 part 0CW300041G · hardware 06K907425B · software 0005\n\
			 \x20 serial CU501702277773 · coding 0C 25\n"
		);
	}

	#[test]
	fn a_unit_that_answered_nothing_says_so_in_its_section() {
		let engine = identity("8V0906264H");
		let none = EcuIdentity::default();
		let units = [
			InfoUnit {
				title: "ENGINE",
				label: "01".into(),
				request: 0x7E0,
				identity: &engine,
				odx: None,
			},
			InfoUnit {
				title: "GEARBOX",
				label: "02".into(),
				request: 0x7E1,
				identity: &none,
				odx: None,
			},
		];
		let text = render_info(None, None, "read 2026-10-21 14:02", &units);
		assert!(text.starts_with("VIN not read\nread 2026-10-21 14:02\n\n"), "{text}");
		assert!(text.ends_with("GEARBOX  02 · 7E1\n  no answer\n"), "{text}");
		assert!(!text.contains('—'), "no column of dashes: {text}");
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
	fn a_count_prints_as_an_integer_with_no_unit() {
		// PID 30 on the reference engine: a count, no unit. It used to print
		// `4.00 ` — two decimals it cannot have, and a trailing space where a
		// unit would have been.
		assert_eq!(
			sensor_cells(&reading("Warm-ups since codes cleared", "", Some(4.0))),
			("4".into(), String::new())
		);
		// Unitless and NOT whole: decimals kept because they are real.
		assert_eq!(sensor_cells(&reading("Something", "", Some(4.5))), ("4.50".into(), String::new()));
	}

	#[test]
	fn a_measurement_keeps_its_unit_and_its_decimals() {
		assert_eq!(
			sensor_cells(&reading("Coolant temperature", "°C", Some(74.0))),
			("74.00".into(), "°C".into())
		);
	}

	#[test]
	fn bytes_that_did_not_fit_the_form_are_shown_as_bytes() {
		let mut r = reading("Odd one", "km/h", None);
		r.raw = vec![0xAB, 0xCD];
		// No unit on a value that was never formed — it would claim the bytes
		// had been understood.
		assert_eq!(sensor_cells(&r), ("AB CD".into(), "raw".into()));
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
		let (text, notes) = render_sensors("746", 32, &lines);
		assert_eq!(
			text,
			"2 of 32 standard parameters answered by unit 746 · 0 converted\n\
			 \n\
			 DID    VALUE  NAME\n\
			 F405  57 raw  not J1979: unit outside 7E0–7E7\n\
			 F40C  00 raw  not J1979: unit outside 7E0–7E7\n"
		);
		// The reason in words is a note, not a line of the data.
		assert!(notes.iter().any(|n| n.contains("ISO 15765-4")), "{notes:?}");
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
			SensorLine::Converted {
				did: 0xF405,
				reading: reading("Coolant temperature", "°C", Some(89.0)),
			},
			SensorLine::Unconverted {
				did: 0xF40D,
				bytes: vec![0x9C, 0x02],
				why: Unconverted::WrongWidth { expected: 1, got: 2 },
			},
		];
		let (text, notes) = render_sensors("02", 32, &lines);
		assert_eq!(
			text,
			"2 of 32 standard parameters answered by unit 02 · 1 converted\n\
			 \n\
			 DID       VALUE  NAME\n\
			 F405  89.00 °C   Coolant temperature\n\
			 F40D  9C 02 raw  not J1979: 2 bytes, standard 1\n"
		);
		assert_eq!(notes.len(), 1, "{notes:?}");
		// The first byte read as km/h would be 156. Never printed.
		assert!(!text.contains("156"), "{text}");
	}

	#[test]
	fn the_silence_message_names_the_checks_in_order() {
		let said = nothing_answered();
		let quiet: Vec<&str> = said.iter().filter(|(l, _)| *l == 0).map(|(_, s)| s.as_str()).collect();
		let text = quiet.join("\n");
		assert!(text.contains("ignition"), "{text}");
		assert!(text.contains("pin 6"), "{text}");
		assert!(text.contains("vagcan devices"), "{text}");
		let ignition = text.find("ignition").unwrap();
		assert!(ignition < text.find("pin 6").unwrap() && ignition < text.find("devices").unwrap());
		// Silence is not proof of a fault on this platform — said at -v.
		assert!(said.iter().any(|(l, s)| *l == 1 && s.contains("quiet bus is normal")), "{said:?}");
	}
}
