//! The `dash` image's USB console, from the board's side: its log lines, and what the
//! USB-Serial-JTAG peripheral says about the host.
//!
//! **One task writes to the port** (`usb_writer_task` in `dash.rs`), because a line
//! written from anywhere else lands in the middle of a frame and destroys it —
//! measured, not feared: logging the render report once per frame corrupted 30 frames
//! out of 30. So the log is not printed: [`init_logger`] installs a logger that queues
//! whole lines in [`LINES`], and the writer puts each on the wire between frames. A
//! panic still prints straight to the peripheral through esp-println (`health.rs`): it
//! happens on the way down, and a torn frame then is the least of it. So do the
//! boot-time `[health]` lines, which are printed before any task that writes exists.
//!
//! What esp-hal `=1.0.0-rc.0` does not offer is any word about the host: its async
//! write waits for the endpoint to drain, which with no host reading is forever, and
//! it has no connect or disconnect event. The peripheral's own registers say both, and
//! are read here directly: [`sof_frame`] (the host's start-of-frame counter, which
//! advances every millisecond while a host is attached) and [`packet_taken`].

use core::fmt::{self, Write as _};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use esp_hal::peripherals::USB_DEVICE;

/// The longest log line kept; the rest of a longer one is cut.
pub const LINE_MAX: usize = 256;

/// One queued log line, without its line ending.
pub type LogLine = heapless::String<LINE_MAX>;

/// Lines for the laptop, queued rather than printed. Full means dropped: a log line is
/// never worth stalling the thing it describes.
pub static LINES: Channel<CriticalSectionRawMutex, LogLine, 16> = Channel::new();

/// Queue one line. Cut at [`LINE_MAX`] rather than lost when it is longer.
pub fn say(args: fmt::Arguments<'_>) {
	let mut line = LogLine::new();
	let _ = Cut(&mut line).write_fmt(args);
	let _ = LINES.try_send(line);
}

/// Writes what fits and quietly drops the rest.
struct Cut<'a>(&'a mut LogLine);

impl fmt::Write for Cut<'_> {
	fn write_str(&mut self, s: &str) -> fmt::Result {
		for c in s.chars() {
			if self.0.push(c).is_err() {
				break;
			}
		}
		Ok(())
	}
}

struct QueueLogger;

impl log::Log for QueueLogger {
	fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
		metadata.level() <= log::max_level()
	}

	fn log(&self, record: &log::Record<'_>) {
		if self.enabled(record.metadata()) {
			say(format_args!("{} - {}", record.level(), record.args()));
		}
	}

	fn flush(&self) {}
}

static LOGGER: QueueLogger = QueueLogger;

/// Install the queueing logger, at the level `ESP_LOG` names at build time
/// (`.cargo/config.toml` sets `warn`), as `esp_println::logger::init_logger_from_env`
/// did. Call once, first thing in `main`.
pub fn init_logger() {
	let level = match option_env!("ESP_LOG") {
		Some(level) if level.eq_ignore_ascii_case("off") => log::LevelFilter::Off,
		Some(level) if level.eq_ignore_ascii_case("error") => log::LevelFilter::Error,
		Some(level) if level.eq_ignore_ascii_case("warn") => log::LevelFilter::Warn,
		Some(level) if level.eq_ignore_ascii_case("debug") => log::LevelFilter::Debug,
		Some(level) if level.eq_ignore_ascii_case("trace") => log::LevelFilter::Trace,
		_ => log::LevelFilter::Info,
	};
	// SAFETY: the racy setters are the only ones this core has (no compare-and-swap),
	// and they are sound when nothing else logs or sets a logger concurrently — which
	// holds here: this runs once, from `main`, before any task or interrupt handler
	// that logs has been started.
	unsafe {
		let _ = log::set_logger_racy(&LOGGER);
		log::set_max_level_racy(level);
	}
}

/// The frame index of the last start-of-frame packet the host sent
/// (`USB_SERIAL_JTAG_FRAM_NUM_REG`, 11 bits). A host attached to a full-speed device
/// sends one every millisecond, so an index that has not moved for tens of
/// milliseconds is a cable pulled, or a host asleep.
pub fn sof_frame() -> u16 {
	USB_DEVICE::regs().fram_num().read().sof_frame_index().bits()
}

/// Whether a packet from the host sits in the receive FIFO: `SERIAL_OUT_EP_DATA_AVAIL`.
/// A read only; nothing is written, so it cannot race the driver's interrupt handler.
pub fn rx_waiting() -> bool {
	USB_DEVICE::regs().ep1_conf().read().serial_out_ep_data_avail().bit_is_set()
}

/// Whether the host has taken the last packet written: `SERIAL_IN_EP_DATA_FREE`, which
/// goes low when a packet is committed and high again once the host has read it. A
/// write while it is low overruns the 64-byte endpoint buffer, so a writer that gave up
/// waiting looks here before writing again.
pub fn packet_taken() -> bool {
	USB_DEVICE::regs().ep1_conf().read().serial_in_ep_data_free().bit_is_set()
}
