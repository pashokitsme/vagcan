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

#[esp_hal::main]
fn main() -> ! {
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let delay = Delay::new();
    let mut rtc = Rtc::new(peripherals.LPWR);

    let woke = sleep::wake_reason();
    println!();
    println!("sleeptest: awake, wake reason = {}", woke.as_str());
    println!("sleeptest: wake button belongs on GPIO{WAKE_BUTTON_GPIO}, rail divider on GPIO{RAIL_SENSE_GPIO}");
    println!("sleeptest: neither is wired, so timer wake is the only source registered");
    println!("sleeptest: staying up {AWAKE_SECS} s, then sleeping {SLEEP_SECS} s");

    delay.delay_millis(AWAKE_SECS * 1_000);

    // Nothing to shut down: this binary drives no panel, no transceiver and no
    // radio. On the real firmware the checklist on `sleep::deep_sleep` is the
    // part that decides the number a meter shows.
    println!("sleeptest: sleeping now");
    sleep::deep_sleep(&mut rtc, Duration::from_secs(SLEEP_SECS))
}
