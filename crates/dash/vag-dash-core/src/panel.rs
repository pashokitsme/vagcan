//! A panel made of memory, and the wire format that carries it to a laptop.
//!
//! The board renders the *real* thing — `vag_dash_render::draw_page` onto a framebuffer —
//! and ships the pixels out of the USB port it is already logged through. A
//! program on the laptop draws them in a terminal. Nothing about the layout,
//! the page order or the formatting lives on that side; if the simulator were
//! clever, the thing being tested would be the simulator.
//!
//! This was written before the OLED was bought, and it stays useful now that
//! the glass is fitted (`ssd1322.rs` sends it the same rows): a panel you can
//! screenshot, diff and read at arm's length is worth more on the bench than a
//! strip of glass in a vent.

use core::convert::Infallible;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

/// The glass, in pixels: the renderer's [`PANEL`](vag_dash_render::PANEL), 256×64, which is
/// what the board has and what every page is laid out on.
pub const WIDTH: usize = vag_dash_render::PANEL.width as usize;
pub const HEIGHT: usize = vag_dash_render::PANEL.height as usize;
const BYTES: usize = WIDTH * HEIGHT / 8;

/// One bit per pixel, row-major, most significant bit leftmost. 2 KB, static:
/// this is the one large buffer in the firmware and it must not come from the
/// heap, which is reserved for the radio.
pub struct Framebuffer {
	bits: [u8; BYTES],
}

impl Framebuffer {
	pub const fn new() -> Self {
		Self { bits: [0; BYTES] }
	}

	pub fn clear_all(&mut self) {
		self.bits = [0; BYTES];
	}

	pub fn get(&self, x: usize, y: usize) -> bool {
		let index = y * WIDTH + x;
		self.bits[index / 8] & (0x80 >> (index % 8)) != 0
	}

	/// Row `y` as it is held: a bit a pixel, most significant bit leftmost — what the glass's
	/// driver widens and sends.
	pub fn row(&self, y: usize) -> &[u8; WIDTH / 8] {
		let start = y * (WIDTH / 8);
		self.bits[start..start + WIDTH / 8].try_into().expect("a row is WIDTH / 8 bytes")
	}

	/// Every row, top to bottom.
	pub fn rows(&self) -> impl Iterator<Item = &[u8; WIDTH / 8]> {
		(0..HEIGHT).map(|y| self.row(y))
	}

	fn set(&mut self, x: usize, y: usize, lit: bool) {
		let index = y * WIDTH + x;
		let mask = 0x80 >> (index % 8);
		if lit {
			self.bits[index / 8] |= mask;
		} else {
			self.bits[index / 8] &= !mask;
		}
	}

	/// The pixels of `other`, copied in: the picture a frame leaves for the USB writer.
	pub fn copy_from(&mut self, other: &Framebuffer) {
		self.bits = other.bits;
	}

	/// The `FRAME` line, a piece at a time ([`FrameLine::fill`]): the board writes it to USB
	/// as it encodes, so no buffer holds the whole line — which can be [`LINE_MAX`] long.
	pub fn frame_line(&self) -> FrameLine<'_> {
		FrameLine {
			frame: self,
			index: 0,
			lit: false,
			run: 0,
			started: false,
			finished: false,
			token: [0; TOKEN_MAX],
			token_len: 0,
			token_at: 0,
		}
	}

	/// Writes the whole `FRAME` line to `out` ([`Framebuffer::frame_line`]).
	pub fn write_frame(&self, out: &mut impl core::fmt::Write) -> core::fmt::Result {
		let mut line = self.frame_line();
		let mut chunk = [0u8; 64];
		loop {
			let n = line.fill(&mut chunk);
			if n == 0 {
				return Ok(());
			}
			// ASCII by construction.
			out.write_str(core::str::from_utf8(&chunk[..n]).map_err(|_| core::fmt::Error)?)?;
		}
	}

	fn pixel(&self, index: usize) -> bool {
		self.bits[index / 8] & (0x80 >> (index % 8)) != 0
	}
}

/// Decimal digits of `n`, for the header's length.
const fn digits(mut n: usize) -> usize {
	let mut count = 1;
	while n >= 10 {
		n /= 10;
		count += 1;
	}
	count
}

/// `FRAME <width> <height> `.
const HEADER_LEN: usize = "FRAME ".len() + digits(WIDTH) + 1 + digits(HEIGHT) + 1;

/// The longest `FRAME` line any picture makes, its `\r\n` not counted: the header and two hex
/// digits a run, where at worst every pixel is a run of its own and a lit first pixel adds the
/// empty unlit run before it — `WIDTH × HEIGHT + 1` runs. A run split at 255 costs a run more,
/// but only in place of the 255 changes a run that long cannot have.
pub const LINE_MAX: usize = HEADER_LEN + 2 * (WIDTH * HEIGHT + 1);

/// The longest piece [`FrameLine`] makes at once: the header.
const TOKEN_MAX: usize = HEADER_LEN;

/// One `FRAME` line, encoded as it is read: a run-length encoding of the pixels, starting with
/// unlit, two hex digits per run.
///
/// A run longer than 255 is split by emitting `ff` then an empty run `00` of the other colour.
/// That costs two characters and keeps the decoder on the other side trivial — and
/// `research/dash/host/src/frame.rs` holds the matching decoder; its tests decode this encoder's
/// lines.
pub struct FrameLine<'a> {
	frame: &'a Framebuffer,
	/// The next pixel to look at.
	index: usize,
	lit: bool,
	run: u32,
	started: bool,
	finished: bool,
	/// The piece being handed out: the header, or one run (`ff00` for a split).
	token: [u8; TOKEN_MAX],
	token_len: usize,
	token_at: usize,
}

impl FrameLine<'_> {
	/// The next bytes of the line into `out`, as many as fit; 0 once the line is out.
	pub fn fill(&mut self, out: &mut [u8]) -> usize {
		let mut n = 0;
		while n < out.len() {
			if self.token_at == self.token_len && !self.next_token() {
				break;
			}
			let take = (out.len() - n).min(self.token_len - self.token_at);
			out[n..n + take].copy_from_slice(&self.token[self.token_at..self.token_at + take]);
			self.token_at += take;
			n += take;
		}
		n
	}

	fn next_token(&mut self) -> bool {
		self.token_len = 0;
		self.token_at = 0;
		if !self.started {
			self.started = true;
			self.push(b"FRAME ");
			self.push_decimal(WIDTH);
			self.push(b" ");
			self.push_decimal(HEIGHT);
			self.push(b" ");
			return true;
		}
		while self.index < WIDTH * HEIGHT {
			let pixel = self.frame.pixel(self.index);
			self.index += 1;
			if pixel == self.lit && self.run < 255 {
				self.run += 1;
				continue;
			}
			self.push_run();
			if pixel == self.lit {
				self.push(b"00");
			} else {
				self.lit = pixel;
			}
			self.run = 1;
			return true;
		}
		if self.finished {
			return false;
		}
		self.finished = true;
		self.push_run();
		true
	}

	fn push(&mut self, bytes: &[u8]) {
		self.token[self.token_len..self.token_len + bytes.len()].copy_from_slice(bytes);
		self.token_len += bytes.len();
	}

	fn push_run(&mut self) {
		const HEX: &[u8; 16] = b"0123456789abcdef";
		let run = self.run as usize;
		self.push(&[HEX[run >> 4], HEX[run & 0xf]]);
	}

	fn push_decimal(&mut self, n: usize) {
		let mut digit = [0u8; digits(usize::MAX)];
		let count = digits(n);
		let mut rest = n;
		for at in (0..count).rev() {
			digit[at] = b'0' + (rest % 10) as u8;
			rest /= 10;
		}
		self.push(&digit[..count]);
	}
}

impl Default for Framebuffer {
	fn default() -> Self {
		Self::new()
	}
}

impl Dimensions for Framebuffer {
	fn bounding_box(&self) -> Rectangle {
		// `vag_dash_render::render` reads the panel size from here rather than from a
		// constant: the layout is decided by measuring the target.
		Rectangle::new(Point::zero(), Size::new(WIDTH as u32, HEIGHT as u32))
	}
}

impl DrawTarget for Framebuffer {
	type Color = BinaryColor;
	type Error = Infallible;

	fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
	where
		I: IntoIterator<Item = Pixel<Self::Color>>,
	{
		for Pixel(point, colour) in pixels {
			// Out of bounds is silently dropped, as `DrawTarget` requires:
			// clipping is the target's job, and a renderer that had to bounds
			// check every glyph would be the wrong shape.
			if (0..WIDTH as i32).contains(&point.x) && (0..HEIGHT as i32).contains(&point.y) {
				self.set(point.x as usize, point.y as usize, colour.is_on());
			}
		}
		Ok(())
	}

	fn clear(&mut self, colour: Self::Color) -> Result<(), Self::Error> {
		self.bits = [if colour.is_on() { 0xff } else { 0x00 }; BYTES];
		Ok(())
	}
}

#[cfg(test)]
mod tests {
	extern crate std;
	use super::*;
	use std::string::String;

	#[test]
	fn the_framebuffer_is_the_glass() {
		assert_eq!((WIDTH, HEIGHT), (256, 64));
		assert_eq!(Framebuffer::new().bounding_box().size, vag_dash_render::PANEL);
	}

	#[test]
	fn a_pixel_is_a_bit_row_major_the_leftmost_in_the_top_bit() {
		let mut frame = Framebuffer::new();
		frame.draw_iter([Pixel(Point::new(9, 1), BinaryColor::On)]).unwrap();
		assert!(frame.get(9, 1));
		let row = frame.row(1);
		assert_eq!((row[0], row[1]), (0, 0x40), "pixel 9 is 0x40 of the row's byte 1");
		assert!(frame.rows().enumerate().all(|(y, r)| y == 1 || r.iter().all(|b| *b == 0)));
		// Off the glass is dropped, not wrapped onto the next row.
		frame.draw_iter([Pixel(Point::new(WIDTH as i32, 1), BinaryColor::On)]).unwrap();
		assert!(!frame.get(0, 2));
	}

	#[test]
	fn a_frame_line_is_runs_from_unlit_and_a_long_run_is_split() {
		let mut line = String::new();
		Framebuffer::new().write_frame(&mut line).unwrap();
		// Every pixel unlit: runs of 255 with an empty lit run after each, then the rest.
		let full = (WIDTH * HEIGHT) / 255;
		let mut want = std::format!("FRAME {WIDTH} {HEIGHT} ");
		for _ in 0..full {
			want.push_str("ff00");
		}
		want.push_str(&std::format!("{:02x}", WIDTH * HEIGHT - full * 255));
		assert_eq!(line, want);

		let mut frame = Framebuffer::new();
		frame.draw_iter([Pixel(Point::new(1, 0), BinaryColor::On)]).unwrap();
		let mut line = String::new();
		frame.write_frame(&mut line).unwrap();
		assert!(line.starts_with("FRAME 256 64 0101ff00"), "{line}");
	}

	/// The encoder as it was, whole line into a `String`: what the pieces must add up to.
	fn reference(frame: &Framebuffer) -> String {
		use core::fmt::Write;
		let mut out = std::format!("FRAME {WIDTH} {HEIGHT} ");
		let mut lit = false;
		let mut run: u32 = 0;
		for index in 0..WIDTH * HEIGHT {
			let pixel = frame.pixel(index);
			if pixel == lit && run < 255 {
				run += 1;
			} else {
				write!(out, "{run:02x}").unwrap();
				if pixel == lit {
					out.push_str("00");
				} else {
					lit = pixel;
				}
				run = 1;
			}
		}
		write!(out, "{run:02x}").unwrap();
		out
	}

	fn drawn(lit: impl Fn(usize, usize) -> bool) -> Framebuffer {
		let mut frame = Framebuffer::new();
		for y in 0..HEIGHT {
			for x in 0..WIDTH {
				frame.set(x, y, lit(x, y));
			}
		}
		frame
	}

	/// In pieces of `size`, as the USB writer takes it.
	fn in_pieces(frame: &Framebuffer, size: usize) -> String {
		let mut line = frame.frame_line();
		let mut out = std::vec::Vec::new();
		let mut piece = std::vec![0u8; size];
		loop {
			let n = line.fill(&mut piece);
			if n == 0 {
				break;
			}
			assert!(n == size || line.fill(&mut piece) == 0, "only the last piece is short");
			out.extend_from_slice(&piece[..n]);
		}
		String::from_utf8(out).unwrap()
	}

	#[test]
	fn the_longest_line_any_picture_makes_is_line_max() {
		// Every pixel its own run from a lit first one: the worst case, exactly.
		let stripes = drawn(|x, _| x % 2 == 0);
		let line = in_pieces(&stripes, 64);
		assert_eq!(line.len(), LINE_MAX);
		assert_eq!(LINE_MAX, "FRAME 256 64 ".len() + 2 * (256 * 64 + 1));
		// The checkerboard: a run of two where each row meets the next, so a little shorter.
		let checker = drawn(|x, y| (x + y) % 2 == 0);
		let line = in_pieces(&checker, 64);
		assert!(line.len() <= LINE_MAX && line.len() > LINE_MAX - 2 * HEIGHT, "{}", line.len());
		// Starting unlit saves the empty first run.
		assert_eq!(in_pieces(&drawn(|x, _| x % 2 == 1), 64).len(), LINE_MAX - 2);
	}

	#[test]
	fn the_pieces_are_the_encoder_whole_whatever_their_size() {
		let pictures = [
			Framebuffer::new(),
			drawn(|_, _| true),
			drawn(|x, y| (x + y) % 2 == 0),
			drawn(|x, _| x % 2 == 0),
			drawn(|x, y| (x / 8 + y / 8) % 2 == 0),
			drawn(|x, y| x * y % 7 == 3),
		];
		for (i, frame) in pictures.iter().enumerate() {
			let want = reference(frame);
			assert!(want.len() <= LINE_MAX, "picture {i}");
			for size in [1, 2, 3, 7, 13, 64, 4096, LINE_MAX + 5] {
				assert_eq!(in_pieces(frame, size), want, "picture {i}, pieces of {size}");
			}
			let mut whole = String::new();
			frame.write_frame(&mut whole).unwrap();
			assert_eq!(whole, want, "picture {i}, write_frame");
		}
	}

	#[test]
	fn a_copy_is_the_same_picture() {
		let from = drawn(|x, y| (x ^ y) % 3 == 0);
		let mut to = Framebuffer::new();
		to.copy_from(&from);
		assert!(to.rows().eq(from.rows()));
	}
}
