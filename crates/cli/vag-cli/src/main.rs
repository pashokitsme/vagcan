//! `vagcan` — read a VAG car over CAN.
//!
//! Read-only by construction: the UDS client's allowlist admits only reads
//! (`0x22` ReadDataByIdentifier, `0x19` DTC reads, session control and
//! TesterPresent). Nothing here writes to a control unit.
//!
//! The commands are the live ones. The HEX-clone experiments that used to live
//! here (`doctor`, `probe`, `handshake`, `replay-drive`, `decode`) drove a
//! cable whose session crypto is a dead end for this project; the research and
//! the `vag-hex` crate remain, but they are not product commands.

// This crate is the command surface and nothing else: the clap declarations,
// and a dispatcher that hands each one to the crate that does the work. The
// modules below are `use`d rather than declared, so every `vag_cli_core::analyse::run(…)`
// written when this was one crate still reads the same — it now names another
// crate's module instead of a local file.
mod overview;

use vag_cli_core::device::ADAPTER_BAUD;
use vag_cli_core::{config, datadir, device, glossary, plan, progress, project};
use vag_cli_diag::{anomaly, faults, labels, props, recording, render, rescue, safety, scan, setup, sniff, survey, vcds, watch};
#[cfg(feature = "measure")]
use vag_cli_measure as measure;

use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use vag_uds_can::{IsoTpCan, SlcanMode};
use vag_uds_client::address::UnitAddress;
use vag_uds_client::{AsyncUdsClient, UdsReadExt};

/// `--help`'s tour of the commands, built rather than written, because one line
/// of it is not always there.
///
/// `measure` is a cargo feature, and this tour used to name it unconditionally:
/// `cargo build --no-default-features` then produced a `--help` listing a
/// subcommand the binary does not have, and typing the word it had just
/// suggested answered `unrecognized subcommand`. A help text is a promise about
/// what the binary in front of you does; the promise now comes from the same
/// `cfg` the subcommand does.
fn long_about() -> String {
	format!(
		"Read a VAG car over a USB-CAN adapter on the OBD-II port.\n\n\
         Read-only: this tool never writes to a control unit.\n\n\
         Wiring: OBD-II pin 6 → CAN-H, pin 14 → CAN-L, pin 5 → GND,\n\
         and the adapter's termination jumper OFF.\n\n\
         START HERE\n  \
         vagcan setup              once, offline: read a VCDS install or an ODIS\n                            \
         project for names and scalings. With no path given it asks\n                            \
         which, and offers to download an installation.\n  \
         vagcan devices            is the adapter connected?\n  \
         vagcan info               which car is this?\n  \
         vagcan units              which control units does it have?\n\n\
         LOOK AT THE CAR\n  \
         faults                    stored fault codes\n  \
         units --identify 01       everything one unit says about itself\n  \
         sensors                   the standard OBD-II readings\n\n\
         WATCH IT LIVE\n  \
         watch                     values from several units, chosen on screen{}\n\n\
         THE WORKSHOP\n  \
         dev ...                   build and prove the data the above runs on:\n                            \
         the whole-car survey, the bus sniffer, your own channel\n                            \
         names, and the offline work over recordings and over\n                            \
         VCDS's files. `vagcan dev --help`.",
		if cfg!(feature = "measure") {
			"\n  measure                   time an acceleration run"
		} else {
			""
		}
	)
}

#[derive(Parser)]
#[command(
	name = "vagcan",
	version,
	about = "Read a VAG car (VW / Audi / Škoda / SEAT) over CAN. \
             Wiring: OBD-II pin 6 → CAN-H, pin 14 → CAN-L, pin 5 → GND, termination OFF. \
             Start with `vagcan devices`.",
	long_about = long_about()
)]
struct Cli {
	/// Which car's data to read — a directory name under `~/.vagcan/data/`.
	///
	/// Only needed with more than one car set up. `vagcan setup` writes down
	/// the one it just built, `VAGCAN_PROJECT` overrides that for a shell, and
	/// this overrides both for one command.
	#[arg(long, global = true, value_name = "ID")]
	project: Option<String>,

	/// Nothing at all is a question — "what is this and what do I type" — and
	/// clap's answer to it was `error: requires a subcommand`, which is true of
	/// the grammar and useless to a person. `None` is that question, and
	/// [`overview`] answers it.
	#[command(subcommand)]
	command: Option<Command>,
}

// Clone because a command that stopped for want of label data is run again
// once that data has been made — see `dispatch_or_offer`. Nothing here is more
// than a handful of strings and flags.
#[derive(Clone, Subcommand)]
enum Command {
	/// Learn a car from a VCDS installation or an ODIS project. Run once. Offline.
	///
	/// Everything the label files contribute — the parsed label files
	/// themselves, the measurement names, the `.rod` section keys — is derived
	/// from somebody else's data and cannot be shipped with this tool. This
	/// recovers it from what you have.
	///
	/// Two sources, and with no path given it asks which. A VCDS installation
	/// gives names; an extracted ODIS-Service project gives names *and*
	/// scalings, per identifier, with no drive required. Both land in one
	/// project under `~/.vagcan/data/<id>/`, and a second source is added to
	/// a project rather than replacing what is in it.
	///
	/// It takes minutes, mostly in the name recovery, and it touches no car.
	/// Running it again on an unchanged source does nothing and says so.
	///
	/// No VCDS installation: https://www.ross-tech.com/vcds/download/
	Setup {
		/// What to read: a VCDS installation root (the directory holding
		/// `Labels/` and `UDS_EV/`) or an extracted ODIS project folder. Leave
		/// it out and it asks which, offering to download an installation.
		#[arg(value_name = "DIR")]
		dir: Option<String>,
		/// Redo every step, whatever is already in the project.
		#[arg(long)]
		refresh: bool,
	},

	/// List connected USB-CAN adapters.
	///
	/// Start here if a command says it cannot find an adapter.
	Devices,

	/// Identify the car: VIN, engine and gearbox passports.
	Info {
		/// Adapter to use. Omit it when only one is connected.
		#[arg(long, value_name = "PATH")]
		device: Option<String>,
	},

	/// Ask the gateway which control units this car has.
	///
	/// One read of the gateway's installation list, instead of sweeping every
	/// diagnostic address and waiting out a timeout for each one the car does
	/// not have. `--identify` has every unit name itself; `--identify <unit>`
	/// has one of them say everything it knows.
	Units {
		/// Adapter to use. Omit it when only one is connected.
		#[arg(long, value_name = "PATH")]
		device: Option<String>,
		/// Have the units name themselves: part number and component name, for
		/// every unit the gateway lists. Slower, and a unit that does not
		/// answer is reported as such.
		///
		/// Name ONE unit — a short number (01 engine, 02 gearbox, 09, 16, 17)
		/// or a request id (713, 70E) — and it reads that unit's whole
		/// identification block instead: every software and hardware version,
		/// the supplier numbers, the ODX label file the unit is described by,
		/// and whatever else answers, named where the meaning is documented and
		/// raw where it is not. That is 256 reads of one control unit, so it is
		/// refused on a moving car.
		#[arg(long, value_name = "UNIT", num_args = 0..=1)]
		identify: Option<Option<String>>,
		/// Read one unit's identification block while the car is moving.
		/// Refused by default: 256 reads of one control unit is a sweep, and a
		/// unit that falls over at speed is a different event from one that
		/// falls over on a driveway.
		///
		/// `requires` because there is nothing for it to lift without a unit
		/// named — and a flag that is accepted and ignored is the same defect
		/// `--range` without `--blind` is refused for: a run that did less than
		/// its flags said is how somebody concludes the tool is broken.
		#[arg(long, requires = "identify")]
		while_driving: bool,
	},

	/// Read stored fault codes from every control unit.
	///
	/// Only codes the unit has confirmed are called faults: asking for
	/// everything returns hundreds of tests that have merely never run since
	/// the memory was cleared. Read-only — clearing faults is a write, which
	/// this tool cannot do.
	Faults {
		/// Adapter to use. Omit it when only one is connected.
		#[arg(long, value_name = "PATH")]
		device: Option<String>,
		/// Read only these units, e.g. `01,713,70E`. Default: every unit the
		/// gateway lists.
		#[arg(long, value_name = "LIST")]
		ecu: Option<String>,
		/// Also dump each fault's raw extended-data record as hex. The layout is
		/// per-unit and mostly undecoded — for offline analysis.
		#[arg(long)]
		details: bool,
		/// Show every code the units list, not just the confirmed ones.
		#[arg(long)]
		all: bool,
		/// List every code each unit *can* report, in the unit's own order.
		#[arg(long)]
		supported: bool,
		/// Ask each unit for an extended diagnostic session first. Off by
		/// default and refused while the car is moving: that session is
		/// workshop mode, and a unit that assists the driver may stop
		/// assisting while it is in one.
		#[arg(long)]
		extended: bool,
		/// Where the recovered `.rod` section keys are cached. A fault
		/// catalogue is sealed with one, and recovering one costs ~95 s of
		/// every core — so they are kept as data, not searched for per run.
		/// Default: this project's `rod-keys.json`, written by `vagcan setup`.
		#[arg(long, value_name = "FILE")]
		iv_cache: Option<String>,
		/// Name the faults in a survey this tool already recorded, instead of
		/// reading the car. Offline; names them from what `vagcan setup`
		/// extracted into ~/.vagcan.
		#[arg(long, value_name = "FILE",
              conflicts_with_all = ["device", "ecu", "supported", "extended", "details"])]
		from: Option<String>,
	},

	/// Read the standard OBD-II sensors a control unit exposes.
	///
	/// These ride the legislated parameter set mirrored at `F400 + PID`, so
	/// their conversions are public and need no reverse engineering — and five
	/// of them were independently confirmed against this car.
	///
	/// They are only converted on the emissions-related units ISO 15765-4
	/// addresses (0x7E0..0x7E7), and only where the answer is the width SAE
	/// J1979 defines. Other units answer `F4xx` identifiers too and mean
	/// something else by them, so those are shown as bytes with the reason.
	Sensors {
		/// Adapter to use. Omit it when only one is connected.
		#[arg(long, value_name = "PATH")]
		device: Option<String>,
		/// Control unit: a short number (01 engine, 02 gearbox, 09, 16, 17) or
		/// a request id (713, 70E). `vagcan units` lists this car's.
		#[arg(long, default_value = "01", value_name = "ID")]
		ecu: String,
	},

	/// Live view of the car — configured from inside, not by flags.
	///
	/// Shows values from several control units at once. The catalogs cover the
	/// engine, gearbox and instrument cluster with proven scalings; every other
	/// unit is shown from this car's own cached survey, as raw bytes. Run
	/// `vagcan dev survey` once, parked, and the cache is written — after that
	/// `watch` offers every identifier the car answers, on every unit, with no
	/// flag. Press `c` to choose what appears.
	///
	/// A survey also decides what the chooser holds back. A project describes a
	/// vehicle family and no one car has all of it, so the channels this car was
	/// asked for and did not answer are kept off the list — along with the ones
	/// nothing can name — and `u` shows both. Without a survey nothing is hidden
	/// on those grounds: silence is only evidence where somebody asked.
	Watch {
		/// Adapter to use. Omit it when only one is connected.
		#[arg(long, value_name = "PATH")]
		device: Option<String>,
		/// Start with these selected, e.g. `01:2029,202A 713:1001`. The part
		/// before the colon is a unit — short number or request id — and a
		/// bare list means the engine.
		#[arg(long, value_name = "SPEC")]
		did: Option<String>,
		/// Target poll rate.
		#[arg(long, default_value_t = 10.0, value_name = "HZ")]
		hz: f64,
		/// Also record to CSV.
		#[arg(long, value_name = "FILE")]
		out: Option<String>,
		/// Use this survey file instead of the one kept for this car. Without
		/// it, the survey `vagcan dev survey` last recorded off this car is loaded
		/// from `~/.vagcan/cars/<VIN>/survey.jsonl` — offering every identifier
		/// the car answers, on every unit, as raw bytes.
		#[arg(long, value_name = "FILE")]
		survey: Option<String>,
		/// Replay a recording written by `--out` instead of reading a car.
		/// No adapter is opened and nothing is addressed — for trying the
		/// interface, or showing it, away from a vehicle. Pass `--survey`
		/// alongside it to get one tab per control unit; a recording alone
		/// does not say which unit each column came from.
		#[arg(long, value_name = "FILE", conflicts_with = "device")]
		replay: Option<String>,
		/// Playback speed for --replay. 2 is twice as fast as it happened.
		#[arg(long, default_value_t = 1.0, value_name = "N")]
		speed: f64,
		/// Poll for this many seconds and exit, printing CSV instead of drawing
		/// a screen. This is the plain-console mode: no terminal needed, so it
		/// works over a pipe, in a log, or from a script. Without `--out` the
		/// rows go to stdout, one per poll cycle, flushed as they happen.
		///
		/// Output that is not a terminal uses this mode whether or not it was
		/// asked for, running until interrupted.
		#[arg(long = "for", value_name = "SECONDS", value_parser = duration_arg, conflicts_with = "replay")]
		r#for: Option<Duration>,
		/// Where the proven measurement rows live. Each file is named after the
		/// part number or ODX name of the control unit it describes, so a car
		/// this tool has not seen before simply finds none.
		/// Default: this project's `~/.vagcan/data/<project>/measurements`.
		#[arg(long, value_name = "DIR")]
		data: Option<String>,
	},

	/// Time an acceleration run from the car's own speed signal.
	///
	/// Arms itself when the car stands still, starts when it moves, and times
	/// every mark on the way up — no keystroke is needed for a run to be
	/// measured, and nothing prompts the driver while the car is moving.
	///
	/// The ordinary invocation is `vagcan measure` with no flags at all. It
	/// gives every time, every mark, the acceleration, the distance and the
	/// shift costs. `--full` adds the power column and needs this car measured
	/// first, by `vagcan measure setup`.
	///
	/// There is no `--hz`: the rate is measured and reported, never asserted in
	/// advance, and a flag that throttled a stopwatch could only make it worse.
	#[cfg(feature = "measure")]
	Measure(#[command(flatten)] measure::args::Args),

	/// The workshop: build and prove the data the other commands use.
	///
	/// Nothing under here is part of reading the car for an answer. These are
	/// the tools that make the data the commands above run on — the whole-car
	/// survey, the bus sniffer, the owner's own channel names, and the offline
	/// work over our recordings and over VCDS's files.
	Dev {
		#[command(subcommand)]
		tool: Dev,
	},
}

/// The workshop (see the `Dev` subcommand docs).
//
// Clone for the same reason `Command` is: `dispatch_or_offer` keeps a copy so
// the command can be run again once the label data it wanted has been made,
// and `vcds` and `survey` are among the commands that want it.
#[derive(Clone, Subcommand)]
enum Dev {
	/// Read EVERY control unit the car has, one after another.
	///
	/// Reads the gateway's installation list, then walks each unit: its
	/// identification block, its faults, and the identifiers that unit's own
	/// data declares it answers. Run it once parked and once driving — the
	/// identifiers whose bytes differ between the two runs are the live
	/// measurements, and that list needs no label file.
	///
	/// It does NOT sweep identifier space nothing vouches for. That is a fuzz
	/// test of a diagnostic server, and it needs `--blind <unit>` aimed by
	/// hand.
	///
	/// The result is always filed under this car in
	/// `~/.vagcan/cars/<VIN>/survey.jsonl`, whether or not `--out` was given,
	/// and that is what makes every control unit watchable: run this once and
	/// `vagcan watch` offers all of them from then on.
	Survey {
		/// Adapter to use. Omit it when only one is connected.
		#[arg(long, value_name = "PATH")]
		device: Option<String>,
		/// Hex ranges for the units named by --blind. Only means anything with
		/// --blind, and is refused without it rather than quietly ignored.
		#[arg(long, value_name = "SPEC")]
		range: Option<String>,
		/// Write the answers to this file (JSON lines, one object per unit).
		#[arg(long, value_name = "FILE")]
		out: Option<String>,
		/// Pause between reads, in milliseconds.
		#[arg(long, default_value_t = 2, value_name = "MS")]
		delay_ms: u64,
		/// Survey only these units, e.g. `17,70E,7E0`, skipping the gateway
		/// read.
		#[arg(long, value_name = "LIST")]
		only: Option<String>,
		/// Ask THESE units, named one by one (e.g. `713`), identifiers nothing
		/// declares they answer — a fuzz test of their diagnostic servers. Each
		/// request takes a path through firmware that may never have been
		/// exercised, and a path with a defect in it crashes the server, which
		/// on a control unit the car is relying on is not a small event. There
		/// is no value of this meaning "the whole car": one unit's crash is an
		/// incident, and every unit's is the same incident fifteen times.
		#[arg(long, value_name = "LIST")]
		blind: Option<String>,
		/// Compare two earlier survey files instead of reading the car, and
		/// list the identifiers whose bytes differ. Offline.
		#[arg(long, num_args = 2, value_names = ["BEFORE", "AFTER"])]
		diff: Option<Vec<String>>,
		/// Read while the car is moving. Refused by default: a declared
		/// identifier can still be the one whose path through the firmware has
		/// the defect in it, and a unit that falls over at speed is a different
		/// event from one that falls over on a driveway.
		#[arg(long)]
		while_driving: bool,
		/// Ask each unit for an extended diagnostic session first. Off by
		/// default and refused while the car is moving: that session is
		/// workshop mode, and a unit that assists the driver may stop
		/// assisting while it is in one.
		#[arg(long)]
		extended: bool,
	},

	/// Watch the bus. Listen-only: cannot disturb anything.
	///
	/// Made to run alongside VCDS — CAN is multi-drop, so both adapters share
	/// the bus and this one records the whole conversation.
	Sniff {
		/// Adapter to use. Omit it when only one is connected.
		#[arg(long, value_name = "PATH")]
		device: Option<String>,
		/// Write every frame to this capture file (JSON lines).
		#[arg(long, value_name = "FILE")]
		out: Option<String>,
		/// Record only diagnostic traffic, dropping the rest.
		#[arg(long)]
		diag_only: bool,
		/// Stop after this many seconds. Default: until Ctrl-C.
		#[arg(long, value_name = "N")]
		seconds: Option<u64>,
		/// Join the bus normally instead of listen-only, so the adapter
		/// acknowledges frames. Needed only when nothing else is on the bus to
		/// acknowledge — it is no longer strictly passive.
		#[arg(long)]
		active: bool,
	},

	/// Write your own names for channels. Offline.
	///
	/// The wording ODIS and VCDS carry is written for a diagnostic engineer:
	/// `Brake_pedal_information_plausibility` is accurate and unreadable at an
	/// open driver's door. This creates `~/.vagcan/names.csv`, where you write
	/// what you would call the channel — in English, in Russian, or both — and
	/// what you write wins over both vendors everywhere a name is shown.
	///
	/// It is keyed by VW's own text id, so a translation written once holds for
	/// every car afterwards, not just this one. Running it again keeps every
	/// line you have written and only adds ids that are new; the `current`
	/// column is what the channel is called today and is never read back.
	///
	/// Which column is used is `language` in `~/.vagcan/config.toml`.
	Glossary,

	/// Read back a drive this tool recorded. Offline — no car.
	///
	/// `vagcan watch --out` writes the CSV; these read it afterwards, at a
	/// desk. Neither has anything to say with the car in front of you.
	Recording {
		#[command(subcommand)]
		tool: recording::Tool,
	},

	/// Work with VCDS's own files: labels, recovered names, its logs. Offline.
	///
	/// Nothing here needs an adapter — the input is always a file that came
	/// from a VCDS installation, or something recovered from one.
	Vcds {
		#[command(subcommand)]
		tool: vcds::Tool,
	},
}

/// How a command's error is reported.
///
/// `Termination for Result<T, E: Debug>` prints exactly `Error: {err:?}` before
/// exiting with a failure code; this is that half of it, by hand. **`main`
/// returns an `ExitCode` rather than a `Result` for one reason**: the label-data
/// offer has to put the shortage in front of the reader *before* it asks whether
/// to fix it, and an error already printed there must not be printed again on
/// the way out. Owning the printing is what keeps a `no` at that question
/// costing the reader nothing they would not have seen anyway.
fn report(err: &anyhow::Error) {
	eprintln!("Error: {err:?}");
}

#[tokio::main]
async fn main() -> ExitCode {
	match run().await {
		Ok(code) => code,
		Err(err) => {
			report(&err);
			ExitCode::FAILURE
		}
	}
}

/// Everything before the command, and then the command.
async fn run() -> Result<ExitCode> {
	let cli = Cli::parse();
	// Before any command runs, because a dozen leaves consult it and none of
	// them takes it as an argument — the same reason the label files' unit
	// numbering is installed rather than threaded through.
	if let Some(id) = &cli.project {
		project::select(id);
	}
	// A setting that cannot be honoured is said once, at the top, rather than
	// applied silently: names would then arrive in the vendor's wording and
	// nothing on screen would connect that to the line somebody wrote.
	if let Some(why) = config::language_complaint(&config::load()) {
		eprintln!("{why}\n");
	}
	// It reads no more than `~/.vagcan/data/` and opens no adapter.
	let Some(command) = cli.command else {
		let facts = overview::gather();
		print!("{}", overview::render(&facts));
		return Ok(if overview::settled(&facts) {
			ExitCode::SUCCESS
		} else {
			ExitCode::FAILURE
		});
	};
	dispatch_or_offer(command).await
}

/// Run one command; if it stopped for want of label data, offer to make the
/// data and then run it again.
///
/// **The offer is made here and nowhere else.** Every command that needs what
/// `vagcan setup` produces reports the same typed shortage
/// ([`vag_cli_core::missing::NoLabelData`]), so one place can meet all of them
/// — and six commands each growing their own copy of a question that downloads
/// ninety megabytes is six places for one of them to quietly stop asking.
/// [`rescue`] carries the reason it is in `diag` and not beside the shortage in
/// `core`: fixing it means `setup`, and `core` may not depend on `diag`.
///
/// **The second attempt is the whole command over again**, which is safe
/// because this shortage is a missing *file*, checked on the way in: no site
/// that raises it has opened an adapter or printed any of the command's own
/// output yet — `faults` opens its label files before the port and says so in
/// as many words. One retry, and only after an explicit `y`.
async fn dispatch_or_offer(command: Command) -> Result<ExitCode> {
	// Kept before the first run consumes it: it is what "carry on with what you
	// asked for" is made of.
	let again = command.clone();
	let Err(err) = dispatch(command).await else {
		return Ok(ExitCode::SUCCESS);
	};
	// Not this shortage, or nobody at the keyboard: report it the ordinary way
	// and fail the ordinary way, which is byte for byte what happened before.
	if !rescue::worth_offering(&err) {
		return Err(err);
	}
	report(&err);
	if !rescue::offer(setup::vendor::ARCHIVE_BASE)? {
		// The shortage above is the refusal, and it has been said once.
		return Ok(ExitCode::FAILURE);
	}
	dispatch(again).await?;
	Ok(ExitCode::SUCCESS)
}

/// The command surface: one arm per command, each handing off to the crate that
/// does the work.
async fn dispatch(command: Command) -> Result<()> {
	match command {
		Command::Setup { dir, refresh } => setup::run(setup::Options {
			dir: dir.as_deref(),
			refresh,
			archive_base: setup::vendor::ARCHIVE_BASE,
			// `vagcan setup` with no path asks which source, and offers the
			// download as one of the answers. Only `rescue` skips that menu.
			download: false,
		}),
		Command::Devices => {
			println!("{}", device::render_list(&device::list()?));
			Ok(())
		}
		Command::Info { device } => info(device.as_deref()).await,
		// The two depths of the same question. `--identify <unit>` names one
		// unit and reads its whole identification block; `--identify` alone
		// asks every unit the gateway lists for the two fields that name it.
		Command::Units {
			device,
			identify: Some(Some(ecu)),
			while_driving,
		} => identification(device.as_deref(), &ecu, while_driving).await,
		// `requires = "identify"` above stops `units --while-driving` at the
		// parse, but `--identify` with no unit satisfies it and lands here,
		// where the flag has nothing to lift: this arm asks each unit for the
		// two fields that name it, which is not a sweep and is not gated on
		// road speed. Refused rather than dropped, for the reason on the flag.
		Command::Units { while_driving: true, .. } => bail!(
			"`--while-driving` needs a unit: `--identify <unit>`. With no unit named, `units` asks each one \
             for the two fields that name it — that is not a sweep, and nothing about it is gated \
             on road speed."
		),
		Command::Units { device, identify, .. } => units(device.as_deref(), identify.is_some()).await,
		Command::Sensors { device, ecu } => sensors(device.as_deref(), &ecu).await,
		Command::Watch {
			replay: Some(path),
			data,
			survey,
			speed,
			..
		} => watch::run_recording(&path, &data_dir(data.as_deref())?, survey.as_deref(), speed).await,
		Command::Watch {
			device,
			did,
			hz,
			out,
			survey,
			data,
			r#for,
			..
		} => {
			let preselect = match did.as_deref() {
				Some(spec) => plan::parse_spec(spec).map_err(|e| anyhow::anyhow!("--did: {e}"))?,
				None => Vec::new(),
			};
			// A pipe, a log file or an agent gets the plain-console view
			// whether or not it thought to ask: the full-screen one needs a
			// terminal and would otherwise fail with a bare errno. With no
			// duration named it runs until interrupted.
			let view = match (r#for, std::io::IsTerminal::is_terminal(&std::io::stdout())) {
				(Some(d), _) => watch::View::Plain(Some(d)),
				(None, false) => watch::View::Plain(None),
				(None, true) => watch::View::FullScreen,
			};
			watch::run(
				&device::resolve(device.as_deref())?,
				ADAPTER_BAUD,
				watch::Options {
					preselect: &preselect,
					hz,
					out: out.as_deref(),
					survey: survey.as_deref(),
					catalogs: &data_dir(data.as_deref())?,
					view,
				},
			)
			.await
		}
		#[cfg(feature = "measure")]
		Command::Measure(args) => measure::dispatch(args, &data_dir(None)?).await,
		Command::Faults {
			from: Some(survey),
			iv_cache,
			all,
			..
		} => faults::run_named(&survey, &rod_keys(iv_cache.as_deref())?, all),
		Command::Faults {
			device,
			ecu,
			details,
			all,
			supported,
			extended,
			iv_cache,
			..
		} => {
			faults::run(
				&device::resolve(device.as_deref())?,
				ADAPTER_BAUD,
				ecu.as_deref(),
				details,
				all,
				supported,
				extended,
				&rod_keys(iv_cache.as_deref())?,
			)
			.await
		}
		Command::Dev { tool } => dispatch_dev(tool).await,
	}
}

/// The workshop group: one arm per tool under `vagcan dev`.
async fn dispatch_dev(tool: Dev) -> Result<()> {
	match tool {
		Dev::Survey { diff: Some(files), .. } => survey::run_diff(&files[0], &files[1]),
		Dev::Survey {
			device,
			range,
			out,
			delay_ms,
			only,
			blind,
			extended,
			while_driving,
			..
		} => {
			survey::run(
				&device::resolve(device.as_deref())?,
				ADAPTER_BAUD,
				survey::Options {
					range: range.as_deref(),
					out: out.as_deref(),
					delay_ms,
					only: only.as_deref(),
					blind: blind.as_deref(),
					extended,
					while_driving,
				},
			)
			.await
		}
		Dev::Sniff {
			device,
			out,
			diag_only,
			seconds,
			active,
		} => {
			sniff::run(
				&device::resolve(device.as_deref())?,
				ADAPTER_BAUD,
				out.as_deref(),
				diag_only,
				seconds,
				active,
			)
			.await
		}
		Dev::Glossary => glossary_command(),
		Dev::Recording { tool } => recording::run(tool),
		// `labels --from-car` is the one thing under `vcds` that touches a
		// vehicle: it reads F19E off the unit and resolves that. The group is
		// otherwise pure file work, so it hands this one case back here rather
		// than starting a runtime of its own inside a synchronous call.
		Dev::Vcds { tool } => match vcds::run(tool)? {
			vcds::Outcome::Done => Ok(()),
			vcds::Outcome::FromCar { dir, ecu, iv_cache, device } => {
				let name = odx_name_from_car(device.as_deref(), &ecu).await?;
				println!("control unit {ecu} names its label file {name:?}\n");
				labels::resolve_odx(&dir, &name, &iv_cache)
			}
		},
	}
}

/// Where the proven measurement rows are for this run.
///
/// `--data` if it was given, this run's project otherwise. Never a path relative
/// to the working directory: that is what made these commands work in a checkout
/// and nowhere else.
fn data_dir(given: Option<&str>) -> Result<String> {
	Ok(
		datadir::or_default(given, || Ok(project::current()?.measurements_dir()))?
			.to_string_lossy()
			.into_owned(),
	)
}

/// Where the recovered `.rod` section keys are, for this run.
///
/// Per project, not shared beside the `.rod` pool: a recovered key is a property
/// of one file's *bytes*, and two VCDS builds ship a same-named `.rod` with
/// different content (design §4.2).
fn rod_keys(given: Option<&str>) -> Result<String> {
	Ok(
		datadir::or_default(given, || Ok(project::current()?.rod_keys()))?
			.to_string_lossy()
			.into_owned(),
	)
}

/// A duration in seconds, rejected here rather than at the point of use.
///
/// `Duration::from_secs_f64` panics on a negative, a NaN or an infinity, and
/// the point of use is inside the poll loop — with the adapter open and the car
/// on the bus. A usage error belongs before any of that happens.
fn duration_arg(text: &str) -> Result<Duration, String> {
	let seconds: f64 = text.parse().map_err(|_| format!("{text:?} is not a number"))?;
	if !seconds.is_finite() || seconds <= 0.0 {
		return Err(format!("{text:?} is not a positive number of seconds"));
	}
	Duration::try_from_secs_f64(seconds).map_err(|e| e.to_string())
}

/// Parse how the user named a control unit — `01`, `17`, or a request id like
/// `70E`. Which id block it lives on, and therefore which response rule
/// applies, is decided by `vag_uds_client::address`.
fn parse_ecu(text: &str) -> Result<UnitAddress> {
	vag_uds_client::address::parse(text).map_err(|e| anyhow::anyhow!("--ecu: {e}"))
}

/// Open the adapter and address one control unit over UDS.
async fn open_ecu(device_path: &str, unit: UnitAddress) -> Result<AsyncUdsClient<IsoTpCan<vag_uds_can::SerialSlcan>>> {
	let backend = device::open(device_path, ADAPTER_BAUD, SlcanMode::Normal).await?;
	Ok(AsyncUdsClient::new(IsoTpCan::new(
		backend,
		vag_uds_transport::CanId::Standard(unit.request),
		vag_uds_transport::CanId::Standard(unit.response),
	)))
}

/// Identify the car (see the `Info` subcommand docs).
async fn info(device_arg: Option<&str>) -> Result<()> {
	let path = device::resolve(device_arg)?;

	// One serial port, two control units: read the engine, then re-address the
	// same backend for the gearbox rather than re-opening the adapter.
	let engine_unit = parse_ecu("01")?;
	let mut engine_uds = open_ecu(&path, engine_unit).await?;
	let engine = engine_uds.read_identity().await;

	let gearbox_unit = parse_ecu("02")?;
	let backend = engine_uds.into_transport().into_backend();
	let mut gearbox_uds = AsyncUdsClient::new(IsoTpCan::new(
		backend,
		vag_uds_transport::CanId::Standard(gearbox_unit.request),
		vag_uds_transport::CanId::Standard(gearbox_unit.response),
	));
	let gearbox = gearbox_uds.read_identity().await;

	if engine.is_empty() && gearbox.is_empty() {
		println!("{}", render::render_nothing_answered());
		return Ok(());
	}
	println!("{}", render::render_info(engine.vin.as_deref(), &engine, &gearbox));
	println!(
		"\nNext:  vagcan units      what else this car has\n       \
         vagcan faults     stored fault codes\n       \
         vagcan sensors    live standard readings\n       \
         vagcan watch      live values from several units at once"
	);
	Ok(())
}

/// Read the standard OBD-II sensors (see the `Sensors` subcommand docs).
///
/// The table in `vag_data_labels::obd` is SAE J1979's, and J1979 is only binding on
/// the emissions-related units ISO 15765-4 addresses. Every identifier that
/// answers is still shown; whether its bytes become a number is decided by
/// `obd::conversion_for`, per parameter, from the unit's block and the width of
/// what it actually answered.
async fn sensors(device_arg: Option<&str>, ecu_text: &str) -> Result<()> {
	use render::SensorLine;
	use vag_data_labels::obd::{self, PIDS};

	let path = device::resolve(device_arg)?;
	let unit = parse_ecu(ecu_text)?;
	let established = unit.is_emissions_related();
	let mut uds = open_ecu(&path, unit).await?;

	// Ask for every standard parameter; the unit refuses the ones it does not
	// implement, and those are skipped rather than failing the run.
	let mut lines = Vec::new();
	for p in PIDS {
		let did = obd::did_for_pid(p.pid);
		let Ok(bytes) = uds.read_data_by_identifier(did).await else { continue };
		lines.push(match obd::conversion_for(p, established, &bytes) {
			Ok(def) => SensorLine::Converted(vag_uds_client::Reading {
				name: def.name.to_string(),
				unit: def.unit.to_string(),
				value: def.interpret(&bytes),
				raw: bytes,
			}),
			Err(why) => SensorLine::Unconverted { did, bytes, why },
		});
	}

	if lines.is_empty() {
		println!("{}", render::render_nothing_answered());
		return Ok(());
	}
	println!("{}", render::render_sensors(&unit.label(), &lines));
	Ok(())
}

/// Read the ODX label-file name a control unit reports for itself (F19E).
async fn odx_name_from_car(device_arg: Option<&str>, ecu_text: &str) -> Result<String> {
	const ODX_FILE_NAME: u16 = 0xF19E;

	let path = device::resolve(device_arg)?;
	let mut uds = open_ecu(&path, parse_ecu(ecu_text)?).await?;
	let data = uds
		.read_data_by_identifier(ODX_FILE_NAME)
		.await
		.context("reading the ODX file name (F19E) from the control unit")?;
	let name = String::from_utf8_lossy(&data).trim_end_matches(['\0', ' ']).to_string();
	if name.is_empty() {
		anyhow::bail!("the control unit returned an empty ODX file name");
	}
	Ok(name)
}

/// List the car's control units (see the `Units` subcommand docs).
async fn units(device_arg: Option<&str>, identify: bool) -> Result<()> {
	use vag_uds_can::{IsoTpCan, SlcanMode};
	use vag_uds_client::gateway;
	use vag_uds_transport::CanId;

	const GATEWAY_REQUEST: u16 = 0x710;
	const VW_RESPONSE_OFFSET: u16 = 0x6A;

	// The label files turn a part number the car reports into the unit's diagnostic
	// address and name, for any VAG car rather than for a list written here.
	// They come from what `vagcan setup` extracted, so `units --identify`
	// resolves names with no flag, and a machine that has not run setup simply
	// identifies without them.
	let label_files_dir = match identify {
		// A machine with no project set up simply identifies without names —
		// this is the ordinary "setup has not run yet" case, not an error.
		true => project::current().ok().filter(labels::has_project_labels),
		false => None,
	};
	let label_files = match &label_files_dir {
		Some(project) => {
			let db = labels::load_project(project)?;
			// The label files' numbering, in force for the rest of the run: what
			// each number *is*. Which id answers it is learned below, from the
			// car.
			labels::install_unit_numbers(&db);
			Some(db)
		}
		None => None,
	};

	let path = device::resolve(device_arg)?;
	let backend = device::open(&path, ADAPTER_BAUD, SlcanMode::Normal).await?;
	let channel = IsoTpCan::new(
		backend,
		CanId::Standard(GATEWAY_REQUEST),
		CanId::Standard(GATEWAY_REQUEST + VW_RESPONSE_OFFSET),
	);
	let mut uds = AsyncUdsClient::new(channel);

	let bitmap = uds
		.read_data_by_identifier(gateway::INSTALLATION_LIST)
		.await
		.context("reading the gateway's installation list")?;
	let ids = gateway::decode_installation_list(&bitmap);
	if ids.is_empty() {
		println!("The gateway listed no control units.");
		return Ok(());
	}

	println!("{} {}:\n", ids.len(), render::plural(ids.len(), "control unit"));
	let mut spinner = progress::Line::new();
	let mut identified = 0usize;
	let mut resolved = 0usize;
	let mut backend = uds.into_transport().into_backend();
	let listed = ids.len();
	for (at, id) in ids.into_iter().enumerate() {
		if identify {
			spinner.update(&format!("identifying {id:03X} — {} of {listed}", at + 1));
		}
		if !identify {
			println!("  {id:03X}");
			continue;
		}
		// Re-address the same adapter for each unit rather than reopening it.
		let channel = IsoTpCan::new(backend, CanId::Standard(id), CanId::Standard(id + VW_RESPONSE_OFFSET));
		let mut unit = AsyncUdsClient::new(channel);
		let part = unit.read_data_by_identifier(0xF187).await.ok();
		let component = unit.read_data_by_identifier(0xF197).await.ok();
		let text = |v: Option<Vec<u8>>| {
			v.map(|b| String::from_utf8_lossy(&b).trim_end_matches(['\0', ' ']).to_string())
				.unwrap_or_default()
		};
		let (part, component) = (text(part), text(component));
		spinner.finish();
		if part.is_empty() && component.is_empty() {
			println!("  {id:03X}  (did not answer)");
		} else {
			// Two names, both from data: the unit's own component string, and
			// what the label files call the part number — the latter also
			// supplying the diagnostic address people use.
			identified += 1;
			let name = label_files
				.as_ref()
				.and_then(|db| db.unit_for_part(&part))
				.map(|u| {
					resolved += 1;
					// This is the pairing: the label files say the part number is
					// unit 44, the car says 0x712 answered with it. Neither
					// half is in this program's source, and one read of the
					// car is what joins them.
					vag_uds_client::address::install([vag_uds_client::address::UnitNumber {
						number: u.address,
						request: Some(id),
						name: Some(u.name.clone()),
					}]);
					u.name.clone()
				})
				.unwrap_or_default();
			// The number in force — the override file's, then the label files',
			// then the built-in fallback's — or the request id when nothing
			// has paired one with it.
			let number = vag_uds_client::address::UnitAddress::from_request(id)
				.map(|a| a.label())
				.unwrap_or_else(|| format!("{id:03X}"));
			println!("  {id:03X}  {number:<4} {part:<14} {component:<16} {name}");
		}
		backend = unit.into_transport().into_backend();
	}
	if let Some(project) = &label_files_dir {
		// Silence here would read as "the label files agree"; it usually means the
		// label files have no entry for these part numbers.
		println!(
			"\n{resolved} of {identified} part numbers resolved against the label files of project `{}`.",
			project.id
		);
	}
	Ok(())
}

/// Write or refresh the owner's glossary (see the `Glossary` subcommand docs).
fn glossary_command() -> Result<()> {
	let project = project::current()?;
	let seeded = glossary::seed(&project)?;
	let language = config::language(&config::load());
	println!(
		"{} channel names in {}\n  {} already yours, {} still blank\n",
		seeded.total,
		seeded.path.display(),
		seeded.translated,
		seeded.blank
	);
	println!(
		"Write in the `{}` column and it wins over ODIS and VCDS wherever that \n         channel is shown. The `current` column is what it is called today and is \n         not read back. Change which column is used with `language` in {}.",
		language.code(),
		config::path()?.display()
	);
	Ok(())
}

/// Read one unit's whole identification block (`vagcan units --identify <unit>`).
///
/// The deeper of the two depths `units` has: the shallow one asks every unit
/// the two fields that name it, this asks one unit the whole 256-identifier
/// block and names what answers.
async fn identification(device_arg: Option<&str>, ecu_text: &str, while_driving: bool) -> Result<()> {
	let path = device::resolve(device_arg)?;
	let unit = parse_ecu(ecu_text)?;
	let ranges = scan::parse_ranges(props::IDENT_RANGE).expect("the built-in range parses");

	let mut backend = device::open(&path, ADAPTER_BAUD, SlcanMode::Normal).await?;
	// 256 reads aimed at one control unit is a sweep, whatever the block they
	// are in is called. This was the one sweep-shaped path in the tool with no
	// road-speed check on it — `vagcan units --identify`, which anybody could run at
	// speed while `scan` and `survey` refused to. Guarded now like the rest.
	if !while_driving {
		backend = match safety::require_stationary(backend).await {
			Ok(backend) => backend,
			Err((_, why)) => anyhow::bail!(
				"{why}\n\n\
                 Reading a unit's whole identification block asks it 256 identifiers, \n\
                 and a unit that mishandles one can stop doing its job while the car is \n\
                 in motion. Read it while parked, or pass --while-driving if you accept \n\
                 that risk with the car moving."
			),
		};
	}
	let mut uds = AsyncUdsClient::new(IsoTpCan::new(
		backend,
		vag_uds_transport::CanId::Standard(unit.request),
		vag_uds_transport::CanId::Standard(unit.response),
	));

	let mut found = Vec::new();
	// The read is bounded and the block is standardised, but the rule "stop
	// when something changes" is not about how big the read was. No witness:
	// there is nothing established as known-good before this runs, so the guard
	// watches only for the unit going quiet after it had been answering.
	let mut monitor = anomaly::Monitor::new(unit.request);
	let mut guard = scan::Guard {
		witness: None,
		monitor: &mut monitor,
	};
	scan::scan_dids(&mut uds, &ranges, std::time::Duration::from_millis(2), 400, &mut guard, |hit| {
		found.push(props::Property {
			did: hit.did,
			data: hit.data.clone(),
		});
		Ok(())
	})
	.await?;
	if let Some(halt) = monitor.halted() {
		let mut progress = progress::Line::new();
		progress.notice(&halt.report());
		anyhow::bail!("the read was stopped: control unit {} changed while it was being read", halt.unit());
	}

	if found.is_empty() {
		println!("{}", render::render_nothing_answered());
		return Ok(());
	}
	println!("{}", props::render(&format!("Control unit {}", unit.label()), &found));

	// Mode 09 lives outside the identification block and carries what a part
	// number cannot: which emissions calibration this unit is actually
	// running.
	let mut info = Vec::new();
	for (pid, name) in vag_data_labels::obd::VEHICLE_INFO {
		let did = vag_data_labels::obd::did_for_info_pid(*pid);
		let Ok(data) = uds.read_data_by_identifier(did).await else {
			continue;
		};
		if let Some(items) = vag_data_labels::obd::decode_info_text(&data) {
			info.push((*name, items.join(", ")));
		}
	}
	if !info.is_empty() {
		let width = info.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
		println!("  Vehicle information (OBD-II mode 09):");
		for (name, value) in info {
			println!("    {name:<width$}  {value}");
		}
	}
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use clap::CommandFactory;

	/// The help for one flag, named the way a user would reach it — the whole
	/// path, because the workshop commands live under `dev` now.
	fn flag_help(path: &[&str], flag: &str) -> String {
		let mut cli = Cli::command();
		let mut sub = &mut cli;
		for name in path {
			sub = sub
				.find_subcommand_mut(name)
				.unwrap_or_else(|| panic!("no subcommand {name} in {path:?}"));
		}
		let arg = sub
			.get_arguments()
			.find(|a| a.get_id() == flag)
			.unwrap_or_else(|| panic!("{path:?} has no {flag}"));
		// The long help, which is the whole doc comment: the short one is only
		// its first paragraph, and a flag whose second paragraph is the part
		// that carries the warning would pass a test that never read it.
		arg.get_long_help().or_else(|| arg.get_help()).map(|h| h.to_string()).unwrap_or_default()
	}

	#[test]
	fn a_bare_invocation_is_a_question_and_help_still_answers_its_own() {
		// `vagcan` on its own used to be `error: requires a subcommand`, which
		// is the first thing a new user sees and says nothing about the tool.
		// It parses now, and the absent subcommand is what `overview` answers.
		let bare = Cli::try_parse_from(["vagcan"]).expect("a bare `vagcan` must parse");
		assert!(bare.command.is_none());
		// The global flag still binds without one, so `vagcan --project X`
		// describes that project rather than failing.
		assert_eq!(
			Cli::try_parse_from(["vagcan", "--project", "SK37X"]).unwrap().project.as_deref(),
			Some("SK37X")
		);

		// And the overview replaced no part of `--help`: that is still clap's,
		// and an unknown subcommand is still an error rather than a screen.
		let help = Cli::try_parse_from(["vagcan", "--help"]).err().expect("--help is clap's own");
		assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp);
		assert!(help.to_string().contains("START HERE"), "{help}");
		let unknown = Cli::try_parse_from(["vagcan", "nonsense"])
			.err()
			.expect("an unknown subcommand is still an error");
		assert_eq!(unknown.kind(), clap::error::ErrorKind::InvalidSubcommand);
	}

	#[test]
	fn the_fit_flags_state_the_bar_that_is_actually_enforced() {
		// The bar is quoted twice — in this static help and in the failure
		// message the fitters print — and the numbers come from a third place,
		// `Thresholds::default()`. Loosening a threshold without updating the
		// help would leave the tool advertising a standard it no longer holds
		// itself to; this makes that a test failure.
		let bar = vag_cli_core::analyse::Thresholds::default();
		for path in [["dev", "vcds", "analyse"], ["dev", "recording", "calibrate"]] {
			for flag in ["min_r2", "min_points"] {
				let help = flag_help(&path, flag);
				assert!(help.contains(&format!("R² ≥ {:.3}", bar.min_r2)), "{path:?} {flag}: {help}");
				assert!(help.contains(&format!("≥ {} points", bar.min_points)), "{path:?} {flag}: {help}");
				assert!(
					help.contains(&format!("≥ {} distinct raw values", bar.min_levels)),
					"{path:?} {flag}: {help}"
				);
			}
		}
	}

	#[test]
	fn the_iv_cache_flag_is_legible_without_the_research() {
		// It used to explain itself with a `cargo run --features rod-crack`
		// invocation, which says nothing to someone holding an OBD adapter.
		let help = flag_help(&["dev", "vcds", "labels"], "iv_cache");
		assert!(!help.contains("cargo"), "{help}");
		assert!(help.contains(".rod"), "{help}");
	}

	#[test]
	fn naming_faults_offline_cannot_be_asked_to_touch_the_car() {
		// `faults --from` reads a recorded survey. Letting it keep --device or
		// --extended would offer a command that half-reads the car, and
		// --extended is the flag guarded by the road-speed check.
		let parse = |args: &[&str]| Cli::try_parse_from(["vagcan", "faults"].iter().chain(args).collect::<Vec<_>>());
		assert!(parse(&["--from", "s.jsonl"]).is_ok());
		for (flag, value) in [
			("--device", Some("/dev/x")),
			("--extended", None),
			("--supported", None),
			("--ecu", Some("713")),
		] {
			let mut args = vec!["--from", "s.jsonl", flag];
			args.extend(value);
			assert!(parse(&args).is_err(), "--from should refuse {flag}");
		}
		// There is no label directory to name: the names come from what
		// `vagcan setup` extracted, and nowhere else.
		assert!(parse(&["--from", "s.jsonl", "--labels", "d"]).is_err(), "--labels is gone");
	}

	#[test]
	fn watch_says_where_the_units_beyond_the_catalogs_come_from() {
		// `--survey` used to be the only way to see the twelve control units
		// no catalog covers, and its help said so as if that were fine. It is
		// an override now; the default is this car's own cached survey, and a
		// flag that does not say which file it is overriding is a flag nobody
		// knows they can omit.
		let help = flag_help(&["watch"], "survey");
		assert!(help.contains("instead of"), "{help}");
		assert!(help.contains("survey.jsonl"), "{help}");
	}

	#[test]
	fn every_sweep_is_refused_on_a_moving_car() {
		// The danger moves to whichever spelling is unguarded, so the rule is
		// asserted over every one of them. `scan` used to be the unguarded one;
		// then it was `properties`, which read 256 identifiers off a unit with
		// no road-speed check at all. `scan` is gone and `properties` is now
		// `units --identify <unit>` — which is in this list.
		let mut cli = Cli::command();
		let dev = cli.find_subcommand_mut("dev").expect("the workshop group exists").clone();
		for (sweep, sub) in [
			("units", cli.find_subcommand("units").expect("units exists")),
			("dev survey", dev.find_subcommand("survey").expect("the survey exists")),
		] {
			assert!(
				sub.get_arguments().any(|a| a.get_id() == "while_driving"),
				"{sweep} is a sweep with no --while-driving gate"
			);
		}
	}

	#[test]
	fn blind_sweeping_is_opt_in_on_every_sweep_and_says_what_it_costs() {
		// The default was to ask every unit 2816 identifiers nothing said
		// existed. That is a fuzz test of a diagnostic server, and it is now
		// something somebody asks for rather than something that happens.
		// One sweep, since `scan` was folded into `survey --only`. Written for
		// one deliberately: a second would be a second place for the warning to
		// go stale, which is what having two of them cost before.
		let mut cli = Cli::command();
		let sub = cli
			.find_subcommand_mut("dev")
			.expect("the workshop group exists")
			.find_subcommand_mut("survey")
			.expect("the sweep exists");
		let blind = sub
			.get_arguments()
			.find(|a| a.get_id() == "blind")
			.expect("the sweep sweeps blind with some way to say so");
		let help = blind.get_help().map(|h| h.to_string()).unwrap_or_default();
		assert!(help.contains("fuzz test"), "--blind does not say what it is: {help}");
		assert!(help.contains("crashes the server"), "--blind does not say what it risks: {help}");
	}

	#[test]
	fn a_whole_car_blind_sweep_cannot_be_asked_for() {
		// `survey --blind` takes a unit list and nothing else. A bare flag
		// would put the old default back behind five keystrokes, and the thing
		// that made this a whole-car event was that it applied to every unit.
		assert!(
			Cli::try_parse_from(["vagcan", "dev", "survey", "--blind"]).is_err(),
			"--blind must be aimed at named units"
		);
		assert!(Cli::try_parse_from(["vagcan", "dev", "survey", "--blind", "712"]).is_ok());
	}

	#[test]
	fn a_range_is_a_blind_range_and_says_so() {
		// `--range` used to describe the default sweep. It now describes only
		// what `--blind` sweeps, and naming one without a unit to aim it at is
		// refused at run time (`declared::blind_ranges`) rather than ignored —
		// so the help has to say which flag it belongs to.
		{
			let path = ["dev", "survey"];
			let help = flag_help(&path, "range");
			assert!(help.contains("--blind"), "{path:?} --range: {help}");
			assert!(
				help.to_lowercase().contains("refused") || help.to_lowercase().contains("only means"),
				"{path:?} --range does not say it is inert alone: {help}"
			);
		}
	}

	#[test]
	fn the_top_level_is_only_what_needs_a_car() {
		// The whole point of the `dev` group: a top level crowded with the
		// workshop cannot be scanned while standing at an open driver's door.
		// This is the rule made enforceable, and every name that has ever moved
		// off the top level stays on the denylist — the leaves that went under
		// `vcds` and `recording` first, then those two groups themselves along
		// with `survey`, `sniff` and `glossary` when `dev` swallowed them, and
		// finally `scan` and `properties`, which were deleted outright as
		// second spellings of `dev survey --only` and `units --identify`.
		let cli = Cli::command();
		let top: Vec<&str> = cli.get_subcommands().map(|s| s.get_name()).collect();
		for offline in [
			"analyse",
			"calibrate",
			"discover",
			"labels",
			"names",
			"survey",
			"sniff",
			"glossary",
			"recording",
			"vcds",
			"scan",
			"properties",
		] {
			assert!(!top.contains(&offline), "{offline} belongs under a group, not at the top");
		}
		for live in ["setup", "devices", "info", "units", "faults", "watch", "sensors"] {
			assert!(top.contains(&live), "{live} needs a car and belongs at the top");
		}
		// And the ones that moved are reachable where they were moved to,
		// rather than merely gone.
		let dev = cli.find_subcommand("dev").expect("the workshop group exists");
		let workshop: Vec<&str> = dev.get_subcommands().map(|s| s.get_name()).collect();
		for tool in ["survey", "sniff", "glossary", "recording", "vcds"] {
			assert!(workshop.contains(&tool), "{tool} moved to `dev` and must be there");
		}
	}

	#[test]
	fn one_unit_identified_in_full_is_a_depth_of_units_not_a_command_of_its_own() {
		// `properties --ecu 01` and `units --identify` asked the same question
		// at two depths, and only one of them was guarded. One flag now, whose
		// argument is the depth.
		assert!(Cli::try_parse_from(["vagcan", "units"]).is_ok());
		assert!(Cli::try_parse_from(["vagcan", "units", "--identify"]).is_ok());
		assert!(Cli::try_parse_from(["vagcan", "units", "--identify", "713"]).is_ok());
		assert!(Cli::try_parse_from(["vagcan", "properties", "--ecu", "01"]).is_err());
		// The depth is what the guard is about, and the help has to name the
		// unit spelling somebody would type.
		let help = flag_help(&["units"], "identify");
		assert!(help.contains("713"), "{help}");
		assert!(help.contains("moving car"), "{help}");
	}
}
