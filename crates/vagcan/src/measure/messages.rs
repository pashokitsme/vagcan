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
/// missing", but `survey --diff` is exactly the tool that finds it.
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
    let _ = writeln!(
        out,
        "\n\
         There is no stopwatch without a speed channel, and measure will not guess one\n\
         from raw bytes. To find it:\n    \
         vagcan survey --out parked.jsonl      then, after a drive:\n    \
         vagcan survey --out driving.jsonl\n    \
         vagcan survey --diff parked.jsonl driving.jsonl\n\
         The identifiers whose bytes moved are the live measurements."
    );
    out
}

/// `--full` was asked for on a car whose file is not finished.
///
/// Ends by saying what the run *will* still do, because "power is unavailable"
/// reads as "this command is unavailable" and the times are the point.
pub fn full_without_car_file(known: &[(&str, String)], missing: &[&str]) -> String {
    let mut out = String::from(
        "--full computes power, and power needs this car measured, not assumed.\n\n",
    );
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
                &[MissingChannel { key: "speed", tried: vec!["vehicle speed".into()] }],
            ),
            full_without_car_file(&[("mass", "1475 kg".into())], &["CdA, Crr"]),
            pass_rejected(2, "the brake was used", 1, 2),
            fit_rejected(11.0, 5.0, &[(0.61, 0.0109), (0.68, 0.0121)], 0.9, 4.1),
            degraded(6.0, 21.0),
            car_silent(3),
            wrong_car("XW8AD4NE9JH008917", "XW8AD4NE9JH000123"),
            unsaved_on_quit(4),
            no_car_file("XW8AD4NE9JH008917"),
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
    fn a_missing_channel_names_the_unit_and_the_way_to_find_it() {
        let text = missing_channels(
            &[ChannelFound {
                unit: "7E1".into(),
                part_number: "0CW300041G".into(),
                key: "speed",
                ok: false,
            }],
            &[MissingChannel { key: "speed", tried: vec!["vehicle speed".into()] }],
        );
        assert!(text.contains("0CW300041G"), "{text}");
        assert!(text.contains("survey --diff"), "{text}");
    }
}
