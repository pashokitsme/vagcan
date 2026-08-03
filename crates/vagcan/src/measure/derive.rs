//! Everything a finished run can be asked that needs no parameter: the
//! acceleration trace, where the launch really was, the peak, what each
//! gearchange cost, the distance covered, and how coarse the speed signal's own
//! clock is.
//!
//! Pure arithmetic over [`Track`] and [`States`] — no adapter, no catalog, no
//! `Instant::now()`. Time is always a parameter, which is what makes every
//! claim below testable against a trace written by hand.
//!
//! Three decisions here are worth stating at the door, because each of them was
//! reached by discarding something that looked simpler and was wrong.
//!
//! **Acceleration is a least-squares slope, not an endpoint difference.** The
//! samples are unevenly spaced in time — that is the whole reason each value
//! carries its own timestamp — so an endpoint difference's baseline wanders
//! with the jitter, and it throws away five of the seven samples in a 0.3 s
//! window, which lets one stale endpoint corrupt the estimate on its own. The
//! fit also has a well-defined attachment point, the timestamp centroid `t̄`,
//! and that is what makes the causal lag exactly `t_now − t̄` rather than
//! "about half the window".
//!
//! **What the window costs is not `sinc(πfW)`.** That is the response of a
//! boxcar *smoother*. A first-order least-squares *differentiator* has
//! `|H(f)| = (3/x²)·|sin x/x − cos x|` with `x = πfW`, which loses 9 % on a
//! 1 Hz peak where sinc would claim 14 %, and still passes 0.30 at `f = 1/W`
//! where sinc claims a perfect null. Causal and central share that response
//! exactly: they differ in **phase only**, so moving to a centred window after
//! the run fixes the lag and recovers no peak height whatever.
//!
//! **Selection on noise is the recurring failure mode**, and it is guarded
//! twice. The peak is a neighbourhood mean rather than a maximum, because the
//! maximum of a noisy estimator picks positive excursions and reads about 7 %
//! high — always in the flattering direction. And gearchanges are located from
//! the gear channel, never by thresholding the acceleration trace, because a
//! threshold-found window makes the cost positive by construction and reports
//! one where no shift happened.

use super::types::{Seconds, States, Track};

/// Where the fitting window sits relative to the sample it is reported for.
///
/// The choice is about **phase and nothing else**. Both schemes fit the same
/// samples over the same span when the run allows it, so their magnitude
/// response is identical; what changes is where the answer is valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    /// Trailing window, `[t − W, t]`. The only scheme available live, because
    /// the future half of a centred window has not been read yet. Its answer is
    /// valid at `t̄ ≈ t − W/2`, and that delay is exact rather than nominal.
    Causal,
    /// Symmetric window, `[t − W/2, t + W/2]`, for a run that is already over.
    /// At the run's edges it degenerates to a one-sided fit over whatever
    /// samples exist — flagged by [`Slope::span`], never skipped, because a
    /// launch's peak often lives inside the first half-second.
    Central,
}

/// One acceleration estimate.
///
/// `span` is the time between the first and last sample the fit actually saw,
/// which is less than the window at the run's edges and is how a caller tells a
/// full fit from a one-sided one. `sigma` is the fit's own standard error,
/// `√(SSE/(n−2)/Σ(tᵢ−t̄)²)` — over the same `Σ(tᵢ−t̄)²` the slope is divided by,
/// so it costs nothing to compute and it grows sharply as the span shrinks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slope {
    pub a: f64,
    pub t: Seconds,
    pub span: Seconds,
    pub sigma: f64,
}

/// Below three samples there is a line through the points but no residual to
/// judge it by, and therefore no standard error. A slope without one is not
/// something this module is willing to hand out.
const MIN_FIT_SAMPLES: usize = 3;

/// First-order least squares of the track against each sample's **own**
/// timestamp, over the window ending at, or centred on, `index`.
///
/// ```text
/// a = Σ(tᵢ − t̄)(vᵢ − v̄) / Σ(tᵢ − t̄)²        valid at t = t̄
/// ```
///
/// `None` when the window holds fewer than three samples, or when every sample
/// in it shares one timestamp and there is no baseline to fit against.
///
/// The units are the track's own, divided by seconds: a track in m/s yields
/// m/s². Nothing here converts anything, because a conversion applied twice is
/// the kind of error no test in this file could see.
pub fn slope(track: &Track, index: usize, window: Seconds, scheme: Scheme) -> Option<Slope> {
    let centre = *track.t.get(index)?;
    let (lo, hi) = match scheme {
        Scheme::Causal => (centre - window, centre),
        Scheme::Central => (centre - window / 2.0, centre + window / 2.0),
    };

    // The track is pushed in time order, so the window is a contiguous range
    // and can be found by bisection rather than by scanning the whole run once
    // per sample.
    let from = track.t.partition_point(|probe| *probe < lo);
    let to = track.t.partition_point(|probe| *probe <= hi);
    if to < from || to - from < MIN_FIT_SAMPLES {
        return None;
    }

    let count = (to - from) as f64;
    let t_bar = track.t[from..to].iter().sum::<f64>() / count;
    let v_bar = track.v[from..to].iter().sum::<f64>() / count;

    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for i in from..to {
        let dt = track.t[i] - t_bar;
        sxx += dt * dt;
        sxy += dt * (track.v[i] - v_bar);
    }
    if sxx <= 0.0 {
        return None;
    }
    let a = sxy / sxx;

    let mut sse = 0.0;
    for i in from..to {
        let residual = (track.v[i] - v_bar) - a * (track.t[i] - t_bar);
        sse += residual * residual;
    }
    let sigma = (sse / (count - 2.0) / sxx).sqrt();

    Some(Slope { a, t: t_bar, span: track.t[to - 1] - track.t[from], sigma })
}

/// [`slope`] at every sample that has a window worth fitting.
///
/// The result is shorter than the track wherever a window held fewer than three
/// samples, and its times are centroids rather than sample times, so it must
/// not be indexed against the track it came from.
pub fn accel_series(track: &Track, window: Seconds, scheme: Scheme) -> Vec<Slope> {
    (0..track.len()).filter_map(|i| slope(track, i, window, scheme)).collect()
}

/// How much of the start of the movement the launch fit looks at.
///
/// Long enough to hold half a dozen samples at any poll rate this tool
/// achieves, short enough that the fit is still inside the launch rather than
/// inside first gear. A property of the estimator, not of any car.
pub const START_FIT_S: Seconds = 0.4;

/// The launch instant, as an interval rather than a number.
///
/// The two estimators available **bracket** the answer instead of agreeing on
/// it, so the pair is the result and neither half is. A caller prints a 0-based
/// mark as `6.03 … 6.38 s`, and the width of that is an uncertainty derived from
/// this run's own samples rather than a tolerance copied from a table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Start {
    /// The estimate to use — the midpoint of the bracket.
    pub t: Seconds,
    /// The constant-jerk fit: it reaches back too far, so it is the earliest
    /// launch worth believing.
    pub earliest: Seconds,
    /// The two-point linear extrapolation: it falls short, so it is the latest.
    pub latest: Seconds,
}

/// When the car actually started moving, bracketed by two estimators that miss
/// it from opposite sides.
///
/// **Not the first non-zero sample.** A wheel-speed signal has a low-speed dead
/// band — at a few dozen teeth per revolution the pulse interval at walking pace
/// is longer than the unit's own update — so it reports zero through the first
/// tenths of a second of a launch that is already under way. Whatever is done
/// here is an extrapolation into a stretch nobody observed, and the honest form
/// of that is an interval.
///
/// **`earliest` is a constant-jerk fit**, `v = ½j(t − t₀)²` over the first
/// [`START_FIT_S`] of movement, extrapolated back to `v = 0`. It is done on
/// `√v`, which is exactly linear in `t` under that model, so it is closed-form
/// and has exactly one root. It overshoots: the model forces
/// `v/v̇ = (t − t₀)/2`, so wherever the acceleration has already saturated by
/// the time the signal wakes — which is most launches — it reaches back about
/// twice as far as it should. Simulated across ramp and exponential launches
/// with dead bands from 1 to 3 km/h, it lands 0.02 to 0.27 s early.
///
/// **`latest` is a two-point linear extrapolation** through the first two
/// moving samples. A launch is convex, so a straight line drawn through it runs
/// under the true curve and reaches zero late; the two samples nearest the wake
/// are the ones least contaminated by the saturated stretch, which is what makes
/// this the tightest late bound available. It lands 0.10 to 0.25 s late over the
/// same set.
///
/// The truth was between the two on every trace tried, which is the whole
/// argument for reporting both. An earlier draft reported the constant-jerk fit
/// alone with a one-signed `+x/−0.00` band, on the reasoning that a convex
/// launch always reads short — sound for the linear estimator and false for the
/// quadratic one that paragraph then specified.
///
/// **Both are clamped above at the first moving sample, and neither has a lower
/// clamp.** The upper clamp is a fact rather than an estimate: the car was
/// observed moving at that sample, so no launch time after it is possible. The
/// lower clamp an earlier draft proposed — into `(last zero, first non-zero]` —
/// sounds conservative and is the opposite: under the dead band the true launch
/// lies *before* that window, so the clamp bounds the estimate into a region
/// that provably excludes the answer, and it fires on every run.
///
/// **Where the two estimators cross, the interval is ordered and not
/// collapsed.** The quadratic normally reaches back further, but on a noisy
/// wake it can land after the linear one, and an earlier version answered that
/// by pulling `earliest` down to `latest` — turning a disagreement between two
/// estimators into a zero-width interval, which is the one thing the pair exists
/// to avoid. `8.94 … 8.94 s` reads as a number known exactly, and it would have
/// been printed exactly when the two methods agreed least.
pub fn start(track: &Track) -> Option<Start> {
    let first = (0..track.len()).find(|&i| track.v[i] > 0.0)?;
    let second = ((first + 1)..track.len()).find(|&i| track.v[i] > 0.0)?;
    let t_first = track.t[first];

    let (rise, step) = (track.v[second] - track.v[first], track.t[second] - track.t[first]);
    let latest = if rise > 0.0 && step > 0.0 {
        (t_first - track.v[first] * step / rise).min(t_first)
    } else {
        // Two readings that gain nothing extrapolate to nowhere; what is still
        // known is that the car was already moving at the first of them.
        t_first
    };

    let quadratic = constant_jerk_launch(track, first, t_first)?.min(t_first);
    let (earliest, latest) = (quadratic.min(latest), quadratic.max(latest));
    Some(Start { t: 0.5 * (earliest + latest), earliest, latest })
}

/// Where `v = ½j(t − t₀)²` puts the launch, fitted as a straight line through
/// `√v` over the first [`START_FIT_S`] of movement.
///
/// `None` when the window holds fewer than three moving samples, or when the
/// car gained no speed across it and there is nothing for the model to reach
/// back through.
fn constant_jerk_launch(track: &Track, first: usize, t_first: Seconds) -> Option<Seconds> {
    let last = track.t.partition_point(|probe| *probe <= t_first + START_FIT_S);

    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for i in first..last {
        if track.v[i] > 0.0 {
            xs.push(track.t[i]);
            ys.push(track.v[i].sqrt());
        }
    }
    if xs.len() < MIN_FIT_SAMPLES {
        return None;
    }

    let count = xs.len() as f64;
    let x_bar = xs.iter().sum::<f64>() / count;
    let y_bar = ys.iter().sum::<f64>() / count;
    let mut sxx = 0.0;
    let mut sxy = 0.0;
    for (x, y) in xs.iter().zip(&ys) {
        sxx += (x - x_bar) * (x - x_bar);
        sxy += (x - x_bar) * (y - y_bar);
    }
    if sxx <= 0.0 {
        return None;
    }
    let gradient = sxy / sxx;
    if gradient <= 0.0 {
        return None;
    }
    Some(x_bar - y_bar / gradient)
}

/// The peak of an acceleration series, and where it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Peak {
    pub value: f64,
    pub t: Seconds,
    pub sigma: f64,
}

/// What share of the window a fit must actually span before it may win the peak
/// search.
///
/// At the run's edges the fit is one-sided over a shrinking span, and its
/// standard error grows as `1/√Σ(tᵢ−t̄)²` — five times the interior noise by the
/// time two samples remain. Combined with an argmax, that would make the very
/// first eligible sample the reported peak far more often than it deserves, and
/// it would do so exactly where a fast car's real peak lives, which is where
/// the error would be hardest to spot.
pub const PEAK_MIN_SPAN_FRACTION: f64 = 0.6;

/// The peak acceleration, as the mean over `±tau` of the argmax.
///
/// **Not the maximum.** The maximum of a noisy estimator selects positive
/// excursions and reads about 7 % high, always in the flattering direction.
///
/// The correction has a residual of its own, and it is now the dominant one:
/// averaging a locally parabolic peak of curvature `c` over `±τ` under-reads it
/// by exactly `c·τ²/6` — about −1.3 % on a broad peak and −4 % on a sharp
/// first-gear one at `τ = 0.2 s`. Halving `τ` quarters it. That is stated
/// rather than left to be rediscovered, because the whole reason for the
/// correction was that a biased peak is not acceptable.
///
/// Only samples whose span reaches [`PEAK_MIN_SPAN_FRACTION`] of the window
/// take part — in the search *and* in the mean, since an edge fit is no better
/// as an ingredient than as a winner. `sigma` is the standard error of that
/// mean, built from the fits' own.
pub fn peak(series: &[Slope], tau: Seconds, window: Seconds) -> Option<Peak> {
    let floor = PEAK_MIN_SPAN_FRACTION * window;
    let eligible: Vec<&Slope> = series.iter().filter(|s| s.span >= floor).collect();
    let top = eligible.iter().copied().max_by(|x, y| x.a.total_cmp(&y.a))?;

    let near: Vec<&Slope> =
        eligible.iter().copied().filter(|s| (s.t - top.t).abs() <= tau).collect();
    let count = near.len() as f64;
    let value = near.iter().map(|s| s.a).sum::<f64>() / count;
    let variance = near.iter().map(|s| s.sigma * s.sigma).sum::<f64>();
    Some(Peak { value, t: top.t, sigma: variance.sqrt() / count })
}

/// How far either side of the reported gearchange the cost is integrated.
///
/// The gear channel says *when*, to within its own poll interval; it does not
/// say when torque left the road, which is earlier, nor when it came back,
/// which is later. A judgement about gearchanges in general — a few tenths of a
/// second — and not about any one gearbox, which is what keeps it admissible
/// here at all.
///
/// It is deliberately **symmetric**. The stretch before the change belongs to
/// the old and shorter gear, where the car was accelerating harder than the new
/// gear's baseline, so it enters the integral as a credit. Cropping it away
/// would leave a window containing only the dip, which is the same
/// positive-by-construction mistake as finding the window by threshold.
pub const SHIFT_PAD_S: Seconds = 0.35;

/// One gearchange and what it cost.
///
/// `speed_deficit_ms` is metres per second of speed the change gave up;
/// `cost_on_mark_s` is what that is worth in seconds on a mark whose upper
/// endpoint the car reaches at a stated acceleration.
///
/// The cost is an `Option` rather than a number that might be NaN. A deficit
/// costs no measurable time on a car that is not gaining speed at the mark, and
/// this figure goes straight into the session file, where a NaN either fails the
/// write or lands as `null` depending on the path. `None` survives the round
/// trip and says the same thing.
#[derive(Clone, Debug, PartialEq)]
pub struct Shift {
    pub t: Seconds,
    pub from: String,
    pub to: String,
    pub speed_deficit_ms: f64,
    pub cost_on_mark_s: Option<Seconds>,
}

/// Every gearchange in the run, located from the gear channel and costed
/// against the acceleration trace.
///
/// ```text
/// speed_deficit = ∫_shift ( a_post − a(t) ) dt          [m/s]
/// cost_on_mark  = speed_deficit / accel_at_mark_top     [s]
/// ```
///
/// The cost is a velocity deficit and not the dip's duration: a full-load
/// upshift does not slow the car down, it accelerates it less, so a
/// "speed lost" would read zero essentially always.
///
/// Three details decide whether the number means anything, and each was wrong
/// in an earlier draft:
///
/// - **The shift is observed, not inferred.** The gear is on the bus, so no
///   threshold and no baseline detection are needed — and a threshold relative
///   to the run's peak could not work anyway, since in a tall gear the
///   acceleration is *permanently* a small fraction of the peak.
/// - **The baseline is the gear being entered, not the one being left.** The
///   counterfactual for "what did this change cost" is an instantaneous swap
///   into the new gear, which is already slower than the old one. Charging the
///   shift for the ratio change as well overstates it by about a third.
/// - **The divisor is the acceleration at the mark's upper endpoint.** A
///   velocity deficit taken at 60 km/h persists all the way to 100, so dividing
///   by the acceleration where the shift happened understates the cost by
///   nearly a factor of two.
///
/// Transitions into or out of a level that is not a numbered gear — "not
/// engaged", "R" — are not shifts and are skipped. The labels are the catalog's
/// own and are read as labels: the codes behind them are neither contiguous nor
/// ordered by ratio, which is the bug this project already made once.
///
/// `cost_on_mark_s` is `None` when `accel_at_mark_top` is not positive: there is
/// no time a speed deficit costs on a car that is not gaining speed at the mark.
pub fn shifts(gear: &States, accel: &[Slope], accel_at_mark_top: f64) -> Vec<Shift> {
    let t: Vec<Seconds> = accel.iter().map(|s| s.t).collect();
    let a: Vec<f64> = accel.iter().map(|s| s.a).collect();

    let mut out = Vec::new();
    for (when, from, to) in gear.transitions() {
        if !is_numbered_gear(&from) || !is_numbered_gear(&to) {
            continue;
        }
        let (opens, closes) = (when - SHIFT_PAD_S, when + SHIFT_PAD_S);
        let Some(a_post) = mean_between(&t, &a, closes, closes + SHIFT_PAD_S) else {
            // The run ended inside the new gear's settling time, so there is
            // no baseline and nothing honest to report.
            continue;
        };
        let against_baseline: Vec<f64> = a.iter().map(|value| a_post - value).collect();
        let speed_deficit_ms = trapezoid(&t, &against_baseline, opens, closes);
        let cost_on_mark_s =
            (accel_at_mark_top > 0.0).then(|| speed_deficit_ms / accel_at_mark_top);
        out.push(Shift { t: when, from, to, speed_deficit_ms, cost_on_mark_s });
    }
    out
}

/// Whether a gear label names a numbered gear at all.
///
/// By parsing, not by matching a list of words: "not engaged" and "R" are this
/// car's labels and the next car's are its own. What travels between cars is
/// that a gear a ratio can be measured in is written as a number.
fn is_numbered_gear(label: &str) -> bool {
    label.trim().parse::<f64>().is_ok()
}

/// The mean of a series over a time range, or `None` if it holds no samples.
fn mean_between(t: &[Seconds], v: &[f64], from: Seconds, to: Seconds) -> Option<f64> {
    let mut sum = 0.0;
    let mut count = 0usize;
    for i in 0..t.len() {
        if t[i] >= from && t[i] <= to {
            sum += v[i];
            count += 1;
        }
    }
    (count > 0).then(|| sum / count as f64)
}

/// Trapezoidal integration over each interval's **own** step, clipped to
/// `[from, to]` with the endpoints interpolated.
///
/// Never a nominal `1/hz`: the samples are unevenly spaced, and an integrator
/// that assumed otherwise would silently rescale the answer by whatever the
/// poll loop happened to be achieving.
fn trapezoid(t: &[Seconds], v: &[f64], from: Seconds, to: Seconds) -> f64 {
    if t.len() < 2 || to <= from {
        return 0.0;
    }
    let mut total = 0.0;
    for i in 1..t.len() {
        let (t0, t1) = (t[i - 1], t[i]);
        if t1 <= t0 {
            continue;
        }
        let lo = t0.max(from);
        let hi = t1.min(to);
        if hi <= lo {
            continue;
        }
        let at = |x: Seconds| v[i - 1] + (v[i] - v[i - 1]) * (x - t0) / (t1 - t0);
        total += 0.5 * (at(lo) + at(hi)) * (hi - lo);
    }
    total
}

/// How far the car travelled between two times, in metres, from a speed track
/// **in m/s**.
///
/// The integrator is not the caveat. Composite-trapezoid error over a 10 s run
/// is about a millimetre; the real distance error is the speed signal, and it
/// has three multiplicative parts thousands of times larger — the speedometer's
/// legislated optimism, driven-wheel slip at the launch, and the rolling
/// circumference the unit is calibrated with, which moves with tyre size, with
/// wear and with pressure. Nobody should reach for Simpson's rule over this.
pub fn distance_m(track: &Track, from: Seconds, to: Seconds) -> f64 {
    trapezoid(&track.t, &track.v, from, to)
}

/// How many intervals between changes it takes before a middle one means
/// anything. Two is the least that has one.
const MIN_REFRESH_INTERVALS: usize = 2;

/// An **upper bound** on the control unit's refresh period for this channel,
/// from the intervals between consecutive *distinct* values.
///
/// It is a bound and not a measurement, and the distinction is not pedantry.
/// The unit refreshes the identifier on its own schedule, asynchronous to our
/// polling. Poll faster than it refreshes and the observed interval between
/// distinct values is the refresh period; poll slower and every reading differs,
/// so the observed interval is *our* period, which is the longer of the two.
/// Either way the answer is at least the refresh period — which is the useful
/// direction, since it feeds [`rolling_mark_sigma`] and a bound there gives a
/// conservative ± rather than a flattering one. When the two periods are close,
/// as they are on a car polled about as fast as its gearbox updates, a bound is
/// all this can ever be, and it should be shown to one significant figure with
/// a flag rather than three digits of false precision.
///
/// The comparison between values is exact on purpose: a reading is an integer
/// times the catalog's scaling factor, so two readings are the same reading or
/// they are not, and a tolerance would only invent a third case.
///
/// `None` when the channel showed fewer than [`MIN_REFRESH_INTERVALS`]
/// intervals — a track that never changed says nothing about how often it could
/// have.
pub fn refresh_bound(track: &Track) -> Option<Seconds> {
    let mut intervals = Vec::new();
    let mut previous_change: Option<Seconds> = None;
    for i in 1..track.len() {
        if track.v[i] != track.v[i - 1] {
            if let Some(previous) = previous_change {
                intervals.push(track.t[i] - previous);
            }
            previous_change = Some(track.t[i]);
        }
    }
    if intervals.len() < MIN_REFRESH_INTERVALS {
        return None;
    }
    intervals.sort_by(f64::total_cmp);
    Some(intervals[intervals.len() / 2])
}

/// The 1σ uncertainty of a rolling mark, `√2·T_refresh/√12`.
///
/// Both endpoints of a rolling mark are interpolated crossings of a signal that
/// is stale by an unknown amount, and the *mean* staleness is the same at both,
/// so it cancels in the difference — simulated at 24.6 ms and 25.0 ms on the
/// two ends and 0.4 ms on the difference, independent of how hard the car is
/// accelerating. What is left is the jitter: staleness uniform on `0…T_refresh`
/// has standard deviation `T_refresh/√12`, and two independent endpoints add in
/// quadrature for the `√2`.
///
/// **It depends on the refresh period and not on our sample interval.** Halving
/// the poll interval at a fixed refresh moves it by nothing, and an earlier
/// draft that asserted the opposite was asserting a falsehood.
///
/// This is the whole of the printed uncertainty on a rolling mark. A 0-based
/// mark has no such formula, because its lower endpoint is not a crossing but
/// [`start`], whose error is dominated by a dead band that is not observable
/// from the bus at all.
pub fn rolling_mark_sigma(refresh: Seconds) -> Seconds {
    std::f64::consts::SQRT_2 * refresh / 12.0_f64.sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A track from a closed-form `v(t)`, sampled on a uniform grid.
    fn sampled(hz: f64, seconds: Seconds, v: impl Fn(Seconds) -> f64) -> Track {
        let dt = 1.0 / hz;
        let mut track = Track::default();
        for i in 0..(seconds * hz) as usize {
            let t = i as f64 * dt;
            track.push(t, v(t));
        }
        track
    }

    // ---- the slope estimator ------------------------------------------

    #[test]
    fn a_ramp_is_recovered_from_each_samples_own_timestamp() {
        // Deliberately uneven spacing — 30 ms then 90 ms, repeating — which is
        // what a two-batch poll loop actually produces. An endpoint difference
        // taken over a nominal step would read this ramp wrong by whatever the
        // jitter was; a fit against the real timestamps reads it exactly.
        let mut track = Track::default();
        let mut t = 0.0;
        for i in 0..40 {
            track.push(t, 3.5 * t);
            t += if i % 2 == 0 { 0.03 } else { 0.09 };
        }
        let fit = slope(&track, 20, 0.3, Scheme::Central).unwrap();
        assert!((fit.a - 3.5).abs() < 1e-9, "{fit:?}");
        assert!(fit.sigma < 1e-9, "an exact ramp leaves no residual: {fit:?}");
    }

    #[test]
    fn the_fit_reports_at_the_centroid_so_the_causal_lag_is_exact() {
        let track = sampled(20.0, 3.0, |t| 3.0 * t);
        let i = 30;
        let fit = slope(&track, i, 0.32, Scheme::Causal).unwrap();
        // Seven samples at 50 ms span 0.30 s and their centroid is the middle
        // one, so the lag is half the *span* — a measured quantity — rather
        // than half the window, which is only nominal.
        assert!((track.t[i] - fit.t - fit.span / 2.0).abs() < 1e-12, "{fit:?}");
        assert!((track.t[i] - fit.t - 0.15).abs() < 1e-9, "{fit:?}");
    }

    #[test]
    fn a_window_holding_fewer_than_three_samples_has_no_slope() {
        let track = sampled(20.0, 1.0, |t| 3.0 * t);
        // 0.08 s at 20 Hz reaches back over one sample and holds two.
        assert_eq!(slope(&track, 10, 0.08, Scheme::Causal), None);
        assert_eq!(slope(&track, 999, 0.3, Scheme::Central), None);
    }

    #[test]
    fn causal_and_central_differ_in_phase_and_not_in_magnitude() {
        // On a uniform grid a 0.32 s causal window at sample i and a central
        // one at sample i−3 hold the *same seven samples*, so the two schemes
        // are the same arithmetic reported at the same instant. Attenuation is
        // a property of the window; switching to central after the run fixes
        // the lag and recovers no peak height whatever.
        let track = sampled(20.0, 3.0, |t| 3.0 * t + 0.8 * (t / 1.5 * std::f64::consts::TAU).sin());
        let causal = slope(&track, 30, 0.32, Scheme::Causal).unwrap();
        let central = slope(&track, 27, 0.32, Scheme::Central).unwrap();
        assert_eq!(causal.a, central.a);
        assert_eq!(causal.t, central.t);
        assert_eq!(causal.span, central.span);
        // The phase difference is the whole of the difference: the causal
        // scheme reaches that same answer three samples later.
        assert!(track.t[30] > track.t[27]);
    }

    #[test]
    fn the_window_costs_what_a_differentiator_costs_and_not_what_a_boxcar_would() {
        // A half-cosine dip 0.3 s wide and 3.0 m/s² deep riding on 4 m/s²,
        // sampled at 21 Hz and differentiated over a 0.3 s window. Integrated
        // in closed form, so the trace carries no numerical error of its own.
        //
        // Its fundamental has period 0.6 s, so f = 1.667 Hz and x = πfW = π/2.
        // The least-squares differentiator's response there is
        // (3/x²)|sin x/x − cos x| = 0.774, i.e. 2.32 m/s² for a pure tone; an
        // isolated pulse comes back a little lower still, at 2.21, because its
        // higher harmonics are attenuated harder. A boxcar smoother's sinc
        // would have claimed 0.637 and 1.91 — and, at f = 1/W, a perfect null
        // where the true response is still 0.30.
        let (base, depth, width, centre) = (4.0, 3.0, 0.3, 2.0);
        let k = depth * width / std::f64::consts::PI;
        let track = sampled(21.0, 4.0, |t| {
            let s = t - centre;
            if s < -width / 2.0 {
                base * t
            } else if s > width / 2.0 {
                base * t - 2.0 * k
            } else {
                base * t - k * ((std::f64::consts::PI * s / width).sin() + 1.0)
            }
        });

        let series = accel_series(&track, 0.3, Scheme::Central);
        let lowest = series.iter().map(|s| s.a).fold(f64::INFINITY, f64::min);
        let recovered = base - lowest;
        assert!((recovered - 2.21).abs() < 0.05, "{recovered}");
        assert!(recovered > 2.0, "sinc would have said 1.91: {recovered}");
    }

    #[test]
    fn a_one_sided_fit_at_the_edge_is_visibly_noisier_than_an_interior_one() {
        // The same alternating ±0.01 m/s perturbation everywhere, so the only
        // thing that changes between the two fits is how much baseline each
        // has: Σ(tᵢ−t̄)² is what the standard error divides by, and at the
        // first sample it is a fraction of what it is in the interior.
        let mut track = Track::default();
        for i in 0..60 {
            let t = i as f64 * 0.05;
            track.push(t, 3.0 * t + if i % 2 == 0 { -0.01 } else { 0.01 });
        }
        let edge = slope(&track, 0, 0.31, Scheme::Central).unwrap();
        let interior = slope(&track, 20, 0.31, Scheme::Central).unwrap();
        assert!(edge.span < interior.span);
        assert!(edge.sigma > 2.0 * interior.sigma, "{edge:?} vs {interior:?}");
    }

    // ---- the launch ---------------------------------------------------

    /// A launch under a wheel-speed dead band: everything below `cut_kmh`
    /// reads zero, which is what a signal from a toothed wheel does at walking
    /// pace. `v` is in m/s.
    fn under_dead_band(cut_kmh: f64, v: impl Fn(Seconds) -> f64) -> Track {
        let mut track = Track::default();
        for i in 0..80 {
            let t = i as f64 * 0.05;
            let value = v(t);
            track.push(t, if value * 3.6 < cut_kmh { 0.0 } else { value });
        }
        track
    }

    #[test]
    fn the_launch_reaches_back_past_the_last_zero_sample() {
        // Constant jerk of 8 m/s³ from rest at t = 0, with everything under
        // 2 km/h suppressed. The signal wakes at 0.40 s, so the last zero
        // sample is at 0.35 — and the truth is 0.00, before both. A clamp into
        // (last zero, first non-zero] would bound the bracket into a region
        // that provably excludes the answer, which is why there is none.
        //
        // On its own model the constant-jerk fit is exact, so here it lands on
        // the truth and forms the early end of the bracket. Constant
        // acceleration is deliberately not this test: linear extrapolation is
        // exact there and it would prove nothing about either estimator.
        let track = under_dead_band(2.0, |t| 0.5 * 8.0 * t * t);
        let launch = start(&track).unwrap();
        assert!((launch.earliest - 0.0).abs() < 1e-6, "{launch:?}");
        assert!(launch.earliest < 0.35, "the last zero sample is at 0.35: {launch:?}");
        assert!(launch.latest > launch.earliest, "{launch:?}");
    }

    #[test]
    fn the_reported_launch_is_the_middle_of_the_bracket() {
        let track = under_dead_band(2.0, |t| 0.5 * 8.0 * t * t);
        let launch = start(&track).unwrap();
        assert!((launch.t - 0.5 * (launch.earliest + launch.latest)).abs() < 1e-12);
        // And the bracket's width is what a caller prints as an interval: the
        // dead band cost the run something between nothing and a fifth of a
        // second that nobody watched.
        assert!(launch.latest - launch.earliest > 0.15, "{launch:?}");
    }

    #[test]
    fn the_late_bound_is_never_placed_after_the_first_moving_sample() {
        // The one bound that is sound rather than modelled: the car was seen
        // moving at 0.25 s, so it was already under way by then whatever any
        // fit says.
        //
        // A launch that builds faster than constant jerk makes the quadratic
        // fit say otherwise. Here v = 100·(t−0.20)³, so √v is convex rather
        // than straight and the least-squares line through it reaches zero at
        // 0.256 s — after the first moving sample at 0.25, and after the
        // two-point bound at 0.243. The impossible part is cut by the one hard
        // fact available (the car was seen moving at 0.25) and the two ends are
        // then ordered, so the two estimators disagreeing widens the interval
        // instead of collapsing it.
        let mut track = Track::default();
        for i in 0..20 {
            let t = 0.20 + i as f64 * 0.05;
            track.push(t, 100.0 * (t - 0.20).powi(3));
        }
        let launch = start(&track).unwrap();
        assert!((launch.earliest - 0.242_857).abs() < 1e-5, "{launch:?}");
        assert!((launch.latest - 0.25).abs() < 1e-9, "{launch:?}");
        assert!(launch.earliest < launch.latest, "a disagreement is not a certainty: {launch:?}");
        assert!(launch.latest <= 0.25, "never after a sample that showed movement: {launch:?}");
        assert!((launch.t - 0.5 * (launch.earliest + launch.latest)).abs() < 1e-12);
    }

    #[test]
    fn a_car_that_gains_no_speed_over_the_fit_window_has_no_launch() {
        // A trace that jumps to a steady speed and holds it has no rise for
        // either estimator to reach back through, and inventing one from a flat
        // line would put the answer wherever the arithmetic overflowed to.
        let mut track = Track::default();
        for i in 0..20 {
            let t = i as f64 * 0.05;
            track.push(t, if t < 0.2 { 0.0 } else { 5.0 });
        }
        assert_eq!(start(&track), None);
    }

    #[test]
    fn the_bracket_holds_the_launch_between_two_estimators_that_each_miss_it() {
        // The pair is the answer and neither half is. The design used to argue
        // that a convex launch makes backwards extrapolation reach zero *late*,
        // so the error is one-signed and flatters; that is sound for a
        // **linear** extrapolation and false for the constant-jerk fit the same
        // paragraph then specified. The quadratic model forces
        // v/v̇ = (t − t₀)/2, so it overshoots backwards by about twice what a
        // straight line undershoots, and the truth sits between them.
        //
        // Both shapes below start from rest at exactly t = 0 and hide
        // everything under 2 km/h, which is where a toothed wheel wakes up.
        let saturating = under_dead_band(2.0, |t| {
            // Jerk 8 m/s³ until the acceleration saturates at 4 m/s².
            if t <= 0.5 { 0.5 * 8.0 * t * t } else { 1.0 + 4.0 * (t - 0.5) }
        });
        let exponential = under_dead_band(2.0, |t| {
            // a = 4.5·(1 − e^(−t/0.35)), integrated in closed form.
            4.5 * (t - 0.35 * (1.0 - (-t / 0.35).exp()))
        });

        for (name, track) in [("saturating", saturating), ("exponential", exponential)] {
            let launch = start(&track).unwrap();
            assert!(launch.earliest < 0.0, "{name}: the quadratic overshoots: {launch:?}");
            assert!(launch.latest > 0.0, "{name}: the straight line falls short: {launch:?}");
            assert!(launch.t > launch.earliest && launch.t < launch.latest, "{name}");
        }
    }

    #[test]
    fn a_car_that_never_moved_has_no_launch() {
        let mut track = Track::default();
        for i in 0..20 {
            track.push(i as f64 * 0.05, 0.0);
        }
        assert_eq!(start(&track), None);
    }

    // ---- the peak -----------------------------------------------------

    /// A series of slopes written directly, so the peak statistic can be tested
    /// without the differentiator's own response in the way.
    fn parabolic_series(step: Seconds, span: Seconds, curvature: f64, at: Seconds) -> Vec<Slope> {
        let mut out = Vec::new();
        let mut t = 0.0;
        while t < 4.0 {
            let u = t - at;
            out.push(Slope { a: 4.0 + 0.5 * curvature * u * u, t, span, sigma: 0.0 });
            t += step;
        }
        out
    }

    #[test]
    fn the_peak_is_a_neighbourhood_mean_and_not_the_maximum() {
        // One sample sitting 0.5 m/s² above its neighbours is what a stale
        // reading followed by a double-sized step looks like. `max` would
        // report it; the ±τ mean sees it for the single sample it is.
        let mut series = parabolic_series(0.05, 0.3, -1.0, 2.0);
        let spike = series.iter().position(|s| (s.t - 2.0).abs() < 1e-9).unwrap();
        series[spike].a += 0.5;
        let highest = series.iter().map(|s| s.a).fold(f64::NEG_INFINITY, f64::max);
        let found = peak(&series, 0.21, 0.3).unwrap();
        assert!((highest - 4.5).abs() < 1e-9);
        assert!(found.value < 4.1, "the excursion is averaged away: {found:?}");
        assert!(found.value > 3.9, "{found:?}");
    }

    #[test]
    fn the_neighbourhood_mean_under_reads_a_parabolic_peak_by_c_tau_squared_over_six() {
        // a(t) = 4 + ½c·u² with c = −1, so the mean over ±τ is 4 + c·τ²/6. At
        // τ = 0.2 that is a deficit of 6.7 mm/s² — the residual the design
        // names as now dominant, and the reason τ is a parameter rather than a
        // constant: halving it quarters this.
        //
        // A fine grid, because the law is continuous: a discrete mean of u²
        // over 2n+1 points is (τ²/3)(1 + 1/n), so at n = 40 the assertion has
        // to allow about 2.5 % for the grid itself. τ is 0.2025 so that the
        // neighbourhood is exactly ±0.200 with no sample sitting on the
        // boundary.
        let series = parabolic_series(0.005, 0.3, -1.0, 2.0);
        let found = peak(&series, 0.2025, 0.3).unwrap();
        let deficit = 4.0 - found.value;
        let predicted = 0.2 * 0.2 / 6.0;
        assert!((deficit / predicted - 1.0).abs() < 0.04, "{deficit} vs {predicted}");
        assert!((found.t - 2.0).abs() < 0.01, "{found:?}");
    }

    #[test]
    fn an_early_one_sided_fit_never_wins_the_peak_search() {
        // v = 4t − (t−2)³/6, so the true acceleration is 4 − ½(t−2)², peaking
        // at 4.0 m/s² at t = 2 s. One corrupted sample at t = 0, low by
        // 0.35 m/s, inflates every fit that can see it, and the more so the
        // less baseline the fit has: the shift is (t₀−t̄)·δ/Σ(tᵢ−t̄)², which is
        // +6·δ at the first sample, +4·δ at the second and +2.9·δ at the third.
        // So the first fit reads 4.25 m/s² and would win the argmax outright,
        // from 0.075 s into a run — exactly where a fast car's real peak lives
        // and therefore exactly where nobody would question it.
        //
        // Its span is 0.15 s against a 0.31 s window, under 0.6·W, so it never
        // enters the search. The second fit does enter, at 3.6 m/s², and loses
        // to the real peak on merit.
        let mut track = sampled(20.0, 4.0, |t| 4.0 * t - (t - 2.0).powi(3) / 6.0);
        track.v[0] -= 0.35;

        let series = accel_series(&track, 0.31, Scheme::Central);
        let unfiltered = series.iter().map(|s| s.a).fold(f64::NEG_INFINITY, f64::max);
        assert!(unfiltered > 4.2, "the corruption really does dominate: {unfiltered}");

        let found = peak(&series, 0.2, 0.31).unwrap();
        assert!(found.t > 1.5, "{found:?}");
        assert!((found.value - 4.0).abs() < 0.05, "{found:?}");
    }

    #[test]
    fn a_series_with_no_fit_wide_enough_to_measure_has_no_peak() {
        let series = parabolic_series(0.05, 0.1, -1.0, 2.0);
        assert_eq!(peak(&series, 0.2, 0.3), None);
        assert_eq!(peak(&[], 0.2, 0.3), None);
    }

    // ---- shifts -------------------------------------------------------

    /// A 1→2 upshift at t = 2.0: 4 m/s² in first, a 0.2 s torque interruption
    /// with 0.1 s ramps either side of it, then 3 m/s² in second. Piecewise
    /// linear with every breakpoint on a sample, so the trapezoid over it is
    /// exact and the expected deficit can be worked out by hand.
    fn upshift_trace() -> (States, Vec<Slope>) {
        let profile = |t: Seconds| {
            if t <= 1.80 {
                4.0
            } else if t <= 1.90 {
                4.0 - 40.0 * (t - 1.80)
            } else if t <= 2.10 {
                0.0
            } else if t <= 2.20 {
                30.0 * (t - 2.10)
            } else {
                3.0
            }
        };
        let mut accel = Vec::new();
        let mut gear = States::default();
        for i in 0..70 {
            let t = i as f64 * 0.05;
            accel.push(Slope { a: profile(t), t, span: 0.3, sigma: 0.0 });
            gear.push(t, if t < 2.0 { "1" } else { "2" });
        }
        (gear, accel)
    }

    #[test]
    fn a_shift_is_located_from_the_gear_channel_and_costed_against_the_gear_entered() {
        let (gear, accel) = upshift_trace();
        let found = shifts(&gear, &accel, 2.5);
        assert_eq!(found.len(), 1);
        let shift = &found[0];
        assert_eq!((shift.from.as_str(), shift.to.as_str()), ("1", "2"));
        assert!((shift.t - 2.0).abs() < 1e-9);

        // ∫(a_post − a) over [1.65, 2.35] with a_post = 3.0:
        //   [1.65,1.80]  (3−4)·0.15         = −0.15   the old gear's credit
        //   [1.80,1.90]  mean(−1, 3)·0.10   = +0.10
        //   [1.90,2.10]  3·0.20             = +0.60
        //   [2.10,2.20]  mean(3, 0)·0.10    = +0.15
        //   [2.20,2.35]  0                  =  0
        //                                     = 0.70 m/s
        assert!((shift.speed_deficit_ms - 0.70).abs() < 1e-9, "{shift:?}");
        assert!((shift.cost_on_mark_s.unwrap() - 0.70 / 2.5).abs() < 1e-9, "{shift:?}");
    }

    #[test]
    fn the_baseline_is_the_gear_entered_and_not_the_one_left() {
        // Charging the change against the 4 m/s² it left rather than the
        // 3 m/s² it entered adds a whole extra m/s² across the 0.7 s window:
        // 0.70 m/s becomes 1.40, doubling the cost on this trace and
        // overstating a realistic one by about a third.
        let (gear, accel) = upshift_trace();
        let shift = shifts(&gear, &accel, 2.5).remove(0);
        let against_the_old_gear = shift.speed_deficit_ms + (4.0 - 3.0) * 2.0 * SHIFT_PAD_S;
        assert!((against_the_old_gear - 1.40).abs() < 1e-9);
        assert!(shift.speed_deficit_ms < against_the_old_gear);
    }

    #[test]
    fn the_cost_divides_by_the_acceleration_at_the_marks_top_not_at_the_shift() {
        // The deficit is taken where the car is pulling 4 m/s²; the mark's
        // upper endpoint is up in a tall gear at 1.5. Dividing by the wrong one
        // understates the cost by nearly a factor of three here, and by about
        // two on a real 0-100.
        let (gear, accel) = upshift_trace();
        let at_the_top = shifts(&gear, &accel, 1.5).remove(0).cost_on_mark_s.unwrap();
        let at_the_shift = shifts(&gear, &accel, 4.0).remove(0).cost_on_mark_s.unwrap();
        assert!((at_the_top - 0.70 / 1.5).abs() < 1e-9);
        assert!(at_the_top > 2.5 * at_the_shift);
    }

    #[test]
    fn a_run_that_never_changed_gear_reports_no_shift() {
        // The point of reading the gear rather than thresholding the
        // acceleration: this trace has the same dip in it, and no shift. A
        // threshold would report a cost where nothing happened.
        let (_, accel) = upshift_trace();
        let mut gear = States::default();
        for i in 0..70 {
            gear.push(i as f64 * 0.05, "3");
        }
        assert!(shifts(&gear, &accel, 2.5).is_empty());
    }

    #[test]
    fn a_level_that_is_not_a_numbered_gear_is_not_a_shift() {
        // This car's enum is [[0,"not engaged"],[2,"1"],…,[12,"R"]] — the codes
        // are neither contiguous nor ordered by ratio, and two of the levels
        // are not gears. Nothing here names a label: what is asked is whether
        // it parses as a number, which is what a gear a ratio can be measured
        // in looks like on any car.
        let (_, accel) = upshift_trace();
        let mut gear = States::default();
        for i in 0..70 {
            let t = i as f64 * 0.05;
            gear.push(t, if t < 2.0 { "not engaged" } else { "R" });
        }
        assert!(shifts(&gear, &accel, 2.5).is_empty());

        let mut half = States::default();
        for i in 0..70 {
            let t = i as f64 * 0.05;
            half.push(t, if t < 2.0 { "not engaged" } else { "1" });
        }
        assert!(shifts(&half, &accel, 2.5).is_empty());
    }

    #[test]
    fn a_shift_at_the_very_end_of_a_run_has_no_baseline_and_is_not_reported() {
        let (_, accel) = upshift_trace();
        let mut gear = States::default();
        for i in 0..70 {
            let t = i as f64 * 0.05;
            gear.push(t, if t < 3.30 { "1" } else { "2" });
        }
        assert!(shifts(&gear, &accel, 2.5).is_empty());
    }

    #[test]
    fn a_cost_on_a_car_that_is_not_gaining_speed_is_absent_rather_than_a_nan() {
        // The deficit is still a measurement; what it is worth in seconds is
        // not, and this figure is serialised into the session file, where a NaN
        // either fails the write or arrives as `null` depending on the path.
        let (gear, accel) = upshift_trace();
        let shift = shifts(&gear, &accel, 0.0).remove(0);
        assert_eq!(shift.cost_on_mark_s, None);
        assert!((shift.speed_deficit_ms - 0.70).abs() < 1e-9);
    }

    // ---- distance -----------------------------------------------------

    #[test]
    fn distance_uses_each_intervals_own_step() {
        // A constant 10 m/s for one second, sampled at 10 Hz for the first half
        // and once for the second. An integrator assuming a nominal 1/hz would
        // read 6.0 m if it believed 10 Hz throughout.
        let mut track = Track::default();
        for i in 0..=5 {
            track.push(i as f64 * 0.1, 10.0);
        }
        track.push(1.0, 10.0);
        assert!((distance_m(&track, 0.0, 1.0) - 10.0).abs() < 1e-12);
    }

    #[test]
    fn a_constant_acceleration_trace_integrates_to_the_closed_form_within_a_millimetre() {
        // v = 3t, so the distance to t is 1.5·t². Trapezoid is exact on a
        // linear v, which is the point: the integrator is not where the
        // distance error lives.
        let track = sampled(20.0, 10.0, |t| 3.0 * t);
        let covered = distance_m(&track, 0.0, 9.95);
        assert!((covered - 1.5 * 9.95 * 9.95).abs() < 1e-3, "{covered}");
    }

    #[test]
    fn distance_is_clipped_to_the_range_and_never_extrapolated_past_the_samples() {
        let track = sampled(20.0, 4.0, |t| 3.0 * t);
        // Endpoints that land between samples, against the closed form.
        let part = distance_m(&track, 1.0, 2.0);
        assert!((part - (1.5 * 4.0 - 1.5)).abs() < 1e-9, "{part}");
        // Asking beyond the last sample adds nothing rather than inventing it.
        let whole = distance_m(&track, 0.0, 3.95);
        assert!((distance_m(&track, 0.0, 60.0) - whole).abs() < 1e-12);
        assert_eq!(distance_m(&track, 2.0, 1.0), 0.0);
    }

    // ---- the refresh bound and what it feeds --------------------------

    /// A channel the unit refreshes every `refresh` seconds, read every `poll`
    /// seconds — a zero-order hold sampled asynchronously, which is what every
    /// reading in this tool actually is.
    fn held(refresh: Seconds, poll: Seconds, seconds: Seconds) -> Track {
        let mut track = Track::default();
        let mut t = 0.0;
        while t < seconds {
            let step = (t / refresh).floor();
            track.push(t, 3.0 * refresh * step);
            t += poll;
        }
        track
    }

    #[test]
    fn the_refresh_bound_is_the_interval_between_distinct_values() {
        // Held for 0.15 s and polled at 20 Hz: three readings of each value,
        // and the value changes every 0.15 s.
        let track = held(0.15, 0.05, 6.0);
        let bound = refresh_bound(&track).unwrap();
        assert!((bound - 0.15).abs() < 1e-9, "{bound}");
    }

    #[test]
    fn polled_no_faster_than_the_unit_refreshes_the_answer_is_a_bound_not_a_measurement() {
        // 50 ms refresh, 60 ms poll: every reading differs from the last, so
        // what comes back is our own period and not the unit's. It is still an
        // upper bound on the refresh period, which is the useful direction —
        // the σ it feeds comes out conservative rather than flattering.
        let track = held(0.05, 0.06, 6.0);
        let bound = refresh_bound(&track).unwrap();
        assert!(bound >= 0.05, "{bound}");
        assert!((bound - 0.06).abs() < 1e-9, "{bound}");
    }

    #[test]
    fn a_channel_that_never_changed_says_nothing_about_how_often_it_could_have() {
        let mut track = Track::default();
        for i in 0..40 {
            track.push(i as f64 * 0.05, 7.0);
        }
        assert_eq!(refresh_bound(&track), None);
        // One change gives an instant and no interval; two give one interval,
        // which has no middle to take.
        track.push(2.0, 8.0);
        track.push(2.05, 9.0);
        assert_eq!(refresh_bound(&track), None);
    }

    #[test]
    fn a_rolling_marks_sigma_follows_the_refresh_period_and_not_the_sample_interval() {
        // The same channel read at 20 Hz and at 40 Hz yields the same bound and
        // therefore the same ±. An earlier draft asserted that halving the poll
        // interval improved it, which is a falsehood about a quantity the unit
        // alone controls.
        let slow = refresh_bound(&held(0.15, 0.05, 6.0)).unwrap();
        let fast = refresh_bound(&held(0.15, 0.025, 6.0)).unwrap();
        assert!((slow - fast).abs() < 1e-9, "{slow} vs {fast}");
        assert!((rolling_mark_sigma(slow) - rolling_mark_sigma(fast)).abs() < 1e-12);

        // √2·0.05/√12 = 0.0204 s, the design's 20 ms at a 50 ms refresh. It is
        // a 1σ figure; the 1st-to-99th-percentile spread is about twice it
        // either way.
        assert!((rolling_mark_sigma(0.05) - 0.020_412).abs() < 1e-6);
        // Linear in the refresh period, and zero when nothing is stale.
        assert!((rolling_mark_sigma(0.10) - 2.0 * rolling_mark_sigma(0.05)).abs() < 1e-12);
        assert_eq!(rolling_mark_sigma(0.0), 0.0);
    }
}
