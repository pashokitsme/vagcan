//! The wire format between a board that renders and a laptop that shows.
//!
//! One line of text per frame, so it can share the device's serial port with
//! its log output and neither has to know about the other. A line that starts
//! with `FRAME ` is a picture; anything else is a log line and is shown as one.
//!
//! ```text
//! FRAME <width> <height> <runs...>
//! ```
//!
//! The runs are a run-length encoding of the pixels in row-major order,
//! **starting with unlit**, each run two hex digits. A run longer than 255 is
//! split by emitting `ff` and then `00` for the other colour, which costs two
//! bytes and keeps the decoder trivial.
//!
//! Why run-length and not the raw bitmap: a dash panel is mostly empty. 256×64
//! is 2048 bytes raw, 4096 as hex, every frame. The same picture as runs is
//! usually a few hundred bytes, and the encoder is a dozen lines.

pub const MARKER: &str = "FRAME ";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
	pub width: u32,
	pub height: u32,
	/// Row-major, lit or not.
	pub pixels: Pixels,
}

impl Bitmap {
	pub fn get(&self, x: u32, y: u32) -> bool {
		if x >= self.width || y >= self.height {
			return false;
		}
		self.pixels.get((y * self.width + x) as usize)
	}
}

/// Pixels one bit each, the first of a byte in its top bit — the layout of the board's
/// `Framebuffer` (`vag-dash-fw`'s `panel.rs`), so the two agree bit for bit. A 256×64
/// panel is 2 KB this way, where a `bool` a pixel would be 16.
///
/// The bits past `len` in the last byte stay clear, which is what lets `==` compare bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pixels {
	len: usize,
	bits: Vec<u8>,
}

impl Pixels {
	/// `len` pixels, all dark.
	pub fn dark(len: usize) -> Self {
		Pixels {
			len,
			bits: vec![0; len.div_ceil(8)],
		}
	}

	pub fn len(&self) -> usize {
		self.len
	}

	pub fn is_empty(&self) -> bool {
		self.len == 0
	}

	/// Pixel `index`; one past the end is dark.
	pub fn get(&self, index: usize) -> bool {
		index < self.len && self.bits[index / 8] & (0x80 >> (index % 8)) != 0
	}

	/// Sets pixel `index`. One past the end is a bug in the caller, and panics as a slice would.
	pub fn set(&mut self, index: usize, lit: bool) {
		assert!(index < self.len, "pixel {index} of {}", self.len);
		let mask = 0x80 >> (index % 8);
		if lit {
			self.bits[index / 8] |= mask;
		} else {
			self.bits[index / 8] &= !mask;
		}
	}

	pub fn iter(&self) -> impl Iterator<Item = bool> + '_ {
		(0..self.len).map(|index| self.get(index))
	}
}

impl FromIterator<bool> for Pixels {
	fn from_iter<I: IntoIterator<Item = bool>>(pixels: I) -> Self {
		let mut out = Pixels::default();
		for lit in pixels {
			if out.len.is_multiple_of(8) {
				out.bits.push(0);
			}
			out.len += 1;
			out.set(out.len - 1, lit);
		}
		out
	}
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DecodeError {
	#[error("not a frame line")]
	NotAFrame,
	#[error("frame header is malformed")]
	BadHeader,
	#[error("run data is not pairs of hex digits")]
	BadRuns,
	#[error("runs describe {got} pixels, header says {want}")]
	WrongLength { got: usize, want: usize },
}

/// Decodes one `FRAME` line.
///
/// Deliberately strict about the total: a frame whose runs do not add up is a
/// frame that would be drawn subtly wrong, and a subtly wrong picture is worse
/// than a missing one — the whole reason this simulator exists is to be
/// believed.
pub fn decode(line: &str) -> Result<Bitmap, DecodeError> {
	let rest = line.strip_prefix(MARKER).ok_or(DecodeError::NotAFrame)?;
	let mut parts = rest.split_whitespace();
	let width: u32 = parts.next().ok_or(DecodeError::BadHeader)?.parse().map_err(|_| DecodeError::BadHeader)?;
	let height: u32 = parts.next().ok_or(DecodeError::BadHeader)?.parse().map_err(|_| DecodeError::BadHeader)?;
	let runs = parts.next().unwrap_or("");
	if width == 0 || height == 0 || width > 4096 || height > 4096 {
		return Err(DecodeError::BadHeader);
	}
	if !runs.len().is_multiple_of(2) {
		return Err(DecodeError::BadRuns);
	}

	let want = (width * height) as usize;
	let mut pixels = Pixels::dark(want);
	// How many pixels the runs so far have described.
	let mut got = 0;
	let mut lit = false;
	for pair in runs.as_bytes().chunks(2) {
		let text = std::str::from_utf8(pair).map_err(|_| DecodeError::BadRuns)?;
		let run = u8::from_str_radix(text, 16).map_err(|_| DecodeError::BadRuns)?;
		let end = got + usize::from(run);
		if end > want {
			return Err(DecodeError::WrongLength { got: end, want });
		}
		// The pixels start dark, so only a lit run has anything to write.
		if lit {
			(got..end).for_each(|index| pixels.set(index, true));
		}
		got = end;
		lit = !lit;
	}
	if got != want {
		return Err(DecodeError::WrongLength { got, want });
	}
	Ok(Bitmap { width, height, pixels })
}

/// The encoder, kept here beside the decoder so the two cannot drift.
///
/// The firmware has its own copy in Rust that must agree with this one; these
/// tests are what says whether it does.
pub fn encode(bitmap: &Bitmap) -> String {
	let mut out = format!("{MARKER}{} {} ", bitmap.width, bitmap.height);
	let mut lit = false;
	let mut run = 0u32;
	for pixel in bitmap.pixels.iter() {
		if pixel == lit && run < 255 {
			run += 1;
		} else {
			out.push_str(&format!("{run:02x}"));
			if pixel == lit {
				// The run hit 255 without the colour changing: emit an empty
				// run of the other colour and carry on with the same one.
				out.push_str("00");
				run = 1;
			} else {
				lit = pixel;
				run = 1;
			}
		}
	}
	out.push_str(&format!("{run:02x}"));
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	fn roundtrip(width: u32, height: u32, pixels: Pixels) {
		let bitmap = Bitmap { width, height, pixels };
		assert_eq!(decode(&encode(&bitmap)).unwrap(), bitmap);
	}

	#[test]
	fn empty_panel_roundtrips() {
		roundtrip(256, 64, Pixels::dark(256 * 64));
	}

	#[test]
	fn full_panel_roundtrips() {
		// Every pixel lit is the case that forces run splitting: 16384 lit
		// pixels cannot be one run of 255.
		roundtrip(256, 64, std::iter::repeat_n(true, 256 * 64).collect());
	}

	#[test]
	fn alternating_pixels_roundtrip() {
		roundtrip(16, 4, (0..64).map(|i| i % 2 == 0).collect());
	}

	#[test]
	fn a_panel_that_does_not_fill_its_last_byte_roundtrips() {
		// 5×3 is 15 pixels: the last byte holds seven, and its spare bit stays clear.
		roundtrip(5, 3, (0..15).map(|i| i % 3 != 1).collect());
		roundtrip(5, 3, std::iter::repeat_n(true, 15).collect());
	}

	#[test]
	fn a_shape_roundtrips() {
		let mut pixels = Pixels::dark(256 * 64);
		for y in 10..20 {
			for x in 30..200 {
				pixels.set(y * 256 + x, true);
			}
		}
		roundtrip(256, 64, pixels);
	}

	#[test]
	fn pixels_are_bits_laid_out_as_the_boards_framebuffer() {
		// Row-major, the first pixel of a byte in its top bit: pixel 0 is 0x80 of byte 0,
		// pixel 9 is 0x40 of byte 1, as `vag-dash-fw`'s `Framebuffer` has them.
		let mut pixels = Pixels::dark(256 * 64);
		assert_eq!(pixels.bits.len(), 256 * 64 / 8);
		pixels.set(0, true);
		pixels.set(9, true);
		assert_eq!(pixels.bits[..2], [0x80, 0x40]);
		pixels.set(9, false);
		assert_eq!(pixels.bits[..2], [0x80, 0x00]);
		// Built one pixel at a time: the same bits, the same length.
		let collected: Pixels = pixels.iter().collect();
		assert_eq!(collected, pixels);
		assert!(!pixels.get(256 * 64), "one past the end is dark");
	}

	#[test]
	fn a_short_frame_is_refused_rather_than_padded() {
		let err = decode("FRAME 256 64 0102").unwrap_err();
		assert!(matches!(err, DecodeError::WrongLength { .. }));
	}

	#[test]
	fn a_log_line_is_not_a_frame() {
		assert_eq!(decode("INFO - hello").unwrap_err(), DecodeError::NotAFrame);
	}

	#[test]
	fn odd_hex_is_refused() {
		assert_eq!(decode("FRAME 8 1 010").unwrap_err(), DecodeError::BadRuns);
	}

	#[test]
	fn encoding_an_empty_panel_is_small() {
		// The point of the encoding: 16384 unlit pixels must not cost 16384
		// characters. 65 runs of 255 plus the splits.
		let bitmap = Bitmap {
			width: 256,
			height: 64,
			pixels: Pixels::dark(256 * 64),
		};
		assert!(encode(&bitmap).len() < 300, "{}", encode(&bitmap).len());
	}
}
