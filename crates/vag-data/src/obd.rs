//! Standard OBD-II sensors, read through their UDS mirrors.
//!
//! A VAG control unit exposes the legislated OBD-II parameters at
//! `0xF400 + PID`: reading data identifier `F405` returns what OBD-II mode 01
//! PID `05` would. The conversions are the public SAE J1979 ones, so this whole
//! family is decodable without reverse-engineering anything.
//!
//! **Five of these rows were measured on this car; the other 27 are transcribed
//! from SAE J1979.** The five were fitted blind by crossing a passive CAN
//! capture with a simultaneous VCDS log (`vagcan analyse`, 2026-08-01) — the
//! fitter is told nothing about J1979 — and every one landed exactly on the
//! standard's conversion, including a two-byte pressure with a ×10 factor:
//!
//! | DID | fitted from the car | J1979 |
//! |---|---|---|
//! | `F405` | `raw − 40` °C | `A − 40` |
//! | `F40D` | `raw` km/h | `A` |
//! | `F40F` | `raw − 40` °C | `A − 40` |
//! | `F423` | `raw × 10` kPa | `(256A+B) × 10` |
//! | `F446` | `raw − 40` °C | `A − 40` |
//!
//! Five blind fits landing on published formulas — one of them neither a
//! temperature nor a single byte, so not something a lucky guess reaches — is
//! powerful evidence that this unit implements the family faithfully. The
//! remaining rows rest on that inference rather than on a fit of their own: two
//! more (fuel tank level, barometric pressure) agreed with VCDS to display
//! precision on a live read, and the rest are unverified predictions — correct
//! if and only if the mirror is faithful beyond the five that were measured. A
//! test in this module pins the measured five, so the table cannot drift away
//! from the evidence that justifies trusting the rest.
//!
//! ## Where this table applies — and where it does not
//!
//! On the **emissions-related** control units ISO 15765-4 addresses
//! (`0x7E0..0x7E7`), and nowhere else. Units on VW's own `0x700..0x7BF` block
//! answer `F4xx` identifiers too, and they are not these parameters: the
//! reference car's climate unit (`0x746`) answers `F405` with 87 / 90 / 109 at
//! three moments when the engine's `F405` reads 129 / 93 / 137 — a line through
//! the first two pairs predicts −135 for the third — and answers `F40C` with
//! one byte where PID `0C` is two. Even inside the block the width has to be
//! checked: the gearbox at `0x7E1` answers `F40D` with two little-endian bytes
//! at ×0.01 km/h (its own catalog row) where PID `0D` is one byte of km/h.
//! [`conversion_for`] is the gate; applying this table without it prints
//! confident nonsense.
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
	ObdPid {
		pid: 0x04,
		name: "Calculated engine load",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 255.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x05,
		name: "Coolant temperature",
		unit: "°C",
		form: RawForm::U8First,
		factor: 1.0,
		offset: -40.0,
	},
	ObdPid {
		pid: 0x06,
		name: "Short term fuel trim, bank 1",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 128.0,
		offset: -100.0,
	},
	ObdPid {
		pid: 0x07,
		name: "Long term fuel trim, bank 1",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 128.0,
		offset: -100.0,
	},
	ObdPid {
		pid: 0x0B,
		name: "Intake manifold absolute pressure",
		unit: "kPa",
		form: RawForm::U8First,
		factor: 1.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x0C,
		name: "Engine speed",
		unit: "/min",
		form: RawForm::U16Be,
		factor: 0.25,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x0D,
		name: "Vehicle speed",
		unit: "km/h",
		form: RawForm::U8First,
		factor: 1.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x0E,
		name: "Timing advance",
		unit: "°",
		form: RawForm::U8First,
		factor: 0.5,
		offset: -64.0,
	},
	ObdPid {
		pid: 0x0F,
		name: "Intake air temperature",
		unit: "°C",
		form: RawForm::U8First,
		factor: 1.0,
		offset: -40.0,
	},
	ObdPid {
		pid: 0x10,
		name: "Mass air flow",
		unit: "g/s",
		form: RawForm::U16Be,
		factor: 0.01,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x11,
		name: "Throttle position",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 255.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x1F,
		name: "Run time since engine start",
		unit: "s",
		form: RawForm::U16Be,
		factor: 1.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x21,
		name: "Distance with warning lamp on",
		unit: "km",
		form: RawForm::U16Be,
		factor: 1.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x23,
		name: "Fuel rail gauge pressure",
		unit: "kPa",
		form: RawForm::U16Be,
		factor: 10.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x2E,
		name: "Commanded evaporative purge",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 255.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x2F,
		name: "Fuel tank level",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 255.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x30,
		name: "Warm-ups since codes cleared",
		unit: "",
		form: RawForm::U8First,
		factor: 1.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x31,
		name: "Distance since codes cleared",
		unit: "km",
		form: RawForm::U16Be,
		factor: 1.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x33,
		name: "Absolute barometric pressure",
		unit: "kPa",
		form: RawForm::U8First,
		factor: 1.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x3C,
		name: "Catalyst temperature, bank 1 sensor 1",
		unit: "°C",
		form: RawForm::U16Be,
		factor: 0.1,
		offset: -40.0,
	},
	ObdPid {
		pid: 0x3D,
		name: "Catalyst temperature, bank 2 sensor 1",
		unit: "°C",
		form: RawForm::U16Be,
		factor: 0.1,
		offset: -40.0,
	},
	ObdPid {
		pid: 0x3E,
		name: "Catalyst temperature, bank 1 sensor 2",
		unit: "°C",
		form: RawForm::U16Be,
		factor: 0.1,
		offset: -40.0,
	},
	ObdPid {
		pid: 0x42,
		name: "Control module voltage",
		unit: "V",
		form: RawForm::U16Be,
		factor: 0.001,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x43,
		name: "Absolute load",
		unit: "%",
		form: RawForm::U16Be,
		factor: 100.0 / 255.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x45,
		name: "Relative throttle position",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 255.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x46,
		name: "Ambient air temperature",
		unit: "°C",
		form: RawForm::U8First,
		factor: 1.0,
		offset: -40.0,
	},
	ObdPid {
		pid: 0x47,
		name: "Absolute throttle position B",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 255.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x49,
		name: "Accelerator pedal position D",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 255.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x4A,
		name: "Accelerator pedal position E",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 255.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x4C,
		name: "Commanded throttle actuator",
		unit: "%",
		form: RawForm::U8First,
		factor: 100.0 / 255.0,
		offset: 0.0,
	},
	ObdPid {
		pid: 0x5C,
		name: "Engine oil temperature",
		unit: "°C",
		form: RawForm::U8First,
		factor: 1.0,
		offset: -40.0,
	},
	ObdPid {
		pid: 0x5E,
		name: "Engine fuel rate",
		unit: "L/h",
		form: RawForm::U16Be,
		factor: 0.05,
		offset: 0.0,
	},
];

/// Mode 09 (vehicle information) mirrored at `0xF800 + PID`.
///
/// Confirmed against the reference engine's own bytes rather than assumed:
/// `F802` carries the VIN, `F804` a 16-character calibration identifier, and
/// `F80A` the string `ECM\0-EngineControl`. Each response opens with a count
/// of data items, then the items themselves — so the payload is not the value,
/// and reading it as one would prepend a stray byte.
///
/// These are worth having because they are **not** in the `F1xx`
/// identification block: the calibration identifier and its verification
/// number identify the exact emissions calibration, which a part number does
/// not.
pub const VEHICLE_INFO: &[(u8, &str)] = &[(0x02, "VIN"), (0x04, "Calibration ID"), (0x0A, "ECU name")];

/// The UDS data identifier a mode-09 PID is mirrored at.
pub const fn did_for_info_pid(pid: u8) -> u16 {
	0xF800 | pid as u16
}

/// Decode a mode-09 text response: a count byte, then that many fixed-width
/// items packed together.
///
/// Returns each item separately. NUL and space padding is trimmed — VW pads
/// both ways — and a response whose length is not a whole number of items is
/// rejected rather than split arbitrarily.
pub fn decode_info_text(data: &[u8]) -> Option<Vec<String>> {
	let (&count, items) = data.split_first()?;
	let count = count as usize;
	if count == 0 || items.is_empty() || items.len() % count != 0 {
		return None;
	}
	let width = items.len() / count;
	Some(
		items
			.chunks(width)
			.map(|item| String::from_utf8_lossy(item).trim_matches(|c: char| c == '\0' || c == ' ').to_string())
			.collect(),
	)
}

/// Look a parameter up by its PID.
pub fn pid(pid: u8) -> Option<&'static ObdPid> {
	PIDS.iter().find(|p| p.pid == pid)
}

/// Why a mirrored identifier's bytes were **not** converted.
///
/// A reading must never print as an engineering value when the conversion is
/// not known to hold; these are the two ways this project has established that
/// it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unconverted {
	/// The control unit is not one ISO 15765-4 addresses for emissions
	/// diagnostics, so nothing obliges its `F4xx` identifiers to be the J1979
	/// ones — and on the reference car they demonstrably are not (see
	/// `vag_protocol::address::UnitAddress::is_emissions_related`).
	NotAnEmissionsUnit,
	/// The unit *is* one, but the response is not the width the standard
	/// defines for this parameter. Whatever it answered, it is not this
	/// parameter: a two-byte answer to a one-byte PID means the first byte is
	/// not the value, and a one-byte answer to a two-byte PID has no value in
	/// it at all.
	WrongWidth { expected: usize, got: usize },
}

impl ObdPid {
	/// How many data bytes SAE J1979 defines for this parameter — one for the
	/// single-byte forms, two for the `256A + B` ones.
	pub const fn data_len(&self) -> usize {
		match self.form {
			RawForm::U8First | RawForm::U8Second => 1,
			RawForm::U16Be | RawForm::U16Le | RawForm::I16Be => 2,
			RawForm::U24Be => 3,
			RawForm::U32Be => 4,
			// No J1979 parameter in [`STANDARD`] uses the general form — the
			// standard set is one and two byte values at the front of the
			// response — but the width still has to be right rather than
			// plausible, because [`conversion_for`] refuses on it. A field of
			// `byte_length` bytes at `byte_offset` needs the sum to be present.
			RawForm::Int {
				byte_offset, byte_length, ..
			} => byte_offset as usize + byte_length as usize,
		}
	}
}

/// Decide whether the standard conversion may be applied to what a unit
/// answered at this parameter's mirror.
///
/// `mirror_established` is the caller's answer to "is this a unit the standard
/// set is defined on" — in the CLI, `UnitAddress::is_emissions_related`. Both
/// gates are needed and neither subsumes the other: the reference car's climate
/// unit answers `F405` with one byte, the right width for a wrong quantity, so
/// the width check alone would convert it; and its in-block gearbox answers
/// `F40D` with two bytes where PID `0D` is one, so the block check alone would
/// convert that.
pub fn conversion_for(p: &ObdPid, mirror_established: bool, data: &[u8]) -> Result<MeasurementDef, Unconverted> {
	if !mirror_established {
		return Err(Unconverted::NotAnEmissionsUnit);
	}
	if data.len() != p.data_len() {
		return Err(Unconverted::WrongWidth {
			expected: p.data_len(),
			got: data.len(),
		});
	}
	Ok(p.to_def())
}

impl ObdPid {
	/// The catalog row for reading this parameter over UDS.
	pub fn to_def(&self) -> MeasurementDef {
		MeasurementDef {
			name: Cow::Borrowed(self.name),
			unit: Cow::Borrowed(self.unit),
			address: ReadId::Uds(did_for_pid(self.pid)),
			raw_form: self.form,
			scaling: Scaling::Linear(LinearScale {
				factor: self.factor,
				offset: self.offset,
			}),
		}
	}
}

/// Catalog rows for whichever parameters a control unit actually implements.
///
/// Feed it the identifiers a sweep found (`vagcan scan`); only the standard
/// linear parameters among them are returned.
pub fn catalog_for(supported_dids: &[u16]) -> Vec<MeasurementDef> {
	PIDS
		.iter()
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
	fn mode_nine_text_is_decoded_from_the_cars_own_bytes() {
		// Exactly what the reference engine returned, count byte included.
		let vin = decode_info_text(b"\x01XW8AD4NE9JH008917").unwrap();
		assert_eq!(vin, vec!["XW8AD4NE9JH008917"]);

		let calid = decode_info_text(b"\x018V0264H 0005AEAJ").unwrap();
		assert_eq!(calid, vec!["8V0264H 0005AEAJ"]);

		// The ECU name carries an interior NUL; only the padding is trimmed.
		let name = decode_info_text(b"\x01ECM\0-EngineControl\0\0").unwrap();
		assert_eq!(name, vec!["ECM\0-EngineControl"]);

		assert_eq!(did_for_info_pid(0x02), 0xF802);
	}

	#[test]
	fn a_response_that_does_not_divide_into_items_is_refused() {
		// Splitting it anyway would hand back fragments of a value.
		assert_eq!(decode_info_text(b"\x02abcde"), None);
		assert_eq!(decode_info_text(b"\x00abc"), None);
		assert_eq!(decode_info_text(b"\x01"), None);
		assert_eq!(decode_info_text(b""), None);
	}

	#[test]
	fn several_items_come_back_separately() {
		// Mode 09 allows a count above one; each item is its own string.
		let items = decode_info_text(b"\x02AAAABBBB").unwrap();
		assert_eq!(items, vec!["AAAA", "BBBB"]);
	}

	#[test]
	fn the_climate_units_f405_is_refused_because_no_conversion_carries_it() {
		// research/dumps/survey-parked.jsonl and survey-driving-20260802-03{14,22}
		// .jsonl, unit 0x746 (5E0907044AM) against unit 0x7E0 (8V0906264H):
		//
		//   climate F405   0x57=87   0x5A=90   0x6D=109
		//   engine  F405   0x81=129  0x5D=93   0x89=137  → 89 / 53 / 97 °C
		//
		// Between the first two the engine's coolant fell 36 °C while the
		// climate value rose by 3; a line through those two pairs (slope −12)
		// predicts −135 for the third, where the engine reads 137. So the
		// climate value is not this parameter under any linear conversion —
		// and it is one byte wide, exactly like PID 05, which is why the width
		// check alone cannot save us here.
		let p = pid(0x05).unwrap();
		assert_eq!(p.data_len(), 1);
		for raw in [0x57u8, 0x5A, 0x6D] {
			assert_eq!(
				conversion_for(p, false, &[raw]),
				Err(Unconverted::NotAnEmissionsUnit),
				"a non-emissions unit's F405 must never convert"
			);
		}
		// The same bytes on the engine do convert, unchanged.
		assert_eq!(conversion_for(p, true, &[0x81]).unwrap().interpret(&[0x81]), Some(89.0));
	}

	#[test]
	fn a_response_of_the_wrong_width_is_refused_even_on_the_engine_block() {
		// The gearbox (0x7E1) is inside the ISO block, and still answers F40D
		// with two bytes where J1979 PID 0D is one — its own catalog row reads
		// them little-endian at ×0.01 km/h. Converting the first byte as km/h
		// would report 0x9C 0x02 (668.4 km/h in its own units) as 156 km/h.
		let speed = pid(0x0D).unwrap();
		assert_eq!(
			conversion_for(speed, true, &[0x00, 0x00]),
			Err(Unconverted::WrongWidth { expected: 1, got: 2 })
		);
		// And the climate unit's one-byte F40C, where PID 0C is two bytes.
		let rpm = pid(0x0C).unwrap();
		assert_eq!(rpm.data_len(), 2);
		assert_eq!(conversion_for(rpm, true, &[0x0A]), Err(Unconverted::WrongWidth { expected: 2, got: 1 }));
	}

	#[test]
	fn every_parameter_the_car_proved_still_converts_at_its_own_width() {
		// The five blind fits, at the byte widths the engine actually answered
		// in research/dumps/survey-parked.jsonl. If the new gate rejected any
		// of these, it would have broken the one path that is established.
		let proven: &[(u8, &[u8], f64)] = &[
			(0x05, &[0x81], 89.0),          // parked coolant
			(0x0D, &[0x00], 0.0),           // parked road speed
			(0x0F, &[0x71], 73.0),          // parked intake air
			(0x23, &[0x05, 0xCE], 14860.0), // parked fuel rail, two bytes ×10
			(0x46, &[0x44], 28.0),          // parked ambient
		];
		for (id, bytes, expected) in proven {
			let p = pid(*id).unwrap();
			let def = conversion_for(p, true, bytes).unwrap_or_else(|e| panic!("PID {id:02X} refused on the engine: {e:?}"));
			assert_eq!(def.interpret(bytes), Some(*expected), "PID {id:02X}");
		}
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
