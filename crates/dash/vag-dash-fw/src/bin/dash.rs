//! BLE peripheral probe: three real profiles, not a toy service.
//!
//! * **DIS** (0x180A) — Device Information: who this thing is. Every scanner
//!   and every OS reads it, and it costs three constant strings.
//! * **BAS** (0x180F) — Battery Service. Android shows this next to the device
//!   name; on a car-powered dash it will eventually report the rail, not a cell.
//! * **NUS** — Nordic UART Service. Not a SIG profile, but the de-facto one:
//!   it is what every "BLE terminal" app speaks, and it is the honest
//!   replacement for the Bluetooth-Classic SPP that `09` was written around
//!   before the board turned out to be a C3.
//!
//! What none of these do is put the device in the phone's *Settings* list —
//! see `10-c3-recon.md`. That needs HID-over-GATT and nothing else.

#![no_std]
#![no_main]
#![deny(
	clippy::mem_forget,
	reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use alloc::vec::Vec;
use bt_hci::controller::ExternalController;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicU8, Ordering};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{Either3, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::twai::filter::SingleStandardFilter;
use esp_hal::twai::{BaudRate, StandardId, TwaiConfiguration, TwaiMode};
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_wifi::ble::controller::BleConnector;
use log::{info, warn};
use static_cell::StaticCell;
use trouble_host::prelude::*;
use vag_dash_fw::can::TwaiBackend;
use vag_dash_fw::config::{Config, PageKind};
use vag_dash_fw::panel::Framebuffer;
use vag_dash_fw::plan::{CHANNEL_COUNT, PLAN, UNIT_COUNT};
use vag_dash_fw::store::{Error as StoreError, Store};
use vag_dash_fw::ui::{ADVERTISE_WINDOW_SECS, Button, DEBOUNCE_MS, Press, Visibility};
use vag_dash_render::plan::{Page as PlanPage, Unit};
use vag_uds_can::IsoTpCan;
use vag_uds_client::identity::did;
use vag_uds_client::{AsyncUdsClient, UdsError};
use vag_uds_transport::{CanId, TransportError};

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

/// The name the phone shows. Kept in sync with the Wi-Fi AP so one device is
/// recognisable in both lists.
const DEVICE_NAME: &str = "vagcan-dash";

/// One central at a time is all a dash needs.
const CONNECTIONS_MAX: usize = 1;
/// Signalling + ATT.
const L2CAP_CHANNELS_MAX: usize = 2;

/// The longest payload the UART service carries in one go.
///
/// This is the characteristic's *storage*, and it is the real limit: a write
/// longer than this is refused by the server with ATT `Invalid Offset` (0x07)
/// no matter how large the negotiated MTU is. macOS negotiates ATT MTU 251,
/// so 248 bytes are usable on the air; 244 leaves room for a framing header
/// and stays inside the MTU-255 packet pool.
const UART_MTU: usize = 244;

type UartData = heapless::Vec<u8, UART_MTU>;

/// The GATT macro backs every characteristic with `[u8; T::MAX_SIZE]`, so the
/// type must have a bounded size. `&'static str` does not — its `MAX_SIZE` is
/// `usize::MAX` and the array fails to lay out. A `heapless::String` does.
type DisString = heapless::String<16>;

fn dis(s: &str) -> DisString {
	DisString::try_from(s).expect("DIS string too long")
}

#[gatt_server]
struct Server {
	// Read over the air, never from Rust — the compiler cannot see that.
	#[allow(dead_code)]
	dis: DeviceInformationService,
	bas: BatteryService,
	uart: NordicUartService,
}

/// 0x180A. Static strings, read-only — this is what "a profile" mostly is.
#[gatt_service(uuid = service::DEVICE_INFORMATION)]
struct DeviceInformationService {
	#[characteristic(uuid = characteristic::MANUFACTURER_NAME_STRING, read, value = dis("vagcan"))]
	manufacturer: DisString,
	#[characteristic(uuid = characteristic::MODEL_NUMBER_STRING, read, value = dis("dash-c3"))]
	model: DisString,
	#[characteristic(uuid = characteristic::FIRMWARE_REVISION_STRING, read, value = dis("recon-0.1"))]
	firmware: DisString,
}

/// 0x180F. Reports a fake ramp for now; the real one is the 12 V rail through
/// the divider that `08` puts on an ADC pin.
#[gatt_service(uuid = service::BATTERY)]
struct BatteryService {
	#[characteristic(uuid = characteristic::BATTERY_LEVEL, read, notify, value = 100)]
	level: u8,
}

/// The Nordic UART Service. Note the direction names are from the *central's*
/// point of view, which is the usual source of confusion: `rx` is what the
/// phone writes to us, `tx` is what we notify back.
#[gatt_service(uuid = "6e400001-b5a3-f393-e0a9-e50e24dcca9e")]
struct NordicUartService {
	#[characteristic(uuid = "6e400002-b5a3-f393-e0a9-e50e24dcca9e", write, write_without_response)]
	rx: UartData,
	#[characteristic(uuid = "6e400003-b5a3-f393-e0a9-e50e24dcca9e", read, notify)]
	tx: UartData,
}

/// Everything the configuration commands touch. One task owns it, so a
/// `RefCell` is the whole synchronisation story — no mutex, no static.
struct Settings {
	/// `None` when the board was flashed against the default partition table
	/// and has nowhere to keep anything. The panel still works; it just forgets.
	store: Option<Store>,
	config: Config,
	/// Set by every change, cleared by a save. Without it, "did that survive?"
	/// is answered by a reboot instead of by looking.
	unsaved: bool,
}

/// Shared because two tasks touch it: the button cycles pages, the GATT
/// handler edits and saves. An **async** mutex, not a blocking one — a save
/// erases and writes a flash sector, and holding a critical section for that
/// long would stall the radio.
type Shared = Mutex<CriticalSectionRawMutex, Settings>;

static SETTINGS: StaticCell<Shared> = StaticCell::new();

/// Raised by a three-second hold. It both arms and cancels: the meaning is
/// decided by what the BLE loop is currently doing.
static LONG_PRESS: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Raised whenever something a connected client would want to know changes.
static STATE_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// `Visibility as u8`. An atomic rather than the mutex because the LED task
/// reads it constantly and must never wait for a flash write.
static VISIBILITY: AtomicU8 = AtomicU8::new(Visibility::Dark as u8);

/// Lines for the laptop, queued rather than printed.
///
/// The USB port carries the frame stream, and **exactly one task may write to
/// it**: a log line printed from anywhere else lands in the middle of a frame
/// and destroys it. Measured, not feared — logging the render report once per
/// frame corrupted 30 frames out of 30. So everything that wants to say
/// something puts it here and `panel_task` writes it between frames.
///
/// This is the same rule `10-c3-recon.md` records for Wi-Fi event handlers,
/// arrived at from the other direction: a callback may push to a channel; the
/// writing belongs to one task.
static NOTES: Channel<CriticalSectionRawMutex, heapless::String<128>, 8> = Channel::new();

/// Queue a line for the laptop. Drops it if the queue is full rather than
/// waiting: a note is never worth stalling the thing it is describing.
macro_rules! note {
    ($($arg:tt)*) => {{
        let mut line: heapless::String<128> = heapless::String::new();
        let _ = write!(line, $($arg)*);
        let _ = NOTES.try_send(line);
    }};
}

/// Presses arriving from the panel simulator over USB. They go through the
/// **same** handling as the physical button rather than a parallel path — a
/// test rig that exercises different code from the real thing tests the rig.
static REMOTE_PRESS: Signal<CriticalSectionRawMutex, Press> = Signal::new();

fn visibility() -> Visibility {
	match VISIBILITY.load(Ordering::Relaxed) {
		1 => Visibility::Advertising,
		2 => Visibility::Connected,
		_ => Visibility::Dark,
	}
}

fn set_visibility(v: Visibility) {
	VISIBILITY.store(v as u8, Ordering::Relaxed);
}

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
	esp_println::logger::init_logger_from_env();

	let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
	esp_alloc::heap_allocator!(size: 72 * 1024);

	let timer0 = SystemTimer::new(peripherals.SYSTIMER);
	esp_hal_embassy::init(timer0.alarm0);

	// Reset reason, last panic, watchdog — armed here, before the radio, so a
	// hang during start-up reboots too. See `health.rs`.
	vag_dash_fw::health::init(spawner);

	let led = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());
	// GPIO9 is the SuperMini's BOOT button: a real button, already fitted, so
	// the whole interaction can be proven before anything is wired. On the
	// finished device it moves to one of GPIO0..5, which are the pins that can
	// wake the chip from deep sleep.
	let button = Input::new(peripherals.GPIO9, InputConfig::default().with_pull(Pull::Up));

	// Which car this image is for, said once, before anything is asked of the
	// bus: a plan and a car that disagree is the first thing to look for.
	info!(
		"plan: VIN {}, {} unit(s), {} channel(s), {} page(s), labels in {:?}",
		PLAN.vin,
		PLAN.units.len(),
		PLAN.channels.len(),
		PLAN.pages.len(),
		PLAN.language
	);
	for unit in PLAN.units {
		info!("plan: unit {:03X}/{:03X} part {}", unit.request, unit.response, unit.part_number);
	}
	note!(
		"plan: VIN {} — {} unit(s), {} channel(s)",
		PLAN.vin,
		PLAN.units.len(),
		PLAN.channels.len()
	);

	// Settings are read before the radio starts: a panel that cannot find its
	// configuration should say so at boot, not when somebody connects.
	let settings: &'static Shared = SETTINGS.init(Mutex::new(open_settings()));

	// The bus. GPIO1 reads the transceiver's RXD, GPIO6 drives its TXD — see
	// the pin table in `sleep.rs` for why those two. **Normal mode, not the
	// listen-only default `can.rs` argues for**, and the reason is the whole
	// job: a `0x22` request has to be transmitted, and a controller that
	// cannot acknowledge cannot be answered either. What goes out is the same
	// single-identifier read `vagcan watch` already puts on this bus from the
	// laptop, through the same client and the same allowlist; nothing here can
	// widen it.
	let mut twai = TwaiConfiguration::new(peripherals.TWAI0, peripherals.GPIO1, peripherals.GPIO6, BaudRate::B500K, TwaiMode::Normal);
	// The controller is told which ids to hand up **before** it is started,
	// because a car's powertrain bus is not quiet: the engine alone broadcasts
	// thousands of frames a second, the driver's receive queue is 32 deep, and
	// a full queue drops what arrives next. Without a filter that "next" is the
	// answer to the request we just sent, and the first run on the car said so
	// — every exchange opened by sweeping the queue's full 32 entries. The
	// filter is the plan's own answer ids, so it narrows to what this image
	// polls and nothing about any car reaches the source.
	if let Some(filter) = response_filter() {
		twai.set_filter(filter);
	}
	let twai = twai.into_async().start();
	let backend = TwaiBackend::new(twai);

	let rng = esp_hal::rng::Rng::new(peripherals.RNG);
	let timer1 = TimerGroup::new(peripherals.TIMG0);
	let wifi_init = esp_wifi::init(timer1.timer0, rng).expect("radio init");

	// The controller stays up for the life of the device and only *advertising*
	// is gated. Tearing the controller down would free ~46 KB and reintroduce
	// the one allocation pattern that can fragment this heap (see 11-ble.md);
	// gating advertising is a single HCI command and costs nothing.
	let transport = BleConnector::new(&wifi_init, peripherals.BT);
	let controller: ExternalController<_, 20> = ExternalController::new(transport);

	// One driver owns the USB port for the frame stream and the command line.
	// `esp-println` keeps writing its log to the same peripheral; the two
	// interleave at worst within a line, and a mangled frame is *reported* by
	// the simulator rather than drawn, which is the failure mode to want.
	let (usb_rx, usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async().split();

	// Never `.ok()` a spawn: the arena is finite and a full one fails silently.
	if let Err(e) = spawner.spawn(panel_task(usb_tx, settings)) {
		warn!("SPAWN panel FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(remote_task(usb_rx)) {
		warn!("SPAWN remote FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(led_task(led)) {
		warn!("SPAWN led FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(button_task(button, settings)) {
		warn!("SPAWN button FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(heap_task()) {
		warn!("SPAWN heap FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(can_task(backend)) {
		warn!("SPAWN can FAILED: {e:?}");
	}

	run(controller, settings).await;
}

fn open_settings() -> Settings {
	match Store::open() {
		Ok(mut store) => {
			let (offset, len, used) = store.partition();
			info!("config partition at 0x{offset:06x}, {len} bytes ({used} in use: 2 slots)");
			match store.load() {
				// A stored configuration is checked against *this* plan before
				// it is trusted: it may have been saved by an image with more
				// channels, and a cell past the end of the plan is nothing.
				Ok(config) => match config.validate() {
					Ok(()) => {
						info!("config loaded, generation {}: {config:?}", store.generation());
						Settings {
							store: Some(store),
							config,
							unsaved: false,
						}
					}
					Err(reason) => {
						warn!(
							"config generation {} does not fit this plan ({reason}), running on defaults",
							store.generation()
						);
						// `unsaved`: what runs and what flash holds now disagree,
						// and `state` should say so rather than claim they match.
						Settings {
							store: Some(store),
							config: Config::default(),
							unsaved: true,
						}
					}
				},
				Err(StoreError::Empty) => {
					info!("nothing stored yet, running on defaults");
					Settings {
						store: Some(store),
						config: Config::default(),
						unsaved: false,
					}
				}
				Err(e) => {
					warn!("config unreadable ({e:?}), running on defaults");
					Settings {
						store: Some(store),
						config: Config::default(),
						unsaved: false,
					}
				}
			}
		}
		Err(e) => {
			warn!("no settings storage ({e:?}) — changes will not survive a reboot");
			Settings {
				store: None,
				config: Config::default(),
				unsaved: false,
			}
		}
	}
}

/// Polls the button, debounces it, and acts.
///
/// A short press moves to the next page. When `04`'s alarms exist, a short
/// press *while an alarm is showing* silences that episode instead — the
/// button is modal because the screen already says which mode it is in.
#[embassy_executor::task]
async fn button_task(button: Input<'static>, settings: &'static Shared) -> ! {
	let mut machine = Button::new();
	loop {
		// Half the debounce interval: fast enough that no edge is missed,
		// slow enough to be free.
		let press = match embassy_futures::select::select(Timer::after(Duration::from_millis(DEBOUNCE_MS / 2)), REMOTE_PRESS.wait()).await {
			embassy_futures::select::Either::First(()) => machine.poll(button.is_low(), embassy_time::Instant::now().as_millis()),
			embassy_futures::select::Either::Second(press) => Some(press),
		};
		match press {
			Some(Press::Short) => {
				let mut s = settings.lock().await;
				let pages = s.config.pages.len() as u8;
				if pages > 0 {
					s.config.active_page = (s.config.active_page + 1) % pages;
					s.unsaved = true;
					note!("button: page {} of {}", s.config.active_page, pages);
				}
				drop(s);
				STATE_CHANGED.signal(());
			}
			Some(Press::Long) => {
				info!(
					"[button] held — {}",
					match visibility() {
						Visibility::Dark => "allowing configuration",
						_ => "closing configuration",
					}
				);
				LONG_PRESS.signal(());
			}
			None => {}
		}
	}
}

/// The only thing that says what state the device is in while there is no
/// panel: dark is off, advertising is a hurried blink, connected is a slow
/// double pulse.
#[embassy_executor::task]
async fn led_task(mut led: Output<'static>) -> ! {
	loop {
		match visibility() {
			Visibility::Dark => {
				led.set_high();
				Timer::after(Duration::from_millis(200)).await;
			}
			Visibility::Advertising => {
				led.set_low();
				Timer::after(Duration::from_millis(60)).await;
				led.set_high();
				Timer::after(Duration::from_millis(140)).await;
			}
			Visibility::Connected => {
				for _ in 0..2 {
					led.set_low();
					Timer::after(Duration::from_millis(40)).await;
					led.set_high();
					Timer::after(Duration::from_millis(120)).await;
				}
				Timer::after(Duration::from_millis(1200)).await;
			}
		}
	}
}

/// Prints the heap every fifteen seconds. The number that matters is not
/// `Current usage` but whether `Total allocated` drifts up while the workload
/// is unchanged: only turnover can fragment a first-fit heap.
#[embassy_executor::task]
async fn heap_task() -> ! {
	loop {
		Timer::after(Duration::from_secs(15)).await;
		info!("heap:\n{}", esp_alloc::HEAP.stats());
	}
}

async fn run<C: Controller>(controller: C, settings: &'static Shared) {
	// A fixed random address keeps the device recognisable across reflashes.
	// A shipping device would derive this from its own MAC.
	let address = Address::random([0xf2, 0xa6, 0x1c, 0x11, 0x5e, 0xc3]);
	info!("BLE address = {address:?}");

	let mut resources: HostResources<DefaultPacketPool, CONNECTIONS_MAX, L2CAP_CHANNELS_MAX> = HostResources::new();
	let stack = trouble_host::new(controller, &mut resources).set_random_address(address);
	let Host { mut peripheral, runner, .. } = stack.build();

	let server = Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
		name: DEVICE_NAME,
		appearance: &appearance::power_device::GENERIC_POWER_DEVICE,
	}))
	.expect("gatt server");

	info!("heap after host build:\n{}", esp_alloc::HEAP.stats());

	let _ = join(ble_task(runner), async {
		loop {
			set_visibility(Visibility::Dark);
			info!("dark — hold the button for 3 s to allow configuration");
			LONG_PRESS.wait().await;

			set_visibility(Visibility::Advertising);
			let advertiser = match start_advertising(DEVICE_NAME, &mut peripheral).await {
				Ok(a) => a,
				Err(e) => {
					warn!("[adv] could not start: {e:?}");
					Timer::after(Duration::from_secs(1)).await;
					continue;
				}
			};
			info!("[adv] advertising as {DEVICE_NAME} for {ADVERTISE_WINDOW_SECS} s");

			match select3(
				advertiser.accept(),
				Timer::after(Duration::from_secs(ADVERTISE_WINDOW_SECS)),
				LONG_PRESS.wait(),
			)
			.await
			{
				Either3::First(Ok(conn)) => match conn.with_attribute_server(&server) {
					Ok(conn) => {
						set_visibility(Visibility::Connected);
						info!("[adv] connected");
						select3(
							gatt_events_task(&server, &conn, settings),
							state_task(&server, &conn, settings),
							battery_task(&server, &conn),
						)
						.await;
						// Whatever ended it, the device goes dark. It does NOT
						// return to advertising: requiring someone to press the
						// button again is the entire point — reaching this
						// device means standing next to it.
						info!("[adv] connection over, going dark");
					}
					Err(e) => warn!("[adv] attribute server: {e:?}"),
				},
				Either3::First(Err(e)) => warn!("[adv] accept failed: {e:?}"),
				Either3::Second(()) => info!("[adv] window closed with nobody connected"),
				Either3::Third(()) => info!("[adv] cancelled by the button"),
			}
			// Dropping the advertiser cancels advertising; `Advertiser::drop`
			// sends the cancel for us, which is why nothing does it by hand.
		}
	})
	.await;
}

/// Must run forever alongside everything else; it is the host's pump.
async fn ble_task<C: Controller, P: PacketPool>(mut runner: Runner<'_, C, P>) {
	loop {
		if let Err(e) = runner.run().await {
			panic!("[ble_task] error: {e:?}");
		}
	}
}

async fn start_advertising<'values, C: Controller>(
	name: &'values str,
	peripheral: &mut Peripheral<'values, C, DefaultPacketPool>,
) -> Result<Advertiser<'values, C, DefaultPacketPool>, BleHostError<C::Error>> {
	// 31 bytes is the whole budget. Flags (3) + name (2 + 11) leaves no room
	// for a 128-bit UUID (18), so the UART service UUID goes in the scan
	// response — which is exactly what the scan response is for.
	let mut adv_data = [0u8; 31];
	let adv_len = AdStructure::encode_slice(
		&[
			AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
			AdStructure::ServiceUuids16(&[service::DEVICE_INFORMATION.to_le_bytes(), service::BATTERY.to_le_bytes()]),
			AdStructure::CompleteLocalName(name.as_bytes()),
		],
		&mut adv_data[..],
	)?;
	let mut scan_data = [0u8; 31];
	let scan_len = AdStructure::encode_slice(&[AdStructure::ServiceUuids128(&[NUS_UUID_LE])], &mut scan_data[..])?;

	peripheral
		.advertise(
			&Default::default(),
			Advertisement::ConnectableScannableUndirected {
				adv_data: &adv_data[..adv_len],
				scan_data: &scan_data[..scan_len],
			},
		)
		.await
}

/// 6e400001-b5a3-f393-e0a9-e50e24dcca9e, little-endian as the air format wants.
const NUS_UUID_LE: [u8; 16] = [
	0x9e, 0xca, 0xdc, 0x24, 0x0e, 0xe5, 0xa9, 0xe0, 0x93, 0xf3, 0xa3, 0xb5, 0x01, 0x00, 0x40, 0x6e,
];

/// Renders the one line that describes the device completely enough for a
/// client to draw its own view of it.
async fn state_line(settings: &Shared) -> heapless::String<UART_MTU> {
	let mut out: heapless::String<UART_MTU> = heapless::String::new();
	let s = settings.lock().await;
	let _ = write!(
		out,
		"state page={}/{} brightness={} unsaved={} gen={}",
		s.config.active_page,
		s.config.pages.len(),
		s.config.brightness,
		u8::from(s.unsaved),
		s.store.as_ref().map_or(0, Store::generation)
	);
	if let Some(page) = s.config.pages.get(usize::from(s.config.active_page)) {
		let kind = match page.kind {
			PageKind::Chart => "chart",
			PageKind::Values => "values",
		};
		let _ = write!(out, " kind={kind} cells={:?}", page.cells);
	}
	out
}

/// Pushes the state to the client: once on connecting, and again whenever the
/// button changes something. A client that has to poll to notice a button
/// press is a client that shows the wrong thing most of the time.
async fn state_task<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>, settings: &Shared) {
	let tx = &server.uart.tx;
	loop {
		let line = state_line(settings).await;
		let mut out: UartData = heapless::Vec::new();
		let _ = out.extend_from_slice(line.as_bytes());
		if server.set(tx, &out).is_ok() {
			let _ = tx.notify(conn, &out).await;
		}
		STATE_CHANGED.wait().await;
	}
}

async fn gatt_events_task<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>, settings: &Shared) {
	let rx = &server.uart.rx;
	let tx = &server.uart.tx;
	let reason = loop {
		match conn.next().await {
			GattConnectionEvent::Disconnected { reason } => break reason,
			GattConnectionEvent::Gatt { event } => {
				if let GattEvent::Write(e) = &event {
					if e.handle() == rx.handle {
						let data = e.data();
						let reply = command(settings, data).await;
						let mut out: UartData = heapless::Vec::new();
						let _ = out.extend_from_slice(&reply.as_bytes()[..reply.len().min(UART_MTU)]);
						if server.set(tx, &out).is_ok() {
							let _ = tx.notify(conn, &out).await;
						}
					}
				}
				// Dropping the event also replies, but the reply is the point,
				// so send it where it can be seen to fail.
				match event.accept() {
					Ok(reply) => reply.send().await,
					Err(e) => warn!("[gatt] reply failed: {e:?}"),
				}
			}
			_ => {}
		}
	};
	info!("[gatt] disconnected: {reason:?}");
}

/// A visible, standard-profile heartbeat: the battery percentage ramps down so
/// the client has something changing to show. `08`'s divider from the 12 V rail
/// is what eventually feeds this.
async fn battery_task<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>) {
	let level = &server.bas.level;
	let mut pct: u8 = 100;
	loop {
		Timer::after(Duration::from_secs(5)).await;
		pct = if pct == 0 { 100 } else { pct - 1 };
		if server.set(level, &pct).is_err() {
			break;
		}
		// Fails until someone subscribes; not a reason to stop.
		let _ = level.notify(conn, &pct).await;
	}
}

/// The last thing the car said about one channel, and when.
///
/// `value` is what the panel draws; `None` is a dash, never a zero. `at` is
/// there so a value the bus stopped refreshing does not stay on the glass
/// looking current — see [`STALE`].
#[derive(Clone, Copy)]
struct Slot {
	value: Option<f32>,
	at: Option<Instant>,
}

impl Slot {
	const EMPTY: Slot = Slot { value: None, at: None };

	/// The value, unless it is older than [`STALE`].
	fn current(&self, now: Instant) -> Option<f32> {
		let at = self.at?;
		if now.saturating_duration_since(at) > STALE {
			return None;
		}
		self.value
	}
}

/// How old a value may be and still be shown. A poll cycle is well under a
/// second; a value nobody has refreshed for five is a bus that went quiet,
/// and the panel should say so rather than hold the last number.
const STALE: Duration = Duration::from_secs(5);

/// One slot per plan channel — the whole of what the CAN task tells the panel
/// task. Sized by the plan, so a channel the plan does not have has nowhere to
/// be stored, which is the same property `config.rs` has for cells.
static VALUES: Mutex<CriticalSectionRawMutex, [Slot; CHANNEL_COUNT]> = Mutex::new([Slot::EMPTY; CHANNEL_COUNT]);

/// How long one `0x22` waits for its answer. Short on purpose: a unit that
/// is there answers in milliseconds, and a unit that is not should not cost
/// the others a second each.
const READ_TIMEOUT: core::time::Duration = core::time::Duration::from_millis(300);
/// The most one exchange may take, all in. The transport's receive has
/// [`READ_TIMEOUT`]; its send has no deadline of its own, and what every
/// state of the controller does to a transmit future is not this loop's to
/// find out — so the whole exchange gets one, and past it the request is
/// dropped (which aborts the transmission) and counted as no answer.
const EXCHANGE_DEADLINE: Duration = Duration::from_secs(1);
/// Between two reads of one unit — a control unit's diagnostic server is not
/// its day job, and back-to-back requests are how a server gets crowded.
const READ_GAP: Duration = Duration::from_millis(50);
/// Between two full passes over every unit.
const CYCLE_GAP: Duration = Duration::from_millis(200);
/// Between two passes when **no** unit answers — ignition off, most likely.
/// The device hangs off permanent battery positive, and a request every third
/// of a second is a request that may keep the gateway awake (`07-sleep.md`,
/// `08-power.md`); one every two seconds is a different order of thing. This
/// puts nothing to sleep; it only stops hammering a bus that is not listening.
const DEAD_BUS_GAP: Duration = Duration::from_secs(2);
/// After a bus-off, before the restarted controller is asked anything.
const BUS_OFF_GAP: Duration = Duration::from_secs(1);

/// What the part-number check has established about one unit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
	/// Not asked yet.
	Pending,
	/// Asked, no answer; said so once. Asked again next cycle — and a unit
	/// that answered once and then fell silent comes back here, so that a bus
	/// which has gone quiet is asked one thing per cycle, not everything.
	Absent,
	/// Answered with the part number the plan was built against.
	Matched,
	/// Answered with a different one. Never polled again this run: the plan's
	/// identifiers would answer, plausibly, about a unit they were not
	/// resolved for.
	Mismatch,
}

/// The controller's acceptance filter: the ids this plan's units answer on.
///
/// Hardware filtering is not an optimisation here, it is what makes the
/// answer arrive at all. The frames a powertrain bus carries are almost
/// entirely other people's, esp-hal's async driver queues every accepted one
/// in a 32-deep channel, and a `try_send` into a full channel drops the
/// frame. So on a live bus an unfiltered controller fills that queue between
/// the request and its answer, and the answer is what falls off the end.
///
/// The filter is one ESP32 "single standard" filter: an 11-bit code and, in
/// this API's direction, a mask whose set bits are the ones that must match.
/// Two answer ids that differ in one bit (`7E8`/`7E9` is the usual pair) cost
/// that bit; ids scattered further make the filter a superset, which is
/// harmless — [`vag_uds_can::IsoTpCan`] compares the id exactly, and always
/// did. Data frames only: nothing in UDS is remote-transmission.
///
/// `None` when the plan polls nobody, since a filter matching nothing would
/// be the same silence with a harder-to-find cause.
fn response_filter() -> Option<SingleStandardFilter> {
	/// An 11-bit id is all a standard filter can hold.
	const SFF: u16 = 0x7FF;
	let (mut common, mut any) = (SFF, 0u16);
	for unit in PLAN.units {
		common &= unit.response & SFF;
		any |= unit.response & SFF;
	}
	if PLAN.units.is_empty() {
		return None;
	}
	// A bit that is set in one answer id and clear in another cannot be
	// insisted on, so it is dropped from the mask.
	let must_match = !(any & !common) & SFF;
	Some(SingleStandardFilter::new_from_code_mask(
		StandardId::new(common)?,
		StandardId::new(must_match)?,
		// RTR clear, and that much is insisted on.
		false,
		true,
		// Any payload: the first two bytes are an ISO-TP header, not a key.
		[0, 0],
		[0, 0],
	))
}

/// One `0x22` exchange with one unit, and what came of it.
struct Exchange {
	backend: TwaiBackend<'static>,
	answer: Result<Vec<u8>, UdsError>,
	/// Entries swept out of the receive queue before asking.
	swept: usize,
}

/// One exchange: sweep, wrap, ask, unwrap.
///
/// The wrappers are stateless, so building them per exchange costs nothing,
/// and the sweep is what it buys: whatever the controller queued *since the
/// last answer* — a reply that came in past its deadline, a frame from
/// another tester on the same response id — is thrown away here rather than
/// taken as this request's answer, which is how every read of a unit ends up
/// one behind until a request gets nothing back.
async fn exchange(mut backend: TwaiBackend<'static>, unit: &Unit, did: u16) -> Exchange {
	let swept = backend.drain().await;
	let mut uds = AsyncUdsClient::new(IsoTpCan::new(backend, CanId::Standard(unit.request), CanId::Standard(unit.response)));
	let answer = match with_timeout(EXCHANGE_DEADLINE, uds.read_data_by_identifier_within(did, READ_TIMEOUT)).await {
		Ok(answer) => answer,
		Err(_elapsed) => Err(UdsError::Transport(TransportError::Timeout)),
	};
	Exchange {
		backend: uds.into_transport().into_backend(),
		answer,
		swept,
	}
}

/// Nothing came back at all — as opposed to a refusal, which is an answer.
fn silent(answer: &Result<Vec<u8>, UdsError>) -> bool {
	matches!(answer, Err(UdsError::Transport(TransportError::Timeout)))
}

/// The controller after an exchange: restarted if it went bus-off, and the
/// gap before the next request either way.
///
/// Bus-off is sticky. Once the controller has counted its way there, esp-hal
/// answers every transmit and receive with `BusOff` until it is put through
/// reset mode again — so without this, one bad moment on the bus would be
/// dashes until somebody pulled the plug. `stop()` hands back the
/// configuration, mode and all; `start()` clears the error counters and
/// leaves reset; both keep the async driver.
async fn settle(backend: TwaiBackend<'static>, answer: &Result<Vec<u8>, UdsError>, bus_off: &mut bool) -> TwaiBackend<'static> {
	if let Err(UdsError::Transport(TransportError::Disconnected)) = answer {
		if !*bus_off {
			*bus_off = true;
			note!("can: controller went bus-off — restarting it");
		}
		let backend = TwaiBackend::new(backend.into_twai().stop().start());
		Timer::after(BUS_OFF_GAP).await;
		return backend;
	}
	if *bus_off {
		*bus_off = false;
		note!("can: controller is back on the bus");
	}
	Timer::after(READ_GAP).await;
	backend
}

/// Says, once, that the sweep found something. The mechanism is silent by
/// design; one line is what proves it earned its place.
fn sweep(swept: usize, unit: &Unit, did: u16, said: &mut bool) {
	if swept > 0 && !*said {
		*said = true;
		note!(
			"can: swept {swept} stale frame(s) before {:03X} {:04X} — a late reply, or another tester",
			unit.request,
			did
		);
	}
}

/// Polls the plan's channels off the car and keeps [`VALUES`] current.
///
/// **One conversation at a time, re-addressed per exchange** — the same
/// shape as `vag-cli-core`'s `read_batch`, and not by accident: there is one
/// CAN controller, one ISO-TP state, and a unit is a `(request, response)`
/// pair the transport is built around. So the backend is wrapped for one
/// read, unwrapped, swept, and wrapped for the next. No actor, no queue,
/// nothing in flight while another unit is being asked.
///
/// Before a unit is polled its part number is read and compared to the
/// plan's (`05`, "the car check"). This image is built for one car; on
/// another the same identifiers answer and the answers mean something else.
/// A mismatch is said once and the unit is left alone for the rest of the
/// run. No answer is said once and retried — ignition off looks like that —
/// and when no unit answers at all the retries slow to [`DEAD_BUS_GAP`].
///
/// Every failure lands in the store as `None` before anything else happens,
/// so the panel never shows a number the bus has stopped confirming; what is
/// *said* about a failure is said on the change, because the USB line is
/// shared with the frame stream and a note per tick would be noise.
#[embassy_executor::task]
async fn can_task(mut backend: TwaiBackend<'static>) -> ! {
	let mut checks = [Check::Pending; UNIT_COUNT];
	// Per channel: whether the last read decoded, or `None` before the first.
	let mut answering = [None::<bool>; CHANNEL_COUNT];
	let mut swept_said = false;
	let mut bus_off = false;
	let mut dead_bus = false;

	loop {
		for (u, unit) in PLAN.units.iter().enumerate() {
			if checks[u] == Check::Mismatch {
				continue;
			}
			if checks[u] != Check::Matched {
				let ex = exchange(backend, unit, did::PART_NUMBER).await;
				sweep(ex.swept, unit, did::PART_NUMBER, &mut swept_said);
				checks[u] = judge(unit, &ex.answer, checks[u]);
				backend = settle(ex.backend, &ex.answer, &mut bus_off).await;
				if checks[u] != Check::Matched {
					continue;
				}
			}

			let mut heard = false;
			for (index, channel) in PLAN.channels_of(unit) {
				let ex = exchange(backend, unit, channel.did).await;
				sweep(ex.swept, unit, channel.did, &mut swept_said);
				heard |= !silent(&ex.answer);
				let value = ex.answer.as_ref().ok().and_then(|data| channel.decode(data));
				store(index, value).await;

				let slot = &mut answering[usize::from(index)];
				if *slot != Some(value.is_some()) {
					*slot = Some(value.is_some());
					match &ex.answer {
						Ok(data) if value.is_none() => note!(
							"can: {:03X} {:04X} answered {} byte(s), the plan wants bits {}+{}",
							unit.request,
							channel.did,
							data.len(),
							channel.bit_offset,
							channel.bit_length
						),
						Ok(_) => note!("can: {:03X} {:04X} {} is answering", unit.request, channel.did, channel.label),
						Err(e) => note!("can: {:03X} {:04X} {}: {e}", unit.request, channel.did, channel.label),
					}
				}
				backend = settle(ex.backend, &ex.answer, &mut bus_off).await;
			}
			// A unit that said nothing to a whole pass is asked one thing
			// per cycle from here on — its part number — until it speaks.
			if !heard && PLAN.channels_of(unit).next().is_some() {
				checks[u] = Check::Absent;
				note!("can: {:03X} went silent — will keep asking", unit.request);
			}
		}

		let dead = UNIT_COUNT > 0 && checks.iter().all(|c| *c == Check::Absent);
		if dead != dead_bus {
			dead_bus = dead;
			if dead {
				note!("can: no unit answers — asking every {} s until one does", DEAD_BUS_GAP.as_secs());
			} else {
				note!("can: the bus is answering again");
			}
		}
		Timer::after(if dead_bus { DEAD_BUS_GAP } else { CYCLE_GAP }).await;
	}
}

/// What the unit's part number says, against the plan's.
///
/// `F187` carries the number padded — one trailing space on the reference
/// car, and a NUL is the other thing a fixed-width field is padded with — so
/// the padding is trimmed before comparing, exactly as the survey that the
/// plan was built from trimmed it.
fn judge(unit: &Unit, answer: &Result<Vec<u8>, UdsError>, previous: Check) -> Check {
	match answer {
		Ok(data) => {
			let reported = core::str::from_utf8(data).map(|s| s.trim_end_matches([' ', '\0']));
			match reported {
				Ok(reported) if reported == unit.part_number => {
					note!("can: {:03X} is {} as planned", unit.request, unit.part_number);
					Check::Matched
				}
				Ok(reported) => {
					note!(
						"can: {:03X} is {reported:?}, the plan was built for {} — not polling it",
						unit.request,
						unit.part_number
					);
					Check::Mismatch
				}
				Err(_) => {
					note!(
						"can: {:03X} answered F187 with {:02X?}, not a part number — not polling it",
						unit.request,
						data
					);
					Check::Mismatch
				}
			}
		}
		Err(e) => {
			if previous != Check::Absent {
				note!("can: {:03X} did not answer F187 ({e}) — will keep asking", unit.request);
			}
			Check::Absent
		}
	}
}

/// Puts one reading in the store. `None` clears the slot, timestamp and all.
async fn store(index: u16, value: Option<f32>) {
	let mut values = VALUES.lock().await;
	if let Some(slot) = values.get_mut(usize::from(index)) {
		*slot = Slot {
			value,
			at: value.map(|_| Instant::now()),
		};
	}
}

/// The fixed range the plan gives a chart of this channel, if it gives one.
fn chart_range(index: u16) -> Option<(f32, f32)> {
	PLAN.pages.iter().find_map(|page| match page {
		PlanPage::Chart { channel, min, max } if *channel == index => Some((*min, *max)),
		_ => None,
	})
}

/// Draws the current page and ships the pixels out of the USB port.
///
/// This is the real renderer on real pixels: `vag_dash_render::draw` into a 256×64
/// framebuffer. What the laptop shows is not an impression of the panel, it is
/// the panel.
///
/// Labels, units and decimals come from the plan; values come from
/// [`VALUES`], where the CAN task left them. A cell whose channel has not
/// answered is drawn with `None`, and the renderer draws a dash. Nothing here
/// invents a number.
#[embassy_executor::task]
async fn panel_task(mut usb: esp_hal::usb_serial_jtag::UsbSerialJtagTx<'static, esp_hal::Async>, settings: &'static Shared) -> ! {
	use vag_dash_render::{Cell, Frame, Theme, draw};

	static FRAMEBUFFER: StaticCell<Framebuffer> = StaticCell::new();
	let framebuffer = FRAMEBUFFER.init(Framebuffer::new());
	let theme = Theme::bold_mono();

	// One sample per pixel column, oldest first — the chart's own rule. One
	// sample is taken per frame, so the window is width ÷ frame rate, and
	// that is only true of a trace with no holes in it: a frame with no
	// value, or a frame that was not this chart, ends the trace, and the next
	// value starts a new one. Joining across a gap would draw ten minutes on
	// another page, or five seconds of silence, as one continuous line.
	let mut history = [0.0f32; vag_dash_fw::panel::WIDTH];
	let mut filled = 0usize;
	// Which chart the previous frame drew, if it drew one.
	let mut charted: Option<u16> = None;
	let mut last_compromised = false;
	// A chart page whose channel the plan gives no range for is said once.
	let mut no_range_said: Option<u16> = None;

	loop {
		// Five frames a second: fast enough to look live over a terminal,
		// slow enough that the encoding never becomes the bottleneck.
		Timer::after(Duration::from_millis(200)).await;

		let (kind, indices) = {
			let s = settings.lock().await;
			let page = s.config.pages.get(usize::from(s.config.active_page));
			match page {
				Some(page) => (page.kind, page.cells.clone()),
				None => continue,
			}
		};
		// A copy, so the lock is held for a memcpy and not for a frame.
		let values = *VALUES.lock().await;
		let now = Instant::now();
		let value_of = |index: u16| values.get(usize::from(index)).and_then(|slot| slot.current(now));
		// A cell the plan cannot name draws as a question mark rather than
		// vanishing: a missing column hides the fault, a wrong one shows it.
		let cell_of = |index: u16| match PLAN.channel(index) {
			Some(channel) => Cell::new(channel.label, value_of(index), channel.unit_text, channel.decimals),
			None => Cell::new("?", None, "", 0),
		};

		framebuffer.clear_all();
		// What this frame charts, if anything; compared with `charted` next time.
		let mut charting: Option<u16> = None;
		let report = match kind {
			PageKind::Values => {
				let mut cells: heapless::Vec<Cell<'_>, 4> = heapless::Vec::new();
				for index in indices.iter().take(4) {
					let _ = cells.push(cell_of(*index));
				}
				draw(&Frame::Values { cells: &cells }, &theme, framebuffer)
			}
			PageKind::Chart => {
				let index = indices.first().copied().unwrap_or(0);
				match chart_range(index) {
					Some((min, max)) => {
						if charted != Some(index) {
							filled = 0;
						}
						charting = Some(index);
						let cell = cell_of(index);
						match cell.value {
							Some(value) if filled < history.len() => {
								history[filled] = value;
								filled += 1;
							}
							Some(value) => {
								history.rotate_left(1);
								history[history.len() - 1] = value;
							}
							None => filled = 0,
						}
						draw(
							&Frame::Chart {
								cell,
								min,
								max,
								samples: &history[..filled],
								window_seconds: filled as f32 * 0.2,
							},
							&theme,
							framebuffer,
						)
					}
					// No range means no chart: the range is fixed by the plan
					// or there is none, and autoscale is not a fallback (`02`).
					// The value is still shown, as a one-cell page.
					None => {
						if no_range_said != Some(index) {
							no_range_said = Some(index);
							note!("panel: the plan has no chart range for channel {index} — showing it as a value");
						}
						let cells = [cell_of(index)];
						draw(&Frame::Values { cells: &cells }, &theme, framebuffer)
					}
				}
			}
		};
		charted = charting;
		// The renderer reports what it had to compromise — a label too long,
		// a unit it had to drop. It is the same answer every frame, so say it
		// when it changes and never otherwise.
		let compromised = report.label_overrun || report.value_overrun || report.unit_dropped || report.value_shrunk || report.glyph_missing;
		if compromised != last_compromised {
			last_compromised = compromised;
			note!("panel: {report:?}");
		}

		use embedded_io_async::Write as _;
		// Notes first, whole lines, before the frame starts. Nothing may be
		// written between the frame's first byte and its newline.
		while let Ok(note) = NOTES.try_receive() {
			let _ = usb.write_all(note.as_bytes()).await;
			let _ = usb.write_all(b"\r\n").await;
		}
		let mut line: heapless::String<{ vag_dash_fw::panel::WIDTH * vag_dash_fw::panel::HEIGHT / 8 * 2 + 32 }> = heapless::String::new();
		if framebuffer.write_frame(&mut line).is_ok() {
			let _ = usb.write_all(line.as_bytes()).await;
			let _ = usb.write_all(b"\r\n").await;
		}
	}
}

/// Reads `BTN S` / `BTN L` from the simulator and injects them as presses.
#[embassy_executor::task]
async fn remote_task(mut usb: esp_hal::usb_serial_jtag::UsbSerialJtagRx<'static, esp_hal::Async>) -> ! {
	use embedded_io_async::Read as _;
	let mut buffer = [0u8; 64];
	let mut line: heapless::String<64> = heapless::String::new();
	loop {
		// The async read cannot fail on this peripheral, but going through
		// `Result` keeps the shape right if it ever moves to a UART.
		let n = usb.read(&mut buffer).await.unwrap_or(0);
		for &byte in &buffer[..n] {
			if byte == b'\n' || byte == b'\r' {
				match line.trim() {
					"BTN S" => REMOTE_PRESS.signal(Press::Short),
					"BTN L" => REMOTE_PRESS.signal(Press::Long),
					"" => {}
					other => note!("remote: ignoring {other:?}"),
				}
				line.clear();
			} else if line.push(byte as char).is_err() {
				// A line longer than anything we understand is a desynchronised
				// stream, not a command. Drop it rather than let it wrap.
				line.clear();
			}
		}
	}
}

/// The configuration protocol, in its first and deliberately dumbest form:
/// lines of text. A binary framing with a CRC is what a real client wants, but
/// text is what a person with a terminal can drive, and being able to drive it
/// by hand is worth more right now than being able to parse it fast.
async fn command(settings: &Shared, raw: &[u8]) -> heapless::String<UART_MTU> {
	let mut out: heapless::String<UART_MTU> = heapless::String::new();
	let Ok(line) = core::str::from_utf8(raw) else {
		let _ = write!(out, "err: not utf-8");
		return out;
	};
	let line = line.trim();
	let mut words = line.split_whitespace();

	match words.next() {
		Some("help") => {
			let _ = write!(out, "state | get | set brightness N | set page N | save | load | defaults | erase");
		}
		// The machine-readable form of `get`. A client that has to parse prose
		// is a client that breaks when the prose is improved.
		Some("state") => return state_line(settings).await,
		Some("get") => {
			let s = settings.lock().await;
			let _ = write!(
				out,
				"brightness {} page {} of {}",
				s.config.brightness,
				s.config.active_page,
				s.config.pages.len()
			);
			for (i, page) in s.config.pages.iter().enumerate() {
				let kind = match page.kind {
					PageKind::Chart => "chart",
					PageKind::Values => "values",
				};
				let _ = write!(out, " | {i}:{kind}{:?}", page.cells);
			}
			let _ = write!(
				out,
				" | {} gen {}",
				if s.unsaved { "UNSAVED" } else { "saved" },
				s.store.as_ref().map_or(0, Store::generation)
			);
		}
		Some("set") => match (words.next(), words.next()) {
			(Some("brightness"), Some(value)) => match value.parse::<u8>() {
				Ok(v) => {
					let mut s = settings.lock().await;
					s.config.brightness = v;
					s.unsaved = true;
					let _ = write!(out, "ok: brightness {v}");
				}
				Err(_) => {
					let _ = write!(out, "err: brightness takes 0..255");
				}
			},
			(Some("page"), Some(value)) => match value.parse::<u8>() {
				Ok(v) => {
					let mut s = settings.lock().await;
					if usize::from(v) >= s.config.pages.len() {
						let _ = write!(out, "err: only {} pages", s.config.pages.len());
					} else {
						s.config.active_page = v;
						s.unsaved = true;
						let _ = write!(out, "ok: page {v}");
					}
				}
				Err(_) => {
					let _ = write!(out, "err: page takes a number");
				}
			},
			_ => {
				let _ = write!(out, "err: set brightness N | set page N");
			}
		},
		Some("save") => {
			let mut s = settings.lock().await;
			if let Err(reason) = s.config.validate() {
				let _ = write!(out, "err: refusing to save — {reason}");
				return out;
			}
			let config = s.config.clone();
			match s.store.as_mut() {
				None => {
					let _ = write!(out, "err: no config partition on this board");
				}
				Some(store) => match store.save(&config) {
					Ok(generation) => {
						s.unsaved = false;
						let _ = write!(out, "ok: saved, generation {generation}");
					}
					Err(e) => {
						let _ = write!(out, "err: save failed {e:?}");
					}
				},
			}
		}
		Some("load") => {
			let mut s = settings.lock().await;
			match s.store.as_mut() {
				None => {
					let _ = write!(out, "err: no config partition on this board");
				}
				Some(store) => match store.load() {
					Ok(config) => match config.validate() {
						Ok(()) => {
							s.config = config;
							s.unsaved = false;
							let _ = write!(out, "ok: reloaded from flash");
						}
						Err(reason) => {
							let _ = write!(out, "err: stored config does not fit this plan — {reason}");
						}
					},
					Err(e) => {
						let _ = write!(out, "err: {e:?}");
					}
				},
			}
		}
		Some("defaults") => {
			let mut s = settings.lock().await;
			s.config = Config::default();
			s.unsaved = true;
			let _ = write!(out, "ok: defaults in memory — 'save' to keep them");
		}
		Some("erase") => {
			let mut s = settings.lock().await;
			match s.store.as_mut() {
				None => {
					let _ = write!(out, "err: no config partition on this board");
				}
				Some(store) => match store.erase() {
					Ok(()) => {
						let _ = write!(out, "ok: erased — next boot uses defaults");
					}
					Err(e) => {
						let _ = write!(out, "err: {e:?}");
					}
				},
			}
		}
		Some(other) => {
			let _ = write!(out, "err: no such command '{other}' — try 'help'");
		}
		None => {
			let _ = write!(out, "err: empty");
		}
	}
	out
}
