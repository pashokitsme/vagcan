//! The chart: which lines share a page, how they are folded onto one axis, and
//! the key that keeps that fold honest.
//!
//! `ratatui::Chart` has one Y axis and no second one to draw on, and the lines
//! a car offers are 100 km/h beside 6000 /min beside 2.1 bar. So one unit owns
//! the drawn axis and everything else is folded onto it, and the range each
//! folded line came from is printed in the key. That is not a workaround: a
//! curve whose axis is nowhere is decoration rather than data, and the range is
//! that axis, in words.
//!
//! **[`plot`] decides and [`draw`] renders**, because the deciding has no
//! readable output. The fold's only trace on the screen is which braille cells
//! are lit, so it cannot be asserted through a rendered buffer at all; against
//! the [`Plot`] that comes out of `plot` it is an equality. Nothing is worked
//! out on the [`draw`] side.
//!
//! **Two parameters and a width.** The caller says which series, in what order,
//! and what is in their buffers; everything else — paging, folding, the key, the
//! palette, dropping to fit, the bounds, the time origin — is decided here.
//! There is deliberately no `show_key`, no `max_lines`, no `max_units` and no
//! palette argument: a parameter that can turn a chart into a lie is not a
//! parameter, and two callers wanting different caps is one widget with a bug.
//! If a second caller ever needs a third parameter, the question to ask is why
//! this module cannot work it out from the width it already has.
//!
//! **A [`Series`] carries plain `(t, v)` pairs and not `measure`'s `Track`.**
//! `Track` is a numerics type — it interpolates, it finds crossings, it windows
//! — and it belongs with the physics that needs those. Defining it here would
//! make the physics import from the drawing, which is backwards; importing it
//! from `measure` would make this module a `measure` helper wearing a widget's
//! name. Neither: a chart's input is a list of points, the caller hands one
//! over, and the two sides share no type at all.

use ratatui::prelude::*;
use ratatui::symbols::Marker;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType};

/// Where a number came from.
///
/// Kept apart from the number itself, wherever one is shown, because a figure
/// that was never on the bus must not look like one that was: the value table
/// gives it a column and the chart gives it a marker and a word in the key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
	/// The car reported it.
	Bus,
	/// This tool worked it out. The qualifier is the one that matters for
	/// reading it: live acceleration can only be causal, and power is an
	/// estimate.
	Computed(&'static str),
}

/// One series the chart can show.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
	/// What the key calls it.
	pub label: String,
	/// What it is measured in — the catalog's own word for a channel it
	/// resolved, or the unit the computation produces for one this tool worked
	/// out. It is what groups lines onto a scale, so a series without one has a
	/// scale of its own by definition.
	pub unit: String,
	/// The line itself, as `(seconds, value)` pairs in time order.
	///
	/// A plain list rather than the caller's own buffer type: see the module
	/// doc — a chart and a set of physics have no business sharing a type.
	pub points: Vec<(f64, f64)>,
	/// Where the line came from, the same distinction the value table draws.
	///
	/// The chart is the one place it would be easiest to drop, and a line whose
	/// origin is not stated is indistinguishable from a measurement. A derived
	/// series' running end is causal by construction, which is what
	/// `Computed("trailing")` says.
	pub origin: Origin,
}

impl Series {
	fn computed(&self) -> bool {
		matches!(self.origin, Origin::Computed(_))
	}

	/// The lowest and highest value in the series, or `None` when there is
	/// nothing to bound — an empty series, or one that has not moved.
	fn span(&self) -> Option<(f64, f64)> {
		let low = self.points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
		let high = self.points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
		(low.is_finite() && high.is_finite()).then_some((low, high))
	}
}

/// How many lines one chart carries, and how many scales it puts them on.
///
/// The browser page caps a hand-picked set at three series and two Y axes, and
/// says why: rpm at 6480 beside boost at 2.1 bar destroys both scales, and
/// dropping one quietly is worse than saying so. A terminal has a fraction of
/// the room and **no second Y axis to draw on at all** — `ratatui::Chart` has
/// one — so the cap here is the same three lines and a stricter two units: one
/// drawn on the axis in its own numbers, the other folded onto that axis and
/// printed in the key with the range it was folded from. A curve whose axis is
/// not shown would be decoration; the range is that axis, in words.
///
/// What does not fit is one `←`/`→` away, and the key says which page this is.
const MAX_LINES: usize = 3;
const MAX_UNITS: usize = 2;

/// One colour per line, in a fixed order so a series does not change colour from
/// one cycle to the next.
///
/// Colour is how a driver tells three lines apart at a glance. It is not the
/// only way they are told apart — a computed line is drawn with a different
/// marker and named with a different glyph in the key — because a terminal that
/// will not colour, or an eye that will not separate these three, must still be
/// able to read the chart.
const LINE_COLOURS: [Color; MAX_LINES] = [Color::Cyan, Color::Magenta, Color::Yellow];

/// Which series share one chart, in the order [`Series`] came in.
///
/// Greedy and stable: a page takes series until it would exceed [`MAX_LINES`]
/// or [`MAX_UNITS`], then a new page starts. Stable matters more than clever —
/// `←`/`→` has to mean the same thing on the next cycle as it did on this one,
/// and a page that reshuffles itself as a channel starts answering is a page
/// nobody can navigate at 100 km/h.
pub fn pages(series: &[Series]) -> Vec<Vec<usize>> {
	let mut pages: Vec<Vec<usize>> = Vec::new();
	for (i, next) in series.iter().enumerate() {
		let fits = pages.last().is_some_and(|page: &Vec<usize>| {
			if page.len() >= MAX_LINES {
				return false;
			}
			let mut units: Vec<&str> = page.iter().map(|j| series[*j].unit.as_str()).collect();
			units.sort_unstable();
			units.dedup();
			units.contains(&next.unit.as_str()) || units.len() < MAX_UNITS
		});
		match fits {
			true => pages.last_mut().expect("a page to add to").push(i),
			false => pages.push(vec![i]),
		}
	}
	pages
}

/// One line of a [`Plot`], as a renderer needs it and no more.
#[derive(Clone, Debug, PartialEq)]
pub struct PlotLine {
	/// What the key calls it.
	pub label: String,
	/// What it is measured in. One unit owns the drawn axis and the rest are
	/// folded onto it, so this is also what says whether a line was folded: it
	/// was, whenever it differs from [`Plot::axis_unit`].
	pub unit: String,
	/// Which colour, as an index into the palette rather than a
	/// `ratatui::Color`. The palette is the renderer's, and its fixed order is
	/// what keeps a series the same colour from one cycle to the next.
	pub colour: usize,
	/// A computed line is dotted where a read one is solid, so the distinction
	/// survives a terminal that will not colour.
	pub dotted: bool,
	/// The range this line was folded from, in its own numbers.
	///
	/// It is what the key prints, and it is the folded line's axis in words: a
	/// curve whose axis is nowhere is decoration rather than data. Nothing
	/// outside this module builds a `PlotLine`, so there is no way for a caller
	/// to have a folded line without one. `None` for the line that owns the
	/// axis, and for a folded line with nothing in it yet — that one draws no
	/// curve, so there is no scale to be misread.
	pub folded_from: Option<(f64, f64)>,
	/// The qualifier a computed line carries into the key.
	pub note: Option<&'static str>,
	/// Seconds from the start of the window against the drawn axis: already
	/// folded, already shifted, ready to hand to a `Dataset`.
	pub points: Vec<(f64, f64)>,
}

/// Everything the chart draws, and nothing about how it is drawn.
///
/// It exists because the arithmetic that produces it has no readable output.
/// The fold's only trace on the screen is which braille cells are lit, so the
/// one property it has to satisfy — a line folded onto somebody else's axis
/// lands where its own range says it should — cannot be reached through a
/// rendered buffer at all. Against a `Plot` it is an equality.
#[derive(Clone, Debug, PartialEq)]
pub struct Plot {
	/// The unit drawn on the Y axis: whatever the first line on the page is in.
	pub axis_unit: String,
	/// The Y bounds, in `axis_unit`'s own numbers.
	pub y: (f64, f64),
	/// The X bounds in seconds, counted from the earliest point drawn — so the
	/// low end is zero, and the high end is how long the window is.
	pub x: (f64, f64),
	pub lines: Vec<PlotLine>,
	/// How many lines came off the tail of the page because the key would not
	/// fit. Carried out rather than left for the driver to notice: a screen
	/// that drops a series must say it dropped it.
	pub dropped: usize,
	/// Which page this is, and how many there are.
	pub page: (usize, usize),
}

/// A flat or empty group still needs bounds, or the chart draws nothing and
/// looks like a failure to read the car.
fn widen(span: Option<(f64, f64)>) -> (f64, f64) {
	match span {
		Some((lo, hi)) if hi > lo => (lo, hi),
		Some((lo, _)) => (lo - 0.5, lo + 0.5),
		None => (0.0, 1.0),
	}
}

/// What one page of the chart shows — every decision, and nothing rendered.
///
/// **The lines have wildly different scales** — 100 km/h against 6000 rpm — so
/// one unit owns the drawn axis and everything else is folded onto it. Folding
/// is what keeps a 6000-rpm line from flattening the speed trace into a stripe
/// along the bottom, and the price of it is that the folded line's numbers are
/// not on the axis. They are in the key instead, as the range the fold came
/// from.
///
/// `width` and no `Rect`, because the one thing the width decides is whether
/// the key fits, and fitting the key is what decides how many lines survive.
/// Nothing else here is a property of the area it will be drawn in.
///
/// `None` when there is no page to show, which is what a series list with
/// nothing in it means — the caller says what an empty chart looks like,
/// because "nothing read yet" is a sentence about the car and not about a plot.
pub fn plot(series: &[Series], page: usize, width: u16) -> Option<Plot> {
	let pages = pages(series);
	let count = pages.len();
	// A page index that has run past the end is clamped rather than refused:
	// the count changes as channels start answering, and `←`/`→` must not
	// strand the screen on a page that stopped existing.
	let page = page.min(count.saturating_sub(1));
	let on_page = pages.get(page)?;

	// The unit that came first owns the axis; the rest are folded onto it.
	let axis_unit = series[on_page[0]].unit.clone();
	let mut lines: Vec<PlotLine> = on_page
		.iter()
		.enumerate()
		.map(|(n, i)| {
			let source = &series[*i];
			PlotLine {
				label: source.label.clone(),
				unit: source.unit.clone(),
				colour: n,
				dotted: source.computed(),
				folded_from: match source.unit == axis_unit {
					true => None,
					false => source.span(),
				},
				note: match source.origin {
					Origin::Bus => None,
					Origin::Computed(note) => Some(note),
				},
				points: Vec::new(),
			}
		})
		.collect();

	// The key has to fit or it is not a key. Lines come off the tail of the
	// page until it does, and the count that went travels with the plot.
	//
	// Nothing in the key depends on the points, so this runs before they are
	// built: what gets dropped is never folded, and the loop measures the same
	// `Line` the border will carry rather than an estimate of it.
	let room = width.saturating_sub(2) as usize;
	let mut dropped = 0usize;
	while lines.len() > 1 && key_line(&lines, dropped, (page, count)).width() > room {
		lines.pop();
		dropped += 1;
	}
	let drawn = &on_page[..lines.len()];

	let group_span = |unit: &str| {
		drawn
			.iter()
			.filter(|i| series[**i].unit == unit)
			.filter_map(|i| series[*i].span())
			.reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
	};
	let y = widen(group_span(&axis_unit));

	// Time is the same for every line, and it is counted from the oldest sample
	// handed over rather than from any clock this module knows about. What the
	// axis then reads as belongs to the caller: `measure` empties its buffers at
	// each launch, so during a run its chart reads as seconds since the car set
	// off. Trimming is the caller's for the same reason.
	let t0 = drawn
		.iter()
		.filter_map(|i| series[*i].points.first().map(|p| p.0))
		.fold(f64::INFINITY, f64::min);
	let t1 = drawn
		.iter()
		.filter_map(|i| series[*i].points.last().map(|p| p.0))
		.fold(f64::NEG_INFINITY, f64::max);
	let (t0, t1) = match t0.is_finite() && t1 > t0 {
		true => (t0, t1),
		false => (0.0, 1.0),
	};

	for (line, i) in lines.iter_mut().zip(drawn) {
		let source = &series[*i];
		let (lo, hi) = widen(group_span(&source.unit));
		let fold = source.unit != axis_unit;
		line.points = source
			.points
			.iter()
			.map(|(t, v)| {
				let v = match fold {
					true => y.0 + (v - lo) / (hi - lo) * (y.1 - y.0),
					false => *v,
				};
				(t - t0, v)
			})
			.collect();
	}

	Some(Plot {
		axis_unit,
		y,
		x: (0.0, t1 - t0),
		lines,
		dropped,
		page: (page, count),
	})
}

/// A number as a person reads it off an axis: as many decimals as its size
/// leaves room to mean anything, and no more.
fn tick(value: f64) -> String {
	match value.abs() {
		v if v >= 100.0 => format!("{value:.0}"),
		v if v >= 10.0 => format!("{value:.1}"),
		_ => format!("{value:.2}"),
	}
}

/// One page's lines, overlaid, with the key that makes them readable.
///
/// **Nothing is worked out here.** Every number is already in the [`Plot`];
/// arithmetic that happened at this end would be arithmetic no test could
/// reach, which is the whole reason for the split.
pub fn draw(frame: &mut Frame, plot: &Plot, area: Rect) {
	let data: Vec<Dataset> = plot
		.lines
		.iter()
		.map(|line| {
			Dataset::default()
				// A computed line is dotted where a read one is solid, so the
				// distinction survives a terminal that will not colour.
				.marker(match line.dotted {
					true => Marker::Dot,
					false => Marker::Braille,
				})
				.graph_type(GraphType::Line)
				.style(Style::default().fg(colour(line.colour)))
				.data(&line.points)
		})
		.collect();

	let chart = Chart::new(data)
		.block(
			Block::default()
				.borders(Borders::ALL)
				.title(key_line(&plot.lines, plot.dropped, plot.page))
				.title_bottom(notes_line(plot)),
		)
		.x_axis(
			Axis::default()
				.title("time")
				.bounds([plot.x.0, plot.x.1])
				.labels([format!("{:.0}s", plot.x.0), format!("{:.1}s", plot.x.1)]),
		)
		.y_axis(
			Axis::default()
				.title(plot.axis_unit.clone())
				.bounds([plot.y.0, plot.y.1])
				.labels([tick(plot.y.0), tick(plot.y.1)]),
		);
	frame.render_widget(chart, area);
}

/// The palette, looked up. A [`PlotLine`] carries an index and this is the only
/// place it becomes a colour.
fn colour(n: usize) -> Color {
	LINE_COLOURS[n % LINE_COLOURS.len()]
}

/// The key: what each line is, in its own colour, with the range of any line
/// that had to be folded onto somebody else's axis.
///
/// Built from the lines alone, so [`plot`] can measure it while it is still
/// deciding how many of them there is room for. That it is a `Line` rather than
/// a `String` is what makes the measurement the true one: the width the drop
/// loop compares against is the width the border will actually take.
fn key_line<'a>(lines: &[PlotLine], dropped: usize, page: (usize, usize)) -> Line<'a> {
	let mut spans = vec![Span::raw(" ")];
	for (n, line) in lines.iter().enumerate() {
		if n > 0 {
			spans.push(Span::styled(" · ", Style::default().fg(Color::DarkGray)));
		}
		let mut text = String::new();
		if line.dotted {
			text.push('⋯');
		}
		text.push_str(&line.label);
		// Only a folded line carries a range, which is what makes the range the
		// marker: the axis says everything about the line that owns it.
		if let Some((lo, hi)) = line.folded_from {
			text.push_str(&format!(" [{}…{} {}]", tick(lo), tick(hi), line.unit));
		}
		if let Some(note) = line.note {
			text.push(' ');
			text.push_str(note);
		}
		spans.push(Span::styled(text, Style::default().fg(colour(line.colour))));
	}
	if dropped > 0 {
		spans.push(Span::styled(format!("  +{dropped} no room"), Style::default().fg(Color::DarkGray)));
	}
	let (page, pages) = page;
	if pages > 1 {
		spans.push(Span::styled(format!("  {}/{pages}", page + 1), Style::default().fg(Color::DarkGray)));
	}
	spans.push(Span::raw(" "));
	Line::from(spans)
}

/// The bottom border: what the glyphs on this particular chart mean, and
/// nothing about the ones it is not carrying.
fn notes_line<'a>(plot: &Plot) -> Line<'a> {
	let mut notes: Vec<String> = Vec::new();
	if plot.lines.iter().any(|line| line.dotted) {
		notes.push("⋯ computed".to_string());
	}
	// A line in a unit the axis is not drawn in is a folded line, whether or
	// not it has anything in it yet to fold.
	if plot.lines.iter().any(|line| line.unit != plot.axis_unit) {
		notes.push("[ ] own scale".to_string());
	}
	if plot.page.1 > 1 {
		notes.push("←→ chart".to_string());
	}
	match notes.is_empty() {
		true => Line::default(),
		false => Line::styled(format!(" {} ", notes.join(" · ")), Style::default().fg(Color::DarkGray)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_page_stops_at_three_lines_and_two_scales_and_the_rest_gets_its_own_page() {
		let series = |label: &str, unit: &str| read(label, unit, &[]);
		// Four units: two per page, in the order they were offered.
		let four = [
			series("speed", "km/h"),
			series("engine speed", "/min"),
			series("power", "kW"),
			series("accel", "m/s²"),
		];
		assert_eq!(pages(&four), vec![vec![0, 1], vec![2, 3]]);

		// Series that share a unit share a scale, so three of them are one page
		// — and a fourth starts another even though it costs no new scale.
		let same = [
			series("boost actual", "bar"),
			series("boost specified", "bar"),
			series("boost peak", "bar"),
			series("boost held", "bar"),
		];
		assert_eq!(pages(&same), vec![vec![0, 1, 2], vec![3]]);

		assert!(pages(&[]).is_empty(), "nothing read is no pages, not one empty one");
	}

	/// A series the car reported, from `(t, v)` pairs.
	fn read(label: &str, unit: &str, points: &[(f64, f64)]) -> Series {
		Series {
			label: label.into(),
			unit: unit.into(),
			points: points.to_vec(),
			origin: Origin::Bus,
		}
	}

	/// Speed and engine speed over one second, which is the pair the fold was
	/// written for: 100 km/h against 6480 /min on one axis.
	fn speed() -> Series {
		read("speed", "km/h", &[(0.0, 0.0), (0.5, 50.0), (1.0, 100.0)])
	}

	fn engine() -> Series {
		read("engine", "1/min", &[(0.0, 800.0), (0.5, 3640.0), (1.0, 6480.0)])
	}

	#[test]
	fn a_folded_line_lands_exactly_where_its_own_range_says_it_should() {
		// The one property the fold has to satisfy, and the one no rendered
		// screen can be asked about: its only visible output is which braille
		// cells are lit. 800 is the bottom of the engine's range, so it lands on
		// the bottom of the speed axis; 6480 is the top, so it lands on the top;
		// and 3640 is halfway, so it lands halfway. Not "somewhere sensible".
		let plotted = plot(&[speed(), engine()], 0, 80).expect("a page");
		assert_eq!(plotted.axis_unit, "km/h");
		assert_eq!(plotted.y, (0.0, 100.0));
		assert_eq!(plotted.x, (0.0, 1.0));
		assert_eq!(plotted.lines[0].points, [(0.0, 0.0), (0.5, 50.0), (1.0, 100.0)]);
		assert_eq!(plotted.lines[1].points, [(0.0, 0.0), (0.5, 50.0), (1.0, 100.0)]);

		// And the line that owns the axis is the one without a range in the key,
		// because the axis is already its range.
		assert_eq!(plotted.lines[0].folded_from, None);
		assert_eq!(plotted.lines[1].folded_from, Some((800.0, 6480.0)));
		assert_eq!(plotted.lines[1].unit, "1/min");
	}

	#[test]
	fn the_axis_belongs_to_whichever_unit_came_first_and_not_to_the_larger_one() {
		// Hand the same two the other way round and the fold reverses with
		// them. Which line the driver asked for first is the one whose numbers
		// are on the axis; nothing here ranks units by size.
		let plotted = plot(&[engine(), speed()], 0, 80).expect("a page");
		assert_eq!(plotted.axis_unit, "1/min");
		assert_eq!(plotted.y, (800.0, 6480.0));
		assert_eq!(plotted.lines[1].points, [(0.0, 800.0), (0.5, 3640.0), (1.0, 6480.0)]);
		assert_eq!(plotted.lines[1].folded_from, Some((0.0, 100.0)));
	}

	#[test]
	fn lines_in_one_unit_share_the_axis_and_none_of_them_is_folded() {
		// Two boost channels are the case the chart exists for: they are only
		// worth reading against each other, so they must not be rescaled apart.
		let plotted = plot(
			&[
				read("boost actual", "bar", &[(0.0, 1.0), (1.0, 2.0)]),
				read("boost specified", "bar", &[(0.0, 1.2), (1.0, 2.4)]),
			],
			0,
			80,
		)
		.expect("a page");
		assert_eq!(plotted.axis_unit, "bar");
		assert_eq!(plotted.y, (1.0, 2.4), "the axis spans both, not the first one");
		assert_eq!(plotted.lines[0].points, [(0.0, 1.0), (1.0, 2.0)]);
		assert_eq!(plotted.lines[1].points, [(0.0, 1.2), (1.0, 2.4)]);
		assert!(plotted.lines.iter().all(|line| line.folded_from.is_none()));
		// Colour is an index and the order is fixed, so a series keeps its
		// colour from one cycle to the next.
		assert_eq!(plotted.lines.iter().map(|line| line.colour).collect::<Vec<_>>(), [0, 1]);
	}

	#[test]
	fn a_third_unit_waits_for_the_next_page_rather_than_being_dropped() {
		// Two scales is the cap, and what does not fit is one `←`/`→` away.
		// That is not the same thing as a line there was no room to print, and
		// the plot must not report it as one.
		let three = [speed(), engine(), read("boost", "bar", &[(0.0, 1.0), (1.0, 2.0)])];
		let first = plot(&three, 0, 80).expect("a first page");
		assert_eq!(first.lines.len(), 2);
		assert_eq!(first.page, (0, 2));
		assert_eq!(first.dropped, 0);

		let second = plot(&three, 1, 80).expect("a second page");
		assert_eq!(second.axis_unit, "bar");
		assert_eq!(second.page, (1, 2));
		assert_eq!(second.lines[0].label, "boost");
	}

	#[test]
	fn a_key_that_will_not_fit_costs_lines_off_the_tail_and_the_count_is_kept() {
		// Degrading is allowed; degrading quietly is not. The key is what
		// decides, because a chart whose lines are not named is not a chart.
		let plotted = plot(&[speed(), engine()], 0, 24).expect("a page");
		assert_eq!(plotted.lines.len(), 1);
		assert_eq!(plotted.dropped, 1);
		assert_eq!(plotted.lines[0].label, "speed");
		// And the survivor owns the axis it is drawn against, rather than being
		// left folded onto a unit that is no longer on the page.
		assert_eq!(plotted.axis_unit, "km/h");
		assert_eq!(plotted.y, (0.0, 100.0));
		assert_eq!(plotted.lines[0].folded_from, None);

		let squeezed = plot(&[speed(), engine()], 1, 80).expect("a page");
		assert_eq!(squeezed.dropped, 0, "80 columns is room for both");

		// However narrow it gets, one line stays: a chart with nothing in it
		// says less than a chart with one thing in it.
		let sliver = plot(&[speed(), engine()], 0, 1).expect("a page");
		assert_eq!(sliver.lines.len(), 1);
	}

	#[test]
	fn a_flat_or_empty_series_still_has_bounds_and_does_not_read_as_a_dead_channel() {
		// A chart drawing nothing looks exactly like a car that stopped
		// answering, and on the road that is the wrong conclusion to invite.
		let flat = plot(&[read("speed", "km/h", &[(0.0, 50.0), (1.0, 50.0)])], 0, 80).expect("a page");
		assert_eq!(flat.y, (49.5, 50.5));

		// Every series looks like this on the first cycle of every run.
		let empty = plot(&[read("accel", "m/s²", &[])], 0, 80).expect("a page");
		assert_eq!(empty.y, (0.0, 1.0));
		assert_eq!(empty.x, (0.0, 1.0));
		assert!(empty.lines[0].points.is_empty());
	}

	#[test]
	fn nothing_read_yet_is_no_plot_at_all_and_a_page_past_the_end_comes_back() {
		// What an empty chart should say is a sentence about the car, so it is
		// the caller's and not the plot's.
		assert_eq!(plot(&[], 0, 80), None);

		// The page count grows as channels start answering, so an index that
		// has run past the end is clamped rather than refused — `←`/`→` must
		// not be able to strand the screen on a page that stopped existing.
		let plotted = plot(&[speed()], 7, 80).expect("a page");
		assert_eq!(plotted.page, (0, 1));
	}

	#[test]
	fn a_computed_line_is_marked_as_one_before_anything_is_drawn() {
		// The value table says so in a column of its own, and the chart must not
		// be the one place the distinction is dropped — so it is decided here,
		// where it can be asserted, rather than at the marker.
		let power = Series {
			origin: Origin::Computed("estimate"),
			..read("power", "kW", &[(0.0, 4.0), (1.0, 90.0)])
		};
		let plotted = plot(&[speed(), power], 0, 80).expect("a page");
		assert!(!plotted.lines[0].dotted);
		assert_eq!(plotted.lines[0].note, None);
		assert!(plotted.lines[1].dotted);
		assert_eq!(plotted.lines[1].note, Some("estimate"));
	}
}
