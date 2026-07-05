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
    }
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
