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
    /// Decode a captured link-cipher log (NOT YET WIRED).
    ///
    /// Stub: the link-decode port lands in a parallel task; a follow-up wires
    /// this subcommand to it. Always exits non-zero for now.
    Decode,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Doctor { serial } => doctor(serial.as_deref()).await,
        Command::Decode => anyhow::bail!(
            "decode: not yet wired — the link-decode port lands in a follow-up task"
        ),
    }
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
