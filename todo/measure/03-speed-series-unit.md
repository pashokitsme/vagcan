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
