//! BLE central probe: scan and print what is on the air.
//!
//! The mirror of `peri.rs`, and the same self-test the Wi-Fi `scan` binary was:
//! if the radio hears nothing at all, the antenna is the suspect, not the stack.

#![no_std]
#![no_main]

use bt_hci::cmd::le::LeSetScanParams;
use bt_hci::controller::{ControllerCmdSync, ExternalController};
use core::cell::RefCell;
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_wifi::ble::controller::BleConnector;
use heapless::Deque;
use log::info;
use trouble_host::prelude::*;

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

const CONNECTIONS_MAX: usize = 1;
const L2CAP_CHANNELS_MAX: usize = 1;

#[esp_hal_embassy::main]
async fn main(_spawner: Spawner) {
    esp_println::logger::init_logger_from_env();

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);

    let rng = esp_hal::rng::Rng::new(peripherals.RNG);
    let timer1 = TimerGroup::new(peripherals.TIMG0);
    let wifi_init = esp_wifi::init(timer1.timer0, rng).expect("radio init");

    let transport = BleConnector::new(&wifi_init, peripherals.BT);
    let controller: ExternalController<_, 20> = ExternalController::new(transport);

    run(controller).await;
}

async fn run<C>(controller: C)
where
    C: Controller + ControllerCmdSync<LeSetScanParams>,
{
    let address = Address::random([0xf3, 0xa6, 0x1c, 0x11, 0x5e, 0xc3]);
    info!("BLE address = {address:?}");

    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> = HostResources::new();
    let stack = trouble_host::new(controller, &mut resources).set_random_address(address);
    let Host { central, mut runner, .. } = stack.build();

    let printer = Printer {
        seen: RefCell::new(Deque::new()),
    };
    let mut scanner = Scanner::new(central);

    let _ = join(runner.run_with_handler(&printer), async {
        let mut config = ScanConfig::default();
        config.active = true;
        config.phys = PhySet::M1;
        config.interval = Duration::from_secs(1);
        config.window = Duration::from_secs(1);
        let _session = scanner.scan(&config).await.expect("scan start");
        info!("scanning");
        loop {
            Timer::after(Duration::from_secs(5)).await;
            info!("heap:\n{}", esp_alloc::HEAP.stats());
        }
    })
    .await;
}

struct Printer {
    /// Bounded on purpose: an unbounded set of every address ever seen is a
    /// slow leak on a device that sits in a car park all day.
    seen: RefCell<Deque<BdAddr, 64>>,
}

impl EventHandler for Printer {
    fn on_adv_reports(&self, mut it: LeAdvReportsIter<'_>) {
        let mut seen = self.seen.borrow_mut();
        while let Some(Ok(report)) = it.next() {
            if !seen.iter().any(|b| b.raw() == report.addr.raw()) {
                info!("discovered {:?} rssi {:?}", report.addr, report.rssi);
                if seen.is_full() {
                    seen.pop_front();
                }
                let _ = seen.push_back(report.addr);
            }
        }
    }
}
