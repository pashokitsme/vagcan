//! What a person reads when something goes wrong.
//!
//! Every refusal in this command has a test asserting that it refuses. A test
//! is not a message, and the first draft of the design had a dozen of the
//! former and none of the latter — which is how a tool ends up saying
//! "fit rejected" after twenty minutes of driving.
//!
//! The rule these follow: **say what happened, say what it cost, and end with
//! something to do.** The last part is the one that gets dropped under
//! pressure, and it is the one the reader is actually looking for.
//!
//! They live here rather than inline so that the poll loop never formats prose
//! in the middle of a run, and so that they can be read — and tested — without
//! a car.

use std::fmt::Write as _;

/// One channel the resolution could not find, and the names it looked under.
pub struct MissingChannel {
	pub key: &'static str,
	pub tried: Vec<String>,
}

/// What the units did answer, so the reader can see the check was real.
pub struct ChannelFound {
	pub unit: String,
	pub part_number: String,
	pub key: &'static str,
	pub ok: bool,
}

/// The channel check failed, at a standstill, before anything else happened.
///
/// Naming the survey commands matters more than naming the missing channel: a
/// person whose car is not in the catalogs cannot do anything with "speed is
/// missing", but `survey --diff` is exactly the tool that finds it. The names
/// the resolution looked under come between the two, because they are what
/// somebody reading their own label files can check against without driving.
pub fn missing_channels(found: &[ChannelFound], missing: &[MissingChannel]) -> String {
	let mut out = String::new();
	let names: Vec<&str> = missing.iter().map(|m| m.key).collect();
	let _ = writeln!(
		out,
		"measure needs {}, and this car's catalogs do not have {}.\n",
		names.join(", "),
		if names.len() == 1 { "it" } else { "them" }
	);
	for f in found {
		let _ = writeln!(
			out,
			"    {:<8} {:<12} {:<16} {}",
			f.unit,
			f.part_number,
			f.key,
			if f.ok { "ok" } else { "not in the catalog" }
		);
	}
	// The words, because they are the half of this the reader can act on
	// without a survey: a car whose label files call the channel something else is
	// the ordinary reason for this refusal, and the only useful thing to tell
	// its owner is which names their catalogs would have to use.
	for m in missing.iter().filter(|m| !m.tried.is_empty()) {
		let names: Vec<String> = m.tried.iter().map(|n| format!("\"{n}\"")).collect();
		let _ = writeln!(out, "\n    {} was looked for under {}", m.key, names.join(", "));
	}
	// Through to the end, not as far as the diff. "The identifiers whose bytes
	// moved are the live measurements" tells a reader what they have found and
	// stops there — leaving them holding a list of hex with no way to learn
	// that the next two commands turn it into the catalog this refusal is
	// about. That gap is the whole distance between a refusal and a fix.
	let _ = writeln!(
		out,
		"\n\
         There is no stopwatch without a speed channel, and measure will not guess one\n\
         from raw bytes. The whole way there:\n    \
         vagcan survey --out parked.jsonl      then, after a drive:\n    \
         vagcan survey --out driving.jsonl\n    \
         vagcan survey --diff parked.jsonl driving.jsonl\n\
         The identifiers whose bytes moved are the live measurements. Then record a\n\
         drive with them on screen and fit them against a reading already trusted:\n    \
         vagcan watch --did <the identifiers> --out drive.csv\n    \
         vagcan recording calibrate --log drive.csv --out <part-number>.json\n\
         Move that file to {} — the file name is the unit's own\n\
         F187 part number — and name its rows so this command can find them:\n\
         `speed` and `gear` are what it looks for.\n\n\
         None of this is what `vagcan setup` does. Label files carry names and no\n\
         scaling at all, so no installation of VCDS can supply what is missing here.",
		crate::project::measurements_hint()
	);
	out
}

/// `--full` was asked for on a car whose file is not finished.
///
/// Ends by saying what the run *will* still do, because "power is unavailable"
/// reads as "this command is unavailable" and the times are the point.
pub fn full_without_car_file(known: &[(&str, String)], missing: &[&str]) -> String {
	let mut out = String::from("--full computes power, and power needs this car measured, not assumed.\n\n");
	for (name, value) in known {
		let _ = writeln!(out, "    {name:<20} {value}");
	}
	for name in missing {
		let _ = writeln!(out, "    {name:<20} missing");
	}
	let _ = writeln!(
		out,
		"\n\
         Park the car and run:  vagcan measure setup\n\
         It keeps what you already answered and asks only for what is left.\n\n\
         Running without --full: every time, every mark, acceleration, distance and\n\
         shift cost. Only the power column is absent."
	);
	out
}

/// The car publishes no barometer, or no ambient air temperature.
///
/// This used to be a refusal, and it was the wrong shape: the reading is
/// legislated but not universal, and a car that simply does not expose it was
/// being sent home from a job the standard atmosphere can do. So the fallback is
/// announced, priced, and offered as a question instead — because a person who
/// can see a forecast can do better than a standard, and one who cannot should
/// still get a car file.
///
/// It states the cost in the terms the fit is actually sensitive to. `ρ` enters
/// drag linearly and the fit returns `½·ρ·CdA`, so an error in `ρ` is the same
/// error in `CdA`, one for one: about 3 % per 30 hPa and 3 % per 10 °C, and a
/// real day is often both.
pub fn no_barometer() -> String {
	"\nThis car publishes no barometer, or no outside air temperature — SAE J1979's\n\
     PIDs 0x33 and 0x46 — and the coastdown fit returns ½·ρ·CdA, so a drag area\n\
     without a density is not a number.\n\n\
     That is not a reason to stop. The ISO 2533 standard atmosphere is 1013.25 hPa at\n\
     15 °C, which is 1.2250 kg/m³, and the car file will say plainly that is where the\n\
     density came from. What it costs: density enters the drag term linearly, so a day\n\
     30 hPa and 10 °C away from standard puts about 6 % on the aerodynamic half of the\n\
     fit, and every power figure computed from it afterwards carries the same 6 %.\n\n\
     So the next two questions are worth answering if you can. A forecast, a weather\n\
     app or an airport METAR has both numbers; press Enter twice if you have neither\n\
     and the standard atmosphere will be used and recorded as such.\n\n\
     If this car does answer those readings under other names, this will find them:\n    \
     vagcan survey --out parked.jsonl"
		.to_string()
}

/// A coastdown pass was thrown away.
///
/// The direction sentence is the load-bearing part. The tool cannot tell which
/// way the car is pointing — there is no compass on this bus — so a driver who
/// turns around after a rejection silently poisons the pair.
pub fn pass_rejected(index: usize, reason: &str, accepted: usize, wanted: usize) -> String {
	format!(
		"pass {index} rejected — {reason}.\n\
         Stay pointing the way you are now and do it again: you still owe one pass\n\
         in this direction.   Passes so far: {accepted} of {wanted}."
	)
}

/// A coastdown pass was accepted, and the next one goes the other way.
pub fn pass_accepted(index: usize, from_kmh: f64, to_kmh: f64, seconds: f64, wanted: usize) -> String {
	format!(
		"pass {index} accepted — {from_kmh:.1} → {to_kmh:.1} km/h, {seconds:.1} s\n\
         Turn around and repeat on the same stretch, pointing the other way.\n\
         (Waiting for pass {} of {wanted}.)",
		index + 1
	)
}

/// The two passes disagreed, so neither is trusted.
///
/// The worst moment in the feature: twenty minutes of driving and no result.
/// So it ends with a plan rather than a verdict, and it reports the slope and
/// the wind the disagreement implies — both come free from the same two fits,
/// and both are things the driver can act on.
pub fn fit_rejected(
	crr_disagreement_percent: f64,
	limit_percent: f64,
	passes: &[(f64, f64)],
	implied_grade_percent: f64,
	implied_wind_ms: f64,
) -> String {
	let mut out = format!(
		"The two passes disagree by {crr_disagreement_percent:.0} % on Crr (limit \
         {limit_percent:.0} %), so neither is\ntrusted and no road load was written.\n\n"
	);
	for (i, (cda, crr)) in passes.iter().enumerate() {
		let _ = writeln!(out, "    pass {}   CdA {cda:.2} m²   Crr {crr:.4}", i + 1);
	}
	let _ = writeln!(
		out,
		"\n    implied slope between them: {implied_grade_percent:.1} %\n    \
         implied wind:               {implied_wind_ms:.1} m/s\n\n\
         Both passes fit their own data well, so this is not noise — something differed\n\
         between the two directions. A slope of about 1 % would do it, and so would that\n\
         much wind.\n\n\
         What to try, in order:\n  \
         • a flatter stretch — both passes must be the same piece of road\n  \
         • a calmer day; above roughly 2 m/s the wind alone shifts Crr by 2 %\n  \
         • warm the car first: cold tyres and cold gearbox oil read a higher rolling\n    \
           resistance than the car will have on a run\n\n\
         Nothing else is lost. Mass and tyre size are saved. Re-run vagcan measure setup\n\
         and it asks only for the passes."
	);
	out
}

/// The poll rate collapsed mid-session.
///
/// Says what it costs rather than only that it happened: "degraded" on its own
/// reads as a judgement about the car.
pub fn degraded(now_hz: f64, was_hz: f64) -> String {
	format!(
		"SLOW — {now_hz:.0} Hz (was {was_hz:.0}). A control unit has started timing out.\n\n\
         The times are still real, but their uncertainty has roughly tripled and the run\n\
         is flagged `degraded` in the file. Marks from a standstill are worst affected.\n\
         Try --minimal, or check the adapter at the OBD port."
	)
}

/// The car went quiet — ignition off, a pulled connector, a unit that stopped.
///
/// Says what survived, because the reader's first thought is that the session
/// is gone.
pub fn car_silent(saved_runs: usize) -> String {
	let kept = match saved_runs {
		0 => "nothing was recorded yet".to_string(),
		1 => "1 saved run is untouched".to_string(),
		n => format!("{n} saved runs are untouched"),
	};
	format!(
		"the car stopped answering. Current run discarded; {kept}.\n\
         Waiting — this will pick up again when the ignition is back on."
	)
}

/// A car file that belongs to a different car.
///
/// Applying it would put another car's mass and drag on these numbers, which
/// is exactly the false comparison this design spends its length avoiding.
pub fn wrong_car(file_vin: &str, car_vin: &str) -> String {
	format!(
		"that car file is for {file_vin} and this car says {car_vin}.\n\
         Ignoring it: mass and road load belong to one specific car. Run\n\
         vagcan measure setup for this one, or pass --car with the right file."
	)
}

/// Quitting with work that has not been written.
///
/// Two keystrokes to throw away a drive, one to keep it. The alternative — a
/// file nobody asked for — is what the design rejected when it made saving
/// explicit.
pub fn unsaved_on_quit(runs: usize) -> String {
	format!("{runs} runs not saved.   [s] save    [q] again to discard")
}

/// The session was written out.
///
/// It names the path because saving is explicit here by design, and an
/// acknowledgement that does not say *where* leaves the driver to go looking.
pub fn saved(path: &str, runs: usize) -> String {
	format!("saved {runs} {} to {path}", if runs == 1 { "run" } else { "runs" })
}

/// No car file yet: the run happens anyway, and says what it is not doing.
pub fn no_car_file(vin: &str) -> String {
	format!(
		"no car file for {vin} — default mode: times, speeds and telemetry,\n\
         no power. Park, then run: vagcan measure setup"
	)
}

/// A car file exists: say so, or twenty minutes of coastdown look like nothing.
pub fn car_file_summary(vin: &str, at: &str, mass_kg: f64, cda: f64, measured: bool) -> String {
	format!(
		"{vin} — car file {at} (mass {mass_kg:.0} kg, CdA {cda:.2} m² {})\n\
         default mode: times and telemetry. Add --full for the power column.",
		if measured { "measured" } else { "stated" }
	)
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The rule the whole module exists for, asserted rather than assumed.
	fn ends_with_something_to_do(text: &str) -> bool {
		// An instruction, not a verdict: either a command to run, a key to
		// press, a thing to try, or a state to wait in.
		["vagcan ", "[s] save", "Try ", "Waiting", "do it again"]
			.iter()
			.any(|hint| text.contains(hint))
	}

	#[test]
	fn every_refusal_ends_with_something_the_reader_can_do() {
		let texts = vec![
			missing_channels(
				&[ChannelFound {
					unit: "7E1".into(),
					part_number: "0CW300041G".into(),
					key: "speed",
					ok: false,
				}],
				&[MissingChannel {
					key: "speed",
					tried: vec!["vehicle speed".into()],
				}],
			),
			full_without_car_file(&[("mass", "1475 kg".into())], &["CdA, Crr"]),
			pass_rejected(2, "the brake was used", 1, 2),
			fit_rejected(11.0, 5.0, &[(0.61, 0.0109), (0.68, 0.0121)], 0.9, 4.1),
			degraded(6.0, 21.0),
			car_silent(3),
			wrong_car("XW8AD4NE9JH008917", "XW8AD4NE9JH000123"),
			unsaved_on_quit(4),
			no_car_file("XW8AD4NE9JH008917"),
			no_barometer(),
		];
		for text in texts {
			assert!(ends_with_something_to_do(&text), "no way out offered:\n{text}");
		}
	}

	#[test]
	fn a_rejected_pass_says_which_way_to_point() {
		// The tool cannot see direction, so a driver who turns around after a
		// rejection silently ruins the pair. This sentence is the only thing
		// preventing that.
		let text = pass_rejected(2, "the selector left N", 1, 2);
		assert!(text.contains("Stay pointing the way you are now"), "{text}");
		assert!(text.contains("1 of 2"), "{text}");
	}

	#[test]
	fn a_rejected_fit_reports_the_slope_and_the_wind_it_implies() {
		// Both come free from the same two fits, and both are things the driver
		// can act on — unlike "rejected".
		let text = fit_rejected(11.0, 5.0, &[(0.61, 0.0109), (0.68, 0.0121)], 0.9, 4.1);
		assert!(text.contains("implied slope"), "{text}");
		assert!(text.contains("implied wind"), "{text}");
		assert!(text.contains("0.9 %"), "{text}");
	}

	#[test]
	fn losing_a_drive_costs_two_keystrokes_and_keeping_it_costs_one() {
		let text = unsaved_on_quit(4);
		assert!(text.contains("[s] save"), "{text}");
		assert!(text.contains("[q] again to discard"), "{text}");
	}

	#[test]
	fn the_car_going_quiet_says_what_survived() {
		// The reader's first thought is that the session is gone.
		assert!(car_silent(3).contains("3 saved runs are untouched"));
		assert!(car_silent(1).contains("1 saved run is untouched"));
		assert!(car_silent(0).contains("nothing was recorded yet"));
	}

	#[test]
	fn saving_says_where_it_went() {
		// Saving is explicit by design; an acknowledgement that does not name
		// the path leaves the driver to go looking for their own drive.
		assert_eq!(saved("/tmp/drive.json", 1), "saved 1 run to /tmp/drive.json");
		assert!(saved("/tmp/drive.json", 4).contains("4 runs"));
	}

	#[test]
	fn a_car_with_no_barometer_is_told_the_price_of_the_standard_atmosphere() {
		// The owner's question was "what do I do if the car has no barometer" —
		// and the answer has to be a way forward with its cost attached, not a
		// refusal and not a silent substitution.
		let text = no_barometer();
		assert!(text.contains("ISO 2533"), "{text}");
		assert!(text.contains("1013.25 hPa"), "{text}");
		assert!(text.contains("1.2250 kg/m³"), "{text}");
		// What it costs, in the units the fit is sensitive to.
		assert!(text.contains("linearly"), "{text}");
		assert!(text.contains("6 %"), "{text}");
		// And the escape: he can state the real value himself.
		assert!(text.contains("next two questions"), "{text}");
		assert!(!text.contains("cannot continue"), "{text}");
	}

	#[test]
	fn a_missing_channel_names_the_unit_and_the_way_to_find_it() {
		let text = missing_channels(
			&[ChannelFound {
				unit: "7E1".into(),
				part_number: "0CW300041G".into(),
				key: "speed",
				ok: false,
			}],
			&[MissingChannel {
				key: "speed",
				tried: vec!["vehicle speed".into()],
			}],
		);
		assert!(text.contains("0CW300041G"), "{text}");
		assert!(text.contains("survey --diff"), "{text}");
	}

	#[test]
	fn a_missing_channel_says_which_names_were_looked_under() {
		// The resolution carried them all the way here and then dropped them.
		// For a car this project has never seen, the words its label files would
		// have to use are the only thing the owner can check by reading.
		let text = missing_channels(
			&[],
			&[MissingChannel {
				key: "speed",
				tried: vec!["vehicle speed".into(), "road speed".into()],
			}],
		);
		assert!(text.contains("\"vehicle speed\""), "{text}");
		assert!(text.contains("\"road speed\""), "{text}");
	}

	#[test]
	fn the_refusal_goes_all_the_way_to_a_catalog_rather_than_to_a_list_of_hex() {
		// It used to end at `survey --diff`, which finds the identifiers and
		// says nothing about what to do with them. A reader who follows it to
		// the letter is left holding a list of hex and no catalog — and the
		// catalog is what this command refused for.
		let text = missing_channels(
			&[],
			&[MissingChannel {
				key: "speed",
				tried: vec!["vehicle speed".into()],
			}],
		);
		for step in ["survey --diff", "watch --did", "recording calibrate"] {
			assert!(text.contains(step), "{step} missing from:\n{text}");
		}
		// Where the file goes, resolved rather than written out: this literal
		// used to be `~/.vagcan/data/measured/`, and it reached somebody
		// standing at a car after the store had moved out from under it. A
		// printed path is a promise, so it is asked rather than remembered.
		assert!(text.contains("measurements"), "where the file goes:\n{text}");
		assert!(!text.contains("data/measured/"), "the path this instruction moved off:\n{text}");
		// And it rules out the other shortage explicitly, because "the tool has
		// no data" is the same sentence for both and only one of them is true
		// here.
		assert!(text.contains("None of this is what `vagcan setup` does"), "{text}");
	}
}
