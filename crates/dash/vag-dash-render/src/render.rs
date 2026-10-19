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

use crate::frame::{Board, Cell, Deviation, Faults, Frame, Rates, Strip, Volts};
use crate::theme::{Numerals, Theme};

/// Breathing room each side of a cell's contents.
///
/// Without it a four-digit reading fills its column edge to edge and touches its
/// neighbour, and two numbers with no gap between them read as one number. Three
/// pixels is the smallest gap that still separates at a glance — which is the
/// only kind of look this panel ever gets.
const PAD: u32 = 3;

/// Rows kept clear above the label and below the unit on a values page.
///
/// Owner, 2026-09-15: flush with the top and bottom edges the two small lines read as falling
/// off the glass; pulled in, the cell reads as one block.
const ROW_INSET: i32 = 2;

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
	/// A cell had a specified value to show and the panel had no room for the line.
	///
	/// The number and the unit survive; only the difference goes. A plan that sees this on
	/// the board's own panel has asked for a page the glass cannot hold.
	pub deviation_dropped: bool,
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

/// Rows between the sign and the digits it qualifies.
const SIGN_GAP: i32 = 1;

/// The difference as it is drawn: the sign a pixel smaller than the digits, across and down
/// (owner, 2026-09-15), so `+` and `-` do not shout as loudly as the number.
struct Difference {
	/// Whether the sign is a minus. `None` for the dash of a pair that has not answered — a
	/// dash is not a sign.
	sign: Option<bool>,
	digits: Buf,
}

/// The sign's box: a pixel shorter than the digits beside it, and as wide as it is tall.
///
/// Drawn from two lines rather than set from a face, because a face's `+` is whatever size
/// that face is — this one has to be one pixel smaller than the digits, whichever face they
/// are in.
fn sign_size(theme: &Theme) -> i32 {
	(text_height(&theme.label, "0") as i32 - 1).max(3)
}

/// The sign, sitting on `baseline` with its left edge at `left`.
fn draw_sign<D>(negative: bool, left: i32, baseline: i32, size: i32, ink: BinaryColor, target: &mut D)
where
	D: DrawTarget<Color = BinaryColor>,
{
	let stroke = PrimitiveStyle::with_stroke(ink, 1);
	let top = baseline - size + 1;
	let middle = baseline - size / 2;
	let _ = Line::new(Point::new(left, middle), Point::new(left + size - 1, middle))
		.into_styled(stroke)
		.draw(target);
	if !negative {
		let _ = Line::new(Point::new(left + size / 2, top), Point::new(left + size / 2, baseline))
			.into_styled(stroke)
			.draw(target);
	}
}

/// What a cell's difference reads as: `None` where the channel has no specified value at all
/// and the line is not drawn.
fn difference(cell: &Cell<'_>) -> Option<Difference> {
	let mut digits = Buf::new();
	match cell.deviation {
		Deviation::None => None,
		Deviation::Unknown => {
			let _ = digits.write_str("--");
			Some(Difference { sign: None, digits })
		}
		// Always signed: the sign is half the reading. `+0.07` is the unit getting more than
		// it asked for, `-0.07` less.
		Deviation::Value(v) => {
			let magnitude = if v < 0.0 { -v } else { v };
			let _ = write!(digits, "{:.*}", cell.decimals as usize, magnitude);
			Some(Difference { sign: Some(v < 0.0), digits })
		}
	}
}

/// How wide it is drawn, the sign and its gap included.
fn difference_width(theme: &Theme, text: &Difference) -> u32 {
	let sign = text.sign.map_or(0, |_| (sign_size(theme) + SIGN_GAP) as u32);
	sign + text_width(&theme.label, text.digits.as_str())
}

/// Draws it on `baseline`, centred on `centre`.
fn draw_difference<D>(text: &Difference, centre: i32, baseline: i32, theme: &Theme, ink: BinaryColor, target: &mut D, report: &mut Report)
where
	D: DrawTarget<Color = BinaryColor>,
{
	let mut x = centre - difference_width(theme, text) as i32 / 2;
	if let Some(negative) = text.sign {
		let size = sign_size(theme);
		draw_sign(negative, x, baseline, size, ink, target);
		x += size + SIGN_GAP;
	}
	if theme
		.label
		.render(
			text.digits.as_str(),
			Point::new(x, baseline),
			VerticalPosition::Baseline,
			FontColor::Transparent(ink),
			target,
		)
		.is_err()
	{
		report.glyph_missing = true;
	}
}

/// Draw one frame alone on the whole target: no strip, nothing of the board's, no rates on the
/// adapter screen. The layout as it is designed, for pictures and tests; the board draws
/// [`draw_page`]. Returns what did not fit.
pub fn draw<D>(frame: &Frame<'_>, theme: &Theme, target: &mut D) -> Report
where
	D: DrawTarget<Color = BinaryColor>,
{
	draw_frame(frame, None, theme, target, true)
}

/// [`draw_page`] with no title and no voltage: a values or chart frame under a strip that holds
/// only what the board says about itself — the stopwatch page is drawn so — or the adapter
/// screen with the bus rates. Returns what did not fit.
pub fn draw_with<D>(frame: &Frame<'_>, board: &Board, theme: &Theme, target: &mut D) -> Report
where
	D: DrawTarget<Color = BinaryColor>,
{
	draw_page(frame, &Strip::NONE, board, theme, target)
}

/// Rows the status strip takes at the top of the panel: the label face's line and a dark row
/// under it, with no rule (owner, 2026-10-19). The page is drawn in the rows under them.
pub const STRIP_ROWS: u32 = 9;

/// The strip's voltage stands this far in from the left edge.
const STRIP_LEFT: i32 = 1;

/// Draw one page as the board shows it: the [`Strip`] across the top — the voltage at the left,
/// the title in the middle, the link runes and the fault badge at the right end — and the frame
/// in the rows under it, on the panel's whole width.
///
/// Every values and chart page has the strip, whatever there is to fill it (2026-10-19): a
/// page with no title and no voltage still has the board's runes and badge to show, and they
/// come and go — a host connects, the fault count arrives ten seconds after boot. Drawn in the
/// strip they change the strip and nothing under it, so a page's geometry is the page's alone.
/// The adapter screen, which is no plan page, has no strip: it is drawn on the whole panel with
/// the bus rates, and no runes or badge — its `SLCAN` says the host is on the cable.
///
/// With a title in the strip, a chart's header drops its label — the title says it — and
/// keeps its range and seconds.
pub fn draw_page<D>(frame: &Frame<'_>, strip: &Strip<'_>, board: &Board, theme: &Theme, target: &mut D) -> Report
where
	D: DrawTarget<Color = BinaryColor>,
{
	if let Frame::Adapter(_) = frame {
		return draw_frame(frame, board.rates, theme, target, true);
	}
	let area = target.bounding_box();
	// The renderer reads its height from the target, so the page under the strip is the page
	// on a shorter panel.
	let below = Rectangle::new(
		area.top_left + Point::new(0, STRIP_ROWS as i32),
		Size::new(area.size.width, area.size.height.saturating_sub(STRIP_ROWS)),
	);
	let mut report = draw_frame(frame, None, theme, &mut target.cropped(&below), strip.title.is_empty());
	draw_strip(strip, board, theme, target, &mut report);
	report
}

/// What the strip writes for the voltage: one decimal and `V` — `14.2V` — or `--V` while the
/// channel has not answered, the dash a cell draws for a missing value.
fn volts_text(volts: Option<f32>) -> Buf {
	let mut buf = Buf::new();
	let _ = match volts {
		Some(v) => write!(buf, "{v:.1}V"),
		None => buf.write_str("--V"),
	};
	buf
}

/// The strip: the voltage left-aligned at [`STRIP_LEFT`], the title centred on the panel, both
/// in the label face from the top row, and the right end ([`corner`]). A title that runs into
/// the voltage, or into the right end as this frame has it — the badge's ground included — is
/// reported and drawn anyway, as a label that overruns its column is.
fn draw_strip<D>(strip: &Strip<'_>, board: &Board, theme: &Theme, target: &mut D, report: &mut Report)
where
	D: DrawTarget<Color = BinaryColor>,
{
	let width = target.bounding_box().size.width;
	let ink = FontColor::Transparent(BinaryColor::On);
	// One past the voltage's last lit column: the title has to start past it.
	let mut taken = 0;
	if let Volts::Shown(volts) = strip.volts {
		let text = volts_text(volts);
		match theme
			.label
			.render(text.as_str(), Point::new(STRIP_LEFT, 0), VerticalPosition::Top, ink, target)
		{
			Ok(dims) => taken = dims.bounding_box.map_or(0, |b| b.top_left.x + b.size.width as i32),
			Err(_) => report.glyph_missing = true,
		}
	}
	let corner = corner(board, theme, width, report);
	draw_corner(&corner, theme, target, report);
	if strip.title.is_empty() {
		return;
	}
	// The title's last lit column has a dark one after it before the right end, as its first
	// has one after the voltage; with nothing at the right end, the panel's edge is the limit.
	let limit = corner.left.map_or(width as i32, |left| left - 1);
	// A title too long for the room it has is cut there, never drawn over the voltage or the
	// right end: a host connecting narrows the room, and the board's own state stays readable
	// whatever the title (review, 2026-10-19). The overrun is reported either way.
	let room = Rectangle::new(Point::new(taken + 1, 0), Size::new((limit - taken - 1).max(0) as u32, STRIP_ROWS));
	match theme.label.render_aligned(
		strip.title,
		Point::new(width as i32 / 2, 0),
		VerticalPosition::Top,
		HorizontalAlignment::Center,
		ink,
		&mut target.clipped(&room),
	) {
		Ok(ink_box) => {
			if let Some(b) = ink_box
				&& (b.top_left.x <= taken || b.top_left.x + b.size.width as i32 > limit)
			{
				report.label_overrun = true;
			}
		}
		Err(_) => report.glyph_missing = true,
	}
}

/// The frame on whatever target [`draw`] or [`draw_page`] gives it: the bus rates for the
/// adapter screen, and whether a chart's header carries the channel's label — not under a
/// strip that carries the page's title.
fn draw_frame<D>(frame: &Frame<'_>, rates: Option<Rates>, theme: &Theme, target: &mut D, chart_label: bool) -> Report
where
	D: DrawTarget<Color = BinaryColor>,
{
	match frame {
		Frame::Values { cells } => values(cells, theme, target),
		Frame::Chart {
			cell,
			min,
			max,
			samples,
			seconds_per_sample,
		} => chart(cell, *min, *max, samples, *seconds_per_sample, chart_label, theme, target),
		Frame::Adapter(state) => adapter(state, rates, target),
	}
}

// --- the strip's right end ----------------------------------------------------------------

/// The warning triangle, row by row: a filled triangle with the `!` cut out of it — the stroke,
/// a row of ink, the dot — six rows tall and seven wide. Six rows are the label face's digits,
/// so it stands on the strip's line with the count beside it. Drawn from rows, as the runes
/// are: no face here has one.
pub const TRIANGLE: [&str; 6] = ["...#...", "..#.#..", "..#.#..", ".#####.", ".##.##.", "#######"];

/// The USB plug, row by row: two pins, the body, the neck and the cable, in eight rows — the
/// strip's whole line: the row over the digits, the digits' six, and the row a descender takes.
pub const USB_RUNE: [&str; 8] = [".#.#.", ".#.#.", "#####", "#####", "#####", "#####", ".###.", "..#.."];

/// The Bluetooth rune: the stem, and the two arrowheads crossing it on the stem, in the plug's
/// eight rows. Five columns is the narrowest it keeps its arrowheads clear of the stem's tails,
/// and an even height is what puts the crossing on the stem.
pub const BLE_RUNE: [&str; 8] = ["..#..", "#.##.", ".##.#", "..##.", "..##.", ".##.#", "#.##.", "..#.."];

/// A rune's columns.
const RUNE_W: i32 = 5;

/// Dark columns right of the right end: the voltage's margin at the left edge, mirrored.
const STRIP_RIGHT: i32 = STRIP_LEFT;

/// Dark columns between the plug and the Bluetooth rune.
const RUNE_GAP: i32 = 3;

/// Dark columns between the badge and the runes: wider than between two runes, so the badge
/// reads as a thing of its own and not as a third link.
const BADGE_GAP: i32 = 6;

/// Dark columns between the triangle and the count's first character.
const COUNT_GAP: i32 = 2;

/// Ground round the badge's ink — lit when a code is failing now — above, below and left of it;
/// on the right the count's last character cell has a blank column of its own.
const BADGE_MARGIN: i32 = 1;

/// Where the strip's right end puts its parts, laid out from the panel's right edge leftwards:
/// the Bluetooth rune, the plug left of it, and the fault badge — the triangle and the count —
/// left of those. A part with nothing to show takes no room and the rest close up to the edge,
/// as the link icons closed up in the column they had. The page under the strip never moves
/// for any of it.
struct Corner {
	usb: Option<Point>,
	ble: Option<Point>,
	badge: Option<Badge>,
	/// The right end's first column: the badge's ground's — the same whether or not a code is
	/// failing now — else the leftmost rune's. `None` when there is nothing to show.
	left: Option<i32>,
}

/// Where the badge's parts land.
struct Badge {
	/// The triangle's top-left pixel.
	triangle: Point,
	/// The count, as text, and where to anchor its top: on the strip's line, as the voltage is.
	count: Buf,
	count_origin: Point,
	/// The triangle and the count's character cells, and [`BADGE_MARGIN`] round them: the strip's
	/// first eight rows, as tall as a rune, so the strip's last row stays dark. Laid out by the
	/// cells and not by the ink, so `?` and every one-digit count take the same box. Lit, under
	/// dark ink, when a code is failing now.
	ground: Rectangle,
	inverted: bool,
}

/// The strip's right end for what the board says, on a panel `width` columns wide. The badge
/// is the count of the board's stored fault codes: none for no count or a count of zero, `?`
/// for the number when the count failed, laid out as one digit and never inverted.
fn corner(board: &Board, theme: &Theme, width: u32, report: &mut Report) -> Corner {
	// One past the column the next part, leftwards, ends on.
	let mut end = width as i32 - STRIP_RIGHT;
	let mut left = None;
	let mut runes = [None, None];
	for (slot, present) in [(1, board.links.ble), (0, board.links.usb)] {
		if present {
			if left.is_some() {
				end -= RUNE_GAP;
			}
			end -= RUNE_W;
			// From the strip's top row: a rune is the line's height.
			runes[slot] = Some(Point::new(end, 0));
			left = Some(end);
		}
	}
	if left.is_some() {
		end -= BADGE_GAP;
	}
	let badge = badge(board.faults, theme, end, report);
	if let Some(badge) = &badge {
		left = Some(badge.ground.top_left.x);
	}
	Corner {
		usb: runes[0],
		ble: runes[1],
		badge,
		left,
	}
}

/// The badge with its ground ending one column before `end`; a longer count grows left.
fn badge(faults: Option<Faults>, theme: &Theme, end: i32, report: &mut Report) -> Option<Badge> {
	let mut count = Buf::new();
	let inverted = match faults? {
		Faults::Counted { stored: 0, .. } => return None,
		Faults::Counted { stored, failing_now } => {
			let _ = write!(count, "{stored}");
			failing_now
		}
		// No number to give: the count failed, and nothing says a code fails now.
		Faults::Failed => {
			let _ = count.write_str("?");
			false
		}
	};
	// The count's character cells end at `end`; the last cell's blank column is the ground's
	// right margin.
	let advance = match theme.label.get_rendered_dimensions(count.as_str(), Point::zero(), VerticalPosition::Top) {
		Ok(dims) => dims.advance.x,
		Err(_) => {
			report.glyph_missing = true;
			0
		}
	};
	let count_origin = Point::new(end - advance, 0);
	// The digits' rows: the triangle stands on them, and the ground is a row over and under.
	let digits = lit_box(&theme.label, "0").unwrap_or(Rectangle::new(Point::new(0, 1), Size::new(0, TRIANGLE.len() as u32)));
	let triangle = Point::new(count_origin.x - COUNT_GAP - TRIANGLE[0].len() as i32, digits.top_left.y);
	let line = Rectangle::new(Point::new(i32::MIN / 2, 0), Size::new(u32::MAX / 2, STRIP_ROWS - 1));
	let ground = Rectangle::with_corners(
		triangle - Point::new(BADGE_MARGIN, BADGE_MARGIN),
		Point::new(end - 1, triangle.y + TRIANGLE.len() as i32 - 1 + BADGE_MARGIN),
	)
	.intersection(&line);
	Some(Badge {
		triangle,
		count,
		count_origin,
		ground,
		inverted,
	})
}

/// The right end on the strip's dark ground: the runes lit, and the badge lit — or, for a code
/// failing now, the whole badge inverted: dark ink on its lit ground, steady.
fn draw_corner<D>(corner: &Corner, theme: &Theme, target: &mut D, report: &mut Report)
where
	D: DrawTarget<Color = BinaryColor>,
{
	let lit = BinaryColor::On;
	if let Some(at) = corner.usb {
		draw_rows(&USB_RUNE, at, lit, target);
	}
	if let Some(at) = corner.ble {
		draw_rows(&BLE_RUNE, at, lit, target);
	}
	let Some(badge) = &corner.badge else {
		return;
	};
	let ink = if badge.inverted {
		let _ = target.fill_solid(&badge.ground, lit);
		BinaryColor::Off
	} else {
		lit
	};
	draw_rows(&TRIANGLE, badge.triangle, ink, target);
	if theme
		.label
		.render(
			badge.count.as_str(),
			badge.count_origin,
			VerticalPosition::Top,
			FontColor::Transparent(ink),
			target,
		)
		.is_err()
	{
		report.glyph_missing = true;
	}
}

/// A picture given row by row, `#` for ink, with its top-left pixel at `at`.
fn draw_rows<D>(rows: &[&str], at: Point, ink: BinaryColor, target: &mut D)
where
	D: DrawTarget<Color = BinaryColor>,
{
	let pixels = rows.iter().enumerate().flat_map(|(dy, row)| {
		row
			.bytes()
			.enumerate()
			.filter(|(_, c)| *c == b'#')
			.map(move |(dx, _)| Pixel(at + Point::new(dx as i32, dy as i32), ink))
	});
	let _ = target.draw_iter(pixels);
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

/// Dark rows above `SLCAN`, and dark columns left of it: the margins the link icons had in the
/// pages' corner when the title was set (owner, 2026-09-14 — flush with the corner it did not
/// match them). Wider across than down, because columns read closer than rows on this panel.
const ADAPTER_TOP: i32 = 2;
const ADAPTER_LEFT: i32 = 4;

/// The layout, measured: `SLCAN` in the top-left corner, [`ADAPTER_TOP`] and [`ADAPTER_LEFT`]
/// in from its edges; under it three centred lines —
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

	// Its ink, not its anchor, sits at the margin.
	let title_ink = ink_at_origin(&fonts.title, ADAPTER_TITLE, report).unwrap_or_default();
	let title_h = title_ink.size.height as i32;
	let (title_x, title_y) = (ADAPTER_LEFT, ADAPTER_TOP);
	let title = if fits(title_y, title_h, report) {
		place(&fonts.title, ADAPTER_TITLE, title_x, title_y, title_ink.top_left.y, report)
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
	let band = title_y + title_h + LINE_GAP;
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

fn values<D>(cells: &[Cell<'_>], theme: &Theme, target: &mut D) -> Report
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
		// The label's room is its column less the padding.
		let span = (x as i32 + PAD as i32, (x + w) as i32 - PAD as i32);
		draw_label(cell.label, centre, span, layout.label_top, &theme.label, ink, target, &mut report);
		draw_value(cell, centre, inner, height, theme, ink, &layout, target, &mut report);
	}
	report
}

/// What the whole row agreed on: one face, one unit policy, one arrangement.
#[derive(Debug)]
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
	/// The baseline of the deviation line, when the row has one: between the number and the
	/// unit. `None` when no cell in the row has a specified value, or when the panel had no
	/// room for a fourth line (then [`Report::deviation_dropped`] says so).
	dev_baseline: Option<i32>,
	/// Where the label's ink starts, and the baseline the unit sits on.
	///
	/// Every line's place is decided here, not at the edges: the air left after the lines'
	/// boxes is shared between the gaps, so three lines and four lines both read as one block
	/// rather than as text pushed against the top and bottom (owner, 2026-09-15). The gaps are
	/// equal in the arithmetic, not on the glass: a numeral's box is a few rows taller than the
	/// digits drawn in it, and that slack sits above the number.
	label_top: i32,
	unit_baseline: i32,
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
		.map(|c| {
			// A unit the face cannot draw measures nothing and leaves the row without a unit line;
			// said here, or the stacked arrangement would drop it with only `unit_dropped` to show
			// for it — the beside-the-number one says it in `unit_width`.
			if theme
				.unit
				.get_rendered_dimensions(c.unit, Point::zero(), VerticalPosition::Baseline)
				.is_err()
			{
				report.glyph_missing = true;
			}
			text_height(&theme.unit, c.unit)
		})
		.max()
		.unwrap_or(0);
	// A fourth line, of the small face, when any cell in the row has a specified value. The
	// whole row keeps it or none of it does: one cell a line shorter than its neighbours reads
	// as meaning something, the way an odd face would (see this function's note).
	let wants_deviation = cells.iter().any(|cell| cell.deviation != Deviation::None);
	// The label's face, not the unit's: the difference is a number a person reads, and at the
	// unit's size it was too small to read at a glance (owner, 2026-09-15).
	let dev_h = if wants_deviation { text_height(&theme.label, "0") } else { 0 };

	// One pixel of air above and below the number. Any less and the tiers touch,
	// which reads as one smeared block rather than three things.
	let fits = |step: usize, dev_h: u32| ROW_INSET as u32 * 2 + label_h + numeral_height(&theme.numerals[step], "0") + dev_h + unit_h + 2 <= height;
	let mut step = stacked_step;
	let mut dev_h = dev_h;
	// The deviation buys its line from the number's face before it gives up: the number is
	// still the thing being read, but a step of the ladder costs less than the difference.
	while dev_h > 0 && step < theme.numerals.len() - 1 && !fits(step, dev_h) {
		step += 1;
	}
	if dev_h > 0 && !fits(step, dev_h) {
		report.deviation_dropped = true;
		dev_h = 0;
		step = stacked_step;
	}
	if fits(step, dev_h) {
		// The boxes of the lines this row actually draws, and the air to share between them. A
		// line of no height is not a line: a row of unlabelled cells has three boxes, not four,
		// and counting a gap for the one that is not there pushed everything below it off the
		// glass (review, 2026-09-15).
		let value_h = numeral_height(&theme.numerals[step], "0") as i32;
		let heights = [label_h as i32, value_h, dev_h as i32, unit_h as i32];
		let ink: i32 = heights.iter().sum();
		let drawn = heights.iter().filter(|h| **h > 0).count() as i32;
		let gaps = (drawn - 1).max(1);
		let air = (height as i32 - 2 * ROW_INSET - ink).max(0);
		// The remainder goes above the number, the widest gap on the page anyway.
		let (gap, over) = (air / gaps, air % gaps);
		// One gap under a line, and only under one that was drawn.
		let after = |h: i32, extra: i32| if h > 0 { h + gap + extra } else { 0 };

		let mut y = ROW_INSET;
		let label_top = y;
		y += after(label_h as i32, over);
		let value_top = y;
		y += value_h + gap;
		let dev_baseline = (dev_h > 0).then(|| {
			let baseline = y + dev_h as i32;
			y = baseline + gap;
			baseline
		});
		let unit_baseline = if unit_h > 0 { y + unit_h as i32 } else { height as i32 - 1 - ROW_INSET };
		return RowLayout {
			step,
			with_unit: unit_h > 0,
			tiered: true,
			band: (value_top, value_top + value_h),
			dev_baseline,
			label_top,
			unit_baseline,
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
	// The two-tier arrangement has the number and the unit on one line and no room for a
	// third thing; a row that wanted a difference does not get one.
	report.deviation_dropped |= wants_deviation;
	RowLayout {
		step,
		with_unit,
		tiered: false,
		band: (0, height as i32 - 1),
		dev_baseline: None,
		label_top: 0,
		unit_baseline: height as i32 - 1,
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
/// A label that fits is moved only as far as it must to stay inside. One that does not fit is
/// reported and drawn centred on its room anyway: it will collide, and the report says so.
#[allow(clippy::too_many_arguments)]
fn draw_label<D>(label: &str, centre: i32, span: (i32, i32), top: i32, font: &FontRenderer, ink: BinaryColor, target: &mut D, report: &mut Report)
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
			Point::new(x - ink_box.top_left.x, top),
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
		Point::new(centre, top),
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
		if let (Some(baseline), Some(text)) = (layout.dev_baseline, difference(cell)) {
			draw_difference(&text, centre, baseline, theme, ink, target, report);
		}
		if layout.with_unit && !cell.unit.is_empty() {
			// Unit last, on the floor, centred under the number. It is the
			// smallest thing on the panel and the one you look at least.
			if theme
				.unit
				.render_aligned(
					cell.unit,
					Point::new(centre, layout.unit_baseline),
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

/// The chart's header rows: a line of the label face and a dark row under it, as the strip's
/// are. The trace starts under them.
const HEADER_ROWS: i32 = STRIP_ROWS as i32;

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

/// The chart header's right-hand half: the scale's range, and how much time the trace holds.
///
/// **The seconds go when the channel has a specified value** (owner, 2026-09-14): the
/// difference under the number is worth more than the window, and the header has room for one
/// of them. A channel with no specified value keeps its seconds.
fn chart_header(cell: &Cell<'_>, min: f32, max: f32, drawn: usize, seconds_per_sample: f32) -> Buf {
	let mut head = Buf::new();
	let d = cell.decimals as usize;
	let _ = write!(head, "{min:.d$}-{max:.d$}{}", cell.unit);
	if cell.deviation == Deviation::None {
		let _ = write!(head, "  {:.0}s", drawn as f32 * seconds_per_sample);
	}
	head
}

#[allow(clippy::too_many_arguments)]
fn chart<D>(cell: &Cell<'_>, min: f32, max: f32, samples: &[f32], seconds_per_sample: f32, label: bool, theme: &Theme, target: &mut D) -> Report
where
	D: DrawTarget<Color = BinaryColor>,
{
	let mut report = Report::default();
	let area = target.bounding_box();
	let width = area.size.width;
	let height = area.size.height;
	let ink = BinaryColor::On;
	// An alarm on a chart page inverts the chart's header, as one on a values page inverts its
	// cell: the header's line is lit and its text dark, the dark row under it stays, so the
	// trace keeps its edge (`todo/dash/27`, archived).
	let head_ink = if cell.alarm {
		let line = Rectangle::new(Point::zero(), Size::new(width, HEADER_ROWS as u32 - 1));
		let _ = target.fill_solid(&line, BinaryColor::On);
		BinaryColor::Off
	} else {
		ink
	};

	// The header carries what a chart cannot show about itself: what it is, what
	// the vertical extent is, and how much time the width holds. Without the
	// last two a trace is a shape with no units, which is decoration.
	//
	// Drawn in two pieces because it is two alphabets: the label is a word in the
	// reader's language, the rest is `0.00-2.50bar 19s`. One face has Cyrillic
	// and the other has `°`, and no face here has both.
	//
	// Under a strip that carries the page's title the label goes: the title says what the
	// chart is, and the same word twice reads as two things (owner, 2026-10-19).
	let after = if label {
		match theme.label.render(
			cell.label,
			Point::new(0, 0),
			VerticalPosition::Top,
			FontColor::Transparent(head_ink),
			target,
		) {
			Ok(dim) => dim.advance.x + 6,
			Err(_) => {
				report.glyph_missing = true;
				0
			}
		}
	} else {
		0
	};
	let buf = number(cell);
	// A chart gives the number a third of the width; past that the trace has
	// nowhere left to be, and a chart with no room for its trace is a bad table.
	let (step, value_w, _) = fit(&theme.numerals, buf.as_str(), 0, width / 3);
	if step > 0 {
		report.value_shrunk = true;
	}
	let plot_top = HEADER_ROWS;
	// The column the number and its difference share: the wider of the two decides where the
	// trace starts, or a long difference would run under it.
	let deviation = difference(cell);
	// The label's face: the numeral ladder's smallest was too big here (owner, 2026-09-15).
	let dev_w = deviation.as_ref().map_or(0, |text| difference_width(theme, text));
	let column_w = value_w.max(dev_w) as i32;
	let plot_x = column_w + 4;
	let plot_bottom = height as i32 - 1;
	// To the panel's edge: the link runes and the fault badge are in the strip, so nothing of
	// the board's stands beside the trace and its width — the seconds it holds — is the page's.
	let plot_w = width as i32 - plot_x;
	// The geometry is settled before the header is written, because the header
	// counts what the trace draws and the trace is only as wide as the plot.
	let drawn = drawn_columns(samples.len(), plot_w);

	let tail = chart_header(cell, min, max, drawn, seconds_per_sample);
	match theme.unit.render(
		tail.as_str(),
		Point::new(after, 1),
		VerticalPosition::Top,
		FontColor::Transparent(head_ink),
		target,
	) {
		Ok(dim) => {
			if dim.bounding_box.is_some_and(|b| b.top_left.x + b.size.width as i32 > width as i32) {
				report.label_overrun = true;
			}
		}
		Err(_) => report.glyph_missing = true,
	}

	// Centred in the band under the header, for the same reason as the values
	// page: the number is what the eye came for, and on the floor it reads as an
	// afterthought under the trace. The difference from the specified value, when there is
	// one, takes the floor under it.
	// Both the number and the difference are centred in the column they share.
	let dev_h = if deviation.is_some() { text_height(&theme.label, "0") as i32 } else { 0 };
	let band_bottom = height as i32 - 1 - if dev_h > 0 { dev_h + 1 } else { 0 };
	let value_h = numeral_height(&theme.numerals[step], buf.as_str());
	let value_baseline = plot_top + (band_bottom - plot_top - value_h as i32) / 2 + value_h as i32;
	let value_x = (column_w - value_w as i32) / 2;
	draw_numerals(&theme.numerals[step], buf.as_str(), Point::new(value_x, value_baseline), ink, target);
	if let Some(text) = deviation {
		draw_difference(&text, column_w / 2, height as i32 - 1, theme, ink, target, &mut report);
	}

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
	use crate::frame::Links;
	use embedded_graphics_simulator::SimulatorDisplay;

	/// The glass the board has.
	fn panel() -> SimulatorDisplay<BinaryColor> {
		SimulatorDisplay::new(PANEL)
	}

	/// A 32-row panel. No board has one — the glass is [`PANEL`] — but the renderer measures
	/// its target, and 32 rows is where three tiers stop fitting and the unit goes beside the
	/// number: what any glass too short for three gets. Only the tests of that fallback use it.
	const SHORT: Size = Size::new(256, 32);

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
		let report = values(&cells, &Theme::bold_mono(), &mut display);
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
		let layout = row_layout(&cells, &theme, INNER, PANEL.height, &mut report);
		assert!(layout.tiered, "three tiers fit in 64 rows");
		assert!(layout.with_unit, "a stacked unit is never dropped for width");

		let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(PANEL);
		let report = values(&cells, &theme, &mut display);
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
		let layout = row_layout(&cells, &theme, INNER, SHORT.height, &mut report);
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
		let report = values(&cells, &Theme::bold_mono(), &mut display);
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
		values(&cells, &Theme::bold_mono(), &mut display);
		// The top-left corner of the alarmed column is lit ground; the same corner
		// of its neighbours is not.
		assert!(lit(&display, 65, 0), "the alarmed cell's ground is lit");
		assert!(!lit(&display, 1, 0), "its left neighbour's is not");
		assert!(!lit(&display, 129, 0), "nor its right neighbour's");
		// And the label survived: somewhere in the alarmed column's label row
		// there is an unlit pixel, which is the dark text on the lit ground.
		assert!((64..128).any(|x| !lit(&display, x, 3)), "the label is drawn dark on the lit ground");
	}

	/// A channel shown only on a chart page can carry an alarm: its page is raised and the
	/// chart's header line inverted, the row under it and the trace left as they are.
	#[test]
	fn an_alarm_on_a_chart_inverts_its_header_line_and_nothing_else() {
		let samples = [0.5f32; 8];
		let draw_one = |cell: Cell<'static>| {
			let mut display = panel();
			let frame = Frame::Chart {
				cell,
				min: 0.0,
				max: 2.5,
				samples: &samples,
				seconds_per_sample: 0.2,
			};
			draw(&frame, &Theme::bold_mono(), &mut display);
			display
		};
		let plain = draw_one(Cell::new("BOOST", Some(0.5), "bar", 2));
		let alarmed = draw_one(Cell::new("BOOST", Some(0.5), "bar", 2).alarmed());
		let far = PANEL.width as i32 - 1;
		assert!(lit(&alarmed, far, 0) && !lit(&plain, far, 0), "the header's line is lit ground");
		assert!((0..PANEL.width as i32).any(|x| !lit(&alarmed, x, 3)), "its text is drawn dark on it");
		let under = HEADER_ROWS - 1;
		assert!(
			(0..PANEL.width as i32).all(|x| lit(&alarmed, x, under) == lit(&plain, x, under)),
			"the dark row under the header is the plain chart's"
		);
		for y in HEADER_ROWS..PANEL.height as i32 {
			for x in 0..PANEL.width as i32 {
				assert_eq!(lit(&alarmed, x, y), lit(&plain, x, y), "({x}, {y}) under the header changed");
			}
		}
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
		let report = values(&cells, &Theme::bold_mono(), &mut display);
		assert!(report.label_overrun, "{report:?}");
	}

	#[test]
	fn the_difference_signs_are_a_pixel_shorter_than_its_digits() {
		let theme = Theme::bold_mono();
		let digits = text_height(&theme.label, "0") as i32;
		let size = sign_size(&theme);
		assert_eq!(size, digits - 1, "a pixel shorter than the digits, and as wide as it is tall");

		// Drawn on a blank panel: the plus is a cross of two strokes, the minus one bar, both
		// inside a box of `size` sitting on the baseline.
		for negative in [false, true] {
			let mut display = panel();
			let baseline = 40;
			draw_sign(negative, 10, baseline, size, BinaryColor::On, &mut display);
			let lit: std::vec::Vec<Point> = display
				.bounding_box()
				.points()
				.filter(|p| display.get_pixel(*p) == BinaryColor::On)
				.collect();
			let rows: std::vec::Vec<i32> = lit.iter().map(|p| p.y).collect();
			let columns: std::vec::Vec<i32> = lit.iter().map(|p| p.x).collect();
			let (top, bottom) = (*rows.iter().min().unwrap(), *rows.iter().max().unwrap());
			let (left, right) = (*columns.iter().min().unwrap(), *columns.iter().max().unwrap());
			assert_eq!(right - left + 1, size, "{negative}: as wide as its box");
			assert_eq!(left, 10, "{negative}: drawn from the left edge it was given");
			if negative {
				assert_eq!(top, bottom, "a minus is one bar");
				assert_eq!(lit.len() as i32, size);
			} else {
				assert_eq!(bottom, baseline, "the plus stands on the baseline");
				assert_eq!(bottom - top + 1, size, "and is a pixel shorter than the digits beside it");
				assert_eq!(lit.len() as i32, 2 * size - 1, "two strokes crossing once");
			}
		}
	}

	#[test]
	fn a_row_of_unlabelled_cells_still_fits_the_panel() {
		// A line of no height is not a line, and must not be given a gap of its own: with one
		// the unit slid off the bottom of the glass (review, 2026-09-15).
		for deviation in [Deviation::None, Deviation::Value(0.07)] {
			let cells = [Cell::new("", Some(1.92), "bar", 2).with_deviation(deviation)];
			let theme = Theme::bold_mono();
			let mut report = Report::default();
			let layout = row_layout(&cells, &theme, PANEL.width - 2 * PAD, PANEL.height, &mut report);
			assert!(layout.unit_baseline < PANEL.height as i32, "{deviation:?}: {layout:?}");
			if let Some(baseline) = layout.dev_baseline {
				assert!(baseline < layout.unit_baseline, "{layout:?}");
			}

			let mut display = panel();
			let report = values(&cells, &theme, &mut display);
			assert_eq!(report, Report::default(), "{deviation:?}: {report:?}");
			// The bottom line is drawn, which it is not when it is pushed past the floor.
			assert!(
				(layout.unit_baseline - 6..=layout.unit_baseline).any(|y| lit_row(&display, y)),
				"{deviation:?}: the unit is on the glass"
			);
		}
	}

	// --- a channel's specified value ----------------------------------------------------

	/// A cell with a specified value behind it.
	fn drifting<'a>(label: &'a str, value: f32, unit: &'a str, decimals: u8, deviation: Deviation) -> Cell<'a> {
		Cell::new(label, Some(value), unit, decimals).with_deviation(deviation)
	}

	fn lit_row(display: &SimulatorDisplay<BinaryColor>, y: i32) -> bool {
		(0..PANEL.width as i32).any(|x| lit(display, x, y))
	}

	#[test]
	fn a_cell_with_no_specified_value_is_drawn_exactly_as_before() {
		let plain = [Cell::new("НАДДУВ", Some(1.82), "bar", 2), Cell::new("ОЖ", Some(93.0), "°C", 0)];
		let marked = [
			Cell::new("НАДДУВ", Some(1.82), "bar", 2).with_deviation(Deviation::None),
			Cell::new("ОЖ", Some(93.0), "°C", 0).with_deviation(Deviation::None),
		];
		let (mut a, mut b) = (panel(), panel());
		let one = values(&plain, &Theme::bold_mono(), &mut a);
		let two = values(&marked, &Theme::bold_mono(), &mut b);
		assert_eq!(one, two);
		assert!(a.bounding_box().points().all(|p| a.get_pixel(p) == b.get_pixel(p)));
	}

	#[test]
	fn a_deviation_is_drawn_between_the_number_and_the_unit() {
		let cells = [drifting("НАДДУВ", 1.92, "bar", 2, Deviation::Value(0.07))];
		let mut display = panel();
		let report = values(&cells, &Theme::bold_mono(), &mut display);
		assert_eq!(report, Report::default(), "{report:?}");

		let theme = Theme::bold_mono();
		let layout = row_layout(&cells, &theme, PANEL.width - 2 * PAD, PANEL.height, &mut Report::default());
		// The ink of a line ends a row above its baseline, so the line is looked for in the
		// band between the number's floor and it.
		let baseline = layout.dev_baseline.expect("the row keeps a line for the deviation");
		assert!((layout.band.1..=baseline).any(|y| lit_row(&display, y)), "the deviation line is drawn");
		assert!(baseline < PANEL.height as i32 - 1, "above the unit");
		assert!(
			(baseline + 1..PANEL.height as i32).any(|y| lit_row(&display, y)),
			"the unit is still under it"
		);
	}

	#[test]
	fn an_unanswered_specified_value_draws_a_dash_and_keeps_the_line() {
		let unknown = [drifting("НАДДУВ", 1.92, "bar", 2, Deviation::Unknown)];
		let answered = [drifting("НАДДУВ", 1.92, "bar", 2, Deviation::Value(0.07))];
		let (mut a, mut b) = (panel(), panel());
		values(&unknown, &Theme::bold_mono(), &mut a);
		values(&answered, &Theme::bold_mono(), &mut b);
		let layout = row_layout(&unknown, &Theme::bold_mono(), PANEL.width - 2 * PAD, PANEL.height, &mut Report::default());
		let baseline = layout.dev_baseline.expect("the line is kept whether or not it answered");
		assert!((layout.band.1..=baseline).any(|y| lit_row(&a, y)), "a dash is drawn");
		assert!(
			a.bounding_box().points().any(|p| a.get_pixel(p) != b.get_pixel(p)),
			"and it is not the picture of an answered one"
		);
	}

	#[test]
	fn the_deviation_line_does_not_shrink_the_number_on_this_panel() {
		let theme = Theme::bold_mono();
		let cells: std::vec::Vec<Cell<'_>> = (0..4).map(|_| drifting("НАДДУВ", 1.92, "bar", 2, Deviation::Value(-0.07))).collect();
		let plain: std::vec::Vec<Cell<'_>> = (0..4).map(|_| Cell::new("НАДДУВ", Some(1.92), "bar", 2)).collect();
		let inner = PANEL.width / 4 - 2 * PAD;
		let mut report = Report::default();
		let with = row_layout(&cells, &theme, inner, PANEL.height, &mut report);
		let without = row_layout(&plain, &theme, inner, PANEL.height, &mut Report::default());
		assert_eq!(with.step, without.step, "the 64-row panel holds four lines at the same face");
		assert!(!report.deviation_dropped, "{report:?}");
	}

	#[test]
	fn a_panel_too_short_for_four_lines_drops_the_deviation_and_says_so() {
		let cells = [drifting("ОЖ", 93.0, "°C", 0, Deviation::Value(1.0))];
		let mut report = Report::default();
		let layout = row_layout(&cells, &Theme::bold_mono(), PANEL.width - 2 * PAD, 24, &mut report);
		assert!(report.deviation_dropped, "{report:?}");
		assert!(layout.dev_baseline.is_none());
	}

	#[test]
	fn a_chart_with_a_specified_value_shows_the_deviation_and_drops_the_seconds() {
		let samples = [1.9f32; 120];
		let cell = drifting("НАДДУВ", 1.92, "bar", 2, Deviation::Value(0.07));
		let frame = Frame::Chart {
			cell: drifting("НАДДУВ", 1.92, "bar", 2, Deviation::Value(0.07)),
			min: 0.0,
			max: 2.5,
			samples: &samples,
			seconds_per_sample: 0.2,
		};
		let mut display = panel();
		let report = draw(&frame, &Theme::bold_mono(), &mut display);
		assert_eq!(report, Report::default(), "{report:?}");

		let head = chart_header(&cell, 0.0, 2.5, 120, 0.2);
		assert!(head.as_str().contains("0.00-2.50bar"), "the range stays: {:?}", head.as_str());
		assert!(!head.as_str().contains('s'), "no seconds: {:?}", head.as_str());
		let plain = Cell::new("НАДДУВ", Some(1.92), "bar", 2);
		let head = chart_header(&plain, 0.0, 2.5, 120, 0.2);
		assert!(head.as_str().contains("24s"), "without one the seconds stay: {:?}", head.as_str());
	}

	#[test]
	fn a_difference_wider_than_the_number_keeps_the_trace_out_of_its_column() {
		// `-1234` is wider than `9`, so the column is the difference's and the trace starts past
		// it rather than running under it.
		static SAMPLES: [f32; 200] = [9.0; 200];
		let of = |v: f32| Cell::new("ДАВЛЕНИЕ", Some(v), "bar", 0).with_deviation(Deviation::Value(-1234.0));
		let frame = |samples: &'static [f32]| Frame::Chart {
			cell: of(9.0),
			min: 0.0,
			max: 20.0,
			samples,
			seconds_per_sample: 0.2,
		};
		let mut display = panel();
		let report = draw(&frame(&SAMPLES), &Theme::bold_mono(), &mut display);
		assert!(!report.value_overrun, "{report:?}");

		let theme = Theme::bold_mono();
		let cell = of(9.0);
		let (_, value_w, _) = fit(&theme.numerals, number(&cell).as_str(), 0, PANEL.width / 3);
		let dev_w = difference_width(&theme, &difference(&cell).unwrap());
		assert!(dev_w > value_w, "the difference is the wider of the two: {dev_w} against {value_w}");

		// Nothing of the trace in the column: with no samples at all the same rows hold the
		// same pixels.
		let mut empty = panel();
		draw(&frame(&[]), &theme, &mut empty);
		let column = Rectangle::new(Point::new(0, HEADER_ROWS), Size::new(dev_w, PANEL.height - HEADER_ROWS as u32));
		assert!(
			column.points().all(|p| empty.get_pixel(p) == display.get_pixel(p)),
			"the trace stays out of the number's column"
		);
	}

	// --- the board's panel ------------------------------------------------------------

	/// The whole glass.
	const PANEL_BOX: Rectangle = Rectangle::new(Point::zero(), PANEL);

	const USB: Links = Links { usb: true, ble: false };
	const BLE: Links = Links { usb: false, ble: true };
	const BOTH: Links = Links { usb: true, ble: true };

	const NINE: Option<Faults> = Some(Faults::Counted {
		stored: 9,
		failing_now: false,
	});
	const NINE_FAILING: Option<Faults> = Some(Faults::Counted {
		stored: 9,
		failing_now: true,
	});
	/// The count failed: no list from the gateway, or one the board refused.
	const FAILED: Option<Faults> = Some(Faults::Failed);

	fn lit_in(display: &SimulatorDisplay<BinaryColor>, area: Rectangle) -> bool {
		area.points().any(|p| display.get_pixel(p) == BinaryColor::On)
	}

	/// Every pixel outside `area` is the same in both.
	fn same_outside(a: &SimulatorDisplay<BinaryColor>, b: &SimulatorDisplay<BinaryColor>, area: Rectangle) -> bool {
		a.bounding_box().points().all(|p| area.contains(p) || a.get_pixel(p) == b.get_pixel(p))
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

	/// A chart pinned at the top of its scale, so the trace runs along its highest row.
	fn pinned_chart(samples: &[f32]) -> Frame<'_> {
		Frame::Chart {
			cell: Cell::new("НАДДУВ", Some(2.5), "bar", 2),
			min: 0.0,
			max: 2.5,
			samples,
			seconds_per_sample: 0.2,
		}
	}

	/// A drive page: four cells, the last with a specified value, so its number stands high.
	fn drive(last: Cell<'_>) -> [Cell<'_>; 4] {
		[
			Cell::new("СКОРОСТЬ", Some(87.0), "km/h", 0),
			Cell::new("ОБОРОТЫ", Some(2840.0), "", 0),
			Cell::new("НАДДУВ", Some(1.42), "bar", 2).with_deviation(Deviation::Value(-0.06)),
			last,
		]
	}

	// --- the status strip -------------------------------------------------------------

	/// The page under the strip: the panel less the strip's rows.
	const UNDER: Size = Size::new(PANEL.width, PANEL.height - STRIP_ROWS);

	fn board(links: Links, faults: Option<Faults>) -> Board {
		Board { links, rates: None, faults }
	}

	/// Every combination of what the board can say: the links, and the fault count's kinds.
	fn boards() -> std::vec::Vec<Board> {
		let counts = [
			None,
			NINE,
			NINE_FAILING,
			FAILED,
			Some(Faults::Counted {
				stored: 99,
				failing_now: true,
			}),
		];
		[Links::NONE, USB, BLE, BOTH]
			.into_iter()
			.flat_map(|links| counts.map(|faults| board(links, faults)))
			.collect()
	}

	/// `page` drawn with `strip` on the board's panel, and the report.
	fn paged(frame: &Frame<'_>, strip: &Strip<'_>, board: &Board) -> (SimulatorDisplay<BinaryColor>, Report) {
		let mut display = panel();
		let report = draw_page(frame, strip, board, &Theme::bold_mono(), &mut display);
		(display, report)
	}

	/// Rows `STRIP_ROWS..` of `display` are, pixel for pixel, `under` — the page drawn alone on a
	/// panel as tall as what the strip leaves.
	fn page_under_strip_is(display: &SimulatorDisplay<BinaryColor>, under: &SimulatorDisplay<BinaryColor>) -> bool {
		Rectangle::new(Point::zero(), UNDER)
			.points()
			.all(|p| under.get_pixel(p) == display.get_pixel(p + Point::new(0, STRIP_ROWS as i32)))
	}

	fn strip_rows() -> Rectangle {
		Rectangle::new(Point::zero(), Size::new(PANEL.width, STRIP_ROWS))
	}

	/// The strip's right end on the board's panel.
	fn right_end(board: &Board) -> Corner {
		corner(board, &Theme::bold_mono(), PANEL.width, &mut Report::default())
	}

	/// Whether `display` holds `rows` at `at`, pixel for pixel: `#` in `ink`, the rest not.
	fn holds(display: &SimulatorDisplay<BinaryColor>, rows: &[&str], at: Point, ink: BinaryColor) -> bool {
		rows.iter().enumerate().all(|(dy, row)| {
			row.bytes().enumerate().all(|(dx, c)| {
				let colour = display.get_pixel(at + Point::new(dx as i32, dy as i32));
				(colour == ink) == (c == b'#')
			})
		})
	}

	/// The panel's last column with ink on the strip's rows, and its first.
	fn strip_ink_span(display: &SimulatorDisplay<BinaryColor>, from_x: i32) -> Option<(i32, i32)> {
		let xs: std::vec::Vec<i32> = strip_rows()
			.points()
			.filter(|p| p.x >= from_x && display.get_pixel(*p) == BinaryColor::On)
			.map(|p| p.x)
			.collect();
		Some((*xs.iter().min()?, *xs.iter().max()?))
	}

	#[test]
	fn the_strip_writes_volts_with_one_decimal_and_a_dash_for_no_answer() {
		assert_eq!(volts_text(Some(14.24)).as_str(), "14.2V");
		assert_eq!(volts_text(Some(12.0)).as_str(), "12.0V");
		assert_eq!(volts_text(None).as_str(), "--V", "no answer is a dash, as in a cell, never 0.0V");
	}

	/// The runes and the triangle are drawn from rows, and stay inside the strip's line: the
	/// strip's last row is dark under them. The triangle is the digits' height, so the count
	/// stands beside it on the line the voltage is written on.
	#[test]
	fn the_runes_and_the_badge_fit_the_strips_line() {
		for rows in [&USB_RUNE[..], &BLE_RUNE[..], &TRIANGLE[..]] {
			assert!(rows.len() < STRIP_ROWS as usize, "{rows:?}");
		}
		for rows in [&USB_RUNE[..], &BLE_RUNE[..]] {
			assert!(rows.iter().all(|row| row.len() == RUNE_W as usize), "{rows:?}");
		}
		let digit = lit_box(&Theme::bold_mono().label, "0").expect("the label face has digits");
		assert_eq!(digit.size.height as usize, TRIANGLE.len(), "the triangle is the digits' height");
		let (mut usb, mut ble) = (panel(), panel());
		draw_rows(&USB_RUNE, Point::zero(), BinaryColor::On, &mut usb);
		draw_rows(&BLE_RUNE, Point::zero(), BinaryColor::On, &mut ble);
		assert_ne!(usb, ble, "two different pictures");
	}

	/// Nothing to show: no runes, no badge, and nothing at the right end.
	#[test]
	fn with_nothing_connected_and_no_count_the_right_end_is_dark() {
		for faults in [
			None,
			Some(Faults::Counted {
				stored: 0,
				failing_now: false,
			}),
			Some(Faults::Counted {
				stored: 0,
				failing_now: true,
			}),
		] {
			let quiet = board(Links::NONE, faults);
			let end = right_end(&quiet);
			assert!(end.usb.is_none() && end.ble.is_none() && end.badge.is_none(), "{faults:?}");
			assert_eq!(end.left, None, "{faults:?}: hidden at zero");
			let cells = temps("ВПУСК");
			let (display, _) = paged(&Frame::Values { cells: &cells }, &Strip::NONE, &quiet);
			assert!(!lit_in(&display, strip_rows()), "{faults:?}: an empty strip is dark");
		}
	}

	/// From the right edge, a column in as the voltage is from the left: the Bluetooth rune, the
	/// plug left of it, then the badge — the triangle and the count — left of the runes. A part
	/// with nothing to show takes no room; the rest close up to the edge.
	#[test]
	fn the_right_end_holds_the_runes_at_the_edge_and_the_badge_left_of_them() {
		let edge = PANEL.width as i32 - STRIP_RIGHT;
		let ble_at = Point::new(edge - RUNE_W, 0);
		let usb_beside = Point::new(ble_at.x - RUNE_GAP - RUNE_W, 0);
		let cells = temps("ВПУСК");
		let page = Frame::Values { cells: &cells };
		for (links, usb, ble) in [
			(USB, Some(Point::new(edge - RUNE_W, 0)), None),
			(BLE, None, Some(ble_at)),
			(BOTH, Some(usb_beside), Some(ble_at)),
		] {
			for faults in [None, NINE, NINE_FAILING, FAILED] {
				let said = board(links, faults);
				let end = right_end(&said);
				assert_eq!((end.usb, end.ble), (usb, ble), "{links:?} {faults:?}");
				let (display, report) = paged(&page, &Strip::NONE, &said);
				assert!(!report.glyph_missing, "{report:?}");
				if let Some(at) = usb {
					assert!(holds(&display, &USB_RUNE, at, BinaryColor::On), "{links:?} {faults:?}: the plug");
				}
				if let Some(at) = ble {
					assert!(holds(&display, &BLE_RUNE, at, BinaryColor::On), "{links:?} {faults:?}: the rune");
				}
				let runes_left = usb.or(ble).unwrap().x;
				match &end.badge {
					None => assert_eq!(end.left, Some(runes_left)),
					Some(badge) => {
						assert_eq!(end.left, Some(badge.ground.top_left.x));
						// The badge's ground ends BADGE_GAP dark columns before the runes.
						let ground_end = badge.ground.top_left.x + badge.ground.size.width as i32;
						assert_eq!(ground_end, runes_left - BADGE_GAP, "{links:?} {faults:?}");
						let gap = Rectangle::new(Point::new(ground_end, 0), Size::new(BADGE_GAP as u32, STRIP_ROWS));
						assert!(!lit_in(&display, gap), "{links:?} {faults:?}: the gap before the runes is dark");
					}
				}
				// Nothing of the strip's on its last row, nor left of the right end.
				let last = Rectangle::new(Point::new(0, STRIP_ROWS as i32 - 1), Size::new(PANEL.width, 1));
				assert!(!lit_in(&display, last), "{links:?} {faults:?}: the strip's last row is dark");
				let before = Rectangle::new(Point::zero(), Size::new(end.left.unwrap() as u32, STRIP_ROWS));
				assert!(!lit_in(&display, before), "{links:?} {faults:?}: nothing left of the right end");
				let (_, right) = strip_ink_span(&display, 0).unwrap();
				assert!(right < edge, "{links:?} {faults:?}: a dark column at the panel's edge");
			}
		}
	}

	/// The badge alone stands where the runes would: its ground ends a column in from the edge.
	#[test]
	fn with_no_host_the_badge_takes_the_corner() {
		let edge = PANEL.width as i32 - STRIP_RIGHT;
		for faults in [NINE, NINE_FAILING, FAILED] {
			let said = board(Links::NONE, faults);
			let badge = right_end(&said).badge.expect("a count to show");
			assert_eq!(badge.ground.top_left.x + badge.ground.size.width as i32, edge, "{faults:?}");
			let (display, _) = paged(&Frame::Values { cells: &temps("ОЖ") }, &Strip::NONE, &said);
			let (left, right) = strip_ink_span(&display, 0).expect("the badge is drawn");
			assert!(right < edge, "{faults:?}: {right}");
			let lead = if badge.inverted { badge.ground.top_left.x } else { badge.triangle.x };
			assert_eq!(left, lead, "{faults:?}: the triangle first");
			let ink = if badge.inverted { BinaryColor::Off } else { BinaryColor::On };
			assert!(holds(&display, &TRIANGLE, badge.triangle, ink), "{faults:?}");
		}
	}

	/// The count is the label face's digits, on the voltage's line, after the triangle.
	#[test]
	fn the_count_stands_beside_the_triangle_on_the_strips_line() {
		let theme = Theme::bold_mono();
		let said = board(Links::NONE, NINE);
		let badge = right_end(&said).badge.unwrap();
		assert_eq!(badge.count.as_str(), "9");
		let (display, _) = paged(&Frame::Values { cells: &[] }, &Strip::NONE, &said);
		let mut nine = panel();
		theme
			.label
			.render(
				"9",
				badge.count_origin,
				VerticalPosition::Top,
				FontColor::Transparent(BinaryColor::On),
				&mut nine,
			)
			.unwrap();
		let mut alone = panel();
		draw_rows(&TRIANGLE, badge.triangle, BinaryColor::On, &mut alone);
		assert!(
			PANEL_BOX
				.points()
				.all(|p| display.get_pixel(p) == (nine.get_pixel(p).is_on() || alone.get_pixel(p).is_on()).into())
		);
		let digit = lit_box(&theme.label, "9").unwrap();
		let count_left = badge.count_origin.x + digit.top_left.x;
		assert_eq!(
			count_left,
			badge.triangle.x + TRIANGLE[0].len() as i32 + COUNT_GAP,
			"COUNT_GAP after the triangle"
		);
		assert_eq!(badge.count_origin.y + digit.top_left.y, badge.triangle.y, "on one line");
		// The voltage's digits stand on the same rows.
		let mut volts = panel();
		let strip = Strip {
			title: "",
			volts: Volts::Shown(Some(19.9)),
		};
		draw_page(&Frame::Values { cells: &[] }, &strip, &Board::default(), &theme, &mut volts);
		let rows = |d: &SimulatorDisplay<BinaryColor>, x0: i32, x1: i32| {
			let ys: std::vec::Vec<i32> = strip_rows()
				.points()
				.filter(|p| (x0..x1).contains(&p.x) && d.get_pixel(*p).is_on())
				.map(|p| p.y)
				.collect();
			(*ys.iter().min().unwrap(), *ys.iter().max().unwrap())
		};
		assert_eq!(
			rows(&volts, 0, 20),
			rows(&display, count_left, PANEL.width as i32),
			"the count and `19.9` share their rows"
		);
	}

	#[test]
	fn a_code_failing_now_inverts_the_badge_and_nothing_else() {
		let cells = temps("ОЖ");
		let page = Frame::Values { cells: &cells };
		for links in [Links::NONE, BOTH] {
			let (normal, _) = paged(&page, &Strip::NONE, &board(links, NINE));
			let (inverted, report) = paged(&page, &Strip::NONE, &board(links, NINE_FAILING));
			assert!(!report.glyph_missing);
			let ground = right_end(&board(links, NINE_FAILING)).badge.unwrap().ground;
			assert_eq!(ground, right_end(&board(links, NINE)).badge.unwrap().ground, "the same box either way");
			assert!(
				ground.points().all(|p| inverted.get_pixel(p) != normal.get_pixel(p)),
				"{links:?}: every pixel of the box swaps"
			);
			assert!(same_outside(&inverted, &normal, ground), "{links:?}");
			// Inside the strip's line, and a pixel of lit ground round the ink at the sides.
			assert_eq!(ground.top_left.y, 0);
			assert_eq!(ground.top_left.y + ground.size.height as i32, STRIP_ROWS as i32 - 1);
		}
	}

	/// Owner, 2026-09-27: the gateway gave no list, or the board refused it — the triangle with
	/// `?` for the number, in the box a one-digit count has, and never inverted.
	#[test]
	fn a_count_that_failed_is_a_question_mark_laid_out_as_one_digit() {
		let cells = temps("ОЖ");
		let page = Frame::Values { cells: &cells };
		for links in [Links::NONE, BOTH] {
			let failed = right_end(&board(links, FAILED)).badge.unwrap();
			let nine = right_end(&board(links, NINE)).badge.unwrap();
			assert_eq!(failed.count.as_str(), "?");
			assert!(!failed.inverted);
			assert_eq!(failed.ground, nine.ground, "{links:?}: the box of a one-digit count");
			assert_eq!(failed.triangle, nine.triangle);
			let (display, report) = paged(&page, &Strip::NONE, &board(links, FAILED));
			assert!(!report.glyph_missing, "{report:?}: the label face has a `?`");
			let (digit, _) = paged(&page, &Strip::NONE, &board(links, NINE));
			assert!(failed.ground.points().any(|p| display.get_pixel(p) != digit.get_pixel(p)), "not a digit");
			assert!(holds(&display, &TRIANGLE, failed.triangle, BinaryColor::On), "the triangle, lit on dark");
		}
	}

	#[test]
	fn a_longer_count_grows_left_and_keeps_its_right_edge() {
		let wide = Some(Faults::Counted {
			stored: 1234,
			failing_now: false,
		});
		for links in [Links::NONE, BLE] {
			let short = right_end(&board(links, NINE)).badge.unwrap().ground;
			let long = right_end(&board(links, wide)).badge.unwrap().ground;
			assert_eq!(short.top_left.x + short.size.width as i32, long.top_left.x + long.size.width as i32);
			assert_eq!(short.size.height, long.size.height);
			assert!(long.size.width > short.size.width);
			let (display, report) = paged(&Frame::Values { cells: &[] }, &Strip::NONE, &board(links, wide));
			assert!(!report.glyph_missing);
			assert_eq!(
				strip_ink_span(&display, 0).unwrap().0,
				long.top_left.x + BADGE_MARGIN,
				"the triangle leads"
			);
		}
	}

	/// The point of the strip's right end: what the board says changes the strip, never the page
	/// under it — a values page with a four-character number in its last cell, an alarmed one, a
	/// chart and the stopwatch, with every link and count there is.
	#[test]
	fn the_page_under_the_strip_is_the_same_whatever_the_board_says() {
		let samples = [2.5f32; 256];
		let wide = drive(Cell::new("ОБОРОТЫ", Some(6500.0), "", 0).with_deviation(Deviation::Value(-120.0)));
		let long = temps("ТЕМП.МАСЛА");
		let alarmed = [Cell::new("ОЖ", Some(93.0), "°C", 0), Cell::new("КОРОБКА", Some(128.0), "°C", 0).alarmed()];
		let marks = [60u16, 100, 150];
		let labels = crate::stopwatch::Labels::new(&marks);
		let words = crate::stopwatch::Words::of("ru");
		let watch = crate::stopwatch::Stopwatch::new(&marks, 0.01);
		let saved = [(60u16, 5.43f32), (100, 9.87), (150, 18.4)];
		let (row, count) = crate::stopwatch::cells(&watch, Some(0.0), &saved, &words, &labels);
		let titled = Strip {
			title: "ЕДЕМ",
			volts: Volts::Shown(Some(13.9)),
		};
		for (name, page, strip) in [
			("6500", Frame::Values { cells: &wide }, titled),
			("a long last label", Frame::Values { cells: &long }, Strip::NONE),
			("alarm", Frame::Values { cells: &alarmed }, titled),
			("chart", pinned_chart(&samples), titled),
			("stopwatch", Frame::Values { cells: &row[..count] }, Strip::NONE),
		] {
			let mut under: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(UNDER);
			let alone = draw_frame(&page, None, &Theme::bold_mono(), &mut under, strip.title.is_empty());
			assert!(!alone.label_overrun && !alone.value_overrun && !alone.glyph_missing, "{name}: {alone:?}");
			for said in boards() {
				let (display, report) = paged(&page, &strip, &said);
				assert!(page_under_strip_is(&display, &under), "{name}, {said:?}: the page moved");
				assert_eq!(report, alone, "{name}, {said:?}: nothing to report beyond the page's own");
			}
		}
	}

	/// The chart under the strip has nothing of the board's beside it: its trace runs to the
	/// panel's edge, and the seconds its header counts are the page's.
	#[test]
	fn a_chart_under_the_strip_runs_its_trace_to_the_panels_edge() {
		let samples = [2.5f32; 256];
		let titled = Strip {
			title: "НАДДУВ",
			volts: Volts::Hidden,
		};
		let (display, report) = paged(&pinned_chart(&samples), &titled, &board(BOTH, NINE));
		assert_eq!(report, Report::default(), "{report:?}");
		let top = STRIP_ROWS as i32 + HEADER_ROWS;
		assert!(lit(&display, PANEL.width as i32 - 1, top), "the trace's top row reaches the last column");
	}

	#[test]
	fn a_values_page_under_the_strip_is_the_page_on_the_rows_under_it() {
		let cells = temps("ВПУСК");
		let page = Frame::Values { cells: &cells };
		let strip = Strip {
			title: "ТЕМПЕРАТУРЫ",
			volts: Volts::Shown(Some(14.2)),
		};
		let said = board(BLE, NINE);
		let (display, report) = paged(&page, &strip, &said);
		let mut under: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(UNDER);
		let alone = draw(&page, &Theme::bold_mono(), &mut under);
		assert_eq!(report, alone, "the strip itself fits: nothing to report beyond the page's own");
		let whole = draw(&page, &Theme::bold_mono(), &mut panel());
		assert_eq!(alone, whole, "and the page fits its 55 rows as it fits 64: {alone:?}");
		assert!(page_under_strip_is(&display, &under), "the page, moved down by the strip");

		// The strip: the voltage at the left, the title in the middle, nothing on its last row
		// — no rule under it.
		let ink: std::vec::Vec<Point> = strip_rows().points().filter(|p| display.get_pixel(*p) == BinaryColor::On).collect();
		assert!(
			ink.iter().all(|p| p.y < STRIP_ROWS as i32 - 1),
			"a dark row between the strip and the page"
		);
		let volts: std::vec::Vec<i32> = ink.iter().filter(|p| p.x < 64).map(|p| p.x).collect();
		// Anchored a column in; a `1` has a blank column of its own before its stroke.
		let first = *volts.iter().min().expect("the voltage is drawn");
		assert!((STRIP_LEFT..=STRIP_LEFT + 1).contains(&first), "the voltage starts at the left: {first}");
		let end = right_end(&said).left.unwrap();
		let title: std::vec::Vec<i32> = ink.iter().filter(|p| p.x >= 64 && p.x < end).map(|p| p.x).collect();
		let (left, right) = (*title.iter().min().unwrap(), *title.iter().max().unwrap());
		assert!(
			((left + right) / 2 - PANEL.width as i32 / 2).abs() <= 2,
			"the title is centred on the panel, whatever is at the right end: {left}..{right}"
		);
	}

	/// A page with no title and no voltage has the strip all the same: the page under it does
	/// not move when something comes to show in it.
	#[test]
	fn a_page_with_no_title_and_no_voltage_still_has_the_strip() {
		let cells = temps("ВПУСК");
		let page = Frame::Values { cells: &cells };
		let (quiet, _) = paged(&page, &Strip::NONE, &Board::default());
		let mut under: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(UNDER);
		draw(&page, &Theme::bold_mono(), &mut under);
		assert!(page_under_strip_is(&quiet, &under), "the page under an empty strip");
		assert!(!lit_in(&quiet, strip_rows()), "and the strip dark");
		let mut with = panel();
		draw_with(&page, &board(BOTH, NINE), &Theme::bold_mono(), &mut with);
		assert!(page_under_strip_is(&with, &under), "`draw_with` is that page");
		assert!(lit_in(&with, strip_rows()), "with the runes and the badge in its strip");
	}

	#[test]
	fn the_strip_draws_only_what_it_has() {
		let cells = temps("ВПУСК");
		let page = Frame::Values { cells: &cells };
		let title_only = Strip {
			title: "ТЕМП",
			volts: Volts::Hidden,
		};
		let (display, _) = paged(&page, &title_only, &Board::default());
		let left = Rectangle::new(Point::zero(), Size::new(64, STRIP_ROWS));
		assert!(!lit_in(&display, left), "no voltage on a page that hides it");
		assert!(lit_in(&display, strip_rows()), "the title is there");

		let volts_only = Strip {
			title: "",
			volts: Volts::Shown(None),
		};
		let (display, report) = paged(&page, &volts_only, &Board::default());
		assert!(!report.glyph_missing, "{report:?}");
		let right = Rectangle::new(Point::new(64, 0), Size::new(PANEL.width - 64, STRIP_ROWS));
		assert!(lit_in(&display, left), "`--V`");
		assert!(!lit_in(&display, right), "and no title");
	}

	/// The strip's own colours whatever the page does: an alarmed cell under it inverts its
	/// column of the page, and the runes and the badge stay lit on the strip's dark ground.
	#[test]
	fn an_alarmed_cell_leaves_the_strip_alone() {
		let alarmed = [Cell::new("ОЖ", Some(93.0), "°C", 0), Cell::new("КОРОБКА", Some(128.0), "°C", 0).alarmed()];
		let steady = [Cell::new("ОЖ", Some(93.0), "°C", 0), Cell::new("КОРОБКА", Some(128.0), "°C", 0)];
		for said in boards() {
			let (on, _) = paged(&Frame::Values { cells: &alarmed }, &Strip::NONE, &said);
			let (off, _) = paged(&Frame::Values { cells: &steady }, &Strip::NONE, &said);
			assert!(strip_rows().points().all(|p| on.get_pixel(p) == off.get_pixel(p)), "{said:?}");
			assert!(lit(&on, PANEL.width as i32 - 1, STRIP_ROWS as i32), "{said:?}: the cell's ground is lit");
		}
	}

	#[test]
	fn under_a_title_a_charts_header_drops_its_label_and_keeps_its_range_and_seconds() {
		let samples = [1.5f32; 120];
		let chart = pinned_chart(&samples);
		let titled = Strip {
			title: "НАДДУВ",
			volts: Volts::Shown(Some(14.2)),
		};
		let said = board(BLE, NINE);
		let (display, report) = paged(&chart, &titled, &said);
		assert_eq!(report, Report::default(), "{report:?}");
		// `Д` reaches under the line, and still stays off the strip's last row.
		let last = Rectangle::new(Point::new(0, STRIP_ROWS as i32 - 1), Size::new(PANEL.width, 1));
		assert!(!lit_in(&display, last), "a dark row between the strip and the chart");
		let theme = Theme::bold_mono();
		let mut without: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(UNDER);
		draw_frame(&chart, None, &theme, &mut without, false);
		assert!(page_under_strip_is(&display, &without), "the chart without its label");
		let mut with: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(UNDER);
		draw(&chart, &theme, &mut with);
		assert!(!page_under_strip_is(&display, &with), "which is not the chart with it");
		// The range and the seconds moved to the left edge: the header's row has ink from there.
		let header = Rectangle::new(Point::new(0, STRIP_ROWS as i32), Size::new(8, 8));
		assert!(lit_in(&display, header), "the range starts the header");

		// A strip with the voltage alone keeps the chart's label: nothing else says what it is.
		let volts_only = Strip {
			title: "",
			volts: Volts::Shown(Some(14.2)),
		};
		let (display, _) = paged(&chart, &volts_only, &said);
		assert!(page_under_strip_is(&display, &with), "the chart as it is, under the strip");
	}

	#[test]
	fn a_title_that_runs_into_the_voltage_is_reported() {
		let cells = temps("ВПУСК");
		let page = Frame::Values { cells: &cells };
		let long = Strip {
			title: "ОЧЕНЬ ДЛИННЫЙ ЗАГОЛОВОК СТРАНИЦЫ НА ВЕСЬ ЭКРАН",
			volts: Volts::Shown(Some(14.2)),
		};
		let (_, report) = paged(&page, &long, &Board::default());
		assert!(report.label_overrun, "{report:?}");
		assert!(!report.glyph_missing, "Cyrillic is in the label face: {report:?}");
	}

	/// The title is checked against the right end as this frame draws it: what the board shows
	/// only takes room away, and a title one letter too long for it is reported. One dark column
	/// between the title and the right end is enough, as between the voltage and the title.
	#[test]
	fn a_title_that_runs_into_the_right_end_is_reported() {
		let cells = temps("ВПУСК");
		let page = Frame::Values { cells: &cells };
		let overruns = |n: usize, said: &Board| {
			let title: std::string::String = core::iter::repeat_n('Ж', n).collect();
			let strip = Strip {
				title: &title,
				volts: Volts::Hidden,
			};
			let (display, report) = paged(&page, &strip, said);
			(report.label_overrun, display)
		};
		let mut collided = false;
		for n in 1..60 {
			let (alone, _) = overruns(n, &Board::default());
			for said in boards() {
				let (beside, display) = overruns(n, &said);
				assert!(!alone || beside, "{n} letters, {said:?}: the right end only takes room away");
				collided |= beside && !alone;
				if !beside && let Some(left) = right_end(&said).left {
					// It fits: a dark column at least before the right end.
					let gap = Rectangle::new(Point::new(left - 1, 0), Size::new(1, STRIP_ROWS));
					assert!(!lit_in(&display, gap), "{n} letters, {said:?}: the column before the right end");
				}
			}
		}
		assert!(collided, "some title fits the strip but not beside the right end");
	}

	/// What `docs/dash/dash-toml.md`'s limits say: a title of 34 characters fits the strip with
	/// the voltage, both hosts and a two-digit count failing now; 35 do not. The face is fixed
	/// pitch, so any 34 letters are as wide as these.
	#[test]
	fn a_title_of_thirty_four_characters_fits_beside_the_widest_right_end_of_two_digits() {
		let cells = temps("ВПУСК");
		let page = Frame::Values { cells: &cells };
		let widest = board(
			BOTH,
			Some(Faults::Counted {
				stored: 99,
				failing_now: true,
			}),
		);
		let fits = |n: usize| {
			let title: std::string::String = core::iter::repeat_n('Ж', n).collect();
			let strip = Strip {
				title: &title,
				volts: Volts::Shown(Some(14.2)),
			};
			!paged(&page, &strip, &widest).1.label_overrun
		};
		assert!(fits(34));
		assert!(!fits(35));
	}

	/// A title that fits with nothing connected and overruns once both hosts and a count arrive
	/// is cut at the right end: the runes and the badge are the same pixels as under no title.
	#[test]
	fn a_title_too_long_is_cut_before_the_right_end_never_drawn_over_it() {
		let cells = temps("ВПУСК");
		let page = Frame::Values { cells: &cells };
		let widest = board(
			BOTH,
			Some(Faults::Counted {
				stored: 99,
				failing_now: true,
			}),
		);
		let title: std::string::String = core::iter::repeat_n('W', 40).collect();
		let long = Strip {
			title: &title,
			volts: Volts::Shown(Some(14.2)),
		};
		let none = Strip { title: "", ..long };
		let (with_title, report) = paged(&page, &long, &widest);
		let (without, _) = paged(&page, &none, &widest);
		assert!(report.label_overrun);
		// The right end's first lit column, as drawn under no title.
		let left = (0..256)
			.find(|&x| x > 128 && (0..STRIP_ROWS as i32).any(|y| lit(&without, x, y)))
			.expect("the right end draws");
		for x in left..256 {
			for y in 0..STRIP_ROWS as i32 {
				assert_eq!(lit(&with_title, x, y), lit(&without, x, y), "({x}, {y})");
			}
		}
	}

	#[test]
	fn the_adapter_screen_draws_no_strip_no_runes_and_no_badge() {
		let state = adapter_state(Some(500), false);
		let frame = Frame::Adapter(state);
		let strip = Strip {
			title: "Т",
			volts: Volts::Shown(Some(14.2)),
		};
		let mut plain = panel();
		draw(&frame, &Theme::bold_mono(), &mut plain);
		for said in boards() {
			let (display, _) = paged(&frame, &strip, &said);
			assert_eq!(display, plain, "{said:?}");
		}
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
		let board = Board {
			links: Links::NONE,
			rates,
			faults: None,
		};
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
		let (display, report, layout) = adapter_on(PANEL, state, rates);
		assert_eq!(report, Report::default(), "nothing overran and no glyph was missing");

		let boxes: std::vec::Vec<Rectangle> = layout.boxes().collect();
		assert_eq!(boxes.len(), 7, "every piece was placed: {layout:?}");
		let panel = Rectangle::new(Point::zero(), PANEL);
		for (i, a) in boxes.iter().enumerate() {
			assert_eq!(panel.intersection(a), *a, "{a:?} is inside the panel");
			for b in &boxes[i + 1..] {
				assert_eq!(a.intersection(b).size, Size::zero(), "{a:?} and {b:?} overlap");
			}
		}
		lit_only_inside(&display, &boxes);

		// `SLCAN` in the corner, at its margins; the rest centred, in order down the panel.
		let title = layout.title.unwrap().ink;
		assert_eq!(title.top_left, Point::new(ADAPTER_LEFT, ADAPTER_TOP));
		let (up, _, _, rx) = layout.speed.unwrap();
		let (rate, counters) = (layout.rate.unwrap().ink, layout.counters.unwrap().ink);
		let centred = |left: i32, right: i32| (left - (PANEL.width as i32 - right)).abs() <= 1;
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
		let (_, report, layout) = adapter_on(PANEL, state, None);
		assert!(report.label_overrun, "{report:?}");
		assert!(
			layout.counters.is_some_and(|c| c.ink.size.width > PANEL.width),
			"it is the counters, drawn anyway"
		);
	}

	#[test]
	fn on_a_short_panel_what_does_not_fit_the_height_is_reported_not_drawn_over() {
		let (display, report, layout) = adapter_on(SHORT, adapter_state(Some(500), false), None);
		assert!(report.label_overrun, "the lower lines have no room on 32 pixels: {report:?}");
		assert!(!report.glyph_missing, "{report:?}");
		assert!(layout.title.is_some() && layout.speed.is_some(), "the title and the speeds fit");
		assert!(layout.counters.is_none(), "the counters do not, and are not drawn");
		lit_only_inside(&display, &layout.boxes().collect::<std::vec::Vec<_>>());
	}
}
