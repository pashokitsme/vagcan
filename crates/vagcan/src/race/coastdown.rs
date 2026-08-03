//! The coastdown: the one measurement in this tool that puts a number on the
//! road rather than reading one off the bus.
//!
//! Coasting in neutral, the drivetrain is disconnected and only two forces are
//! left:
//!
//! ```text
//! m·(1 + δ₁)·(−dv/dt)  =  ½·ρ·CdA·v²  +  m·g·Crr
//! ```
//!
//! `1 + δ₁` and not `1 + δ₁ + δ₂`, because the wheels turn with the car and the
//! engine does not — and it is the **same** `δ₁` the power model carries, taken
//! from [`super::power::deltas`], never written down here as a second literal.
//! An earlier draft that said 1.03 in one place and 1.04 in the other biased
//! both coefficients 1.4 % low, since they scale together with it.
//!
//! **The fit is on `v(t)`, in closed form, and never on force against `v²`.**
//! The equation integrates to
//!
//! ```text
//! v(t) = v_c·tan( arctan(v₀/v_c) − t/τ )    v_c = √(B/A)   τ = m(1+δ₁)/√(A·B)
//! ```
//!
//! with `A = ½ρ·CdA` and `B = m·g·Crr`, so three parameters fitted straight
//! against the raw speed samples give both coefficients with **no
//! differentiation anywhere**. That is the whole reason it exists. The force
//! form has to differentiate, differentiating needs a smoothing window, and the
//! window is not the run's 0.3 s: at 0.3 s the force fit reaches only R² ≈ 0.93
//! and would reject every valid coastdown. The closed form has no window to
//! argue about, is better conditioned by an order of magnitude, and gives
//! residuals in km/h — a unit a person can judge.
//!
//! **`Crr` as fitted is the whole speed-independent road load, not a tyre
//! property.** Wheel bearings, seals, brake-pad rub and gearbox oil churning
//! are all roughly speed-independent over this range and land in the same
//! constant. A realistic 15 N of that is +10 % on a `Crr` of 0.0114. It is the
//! right number for the power model — it is what actually resists the car — and
//! the wrong number to compare against a tyre catalogue, or to replace with one
//! out of a datasheet.
//!
//! **What R² cannot do, and what the two passes can.** A constant road gradient
//! adds a constant force, which this model absorbs *entirely* into `B`. The
//! curve still fits exactly; only the constant moves. One per cent of grade
//! nearly doubles `Crr` and leaves the residual where it was, to the last
//! decimal — so no residual bar, however tight, can see a slope. The only
//! detector there is is the disagreement between two reciprocal passes, which
//! is why [`combine`] takes two and why one alone is [`Reject::OnlyOnePass`].
//!
//! Nothing here knows anything about one car: the mass, the density, `δ₁` and
//! the speed samples all arrive as arguments.

use super::power::{self, G, KMH_PER_MS};
use super::types::{Seconds, Track};

/// How far the residual may sit from the curve before the pass is not a coast.
///
/// One km/h is one count of the coarsest speed channel this tool would ever
/// lead with, so below it the residual is the channel's own quantisation rather
/// than the car's behaviour. Above it something happened that the two-force
/// model does not describe — a touch of brake, a gust, a hill that is not
/// constant — and the honest answer is no road load rather than a fitted one.
///
/// **This is deliberately not [`crate::analyse::Thresholds::min_r2`].** That bar
/// is stated in R² and belongs to proving a linear scaling against its own
/// source data, where a real COMPU method fits nearly perfectly. A coastdown's
/// R² is 0.999 on a road with a hill in it, so an R² bar here would pass exactly
/// the pass that must be caught. The unit had to change with the question.
pub const MAX_RESIDUAL_KMH: f64 = 1.0;

/// How far two reciprocal passes may disagree on `Crr`, as a percentage of
/// twice their mean, before the pair is not two measurements of one road.
///
/// The disagreement is *expected* to be large and is not by itself a defect: a
/// grade cancels exactly between the passes, so a 1 % slope shows up here as
/// some 88 % and still produces the right answer, plus a slope worth telling the
/// driver about. Rejecting at that point would throw away a good measurement
/// because the road it was taken on was ordinary.
///
/// What the bar means instead is that **both** directions must have measured a
/// road load, with something left over. The statistic saturates at 100 % by
/// construction — there the downhill pass has fitted a `Crr` of exactly zero,
/// and past it the closed form has no real `v_c` at all — so 95 % is "the
/// downhill direction still has a twentieth of the mean road load in it". Below
/// that the difference is no longer two numbers being compared, it is one number
/// and a rounding error, and the rejection carries the implied grade so it says
/// which way the road slopes.
///
/// For scale: the statistical scatter on `Crr` over a full 120→40 pass is
/// 0.16 %, some three orders of magnitude below this. Nothing about this bar is
/// limited by noise.
pub const MAX_CRR_DISAGREEMENT_PERCENT: f64 = 95.0;

/// The narrowest speed span the three parameters can be told apart across.
///
/// Not a rule about roads — `--coast-from`/`--coast-to` set the actual range,
/// and narrowing it for a road that does not allow 120 km/h is expected. It is a
/// statement about the arithmetic: the correlation between the two coefficients
/// is −0.86 over 120→40 and −0.97 over 120→80, and it goes to −1 as the span
/// closes. Under ten km/h the fit is reporting the seed back.
pub const MIN_SPAN_KMH: f64 = 10.0;

/// A pedal reading at or under this counts as "foot off", in per cent.
///
/// A potentiometric pedal at rest reads a fraction of a per cent on any car —
/// the sensor's own zero, not a request for torque. One per cent is above every
/// resting value and below anything a foot does deliberately.
const PEDAL_ZERO_PERCENT: f64 = 1.0;

/// How far the speed may tick back up before the coast is judged to have ended.
///
/// A speed channel quantised at 1 km/h can gain a whole count on rounding
/// alone, so anything at or below one count has to be tolerated or a clean coast
/// is discarded by its own arithmetic. Real acceleration accumulates and passes
/// this within a sample or two.
const RISE_TOLERANCE_KMH: f64 = 1.0;

/// The window the deceleration is measured over, for the braking check.
///
/// Long enough that a quantised speed channel gives a usable slope — at a
/// coast's 0.3 m/s² a second of it is over a km/h — and short enough that a
/// brake application is caught while the pass can still be thrown away.
const BRAKE_WINDOW_S: f64 = 1.0;

/// How many times its own opening deceleration a coast may decelerate before it
/// is braking.
///
/// A coast's deceleration only ever *falls* through a pass, because drag falls
/// with `v²`, so the first window is the largest one a clean pass can produce
/// and every later window has headroom against it. Braking is five to twenty
/// times a coast; two and a half sits clear of both the noise and the event.
const BRAKING_STEP_FACTOR: f64 = 2.5;

/// One accepted coast: speed against time, and the conditions it happened in.
///
/// `speed` is **m/s** on the leading channel's own timestamps, like every other
/// number in `race` that is not being shown to someone. `rho` and `mass_kg` are
/// not optional decoration: the fit returns `½ρ·CdA`, so a `CdA` without the `ρ`
/// that was in the air is meaningless, and it scales with the mass used in the
/// fit — which is that day's load, not the run's. Both go in the car file
/// alongside the answer.
#[derive(Clone, Debug, PartialEq)]
pub struct Pass {
    pub speed: Track,
    pub rho: f64,
    pub mass_kg: f64,
}

/// What one pass measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    pub cda: f64,
    pub crr: f64,
    /// Root-mean-square distance from the fitted curve, in km/h. In km/h and
    /// not as an R² because a person can look at 0.2 km/h and know whether it is
    /// a coast; nobody can do that with 0.9993.
    pub rms_kmh: f64,
    /// The mean speed the pass happened at, in m/s.
    ///
    /// Carried because [`combine`] needs it and cannot recover it: the wind
    /// estimate is a *relative* spread in `CdA` and has to be multiplied by a
    /// speed to become a speed. The plan's `Fit` did not have this field and the
    /// wind figure it also asks for cannot be produced without it.
    pub mean_speed_ms: f64,
}

/// Why a coastdown produced no road load.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reject {
    /// One pass is not a coastdown. A single direction cannot tell a slope from
    /// a rolling resistance, and on any real road it will not be flat.
    OnlyOnePass,
    /// The two passes do not describe one road.
    Disagreement {
        crr_percent: f64,
        limit: f64,
        implied_grade_percent: f64,
        implied_wind_ms: f64,
    },
    /// The samples are not on the curve, so whatever happened was not a free
    /// coast.
    Residual { rms_kmh: f64, limit: f64 },
    /// Too little speed range to separate the drag term from the constant one.
    TooNarrow { span_kmh: f64 },
    /// The air density or the mass was never stated.
    ///
    /// Not a property of the drive. The detector watches speed and time and
    /// nothing else; the barometer, the ambient sensor and the car file are the
    /// caller's, and a fit run without them would be a number with no units
    /// behind it rather than a wrong one.
    ConditionsUnstated,
}

/// The road load two reciprocal passes agree on, and the two things their
/// disagreement reveals about the road they were driven on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadLoadResult {
    pub cda: f64,
    pub crr: f64,
    /// The slope the two passes imply, positive when the **first** pass ran
    /// uphill. Reported rather than merely used: it costs nothing, it is the
    /// only slope figure anything here can produce, and it tells the driver
    /// something true about the road they chose.
    pub implied_grade_percent: f64,
    /// The wind the two passes imply, positive when the **first** pass ran into
    /// it. Also reported rather than used — see [`combine`] for why it cannot
    /// simply be subtracted.
    pub implied_wind_ms: f64,
}

/// Fit one pass and turn the curve back into `CdA` and `Crr`.
///
/// `delta1` is `I_wheels/(m·r²)` for this car and comes from
/// [`super::power::deltas`]. It is an argument rather than a constant for the
/// same reason it is there: the textbook 0.04 is a typical car's wheels divided
/// by someone else's mass, and it moves by nearly a factor of two across the
/// cars this tool serves.
///
/// The seed comes from the force form through [`crate::analyse::fit_linear`] —
/// a crude difference quotient over a one-second stride, which is exactly the
/// differentiation this module refuses to build an answer on. As a *starting
/// point* it costs nothing to be wrong: Gauss-Newton on the closed form is what
/// produces the number, and `fit_linear`'s two `None` paths are precisely the
/// "no speed range" case, so the seed doubles as the degeneracy check.
pub fn fit(pass: &Pass, delta1: f64) -> Result<Fit, Reject> {
    let stated = |x: f64| x.is_finite() && x > 0.0;
    if !(stated(pass.rho) && stated(pass.mass_kg)) {
        return Err(Reject::ConditionsUnstated);
    }

    let (t, v) = (&pass.speed.t, &pass.speed.v);
    let span_kmh = span_kmh(&pass.speed);
    if pass.speed.len() < 4 || span_kmh < MIN_SPAN_KMH {
        return Err(Reject::TooNarrow { span_kmh });
    }

    // Everything is measured from the pass's own zero, so the fitted `v₀` is the
    // speed at the first sample rather than at some arbitrary session clock.
    let t0 = t[0];
    let times: Vec<f64> = t.iter().map(|x| x - t0).collect();

    // `m·(1 + δ₁)` — the inertia the road actually has to slow down.
    let inertial_mass = pass.mass_kg * (1.0 + delta1);

    let seed = seed(&times, v, inertial_mass).ok_or(Reject::TooNarrow { span_kmh })?;
    let curve = gauss_newton(seed, &times, v);

    // A = ½ρ·CdA and B = m·g·Crr, read back out of v_c = √(B/A) and
    // τ = m(1+δ₁)/√(A·B): A = m(1+δ₁)/(τ·v_c) and B = v_c·m(1+δ₁)/τ.
    let a = inertial_mass / (curve.tau * curve.vc);
    let b = curve.vc * inertial_mass / curve.tau;
    let cda = 2.0 * a / pass.rho;
    let crr = b / (pass.mass_kg * G);
    if !cda.is_finite() || cda <= 0.0 || !crr.is_finite() {
        return Err(Reject::TooNarrow { span_kmh });
    }

    let rms_kmh = rms(&curve, &times, v) * KMH_PER_MS;
    if !rms_kmh.is_finite() || rms_kmh > MAX_RESIDUAL_KMH {
        return Err(Reject::Residual { rms_kmh, limit: MAX_RESIDUAL_KMH });
    }

    let mean_speed_ms = v.iter().sum::<f64>() / v.len() as f64;
    Ok(Fit { cda, crr, rms_kmh, mean_speed_ms })
}

/// Two reciprocal passes, and everything their difference says.
///
/// **A grade cancels exactly and the wind does not**, and that asymmetry is the
/// whole content of this function. Gravity's component along the road is
/// `±m·g·sin θ`, a constant that lands wholly in `B` with the sign of the
/// direction, so averaging the two passes removes it and differencing them
/// measures it. Drag acts on `(v ± w)²`: the cross term `2vw` is antisymmetric
/// and cancels too, but the `w²` term is *symmetric*, is a constant, and is
/// always positive — so a wind inflates both passes' `Crr` in the same
/// direction, survives the averaging untouched, and adds
/// `ΔCrr = ½ρ·CdA·w²/(m·g)`, about +6 % at 5 m/s and +1.5 % at 2.5 m/s.
///
/// That is why the wind is *reported* rather than subtracted. The estimate comes
/// from the antisymmetric part, which is the well-measured half; using it to
/// correct the symmetric part would be squaring a difference of two fitted
/// numbers and calling the result a measurement. What it is good for is telling
/// someone their `Crr` reads a couple of per cent high and the road was too
/// windy — which is a criterion they can act on and a correction they cannot
/// check.
///
/// `delta1` is accepted for symmetry with [`fit`] and is deliberately unused;
/// see [`RoadLoadResult::implied_grade_percent`] and the note in the source.
pub fn combine(
    a: &Fit,
    b: &Fit,
    _mass_kg: f64,
    _rho: f64,
    _delta1: f64,
) -> Result<RoadLoadResult, Reject> {
    let cda = 0.5 * (a.cda + b.cda);
    let crr = 0.5 * (a.crr + b.crr);

    // sin θ = (Crr_A − Crr_B)/2, with no (1 + δ₁) on it.
    //
    // The design writes this factor in, and it does not belong to a `Crr`
    // recovered the way `fit` recovers it. The grade force is `m·g·sin θ` and it
    // enters `B` beside `m·g·Crr`, so the two are directly comparable and the
    // half-difference is the sine outright. The factor is what an implementation
    // would need if it had divided the fitted constant by `m` instead of by
    // `m·(1 + δ₁)` — that `Crr` comes out `1/(1 + δ₁)` small, and the factor puts
    // it back. `fit` divides by the right thing, so applying it here would
    // overstate every slope by δ₁, about 4 %.
    let sin_theta = 0.5 * (a.crr - b.crr);
    let implied_grade_percent = power::grade_percent(sin_theta);

    // w ≈ (CdA_A − CdA_B)/(CdA_A + CdA_B)·v̄. Near the mean speed the wind's
    // effect on drag is `(v + w)² ≈ v²(1 + 2w/v̄)`, which the fit reads as a
    // `CdA` scaled by that factor; the relative spread between the two passes is
    // therefore ≈ 2w/v̄, and the missing half of it goes into the constant term
    // instead, which is what leaves the ratio at w/v̄ rather than 2w/v̄.
    let mean_speed_ms = 0.5 * (a.mean_speed_ms + b.mean_speed_ms);
    let cda_sum = a.cda + b.cda;
    let implied_wind_ms =
        if cda_sum > 0.0 { (a.cda - b.cda) / cda_sum * mean_speed_ms } else { 0.0 };

    let crr_sum = a.crr + b.crr;
    let crr_percent =
        if crr_sum > 0.0 { 100.0 * (a.crr - b.crr).abs() / crr_sum } else { f64::INFINITY };
    if !crr_percent.is_finite() || crr_percent > MAX_CRR_DISAGREEMENT_PERCENT {
        return Err(Reject::Disagreement {
            crr_percent,
            limit: MAX_CRR_DISAGREEMENT_PERCENT,
            implied_grade_percent,
            implied_wind_ms,
        });
    }

    Ok(RoadLoadResult { cda, crr, implied_grade_percent, implied_wind_ms })
}

/// The caller's entry: whatever passes the session collected, into one answer.
///
/// It exists because [`combine`] takes exactly two and therefore cannot say
/// [`Reject::OnlyOnePass`], which is the commonest way a coastdown ends —
/// traffic arrives, or the road runs out, and the driver has one direction and
/// no other. One pass is not half a measurement; it is a rolling resistance and
/// a slope added together with no way to tell which is which.
pub fn road_load(
    passes: &[Fit],
    mass_kg: f64,
    rho: f64,
    delta1: f64,
) -> Result<RoadLoadResult, Reject> {
    match passes {
        [a, b, ..] => combine(a, b, mass_kg, rho, delta1),
        _ => Err(Reject::OnlyOnePass),
    }
}

// ---------------------------------------------------------------------------
// The curve, and the three parameters behind it
// ---------------------------------------------------------------------------

/// How close the phase may come to π/2 before `tan` is meaningless. A guard on
/// the arithmetic, not on any car: `v` is infinite at π/2.
const PHASE_GUARD: f64 = 1e-6;

/// `v(t) = v_c·tan( arctan(v₀/v_c) − t/τ )`, the coastdown's closed form.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Curve {
    v0: f64,
    vc: f64,
    tau: f64,
}

impl Curve {
    fn phase(&self, t: f64) -> Option<f64> {
        let u = (self.v0 / self.vc).atan() - t / self.tau;
        let limit = std::f64::consts::FRAC_PI_2 - PHASE_GUARD;
        (u > 0.0 && u < limit).then_some(u)
    }

    fn at(&self, t: f64) -> Option<f64> {
        Some(self.vc * self.phase(t)?.tan())
    }

    /// `∂v/∂v₀`, `∂v/∂v_c`, `∂v/∂τ`, written out rather than differenced.
    fn jacobian(&self, t: f64) -> Option<[f64; 3]> {
        let u = self.phase(t)?;
        let tan = u.tan();
        // sec²u, from the identity rather than from a division by cos².
        let sec2 = 1.0 + tan * tan;
        let hypot2 = self.vc * self.vc + self.v0 * self.v0;
        Some([
            self.vc * self.vc * sec2 / hypot2,
            tan - self.vc * self.v0 * sec2 / hypot2,
            self.vc * sec2 * t / (self.tau * self.tau),
        ])
    }

    fn valid(&self) -> bool {
        self.vc.is_finite()
            && self.vc > 0.0
            && self.tau.is_finite()
            && self.tau > 0.0
            && self.v0.is_finite()
            && self.v0 > 0.0
    }
}

fn span_kmh(speed: &Track) -> f64 {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for v in &speed.v {
        lo = lo.min(*v);
        hi = hi.max(*v);
    }
    if lo.is_finite() && hi.is_finite() { (hi - lo) * KMH_PER_MS } else { 0.0 }
}

fn sum_squares(curve: &Curve, times: &[f64], v: &[f64]) -> f64 {
    let mut total = 0.0;
    for (t, measured) in times.iter().zip(v) {
        let Some(modelled) = curve.at(*t) else { return f64::INFINITY };
        total += (modelled - measured).powi(2);
    }
    total
}

fn rms(curve: &Curve, times: &[f64], v: &[f64]) -> f64 {
    (sum_squares(curve, times, v) / v.len() as f64).sqrt()
}

/// A starting point for the three parameters, from the force form.
///
/// Pairs of samples a second or so apart give `(v², m(1+δ₁)·(−dv/dt))`, and
/// [`crate::analyse::fit_linear`] turns those into `A` and `B` directly. It is
/// the differentiation this module exists to avoid, which is why its answer is
/// only ever a seed — and why its failure to fit at all is the honest signal
/// that the pass had no speed range in it.
fn seed(times: &[f64], v: &[f64], inertial_mass: f64) -> Option<Curve> {
    let mut pairs = Vec::new();
    let mut i = 0;
    for j in 1..times.len() {
        let dt = times[j] - times[i];
        if dt < BRAKE_WINDOW_S {
            continue;
        }
        let mid = 0.5 * (v[i] + v[j]);
        pairs.push((mid * mid, inertial_mass * (v[i] - v[j]) / dt));
        i = j;
    }
    let linear = crate::analyse::fit_linear(&pairs);

    let v0 = *v.first()?;
    let (vc, tau) = match linear {
        // A = factor, B = offset. Both have to be positive for the closed form
        // to have a real v_c at all; a pass that produces anything else was not
        // a free coast and the crude seed below at least lets the real fit say
        // so in its own units.
        Some((scale, _r2)) if scale.factor > 0.0 && scale.offset > 0.0 => {
            let (a, b) = (scale.factor, scale.offset);
            ((b / a).sqrt(), inertial_mass / (a * b).sqrt())
        }
        Some(_) => fallback_seed(times, v)?,
        // No variation in v² — there is no coastdown here to fit.
        None => return None,
    };

    let curve = Curve { v0, vc, tau };
    curve.valid().then_some(curve)
}

/// A seed from the endpoints alone, for a pass whose difference quotients do
/// not even have the right sign.
///
/// `v_c` is taken as the pass's own mean speed — the scale on which the two
/// terms are comparable, by construction of the range — and `τ` is then whatever
/// makes the curve pass through both ends.
fn fallback_seed(times: &[f64], v: &[f64]) -> Option<(f64, f64)> {
    let (first, last) = (*v.first()?, *v.last()?);
    let elapsed = times.last()? - times.first()?;
    let vc = v.iter().sum::<f64>() / v.len() as f64;
    let swept = (first / vc).atan() - (last / vc).atan();
    (swept > 0.0 && elapsed > 0.0).then(|| (vc, elapsed / swept))
}

/// Gauss-Newton on the three parameters, with a Levenberg damping term and step
/// halving so that a seed which is merely in the right region still converges.
///
/// Three parameters and a few hundred samples: the normal equations are a 3×3
/// solve and the whole loop is microseconds. There is no reason to be clever.
fn gauss_newton(seed: Curve, times: &[f64], v: &[f64]) -> Curve {
    let mut curve = seed;
    let mut best = sum_squares(&curve, times, v);
    let mut lambda = 1e-6;

    for _ in 0..100 {
        let mut normal = [[0.0f64; 3]; 3];
        let mut gradient = [0.0f64; 3];
        let mut usable = 0usize;
        for (t, measured) in times.iter().zip(v) {
            let (Some(modelled), Some(j)) = (curve.at(*t), curve.jacobian(*t)) else {
                continue;
            };
            let residual = modelled - measured;
            usable += 1;
            for r in 0..3 {
                gradient[r] += j[r] * residual;
                for c in 0..3 {
                    normal[r][c] += j[r] * j[c];
                }
            }
        }
        if usable < 4 {
            break;
        }
        for (r, row) in normal.iter_mut().enumerate() {
            row[r] *= 1.0 + lambda;
        }
        let Some(step) = solve3(normal, [-gradient[0], -gradient[1], -gradient[2]]) else {
            break;
        };

        let mut scale = 1.0;
        let mut improved = false;
        for _ in 0..30 {
            let trial = Curve {
                v0: curve.v0 + scale * step[0],
                vc: curve.vc + scale * step[1],
                tau: curve.tau + scale * step[2],
            };
            if trial.valid() {
                let ssr = sum_squares(&trial, times, v);
                if ssr < best {
                    let converged = best - ssr < 1e-15 * best.max(1e-12);
                    curve = trial;
                    best = ssr;
                    improved = true;
                    if converged {
                        return curve;
                    }
                    break;
                }
            }
            scale *= 0.5;
        }
        if improved {
            lambda = (lambda * 0.3).max(1e-9);
        } else {
            lambda *= 10.0;
            if lambda > 1e9 {
                break;
            }
        }
    }
    curve
}

/// A 3×3 solve by Cramer's rule. The matrix is `JᵀJ` plus a damping term, so it
/// is symmetric and positive definite whenever it is not degenerate, and a
/// determinant that has collapsed is exactly the case the caller wants told.
fn solve3(m: [[f64; 3]; 3], v: [f64; 3]) -> Option<[f64; 3]> {
    let det = |a: [[f64; 3]; 3]| {
        a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
    };
    let base = det(m);
    if !base.is_finite() || base == 0.0 {
        return None;
    }
    let mut out = [0.0; 3];
    for (col, slot) in out.iter_mut().enumerate() {
        let mut replaced = m;
        for (row, cell) in replaced.iter_mut().enumerate() {
            cell[col] = v[row];
        }
        *slot = det(replaced) / base;
    }
    out.iter().all(|x| x.is_finite()).then_some(out)
}

// ---------------------------------------------------------------------------
// Finding a pass in the stream
// ---------------------------------------------------------------------------

/// What the detector has to say about the sample it was just given.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A coast started here.
    Opened,
    /// A coast reached `to_kmh` intact.
    Closed(Pass),
    /// A coast in progress was thrown away, and this is why.
    ///
    /// The reason is shown to the driver, because "rejected" on its own gives
    /// them nothing to do differently on the next attempt.
    Discarded(&'static str),
}

/// A pass, recognised from the bus rather than from a keystroke.
///
/// It opens when the speed falls past `from_kmh` with the pedal at zero and the
/// selector in neutral, closes at `to_kmh`, and is discarded the moment the
/// drive stops being a free coast. All of those are proven channels, so nobody
/// has to press anything at 120 km/h — and the tool never asks for neutral to be
/// selected at speed, which is the driver's decision and is taken before the
/// pass begins.
///
/// The detector sees speed and time and nothing else, so the `Pass` it emits
/// carries `rho` and `mass_kg` as NaN unless [`Detector::with_conditions`] was
/// used. That is not a defaulting: [`fit`] answers
/// [`Reject::ConditionsUnstated`] rather than producing a number, because the
/// barometer, the ambient sensor and the car file belong to the caller.
#[derive(Clone, Debug)]
pub struct Detector {
    from_kmh: f64,
    to_kmh: f64,
    rho: f64,
    mass_kg: f64,
    previous_kmh: Option<f64>,
    open: Option<Open>,
}

#[derive(Clone, Debug)]
struct Open {
    speed: Track,
    kmh: Vec<f64>,
    lowest_kmh: f64,
    /// The deceleration over the pass's first [`BRAKE_WINDOW_S`], which is the
    /// largest a free coast can produce and therefore the yardstick for every
    /// later window.
    opening_decel: Option<f64>,
}

impl Detector {
    pub fn new(from_kmh: f64, to_kmh: f64) -> Self {
        Detector {
            from_kmh,
            to_kmh,
            rho: f64::NAN,
            mass_kg: f64::NAN,
            previous_kmh: None,
            open: None,
        }
    }

    /// The air and the load the passes will be fitted against, stated once
    /// because neither changes over a coastdown.
    pub fn with_conditions(mut self, rho: f64, mass_kg: f64) -> Self {
        self.rho = rho;
        self.mass_kg = mass_kg;
        self
    }

    /// One sample of the leading speed channel, with whatever the pedal and the
    /// selector said at that instant.
    ///
    /// `None` for either channel means it had no reading there, not that it read
    /// zero: an absent value can never open a pass and never discards one. A
    /// *present* value that fails the test does both.
    pub fn on_sample(
        &mut self,
        t: Seconds,
        speed_kmh: f64,
        pedal_pct: Option<f64>,
        selector: Option<&str>,
    ) -> Option<Event> {
        let previous = self.previous_kmh.replace(speed_kmh);

        let Some(open) = self.open.as_mut() else {
            let crossed = previous.is_some_and(|p| p > self.from_kmh) && speed_kmh <= self.from_kmh;
            let ready = pedal_pct.is_some_and(|p| p <= PEDAL_ZERO_PERCENT)
                && selector.is_some_and(in_neutral);
            if crossed && ready && speed_kmh > self.to_kmh {
                let mut fresh = Open {
                    speed: Track::default(),
                    kmh: Vec::new(),
                    lowest_kmh: speed_kmh,
                    opening_decel: None,
                };
                fresh.push(t, speed_kmh);
                self.open = Some(fresh);
                return Some(Event::Opened);
            }
            return None;
        };

        if pedal_pct.is_some_and(|p| p > PEDAL_ZERO_PERCENT) {
            self.open = None;
            return Some(Event::Discarded("the pedal moved"));
        }
        if selector.is_some_and(|s| !in_neutral(s)) {
            self.open = None;
            return Some(Event::Discarded("the selector left neutral"));
        }
        if speed_kmh > open.lowest_kmh + RISE_TOLERANCE_KMH {
            self.open = None;
            return Some(Event::Discarded("the speed rose"));
        }

        open.push(t, speed_kmh);
        if let Some(decel) = open.window_decel() {
            match open.opening_decel {
                None => open.opening_decel = Some(decel),
                Some(opening) if decel > BRAKING_STEP_FACTOR * opening.max(0.0) => {
                    self.open = None;
                    return Some(Event::Discarded("the deceleration stepped like braking"));
                }
                Some(_) => {}
            }
        }

        if speed_kmh <= self.to_kmh {
            let done = self.open.take().expect("still open");
            return Some(Event::Closed(Pass {
                speed: done.speed,
                rho: self.rho,
                mass_kg: self.mass_kg,
            }));
        }
        None
    }
}

impl Open {
    fn push(&mut self, t: Seconds, speed_kmh: f64) {
        self.speed.push(t, speed_kmh / KMH_PER_MS);
        self.kmh.push(speed_kmh);
        self.lowest_kmh = self.lowest_kmh.min(speed_kmh);
    }

    /// The deceleration over the last [`BRAKE_WINDOW_S`], in m/s², or `None`
    /// while the pass is younger than that.
    ///
    /// Measured across a window rather than between adjacent samples because a
    /// quantised speed channel differenced at 20 Hz is mostly quantisation: a
    /// coast loses a couple of hundredths of a km/h per sample and a whole count
    /// of noise.
    fn window_decel(&self) -> Option<f64> {
        let last = self.speed.len().checked_sub(1)?;
        let now = self.speed.t[last];
        let start = self.speed.t.iter().position(|t| now - t <= BRAKE_WINDOW_S)?;
        if start == last {
            return None;
        }
        let dt = now - self.speed.t[start];
        (dt > 0.0).then(|| (self.speed.v[start] - self.speed.v[last]) / dt)
    }
}

/// Whether a selector label means neutral.
///
/// The gate's lettering is one of the few things about a car that is not a
/// corpus question: P-R-N-D is the sequence type approval requires, so `N` is
/// what a selector channel spells neutral with, whatever the catalog's language.
/// Anything else — a manual car with no selector channel at all, a corpus that
/// writes something longer — arrives as [`None`] or as a label that is not
/// neutral, and either way no pass opens, which is the safe direction.
fn in_neutral(label: &str) -> bool {
    let label = label.trim().to_ascii_lowercase();
    label == "n" || label.starts_with("neutral")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::race::power::Inertias;

    /// A mid-size car, invented for these tests. Nothing in the module knows any
    /// of it — mass, density and `δ₁` are all arguments — and `δ₁` in particular
    /// is computed by [`power::deltas`] rather than written down, which is the
    /// point the design makes about 1.03 versus 1.04.
    const MASS_KG: f64 = 1475.0;
    const RHO: f64 = 1.2;
    const RADIUS_M: f64 = 0.313;
    const INERTIAS: Inertias = Inertias { wheels_kgm2: 5.5, engine_kgm2: 0.34 };
    const TRUE_CDA: f64 = 0.63;
    const TRUE_CRR: f64 = 0.0114;

    fn delta1() -> f64 {
        power::deltas(&INERTIAS, MASS_KG, RADIUS_M).0
    }

    /// The road a synthetic pass is driven on.
    #[derive(Clone, Copy)]
    struct Road {
        cda: f64,
        crr: f64,
        grade_sin: f64,
        /// Positive is a headwind: the air the car meets is `v + headwind`.
        headwind_ms: f64,
    }

    impl Default for Road {
        fn default() -> Self {
            Road { cda: TRUE_CDA, crr: TRUE_CRR, grade_sin: 0.0, headwind_ms: 0.0 }
        }
    }

    /// The step the ODE is integrated at. One millisecond over a forty-second
    /// coast, by RK4, is exact to far more digits than anything asserted here.
    const SIM_DT: f64 = 1e-3;

    /// `dv/dt` for the coastdown, headwind and slope included.
    fn accel(road: &Road, v: f64) -> f64 {
        let air = v + road.headwind_ms;
        let drag = 0.5 * RHO * road.cda * air.abs() * air;
        let rolling = MASS_KG * G * road.crr;
        let gravity = MASS_KG * G * road.grade_sin;
        -(drag + rolling + gravity) / (MASS_KG * (1.0 + delta1()))
    }

    /// The true `v(t)` of a coast from `from_kmh` to `to_kmh`, on a 1 ms grid.
    fn truth(road: &Road, from_kmh: f64, to_kmh: f64) -> Vec<f64> {
        let mut v = from_kmh / KMH_PER_MS;
        let floor = to_kmh / KMH_PER_MS;
        let mut out = vec![v];
        while v > floor && out.len() < 200_000 {
            let k1 = accel(road, v);
            let k2 = accel(road, v + 0.5 * SIM_DT * k1);
            let k3 = accel(road, v + 0.5 * SIM_DT * k2);
            let k4 = accel(road, v + SIM_DT * k3);
            v += SIM_DT / 6.0 * (k1 + 2.0 * k2 + 2.0 * k3 + k4);
            out.push(v);
        }
        out
    }

    fn truth_at(dense: &[f64], t: f64) -> f64 {
        let x = (t / SIM_DT).max(0.0);
        let i = (x.floor() as usize).min(dense.len() - 1);
        let j = (i + 1).min(dense.len() - 1);
        dense[i] + (dense[j] - dense[i]) * (x - i as f64)
    }

    /// How a channel is sampled. `jitter_s` shifts each timestamp by a
    /// deterministic amount — a golden-ratio sequence, so it is spread evenly and
    /// is the same on every machine and every run — and `hold_s` is the
    /// zero-order hold of a channel that only updates that often.
    #[derive(Clone, Copy)]
    struct Sampling {
        period_s: f64,
        jitter_s: f64,
        hold_s: f64,
    }

    const EXACT: Sampling = Sampling { period_s: 0.05, jitter_s: 0.0, hold_s: 0.0 };
    /// 20 Hz, ±4 ms of jitter, and a 50 ms hold deliberately out of phase with
    /// the polling so the staircase walks through the samples rather than
    /// sitting still under them.
    const REALISTIC: Sampling = Sampling { period_s: 0.05, jitter_s: 0.004, hold_s: 0.05 };

    fn jitter(i: usize) -> f64 {
        // Low-discrepancy and entirely deterministic: no RNG anywhere in these
        // tests, so a failure is always reproducible.
        ((i as f64 * 0.618_033_988_749_895).fract()) * 2.0 - 1.0
    }

    fn pass_of(road: &Road, from_kmh: f64, to_kmh: f64, s: Sampling) -> Pass {
        let dense = truth(road, from_kmh, to_kmh);
        let end = (dense.len() - 1) as f64 * SIM_DT;
        let mut speed = Track::default();
        let mut i = 0;
        loop {
            let t = i as f64 * s.period_s + s.jitter_s * jitter(i);
            if t > end {
                break;
            }
            // The hold's phase is offset from the polling grid on purpose.
            let held = if s.hold_s > 0.0 {
                let phase = 0.013;
                (((t - phase) / s.hold_s).floor() * s.hold_s + phase).max(0.0)
            } else {
                t
            };
            speed.push(t.max(0.0), truth_at(&dense, held));
            i += 1;
        }
        Pass { speed, rho: RHO, mass_kg: MASS_KG }
    }

    #[test]
    fn the_closed_form_recovers_a_road_load_it_was_never_told() {
        let pass = pass_of(&Road::default(), 120.0, 40.0, EXACT);
        let f = fit(&pass, delta1()).unwrap();
        // The samples are the ODE's own solution, so the only error left is the
        // integrator's and the fit's: both coefficients come back to five digits.
        assert!((f.cda - TRUE_CDA).abs() < 1e-4 * TRUE_CDA, "CdA {}", f.cda);
        assert!((f.crr - TRUE_CRR).abs() < 1e-4 * TRUE_CRR, "Crr {}", f.crr);
        assert!(f.rms_kmh < 1e-3, "rms {}", f.rms_kmh);
    }

    #[test]
    fn jitter_and_a_fifty_millisecond_hold_do_not_move_the_answer() {
        let pass = pass_of(&Road::default(), 120.0, 40.0, REALISTIC);
        let f = fit(&pass, delta1()).unwrap();
        // A 50 ms hold at 120 km/h is 1.7 cm/s of stale speed and the jitter is
        // ±4 ms; neither is correlated with v², so both land in the residual
        // rather than in the coefficients. Half a per cent covers both.
        assert!((f.cda - TRUE_CDA).abs() < 0.005 * TRUE_CDA, "CdA {}", f.cda);
        assert!((f.crr - TRUE_CRR).abs() < 0.005 * TRUE_CRR, "Crr {}", f.crr);
        assert!(f.rms_kmh < MAX_RESIDUAL_KMH, "rms {}", f.rms_kmh);
    }

    #[test]
    fn a_one_per_cent_grade_moves_crr_by_tens_of_per_cent_and_the_residual_by_nothing() {
        // The test the design asks for by name, so that nobody reintroduces "the
        // residual threshold catches a slope". It does not, and it cannot: a
        // constant force is absorbed *entirely* into the constant term, and the
        // curve through the samples is the same curve.
        let flat = fit(&pass_of(&Road::default(), 120.0, 40.0, REALISTIC), delta1()).unwrap();
        let uphill = Road { grade_sin: power::grade_sin(1.0), ..Road::default() };
        let sloped = fit(&pass_of(&uphill, 120.0, 40.0, REALISTIC), delta1()).unwrap();

        // sin θ at 1 % is 0.009 999 5 and adds to Crr outright, so
        // 0.0114 → 0.021 4: +87.7 %.
        let moved = (sloped.crr - flat.crr) / flat.crr * 100.0;
        assert!(moved > 80.0, "Crr moved only {moved} %");
        assert!(
            (sloped.crr - (flat.crr + power::grade_sin(1.0))).abs() < 1e-4 * flat.crr,
            "the slope lands in Crr exactly: {} vs {}",
            sloped.crr,
            flat.crr + power::grade_sin(1.0)
        );

        // And the fit quality does not notice. Both residuals are the sampling's,
        // and they agree to a thousandth of a km/h.
        assert!(
            (sloped.rms_kmh - flat.rms_kmh).abs() < 0.01,
            "residual moved from {} to {} km/h",
            flat.rms_kmh,
            sloped.rms_kmh
        );
        assert!(sloped.rms_kmh < MAX_RESIDUAL_KMH, "a hill still passes every residual bar");
    }

    #[test]
    fn two_reciprocal_passes_cancel_a_grade_and_report_it() {
        let up = Road { grade_sin: power::grade_sin(1.0), ..Road::default() };
        let down = Road { grade_sin: -power::grade_sin(1.0), ..Road::default() };
        let a = fit(&pass_of(&up, 120.0, 40.0, REALISTIC), delta1()).unwrap();
        let b = fit(&pass_of(&down, 120.0, 40.0, REALISTIC), delta1()).unwrap();

        let out = combine(&a, &b, MASS_KG, RHO, delta1()).unwrap();
        assert!((out.cda - TRUE_CDA).abs() < 0.01 * TRUE_CDA, "CdA {}", out.cda);
        assert!((out.crr - TRUE_CRR).abs() < 0.01 * TRUE_CRR, "Crr {}", out.crr);
        // Reported to within a hundredth of a per cent of grade — which is the
        // assertion that fails if the (1 + δ₁) the design writes on this formula
        // is ever applied here: δ₁ is 0.038 on this car, so it would read 1.04 %.
        assert!(
            (out.implied_grade_percent - 1.0).abs() < 0.01,
            "grade {}",
            out.implied_grade_percent
        );
        assert!(out.implied_wind_ms.abs() < 0.5, "no wind was simulated: {}", out.implied_wind_ms);
    }

    #[test]
    fn wind_does_not_cancel_and_lands_one_signed_in_crr() {
        let into = Road { headwind_ms: 5.0, ..Road::default() };
        let behind = Road { headwind_ms: -5.0, ..Road::default() };
        let a = fit(&pass_of(&into, 120.0, 40.0, REALISTIC), delta1()).unwrap();
        let b = fit(&pass_of(&behind, 120.0, 40.0, REALISTIC), delta1()).unwrap();
        let out = combine(&a, &b, MASS_KG, RHO, delta1()).unwrap();

        // ΔCrr = ½ρ·CdA·w²/(m·g) = 0.5·1.2·0.63·25/(1475·9.80665) = 6.53e-4,
        // which is +5.7 % on 0.0114. It survives the averaging because the w²
        // term is symmetric in the direction, unlike the 2vw cross term.
        let expected = 0.5 * RHO * TRUE_CDA * 25.0 / (MASS_KG * G);
        let excess = out.crr - TRUE_CRR;
        assert!(
            (excess - expected).abs() < 0.05 * expected,
            "Crr excess {excess} against the predicted {expected}"
        );
        assert!(excess > 0.0, "a wind can only ever inflate Crr");
        // The averaged CdA is untouched, which is the other half of the claim:
        // 0.780 into the wind and 0.480 with it, and 0.630 between them.
        assert!((out.cda - TRUE_CDA).abs() < 0.005 * TRUE_CDA, "CdA {}", out.cda);

        // The per-pass CdA spread recovers the wind: 4.86 m/s against the 5.0
        // simulated, from (CdA_A − CdA_B)/(CdA_A + CdA_B)·v̄ alone.
        assert!((out.implied_wind_ms - 5.0).abs() < 0.2, "wind {}", out.implied_wind_ms);

        // The grade estimate is contaminated but not destroyed. The 2vw cross
        // term is antisymmetric and so cancels out of the *average*, which is
        // what protects CdA and Crr — but it does not cancel out of the
        // *difference*, and the part of it the fit puts into the constant term
        // reads as a slope. Simulated: 0.26 % of apparent grade per 5 m/s of
        // wind, a quarter of the 1 % the grade test resolves, and one more
        // reason the help text asks for under about 2 m/s.
        assert!(out.implied_grade_percent.abs() < 0.4, "grade {}", out.implied_grade_percent);
    }

    #[test]
    fn one_pass_alone_is_not_a_coastdown() {
        let f = fit(&pass_of(&Road::default(), 120.0, 40.0, REALISTIC), delta1()).unwrap();
        assert_eq!(road_load(&[], MASS_KG, RHO, delta1()), Err(Reject::OnlyOnePass));
        assert_eq!(road_load(&[f], MASS_KG, RHO, delta1()), Err(Reject::OnlyOnePass));
        assert!(road_load(&[f, f], MASS_KG, RHO, delta1()).is_ok());
    }

    #[test]
    fn a_pass_with_no_speed_range_in_it_is_refused() {
        let pass = pass_of(&Road::default(), 120.0, 118.0, REALISTIC);
        assert!(
            matches!(fit(&pass, delta1()), Err(Reject::TooNarrow { .. })),
            "a two km/h coast is a seed being handed back"
        );
    }

    #[test]
    fn a_pass_whose_conditions_were_never_stated_produces_no_number() {
        let mut pass = pass_of(&Road::default(), 120.0, 40.0, REALISTIC);
        pass.rho = f64::NAN;
        assert_eq!(fit(&pass, delta1()), Err(Reject::ConditionsUnstated));
    }

    #[test]
    fn passes_that_disagree_past_the_bar_are_rejected_with_the_slope_that_explains_it() {
        // 1.1 % of grade on a Crr of 0.0114: the uphill pass fits 0.0224 and the
        // downhill one 0.0004, which is |a−b|/(a+b) = 96.5 %. The downhill
        // direction has almost no road load left in it, and a pair like that is
        // one measurement and a rounding error rather than two.
        let a = Fit { cda: 0.63, crr: 0.0114 + 0.011, rms_kmh: 0.1, mean_speed_ms: 22.0 };
        let b = Fit { cda: 0.63, crr: 0.0114 - 0.011, rms_kmh: 0.1, mean_speed_ms: 22.0 };
        match combine(&a, &b, MASS_KG, RHO, delta1()) {
            Err(Reject::Disagreement { crr_percent, limit, implied_grade_percent, .. }) => {
                assert!((crr_percent - 96.49).abs() < 0.01, "{crr_percent}");
                assert!(crr_percent > limit, "{crr_percent} vs {limit}");
                // sin θ = 0.011 → 1.1 % of road, and the driver is told it.
                assert!((implied_grade_percent - 1.1).abs() < 0.01, "{implied_grade_percent}");
            }
            other => panic!("{other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // The detector
    // -----------------------------------------------------------------------

    /// A clean coast as the stream sees it: km/h, 20 Hz, foot off, lever in N.
    fn stream(road: &Road) -> Vec<(Seconds, f64)> {
        let pass = pass_of(road, 130.0, 30.0, REALISTIC);
        pass.speed
            .t
            .iter()
            .zip(&pass.speed.v)
            .map(|(t, v)| (*t, v * KMH_PER_MS))
            .collect()
    }

    fn detector() -> Detector {
        Detector::new(120.0, 40.0).with_conditions(RHO, MASS_KG)
    }

    #[test]
    fn a_clean_coast_opens_at_the_top_and_closes_at_the_bottom() {
        let mut d = detector();
        let (mut opened, mut closed) = (false, None);
        for (t, kmh) in stream(&Road::default()) {
            match d.on_sample(t, kmh, Some(0.0), Some("N")) {
                Some(Event::Opened) => opened = true,
                Some(Event::Closed(p)) => closed = Some(p),
                Some(Event::Discarded(why)) => panic!("discarded: {why}"),
                None => {}
            }
        }
        assert!(opened);
        let pass = closed.expect("the coast reached 40 km/h");
        assert!(pass.speed.v.first().unwrap() * KMH_PER_MS <= 120.0);
        assert!(pass.speed.v.last().unwrap() * KMH_PER_MS <= 40.0);
        // And what it collected is fittable — the detector's output is the fit's
        // input and nothing in between reshapes it.
        let f = fit(&pass, delta1()).unwrap();
        assert!((f.cda - TRUE_CDA).abs() < 0.01 * TRUE_CDA, "{f:?}");
    }

    #[test]
    fn a_pass_never_opens_out_of_neutral_or_under_power() {
        let mut in_gear = detector();
        let mut on_pedal = detector();
        for (t, kmh) in stream(&Road::default()) {
            assert_eq!(in_gear.on_sample(t, kmh, Some(0.0), Some("D")), None);
            assert_eq!(on_pedal.on_sample(t, kmh, Some(12.0), Some("N")), None);
        }
    }

    /// Run a clean coast through the detector, changing one thing partway down.
    fn discarded_when(
        mut change: impl FnMut(f64, f64, &mut f64, &mut f64, &mut &'static str),
    ) -> Option<&'static str> {
        let mut d = detector();
        for (t, kmh) in stream(&Road::default()) {
            let (mut speed, mut pedal, mut selector) = (kmh, 0.0f64, "N");
            change(t, kmh, &mut speed, &mut pedal, &mut selector);
            if let Some(Event::Discarded(why)) = d.on_sample(t, speed, Some(pedal), Some(selector))
            {
                return Some(why);
            }
        }
        None
    }

    #[test]
    fn the_pedal_moving_discards_the_pass_and_says_so() {
        let why = discarded_when(|_, kmh, _, pedal, _| {
            if kmh < 90.0 {
                *pedal = 8.0;
            }
        });
        assert_eq!(why, Some("the pedal moved"));
    }

    #[test]
    fn the_selector_leaving_neutral_discards_the_pass_and_says_so() {
        let why = discarded_when(|_, kmh, _, _, selector| {
            if kmh < 90.0 {
                *selector = "D";
            }
        });
        assert_eq!(why, Some("the selector left neutral"));
    }

    #[test]
    fn rising_speed_discards_the_pass_and_says_so() {
        // A push from behind — or a hill that turns over — without the pedal
        // ever registering. Two km/h of recovery, which is past the one count of
        // quantisation the detector has to forgive.
        let why = discarded_when(|_, kmh, speed, _, _| {
            if kmh < 90.0 {
                *speed = kmh + 2.0;
            }
        });
        assert_eq!(why, Some("the speed rose"));
    }

    #[test]
    fn a_braking_shaped_deceleration_step_discards_the_pass_and_says_so() {
        // The one that matters most, because a part-braked coast fitted as a
        // whole one puts the brakes into Crr and the driver never sees it. Below
        // 90 km/h an extra 1.5 m/s² is subtracted — light braking, four or five
        // times the 0.35 m/s² a coast makes at that speed.
        let mut d = detector();
        let mut extra = 0.0;
        let mut previous_t = None;
        let mut why = None;
        for (t, kmh) in stream(&Road::default()) {
            if let Some(prev) = previous_t
                && kmh - extra < 90.0
            {
                extra += 1.5 * (t - prev) * KMH_PER_MS;
            }
            previous_t = Some(t);
            if let Some(Event::Discarded(reason)) =
                d.on_sample(t, kmh - extra, Some(0.0), Some("N"))
            {
                why = Some(reason);
                break;
            }
        }
        assert_eq!(why, Some("the deceleration stepped like braking"));
    }

    #[test]
    fn a_selector_label_is_read_as_a_word_not_as_a_code() {
        assert!(in_neutral("N"));
        assert!(in_neutral(" n "));
        assert!(in_neutral("Neutral"));
        assert!(!in_neutral("D"));
        assert!(!in_neutral("not engaged"));
        assert!(!in_neutral(""));
    }
}
