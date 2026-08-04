//! The dynamics behind the power column: air density, this car's own inertia
//! coefficients, the road load, and the two power figures.
//!
//! Two figures come out of here because they are two different quantities.
//! `P_wheel` is what crosses the contact patch and is the number a rolling road
//! would print. `P_shaft` adds the power going into spinning the engine and
//! clutch up — real work the engine is doing, upstream of the clutch and never
//! delivered to the road. Summing them into one figure with a paragraph of
//! apology was an earlier draft's mistake and is simply wrong.
//!
//! **The engine-side term is written exactly and never through a gear ratio.**
//! `P_shaft = P_wheel + I_engine·ω·ω̇`. The tempting alternative folds the
//! engine into an equivalent-inertia factor `k = 1 + δ₁ + δ₂·ξ²` computed from
//! a measured engine-to-wheel ratio `ξ = ω·r/v`. That is algebraically
//! identical while the clutch is locked and catastrophic while it slips: at
//! launch the engine sits at a couple of thousand rpm with the car barely
//! moving, so `ξ` diverges and the factor explodes — simulated at 330 kW on a
//! 132 kW car, from the first sample of every run. The exact form is finite
//! there because `ω̇ ≈ 0`, goes correctly negative during an upshift, and makes
//! the rolling radius cancel out of the power path entirely.
//!
//! And it is *still* suppressed while the clutch slips, because even the exact
//! form is wrong there: the energy the engine releases goes into the clutch as
//! heat rather than to the road. That is what [`Ratios`] is for.
//!
//! Nothing here knows anything about one car. Masses, radii, inertias, `CdA`
//! and `Crr` all arrive as parameters; the only literals are the four physical
//! constants at the top, each with the standard that fixes it.

use std::collections::BTreeMap;

use super::types::{States, Track};

/// Standard gravity, `9.806 65 m/s²` — fixed by the 3rd CGPM (1901).
///
/// An SI convention rather than a property of any car, any road or any place,
/// which is what makes it admissible as a literal at all.
pub const G: f64 = 9.806_65;

/// One m/s in km/h, exactly (ISO 80000-3).
///
/// Speed arrives from the bus in the unit its catalog row uses, and every
/// formula in this module is in SI, so the conversion has exactly one home.
pub const KMH_PER_MS: f64 = 3.6;

/// The specific gas constant of dry air, `287.052 87 J/(kg·K)` (ISO 2533, the
/// Standard Atmosphere).
const R_DRY_AIR: f64 = 287.052_87;

/// Zero Celsius in kelvin — the definition of the scale, not a measurement.
const KELVIN_AT_ZERO_C: f64 = 273.15;

/// Air density from the car's own barometer and ambient sensor: `ρ = p/(R·T)`.
///
/// `pressure_kpa` is the unit SAE J1979 PID 0x33 answers in (1 kPa/bit) and
/// `ambient_c` the unit PID 0x46 answers in (`A − 40 °C`), so the conversion to
/// pascals and to kelvin lives here and no caller has to remember it.
///
/// Dry air. Humidity costs at most −1.6 % on `ρ` at 30 °C and saturation, which
/// is under 0.1 kW on a 120 kW figure and far below the heat-soak in the
/// ambient sensor itself — which is why *when* the temperature is read matters
/// more than what is done with it afterwards.
pub fn air_density(pressure_kpa: f64, ambient_c: f64) -> f64 {
    // Below absolute zero the denominator changes sign and the answer would be
    // a negative density — physical nonsense that would then divide into a
    // drag figure. J1979 spells PID 0x46 as one byte offset by 40, so a real
    // reading cannot get here; a corrupted one can.
    if ambient_c <= -KELVIN_AT_ZERO_C {
        return f64::NAN;
    }
    pressure_kpa * 1_000.0 / (R_DRY_AIR * (ambient_c + KELVIN_AT_ZERO_C))
}

/// Crank speed in rad/s from the rpm a catalog row reports.
///
/// A revolution is 2π rad and a minute is 60 s; both are definitions. It is a
/// function rather than an open-coded factor so that every ratio in this module
/// is certainly in the same units.
pub fn omega_from_rpm(rpm: f64) -> f64 {
    rpm * std::f64::consts::TAU / 60.0
}

/// The rotating masses the car carries, in kg·m².
///
/// **These are the generic numbers, and the coefficients are not.** A textbook
/// quotes `δ₁ ≈ 0.04` and `δ₂ ≈ 0.0025`, but those are a typical car's inertias
/// divided by *someone else's* mass and wheels: they move by nearly a factor of
/// two across the cars this tool is meant to serve. What travels between cars
/// is the inertia, so that is what is stored — with its own provenance — and
/// [`deltas`] turns it into coefficients using this car's mass and radius.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inertias {
    pub wheels_kgm2: f64,
    pub engine_kgm2: f64,
}

/// `δ₁ = I_wheels/(m·r²)` and `δ₂ = I_engine/(m·r²)`, for *this* car.
///
/// `δ₁` is the one the power path uses: the wheels turn with the car whatever
/// the clutch is doing, so `m·(1+δ₁)·a` is the inertial force at the road.
///
/// `δ₂` is returned because the car file stores it and §3a's error budget is
/// stated in it — **not** because the power path multiplies by it. It never
/// does: the engine-side term uses `I_engine` directly, which is the whole
/// point of writing that term exactly (see the module docs).
pub fn deltas(i: &Inertias, mass_kg: f64, radius_m: f64) -> (f64, f64) {
    let denominator = mass_kg * radius_m * radius_m;
    (i.wheels_kgm2 / denominator, i.engine_kgm2 / denominator)
}

/// The road load measured on this car by the coastdown, or stated by someone
/// who genuinely has the figures.
///
/// `crr` is the whole speed-independent road load, not a tyre property — see
/// [`super::coastdown`], which is where it comes from and where that is
/// explained.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadLoad {
    pub cda: f64,
    pub crr: f64,
}

/// Everything about the moment and the car that is not a channel.
///
/// `grade_percent` and `headwind_ms` are zero unless someone stated them: the
/// tool cannot see either, and an unnoticed 1 % of slope is worth ±5 PS.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Conditions {
    pub mass_kg: f64,
    pub rho: f64,
    pub grade_percent: f64,
    pub headwind_ms: f64,
    pub inertias: Inertias,
    pub radius_m: f64,
}

/// The engine's angular speed and its rate of change, in rad/s and rad/s².
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EngineState {
    pub omega: f64,
    pub omega_dot: f64,
}

/// The two power figures, in watts. Both are estimates and are labelled so.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Power {
    pub wheel_w: f64,
    pub shaft_w: Option<f64>,
}

/// The sine of a gradient quoted the way roads quote it — rise over run, in
/// per cent, i.e. a tangent.
///
/// The difference from treating per cent as a sine directly is 5·10⁻⁵ at 1 %,
/// which matters to nothing here; it costs one square root to be right instead
/// of nearly right, and it means the conversion is stated once rather than
/// assumed differently in two modules.
pub fn grade_sin(percent: f64) -> f64 {
    let tangent = percent / 100.0;
    tangent / (1.0 + tangent * tangent).sqrt()
}

/// The inverse of [`grade_sin`]: a gradient in per cent from its sine.
///
/// The coastdown recovers a slope as a sine and has to report it in the units
/// the driver reads off a road sign.
pub fn grade_percent(sin_theta: f64) -> f64 {
    let clamped = sin_theta.clamp(-0.999_999, 0.999_999);
    100.0 * clamped / (1.0 - clamped * clamped).sqrt()
}

/// The force the air and the road ask for at a given speed, in newtons.
///
/// Note the asymmetry the power formula depends on: **drag acts on air speed,
/// power is delivered against ground speed.** The drag term is written
/// `|u|·u` rather than `u²` so that a tailwind stronger than the car pushes
/// instead of dragging — it costs nothing, and the squared form is silently
/// wrong there.
pub fn road_force(speed_ms: f64, load: &RoadLoad, c: &Conditions) -> f64 {
    let air_speed = speed_ms + c.headwind_ms;
    0.5 * c.rho * load.cda * air_speed.abs() * air_speed
        + c.mass_kg * G * load.crr
        + c.mass_kg * G * grade_sin(c.grade_percent)
}

/// The power at the contact patch, and the power at the shaft if the engine
/// side can be trusted.
///
/// ```text
/// P_wheel = ( m·(1+δ₁)·a + ½·ρ·CdA·(v + v_head)² + m·g·Crr + m·g·sin θ ) · v
/// P_shaft = P_wheel + I_engine·ω·ω̇
/// ```
///
/// `engine` is `None` when there is no engine data for this instant **or when
/// the clutch is slipping**, and the caller is the one that knows which: it
/// asks [`Ratios::slipping`] and hands `None` over. This function is
/// arithmetic and takes no view — but a caller that skips that question will
/// report the clutch's heat as the engine's output through every launch.
pub fn power(
    speed_ms: f64,
    accel_ms2: f64,
    engine: Option<EngineState>,
    load: &RoadLoad,
    c: &Conditions,
) -> Power {
    let (delta1, _delta2) = deltas(&c.inertias, c.mass_kg, c.radius_m);
    let inertial = c.mass_kg * (1.0 + delta1) * accel_ms2;
    let wheel_w = (inertial + road_force(speed_ms, load, c)) * speed_ms;
    let shaft_w = engine.map(|e| wheel_w + c.inertias.engine_kgm2 * e.omega * e.omega_dot);
    Power { wheel_w, shaft_w }
}

/// How far a measured ratio may sit from its plateau before the clutch is
/// judged to be slipping.
///
/// A locked gear holds its ratio to a fraction of a per cent; a clutch that is
/// taking up is tens of per cent away. Five per cent is comfortably between the
/// two and is a judgement about clutches in general, not about one gearbox.
pub const SLIP_TOLERANCE: f64 = 0.05;

/// Below this speed a ratio means nothing and the answer is always "slipping".
///
/// Not a property of any car: at walking pace the ratio is a small number
/// divided by a smaller one, and no car is in a locked gear there anyway. It
/// backs the tolerance up rather than replacing it.
pub const RATIO_FLOOR_KMH: f64 = 15.0;

/// How many samples a gear needs before its plateau is believed.
const MIN_PLATEAU_SAMPLES: usize = 5;

/// What share of a gear's samples must agree with the median before the spread
/// counts as a plateau at all.
///
/// This is what keeps the levels that are not gears out of the table — "not
/// engaged" produces a ratio that wanders over an order of magnitude — without
/// naming a single label, which would be this car's enum in disguise.
const MIN_PLATEAU_AGREEMENT: f64 = 0.6;

/// The ratio plateaus this car showed, learned from its own trace, one per gear
/// label.
///
/// **There is no ratio table in this source and there must never be one.** The
/// numbers are whatever the car did while it was driving steadily, keyed by the
/// label the catalog gives the gear — never by the code behind it, which is
/// neither contiguous nor ordered by ratio.
///
/// The plateau is stored as `ω/v` in rad per metre rather than as the
/// dimensionless `ξ = ω·r/v`, because the rolling radius cancels out of every
/// comparison made with it, and a parameter that cannot change an answer has no
/// business being asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ratios(BTreeMap<String, f64>);

impl Ratios {
    /// Learn one plateau per gear from a stretch of the car's own driving.
    ///
    /// `engine_rad_s` is angular speed (see [`omega_from_rpm`]) and `speed_ms`
    /// is m/s; both must be, since the plateau is their quotient. Samples are
    /// taken on the speed channel's grid — the leading one — with the engine
    /// interpolated onto it, which is the rule every derived figure in `measure`
    /// follows.
    ///
    /// The statistic is the median, and a gear is kept only if most of its
    /// samples sit near it. That is what distinguishes a gear from a level that
    /// merely has a label.
    pub fn learn(engine_rad_s: &Track, speed_ms: &Track, gear: &States) -> Ratios {
        let mut seen: BTreeMap<String, Vec<f64>> = BTreeMap::new();
        for i in 0..speed_ms.len() {
            let (t, v) = (speed_ms.t[i], speed_ms.v[i]);
            if v * KMH_PER_MS < RATIO_FLOOR_KMH {
                continue;
            }
            let (Some(omega), Some(label)) = (engine_rad_s.at(t), gear.at(t)) else {
                continue;
            };
            seen.entry(label.to_string()).or_default().push(omega / v);
        }

        let mut plateaus = BTreeMap::new();
        for (label, mut ratios) in seen {
            if ratios.len() < MIN_PLATEAU_SAMPLES {
                continue;
            }
            ratios.sort_by(f64::total_cmp);
            let median = ratios[ratios.len() / 2];
            if median <= 0.0 {
                continue;
            }
            let agreeing = ratios
                .iter()
                .filter(|r| ((*r - median) / median).abs() <= SLIP_TOLERANCE)
                .count();
            if (agreeing as f64) < MIN_PLATEAU_AGREEMENT * ratios.len() as f64 {
                continue;
            }
            plateaus.insert(label, median);
        }
        Ratios(plateaus)
    }

    /// The plateau learned for a gear, if it was learned at all.
    pub fn plateau(&self, gear: &str) -> Option<f64> {
        self.0.get(gear).copied()
    }

    /// Whether the clutch is slipping — that is, whether the engine-side power
    /// term has to be thrown away for this sample.
    ///
    /// True below the speed floor, true for a gear whose plateau was never
    /// learned, and true when the measured ratio is more than
    /// [`SLIP_TOLERANCE`] from that plateau. Every uncertain case answers
    /// "slipping", because a suppressed sample costs a gap in one series and a
    /// wrong one costs a power figure that never happened.
    ///
    /// The rolling radius the plan passes here is absent on purpose: the
    /// plateau is `ω/v`, so `r` multiplies both sides of the comparison and
    /// cancels — the same cancellation that makes the exact engine-side term
    /// radius-free.
    pub fn slipping(&self, gear: &str, engine_omega: f64, speed_ms: f64) -> bool {
        // A NaN answers `false` to every comparison, including the one below,
        // so a reading that is not a number would arrive here and be called
        // *locked* — the one verdict that lets a bad sample through into a
        // power figure. Every uncertain case answers "slipping", and an
        // engine speed that is not a number is the most uncertain of all.
        if !engine_omega.is_finite() || !speed_ms.is_finite() {
            return true;
        }
        if speed_ms <= 0.0 || speed_ms * KMH_PER_MS < RATIO_FLOOR_KMH {
            return true;
        }
        let Some(plateau) = self.plateau(gear) else {
            return true;
        };
        let measured = engine_omega / speed_ms;
        ((measured - plateau) / plateau).abs() > SLIP_TOLERANCE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mid-size car's rotating inertias, from Wong, *Theory of Ground
    /// Vehicles*. They are the tool's generic input — the coefficients they
    /// produce are not, which is what the first tests below are about.
    const TYPICAL: Inertias = Inertias { wheels_kgm2: 5.5, engine_kgm2: 0.34 };

    fn conditions(mass_kg: f64, radius_m: f64) -> Conditions {
        Conditions {
            mass_kg,
            rho: 1.2,
            grade_percent: 0.0,
            headwind_ms: 0.0,
            inertias: TYPICAL,
            radius_m,
        }
    }

    #[test]
    fn air_density_matches_the_standard_atmosphere_at_sea_level() {
        // The anchor. Testing ρ = p/(R·T) against its own formula proves
        // nothing; testing it against the one value the standard publishes
        // catches a wrong R, a kelvin/celsius slip and a kPa/Pa slip at once,
        // because each of the three is a factor of tens or hundreds out.
        let rho = air_density(101.325, 15.0);
        assert!((rho - 1.2250).abs() < 1e-4, "{rho}");
    }

    #[test]
    fn the_textbook_coefficients_are_what_a_typical_car_produces_not_constants() {
        // 1400 kg on 0.313 m wheels is roughly the car the textbook divided by,
        // so its 0.04 and 0.0025 come back out. That is the whole argument for
        // storing inertias instead: the same wheels on a lighter car with
        // smaller ones give visibly different coefficients, and hardcoding
        // either number would quietly apply this car's figures to that one.
        let (d1, d2) = deltas(&TYPICAL, 1400.0, 0.313);
        assert!((d1 - 0.04).abs() < 0.001, "{d1}");
        assert!((d2 - 0.0025).abs() < 0.0001, "{d2}");

        let (light_d1, light_d2) = deltas(&TYPICAL, 1000.0, 0.28);
        assert!(light_d1 > 1.7 * d1, "{light_d1} vs {d1}");
        // The scaling is exactly 1/(m·r²) and nothing else.
        let expected = (1400.0 * 0.313 * 0.313) / (1000.0 * 0.28 * 0.28);
        assert!((light_d1 / d1 - expected).abs() < 1e-9);
        assert!((light_d2 / d2 - expected).abs() < 1e-9);
    }

    #[test]
    fn drag_acts_on_air_speed_while_power_is_delivered_against_ground_speed() {
        let load = RoadLoad { cda: 0.65, crr: 0.012 };
        let mut c = conditions(1400.0, 0.313);
        let still = power(30.0, 0.0, None, &load, &c);
        c.headwind_ms = 5.0;
        let into_wind = power(30.0, 0.0, None, &load, &c);

        // The extra force is ½ρCdA(35² − 30²) = 126.75 N, and it is delivered
        // against 30 m/s and not against 35: 3.80 kW.
        let extra = into_wind.wheel_w - still.wheel_w;
        let expected = 0.5 * 1.2 * 0.65 * (35.0f64.powi(2) - 30.0f64.powi(2)) * 30.0;
        assert!((extra - expected).abs() < 1e-6, "{extra} vs {expected}");
        assert!((extra - 3_802.5).abs() < 1.0, "{extra}");
    }

    #[test]
    fn a_tailwind_stronger_than_the_car_pushes_instead_of_dragging() {
        // |u|·u rather than u². Squaring would report a 1 m/s crawl with a
        // 5 m/s tailwind as fighting 16 m/s of air.
        let load = RoadLoad { cda: 0.65, crr: 0.0 };
        let c = Conditions { headwind_ms: -5.0, ..conditions(1400.0, 0.313) };
        assert!(road_force(1.0, &load, &c) < 0.0);
    }

    #[test]
    fn the_exact_engine_term_equals_the_ratio_form_while_the_clutch_is_locked() {
        // Locked: ω = ξ·v/r and ω̇ = ξ·a/r, so I_e·ω·ω̇ and δ₂·ξ²·m·a·v are the
        // same quantity. This is what licenses throwing the ratio form away —
        // the exact term is not an approximation of it, it is it, minus the
        // pole at v → 0.
        let (mass, radius, xi) = (1400.0, 0.313, 3.13);
        let (v, a) = (30.0, 2.0);
        let c = conditions(mass, radius);
        let load = RoadLoad { cda: 0.65, crr: 0.012 };

        let engine = EngineState { omega: xi * v / radius, omega_dot: xi * a / radius };
        let p = power(v, a, Some(engine), &load, &c);
        let exact_term = p.shaft_w.unwrap() - p.wheel_w;

        let (_, delta2) = deltas(&TYPICAL, mass, radius);
        let ratio_term = delta2 * xi * xi * mass * a * v;
        assert!(
            (exact_term - ratio_term).abs() < 1e-6 * ratio_term.abs(),
            "{exact_term} vs {ratio_term}"
        );
    }

    #[test]
    fn a_launch_on_a_slipping_clutch_produces_no_shaft_figure() {
        // The defect this whole design exists to avoid. The engine is held at
        // 2200 rpm while the car crawls at 1 km/h, which is what a clutch
        // take-up looks like from the bus.
        let (mass, radius) = (1400.0, 0.313);
        let c = conditions(mass, radius);
        let load = RoadLoad { cda: 0.65, crr: 0.012 };
        let (v, a) = (1.0 / KMH_PER_MS, 4.0);
        let omega = omega_from_rpm(2200.0);

        // What the ratio form would have said: ξ = ω·r/v is 259 here, so δ₂·ξ²
        // is 166 and the inertial term is multiplied by a hundred and sixty. A
        // 132 kW car reports a quarter of a megawatt, from the first sample of
        // every single run.
        let (delta1, delta2) = deltas(&TYPICAL, mass, radius);
        let xi = omega * radius / v;
        let ratio_form = (1.0 + delta1 + delta2 * xi * xi) * mass * a * v;
        assert!(ratio_form > 200_000.0, "{ratio_form}");

        // The exact form does not do that even when it is asked: at a plateau
        // the engine is barely accelerating, so its term is small and finite.
        let steady = power(v, a, Some(EngineState { omega, omega_dot: 0.0 }), &load, &c);
        assert!(steady.shaft_w.unwrap() < 20_000.0, "{steady:?}");

        // And it is not asked, because the ratio is nowhere near any plateau
        // and the speed is under the floor besides.
        let (engine_track, speed_track, gear_track) = steady_drive();
        let ratios = Ratios::learn(&engine_track, &speed_track, &gear_track);
        assert!(ratios.slipping("3", omega, v));
        let suppressed = power(v, a, None, &load, &c);
        assert_eq!(suppressed.shaft_w, None);
        assert!(
            (suppressed.wheel_w - steady.wheel_w).abs() < 1e-9,
            "the wheel figure is unaffected by the suppression"
        );
    }

    #[test]
    fn the_shaft_term_goes_negative_during_an_upshift() {
        // An upshift hands the engine's stored energy back: ω̇ < 0, so the
        // engine is doing less work than reaches the road rather than more.
        // The magnitude is startling — 6500 to 4200 rpm in 0.3 s is 46 kJ out
        // of the crank, 153 kW — and that is precisely why the same event is
        // normally suppressed by the slip check rather than reported.
        let c = conditions(1400.0, 0.313);
        let load = RoadLoad { cda: 0.65, crr: 0.012 };
        let omega = omega_from_rpm(5350.0);
        let omega_dot = (omega_from_rpm(4200.0) - omega_from_rpm(6500.0)) / 0.3;
        let p = power(30.0, 2.0, Some(EngineState { omega, omega_dot }), &load, &c);
        assert!(p.shaft_w.unwrap() < p.wheel_w);
    }

    /// Twenty-five seconds of steady driving: ten in a gear whose ratio is
    /// 15 rad/m, ten in a taller one at 9 rad/m, and five of coasting between
    /// them where the label is not a gear at all and the ratio means nothing.
    fn steady_drive() -> (Track, Track, States) {
        let (mut engine, mut speed, mut gear) =
            (Track::default(), Track::default(), States::default());
        let mut t = 0.0;
        // A gentle drift, so the samples are a stretch of driving rather than
        // one point repeated two hundred times.
        for i in 0..200 {
            let v = 20.0 + i as f64 * 0.01;
            speed.push(t, v);
            engine.push(t, 15.0 * v);
            gear.push(t, "3");
            t += 0.05;
        }
        for i in 0..100 {
            // Coasting: the engine falls towards idle while the car rolls on,
            // so the ratio walks from 15 rad/m down to under 3.
            let v = 22.0 - i as f64 * 0.02;
            speed.push(t, v);
            engine.push(t, 330.0 - i as f64 * 2.8);
            gear.push(t, "not engaged");
            t += 0.05;
        }
        for i in 0..200 {
            let v = 25.0 + i as f64 * 0.01;
            speed.push(t, v);
            engine.push(t, 9.0 * v);
            gear.push(t, "5");
            t += 0.05;
        }
        (engine, speed, gear)
    }

    #[test]
    fn plateaus_are_learned_from_the_car_rather_than_read_from_a_table() {
        let (engine, speed, gear) = steady_drive();
        let ratios = Ratios::learn(&engine, &speed, &gear);

        assert!((ratios.plateau("3").unwrap() - 15.0).abs() < 0.05);
        assert!((ratios.plateau("5").unwrap() - 9.0).abs() < 0.05);
        // The level that is not a gear has no plateau, and no label was named
        // to arrive at that: its samples simply do not agree with each other.
        assert_eq!(ratios.plateau("not engaged"), None);
        // And nothing else was learned — counted through the map itself, because
        // no measurement outside this test has a use for a count of gears.
        assert_eq!(ratios.0.len(), 2);
    }

    #[test]
    fn a_ratio_on_its_plateau_is_locked_and_one_off_it_is_slipping() {
        let (engine, speed, gear) = steady_drive();
        let ratios = Ratios::learn(&engine, &speed, &gear);
        let v = 25.0;

        assert!(!ratios.slipping("5", 9.0 * v, v));
        assert!(!ratios.slipping("5", 9.0 * v * 1.04, v), "4 % is inside the tolerance");
        assert!(ratios.slipping("5", 9.0 * v * 1.06, v), "6 % is not");
        // A gear nobody has seen driving is never assumed to be locked.
        assert!(ratios.slipping("7", 9.0 * v, v));
    }

    #[test]
    fn below_the_speed_floor_every_ratio_is_treated_as_slipping() {
        let (engine, speed, gear) = steady_drive();
        let ratios = Ratios::learn(&engine, &speed, &gear);
        // Exactly on its own plateau, and still refused: a ratio measured at
        // walking pace is a small number over a smaller one.
        let below = (RATIO_FLOOR_KMH - 1.0) / KMH_PER_MS;
        assert!(ratios.slipping("5", 9.0 * below, below));
        let above = (RATIO_FLOOR_KMH + 1.0) / KMH_PER_MS;
        assert!(!ratios.slipping("5", 9.0 * above, above));
    }

    #[test]
    fn a_gradient_is_a_tangent_and_survives_the_round_trip() {
        // Roads are signed in rise over run. 1 % is 0.009 999 5 as a sine, and
        // the coastdown recovers a sine and has to print a per cent.
        assert!((grade_sin(1.0) - 0.009_999_5).abs() < 1e-7);
        assert!((grade_percent(grade_sin(3.7)) - 3.7).abs() < 1e-9);
        assert!((grade_percent(grade_sin(-1.2)) + 1.2).abs() < 1e-9);
    }
}
