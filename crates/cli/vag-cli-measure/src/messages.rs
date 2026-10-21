//! What a person reads when something goes wrong.
//!
//! Every refusal in this command has a test asserting that it refuses. A test
//! is not a message, and the first draft of the design had a dozen of the
//! former and none of the latter — which is how a tool ends up saying
//! "fit rejected" after twenty minutes of driving.
//!
//! The rule these follow: **say what happened, and end with something to do** — in
//! two lines, which is what every run prints ([`Message::text`]). The reasoning
//! behind a refusal, what was looked for and what it costs is the reader's to ask
//! for: it is [`Message::detail`], written at `-v` (owner, 2026-10-20: the default
//! output is the data, technical and structured).
//!
//! They live here rather than inline so that the poll loop never formats prose
//! in the middle of a run, and so that they can be read — and tested — without
//! a car.

use std::fmt::Write as _;

use vag_cli_core::table::{Table, pairs};

/// A message in two parts: what every run says — for a refusal, what failed and what
/// to do — and what a reader checking it needs, said at `-v`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
	/// For a refusal two lines: what failed, then what to do.
	pub text: String,
	/// The evidence and the reasoning; empty when there is none.
	pub detail: String,
}

impl Message {
	/// The refusal as the error a run ends with: its first line the `error:`, each line after
	/// it a `help:`.
	pub fn into_error(&self) -> anyhow::Error {
		use vag_cli_core::report::Advice as _;
		let mut lines = self.text.lines().map(str::trim).filter(|l| !l.is_empty());
		let err = anyhow::anyhow!("{}", lines.next().unwrap_or_default());
		lines.fold(err, |err, line| err.help(line))
	}
}

/// One channel the resolution could not find, and what it looked under: the
/// text ids, then the names, in the order they were tried.
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
/// Naming the way to the channels matters more than naming the missing channel:
/// a person whose car's rows are not in the project cannot do anything with
/// "speed is missing", but `vagcan setup` is what brings the rows. The ids and the
/// names the resolution looked under are the detail, because they are what somebody
/// reading their own label files can check against without driving.
pub fn missing_channels(found: &[ChannelFound], missing: &[MissingChannel]) -> Message {
	let names: Vec<&str> = missing.iter().map(|m| m.key).collect();
	let error = format!(
		"no {} channel in this car's catalogs\n\
         vagcan setup <ODIS project | VCDS installation> brings the channels; -v lists what was looked for",
		names.join(", ")
	);
	let mut detail = String::new();
	if !found.is_empty() {
		let mut table = Table::new(&["unit", "part", "role", "found"]);
		for f in found {
			table.row(vec![
				f.unit.clone(),
				f.part_number.clone(),
				f.key.to_string(),
				if f.ok { "yes" } else { "no" }.to_string(),
			]);
		}
		detail.push_str(&table.render(None));
	}
	// The ids and the words, because they are the half of this the reader can
	// act on from their desk: what a row of their own label files has to carry
	// to be found — one of the text ids, or the words in its name with no id of
	// its own — in the role's unit of measure.
	let looked: Vec<(&str, String)> = missing
		.iter()
		.filter(|m| !m.tried.is_empty())
		.map(|m| {
			(
				m.key,
				format!(
					"looked for under {}",
					m.tried.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ")
				),
			)
		})
		.collect();
	detail.push_str(&pairs(&looked));
	// Quoted from `missing::scalings_path`, not written again here. This message
	// and `watch`'s describe the same job on the same car, and they had drifted
	// to five commands and three: a reader who meets the shortage twice cannot
	// tell two wordings of one procedure from two procedures. Only what is
	// genuinely `measure`'s — which rows it looks for — is written above.
	let _ = write!(
		detail,
		"a channel is found by the text ids above; by the names above, for a row with no text id \
         when none on the car carries one of the ids, or for a row a drive proved; and, on an engine \
         for which the project declares no OBD-II rows, by the standard OBD-II identifiers. A row \
         in another unit of measure is not used, and no channel is guessed from raw bytes.\n\
         If this car's project has no channels for its units yet, `vagcan setup` brings them:\n{}",
		vag_cli_core::missing::scalings_path()
	);
	Message { text: error, detail }
}

/// `--full` was asked for on a car whose file is not finished.
///
/// The detail says what the run *will* still do, because "power is unavailable"
/// reads as "this command is unavailable" and the times are the point.
pub fn full_without_car_file(known: &[(&str, String)], missing: &[&str]) -> Message {
	let error = format!(
		"--full needs this car measured: {} missing\n\
         park and run: vagcan measure setup (it asks only for what is missing)",
		missing.join(", ")
	);
	let rows: Vec<(&str, String)> = known
		.iter()
		.map(|(name, value)| (*name, value.clone()))
		.chain(missing.iter().map(|name| (*name, "missing".to_string())))
		.collect();
	let mut detail = pairs(&rows);
	detail.push_str("without --full: every time, every mark, acceleration, distance and shift cost; only the power column is absent");
	Message { text: error, detail }
}

/// The car publishes no barometer, or no ambient air temperature: the two lines said
/// before the two questions, and what the fallback costs, at `-v`.
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
pub fn no_barometer() -> Message {
	Message {
		text: "no barometer or outside air temperature on this car (SAE J1979 PIDs 0x33, 0x46)\n\
                answer the next two questions from a forecast, or press Enter twice for the ISO 2533 \
                standard atmosphere: 1013.25 hPa, 15 °C, 1.2250 kg/m³"
			.to_string(),
		detail: "the coastdown fit returns ½ρCdA, so density enters the drag term linearly: a day 30 hPa \
                 and 10 °C away from standard puts about 6 % on the aerodynamic half of the fit, and on \
                 every power figure computed from it; the car file records where the density came from"
			.to_string(),
	}
}

/// A coastdown pass was thrown away.
///
/// The direction is the load-bearing part. The tool cannot tell which way the car
/// is pointing — there is no compass on this bus — so a driver who turns around
/// after a rejection silently poisons the pair.
pub fn pass_rejected(index: usize, reason: &str, accepted: usize, wanted: usize) -> String {
	format!(
		"pass {index} rejected: {reason}\n\
         stay pointing the way you are now and do it again: one pass is still owed in this direction ({accepted} of {wanted} done)"
	)
}

/// A coastdown pass was accepted, and the next one goes the other way.
pub fn pass_accepted(index: usize, from_kmh: f64, to_kmh: f64, seconds: f64, wanted: usize) -> String {
	format!(
		"pass {index} accepted: {from_kmh:.1} → {to_kmh:.1} km/h, {seconds:.1} s\n\
         turn around: pass {} of {wanted} on the same stretch, pointing the other way",
		index + 1
	)
}

/// The two passes disagreed, so neither is trusted.
///
/// The worst moment in the feature: twenty minutes of driving and no result.
/// So it ends with a plan rather than a verdict, and it reports the slope and
/// the wind the disagreement implies — both come free from the same two fits,
/// and both are things the driver can act on.
pub struct FitRejected {
	/// The two fits and what they imply, as a `key  value` block: the command's data.
	pub data: String,
	pub refusal: Message,
}

pub fn fit_rejected(
	crr_disagreement_percent: f64,
	limit_percent: f64,
	passes: &[(f64, f64)],
	implied_grade_percent: f64,
	implied_wind_ms: f64,
) -> FitRejected {
	let keys: Vec<String> = (1..=passes.len()).map(|i| format!("pass {i}")).collect();
	let mut rows: Vec<(&str, String)> = keys
		.iter()
		.zip(passes)
		.map(|(key, (cda, crr))| (key.as_str(), format!("CdA {cda:.2} m²  Crr {crr:.4}")))
		.collect();
	rows.push(("implied slope", format!("{implied_grade_percent:.1} %")));
	rows.push(("implied wind", format!("{implied_wind_ms:.1} m/s")));
	FitRejected {
		data: pairs(&rows),
		refusal: Message {
			text: format!(
				"no road load written: the two passes disagree by {crr_disagreement_percent:.0} % on Crr (limit {limit_percent:.0} %)\n\
                 re-run vagcan measure setup on a flatter stretch or a calmer day; it asks only for the passes"
			),
			detail: "both passes fit their own data well, so something differed between the two directions: \
                     a slope of about 1 % would do it, and so would that much wind\n\
                     try, in order: a flatter stretch (both passes on the same piece of road); a calmer day \
                     (above about 2 m/s the wind alone shifts Crr by 2 %); the car warm (cold tyres and gearbox \
                     oil read a higher rolling resistance than a run has)\n\
                     mass and tyre size are saved"
				.to_string(),
		},
	}
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
		"the car file is for {file_vin} and this car is {car_vin}\n\
         run vagcan measure setup for this car, or pass --car with its own file"
	)
}

/// Quitting with work that has not been written, on the full screen.
///
/// Two keystrokes to throw away a drive, one to keep it. The alternative — a
/// file nobody asked for — is what the design rejected when it made saving
/// explicit.
pub fn unsaved_on_quit(runs: usize) -> String {
	format!("{runs} runs not saved.   [s] save    [q] again to discard")
}

/// What is said once the command has quit with runs never written: nothing can save
/// them any more, so no key is offered.
pub fn unsaved_at_exit(runs: usize) -> String {
	format!("{runs} {} not saved", if runs == 1 { "run" } else { "runs" })
}

/// Above a run's results printed again on the plain console, once the density read
/// has answered: the table before it had the standard atmosphere's.
pub const DENSITY_MEASURED: &str = "With the measured air density:";

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
		["vagcan ", "[s] save", "press Enter", "Try ", "Waiting", "do it again"]
			.iter()
			.any(|hint| text.lines().last().unwrap_or_default().contains(hint))
	}

	fn a_missing_speed() -> Message {
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
		)
	}

	#[test]
	fn every_refusal_ends_with_something_the_reader_can_do() {
		let texts = vec![
			a_missing_speed().text,
			full_without_car_file(&[("mass", "1475 kg".into())], &["CdA, Crr"]).text,
			pass_rejected(2, "the brake was used", 1, 2),
			fit_rejected(11.0, 5.0, &[(0.61, 0.0109), (0.68, 0.0121)], 0.9, 4.1).refusal.text,
			degraded(6.0, 21.0),
			car_silent(3),
			wrong_car("XW8AD4NE9JH008917", "XW8AD4NE9JH000123"),
			unsaved_on_quit(4),
			no_car_file("XW8AD4NE9JH008917"),
			no_barometer().text,
		];
		for text in texts {
			assert!(ends_with_something_to_do(&text), "no way out offered:\n{text}");
		}
	}

	#[test]
	fn a_refusal_is_two_lines_what_failed_then_what_to_do() {
		for refusal in [
			a_missing_speed(),
			full_without_car_file(&[], &["CdA, Crr"]),
			fit_rejected(11.0, 5.0, &[], 0.9, 4.1).refusal,
			no_barometer(),
		] {
			assert_eq!(refusal.text.lines().count(), 2, "{}", refusal.text);
		}
		assert_eq!(wrong_car("A", "B").lines().count(), 2);
		assert_eq!(pass_rejected(2, "the brake was used", 1, 2).lines().count(), 2);
	}

	#[test]
	fn a_rejected_pass_says_which_way_to_point() {
		// The tool cannot see direction, so a driver who turns around after a
		// rejection silently ruins the pair. This sentence is the only thing
		// preventing that.
		let text = pass_rejected(2, "the selector left N", 1, 2);
		assert!(text.contains("stay pointing the way you are now"), "{text}");
		assert!(text.contains("1 of 2"), "{text}");
	}

	#[test]
	fn a_rejected_fit_reports_the_slope_and_the_wind_it_implies() {
		// Both come free from the same two fits, and both are things the driver
		// can act on — unlike "rejected". They are the data, on stdout.
		let rejected = fit_rejected(11.0, 5.0, &[(0.61, 0.0109), (0.68, 0.0121)], 0.9, 4.1);
		assert_eq!(
			rejected.data,
			"pass 1         CdA 0.61 m²  Crr 0.0109\n\
             pass 2         CdA 0.68 m²  Crr 0.0121\n\
             implied slope  0.9 %\n\
             implied wind   4.1 m/s\n"
		);
		assert!(
			rejected
				.refusal
				.text
				.starts_with("no road load written: the two passes disagree by 11 % on Crr (limit 5 %)")
		);
		assert!(rejected.refusal.detail.contains("a flatter stretch"), "{}", rejected.refusal.detail);
	}

	#[test]
	fn losing_a_drive_costs_two_keystrokes_and_keeping_it_costs_one() {
		let text = unsaved_on_quit(4);
		assert!(text.contains("[s] save"), "{text}");
		assert!(text.contains("[q] again to discard"), "{text}");
		// Once the command has quit no key works, so none is offered.
		assert_eq!(unsaved_at_exit(4), "4 runs not saved");
		assert_eq!(unsaved_at_exit(1), "1 run not saved");
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
		let said = no_barometer();
		// Every run: the fallback, in numbers, and how to answer.
		assert!(said.text.contains("ISO 2533"), "{}", said.text);
		assert!(said.text.contains("1013.25 hPa"), "{}", said.text);
		assert!(said.text.contains("1.2250 kg/m³"), "{}", said.text);
		assert!(said.text.contains("next two questions"), "{}", said.text);
		assert!(!said.text.contains("cannot continue"), "{}", said.text);
		// What it costs, in the units the fit is sensitive to: at -v.
		assert!(said.detail.contains("linearly"), "{}", said.detail);
		assert!(said.detail.contains("6 %"), "{}", said.detail);
	}

	#[test]
	fn a_missing_channel_names_the_unit_and_the_way_to_find_it() {
		let refusal = a_missing_speed();
		assert_eq!(
			refusal.text,
			"no speed channel in this car's catalogs\n\
             vagcan setup <ODIS project | VCDS installation> brings the channels; -v lists what was looked for"
		);
		assert!(refusal.detail.contains("0CW300041G"), "{}", refusal.detail);
	}

	#[test]
	fn a_missing_channel_says_which_ids_and_names_were_looked_under() {
		// The resolution carried them all the way here and then dropped them.
		// For a car this project has never seen, the ids and the words its label
		// files would have to use are the only thing the owner can check by
		// reading — and the sentence after them has to say that both were tried,
		// not that a name is the only way in.
		let detail = missing_channels(
			&[],
			&[MissingChannel {
				key: "speed",
				tried: vec!["IDE00075".into(), "vehicle speed".into(), "road speed".into()],
			}],
		)
		.detail;
		assert!(
			detail.starts_with("speed  looked for under \"IDE00075\", \"vehicle speed\", \"road speed\"\n"),
			"{detail}"
		);
		// "text id" is what every other screen and document calls the key.
		assert!(detail.contains("by the text ids above"), "{detail}");
		assert!(!detail.contains("ODX"), "{detail}");
		assert!(!detail.contains("by the names above only"), "no longer true:\n{detail}");
	}

	#[test]
	fn the_refusal_goes_all_the_way_to_the_channels_rather_than_to_a_list_of_hex() {
		// It used to end at a sweep, which finds identifiers and says nothing
		// about what to do with them, and then at a drive and
		// `calibrate`, which is gone (owner, 2026-09-28). Since that day `setup`
		// brings the channels from either source, and the one path is quoted
		// verbatim, not as a list of substrings: substrings are what let this
		// message and `watch`'s drift apart once.
		let refusal = missing_channels(
			&[],
			&[MissingChannel {
				key: "speed",
				tried: vec!["vehicle speed".into()],
			}],
		);
		assert!(
			refusal.detail.contains(vag_cli_core::missing::scalings_path()),
			"not the one path `watch` prints:\n{}",
			refusal.detail
		);
		// Said as a condition: a project that has read a source and still has
		// no speed row is short of something `setup` cannot bring.
		assert!(refusal.detail.contains("has no channels for its units yet"), "{}", refusal.detail);
		let both = format!("{}\n{}", refusal.text, refusal.detail);
		assert!(!both.contains("calibrate"), "that command is gone:\n{both}");
	}

	#[test]
	fn full_without_a_measured_car_says_what_is_missing_and_what_still_works() {
		let refusal = full_without_car_file(&[("mass", "1475 kg".into())], &["CdA, Crr"]);
		assert_eq!(
			refusal.text,
			"--full needs this car measured: CdA, Crr missing\n\
             park and run: vagcan measure setup (it asks only for what is missing)"
		);
		assert!(refusal.detail.starts_with("mass      1475 kg\nCdA, Crr  missing\n"), "{}", refusal.detail);
		assert!(refusal.detail.contains("only the power column is absent"), "{}", refusal.detail);
	}
}
