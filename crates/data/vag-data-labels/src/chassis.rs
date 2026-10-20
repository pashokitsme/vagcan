//! Which project a car is, in VCDS's words: `UDS_EV/chassis.clb`.
//!
//! Some unit files and redirect sources carry a project suffix —
//! `EV_InventedMQB_016_SK37` beside `_AU37` and `_VW48`, all the same unit at the
//! same version — and VCDS picks among them by the car's project. The project
//! comes from the VIN: `chassis.clb` maps the **model code**, VIN characters 7
//! and 8, to a project code (`research/vcds-registry/README.md` §chassis).
//!
//! The file is a plain label file once decrypted ([`crate::clb::decrypt_clb`]):
//! one `<model code>,<project>` line per model. It is read here rather than
//! through [`crate::label::parse_label`] because that parser takes a numeric
//! first field for a measurement block, and some model codes are two digits.

use std::collections::BTreeMap;
use std::path::Path;

use crate::clb::decrypt_clb;
use crate::dtc::find_named;

/// The model-code table's file name inside a VCDS installation.
pub const CHASSIS_FILE: &str = "chassis.clb";

/// Where the model code sits in a VIN: characters 7 and 8, 1-based.
///
/// ISO 3779 makes positions 4–9 the vehicle descriptor section and leaves its
/// content to the maker; the model code in 7–8 is the VW group's convention for
/// its own VINs — which `chassis.clb` is keyed by — not a property of any one car.
const MODEL_CODE: std::ops::Range<usize> = 6..8;

/// Model code → project code, as one installation's `chassis.clb` has it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Projects {
	by_model: BTreeMap<String, String>,
}

impl Projects {
	/// Parse a decrypted `chassis.clb`. Comments (`;`) and lines that are not
	/// two non-empty fields are skipped.
	pub fn parse(plain: &[u8]) -> Self {
		let text: String = plain.iter().map(|b| *b as char).collect();
		let by_model = text
			.lines()
			.filter_map(|line| {
				let content = line.split(';').next()?.trim();
				let mut fields = content.split(',').map(str::trim);
				let (model, project) = (fields.next()?, fields.next()?);
				(!model.is_empty() && !project.is_empty()).then(|| (model.to_ascii_uppercase(), project.to_string()))
			})
			.collect();
		Self { by_model }
	}

	pub fn len(&self) -> usize {
		self.by_model.len()
	}

	pub fn is_empty(&self) -> bool {
		self.by_model.is_empty()
	}

	/// The project of a car, from its VIN; `None` for a VIN too short to hold a
	/// model code, or a model this table does not list.
	pub fn of_vin(&self, vin: &str) -> Option<&str> {
		let model = vin.get(MODEL_CODE)?.to_ascii_uppercase();
		self.by_model.get(&model).map(String::as_str)
	}
}

/// Read an installation's `chassis.clb`; `None` when it has none.
pub fn load_projects(root: &Path) -> Option<Projects> {
	let file = find_named(root, CHASSIS_FILE)?;
	let bytes = std::fs::read(file).ok()?;
	Some(Projects::parse(&decrypt_clb(&bytes)))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_model_code_is_vin_characters_seven_and_eight() {
		// Invented models and projects; the VIN shape is the standard's.
		let projects = Projects::parse(b"; model, project\r\nQZ,XX37\r\n42,YY48\r\nbroken\r\n,ZZ01\r\n");
		assert_eq!(projects.len(), 2);
		assert_eq!(projects.of_vin("AAA123QZ9B000001"), Some("XX37"));
		assert_eq!(projects.of_vin("aaa123qz"), Some("XX37"), "case does not matter");
		assert_eq!(projects.of_vin("AAA123425B000001"), Some("YY48"), "a numeric model code is a model code");
		assert_eq!(projects.of_vin("AAA123ZZ5B000001"), None, "a model the table does not list");
		assert_eq!(projects.of_vin("AAA12"), None, "too short to hold one");
	}
}
