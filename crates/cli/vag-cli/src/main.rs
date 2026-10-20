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

use vag_cli_core::device::{ADAPTER_BAUD, DeviceArg, NotThroughTheBoard, Target};
use vag_cli_core::{config, datadir, device, glossary, plan, project};
use vag_cli_diag::{car, dash, faults, labels, recording, rescue, setup, sniff, vcds, watch};
#[cfg(feature = "measure")]
use vag_cli_measure as measure;

use std::io::IsTerminal as _;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

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
         vagcan setup              offline: read an ODIS project or a VCDS install\n                            \
         for names and scalings. With no path given it asks which,\n                            \
         and offers to download an installation.\n  \
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
         the bus sniffer, your own channel names, the dash plan,\n                            \
         and the offline work over recordings and over\n                            \
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
             Start with `vagcan setup`.",
	long_about = long_about(),
	// After the subcommands exist: it decides, command by command, where `--project` and
	// `--slcan` are shown.
	scoped_globals()
)]
struct Cli {
	/// Which project to read — a directory name under `~/.vagcan/data/`.
	///
	/// A project is one car model's data, from `vagcan setup`. Only needed with
	/// more than one project set up. `vagcan setup` writes down
	/// the one it just built, `VAGCAN_PROJECT` overrides that for a shell, and
	/// this overrides both for one command.
	//
	// The globals are listed after each command's own flags. Left to clap they take their
	// place here (0 and 1) in every subcommand too, and sort in between that command's
	// first flags — `--project` between `--device` and `--ble`.
	#[arg(long, global = true, value_name = "ID", display_order = 900)]
	project: Option<String>,

	/// Use the dash board's USB cable as a plain slcan adapter.
	///
	/// Without it, a dash board on its `dash` image is read through: the board runs the
	/// requests and its panel keeps running. With it, the board stops the panel and relays
	/// CAN frames the way a CANable does — what `dev sniff` needs, and what lets a sweep run
	/// on the bench. It changes nothing for any other adapter, and means nothing over BLE.
	#[arg(long, global = true, display_order = 901)]
	slcan: bool,

	#[command(flatten)]
	verbose: vag_cli_core::verbosity::VerboseArg,

	/// Nothing at all is a question — "what is this and what do I type" — and
	/// clap's answer to it was `error: requires a subcommand`, which is true of
	/// the grammar and useless to a person. `None` is that question, and
	/// [`overview`] answers it.
	#[command(subcommand)]
	command: Option<Command>,
}

/// The commands each global flag means something on, by their path under `vagcan`.
///
/// `--project` is read where a command consults the project's data; `--slcan` where a
/// command opens an adapter. Everywhere else the flag is still accepted — `vagcan --slcan
/// setup` parses as it always did — but not shown: on `setup`, `devices` or `dev vcds rod`
/// the two lines were noise in every help screen. A path not listed hides both, so a new
/// command shows a flag only once somebody has said it means something there.
const GLOBALS_APPLY: &[(&str, &[&str])] = &[
	(
		"project",
		&[
			"units",
			"faults",
			"watch",
			"measure",
			"measure setup",
			"dev glossary",
			"dev recording dash",
			"dev dash build",
			"dev dash edit",
			"dev vcds labels",
			"dev vcds names",
			"dev vcds stats",
			"dev vcds tttext",
		],
	),
	(
		"slcan",
		&[
			"info",
			"units",
			"faults",
			"sensors",
			"watch",
			"measure",
			"measure setup",
			"dev sniff",
			"dev vcds labels",
		],
	),
];

/// `--verbose`'s help on a command that opens no adapter: nothing to say about the bus.
const VERBOSE_OFFLINE: &str = "More on stderr. -v: where the data came from, why a row is unnamed or skipped, caveats and timings; -vv: more detail where the command has it. Data stays on stdout.";

/// Whether global flag `id` is shown on the command at `path`. `--verbose` means
/// something on every command, and is shown on every one.
fn global_applies(id: &str, path: &str) -> bool {
	id == "verbose" || GLOBALS_APPLY.iter().any(|(flag, paths)| *flag == id && paths.contains(&path))
}

/// [`GLOBALS_APPLY`], applied to the command tree.
///
/// A trait so that the derive can call it (`#[command(scoped_globals())]`) after it has
/// added the subcommands: then `Cli::command()`, `Cli::parse()` and every test's
/// `try_parse_from` see one tree, rather than a parse that bypasses the scoping.
trait ScopedGlobals {
	fn scoped_globals(self) -> Self;
}

impl ScopedGlobals for clap::Command {
	fn scoped_globals(self) -> Self {
		let globals: Vec<clap::Arg> = self.get_arguments().filter(|a| a.is_global_set()).cloned().collect();
		scope_globals(self, &globals, "")
	}
}

/// Give every subcommand under `command` its own copy of each global flag, shown or hidden.
///
/// clap copies a global flag into a subcommand only when the subcommand has none of that
/// name, so a copy put there first is the one it keeps — and a value given on it still
/// reaches `Cli`, which reads globals by name.
fn scope_globals(mut command: clap::Command, globals: &[clap::Arg], path: &str) -> clap::Command {
	let names: Vec<String> = command.get_subcommands().map(|s| s.get_name().to_owned()).collect();
	for name in names {
		let here = if path.is_empty() { name.clone() } else { format!("{path} {name}") };
		command = command.mut_subcommand(&name, |mut sub| {
			for flag in globals {
				let shown = global_applies(flag.get_id().as_str(), &here);
				let mut flag = flag.clone().hide(!shown);
				// `-vv`'s bus traffic means something only where a command talks to a car.
				if flag.get_id() == "verbose" && !global_applies("slcan", &here) {
					flag = flag.help(VERBOSE_OFFLINE).long_help(VERBOSE_OFFLINE);
				}
				sub = sub.arg(flag);
			}
			scope_globals(sub, globals, &here)
		});
	}
	command
}

// Clone because a command that stopped for want of label data is run again
// once that data has been made — see `dispatch_or_offer`. Nothing here is more
// than a handful of strings and flags.
#[derive(Clone, Subcommand)]
enum Command {
	/// Read a car model's data from a VCDS installation or an ODIS project. Offline.
	///
	/// Everything the label files contribute — the parsed label files
	/// themselves, the measurement names, the `.rod` section keys, the
	/// channels — is derived from somebody else's data and cannot be shipped
	/// with this tool. This recovers it from what you have.
	///
	/// Two sources, and with no path given it asks which. An extracted
	/// ODIS-Service project gives names and scalings for every unit it
	/// describes, with no drive required. A VCDS installation gives names and
	/// fault text, and the channels of the units this machine's cars have been
	/// seen to carry: `vagcan watch`, `measure` or `units --identify` with the
	/// car records them and reads their channels from the installation the
	/// first time (`vagcan dev dash build` does the same offline, for a car
	/// already recorded) — so keep it in place; an ODIS project can be deleted
	/// afterwards. Both land in one project under
	/// `~/.vagcan/data/<id>/`, and a second source is added to a project rather
	/// than replacing what is in it; where both describe a channel, ODIS wins.
	///
	/// An ODIS project reads in seconds; a VCDS installation can take tens of
	/// minutes the first time, most of it searching for keys. It touches no car.
	/// Running it again on a source already in the project replaces what that
	/// source wrote before and leaves every other source's data where it is,
	/// and a second VCDS installation replaces the first. A VCDS run skips the
	/// copy and the label files when they are newer than what they read, and
	/// `--refresh` redoes them. `--list` shows what the project was read from, and
	/// `--drop` removes one source with everything it wrote.
	///
	/// Prints one row per step and a summary line; `-v` adds what each step wrote,
	/// why a step is short, and timings.
	///
	/// No VCDS installation: https://www.ross-tech.com/vcds/download/
	Setup {
		/// What to read: a VCDS installation root (the directory holding
		/// `Labels/` and `UDS_EV/`) or an extracted ODIS project folder. Leave
		/// it out and it asks which, offering to download an installation —
		/// and lets you pick the folder in a dialog rather than type its path.
		#[arg(value_name = "DIR")]
		dir: Option<String>,
		/// Redo every step, whatever is already in the project.
		#[arg(long)]
		refresh: bool,
		/// List the project's sources: id, kind, language, when read, rows per kind, path.
		#[arg(long, conflicts_with_all = ["dir", "refresh", "drop"])]
		list: bool,
		/// Remove one source and every row it wrote: `odis`, `vcds`, or an id from `--list`.
		/// Asks first.
		#[arg(long, value_name = "SOURCE", conflicts_with_all = ["dir", "refresh"])]
		drop: Option<String>,
		/// With `--drop`: do not ask. Needed without a terminal.
		#[arg(long, requires = "drop")]
		yes: bool,
	},

	/// Lists USB-CAN adapters and dash boards on USB.
	///
	/// Start here if a command says it cannot find an adapter. A dash board over
	/// Bluetooth is not looked for here: give a command `--ble`.
	Devices,

	/// Identify the car: VIN, and the engine's and gearbox's part numbers and software versions.
	Info {
		#[command(flatten)]
		device: DeviceArg,
	},

	/// List the car's control units.
	///
	/// One read of the car's list of control units, which the gateway keeps, instead
	/// of asking every diagnostic address and waiting out a timeout for each one the
	/// car does not have. Each is named from what is already on this machine — the
	/// project's label files and what an earlier `--identify` recorded — and nothing
	/// more is asked of the car for it. `--identify` has every unit name itself;
	/// `--identify <unit>` has one of them say everything it knows.
	#[command(mut_arg("device", |a| a.help(UNITS_DEVICE_HELP)))]
	Units {
		#[command(flatten)]
		device: DeviceArg,
		/// Have the units name themselves: part number, component name and the
		/// ODX file that describes each (F187, F197, F19E, F1A2), for every unit
		/// on the car's list, the gateway itself and the powertrain. Slower, and
		/// a unit that does not answer is reported as such. What they said is
		/// recorded under `~/.vagcan/cars/<VIN>/units.json` — the list `vagcan
		/// setup` and `vagcan dev dash build` work from when the car is not
		/// there — and with a VCDS installation their channels are read from it
		/// the first time.
		///
		/// Name ONE unit — a short number (01 engine, 02 gearbox, 09, 16, 17)
		/// or a request id (713, 70E) — and it reads that unit's whole
		/// identification block instead: every software and hardware version,
		/// the supplier numbers, the ODX label file the unit is described by,
		/// and whatever else answers, named where the meaning is documented and
		/// raw where it is not. That is a sweep: it reads one unit's
		/// identification block, 256 entries (F100–F1FF), one after another — so
		/// it is refused on a moving car.
		#[arg(long, value_name = "UNIT", num_args = 0..=1)]
		identify: Option<Option<String>>,
		/// Read one unit's identification block while the car is moving.
		/// Refused by default: 256 reads of one control unit is a sweep, and a
		/// unit that falls over at speed is a different event from one that
		/// falls over on a driveway.
		///
		/// `requires` because there is nothing for it to lift without a unit
		/// named — and a flag that is accepted and ignored is a defect of its
		/// own: a run that did less than its flags said is how somebody
		/// concludes the tool is broken.
		#[arg(long, requires = "identify")]
		while_driving: bool,
	},

	/// Read stored fault codes from every control unit.
	///
	/// Only codes the unit has confirmed are called faults: asking for
	/// everything returns hundreds of tests that have merely never run since
	/// the memory was cleared. Read-only — clearing faults is a write, which
	/// this tool cannot do.
	///
	/// A stored code is a record that something happened once, not a diagnosis.
	/// `failing` in the status column is failing now. `km`, `count` and `last seen`
	/// are the unit's own record; `last seen` is the car's clock, which can be days
	/// off. An empty text: nothing names the code; `-v` says why, and where every
	/// text came from.
	Faults {
		#[command(flatten)]
		device: DeviceArg,
		/// Read only these control units, e.g. `01,713,70E`: short numbers or
		/// request ids, as `vagcan units` lists them. Default: every unit the car
		/// lists.
		//
		// `--ecu` was its name until 2026-10-19 and still parses, unlisted.
		#[arg(long, alias = "ecu", value_name = "UNITS")]
		unit: Option<String>,
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
		/// catalogue is sealed with one, and recovering one takes about 3 min per
		/// file — so they are kept as data, not searched for per run.
		/// Default: this project's `rod-keys.json`, written by `vagcan setup`.
		//
		// Not on `-h`: nobody reading a car needs it, and `--help` still has it.
		#[arg(long, value_name = "FILE", hide_short_help = true)]
		iv_cache: Option<String>,
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
	/// something else by them, so those are shown as `raw` bytes, the reason in the name column.
	Sensors {
		#[command(flatten)]
		device: DeviceArg,
		/// Control unit: a short number or a request id, as `vagcan units` lists
		/// them, e.g. 01 (the engine) or 713.
		//
		// `--ecu` was its name until 2026-10-19 and still parses, unlisted.
		#[arg(long, alias = "ecu", default_value = "01", value_name = "UNIT")]
		unit: String,
	},

	/// Live view of the car — configured from inside, not by flags.
	///
	/// Shows values from several control units at once: for every unit the car
	/// says it has, the channels this project describes and the scalings proven
	/// on a car. Press `c` to choose what appears; the rows nothing can name are
	/// held off the list, and `u` shows them.
	///
	/// What the units said about themselves is recorded under
	/// `~/.vagcan/cars/<VIN>/units.json`, for `setup` and `dev dash build` to
	/// work from without the car; the channels of units nothing has read yet
	/// come from the project's VCDS installation on the spot, the first time.
	Watch {
		#[command(flatten)]
		device: DeviceArg,
		/// Start with these selected, e.g. `01:2029,202A 713:1001 01:IDE00191`.
		/// Each is a control unit (short number or request id), a colon, and an
		/// identifier, a text id or `<identifier>@<bit>` — as `dash.toml` writes
		/// a channel. A bare list means the engine.
		///
		/// An identifier selects every field it carries, and one nothing
		/// describes is shown as bytes. A text id is what the Key column shows
		/// (turned on in the settings), and a pair it shows as `A/B` is both
		/// rows; the unit is what the Unit column shows.
		#[arg(long, value_name = "SPEC")]
		did: Option<String>,
		/// Poll rate for this run. Without it, the rate last set on the settings
		/// screen (10 Hz when none was ever set).
		#[arg(long, value_name = "HZ")]
		hz: Option<f64>,
		/// Also record to CSV.
		#[arg(long, value_name = "FILE")]
		out: Option<String>,
		/// Replay a recording written by `--out` instead of reading a car.
		/// No adapter is opened and nothing is addressed — for trying the
		/// interface, or showing it, away from a vehicle. The tabs are the
		/// recorded car's units (`--vin`); a recording alone does not say which
		/// unit each column came from.
		#[arg(long, value_name = "FILE", conflicts_with_all = ["device", "ble"])]
		replay: Option<String>,
		/// With --replay: the car whose recorded units give the tabs, as `vagcan
		/// info` reports it. Without it, the one car recorded on this machine;
		/// with none, or several, every catalog is offered under one tab.
		#[arg(long, value_name = "VIN", requires = "replay")]
		vin: Option<String>,
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
		//
		// Not on `-h`, as `faults --iv-cache`: a path override nobody reading a car needs.
		#[arg(long, value_name = "DIR", hide_short_help = true)]
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
	///
	/// A mark from a standstill prints as a range, its middle first — not a ±.
	/// `-v` says how to read the tables and gives the rate.
	#[cfg(feature = "measure")]
	Measure(#[command(flatten)] measure::args::Args),

	/// The workshop: build and prove the data the other commands use.
	///
	/// Nothing under here is part of reading the car for an answer. These are
	/// the tools that make the data the commands above run on — the bus
	/// sniffer, the owner's own channel names, the dash plan, and the offline
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
// and `vcds` is among the commands that want it.
#[derive(Clone, Subcommand)]
enum Dev {
	/// Watch the bus. Listen-only by default: nothing is acknowledged or sent.
	///
	/// Made to run alongside VCDS — CAN is multi-drop, so both adapters share
	/// the bus and this one records the whole conversation.
	Sniff {
		/// Adapter to use: a serial path. Omit it when only one is connected. Not `ble`:
		/// the BLE link carries no CAN frames. The dash board's cable needs `--slcan`.
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
	/// desk. None has anything to say with the car in front of you.
	Recording {
		#[command(subcommand)]
		tool: recording::Tool,
	},

	/// Build the dash device's plan for one car, or lay out its pages. Offline.
	///
	/// The OLED panel resolves nothing for itself: what it reads, how it
	/// decodes it and what it calls it are all decided here, from the car's
	/// recorded units and this project's catalogs, and written into a plan the
	/// firmware links. `edit` lays out the pages in a browser first.
	Dash {
		#[command(subcommand)]
		tool: dash::Tool,
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
///
/// Written, not `eprintln!`ed: `eprintln!` panics when stderr will not take the bytes — a
/// full disk under `2>log`, `2>/dev/full` — and the panic's own message then has nowhere to
/// go either, which ended a run with exit 101 and nothing said.
fn report(err: &anyhow::Error) {
	use std::io::Write as _;
	let _ = writeln!(std::io::stderr(), "Error: {err:?}");
}

#[tokio::main]
async fn main() -> ExitCode {
	// A binary `cargo test` built reads a temporary data dir, never ~/.vagcan: said once,
	// first, so one run by hand cannot quietly report a machine with no data.
	// Through `say!`, which never panics on a stderr that takes nothing (`report`).
	if let Some(note) = vag_cli_core::datadir::test_build_note() {
		vag_cli_core::say!("{note}");
	}
	match run().await {
		Ok(code) => code,
		// `vagcan watch … | head`: the reader had what it wanted; the run ends quietly.
		Err(err) if vag_cli_core::verbosity::is_broken_pipe(&err) => ExitCode::SUCCESS,
		Err(err) => {
			report(&err);
			ExitCode::FAILURE
		}
	}
}

/// Everything before the command, and then the command.
async fn run() -> Result<ExitCode> {
	let cli = Cli::parse();
	vag_cli_core::verbosity::set(cli.verbose.verbose);
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
		vag_cli_core::say!("{why}");
	}
	// It reads no more than `~/.vagcan/data/` and opens no adapter.
	let Some(command) = cli.command else {
		let facts = overview::gather();
		vag_cli_core::out!("{}", overview::render(&facts));
		return Ok(if overview::settled(&facts) {
			ExitCode::SUCCESS
		} else {
			ExitCode::FAILURE
		});
	};
	// `dev sniff` watches Ctrl-C itself, to stop and print what it saw.
	if matches!(command, Command::Dev { tool: Dev::Sniff { .. } }) {
		return dispatch_or_offer(command, cli.slcan).await;
	}
	tokio::select! {
		finished = dispatch_or_offer(command, cli.slcan) => finished,
		_ = tokio::signal::ctrl_c() => {
			// A spinner a blocking read is drawing cannot hear this and would be
			// left frozen mid-line, ticking on until the process ends: stop every
			// line drawing, end the one on screen, and where a key search was
			// running say what it leaves behind — every key found is saved as it
			// is found, so the next run does not search for it again.
			vag_cli_core::progress::silence();
			if std::io::stderr().is_terminal() {
				vag_cli_core::say!();
			}
			if vag_cli_core::registry::reading() {
				// Saved after the tables and after each unit — not after each key,
				// so a unit that needs two searches and was cut short between them
				// is searched for again.
				vag_cli_core::say!("interrupted: the VCDS registry keys found so far are cached; the next run goes on from them");
			}
			// The command is dropped here, and every bus handle with it: each bus task ends
			// and drops its link, and a serial link closes its channel on the way
			// (`SlcanBackend::closing_on_drop`) — which is what takes the dash board out of
			// the adapter mode `--slcan` put it in. Those tasks run on the blocking pool,
			// whose shutdown would also wait for anything else parked there, so the process
			// ends after a moment instead.
			tokio::time::sleep(INTERRUPT_GRACE).await;
			std::process::exit(130);
		}
	}
}

/// How long an interrupted command's links get to close before the process ends.
const INTERRUPT_GRACE: std::time::Duration = std::time::Duration::from_millis(300);

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
async fn dispatch_or_offer(command: Command, slcan: bool) -> Result<ExitCode> {
	// Kept before the first run consumes it: it is what "carry on with what you
	// asked for" is made of.
	let again = command.clone();
	let Err(err) = dispatch(command, slcan).await else {
		return Ok(ExitCode::SUCCESS);
	};
	// Not this shortage, or nobody at the keyboard: report it the ordinary way
	// and fail the ordinary way, which is byte for byte what happened before.
	if !rescue::worth_offering(&err) {
		return Err(err);
	}
	report(&err);
	if !rescue::offer(setup::vendor::ARCHIVE_BASE).await? {
		// The shortage above is the refusal, and it has been said once.
		return Ok(ExitCode::FAILURE);
	}
	dispatch(again, slcan).await?;
	Ok(ExitCode::SUCCESS)
}

/// The command surface: one arm per command, each handing off to the crate that
/// does the work. `slcan` is the global `--slcan`, for every command that picks a device.
async fn dispatch(command: Command, slcan: bool) -> Result<()> {
	match command {
		// Awaited on this very thread, never spawned: the folder panel it may
		// open needs the main thread (see `setup::run`).
		Command::Setup { list: true, .. } => setup::sources::list(),
		Command::Setup {
			drop: Some(selector), yes, ..
		} => setup::sources::drop_source(&selector, yes),
		Command::Setup { dir, refresh, .. } => {
			setup::run(setup::Options {
				dir: dir.as_deref(),
				refresh,
				archive_base: setup::vendor::ARCHIVE_BASE,
				// `vagcan setup` with no path asks which source, and offers the
				// download as one of the answers. Only `rescue` skips that menu.
				download: false,
			})
			.await
		}
		// Serial devices only: Bluetooth is scanned by a command given `--ble`.
		Command::Devices => {
			let found = device::list()?;
			vag_cli_core::out!("{}", device::render_list(&found));
			for (level, note) in device::list_notes(&found) {
				vag_cli_core::verbosity::say(level, &note);
			}
			Ok(())
		}
		// Every car command resolves its device inside `open`, once what it can check
		// without the car has been checked: a typo must not cost a Bluetooth scan or a menu.
		Command::Info { device } => car::info(async || device::connect(device.requested(), slcan).await).await,
		// The two depths of the same question. `--identify <unit>` names one
		// unit and reads its whole identification block; `--identify` alone
		// asks every unit the gateway lists, the gateway and the powertrain for
		// the four identifiers that name it.
		Command::Units {
			device,
			identify: Some(Some(ecu)),
			while_driving,
		} => {
			// Resolved inside `open`: the unit is parsed first.
			let open = async || {
				let path = device::resolve_cable_for(device.requested(), slcan, &IDENTIFY)?;
				device::open_bus(&Target::Serial(path)).await
			};
			car::identification(open, &ecu, while_driving).await
		}
		// `requires = "identify"` above stops `units --while-driving` at the
		// parse, but `--identify` with no unit satisfies it and lands here,
		// where the flag has nothing to lift: this arm asks each unit for the
		// four identifiers that name it, which is not a sweep and is not gated
		// on road speed. Refused rather than dropped, for the reason on the flag.
		Command::Units { while_driving: true, .. } => bail!(
			"`--while-driving` needs a unit: `--identify <unit>`. With no unit named, `units` asks each one \
             for the four identifiers that name it — that is not a sweep, and nothing about it is gated \
             on road speed."
		),
		// Resolved inside `open`, not here: `units` reads the label files first.
		Command::Units { device, identify, .. } => {
			let open = async || device::connect(device.requested(), slcan).await;
			car::units(open, identify.is_some()).await
		}
		Command::Sensors { device, unit } => car::sensors(async || device::connect(device.requested(), slcan).await, &unit).await,
		Command::Watch {
			replay: Some(path),
			data,
			vin,
			speed,
			..
		} => watch::run_recording(&path, &data_dir(data.as_deref())?, vin.as_deref(), speed).await,
		Command::Watch {
			device,
			did,
			hz,
			out,
			data,
			r#for,
			..
		} => {
			// What `units` lists, `--did` and the Unit column use: 44 as well as 712.
			labels::install_recorded_units();
			let preselect = match did.as_deref() {
				Some(spec) => plan::parse_checked(spec).map_err(|e| anyhow::anyhow!("--did: {e}"))?,
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
				async || device::connect(device.requested(), slcan).await,
				watch::Options {
					preselect: &preselect,
					hz,
					out: out.as_deref(),
					catalogs: &data_dir(data.as_deref())?,
					view,
				},
			)
			.await
		}
		#[cfg(feature = "measure")]
		Command::Measure(args) => {
			// The project is resolved only by what reads its rows: `measure view` opens a file.
			measure::dispatch(args, data_dir, async |device: Option<String>| {
				device::connect(device.as_deref(), slcan).await
			})
			.await
		}
		Command::Faults {
			device,
			unit,
			details,
			all,
			supported,
			extended,
			iv_cache,
			..
		} => {
			faults::run(
				async || device::connect(device.requested(), slcan).await,
				unit.as_deref(),
				details,
				all,
				supported,
				extended,
				iv_cache.as_deref(),
			)
			.await
		}
		Command::Dev { tool } => dispatch_dev(tool, slcan).await,
	}
}

/// The workshop group: one arm per tool under `vagcan dev`.
async fn dispatch_dev(tool: Dev, slcan: bool) -> Result<()> {
	match tool {
		Dev::Sniff {
			device,
			out,
			diag_only,
			seconds,
			active,
		} => {
			// Resolved before `--out` is created, which truncates it; the adapter is opened after.
			sniff::run(
				|| device::resolve_cable_for(device.as_deref(), slcan, &SNIFF),
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
		Dev::Dash { tool } => dash::run(tool).await,
		// `labels --from-car` is the one thing under `vcds` that touches a
		// vehicle: it reads F19E off the unit and resolves that. The group is
		// otherwise pure file work, so it hands this one case back here rather
		// than starting a runtime of its own inside a synchronous call.
		Dev::Vcds { tool } => match vcds::run(tool)? {
			vcds::Outcome::Done => Ok(()),
			vcds::Outcome::FromCar { dir, ecu, iv_cache, device } => {
				let name = car::odx_name_from_car(async || device::connect(device.as_deref(), slcan).await, &ecu).await?;
				vag_cli_core::note!("unit {ecu}  F19E  {name}");
				labels::resolve_odx(&dir, &name, &iv_cache)
			}
		},
	}
}

/// Where the proven measurement rows are for this run.
///
/// `--data` as typed if it was given, this run's project otherwise. Never a default
/// relative to the working directory: that is what made these commands work in a
/// checkout and nowhere else.
fn data_dir(given: Option<&str>) -> Result<String> {
	Ok(
		datadir::or_default(given, || Ok(project::current()?.measurements_dir()))?
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

/// `units`' `--device`: the shared help, and which of its depths needs a cable.
const UNITS_DEVICE_HELP: &str = "Adapter to use: a serial path (a USB-CAN adapter, or the dash board on its USB cable), `ble` for an adapter \
                                 over Bluetooth, or `ble:<name>` for one adapter by name. Omit it to use the one adapter or board on USB; \
                                 Bluetooth is looked for only when asked. `--identify <unit>` needs a cable adapter: it is a sweep, and a \
                                 sweep does not run through the dash board (`--slcan` makes its cable one)";

/// Why `units --identify <unit>` does not run through the dash board: it walks
/// `F100–F1FF`, a sweep. Over BLE the board refuses a sweep itself
/// (`vag_uds_client::guard`), by its eighth identifier; over its cable the safe default is
/// the same (`todo/dash/14-one-bus-three-clients.md` §8). Both are said before a session
/// starts.
const IDENTIFY: NotThroughTheBoard<'static> = NotThroughTheBoard {
	over_ble: IDENTIFY_OVER_BLE,
	over_usb: IDENTIFY_OVER_USB,
};
const IDENTIFY_OVER_BLE: &str = "`units --identify <unit>` walks F100–F1FF, a sweep, and a sweep is refused over BLE — use a cable";
const IDENTIFY_OVER_USB: &str = "`units --identify <unit>` walks F100–F1FF, a sweep, and a sweep does not run through the dash board — use a cable \
                                 adapter, or `vagcan --slcan units --identify <unit>`";

/// Why `dev sniff` needs an adapter: it reads CAN frames, and the link to the dash board
/// carries whole answers only.
const SNIFF: NotThroughTheBoard<'static> = NotThroughTheBoard {
	over_ble: SNIFF_OVER_BLE,
	over_usb: SNIFF_OVER_USB,
};
const SNIFF_OVER_BLE: &str = "`dev sniff` reads CAN frames, and the BLE link carries none — use a cable";
const SNIFF_OVER_USB: &str = "`dev sniff` reads CAN frames; through the dash board that needs `--slcan`, which makes it a plain adapter";

/// Write or refresh the owner's glossary (see the `Glossary` subcommand docs).
fn glossary_command() -> Result<()> {
	let project = project::current()?;
	let seeded = glossary::seed(&project)?;
	let language = config::language(&config::load());
	vag_cli_core::out!(
		"{}",
		vag_cli_core::table::pairs(&[
			("file", seeded.path.display().to_string()),
			("channels", seeded.total.to_string()),
			("yours", seeded.translated.to_string()),
			("blank", seeded.blank.to_string()),
			("column", language.code().to_string()),
		])
	);
	vag_cli_core::note!(
		"a name written in the `{}` column wins over ODIS and VCDS; `current` is today's name and is not read back; \
		 `language` in {} picks the column",
		language.code(),
		config::path()?.display()
	);
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
		let path = ["dev", "vcds", "analyse"];
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

	#[test]
	fn the_iv_cache_flag_is_legible_without_the_research() {
		// It used to explain itself with a `cargo run --features rod-crack`
		// invocation, which says nothing to someone holding an OBD adapter.
		let help = flag_help(&["dev", "vcds", "labels"], "iv_cache");
		assert!(!help.contains("cargo"), "{help}");
		assert!(help.contains(".rod"), "{help}");
	}

	#[test]
	fn the_dash_replay_takes_a_recording_a_car_and_presses_and_no_adapter() {
		let parse = |args: &[&str]| Cli::try_parse_from(["vagcan", "dev", "recording", "dash"].iter().chain(args).collect::<Vec<_>>());
		let parsed = parse(&["TESTVIN0000000001", "--log", "d.csv", "--press", "12.5", "--press", "20", "--speed", "2"]).unwrap();
		let Some(Command::Dev {
			tool: Dev::Recording {
				tool: recording::Tool::Dash {
					vin, log, presses, speed, ..
				},
			},
		}) = parsed.command
		else {
			panic!("not the dash replay");
		};
		assert_eq!(
			(vin.as_str(), log.as_deref(), presses, speed),
			("TESTVIN0000000001", Some("d.csv"), vec![12.5, 20.0], 2.0)
		);
		assert!(parse(&["TESTVIN0000000001", "--press", "-1"]).is_err(), "a time before the recording");
		assert!(parse(&["TESTVIN0000000001", "--press", "soon"]).is_err());
		for speed in ["0", "-1", "nan", "inf", "fast"] {
			assert!(parse(&["TESTVIN0000000001", "--speed", speed]).is_err(), "--speed {speed}");
		}
		assert!(parse(&["TESTVIN0000000001", "--speed", "0.5"]).is_ok());
		// Offline: nothing to connect to.
		assert!(parse(&["TESTVIN0000000001", "--device", "/dev/x"]).is_err());
	}

	/// The adapter a parsed command names, for the commands that take `--device`.
	fn device_of(args: &[&str]) -> Result<DeviceArg, clap::Error> {
		let command = Cli::try_parse_from(args)?.command.expect("a command");
		Ok(match command {
			Command::Info { device }
			| Command::Units { device, .. }
			| Command::Faults { device, .. }
			| Command::Sensors { device, .. }
			| Command::Watch { device, .. } => device,
			#[cfg(feature = "measure")]
			Command::Measure(args) => match args.tool {
				Some(measure::Tool::Setup { device, .. }) => device,
				_ => args.device,
			},
			Command::Dev {
				tool: Dev::Vcds {
					tool: vcds::Tool::Labels { device, .. },
				},
			} => device,
			_ => panic!("{args:?} takes no --device"),
		})
	}

	/// Every command that takes `--device` and can read through the board.
	fn commands_with_device() -> Vec<Vec<&'static str>> {
		let mut commands = vec![
			vec!["vagcan", "info"],
			vec!["vagcan", "units"],
			vec!["vagcan", "units", "--identify", "01"],
			vec!["vagcan", "faults"],
			vec!["vagcan", "sensors"],
			vec!["vagcan", "watch"],
			vec!["vagcan", "dev", "vcds", "labels", "dir", "--from-car"],
		];
		if cfg!(feature = "measure") {
			commands.extend([vec!["vagcan", "measure"], vec!["vagcan", "measure", "setup"]]);
		}
		commands
	}

	#[test]
	fn ble_is_device_ble_and_nothing_else() {
		for command in commands_with_device() {
			let spelled = |extra: &[&'static str]| {
				let args: Vec<&str> = command.iter().chain(extra).copied().collect();
				device_of(&args).unwrap_or_else(|e| panic!("{args:?}: {e}"))
			};
			assert_eq!(spelled(&["--ble"]).requested(), Some("ble"), "{command:?}");
			assert_eq!(spelled(&["--device", "ble"]).requested(), Some("ble"), "{command:?}");
			assert_eq!(spelled(&["--device", "/dev/x"]).requested(), Some("/dev/x"), "{command:?}");
			assert_eq!(spelled(&[]).requested(), None, "{command:?}");
		}
	}

	#[test]
	fn ble_and_device_together_are_refused() {
		for command in commands_with_device() {
			for device in ["/dev/x", "ble", "ble:vagcan-dash"] {
				let args: Vec<&str> = command.iter().copied().chain(["--ble", "--device", device]).collect();
				let refused = device_of(&args).expect_err("two adapters named");
				assert_eq!(refused.kind(), clap::error::ErrorKind::ArgumentConflict, "{args:?}");
			}
		}
	}

	/// `vagcan measure --ble setup` names setup's adapter, as `vagcan measure setup --ble`
	/// does: the flag is not dropped for having been written before the subcommand.
	#[cfg(feature = "measure")]
	#[tokio::test]
	async fn measure_setup_takes_the_adapter_named_before_it() {
		for (args, adapter) in [
			(&["vagcan", "measure", "--ble", "setup"][..], "ble"),
			(&["vagcan", "measure", "--device", "ble", "setup"], "ble"),
			(&["vagcan", "measure", "setup", "--ble"], "ble"),
			(&["vagcan", "measure", "--device", "/dev/x", "setup"], "/dev/x"),
		] {
			let Some(Command::Measure(parsed)) = Cli::try_parse_from(args).unwrap().command else {
				panic!("{args:?} is not measure")
			};
			let mut seen = None;
			let here = |_: Option<&str>| Ok("/definitely/not/here".to_string());
			let stopped = measure::dispatch(parsed, here, async |device| -> Result<vag_cli_core::bus::Bus> {
				seen = Some(device);
				bail!("no adapter here")
			})
			.await;
			assert!(stopped.is_err(), "{args:?}");
			assert_eq!(seen, Some(Some(adapter.to_owned())), "{args:?}");
		}
	}

	/// `measure view` opens the file it is given and nothing else, so it asks for no
	/// project: on a machine with none it used to fail before reading the file. `--data`
	/// reaches the rows' resolver, on the run and on `setup`, where it was dropped.
	#[cfg(feature = "measure")]
	#[tokio::test]
	async fn measure_asks_for_the_project_only_where_it_reads_the_rows() {
		let parse = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
			Some(Command::Measure(parsed)) => parsed,
			_ => panic!("{args:?} is not measure"),
		};
		let mut asked = Vec::new();
		let viewed = measure::dispatch(
			parse(&["vagcan", "measure", "view", "/nonexistent/session.json"]),
			|given: Option<&str>| {
				asked.push(given.map(str::to_owned));
				bail!("no project")
			},
			async |_| -> Result<vag_cli_core::bus::Bus> { bail!("no adapter here") },
		)
		.await;
		let said = format!("{:#}", viewed.expect_err("there is no such file"));
		assert!(said.contains("reading the session"), "{said}");
		assert!(asked.is_empty(), "view asked for the rows: {asked:?}");

		for (args, data) in [
			(&["vagcan", "measure", "--data", "/rows"][..], Some("/rows")),
			(&["vagcan", "measure", "setup", "--data", "/rows"], Some("/rows")),
			(&["vagcan", "measure", "--data", "/rows", "setup"], Some("/rows")),
			(&["vagcan", "measure"], None),
		] {
			let mut asked = Vec::new();
			let stopped = measure::dispatch(
				parse(args),
				|given: Option<&str>| {
					asked.push(given.map(str::to_owned));
					bail!("no project")
				},
				async |_| -> Result<vag_cli_core::bus::Bus> { bail!("no adapter here") },
			)
			.await;
			assert_eq!(format!("{:#}", stopped.expect_err("stopped")), "no project", "{args:?}");
			assert_eq!(asked, vec![data.map(str::to_owned)], "{args:?}");
		}
		let twice = measure::dispatch(
			parse(&["vagcan", "measure", "--data", "/a", "setup", "--data", "/b"]),
			|_: Option<&str>| Ok(String::new()),
			async |_| -> Result<vag_cli_core::bus::Bus> { bail!("no adapter here") },
		)
		.await;
		assert!(format!("{:#}", twice.expect_err("named twice")).contains("named twice"));
	}

	#[test]
	fn a_replay_reads_no_adapter_on_either_spelling() {
		for adapter in [&["--ble"][..], &["--device", "ble"], &["--device", "/dev/x"]] {
			let args: Vec<&str> = ["vagcan", "watch", "--replay", "drive.csv"]
				.into_iter()
				.chain(adapter.iter().copied())
				.collect();
			let refused = Cli::try_parse_from(&args).err().expect("a replay opens no adapter");
			assert_eq!(refused.kind(), clap::error::ErrorKind::ArgumentConflict, "{args:?}");
		}
	}

	/// `dev sniff` never runs over BLE, so it does not offer the flag that asks for it;
	/// `--device ble` there is still refused with the reason (below).
	#[test]
	fn a_command_that_never_runs_over_ble_does_not_offer_ble() {
		let mut cli = Cli::command();
		let sub = cli
			.find_subcommand_mut("dev")
			.expect("the workshop group exists")
			.find_subcommand_mut("sniff")
			.expect("exists");
		assert!(sub.get_arguments().any(|a| a.get_id() == "device"));
		assert!(!sub.get_arguments().any(|a| a.get_id() == "ble"));
	}

	/// `--ble` is read as the short spelling of the flag above it, so it is listed right
	/// under `--device`, with no global flag between them.
	#[test]
	fn ble_is_listed_right_under_device() {
		let mut cli = Cli::command();
		cli.build();
		let mut paths = vec![
			vec!["info"],
			vec!["units"],
			vec!["faults"],
			vec!["sensors"],
			vec!["watch"],
			vec!["dev", "vcds", "labels"],
		];
		if cfg!(feature = "measure") {
			paths.extend([vec!["measure"], vec!["measure", "setup"]]);
		}
		for path in paths {
			let sub = path.iter().fold(&mut cli, |c, name| c.find_subcommand_mut(name).expect("exists"));
			let help = sub.render_help().to_string();
			let flags: Vec<&str> = help
				.lines()
				.filter_map(|line| line.trim_start().strip_prefix("--"))
				.map(|rest| rest.split([' ', '<']).next().unwrap_or(rest))
				.collect();
			let device = flags.iter().position(|f| *f == "device").unwrap_or_else(|| panic!("{path:?}: {help}"));
			assert_eq!(flags.get(device + 1), Some(&"ble"), "{path:?}: {flags:?}");
		}
	}

	#[test]
	fn the_ble_help_says_what_it_is_short_for() {
		for path in [&["info"][..], &["faults"], &["watch"], &["units"], &["dev", "vcds", "labels"]] {
			assert_eq!(
				flag_help(path, "ble"),
				"Same as `--device ble`: the dash board over Bluetooth",
				"{path:?}"
			);
		}
		// Where a command says more about its adapter, that is kept.
		assert!(flag_help(&["units"], "device").contains("`--identify <unit>` needs a cable adapter"));
		assert!(flag_help(&["dev", "vcds", "labels"], "device").starts_with("Adapter to use with --from-car"));
	}

	#[test]
	fn a_replay_names_the_car_whose_recorded_units_give_the_tabs() {
		// A recording does not say which unit each column came from; the car's
		// record does. `--vin` picks the car, and says what happens without it.
		let help = flag_help(&["watch"], "vin");
		assert!(help.contains("recorded units"), "{help}");
		assert!(help.contains("one car recorded on this machine"), "{help}");
		// Only a replay has a car to name: live, the car names itself.
		assert!(Cli::try_parse_from(["vagcan", "watch", "--vin", "TESTVIN0000000001"]).is_err());
		assert!(Cli::try_parse_from(["vagcan", "watch", "--replay", "d.csv", "--vin", "TESTVIN0000000001"]).is_ok());
	}

	#[test]
	fn every_sweep_is_refused_on_a_moving_car() {
		// The danger moves to whichever spelling is unguarded, so the rule is
		// asserted over every one of them. `scan` used to be the unguarded one;
		// then it was `properties`, which read 256 identifiers off a unit with
		// no road-speed check at all. `scan` is gone, `properties` is now
		// `units --identify <unit>`, and the whole-car sweep `dev survey` went
		// with its command (owner, 2026-09-28) — so this is the one left.
		let cli = Cli::command();
		let sub = cli.find_subcommand("units").expect("units exists");
		assert!(
			sub.get_arguments().any(|a| a.get_id() == "while_driving"),
			"units --identify <unit> is a sweep with no --while-driving gate"
		);
	}

	#[tokio::test]
	async fn what_the_board_refuses_is_refused_here_before_anything_is_opened() {
		// Hardware-free: each is refused on the `--device` spelling alone, before a
		// scan, a port or a connection. Through the board's cable the same table is
		// `device`'s to test, with the probe handed in.
		for device in ["ble", "ble:vagcan-dash"] {
			let cases = [
				(vec!["vagcan", "units", "--identify", "01", "--device", device], IDENTIFY_OVER_BLE),
				(vec!["vagcan", "dev", "sniff", "--device", device], SNIFF_OVER_BLE),
				// A command that reads through the board is refused only for `--slcan`.
				(vec!["vagcan", "info", "--device", device], ""),
			];
			for (args, why) in cases {
				for slcan in [false, true] {
					if why.is_empty() && !slcan {
						continue;
					}
					let mut args = args.clone();
					args.extend(slcan.then_some("--slcan"));
					let cli = Cli::try_parse_from(args.clone()).unwrap();
					let refused = dispatch(cli.command.expect("a command"), cli.slcan).await.expect_err("refused");
					let why = if slcan { device::SLCAN_OVER_BLE } else { why };
					assert_eq!(refused.to_string(), why, "{args:?}");
				}
			}
		}
	}

	/// `--ble` is refused exactly where `--device ble` is, with the same words.
	#[tokio::test]
	async fn ble_is_refused_where_device_ble_is() {
		let cases = [
			(vec!["vagcan", "units", "--identify", "01", "--ble"], IDENTIFY_OVER_BLE),
			(vec!["vagcan", "units", "--identify", "01", "--ble", "--slcan"], device::SLCAN_OVER_BLE),
			(vec!["vagcan", "info", "--ble", "--slcan"], device::SLCAN_OVER_BLE),
		];
		for (args, why) in cases {
			let cli = Cli::try_parse_from(args.clone()).unwrap();
			let refused = dispatch(cli.command.expect("a command"), cli.slcan).await.expect_err("refused");
			assert_eq!(refused.to_string(), why, "{args:?}");
		}
	}

	#[test]
	fn a_sweep_refused_through_the_board_says_the_command_that_runs_it_on_a_cable() {
		assert!(
			IDENTIFY_OVER_USB.contains("`vagcan --slcan units --identify <unit>`"),
			"{IDENTIFY_OVER_USB}"
		);
		assert!(!IDENTIFY_OVER_USB.contains("on the bench"), "{IDENTIFY_OVER_USB}");
	}

	/// What a command can check without the car goes before the device is resolved: a
	/// typo must not cost a Bluetooth scan, a menu or a port. `--device ble --slcan` is the
	/// device refusal that needs no hardware, so it shows which came first.
	///
	/// `dev sniff --out` is not among them: creating the file truncates it, so the device
	/// is resolved first and the file is created before the port is opened (`sniff`'s tests).
	#[tokio::test]
	async fn arguments_are_checked_before_the_device_is_resolved() {
		const NOWHERE: &str = "/nonexistent/vagcan-test";
		let keys = format!("{NOWHERE}/rod-keys.json");
		let cases: [(Vec<&str>, &str); 3] = [
			(vec!["vagcan", "sensors", "--unit", "ZZZ"], "--unit"),
			(vec!["vagcan", "faults", "--unit", "ZZZ", "--iv-cache", &keys], ""),
			(vec!["vagcan", "units", "--identify", "ZZZ"], "ZZZ"),
		];
		for (args, why) in cases {
			let mut args = args.clone();
			args.extend(["--device", "ble", "--slcan"]);
			let cli = Cli::try_parse_from(args.clone()).unwrap();
			let refused = dispatch(cli.command.expect("a command"), cli.slcan).await.expect_err("refused");
			let said = format!("{refused:#}");
			assert_ne!(said, device::SLCAN_OVER_BLE, "the device was resolved first: {args:?}");
			assert!(said.contains(why), "{args:?}: {said}");
		}
	}

	/// A unit that does not parse is said under the flag the person typed: `units` takes
	/// its unit on `--identify`, and an error about `--unit` there names a flag not given.
	#[tokio::test]
	async fn a_unit_that_does_not_parse_is_named_by_the_flag_typed() {
		let cases = [
			(vec!["vagcan", "units", "--identify", "ZZZ"], "--identify: "),
			(vec!["vagcan", "sensors", "--unit", "ZZZ"], "--unit: "),
		];
		for (args, flag) in cases {
			let mut args = args.clone();
			// A device refused without hardware: the parse has to come first to be seen.
			args.extend(["--device", "ble", "--slcan"]);
			let cli = Cli::try_parse_from(args.clone()).unwrap();
			let refused = dispatch(cli.command.expect("a command"), cli.slcan).await.expect_err("refused");
			let said = format!("{refused:#}");
			assert!(said.starts_with(flag), "{args:?}: {said}");
		}
	}

	#[test]
	fn slcan_is_one_flag_for_every_command_wherever_it_is_written() {
		for args in [
			&["vagcan", "--slcan", "info"][..],
			&["vagcan", "info", "--slcan"],
			&["vagcan", "dev", "sniff", "--slcan"],
			&["vagcan", "units", "--identify", "01", "--slcan"],
		] {
			assert!(Cli::try_parse_from(args).unwrap().slcan, "{args:?}");
		}
		assert!(!Cli::try_parse_from(["vagcan", "info"]).unwrap().slcan);
		#[cfg(feature = "measure")]
		assert!(Cli::try_parse_from(["vagcan", "measure", "--slcan"]).unwrap().slcan);
	}

	#[test]
	fn the_tests_here_never_reach_the_data_dir_home_points_at() {
		// `arguments_are_checked_before_the_device_is_resolved` and
		// `a_unit_that_does_not_parse_is_named_by_the_flag_typed` resolve a unit from the
		// project's label cache, and opening it upgraded the reference machine's cache
		// (2026-10-20). The dev-dependency's `test-root` is what keeps them off it.
		if std::env::var_os(vag_cli_core::datadir::ROOT_ENV).is_some() {
			return;
		}
		let dir = vag_cli_core::datadir::vagcan_dir().unwrap();
		assert!(dir.starts_with(std::env::temp_dir()), "{dir:?}");
		if let Some(home) = std::env::var_os("HOME") {
			assert!(
				!dir.starts_with(std::path::Path::new(&home).join(".vagcan")),
				"{dir:?} is the person's own"
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
		// with `survey`, `sniff` and `glossary` when `dev` swallowed them, then
		// `scan` and `properties`, which were deleted outright as second
		// spellings of the sweep's `--only` and `units --identify`, and
		// `calibrate` and `survey`, deleted entirely (owner, 2026-09-28).
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
		for tool in ["sniff", "glossary", "recording", "vcds"] {
			assert!(workshop.contains(&tool), "{tool} moved to `dev` and must be there");
		}
	}

	#[test]
	fn dev_survey_and_faults_from_do_not_exist() {
		// Removed entirely (owner, 2026-09-28): the car's units are recorded by
		// `watch`, `measure` and `units --identify`, and nothing reads a survey.
		for args in [
			vec!["vagcan", "dev", "survey"],
			vec!["vagcan", "dev", "survey", "--only", "01"],
			vec!["vagcan", "faults", "--from", "s.jsonl"],
			vec!["vagcan", "watch", "--survey", "s.jsonl"],
		] {
			assert!(Cli::try_parse_from(&args).is_err(), "{args:?} parsed");
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

	/// The help of the command at `path`, the short (`-h`) or the long (`--help`) one.
	fn help_of(path: &[&str], long: bool) -> String {
		let mut cli = Cli::command();
		cli.build();
		let sub = path.iter().fold(&mut cli, |c, name| {
			c.find_subcommand_mut(name).unwrap_or_else(|| panic!("no {name} in {path:?}"))
		});
		match long {
			true => sub.render_long_help().to_string(),
			false => sub.render_help().to_string(),
		}
	}

	/// Every command path under `vagcan`, as `GLOBALS_APPLY` spells them.
	fn every_path() -> Vec<String> {
		fn walk(command: &clap::Command, path: &str, out: &mut Vec<String>) {
			for sub in command.get_subcommands().filter(|s| s.get_name() != "help") {
				let here = if path.is_empty() {
					sub.get_name().to_owned()
				} else {
					format!("{path} {}", sub.get_name())
				};
				out.push(here.clone());
				walk(sub, &here, out);
			}
		}
		let mut out = Vec::new();
		walk(&Cli::command(), "", &mut out);
		out
	}

	#[test]
	fn the_global_flags_are_shown_only_where_they_mean_something() {
		let paths = every_path();
		for (flag, applies) in GLOBALS_APPLY {
			// `measure` is a cargo feature; its rows mean nothing in a build without it.
			for path in applies.iter().filter(|p| cfg!(feature = "measure") || !p.starts_with("measure")) {
				assert!(paths.iter().any(|p| p == path), "{flag}: no command `{path}`");
			}
		}
		for path in &paths {
			let words: Vec<&str> = path.split(' ').collect();
			let help = help_of(&words, false);
			for (flag, _) in GLOBALS_APPLY {
				assert_eq!(
					help.contains(&format!("--{flag}")),
					global_applies(flag, path),
					"`vagcan {path} -h` and --{flag}:\n{help}"
				);
			}
		}
		// The noise the owner saw, named.
		for path in [&["setup"][..], &["devices"], &["dev", "vcds", "rod"]] {
			let help = help_of(path, false);
			assert!(!help.contains("--slcan") && !help.contains("--project"), "{path:?}:\n{help}");
		}
		#[cfg(feature = "measure")]
		assert!(!help_of(&["measure", "view"], false).contains("--slcan"));
		// Every command that can read through the board offers `--slcan`.
		for command in commands_with_device() {
			// The command's path: its words up to the first flag or argument.
			let path: Vec<&str> = command[1..].iter().copied().take_while(|w| !w.starts_with('-') && *w != "dir").collect();
			assert!(help_of(&path, false).contains("--slcan"), "{command:?}");
		}
		assert!(help_of(&["dev", "sniff"], false).contains("--slcan"));
		// The top level still lists both: they can be written there for any command.
		let top = Cli::command().render_help().to_string();
		assert!(top.contains("--slcan") && top.contains("--project"), "{top}");
	}

	#[test]
	fn setup_help_says_what_lands_where_and_what_to_keep() {
		// Said here since the shortage message is one line of what to type
		// (`vag_cli_core::missing::FIX`), which points at this help.
		let help = help_of(&["setup"], true);
		assert!(help.contains("~/.vagcan/data/<id>/"), "{help}");
		assert!(help.contains("keep it in place"), "a VCDS installation is read again later:\n{help}");
		assert!(help.contains("ODIS project can be deleted"), "{help}");
		assert!(help.contains(vag_cli_core::missing::VCDS_DOWNLOAD), "{help}");
		assert!(vag_cli_core::missing::FIX.contains("vagcan setup --help"));
	}

	#[test]
	fn faults_help_carries_the_caveats_the_listing_no_longer_prints() {
		let help = help_of(&["faults"], true);
		assert!(help.contains("not a diagnosis"), "{help}");
		assert!(help.contains("`failing`"), "{help}");
		assert!(help.contains("car's clock"), "{help}");
		assert!(help.contains("-v"), "{help}");
	}

	#[test]
	fn the_verbose_help_names_the_bus_only_where_a_command_talks_to_a_car() {
		let verbose = |path: &[&str]| flag_help(path, "verbose");
		for path in [&["devices"][..], &["setup"], &["dev", "glossary"], &["dev", "vcds", "rod"]] {
			let help = verbose(path);
			assert!(!help.contains("bus"), "{path:?}: {help}");
			assert!(help.contains("-v:"), "{path:?}: {help}");
		}
		for path in [&["faults"][..], &["watch"], &["units"]] {
			assert!(verbose(path).contains("every request and answer on the bus"), "{path:?}");
		}
	}

	#[test]
	fn verbose_is_counted_on_every_command_before_or_after_it_and_shown_on_each() {
		let level = |args: &[&str]| Cli::try_parse_from(args).unwrap_or_else(|e| panic!("{args:?}: {e}")).verbose.verbose;
		assert_eq!(level(&["vagcan", "faults"]), 0);
		assert_eq!(level(&["vagcan", "-v", "faults"]), 1);
		assert_eq!(level(&["vagcan", "faults", "-v"]), 1);
		assert_eq!(level(&["vagcan", "faults", "-vv"]), 2);
		assert_eq!(level(&["vagcan", "--verbose", "faults"]), 1);
		assert_eq!(level(&["vagcan", "dev", "vcds", "rod", "f.rod", "-vv"]), 2);
		assert_eq!(level(&["vagcan", "-v"]), 1, "the bare overview takes it too");
		for path in every_path() {
			let words: Vec<&str> = path.split(' ').collect();
			let help = help_of(&words, false);
			assert!(help.contains("-v, --verbose"), "`vagcan {path} -h`:\n{help}");
			let mut args = vec!["vagcan"];
			args.extend(&words);
			args.push("-vv");
			// Only the parse of the flag is asked here: a command with a required
			// argument fails for want of it, never for `-vv`.
			if let Err(e) = Cli::try_parse_from(&args) {
				assert_ne!(e.kind(), clap::error::ErrorKind::UnknownArgument, "{args:?}: {e}");
			}
		}
	}

	#[test]
	fn a_hidden_global_flag_still_parses_wherever_it_did() {
		for args in [
			&["vagcan", "setup", "--slcan", "--project", "SK37X"][..],
			&["vagcan", "--slcan", "--project", "SK37X", "setup"],
			&["vagcan", "devices", "--slcan", "--project", "SK37X"],
			&["vagcan", "dev", "--slcan", "--project", "SK37X", "sniff"],
			&["vagcan", "dev", "vcds", "rod", "f.rod", "--slcan", "--project", "SK37X"],
			&["vagcan", "units", "--slcan", "--project", "SK37X"],
		] {
			let cli = Cli::try_parse_from(args).unwrap_or_else(|e| panic!("{args:?}: {e}"));
			assert!(cli.slcan, "{args:?}");
			assert_eq!(cli.project.as_deref(), Some("SK37X"), "{args:?}");
		}
	}

	#[test]
	fn short_and_long_help_name_the_same_first_command() {
		let short = Cli::command().render_help().to_string();
		let long = Cli::command().render_long_help().to_string();
		assert!(short.contains("Start with `vagcan setup`"), "{short}");
		let start = long.split("START HERE").nth(1).expect("--help has START HERE");
		let first = start.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default();
		assert!(first.starts_with("vagcan setup"), "{first}");
	}

	#[test]
	fn a_unit_is_unit_on_every_command_and_ecu_still_parses() {
		for (path, flag) in [(&["faults"][..], "unit"), (&["sensors"], "unit")] {
			let help = help_of(path, false);
			assert!(help.contains(&format!("--{flag} <UNIT")), "{path:?}:\n{help}");
			assert!(!help.contains("--ecu"), "{path:?}:\n{help}");
		}
		let labels = help_of(&["dev", "vcds", "labels"], false);
		assert!(labels.contains("--unit <UNIT>") && !labels.contains("--ecu"), "{labels}");
		let sensors = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
			Some(Command::Sensors { unit, .. }) => unit,
			_ => panic!("not sensors"),
		};
		assert_eq!(sensors(&["vagcan", "sensors", "--ecu", "713"]), "713");
		assert_eq!(sensors(&["vagcan", "sensors", "--unit", "713"]), "713");
		let faults = |args: &[&str]| match Cli::try_parse_from(args).unwrap().command {
			Some(Command::Faults { unit, .. }) => unit,
			_ => panic!("not faults"),
		};
		assert_eq!(faults(&["vagcan", "faults", "--ecu", "01,713"]).as_deref(), Some("01,713"));
		assert!(Cli::try_parse_from(["vagcan", "dev", "vcds", "labels", "d", "--from-car", "--ecu", "02"]).is_ok());
	}

	#[test]
	fn the_help_speaks_plainly() {
		let info = help_of(&["info"], true);
		assert!(!info.contains("passport"), "{info}");
		let units = help_of(&["units"], true);
		assert!(!units.contains("installation list"), "{units}");
		assert!(units.contains("list of control units"), "{units}");
		// "Sweep" is said once in so many words, where it is done.
		let identify = flag_help(&["units"], "identify");
		assert!(
			identify.contains("That is a sweep: it reads one unit's identification block, 256 entries"),
			"{identify}"
		);
		// A path override is for `--help`, not for the first screen.
		assert!(!help_of(&["faults"], false).contains("--iv-cache"));
		assert!(help_of(&["faults"], true).contains("--iv-cache"));
		assert!(!help_of(&["watch"], false).contains("--data"));
		assert!(help_of(&["watch"], true).contains("--data"));
	}
}
