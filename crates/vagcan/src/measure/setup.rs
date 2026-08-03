//! `vagcan measure setup` — one command that starts parked and ends on the road.
//!
//! Almost everything the power model needs is on the bus. Three things are not:
//! the mass, which is on a registration document, the tyre size, which is on the
//! sidewall, and the road load, which is on no document at all and has to be
//! measured by coasting the car in neutral. This command collects the first two
//! by asking, measures the third by watching, and writes
//! [`crate::measure::carfile::CarFile`].
//!
//! **Nothing is asked of the driver while the car is moving.** That is the
//! design's central promise and it shapes the whole module: every question — the
//! name, the mass, the tyre, and the explanation of what the road part involves —
//! happens at a standstill, before anything moves, and the coastdown passes are
//! then recognised from the bus by [`coastdown::Detector`] rather than from a
//! keystroke. The screen still talks during the drive; it just never expects an
//! answer.
//!
//! **Read-only, on a car that will be moving.** No session change, no sweep,
//! nothing beyond the identifier reads `watch` already makes. `SAFETY.md` is
//! about what a read can *provoke*, and the answer here is "nothing a `watch`
//! session does not" — but this is the one command that runs at 120 km/h, so it
//! is worth saying at the door.
//!
//! **A setup abandoned partway keeps everything it obtained.** The answers go
//! into the car file as they are given, and an accepted coastdown pass is written
//! beside it, so re-running says where it stands instead of starting over. Twenty
//! minutes of driving is not something to lose because traffic arrived.
//!
//! The interview is behind [`Interview`] and the road stage is a state machine
//! ([`Coastdown`]) fed one [`Sample`] at a time, so both are driven by tests with
//! no terminal and no car. The live glue in [`run`] is the only part that needs
//! either.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::measure::carfile::{
    CarFile, FitConditions, Mass, Source, Sourced, UnitRef, rolling_radius_m,
};
use crate::measure::channels::{self, Resolved, Set};
use crate::measure::coastdown::{self, Detector, Fit, Reject, RoadLoadResult};
use crate::measure::messages::{self, ChannelFound, MissingChannel};
use crate::measure::power::{self, Inertias, KMH_PER_MS};
use crate::measure::types::Seconds;
use crate::watch::plan::{self, Batch, BatchOutcome, UnitIdentity};

/// The speed a coastdown starts from by default, in km/h.
///
/// 120 → 40 is what separates the drag term from the constant one; narrowing it
/// correlates the two badly. It is a default and not a rule, which is what
/// `--coast-from` and `--coast-to` are for — the design's §0. Exposed so that the
/// flags default to the same numbers this module reasons about.
pub const COAST_FROM_KMH: f64 = 120.0;
/// The speed a coastdown ends at by default, in km/h. See [`COAST_FROM_KMH`].
pub const COAST_TO_KMH: f64 = 40.0;

/// How many passes a road load needs.
///
/// Two, in opposite directions. One pass cannot tell a slope from a rolling
/// resistance, and on any real road there is a slope.
const WANTED_PASSES: usize = 2;

/// How long a road may fail to reach the target before the narrower range is
/// offered, in seconds.
///
/// `--coast-from` and `--coast-to` are unreachable at the moment they are
/// needed: the driver is on a road that will not do 120 and their hands are
/// occupied. Two minutes is long enough not to nag a driver still looking for a
/// clear stretch, and short enough to arrive before they give up.
const HINT_AFTER_S: Seconds = 120.0;

/// The poll period during the road stage.
///
/// A coast loses about 0.3 m/s², so 10 Hz puts hundreds of samples into a pass —
/// far more than the fit needs, and slow enough that the pedal and the selector,
/// which usually live on other control units, stay fresh in the same cycle.
const POLL_PERIOD: Duration = Duration::from_millis(100);

/// The adapter's USB serial rate, the same one every other live command opens
/// at. A property of the slcan adapter, not of any car.
const ADAPTER_BAUD: u32 = 115_200;

/// Wong, *Theory of Ground Vehicles*: the rotating inertia of a typical
/// passenger car's wheels, and of its crank, flywheel and clutch, in kg·m².
///
/// Not this car's, and recorded as [`Source::WongTypical`] so that no figure
/// resting on them ever reads as one that was measured. They are stored as
/// *inertias* rather than as coefficients on purpose: `δ₁ = I/(m·r²)` moves by
/// nearly a factor of two across the cars this tool serves, so what travels
/// between cars is the kg·m², and [`power::deltas`] turns it into this car's
/// coefficient using this car's own mass and radius. They carry about ±30 %,
/// which is ±2 % of power in top gear and ±12 % in first.
const WHEELS_KGM2: f64 = 5.5;
/// See [`WHEELS_KGM2`].
const ENGINE_KGM2: f64 = 0.34;

/// How far the air may have moved before a pass kept from an earlier attempt is
/// no longer a pass in today's air.
///
/// The fit returns `½ρ·CdA`, so a pass measured at one density and a pass
/// measured at another do not average into anything. Three per cent is what the
/// design gives as the cost of an unrecorded `ρ`, and it is the same three per
/// cent.
const RHO_TOLERANCE: f64 = 0.03;

/// What a plausible barometer and ambient sensor read, in kPa and °C.
///
/// SAE J1979 spells PID 0x33 in kPa and PID 0x46 in °C, and every road on earth
/// is inside these. A catalog row answering in millibars would otherwise give an
/// air density out by a factor of ten and a drag area to match, so the reading is
/// checked against the standard's own units rather than trusted.
const PRESSURE_RANGE_KPA: std::ops::RangeInclusive<f64> = 50.0..=115.0;
/// See [`PRESSURE_RANGE_KPA`].
const AMBIENT_RANGE_C: std::ops::RangeInclusive<f64> = -60.0..=90.0;

/// What `measure setup` was asked for.
pub struct Options<'a> {
    pub device: Option<&'a str>,
    pub catalogs: &'a str,
    /// The speed a coastdown pass opens at, and the one it closes at.
    pub coast_from_kmh: f64,
    pub coast_to_kmh: f64,
    /// Write the car file here instead of into this tool's own directory.
    pub car: Option<&'a str>,
}

// ---------------------------------------------------------------------------
// The interview
// ---------------------------------------------------------------------------

/// The two directions of the interview.
///
/// Behind a trait because the questions are the part worth testing — the mass
/// arithmetic above all, which exists to prevent a double-counted driver — and
/// because a test cannot type at a terminal.
pub trait Interview {
    /// Ask one question and read one answer.
    ///
    /// An empty answer means `default` where there is one. `Err` is the end of
    /// the conversation — a closed stdin, a person who walked away — and never an
    /// invalid answer, which is re-asked instead.
    fn ask(&mut self, prompt: &str, default: Option<&str>) -> Result<String>;

    /// Say something that needs no answer.
    fn say(&mut self, text: &str);
}

/// The interview as a person has it: stdout and stdin.
pub struct Console;

impl Interview for Console {
    fn ask(&mut self, prompt: &str, default: Option<&str>) -> Result<String> {
        let mut out = std::io::stdout();
        match default {
            Some(value) if !value.is_empty() => write!(out, "  {prompt} [{value}] ")?,
            _ => write!(out, "  {prompt} ")?,
        }
        out.flush()?;
        let mut line = String::new();
        // Zero bytes read is the end of input, which is not an empty answer: a
        // script that was piped in and ran out must not be taken for agreement
        // with every remaining default.
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!("stdin ended in the middle of the questions — nothing further was written");
        }
        let answer = line.trim();
        Ok(match (answer.is_empty(), default) {
            (true, Some(value)) => value.to_string(),
            _ => answer.to_string(),
        })
    }

    fn say(&mut self, text: &str) {
        println!("{text}");
    }
}

/// Ask until the answer is a number in range.
///
/// Re-asked rather than refused: a typo in a mass is the commonest thing that
/// happens here, and it happens with the car parked and the person present.
fn ask_number(
    io: &mut impl Interview,
    prompt: &str,
    default: Option<&str>,
    range: std::ops::RangeInclusive<f64>,
) -> Result<f64> {
    loop {
        let answer = io.ask(prompt, default)?;
        match answer.trim().replace(',', ".").parse::<f64>() {
            Ok(value) if value.is_finite() && range.contains(&value) => return Ok(value),
            _ => io.say(&screens::not_a_number(&answer, &range)),
        }
    }
}

/// Ask a question whose answer is yes or no.
fn ask_yes_no(io: &mut impl Interview, prompt: &str, default: bool) -> Result<bool> {
    let spelled = if default { "yes" } else { "no" };
    loop {
        let answer = io.ask(prompt, Some(spelled))?.trim().to_lowercase();
        match answer.as_str() {
            "y" | "yes" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => io.say("  yes or no."),
        }
    }
}

/// Ask for everything the car file is still missing, and nothing it already has.
///
/// The order is the design's: what to call the car, then the mass in two parts,
/// then the tyre. `description` is what the engine said about itself, which is
/// the nearest thing to a name anything on the bus offers.
pub fn interview(
    io: &mut impl Interview,
    car: &mut CarFile,
    description: Option<&str>,
    today: &str,
) -> Result<()> {
    if car.name.is_none() {
        io.say(&screens::name_intro());
        let default = description.unwrap_or("").trim().to_string();
        let answer = io.ask("what do you call this car?", Some(default.as_str()))?;
        let name = answer.trim();
        if !name.is_empty() {
            car.name = Some(Sourced::on(name.to_string(), Source::Stated, today));
        }
    }

    if car.mass.is_none() {
        io.say(&screens::mass_intro());
        // Three questions and one sum, rather than one question and a sum the
        // owner has to do: a mass in running order already includes a 75 kg
        // driver, so "kerb mass plus yourself" double-counts about 150 kg — and
        // mass lands on the inertial term, some 90 % of the power figure.
        let running_order_kg = ask_number(
            io,
            "mass in running order, in kg (EU field G, \"mass in service\" on a V5C)?",
            None,
            300.0..=5000.0,
        )?;
        let includes_driver = ask_yes_no(
            io,
            "does that figure already include a driver (an EU field G does)?",
            true,
        )?;
        let aboard_kg = ask_number(
            io,
            "what else will be aboard, in kg — passengers, luggage, and your own \
             difference from 75 kg?",
            Some("0"),
            -200.0..=2000.0,
        )?;
        let mass = Mass { running_order_kg, includes_driver, aboard_kg };
        io.say(&screens::mass_sum(&mass));
        car.mass = Some(Sourced::on(mass, Source::Stated, today));
    }

    if car.tyre.is_none() {
        loop {
            let answer = io.ask("tyre size, as written on the sidewall?", None)?;
            let Some(radius) = rolling_radius_m(&answer) else {
                io.say(&screens::not_a_tyre(&answer));
                continue;
            };
            car.tyre = Some(Sourced::on(answer.trim().to_string(), Source::Stated, today));
            // Arithmetic on a stated value, and recorded as exactly that.
            car.rolling_radius_m = Some(Sourced::new(radius, Source::DerivedFromTyre));
            break;
        }
    }

    // Not asked, because nobody can answer it at the roadside. Written with the
    // provenance that says so, so that a figure resting on them is never taken
    // for one that was measured.
    if car.i_wheels_kgm2.is_none() {
        car.i_wheels_kgm2 = Some(Sourced::new(WHEELS_KGM2, Source::WongTypical));
    }
    if car.i_engine_kgm2.is_none() {
        car.i_engine_kgm2 = Some(Sourced::new(ENGINE_KGM2, Source::WongTypical));
    }
    if car.speed_scale.is_none() {
        // No correction applied — which is not the same as a correction of 1.0
        // having been chosen and checked. The closing screen says how to check.
        car.speed_scale = Some(Sourced::new(1.0, Source::Uncorrected));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// The road stage
// ---------------------------------------------------------------------------

/// One cycle of the three channels a coast is recognised from.
///
/// `pedal_pct` and `selector` are optional because a channel that did not answer
/// this cycle read *nothing*, which is not the same as reading zero: an absent
/// value never opens a pass and never discards one.
#[derive(Clone, Debug, PartialEq)]
pub struct Sample {
    pub t: Seconds,
    pub speed_kmh: f64,
    pub pedal_pct: Option<f64>,
    pub selector: Option<String>,
}

/// What the road stage has to say about the sample it was just given.
#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    /// Replaces whatever is on the progress line: the current speed, and what is
    /// being waited for. Nothing in one is ever a question.
    Waiting(String),
    /// Printed and kept — a pass accepted, a pass rejected, the way out of a road
    /// that will not do the speed.
    Said(String),
    /// Enough passes. The caller stops polling and fits.
    Done,
}

/// One accepted pass, in the form that survives an abandoned setup.
///
/// The fitted coefficients rather than the samples: they are what
/// [`coastdown::road_load`] combines, and they are four numbers instead of two
/// thousand. `rho` and `mass_kg` travel with them because a `CdA` scales with
/// both, so a pass driven in different air on a different day cannot quietly be
/// averaged with today's.
#[derive(Clone, Debug, PartialEq)]
pub struct KeptPass {
    pub fit: Fit,
    /// The day it was driven, `YYYY-MM-DD`.
    pub at: String,
    pub seconds: f64,
    pub rho: f64,
    /// Whether `rho` came off the car's own sensors or from somewhere else.
    pub rho_measured: bool,
    pub mass_kg: f64,
}

/// The coastdown as a state machine: samples in, screens and passes out.
///
/// Everything that decides whether a pass *happened* lives in
/// [`coastdown::Detector`], and everything that decides what it *measured* lives
/// in [`coastdown::fit`]. What is here is the part a driver experiences: what the
/// screen says while nothing is happening, when to offer a narrower range, and
/// which pass they are on.
pub struct Coastdown {
    detector: Detector,
    from_kmh: f64,
    to_kmh: f64,
    delta1: f64,
    rho: f64,
    rho_measured: bool,
    mass_kg: f64,
    at: String,
    accepted: Vec<KeptPass>,
    fastest_kmh: f64,
    began: Option<Seconds>,
    hinted: bool,
    open: bool,
    finished: bool,
}

impl Coastdown {
    /// `already` is whatever an earlier attempt got as far as, so a resumed setup
    /// owes one pass rather than two. `conditions` is the air and the load every
    /// pass will be fitted against — `(ρ, ρ was measured, mass)` — stated once,
    /// because neither changes over a coastdown.
    pub fn new(
        from_kmh: f64,
        to_kmh: f64,
        conditions: (f64, bool, f64),
        delta1: f64,
        already: Vec<KeptPass>,
        at: impl Into<String>,
    ) -> Coastdown {
        let (rho, rho_measured, mass_kg) = conditions;
        Coastdown {
            detector: Detector::new(from_kmh, to_kmh).with_conditions(rho, mass_kg),
            from_kmh,
            to_kmh,
            delta1,
            rho,
            rho_measured,
            mass_kg,
            at: at.into(),
            accepted: already,
            fastest_kmh: 0.0,
            began: None,
            hinted: false,
            open: false,
            finished: false,
        }
    }

    /// The passes in hand, oldest first.
    pub fn accepted(&self) -> &[KeptPass] {
        &self.accepted
    }

    /// Whether enough passes have been accepted.
    pub fn is_done(&self) -> bool {
        self.accepted.len() >= WANTED_PASSES
    }

    /// One cycle of the bus.
    pub fn on_sample(&mut self, s: &Sample) -> Vec<Note> {
        let mut notes = Vec::new();
        if self.finished {
            return notes;
        }
        let began = *self.began.get_or_insert(s.t);
        self.fastest_kmh = self.fastest_kmh.max(s.speed_kmh);

        match self.detector.on_sample(s.t, s.speed_kmh, s.pedal_pct, s.selector.as_deref()) {
            Some(coastdown::Event::Opened) => {
                self.open = true;
                notes.push(Note::Said(screens::pass_opened(
                    self.accepted.len() + 1,
                    WANTED_PASSES,
                    self.to_kmh,
                )));
            }
            Some(coastdown::Event::Discarded(reason)) => {
                self.open = false;
                notes.push(Note::Said(messages::pass_rejected(
                    self.accepted.len() + 1,
                    reason,
                    self.accepted.len(),
                    WANTED_PASSES,
                )));
            }
            Some(coastdown::Event::Closed(pass)) => {
                self.open = false;
                let index = self.accepted.len() + 1;
                let from = pass.speed.v.first().copied().unwrap_or_default() * KMH_PER_MS;
                let to = pass.speed.v.last().copied().unwrap_or_default() * KMH_PER_MS;
                let seconds = match (pass.speed.t.first(), pass.speed.t.last()) {
                    (Some(first), Some(last)) => last - first,
                    _ => 0.0,
                };
                match coastdown::fit(&pass, self.delta1) {
                    Ok(fit) => {
                        self.accepted.push(KeptPass {
                            fit,
                            at: self.at.clone(),
                            seconds,
                            rho: self.rho,
                            rho_measured: self.rho_measured,
                            mass_kg: self.mass_kg,
                        });
                        notes.push(Note::Said(messages::pass_accepted(
                            index,
                            from,
                            to,
                            seconds,
                            WANTED_PASSES,
                        )));
                        if self.is_done() {
                            self.finished = true;
                            notes.push(Note::Done);
                            return notes;
                        }
                    }
                    Err(reject) => notes.push(Note::Said(messages::pass_rejected(
                        index,
                        &screens::reject_reason(&reject),
                        self.accepted.len(),
                        WANTED_PASSES,
                    ))),
                }
            }
            None => {}
        }

        // The flags are unreachable at the moment they are needed — the driver is
        // on a road that will not do the speed and their hands are busy — so the
        // way out is offered rather than merely documented.
        if !self.open
            && !self.hinted
            && self.accepted.is_empty()
            && s.t - began > HINT_AFTER_S
            && self.fastest_kmh < self.from_kmh
        {
            self.hinted = true;
            notes.push(Note::Said(screens::narrower_range(self.from_kmh, self.fastest_kmh)));
        }

        notes.push(Note::Waiting(if self.open {
            screens::coasting(s.speed_kmh, self.to_kmh, self.accepted.len() + 1, WANTED_PASSES)
        } else {
            screens::waiting(s.speed_kmh, self.from_kmh, self.accepted.len(), WANTED_PASSES)
        }));
        notes
    }
}

/// Fit the passes and put the answer in the car file, or say why not.
///
/// The `Err` side leaves the car file exactly as it was: **a rejected fit writes
/// no road load**, which keeps `--full` unavailable, which is the correct outcome
/// rather than a failure of the tool. There is no half-measured state and no
/// generic figure to fall back on.
pub fn finish(
    car: &mut CarFile,
    passes: &[KeptPass],
    delta1: f64,
    today: &str,
) -> Result<RoadLoadResult, Reject> {
    let fits: Vec<Fit> = passes.iter().map(|p| p.fit).collect();
    let first = passes.first().ok_or(Reject::OnlyOnePass)?;
    let result = coastdown::road_load(&fits, first.mass_kg, first.rho, delta1)?;
    car.cda = Some(Sourced::on(result.cda, Source::Coastdown, today));
    car.crr = Some(Sourced::on(result.crr, Source::Coastdown, today));
    car.fit = Some(FitConditions {
        passes: passes.len() as u32,
        rho_at_fit: first.rho,
        rho_source: if first.rho_measured { Source::Measured } else { Source::Stated },
        mass_at_fit_kg: first.mass_kg,
        wind_estimate_ms: Some(result.implied_wind_ms),
        grade_estimate_percent: Some(result.implied_grade_percent),
    });
    Ok(result)
}

/// `δ₁ = I_wheels/(m·r²)` for this car, out of its own file.
///
/// `None` while the file does not yet describe a car — which cannot happen after
/// [`interview`], and is a refusal rather than a default if it ever does.
pub fn delta1(car: &CarFile) -> Option<f64> {
    let inertias = Inertias {
        wheels_kgm2: car.i_wheels_kgm2.as_ref()?.value,
        engine_kgm2: car.i_engine_kgm2.as_ref()?.value,
    };
    Some(power::deltas(&inertias, car.mass_total_kg()?, car.rolling_radius_m.as_ref()?.value).0)
}

// ---------------------------------------------------------------------------
// Passes that outlive the attempt that drove them
// ---------------------------------------------------------------------------

/// Where accepted passes wait for the drive that completes them: beside the car
/// file, in the directory the rest of this car's data lives in.
///
/// Not inside `car.json`. A single pass is not a road load — it is a rolling
/// resistance and a slope added together with no way to tell which is which — and
/// the car file holds answers, not working.
pub fn passes_path(car_file: &Path) -> PathBuf {
    car_file.with_file_name("setup-passes.json")
}

/// Read whatever passes an earlier attempt left.
///
/// Anything unreadable counts as nothing kept. The alternative — refusing to
/// start because a scratch file is malformed — would block the whole command over
/// a file the owner never knew existed.
pub fn load_passes(path: &Path) -> Vec<KeptPass> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(root) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    let Some(list) = root.get("passes").and_then(Value::as_array) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in list {
        let number = |key: &str| entry.get(key).and_then(Value::as_f64).filter(|v| v.is_finite());
        let (Some(cda), Some(crr), Some(rms_kmh), Some(mean_speed_ms)) =
            (number("cda"), number("crr"), number("rms_kmh"), number("mean_speed_ms"))
        else {
            return Vec::new();
        };
        // A pass without the air and the load it was fitted at cannot be combined
        // with anything, so a file missing them is no better than one that will
        // not parse.
        let (Some(rho), Some(mass_kg)) = (number("rho"), number("mass_kg")) else {
            return Vec::new();
        };
        out.push(KeptPass {
            fit: Fit { cda, crr, rms_kmh, mean_speed_ms },
            at: entry.get("at").and_then(Value::as_str).unwrap_or_default().to_string(),
            seconds: number("seconds").unwrap_or_default(),
            rho,
            rho_measured: entry.get("rho_measured").and_then(Value::as_bool).unwrap_or(false),
            mass_kg,
        });
    }
    out
}

/// Write the accepted passes out, the moment each one is accepted.
///
/// Not at the end: the commonest way a coastdown ends is that traffic arrives,
/// and a pass written only when the whole thing succeeds is a pass lost every
/// time the whole thing does not. An empty list removes the file, so a finished
/// setup leaves nothing for the next one to resume from.
pub fn save_passes(path: &Path, passes: &[KeptPass]) -> Result<()> {
    if passes.is_empty() {
        let _ = std::fs::remove_file(path);
        return Ok(());
    }
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let list: Vec<Value> = passes
        .iter()
        .map(|p| {
            json!({
                "cda": p.fit.cda, "crr": p.fit.crr, "rms_kmh": p.fit.rms_kmh,
                "mean_speed_ms": p.fit.mean_speed_ms, "at": p.at, "seconds": p.seconds,
                "rho": p.rho, "rho_measured": p.rho_measured, "mass_kg": p.mass_kg,
            })
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&json!({ "passes": list }))?;
    text.push('\n');
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
}

/// Which kept passes still describe today's car in today's air.
///
/// A `CdA` is `½ρ·CdA` divided by the `ρ` that was in the air, and it scales with
/// the mass it was fitted at. Averaging a pass from a cold morning with one from
/// a warm afternoon is not a two-pass measurement of anything, so the mismatched
/// ones are dropped — with the reason on screen, because a driver who was told
/// they owed one pass and is now asked for two deserves to know why.
pub fn still_valid(
    passes: Vec<KeptPass>,
    rho: f64,
    mass_kg: f64,
) -> (Vec<KeptPass>, Option<String>) {
    let matches = |p: &KeptPass| {
        (p.rho - rho).abs() <= RHO_TOLERANCE * rho && (p.mass_kg - mass_kg).abs() <= 1.0
    };
    if passes.iter().all(matches) {
        return (passes, None);
    }
    let note = screens::conditions_moved(passes.len());
    (passes.into_iter().filter(matches).collect(), Some(note))
}

// ---------------------------------------------------------------------------
// The live command
// ---------------------------------------------------------------------------

/// Describe this car once, then measure its road load on the road.
///
/// Parked for everything that involves a person, moving for everything that
/// involves the car, and in that order.
pub async fn run(opts: Options<'_>) -> Result<()> {
    use vag_can::{SlcanBackend, SlcanBitrate, SlcanMode};

    let today = today();
    let device_path = crate::device::resolve(opts.device)?;
    let store = vag_data::catalog::CatalogStore::open(opts.catalogs);
    let mut adapter = SlcanBackend::open_mode(
        &device_path,
        ADAPTER_BAUD,
        SlcanBitrate::Rate500k,
        SlcanMode::Normal,
    )
    .await
    .with_context(|| crate::device::open_failure(&device_path))?;

    // 1. What this car is: the gateway's installation list, then one
    //    identification block per unit, then the VIN off the engine. The same
    //    reads `watch` makes, and no session change in any of them.
    let mut progress = crate::progress::Line::new();
    let (back, identities) = crate::units::identify(adapter, &[plan::ENGINE], &mut progress).await;
    adapter = back;
    progress.update("asking the engine for the VIN");
    let (back, engine) = read_engine_identity(adapter).await;
    adapter = back;
    progress.finish();

    let vin = engine
        .vin
        .clone()
        .filter(|v| !v.trim().is_empty())
        .context("the engine did not report a VIN, and a car file is keyed by the car")?;
    let description = identities
        .iter()
        .find(|i| i.request == plan::ENGINE)
        .and_then(|i| i.component.clone())
        .or_else(|| engine.component.clone());

    let mut io = Console;

    // 2. The channel check, at a standstill, so a missing channel is found with
    //    the handbrake on rather than at a green light. Resolved as `full`
    //    whatever the mode: the barometer and the ambient sensor are what a `CdA`
    //    means anything against, and this is the measurement that makes runs
    //    possible rather than a run.
    let set = match channels::resolve(&store, &identities, true) {
        Ok(set) => set,
        Err(missing) => {
            let rows: Vec<ChannelFound> = missing
                .iter()
                .map(|m| ChannelFound {
                    unit: "—".into(),
                    part_number: "—".into(),
                    key: m.key,
                    ok: false,
                })
                .collect();
            let missing: Vec<MissingChannel> = missing
                .into_iter()
                .map(|m| MissingChannel { key: m.key, tried: m.tried })
                .collect();
            io.say(&screens::units_answered(&identities));
            io.say(&messages::missing_channels(&rows, &missing));
            bail!("this car's catalogs are missing a channel the coastdown needs");
        }
    };
    io.say(&screens::channel_check(&set, &identities));
    if let Some(refusal) = screens::coast_impossible(&set) {
        io.say(&refusal);
        bail!("a coastdown pass could not be recognised from this car's channels");
    }

    // 3. The car file, and where it stands. A file for a different car is refused
    //    rather than applied: mass and road load belong to one car.
    let path = match opts.car {
        Some(path) => PathBuf::from(path),
        None => CarFile::path_for(&vin, description.as_deref())?,
    };
    let mut car = if path.exists() {
        let existing = CarFile::load(&path)?;
        if existing.vin != vin {
            io.say(&messages::wrong_car(&existing.vin, &vin));
            bail!("that car file is for another car");
        }
        existing
    } else {
        CarFile::new(&vin)
    };
    car.units = identities
        .iter()
        .filter_map(|i| Some(UnitRef { request: i.request, part_number: i.part_number.clone()? }))
        .collect();
    let mut kept = load_passes(&passes_path(&path));
    if car.mass.is_some() || car.tyre.is_some() || !kept.is_empty() {
        io.say(&screens::resuming(&car, &kept));
        if !kept.is_empty()
            && io
                .ask("keep the pass already driven, or discard it?", Some("keep"))?
                .trim()
                .eq_ignore_ascii_case("r")
        {
            kept.clear();
        }
    }

    // 4. The questions. All of them, here, parked.
    interview(&mut io, &mut car, description.as_deref(), &today)?;
    car.save(&path)?;
    io.say(&screens::answers_saved(&path));

    // 5. The air the road load will be measured in, read before anything moves.
    let mut reader = Reader::new(&set);
    let started = Instant::now();
    let mut backend = Some(adapter);
    reader.cycle(&mut backend, started).await;
    let Some(rho) = reader.air_density(&set) else {
        io.say(&screens::no_air_density());
        bail!("no air density: the fit returns ½·ρ·CdA, and there is no CdA without ρ");
    };
    let mass_kg = car.mass_total_kg().context("the mass was answered just above")?;
    let delta1 = delta1(&car).context("the car file describes this car by now")?;
    let (kept, dropped) = still_valid(kept, rho, mass_kg);
    if let Some(note) = dropped {
        io.say(&note);
    }
    save_passes(&passes_path(&path), &kept)?;

    // 6. The road part, explained while the car is still parked, because none of
    //    it can be explained at 120 km/h.
    io.say(&screens::road_briefing(opts.coast_from_kmh, opts.coast_to_kmh, rho));
    io.ask("press Enter when you have read that and are ready to set off", Some(""))?;

    // 7. The drive. Nothing below this line asks the driver anything.
    let mut stage = Coastdown::new(
        opts.coast_from_kmh,
        opts.coast_to_kmh,
        (rho, true, mass_kg),
        delta1,
        kept,
        &today,
    );
    let mut line = crate::progress::Line::new();
    while !stage.is_done() {
        let cycle = Instant::now();
        let t = reader.cycle(&mut backend, started).await;
        match reader.sample(&set, t) {
            Some(sample) => {
                for note in stage.on_sample(&sample) {
                    match note {
                        Note::Waiting(text) => line.update(&text),
                        Note::Said(text) => {
                            line.finish();
                            println!("\n{text}\n");
                            // Every accepted pass is on disk before the next one
                            // is asked for.
                            save_passes(&passes_path(&path), stage.accepted())?;
                        }
                        Note::Done => break,
                    }
                }
            }
            None => line.update(&screens::not_answering()),
        }
        if let Some(rest) = POLL_PERIOD.checked_sub(cycle.elapsed()) {
            tokio::time::sleep(rest).await;
        }
    }
    line.finish();
    let kept = stage.accepted().to_vec();

    // 8. The fit, and what the car is now known to be.
    match finish(&mut car, &kept, delta1, &today) {
        Ok(result) => {
            car.save(&path)?;
            let _ = std::fs::remove_file(passes_path(&path));
            io.say(&screens::complete(&car, &path, &result));
        }
        Err(Reject::Disagreement { crr_percent, limit, implied_grade_percent, implied_wind_ms }) => {
            let per_pass: Vec<(f64, f64)> = kept.iter().map(|p| (p.fit.cda, p.fit.crr)).collect();
            io.say(&messages::fit_rejected(
                crr_percent,
                limit,
                &per_pass,
                implied_grade_percent,
                implied_wind_ms,
            ));
        }
        Err(other) => io.say(&screens::fit_not_possible(&other, kept.len())),
    }
    Ok(())
}

/// Today, `YYYY-MM-DD`, in the owner's own time zone — the day they would write
/// on the answer themselves.
fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// The engine's identification block, which is where the VIN lives.
///
/// The adapter is handed over and handed back: it is a single-user resource with
/// no way to borrow it across an await.
async fn read_engine_identity<B: vag_can::CanBackend>(
    backend: B,
) -> (B, vag_protocol::identity::EcuIdentity) {
    use vag_protocol::AsyncUdsClient;
    use vag_protocol::address::UnitAddress;
    use vag_transport::CanId;

    let Some(address) = UnitAddress::from_request(plan::ENGINE) else {
        return (backend, vag_protocol::identity::EcuIdentity::default());
    };
    let mut uds = AsyncUdsClient::new(vag_can::IsoTpCan::new(
        backend,
        CanId::Standard(address.request),
        CanId::Standard(address.response),
    ));
    let identity = vag_protocol::identity::read_identity(&mut uds).await;
    (uds.into_transport().into_backend(), identity)
}

/// One cycle of every channel the coastdown watches.
///
/// Every resolved channel is read every cycle, not only the leading unit's: the
/// pedal and the selector usually live on other control units, and a coast is
/// recognised from all three together. A coastdown is slow — 0.3 m/s² — so the
/// rate this costs is rate the fit does not need.
struct Reader {
    batches: Vec<Batch>,
    latest: BTreeMap<(u16, u16), Vec<u8>>,
}

impl Reader {
    fn new(set: &Set) -> Reader {
        let mut by_unit: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
        for channel in set.all() {
            let dids = by_unit.entry(channel.request).or_default();
            // The same identifier twice in one request wastes a slot and makes
            // the response ambiguous to split.
            if !dids.contains(&channel.did) {
                dids.push(channel.did);
            }
        }
        let batches = by_unit
            .into_iter()
            .flat_map(|(request, dids)| {
                dids.chunks(plan::BATCH)
                    .map(|chunk| Batch { request, dids: chunk.to_vec() })
                    .collect::<Vec<_>>()
            })
            .collect();
        Reader { batches, latest: BTreeMap::new() }
    }

    /// Read every batch once, and say when the cycle ended.
    async fn cycle<B: vag_can::CanBackend>(
        &mut self,
        backend: &mut Option<B>,
        started: Instant,
    ) -> Seconds {
        let mut at = started.elapsed().as_secs_f64();
        for batch in &self.batches {
            let (t, outcome) = plan::read_batch(backend, batch, started).await;
            at = t;
            match outcome {
                BatchOutcome::Answered(records) => {
                    for (did, data) in records {
                        self.latest.insert((batch.request, did), data);
                    }
                }
                // A unit that stopped answering keeps no stale value: the
                // detector treats an absent reading as "nothing was said", which
                // neither opens a pass nor discards one — and a held-over pedal
                // reading is exactly how a braked pass would come to be accepted.
                BatchOutcome::NoAnswer | BatchOutcome::Unaddressable => {
                    for did in &batch.dids {
                        self.latest.remove(&(batch.request, *did));
                    }
                }
            }
        }
        at
    }

    fn raw(&self, channel: &Resolved) -> Option<&[u8]> {
        self.latest.get(&(channel.request, channel.did)).map(Vec::as_slice)
    }

    fn value_of(&self, set: &Set, key: &str) -> Option<f64> {
        let channel = set.all().find(|c| c.key == key)?;
        channel.value(self.raw(channel)?)
    }

    fn state_of(&self, set: &Set, key: &str) -> Option<String> {
        let channel = set.all().find(|c| c.key == key)?;
        channel.state(self.raw(channel)?)
    }

    /// `ρ = p/(R·T)`, from the car's own barometer and ambient sensor.
    ///
    /// `None` when either is absent or reads outside what SAE J1979's own units
    /// allow: a catalog row in millibars would otherwise give a density out by a
    /// factor of ten and a drag area to match.
    fn air_density(&self, set: &Set) -> Option<f64> {
        let kpa = self.value_of(set, "barometer").filter(|p| PRESSURE_RANGE_KPA.contains(p))?;
        let celsius = self.value_of(set, "ambient").filter(|c| AMBIENT_RANGE_C.contains(c))?;
        Some(power::air_density(kpa, celsius)).filter(|rho| rho.is_finite() && *rho > 0.0)
    }

    /// The three channels a coast is recognised from, or `None` while the car is
    /// not reporting a speed at all.
    fn sample(&self, set: &Set, t: Seconds) -> Option<Sample> {
        let speed_kmh = set.leading.value(self.raw(&set.leading)?)?;
        Some(Sample {
            t,
            speed_kmh,
            pedal_pct: self.value_of(set, "pedal"),
            selector: self.state_of(set, "selector"),
        })
    }
}

// ---------------------------------------------------------------------------
// What the screen says
// ---------------------------------------------------------------------------

/// The screens `measure setup` shows on its ordinary path.
///
/// In one place, and never inline in the loop, for the reason
/// [`crate::measure::messages`] gives: prose formatted in the middle of a drive
/// is prose nobody can read or test. The refusals in that module are reused as
/// they stand — a rejected pass and a rejected fit say the same thing whoever
/// asked — and what is here is the part only this command has: the questions, the
/// road briefing, the resumed-setup banner and the closing screen.
mod screens {
    use super::*;

    pub fn name_intro() -> String {
        "\nNo control unit broadcasts a make or a model. The engine describes itself and\n\
         every unit reports a part number; the rest is yours to say, and it becomes the\n\
         readable half of this car's own directory."
            .to_string()
    }

    pub fn mass_intro() -> String {
        "\nThe mass, in two parts, so the arithmetic is mine rather than yours. A\n\
         registration document's mass in running order already includes a 75 kg driver\n\
         and a nearly full tank, so adding yourself to it counts you twice — about\n\
         150 kg on a 1400 kg car, and mass is most of the power figure."
            .to_string()
    }

    /// The sum, shown rather than assumed: this is the one line on which an owner
    /// can see whether the driver got counted once.
    pub fn mass_sum(mass: &Mass) -> String {
        let driver = if mass.includes_driver {
            "the document's 75 kg driver is already in it"
        } else {
            "including the 75 kg driver the document left out"
        };
        format!(
            "    {:.0} kg stated + {:.0} kg aboard  =  {:.0} kg   ({driver})",
            mass.running_order_kg,
            mass.aboard_kg,
            mass.total()
        )
    }

    pub fn not_a_number(answer: &str, range: &std::ops::RangeInclusive<f64>) -> String {
        format!(
            "  {answer:?} is not a number between {:.0} and {:.0}. Try again.",
            range.start(),
            range.end()
        )
    }

    pub fn not_a_tyre(answer: &str) -> String {
        format!(
            "  {answer:?} is not a tyre size I can turn into a wheel radius. It is written\n  \
             on the sidewall as width/aspect then the rim — 205/55R16, 225/40ZR18."
        )
    }

    /// What answered, before anything is said about what is missing.
    pub fn units_answered(identities: &[UnitIdentity]) -> String {
        let mut out = format!("\n{} control units answered:\n", identities.len());
        for unit in identities {
            let _ = writeln!(
                out,
                "    {:03X}  {:<14} {}",
                unit.request,
                unit.part_number.as_deref().unwrap_or("—"),
                unit.component.as_deref().unwrap_or("")
            );
        }
        out
    }

    /// The pre-flight check, at a standstill.
    pub fn channel_check(set: &Set, identities: &[UnitIdentity]) -> String {
        let part = |request: u16| {
            identities
                .iter()
                .find(|i| i.request == request)
                .and_then(|i| i.part_number.clone())
                .unwrap_or_else(|| "—".to_string())
        };
        let mut out =
            String::from("\nchannel check — read now, parked, and not at a green light:\n\n");
        for channel in set.all() {
            let _ = writeln!(
                out,
                "    {:03X}  {:<14} {:<18} {}",
                channel.request,
                part(channel.request),
                channel.key,
                channel.def.name
            );
        }
        out
    }

    /// The two channels without which no pass can ever be recognised.
    ///
    /// A manual car with no selector channel is the real case: the coast would
    /// never open, and the driver would discover that after a kilometre of clear
    /// road rather than on the drive.
    pub fn coast_impossible(set: &Set) -> Option<String> {
        let has = |key: &str| set.all().any(|c| c.key == key);
        let missing: Vec<&str> = ["pedal", "selector"].into_iter().filter(|k| !has(k)).collect();
        if missing.is_empty() {
            return None;
        }
        Some(format!(
            "\nA coastdown pass is recognised from the bus — the speed falling with the pedal\n\
             at zero and the selector in N — and this car publishes no {}.\n\n\
             Without it no pass can open, so the road part would never start, and nothing is\n\
             asked of you at speed to make up for it. Everything answered so far is saved.\n\
             To look for the channel:\n    \
             vagcan survey --out parked.jsonl      then, after a drive:\n    \
             vagcan survey --out driving.jsonl\n    \
             vagcan survey --diff parked.jsonl driving.jsonl",
            missing.join(" and ")
        ))
    }

    pub fn no_air_density() -> String {
        "\nThis car published no barometer, or no ambient air temperature, and the coastdown\n\
         fit returns ½·ρ·CdA — a drag area without the air it was measured in is not a\n\
         number. Nothing is lost: the answers are saved.\n\n\
         The two readings are SAE J1979's PIDs 0x33 and 0x46. If this car answers them\n\
         under other names, `vagcan survey --diff` will find them:\n    \
         vagcan survey --out parked.jsonl"
            .to_string()
    }

    pub fn answers_saved(path: &Path) -> String {
        format!("\nanswers saved — {}", path.display())
    }

    /// Where a resumed setup stands, rather than starting over.
    pub fn resuming(car: &CarFile, kept: &[KeptPass]) -> String {
        let mut out = format!("\nresuming setup for {}\n", car.vin);
        let answered: Vec<&str> =
            [car.mass.as_ref().map(|_| "mass"), car.tyre.as_ref().map(|_| "tyre")]
                .into_iter()
                .flatten()
                .collect();
        if !answered.is_empty() {
            let at = car
                .mass
                .as_ref()
                .and_then(|m| m.at.clone())
                .or_else(|| car.tyre.as_ref().and_then(|t| t.at.clone()))
                .unwrap_or_else(|| "earlier".to_string());
            let _ = writeln!(out, "    {:<16}answered {at}", answered.join(", "));
        }
        if let Some(first) = kept.first() {
            let _ = writeln!(
                out,
                "    {:<16}{} of {WANTED_PASSES} passes done ({}, {:.1} s)",
                "coastdown",
                kept.len(),
                first.at,
                first.seconds
            );
            // The tool cannot see which way the car was pointing, and a pair
            // driven the same way is undetectable from the bus and silently
            // wrong. So the way out is offered here, parked, where it is free.
            let _ = writeln!(
                out,
                "  Drive the return pass on the same stretch. If you no longer know which way\n  \
                 the first pass went, answer r to discard it and drive both again."
            );
        }
        out
    }

    /// The kept passes no longer describe today's drive.
    pub fn conditions_moved(had: usize) -> String {
        format!(
            "\nthe air density or the load has moved since {had} pass{} driven earlier, and a\n\
             CdA fitted at one density does not average with one fitted at another.\n\
             Discarding it: both passes have to be the same car in the same air.",
            if had == 1 { " was" } else { "es were" }
        )
    }

    /// The road part, explained while the car is still parked, because none of it
    /// can be explained at 120 km/h.
    pub fn road_briefing(from_kmh: f64, to_kmh: f64, rho: f64) -> String {
        format!(
            "\nThe road part needs about a kilometre of clear, flat, dry road with no traffic\n\
             behind you — twice, once in each direction. Coasting from {from_kmh:.0} to \
             {to_kmh:.0} km/h takes\n\
             30 to 45 seconds; the car does not slow quickly in neutral. Find the road\n\
             before you set off.\n\n\
             Each pass: get to {from_kmh:.0}, select N, take your foot off, let it roll to \
             {to_kmh:.0}, then\n\
             drive normally. Nothing here touches the car — it is coasting and this tool is\n\
             reading its speed. Decide about neutral before the pass, not during it: I will\n\
             not ask you anything while you are moving.\n\n\
             You can stop at any point. Everything answered and every accepted pass is kept.\n\n\
             The air is {rho:.3} kg/m³ by this car's own barometer, and wind is the one thing\n\
             that does not cancel between the two directions: above about 2 m/s it puts 2 %\n\
             on the rolling resistance whichever way you drive."
        )
    }

    pub fn pass_opened(index: usize, wanted: usize, to_kmh: f64) -> String {
        format!(
            "pass {index} of {wanted} — coasting. Foot off, N selected; let it roll to \
             {to_kmh:.0} km/h."
        )
    }

    pub fn coasting(speed_kmh: f64, to_kmh: f64, index: usize, wanted: usize) -> String {
        format!("coasting — {speed_kmh:.0} km/h, down to {to_kmh:.0} (pass {index} of {wanted})")
    }

    pub fn waiting(speed_kmh: f64, from_kmh: f64, done: usize, wanted: usize) -> String {
        format!(
            "{speed_kmh:.0} km/h — waiting for {from_kmh:.0} with the pedal off and N selected \
             ({done} of {wanted} passes done)"
        )
    }

    /// The way out of a road that will not do the speed.
    ///
    /// The suggested range is the design's own arithmetic: the nearest ten below
    /// what this road has actually managed, and the same 60 km/h of span
    /// underneath it, so a road that reached 96 is offered 90 → 30.
    pub fn narrower_range(from_kmh: f64, fastest_kmh: f64) -> String {
        let suggested_from = ((fastest_kmh / 10.0).floor() * 10.0).max(50.0);
        let suggested_to = (suggested_from - 60.0).max(20.0);
        format!(
            "still waiting for {from_kmh:.0} km/h — the fastest so far is {fastest_kmh:.0}.\n\
             If this road will not do it, press Ctrl-C and start again with a lower range:\n    \
             vagcan measure setup --coast-from {suggested_from:.0} --coast-to {suggested_to:.0}\n\
             A narrower range separates drag from rolling resistance less well, and the fit\n\
             will say by how much rather than hiding it."
        )
    }

    pub fn not_answering() -> String {
        "the car is not reporting a speed — waiting. Nothing is lost; this picks up again \
         when it does."
            .to_string()
    }

    /// Why one pass measured nothing, in the words the rejection notice needs.
    pub fn reject_reason(reject: &Reject) -> String {
        match reject {
            Reject::Residual { rms_kmh, .. } => format!(
                "the speed did not follow a free coast — {rms_kmh:.1} km/h off the curve, which \
                 is braking, or a slope that changed"
            ),
            Reject::TooNarrow { span_kmh } => {
                format!("only {span_kmh:.0} km/h of speed range in it")
            }
            Reject::ConditionsUnstated => {
                "the air density or the mass was not known when it was driven".to_string()
            }
            Reject::Disagreement { crr_percent, .. } => {
                format!("it disagrees with the other pass by {crr_percent:.0} % on Crr")
            }
            Reject::OnlyOnePass => "one pass on its own cannot tell a slope from a Crr".to_string(),
        }
    }

    /// A fit that never got as far as comparing two passes.
    pub fn fit_not_possible(reject: &Reject, passes: usize) -> String {
        format!(
            "no road load was written — {}.\n\n\
             {passes} pass{} kept, so re-running vagcan measure setup asks only for the rest.\n\
             Mass and tyre size are saved either way, and --full stays unavailable until both\n\
             directions have been driven — which is the right answer, not a failure.",
            reject_reason(reject),
            if passes == 1 { " is" } else { "es are" }
        )
    }

    /// What the car is now known to be, and what that unlocks.
    pub fn complete(car: &CarFile, path: &Path, result: &RoadLoadResult) -> String {
        let mut out = format!("\nSetup complete — {}\n\n", path.display());
        if let Some(mass) = &car.mass {
            let _ = writeln!(
                out,
                "  mass    {:<14} {}",
                format!("{:.0} kg", mass.value.total()),
                provenance(mass.source, mass.at.as_deref(), None)
            );
        }
        if let Some(tyre) = &car.tyre {
            let _ = writeln!(
                out,
                "  tyre    {:<14} {}",
                tyre.value,
                provenance(tyre.source, tyre.at.as_deref(), None)
            );
        }
        let passes = car.fit.as_ref().map(|f| f.passes);
        if let Some(cda) = &car.cda {
            let _ = writeln!(
                out,
                "  CdA     {:<14} {}",
                format!("{:.2} m²", cda.value),
                provenance(cda.source, None, passes)
            );
        }
        if let Some(crr) = &car.crr {
            let _ = writeln!(
                out,
                "  Crr     {:<14} {}",
                format!("{:.4}", crr.value),
                provenance(crr.source, None, None)
            );
        }
        if let Some(fit) = &car.fit {
            // Without these the CdA above is not a number: the fit returns
            // ½·ρ·CdA, and it scales with the mass it was fitted at.
            let _ = writeln!(
                out,
                "  ρ {:.3} kg/m³ and {:.0} kg at fit time, wind ≈ {:.1} m/s, slope ≈ {:.1} %",
                fit.rho_at_fit,
                fit.mass_at_fit_kg,
                result.implied_wind_ms.abs(),
                result.implied_grade_percent.abs()
            );
        }
        let _ = writeln!(
            out,
            "\n  Power is now available:   vagcan measure --full\n\n\
             \x20 Two things worth doing once, neither of which needs this tool:\n\
             \x20   • run out and back on the same stretch and average at matched speeds — slope\n\
             \x20     and steady wind reverse sign between the two and cancel\n\
             \x20   • compare one run against GPS to see whether this car's bus speed carries the\n\
             \x20     speedometer's optimism, then pass --speed-scale"
        );
        out
    }

    /// Where a figure came from, in the words the owner was asked in.
    fn provenance(source: Source, at: Option<&str>, passes: Option<u32>) -> String {
        match (source, at, passes) {
            (Source::Stated, Some(at), _) => format!("you, {at}"),
            (Source::Stated, None, _) => "you".to_string(),
            (Source::Coastdown, _, Some(n)) => {
                format!("measured on this car, {n} pass{}", if n == 1 { "" } else { "es" })
            }
            (Source::Coastdown, _, None) => "measured on this car".to_string(),
            (other, _, _) => other.as_str().to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique-per-test temp dir, cleaned up on drop — the shape the rest of
    /// this crate's file tests use. Nothing here may write inside a checkout.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> TempDir {
            let path = std::env::temp_dir().join(format!(
                "vagcan-setup-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            TempDir(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The interview with a script instead of a person: the answers go in, and
    /// everything said comes back out to be asserted against.
    struct Scripted {
        answers: std::collections::VecDeque<String>,
        asked: Vec<String>,
        said: Vec<String>,
    }

    impl Scripted {
        fn new<'a>(answers: impl IntoIterator<Item = &'a str>) -> Scripted {
            Scripted {
                answers: answers.into_iter().map(str::to_string).collect(),
                asked: Vec::new(),
                said: Vec::new(),
            }
        }

        /// Everything the interview put on screen, questions included.
        fn transcript(&self) -> String {
            format!("{}\n{}", self.asked.join("\n"), self.said.join("\n"))
        }
    }

    impl Interview for Scripted {
        fn ask(&mut self, prompt: &str, default: Option<&str>) -> Result<String> {
            self.asked.push(prompt.to_string());
            let answer = self
                .answers
                .pop_front()
                .with_context(|| format!("the script ran out at {prompt:?}"))?;
            Ok(match (answer.is_empty(), default) {
                (true, Some(value)) => value.to_string(),
                _ => answer,
            })
        }

        fn say(&mut self, text: &str) {
            self.said.push(text.to_string());
        }
    }

    const MASS_KG: f64 = 1475.0;
    const RHO: f64 = 1.2;
    const TRUE_CDA: f64 = 0.63;
    const TRUE_CRR: f64 = 0.0114;

    fn a_described_car() -> CarFile {
        let mut car = CarFile::new("XW8AD4NE9JH008917");
        car.name = Some(Sourced::on("Škoda Octavia III".into(), Source::Stated, "2026-08-03"));
        car.mass = Some(Sourced::on(
            Mass { running_order_kg: 1395.0, includes_driver: true, aboard_kg: 80.0 },
            Source::Stated,
            "2026-08-03",
        ));
        car.tyre = Some(Sourced::on("205/55R16".into(), Source::Stated, "2026-08-03"));
        car.rolling_radius_m =
            Some(Sourced::new(rolling_radius_m("205/55R16").unwrap(), Source::DerivedFromTyre));
        car.i_wheels_kgm2 = Some(Sourced::new(WHEELS_KGM2, Source::WongTypical));
        car.i_engine_kgm2 = Some(Sourced::new(ENGINE_KGM2, Source::WongTypical));
        car
    }

    fn a_kept_pass(crr: f64) -> KeptPass {
        KeptPass {
            fit: Fit { cda: TRUE_CDA, crr, rms_kmh: 0.21, mean_speed_ms: 22.3 },
            at: "2026-08-04".into(),
            seconds: 38.2,
            rho: RHO,
            rho_measured: true,
            mass_kg: MASS_KG,
        }
    }

    /// The closed form of a coast down a flat road:
    /// `v(t) = v_c·tan(arctan(v₀/v_c) − t/τ)`. The same curve the fit solves for,
    /// so a synthetic pass is the fit's own truth rather than an integration of
    /// it — these tests are about the stage around the fit, not about the fit.
    fn coast(cda: f64, crr: f64, delta1: f64) -> Vec<(f64, f64)> {
        let a = 0.5 * RHO * cda;
        let b = MASS_KG * crate::measure::power::G * crr;
        let vc = (b / a).sqrt();
        let tau = MASS_KG * (1.0 + delta1) / (a * b).sqrt();
        let v0 = (COAST_FROM_KMH + 5.0) / KMH_PER_MS;
        let floor = (COAST_TO_KMH - 1.0) / KMH_PER_MS;
        let mut out = Vec::new();
        let mut t = 0.0;
        while out.len() < 5000 {
            let v = vc * ((v0 / vc).atan() - t / tau).tan();
            out.push((t, v));
            if v <= floor {
                break;
            }
            t += 0.05;
        }
        out
    }

    fn delta1_of(car: &CarFile) -> f64 {
        delta1(car).unwrap()
    }

    fn a_stage(car: &CarFile, already: Vec<KeptPass>) -> Coastdown {
        Coastdown::new(
            COAST_FROM_KMH,
            COAST_TO_KMH,
            (RHO, true, MASS_KG),
            delta1_of(car),
            already,
            "2026-08-04",
        )
    }

    /// Drive one synthetic pass into a stage — pedal off, N selected, from above
    /// the target down through it. Nothing is pressed: the pass is recognised
    /// from these samples alone.
    fn drive(stage: &mut Coastdown, cda: f64, crr: f64, at: f64, delta1: f64) -> Vec<Note> {
        let mut notes = Vec::new();
        for (t, v) in coast(cda, crr, delta1) {
            notes.extend(stage.on_sample(&Sample {
                t: at + t,
                speed_kmh: v * KMH_PER_MS,
                pedal_pct: Some(0.0),
                selector: Some("N".into()),
            }));
        }
        notes
    }

    fn said(notes: &[Note]) -> String {
        notes
            .iter()
            .filter_map(|n| match n {
                Note::Said(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // -- the interview ----------------------------------------------------

    #[test]
    fn a_mass_in_running_order_is_not_charged_for_its_driver_twice() {
        // The whole reason this command asks three questions instead of one. The
        // document says 1395 kg, which under Regulation 1230/2012 already carries
        // a 75 kg driver; a passenger and some luggage, 80 kg, are aboard on top
        // of it.
        let mut io = Scripted::new(["Škoda Octavia III", "1395", "y", "80", "205/55R16"]);
        let mut car = CarFile::new("XW8AD4NE9JH008917");
        interview(&mut io, &mut car, Some("1.8l R4 TFSI"), "2026-08-03").unwrap();

        let mass = car.mass.as_ref().unwrap();
        assert_eq!(mass.value.total(), 1475.0);
        assert_eq!(mass.value.running_order_kg, 1395.0);
        assert!(mass.value.includes_driver);
        // Shown, not merely stored: this line is where an owner would catch it.
        let transcript = io.transcript();
        assert!(transcript.contains("1475 kg"), "{transcript}");
        assert!(!transcript.contains("1550"), "the driver was counted twice:\n{transcript}");
    }

    #[test]
    fn a_stated_mass_without_a_driver_gains_exactly_one() {
        // The same car with the same load, described the other way round, has to
        // come out at the same total.
        let mut io = Scripted::new(["", "1320", "n", "80", "205/55R16"]);
        let mut car = CarFile::new("XW8AD4NE9JH008917");
        interview(&mut io, &mut car, Some("1.8l R4 TFSI"), "2026-08-03").unwrap();
        assert_eq!(car.mass.as_ref().unwrap().value.total(), 1475.0);
        assert!(io.transcript().contains("75 kg driver"), "{}", io.transcript());
    }

    #[test]
    fn the_engine_describes_itself_and_the_owner_names_the_car() {
        // No unit broadcasts a make or a model, so an empty answer keeps the
        // engine's own words rather than inventing a marque.
        let mut io = Scripted::new(["", "1395", "y", "0", "205/55R16"]);
        let mut car = CarFile::new("XW8AD4NE9JH008917");
        interview(&mut io, &mut car, Some("1.8l R4 TFSI"), "2026-08-03").unwrap();
        let name = car.name.as_ref().unwrap();
        assert_eq!(name.value, "1.8l R4 TFSI");
        assert_eq!(name.source, Source::Stated);
    }

    #[test]
    fn a_typo_is_re_asked_rather_than_stored() {
        let mut io = Scripted::new([
            "a car",
            "fourteen hundred",
            "1395",
            "y",
            "0",
            "16 inch",
            "205/55R16",
        ]);
        let mut car = CarFile::new("XW8AD4NE9JH008917");
        interview(&mut io, &mut car, None, "2026-08-03").unwrap();
        assert_eq!(car.mass.as_ref().unwrap().value.running_order_kg, 1395.0);
        assert_eq!(car.tyre.as_ref().unwrap().value, "205/55R16");
        assert!(io.transcript().contains("not a number"), "{}", io.transcript());
        assert!(io.transcript().contains("sidewall"), "{}", io.transcript());
    }

    #[test]
    fn the_tyre_size_becomes_a_radius_that_says_where_it_came_from() {
        let mut io = Scripted::new(["a car", "1395", "y", "0", "205/55R16"]);
        let mut car = CarFile::new("XW8AD4NE9JH008917");
        interview(&mut io, &mut car, None, "2026-08-03").unwrap();
        let radius = car.rolling_radius_m.as_ref().unwrap();
        assert!((radius.value - 0.31595).abs() < 1e-9);
        assert_eq!(radius.source, Source::DerivedFromTyre);
        // Nobody can measure these at the roadside, so they are written with the
        // provenance that says so rather than asked for.
        assert_eq!(car.i_wheels_kgm2.as_ref().unwrap().source, Source::WongTypical);
        assert_eq!(car.speed_scale.as_ref().unwrap().source, Source::Uncorrected);
    }

    #[test]
    fn a_resumed_setup_asks_only_for_what_is_missing() {
        // An abandoned setup keeps its answers, so re-running must not put the
        // owner through the registration document again.
        let mut io = Scripted::new(std::iter::empty::<&str>());
        let mut car = a_described_car();
        interview(&mut io, &mut car, Some("1.8l R4 TFSI"), "2026-08-04").unwrap();
        assert!(io.asked.is_empty(), "asked again: {:?}", io.asked);
        assert_eq!(car.mass.as_ref().unwrap().at.as_deref(), Some("2026-08-03"));
    }

    #[test]
    fn a_resumed_setup_says_where_it_stands() {
        let text = screens::resuming(&a_described_car(), &[a_kept_pass(TRUE_CRR)]);
        assert!(text.contains("resuming setup for XW8AD4NE9JH008917"), "{text}");
        assert!(text.contains("mass, tyre"), "{text}");
        assert!(text.contains("answered 2026-08-03"), "{text}");
        assert!(text.contains("1 of 2 passes done (2026-08-04, 38.2 s)"), "{text}");
        // The tool cannot see direction, so a driver who has lost track of which
        // way the first pass went has to be given a way out.
        assert!(text.contains("discard it and drive both again"), "{text}");
    }

    // -- the road stage ---------------------------------------------------

    #[test]
    fn two_reciprocal_passes_are_recognised_from_the_bus_and_nothing_is_asked() {
        let car = a_described_car();
        let d1 = delta1_of(&car);
        let mut stage = a_stage(&car, Vec::new());

        let first = drive(&mut stage, TRUE_CDA, TRUE_CRR, 0.0, d1);
        assert_eq!(stage.accepted().len(), 1, "{}", said(&first));
        assert!(said(&first).contains("Turn around"), "{}", said(&first));
        assert!(!stage.is_done());

        let second = drive(&mut stage, TRUE_CDA, TRUE_CRR, 600.0, d1);
        assert_eq!(stage.accepted().len(), 2);
        assert!(stage.is_done());
        assert!(second.contains(&Note::Done));

        // The design's central promise: the driver is never asked anything while
        // the car is moving. Nothing the road stage says is a question.
        let everything = format!("{}\n{}", said(&first), said(&second));
        assert!(!everything.contains('?'), "a question was put to a moving car:\n{everything}");
    }

    #[test]
    fn the_passes_recover_the_road_load_they_were_driven_at() {
        let mut car = a_described_car();
        let d1 = delta1_of(&car);
        let mut stage = a_stage(&car, Vec::new());
        drive(&mut stage, TRUE_CDA, TRUE_CRR, 0.0, d1);
        drive(&mut stage, TRUE_CDA, TRUE_CRR, 600.0, d1);

        let result = finish(&mut car, stage.accepted(), d1, "2026-08-04").unwrap();
        assert!((result.cda - TRUE_CDA).abs() < 0.01 * TRUE_CDA, "CdA {}", result.cda);
        assert!((result.crr - TRUE_CRR).abs() < 0.01 * TRUE_CRR, "Crr {}", result.crr);
        // Meaningless without the air it was fitted in, so both are recorded.
        let fit = car.fit.as_ref().unwrap();
        assert_eq!(fit.rho_at_fit, RHO);
        assert_eq!(fit.mass_at_fit_kg, MASS_KG);
        assert_eq!(fit.rho_source, Source::Measured);
        assert_eq!(fit.passes, 2);
        assert!(car.road_load().is_ok());
    }

    #[test]
    fn a_rejected_fit_leaves_the_car_file_without_road_load_rather_than_with_a_guess() {
        // Two passes that do not describe one road. The correct outcome is no
        // road load at all, which keeps --full unavailable.
        //
        // They have to disagree wildly to fail, and that is deliberate rather
        // than lax: two reciprocal passes on a 1 % slope disagree by ~88 % and
        // are exactly the pair whose *average* is right, so a tight bar would
        // reject the measurements the two-way procedure exists to make possible.
        // See `coastdown::MAX_CRR_DISAGREEMENT_PERCENT`.
        let mut car = a_described_car();
        let d1 = delta1_of(&car);
        let err = finish(&mut car, &[a_kept_pass(0.001), a_kept_pass(0.08)], d1, "2026-08-04")
            .unwrap_err();
        assert!(matches!(err, Reject::Disagreement { .. }), "{err:?}");
        assert!(car.cda.is_none(), "a guess was written");
        assert!(car.crr.is_none());
        assert!(car.fit.is_none());
        let missing = car.road_load().unwrap_err();
        assert!(missing.iter().any(|what| what.contains("CdA")), "{missing:?}");
    }

    #[test]
    fn one_pass_is_not_half_a_measurement() {
        let mut car = a_described_car();
        let d1 = delta1_of(&car);
        assert_eq!(
            finish(&mut car, &[a_kept_pass(TRUE_CRR)], d1, "2026-08-04").unwrap_err(),
            Reject::OnlyOnePass
        );
        assert!(car.cda.is_none());
        // And what is said about it ends with something to do, and with what
        // survived.
        let text = screens::fit_not_possible(&Reject::OnlyOnePass, 1);
        assert!(text.contains("vagcan measure setup"), "{text}");
        assert!(text.contains("1 pass is kept"), "{text}");
    }

    #[test]
    fn a_road_that_will_not_do_the_speed_offers_the_narrower_range() {
        let car = a_described_car();
        let mut stage = a_stage(&car, Vec::new());
        let mut notes = Vec::new();
        // Two and a half minutes of a road that tops out at 96.
        for step in 0..1500 {
            notes.extend(stage.on_sample(&Sample {
                t: step as f64 * 0.1,
                speed_kmh: 96.0,
                pedal_pct: Some(20.0),
                selector: Some("D".into()),
            }));
        }
        let text = said(&notes);
        assert!(text.contains("the fastest so far is 96"), "{text}");
        assert!(text.contains("--coast-from 90 --coast-to 30"), "{text}");
        // Offered once. A hint repeated every cycle is noise.
        assert_eq!(text.matches("still waiting for").count(), 1, "{text}");
    }

    #[test]
    fn a_pass_that_was_braked_is_rejected_and_says_which_way_to_point() {
        let car = a_described_car();
        let d1 = delta1_of(&car);
        let mut stage = a_stage(&car, Vec::new());
        let curve = coast(TRUE_CDA, TRUE_CRR, d1);
        let mut notes = Vec::new();
        for (index, (t, v)) in curve.iter().enumerate() {
            // Half way down, the driver puts a foot back on the pedal.
            let pedal = if index == curve.len() / 2 { 30.0 } else { 0.0 };
            notes.extend(stage.on_sample(&Sample {
                t: *t,
                speed_kmh: v * KMH_PER_MS,
                pedal_pct: Some(pedal),
                selector: Some("N".into()),
            }));
        }
        let text = said(&notes);
        assert!(stage.accepted().is_empty(), "{text}");
        assert!(text.contains("the pedal moved"), "{text}");
        assert!(text.contains("Stay pointing the way you are now"), "{text}");
    }

    #[test]
    fn a_pass_owed_from_an_earlier_attempt_is_the_only_one_still_wanted() {
        let car = a_described_car();
        let d1 = delta1_of(&car);
        let mut stage = a_stage(&car, vec![a_kept_pass(TRUE_CRR)]);
        assert!(!stage.is_done());
        drive(&mut stage, TRUE_CDA, TRUE_CRR, 0.0, d1);
        assert!(stage.is_done(), "the resumed pass was not counted");
        assert_eq!(stage.accepted().len(), 2);
    }

    // -- what survives an abandoned setup ---------------------------------

    #[test]
    fn an_accepted_pass_outlives_the_attempt_that_drove_it() {
        let dir = TempDir::new("kept");
        let car_file = dir.0.join("car.json");
        let path = passes_path(&car_file);
        assert_eq!(path.parent(), car_file.parent(), "it belongs beside the car file");

        let kept = vec![a_kept_pass(TRUE_CRR)];
        save_passes(&path, &kept).unwrap();
        assert_eq!(load_passes(&path), kept);

        // An empty list takes the file with it, so a finished setup leaves no
        // half-measurement for the next one to resume from.
        save_passes(&path, &[]).unwrap();
        assert!(!path.exists());
        assert!(load_passes(&path).is_empty());
    }

    #[test]
    fn a_scratch_file_nobody_can_read_costs_a_pass_and_not_the_command() {
        let dir = TempDir::new("corrupt");
        let path = dir.0.join("setup-passes.json");
        std::fs::write(&path, "{ not json at all").unwrap();
        assert!(load_passes(&path).is_empty());
        // A CdA with no air behind it is not a pass either.
        std::fs::write(&path, r#"{"passes":[{"cda":0.63,"crr":0.0114}]}"#).unwrap();
        assert!(load_passes(&path).is_empty());
    }

    #[test]
    fn a_pass_driven_in_other_air_is_not_averaged_with_todays() {
        let (kept, note) = still_valid(vec![a_kept_pass(TRUE_CRR)], RHO * 1.01, MASS_KG);
        assert_eq!(kept.len(), 1, "one per cent of density is not another day");
        assert!(note.is_none());

        let (kept, note) = still_valid(vec![a_kept_pass(TRUE_CRR)], RHO * 1.10, MASS_KG);
        assert!(kept.is_empty(), "a CdA fitted at another density was averaged in");
        assert!(note.unwrap().contains("density"));

        let (kept, _) = still_valid(vec![a_kept_pass(TRUE_CRR)], RHO, MASS_KG + 200.0);
        assert!(kept.is_empty(), "a pass fitted at another load was averaged in");
    }

    // -- the screens ------------------------------------------------------

    #[test]
    fn the_road_is_explained_while_the_car_is_still_parked() {
        let text = screens::road_briefing(COAST_FROM_KMH, COAST_TO_KMH, 1.183);
        for promise in [
            "kilometre of clear, flat, dry road",
            "once in each direction",
            "30 to 45 seconds",
            "select N",
            "not ask you anything while you are moving",
            "every accepted pass is kept",
        ] {
            assert!(text.contains(promise), "missing {promise:?}:\n{text}");
        }
    }

    #[test]
    fn the_closing_screen_says_what_the_car_is_now_known_to_be() {
        let mut car = a_described_car();
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
        let result = RoadLoadResult {
            cda: 0.63,
            crr: 0.0114,
            implied_grade_percent: 0.3,
            implied_wind_ms: 0.8,
        };
        let text = screens::complete(&car, Path::new("/tmp/cars/x/car.json"), &result);
        assert!(text.contains("Setup complete — /tmp/cars/x/car.json"), "{text}");
        assert!(text.contains("1475 kg"), "{text}");
        assert!(text.contains("205/55R16"), "{text}");
        assert!(text.contains("0.63 m²"), "{text}");
        // Every figure says which kind of claim it is.
        assert!(text.contains("you, 2026-08-03"), "{text}");
        assert!(text.contains("measured on this car, 2 passes"), "{text}");
        // The fit's own conditions, without which a CdA means nothing.
        assert!(text.contains("1.183 kg/m³"), "{text}");
        assert!(text.contains("1385 kg at fit time"), "{text}");
        // What it unlocks, and the two things worth doing once.
        assert!(text.contains("vagcan measure --full"), "{text}");
        assert!(text.contains("--speed-scale"), "{text}");
    }

    #[test]
    fn a_car_that_cannot_show_a_coast_is_refused_at_a_standstill() {
        // A manual car with no selector channel: the pass would never open, and
        // finding that out after a kilometre of clear road is exactly what the
        // parked check exists to prevent.
        let speed = a_channel("speed", 0x7E1, 0xF40D);
        let with_neither = a_set(vec![speed.clone()]);
        let text = screens::coast_impossible(&with_neither).expect("refused");
        assert!(text.contains("pedal and selector"), "{text}");
        assert!(text.contains("vagcan survey"), "{text}");

        let complete = a_set(vec![
            speed,
            a_channel("pedal", 0x7E0, 0xF449),
            a_channel("selector", 0x7E1, 0x1234),
        ]);
        assert!(screens::coast_impossible(&complete).is_none());
    }

    fn a_set(channels: Vec<Resolved>) -> Set {
        Set {
            leading: channels[0].clone(),
            leading_batch: channels,
            background: Vec::new(),
            cross_check_speeds: Vec::new(),
        }
    }

    fn a_channel(key: &'static str, request: u16, did: u16) -> Resolved {
        use vag_data::catalog::{MeasurementDef, ReadId, Scaling};
        use vag_data::measure::{LinearScale, RawForm};
        Resolved {
            key,
            request,
            did,
            def: MeasurementDef {
                address: ReadId::Uds(did),
                name: key.into(),
                unit: "".into(),
                raw_form: RawForm::U8First,
                scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 }),
            },
        }
    }
}
