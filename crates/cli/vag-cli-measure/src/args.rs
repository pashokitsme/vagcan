//! The stopwatch's own command line.
//!
//! Declared here rather than in `vag-cli` so that `vagcan measure` and the
//! standalone `vagcan-measure` cannot drift apart: one is `#[command(flatten)]`
//! over this struct and the other is `Parser` over it, and neither has a copy
//! of the flags to keep in step.

use clap::{Args as ClapArgs, Parser};

/// Everything `measure` takes, whichever binary is asking.
// Clone for the reason the command enums are: `vagcan`'s dispatcher keeps a
// copy of the command so it can be run again after the label data has been made.
#[derive(Clone, ClapArgs)]
pub struct Args {
	/// `setup` describes this car once; `view` opens a saved session.
	#[command(subcommand)]
	pub tool: Option<crate::Tool>,
	/// Adapter to use. Omit it when only one is connected.
	#[arg(long, value_name = "PATH")]
	pub device: Option<String>,
	/// Use this car file instead of the one kept for this car's VIN.
	#[arg(long, value_name = "FILE")]
	pub car: Option<String>,
	/// Compute power as well. Needs a car file completed by
	/// `vagcan measure setup`, and is refused without one rather than
	/// falling back to generic road-load numbers.
	#[arg(long)]
	pub full: bool,
	/// Poll only what the stopwatch needs — speed and gear — for the
	/// highest achievable rate, at the cost of the telemetry.
	#[arg(long, conflicts_with = "full")]
	pub minimal: bool,
	/// Marks to time, as `A-B` pairs in km/h, `A < B`. `0-60` here is the
	/// metric one; the American figure is in mph.
	#[arg(long, default_value = crate::DEFAULT_MARKS, value_name = "LIST",
              value_parser = crate::parse_marks)]
	pub marks: crate::Marks,
	/// Half-width of the least-squares acceleration window, in seconds.
	#[arg(long, default_value_t = crate::report::ACCEL_WINDOW_S, value_name = "SECONDS",
              value_parser = crate::parse_seconds)]
	pub accel_window: f64,
	/// Write the session here continuously. Without it, `s` saves on demand
	/// into this car's own directory.
	#[arg(long, value_name = "FILE")]
	pub out: Option<String>,
	/// No tone on a closed mark.
	#[arg(long)]
	pub quiet: bool,
	/// Where the proven measurement rows live.
	/// Default: this project's `~/.vagcan/data/<project>/measurements`.
	#[arg(long, value_name = "DIR")]
	pub data: Option<String>,
	/// Mass in kilograms, overriding the car file for this run.
	#[arg(long, value_name = "KG")]
	pub mass: Option<f64>,
	/// Tyre size as written on the sidewall, e.g. `205/55R16`.
	#[arg(long, value_name = "SIZE")]
	pub tyre: Option<String>,
	/// Drag area in m². The coastdown measures this; pass it only if you
	/// genuinely have the figure, and pass `--crr` with it.
	#[arg(long, value_name = "M2", requires = "crr")]
	pub cda: Option<f64>,
	/// Rolling resistance coefficient. The fit produces it and `--cda` as a
	/// pair, so neither is accepted alone.
	#[arg(long, value_name = "N", requires = "cda")]
	pub crr: Option<f64>,
	/// Scale the stored wheel and engine inertias, for a car whose
	/// rotating mass is known to differ from the typical figures.
	#[arg(long, value_name = "N")]
	pub inertia_factor: Option<f64>,
	/// Road gradient in per cent. Downhill flatters every figure.
	#[arg(long, default_value_t = 0.0, value_name = "PERCENT")]
	pub grade: f64,
	/// Headwind in m/s. Drag acts on air speed, not on ground speed.
	#[arg(long, default_value_t = 0.0, value_name = "M_S")]
	pub headwind: f64,
	/// Air density in kg/m³, for a car whose barometer or ambient sensor
	/// this tool cannot read. It feeds power and nothing else.
	#[arg(long, value_name = "KG_M3", requires = "full")]
	pub air_density: Option<f64>,
	/// Multiply every speed reading before mark detection, so that `0-100`
	/// means a corrected 100 rather than an indicated one. One GPS
	/// comparison run is what settles the value.
	#[arg(long, default_value_t = 1.0, value_name = "N",
              value_parser = crate::parse_speed_scale)]
	pub speed_scale: f64,
}

/// `vagcan-measure` as its own program.
#[derive(Parser)]
#[command(name = "vagcan-measure", version, about = "Time a car's acceleration, from its own speed signal.")]
pub struct Cli {
	/// Which car's data to read — a directory name under `~/.vagcan/data/`.
	///
	/// The diagnostics binary carries this as a global flag, so the two
	/// spellings would otherwise read different directories from the same
	/// command line and say nothing about it.
	#[arg(long, value_name = "ID", global = true)]
	pub project: Option<String>,

	#[command(flatten)]
	pub args: Args,
}
