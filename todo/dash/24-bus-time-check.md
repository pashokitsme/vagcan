# dash / 24 — a bus-time check for the plan, measured on the car

**Subsystem:** dash · **Crates:** `vag-uds-client` (`schedule::Planner`), `vag-cli-core` (the plan
build) · **Needs the car:** for the one measured constant

**State (2026-10-19):** recorded, not started. Found by the architectural review of 2026-10-19;
the owner approved the direction the same day.

## Why

`feat/dash-thresholds` first replaced `MAX_ALARMS` with `STANDING_READS_PER_S = 75` (the
scheduler's ceiling 100 − the 25 kept for the page). The review showed it is the wrong unit:

- 100 and 25 are owner policy (`schedule/mod.rs` `Budget`), not measurements; 100 is a send
  spacing reached on the bench against `benchecu`, which answers at once.
- On the car the whole bus does 45–48 exchanges a second with the panel loaded
  (`research/dash/can-bring-up.md`, the `measure` section), about 21 ms an exchange.
- The planner batches up to 8 identifiers of one unit into one request (`schedule/mod.rs`
  `max_dids_per_request`); the 10 Hz recording measured 5.3 identifiers an exchange. Counting
  identifiers refuses a plan the car serves (10 alarm channels on one unit) and passes one it
  cannot (8 channels on 8 units at 9 Hz, 72 exchanges a second on a 48/s bus).

So the branch prints the background reads and refuses nothing (owner, 2026-10-19), and this
task is the real check.

## What

- Run the pure, clockless `Planner` offline over the plan for a simulated minute, with every
  page's rates as the board sets them (`Plan::rates_in`), and count the exchanges it issues.
- Cost each at a per-exchange time **measured on the car** — one dated constant with its
  evidence beside it, re-measurable per car (a `watch --out` run gives it).
- Report the bus share in the build output; refuse only when the page on the glass would be
  thinned below its rates.
