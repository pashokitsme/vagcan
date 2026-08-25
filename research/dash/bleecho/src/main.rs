//! `bleecho` — scan for BLE devices, pick one, and talk to it.
//!
//! The point of this tool is the *client* side. The firmware in `../ble`
//! proved a board can advertise and serve GATT; this proves a laptop can find
//! it, choose it, and move data both ways from Rust — which is the language
//! `vagcan` is written in, so whatever this does is directly reusable.
//!
//! Transport is the Nordic UART Service: a write characteristic in, a notify
//! characteristic out. Not a SIG profile, but the de-facto one, and the honest
//! replacement for the Bluetooth-Classic SPP that BLE simply does not have.

use anyhow::{Context, Result, bail};
use btleplug::api::{
    Central, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Manager, Peripheral};
use futures::StreamExt;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader, Stdin};
use uuid::{Uuid, uuid};

/// Nordic UART Service, and the two characteristics that make it a pipe.
/// The direction names are from the *central's* point of view, which is the
/// usual source of confusion: we write to RX, the device notifies on TX.
const NUS_SERVICE: Uuid = uuid!("6e400001-b5a3-f393-e0a9-e50e24dcca9e");
const NUS_RX: Uuid = uuid!("6e400002-b5a3-f393-e0a9-e50e24dcca9e");
const NUS_TX: Uuid = uuid!("6e400003-b5a3-f393-e0a9-e50e24dcca9e");

const SCAN_SECS: u64 = 6;

/// One reader for the whole process. Two `BufReader`s over the same stdin
/// silently eat each other's input: the first buffers everything available and
/// throws the remainder away when it is dropped, so the second sees EOF. That
/// is not a hypothetical — it cost this tool its first run.
type Lines = tokio::io::Lines<BufReader<Stdin>>;

/// What one scan turned up. Kept separate from btleplug's `Peripheral` so the
/// listing can be sorted and printed without holding the adapter's locks.
struct Found {
    peripheral: Peripheral,
    name: Option<String>,
    rssi: Option<i16>,
    services: Vec<Uuid>,
    /// Manufacturer-specific data, by company identifier. Often the only thing
    /// an unnamed device tells you about itself.
    manufacturer: BTreeMap<u16, Vec<u8>>,
}

impl Found {
    fn speaks_nus(&self) -> bool {
        self.services.contains(&NUS_SERVICE)
    }

    /// A device with no name is not a broken device — plenty advertise only an
    /// address until you connect. Say so rather than printing an empty field.
    fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| "(no name)".into())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    let manager = Manager::new().await?;
    let adapter = manager
        .adapters()
        .await?
        .into_iter()
        .next()
        .context("no Bluetooth adapter (on macOS this also means the app was denied Bluetooth access)")?;

    println!("scanning for {SCAN_SECS} s ...");
    adapter.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(Duration::from_secs(SCAN_SECS)).await;
    adapter.stop_scan().await?;

    let mut found = collect(&adapter).await?;
    if found.is_empty() {
        bail!("nothing on the air");
    }
    // Named devices first, then by signal: the thing you are looking for is
    // almost always named and almost always the closest.
    found.sort_by(|a, b| {
        b.speaks_nus()
            .cmp(&a.speaks_nus())
            .then(a.name.is_none().cmp(&b.name.is_none()))
            .then(b.rssi.unwrap_or(i16::MIN).cmp(&a.rssi.unwrap_or(i16::MIN)))
    });

    list(&found);
    let choice = prompt_choice(&mut lines, found.len()).await?;
    let target = &found[choice];

    println!("\nconnecting to {} ...", target.label());
    target.peripheral.connect().await?;
    target.peripheral.discover_services().await?;
    println!("connected");

    let (rx, tx) = match nus_characteristics(&target.peripheral) {
        Some(pair) => pair,
        None => {
            describe(&target.peripheral);
            target.peripheral.disconnect().await.ok();
            bail!("this device does not expose the Nordic UART Service, so there is nothing to echo against");
        }
    };

    echo(&mut lines, &target.peripheral, rx, tx).await?;
    target.peripheral.disconnect().await.ok();
    Ok(())
}

async fn collect(adapter: &btleplug::platform::Adapter) -> Result<Vec<Found>> {
    let mut out = Vec::new();
    for peripheral in adapter.peripherals().await? {
        let props = match peripheral.properties().await? {
            Some(p) => p,
            None => continue,
        };
        out.push(Found {
            peripheral,
            name: props.local_name,
            rssi: props.rssi,
            services: props.services,
            manufacturer: props.manufacturer_data.into_iter().collect(),
        });
    }
    Ok(out)
}

fn list(found: &[Found]) {
    println!("\n{} device(s):\n", found.len());
    for (i, d) in found.iter().enumerate() {
        let rssi = d.rssi.map(|r| format!("{r:>4} dBm")).unwrap_or_else(|| "   ?    ".into());
        let mark = if d.speaks_nus() { " [NUS]" } else { "" };
        // macOS hands out a per-host UUID instead of the BLE address; on Linux
        // and Windows this is the real MAC. Print whatever the platform gives.
        println!("  {:>2}. {:<28} {rssi}  {}{mark}", i + 1, d.label(), d.peripheral.id());
        if !d.services.is_empty() {
            println!("      services: {}", d.services.iter().map(|u| short(*u)).collect::<Vec<_>>().join(", "));
        }
        for (company, data) in &d.manufacturer {
            println!("      manufacturer 0x{company:04x}: {}", hex(data));
        }
    }
}

async fn prompt_choice(lines: &mut Lines, count: usize) -> Result<usize> {
    loop {
        print!("\nselect 1..{count} (or q to quit): ");
        flush();
        let line = lines.next_line().await?.context("stdin closed")?;
        let line = line.trim();
        if line.eq_ignore_ascii_case("q") {
            std::process::exit(0);
        }
        match line.parse::<usize>() {
            Ok(n) if (1..=count).contains(&n) => return Ok(n - 1),
            _ => println!("not a choice"),
        }
    }
}

fn nus_characteristics(p: &Peripheral) -> Option<(Characteristic, Characteristic)> {
    let chars = p.characteristics();
    let rx = chars.iter().find(|c| c.uuid == NUS_RX)?.clone();
    let tx = chars.iter().find(|c| c.uuid == NUS_TX)?.clone();
    Some((rx, tx))
}

/// Printed only when the chosen device is not what we hoped: a listing of what
/// it *does* offer beats "not supported".
fn describe(p: &Peripheral) {
    println!("\nwhat it does offer:");
    for c in p.characteristics() {
        println!("  {}  service {}  {:?}", short(c.uuid), short(c.service_uuid), c.properties);
    }
}

/// Type a line, get it back. Notifications that arrive unprompted — a banner,
/// a battery level — are printed as they come rather than swallowed, because
/// the whole point is seeing what the device says.
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

/// Text if it is text, hex if it is not. A config protocol will be binary and
/// this is where that becomes visible.
fn render(bytes: &[u8]) -> String {
    match core::str::from_utf8(bytes) {
        Ok(s) if s.chars().all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t') => {
            format!("{:?}", s)
        }
        _ => format!("{} ({} bytes)", hex(bytes), bytes.len()),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

/// 16-bit SIG UUIDs are written in full over the air but nobody reads them
/// that way; print the short form when the Bluetooth base UUID applies.
fn short(u: Uuid) -> String {
    const BASE: u128 = 0x0000_0000_0000_1000_8000_0080_5f9b_34fb;
    /// Everything except the 32 bits the short form lives in.
    const MASK: u128 = !(0xffff_ffff_u128 << 96);
    let v = u.as_u128();
    if v & MASK == BASE & MASK {
        format!("0x{:04x}", (v >> 96) as u32)
    } else {
        u.to_string()
    }
}

fn flush() {
    use std::io::Write;
    std::io::stdout().flush().ok();
}
