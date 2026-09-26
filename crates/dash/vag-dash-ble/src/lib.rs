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
use std::future::Future;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader, Stdin};
use tokio::sync::mpsc;
use uuid::{Uuid, uuid};
use vag_uds_transport::TransportError;
use vag_uds_transport::link::{self, Pipe};

/// Nordic UART Service, and the two characteristics that make it a pipe.
/// The direction names are from the *central's* point of view, which is the
/// usual source of confusion: we write to RX, the device notifies on TX.
pub const NUS_SERVICE: Uuid = uuid!("6e400001-b5a3-f393-e0a9-e50e24dcca9e");
pub const NUS_RX: Uuid = uuid!("6e400002-b5a3-f393-e0a9-e50e24dcca9e");
pub const NUS_TX: Uuid = uuid!("6e400003-b5a3-f393-e0a9-e50e24dcca9e");

/// How much longer a scan for boards listens once the first one is heard: long enough
/// for a second board nearby to be heard too, short enough that the usual case — one
/// board — connects at once.
pub const SCAN_SETTLE: Duration = Duration::from_secs(1);

/// How often a scan for boards looks at what it has heard so far.
const SCAN_LOOK: Duration = Duration::from_millis(250);

/// How long connecting to a device may take, service discovery included. CoreBluetooth
/// never gives up on a connection by itself, so without a bound a board that went out of
/// range between the scan and the connect hangs the command.
pub const CONNECT_WITHIN: Duration = Duration::from_secs(10);

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
	Manager::new().await?.adapters().await?.into_iter().next().context(
		"no Bluetooth adapter (on macOS this also means Bluetooth access was denied: allow the app that runs \
             this in System Settings → Privacy & Security → Bluetooth)",
	)
}

/// One scan pass. Devices that speak NUS come first, then named ones, then by
/// signal: the thing you are looking for is almost always named and almost
/// always the closest.
pub async fn scan(adapter: &Adapter, seconds: u64) -> Result<Vec<Found>> {
	adapter.start_scan(ScanFilter::default()).await?;
	tokio::time::sleep(Duration::from_secs(seconds)).await;
	adapter.stop_scan().await?;
	let mut out = heard(adapter).await?;
	sort(&mut out);
	Ok(out)
}

/// Everything the adapter has heard so far, unsorted.
async fn heard(adapter: &Adapter) -> Result<Vec<Found>> {
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
	Ok(out)
}

fn sort(found: &mut [Found]) {
	found.sort_by(|a, b| {
		b.speaks_nus()
			.cmp(&a.speaks_nus())
			.then(a.name.is_none().cmp(&b.name.is_none()))
			.then(b.rssi.unwrap_or(i16::MIN).cmp(&a.rssi.unwrap_or(i16::MIN)))
	});
}

/// The dash boards in range: what a scan heard offering the Nordic UART Service, in
/// [`scan`]'s order — named first, then the strongest signal.
///
/// The scan stops [`SCAN_SETTLE`] after the first board is heard, and at `cap` when none
/// is ([`scan_done`]). The board advertises the service's UUID (in its scan response)
/// and the name `vagcan-dash`. A device heard only by name has not been heard in full
/// and is not taken for a board; a name is not proof of anything anyway.
pub async fn scan_boards(adapter: &Adapter, cap: Duration) -> Result<Vec<Found>> {
	adapter.start_scan(ScanFilter::default()).await?;
	let listened = listen_for_boards(adapter, cap).await;
	// Stopped whatever the listening came to, so a failed look leaves no radio scanning.
	let stopped = adapter.stop_scan().await;
	let mut boards = listened?;
	stopped?;
	sort(&mut boards);
	Ok(boards)
}

async fn listen_for_boards(adapter: &Adapter, cap: Duration) -> Result<Vec<Found>> {
	let started = tokio::time::Instant::now();
	let mut first = None;
	loop {
		tokio::time::sleep(SCAN_LOOK).await;
		let boards: Vec<Found> = heard(adapter).await?.into_iter().filter(Found::speaks_nus).collect();
		let elapsed = started.elapsed();
		if first.is_none() && !boards.is_empty() {
			first = Some(elapsed);
		}
		if scan_done(elapsed, first, cap) {
			return Ok(boards);
		}
	}
}

/// Whether a scan for boards has listened long enough, `elapsed` in, having heard its
/// first board at `first_board`: [`SCAN_SETTLE`] after that, or at `cap` if none came.
fn scan_done(elapsed: Duration, first_board: Option<Duration>, cap: Duration) -> bool {
	elapsed >= cap || first_board.is_some_and(|first| elapsed >= first + SCAN_SETTLE)
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
///
/// Connecting and discovering are given [`CONNECT_WITHIN`] together; past it the
/// connection attempt is cancelled and the error says what to check.
pub async fn open_nus(p: &Peripheral) -> Result<(Characteristic, Characteristic)> {
	let name = match p.properties().await {
		Ok(Some(props)) => props.local_name,
		_ => None,
	}
	.unwrap_or_else(|| p.id().to_string());
	let attempt = async {
		p.connect().await?;
		p.discover_services().await?;
		anyhow::Ok(())
	};
	if let Err(failed) = within(CONNECT_WITHIN, &name, attempt).await {
		p.disconnect().await.ok();
		return Err(failed);
	}
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

/// A connection `attempt` to `name`, given up on after `limit`.
async fn within<T>(limit: Duration, name: &str, attempt: impl Future<Output = Result<T>>) -> Result<T> {
	match tokio::time::timeout(limit, attempt).await {
		Ok(done) => done,
		Err(_) => bail!(
			"could not connect to {name} over BLE within {} s — ignition on and in range?",
			limit.as_secs()
		),
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
/// Notifications are taken off btleplug by a task of their own ([`pump`]) into a queue
/// with no bound, the moment they land, so nothing waits on whoever reads the pipe.
/// Dropping the pipe stops that task and asks the platform to disconnect, as far as a
/// runtime is still there to do it.
pub struct NusPipe {
	peripheral: Peripheral,
	rx: Characteristic,
	write_type: WriteType,
	chunks: mpsc::UnboundedReceiver<Vec<u8>>,
	pump: tokio::task::JoinHandle<()>,
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
		let (to, chunks) = mpsc::unbounded_channel();
		let id = peripheral.id();
		let gone = move |event: &CentralEvent| matches!(event, CentralEvent::DeviceDisconnected(gone) if *gone == id);
		Ok(NusPipe {
			peripheral: peripheral.clone(),
			rx,
			write_type,
			chunks,
			pump: tokio::spawn(pump(notifications, events, gone, to)),
			closed: false,
		})
	}
}

/// Move every TX notification into `to` as it arrives, until `gone` says the device has
/// disconnected, either stream ends, or the pipe is dropped.
///
/// Its own task, so notifications leave btleplug's broadcast channel the moment they land:
/// that channel holds 16 (btleplug 0.11, macOS), and a receiver that falls further behind
/// loses the oldest without a word — `Peripheral::notifications` filters the `Lagged`
/// error out (`notifications_stream_from_broadcast_receiver`), so a loss cannot be seen
/// from here. A lost chunk still shows at the reassembler, which the remote bus takes as
/// a broken link rather than go on reading bytes it cannot trust.
async fn pump<E>(
	mut notifications: impl Stream<Item = ValueNotification> + Unpin,
	mut events: impl Stream<Item = E> + Unpin,
	gone: impl Fn(&E) -> bool,
	to: mpsc::UnboundedSender<Vec<u8>>,
) {
	loop {
		tokio::select! {
			biased;
			notified = notifications.next() => match notified {
				Some(n) if n.uuid == NUS_TX => {
					if to.send(n.value).is_err() {
						return;
					}
				}
				Some(_) => {}
				None => return,
			},
			event = events.next() => match event {
				Some(event) if gone(&event) => return,
				Some(_) => {}
				None => return,
			},
			() = to.closed() => return,
		}
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
		// A channel's `recv` is cancel-safe, so this is too.
		let chunk = self.chunks.recv().await;
		self.closed = chunk.is_none();
		chunk
	}
}

impl Drop for NusPipe {
	fn drop(&mut self) {
		self.pump.abort();
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
	use futures::stream;

	#[test]
	fn a_scan_stops_a_moment_after_the_first_board_and_at_the_cap_when_none_comes() {
		let cap = Duration::from_secs(4);
		let ms = Duration::from_millis;
		assert!(!scan_done(ms(250), None, cap), "nothing yet, still listening");
		assert!(!scan_done(ms(3750), None, cap));
		assert!(scan_done(ms(4000), None, cap), "the cap, with nothing heard");
		assert!(!scan_done(ms(1000), Some(ms(500)), cap), "a second board may still be coming");
		assert!(scan_done(ms(1500), Some(ms(500)), cap), "settled a second after the first board");
		assert!(scan_done(ms(4000), Some(ms(3750)), cap), "never past the cap");
	}

	#[tokio::test]
	async fn a_connection_that_never_completes_is_given_up_on_and_says_what_to_check() {
		let failed = within(Duration::from_millis(10), "vagcan-dash", std::future::pending::<Result<()>>())
			.await
			.expect_err("given up on");
		let text = failed.to_string();
		assert!(text.contains("could not connect to vagcan-dash over BLE within"), "{text}");
		assert!(text.contains("ignition on and in range"), "{text}");
		assert_eq!(within(Duration::from_secs(1), "vagcan-dash", async { Ok(7) }).await.unwrap(), 7);
	}

	fn notified(uuid: Uuid, value: Vec<u8>) -> ValueNotification {
		ValueNotification { uuid, value }
	}

	#[tokio::test]
	async fn the_pump_forwards_every_tx_notification_in_order_and_stops_at_this_devices_disconnect() {
		let mut burst: Vec<ValueNotification> = (0..1000u16).map(|i| notified(NUS_TX, i.to_le_bytes().to_vec())).collect();
		burst.insert(3, notified(NUS_RX, vec![0xEE]));
		let notifications = stream::iter(burst).chain(stream::pending());
		// Another device's disconnect first, then this one's.
		let events = stream::iter([false, true]).chain(stream::pending());
		let (to, mut chunks) = mpsc::unbounded_channel();
		pump(notifications, events, |gone: &bool| *gone, to).await;
		for i in 0..1000u16 {
			assert_eq!(chunks.recv().await, Some(i.to_le_bytes().to_vec()), "chunk {i}");
		}
		assert_eq!(chunks.recv().await, None, "the pump has stopped, so the pipe reads closed");
	}

	#[tokio::test]
	async fn the_pump_stops_when_nobody_reads_the_pipe_any_more() {
		let (to, chunks) = mpsc::unbounded_channel();
		drop(chunks);
		let events = stream::pending::<bool>();
		tokio::time::timeout(Duration::from_secs(1), pump(stream::pending(), events, |_: &bool| false, to))
			.await
			.expect("stopped");
	}
}
