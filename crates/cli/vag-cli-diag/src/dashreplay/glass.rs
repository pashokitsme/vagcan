//! The panel in memory, and as text a terminal can show.
//!
//! The board draws into its own framebuffer (`vag-dash-fw`'s `panel.rs`), which is not a
//! workspace crate and cannot be built for the host; `dashsim`'s canvas is research
//! tooling outside the workspace too. So the laptop keeps its own: one bit per pixel,
//! which is what the renderer draws — `BinaryColor`, lit or not.
//!
//! As text, two pixel rows per character: `▀` the upper lit, `▄` the lower, `█` both.
//! Where the terminal is narrower than the panel, braille — two columns and four rows of
//! pixels per character, every pixel still its own dot.

use std::convert::Infallible;

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;

/// A panel in memory, `width × height`, every pixel dark until drawn.
///
/// One bit per pixel, row-major, the first pixel in a byte's top bit — the layout of
/// the board's `Framebuffer`, so the two agree bit for bit. 256×64 is 2 KB here, where
/// a `bool` a pixel would be 16.
pub struct Canvas {
	width: usize,
	height: usize,
	bits: Vec<u8>,
}

impl Canvas {
	pub fn new(width: usize, height: usize) -> Canvas {
		Canvas {
			width,
			height,
			bits: vec![0; (width * height).div_ceil(8)],
		}
	}

	/// The byte pixel `(x, y)` is in, and its bit there. The caller keeps `(x, y)` on the panel.
	fn at(&self, x: usize, y: usize) -> (usize, u8) {
		let index = y * self.width + x;
		(index / 8, 0x80 >> (index % 8))
	}

	pub fn width(&self) -> usize {
		self.width
	}

	pub fn height(&self) -> usize {
		self.height
	}

	/// Every pixel dark again, as the board clears its framebuffer before each frame.
	pub fn clear(&mut self) {
		self.bits.fill(0);
	}

	/// Whether the pixel is lit; a pixel off the panel is dark.
	pub fn lit(&self, x: usize, y: usize) -> bool {
		if x >= self.width || y >= self.height {
			return false;
		}
		let (byte, bit) = self.at(x, y);
		self.bits[byte] & bit != 0
	}
}

impl OriginDimensions for Canvas {
	fn size(&self) -> Size {
		Size::new(self.width as u32, self.height as u32)
	}
}

impl DrawTarget for Canvas {
	type Color = BinaryColor;
	type Error = Infallible;

	fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
	where
		I: IntoIterator<Item = Pixel<BinaryColor>>,
	{
		for Pixel(point, colour) in pixels {
			let (Ok(x), Ok(y)) = (usize::try_from(point.x), usize::try_from(point.y)) else {
				continue;
			};
			if x < self.width && y < self.height {
				let (byte, bit) = self.at(x, y);
				if colour.is_on() {
					self.bits[byte] |= bit;
				} else {
					self.bits[byte] &= !bit;
				}
			}
		}
		Ok(())
	}
}

/// The panel as lines of half blocks: one character per pixel column, two pixel rows per
/// line. An odd last row pairs with a dark one.
pub fn half_blocks(canvas: &Canvas) -> Vec<String> {
	(0..canvas.height.div_ceil(2))
		.map(|line| {
			(0..canvas.width)
				.map(|x| match (canvas.lit(x, line * 2), canvas.lit(x, line * 2 + 1)) {
					(true, true) => '█',
					(true, false) => '▀',
					(false, true) => '▄',
					(false, false) => ' ',
				})
				.collect()
		})
		.collect()
}

/// The panel as lines of braille: two pixel columns and four pixel rows per character.
pub fn braille(canvas: &Canvas) -> Vec<String> {
	// The dot each pixel of a 2×4 cell lights, by (column, row) — Unicode's own order.
	const DOTS: [[u32; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];
	(0..canvas.height.div_ceil(4))
		.map(|line| {
			(0..canvas.width.div_ceil(2))
				.map(|column| {
					let mut bits = 0;
					for (dx, rows) in DOTS.iter().enumerate() {
						for (dy, dot) in rows.iter().enumerate() {
							if canvas.lit(column * 2 + dx, line * 4 + dy) {
								bits |= dot;
							}
						}
					}
					char::from_u32(0x2800 + bits).unwrap_or(' ')
				})
				.collect()
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	/// A 3×3 panel with a diagonal: (0,0), (1,1), (2,2).
	fn diagonal() -> Canvas {
		let mut canvas = Canvas::new(3, 3);
		let pixels = (0..3).map(|i| Pixel(Point::new(i, i), BinaryColor::On));
		canvas.draw_iter(pixels).unwrap();
		canvas
	}

	#[test]
	fn two_pixel_rows_make_one_line_of_half_blocks() {
		// Rows 0 and 1 share the first line; row 2 pairs with a dark row under the panel.
		assert_eq!(half_blocks(&diagonal()), ["▀▄ ", "  ▀"]);
		let mut full = Canvas::new(2, 2);
		full.draw_iter((0..4).map(|i| Pixel(Point::new(i % 2, i / 2), BinaryColor::On))).unwrap();
		assert_eq!(half_blocks(&full), ["██"]);
	}

	#[test]
	fn braille_keeps_every_pixel_as_its_own_dot() {
		// (0,0) is dot 1, (1,1) dot 5; (2,2) is dot 3 of the next character.
		assert_eq!(braille(&diagonal()), ["\u{2811}\u{2804}"]);
	}

	#[test]
	fn a_pixel_is_one_bit_laid_out_as_the_boards_framebuffer() {
		let mut canvas = Canvas::new(256, 64);
		assert_eq!(canvas.bits.len(), 256 * 64 / 8);
		// Row-major, the first pixel of a byte in its top bit.
		let pixels = [(0, 0), (9, 0), (7, 1)].map(|(x, y)| Pixel(Point::new(x, y), BinaryColor::On));
		canvas.draw_iter(pixels).unwrap();
		assert_eq!((canvas.bits[0], canvas.bits[1], canvas.bits[32]), (0x80, 0x40, 0x01));
		// Drawing dark over a lit pixel darkens it and nothing beside it.
		canvas.draw_iter([Pixel(Point::new(9, 0), BinaryColor::Off)]).unwrap();
		assert!(!canvas.lit(9, 0) && canvas.lit(0, 0) && canvas.lit(7, 1));
	}

	#[test]
	fn pixels_off_the_panel_are_dropped_and_clear_darkens_everything() {
		let mut canvas = diagonal();
		canvas
			.draw_iter([Pixel(Point::new(-1, 0), BinaryColor::On), Pixel(Point::new(3, 0), BinaryColor::On)])
			.unwrap();
		assert_eq!(half_blocks(&canvas), ["▀▄ ", "  ▀"]);
		canvas.clear();
		assert_eq!(half_blocks(&canvas), ["   ", "   "]);
	}
}
