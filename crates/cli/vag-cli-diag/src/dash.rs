//! The dash device's plan, built at a desk.
//!
//! The device resolves nothing: every unit address, identifier, bit layout,
//! scaling and label it will ever show is decided on the laptop, where the
//! catalogs are. [`vag_cli_core::dash`] is that decision; this is the command
//! surface over it, and it holds no logic of its own — the firmware's own build
//! calls the same function, so anything decided here and not there would be a
//! second answer to a question that has one.

use std::path::PathBuf;

use anyhow::Result;
use clap::Subcommand;

use vag_cli_core::dash;

// Clone for the reason `recording::Tool` is: the dispatcher keeps a copy of the
// command so it can be run again after the label data has been made.
#[derive(Clone, Subcommand)]
pub enum Tool {
	/// Build the plan the dash firmware executes, for one car. Offline.
	///
	/// FOR: seeing what the device will show and why, without compiling
	/// firmware. Every unit address, identifier, bit layout, scaling, unit
	/// string and label is resolved here and written into the plan, so the dash
	/// cannot show a number `vagcan watch` would not. A channel the car's
	/// variant does not declare fails the build and the message names it; so
	/// does one whose scaling is not linear.
	///
	/// IN: `~/.vagcan/dash/<VIN>/dash.toml`, written by hand, together with the
	/// car's own survey (`vagcan dev survey`) and this project's catalogs. No
	/// adapter, no car, no key in the ignition.
	///
	/// OUT: `plan.json` for a person and the simulator, `plan.rs` for the
	/// firmware, both under the car in `~/.vagcan/dash/<VIN>/` — wherever the
	/// input was read from.
	///
	/// If there is no input yet, the smallest one that builds is `vin =
	/// "<VIN>"`, then a `[[channel]]` with `ref = "01:IDE00025"`, then a
	/// `[[page]]` with `kind = "values"`, `title = "MAIN"` and `cells =
	/// ["01:IDE00025"]`. A channel is `<unit>:<text id>` or
	/// `<unit>:<DID>[@<bit offset>]`, and may carry its own `label` and
	/// `decimals`; a page is `values` (1 to 4 cells) or `chart` (one `cell`
	/// between `min` and `max`).
	///
	/// The firmware's own build runs this same build —
	/// `VAGCAN_DASH_VIN=<VIN> cargo build` in `crates/dash/vag-dash-fw` — so
	/// this command is for reading the result and the reasons, not a step
	/// before it.
	Build {
		/// The car to build for, as `vagcan info` reports it.
		#[arg(value_name = "VIN")]
		vin: String,
		/// Build input to read instead of `~/.vagcan/dash/<VIN>/dash.toml`.
		/// The outputs still go beside that default, under the car.
		#[arg(long, value_name = "FILE")]
		input: Option<PathBuf>,
	},
}

pub fn run(tool: Tool) -> Result<()> {
	match tool {
		Tool::Build { vin, input } => {
			let written = dash::build_for_car(&vin, input.as_deref())?;
			// The notes first, because they are the answer: one line per
			// channel saying which row was chosen, how it is decoded, and
			// whether the car proved it or a label file merely declared it.
			for note in &written.built.notes {
				println!("{note}");
			}
			println!("\nwrote {}", written.json.display());
			println!("wrote {}", written.rust.display());
			let plan = &written.built.plan;
			println!(
				"{} channels on {} units, {} pages",
				plan.channels.len(),
				plan.units.len(),
				plan.pages.len()
			);
			Ok(())
		}
	}
}
