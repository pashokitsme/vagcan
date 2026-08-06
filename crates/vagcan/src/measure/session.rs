//! The stopwatch: what turns a stream of samples into runs.
//!
//! Pure state and assembly. No adapter, no terminal, no catalog, and no
//! `Instant::now()` — time and readings arrive as parameters, which is what
//! makes a launch, an abort and a collapsing poll rate all testable against a
//! profile written by hand.
//!
//! Three rules shape everything below, and each of them was reached by
//! discarding something that looked simpler.
//!
//! **Arming is decided on the raw integer the unit answered.** `--speed-scale`
//! multiplies at ingest, before mark detection, so that `0-100` means a
//! corrected 100 rather than an indicated one — and that makes `v == 0.0` on the
//! scaled float a comparison this project would regret. The raw value carries no
//! correction and no rounding, so a car is standing still exactly when its speed
//! channel says the integer zero. Zero must then *hold* for
//! [`ARMING_HOLD_S`]: the hold is what stops a crawling stop-and-go from arming
//! the trigger in every gap between cars.
//!
//! **The run's clock is the launch, and the launch is an interval.** The first
//! moving sample starts the run, but it is not `t = 0`: a wheel-speed signal has
//! a low-speed dead band, so the car is already under way before its own speed
//! channel wakes up. [`derive::start`] reaches back through that with two
//! estimators that miss from opposite sides, and the run is rebased onto the
//! midpoint of the bracket they form. Everything before it keeps a negative
//! timestamp, which is what the ring buffer is for.
//!
//! **The run keeps raw samples; the report recomputes.** A [`Run`] is the
//! file's contents — the channels as they were read, on the run's own clock —
//! plus the marks that closed and whether it was thrown away. The design's
//! storage rule is that every derivative is recomputed in one pass over the
//! finished run, which is what lets a method be corrected without re-driving the
//! car. The marks kept here are the ones the stopwatch closed on, so that the
//! screen and the file agree about what happened; they are not a cache the
//! report is obliged to believe.

use std::collections::BTreeMap;

use super::derive::{self, Start};
use super::power::KMH_PER_MS;
use super::types::{Seconds, States, Track};

/// How long the raw zero has to hold before the trigger arms.
///
/// A property of traffic rather than of any car: without a hold, a stop-and-go
/// queue arms the trigger in every gap and the first person to lift their foot
/// records a "run" from walking pace.
pub const ARMING_HOLD_S: Seconds = 1.0;

/// How much more than it will hand out the ring keeps.
///
/// The run is entitled to the three seconds before **`t0`**, and `t0` is
/// reconstructed backwards from the first moving sample — so it lies further
/// back than anything that was known while the car was still stopped. This
/// margin is wider than any bracket [`derive::start`] produces, and the surplus
/// is trimmed off when the run is closed rather than stored.
const RING_MARGIN_S: Seconds = 1.0;

/// How many cycles the cadence is judged over, at each end.
///
/// A median over several intervals rather than one interval, because a single
/// long cycle is not a collapsed rate: a unit answering "response pending" can
/// legally stall for seconds without missing a single answer, and flagging that
/// would put `SLOW` on the screen of a car doing nothing wrong. Ten intervals is
/// about half a second at the rates this loop achieves — long enough to need a
/// sustained collapse, short enough that the baseline is settled before anybody
/// has come to a stop.
const CADENCE_CYCLES: usize = 10;

/// How far the cycle may lengthen against the session's own established cadence
/// before the run is flagged `degraded`.
///
/// Relative, because there is no absolute rate this tool is entitled to expect:
/// what a car answers at is a property of that car, its units and the adapter,
/// and a hertz figure written here would be one bench's number applied to every
/// other. Halving the rate doubles what the sampling contributes to a
/// launch-based mark, which is the point at which the times stop deserving the
/// confidence they are printed with.
pub const DEGRADED_CYCLE_FACTOR: f64 = 2.0;

/// One poll cycle's worth of readings, already converted through each channel's
/// own definition.
///
/// Every field carries its own timestamp because no two channels share a clock:
/// identifiers are polled in batches, the batches are separated in time, and one
/// shared timestamp has already cost this project a wrong proof.
///
/// The four named channels are exactly the roles
/// [`super::channels`] marks required — a run with no speed has no stopwatch,
/// and a run with no engine speed, gear or pedal explains nothing about the time
/// it did measure. Everything optional arrives in [`SampleSet::others`] or
/// [`SampleSet::states`] under the key its role was resolved as.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SampleSet {
	/// The leading speed, in m/s, and the raw integer it came from.
	///
	/// Both, because they answer different questions. The float is what marks
	/// are timed against, after [`Session`] applies the speed scale to it; the
	/// integer is what standstill is decided on, and no correction and no
	/// rounding stands between it and what the unit said.
	pub speed: Option<(Seconds, f64, u32)>,
	pub engine_speed: Option<(Seconds, f64)>,
	/// The catalog's own label for the engaged gear, never the code behind it.
	pub gear: Option<(Seconds, String)>,
	pub pedal: Option<(Seconds, f64)>,
	/// Any other quantity the poll loop read this cycle, keyed by role.
	pub others: Vec<(&'static str, Seconds, f64)>,
	/// Any other discrete channel — the selector lever — as labels.
	pub states: Vec<(&'static str, Seconds, String)>,
}

/// Every channel of a run, columnar, each with its own timestamps.
///
/// This is what the session file holds and what the chart page wants: a channel
/// is its own values against its own times, and a layout that implied otherwise
/// would be inventing a clock the car never had.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Samples {
	/// In m/s, with the speed scale already applied — the correction has to be
	/// in the values the marks are found in, or it would not apply to the thing
	/// it was set for.
	pub speed: Track,
	pub engine_speed: Track,
	pub pedal: Track,
	pub gear: States,
	pub others: BTreeMap<&'static str, Track>,
	pub states: BTreeMap<&'static str, States>,
}

impl Samples {
	/// Take one cycle's readings, scaling the speed on the way in.
	fn push(&mut self, set: SampleSet, speed_scale: f64) {
		if let Some((t, v, _)) = set.speed {
			self.speed.push(t, v * speed_scale);
		}
		if let Some((t, v)) = set.engine_speed {
			self.engine_speed.push(t, v);
		}
		if let Some((t, v)) = set.pedal {
			self.pedal.push(t, v);
		}
		if let Some((t, label)) = set.gear {
			self.gear.push(t, label);
		}
		for (key, t, v) in set.others {
			self.others.entry(key).or_default().push(t, v);
		}
		for (key, t, label) in set.states {
			self.states.entry(key).or_default().push(t, label);
		}
	}

	/// Drop everything older than `from`, on every channel.
	fn trim(&mut self, from: Seconds) {
		for track in [&mut self.speed, &mut self.engine_speed, &mut self.pedal]
			.into_iter()
			.chain(self.others.values_mut())
		{
			let drop = track.t.partition_point(|probe| *probe < from);
			track.t.drain(..drop);
			track.v.drain(..drop);
		}
		for states in std::iter::once(&mut self.gear).chain(self.states.values_mut()) {
			let drop = states.t.partition_point(|probe| *probe < from);
			states.t.drain(..drop);
			states.v.drain(..drop);
		}
	}

	/// Move every timestamp onto a clock whose origin is `zero`.
	fn rebase(&mut self, zero: Seconds) {
		for track in [&mut self.speed, &mut self.engine_speed, &mut self.pedal]
			.into_iter()
			.chain(self.others.values_mut())
		{
			for t in &mut track.t {
				*t -= zero;
			}
		}
		for states in std::iter::once(&mut self.gear).chain(self.states.values_mut()) {
			for t in &mut states.t {
				*t -= zero;
			}
		}
	}
}

/// The interval a launch-based mark is known to within.
///
/// Not a tolerance copied out of a table: it is the width of this run's own
/// launch bracket, and a caller prints it as `0-100  6.03 … 6.38 s`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
	/// The shortest the mark can honestly be — it follows from the *latest* the
	/// launch can have been.
	pub earliest: Seconds,
	/// The longest, from the earliest launch.
	pub latest: Seconds,
}

/// One mark that closed, on the run's own clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
	/// In km/h, as the flag states them. The unit is spelled out everywhere it
	/// is shown, because `0-60` is the American figure and that one is in mph.
	pub from_kmh: u32,
	pub to_kmh: u32,
	/// When the upper endpoint was crossed — an interpolated crossing, not the
	/// nearer sample.
	pub closed_at: Seconds,
	/// What the mark took, from the launch or from the lower crossing.
	pub seconds: Seconds,
	/// Present only for a mark that starts from a standstill, whose lower
	/// endpoint is the launch and not a crossing. A rolling mark has two real
	/// crossings, the staleness bias cancels between them, and what is left is a
	/// symmetric ± that [`derive::rolling_mark_sigma`] computes from the
	/// channel's own refresh period.
	pub bracket: Option<Span>,
}

impl Mark {
	/// Whether the lower endpoint is the launch rather than a crossing.
	///
	/// The two are not the same kind of number and neither display should
	/// pretend they are: this one prints as a range, a rolling one as a single
	/// figure with a real ±.
	pub fn starts_at_launch(&self) -> bool {
		self.from_kmh == 0
	}

	/// `Δv / Δt` across the mark's own endpoints.
	///
	/// Its numerator is exact by construction — `Δv` is the mark's definition
	/// and carries no measurement error — so its relative error is the mark
	/// time's. That makes it the most trustworthy acceleration figure available
	/// on a rolling mark and the least on a short launch-based one.
	pub fn avg_accel_ms2(&self) -> f64 {
		(f64::from(self.to_kmh) - f64::from(self.from_kmh)) / KMH_PER_MS / self.seconds
	}
}

/// One acceleration run: the channels as they were read, and what they closed.
///
/// The samples are the file's contents. Every derivative — acceleration, peak,
/// shift costs, distance, power — is recomputed from them in one pass, which is
/// what lets any of those methods be corrected later without re-driving the car.
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
	/// 1-based, as the session file numbers them.
	pub index: usize,
	pub samples: Samples,
	/// The launch, on the run's own clock — so `t` is zero by construction and
	/// `earliest`/`latest` are the bracket either side of it.
	///
	/// `None` when the movement was too brief for either estimator to reach back
	/// through, in which case the clock's origin is the first moving sample and
	/// no mark from a standstill was allowed to close. That is a refusal, not a
	/// fallback: a launch time invented from two samples is not a measurement.
	pub launch: Option<Start>,
	/// In the order they closed, which on a rising pass is the order of their
	/// upper endpoints.
	pub marks: Vec<Mark>,
	/// The run was cancelled or the car came back to a standstill before the
	/// highest mark. The marks that did close are kept — a run that died at 80
	/// still measured 0-60.
	pub aborted: bool,
	/// The poll rate collapsed at some point during this run, so its times carry
	/// more uncertainty than the same figures from a healthy session.
	pub degraded: bool,
}

/// Where the stopwatch stands, which the screen states in a band because it is
/// otherwise invisible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum State {
	/// The car is moving, or has not stood still yet.
	Idle,
	/// Standing still and counting towards [`ARMING_HOLD_S`]. `since` is what
	/// the band counts down from.
	Arming {
		since: Seconds,
	},
	Armed,
	Running,
	/// A run reached its highest mark. The next standstill arms the next one.
	Finished,
	/// The trigger is switched off — for a traffic light, where a genuine
	/// standstill would otherwise arm a run nobody wants.
	Paused,
}

/// What a keystroke means here, so that no key handling reaches this module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
	/// Toggle the trigger off, or back on.
	PauseTrigger,
	/// Throw the current run away, keeping the marks it did close.
	Cancel,
	/// The session has been written out.
	Save,
}

/// What a sample or a command caused.
///
/// Events rather than return codes because one sample can cause several: the
/// cycle that closes `0-100` on a car whose only mark that was closes the run in
/// the same breath, and the screen has to hear about both.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
	Armed,
	/// The first moving sample, on the session's clock.
	///
	/// Not `t0`: the launch is a bracket and it is not known until the movement
	/// has been watched for [`derive::START_FIT_S`].
	Started(Seconds),
	MarkClosed(Mark),
	Finished(Box<Run>),
	Aborted(Box<Run>),
	/// The cycle time collapsed against the session's own established cadence.
	/// Emitted once, because the condition is a state and not an event that
	/// recurs; the rates are carried so the message can say what it cost.
	Degraded {
		now_hz: f64,
		was_hz: f64,
	},
}

/// A run in progress.
struct Active {
	/// The first moving sample — the clock's origin when no launch can be
	/// fitted, and nothing else.
	started_at: Seconds,
	/// When the car last came to a standstill. Everything before it belongs to
	/// the ring buffer and may hold the tail of the *previous* run, so the
	/// launch fit and every mark are timed against the stretch after it.
	stopped_since: Seconds,
	samples: Samples,
	/// One slot per configured mark, so a mark closes once and the order they
	/// were configured in is preserved for the ones that did not.
	results: Vec<Option<Mark>>,
	degraded: bool,
}

/// The stopwatch itself.
///
/// One entry point per kind of input — a sample, a keystroke — and every answer
/// is a list of events. Nothing here reads a clock or a bus.
pub struct Session {
	marks: Vec<(u32, u32)>,
	/// Which mark ends the run: the highest upper endpoint there is. `None` when
	/// no marks were configured at all, in which case a run ends only by
	/// standstill or by cancellation.
	finish_on: Option<usize>,
	ring_seconds: Seconds,
	speed_scale: f64,
	state: State,
	paused: bool,
	stopped_since: Option<Seconds>,
	ring: Samples,
	active: Option<Active>,
	runs: Vec<Run>,
	saved: usize,
	cycles: Vec<Seconds>,
	last_sample_at: Option<Seconds>,
	degraded: bool,
}

impl Session {
	/// `marks` are `(from, to)` pairs in **km/h**, in the order they were asked
	/// for; `ring_seconds` is how much of the approach to keep before the
	/// launch; `speed_scale` multiplies every speed reading on the way in.
	///
	/// A pair whose upper endpoint is not above its lower one is not a mark and
	/// is dropped. The CLI's own parser refuses `100-50` before the adapter is
	/// opened; this is the second line rather than the first, and it exists so
	/// that a nonsense pair cannot become the mark a run is waiting to end on.
	pub fn new(marks: Vec<(u32, u32)>, ring_seconds: Seconds, speed_scale: f64) -> Self {
		let marks: Vec<(u32, u32)> = marks.into_iter().filter(|(from, to)| to > from).collect();
		let finish_on = marks.iter().enumerate().max_by_key(|(_, (from, to))| (*to, *from)).map(|(i, _)| i);
		Session {
			marks,
			finish_on,
			ring_seconds,
			speed_scale,
			state: State::Idle,
			paused: false,
			stopped_since: None,
			ring: Samples::default(),
			active: None,
			runs: Vec::new(),
			saved: 0,
			cycles: Vec::new(),
			last_sample_at: None,
			degraded: false,
		}
	}

	pub fn state(&self) -> State {
		self.state
	}

	pub fn runs(&self) -> &[Run] {
		&self.runs
	}

	/// How many runs have not been written out. The quit guard's whole input:
	/// two keystrokes to throw away a drive, one to keep it.
	pub fn unsaved(&self) -> usize {
		self.runs.len() - self.saved
	}

	/// Whether the cadence has collapsed at any point in this session.
	pub fn degraded(&self) -> bool {
		self.degraded
	}

	/// The recent achieved rate, from the cycles themselves. Measured, never
	/// asserted in advance — there is no `--hz`, and a figure printed before the
	/// loop ran would be a setting pretending to be a measurement.
	pub fn hz(&self) -> Option<f64> {
		let recent = self.cycles.len().checked_sub(CADENCE_CYCLES)?;
		let median = median(&self.cycles[recent..]);
		(median > 0.0).then(|| 1.0 / median)
	}

	/// The median cycle over the whole session, for the file's `config` block.
	pub fn cycle_median_s(&self) -> Option<Seconds> {
		(!self.cycles.is_empty()).then(|| median(&self.cycles))
	}

	/// One poll cycle. Time is a parameter, which is what makes every behaviour
	/// here testable against a profile written by hand.
	pub fn on_sample(&mut self, t: Seconds, set: SampleSet) -> Vec<Event> {
		let mut events = Vec::new();
		events.extend(self.on_cycle(t));

		// The raw integer is read out before the set is consumed: standstill is
		// decided on it and on nothing else.
		let raw = set.speed.map(|(_, _, raw)| raw);
		match self.active.as_mut() {
			Some(active) => {
				active.samples.push(set, self.speed_scale);
				active.degraded |= self.degraded;
			}
			None => {
				self.ring.push(set, self.speed_scale);
				self.ring.trim(t - self.ring_seconds - RING_MARGIN_S);
			}
		}

		let Some(raw) = raw else {
			// A cycle the leading unit did not answer moves no state. It is
			// still a cycle, and the cadence above has already counted it.
			return events;
		};
		let standing = raw == 0;

		match self.state {
			State::Paused => {}
			State::Idle | State::Finished => {
				if standing {
					self.stand_still(t);
				}
			}
			State::Arming { since } => {
				if !standing {
					self.state = State::Idle;
					self.stopped_since = None;
				} else if t - since >= ARMING_HOLD_S {
					self.state = State::Armed;
					events.push(Event::Armed);
				}
			}
			State::Armed => {
				if !standing {
					self.launch(t);
					events.push(Event::Started(t));
				}
			}
			State::Running => {
				if standing {
					// The car came back to a standstill without reaching the
					// highest mark. Whatever did close is kept, and since the
					// car is already stopped the hold starts here rather than a
					// cycle later.
					let run = self.close(true);
					self.stand_still(t);
					events.push(Event::Aborted(Box::new(run)));
				}
			}
		}

		if self.state == State::Running {
			events.extend(self.advance());
		}
		events
	}

	/// A keystroke's worth of input.
	pub fn on_command(&mut self, command: Command) -> Vec<Event> {
		match command {
			Command::PauseTrigger => {
				self.paused = !self.paused;
				if self.paused {
					// A run already under way is not a run the trigger owns, so
					// pausing does not cancel it — and neither does it lose the
					// runs already recorded.
					if self.state != State::Running {
						self.state = State::Paused;
						self.stopped_since = None;
					}
				} else if self.state == State::Paused {
					self.state = State::Idle;
				}
				Vec::new()
			}
			Command::Cancel => match self.active.is_some() {
				true => {
					let run = self.close(true);
					self.state = if self.paused { State::Paused } else { State::Idle };
					self.stopped_since = None;
					vec![Event::Aborted(Box::new(run))]
				}
				false => Vec::new(),
			},
			Command::Save => {
				self.saved = self.runs.len();
				Vec::new()
			}
		}
	}

	/// The car is standing still: start counting towards the hold.
	fn stand_still(&mut self, t: Seconds) {
		self.stopped_since = Some(t);
		self.state = if self.paused { State::Paused } else { State::Arming { since: t } };
	}

	/// The first moving sample after an armed standstill. The ring becomes the
	/// run's opening, which is where the pedal and the engine before the launch
	/// come from.
	fn launch(&mut self, t: Seconds) {
		self.active = Some(Active {
			started_at: t,
			stopped_since: self.stopped_since.unwrap_or(t),
			samples: std::mem::take(&mut self.ring),
			results: vec![None; self.marks.len()],
			degraded: self.degraded,
		});
		self.state = State::Running;
	}

	/// Close whatever this cycle closed, and end the run if that was the
	/// highest mark.
	fn advance(&mut self) -> Vec<Event> {
		let mut events: Vec<Event> = self.close_marks().into_iter().map(Event::MarkClosed).collect();
		let finished = self
			.finish_on
			.zip(self.active.as_ref())
			.is_some_and(|(i, active)| active.results[i].is_some());
		if finished {
			let run = self.close(false);
			self.state = State::Finished;
			self.stopped_since = None;
			events.push(Event::Finished(Box::new(run)));
		}
		events
	}

	/// Every mark whose upper endpoint was crossed by this cycle's sample.
	///
	/// The order is the car's, not the configuration's: on a rising pass the
	/// lower marks cross first whatever order they were asked for in, and two
	/// that close on the same crossing are reported lower first.
	fn close_marks(&mut self) -> Vec<Mark> {
		let Some(active) = self.active.as_mut() else {
			return Vec::new();
		};
		// Timed against the stretch since the car last stood still: the ring
		// ahead of it can hold the tail of the previous run, where the same
		// speeds occur on the way down.
		let pass = active.samples.speed.window(active.stopped_since, f64::INFINITY);
		let launch = derive::start(&pass);

		let mut closed: Vec<Mark> = Vec::new();
		for (i, &(from_kmh, to_kmh)) in self.marks.iter().enumerate() {
			if active.results[i].is_some() {
				continue;
			}
			let lower = match from_kmh {
				// A mark from a standstill starts at the launch, which is
				// earlier than any sample and is known only once the movement
				// has been watched long enough to fit.
				0 => match launch {
					Some(start) => start.t,
					None => continue,
				},
				_ => match pass_start(&pass, f64::from(from_kmh) / KMH_PER_MS) {
					Some(at) => at,
					None => continue,
				},
			};
			let Some(closed_at) = pass.crossing(f64::from(to_kmh) / KMH_PER_MS, lower) else {
				continue;
			};
			let mark = Mark {
				from_kmh,
				to_kmh,
				closed_at,
				seconds: closed_at - lower,
				bracket: launch.filter(|_| from_kmh == 0).map(|start| Span {
					earliest: closed_at - start.latest,
					latest: closed_at - start.earliest,
				}),
			};
			active.results[i] = Some(mark);
			closed.push(mark);
		}
		closed.sort_by_key(|mark| (mark.to_kmh, mark.from_kmh));
		closed
	}

	/// End the run: fit the launch, put every channel on the run's own clock,
	/// and hand back what the file will hold.
	fn close(&mut self, aborted: bool) -> Run {
		let active = self.active.take().expect("a run is only closed while one is open");
		let pass = active.samples.speed.window(active.stopped_since, f64::INFINITY);
		let launch = derive::start(&pass);
		let zero = launch.map_or(active.started_at, |start| start.t);

		let mut marks: Vec<Mark> = active.results.into_iter().flatten().collect();
		marks.sort_by(|a, b| a.closed_at.total_cmp(&b.closed_at));
		for mark in &mut marks {
			// A mark that closed early was timed against a launch fitted over
			// less than its full window. The fit is settled now, so the stored
			// figure is the settled one and the file never disagrees with
			// itself about when the car set off.
			if let (0, Some(start)) = (mark.from_kmh, launch) {
				mark.seconds = mark.closed_at - start.t;
				mark.bracket = Some(Span {
					earliest: mark.closed_at - start.latest,
					latest: mark.closed_at - start.earliest,
				});
			}
			mark.closed_at -= zero;
		}

		let mut samples = active.samples;
		samples.trim(zero - self.ring_seconds);
		samples.rebase(zero);

		let run = Run {
			index: self.runs.len() + 1,
			samples,
			launch: launch.map(|start| Start {
				t: start.t - zero,
				earliest: start.earliest - zero,
				latest: start.latest - zero,
			}),
			marks,
			aborted,
			degraded: active.degraded,
		};
		self.runs.push(run.clone());
		run
	}

	/// Judge the cadence on this cycle's interval.
	///
	/// Measured in **cycle time**, never in missed answers: a unit replying
	/// "response pending" can stall for seconds without missing one, and a
	/// counter of unanswered requests would report a healthy loop while the
	/// stopwatch quietly lost most of its resolution.
	fn on_cycle(&mut self, t: Seconds) -> Option<Event> {
		let previous = self.last_sample_at.replace(t)?;
		let interval = t - previous;
		if interval <= 0.0 {
			return None;
		}
		self.cycles.push(interval);
		if self.degraded || self.cycles.len() < 2 * CADENCE_CYCLES {
			return None;
		}
		let was = median(&self.cycles[..CADENCE_CYCLES]);
		let now = median(&self.cycles[self.cycles.len() - CADENCE_CYCLES..]);
		if was <= 0.0 || now <= DEGRADED_CYCLE_FACTOR * was {
			return None;
		}
		self.degraded = true;
		if let Some(active) = self.active.as_mut() {
			active.degraded = true;
		}
		Some(Event::Degraded {
			now_hz: 1.0 / now,
			was_hz: 1.0 / was,
		})
	}
}

/// Where the rising pass a rolling mark is timed over began.
///
/// The **last** crossing of `from` that the car did not then fall back below.
/// Both endpoints of a mark have to belong to one monotonically rising pass: a
/// driver who reaches 50, lifts to 40 and goes again has made two attempts at
/// `50-100`, and timing the second from the first would report a figure the car
/// never produced.
fn pass_start(track: &Track, from_ms: f64) -> Option<Seconds> {
	let mut at = None;
	for i in 1..track.len() {
		let (previous, current) = (track.v[i - 1], track.v[i]);
		if previous < from_ms && current >= from_ms {
			let rise = current - previous;
			at = Some(match rise > 0.0 {
				true => track.t[i - 1] + (track.t[i] - track.t[i - 1]) * (from_ms - previous) / rise,
				false => track.t[i],
			});
		} else if current < from_ms {
			at = None;
		}
	}
	at
}

/// The middle value, for a slice that is not empty.
fn median(values: &[Seconds]) -> Seconds {
	let mut sorted = values.to_vec();
	sorted.sort_by(f64::total_cmp);
	sorted[sorted.len() / 2]
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The step of a speed channel that answers in hundredths of a km/h — the
	/// finest this project has proven on any car. Nothing here depends on the
	/// figure: it exists so that the raw integer and the float in a sample are
	/// two views of one reading, which is the relationship the whole arming rule
	/// turns on.
	const QUANTUM_KMH: f64 = 0.01;

	/// One cycle of a car doing `kmh`, with every kind of channel populated so
	/// that trimming, rebasing and the ring are exercised on all of them.
	fn sample(t: Seconds, kmh: f64) -> SampleSet {
		let raw = (kmh.max(0.0) / QUANTUM_KMH).round() as u32;
		let kmh = f64::from(raw) * QUANTUM_KMH;
		SampleSet {
			speed: Some((t, kmh / KMH_PER_MS, raw)),
			engine_speed: Some((t, 800.0 + 40.0 * kmh)),
			pedal: Some((t, if kmh > 0.0 { 100.0 } else { 0.0 })),
			gear: Some((t, if kmh > 0.0 { "1".into() } else { "not engaged".into() })),
			others: vec![("boost actual", t, 1.0)],
			states: vec![("selector", t, "D".into())],
		}
	}

	/// Feed a profile in km/h at a fixed cadence, from `from` up to but not
	/// including `to`.
	fn feed(session: &mut Session, from: Seconds, to: Seconds, step: Seconds, kmh: impl Fn(Seconds) -> f64) -> Vec<Event> {
		let mut events = Vec::new();
		let cycles = ((to - from) / step).round().max(0.0) as usize;
		for i in 0..cycles {
			let t = from + i as f64 * step;
			events.extend(session.on_sample(t, sample(t, kmh(t))));
		}
		events
	}

	/// A standstill until `at`, then a constant `a` m/s² for ever.
	fn launch_at(at: Seconds, a: f64) -> impl Fn(Seconds) -> f64 {
		move |t| if t < at { 0.0 } else { a * (t - at) * KMH_PER_MS }
	}

	fn marks_closed(events: &[Event]) -> Vec<(u32, u32)> {
		events
			.iter()
			.filter_map(|e| match e {
				Event::MarkClosed(mark) => Some((mark.from_kmh, mark.to_kmh)),
				_ => None,
			})
			.collect()
	}

	fn finished(events: &[Event]) -> Option<&Run> {
		events.iter().find_map(|e| match e {
			Event::Finished(run) | Event::Aborted(run) => Some(&**run),
			_ => None,
		})
	}

	// ---- arming -------------------------------------------------------

	#[test]
	fn arming_is_decided_on_the_raw_integer_and_never_on_the_scaled_float() {
		// A car creeping at the channel's own smallest step. Scaled by 0.97 and
		// converted to m/s the float is 2.7e-5 — zero to any printed precision,
		// and zero to any comparison written with a tolerance. The integer says
		// otherwise, and the integer is what decides.
		let mut creeping = Session::new(vec![(0, 100)], 3.0, 0.97);
		feed(&mut creeping, 0.0, 5.0, 0.05, |_| QUANTUM_KMH);
		assert_eq!(creeping.state(), State::Idle, "a moving car must not arm");

		// And the correction cannot stop a stopped car from arming either: the
		// raw zero is zero whatever it is multiplied by.
		let mut stopped = Session::new(vec![(0, 100)], 3.0, 0.97);
		feed(&mut stopped, 0.0, 5.0, 0.05, |_| 0.0);
		assert_eq!(stopped.state(), State::Armed);
	}

	#[test]
	fn zero_has_to_hold_for_a_full_second_before_the_trigger_arms() {
		// The hold is what stops a stop-and-go queue arming the trigger in every
		// gap between cars.
		let mut session = Session::new(vec![(0, 100)], 3.0, 1.0);
		feed(&mut session, 0.0, 0.95, 0.05, |_| 0.0);
		assert!(matches!(session.state(), State::Arming { .. }), "{:?}", session.state());

		let events = feed(&mut session, 0.95, 1.05, 0.05, |_| 0.0);
		assert_eq!(events, vec![Event::Armed]);
		assert_eq!(session.state(), State::Armed);
	}

	#[test]
	fn a_car_that_creeps_mid_hold_starts_the_hold_again() {
		let mut session = Session::new(vec![(0, 100)], 3.0, 1.0);
		feed(&mut session, 0.0, 0.8, 0.05, |_| 0.0);
		// One cycle of movement, then still again: the clock restarts, so the
		// hold that was 0.15 s from done now has a full second left.
		session.on_sample(0.8, sample(0.8, 0.5));
		assert_eq!(session.state(), State::Idle);
		let events = feed(&mut session, 0.85, 1.75, 0.05, |_| 0.0);
		assert!(events.is_empty(), "{events:?}");
		let events = feed(&mut session, 1.75, 1.95, 0.05, |_| 0.0);
		assert_eq!(events, vec![Event::Armed]);
	}

	// ---- the launch and the ring --------------------------------------

	#[test]
	fn the_run_starts_at_the_first_moving_sample_and_is_clocked_from_the_bracket() {
		let mut session = Session::new(vec![(0, 100)], 3.0, 1.0);
		let events = feed(&mut session, 0.0, 14.0, 0.05, launch_at(6.0, 4.0));

		// The run starts at the first sample that moved, which at 4 m/s² and
		// 20 Hz is one step after the last zero.
		let started: Vec<Seconds> = events
			.iter()
			.filter_map(|e| match e {
				Event::Started(t) => Some(*t),
				_ => None,
			})
			.collect();
		assert_eq!(started.len(), 1);
		assert!((started[0] - 6.05).abs() < 1e-9, "{started:?}");

		// t0 is not that sample. It is the midpoint of a bracket that straddles
		// the true launch, and on the run's own clock it is the origin.
		let run = finished(&events).expect("the run reached its highest mark");
		let launch = run.launch.expect("a fitted launch");
		assert_eq!(launch.t, 0.0);
		assert!(launch.earliest < 0.0 && launch.latest > 0.0, "{launch:?}");
		// The one bound that is sound rather than modelled: the car was seen
		// moving, so it was already under way by then whatever any fit says.
		let moving = (0..run.samples.speed.len())
			.find(|&i| run.samples.speed.v[i] > 0.0)
			.map(|i| run.samples.speed.t[i])
			.expect("the run moved");
		assert!(launch.latest <= moving, "{launch:?} against {moving}");
		// The width is what the two estimators disagree by, and that
		// disagreement is the whole of the uncertainty a 0-based mark carries.
		assert!(launch.latest - launch.earliest < 0.5, "{launch:?}");
	}

	#[test]
	fn three_seconds_before_the_launch_are_written_into_the_run() {
		// What the pedal and the engine were doing before the start is half of
		// what explains a bad one, and none of it exists once the run has begun.
		let mut session = Session::new(vec![(0, 100)], 3.0, 1.0);
		let events = feed(&mut session, 0.0, 16.0, 0.05, launch_at(8.0, 4.0));
		let run = finished(&events).expect("a finished run");

		for (name, first) in [
			("speed", run.samples.speed.t.first()),
			("engine speed", run.samples.engine_speed.t.first()),
			("pedal", run.samples.pedal.t.first()),
			("gear", run.samples.gear.t.first()),
			("selector", run.samples.states["selector"].t.first()),
			("boost", run.samples.others["boost actual"].t.first()),
		] {
			let first = *first.unwrap_or_else(|| panic!("{name} has no samples"));
			assert!(first >= -3.0, "{name} reaches further back than asked: {first}");
			assert!(first <= -3.0 + 0.05, "{name} keeps less than three seconds: {first}");
		}
		// And it really is the approach, not padding: the car was stationary
		// with the pedal up through all of it.
		assert_eq!(run.samples.pedal.at(-2.0), Some(0.0));
		assert_eq!(run.samples.gear.at(-2.0), Some("not engaged"));
	}

	// ---- marks --------------------------------------------------------

	#[test]
	fn a_mark_from_a_standstill_carries_the_bracket_and_a_rolling_one_does_not() {
		let mut session = Session::new(vec![(0, 100), (50, 100)], 3.0, 1.0);
		let events = feed(&mut session, 0.0, 20.0, 0.05, launch_at(5.0, 4.0));
		let run = finished(&events).expect("a finished run");

		let launched = run.marks.iter().find(|m| m.from_kmh == 0).unwrap();
		let rolling = run.marks.iter().find(|m| m.from_kmh == 50).unwrap();

		assert!(launched.starts_at_launch());
		let bracket = launched.bracket.expect("a launch-based mark prints as a range");
		assert!(bracket.earliest < launched.seconds && launched.seconds < bracket.latest);
		// 100 km/h at 4 m/s² from rest is 6.944 s, and the whole claim a
		// launch-based mark makes is that the truth is inside the interval it
		// prints. Asserting the midpoint instead would be asserting that one of
		// two estimators known to miss is right.
		assert!(bracket.earliest <= 6.9445, "{bracket:?}");
		assert!(bracket.latest >= 6.9444, "{bracket:?}");

		// Both of a rolling mark's endpoints are real crossings, so the
		// staleness bias cancels and there is nothing one-signed to report.
		assert!(!rolling.starts_at_launch());
		assert_eq!(rolling.bracket, None);
		// 50 to 100 km/h at 4 m/s² is 3.472 s, and the interpolated crossings
		// put it there to well under the sample interval.
		assert!((rolling.seconds - 3.472).abs() < 0.01, "{rolling:?}");
		assert!((rolling.avg_accel_ms2() - 4.0).abs() < 0.02, "{rolling:?}");
	}

	#[test]
	fn marks_close_in_the_order_the_car_reaches_them_and_not_the_order_asked_for() {
		// Deliberately jumbled, and with two marks sharing an upper endpoint.
		let mut session = Session::new(vec![(0, 100), (0, 10), (50, 100)], 3.0, 1.0);
		let events = feed(&mut session, 0.0, 20.0, 0.05, launch_at(5.0, 4.0));
		assert_eq!(marks_closed(&events), vec![(0, 10), (0, 100), (50, 100)]);

		let run = finished(&events).expect("a finished run");
		assert_eq!(run.marks.len(), 3);
		assert!(run.marks.windows(2).all(|w| w[0].closed_at <= w[1].closed_at));
	}

	#[test]
	fn a_rolling_mark_is_timed_over_one_rising_pass_and_not_across_a_lift() {
		// 0 to 60, lift back to 40, then away again. `50-100` was attempted
		// twice; timing it from the first crossing would report a figure the car
		// never produced.
		let profile = |t: Seconds| match t {
			t if t < 2.0 => 0.0,
			t if t < 6.0 => 15.0 * (t - 2.0),
			t if t < 8.0 => 60.0 - 10.0 * (t - 6.0),
			t => 40.0 + 15.0 * (t - 8.0),
		};
		let mut session = Session::new(vec![(50, 100)], 3.0, 1.0);
		let events = feed(&mut session, 0.0, 16.0, 0.05, profile);
		let run = finished(&events).expect("a finished run");
		let mark = run.marks.first().expect("50-100 closed on the second attempt");

		// The second crossing of 50 km/h is at 8.667 s and 100 arrives at
		// 12.0 s, so the mark is 3.33 s. Timed from the first crossing at
		// 5.333 s it would have read 6.67 s — twice the car's real figure.
		assert!((mark.seconds - 3.333).abs() < 0.02, "{mark:?}");
	}

	#[test]
	fn the_speed_scale_moves_the_crossing_and_not_only_the_printed_number() {
		// `0-100` has to mean a corrected 100, so the correction applies before
		// the mark is detected. A car whose bus reads 3 % high reaches a true
		// 100 later, and the mark has to say so.
		let mut plain = Session::new(vec![(0, 100)], 3.0, 1.0);
		let mut corrected = Session::new(vec![(0, 100)], 3.0, 0.97);
		let plain = feed(&mut plain, 0.0, 16.0, 0.05, launch_at(4.0, 4.0));
		let corrected = feed(&mut corrected, 0.0, 16.0, 0.05, launch_at(4.0, 4.0));

		let plain = finished(&plain).unwrap().marks[0].seconds;
		let corrected = finished(&corrected).unwrap().marks[0].seconds;
		// The same launch, so the same launch bracket: what is left in the
		// difference is the correction, and it is in the crossing rather than in
		// the printing. A true 100 is an indicated 103.09, which at 4 m/s² is
		// 7.159 s against 6.944 — 0.215 s later.
		assert!(corrected > plain, "{corrected} vs {plain}");
		assert!((corrected - plain - 0.215).abs() < 0.01, "{corrected} vs {plain}");
	}

	// ---- how a run ends -----------------------------------------------

	#[test]
	fn a_run_ends_at_the_highest_mark_and_the_next_standstill_arms_the_next_one() {
		let mut session = Session::new(vec![(0, 10), (0, 50)], 3.0, 1.0);
		let events = feed(&mut session, 0.0, 12.0, 0.05, launch_at(4.0, 4.0));

		let run = finished(&events).expect("a finished run");
		assert!(!run.aborted);
		assert_eq!(run.index, 1);
		assert_eq!(session.state(), State::Finished);
		// 0-50 is the highest mark, so the run ends there even though the car
		// kept accelerating for another six seconds.
		assert_eq!(marks_closed(&events), vec![(0, 10), (0, 50)]);

		// Stop, hold, and the trigger arms again.
		let events = feed(&mut session, 12.0, 13.2, 0.05, |_| 0.0);
		assert_eq!(events, vec![Event::Armed]);
	}

	#[test]
	fn speed_returning_to_zero_flags_the_run_and_keeps_the_marks_that_closed() {
		// A run that died at 80 still measured 0-60.
		let profile = |t: Seconds| match t {
			t if t < 3.0 => 0.0,
			t if t < 8.0 => 16.0 * (t - 3.0),
			t if t < 12.0 => (80.0 - 20.0 * (t - 8.0)).max(0.0),
			_ => 0.0,
		};
		let mut session = Session::new(vec![(0, 60), (0, 100)], 3.0, 1.0);
		// Up to and including the first sample that reads a raw zero, at 12.0.
		let events = feed(&mut session, 0.0, 12.05, 0.05, profile);

		assert_eq!(marks_closed(&events), vec![(0, 60)]);
		let run = events
			.iter()
			.find_map(|e| match e {
				Event::Aborted(run) => Some(&**run),
				_ => None,
			})
			.expect("returning to zero aborts the run");
		assert!(run.aborted);
		assert_eq!(run.marks.len(), 1);
		assert_eq!(run.marks[0].to_kmh, 60);
		assert!(!events.iter().any(|e| matches!(e, Event::Finished(_))));

		// The car is already stopped, so the hold starts at the sample that
		// aborted the run rather than a cycle later.
		assert_eq!(session.state(), State::Arming { since: 12.0 });
		let events = feed(&mut session, 12.05, 13.1, 0.05, |_| 0.0);
		assert_eq!(events, vec![Event::Armed]);
	}

	#[test]
	fn cancelling_keeps_the_marks_that_closed_and_leaves_the_car_moving() {
		let mut session = Session::new(vec![(0, 10), (0, 100)], 3.0, 1.0);
		feed(&mut session, 0.0, 6.0, 0.05, launch_at(3.0, 4.0));
		assert_eq!(session.state(), State::Running);

		let events = session.on_command(Command::Cancel);
		let run = match events.as_slice() {
			[Event::Aborted(run)] => run,
			other => panic!("{other:?}"),
		};
		assert!(run.aborted);
		assert_eq!(run.marks.len(), 1, "0-10 closed and 0-100 did not");
		assert_eq!(session.state(), State::Idle, "the car is still moving");
		assert_eq!(session.runs().len(), 1);
	}

	#[test]
	fn a_second_standstill_starts_a_second_run_in_the_same_session() {
		let mut session = Session::new(vec![(0, 50)], 3.0, 1.0);
		feed(&mut session, 0.0, 10.0, 0.05, launch_at(3.0, 4.0));
		// Stop, hold, and go again.
		feed(&mut session, 10.0, 22.0, 0.05, launch_at(15.0, 4.0));

		assert_eq!(session.runs().len(), 2);
		assert_eq!(session.runs()[0].index, 1);
		assert_eq!(session.runs()[1].index, 2);
		// Each run is clocked from its own launch, and the second one's ring
		// does not carry the first one's deceleration into its launch fit.
		for run in session.runs() {
			let launch = run.launch.expect("each run has its own launch");
			assert_eq!(launch.t, 0.0);
			assert!(run.samples.speed.at(0.0).is_some_and(|v| v < 0.5), "{:?}", run.marks);
			assert!((run.marks[0].seconds - 3.472).abs() < 0.1, "{:?}", run.marks);
		}
	}

	// ---- the trigger switch -------------------------------------------

	#[test]
	fn pausing_the_trigger_prevents_arming_and_loses_no_run() {
		let mut session = Session::new(vec![(0, 50)], 3.0, 1.0);
		feed(&mut session, 0.0, 10.0, 0.05, launch_at(3.0, 4.0));
		assert_eq!(session.runs().len(), 1);

		session.on_command(Command::PauseTrigger);
		assert_eq!(session.state(), State::Paused);
		// A whole minute at a traffic light arms nothing.
		let events = feed(&mut session, 10.0, 70.0, 0.05, |_| 0.0);
		assert!(events.is_empty(), "{events:?}");
		assert_eq!(session.state(), State::Paused);
		assert_eq!(session.runs().len(), 1, "the recorded run is untouched");

		// And it toggles: pressing it again puts the trigger back.
		session.on_command(Command::PauseTrigger);
		let events = feed(&mut session, 70.0, 72.0, 0.05, |_| 0.0);
		assert_eq!(events, vec![Event::Armed]);
	}

	#[test]
	fn pausing_the_trigger_does_not_cancel_a_run_already_under_way() {
		let mut session = Session::new(vec![(0, 50)], 3.0, 1.0);
		feed(&mut session, 0.0, 6.0, 0.05, launch_at(3.0, 4.0));
		assert_eq!(session.state(), State::Running);
		assert!(session.on_command(Command::PauseTrigger).is_empty());
		assert_eq!(session.state(), State::Running);

		let events = feed(&mut session, 6.0, 10.0, 0.05, launch_at(3.0, 4.0));
		assert!(matches!(events.last(), Some(Event::Finished(_))), "{events:?}");
		// The run finished; the trigger stays off, so the next standstill does
		// not arm.
		let events = feed(&mut session, 10.0, 14.0, 0.05, |_| 0.0);
		assert!(events.is_empty(), "{events:?}");
		assert_eq!(session.state(), State::Paused);
	}

	#[test]
	fn saving_clears_what_the_quit_guard_is_arguing_about() {
		let mut session = Session::new(vec![(0, 50)], 3.0, 1.0);
		feed(&mut session, 0.0, 10.0, 0.05, launch_at(3.0, 4.0));
		assert_eq!(session.unsaved(), 1);
		assert!(session.on_command(Command::Save).is_empty());
		assert_eq!(session.unsaved(), 0);
	}

	// ---- the cadence --------------------------------------------------

	#[test]
	fn a_collapsed_rate_is_reported_once_and_not_once_per_cycle() {
		let mut session = Session::new(vec![(0, 100)], 3.0, 1.0);
		feed(&mut session, 0.0, 3.0, 0.05, |_| 0.0);
		assert!(!session.degraded());

		// The loop falls to 5 Hz and stays there.
		let mut t = 3.0;
		let mut events = Vec::new();
		for _ in 0..40 {
			events.extend(session.on_sample(t, sample(t, 0.0)));
			t += 0.2;
		}
		let flags: Vec<&Event> = events.iter().filter(|e| matches!(e, Event::Degraded { .. })).collect();
		assert_eq!(flags.len(), 1, "{flags:?}");
		match flags[0] {
			Event::Degraded { now_hz, was_hz } => {
				assert!((now_hz - 5.0).abs() < 0.01, "{now_hz}");
				assert!((was_hz - 20.0).abs() < 0.01, "{was_hz}");
			}
			other => panic!("{other:?}"),
		}
		assert!(session.degraded());
	}

	#[test]
	fn one_long_cycle_is_a_stall_and_not_a_collapsed_rate() {
		// A unit replying "response pending" can hold the bus for seconds
		// without missing a single answer. Flagging that would put SLOW on the
		// screen of a car doing nothing wrong, which is why the judgement is a
		// median over several cycles rather than one interval.
		let mut session = Session::new(vec![(0, 100)], 3.0, 1.0);
		feed(&mut session, 0.0, 3.0, 0.05, |_| 0.0);
		let events = session.on_sample(5.0, sample(5.0, 0.0));
		assert!(events.is_empty(), "{events:?}");
		let events = feed(&mut session, 5.05, 8.0, 0.05, |_| 0.0);
		assert!(!events.iter().any(|e| matches!(e, Event::Degraded { .. })), "{events:?}");
		assert!(!session.degraded());
	}

	#[test]
	fn a_run_driven_after_the_rate_collapsed_carries_the_flag_and_earlier_ones_do_not() {
		let mut session = Session::new(vec![(0, 50)], 3.0, 1.0);
		feed(&mut session, 0.0, 10.0, 0.05, launch_at(3.0, 4.0));
		assert_eq!(session.runs().len(), 1);
		assert!(!session.runs()[0].degraded);

		// The rate collapses, then a second run is driven on the slow loop.
		let mut t = 10.0;
		let profile = launch_at(25.0, 4.0);
		while t < 40.0 {
			session.on_sample(t, sample(t, profile(t)));
			t += 0.2;
		}
		assert!(session.degraded());
		assert_eq!(session.runs().len(), 2, "the slow loop still timed a run");
		assert!(!session.runs()[0].degraded, "a run that was over before it happened");
		assert!(session.runs()[1].degraded);
	}

	#[test]
	fn the_rate_is_measured_from_the_cycles_and_never_asserted() {
		let mut session = Session::new(vec![(0, 100)], 3.0, 1.0);
		assert_eq!(session.hz(), None, "nothing has been read yet");
		feed(&mut session, 0.0, 2.0, 0.05, |_| 0.0);
		assert!((session.hz().unwrap() - 20.0).abs() < 1e-9);
		assert!((session.cycle_median_s().unwrap() - 0.05).abs() < 1e-9);
	}

	// ---- edges --------------------------------------------------------

	#[test]
	fn a_cycle_the_leading_unit_did_not_answer_moves_no_state() {
		let mut session = Session::new(vec![(0, 100)], 3.0, 1.0);
		feed(&mut session, 0.0, 0.9, 0.05, |_| 0.0);
		// A silent leading channel is not a standstill and not a launch. The
		// other channels are still recorded.
		let quiet = SampleSet {
			engine_speed: Some((0.9, 900.0)),
			..SampleSet::default()
		};
		assert!(session.on_sample(0.9, quiet).is_empty());
		assert!(matches!(session.state(), State::Arming { .. }));
		let events = feed(&mut session, 0.95, 1.1, 0.05, |_| 0.0);
		assert_eq!(events, vec![Event::Armed]);
	}

	#[test]
	fn a_mark_that_is_not_one_never_becomes_the_mark_a_run_waits_to_end_on() {
		// `100-50` is rejected by the CLI's parser; if it ever reached here it
		// would be a mark that can never close, and a run that can never end.
		let mut session = Session::new(vec![(0, 50), (100, 50)], 3.0, 1.0);
		let events = feed(&mut session, 0.0, 10.0, 0.05, launch_at(3.0, 4.0));
		assert!(matches!(events.last(), Some(Event::Finished(_))), "{events:?}");
		assert_eq!(finished(&events).unwrap().marks.len(), 1);
	}

	#[test]
	fn a_session_with_no_marks_records_a_run_and_ends_it_at_the_standstill() {
		let profile = |t: Seconds| match t {
			t if t < 2.0 => 0.0,
			t if t < 6.0 => 15.0 * (t - 2.0),
			t => (60.0 - 30.0 * (t - 6.0)).max(0.0),
		};
		let mut session = Session::new(Vec::new(), 3.0, 1.0);
		let events = feed(&mut session, 0.0, 12.0, 0.05, profile);
		let run = finished(&events).expect("a run with nothing to time is still a run");
		assert!(run.aborted, "nothing closed it, so the standstill did");
		assert!(run.marks.is_empty());
		assert!(run.samples.speed.len() > 100);
	}
}
