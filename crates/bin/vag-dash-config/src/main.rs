//! `dashcfg` — configure the dash over BLE.
//!
//! The device is **dark by default**: it advertises nothing until somebody
//! holds its button for three seconds, and it goes dark again the moment the
//! connection drops. That is the whole access-control story — reaching this
//! device means standing next to it — so this tool's first job is to wait
//! patiently and say what to press, rather than to fail with "not found".

use anyhow::{Result, bail};
use vag_ble::{Lines, flush, open_nus, render, scan, stdin_lines};
use btleplug::api::{CharPropFlags, Characteristic, Peripheral as _, WriteType};
use btleplug::platform::Peripheral;
use futures::StreamExt;
use std::time::Duration;

const DEFAULT_NAME: &str = "vagcan-dash";
const SCAN_SECS: u64 = 4;

/// What the device told us about itself. Parsed from the one line it pushes on
/// every change, so the client never has to poll to stay right.
#[derive(Debug, Default, Clone)]
struct DashState {
    page: Option<u8>,
    pages: Option<u8>,
    brightness: Option<u8>,
    unsaved: Option<bool>,
    generation: Option<u32>,
    kind: Option<String>,
    cells: Option<String>,
}

impl DashState {
    /// `state page=1/2 brightness=77 unsaved=0 gen=3 kind=chart cells=[0]`
    ///
    /// Unknown keys are ignored rather than rejected: the firmware will grow
    /// fields, and a client that refuses to parse a newer device is a client
    /// that has to be updated in lockstep with it.
    fn parse(line: &str) -> Option<Self> {
        let rest = line.strip_prefix("state ")?;
        let mut s = Self::default();
        for token in rest.split_whitespace() {
            let Some((key, value)) = token.split_once('=') else {
                continue;
            };
            match key {
                "page" => {
                    if let Some((now, total)) = value.split_once('/') {
                        s.page = now.parse().ok();
                        s.pages = total.parse().ok();
                    }
                }
                "brightness" => s.brightness = value.parse().ok(),
                "unsaved" => s.unsaved = Some(value != "0"),
                "gen" => s.generation = value.parse().ok(),
                "kind" => s.kind = Some(value.to_string()),
                "cells" => s.cells = Some(value.to_string()),
                _ => {}
            }
        }
        Some(s)
    }

    fn show(&self, name: &str) {
        const WIDTH: usize = 46;
        println!("\n┌─ {name} {}", "─".repeat(WIDTH.saturating_sub(name.len() + 4)));
        match (self.page, self.pages) {
            (Some(page), Some(pages)) => {
                let kind = self.kind.as_deref().unwrap_or("?");
                let cells = self.cells.as_deref().unwrap_or("?");
                println!("│ page        {} of {pages}   {kind} {cells}", page + 1);
            }
            _ => println!("│ page        ?"),
        }
        if let Some(b) = self.brightness {
            let filled = usize::from(b) * 20 / 255;
            println!("│ brightness  {b:>3}  [{}{}]", "█".repeat(filled), "·".repeat(20 - filled));
        }
        let saved = match self.unsaved {
            Some(true) => "UNSAVED — 'save' to keep it".to_string(),
            Some(false) => format!("saved (generation {})", self.generation.unwrap_or(0)),
            None => "?".to_string(),
        };
        println!("│ storage     {saved}");
        println!("└{}", "─".repeat(WIDTH));
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let name = std::env::args().nth(1).unwrap_or_else(|| DEFAULT_NAME.into());
    let mut lines = stdin_lines();
    let adapter = vag_ble::adapter().await?;

    let target = wait_for(&adapter, &name).await?;
    println!("\nfound {name}, connecting ...");
    let (rx, tx) = open_nus(&target).await?;
    println!("connected");

    let result = session(&mut lines, &target, rx, tx, &name).await;
    target.disconnect().await.ok();
    result
}

/// Scans until the device appears. It only appears when somebody holds the
/// button, so the hint is repeated rather than printed once and scrolled away.
async fn wait_for(adapter: &btleplug::platform::Adapter, name: &str) -> Result<Peripheral> {
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        if attempt == 1 {
            println!("looking for {name} ...");
        } else if attempt % 3 == 0 {
            println!("still nothing — hold the button on the device for 3 s to make it visible");
        }
        for found in scan(adapter, SCAN_SECS).await? {
            if found.name.as_deref() == Some(name) {
                return Ok(found.peripheral);
            }
        }
    }
}

async fn session(lines: &mut Lines, p: &Peripheral, rx: Characteristic, tx: Characteristic, name: &str) -> Result<()> {
    if !tx.properties.contains(CharPropFlags::NOTIFY) {
        bail!("the device cannot notify, so nothing can come back");
    }
    p.subscribe(&tx).await?;
    let mut notifications = p.notifications().await?;
    let write_type = if rx.properties.contains(CharPropFlags::WRITE) {
        WriteType::WithResponse
    } else {
        WriteType::WithoutResponse
    };

    // Ask, rather than wait to be told. The device pushes its state on
    // connecting, but a notification sent before the subscription lands is
    // dropped by design — so the first state always comes from a request.
    p.write(&rx, b"state", write_type).await?;

    println!("\ncommands: set brightness N | set page N | save | load | defaults | erase | q");
    println!("the device pushes its state whenever its button is pressed.\n");

    loop {
        print!("dash> ");
        flush();
        tokio::select! {
            // Bias towards notifications: a button press on the device should
            // appear the moment it happens, not after the next command.
            biased;
            Some(n) = notifications.next() => {
                print!("\r");
                report(&n.value, name);
                print!("dash> ");
                flush();
            }
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                if line.eq_ignore_ascii_case("q") || line.eq_ignore_ascii_case("quit") {
                    break;
                }
                if let Err(e) = p.write(&rx, line.as_bytes(), write_type).await {
                    println!("  write failed: {e}");
                    continue;
                }
                match tokio::time::timeout(Duration::from_secs(3), notifications.next()).await {
                    Ok(Some(n)) => report(&n.value, name),
                    Ok(None) => break,
                    Err(_) => println!("  (no reply in 3 s — the device may have gone dark)"),
                }
            }
        }
    }
    p.unsubscribe(&tx).await.ok();
    Ok(())
}

/// A state push gets drawn as a panel; anything else is the device answering a
/// command, and is printed as it came.
fn report(bytes: &[u8], name: &str) {
    match std::str::from_utf8(bytes) {
        Ok(text) => match DashState::parse(text) {
            Some(state) => state.show(name),
            None => println!("  {text}"),
        },
        Err(_) => println!("  {}", render(bytes)),
    }
}
