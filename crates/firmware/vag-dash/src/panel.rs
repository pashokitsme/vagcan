//! A panel made of memory, and the wire format that carries it to a laptop.
//!
//! The board renders the *real* thing — `vag_panel::draw` onto a framebuffer —
//! and ships the pixels out of the USB port it is already logged through. A
//! program on the laptop draws them in a terminal. Nothing about the layout,
//! the page order or the formatting lives on that side; if the simulator were
//! clever, the thing being tested would be the simulator.
//!
//! This exists because the OLED has not been bought yet, and it stays useful
//! after it has: a panel you can screenshot, diff and read at arm's length is
//! worth more on the bench than a strip of glass in a vent.

use core::convert::Infallible;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;

/// The panel we are pretending to have. 256×64 is what you can actually buy —
/// see `README.md`; 256×32 was the original target and is not a real part.
pub const WIDTH: usize = 256;
pub const HEIGHT: usize = 64;
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

	fn set(&mut self, x: usize, y: usize, lit: bool) {
		let index = y * WIDTH + x;
		let mask = 0x80 >> (index % 8);
		if lit {
			self.bits[index / 8] |= mask;
		} else {
			self.bits[index / 8] &= !mask;
		}
	}

	/// Writes one `FRAME` line: a run-length encoding of the pixels, starting
	/// with unlit, two hex digits per run.
	///
	/// A run longer than 255 is split by emitting `ff` then an empty run `00`
	/// of the other colour. That costs two characters and keeps the decoder on
	/// the other side trivial — and `research/dash/bleecho/src/frame.rs` holds
	/// the matching decoder plus the round-trip tests that say the two agree.
	pub fn write_frame(&self, out: &mut impl core::fmt::Write) -> core::fmt::Result {
		write!(out, "FRAME {WIDTH} {HEIGHT} ")?;
		let mut lit = false;
		let mut run: u32 = 0;
		for index in 0..WIDTH * HEIGHT {
			let pixel = self.bits[index / 8] & (0x80 >> (index % 8)) != 0;
			if pixel == lit && run < 255 {
				run += 1;
			} else {
				write!(out, "{run:02x}")?;
				if pixel == lit {
					write!(out, "00")?;
				} else {
					lit = pixel;
				}
				run = 1;
			}
		}
		write!(out, "{run:02x}")
	}
}

impl Default for Framebuffer {
	fn default() -> Self {
		Self::new()
	}
}

impl Dimensions for Framebuffer {
	fn bounding_box(&self) -> Rectangle {
		// `vag_panel::render` reads the panel size from here rather than from a
		// constant, which is what lets the same code draw 256×32 and 256×64.
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
