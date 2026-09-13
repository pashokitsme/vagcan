#![no_std]
#![no_main]

//! Radio self-test: does this board hear anything at all?
//!
//! Station mode, one scan, print every AP with its channel and RSSI. If this
//! lists the neighbourhood, the antenna and the receiver work and any AP-mode
//! failure is a software matter. If it lists nothing, the hardware is the
//! suspect and everything above it is moot.

extern crate alloc;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_wifi::wifi::{ClientConfiguration, Configuration};
use log::info;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

static WIFI_INIT: StaticCell<esp_wifi::EspWifiController<'static>> = StaticCell::new();

#[esp_hal_embassy::main]
async fn main(_spawner: Spawner) {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);

    let rng = Rng::new(peripherals.RNG);
    let timer1 = TimerGroup::new(peripherals.TIMG0);
    let wifi_init = WIFI_INIT.init(esp_wifi::init(timer1.timer0, rng).expect("wifi init"));
    let (mut controller, _interfaces) =
        esp_wifi::wifi::new(wifi_init, peripherals.WIFI).expect("wifi controller");

    controller
        .set_configuration(&Configuration::Client(ClientConfiguration::default()))
        .expect("sta configuration");

    match controller.start_async().await {
        Ok(()) => info!("sta started"),
        Err(e) => info!("sta start FAILED: {e:?}"),
    }

    match controller.capabilities() {
        Ok(c) => info!("capabilities: {c:?}"),
        Err(e) => info!("capabilities failed: {e:?}"),
    }

    let mut round = 0u32;
    loop {
        round += 1;
        info!("--- scan {round} ---");
        match controller.scan_n_async(20).await {
            Ok(found) => {
                info!("found {} networks", found.len());
                for ap in &found {
                    info!(
                        "  ch{:>2}  {:>4} dBm  {:?}  {}",
                        ap.channel, ap.signal_strength, ap.auth_method, ap.ssid
                    );
                }
            }
            Err(e) => info!("scan failed: {e:?}"),
        }
        Timer::after(Duration::from_secs(8)).await;
    }
}
