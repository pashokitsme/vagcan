//! The results table — what a finished run says once the car has stopped.
//!
//! Two blocks under their own headings, and the split is the design's whole
//! point about numbers: the **measured** block holds times and the peaks of
//! channels the car itself reported, the **computed** block holds figures this
//! tool worked out — and it carries the parameters it worked them out with in
//! its heading, because the same run under a different mass is a different set
//! of numbers and a table that hides that invites the comparison it cannot
//! support.
//!
//! Nothing here reads a bus, a clock or a file. It takes a finished [`Run`],
//! recomputes the derivative layer in one pass ([`recompute`]) and formats it
//! ([`results`]). That is the design's storage rule made structural: what was
//! shown live never reaches either the table or the file, so a method can be
//! corrected afterwards without re-driving the car, and the screen and the JSON
//! cannot disagree about what happened.
//!
//! **Two kinds of mark, printed as two kinds of number.** A mark from a
//! standstill carries an interval — its lower endpoint is not a crossing but a
//! launch reconstructed by two estimators that miss from opposite sides, so the
//! truth is between them. It leads with the midpoint so there is one figure to
//! quote, and the interval stays beside it because the midpoint is not better
//! known than its ends. A rolling mark is a single figure with a real `±`,
//! computed from this run's own measured refresh period. Neither display
//! pretends to be the other, and the interval is never respelled as a `±`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::carfile;
use super::derive::{self, Scheme, Slope};
use super::power::{self, Conditions, EngineState, KMH_PER_MS, RoadLoad};
use super::session::{Mark, Run};
use super::types::{Seconds, Track};

/// One metric horsepower, DIN 66036. Stated because "hp" names two units 1.4 %
/// apart, and a power figure that does not say which is not a figure.
pub const PS_W: f64 = 735.498_75;

/// A power figure as a person reads it: horsepower first, watts in brackets.
///
/// The model computes in watts and everything stored is in watts — this is the
/// last step before the eye and nowhere else. A car is sold, compared and
/// argued about in PS, so leading with kW makes the reader do the division
/// every time; leading with PS and keeping kW in reach costs nothing.
pub(super) fn power_figure(kw: f64) -> String {
    format!("{:.0} PS ({kw:.1} kW)", kw * 1000.0 / PS_W)
}

/// The default half-width of the least-squares acceleration window.
///
/// Long enough to hold half a dozen samples at the rates this loop achieves,
/// short enough to still resolve a gearshift dip. A property of the estimator
/// rather than of any car; `--accel-window` moves it and the file records what
/// was used.
pub const ACCEL_WINDOW_S: Seconds = 0.3;

/// How far either side of the argmax a peak is averaged.
///
/// The maximum of a noisy estimator selects positive noise excursions and reads
/// about 7 % high, in the flattering direction. Averaging over a neighbourhood
/// removes that and costs a known `c·τ²/6` — −1.3 % on a broad peak, −4 % on a
/// sharp first-gear one — which is stated rather than left to be rediscovered.
pub const PEAK_TAU_S: Seconds = 0.2;

/// Ambient pressure in the ISO 2533 standard atmosphere at sea level, in bar.
///
/// Used for one thing: deciding whether a boost channel the catalog merely
/// calls `bar` is absolute or gauge. An engine that is not on boost sits at
/// ambient, so the two readings are a whole bar apart. A property of the
/// atmosphere, not of a car.
const AMBIENT_BAR: f64 = 1.013_25;

/// The parameters a computed figure needed, so the heading can name them.
///
/// A `model` of `None` is the ordinary case: the default mode computes every
/// figure that needs no parameter, which is everything except power.
#[derive(Clone, Debug)]
pub struct Setting {
    pub accel_window_s: Seconds,
    pub peak_tau_s: Seconds,
    /// The road load and the car, when a finished car file supplied both.
    pub model: Option<Model>,
    /// As written on the sidewall, for the heading.
    pub tyre: Option<String>,
    /// Where the air density in `model` came from.
    ///
    /// Three states and not two. A `bool` had room for *measured* and *stated*
    /// only, so a car that publishes no barometer — where the density is the
    /// ISO 2533 standard atmosphere and nothing was read at all — was written
    /// down as measured. Density enters drag linearly, so that is a figure the
    /// weather can move several percent labelled as one the car reported.
    pub rho_from: carfile::Source,
    /// One raw step of the pedal channel, which is what a kickdown threshold is
    /// measured in. A percentage written here would be one unit's scaling in
    /// disguise: the reference car's pedal reads 102 % at full travel.
    pub pedal_step: Option<f64>,
    /// What each channel calls its own unit, keyed by the role it fills.
    ///
    /// The table prints these and never a word of its own. Writing `rpm` where
    /// the car's catalog says `/min` is a unit this tool invented: the session
    /// file beside the table records `/min`, and a reader comparing the two has
    /// no way to tell whether the car was asked something different or the
    /// spelling was changed on the way to the screen. A channel whose unit is
    /// not known prints a bare number, which is the honest form of not knowing.
    pub units: BTreeMap<&'static str, String>,
}

impl Setting {
    /// The unit of the channel filling `key`, ready to append to a number —
    /// `" /min"`, or nothing at all for a dimensionless or unknown channel.
    fn unit_suffix(&self, key: &str) -> String {
        match self.units.get(key).map(String::as_str).unwrap_or_default() {
            "" => String::new(),
            unit => format!(" {unit}"),
        }
    }
}

/// The dynamics half of [`Setting`] — present exactly when `--full` was
/// accepted, and absent in every other case rather than filled with typical
/// values.
#[derive(Clone, Debug)]
pub struct Model {
    pub load: RoadLoad,
    pub conditions: Conditions,
}

impl Default for Setting {
    /// The windows this module defaults to, and no car in it.
    fn default() -> Setting {
        Setting {
            accel_window_s: ACCEL_WINDOW_S,
            peak_tau_s: PEAK_TAU_S,
            model: None,
            tyre: None,
            rho_from: carfile::Source::StandardAtmosphere,
            pedal_step: None,
            units: BTreeMap::new(),
        }
    }
}

/// A peak of a channel that was on the bus: the reading, and when it happened.
///
/// A plain maximum, unlike [`derive::peak`]. The argument against a maximum is
/// about *estimators* — a noisy series whose argmax selects its own noise — and
/// a value the unit reported is not an estimate of anything.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub value: f64,
    pub t: Seconds,
}

/// The derivative layer, recomputed in one pass over a finished run.
///
/// The session file's `derived` block and the results table are both this, so
/// the two cannot disagree — which is the only reason a cache of computed
/// answers is admissible at all. `stamp` names the methods that produced it, so
/// a reader running different maths knows to recompute rather than believe it.
#[derive(Clone, Debug, Default)]
pub struct Derived {
    pub stamp: String,
    /// Central least squares, valid at each fit's own centroid — never the
    /// causal series the live screen drew.
    pub accel: Vec<Slope>,
    /// Cumulative distance from the launch, as a channel in its own right: the
    /// chart page offers a distance axis, and integrating speed a second time
    /// in the page would be a second implementation of the same maths.
    pub distance: Track,
    pub distance_m: f64,
    pub peak_accel: Option<derive::Peak>,
    /// Which gear the peak happened in — a launch peak in first says something
    /// different from one in third.
    pub peak_accel_gear: Option<String>,
    pub peak_engine_speed: Option<Reading>,
    pub peak_boost: Option<Reading>,
    /// Whether the boost channel reads absolute or gauge, from what it read
    /// while the engine was not on boost.
    pub boost_reference: Option<&'static str>,
    pub shifts: Vec<derive::Shift>,
    pub power_wheel: Track,
    pub power_shaft: Track,
    pub peak_power_wheel_kw: Option<f64>,
    pub peak_power_shaft_kw: Option<f64>,
    /// One wherever the pedal was at its own observed maximum. Derived, and
    /// labelled so: no identifier for kickdown has been proven on any car this
    /// project has seen.
    pub kickdown: Option<Track>,
    /// An upper bound on the leading unit's refresh period, which is what a
    /// rolling mark's ± is computed from.
    pub refresh_s: Option<Seconds>,
    /// The mark the shift costs are charged against — the highest that closed.
    pub cost_mark: Option<(u32, u32)>,
}

impl Derived {
    /// The 1σ on a rolling mark, from this run's own measured refresh period.
    ///
    /// `None` when the channel never showed enough distinct values to bound its
    /// refresh at all, in which case the mark prints without a `±` rather than
    /// with one this run did not measure.
    pub fn rolling_sigma(&self) -> Option<Seconds> {
        self.refresh_s.map(derive::rolling_mark_sigma)
    }
}

/// Recompute everything derivable from a finished run.
///
/// Central rather than causal: the causal estimate the live screen drew lags by
/// exactly `t_now − t̄ ≈ W/2`, and the run is over, so the future half of the
/// window exists now. It recovers no peak height whatever — attenuation is a
/// property of the window and not of where it sits — and that is worth saying,
/// because it is the thing people expect it to fix.
pub fn recompute(run: &Run, setting: &Setting) -> Derived {
    let window = window_of(setting);
    let tau = tau_of(setting);
    let speed = &run.samples.speed;
    let accel = derive::accel_series(speed, window, Scheme::Central);
    let accel_track = as_track(&accel);

    let distance = cumulative_distance(speed);
    // The total goes through the shared integrator rather than the running one,
    // so the scalar in the file and the channel beside it cannot drift apart.
    let distance_m = match (speed.t.first(), speed.t.last()) {
        (Some(first), Some(last)) => derive::distance_m(speed, first.max(0.0), *last),
        _ => 0.0,
    };

    let peak_accel = derive::peak(&accel, tau, window);
    let peak_accel_gear = peak_accel.and_then(|p| run.samples.gear.at(p.t)).map(str::to_string);

    // The shift cost is a velocity deficit divided by the acceleration at the
    // **mark's upper endpoint**, not at the shift: a deficit taken at 60 km/h
    // persists all the way to 100, and dividing where the shift happened
    // understates the cost by nearly a factor of two.
    // Highest upper endpoint, and among those the longest mark: two marks that
    // end at 100 need the same divisor, and `0-100` is the one worth naming.
    let top = run
        .marks
        .iter()
        .max_by_key(|mark| (mark.to_kmh, std::cmp::Reverse(mark.from_kmh)));
    let cost_mark = top.map(|mark| (mark.from_kmh, mark.to_kmh));
    let accel_at_mark_top =
        top.and_then(|mark| accel_track.at(mark.closed_at)).unwrap_or(0.0);
    let shifts = derive::shifts(&run.samples.gear, &accel, accel_at_mark_top, window);

    let peak_engine_speed = highest(&run.samples.engine_speed);
    let boost = run.samples.others.get("boost actual");
    let peak_boost = boost.and_then(highest);
    let boost_reference = boost
        .and_then(|track| track.v.iter().copied().min_by(f64::total_cmp))
        .map(boost_reference);

    let (power_wheel, power_shaft) = match &setting.model {
        Some(model) => power_series(run, &accel_track, model, window),
        None => (Track::default(), Track::default()),
    };
    let peak_power_wheel_kw = neighbourhood_peak(&power_wheel, tau).map(|p| p.value / 1000.0);
    let peak_power_shaft_kw = neighbourhood_peak(&power_shaft, tau).map(|p| p.value / 1000.0);

    Derived {
        stamp: stamp(window, tau),
        accel,
        distance,
        distance_m,
        peak_accel,
        peak_accel_gear,
        peak_engine_speed,
        peak_boost,
        boost_reference,
        shifts,
        power_wheel,
        power_shaft,
        peak_power_wheel_kw,
        peak_power_shaft_kw,
        kickdown: kickdown(&run.samples.pedal, setting.pedal_step),
        refresh_s: derive::refresh_bound(speed),
        cost_mark,
    }
}

/// Which maths produced a `derived` block.
///
/// Every correction in this design's history arrived after the first draft, and
/// the reason they were all retrofittable is that the file keeps raw speed.
/// Recording the vintage of the maths beside it is what makes an old file
/// comparable to a new one rather than merely readable.
fn stamp(window: Seconds, tau: Seconds) -> String {
    // The shift cost is named here because it changed after files were already
    // written: a session recorded before it carries deficits taken against a
    // baseline that was never checked for steadiness, downshifts costed as
    // though they were shifts, and signs below the session's own noise. A
    // reader comparing this string against its own knows to recompute rather
    // than believe the `derived` block.
    format!(
        "t0=quadratic-and-linear accel=central-least-squares/{window:.2} peak=mean-{tau:.1}s \
         shift=steady-baseline-upshift-2sigma"
    )
}

fn window_of(setting: &Setting) -> Seconds {
    match setting.accel_window_s > 0.0 {
        true => setting.accel_window_s,
        false => ACCEL_WINDOW_S,
    }
}

fn tau_of(setting: &Setting) -> Seconds {
    match setting.peak_tau_s > 0.0 {
        true => setting.peak_tau_s,
        false => PEAK_TAU_S,
    }
}

/// A slope series as a track, so it can be interpolated onto other channels'
/// instants. The times are the fits' centroids, never the samples'.
pub fn as_track(series: &[Slope]) -> Track {
    let mut out = Track::default();
    for slope in series {
        out.push(slope.t, slope.a);
    }
    out
}

/// Distance from the launch, integrated with each interval's own `Δt`.
///
/// Never a nominal `1/hz`: the samples are unevenly spaced, which is the whole
/// reason each value carries its own timestamp. The integrator is not the
/// caveat — see [`derive::distance_m`] for the three multiplicative errors that
/// are thousands of times larger.
fn cumulative_distance(speed: &Track) -> Track {
    let mut out = Track::default();
    let mut total = 0.0;
    let mut previous: Option<(Seconds, f64)> = None;
    for i in 0..speed.len() {
        let (t, v) = (speed.t[i], speed.v[i]);
        if t < 0.0 {
            continue;
        }
        if let Some((pt, pv)) = previous {
            total += (v + pv) / 2.0 * (t - pt);
        }
        previous = Some((t, v));
        out.push(t, total);
    }
    out
}

/// The largest reading a channel gave, and when.
fn highest(track: &Track) -> Option<Reading> {
    let mut best: Option<Reading> = None;
    for i in 0..track.len() {
        let candidate = Reading { value: track.v[i], t: track.t[i] };
        if best.is_none_or(|current| candidate.value > current.value) {
            best = Some(candidate);
        }
    }
    best
}

/// The peak of a *derived* series: the mean over `±tau` of the argmax.
///
/// The same argument as [`derive::peak`] — the maximum of a noisy estimator
/// selects its own noise — applied to a series that is not a slope fit and so
/// carries no per-sample sigma of its own.
fn neighbourhood_peak(track: &Track, tau: Seconds) -> Option<Reading> {
    let top = highest(track)?;
    let near: Vec<f64> = (0..track.len())
        .filter(|i| (track.t[*i] - top.t).abs() <= tau)
        .map(|i| track.v[i])
        .collect();
    let count = near.len() as f64;
    Some(Reading { value: near.iter().sum::<f64>() / count, t: top.t })
}

/// Whether a boost channel reads absolute or gauge pressure.
///
/// The catalogs say `bar` and stop there, and the gap is the first thing a
/// knowledgeable driver checks: a healthy turbo at full load reads about 1.1
/// gauge and 2.1 absolute, so an unlabelled column has them reading a healthy
/// car as a sick one. An engine that is not on boost sits at ambient, which
/// [`AMBIENT_BAR`] puts a whole bar above zero — so the run's own minimum
/// answers the question without a table, and half a bar separates the two cases
/// with room to spare.
fn boost_reference(minimum_bar: f64) -> &'static str {
    match minimum_bar > AMBIENT_BAR / 2.0 {
        true => "abs",
        false => "gauge",
    }
}

/// One, wherever the pedal was at its own observed maximum.
///
/// **Not against a fixed 99 %**, which would be one unit's scaling written into
/// the source: the reference car's pedal is a byte scaled by 0.4, so full travel
/// reads 102 %. The threshold is this run's own maximum less one raw step, and
/// the result is labelled derived — no identifier for kickdown has been proven
/// on any car this project has seen, and a column that silently guesses is worse
/// than an empty one.
fn kickdown(pedal: &Track, step: Option<f64>) -> Option<Track> {
    let step = step?;
    if pedal.is_empty() || step <= 0.0 {
        return None;
    }
    let top = pedal.v.iter().copied().max_by(f64::total_cmp)?;
    let threshold = top - step;
    let mut out = Track::default();
    for i in 0..pedal.len() {
        out.push(pedal.t[i], f64::from(u8::from(pedal.v[i] >= threshold)));
    }
    Some(out)
}

/// Power at the contact patch, and power including the engine-side inertia.
///
/// Everything is computed on the **leading channel's grid**, with every other
/// input interpolated onto it: channels are sampled at different rates and at
/// different instants, and a derived value that ignores that is comparing two
/// different moments.
///
/// The engine-side term is suppressed while the clutch slips — the energy the
/// engine releases there goes into the clutch as heat, not to the road — which
/// is why the shaft series is shorter than the wheel one through every launch.
fn power_series(run: &Run, accel: &Track, model: &Model, window: Seconds) -> (Track, Track) {
    let speed = &run.samples.speed;
    let mut omega = Track::default();
    for i in 0..run.samples.engine_speed.len() {
        omega.push(
            run.samples.engine_speed.t[i],
            power::omega_from_rpm(run.samples.engine_speed.v[i]),
        );
    }
    let omega_dot = as_track(&derive::accel_series(&omega, window, Scheme::Central));
    let ratios = power::Ratios::learn(&omega, speed, &run.samples.gear);

    let (mut wheel, mut shaft) = (Track::default(), Track::default());
    for i in 0..speed.len() {
        let (t, v) = (speed.t[i], speed.v[i]);
        if t < 0.0 {
            continue;
        }
        let Some(a) = accel.at(t) else { continue };
        let engine = match (omega.at(t), omega_dot.at(t), run.samples.gear.at(t)) {
            (Some(w), Some(dot), Some(gear)) if !ratios.slipping(gear, w, v) => {
                Some(EngineState { omega: w, omega_dot: dot })
            }
            _ => None,
        };
        let p = power::power(v, a, engine, &model.load, &model.conditions);
        wheel.push(t, p.wheel_w);
        if let Some(w) = p.shaft_w {
            shaft.push(t, w);
        }
    }
    (wheel, shaft)
}

/// The two-block results table for one finished run.
///
/// Pure formatting: everything numeric arrived in [`Derived`], and the only
/// decisions taken here are about how a number is spelled — which is not a
/// detail, since a mark's own precision is what says how much of it to believe.
pub fn results(run: &Run, derived: &Derived, setting: &Setting) -> String {
    let mut out = String::new();
    measured_block(&mut out, run, derived, setting);
    computed_block(&mut out, run, derived, setting);
    out
}

/// Times, and the peaks of channels the car itself reported.
fn measured_block(out: &mut String, run: &Run, derived: &Derived, setting: &Setting) {
    let aborted = if run.aborted { " (aborted)" } else { "" };
    let _ = writeln!(out, "  Run {} — measured{aborted}", run.index);

    let sigma = derived.rolling_sigma();
    let rows: Vec<(String, String, String)> = run
        .marks
        .iter()
        .map(|mark| {
            (
                format!("{}-{}", mark.from_kmh, mark.to_kmh),
                mark_time(mark, sigma),
                avg_accel(mark),
            )
        })
        .collect();

    let name_w = rows.iter().map(|r| r.0.len()).chain([11]).max().unwrap_or(11);
    let time_w = rows.iter().map(|r| r.1.len()).chain([4]).max().unwrap_or(4);
    let _ = writeln!(
        out,
        "    {:<name_w$}   {:<time_w$}   average acceleration",
        "mark (km/h)", "time"
    );
    for (name, time, accel) in &rows {
        let _ = writeln!(out, "    {name:<name_w$}   {time:<time_w$}   {accel}");
    }

    if run.marks.iter().any(Mark::starts_at_launch) {
        let _ = writeln!(
            out,
            "\n    A mark from a standstill carries a range: the car is already rolling\n\
             \x20   before its own speed signal wakes up, and where inside that gap it\n\
             \x20   started cannot be recovered. The two ways of extrapolating back to zero\n\
             \x20   err in opposite directions, so the answer is between them. The figure in\n\
             \x20   front is the middle of the range — quotable, and no better known than\n\
             \x20   the range. It is not a ±: nothing here is more likely in the centre."
        );
        if let Some(mark) = run.marks.iter().find(|mark| !mark.starts_at_launch()) {
            let _ = writeln!(
                out,
                "    {}-{} starts from a real crossing and has no such gap.",
                mark.from_kmh, mark.to_kmh
            );
        }
        let _ = writeln!(out);
    }

    if let Some(peak) = derived.peak_engine_speed {
        // The channel's own unit, whatever it is. This row used to print `rpm`
        // on the grounds that a driver says rpm, which renamed the catalog's
        // `/min` — the one thing about a measurement nobody may quietly change.
        let _ = writeln!(
            out,
            "    peak engine speed   {:.0}{} at {:.1} s",
            peak.value,
            setting.unit_suffix("engine speed"),
            peak.t
        );
    }
    if let (Some(peak), Some(reference)) = (derived.peak_boost, derived.boost_reference) {
        // Likewise `bar`, which was the reference car's catalog spelling written
        // into the source: a unit that answers in mbar would have read a
        // thousandfold wrong and said nothing about it.
        let _ = writeln!(
            out,
            "    peak boost          {:.2}{} {reference} at {:.1} s",
            peak.value,
            setting.unit_suffix("boost actual"),
            peak.t
        );
    }
}

/// Figures this tool worked out, under a heading naming what it worked them out
/// with.
fn computed_block(out: &mut String, run: &Run, derived: &Derived, setting: &Setting) {
    let _ =
        writeln!(out, "  Run {} — computed   ({})", run.index, conditions(setting, derived));

    let _ = writeln!(
        out,
        "    distance            {:.0} m      ±3 % — the car's own speed signal, not the maths",
        derived.distance_m
    );
    if let Some(peak) = derived.peak_accel {
        let gear = match &derived.peak_accel_gear {
            Some(gear) => format!(" in gear {gear}"),
            None => String::new(),
        };
        let _ = writeln!(
            out,
            "    peak acceleration   {:.1} m/s²  ({:.2} g) at {:.2} s{gear}  (mean over ±{:.1} s)",
            peak.value,
            peak.value / power::G,
            peak.t,
            tau_of(setting)
        );
    }
    if let Some(kw) = derived.peak_power_wheel_kw {
        let _ = writeln!(out, "    peak power, wheel   {}    estimate", power_figure(kw));
    }
    if let Some(kw) = derived.peak_power_shaft_kw {
        let _ = writeln!(
            out,
            "    peak power, shaft   {}    estimate, engine-side inertia included",
            power_figure(kw)
        );
    }
    for shift in &derived.shifts {
        // A deficit that does not clear the session's own noise is printed as
        // the noise and not as itself: at 14 Hz the reference car's 1→2 came out
        // at −0.04 m/s against a ±0.03 floor, and "−0.1 km/h" reads as a
        // measured sign where there is none.
        let sigma = shift.deficit_sigma_ms * KMH_PER_MS;
        if !shift.resolved() {
            // The bar, not the σ. `resolved` clears at two σ and the browser
            // page prints the same doubled figure, so printing one σ here left
            // the two saying 0.10 and 0.20 about one shift with no way for a
            // reader to tell they agreed.
            let bar = sigma * derive::DEFICIT_RESOLVED_SIGMAS;
            let _ = writeln!(
                out,
                "    shift {}→{}           cost under ±{bar:.2} km/h — below what this \
                 session could resolve",
                shift.from, shift.to
            );
            continue;
        }
        let cost = match (shift.cost_on_mark_s, derived.cost_mark) {
            (Some(seconds), Some((from, to))) => format!(", {seconds:.2} s on the {from}-{to}"),
            _ => String::new(),
        };
        let _ = writeln!(
            out,
            "    shift {}→{}           cost {:.1} ±{sigma:.1} km/h{cost}",
            shift.from,
            shift.to,
            shift.speed_deficit_ms * KMH_PER_MS
        );
    }
}

/// The parameters in the computed heading.
///
/// A run under a different mass is a different set of numbers, and two runs
/// compared across a silent change in how the car was described is exactly the
/// comparison this table exists to prevent.
fn conditions(setting: &Setting, derived: &Derived) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = &setting.model {
        parts.push(format!("mass {:.0} kg", model.conditions.mass_kg));
    }
    if let Some(tyre) = &setting.tyre {
        parts.push(format!("tyre {tyre}"));
    }
    if let Some(model) = &setting.model {
        parts.push(format!("CdA {:.2} m²", model.load.cda));
        parts.push(format!("Crr {:.4}", model.load.crr));
        parts.push(format!(
            "ρ {:.2} kg/m³ {}",
            model.conditions.rho,
            setting.rho_from.as_str()
        ));
        parts.push(format!("grade {:.0} %", model.conditions.grade_percent));
    }
    parts.push(format!("window {:.2} s, central", window_of(setting)));
    if let Some(refresh) = derived.refresh_s {
        // A bound and not a measurement: when the poll interval and the refresh
        // period are close, which is the ordinary case here, this is all it can
        // ever be, and three digits would be false precision.
        parts.push(format!("refresh ≤ {refresh:.2} s bound"));
    }
    parts.join(", ")
}

/// How a mark's time is spelled, which is how much of it to believe.
///
/// A mark from a standstill leads with one number so there is something to read
/// at a glance and to quote, and carries the bracket beside it. The number is
/// the bracket's midpoint and nothing more: the moment the car started was not
/// observed, and the two estimators that reach back through the speed signal's
/// dead band disagree by the width shown. It is deliberately **not** written
/// `1.19 s ± 0.05` — that spelling claims a symmetric error around a measured
/// value, and there is neither a measurement nor a reason to think the middle
/// of the interval more likely than its ends.
///
/// A rolling mark is the opposite case and gets the `±` it has earned: both
/// ends are real crossings, the dead band cancels in the difference, and what
/// is left is the leading unit's refresh period.
fn mark_time(mark: &Mark, sigma: Option<Seconds>) -> String {
    match (mark.bracket, sigma) {
        (Some(span), _) => {
            format!("{:.2} s ({:.2} … {:.2})", mark.seconds, span.earliest, span.latest)
        }
        (None, Some(sigma)) => format!("{:.2} s ± {sigma:.2}", mark.seconds),
        (None, None) => format!("{:.2} s", mark.seconds),
    }
}

/// `Δv/Δt` across the mark's own endpoints, printed to the precision it earns.
///
/// The numerator is exact by construction — `Δv` is the mark's definition — so
/// the relative error of the average *is* the relative error of the mark time:
/// 0.6 % on a rolling mark, 2–4 % on `0-100`, and 10–25 % on `0-10`, where the
/// launch bias dominates. A launch-based figure therefore gets one decimal and a
/// rolling one gets two, rather than both being printed as if they were equally
/// well known.
fn avg_accel(mark: &Mark) -> String {
    match mark.starts_at_launch() {
        true => format!("{:.1} m/s²", mark.avg_accel_ms2()),
        false => format!("{:.2} m/s²", mark.avg_accel_ms2()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::session::{Samples, Span};
    use crate::measure::types::States;

    /// A run built by hand: an analytic 4 m/s² launch, one upshift, three marks.
    /// No car was driven for it and no adapter exists in this file.
    fn run() -> Run {
        let mut samples = Samples::default();
        let a = 4.0;
        let mut t = -1.0;
        while t <= 8.0 {
            let v = if t <= 0.0 { 0.0 } else { a * t };
            samples.speed.push(t, v);
            samples.engine_speed.push(t, 1000.0 + 800.0 * v.min(20.0) / 4.0);
            samples.pedal.push(t, if t < 0.0 { 0.0 } else { 102.0 });
            samples.gear.push(t, if t < 3.0 { "1" } else { "2" });
            samples
                .others
                .entry("boost actual")
                .or_default()
                .push(t, if t < 0.0 { 1.0 } else { 2.1 });
            t += 0.05;
        }
        Run {
            index: 2,
            samples,
            launch: None,
            marks: vec![
                Mark {
                    from_kmh: 0,
                    to_kmh: 10,
                    closed_at: 0.694,
                    seconds: 0.694,
                    bracket: Some(Span { earliest: 0.60, latest: 0.79 }),
                },
                Mark {
                    from_kmh: 0,
                    to_kmh: 100,
                    closed_at: 6.944,
                    seconds: 6.944,
                    bracket: Some(Span { earliest: 6.85, latest: 7.04 }),
                },
                Mark {
                    from_kmh: 50,
                    to_kmh: 100,
                    closed_at: 6.944,
                    seconds: 3.472,
                    bracket: None,
                },
            ],
            aborted: false,
            degraded: false,
        }
    }

    #[test]
    fn an_unresolved_shift_prints_the_same_bar_the_page_does() {
        // Both sides say a shift is unresolved below two σ, and both must print
        // the same number for it. Printing one σ here and two on the page left
        // the text report saying 0.10 km/h and the page 0.20 about one shift,
        // with nothing on either to say they agreed.
        let page = include_str!("view.html");
        assert!(page.contains("var RESOLVED_SIGMAS = 2;"), "the page's bar moved");
        assert_eq!(derive::DEFICIT_RESOLVED_SIGMAS, 2.0, "and the code's bar moved with it");
    }

    #[test]
    fn a_launch_mark_prints_an_interval_and_a_rolling_one_prints_a_plus_minus() {
        // They are not the same kind of number: one endpoint of a launch mark
        // is a reconstruction between two estimators, and both endpoints of a
        // rolling mark are real crossings whose staleness cancels.
        let run = run();
        let derived = recompute(&run, &Setting::default());
        let table = results(&run, &derived, &Setting::default());
        // One number to quote, and the interval it came out of beside it.
        let launch = table.lines().find(|l| l.contains("0-100")).unwrap();
        assert!(launch.contains("6.94 s (6.85 … 7.04)"), "{launch}");
        // Never spelled as a ±: that would claim a symmetric error around a
        // measurement, and the launch is neither symmetric nor measured.
        assert!(!launch.contains('±'), "a bracket is not a ±: {launch}");
        let rolling = table.lines().find(|l| l.contains("50-100")).unwrap();
        assert!(rolling.contains(" ± "), "a rolling mark carries a real ±: {rolling}");
        assert!(!rolling.contains('…'), "and never a bracket: {rolling}");
    }

    #[test]
    fn the_unit_of_a_mark_is_stated_because_0_60_is_the_american_one() {
        // `0-60` is in mph everywhere it is famous, and the default list has it.
        let run = run();
        let derived = recompute(&run, &Setting::default());
        assert!(results(&run, &derived, &Setting::default()).contains("mark (km/h)"));
    }

    /// A run whose channels reported their units, as a resolved set does.
    fn setting_with_units() -> Setting {
        Setting {
            units: [("engine speed", "/min".to_string()), ("boost actual", "bar".to_string())]
                .into_iter()
                .collect(),
            ..Setting::default()
        }
    }

    #[test]
    fn a_peak_is_printed_in_the_unit_its_own_channel_reports_and_never_a_renamed_one() {
        // This row used to print `rpm` because that is the word a driver uses.
        // The catalog says `/min`, the session file says `/min`, and a table
        // that says something else has invented a unit — the reader comparing
        // the two cannot tell which of them the car actually reported.
        let run = run();
        let setting = setting_with_units();
        let derived = recompute(&run, &setting);
        let table = results(&run, &derived, &setting);
        assert!(table.contains("peak engine speed"), "{table}");
        assert!(table.contains(" /min at "), "{table}");
        assert!(!table.contains("rpm"), "{table}");
    }

    #[test]
    fn a_channel_that_did_not_say_what_it_measures_in_prints_a_bare_number() {
        // Not a borrowed unit: a car whose catalog leaves the unit empty gets a
        // number with nothing after it, which is the honest form of not knowing.
        let run = run();
        let derived = recompute(&run, &Setting::default());
        let table = results(&run, &derived, &Setting::default());
        let row = table.lines().find(|l| l.contains("peak engine speed")).expect("the row is there");
        assert!(row.contains(" at "), "{row}");
        assert!(!row.contains("rpm") && !row.contains("/min"), "{row}");
    }

    #[test]
    fn boost_says_whether_it_is_absolute_or_gauge() {
        // 1.6 bar absolute is part load and 1.6 gauge is a healthy full-load
        // figure; an unlabelled column reads a healthy car as a sick one.
        let run = run();
        let setting = setting_with_units();
        let derived = recompute(&run, &setting);
        assert_eq!(derived.boost_reference, Some("abs"));
        assert!(results(&run, &derived, &setting).contains("bar abs at"));

        // The same channel offset to gauge is recognised as gauge, with no
        // table and no per-car constant.
        let mut gauge = run.clone();
        let track = gauge.samples.others.get_mut("boost actual").unwrap();
        for v in &mut track.v {
            *v -= 1.0;
        }
        assert_eq!(recompute(&gauge, &Setting::default()).boost_reference, Some("gauge"));
    }

    #[test]
    fn the_computed_heading_carries_the_parameters_it_was_computed_with() {
        // The same run under a different mass is a different set of numbers.
        let run = run();
        let setting = Setting {
            model: Some(Model {
                load: RoadLoad { cda: 0.63, crr: 0.0114 },
                conditions: Conditions {
                    mass_kg: 1475.0,
                    rho: 1.19,
                    grade_percent: 0.0,
                    headwind_ms: 0.0,
                    inertias: power::Inertias { wheels_kgm2: 5.5, engine_kgm2: 0.34 },
                    radius_m: 0.313,
                },
            }),
            tyre: Some("205/55R16".to_string()),
            rho_from: carfile::Source::Measured,
            ..Setting::default()
        };
        let derived = recompute(&run, &setting);
        let table = results(&run, &derived, &setting);
        for expected in [
            "mass 1475 kg",
            "tyre 205/55R16",
            "CdA 0.63 m²",
            "Crr 0.0114",
            "ρ 1.19 kg/m³ measured",
            "window 0.30 s, central",
        ] {
            assert!(table.contains(expected), "{expected} missing from:\n{table}");
        }
        assert!(table.contains("peak power, wheel"), "{table}");
        // Horsepower leads and the watts stay in reach, and the figure still
        // says it is an estimate wherever it appears.
        assert!(table.contains(" PS ("), "power reads PS first: {table}");
        assert!(table.contains(" kW)    estimate"), "every power figure says so: {table}");
    }

    #[test]
    fn there_is_no_power_column_without_a_car_to_compute_it_for() {
        // A power figure resting on a hatchback-shaped guess is exactly the
        // number this design spends its length refusing to print.
        let run = run();
        let derived = recompute(&run, &Setting::default());
        assert_eq!(derived.peak_power_wheel_kw, None);
        let table = results(&run, &derived, &Setting::default());
        assert!(!table.contains("peak power"), "{table}");
        // And everything that needs no parameter is still there.
        assert!(table.contains("distance"), "{table}");
        assert!(table.contains("peak acceleration"), "{table}");
    }

    #[test]
    fn distance_is_a_channel_and_not_only_a_number() {
        // The chart page offers a distance axis; integrating speed a second
        // time in the page would be a second implementation of the maths.
        let run = run();
        let derived = recompute(&run, &Setting::default());
        assert!(derived.distance.len() > 100);
        // ½at² over 8 s at 4 m/s², within the trapezoid's own error.
        assert!((derived.distance_m - 128.0).abs() < 0.5, "{}", derived.distance_m);
        assert!(derived.distance.v.windows(2).all(|w| w[1] >= w[0]), "distance never falls");
    }

    #[test]
    fn a_shift_is_charged_against_the_highest_mark_that_closed() {
        // A deficit taken at 60 km/h persists to 100, so the time it costs is
        // divided by the acceleration at the mark's top, not at the shift.
        let run = run();
        let derived = recompute(&run, &Setting::default());
        assert_eq!(derived.cost_mark, Some((0, 100)));
        assert_eq!(derived.shifts.len(), 1, "{:?}", derived.shifts);
        assert_eq!((derived.shifts[0].from.as_str(), derived.shifts[0].to.as_str()), ("1", "2"));
        assert!(results(&run, &derived, &Setting::default()).contains("shift 1→2"));
    }

    #[test]
    fn a_gear_that_is_not_a_number_is_not_a_shift() {
        // The codes are neither contiguous nor ordered by ratio, and two of the
        // levels are not gears — the bug this project already made once.
        let mut run = run();
        run.samples.gear = States::default();
        for i in 0..40 {
            let t = f64::from(i) * 0.1;
            run.samples.gear.push(t, if t < 2.0 { "not engaged" } else { "R" });
        }
        let derived = recompute(&run, &Setting::default());
        assert!(derived.shifts.is_empty(), "{:?}", derived.shifts);
    }

    #[test]
    fn kickdown_needs_the_channels_own_step_and_never_a_percentage() {
        // The reference car's pedal reads 102 % at full travel, so any literal
        // threshold would be one unit's scaling written into the source.
        let run = run();
        assert!(recompute(&run, &Setting::default()).kickdown.is_none(), "no step, no column");
        let setting = Setting { pedal_step: Some(0.4), ..Setting::default() };
        let kickdown = recompute(&run, &setting).kickdown.expect("a step makes it derivable");
        assert!(kickdown.v.contains(&1.0), "full travel is kickdown");
        assert!(kickdown.v.contains(&0.0), "a closed pedal is not");
    }

    #[test]
    fn the_derived_block_says_which_maths_produced_it() {
        // Without the stamp a file would hold two answers and no way to choose.
        let derived = recompute(&run(), &Setting::default());
        assert!(derived.stamp.contains("central-least-squares/0.30"), "{}", derived.stamp);
        assert!(derived.stamp.contains("peak=mean-0.2s"), "{}", derived.stamp);
    }

    #[test]
    fn an_aborted_run_still_reports_the_marks_that_closed() {
        let mut run = run();
        run.aborted = true;
        run.marks.truncate(1);
        let derived = recompute(&run, &Setting::default());
        let table = results(&run, &derived, &Setting::default());
        assert!(table.contains("Run 2 — measured (aborted)"), "{table}");
        assert!(table.contains("0-10"), "{table}");
    }
}
