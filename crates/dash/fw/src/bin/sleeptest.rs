//! Sleep, wake, say why, repeat — the smallest program that demonstrates the
//! deep-sleep path in `sleep.rs`, and the one to leave running with a meter in
//! series.
//!
//! Flash it and watch the serial port. Each cycle prints the wake reason, sits
//! awake for [`AWAKE_SECS`], and goes down for [`SLEEP_SECS`]. The first line
//! after a flash says `cold boot`; every line after that should say
//! `RTC timer`, and that transition is the whole proof that the chip really
//! slept rather than crashed and restarted.
//!
//! **Only timer wake is exercised, and that is not a shortcut.** The board's
//! BOOT button is `GPIO9`, which is not one of the C3's RTC pins (`GPIO0`–
//! `GPIO5`) and so cannot wake the chip at all; it is also the strapping pin
//! that puts the chip in the USB download bootloader when held low at reset.
//! A wake button needs a wire to `GPIO5` first — see the pin allocation in
//! [`vag_dash_fw::sleep`].
//!
//! # Putting a meter on it
//!
//! `println!` is used rather than `log`, because `.cargo/config.toml` sets
//! `ESP_LOG=warn` and the point of this binary is that its output is never
//! filtered.
//!
//! What the numbers are is deliberately not written down anywhere here. Nothing
//! in this repository has measured the board's current, and the sleep design in
//! `08-power.md` is settled by a measurement, not by a firmware author's
//! expectation. Two things make the reading honest:
//!
//! - **Measure the whole device, not the chip.** The target in `08-power.md` is
//!   under about 1 mA for everything on the OBD plug, and the regulator's own
//!   quiescent current is part of that.
//! - **Measure over a whole cycle.** [`SLEEP_SECS`] is short so that a person
//!   watching does not get bored; it also means the awake seconds dominate the
//!   average. Lengthen it, or gate the meter on the sleeping stretch.

#![no_std]
#![no_main]
#![deny(
	clippy::mem_forget,
	reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use core::time::Duration;
// Linked for its `#[global_allocator]`, not for a heap. `vag_dash_fw`'s crate
// root declares `extern crate alloc`, so every binary against it must supply an
// allocator even when it never allocates — and this one never does. No region
// is registered, so an allocation would fail loudly rather than quietly costing
// SRAM in a program whose whole subject is what it costs.
use esp_alloc as _;
use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rtc_cntl::Rtc;
use esp_println::println;
use vag_dash_fw::sleep::{self, RAIL_SENSE_GPIO, WAKE_BUTTON_GPIO};

esp_bootloader_esp_idf::esp_app_desc!();

/// Long enough to read the banner and to see the meter settle at the awake
/// current, short enough that nobody waits for it.
const AWAKE_SECS: u32 = 5;

/// Long enough for the sleeping current to be a flat line on a meter, short
/// enough to watch several cycles in a minute.
const SLEEP_SECS: u64 = 10;

/// Say why we woke in flashes, before saying it in words.
///
/// The words are unreliable at exactly this moment and the light is not: deep
/// sleep powers the USB peripheral down, so for a second or two after a wake
/// there is no console to print to. The LED is lit by the time the host has
/// noticed the device is back.
///
/// One long flash is a cold boot, one short is the timer, three short is the
/// button — the last deliberately the most distinctive, because it is the one
/// a person causes on purpose and wants confirmed.
fn signal(led: &mut Output<'_>, delay: &Delay, woke: sleep::Wake) {
	let (count, on_ms, off_ms) = match woke {
		sleep::Wake::ColdBoot => (1, 600, 200),
		sleep::Wake::Timer => (1, 80, 200),
		sleep::Wake::Pin => (3, 80, 120),
		sleep::Wake::Unexpected => (8, 40, 40),
	};
	for _ in 0..count {
		led.set_low();
		delay.delay_millis(on_ms);
		led.set_high();
		delay.delay_millis(off_ms);
	}
}

#[esp_hal::main]
fn main() -> ! {
	let peripherals = esp_hal::init(esp_hal::Config::default());
	let delay = Delay::new();
	let mut rtc = Rtc::new(peripherals.LPWR);

	let woke = sleep::wake_reason();

	// The board's own LED sinks through GPIO8, so `High` is dark. Flashed
	// before anything is printed, because at this moment the light works and
	// the console does not — and because a person watching the board should
	// not need a terminal to see that it woke, or why.
	let mut led = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());
	signal(&mut led, &delay, woke);

	println!();
	println!("sleeptest: wake button on GPIO{WAKE_BUTTON_GPIO}, rail divider on GPIO{RAIL_SENSE_GPIO}");
	println!("sleeptest: short GPIO{WAKE_BUTTON_GPIO} to ground to wake early — it is pulled up inside the chip");

	// The wake reason, once a second for the whole awake window, rather than
	// once at boot. Deep sleep powers the USB Serial/JTAG peripheral down, so
	// the host un-enumerates the device and takes a second or two to bring it
	// back — and anything printed before that lands in a port nobody is
	// holding open. Measured, not guessed: a single line at boot is invisible,
	// and so is one delayed by 1.2 s. Repeating it costs nothing and does not
	// depend on how fast a particular host enumerates.
	//
	// A bench that watches sleep closely wants a USB-serial adapter on UART0
	// instead, which stays enumerated because it is a separate chip.
	for second in 0..AWAKE_SECS {
		println!("sleeptest: awake {}/{AWAKE_SECS} s, wake reason = {}", second + 1, woke.as_str());
		delay.delay_millis(1_000);
	}

	// Nothing to shut down: this binary drives no panel, no transceiver and no
	// radio. On the real firmware the checklist on `sleep::deep_sleep` is the
	// part that decides the number a meter shows.
	println!("sleeptest: sleeping now");
	let mut button = peripherals.GPIO5;
	sleep::deep_sleep_with_button(&mut rtc, Duration::from_secs(SLEEP_SECS), &mut button)
}
