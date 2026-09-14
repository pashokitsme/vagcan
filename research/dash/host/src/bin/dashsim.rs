//! `dashsim` — be the panel and the buttons, so the board can be the device.
//!
//! The board renders a real 256×64 frame with the real `vag-dash` code and
//! sends it over the USB serial it is already flashed and logged through. This
//! draws it in the terminal and sends button presses back. Nothing here decides
//! anything: no layout, no page order, no formatting. If this program were
//! clever, the thing being tested would be this program.
//!
//! Two ways to draw pixels in a terminal, both of which keep the aspect ratio
//! square because a terminal cell is about twice as tall as it is wide:
//!
//! * **half blocks** (`▀ ▄ █`) — one pixel per column, two per row, so 256×64
//!   wants 256 columns. Crisp, and the closest thing to seeing the panel.
//! * **braille** (`⠀`–`⣿`) — two pixels per column, four per row, so the same
//!   panel fits in **128 columns**. Denser and less crisp, and the only option
//!   in a window that is not 258 columns wide.
//!
//! The mode is chosen from the terminal width each time it draws, so widening
//! the window switches back on its own; `b` forces braille either way.
//!
//! One press is one press, at this end too. A held key auto-repeats, and on
//! 2026-09-13 every repeat went down the wire as `BTN S` — a dozen page turns
//! for one keystroke. Where the terminal speaks the kitty keyboard protocol
//! (kitty, WezTerm, foot, Ghostty, iTerm2 3.5+) it is asked to *report* repeats,
//! which then arrive as `KeyEventKind::Repeat` and are dropped; where it does
//! not (Terminal.app), a second press inside the board's own
//! [`PRESS_GAP_MS`] is taken for a repeat. The board gates the same way, so
//! neither end can reproduce the burst alone.

use anyhow::{Context, Result, bail};
use crossterm::event::{
	self, Event, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::{execute, terminal};
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use vag_dash_host::frame::{self, Bitmap};
use vag_dash_render::button::PRESS_GAP_MS;

const BAUD: u32 = 115_200;
/// How many of the board's log lines to keep under the panel. Enough to see
/// what just happened, few enough that the panel stays on screen.
const LOG_LINES: usize = 8;

/// How pixels become characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
	HalfBlocks,
	Braille,
}

impl Mode {
	/// Terminal columns one panel row needs, borders included.
	fn columns_for(self, width: u32) -> u32 {
		match self {
			Mode::HalfBlocks => width + 2,
			Mode::Braille => width.div_ceil(2) + 2,
		}
	}
}

/// Renders the bitmap as terminal rows, without borders.
///
/// Braille packs a 2×4 block of pixels into one code point. The dot numbering
/// is the historical one and is *not* row-major — dots 1,2,3 run down the left
/// column, 4,5,6 down the right, and 7,8 are the fourth row added later for
/// computing. Hence the table rather than a shift.
fn rows(bitmap: &Bitmap, mode: Mode) -> Vec<String> {
	match mode {
		Mode::HalfBlocks => (0..bitmap.height)
			.step_by(2)
			.map(|y| {
				(0..bitmap.width)
					.map(|x| match (bitmap.get(x, y), bitmap.get(x, y + 1)) {
						(true, true) => '█',
						(true, false) => '▀',
						(false, true) => '▄',
						(false, false) => ' ',
					})
					.collect()
			})
			.collect(),
		Mode::Braille => {
			const DOTS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
			(0..bitmap.height)
				.step_by(4)
				.map(|y| {
					(0..bitmap.width)
						.step_by(2)
						.map(|x| {
							let mut bits = 0u8;
							for (dy, row) in DOTS.iter().enumerate() {
								for (dx, dot) in row.iter().enumerate() {
									if bitmap.get(x + dx as u32, y + dy as u32) {
										bits |= dot;
									}
								}
							}
							char::from_u32(0x2800 + u32::from(bits)).unwrap_or(' ')
						})
						.collect()
				})
				.collect()
		}
	}
}

enum FromBoard {
	Frame(Box<Bitmap>),
	Log(String),
	Gone(String),
}

/// What one read of the board's stream came to.
enum Heard {
	/// The port closed.
	End,
	/// A line with nothing for the panel.
	Nothing,
	Said(FromBoard),
}

/// One line off the board, kept in `line` across reads that time out.
///
/// The port carries more than text: the board's link frames (`vag_uds_transport::link`)
/// for a host on the cable, and the filler its writer closes a stalled frame with — which a
/// host killed mid-session leaves for the next one to read. So the line is bytes, not a
/// `String`: `read_line` fails on the first byte past ASCII that is not UTF-8, and that
/// failure used to end this reader for good. A line holding a NUL is the link's and is
/// skipped; any other is read lossily.
fn read_board<R: BufRead>(reader: &mut R, line: &mut Vec<u8>) -> std::io::Result<Heard> {
	// A read that times out keeps what it read in `line` and says so; the next call goes on.
	if reader.read_until(b'\n', line)? == 0 {
		return Ok(Heard::End);
	}
	// A whole line, or at the end of the stream what there is of the last one.
	let heard = if line.contains(&0) {
		Heard::Nothing
	} else {
		said(&String::from_utf8_lossy(line))
	};
	line.clear();
	Ok(heard)
}

/// A whole text line from the board: a panel frame, or a log line.
fn said(text: &str) -> Heard {
	let trimmed = text.trim_end_matches(['\r', '\n']);
	Heard::Said(match frame::decode(trimmed) {
		Ok(bitmap) => FromBoard::Frame(Box::new(bitmap)),
		Err(frame::DecodeError::NotAFrame) => FromBoard::Log(strip_ansi(trimmed)),
		// A malformed frame is worth seeing, not hiding: it
		// means the two encoders have drifted apart.
		Err(e) => FromBoard::Log(format!("[bad frame] {e}")),
	})
}

/// What the arguments ask for.
enum Invocation {
	Demo,
	/// `--preview DIR`; `None` when the directory is missing.
	Preview(Option<String>),
	/// `--snap FILE` / `--hello-snap FILE`: one frame off the board into a PNG, after a
	/// Hello when `hello`; `None` when the file is missing.
	Snap {
		file: Option<String>,
		hello: bool,
	},
	List,
	Help,
	Unknown(String),
	Port(String),
	Guess,
}

fn parse(first: Option<&str>, second: Option<&str>) -> Invocation {
	match first {
		Some("--demo") => Invocation::Demo,
		Some("--preview") => Invocation::Preview(second.map(str::to_string)),
		Some("--snap") => Invocation::Snap {
			file: second.map(str::to_string),
			hello: false,
		},
		Some("--hello-snap") => Invocation::Snap {
			file: second.map(str::to_string),
			hello: true,
		},
		Some("--list") => Invocation::List,
		Some("--help" | "-h") => Invocation::Help,
		// Nothing else starts with a dash; a port path never does.
		Some(flag) if flag.starts_with('-') => Invocation::Unknown(flag.to_string()),
		Some(name) => Invocation::Port(name.to_string()),
		None => Invocation::Guess,
	}
}

const USAGE: &str = "\
dashsim — be the panel and the buttons for a board running the `dash` image

usage:
  dashsim [PORT]          open PORT, or the one ESP32 board (USB vendor 303a) if omitted
  dashsim --list          list the serial ports
  dashsim --demo          draw one synthetic frame and exit (no board needed)
  dashsim --preview DIR   render the layout preview scenarios (placeholder values) to
                          PNGs in DIR and print each one's report (no board needed)
  dashsim --snap FILE     write the panel as the board draws it to the PNG FILE; the
                          frame is the third after the port opens, and the board's log
                          lines meanwhile go to stderr
  dashsim --hello-snap FILE
                          the same after saying Hello on the link first, as a host on
                          the cable does — the frame then shows the USB link icon
  dashsim --help          this text

keys: space = short press, L = long press, b = braille, q = quit";

fn main() -> Result<()> {
	let args: Vec<String> = std::env::args().skip(1).take(2).collect();
	match parse(args.first().map(String::as_str), args.get(1).map(String::as_str)) {
		Invocation::Demo => demo(),
		Invocation::Preview(Some(dir)) => {
			for (name, report) in preview::write_all(std::path::Path::new(&dir))? {
				println!("{name:<44} {report:?}");
			}
			Ok(())
		}
		Invocation::Preview(None) => bail!("--preview needs a directory to write the PNGs to\n\n{USAGE}"),
		Invocation::Snap { file: Some(file), hello } => snap(&guess_port()?, std::path::Path::new(&file), hello),
		Invocation::Snap { file: None, .. } => bail!("--snap and --hello-snap need the PNG file to write\n\n{USAGE}"),
		Invocation::List => list_ports(),
		Invocation::Help => {
			println!("{USAGE}");
			Ok(())
		}
		Invocation::Unknown(flag) => bail!("unknown option {flag}\n\n{USAGE}"),
		Invocation::Port(name) => run(&name),
		Invocation::Guess => run(&guess_port()?),
	}
}

fn list_ports() -> Result<()> {
	for port in serialport::available_ports()? {
		println!("{}", port.port_name);
	}
	Ok(())
}

/// Espressif's USB vendor id. The C3's native USB (its USB-Serial-JTAG) enumerates
/// under it whatever image the board runs; a property of the chip, the same id
/// `vag-uds-can`'s adapter listing knows the board by.
const ESPRESSIF_VID: u16 = 0x303a;

/// Picking the board beats making every run start with a path nobody remembers.
fn guess_port() -> Result<String> {
	pick_board(&serialport::available_ports()?)
}

/// The one port under [`ESPRESSIF_VID`]. Not "the first `usbmodem`": a CANable
/// is a `usbmodem` too, and on a desk with both it lists first — which left this
/// program waiting forever for a board on the wrong port.
fn pick_board(ports: &[serialport::SerialPortInfo]) -> Result<String> {
	let boards: Vec<&str> = ports
		.iter()
		.filter(|p| matches!(&p.port_type, serialport::SerialPortType::UsbPort(usb) if usb.vid == ESPRESSIF_VID))
		// macOS lists a `tty.*` and a `cu.*` node per device; `cu.*` is the one to open.
		.filter(|p| !p.port_name.contains("/tty."))
		.map(|p| p.port_name.as_str())
		.collect();
	match boards.as_slice() {
		[one] => Ok((*one).to_string()),
		[] => bail!("no ESP32 board found (USB vendor {ESPRESSIF_VID:04x}) — plug it in, pass its port explicitly, or --list to see the ports"),
		several => bail!("several ESP32 boards found — pass one explicitly:\n  {}", several.join("\n  ")),
	}
}

/// Frames skipped before the one written: the board draws one every 200 ms, so the
/// third is drawn at least 400 ms after the port opened — after the Hello, when one
/// was sent, has reached the board.
const SNAP_SKIP: usize = 2;

/// How long a snap waits for its frame. A board in adapter mode sends none.
const SNAP_WAIT: Duration = Duration::from_secs(5);

/// Bytes past which a line with no `\n` is thrown away: twice the buffer the firmware keeps
/// its `FRAME` line in (`FRAME_LINE` in `vag-dash-fw`'s `dash.rs`), so no panel line is cut.
const SNAP_LINE_MAX: usize = 2 * (256 * 64 / 8 * 2 + 32);

/// One frame off the board, as `--preview` writes its scenarios.
fn snap(port_name: &str, file: &std::path::Path, hello: bool) -> Result<()> {
	let mut port = serialport::new(port_name, BAUD)
		.timeout(Duration::from_millis(200))
		.open()
		.with_context(|| format!("opening {port_name}"))?;
	if hello {
		let bytes = vag_uds_transport::link::encode(&vag_uds_transport::link::Message::Hello)?;
		port.write_all(&bytes).context("saying Hello")?;
	}
	// Read in chunks and cut lines here, not with `read_until`: a board in adapter mode ends
	// its lines with `\r` alone, and on a busy bus a `read_until(b'\n')` gets bytes inside
	// every timeout and never returns — past the deadline and without bound.
	let mut chunk = [0u8; 1024];
	let mut pending: Vec<u8> = Vec::new();
	let deadline = Instant::now() + SNAP_WAIT;
	let mut seen = 0;
	while Instant::now() < deadline {
		let n = match port.read(&mut chunk) {
			Ok(0) => bail!("{port_name} closed before a frame came"),
			Ok(n) => n,
			Err(e) if e.kind() == std::io::ErrorKind::TimedOut => continue,
			Err(e) => return Err(e).with_context(|| format!("reading {port_name}")),
		};
		pending.extend_from_slice(&chunk[..n]);
		while let Some(end) = pending.iter().position(|&b| b == b'\n') {
			let line: Vec<u8> = pending.drain(..=end).collect();
			// A line holding a NUL is the link's, as in `read_board`.
			if line.contains(&0) {
				continue;
			}
			match said(&String::from_utf8_lossy(&line)) {
				Heard::Said(FromBoard::Frame(_)) if seen < SNAP_SKIP => seen += 1,
				Heard::Said(FromBoard::Frame(bitmap)) => {
					let mut canvas = preview::Canvas::new(embedded_graphics::prelude::Size::new(bitmap.width, bitmap.height));
					canvas.lit.clone_from(&bitmap.pixels);
					let out = std::fs::File::create(file).with_context(|| format!("creating {}", file.display()))?;
					preview::encode_png(&canvas, std::io::BufWriter::new(out))?;
					println!("{}", file.display());
					return Ok(());
				}
				Heard::Said(FromBoard::Log(text)) => eprintln!("{text}"),
				_ => {}
			}
		}
		// No `\n` in more than any panel line holds: not the panel's stream.
		if pending.len() > SNAP_LINE_MAX {
			pending.clear();
		}
	}
	bail!(
		"no frame from {port_name} in {} s — is the board in adapter mode, or not on the dash image?",
		SNAP_WAIT.as_secs()
	)
}

fn run(port_name: &str) -> Result<()> {
	let port = serialport::new(port_name, BAUD)
		.timeout(Duration::from_millis(200))
		.open()
		.with_context(|| format!("opening {port_name}"))?;
	let mut writer = port.try_clone().context("cloning the port for writing")?;

	let (tx, rx) = mpsc::channel();
	std::thread::spawn(move || {
		let mut reader = BufReader::new(port);
		let mut line = Vec::new();
		loop {
			match read_board(&mut reader, &mut line) {
				Ok(Heard::End) => {
					let _ = tx.send(FromBoard::Gone("port closed".into()));
					return;
				}
				Ok(Heard::Nothing) => {}
				Ok(Heard::Said(message)) => {
					if tx.send(message).is_err() {
						return;
					}
				}
				// A read timeout is not an error here: the board is simply quiet.
				Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
				Err(e) => {
					let _ = tx.send(FromBoard::Gone(e.to_string()));
					return;
				}
			}
		}
	});

	terminal::enable_raw_mode()?;
	let mut out = std::io::stdout();
	execute!(out, terminal::EnterAlternateScreen, crossterm::cursor::Hide)?;

	// Asked in raw mode, as crossterm wants, and before the event loop starts
	// reading — the query is answered on the same input.
	let repeats_reported = terminal::supports_keyboard_enhancement().unwrap_or(false);
	if repeats_reported {
		execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::REPORT_EVENT_TYPES))?;
	}

	let result = event_loop(&mut out, &mut writer, &rx, repeats_reported, port_name);

	if repeats_reported {
		execute!(out, PopKeyboardEnhancementFlags)?;
	}
	execute!(out, crossterm::cursor::Show, terminal::LeaveAlternateScreen)?;
	terminal::disable_raw_mode()?;
	result
}

fn event_loop(
	out: &mut std::io::Stdout,
	writer: &mut Box<dyn serialport::SerialPort>,
	rx: &mpsc::Receiver<FromBoard>,
	repeats_reported: bool,
	port_name: &str,
) -> Result<()> {
	let mut logs: VecDeque<String> = VecDeque::new();
	let mut latest: Option<Bitmap> = None;
	let mut frames = 0u64;
	// The port is on the status line always: a board that never speaks is
	// most often the right program on the wrong port, and that is where to look.
	let mut status = format!("{port_name}: waiting for the board");
	// `None` means "pick whatever fits"; `b` pins it to braille.
	let mut forced: Option<Mode> = None;
	let mut redraw = true;
	let repeats = if repeats_reported {
		"key repeat: reported by the terminal"
	} else {
		"key repeat: gated, one press per 250 ms"
	};
	// When the last button line went down the wire — the gate for a terminal
	// that cannot tell a repeat from a press.
	let mut last_button: Option<Instant> = None;
	let gap = Duration::from_millis(PRESS_GAP_MS);

	loop {
		// Drain everything the board has said, then draw once. Drawing per
		// message would make the terminal the bottleneck.
		let mut dirty = false;
		while let Ok(message) = rx.try_recv() {
			dirty = true;
			match message {
				FromBoard::Frame(bitmap) => {
					frames += 1;
					status = format!("{port_name}: {}×{}, {frames} frames", bitmap.width, bitmap.height);
					latest = Some(*bitmap);
				}
				FromBoard::Log(line) => {
					if !line.trim().is_empty() {
						logs.push_back(line);
						while logs.len() > LOG_LINES {
							logs.pop_front();
						}
					}
				}
				FromBoard::Gone(why) => {
					status = format!("{port_name}: board gone: {why}");
				}
			}
		}
		if dirty || redraw {
			redraw = false;
			draw(out, latest.as_ref(), &logs, &status, repeats, forced)?;
		}

		if event::poll(Duration::from_millis(30))?
			&& let Event::Key(key) = event::read()?
		{
			// With `REPORT_EVENT_TYPES` a held key arrives as `Repeat`,
			// and a repeat is not a press. Without it everything is
			// `Press`, and the gate below does the telling.
			if key.kind != KeyEventKind::Press {
				continue;
			}
			match key.code {
				KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
				KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
				// The two gestures the device has. They are written as
				// events rather than as a held level because the board's
				// debounce belongs to the board — this is a keyboard, and
				// a keyboard cannot honestly imitate a contact bouncing.
				KeyCode::Char(' ') | KeyCode::Char('l') | KeyCode::Char('L') => {
					let repeat = !repeats_reported && last_button.is_some_and(|at| at.elapsed() < gap);
					if repeat {
						continue;
					}
					last_button = Some(Instant::now());
					let line = if key.code == KeyCode::Char(' ') { "BTN S" } else { "BTN L" };
					writeln!(writer, "{line}")?;
					writer.flush()?;
				}
				KeyCode::Char('b') | KeyCode::Char('B') => {
					forced = match forced {
						Some(Mode::Braille) => None,
						_ => Some(Mode::Braille),
					};
					redraw = true;
				}
				_ => {}
			}
		}
	}
}

fn draw(
	out: &mut std::io::Stdout,
	bitmap: Option<&Bitmap>,
	logs: &VecDeque<String>,
	status: &str,
	repeats: &str,
	forced: Option<Mode>,
) -> Result<()> {
	execute!(out, terminal::Clear(terminal::ClearType::All), crossterm::cursor::MoveTo(0, 0))?;
	let (columns, _) = terminal::size().unwrap_or((80, 24));
	let mut mode_note = "";

	match bitmap {
		Some(bitmap) => {
			// Half blocks if they fit, braille if they do not. Chosen per draw
			// rather than at start-up so resizing the window just works.
			let mode = forced.unwrap_or({
				if u32::from(columns) >= Mode::HalfBlocks.columns_for(bitmap.width) {
					Mode::HalfBlocks
				} else {
					Mode::Braille
				}
			});
			let lines = rows(bitmap, mode);
			let inner = lines.first().map(|l| l.chars().count()).unwrap_or(0);
			if u32::from(columns) < mode.columns_for(bitmap.width) {
				writeln!(
					out,
					"terminal is {columns} columns; even braille needs {}. Make the window wider.\r",
					mode.columns_for(bitmap.width)
				)?;
			} else {
				writeln!(out, "┌{}┐\r", "─".repeat(inner))?;
				for line in &lines {
					writeln!(out, "│{line}│\r")?;
				}
				writeln!(out, "└{}┘\r", "─".repeat(inner))?;
			}
			mode_note = match mode {
				Mode::HalfBlocks => "half blocks",
				Mode::Braille => "braille",
			};
		}
		None => writeln!(out, "(no frame yet)\r")?,
	}

	writeln!(
		out,
		"\r\n  {status}   ·   {mode_note}   ·   {repeats}   ·   space = short, L = long, b = braille, q = quit\r\n\r"
	)?;
	for line in logs {
		writeln!(out, "  {line}\r")?;
	}
	out.flush()?;
	Ok(())
}

/// The board's logger colours its output. In the alternate screen those codes
/// would smear, so they come off.
fn strip_ansi(text: &str) -> String {
	let mut out = String::with_capacity(text.len());
	let mut chars = text.chars();
	while let Some(c) = chars.next() {
		if c == '\x1b' {
			for c in chars.by_ref() {
				if c.is_ascii_alphabetic() {
					break;
				}
			}
		} else {
			out.push(c);
		}
	}
	out
}

/// Draws one synthetic frame and exits. Proves the renderer without a board —
/// and, when the board is connected, tells you whether a blank panel is the
/// board's fault or this program's.
fn demo() -> Result<()> {
	let (width, height) = (256u32, 64u32);
	let mut pixels = vec![false; (width * height) as usize];
	let mut set = |x: u32, y: u32| {
		if x < width && y < height {
			pixels[(y * width + x) as usize] = true;
		}
	};
	for x in 0..width {
		set(x, 0);
		set(x, height - 1);
	}
	for y in 0..height {
		set(0, y);
		set(width - 1, y);
	}
	// A ramp, so the half-block packing is visibly right rather than plausible.
	for x in 0..width {
		let h = (x * (height - 4) / width) + 2;
		set(x, h);
	}
	for y in 20..40 {
		for x in 100..160 {
			set(x, y);
		}
	}
	let bitmap = Bitmap { width, height, pixels };
	let line = frame::encode(&bitmap);
	println!("encoded frame is {} characters", line.len());
	let decoded = frame::decode(&line)?;
	let (columns, _) = terminal::size().unwrap_or((80, 24));
	let mode = if u32::from(columns) >= Mode::HalfBlocks.columns_for(width) {
		Mode::HalfBlocks
	} else {
		Mode::Braille
	};
	let lines = rows(&decoded, mode);
	let inner = lines.first().map(|l| l.chars().count()).unwrap_or(0);
	println!("┌{}┐", "─".repeat(inner));
	for line in &lines {
		println!("│{line}│");
	}
	println!("└{}┘", "─".repeat(inner));
	Ok(())
}

/// `--preview`: fixed scenarios through the real renderer into PNGs. No board, no terminal.
///
/// **These are preview scenarios with placeholder values, not readings.** Each value is
/// chosen for its shape — digit count, label length, unit — so a person can judge whether
/// the layout holds; none of it is a car's, none of it reaches the firmware or a plan, and
/// every file is named `preview-…` so a picture is never mistaken for a capture.
mod preview {
	use anyhow::{Context, Result};
	use embedded_graphics::pixelcolor::BinaryColor;
	use embedded_graphics::prelude::*;
	use std::io::Write;
	use std::path::Path;
	use vag_dash_render::frame::Adapter;
	use vag_dash_render::render::Report;
	use vag_dash_render::{Board, Cell, Deviation, Frame, Links, Rates, Theme, draw_with};

	/// The panel the board has.
	pub const PANEL: Size = Size::new(256, 64);
	/// Picture pixels per panel pixel, so a person can see the pixels.
	pub const SCALE: u32 = 4;
	pub const LIT: [u8; 3] = [0xF0, 0xF0, 0xF0];
	pub const DARK: [u8; 3] = [0x00, 0x00, 0x00];
	/// One panel pixel of it around the glass, so where the panel ends is visible.
	pub const FRAME: [u8; 3] = [0x60, 0x60, 0x60];

	/// A panel in memory.
	pub struct Canvas {
		pub size: Size,
		pub lit: Vec<bool>,
	}

	impl Canvas {
		pub fn new(size: Size) -> Self {
			Canvas {
				size,
				lit: vec![false; (size.width * size.height) as usize],
			}
		}

		fn get(&self, x: u32, y: u32) -> bool {
			self.lit[(y * self.size.width + x) as usize]
		}
	}

	impl OriginDimensions for Canvas {
		fn size(&self) -> Size {
			self.size
		}
	}

	impl DrawTarget for Canvas {
		type Color = BinaryColor;
		type Error = std::convert::Infallible;

		fn draw_iter<I>(&mut self, pixels: I) -> std::result::Result<(), Self::Error>
		where
			I: IntoIterator<Item = Pixel<BinaryColor>>,
		{
			for Pixel(p, colour) in pixels {
				if p.x >= 0 && p.y >= 0 && (p.x as u32) < self.size.width && (p.y as u32) < self.size.height {
					let i = (p.y as u32 * self.size.width + p.x as u32) as usize;
					self.lit[i] = colour.is_on();
				}
			}
			Ok(())
		}
	}

	/// The panel at [`SCALE`], inside a [`FRAME`] one panel pixel wide, as an RGB PNG.
	pub fn encode_png<W: Write>(canvas: &Canvas, out: W) -> Result<()> {
		let (w, h) = ((canvas.size.width + 2) * SCALE, (canvas.size.height + 2) * SCALE);
		let mut rgb = Vec::with_capacity((w * h * 3) as usize);
		for oy in 0..h {
			for ox in 0..w {
				let (px, py) = ((ox / SCALE) as i64 - 1, (oy / SCALE) as i64 - 1);
				let inside = (0..i64::from(canvas.size.width)).contains(&px) && (0..i64::from(canvas.size.height)).contains(&py);
				let colour = match inside {
					false => FRAME,
					true if canvas.get(px as u32, py as u32) => LIT,
					true => DARK,
				};
				rgb.extend_from_slice(&colour);
			}
		}
		let mut encoder = png::Encoder::new(out, w, h);
		encoder.set_color(png::ColorType::Rgb);
		encoder.set_depth(png::BitDepth::Eight);
		let mut writer = encoder.write_header()?;
		writer.write_image_data(&rgb)?;
		writer.finish()?;
		Ok(())
	}

	/// One scenario, drawn.
	pub struct Shot {
		pub name: String,
		pub canvas: Canvas,
		pub report: Report,
	}

	fn shot(name: &str, frame: &Frame<'_>, board: Board) -> Shot {
		let mut canvas = Canvas::new(PANEL);
		// The theme the firmware draws with (`vag-dash-fw`'s `panel_task`).
		let report = draw_with(frame, &board, &Theme::bold_mono(), &mut canvas);
		Shot {
			name: format!("preview-{name}"),
			canvas,
			report,
		}
	}

	const LINKS: [(&str, Links); 4] = [
		("none", Links::NONE),
		("usb", Links { usb: true, ble: false }),
		("ble", Links { usb: false, ble: true }),
		("usb-ble", Links { usb: true, ble: true }),
	];
	const BOTH: Links = LINKS[3].1;

	fn linked(links: Links) -> Board {
		Board { links, rates: None }
	}

	/// Every preview scenario, drawn. Placeholder values throughout — see the module.
	pub fn render_all() -> Vec<Shot> {
		let mut shots = Vec::new();

		// A values page with labels as long as a real plan's and both kinds of unit.
		let four = [
			Cell::new("ОЖ", Some(93.0), "°C", 0),
			Cell::new("НАДДУВ", Some(1.82), "bar", 2),
			Cell::new("МАСЛО", Some(104.0), "°C", 0),
			Cell::new("КОРОБКА", Some(78.0), "°C", 0),
		];
		for (tag, links) in LINKS {
			shots.push(shot(&format!("values4-links-{tag}"), &Frame::Values { cells: &four }, linked(links)));
		}

		let two = [Cell::new("ОЖ", Some(93.0), "°C", 0), Cell::new("НАДДУВ", Some(1.82), "bar", 2)];
		shots.push(shot("values2-links-usb-ble", &Frame::Values { cells: &two }, linked(BOTH)));

		// The rightmost label fits its column alone and not beside the icons.
		let long = [
			Cell::new("ОЖ", Some(93.0), "°C", 0),
			Cell::new("НАДДУВ", Some(1.82), "bar", 2),
			Cell::new("МАСЛО", Some(104.0), "°C", 0),
			Cell::new("ТЕМП.МАСЛА", Some(104.0), "°C", 0),
		];
		shots.push(shot("values4-long-label-links-usb-ble", &Frame::Values { cells: &long }, linked(BOTH)));

		// A page where two channels have a specified value behind them: boost is 0.07 bar over
		// what the engine asked for, the throttle 1.2° under it, and the gearbox's specified
		// value has not answered. Temperatures have none at all.
		let drifting = [
			Cell::new("ОЖ", Some(93.0), "°C", 0),
			Cell::new("НАДДУВ", Some(1.92), "bar", 2).with_deviation(Deviation::Value(0.07)),
			Cell::new("ДРОССЕЛЬ", Some(42.8), "°", 1).with_deviation(Deviation::Value(-1.2)),
			Cell::new("КОРОБКА", Some(78.0), "°C", 0).with_deviation(Deviation::Unknown),
		];
		shots.push(shot("values4-deviation-links-usb-ble", &Frame::Values { cells: &drifting }, linked(BOTH)));

		// A boost-shaped trace: spool, plateau, a dip, back on it.
		let samples: Vec<f32> = (0..256)
			.map(|i| {
				let t = i as f32 / 256.0;
				let spool = (1.0 - (-t * 9.0).exp()) * 1.9;
				let dip = if (0.55..0.62).contains(&t) { -1.4 } else { 0.0 };
				(spool + dip + (i as f32 * 0.7).sin() * 0.04).max(0.0)
			})
			.collect();
		let chart = Frame::Chart {
			cell: Cell::new("НАДДУВ", Some(1.82), "bar", 2),
			min: 0.0,
			max: 2.5,
			samples: &samples,
			seconds_per_sample: 0.2,
		};
		shots.push(shot("chart-boost-links-none", &chart, linked(Links::NONE)));
		shots.push(shot("chart-boost-links-usb-ble", &chart, linked(BOTH)));
		let chart_deviation = Frame::Chart {
			cell: Cell::new("НАДДУВ", Some(1.92), "bar", 2).with_deviation(Deviation::Value(0.07)),
			min: 0.0,
			max: 2.5,
			samples: &samples,
			seconds_per_sample: 0.2,
		};
		shots.push(shot("chart-boost-deviation-links-usb-ble", &chart_deviation, linked(BOTH)));

		// The adapter screen.
		let adapter = |kbit, listen_only, rx, tx, errors| Adapter {
			kbit,
			listen_only,
			rx,
			tx,
			errors,
		};
		let rates = |tx_bps, rx_bps| Some(Rates { tx_bps, rx_bps });
		let screens = [
			(
				"adapter-500-normal-traffic",
				adapter(Some(500), false, 1_284_311, 9_402, 3),
				rates(12_400, 380_200),
			),
			("adapter-500-listen-only", adapter(Some(500), true, 431_907, 0, 0), rates(0, 214_600)),
			("adapter-closed", adapter(None, false, 0, 0, 0), rates(0, 0)),
		];
		for (name, state, rates) in screens {
			shots.push(shot(name, &Frame::Adapter(state), Board { links: Links::NONE, rates }));
		}
		shots
	}

	/// Render every scenario into `dir` as `<name>.png`; each name with its report.
	pub fn write_all(dir: &Path) -> Result<Vec<(String, Report)>> {
		std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
		let mut written = Vec::new();
		for shot in render_all() {
			let path = dir.join(format!("{}.png", shot.name));
			let file = std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
			encode_png(&shot.canvas, std::io::BufWriter::new(file)).with_context(|| format!("writing {}", path.display()))?;
			written.push((path.display().to_string(), shot.report));
		}
		Ok(written)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use serialport::{SerialPortInfo, SerialPortType, UsbPortInfo};

	fn usb(name: &str, vid: u16, pid: u16) -> SerialPortInfo {
		SerialPortInfo {
			port_name: name.to_string(),
			port_type: SerialPortType::UsbPort(UsbPortInfo {
				vid,
				pid,
				serial_number: None,
				manufacturer: None,
				product: None,
			}),
		}
	}

	#[test]
	fn the_board_is_picked_by_its_usb_ids_not_by_listing_order() {
		// The owner's desk: the CANable enumerates first, and it is a
		// `usbmodem` too. Taking the first one waited forever on the wrong port.
		let ports = [
			usb("/dev/cu.usbmodem206E37A148451", 0x16d0, 0x117e),
			usb("/dev/cu.usbmodem1101", 0x303a, 0x1001),
		];
		assert_eq!(pick_board(&ports).unwrap(), "/dev/cu.usbmodem1101");
	}

	#[test]
	fn no_board_is_said_rather_than_guessed() {
		let ports = [usb("/dev/cu.usbmodem206E37A148451", 0x16d0, 0x117e)];
		let err = pick_board(&ports).unwrap_err().to_string();
		assert!(err.contains("no ESP32 board"), "{err}");
	}

	#[test]
	fn several_boards_are_listed_and_none_is_picked() {
		let ports = [usb("/dev/cu.usbmodem1101", 0x303a, 0x1001), usb("/dev/cu.usbmodem2101", 0x303a, 0x1001)];
		let err = pick_board(&ports).unwrap_err().to_string();
		assert!(err.contains("/dev/cu.usbmodem1101") && err.contains("/dev/cu.usbmodem2101"), "{err}");
	}

	fn heard(reader: &mut impl BufRead, line: &mut Vec<u8>) -> String {
		match read_board(reader, line) {
			Ok(Heard::End) => "end".into(),
			Ok(Heard::Nothing) => "nothing".into(),
			Ok(Heard::Said(FromBoard::Frame(bitmap))) => format!("frame {}x{}", bitmap.width, bitmap.height),
			Ok(Heard::Said(FromBoard::Log(text))) => format!("log {text}"),
			Ok(Heard::Said(FromBoard::Gone(why))) => format!("gone {why}"),
			Err(e) => format!("error {:?}", e.kind()),
		}
	}

	#[test]
	fn link_bytes_and_bytes_past_ascii_on_the_port_do_not_end_the_reader() {
		let bitmap = Bitmap {
			width: 4,
			height: 2,
			pixels: vec![true; 8],
		};
		let mut stream = Vec::new();
		// The filler the board's writer closes a stalled frame with (`link::BROKEN_FRAME`),
		// run into the log line after it: a line with a NUL is the link's, not the panel's.
		stream.extend_from_slice(&[0x00, 0xFF, 0x00, 0x00]);
		stream.extend_from_slice(b"W - usb: the host went away\r\n");
		// A stale Reading for a host killed mid-session: its bytes past 0x7F are not UTF-8.
		stream.extend_from_slice(&[0x00, 0x05, 0x04, 0x00, 0x01, 0x00, 0x9C, 0xE8, b'\n']);
		stream.extend_from_slice(b"W - caf\xE9\r\n");
		stream.extend_from_slice(frame::encode(&bitmap).as_bytes());
		stream.extend_from_slice(b"\r\n");
		let mut reader = &stream[..];
		let mut line = Vec::new();
		let said: Vec<String> = (0..6).map(|_| heard(&mut reader, &mut line)).collect();
		assert_eq!(said, ["nothing", "nothing", "log W - caf\u{FFFD}", "frame 4x2", "end", "end"]);
	}

	/// Reads one scripted piece at a time; `None` is a read that timed out.
	struct Stutter(VecDeque<Option<&'static [u8]>>);

	impl std::io::Read for Stutter {
		fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
			match self.0.pop_front() {
				Some(Some(bytes)) => {
					buf[..bytes.len()].copy_from_slice(bytes);
					Ok(bytes.len())
				}
				Some(None) => Err(std::io::ErrorKind::TimedOut.into()),
				None => Ok(0),
			}
		}
	}

	#[test]
	fn a_line_cut_by_a_read_timeout_is_kept_whole() {
		let mut reader = BufReader::new(Stutter(VecDeque::from([Some(&b"I - hel"[..]), None, Some(&b"lo\r\n"[..])])));
		let mut line = Vec::new();
		assert_eq!(heard(&mut reader, &mut line), "error TimedOut");
		assert_eq!(heard(&mut reader, &mut line), "log I - hello");
		assert_eq!(heard(&mut reader, &mut line), "end");
	}

	#[test]
	fn help_is_usage_not_a_port_name() {
		for flag in ["--help", "-h"] {
			assert!(matches!(parse(Some(flag), None), Invocation::Help), "{flag}");
		}
		assert!(matches!(parse(Some("--bogus"), None), Invocation::Unknown(_)));
		assert!(matches!(parse(Some("/dev/cu.usbmodem1101"), None), Invocation::Port(p) if p == "/dev/cu.usbmodem1101"));
		assert!(matches!(parse(None, None), Invocation::Guess));
	}

	#[test]
	fn preview_takes_the_directory_after_it() {
		assert!(matches!(parse(Some("--preview"), Some("out")), Invocation::Preview(Some(d)) if d == "out"));
		assert!(matches!(parse(Some("--preview"), None), Invocation::Preview(None)));
		assert!(matches!(parse(Some("--snap"), Some("a.png")), Invocation::Snap { file: Some(f), hello: false } if f == "a.png"));
		assert!(matches!(
			parse(Some("--hello-snap"), Some("a.png")),
			Invocation::Snap { file: Some(_), hello: true }
		));
		assert!(matches!(parse(Some("--snap"), None), Invocation::Snap { file: None, .. }));
	}

	#[test]
	fn every_preview_is_named_as_one_and_names_are_unique() {
		let shots = preview::render_all();
		// Values ×4 links, two cells, a long label, the chart ×2, the adapter ×3.
		assert_eq!(shots.len(), 11);
		let mut names: Vec<&str> = shots.iter().map(|s| s.name.as_str()).collect();
		assert!(names.iter().all(|n| n.starts_with("preview-")), "{names:?}");
		names.sort_unstable();
		names.dedup();
		assert_eq!(names.len(), shots.len(), "a name is used twice");
		assert!(
			shots.iter().all(|s| s.canvas.lit.iter().any(|&on| on)),
			"every scenario put something on the glass"
		);
	}

	#[test]
	fn a_preview_png_is_the_panel_times_four_inside_a_one_pixel_frame() {
		let mut canvas = preview::Canvas::new(preview::PANEL);
		canvas.lit[0] = true;
		let mut bytes = Vec::new();
		preview::encode_png(&canvas, &mut bytes).unwrap();

		let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
		let mut reader = decoder.read_info().unwrap();
		let mut rgb = vec![0; reader.output_buffer_size()];
		let info = reader.next_frame(&mut rgb).unwrap();
		let scale = preview::SCALE;
		assert_eq!((info.width, info.height), ((256 + 2) * scale, (64 + 2) * scale));
		let at = |x: u32, y: u32| {
			let i = ((y * info.width + x) * 3) as usize;
			[rgb[i], rgb[i + 1], rgb[i + 2]]
		};
		assert_eq!(at(0, 0), preview::FRAME, "the frame");
		assert_eq!(at(scale, scale), preview::LIT, "panel pixel (0, 0), lit");
		assert_eq!(at(2 * scale, scale), preview::DARK, "panel pixel (1, 0), dark");
		assert_eq!(at(info.width - 1, info.height - 1), preview::FRAME);
	}
}
