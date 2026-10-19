//! The dash renderer compiled for the browser: a page in as JSON, 256×64 bits out.
//!
//! `vagcan dev dash edit` serves this as `dash.wasm` beside its page, which calls `input(len)`
//! for a buffer, writes the page's JSON into it, calls `render()` and reads `W × H / 8` bytes
//! from `output()`, one bit a pixel, most significant first. Every pixel is drawn by
//! `vag-dash-render`'s own `draw_page`, the board's code.
//!
//! Runs on the board's std-free renderer; this crate itself is std, for the JSON.

use core::convert::Infallible;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use serde::Deserialize;
use vag_dash_render::{Board, Cell, Deviation, Faults, Frame, Links, Strip, Theme, Volts, draw_page};

/// The glass, as the renderer's tests lay pages out on it.
const W: u32 = vag_dash_render::PANEL.width;
const H: u32 = vag_dash_render::PANEL.height;

/// The panel, one bit a pixel.
struct Bits([u8; (W * H / 8) as usize]);

impl OriginDimensions for Bits {
	fn size(&self) -> Size {
		Size::new(W, H)
	}
}

impl DrawTarget for Bits {
	type Color = BinaryColor;
	type Error = Infallible;
	fn draw_iter<I: IntoIterator<Item = Pixel<BinaryColor>>>(&mut self, pixels: I) -> Result<(), Infallible> {
		for Pixel(p, c) in pixels {
			if p.x < 0 || p.y < 0 || p.x >= W as i32 || p.y >= H as i32 {
				continue;
			}
			let i = (p.y as u32 * W + p.x as u32) as usize;
			if c.is_on() {
				self.0[i / 8] |= 0x80 >> (i % 8);
			} else {
				self.0[i / 8] &= !(0x80 >> (i % 8));
			}
		}
		Ok(())
	}
}

#[derive(Deserialize)]
struct CellIn {
	label: String,
	/// `null` is a channel that has not answered: the board's dashes.
	value: Option<f32>,
	#[serde(default)]
	unit: String,
	#[serde(default)]
	decimals: u8,
	deviation: Option<f32>,
}

#[derive(Deserialize)]
struct ChartIn {
	cell: CellIn,
	min: f32,
	max: f32,
	#[serde(default)]
	samples: Vec<f32>,
	seconds_per_sample: f32,
}

#[derive(Deserialize)]
struct StripIn {
	/// Whether the strip shows a voltage on this page; `volts` is then its reading, `null` for
	/// one that has not answered.
	#[serde(default, rename = "voltsShown")]
	volts_shown: bool,
	volts: Option<f32>,
	#[serde(default)]
	title: String,
}

#[derive(Deserialize)]
struct PageIn {
	#[serde(default)]
	cells: Vec<CellIn>,
	chart: Option<ChartIn>,
	strip: Option<StripIn>,
}

fn cell(c: &CellIn) -> Cell<'_> {
	let cell = Cell::new(&c.label, c.value, &c.unit, c.decimals);
	match c.deviation {
		Some(d) => cell.with_deviation(Deviation::Value(d)),
		None => cell,
	}
}

/// Draw one page, as JSON, into `out`. `false` when the JSON is not a page.
fn draw(json: &[u8], out: &mut Bits) -> bool {
	let Ok(page) = serde_json::from_slice::<PageIn>(json) else {
		return false;
	};
	out.0.fill(0);
	// A board with BLE connected whose fault count came back zero, nothing failing now: what
	// the strip's right end holds most of the time. Not a reading of any car.
	let board = Board {
		links: Links { usb: false, ble: true },
		rates: None,
		faults: Some(Faults::Counted {
			stored: 0,
			failing_now: false,
		}),
	};
	let theme = Theme::bold_mono();
	let cells: Vec<Cell<'_>> = page.cells.iter().map(cell).collect();
	let frame = match &page.chart {
		Some(c) => Frame::Chart {
			cell: cell(&c.cell),
			min: c.min,
			max: c.max,
			samples: &c.samples,
			seconds_per_sample: c.seconds_per_sample,
		},
		None => Frame::Values { cells: &cells },
	};
	let strip = page.strip.as_ref();
	let title = strip.map(|s| s.title.as_str()).unwrap_or("");
	let volts = match strip {
		Some(s) if s.volts_shown => Volts::Shown(s.volts),
		_ => Volts::Hidden,
	};
	let _ = draw_page(&frame, &Strip { title, volts }, &board, &theme, out);
	true
}

static mut INPUT: Vec<u8> = Vec::new();
static mut OUTPUT: Bits = Bits([0; (W * H / 8) as usize]);

/// A buffer of `len` bytes for the page's JSON.
#[unsafe(no_mangle)]
pub extern "C" fn input(len: usize) -> *mut u8 {
	// SAFETY: the page calls one export at a time, on one thread.
	unsafe {
		let buf = &mut *core::ptr::addr_of_mut!(INPUT);
		buf.clear();
		buf.resize(len, 0);
		buf.as_mut_ptr()
	}
}

/// The panel `render` drew, `W × H / 8` bytes.
#[unsafe(no_mangle)]
pub extern "C" fn output() -> *const u8 {
	// SAFETY: as in `input`.
	unsafe { (*core::ptr::addr_of!(OUTPUT)).0.as_ptr() }
}

/// 0 drawn, 1 the JSON did not parse.
#[unsafe(no_mangle)]
pub extern "C" fn render() -> u32 {
	// SAFETY: as in `input`.
	let (input, out) = unsafe { (&*core::ptr::addr_of!(INPUT), &mut *core::ptr::addr_of_mut!(OUTPUT)) };
	if draw(input, out) { 0 } else { 1 }
}

#[cfg(test)]
mod tests {
	use super::*;

	fn lit(bits: &Bits) -> usize {
		bits.0.iter().map(|b| b.count_ones() as usize).sum()
	}

	#[test]
	fn a_page_draws_and_what_is_not_one_does_not() {
		let mut out = Bits([0; (W * H / 8) as usize]);
		assert!(!draw(b"not json", &mut out));
		let page = br#"{"cells":[{"label":"COOLANT","value":null,"unit":"C","decimals":0,"deviation":null}],"chart":null,"strip":{"voltsShown":true,"volts":null,"title":"MAIN"}}"#;
		assert!(draw(page, &mut out));
		assert!(lit(&out) > 0, "the label, the dashes and the strip are drawn");
	}

	#[test]
	fn a_chart_with_no_samples_yet_draws() {
		let mut out = Bits([0; (W * H / 8) as usize]);
		let page = br#"{"chart":{"cell":{"label":"RPM","value":null,"unit":"1/min","decimals":0,"deviation":null},"min":0,"max":4000,"samples":[],"seconds_per_sample":0.5},"strip":null}"#;
		assert!(draw(page, &mut out));
		assert!(lit(&out) > 0);
	}
}
