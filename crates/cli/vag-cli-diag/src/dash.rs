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
use vag_cli_core::table::Table;
use vag_cli_core::{note, outln, say};

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
	/// names in `vagcan devices` and `dashcfg`. An `over budget` row names the pages the bus
	/// estimate would serve slower than they ask. `-v` adds a table of how each channel
	/// resolved (identifier, bits, type, scaling, rate, measured or declared), the alarms, and
	/// a table of the bus estimate per page; `-vv` what each unit's catalog held and which
	/// rows each `ref` was chosen from.
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
	/// OUT: a page on `http://127.0.0.1:<port>/`, opened in the browser; the
	/// address is the one line on stdout. Its
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
			note!("read {}", vag_cli_core::datadir::tilde(&inputs.input_path));
			note!(
				"read {}: {} {}",
				vag_cli_core::datadir::tilde(&inputs.record),
				inputs.units.len(),
				crate::render::plural(inputs.units.len(), "unit")
			);
			// The channels of units nothing has read yet, out of the VCDS
			// installation the project was set up from. Offline: the read needs
			// the units' identities — the car's record — and the installation,
			// not the car. Off the main task, or Ctrl-C waits for it to finish.
			// The build below knows it was tried, so a unit still unread is not
			// a reason to tell the reader to run this command.
			vag_cli_core::registry::ensure_async(inputs.units.clone()).await;
			inputs.read_attempted = true;
			let written = dash::build_inputs(inputs)?;
			report(&written);
			Ok(())
		}
	}
}

/// What a build says: the plan as a `key  value` block on stdout — with an `over budget` row
/// when the bus estimate comes out short on a page; on stderr each line of advice — what the
/// input lacks and what to write — and at `-v` the rest of the build log: a table of how each
/// channel resolved, the lines on alarms and inputs, and a table of each page's bus share.
pub fn report(written: &dash::Written) {
	let built = &written.built;
	if vag_cli_core::verbosity::at(vag_cli_core::verbosity::NOTES) {
		let caps = |headers: &[&str]| headers.iter().map(|h| h.to_uppercase()).collect::<Vec<_>>();
		let channel_headers = caps(&dash::ChannelRow::HEADERS);
		let channel_headers: Vec<&str> = channel_headers.iter().map(String::as_str).collect();
		let mut channels = Table::new(&channel_headers).right(4).right(5).right(6);
		for row in &built.channels {
			channels.row(row.cells());
		}
		if !channels.is_empty() {
			note!("{}\n", channels.render(None).trim_end());
		}
		for line in built.prose() {
			note!("{line}");
		}
		let bus_headers = caps(&dash::BusRow::HEADERS);
		let bus_headers: Vec<&str> = bus_headers.iter().map(String::as_str).collect();
		let mut bus = Table::new(&bus_headers).right(1);
		for row in &built.bus {
			bus.row(row.cells());
		}
		if !bus.is_empty() {
			note!("{}", bus.render(None).trim_end());
		}
	}
	outln!("{}", card(written).trim_end());
	for line in &built.advice {
		say!("{line}");
	}
}

/// The plan as a card, `gh pr view`'s shape: the plan and its car on the first line — the
/// id is what a board flashed with it names in `vagcan devices` and `dashcfg` — what it
/// holds on the meta line, then the pages the bus comes out short on, and the files written.
fn card(written: &dash::Written) -> String {
	use vag_cli_core::marks::facts;
	let plan = &written.built.plan;
	let count = |n: usize, what: &str| format!("{n} {}", crate::render::plural(n, what));
	let mut out = format!(
		"{}\n{}\n",
		facts([format!("plan {:016x}", dash::plan_id(plan)), plan.vin.clone()]),
		facts([
			count(plan.pages.len(), "page"),
			format!("{} on {}", count(plan.channels.len(), "channel"), count(plan.units.len(), "unit")),
			count(plan.alarms.len(), "alarm"),
		])
	);
	if !written.built.over_budget.is_empty() {
		out.push('\n');
		out.push_str(&vag_cli_core::table::section(
			"OVER BUDGET",
			[dash::over_budget_text(&written.built.over_budget)],
		));
	}
	out.push('\n');
	out.push_str(&vag_cli_core::table::section(
		"WRITTEN",
		[&written.json, &written.rust].map(|path| vag_cli_core::datadir::tilde(path)),
	));
	out
}

#[cfg(test)]
mod tests {
	use super::*;
	use vag_cli_core::verbosity::{Capture, assert_split};

	fn written() -> dash::Written {
		let plan = dash::Plan {
			vin: "TESTVIN0000000001".into(),
			language: "en".into(),
			units: vec![],
			channels: vec![],
			pages: vec![],
			alarms: vec![],
			stalk: None,
			stopwatch: None,
			buttons: vec![],
			strip: None,
			standstill: None,
		};
		let advice = "stalk: there is no [stopwatch] — a press of measure only silences an alarm".to_string();
		let boost = dash::ChannelRow {
			label: "BOOST".into(),
			reference: "01:IDE00191".into(),
			did: 0x202A,
			bit_offset: 0,
			bit_length: 16,
			signed: false,
			big_endian: true,
			factor: 0.001,
			offset: 0.0,
			hz: 2.0,
			proven: true,
			source: "IDE00191".into(),
		};
		let bus_line = "bus: page 1 \"MAIN\" ~120 % — reads 01:IDE00191 BOOST 1.5/s of 2".to_string();
		dash::Written {
			built: dash::Built {
				plan,
				notes: vec![
					boost.line(),
					advice.clone(),
					"bus: an estimate at 34 ms an exchange".into(),
					bus_line.clone(),
				],
				advice: vec![advice],
				tabled: vec![boost.line(), bus_line],
				channels: vec![boost],
				bus: vec![dash::BusRow {
					page: "1 \"MAIN\"".into(),
					load: "~120 %".into(),
					short_of: "01:IDE00191 BOOST 1.5/s of 2".into(),
				}],
				over_budget: vec![dash::OverBudget::Page(1), dash::OverBudget::Page(3), dash::OverBudget::Stopwatch],
			},
			dir: "/cars/X".into(),
			json: "/cars/X/plan.json".into(),
			rust: "/cars/X/plan.rs".into(),
			inputs: vec![],
		}
	}

	/// The plan as a block on stdout, the pages the bus comes out short on among it; the advice
	/// by default and the log at `-v`, its channels and its bus estimate as tables, on stderr.
	#[test]
	fn a_build_is_a_block_of_data_with_its_advice_and_its_log_at_v() {
		let written = written();
		let run = |level| {
			let capture = Capture::start(level);
			report(&written);
			capture.finish()
		};
		let runs = [run(0), run(1), run(2)];
		assert_eq!(
			runs[0].stdout,
			format!(
				"plan {:016x} · TESTVIN0000000001\n\
				 0 pages · 0 channels on 0 units · 0 alarms\n\
				 \n\
				 OVER BUDGET\n\
				 \x20 page 1, 3, stopwatch\n\
				 \n\
				 WRITTEN\n\
				 \x20 /cars/X/plan.json\n\
				 \x20 /cars/X/plan.rs\n",
				dash::plan_id(&written.built.plan)
			)
		);
		assert_eq!(
			runs[0].stderr,
			"stalk: there is no [stopwatch] — a press of measure only silences an alarm\n"
		);
		assert_eq!(
			runs[1].stderr,
			"LABEL  CHANNEL      DID@BIT/WIDTH  TYPE  SCALE  OFFSET  HZ  SOURCE\n\
			 BOOST  01:IDE00191  202A@0/16      u BE  0.001       0   2  measured IDE00191\n\
			 \n\
			 bus: an estimate at 34 ms an exchange\n\
			 PAGE        LOAD  SHORT-OF\n\
			 1 \"MAIN\"  ~120 %  01:IDE00191 BOOST 1.5/s of 2\n\
			 stalk: there is no [stopwatch] — a press of measure only silences an alarm\n",
			"the advice once, not again in the log; no row of a table again as a line"
		);
		assert_split("dev dash build", &runs);

		// Nothing short, no row for it.
		let mut calm = written;
		calm.built.over_budget.clear();
		let capture = Capture::start(0);
		report(&calm);
		assert!(!capture.finish().stdout.contains("OVER BUDGET"));
	}

	#[test]
	fn the_over_budget_row_names_pages_then_a_stopwatch_run() {
		use dash::OverBudget::{Page, Stopwatch};
		assert_eq!(dash::over_budget_text(&[Page(2)]), "page 2");
		assert_eq!(dash::over_budget_text(&[Stopwatch]), "stopwatch");
		assert_eq!(dash::over_budget_text(&[Page(1), Page(3), Stopwatch]), "page 1, 3, stopwatch");
	}
}
