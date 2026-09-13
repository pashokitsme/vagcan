//! The board as a CAN adapter: slcan over the USB console, TWAI underneath.
//!
//! Plugged into the laptop, this image is a second CANable. It speaks the
//! LAWICEL slcan ASCII protocol on the USB-Serial-JTAG port, so
//! `vag_uds_can::SlcanBackend` — the client every `vagcan` command opens the
//! cable with — drives it unchanged: `watch`, `info`, `units`, `faults`,
//! `dev sniff`, `dev survey` all work through `--device /dev/cu.usbmodem…`
//! exactly as through the CANable. Nothing is decided on the board: no
//! address, no identifier, no service. Bytes in, frames out, and back.
//!
//! This is the **exclusive adapter mode** of `todo/dash/14-one-bus-three-clients.md`
//! (its "mode 2", the dumb slcan proxy): raw frames on the host's own clock,
//! and no panel beside it. The rule that file sets for it — *slowing down is
//! allowed, dropping is forbidden as far as a buffer can prevent it* — is
//! what the ring below is sized for.
//!
//! What it honours is what `slcan.rs` on the host sends, plus the handful of
//! Lawicel commands a terminal user or a probe would type:
//!
//! | command | does | reply |
//! |---|---|---|
//! | `C` | close the channel; drops queued frames | `\r` |
//! | `S4` `S5` `S6` `S8` | 125 / 250 / 500 / 1000 kbit/s, closed only | `\r`, else `\x07` |
//! | other `S` | a rate this controller does not have: **unsets** the rate, so the next `O` is refused too | `\x07` |
//! | `M0` / `M1` | normal / listen-only for the next `O`, closed only | `\r` |
//! | `O` | open in the configured mode, closed only | `\r`, else `\x07` |
//! | `L` | open listen-only this once; `M` is not changed; closed only | `\r`, else `\x07` |
//! | `tiiiLdd…` / `Tiiiiiiiildd…` | transmit an 11- / 29-bit frame | `z\r` / `Z\r` once the controller reports the frame completed — on the bus, acknowledged; `\x07` if refused, or not completed within [`TX_ATTEMPTS`] tries or [`TX_TIMEOUT`] |
//! | `r…` / `R…` | remote frames | refused, `\x07` |
//! | `F` / `E` | status flags, Lawicel bit layout (below) | `Fxx\r` / `Exx\r` |
//! | `V` / `v` | version | `V0101\r` |
//! | `N` | serial: the last two bytes of the chip's MAC | `Nxxxx\r` |
//! | `Z0` | timestamps off (the only setting) | `\r`; `Z1` is `\x07` |
//!
//! Frames from the bus go up as `tiiiLdd…\r` and `Tiiiiiiiildd…\r`, a remote
//! frame as `r`/`R` with no data. The gateway's heartbeat `0x17F00010` is a
//! 29-bit id, so the `T` path is not optional.
//!
//! **Silence means listen-only.** `M1` before `O` (which is what the client's
//! `SlcanMode::Silent` sends) starts the controller in `TwaiMode::ListenOnly`:
//! no acknowledge, no error flag, no frame — the mode `dev sniff` uses next to
//! another tester. `M0` is normal mode and is sent explicitly on every open
//! (see `slcan.rs`), so the board never inherits a mode from an earlier
//! session. `L` opens listen-only without touching what `M` set, as Lawicel
//! has it: `L`, `C`, `O` is back in the configured mode.
//!
//! **A refused `S` refuses the `O` after it.** The host does not read acks —
//! `SlcanBackend` sends `C`, `S6`, `M0`, `O` blind, because the CANable's
//! firmware acks nothing at all — so an `S` this controller cannot honour
//! (`S0`–`S3`, `S7`) would otherwise be followed by an `O` at whatever rate
//! was set before, and a normal-mode node at the wrong bit rate is not a silent
//! node: it sees every frame as an error and answers each with an error flag,
//! which is the one thing an adapter must never do to a car. So a refused `S`
//! unsets the rate, `O` and `L` answer `\x07` until an `S` the board has, and
//! the host sees its requests refused instead of a bus it has wrecked. The
//! `vagcan` commands only ever send `S6`.
//!
//! **Nothing but slcan traffic goes down the console.** No logger is installed
//! in this binary — not `esp_println::logger::init_logger`, not `…_from_env` —
//! so esp-hal's own `warn!`s go to the `log` facade and evaporate. A panic is
//! the one exception: `health.rs`'s handler prints it through esp-println, and
//! at that point the stream is dead anyway.
//!
//! ## Keeping up with a loaded bus, and what happens when it cannot
//!
//! A saturated 500 kbit/s bus is ≈4,000 eight-byte frames a second, and
//! esp-hal's receive queue is 32 deep — eight milliseconds, and what does not
//! fit it is dropped by esp-hal's interrupt handler without a count. So frames
//! are moved out of it as fast as they land into [`OUT`], a ring of
//! [`RING`] lines that a separate task drains into the console, whole lines
//! packed into each 64-byte USB packet. The console runs at USB speed (the
//! "baud rate" the host opens it with is decorative); a `t` line is 22 bytes,
//! a `T` line 27, so the wire needs ≈90 KB/s at the ceiling.
//!
//! The ring is sized for the host stalling, not for the bus: **half a second
//! of a saturated bus** (2,048 lines at 4,000/s; two thirds of a second at
//! the gateway's 3,106/s heartbeat storm, the heaviest thing the car does).
//! What stalls the writer is the host not reading — a tokio task descheduled
//! behind a SQLite write, a USB service interval, a laptop that is busy — and
//! those are milliseconds to tens of milliseconds, which the ring covers ten
//! times over. A stall longer than half a second is a host that has stopped
//! reading (a paused process, a terminal not draining), and no ring wins that;
//! a receiver cannot slow a bus. It costs 74 KB of the C3's 400 KB, which an
//! image with no radio and a 32 KB heap has to spare.
//!
//! What the ring cannot hold is dropped **and counted**: `F` reports it as
//! bit 3, data overrun — a frame was lost between the bus and the host — with
//! bit 0, receive queue full, saying it was this ring and not the
//! controller's FIFO. Both clear on read, as the CANable's do.
//!
//! ## Status flags (`F`, and `E` as its alias)
//!
//! Lawicel's layout: bit 0 receive queue full (frames were dropped in the
//! ring), bit 2 error warning (an error counter at or past 96), bit 3 data
//! overrun (a frame was lost — in the ring, or in the controller's own FIFO),
//! bit 5 error passive (a counter at or past 128), bit 6 arbitration lost (a
//! transmit was refused without the error counter moving, while the
//! controller was below error-passive — see "Transmit"), bit 7 bus error
//! (the controller went bus-off). The latched bits — 0, 3, 6, 7 — clear on
//! read; 2 and 5 are read live off the counters, except in listen-only mode,
//! where esp-hal parks the receive counter at 128 on purpose (an errata
//! workaround that keeps the controller error-passive so it can never drive a
//! dominant bit) and neither counter moves — so there they are not read at
//! all, and a healthy listen-only channel answers `F00`.
//!
//! A bus-off is recovered on its own: the controller is reopened with the
//! same bit rate and mode, as `dash` does, so the host sees a gap rather than
//! a dead adapter. A controller overrun is not a rebuild. What esp-hal reports
//! as `Overrun` is `MISS_ST` (status bit 8): a per-packet marker saying the
//! packet at the head of the receive FIFO is a placeholder for one the FIFO
//! had no room for, and it goes with the placeholder when `RELEASE_BUF`
//! releases it — which is how ESP-IDF handles it, one release per missing
//! packet. (`CLR_OVERRUN` clears a different bit, `OVERRUN_ST`, status bit 1,
//! which nothing here reads.) The placeholder is released in place, inside a
//! critical section so the interrupt handler cannot release the packet behind
//! it instead; only a marker that survives that — a controller in a state the
//! command does not describe — rebuilds the controller the way a bus-off is.
//! A wrinkle in esp-hal's handler: it reports the marker and then *reads* the
//! placeholder as a frame, so by the time the report reaches the bridge the
//! slot is usually already released and its read is the next thing in the
//! queue. That read is discarded — see [`take`], and the bench check named
//! there.
//!
//! ## Transmit, and what `z` promises
//!
//! esp-hal's transmit future resolves `Ok` when the transmit buffer is
//! *released* — which is what a completed frame does, and also what an abort
//! does, and esp-hal's interrupt handler aborts a pending transmit on any
//! error interrupt whose captured direction says "transmit", including a lost
//! arbitration and the acknowledge error a bus with no second node produces.
//! So `Ok` is not "sent". The controller's `tx_complete` status bit is: the
//! SJA1000 lineage clears it on a transmit request and sets it only when the
//! frame completed on the bus, acknowledge included, and ESP-IDF's own driver
//! reads the same bit for the same verdict. A transmit here is retried while
//! that bit stays clear, up to [`TX_ATTEMPTS`] times inside [`TX_TIMEOUT`],
//! receiving all the while — a request must not cost the reply — and only
//! then answered `\x07`. Whether the refusal was arbitration or an error is
//! told by the error counter: unmoved is arbitration lost, latched as `F`
//! bit 6; moved is an error, and the live bits show it — with one exception.
//! ISO 11898-1 exempts an error-passive transmitter's acknowledge error from
//! the count (`research/dash/can-bring-up.md` §3 saw exactly this on the
//! bench), so once the counter is at 128 an unmoved counter says nothing about
//! arbitration, and it is not read as it. A bench with no partner therefore
//! answers `\x07` to every transmit, climbs by eight per attempt through error
//! warning to passive, and **stays there**: a lone node never reaches bus-off.
//! Bus-off takes a loaded bus that nobody acknowledges on — there the passive
//! error flags meet real dominant bits — and that is recovered and reported as
//! bit 7. A transmit in listen-only mode or on a closed channel is refused up
//! front, and so is one whose time budget is already spent: a request with no
//! time left would be issued and aborted in the same poll.
//!
//! ## A watchdog
//!
//! The RTC watchdog is armed at boot and fed from a task that does nothing
//! else, as `dash` does — so if the bridge ever stops yielding (the spin
//! above was exactly that), the adapter reboots within [`WATCHDOG`] rather
//! than sit dead on the port until the cable is pulled.
//!
//! This binary is the adapter, so it **may see a car** — it does on the bus
//! exactly what the CANable does, which is whatever `vagcan` asks of it, and
//! `vagcan`'s allowlist is what bounds that.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_executor::Spawner;
use embassy_futures::poll_once;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_can::Frame as _;
use embedded_io_async::{Read as _, Write as _};
use esp_backtrace as _;
use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::{GPIO1, GPIO6, TWAI0};
use esp_hal::rtc_cntl::{Rwdt, RwdtStage};
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::twai::{BaudRate, ErrorKind, EspTwaiError, EspTwaiFrame, ExtendedId, Id, StandardId, TwaiConfiguration, TwaiMode, TwaiRx, TwaiTx};
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
// The `#[panic_handler]` lives in the library (`health.rs`), and its printer
// allocates, so the heap below exists for it even though this image never
// allocates in its steady state.
use vag_dash_fw as _;

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

/// One console line, either direction. The longest is a 29-bit frame with
/// eight bytes: `T` + 8 + 1 + 16 + `\r` = 27 bytes.
type Line = heapless::Vec<u8, 32>;

/// How many lines [`OUT`] holds: half a second of a saturated bus. The
/// argument is in the module docs ("Keeping up with a loaded bus").
const RING: usize = 2048;

/// Lines waiting for the console: bus frames from the bridge and replies to
/// commands, in the order they were produced.
static OUT: Channel<CriticalSectionRawMutex, Line, RING> = Channel::new();

/// Bumped by every `C`. The console writer reads it around each USB write:
/// a line it took out of [`OUT`] before the bump predates the close and is
/// dropped with the rest — [`Adapter::close`] clears the ring, but not what
/// the writer already holds.
static EPOCH: AtomicU32 = AtomicU32::new(0);

/// Depth of esp-hal's receive queue (`TwaiAsyncState::rx_queue`). It is a
/// static, so it survives the controller being rebuilt, and whatever it held
/// from before a `C` or a bus-off would otherwise come up as live after `O`.
const RX_QUEUE_DEPTH: usize = 32;

/// The most packets the controller's receive FIFO can count, placeholders
/// included: `TWAI_RX_MESSAGE_CNT_REG` is seven bits wide (esp32c3 PAC,
/// `rx_message_cnt`, bits 0:6). A run of overrun markers at the head of the
/// FIFO is at most this long, so a release loop that outlives it has found
/// something the command does not describe.
const RX_FIFO_MESSAGES: usize = 128;

/// ISO 11898-1's error-counter thresholds: at 96 the controller is in error
/// warning, at 128 it is error-passive. Properties of the protocol, not of a
/// car.
const TEC_WARNING: u8 = 96;
const TEC_PASSIVE: u8 = 128;

/// How many times a transmit is re-issued while the controller says the
/// frame did not complete. What it recovers from is a lost arbitration — one
/// heartbeat is one loss, and eight in a row is a bus that is not letting
/// this frame on. On a bench with no partner every attempt costs the transmit
/// error counter eight, so one refused transmit ends at 64: under the warning
/// limit, and the refusal itself is the report.
const TX_ATTEMPTS: usize = 8;

/// How long all the attempts together may take. A frame on an acknowledged
/// bus is through in well under a millisecond; a bus nobody acknowledges
/// aborts the attempt in the interrupt handler in about as long. This bounds
/// the case in between — a transmit the controller neither completes nor
/// aborts — which is what the future's own drop then cancels.
const TX_TIMEOUT: Duration = Duration::from_millis(100);

/// The watchdog's patience: nothing legitimate on this executor blocks for
/// even a tenth of it, and an adapter that is dead for longer is one the host
/// has already given up on.
const WATCHDOG: esp_hal::time::Duration = esp_hal::time::Duration::from_secs(4);

/// Feed interval — a quarter of the timeout, so a missed feed or two is not a
/// reboot.
const FEED_EVERY: Duration = Duration::from_millis(1000);

/// Lawicel `V` answer: hardware 01, software 01.
const VERSION: &[u8] = b"V0101\r";

/// Lawicel error/success replies.
const OK: u8 = b'\r';
const BELL: u8 = 0x07;

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
	// Deliberately no logger: the console is the protocol.
	let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
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

/// Drains [`OUT`] into the console, whole lines packed into each USB packet.
///
/// The USB-Serial-JTAG hands the host 64 bytes per packet and esp-hal's
/// writer waits for each packet to leave, so one packet per write is the
/// natural unit: packing lines into it is what keeps the wire ahead of the
/// bus, and never splitting a line across packets is what lets a close
/// discard the writer's hand without leaving the host half a line. Latency
/// is unaffected — a lone line is written the moment it arrives.
#[embassy_executor::task]
async fn console_tx(mut usb: UsbSerialJtagTx<'static, Async>) -> ! {
	let mut packet: heapless::Vec<u8, 64> = heapless::Vec::new();
	// A line taken out of the ring that did not fit the packet it was taken
	// for. It is the one thing this task holds that a `C` cannot see.
	let mut carry: Option<Line> = None;
	loop {
		let first = match carry.take() {
			Some(line) => line,
			None => OUT.receive().await,
		};
		// Everything taken from the ring between here and the write belongs
		// to this epoch: the bridge cannot run in between, because there is no
		// await in between and the executor is cooperative.
		let epoch = EPOCH.load(Ordering::Relaxed);
		packet.clear();
		let _ = packet.extend_from_slice(&first);
		while let Ok(line) = OUT.try_receive() {
			// `extend_from_slice` is all-or-nothing, so a line that does not
			// fit is still whole, and goes first into the next packet.
			if packet.extend_from_slice(&line).is_err() {
				carry = Some(line);
				break;
			}
		}
		// Cannot fail on this peripheral; if the host is not reading, it waits,
		// and the bridge keeps counting what it has to drop meanwhile.
		let _ = usb.write_all(&packet).await;
		if EPOCH.load(Ordering::Relaxed) != epoch {
			// A `C` came while the packet was going out: what was taken before
			// it predates the close, and the ring it came from is already empty.
			carry = None;
		}
	}
}

/// Console bytes in, commands out; bus frames in, console lines out. One task
/// owns both ends so that nothing about the controller — open, closed, which
/// mode — is ever shared.
#[embassy_executor::task]
async fn bridge(mut usb: UsbSerialJtagRx<'static, Async>, twai0: TWAI0<'static>, rx_pin: GPIO1<'static>, tx_pin: GPIO6<'static>) -> ! {
	let mut adapter = Adapter {
		twai0,
		rx_pin,
		tx_pin,
		bus: None,
		bitrate: Some(BaudRate::B500K),
		mode: TwaiMode::Normal,
		intake: Intake::default(),
	};
	let mut parser = LineParser::default();
	let mut chunk = [0u8; 64];

	loop {
		// The borrow of the open controller ends with the event; the handling
		// below needs the adapter whole again (a `C` drops the controller).
		let event = match adapter.bus.as_mut() {
			None => Event::Console(usb.read(&mut chunk).await.unwrap_or(0)),
			Some(bus) => match select(usb.read(&mut chunk), bus.rx.receive_async()).await {
				Either::First(n) => Event::Console(n.unwrap_or(0)),
				Either::Second(result) => Event::Bus(result),
			},
		};
		match event {
			Event::Console(n) => {
				for &byte in &chunk[..n] {
					if let Some(line) = parser.feed(byte) {
						let reply = adapter.command(&line).await;
						adapter.answer(reply).await;
					}
				}
			}
			Event::Bus(result) => {
				if let Err(fault) = take(result, &mut adapter.intake) {
					adapter.repair(fault);
				}
			}
		}
	}
}

enum Event {
	/// This many bytes arrived from the host.
	Console(usize),
	/// The controller produced a frame, or an error in its place.
	Bus(Result<EspTwaiFrame, EspTwaiError>),
}

/// Lawicel `F` status bits.
mod flags {
	pub const RX_QUEUE_FULL: u8 = 0x01;
	pub const ERROR_WARNING: u8 = 0x04;
	pub const DATA_OVERRUN: u8 = 0x08;
	pub const ERROR_PASSIVE: u8 = 0x20;
	pub const ARBITRATION_LOST: u8 = 0x40;
	pub const BUS_ERROR: u8 = 0x80;
}

/// The controller, once opened: the two halves esp-hal splits it into, so
/// that a transmit can be awaited while reception carries on, and what it
/// was opened with — the mode `L` may have chosen for this open alone, and
/// the bit rate, so a repair can rebuild it without asking the adapter.
struct Bus {
	rx: TwaiRx<'static, Async>,
	tx: TwaiTx<'static, Async>,
	mode: TwaiMode,
	bitrate: BaudRate,
}

struct Adapter {
	twai0: TWAI0<'static>,
	rx_pin: GPIO1<'static>,
	tx_pin: GPIO6<'static>,
	bus: Option<Bus>,
	/// What `S` chose, for the next `O`. `None` after an `S` this controller
	/// cannot honour: the next `O` is refused rather than opened at a rate the
	/// host did not ask for (the module docs say why that matters).
	bitrate: Option<BaudRate>,
	/// What `M` chose, for the next `O`.
	mode: TwaiMode,
	intake: Intake,
}

/// What the bridge keeps between receives: the `F` bits waiting to be read,
/// the count of frames the ring had no room for, and the one piece of
/// look-ahead a controller overrun needs.
#[derive(Default)]
struct Intake {
	/// `F` bits that stay set until somebody reads them.
	latched: u8,
	/// Frames that arrived while [`OUT`] was full. Reported as `F` bits 0
	/// and 3 and zeroed by that read. Only this task counts, so no atomic is
	/// needed — and the `imc` core has no atomic read-modify-write to offer
	/// anyway.
	dropped: u32,
	/// The next `Ok` out of the receive queue is esp-hal's read of a released
	/// overrun placeholder, not a frame — see [`take`].
	skip: bool,
}

/// What one receive produced: a frame goes up the ring, a fault into the
/// latched flags. `Err` is a fault the controller has to be rebuilt for —
/// bus-off, or an overrun marker that releasing did not clear — and carries
/// the error so the caller can say which.
///
/// A controller overrun arrives as `Err(Overrun)` and, usually, one frame
/// that is not one. esp-hal's interrupt handler reports `MISS_ST` and then
/// `read_frame`s the same slot — the placeholder — and queues what it read as
/// `Ok`, right behind the report; ESP-IDF releases that slot without reading
/// it. So when the report finds the placeholder already gone (the handler
/// released it), the `Ok` that follows is discarded. What this cannot see is
/// whether the handler queued anything: it queues nothing when the
/// placeholder's header decodes to a DLC over 8, or when the report took the
/// queue's last slot — and then the first real frame after the overrun is
/// discarded instead. If the handler ran between the report and this call,
/// its own report of the same placeholder comes next and lands here again,
/// finds the marker clear, and sets the same flag: nothing is skipped twice.
///
/// **Bench check, not yet run:** stall the host's reader on a busy bus (a
/// `dev sniff` paused with `^Z`), resume, and look in the recording for ids
/// that were never on the bus — a placeholder read as a frame, the flag not
/// working — and for one frame missing right after each `F` bit 3, the flag
/// eating a real one because the handler queued nothing.
fn take(result: Result<EspTwaiFrame, EspTwaiError>, intake: &mut Intake) -> Result<(), EspTwaiError> {
	match result {
		Ok(frame) => {
			if core::mem::take(&mut intake.skip) {
				return Ok(());
			}
			if OUT.try_send(encode(&frame)).is_err() {
				intake.dropped = intake.dropped.saturating_add(1);
			}
			Ok(())
		}
		Err(EspTwaiError::BusOff) => {
			intake.latched |= flags::BUS_ERROR;
			Err(EspTwaiError::BusOff)
		}
		Err(EspTwaiError::EmbeddedHAL(ErrorKind::Overrun)) => {
			intake.latched |= flags::DATA_OVERRUN;
			match release_missing()? {
				Missing::Consumed => intake.skip = true,
				Missing::Released => {}
			}
			Ok(())
		}
		// A frame the controller could not decode (a non-compliant DLC, a
		// bus error it attributed to a frame): nothing to forward.
		Err(_) => Ok(()),
	}
}

/// What became of the placeholder a controller overrun leaves at the head
/// of the receive FIFO.
enum Missing {
	/// esp-hal's interrupt handler had already released it — and read it as
	/// a frame first, which is now the next item in its queue.
	Consumed,
	/// It was still at the head, and is released here.
	Released,
}

/// Release the placeholder at the head of the receive FIFO with the
/// controller's own command — `RELEASE_BUF` in `TWAI_CMD_REG`, the one
/// ESP-IDF issues for a packet whose `MISS_ST` is set — and say what was
/// found there. (`CLR_OVERRUN` is the wrong command: it clears `OVERRUN_ST`,
/// a different bit, and leaves the marker and the placeholder where they
/// are.) Checked and released in one critical section: the interrupt handler
/// releases the head too, and a release that lands after its release would
/// discard the real packet behind. `Err` is a marker that outlives every
/// release the FIFO could need, which is a controller in a state the command
/// does not describe; the caller rebuilds it.
fn release_missing() -> Result<Missing, EspTwaiError> {
	critical_section::with(|_| {
		let regs = TWAI0::regs();
		if regs.status().read().miss_st().bit_is_clear() {
			return Ok(Missing::Consumed);
		}
		for _ in 0..RX_FIFO_MESSAGES {
			regs.cmd().write(|w| w.release_buf().set_bit());
			if regs.status().read().miss_st().bit_is_clear() {
				return Ok(Missing::Released);
			}
		}
		Err(EspTwaiError::EmbeddedHAL(ErrorKind::Overrun))
	})
}

/// Run `work` while frames keep going up. Ends early if the controller
/// faults (bus-off, an overrun marker that would not clear), which the
/// caller answers by rebuilding it; `work` is dropped then, which for a
/// transmit aborts the attempt.
async fn attend<F: Future>(rx: &mut TwaiRx<'static, Async>, intake: &mut Intake, work: F) -> Result<F::Output, EspTwaiError> {
	let receive = async {
		loop {
			if let Err(fault) = take(rx.receive_async().await, intake) {
				return fault;
			}
		}
	};
	match select(work, receive).await {
		Either::First(out) => Ok(out),
		Either::Second(fault) => Err(fault),
	}
}

impl Adapter {
	/// Start the controller with the given bit rate and mode. An open
	/// controller is dropped first — that releases the peripheral (its clock
	/// gates off with the last guard) so the new configuration starts from
	/// reset, which is also what clears the error counters and the overrun
	/// status. The command handler refuses `O` on an open channel; this is
	/// reached with one open only from [`Adapter::repair`].
	fn open(&mut self, bitrate: BaudRate, mode: TwaiMode) {
		self.bus = None;
		// SAFETY: `clone_unchecked` asks that a clone and its original never
		// both drive the peripheral. The originals — `self.twai0`,
		// `self.rx_pin`, `self.tx_pin` — stay alive beside the clones for the
		// life of the task, and the invariant kept here is that they are never
		// used for anything but making the next clone, and only while
		// `self.bus` is `None` (the line above): the clones live inside
		// `self.bus` and drive the peripheral; the originals are inert. The
		// safe `reborrow()` cannot be used because the result has to outlive
		// this call. (esp-hal's own `start()` clones the same handle again for
		// the two halves and keeps its invariant the same way.)
		let (twai0, rx_pin, tx_pin) = unsafe { (self.twai0.clone_unchecked(), self.rx_pin.clone_unchecked(), self.tx_pin.clone_unchecked()) };
		// No acceptance filter: an adapter forwards everything, and the host
		// decides what it wanted. (`TwaiConfiguration::new` installs accept-all.)
		let config = TwaiConfiguration::new(twai0, rx_pin, tx_pin, bitrate, mode);
		let mut twai = config.into_async().start();
		// esp-hal's receive queue is a static: what it held from before the
		// rebuild — frames from before a `C`, the errors of a bus-off — would
		// come up as live after this open. Drain it with polls that cannot
		// block; a fresh controller answers `Pending` once the queue is empty.
		// The bound is against a status bit that would answer `Ready` forever.
		//
		// The drain runs after `start()` because it has to: the queue lives in
		// esp-hal's `TwaiAsyncState`, reachable only through a private trait
		// and so only through the receive future of a started controller. A
		// frame that lands in the tens of microseconds the loop takes is
		// drained with the stale ones. Accepted: nothing on the bus in that
		// window can be an answer to the host, which has not had its `\r` for
		// this open yet and so has sent nothing through it.
		for _ in 0..=RX_QUEUE_DEPTH {
			if poll_once(twai.receive_async()).is_pending() {
				break;
			}
		}
		// A skip pending from before the rebuild referred to a queue that has
		// just been drained; left set it would eat the first real frame.
		self.intake.skip = false;
		let (rx, tx) = twai.split();
		self.bus = Some(Bus { rx, tx, mode, bitrate });
	}

	/// After a fault: the same settings again, nothing forgotten. Which fault
	/// is already in the latched flags; `BusOff` from the transmit future is
	/// the one path that has not latched it yet.
	fn repair(&mut self, fault: EspTwaiError) {
		if matches!(fault, EspTwaiError::BusOff) {
			self.intake.latched |= flags::BUS_ERROR;
		}
		if let Some(bus) = &self.bus {
			let (bitrate, mode) = (bus.bitrate, bus.mode);
			self.open(bitrate, mode);
		}
	}

	fn close(&mut self) {
		self.bus = None;
		// Everything queued for the host predates the close, and so does the
		// line the writer may already hold: the epoch is how it learns that.
		// Only this task stores, so load-then-store is a whole increment.
		EPOCH.store(EPOCH.load(Ordering::Relaxed).wrapping_add(1), Ordering::Relaxed);
		OUT.clear();
	}

	/// A reply goes into the ring whatever the ring holds — the host that
	/// asked is waiting for it, unlike a bus frame, which has a successor.
	/// While it waits for room the bus is still attended: what arrives then
	/// has nowhere to go and is counted here, rather than left to overflow
	/// esp-hal's queue where nothing counts it.
	async fn answer(&mut self, reply: Line) {
		let Some(Bus { rx, .. }) = self.bus.as_mut() else {
			OUT.send(reply).await;
			return;
		};
		let copy = reply.clone();
		if let Err(fault) = attend(rx, &mut self.intake, OUT.send(reply)).await {
			// The send was dropped with the fault; the reply still has to go.
			self.repair(fault);
			OUT.send(copy).await;
		}
	}

	/// Execute one command line (without its `\r`) and produce the reply.
	async fn command(&mut self, line: &[u8]) -> Line {
		let Some((&head, args)) = line.split_first() else {
			// An empty line: Lawicel says ignore it, and an empty reply is
			// the closest thing to that which still tells a terminal it was
			// heard.
			return reply(OK);
		};
		match head {
			b'C' => {
				self.close();
				reply(OK)
			}
			// Open commands on an open channel: refused, as Lawicel specifies.
			// Rebuilding instead would zero the error counters and drop the
			// receive FIFO behind a host that asked for nothing of the kind.
			b'O' | b'L' if self.bus.is_some() => reply(BELL),
			// `L` is one-shot, as Lawicel has it: the next `O` is back in `M`'s
			// mode. No rate means the last `S` was one this controller does not
			// have, and the module docs say why that refuses the open.
			b'O' | b'L' => match self.bitrate {
				Some(bitrate) => {
					self.open(bitrate, if head == b'O' { self.mode } else { TwaiMode::ListenOnly });
					reply(OK)
				}
				None => reply(BELL),
			},
			b'S' if self.bus.is_none() => match args {
				b"4" => self.set_bitrate(BaudRate::B125K),
				b"5" => self.set_bitrate(BaudRate::B250K),
				b"6" => self.set_bitrate(BaudRate::B500K),
				b"8" => self.set_bitrate(BaudRate::B1000K),
				// A rate the controller does not have. The host does not read
				// this refusal, so the rate is unset and the `O` that follows
				// is refused in its place.
				_ => {
					self.bitrate = None;
					reply(BELL)
				}
			},
			b'M' if self.bus.is_none() => match args {
				b"0" => {
					self.mode = TwaiMode::Normal;
					reply(OK)
				}
				b"1" => {
					self.mode = TwaiMode::ListenOnly;
					reply(OK)
				}
				_ => reply(BELL),
			},
			// Setup commands while the channel is open: refused, as Lawicel
			// specifies — the controller latches its configuration at open.
			b'S' | b'M' => reply(BELL),
			b't' | b'T' => self.transmit(head, args).await,
			b'F' | b'E' => {
				let mut out = Line::new();
				let _ = out.push(head);
				push_hex(&mut out, self.flags());
				let _ = out.push(OK);
				out
			}
			b'V' | b'v' => Line::from_slice(VERSION).unwrap_or_default(),
			b'N' => {
				let mac = esp_hal::efuse::Efuse::mac_address();
				let mut out = Line::new();
				let _ = out.push(b'N');
				push_hex(&mut out, mac[4]);
				push_hex(&mut out, mac[5]);
				let _ = out.push(OK);
				out
			}
			b'Z' if args == b"0" => reply(OK),
			_ => reply(BELL),
		}
	}

	fn set_bitrate(&mut self, rate: BaudRate) -> Line {
		self.bitrate = Some(rate);
		reply(OK)
	}

	/// `F`: the latched bits, cleared by this read, plus the live error state.
	fn flags(&mut self) -> u8 {
		let mut f = core::mem::take(&mut self.intake.latched);
		if core::mem::take(&mut self.intake.dropped) > 0 {
			f |= flags::RX_QUEUE_FULL | flags::DATA_OVERRUN;
		}
		// The counters mean something only in normal mode. In listen-only
		// esp-hal's `start()` sets the receive counter to 128 on purpose
		// (errata: error-passive can never drive a dominant bit) and the
		// controller freezes both, so reading them there would answer `F24`
		// on a healthy bus forever.
		if let Some(Bus { mode: TwaiMode::Normal, .. }) = &self.bus {
			let regs = TWAI0::regs();
			let tec = regs.tx_err_cnt().read().tx_err_cnt().bits();
			let rec = regs.rx_err_cnt().read().rx_err_cnt().bits();
			if tec >= TEC_WARNING || rec >= TEC_WARNING {
				f |= flags::ERROR_WARNING;
			}
			if tec >= TEC_PASSIVE || rec >= TEC_PASSIVE {
				f |= flags::ERROR_PASSIVE;
			}
			if regs.status().read().bus_off_st().bit_is_set() {
				f |= flags::BUS_ERROR;
			}
		}
		f
	}

	/// `t`/`T`: put the frame on the bus and wait for the controller to say
	/// it completed, taking in whatever arrives meanwhile — see the module
	/// docs, "Transmit, and what `z` promises".
	async fn transmit(&mut self, head: u8, args: &[u8]) -> Line {
		let Some(frame) = parse_frame(head, args) else {
			return reply(BELL);
		};
		let deadline = Instant::now() + TX_TIMEOUT;
		for _ in 0..TX_ATTEMPTS {
			let Some(Bus { rx, tx, mode, .. }) = self.bus.as_mut() else {
				return reply(BELL);
			};
			if *mode == TwaiMode::ListenOnly {
				return reply(BELL);
			}
			let left = deadline.saturating_duration_since(Instant::now());
			if left.as_ticks() == 0 {
				// The budget is spent with attempts to spare. A zero timeout is
				// not a refusal: `with_timeout` polls the transmit once before
				// its timer — which issues the request — and then drops it,
				// which aborts a frame that may already be on the wire.
				return reply(BELL);
			}
			let regs = TWAI0::regs();
			let tec_before = regs.tx_err_cnt().read().tx_err_cnt().bits();
			// Frames keep flowing up while the transmit is in flight — on a
			// car the answer to this frame is among them, and the ISO-TP flow
			// control that follows a first frame is what the host is waiting
			// for.
			let attempt = with_timeout(left, attend(rx, &mut self.intake, tx.transmit_async(&frame))).await;
			match attempt {
				Ok(Ok(Ok(()))) => {
					// The buffer was released. Completed, or aborted by esp-hal's
					// interrupt handler: `tx_complete` is the controller's verdict.
					if regs.status().read().tx_complete().bit_is_set() {
						return reply(if head == b't' { b'z' } else { b'Z' }).with(OK);
					}
					let tec_after = regs.tx_err_cnt().read().tx_err_cnt().bits();
					// An unmoved counter means nothing went wrong on the wire —
					// somebody else's frame had the lower id — unless the
					// controller is already error-passive: there an acknowledge
					// error does not move it either (ISO 11898-1's exception;
					// can-bring-up.md §3 saw it), so it says nothing, and the
					// live bits of `F` are the report.
					if tec_before < TEC_PASSIVE && tec_after <= tec_before {
						self.intake.latched |= flags::ARBITRATION_LOST;
					}
					// A moved counter is an error the live bits show; either
					// way, again.
				}
				// The transmit future's own bus-off, or a fault on the receive
				// side that ended the attempt: rebuild, and refuse.
				Ok(Ok(Err(fault))) | Ok(Err(fault)) => {
					self.repair(fault);
					return reply(BELL);
				}
				// Neither completed nor aborted in time; the future's drop has
				// cancelled the request.
				Err(_) => return reply(BELL),
			}
		}
		reply(BELL)
	}
}

/// A one-byte reply.
fn reply(byte: u8) -> Line {
	let mut out = Line::new();
	let _ = out.push(byte);
	out
}

trait With {
	fn with(self, byte: u8) -> Self;
}

impl With for Line {
	fn with(mut self, byte: u8) -> Self {
		let _ = self.push(byte);
		self
	}
}

/// Bytes into command lines. `\r` ends a line, `\n` is ignored so a terminal
/// user's Enter works, and a line too long to be any command is discarded
/// whole: a desynchronised stream, not a command.
#[derive(Default)]
struct LineParser {
	line: Line,
	overflow: bool,
}

impl LineParser {
	fn feed(&mut self, byte: u8) -> Option<Line> {
		match byte {
			b'\r' => {
				// An overflowed line comes back as the error byte alone, which
				// no command matches, so it is answered with `\x07`.
				let done = if self.overflow { reply(BELL) } else { core::mem::take(&mut self.line) };
				self.line.clear();
				self.overflow = false;
				Some(done)
			}
			b'\n' => None,
			_ => {
				if self.line.push(byte).is_err() {
					self.overflow = true;
				}
				None
			}
		}
	}
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn push_hex(out: &mut Line, byte: u8) {
	let _ = out.push(HEX[usize::from(byte >> 4)]);
	let _ = out.push(HEX[usize::from(byte & 0xF)]);
}

fn nibble(byte: u8) -> Option<u32> {
	match byte {
		b'0'..=b'9' => Some(u32::from(byte - b'0')),
		b'a'..=b'f' => Some(u32::from(byte - b'a' + 10)),
		b'A'..=b'F' => Some(u32::from(byte - b'A' + 10)),
		_ => None,
	}
}

fn hex_field(bytes: &[u8]) -> Option<u32> {
	bytes.iter().try_fold(0u32, |acc, &b| Some((acc << 4) | nibble(b)?))
}

/// `t` + 3 hex id + 1 hex length + 2 hex per byte, or `T` with 8 hex of id.
/// Anything after the data (a timestamp a host might append) is ignored, as
/// the host's own decoder ignores it.
fn parse_frame(head: u8, args: &[u8]) -> Option<EspTwaiFrame> {
	let id_len = if head == b't' { 3 } else { 8 };
	let id = hex_field(args.get(..id_len)?)?;
	let dlc = nibble(*args.get(id_len)?)? as usize;
	if dlc > 8 {
		return None;
	}
	let mut data = [0u8; 8];
	for (i, slot) in data.iter_mut().enumerate().take(dlc) {
		let at = id_len + 1 + i * 2;
		*slot = hex_field(args.get(at..at + 2)?)? as u8;
	}
	let id = if head == b't' {
		Id::Standard(StandardId::new(id as u16)?)
	} else {
		Id::Extended(ExtendedId::new(id)?)
	};
	EspTwaiFrame::new(id, &data[..dlc])
}

/// A received frame as its slcan line, `\r` included.
fn encode(frame: &EspTwaiFrame) -> Line {
	let mut out = Line::new();
	let remote = frame.is_remote_frame();
	match frame.id() {
		embedded_can::Id::Standard(id) => {
			let _ = out.push(if remote { b'r' } else { b't' });
			let raw = id.as_raw();
			let _ = out.push(HEX[usize::from((raw >> 8) & 0xF)]);
			push_hex(&mut out, (raw & 0xFF) as u8);
		}
		embedded_can::Id::Extended(id) => {
			let _ = out.push(if remote { b'R' } else { b'T' });
			let raw = id.as_raw();
			for byte in raw.to_be_bytes() {
				push_hex(&mut out, byte);
			}
		}
	}
	let _ = out.push(HEX[frame.dlc().min(8)]);
	if !remote {
		for &byte in frame.data() {
			push_hex(&mut out, byte);
		}
	}
	let _ = out.push(OK);
	out
}
