//! Is the transceiver there at all? A GPIO-level answer, with no CAN in it.
//!
//! Throwaway bring-up probe. The board would not transmit and a multimeter gave
//! numbers that could not all be true at once, so this drives the two pads by
//! hand and reports what happens:
//!
//! 1. **idle** — samples the receive pad for a second with the transmit pad left
//!    recessive, and says how much of that second was high and how many times it
//!    changed. Stuck low, stuck high and toggling are three different faults and
//!    an averaging voltmeter cannot tell them apart.
//! 2. **echo** — drives the transmit pad (the transceiver's `D`) low, which makes
//!    a healthy transceiver put a dominant bit on the pair, and reads the receive
//!    pad (its `R`) back. `D` low must come back as `R` low. This walks the whole
//!    path — pad, wire, chip, pair, chip, wire, pad — without a single CAN bit.
//!
//! Nothing here is a CAN controller, so nothing needs a bit rate, an
//! acknowledgement or a second node. It holds a DC level on the pair, so it is a
//! bench tool: never point it at a car.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
// The `#[panic_handler]` lives in the library (`health.rs`), so the library has
// to be linked even though this probe calls nothing from it.
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::gpio::{AnyPin, Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::timer::systimer::SystemTimer;
use log::info;
use vag_dash_fw as _;

// The library's panic handler prints through the allocator-backed logger, so the
// heap has to exist even though this probe allocates nothing itself.
extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

/// The transceiver's `D` (`CTX`), driven here by hand. Recessive is high.
const TX_PIN: u8 = 6;
/// The transceiver's `R` (`CRX`), read here by hand.
const RX_PIN: u8 = 3;

/// How long the idle sample runs.
const WATCH: Duration = Duration::from_secs(1);
/// Microseconds between two idle samples — fast enough to catch a 500 kbit/s
/// burst as *some* change, slow enough that a second of it fits in a counter.
const STEP_US: u32 = 20;

#[esp_hal_embassy::main]
async fn main(_spawner: Spawner) {
	esp_println::logger::init_logger(log::LevelFilter::Info);

	let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
	esp_alloc::heap_allocator!(size: 32 * 1024);
	esp_hal_embassy::init(SystemTimer::new(peripherals.SYSTIMER).alarm0);

	// SAFETY: this binary configures no other GPIO, so nothing else in it holds
	// these two pads.
	let tx = unsafe { AnyPin::steal(TX_PIN) };
	let rx = unsafe { AnyPin::steal(RX_PIN) };

	// Recessive: a CAN transceiver's `D` idles high, and so does its `R`.
	let mut tx = Output::new(tx, Level::High, OutputConfig::default());
	// No pull. The transceiver's `R` is a push-pull output and is supposed to be
	// the only thing setting this level; a pull here would hide exactly the fault
	// being looked for.
	let rx = Input::new(rx, InputConfig::default().with_pull(Pull::None));
	let delay = Delay::new();

	info!("rxprobe: D on GPIO{TX_PIN} (driven), R on GPIO{RX_PIN} (read), no pull, no CAN");

	// --- 1. idle -----------------------------------------------------------
	let (mut high, mut total, mut changes) = (0u32, 0u32, 0u32);
	let mut last = rx.is_high();
	let deadline = Instant::now() + WATCH;
	while Instant::now() < deadline {
		let now = rx.is_high();
		if now != last {
			changes += 1;
			last = now;
		}
		if now {
			high += 1;
		}
		total += 1;
		delay.delay_micros(STEP_US);
	}
	// `total` cannot be zero — the loop runs at least once — but the compiler
	// cannot see that and a probe is the last place to divide by hope.
	let percent = (high * 100).checked_div(total).unwrap_or(0);
	info!("[1/2 idle] R was high {percent}% of {total} samples, {changes} change(s) in one second");
	match (percent, changes) {
		(_, c) if c > 0 => info!("[1/2 idle] the line moves — something is driving the pair"),
		(0, _) => info!("[1/2 idle] STUCK LOW — the controller sees a permanently busy bus and will never transmit"),
		(100, _) => info!("[1/2 idle] stuck high — a clean recessive idle, which is what it should be"),
		_ => info!("[1/2 idle] neither high nor low: the pad is floating or the level sits on the threshold"),
	}

	// --- 2. echo -----------------------------------------------------------
	// `D` low must come back as `R` low, through the transceiver and the pair.
	info!("[2/2 echo] driving D and reading R back");
	let mut ok = true;
	for round in 0..3 {
		for (level, name) in [(Level::Low, "dominant"), (Level::High, "recessive")] {
			tx.set_level(level);
			// A transceiver switches in nanoseconds; this is slack for the wire
			// and for a scope, not for the silicon.
			delay.delay_micros(200);
			let mut seen_high = 0;
			for _ in 0..64 {
				if rx.is_high() {
					seen_high += 1;
				}
				delay.delay_micros(2);
			}
			let want_high = level == Level::High;
			let got_high = seen_high > 32;
			if got_high != want_high {
				ok = false;
			}
			info!(
				"[2/2 echo] round {round}: D {name} -> R {} ({seen_high}/64 high){}",
				if got_high { "high" } else { "low" },
				if got_high == want_high { "" } else { "  <-- WRONG" }
			);
		}
	}
	tx.set_high();

	if ok {
		info!("== echo passes: pads, wires, transceiver and the pair all carry a level ==");
	} else {
		info!("== echo FAILS: R does not follow D — the break is in CTX, CRX, the module, or its supply ==");
	}

	loop {
		Timer::after(Duration::from_secs(5)).await;
	}
}
