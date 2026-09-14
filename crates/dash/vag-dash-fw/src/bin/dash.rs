//! The dash: reads the plan's channels off the car, draws them, and serves the
//! car over BLE.
//!
//! * **The bus** is one task, `can_task`, and one scheduler:
//!   [`vag_uds_client::schedule::Planner`]. The panel subscribes to the
//!   channels of the page on the glass at their own rates and to the rest at
//!   1 Hz; a BLE host's requests and subscriptions go through the same planner
//!   (`todo/dash/14` §2). One exchange is in flight at a time.
//! * **BLE** is always visible (owner, 2026-09-13/14): the board advertises from
//!   boot, accepts one central, serves it, and advertises again when it leaves.
//!   Two real profiles: **DIS** (0x180A, who this thing is) and **NUS**, the
//!   Nordic UART Service, which carries `dashcfg`'s text commands and the framed
//!   UDS link (`vag_uds_transport::link`) side by side. A host across the radio
//!   is not trusted, so every framed request passes the board's own guard
//!   (`vag_uds_client::guard`, `todo/dash/16`).
//! * **USB** carries the same framed link, for a host on the cable, which is trusted
//!   more (`Guard::cable`): a second client of the planner with a session of its own,
//!   beside the BLE one. The cable also carries `dashsim`'s `FRAME` and `BTN` lines and
//!   the log, and **one task writes it** (`usb_writer_task`), so no line lands inside
//!   a frame. What each incoming byte is, `vag_uds_client::console` decides.
//! * **Adapter mode** (`todo/dash/14` §3, mode 2): an slcan command line on the cable
//!   while no link client holds a session there makes the board a plain slcan adapter
//!   — `vagcan --slcan` — running `vag_dash_fw::slcan`, the standalone `slcan` image's
//!   bridge. The planner then sends nothing (its subscriptions stay), framed requests
//!   over either carrier are refused, and the panel shows the adapter's counters. It
//!   ends on `C`, or when the host's start-of-frame packets stop (the cable pulled).
//! * **Alarms** (`todo/dash/04`): the plan's `[[alarm]]` rules are read at full rate on
//!   every page; past a threshold the rule's page takes the glass with the offending
//!   cell inverted, and a short press silences the episode
//!   ([`vag_dash_render::screen`]). The adapter screen runs none.
//!
//! There is no Battery Service (0x180F). Phones show its level as the device's
//! battery, and this board has no battery and no reading of the rail (the
//! divider was retired), so anything it notified there would be invented — it
//! once was, a 100→0 ramp. It comes back only with a real measurement behind it.
//!
//! What none of these do is put the device in the phone's *Settings* list —
//! see `.archive/tasks/done/dash/10-c3-recon.md`. That needs HID-over-GATT and nothing else.

#![no_std]
#![no_main]
#![deny(
	clippy::mem_forget,
	reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use alloc::vec::Vec;
use bt_hci::controller::ExternalController;
use core::cell::RefCell;
use core::fmt::Write as _;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_futures::select::{Either, Either3, Either4, select, select3, select4};
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::mutex::Mutex;
use embassy_sync::signal::Signal;
use embassy_sync::waitqueue::MultiWakerRegistration;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_backtrace as _;
use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Input, InputConfig, Pull};
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::peripherals::{GPIO1, GPIO6, TWAI0};
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::twai::filter::SingleStandardFilter;
use esp_hal::twai::{BaudRate, StandardId, TwaiConfiguration, TwaiMode};
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_wifi::ble::controller::BleConnector;
use log::{info, warn};
use static_cell::StaticCell;
use trouble_host::prelude::*;
use vag_dash_fw::can::TwaiBackend;
use vag_dash_fw::config::{Config, PageKind};
use vag_dash_fw::panel::Framebuffer;
use vag_dash_fw::plan::{ALARM_COUNT, CHANNEL_COUNT, CHART_COUNT, PLAN, UNIT_COUNT};
use vag_dash_fw::slcan::{self, Adapter, CommandLine, Commands, Line, Packet, Port};
use vag_dash_fw::store::{Error as StoreError, Store};
use vag_dash_fw::ui::{Button, DEBOUNCE_MS, Press, Visibility};
use vag_dash_fw::usb;
use vag_dash_render::alarm::{self, ChannelId};
use vag_dash_render::pages::Mismatch;
use vag_dash_render::plan::{PartAnswer, PartCheck};
use vag_dash_render::screen::{Change, Screen};
use vag_uds_can::{FilterFollower, IsoTpCan, StandardFilter};
use vag_uds_client::console::{self, Console, Ignored, Input as ConsoleInput, Mode};
use vag_uds_client::guard::{Guard, MAX_SUBSCRIPTIONS};
use vag_uds_client::identity::did;
use vag_uds_client::remote::Session;
use vag_uds_client::schedule::{Answer, Budget, Class, Delivery, Miss, Next, Planner, ReqId, SubId, Unit, expects_no_answer};
use vag_uds_transport::link::{self, HelloReply, Message, Piece, Reassembler};
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

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

/// The payload of one notification before the central has negotiated anything:
/// ATT's default MTU of 23 less the 3-byte notification header (Bluetooth Core,
/// Vol 3 Part F §3.2.8). The floor under [`notify_size`].
const ATT_DEFAULT_PAYLOAD: usize = 20;

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
	dis: DeviceInformationService,
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

/// The one scheduler of the bus, shared by the bus task and the BLE session.
///
/// A **blocking** mutex, held only for a planner call — a few map operations,
/// microseconds — and never across an `.await`. What waits on the bus waits
/// outside it.
type Bus = BlockingMutex<NoopRawMutex, RefCell<Planner>>;

static BUS: StaticCell<Bus> = StaticCell::new();

/// Raised by whoever gives the planner work while the bus task sleeps, so a
/// BLE request does not wait for the panel's next due read.
static BUS_WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Raised whenever the pages or the page on the glass may have changed, so the
/// panel's subscriptions follow.
static PAGES_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The page the panel last drew — the cursor's, or an alarm's during a takeover — for the
/// bus task, which reads the page on the glass in the foreground. [`NO_PAGE`] before the
/// first frame, when the cursor's page is the one about to be drawn.
static GLASS_PAGE: AtomicU8 = AtomicU8::new(NO_PAGE);
const NO_PAGE: u8 = u8::MAX;

/// Raised whenever something a connected client would want to know changes.
static STATE_CHANGED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Queue a line for the laptop, in [`usb::LINES`] with the log. Drops it if the queue
/// is full rather than waiting: a note is never worth stalling the thing it is
/// describing.
///
/// The USB port carries the frame stream, and **exactly one task may write to
/// it** (`usb_writer_task`): a log line printed from anywhere else lands in the middle
/// of a frame and destroys it. This is the same rule
/// `.archive/tasks/done/dash/10-c3-recon.md` records for Wi-Fi event handlers,
/// arrived at from the other direction: a callback may push to a channel; the
/// writing belongs to one task.
macro_rules! note {
    ($($arg:tt)*) => {
        vag_dash_fw::usb::say(format_args!($($arg)*))
    };
}

/// One framed-link client of the planner — the BLE connection, or the host on the USB
/// cable — as the bus task reaches it. Each has a session of its own.
struct Client {
	/// Answers to the session's raw exchanges: the request, the answer, and the
	/// board's clock when it arrived. The session waits for one at a time, so **one
	/// slot** holds every answer it will read; one left over from a session that is gone is
	/// ignored by the next (S-F4: a raw answer keeps its full ~4.1 KB, so the queue is one
	/// deep, not four).
	answers: Channel<CriticalSectionRawMutex, (ReqId, Answer, u64), 1>,
	/// Planner deliveries for the session's subscriptions. **Drop semantics**
	/// (owner, 2026-09-14): a reading that finds this full is thrown away and
	/// counted in `dropped` — the next one is newer anyway, and a bus that waited
	/// for a host would starve the panel. In steady state every reading is at most
	/// `MAX_READING_BYTES`: a subscription whose reading is larger ends on its first
	/// delivery (`Session::deliver`, S-F4), so a big record is delivered once and never
	/// again — the 16 slots hold ~0.5 KB each rather than filling with ~4.1 KB records.
	readings: Channel<CriticalSectionRawMutex, Delivery, 16>,
	dropped: AtomicU32,
	/// What the session owns now, where the bus task can see it: a delivery for none
	/// of it is nobody's and is dropped there (`remote`).
	owned: BlockingMutex<CriticalSectionRawMutex, RefCell<Owned>>,
}

struct Owned {
	subs: heapless::Vec<SubId, MAX_SUBSCRIPTIONS>,
	awaiting: Option<ReqId>,
	/// `Session::is_active`: while it holds, the USB console takes no slcan line.
	active: bool,
	/// `Session::queued_bytes`, for the reader that stops at `QUEUED_PDU_BYTES`.
	queued_bytes: usize,
}

impl Client {
	const fn new() -> Self {
		Client {
			answers: Channel::new(),
			readings: Channel::new(),
			dropped: AtomicU32::new(0),
			owned: BlockingMutex::new(RefCell::new(Owned {
				subs: heapless::Vec::new(),
				awaiting: None,
				active: false,
				queued_bytes: 0,
			})),
		}
	}

	/// Publish what `session` owns now. Held for a copy of at most
	/// [`MAX_SUBSCRIPTIONS`] ids. Called after every step of the session with no await
	/// in between, so the bus task never sees an exchange it has not been told is owned.
	fn publish(&self, session: &Session) {
		self.owned.lock(|owned| {
			let mut owned = owned.borrow_mut();
			owned.subs.clear();
			for id in session.subscriptions() {
				let _ = owned.subs.push(id);
			}
			owned.awaiting = session.awaiting();
			owned.active = session.is_active();
			owned.queued_bytes = session.queued_bytes();
		});
	}

	fn queued_bytes(&self) -> usize {
		self.owned.lock(|owned| owned.borrow().queued_bytes)
	}

	fn owns(&self, sub: SubId) -> bool {
		self.owned.lock(|owned| owned.borrow().subs.contains(&sub))
	}

	fn awaits(&self, req: ReqId) -> bool {
		self.owned.lock(|owned| owned.borrow().awaiting == Some(req))
	}

	fn active(&self) -> bool {
		self.owned.lock(|owned| owned.borrow().active)
	}

	/// A new session: nothing queued for the last one is this one's, and drops counted
	/// before it subscribed anything are not its drops.
	fn reset(&self) {
		self.answers.clear();
		self.readings.clear();
		self.dropped.store(0, Ordering::Relaxed);
	}

	/// Say, on each doubling, that readings were dropped. `said` is the count last said.
	fn say_dropped(&self, carrier: &str, said: &mut u32) {
		let dropped = self.dropped.load(Ordering::Relaxed);
		if dropped != *said && dropped >= said.saturating_mul(2).max(1) {
			*said = dropped;
			note!("{carrier}: {dropped} reading(s) dropped — the link is slower than its subscriptions");
		}
	}
}

static BLE_CLIENT: Client = Client::new();
static USB_CLIENT: Client = Client::new();

/// Which of its two jobs the board is doing ([`Mode`] as a byte): read by the BLE and USB
/// sessions, the bus task, the panel and the writer, each without waiting for another.
static MODE: AtomicU8 = AtomicU8::new(MODE_PANEL);
const MODE_PANEL: u8 = 0;
const MODE_ADAPTER: u8 = 1;

/// Raised with every mode change, one per consumer: a signal wakes one waiter.
static MODE_FOR_BUS: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static MODE_FOR_BLE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static MODE_FOR_USB: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// What a host asking for the bus in adapter mode is told.
const ADAPTER_MODE: &str = "the board is in adapter mode";

fn adapter_mode() -> bool {
	MODE.load(Ordering::Relaxed) == MODE_ADAPTER
}

fn set_mode(mode: Mode) {
	MODE.store(if mode == Mode::Adapter { MODE_ADAPTER } else { MODE_PANEL }, Ordering::Relaxed);
	MODE_FOR_BUS.signal(());
	MODE_FOR_BLE.signal(());
	MODE_FOR_USB.signal(());
	STATE_CHANGED.signal(());
}

/// Bytes the central wrote to the UART service, in arrival order. Small on
/// purpose: a full one holds the next write's ATT response back, which is the
/// host's back-pressure.
static INBOX: Channel<CriticalSectionRawMutex, UartData, 4> = Channel::new();

/// What goes back to the central. One task notifies, so a framed message cut
/// into chunks is never interleaved with a text line.
static OUTBOX: Channel<CriticalSectionRawMutex, Outgoing, 8> = Channel::new();

/// The bytes in `OUTBOX` and in the notifier's hands.
static OUTBOX_BYTES: QueuedBytes = QueuedBytes::new();

/// Bytes one queue for a host may hold — sent and not yet written out — before the task
/// that fills it waits: back-pressure, nothing dropped. Per queue: `OUTBOX` (BLE) and
/// `USB_OUT` (the cable).
///
/// A queue counts eight items, and an item is a 20-byte text line or an encoded link frame
/// of up to 4.2 KB (`link::MAX_BODY` and its header): eight frames were 33.6 KB. Counted in
/// bytes a queue holds at most 8 KB, the item its writer is putting on the wire included:
/// an item goes past the cap only into an empty queue, and no item is larger than the cap.
/// Worst case per queue beside that: each task waiting to send holds its one encoded item —
/// a frame of 4.2 KB from the session on either carrier — so about 12.2 KB a queue, 24.4 KB
/// for both. Not measured.
const QUEUED_OUT_BYTES: usize = 8 * 1024;

/// A queue's bytes, counted from the send until its writer has put them on the wire
/// ([`QUEUED_OUT_BYTES`]).
struct QueuedBytes {
	/// Bytes counted, and the tasks waiting for room. Two senders to the cable's queue are
	/// two tasks, so not a `Signal`: it holds one waiter, and two would wake each other
	/// without end.
	state: BlockingMutex<CriticalSectionRawMutex, RefCell<(usize, MultiWakerRegistration<4>)>>,
}

impl QueuedBytes {
	const fn new() -> Self {
		QueuedBytes {
			state: BlockingMutex::new(RefCell::new((0, MultiWakerRegistration::new()))),
		}
	}

	/// Send `item`, of `bytes`, once they fit under the cap — an empty queue takes any item.
	/// Cancelled before the queue has it — a session ending while it waits — nothing is
	/// counted.
	async fn send<T, const N: usize>(&self, queue: &Channel<CriticalSectionRawMutex, T, N>, item: T, bytes: usize) {
		core::future::poll_fn(|cx| {
			self.state.lock(|state| {
				let (queued, waiting) = &mut *state.borrow_mut();
				if *queued == 0 || *queued + bytes <= QUEUED_OUT_BYTES {
					*queued += bytes;
					core::task::Poll::Ready(())
				} else {
					waiting.register(cx.waker());
					core::task::Poll::Pending
				}
			})
		})
		.await;
		let mut refund = Refund {
			counted: self,
			bytes,
			armed: true,
		};
		queue.send(item).await;
		refund.armed = false;
	}

	/// `bytes` are out of the writer's hands.
	fn written(&self, bytes: usize) {
		self.state.lock(|state| {
			let (queued, waiting) = &mut *state.borrow_mut();
			*queued = queued.saturating_sub(bytes);
			waiting.wake();
		});
	}

	/// The queue was cleared, and whatever its writer held is gone with it.
	fn clear(&self) {
		self.state.lock(|state| {
			let (queued, waiting) = &mut *state.borrow_mut();
			*queued = 0;
			waiting.wake();
		});
	}
}

/// Gives counted bytes back when a send is dropped before its queue took the item.
struct Refund<'a> {
	counted: &'a QueuedBytes,
	bytes: usize,
	/// Still the sender's: the queue has not taken the item.
	armed: bool,
}

impl Drop for Refund<'_> {
	fn drop(&mut self) {
		if self.armed {
			self.counted.written(self.bytes);
		}
	}
}

enum Outgoing {
	/// One text line, one notification.
	Text(Vec<u8>),
	/// One encoded link frame, cut at the notification size.
	Frame(Vec<u8>),
}

/// `Visibility as u8`. An atomic rather than the mutex because the LED task
/// reads it constantly and must never wait for a flash write.
static VISIBILITY: AtomicU8 = AtomicU8::new(Visibility::Dark as u8);

/// Presses arriving from the panel simulator over USB. They go through the
/// **same** handling as the physical button rather than a parallel path — a
/// test rig that exercises different code from the real thing tests the rig.
static REMOTE_PRESS: Signal<CriticalSectionRawMutex, Press> = Signal::new();

/// The plan's alarms and what the glass showed last: polled by the panel every frame,
/// pressed by the button. The page cursor is not in it — that stays `Config::active_page`,
/// passed in each time. A **blocking** mutex, held for one call and never across an
/// `.await`.
type ScreenCell = BlockingMutex<CriticalSectionRawMutex, RefCell<Screen<'static, ALARM_COUNT>>>;

static SCREEN: StaticCell<ScreenCell> = StaticCell::new();

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

/// The board's clock in milliseconds since boot — what the planner, the guard
/// and every timestamp sent to a host are on.
fn ms() -> u64 {
	Instant::now().as_millis()
}

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) {
	// First: the log is queued for the one USB writer, never printed (`vag_dash_fw::usb`).
	vag_dash_fw::usb::init_logger();

	let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
	esp_alloc::heap_allocator!(size: 72 * 1024);

	let timer0 = SystemTimer::new(peripherals.SYSTIMER);
	esp_hal_embassy::init(timer0.alarm0);

	// Reset reason, last panic, watchdog — armed here, before the radio, so a
	// hang during start-up reboots too. See `health.rs`.
	vag_dash_fw::health::init(spawner);

	let led = Output::new(peripherals.GPIO8, Level::High, OutputConfig::default());
	// GPIO9 is the SuperMini's BOOT button: a real button, already fitted, and
	// the device's only one — it is powered from OBD pin 1 and never sleeps, and
	// in the car the cruise lever pages it (`todo/dash/14` §6a).
	let button = Input::new(peripherals.GPIO9, InputConfig::default().with_pull(Pull::Up));

	// Which car this image is for, said once, before anything is asked of the
	// bus: a plan and a car that disagree is the first thing to look for.
	info!(
		"plan: VIN {}, {} unit(s), {} channel(s), {} page(s), {} alarm(s), labels in {:?}",
		PLAN.vin,
		PLAN.units.len(),
		PLAN.channels.len(),
		PLAN.pages.len(),
		PLAN.alarms.len(),
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
	let bus: &'static Bus = BUS.init(BlockingMutex::new(RefCell::new(Planner::new(Budget::board()))));
	let screen: &'static ScreenCell = SCREEN.init(BlockingMutex::new(RefCell::new(Screen::new(PLAN.alarms))));

	let rng = esp_hal::rng::Rng::new(peripherals.RNG);
	let timer1 = TimerGroup::new(peripherals.TIMG0);
	let wifi_init = esp_wifi::init(timer1.timer0, rng).expect("radio init");

	// The controller stays up for the life of the device. Tearing it down would
	// free ~46 KB and reintroduce the one allocation pattern that can fragment
	// this heap (see .archive/tasks/done/dash/11-ble.md).
	let transport = BleConnector::new(&wifi_init, peripherals.BT);
	let controller: ExternalController<_, 20> = ExternalController::new(transport);

	// One driver owns the USB port: one task writes it (frames, answers, the log),
	// one reads it (the framed link, `dashsim`'s buttons, slcan lines). Nothing
	// else touches the peripheral while they run.
	let (usb_rx, usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE).into_async().split();

	// Never `.ok()` a spawn: the arena is finite and a full one fails silently.
	if let Err(e) = spawner.spawn(usb_writer_task(usb_tx)) {
		warn!("SPAWN usb writer FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(usb_reader_task(usb_rx)) {
		warn!("SPAWN usb reader FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(usb_presence_task()) {
		warn!("SPAWN usb presence FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(usb_session_task(bus)) {
		warn!("SPAWN usb session FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(panel_task(settings, screen)) {
		warn!("SPAWN panel FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(led_task(led)) {
		warn!("SPAWN led FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(button_task(button, settings, screen)) {
		warn!("SPAWN button FAILED: {e:?}");
	}
	if let Err(e) = spawner.spawn(heap_task()) {
		warn!("SPAWN heap FAILED: {e:?}");
	}
	// The bus. GPIO1 reads the transceiver's RXD, GPIO6 drives its TXD — the
	// wiring is `todo/dash/15-enclosure.md` §3. The task builds the controller
	// itself, once per mode: filtered and in normal mode for the planner, as the
	// adapter's host asks for in adapter mode.
	if let Err(e) = spawner.spawn(can_task(peripherals.TWAI0, peripherals.GPIO1, peripherals.GPIO6, bus, settings)) {
		warn!("SPAWN can FAILED: {e:?}");
	}

	run(controller, settings, bus).await;
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
						// Said again as a note, because the `warn!` above is text
						// among the boot log's and a note is one whole line of its
						// own between frames. Both leave by the same USB serial at
						// the same moment, so a laptop attached later sees
						// neither; what outlasts boot is `state`'s `unsaved=1`. A
						// stored configuration from an older `dash.toml` hid the
						// plan's chart page, and the only trace was an early
						// `info!` (2026-09-13).
						let plural = |n: usize| if n == 1 { "page" } else { "pages" };
						match config.plan_mismatch() {
							Some(Mismatch::Count { stored, plan }) => note!(
								"settings: stored config has {stored} {}, this plan has {plan} {} — discarded, showing the plan's",
								plural(stored),
								plural(plan)
							),
							Some(Mismatch::Page { index }) => note!(
								"settings: stored page {} is not the plan's — config discarded, showing the plan's pages",
								index + 1
							),
							None => note!("settings: stored config does not fit this plan ({reason}) — discarded"),
						}
						note!("settings: brightness and active page are back to defaults; `save` stores them and this note goes away");
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
/// A short press goes through the alarms first ([`Screen::press`]): while an
/// alarm owns the glass it silences that episode and the page stays; otherwise
/// the page cursor moves on. The button is modal because the screen already
/// says which mode it is in. `set page` over BLE is not a press and does not
/// come here.
///
/// A long press does nothing any more: it used to open a three-minute BLE
/// window, and BLE is now always on (owner, 2026-09-13/14).
///
/// The simulator's `BTN S` / `BTN L` come in through the same machine as the
/// GPIO level, and go out through the same gate: one press per
/// [`PRESS_GAP_MS`](vag_dash_fw::ui::PRESS_GAP_MS), whoever pressed it.
/// Taking a remote press at face value is what turned one held space bar
/// into a dozen page turns.
#[embassy_executor::task]
async fn button_task(button: Input<'static>, settings: &'static Shared, screen: &'static ScreenCell) -> ! {
	let mut machine = Button::new();
	loop {
		// Half the debounce interval: fast enough that no edge is missed,
		// slow enough to be free.
		let press = match select(Timer::after(Duration::from_millis(DEBOUNCE_MS / 2)), REMOTE_PRESS.wait()).await {
			Either::First(()) => machine.poll(button.is_low(), Instant::now().as_millis()),
			Either::Second(press) => machine.remote(press, Instant::now().as_millis()),
		};
		match press {
			Some(Press::Short) => {
				let mut s = settings.lock().await;
				let before = s.config.active_page;
				// `pages` is bounded by `MAX_PAGES`, so the count fits.
				let pages = s.config.pages.len() as u8;
				match screen.lock(|cell| cell.borrow_mut().press(&mut s.config.active_page, pages)) {
					alarm::Press::Silenced => {
						drop(s);
						note!("button: alarm silenced until its value comes back");
					}
					alarm::Press::NextPage => {
						s.unsaved |= s.config.active_page != before;
						// One-based: this line is read by a person, and "page 0 of 2" reads as
						// no page at all. The `state` line stays zero-based — it is a protocol.
						note!("button: page {} of {}", usize::from(s.config.active_page) + 1, s.config.pages.len());
						drop(s);
						STATE_CHANGED.signal(());
						PAGES_CHANGED.signal(());
					}
				}
			}
			Some(Press::Long) => note!("button: held — nothing to do, BLE is always on"),
			None => {}
		}
	}
}

/// The only thing that says what state the device is in while there is no
/// panel: off is not advertising (it could not start), a hurried blink is
/// advertising, a slow double pulse is connected.
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

async fn run<C: Controller>(controller: C, settings: &'static Shared, bus: &'static Bus) {
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
	// The Device Information service is read over the air, never from Rust, so
	// nothing here reads `dis` and the dead-code lint says so. An `allow` cannot
	// quiet it: trouble-host's `#[gatt_server]` rebuilds the struct and drops
	// every attribute on it and its fields — which is how the one that stood on
	// the field stopped working unnoticed. Naming the field is the read.
	let _ = &server.dis;

	info!("heap after host build:\n{}", esp_alloc::HEAP.stats());

	// The radio's session, one for the board's life: a close frees a connection's
	// subscriptions and queue, never what its guard remembers (`guard` module docs, "Memory").
	let mut session = Session::new();
	let _ = join(ble_task(runner), async {
		// Always visible (owner, 2026-09-13: "мы можем видимость всегда включенной
		// держать … антенна всё равно далеко не бьёт"; 2026-09-14: zero friction).
		// There is no gate to open: the board advertises from boot, serves one
		// central, and advertises again as soon as it leaves. What a stranger in
		// range can do is what the guard allows, and that was accepted with it.
		loop {
			let advertiser = match start_advertising(DEVICE_NAME, &mut peripheral).await {
				Ok(a) => a,
				Err(e) => {
					set_visibility(Visibility::Dark);
					warn!("[adv] could not start: {e:?}");
					Timer::after(Duration::from_secs(1)).await;
					continue;
				}
			};
			set_visibility(Visibility::Advertising);
			info!("[adv] advertising as {DEVICE_NAME}");

			match advertiser.accept().await {
				Ok(conn) => match conn.with_attribute_server(&server) {
					Ok(conn) => {
						set_visibility(Visibility::Connected);
						info!("[adv] connected");
						serve(&server, &conn, settings, bus, &mut session).await;
						info!("[adv] connection over, advertising again");
					}
					Err(e) => warn!("[adv] attribute server: {e:?}"),
				},
				Err(e) => warn!("[adv] accept failed: {e:?}"),
			}
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
			AdStructure::ServiceUuids16(&[service::DEVICE_INFORMATION.to_le_bytes()]),
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

/// One connection, from accept to disconnect.
///
/// Four jobs, and the first one to end — the GATT event loop, on disconnect —
/// ends them all. Then the session is closed: the host's subscriptions leave
/// the planner (drop semantics) and the reassembler goes. The session itself is the
/// board's, `run` keeps one for every connection: its radio guard remembers what the
/// last host asked (`vag_uds_client::guard`, "Memory"), so a reconnect is no reset.
async fn serve<P: PacketPool>(
	server: &Server<'_>,
	conn: &GattConnection<'_, '_, P>,
	settings: &'static Shared,
	bus: &'static Bus,
	session: &mut Session,
) {
	// Nothing from a previous connection is this one's.
	INBOX.clear();
	OUTBOX.clear();
	// The last connection's notifier may have ended with an item in its hands, uncounted out.
	OUTBOX_BYTES.clear();
	BLE_CLIENT.reset();
	info!("[gatt] ATT MTU {} at connect", conn.raw().att_mtu());

	select4(
		gatt_events(server, conn),
		notifier(server, conn),
		state_pushes(settings),
		uds_server(session, settings, bus),
	)
	.await;
	// The host is gone: what it wrote and nobody read yet is not sent to the car, the
	// exchange it has queued is cancelled, and its subscriptions leave the planner.
	INBOX.clear();
	bus.lock(|planner| session.close(ms(), &mut planner.borrow_mut()));
	BLE_CLIENT.publish(session);
	BUS_WAKE.signal(());
}

/// The bytes of one notification: the negotiated ATT MTU less the 3-byte
/// header, never above the characteristic's storage ([`UART_MTU`]) and never
/// below ATT's default payload.
///
/// trouble-host 0.2.4 exposes the MTU as `Connection::att_mtu` and does not
/// clip a notification to it, so this is where it is kept. macOS asks for 251
/// at connect (the figure `UART_MTU`'s note records), which makes this 244.
fn notify_size<P: PacketPool>(conn: &GattConnection<'_, '_, P>) -> usize {
	usize::from(conn.raw().att_mtu()).saturating_sub(3).clamp(ATT_DEFAULT_PAYLOAD, UART_MTU)
}

/// Writes into the UART characteristic, handed on as they came.
async fn gatt_events<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>) {
	let rx = &server.uart.rx;
	let reason = loop {
		match conn.next().await {
			GattConnectionEvent::Disconnected { reason } => break reason,
			GattConnectionEvent::Gatt { event } => {
				if let GattEvent::Write(e) = &event {
					if e.handle() == rx.handle {
						let mut chunk = UartData::new();
						let data = e.data();
						let _ = chunk.extend_from_slice(&data[..data.len().min(UART_MTU)]);
						// Awaited before the write is acknowledged: a host that
						// writes faster than the board reads is held at the ATT
						// layer instead of being queued without end. Raced against
						// the connection, because while this waits no event —
						// the disconnect included — is read, and a session left
						// open serves a host that is gone.
						if let Either::Second(()) = select(INBOX.send(chunk), gone(conn)).await {
							info!("[gatt] disconnected while the session was full");
							return;
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

/// How often a stalled write looks whether its connection is still there.
const GONE_POLL: Duration = Duration::from_millis(50);

/// Resolves once the connection has dropped. Polled, because the event that says
/// so is the one `gatt_events` cannot read while it waits.
async fn gone<P: PacketPool>(conn: &GattConnection<'_, '_, P>) {
	while conn.raw().is_connected() {
		Timer::after(GONE_POLL).await;
	}
}

/// One text line as the link carries it: its bytes and a `\n`, which is how a client
/// knows where a line cut into several notifications ends.
fn text_line(text: &str) -> Vec<u8> {
	let mut line = Vec::with_capacity(text.len() + 1);
	line.extend_from_slice(text.as_bytes());
	line.push(b'\n');
	line
}

/// The one writer of notifications.
async fn notifier<P: PacketPool>(server: &Server<'_>, conn: &GattConnection<'_, '_, P>) {
	let tx = &server.uart.tx;
	let mut said_mtu = 0;
	loop {
		let outgoing = OUTBOX.receive().await;
		let size = notify_size(conn);
		if size != said_mtu {
			said_mtu = size;
			note!("ble: notifications of {size} bytes (ATT MTU {})", conn.raw().att_mtu());
		}
		let send = async |bytes: &[u8]| {
			let mut out = UartData::new();
			let _ = out.extend_from_slice(bytes);
			tx.notify(conn, &out).await
		};
		// Both cut at the notification size: a frame's reassembler and a text line's
		// `\n` say where each ends. The first state push goes out before the MTU
		// exchange, at 20 bytes a notification, and must still arrive whole.
		let (Outgoing::Text(bytes) | Outgoing::Frame(bytes)) = outgoing;
		for piece in link::chunks(&bytes, size) {
			if let Err(e) = send(piece).await {
				// A chunk lost is a hole in the host's stream with nothing to say so: its
				// reassembler takes the next message's bytes for this one's rest, and a text
				// line runs into the next. A notification is not sent again, so the
				// connection ends — the host reconnects and says Hello — and the disconnect
				// ends `gatt_events`, and with it this session.
				note!("ble: a notification failed ({e:?}) — ending the connection, the host's stream has a hole");
				conn.raw().disconnect();
				core::future::pending::<()>().await;
			}
		}
		OUTBOX_BYTES.written(bytes.len());
	}
}

/// Pushes the state line: once on connecting, and again whenever the button
/// changes something. A client that has to poll to notice a button press is a
/// client that shows the wrong thing most of the time.
async fn state_pushes(settings: &Shared) {
	loop {
		let line = text_line(&state_line(settings).await);
		let bytes = line.len();
		OUTBOX_BYTES.send(&OUTBOX, Outgoing::Text(line), bytes).await;
		STATE_CHANGED.wait().await;
	}
}

/// Requests in flight in the session past which the board stops reading the
/// link, so the host waits at the ATT layer. A host that awaits each answer
/// never has more than one.
const SESSION_QUEUE_MAX: usize = 4;

/// PDU bytes one carrier's host may have waiting on the board — in its session, and for
/// the cable in `USB_MESSAGES` too — past which the board stops reading that carrier.
/// Back-pressure: nothing is dropped, the host waits.
///
/// Worst case for one carrier's host, all the queues it can pin at once on the 72 KB heap
/// (not measured: the bench was down when this was written):
///
/// - **inbound**, sending nothing but 4095-byte requests: under this cap (4 KB) when a read
///   is allowed, plus the request that read completes (4 KB), the reassembler's frame in
///   progress (4.2 KB), and the one exchange the planner holds (4 KB) — about 16.5 KB;
/// - **outbound** ([`QUEUED_OUT_BYTES`], 8 KB) plus the one encoded item the notifier/writer
///   holds (4.2 KB) — about 12 KB;
/// - **answers** ([`Client::answers`]), one raw answer of up to 4.1 KB;
/// - **readings** ([`Client::readings`]), 16 records each capped at `MAX_READING_BYTES`
///   (`Session::deliver` ends a bigger one, S-F4) — about 8 KB.
///
/// About 41 KB for one carrier, ~57 KB for both at once (only one holds a full raw answer at
/// a time), beside the radio stack's share. The readings and answers queues are the S-F4
/// additions; before them this counted only the inbound side.
const QUEUED_PDU_BYTES: usize = 4096;

/// The PDU bytes a message brings.
fn pdu_bytes(message: &Message) -> usize {
	match message {
		Message::Request(request) => request.pdu.len(),
		_ => 0,
	}
}

/// When a session with nothing to wake for wakes anyway: an hour away, not
/// `Instant::MAX` — an alarm that far out is a question for the time driver a session
/// loop does not need to ask.
fn session_wake(session: &Session) -> Instant {
	session.wake_at().map_or(Instant::now() + Duration::from_secs(3600), Instant::from_millis)
}

/// The link's two protocols over BLE: `dashcfg`'s text commands, answered as before,
/// and framed UDS messages, through the session.
async fn uds_server(session: &mut Session, settings: &'static Shared, bus: &'static Bus) {
	let client = &BLE_CLIENT;
	let mut reassembler = Reassembler::new();
	let mut dropped_said = 0;
	loop {
		let wake = session_wake(session);
		let full = session.queued() >= SESSION_QUEUE_MAX || session.queued_bytes() >= QUEUED_PDU_BYTES;
		let inbox = async {
			if full {
				core::future::pending::<()>().await;
			}
			INBOX.receive().await
		};
		let event = select(
			select4(inbox, client.answers.receive(), client.readings.receive(), Timer::at(wake)),
			MODE_FOR_BLE.wait(),
		)
		.await;
		let out = match event {
			Either::First(Either4::First(chunk)) => {
				let mut out = Vec::new();
				for piece in reassembler.push(&chunk) {
					match piece {
						Piece::Text(bytes) => {
							let line = text_line(&command(settings, &bytes).await);
							let bytes = line.len();
							OUTBOX_BYTES.send(&OUTBOX, Outgoing::Text(line), bytes).await;
						}
						Piece::Message(message) => out.extend(take_message(session, bus, message)),
						Piece::Error(e) => note!("ble: a malformed frame from the host was dropped: {e}"),
					}
				}
				out
			}
			Either::First(Either4::Second((req, answer, at))) => bus.lock(|p| session.answered(at, &mut p.borrow_mut(), req, &answer)),
			Either::First(Either4::Third(delivery)) => bus.lock(|p| session.deliver(ms(), &mut p.borrow_mut(), &delivery)).into_iter().collect(),
			Either::First(Either4::Fourth(())) => bus.lock(|p| session.poll(ms(), &mut p.borrow_mut())),
			Either::Second(()) => mode_changed(session, bus),
		};
		// Whatever the session did may have given the planner work, and changed which
		// subscriptions are its.
		client.publish(session);
		BUS_WAKE.signal(());
		for message in out {
			match link::encode(&message) {
				Ok(frame) => {
					let bytes = frame.len();
					OUTBOX_BYTES.send(&OUTBOX, Outgoing::Frame(frame), bytes).await;
				}
				Err(e) => note!("ble: an answer for the host did not encode: {e}"),
			}
		}
		client.say_dropped("ble", &mut dropped_said);
	}
}

/// One framed message from a host, on either carrier.
///
/// A Hello starts the session over — whatever an earlier host left on this carrier is
/// closed, except what the radio guard remembers — and is answered with what this image
/// is. In adapter mode everything that would reach the bus is refused, and says why.
fn take_message(session: &mut Session, bus: &Bus, message: Message) -> Vec<Message> {
	bus.lock(|p| {
		let mut planner = p.borrow_mut();
		match message {
			Message::Hello => {
				session.close(ms(), &mut planner);
				alloc::vec![hello_reply()]
			}
			message if adapter_mode() => session.push_refused(ms(), &mut planner, message, ADAPTER_MODE),
			message => session.push(ms(), &mut planner, message),
		}
	})
}

/// The board changed mode: entering adapter mode refuses what the session's host waits
/// for. Its subscriptions stay, and resume with the bus.
fn mode_changed(session: &mut Session, bus: &Bus) -> Vec<Message> {
	match adapter_mode() {
		true => bus.lock(|p| session.refuse_pending(&mut p.borrow_mut(), ADAPTER_MODE)),
		false => Vec::new(),
	}
}

/// What this image is, for a Hello.
fn hello_reply() -> Message {
	Message::HelloReply(HelloReply {
		image: "dash".into(),
		version: env!("CARGO_PKG_VERSION").into(),
	})
}

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
	// Which job the board is doing: `dashcfg` ignores keys it does not know, and over
	// BLE this is how a person sees that the cable made the board an adapter.
	let _ = write!(out, " mode={}", if adapter_mode() { "adapter" } else { "panel" });
	if let Some(page) = s.config.pages.get(usize::from(s.config.active_page)) {
		let kind = match page.kind {
			PageKind::Chart => "chart",
			PageKind::Values => "values",
		};
		let _ = write!(out, " kind={kind} cells={:?}", page.cells);
	}
	out
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
	/// How long this value may be shown: [`STALE`], or three periods of a
	/// channel read slower than that.
	fresh_for: Duration,
}

impl Slot {
	const EMPTY: Slot = Slot {
		value: None,
		at: None,
		fresh_for: STALE,
	};

	/// The value, unless it is older than it may be.
	fn current(&self, now: Instant) -> Option<f32> {
		let at = self.at?;
		if now.saturating_duration_since(at) > self.fresh_for {
			return None;
		}
		self.value
	}
}

/// How old a value may be and still be shown. A shown channel is read several
/// times a second and a hidden one once; a value nobody has refreshed for five
/// is a bus that went quiet, and the panel should say so rather than hold the
/// last number.
const STALE: Duration = Duration::from_secs(5);

/// One slot per plan channel — the whole of what the bus task tells the panel
/// task. Sized by the plan, so a channel the plan does not have has nowhere to
/// be stored, which is the same property `config.rs` has for cells.
static VALUES: Mutex<CriticalSectionRawMutex, [Slot; CHANNEL_COUNT]> = Mutex::new([Slot::EMPTY; CHANNEL_COUNT]);

/// How long one answer PDU may take to arrive, all its frames together. A unit
/// that is there answers a read in milliseconds; this is also what a silent
/// unit costs the bus per attempt, so it stays short. 500 ms rather than the
/// 300 the round-robin used: a host's request may be a fault list of many
/// frames from a unit behind the gateway.
const RESPONSE_TIMEOUT: core::time::Duration = core::time::Duration::from_millis(500);
/// How long a request that suppressed its positive response (`3E 80`, `10 81`) waits
/// for the refusal that is its only possible answer. ISO 14229-2's default
/// P2server_max is 50 ms; three times that leaves room for a unit behind the gateway.
const SUPPRESSED_WAIT: core::time::Duration = core::time::Duration::from_millis(150);
/// After `7F xx 78` (response pending), how long to wait for the next answer:
/// ISO 14229-2's default P2*server_max, 5000 ms.
const PENDING_WAIT: Duration = Duration::from_secs(5);
/// The most a run of `7F xx 78` may hold the bus, all of them together. Past it
/// the exchange is no answer and the next one goes out. Twice P2*: one unit's
/// slow operation, not an open-ended one — the panel waits while it lasts.
const PENDING_DEADLINE: Duration = Duration::from_secs(10);
/// The transmit side of an exchange. The transport's receive has its own
/// deadline; its send has none, and what every state of the controller does to
/// a transmit future is not this loop's to find out — so it gets one, and past
/// it the request is dropped (which aborts the transmission).
const SEND_DEADLINE: Duration = Duration::from_secs(1);
/// After a bus-off, before the restarted controller is asked anything.
const BUS_OFF_GAP: Duration = Duration::from_secs(1);
/// How often the bus task looks at the pages when nothing has said they changed.
const PAGE_RECHECK: Duration = Duration::from_secs(1);
/// ISO 14229-1: a negative response, and the NRC that asks for more time.
const NEGATIVE: u8 = 0x7F;
const RESPONSE_PENDING: u8 = 0x78;

/// What the part-number check has established about one unit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Check {
	/// Asked, not answered yet.
	Pending,
	/// Asked, no answer; said so once, and asked again as the planner's backoff
	/// allows — and a unit that answered once and then fell silent comes back
	/// here, so that a bus which has gone quiet is asked one thing per unit,
	/// not everything.
	Absent,
	/// Answered with the part number the plan was built against: its channels
	/// are subscribed.
	Matched,
	/// Answered with a different one. Never polled again this run: the plan's
	/// identifiers would answer, plausibly, about a unit they were not
	/// resolved for.
	Mismatch,
}

/// The acceptance filter for the plan's own answer ids; `None` when the plan
/// polls nobody, since a filter matching nothing would be the same silence with
/// a harder-to-find cause. See [`vag_uds_can::filter`] for why a filter at all.
fn plan_filter() -> Option<StandardFilter> {
	StandardFilter::covering(PLAN.units.iter().map(|unit| unit.response))
}

/// The same filter in esp-hal's type: one ESP32 "single standard" filter, an
/// 11-bit code and, in this API's direction, a mask whose set bits are the ones
/// that must match. Data frames only: nothing in UDS is remote-transmission.
fn twai_filter(filter: StandardFilter) -> Option<SingleStandardFilter> {
	Some(SingleStandardFilter::new_from_code_mask(
		StandardId::new(filter.code)?,
		StandardId::new(filter.must_match)?,
		// RTR clear, and that much is insisted on.
		false,
		true,
		// Any payload: the first two bytes are an ISO-TP header, not a key.
		[0, 0],
		[0, 0],
	))
}

/// Moves the controller's filter, and says so the first time with what it cost.
///
/// esp-hal `=1.0.0-rc.0` sets a filter only in reset mode: `stop()` enters it,
/// `set_filter` stores the value, `start()` writes it to the acceptance
/// registers and leaves reset (clearing the error counters, as the bus-off
/// restart relies on). That is a handful of register writes — the measured
/// cost is the note — plus, on the wire, the 11 recessive bits a controller
/// leaving reset waits for before it takes part: at most one frame's length on
/// a busy bus, well under a millisecond. It happens only between exchanges,
/// after nothing is in flight and before the stale-frame sweep, so no answer
/// can be lost to it. The async driver's receive queue is software and survives
/// the restart; the sweep empties it.
fn refilter(backend: TwaiBackend<'static>, filter: StandardFilter, said: &mut bool) -> TwaiBackend<'static> {
	let Some(twai_filter) = twai_filter(filter) else {
		return backend;
	};
	let started = Instant::now();
	let backend = backend.refilter(twai_filter);
	let cost = started.elapsed();
	if !*said {
		*said = true;
		note!(
			"can: the filter follows the exchange — moved to {:03X}/{:03X} in {} µs (said once)",
			filter.code,
			filter.must_match,
			cost.as_micros()
		);
	}
	backend
}

/// One exchange: sweep, address, send, wait out `7F xx 78`, unwrap.
///
/// The wrappers are stateless, so building them per exchange costs nothing,
/// and the sweep is what it buys: whatever the controller queued *since the
/// last answer* — a reply that came in past its deadline, a frame from
/// another tester on the same response id — is thrown away here rather than
/// taken as this request's answer, which is how every read of a unit ends up
/// one behind until a request gets nothing back.
///
/// The backend is lent, not given: an exchange given up for adapter mode leaves the
/// controller with its caller, which quiesces it before dropping it.
async fn exchange(backend: &mut TwaiBackend<'static>, unit: Unit, pdu: &[u8]) -> (Result<Vec<u8>, TransportError>, usize) {
	let swept = backend.drain().await;
	let mut link = IsoTpCan::new(backend, CanId::Standard(unit.request), CanId::Standard(unit.response));
	let result = transact(&mut link, pdu).await;
	(result, swept)
}

/// One unit's ISO-TP over the lent controller.
type Link<'a> = IsoTpCan<&'a mut TwaiBackend<'static>>;

async fn transact(link: &mut Link<'_>, pdu: &[u8]) -> Result<Vec<u8>, TransportError> {
	match with_timeout(SEND_DEADLINE, link.send(pdu)).await {
		Ok(sent) => sent?,
		Err(_elapsed) => return Err(TransportError::Timeout),
	}
	let sid = pdu.first().copied().unwrap_or(0);
	// A request that suppressed its positive response is answered only by a refusal,
	// and a refusal comes within P2: waiting the full deadline for silence would hold
	// the bus for nothing. `answer_of` turns the timeout into `NotExpected`.
	let first = if expects_no_answer(pdu) { SUPPRESSED_WAIT } else { RESPONSE_TIMEOUT };
	let mut answer = receive(link, first).await?;
	// The planner takes a `78` that reaches it as a refusal; the shell's job is
	// that one does not (`schedule` module docs).
	let pending_since = Instant::now();
	while matches!(answer.as_slice(), [NEGATIVE, s, RESPONSE_PENDING, ..] if *s == sid) {
		let left = PENDING_DEADLINE.checked_sub(pending_since.elapsed()).unwrap_or(Duration::MIN);
		if left == Duration::MIN {
			return Err(TransportError::Timeout);
		}
		let wait = if left < PENDING_WAIT { left } else { PENDING_WAIT };
		answer = receive(link, core::time::Duration::from_millis(wait.as_millis())).await?;
	}
	Ok(answer)
}

/// One answer PDU within `timeout`, with a backstop over the transport's own
/// deadline for the flow-control frame it may transmit.
async fn receive(link: &mut Link<'_>, timeout: core::time::Duration) -> Result<Vec<u8>, TransportError> {
	let backstop = Duration::from_millis(timeout.as_millis() as u64) + SEND_DEADLINE;
	with_timeout(backstop, link.recv(timeout)).await.unwrap_or(Err(TransportError::Timeout))
}

/// What the planner is told about an exchange of `request`. Silence after a request
/// that asked for it is [`Answer::NotExpected`], not an absent unit: the panel keeps
/// reading that unit and nothing is backed off.
fn answer_of(request: &[u8], result: Result<Vec<u8>, TransportError>) -> Answer {
	match result {
		Ok(pdu) => Answer::Pdu(pdu),
		Err(TransportError::Timeout) if expects_no_answer(request) => Answer::NotExpected,
		Err(TransportError::Timeout) => Answer::NoAnswer,
		Err(_) => Answer::BusError,
	}
}

/// The controller after an exchange: restarted if it went bus-off.
///
/// Bus-off is sticky. Once the controller has counted its way there, esp-hal
/// answers every transmit and receive with `BusOff` until it is put through
/// reset mode again — so without this, one bad moment on the bus would be
/// dashes until somebody pulled the plug. `stop()` hands back the
/// configuration, mode and all; `start()` clears the error counters and
/// leaves reset; both keep the async driver and the filter's registers.
async fn settle(backend: TwaiBackend<'static>, result: &Result<Vec<u8>, TransportError>, bus_off: &mut bool) -> TwaiBackend<'static> {
	if let Err(TransportError::Disconnected) = result {
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
	backend
}

/// The bus: one planner, one exchange at a time, and the panel as its first
/// consumer.
///
/// **One conversation at a time, re-addressed per exchange** — the same shape
/// as the laptop's `vag_cli_core::bus`: there is one CAN controller, one ISO-TP
/// state, and a unit is a `(request, response)` pair the transport is built
/// around. The planner says what goes out next — the visible page's channels
/// at their rates, hidden pages at 1 Hz, a BLE host's requests between them —
/// and when; this task puts it on the pair, waits for the answer, stamps its
/// arrival, and routes what the planner makes of it.
///
/// **The pins are this task's in both modes.** In panel mode it builds the planner's
/// controller ([`open_panel_bus`]) and runs [`panel_bus`] until the console asks for
/// adapter mode; then it gives the same pins to [`vag_dash_fw::slcan`] until that
/// session ends, and builds the planner's controller again. The panel's subscriptions
/// and part checks outlive both: the planner keeps them, and they resume.
#[embassy_executor::task]
async fn can_task(twai0: TWAI0<'static>, rx_pin: GPIO1<'static>, tx_pin: GPIO6<'static>, bus: &'static Bus, settings: &'static Shared) -> ! {
	let mut panel = PanelReads::new();
	panel.start(bus);
	loop {
		// SAFETY: `clone_unchecked` asks that a clone and its original never both drive
		// the peripheral. The originals stay in this task and are used for nothing but
		// making clones, and one set of clones at a time is alive: `panel_bus` owns its
		// controller until it returns and drops it, and `slcan::serve` returns with the
		// adapter's controller closed, the adapter dropped at the end of its block.
		// (`vag_dash_fw::slcan::Adapter` keeps the same invariant for its own clones.)
		let (twai, rx, tx) = unsafe { (twai0.clone_unchecked(), rx_pin.clone_unchecked(), tx_pin.clone_unchecked()) };
		if adapter_wanted() {
			SLCAN_PORT.reset_counts();
			let mut adapter = Adapter::new(&SLCAN_PORT, twai, rx, tx);
			slcan::serve(&mut adapter, &mut QueuedCommands).await;
			// That session took its `Leave`; the next one has not ended.
			ADAPTER_ENDING.store(false, Ordering::Relaxed);
			// Queued after the adapter's last reply is in its ring, and the writer takes the
			// ring before log lines when both are ready, so the host sees the `\r` first.
			// `load` then `store`, not `swap`: riscv32imc has no atomic compare-and-swap. One
			// executor on one core, and nothing sets it between the two.
			if ADAPTER_LEFT_BY_C.load(Ordering::Relaxed) {
				ADAPTER_LEFT_BY_C.store(false, Ordering::Relaxed);
				note!("usb: adapter mode is over — the panel reads the car again");
			}
			continue;
		}
		panel_bus(open_panel_bus(twai, rx, tx), &mut panel, bus, settings).await;
	}
}

/// The planner's controller.
///
/// **Normal mode, not the listen-only default `can.rs` argues for**, and the reason is
/// the whole job: a `0x22` request has to be transmitted, and a controller that cannot
/// acknowledge cannot be answered either. What goes out is what the planner sends: the
/// plan's reads, and a host's requests after the board's guard — all inside the same
/// read-only allowlist.
///
/// The controller is told which ids to hand up **before** it is started, because a
/// car's powertrain bus is not quiet: the engine alone broadcasts thousands of frames a
/// second, the driver's receive queue is 32 deep, and a full queue drops what arrives
/// next. Without a filter that "next" is the answer to the request we just sent, and
/// the first run on the car said so — every exchange opened by sweeping the queue's
/// full 32 entries. The filter starts as the plan's own answer ids and follows each
/// exchange from there ([`panel_bus`]), so nothing about any car reaches the source.
fn open_panel_bus(twai0: TWAI0<'static>, rx_pin: GPIO1<'static>, tx_pin: GPIO6<'static>) -> TwaiBackend<'static> {
	let mut twai = TwaiConfiguration::new(twai0, rx_pin, tx_pin, BaudRate::B500K, TwaiMode::Normal);
	if let Some(filter) = plan_filter().and_then(twai_filter) {
		twai.set_filter(filter);
	}
	TwaiBackend::new(twai.into_async().start())
}

/// The bus in panel mode, until the board has to become an adapter. Returns with the
/// controller dropped.
async fn panel_bus(mut backend: TwaiBackend<'static>, panel: &mut PanelReads, bus: &'static Bus, settings: &'static Shared) {
	// The controller was just built with the plan's filter.
	let mut filter = FilterFollower::new(plan_filter());
	let mut filter_said = false;
	let mut swept_said = false;
	let mut bus_off = false;
	let mut pages_seen = Instant::MIN;

	loop {
		if adapter_wanted() {
			return;
		}
		if PAGES_CHANGED.signaled() || pages_seen.elapsed() >= PAGE_RECHECK {
			PAGES_CHANGED.reset();
			pages_seen = Instant::now();
			panel.follow_pages(bus, settings).await;
		}
		panel.ask_again_when_due(bus);

		let next = bus.lock(|p| p.borrow_mut().due(ms()));
		match next {
			Next::Send(out) => {
				if let Some(moved) = filter.before(out.unit.response) {
					backend = refilter(backend, moved, &mut filter_said);
				}
				// The mode is polled first, every time the two are woken: once the board is an
				// adapter the exchange is not polled again, so no frame of it — a flow control
				// half way through an answer — starts after that.
				let exchanged = select(adapter_requested(), exchange(&mut backend, out.unit, &out.pdu)).await;
				let (result, swept) = match exchanged {
					Either::Second(done) => done,
					Either::First(()) => {
						// The exchange is given up. A frame of it may be on the wire: the
						// controller is quiesced before it is dropped, so it is not reset in
						// the middle of one. The planner still has to hear of the exchange,
						// or it would wait for this answer forever.
						backend.quiesce().await;
						let deliveries = bus.lock(|p| p.borrow_mut().answered(ms(), out.token, Answer::BusError));
						deliver(deliveries, panel, bus).await;
						return;
					}
				};
				let at = ms();
				if swept > 0 && !swept_said {
					swept_said = true;
					note!(
						"can: swept {swept} stale frame(s) before {:03X} {:02X?} — a late reply, or another tester",
						out.unit.request,
						&out.pdu[..out.pdu.len().min(3)]
					);
				}
				backend = settle(backend, &result, &mut bus_off).await;
				let deliveries = bus.lock(|p| p.borrow_mut().answered(at, out.token, answer_of(&out.pdu, result)));
				deliver(deliveries, panel, bus).await;
			}
			Next::Idle { until_ms } => {
				let recheck = pages_seen + PAGE_RECHECK;
				let recheck = panel.next_retry().map_or(recheck, |retry| retry.min(recheck));
				let until = until_ms.map_or(recheck, |t| Instant::from_millis(t).min(recheck));
				let woke = select4(Timer::at(until), BUS_WAKE.wait(), PAGES_CHANGED.wait(), adapter_requested()).await;
				if let Either4::Third(()) = woke {
					// `wait` consumed the signal; the top of the loop looks for it.
					PAGES_CHANGED.signal(());
				}
			}
		}
	}
}

/// Each delivery to the panel if it is the panel's, and to a host's session otherwise.
async fn deliver(deliveries: impl IntoIterator<Item = Delivery>, panel: &mut PanelReads, bus: &Bus) {
	for delivery in deliveries {
		if let Some(delivery) = panel.take(delivery, bus).await {
			remote(delivery);
		}
	}
}

/// How many lines the adapter's ring holds in this image: an eighth of a second of a
/// saturated bus, 512 lines, 18 KB. The standalone `slcan` image holds 2,048; this one
/// keeps its radio, its heap and its panel beside the ring, and a host that stalls for
/// longer is told by `F` bit 3, as it would be there.
const ADAPTER_RING: usize = 512;

/// The adapter's output, waiting for `usb_writer_task`, and its counters for the panel.
static SLCAN_PORT: Port<ADAPTER_RING> = Port::new();

/// What the console hands the adapter.
enum SlcanIn {
	/// One slcan command line.
	Line(CommandLine),
	/// Adapter mode is over: close the channel and give the pins back.
	Leave,
}

/// The console's slcan lines, in order. A full one holds the USB reader back, which
/// holds the host back.
static SLCAN_IN: Channel<CriticalSectionRawMutex, SlcanIn, 16> = Channel::new();

/// The adapter's commands in this image: the console's queue, ending at `Leave`.
struct QueuedCommands;

/// Set, and raised, when the console queues `Leave`: the session is ending. Cleared once
/// the session has taken its `Leave`.
static ADAPTER_ENDING: AtomicBool = AtomicBool::new(false);
static ADAPTER_ENDING_RAISED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

impl Commands for QueuedCommands {
	/// A channel's `receive` is cancel-safe, so this is too.
	async fn next(&mut self) -> Option<CommandLine> {
		match SLCAN_IN.receive().await {
			SlcanIn::Line(line) => Some(line),
			SlcanIn::Leave => None,
		}
	}

	async fn ending(&self) {
		while !ADAPTER_ENDING.load(Ordering::Relaxed) {
			ADAPTER_ENDING_RAISED.wait().await;
		}
	}
}

/// Whether an adapter session has to run: the board is in adapter mode, or lines of one
/// are already queued. The second matters when the console entered and left adapter
/// mode before this task noticed: those lines, and the `Leave` behind them, still
/// belong to an adapter session and are run through one, rather than left for the next.
fn adapter_wanted() -> bool {
	adapter_mode() || !SLCAN_IN.is_empty()
}

/// Resolves once an adapter session has to run.
async fn adapter_requested() {
	while !adapter_wanted() {
		MODE_FOR_BUS.wait().await;
	}
}

/// A delivery that is not the panel's is a host's — the session that owns it, over BLE
/// or the cable. A reading for a subscription nobody owns any more (a unit the panel
/// just dropped, or a host that left) is nobody's: dropped here, and not counted as a
/// drop; so is the answer to an exchange whose session closed after it went out.
fn remote(delivery: Delivery) {
	let clients = [&BLE_CLIENT, &USB_CLIENT];
	match delivery {
		Delivery::Raw { req, answer, at_ms, .. } => {
			let Some(client) = clients.into_iter().find(|client| client.awaits(req)) else {
				return;
			};
			if client.answers.try_send((req, answer, at_ms)).is_err() {
				note!("link: an answer found its queue full and was lost");
			}
		}
		Delivery::Reading { sub, .. } | Delivery::Missed { sub, .. } => {
			let Some(client) = clients.into_iter().find(|client| client.owns(sub)) else {
				return;
			};
			if client.readings.try_send(delivery).is_err() {
				// No compare-and-swap on this core (riscv32imc), and one writer: this task.
				client
					.dropped
					.store(client.dropped.load(Ordering::Relaxed).wrapping_add(1), Ordering::Relaxed);
			}
		}
		// The panel is the only one that reads once.
		Delivery::Once { .. } => {}
	}
}

/// One panel subscription.
#[derive(Clone, Copy, PartialEq, Eq)]
struct PanelSub {
	id: SubId,
	shown: bool,
	period_ms: u32,
}

/// The panel as a consumer of the planner: the part-number check per unit, a
/// subscription per channel worth reading, and the notes about both.
///
/// Before a unit's channels are subscribed its part number is read and compared
/// to the plan's (`05`, "the car check"). This image is built for one car; on
/// another the same identifiers answer and the answers mean something else. A
/// mismatch is said once and the unit is left alone for the rest of the run. No
/// answer is said once and retried — ignition off looks like that — as often as
/// the planner's backoff allows, which doubles to 2 s a unit.
///
/// Every failure lands in [`VALUES`] as `None` before anything else happens, so
/// the panel never shows a number the bus has stopped confirming; what is
/// *said* about a failure is said on the change, because the USB line is shared
/// with the frame stream and a note per reading would be noise.
struct PanelReads {
	checks: [Check; UNIT_COUNT],
	/// The part-number read out for each unit, if one is.
	part_reads: [Option<ReqId>; UNIT_COUNT],
	subs: [Option<PanelSub>; CHANNEL_COUNT],
	/// Per channel: whether the last reading decoded, or `None` before the first.
	answering: [Option<bool>; CHANNEL_COUNT],
	/// What the pages ask for now: the page on the glass, and every page.
	shown: Vec<u16>,
	listed: Vec<u16>,
	dead_bus: bool,
	/// A unit whose part-number answer did not parse is asked again no sooner than
	/// this: the answer reset the planner's backoff, so the pace is kept here.
	part_retry_at: [Option<Instant>; UNIT_COUNT],
}

impl PanelReads {
	fn new() -> Self {
		PanelReads {
			checks: [Check::Pending; UNIT_COUNT],
			part_reads: [None; UNIT_COUNT],
			subs: [None; CHANNEL_COUNT],
			answering: [None; CHANNEL_COUNT],
			shown: Vec::new(),
			listed: Vec::new(),
			dead_bus: false,
			part_retry_at: [None; UNIT_COUNT],
		}
	}

	/// Ask again every unit whose garbled part number has waited long enough.
	fn ask_again_when_due(&mut self, bus: &Bus) {
		let now = Instant::now();
		for u in 0..PLAN.units.len() {
			if self.part_retry_at[u].is_some_and(|at| at <= now) {
				self.part_retry_at[u] = None;
				self.ask_part_number(u, bus);
			}
		}
	}

	/// When the next such retry is due, for the bus task's sleep.
	fn next_retry(&self) -> Option<Instant> {
		self.part_retry_at.iter().flatten().min().copied()
	}

	/// Ask every unit its part number.
	fn start(&mut self, bus: &Bus) {
		for u in 0..PLAN.units.len() {
			self.ask_part_number(u, bus);
		}
	}

	fn ask_part_number(&mut self, u: usize, bus: &Bus) {
		let unit = unit_of(u);
		self.part_reads[u] = Some(bus.lock(|p| p.borrow_mut().read_once(ms(), Class::Foreground, unit, did::PART_NUMBER)));
	}

	/// Re-read the pages and move every subscription that no longer matches them.
	async fn follow_pages(&mut self, bus: &Bus, settings: &Shared) {
		let (shown, listed) = {
			let s = settings.lock().await;
			// The page on the glass, which during a takeover is the alarm's and not the
			// cursor's: its cells are the ones being looked at (`Glass::page_changed`).
			let glass = match GLASS_PAGE.load(Ordering::Relaxed) {
				NO_PAGE => s.config.active_page,
				page => page,
			};
			let shown: Vec<u16> = match s.config.pages.get(usize::from(glass)) {
				Some(page) if page.kind == PageKind::Chart => page.cells.first().copied().into_iter().collect(),
				Some(page) => page.cells.iter().copied().collect(),
				None => Vec::new(),
			};
			// Every page's cells, charts included: a chart samples its channel
			// every frame whether or not it is shown.
			let listed: Vec<u16> = s.config.pages.iter().flat_map(|page| page.cells.iter().copied()).collect();
			(shown, listed)
		};
		if shown == self.shown && listed == self.listed {
			return;
		}
		self.shown = shown;
		self.listed = listed;
		for u in 0..PLAN.units.len() {
			if self.checks[u] == Check::Matched {
				self.subscribe_unit(u, bus);
			}
		}
	}

	/// Make one unit's subscriptions what the pages ask for: subscribe what is
	/// missing, move what changed class or rate, drop what is on no page.
	fn subscribe_unit(&mut self, u: usize, bus: &Bus) {
		let unit = unit_of(u);
		let request = PLAN.units[u].request;
		let mut wanted: [Option<(bool, u32)>; CHANNEL_COUNT] = [None; CHANNEL_COUNT];
		for rate in PLAN.rates(&self.shown, &self.listed) {
			wanted[usize::from(rate.channel)] = Some((rate.foreground, rate.period_ms));
		}
		let now = ms();
		bus.lock(|p| {
			let mut planner = p.borrow_mut();
			for (index, channel) in PLAN.channels.iter().enumerate() {
				if channel.unit != request {
					continue;
				}
				let current = self.subs[index];
				match (current, wanted[index]) {
					(Some(sub), Some((shown, period_ms))) if sub.shown == shown && sub.period_ms == period_ms => {}
					(current, wanted) => {
						if let Some(sub) = current {
							planner.unsubscribe(sub.id);
						}
						self.subs[index] = wanted.map(|(shown, period_ms)| {
							let class = if shown { Class::Foreground } else { Class::Background };
							PanelSub {
								id: planner.subscribe(now, class, unit, channel.did, period_ms, None),
								shown,
								period_ms,
							}
						});
					}
				}
			}
		});
	}

	/// Drop every subscription of one unit and clear its values.
	async fn unsubscribe_unit(&mut self, u: usize, bus: &Bus) {
		let request = PLAN.units[u].request;
		for (index, channel) in PLAN.channels.iter().enumerate() {
			if channel.unit == request {
				if let Some(sub) = self.subs[index].take() {
					bus.lock(|p| p.borrow_mut().unsubscribe(sub.id));
				}
				store(index, None, STALE).await;
			}
		}
	}

	/// Take a delivery if it is the panel's; hand it back otherwise.
	async fn take(&mut self, delivery: Delivery, bus: &Bus) -> Option<Delivery> {
		match &delivery {
			Delivery::Once { req, result, .. } => {
				let Some(u) = self.part_reads.iter().position(|r| *r == Some(*req)) else {
					return Some(delivery);
				};
				self.part_reads[u] = None;
				let previous = self.checks[u];
				match judge(u, result.as_deref(), previous) {
					PartCheck::Matched => {
						self.checks[u] = Check::Matched;
						self.subscribe_unit(u, bus);
					}
					PartCheck::Mismatch => self.checks[u] = Check::Mismatch,
					PartCheck::Absent => {
						self.checks[u] = Check::Absent;
						self.ask_part_number(u, bus);
					}
					PartCheck::RetryLater => {
						self.checks[u] = Check::Absent;
						let cap = Duration::from_millis(u64::from(Budget::default().backoff_cap_ms));
						self.part_retry_at[u] = Some(Instant::now() + cap);
					}
				}
				self.say_dead_bus();
				None
			}
			Delivery::Reading { sub, data, .. } => {
				let Some(index) = self.channel_of(*sub) else {
					return Some(delivery);
				};
				let channel = &PLAN.channels[index];
				let value = channel.decode(data);
				store(index, value, self.fresh_for(index)).await;
				if self.answering[index] != Some(value.is_some()) {
					self.answering[index] = Some(value.is_some());
					match value {
						None => note!(
							"can: {:03X} {:04X} answered {} byte(s), the plan wants bits {}+{}",
							channel.unit,
							channel.did,
							data.len(),
							channel.bit_offset,
							channel.bit_length
						),
						Some(_) => note!("can: {:03X} {:04X} {} is answering", channel.unit, channel.did, channel.label),
					}
				}
				None
			}
			Delivery::Missed { sub, why, .. } => {
				let Some(index) = self.channel_of(*sub) else {
					return Some(delivery);
				};
				let channel = &PLAN.channels[index];
				store(index, None, STALE).await;
				if self.answering[index] != Some(false) {
					self.answering[index] = Some(false);
					note!("can: {:03X} {:04X} {}: {}", channel.unit, channel.did, channel.label, miss_text(*why));
				}
				// A unit that stops answering is asked one thing — its part
				// number — until it speaks, not every channel.
				if *why == Miss::NoAnswer {
					if let Some(u) = PLAN.units.iter().position(|unit| unit.request == channel.unit) {
						if self.checks[u] == Check::Matched {
							self.checks[u] = Check::Absent;
							note!("can: {:03X} went silent — will keep asking", channel.unit);
							self.unsubscribe_unit(u, bus).await;
							self.ask_part_number(u, bus);
							self.say_dead_bus();
						}
					}
				}
				None
			}
			Delivery::Raw { .. } => Some(delivery),
		}
	}

	fn channel_of(&self, sub: SubId) -> Option<usize> {
		self.subs.iter().position(|s| s.is_some_and(|s| s.id == sub))
	}

	/// Three periods of a channel read slower than [`STALE`] allows, else [`STALE`].
	fn fresh_for(&self, index: usize) -> Duration {
		let period = self.subs[index].map_or(0, |s| s.period_ms);
		let three = Duration::from_millis(u64::from(period) * 3);
		if three > STALE { three } else { STALE }
	}

	/// Said on the change: no unit answers, or one does again.
	fn say_dead_bus(&mut self) {
		// `is_empty` on the plan rather than `UNIT_COUNT > 0`: with the empty plan a
		// CI build links, the constant comparison is one clippy refuses.
		let dead = !PLAN.units.is_empty() && self.checks.iter().all(|c| *c == Check::Absent);
		if dead != self.dead_bus {
			self.dead_bus = dead;
			if dead {
				note!(
					"can: no unit answers — asking each for its part number every {} s at most until one does",
					Budget::default().backoff_cap_ms / 1000
				);
			} else {
				note!("can: the bus is answering again");
			}
		}
	}
}

fn unit_of(u: usize) -> Unit {
	Unit {
		request: PLAN.units[u].request,
		response: PLAN.units[u].response,
	}
}

fn miss_text(why: Miss) -> heapless::String<32> {
	let mut out = heapless::String::new();
	let _ = match why {
		Miss::NoAnswer => write!(out, "no answer"),
		Miss::BusError => write!(out, "bus error"),
		Miss::Refused(nrc) => write!(out, "refused, NRC {nrc:02X}"),
		Miss::Absent => write!(out, "left out of the answer"),
		Miss::Malformed => write!(out, "answer did not parse"),
	};
	out
}

/// What the unit's part-number answer means, decided by
/// [`vag_dash_render::plan::Unit::check_part`] (host-tested there) and said here, once
/// per change: a match, a mismatch (another number, not text, or a refusal to say —
/// never polled this run), silence (asked again under the planner's backoff), or an
/// answer that did not parse (asked again after the backoff's cap).
fn judge(u: usize, answer: Result<&[u8], &Miss>, previous: Check) -> PartCheck {
	let unit = &PLAN.units[u];
	let part = match answer {
		Ok(data) => PartAnswer::Data(data),
		Err(Miss::Refused(nrc)) => PartAnswer::Refused(*nrc),
		Err(Miss::NoAnswer) => PartAnswer::NoAnswer,
		Err(Miss::BusError) => PartAnswer::BusError,
		Err(Miss::Malformed | Miss::Absent) => PartAnswer::Malformed,
	};
	let check = unit.check_part(part);
	match (check, answer) {
		(PartCheck::Matched, _) => note!("can: {:03X} is {} as planned", unit.request, unit.part_number),
		(PartCheck::Mismatch, Ok(data)) => match core::str::from_utf8(data) {
			Ok(reported) => note!(
				"can: {:03X} is {:?}, the plan was built for {} — not polling it",
				unit.request,
				reported.trim_end_matches([' ', '\0']),
				unit.part_number
			),
			Err(_) => note!(
				"can: {:03X} answered F187 with {:02X?}, not a part number — not polling it",
				unit.request,
				data
			),
		},
		(PartCheck::Mismatch, Err(why)) => note!(
			"can: {:03X} refused F187 ({}) — cannot check it against the plan, not polling it",
			unit.request,
			miss_text(*why)
		),
		(PartCheck::Absent | PartCheck::RetryLater, Err(why)) if previous != Check::Absent => {
			note!("can: {:03X} did not answer F187 ({}) — will keep asking", unit.request, miss_text(*why));
		}
		_ => {}
	}
	check
}

/// Puts one reading in the store. `None` clears the slot, timestamp and all.
async fn store(index: usize, value: Option<f32>, fresh_for: Duration) {
	let mut values = VALUES.lock().await;
	if let Some(slot) = values.get_mut(index) {
		*slot = Slot {
			value,
			at: value.map(|_| Instant::now()),
			fresh_for,
		};
	}
}

/// How often the panel draws — and, because a chart's column is one frame,
/// how much time one column holds. Five a second: fast enough to look live
/// over a terminal, slow enough that the encoding never becomes the
/// bottleneck.
const FRAME_MS: u64 = 200;

/// The longest `FRAME` line: two hex digits per run of pixels at worst, and its header.
const FRAME_LINE: usize = vag_dash_fw::panel::WIDTH * vag_dash_fw::panel::HEIGHT / 8 * 2 + 32;

/// The last frame drawn, as its `FRAME` line, for `usb_writer_task`. A static and not
/// a task local: four kilobytes in the embassy arena, which every task shares, is a
/// fifth of it.
static PANEL_LINE: Mutex<CriticalSectionRawMutex, heapless::String<FRAME_LINE>> = Mutex::new(heapless::String::new());

/// Raised when [`PANEL_LINE`] holds a frame not yet written.
static PANEL_READY: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Draws the current page and leaves its pixels for the USB port.
///
/// This is the real renderer on real pixels: `vag_dash_render::draw` into a 256×64
/// framebuffer. What the laptop shows is not an impression of the panel, it is
/// the panel.
///
/// Labels, units and decimals come from the plan; values come from
/// [`VALUES`], where the bus task left them. A cell whose channel has not
/// answered is drawn with `None`, and the renderer draws a dash. Nothing here
/// invents a number.
///
/// In adapter mode the page is the adapter's: `SLCAN`, the bit rate and the counters.
/// It is drawn into the framebuffer — the glass, when the panel is fitted — and not
/// sent as a `FRAME` line, because then the cable is an slcan host's.
///
/// Which page is drawn is [`Screen::frame`]'s answer: the page cursor, unless an alarm
/// has taken the glass, in which case its page is drawn with the offending channel's cell
/// inverted. The alarms see the same values the cells do, `None` once stale. On the
/// adapter screen they are not polled.
#[embassy_executor::task]
async fn panel_task(settings: &'static Shared, screen: &'static ScreenCell) -> ! {
	use vag_dash_render::history::History;
	use vag_dash_render::{Cell, Frame, Theme, draw};

	static FRAMEBUFFER: StaticCell<Framebuffer> = StaticCell::new();
	let framebuffer = FRAMEBUFFER.init(Framebuffer::new());
	let theme = Theme::bold_mono();

	// One history per chart the plan has, in the plan's order — `PLAN.chart`
	// says which slot a channel's is. Every one of them takes a sample **every
	// frame, whether or not a chart is on the glass**, so the chart page comes
	// up with its last `WIDTH × FRAME_MS` already drawn. Sampling only while
	// the chart was shown, and starting over on each entry, is why it used to
	// come up empty (2026-09-13). A history is of one channel and ends at a
	// missing value — `History` says why.
	//
	// Static rather than a local: a task's locals live in the embassy arena
	// (20 KiB for every task together), and these are
	// `CHART_COUNT × (WIDTH × 4 + 8)` bytes — 1032 per chart — which belong in
	// `.bss` next to the framebuffer.
	static HISTORIES: StaticCell<[History<{ vag_dash_fw::panel::WIDTH }>; CHART_COUNT]> = StaticCell::new();
	let histories = HISTORIES.init(core::array::from_fn(|_| History::new()));
	let mut last_compromised = false;
	// A chart page whose channel the plan gives no range for is said once.
	let mut no_range_said: Option<u16> = None;
	// An alarm page this board does not hold is said once per miss.
	let mut missed_said: Option<u16> = None;

	loop {
		Timer::after(Duration::from_millis(FRAME_MS)).await;

		if adapter_mode() {
			screen.lock(|cell| cell.borrow_mut().adapter());
			framebuffer.clear_all();
			let report = draw(&Frame::Adapter(SLCAN_PORT.status()), &theme, framebuffer);
			let compromised = report != vag_dash_render::render::Report::default();
			if compromised != last_compromised {
				last_compromised = compromised;
				note!("panel: {report:?}");
			}
			continue;
		}

		// A copy, so the lock is held for a memcpy and not for a frame.
		let values = *VALUES.lock().await;
		let now = Instant::now();
		let value_of = |index: u16| values.get(usize::from(index)).and_then(|slot| slot.current(now));
		// The cursor is read here and passed in, never copied into the screen: it is what
		// `set page` and `save` act on, and what an alarm hands back to.
		let (kind, indices, glass) = {
			let s = settings.lock().await;
			// `pages` is bounded by `MAX_PAGES`, so the count fits.
			let pages = s.config.pages.len() as u8;
			let glass = screen.lock(|cell| cell.borrow_mut().frame(s.config.active_page, pages, now.as_millis(), value_of));
			match s.config.pages.get(usize::from(glass.page)) {
				Some(page) => (page.kind, page.cells.clone(), glass),
				None => continue,
			}
		};
		// The bus reads the page on the glass in the foreground, so a takeover and a
		// hand-back move its subscriptions as much as a page turn does.
		GLASS_PAGE.store(glass.page, Ordering::Relaxed);
		if glass.page_changed {
			PAGES_CHANGED.signal(());
		}
		let page_no = usize::from(glass.page) + 1;
		match glass.change {
			Some(Change::Took { rule }) if glass.missed.is_none() => note!(
				"alarm: rule {} — {} took the screen, page {page_no}",
				rule + 1,
				glass.offending.and_then(|c| PLAN.channel(c.0)).map_or("?", |c| c.label)
			),
			Some(Change::Over) => note!("alarm: over — back to page {page_no}"),
			Some(Change::Silenced) => note!("alarm: silenced — back to page {page_no}"),
			_ => {}
		}
		let missed = glass.missed.map(|page| page.0);
		if missed != missed_said {
			missed_said = missed;
			if let Some(page) = missed {
				note!(
					"alarm: its page {} is not on this board — showing page {page_no}; rebuild the plan",
					usize::from(page) + 1
				);
			}
		}
		for chart in PLAN.charts() {
			histories[chart.slot].push(chart.channel, value_of(chart.channel));
		}
		// A cell the plan cannot name draws as a question mark rather than
		// vanishing: a missing column hides the fault, a wrong one shows it. The
		// alarm's offending channel is drawn inverted, so the page says which one.
		let cell_of = |index: u16| {
			let cell = match PLAN.channel(index) {
				Some(channel) => Cell::new(channel.label, value_of(index), channel.unit_text, channel.decimals),
				None => Cell::new("?", None, "", 0),
			};
			if glass.offending == Some(ChannelId(index)) {
				cell.alarmed()
			} else {
				cell
			}
		};

		framebuffer.clear_all();
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
				match PLAN.chart(index) {
					Some(chart) => {
						let samples = histories[chart.slot].samples();
						draw(
							&Frame::Chart {
								cell: cell_of(index),
								min: chart.min,
								max: chart.max,
								samples,
								seconds_per_sample: FRAME_MS as f32 / 1000.0,
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
		// The renderer reports what it had to compromise — a label too long,
		// a unit it had to drop. It is the same answer every frame, so say it
		// when it changes and never otherwise.
		let compromised = report.label_overrun || report.value_overrun || report.unit_dropped || report.value_shrunk || report.glyph_missing;
		if compromised != last_compromised {
			last_compromised = compromised;
			note!("panel: {report:?}");
		}

		// Left for the writer, which puts it on the wire whole between everything else.
		let mut line = PANEL_LINE.lock().await;
		line.clear();
		if framebuffer.write_frame(&mut *line).is_ok() {
			PANEL_READY.signal(());
		}
	}
}

/// Framed messages from the host on the cable, for its session. Small: a full one holds
/// the reader back, and so the host.
static USB_MESSAGES: Channel<CriticalSectionRawMutex, Message, 4> = Channel::new();

/// Bytes for the cable, each written whole.
static USB_OUT: Channel<CriticalSectionRawMutex, UsbOut, 8> = Channel::new();

/// The bytes in `USB_OUT` and in the writer's hands ([`QUEUED_OUT_BYTES`]).
static USB_OUT_BYTES: QueuedBytes = QueuedBytes::new();

enum UsbOut {
	/// One encoded link frame. Cut short by a stall, it is closed with filler later.
	Frame(Vec<u8>),
	/// Text, such as the `\r` a bare `C` is answered with.
	Text(Vec<u8>),
}

/// PDU bytes of requests the reader has put in `USB_MESSAGES` that the session has not
/// taken yet: the channel's share of `QUEUED_PDU_BYTES`. Two writers, so changed in a
/// critical section.
static USB_INBOUND_BYTES: AtomicU32 = AtomicU32::new(0);

/// Raised whenever the cable's session has taken something in, or let something go: the
/// reader, stopped at the cap, looks again.
static USB_ROOM: Signal<CriticalSectionRawMutex, ()> = Signal::new();

fn inbound(add: usize, take: usize) {
	critical_section::with(|_| {
		let now = USB_INBOUND_BYTES.load(Ordering::Relaxed) as usize;
		USB_INBOUND_BYTES.store(now.saturating_add(add).saturating_sub(take) as u32, Ordering::Relaxed);
	});
}

/// What the host on the cable has waiting on the board, in PDU bytes.
fn usb_backlog() -> usize {
	USB_INBOUND_BYTES.load(Ordering::Relaxed) as usize + USB_CLIENT.queued_bytes()
}

/// The host is gone, for the console: its cable was pulled. One signal per consumer.
static USB_GONE_FOR_CONSOLE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// The host is gone or not reading, for its session.
static USB_GONE_FOR_SESSION: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Whether a host is attached: its start-of-frame counter moved at the last look.
static USB_PRESENT: AtomicBool = AtomicBool::new(false);

/// How often the host's start-of-frame counter is looked at. It moves every millisecond
/// while a host is attached (USB 2.0 full speed), so a hundred milliseconds without a
/// move is a cable pulled or a host asleep, never a slow one.
const PRESENCE_POLL: Duration = Duration::from_millis(100);

/// How long one write may wait for the host to take it. A host that reads takes a
/// packet within a few milliseconds; one that has not taken it in a second is not
/// reading — a port nobody has open, a process stopped — and waiting longer only
/// stalls every other line behind it.
const WRITE_STALL: Duration = Duration::from_secs(1);

/// Watches for the host: esp-hal `=1.0.0-rc.0` has no event for a USB disconnect, and
/// the peripheral's start-of-frame counter is one (`vag_dash_fw::usb::sof_frame`).
#[embassy_executor::task]
async fn usb_presence_task() -> ! {
	let mut last = usb::sof_frame();
	loop {
		Timer::after(PRESENCE_POLL).await;
		let frame = usb::sof_frame();
		let present = frame != last;
		last = frame;
		let was = USB_PRESENT.load(Ordering::Relaxed);
		USB_PRESENT.store(present, Ordering::Relaxed);
		match (was, present) {
			(true, false) => {
				// Said when a host next reads: nothing reaches one before.
				note!("usb: the host went away — its session is closed, adapter mode is over");
				USB_GONE_FOR_CONSOLE.signal(());
				USB_GONE_FOR_SESSION.signal(());
			}
			(false, true) => USB_GONE_FOR_SESSION.reset(),
			_ => {}
		}
	}
}

/// The one task that writes the cable. Everything the board says there comes through
/// here, whole: the host's link frames first, then the adapter's lines, then log lines
/// and notes, then the panel's `FRAME` line — so a log line can never be written in
/// the middle of a frame's bytes. In adapter mode no log line and no panel line goes out:
/// the host there is an slcan client, whose reader takes a line starting with `t` for a CAN
/// frame. Link frames and the `\r` a bare `C` is answered with go out in either mode — a
/// Hello is answered in adapter mode too, and the probe (`vag_uds_can::slcan::ask_board`)
/// relies on both.
///
/// While no host is attached nothing but the adapter's ring is taken off the queues, so
/// what was said at boot is there when one arrives; the queues that fill drop as they
/// always do. The ring is drained, its lines counted as lost: nothing else reads it, and a
/// full ring would hold the adapter's next reply — and the adapter — until a host came.
#[embassy_executor::task]
async fn usb_writer_task(usb: UsbSerialJtagTx<'static, Async>) -> ! {
	let mut writer = Writer {
		usb,
		stalled: false,
		owed: 0,
	};
	let mut carry: Option<Line> = None;
	let mut packet = Packet::new();
	loop {
		if !USB_PRESENT.load(Ordering::Relaxed) {
			writer.stalled = true;
			// The host whose reassembler is inside a cut frame is gone with the cable. The next
			// host never saw that frame's head: its zeros and the broken header would only be
			// garbage ahead of that host's handshake, and `dashsim` would read them first.
			writer.owed = 0;
			let drained = select(Timer::after(PRESENCE_POLL), SLCAN_PORT.next_packet(&mut carry, &mut packet)).await;
			if let Either::Second(packed) = drained {
				SLCAN_PORT.lost(packed);
				SLCAN_PORT.written(packed, &mut carry);
			}
			continue;
		}
		let event = select4(
			USB_OUT.receive(),
			SLCAN_PORT.next_packet(&mut carry, &mut packet),
			usb::LINES.receive(),
			PANEL_READY.wait(),
		)
		.await;
		match event {
			// Counted out once written, or given up on: either way out of the writer's hands.
			Either4::First(UsbOut::Frame(bytes)) => {
				writer.put(&bytes, true).await;
				USB_OUT_BYTES.written(bytes.len());
			}
			Either4::First(UsbOut::Text(bytes)) => {
				writer.put(&bytes, false).await;
				USB_OUT_BYTES.written(bytes.len());
			}
			Either4::Second(packed) => {
				if !writer.put(&packet, false).await {
					SLCAN_PORT.lost(packed);
				}
				SLCAN_PORT.written(packed, &mut carry);
			}
			Either4::Third(line) => {
				if !adapter_mode() && writer.put(line.as_bytes(), false).await {
					writer.put(b"\r\n", false).await;
				}
			}
			Either4::Fourth(()) => {
				if !adapter_mode() {
					let line = PANEL_LINE.lock().await;
					if writer.put(line.as_bytes(), false).await {
						writer.put(b"\r\n", false).await;
					}
				}
			}
		}
	}
}

/// The port, and whether the host stopped taking what is written.
struct Writer {
	usb: UsbSerialJtagTx<'static, Async>,
	/// A write gave up waiting, and its last packet may still be in the endpoint.
	stalled: bool,
	/// Body bytes of a link frame cut short by a stall, not yet sent as filler.
	owed: usize,
}

/// One USB-Serial-JTAG packet: what the endpoint holds, and what esp-hal writes at a time.
const USB_PACKET: usize = 64;

impl Writer {
	/// Write `bytes` whole; `false` when they were not written whole, because no host is
	/// taking them.
	///
	/// esp-hal's write waits for the host to take each packet, which with no host
	/// reading is forever, so each packet gets [`WRITE_STALL`]. A packet whose wait ran
	/// out is already in the endpoint; the rest is not written. Until the host has taken
	/// that packet nothing more is written, because a write into a full endpoint overruns
	/// it.
	///
	/// A link frame (`frame`) cut that way is a frame the host's reassembler is still
	/// inside: whatever came next would be read as its body. So when the host takes
	/// packets again, the rest of its length goes first as zeros, and then
	/// [`link::BROKEN_FRAME`], a header nobody sends. The zeros close the cut frame where
	/// it was declared to end; the header makes a host that goes on reading drop the link
	/// as having lost data. (The cut frame itself may decode, padded — its first packet
	/// holds its header and status, and zeros are a valid payload — and reach that host
	/// just before the header ends its link. A new host's handshake discards both.)
	async fn put(&mut self, bytes: &[u8], frame: bool) -> bool {
		if self.stalled {
			if !usb::packet_taken() {
				return false;
			}
			self.stalled = false;
			if self.owed > 0 {
				const ZEROS: [u8; USB_PACKET] = [0; USB_PACKET];
				while self.owed > 0 {
					let n = self.owed.min(USB_PACKET);
					if !self.packet(&ZEROS[..n]).await {
						self.owed -= n;
						return false;
					}
					self.owed -= n;
				}
				if !self.packet(&link::BROKEN_FRAME).await {
					return false;
				}
			}
		}
		let mut sent = 0;
		for chunk in bytes.chunks(USB_PACKET) {
			sent += chunk.len();
			if !self.packet(chunk).await {
				if frame {
					self.owed = bytes.len() - sent;
				}
				return false;
			}
		}
		true
	}

	/// One packet of at most [`USB_PACKET`] bytes; `false` when the host did not take it in
	/// [`WRITE_STALL`] — it is in the endpoint all the same.
	async fn packet(&mut self, bytes: &[u8]) -> bool {
		if with_timeout(WRITE_STALL, embedded_io_async::Write::write_all(&mut self.usb, bytes))
			.await
			.is_ok()
		{
			return true;
		}
		self.stalled = true;
		if adapter_mode() {
			// The slcan host stopped reading: its process is gone or stopped. Adapter mode
			// ends as it does when the cable is pulled, so a host that died does not leave
			// the panel blank. A quiet bus with a dead host writes nothing and so never
			// stalls: that still takes `C`, the cable, or the next host's `\rC\r`.
			note!("usb: the slcan host stopped reading — adapter mode is over");
			USB_GONE_FOR_CONSOLE.signal(());
		}
		if USB_CLIENT.active() {
			note!("usb: the host stopped reading — its session is closed");
			USB_GONE_FOR_SESSION.signal(());
		}
		false
	}
}

/// Everything the host sends on the cable, through `vag_uds_client::console`.
#[embassy_executor::task]
async fn usb_reader_task(mut usb: UsbSerialJtagRx<'static, Async>) -> ! {
	let mut console = Console::new();
	let mut buffer = [0u8; 64];
	loop {
		// What the board waited on since the last chunk — `USB_ROOM` below, or a full
		// `USB_MESSAGES`, `SLCAN_IN` or `USB_OUT` while handing that chunk on — is its own
		// back-pressure, not the host's silence: the host's rest of a frame was sent meanwhile
		// and sits in the FIFO. Counted, a 200 ms wait gave the frame up and read its tail as
		// text, which could switch the board into adapter mode. Only the read counts.
		console.resume(ms());
		// Past the cap the host is not read: it waits, and nothing it sent is dropped.
		let full = usb_backlog() >= QUEUED_PDU_BYTES;
		let read = async {
			if full {
				USB_ROOM.wait().await;
				return None;
			}
			Some(embedded_io_async::Read::read(&mut usb, &mut buffer).await)
		};
		let event = select(read, USB_GONE_FOR_CONSOLE.wait()).await;
		match event {
			Either::First(None) => {}
			Either::First(Some(read)) => {
				// The async read cannot fail on this peripheral, but going through
				// `Result` keeps the shape right if it ever moves to a UART.
				let n = read.unwrap_or(0);
				for input in console.push_at(&buffer[..n], USB_CLIENT.active(), ms()) {
					take_console_input(input).await;
				}
			}
			Either::Second(()) => {
				if let Some(input) = console.disconnected() {
					take_console_input(input).await;
				}
			}
		}
	}
}

/// Set when a `C` ended adapter mode, so the session's end is noted by the adapter's
/// task once `serve` has returned — after the `C`'s own `\r`, not before it. Noted from
/// the console, the line reached the host ahead of the reply: a Lawicel host reading the
/// answer to `C` took the note's first byte for it (bench, 2026-09-14).
static ADAPTER_LEFT_BY_C: AtomicBool = AtomicBool::new(false);

/// Act on one thing the console decided.
async fn take_console_input(input: ConsoleInput) {
	match input {
		ConsoleInput::Message(message) => {
			inbound(pdu_bytes(&message), 0);
			USB_MESSAGES.send(message).await;
		}
		ConsoleInput::Malformed(why) => note!("usb: a malformed frame from the host was dropped: {why}"),
		// The simulator's presses go through the same handling as the physical button.
		ConsoleInput::Press(console::Button::Short) => REMOTE_PRESS.signal(Press::Short),
		ConsoleInput::Press(console::Button::Long) => REMOTE_PRESS.signal(Press::Long),
		ConsoleInput::EnterAdapter => {
			note!("usb: slcan on the cable — the board is a CAN adapter until C or the cable is pulled");
			set_mode(Mode::Adapter);
		}
		ConsoleInput::Slcan(line) => SLCAN_IN.send(SlcanIn::Line(line)).await,
		ConsoleInput::LeaveAdapter => {
			// Said before the queue is waited on: the adapter may be the one holding it up.
			ADAPTER_ENDING.store(true, Ordering::Relaxed);
			ADAPTER_ENDING_RAISED.signal(());
			ADAPTER_LEFT_BY_C.store(true, Ordering::Relaxed);
			SLCAN_IN.send(SlcanIn::Leave).await;
			set_mode(Mode::Panel);
		}
		ConsoleInput::Closed => USB_OUT_BYTES.send(&USB_OUT, UsbOut::Text(alloc::vec![b'\r']), 1).await,
		ConsoleInput::Ignored {
			line,
			why: Ignored::LinkActive,
		} => note!("usb: {line:?} not taken — a link client holds a session on the cable"),
		ConsoleInput::Ignored {
			line,
			why: Ignored::NotACommand,
		} => note!("remote: ignoring {line:?}"),
	}
}

/// The host on the cable: a second client of the planner, beside BLE, with a session
/// of its own held to the cable's guard (`Guard::cable`).
#[embassy_executor::task]
async fn usb_session_task(bus: &'static Bus) -> ! {
	let client = &USB_CLIENT;
	let mut session = Session::with_guard(Guard::cable());
	let mut dropped_said = 0;
	loop {
		let wake = session_wake(&session);
		let full = session.queued() >= SESSION_QUEUE_MAX || session.queued_bytes() >= QUEUED_PDU_BYTES;
		let inbox = async {
			if full {
				core::future::pending::<()>().await;
			}
			USB_MESSAGES.receive().await
		};
		let event = select3(
			select4(inbox, client.answers.receive(), client.readings.receive(), Timer::at(wake)),
			MODE_FOR_USB.wait(),
			USB_GONE_FOR_SESSION.wait(),
		)
		.await;
		let out = match event {
			Either3::First(Either4::First(message)) => {
				inbound(0, pdu_bytes(&message));
				if message == Message::Hello {
					// A host that says hello is here and reading, whatever an earlier stall said.
					USB_GONE_FOR_SESSION.reset();
					client.reset();
				}
				take_message(&mut session, bus, message)
			}
			Either3::First(Either4::Second((req, answer, at))) => bus.lock(|p| session.answered(at, &mut p.borrow_mut(), req, &answer)),
			Either3::First(Either4::Third(delivery)) => bus.lock(|p| session.deliver(ms(), &mut p.borrow_mut(), &delivery)).into_iter().collect(),
			Either3::First(Either4::Fourth(())) => bus.lock(|p| session.poll(ms(), &mut p.borrow_mut())),
			Either3::Second(()) => mode_changed(&mut session, bus),
			Either3::Third(()) => {
				close_usb_session(&mut session, bus);
				Vec::new()
			}
		};
		client.publish(&session);
		USB_ROOM.signal(());
		BUS_WAKE.signal(());
		for message in out {
			let frame = match link::encode(&message) {
				Ok(frame) => frame,
				Err(e) => {
					note!("usb: an answer for the host did not encode: {e}");
					continue;
				}
			};
			let bytes = frame.len();
			if let Either::Second(()) = select(USB_OUT_BYTES.send(&USB_OUT, UsbOut::Frame(frame), bytes), USB_GONE_FOR_SESSION.wait()).await {
				close_usb_session(&mut session, bus);
				client.publish(&session);
				break;
			}
		}
		client.say_dropped("usb", &mut dropped_said);
	}
}

/// The host on the cable is gone: its subscriptions leave the planner, its queued
/// exchange is cancelled, and nothing it sent and nobody read is sent to the car.
fn close_usb_session(session: &mut Session, bus: &Bus) {
	bus.lock(|p| session.close(ms(), &mut p.borrow_mut()));
	USB_MESSAGES.clear();
	critical_section::with(|_| USB_INBOUND_BYTES.store(0, Ordering::Relaxed));
	USB_CLIENT.reset();
	USB_CLIENT.publish(session);
	USB_ROOM.signal(());
	BUS_WAKE.signal(());
}

/// The configuration protocol, in its first and deliberately dumbest form:
/// lines of text. It shares the UART service with the framed UDS link, which
/// starts every message with a NUL that no text line has (`link`'s docs), and
/// text is what a person with a terminal can drive by hand.
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
						PAGES_CHANGED.signal(());
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
							PAGES_CHANGED.signal(());
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
			PAGES_CHANGED.signal(());
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
