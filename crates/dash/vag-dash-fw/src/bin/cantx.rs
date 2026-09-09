//! Hammer the bus: transmit one `7E0` request as fast as the controller will
//! take it, and report how many the peripheral accepted and how many it could
//! not. A bench fault-finder, not a product path.
//!
//! `dash` transmits a request and then waits for a reply, so on a broken
//! transmit path it is slow and quiet. This does nothing but transmit, so a
//! sniffer on the pair sees a continuous stream the moment the driver works,
//! and `transmit_error_count` climbing to 128 and staying there is the
//! error-passive signature of a node nobody acknowledges.
//!
//! **Normal mode: it transmits and expects acknowledgement, so it must never be
//! pointed at a car.** It is the bench half of the CANable test — flash this,
//! run `vagcan dev sniff` on the CANable, and the board's `7E0` appearing in the
//! capture is the transmit path proven end to end.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_time::{Duration, Instant, with_timeout};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::twai::{BaudRate, EspTwaiFrame, Id, StandardId, TwaiConfiguration, TwaiMode};
use log::info;
use vag_dash_fw as _;

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

/// Engine, the id `dash` reads first. The payload is a real `0x22 F1 87` read,
/// ISO-TP single frame, so the frame on the wire is byte-for-byte one `dash`
/// sends — nothing here widens what the allowlist permits.
const REQUEST: [u8; 8] = [0x02, 0x22, 0xF1, 0x87, 0x00, 0x00, 0x00, 0x00];

#[esp_hal_embassy::main]
async fn main(_spawner: Spawner) {
	esp_println::logger::init_logger(log::LevelFilter::Info);

	let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
	esp_alloc::heap_allocator!(size: 32 * 1024);
	esp_hal_embassy::init(SystemTimer::new(peripherals.SYSTIMER).alarm0);

	// Same pins and speed as `dash`: GPIO1 = RXD, GPIO6 = TXD, 500 kbit/s.
	let twai = TwaiConfiguration::new(peripherals.TWAI0, peripherals.GPIO1, peripherals.GPIO6, BaudRate::B500K, TwaiMode::Normal);
	let mut twai = twai.into_async().start();
	info!("cantx: Normal at 500 kbit/s, hammering 7E0 [02 22 F1 87] — BENCH ONLY, never a car");

	let id = Id::Standard(StandardId::new(0x7E0).unwrap());
	let frame = EspTwaiFrame::new(id, &REQUEST).unwrap();

	let mut second = Instant::now() + Duration::from_secs(1);
	let (mut ok, mut fail) = (0u32, 0u32);
	loop {
		// A stuck transmit would block forever; cap the wait so the counters
		// still print and the failure is visible rather than silent.
		match with_timeout(Duration::from_millis(200), twai.transmit_async(&frame)).await {
			Ok(Ok(())) => ok += 1,
			_ => fail += 1,
		}
		if Instant::now() >= second {
			info!("cantx: {ok} accepted, {fail} timed out this second; TEC {}", twai.transmit_error_count());
			ok = 0;
			fail = 0;
			second = Instant::now() + Duration::from_secs(1);
		}
	}
}
