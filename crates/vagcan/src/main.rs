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

mod render;

use anyhow::Context;
use clap::{Parser, Subcommand};
use vag_hex::{D2xxBackend, HexError};

use crate::render::render_identity;

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
    /// Session key-match test: replay the bring-up + the captured TesterPresent
    /// `b8` block, then decode the cable's `b7` replies with the recovered
    /// `KS_F3` keystream. A `7E 00` decode proves the live session key equals the
    /// capture's — i.e. the recovered keystreams drive this cable live.
    ///
    /// Requires ignition on. Read-only (TesterPresent is a keepalive no-op).
    Session {
        #[arg(long)]
        serial: Option<String>,
        #[arg(long, default_value_t = 3)]
        listen: u64,
    },
    /// Live VIN read: bring-up → post-auth choreography → craft and send the
    /// encoded ReadDataByIdentifier F1 90 request on the engine (f3) channel →
    /// reassemble the multiframe response → print the VIN.
    ///
    /// Requires ignition on. Read-only UDS (RDBI F1 90 reads the VIN only). This
    /// is the end-to-end hardware experiment; if no VIN comes back it prints the
    /// decoded response blocks so the session state is diagnosable.
    Vin {
        #[arg(long)]
        serial: Option<String>,
        /// Seconds to listen for the (multiframe) response after the request.
        #[arg(long, default_value_t = 4)]
        listen: u64,
    },
    /// Dynamic session handshake: bring-up → read the cable's live `0x39`
    /// counter → send the auth-completion `b8` with a dynamically-derived off14 →
    /// watch the cable advance past auth → `f3` TesterPresent → (if `7E`) VIN.
    ///
    /// Unlike `vin`, every transport counter (off14) is derived at runtime from
    /// the cable's observed counter (`paired_off14` = flip bit0) — nothing is
    /// hardcoded. Requires ignition on. Read-only UDS. This answers the open
    /// question: does the capture's `KS_F3` decode a session we bootstrap with
    /// only the first `b6`? A `7E` says yes; no `7E` means the key rotates per
    /// `b6` re-auth.
    Handshake {
        #[arg(long)]
        serial: Option<String>,
        /// Seconds for each post-send observation window.
        #[arg(long, default_value_t = 3)]
        listen: u64,
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
        Command::Session { serial, listen } => session(serial.as_deref(), listen).await,
        Command::Vin { serial, listen } => vin(serial.as_deref(), listen).await,
        Command::Handshake { serial, listen } => handshake(serial.as_deref(), listen).await,
    }
}

/// Dynamic session handshake: advance past the 0x39 auth-stall with a
/// runtime-derived counter, then TesterPresent + VIN on the f3 channel.
async fn handshake(serial: Option<&str>, listen_secs: u64) -> anyhow::Result<()> {
    use std::time::Duration;

    let mut backend = D2xxBackend::open(serial).map_err(|e| open_diagnostic(serial, e))?;
    let report = vag_hex::drive_session(&mut backend, Duration::from_secs(listen_secs))
        .await
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
        println!("\n✅ ADVANCED: the cable emitted a non-0x39 channel after the auth-completion.");
    } else {
        println!(
            "\n❌ STUCK: the cable kept repeating the 0x39 block (no other channel seen). \
             The auth-completion off14 may be wrong, or this build needs the RSA-OAEP key \
             push first (see `vagcan probe`)."
        );
    }

    if report.tp_positive {
        println!("✅ f3 TesterPresent POSITIVE (7E) decoded with the capture's KS_F3.");
    } else {
        println!("⚠️  No f3 7E. See the decoded f3 blocks below (off6..15):");
    }
    for (i, b) in report.f3_decoded_blocks.iter().enumerate() {
        let region: String = b[6..].iter().map(|x| format!("{x:02x} ")).collect();
        println!("  [{i:2}] {}", region.trim_end());
    }

    match &report.vin {
        Some(v) if v.len() == 17 => println!("\n✅ VIN: {v}"),
        Some(v) => println!("\n⚠️  VIN reassembled but length {} != 17: {v:?}", v.len()),
        None if report.tp_positive => println!("\n⚠️  TP was positive but no VIN reassembled."),
        None => println!(
            "\nOPEN QUESTION: no 7E means the capture's KS_F3 likely does NOT decode a session \
             bootstrapped from only the first b6 — the keystream probably rotates per b6 \
             re-auth. Next step: replay the capture's b6 re-auth nonces in sequence."
        ),
    }
    Ok(())
}

/// Live VIN read on the engine (f3) channel.
async fn vin(serial: Option<&str>, listen_secs: u64) -> anyhow::Result<()> {
    use std::time::Duration;

    let mut backend = D2xxBackend::open(serial).map_err(|e| open_diagnostic(serial, e))?;
    let report = vag_hex::vin_read(&mut backend, Duration::from_secs(listen_secs))
        .await
        .context("VIN read failed while driving the session")?;

    match &report.vin {
        Some(v) if v.len() == 17 => {
            println!("VIN: {v}");
            println!("\n✅ Read the VIN live over the cable (RDBI F1 90, engine channel).");
        }
        Some(v) => {
            println!("VIN (unexpected length {}): {v:?}", v.len());
            println!("A 62 F1 90 reply reassembled but was not 17 chars — check the bytes below.");
        }
        None => {
            println!(
                "No VIN reassembled. Decoded {} f3 response block(s) below (off6..15):",
                report.decoded_blocks.len()
            );
            for (i, b) in report.decoded_blocks.iter().enumerate() {
                let region: String = b[6..].iter().map(|x| format!("{x:02x} ")).collect();
                println!("  [{i:2}] {}", region.trim_end());
            }
            println!(
                "\n⚠️  If there are no f3 blocks at all, the cable did not reach the engine \
                 diagnostic state from the replayed choreography (this build may require the \
                 cable to key the link first — see `vagcan probe`). If blocks decoded but no \
                 62 F1 90, the ECU answered something else; inspect the bytes above."
            );
        }
    }
    Ok(())
}

/// Replay bring-up + TesterPresent, decode `b7` replies with `KS_F3`.
async fn session(serial: Option<&str>, listen_secs: u64) -> anyhow::Result<()> {
    use std::time::Duration;
    use vag_hex::{KS_F3, decrypt_block};

    let mut backend = D2xxBackend::open(serial).map_err(|e| open_diagnostic(serial, e))?;
    let report = vag_hex::session_probe(&mut backend, Duration::from_secs(listen_secs))
        .await
        .context("session probe failed")?;

    println!(
        "cable sent {} raw byte(s), {} frame(s). Decoding b7 replies with KS_F3 (off6..13):",
        report.raw_bytes.len(),
        report.received.len()
    );
    let mut tp_ok = false;
    for f in &report.received {
        if f.opcode != vag_hex::frame::OP_DIAG_RESP || f.data.len() < 16 {
            continue;
        }
        let block: [u8; 16] = f.data[..16].try_into().unwrap();
        let dec = decrypt_block(&block, &KS_F3);
        // off6 = PCI, off7 = SID (positive resp = SID|0x40; TesterPresent = 0x7E).
        let region: String = dec[6..14].iter().map(|b| format!("{b:02x} ")).collect();
        let sid = dec[7];
        let tag = if sid == 0x7E { "  <- TesterPresent POSITIVE (7E)" } else { "" };
        if sid == 0x7E {
            tp_ok = true;
        }
        println!("  b7 off6..13 = {}{}", region.trim_end(), tag);
    }

    if tp_ok {
        println!(
            "\n✅ PROVEN: a b7 reply decodes to TesterPresent positive (7E) with the capture's \
             KS_F3. The live session key == the capture's — recovered keystreams drive this \
             cable live. VIN read is next."
        );
    } else {
        println!(
            "\n⚠️  No 7E decode yet. Either the cable NAK'd the replayed counter (off14), or the \
             live key differs. Check the raw frames above; next we align the counter/trailer."
        );
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
