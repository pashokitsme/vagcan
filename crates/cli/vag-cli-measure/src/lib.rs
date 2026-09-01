//! `vagcan measure` — time an acceleration run from the car's own speed signal.
//!
//! The design is `docs/superpowers/specs/2026-08-03-measure-design.md`; the two
//! rules that shape every module here are worth repeating at the door.
//!
//! **Two kinds of number, and only two.** Everything shown is either *read* — a
//! value that was on the bus and whose meaning is proven by a catalog row or by
//! SAE J1979 — or *derived*, computed here from read values. The `(raw)` class
//! the rest of this crate deals with does not exist in `measure`: an unproven byte
//! cannot be timed, integrated or differentiated, and a channel that will not
//! resolve is a channel this command does without.
//!
//! **The file holds raw samples; derivatives are recomputed.** Nothing shown
//! live is ever saved. That is what lets a method be corrected afterwards
//! without re-driving the car, and every correction in the design's history so
//! far has needed it.
//!
//! This file is the command and the poll loop. Two things about the loop are
//! load-bearing and easy to undo by accident:
//!
//! * **The keyboard is drained between batches, never around one.** A batch read
//!   can sit out a unit's two-second response deadline, and `Esc` must not wait
//!   with it. Wrapping the read in a `select!` would be worse than useless: the
//!   backend is `take()`n out of an `Option` and put back after the await, so a
//!   dropped future leaves the adapter silently gone for the rest of the run.
//! * **No session control is ever sent.** `measure` reads a fixed handful of
//!   known identifiers with `0x22` and nothing else. The danger is what a sweep
//!   can provoke; this is `watch` with a stopwatch.

use std::collections::BTreeMap;
use std::io;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{self, Event as TermEvent, KeyEventKind};
use ratatui::prelude::*;
use serde_json::{Map, Value, json};

// `ui` is deliberately not re-exported: this crate has its own, the live
// stopwatch screen, and core's shared widgets are reached by their own name.
pub use vag_cli_core::{analyse, config, datadir, device, extracted, glossary, plan, progress, project, units, vcdslog};

pub mod args;
pub mod carfile;
pub mod channels;
pub mod coastdown;
pub mod derive;
pub mod messages;
pub mod power;
pub mod report;
pub mod reread;
pub mod session;
pub mod setup;
pub mod types;
pub mod ui;
pub mod view;

use channels::Resolved;
use types::{Seconds, Track};
use vag_cli_core::ui::{chart, term};

/// The session file's format. Checked before anything else is read.
///
/// A session cannot be regenerated — it is evidence from a drive — so a reader
/// refuses a schema it does not know, naming it, rather than half-reading the
/// file and reporting whichever fields happened to survive.
pub const SCHEMA: u64 = 1;

/// The marks a run measures when nobody said otherwise, in **km/h**.
///
/// The unit is stated wherever this list appears, because `0-60` is the American
/// figure and that one is in mph — an ambiguity that would otherwise sit
/// unnoticed in the default itself.
pub const DEFAULT_MARKS: &str = "0-10,0-25,0-50,0-60,0-80,0-100";

/// How much of the approach a run keeps before the launch.
///
/// The pedal, the engine and the selector *before* the start are half of what
/// explains a bad one, and none of them can be recovered afterwards.
const RING_SECONDS: Seconds = 3.0;

/// How many cycles the leading unit may fail to answer before the car counts as
/// having stopped answering.
///
/// Enough that one timed-out request is not an ignition-off — a unit replying
/// "response pending" can stall legally — and few enough that a pulled connector
/// is noticed before the driver has gone another kilometre.
const SILENT_CYCLES: u32 = 10;

/// How much of a run's tail the live chart keeps, in seconds.
///
/// The chart is drawn from the accumulated buffer rather than the last point, so
/// the shape of the run is visible while it happens; a buffer with no bound
/// would grow for as long as the tool is left running at a kerbside.
const CHART_SECONDS: Seconds = 30.0;

/// A parsed `--marks` list.
///
/// A newtype rather than a bare `Vec`, so that clap parses the whole flag with
/// one value parser and a bad list fails before the adapter is opened — the
/// reason `duration_arg` exists in `main.rs` today.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Marks(pub Vec<(u32, u32)>);

/// Parse `0-100,50-100`: `A-B` pairs in **km/h**, `A < B`.
pub fn parse_marks(text: &str) -> Result<Marks, String> {
	let mut out = Vec::new();
	for pair in text.split(',').map(str::trim).filter(|s| !s.is_empty()) {
		let (from, to) = pair
			.split_once('-')
			.ok_or_else(|| format!("{pair:?} is not a mark — write it as `0-100`, in km/h"))?;
		let number = |text: &str| {
			text
				.trim()
				.parse::<u32>()
				.map_err(|_| format!("{text:?} in {pair:?} is not a speed in km/h"))
		};
		let (from, to) = (number(from)?, number(to)?);
		if from >= to {
			return Err(format!("{pair:?} does not rise — a mark runs from a lower speed to a higher one"));
		}
		out.push((from, to));
	}
	if out.is_empty() {
		return Err("no marks given — the default is `0-10,0-25,0-50,0-60,0-80,0-100`, in km/h".to_string());
	}
	Ok(Marks(out))
}

/// Parse `--speed-scale`: a positive, finite multiplier.
///
/// It is applied **before** mark detection, so a value that is not a number
/// would corrupt every time rather than merely a printed figure.
pub fn parse_speed_scale(text: &str) -> Result<f64, String> {
	let value: f64 = text.parse().map_err(|_| format!("{text:?} is not a number"))?;
	match value.is_finite() && value > 0.0 {
		true => Ok(value),
		false => Err(format!("{text:?} is not a positive speed correction")),
	}
}

/// Parse a positive number of seconds — the acceleration window.
pub fn parse_seconds(text: &str) -> Result<f64, String> {
	let value: f64 = text.parse().map_err(|_| format!("{text:?} is not a number"))?;
	match value.is_finite() && value > 0.0 {
		true => Ok(value),
		false => Err(format!("{text:?} is not a positive number of seconds")),
	}
}

/// The two jobs that are not "time a run".
///
/// Subcommands rather than flags: `setup` prompts, runs a driving script, takes
/// flags that mean nothing elsewhere and produces a different artefact, and
/// `view` touches no adapter at all. The repository already groups that way with
/// `recording` and `vcds`.
// Clone for the reason the other command enums are: `vagcan`'s dispatcher keeps
// a copy of the command so it can be run again after the label data has been
// made.
#[derive(Clone, clap::Subcommand)]
pub enum Tool {
	/// Describe this car once, then measure its road load on the road.
	///
	/// Starts parked and ends on the road: it identifies the car, checks every
	/// channel a run needs, asks for the mass and the tyre size, and then runs
	/// two coastdown passes — one in each direction — whose fit is what makes
	/// `--full` available. It keeps whatever was already answered.
	Setup {
		/// Adapter to use. Omit it when only one is connected.
		#[arg(long, value_name = "PATH")]
		device: Option<String>,
		/// The speed a coastdown pass opens at. Narrowing the range separates
		/// drag from rolling resistance less well, and the fit says by how much.
		#[arg(long, default_value_t = setup::COAST_FROM_KMH, value_name = "KMH")]
		coast_from: f64,
		/// The speed a coastdown pass closes at.
		#[arg(long, default_value_t = setup::COAST_TO_KMH, value_name = "KMH")]
		coast_to: f64,
		/// Where the proven measurement rows live.
		/// Default: this project's `~/.vagcan/data/<project>/measurements`.
		#[arg(long, value_name = "DIR")]
		data: Option<String>,
		/// Write the car file here instead of into this tool's own directory.
		#[arg(long, value_name = "FILE")]
		car: Option<String>,
	},

	/// Open a saved session as a chart page. Offline — no adapter.
	View {
		/// A session file written by `--out` or by pressing `s`. Omit it and
		/// this tool's own directory is offered: a car, then one of its
		/// sessions.
		#[arg(value_name = "FILE")]
		file: Option<String>,
	},
}

/// Everything `measure` was asked for.
///
/// Every override here normally lives in the car file. They exist for a one-off
/// — a loaded boot, a different set of wheels — and what was used is recorded in
/// every file that comes out.
pub struct Options<'a> {
	pub device: Option<&'a str>,
	pub car: Option<&'a str>,
	pub catalogs: &'a str,
	pub full: bool,
	pub minimal: bool,
	pub marks: Vec<(u32, u32)>,
	pub accel_window_s: f64,
	pub out: Option<&'a str>,
	pub quiet: bool,
	pub mass_kg: Option<f64>,
	pub tyre: Option<&'a str>,
	pub cda: Option<f64>,
	pub crr: Option<f64>,
	pub inertia_factor: Option<f64>,
	pub grade_percent: f64,
	pub headwind_ms: f64,
	pub air_density: Option<f64>,
	pub speed_scale: f64,
}

/// Read one batch of identifiers.
///
/// The seam the loop is tested behind: the scheduling — two batches a cycle, the
/// background one every second cycle, the barometer once a run — is decided here
/// and can be observed with no CAN at all. The live implementation is
/// [`LiveReader`], over `plan::read_batch`.
pub trait BatchReader {
	fn read(&mut self, batch: &crate::plan::Batch) -> impl std::future::Future<Output = (Seconds, crate::plan::BatchOutcome)>;
}

/// The live reader: one adapter, addressed a unit at a time.
///
/// The backend lives in an `Option` because it is a single-user resource with no
/// way to borrow it across an await — it is handed over and handed back, which
/// is also why this future must never be dropped mid-flight.
pub struct LiveReader<B> {
	backend: Option<B>,
	started: Instant,
}

impl<B: vag_uds_can::CanBackend> BatchReader for LiveReader<B> {
	async fn read(&mut self, batch: &crate::plan::Batch) -> (Seconds, crate::plan::BatchOutcome) {
		crate::plan::read_batch(&mut self.backend, batch, self.started).await
	}
}

/// Which channels are polled at which cadence, already grouped into requests.
///
/// One request addresses one control unit, so the grouping is by unit and not by
/// taste. The leading batch — everything on the unit that owns the winning speed
/// channel — is read every cycle; everything else every second cycle, because
/// marks are timed from the leading speed alone and its rate is the only one
/// that sets a stopwatch.
struct Plan {
	leading: crate::plan::Batch,
	background: Vec<crate::plan::Batch>,
	/// The barometer and the ambient sensor, read **once per run** and at the
	/// end of it. Once, because neither moves measurably in seven seconds and
	/// polling them at 20 Hz would cost cycles for no information; at the end,
	/// because the ambient sensor heat-soaks at a standstill and +10 K reads the
	/// air density 3.4 % low.
	density: Option<crate::plan::Batch>,
	/// Every resolved channel by address, for turning an answer back into a
	/// value.
	by_address: BTreeMap<(u16, u16), Resolved>,
}

impl Plan {
	fn build(set: &channels::Set, minimal: bool) -> Plan {
		let wanted = |channel: &Resolved| match minimal {
			// `--minimal` polls only what the stopwatch needs, for the highest
			// achievable rate and at the cost of the telemetry. A deliberate
			// trade, and therefore a flag rather than a hidden heuristic.
			true => matches!(channel.key, "speed" | "gear" | "cross-check speed"),
			false => true,
		};
		let density_role = |key: &str| matches!(key, "barometer" | "ambient");

		let mut by_address = BTreeMap::new();
		let mut leading = crate::plan::Batch {
			request: set.leading.request,
			dids: vec![],
		};
		let mut background: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
		let mut density: BTreeMap<u16, Vec<u16>> = BTreeMap::new();

		for channel in set.all() {
			if !wanted(channel) {
				continue;
			}
			by_address.insert((channel.request, channel.did), channel.clone());
			let into = if density_role(channel.key) {
				density.entry(channel.request).or_default()
			} else if channel.request == set.leading.request {
				&mut leading.dids
			} else {
				background.entry(channel.request).or_default()
			};
			into.push(channel.did);
		}

		let batches = |grouped: BTreeMap<u16, Vec<u16>>| {
			grouped
				.into_iter()
				.map(|(request, dids)| crate::plan::Batch { request, dids })
				.collect::<Vec<_>>()
		};
		Plan {
			leading,
			background: batches(background),
			// The two density channels always live on the same emissions unit,
			// so this is one request or none.
			density: batches(density).into_iter().next(),
			by_address,
		}
	}
}

/// A speed reading in metres per second, from whatever the catalog said its
/// unit was.
///
/// `None` for a unit this does not know, which makes the channel absent rather
/// than off by a factor of 3.6. The conversions are exact and are properties of
/// the units themselves: ISO 80000-3 for km/h, and the international mile for
/// mph.
fn speed_to_ms(unit: &str, value: f64) -> Option<f64> {
	match unit.trim() {
		"km/h" | "kmh" | "kph" => Some(value / power::KMH_PER_MS),
		"m/s" | "ms" => Some(value),
		"mph" => Some(value * 0.447_04),
		_ => None,
	}
}

/// One cycle's answers, keyed by address.
type Records = Vec<(u16, u16, Vec<u8>)>;

/// Turn one batch's answers into the state machine's input.
///
/// Only what was answered goes in. Repeating a background value on the cycles it
/// was not read would invent samples the car never produced, and would make the
/// leading channel's refresh period unmeasurable.
///
/// Every value carries `at` — the batch's own arrival time. Batches are
/// separated in time, and one shared timestamp has already cost this project a
/// wrong proof: the gear evidence moved from η² 0.872 to 0.972 when the columns
/// got their own clocks.
fn sample_set(plan: &Plan, records: &Records, at: Seconds) -> session::SampleSet {
	let mut set = session::SampleSet::default();
	let mut cross_check_taken = false;
	for (request, did, data) in records {
		let Some(channel) = plan.by_address.get(&(*request, *did)) else {
			continue;
		};
		match channel.key {
			"speed" => {
				let Some(value) = channel.value(data) else { continue };
				let Some(ms) = speed_to_ms(&channel.def.unit, value) else { continue };
				// The raw integer travels beside the converted value because
				// standstill is decided on it and on nothing else: the scale is
				// applied before mark detection, and `v == 0.0` on a scaled
				// float is a comparison this project would regret.
				let Some(raw) = channel.def.raw_form.read(data) else { continue };
				set.speed = Some((at, ms, raw.unsigned_abs()));
			}
			"engine speed" => {
				if let Some(value) = channel.value(data) {
					set.engine_speed = Some((at, value));
				}
			}
			"pedal" => {
				if let Some(value) = channel.value(data) {
					set.pedal = Some((at, value));
				}
			}
			"gear" => {
				if let Some(label) = channel.state(data) {
					set.gear = Some((at, label));
				}
			}
			"selector" => {
				if let Some(label) = channel.state(data) {
					set.states.push((channel.key, at, label));
				}
			}
			// Several units can answer to "speed"; they share one role key, so
			// only the first is carried as a channel of its own. Merging two
			// units' speeds into one track would invent a signal neither
			// reported.
			"cross-check speed" if cross_check_taken => {}
			key => {
				if let Some(value) = channel.value(data) {
					cross_check_taken |= key == "cross-check speed";
					set.others.push((key, at, value));
				}
			}
		}
	}
	set
}

/// One finished run, with the derivative layer already recomputed.
///
/// Recomputed here rather than in the writer, because the page and the results
/// table have no arithmetic layer of their own and could not honour the storage
/// rule by themselves.
struct Recorded {
	run: session::Run,
	derived: report::Derived,
	at: String,
}

/// What the whole session was recorded under.
struct Meta {
	vin: Option<String>,
	units: Vec<crate::plan::UnitIdentity>,
	marks: Vec<(u32, u32)>,
	speed_source: String,
	speed_scale: f64,
	accel_window_s: f64,
	setting: report::Setting,
	grade_percent: f64,
	headwind_ms: f64,
	car_file: Option<String>,
	channels: Vec<Value>,
}

/// Everything a `measure` command needs to know before it can draw a screen.
struct Prepared {
	plan: Plan,
	meta: Meta,
	banner: String,
	/// Present exactly when `--full` was accepted.
	full: bool,
}

/// A channel's key as the session file spells it: `snake_case`, one spelling in
/// the document, so the reader never has to normalise.
fn file_key(role: &str) -> String {
	role.replace(['-', ' '], "_")
}

/// The channel descriptors for the file: every read channel, then every derived
/// one with what it was derived from and by what method.
fn channel_descriptors(plan: &Plan, full: bool, window: f64) -> Vec<Value> {
	let mut out: Vec<Value> = Vec::new();
	for ((request, did), channel) in &plan.by_address {
		out.push(json!({
				"key": file_key(channel.key),
				"name": channel.def.name,
				"unit": channel.def.unit,
				"origin": "read",
				"request": format!("{request:03X}"),
				"did": format!("{did:04X}"),
		}));
	}
	out.push(json!({
			"key": "accel", "name": "Acceleration", "unit": "m/s2", "origin": "derived",
			"from": ["speed"], "method": "central-least-squares", "window_s": window,
	}));
	out.push(json!({
			"key": "distance", "name": "Distance travelled", "unit": "m", "origin": "derived",
			"from": ["speed"], "method": "trapezoid",
	}));
	out.push(json!({
			"key": "kickdown", "name": "Kickdown", "unit": "", "origin": "derived",
			"from": ["pedal"], "method": "pedal at observed maximum less one raw step",
	}));
	if full {
		out.push(json!({
				"key": "power_wheel", "name": "Power at the wheels", "unit": "kW",
				"origin": "derived", "estimate": true,
				"from": ["speed", "barometer", "ambient"], "method": "road-load",
		}));
		out.push(json!({
				"key": "power_shaft", "name": "Power including engine-side inertia", "unit": "kW",
				"origin": "derived", "estimate": true,
				"from": ["power_wheel", "engine_speed"],
				"method": "road-load+engine-inertia", "suppressed_when": "clutch slipping",
		}));
	}
	out
}

/// A numeric channel, columnar: its own values against its own timestamps.
fn track_json(track: &Track) -> Value {
	json!({ "t": track.t, "v": track.v })
}

/// The whole session, as the file holds it.
///
/// Columnar rather than a list of objects: twenty runs of ten seconds at 20 Hz
/// over ten channels is about a megabyte written the verbose way, and
/// `measure view` inlines that into a page a browser has to load.
fn document(meta: &Meta, recorded: &[Recorded], degraded: bool, hz: Option<f64>, cycle_median_s: Option<f64>) -> Value {
	let mut config = Map::new();
	config.insert("marks".into(), json!(meta.marks.iter().map(|(a, b)| [a, b]).collect::<Vec<_>>()));
	config.insert("speed_source".into(), json!(meta.speed_source));
	config.insert("speed_scale".into(), json!(meta.speed_scale));
	config.insert("speed_scale_applied".into(), json!("before-marks"));
	config.insert("accel_window_s".into(), json!(meta.accel_window_s));
	config.insert("accel_method".into(), json!("central-least-squares"));
	config.insert("t0_method".into(), json!("quadratic-and-linear-bracket"));
	config.insert("peak_statistic".into(), json!("mean-over-neighbourhood"));
	config.insert("inertia_model".into(), json!("exact-engine-side"));
	config.insert("grade_percent".into(), json!(meta.grade_percent));
	config.insert("headwind_ms".into(), json!(meta.headwind_ms));
	config.insert("degraded".into(), json!(degraded));
	if let Some(hz) = hz {
		config.insert("hz".into(), json!(round3(hz)));
	}
	if let Some(median) = cycle_median_s {
		config.insert("cycle_median_s".into(), json!(round3(median)));
	}
	if let Some(tyre) = &meta.setting.tyre {
		config.insert("tyre".into(), json!(tyre));
	}
	if let Some(model) = &meta.setting.model {
		config.insert("mass_kg".into(), json!(model.conditions.mass_kg));
		config.insert("rolling_radius_m".into(), json!(model.conditions.radius_m));
		config.insert("i_wheels_kgm2".into(), json!(model.conditions.inertias.wheels_kgm2));
		config.insert("i_engine_kgm2".into(), json!(model.conditions.inertias.engine_kgm2));
		config.insert("cda".into(), json!(model.load.cda));
		config.insert("crr".into(), json!(model.load.crr));
	}
	if let Some(path) = &meta.car_file {
		config.insert("car_file".into(), json!(path));
	}
	// Whatever density the power model actually used, and where it came from.
	// Taken off the setting rather than off what a run happened to read, because
	// a run reads nothing in the one case worth recording most: a car with no
	// barometer, where the figure is the ISO 2533 standard atmosphere and the
	// file used to say nothing at all about the number every power figure in it
	// rests on.
	if let Some(model) = &meta.setting.model {
		config.insert("air_density_kg_m3".into(), json!(round3(model.conditions.rho)));
		config.insert("air_density_source".into(), json!(meta.setting.rho_from.as_str()));
	}
	if let Some(refresh) = recorded.iter().rev().find_map(|r| r.derived.refresh_s) {
		config.insert("refresh_estimate_s".into(), json!(round3(refresh)));
		config.insert("refresh_is_a_bound".into(), json!(true));
	}

	let units: Vec<Value> = meta
		.units
		.iter()
		.map(|unit| json!({ "request": format!("{:03X}", unit.request), "part_number": unit.part_number }))
		.collect();

	json!({
			"schema": SCHEMA,
			"tool": "vagcan measure",
			"recorded_at": now(),
			"car": { "vin": meta.vin, "units": units },
			"config": Value::Object(config),
			"channels": meta.channels,
			"runs": recorded.iter().map(|r| run_json(meta, r)).collect::<Vec<_>>(),
	})
}

/// The `derived` block: the cache of everything computed over a finished run.
///
/// Split out of [`run_json`] so that [`refresh_derived`] can rebuild exactly
/// this and nothing else. Two builders would drift, and the drift would show
/// as a figure that changes when a session is re-opened.
fn derived_json(derived: &report::Derived) -> Map<String, Value> {
	let mut block = Map::new();
	block.insert("stamp".into(), json!(derived.stamp));
	block.insert("distance_m".into(), json!(round3(derived.distance_m)));
	if let Some(peak) = derived.peak_engine_speed {
		block.insert("peak_rpm".into(), json!(round3(peak.value)));
	}
	if let Some(peak) = derived.peak_accel {
		block.insert("peak_accel_ms2".into(), json!(round3(peak.value)));
		block.insert("peak_accel_t".into(), json!(round3(peak.t)));
		block.insert("peak_accel_sigma".into(), json!(round3(peak.sigma)));
	}
	if let Some(gear) = &derived.peak_accel_gear {
		block.insert("peak_accel_gear".into(), json!(gear));
	}
	if let Some(kw) = derived.peak_power_wheel_kw {
		block.insert("peak_power_wheel_kw".into(), json!(round3(kw)));
	}
	if let Some(kw) = derived.peak_power_shaft_kw {
		block.insert("peak_power_shaft_kw".into(), json!(round3(kw)));
	}
	if let Some(reference) = derived.boost_reference {
		block.insert("boost_reference".into(), json!(reference));
	}
	block.insert(
		"shifts".into(),
		json!(
			derived
				.shifts
				.iter()
				.map(|shift| json!({
						"t": round3(shift.t), "from": shift.from, "to": shift.to,
						"speed_deficit_ms": round3(shift.speed_deficit_ms),
						// The floor travels with the figure, so the page can say
						// "not resolved" for the same rows the text report does
						// rather than reprint a sign the session never measured.
						"deficit_sigma_ms": round3(shift.deficit_sigma_ms),
						"cost_on_mark_s": shift.cost_on_mark_s.map(round3),
				}))
				.collect::<Vec<_>>()
		),
	);
	block
}

fn run_json(meta: &Meta, recorded: &Recorded) -> Value {
	let run = &recorded.run;
	let derived = &recorded.derived;
	let mut series = Map::new();
	series.insert("speed".into(), track_json(&run.samples.speed));
	series.insert("engine_speed".into(), track_json(&run.samples.engine_speed));
	series.insert("pedal".into(), track_json(&run.samples.pedal));
	series.insert("gear".into(), json!({ "t": run.samples.gear.t, "v": run.samples.gear.v }));
	for (key, track) in &run.samples.others {
		series.insert(file_key(key), track_json(track));
	}
	for (key, states) in &run.samples.states {
		series.insert(file_key(key), json!({ "t": states.t, "v": states.v }));
	}
	series.insert("accel".into(), track_json(&report::as_track(&derived.accel)));
	series.insert("distance".into(), track_json(&derived.distance));
	if let Some(kickdown) = &derived.kickdown {
		series.insert("kickdown".into(), track_json(kickdown));
	}
	if !derived.power_wheel.is_empty() {
		series.insert("power_wheel".into(), track_json(&scaled(&derived.power_wheel, 0.001)));
		series.insert("power_shaft".into(), track_json(&scaled(&derived.power_shaft, 0.001)));
	}

	// Marks that never closed are still listed, with no time: a run that died at
	// 80 says so by having `0-100` present and empty, not by omitting it.
	let marks: Vec<Value> = meta
		.marks
		.iter()
		.map(|(from, to)| match run.marks.iter().find(|m| m.from_kmh == *from && m.to_kmh == *to) {
			None => json!({ "from": from, "to": to, "from_t0": *from == 0 }),
			Some(mark) => {
				let mut entry = Map::new();
				entry.insert("from".into(), json!(from));
				entry.insert("to".into(), json!(to));
				entry.insert("seconds".into(), json!(round3(mark.seconds)));
				entry.insert("from_t0".into(), json!(mark.starts_at_launch()));
				entry.insert("avg_accel_ms2".into(), json!(round3(mark.avg_accel_ms2())));
				match mark.bracket {
					Some(span) => {
						entry.insert(
							"bracket_s".into(),
							json!({
									"earliest": round3(span.earliest),
									"latest": round3(span.latest),
									"from": "quadratic-and-linear-t0",
							}),
						);
					}
					None => {
						if let Some(sigma) = derived.rolling_sigma() {
							entry.insert("sigma_s".into(), json!(round3(sigma)));
						}
					}
				}
				Value::Object(entry)
			}
		})
		.collect();

	let block = derived_json(derived);
	json!({
			"index": run.index,
			"t0_wall": recorded.at,
			"aborted": run.aborted,
			"degraded": run.degraded,
			"series": Value::Object(series),
			"marks": marks,
			"derived": Value::Object(block),
	})
}

fn scaled(track: &Track, by: f64) -> Track {
	let mut out = Track::default();
	for i in 0..track.len() {
		out.push(track.t[i], track.v[i] * by);
	}
	out
}

/// Three decimals, so that a millisecond and a gram survive and the file does
/// not carry seventeen digits of float noise per sample.
fn round3(value: f64) -> f64 {
	(value * 1000.0).round() / 1000.0
}

fn now() -> String {
	chrono::Local::now().to_rfc3339()
}

/// `measure view` with nothing to open — offer what has been recorded.
///
/// A car, then one of its sessions, newest first: a session is named for the
/// minute it was driven, so a list in name order puts the drive somebody just
/// finished at the bottom, which is the wrong end for the reason they are
/// looking.
pub fn view_picked() -> Result<()> {
	let cars = crate::datadir::vagcan_dir()?.join("cars");
	let levels = [
		vag_cli_core::ui::picker::Level::directories("car").filled_by("vagcan measure   records one"),
		vag_cli_core::ui::picker::Level::files("session")
			.within("measures")
			.ending(".json")
			.newest_first()
			.filled_by("vagcan measure   then press `s` to keep the drive"),
	];
	let mut chooser = vag_cli_core::ui::picker::Console::new("vagcan measure view FILE.json");
	match vag_cli_core::ui::picker::pick_path(&mut chooser, &cars, &levels)? {
		Some(path) => open_view(&path.to_string_lossy()),
		// Backing out of the first list is an answer, not a failure.
		None => Ok(()),
	}
}

/// `measure view FILE.json` — open a saved session as a chart page.
///
/// Touches no adapter. The schema is checked before anything else, by name: a
/// session is evidence from a drive and cannot be regenerated, so half-reading
/// one is worse than refusing it.
pub fn open_view(path: &str) -> Result<()> {
	let text = std::fs::read_to_string(path).with_context(|| format!("reading the session {path:?}"))?;
	let session: Value = serde_json::from_str(&text).with_context(|| format!("{path} is not a session this tool wrote"))?;
	match session["schema"].as_u64() {
		Some(SCHEMA) => {}
		Some(other) => anyhow::bail!(
			"{path} is schema {other} and this build reads schema {SCHEMA}.\n\
             A session cannot be regenerated, so it is left alone rather than half-read.\n\
             Use a build that knows schema {other}."
		),
		None => anyhow::bail!("{path} carries no `schema` field, so it is not a session this tool wrote."),
	}
	let mut session = session;
	refresh_derived(&mut session);
	view::write_and_open(std::path::Path::new(path), &session)?;
	Ok(())
}

/// Re-derive every run from its own samples, so an old file is read by today's
/// maths rather than shown yesterday's answers.
///
/// The `derived` block is a cache over `series`, and `stamp` exists so a reader
/// can tell it was computed by something else — `report`'s module doc says so.
/// This is that reader. Without it a session recorded before the shift cost was
/// corrected kept showing costs taken against a baseline that could land on a
/// lift, downshifts charged as though they were shifts, and signs below the
/// session's own noise.
///
/// **A run that cannot be rebuilt faithfully keeps what it has.** Recomputing
/// over samples this build did not fully understand would replace one wrong
/// answer with a different one, so the reader refuses instead ([`reread`]) and
/// the stamp goes on saying which maths produced what is shown.
///
/// The power block is deliberately not recomputed. It needs the car's mass and
/// road load, and those live in a car file that may have been edited, replaced
/// or measured again since — so a power figure is left exactly as it was
/// computed, with the conditions the file records beside it.
fn refresh_derived(session: &mut Value) {
	let setting = report::Setting {
		accel_window_s: session["config"]["accel_window_s"].as_f64().unwrap_or_default(),
		peak_tau_s: report::PEAK_TAU_S,
		..report::Setting::default()
	};
	let Some(runs) = session.get_mut("runs").and_then(Value::as_array_mut) else {
		return;
	};
	for value in runs.iter_mut() {
		let Some(run) = reread::run(value) else { continue };
		// Only the block that is a cache. Everything else in the run — the
		// samples, the marks, when it was driven — is evidence and is left
		// exactly as the drive left it.
		merge_derived(value, Value::Object(derived_json(&report::recompute(&run, &setting))));
	}
}

/// Replace the cached block, keeping any figure this build could not recompute.
///
/// Power is the case that matters: it is absent from a recomputed block because
/// no car file was loaded, and dropping it would silently delete a figure the
/// drive really did produce.
fn merge_derived(run: &mut Value, fresh: Value) {
	let (Some(old), Some(new)) = (run["derived"].as_object(), fresh.as_object()) else {
		return;
	};
	let mut merged = old.clone();
	for (key, value) in new {
		merged.insert(key.clone(), value.clone());
	}
	run["derived"] = Value::Object(merged);
}

/// Run the command against a car.
pub async fn run(opts: Options<'_>) -> Result<()> {
	use vag_uds_can::{SlcanBackend, SlcanBitrate, SlcanMode};

	// Argument checking before the adapter, which is a single-user resource: an
	// unwritable `--out` is the same typo as a bad `--marks`, and holding the
	// port open while failing on either blocks the next attempt.
	let store = vag_data_labels::catalog::CatalogStore::open(opts.catalogs);
	if let Some(path) = opts.out {
		std::fs::File::create(path).with_context(|| format!("creating {path:?}"))?;
	}

	let device = crate::device::resolve(opts.device)?;
	let adapter = SlcanBackend::open_mode(&device, vag_cli_core::device::ADAPTER_BAUD, SlcanBitrate::Rate500k, SlcanMode::Normal)
		.await
		.with_context(|| crate::device::open_failure(&device))?;

	let mut progress = crate::progress::Line::new();
	let (mut adapter, identities) = crate::units::identify(adapter, &[crate::plan::ENGINE], &[], &mut progress).await;
	progress.update("reading the vehicle identification number");
	let (back, vin) = crate::units::read_vin(adapter).await;
	adapter = back;
	progress.finish();

	let prepared = prepare(&store, &crate::extracted::current(), &identities, vin.clone(), &opts)?;
	if !prepared.banner.is_empty() {
		println!("{}", prepared.banner);
	}
	println!(
		"{} channels on {} control units — timing from {}",
		prepared.plan.by_address.len(),
		identities.len(),
		prepared.meta.speed_source
	);

	let reader = LiveReader {
		backend: Some(adapter),
		started: Instant::now(),
	};
	let full_screen = std::io::IsTerminal::is_terminal(&std::io::stdout());
	drive(reader, prepared, &opts, full_screen).await
}

/// Resolve the channels, settle what mode the run is in, and say so.
///
/// Every refusal here happens at a standstill, before a screen is drawn, and
/// every one of them goes through [`messages`].
fn prepare(
	store: &vag_data_labels::catalog::CatalogStore,
	extracted: &crate::extracted::Extracted,
	identities: &[crate::plan::UnitIdentity],
	vin: Option<String>,
	opts: &Options<'_>,
) -> Result<Prepared> {
	let set = channels::resolve(store, extracted, identities, opts.full).map_err(|missing| {
		let found = found_report(identities, &missing);
		anyhow::anyhow!(
			"{}",
			messages::missing_channels(
				&found,
				&missing
					.into_iter()
					.map(|m| messages::MissingChannel { key: m.key, tried: m.tried })
					.collect::<Vec<_>>()
			)
		)
	})?;

	let (car, car_path) = load_car_file(vin.as_deref(), opts);
	let mut setting = report::Setting {
		accel_window_s: opts.accel_window_s,
		tyre: opts
			.tyre
			.map(str::to_string)
			.or_else(|| car.as_ref().and_then(|c| c.tyre.as_ref().map(|t| t.value.clone()))),
		pedal_step: pedal_step(&set),
		units: channel_units(&set),
		..report::Setting::default()
	};

	let banner = if opts.full {
		// A car with no file at all takes the same path as one whose setup was
		// abandoned halfway: the refusal names every parameter that is missing,
		// and there is deliberately no state in between the two modes.
		let described = car.clone().unwrap_or_else(|| carfile::CarFile::new(vin.clone().unwrap_or_default()));
		let (load, conditions) =
			road_load(&described, opts).map_err(|missing| anyhow::anyhow!("{}", messages::full_without_car_file(&known_of(&described), &missing)))?;
		setting.model = Some(report::Model {
			load,
			conditions: power::Conditions {
				mass_kg: conditions.mass_kg,
				// Replaced with the measured value at the end of the first run;
				// until then this is whatever was stated, or the sea-level
				// standard, and the file records which.
				rho: opts.air_density.unwrap_or(power::air_density(101.325, 15.0)),
				grade_percent: opts.grade_percent,
				headwind_ms: opts.headwind_ms,
				inertias: power::Inertias {
					wheels_kgm2: conditions.i_wheels_kgm2 * opts.inertia_factor.unwrap_or(1.0),
					engine_kgm2: conditions.i_engine_kgm2 * opts.inertia_factor.unwrap_or(1.0),
				},
				radius_m: conditions.radius_m,
			},
		});
		// Whatever it starts as, a barometer read at the end of the first run
		// replaces it and says so. It is only the *opening* claim that has to
		// be honest: a car with no barometer never gets that replacement, and
		// saying "measured" here left it standing for the whole drive.
		setting.rho_from = match opts.air_density {
			Some(_) => carfile::Source::Stated,
			None => carfile::Source::StandardAtmosphere,
		};
		String::new()
	} else {
		match (&car, &vin) {
			(Some(car), _) => messages::car_file_summary(
				&car.vin,
				car.cda.as_ref().and_then(|c| c.at.as_deref()).unwrap_or("—"),
				car.mass_total_kg().unwrap_or(0.0),
				car.cda.as_ref().map_or(0.0, |c| c.value),
				car.cda.as_ref().is_some_and(|c| c.source == carfile::Source::Coastdown),
			),
			(None, Some(vin)) => messages::no_car_file(vin),
			(None, None) => messages::no_car_file("a car that did not give its VIN"),
		}
	};

	let plan = Plan::build(&set, opts.minimal);
	let channels = channel_descriptors(&plan, opts.full, opts.accel_window_s);
	Ok(Prepared {
		meta: Meta {
			vin,
			units: identities.to_vec(),
			marks: opts.marks.clone(),
			speed_source: set.leading.source(),
			speed_scale: opts.speed_scale,
			accel_window_s: opts.accel_window_s,
			setting,
			grade_percent: opts.grade_percent,
			headwind_ms: opts.headwind_ms,
			car_file: car_path,
			channels,
		},
		plan,
		banner,
		full: opts.full,
	})
}

/// What the units answered, so the reader of a refusal can see the check was
/// real.
fn found_report(identities: &[crate::plan::UnitIdentity], missing: &[channels::Missing]) -> Vec<messages::ChannelFound> {
	identities
		.iter()
		.flat_map(|unit| {
			missing.iter().map(move |m| messages::ChannelFound {
				unit: format!("{:03X}", unit.request),
				part_number: unit.part_number.clone().unwrap_or_else(|| "—".to_string()),
				key: m.key,
				ok: false,
			})
		})
		.collect()
}

/// What each resolved channel calls its own unit, keyed by the role it fills.
///
/// The same words the session file records, so the table and the file cannot
/// disagree about what the car was asked. Nothing here knows which quantity is
/// which: every channel hands over its catalog's spelling and the table prints
/// it.
fn channel_units(set: &channels::Set) -> BTreeMap<&'static str, String> {
	set.all().map(|channel| (channel.key, channel.def.unit.to_string())).collect()
}

/// One raw step of the pedal channel, which is what a kickdown threshold is
/// measured in.
fn pedal_step(set: &channels::Set) -> Option<f64> {
	set
		.all()
		.find(|channel| channel.key == "pedal")
		.and_then(|channel| match &channel.def.scaling {
			vag_data_labels::catalog::Scaling::Linear(scale) => Some(scale.factor.abs()),
			_ => None,
		})
}

/// Which car file applies, if any — the flag's, or this car's own by VIN.
///
/// A file whose VIN differs from the car's is **ignored with a message**, never
/// applied: mass and road load belong to one specific car, and quietly using
/// another one's is the false comparison this design spends its length avoiding.
fn load_car_file(vin: Option<&str>, opts: &Options<'_>) -> (Option<carfile::CarFile>, Option<String>) {
	let path = match opts.car {
		Some(path) => std::path::PathBuf::from(path),
		None => match vin.and_then(|vin| carfile::CarFile::path_for(vin).ok()) {
			Some(path) => path,
			None => return (None, None),
		},
	};
	if !path.exists() {
		return (None, None);
	}
	let car = match carfile::CarFile::load(&path) {
		Ok(car) => car,
		Err(why) => {
			eprintln!("{why:#}");
			return (None, None);
		}
	};
	if let Some(vin) = vin
		&& car.vin != vin
	{
		println!("{}", messages::wrong_car(&car.vin, vin));
		return (None, None);
	}
	let shown = path.display().to_string();
	(Some(car), Some(shown))
}

/// What is already known about the car, in the words the owner was asked in.
fn known_of(car: &carfile::CarFile) -> Vec<(&'static str, String)> {
	let mut out = Vec::new();
	if let Some(mass) = car.mass_total_kg() {
		out.push(("mass", format!("{mass:.0} kg")));
	}
	if let Some(tyre) = &car.tyre {
		out.push(("tyre", tyre.value.clone()));
	}
	out
}

/// The road load and the car, with the flags layered over the file.
///
/// Precedence is flag → car file, and there is no third tier: a parameter
/// neither source supplies is one the run does without. `--cda` and `--crr`
/// arrive as a pair — clap enforces that — and count as `stated` rather than
/// `coastdown`.
fn road_load(car: &carfile::CarFile, opts: &Options<'_>) -> Result<(power::RoadLoad, carfile::CarConditions), Vec<&'static str>> {
	let mut car = car.clone();
	if let Some(mass) = opts.mass_kg {
		car.mass = Some(carfile::Sourced::new(
			carfile::Mass {
				running_order_kg: mass,
				includes_driver: true,
				aboard_kg: 0.0,
			},
			carfile::Source::Stated,
		));
	}
	if let Some(tyre) = opts.tyre {
		car.rolling_radius_m = carfile::rolling_radius_m(tyre).map(|radius| carfile::Sourced::new(radius, carfile::Source::DerivedFromTyre));
	}
	if let (Some(cda), Some(crr)) = (opts.cda, opts.crr) {
		car.cda = Some(carfile::Sourced::new(cda, carfile::Source::Stated));
		car.crr = Some(carfile::Sourced::new(crr, carfile::Source::Stated));
	}
	let (load, conditions) = car.road_load()?;
	Ok((
		power::RoadLoad {
			cda: load.cda,
			crr: load.crr,
		},
		conditions,
	))
}

/// The poll loop.
///
/// Two batches a cycle — the leading one every cycle, the background one every
/// second — with the keyboard drained between them and never around one.
async fn drive<R: BatchReader>(mut reader: R, prepared: Prepared, opts: &Options<'_>, full_screen: bool) -> Result<()> {
	let Prepared {
		plan,
		mut meta,
		banner,
		full,
	} = prepared;
	let units = chart_units(&plan);
	let mut session = session::Session::new(opts.marks.clone(), RING_SECONDS, opts.speed_scale);
	let mut controls = ui::Controls::default();
	let mut recorded: Vec<Recorded> = Vec::new();
	// Events a keystroke caused, waiting for this cycle's one event pass.
	let mut pending: Vec<session::Event> = Vec::new();
	// Where the session was last written, so a discard can rewrite it. `--out`
	// fixes it up front; `s` and Enter set it to wherever `save` chose.
	let mut written_to: Option<String> = opts.out.map(str::to_string);
	// Runs thrown away that had not been written.
	//
	// `Session` counts the runs the stopwatch closed and has no notion of one
	// being taken back, so the difference is held here. `recorded` is what the
	// writer writes, so it is `recorded` that decides what is unsaved — and a
	// save clears this to nothing, because after one there is neither an
	// unsaved run nor a discard still owed against the count.
	let mut discarded_unsaved = 0usize;
	let mut charts: BTreeMap<String, Track> = BTreeMap::new();
	let mut values: BTreeMap<&'static str, String> = BTreeMap::new();
	let mut closed: BTreeMap<(u32, u32), Seconds> = BTreeMap::new();
	let mut last_outcome: Option<ui::Outcome> = None;
	let mut warning: Option<String> = None;
	let mut table: Option<String> = None;
	let mut silent = 0u32;
	let mut cycles = 0u64;
	let mut speed_kmh = 0.0f64;
	let mut clock = 0.0f64;
	let mut density: Option<(f64, bool)> = opts.air_density.map(|rho| (rho, false));

	// Held for the length of the drive, and given back by `Drop`: every `?`
	// below this line is a run that ended badly on somebody's dashboard, and it
	// must not also be a shell left with no echo and no cursor.
	let screen = match full_screen {
		true => Some(term::full_screen().enter().map_err(|e| {
			anyhow::anyhow!(
				"`measure` needs an interactive terminal (it draws a full-screen view). \
                 Without one it prints a line per cycle instead: {e}"
			)
		})?),
		false => None,
	};
	let mut terminal = match screen.is_some() {
		true => Some(Terminal::new(CrosstermBackend::new(io::stdout()))?),
		false => None,
	};

	let result: Result<()> = loop {
		// The results table waits for the car to stop: redrawing a dense table
		// at 100 km/h is exactly what the rest of this design avoids.
		let stationary = matches!(
			session.state(),
			session::State::Arming { .. } | session::State::Armed | session::State::Paused
		);
		let series = series_of(&charts, &units);
		let screen = ui::Screen {
			band: ui::band(
				&ui::phase_of(session.state(), speed_kmh, clock, last_outcome.as_ref()),
				session.degraded().then(|| session.hz().unwrap_or(0.0)),
			),
			banner: (!banner.is_empty()).then(|| banner.clone()),
			rows: value_rows(&values, &charts),
			marks: mark_rows(&opts.marks, &closed),
			series,
			hz: session.hz(),
			file: opts.out.map(str::to_string),
			warning: warning.clone(),
			table: table.clone().filter(|_| stationary),
		};
		match terminal.as_mut() {
			Some(terminal) => {
				terminal.draw(|frame| ui::draw(frame, &screen))?;
			}
			None => println!("{}", ui::plain_line(&screen)),
		}

		// Between batches, never around one: a read can sit out a unit's
		// two-second deadline and a cancel must not wait with it.
		let mut quit = false;
		let mut set = session::SampleSet::default();
		let mut answered_leading = false;
		let mut records: Records = Vec::new();

		for (index, batch) in due(&plan, cycles).into_iter().enumerate() {
			let unsaved = session.unsaved().saturating_sub(discarded_unsaved);
			if terminal.is_some() && drain(&mut controls, &mut session, unsaved, &mut warning, &mut pending, &mut quit)? {
				break;
			}
			let (at, outcome) = reader.read(batch).await;
			clock = at;
			let answers = match outcome {
				crate::plan::BatchOutcome::Answered(answers) => answers,
				crate::plan::BatchOutcome::NoAnswer | crate::plan::BatchOutcome::Unaddressable => Vec::new(),
			};
			if index == 0 {
				answered_leading = !answers.is_empty();
			}
			let batch_records: Records = answers.into_iter().map(|(did, data)| (batch.request, did, data)).collect();
			merge(&mut set, sample_set(&plan, &batch_records, at));
			records.extend(batch_records);
		}
		// A car that stops answering is not a car that is standing still.
		silent = match answered_leading {
			true => 0,
			false => silent + 1,
		};
		if silent == SILENT_CYCLES {
			warning = Some(messages::car_silent(session.runs().len()));
			pending.extend(session.on_command(session::Command::Cancel));
		}

		for (key, text) in rendered(&plan, &records) {
			values.insert(key, text);
		}
		if let Some((_, ms, _)) = set.speed {
			speed_kmh = ms * opts.speed_scale * power::KMH_PER_MS;
		}
		accumulate(
			&mut charts,
			&set,
			opts.speed_scale,
			clock,
			opts.accel_window_s,
			meta.setting.model.as_ref(),
		);

		// The keyboard's second drain of the cycle, here rather than after the
		// events, so that everything a key caused is recorded in the same pass
		// as everything the car caused. A key handled after this point would
		// have to wait a cycle for its events to be looked at, and a quit in
		// that window used to drop them entirely.
		if terminal.is_some() {
			let unsaved = session.unsaved().saturating_sub(discarded_unsaved);
			drain(&mut controls, &mut session, unsaved, &mut warning, &mut pending, &mut quit)?;
		}

		// **Every event is handled, whatever produced it.** The events a
		// command returns used to be thrown away here, which is how `Esc`
		// came to "do nothing": the run really was cancelled, but with no tone,
		// no band, no results table and — worse — no entry in `recorded`, so a
		// cancelled run was counted as unsaved and could never be written.
		pending.extend(session.on_sample(clock, set));
		for event in std::mem::take(&mut pending) {
			match event {
				session::Event::Started(_) => {
					closed.clear();
					charts.clear();
					table = None;
				}
				session::Event::MarkClosed(mark) => {
					closed.insert((mark.from_kmh, mark.to_kmh), mark.seconds);
					ui::play(ui::Tone::Mark, opts.quiet);
				}
				session::Event::Degraded { now_hz, was_hz } => {
					warning = Some(messages::degraded(now_hz, was_hz));
				}
				session::Event::Finished(run) | session::Event::Aborted(run) => {
					ui::play(if run.aborted { ui::Tone::Rejected } else { ui::Tone::Finished }, opts.quiet);
					// The barometer and the ambient sensor are read here and
					// nowhere else: once per run, and at the end of it, because
					// that sensor heat-soaks at a standstill and +10 K reads the
					// air density 3.4 % low.
					if full
						&& let Some(batch) = &plan.density
						&& let Some(measured) = read_density(&mut reader, &plan, batch).await
					{
						density = Some((measured, true));
					}
					if let Some((rho, measured)) = density
						&& let Some(model) = meta.setting.model.as_mut()
					{
						model.conditions.rho = rho;
						// The heading follows the number. A stated density is
						// already what it says it is; a read one has just
						// stopped being the standard atmosphere.
						meta.setting.rho_from = match measured {
							true => carfile::Source::Measured,
							false => carfile::Source::Stated,
						};
					}
					last_outcome = Some(match run.aborted {
						true => ui::Outcome::Aborted {
							at_kmh: speed_kmh,
							kept: run.marks.iter().map(|m| format!("{}-{}", m.from_kmh, m.to_kmh)).collect(),
						},
						false => ui::Outcome::Done {
							seconds: run.marks.last().map(|mark| mark.seconds),
						},
					});
					let derived = report::recompute(&run, &meta.setting);
					let text = report::results(&run, &derived, &meta.setting);
					recorded.push(Recorded {
						run: *run,
						derived,
						at: now(),
					});
					match terminal.is_some() {
						true => table = Some(text),
						false => println!("{text}"),
					}
					// `--out` writes continuously; `s` writes on demand. Both
					// write the same document.
					if let Some(path) = opts.out {
						write_session(path, &meta, &recorded, &session)?;
						session.on_command(session::Command::Save);
						discarded_unsaved = 0;
					}
				}
				session::Event::Armed => {}
			}
		}

		if controls.take_save() {
			match save(opts.out, &meta, &recorded, &session) {
				Ok(path) => {
					session.on_command(session::Command::Save);
					discarded_unsaved = 0;
					warning = Some(messages::saved(&path, recorded.len()));
					written_to = Some(path);
				}
				Err(why) => warning = Some(format!("{why:#}")),
			}
		}

		// `d` — throw the run whose results are on screen away. It leaves
		// `recorded`, which is what the writer writes, so it leaves the file
		// and the unsaved count with it.
		if controls.take_discard() {
			match table.is_some().then(|| recorded.pop()).flatten() {
				None => warning = Some(ui::nothing_to_discard()),
				Some(dropped) => {
					// Runs are recorded in order and saved in one go, so the
					// last one is unwritten exactly when anything is.
					let written = session.unsaved().saturating_sub(discarded_unsaved) == 0;
					let rewritten = match written {
						true => written_to.clone(),
						false => {
							discarded_unsaved += 1;
							None
						}
					};
					if let Some(path) = &rewritten {
						write_session(path, &meta, &recorded, &session)?;
					}
					warning = Some(ui::discarded(dropped.run.index, rewritten.as_deref(), recorded.len()));
					start_screen(&mut table, &mut closed, &mut charts);
				}
			}
		}

		// Enter — keep it and go again. The next launch has to begin on the
		// screen the first one began on, and until now the results table was
		// still up when the car moved off, which reads as a run nobody noticed.
		if controls.take_keep() {
			match table.is_some() {
				false => warning = Some(ui::nothing_to_keep()),
				true => match save(opts.out, &meta, &recorded, &session) {
					Ok(path) => {
						session.on_command(session::Command::Save);
						discarded_unsaved = 0;
						warning = Some(ui::kept(&path, recorded.len()));
						written_to = Some(path);
						start_screen(&mut table, &mut closed, &mut charts);
					}
					// The results stay up: a run whose save failed is a run the
					// driver still has to decide about.
					Err(why) => warning = Some(format!("{why:#}")),
				},
			}
		}
		if quit {
			break Ok(());
		}
		cycles += 1;
	};

	if terminal.is_some() {
		// Given back here rather than at the end of the function: the results
		// belong on the screen the command was started from, not on one that is
		// about to be thrown away. ratatui's own `Drop` shows the cursor it hid
		// while drawing, so it goes first and this guard leaves the screen after.
		drop(terminal);
		drop(screen);
		for record in &recorded {
			println!("{}", report::results(&record.run, &record.derived, &meta.setting));
		}
	}
	let unsaved = session.unsaved().saturating_sub(discarded_unsaved);
	if unsaved > 0 {
		println!("{}", messages::unsaved_on_quit(unsaved));
	}
	result
}

/// Which batches this cycle asks for.
///
/// The leading batch every cycle, the background one every second. Marks are
/// timed from the leading speed alone, so its rate is the only one that sets a
/// stopwatch, and it gets twice the rate of everything else. The barometer and
/// the ambient sensor are in neither: they are read once per run, at the end of
/// it, and never here.
fn due(plan: &Plan, cycle: u64) -> Vec<&crate::plan::Batch> {
	let mut out = vec![&plan.leading];
	if cycle % 2 == 0 {
		out.extend(plan.background.iter());
	}
	out
}

/// Write the session out, and say where it went.
///
/// `--out` names the file; otherwise it goes into this car's own directory
/// beside its car file, under the time it was written. Not the working
/// directory: a drive belongs to the car, not to wherever the shell happened to
/// be standing.
fn save(out: Option<&str>, meta: &Meta, recorded: &[Recorded], session: &session::Session) -> Result<String> {
	let path = match (out, &meta.vin) {
		(Some(path), _) => path.to_string(),
		(None, Some(vin)) => {
			let dir = crate::datadir::measures_dir(vin)?;
			std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
			dir
				.join(format!("{}.json", chrono::Local::now().format("%Y-%m-%d-%H%M%S")))
				.display()
				.to_string()
		}
		// A car that would not give its VIN has no directory of its own, so the
		// file lands where the command was run and says so.
		(None, None) => format!("{}.json", chrono::Local::now().format("%Y-%m-%d-%H%M%S")),
	};
	write_session(&path, meta, recorded, session)?;
	Ok(path)
}

fn write_session(path: &str, meta: &Meta, recorded: &[Recorded], session: &session::Session) -> Result<()> {
	let document = document(meta, recorded, session.degraded(), session.hz(), session.cycle_median_s());
	let mut text = serde_json::to_string(&document)?;
	text.push('\n');
	std::fs::write(path, text).with_context(|| format!("writing {path}"))
}

/// Read the barometer and the ambient sensor, once, and turn them into a
/// density.
async fn read_density<R: BatchReader>(reader: &mut R, plan: &Plan, batch: &crate::plan::Batch) -> Option<f64> {
	let (_, outcome) = reader.read(batch).await;
	let crate::plan::BatchOutcome::Answered(answers) = outcome else {
		return None;
	};
	let mut pressure_kpa = None;
	let mut ambient_c = None;
	for (did, data) in answers {
		let Some(channel) = plan.by_address.get(&(batch.request, did)) else {
			continue;
		};
		match channel.key {
			"barometer" => pressure_kpa = channel.value(&data),
			"ambient" => ambient_c = channel.value(&data),
			_ => {}
		}
	}
	Some(power::air_density(pressure_kpa?, ambient_c?))
}

/// Fold one batch's readings into the cycle's set.
fn merge(into: &mut session::SampleSet, from: session::SampleSet) {
	if from.speed.is_some() {
		into.speed = from.speed;
	}
	if from.engine_speed.is_some() {
		into.engine_speed = from.engine_speed;
	}
	if from.pedal.is_some() {
		into.pedal = from.pedal;
	}
	if from.gear.is_some() {
		into.gear = from.gear;
	}
	into.others.extend(from.others);
	into.states.extend(from.states);
}

/// Drain the keyboard. Returns true when the user asked to quit.
fn drain(
	controls: &mut ui::Controls,
	session: &mut session::Session,
	unsaved: usize,
	warning: &mut Option<String>,
	events: &mut Vec<session::Event>,
	quit: &mut bool,
) -> Result<bool> {
	// Zero, and it is not the reason a cancel ever went missing. A lone `Esc`
	// is the prefix of every ANSI sequence, so the suspicion was that
	// crossterm's parser holds it back waiting for the rest and never emits it
	// under a zero timeout. Measured through a pty against crossterm 0.28.1, it
	// does not: `parse_event` only waits when `input_available`, which
	// `source/unix/mio.rs` sets from `read_count == TTY_BUFFER_SIZE` (1024), so
	// a one-byte read of `0x1B` is delivered at once at 0 ms exactly as it is
	// at 5 ms. A timeout here would buy nothing and cost the loop its cycle.
	while event::poll(Duration::from_millis(0))? {
		if let TermEvent::Key(key) = event::read()?
			&& key.kind == KeyEventKind::Press
		{
			match ui::on_key(controls, key.code, unsaved) {
				ui::Action::Nothing => {}
				ui::Action::Session(command) => {
					// Handed back rather than swallowed: a cancel closes a run,
					// and the run it closed is in the events.
					let caused = session.on_command(command);
					// Cancel is the one command here that can legitimately do
					// nothing — it only acts on a run that is under way — and a
					// key that does nothing reads as a key that was not
					// received, which is how this was reported from the car.
					*warning = match caused.is_empty() && command == session::Command::Cancel {
						true => Some(ui::nothing_to_cancel()),
						false => None,
					};
					events.extend(caused);
				}
				ui::Action::Save => controls.ask_save(),
				ui::Action::Discard => controls.ask_discard(),
				ui::Action::KeepGoing => controls.ask_keep(),
				ui::Action::Refuse(text) => *warning = Some(text),
				ui::Action::Quit => {
					*quit = true;
					return Ok(true);
				}
			}
		}
	}
	Ok(false)
}

/// Put the screen back the way a run finds it: no results table, no marks from
/// the last run, an empty chart.
///
/// The stopwatch re-arms by itself, so this is only about what is drawn — but
/// what is drawn is the whole of what a driver has to go on, and a post-run
/// table still up as the car moves off reads as a run that was not noticed.
fn start_screen(table: &mut Option<String>, closed: &mut BTreeMap<(u32, u32), Seconds>, charts: &mut BTreeMap<String, Track>) {
	*table = None;
	closed.clear();
	charts.clear();
}

/// What each channel last read, as the catalog's own `describe` renders it.
///
/// Never `plan::Channel::render`: that falls back to `"… (raw)"`, which is
/// exactly the class of number this command excludes.
fn rendered(plan: &Plan, records: &Records) -> Vec<(&'static str, String)> {
	records
		.iter()
		.filter_map(|(request, did, data)| {
			let channel = plan.by_address.get(&(*request, *did))?;
			Some((channel.key, channel.def.describe(data)?))
		})
		.collect()
}

/// Keep the chart buffers, trimmed to the tail worth drawing.
///
/// The live acceleration is **causal** and nothing else can be: the future half
/// of a centred window has not been read yet. The file and the results table use
/// the central scheme, recomputed over the finished run, which is why nothing
/// accumulated here is ever saved.
fn accumulate(
	charts: &mut BTreeMap<String, Track>,
	set: &session::SampleSet,
	speed_scale: f64,
	now: Seconds,
	window: Seconds,
	model: Option<&report::Model>,
) {
	if let Some((t, ms, _)) = set.speed {
		let speed = charts.entry(SPEED_CHART.to_string()).or_default();
		speed.push(t, ms * speed_scale * power::KMH_PER_MS);
		let last = speed.len() - 1;
		let slope = derive::slope(speed, last, window, derive::Scheme::Causal);
		if let Some(slope) = slope {
			// The chart carries km/h, so its slope is km/h per second, and the
			// buffer is in m/s² like everything else derived here.
			let accel = slope.a / power::KMH_PER_MS;
			charts.entry(ACCEL_CHART.to_string()).or_default().push(slope.t, accel);
			// Power needs a car that was measured rather than assumed, so
			// without a finished car file the series is absent — not zero, and
			// not computed against a generic drag figure. Kept beside the other
			// two so that the table and the chart quote one number, not two.
			if let Some(model) = model {
				let watts = power::power(ms * speed_scale, accel, None, &model.load, &model.conditions);
				charts.entry(POWER_CHART.to_string()).or_default().push(slope.t, watts.wheel_w / 1000.0);
			}
		}
	}
	if let Some((t, rpm)) = set.engine_speed {
		charts.entry("engine speed".to_string()).or_default().push(t, rpm);
	}
	if let Some((t, pedal)) = set.pedal {
		charts.entry("pedal".to_string()).or_default().push(t, pedal);
	}
	for (key, t, value) in &set.others {
		charts.entry((*key).to_string()).or_default().push(*t, *value);
	}
	for track in charts.values_mut() {
		let drop = track.t.partition_point(|probe| *probe < now - CHART_SECONDS);
		track.t.drain(..drop);
		track.v.drain(..drop);
	}
}

/// What each chart buffer is measured in, which is what decides the scales the
/// chart puts its lines on.
///
/// The catalog's own word for every channel that resolved — never a table of
/// this car's units — and the unit the arithmetic produces for the three keys
/// this module computes rather than reads.
fn chart_units(plan: &Plan) -> BTreeMap<String, String> {
	let mut out: BTreeMap<String, String> = BTreeMap::new();
	for channel in plan.by_address.values() {
		out.entry(channel.key.to_string()).or_insert_with(|| channel.def.unit.trim().to_string());
	}
	// Speed is converted on the way into the buffer, so the buffer is in km/h
	// whatever unit the catalog wrote the channel down in (ISO 80000-3); the
	// other two are computed here and are in SI by §3.
	out.insert(SPEED_CHART.into(), "km/h".into());
	out.insert(ACCEL_CHART.into(), "m/s²".into());
	out.insert(POWER_CHART.into(), "kW".into());
	out
}

/// The chart buffers this module fills in itself rather than reading.
const SPEED_CHART: &str = "speed";
const ACCEL_CHART: &str = "accel";
const POWER_CHART: &str = "power";

/// The series on offer, in a stable order so the arrow keys mean the same thing
/// from one cycle to the next.
///
/// The order is the one the driver asked for them in — speed, engine speed,
/// power, acceleration — and it is what [`chart::pages`] then cuts into pages: the
/// first two share a page, and the two computed ones share the next.
fn series_of(charts: &BTreeMap<String, Track>, units: &BTreeMap<String, String>) -> Vec<chart::Series> {
	let origin = |name: &str| match name {
		// Live, the slope can only be causal: the future half of a centred
		// window has not been read yet.
		ACCEL_CHART => chart::Origin::Computed("trailing"),
		POWER_CHART => chart::Origin::Computed("estimate"),
		_ => chart::Origin::Bus,
	};
	let series = |name: &str, track: &Track| chart::Series {
		label: name.to_string(),
		unit: units.get(name).cloned().unwrap_or_default(),
		// A `Track` is a numerics buffer and a chart wants a list of points, so
		// this is where one becomes the other. It was already a copy per cycle.
		points: track.t.iter().copied().zip(track.v.iter().copied()).collect(),
		origin: origin(name),
	};
	let mut out: Vec<chart::Series> = Vec::new();
	for name in [SPEED_CHART, "engine speed", POWER_CHART, ACCEL_CHART, "pedal"] {
		if let Some(track) = charts.get(name) {
			out.push(series(name, track));
		}
	}
	for (name, track) in charts {
		if !out.iter().any(|s| &s.label == name) {
			out.push(series(name, track));
		}
	}
	out
}

/// The value table: every channel that answered, and every figure derived from
/// one, each saying which it is.
fn value_rows(values: &BTreeMap<&'static str, String>, charts: &BTreeMap<String, Track>) -> Vec<ui::ValueRow> {
	let mut rows: Vec<ui::ValueRow> = Vec::new();
	// The order a driver reads them in, not the order they were resolved.
	for (name, role) in [
		("speed", "speed"),
		("engine", "engine speed"),
		("gear", "gear"),
		("pedal", "pedal"),
		("selector", "selector"),
	] {
		if let Some(value) = values.get(role) {
			rows.push(ui::ValueRow {
				name: name.to_string(),
				value: value.clone(),
				origin: chart::Origin::Bus,
			});
		}
	}
	// Actual before specified, the order `watch` already uses, and on one line
	// because the gap between them is the whole diagnostic.
	if let Some(actual) = values.get("boost actual") {
		let pair = match values.get("boost specified") {
			Some(specified) => format!("{actual} / {specified} (act/spec)"),
			None => actual.clone(),
		};
		rows.push(ui::ValueRow {
			name: "boost".into(),
			value: pair,
			origin: chart::Origin::Bus,
		});
	}
	if let Some(value) = values.get("air mass") {
		rows.push(ui::ValueRow {
			name: "air mass".into(),
			value: value.clone(),
			origin: chart::Origin::Bus,
		});
	}

	if let Some(accel) = charts.get(ACCEL_CHART).and_then(|track| track.v.last()) {
		rows.push(ui::ValueRow {
			name: "accel".into(),
			value: format!("{:.2} g", accel / power::G),
			origin: chart::Origin::Computed("trailing"),
		});
	}
	// Live power needs an air density, and under `--full` the car's own is only
	// read at the end of a run. Until one exists the row is absent rather than
	// computed against a guess. It is read out of the same buffer the chart
	// draws, so the row and the line can never quote different numbers.
	if let Some(kw) = charts.get(POWER_CHART).and_then(|track| track.v.last()) {
		rows.push(ui::ValueRow {
			name: "power".into(),
			value: report::power_figure(*kw),
			origin: chart::Origin::Computed("estimate"),
		});
	}
	rows
}

/// The marks panel: every mark asked for, with the ones that closed filled in.
///
/// A mark that has not closed shows a placeholder rather than a blank, because a
/// gap reads as a mark that was never asked for.
fn mark_rows(wanted: &[(u32, u32)], closed: &BTreeMap<(u32, u32), Seconds>) -> Vec<ui::MarkRow> {
	wanted
		.iter()
		.map(|(from, to)| ui::MarkRow {
			name: format!("{from}-{to}"),
			seconds: closed.get(&(*from, *to)).copied(),
			from_launch: *from == 0,
		})
		.collect()
}

/// Run whatever the command line asked for.
///
/// The one entry point both binaries use, so `vagcan measure` and
/// `vagcan-measure` cannot diverge in behaviour any more than they can in
/// flags. `catalogs` is passed in rather than resolved here: where a project's
/// measurements live is `core`'s question, and answering it twice is how two
/// binaries end up reading different directories.
pub async fn dispatch(a: args::Args, catalogs: &str) -> anyhow::Result<()> {
	match a.tool {
		Some(Tool::View { file }) => match file {
			Some(file) => open_view(&file),
			None => view_picked(),
		},
		Some(Tool::Setup {
			device,
			coast_from,
			coast_to,
			data: _,
			car,
		}) => {
			setup::run(setup::Options {
				device: device.as_deref(),
				catalogs,
				coast_from_kmh: coast_from,
				coast_to_kmh: coast_to,
				car: car.as_deref(),
			})
			.await
		}
		None => {
			run(Options {
				device: a.device.as_deref(),
				car: a.car.as_deref(),
				catalogs,
				full: a.full,
				minimal: a.minimal,
				marks: a.marks.0,
				accel_window_s: a.accel_window,
				out: a.out.as_deref(),
				quiet: a.quiet,
				mass_kg: a.mass,
				tyre: a.tyre.as_deref(),
				cda: a.cda,
				crr: a.crr,
				inertia_factor: a.inertia_factor,
				grade_percent: a.grade,
				headwind_ms: a.headwind,
				air_density: a.air_density,
				speed_scale: a.speed_scale,
			})
			.await
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_marks_parser_rejects_a_pair_that_does_not_rise() {
		assert_eq!(parse_marks("0-100,50-100").unwrap(), Marks(vec![(0, 100), (50, 100)]));
		assert!(parse_marks("100-50").is_err(), "a mark runs upwards");
		assert!(parse_marks("abc").is_err());
		assert!(parse_marks("").is_err(), "an empty list is not a default");
		assert!(parse_marks("0-0").is_err());
		// The documented default is what the flag actually parses.
		assert_eq!(parse_marks(DEFAULT_MARKS).unwrap().0.len(), 6);
	}

	#[test]
	fn the_speed_scale_parser_refuses_anything_a_stopwatch_cannot_use() {
		assert_eq!(parse_speed_scale("0.97").unwrap(), 0.97);
		for bad in ["0", "-1", "nan", "inf", "x"] {
			assert!(parse_speed_scale(bad).is_err(), "{bad} was accepted");
		}
	}

	#[test]
	fn a_speed_is_converted_by_the_unit_the_catalog_gave_it() {
		// A conversion applied twice, or not at all, is the kind of error no
		// other test in this crate could see.
		assert!((speed_to_ms("km/h", 36.0).unwrap() - 10.0).abs() < 1e-12);
		assert_eq!(speed_to_ms("m/s", 10.0), Some(10.0));
		assert!((speed_to_ms("mph", 100.0).unwrap() - 44.704).abs() < 1e-9);
		// An unknown unit makes the channel absent rather than off by 3.6.
		assert_eq!(speed_to_ms("furlong/fortnight", 1.0), None);
	}

	#[test]
	fn a_channel_key_is_snake_case_in_the_file() {
		// One spelling in the document, so the reader never has to normalise.
		assert_eq!(file_key("engine speed"), "engine_speed");
		assert_eq!(file_key("boost actual"), "boost_actual");
		assert_eq!(file_key("cross-check speed"), "cross_check_speed");
	}

	/// A three-unit fixture with the roles these scheduling tests reason about
	/// — engine, gearbox and cluster — written to a temp directory as synthetic
	/// catalogs.
	///
	/// **Synthetic, not the reference car.** The proven rows a real vehicle
	/// yields are no longer in the repository — they live under
	/// this project's `measurements/` once a drive establishes them, and asserting their
	/// exact values is a job for the machine that measured them. What these tests
	/// need is not those numbers but a plan with the right *shape*: a required
	/// speed and gear, a pedal, a boost pair, and — on an emissions unit — the
	/// barometer and ambient sensor the density batch is built from. Round
	/// numbers serve that, and keep the poll scheduler and the read-only
	/// allowlist under test without shipping a car's data to do it.
	///
	/// The requests are kept at `0x7E0`/`0x7E1`/`0x714` because emissions
	/// addressing (`0x7E0..=0x7E7`) is what makes the OBD-II PIDs — speed,
	/// barometer, ambient, air mass — appear on the powertrain units at all.
	fn reference() -> (vag_data_labels::catalog::CatalogStore, Vec<crate::plan::UnitIdentity>) {
		use std::borrow::Cow;
		use vag_data_labels::catalog::{MeasurementCatalog, MeasurementDef, ReadId, Scaling};
		use vag_data_labels::measure::{LinearScale, RawForm};

		// Held for the life of the test process: the store reads the files
		// lazily, so the directory has to outlive every `reference()` call.
		static DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
		let dir = DIR.get_or_init(|| {
			let dir = tempfile::tempdir().expect("a temp dir for the fixture catalogs");
			let linear = |name: &str, unit: &str, did: u16, form: RawForm, factor: f64| MeasurementDef {
				name: Cow::Owned(name.to_string()),
				unit: Cow::Owned(unit.to_string()),
				address: ReadId::Uds(did),
				raw_form: form,
				scaling: Scaling::Linear(LinearScale { factor, offset: 0.0 }),
			};
			let enumeration = |name: &str, did: u16, levels: Vec<(i32, &str)>| MeasurementDef {
				name: Cow::Owned(name.to_string()),
				unit: Cow::Borrowed(""),
				address: ReadId::Uds(did),
				raw_form: RawForm::U8First,
				scaling: Scaling::Enum {
					levels: levels.into_iter().map(|(c, s)| (c, s.to_string())).collect(),
				},
			};
			let write = |part: &str, defs: Vec<MeasurementDef>| {
				let json = MeasurementCatalog::new(defs).to_json().unwrap();
				std::fs::write(dir.path().join(format!("{part}.json")), json).unwrap();
			};

			// Engine: the two roles OBD-II does not carry — engine speed and the
			// boost pair. Speed, barometer, ambient and air mass arrive from the
			// OBD PIDs because the unit is emissions-addressed.
			write(
				"ENG00001",
				vec![
					linear("Engine speed", "/min", 0x8302, RawForm::U16Be, 1.0),
					linear("Boost pressure, specified", "bar", 0x8233, RawForm::U16Be, 0.001),
					linear("Boost pressure, actual", "bar", 0x8234, RawForm::U16Be, 0.001),
				],
			);
			// Gearbox: the shafts, the pedal, the selector, the gear, and its own
			// finer vehicle-speed row that outranks the OBD byte.
			write(
				"GBX00002",
				vec![
					linear("Input shaft speed", "/min", 0x380A, RawForm::U16Le, 1.0),
					linear("Output shaft speed", "/min", 0x380B, RawForm::U16Le, 1.0),
					linear("Accelerator pedal position", "%", 0x3804, RawForm::U8First, 0.4),
					linear("Vehicle speed", "km/h", 0xF40D, RawForm::U16Le, 0.01),
					enumeration("Selector lever", 0x3809, vec![(0x05, "D"), (0x06, "R")]),
					enumeration(
						"Selected gear",
						0x3816,
						vec![(0x00, "not engaged"), (0x02, "1"), (0x03, "2"), (0x0C, "R")],
					),
				],
			);
			// Cluster: a second speed, so a cross-check channel exists.
			write("CLU00003", vec![linear("Road speed", "km/h", 0x22D2, RawForm::U16Be, 1.0)]);
			dir
		});

		let store = vag_data_labels::catalog::CatalogStore::open(dir.path());
		let ident = |request, part: &str| crate::plan::UnitIdentity {
			request,
			part_number: Some(part.to_string()),
			odx_name: None,
			odx_version: None,
			component: None,
		};
		(store, vec![ident(0x7E0, "ENG00001"), ident(0x7E1, "GBX00002"), ident(0x714, "CLU00003")])
	}

	fn plan_for(full: bool, minimal: bool) -> Plan {
		let (store, units) = reference();
		let set = channels::resolve(&store, &crate::extracted::Extracted::none(), &units, full).expect("the reference car resolves");
		Plan::build(&set, minimal)
	}

	/// The keys a plan would poll, per batch, so a test can talk about roles
	/// rather than about identifiers.
	fn keys(plan: &Plan, batch: &crate::plan::Batch) -> Vec<&'static str> {
		batch
			.dids
			.iter()
			.filter_map(|did| plan.by_address.get(&(batch.request, *did)).map(|c| c.key))
			.collect()
	}

	#[test]
	fn the_channels_that_only_feed_the_power_model_are_not_polled_without_full() {
		// A cycle spent on a number nobody will look at is a cycle not spent on
		// speed, and a default-mode recording can never become a power figure
		// afterwards because the density its model needs was never sampled.
		let plan = plan_for(false, false);
		assert!(plan.density.is_none(), "nothing to read them with");
		let polled: Vec<&str> = std::iter::once(&plan.leading)
			.chain(plan.background.iter())
			.flat_map(|batch| keys(&plan, batch))
			.collect();
		assert!(!polled.contains(&"barometer"), "{polled:?}");
		assert!(!polled.contains(&"ambient"), "{polled:?}");
		// And everything worth having on its own is still there.
		for wanted in ["speed", "engine speed", "gear", "pedal"] {
			assert!(polled.contains(&wanted), "{wanted} missing from {polled:?}");
		}
	}

	#[test]
	fn under_full_the_barometer_is_a_batch_of_its_own_and_not_in_the_cycle() {
		// Once per run, not per cycle: neither reading moves measurably in seven
		// seconds, and polling them at 20 Hz would cost cycles for no
		// information.
		let plan = plan_for(true, false);
		let density = plan.density.as_ref().expect("--full reads them");
		let mut found = keys(&plan, density);
		found.sort_unstable();
		assert_eq!(found, ["ambient", "barometer"]);
		let per_cycle: Vec<&str> = std::iter::once(&plan.leading)
			.chain(plan.background.iter())
			.flat_map(|batch| keys(&plan, batch))
			.collect();
		assert!(!per_cycle.contains(&"barometer"), "{per_cycle:?}");
		assert!(!per_cycle.contains(&"ambient"), "{per_cycle:?}");
	}

	#[test]
	fn minimal_polls_only_what_the_stopwatch_needs() {
		let plan = plan_for(false, true);
		let polled: Vec<&str> = std::iter::once(&plan.leading)
			.chain(plan.background.iter())
			.flat_map(|batch| keys(&plan, batch))
			.collect();
		assert!(polled.contains(&"speed"), "{polled:?}");
		assert!(polled.contains(&"gear"), "{polled:?}");
		assert!(!polled.contains(&"pedal"), "the telemetry is the trade: {polled:?}");
		assert!(!polled.contains(&"boost actual"), "{polled:?}");
	}

	#[test]
	fn the_leading_batch_runs_every_cycle_and_the_background_every_second() {
		// Marks are timed from the leading speed alone, so its rate is the only
		// one that sets a stopwatch.
		let plan = plan_for(false, false);
		assert!(!plan.background.is_empty(), "the reference car spans units");
		for cycle in 0..6u64 {
			let batches = due(&plan, cycle);
			assert_eq!(batches[0].request, plan.leading.request, "cycle {cycle}");
			let expected = match cycle % 2 {
				0 => 1 + plan.background.len(),
				_ => 1,
			};
			assert_eq!(batches.len(), expected, "cycle {cycle}");
		}
	}

	/// A reader that answers from a table and counts what it was asked for.
	/// The seam the loop's scheduling is tested behind — no CAN, no adapter.
	struct Fake {
		asked: Vec<crate::plan::Batch>,
		answers: BTreeMap<(u16, u16), Vec<u8>>,
	}

	impl BatchReader for Fake {
		async fn read(&mut self, batch: &crate::plan::Batch) -> (Seconds, crate::plan::BatchOutcome) {
			self.asked.push(batch.clone());
			let records: Vec<(u16, Vec<u8>)> = batch
				.dids
				.iter()
				.filter_map(|did| self.answers.get(&(batch.request, *did)).map(|data| (*did, data.clone())))
				.collect();
			(self.asked.len() as f64 * 0.05, crate::plan::BatchOutcome::Answered(records))
		}
	}

	#[tokio::test]
	async fn the_density_batch_is_read_once_and_lands_on_the_iso_2533_anchor() {
		let plan = plan_for(true, false);
		let density = plan.density.clone().expect("--full reads them");
		// 101 kPa and 15 °C, as SAE J1979 spells them: 1 kPa/bit and A − 40 °C.
		let answers = plan
			.by_address
			.iter()
			.filter_map(|((request, did), channel)| match channel.key {
				"barometer" => Some(((*request, *did), vec![101u8])),
				"ambient" => Some(((*request, *did), vec![55u8])),
				_ => None,
			})
			.collect();
		let mut reader = Fake { asked: Vec::new(), answers };
		let rho = read_density(&mut reader, &plan, &density).await.expect("both answered");
		assert_eq!(reader.asked.len(), 1, "once per run, not per channel");
		assert!((rho - 1.2211).abs() < 1e-3, "{rho}");
	}

	/// A car that answers reads and remembers every byte it was sent.
	///
	/// Below the [`BatchReader`] seam on purpose. `Fake` proves what the loop
	/// *asks* for; this proves what actually reaches the wire, which is the
	/// only level at which "no service outside the allowlist" can be checked.
	struct FakeCar {
		/// The service byte of every request, however it was framed.
		services: Vec<u8>,
		/// Set by a first frame, cleared once the flow control is handed back.
		owes_flow_control: bool,
		/// Where the last request went, so the answer comes back on the id
		/// that unit actually answers on. Replying on one fixed id makes the
		/// client discard every frame and spin until its timeout.
		addressed: Option<u16>,
	}

	impl vag_uds_can::CanBackend for FakeCar {
		async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), vag_uds_can::CanError> {
			self.addressed = u16::try_from(id).ok();
			// ISO 15765-2: the high nibble of byte 0 is the frame type. The
			// service byte follows the length in a single frame (type 0) and
			// the two-byte length in a first frame (type 1). A consecutive
			// frame carries no service and must not be counted as one.
			match data.first().map(|pci| pci >> 4) {
				Some(0) => self.services.extend(data.get(1)),
				Some(1) => {
					self.services.extend(data.get(2));
					self.owes_flow_control = true;
				}
				_ => {}
			}
			Ok(())
		}

		async fn recv_frame(&mut self, _timeout: std::time::Duration) -> Result<(u32, Vec<u8>), vag_uds_can::CanError> {
			// A batch of eight identifiers does not fit one frame, so the car
			// has to clear the sender to continue before it can answer at all.
			let from = self
				.addressed
				.and_then(vag_uds_client::address::UnitAddress::from_request)
				.map(|unit| u32::from(unit.response))
				.unwrap_or(0x7E8);
			if std::mem::take(&mut self.owes_flow_control) {
				return Ok((from, vec![0x30, 0x00, 0x00]));
			}
			// Then refuse. What was asked is the whole point here, and a car is
			// free to say no: `7F 22 31` is request-out-of-range, the ordinary
			// answer to an identifier a unit does not carry.
			Ok((from, vec![0x03, 0x7F, 0x22, 0x31]))
		}
	}

	#[tokio::test]
	async fn nothing_measure_can_ask_for_leaves_the_read_allowlist() {
		// A read-only tool can still provoke a control unit into misbehaving:
		// read-only bounds what can be changed about a car, not what can be
		// provoked. `0x10` — DiagnosticSessionControl — is what a sweep opens
		// with, and it is the service that must never appear on a bus this
		// command owns.
		//
		// Both plans, because `--full` adds units and a whole extra batch.
		for full in [false, true] {
			let plan = plan_for(full, false);
			let mut car = Some(FakeCar {
				services: Vec::new(),
				owes_flow_control: false,
				addressed: None,
			});
			let started = std::time::Instant::now();

			let batches: Vec<_> = std::iter::once(plan.leading.clone())
				.chain(plan.background.iter().cloned())
				.chain(plan.density.iter().cloned())
				.collect();
			assert!(!batches.is_empty());
			for batch in &batches {
				let _ = crate::plan::read_batch(&mut car, batch, started).await;
			}

			let services = car.expect("handed back").services;
			assert!(!services.is_empty(), "the test proves nothing if nothing was sent");
			for service in &services {
				assert_eq!(
					*service, 0x22,
					"measure sent service 0x{service:02X} (full = {full}); the allowlist is \
                     0x22, 0x19, 0x10, 0x3E and this command needs only the first"
				);
			}
		}
	}

	/// A session with one launch mark, one rolling mark, and a car described
	/// well enough that the computed block exists.
	fn recorded_session() -> (Meta, Vec<Recorded>) {
		use session::{Mark, Run, Samples, Span};

		let mut samples = Samples::default();
		let mut t = -1.0;
		while t <= 8.0 {
			let v: f64 = if t <= 0.0 { 0.0 } else { 4.0 * t };
			samples.speed.push(t, v);
			samples.engine_speed.push(t, 1000.0 + 200.0 * v.min(20.0));
			samples.pedal.push(t, if t < 0.0 { 0.0 } else { 102.0 });
			samples.gear.push(t, if t < 3.0 { "1" } else { "2" });
			t += 0.05;
		}
		let run = Run {
			index: 1,
			samples,
			launch: None,
			marks: vec![
				Mark {
					from_kmh: 0,
					to_kmh: 100,
					closed_at: 6.944,
					seconds: 6.944,
					bracket: Some(Span {
						earliest: 6.85,
						latest: 7.04,
					}),
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
		};
		let setting = report::Setting::default();
		let derived = report::recompute(&run, &setting);
		let meta = Meta {
			vin: Some("TMBJJ7NE1J0000000".into()),
			units: Vec::new(),
			marks: vec![(0, 100), (50, 100)],
			speed_source: "7E1:F40D".into(),
			speed_scale: 1.0,
			accel_window_s: 0.3,
			setting,
			grade_percent: 0.0,
			headwind_ms: 0.0,
			car_file: None,
			channels: Vec::new(),
		};
		let recorded = vec![Recorded {
			run,
			derived,
			at: "2026-08-04T01:00:00+03:00".into(),
		}];
		(meta, recorded)
	}

	#[test]
	fn the_page_reads_the_field_names_the_writer_writes() {
		// This is the defect that already happened: the page was written
		// against `bias_s` — the one-signed launch model — while the writer
		// emitted `bracket_s`, so `measure view` silently dropped the interval
		// and printed a bare midpoint. Both sides were valid on their own, and
		// nothing failed; only opening the page in a browser showed it.
		//
		// So the contract is asserted directly: every field the page reads off
		// a mark or off the config must be one the writer emits.
		let page = include_str!("view.html");
		let (meta, recorded) = recorded_session();
		let document = document(&meta, &recorded, false, Some(21.3), Some(0.047));

		// Fields legitimately absent from *this* session, each for a reason
		// that is about the run and not about the contract.
		let absent: &[&str] = &[
			// Only a car file supplies the road load, and none is set here.
			"cda",
			"cda_source",
			"crr",
			"crr_source",
			"mass_kg",
			"tyre",
			// Same reason, one step removed: the air density is what the power
			// model divides by, so it is written exactly when there is a model
			// to use it. `--air-density` is refused without `--full` at parse
			// time, so a density with no model is not a state this tool has.
			"air_density_kg_m3",
			"air_density_source",
			// The bracket and the ± are exclusive by construction: a mark has
			// one or the other, never both, and the page reads both names.
			"sigma_s",
		];

		let config = document["config"].as_object().expect("a config object");
		for name in accessors(page, "CFG") {
			assert!(
				config.contains_key(&name) || absent.contains(&name.as_str()),
				"the page reads CFG.{name} and the writer never writes it"
			);
		}

		let marks = document["runs"][0]["marks"].as_array().expect("marks");
		let written: std::collections::BTreeSet<String> = marks
			.iter()
			.filter_map(|mark| mark.as_object())
			.flat_map(|mark| mark.keys().cloned())
			.collect();
		for name in accessors(page, "m") {
			assert!(
				written.contains(&name) || absent.contains(&name.as_str()),
				"the page reads m.{name} off a mark and the writer never writes it"
			);
		}
		// The test is worthless if it found nothing to check.
		assert!(written.contains("bracket_s"), "{written:?}");
		assert!(config.contains_key("speed_source"), "{config:?}");
	}

	/// Every `<object>.<field>` the page reads, for one object name.
	///
	/// Deliberately not applied to the page's `d` (the derived block): `d` is
	/// also the parameter name of an unrelated callback over dropped series, so
	/// scanning for it would assert against fields that are not the writer's to
	/// provide. `CFG` and `m` are used for one thing each.
	fn accessors(page: &str, object: &str) -> std::collections::BTreeSet<String> {
		let mut out = std::collections::BTreeSet::new();
		let needle = format!("{object}.");
		for (at, _) in page.match_indices(&needle) {
			// `.foo` inside `bar.CFG.foo` is not an access on `CFG`.
			let before = page[..at].chars().next_back();
			if before.is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '.') {
				continue;
			}
			let rest = &page[at + needle.len()..];
			let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(rest.len());
			if end > 0 {
				out.insert(rest[..end].to_string());
			}
		}
		out
	}

	#[test]
	fn a_session_refuses_a_schema_it_does_not_know_by_name() {
		let dir = std::env::temp_dir().join(format!("vagcan-measure-{}", std::process::id()));
		std::fs::create_dir_all(&dir).unwrap();
		let path = dir.join("future.json");
		std::fs::write(&path, r#"{"schema": 99, "runs": []}"#).unwrap();
		let why = open_view(path.to_str().unwrap()).unwrap_err().to_string();
		assert!(why.contains("schema 99"), "{why}");

		std::fs::write(&path, r#"{"runs": []}"#).unwrap();
		assert!(open_view(path.to_str().unwrap()).unwrap_err().to_string().contains("no `schema`"));
		std::fs::remove_dir_all(&dir).ok();
	}
}
