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
mod render;
mod safety;
mod scan;
mod sniff;
mod survey;
mod vcdslog;
mod watch;

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
                  survey --diff parked.jsonl driving.jsonl   what moved = what is live\n  \
                  watch --out drive.csv            then: discover --log drive.csv\n  \
                  calibrate --log drive.csv        prove a scaling against a known one\n  \
                  sniff --out cap.jsonl beside VCDS, then analyse --capture cap.jsonl\n\n\
                  OFFLINE REFERENCE\n  \
                  names / labels"
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

    /// Cross a capture with a VCDS log to prove measurement scalings.
    ///
    /// Offline — no car. Aligns the two by their wall-clock stamps and fits
    /// raw bytes to displayed values, reporting only what clears the bar.
    Analyse {
        /// Capture written by `vagcan sniff`.
        #[arg(long, value_name = "FILE")]
        capture: String,
        /// VCDS measuring-blocks CSV export recorded at the same time.
        #[arg(long, value_name = "FILE")]
        log: String,
        /// Write the proven scalings as a measurement catalog.
        #[arg(long, value_name = "FILE")]
        out: Option<String>,
        /// Minimum R² for a fit to count (the whole bar: R² ≥ 0.995, ≥ 20
        /// points over ≥ 4 distinct raw values).
        #[arg(long, default_value_t = 0.995, value_name = "R2")]
        min_r2: f64,
        /// Minimum matched samples for a fit to count (the whole bar: R² ≥
        /// 0.995, ≥ 20 points over ≥ 4 distinct raw values).
        #[arg(long, default_value_t = 20, value_name = "N")]
        min_points: usize,
    },

    /// Find which identifiers carry discrete state — gear, mode, a switch.
    ///
    /// Offline — no car. Reads a `vagcan watch --out` recording and sorts every
    /// identifier by how it behaved: never moved, stepped between a few values,
    /// or varied continuously. Gear and switches cannot be found by fitting a
    /// line; they are found by noticing what changed when.
    Discover {
        /// Recording written by `vagcan watch --out`.
        #[arg(long, value_name = "FILE")]
        log: String,
        /// Also list identifiers that changed at the same moments.
        #[arg(long)]
        pairs: bool,
    },

    /// Prove new scalings against ones already trusted — no VCDS needed.
    ///
    /// Offline. Reads a `watch --out` recording that contains BOTH converted
    /// reference columns and raw hex columns, and fits each unknown against
    /// each reference. One clock, so no alignment error is possible. Cannot
    /// name anything, and cannot find a quantity unrelated to everything
    /// already known.
    Calibrate {
        /// Recording written by `vagcan watch --out`.
        #[arg(long, value_name = "FILE")]
        log: String,
        /// Minimum R² for a fit to count (the whole bar: R² ≥ 0.995, ≥ 20
        /// points over ≥ 4 distinct raw values).
        #[arg(long, default_value_t = 0.995, value_name = "R2")]
        min_r2: f64,
        /// Minimum matched samples for a fit to count (the whole bar: R² ≥
        /// 0.995, ≥ 20 points over ≥ 4 distinct raw values).
        #[arg(long, default_value_t = 20, value_name = "N")]
        min_points: usize,
    },

    /// Search the measurement names recovered from the label corpus.
    ///
    /// Offline. The names are keyed by the corpus's own text id, not by data
    /// identifier — that join does not exist in the label files — so a match
    /// is a hypothesis to test on the car, not an identification.
    Names {
        /// Substring to look for, case-insensitive.
        #[arg(value_name = "TEXT")]
        text: String,
        /// Stop after this many matches.
        #[arg(long, default_value_t = 40, value_name = "N")]
        limit: usize,
        /// Names file to search. Recovered from a VCDS installation, so a
        /// different installation means a different file.
        #[arg(long, default_value = names::DEFAULT_PATH, value_name = "FILE")]
        catalog: String,
    },

    /// Look measurements up in a VCDS label directory. Offline — no car.
    Labels {
        /// VCDS install root, or any directory below it.
        #[arg(value_name = "DIR")]
        dir: String,
        /// Resolve a part number to its label file and measurements.
        #[arg(long, value_name = "PART")]
        part: Option<String>,
        /// List every file defining this measuring block.
        #[arg(long, value_name = "N")]
        block: Option<u16>,
        /// Narrow --block to one field.
        #[arg(long, requires = "block", value_name = "N")]
        field: Option<u8>,
        /// Resolve the ODX file a control unit names for itself, e.g.
        /// `EV_ECM18TFS0208V0906264H` — the value of identifier F19E, which
        /// `vagcan properties` reads off the car.
        #[arg(long, value_name = "NAME")]
        odx: Option<String>,
        /// Read F19E from the car and resolve that, instead of passing --odx.
        #[arg(long, conflicts_with = "odx")]
        from_car: bool,
        /// Control unit to ask when using --from-car.
        #[arg(long, default_value = "01", value_name = "NN")]
        ecu: String,
        /// Rebuild the label cache even if it looks current.
        #[arg(long)]
        refresh: bool,
        /// Where the keys for reading encrypted .rod label files are cached.
        ///
        /// VW ships .rod files with their contents encrypted, and the key for a
        /// section has to be recovered by a separate, slow tool before that
        /// section can be read. A section with no key here is reported as
        /// unreadable rather than guessed, and the command that recovers one is
        /// printed against the section that needs it.
        #[arg(long, default_value = "catalogs/rod-iv-cache.json", value_name = "FILE")]
        iv_cache: String,
        /// Adapter to use with --from-car.
        #[arg(long, value_name = "PATH")]
        device: Option<String>,
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
        Command::Watch { device, did, hz, out, survey, catalogs, .. } => {
            let preselect = match did.as_deref() {
                Some(spec) => watch::plan::parse_spec(spec)
                    .map_err(|e| anyhow::anyhow!("--did: {e}"))?,
                None => Vec::new(),
            };
            watch::run(
                &device::resolve(device.as_deref())?,
                ADAPTER_BAUD,
                &preselect,
                hz,
                out.as_deref(),
                survey.as_deref(),
                &datadir::resolve(&catalogs).to_string_lossy(),
            )
            .await
        }
        Command::Scan { device, ecu, range, out, delay_ms } => {
            scan::run(&device::resolve(device.as_deref())?, ADAPTER_BAUD, parse_ecu(&ecu)?, &range, out.as_deref(), delay_ms)
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
        Command::Names { text, limit, catalog } => {
            names::run(&text, limit, &datadir::resolve(&catalog).to_string_lossy())
        }
        Command::Calibrate { log, min_r2, min_points } => calibrate::run(
            &log,
            analyse::Thresholds { min_r2, min_points, ..Default::default() },
        ),
        Command::Analyse { capture, log, out, min_r2, min_points } => analyse::run(
            &capture,
            &log,
            out.as_deref(),
            analyse::Thresholds { min_r2, min_points, ..Default::default() },
        ),
        Command::Discover { log, pairs } => {
            let text = std::fs::read_to_string(&log)
                .with_context(|| format!("reading the recording {log:?}"))?;
            let columns = discover::classify(&text).map_err(|e| anyhow::anyhow!("{log}: {e}"))?;
            print!("{}", discover::render(&columns));
            if pairs {
                let together = discover::co_changing(&columns, 0.5);
                if together.is_empty() {
                    println!("\nNo two candidates changed together.");
                } else {
                    println!("\nChanged at the same moments — probably one thing seen twice:");
                    for (a, b, overlap) in together {
                        println!("  {a} + {b}   {:.0}% of transitions coincide", overlap * 100.0);
                    }
                }
            }
            Ok(())
        }
        Command::Labels {
            dir,
            part,
            block,
            field,
            odx,
            from_car,
            ecu,
            device,
            iv_cache,
            refresh,
        } => {
            if from_car {
                // The corpus is checked before the adapter is opened: reading
                // F19E off the car and only then discovering that the label
                // directory does not exist costs the port for nothing.
                if !std::path::Path::new(&dir).is_dir() {
                    anyhow::bail!(
                        "{dir:?} is not a directory — point it at the VCDS install root"
                    );
                }
                let name = odx_name_from_car(device.as_deref(), &ecu).await?;
                println!("control unit {ecu} names its label file {name:?}\n");
                labels::resolve_odx(&dir, &name, &datadir::resolve(&iv_cache).to_string_lossy())
            } else if let Some(name) = odx {
                labels::resolve_odx(&dir, &name, &datadir::resolve(&iv_cache).to_string_lossy())
            } else {
                labels::labels_cmd(&dir, part.as_deref(), block, field, refresh)
            }
        }
    }
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

    /// The help for one flag of one subcommand.
    fn flag_help(command: &str, flag: &str) -> String {
        let mut cli = Cli::command();
        let sub = cli.find_subcommand_mut(command).expect("the subcommand exists");
        let arg = sub
            .get_arguments()
            .find(|a| a.get_id() == flag)
            .unwrap_or_else(|| panic!("{command} has no {flag}"));
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
        for command in ["analyse", "calibrate"] {
            for flag in ["min_r2", "min_points"] {
                let help = flag_help(command, flag);
                assert!(help.contains(&format!("R² ≥ {:.3}", bar.min_r2)), "{command} {flag}: {help}");
                assert!(
                    help.contains(&format!("≥ {} points", bar.min_points)),
                    "{command} {flag}: {help}"
                );
                assert!(
                    help.contains(&format!("≥ {} distinct raw values", bar.min_levels)),
                    "{command} {flag}: {help}"
                );
            }
        }
    }

    #[test]
    fn the_iv_cache_flag_is_legible_without_the_research() {
        // It used to explain itself with a `cargo run --features rod-crack`
        // invocation, which says nothing to someone holding an OBD adapter.
        let help = flag_help("labels", "iv_cache");
        assert!(!help.contains("cargo"), "{help}");
        assert!(help.contains(".rod"), "{help}");
    }
}
