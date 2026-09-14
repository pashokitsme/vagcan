//! The board as a CAN adapter and nothing else: slcan over the USB console, TWAI
//! underneath.
//!
//! Plugged into the laptop, this image is a second CANable. It speaks the
//! LAWICEL slcan ASCII protocol on the USB-Serial-JTAG port, so
//! `vag_uds_can::SlcanBackend` — the client every `vagcan` command opens the
//! cable with — drives it unchanged: `watch`, `info`, `units`, `faults`,
//! `dev sniff`, `dev survey` all work through `--device /dev/cu.usbmodem…`
//! exactly as through the CANable. Nothing is decided on the board: no
//! address, no identifier, no service. Bytes in, frames out, and back.
//!
//! The adapter itself — the commands, the ring, the flags, transmit and what `z`
//! promises — is [`vag_dash_fw::slcan`], which the `dash` image runs too, as its
//! adapter mode (`vagcan --slcan`). What is this image's own:
//!
//! - **The ring holds half a second of a saturated bus**: [`RING`] lines, 2,048 at
//!   4,000 frames a second (two thirds of a second at the gateway's 3,106/s heartbeat
//!   storm, the heaviest thing the car does). It costs 74 KB of the C3's 400 KB, which
//!   an image with no radio and a 32 KB heap has to spare.
//! - **Nothing but slcan traffic goes down the console.** No logger is installed
//!   in this binary — not `esp_println::logger::init_logger`, not `…_from_env` —
//!   so esp-hal's own `warn!`s go to the `log` facade and evaporate. A panic is
//!   the one exception: `health.rs`'s handler prints it through esp-println, and
//!   at that point the stream is dead anyway.
//! - **A watchdog.** The RTC watchdog is armed at boot and fed from a task that does
//!   nothing else, as `dash` does — so if the bridge ever stops yielding, the adapter
//!   reboots within [`WATCHDOG`] rather than sit dead on the port until the cable is
//!   pulled.
//!
//! This binary is the adapter, so it **may see a car** — it does on the bus
//! exactly what the CANable does, which is whatever `vagcan` asks of it, and
//! `vagcan`'s allowlist is what bounds that.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_io_async::{Read as _, Write as _};
use esp_backtrace as _;
use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::{GPIO1, GPIO6, TWAI0};
use esp_hal::rtc_cntl::{Rwdt, RwdtStage};
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use vag_dash_fw::slcan::{Adapter, CommandLine, Commands, Line, LineParser, Packet, Port};

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

/// How many lines the port's ring holds: half a second of a saturated bus (module docs).
const RING: usize = 2048;

/// The adapter's output, waiting for [`console_tx`].
static PORT: Port<RING> = Port::new();

/// The watchdog's patience: nothing legitimate on this executor blocks for
/// even a tenth of it, and an adapter that is dead for longer is one the host
/// has already given up on.
const WATCHDOG: esp_hal::time::Duration = esp_hal::time::Duration::from_secs(4);

/// Feed interval — a quarter of the timeout, so a missed feed or two is not a
/// reboot.
const FEED_EVERY: Duration = Duration::from_millis(1000);

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
	// Deliberately no logger: the console is the protocol.
	let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
	// The `#[panic_handler]` lives in the library (`health.rs`), and its printer
	// allocates, so the heap exists for it even though this image never allocates in
	// its steady state.
	esp_alloc::heap_allocator!(size: 32 * 1024);
	esp_hal_embassy::init(SystemTimer::new(peripherals.SYSTIMER).alarm0);

	// Never `.ok()` a spawn: the arena is finite and a full one fails silently.
	// With no console to say so, a failed spawn would be an adapter that
	// enumerates and answers nothing — so it halts here instead, and the
	// symptom is a port that opens and never replies to `V`.
	//
	// The feeder first, and the watchdog only once it is running: a watchdog
	// nobody feeds is a reboot loop, which is worse than any hang.
	if spawner.spawn(feed()).is_err() {
		panic!("spawn feed");
	}
	let mut wdt = Rwdt::new();
	wdt.enable();
	wdt.set_timeout(RwdtStage::Stage0, WATCHDOG);

	let (usb_rx, usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async().split();

	if spawner.spawn(console_tx(usb_tx)).is_err() {
		panic!("spawn console_tx");
	}
	if spawner
		.spawn(bridge(usb_rx, peripherals.TWAI0, peripherals.GPIO1, peripherals.GPIO6))
		.is_err()
	{
		panic!("spawn bridge");
	}
}

/// Feeds the watchdog for as long as the executor is scheduling tasks, and
/// does nothing else — anything it also did could block it, and a feeder that
/// can block is a watchdog that fires for the wrong reason.
#[embassy_executor::task]
async fn feed() -> ! {
	let mut wdt = Rwdt::new();
	loop {
		wdt.feed();
		Timer::after(FEED_EVERY).await;
	}
}

/// Drains the port into the console, whole lines packed into each USB packet.
///
/// The USB-Serial-JTAG hands the host 64 bytes per packet and esp-hal's writer waits
/// for each packet to leave, so one packet per write is the natural unit:
/// [`Port::next_packet`] says why packing whole lines is what keeps the wire ahead of
/// the bus.
#[embassy_executor::task]
async fn console_tx(mut usb: UsbSerialJtagTx<'static, Async>) -> ! {
	let mut packet = Packet::new();
	// A line taken out of the ring that did not fit the packet it was taken for. It is
	// the one thing this task holds that a `C` cannot see.
	let mut carry: Option<Line> = None;
	loop {
		let packed = PORT.next_packet(&mut carry, &mut packet).await;
		// Cannot fail on this peripheral; if the host is not reading, it waits,
		// and the bridge keeps counting what it has to drop meanwhile.
		let _ = usb.write_all(&packet).await;
		PORT.written(packed, &mut carry);
	}
}

/// The console's bytes, cut into command lines as they arrive.
struct UsbCommands {
	usb: UsbSerialJtagRx<'static, Async>,
	parser: LineParser,
	chunk: [u8; 64],
	at: usize,
	len: usize,
}

impl Commands for UsbCommands {
	/// Never `None`: this image is an adapter until it is powered off.
	///
	/// Cancel-safe: the lines of a chunk already read wait in `chunk`, and esp-hal's
	/// read takes bytes out of the FIFO only in the poll that returns them.
	async fn next(&mut self) -> Option<CommandLine> {
		loop {
			while self.at < self.len {
				let byte = self.chunk[self.at];
				self.at += 1;
				if let Some((line, _)) = self.parser.feed(byte) {
					return Some(line);
				}
			}
			self.len = self.usb.read(&mut self.chunk).await.unwrap_or(0);
			self.at = 0;
		}
	}
}

/// Console bytes in, commands out; bus frames in, console lines out.
#[embassy_executor::task]
async fn bridge(usb: UsbSerialJtagRx<'static, Async>, twai0: TWAI0<'static>, rx_pin: GPIO1<'static>, tx_pin: GPIO6<'static>) -> ! {
	let mut adapter = Adapter::new(&PORT, twai0, rx_pin, tx_pin);
	let mut commands = UsbCommands {
		usb,
		parser: LineParser::slcan(),
		chunk: [0; 64],
		at: 0,
		len: 0,
	};
	vag_dash_fw::slcan::serve(&mut adapter, &mut commands).await;
	// `serve` returns only when the commands end, and these never do.
	unreachable!("the standalone adapter's commands never end")
}
