# measure/03 — the saved speed is in m/s, labelled and drawn as km/h

Found by review on `feat/measure-roles-by-id` (2026-09-28). Present on `master` before it.

## What is wrong

- The session keeps the leading speed in m/s (`session.rs` ~118-122: "In m/s, with the speed scale
  already applied"), and `run_json` writes that series as it is (`lib.rs` ~802, `series.speed`);
  `reread.rs` ~42 reads it back as m/s.
- The session file's channel descriptor gives the speed the catalog's unit, `km/h`.
- `measure view` (`view.html`) takes the series as km/h: the axis unit (~600) and
  `crossingTime(run, kmh)` against the marks (~1558, ~1568-1575). (Its distance integral's `/ 3.6`
  (~658) would be wrong too, but runs only for a file with no `distance` series, and `run_json`
  always writes one.)
- So the chart shows the speed 3.6 times too low, and a rolling mark such as 60–100 is never
  crossed, so its band is never drawn.
- Evidence: the owner's 2026-09-26 session (`research/captures/ble-measure.csv`): the `speed`
  series peaks at 6.59 while its descriptor says `km/h`; the `cross_check_speed` series, in km/h,
  peaks at 23.7 over the same run.

## Fix (one of)

1. Write the series in km/h (convert in `run_json`, back in `reread`), and keep m/s inside.
2. Label it m/s in the descriptor and make `view.html` convert.

Either way a test that writes a run and reads it back through the viewer's arithmetic (or the
report) with a known speed.

## Done (2026-10-21) — fix 2: the file says m/s, the page converts

- Why not 1: every session on disk is schema 1 with the speed in m/s under a km/h label.
  Writing km/h under the same label and schema would leave nothing in a file to tell an old one
  from a new one; a new schema would make `measure view` refuse the old ones.
- The file: the leading speed's descriptor says `m/s` (`lib.rs`, `SPEED_SERIES_UNIT`,
  `recorded_unit`). The series is unchanged.
- The page: `measure view` re-derives from the m/s samples, then `view::in_page_units` multiplies
  the series by 3.6 and labels it km/h. It converts whatever the label says, because the label on
  an old file is wrong. `view.html` is unchanged apart from a comment.
- `view_sample.json` had its speed in km/h, which no writer produced. It is now m/s, labelled m/s.
- Tests (`lib.rs`): `the_file_labels_the_leading_speed_in_the_unit_its_series_is_written_in`;
  `the_page_draws_the_speed_in_kmh_and_crosses_the_marks_the_stopwatch_closed` (the page's
  `crossingTime` ported: 100 km/h at the 0–100 time, the 50–100 band ends at 100 km/h, distance
  still from m/s); `a_session_written_before_the_label_was_fixed_draws_the_same`.
- The 2026-09-26 session through `vagcan measure view`: speed peak 6.59 → 23.71 km/h (its km/h
  cross-check: 23.70); 10 km/h first crossed never → 2.185 s, the time the stopwatch closed 0–10.
