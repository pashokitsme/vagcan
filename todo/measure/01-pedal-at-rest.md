# measure/01 — the pedal at rest reads 15 %, and the coastdown waits for 1 %

Found by review on `feat/measure-roles-by-id` (2026-09-28). Present on `master` before it.

## What is wrong

- When the project knows the engine's variant and no pedal is drive-proven elsewhere, `measure`'s
  pedal role resolves to the engine's `F449`: SAE J1979 PID `49`, accelerator pedal position D —
  the sensor's absolute position. VW declares `IDE00086` on the engine at `F449` only, in 381 of
  the reference ODIS project's 385 engine variants.
- On the reference car it reads 15.69 % at rest: the parked survey's `F449` is `0x28`
  (`research/dumps/survey-parked.jsonl` — kept out of git, on the owner's machine), and on
  2026-09-26 it never read lower, standstill
  included (`research/captures/ble-measure.csv`, the pedal was `7E0:F449`).
- `measure setup`'s coastdown opens a pass only when the foot is off, `pedal_pct <= 1.0`
  (`crates/cli/vag-cli-measure/src/coastdown.rs` ~107 and ~667, fed through `setup.rs` ~798/~1065).
- So an owner without a drive-proven relative pedal cannot complete the coastdown. The reference
  car passes only because its gearbox's `3804` is drive-proven (`0CW300041G.json`); the parked
  survey reads it `00` at rest.

## Options (the owner's call)

1. Prefer a relative pedal position where the car has one: ODIS `IDE04005` (relative accelerator
   pedal position, `%`, at `F45A` in 35 variants), SAE J1979 PID `5A`. The reference engine has
   none: its `F440` support bitmap (`FE D0 84 01`) has PID `5A` clear and it does not answer `F45A`.
2. Judge "foot off" against the pedal's own reading at rest, the way kickdown is judged against the
   run's own maximum less one raw step (`report.rs` ~395-403), rather than against 1 %.
3. Both: the relative position where the car has one, the rest-relative check otherwise.
