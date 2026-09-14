//! Putting a [`Frame`] on the glass.
//!
//! Panel height is the constraint that decides everything below, and it is read
//! from the target rather than assumed.
//!
//! At **32 pixels** a label over a number is two tiers and fits; the reference
//! panel's four — label on two lines, number, unit underneath — do not, so the
//! unit sits beside the number and the label gets one line.
//!
//! At **64**, which is the height you can actually buy, three tiers fit: label
//! at the top, the number centred in the band below it, the unit under that.
//! The number is then vertically centred rather than sitting on the floor,
//! which is what the eye expects of the thing it came to read. Which layout is
//! used is decided by measuring, not by a flag: if the three stack inside the
//! height, they are stacked.
//!
//! Nothing here scales, clips or rounds silently. Where the text does not fit,
//! [`draw`] says so in its [`Report`] and the caller decides. A layout that
//! quietly clips is a layout whose failures are invisible in exactly the
//! situation nobody is watching for them — which is the whole situation this
//! panel is for.

use core::fmt::Write;

use eg_seven_segment::SevenSegmentStyle;
use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle, Triangle};
use embedded_graphics::text::Text;
use u8g2_fonts::FontRenderer;
use u8g2_fonts::types::{FontColor, HorizontalAlignment, VerticalPosition};

use crate::frame::{Board, Cell, Frame, Links, Rates};
use crate::theme::{Numerals, Theme};

/// Breathing room each side of a cell's contents.
///
/// Without it a four-digit reading fills its column edge to edge and touches its
/// neighbour, and two numbers with no gap between them read as one number. Three
/// pixels is the smallest gap that still separates at a glance — which is the
/// only kind of look this panel ever gets.
const PAD: u32 = 3;

/// What did not fit.
///
/// Returned rather than logged because there is nowhere to log to on the board,
/// and returned rather than ignored because the generator that built the plan is
/// the thing that can fix it — by shortening a label or by choosing a page with
/// fewer cells.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
	/// A label wider than its column. Drawn anyway, and it will collide.
	pub label_overrun: bool,
	/// The number itself did not fit its column, even without the unit.
	pub value_overrun: bool,
	/// The unit string was dropped to make room for the number.
	pub unit_dropped: bool,
	/// The number was drawn in a smaller face than the theme's first choice.
	///
	/// Not an error — it is the ladder working — but the generator should know,
	/// because a page where every cell shrinks is a page with too many cells.
	pub value_shrunk: bool,
	/// The font had no glyph for something it was asked to draw.
	///
	/// Reported because the alternative is what this cost an afternoon: a face
	/// with no `°` drew nothing at all, the error went into a `let _ =`, and the
	/// panel simply had no degree sign on it. Text that vanishes is worse than
	/// text that overruns, because nothing on the screen says it happened.
	pub glyph_missing: bool,
}

/// A stack buffer for one formatted string.
///
/// Sized for the chart header, not for a number: the header is a sentence and
/// Cyrillic costs two bytes a character, so sixteen bytes — which looked ample
/// for `"1234"` — truncated `"НАДДУВ 0.00-2.50bar 19s"` after nine characters.
/// The alternative is an allocator on a microcontroller for the sake of a label.
struct Buf {
	bytes: [u8; 96],
	len: usize,
}

impl Buf {
	fn new() -> Self {
		Buf { bytes: [0; 96], len: 0 }
	}

	fn as_str(&self) -> &str {
		// Only ever written through `write_str`, so it is UTF-8 by construction;
		// a truncated write stops at a byte boundary because it stops at a whole
		// `&str`.
		core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("?")
	}
}

impl Write for Buf {
	fn write_str(&mut self, s: &str) -> core::fmt::Result {
		let room = self.bytes.len() - self.len;
		if s.len() > room {
			return Err(core::fmt::Error);
		}
		self.bytes[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
		self.len += s.len();
		Ok(())
	}
}

/// Render a value, or the dash that stands for "the car did not say".
fn number(cell: &Cell<'_>) -> Buf {
	let mut buf = Buf::new();
	match cell.value {
		// A channel that has not answered gets a dash. Not a zero: a zero is a
		// reading, and this is the absence of one.
		None => {
			let _ = buf.write_str("--");
		}
		Some(v) => {
			let _ = write!(buf, "{:.*}", cell.decimals as usize, v);
		}
	}
	buf
}

/// Draw one frame with nothing connected and no rates measured. Returns what did not fit.
pub fn draw<D>(frame: &Frame<'_>, theme: &Theme, target: &mut D) -> Report
where
	D: DrawTarget<Color = BinaryColor>,
{
	draw_with(frame, &Board::default(), theme, target)
}

/// Draw one frame with what the board says about itself: the link icons on the values and
/// chart pages, the bus rates on the adapter screen. Returns what did not fit.
pub fn draw_with<D>(frame: &Frame<'_>, board: &Board, theme: &Theme, target: &mut D) -> Report
where
	D: DrawTarget<Color = BinaryColor>,
{
	match frame {
		Frame::Values { cells } => values(cells, board.links, theme, target),
		Frame::Chart {
			cell,
			min,
			max,
			samples,
			seconds_per_sample,
		} => chart(cell, *min, *max, samples, *seconds_per_sample, board.links, theme, target),
		Frame::Adapter(state) => adapter(state, board.rates, target),
	}
}

// --- the link icons ---------------------------------------------------------------------

/// One link icon's cell, in pixels.
///
/// Drawn from primitives, not from a font: no face here has either symbol. Five columns
/// is the narrowest a Bluetooth rune keeps its two arrowheads apart and nine rows is what
/// its diagonals need at that width; the cell adds a clear column each side, and the USB
/// plug is drawn to the same cell so the pair reads as a pair.
pub const ICON: Size = Size::new(7, 9);

/// Dark rows between two icons, one under the other: as many as above them.
const ICON_GAP: u32 = 2;

/// Dark rows above the icons: an icon touching the edge of the glass reads as cut off
/// (owner, on the first preview).
const ICON_TOP: u32 = 2;

/// Dark columns right of the icons. Wider than [`ICON_TOP`] because columns read closer
/// than rows on this panel (owner, on the second preview).
const ICON_RIGHT: u32 = 4;

/// Dark columns between the icons and the label or chart header they narrow.
const ICON_CLEARANCE: i32 = 4;

/// Where the link icons go: a column in the top-right corner, [`ICON_TOP`] down and
/// [`ICON_RIGHT`] in, USB above BLE. `None` when nothing is connected — then nothing is
/// drawn.
///
/// A column and not a row (owner, 2026-09-14): the room it takes from the rightmost label is
/// one icon wide whether one host is connected or two, so what fits there does not depend on
/// how many hosts there are — and that label is whatever the plan puts last.
pub fn icon_box(links: Links, width: u32) -> Option<Rectangle> {
	let n = u32::from(links.usb) + u32::from(links.ble);
	if n == 0 {
		return None;
	}
	let h = n * ICON.height + (n - 1) * ICON_GAP;
	Some(Rectangle::new(Point::new(icon_column(width), ICON_TOP as i32), Size::new(ICON.width, h)))
}

/// The icons' left edge, connected or not.
fn icon_column(width: u32) -> i32 {
	width as i32 - (ICON_RIGHT + ICON.width) as i32
}

fn draw_icons<D>(links: Links, width: u32, ink: BinaryColor, target: &mut D)
where
	D: DrawTarget<Color = BinaryColor>,
{
	let Some(area) = icon_box(links, width) else {
		return;
	};
	let mut at = area.top_left;
	if links.usb {
		usb_icon(at, ink, target);
		at.y += (ICON.height + ICON_GAP) as i32;
	}
	if links.ble {
		ble_icon(at, ink, target);
	}
}

/// A plug: two pins, the body, the cable.
fn usb_icon<D>(at: Point, ink: BinaryColor, target: &mut D)
where
	D: DrawTarget<Color = BinaryColor>,
{
	let stroke = PrimitiveStyle::with_stroke(ink, 1);
	for (a, b) in [((2, 0), (2, 1)), ((4, 0), (4, 1)), ((2, 6), (4, 6)), ((3, 7), (3, 8))] {
		let _ = Line::new(at + Point::from(a), at + Point::from(b)).into_styled(stroke).draw(target);
	}
	let _ = Rectangle::new(at + Point::new(1, 2), Size::new(5, 4))
		.into_styled(PrimitiveStyle::with_fill(ink))
		.draw(target);
}

/// The Bluetooth rune: the stem, and the two arrowheads crossing it.
fn ble_icon<D>(at: Point, ink: BinaryColor, target: &mut D)
where
	D: DrawTarget<Color = BinaryColor>,
{
	let stroke = PrimitiveStyle::with_stroke(ink, 1);
	for (a, b) in [((3, 0), (3, 8)), ((3, 0), (5, 2)), ((5, 2), (1, 6)), ((1, 2), (5, 6)), ((5, 6), (3, 8))] {
		let _ = Line::new(at + Point::from(a), at + Point::from(b)).into_styled(stroke).draw(target);
	}
}

// --- the adapter screen -----------------------------------------------------------------

/// The adapter screen's words, decided before any pixel so they can be tested as words.
struct AdapterText {
	/// TX then RX: `12.4 kb/s`, or `-- kb/s` while the board has not measured them.
	speeds: [Buf; 2],
	/// `500 kbit/s · normal`, `500 kbit/s · listen-only`, or `closed`.
	rate: Buf,
	/// `rx N  tx N  err N`.
	counters: Buf,
}

/// The screen's name, in its top-left corner.
const ADAPTER_TITLE: &str = "SLCAN";

fn adapter_text(state: &crate::frame::Adapter, rates: Option<Rates>) -> AdapterText {
	let mut rate = Buf::new();
	let _ = match state.kbit {
		Some(kbit) => write!(rate, "{kbit} kbit/s \u{b7} {}", if state.listen_only { "listen-only" } else { "normal" }),
		None => rate.write_str("closed"),
	};
	let speed = |bps: Option<u32>| {
		let mut buf = Buf::new();
		let _ = match bps {
			Some(bps) => kbps(&mut buf, bps),
			// Not measured is not zero traffic.
			None => buf.write_str("--"),
		};
		let _ = buf.write_str(" kb/s");
		buf
	};
	let mut counters = Buf::new();
	let _ = write!(counters, "rx {}  tx {}  err {}", state.rx, state.tx, state.errors);
	AdapterText {
		speeds: [speed(rates.map(|r| r.tx_bps)), speed(rates.map(|r| r.rx_bps))],
		rate,
		counters,
	}
}

/// Bits per second as kb/s (1 kb/s = 1000 bit/s, as CAN bit rates are counted): one
/// decimal below 100 kb/s, none from there up. Integer arithmetic, rounded half up, and a
/// value that rounds to 100.0 is written `100`.
fn kbps(buf: &mut Buf, bps: u32) -> core::fmt::Result {
	let bps = u64::from(bps);
	let tenths = (bps + 50) / 100;
	if tenths < 1000 {
		write!(buf, "{}.{}", tenths / 10, tenths % 10)
	} else {
		write!(buf, "{}", (bps + 500) / 1000)
	}
}

/// The adapter screen's three faces.
///
/// Its own rather than the theme's: the theme's numerals are digits only and its label
/// face is the smallest there is. The title is medium; the speeds are the line a person
/// reads, so they get the largest face, monospaced so a changing figure does not shift;
/// the rest is small, in a face that has `·`.
struct AdapterFonts {
	title: FontRenderer,
	speed: FontRenderer,
	small: FontRenderer,
}

impl AdapterFonts {
	fn new() -> Self {
		AdapterFonts {
			title: FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_7x13B_tr>(),
			speed: FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_9x15B_mr>(),
			small: FontRenderer::new::<u8g2_fonts::fonts::u8g2_font_6x10_tf>(),
		}
	}
}

/// Rows between two lines, and between the title and the first centred line.
const LINE_GAP: i32 = 2;
/// Between an arrow and its figure.
const ARROW_GAP: i32 = 3;
/// Between `… kb/s` and the next arrow.
const PAIR_GAP: i32 = 14;
/// Measures a face's line: ascender, descender and a stroke that reaches both.
const LINE_PROBE: &str = "Ag/|";

/// One string placed: where to anchor its top, and the box its ink covers.
#[derive(Debug, Clone, Copy)]
struct Placed {
	origin: Point,
	ink: Rectangle,
}

/// Where everything on the adapter screen lands. A line that does not fit the height is
/// `None` and not drawn.
#[derive(Debug)]
struct AdapterLayout {
	title: Option<Placed>,
	/// TX arrow, TX figure, RX arrow, RX figure.
	speed: Option<(Rectangle, Placed, Rectangle, Placed)>,
	rate: Option<Placed>,
	counters: Option<Placed>,
}

impl AdapterLayout {
	/// Every box that will have ink in it.
	fn boxes(&self) -> impl Iterator<Item = Rectangle> {
		let speed = self.speed.map(|(up, tx, down, rx)| [up, tx.ink, down, rx.ink]);
		self
			.title
			.map(|p| p.ink)
			.into_iter()
			.chain(speed.into_iter().flatten())
			.chain(self.rate.map(|p| p.ink))
			.chain(self.counters.map(|p| p.ink))
	}
}

/// The ink box of `text` anchored at the origin by its top. `None` for nothing to draw, or
/// a glyph the face lacks — which is reported.
fn ink_at_origin(font: &FontRenderer, text: &str, report: &mut Report) -> Option<Rectangle> {
	match font.get_rendered_dimensions(text, Point::zero(), VerticalPosition::Top) {
		Ok(dims) => dims.bounding_box,
		Err(_) => {
			report.glyph_missing = true;
			None
		}
	}
}

/// Place `text` so its ink starts at column `x`, on a line whose box starts at row `y`.
/// `line_dy` is where the face's line box starts below its top anchor.
fn place(font: &FontRenderer, text: &str, x: i32, y: i32, line_dy: i32, report: &mut Report) -> Option<Placed> {
	let ink = ink_at_origin(font, text, report)?;
	let origin = Point::new(x - ink.top_left.x, y - line_dy);
	Some(Placed {
		origin,
		ink: Rectangle::new(origin + ink.top_left, ink.size),
	})
}

/// The box of the pixels `text` actually lights, anchored at the origin by its top.
fn lit_box(font: &FontRenderer, text: &str) -> Option<Rectangle> {
	/// A target that keeps only the extent of what is drawn on it.
	struct Extent(Option<(Point, Point)>);

	impl Dimensions for Extent {
		fn bounding_box(&self) -> Rectangle {
			// Far wider than any glyph, on both sides of the anchor.
			Rectangle::new(Point::new(-128, -128), Size::new(256, 256))
		}
	}

	impl DrawTarget for Extent {
		type Color = BinaryColor;
		type Error = core::convert::Infallible;

		fn draw_iter<I: IntoIterator<Item = Pixel<BinaryColor>>>(&mut self, pixels: I) -> Result<(), Self::Error> {
			for Pixel(p, colour) in pixels {
				if colour.is_on() {
					self.0 = Some(match self.0 {
						None => (p, p),
						Some((lo, hi)) => (lo.component_min(p), hi.component_max(p)),
					});
				}
			}
			Ok(())
		}
	}

	let mut extent = Extent(None);
	font
		.render(
			text,
			Point::zero(),
			VerticalPosition::Top,
			FontColor::Transparent(BinaryColor::On),
			&mut extent,
		)
		.ok()?;
	extent.0.map(|(lo, hi)| Rectangle::with_corners(lo, hi))
}

/// A face's line box below its top anchor: (offset, height).
fn line_box(font: &FontRenderer) -> (i32, i32) {
	font
		.get_rendered_dimensions(LINE_PROBE, Point::zero(), VerticalPosition::Top)
		.ok()
		.and_then(|d| d.bounding_box)
		.map_or((0, 0), |b| (b.top_left.y, b.size.height as i32))
}

/// The layout, measured: `SLCAN` in the top-left corner; under it three centred lines —
/// the speeds, the bit rate and mode, the counters — as a block centred in the rows under
/// the title, starting no closer to it than [`LINE_GAP`]. A line wider than the panel is
/// reported and drawn centred anyway; a line below the floor is reported and not drawn.
fn adapter_layout(fonts: &AdapterFonts, text: &AdapterText, size: Size, report: &mut Report) -> AdapterLayout {
	let (width, height) = (size.width as i32, size.height as i32);
	// The floor is checked line by line, so a line below it is never placed; the sides are
	// checked once everything is placed, from the ink boxes.
	let fits = |top: i32, h: i32, report: &mut Report| {
		let fits = top + h <= height;
		report.label_overrun |= !fits;
		fits
	};

	// Its ink, not its anchor, sits in the corner.
	let title_ink = ink_at_origin(&fonts.title, ADAPTER_TITLE, report).unwrap_or_default();
	let title_h = title_ink.size.height as i32;
	let title = if fits(0, title_h, report) {
		place(&fonts.title, ADAPTER_TITLE, 0, 0, title_ink.top_left.y, report)
	} else {
		None
	};

	let (speed_dy, speed_h) = line_box(&fonts.speed);
	let (small_dy, small_h) = line_box(&fonts.small);
	// The arrows stand two rows shorter than a digit's lit pixels, centred on them, and are
	// as wide as tall (odd, so the apex has a middle column). Measured by drawing: u8g2's
	// dimensions are the glyph's cell, which for this face is five rows taller than the ink.
	let digit = lit_box(&fonts.speed, "0").unwrap_or_default();
	let arrow_h = (digit.size.height as i32 - 2).max(3);
	let arrow_w = arrow_h | 1;

	let block = speed_h + LINE_GAP + small_h + LINE_GAP + small_h;
	let band = title_h + LINE_GAP;
	let top = band + ((height - band - block) / 2).max(0);

	let widths = text
		.speeds
		.each_ref()
		.map(|s| ink_at_origin(&fonts.speed, s.as_str(), report).map_or(0, |b| b.size.width as i32));
	let speed_w = 2 * (arrow_w + ARROW_GAP) + widths[0] + PAIR_GAP + widths[1];
	let speed = if fits(top, speed_h, report) {
		let x = (width - speed_w) / 2;
		let arrow_top = top - speed_dy + digit.top_left.y + (digit.size.height as i32 - arrow_h) / 2;
		let arrow = |x: i32| Rectangle::new(Point::new(x, arrow_top), Size::new(arrow_w as u32, arrow_h as u32));
		let tx_x = x + arrow_w + ARROW_GAP;
		let down_x = tx_x + widths[0] + PAIR_GAP;
		let rx_x = down_x + arrow_w + ARROW_GAP;
		let tx = place(&fonts.speed, text.speeds[0].as_str(), tx_x, top, speed_dy, report);
		let rx = place(&fonts.speed, text.speeds[1].as_str(), rx_x, top, speed_dy, report);
		tx.zip(rx).map(|(tx, rx)| (arrow(x), tx, arrow(down_x), rx))
	} else {
		None
	};

	let centred = |line: &str, top: i32, report: &mut Report| {
		let w = ink_at_origin(&fonts.small, line, report).map_or(0, |b| b.size.width as i32);
		if fits(top, small_h, report) {
			place(&fonts.small, line, (width - w) / 2, top, small_dy, report)
		} else {
			None
		}
	};
	let rate_top = top + speed_h + LINE_GAP;
	let rate = centred(text.rate.as_str(), rate_top, report);
	let counters = centred(text.counters.as_str(), rate_top + small_h + LINE_GAP, report);

	let layout = AdapterLayout {
		title,
		speed,
		rate,
		counters,
	};
	let panel = Rectangle::new(Point::zero(), size);
	if layout.boxes().any(|b| panel.intersection(&b) != b) {
		report.label_overrun = true;
	}
	layout
}

/// The board as a CAN adapter (`Frame::Adapter`); the layout is [`adapter_layout`]'s.
fn adapter<D>(state: &crate::frame::Adapter, rates: Option<Rates>, target: &mut D) -> Report
where
	D: DrawTarget<Color = BinaryColor>,
{
	let mut report = Report::default();
	let fonts = AdapterFonts::new();
	let text = adapter_text(state, rates);
	let layout = adapter_layout(&fonts, &text, target.bounding_box().size, &mut report);
	let ink = BinaryColor::On;

	put(&fonts.title, ADAPTER_TITLE, layout.title, target, &mut report);
	if let Some((up, tx, down, rx)) = layout.speed {
		put(&fonts.speed, text.speeds[0].as_str(), Some(tx), target, &mut report);
		put(&fonts.speed, text.speeds[1].as_str(), Some(rx), target, &mut report);
		let fill = PrimitiveStyle::with_fill(ink);
		let (w, h) = (up.size.width as i32 - 1, up.size.height as i32 - 1);
		let (u, d) = (up.top_left, down.top_left);
		let _ = Triangle::new(u + Point::new(0, h), u + Point::new(w, h), u + Point::new(w / 2, 0))
			.into_styled(fill)
			.draw(target);
		let _ = Triangle::new(d, d + Point::new(w, 0), d + Point::new(w / 2, h))
			.into_styled(fill)
			.draw(target);
	}
	put(&fonts.small, text.rate.as_str(), layout.rate, target, &mut report);
	put(&fonts.small, text.counters.as_str(), layout.counters, target, &mut report);
	report
}

/// Draw one placed string, if it was placed.
fn put<D>(font: &FontRenderer, line: &str, placed: Option<Placed>, target: &mut D, report: &mut Report)
where
	D: DrawTarget<Color = BinaryColor>,
{
	if let Some(p) = placed
		&& font
			.render(line, p.origin, VerticalPosition::Top, FontColor::Transparent(BinaryColor::On), target)
			.is_err()
	{
		report.glyph_missing = true;
	}
}

// --- the values page --------------------------------------------------------------------

fn values<D>(cells: &[Cell<'_>], links: Links, theme: &Theme, target: &mut D) -> Report
where
	D: DrawTarget<Color = BinaryColor>,
{
	let mut report = Report::default();
	if cells.is_empty() {
		return report;
	}
	let area = target.bounding_box();
	let width = area.size.width;
	let height = area.size.height;
	// Integer division leaves up to three columns' worth of pixels unclaimed at
	// the right; give them to the last cell rather than leaving a gap, which
	// reads as a missing fifth column.
	let cell_w = width / cells.len() as u32;
	let inner = cell_w.saturating_sub(PAD * 2);
	let icons = icon_box(links, width);

	let layout = row_layout(cells, theme, inner, height, &mut report);
	if layout.step > 0 {
		report.value_shrunk = true;
	}
	if !layout.with_unit && cells.iter().any(|c| !c.unit.is_empty()) {
		report.unit_dropped = true;
	}

	for (i, cell) in cells.iter().enumerate() {
		let x = i as u32 * cell_w;
		let w = if i + 1 == cells.len() { width - x } else { cell_w };
		let rect = Rectangle::new(Point::new(x as i32, 0), Size::new(w, height));
		let ink = if cell.alarm {
			// Inverted: the ground is lit and the text is dark. The label and the
			// number both survive — they swap with the background rather than
			// being covered by it, which is the difference between "this cylinder"
			// and "something is wrong somewhere".
			let _ = target.fill_solid(&rect, BinaryColor::On);
			BinaryColor::Off
		} else {
			BinaryColor::On
		};

		let centre = x as i32 + w as i32 / 2;
		// The label's room is its column less the padding — and in the rightmost column,
		// less the icons and the padding before them.
		let left = x as i32 + PAD as i32;
		let mut right = (x + w) as i32 - PAD as i32;
		if i + 1 == cells.len()
			&& let Some(icons) = icons
		{
			right = right.min(icons.top_left.x - ICON_CLEARANCE);
		}
		draw_label(cell.label, centre, (left, right), &theme.label, ink, target, &mut report);
		draw_value(cell, centre, inner, height, theme, ink, &layout, target, &mut report);
	}
	// Last, over the rightmost column's ground: dark on an alarmed one, or they vanish.
	let ink = if cells.last().is_some_and(|c| c.alarm) {
		BinaryColor::Off
	} else {
		BinaryColor::On
	};
	draw_icons(links, width, ink, target);
	report
}

/// What the whole row agreed on: one face, one unit policy, one arrangement.
struct RowLayout {
	/// Index into the theme's numeral ladder.
	step: usize,
	/// Whether the unit is drawn at all.
	with_unit: bool,
	/// Unit under the number rather than beside it, and the number centred in
	/// what is left. Only when the height has room for all three.
	tiered: bool,
	/// Where the number's band starts and ends when `tiered`, in pixels from
	/// the top. Computed once for the row so no cell's number sits at a
	/// different level from its neighbour's — a difference the eye reads as
	/// meaning something when it means nothing.
	band: (i32, i32),
}

/// One face and one unit policy for the whole row.
///
/// A row decides as a row. Two passes: ask every cell what it needs, then give
/// all of them the same answer — the smallest face any cell required, and units
/// only if every cell can still fit one at that face.
///
/// The alternative was tried and looked wrong immediately: one cylinder kept its
/// `°` while its three neighbours dropped theirs, and one reading sat a size
/// smaller than the rest. Both differences are visible at a glance and neither
/// means anything, so the eye reads the odd cell as the important one — exactly
/// backwards on a panel whose whole job is to make the important cell obvious.
fn row_layout(cells: &[Cell<'_>], theme: &Theme, inner: u32, height: u32, report: &mut Report) -> RowLayout {
	// First ask what the numbers alone need. Stacking the unit takes it out of
	// the width competition entirely, so this is also the best face available
	// if the three tiers turn out to fit.
	let mut stacked_step = 0usize;
	for cell in cells {
		let (s, _, _) = fit(&theme.numerals, number(cell).as_str(), 0, inner);
		stacked_step = stacked_step.max(s);
	}

	let label_h = cells.iter().map(|c| text_height(&theme.label, c.label)).max().unwrap_or(0);
	let unit_h = cells
		.iter()
		.filter(|c| !c.unit.is_empty())
		.map(|c| text_height(&theme.unit, c.unit))
		.max()
		.unwrap_or(0);
	let value_h = numeral_height(&theme.numerals[stacked_step], "0");

	// One pixel of air above and below the number. Any less and the tiers touch,
	// which reads as one smeared block rather than three things.
	if label_h + value_h + unit_h + 2 <= height {
		let top = label_h as i32 + 1;
		let bottom = height as i32 - unit_h as i32 - 1;
		return RowLayout {
			step: stacked_step,
			with_unit: unit_h > 0,
			tiered: true,
			band: (top, bottom),
		};
	}

	// Not enough height: the old two-tier arrangement, unit beside the number.
	let mut step = 0usize;
	let mut with_unit = true;
	for cell in cells {
		let buf = number(cell);
		let unit_w = unit_width(cell, theme, report);
		let (s, _, u) = fit(&theme.numerals, buf.as_str(), unit_w, inner);
		step = step.max(s);
		with_unit &= u || unit_w == 0;
	}
	// Re-check at the row's step: a cell that fitted its unit beside a large
	// number still has to fit it beside the small one everybody ended up with.
	if with_unit {
		for cell in cells {
			let buf = number(cell);
			let unit_w = unit_width(cell, theme, report);
			if unit_w > 0 && measure(&theme.numerals[step], buf.as_str()) + unit_w + 2 > inner {
				with_unit = false;
			}
		}
	}
	RowLayout {
		step,
		with_unit,
		tiered: false,
		band: (0, height as i32 - 1),
	}
}

fn unit_width(cell: &Cell<'_>, theme: &Theme, report: &mut Report) -> u32 {
	if cell.unit.is_empty() {
		return 0;
	}
	let w = text_width(&theme.unit, cell.unit);
	if w == 0 {
		// The face has no glyph for it. Say so; do not simply draw nothing, which
		// is what a swallowed error looks like from the driver's seat.
		report.glyph_missing = true;
	}
	w
}

/// A label centred on `centre` inside its room, `span` = (first column, one past the last).
///
/// A label that fits is moved only as far as it must to stay inside — a short one stays over
/// its number, a long one in a narrowed column slides left. One that does not fit is
/// reported and drawn centred on its room anyway: it will collide, and the report says so.
fn draw_label<D>(label: &str, centre: i32, span: (i32, i32), font: &FontRenderer, ink: BinaryColor, target: &mut D, report: &mut Report)
where
	D: DrawTarget<Color = BinaryColor>,
{
	let (left, right) = span;
	if let Ok(dims) = font.get_rendered_dimensions(label, Point::zero(), VerticalPosition::Top)
		&& let Some(ink_box) = dims.bounding_box
	{
		let w = ink_box.size.width as i32;
		let x = if w > right - left {
			report.label_overrun = true;
			(left + right) / 2 - w / 2
		} else {
			(centre - w / 2).clamp(left, right - w)
		};
		let drawn = font.render(
			label,
			Point::new(x - ink_box.top_left.x, 0),
			VerticalPosition::Top,
			FontColor::Transparent(ink),
			target,
		);
		if drawn.is_err() {
			report.glyph_missing = true;
		}
		return;
	}
	let drawn = font.render_aligned(
		label,
		Point::new(centre, 0),
		VerticalPosition::Top,
		HorizontalAlignment::Center,
		FontColor::Transparent(ink),
		target,
	);
	if drawn.is_err() {
		report.glyph_missing = true;
	}
}

#[allow(clippy::too_many_arguments)]
fn draw_value<D>(
	cell: &Cell<'_>,
	centre: i32,
	w: u32,
	height: u32,
	theme: &Theme,
	ink: BinaryColor,
	layout: &RowLayout,
	target: &mut D,
	report: &mut Report,
) where
	D: DrawTarget<Color = BinaryColor>,
{
	let buf = number(cell);
	let text = buf.as_str();
	let numerals = &theme.numerals[layout.step];
	let value_w = measure(numerals, text);
	if value_w > w {
		report.value_overrun = true;
	}

	if layout.tiered {
		// The number is centred in its band by its glyph box, not by its
		// baseline: a baseline centred looks low, because descenders are
		// counted and digits have none.
		let value_h = numeral_height(numerals, text);
		let (top, bottom) = layout.band;
		let baseline = top + (bottom - top - value_h as i32) / 2 + value_h as i32;
		draw_numerals(numerals, text, Point::new(centre - value_w as i32 / 2, baseline), ink, target);
		if layout.with_unit && !cell.unit.is_empty() {
			// Unit last, on the floor, centred under the number. It is the
			// smallest thing on the panel and the one you look at least.
			if theme
				.unit
				.render_aligned(
					cell.unit,
					Point::new(centre, height as i32 - 1),
					VerticalPosition::Baseline,
					HorizontalAlignment::Center,
					FontColor::Transparent(ink),
					target,
				)
				.is_err()
			{
				report.glyph_missing = true;
			}
		}
		return;
	}

	let baseline = height as i32 - 1;
	let unit_w = if layout.with_unit { unit_width(cell, theme, report) } else { 0 };
	let total = if unit_w > 0 { value_w + unit_w + 2 } else { value_w };
	let left = centre - total as i32 / 2;
	draw_numerals(numerals, text, Point::new(left, baseline), ink, target);
	if unit_w > 0 {
		let _ = theme.unit.render(
			cell.unit,
			Point::new(left + value_w as i32 + 2, baseline),
			VerticalPosition::Baseline,
			FontColor::Transparent(ink),
			target,
		);
	}
}

/// Pick the largest face on the ladder that fits, and say whether the unit
/// survived.
///
/// The unit is the first thing to go, but only after every step has been tried
/// *with* it: a smaller number that keeps its unit reads better than a large one
/// whose `bar` fell off. Only when nothing on the ladder fits both does the unit
/// go, and then the largest face is taken again. If nothing fits even alone the
/// smallest is drawn and the caller is told it overran — drawn rather than
/// omitted, because a cell that renders nothing looks like a channel that did
/// not answer, and those must never be confusable.
fn fit(ladder: &[Numerals; 3], text: &str, unit_w: u32, w: u32) -> (usize, u32, bool) {
	let widths: [u32; 3] = [measure(&ladder[0], text), measure(&ladder[1], text), measure(&ladder[2], text)];
	if unit_w > 0 {
		for (i, &vw) in widths.iter().enumerate() {
			if vw + unit_w + 2 <= w {
				return (i, vw, true);
			}
		}
	}
	for (i, &vw) in widths.iter().enumerate() {
		if vw <= w {
			return (i, vw, false);
		}
	}
	(2, widths[2], false)
}

fn text_width(font: &FontRenderer, text: &str) -> u32 {
	font
		.get_rendered_dimensions(text, Point::zero(), VerticalPosition::Baseline)
		.map(|d| d.bounding_box.map(|b| b.size.width).unwrap_or(0))
		.unwrap_or(0)
}

fn text_height(font: &FontRenderer, text: &str) -> u32 {
	font
		.get_rendered_dimensions(text, Point::zero(), VerticalPosition::Baseline)
		.map(|d| d.bounding_box.map(|b| b.size.height).unwrap_or(0))
		.unwrap_or(0)
}

/// How tall the numerals stand. Measured from a digit rather than from the
/// text, so a value's height does not change as its digits do — `1.05` and
/// `188` must sit at the same level or the row ripples.
fn numeral_height(numerals: &Numerals, _text: &str) -> u32 {
	match numerals {
		Numerals::Font(font) => text_height(font, "0"),
		Numerals::Segments(style) => style.digit_size.height,
	}
}

fn measure(numerals: &Numerals, text: &str) -> u32 {
	match numerals {
		Numerals::Font(font) => text_width(font, text),
		Numerals::Segments(style) => segment_width(style, text),
	}
}

/// Seven-segment text has no font metrics to ask, so its width is arithmetic:
/// one digit plus one gap each, less the trailing gap.
fn segment_width(style: &SevenSegmentStyle<BinaryColor>, text: &str) -> u32 {
	let n = text.chars().count() as u32;
	if n == 0 {
		return 0;
	}
	n * style.digit_size.width + (n - 1) * style.digit_spacing
}

fn draw_numerals<D>(numerals: &Numerals, text: &str, at: Point, ink: BinaryColor, target: &mut D)
where
	D: DrawTarget<Color = BinaryColor>,
{
	match numerals {
		Numerals::Font(font) => {
			let _ = font.render(text, at, VerticalPosition::Baseline, FontColor::Transparent(ink), target);
		}
		Numerals::Segments(style) => {
			let mut style = *style;
			style.segment_color = Some(ink);
			let _ = Text::new(text, at, style).draw(target);
		}
	}
}

/// The chart's header rows: the header text, and the first link icon beside it. The trace
/// starts under them.
const HEADER_ROWS: i32 = (ICON_TOP + ICON.height) as i32 + 1;

/// How many columns the trace will take: one sample is one column, and there
/// are only so many columns.
///
/// A shorter trace is the truth about a run that has just started; stretching
/// it across the width would invent history. A history *deeper* than the plot
/// is the same rule from the other side — the oldest samples are never drawn,
/// so the header must not count them either. A `History<256>` on a plot 150
/// columns wide said 51 s over a picture holding 30.
fn drawn_columns(samples: usize, plot_w: i32) -> usize {
	samples.min(plot_w.max(0) as usize)
}

#[allow(clippy::too_many_arguments)]
fn chart<D>(cell: &Cell<'_>, min: f32, max: f32, samples: &[f32], seconds_per_sample: f32, links: Links, theme: &Theme, target: &mut D) -> Report
where
	D: DrawTarget<Color = BinaryColor>,
{
	let mut report = Report::default();
	let area = target.bounding_box();
	let width = area.size.width;
	let height = area.size.height;
	let ink = BinaryColor::On;

	// The header carries what a chart cannot show about itself: what it is, what
	// the vertical extent is, and how much time the width holds. Without the
	// last two a trace is a shape with no units, which is decoration.
	//
	// Drawn in two pieces because it is two alphabets: the label is a word in the
	// reader's language, the rest is `0.00-2.50bar 19s`. One face has Cyrillic
	// and the other has `°`, and no face here has both.
	let head = theme
		.label
		.render(cell.label, Point::new(0, 0), VerticalPosition::Top, FontColor::Transparent(ink), target);
	let after = match &head {
		Ok(dim) => dim.advance.x + 6,
		Err(_) => {
			report.glyph_missing = true;
			0
		}
	};
	let buf = number(cell);
	// A chart gives the number a third of the width; past that the trace has
	// nowhere left to be, and a chart with no room for its trace is a bad table.
	let (step, value_w, _) = fit(&theme.numerals, buf.as_str(), 0, width / 3);
	if step > 0 {
		report.value_shrunk = true;
	}
	let plot_top = HEADER_ROWS;
	let plot_x = value_w as i32 + 4;
	let plot_bottom = height as i32 - 1;
	// The trace ends before the icons' column **whether or not a host is connected**: the icons
	// stand one under the other, so a second one is beside the trace's top rows, and a plot
	// that widened when the last host left would change how many seconds the chart holds.
	let plot_w = icon_column(width) - ICON_CLEARANCE - plot_x;
	// The geometry is settled before the header is written, because the header
	// counts what the trace draws and the trace is only as wide as the plot.
	let drawn = drawn_columns(samples.len(), plot_w);

	let mut tail = Buf::new();
	let _ = write!(
		tail,
		"{:.*}-{:.*}{}  {:.0}s",
		cell.decimals as usize,
		min,
		cell.decimals as usize,
		max,
		cell.unit,
		drawn as f32 * seconds_per_sample
	);
	match theme.unit.render(
		tail.as_str(),
		Point::new(after, 1),
		VerticalPosition::Top,
		FontColor::Transparent(ink),
		target,
	) {
		// The header's room is the width less the icons and the padding before them.
		Ok(dim) => {
			let room = icon_box(links, width).map_or(width as i32, |icons| icons.top_left.x - ICON_CLEARANCE);
			if dim.bounding_box.is_some_and(|b| b.top_left.x + b.size.width as i32 > room) {
				report.label_overrun = true;
			}
		}
		Err(_) => report.glyph_missing = true,
	}
	draw_icons(links, width, ink, target);

	// Centred in the band under the header, for the same reason as the values
	// page: the number is what the eye came for, and on the floor it reads as an
	// afterthought under the trace.
	let value_h = numeral_height(&theme.numerals[step], buf.as_str());
	let value_baseline = plot_top + (height as i32 - 1 - plot_top - value_h as i32) / 2 + value_h as i32;
	draw_numerals(&theme.numerals[step], buf.as_str(), Point::new(0, value_baseline), ink, target);

	if plot_w < 8 || max <= min {
		report.value_overrun = plot_w < 8;
		return report;
	}

	let span = max - min;
	let usable = (plot_bottom - plot_top) as f32;
	let y_of = |v: f32| -> i32 {
		let t = ((v - min) / span).clamp(0.0, 1.0);
		plot_bottom - (t * usable) as i32
	};

	let n = drawn;
	let start = samples.len() - n;
	let line = PrimitiveStyle::with_stroke(ink, 1);
	for i in 1..n {
		let a = Point::new(plot_x + (i - 1) as i32, y_of(samples[start + i - 1]));
		let b = Point::new(plot_x + i as i32, y_of(samples[start + i]));
		let _ = Line::new(a, b).into_styled(line).draw(target);
	}
	if n == 1 {
		let p = Point::new(plot_x, y_of(samples[start]));
		let _ = Line::new(p, p).into_styled(line).draw(target);
	}
	report
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::PANEL;
	use embedded_graphics_simulator::SimulatorDisplay;

	fn panel() -> SimulatorDisplay<BinaryColor> {
		SimulatorDisplay::new(PANEL)
	}

	/// A four-column page: 256 ÷ 4, less the padding each side.
	const INNER: u32 = 64 - PAD * 2;

	fn lit(display: &SimulatorDisplay<BinaryColor>, x: i32, y: i32) -> bool {
		display.get_pixel(Point::new(x, y)) == BinaryColor::On
	}

	#[test]
	fn a_channel_that_did_not_answer_draws_a_dash_and_never_a_zero() {
		// The whole project is built against showing a number the car never gave.
		// On a panel with no room for a footnote it matters more, not less: a
		// zero here is a reading, and this is the absence of one.
		let cell = Cell::new("ОЖ", None, "°C", 0);
		assert_eq!(number(&cell).as_str(), "--");
		assert_eq!(number(&Cell::new("ОЖ", Some(0.0), "°C", 0)).as_str(), "0");
	}

	#[test]
	fn a_row_takes_one_size_for_all_of_its_cells() {
		// Four cylinders where one reading is wider than the rest. If each cell
		// chose for itself, three would be large and one small, and the small one
		// would read as the odd — that is, the important — one.
		let theme = Theme::bold_mono();
		let cells = [
			Cell::new("ЦИЛ 1", Some(-0.8), "", 1),
			Cell::new("ЦИЛ 2", Some(-2.6), "", 1),
			Cell::new("ЦИЛ 3", Some(-0.4), "", 1),
			Cell::new("ЦИЛ 4", Some(0.0), "", 1),
		];
		let mut report = Report::default();
		let step = row_layout(&cells, &theme, INNER, PANEL.height, &mut report).step;
		// The step is chosen for the widest cell...
		assert!(measure(&theme.numerals[step], "-2.6") <= INNER);
		// ...and every other cell is drawn at that same step, not at its own.
		assert!(step > 0, "this row does not fit at the largest face, so it came down");
		for cell in &cells {
			assert!(measure(&theme.numerals[step], number(cell).as_str()) <= INNER);
		}
	}

	#[test]
	fn a_row_that_cannot_be_made_to_fit_says_so_rather_than_shrinking_forever() {
		// The ladder has a bottom, and five characters in a quarter of 256 pixels
		// is past it. The right answer is a page with fewer cells, and the only
		// thing that can choose that is the generator — so it has to be told.
		let cells = [
			Cell::new("ЦИЛ 1", Some(-12.6), "", 1),
			Cell::new("ЦИЛ 2", Some(-11.4), "", 1),
			Cell::new("ЦИЛ 3", Some(-10.2), "", 1),
			Cell::new("ЦИЛ 4", Some(-13.8), "", 1),
		];
		let mut display = panel();
		let report = values(&cells, Links::NONE, &Theme::bold_mono(), &mut display);
		assert!(report.value_overrun, "{report:?}");
		// Drawn anyway. A cell that rendered nothing would look like a channel
		// that did not answer, and those two must never be confusable.
		assert!((0..64).any(|x| (8..32).any(|y| lit(&display, x, y))), "the number is still on the glass");
	}

	#[test]
	fn a_row_keeps_or_drops_its_units_together() {
		// The bug this exists for was visible in the first render: cylinder 4 kept
		// its degree sign because "0.0" is narrow, while its three neighbours lost
		// theirs. A ragged row of units is a difference that means nothing.
		let theme = Theme::bold_mono();
		let cells = [
			Cell::new("ЦИЛ 1", Some(-0.8), "°", 1),
			Cell::new("ЦИЛ 2", Some(-12.6), "°", 1),
			Cell::new("ЦИЛ 3", Some(-0.4), "°", 1),
			Cell::new("ЦИЛ 4", Some(0.0), "°", 1),
		];
		let mut report = Report::default();
		let layout = row_layout(&cells, &theme, INNER, PANEL.height, &mut report);
		let (step, with_unit) = (layout.step, layout.with_unit);
		if with_unit && !layout.tiered {
			let unit_w = text_width(&theme.unit, "°");
			for cell in &cells {
				let w = measure(&theme.numerals[step], number(cell).as_str());
				assert!(w + unit_w + 2 <= INNER, "every cell must fit its unit, or none may keep one");
			}
		}
	}

	#[test]
	fn a_taller_panel_stacks_the_unit_under_the_number() {
		// 256x64 is the part you can buy, and at that height the reference
		// panel's three tiers fit. The unit then stops competing with the number
		// for width, so it is never dropped and the face never shrinks for it.
		let theme = Theme::bold_mono();
		let cells = [
			Cell::new("МАСЛО", Some(93.0), "°C", 0),
			Cell::new("КОРОБКА", Some(72.0), "°C", 0),
			Cell::new("ОЖ", Some(93.0), "°C", 0),
			Cell::new("НАДДУВ", Some(1.82), "bar", 2),
		];
		let mut report = Report::default();
		let layout = row_layout(&cells, &theme, INNER, 64, &mut report);
		assert!(layout.tiered, "three tiers fit in 64 rows");
		assert!(layout.with_unit, "a stacked unit is never dropped for width");

		let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(256, 64));
		let report = values(&cells, Links::NONE, &theme, &mut display);
		assert!(!report.unit_dropped, "{report:?}");
		assert!(!report.value_overrun, "{report:?}");

		// The number sits in the band, not on the floor: the bottom rows belong
		// to the unit, and the rows just under the label are empty.
		let (top, bottom) = layout.band;
		assert!(
			(top..bottom).any(|y| (0..64).any(|x| lit(&display, x, y))),
			"something is drawn in the number's band"
		);
	}

	#[test]
	fn a_short_panel_keeps_the_unit_beside_the_number() {
		// The old arrangement has to survive, because 32 rows cannot stack three
		// tiers and a panel that silently drew them on top of each other would
		// be worse than one that admits the unit did not fit.
		let theme = Theme::bold_mono();
		let cells = [
			Cell::new("МАСЛО", Some(93.0), "°C", 0),
			Cell::new("КОРОБКА", Some(72.0), "°C", 0),
			Cell::new("ОЖ", Some(93.0), "°C", 0),
			Cell::new("ВПУСК", Some(46.0), "°C", 0),
		];
		let mut report = Report::default();
		let layout = row_layout(&cells, &theme, INNER, PANEL.height, &mut report);
		assert!(!layout.tiered, "three tiers do not fit in 32 rows");
	}

	#[test]
	fn a_glyph_the_face_does_not_have_is_reported_rather_than_dropped() {
		// This cost an afternoon: `u8g2`'s Cyrillic faces carry no `°`, the error
		// went into a `let _ =`, and the panel simply had no degree sign on it.
		// Text that vanishes is worse than text that overruns, because nothing on
		// the screen says it happened.
		let cells = [Cell::new("ТЕСТ", Some(1.0), "\u{2192}", 0)];
		let mut display = panel();
		let report = values(&cells, Links::NONE, &Theme::bold_mono(), &mut display);
		assert!(report.glyph_missing, "{report:?}");
	}

	#[test]
	fn an_alarm_inverts_its_own_column_and_leaves_the_others_alone() {
		// Filling the whole panel would lose the one thing the alarm view exists
		// to say — which cylinder.
		let cells = [
			Cell::new("ЦИЛ 1", Some(-0.8), "", 1),
			Cell::new("ЦИЛ 2", Some(-2.6), "", 1).alarmed(),
			Cell::new("ЦИЛ 3", Some(-0.4), "", 1),
			Cell::new("ЦИЛ 4", Some(0.0), "", 1),
		];
		let mut display = panel();
		values(&cells, Links::NONE, &Theme::bold_mono(), &mut display);
		// The top-left corner of the alarmed column is lit ground; the same corner
		// of its neighbours is not.
		assert!(lit(&display, 65, 0), "the alarmed cell's ground is lit");
		assert!(!lit(&display, 1, 0), "its left neighbour's is not");
		assert!(!lit(&display, 129, 0), "nor its right neighbour's");
		// And the label survived: somewhere in the alarmed column's label row
		// there is an unlit pixel, which is the dark text on the lit ground.
		assert!((64..128).any(|x| !lit(&display, x, 3)), "the label is drawn dark on the lit ground");
	}

	#[test]
	fn a_chart_draws_only_the_samples_it_has() {
		// Every run looks like this in its first seconds. Stretching eight points
		// across the width would invent history.
		let samples = [0.1f32, 0.3, 0.6, 0.9, 1.1, 1.3, 1.4, 1.5];
		let frame = Frame::Chart {
			cell: Cell::new("НАДДУВ", Some(1.5), "bar", 2),
			min: 0.0,
			max: 2.5,
			samples: &samples,
			seconds_per_sample: 0.2,
		};
		let mut display = panel();
		draw(&frame, &Theme::bold_mono(), &mut display);
		// The trace occupies at most `samples.len()` columns. Well past that, and
		// below the header, nothing is drawn.
		let far = PANEL.width as i32 - 4;
		assert!(
			(10..PANEL.height as i32).all(|y| !lit(&display, far, y)),
			"no trace where there is no data"
		);
	}

	#[test]
	fn a_chart_header_counts_the_columns_it_draws_and_not_the_history_it_holds() {
		// A full `History<256>` on a plot narrower than that: the oldest samples
		// are never drawn, so the seconds the header claims are the seconds on
		// the glass, not the seconds in RAM.
		const PERIOD: f32 = 0.2;
		let samples: [f32; 256] = core::array::from_fn(|i| (i % 20) as f32 / 10.0);
		let theme = Theme::bold_mono();
		let cell = || Cell::new("НАДДУВ", Some(1.5), "bar", 2);
		// The geometry `chart` works out: the number takes what it takes, the
		// plot is the rest.
		let (_, value_w, _) = fit(&theme.numerals, number(&cell()).as_str(), 0, PANEL.width / 3);
		let plot_w = PANEL.width as i32 - (value_w as i32 + 4);
		let drawn = drawn_columns(samples.len(), plot_w);
		assert_eq!(drawn, plot_w as usize, "this panel is narrower than the history is deep");
		assert!(drawn < samples.len());

		let mut header = Buf::new();
		let _ = write!(header, "{:.0}s", drawn as f32 * PERIOD);
		let mut whole_history = Buf::new();
		let _ = write!(whole_history, "{:.0}s", samples.len() as f32 * PERIOD);
		assert_eq!(whole_history.as_str(), "51s");
		assert_ne!(header.as_str(), whole_history.as_str(), "51 s is what is held, not what is shown");

		// And the picture agrees with the header: the same frame given only the
		// newest `plot_w` samples draws pixel for pixel the same thing.
		let mut full = panel();
		draw(
			&Frame::Chart {
				cell: cell(),
				min: 0.0,
				max: 2.5,
				samples: &samples,
				seconds_per_sample: PERIOD,
			},
			&theme,
			&mut full,
		);
		let mut trimmed = panel();
		draw(
			&Frame::Chart {
				cell: cell(),
				min: 0.0,
				max: 2.5,
				samples: &samples[samples.len() - drawn..],
				seconds_per_sample: PERIOD,
			},
			&theme,
			&mut trimmed,
		);
		for y in 0..PANEL.height as i32 {
			for x in 0..PANEL.width as i32 {
				assert_eq!(lit(&full, x, y), lit(&trimmed, x, y), "differs at {x},{y}");
			}
		}
	}

	#[test]
	fn a_chart_header_is_not_truncated_by_its_buffer() {
		// Sixteen bytes looked ample for `"1234"` and cut `"НАДДУВ 0.00-2.50bar"`
		// after nine characters, because Cyrillic costs two bytes each.
		let mut buf = Buf::new();
		let unit = Cell::new("НАДДУВ", Some(1.82), "bar", 2).unit;
		let r = write!(buf, "{:.*}-{:.*}{}  {:.0}s", 2, 0.0, 2, 2.5, unit, 19.0);
		assert!(r.is_ok());
		assert_eq!(buf.as_str(), "0.00-2.50bar  19s");
	}

	#[test]
	fn a_label_wider_than_its_column_is_reported() {
		// Reported, not clipped: the generator that chose the label is the thing
		// that can shorten it, and it only learns from here.
		let cells = [
			Cell::new("ОЧЕНЬ ДЛИННАЯ ПОДПИСЬ", Some(93.0), "", 0),
			Cell::new("ОЖ", Some(93.0), "", 0),
			Cell::new("ВПУСК", Some(46.0), "", 0),
			Cell::new("МАСЛО", Some(93.0), "", 0),
		];
		let mut display = panel();
		let report = values(&cells, Links::NONE, &Theme::bold_mono(), &mut display);
		assert!(report.label_overrun, "{report:?}");
	}

	// --- the link icons ---------------------------------------------------------------

	/// The panel the board has.
	const TALL: Size = Size::new(256, 64);

	fn tall() -> SimulatorDisplay<BinaryColor> {
		SimulatorDisplay::new(TALL)
	}

	const USB: Links = Links { usb: true, ble: false };
	const BLE: Links = Links { usb: false, ble: true };
	const BOTH: Links = Links { usb: true, ble: true };

	fn lit_in(display: &SimulatorDisplay<BinaryColor>, area: Rectangle) -> bool {
		area.points().any(|p| display.get_pixel(p) == BinaryColor::On)
	}

	/// Inside the icon box, `display` is pixel for pixel the icons drawn alone.
	fn icon_box_holds_only_the_icons(display: &SimulatorDisplay<BinaryColor>, links: Links) -> bool {
		let mut alone = tall();
		draw_icons(links, TALL.width, BinaryColor::On, &mut alone);
		icon_box(links, TALL.width)
			.unwrap()
			.points()
			.all(|p| display.get_pixel(p) == alone.get_pixel(p))
	}

	/// A real plan's page: labels as long as the plan's, two kinds of unit.
	fn temps<'a>(last: &'a str) -> [Cell<'a>; 4] {
		[
			Cell::new("ОЖ", Some(93.0), "°C", 0),
			Cell::new("НАДДУВ", Some(1.82), "bar", 2),
			Cell::new("МАСЛО", Some(104.0), "°C", 0),
			Cell::new(last, Some(78.0), "°C", 0),
		]
	}

	#[test]
	fn the_icons_stand_in_a_column_two_pixels_down_four_in_and_two_apart() {
		assert_eq!(icon_box(Links::NONE, 256), None, "nothing connected, nothing drawn");
		assert_eq!(icon_box(USB, 256), Some(Rectangle::new(Point::new(245, 2), ICON)));
		assert_eq!(icon_box(BLE, 256), Some(Rectangle::new(Point::new(245, 2), ICON)));
		assert_eq!(icon_box(BOTH, 256), Some(Rectangle::new(Point::new(245, 2), Size::new(7, 20))));
	}

	#[test]
	fn each_icon_draws_inside_its_own_cell_and_nowhere_else() {
		let mut none = tall();
		draw_icons(Links::NONE, TALL.width, BinaryColor::On, &mut none);
		assert!(!lit_in(&none, none.bounding_box()), "no link, no pixel");

		// Every lit pixel is inside the icon box, so the two dark rows above it and the two
		// dark columns right of it are checked by the loop below.
		let right = Rectangle::new(Point::new(245, 2), ICON);
		for links in [USB, BLE, BOTH] {
			let mut display = tall();
			draw_icons(links, TALL.width, BinaryColor::On, &mut display);
			let area = icon_box(links, TALL.width).unwrap();
			for p in display.bounding_box().points() {
				assert!(
					display.get_pixel(p) == BinaryColor::Off || area.contains(p),
					"{links:?}: {p:?} lit outside {area:?}"
				);
			}
			assert!(lit_in(&display, right), "{links:?}: the corner cell has its icon");
			if links == BOTH {
				assert!(lit_in(&display, Rectangle::new(Point::new(245, 13), ICON)), "and the one under it");
				assert!(!lit_in(&display, Rectangle::new(Point::new(245, 11), Size::new(7, 2))), "the gap is dark");
			}
		}

		let (mut usb, mut ble) = (tall(), tall());
		draw_icons(USB, TALL.width, BinaryColor::On, &mut usb);
		draw_icons(BLE, TALL.width, BinaryColor::On, &mut ble);
		assert!(right.points().any(|p| usb.get_pixel(p) != ble.get_pixel(p)), "two different pictures");
	}

	#[test]
	fn a_label_never_lights_a_pixel_inside_the_icon_box() {
		for links in [USB, BLE, BOTH] {
			let mut display = tall();
			let report = values(&temps("КОРОБКА"), links, &Theme::bold_mono(), &mut display);
			assert!(!report.label_overrun && !report.glyph_missing, "{links:?}: {report:?}");
			assert!(icon_box_holds_only_the_icons(&display, links), "{links:?}");
			// `КОРОБКА` is 34 px and the room beside the icons 46: it fits, and the clearance
			// before the icons stays dark.
			let area = icon_box(links, TALL.width).unwrap();
			let clearance = Rectangle::new(
				Point::new(area.top_left.x - ICON_CLEARANCE, 0),
				Size::new(ICON_CLEARANCE as u32, ICON_TOP + ICON.height),
			);
			assert!(!lit_in(&display, clearance), "{links:?}: the clearance before the icons is dark");
		}
	}

	#[test]
	fn a_short_rightmost_label_stays_over_its_number_beside_the_icons() {
		let cells = temps("ОЖ");
		let (mut plain, mut linked) = (tall(), tall());
		values(&cells, Links::NONE, &Theme::bold_mono(), &mut plain);
		values(&cells, BOTH, &Theme::bold_mono(), &mut linked);
		let label_row = Rectangle::new(Point::new(192, 0), Size::new(40, 8));
		assert!(
			label_row.points().all(|p| plain.get_pixel(p) == linked.get_pixel(p)),
			"a label with room to spare does not move"
		);
	}

	#[test]
	fn a_second_host_takes_no_more_room_from_the_rightmost_label() {
		for last in ["ОЖ", "НАДДУВ", "КОРОБКА", "ТЕМП.МАСЛА"] {
			let cells = temps(last);
			let (mut one, mut two) = (tall(), tall());
			let alone = values(&cells, USB, &Theme::bold_mono(), &mut one);
			let beside = values(&cells, BOTH, &Theme::bold_mono(), &mut two);
			assert_eq!(alone.label_overrun, beside.label_overrun, "{last}");
			let labels = Rectangle::new(Point::new(0, 0), Size::new(241, ICON_TOP + ICON.height));
			assert!(
				labels.points().all(|p| one.get_pixel(p) == two.get_pixel(p)),
				"{last}: the label row did not move"
			);
		}
	}

	#[test]
	fn a_value_never_lights_a_pixel_beside_the_icons() {
		for n in 1..=4 {
			for (value, decimals) in [(78.0, 0), (1234.0, 0), (12345.0, 0), (104.5, 1), (-40.5, 1)] {
				let cells: std::vec::Vec<Cell<'_>> = (0..n).map(|_| Cell::new("ОЖ", Some(value), "°C", decimals)).collect();
				let mut display = tall();
				values(&cells, BOTH, &Theme::bold_mono(), &mut display);
				let mut alone = tall();
				draw_icons(BOTH, TALL.width, BinaryColor::On, &mut alone);
				let area = icon_box(BOTH, TALL.width).unwrap();
				let around = Rectangle::new(
					area.top_left - Point::new(ICON_CLEARANCE, 0),
					area.size + Size::new(ICON_CLEARANCE as u32, ICON_GAP),
				);
				assert!(
					around.points().all(|p| display.get_pixel(p) == alone.get_pixel(p)),
					"{n} cells of {value}: a value reached the icons"
				);
			}
		}
	}

	#[test]
	fn a_rightmost_label_that_fits_only_without_the_icons_is_reported_with_them() {
		let cells = temps("ТЕМП.МАСЛА");
		let mut display = tall();
		let alone = values(&cells, Links::NONE, &Theme::bold_mono(), &mut display);
		assert!(!alone.label_overrun, "it fits its column: {alone:?}");
		let mut display = tall();
		let beside = values(&cells, BOTH, &Theme::bold_mono(), &mut display);
		assert!(beside.label_overrun, "it does not fit beside the icons: {beside:?}");
	}

	#[test]
	fn an_alarmed_rightmost_column_draws_its_icons_dark() {
		let cells = [Cell::new("ОЖ", Some(93.0), "°C", 0), Cell::new("КОРОБКА", Some(78.0), "°C", 0).alarmed()];
		let mut display = tall();
		values(&cells, USB, &Theme::bold_mono(), &mut display);
		assert!(lit(&display, 245, 5), "the ground beside the plug is lit");
		assert!(!lit(&display, 247, 5), "the plug's body is dark on it");
	}

	#[test]
	fn a_chart_header_gives_way_to_the_icons_and_the_trace_ends_before_their_column() {
		// Pinned at the top of the scale, so the trace runs along the highest row it can.
		let samples = [2.5f32; 240];
		fn pinned<'a>(label: &'a str, samples: &'a [f32]) -> Frame<'a> {
			Frame::Chart {
				cell: Cell::new(label, Some(2.5), "bar", 2),
				min: 0.0,
				max: 2.5,
				samples,
				seconds_per_sample: 0.2,
			}
		}
		let chart = |label: &'static str| pinned(label, &samples);
		let board = Board { links: BOTH, rates: None };
		let mut display = tall();
		let report = draw_with(&chart("НАДДУВ"), &board, &Theme::bold_mono(), &mut display);
		assert!(!report.label_overrun, "{report:?}");
		assert!(icon_box_holds_only_the_icons(&display, BOTH));
		let end = icon_column(TALL.width) - ICON_CLEARANCE;
		assert!(lit(&display, end - 1, HEADER_ROWS), "the trace runs along its top row to the clearance");
		assert!(
			(end..TALL.width as i32)
				.all(|x| (HEADER_ROWS..TALL.height as i32).all(|y| !lit(&display, x, y) || icon_box(BOTH, TALL.width).unwrap().contains(Point::new(x, y)))),
			"nothing of the trace in the clearance or the icons' column"
		);
		let mut plain = tall();
		draw(&chart("НАДДУВ"), &Theme::bold_mono(), &mut plain);
		let under = Rectangle::new(Point::new(0, HEADER_ROWS), Size::new(end as u32, TALL.height - HEADER_ROWS as u32));
		assert!(
			under.points().all(|p| plain.get_pixel(p) == display.get_pixel(p)),
			"the trace is the same picture with no host connected"
		);

		// Lengthen the label a letter at a time: the icons never let a longer header through,
		// and some length fits the whole width but not the width less the icons.
		let mut collided = false;
		for n in 1..40 {
			let long: std::string::String = core::iter::repeat_n('Ж', n).collect();
			let alone = draw(&pinned(&long, &samples), &Theme::bold_mono(), &mut tall()).label_overrun;
			let beside = draw_with(&pinned(&long, &samples), &board, &Theme::bold_mono(), &mut tall()).label_overrun;
			assert!(!alone || beside, "{n} letters: the icons only take room away");
			collided |= beside && !alone;
		}
		assert!(collided, "some header fits the panel but not beside the icons");
	}

	// --- the adapter screen -----------------------------------------------------------

	use crate::frame::Adapter;

	fn adapter_state(kbit: Option<u32>, listen_only: bool) -> Adapter {
		Adapter {
			kbit,
			listen_only,
			rx: 4_294_967_295,
			tx: 12,
			errors: 0,
		}
	}

	fn words(text: &AdapterText) -> [&str; 4] {
		[
			text.speeds[0].as_str(),
			text.speeds[1].as_str(),
			text.rate.as_str(),
			text.counters.as_str(),
		]
	}

	#[test]
	fn the_adapter_screen_says_the_speeds_the_rate_the_mode_and_the_counters() {
		let rates = Some(Rates {
			tx_bps: 12_400,
			rx_bps: 380_200,
		});
		let open = adapter_text(&adapter_state(Some(500), false), rates);
		assert_eq!(
			words(&open),
			["12.4 kb/s", "380 kb/s", "500 kbit/s \u{b7} normal", "rx 4294967295  tx 12  err 0"]
		);

		let silent = adapter_text(&adapter_state(Some(125), true), rates);
		assert_eq!(silent.rate.as_str(), "125 kbit/s \u{b7} listen-only");

		let closed = adapter_text(&adapter_state(None, true), None);
		assert_eq!(
			words(&closed)[..3],
			["-- kb/s", "-- kb/s", "closed"],
			"a closed channel has no mode, and a rate not measured is not a zero"
		);
	}

	#[test]
	fn kb_per_s_has_one_decimal_below_a_hundred_and_none_from_there() {
		for (bps, text) in [
			(0, "0.0"),
			(49, "0.0"),
			(50, "0.1"),
			(12_400, "12.4"),
			(99_949, "99.9"),
			(99_950, "100"),
			(380_200, "380"),
			(1_000_000, "1000"),
			(u32::MAX, "4294967"),
		] {
			let mut buf = Buf::new();
			kbps(&mut buf, bps).unwrap();
			assert_eq!(buf.as_str(), text, "{bps} bit/s");
		}
	}

	/// The adapter screen drawn on `size`, with the layout it was drawn from.
	fn adapter_on(size: Size, state: Adapter, rates: Option<Rates>) -> (SimulatorDisplay<BinaryColor>, Report, AdapterLayout) {
		let mut display = SimulatorDisplay::new(size);
		let board = Board { links: Links::NONE, rates };
		let report = draw_with(&Frame::Adapter(state), &board, &Theme::bold_mono(), &mut display);
		let layout = adapter_layout(&AdapterFonts::new(), &adapter_text(&state, rates), size, &mut Report::default());
		(display, report, layout)
	}

	fn lit_only_inside(display: &SimulatorDisplay<BinaryColor>, boxes: &[Rectangle]) {
		for p in display.bounding_box().points() {
			if display.get_pixel(p) == BinaryColor::On {
				assert!(boxes.iter().any(|b| b.contains(p)), "{p:?} is lit outside every box");
			}
		}
	}

	#[test]
	fn the_adapter_screen_fits_the_panel_and_nothing_overlaps() {
		// Eight-digit counters and a megabit each way: past anything a session reaches.
		let state = Adapter {
			kbit: Some(1000),
			listen_only: true,
			rx: 99_999_999,
			tx: 99_999_999,
			errors: 99_999_999,
		};
		let rates = Some(Rates {
			tx_bps: 1_000_000,
			rx_bps: 1_000_000,
		});
		let (display, report, layout) = adapter_on(TALL, state, rates);
		assert_eq!(report, Report::default(), "nothing overran and no glyph was missing");

		let boxes: std::vec::Vec<Rectangle> = layout.boxes().collect();
		assert_eq!(boxes.len(), 7, "every piece was placed: {layout:?}");
		let panel = Rectangle::new(Point::zero(), TALL);
		for (i, a) in boxes.iter().enumerate() {
			assert_eq!(panel.intersection(a), *a, "{a:?} is inside the panel");
			for b in &boxes[i + 1..] {
				assert_eq!(a.intersection(b).size, Size::zero(), "{a:?} and {b:?} overlap");
			}
		}
		lit_only_inside(&display, &boxes);

		// `SLCAN` in the corner; the rest centred, in order down the panel.
		let title = layout.title.unwrap().ink;
		assert_eq!(title.top_left, Point::zero());
		let (up, _, _, rx) = layout.speed.unwrap();
		let (rate, counters) = (layout.rate.unwrap().ink, layout.counters.unwrap().ink);
		let centred = |left: i32, right: i32| (left - (TALL.width as i32 - right)).abs() <= 1;
		assert!(centred(up.top_left.x, rx.ink.top_left.x + rx.ink.size.width as i32), "{up:?} {rx:?}");
		for line in [rate, counters] {
			assert!(centred(line.top_left.x, line.top_left.x + line.size.width as i32), "{line:?}");
		}
		assert!(title.top_left.y < up.top_left.y && up.top_left.y < rate.top_left.y && rate.top_left.y < counters.top_left.y);
	}

	#[test]
	fn a_counter_line_wider_than_the_panel_is_reported() {
		let state = Adapter {
			kbit: Some(500),
			listen_only: false,
			rx: u32::MAX,
			tx: u32::MAX,
			errors: u32::MAX,
		};
		let (_, report, layout) = adapter_on(TALL, state, None);
		assert!(report.label_overrun, "{report:?}");
		assert!(
			layout.counters.is_some_and(|c| c.ink.size.width > TALL.width),
			"it is the counters, drawn anyway"
		);
	}

	#[test]
	fn on_a_short_panel_what_does_not_fit_the_height_is_reported_not_drawn_over() {
		let (display, report, layout) = adapter_on(PANEL, adapter_state(Some(500), false), None);
		assert!(report.label_overrun, "the lower lines have no room on 32 pixels: {report:?}");
		assert!(!report.glyph_missing, "{report:?}");
		assert!(layout.title.is_some() && layout.speed.is_some(), "the title and the speeds fit");
		assert!(layout.counters.is_none(), "the counters do not, and are not drawn");
		lit_only_inside(&display, &layout.boxes().collect::<std::vec::Vec<_>>());
	}
}
