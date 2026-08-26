//! Scan, pick a device, and echo whatever you type at it over the Nordic UART
//! Service. The generic tool; `dashcfg` is the one that knows what the dash's
//! answers mean.

use anyhow::{Result, bail};
use vag_ble::{Lines, flush, list, open_nus, prompt_choice, render, scan, stdin_lines};
use btleplug::api::{CharPropFlags, Characteristic, Peripheral as _, WriteType};
use btleplug::platform::Peripheral;
use futures::StreamExt;
use std::time::Duration;

const SCAN_SECS: u64 = 6;

#[tokio::main]
async fn main() -> Result<()> {
    let mut lines = stdin_lines();
    let adapter = vag_ble::adapter().await?;

    println!("scanning for {SCAN_SECS} s ...");
    let found = scan(&adapter, SCAN_SECS).await?;
    if found.is_empty() {
        bail!("nothing on the air");
    }
    list(&found);

    let target = &found[prompt_choice(&mut lines, found.len()).await?];
    println!("\nconnecting to {} ...", target.label());
    let (rx, tx) = open_nus(&target.peripheral).await?;
    println!("connected");

    let result = echo(&mut lines, &target.peripheral, rx, tx).await;
    target.peripheral.disconnect().await.ok();
    result
}

/// Type a line, get it back. Notifications that arrive unprompted — a state
/// push, a battery level — are printed as they come rather than swallowed,
/// because the whole point is seeing what the device says.
async fn echo(lines: &mut Lines, p: &Peripheral, rx: Characteristic, tx: Characteristic) -> Result<()> {
    if !tx.properties.contains(CharPropFlags::NOTIFY) {
        bail!("the device's TX characteristic cannot notify, so nothing can come back");
    }
    p.subscribe(&tx).await?;
    let mut notifications = p.notifications().await?;

    // With response, not without: this is configuration, not a stream. Each
    // write is acknowledged at the ATT layer, so a failure is reported rather
    // than dropped silently.
    let write_type = if rx.properties.contains(CharPropFlags::WRITE) {
        WriteType::WithResponse
    } else {
        WriteType::WithoutResponse
    };

    println!("\necho mode. type a line and it comes back. ctrl-d or 'q' to leave.");
    println!("writes use {write_type:?}\n");

    loop {
        print!("> ");
        flush();
        tokio::select! {
            // Bias towards draining notifications so an unprompted message is
            // not held behind the prompt.
            biased;
            Some(n) = notifications.next() => {
                println!("\r< {}", render(&n.value));
            }
            line = lines.next_line() => {
                let Some(line) = line? else { break };
                let line = line.trim_end_matches(['\r', '\n']);
                if line.eq_ignore_ascii_case("q") {
                    break;
                }
                if let Err(e) = p.write(&rx, line.as_bytes(), write_type).await {
                    println!("  write failed: {e}");
                    continue;
                }
                // The reply is one round trip away; a connection interval is
                // 7.5-30 ms, so this is generous rather than tight.
                match tokio::time::timeout(Duration::from_secs(2), notifications.next()).await {
                    Ok(Some(n)) => println!("< {}", render(&n.value)),
                    Ok(None) => break,
                    Err(_) => println!("  (no reply in 2 s)"),
                }
            }
        }
    }
    p.unsubscribe(&tx).await.ok();
    Ok(())
}
