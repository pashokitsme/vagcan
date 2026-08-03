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

mod analyse;
mod calibrate;
mod datadir;
mod device;
mod discover;
mod faults;
mod labels;
mod names;
mod progress;
mod props;
mod measure;
mod recording;
mod render;
mod safety;
mod scan;
mod sniff;
mod survey;
mod units;
mod vcds;
mod vcdslog;
mod watch;

use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use vag_can::{IsoTpCan, SlcanBackend, SlcanBitrate, SlcanMode};
use vag_protocol::address::UnitAddress;
use vag_protocol::{AsyncUdsClient, UdsReadExt};

/// Serial speed to the adapter. slcan is ASCII over USB CDC, where the baud
/// rate is ignored by the hardware — no reason to make anyone choose it.
const ADAPTER_BAUD: u32 = 115_200;

#[derive(Parser)]
#[command(
    name = "vagcan",
    version,
    about = "Read a VAG car (VW / Audi / Škoda / SEAT) over CAN. \
             Wiring: OBD-II pin 6 → CAN-H, pin 14 → CAN-L, pin 5 → GND, termination OFF. \
             Start with `vagcan devices`.",
    long_about = "Read a VAG car over a USB-CAN adapter on the OBD-II port.\n\n\
                  Read-only: this tool never writes to a control unit.\n\n\
                  Wiring: OBD-II pin 6 → CAN-H, pin 14 → CAN-L, pin 5 → GND,\n\
                  and the adapter's termination jumper OFF.\n\n\
                  START HERE\n  \
                  vagcan devices            is the adapter connected?\n  \
                  vagcan info               which car is this?\n  \
                  vagcan units              which control units does it have?\n\n\
                  LOOK AT THE CAR\n  \
                  faults / properties / sensors\n\n\
                  WATCH IT LIVE\n  \
                  watch                     values from several units, chosen on screen\n  \
                  sniff                     the bus itself, listen-only\n\n\
                  FIND NEW MEASUREMENTS\n  \
                  survey --out parked.jsonl        then, after a drive:\n  \
                  survey --out driving.jsonl       then:\n  \
                  survey --diff parked.jsonl driving.jsonl   what moved = what is live\n\n\
                  AWAY FROM THE CAR\n  \
                  recording ...             read back a `watch --out` drive\n  \
                  vcds ...                  VCDS's own files: labels, names, logs\n\n\
                  Everything above this line needs a car in front of you;\n\
                  everything below it needs only files."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
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
    /// not have. Pass `--identify` to have each unit name itself.
    Units {
        /// Adapter to use. Omit it when only one is connected.
        #[arg(long, value_name = "PATH")]
        device: Option<String>,
        /// Also read each listed unit's part number and component name. Slower,
        /// and a unit that does not answer is reported as such.
        #[arg(long)]
        identify: bool,
        /// VCDS label directory. With one, each unit's part number is resolved
        /// against the corpus, which supplies the diagnostic address and the
        /// corpus's name for it — data, not a table in this program.
        #[arg(long, value_name = "DIR", requires = "identify")]
        labels: Option<String>,
    },

    /// List everything a control unit tells about itself.
    ///
    /// Sweeps the identification range and names what answers — part numbers,
    /// software versions, the ODX label file the unit is described by.
    Properties {
        /// Adapter to use. Omit it when only one is connected.
        #[arg(long, value_name = "PATH")]
        device: Option<String>,
        /// Control unit: a short number (01 engine, 02 gearbox, 09, 16, 17) or
        /// a request id (713, 70E). `vagcan units` lists this car's.
        #[arg(long, default_value = "01", value_name = "ID")]
        ecu: String,
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
    /// Shows values from several control units at once. The catalogs cover
    /// the engine, gearbox and instrument cluster; pass `--survey` to offer
    /// every identifier a survey found on any other unit as well. Press `c`
    /// to choose what appears.
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
        /// Offer everything a `vagcan survey` run found, on every unit it
        /// found it on — not just the measurements already proven.
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
        /// Where the measurement catalogs live. Each file is named after the
        /// part number or ODX name of the control unit it describes, so a car
        /// this tool has not seen before simply finds none.
        #[arg(long, default_value = vag_data::catalog::CatalogStore::DEFAULT_DIR, value_name = "DIR")]
        catalogs: String,
    },

    /// Sweep ONE control unit for every data identifier it answers.
    Scan {
        /// Adapter to use. Omit it when only one is connected.
        #[arg(long, value_name = "PATH")]
        device: Option<String>,
        /// Control unit: a short number (01 engine, 02 gearbox, 09, 16, 17) or
        /// a request id (713, 70E). `vagcan units` lists this car's.
        #[arg(long, default_value = "01", value_name = "ID")]
        ecu: String,
        /// Hex ranges to sweep, e.g. `7400-7500,A000-A100`. The default is the
        /// identification block plus two bands seen carrying live values on the
        /// reference car; `0000-FFFF` sweeps everything, slowly.
        #[arg(long, default_value = scan::DEFAULT_RANGES, value_name = "SPEC")]
        range: String,
        /// Write the answers to this file (JSON lines).
        #[arg(long, value_name = "FILE")]
        out: Option<String>,
        /// Pause between reads, in milliseconds.
        #[arg(long, default_value_t = 2, value_name = "MS")]
        delay_ms: u64,
        /// Sweep while the car is moving. Refused by default: a sweep is
        /// thousands of requests a unit may never have handled, and on the
        /// reference car it made the steering assist stop assisting mid-drive.
        #[arg(long)]
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
    },

    /// Sweep EVERY control unit the car has — `scan` for the whole car. Slow:
    /// about 8 minutes.
    ///
    /// Reads the gateway's installation list, then walks each unit: its
    /// identification block, then the identifier pages known to be in use on
    /// this car. Run it once parked and once driving — the identifiers whose
    /// bytes differ between the two runs are the live measurements, and that
    /// list needs no label file.
    Survey {
        /// Adapter to use. Omit it when only one is connected.
        #[arg(long, value_name = "PATH")]
        device: Option<String>,
        /// Hex ranges to sweep per unit.
        #[arg(long, default_value = survey::SURVEY_RANGES, value_name = "SPEC")]
        range: String,
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
        /// Compare two earlier survey files instead of reading the car, and
        /// list the identifiers whose bytes differ. Offline.
        #[arg(long, num_args = 2, value_names = ["BEFORE", "AFTER"])]
        diff: Option<Vec<String>>,
        /// Sweep while the car is moving. Refused by default: a sweep is
        /// thousands of requests a unit may never have handled, and on the
        /// reference car it made the steering assist stop assisting mid-drive.
        #[arg(long)]
        while_driving: bool,
        /// Ask each unit for an extended diagnostic session first. Off by
        /// default and refused while the car is moving: that session is
        /// workshop mode, and a unit that assists the driver may stop
        /// assisting while it is in one.
        #[arg(long)]
        extended: bool,
    },

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

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Devices => {
            println!("{}", device::render_list(&device::list()?));
            Ok(())
        }
        Command::Info { device } => info(device.as_deref()).await,
        Command::Properties { device, ecu } => properties(device.as_deref(), &ecu).await,
        Command::Units { device, identify, labels } => {
            units(device.as_deref(), identify, labels.as_deref()).await
        }
        Command::Sniff { device, out, diag_only, seconds, active } => {
            sniff::run(&device::resolve(device.as_deref())?, ADAPTER_BAUD, out.as_deref(), diag_only, seconds, active)
                .await
        }
        Command::Sensors { device, ecu } => sensors(device.as_deref(), &ecu).await,
        Command::Watch { replay: Some(path), catalogs, survey, speed, .. } => {
            watch::run_recording(
                &path,
                &datadir::resolve(&catalogs).to_string_lossy(),
                survey.as_deref(),
                speed,
            )
            .await
        }
        Command::Watch { device, did, hz, out, survey, catalogs, r#for, .. } => {
            let preselect = match did.as_deref() {
                Some(spec) => watch::plan::parse_spec(spec)
                    .map_err(|e| anyhow::anyhow!("--did: {e}"))?,
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
                    catalogs: &datadir::resolve(&catalogs).to_string_lossy(),
                    view,
                },
            )
            .await
        }
        Command::Scan { device, ecu, range, out, delay_ms, while_driving } => {
            scan::run(
                &device::resolve(device.as_deref())?,
                ADAPTER_BAUD,
                parse_ecu(&ecu)?,
                &range,
                out.as_deref(),
                delay_ms,
                while_driving,
            )
            .await
        }
        Command::Faults { device, ecu, details, all, supported, extended } => {
            faults::run(
                &device::resolve(device.as_deref())?,
                ADAPTER_BAUD,
                ecu.as_deref(),
                details,
                all,
                supported,
                extended,
            )
            .await
        }
        Command::Survey { diff: Some(files), .. } => survey::run_diff(&files[0], &files[1]),
        Command::Survey { device, range, out, delay_ms, only, extended, while_driving, .. } => {
            survey::run(
                &device::resolve(device.as_deref())?,
                ADAPTER_BAUD,
                survey::Options {
                    range: &range,
                    out: out.as_deref(),
                    delay_ms,
                    only: only.as_deref(),
                    extended,
                    while_driving,
                },
            )
            .await
        }
        Command::Recording { tool } => recording::run(tool),
        // `labels --from-car` is the one thing under `vcds` that touches a
        // vehicle: it reads F19E off the unit and resolves that. The group is
        // otherwise pure file work, so it hands this one case back here rather
        // than starting a runtime of its own inside a synchronous call.
        Command::Vcds { tool } => match vcds::run(tool)? {
            vcds::Outcome::Done => Ok(()),
            vcds::Outcome::FromCar { dir, ecu, iv_cache, device } => {
                let name = odx_name_from_car(device.as_deref(), &ecu).await?;
                println!("control unit {ecu} names its label file {name:?}\n");
                labels::resolve_odx(&dir, &name, &datadir::resolve(&iv_cache).to_string_lossy())
            }
        },
    }
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
/// applies, is decided by `vag_protocol::address`.
fn parse_ecu(text: &str) -> Result<UnitAddress> {
    vag_protocol::address::parse(text).map_err(|e| anyhow::anyhow!("--ecu: {e}"))
}

/// Open the adapter and address one control unit over UDS.
async fn open_ecu(
    device_path: &str,
    unit: UnitAddress,
) -> Result<AsyncUdsClient<IsoTpCan<vag_can::SerialSlcan>>> {
    let backend = SlcanBackend::open_mode(device_path, ADAPTER_BAUD, SlcanBitrate::Rate500k, SlcanMode::Normal)
        .await
        .with_context(|| device::open_failure(device_path))?;
    Ok(AsyncUdsClient::new(IsoTpCan::new(
        backend,
        vag_transport::CanId::Standard(unit.request),
        vag_transport::CanId::Standard(unit.response),
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
        vag_transport::CanId::Standard(gearbox_unit.request),
        vag_transport::CanId::Standard(gearbox_unit.response),
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
/// The table in `vag_data::obd` is SAE J1979's, and J1979 is only binding on
/// the emissions-related units ISO 15765-4 addresses. Every identifier that
/// answers is still shown; whether its bytes become a number is decided by
/// `obd::conversion_for`, per parameter, from the unit's block and the width of
/// what it actually answered.
async fn sensors(device_arg: Option<&str>, ecu_text: &str) -> Result<()> {
    use vag_data::obd::{self, PIDS};
    use render::SensorLine;

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
            Ok(def) => SensorLine::Converted(vag_protocol::Reading {
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
async fn units(device_arg: Option<&str>, identify: bool, labels_dir: Option<&str>) -> Result<()> {
    use vag_can::{IsoTpCan, SlcanBackend, SlcanBitrate, SlcanMode};
    use vag_protocol::gateway;
    use vag_transport::CanId;

    const GATEWAY_REQUEST: u16 = 0x710;
    const VW_RESPONSE_OFFSET: u16 = 0x6A;

    // The corpus, when one was given: it turns a part number the car reports
    // into the unit's diagnostic address and name, for any VAG car rather than
    // for a list written here.
    let corpus = match labels_dir {
        Some(dir) => Some(labels::load_cached(std::path::Path::new(dir), false)?),
        None => None,
    };

    let path = device::resolve(device_arg)?;
    let backend =
        SlcanBackend::open_mode(&path, ADAPTER_BAUD, SlcanBitrate::Rate500k, SlcanMode::Normal)
            .await
            .with_context(|| device::open_failure(&path))?;
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
        let channel = IsoTpCan::new(
            backend,
            CanId::Standard(id),
            CanId::Standard(id + VW_RESPONSE_OFFSET),
        );
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
            // what the label corpus calls the part number — the latter also
            // supplying the diagnostic address people use.
            identified += 1;
            let from_corpus = corpus
                .as_ref()
                .and_then(|db| db.unit_for_part(&part))
                .map(|u| {
                    resolved += 1;
                    format!("{:02X}  {}", u.address, u.name)
                })
                .unwrap_or_default();
            println!("  {id:03X}  {part:<14} {component:<16} {from_corpus}");
        }
        backend = unit.into_transport().into_backend();
    }
    if let Some(dir) = labels_dir {
        // Silence here would read as "the corpus agrees"; it usually means the
        // corpus has no entry for these part numbers.
        println!(
            "\n{resolved} of {identified} part numbers resolved against the corpus at {dir}."
        );
    }
    Ok(())
}

/// List a control unit's properties (see the `Properties` subcommand docs).
async fn properties(device_arg: Option<&str>, ecu_text: &str) -> Result<()> {
    let path = device::resolve(device_arg)?;
    let unit = parse_ecu(ecu_text)?;
    let ranges = scan::parse_ranges(props::IDENT_RANGE).expect("the built-in range parses");

    let mut uds = open_ecu(&path, unit).await?;
    let mut found = Vec::new();
    scan::scan_dids(&mut uds, &ranges, std::time::Duration::from_millis(2), 400, |hit| {
        found.push(props::Property { did: hit.did, data: hit.data.clone() });
        Ok(())
    })
    .await?;

    if found.is_empty() {
        println!("{}", render::render_nothing_answered());
        return Ok(());
    }
    println!("{}", props::render(&format!("Control unit {}", unit.label()), &found));

    // Mode 09 lives outside the identification block and carries what a part
    // number cannot: which emissions calibration this unit is actually
    // running.
    let mut info = Vec::new();
    for (pid, name) in vag_data::obd::VEHICLE_INFO {
        let did = vag_data::obd::did_for_info_pid(*pid);
        let Ok(data) = uds.read_data_by_identifier(did).await else {
            continue;
        };
        if let Some(items) = vag_data::obd::decode_info_text(&data) {
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
    /// path, because the offline commands live under a group now.
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
        arg.get_help().map(|h| h.to_string()).unwrap_or_default()
    }

    #[test]
    fn the_fit_flags_state_the_bar_that_is_actually_enforced() {
        // The bar is quoted twice — in this static help and in the failure
        // message the fitters print — and the numbers come from a third place,
        // `Thresholds::default()`. Loosening a threshold without updating the
        // help would leave the tool advertising a standard it no longer holds
        // itself to; this makes that a test failure.
        let bar = analyse::Thresholds::default();
        for path in [["vcds", "analyse"], ["recording", "calibrate"]] {
            for flag in ["min_r2", "min_points"] {
                let help = flag_help(&path, flag);
                assert!(help.contains(&format!("R² ≥ {:.3}", bar.min_r2)), "{path:?} {flag}: {help}");
                assert!(
                    help.contains(&format!("≥ {} points", bar.min_points)),
                    "{path:?} {flag}: {help}"
                );
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
        let help = flag_help(&["vcds", "labels"], "iv_cache");
        assert!(!help.contains("cargo"), "{help}");
        assert!(help.contains(".rod"), "{help}");
    }

    #[test]
    fn every_sweep_is_refused_on_a_moving_car() {
        // `scan` had no guard while `survey` did, for no better reason than
        // that the incident happened during a survey. They are the same
        // operation over a different number of units, and the danger simply
        // moves to whichever spelling is unguarded.
        let mut cli = Cli::command();
        for sweep in ["scan", "survey"] {
            let sub = cli.find_subcommand_mut(sweep).expect("the sweep exists");
            assert!(
                sub.get_arguments().any(|a| a.get_id() == "while_driving"),
                "{sweep} is a sweep with no --while-driving gate"
            );
        }
    }

    #[test]
    fn the_top_level_is_only_what_needs_a_car() {
        // The whole point of the `vcds` and `recording` groups: a top level
        // crowded with offline analysis cannot be scanned while standing at an
        // open driver's door. This is the rule made enforceable.
        let cli = Cli::command();
        let top: Vec<&str> = cli.get_subcommands().map(|s| s.get_name()).collect();
        for offline in ["analyse", "calibrate", "discover", "labels", "names"] {
            assert!(!top.contains(&offline), "{offline} belongs under a group, not at the top");
        }
        for live in ["info", "units", "faults", "watch", "survey", "sniff"] {
            assert!(top.contains(&live), "{live} needs a car and belongs at the top");
        }
    }
}
