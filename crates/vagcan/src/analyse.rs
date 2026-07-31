//! `vagcan analyse` — turn a capture plus a VCDS log into proven scalings.
//!
//! The capture holds what went over the wire; the VCDS CSV holds what VCDS
//! displayed at the same moment. Crossing them gives
//! `(read identifier → raw bytes → engineering value)` directly, without
//! needing the `.rod` field codec that blocks the offline route
//! (`research/rod-labels.md` §4.0c).
//!
//! Two rules keep this honest, because breaking them is exactly how the
//! earlier attempts produced results that later evaporated:
//!
//! 1. **The two clocks are aligned arithmetically**, from the capture's
//!    wall-clock anchor and the log's header time — never by sliding one
//!    series against the other looking for correlation. Fitting the offset is
//!    what produced the phantom matches in §4.0a/§4.0b.
//! 2. **A fit that does not clear the bar is rejected**, and rejection is
//!    reported. A constant series, too few points, or a mediocre `R²` yields
//!    nothing. No forced fits.

use std::collections::BTreeMap;

use vag_capture::{parse_wall_clock_anchor, CapturePayload, CaptureRecord};
use vag_can::sniff::IsoTpSniffer;
use vag_can::{CAN_EFF_FLAG, CAN_EFF_MASK};
use vag_data::measure::{LinearScale, RawForm};

/// Every interpretation of the raw bytes that gets tried.
const FORMS: &[RawForm] =
    &[RawForm::U8First, RawForm::U8Second, RawForm::U16Be, RawForm::U16Le, RawForm::I16Be];

/// One observed response for one identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSample {
    /// Seconds since the capture started.
    pub t_s: f64,
    /// The data bytes, with the identifier echo already stripped.
    pub data: Vec<u8>,
}

/// Everything seen for one read identifier **on one control unit**.
///
/// The unit matters: identifiers are per-ECU, and this car reuses them. `F40D`
/// is one byte of km/h on the engine and two little-endian bytes of a
/// different quantity on the gearbox. Keying series by identifier alone merges
/// the two and invites a fit that is right for neither.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSeries {
    /// CAN id the answers arrived on — identifies the control unit.
    pub ecu: u32,
    pub did: u16,
    pub samples: Vec<RawSample>,
}

/// A scaling that survived the checks.
#[derive(Debug, Clone, PartialEq)]
pub struct Fitted {
    /// CAN id the answers arrived on.
    pub ecu: u32,
    pub did: u16,
    pub form: RawForm,
    pub scale: LinearScale,
    /// Coefficient of determination over the matched points.
    pub r2: f64,
    /// How many capture samples were matched to logged values.
    pub points: usize,
    /// The VCDS measurement it was fitted against.
    pub ide: String,
    pub name: String,
    pub unit: String,
}

/// Why a candidate pairing was not accepted. Reported, not silently dropped —
/// a near miss is a lead, and a rejection is a result.
#[derive(Debug, Clone, PartialEq)]
pub enum Rejected {
    /// Fewer overlapping samples than required.
    TooFewPoints { did: u16, ide: String, points: usize },
    /// The raw bytes never move, so no slope is observable.
    RawConstant { did: u16, ide: String },
    /// The raw bytes take too few distinct values to constrain a line.
    RawTooFewLevels { did: u16, ide: String, levels: usize },
    /// The displayed value never moves.
    ValueConstant { ide: String },
    /// A fit was computed but did not clear the threshold.
    PoorFit { did: u16, ide: String, form: RawForm, r2: f64 },
    /// The fit cleared the bar but something else explained the same
    /// identifier, or the same measurement, better — the signature of two
    /// physically proportional quantities fitting each other.
    Outranked { ecu: u32, did: u16, ide: String, r2: f64 },
}

impl Rejected {
    /// How informative this reason is. Every raw interpretation is tried, and
    /// they fail differently — reading one byte of a two-byte value looks
    /// constant while the pair varies. Reporting whichever failure happened to
    /// come first would make the reason an artefact of the trial order, so the
    /// most informative one is kept.
    fn rank(&self) -> u8 {
        match self {
            Rejected::Outranked { .. } => 5,
            Rejected::PoorFit { .. } => 4,
            Rejected::RawTooFewLevels { .. } => 3,
            Rejected::RawConstant { .. } => 2,
            Rejected::TooFewPoints { .. } => 1,
            Rejected::ValueConstant { .. } => 0,
        }
    }
}

/// Keep `candidate` only if it explains more than what is already held.
fn keep_best_miss(held: &mut Option<Rejected>, candidate: Rejected) {
    let better = match held {
        None => true,
        Some(had) => match (had, &candidate) {
            // Among poor fits, the closest one is the useful lead.
            (Rejected::PoorFit { r2: had_r2, .. }, Rejected::PoorFit { r2, .. }) => r2 > had_r2,
            (had, new) => new.rank() > had.rank(),
        },
    };
    if better {
        *held = Some(candidate);
    }
}

/// Split a positive `ReadDataByIdentifier` response into per-identifier records.
///
/// The server does not state record lengths, so a multi-identifier response is
/// only separable when each requested identifier appears, in the order asked,
/// as a record header. When that does not hold the response is **skipped**
/// rather than guessed at — a mis-split would attribute one measurement's bytes
/// to another identifier, which is worse than having no sample.
pub fn split_records(payload: &[u8], dids: &[u16]) -> Option<Vec<(u16, Vec<u8>)>> {
    let head = |did: u16| [(did >> 8) as u8, (did & 0xFF) as u8];

    if dids.len() == 1 {
        let did = dids[0];
        return payload
            .strip_prefix(&head(did))
            .map(|data| vec![(did, data.to_vec())]);
    }

    let mut out = Vec::with_capacity(dids.len());
    let mut cursor = 0usize;
    for (i, &did) in dids.iter().enumerate() {
        if payload.get(cursor..cursor + 2)? != head(did) {
            return None;
        }
        let body = cursor + 2;
        let end = match dids.get(i + 1) {
            Some(&next) => {
                let next_head = head(next);
                // The next record starts at the next occurrence of its own
                // identifier; anything else means we cannot tell where this
                // record ends.
                payload
                    .windows(2)
                    .enumerate()
                    .skip(body)
                    .find(|(_, w)| *w == next_head)
                    .map(|(at, _)| at)?
            }
            None => payload.len(),
        };
        out.push((did, payload[body..end].to_vec()));
        cursor = end;
    }
    Some(out)
}

/// The id a control unit answers a request on, if the request id is one we
/// recognise.
///
/// This car uses **two** conventions at once, both observed in a real capture:
///
/// - ISO 15765-4 pairs `0x7E0..=0x7E7` with `+8` — engine `0x7E0 → 0x7E8`,
///   gearbox `0x7E1 → 0x7E9`.
/// - VW's own block answers at **`+0x6A`** — instrument cluster
///   `0x714 → 0x77E`, gateway `0x710 → 0x77A`, and likewise `0x70C → 0x776`,
///   `0x70E → 0x778`, `0x74B → 0x7B5`.
///
/// Assuming only the first convention makes every control unit outside the
/// powertrain invisible, which is exactly what happened before this was
/// measured.
///
/// Functional requests (`0x7DF`) are ignored: several units answer at once and
/// the samples could not be attributed.
fn response_id_for(request: u32) -> Option<u32> {
    if request & CAN_EFF_FLAG != 0 {
        // Normal fixed addressing: 0x18DA <target> <source>; the answer swaps
        // the two, so a request to `tt` from the tester `F1` is answered on
        // 0x18DA F1 tt.
        let id = request & CAN_EFF_MASK;
        if id >> 16 != 0x18DA {
            return None;
        }
        let (target, source) = ((id >> 8) & 0xFF, id & 0xFF);
        if source != 0xF1 {
            return None;
        }
        return Some((0x18DA << 16 | source << 8 | target) | CAN_EFF_FLAG);
    }
    match request {
        0x7E0..=0x7E7 => Some(request + 8),
        // The VW block. 0x7DF (functional) is deliberately excluded.
        0x700..=0x7BF if request != 0x7DF => Some(request + 0x6A),
        _ => None,
    }
}

/// Pull per-identifier raw series out of a capture.
///
/// Returns the capture's wall-clock anchor (epoch microseconds) alongside the
/// series, or `None` for the anchor when the capture predates anchors.
pub fn extract_series(records: &[CaptureRecord]) -> (Option<u64>, Vec<RawSeries>) {
    let mut anchor = None;
    let mut sniffer = IsoTpSniffer::new();
    // Identifiers asked for on each request id, awaiting their answer.
    let mut pending: BTreeMap<u32, Vec<u16>> = BTreeMap::new();
    let mut series: BTreeMap<(u32, u16), Vec<RawSample>> = BTreeMap::new();

    for record in records {
        match &record.payload {
            CapturePayload::Marker { note } => {
                if anchor.is_none() {
                    anchor = parse_wall_clock_anchor(note);
                }
            }
            CapturePayload::CanFrame { id, data } => {
                let raw = vag_can::to_raw_id(*id);
                let Some(pdu) = sniffer.observe(raw, data) else {
                    continue;
                };
                match pdu.data.first() {
                    // A request: remember which identifiers it asked for.
                    Some(0x22) => {
                        let Some(response_id) = response_id_for(raw) else {
                            continue;
                        };
                        let dids: Vec<u16> = pdu.data[1..]
                            .chunks_exact(2)
                            .map(|c| u16::from_be_bytes([c[0], c[1]]))
                            .collect();
                        if !dids.is_empty() {
                            pending.insert(response_id, dids);
                        }
                    }
                    // The matching positive response.
                    Some(0x62) => {
                        let Some(dids) = pending.remove(&raw) else {
                            continue;
                        };
                        let Some(records) = split_records(&pdu.data[1..], &dids) else {
                            continue; // ambiguous split — skip rather than guess
                        };
                        let t_s = record.ts_us as f64 / 1e6;
                        for (did, data) in records {
                            series.entry((raw, did)).or_default().push(RawSample { t_s, data });
                        }
                    }
                    _ => {}
                }
            }
            CapturePayload::CableBytes { .. } => {}
        }
    }

    let series = series
        .into_iter()
        .map(|((ecu, did), samples)| RawSeries { ecu, did, samples })
        .collect();
    (anchor, series)
}

/// Least-squares fit of `value = raw * factor + offset`, with `R²`.
///
/// `None` when the raw values do not vary — a vertical fit is not a scaling,
/// it is a coincidence waiting to be believed.
pub fn fit_linear(pairs: &[(f64, f64)]) -> Option<(LinearScale, f64)> {
    let n = pairs.len() as f64;
    if pairs.len() < 2 {
        return None;
    }
    let mean_x = pairs.iter().map(|(x, _)| x).sum::<f64>() / n;
    let mean_y = pairs.iter().map(|(_, y)| y).sum::<f64>() / n;
    let sxx: f64 = pairs.iter().map(|(x, _)| (x - mean_x).powi(2)).sum();
    let sxy: f64 = pairs.iter().map(|(x, y)| (x - mean_x) * (y - mean_y)).sum();
    if sxx <= f64::EPSILON {
        return None;
    }
    let factor = sxy / sxx;
    let mut offset = mean_y - factor * mean_x;
    // A least-squares offset of ~1e-15 is floating-point noise, not a real
    // shift; report the zero it is indistinguishable from. The factor is left
    // exactly as fitted — rounding that to a "nicer" number would be inventing
    // a value rather than measuring one.
    if offset.abs() < 1e-9 * factor.abs().max(1.0) {
        offset = 0.0;
    }

    let ss_tot: f64 = pairs.iter().map(|(_, y)| (y - mean_y).powi(2)).sum();
    if ss_tot <= f64::EPSILON {
        return None; // the displayed value is constant
    }
    let ss_res: f64 =
        pairs.iter().map(|(x, y)| (y - (factor * x + offset)).powi(2)).sum();
    Some((LinearScale { factor, offset }, 1.0 - ss_res / ss_tot))
}

/// Pair capture samples with logged values by nearest timestamp.
///
/// `log_offset_s` is where the log's `t = 0` falls on the capture's clock, and
/// it comes from the two files' wall-clock stamps — it is never searched for.
fn pair_samples(
    series: &RawSeries,
    form: RawForm,
    logged: &[(f64, f64)],
    log_offset_s: f64,
    tolerance_s: f64,
) -> Vec<(f64, f64)> {
    let mut pairs = Vec::new();
    for (t_log, value) in logged {
        let want = t_log + log_offset_s;
        let nearest = series
            .samples
            .iter()
            .min_by(|a, b| {
                (a.t_s - want).abs().partial_cmp(&(b.t_s - want).abs()).unwrap()
            });
        let Some(sample) = nearest else { continue };
        if (sample.t_s - want).abs() > tolerance_s {
            continue;
        }
        let Some(raw) = form.read(&sample.data) else { continue };
        pairs.push((raw as f64, *value));
    }
    pairs
}

/// Thresholds a fit must clear to be reported as proven.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub min_r2: f64,
    pub min_points: usize,
    pub tolerance_s: f64,
    /// How many DISTINCT raw values a fit must be built on.
    ///
    /// Two distinct values define a line exactly, so `R²` is 1.0 by
    /// construction and says nothing. This caught a real false positive on the
    /// first live run: identifier `200C` "fitted" an ignition angle with factor
    /// −0.008824 off two levels. Points are not evidence; levels are.
    pub min_levels: usize,
}

impl Default for Thresholds {
    fn default() -> Self {
        // Deliberately strict. A real linear COMPU method against its own
        // source data fits nearly perfectly; anything looser starts admitting
        // the "|r| ≈ 0.9 at some lag" results that did not survive scrutiny.
        Thresholds { min_r2: 0.995, min_points: 20, tolerance_s: 0.5, min_levels: 4 }
    }
}

/// Try every identifier against every logged measurement.
pub fn fit_all(
    series: &[RawSeries],
    logged: &[crate::vcdslog::LoggedMeasurement],
    log_offset_s: f64,
    limits: Thresholds,
) -> (Vec<Fitted>, Vec<Rejected>) {
    let mut accepted: Vec<Fitted> = Vec::new();
    let mut rejected = Vec::new();

    for measurement in logged {
        if measurement.is_constant() {
            rejected.push(Rejected::ValueConstant { ide: measurement.ide.clone() });
            continue;
        }
        for s in series {
            let mut best: Option<Fitted> = None;
            let mut best_miss: Option<Rejected> = None;

            for &form in FORMS {
                let pairs =
                    pair_samples(s, form, &measurement.samples, log_offset_s, limits.tolerance_s);
                if pairs.len() < limits.min_points {
                    keep_best_miss(
                        &mut best_miss,
                        Rejected::TooFewPoints {
                            did: s.did,
                            ide: measurement.ide.clone(),
                            points: pairs.len(),
                        },
                    );
                    continue;
                }
                let mut levels: Vec<i64> = pairs.iter().map(|(x, _)| *x as i64).collect();
                levels.sort_unstable();
                levels.dedup();
                if levels.len() < limits.min_levels {
                    let why = if levels.len() <= 1 {
                        Rejected::RawConstant { did: s.did, ide: measurement.ide.clone() }
                    } else {
                        Rejected::RawTooFewLevels {
                            did: s.did,
                            ide: measurement.ide.clone(),
                            levels: levels.len(),
                        }
                    };
                    keep_best_miss(&mut best_miss, why);
                    continue;
                }
                let Some((scale, r2)) = fit_linear(&pairs) else {
                    keep_best_miss(
                        &mut best_miss,
                        Rejected::RawConstant { did: s.did, ide: measurement.ide.clone() },
                    );
                    continue;
                };
                if r2 < limits.min_r2 {
                    keep_best_miss(
                        &mut best_miss,
                        Rejected::PoorFit { did: s.did, ide: measurement.ide.clone(), form, r2 },
                    );
                    continue;
                }
                let candidate = Fitted {
                    ecu: s.ecu,
                    did: s.did,
                    form,
                    scale,
                    r2,
                    points: pairs.len(),
                    ide: measurement.ide.clone(),
                    name: measurement.name.clone(),
                    unit: measurement.unit.clone(),
                };
                if best.as_ref().is_none_or(|b| candidate.r2 > b.r2) {
                    best = Some(candidate);
                }
            }

            match best {
                Some(fit) => accepted.push(fit),
                None => {
                    if let Some(miss) = best_miss {
                        rejected.push(miss);
                    }
                }
            }
        }
    }
    // Physically proportional quantities fit each other. On this car vehicle
    // speed and gearbox output-shaft speed are proportional in a fixed gear,
    // so each "fits" the other's identifier — the true pairs come out at
    // R² = 1.00000 and the crossed ones at 0.99915. A quantity cannot be two
    // measurements at once, so a fit is kept only when it is the best
    // explanation for BOTH its identifier and its measurement; the rest are
    // demoted rather than presented as proven.
    let mut ambiguous = Vec::new();
    accepted = {
        let best_for_did = |ecu: u32, did: u16, list: &[Fitted]| -> f64 {
            list.iter()
                .filter(|f| f.ecu == ecu && f.did == did)
                .map(|f| f.r2)
                .fold(f64::MIN, f64::max)
        };
        let best_for_ide = |ide: &str, list: &[Fitted]| -> f64 {
            list.iter().filter(|f| f.ide == ide).map(|f| f.r2).fold(f64::MIN, f64::max)
        };
        let all = accepted.clone();
        let mut kept = Vec::new();
        for fit in accepted {
            let wins_did = fit.r2 >= best_for_did(fit.ecu, fit.did, &all);
            let wins_ide = fit.r2 >= best_for_ide(&fit.ide, &all);
            if wins_did && wins_ide {
                kept.push(fit);
            } else {
                ambiguous.push(fit);
            }
        }
        kept
    };
    for fit in ambiguous {
        rejected.push(Rejected::Outranked {
            ecu: fit.ecu,
            did: fit.did,
            ide: fit.ide,
            r2: fit.r2,
        });
    }

    (accepted, rejected)
}

/// Turn accepted fits into catalog rows.
///
/// The row is named by the measurement's own `IDE` identifier, not by the
/// string VCDS displayed. Two reasons: those strings are Ross-Tech's
/// localised label text, which this project does not reproduce, and the
/// architecture sources names from the label corpus anyway. What the car
/// supplies is the scaling.
pub fn to_catalog(fits: &[Fitted]) -> vag_data::catalog::MeasurementCatalog {
    use std::borrow::Cow;
    use vag_data::catalog::{MeasurementCatalog, MeasurementDef, ReadId, Scaling};

    MeasurementCatalog::new(
        fits.iter()
            .map(|f| MeasurementDef {
                name: Cow::Owned(f.ide.clone()),
                unit: Cow::Owned(f.unit.clone()),
                address: ReadId::Uds(f.did),
                raw_form: f.form,
                scaling: Scaling::Linear(f.scale),
            })
            .collect(),
    )
}

/// Where the log's `t = 0` sits on the capture's clock.
///
/// Both files stamp themselves against the same host clock: the capture with
/// epoch microseconds, the log with a local time of day. Converting the anchor
/// to local time makes the difference a subtraction. Nothing is searched for —
/// that is the whole point.
pub fn log_offset_seconds(anchor_unix_us: u64, log_hms: (u32, u32, u32)) -> f64 {
    use chrono::{Local, TimeZone, Timelike};

    let anchor = Local
        .timestamp_micros(anchor_unix_us as i64)
        .single()
        .unwrap_or_else(Local::now);
    let capture_secs =
        anchor.hour() as f64 * 3600.0 + anchor.minute() as f64 * 60.0 + anchor.second() as f64;
    let log_secs =
        log_hms.0 as f64 * 3600.0 + log_hms.1 as f64 * 60.0 + log_hms.2 as f64;

    // Both are times of day on the same date; a session crossing midnight
    // would need the date, which the log does not state in a parseable form.
    log_secs - capture_secs
}

/// `vagcan analyse` — cross a capture with a VCDS log.
pub fn run(
    capture_path: &str,
    log_path: &str,
    out: Option<&str>,
    limits: Thresholds,
) -> anyhow::Result<()> {
    use anyhow::Context as _;

    let capture = std::fs::File::open(capture_path)
        .with_context(|| format!("opening the capture {capture_path:?}"))?;
    let records = vag_capture::read_records(std::io::BufReader::new(capture))
        .with_context(|| format!("reading the capture {capture_path:?}"))?;
    let (anchor, series) = extract_series(&records);

    let log_bytes = std::fs::read(log_path)
        .with_context(|| format!("reading the VCDS log {log_path:?}"))?;
    let log = crate::vcdslog::parse(&log_bytes).map_err(|e| anyhow::anyhow!("{log_path}: {e}"))?;

    let Some(anchor) = anchor else {
        anyhow::bail!(
            "the capture carries no wall-clock anchor, so it cannot be aligned with the log \
             arithmetically. Re-record it with a current `vagcan sniff`; guessing the offset is \
             what invalidated the earlier analyses."
        );
    };
    let offset = log_offset_seconds(anchor, log.started_hms);

    println!(
        "capture: {} identifiers, {} samples\nlog:     {} measurements from {}",
        series.len(),
        series.iter().map(|s| s.samples.len()).sum::<usize>(),
        log.measurements.len(),
        log.part_number.as_deref().unwrap_or("an unnamed control unit"),
    );
    println!("the log starts {offset:+.1}s into the capture\n");

    if series.is_empty() {
        println!(
            "No read identifiers were recovered from the capture. Either nothing queried the \
             car while it recorded, or the traffic rode identifiers this build does not treat \
             as diagnostic."
        );
        return Ok(());
    }

    let (fits, rejected) = fit_all(&series, &log.measurements, offset, limits);

    if fits.is_empty() {
        println!(
            "Nothing cleared the bar (R² ≥ {:.3}, ≥ {} points over ≥ {} distinct raw values).",
            limits.min_r2, limits.min_points, limits.min_levels
        );
        let mut why: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for r in &rejected {
            *why.entry(match r {
                Rejected::TooFewPoints { .. } => "too few overlapping samples",
                Rejected::RawConstant { .. } => "the raw bytes never moved",
                Rejected::RawTooFewLevels { .. } => "too few distinct raw values",
                Rejected::ValueConstant { .. } => "the logged value never moved",
                Rejected::PoorFit { .. } => "the fit was too poor",
                Rejected::Outranked { .. } => "something explained it better",
            })
            .or_default() += 1;
        }
        println!("\nWhy:");
        for (reason, count) in why {
            println!("  {count:4}  {reason}");
        }
        let best_overlap = rejected
            .iter()
            .filter_map(|r| match r {
                Rejected::TooFewPoints { points, .. } => Some(*points),
                _ => None,
            })
            .max();
        if let Some(points) = best_overlap {
            println!(
                "\nThe best overlap was {points} samples. Either the log covers a shorter window \
                 than the capture, or the two barely overlap — check that both were recorded at \
                 the same time."
            );
        }
    } else {
        println!("Proven scalings:\n");
        for f in &fits {
            println!(
                "  {:03X}/{:04X}  {:?}  × {:.6} {:+.4}   → {} [{}]   R²={:.5} over {} points",
                f.ecu, f.did, f.form, f.scale.factor, f.scale.offset, f.ide, f.unit, f.r2, f.points
            );
        }
    }

    let outranked: Vec<&Rejected> =
        rejected.iter().filter(|r| matches!(r, Rejected::Outranked { .. })).collect();
    if !outranked.is_empty() {
        println!("\nFitted but not kept — a better explanation exists for the same identifier");
        println!("or the same measurement (proportional quantities fit each other):");
        for r in &outranked {
            if let Rejected::Outranked { ecu, did, ide, r2 } = r {
                println!("  {ecu:03X}/{did:04X}  vs {ide}  R²={r2:.5}");
            }
        }
    }

    // Near misses are leads, and a stated rejection beats a silent drop.
    let poor: Vec<&Rejected> = rejected
        .iter()
        .filter(|r| matches!(r, Rejected::PoorFit { r2, .. } if *r2 > 0.8))
        .collect();
    if !poor.is_empty() {
        println!("\nClosest rejected candidates:");
        for r in poor.iter().take(10) {
            if let Rejected::PoorFit { did, ide, form, r2 } = r {
                println!("  {did:04X}  {form:?}  vs {ide}  R²={r2:.3} — below the threshold");
            }
        }
    }

    if let Some(path) = out {
        let catalog = to_catalog(&fits);
        std::fs::write(path, catalog.to_json()?)
            .with_context(|| format!("writing the catalog {path:?}"))?;
        println!("\nwrote {} catalog rows to {path}", catalog.len());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vcdslog::LoggedMeasurement;

    fn series(did: u16, samples: &[(f64, u16)]) -> RawSeries {
        RawSeries {
            ecu: 0x7E8,
            did,
            samples: samples
                .iter()
                .map(|(t, raw)| RawSample { t_s: *t, data: raw.to_be_bytes().to_vec() })
                .collect(),
        }
    }

    fn logged(ide: &str, samples: Vec<(f64, f64)>) -> LoggedMeasurement {
        LoggedMeasurement {
            ide: ide.to_string(),
            name: "Engine speed".to_string(),
            unit: "/min".to_string(),
            samples,
        }
    }

    #[test]
    fn both_addressing_conventions_on_this_car_are_recognised() {
        // Measured from a real capture; assuming only the ISO pairing hides
        // every control unit outside the powertrain.
        assert_eq!(response_id_for(0x7E0), Some(0x7E8), "engine");
        assert_eq!(response_id_for(0x7E1), Some(0x7E9), "gearbox");
        assert_eq!(response_id_for(0x714), Some(0x77E), "instrument cluster");
        assert_eq!(response_id_for(0x710), Some(0x77A), "gateway");
        assert_eq!(response_id_for(0x70C), Some(0x776));
        assert_eq!(response_id_for(0x74B), Some(0x7B5));
        // Functional requests are answered by several units at once.
        assert_eq!(response_id_for(0x7DF), None);
        // Normal fixed addressing swaps target and source.
        assert_eq!(
            response_id_for(0x18DA_10F1 | CAN_EFF_FLAG),
            Some(0x18DA_F110 | CAN_EFF_FLAG)
        );
        // Not a diagnostic id at all.
        assert_eq!(response_id_for(0x0FD), None);
    }

    #[test]
    fn a_single_identifier_response_splits_trivially() {
        assert_eq!(
            split_records(&[0xF1, 0x90, b'X', b'W'], &[0xF190]),
            Some(vec![(0xF190, b"XW".to_vec())])
        );
        // A response echoing a different identifier is not ours.
        assert_eq!(split_records(&[0xF1, 0x91, b'X'], &[0xF190]), None);
    }

    #[test]
    fn a_multi_identifier_response_splits_on_the_requested_order() {
        // Exactly the shape the car returns for a batched read.
        let payload = [0xF1, 0x86, 0x01, 0xF1, 0x87, b'8', b'V', 0xF1, 0x89, b'0', b'5'];
        let split = split_records(&payload, &[0xF186, 0xF187, 0xF189]).unwrap();
        assert_eq!(
            split,
            vec![
                (0xF186, vec![0x01]),
                (0xF187, b"8V".to_vec()),
                (0xF189, b"05".to_vec()),
            ]
        );
    }

    #[test]
    fn an_unsplittable_response_is_skipped_not_guessed() {
        // The unit answered only some of what was asked; without lengths there
        // is no way to attribute bytes, and a wrong attribution is worse than
        // no sample at all.
        let payload = [0xF1, 0x90, b'X', b'W'];
        assert_eq!(split_records(&payload, &[0xF190, 0xF187]), None);
    }

    #[test]
    fn a_clean_linear_relation_is_recovered_exactly() {
        // raw * 0.25 → rpm, the textbook VW scaling.
        let pairs: Vec<(f64, f64)> = (0..50).map(|i| (i as f64 * 100.0, i as f64 * 25.0)).collect();
        let (scale, r2) = fit_linear(&pairs).unwrap();
        assert!((scale.factor - 0.25).abs() < 1e-9, "{scale:?}");
        assert!(scale.offset.abs() < 1e-9);
        assert!(r2 > 0.9999);
    }

    #[test]
    fn a_constant_raw_or_a_constant_value_yields_no_fit() {
        // Both directions of "there is no slope here".
        let flat_raw: Vec<(f64, f64)> = (0..30).map(|i| (100.0, i as f64)).collect();
        assert_eq!(fit_linear(&flat_raw), None);
        let flat_value: Vec<(f64, f64)> = (0..30).map(|i| (i as f64, 7.0)).collect();
        assert_eq!(fit_linear(&flat_value), None);
    }

    #[test]
    fn a_matching_identifier_is_found_and_its_scaling_reported() {
        // A capture where DID 0xF40C carries rpm as u16be * 0.25, sampled at
        // 0.1 s, and a log sampled at 0.5 s over the same window.
        let raw: Vec<(f64, u16)> = (0..200).map(|i| (i as f64 * 0.1, 3200 + i as u16 * 8)).collect();
        let log_samples: Vec<(f64, f64)> = (0..40)
            .map(|i| {
                let t = i as f64 * 0.5;
                let raw = 3200.0 + (t / 0.1) * 8.0;
                (t, raw * 0.25)
            })
            .collect();

        let (fits, _) = fit_all(
            &[series(0xF40C, &raw)],
            &[logged("IDE00405", log_samples)],
            0.0,
            Thresholds::default(),
        );

        assert_eq!(fits.len(), 1, "one identifier, one measurement");
        let fit = &fits[0];
        assert_eq!(fit.did, 0xF40C);
        assert_eq!(fit.form, RawForm::U16Be);
        assert!((fit.scale.factor - 0.25).abs() < 1e-6, "{:?}", fit.scale);
        assert!(fit.r2 > 0.999);
        assert_eq!(fit.unit, "/min");
    }

    #[test]
    fn an_unrelated_identifier_is_rejected_rather_than_fitted() {
        // Noise that happens to drift. This is the case the earlier analyses
        // got wrong, so it is pinned: no fit, and a stated reason.
        let noise: Vec<(f64, u16)> = (0..200)
            .map(|i| (i as f64 * 0.1, ((i * 7919) % 4096) as u16))
            .collect();
        let log_samples: Vec<(f64, f64)> =
            (0..40).map(|i| (i as f64 * 0.5, 800.0 + i as f64 * 75.0)).collect();

        let (fits, rejected) = fit_all(
            &[series(0x7458, &noise)],
            &[logged("IDE00405", log_samples)],
            0.0,
            Thresholds::default(),
        );

        assert!(fits.is_empty(), "no forced fit: {fits:?}");
        assert!(
            matches!(rejected.first(), Some(Rejected::PoorFit { .. })),
            "the rejection states why: {rejected:?}"
        );
    }

    #[test]
    fn a_wrong_clock_offset_produces_nothing_rather_than_a_shifted_fit() {
        // Alignment is arithmetic; if it were searched for, this would be the
        // bug that hides. Feeding a deliberately wrong offset must not yield a
        // "good" fit against a strongly varying signal.
        let raw: Vec<(f64, u16)> =
            (0..200).map(|i| (i as f64 * 0.1, (1000.0 + (i as f64 * 0.3).sin() * 900.0) as u16)).collect();
        let log_samples: Vec<(f64, f64)> = (0..40)
            .map(|i| {
                let t = i as f64 * 0.5;
                (t, 1000.0 + ((t / 0.1) * 0.3).sin() * 900.0)
            })
            .collect();

        let aligned = fit_all(
            &[series(0xF40C, &raw)],
            &[logged("IDE00405", log_samples.clone())],
            0.0,
            Thresholds::default(),
        );
        assert_eq!(aligned.0.len(), 1, "aligned, it fits");

        let skewed = fit_all(
            &[series(0xF40C, &raw)],
            &[logged("IDE00405", log_samples)],
            3.7,
            Thresholds::default(),
        );
        assert!(skewed.0.is_empty(), "misaligned, it must not: {:?}", skewed.0);
    }

    #[test]
    fn two_raw_levels_cannot_prove_a_scaling() {
        // The false positive from the first live run: identifier 200C "fitted"
        // an ignition angle with factor -0.008824 and R² = 1.0, off two raw
        // values. Two points define a line exactly, so a perfect R² there is
        // arithmetic, not evidence.
        // Two raw levels that DO reach the matched points (alternating every
        // other captured sample would alias against the log's 1 s spacing and
        // present a single level, which is a different rejection).
        let two_levels: Vec<(f64, u16)> = (0..60)
            .map(|i| (i as f64 * 0.5, 0x0100 + (i / 2) % 2))
            .collect();
        let log_samples: Vec<(f64, f64)> = (0..30)
            .map(|i| (i as f64, if i % 2 == 0 { 0.0 } else { -2.25 }))
            .collect();

        let (fits, rejected) = fit_all(
            &[series(0x200C, &two_levels)],
            &[logged("IDE00157", log_samples)],
            0.0,
            Thresholds { min_points: 10, ..Default::default() },
        );

        assert!(fits.is_empty(), "two levels are not a proof: {fits:?}");
        assert!(
            matches!(rejected.first(), Some(Rejected::RawTooFewLevels { levels: 2, .. })),
            "and the reason is stated: {rejected:?}"
        );
    }

    #[test]
    fn accepted_fits_become_catalog_rows_the_reader_can_use() {
        let fit = Fitted {
            ecu: 0x7E8,
            did: 0xF40C,
            form: RawForm::U16Be,
            scale: LinearScale { factor: 0.25, offset: 0.0 },
            r2: 0.9999,
            points: 40,
            ide: "IDE00405".to_string(),
            name: "Engine speed".to_string(),
            unit: "/min".to_string(),
        };
        let catalog = to_catalog(&[fit]);
        assert_eq!(catalog.len(), 1);
        // The round trip the product path actually uses.
        let def = &catalog.defs[0];
        assert_eq!(def.interpret(&[0x0B, 0x34]), Some(717.0));
    }
}
