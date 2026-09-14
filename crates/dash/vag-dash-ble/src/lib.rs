//! Finding a BLE device from a laptop, and opening a pipe to it.
//!
//! The laptop half of everything this project does over BLE: scanning, picking
//! a device out of what the air offers, and opening the Nordic UART Service as
//! a byte pipe. It knows nothing about what travels through that pipe — the
//! dash's settings protocol, an echo, a stream of pixels are all the same to
//! it, which is what lets the product tool and the bench rig share it instead
//! of growing two copies that drift.

use anyhow::{Context, Result, bail};
use btleplug::api::{Central, CentralEvent, CharPropFlags, Characteristic, Manager as _, Peripheral as _, ScanFilter, ValueNotification, WriteType};
use btleplug::platform::Manager;
pub use btleplug::platform::{Adapter, Peripheral};
use futures::{Stream, StreamExt};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader, Stdin};
use uuid::{Uuid, uuid};
use vag_uds_transport::TransportError;
use vag_uds_transport::link::{self, Pipe};

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

	/// What the platform calls the device: on macOS a per-host UUID, elsewhere the address.
	pub fn id(&self) -> String {
		self.peripheral.id().to_string()
	}
}

pub async fn adapter() -> Result<Adapter> {
	// Said before CoreBluetooth is touched, because after is too late (see the function).
	if let Some(warning) = bluetooth_warning(std::env::var("__CFBundleIdentifier").ok().as_deref()) {
		eprintln!("{warning}");
	}
	Manager::new().await?.adapters().await?.into_iter().next().context(
		"no Bluetooth adapter (on macOS this also means Bluetooth access was denied: allow the terminal in \
             System Settings → Privacy & Security → Bluetooth, and run from Terminal.app)",
	)
}

/// Apps macOS is known to *ask* about Bluetooth for, on the first use, rather than kill.
const ASKS_FOR_BLUETOOTH: &[&str] = &["com.apple.Terminal"];

/// What to say before touching Bluetooth, when the process may be killed for it.
///
/// macOS does not refuse Bluetooth to a process whose app carries no Bluetooth usage
/// description: it kills the process (TCC) the moment CoreBluetooth starts, and nothing
/// in the process can catch that or say why afterwards. So it is said before, and only
/// when the app that started the process — `__CFBundleIdentifier`, which launchd sets —
/// is one not known to ask. Seen on the bench (2026-09-14): a run started from the Claude
/// app died; the same run from Terminal.app asked for access. Unset (not macOS, or not
/// started from an app) says nothing.
fn bluetooth_warning(app: Option<&str>) -> Option<String> {
	let app = app?;
	(!ASKS_FOR_BLUETOOTH.contains(&app)).then(|| {
		format!(
			"note: this process was started from {app}, and macOS kills a process whose app is not allowed \
             Bluetooth. If it stops here with no error, run the command from Terminal.app."
		)
	})
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

/// The dash boards in range: what one [`scan`] pass heard offering the Nordic UART
/// Service, in [`scan`]'s order — named first, then the strongest signal.
///
/// The board advertises the service's UUID (in its scan response) and the name
/// `vagcan-dash`. A device heard only by name has not been heard in full and is not
/// taken for a board; a name is not proof of anything anyway.
pub async fn scan_boards(adapter: &Adapter, seconds: u64) -> Result<Vec<Found>> {
	Ok(scan(adapter, seconds).await?.into_iter().filter(Found::speaks_nus).collect())
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

/// Writes to the board are cut to this many bytes.
///
/// btleplug 0.11 has no call for the ATT MTU the platform agreed, so this is a size
/// every agreed MTU on a Mac carries in one write: 185 on older macOS (182 bytes of
/// payload) and 251 on the owner's (the board logs it at connect; 248 of payload). It is
/// also under the board's characteristic storage — 244 bytes, `vag-dash-fw`'s
/// `UART_MTU` — which refuses a longer write whatever the MTU. A host's frame is a
/// request of tens of bytes, so it is one chunk anyway.
pub const WRITE_CHUNK: usize = 180;

/// The Nordic UART Service of a connected board, as a [`Pipe`]: writes go to RX,
/// notifications on TX come back as chunks.
///
/// A notification stream in btleplug does not end when the device goes away, so the
/// adapter's events are watched beside it and a disconnect of this device closes the
/// pipe. Dropping the pipe asks the platform to disconnect, as far as a runtime is
/// still there to do it.
pub struct NusPipe {
	peripheral: Peripheral,
	rx: Characteristic,
	write_type: WriteType,
	notifications: Pin<Box<dyn Stream<Item = ValueNotification> + Send>>,
	events: Pin<Box<dyn Stream<Item = CentralEvent> + Send>>,
	closed: bool,
}

impl NusPipe {
	/// Connect to `peripheral` and open its UART.
	pub async fn connect(adapter: &Adapter, peripheral: &Peripheral) -> Result<NusPipe> {
		// Before connecting, so a disconnect in the first moments is not missed.
		let events = adapter.events().await?;
		let (rx, tx) = open_nus(peripheral).await?;
		if !tx.properties.contains(CharPropFlags::NOTIFY) {
			peripheral.disconnect().await.ok();
			bail!("the board's UART cannot notify, so nothing can come back");
		}
		// The stream before the subscription, so nothing notified in between is lost.
		let notifications = peripheral.notifications().await?;
		peripheral.subscribe(&tx).await?;
		// As `dashcfg` writes: with a response where the board offers one, which also
		// holds the host back at the ATT layer while the board is busy.
		let write_type = if rx.properties.contains(CharPropFlags::WRITE) {
			WriteType::WithResponse
		} else {
			WriteType::WithoutResponse
		};
		Ok(NusPipe {
			peripheral: peripheral.clone(),
			rx,
			write_type,
			notifications,
			events,
			closed: false,
		})
	}
}

impl Pipe for NusPipe {
	async fn write(&mut self, bytes: &[u8]) -> Result<(), TransportError> {
		if self.closed {
			return Err(TransportError::Disconnected);
		}
		for piece in link::chunks(bytes, WRITE_CHUNK) {
			self
				.peripheral
				.write(&self.rx, piece, self.write_type)
				.await
				.map_err(|e| TransportError::Io(format!("writing to the board over BLE: {e}")))?;
		}
		Ok(())
	}

	async fn read(&mut self) -> Option<Vec<u8>> {
		if self.closed {
			return None;
		}
		let id = self.peripheral.id();
		loop {
			// Both are streams, and a stream's `next` is cancel-safe, so this is too.
			tokio::select! {
				biased;
				notified = self.notifications.next() => match notified {
					Some(n) if n.uuid == NUS_TX => return Some(n.value),
					Some(_) => {}
					None => break,
				},
				event = self.events.next() => match event {
					Some(CentralEvent::DeviceDisconnected(gone)) if gone == id => break,
					Some(_) => {}
					None => break,
				},
			}
		}
		self.closed = true;
		None
	}
}

impl Drop for NusPipe {
	fn drop(&mut self) {
		let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
		let peripheral = self.peripheral.clone();
		runtime.spawn(async move {
			peripheral.disconnect().await.ok();
		});
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

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_process_started_from_an_app_that_may_be_killed_is_warned_first() {
		let warning = bluetooth_warning(Some("com.anthropic.claudefordesktop")).expect("a warning");
		assert!(warning.contains("com.anthropic.claudefordesktop"), "{warning}");
		assert!(warning.contains("Terminal.app"), "say where to run it instead: {warning}");
	}

	#[test]
	fn terminal_and_no_app_at_all_are_not_warned() {
		assert_eq!(bluetooth_warning(Some("com.apple.Terminal")), None);
		assert_eq!(bluetooth_warning(None), None);
	}
}
