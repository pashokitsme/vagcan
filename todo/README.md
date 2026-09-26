# vagcan — roadmap (detail)

The short roadmap is in [`README.md`](../README.md#roadmap) and is kept current there.
This file is the detail behind it: state, decisions, open items, and what is dead. Older
dated status sections moved verbatim to
[`.archive/tasks/roadmap-history.md`](../.archive/tasks/roadmap-history.md) on 2026-09-14,
2026-09-15 and 2026-09-22.

## Where things stand (2026-09-26)

**Milestone: the board reads the car over BLE, parked, and the panel runs on it.** The
2026-09-22 status moved to [`.archive/tasks/roadmap-history.md`](../.archive/tasks/roadmap-history.md)
on 2026-09-26. Car record: [`dash/17`](dash/17-bench-ble-usb.md) §4, `research/captures/`.

- **The car through the board over BLE, 2026-09-26** (old board, rev v1.1, the owner's plan; all
  13 channels answer at boot): `info`, `faults` (18 units, 9 stored codes, ~50 s), `watch --hz
  10` (rows 100 ms apart, p90 110 ms) and `units` (15 units) pass; `measure` works, its run
  stopped for want of road. The panel on the car through `dashsim`: works (owner).
- **USB on the car:** the cable enumerates only when plugged in **before** OBD power; with the
  board already powered the Mac sees nothing — likely `VBUS` back-fed from the car's 5 V
  (`research/dash/can-bring-up.md` §9.16). Order: USB first.
- **Alarms on the car, 2026-09-26:** knock retard went beyond −2.6° on most full-throttle pulls
  on 95 RON, so −2.0/−1.5 fires on every pull. The owner picked −4.0/−3.0 pending research; the research
  (forum and tuner logs of stock EA888, this engine among them; no OEM number) puts 3–4° at WOT
  in the normal band; **−6.0/−4.5 in the owner's `dash.toml` and plan since 2026-09-26** (the
  file before: `dash.toml.before-retard-6`). Not on the board yet: flashed only when the owner
  says. A sustained-retard rule and a part-load gate would fit the sources better than a
  threshold — not built. **The cell blinks** since 2026-09-26 (PR #7, `cad671b`): 400/400 ms
  from the takeover while out, steady through the hold.
- **The cruise lever, probed 2026-09-26** (`dash/14` §6a): with cruise off the rocker moves
  `70C` `1105` byte 8 and the engine ignores it; OFF is latched, CANCEL springs back. The lever
  as buttons (+ next, − previous, LIMIT the stopwatch) and the stopwatch page: spec in
  [`dash/19`](dash/19-stalk-and-stopwatch.md), approved 2026-09-26; phase 1 (pure logic)
  reviewed, phase 2 (plan, firmware) in progress on `feat/stalk-stopwatch`. LIMIT is the
  "neutral ohne Limiterverbau" state despite its ODIS name: the owner pressed LIMIT in the
  capture, and the state appears only on those presses (0.4 and 0.6 s), never at rest.
- **PR #6 merged 2026-09-26** (`2855b5c`): `vagcan dev recording dash` replays a `watch --out`
  recording on the panel in the terminal. It changed `watch --out`: a heading with a comma is
  quoted, an unconverted answer is `0x…`, a missed read is its time with no value.
- **`--ble`** is short for `--device ble` on every command that takes `--device` (merged
  2026-09-26).
- **PR #9 merged 2026-09-26** (`4486567`): ODIS text tables keep their intervals, so `watch`
  names a lever state instead of printing hex — `1105`'s readings never equal a table value.
  **A project cache made before it holds only each state's lower end**: `watch` says to re-run
  `vagcan setup`; `dash/19`'s plan build refuses such a cache. The owner's `SK37X` cache is one
  (checked 2026-09-26).
- **PR #8 merged 2026-09-26** (`c186a44`): `crates/dash/vag-dash-fw/ram-budget.sh`, run by CI,
  fails when the firmware's statics pass a ceiling or its stack falls under a floor; the
  host's canvases hold a pixel in a bit.
- **Fault count on the panel** (`dash/20`, a number and a triangle in a corner, read once
  ≥10 s after start): phase 1 (`vag_uds_client::faultcount`) reviewed on `feat/fault-count`,
  not merged; phase 2 wires it into the firmware after `dash/19`.
- **Fresh-eyes pass, 2026-09-26:** three reviewers over the day's merged work, the two open
  branches and the plans. Fixed on `fix/sanity-2026-09-26`: the replay took old hex `1E05` as
  100000 and kept digit-only hex past its field's width; a VW-block request past `0x795` got a
  response id past `0x7FF`, which no 11-bit frame carries (now no address, and `faults` says
  so); the blink after the adapter screen started from a plain half. Open questions for the
  owner are in "Next".
- **Boards:** unchanged since 2026-09-22 — the old board on 5 V is the working one, the rev v0.4
  board a spare without BLE. An agent's BLE tools still start from Terminal.app: macOS gives
  the owner's Bluetooth grant to the Claude app, not to the agent's processes (`dash/17`).

**Not verified on hardware:** over the cable through the board on the car (`info`, `watch`),
the moving-car guard, the CANable on car traffic, the ESC's channels, `dash/17` §2 item 8,
`dash/18` on a real pull, and all of `dash/19`.

## Decisions (owner)

| decision | designed in |
|---|---|
| Two firmware modes; the host picks slcan with `vagcan --slcan` | `dash/14` §3 |
| Power from OBD pin 1 (ignition); sleep, power budget, wake button dropped | `.archive/specs/dash/` |
| The cruise lever pages the dash only with cruise OFF | `dash/14` §6a |
| Stopwatch on gearbox output shaft speed `380B`, km/h factor measured per car | `dash/14` §6 |
| UDS over BLE: slow single reads, board always visible and guarding itself | `dash/16` |
| No `frame` mirror; no separate link crate | `dash/14` §7 |
| Enclosure redesigned, `flat` layout, snap-in boards; CAD in `~/CAD/projects/vagcan/` | `dash/15` |
| M3 (whole-car measurement coverage by survey) off the list (2026-09-10) | — |
| The scheduler is subscriptions with drop semantics, one layer on the board and the laptop; rates only from `hz` in `dash.toml` (2026-09-14) — except the lever's and the stopwatch's, fixed in code by what a press and a launch need (2026-09-26) | `dash/14` §2, `dash/19` |
| BLE with no pairing and no button, and only when asked (`--device ble`, no automatic scan); `watch` and `measure` over BLE (2026-09-14) | `dash/16` |
| Link icons in a column at the right edge; the chart ends before it, connected or not; no icons on the adapter screen so far (2026-09-14) | `PR #3` |
| BLE on the rev v0.4 board is not pursued; the board is a spare, run without BLE (2026-09-22) | `research/dash/ble-controller-hang.md` |
| `measure` over BLE at 34–38 speed reads a second, under the cable's 45–48, is accepted (2026-09-22) | `research/dash/can-bring-up.md` §9.15 |
| The bench board's transceiver stays on 5 V: its 3V3 trace is broken, and the board is expendable (2026-09-22). The risk is `RXD` at 5 V into `GPIO1`, not the car's bus; if the board acts up in the car, unplug it | `research/dash/can-bring-up.md` §9.12 |
| A channel's specified value written by hand as `setpoint`, never guessed from names; the panel shows the difference, not the value; a drift alarm is a percentage, a hold and a floor (2026-09-14/15) | `dash/18` |
| A recording's cell off the plan's scaling drops the column in the replay; no guessing at old bare hex (2026-09-26) | `dash/04` |
| The alarm highlight blinks while the value is out, steady in the hold, the cell only (2026-09-26) | `dash/04` |
| Cruise lever with cruise off: RES/+ next page, SET/− previous, LIMIT the stopwatch; lever and stopwatch page as one feature (2026-09-26) | `dash/19` |
| Knock retard alarm −6.0/−4.5, from the research (owner, 2026-09-26) | `dash/04` |

## Next, in order

**Without the car**

1. **`fix/sanity-2026-09-26`** — the fresh-eyes fixes above; a PR for the owner.
2. **The owner re-runs `vagcan setup <ODIS project>`** — the cache predates PR #9; without it
   `watch` prints the lever as hex and `dash/19`'s plan build refuses.
3. **Flash `master` now or with `dash/19`** — the owner's call. `master` carries −6.0/−4.5 and
   the blink; the board in the car still has the threshold that fires on every pull. Flashing
   in stages keeps a fault on the car attributable to one feature.
4. **The lever as buttons and the stopwatch page** — [`dash/19`](dash/19-stalk-and-stopwatch.md),
   approved 2026-09-26; phase 2 in progress. Built and tested without the car; the speed
   factor and a run need it.
5. **The fault count, phase 2** — `dash/20` on `feat/fault-count`, after `dash/19`. Open, for the
   owner: the board decodes the gateway's list at run time and addresses units no plan holds,
   against "the board resolves nothing" (`dash/README.md`) — keep it and amend the rule, or put
   the unit list in the plan at build time; `MAX_UNITS = 40` comes from this car alone, the
   BLE guard allows 64; a unit answering `78` holds the one link up to 10 s, freezing the
   panel and the alarms, so the count needs its own short deadline and must wait while the
   stopwatch is up; a badge counted once shows no age, and hidden-at-0 looks like not counted.
6. **OLED and enclosure** — `dash/15`; waits for the panel.

**With the car**

7. **The rest of `dash/17` §4** — through the board over the cable (USB before OBD power), the
   moving-car guard (`bleuds`), the CANable on car traffic, the ESC's channels; §2 item 8.
8. **Alarms on a drive** — `dash/04`: the retard threshold from the research, the misfire
   window; a `watch --out` recording with `200A`–`200D` for the replay.
9. **A specified value on a real pull** — `dash/18` §6: boost's difference through a pull,
   then the owner's `percent`, `hold_ms` and `min_setpoint`.
10. **`dash/19` on the car** — LIMIT's state, paging with cruise off, the `380B` factor on a
   steady stretch, a 0–100 run beside `vagcan measure --ble` on `380B` — through the board,
   never with a second adapter on the port while the board polls.
11. **Faults without VCDS, live** — `vagcan faults` after an ODIS-only `setup` (on 2026-09-26 it
   ran with the `.rod` fallback beside the project); then freeze-frame layouts
   (`MCD_DB_ENV_DATA_DESC`) for `faults --details`.
12. **Questions only the car answers** — `dash/06`.
13. **Reverse-gear code** — `catalog.rs` says `0C`, ODIS says reverse is `7`. Select
    reverse, read `0x210F` on `7E0` and `0x3816` on `7E1`.
14. **Sweep witness constants** — `WITNESS_EVERY = 64`, `QUIET_RUN = 3` are reasoned, not
    measured. One parked whole-car run.
15. **`watch` and `measure` across all fifteen units** — measured against the file, not
    the car.

## Task files

| file | state |
|---|---|
| [`dash/04-alarms.md`](dash/04-alarms.md) | on `master`; four rules in the owner's `dash.toml`; replay since PR #6; blink since PR #7; retard at −6.0/−4.5 in the plan, not on the board yet (2026-09-26) |
| [`dash/06-car-and-bench.md`](dash/06-car-and-bench.md) | open questions for the car |
| [`dash/13-screens.md`](dash/13-screens.md) | channel menu for pages |
| [`dash/14-one-bus-three-clients.md`](dash/14-one-bus-three-clients.md) | design; §7 is the dash work order; §6a has the lever probe (2026-09-26) |
| [`dash/15-enclosure.md`](dash/15-enclosure.md) | enclosure hand-off |
| [`dash/17-bench-ble-usb.md`](dash/17-bench-ble-usb.md) | bench passed except §2 item 8; §4 on the car: BLE `info`, `faults`, `watch`, `units`, `measure` pass (2026-09-26), the cable and the guard open |
| [`dash/18-setpoints-and-drift.md`](dash/18-setpoints-and-drift.md) | specified vs actual channels, and the drift alarm — merged (PR #4, 2026-09-15); the owner's `dash.toml` pairs boost; car pending |
| [`dash/19-stalk-and-stopwatch.md`](dash/19-stalk-and-stopwatch.md) | the cruise lever as buttons and the stopwatch page; approved 2026-09-26, phase 1 reviewed, phase 2 in progress on `feat/stalk-stopwatch` |

Finished task files are in `.archive/tasks/done/` (`dash/16`, UDS over BLE, moved there on
2026-09-15 — its car check is `dash/17` §4); superseded designs in `.archive/specs/`.

## Command names in older documents

`research/` and `.archive/` keep commands as they were typed on the day. Today:

```
setup devices info units faults sensors watch measure dev
dev: survey sniff glossary recording dash vcds
```

| older spelling | today |
|---|---|
| `vagcan survey` | `vagcan dev survey` (`--diff BEFORE AFTER` is the parked-vs-driving compare) |
| `vagcan sniff` | `vagcan dev sniff` |
| `vagcan vcds …`, `vagcan labels` | `vagcan dev vcds …` |
| `vagcan recording …` | `vagcan dev recording …` |
| `vagcan dash build` | `vagcan dev dash build` |
| `vagcan scan` | gone — it was a strict subset of `dev survey --only` |
| `vagcan properties` | gone — it is `units --identify <unit>`, and now carries the moving-car guard |

Every command the skills under `.claude/skills/` name was run against `--help` on
2026-09-15 and resolves.

## Dead and archived (kept as negative results — do not retry)

- **Measurement names from a masked (`shifted`) text table — 2026-08-06.** Not slow:
  out of reach. Such files XOR an 8-byte mask over the finished IV, and that mask is a
  **runtime global inside VCDS** — read off the binary at `0x140033b70`, and confirmed
  from the outside by 348 distinct values across 349 files matching nothing structural.
  The files do not carry it, so no amount of analysis recovers it. Because the mask is
  8 bytes it reaches `IV[3..8]`, which costs both the free deflate anchor and the
  multiplicative reduction of the candidate sets: 60 anchors against the full 2⁴⁰ space,
  ~960× the work, hours per file. Measured corpus-wide: the unmasked half is ~39 h of
  CPU, the masked half ~5.2 years. **The Russian build's `TTText-RUS.rod` is masked**,
  so that build gives fault text and labels but no measurement names, and `vagcan setup`
  now says so up front instead of spinning. The only route that would change this is
  lifting the mask out of a running VCDS process, which is a Windows-debugger job and
  not an offline one (`.archive/research/labels/tttext2.md` §3.3a, §3.5).

- **HEX-clone live UDS** — the session KDF is VMProtect-sealed and dead. The `vag-hex`
  crate and the vendored FTDI D2XX driver are **deleted**; the research writeups moved to
  `.archive/research/` (`vag-hex-framing.md`, `clone-crypto.md`, `vcds-rus-crack.md`) and
  stay authoritative as negative results. The clone capture decoder
  (`.archive/research/clb-crack/extract_uds.py`) stays useful as an offline crib source.
- **Scaling from the *VCDS* label files** — refuted structurally, twice over
  (`.archive/research/labels/rod-labels.md` §4.0c, `.archive/research/labels/label-linkage.md` §3/§5).
  **Still true, and no longer the whole story (2026-08-08):** the refutation is about
  what a `.rod`/`.clb` label file contains, not about files in general. A VW ODIS
  project declares the entire chain — identifier, offset, length, byte order, compu
  formula — per ECU variant, and three rows this project had proved *by driving* came
  back identical from it with no drive. Read this entry as "VCDS cannot supply a
  scaling", never as "a scaling can only come from a drive".
- **OBD-II Mode 01 as the product path** — dropped. The standard sensors survive as
  `vagcan sensors` and as calibration references, not as the measurement model.
- **`MUX.rod` as the measurement registry** — opened 2026-08-04 and it is not one. It is
  the ODX multiplexer table, a leaf of the `STRUC` subgraph a car cannot enter, with no
  read identifier by four independent tests and a median table of three rows.
  `.archive/research/labels/mux.md`. No decoder ships: the only way in is a `STRUC` id and nothing a
  control unit reports yields one.
- **Pooling the `RD.rod` digit substitution across tables** — refuted 2026-08-05, then
  made irrelevant. 95 solved tables have 95 distinct alphabets, so there was nothing to
  intersect; the alphabet turned out to be *generated* from the table key by
  `srand(key)` and two shuffles, read off `VCDS-ARM.exe`
  (`.archive/research/labels/fault-naming-hop.md`).

### Open, and bounded

- **`TTTEXT2.ROD`** is the whole of `.archive/research/labels/label-linkage.md` §7 item 3 — whether the
  `.rod` label files are names-and-lists-only. It is a **bounded sweep, measured at ~21 h**
  (`.archive/research/labels/tttext2.md` §12, 2026-08-06): its `[CMP]` section is exempt from the shifted-IV regime, so its
  anchor byte cannot be narrowed and all 60 legal values need the full space
  (`.archive/research/labels/tttext2.md` §4.2). Two of the 60 anchors were run, both misses; the rest are not started.
- **A per-car cache of learned unit pairings.** Which CAN request id answers a unit number
  is in no label file — the two numberings are unrelated — so it is learned per car by
  `units --identify` and lost when the process exits. `~/.vagcan/cars/<VIN>/` is
  where it would live.

## Parked (designed, not being implemented now)
- **The car picks its project** (parked by the owner, 2026-09-21, until a second ODIS
  project is at hand) — `project::covering()` returns `None`, and the resolution order around
  it is built and tested. Which of a car's part numbers identifies its platform (`5E0` × 3
  against the MQB-shared `8V0`, `5Q0`, `3Q0`, `0CW`) needs a second project's `.vi` pool to
  check against; with one project installed the sole-project step already picks it.
- **The BLE stack out of the firmware's build** (2026-09-14) — `vag-dash-fw`'s `build.rs`
  generates the plan through `vag-cli-core`, which now depends on `vag-dash-ble`
  (btleplug), so CI's firmware job installs libdbus. Cleaner: `vag-dash-ble` behind a
  default `ble` feature of `vag-cli-core` (the BLE branches of `device.rs`), with the
  firmware's build-dependency on `default-features = false`.
- **Cross-platform `no_std` core + `vag-runtime-*`** — spec + M1 plan retired with
  `docs/superpowers/` in `2e4721b`. Below-the-seam refactor.

