//! `vagcan vcds analyse` — turn a capture plus a VCDS log into proven scalings.
//!
//! The capture holds what went over the wire; the VCDS CSV holds what VCDS
//! displayed at the same moment. Crossing them gives
//! `(read identifier → raw bytes → engineering value)` directly, without
//! needing the `.rod` field codec that blocks the offline route
//! (`research/labels/rod-labels.md` §4.0c).
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

use vag_can::sniff::IsoTpSniffer;
use vag_can::{CAN_EFF_FLAG, CAN_EFF_MASK};
use vag_capture::{CapturePayload, CaptureRecord, parse_wall_clock_anchor};
use vag_data::measure::{LinearScale, RawForm};

/// Every interpretation of the raw bytes that gets tried.
const FORMS: &[RawForm] = &[RawForm::U8First, RawForm::U8Second, RawForm::U16Be, RawForm::U16Le, RawForm::I16Be];

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
/// The server does not state record lengths, so the only thing marking a
/// boundary is the next identifier's own bytes appearing. Everything here
/// follows from that.
///
/// **An identifier that was asked for need not be in the answer.** A control
/// unit answers a multi-identifier request with only the identifiers it
/// supports — measured on the reference car and recorded in `todo/README.md`,
/// and the reason a sweep can group-test at all. This function used to require
/// all of them, so one unsupported identifier in a batch discarded the whole
/// response. That is not a hypothetical: `measure` asks the engine unit for the
/// mass air flow (`F410`, standard PID 10), which the reference car does not
/// implement, and so lost engine speed, both boost channels and the air mass
/// together, on every cycle of every run. Eleven saved sessions carry
/// `engine_speed: 0 points` for that reason and nothing else.
///
/// **What is still refused is a guess.** Among the ways of reading a response,
/// the one accounting for the most identifiers is the better explanation and
/// wins; if two readings account for equally many, the response genuinely reads
/// two ways and `None` is returned. A mis-split attributes one measurement's
/// bytes to another identifier, which is worse than having no sample.
pub fn split_records(payload: &[u8], dids: &[u16]) -> Option<Records> {
	split_within(payload, dids, SPLIT_BUDGET).0
}

/// One response, cut into the identifiers it answered and their bytes.
type Records = Vec<(u16, Vec<u8>)>;

/// The split, and how much of the budget was left when it finished.
///
/// The remainder is what the guard is tested through. Timing the call instead
/// would assert on how loaded the machine is: the same search that takes a
/// millisecond alone takes far longer when the rest of the suite is running
/// beside it, and a test that fails for that reason teaches nothing.
fn split_within(payload: &[u8], dids: &[u16], budget: u32) -> (Option<Records>, u32) {
	let mut best = Best { budget, ..Best::default() };
	parse_from(payload, dids, 0, &mut Vec::new(), &mut best);
	let records = match (best.budget, best.ties) {
		// Out of budget is not "no reading" — it is a response this function
		// did not finish examining, and calling that a unique parse would ship
		// whichever reading it happened to find first.
		(0, _) => None,
		(_, 1) => best.records(payload),
		_ => None,
	};
	(records, best.budget)
}

/// How many placements the split may try before giving up on a response.
///
/// The search is small on anything a control unit actually sends — eight
/// identifiers, a few hundred bytes, and a header occurring in a handful of
/// places. It is not small in the worst case: a payload whose bytes look like
/// identifier headers throughout branches at every position of every level. The
/// caller is a poll loop timing an acceleration run, so an unbounded search is
/// a stall on a moving car; this bounds it at a cost far above any real
/// response and well below anything a driver would notice.
///
/// The number comes from both ends. A real response branches two or three ways
/// per identifier — a header rarely occurs inside another record's data — so
/// eight of them cost a few thousand placements at most. A placement itself is
/// a push of two integers, because the search carries byte *ranges* and only
/// the winning parse is ever copied out; that keeps the ceiling in the
/// low milliseconds, well inside one cycle of the poll loop.
const SPLIT_BUDGET: u32 = 20_000;

/// The best reading of a response so far, and whether anything ties with it.
///
/// Kept as a running maximum rather than a list of every parse: the search is
/// bounded but not tiny, and only the top rank and the size of its tie can
/// change the answer.
#[derive(Default)]
struct Best {
	/// Where the held parse cut the payload, as `(identifier, start, end)`.
	/// Ranges rather than bytes: the search offers a candidate at every leaf it
	/// reaches, and copying each one out made the cost of a placement scale
	/// with the size of the response — which is exactly backwards for a
	/// pathological payload, the case the budget exists to survive.
	parse: Option<Vec<(u16, usize, usize)>>,
	/// How many identifiers the held parse places. A parse placing more
	/// replaces it outright.
	placed: usize,
	/// How many parses place exactly `placed` identifiers. Anything but one
	/// means the response does not read a single way.
	ties: usize,
	/// Placements left before the search gives up. See [`SPLIT_BUDGET`].
	budget: u32,
}

impl Best {
	fn offer(&mut self, parse: &[(u16, usize, usize)]) {
		match parse.len().cmp(&self.placed) {
			std::cmp::Ordering::Greater => {
				self.placed = parse.len();
				self.parse = Some(parse.to_vec());
				self.ties = 1;
			}
			std::cmp::Ordering::Equal => self.ties += 1,
			std::cmp::Ordering::Less => {}
		}
	}

	/// The held parse with its bytes, once the search has finished and there is
	/// exactly one of it.
	fn records(&self, payload: &[u8]) -> Option<Records> {
		Some(
			self
				.parse
				.as_ref()?
				.iter()
				.map(|(did, from, to)| (*did, payload[*from..*to].to_vec()))
				.collect(),
		)
	}
}

/// Enumerate every self-consistent way to cut `payload` into records.
///
/// Two choices are open at each identifier: the unit answered it, and its
/// record starts here; or the unit did not implement it and there is nothing to
/// place. The second is what makes a partial answer readable.
///
/// A record's end is only ever a position where **some still-unplaced
/// identifier's** header sits, or the end of the payload — which is what keeps
/// the search cheap. Trying every byte position instead would be exponential in
/// the payload length rather than in the handful of places a header occurs, and
/// those bytes do occur inside a record's data by coincidence, which
/// little-endian gearbox values do readily.
///
/// At most eight identifiers per request and a few hundred bytes.
fn parse_from(payload: &[u8], dids: &[u16], at: usize, prefix: &mut Vec<(u16, usize, usize)>, best: &mut Best) {
	match best.budget.checked_sub(1) {
		Some(left) => best.budget = left,
		None => return,
	}
	let Some((&did, rest)) = dids.split_first() else {
		// Every identifier accounted for — placed or absent — and the payload
		// fully consumed. A parse that leaves bytes over has mis-read one of
		// the records it did place.
		if at == payload.len() {
			best.offer(prefix);
		}
		return;
	};

	// The unit did not answer this one. Nothing is consumed, and the next
	// identifier is tried at the same position.
	parse_from(payload, rest, at, prefix, best);

	let head = [(did >> 8) as u8, (did & 0xFF) as u8];
	if payload.get(at..at + 2) != Some(&head[..]) {
		return;
	}
	let body = at + 2;

	let heads: Vec<[u8; 2]> = rest.iter().map(|d| [(*d >> 8) as u8, (*d & 0xFF) as u8]).collect();
	let boundaries = (body..payload.len().saturating_sub(1))
		.filter(|i| heads.iter().any(|h| payload[*i..*i + 2] == *h))
		// The record can also be the last one in the response, whether or not
		// it is the last one requested.
		.chain(std::iter::once(payload.len()));
	for end in boundaries {
		prefix.push((did, body, end));
		parse_from(payload, rest, end, prefix, best);
		prefix.pop();
	}
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
						let dids: Vec<u16> = pdu.data[1..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
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

	let series = series.into_iter().map(|((ecu, did), samples)| RawSeries { ecu, did, samples }).collect();
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
	let ss_res: f64 = pairs.iter().map(|(x, y)| (y - (factor * x + offset)).powi(2)).sum();
	Some((LinearScale { factor, offset }, 1.0 - ss_res / ss_tot))
}

/// Pair capture samples with logged values by nearest timestamp.
///
/// `log_offset_s` is where the log's `t = 0` falls on the capture's clock, and
/// it comes from the two files' wall-clock stamps — it is never searched for.
fn pair_samples(series: &RawSeries, form: RawForm, logged: &[(f64, f64)], log_offset_s: f64, tolerance_s: f64) -> Vec<(f64, f64)> {
	let mut pairs = Vec::new();
	// Each captured sample may be used once. Without this, one bus observation
	// pairs with every log point in a window and the point count — which the
	// report presents as evidence — counts the same observation repeatedly.
	let mut used = vec![false; series.samples.len()];
	for (t_log, value) in logged {
		let want = t_log + log_offset_s;
		let nearest = series
			.samples
			.iter()
			.enumerate()
			.filter(|(i, _)| !used[*i])
			.min_by(|(_, a), (_, b)| (a.t_s - want).abs().partial_cmp(&(b.t_s - want).abs()).unwrap());
		let Some((index, sample)) = nearest else { continue };
		if (sample.t_s - want).abs() > tolerance_s {
			continue;
		}
		let Some(raw) = form.read(&sample.data) else { continue };
		used[index] = true;
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
		Thresholds {
			min_r2: 0.995,
			min_points: 20,
			tolerance_s: 0.5,
			min_levels: 4,
		}
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
			rejected.push(Rejected::ValueConstant {
				ide: measurement.ide.clone(),
			});
			continue;
		}
		for s in series {
			let mut best: Option<Fitted> = None;
			let mut best_miss: Option<Rejected> = None;

			for &form in FORMS {
				let pairs = pair_samples(s, form, &measurement.samples, log_offset_s, limits.tolerance_s);
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
						Rejected::RawConstant {
							did: s.did,
							ide: measurement.ide.clone(),
						}
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
						Rejected::RawConstant {
							did: s.did,
							ide: measurement.ide.clone(),
						},
					);
					continue;
				};
				if r2 < limits.min_r2 {
					keep_best_miss(
						&mut best_miss,
						Rejected::PoorFit {
							did: s.did,
							ide: measurement.ide.clone(),
							form,
							r2,
						},
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
		let all = accepted;
		// A fit must be the strictly best explanation for its identifier AND
		// for its measurement, comparing against the OTHER fits by index — an
		// exact tie means two explanations are equally good, which proves
		// neither.
		let mut kept = Vec::new();
		for (i, fit) in all.iter().enumerate() {
			let best_other = |pick: &dyn Fn(&Fitted) -> bool| -> Option<f64> {
				all
					.iter()
					.enumerate()
					.filter(|(j, f)| *j != i && pick(f))
					.map(|(_, f)| f.r2)
					.fold(None, |acc: Option<f64>, r| Some(acc.map_or(r, |a: f64| a.max(r))))
			};
			let same_did = |f: &Fitted| f.ecu == fit.ecu && f.did == fit.did;
			let same_ide = |f: &Fitted| f.ide == fit.ide;
			let wins_did = best_other(&same_did).is_none_or(|best| fit.r2 > best);
			let wins_ide = best_other(&same_ide).is_none_or(|best| fit.r2 > best);
			if wins_did && wins_ide {
				kept.push(fit.clone());
			} else {
				ambiguous.push(fit.clone());
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
/// architecture sources names from the label files anyway. What the car
/// supplies is the scaling.
pub fn to_catalog(fits: &[Fitted]) -> vag_data::catalog::MeasurementCatalog {
	use std::borrow::Cow;
	use vag_data::catalog::{MeasurementCatalog, MeasurementDef, ReadId, Scaling};

	MeasurementCatalog::new(
		fits
			.iter()
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
pub fn log_offset_seconds(anchor_unix_us: u64, log_hms: (u32, u32, u32)) -> Option<f64> {
	use chrono::{Local, TimeZone, Timelike};

	// An anchor that does not map to a local time is a broken capture, not a
	// reason to substitute the current time and print a confident offset.
	let anchor = Local.timestamp_micros(anchor_unix_us as i64).single()?;
	// Keep the sub-second part: the anchor has microsecond precision, and
	// discarding it costs up to a second of skew against a 0.5 s pairing
	// tolerance.
	let capture_secs =
		anchor.hour() as f64 * 3600.0 + anchor.minute() as f64 * 60.0 + anchor.second() as f64 + anchor.timestamp_subsec_micros() as f64 / 1e6;
	// The log header states its start to the second only, so the true start
	// lies somewhere inside that second and the midpoint is the best estimate.
	// Subtracting the capture's exact microsecond time from a truncated log
	// time without this correction biases every pairing by up to a second —
	// measured, not theorised: on the reference session it shifted the gearbox
	// offset by 0.5 s, which flipped half the pairings to the neighbouring
	// sample and collapsed an R² = 1.00000 fit to 0.051.
	const LOG_SECOND_MIDPOINT: f64 = 0.5;
	let log_secs = log_hms.0 as f64 * 3600.0 + log_hms.1 as f64 * 60.0 + log_hms.2 as f64 + LOG_SECOND_MIDPOINT;

	// Both are times of day on the same date; a session crossing midnight
	// would need the date, which the log does not state in a parseable form.
	Some(log_secs - capture_secs)
}

/// `vagcan vcds analyse` — cross a capture with a VCDS log.
pub fn run(capture_path: &str, log_path: &str, out: Option<&str>, limits: Thresholds) -> anyhow::Result<()> {
	use anyhow::Context as _;

	let capture = std::fs::File::open(capture_path).with_context(|| format!("opening the capture {capture_path:?}"))?;
	let records = vag_capture::read_records(std::io::BufReader::new(capture)).map_err(|e| {
		// Three different things in this project are written as `.jsonl`, and
		// a parser's opinion about line 1 column 2 does not tell the user
		// which one they handed over.
		anyhow::anyhow!(
			"{capture_path} is not a `vagcan sniff` capture — that file is JSON lines with a \
             `ts_us` field per frame. A `scan --out` or `survey --out` file has a different \
             shape, and a `watch --out` file is CSV (use `discover` or `calibrate` for that \
             one). Parser said: {e}"
		)
	})?;
	let (anchor, series) = extract_series(&records);

	let log_bytes = std::fs::read(log_path).with_context(|| format!("reading the VCDS log {log_path:?}"))?;
	let log = crate::vcdslog::parse(&log_bytes).map_err(|e| anyhow::anyhow!("{log_path}: {e}"))?;

	let Some(anchor) = anchor else {
		anyhow::bail!(
			"the capture carries no wall-clock anchor, so it cannot be aligned with the log \
             arithmetically. Re-record it with a current `vagcan sniff`; guessing the offset is \
             what invalidated the earlier analyses."
		);
	};
	let offset =
		log_offset_seconds(anchor, log.started_hms).ok_or_else(|| anyhow::anyhow!("the capture's wall-clock anchor is not a valid local time"))?;

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
			*why
				.entry(match r {
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

	// What the log carried but the run could not explain. This is the list
	// that tells you what to record next, so it is printed even on a
	// successful run — a bare list of wins hides the gaps.
	let unmatched: Vec<&crate::vcdslog::LoggedMeasurement> = log.measurements.iter().filter(|m| !fits.iter().any(|f| f.ide == m.ide)).collect();
	if !unmatched.is_empty() {
		println!("\nLogged but not proven:");
		for m in &unmatched {
			let span = m
				.samples
				.iter()
				.map(|(_, v)| *v)
				.fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
			let levels = {
				let mut v: Vec<String> = m.samples.iter().map(|(_, x)| format!("{x:.3}")).collect();
				v.sort();
				v.dedup();
				v.len()
			};
			let why = if levels <= 1 {
				"never moved".to_string()
			} else if levels < limits.min_levels {
				format!("only {levels} distinct values")
			} else {
				// Report the CLOSEST any identifier came, not whichever
				// rejection happened to be recorded first — most identifiers
				// in a capture never overlap this measurement's window at all,
				// and quoting one of those says nothing about the ones that
				// did.
				let best_r2 = rejected
					.iter()
					.filter_map(|r| match r {
						Rejected::PoorFit { ide, r2, .. } if *ide == m.ide => Some(*r2),
						_ => None,
					})
					.fold(None, |acc: Option<f64>, r| Some(acc.map_or(r, |a: f64| a.max(r))));
				let most_points = rejected
					.iter()
					.filter_map(|r| match r {
						Rejected::TooFewPoints { ide, points, .. } if *ide == m.ide => Some(*points),
						_ => None,
					})
					.max();
				match (best_r2, most_points) {
					(Some(r2), _) => format!("best fit only R²={r2:.3}"),
					(None, Some(points)) => {
						format!("no identifier overlapped it by more than {points} samples")
					}
					(None, None) => "no identifier explained it".to_string(),
				}
			};
			println!("  {:<22} {:>9.2}..{:<9.2} [{}]  — {why}", m.ide, span.0, span.1, m.unit);
		}
	}

	let outranked: Vec<&Rejected> = rejected.iter().filter(|r| matches!(r, Rejected::Outranked { .. })).collect();
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
		std::fs::write(path, catalog.to_json()?).with_context(|| format!("writing the catalog {path:?}"))?;
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
				.map(|(t, raw)| RawSample {
					t_s: *t,
					data: raw.to_be_bytes().to_vec(),
				})
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
		assert_eq!(response_id_for(0x18DA_10F1 | CAN_EFF_FLAG), Some(0x18DA_F110 | CAN_EFF_FLAG));
		// Not a diagnostic id at all.
		assert_eq!(response_id_for(0x0FD), None);
	}

	#[test]
	fn a_single_identifier_response_splits_trivially() {
		assert_eq!(split_records(&[0xF1, 0x90, b'X', b'W'], &[0xF190]), Some(vec![(0xF190, b"XW".to_vec())]));
		// A response echoing a different identifier is not ours.
		assert_eq!(split_records(&[0xF1, 0x91, b'X'], &[0xF190]), None);
	}

	#[test]
	fn a_multi_identifier_response_splits_on_the_requested_order() {
		// Exactly the shape the car returns for a batched read.
		let payload = [0xF1, 0x86, 0x01, 0xF1, 0x87, b'8', b'V', 0xF1, 0x89, b'0', b'5'];
		let split = split_records(&payload, &[0xF186, 0xF187, 0xF189]).unwrap();
		assert_eq!(split, vec![(0xF186, vec![0x01]), (0xF187, b"8V".to_vec()), (0xF189, b"05".to_vec()),]);
	}

	#[test]
	fn an_identifier_the_unit_does_not_support_costs_only_itself() {
		// A control unit answers a multi-identifier request with **only the
		// identifiers it supports** — established on this car and recorded in
		// `todo/README.md`: asking for `F190` together with something the unit
		// does not implement returns just `F190`.
		//
		// This used to return `None`, which threw the whole response away. It
		// cost every telemetry channel on the engine unit for eleven recorded
		// sessions: `measure` asks it for the mass air flow (`F410`, standard
		// PID 10) which this car does not implement, so all five identifiers in
		// that batch — engine speed included — were discarded every cycle, and
		// every saved run carries `engine_speed: 0 points`.
		let payload = [0xF1, 0x90, b'X', b'W'];
		assert_eq!(
			split_records(&payload, &[0xF190, 0xF187]),
			Some(vec![(0xF190, b"XW".to_vec())]),
			"the identifier that was answered is still readable"
		);

		// The omission can fall anywhere in the request, including first.
		let payload = [0xF1, 0x87, b'8', b'V'];
		assert_eq!(split_records(&payload, &[0xF190, 0xF187]), Some(vec![(0xF187, b"8V".to_vec())]));

		// And in the middle of three, which is the shape that loses a whole
		// batch: the two that answered still split on the requested order.
		let payload = [0xF1, 0x86, 0x01, 0xF1, 0x89, b'0', b'5'];
		assert_eq!(
			split_records(&payload, &[0xF186, 0xF187, 0xF189]),
			Some(vec![(0xF186, vec![0x01]), (0xF189, b"05".to_vec())])
		);
	}

	#[test]
	fn the_engine_batch_that_measure_actually_sends_splits() {
		// The exact request `measure` makes of the engine unit on the reference
		// car: boost specified, boost actual, engine speed, road speed, and the
		// mass air flow — standard PID 10, which this car does not implement
		// and never answers. Everything but the last is in the response.
		//
		// Values from a saved session: 1.15 bar specified as 0x047E, 1.13 bar
		// actual as 0x0468, 4284 /min as 0x10BC, 87 km/h as 0x57.
		let dids = [0x2029, 0x202A, 0x206E, 0xF40D, 0xF410];
		let payload = [
			0x20, 0x29, 0x04, 0x7E, //
			0x20, 0x2A, 0x04, 0x68, //
			0x20, 0x6E, 0x10, 0xBC, //
			0xF4, 0x0D, 0x57,
		];
		assert_eq!(
			split_records(&payload, &dids),
			Some(vec![
				(0x2029, vec![0x04, 0x7E]),
				(0x202A, vec![0x04, 0x68]),
				(0x206E, vec![0x10, 0xBC]),
				(0xF40D, vec![0x57]),
			]),
			"this is the read that produced `engine_speed: 0 points` in eleven sessions"
		);
	}

	#[test]
	fn a_response_that_genuinely_reads_two_ways_is_still_refused() {
		// Tolerating an unanswered identifier must not become tolerating a
		// guess. Here `F187`'s header occurs twice, so there are two ways to
		// place it and both place the same number of identifiers — nothing
		// distinguishes them, and a mis-split attributes one measurement's
		// bytes to another.
		let payload = [0xF1, 0x86, 0xF1, 0x87, 0x01, 0xF1, 0x87, 0x02];
		assert_eq!(split_records(&payload, &[0xF186, 0xF187]), None);
	}

	#[test]
	fn a_payload_built_to_explode_the_search_returns_rather_than_hangs() {
		// Every byte pair in this payload is one of the requested headers, so
		// every position of every level branches. The caller is the poll loop
		// of a stopwatch running on a moving car: the guarantee that matters is
		// that this returns, and that it does not call whatever it found first
		// a unique reading.
		let dids: Vec<u16> = (0..8).map(|i| 0xF100 + i).collect();
		let payload: Vec<u8> = (0..300).flat_map(|i| [0xF1u8, (i % 8) as u8]).collect();
		let (records, left) = split_within(&payload, &dids, SPLIT_BUDGET);
		assert_eq!(left, 0, "the search finished, so the guard was never the reason it stopped");
		assert_eq!(records, None, "and giving up reports no reading, not the first one found");

		// The same payload with a budget it can afford returns the same answer
		// by exhausting the search rather than by running out — the two paths
		// to `None` are different and only one of them is the guard.
		let short: Vec<u16> = dids[..2].to_vec();
		let (_, left) = split_within(&payload[..8], &short, SPLIT_BUDGET);
		assert!(left > 0, "a real response must never come near the ceiling");
	}

	#[test]
	fn the_parse_that_places_the_most_identifiers_wins() {
		// `F187`'s header also appears inside `F186`'s data, so the response
		// can be read as one record or as two. Reading it as two accounts for
		// an identifier that was asked for and is therefore the better
		// explanation — and it is the reading this code has always taken. The
		// ranking exists to keep it that way now that a short parse is legal
		// at all.
		let payload = [0xF1, 0x86, 0xF1, 0x87, 0x01];
		assert_eq!(
			split_records(&payload, &[0xF186, 0xF187]),
			Some(vec![(0xF186, vec![]), (0xF187, vec![0x01])])
		);
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

		let (fits, _) = fit_all(&[series(0xF40C, &raw)], &[logged("IDE00405", log_samples)], 0.0, Thresholds::default());

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
		let noise: Vec<(f64, u16)> = (0..200).map(|i| (i as f64 * 0.1, ((i * 7919) % 4096) as u16)).collect();
		let log_samples: Vec<(f64, f64)> = (0..40).map(|i| (i as f64 * 0.5, 800.0 + i as f64 * 75.0)).collect();

		let (fits, rejected) = fit_all(&[series(0x7458, &noise)], &[logged("IDE00405", log_samples)], 0.0, Thresholds::default());

		assert!(fits.is_empty(), "no forced fit: {fits:?}");
		assert!(
			matches!(rejected.first(), Some(Rejected::PoorFit { .. })),
			"the rejection states why: {rejected:?}"
		);
	}

	#[test]
	fn the_offset_accounts_for_the_logs_whole_second_granularity() {
		// The capture anchor is exact to the microsecond; the log header is
		// truncated to the second. Subtracting one from the other directly
		// biases every pairing, and on the reference session that bias was
		// enough to turn an R² = 1.00000 gearbox fit into 0.051 — half the log
		// points matched the neighbouring capture sample instead of their own.
		// A capture starting exactly on the second, and a log header one
		// minute later, must give 60.5 s: the log's true start is somewhere
		// inside its stated second, and the midpoint is the best estimate.
		let on_the_second = 1_785_536_240_000_000u64;
		let capture_hms = {
			use chrono::{Local, TimeZone, Timelike};
			let t = Local.timestamp_micros(on_the_second as i64).single().unwrap();
			(t.hour(), t.minute(), t.second())
		};
		let one_minute_later = (capture_hms.0, capture_hms.1 + 1, capture_hms.2);
		let offset = log_offset_seconds(on_the_second, one_minute_later).unwrap();
		assert!((offset - 60.5).abs() < 1e-9, "got {offset}");

		// Half a second into the capture's second, the same log header is that
		// much closer.
		let half_past = on_the_second + 500_000;
		let offset = log_offset_seconds(half_past, one_minute_later).unwrap();
		assert!((offset - 60.0).abs() < 1e-9, "got {offset}");
	}

	#[test]
	fn a_wrong_clock_offset_produces_nothing_rather_than_a_shifted_fit() {
		// Alignment is arithmetic; if it were searched for, this would be the
		// bug that hides. Feeding a deliberately wrong offset must not yield a
		// "good" fit against a strongly varying signal.
		let raw: Vec<(f64, u16)> = (0..200)
			.map(|i| (i as f64 * 0.1, (1000.0 + (i as f64 * 0.3).sin() * 900.0) as u16))
			.collect();
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

		let skewed = fit_all(&[series(0xF40C, &raw)], &[logged("IDE00405", log_samples)], 3.7, Thresholds::default());
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
		let two_levels: Vec<(f64, u16)> = (0..60).map(|i| (i as f64 * 0.5, 0x0100 + (i / 2) % 2)).collect();
		let log_samples: Vec<(f64, f64)> = (0..30).map(|i| (i as f64, if i % 2 == 0 { 0.0 } else { -2.25 })).collect();

		let (fits, rejected) = fit_all(
			&[series(0x200C, &two_levels)],
			&[logged("IDE00157", log_samples)],
			0.0,
			Thresholds {
				min_points: 10,
				..Default::default()
			},
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
