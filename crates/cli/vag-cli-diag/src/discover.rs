//! `vagcan dev recording discover` — find the identifiers that carry discrete state.
//!
//! Gear, gearbox mode, a turn signal, a brake switch: none of these can be
//! found by fitting a straight line. A gear takes seven values and a switch
//! takes two, and two points define a line exactly — which is precisely how a
//! false "proof" got through once already (`.archive/research/labels/rod-labels.md` §4.3).
//!
//! The right question for discrete state is not *what scale is this* but
//! **which identifier changed when the thing changed**. So this reads a
//! recording made by `vagcan watch --out` and sorts every column by how it
//! behaves: never moved, stepped between a handful of values, or varied
//! continuously. The stepped ones are the candidates, and their transition
//! times say what to compare against.

/// How a recorded identifier behaved over the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Behaviour {
	/// Never changed — carries no information about anything that happened.
	Constant,
	/// Moved between a small set of values: a gear, a mode, a switch, a state.
	Stepped { levels: usize, changes: usize },
	/// Took many values — an analogue quantity, for `vagcan dev vcds analyse` instead.
	Continuous { levels: usize },
}

/// One column of a recording.
#[derive(Debug, Clone)]
pub struct Column {
	pub name: String,
	pub samples: usize,
	pub behaviour: Behaviour,
	/// The distinct values, in first-seen order, for a stepped column.
	pub values: Vec<String>,
	/// Times (seconds into the recording) where the value changed.
	pub transitions: Vec<f64>,
}

impl Column {
	/// Candidates worth investigating: something that moved, but between few
	/// enough values to be a state rather than a measurement.
	pub fn is_candidate(&self) -> bool {
		matches!(self.behaviour, Behaviour::Stepped { changes, .. } if changes > 0)
	}
}

/// Above this many distinct values a column is treated as analogue.
const STEPPED_MAX_LEVELS: usize = 12;

/// Every value column of a `watch --out` recording, with its samples, in the
/// order the header lists them.
///
/// **The one reader of this layout, where there were three.** `classify` below,
/// the since-removed `calibrate`'s `series_of` and
/// `watch::replay::Recording::parse` each grew their own walk over a header this
/// tool writes itself — and `calibrate`'s carried the comment "same layout rule
/// as `discover`", which is a promise prose cannot keep.
///
/// A sample is `(time, cell)`, and the time is the column's **own** when it has
/// one: values on a row are not simultaneous, because identifiers are polled in
/// batches, so the last column can be most of a cycle newer than the first.
/// Older recordings carry one `t_s` for the row and still parse, at the cost of
/// locating a transition only to within a polling cycle.
///
/// A blank cell is dropped rather than recorded. Blank means the unit did not
/// answer that cycle, and a dropped read must not read as a state change.
pub fn series(csv: &str) -> Vec<(String, Vec<(f64, String)>)> {
	let mut lines = csv.lines();
	let Some(header) = lines.next() else { return Vec::new() };
	let names = fields(header);
	let names: Vec<&str> = names.iter().map(String::as_str).collect();
	let columns = value_columns(&names);
	let mut out: Vec<(String, Vec<(f64, String)>)> = columns.iter().map(|(_, _, name)| ((*name).to_string(), Vec::new())).collect();
	for line in lines {
		if line.trim().is_empty() {
			continue;
		}
		let cells = fields(line);
		let Some(Ok(row_t)) = cells.first().map(|c| c.trim().parse::<f64>()) else {
			continue;
		};
		for (slot, (value_at, time_at, _)) in columns.iter().enumerate() {
			let Some(cell) = cells.get(*value_at).map(|c| c.trim()).filter(|c| !c.is_empty()) else {
				continue;
			};
			let t = time_at
				.and_then(|at| cells.get(at))
				.and_then(|c| c.trim().parse::<f64>().ok())
				.unwrap_or(row_t);
			out[slot].1.push((t, cell.to_string()));
		}
	}
	out
}

/// One line of a recording, cut into its fields.
///
/// A field in double quotes may hold commas, and `""` inside it is one quote — the part
/// of RFC 4180 `watch --out` writes, because channel names carry commas ("…, cylinder
/// 1") and a bare one made one heading two. A recording written before the quoting cuts
/// at every comma as it always did — except where a field there begins with `"`: that is
/// read as quoted now, so its commas no longer cut and its quotes are dropped. The old
/// writer quoted nothing, so such a field could only come from a heading that itself
/// began with a quote.
pub fn fields(line: &str) -> Vec<String> {
	let mut out = Vec::new();
	let mut field = String::new();
	let mut quoted = false;
	let mut chars = line.chars().peekable();
	while let Some(c) = chars.next() {
		match (c, quoted) {
			('"', true) if chars.peek() == Some(&'"') => {
				chars.next();
				field.push('"');
			}
			('"', true) => quoted = false,
			('"', false) if field.is_empty() => quoted = true,
			(',', false) => out.push(std::mem::take(&mut field)),
			_ => field.push(c),
		}
	}
	out.push(field);
	out
}

/// A heading as `watch --out` writes it: in quotes when it holds a comma or a quote, as
/// is otherwise. The reverse of [`fields`].
pub fn quoted(field: &str) -> std::borrow::Cow<'_, str> {
	match field.contains([',', '"']) {
		true => format!("\"{}\"", field.replace('"', "\"\"")).into(),
		false => field.into(),
	}
}

/// Where each value column sits in a row, and where its own timestamp sits when
/// it has one: `(value_at, time_at, heading)`.
///
/// The pairing rule, in one place: a heading is a timestamp column when it is
/// `<name>_t_s` and the very next heading is `<name>`. Anything else is a value
/// column that takes the row's time. Public because `watch::replay` needs the
/// same walk and keeps different things from it.
pub fn value_columns<'a>(headings: &[&'a str]) -> Vec<(usize, Option<usize>, &'a str)> {
	let mut out = Vec::new();
	let mut i = 1;
	while i < headings.len() {
		let paired = headings
			.get(i)
			.zip(headings.get(i + 1))
			.is_some_and(|(t, v)| t.strip_suffix("_t_s") == Some(*v));
		match paired {
			true => out.push((i + 1, Some(i), headings[i + 1])),
			false => out.push((i, None, headings[i])),
		}
		i += if paired { 2 } else { 1 };
	}
	out
}

/// Read a `vagcan watch --out` recording and classify every column.
///
/// Two layouts are accepted. Current recordings carry a time column per value
/// (`<name>_t_s,<name>`), because values on one row are **not** simultaneous —
/// identifiers are polled in batches, so the last column can be most of a
/// cycle newer than the first. Older recordings have a single `t_s` and one
/// column per value; they still parse, using the row time for everything, but
/// a transition read from one is only located to within a polling cycle.
pub fn classify(csv: &str) -> Result<Vec<Column>, String> {
	let mut lines = csv.lines();
	let header = lines.next().ok_or("the recording is empty")?;
	let names = fields(header);
	if names.len() < 2 || names[0] != "t_s" {
		return Err("not a `vagcan watch --out` recording (expected a t_s column)".to_string());
	}

	let mut out = Vec::new();
	for (name, samples) in series(csv) {
		// A `_t_s` heading the pairing could not pair is still a timestamp, not
		// a state: a recording written before names were quoted splits
		// `Cruise control, cancel` into ` cancel_t_s` and `Cruise control`, and
		// the timestamp then counted as a candidate with "46 changes".
		if name.ends_with("_t_s") {
			continue;
		}
		let mut values: Vec<String> = Vec::new();
		let mut transitions = Vec::new();
		let mut previous: Option<&String> = None;
		for (t, v) in &samples {
			if !values.contains(v) {
				values.push(v.clone());
			}
			if previous.is_some_and(|p| p != v) {
				transitions.push(*t);
			}
			previous = Some(v);
		}

		let levels = values.len();
		let behaviour = match levels {
			0 | 1 => Behaviour::Constant,
			n if n <= STEPPED_MAX_LEVELS => Behaviour::Stepped {
				levels: n,
				changes: transitions.len(),
			},
			n => Behaviour::Continuous { levels: n },
		};
		out.push(Column {
			name,
			samples: samples.len(),
			behaviour,
			values,
			transitions,
		});
	}
	Ok(out)
}

/// How much two columns change together.
///
/// Discrete state is identified by coincidence in time, not by a scale factor:
/// if the value that means "gear" changes at the same moments as a known gear
/// change, that is the evidence. Returns the fraction of `a`'s transitions
/// that fall within `window` of one of `b`'s.
pub fn transition_overlap(a: &Column, b: &Column, window: f64) -> f64 {
	if a.transitions.is_empty() {
		return 0.0;
	}
	let matched = a
		.transitions
		.iter()
		.filter(|t| b.transitions.iter().any(|u| (*t - u).abs() <= window))
		.count();
	matched as f64 / a.transitions.len() as f64
}

use crate::render::plural;
use vag_cli_core::marks::SEP;
use vag_cli_core::table::{Table, terminal_width};

/// How many of a stepped column's values the table shows before `…`.
const SHOWN_VALUES: usize = 8;

/// The candidates, fewest levels first — a two-state column is a switch, which is the
/// easiest thing to confirm — then most changes first.
fn candidates(columns: &[Column]) -> Vec<&Column> {
	let mut candidates: Vec<&Column> = columns.iter().filter(|c| c.is_candidate()).collect();
	candidates.sort_by_key(|c| match c.behaviour {
		Behaviour::Stepped { levels, changes } => (levels, usize::MAX - changes),
		_ => (usize::MAX, 0),
	});
	candidates
}

/// The classification as data: a table of the candidates — when there are any — and one
/// summary line, `45 columns · 14 candidates · 21 continuous · 10 never moved`.
pub fn render(columns: &[Column]) -> String {
	let candidates = candidates(columns);
	let mut out = String::new();
	if !candidates.is_empty() {
		let mut table = Table::new(&["column", "values", "changes", "reads", "seen"]).right(1).right(2).right(3);
		for c in &candidates {
			let Behaviour::Stepped { levels, changes } = c.behaviour else {
				continue;
			};
			let shown: Vec<String> = c.values.iter().take(SHOWN_VALUES).cloned().collect();
			let more = if c.values.len() > shown.len() {
				format!(" {}", vag_cli_core::marks::ELLIPSIS)
			} else {
				String::new()
			};
			table.row(vec![
				c.name.clone(),
				levels.to_string(),
				changes.to_string(),
				c.samples.to_string(),
				format!("{}{more}", shown.join(" ")),
			]);
		}
		out.push_str(&table.render(terminal_width()));
		out.push('\n');
	}
	let continuous = columns.iter().filter(|c| matches!(c.behaviour, Behaviour::Continuous { .. })).count();
	let constant = columns.iter().filter(|c| c.behaviour == Behaviour::Constant).count();
	out.push_str(&format!(
		"{} {}{SEP}{} {}{SEP}{continuous} continuous{SEP}{constant} never moved\n",
		columns.len(),
		plural(columns.len(), "column"),
		candidates.len(),
		plural(candidates.len(), "candidate"),
	));
	out
}

/// What `discover` says beside its data: at `-v` how it sorts and what an empty answer means,
/// at `-vv` when each candidate changed. `untimed`: how many columns carry no time of their
/// own ([`untimed`]).
pub fn notes(columns: &[Column], untimed: usize) -> Vec<(u8, String)> {
	use vag_cli_core::verbosity::{NOTES, WIRE};
	let candidates = candidates(columns);
	let mut notes = vec![(
		NOTES,
		format!("candidate: changed between 2 and {STEPPED_MAX_LEVELS} values; more is continuous; a blank cell (no answer) is no change"),
	)];
	if untimed > 0 {
		notes.push((
			NOTES,
			format!("columns without their own time: {untimed} (an older recording) — a change there is placed only to within a polling cycle"),
		));
	}
	if candidates.is_empty() {
		notes.push((
			NOTES,
			"no candidate: nothing discrete happened while recording, or the state lives on a unit that was not polled".to_string(),
		));
	}
	for c in candidates {
		let at: Vec<String> = c.transitions.iter().map(|t| format!("{t:.2}")).collect();
		notes.push((WIRE, format!("{}: changed at {} s", c.name, at.join(" "))));
	}
	notes
}

/// How many value columns of a recording carry no time of their own (`<name>_t_s`).
pub fn untimed(csv: &str) -> usize {
	let Some(header) = csv.lines().next() else { return 0 };
	let names = fields(header);
	let names: Vec<&str> = names.iter().map(String::as_str).collect();
	value_columns(&names).iter().filter(|(_, time_at, _)| time_at.is_none()).count()
}

/// The pairs [`co_changing`] found, as data: a table — when there are any — and one line.
pub fn render_pairs(pairs: &[(String, String, f64)]) -> String {
	let mut out = String::new();
	if !pairs.is_empty() {
		let mut table = Table::new(&["first", "second", "coincide"]).right(2);
		for (a, b, overlap) in pairs {
			table.row(vec![a.clone(), b.clone(), format!("{:.0}%", overlap * 100.0)]);
		}
		out.push_str(&table.render(terminal_width()));
		out.push('\n');
	}
	out.push_str(&format!("{} {} changed together\n", pairs.len(), plural(pairs.len(), "pair")));
	out
}

/// How [`co_changing`] pairs, at `-v`: `window` is its argument.
pub fn pairs_note(window: f64) -> String {
	format!(
		"pair: two candidates whose changes fall within {window} s of each other's, {:.0}% or more both ways — probably one thing seen twice",
		PAIR_OVERLAP * 100.0
	)
}

/// The share of transitions two candidates must share, both ways, to be a pair.
const PAIR_OVERLAP: f64 = 0.8;

/// Group candidates that changed at the same moments — they are probably
/// facets of one thing (a gear number and the gear-change flag, say).
pub fn co_changing(columns: &[Column], window: f64) -> Vec<(String, String, f64)> {
	let candidates: Vec<&Column> = columns.iter().filter(|c| c.is_candidate()).collect();
	let mut pairs = Vec::new();
	for (i, a) in candidates.iter().enumerate() {
		for b in candidates.iter().skip(i + 1) {
			let overlap = transition_overlap(a, b, window).min(transition_overlap(b, a, window));
			if overlap >= PAIR_OVERLAP {
				pairs.push((a.name.clone(), b.name.clone(), overlap));
			}
		}
	}
	pairs
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_timestamp_column_left_unpaired_is_not_a_candidate() {
		// Unquoted `Lever, cancel` in an old header: ` cancel_t_s` is followed by
		// `Lever`, not ` cancel`, so it is not paired — and it steps like a state.
		let csv = "t_s,Lever, cancel_t_s,Lever, cancel\n0.0,1.0,0,1.0,0\n0.1,1.1,1,1.1,1\n0.2,1.2,0,1.2,0\n";
		let columns = classify(csv).unwrap();
		assert!(
			columns.iter().all(|c| !c.name.ends_with("_t_s")),
			"{:?}",
			columns.iter().map(|c| &c.name).collect::<Vec<_>>()
		);
	}

	#[test]
	fn a_state_written_as_marked_bytes_is_still_a_state_and_a_missed_read_no_change() {
		// A gear has a definition and no number, so `watch --out` writes its bytes
		// marked `0x`; a read that missed writes an empty cell, which is not a level.
		let csv = "t_s,Gear_t_s,Gear\n0.0,0.0,0x05\n0.1,,\n0.2,0.2,0x06\n0.3,0.3,0x05\n";
		let columns = classify(csv).unwrap();
		assert_eq!(columns[0].samples, 3);
		assert!(
			matches!(columns[0].behaviour, Behaviour::Stepped { levels: 2, changes: 2 }),
			"{:?}",
			columns[0].behaviour
		);
	}

	#[test]
	fn a_state_written_by_name_counts_each_name_as_a_level() {
		// Since 2026-09-26 a state is recorded by the name of its level — quoted
		// when it holds a comma. Each name is one level, however many raw values
		// its band spans, and an unmatched answer written as bytes is one more.
		let csv = "t_s,Lever_t_s,Lever\n\
             0.0,0.0,rest\n\
             0.1,0.1,\"pulled, hard\"\n\
             0.2,0.2,rest\n\
             0.3,0.3,0x05\n\
             0.4,0.4,rest\n";
		let columns = classify(csv).unwrap();
		assert_eq!(columns[0].behaviour, Behaviour::Stepped { levels: 3, changes: 4 });
		assert_eq!(columns[0].values, ["rest", "pulled, hard", "0x05"]);
		assert!(columns[0].is_candidate());
	}

	#[test]
	fn a_heading_with_a_comma_is_quoted_and_read_back_whole() {
		// Channel names carry commas ("Pressure, left"); unquoted, one heading was two.
		assert_eq!(quoted("Pressure"), "Pressure");
		assert_eq!(quoted("Pressure, left"), "\"Pressure, left\"");
		assert_eq!(quoted("say \"hi\""), "\"say \"\"hi\"\"\"");
		assert_eq!(
			fields("t_s,\"Pressure, left_t_s\",\"Pressure, left\",\"say \"\"hi\"\"\",,1"),
			["t_s", "Pressure, left_t_s", "Pressure, left", "say \"hi\"", "", "1"]
		);
		// And every reader of the layout sees one column, paired with its own time.
		let csv = "t_s,\"Pressure, left_t_s\",\"Pressure, left\",Plain\n0.100,0.050,1.5,7\n";
		assert_eq!(
			series(csv),
			[
				("Pressure, left".to_string(), vec![(0.05, "1.5".to_string())]),
				("Plain".to_string(), vec![(0.1, "7".to_string())])
			]
		);
		assert_eq!(classify(csv).unwrap().len(), 2);
	}

	/// A recording shaped like `vagcan watch --out` writes them.
	const RECORDING: &str = "t_s,3805,3806,3807,3808\n\
         0.000,01,0A,7F,\n\
         0.200,01,0B,7F,\n\
         0.400,02,0C,7F,\n\
         0.600,02,0D,7F,\n\
         0.800,03,0E,7F,\n\
         1.000,03,0F,7F,\n";

	#[test]
	fn a_stepping_value_is_separated_from_an_analogue_one() {
		let cols = classify(RECORDING).unwrap();
		assert_eq!(cols.len(), 4);

		// Three levels with two transitions: the shape of a gear.
		assert_eq!(cols[0].name, "3805");
		assert_eq!(cols[0].behaviour, Behaviour::Stepped { levels: 3, changes: 2 });
		assert_eq!(cols[0].transitions, vec![0.4, 0.8]);
		assert!(cols[0].is_candidate());

		// Six levels, changing every sample — analogue, not a state.
		assert_eq!(cols[1].behaviour, Behaviour::Stepped { levels: 6, changes: 5 });

		// Never moved.
		assert_eq!(cols[2].behaviour, Behaviour::Constant);
		assert!(!cols[2].is_candidate());
	}

	#[test]
	fn a_column_the_unit_never_answered_is_not_a_state_change() {
		// Blank cells mean "no answer this cycle". Treating them as a value
		// would invent a transition every time a read was dropped.
		let cols = classify(RECORDING).unwrap();
		assert_eq!(cols[3].name, "3808");
		assert_eq!(cols[3].samples, 0);
		assert_eq!(cols[3].behaviour, Behaviour::Constant);
	}

	#[test]
	fn a_recording_with_per_value_timestamps_uses_them() {
		// The current format. Values on one row are not simultaneous, and a
		// transition must be located by the value's OWN sample time — using
		// the row time would misplace it by most of a polling cycle, which is
		// exactly what defeats co-change analysis.
		let csv = "t_s,A_t_s,A,B_t_s,B\n\
             0.000,0.010,01,0.780,10\n\
             1.000,1.010,02,1.780,10\n\
             2.000,2.010,02,2.780,20\n";
		let cols = classify(csv).unwrap();
		assert_eq!(cols.len(), 2);
		assert_eq!(cols[0].name, "A");
		assert_eq!(cols[0].transitions, vec![1.010], "A's own time, not 1.000");
		assert_eq!(cols[1].name, "B");
		assert_eq!(cols[1].transitions, vec![2.780], "B is sampled late in the cycle");
	}

	#[test]
	fn a_recording_from_before_per_value_timestamps_still_parses() {
		let cols = classify(RECORDING).unwrap();
		assert_eq!(cols.len(), 4);
		assert_eq!(cols[0].transitions, vec![0.4, 0.8]);
	}

	#[test]
	fn columns_that_change_together_are_paired() {
		// Two identifiers that switch at the same moments are facets of one
		// thing; that coincidence is the evidence, not a scale factor.
		let csv = "t_s,A,B,C\n\
             0.0,1,10,5\n\
             1.0,1,10,6\n\
             2.0,2,20,7\n\
             3.0,2,20,8\n";
		let cols = classify(csv).unwrap();
		let pairs = co_changing(&cols, 0.25);
		assert_eq!(pairs.len(), 1, "{pairs:?}");
		assert_eq!((pairs[0].0.as_str(), pairs[0].1.as_str()), ("A", "B"));
	}

	#[test]
	fn transition_overlap_is_directional_and_time_windowed() {
		let csv = "t_s,A,B\n0.0,1,1\n1.0,2,1\n1.1,2,2\n2.0,3,2\n";
		let cols = classify(csv).unwrap();
		// A changes at 1.0 and 2.0; B at 1.1 only.
		assert!((transition_overlap(&cols[1], &cols[0], 0.2) - 1.0).abs() < 1e-9);
		assert!((transition_overlap(&cols[0], &cols[1], 0.2) - 0.5).abs() < 1e-9);
		// Too tight a window matches nothing.
		assert_eq!(transition_overlap(&cols[1], &cols[0], 0.05), 0.0);
	}

	#[test]
	fn a_recording_where_nothing_moved_is_a_summary_alone_and_a_note_says_why() {
		let csv = "t_s,A,B\n0.0,1,7F\n1.0,1,7F\n";
		let columns = classify(csv).unwrap();
		assert_eq!(render(&columns), "2 columns · 0 candidates · 0 continuous · 2 never moved\n");
		let notes = notes(&columns, untimed(csv));
		assert!(
			notes
				.iter()
				.any(|(level, n)| *level == 1 && n.starts_with("no candidate: nothing discrete happened")),
			"{notes:?}"
		);
	}

	#[test]
	fn the_candidates_are_a_table_and_counts_of_one_read_as_one() {
		// A switch that flipped once is 2 values, 1 change of 3 reads, and one such column
		// is "1 candidate" — not "1 candidates".
		let csv = "t_s,A\n0.0,0\n1.0,1\n2.0,1\n";
		assert_eq!(
			render(&classify(csv).unwrap()),
			"column  values  changes  reads  seen\n\
			 A            2        1      3  0 1\n\
			 \n\
			 1 column · 1 candidate · 0 continuous · 0 never moved\n"
		);
	}

	#[test]
	fn a_stepped_column_shows_its_first_values_and_says_there_are_more() {
		let csv = "t_s,A\n0,1\n1,2\n2,3\n3,4\n4,5\n5,6\n6,7\n7,8\n8,9\n";
		let text = render(&classify(csv).unwrap());
		assert!(text.lines().nth(1).unwrap().ends_with("  1 2 3 4 5 6 7 8 …"), "{text}");
	}

	#[test]
	fn pairs_are_a_table_and_a_line_and_none_is_a_line() {
		assert_eq!(render_pairs(&[]), "0 pairs changed together\n");
		assert_eq!(
			render_pairs(&[("A".into(), "Gear".into(), 1.0)]),
			"first  second  coincide\n\
			 A      Gear        100%\n\
			 \n\
			 1 pair changed together\n"
		);
	}

	#[test]
	fn an_older_recording_is_said_to_place_a_change_within_a_cycle() {
		assert_eq!(untimed("t_s,A_t_s,A\n0,0,1\n"), 0);
		assert_eq!(untimed("t_s,A_t_s,A,B\n0,0,1,2\n"), 1);
		let columns = classify("t_s,A\n0,1\n1,2\n").unwrap();
		let said = notes(&columns, 1);
		assert!(said.iter().any(|(_, n)| n.starts_with("columns without their own time: 1 ")), "{said:?}");
		assert!(said.contains(&(2, "A: changed at 1.00 s".to_string())), "{said:?}");
	}

	#[test]
	fn a_file_that_is_not_a_recording_is_rejected() {
		assert!(classify("").is_err());
		assert!(classify("a,b,c\n1,2,3\n").unwrap_err().contains("t_s"));
	}
}
