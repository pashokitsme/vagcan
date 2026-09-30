//! The glass: an SSD1322 behind a 3.12″ 256×64 OLED, over 4-wire SPI.
//!
//! The panel the firmware draws is one bit a pixel ([`crate::panel::Framebuffer`]); the
//! controller stores four. A row is widened as it is sent, through 128 bytes the driver
//! holds, so the board keeps no second picture — a 4 bpp copy would be 8 KB of the one RAM
//! region. What the glass already shows is remembered as a checksum a row ([`Shown`], 256
//! bytes), which is what lets a frame send only the rows that changed.
//!
//! The module is wired for 4-wire SPI by two 0 Ω links (`BS1` = `BS0` = 0; the owner's
//! module came as `80XX` and had one moved, 2026-09-30). In that mode a command's byte goes
//! with `D/C` low and **its arguments with `D/C` high** — unlike the SSD1306 family, where
//! arguments are commands too. The link is write-only: nothing is ever read back, so a
//! missing panel cannot be told from a present one.
//!
//! Nothing here names the chip's HAL: the bus, the pins and the delay are `embedded-hal`
//! traits, which is what lets `research/dash/host` compile this file for the laptop and
//! read what goes down the wire.

use embedded_hal::digital::OutputPin;
use embedded_hal_async::delay::DelayNs;
use embedded_hal_async::spi::SpiBus;

/// The glass, in pixels.
pub const WIDTH: usize = 256;
pub const HEIGHT: usize = 64;
/// One row as the framebuffer holds it: a bit a pixel, most significant bit leftmost.
pub const ROW_BITS: usize = WIDTH / 8;
/// One row on the wire: four bits a pixel, the left pixel in the high half.
pub const ROW_BYTES: usize = WIDTH / 2;
/// The brightest of the controller's sixteen grey levels.
pub const FULL: u8 = 0x0F;
/// The bus clock both images run the glass at. Well inside what the controller takes, and
/// slow enough for a bench of loose wires; a whole picture is 8 KB, 16 ms at this rate.
pub const CLOCK_HZ: u32 = 4_000_000;

/// The controller drives 480 columns and this glass is bonded to the middle 256 of them. A
/// column address counts four pixels, so the glass is addresses 28..=91 — true of every
/// 256×64 SSD1322 module, a property of the panel and not of one board.
const COLUMN_FIRST: u8 = 0x1C;
const COLUMN_LAST: u8 = 0x5B;

// The controller's commands used here (SSD1322 datasheet, command table).
const SET_COLUMNS: u8 = 0x15;
const WRITE_RAM: u8 = 0x5C;
const SET_ROWS: u8 = 0x75;
const SET_REMAP: u8 = 0xA0;
const SCAN_CLOCK: u8 = 0xB3;
const CONTRAST_CURRENT: u8 = 0xC1;
const DISPLAY_ON: u8 = 0xAF;
const MASTER_CURRENT: u8 = 0xC7;

/// `SET_REMAP`'s first argument, as the glass hangs: nibble remap on (the left pixel is the
/// high half of a byte) and the rows scanned so that row 0 is at the top with the flex cable
/// at the bottom edge.
const REMAP_UPRIGHT: u8 = 0x14;
/// The same picture turned half a turn, for a housing that holds the module flex-up.
const REMAP_TURNED: u8 = 0x06;

/// What the controller is told after a reset, in order: a command and its arguments. The
/// values are the ones the panel's maker publishes for this glass (the 256×64 3.12″ module's
/// own initialisation listing, which u8g2's driver for it repeats), but for the scan clock;
/// `SET_REMAP` is sent apart, because it carries the orientation.
const INIT: &[(u8, &[u8])] = &[
	(0xFD, &[0x12]), // unlock the command set
	(0xAE, &[]),     // display off while it is set up
	// The scan clock: the oscillator in the high half (faster with a higher number), the
	// divider in the low half (by 2 to that power). Not the maker's `0x91`, which bands
	// through a camera: the owner compared `91`, `D1`, `F1` and `F0` on 2026-09-30 and took
	// `F1`, the fastest oscillator still divided by 2; `F0` was visibly dimmer.
	(SCAN_CLOCK, &[0xF1]),
	(0xCA, &[0x3F]),       // multiplex ratio: 64 rows
	(0xA2, &[0x00]),       // display offset
	(0xA1, &[0x00]),       // start line
	(0xB5, &[0x00]),       // the controller's GPIO pins: off
	(0xAB, &[0x01]),       // the internal regulator for the logic supply
	(0xB4, &[0xA0, 0xFD]), // display enhancement A: external VSL, better low greys
	(CONTRAST_CURRENT, &[0x9F]),
	(MASTER_CURRENT, &[FULL]),
	(0xB9, &[]),           // the default linear grey table
	(0xB1, &[0xE2]),       // phase lengths
	(0xD1, &[0x82, 0x20]), // display enhancement B
	(0xBB, &[0x1F]),       // pre-charge voltage
	(0xB6, &[0x08]),       // second pre-charge period
	(0xBE, &[0x07]),       // VCOMH
	(0xA6, &[]),           // normal display: not inverted, not all-on
	(0xA9, &[]),           // leave partial display
];

/// What went wrong on the way to the glass: the bus, or one of the three pins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<S, P> {
	Spi(S),
	Pin(P),
}

/// Widens one framebuffer row to the wire's four bits a pixel: a lit pixel becomes `ink`
/// (0..=15), an unlit one 0.
pub fn widen(bits: &[u8; ROW_BITS], ink: u8, out: &mut [u8; ROW_BYTES]) {
	let ink = ink & FULL;
	// Byte `i` of the wire is pixels `2i` and `2i + 1`: two bits of framebuffer byte `i / 4`.
	for (i, wide) in out.iter_mut().enumerate() {
		let pair = bits[i / 4] << (2 * (i % 4));
		let left = if pair & 0x80 != 0 { ink << 4 } else { 0 };
		let right = if pair & 0x40 != 0 { ink } else { 0 };
		*wide = left | right;
	}
}

/// What the glass was last sent, a checksum a row, so a frame sends only what changed. A
/// checksum and not a copy: a copy is another 2 KB of the board's one RAM region. Two rows
/// with one checksum would leave a stale row on the glass until it changes again, which is
/// why the caller also sends everything now and then ([`Shown::forget`]) — and that same
/// resend is what repairs a byte lost on a link nothing can be read back from.
pub struct Shown {
	sums: [u32; HEIGHT],
	known: bool,
}

const ROW_SUM: crc::Crc<u32> = crc::Crc::<u32>::new(&crc::CRC_32_ISO_HDLC);

impl Shown {
	pub const fn new() -> Self {
		Self {
			sums: [0; HEIGHT],
			known: false,
		}
	}

	/// The rows of this picture the glass does not show yet, as the first and the last of
	/// them — and takes them as sent. `None` when the glass is up to date. Everything,
	/// the first time and after [`Shown::forget`].
	pub fn changed<'a>(&mut self, rows: impl IntoIterator<Item = &'a [u8; ROW_BITS]>) -> Option<(usize, usize)> {
		let mut span: Option<(usize, usize)> = None;
		for (y, (row, sum)) in rows.into_iter().zip(self.sums.iter_mut()).enumerate() {
			let now = ROW_SUM.checksum(row);
			if !self.known || *sum != now {
				*sum = now;
				span = Some((span.map_or(y, |(first, _)| first), y));
			}
		}
		self.known = true;
		span
	}

	/// The glass may hold anything: the next picture goes out whole. For a frame that did
	/// not go out, and for the periodic resend.
	pub fn forget(&mut self) {
		self.known = false;
	}
}

impl Default for Shown {
	fn default() -> Self {
		Self::new()
	}
}

/// The controller on its bus: `spi` carries clock and data, `dc` says command or data, `cs`
/// selects the chip (low) and `res` resets it (low).
pub struct Ssd1322<SPI, PIN> {
	spi: SPI,
	dc: PIN,
	cs: PIN,
	res: PIN,
	/// One row widened for the wire. Held here and not in `rows`' future, so it sits where
	/// the driver does — a static, on the board — and not in the executor's shared arena.
	wide: [u8; ROW_BYTES],
}

impl<SPI: SpiBus, PIN: OutputPin> Ssd1322<SPI, PIN> {
	pub fn new(spi: SPI, dc: PIN, cs: PIN, res: PIN) -> Self {
		Self {
			spi,
			dc,
			cs,
			res,
			wide: [0; ROW_BYTES],
		}
	}

	/// Resets the controller, sets it up, blanks the glass and turns it on. `turned` draws
	/// the picture half a turn round.
	pub async fn start(&mut self, delay: &mut impl DelayNs, turned: bool) -> Result<(), Error<SPI::Error, PIN::Error>> {
		self.cs.set_high().map_err(Error::Pin)?;
		self.res.set_high().map_err(Error::Pin)?;
		delay.delay_ms(1).await;
		self.res.set_low().map_err(Error::Pin)?;
		delay.delay_ms(10).await;
		self.res.set_high().map_err(Error::Pin)?;
		// The module's own converter brings the panel supply up after the logic; the wait
		// is generous because nothing can be read back to say it is ready.
		delay.delay_ms(200).await;

		for (command, arguments) in INIT {
			self.command(*command, arguments).await?;
		}
		// The second argument keeps dual-COM mode on, as the maker's listing has it.
		self
			.command(SET_REMAP, &[if turned { REMAP_TURNED } else { REMAP_UPRIGHT }, 0x11])
			.await?;
		// The controller's RAM holds noise after power-on: blank what the glass shows before
		// it is lit.
		let dark = [0u8; ROW_BITS];
		self.rows(0, core::iter::repeat_n(&dark, HEIGHT), FULL).await?;
		self.command(DISPLAY_ON, &[]).await?;
		delay.delay_ms(100).await;
		Ok(())
	}

	/// Sends framebuffer rows to the glass, starting at row `first` and going down, lit
	/// pixels at grey level `ink`. Rows past the bottom edge are not sent.
	pub async fn rows<'a>(
		&mut self,
		first: usize,
		rows: impl IntoIterator<Item = &'a [u8; ROW_BITS]>,
		ink: u8,
	) -> Result<(), Error<SPI::Error, PIN::Error>> {
		if first >= HEIGHT {
			return Ok(());
		}
		self.command(SET_COLUMNS, &[COLUMN_FIRST, COLUMN_LAST]).await?;
		self.command(SET_ROWS, &[first as u8, (HEIGHT - 1) as u8]).await?;
		self.command(WRITE_RAM, &[]).await?;
		self.dc.set_high().map_err(Error::Pin)?;
		self.cs.set_low().map_err(Error::Pin)?;
		let mut sent = Ok(());
		for bits in rows.into_iter().take(HEIGHT - first) {
			widen(bits, ink, &mut self.wide);
			sent = self.spi.write(&self.wide).await;
			if sent.is_err() {
				break;
			}
		}
		// The chip is released whatever the bus said, so a failed frame does not leave it
		// selected for the next one.
		let flushed = self.spi.flush().await;
		self.cs.set_high().map_err(Error::Pin)?;
		sent.and(flushed).map_err(Error::Spi)
	}

	/// How bright a lit pixel is, 0..=255: the controller's contrast current, which takes
	/// the whole byte. The picture is not touched.
	pub async fn brightness(&mut self, level: u8) -> Result<(), Error<SPI::Error, PIN::Error>> {
		self.command(CONTRAST_CURRENT, &[level]).await
	}

	/// One command: its byte with `D/C` low, its arguments with `D/C` high.
	async fn command(&mut self, command: u8, arguments: &[u8]) -> Result<(), Error<SPI::Error, PIN::Error>> {
		self.dc.set_low().map_err(Error::Pin)?;
		self.cs.set_low().map_err(Error::Pin)?;
		let mut sent = self.spi.write(&[command]).await;
		// `D/C` is sampled with each byte's last bit, so the byte has to be on the wire
		// before the line moves.
		sent = sent.and(self.spi.flush().await);
		if sent.is_ok() && !arguments.is_empty() {
			self.dc.set_high().map_err(Error::Pin)?;
			sent = self.spi.write(arguments).await;
			sent = sent.and(self.spi.flush().await);
		}
		self.cs.set_high().map_err(Error::Pin)?;
		sent.map_err(Error::Spi)
	}
}
