//! `vagcan discover` — find the identifiers that carry discrete state.
//!
//! Gear, gearbox mode, a turn signal, a brake switch: none of these can be
//! found by fitting a straight line. A gear takes seven values and a switch
//! takes two, and two points define a line exactly — which is precisely how a
//! false "proof" got through once already (`research/rod-labels.md` §4.3).
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
    /// Took many values — an analogue quantity, for `vagcan analyse` instead.
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
    let names: Vec<&str> = header.split(',').collect();
    if names.len() < 2 || names[0] != "t_s" {
        return Err("not a `vagcan watch --out` recording (expected a t_s column)".to_string());
    }

    // Value columns, and where each one's own timestamp lives.
    let mut value_columns: Vec<(usize, Option<usize>, &str)> = Vec::new();
    let mut i = 1;
    while i < names.len() {
        let paired = names
            .get(i)
            .zip(names.get(i + 1))
            .is_some_and(|(t, v)| t.strip_suffix("_t_s") == Some(*v));
        if paired {
            value_columns.push((i + 1, Some(i), names[i + 1]));
            i += 2;
        } else {
            value_columns.push((i, None, names[i]));
            i += 1;
        }
    }

    let mut series: Vec<Vec<(f64, String)>> = vec![Vec::new(); value_columns.len()];
    for line in lines {
        if line.trim().is_empty() {
            continue;
        }
        let cells: Vec<&str> = line.split(',').collect();
        let Some(Ok(row_t)) = cells.first().map(|c| c.trim().parse::<f64>()) else {
            continue;
        };
        for (slot, (value_at, time_at, _)) in value_columns.iter().enumerate() {
            let Some(cell) = cells.get(*value_at).map(|c| c.trim()) else {
                continue;
            };
            // A blank cell means the unit did not answer that cycle. Skipping
            // keeps a dropped read from looking like a state change.
            if cell.is_empty() {
                continue;
            }
            let t = time_at
                .and_then(|at| cells.get(at))
                .and_then(|c| c.trim().parse::<f64>().ok())
                .unwrap_or(row_t);
            series[slot].push((t, cell.to_string()));
        }
    }

    let mut out = Vec::new();
    for (slot, (_, _, name)) in value_columns.iter().enumerate() {
        let samples = &series[slot];
        let mut values: Vec<String> = Vec::new();
        let mut transitions = Vec::new();
        let mut previous: Option<&String> = None;
        for (t, v) in samples {
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
            n if n <= STEPPED_MAX_LEVELS => {
                Behaviour::Stepped { levels: n, changes: transitions.len() }
            }
            n => Behaviour::Continuous { levels: n },
        };
        out.push(Column {
            name: (*name).to_string(),
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

/// Render the classification for a human.
pub fn render(columns: &[Column]) -> String {
    let mut out = String::new();
    let mut candidates: Vec<&Column> = columns.iter().filter(|c| c.is_candidate()).collect();
    // Fewest levels first: a two-state column is a switch, which is the
    // easiest thing to confirm.
    candidates.sort_by_key(|c| match c.behaviour {
        Behaviour::Stepped { levels, changes } => (levels, usize::MAX - changes),
        _ => (usize::MAX, 0),
    });

    if candidates.is_empty() {
        out.push_str("No identifier changed between a small set of values.\n\n");
        out.push_str(
            "Either nothing discrete happened while recording, or the state lives on a \
             control unit that was not polled.\n",
        );
    } else {
        out.push_str("Discrete-state candidates — changed, but between few values:\n\n");
        for c in &candidates {
            let Behaviour::Stepped { levels, changes } = c.behaviour else {
                continue;
            };
            let shown: Vec<String> = c.values.iter().take(8).cloned().collect();
            let more = if c.values.len() > shown.len() { " …" } else { "" };
            out.push_str(&format!(
                "  {:<10} {levels:>2} values, {changes:>4} changes of {:>5} reads   [{}{more}]\n",
                c.name,
                c.samples,
                shown.join(" "),
            ));
        }
    }

    let continuous = columns
        .iter()
        .filter(|c| matches!(c.behaviour, Behaviour::Continuous { .. }))
        .count();
    let constant = columns
        .iter()
        .filter(|c| c.behaviour == Behaviour::Constant)
        .count();
    out.push_str(&format!(
        "\n{} columns: {} candidates, {continuous} continuous (use `calibrate`), {constant} never moved\n",
        columns.len(),
        candidates.len(),
    ));
    out
}

/// Group candidates that changed at the same moments — they are probably
/// facets of one thing (a gear number and the gear-change flag, say).
pub fn co_changing(columns: &[Column], window: f64) -> Vec<(String, String, f64)> {
    let candidates: Vec<&Column> = columns.iter().filter(|c| c.is_candidate()).collect();
    let mut pairs = Vec::new();
    for (i, a) in candidates.iter().enumerate() {
        for b in candidates.iter().skip(i + 1) {
            let overlap = transition_overlap(a, b, window).min(transition_overlap(b, a, window));
            if overlap >= 0.8 {
                pairs.push((a.name.clone(), b.name.clone(), overlap));
            }
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_recording_where_nothing_moved_says_so_instead_of_listing_nothing() {
        let csv = "t_s,A,B\n0.0,1,7F\n1.0,1,7F\n";
        let text = render(&classify(csv).unwrap());
        assert!(text.contains("No identifier changed"), "{text}");
        assert!(text.contains("2 never moved"), "{text}");
    }

    #[test]
    fn a_file_that_is_not_a_recording_is_rejected() {
        assert!(classify("").is_err());
        assert!(classify("a,b,c\n1,2,3\n").unwrap_err().contains("t_s"));
    }
}
