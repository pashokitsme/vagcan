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
mod device;
mod labels;
mod props;
mod render;
mod scan;
mod sniff;
mod vcdslog;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use vag_can::{IsoTpCan, SlcanBackend, SlcanBitrate, SlcanMode};
use vag_protocol::{AsyncUdsClient, UdsReadExt};

/// Serial speed to the adapter. slcan is ASCII over USB CDC, where the baud
/// rate is ignored by the hardware — no reason to make anyone choose it.
const ADAPTER_BAUD: u32 = 115_200;

#[derive(Parser)]
#[command(
    name = "vagcan",
    version,
    about = "Read a VAG car (VW / Audi / Škoda / SEAT) over CAN",
    long_about = "Read a VAG car over a USB-CAN adapter on the OBD-II port.\n\n\
                  Read-only: this tool never writes to a control unit.\n\n\
                  Wiring: OBD-II pin 6 → CAN-H, pin 14 → CAN-L, pin 5 → GND,\n\
                  and the adapter's termination jumper OFF."
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

    /// List everything a control unit tells about itself.
    ///
    /// Sweeps the identification range and names what answers — part numbers,
    /// software versions, the ODX label file the unit is described by.
    Properties {
        /// Adapter to use. Omit it when only one is connected.
        #[arg(long, value_name = "PATH")]
        device: Option<String>,
        /// Control unit: 01 = engine, 02 = gearbox, and so on.
        #[arg(long, default_value = "01", value_name = "NN")]
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
        /// Join the bus normally instead of listen-only. The adapter will then
        /// acknowledge frames.
        #[arg(long)]
        active: bool,
    },

    /// Read the standard OBD-II sensors a control unit exposes.
    ///
    /// These ride the legislated parameter set mirrored at `F400 + PID`, so
    /// their conversions are public and need no reverse engineering — and five
    /// of them were independently confirmed against this car.
    Sensors {
        /// Adapter to use. Omit it when only one is connected.
        #[arg(long, value_name = "PATH")]
        device: Option<String>,
        /// Control unit: 01 = engine, 02 = gearbox, and so on.
        #[arg(long, default_value = "01", value_name = "NN")]
        ecu: String,
    },

    /// Find every data identifier a control unit answers.
    Scan {
        /// Adapter to use. Omit it when only one is connected.
        #[arg(long, value_name = "PATH")]
        device: Option<String>,
        /// Control unit: 01 = engine, 02 = gearbox, and so on.
        #[arg(long, default_value = "01", value_name = "NN")]
        ecu: String,
        /// Hex ranges to sweep, e.g. `7400-7500,A000-A100`. `0000-FFFF` sweeps
        /// everything.
        #[arg(long, default_value = scan::DEFAULT_RANGES, value_name = "SPEC")]
        range: String,
        /// Write the answers to this file (JSON lines).
        #[arg(long, value_name = "FILE")]
        out: Option<String>,
        /// Pause between reads, in milliseconds.
        #[arg(long, default_value_t = 2, value_name = "MS")]
        delay_ms: u64,
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
        /// Minimum R² for a fit to count.
        #[arg(long, default_value_t = 0.995, value_name = "R2")]
        min_r2: f64,
        /// Minimum matched samples for a fit to count.
        #[arg(long, default_value_t = 20, value_name = "N")]
        min_points: usize,
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
        Command::Sniff { device, out, diag_only, seconds, active } => {
            sniff::run(&device::resolve(device.as_deref())?, ADAPTER_BAUD, out.as_deref(), diag_only, seconds, active)
                .await
        }
        Command::Sensors { device, ecu } => sensors(device.as_deref(), &ecu).await,
        Command::Scan { device, ecu, range, out, delay_ms } => {
            scan::run(&device::resolve(device.as_deref())?, ADAPTER_BAUD, parse_ecu(&ecu)?, &range, out.as_deref(), delay_ms)
                .await
        }
        Command::Analyse { capture, log, out, min_r2, min_points } => analyse::run(
            &capture,
            &log,
            out.as_deref(),
            analyse::Thresholds { min_r2, min_points, ..Default::default() },
        ),
        Command::Labels { dir, part, block, field, odx, from_car, ecu, device } => {
            if from_car {
                let name = odx_name_from_car(device.as_deref(), &ecu).await?;
                println!("control unit {ecu} names its label file {name:?}\n");
                labels::resolve_odx(&dir, &name)
            } else if let Some(name) = odx {
                labels::resolve_odx(&dir, &name)
            } else {
                labels::labels_cmd(&dir, part.as_deref(), block, field)
            }
        }
    }
}

/// Parse an ECU address the way the VCDS world writes it: `01` is the engine,
/// which is index 0 on the wire (tester `0x7E0`, ECU `0x7E8`).
fn parse_ecu(text: &str) -> Result<u8> {
    let n: u8 = text
        .trim_start_matches('0')
        .parse()
        .with_context(|| format!("--ecu {text:?} is not a control-unit number like 01 or 02"))?;
    if n == 0 {
        anyhow::bail!("--ecu is 1-based: 01 = engine, 02 = gearbox");
    }
    Ok(n - 1)
}

/// Open the adapter and address one control unit over UDS.
async fn open_ecu(
    device_path: &str,
    ecu: u8,
) -> Result<AsyncUdsClient<IsoTpCan<vag_can::SerialSlcan>>> {
    let backend = SlcanBackend::open_mode(device_path, ADAPTER_BAUD, SlcanBitrate::Rate500k, SlcanMode::Normal)
        .await
        .with_context(|| format!("opening the adapter at {device_path}"))?;
    Ok(AsyncUdsClient::new(IsoTpCan::for_ecu(backend, ecu)))
}

/// Identify the car (see the `Info` subcommand docs).
async fn info(device_arg: Option<&str>) -> Result<()> {
    let path = device::resolve(device_arg)?;

    // One serial port, two control units: read the engine, then re-address the
    // same backend for the gearbox rather than re-opening the adapter.
    let mut engine_uds = open_ecu(&path, 0).await?;
    let engine = engine_uds.read_identity().await;

    let backend = engine_uds.into_transport().into_backend();
    let mut gearbox_uds = AsyncUdsClient::new(IsoTpCan::for_ecu(backend, 1));
    let gearbox = gearbox_uds.read_identity().await;

    if engine.is_empty() && gearbox.is_empty() {
        println!("{}", render::render_nothing_answered());
        return Ok(());
    }
    println!("{}", render::render_info(engine.vin.as_deref(), &engine, &gearbox));
    Ok(())
}

/// Read the standard OBD-II sensors (see the `Sensors` subcommand docs).
async fn sensors(device_arg: Option<&str>, ecu_text: &str) -> Result<()> {
    use vag_data::catalog::MeasurementCatalog;
    use vag_data::obd::PIDS;

    let path = device::resolve(device_arg)?;
    let mut uds = open_ecu(&path, parse_ecu(ecu_text)?).await?;

    // Ask for every standard parameter; the unit refuses the ones it does not
    // implement, and `read_catalog` skips those rather than failing the run.
    let catalog = MeasurementCatalog::new(PIDS.iter().map(|p| p.to_def()).collect());
    let readings = uds.read_catalog(&catalog).await;

    if readings.is_empty() {
        println!("{}", render::render_nothing_answered());
        return Ok(());
    }
    let width = readings.iter().map(|r| r.name.len()).max().unwrap_or(0);
    for r in &readings {
        match r.value {
            Some(v) => println!("  {:<width$}  {v:>10.2} {}", r.name, r.unit),
            // The identifier answered but the bytes did not fit the form.
            None => println!("  {:<width$}  {:>10}  (raw {})", r.name, "?", hex(&r.raw)),
        }
    }
    println!("\n{} of {} standard sensors answered", readings.len(), catalog.len());
    Ok(())
}

/// Hex for a raw response body.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
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

/// List a control unit's properties (see the `Properties` subcommand docs).
async fn properties(device_arg: Option<&str>, ecu_text: &str) -> Result<()> {
    let path = device::resolve(device_arg)?;
    let ecu = parse_ecu(ecu_text)?;
    let ranges = scan::parse_ranges(props::IDENT_RANGE).expect("the built-in range parses");

    let mut uds = open_ecu(&path, ecu).await?;
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
    println!("{}", props::render(&format!("Control unit {ecu_text}"), &found));
    Ok(())
}
