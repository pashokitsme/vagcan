#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

//! Recon probe: the C3 as a Wi-Fi access point serving one styled page.
//!
//! Static IP only — no DHCP server yet, so a client has to be configured by
//! hand. That is deliberate: it proves the radio, smoltcp and TCP without
//! dragging in `edge-dhcp`, which is the next increment.

extern crate alloc;

use alloc::string::String;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{
    IpEndpoint, Ipv4Address, Ipv4Cidr, Runner, Stack, StackResources, StaticConfigV4,
};
use embassy_time::{Duration, Timer};
use embedded_io_async::Write;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::rng::Rng;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_wifi::wifi::{
    AccessPointConfiguration, AuthMethod, Configuration, WifiController, WifiDevice, WifiEvent,
    WifiState,
};
use log::info;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

const SSID: &str = "vagcan-dash";
/// esp-wifi 0.15 README, "Missing / To be done": *Support for non-open SoftAP*.
/// A secured AP takes the configuration, returns Ok from start, and never
/// beacons. So: open, and the refusals have to live above the radio.
const PASSWORD: &str = "";
/// The gateway address the AP answers on. 192.168.71/24 is picked to be
/// unlikely to collide with whatever the phone's other networks use.
const AP_ADDR: Ipv4Address = Ipv4Address::new(192, 168, 71, 1);
const PREFIX_LEN: u8 = 24;
/// Channel 6: the scan found neighbours on 1, 4, 5 and 11, and 1/6/11 are the
/// only non-overlapping channels in 2.4 GHz.
const CHANNEL: u8 = 6;

static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
// Embassy tasks are `'static`, so anything they borrow must outlive `main`.
static WIFI_INIT: StaticCell<esp_wifi::EspWifiController<'static>> = StaticCell::new();

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 64 * 1024);

    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);

    // SuperMini's blue LED sinks through GPIO8: Low is lit, High is dark.
    let led = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());

    let mut rng = Rng::new(peripherals.RNG);
    let timer1 = TimerGroup::new(peripherals.TIMG0);
    let wifi_init = WIFI_INIT.init(esp_wifi::init(timer1.timer0, rng).expect("wifi init"));
    let (mut controller, interfaces) =
        esp_wifi::wifi::new(wifi_init, peripherals.WIFI).expect("wifi controller");

    controller
        .set_configuration(&Configuration::AccessPoint(AccessPointConfiguration {
            ssid: String::from(SSID),
            password: String::from(PASSWORD),
            auth_method: AuthMethod::None,
            channel: CHANNEL,
            max_connections: 4,
            ..Default::default()
        }))
        .expect("ap configuration");
    controller.start_async().await.expect("ap start");
    info!("AP up: ssid={SSID} (open) ch{CHANNEL}");

    let net_config = embassy_net::Config::ipv4_static(StaticConfigV4 {
        address: Ipv4Cidr::new(AP_ADDR, PREFIX_LEN),
        gateway: Some(AP_ADDR),
        dns_servers: Default::default(),
    });

    // smoltcp wants a seed it cannot guess itself; the hardware RNG has one.
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());

    let (stack, runner) = embassy_net::new(
        interfaces.ap,
        net_config,
        RESOURCES.init(StackResources::new()),
        seed,
    );

    // Never `.ok()` a spawn: the arena is finite and a full one fails silently,
    // which looks exactly like the task running and doing nothing.
    if let Err(e) = spawner.spawn(wifi_events_task(controller)) {
        info!("SPAWN wifi_events FAILED: {e:?}");
    }
    if let Err(e) = spawner.spawn(net_task(runner)) {
        info!("SPAWN net FAILED: {e:?}");
    }
    if let Err(e) = spawner.spawn(dhcp_task(stack)) {
        info!("SPAWN dhcp FAILED: {e:?}");
    }
    if let Err(e) = spawner.spawn(http_task(stack)) {
        info!("SPAWN http FAILED: {e:?}");
    }
    if let Err(e) = spawner.spawn(led_task(led)) {
        info!("SPAWN led FAILED: {e:?}");
    }
    info!("all tasks spawned");

    loop {
        Timer::after(Duration::from_secs(5)).await;
        info!(
            "alive; ap_state={:?} link_up={} addr={:?}",
            esp_wifi::wifi::ap_state(),
            stack.is_link_up(),
            stack.config_v4().map(|c| c.address)
        );
    }
}

/// The AP's state, on the one output the board has without a screen.
///
/// It reads `ap_state()` rather than a flag we set ourselves: the radio's own
/// state machine is the authority, and a flag would only ever repeat what we
/// already believed.
///
/// Deliberately a task of its own, and deliberately not a driver callback —
/// blinking from inside an esp-wifi event handler is how the AP was wedged.
#[embassy_executor::task]
async fn led_task(mut led: Output<'static>) -> ! {
    loop {
        if esp_wifi::wifi::ap_state() == WifiState::ApStarted {
            // A short flash once a second: unmistakable, and dark most of the time.
            led.set_low();
            Timer::after(Duration::from_millis(100)).await;
            led.set_high();
            Timer::after(Duration::from_millis(900)).await;
        } else {
            led.set_high();
            Timer::after(Duration::from_millis(250)).await;
        }
    }
}

/// The radio's own account of what the clients are doing. A probe request seen
/// but no association means the handshake fails, not the beacon.
#[embassy_executor::task]
async fn wifi_events_task(mut controller: WifiController<'static>) -> ! {
    loop {
        let events = controller
            .wait_for_events(
                WifiEvent::ApStaconnected
                    | WifiEvent::ApStadisconnected
                    | WifiEvent::ApProbereqrecved,
                true,
            )
            .await;
        info!("wifi event: {events:?}");
    }
}

#[embassy_executor::task]
async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) -> ! {
    runner.run().await
}

const PAGE: &str = r#"<!doctype html>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>vagcan dash</title>
<style>
  :root { color-scheme: dark; }
  body {
    margin: 0; min-height: 100vh;
    display: grid; place-items: center;
    background: #0d0f12; color: #e6e6e6;
    font: 16px/1.5 ui-monospace, SFMono-Regular, Menlo, monospace;
  }
  .card {
    padding: 2rem 2.5rem; border: 1px solid #2a2f36; border-radius: 12px;
    background: #14181d; box-shadow: 0 8px 32px #0008; text-align: center;
  }
  h1 { margin: 0 0 .5rem; font-size: 1.6rem; letter-spacing: .04em; color: #7fd1a0; }
  p  { margin: .25rem 0; color: #9aa4b1; }
  code { color: #d8b46a; }
</style>
<div class="card">
  <h1>hello world</h1>
  <p>served by an <code>ESP32-C3</code></p>
  <p>no_std &middot; esp-hal &middot; embassy-net &middot; smoltcp</p>
</div>
"#;

#[embassy_executor::task]
async fn http_task(stack: Stack<'static>) -> ! {
    let mut rx = [0u8; 1536];
    let mut tx = [0u8; 1536];
    let mut req = [0u8; 1024];

    loop {
        let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
        socket.set_timeout(Some(Duration::from_secs(10)));

        if let Err(e) = socket.accept(80).await {
            info!("accept failed: {e:?}");
            continue;
        }
        info!("client {:?}", socket.remote_endpoint());

        // Read one chunk; enough to see the request line. A real server would
        // read until CRLFCRLF, this probe only needs to prove the round trip.
        match socket.read(&mut req).await {
            Ok(0) | Err(_) => {
                socket.abort();
                continue;
            }
            Ok(n) => {
                let head = core::str::from_utf8(&req[..n])
                    .unwrap_or("<non-utf8>")
                    .lines()
                    .next()
                    .unwrap_or("");
                info!("request: {head}");
            }
        }

        let mut headers = [0u8; 128];
        let headers = {
            use core::fmt::Write as _;
            let mut w = Writer(&mut headers, 0);
            let _ = write!(
                w,
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                PAGE.len()
            );
            let used = w.1;
            &w.0[..used]
        };

        let _ = socket.write_all(headers).await;
        let _ = socket.write_all(PAGE.as_bytes()).await;
        let _ = socket.flush().await;
        socket.close();
        Timer::after(Duration::from_millis(50)).await;
        socket.abort();
    }
}

/// `core::fmt::Write` into a fixed slice — no allocation for the header block.
struct Writer<'a>(&'a mut [u8], usize);

impl core::fmt::Write for Writer<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let end = self.1 + s.len();
        if end > self.0.len() {
            return Err(core::fmt::Error);
        }
        self.0[self.1..end].copy_from_slice(s.as_bytes());
        self.1 = end;
        Ok(())
    }
}

/// A DHCP server, so a phone can simply join instead of being told an address.
///
/// `edge-dhcp` handles the protocol and nothing else — it takes a decoded
/// request and hands back a reply packet. Carrying that over the wire is our
/// job, and the one thing that is not obvious: the reply must be *broadcast*,
/// because the client has no address yet and so cannot be addressed.
#[embassy_executor::task]
async fn dhcp_task(stack: Stack<'static>) -> ! {
    let mut rx_meta = [PacketMetadata::EMPTY; 8];
    let mut tx_meta = [PacketMetadata::EMPTY; 8];
    let mut rx = [0u8; 1024];
    let mut tx = [0u8; 1024];
    let mut buf = [0u8; 1500];

    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut rx, &mut tx_meta, &mut tx);
    socket.bind(67).expect("bind dhcp");
    info!("dhcp: listening on udp/67, pool {}..{}", "x.x.x.50", "x.x.x.200");

    let mut server: edge_dhcp::server::Server<_, 8> =
        edge_dhcp::server::Server::new(|| embassy_time::Instant::now().as_secs(), AP_ADDR);
    let mut gw_buf = [AP_ADDR];
    let server_options = edge_dhcp::server::ServerOptions::new(AP_ADDR, Some(&mut gw_buf));

    loop {
        let (n, _meta) = match socket.recv_from(&mut buf).await {
            Ok(v) => v,
            Err(e) => {
                info!("dhcp recv: {e:?}");
                continue;
            }
        };

        let request = match edge_dhcp::Packet::decode(&buf[..n]) {
            Ok(p) => p,
            Err(e) => {
                info!("dhcp decode: {e:?}");
                continue;
            }
        };

        let mut opt_buf = edge_dhcp::Options::buf();
        let mut out = [0u8; 1024];
        let reply = server.handle_request(&mut opt_buf, &server_options, &request);

        if let Some(reply) = reply {
            match reply.encode(&mut out) {
                Ok(bytes) => {
                    let to = IpEndpoint::new(Ipv4Address::BROADCAST.into(), 68);
                    if let Err(e) = socket.send_to(bytes, to).await {
                        info!("dhcp send: {e:?}");
                    } else {
                        info!("dhcp: offered/acked {:?}", reply.yiaddr);
                    }
                }
                Err(e) => info!("dhcp encode: {e:?}"),
            }
        }
    }
}
