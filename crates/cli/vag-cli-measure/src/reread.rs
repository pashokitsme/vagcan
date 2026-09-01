//! Reading a saved session back into the types that produced it.
//!
//! The session file holds two different kinds of thing, and only one of them is
//! evidence. `series` and `marks` are what the car did — samples with their own
//! timestamps, crossings found in them — and they cannot be collected again
//! without driving. `derived` is arithmetic over those samples: the
//! acceleration trace, the peaks, the distance, what each gearchange cost.
//!
//! So `derived` is a **cache**, and [`crate::report`]'s own module doc
//! says as much: it exists so that the screen and the file cannot disagree, and
//! `stamp` names the methods that produced it "so a reader running different
//! maths knows to recompute rather than believe it".
//!
//! Nothing was that reader until now. When the shift cost was corrected — a
//! baseline that could land on a lift, downshifts costed as though they were
//! shifts, signs printed below the session's own noise — every session already
//! on disk went on showing the old figures, because `measure view` rendered the
//! stored block. That is the bug this module closes: a saved drive is re-derived
//! from its own samples by whatever build opens it.
//!
//! **It reconstructs or it refuses.** A file it cannot turn back into a `Run`
//! faithfully is left with its stored block and a line saying so, because a
//! half-rebuilt run would recompute over samples it does not have and quietly
//! produce a *different* wrong answer. The reader is deliberately strict about
//! what it will accept and says which field defeated it.

use serde_json::Value;

use super::channels;
use super::session::{Mark, Run, Samples, Span};
use super::types::{Seconds, States, Track};

/// One saved run, back in the shape the physics takes.
///
/// `index`, `aborted` and `degraded` come straight from the file; `launch` is
/// not stored and is not needed, because the marks it produced are.
pub fn run(value: &Value) -> Option<Run> {
	let series = value.get("series")?.as_object()?;
	let mut samples = Samples {
		// Speed is the one channel a run cannot be without: every mark is timed
		// from it, so a file that has lost it is not a run to recompute.
		speed: track(series.get("speed")?)?,
		engine_speed: series.get("engine_speed").and_then(track).unwrap_or_default(),
		pedal: series.get("pedal").and_then(track).unwrap_or_default(),
		gear: series.get("gear").and_then(states).unwrap_or_default(),
		..Samples::default()
	};

	// Every other role goes back under the key it left as. The list comes from
	// `channels` rather than from a copy here: a reader with its own list would
	// drift from the writer the first time a role was added, and the drift
	// would look like a channel the car did not answer.
	for role in channels::known_roles() {
		if matches!(role, "speed" | "engine speed" | "pedal" | "gear") {
			continue;
		}
		let Some(found) = series.get(&super::file_key(role)) else { continue };
		if let Some(numbers) = track(found) {
			samples.others.insert(role, numbers);
		} else if let Some(words) = states(found) {
			samples.states.insert(role, words);
		}
	}

	Some(Run {
		index: value.get("index")?.as_u64()? as usize,
		samples,
		launch: None,
		marks: value.get("marks")?.as_array()?.iter().filter_map(mark).collect(),
		aborted: value.get("aborted").and_then(Value::as_bool).unwrap_or(false),
		degraded: value.get("degraded").and_then(Value::as_bool).unwrap_or(false),
	})
}

/// A mark that closed. One that did not is written as a stub with no time, and
/// is skipped rather than invented — `recompute` is told what happened, not
/// what was asked for.
fn mark(value: &Value) -> Option<Mark> {
	let seconds = value.get("seconds")?.as_f64()?;
	let bracket = value.get("bracket_s").and_then(|span| {
		Some(Span {
			earliest: span.get("earliest")?.as_f64()?,
			latest: span.get("latest")?.as_f64()?,
		})
	});
	Some(Mark {
		from_kmh: value.get("from")?.as_u64()? as u32,
		to_kmh: value.get("to")?.as_u64()? as u32,
		// Not stored: it is only ever used while a run is open, to order the
		// marks as they close. The file already holds them in that order.
		closed_at: seconds,
		seconds,
		bracket,
	})
}

fn track(value: &Value) -> Option<Track> {
	let (t, v) = (numbers(value.get("t")?)?, numbers(value.get("v")?)?);
	// A track whose columns are different lengths is not half a track. It is a
	// file this reader does not understand, and pairing what it can would
	// silently shorten the run.
	(t.len() == v.len()).then_some(Track { t, v })
}

fn states(value: &Value) -> Option<States> {
	let t = numbers(value.get("t")?)?;
	let v: Vec<String> = value
		.get("v")?
		.as_array()?
		.iter()
		.map(|word| word.as_str().map(str::to_string))
		.collect::<Option<_>>()?;
	(t.len() == v.len()).then_some(States { t, v })
}

fn numbers(value: &Value) -> Option<Vec<Seconds>> {
	value.as_array()?.iter().map(Value::as_f64).collect()
}

#[cfg(test)]
mod tests {
	use super::*;
	use serde_json::json;

	fn a_run() -> Value {
		json!({
				"index": 2,
				"aborted": true,
				"degraded": false,
				"series": {
						"speed": { "t": [0.0, 0.5, 1.0], "v": [0.0, 5.0, 10.0] },
						"engine_speed": { "t": [0.0, 0.5], "v": [900.0, 2400.0] },
						"gear": { "t": [0.0, 0.5], "v": ["1", "2"] },
						"selector": { "t": [0.0], "v": ["D"] },
						"boost_actual": { "t": [0.0, 0.5], "v": [1.0, 1.8] }
				},
				"marks": [
						{ "from": 0, "to": 10, "seconds": 1.19, "from_t0": true,
							"bracket_s": { "earliest": 1.14, "latest": 1.24 } },
						{ "from": 0, "to": 25, "from_t0": true }
				]
		})
	}

	#[test]
	fn a_saved_run_comes_back_as_the_run_that_was_driven() {
		let run = run(&a_run()).expect("a session this tool wrote");
		assert_eq!(run.index, 2);
		assert!(run.aborted);
		assert_eq!(run.samples.speed.v, vec![0.0, 5.0, 10.0]);
		assert_eq!(run.samples.gear.v, vec!["1", "2"]);
		// A numeric role and a state role both land under the key they left as.
		assert_eq!(run.samples.others["boost actual"].v, vec![1.0, 1.8]);
		assert_eq!(run.samples.states["selector"].v, vec!["D"]);
	}

	#[test]
	fn a_mark_that_never_closed_is_left_out_rather_than_given_a_time() {
		// The writer emits a stub for every mark that was asked for, so that the
		// page can show what the run did not reach. Recomputing over a stub with
		// an invented time would put a number where the car never went.
		let run = run(&a_run()).unwrap();
		assert_eq!(run.marks.len(), 1, "{:?}", run.marks);
		assert_eq!(run.marks[0].to_kmh, 10);
		assert_eq!(run.marks[0].bracket.unwrap().earliest, 1.14);
	}

	#[test]
	fn a_file_this_reader_cannot_rebuild_faithfully_is_refused() {
		// Each of these would recompute over samples that are not the ones the
		// car gave, which is worse than showing the figures as recorded.
		let mut ragged = a_run();
		ragged["series"]["speed"]["v"] = json!([0.0, 5.0]);
		assert!(run(&ragged).is_none(), "columns of different lengths");

		let mut speedless = a_run();
		speedless["series"].as_object_mut().unwrap().remove("speed");
		assert!(run(&speedless).is_none(), "no speed is no run");

		assert!(run(&json!({ "index": 1 })).is_none(), "no series at all");
	}
}
