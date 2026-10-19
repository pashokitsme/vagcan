# dash / 26 — "the car stands" as one fact of the plan

**Subsystem:** dash · **Crates:** `vag-dash-render` (`plan.rs`), `vag-cli-core` (the plan
build), `vag-dash-fw` · **Needs the car:** to pick the source

**State (2026-10-19):** recorded, not started. Architectural review of 2026-10-19; owner approved.

## Why

Whether the car stands is known only through the stopwatch's speed (`StopwatchPlan.speed`):
`zero_at_rest` (PR #23) is refused without a `[stopwatch]`, and `Plan::shown` looks the speed
up per cell per frame. The BLE guard has its own speed read (`guard.rs`, `22 F40D` on `7E0`),
which is protocol and stays.

## What

A plan-level source of "the car stands", decided once per frame, used by the stopwatch,
`zero_at_rest` and anything later that needs it — not a feature of the stopwatch.

## Candidates for the source (catalog of the reference car, 2026-10-19)

| channel | what | answered | note |
|---|---|---|---|
| the gearbox's vehicle speed `02:IDE00075` (`F40D`) | speed = 0 | yes, proven | what `zero_at_rest` uses today; already read at 20 Hz for the stopwatch |
| `70A:IDE03684` (`25E0`, parking aid) | `Vehicle_Standstill_Detection`, a state | unknown — outside the August survey's pages | the car's own standstill verdict, both ways (stands / moves); an enumeration, which the plan build refuses as a channel today; adds a unit to poll |
| `01:IDE06569` (`414B`, engine) | `Frequency_counter_vehicle_standstill_detection` | unknown | a counter: it says a stop happened when it goes up, not when the car moves off, and a statistics counter may be written late (owner asked about it, 2026-10-19) |
| `01:IDE06610` (`4164`, engine) | the same, counting stops with the engine off (start-stop) | unknown | start-stop only |

First on the car: `vagcan watch --did 70A:25E0,01:414B` while stopping and moving off —
whether they answer, and how many milliseconds after the wheels stop each one changes.
