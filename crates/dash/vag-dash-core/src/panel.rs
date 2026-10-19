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

	/// Writes one `FRAME` line: a run-length encoding of the pixels, starting
	/// with unlit, two hex digits per run.
	///
	/// A run longer than 255 is split by emitting `ff` then an empty run `00`
	/// of the other colour. That costs two characters and keeps the decoder on
	/// the other side trivial — and `research/dash/host/src/frame.rs` holds
	/// the matching decoder; its tests decode this encoder's lines.
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
}
