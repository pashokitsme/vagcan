# measure/02 — every unit's cross-check speed lands in one track

Found by review on `feat/measure-roles-by-id` (2026-09-28). Present on `master` before it.

## What is wrong

- `sample_set` (`crates/cli/vag-cli-measure/src/lib.rs`, the `"cross-check speed" if cross_check_taken`
  arm) means to carry one cross-check speed per batch, because "merging two units' speeds into one
  track would invent a signal neither reported".
- The live loop calls `sample_set` once per arrival (`lib.rs` ~1424-1425), so `cross_check_taken`
  never fires: every cross-check speed goes into the one `cross-check speed` series
  (`session.rs` ~146).
- On the reference car that is six units (`70E`, `767`, `712`, `714`, `7E0`, `746`) interleaved in
  one track. The owner's 2026-09-26 session shows it: 579 samples, whole km/h and fractional values
  interleaved (6.8, 7.0, 7.41, 8.0 within 0.12 s; `research/captures/ble-measure.csv`).
- Nothing reads a refresh period off that track today (`report.rs` ~274 bounds the leading speed
  only), but one would mean nothing.
- Since `feat/measure-roles-by-id` each value is converted to km/h, so the units no longer mix; the
  interleaving is unchanged.

## Options

1. One track per unit: the series named by the unit's request id (a change to the session file's
   series names, and to what reads them — `measure view`, the report).
2. Keep one unit's cross-check: the first by the resolution's order, the others not polled at all
   (less bus time as well).
