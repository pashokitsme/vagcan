# dash / 26 — "the car stands" as one fact of the plan

**Subsystem:** dash · **Crates:** `vag-dash-render` (`plan.rs`), `vag-cli-core` (the plan
build), `vag-dash-fw` · **Needs the car:** to pick another source

**State (2026-10-19):** done, branch `refactor/dash-plan-joins` (with `dash/27`). Recorded from
the architectural review of 2026-10-19 (owner approved). Waits for the car: whether a better
source than the speed answers — the check at the end.

## Why

Whether the car stood was known only through the stopwatch's speed (`StopwatchPlan.speed`):
`zero_at_rest` (PR #23) was refused without a `[stopwatch]`, and `Plan::shown` looked the speed
up per cell per frame. The BLE guard has its own speed read (`guard.rs`, `22 F40D` on `7E0`),
which is protocol and stays as it is.

## What was built

- **One fact of the plan.** `vag_dash_render::plan::Plan.standstill: Option<Standstill>`, with one
  kind today, `Standstill::Speed(channel)`: the car stands while that channel reads exactly 0.
  `Plan::stands(value_of) -> Option<bool>` decides it **once per frame** on the board and in the
  replay; `Plan::shown(index, value, stands)` takes the answer, so no cell looks a speed up.
  `None` (no source, or no current answer) is no claim: a `zero_at_rest` cell shows its value.
- **The stopwatch and `zero_at_rest` share the test.** `plan::stands_at(speed)` (exactly 0) is
  what `Standstill::Speed` decides by and what `Stopwatch::sample` arms on. The stopwatch keeps
  arming on its own speed samples, at the sample's own time: its standstill, launch and marks
  are one series, and a standstill from another unit at another rate would be a different moment
  in the launch fit.
- **The source is read with what needs it.** A `zero_at_rest` channel pulls the standstill's
  channel into the reads exactly as a channel pulls its specified value (`Plan::rates_in`,
  `explains`): at its own rate with the cell on the glass, in the background with it on a page
  not shown — so a page with the acceleration and no speed still knows when the car stands. The
  replay's column matching counts it as read (`columns.rs`, `unused`).
- **`zero_at_rest` needs a standstill source, not a `[stopwatch]`.** The plan build sets
  `standstill` from `[stopwatch] speed` — the one source today — and refuses `zero_at_rest` in a
  plan without one: "needs a source for whether the car stands — today a [stopwatch], whose speed
  reading 0 says it".
- **`dash.toml`: no new key.** The plan takes `[stopwatch]`'s `speed`. A later source is an
  optional `[standstill]` table (an enumerated state and the state that means "stands"); a file
  without one keeps the stopwatch's speed, so no file of today changes. `strict.rs` refuses a
  `[standstill]` now, as any unknown section.
- **`plan.json`:** `"standstill": {"kind": "speed", "channel": N}` — a later kind is another
  `kind`. A `plan.json` written before it takes its stopwatch's speed, which is what its
  `zero_at_rest` asked; the generated `plan.rs` says `standstill: Some(Standstill::Speed(N))`.
- The owner's plan (2026-10-19, built offline from a copy of `~/.vagcan`): `standstill` is the
  gearbox's vehicle speed `02:IDE00075`, channel 5 — the DRIVE page's `ACCELERATION` is zeroed by
  it, as before.

## Candidates for another source (catalog of the reference car, 2026-10-19)

| channel | what | answered | note |
|---|---|---|---|
| the gearbox's vehicle speed `02:IDE00075` (`F40D`) | speed = 0 | yes, proven | the source today; already read at 20 Hz for the stopwatch |
| `70A:IDE03684` (`25E0`, parking aid) | `Vehicle_Standstill_Detection`, a state | unknown — outside the August survey's pages | the car's own standstill verdict, both ways (stands / moves); an enumeration, which the plan build refuses as a channel today; adds a unit to poll |
| `01:IDE06569` (`414B`, engine) | `Frequency_counter_vehicle_standstill_detection` | unknown | a counter: it says a stop happened when it goes up, not when the car moves off, and a statistics counter may be written late (owner asked about it, 2026-10-19) |
| `01:IDE06610` (`4164`, engine) | the same, counting stops with the engine off (start-stop) | unknown | start-stop only |

## Waits for the car

`vagcan watch --did 70A:25E0,01:414B` while stopping and moving off, against the gearbox's speed
at 0: whether they answer, and how many milliseconds after the wheels stop each one changes. Only
a source that answers and changes no later than the speed is worth a `[standstill]` table and a
`Standstill` kind; the parking aid's state would also need the plan build to take an enumerated
field outside `[stalk]`.
