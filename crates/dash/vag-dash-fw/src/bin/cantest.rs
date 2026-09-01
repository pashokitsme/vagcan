//! A UDS round trip that needs no car, no transceiver and no second board.
//!
//! The claim being tested is that the whole protocol stack — `vag-uds-client`'s
//! UDS client and allowlist, `vag-uds-can`'s ISO-TP segmentation, `vag-uds-transport`'s
//! seam — compiles for `riscv32imc-unknown-none-elf` and drives real silicon.
//! The only new code under it is `vag_dash_fw::can::TwaiBackend`; everything
//! above that line is the same source the laptop runs.
//!
//! `TwaiMode::SelfTest` drops the requirement that somebody acknowledge a
//! frame, and `EspTwaiFrame::new_self_reception` asks the controller to deliver
//! a copy of what it just sent into its own receive queue. So the board is both
//! ends of the conversation.
//!
//! # Wiring
//!
//! **None, by default.** [`TX_PIN`] is routed through the GPIO matrix as *both*
//! the TWAI transmit output and the TWAI receive input, which is what ESP-IDF
//! calls a `tx_io == rx_io` loopback, and `new_no_transceiver` puts the pad in
//! open-drain-with-pull-up so a dominant bit reads back as a dominant bit.
//!
//! If that does not close on your board, set [`USE_JUMPER`] to `true` and put
//! one wire between [`TX_PIN`] and [`RX_PIN`]. Both are chosen from GPIO6, 7,
//! 10, 20, 21: GPIO0–5 are spoken for by the ADC divider and the wake button,
//! and GPIO2, 8 and 9 are strapping pins.
//!
//! # What each stage proves, and what none of them can
//!
//! 1. **backend** — a raw frame leaves the controller and comes back byte for
//!    byte. If this fails, nothing above it is implicated: it is pins, bit
//!    rate, or mode.
//! 2. **iso-tp** — the same bytes make the trip as an ISO-TP *PDU*, through
//!    `IsoTpCan`'s single-frame path and its 8-byte padding.
//! 3. **uds** — `AsyncUdsClient` encodes `0x22 F1 90`, the frame goes out, the
//!    firmware hears its own request and answers it, and the client parses the
//!    reply. Read the log: **the answer is fabricated by this binary**. There
//!    is no control unit on this bus. What the stage proves is that the request
//!    was encoded, framed, transmitted, received, reassembled and parsed — not
//!    anything about a car.
//!
//! Nothing here exercises the *multi-frame* path, and it cannot: a First Frame
//! stalls until the other end sends flow control, and in a loopback the only
//! thing that comes back is the First Frame itself. Multi-frame segmentation is
//! covered by `vag-uds-can`'s unit tests, which run the identical code on the host.
//!
//! This binary transmits, so it must never be pointed at a car. On a real bus
//! the mode to start from is `TwaiMode::ListenOnly` — see the module docs of
//! `vag_dash_fw::can`.

#![no_std]
#![no_main]
#![deny(
	clippy::mem_forget,
	reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

use alloc::vec::Vec;
use core::time::Duration;
use embassy_executor::Spawner;
use embassy_time::Timer;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::twai::{BaudRate, TwaiConfiguration, TwaiMode};
use log::{error, info};
use vag_dash_fw::can::TwaiBackend;
use vag_uds_can::{CanBackend, IsoTpCan};
use vag_uds_client::AsyncUdsClient;
use vag_uds_transport::{AsyncIsoTpTransport, CanId, TransportError};

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

/// The TWAI transmit pad, and — unless [`USE_JUMPER`] — the receive pad too.
const TX_PIN: u8 = 6;
/// The receive pad, used only when [`USE_JUMPER`] is set.
const RX_PIN: u8 = 7;
/// `false`: one pad is both directions and nothing needs wiring. `true`: two
/// pads with a jumper across them. See the module docs.
const USE_JUMPER: bool = false;

/// 500 kbit/s is what VW's diagnostic CAN runs at, so the loop is tested at the
/// rate the real thing will use.
const BITRATE: BaudRate = BaudRate::B500K;

/// Both ends of the loop are this board, so the "ECU" answers on the tester's
/// own id. On a car these differ (`0x7E0` out, `0x7E8` back).
const LOOP_ID: CanId = CanId::Standard(0x7E0);

/// How long any single stage waits before calling it a failure.
const STAGE_TIMEOUT: Duration = Duration::from_millis(500);

/// The identifier asked for: `F190` is the VIN. Chosen because it is the read
/// this project's `info` command starts from.
const DID_VIN: u16 = 0xF190;

#[esp_hal_embassy::main]
async fn main(_spawner: Spawner) {
	// Not `init_logger_from_env`: `.cargo/config.toml` pins `ESP_LOG=warn`, and
	// a stage-by-stage probe whose stages are invisible is no use at all. This
	// binary asks for Info and means it.
	esp_println::logger::init_logger(log::LevelFilter::Info);

	let peripherals = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
	esp_alloc::heap_allocator!(size: 32 * 1024);
	esp_hal_embassy::init(SystemTimer::new(peripherals.SYSTIMER).alarm0);

	// SAFETY (both `steal` calls and `clone_unchecked`): this binary configures
	// no other GPIO, so nothing else in it holds these pads. Without a jumper
	// the same pad is handed to the TWAI peripheral twice on purpose — the GPIO
	// matrix drives the transmit signal onto it and reads the receive signal off
	// it, and open-drain-with-pull-up makes that legible.
	let tx_pin = unsafe { esp_hal::gpio::AnyPin::steal(TX_PIN) };
	let rx_pin = if USE_JUMPER {
		info!("wiring: jumper expected between GPIO{TX_PIN} (tx) and GPIO{RX_PIN} (rx)");
		unsafe { esp_hal::gpio::AnyPin::steal(RX_PIN) }
	} else {
		info!("wiring: none — GPIO{TX_PIN} is both tx and rx through the GPIO matrix");
		unsafe { tx_pin.clone_unchecked() }
	};

	// `new_no_transceiver` is the point: open-drain plus pull-up means the pad
	// itself provides the recessive level a CAN transceiver would.
	let config = TwaiConfiguration::new_no_transceiver(peripherals.TWAI0, rx_pin, tx_pin, BITRATE, TwaiMode::SelfTest).into_async();
	let twai = config.start();
	info!("twai: started, 500 kbit/s, self-test mode");

	let mut failures = 0usize;

	let backend = match stage_backend(TwaiBackend::self_test(twai)).await {
		Ok(b) => b,
		Err(b) => {
			failures += 1;
			b
		}
	};

	let iso = IsoTpCan::new(backend, LOOP_ID, LOOP_ID);
	let iso = match stage_isotp(iso).await {
		Ok(i) => i,
		Err(i) => {
			failures += 1;
			i
		}
	};

	if stage_uds(iso).await.is_err() {
		failures += 1;
	}

	if failures == 0 {
		info!("== all stages passed: the stack runs on this chip ==");
	} else {
		error!("== {failures} stage(s) failed — the first failing stage is the one to fix ==");
	}

	loop {
		Timer::after(embassy_time::Duration::from_secs(60)).await;
	}
}

/// Stage 1: one raw frame out, the same frame back.
async fn stage_backend(mut backend: TwaiBackend<'static>) -> Result<TwaiBackend<'static>, TwaiBackend<'static>> {
	const PROBE: [u8; 8] = [0x02, 0x3E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00];
	let id = 0x7E0;

	info!("[1/3 backend] sending id {id:#05X} data {PROBE:02X?}");
	if let Err(e) = backend.send_frame(id, &PROBE).await {
		error!("[1/3 backend] transmit failed: {e}");
		error!("[1/3 backend] the controller could not put a frame on the wire at all — check the mode and the bit rate");
		return Err(backend);
	}

	match backend.recv_frame(STAGE_TIMEOUT).await {
		Ok((got_id, data)) if got_id == id && data == PROBE => {
			info!("[1/3 backend] heard itself: id {got_id:#05X} data {data:02X?} — OK");
			Ok(backend)
		}
		Ok((got_id, data)) => {
			error!("[1/3 backend] frame came back changed: id {got_id:#05X} data {data:02X?}");
			Err(backend)
		}
		Err(e) => {
			error!("[1/3 backend] nothing came back: {e}");
			error!("[1/3 backend] the loop is open — set USE_JUMPER and wire GPIO{TX_PIN} to GPIO{RX_PIN}");
			Err(backend)
		}
	}
}

/// Stage 2: the same bytes as an ISO-TP PDU, through the real segmentation code.
async fn stage_isotp(mut iso: IsoTpCan<TwaiBackend<'static>>) -> Result<IsoTpCan<TwaiBackend<'static>>, IsoTpCan<TwaiBackend<'static>>> {
	let pdu = [0x22, 0xF1, 0x90];

	info!("[2/3 iso-tp] sending pdu {pdu:02X?} (single frame, padded to 8)");
	if let Err(e) = iso.send(&pdu).await {
		error!("[2/3 iso-tp] send failed: {e}");
		return Err(iso);
	}

	match iso.recv(STAGE_TIMEOUT).await {
		Ok(got) if got == pdu => {
			info!("[2/3 iso-tp] pdu came back intact: {got:02X?} — OK");
			Ok(iso)
		}
		Ok(got) => {
			error!("[2/3 iso-tp] pdu came back changed: {got:02X?}, wanted {pdu:02X?}");
			Err(iso)
		}
		Err(e) => {
			error!("[2/3 iso-tp] no pdu came back: {e}");
			Err(iso)
		}
	}
}

/// Stage 3: the UDS client, end to end, against this same firmware playing ECU.
async fn stage_uds(iso: IsoTpCan<TwaiBackend<'static>>) -> Result<(), ()> {
	info!("[3/3 uds] reading DID {DID_VIN:#06X} through AsyncUdsClient");
	info!("[3/3 uds] NOTE: there is no control unit on this bus — the reply below is fabricated by this firmware");

	let mut uds = AsyncUdsClient::new(SelfAnsweringLoop { iso });
	match uds.read_data_by_identifier(DID_VIN).await {
		Ok(payload) => {
			info!("[3/3 uds] client parsed a positive response, payload {payload:02X?} — OK");
			Ok(())
		}
		Err(e) => {
			error!("[3/3 uds] round trip failed: {e}");
			Err(())
		}
	}
}

/// The loopback wearing an ECU's hat.
///
/// `AsyncUdsClient` sends a request and then waits for an answer. On a car the
/// answer comes from a control unit; here it has to come from somewhere, so
/// `recv` builds the positive response, puts it on the *wire* through the same
/// TWAI controller, and returns what the controller hears. Both directions
/// really travel through the peripheral — that is the part being tested. The
/// content of the answer is invented, and stage 3's log says so.
struct SelfAnsweringLoop {
	iso: IsoTpCan<TwaiBackend<'static>>,
}

impl AsyncIsoTpTransport for SelfAnsweringLoop {
	async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError> {
		self.iso.send(pdu).await?;
		// Drain our own echo of the request; it is not the answer.
		let echo = self.iso.recv(STAGE_TIMEOUT).await?;
		info!("[3/3 uds]   request on the wire, heard back: {echo:02X?}");
		Ok(())
	}

	async fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
		// A positive response is the request's service id + 0x40, the identifier
		// echoed, then the data. Four bytes of payload keeps the whole PDU
		// inside one frame, which is what a loopback can carry (see the module
		// docs on why multi-frame cannot be self-tested).
		let response = [0x62, (DID_VIN >> 8) as u8, DID_VIN as u8, b'S', b'E', b'L', b'F'];
		info!("[3/3 uds]   answering itself with {response:02X?}");
		self.iso.send(&response).await?;
		self.iso.recv(timeout).await
	}
}
