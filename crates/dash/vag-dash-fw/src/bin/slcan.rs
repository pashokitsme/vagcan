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
//! What it honours is what `slcan.rs` on the host sends, plus the handful of
//! Lawicel commands a terminal user or a probe would type:
//!
//! | command | does | reply |
//! |---|---|---|
//! | `C` | close the channel; drops queued frames | `\r` |
//! | `S4` `S5` `S6` `S8` | 125 / 250 / 500 / 1000 kbit/s, closed only | `\r`, else `\x07` |
//! | `M0` / `M1` | normal / listen-only for the next open, closed only | `\r` |
//! | `O` | open in the configured mode | `\r` |
//! | `L` | open listen-only, and remember it as `M1` would | `\r` |
//! | `tiiiLdd…` / `Tiiiiiiiildd…` | transmit an 11- / 29-bit frame | `z\r` / `Z\r`, `\x07` if refused or unacknowledged |
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
//! session.
//!
//! **Nothing but slcan traffic goes down the console.** No logger is installed
//! in this binary — not `esp_println::logger::init_logger`, not `…_from_env` —
//! so esp-hal's own `warn!`s go to the `log` facade and evaporate. A panic is
//! the one exception: `health.rs`'s handler prints it through esp-println, and
//! at that point the stream is dead anyway.
//!
//! ## Keeping up with a loaded bus
//!
//! A saturated 500 kbit/s bus is ≈4,000 eight-byte frames a second, and
//! esp-hal's receive queue is 32 deep — eight milliseconds. So frames are
//! moved out of it as fast as they land into [`OUT`], a 512-line channel that
//! a separate task drains into the console, several lines per USB write. The
//! console runs at USB speed (the "baud rate" the host opens it with is
//! decorative), and a `t` line is 22 bytes, so the wire needs ≈90 KB/s at the
//! ceiling. What the channel cannot hold is dropped and counted: `F` reports
//! it as bit 0, "receive FIFO full", until read.
//!
//! ## Status flags (`F`, and `E` as its alias)
//!
//! Lawicel's layout: bit 0 receive queue full (frames were dropped here),
//! bit 2 error warning (an error counter at or past 96), bit 3 data overrun
//! (the controller's own FIFO overflowed), bit 5 error passive (a counter at
//! or past 128), bit 7 bus error (the controller went bus-off). The latched
//! bits — 0, 3, 7 — clear on read; 2 and 5 are read live off the counters.
//! A bus-off is recovered on its own: the controller is reopened with the
//! same bit rate and mode, as `dash` does, so the host sees a gap rather than
//! a dead adapter.
//!
//! ## Transmit and refusal
//!
//! A transmit waits up to [`TX_TIMEOUT`] for the controller to see its frame
//! acknowledged, receiving all the while — a request must not cost the reply.
//! If nobody acknowledges (a bench with no second node in normal mode) the
//! controller retries on its own until it goes bus-off; that is reported as
//! `\x07`, the controller is reopened, and the next `F` shows bit 7. A
//! transmit in listen-only mode or on a closed channel is refused up front.
//!
//! This binary is the adapter, so it **may see a car** — it does on the bus
//! exactly what the CANable does, which is whatever `vagcan` asks of it, and
//! `vagcan`'s allowlist is what bounds that.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, with_timeout};
use embedded_can::Frame as _;
use embedded_io_async::{Read as _, Write as _};
use esp_backtrace as _;
use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::peripherals::{GPIO1, GPIO6, TWAI0};
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::twai::{BaudRate, EspTwaiError, EspTwaiFrame, ExtendedId, Id, StandardId, TwaiConfiguration, TwaiMode, TwaiRx, TwaiTx};
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

/// Lines waiting for the console: bus frames from the bridge and replies to
/// commands, in the order they were produced.
static OUT: Channel<CriticalSectionRawMutex, Line, 512> = Channel::new();

/// How long a transmit may wait for its acknowledge before it is abandoned.
/// A frame on an acknowledged bus is through in well under a millisecond; a
/// bus nobody acknowledges takes the controller to bus-off in a few, which
/// ends the wait early with an error. This only bounds the case in between.
const TX_TIMEOUT: Duration = Duration::from_millis(100);

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

	let (usb_rx, usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async().split();

	// Never `.ok()` a spawn: the arena is finite and a full one fails silently.
	// With no console to say so, a failed spawn would be an adapter that
	// enumerates and answers nothing — so it halts here instead, and the
	// symptom is a port that opens and never replies to `V`.
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

/// Drains [`OUT`] into the console, as many whole lines per write as fit in
/// one buffer. Each 64-byte USB packet costs an interrupt round trip, so
/// coalescing lines is what keeps the wire ahead of the bus; latency is
/// unaffected, because a lone line is written the moment it arrives.
#[embassy_executor::task]
async fn console_tx(mut usb: UsbSerialJtagTx<'static, Async>) -> ! {
	let mut buf: heapless::Vec<u8, 1024> = heapless::Vec::new();
	loop {
		let first = OUT.receive().await;
		buf.clear();
		let _ = buf.extend_from_slice(&first);
		while buf.capacity() - buf.len() >= first.capacity() {
			match OUT.try_receive() {
				Ok(line) => {
					let _ = buf.extend_from_slice(&line);
				}
				Err(_) => break,
			}
		}
		// Cannot fail on this peripheral; if the host is not reading, it waits,
		// and the bridge keeps counting what it has to drop meanwhile.
		let _ = usb.write_all(&buf).await;
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
		bitrate: BaudRate::B500K,
		mode: TwaiMode::Normal,
		latched: 0,
		dropped: 0,
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
				Either::Second(frame) => Event::Bus(frame),
			},
		};
		match event {
			Event::Console(n) => {
				for &byte in &chunk[..n] {
					if let Some(line) = parser.feed(byte) {
						let reply = adapter.command(&line).await;
						// A reply is never dropped: the client that asked is waiting
						// for it, unlike a bus frame, which has a successor.
						OUT.send(reply).await;
					}
				}
			}
			Event::Bus(Ok(frame)) => {
				if OUT.try_send(encode(&frame)).is_err() {
					adapter.dropped += 1;
				}
			}
			Event::Bus(Err(EspTwaiError::BusOff)) => {
				adapter.latched |= flags::BUS_ERROR;
				adapter.reopen();
			}
			Event::Bus(Err(EspTwaiError::EmbeddedHAL(esp_hal::twai::ErrorKind::Overrun))) => {
				adapter.latched |= flags::DATA_OVERRUN;
			}
			// A frame the controller could not decode (a non-compliant DLC, a
			// bus error it attributed to a frame): nothing to forward.
			Event::Bus(Err(_)) => {}
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
	pub const BUS_ERROR: u8 = 0x80;
}

/// The controller, once opened: the two halves esp-hal splits it into, so
/// that a transmit can be awaited while reception carries on.
struct Bus {
	rx: TwaiRx<'static, Async>,
	tx: TwaiTx<'static, Async>,
}

struct Adapter {
	twai0: TWAI0<'static>,
	rx_pin: GPIO1<'static>,
	tx_pin: GPIO6<'static>,
	bus: Option<Bus>,
	bitrate: BaudRate,
	mode: TwaiMode,
	/// `F` bits that stay set until somebody reads them.
	latched: u8,
	/// Frames that arrived while [`OUT`] was full. Reported as `F` bit 0 and
	/// zeroed by that read. Only this task counts, so no atomic is needed —
	/// and the `imc` core has no atomic read-modify-write to offer anyway.
	dropped: u32,
}

impl Adapter {
	/// Start the controller with the configured bit rate and mode. An open
	/// controller is dropped first — that releases the peripheral (its clock
	/// gates off with the last guard) so the new configuration starts from
	/// reset, which is also what clears the error counters.
	fn open(&mut self) {
		self.bus = None;
		// SAFETY: the driver instances built from the previous clones were
		// dropped on the line above, so exactly one instance of each peripheral
		// handle is live at a time — the condition `clone_unchecked` asks for.
		// This is the same reborrow esp-hal's own `start()` performs; the safe
		// `reborrow()` cannot be used because the result has to outlive this
		// call, and the borrow checker cannot see that `self.bus` was emptied.
		let (twai0, rx_pin, tx_pin) = unsafe { (self.twai0.clone_unchecked(), self.rx_pin.clone_unchecked(), self.tx_pin.clone_unchecked()) };
		// No acceptance filter: an adapter forwards everything, and the host
		// decides what it wanted. (`TwaiConfiguration::new` installs accept-all.)
		let config = TwaiConfiguration::new(twai0, rx_pin, tx_pin, self.bitrate, self.mode);
		let (rx, tx) = config.into_async().start().split();
		self.bus = Some(Bus { rx, tx });
	}

	/// After a bus-off: the same settings again, nothing forgotten.
	fn reopen(&mut self) {
		self.open();
	}

	fn close(&mut self) {
		self.bus = None;
		OUT.clear();
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
			b'O' => {
				self.open();
				reply(OK)
			}
			b'L' => {
				self.mode = TwaiMode::ListenOnly;
				self.open();
				reply(OK)
			}
			b'S' if self.bus.is_none() => match args {
				b"4" => self.set_bitrate(BaudRate::B125K),
				b"5" => self.set_bitrate(BaudRate::B250K),
				b"6" => self.set_bitrate(BaudRate::B500K),
				b"8" => self.set_bitrate(BaudRate::B1000K),
				_ => reply(BELL),
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
		self.bitrate = rate;
		reply(OK)
	}

	/// `F`: the latched bits, cleared by this read, plus the live error state.
	fn flags(&mut self) -> u8 {
		let mut f = core::mem::take(&mut self.latched);
		if core::mem::take(&mut self.dropped) > 0 {
			f |= flags::RX_QUEUE_FULL;
		}
		if self.bus.is_some() {
			let regs = TWAI0::regs();
			let tec = regs.tx_err_cnt().read().tx_err_cnt().bits();
			let rec = regs.rx_err_cnt().read().rx_err_cnt().bits();
			if tec >= 96 || rec >= 96 {
				f |= flags::ERROR_WARNING;
			}
			if tec >= 128 || rec >= 128 {
				f |= flags::ERROR_PASSIVE;
			}
			if regs.status().read().bus_off_st().bit_is_set() {
				f |= flags::BUS_ERROR;
			}
		}
		f
	}

	/// `t`/`T`: put the frame on the bus and wait for its acknowledge, taking
	/// in whatever arrives meanwhile.
	async fn transmit(&mut self, head: u8, args: &[u8]) -> Line {
		if self.mode == TwaiMode::ListenOnly {
			return reply(BELL);
		}
		let Some(frame) = parse_frame(head, args) else {
			return reply(BELL);
		};
		let Some(Bus { rx, tx }) = self.bus.as_mut() else {
			return reply(BELL);
		};
		let dropped = &mut self.dropped;
		let outcome = with_timeout(TX_TIMEOUT, async {
			// Frames keep flowing up while the transmit is in flight — on a
			// car the answer to this frame is among them, and the ISO-TP flow
			// control that follows a first frame is what the host is waiting
			// for. The receive arm never finishes; the select ends with the
			// transmit.
			let receive = async {
				loop {
					match rx.receive_async().await {
						Ok(frame) => {
							if OUT.try_send(encode(&frame)).is_err() {
								*dropped += 1;
							}
						}
						Err(EspTwaiError::BusOff) => return EspTwaiError::BusOff,
						Err(_) => {}
					}
				}
			};
			match select(tx.transmit_async(&frame), receive).await {
				Either::First(sent) => sent,
				Either::Second(err) => Err(err),
			}
		})
		.await;
		match outcome {
			Ok(Ok(())) => reply(if head == b't' { b'z' } else { b'Z' }).with(OK),
			Ok(Err(EspTwaiError::BusOff)) => {
				self.latched |= flags::BUS_ERROR;
				self.reopen();
				reply(BELL)
			}
			// Timed out (the future's drop aborted the attempt) or another error.
			_ => reply(BELL),
		}
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
