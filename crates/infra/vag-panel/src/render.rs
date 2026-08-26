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
use embedded_graphics::primitives::{Line, PrimitiveStyle, Rectangle};
use embedded_graphics::text::Text;
use u8g2_fonts::FontRenderer;
use u8g2_fonts::types::{FontColor, HorizontalAlignment, VerticalPosition};

use crate::frame::{Cell, Frame};
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

/// Draw one frame. Returns what did not fit.
pub fn draw<D>(frame: &Frame<'_>, theme: &Theme, target: &mut D) -> Report
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
			window_seconds,
		} => chart(cell, *min, *max, samples, *window_seconds, theme, target),
	}
}

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
		draw_label(cell.label, centre, inner, &theme.label, ink, target, &mut report);
		draw_value(cell, centre, inner, height, theme, ink, &layout, target, &mut report);
	}
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

fn draw_label<D>(label: &str, centre: i32, w: u32, font: &FontRenderer, ink: BinaryColor, target: &mut D, report: &mut Report)
where
	D: DrawTarget<Color = BinaryColor>,
{
	if let Ok(Some(bounds)) = font.get_rendered_dimensions_aligned(label, Point::new(centre, 0), VerticalPosition::Top, HorizontalAlignment::Center)
		&& bounds.size.width > w
	{
		report.label_overrun = true;
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

#[allow(clippy::too_many_arguments)]
fn chart<D>(cell: &Cell<'_>, min: f32, max: f32, samples: &[f32], window_seconds: f32, theme: &Theme, target: &mut D) -> Report
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
	let mut tail = Buf::new();
	let _ = write!(
		tail,
		"{:.*}-{:.*}{}  {:.0}s",
		cell.decimals as usize, min, cell.decimals as usize, max, cell.unit, window_seconds
	);
	if theme
		.unit
		.render(
			tail.as_str(),
			Point::new(after, 1),
			VerticalPosition::Top,
			FontColor::Transparent(ink),
			target,
		)
		.is_err()
	{
		report.glyph_missing = true;
	}

	let buf = number(cell);
	// A chart gives the number a third of the width; past that the trace has
	// nowhere left to be, and a chart with no room for its trace is a bad table.
	let (step, value_w, _) = fit(&theme.numerals, buf.as_str(), 0, width / 3);
	if step > 0 {
		report.value_shrunk = true;
	}
	// Centred in the band under the header, for the same reason as the values
	// page: the number is what the eye came for, and on the floor it reads as an
	// afterthought under the trace.
	let plot_top = 8;
	let value_h = numeral_height(&theme.numerals[step], buf.as_str());
	let value_baseline = plot_top + (height as i32 - 1 - plot_top - value_h as i32) / 2 + value_h as i32;
	draw_numerals(&theme.numerals[step], buf.as_str(), Point::new(0, value_baseline), ink, target);

	let plot_x = value_w as i32 + 4;
	let plot_bottom = height as i32 - 1;
	let plot_w = width as i32 - plot_x;
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

	// Only as many columns as there are samples. A shorter trace is the truth
	// about a run that has just started; stretching it across the width would
	// invent history.
	let n = samples.len().min(plot_w as usize);
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
		let layout = row_layout(&cells, &theme, INNER, 64, &mut report);
		assert!(layout.tiered, "three tiers fit in 64 rows");
		assert!(layout.with_unit, "a stacked unit is never dropped for width");

		let mut display: SimulatorDisplay<BinaryColor> = SimulatorDisplay::new(Size::new(256, 64));
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
			window_seconds: 19.0,
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
}
