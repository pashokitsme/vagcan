//! Is there anything on the pair? A listen-only frame counter, safe on a car.
//!
//! Bring-up probe. `dash` filters the receive queue down to the plan's answer
//! ids, so its silence says nothing about whether the bus is alive; `rxprobe`
//! samples the receive pad as a GPIO, which sees a burst but not what it was;
//! and both of the laptop's ears were in doubt at once. This puts the TWAI
//! controller in **listen-only** mode with no filter and, once a second, says
//! how many frames arrived and which identifiers they carried. Listen-only
//! transmits nothing — no acknowledgement, no error flag — so unlike `rxprobe`
//! and `cantest` this one may be pointed at a car.
//!
//! What to read: on a diagnostic CAN nothing moves until something asks, so a
//! quiet minute with the ignition on is normal; a quiet minute with the engine
//! running, on a car that filled `dash`'s queue at boot on its first run, is
//! a pair that does not reach the car.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use embedded_can::{Frame as _, Id};
use esp_backtrace as _;
// The `#[panic_handler]` lives in the library (`health.rs`), so the library has
// to be linked even though this probe calls nothing from it.
use esp_hal::clock::CpuClock;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::twai::{BaudRate, TwaiConfiguration, TwaiMode};
use log::info;
use vag_dash_fw as _;

// The library's panic handler prints through the allocator-backed logger, so the
// heap has to exist even though this probe allocates nothing itself.
extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

/// How long the watch runs before settling into a five-second heartbeat.
const WATCH: Duration = Duration::from_secs(60);
/// Distinct identifiers remembered per report; the diagnostic CAN carries few.
const IDS: usize = 16;

#[esp_hal_embassy::main]
async fn main(_spawner: Spawner) {
	esp_println::logger::init_logger(log::LevelFilter::Info);

	let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
	esp_alloc::heap_allocator!(size: 32 * 1024);
	esp_hal_embassy::init(SystemTimer::new(peripherals.SYSTIMER).alarm0);

	// Same pins as `dash`: GPIO1 reads the transceiver's `R`, GPIO6 drives its
	// `D` — which listen-only never pulls low.
	let twai = TwaiConfiguration::new(
		peripherals.TWAI0,
		peripherals.GPIO1,
		peripherals.GPIO6,
		BaudRate::B500K,
		TwaiMode::ListenOnly,
	);
	let mut twai = twai.into_async().start();
	info!("rxwatch: listen-only at 500 kbit/s, no filter, R on GPIO1 — transmits nothing");

	let end = Instant::now() + WATCH;
	let mut second = 0u32;
	let mut total = 0u32;
	loop {
		let tick = Instant::now() + Duration::from_secs(1);
		let (mut frames, mut errors) = (0u32, 0u32);
		let mut ids: heapless::Vec<u32, IDS> = heapless::Vec::new();
		while Instant::now() < tick {
			match with_timeout(tick - Instant::now(), twai.receive_async()).await {
				Ok(Ok(frame)) => {
					frames += 1;
					let id = match frame.id() {
						Id::Standard(s) => u32::from(s.as_raw()),
						Id::Extended(e) => e.as_raw() | 0x8000_0000,
					};
					if !ids.contains(&id) {
						let _ = ids.push(id);
					}
				}
				Ok(Err(_)) => errors += 1,
				Err(_) => break,
			}
		}
		second += 1;
		total += frames;
		if frames > 0 || errors > 0 || second % 10 == 0 {
			info!("[{second:3}s] {frames} frame(s), {errors} error(s), ids {:03X?}", ids.as_slice());
		}
		if Instant::now() >= end {
			break;
		}
	}
	info!("== {total} frame(s) in {} s; bus-off: {} ==", WATCH.as_secs(), twai.is_bus_off());
	loop {
		Timer::after(Duration::from_secs(5)).await;
	}
}
