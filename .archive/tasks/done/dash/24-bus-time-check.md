# dash / 24 — the plan's bus time, estimated; the board measures the real number

**Subsystem:** dash · **Crates:** `vag-uds-client` (`schedule::Planner`), `vag-cli-core` (the plan
build), `vag-dash-fw` (`bus_rate`), `vag-dash-cfg` · **Needs the car:** for the measured constant

**State (2026-10-20):** built on `feat/dash-bus-time`, not merged. Found by the architectural
review of 2026-10-19; the owner approved the direction the same day. The first build refused a
plan the bus would read slower than its rates; it refused every page of the owner's plan, which
runs on the car, so the constant is not the car's CAN time and the check became an estimate that
refuses nothing (coordinator's decision, 2026-10-20). **A strict refusal comes after a car
measurement from the board's `exch=`.**

## Done

- `vag-cli-core/src/dash/bus_time.rs` runs the board's `Planner` (`Budget::board()`, no change to
  `schedule`) over the plan offline: 10 s of warm-up, then a minute counted; per page on the glass
  one run with the lever's gate open (cruise off, the lever at 20/s) and, with a `[stalk]`, one
  with it closed (cruise on, 2/s); one stopwatch run (the lever at its 10/s whatever the gate).
  Subscriptions as the firmware's `PanelReads::follow_pages` makes them (`Page::reads`,
  `Plan::rates_in`), a unit's starting one exchange after the one before (its part check). Every
  exchange answered after one fixed time.
- `dash::bus_notes` prints, never refuses:
  `bus: an estimate at 34 ms an exchange, an upper bound from a BLE recording on the reference car; it refuses nothing; the lever is read 20 times a second with cruise off, 2 with it on`,
  then one line per page — `bus: page N "T" cruise off ~P % — reads X r/s of h, …; cruise on ~Q %
  — reads …` — and one for a stopwatch run. Each run names its own reads: a read that gets fewer
  readings than its rate asks, more than the window's two ends explain (2 in the minute). The
  pages behind the glass, thinned first by design, are said in brackets: "(the other pages less
  often than once a second)". `Error::Bus` is gone: the firmware build, `dev recording dash` and the editor's
  catalog take the owner's plan again.
- **The constant: 34 ms an exchange, an upper bound.** From `research/captures/ble-10hz.csv`
  (the car, 2026-09-26, `watch --device ble --hz 10`, 140 s): readings of one exchange share the
  board's answer stamp — 21,918 readings under 4,139 stamps, 29.5 exchanges a second, 5.3
  identifiers an exchange; no channel got its 10/s, so the planner always had a request waiting:
  1 s / 29.5 = 34 ms. The radio and the host are in it. The 45–48/s of `can-bring-up.md` §9.15
  is `measure`'s speed channel alone against `benchecu` on the bench. The same file has a
  single-identifier exchange at 20 ms (median) and a full batch at 32 ms.
- **The board counts its own exchanges** (`vag-dash-fw/src/bus_rate.rs`, pure, tested by
  `research/dash/host/tests/bus_rate.rs`): every exchange the panel's bus task ends — its unit,
  sent and ended times, answered or not — in windows of 10 s. A closed window goes to the USB
  console, `bus: 29.5 exchanges/s, 29.4 answered in 34.0 ms each, 100 % busy — 7E0 19.6/s, …`
  (six units by name, the rest as `others`), and into `state` as ` exch=29.4/s exch_ms=34.0`
  (` exch=- exch_ms=-` before the first window, and while the board is an adapter). `dashcfg`
  shows it as its `bus` line. RAM: +56 B of statics with BLE (`BUS_RATE`, the last window),
  +0 B without; the state line's longest is 234 B of 244.
- Tests: ten alarm channels on one unit at 10 Hz are served (20 exchanges a second); eight on
  eight units at 9 Hz are thinned (72); ten units at 2.9 Hz served at 99 %, at 3 Hz thinned; the
  pages behind the glass thinned; a run reads its speed ahead of the pages; the build's lines for
  a plain plan, three channels at 50 Hz, the lever with a stopwatch, six alarm channels at 50 Hz
  beside the lever (each cruise run with its own reads), and a plan shaped like the owner's
  (eight pages, the strip, the lever, six alarms), which builds.

## The owner's plan at 34 ms (2026-10-20, built from a scratch copy of `~/.vagcan`)

6 pages, 23 channels, 4 units; it builds.

```
bus: an estimate at 34 ms an exchange, an upper bound from a BLE recording on the reference car; it refuses nothing; the lever is read 20 times a second with cruise off, 2 with it on
bus: page 1 "DRIVE" cruise off ~100 % — reads 02:IDE00075 SPEED 7.7/s of 20, 01:IDE00405 ENGINE RPM 7.3/s of 10, 713:IDE03660 ACCELERATION 6.7/s of 10, the lever 7.7/s of 20; cruise on ~100 % — reads 02:IDE00075 SPEED 8.5/s of 20, 01:IDE00405 ENGINE RPM 9.4/s of 10, 713:IDE03660 ACCELERATION 9.4/s of 10
bus: page 2 "TEMPERATURES" cruise off ~93 % — reads the lever 17.4/s of 20; cruise on ~41 %
bus: page 3 cruise off ~100 % — reads the lever 17.3/s of 20 (the other pages less often than once a second); cruise on ~51 %
bus: page 4 "IGNITION TIMING ANGLE" cruise off ~93 % — reads the lever 17.4/s of 20; cruise on ~41 %
bus: page 5 "MISFIRE" cruise off ~93 % — reads the lever 17.4/s of 20; cruise on ~41 %
bus: page 6 "BRAKES" cruise off ~93 % — reads the lever 17.4/s of 20; cruise on ~41 %
bus: a stopwatch run ~100 % — reads 02:IDE00075 SPEED 10.2/s of 20 (the other pages less often than once a second)
```

## Open

- **A drive with the board's `exch=`**: `exch_ms=` on DRIVE with cruise off is the per-exchange
  time of this car's units, the board alone on the bus. It replaces `EXCHANGE_MS`; then a strict
  refusal (the page on the glass, an alarm, the lever, a run's speed read slower than its rate)
  can come back, against that number.
- A cost by batch size (one identifier against a batch of 8) is what the 2026-09-26 file supports
  better than one number; the console line's per-unit rates beside `exch_ms` give it per car.

## Why

`feat/dash-thresholds` first replaced `MAX_ALARMS` with `STANDING_READS_PER_S = 75` (the
scheduler's ceiling 100 − the 25 kept for the page). The review showed it is the wrong unit:

- 100 and 25 are owner policy (`schedule/mod.rs` `Budget`), not measurements; 100 is a send
  spacing reached on the bench against `benchecu`, which answers at once.
- The planner batches up to 8 identifiers of one unit into one request (`schedule/mod.rs`
  `max_dids_per_request`); the 10 Hz recording measured 5.3 identifiers an exchange. Counting
  identifiers refuses a plan the bus serves (10 alarm channels on one unit) and passes one it
  cannot (8 channels on 8 units at 9 Hz, 72 exchanges a second).

## What

- Run the pure, clockless `Planner` offline over the plan for a simulated minute, with every
  page's rates as the board sets them (`Plan::rates_in`), and count the exchanges it issues.
- Cost each at a per-exchange time — one dated constant with its evidence beside it, measured on
  the car by the board itself.
- Report the bus share in the build output; refuse only once the constant is the car's.
