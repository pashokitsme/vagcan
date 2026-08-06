//! What this car is, as far as the bus cannot say — and where each number came
//! from.
//!
//! Almost everything `measure` needs is on the bus every run: speed, engine speed,
//! gear, pedal, boost, and the barometer and ambient sensor the air density
//! comes from. A handful of things are not. The mass is on a registration
//! document, the tyre size is on the sidewall, and `CdA` and `Crr` are on no
//! document at all — they are measured on the road by the coastdown `measure setup`
//! ends with. Those live here, one file per car, written once.
//!
//! **Every parameter carries its provenance and there is no `default`.** A
//! figure a person typed, a figure fitted on this car, a figure that is
//! arithmetic on another figure and a figure out of a textbook are four
//! different kinds of claim, and what is shown says which one it is showing. A
//! parameter this tool cannot obtain honestly is one the run does without —
//! which is why `--full` is refused rather than fed a hatchback-shaped guess.
//!
//! The file is keyed by VIN and so lives in the user's own data directory, never
//! in `catalogs/` — the reasoning is at [`crate::datadir::vagcan_dir`].
//!
//! JSON is read and written by hand against `serde_json::Value` rather than by
//! `#[derive(Serialize)]`, which is how the rest of this crate does it: `vagcan`
//! depends on `serde_json` and not on `serde`'s derive macros. The shape is the
//! one in the design, §0.

use anyhow::{Context, bail};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// Where a parameter came from. Kinds of claim that are not interchangeable,
/// kept apart so that two runs are never compared across a change in how the
/// car was described.
///
/// There is deliberately no `default` variant. A parameter with no honest source
/// is one the run does without, and a variant meaning "this was made up" is the
/// one place that rule could leak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
	/// A person read it off a document or a sidewall and typed it in.
	Stated,
	/// Fitted from this car's own coastdown, on the day the entry records.
	Coastdown,
	/// Arithmetic on a stated value — the wheel radius out of the tyre size.
	DerivedFromTyre,
	/// A published typical figure — Wong, *Theory of Ground Vehicles* — for a
	/// quantity nobody can measure at the roadside. Recorded as such, so a
	/// number resting on it is never mistaken for one that does not.
	WongTypical,
	/// No correction was applied. Not the same as a correction of 1.0 having
	/// been chosen and checked.
	Uncorrected,
	/// Read from the car's own sensors, or measured by this tool as it ran.
	Measured,
	/// The ISO 2533 standard atmosphere — 101.325 kPa and 15 °C, ρ = 1.2250
	/// kg/m³ — used because the car publishes no barometer and no ambient
	/// sensor and nobody said what the air was doing. A property of a published
	/// standard rather than of any car, and never a measurement: air density
	/// enters drag linearly, so a real day away from standard moves every figure
	/// resting on this one. It is spelled out so that it can never be mistaken
	/// for a reading.
	StandardAtmosphere,
}

impl Source {
	/// The spelling used in the file and in anything shown to the user.
	pub fn as_str(self) -> &'static str {
		match self {
			Source::Stated => "stated",
			Source::Coastdown => "coastdown",
			Source::DerivedFromTyre => "derived-from-tyre",
			Source::WongTypical => "wong-typical",
			Source::Uncorrected => "uncorrected",
			Source::Measured => "measured",
			Source::StandardAtmosphere => "standard-atmosphere",
		}
	}

	/// The inverse of [`Source::as_str`]. Public because a scratch file written
	/// mid-setup carries a provenance too, and it has to come back as the same
	/// kind of claim it went out as.
	pub fn parse(text: &str) -> Option<Source> {
		Some(match text {
			"stated" => Source::Stated,
			"coastdown" => Source::Coastdown,
			"derived-from-tyre" => Source::DerivedFromTyre,
			"wong-typical" => Source::WongTypical,
			"uncorrected" => Source::Uncorrected,
			"measured" => Source::Measured,
			"standard-atmosphere" => Source::StandardAtmosphere,
			_ => return None,
		})
	}
}

/// A parameter and where it came from, which is the only form a parameter takes
/// in this file. `at` is the day it was obtained, so a car re-weighed or re-shod
/// can be told from one that was not.
#[derive(Clone, Debug, PartialEq)]
pub struct Sourced<T> {
	pub value: T,
	pub source: Source,
	pub at: Option<String>,
}

impl<T> Sourced<T> {
	/// A parameter belonging to no particular day — a speed correction nobody
	/// has applied, an inertia out of a book.
	pub fn new(value: T, source: Source) -> Sourced<T> {
		Sourced { value, source, at: None }
	}

	/// A parameter obtained on a given day, written `YYYY-MM-DD`.
	pub fn on(value: T, source: Source, at: impl Into<String>) -> Sourced<T> {
		Sourced {
			value,
			source,
			at: Some(at.into()),
		}
	}
}

/// The driver a registration document has already counted, in kilograms.
///
/// Regulation (EU) No 1230/2012, Annex I: the *mass in running order* includes
/// the driver at a nominal **75 kg**, along with a tank at 90 %. It is a figure
/// from a published regulation rather than from any one car, which is why it may
/// appear in the code at all, and it is here for exactly one purpose — so that
/// [`Mass::total`] can avoid counting the driver twice.
pub const DRIVER_KG: f64 = 75.0;

/// The car's mass in parts, never as a total.
///
/// The trap this type exists for: the registration document's field G ("mass in
/// running order"; "mass in service" on a UK V5C) **already includes a 75 kg
/// driver and a near-full tank**. Asking an owner for "kerb mass plus yourself
/// and your fuel" therefore double-counts about 150 kg on a 1400 kg car — and
/// mass lands on the inertial term, some 90 % of the power figure, and on the
/// coastdown fit besides. So the tool asks what the document says, asks whether
/// that figure includes a driver, asks what else is aboard, and does the
/// arithmetic itself in [`Mass::total`].
///
/// `aboard_kg` is everything *besides* the driver: passengers, luggage, fuel
/// beyond what the document assumed. An owner heavier or lighter than the
/// regulation's nominal 75 kg puts the difference there, which is the only place
/// it can go without the driver being counted twice or not at all.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mass {
	/// The figure on the document, whatever it turns out to include.
	pub running_order_kg: f64,
	/// Whether that figure already carries a driver — true of an EU field G.
	pub includes_driver: bool,
	/// What is aboard beyond what the stated figure covers, driver excluded.
	pub aboard_kg: f64,
}

impl Mass {
	/// The mass the models use: the stated figure, what is aboard, and a driver
	/// added **only** when the stated figure did not already have one.
	pub fn total(&self) -> f64 {
		let driver = if self.includes_driver { 0.0 } else { DRIVER_KG };
		self.running_order_kg + driver + self.aboard_kg
	}
}

/// What the air and the road were doing when the coastdown was fitted.
///
/// The fit returns `½·ρ·CdA`, so a `CdA` without the `ρ` that was in the air at
/// the time is not a usable number: an unrecorded ±3 % in `ρ` is a direct ±3 %
/// in `CdA`, larger than anything else in the fit. `CdA` also scales with the
/// mass used in the fit, which is that day's load and not the run's. Both are
/// kept, and so are the wind and the grade the reciprocal passes implied,
/// because they say how far the pair can be trusted.
#[derive(Clone, Debug, PartialEq)]
pub struct FitConditions {
	/// How many accepted passes went into the fit. One pass cannot separate a
	/// slope from rolling resistance; two reciprocal ones can.
	pub passes: u32,
	pub rho_at_fit: f64,
	pub rho_source: Source,
	pub mass_at_fit_kg: f64,
	/// Only a reciprocal pair yields these, so both are absent after one pass.
	pub wind_estimate_ms: Option<f64>,
	pub grade_estimate_percent: Option<f64>,
}

/// One control unit as it named itself when the file was written, so a file can
/// be recognised as describing a car that has since had a unit replaced.
#[derive(Clone, Debug, PartialEq)]
pub struct UnitRef {
	/// The request identifier, e.g. `0x7E0`.
	pub request: u16,
	/// What the unit answered to `F187`.
	pub part_number: String,
}

/// The road load: the pair a coastdown produces, and which no run may have only
/// one half of.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadLoad {
	pub cda: f64,
	pub crr: f64,
}

/// The car-side half of what the power model needs. The other half — air
/// density, grade, headwind — is read off the bus or given on the command line
/// for each run and is never stored here, because it belongs to the day rather
/// than to the car.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarConditions {
	pub mass_kg: f64,
	pub radius_m: f64,
	pub i_wheels_kgm2: f64,
	pub i_engine_kgm2: f64,
}

/// One car, as this tool knows it.
///
/// Every parameter is optional because a setup can be abandoned halfway and
/// everything already answered is kept. What that costs is stated in exactly one
/// place, [`CarFile::road_load`]: a missing parameter is a named absence, never a
/// substituted guess.
#[derive(Clone, Debug, PartialEq)]
pub struct CarFile {
	/// The key. The car names itself, and its file is found by that name.
	pub vin: String,
	pub units: Vec<UnitRef>,
	pub mass: Option<Sourced<Mass>>,
	pub tyre: Option<Sourced<String>>,
	pub rolling_radius_m: Option<Sourced<f64>>,
	pub i_wheels_kgm2: Option<Sourced<f64>>,
	pub i_engine_kgm2: Option<Sourced<f64>>,
	pub cda: Option<Sourced<f64>>,
	pub crr: Option<Sourced<f64>>,
	/// The conditions `cda` and `crr` were fitted in. They belong to the pair
	/// and not to either coefficient; the file writes them alongside `cda`.
	pub fit: Option<FitConditions>,
	pub speed_scale: Option<Sourced<f64>>,
	pub refresh_estimate_s: Option<Sourced<f64>>,
}

impl CarFile {
	/// A car that has just named itself and nothing more.
	pub fn new(vin: impl Into<String>) -> CarFile {
		CarFile {
			vin: vin.into(),
			units: Vec::new(),
			mass: None,
			tyre: None,
			rolling_radius_m: None,
			i_wheels_kgm2: None,
			i_engine_kgm2: None,
			cda: None,
			crr: None,
			fit: None,
			speed_scale: None,
			refresh_estimate_s: None,
		}
	}

	/// Where this tool keeps the file for one car: `car.json` inside that car's
	/// own directory, beside its saved measurements and its reports.
	///
	/// The directory is the VIN. It arrives from the bus, so it is not trusted
	/// as a path: a unit answering with a separator or a `..` would otherwise
	/// choose where this tool writes.
	pub fn path_for(vin: &str) -> anyhow::Result<PathBuf> {
		Ok(crate::datadir::car_dir(checked_vin(vin)?.as_str())?.join("car.json"))
	}

	/// Read a car file. Anything it cannot vouch for is an error naming the
	/// field, because a quietly half-read file becomes a wrong power figure.
	pub fn load(path: &Path) -> anyhow::Result<CarFile> {
		let text = std::fs::read_to_string(path).with_context(|| format!("reading the car file {}", path.display()))?;
		CarFile::from_json(&text).with_context(|| format!("in the car file {}", path.display()))
	}

	/// Write the car file, creating its directory. The caller prints the path:
	/// twenty minutes of coastdown should not end in a matter of faith.
	pub fn save(&self, path: &Path) -> anyhow::Result<()> {
		if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
			std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
		}
		let mut text = serde_json::to_string_pretty(&self.to_json()?)?;
		text.push('\n');
		std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
	}

	/// The mass the models use, or `None` while nobody has said what it is.
	pub fn mass_total_kg(&self) -> Option<f64> {
		self.mass.as_ref().map(|mass| mass.value.total())
	}

	/// Everything `--full` needs, or the list of what is missing.
	///
	/// The `Err` side is user-facing copy — it is printed as the reason `--full`
	/// was refused — so it names things in the words the owner was asked in
	/// rather than in field names.
	pub fn road_load(&self) -> Result<(RoadLoad, CarConditions), Vec<&'static str>> {
		let mut missing = Vec::new();
		let mass = self.mass_total_kg();
		if mass.is_none() {
			missing.push("the car's mass");
		}
		if self.rolling_radius_m.is_none() {
			missing.push("the tyre size, and the wheel radius that comes from it");
		}
		if self.i_wheels_kgm2.is_none() {
			missing.push("the wheel inertia");
		}
		if self.i_engine_kgm2.is_none() {
			missing.push("the engine inertia");
		}
		if self.cda.is_none() {
			missing.push("drag area (CdA), which the coastdown in `vagcan measure setup` measures");
		}
		if self.crr.is_none() {
			missing.push("rolling resistance (Crr), from that same coastdown");
		}
		if !missing.is_empty() {
			return Err(missing);
		}
		let value = |field: &Option<Sourced<f64>>| field.as_ref().expect("checked just above").value;
		Ok((
			RoadLoad {
				cda: value(&self.cda),
				crr: value(&self.crr),
			},
			CarConditions {
				mass_kg: mass.expect("checked just above"),
				radius_m: value(&self.rolling_radius_m),
				i_wheels_kgm2: value(&self.i_wheels_kgm2),
				i_engine_kgm2: value(&self.i_engine_kgm2),
			},
		))
	}

	fn to_json(&self) -> anyhow::Result<Value> {
		let mut root = Map::new();
		root.insert("vin".into(), Value::String(checked_vin(&self.vin)?));
		if !self.units.is_empty() {
			let units: Vec<Value> = self
				.units
				.iter()
				.map(|unit| json!({ "request": format!("{:03X}", unit.request), "part_number": unit.part_number }))
				.collect();
			root.insert("units".into(), Value::Array(units));
		}
		if let Some(mass) = &self.mass {
			let mut entry = entry_of(number("mass_kg", mass.value.total())?, mass.source, mass.at.as_deref());
			entry.insert(
				"parts".into(),
				json!({
						"running_order": number("mass_kg.running_order", mass.value.running_order_kg)?,
						"includes_driver": mass.value.includes_driver,
						"aboard": number("mass_kg.aboard", mass.value.aboard_kg)?,
				}),
			);
			root.insert("mass_kg".into(), Value::Object(entry));
		}
		if let Some(tyre) = &self.tyre {
			let entry = entry_of(Value::String(tyre.value.clone()), tyre.source, tyre.at.as_deref());
			root.insert("tyre".into(), Value::Object(entry));
		}
		for (key, slot) in [
			("rolling_radius_m", &self.rolling_radius_m),
			("i_wheels_kgm2", &self.i_wheels_kgm2),
			("i_engine_kgm2", &self.i_engine_kgm2),
			("speed_scale", &self.speed_scale),
			("refresh_estimate_s", &self.refresh_estimate_s),
		] {
			if let Some(sourced) = slot {
				let entry = entry_of(number(key, sourced.value)?, sourced.source, sourced.at.as_deref());
				root.insert(key.into(), Value::Object(entry));
			}
		}
		if let Some(cda) = &self.cda {
			let mut entry = entry_of(number("cda", cda.value)?, cda.source, cda.at.as_deref());
			if let Some(fit) = &self.fit {
				entry.insert("passes".into(), json!(fit.passes));
				entry.insert("rho_at_fit".into(), number("rho_at_fit", fit.rho_at_fit)?);
				entry.insert("rho_source".into(), Value::String(fit.rho_source.as_str().into()));
				entry.insert("mass_at_fit_kg".into(), number("mass_at_fit_kg", fit.mass_at_fit_kg)?);
				if let Some(wind) = fit.wind_estimate_ms {
					entry.insert("wind_estimate_ms".into(), number("wind_estimate_ms", wind)?);
				}
				if let Some(grade) = fit.grade_estimate_percent {
					entry.insert("grade_estimate_percent".into(), number("grade_estimate_percent", grade)?);
				}
			}
			root.insert("cda".into(), Value::Object(entry));
		}
		if let Some(crr) = &self.crr {
			let mut entry = entry_of(number("crr", crr.value)?, crr.source, crr.at.as_deref());
			if let Some(fit) = &self.fit {
				entry.insert("passes".into(), json!(fit.passes));
			}
			// Not a tyre property: everything speed-independent lands in this
			// intercept, and the file says so where somebody might otherwise
			// compare it against a tyre datasheet.
			entry.insert("includes".into(), json!("bearings, seals, pad rub, gearbox churning"));
			root.insert("crr".into(), Value::Object(entry));
		}
		Ok(Value::Object(root))
	}

	fn from_json(text: &str) -> anyhow::Result<CarFile> {
		let root: Value = serde_json::from_str(text).context("parsing the car file")?;
		let root = root.as_object().context("the car file is not a JSON object")?;
		let vin = root
			.get("vin")
			.and_then(Value::as_str)
			.context("no `vin` — a car file is keyed by the car")?;
		let mut car = CarFile::new(checked_vin(vin)?);

		if let Some(units) = root.get("units") {
			for unit in units.as_array().context("`units` is not a list")? {
				let request = unit.get("request").and_then(Value::as_str).context("a unit with no `request`")?;
				let request = u16::from_str_radix(request, 16).with_context(|| format!("a unit whose `request` is not hexadecimal: {request:?}"))?;
				let part_number = unit.get("part_number").and_then(Value::as_str).context("a unit with no `part_number`")?;
				car.units.push(UnitRef {
					request,
					part_number: part_number.to_string(),
				});
			}
		}

		if let Some(entry) = root.get("mass_kg") {
			let (source, at) = provenance("mass_kg", entry)?;
			// Deliberately not the `value`: the total is arithmetic, and the
			// arithmetic is this tool's. A file holding only a total is the
			// shape that hides a double-counted driver, so it is refused.
			let parts = entry.get("parts").context(
				"`mass_kg` has no `parts` — this tool keeps what the document said and what is aboard, \
                 because a mass in running order already includes a driver",
			)?;
			car.mass = Some(Sourced {
				value: Mass {
					running_order_kg: positive("mass_kg.parts.running_order", field(parts, "running_order")?)?,
					includes_driver: parts
						.get("includes_driver")
						.and_then(Value::as_bool)
						.context("`mass_kg.parts` does not say whether the figure includes a driver")?,
					aboard_kg: finite("mass_kg.parts.aboard", field(parts, "aboard")?)?,
				},
				source,
				at,
			});
		}

		if let Some(entry) = root.get("tyre") {
			let (source, at) = provenance("tyre", entry)?;
			let value = entry.get("value").and_then(Value::as_str).context("`tyre` has no `value`")?;
			car.tyre = Some(Sourced {
				value: value.to_string(),
				source,
				at,
			});
		}

		car.rolling_radius_m = positive_entry(root, "rolling_radius_m")?;
		car.i_wheels_kgm2 = positive_entry(root, "i_wheels_kgm2")?;
		car.i_engine_kgm2 = positive_entry(root, "i_engine_kgm2")?;
		car.speed_scale = positive_entry(root, "speed_scale")?;
		car.refresh_estimate_s = positive_entry(root, "refresh_estimate_s")?;
		car.cda = positive_entry(root, "cda")?;
		car.crr = positive_entry(root, "crr")?;

		if let Some(entry) = root.get("cda") {
			let fitted = car.cda.as_ref().is_some_and(|cda| cda.source == Source::Coastdown);
			if entry.get("rho_at_fit").is_none() {
				if fitted {
					bail!(
						"`cda` says it was measured by a coastdown but does not record the air density it was \
                         fitted at, and the fit returns ½·ρ·CdA"
					);
				}
			} else {
				car.fit = Some(FitConditions {
					passes: entry
						.get("passes")
						.and_then(Value::as_u64)
						.context("`cda` records the conditions of a fit but not how many passes")? as u32,
					rho_at_fit: positive("cda.rho_at_fit", field(entry, "rho_at_fit")?)?,
					rho_source: source_of("cda.rho_source", entry.get("rho_source"))?,
					mass_at_fit_kg: positive("cda.mass_at_fit_kg", field(entry, "mass_at_fit_kg")?)?,
					wind_estimate_ms: optional_finite("cda.wind_estimate_ms", entry.get("wind_estimate_ms"))?,
					grade_estimate_percent: optional_finite("cda.grade_estimate_percent", entry.get("grade_estimate_percent"))?,
				});
			}
		}

		Ok(car)
	}
}

/// The **geometric** (unloaded) rolling radius of a tyre, in metres, from the
/// size as written on the sidewall: rim radius plus section height, i.e.
/// `rim·25.4/2 + width·aspect/100` in millimetres. `205/55R16` gives 0.31595 m.
///
/// Geometric and not dynamic, deliberately. A loaded tyre rolls on a shorter
/// radius — commonly quoted as about 0.98 of this one — but that factor is a
/// rule of thumb that moves with load, pressure, speed and construction, and
/// applying it here would put a number in the car file that nothing measured and
/// no [`Source`] honestly describes. This file has no variant for a guess, on
/// purpose. What the sidewall says is arithmetic on a stated value and really is
/// `derived-from-tyre`; the same figure times somebody's 0.98 would not be.
///
/// Where the difference matters it is measurable rather than assumable: a run
/// against GPS gives `speed_scale`, with a provenance of its own. And the ~2 %
/// is smaller than it looks in both places the radius is used — it enters `δ` as
/// `1/r²` on a term worth a few per cent of the total, and the gear ratios are
/// learned from this car's own speed and engine speed, so a uniformly shifted
/// radius does not move the slip test.
///
/// `None` for anything that is not a tyre size. Sizes outside what a passenger
/// car wears — a section width under 100 mm or over 400, an aspect ratio outside
/// 20–95, a rim under 10 inches or over 26 — are refused rather than converted,
/// because a plausible radius derived from a typo is worse than no radius.
pub fn rolling_radius_m(tyre: &str) -> Option<f64> {
	let text = tyre.trim().to_ascii_uppercase();
	// A leading service description, the `P` of `P205/55R16`.
	let rest = text.trim_start_matches(|c: char| c.is_ascii_alphabetic());
	let (width, rest) = digits(rest)?;
	let rest = rest.strip_prefix('/')?;
	let (aspect, rest) = digits(rest)?;
	let rest = rest.trim_start();
	// The construction letter: `R` for radial, possibly behind a speed symbol as
	// in `ZR`. Anything else — `205/55X16`, the bias-ply `205/55-16` — is not a
	// size this formula applies to.
	let letters: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
	if !letters.ends_with('R') {
		return None;
	}
	let rest = &rest[letters.len()..];
	let (whole, rest) = digits(rest)?;
	// Half-inch rims are a real size. The load index and speed symbol that may
	// follow the size are somebody else's business, but they are separated from
	// it by a space and nothing else may be.
	let (rim, rest) = match rest.strip_prefix('.') {
		Some(after) => {
			let (fraction, tail) = digits(after)?;
			let places = (after.len() - tail.len()) as i32;
			(whole as f64 + fraction as f64 / 10f64.powi(places), tail)
		}
		None => (whole as f64, rest),
	};
	if !rest.is_empty() && !rest.starts_with(' ') {
		return None;
	}
	if !(100..=400).contains(&width) || !(20..=95).contains(&aspect) || !(10.0..=26.0).contains(&rim) {
		return None;
	}
	// 25.4 mm to the inch by definition, and the rim size is a diameter.
	Some((rim * 25.4 / 2.0 + width as f64 * aspect as f64 / 100.0) / 1000.0)
}

/// Leading decimal digits, and what follows them.
fn digits(text: &str) -> Option<(u32, &str)> {
	let end = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
	if end == 0 {
		return None;
	}
	Some((text[..end].parse().ok()?, &text[end..]))
}

/// A VIN is about to become a file name, and it came off the bus.
fn checked_vin(vin: &str) -> anyhow::Result<String> {
	let vin = vin.trim();
	if vin.is_empty() || vin.len() > 32 || !vin.chars().all(|c| c.is_ascii_alphanumeric()) {
		bail!("{vin:?} is not a VIN this tool will make a file name out of");
	}
	Ok(vin.to_string())
}

fn entry_of(value: Value, source: Source, at: Option<&str>) -> Map<String, Value> {
	let mut entry = Map::new();
	entry.insert("value".into(), value);
	entry.insert("source".into(), Value::String(source.as_str().into()));
	if let Some(at) = at {
		entry.insert("at".into(), Value::String(at.into()));
	}
	entry
}

/// JSON has no infinity, and a silent `null` would come back days later as a
/// parameter nobody ever supplied.
fn number(key: &str, value: f64) -> anyhow::Result<Value> {
	if !value.is_finite() {
		bail!("`{key}` is {value}, which is not a number a car file can hold");
	}
	Ok(json!(value))
}

fn provenance(key: &str, entry: &Value) -> anyhow::Result<(Source, Option<String>)> {
	let source = source_of(&format!("{key}.source"), entry.get("source"))?;
	let at = entry.get("at").and_then(Value::as_str).map(str::to_string);
	Ok((source, at))
}

fn source_of(key: &str, value: Option<&Value>) -> anyhow::Result<Source> {
	let text = value.and_then(Value::as_str).with_context(|| format!("`{key}` is missing"))?;
	Source::parse(text).with_context(|| {
		format!(
			"`{key}` is {text:?}, which is not a source this tool knows — every parameter has to say where it \
             came from, and there is no default"
		)
	})
}

fn field<'a>(entry: &'a Value, key: &str) -> anyhow::Result<&'a Value> {
	entry.get(key).with_context(|| format!("`{key}` is missing"))
}

fn finite(key: &str, value: &Value) -> anyhow::Result<f64> {
	let number = value.as_f64().with_context(|| format!("`{key}` is not a number"))?;
	if !number.is_finite() {
		bail!("`{key}` is not a finite number");
	}
	Ok(number)
}

fn positive(key: &str, value: &Value) -> anyhow::Result<f64> {
	let number = finite(key, value)?;
	if number <= 0.0 {
		bail!("`{key}` is {number}, and none of these quantities can be zero or negative");
	}
	Ok(number)
}

fn optional_finite(key: &str, value: Option<&Value>) -> anyhow::Result<Option<f64>> {
	match value {
		None | Some(Value::Null) => Ok(None),
		Some(value) => finite(key, value).map(Some),
	}
}

fn positive_entry(root: &Map<String, Value>, key: &str) -> anyhow::Result<Option<Sourced<f64>>> {
	let Some(entry) = root.get(key) else { return Ok(None) };
	let (source, at) = provenance(key, entry)?;
	let value = positive(&format!("{key}.value"), field(entry, "value")?)?;
	Ok(Some(Sourced { value, source, at }))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A unique-per-test temp dir, cleaned up on drop — the shape `vag-data`'s
	/// corpus tests use. Nothing here may write inside a checkout.
	struct TempDir(PathBuf);

	impl TempDir {
		fn new(tag: &str) -> TempDir {
			let path = std::env::temp_dir().join(format!("vagcan-carfile-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
			std::fs::create_dir_all(&path).unwrap();
			TempDir(path)
		}
	}

	impl Drop for TempDir {
		fn drop(&mut self) {
			let _ = std::fs::remove_dir_all(&self.0);
		}
	}

	fn a_described_car() -> CarFile {
		let mut car = CarFile::new("XW8AD4NE9JH008917");
		car.units.push(UnitRef {
			request: 0x7E0,
			part_number: "8V0906264H".into(),
		});
		car.mass = Some(Sourced::on(
			Mass {
				running_order_kg: 1395.0,
				includes_driver: true,
				aboard_kg: 80.0,
			},
			Source::Stated,
			"2026-08-03",
		));
		car.tyre = Some(Sourced::on("205/55R16".to_string(), Source::Stated, "2026-08-03"));
		car.rolling_radius_m = Some(Sourced::new(rolling_radius_m("205/55R16").unwrap(), Source::DerivedFromTyre));
		car.i_wheels_kgm2 = Some(Sourced::new(5.5, Source::WongTypical));
		car.i_engine_kgm2 = Some(Sourced::new(0.34, Source::WongTypical));
		car.cda = Some(Sourced::on(0.63, Source::Coastdown, "2026-08-04"));
		car.crr = Some(Sourced::on(0.0114, Source::Coastdown, "2026-08-04"));
		car.fit = Some(FitConditions {
			passes: 2,
			rho_at_fit: 1.183,
			rho_source: Source::Measured,
			mass_at_fit_kg: 1385.0,
			wind_estimate_ms: Some(0.8),
			grade_estimate_percent: Some(0.3),
		});
		car.speed_scale = Some(Sourced::new(1.0, Source::Uncorrected));
		car.refresh_estimate_s = Some(Sourced::new(0.048, Source::Measured));
		car
	}

	#[test]
	fn a_mass_in_running_order_is_not_charged_for_its_driver_twice() {
		// The document says 1395 kg, and under Regulation 1230/2012 that figure
		// already carries a 75 kg driver and a near-full tank. A passenger and
		// some luggage, 80 kg, are aboard on top of it.
		let honest = Mass {
			running_order_kg: 1395.0,
			includes_driver: true,
			aboard_kg: 80.0,
		};
		assert_eq!(honest.total(), 1475.0);

		// The double-count this type exists to prevent: an earlier draft asked
		// for "kerb mass plus yourself and your fuel", so an owner reading field
		// G off the document added themselves to a figure that had them already.
		let as_the_old_question_would_have_had_it = honest.running_order_kg + DRIVER_KG + honest.aboard_kg;
		assert_eq!(as_the_old_question_would_have_had_it, 1550.0);
		assert_eq!(as_the_old_question_would_have_had_it - honest.total(), DRIVER_KG);
	}

	#[test]
	fn a_stated_mass_without_a_driver_gains_exactly_one() {
		// A true unladen figure: the same car with the same load, described the
		// other way round, and it must come out at the same total.
		let unladen = Mass {
			running_order_kg: 1320.0,
			includes_driver: false,
			aboard_kg: 80.0,
		};
		assert_eq!(unladen.total(), 1475.0);
		let running_order = Mass {
			running_order_kg: 1395.0,
			includes_driver: true,
			aboard_kg: 80.0,
		};
		assert_eq!(unladen.total(), running_order.total());
	}

	#[test]
	fn an_empty_car_is_the_document_and_a_driver_and_nothing_else() {
		assert_eq!(
			Mass {
				running_order_kg: 1395.0,
				includes_driver: true,
				aboard_kg: 0.0
			}
			.total(),
			1395.0
		);
		assert_eq!(
			Mass {
				running_order_kg: 1320.0,
				includes_driver: false,
				aboard_kg: 0.0
			}
			.total(),
			1395.0
		);
	}

	#[test]
	fn a_saved_car_comes_back_with_every_provenance_it_went_in_with() {
		let dir = TempDir::new("round-trip");
		let path = dir.0.join("XW8AD4NE9JH008917.json");
		let car = a_described_car();
		car.save(&path).unwrap();
		let back = CarFile::load(&path).unwrap();
		assert_eq!(back, car);
		assert_eq!(back.cda.as_ref().unwrap().source, Source::Coastdown);
		assert_eq!(back.i_engine_kgm2.as_ref().unwrap().source, Source::WongTypical);
		assert_eq!(back.speed_scale.as_ref().unwrap().source, Source::Uncorrected);
		assert_eq!(back.mass.as_ref().unwrap().at.as_deref(), Some("2026-08-03"));
		assert_eq!(back.fit.as_ref().unwrap().rho_source, Source::Measured);
	}

	#[test]
	fn the_file_holds_the_parts_of_the_mass_and_not_only_the_total() {
		let dir = TempDir::new("mass-shape");
		let path = dir.0.join("car.json");
		a_described_car().save(&path).unwrap();
		let written: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
		let mass = &written["mass_kg"];
		assert_eq!(mass["value"], json!(1475.0));
		assert_eq!(mass["parts"]["running_order"], json!(1395.0));
		assert_eq!(mass["parts"]["includes_driver"], json!(true));
		assert_eq!(mass["parts"]["aboard"], json!(80.0));
		assert_eq!(mass["source"], json!("stated"));
	}

	#[test]
	fn a_mass_recorded_as_a_total_alone_is_refused() {
		// The shape that hides the double-count: nothing in it says whether a
		// driver is in there once, twice or not at all.
		let text = r#"{ "vin": "X1", "mass_kg": { "value": 1550, "source": "stated" } }"#;
		let err = CarFile::from_json(text).unwrap_err().to_string();
		assert!(err.contains("parts"), "{err}");
	}

	#[test]
	fn a_parameter_that_will_not_say_where_it_came_from_is_refused() {
		let text = r#"{ "vin": "X1", "cda": { "value": 0.63, "source": "default" } }"#;
		let err = CarFile::from_json(text).unwrap_err().to_string();
		assert!(err.contains("default"), "{err}");
		let text = r#"{ "vin": "X1", "cda": { "value": 0.63 } }"#;
		assert!(CarFile::from_json(text).is_err());
	}

	#[test]
	fn a_fitted_drag_area_without_the_air_it_was_fitted_in_is_refused() {
		let text = r#"{ "vin": "X1", "cda": { "value": 0.63, "source": "coastdown" } }"#;
		let err = CarFile::from_json(text).unwrap_err().to_string();
		assert!(err.contains("air density"), "{err}");
		// A figure somebody genuinely has from elsewhere is another matter, and
		// is recorded as stated rather than as measured on this car.
		let text = r#"{ "vin": "X1", "cda": { "value": 0.63, "source": "stated" } }"#;
		assert!(CarFile::from_json(text).is_ok());
	}

	#[test]
	fn a_half_described_car_names_what_it_is_missing_in_words() {
		let mut car = CarFile::new("XW8AD4NE9JH008917");
		car.mass = Some(Sourced::new(
			Mass {
				running_order_kg: 1395.0,
				includes_driver: true,
				aboard_kg: 0.0,
			},
			Source::Stated,
		));
		let missing = car.road_load().unwrap_err();
		assert!(missing.iter().any(|what| what.contains("CdA")), "{missing:?}");
		assert!(missing.iter().any(|what| what.contains("tyre")), "{missing:?}");
		assert!(!missing.iter().any(|what| what.contains("mass")), "the mass was answered: {missing:?}");
		// Copy for a person, not field names.
		assert!(missing.iter().all(|what| !what.contains('_')), "{missing:?}");
	}

	#[test]
	fn a_fully_described_car_hands_over_its_road_load() {
		let (load, conditions) = a_described_car().road_load().unwrap();
		assert_eq!(load.cda, 0.63);
		assert_eq!(load.crr, 0.0114);
		assert_eq!(conditions.mass_kg, 1475.0);
		assert!((conditions.radius_m - 0.31595).abs() < 1e-12);
	}

	#[test]
	fn a_tyre_size_becomes_the_geometric_radius_off_the_sidewall() {
		// A 16 inch rim is 406.4 mm across, so 203.2 mm to the bead, plus 55 %
		// of a 205 mm section.
		assert!((rolling_radius_m("205/55R16").unwrap() - 0.31595).abs() < 1e-12);
		assert!((rolling_radius_m("225/40R18").unwrap() - 0.3186).abs() < 1e-12);
		// The forms a sidewall actually comes in.
		assert_eq!(rolling_radius_m(" 205/55r16 "), rolling_radius_m("205/55R16"));
		assert_eq!(rolling_radius_m("P205/55R16"), rolling_radius_m("205/55R16"));
		assert_eq!(rolling_radius_m("205/55ZR16 91V"), rolling_radius_m("205/55R16"));
		// The dynamic radius would be about 2 % smaller, near 0.3096 m. This is
		// deliberately not that; the doc comment says why.
		assert!(rolling_radius_m("205/55R16").unwrap() > 0.315);
	}

	#[test]
	fn a_half_inch_rim_is_a_real_size_and_survives() {
		let expected = (16.5 * 25.4 / 2.0 + 235.0 * 0.80) / 1000.0;
		assert!((rolling_radius_m("235/80R16.5").unwrap() - expected).abs() < 1e-12);
	}

	#[test]
	fn nonsense_gets_no_radius_rather_than_a_plausible_one() {
		for not_a_tyre in [
			"",
			"205",
			"205/55",
			"205/55R",
			"20555R16",
			"205/55X16",
			"205/55-16",
			"abc",
			"205/55R16/17",
			"205/55R16.",
			// Digits in the right places and a size no car wears: a typo, not a tyre.
			"20/55R16",
			"2050/55R16",
			"205/5R16",
			"205/155R16",
			"205/55R160",
			"205/55R8",
		] {
			assert_eq!(rolling_radius_m(not_a_tyre), None, "{not_a_tyre:?} produced a radius");
		}
	}

	#[test]
	fn a_car_is_filed_under_its_vin_and_keeps_the_directory_it_has() {
		// The readable prefix is gone: it was assembled from whatever each
		// caller happened to know, and `measure setup` knew the engine's
		// component string where `measure` did not — which gave one car two
		// directories, the car file in one and the drives in the other.
		//
		// Asked of a temporary `cars/` on purpose. `CarFile::path` answers the
		// *other* question — where does this car's file live *now* — and on a
		// machine that already has a directory the answer is that directory,
		// whatever it is called. Testing the naming rule through `path` would
		// pass on a fresh machine and fail on the owner's, which is what it did.
		let vin = "XW8AD4NE9JH008917";
		let cars = std::env::temp_dir().join(format!("vagcan-naming-{}", std::process::id()));
		std::fs::create_dir_all(&cars).unwrap();
		assert_eq!(crate::datadir::car_folder_in(&cars, vin).unwrap(), vin);

		// And a directory from before the rename is used rather than orphaned.
		// Nothing is moved: a rename under a running tool is the one operation
		// here that can lose a drive somebody is in the middle of recording.
		std::fs::create_dir_all(cars.join(format!("1.8l-R4-TFSI-{vin}"))).unwrap();
		assert_eq!(crate::datadir::car_folder_in(&cars, vin).unwrap(), format!("1.8l-R4-TFSI-{vin}"));
		std::fs::remove_dir_all(&cars).ok();
	}

	#[test]
	fn a_vin_off_the_bus_never_chooses_where_this_tool_writes() {
		assert!(CarFile::path_for("../../catalogs/vehicles/8V0906264H").is_err());
		assert!(CarFile::path_for("").is_err());
		assert!(CarFile::path_for("XW8/AD4").is_err());

		// What a real VIN produces is asserted only as far as this machine can
		// promise: the directory this car already has may be named the old way,
		// so the *shape* is what holds everywhere — one directory under `cars/`,
		// ending in the VIN, with the car file inside it. The naming rule itself
		// is pinned against a temporary `cars/` in the test above.
		let path = CarFile::path_for("XW8AD4NE9JH008917").unwrap();
		assert!(path.ends_with("car.json"), "{path:?}");
		let folder = path.parent().unwrap().file_name().unwrap().to_string_lossy();
		assert!(folder.ends_with("XW8AD4NE9JH008917"), "{path:?}");
		assert_eq!(path.parent().unwrap().parent().unwrap().file_name().unwrap(), "cars");
	}

	#[test]
	fn saving_creates_the_directory_the_car_file_belongs_in() {
		let dir = TempDir::new("mkdir");
		let path = dir.0.join("cars").join("XW8AD4NE9JH008917.json");
		a_described_car().save(&path).unwrap();
		assert!(path.exists());
	}
}
