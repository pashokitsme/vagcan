//! Finding a BLE device from a laptop, and opening a pipe to it.
//!
//! The laptop half of everything this project does over BLE: scanning, picking
//! a device out of what the air offers, and opening the Nordic UART Service as
//! a byte pipe. It knows nothing about what travels through that pipe — the
//! dash's settings protocol, an echo, a stream of pixels are all the same to
//! it, which is what lets the product tool and the bench rig share it instead
//! of growing two copies that drift.

use anyhow::{Context, Result, bail};
use btleplug::api::{Central, Characteristic, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::{Adapter, Manager, Peripheral};
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader, Stdin};
use uuid::{Uuid, uuid};

/// Nordic UART Service, and the two characteristics that make it a pipe.
/// The direction names are from the *central's* point of view, which is the
/// usual source of confusion: we write to RX, the device notifies on TX.
pub const NUS_SERVICE: Uuid = uuid!("6e400001-b5a3-f393-e0a9-e50e24dcca9e");
pub const NUS_RX: Uuid = uuid!("6e400002-b5a3-f393-e0a9-e50e24dcca9e");
pub const NUS_TX: Uuid = uuid!("6e400003-b5a3-f393-e0a9-e50e24dcca9e");

/// One reader for the whole process. Two `BufReader`s over the same stdin
/// silently eat each other's input: the first buffers everything available and
/// throws the remainder away when it is dropped, so the second sees EOF. That
/// is not a hypothetical — it cost this tool its first run.
pub type Lines = tokio::io::Lines<BufReader<Stdin>>;

pub fn stdin_lines() -> Lines {
    BufReader::new(tokio::io::stdin()).lines()
}

/// What one scan turned up. Kept separate from btleplug's `Peripheral` so the
/// listing can be sorted and printed without holding the adapter's locks.
pub struct Found {
    pub peripheral: Peripheral,
    pub name: Option<String>,
    pub rssi: Option<i16>,
    pub services: Vec<Uuid>,
    /// Manufacturer-specific data, by company identifier. Often the only thing
    /// an unnamed device tells you about itself.
    pub manufacturer: BTreeMap<u16, Vec<u8>>,
}

impl Found {
    pub fn speaks_nus(&self) -> bool {
        self.services.contains(&NUS_SERVICE)
    }

    /// A device with no name is not a broken device — plenty advertise only an
    /// address until you connect. Say so rather than printing an empty field.
    pub fn label(&self) -> String {
        self.name.clone().unwrap_or_else(|| "(no name)".into())
    }
}

pub async fn adapter() -> Result<Adapter> {
    Manager::new()
        .await?
        .adapters()
        .await?
        .into_iter()
        .next()
        .context("no Bluetooth adapter (on macOS this also means the app was denied Bluetooth access)")
}

/// One scan pass. Devices that speak NUS come first, then named ones, then by
/// signal: the thing you are looking for is almost always named and almost
/// always the closest.
pub async fn scan(adapter: &Adapter, seconds: u64) -> Result<Vec<Found>> {
    adapter.start_scan(ScanFilter::default()).await?;
    tokio::time::sleep(Duration::from_secs(seconds)).await;
    adapter.stop_scan().await?;

    let mut out = Vec::new();
    for peripheral in adapter.peripherals().await? {
        let Some(props) = peripheral.properties().await? else {
            continue;
        };
        out.push(Found {
            peripheral,
            name: props.local_name,
            rssi: props.rssi,
            services: props.services,
            manufacturer: props.manufacturer_data.into_iter().collect(),
        });
    }
    out.sort_by(|a, b| {
        b.speaks_nus()
            .cmp(&a.speaks_nus())
            .then(a.name.is_none().cmp(&b.name.is_none()))
            .then(b.rssi.unwrap_or(i16::MIN).cmp(&a.rssi.unwrap_or(i16::MIN)))
    });
    Ok(out)
}

pub fn list(found: &[Found]) {
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

pub async fn prompt_choice(lines: &mut Lines, count: usize) -> Result<usize> {
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

/// Connect and find the pipe. Fails with a listing of what the device *does*
/// offer, because "not supported" is not an answer anyone can act on.
pub async fn open_nus(p: &Peripheral) -> Result<(Characteristic, Characteristic)> {
    p.connect().await?;
    p.discover_services().await?;
    let chars = p.characteristics();
    let rx = chars.iter().find(|c| c.uuid == NUS_RX).cloned();
    let tx = chars.iter().find(|c| c.uuid == NUS_TX).cloned();
    match (rx, tx) {
        (Some(rx), Some(tx)) => Ok((rx, tx)),
        _ => {
            println!("\nwhat it does offer:");
            for c in p.characteristics() {
                println!("  {}  service {}  {:?}", short(c.uuid), short(c.service_uuid), c.properties);
            }
            p.disconnect().await.ok();
            bail!("this device does not expose the Nordic UART Service");
        }
    }
}

/// Text if it is text, hex if it is not. A binary framing will be the second
/// version of this protocol and this is where that becomes visible.
pub fn render(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) if s.chars().all(|c| !c.is_control() || c == '\n' || c == '\r' || c == '\t') => {
            format!("{s:?}")
        }
        _ => format!("{} ({} bytes)", hex(bytes), bytes.len()),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
}

/// 16-bit SIG UUIDs are written in full over the air but nobody reads them
/// that way; print the short form when the Bluetooth base UUID applies.
pub fn short(u: Uuid) -> String {
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

pub fn flush() {
    use std::io::Write;
    std::io::stdout().flush().ok();
}
