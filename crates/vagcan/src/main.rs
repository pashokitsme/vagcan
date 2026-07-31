//! `vagcan` — minimal CLI over the VAG diagnostics stack (`todo/GOAL.md`).
//!
//! Deliberately a thin, extensible command surface — no polished UI. Today it
//! carries `doctor` (live PoC #1: open the real HEX cable, run the PLAINTEXT
//! handshake, print the identity) and a documented `decode` stub.
//!
//! **Scope boundary** (`research/SCOPE-BOUNDARY.md`): `doctor` drives only the
//! plaintext open handshake via [`vag_hex::handshake`] — never the `0xb0..0xb6`
//! auth burst, never the encrypted diagnostic session. Read-only by
//! construction: it opens + identifies, no diagnostic writes.

mod labels;
mod render;
mod scan;
mod sniff;

use anyhow::Context;
use clap::{Parser, Subcommand};
use vag_hex::{D2xxBackend, HexError};

use crate::render::{render_identity, render_info};

#[derive(Parser)]
#[command(name = "vagcan", version, about = "VAG diagnostics over the HEX cable")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Open the HEX cable, run the plaintext handshake, print its identity.
    ///
    /// Read-only: probes + identifies the cable, no car/diagnostic traffic.
    Doctor {
        /// FTDI serial of the cable to open (default: first FTDI device found).
        #[arg(long)]
        serial: Option<String>,
    },
    /// Decode the link-cipher DEMO: recover a channel keystream from a known
    /// TesterPresent frame, then decode real captured `b8`/`b7` blocks to UDS.
    ///
    /// PoC #2: proves the decode pipeline on the owner's own captured car data
    /// (`reading-ecus.pcapng`). Decode-only — keystreams come from UDS
    /// known-plaintext; the AES session key is never derived (see
    /// `research/SCOPE-BOUNDARY.md`). A future revision reads captures from a file.
    Decode,
    /// Live protocol probe: open the cable, replay the plaintext bring-up burst,
    /// and report every frame the cable pushes back — flagging any RSA-OAEP
    /// wrapped-key frame (the new-build key-transport signature).
    ///
    /// Read-only observation: no diagnostic/car traffic, no decryption. Tells us
    /// whether this cable speaks the new RSA-OAEP protocol or the old scheme.
    Probe {
        /// FTDI serial of the cable to open (default: first FTDI device found).
        #[arg(long)]
        serial: Option<String>,
        /// Seconds to listen for a cable-pushed frame after the bring-up.
        #[arg(long, default_value_t = 3)]
        listen: u64,
    },
    /// Dynamic session handshake: bring-up → read the cable's live `0x39`
    /// counter → SWEEP candidate auth-completion off14 values until the cable
    /// advances past auth → `f3` TesterPresent → (if `7E`) VIN.
    ///
    /// Every transport counter (off14) is derived at runtime from the cable's
    /// observed counter — nothing is hardcoded. The auth-completion off14 rule the
    /// capture can't prove is found empirically by trying several candidates in one
    /// run. Requires ignition on. Read-only UDS. Also answers the open question:
    /// does the capture's `KS_F3` decode a session bootstrapped from only the
    /// first `b6`? A `7E` says yes; no `7E` means the key rotates per `b6` re-auth.
    Handshake {
        #[arg(long)]
        serial: Option<String>,
        /// Seconds for each post-send observation window.
        #[arg(long, default_value_t = 3)]
        listen: u64,
        /// EXPERIMENTAL: after advancing past auth, also replay the 2nd
        /// `b0..b5`+`b6` re-init to try to open the `0x9e` diagnostic ECU epoch.
        /// May fault the clone off the USB bus (physical replug). Off by default.
        #[arg(long)]
        deep: bool,
    },
    /// Session-replay diagnostic reader: replay a recorded host→cable frame
    /// sequence (from `extract_replay_stream.py`) in order to bring the cable's
    /// session up to the engine ECU's `f3` channel, then issue a UDS read (VIN).
    ///
    /// The cable is session-oriented: the engine channel only comes up after the
    /// WHOLE ordered setup sequence is replayed from a fresh power-on. Live: on
    /// the first recorded IN that the cable does not match, it reports the exact
    /// divergence index and stops. Read-only UDS. Use `--dry-run` (no hardware)
    /// to validate the stream + encode path.
    ReplayDrive {
        /// Path to the JSONL replay stream from `extract_replay_stream.py`.
        #[arg(long)]
        stream: String,
        /// Replay OUT frames up to and including this index (default: the
        /// f3-channel index detected in the stream).
        #[arg(long)]
        target_index: Option<usize>,
        /// UDS PDU (hex) to send on the engine channel once reached.
        #[arg(long, default_value = "22F190")]
        read: String,
        /// Parse the stream + exercise the encode/decode path WITHOUT opening the
        /// cable (the CI / no-hardware path).
        #[arg(long)]
        dry_run: bool,
        /// FTDI serial of the cable to open (live only; default: first found).
        #[arg(long)]
        serial: Option<String>,
        /// Per-frame receive window in ms for the live replay (default 800).
        #[arg(long, default_value_t = 800)]
        recv_window_ms: u64,
    },
    /// Read ECU IDENTIFICATION (VIN + Engine 01 and Gearbox 02 part/hw/sw
    /// numbers, component, serial, coding) over UDS-on-ISO-TP-on-CAN.
    ///
    /// Read-only (UDS service 0x22 only): no measurements, no DTCs, no writes.
    /// Needs a USB-CAN adapter on the OBD2 bus — pass `--port`. Without it, the
    /// command prints the adapter wiring notice and exits 0 (the reader logic
    /// itself is covered by unit tests, hardware-free).
    Info {
        /// Serial device of the slcan USB-CAN adapter (e.g. `/dev/tty.usbmodem…`).
        #[arg(long)]
        port: Option<String>,
        /// Serial baud rate to the adapter (slcan is ASCII over serial).
        #[arg(long, default_value_t = 115200)]
        baud: u32,
    },
    /// Watch the car's CAN bus. LISTEN-ONLY by default: the adapter receives
    /// but never drives a bit, not even an acknowledge, so it cannot disturb a
    /// bus another tester owns.
    ///
    /// The intended use is to run this **alongside VCDS** — CAN is multi-drop,
    /// so both adapters sit on the same OBD-II bus — and record VCDS's whole
    /// diagnostic conversation in the clear, including the multi-frame group
    /// reads its measurement display is built from.
    ///
    /// Wiring: OBD2 pin 6 → CAN-H, pin 14 → CAN-L, and the adapter's
    /// termination jumper OFF (the vehicle bus is already terminated).
    Sniff {
        /// Serial device of the slcan USB-CAN adapter (e.g. `/dev/cu.usbmodem…`).
        #[arg(long)]
        port: String,
        /// Serial baud rate to the adapter (slcan is ASCII over serial).
        #[arg(long, default_value_t = 115200)]
        baud: u32,
        /// Write every observed frame to this JSON-lines capture file.
        #[arg(long)]
        out: Option<String>,
        /// Keep only diagnostic ids; drop broadcast traffic from the display
        /// AND from the capture file.
        #[arg(long)]
        diag_only: bool,
        /// Stop after this many seconds (default: run until Ctrl-C).
        #[arg(long)]
        seconds: Option<u64>,
        /// Open the channel in NORMAL mode instead of listen-only. The adapter
        /// will then acknowledge frames on the bus.
        #[arg(long)]
        active: bool,
    },
    /// Ask an ECU which data identifiers it actually implements: sweep the
    /// `ReadDataByIdentifier` space and record everything that answers.
    ///
    /// Read-only (UDS service 0x22 only). VCDS reads only the identifiers its
    /// label files name; this asks the ECU itself, so it surfaces values no
    /// label mentions. Results stream to disk as they arrive — an interrupted
    /// sweep keeps what it found.
    ScanDids {
        /// Serial device of the slcan USB-CAN adapter.
        #[arg(long)]
        port: String,
        /// Serial baud rate to the adapter.
        #[arg(long, default_value_t = 115200)]
        baud: u32,
        /// ECU index: 0 = Engine (0x7E0/0x7E8), 1 = Gearbox, and so on.
        #[arg(long, default_value_t = 0)]
        ecu: u8,
        /// Comma-separated inclusive hex spans, e.g. `7400-7500,A000-A100`.
        /// Use `0000-FFFF` for everything (65,536 reads — minutes).
        #[arg(long, default_value = scan::DEFAULT_RANGES)]
        range: String,
        /// Write every answering identifier to this JSON-lines file.
        #[arg(long)]
        out: Option<String>,
        /// Pause between reads, in milliseconds.
        #[arg(long, default_value_t = 5)]
        delay_ms: u64,
    },
    /// Inventory a VCDS label/ODX directory tree and look measurements up.
    ///
    /// Recursively counts `.lbl` / `.clb` / `.rod` files under `<dir>`, then
    /// optionally resolves a part number (`--part`) to its measurements, or
    /// scans the whole corpus for a measuring block (`--block [--field]`).
    /// Read-only, offline: no car, no hardware.
    Labels {
        /// VCDS install root (or any subtree) holding `Labels/` and `UDS_EV/`.
        dir: String,
        /// Resolve this ECU part number → its label file + measurements.
        #[arg(long)]
        part: Option<String>,
        /// Cross-corpus: list every file that defines this measuring block.
        #[arg(long)]
        block: Option<u16>,
        /// Narrow `--block` to a single field (requires `--block`).
        #[arg(long, requires = "block")]
        field: Option<u8>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Doctor { serial } => doctor(serial.as_deref()).await,
        Command::Decode => {
            print!("{}", render::render_decode_demo());
            Ok(())
        }
        Command::Probe { serial, listen } => probe(serial.as_deref(), listen).await,
        Command::Handshake { serial, listen, deep } => {
            handshake(serial.as_deref(), listen, deep).await
        }
        Command::ReplayDrive {
            stream,
            target_index,
            read,
            dry_run,
            serial,
            recv_window_ms,
        } => {
            replay_drive_cmd(
                &stream,
                target_index,
                &read,
                dry_run,
                serial.as_deref(),
                recv_window_ms,
            )
            .await
        }
        Command::Info { port, baud } => info(port.as_deref(), baud).await,
        Command::Sniff {
            port,
            baud,
            out,
            diag_only,
            seconds,
            active,
        } => sniff_cmd(&port, baud, out.as_deref(), diag_only, seconds, active).await,
        Command::ScanDids {
            port,
            baud,
            ecu,
            range,
            out,
            delay_ms,
        } => scan_dids_cmd(&port, baud, ecu, &range, out.as_deref(), delay_ms).await,
        Command::Labels {
            dir,
            part,
            block,
            field,
        } => labels::labels_cmd(&dir, part.as_deref(), block, field),
    }
}

/// Sweep an ECU's data identifiers (see the `ScanDids` subcommand docs).
async fn scan_dids_cmd(
    port: &str,
    baud: u32,
    ecu: u8,
    range: &str,
    out: Option<&str>,
    delay_ms: u64,
) -> anyhow::Result<()> {
    use std::io::Write;
    use std::time::{Duration, Instant};
    use vag_can::{IsoTpCan, SlcanBackend, SlcanBitrate};
    use vag_protocol::AsyncUdsClient;

    use crate::scan::{format_hit, parse_ranges, scan_dids, total_dids, DidHit};

    let ranges = parse_ranges(range).map_err(|e| anyhow::anyhow!("--range: {e}"))?;
    let total = total_dids(&ranges);

    let mut sink: Option<std::io::BufWriter<std::fs::File>> = match out {
        Some(path) => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("creating results file {path:?}"))?;
            Some(std::io::BufWriter::new(file))
        }
        None => None,
    };

    let backend = SlcanBackend::open(port, baud, SlcanBitrate::Rate500k)
        .await
        .with_context(|| format!("opening slcan adapter at {port:?} ({baud} baud)"))?;
    let mut uds = AsyncUdsClient::new(IsoTpCan::for_ecu(backend, ecu));

    println!("scanning ECU {ecu:02} for {total} identifiers ({range}), read-only\n");
    let started = Instant::now();
    let stats = scan_dids(
        &mut uds,
        &ranges,
        Duration::from_millis(delay_ms),
        // Roughly a keep-alive every couple of seconds at the default pace.
        400,
        |hit: &DidHit| {
            println!("{}", format_hit(hit));
            if let Some(w) = sink.as_mut() {
                // The results file is JSON-lines so it can be joined against a
                // capture without a parser.
                let line = serde_json::json!({
                    "did": format!("{:04X}", hit.did),
                    "data": hit.data.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(""),
                });
                writeln!(w, "{line}")?;
            }
            Ok(())
        },
    )
    .await?;
    if let Some(w) = sink.as_mut() {
        w.flush()?;
    }

    println!(
        "\n{} of {} identifiers answered ({} refused, {} failed) in {:.1}s",
        stats.hits,
        stats.asked,
        stats.refused,
        stats.failed,
        started.elapsed().as_secs_f64()
    );
    if stats.failed == stats.asked && stats.asked > 0 {
        println!(
            "\nEvery read failed — the ECU is not answering at all. Check the ignition, the \
             wiring (OBD2 pin 6 → CAN-H, pin 14 → CAN-L), the 120R jumper being OFF, and that \
             --ecu points at an ECU this car has."
        );
    }
    Ok(())
}

/// Watch the bus (see the `Sniff` subcommand docs).
///
/// The loop polls the adapter with a short receive window rather than awaiting
/// frames inside a `select!`: the operator's Ctrl-C and marker input then reach
/// us between whole frames, and no half-consumed serial read can be cancelled.
async fn sniff_cmd(
    port: &str,
    baud: u32,
    out: Option<&str>,
    diag_only: bool,
    seconds: Option<u64>,
    active: bool,
) -> anyhow::Result<()> {
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, Instant, SystemTime};
    use vag_can::{CanBackend, CanError, SlcanBackend, SlcanBitrate, SlcanMode};

    use crate::sniff::SniffSession;

    let mode = if active { SlcanMode::Normal } else { SlcanMode::Silent };
    let mut backend = SlcanBackend::open_mode(port, baud, SlcanBitrate::Rate500k, mode)
        .await
        .with_context(|| format!("opening slcan adapter at {port:?} ({baud} baud)"))?;

    let capture: Option<Box<dyn Write>> = match out {
        Some(path) => {
            let file = std::fs::File::create(path)
                .with_context(|| format!("creating capture file {path:?}"))?;
            Some(Box::new(std::io::BufWriter::new(file)))
        }
        None => None,
    };
    let unix_us = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_micros() as u64)
        .unwrap_or(0);
    let mut session = SniffSession::new(capture, unix_us, diag_only)?;

    println!(
        "listening on {port} at 500 kbit/s in {} mode{}{}",
        if active { "NORMAL (the adapter will ACK on the bus)" } else { "LISTEN-ONLY" },
        match out {
            Some(path) => format!(", writing {path}"),
            None => String::new(),
        },
        if diag_only { ", diagnostic ids only" } else { "" },
    );
    println!("type a note + Enter to mark the capture; Ctrl-C to stop\n");

    // Ctrl-C and stdin are watched by their own tasks, so the receive loop
    // never has to be interrupted mid-frame.
    let stop = Arc::new(AtomicBool::new(false));
    let signalled = stop.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            signalled.store(true, Ordering::Relaxed);
        }
    });
    let (notes_tx, mut notes_rx) = tokio::sync::mpsc::channel::<String>(16);
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt as _;
        let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if notes_tx.send(line).await.is_err() {
                break;
            }
        }
    });

    let started = Instant::now();
    let deadline = seconds.map(|s| started + Duration::from_secs(s));
    while !stop.load(Ordering::Relaxed) {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        let ts_us = started.elapsed().as_micros() as u64;
        while let Ok(note) = notes_rx.try_recv() {
            let note = if note.trim().is_empty() { "mark".to_string() } else { note };
            session.on_marker(&note, ts_us)?;
            println!("{:9.3}  -- {note}", ts_us as f64 / 1e6);
        }

        match backend.recv_frame(Duration::from_millis(200)).await {
            Ok((id, data)) => {
                let ts_us = started.elapsed().as_micros() as u64;
                if let Some(line) = session.on_frame(id, &data, ts_us)? {
                    println!("{line}");
                }
            }
            // A quiet window is normal — the bus may simply be idle.
            Err(CanError::Timeout) => {}
            Err(CanError::MalformedFrame(what)) => eprintln!("skipped: {what}"),
            Err(e) => {
                eprintln!("receive failed: {e}");
                break;
            }
        }
    }

    let _ = backend.close_channel().await;
    let stats = session.stats();
    session.finish()?;
    let elapsed = started.elapsed().as_secs_f64();
    println!(
        "\nstopped after {elapsed:.1}s: {} frames seen, {} written, {} PDUs reassembled, \
         {} incomplete, {} markers",
        stats.frames_seen, stats.frames_kept, stats.pdus, stats.dropped, stats.markers
    );
    if stats.frames_seen == 0 {
        println!(
            "\nNo traffic at all. Check: ignition on; OBD2 pin 6 → CAN-H and pin 14 → CAN-L; \
             the adapter's 120R termination jumper OFF; the bus really running at 500 kbit/s."
        );
    }
    Ok(())
}

/// Read the identification block from the Engine (01) and Gearbox (02) over a
/// generic USB-CAN adapter and print the `vagcan info` report.
///
/// A serial port is a single channel and [`IsoTpCan`] owns the backend, so we
/// read the Engine fully, recover the backend with `into_backend()`, then
/// re-address it for the Gearbox — one port, two ECUs, no re-open.
async fn info(port: Option<&str>, baud: u32) -> anyhow::Result<()> {
    use vag_can::{IsoTpCan, SlcanBackend, SlcanBitrate};
    use vag_protocol::{AsyncUdsClient, UdsReadExt};

    let Some(port) = port else {
        println!(
            "vagcan info reads ECU identification live over a USB-CAN adapter — none selected.\n\
             \n\
             To read your car:\n  \
             - adapter: MKS CANable (or any slcan/LAWICEL USB-CAN), 500 kbit/s\n  \
             - wiring:  OBD2 pin 6 → CAN-H, pin 14 → CAN-L, termination OFF\n  \
             - then:    vagcan info --port <tty> [--baud {baud}]\n\
             \n\
             (The identity reader itself is covered by hardware-free unit tests.)"
        );
        return Ok(());
    };

    // Engine (ECU index 0 → tester 0x7E0 / ECU 0x7E8).
    let backend = SlcanBackend::open(port, baud, SlcanBitrate::Rate500k)
        .await
        .with_context(|| format!("opening slcan adapter at {port:?} ({baud} baud)"))?;
    let mut engine_uds = AsyncUdsClient::new(IsoTpCan::for_ecu(backend, 0));
    let engine = engine_uds.read_identity().await;

    // Recover the single serial channel and re-address it for the Gearbox
    // (ECU index 1 → tester 0x7E1 / ECU 0x7E9).
    let backend = engine_uds.into_transport().into_backend();
    let mut gearbox_uds = AsyncUdsClient::new(IsoTpCan::for_ecu(backend, 1));
    let gearbox = gearbox_uds.read_identity().await;

    // VIN is a global identifier; take it from the Engine's F190.
    println!("{}", render_info(engine.vin.as_deref(), &engine, &gearbox));
    Ok(())
}

/// Session-replay diagnostic reader (see the `ReplayDrive` subcommand docs).
async fn replay_drive_cmd(
    stream_path: &str,
    target_index: Option<usize>,
    read: &str,
    dry_run: bool,
    serial: Option<&str>,
    recv_window_ms: u64,
) -> anyhow::Result<()> {
    use std::time::Duration;

    let text = std::fs::read_to_string(stream_path)
        .with_context(|| format!("cannot read replay stream {stream_path:?}"))?;
    let frames = vag_hex::parse_stream(&text).context("parsing replay stream")?;
    let read_pdu = vag_hex::parse_hex(read).context("parsing --read PDU hex")?;

    // Resolve the target index: the flag, else the detected f3-channel index.
    let target = match target_index {
        Some(n) => n,
        None => vag_hex::f3_channel_index(&frames).context(
            "no --target-index given and no f3 engine channel found in the stream (is this the \
             right capture?)",
        )?,
    };

    println!(
        "replay stream {stream_path:?}: {} frames ({} OUT); target index {target}; read PDU {}",
        frames.len(),
        frames.iter().filter(|f| matches!(f.dir, vag_hex::Dir::Out)).count(),
        read_pdu.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
    );

    if dry_run {
        let plan = vag_hex::plan_dry_run(&frames, target, &read_pdu).context("dry-run plan")?;
        println!("\n--- DRY RUN (no hardware) ---");
        println!(
            "  would re-send {} OUT frame(s) and compare {} IN frame(s) up to target idx {}",
            plan.out_up_to_target, plan.in_up_to_target, plan.target_index
        );
        println!(
            "  would send f3 read {} with off14={:#04x}",
            plan.read_pdu.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
            plan.read_off14
        );
        println!(
            "  encoded f3 block: {}",
            plan.encoded_read.iter().map(|b| format!("{b:02x}")).collect::<String>()
        );
        println!(
            "  encode→decode round-trip: {} (decoded {})",
            if plan.round_trip_ok { "OK" } else { "FAILED" },
            plan.decoded_read_pdu.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
        );
        anyhow::ensure!(plan.round_trip_ok, "read PDU did not round-trip through the f3 codec");
        return Ok(());
    }

    // Live path: open the cable and drive the replay.
    let mut backend = D2xxBackend::open(serial).map_err(|e| open_diagnostic(serial, e))?;
    let mut transport =
        vag_hex::CableTransport::new(&mut backend, Duration::from_millis(recv_window_ms));
    let report = vag_hex::replay_drive(&mut transport, &frames, target, &read_pdu)
        .await
        .context("replay drive failed")?;

    println!("\n--- replay drive log ---");
    for line in &report.log {
        println!("  {line}");
    }
    println!("\nre-sent {} OUT frame(s).", report.sent_out);

    if let Some(d) = &report.divergence {
        println!(
            "\n❌ DIVERGENCE at idx {}: the live cable left the recording here.",
            d.idx
        );
        println!("   expected: {}", d.expected.iter().map(|b| format!("{b:02x}")).collect::<String>());
        println!("   observed: {}", d.observed.iter().map(|b| format!("{b:02x}")).collect::<String>());
        println!(
            "   (a verbatim replay only stays in sync if the cable reproduces the recorded \
             responses byte-for-byte from a fresh power-on.)"
        );
        return Ok(());
    }

    println!(
        "\n✅ reached target idx {} — sent f3 read off14={:#04x}.",
        report.target_index, report.sent_read_off14
    );
    match &report.response_pdu {
        Some(pdu) => println!(
            "f3 response PDU: {}",
            pdu.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
        ),
        None => println!("no f3 response decoded (cable went quiet)."),
    }
    match &report.vin {
        Some(v) if v.len() == 17 => println!("\n✅ VIN: {v}"),
        Some(v) => println!("\n⚠️  VIN reassembled but length {} != 17: {v:?}", v.len()),
        None => {}
    }
    Ok(())
}

/// Dynamic session handshake: advance past the 0x39 auth-stall with a
/// runtime-derived counter, then TesterPresent + VIN on the f3 channel.
async fn handshake(serial: Option<&str>, listen_secs: u64, deep: bool) -> anyhow::Result<()> {
    use std::time::Duration;

    let mut backend = D2xxBackend::open(serial).map_err(|e| open_diagnostic(serial, e))?;
    let dur = Duration::from_secs(listen_secs);
    let report = if deep {
        vag_hex::drive_session_deep(&mut backend, dur).await
    } else {
        vag_hex::drive_session_sweep(&mut backend, dur).await
    }
    .context("session drive failed")?;

    println!("--- session drive log ---");
    for line in &report.log {
        println!("  {line}");
    }
    println!(
        "\ncable sent {} frame(s) total. observed 0x39 counter(s): {:02x?}",
        report.received.len(),
        report.observed_auth_off14
    );
    println!(
        "sent auth-completion b8 off14={:#04x}",
        report.sent_auth_off14
    );

    if report.advanced {
        println!(
            "\n✅ ADVANCED: the cable emitted a non-0x39 channel. Winning auth-completion \
             off14={:#04x}.",
            report.sent_auth_off14
        );
    } else {
        println!(
            "\n❌ STUCK: none of the swept auth-completion off14 candidates made the cable leave \
             the 0x39 channel. The auth-completion may need more than a restamped counter (a \
             challenge-derived payload), or a different advance trigger. The observed 0x39 \
             counters are logged above."
        );
    }

    // Channel (block off0) distribution across every diagnostic b7 the cable
    // sent — shows which channel(s) we reached after leaving 0x39.
    use std::collections::BTreeMap;
    let mut chans: BTreeMap<u8, usize> = BTreeMap::new();
    for f in &report.received {
        if f.opcode == vag_hex::frame::OP_DIAG_RESP && f.data.len() >= 16 {
            *chans.entry(f.data[0]).or_default() += 1;
        }
    }
    println!(
        "\nb7 channels seen (block off0 → count): {}",
        chans
            .iter()
            .map(|(c, n)| format!("{c:#04x}×{n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    if report.tp_positive {
        println!("✅ f3 TesterPresent POSITIVE (7E) decoded with the capture's KS_F3.");
    } else if chans.keys().any(|&c| c == 0xF3) {
        println!(
            "⚠️  f3 blocks present but no 7E — KS_F3 may not match this session's f3 epoch. \
             Decoded f3 blocks (off6..15):"
        );
        for (i, b) in report.f3_decoded_blocks.iter().enumerate() {
            let region: String = b[6..].iter().map(|x| format!("{x:02x} ")).collect();
            println!("  [{i:2}] {}", region.trim_end());
        }
    } else {
        println!(
            "ℹ️  Advanced past auth but the f3 (engine) channel is NOT open yet — it opens deep \
             in VCDS's scan. We reached the channel(s) above; next we drive their per-ECU open \
             toward a channel that answers ReadDataByIdentifier F1 90 (VIN)."
        );
    }

    match &report.vin {
        Some(v) if v.len() == 17 => println!("\n✅ VIN: {v}"),
        Some(v) => println!("\n⚠️  VIN reassembled but length {} != 17: {v:?}", v.len()),
        _ => {}
    }
    Ok(())
}

/// Live protocol probe: replay the bring-up and report what the cable pushes.
async fn probe(serial: Option<&str>, listen_secs: u64) -> anyhow::Result<()> {
    use std::time::Duration;

    let mut backend = D2xxBackend::open(serial).map_err(|e| open_diagnostic(serial, e))?;
    let report = vag_hex::probe_open(&mut backend, Duration::from_secs(listen_secs))
        .await
        .context("probe failed while driving the bring-up sequence")?;

    println!(
        "cable sent {} raw byte(s), {} parsed frame(s) after bring-up:",
        report.raw_bytes.len(),
        report.received.len()
    );
    if report.received.is_empty() && !report.raw_bytes.is_empty() {
        println!(
            "  raw (unframed): {}",
            report.raw_bytes.iter().map(|b| format!("{b:02x} ")).collect::<String>().trim_end()
        );
    }
    for f in &report.received {
        let preview: String = f
            .data
            .iter()
            .take(16)
            .map(|b| format!("{b:02x} "))
            .collect();
        let ell = if f.data.len() > 16 { "…" } else { "" };
        println!(
            "  op={:#04x} len={:3}  {}{}",
            f.opcode,
            f.data.len(),
            preview.trim_end(),
            ell
        );
    }

    match &report.wrapped_key {
        Some(wk) => {
            println!(
                "\n✅ NEW protocol: cable pushed a {}-byte wrapped-key frame (op={:#04x}).",
                wk.data.len(),
                wk.opcode
            );
            println!("   RSA-OAEP decrypt with the embedded key → session key → UDS is the path.");
            println!("   wrapped blob (full hex):");
            println!("   {}", wk.data.iter().map(|b| format!("{b:02x}")).collect::<String>());
        }
        None => {
            println!(
                "\n❌ No wrapped-key frame (nothing ≥100 bytes). This cable likely speaks the OLD \
                 b6/b7-derived scheme, not the new RSA-OAEP key transport."
            );
        }
    }
    Ok(())
}

/// Live PoC #1: open the cable, run the plaintext handshake, print identity.
async fn doctor(serial: Option<&str>) -> anyhow::Result<()> {
    let backend = D2xxBackend::open(serial).map_err(|e| open_diagnostic(serial, e))?;
    let handle = vag_hex::spawn(backend);
    let identity = vag_hex::handshake(&handle)
        .await
        .context("handshake failed: cable opened but did not answer the plaintext probe/identify")?;
    println!("{}", render_identity(&identity));
    Ok(())
}

/// Turn a cable-open failure into a diagnostic that says what to check.
fn open_diagnostic(serial: Option<&str>, e: HexError) -> anyhow::Error {
    let target = match serial {
        Some(s) => format!("cable with FTDI serial {s:?}"),
        None => "any FTDI cable".to_string(),
    };
    anyhow::Error::new(e).context(format!(
        "cable not found / failed to open {target} — is the HEX cable plugged in \
         (and no other program holding it)?"
    ))
}
