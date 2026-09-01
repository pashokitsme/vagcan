//! Staying alive in a car, where nobody is watching.
//!
//! Everything else in this firmware assumes a laptop on the other end of the
//! USB cable. In the car there is none: the board hangs off OBD pin 16, there
//! is no console, no reset button anybody will press, and the only person who
//! could notice a hung dash is driving. Three things follow from that, and this
//! module is all three.
//!
//! # 1. A watchdog, fed by the executor
//!
//! The RTC watchdog (`Rwdt`) is a counter in the always-on domain that resets
//! the chip when it reaches its limit. `esp_hal::init` **disables** it — that is
//! the default in [`esp_hal::Config`] — so a hang today is permanent.
//!
//! What it is fed from matters more than that it is fed. A feed from an
//! interrupt, or from the bottom of some loop, proves only that interrupts
//! still fire. [`feed_task`] is an embassy task, so a feed proves the executor
//! polled it: the scheduler is alive, no task is spinning without yielding, and
//! no `.await` has deadlocked ahead of it in the queue. That is the property
//! worth having, because it is the one that fails.
//!
//! It is armed early — right after the executor exists, before the radio comes
//! up — so that a hang *during* start-up also reboots. A radio that fails to
//! initialise is exactly the field failure nobody will be there to power-cycle.
//! The cost is that a permanent failure becomes a reboot loop; that is the
//! right trade in a car, and [`persist`] is careful not to burn a flash erase
//! on every turn of it.
//!
//! # 2. The reset reason
//!
//! On the next boot the chip still knows why the last one ended. Three lines of
//! code separate "the watchdog fired", "the rail collapsed while the engine
//! cranked" and "somebody unplugged it" — and without them those three are
//! indistinguishable, which is precisely the question `todo/dash/08-power.md`
//! has to answer with a measurement it has not taken yet. [`SocResetReason`]
//! has a `SysBrownOut` variant; if cranking browns the board out, this is what
//! says so, for free, before anybody puts a scope on the rail.
//!
//! One caveat is in esp-hal's own documentation and is repeated here because it
//! bites exactly this use case: the ROM reports `ChipPowerOn` (0x01) for a
//! *chip-level* brown-out too. `SysBrownOut` (0x0F) is the digital-core one. So
//! "power on" in the car does not rule a brown-out out; `SysBrownOut` rules one
//! in.
//!
//! # 3. The last panic, in flash
//!
//! A panic prints to USB and halts. In the car that is a black screen and no
//! evidence at all. So the message goes to flash first, and the next boot says
//! what it was.
//!
//! It lives in the `config` partition, after the two slots
//! [`crate::store::Store`] uses — one more sector out of the 32 KB, of which
//! the settings use 8 KB. The partition is not enlarged and no second partition
//! is invented: the room is already there, and a partition-table change is a
//! re-flash of the table on every board that exists. Where it starts is not a
//! constant here either — [`Store::partition`] reports the offset and how much
//! of it the slots occupy, so this sits *after whatever the store uses*, and
//! stays correct if the store ever grows a third slot.
//!
//! The format follows `store.rs`: magic, version, length, CRC. It does not copy
//! the two-slot scheme, because it does not need it — losing power halfway
//! through writing a panic record loses a panic record, and the CRC makes that
//! detectable. It adds one field the store has no use for: an *acknowledgement*
//! word, cleared from `0xffff_ffff` to zero once the record has been reported.
//! NOR flash lets a bit go 1→0 without an erase (`FlashStorage` implements
//! `MultiwriteNorFlash`, which is that promise), so acknowledging costs one
//! four-byte program and no erase cycle. Without it, "there is a stored panic"
//! could not be told apart from "there was one, fifty boots ago".
//!
//! # The panic handler
//!
//! `esp-backtrace` owns `#[panic_handler]` through its `panic-handler` feature,
//! and there can be exactly one in a binary. There is no hook: its handler
//! takes the `PanicInfo` and never shares it (`custom-pre-backtrace` is called
//! *without* it, so it can capture a stack trace but not a message).
//!
//! So the feature is turned off in `Cargo.toml` and the handler is here — and
//! nothing is lost on the console, because the parts that made it worth having
//! are public API. [`esp_backtrace::Backtrace::capture`] is what its own
//! handler calls; [`print_panic`] below prints the same banner, the same
//! `PanicInfo`, the same frames, in the same colours. `esp-backtrace` is still
//! linked and still owns the *exception* handler, which is the half that
//! catches stack overflows and illegal instructions and which this module does
//! not touch.

use crate::store::Store;
use core::fmt::Write as _;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_time::{Duration as TaskDuration, Timer};
// `ReadNorFlash`/`NorFlash` rather than `ReadStorage`/`Storage`, and the
// difference is not cosmetic. `ReadStorage::read` puts a 4 KB sector buffer on
// the stack unconditionally and `Storage::write` is read-modify-erase-write
// with the same buffer. The `NorFlash` pair skips both when the offset and the
// destination are word-aligned — which everything here is, on purpose — and the
// stack is the one resource a panicking machine may have run out of.
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_hal::rtc_cntl::{Rwdt, RwdtStage, SocResetReason};
use esp_hal::system::{reset_reason, wakeup_cause, SleepSource};
use esp_hal::time::Duration as HalDuration;
use esp_storage::FlashStorage;
use log::warn;

/// How long the dash may be unresponsive before the chip resets itself.
///
/// Long enough that nothing legitimate reaches it: the longest blocking thing
/// on the executor is a settings save, which erases and programs one flash
/// sector — tens of milliseconds. Short enough that a driver sees a blank
/// screen recover rather than stay blank.
const TIMEOUT_SECS: u64 = 10;

/// Feed interval. A fifth of the timeout, so four consecutive missed feeds are
/// tolerated before the reset — the margin exists so that a long flash write or
/// a burst of radio work is not mistaken for a hang.
const FEED_INTERVAL_MS: u64 = 2_000;

/// `VDPN`, little-endian — the same trick `store.rs` plays with `VDSH`: erased
/// flash reads `0xff` everywhere and is otherwise a perfectly plausible record.
const MAGIC: u32 = 0x4E50_4456;
const VERSION: u16 = 1;
const HEADER_LEN: usize = 16;
/// The message is truncated to this. A panic message longer than this is a
/// `write!` with a lot of formatting in it, and the first 240 bytes of one
/// still say which assertion failed and where.
const MSG_MAX: usize = 240;
const RECORD_LEN: usize = HEADER_LEN + MSG_MAX;
/// Byte offset of the acknowledgement word inside the record. Word-aligned,
/// because it is programmed on its own, after the fact, without an erase.
const ACK_OFFSET: u32 = 12;
const ACK_UNREPORTED: u32 = 0xffff_ffff;
const ACK_REPORTED: u32 = 0x0000_0000;

const CRC: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

/// Absolute flash address of the panic sector, or 0 when there is nowhere to
/// write. Zero is a safe sentinel: offset 0 is the bootloader, and no data
/// partition is ever there.
///
/// The address is resolved once, at boot, and only read from the panic handler.
/// Reading the partition table allocates, and a panic handler is the last place
/// to ask an allocator for anything.
static PANIC_BASE: AtomicU32 = AtomicU32::new(0);

/// Set on entry to the panic handler. A panic *inside* the panic handler —
/// which is what a flash driver failing under a corrupt heap would look like —
/// must not recurse into another flash write.
static PANICKING: AtomicBool = AtomicBool::new(false);

/// Everything this module does at start-up, in the order it has to happen.
///
/// Call it once, from `main`, as soon as the executor exists. It reports why
/// the last boot ended, reports and acknowledges a stored panic, notes where
/// the next one goes, and arms the watchdog.
///
/// The watchdog is armed **only if the feed task was actually spawned**. The
/// embassy arena is finite and a full one fails the spawn rather than growing;
/// arming a watchdog nothing feeds would turn a missing feature into a reboot
/// loop, which is a far worse bug than the one being prevented.
pub fn init(spawner: Spawner) {
	report_reset();
	locate_panic_slot();
	report_panic();

	match spawner.spawn(feed_task()) {
		Ok(()) => {
			let mut wdt = Rwdt::new();
			wdt.enable();
			wdt.set_timeout(RwdtStage::Stage0, HalDuration::from_secs(TIMEOUT_SECS));
			esp_println::println!("[health] watchdog armed: {TIMEOUT_SECS} s, fed every {FEED_INTERVAL_MS} ms");
		}
		Err(e) => warn!("SPAWN watchdog FAILED: {e:?} — running unwatched"),
	}
}

/// Feeds the watchdog for as long as the executor is scheduling tasks.
///
/// There is deliberately nothing else in here. Anything this task also did
/// could block it, and a watchdog feeder that can block is a watchdog that
/// fires for the wrong reason.
#[embassy_executor::task]
async fn feed_task() -> ! {
	let mut wdt = Rwdt::new();
	loop {
		wdt.feed();
		Timer::after(TaskDuration::from_millis(FEED_INTERVAL_MS)).await;
	}
}

/// Why the last boot ended, on one line, at every boot.
///
/// `println!` rather than `info!`: `ESP_LOG` is `warn` in `.cargo/config.toml`,
/// so an info line is compiled out of the filter and never seen — and this line
/// is the entire point of the exercise. It is one line per boot.
fn report_reset() {
	let reason = reset_reason();
	let wakeup = wakeup_cause();

	esp_println::println!("[health] reset: {reason:?}, wakeup: {wakeup:?}");

	// The interesting ones, said in words, because `Some(CoreRtcWdt)` is not
	// what somebody reading a console at the roadside needs to see.
	match reason {
		Some(SocResetReason::SysBrownOut) => {
			warn!("last reset was a BROWN-OUT — the rail collapsed. If this follows a crank, that is 08-power's answer.");
		}
		Some(SocResetReason::CoreRtcWdt | SocResetReason::Cpu0RtcWdt | SocResetReason::SysRtcWdt) => {
			warn!("last reset was the WATCHDOG — the executor stopped scheduling for {TIMEOUT_SECS} s");
		}
		Some(SocResetReason::CoreMwdt0 | SocResetReason::CoreMwdt1 | SocResetReason::Cpu0Mwdt0 | SocResetReason::Cpu0Mwdt1) => {
			warn!("last reset was a timer-group watchdog — not ours; something else armed one");
		}
		Some(SocResetReason::SysSuperWdt) => {
			warn!("last reset was the SUPER watchdog — the RTC watchdog itself was not being serviced");
		}
		Some(SocResetReason::ChipPowerOn) => {
			// Worth saying every time: the ROM cannot tell these apart, and a
			// car is full of the second one.
			esp_println::println!("[health] power-on (the ROM reports a chip-level brown-out as this too)");
		}
		_ => {}
	}

	if !matches!(wakeup, SleepSource::Undefined) {
		esp_println::println!("[health] woke from deep sleep via {wakeup:?}");
	}
}

/// Finds the sector the panic record goes in and remembers it for the handler.
///
/// It is the sector immediately after whatever [`Store`] uses, which the store
/// itself reports. Nothing here knows how large a slot is or how many there
/// are, so the store may grow without this silently overwriting it.
fn locate_panic_slot() {
	let store = match Store::open() {
		Ok(store) => store,
		Err(e) => {
			warn!("no panic storage ({e:?}) — a panic will be printed and then lost");
			return;
		}
	};

	let (offset, len, used) = store.partition();

	let base = offset + used;
	let spare = len.saturating_sub(used);
	if spare < FlashStorage::SECTOR_SIZE {
		warn!("config partition has no spare sector after the settings — a panic will be printed and then lost");
		return;
	}
	// The erase granularity is a sector, so an unaligned base would erase a
	// neighbour. Partition offsets are 4 KB-aligned by the ESP-IDF format and
	// `used` is a whole number of sectors, so this should be unreachable —
	// which is why it refuses rather than rounding.
	if base % FlashStorage::SECTOR_SIZE != 0 {
		warn!("panic slot at 0x{base:06x} is not sector-aligned — refusing to use it");
		return;
	}

	PANIC_BASE.store(base, Ordering::Relaxed);
	esp_println::println!("[health] panic slot at 0x{base:06x}, {} bytes spare in the partition", spare);
}

/// Prints the stored panic, if there is one, and marks it seen.
///
/// A record that has already been reported is still printed — it is the last
/// thing that went wrong and that stays true — but it says so, so that "the dash
/// panicked" and "the dash panicked once, last winter" are different sentences.
fn report_panic() {
	let base = PANIC_BASE.load(Ordering::Relaxed);
	if base == 0 {
		return;
	}

	let mut flash = FlashStorage::new();
	let mut record = Record::new();
	if flash.read(base, &mut record.bytes).is_err() {
		warn!("could not read the panic slot");
		return;
	}

	let Some((msg, ack)) = record.parse() else {
		return;
	};

	if ack == ACK_UNREPORTED {
		warn!("PANIC on the previous boot: {msg}");
		// One four-byte program, no erase: `0xffff_ffff` -> 0. If it fails the
		// panic is simply reported again next boot, which is the harmless way
		// round.
		if flash.write(base + ACK_OFFSET, &ACK_REPORTED.to_le_bytes()).is_err() {
			warn!("could not acknowledge the stored panic — it will be reported again");
		}
	} else {
		warn!("last stored panic (already reported, may be old): {msg}");
	}
}

/// The panic handler.
///
/// Console first, flash second: printing cannot fail in a way that loses the
/// flash write, but a flash write on a wrecked machine can hang, and the
/// backtrace is worth more than the record if only one of them survives.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
	print_panic(info);

	// A panic inside the panic handler gets the console and nothing else.
	//
	// Load-then-store rather than a swap: riscv32imc has no atomic
	// read-modify-write instruction, so `AtomicBool` on this target has no
	// `swap`. The window between the two is not a race worth closing — the chip
	// is single-core and this runs with the machine already lost.
	if !PANICKING.load(Ordering::Relaxed) {
		PANICKING.store(true, Ordering::Relaxed);
		persist(info);
	}

	// Halt. If the watchdog is armed this is a reboot in at most `TIMEOUT_SECS`,
	// which is what the car wants: a dash that comes back with an explanation
	// rather than one that stays dark. Unarmed, it is the same dead halt
	// `esp-backtrace` would have left behind.
	loop {
		core::hint::spin_loop();
	}
}

/// What `esp-backtrace`'s own panic handler prints, printed here instead.
fn print_panic(info: &PanicInfo) {
	const RED: &str = "\u{001B}[31m";
	const RESET: &str = "\u{001B}[0m";

	esp_println::println!("{RED}");
	esp_println::println!("");
	esp_println::println!("====================== PANIC ======================");
	esp_println::println!("{}", info);
	esp_println::println!("");
	esp_println::println!("Backtrace:");
	esp_println::println!("");

	let backtrace = esp_backtrace::Backtrace::capture();
	if backtrace.frames().is_empty() {
		esp_println::println!("No backtrace available - make sure to force frame-pointers. (see https://crates.io/crates/esp-backtrace)");
	}
	for frame in backtrace.frames() {
		esp_println::println!("0x{:x}", frame.program_counter());
	}
	esp_println::println!("{RESET}");
}

/// Writes the panic message to the slot found at boot.
///
/// Every failure here is swallowed. This runs on a machine that has already
/// given up; there is no one to tell and nothing better to do.
fn persist(info: &PanicInfo) {
	let base = PANIC_BASE.load(Ordering::Relaxed);
	if base == 0 {
		return;
	}

	let mut record = Record::new();
	let len = record.fill(info);
	let checksum = CRC.checksum(&record.bytes[HEADER_LEN..HEADER_LEN + len]);

	let mut flash = FlashStorage::new();

	// A reboot loop — a panic that reproduces on every boot, which is what
	// arming the watchdog during start-up makes possible — would otherwise
	// erase this sector once per boot and wear it out. The same message need
	// only be written once.
	let mut header = Header { bytes: [0u8; HEADER_LEN] };
	if flash.read(base, &mut header.bytes).is_ok() {
		let h = header.bytes;
		if u32::from_le_bytes([h[0], h[1], h[2], h[3]]) == MAGIC
			&& u16::from_le_bytes([h[4], h[5]]) == VERSION
			&& usize::from(u16::from_le_bytes([h[6], h[7]])) == len
			&& u32::from_le_bytes([h[8], h[9], h[10], h[11]]) == checksum
		{
			return;
		}
	}

	record.bytes[0..4].copy_from_slice(&MAGIC.to_le_bytes());
	record.bytes[4..6].copy_from_slice(&VERSION.to_le_bytes());
	record.bytes[6..8].copy_from_slice(&(len as u16).to_le_bytes());
	record.bytes[8..12].copy_from_slice(&checksum.to_le_bytes());
	record.bytes[12..16].copy_from_slice(&ACK_UNREPORTED.to_le_bytes());

	if flash.erase(base, base + FlashStorage::SECTOR_SIZE).is_err() {
		return;
	}
	let _ = flash.write(base, &record.bytes);
}

/// Just the header, word-aligned, for the "have I already stored this?" check.
#[repr(C, align(4))]
struct Header {
	bytes: [u8; HEADER_LEN],
}

/// The record, word-aligned.
///
/// Alignment is not decoration: `esp-storage` writes an unaligned buffer by
/// copying it through a 4 KB stack buffer first, and this is written from a
/// panic handler.
#[repr(C, align(4))]
struct Record {
	bytes: [u8; RECORD_LEN],
}

impl Record {
	fn new() -> Self {
		Self { bytes: [0xff; RECORD_LEN] }
	}

	/// Formats the panic into the message area and returns its length.
	///
	/// `PanicInfo`'s `Display` is already "panicked at file:line:col:\nmessage",
	/// so nothing has to be assembled by hand.
	fn fill(&mut self, info: &PanicInfo) -> usize {
		let mut sink = Truncating {
			buf: &mut self.bytes[HEADER_LEN..],
			len: 0,
		};
		let _ = write!(sink, "{info}");
		let len = sink.len;
		// Erased flash is 0xff and the sector is erased before the write, but
		// the padding is written too, so it has to be something. 0xff keeps a
		// hex dump of a fresh record and of an erased sector looking alike.
		self.bytes[HEADER_LEN + len..].fill(0xff);
		len
	}

	/// The message and the acknowledgement word, or `None` if the slot holds
	/// nothing this firmware wrote.
	fn parse(&self) -> Option<(&str, u32)> {
		if u32::from_le_bytes(self.bytes[0..4].try_into().ok()?) != MAGIC {
			return None;
		}
		if u16::from_le_bytes(self.bytes[4..6].try_into().ok()?) != VERSION {
			return None;
		}
		let len = usize::from(u16::from_le_bytes(self.bytes[6..8].try_into().ok()?));
		if len == 0 || len > MSG_MAX {
			return None;
		}
		let expected = u32::from_le_bytes(self.bytes[8..12].try_into().ok()?);
		let ack = u32::from_le_bytes(self.bytes[12..16].try_into().ok()?);
		let msg = &self.bytes[HEADER_LEN..HEADER_LEN + len];
		if CRC.checksum(msg) != expected {
			warn!("the stored panic record is corrupt");
			return None;
		}
		// A message truncated mid-character is not a reason to lose the rest.
		let text = match core::str::from_utf8(msg) {
			Ok(text) => text,
			Err(e) => core::str::from_utf8(&msg[..e.valid_up_to()]).ok()?,
		};
		Some((text, ack))
	}
}

/// A `fmt::Write` that fills a buffer and then quietly stops.
///
/// `core::fmt` has no "how much would that have been" and no way to ask it to
/// stop, so a formatter that returns `Err` would abort the whole `write!` and
/// lose what had already landed. This keeps the prefix, which is the part with
/// the file and line in it.
struct Truncating<'a> {
	buf: &'a mut [u8],
	len: usize,
}

impl core::fmt::Write for Truncating<'_> {
	fn write_str(&mut self, s: &str) -> core::fmt::Result {
		let room = self.buf.len() - self.len;
		let take = s.len().min(room);
		self.buf[self.len..self.len + take].copy_from_slice(&s.as_bytes()[..take]);
		self.len += take;
		Ok(())
	}
}
