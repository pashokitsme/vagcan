//! The dash renderer compiled for the browser: a page in as JSON, 256×64 bits out.
//! The strip on top is drawn here the way the layouts prototype draws it, until the
//! renderer has its own.

use core::convert::Infallible;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use serde::Deserialize;
use u8g2_fonts::FontRenderer;
use u8g2_fonts::fonts;
use u8g2_fonts::types::{FontColor, HorizontalAlignment, VerticalPosition};
use vag_dash_render::{Board, Cell, Deviation, Faults, Frame, Links, Strip, Theme, Volts, draw_page, draw_with};

const W: u32 = 256;
const H: u32 = 64;

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
	samples: Vec<f32>,
	seconds_per_sample: f32,
}

#[derive(Deserialize)]
struct StripIn {
	volts: Option<f32>,
	watts: Option<f32>,
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

fn small<D: DrawTarget<Color = BinaryColor>>(text: &str, at: Point, align: HorizontalAlignment, target: &mut D) -> i32 {
	let font = FontRenderer::new::<fonts::u8g2_font_5x7_t_cyrillic>();
	let _ = font.render_aligned(text, at, VerticalPosition::Top, align, FontColor::Transparent(BinaryColor::On), target);
	font
		.get_rendered_dimensions(text, at, VerticalPosition::Top)
		.ok()
		.and_then(|d| d.bounding_box)
		.map(|b| b.size.width as i32)
		.unwrap_or(0)
}

static mut INPUT: Vec<u8> = Vec::new();
static mut OUTPUT: Bits = Bits([0; (W * H / 8) as usize]);

#[unsafe(no_mangle)]
pub extern "C" fn input(len: usize) -> *mut u8 {
	unsafe {
		let buf = &mut *core::ptr::addr_of_mut!(INPUT);
		buf.clear();
		buf.resize(len, 0);
		buf.as_mut_ptr()
	}
}

#[unsafe(no_mangle)]
pub extern "C" fn output() -> *const u8 {
	unsafe { (*core::ptr::addr_of!(OUTPUT)).0.as_ptr() }
}

/// 0 drawn, 1 the JSON did not parse.
#[unsafe(no_mangle)]
pub extern "C" fn render() -> u32 {
	let (input, out) = unsafe { (&*core::ptr::addr_of!(INPUT), &mut *core::ptr::addr_of_mut!(OUTPUT)) };
	let Ok(page) = serde_json::from_slice::<PageIn>(input) else { return 1 };
	out.0.fill(0);
	let board = Board {
		links: Links { usb: false, ble: true },
		rates: None,
		faults: Some(Faults::Counted {
			stored: 2,
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
	let volts = match strip.and_then(|s| s.volts) {
		Some(v) => Volts::Shown(Some(v)),
		None => Volts::Hidden,
	};
	let real = Strip { title, volts };
	let watts = strip.and_then(|s| s.watts);
	if !real.is_empty() {
		// The renderer's own strip (feat/dash-strip).
		let _ = draw_page(&frame, &real, &board, &theme, out);
	} else if watts.is_some() {
		let mut below = out.cropped(&Rectangle::new(Point::new(0, 9), Size::new(W, H - 9)));
		let _ = draw_with(&frame, &board, &theme, &mut below);
	} else {
		let _ = draw_with(&frame, &board, &theme, out);
	}
	// Consumption is not in the renderer yet (todo/dash/23): drawn here after the voltage.
	if let Some(w) = watts {
		let x = match volts {
			Volts::Shown(Some(v)) => 1 + (format!("{v:.1}V").chars().count() as i32 + 2) * 5,
			_ => 1,
		};
		small(&format!("{w:.0}W"), Point::new(x, 0), HorizontalAlignment::Left, out);
	}
	0
}
