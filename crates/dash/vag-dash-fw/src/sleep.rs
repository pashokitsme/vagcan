//! Deep sleep: what can wake the chip, when it decides to go down, and what
//! has to be off before it does.
//!
//! The device hangs off **OBD-II pin 16, permanent battery positive** (SAE
//! J1962). Nothing in the car switches that off, so whatever the board draws
//! it draws for every hour the car stands. `08-power.md` sets the target at
//! under about 1 mA for the whole device asleep.
//!
//! **No current has been measured.** There is no meter on this yet and nothing
//! in this file is evidence about one — see "turning this into a number" at the
//! bottom.
//!
//! # What an ESP32-C3 can actually wake on
//!
//! `07-sleep.md` was written for an ESP32-WROOM-32 and its central mechanism —
//! the ULP coprocessor sampling ADC1 through deep sleep and waking the cores on
//! a threshold — **does not transfer to this board**. The C3 has no ULP. The
//! device description esp-hal builds from (`esp-metadata`, `devices/esp32c3.toml`)
//! lists neither `ulp_supported` nor `pm_support_ext0_wakeup` nor
//! `pm_support_ext1_wakeup` nor touch; the only `pm_support_*` entries are
//! `wifi_wakeup` and `bt_wakeup`, and both are light-sleep only.
//!
//! That leaves exactly two ways out of deep sleep on this part:
//!
//! 1. **the RTC timer** — `TimerWakeupSource`, and
//! 2. **a level on an RTC-capable pin** — `RtcioWakeupSource`.
//!
//! So "wake when the rail reaches 13 V" cannot be a comparison running inside
//! the chip while it sleeps. It is either a periodic timer wake that samples
//! the ADC and goes straight back down, or an **external** comparator holding
//! an RTC pin. Which of those meets the budget is a bench question, not a
//! reasoning one, and it is open.
//!
//! # The six pins, and who gets them
//!
//! The RTC pins on the C3 are **`GPIO0`–`GPIO5` and only those** — `rtc_pins!`
//! in `esp-hal/src/soc/esp32c3/gpio.rs` implements `RtcPin` for those six and
//! stops. `GPIO0`–`GPIO4` are also the whole of **ADC1** (ADC2 is one pin,
//! `GPIO5`, and ADC2 is unusable while the radio runs). Two scarce things over
//! one set of six, so they are allocated here rather than argued about later:
//!
//! | pin | RTC | ADC | assignment | why this one |
//! |---|---|---|---|---|
//! | `GPIO0` | yes | ADC1_CH0 | free | kept for a second analog input |
//! | `GPIO1` | yes | ADC1_CH1 | **TWAI RX** — from the transceiver's `RXD` | an RTC pin, so a dominant bit on the bus can wake the chip later |
//! | `GPIO2` | yes | ADC1_CH2 | **avoid** | strapping pin, sampled at reset |
//! | `GPIO3` | yes | ADC1_CH3 | free | kept for a second wake pin |
//! | `GPIO4` | yes | ADC1_CH4 | **rail sense** — divider from OBD pin 16 | has to be ADC1, and is not a strapping pin |
//! | `GPIO5` | yes | ADC2_CH0 | **wake button** | see below |
//!
//! The button goes on `GPIO5` because `GPIO5` is the one RTC pin that is *not*
//! ADC1. Its analog capability is already worthless — ADC2 does not read while
//! Wi-Fi/BLE is up — so spending it on a digital job costs nothing, and all
//! five ADC1 pins stay analog. The divider then takes `GPIO4`: it must be ADC1
//! (`07-sleep.md`), and of the non-strapping ADC1 pins it is the one furthest
//! from the low-numbered pads a bootloader or a crystal option tends to claim.
//!
//! Note that the divider does **not** consume a wake slot on this chip. With no
//! ULP there is nothing to read it during sleep, so `GPIO4` is an ordinary ADC
//! input that happens to sit on an RTC-capable pad.
//!
//! Two pins are already spoken for by `dash.rs` and neither can help here:
//!
//! - **`GPIO8`** — the SuperMini's blue LED (Low is lit). Strapping pin; the
//!   LED's pull-up holds it high at reset, which is why driving it afterwards
//!   is safe.
//! - **`GPIO9`** — the **BOOT button**. It is a real, already-fitted button and
//!   it is useless as a wake source twice over: it is **not an RTC pin**, so it
//!   physically cannot wake the chip from deep sleep, and it is a strapping pin
//!   held low at reset to enter the USB download bootloader — a wake that
//!   worked would boot the ROM loader instead of the firmware.
//!
//! **So a wake button needs a wire to one of `GPIO0`–`GPIO5`, and nobody has
//! soldered one.** Until somebody does, [`deep_sleep`] offers timer wake and
//! nothing else. There is deliberately no pin-wake path in this file: an
//! untested one would look like a feature and behave like a guess.
//!
//! # Nothing survives the sleep
//!
//! `RtcSleepConfig::deep()` on the C3 sets `rtc_fastmem_pd_en` and
//! `rtc_slowmem_pd_en`, so **RTC memory is powered down** and
//! `#[esp_hal::ram(persistent)]` does not persist across `sleep_deep`. Every
//! wake is a cold start of the program's state. Anything that has to outlive a
//! sleep belongs in flash, which is what `store.rs` is for.
//!
//! # Turning this into a number
//!
//! One caveat for whoever puts the meter on it, because it is a property of
//! this HAL version rather than of the chip: esp-hal 1.0.0-rc.0 runs its
//! equivalent of IDF's `esp_sleep_isolate_digital_gpio` **only from
//! `RtcioWakeupSource::apply`**. With timer-only wake the digital pads are left
//! as they were, and esp-hal's own comment on that step says the deep-sleep
//! bottom current rises without it. A timer-only figure is therefore an upper
//! bound on what the chip can do, not the floor.

use core::time::Duration;
use esp_hal::gpio::RtcPinWithResistors;
use esp_hal::rtc_cntl::sleep::{RtcioWakeupSource, TimerWakeupSource, WakeupLevel};
use esp_hal::rtc_cntl::Rtc;
use esp_hal::system::SleepSource;

/// The pin the wake button must be wired to. Nothing on the board reaches it
/// yet; see the module docs.
pub const WAKE_BUTTON_GPIO: u8 = 5;

/// The pin the divider from OBD pin 16 must be wired to. ADC1, so it is
/// readable with the radio up.
pub const RAIL_SENSE_GPIO: u8 = 4;

/// Why the chip is running. Only three of these can happen on a C3 with the
/// wake sources this firmware registers; the fourth exists so an unexpected
/// one is reported rather than swallowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
	/// Not a wake at all — power-on, a reset button, a flash. The last sleep,
	/// if there was one, is not what started this run.
	ColdBoot,
	/// The RTC timer expired.
	Timer,
	/// A level on an RTC pin. Unreachable today: no pin wake source is
	/// registered, because no pin is wired.
	Pin,
	/// Something the C3 is not documented to do from deep sleep. Worth a log
	/// line if it is ever seen.
	Unexpected,
}

impl Wake {
	/// A word for the log. `Debug` would do, but this is the one thing a
	/// person watching a serial port reads, so it is spelled out.
	pub const fn as_str(self) -> &'static str {
		match self {
			Wake::ColdBoot => "cold boot",
			Wake::Timer => "RTC timer",
			Wake::Pin => "pin level",
			Wake::Unexpected => "unexpected",
		}
	}
}

/// What started this run.
///
/// `esp_hal::system::wakeup_cause()` returns `Undefined` whenever the reset
/// reason was not `CoreDeepSleep`, which is exactly "we did not get here by
/// waking up".
pub fn wake_reason() -> Wake {
	match esp_hal::system::wakeup_cause() {
		SleepSource::Undefined => Wake::ColdBoot,
		SleepSource::Timer => Wake::Timer,
		// On the C3 an RTC-IO level wake is reported as `Gpio`; `Ext0`/`Ext1`
		// do not exist on this part.
		SleepSource::Gpio => Wake::Pin,
		_ => Wake::Unexpected,
	}
}

/// The idle backstop: fifteen minutes with nothing happening and the device
/// goes down regardless of what it did or did not see on the bus.
///
/// This is the condition that fires **today**, because the cyclic frame that
/// carries the ignition has not been identified yet — that is `06` §7 — so
/// [`Awake::saw_ignition`] has no caller on the car and
/// [`SleepReason::IgnitionOff`] cannot be reached. A device that only ever
/// sleeps on the backstop is still a device that sleeps.
pub const IDLE_TIMEOUT_MS: u64 = 15 * 60 * 1000;

/// How long the ignition frame has to be **absent** before its absence is
/// believed.
///
/// A placeholder, and it should be re-derived as a small number of the frame's
/// own periods once `06` §7 says what that period is. The requirement it exists
/// to meet is not the value: sleep must follow *sustained* silence, because one
/// missed frame on the road is ordinary and going to sleep mid-drive is the
/// failure that matters.
pub const IGNITION_LOST_MS: u64 = 5_000;

/// Why the device decided to go down. Both reasons end in the same deep sleep;
/// they differ only in what a log line should say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepReason {
	/// The bus stopped saying the ignition was on, and stayed stopped long
	/// enough to be believed.
	IgnitionOff,
	/// Nothing happened for [`IDLE_TIMEOUT_MS`].
	Idle,
}

/// The decision to sleep, as a state machine over a clock.
///
/// It touches no hardware, so it can be driven from a synthetic clock exactly
/// like the button machine in `ui.rs`.
///
/// **There is deliberately no way to tell it that a request went unanswered.**
/// `07-sleep.md` is explicit that "no answer" must play no part: it is the same
/// input the moving-car guard reads as *moving*, and one signal with two
/// opposite meanings is how a device ends up asleep on the motorway. Sleep here
/// is a positive statement from the bus, or a timer, and the absence of the
/// method is the enforcement.
pub struct Awake {
	/// When anything last happened that should keep the panel up.
	last_activity_ms: u64,
	/// When the ignition frame was last seen, if it ever was.
	last_ignition_ms: Option<u64>,
}

impl Awake {
	/// Start the clock. `now_ms` is whatever monotonic millisecond count the
	/// caller uses; only differences matter.
	pub const fn new(now_ms: u64) -> Self {
		Self {
			last_activity_ms: now_ms,
			last_ignition_ms: None,
		}
	}

	/// The cyclic frame that says the ignition is on was received (`06` §7).
	///
	/// It counts as activity too, so the backstop cannot fire under a running
	/// ignition.
	pub fn saw_ignition(&mut self, now_ms: u64) {
		self.last_ignition_ms = Some(now_ms);
		self.last_activity_ms = now_ms;
	}

	/// Somebody pressed the button, or a page changed — anything that means a
	/// person is present. Defers the backstop and nothing else.
	///
	/// A BLE client merely being connected is **not** activity. A phone left
	/// paired in a parked car would otherwise hold the device up all night,
	/// which is the exact bill this module exists to avoid.
	pub fn saw_activity(&mut self, now_ms: u64) {
		self.last_activity_ms = now_ms;
	}

	/// Ask whether it is time to go down. `None` means stay up.
	pub fn poll(&self, now_ms: u64) -> Option<SleepReason> {
		if let Some(seen) = self.last_ignition_ms {
			if now_ms.saturating_sub(seen) >= IGNITION_LOST_MS {
				return Some(SleepReason::IgnitionOff);
			}
		}
		if now_ms.saturating_sub(self.last_activity_ms) >= IDLE_TIMEOUT_MS {
			return Some(SleepReason::Idle);
		}
		None
	}
}

/// Enter deep sleep, waking after `wake_after`. Does not return: the chip comes
/// back through `main`.
///
/// **Everything below has to be true before this is called.** Deep sleep powers
/// down the CPU and the digital peripherals, so the ESP32's own consumption
/// looks after itself; what it does *not* do is switch off anything outside the
/// chip, and that is where the milliamps are.
///
/// 1. **The panel is off, not dark.** An SSD1322 at zero contrast is still
///    running its oscillator and its charge pump. It wants its display-off
///    command, and better than that it wants its supply cut.
/// 2. **The isolated CAN side is off.** The B0505S-1WR3 that feeds the
///    ADM3050E is a 1 W isolated module, and modules of that class idle in the
///    tens of milliamps (`08-power.md`) — on its own, ten times the whole
///    budget. It is cut with the load switch, and the load switch has no pin
///    assigned yet because the board does not exist.
/// 3. **The radio is stopped.** BLE is torn down, or at least not advertising,
///    before the call rather than being cut off mid-packet.
/// 4. **Every pin driving something outside the chip is at its inactive
///    level.** The C3 can hold a pad through deep sleep only for `GPIO0`–`GPIO5`
///    (`RtcPin::rtcio_pad_hold`); the other seventeen simply stop being driven,
///    so an external part must not depend on a level to stay quiet.
///
/// None of this is enforced by the type system, and pretending otherwise would
/// mean inventing hardware that has not been built. It is a checklist for the
/// caller.
pub fn deep_sleep(rtc: &mut Rtc<'_>, wake_after: Duration) -> ! {
	let timer = TimerWakeupSource::new(wake_after);
	rtc.sleep_deep(&[&timer])
}

/// Sleep until the timer expires **or** the button is pressed.
///
/// The button is an RTC pin held high by its internal pull-up, and the wake is
/// on `Low` — so a press is a short to ground and needs no external resistor.
/// That also means it can be tested before anything is soldered: touch the pin
/// to a ground pad and the chip comes back.
///
/// Only `GPIO0`–`GPIO5` can do this. The board's own BOOT button is on
/// `GPIO9`, which is not an RTC pin and therefore cannot wake the chip at all;
/// it is also the strapping pin that boots the USB loader when held low at
/// reset, so it is the wrong pin twice over.
///
/// There is a second reason to prefer this over the timer alone, and it is
/// about current rather than convenience: esp-hal isolates the digital pads
/// only from `RtcioWakeupSource::apply`. With a timer-only sleep they are left
/// as they are and the floor is higher — so a figure measured without a pin
/// wake source is an upper bound, not the number this design will land on.
pub fn deep_sleep_with_button(rtc: &mut Rtc<'_>, wake_after: Duration, button: &mut dyn RtcPinWithResistors) -> ! {
	// Held high while nothing is pressing it, so the pin does not float and
	// wake the device on stray charge the moment the drivers stop.
	button.rtcio_pullup(true);
	button.rtcio_pulldown(false);

	let timer = TimerWakeupSource::new(wake_after);
	let mut pins: [(&mut dyn RtcPinWithResistors, WakeupLevel); 1] = [(button, WakeupLevel::Low)];
	let pin_wake = RtcioWakeupSource::new(&mut pins);
	rtc.sleep_deep(&[&timer, &pin_wake])
}
