//! `vagcan dev recording calibrate` — prove new scalings against ones we already trust.
//!
//! `analyse` needs a VCDS log because it needs *reference values with units*.
//! But this project already has references of its own: the 32 standard OBD-II
//! parameters, whose conversions come from the published standard, and the
//! measurements proven in earlier sessions. Anything that tracks one of those
//! linearly can be calibrated against it with no VCDS at all — which is how
//! the engaged gear was found, by arithmetic against the shaft-speed ratio.
//!
//! Three things make this strictly better than the VCDS route where it
//! applies:
//!
//! - **One clock.** Reference and unknown are recorded by the same tool into
//!   the same file with the same timestamps. The half-second alignment error
//!   that once turned an `R² = 1.00000` fit into `0.051` cannot arise.
//! - **Sample rate.** `watch` runs at tens of hertz; a VCDS log manages about
//!   one.
//! - **Coverage.** VCDS reads the groups it has labels for; `watch` reads
//!   whatever is asked of it.
//!
//! What it cannot do: **name** anything (names come from the label files via
//! a VCDS log's `IDE`/`ENG` numbers), or find a quantity unrelated to
//! everything already known — with no reference to track, there is nothing to
//! fit against.

use std::collections::BTreeMap;

use vag_data_labels::measure::{LinearScale, RawForm};

use crate::analyse::{Thresholds, fit_linear};
use crate::discover::{Behaviour, Column, classify};

/// A scaling proven against a reference channel in the same recording.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibrated {
	/// Column header of the unknown identifier (its hex id).
	pub unknown: String,
	/// Column header of the reference it was calibrated against.
	pub reference: String,
	pub form: RawForm,
	pub scale: LinearScale,
	pub r2: f64,
	pub points: usize,
}

/// Every raw interpretation tried, same set the VCDS route uses.
const FORMS: &[RawForm] = &[RawForm::U8First, RawForm::U8Second, RawForm::U16Be, RawForm::U16Le, RawForm::I16Be];

/// How `watch --out` marks a column it could not convert.
pub const RAW_SUFFIX: &str = "_raw";

/// A column holding hex bytes, judged from the values alone.
///
/// Only sound where the recording carries no marker: `0640` is a valid hex
/// pair and a valid decimal, so a converted engine speed of 640 rpm and a raw
/// `06 40` are the same four characters. That ambiguity is why `watch` marks
/// its unconverted columns instead of leaving this to be guessed.
fn looks_like_hex(values: &[String]) -> bool {
	values
		.iter()
		.all(|v| !v.is_empty() && v.len() % 2 == 0 && v.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Split a recording's columns into references (already in engineering units)
/// and unknowns (raw bytes).
///
/// A recording written by `watch` names every unconverted column `…_raw`, and
/// then the split is by name and cannot be wrong. Older recordings have no
/// marker, so the values are inspected instead — with the caveat above.
fn split_columns(columns: &[Column]) -> (Vec<&Column>, Vec<&Column>) {
	let marked = columns.iter().any(|c| c.name.ends_with(RAW_SUFFIX));
	let mut references = Vec::new();
	let mut unknowns = Vec::new();
	for c in columns {
		// A column that never moved cannot calibrate anything and cannot be
		// calibrated; skip it either way rather than reporting a degenerate
		// fit.
		if c.behaviour == Behaviour::Constant {
			continue;
		}
		let unknown = if marked {
			c.name.ends_with(RAW_SUFFIX)
		} else {
			looks_like_hex(&c.values)
		};
		if unknown {
			unknowns.push(c);
		} else {
			references.push(c);
		}
	}
	(references, unknowns)
}

/// Parse a hex cell under one interpretation.
fn read_hex(cell: &str, form: RawForm) -> Option<i32> {
	let bytes: Vec<u8> = (0..cell.len() / 2)
		.map(|i| u8::from_str_radix(&cell[i * 2..i * 2 + 2], 16))
		.collect::<Result<_, _>>()
		.ok()?;
	form.read(&bytes)
}

/// Pair an unknown column with a reference by nearest timestamp.
///
/// Both come from the same recording, so the times are the same clock and the
/// tolerance only has to cover the gap between polling batches — not the
/// unknown offset between two programs.
fn pair(
	unknown: &Column,
	reference: &Column,
	form: RawForm,
	samples_u: &[(f64, String)],
	samples_r: &[(f64, String)],
	tolerance_s: f64,
) -> Vec<(f64, f64)> {
	let mut out = Vec::new();
	let mut used = vec![false; samples_r.len()];
	let _ = (unknown, reference);
	for (t_u, cell) in samples_u {
		let Some(raw) = read_hex(cell, form) else { continue };
		let nearest = samples_r
			.iter()
			.enumerate()
			.filter(|(i, _)| !used[*i])
			.min_by(|(_, a), (_, b)| (a.0 - t_u).abs().partial_cmp(&(b.0 - t_u).abs()).unwrap());
		let Some((index, (t_r, value))) = nearest else { continue };
		if (t_r - t_u).abs() > tolerance_s {
			continue;
		}
		let Ok(value) = value.parse::<f64>() else { continue };
		used[index] = true;
		out.push((raw as f64, value));
	}
	out
}

/// Rebuild each column's `(time, value)` samples from the recording.
///
/// `classify` keeps transitions and distinct values but not the full series,
/// so the CSV is walked once more here.
fn series_of(csv: &str) -> BTreeMap<String, Vec<(f64, String)>> {
	let mut out: BTreeMap<String, Vec<(f64, String)>> = BTreeMap::new();
	let mut lines = csv.lines();
	let Some(header) = lines.next() else { return out };
	let names: Vec<&str> = header.split(',').collect();

	// Same layout rule as `discover`: `<name>_t_s,<name>` pairs when present.
	let mut columns: Vec<(usize, Option<usize>, &str)> = Vec::new();
	let mut i = 1;
	while i < names.len() {
		let paired = names
			.get(i)
			.zip(names.get(i + 1))
			.is_some_and(|(t, v)| t.strip_suffix("_t_s") == Some(*v));
		if paired {
			columns.push((i + 1, Some(i), names[i + 1]));
			i += 2;
		} else {
			columns.push((i, None, names[i]));
			i += 1;
		}
	}

	for line in lines {
		let cells: Vec<&str> = line.split(',').collect();
		let Some(Ok(row_t)) = cells.first().map(|c| c.trim().parse::<f64>()) else {
			continue;
		};
		for (value_at, time_at, name) in &columns {
			let Some(cell) = cells.get(*value_at).map(|c| c.trim()) else { continue };
			if cell.is_empty() {
				continue;
			}
			let t = time_at
				.and_then(|at| cells.get(at))
				.and_then(|c| c.trim().parse::<f64>().ok())
				.unwrap_or(row_t);
			out.entry((*name).to_string()).or_default().push((t, cell.to_string()));
		}
	}
	out
}

/// Calibrate every unknown column against every reference column.
pub fn calibrate(csv: &str, limits: Thresholds) -> Result<Vec<Calibrated>, String> {
	let columns = classify(csv)?;
	let samples = series_of(csv);
	let (references, unknowns) = split_columns(&columns);
	if references.is_empty() {
		return Err(
			"no reference column: record at least one measurement the catalog \
                    already converts, so there is something to calibrate against"
				.to_string(),
		);
	}

	let mut out: Vec<Calibrated> = Vec::new();
	for unknown in &unknowns {
		let Some(samples_u) = samples.get(&unknown.name) else { continue };
		let mut best: Option<Calibrated> = None;
		for reference in &references {
			let Some(samples_r) = samples.get(&reference.name) else { continue };
			for &form in FORMS {
				let pairs = pair(unknown, reference, form, samples_u, samples_r, limits.tolerance_s);
				if pairs.len() < limits.min_points {
					continue;
				}
				let mut levels: Vec<i64> = pairs.iter().map(|(x, _)| *x as i64).collect();
				levels.sort_unstable();
				levels.dedup();
				if levels.len() < limits.min_levels {
					continue;
				}
				let Some((scale, r2)) = fit_linear(&pairs) else { continue };
				if r2 < limits.min_r2 {
					continue;
				}
				let candidate = Calibrated {
					unknown: unknown.name.clone(),
					reference: reference.name.clone(),
					form,
					scale,
					r2,
					points: pairs.len(),
				};
				if best.as_ref().is_none_or(|b| candidate.r2 > b.r2) {
					best = Some(candidate);
				}
			}
		}
		if let Some(fit) = best {
			out.push(fit);
		}
	}
	Ok(out)
}

/// `vagcan dev recording calibrate` — see the module docs.
/// The identifier a raw column was recorded from.
///
/// `watch --out` heads an unconverted column with the identifier in hex and the
/// `_raw` marker. A column that is not that shape — somebody's own heading, or
/// a converted channel — has no address to write into a catalog, and inventing
/// one would file a proven scaling under an identifier nothing answers.
fn identifier_of(column: &str) -> Option<u16> {
	let name = column.strip_suffix(RAW_SUFFIX).unwrap_or(column);
	(name.len() == 4).then(|| u16::from_str_radix(name, 16).ok())?
}

/// The fits, as the catalog `watch` and `measure` read.
///
/// **Unnamed on purpose.** A fit proves what an identifier's bytes mean, not
/// what the quantity is called; the label files are where names come from, and
/// putting the reference's name on the row would claim the unknown *is* the
/// reference rather than proportional to it. So the row is keyed by the
/// identifier, and naming it is a separate, manual act.
fn to_catalog(fits: &[Calibrated]) -> (vag_data_labels::catalog::MeasurementCatalog, Vec<&str>) {
	use std::borrow::Cow;
	use vag_data_labels::catalog::{MeasurementCatalog, MeasurementDef, ReadId, Scaling};

	let mut rows = Vec::new();
	let mut unaddressed = Vec::new();
	for f in fits {
		match identifier_of(&f.unknown) {
			Some(did) => rows.push(MeasurementDef {
				name: Cow::Owned(format!("{did:04X}")),
				unit: Cow::Borrowed(""),
				address: ReadId::Uds(did),
				raw_form: f.form,
				scaling: Scaling::Linear(f.scale),
			}),
			None => unaddressed.push(f.unknown.as_str()),
		}
	}
	(MeasurementCatalog::new(rows), unaddressed)
}

pub fn run(log: &str, out: Option<&str>, limits: Thresholds) -> anyhow::Result<()> {
	use anyhow::Context as _;

	let csv = std::fs::read_to_string(log).with_context(|| format!("reading the recording {log:?}"))?;
	let fits = calibrate(&csv, limits).map_err(|e| anyhow::anyhow!("{log}: {e}"))?;

	if fits.is_empty() {
		println!(
			"Nothing calibrated (R² ≥ {:.3}, ≥ {} points over ≥ {} distinct raw values).\n\n\
             Either no unknown identifier tracks a known one, or the recording is too \
             short or too steady. A quantity unrelated to everything already proven \
             cannot be found this way — it has nothing to be fitted against.",
			limits.min_r2, limits.min_points, limits.min_levels
		);
		return Ok(());
	}

	println!("Calibrated against references in the same recording:\n");
	for f in &fits {
		println!(
			"  {:>6}  {:?}  × {:.6} {:+.4}   tracks {}   R²={:.5} over {} points",
			f.unknown, f.form, f.scale.factor, f.scale.offset, f.reference, f.r2, f.points
		);
	}
	println!(
		"\nThese are scalings, not names: what a value IS still comes from the label \n\
         label_files, via a VCDS log's IDE/ENG numbers."
	);

	// Without this the path we tell people to walk — survey, drive, calibrate —
	// ends at a list of numbers they have to hand-copy into JSON. `vcds analyse`
	// has had `--out` since it existed; this is the same step for the route
	// that needs no VCDS at all.
	let Some(path) = out else {
		println!(
			"\nTo keep them: re-run with `--out <part-number>.json`, then move that file to\n\
             {} — the unit's own F187 part number is the file name, and\n\
             `vagcan units --identify` reads it off the car.",
			crate::project::measurements_hint()
		);
		return Ok(());
	};
	let (catalog, unaddressed) = to_catalog(&fits);
	std::fs::write(path, catalog.to_json()?).with_context(|| format!("writing the catalog {path:?}"))?;
	println!("\nwrote {} catalog rows to {path}", catalog.len());
	if !unaddressed.is_empty() {
		// A fit against a column whose heading is not an identifier is real,
		// and there is still nowhere to put it: a catalog row is addressed.
		println!(
			"  {} fit(s) left out — {} is not an identifier heading, so there is no \n  \
             address to file the row under.",
			unaddressed.len(),
			unaddressed[0]
		);
	}
	println!(
		"  The rows are keyed by identifier and carry no name: a fit proves what the\n  \
         bytes mean, not what the quantity is called. Name them by hand, or look the\n  \
         wording up with `vagcan dev vcds names <word>`.\n  \
         Put the file in {}/<part number>.json to have `watch` and\n  \
         `measure` use it.",
		crate::project::measurements_hint()
	);
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A recording shaped like `watch --out` writes: a converted reference
	/// column and a raw hex column carrying the same quantity, half-scaled.
	fn recording() -> String {
		let mut csv = String::from("t_s,Engine speed_t_s,Engine speed,206F_raw_t_s,206F_raw\n");
		for i in 0..60 {
			let t = i as f64 * 0.1;
			let rpm = 800.0 + i as f64 * 50.0;
			// The unknown carries the same physical quantity at half a count
			// per rpm, big-endian.
			let raw = (rpm * 2.0) as u16;
			csv.push_str(&format!("{t:.3},{t:.3},{rpm},{t:.3},{:04X}\n", raw));
		}
		csv
	}

	#[test]
	fn an_unknown_that_tracks_a_reference_is_calibrated_against_it() {
		let fits = calibrate(&recording(), Thresholds::default()).unwrap();
		assert_eq!(fits.len(), 1, "{fits:?}");
		let f = &fits[0];
		assert_eq!(f.unknown, "206F_raw");
		assert_eq!(f.reference, "Engine speed");
		assert_eq!(f.form, RawForm::U16Be);
		// raw = rpm * 2, so rpm = raw * 0.5.
		assert!((f.scale.factor - 0.5).abs() < 1e-6, "{:?}", f.scale);
		assert!(f.r2 > 0.9999);
	}

	#[test]
	fn the_marker_decides_which_columns_are_raw_not_the_digits() {
		// `0640` is both a hex pair and the decimal 640. A recording with the
		// marker must be split by name, or an engine speed of 640 rpm would be
		// calibrated against itself as if it were two raw bytes.
		let csv = "t_s,Engine speed,206F_raw\n0.0,640,0640\n0.1,700,0658\n";
		let columns = classify(csv).unwrap();
		let (references, unknowns) = split_columns(&columns);
		assert_eq!(references.iter().map(|c| &c.name).collect::<Vec<_>>(), ["Engine speed"]);
		assert_eq!(unknowns.iter().map(|c| &c.name).collect::<Vec<_>>(), ["206F_raw"]);
	}

	#[test]
	fn an_unmarked_recording_falls_back_to_the_shape_of_the_values() {
		// Recordings written before the marker existed still have to be
		// readable; there the hex shape is all there is to go on.
		let csv = "t_s,Engine speed,206F\n0.0,717.5,0B34\n0.1,800.5,0C40\n";
		let columns = classify(csv).unwrap();
		let (references, unknowns) = split_columns(&columns);
		assert_eq!(references.iter().map(|c| &c.name).collect::<Vec<_>>(), ["Engine speed"]);
		assert_eq!(unknowns.iter().map(|c| &c.name).collect::<Vec<_>>(), ["206F"]);
	}

	#[test]
	fn a_recording_with_no_reference_says_what_is_missing() {
		let csv = "t_s,1234_t_s,1234\n0.0,0.0,0B34\n0.1,0.1,0C40\n";
		let err = calibrate(csv, Thresholds::default()).unwrap_err();
		assert!(err.contains("no reference column"), "{err}");
	}

	#[test]
	fn an_unrelated_unknown_is_not_calibrated() {
		// Noise that varies but tracks nothing. This is the case the guards
		// exist for: a fit here would be invented, not measured.
		let mut csv = String::from("t_s,Engine speed_t_s,Engine speed,9999_raw_t_s,9999_raw\n");
		for i in 0..60 {
			let t = i as f64 * 0.1;
			let rpm = 800.0 + i as f64 * 50.0;
			let noise = ((i * 7919) % 4096) as u16;
			csv.push_str(&format!("{t:.3},{t:.3},{rpm},{t:.3},{noise:04X}\n"));
		}
		assert!(calibrate(&csv, Thresholds::default()).unwrap().is_empty());
	}
}
