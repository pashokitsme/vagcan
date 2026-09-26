//! `vagcan dev recording dash` — a recorded drive on the dash panel, with its alarms.
//!
//! `todo/dash/04` asked for the retard alarm to be shown from a recorded drive, with no
//! board and no car. This is that: a `watch --out` recording and the car's `dash.toml` in,
//! the board's own screen code over them, and out the panel in the terminal at the
//! recording's pace with a line for every takeover, hand-back, silence and press. Piped, only
//! the lines — so a run can be read by a script or a test. Nothing is written anywhere, and
//! nothing is opened but the two files and what the plan build reads.
//!
//! [`engine`] is the replay and holds no terminal; [`columns`] finds the plan's channels in
//! the recording; [`glass`] is the panel as pixels and as text; [`view`] is the terminal.

pub mod columns;
pub mod engine;
pub mod glass;
mod view;

use std::io::{IsTerminal, Write};
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};

use crate::watch::replay::Recording;
use engine::{Refusal, Replay};
use vag_dash_render::button::PRESS_GAP_MS;

/// The command: `log` is a `watch --out` recording, `vin` and `input` name the dash plan as
/// `vagcan dev dash build` resolves it, `presses` are short presses in seconds of the
/// recording's own clock, `speed` how much faster than it happened.
pub fn run(vin: &str, log: &str, input: Option<&Path>, presses: &[f64], speed: f64) -> Result<()> {
	let text = std::fs::read_to_string(log).with_context(|| format!("reading the recording {log:?}"))?;
	let recording = Recording::parse(&text).map_err(|e| anyhow!("{log}: {e}"))?;
	let resolved = vag_cli_core::dash::resolve_for_car(vin, input)?;
	let plan = &resolved.built.plan;

	// Every channel `watch` offers this car — the names its headings were written from.
	let offered = crate::plan::with_survey(
		crate::plan::available(&resolved.store, &resolved.extracted, &resolved.units),
		&resolved.survey,
	);
	let answered = crate::plan::answered_from_survey(&resolved.survey);
	let matched = columns::match_columns(&recording.columns, &offered, &answered, plan);
	let mut notes = unreplayed(plan);
	notes.extend(matched.notes);
	// The board's plan is `'static`; this one lives until the process ends anyway.
	let device: &'static vag_dash_render::plan::Plan = Box::leak(Box::new(plan.to_device()));
	let series = columns::series(&recording, &matched.sources, plan, device, &mut notes);
	if series.iter().all(Option::is_none) {
		bail!(
			"none of the plan's {} channels is in {log}, so there is nothing to replay\n{}",
			plan.channels.len(),
			notes.join("\n")
		);
	}

	let times = recording.samples.iter().map(|(t, _)| columns::to_ms(*t));
	let (start, end) = (times.clone().min().unwrap_or(0), times.max().unwrap_or(0));
	let at = presses.iter().map(|&press| columns::to_ms(press)).collect();
	let replay = Replay::new(device, series, at, start, end).map_err(|e| anyhow!("{e}"))?;
	let seconds = |ms: u64| ms as f64 / 1000.0;
	for &(ms, why) in replay.refused() {
		notes.push(match why {
			Refusal::Outside => format!(
				"the press at {:.2} s is outside the recording's frames ({:.2}–{:.2} s) — not made",
				seconds(ms),
				seconds(start),
				seconds(replay.last_frame_ms())
			),
			Refusal::TooSoon => format!(
				"the press at {:.2} s is under {PRESS_GAP_MS} ms after the one before, and the board takes the two as one — not made",
				seconds(ms)
			),
		});
	}

	let stdout = std::io::stdout();
	if !stdout.is_terminal() {
		for note in &notes {
			eprintln!("{note}");
		}
		return write_log(replay, &mut stdout.lock());
	}
	let title = Path::new(log)
		.file_name()
		.map_or_else(|| log.to_string(), |n| n.to_string_lossy().into_owned());
	let lines = view::show(replay, &notes, &title, speed)?;
	// The screen is gone; what happened stays in the shell.
	for line in lines {
		println!("{line}");
	}
	Ok(())
}

/// What the plan has that the replay does not run, said before anything else
/// (`todo/dash/19`). The lever is pressed on the board by reading the car, and a recording
/// holds none of those reads as presses; without it the stopwatch page is never entered.
/// `--press` is the one button the replay has.
pub fn unreplayed(plan: &vag_cli_core::dash::Plan) -> Vec<String> {
	let mut notes = Vec::new();
	if plan.stalk.is_some() {
		notes.push("the plan's [stalk] lever is not replayed — the only press here is --press, the button's short press".to_string());
	}
	if plan.stopwatch.is_some() {
		notes.push("the plan's stopwatch page is not replayed — the lever enters it, and the lever is not replayed".to_string());
	}
	notes
}

/// The whole run as its log: one line per event, as fast as it computes, no panel.
pub fn write_log(mut replay: Replay, out: &mut impl Write) -> Result<()> {
	while let Some(tick) = replay.step() {
		for event in &tick.events {
			writeln!(out, "{}", replay.line(tick.t_ms, event))?;
		}
	}
	out.flush()?;
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;
	use vag_dash_render::alarm::{Alarm, ChannelId, Direction, PageId, Rule};
	use vag_dash_render::plan::{Channel, Page, Plan};

	const fn channel(did: u16, label: &'static str) -> Channel {
		Channel {
			unit: 0x7E0,
			did,
			bit_offset: 0,
			bit_length: 16,
			signed: true,
			big_endian: true,
			factor: 1.0,
			offset: 0.0,
			decimals: 0,
			unit_text: "",
			label,
			proven: false,
			hz: 10.0,
			setpoint: None,
		}
	}

	static CHANNELS: [Channel; 2] = [channel(0x1001, "A"), channel(0x1002, "B")];
	static PAGES: [Page; 2] = [Page::Values { title: "MAIN", cells: &[0] }, Page::Values { title: "HIGH", cells: &[1] }];
	static WATCHED: [ChannelId; 1] = [ChannelId(1)];
	static RULES: [Alarm<'static>; 1] = [Alarm {
		channels: &WATCHED,
		page: PageId(1),
		rule: Rule::Threshold {
			trip: 10.0,
			release: 8.0,
			direction: Direction::Above,
		},
	}];
	static PLAN: Plan = Plan {
		vin: "TESTVIN0000000001",
		language: "en",
		units: &[],
		channels: &CHANNELS,
		pages: &PAGES,
		alarms: &RULES,
		stalk: None,
		stopwatch: None,
	};

	#[test]
	fn piped_output_is_the_log_and_nothing_else() {
		let calm: engine::Series = (0..=40).map(|i| (i * 100, Some(0.0))).collect();
		let high: engine::Series = (0..=40)
			.map(|i| (i * 100, Some(if (10..15).contains(&i) { 12.0 } else { 0.0 })))
			.collect();
		let replay = Replay::new(&PLAN, vec![Some(calm), Some(high)], vec![1_200], 0, 4_000).unwrap();
		let mut out = Vec::new();
		write_log(replay, &mut out).unwrap();
		let text = String::from_utf8(out).unwrap();
		assert_eq!(
			text,
			"     0.00 s  start on page 1 \"MAIN\"\n\
			 \x20    1.00 s  alarm #1 took the screen — page 2 \"HIGH\", B (01:1002) = 12 (trips at ≥ 10, releases below 8)\n\
			 \x20    1.20 s  press — silences alarm #1\n\
			 \x20    1.20 s  alarm #1 silenced — back to page 1 \"MAIN\"\n"
		);
		// No panel, no terminal control: a script reads it as it is.
		assert!(!text.contains(['▀', '▄', '█', '\x1b']), "{text}");
	}

	#[test]
	fn the_lever_and_the_stopwatch_are_said_to_be_left_out() {
		use vag_cli_core::dash::{Plan as Input, Stalk, Stopwatch};
		let mut plan = Input {
			vin: "TESTVIN0000000001".into(),
			language: "en".into(),
			units: vec![],
			channels: vec![],
			pages: vec![],
			alarms: vec![],
			stalk: None,
			stopwatch: None,
		};
		assert!(unreplayed(&plan).is_empty(), "a plan without them replays whole");
		plan.stalk = Some(Stalk {
			rocker: 0,
			switch: 1,
			cruise: 2,
			rocker_states: vec![],
			switch_states: vec![],
			cruise_states: vec![],
			next: 0,
			previous: 0,
			measure: 0,
			switch_off: 0,
			cruise_off: 0,
		});
		plan.stopwatch = Some(Stopwatch {
			speed: 0,
			km_h_per_unit: 0.0,
			marks: vec![60],
		});
		let notes = unreplayed(&plan);
		assert_eq!(notes.len(), 2);
		assert!(
			notes[0].contains("[stalk] lever is not replayed") && notes[1].contains("stopwatch page is not replayed"),
			"{notes:?}"
		);
	}
}
