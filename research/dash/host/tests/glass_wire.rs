//! The glass's driver on the host: what goes down the wire to the SSD1322, byte by byte, and
//! which rows a frame sends.
//!
//! `crates/dash/vag-dash-fw/src/ssd1322.rs` compiled as it is — the firmware cannot be built
//! for the host — over a bus and three pins that write into one shared log. The link is
//! write-only on the board too, so this log is the only place the wire can be read at all.

#[path = "../../../../crates/dash/vag-dash-fw/src/ssd1322.rs"]
mod ssd1322;

use std::cell::RefCell;
use std::convert::Infallible;
use std::rc::Rc;

use ssd1322::{CLOCK_HZ, FULL, HEIGHT, ROW_BITS, ROW_BYTES, Shown, Ssd1322, widen};

/// One thing seen on the wire, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Seen {
	/// A byte clocked out, with the levels of `D/C` and `CS` as it went.
	Byte {
		byte: u8,
		data: bool,
		selected: bool,
	},
	Reset(bool),
	Wait(u32),
}

#[derive(Default)]
struct Wire {
	seen: Vec<Seen>,
	dc: bool,
	cs: bool,
}

type Log = Rc<RefCell<Wire>>;

struct Bus(Log);

impl embedded_hal_async::spi::ErrorType for Bus {
	type Error = Infallible;
}

impl embedded_hal_async::spi::SpiBus for Bus {
	async fn read(&mut self, _: &mut [u8]) -> Result<(), Infallible> {
		unreachable!("the glass is never read")
	}

	async fn write(&mut self, words: &[u8]) -> Result<(), Infallible> {
		let mut wire = self.0.borrow_mut();
		let (data, selected) = (wire.dc, !wire.cs);
		wire.seen.extend(words.iter().map(|byte| Seen::Byte { byte: *byte, data, selected }));
		Ok(())
	}

	async fn transfer(&mut self, _: &mut [u8], _: &[u8]) -> Result<(), Infallible> {
		unreachable!("the glass is never read")
	}

	async fn transfer_in_place(&mut self, _: &mut [u8]) -> Result<(), Infallible> {
		unreachable!("the glass is never read")
	}

	async fn flush(&mut self) -> Result<(), Infallible> {
		Ok(())
	}
}

enum Which {
	Dc,
	Cs,
	Res,
}

struct Pin(Log, Which);

impl embedded_hal::digital::ErrorType for Pin {
	type Error = Infallible;
}

impl Pin {
	fn set(&mut self, high: bool) {
		let mut wire = self.0.borrow_mut();
		match self.1 {
			Which::Dc => wire.dc = high,
			Which::Cs => wire.cs = high,
			Which::Res => wire.seen.push(Seen::Reset(high)),
		}
	}
}

impl embedded_hal::digital::OutputPin for Pin {
	fn set_low(&mut self) -> Result<(), Infallible> {
		self.set(false);
		Ok(())
	}

	fn set_high(&mut self) -> Result<(), Infallible> {
		self.set(true);
		Ok(())
	}
}

struct Clock(Log);

impl embedded_hal_async::delay::DelayNs for Clock {
	async fn delay_ns(&mut self, ns: u32) {
		self.0.borrow_mut().seen.push(Seen::Wait(ns));
	}
}

fn glass() -> (Ssd1322<Bus, Pin>, Log) {
	// `CS` starts high, as on the board: a byte sent before the driver selects the chip must
	// read as unselected, or the check in `bytes` proves nothing for a lone command.
	let log: Log = Rc::new(RefCell::new(Wire { cs: true, ..Wire::default() }));
	let glass = Ssd1322::new(
		Bus(log.clone()),
		Pin(log.clone(), Which::Dc),
		Pin(log.clone(), Which::Cs),
		Pin(log.clone(), Which::Res),
	);
	(glass, log)
}

fn taken(log: &Log) -> Vec<Seen> {
	std::mem::take(&mut log.borrow_mut().seen)
}

/// The bytes of a log, as `(byte, is data)`, with every one of them checked to have gone
/// out with the chip selected.
fn bytes(seen: &[Seen]) -> Vec<(u8, bool)> {
	seen
		.iter()
		.filter_map(|s| match s {
			Seen::Byte { byte, data, selected } => {
				assert!(selected, "byte {byte:02X} went out with CS high");
				Some((*byte, *data))
			}
			_ => None,
		})
		.collect()
}

fn run<F: Future>(future: F) -> F::Output {
	tokio::runtime::Builder::new_current_thread().build().expect("a runtime").block_on(future)
}

#[test]
fn a_lit_pixel_is_its_nibble_and_the_left_one_is_the_high_half() {
	let mut bits = [0u8; ROW_BITS];
	bits[0] = 0b1001_0000; // pixels 0 and 3
	bits[ROW_BITS - 1] = 0b0000_0001; // the last pixel of the row
	let mut wide = [0xAAu8; ROW_BYTES];
	widen(&bits, FULL, &mut wide);
	assert_eq!(&wide[..4], &[0xF0, 0x0F, 0x00, 0x00]);
	assert_eq!(wide[ROW_BYTES - 1], 0x0F);
	assert!(
		wide[4..ROW_BYTES - 1].iter().all(|b| *b == 0),
		"unlit pixels are 0, whatever the buffer held"
	);

	widen(&bits, 0x03, &mut wide);
	assert_eq!(&wide[..2], &[0x30, 0x03], "the ink is the grey level");
}

#[test]
fn rows_go_to_the_glass_window_with_arguments_as_data() {
	let (mut glass, log) = glass();
	let lit = [0xFFu8; ROW_BITS];
	run(glass.rows(10, [&lit, &lit], FULL)).expect("the fake bus cannot fail");
	let wire = bytes(&taken(&log));
	// Columns 28..=91 (the glass on the controller's 480), rows from 10, then the RAM write.
	let head = [
		(0x15, false),
		(0x1C, true),
		(0x5B, true),
		(0x75, false),
		(10, true),
		(63, true),
		(0x5C, false),
	];
	assert_eq!(&wire[..head.len()], &head);
	let data = &wire[head.len()..];
	assert_eq!(data.len(), 2 * ROW_BYTES, "two rows, four bits a pixel");
	assert!(data.iter().all(|(byte, is_data)| *byte == 0xFF && *is_data));
}

#[test]
fn rows_past_the_bottom_edge_are_not_sent() {
	let (mut glass, log) = glass();
	let lit = [0xFFu8; ROW_BITS];
	run(glass.rows(HEIGHT - 1, [&lit, &lit, &lit], FULL)).expect("the fake bus cannot fail");
	let data = bytes(&taken(&log)).into_iter().filter(|(_, is_data)| *is_data).count();
	assert_eq!(data, 2 + 2 + ROW_BYTES, "the two windows' arguments and the one row that fits");

	run(glass.rows(HEIGHT, [&lit], FULL)).expect("the fake bus cannot fail");
	assert!(taken(&log).is_empty(), "a first row past the glass sends nothing at all");
}

#[test]
fn start_resets_unlocks_blanks_and_only_then_lights() {
	let (mut glass, log) = glass();
	run(glass.start(&mut Clock(log.clone()), false)).expect("the fake bus cannot fail");
	let seen = taken(&log);

	// The reset pulse comes before any byte.
	let first_byte = seen.iter().position(|s| matches!(s, Seen::Byte { .. })).expect("bytes were sent");
	let resets: Vec<bool> = seen[..first_byte]
		.iter()
		.filter_map(|s| if let Seen::Reset(high) = s { Some(*high) } else { None })
		.collect();
	assert_eq!(resets, [true, false, true], "RES goes low and comes back before the first command");

	let wire = bytes(&seen);
	assert_eq!(&wire[..3], &[(0xFD, false), (0x12, true), (0xAE, false)], "unlock, then display off");
	let remap = wire.windows(3).position(|w| w[0] == (0xA0, false)).expect("the remap command is sent");
	assert_eq!(&wire[remap..remap + 3], &[(0xA0, false), (0x14, true), (0x11, true)], "upright");
	let clock = wire.windows(2).position(|w| w[0] == (0xB3, false)).expect("the scan clock is set");
	assert_eq!(wire[clock + 1], (0xF1, true), "the clock the owner picked against a camera");
	// A whole dark picture goes out before the display is switched on, and nothing lit.
	let on = wire.iter().position(|b| *b == (0xAF, false)).expect("display on is sent");
	let write = wire.iter().position(|b| *b == (0x5C, false)).expect("the RAM is written");
	assert!(write < on, "the glass is blanked before it is lit");
	let picture = &wire[write + 1..on];
	assert_eq!(picture.len(), HEIGHT * ROW_BYTES);
	assert!(picture.iter().all(|(byte, is_data)| *byte == 0 && *is_data));
}

#[test]
fn brightness_is_the_contrast_current_and_takes_the_whole_byte() {
	let (mut glass, log) = glass();
	run(glass.brightness(200)).expect("the fake bus cannot fail");
	assert_eq!(bytes(&taken(&log)), [(0xC1, false), (200, true)]);
}

#[test]
fn turned_is_the_other_remap() {
	let (mut glass, log) = glass();
	run(glass.start(&mut Clock(log.clone()), true)).expect("the fake bus cannot fail");
	let wire = bytes(&taken(&log));
	let remap = wire.windows(3).position(|w| w[0] == (0xA0, false)).expect("the remap command is sent");
	assert_eq!(&wire[remap..remap + 3], &[(0xA0, false), (0x06, true), (0x11, true)]);
}

/// The panel task sends the picture inside its 200 ms frame, between everything else the
/// board does: a clock slow enough to fill that frame would starve the rest.
#[test]
fn a_whole_picture_takes_a_small_part_of_a_frame() {
	let bits = (HEIGHT * ROW_BYTES * 8) as u32;
	let ms = bits * 1000 / CLOCK_HZ;
	assert!(ms <= 20, "a whole picture is {ms} ms on the wire at {CLOCK_HZ} Hz");
}

fn picture() -> Vec<[u8; ROW_BITS]> {
	vec![[0u8; ROW_BITS]; HEIGHT]
}

#[test]
fn the_first_picture_goes_whole_and_an_unchanged_one_not_at_all() {
	let mut shown = Shown::new();
	let frame = picture();
	assert_eq!(shown.changed(&frame), Some((0, HEIGHT - 1)), "nothing is known of the glass yet");
	assert_eq!(shown.changed(&frame), None);
}

#[test]
fn a_change_is_the_span_from_its_first_row_to_its_last() {
	let mut shown = Shown::new();
	let mut frame = picture();
	shown.changed(&frame);
	frame[7][0] = 1;
	frame[40][31] = 0x80;
	assert_eq!(shown.changed(&frame), Some((7, 40)));
	assert_eq!(shown.changed(&frame), None, "and it is taken as sent");
	frame[7][0] = 0;
	assert_eq!(shown.changed(&frame), Some((7, 7)), "a row going back is a change too");
}

#[test]
fn forgetting_sends_everything_again() {
	let mut shown = Shown::new();
	let frame = picture();
	shown.changed(&frame);
	shown.forget();
	assert_eq!(shown.changed(&frame), Some((0, HEIGHT - 1)));
}
