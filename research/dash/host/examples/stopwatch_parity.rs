//! `stopwatch_parity` — the board's stopwatch against the laptop's, on synthetic runs.
//!
//! ```text
//! cd research/dash/host
//! cargo run --release --example stopwatch_parity
//! ```
//!
//! What it is for: the board's `vag_dash_render::stopwatch::Stopwatch` is a port of
//! `vag-cli-measure`'s `session::Session`, and the two have to time a run alike. This feeds
//! both the same speed samples — 7,200 synthetic launches over 10–100 Hz, with and without
//! jitter, noise and a dead band, at four channel factors — and compares the launch and each
//! mark's time. On 2026-09-27 it found the 400 ms window-edge bug fixed in `4c5af0d`: a sample
//! exactly 400 ms after the first moving one fell out of the laptop's launch window to `f64`
//! rounding (`2.3 + 0.4` is `2.6999999999999997`), and board and laptop disagreed by up to
//! 65 ms. After the fix they agree to within 1.6 µs; with it reverted, this version fails 198
//! pairs, the worst 87 ms apart.
//!
//! Input: nothing. The runs are synthetic and seeded, so every invocation drives the same ones.
//!
//! Output: a summary on stdout — runs, worst |laptop − board| in seconds, pairs 10 ms or more
//! apart, presence mismatches (a figure one side has and the other does not) — then a few
//! hand-written edge cases side by side. Exit code 1 when any synthetic pair is 10 ms or more
//! apart or present on one side only, 0 otherwise. The edge cases are printed for reading and
//! not judged: in one of them the two part by design (`stopwatch::Run::time`).

use std::process::ExitCode;

use vag_cli_measure::power::KMH_PER_MS;
use vag_cli_measure::session::{Event as LaptopEvent, SampleSet, Session};
use vag_dash_render::stopwatch::{Event as BoardEvent, Stopwatch};

/// The marks every synthetic run is timed at, in km/h.
static MARKS: [u16; 2] = [60, 100];

/// Two figures this far apart fail the check: far over what `f32` loses on a run of
/// seconds, a fifth of a sample at 20 Hz.
const TOLERANCE_S: f64 = 0.010;

/// How many disagreements are printed one by one; the rest are only counted.
const SHOWN: usize = 5;

/// One speed sample as both machines take it: milliseconds, and the channel's raw integer.
type Sample = (u64, i64);

/// One run, as one machine timed it.
#[derive(Debug)]
struct Timed {
	/// Seconds from the first moving sample back to the launch, so never after `0.0`.
	launch: Option<f64>,
	/// Each mark's time from the launch, by the mark's place in the list.
	marks: Vec<Option<f64>>,
	aborted: bool,
}

/// xorshift64: seeded, so the profiles are the same on every invocation.
struct Rng(u64);

impl Rng {
	/// Uniform in `[0, 1)`.
	fn next(&mut self) -> f64 {
		self.0 ^= self.0 << 13;
		self.0 ^= self.0 >> 7;
		self.0 ^= self.0 << 17;
		(self.0 >> 11) as f64 / (1u64 << 53) as f64
	}

	/// Uniform in `[-1, 1)`.
	fn sym(&mut self) -> f64 {
		2.0 * self.next() - 1.0
	}
}

/// The laptop's `Session`, fed until its first run ends.
fn laptop(samples: &[Sample], factor: f64, marks: &[u16]) -> Option<Timed> {
	let pairs = marks.iter().map(|mark| (0, u32::from(*mark))).collect();
	// A 10 s ring, no speed correction.
	let mut session = Session::new(pairs, 10.0, 1.0);
	for &(t_ms, raw) in samples {
		let t = t_ms as f64 / 1000.0;
		let set = SampleSet {
			speed: Some((t, raw as f64 * factor / KMH_PER_MS, raw.max(0) as u32)),
			..Default::default()
		};
		for event in session.on_sample(t, set) {
			let (LaptopEvent::Finished(run) | LaptopEvent::Aborted(run)) = event else {
				continue;
			};
			// The run's clock starts at the launch. Every run here stands still until it launches,
			// so the first moving sample on that clock is the one that started the run, and the
			// launch lies as far before it as that sample lies after zero.
			let speed = &run.samples.speed;
			let first_moving = speed.t.iter().zip(&speed.v).find(|(_, v)| **v > 0.0).map(|(t, _)| *t);
			return Some(Timed {
				launch: run.launch.and(first_moving).map(|t| -t),
				marks: marks
					.iter()
					.map(|mark| run.marks.iter().find(|m| m.to_kmh == u32::from(*mark)).map(|m| m.seconds))
					.collect(),
				aborted: run.aborted,
			});
		}
	}
	None
}

/// The board's `Stopwatch`, fed until its first run ends. A new stopwatch has no finished run
/// waiting for its flash write (`Stopwatch::hold`), so it arms on its first standstill held.
fn board(samples: &[Sample], factor: f32, marks: &[u16]) -> Option<Timed> {
	let mut watch = Stopwatch::new(marks, factor);
	for &(t_ms, raw) in samples {
		if let Some(BoardEvent::Finished | BoardEvent::Aborted) = watch.sample(Some(raw as f32), t_ms) {
			let run = watch.run()?;
			return Some(Timed {
				launch: run.launch.map(f64::from),
				marks: (0..marks.len()).map(|i| run.time(i).map(f64::from)).collect(),
				aborted: run.aborted,
			});
		}
	}
	None
}

/// A launch from standstill to 110 km/h as a speed channel reports it: constant jerk up to
/// `accel` km/h/s, fading towards 250 km/h, integrated in 0.1 ms steps and sampled every
/// `period_ms` (± `jitter` of it). Standing or under `deadband_kmh` the channel reads zero; above it the
/// raw value is the speed over `factor`, with ±1.5 counts of `noise`, never zero.
fn profile(rng: &mut Rng, period_ms: f64, jitter: f64, factor: f64, deadband_kmh: f64, noise: bool, accel: f64) -> Vec<Sample> {
	let t0_ms = 1500.0 + 500.0 * rng.next();
	// km/h/s².
	let jerk = 60.0 + 80.0 * rng.next();
	let vmax = 250.0;
	let mut v = 0.0f64;
	let mut out = Vec::new();
	let mut next_ms = 0.0f64;
	let mut t_ms = 0.0f64;
	while t_ms < 30_000.0 {
		while t_ms < next_ms {
			let t = t_ms / 1000.0;
			let a = match t_ms < t0_ms {
				true => 0.0,
				false => (jerk * (t - t0_ms / 1000.0)).min(accel) * (1.0 - v / vmax),
			};
			v += a * 0.1e-3;
			t_ms += 0.1;
		}
		// A standing car reads zero with no dead band too: `v < deadband_kmh` alone read it as 1
		// at `db=0`, so a third of the runs never armed and compared nothing.
		let raw = match v <= 0.0 || v < deadband_kmh {
			true => 0,
			false => {
				let mut raw = (v / factor).round() as i64;
				if noise {
					raw += (rng.sym() * 1.5).round() as i64;
				}
				raw.max(1)
			}
		};
		out.push((next_ms.round() as u64, raw));
		if v >= 110.0 {
			break;
		}
		next_ms += (period_ms * (1.0 + jitter * rng.sym())).max(1.0);
	}
	out
}

/// What the synthetic runs came to.
#[derive(Default)]
struct Tally {
	runs: usize,
	/// Runs both machines timed, and runs neither did.
	both: usize,
	neither: usize,
	/// Figures both machines have, compared.
	pairs: usize,
	worst: f64,
	worst_at: String,
	/// Pairs [`TOLERANCE_S`] or more apart.
	far: usize,
	/// A run, or a figure in one, that one machine has and the other does not.
	presence: usize,
}

impl Tally {
	fn add(&mut self, case: &str, laptop: Option<Timed>, board: Option<Timed>) {
		self.runs += 1;
		let (laptop, board) = match (laptop, board) {
			(Some(laptop), Some(board)) => (laptop, board),
			(None, None) => {
				self.neither += 1;
				return;
			}
			(laptop, board) => {
				self.presence += 1;
				if self.presence <= SHOWN {
					println!("presence  {case}: laptop {}  board {}", show(&laptop), show(&board));
				}
				return;
			}
		};
		self.both += 1;
		let names = std::iter::once("launch".to_string()).chain(MARKS.iter().map(|mark| format!("0-{mark}")));
		let laptops = std::iter::once(laptop.launch).chain(laptop.marks);
		let boards = std::iter::once(board.launch).chain(board.marks);
		for ((name, l), b) in names.zip(laptops).zip(boards) {
			match (l, b) {
				(Some(l), Some(b)) => {
					self.pairs += 1;
					let d = (l - b).abs();
					if d > self.worst {
						self.worst = d;
						self.worst_at = format!("{case} {name}: laptop {l:.6} board {b:.6}");
					}
					if d >= TOLERANCE_S {
						self.far += 1;
						if self.far <= SHOWN {
							println!("far       {case} {name}: laptop {l:.6} board {b:.6}");
						}
					}
				}
				(None, None) => {}
				(l, b) => {
					self.presence += 1;
					if self.presence <= SHOWN {
						println!("presence  {case} {name}: laptop {l:?} board {b:?}");
					}
				}
			}
		}
	}
}

/// One machine's run on one line.
fn show(timed: &Option<Timed>) -> String {
	let Some(timed) = timed else {
		return "no run".into();
	};
	let figure = |f: Option<f64>| f.map_or("-".into(), |s| format!("{s:.6}"));
	let marks: Vec<String> = timed.marks.iter().map(|m| figure(*m)).collect();
	let end = if timed.aborted { "aborted" } else { "finished" };
	format!("launch {} marks [{}] {end}", figure(timed.launch), marks.join(", "))
}

fn edge(name: &str, samples: &[Sample], marks: &[u16]) {
	println!("  {name}");
	println!("    laptop {}", show(&laptop(samples, 1.0, marks)));
	println!("    board  {}", show(&board(samples, 1.0, marks)));
}

fn main() -> ExitCode {
	let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
	let mut tally = Tally::default();
	for hz in [10.0, 20.0, 33.0, 50.0, 100.0] {
		for factor in [0.0337, 0.1, 0.5, 1.0] {
			for db in [0.0, 1.5, 3.0] {
				for noise in [false, true] {
					for jit in [0.0, 0.3] {
						for acc in [12.0, 25.0, 40.0] {
							for _ in 0..10 {
								let samples = profile(&mut rng, 1000.0 / hz, jit, factor, db, noise, acc);
								let case = format!("hz={hz} f={factor} db={db} noise={noise} jit={jit} acc={acc}");
								tally.add(&case, laptop(&samples, factor, &MARKS), board(&samples, factor as f32, &MARKS));
							}
						}
					}
				}
			}
		}
	}

	println!("runs {}: timed by both {}, by neither {}", tally.runs, tally.both, tally.neither);
	println!("pairs compared {}, worst |laptop - board| {:.9} s", tally.pairs, tally.worst);
	if !tally.worst_at.is_empty() {
		println!("  at {}", tally.worst_at);
	}
	println!("pairs >= 10 ms: {}, presence mismatches: {}", tally.far, tally.presence);

	println!("edge cases, not judged (launch, then each mark's time, in s):");
	// A low mark crossed before the launch: the laptop times it negative, the board refuses it.
	edge(
		"crossed before launch, marks 20/1000",
		&[(0, 0), (1000, 0), (1450, 100), (1500, 400), (1550, 900), (1600, 1100)],
		&[20, 1000],
	);
	edge(
		"a sample exactly at the mark",
		&[(0, 0), (1000, 0), (1050, 2), (1100, 5), (1150, 9), (1200, 60), (1250, 100)],
		&MARKS,
	);
	edge(
		"dips under 60 and crosses again",
		&[
			(0, 0),
			(1000, 0),
			(1050, 2),
			(1100, 5),
			(1150, 9),
			(1200, 61),
			(1250, 58),
			(1300, 62),
			(1350, 101),
		],
		&MARKS,
	);
	edge(
		"duplicate timestamps",
		&[
			(0, 0),
			(1000, 0),
			(1050, 2),
			(1050, 3),
			(1100, 5),
			(1150, 9),
			(1200, 59),
			(1200, 61),
			(1250, 101),
		],
		&MARKS,
	);
	edge(
		"600 ms gap mid-run",
		&[(0, 0), (1000, 0), (1050, 2), (1100, 5), (1150, 9), (1200, 40), (1800, 70), (1850, 101)],
		&MARKS,
	);
	edge(
		"one count of speed at rest",
		&[(0, 0), (500, 0), (1000, 0), (1020, 1), (1040, 0), (1060, 0)],
		&MARKS,
	);

	match tally.far == 0 && tally.presence == 0 {
		true => {
			println!("PASS");
			ExitCode::SUCCESS
		}
		false => {
			println!("FAIL");
			ExitCode::FAILURE
		}
	}
}
