//! Bars: a handful of live quantities, each drawn against the biggest it has
//! reached.
//!
//! **Why not a chart.** A line chart is the right picture of a run once the run
//! is over, and the wrong one while it is happening. In a terminal it has one Y
//! axis for values that are 100 km/h beside 6000 /min, so all but one line is
//! folded onto somebody else's scale; it needs paging keys because three lines
//! is all that fits; and the thing a driver wants at 100 km/h — *where is this
//! number now, and is it near the best it has been* — is exactly what a
//! shrinking time axis makes hardest to read. The browser page keeps the chart,
//! because that is where a finished run is read.
//!
//! **Every bar has its own scale, and that scale is the session's own peak.**
//! There is no table of full-scale values here and there must never be one: a
//! pedal that reads 102 % at full travel, an engine that turns 6500, a car that
//! makes 142 kW are all facts about one car, and `CLAUDE.md` forbids writing
//! those into the source. The peak a channel has actually reached is a fact this
//! session measured, so it is what the bar is drawn against — and it is printed
//! beside the bar, because a proportion whose reference is invisible says
//! nothing.
//!
//! **A computed quantity is filled differently from a measured one.** Power and
//! acceleration are worked out here; speed and engine speed came off the bus.
//! The value table draws that distinction in a column and the chart drew it in
//! the key, and a bar that dropped it would be the one place on screen where an
//! estimate looks like a reading.

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::chart::{Origin, Series};

/// The character a bar is filled with, by where its number came from.
///
/// Two glyphs of the same width, so a reading and an estimate of the same size
/// occupy the same space and can be compared down the column.
const FILL_MEASURED: char = '█';
const FILL_COMPUTED: char = '▒';
const EMPTY: char = '·';

/// One quantity, as much of it as there has been, and how much there is now.
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
	pub label: String,
	pub unit: String,
	/// The latest value, which is what the number says.
	pub value: f64,
	/// The largest magnitude this channel has reached in the session, which is
	/// what the bar is drawn against. Never a constant from a table — see the
	/// module doc.
	pub peak: f64,
	pub origin: Origin,
}

impl Bar {
	/// How full the bar is, in `0.0..=1.0`.
	///
	/// Against the peak *magnitude*, so a channel that goes negative — which
	/// acceleration does the moment a driver lifts — still has a bar rather than
	/// a length below zero. The sign stays where it can be read exactly, on the
	/// number.
	fn fraction(&self) -> f64 {
		match self.peak.abs() {
			peak if peak > 0.0 => (self.value.abs() / peak).clamp(0.0, 1.0),
			// Nothing has happened yet. An empty bar is the honest picture; a
			// full one would be the result of dividing by nothing.
			_ => 0.0,
		}
	}

	/// The number, in as many decimals as the quantity is worth reading in.
	///
	/// Driven by the size of the peak rather than by the channel's name, so it
	/// holds for a car whose engine speed, boost or pedal are spelled
	/// differently from the reference car's. A tachometer at 4820 gains nothing
	/// from a decimal point; an acceleration of 3.2 loses everything without
	/// one.
	fn figure(value: f64, peak: f64) -> String {
		match peak.abs() {
			p if p >= 1000.0 => format!("{value:.0}"),
			p if p >= 100.0 => format!("{value:.1}"),
			_ => format!("{value:.2}"),
		}
	}
}

/// Turn the series the poll loop already assembles into bars.
///
/// `take` is how many the screen has room for; the caller's order is kept,
/// because the order the series arrive in is the order a driver reads them and
/// deciding it twice is how the two come to disagree.
///
/// A series with no points yet is skipped rather than drawn empty: before the
/// car has said anything there is nothing to be proportional to, and a row of
/// empty bars reads as a car answering with zeroes.
pub fn bars(series: &[Series], take: usize) -> Vec<Bar> {
	series
		.iter()
		.filter_map(|s| {
			let value = s.points.last()?.1;
			let peak = s.points.iter().map(|p| p.1.abs()).fold(0.0, f64::max);
			Some(Bar {
				label: s.label.clone(),
				unit: s.unit.clone(),
				value,
				peak,
				origin: s.origin,
			})
		})
		.take(take)
		.collect()
}

/// Draw the bars, filling the area.
///
/// The columns are sized from the content: a label column, a number column, the
/// bar itself taking whatever is left, and the peak. When the area is too narrow
/// for a bar to mean anything the bar is dropped and the numbers stay, because
/// the numbers are the measurement and the bar is the way of comparing them.
pub fn draw(frame: &mut Frame, bars: &[Bar], area: Rect) {
	let block = Block::default().borders(Borders::ALL).title(" live ");
	if bars.is_empty() {
		frame.render_widget(block.title(" live — nothing read yet "), area);
		return;
	}
	let inner = block.inner(area);
	frame.render_widget(block, area);

	let figures: Vec<String> = bars
		.iter()
		.map(|b| {
			let number = Bar::figure(b.value, b.peak);
			match b.unit.is_empty() {
				true => number,
				false => format!("{number} {}", b.unit),
			}
		})
		.collect();
	let peaks: Vec<String> = bars.iter().map(|b| format!("peak {}", Bar::figure(b.peak, b.peak))).collect();

	let width_of = |texts: &[String]| texts.iter().map(|t| t.chars().count()).max().unwrap_or(0);
	let label_w = bars.iter().map(|b| b.label.chars().count()).max().unwrap_or(0);
	let figure_w = width_of(&figures);
	let peak_w = width_of(&peaks);

	// Three single spaces between four columns, and the bar wants at least a
	// handful of cells before it is worth drawing at all.
	let fixed = label_w + figure_w + peak_w + 3;
	let bar_w = (inner.width as usize).saturating_sub(fixed);
	let bar_w = if bar_w >= 6 { bar_w } else { 0 };

	let lines: Vec<Line> = bars
		.iter()
		.zip(figures.iter().zip(peaks.iter()))
		.map(|(bar, (figure, peak))| {
			let fill = match bar.origin {
				Origin::Bus => FILL_MEASURED,
				Origin::Computed(_) => FILL_COMPUTED,
			};
			let filled = (bar.fraction() * bar_w as f64).round() as usize;
			let drawn: String = std::iter::repeat_n(fill, filled.min(bar_w))
				.chain(std::iter::repeat_n(EMPTY, bar_w.saturating_sub(filled)))
				.collect();
			let dim = Style::default().fg(Color::DarkGray);
			let mut spans = vec![
				Span::raw(format!("{:<label_w$} ", bar.label)),
				Span::styled(format!("{figure:>figure_w$} "), Style::default().add_modifier(Modifier::BOLD)),
			];
			if bar_w > 0 {
				spans.push(Span::styled(
					format!("{drawn} "),
					match bar.origin {
						Origin::Bus => Style::default().fg(Color::Green),
						Origin::Computed(_) => Style::default().fg(Color::Cyan),
					},
				));
			}
			spans.push(Span::styled(peak.clone(), dim));
			Line::from(spans)
		})
		.collect();

	frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
	use super::*;
	use ratatui::Terminal;
	use ratatui::backend::TestBackend;

	fn series(label: &str, unit: &str, values: &[f64], origin: Origin) -> Series {
		Series {
			label: label.to_string(),
			unit: unit.to_string(),
			points: values.iter().enumerate().map(|(i, v)| (i as f64 * 0.1, *v)).collect(),
			origin,
		}
	}

	fn rendered(bars: &[Bar], w: u16, h: u16) -> Vec<String> {
		let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
		terminal.draw(|frame| draw(frame, bars, frame.area())).unwrap();
		let buffer = terminal.backend().buffer().clone();
		(0..h)
			.map(|y| (0..w).map(|x| buffer[(x, y)].symbol().to_string()).collect::<String>())
			.collect()
	}

	#[test]
	fn a_bar_is_drawn_against_the_biggest_this_session_has_seen() {
		// Half of the peak is half a bar, whatever the quantity is measured in.
		// Nothing here knows what a plausible road speed is, and that is the
		// point: the scale is measured, not tabulated.
		let bar = Bar {
			label: "speed".into(),
			unit: "km/h".into(),
			value: 50.0,
			peak: 100.0,
			origin: Origin::Bus,
		};
		assert!((bar.fraction() - 0.5).abs() < 1e-9);
	}

	#[test]
	fn a_channel_that_has_gone_negative_still_has_a_bar() {
		// Acceleration goes negative the moment a driver lifts. Drawing the
		// magnitude keeps the row where it is; the sign is on the number, where
		// it can be read exactly.
		let bar = Bar {
			label: "accel".into(),
			unit: "m/s2".into(),
			value: -2.0,
			peak: 4.0,
			origin: Origin::Computed("trailing"),
		};
		assert!((bar.fraction() - 0.5).abs() < 1e-9);
		assert_eq!(Bar::figure(bar.value, bar.peak), "-2.00");
	}

	#[test]
	fn before_the_car_has_said_anything_there_is_nothing_to_be_proportional_to() {
		// A peak of zero would divide by nothing. An empty bar is honest; a
		// full one would be an artefact of the arithmetic.
		let bar = Bar {
			label: "power".into(),
			unit: "kW".into(),
			value: 0.0,
			peak: 0.0,
			origin: Origin::Bus,
		};
		assert_eq!(bar.fraction(), 0.0);
	}

	#[test]
	fn the_series_the_loop_assembles_become_bars_in_the_order_they_arrive() {
		let series = vec![
			series("speed", "km/h", &[0.0, 40.0, 87.4], Origin::Bus),
			series("engine speed", "/min", &[900.0, 5450.0, 4820.0], Origin::Bus),
			series("power", "kW", &[0.0, 142.0, 118.0], Origin::Computed("estimate")),
			// Not read yet: no points, so no bar rather than a bar of zero.
			Series {
				label: "boost actual".into(),
				unit: "bar".into(),
				points: vec![],
				origin: Origin::Bus,
			},
		];
		let bars = bars(&series, 5);
		assert_eq!(bars.len(), 3, "{bars:?}");
		assert_eq!(bars[0].label, "speed");
		assert_eq!(bars[0].value, 87.4);
		assert_eq!(bars[0].peak, 87.4);
		// The peak is the session's, not the latest value: the engine has been
		// to 5450 and is on its way back down.
		assert_eq!(bars[1].peak, 5450.0);
		assert_eq!(bars[1].value, 4820.0);
		assert_eq!(bars[2].origin, Origin::Computed("estimate"));
	}

	#[test]
	fn take_is_how_many_the_screen_has_room_for() {
		let all: Vec<Series> = (0..8).map(|i| series(&format!("s{i}"), "", &[1.0, 2.0], Origin::Bus)).collect();
		assert_eq!(bars(&all, 5).len(), 5);
	}

	#[test]
	fn an_estimate_is_filled_with_a_different_character_from_a_reading() {
		// The one place on screen where a computed figure could pass for a
		// measured one, so it is the one place that must not.
		let drawn = rendered(
			&[
				Bar {
					label: "speed".into(),
					unit: "km/h".into(),
					value: 100.0,
					peak: 100.0,
					origin: Origin::Bus,
				},
				Bar {
					label: "power".into(),
					unit: "kW".into(),
					value: 142.0,
					peak: 142.0,
					origin: Origin::Computed("estimate"),
				},
			],
			60,
			4,
		);
		let text = drawn.join("\n");
		assert!(text.contains(FILL_MEASURED), "{text}");
		assert!(text.contains(FILL_COMPUTED), "{text}");
	}

	#[test]
	fn the_peak_is_printed_beside_the_bar_because_a_proportion_needs_its_reference() {
		let drawn = rendered(
			&[Bar {
				label: "engine speed".into(),
				unit: "/min".into(),
				value: 4820.0,
				peak: 5450.0,
				origin: Origin::Bus,
			}],
			60,
			3,
		);
		let text = drawn.join("\n");
		assert!(text.contains("4820 /min"), "{text}");
		assert!(text.contains("peak 5450"), "{text}");
	}

	#[test]
	fn a_terminal_too_narrow_for_a_bar_keeps_the_numbers() {
		// The numbers are the measurement; the bar is a way of comparing them.
		// Losing the measurement to keep the comparison would be backwards.
		let drawn = rendered(
			&[Bar {
				label: "speed".into(),
				unit: "km/h".into(),
				value: 87.4,
				peak: 104.2,
				origin: Origin::Bus,
			}],
			30,
			3,
		);
		let text = drawn.join("\n");
		assert!(text.contains("87.4 km/h"), "{text}");
		assert!(!text.contains(FILL_MEASURED), "no room for a bar, so no bar: {text}");
	}

	#[test]
	fn nothing_read_yet_says_so_rather_than_drawing_an_empty_frame() {
		let drawn = rendered(&[], 40, 3);
		assert!(drawn.join("\n").contains("nothing read yet"), "{drawn:?}");
	}
}
