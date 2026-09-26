//! The stopwatch page's machine: arm at a standstill, start at the launch, stamp
//! the marks (`todo/dash/19`, `todo/dash/14` §6).
//!
//! The board's small brother of `vag-cli-measure`'s `Session`
//! (`crates/cli/vag-cli-measure/src/session.rs`), and ported from it rather
//! than re-derived, because the laptop's rules were each reached by discarding
//! something simpler:
//!
//! - **Standstill is the channel's zero, held.** The laptop decides it on the
//!   raw integer the unit answered; here it is the channel's value being exactly
//!   `0.0`, which is the same thing for a linear scaling with no offset — a
//!   speed channel with one is for the plan generator to refuse. Zero must hold for
//!   [`ARMING_HOLD_MS`] (`session::ARMING_HOLD_S`), so a crawling queue does not
//!   arm in every gap. A sample the unit did not answer moves no state.
//! - **The clock's origin is the launch, and the launch is reconstructed.** The
//!   first moving sample starts the run but is not `t = 0`: the car was already
//!   under way before its speed channel woke. [`Launch`] is
//!   `vag-cli-measure`'s `derive::start`, ported: a constant-jerk fit through
//!   `√v` over the first [`START_FIT_MS`] of movement reaches back too far, a
//!   straight line through the first two moving samples falls short, and the
//!   launch is the midpoint of the bracket they form. When the movement gives
//!   neither (fewer than three moving samples in the window — a poll too slow for
//!   the page), there is no launch and no time: a launch invented from two
//!   samples is not a measurement, as the laptop says.
//! - **A mark is a crossing, interpolated.** Each mark is timed where the speed
//!   first rises past it, linearly between the samples either side
//!   (`types::Track::crossing`), not at the nearer sample — at 20 Hz a whole
//!   sample is 50 ms.
//!
//! Speed is the channel's value times `km_h_per_unit`, a per-car factor measured
//! on the car and never written in; `0` means it has not been measured, and the
//! machine stays in [`Phase::NotMeasured`] and does nothing.
//!
//! Nothing here reads a clock: `now_ms` is a parameter, as everywhere in this
//! crate. `no_std`, allocation-free: the marks are the plan's slice, and the
//! launch fit is running sums, so it takes every sample of its window at any rate.

use crate::frame::Cell;

/// How long the channel's zero has to hold before the stopwatch arms.
/// `vag-cli-measure`'s `session::ARMING_HOLD_S`: a property of traffic, not of a car.
pub const ARMING_HOLD_MS: u64 = 1_000;

/// How much of the start of the movement the launch fit looks at.
/// `vag-cli-measure`'s `derive::START_FIT_S`.
pub const START_FIT_MS: u64 = 400;

/// The fewest moving samples the constant-jerk fit is made on.
/// `vag-cli-measure`'s `derive::MIN_FIT_SAMPLES`.
const MIN_FIT_SAMPLES: usize = 3;

/// The most marks one plan may carry: the page is one row of four cells, the phase and
/// the speed in the first, a mark's time in each of the others.
pub const MAX_MARKS: usize = 3;

/// Where the stopwatch stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
	/// `km_h_per_unit` is zero: the factor has not been measured, and nothing is timed.
	NotMeasured,
	/// Moving, or standing but not yet for [`ARMING_HOLD_MS`].
	Idle,
	/// Standing long enough: the next moving sample starts a run.
	Armed,
	Running,
	/// A run ended — at its highest mark, or back at a standstill before it. Its
	/// times are on show until the next run is armed and launched.
	Done,
}

/// When the car set off, in seconds relative to the first moving sample — so
/// never after `0.0`. `vag-cli-measure`'s `derive::Start`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Launch {
	/// The estimate: the midpoint of the bracket.
	pub t: f32,
	/// The constant-jerk fit, which reaches back too far.
	pub earliest: f32,
	/// The two-point line, which falls short.
	pub latest: f32,
}

/// One run: where each mark was crossed, and when the car set off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Run {
	/// Seconds after the first moving sample at which each mark was crossed, by
	/// the mark's place in the plan.
	crossed: [Option<f32>; MAX_MARKS],
	pub launch: Option<Launch>,
	/// The car came back to a standstill (or the page was left) before the highest mark.
	pub aborted: bool,
}

impl Run {
	const fn new() -> Self {
		Run {
			crossed: [None; MAX_MARKS],
			launch: None,
			aborted: false,
		}
	}

	/// Seconds after the first moving sample at which mark `index` was crossed.
	pub fn crossed_at(&self, index: usize) -> Option<f32> {
		self.crossed.get(index).copied().flatten()
	}

	/// The mark's time: from the launch to its crossing. `None` when it was not
	/// crossed, or when there is no launch to time it from.
	pub fn time(&self, index: usize) -> Option<f32> {
		Some(self.crossed_at(index)? - self.launch?.t)
	}
}

/// What one sample did worth saying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
	/// The standstill held: the next moving sample starts a run.
	Armed,
	/// The first moving sample after arming.
	Started,
	/// These marks — bit `i` for mark `i` — were crossed by this sample, and the
	/// run goes on.
	Crossed { marks: u8 },
	/// The highest mark was crossed (with any others crossed by the same sample).
	Finished,
	/// Back at a standstill before the highest mark; the marks that closed are kept.
	Aborted,
}

/// The stopwatch. `Copy`, so a caller holding it behind a lock can take a copy out to draw
/// from and let the lock go.
#[derive(Debug, Clone, Copy)]
pub struct Stopwatch<'a> {
	marks: &'a [u16],
	km_h_per_unit: f32,
	phase: Phase,
	/// When the current standstill began.
	standing_since: Option<u64>,
	/// The last answered sample: when, and the speed in km/h.
	previous: Option<(u64, f32)>,
	/// The first moving sample of the run in progress.
	started_ms: u64,
	/// The moving samples of the first [`START_FIT_MS`], as the launch fit needs them.
	fit: Fit,
	/// The run in progress, while [`Phase::Running`].
	current: Run,
	/// The last run that ended, finished or aborted: the one on show.
	last: Option<Run>,
	/// The last run that finished — what an aborted one gives way to when the page is left.
	finished: Option<Run>,
}

impl<'a> Stopwatch<'a> {
	/// `marks` in km/h, as the plan carries them; `km_h_per_unit` the measured
	/// factor, `0` where it has not been measured.
	///
	/// # Panics
	///
	/// With more than [`MAX_MARKS`] marks: the plan generator refuses such a
	/// plan, so this is an image built wrong.
	pub fn new(marks: &'a [u16], km_h_per_unit: f32) -> Self {
		assert!(marks.len() <= MAX_MARKS, "the plan carries at most MAX_MARKS marks");
		let measured = km_h_per_unit.is_finite() && km_h_per_unit > 0.0;
		Stopwatch {
			marks,
			km_h_per_unit,
			phase: if measured { Phase::Idle } else { Phase::NotMeasured },
			standing_since: None,
			previous: None,
			started_ms: 0,
			fit: Fit::new(),
			current: Run::new(),
			last: None,
			finished: None,
		}
	}

	pub fn phase(&self) -> Phase {
		self.phase
	}

	/// The marks, in km/h, in the plan's order.
	pub fn marks(&self) -> &'a [u16] {
		self.marks
	}

	/// The last run that reached its highest mark — the one the settings keep.
	pub fn finished(&self) -> Option<Run> {
		self.finished
	}

	/// The run on show: the one in progress, else the last one that ended.
	pub fn run(&self) -> Option<Run> {
		match self.phase {
			Phase::Running => Some(self.current),
			_ => self.last,
		}
	}

	/// One reading of the speed channel: its value in the channel's own unit,
	/// `None` where it did not answer or is stale.
	///
	/// At most one event comes out, the last thing the sample did: one that closes
	/// marks and the run says [`Event::Finished`], a launch sample that already
	/// crosses a mark says [`Event::Crossed`]. [`Stopwatch::run`] and
	/// [`Stopwatch::phase`] hold the whole of it.
	pub fn sample(&mut self, value: Option<f32>, now_ms: u64) -> Option<Event> {
		if self.phase == Phase::NotMeasured {
			return None;
		}
		// A cycle the unit did not answer moves no state (`session::on_sample`).
		let value = value.filter(|v| v.is_finite())?;
		let standing = value == 0.0;
		let kmh = value * self.km_h_per_unit;

		let mut event = None;
		match self.phase {
			Phase::NotMeasured => {}
			Phase::Idle | Phase::Done => match standing {
				true => {
					let since = *self.standing_since.get_or_insert(now_ms);
					if now_ms.saturating_sub(since) >= ARMING_HOLD_MS {
						self.phase = Phase::Armed;
						event = Some(Event::Armed);
					}
				}
				false => self.standing_since = None,
			},
			Phase::Armed if value > 0.0 => {
				self.launch(now_ms);
				event = Some(Event::Started);
			}
			// Going backwards is not a launch; the next standstill arms again.
			Phase::Armed if !standing => {
				self.phase = Phase::Idle;
				self.standing_since = None;
			}
			Phase::Armed => {}
			Phase::Running if standing => {
				// Back at a standstill short of the highest mark: what closed is
				// kept, and since the car is already stopped the hold starts here.
				self.end(true);
				self.standing_since = Some(now_ms);
				event = Some(Event::Aborted);
			}
			Phase::Running => {}
		}

		if self.phase == Phase::Running {
			event = self.advance(now_ms, kmh).or(event);
		}
		self.previous = Some((now_ms, kmh));
		event
	}

	/// The page was left: whatever was under way stops, and the next run needs a fresh
	/// standstill. A run in progress is dropped, and so is an aborted one on show: an
	/// aborted run is shown until the page is left and never after (owner's call,
	/// 2026-09-26). The last finished run stays — it is what the settings keep.
	pub fn reset(&mut self) {
		if self.phase == Phase::NotMeasured {
			return;
		}
		if self.last.is_some_and(|run| run.aborted) {
			self.last = self.finished;
		}
		self.phase = Phase::Idle;
		self.standing_since = None;
		self.previous = None;
	}

	/// The first moving sample after an armed standstill.
	fn launch(&mut self, now_ms: u64) {
		self.phase = Phase::Running;
		self.started_ms = now_ms;
		self.fit = Fit::new();
		self.current = Run::new();
	}

	/// Seconds from the first moving sample to `at_ms`, which may be before it.
	fn since_start(&self, at_ms: u64) -> f32 {
		match at_ms.checked_sub(self.started_ms) {
			Some(after) => after as f32 / 1000.0,
			None => -((self.started_ms - at_ms) as f32 / 1000.0),
		}
	}

	/// One running sample: the launch fit while its window is open, then every
	/// mark this sample crossed, and the end of the run at the highest.
	fn advance(&mut self, now_ms: u64, kmh: f32) -> Option<Event> {
		let t = self.since_start(now_ms);
		let after_ms = now_ms.saturating_sub(self.started_ms);
		if kmh > 0.0 && after_ms <= START_FIT_MS {
			// In `f64` from the integer milliseconds, as the sums are kept.
			self.fit.push(after_ms as f64 / 1000.0, f64::from(kmh));
			// Refitted with every sample in the window, as the laptop refits each
			// cycle; past the window the answer no longer changes.
			self.current.launch = self.fit.launch();
		}

		// `Track::crossing`: the first rise past the mark, between the samples
		// either side. The one before the first moving sample is the standstill's.
		let mut crossed = 0u8;
		if let Some((before_ms, before)) = self.previous {
			let t0 = self.since_start(before_ms);
			for (i, mark) in self.marks.iter().enumerate() {
				let target = f32::from(*mark);
				if self.current.crossed[i].is_none() && before < target && kmh >= target {
					self.current.crossed[i] = Some(t0 + (t - t0) * (target - before) / (kmh - before));
					crossed |= 1 << i;
				}
			}
		}

		let highest = (0..self.marks.len()).max_by_key(|&i| self.marks[i]);
		match highest {
			Some(i) if self.current.crossed[i].is_some() => {
				self.end(false);
				self.standing_since = None;
				Some(Event::Finished)
			}
			_ if crossed != 0 => Some(Event::Crossed { marks: crossed }),
			_ => None,
		}
	}

	/// The run ends; it becomes the one on show.
	fn end(&mut self, aborted: bool) {
		self.current.aborted = aborted;
		self.last = Some(self.current);
		if !aborted {
			self.finished = Some(self.current);
		}
		self.phase = Phase::Done;
	}
}

/// The launch fit's window as running sums: every moving sample of the first
/// [`START_FIT_MS`], whatever the rate, in constant space. Times are seconds after
/// the first moving sample; sums are `f64`, so a long window loses nothing to the
/// subtraction that turns them into deviations.
#[derive(Debug, Clone, Copy)]
struct Fit {
	/// Samples taken, and Σt, Σ√v, Σt², Σt·√v over them.
	n: u32,
	st: f64,
	sy: f64,
	stt: f64,
	sty: f64,
	/// The first two samples, for the two-point line: `(t, km/h)`.
	first: Option<(f64, f64)>,
	second: Option<(f64, f64)>,
}

impl Fit {
	const fn new() -> Self {
		Fit {
			n: 0,
			st: 0.0,
			sy: 0.0,
			stt: 0.0,
			sty: 0.0,
			first: None,
			second: None,
		}
	}

	/// One moving sample, `kmh > 0`.
	fn push(&mut self, t: f64, kmh: f64) {
		let y = sqrt64(kmh);
		self.n += 1;
		self.st += t;
		self.sy += y;
		self.stt += t * t;
		self.sty += t * y;
		match (self.first, self.second) {
			(None, _) => self.first = Some((t, kmh)),
			(Some(_), None) => self.second = Some((t, kmh)),
			_ => {}
		}
	}

	/// `vag-cli-measure`'s `derive::start` over the window.
	///
	/// `latest` is the line through the first two samples, clamped at the first:
	/// a launch is convex, so the line runs under it and reaches zero late.
	/// `earliest` is the constant-jerk root, `v = ½j(t − t₀)²` fitted as a straight
	/// line through `√v`, also clamped at the first sample: it reaches back too far.
	/// The two are ordered, never collapsed. `None` without a second sample or
	/// without a fit, as there.
	fn launch(&self) -> Option<Launch> {
		let ((t_first, v_first), (t_second, v_second)) = (self.first?, self.second?);
		let (rise, step) = (v_second - v_first, t_second - t_first);
		let line = match rise > 0.0 && step > 0.0 {
			true => (t_first - v_first * step / rise).min(t_first),
			false => t_first,
		};
		let quadratic = self.constant_jerk_launch()?.min(t_first);
		let (earliest, latest) = (quadratic.min(line), quadratic.max(line));
		Some(Launch {
			t: (0.5 * (earliest + latest)) as f32,
			earliest: earliest as f32,
			latest: latest as f32,
		})
	}

	/// `derive::constant_jerk_launch`: least squares of `√v` on `t`, extrapolated to
	/// `√v = 0`, from the sums — `Sxx = Σt² − n·t̄²`, `Sxy = Σt√v − n·t̄·ȳ`. `None`
	/// under [`MIN_FIT_SAMPLES`] or with no gain across the window.
	fn constant_jerk_launch(&self) -> Option<f64> {
		if (self.n as usize) < MIN_FIT_SAMPLES {
			return None;
		}
		let count = f64::from(self.n);
		let (x_bar, y_bar) = (self.st / count, self.sy / count);
		let sxx = self.stt - count * x_bar * x_bar;
		let sxy = self.sty - count * x_bar * y_bar;
		if sxx <= 0.0 {
			return None;
		}
		let gradient = sxy / sxx;
		if gradient <= 0.0 {
			return None;
		}
		Some(x_bar - y_bar / gradient)
	}
}

/// [`sqrt`] carried to `f64` by two more Newton steps.
fn sqrt64(x: f64) -> f64 {
	let mut y = f64::from(sqrt(x as f32));
	if y <= 0.0 {
		return 0.0;
	}
	for _ in 0..2 {
		y = 0.5 * (y + x / y);
	}
	y
}
/// What the page says, in the plan's language: `"ru"` or anything else, which is English.
/// The panel's own words, not the car's — nothing here names a unit or a state of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Words {
	/// [`Phase::NotMeasured`]: the factor has not been measured.
	pub not_measured: &'static str,
	/// [`Phase::Idle`]: stand still to arm it.
	pub idle: &'static str,
	pub armed: &'static str,
	pub running: &'static str,
	pub done: &'static str,
	pub seconds: &'static str,
	pub km_h: &'static str,
}

impl Words {
	pub fn of(language: &str) -> Words {
		match language {
			"ru" => RUSSIAN,
			_ => ENGLISH,
		}
	}
}

const ENGLISH: Words = Words {
	not_measured: "NO FACTOR",
	idle: "STOP",
	armed: "GO",
	running: "RUN",
	done: "DONE",
	seconds: "s",
	km_h: "km/h",
};

const RUSSIAN: Words = Words {
	not_measured: "НЕТ КОЭФ",
	idle: "СТОП",
	armed: "ПУСК",
	running: "ЗАМЕР",
	done: "ГОТОВО",
	// The units' face has no Cyrillic, and a plan's units are the catalog's SI spellings in
	// either language: the labels are Russian, the units are not.
	seconds: "s",
	km_h: "km/h",
};

/// A mark's label, `0-100`: made once from the plan's marks and borrowed by every frame's
/// cells, since a cell holds a `&str` and this crate has no allocator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Labels {
	text: [[u8; 8]; MAX_MARKS],
	len: [u8; MAX_MARKS],
	count: usize,
}

impl Labels {
	pub fn new(marks: &[u16]) -> Labels {
		let mut labels = Labels {
			text: [[0; 8]; MAX_MARKS],
			len: [0; MAX_MARKS],
			count: marks.len().min(MAX_MARKS),
		};
		for (i, mark) in marks.iter().take(MAX_MARKS).enumerate() {
			// `0-` and at most five digits: seven bytes of eight.
			let text = &mut labels.text[i];
			text[..2].copy_from_slice(b"0-");
			let mut digits = [0u8; 5];
			let mut n = *mark;
			let mut count = 0;
			loop {
				digits[count] = b'0' + (n % 10) as u8;
				count += 1;
				n /= 10;
				if n == 0 {
					break;
				}
			}
			for (at, digit) in digits[..count].iter().rev().enumerate() {
				text[2 + at] = *digit;
			}
			labels.len[i] = (2 + count) as u8;
		}
		labels
	}

	/// The label of mark `index`; empty past the marks.
	pub fn get(&self, index: usize) -> &str {
		if index >= self.count {
			return "";
		}
		// ASCII by construction.
		core::str::from_utf8(&self.text[index][..usize::from(self.len[index])]).unwrap_or("")
	}
}

/// The stopwatch page as a values row: the phase's word over the speed, then each mark's
/// time — the run on show, or where there is none, the last finished run `saved` holds as
/// `(mark in km/h, seconds)`. A mark with no time draws a dash. With the factor not
/// measured the row is that word and nothing else.
pub fn cells<'a>(
	watch: &Stopwatch<'_>,
	speed_km_h: Option<f32>,
	saved: &[(u16, f32)],
	words: &'a Words,
	labels: &'a Labels,
) -> ([Cell<'a>; 1 + MAX_MARKS], usize) {
	let mut row: [Cell<'a>; 1 + MAX_MARKS] = core::array::from_fn(|_| Cell::new("", None, "", 0));
	let word = match watch.phase() {
		Phase::NotMeasured => {
			row[0] = Cell::new(words.not_measured, None, "", 0);
			return (row, 1);
		}
		Phase::Idle => words.idle,
		Phase::Armed => words.armed,
		Phase::Running => words.running,
		Phase::Done => words.done,
	};
	row[0] = Cell::new(word, speed_km_h, words.km_h, 0);
	let run = watch.run();
	let marks = &watch.marks()[..watch.marks().len().min(MAX_MARKS)];
	for (i, mark) in marks.iter().enumerate() {
		let time = match run {
			Some(run) => run.time(i),
			None => saved.iter().find(|(saved, _)| saved == mark).map(|(_, seconds)| *seconds),
		};
		row[1 + i] = Cell::new(labels.get(i), time, words.seconds, time.map_or(2, decimals_for));
	}
	(row, 1 + marks.len())
}

/// Places after the point a time is drawn with: two under 10 s, one under 100, none past
/// it — what a quarter of the panel holds in the large face (measured with the renderer,
/// `the_page_fits_the_panel_in_both_languages_with_its_widest_numbers`).
fn decimals_for(seconds: f32) -> u8 {
	match seconds {
		s if s < 10.0 => 2,
		s if s < 100.0 => 1,
		_ => 0,
	}
}

/// `f32::sqrt` is in `std`, and this crate is `no_std`: Newton's method from the
/// exponent-halving first guess, four steps to full `f32` precision. Zero and
/// below are zero — the fit only ever takes a speed above zero.
fn sqrt(x: f32) -> f32 {
	if x <= 0.0 || !x.is_finite() {
		return 0.0;
	}
	let mut y = f32::from_bits((x.to_bits() >> 1) + 0x1FBD_1DF5);
	for _ in 0..4 {
		y = 0.5 * (y + x / y);
	}
	y
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::vec::Vec;

	/// A neutral factor: the channel counts in units of half a km/h.
	const FACTOR: f32 = 0.5;
	static MARKS: [u16; 2] = [60, 100];

	/// Feed a speed profile in km/h, sampled every `step_ms` from `from_ms` to
	/// `to_ms` inclusive, as the channel's value. What came out, with when.
	fn drive(watch: &mut Stopwatch<'_>, kmh: impl Fn(f64) -> f64, from_ms: u64, to_ms: u64, step_ms: u64) -> Vec<(u64, Event)> {
		let mut out = Vec::new();
		let mut t = from_ms;
		while t <= to_ms {
			let value = (kmh(t as f64 / 1000.0) / f64::from(FACTOR)) as f32;
			if let Some(event) = watch.sample(Some(value), t) {
				out.push((t, event));
			}
			t += step_ms;
		}
		out
	}

	/// Standing until `t0` s, then a constant acceleration of `a` km/h a second.
	fn ramp(t0: f64, a: f64) -> impl Fn(f64) -> f64 {
		move |t| if t <= t0 { 0.0 } else { a * (t - t0) }
	}

	/// Standing until `t0` s, then `v = ½·j·(t − t0)²` — the constant-jerk
	/// model the launch fit assumes, so its root is exact.
	fn jerk(t0: f64, j: f64) -> impl Fn(f64) -> f64 {
		move |t| if t <= t0 { 0.0 } else { 0.5 * j * (t - t0) * (t - t0) }
	}

	fn close(a: f32, b: f64, what: &str) {
		assert!((f64::from(a) - b).abs() < 1e-4, "{what}: {a} is not {b}");
	}

	#[test]
	fn an_unmeasured_factor_times_nothing() {
		for factor in [0.0, -1.0, f32::NAN] {
			let mut watch = Stopwatch::new(&MARKS, factor);
			assert_eq!(watch.phase(), Phase::NotMeasured);
			assert!(drive(&mut watch, ramp(2.0, 10.0), 0, 20_000, 50).is_empty());
			assert_eq!((watch.phase(), watch.run()), (Phase::NotMeasured, None));
		}
	}

	#[test]
	fn a_standstill_arms_once_it_has_held_a_second() {
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		assert_eq!(watch.phase(), Phase::Idle);
		assert_eq!(watch.sample(Some(0.0), 0), None);
		assert_eq!(watch.sample(Some(0.0), ARMING_HOLD_MS - 1), None, "not yet");
		assert_eq!(watch.phase(), Phase::Idle);
		assert_eq!(watch.sample(Some(0.0), ARMING_HOLD_MS), Some(Event::Armed));
		assert_eq!(watch.phase(), Phase::Armed);
	}

	#[test]
	fn creeping_starts_the_hold_again_and_a_missing_answer_does_not() {
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		watch.sample(Some(0.0), 0);
		watch.sample(Some(4.0), 600);
		assert_eq!(watch.sample(Some(0.0), 700), None);
		assert_eq!(watch.sample(Some(0.0), 1_600), None, "the hold began at 700");
		assert_eq!(watch.sample(None, 1_650), None);
		assert_eq!(watch.phase(), Phase::Idle, "no answer is no evidence either way");
		assert_eq!(watch.sample(Some(0.0), 1_700), Some(Event::Armed));
		// Armed, a missing answer is not a launch.
		assert_eq!(watch.sample(None, 1_800), None);
		assert_eq!(watch.phase(), Phase::Armed);
	}

	#[test]
	fn moving_without_arming_starts_nothing() {
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		let events = drive(&mut watch, ramp(0.5, 10.0), 0, 12_000, 50);
		assert!(events.is_empty(), "never stood still for a second: {events:?}");
		assert_eq!(watch.run(), None);
	}

	#[test]
	fn the_first_moving_sample_after_arming_starts_the_run() {
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		let events = drive(&mut watch, ramp(1.47, 10.0), 0, 1_550, 50);
		assert_eq!(events, [(1_000, Event::Armed), (1_500, Event::Started)]);
		assert_eq!(watch.phase(), Phase::Running);
	}

	#[test]
	fn the_launch_is_the_midpoint_of_the_two_estimators() {
		// On the constant-jerk model the fit through √v is exact: 1.05 s. The line
		// through the first two moving samples (1.1 s: 0.025, 1.2 s: 0.225 km/h)
		// reaches zero at 1.1 − 0.025 · 0.1 / 0.2 = 1.0875 s. The launch is between.
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		drive(&mut watch, jerk(1.05, 20.0), 0, 2_000, 100);
		let launch = watch.run().and_then(|run| run.launch).expect("five samples in the window");
		// Seconds relative to the first moving sample, at 1.1 s.
		close(launch.earliest, 1.05 - 1.1, "the constant-jerk root");
		close(launch.latest, 1.0875 - 1.1, "the two-point line");
		close(launch.t, (1.05 + 1.0875) / 2.0 - 1.1, "the midpoint");
	}

	#[test]
	fn a_mark_is_stamped_between_the_samples_either_side() {
		// 10 km/h a second from 1.07 s: 60 km/h at 7.07 s, between the samples at
		// 7.0 and 7.1; 100 at 11.07. A ramp puts the line's root on the launch.
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		let events = drive(&mut watch, ramp(1.07, 10.0), 0, 12_000, 100);
		assert_eq!(
			events,
			[
				(1_000, Event::Armed),
				(1_100, Event::Started),
				(7_100, Event::Crossed { marks: 0b01 }),
				(11_100, Event::Finished)
			]
		);
		let run = watch.run().expect("kept");
		// Crossings relative to the first moving sample, at 1.1 s.
		close(run.crossed_at(0).unwrap(), 5.97, "60 km/h");
		close(run.crossed_at(1).unwrap(), 9.97, "100 km/h");
		let launch = run.launch.unwrap();
		close(launch.latest, -0.03, "on a ramp the line is exact");
		assert!(launch.earliest < launch.latest);
		close(run.time(0).unwrap(), 5.97 - f64::from(launch.t), "0-60 from the launch");
		close(run.time(1).unwrap(), 9.97 - f64::from(launch.t), "0-100 from the launch");
		assert!(!run.aborted);
	}

	#[test]
	fn a_standstill_shorter_than_a_second_never_arms() {
		// The hold counts from the first zero: samples of zero from 0 to 0.9 s are
		// 0.9 s of standing, and the car moved before the second was up.
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		assert!(drive(&mut watch, ramp(0.9, 10.0), 0, 2_000, 100).is_empty());
	}

	#[test]
	fn the_highest_mark_ends_the_run_and_the_next_standstill_arms_the_next() {
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		drive(&mut watch, ramp(1.0, 20.0), 0, 7_000, 100);
		assert_eq!(watch.phase(), Phase::Done);
		let first = watch.run().unwrap();
		assert!(first.time(1).is_some());
		// Still moving: done, and the times stay up.
		assert_eq!(watch.sample(Some(300.0), 7_100), None);
		assert_eq!(watch.phase(), Phase::Done);
		// Stopped for a second: armed, with the last run still on show.
		assert_eq!(watch.sample(Some(0.0), 20_000), None);
		assert_eq!(watch.sample(Some(0.0), 21_000), Some(Event::Armed));
		assert_eq!(watch.run(), Some(first));
		// The next launch is a run of its own.
		assert_eq!(watch.sample(Some(2.0), 21_100), Some(Event::Started));
		assert_eq!(watch.run().unwrap().crossed_at(0), None);
	}

	#[test]
	fn a_standstill_before_the_highest_mark_aborts_and_keeps_what_closed() {
		// Up to 80 km/h and back down to a stop.
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		let up_and_down = |t: f64| match t {
			t if t <= 1.0 => 0.0,
			t if t <= 5.0 => 20.0 * (t - 1.0),
			t if t <= 9.0 => 80.0 - 20.0 * (t - 5.0),
			_ => 0.0,
		};
		let events = drive(&mut watch, up_and_down, 0, 9_500, 100);
		assert_eq!(events.last(), Some(&(9_000, Event::Aborted)));
		assert_eq!(watch.phase(), Phase::Done);
		let run = watch.run().unwrap();
		assert!(run.aborted);
		assert!(run.time(0).is_some(), "0-60 closed on the way up and is kept");
		assert_eq!(run.time(1), None);
		// The standstill that aborted it is the start of the next hold.
		assert_eq!(watch.sample(Some(0.0), 10_000), Some(Event::Armed));
	}

	#[test]
	fn two_marks_crossed_by_one_sample_are_said_together() {
		static CLOSE: [u16; 3] = [40, 30, 100];
		let mut watch = Stopwatch::new(&CLOSE, FACTOR);
		// 1.0 s standing, then a jump from 25 to 45 km/h between two samples.
		watch.sample(Some(0.0), 0);
		watch.sample(Some(0.0), 1_000);
		watch.sample(Some(10.0 / FACTOR), 1_100);
		watch.sample(Some(25.0 / FACTOR), 1_200);
		assert_eq!(watch.sample(Some(25.0 / FACTOR), 1_300), None);
		assert_eq!(watch.sample(Some(45.0 / FACTOR), 1_400), Some(Event::Crossed { marks: 0b011 }));
		let run = watch.run().unwrap();
		// Relative to 1.1 s: 30 at 0.2 + 0.1·5/20, 40 at 0.2 + 0.1·15/20.
		close(run.crossed_at(1).unwrap(), 0.225, "30 km/h");
		close(run.crossed_at(0).unwrap(), 0.275, "40 km/h");
	}

	#[test]
	fn a_poll_too_slow_to_fit_the_launch_times_nothing() {
		// At 4 Hz the first 0.4 s of movement holds two samples: no launch, and a
		// time with no launch is not a time. The crossings are still where they were.
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		let events = drive(&mut watch, ramp(1.1, 20.0), 0, 8_000, 250);
		assert_eq!(events.last().map(|e| e.1), Some(Event::Finished));
		let run = watch.run().unwrap();
		assert_eq!(run.launch, None);
		assert!(run.crossed_at(0).is_some() && run.crossed_at(1).is_some());
		assert_eq!((run.time(0), run.time(1)), (None, None));
	}

	#[test]
	fn leaving_the_page_drops_a_run_that_did_not_finish_keeps_one_that_did_and_needs_a_fresh_standstill() {
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		drive(&mut watch, ramp(1.0, 20.0), 0, 7_000, 100);
		let finished = watch.run().expect("a finished run");
		assert!(!finished.aborted);
		drive(&mut watch, |t| if t < 12.0 { 0.0 } else { 20.0 * (t - 12.0) }, 10_000, 14_500, 100);
		assert_eq!(watch.phase(), Phase::Running);
		watch.reset();
		assert_eq!(watch.phase(), Phase::Idle);
		assert_eq!(watch.run(), Some(finished), "the run in progress is dropped, the finished one stays");
		assert_eq!(watch.finished(), Some(finished));
		// An aborted run on show is dropped the same way.
		drive(&mut watch, |t| if t < 20.0 { 0.0 } else { 20.0 * (t - 20.0) }, 18_000, 22_000, 100);
		watch.sample(Some(0.0), 22_100);
		assert!(watch.run().is_some_and(|run| run.aborted));
		watch.reset();
		assert_eq!(watch.run(), Some(finished));
		// Standing at the moment of the reset is not a standstill already held.
		assert_eq!(watch.sample(Some(0.0), 5_000), None);
		assert_eq!(watch.sample(Some(0.0), 6_000), Some(Event::Armed));
		// And an unmeasured stopwatch stays unmeasured.
		let mut unmeasured = Stopwatch::new(&MARKS, 0.0);
		unmeasured.reset();
		assert_eq!(unmeasured.phase(), Phase::NotMeasured);
	}

	#[test]
	fn reversing_while_armed_is_not_a_launch() {
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		watch.sample(Some(0.0), 0);
		watch.sample(Some(0.0), 1_000);
		assert_eq!(watch.sample(Some(-3.0), 1_100), None);
		assert_eq!(watch.phase(), Phase::Idle);
	}

	/// `vag-cli-measure`'s `derive::start` and `constant_jerk_launch`, transcribed
	/// in `f64` over a whole track, as the reference the board's online fit must
	/// agree with. `track` is `(seconds, km/h)`, standstill included. The answer is
	/// relative to the first moving sample, as the board keeps it.
	fn reference_start(track: &[(f64, f64)]) -> Option<(f64, f64, f64)> {
		let first = track.iter().position(|s| s.1 > 0.0)?;
		let second = (first + 1..track.len()).find(|&i| track[i].1 > 0.0)?;
		let (t_first, v_first) = track[first];
		let (rise, step) = (track[second].1 - v_first, track[second].0 - t_first);
		let latest = if rise > 0.0 && step > 0.0 {
			(t_first - v_first * step / rise).min(t_first)
		} else {
			t_first
		};
		let window: Vec<(f64, f64)> = track[first..]
			.iter()
			.filter(|s| s.0 <= t_first + 0.4 && s.1 > 0.0)
			.map(|s| (s.0, s.1.sqrt()))
			.collect();
		if window.len() < 3 {
			return None;
		}
		let n = window.len() as f64;
		let x_bar = window.iter().map(|s| s.0).sum::<f64>() / n;
		let y_bar = window.iter().map(|s| s.1).sum::<f64>() / n;
		let sxx: f64 = window.iter().map(|s| (s.0 - x_bar) * (s.0 - x_bar)).sum();
		let sxy: f64 = window.iter().map(|s| (s.0 - x_bar) * (s.1 - y_bar)).sum();
		if sxx <= 0.0 || sxy / sxx <= 0.0 {
			return None;
		}
		let quadratic = (x_bar - y_bar / (sxy / sxx)).min(t_first);
		let (earliest, latest) = (quadratic.min(latest), quadratic.max(latest));
		Some((0.5 * (earliest + latest) - t_first, earliest - t_first, latest - t_first))
	}

	#[test]
	fn the_launch_fit_takes_the_whole_window_at_any_rate_and_agrees_with_the_laptop() {
		// 20, 100 and 250 Hz: at 250 the first 0.4 s of movement is 101 samples.
		// A ramp is not the fit's model, so every sample in the window moves the
		// answer: a fit that stopped short of the window would disagree.
		let ramp = ramp(1.013, 12.0);
		let jerk = jerk(1.013, 20.0);
		let profiles: [(&str, &dyn Fn(f64) -> f64); 2] = [("ramp", &ramp), ("jerk", &jerk)];
		for step_ms in [50u64, 10, 4] {
			for (name, kmh) in profiles {
				let mut watch = Stopwatch::new(&MARKS, FACTOR);
				drive(&mut watch, kmh, 0, 2_500, step_ms);
				let got = watch.run().and_then(|run| run.launch).expect("a launch");
				let track: Vec<(f64, f64)> = (0..=2_500 / step_ms)
					.map(|i| {
						let t = (i * step_ms) as f64 / 1000.0;
						(t, kmh(t))
					})
					.collect();
				let (t, earliest, latest) = reference_start(&track).expect("the reference fits too");
				let what = std::format!("{name} at {} Hz", 1000 / step_ms);
				// To a microsecond: the board keeps its answer in `f32`, nothing more.
				for (got, want) in [(got.t, t), (got.earliest, earliest), (got.latest, latest)] {
					assert!((f64::from(got) - want).abs() < 1e-6, "{what}: {got} is not {want}");
				}
			}
		}
	}

	#[test]
	fn a_marks_label_is_its_speeds() {
		static WIDE: [u16; 3] = [60, 100, 65535];
		let labels = Labels::new(&WIDE);
		assert_eq!((labels.get(0), labels.get(1), labels.get(2)), ("0-60", "0-100", "0-65535"));
		assert_eq!(labels.get(3), "", "past the marks");
	}

	type Row = Vec<(std::string::String, Option<f32>, std::string::String, u8)>;

	fn row(watch: &Stopwatch<'_>, speed: Option<f32>, saved: &[(u16, f32)]) -> Row {
		let words = Words::of("en");
		let labels = Labels::new(&MARKS);
		let (cells, count) = cells(watch, speed, saved, &words, &labels);
		cells[..count]
			.iter()
			.map(|c| (c.label.into(), c.value, c.unit.into(), c.decimals))
			.collect()
	}

	#[test]
	fn the_page_says_the_phase_over_the_speed_and_each_marks_time() {
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		// Nothing run yet, a finished run in the settings: its times, under the phase.
		let saved = [(100, 9.87), (60, 5.43)];
		assert_eq!(
			row(&watch, Some(12.0), &saved),
			[
				("STOP".into(), Some(12.0), "km/h".into(), 0),
				("0-60".into(), Some(5.43), "s".into(), 2),
				("0-100".into(), Some(9.87), "s".into(), 2)
			]
		);
		watch.sample(Some(0.0), 0);
		watch.sample(Some(0.0), 1_000);
		assert_eq!(row(&watch, Some(0.0), &saved)[0].0, "GO");
		// Running: this run's times, a dash where a mark has not closed; the saved ones are gone.
		drive(&mut watch, ramp(1.05, 20.0), 1_100, 4_500, 100);
		let running = row(&watch, None, &saved);
		assert_eq!(running[0], ("RUN".into(), None, "km/h".into(), 0), "a stale speed is a dash");
		assert!(running[1].1.is_some() && running[2].1.is_none(), "{running:?}");
		// Done: the run's own.
		drive(&mut watch, ramp(1.05, 20.0), 4_600, 7_000, 100);
		assert_eq!(row(&watch, Some(120.0), &saved)[0].0, "DONE");
		assert_eq!(row(&watch, Some(120.0), &saved)[2].1, watch.run().unwrap().time(1));
	}

	#[test]
	fn an_aborted_run_is_shown_until_the_next_and_an_unmeasured_factor_says_only_so() {
		let mut watch = Stopwatch::new(&MARKS, FACTOR);
		drive(&mut watch, ramp(1.05, 20.0), 0, 4_500, 100);
		watch.sample(Some(0.0), 4_600);
		let shown = row(&watch, Some(0.0), &[(60, 1.0), (100, 2.0)]);
		assert_eq!(shown.len(), 3);
		assert_eq!(shown[1].1, watch.run().unwrap().time(0), "the aborted run's 0-60, not the saved one");
		assert_eq!(shown[2].1, None, "and its 0-100 never closed");

		let unmeasured = Stopwatch::new(&MARKS, 0.0);
		assert_eq!(row(&unmeasured, Some(50.0), &[(60, 1.0)]), [("NO FACTOR".into(), None, "".into(), 0)]);
	}

	#[test]
	fn the_page_fits_the_panel_in_both_languages_with_its_widest_numbers() {
		use crate::{Frame, PANEL, Theme, draw};
		use embedded_graphics::pixelcolor::BinaryColor;
		use embedded_graphics_simulator::SimulatorDisplay;
		static WIDEST: [u16; 3] = [100, 200, 300];
		let labels = Labels::new(&WIDEST);
		let watch = Stopwatch::new(&WIDEST, FACTOR);
		for language in ["en", "ru"] {
			let words = Words::of(language);
			for word in [words.idle, words.armed, words.running, words.done] {
				let saved = [(100, 9.99), (200, 88.88), (300, 188.8)];
				let (mut row, count) = cells(&watch, Some(288.0), &saved, &words, &labels);
				row[0].label = word;
				let mut panel = SimulatorDisplay::<BinaryColor>::new(PANEL);
				let report = draw(&Frame::Values { cells: &row[..count] }, &Theme::bold_mono(), &mut panel);
				assert!(
					!report.label_overrun && !report.value_overrun && !report.glyph_missing,
					"{language} {word}: {report:?}"
				);
			}
			let unmeasured = Stopwatch::new(&WIDEST, 0.0);
			let (row, count) = cells(&unmeasured, None, &[], &words, &labels);
			let mut panel = SimulatorDisplay::<BinaryColor>::new(PANEL);
			let report = draw(&Frame::Values { cells: &row[..count] }, &Theme::bold_mono(), &mut panel);
			assert!(!report.label_overrun && !report.glyph_missing, "{language}: {report:?}");
		}
	}

	#[test]
	fn the_words_follow_the_plans_language() {
		assert_eq!(Words::of("ru").idle, "СТОП");
		assert_eq!(Words::of("en").idle, "STOP");
		assert_eq!(Words::of("de"), Words::of("en"), "a language it has no words for is English");
		for words in [Words::of("ru"), Words::of("en")] {
			assert!(words.not_measured.chars().count() <= 10, "a label is ten characters at most");
		}
	}

	#[test]
	fn the_square_root_the_fit_needs_is_close_enough() {
		for x in [1e-6f32, 0.025, 0.5, 1.0, 2.0, 60.0, 250.0, 1e6] {
			let got = f64::from(sqrt(x));
			let want = f64::from(x).sqrt();
			assert!((got - want).abs() <= want * 1e-6, "√{x}: {got} vs {want}");
		}
		assert_eq!(sqrt(0.0), 0.0);
		for x in [1e-6f64, 0.025, 0.5, 2.0, 60.0, 250.0] {
			let (got, want) = (sqrt64(x), x.sqrt());
			assert!((got - want).abs() <= want * 1e-14, "√{x} in f64: {got} vs {want}");
		}
		assert_eq!(sqrt64(0.0), 0.0);
	}
}
