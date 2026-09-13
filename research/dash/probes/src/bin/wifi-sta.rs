//! Station mode: join an existing network, take a DHCP lease, and prove the
//! link carries traffic rather than merely associating.
//!
//! Associating is the easy half and the half that lies: a board can be
//! "connected" with no address, or hold an address on a network that drops
//! every packet. So this reports four separate facts — associated, addressed,
//! resolved, connected — and does not collapse them into one.
//!
//! Credentials come from the environment at build time:
//!
//! ```sh
//! WIFI_SSID='…' WIFI_PASSWORD='…' cargo build --release --bin sta
//! ```
//!
//! They are deliberately not in the source. This firmware will move into the
//! repository sooner or later and a password in a checked-in file is the kind
//! of thing that is noticed years later by the wrong person.

#![no_std]
#![no_main]

use embassy_executor::Spawner;
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_net::{Runner, StackResources};
use embassy_time::{Duration, Timer, with_timeout};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rng::Rng;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_wifi::wifi::{ClientConfiguration, Configuration, WifiController, WifiDevice, WifiEvent, WifiState};
use log::{info, warn};
use static_cell::StaticCell;

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

const SSID: &str = env!("WIFI_SSID");
const PASSWORD: &str = env!("WIFI_PASSWORD");

/// Somewhere to resolve and connect to. A name rather than an address, because
/// resolving is the step that fails when a network gives out leases and
/// nothing else.
const PROBE_HOST: &str = "example.com";
const PROBE_PORT: u16 = 80;

static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
static WIFI_INIT: StaticCell<esp_wifi::EspWifiController<'static>> = StaticCell::new();

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    esp_println::logger::init_logger_from_env();

    let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    esp_alloc::heap_allocator!(size: 72 * 1024);

    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);

    let led = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());

    let mut rng = Rng::new(peripherals.RNG);
    let timer1 = TimerGroup::new(peripherals.TIMG0);
    let wifi_init = WIFI_INIT.init(esp_wifi::init(timer1.timer0, rng).expect("wifi init"));
    let (controller, interfaces) = esp_wifi::wifi::new(wifi_init, peripherals.WIFI).expect("wifi controller");

    // DHCP client, not a static address: the whole point is to be a guest on
    // somebody else's network.
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let (stack, runner) = embassy_net::new(
        interfaces.sta,
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );

    // Never `.ok()` a spawn: the arena is finite and a full one fails silently.
    if let Err(e) = spawner.spawn(connection_task(controller)) {
        warn!("SPAWN connection FAILED: {e:?}");
    }
    if let Err(e) = spawner.spawn(net_task(runner)) {
        warn!("SPAWN net FAILED: {e:?}");
    }
    if let Err(e) = spawner.spawn(led_task(led)) {
        warn!("SPAWN led FAILED: {e:?}");
    }

    info!("joining {SSID}");

    // 1. Associated.
    while !stack.is_link_up() {
        Timer::after(Duration::from_millis(250)).await;
    }
    info!("STEP 1/4 associated with {SSID}");

    // 2. Addressed.
    stack.wait_config_up().await;
    let config = stack.config_v4().expect("v4 config");
    info!(
        "STEP 2/4 DHCP lease: {} gateway {:?} dns {:?}",
        config.address, config.gateway, config.dns_servers
    );

    // 3. Resolved. A lease without working DNS is a common and confusing state,
    // so it gets its own line rather than being folded into the connect.
    let resolved = match with_timeout(
        Duration::from_secs(10),
        stack.dns_query(PROBE_HOST, DnsQueryType::A),
    )
    .await
    {
        Ok(Ok(addresses)) => match addresses.first() {
            Some(address) => {
                info!("STEP 3/4 {PROBE_HOST} resolves to {addresses:?}");
                Some(*address)
            }
            None => {
                warn!("STEP 3/4 {PROBE_HOST} resolved to nothing — link is up, DNS is not");
                None
            }
        },
        Ok(Err(e)) => {
            warn!("STEP 3/4 DNS failed: {e:?}");
            None
        }
        Err(_) => {
            warn!("STEP 3/4 DNS timed out — an IoT network may have no route out, which is normal");
            None
        }
    };

    // 4. Connected. Falls back to the gateway when there is no route out, so
    // that a walled-garden network still proves TCP works on the local link.
    let (target, label) = match resolved {
        Some(address) => ((address, PROBE_PORT), PROBE_HOST),
        None => match config.gateway {
            Some(gateway) => ((gateway.into(), PROBE_PORT), "the gateway"),
            None => {
                warn!("STEP 4/4 nothing to connect to");
                idle().await
            }
        },
    };

    // The socket owns its buffers for as long as it lives, so the reply needs
    // its own; reusing the receive buffer is a borrow error rather than a
    // clever saving.
    let mut rx = [0u8; 1024];
    let mut tx = [0u8; 512];
    let mut reply = [0u8; 128];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    socket.set_timeout(Some(Duration::from_secs(10)));
    match socket.connect(target).await {
        Ok(()) => info!("STEP 4/4 TCP connected to {label}:{PROBE_PORT}"),
        Err(e) => {
            warn!("STEP 4/4 TCP to {label} failed: {e:?}");
            idle().await
        }
    }

    // Ask for something, so that "connected" is not the last word: a firewall
    // that accepts the handshake and drops the payload looks identical until
    // bytes are asked for.
    use embedded_io_async::Write as _;
    let request = b"HEAD / HTTP/1.0\r\nHost: example.com\r\n\r\n";
    if let Err(e) = socket.write_all(request).await {
        warn!("write failed: {e:?}");
        idle().await
    }
    match with_timeout(Duration::from_secs(10), socket.read(&mut reply)).await {
        Ok(Ok(n)) if n > 0 => {
            let head = core::str::from_utf8(&reply[..n.min(64)]).unwrap_or("<not utf-8>");
            info!("answered {n} bytes: {}", head.lines().next().unwrap_or(""));
        }
        Ok(Ok(_)) => warn!("connection closed with no answer"),
        Ok(Err(e)) => warn!("read failed: {e:?}"),
        Err(_) => warn!("no answer in 10 s"),
    }
    socket.close();

    idle().await
}

async fn idle() -> ! {
    loop {
        info!("sta idle, state {:?}", esp_wifi::wifi::sta_state());
        Timer::after(Duration::from_secs(15)).await;
    }
}

/// Joins, and rejoins. A station that gives up on the first disconnect is
/// useless in a car park; reconnection is the normal case, not the error path.
#[embassy_executor::task]
async fn connection_task(mut controller: WifiController<'static>) -> ! {
    loop {
        if esp_wifi::wifi::sta_state() == WifiState::StaConnected {
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            warn!("disconnected, retrying in 3 s");
            Timer::after(Duration::from_secs(3)).await;
        }
        if !matches!(controller.is_started(), Ok(true)) {
            controller
                .set_configuration(&Configuration::Client(ClientConfiguration {
                    ssid: SSID.into(),
                    password: PASSWORD.into(),
                    ..Default::default()
                }))
                .expect("client configuration");
            controller.start_async().await.expect("wifi start");
            info!("wifi started");
        }
        match controller.connect_async().await {
            Ok(()) => info!("associated"),
            Err(e) => {
                // The 802.11 reason code is in here and it is the difference
                // between a wrong password and a router that is not listening.
                warn!("connect failed: {e:?}");
                Timer::after(Duration::from_secs(3)).await;
            }
        }
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) -> ! {
    runner.run().await
}

/// Off while unassociated, a slow pulse once there is a lease.
#[embassy_executor::task]
async fn led_task(mut led: Output<'static>) -> ! {
    loop {
        if esp_wifi::wifi::sta_state() == WifiState::StaConnected {
            led.set_low();
            Timer::after(Duration::from_millis(80)).await;
            led.set_high();
            Timer::after(Duration::from_millis(920)).await;
        } else {
            led.set_high();
            Timer::after(Duration::from_millis(200)).await;
        }
    }
}
