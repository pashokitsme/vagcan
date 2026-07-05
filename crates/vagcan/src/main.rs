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
        Command::Handshake { serial, listen } => handshake(serial.as_deref(), listen).await,
    }
}

/// Dynamic session handshake: advance past the 0x39 auth-stall with a
/// runtime-derived counter, then TesterPresent + VIN on the f3 channel.
async fn handshake(serial: Option<&str>, listen_secs: u64) -> anyhow::Result<()> {
    use std::time::Duration;

    let mut backend = D2xxBackend::open(serial).map_err(|e| open_diagnostic(serial, e))?;
    let report = vag_hex::drive_session_sweep(&mut backend, Duration::from_secs(listen_secs))
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
