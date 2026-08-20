//! Which faces the panel is drawn in, and why those.
//!
//! The stock `embedded_graphics` fonts are small and carry no Cyrillic, so the
//! labels come from `u8g2-fonts` — a port of the whole U8g2 collection, which
//! has both. The numbers are the interesting choice and there are two credible
//! answers, so both are here rather than one being assumed.

use eg_seven_segment::{SevenSegmentStyle, SevenSegmentStyleBuilder};
use embedded_graphics::geometry::Size;
use embedded_graphics::pixelcolor::BinaryColor;
use u8g2_fonts::FontRenderer;
use u8g2_fonts::fonts;

/// How the large numbers are drawn.
pub enum Numerals {
	/// A bitmap face.
	///
	/// Prefer a **monospaced** one. With a proportional face `111` and `888` are
	/// different widths, so a number twitches sideways as it changes — which on a
	/// gauge you glance at is worse than it sounds, because the movement reads as
	/// information when it is not.
	Font(FontRenderer),
	/// Seven segments, drawn from primitives rather than a bitmap.
	///
	/// Scales to any size for free, so "large font" becomes one integer instead
	/// of a second set of glyphs. Reads as an instrument; whether that is right
	/// for this panel is a matter of taste, which is why it is a variant and not
	/// a decision.
	Segments(SevenSegmentStyle<BinaryColor>),
}

/// The faces one panel is drawn with.
pub struct Theme {
	/// Cell labels and the chart header. Cyrillic-capable.
	pub label: FontRenderer,
	/// The unit string beside a value.
	///
	/// A different face from the label, and deliberately: u8g2's `_t_cyrillic`
	/// fonts carry no `°` — measured, not assumed — while the `_tf` ones do and
	/// carry no Cyrillic. Labels are words in the reader's language and units are
	/// `°C`, `bar`, `Nm`, `km/h`. Two jobs, two faces, and neither one silently
	/// missing a glyph the other has.
	pub unit: FontRenderer,
	/// The value itself, **largest first**.
	///
	/// A ladder rather than one face, because a 64-pixel column holds two digits
	/// comfortably and four not at all, and which of those a cell is depends on
	/// the reading rather than on the page: 93 °C and 4820 /min share a layout.
	/// The renderer takes the largest step that fits and reports that it had to
	/// come down, so the generator can reconsider the page instead of the panel
	/// silently shrinking under it.
	pub numerals: [Numerals; 3],
}

impl Theme {
	/// Inconsolata Bold 21 — monospaced, so digits do not shift as they change.
	/// The closest of these to the panel in the reference photograph.
	pub fn bold_mono() -> Self {
		Theme {
			label: FontRenderer::new::<fonts::u8g2_font_5x7_t_cyrillic>(),
			unit: FontRenderer::new::<fonts::u8g2_font_4x6_tf>(),
			numerals: [
				Numerals::Font(FontRenderer::new::<fonts::u8g2_font_inb21_mn>()),
				Numerals::Font(FontRenderer::new::<fonts::u8g2_font_inb19_mn>()),
				Numerals::Font(FontRenderer::new::<fonts::u8g2_font_inb16_mn>()),
			],
		}
	}

	/// FreeUniversal Bold 20 — proportional, heavier, and it fits more digits
	/// into a 64-pixel column at the cost of the twitch described above.
	pub fn heavy() -> Self {
		Theme {
			label: FontRenderer::new::<fonts::u8g2_font_5x7_t_cyrillic>(),
			unit: FontRenderer::new::<fonts::u8g2_font_4x6_tf>(),
			numerals: [
				Numerals::Font(FontRenderer::new::<fonts::u8g2_font_fub20_tn>()),
				Numerals::Font(FontRenderer::new::<fonts::u8g2_font_fub17_tn>()),
				Numerals::Font(FontRenderer::new::<fonts::u8g2_font_fub14_tn>()),
			],
		}
	}

	/// Seven-segment digits at the tallest size a 32-pixel panel allows once a
	/// label sits above them.
	pub fn segments() -> Self {
		Theme {
			label: FontRenderer::new::<fonts::u8g2_font_5x7_t_cyrillic>(),
			unit: FontRenderer::new::<fonts::u8g2_font_4x6_tf>(),
			numerals: [segments(13, 21, 3), segments(10, 21, 2), segments(7, 18, 2)],
		}
	}
}

fn segments(w: u32, h: u32, stroke: u32) -> Numerals {
	Numerals::Segments(
		SevenSegmentStyleBuilder::new()
			.digit_size(Size::new(w, h))
			.digit_spacing(2)
			.segment_width(stroke)
			.segment_color(BinaryColor::On)
			.build(),
	)
}
