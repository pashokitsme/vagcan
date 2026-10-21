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
use vag_cli_core::verbosity;

use anyhow::{Context, Result, anyhow};

use crate::watch::replay::Recording;
use engine::{Refusal, Replay};
use vag_dash_render::button::PRESS_GAP_MS;

/// The command: `log` is a `watch --out` recording, `vin` and `input` name the dash plan as
/// `vagcan dev dash build` resolves it, `presses` are `dashsim`'s short presses in seconds of
/// the recording's own clock, `speed` how much faster than it happened.
pub fn run(vin: &str, log: &str, input: Option<&Path>, presses: &[f64], speed: f64) -> Result<()> {
	let text = std::fs::read_to_string(log).with_context(|| format!("reading the recording {log:?}"))?;
	let recording = Recording::parse(&text).map_err(|e| anyhow!("{log}: {e}"))?;
	let resolved = vag_cli_core::dash::resolve_for_car(vin, input)?;
	// Every channel `watch` offers this car — the names its headings were written from.
	// The project's, for the car's recorded units: a plan channel is always one of those,
	// since the build refuses anything the project does not describe.
	let offered = crate::plan::available(&resolved.store, &resolved.extracted, &resolved.units);
	let panel = std::io::stdout().is_terminal().then_some(speed);
	replay(log, &recording, &resolved.built.plan, &offered, presses, panel)
}

/// [`run`] once the recording is read and the plan resolved: `offered` is every channel `watch`
/// offers the car. `panel` is the speed to draw the panel at in the terminal; without one, the
/// log at once. The log ends on stdout either way.
fn replay(
	log: &str,
	recording: &Recording,
	plan: &vag_cli_core::dash::Plan,
	offered: &[crate::plan::Channel],
	presses: &[f64],
	panel: Option<f64>,
) -> Result<()> {
	let matched = columns::match_columns(&recording.columns, offered, plan);
	let mut gaps = matched.gaps;
	// The board's plan is `'static`; this one lives until the process ends anyway.
	let device: &'static vag_dash_render::plan::Plan = Box::leak(Box::new(plan.to_device()));
	let series = columns::series(recording, &matched.sources, plan, device, &mut gaps);
	// What the replay leaves out, and what each kind of gap rests on — at `-v`; the gaps
	// themselves as rows, after them.
	let mut notes = unreplayed(plan);
	notes.extend(matched.notes);
	notes.extend(columns::caveats(&gaps));
	let rows = columns::gap_rows(&gaps);
	if series.iter().all(Option::is_none) {
		// Nothing to replay is a refusal: what failed, a row a channel, what to type.
		for line in &notes {
			verbosity::say(verbosity::NOTES, line);
		}
		let channels = plan.channels.len();
		use vag_cli_core::report::Advice as _;
		let mut err = anyhow!(
			"none of the plan's {channels} {} is in {log}: nothing to replay",
			crate::render::plural(channels, "channel")
		);
		if !rows.is_empty() {
			err = err.note(rows.join("\n"));
		}
		return Err(err.help(columns::fix(&gaps, plan)));
	}
	notes.extend(rows);

	let read = series.iter().filter(|s| s.is_some()).count();

	let times = recording.samples.iter().map(|(t, _)| columns::to_ms(*t));
	let (start, end) = (times.clone().min().unwrap_or(0), times.max().unwrap_or(0));
	let at = presses.iter().map(|&press| columns::to_ms(press)).collect();
	let replay = Replay::new(device, series, at, start, end).map_err(|e| anyhow!("{e}"))?;
	let seconds = |ms: u64| ms as f64 / 1000.0;
	// What the person typed and the replay did not do is said at every level; the rest of the
	// notes — what the replay leaves out and why — at `-v`.
	let mut refused = Vec::new();
	for &(ms, why) in replay.refused() {
		refused.push(match why {
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
	let mut said: Vec<(u8, String)> = vec![(
		verbosity::NOTES,
		format!(
			"read {log}: {} {}, {:.2}–{:.2} s; plan {:016x}, {read} of its {} {} in it",
			recording.samples.len(),
			crate::render::plural(recording.samples.len(), "row"),
			seconds(start),
			seconds(end),
			vag_cli_core::dash::plan_id(plan),
			plan.channels.len(),
			crate::render::plural(plan.channels.len(), "channel")
		),
	)];
	said.extend(notes.iter().map(|n| (verbosity::NOTES, n.clone())));
	said.extend(refused.iter().map(|n| (0, n.clone())));
	notes.extend(refused);

	let Some(speed) = panel else {
		say_all(&said);
		let mut events = Vec::new();
		write_log(replay, &mut events)?;
		vag_cli_core::out!("{}", String::from_utf8_lossy(&events));
		return Ok(());
	};
	let title = Path::new(log)
		.file_name()
		.map_or_else(|| log.to_string(), |n| n.to_string_lossy().into_owned());
	let shown = view::show(replay, &notes, &title, speed)?;
	// The screen is gone; what happened stays in the shell.
	say_all(&said);
	for line in shown.events {
		vag_cli_core::outln!("{line}");
	}
	if shown.interrupted {
		return Err(vag_cli_core::interrupt::raise());
	}
	Ok(())
}

/// Each line on stderr at its level.
fn say_all(said: &[(u8, String)]) {
	for (level, line) in said {
		verbosity::say(*level, line);
	}
}

/// What the plan has that the replay does not run, said before anything else
/// (`todo/dash/19`). The lever is pressed on the board by reading the car and a `[[button]]`
/// by a finger on a pin, and a recording holds neither as presses; without them the
/// stopwatch page is never entered. `--press` is the one press the replay has: `dashsim`'s,
/// a page turn.
pub fn unreplayed(plan: &vag_cli_core::dash::Plan) -> Vec<String> {
	let mut notes = Vec::new();
	let inputs: Vec<&str> = [(plan.stalk.is_some(), "[stalk] lever"), (!plan.buttons.is_empty(), "[[button]]s")]
		.into_iter()
		.filter_map(|(has, name)| has.then_some(name))
		.collect();
	if !inputs.is_empty() {
		let verb = if inputs == ["[stalk] lever"] { "is" } else { "are" };
		notes.push(format!(
			"the plan's {} {verb} not replayed — the only press here is --press, a page turn as dashsim gives it",
			inputs.join(" and ")
		));
	}
	if plan.stopwatch.is_some() {
		notes.push("the plan's stopwatch page is not replayed — the lever or a stopwatch button enters it, and neither is replayed".to_string());
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
	use vag_dash_render::alarm::{Alarm, Bound, ChannelId, PageId, Rule};
	use vag_dash_render::plan::{Channel, Page, Plan};

	const fn channel(did: u16, label: &'static str) -> Channel {
		Channel {
			unit: 0,
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
			zero_at_rest: false,
		}
	}

	static CHANNELS: [Channel; 2] = [channel(0x1001, "A"), channel(0x1002, "B")];
	static PAGES: [Page; 2] = [
		Page::Values {
			title: "MAIN",
			cells: &[0],
			volts: None,
		},
		Page::Values {
			title: "HIGH",
			cells: &[1],
			volts: None,
		},
	];
	static RULES: [Alarm; 1] = [Alarm {
		channel: ChannelId(1),
		page: PageId(1),
		rule: Rule::Bounds {
			max: Some(Bound { at: 10.0, off: 8.0 }),
			min: None,
		},
	}];
	static PLAN: Plan = Plan {
		vin: "TESTVIN0000000001",
		language: "en",
		units: &[vag_dash_render::plan::Unit {
			request: 0x7E0,
			response: 0x7E8,
			part_number: "PART",
		}],
		channels: &CHANNELS,
		pages: &PAGES,
		alarms: &RULES,
		stalk: None,
		stopwatch: None,
		buttons: &[],
		standstill: None,
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
			 \x20    1.00 s  alarm #1 took the screen — page 2 \"HIGH\", B (01:1002) = 12 (on at ≥ 10, off at ≤ 8)\n\
			 \x20    1.20 s  press — silences alarm #1\n\
			 \x20    1.20 s  alarm #1 silenced — back to page 1 \"MAIN\"\n"
		);
		// No panel, no terminal control: a script reads it as it is.
		assert!(!text.contains(['▀', '▄', '█', '\x1b']), "{text}");
	}

	/// The log on stdout, the same at every level; a press the replay did not make said by
	/// default; what the replay read and left out at `-v`.
	#[test]
	fn piped_the_log_is_stdout_and_a_press_not_made_is_said() {
		use std::borrow::Cow;
		use vag_cli_core::dash::{Alarm as PlanAlarm, AlarmRule, Bound as PlanBound, Channel as PlanChannel, Page as PlanPage, Plan as Owned, Unit};
		use vag_cli_core::verbosity::{Capture, assert_split};
		use vag_data_labels::catalog::{MeasurementDef, ReadId, Scaling};
		use vag_data_labels::measure::{LinearScale, RawForm};

		let plan = Owned {
			vin: "TESTVIN0000000001".into(),
			language: "en".into(),
			units: vec![Unit {
				request: 0x7E0,
				response: 0x7E8,
				part_number: "PART".into(),
				odx_name: None,
			}],
			channels: vec![PlanChannel {
				unit: 0x7E0,
				did: 0x1001,
				bit_offset: 0,
				bit_length: 16,
				signed: true,
				big_endian: true,
				factor: 1.0,
				offset: 0.0,
				decimals: 0,
				unit_text: String::new(),
				label: "A".into(),
				proven: false,
				hz: 10.0,
				source: String::new(),
				setpoint: None,
				zero_at_rest: false,
			}],
			pages: vec![PlanPage::Values {
				title: "MAIN".into(),
				cells: vec![0],
				volts: false,
			}],
			alarms: vec![PlanAlarm {
				channel: 0,
				page: 0,
				rule: AlarmRule::Bounds {
					max: Some(PlanBound { at: 10.0, off: 8.0 }),
					min: None,
				},
			}],
			stalk: None,
			stopwatch: None,
			buttons: vec![],
			strip: None,
			standstill: None,
		};
		let offered = [crate::plan::Channel {
			request: 0x7E0,
			did: 0x1001,
			def: Some(MeasurementDef {
				name: Cow::Borrowed("A"),
				unit: Cow::Borrowed(""),
				address: ReadId::Uds(0x1001),
				raw_form: RawForm::I16Be,
				scaling: Scaling::Linear(LinearScale { factor: 1.0, offset: 0.0 }),
			}),
			named: None,
			proven: false,
			text_id: None,
			selected: false,
		}];
		let recording = Recording::parse("t_s,A\n0.0,0\n1.0,12\n2.0,0\n3.0,0\n").unwrap();
		let run = |level| {
			let capture = Capture::start(level);
			replay("drive.csv", &recording, &plan, &offered, &[99.0], None).unwrap();
			capture.finish()
		};
		let runs = [run(0), run(1), run(2)];
		assert!(
			runs[0].stdout.starts_with("     0.00 s  start on page 1 \"MAIN\"\n"),
			"{}",
			runs[0].stdout
		);
		assert!(runs[0].stdout.contains("alarm #1 took the screen"), "{}", runs[0].stdout);
		assert_eq!(
			runs[0].stderr,
			"the press at 99.00 s is outside the recording's frames (0.00–3.00 s) — not made\n"
		);
		assert!(
			runs[1].stderr.starts_with(&format!(
				"read drive.csv: 4 rows, 0.00–3.00 s; plan {:016x}, 1 of its 1 channel in it\n",
				vag_cli_core::dash::plan_id(&plan)
			)),
			"{}",
			runs[1].stderr
		);
		assert_split("dev recording dash", &runs);

		// None of the plan's channels in the recording: what failed, a row a channel and what to
		// type; what a missing channel means at `-v` only.
		let other = Recording::parse("t_s,Other\n0.0,1\n1.0,2\n").unwrap();
		let refuse = |level| {
			let capture = Capture::start(level);
			let error = replay("drive.csv", &other, &plan, &offered, &[], None).unwrap_err();
			(
				vag_cli_core::report::render(&error, 0, vag_cli_core::report::Colour::Never),
				capture.finish(),
			)
		};
		let (error, quiet) = refuse(0);
		assert_eq!(
			error,
			"error: none of the plan's 1 channel is in drive.csv: nothing to replay\n\
			 \n\
			 note: missing  A (01:1001)\n\
			 help: record them: `vagcan watch --did \"01:1001\" --out FILE`\n"
		);
		assert_eq!((quiet.stdout.as_str(), quiet.stderr.as_str()), ("", ""));
		let (_, verbose) = refuse(1);
		assert_eq!(verbose.stderr, "missing: not in the recording, so no value and never an alarm\n");
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
			buttons: vec![],
			strip: None,
			standstill: None,
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
		// Buttons on pins are pressed on the board, and a recording holds no press of them.
		use vag_cli_core::dash::Button;
		use vag_dash_render::control::Command;
		plan.buttons = vec![Button {
			pin: 3,
			action: Command::Next,
		}];
		let notes = unreplayed(&plan);
		assert_eq!(notes.len(), 2);
		assert!(
			notes[0].starts_with("the plan's [stalk] lever and [[button]]s are not replayed — the only press here is --press"),
			"{notes:?}"
		);
		plan.stalk = None;
		plan.stopwatch = None;
		assert_eq!(
			unreplayed(&plan),
			["the plan's [[button]]s are not replayed — the only press here is --press, a page turn as dashsim gives it"]
		);
	}
}
