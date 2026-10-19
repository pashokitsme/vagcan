//! The dash device's plan, built at a desk.
//!
//! The device resolves nothing: every unit address, identifier, bit layout,
//! scaling and label it will ever show is decided on the laptop, where the
//! catalogs are. [`vag_cli_core::dash`] is that decision; this is the command
//! surface over it, and it holds no logic of its own — the firmware's own build
//! calls the same function, so anything decided here and not there would be a
//! second answer to a question that has one.
//!
//! One thing this command does that the firmware's build does not: it reads the
//! channels of the car's recorded units out of the project's VCDS installation
//! first ([`vag_cli_core::registry::ensure_async`]), offline. The firmware's
//! build script never searches for keys — minutes of CPU inside `cargo build` —
//! so a car whose units `watch`, `measure` or `units --identify` recorded but
//! whose channels no command has read yet is built here first, once, and the
//! firmware build after that.

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use vag_cli_core::dash;

pub mod catalog;
pub mod edit;

// Clone for the reason `recording::Tool` is: the dispatcher keeps a copy of the
// command so it can be run again after the label data has been made.
#[derive(Clone, Subcommand)]
pub enum Tool {
	/// Build the plan the dash firmware executes, for one car. Offline.
	///
	/// FOR: seeing what the device will show and why, without compiling
	/// firmware — and, with a VCDS installation, reading the channels of the
	/// car's units the first time. Every unit address, identifier, bit layout,
	/// scaling, unit string and label is resolved here and written into the
	/// plan, so the dash cannot show a number `vagcan watch` would not. A
	/// channel the car's variant does not declare fails the build and the
	/// message names it; so does one whose scaling is not linear.
	///
	/// IN: `~/.vagcan/dash/<VIN>/dash.toml`, written by hand, together with the
	/// car's record of its control units (`~/.vagcan/cars/<VIN>/units.json`,
	/// written by `vagcan units --identify`, `vagcan watch` or `vagcan measure`
	/// with the car) and this project's catalogs. No adapter, no car, no key in
	/// the ignition. When the project was set up from a VCDS installation, the
	/// channels of units that installation has not been read for yet are read
	/// now, once — minutes when a key has to be searched for — and the build
	/// follows. The installation has to be where `setup` read it.
	///
	/// OUT: `plan.json` for a person to read, `plan.rs` for the
	/// firmware, both under the car in `~/.vagcan/dash/<VIN>/` — wherever the
	/// input was read from — and the plan's id, which a board flashed with it
	/// names in `vagcan devices` and `dashcfg`.
	///
	/// If there is no input yet, the smallest one that builds is at the end of
	/// this help. A channel is `<unit>:<text id>` or
	/// `<unit>:<DID>[@<bit offset>]`, and may carry its own `label`,
	/// `decimals` and `hz` (readings a second while shown, 2 otherwise); a page is `values` (1 to 4 cells) or `chart` (one `cell`
	/// between `min` and `max`). A channel may carry an alarm: `alarm_max` and/or
	/// `alarm_min`, each with an optional `alarm_max_off` / `alarm_min_off` where it goes off
	/// again; the board then shows the first values page with that channel when it crosses,
	/// whatever page is up. The channels' order is the alarms' priority.
	///
	/// A channel may also carry `setpoint = "<unit>:<row>"` — what its unit asked for, on the
	/// same unit — and the panel then draws the difference under the number. A drift alarm
	/// watches that difference: `drift_percent`, `drift_off_percent`, `drift_hold_ms` (how long
	/// it has to hold) and `drift_min_setpoint` (under which it says nothing).
	///
	/// `docs/dash/dash-toml.md` is the whole grammar, with the limits and every refusal.
	///
	/// The firmware's own build runs this same build —
	/// `VAGCAN_DASH_VIN=<VIN> cargo build` in `crates/dash/vag-dash-fw` — but
	/// reads no channels: it refuses to build for a car whose plan uses a unit
	/// whose channels have not been read, and names this command. So this is a
	/// step before the firmware build only when no command with the car has
	/// read the channels of the plan's units yet, and otherwise for reading the
	/// result and the reasons.
	#[command(after_long_help = format!("The smallest dash.toml, one value on one page:\n\n{}", dash::SMALLEST_INPUT))]
	Build {
		/// The car to build for, as `vagcan info` reports it.
		#[arg(value_name = "VIN")]
		vin: String,
		/// Build input to read instead of `~/.vagcan/dash/<VIN>/dash.toml`.
		/// The outputs still go beside that default, under the car.
		#[arg(long, value_name = "FILE")]
		input: Option<PathBuf>,
	},

	/// Lay out the dash's pages in a browser, for one car. Offline.
	///
	/// FOR: choosing what each page shows — cells, charts, titles, labels — on
	/// the board's own renderer, from the channels this car's build accepts,
	/// before writing them into `dash.toml`.
	///
	/// IN: the car's record of its control units and this project's catalogs,
	/// as `dev dash build` reads them, and `~/.vagcan/dash/<VIN>/dash.toml` when
	/// there is one: the page opens on its pages. No adapter, no car.
	///
	/// OUT: a page on `http://127.0.0.1:<port>/`, opened in the browser. Its
	/// files are under `~/.vagcan/dash/<VIN>/editor/`; the channel list is kept
	/// there and made again when the project, the record, names.csv or the language
	/// change. It changes no file of yours: it shows the `[[page]]` and
	/// `[[channel]]` blocks to copy into `dash.toml`, then `dev dash build`
	/// checks them. Nothing leaves 127.0.0.1. Ctrl-C ends it.
	///
	/// Needs a `vagcan` built with the `wasm32-unknown-unknown` target
	/// installed (`rustup target add wasm32-unknown-unknown`, then build again):
	/// that is when the renderer is compiled into it.
	Edit {
		/// The car, as `vagcan info` reports it.
		#[arg(value_name = "VIN")]
		vin: String,
	},
}

pub async fn run(tool: Tool) -> Result<()> {
	match tool {
		Tool::Edit { vin } => edit::run(&vin).await,
		Tool::Build { vin, input } => {
			// The input and the record first: a typo in `dash.toml`, a VIN that
			// does not match, a car with no record — each is a refusal that must
			// come before the read below, which can take minutes, not after it
			// (found in review, 2026-09-28).
			let mut inputs = dash::read_inputs(&vin, input.as_deref())?;
			// The channels of units nothing has read yet, out of the VCDS
			// installation the project was set up from. Offline: the read needs
			// the units' identities — the car's record — and the installation,
			// not the car. Off the main task, or Ctrl-C waits for it to finish.
			// The build below knows it was tried, so a unit still unread is not
			// a reason to tell the reader to run this command.
			vag_cli_core::registry::ensure_async(inputs.units.clone()).await;
			inputs.read_attempted = true;
			let written = dash::build_inputs(inputs)?;
			// The notes first, because they are the answer: one line per
			// channel saying which row was chosen, how it is decoded, and
			// whether the car proved it or a label file merely declared it.
			for note in &written.built.notes {
				println!("{note}");
			}
			println!("\nwrote {}", written.json.display());
			println!("wrote {}", written.rust.display());
			let plan = &written.built.plan;
			let count = |n: usize, what: &str| format!("{n} {}", crate::render::plural(n, what));
			println!(
				"{} on {}, {}",
				count(plan.channels.len(), "channel"),
				count(plan.units.len(), "unit"),
				count(plan.pages.len(), "page")
			);
			// What a board flashed with it names in `vagcan devices` and `dashcfg`.
			println!("plan {:016x}", dash::plan_id(plan));
			Ok(())
		}
	}
}
