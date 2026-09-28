# vagcan — roadmap (detail)

The short roadmap is in [`README.md`](../README.md#roadmap) and is kept current there.
This file is the detail behind it: state, decisions, open items, and what is dead. Older
dated status sections moved verbatim to
[`.archive/tasks/roadmap-history.md`](../.archive/tasks/roadmap-history.md) on 2026-09-14,
2026-09-15, 2026-09-22, 2026-09-26 and 2026-09-27.

## Where things stand (2026-09-28)

**Milestone: a VCDS installation alone gives the car's channels, and nothing asks for a survey —
PRs #15–#18 merged 2026-09-28** (`7d895f8`, `c19799a`, `c4f45fd`, `198f51f`). CI green on
`198f51f`; `cargo test --workspace` 1,997 passed; firmware RAM unchanged (static 139,500 B with
BLE, 130,048 B without). The 2026-09-27 status and the day's first one moved to
[`.archive/tasks/roadmap-history.md`](../.archive/tasks/roadmap-history.md) on 2026-09-28.

- **#15 — scalings from a VCDS installation**
  ([`label-lookup/02`](../.archive/tasks/done/label-lookup/02-vcds-registry.md)): `setup`'s step 5
  reads the channels of a car's units from VCDS's registry `RM.rod`; a proven row outranks ODIS
  and ODIS outranks VCDS, per field. On the reference car: 5,314 channels for 14 of 15 units (the
  BCM has no VCDS file); the owner's `dash.toml` builds from VCDS alone with the same 19 channels
  as from ODIS.
- **#16 — `calibrate` removed** entirely (owner).
- **#17 — `measure` finds its roles by text id**, in the units its consumers read, and ranks a
  drive-proven row and the powertrain first. The reference car's run is timed from the gearbox's
  `F40D`, the channel the board's stopwatch reads (it was the BCM's `2B16`); gear and pedal are
  the gearbox's drive-proven rows; air mass is absent (no row on that engine carries an air-mass
  id; `2037` is what both sources name a setpoint). Three older defects filed as `measure/01`–`03`.
- **#18 — `dev survey` removed**
  ([`label-lookup/03`](../.archive/tasks/done/label-lookup/03-units-without-survey.md)), with
  `faults --from`, `watch --survey` and the blind sweep. `watch`, `measure` and `units --identify`
  record the car's units in `~/.vagcan/cars/<VIN>/units.json` and read their VCDS channels the
  first time; `dev dash build` reads the record offline; the firmware's build refuses a plan unit
  not read yet and names `dev dash build`. The gateway identifies itself too, four reads a run —
  the controller's decision, the owner told ([`label-lookup/04`](label-lookup/04-vcds-follow-ups.md)
  item 1).
- **The owner's data, migrated 2026-09-28:** his car's `units.json` written from the parked
  survey's identities through `units::record` (15 units; the part numbers match the 2026-09-26
  capture), and his `dash.toml`'s `survey =` line removed (the file before:
  `dash.toml.before-units-record-2026-09-28`). On a copy of his data, master's binary builds his
  plan from the record — 19 channels on 4 units, 5 pages, no VCDS read — identical to the plan the
  code before #18 built from the survey.
- **The board in the car** still runs an image from before PR #7: nothing merged since 2026-09-26
  has been flashed. USB on the car enumerates only when plugged in before OBD power
  (`research/dash/can-bring-up.md` §9.16). Boards unchanged since 2026-09-22: the old board on 5 V
  works, the rev v0.4 board is a spare without BLE.

**Not verified on hardware:** everything in #15–#18 on the car (the units record from a live
`watch`, the gateway's reads, `measure`'s gearbox lead over a real run); and still everything in
PR #12 (the lever, the pin buttons, the stopwatch, the run's flash write), the retard alarm at
−6.0 and the blink, over the cable through the board on the car, the moving-car guard, the CANable
on car traffic, the ESC's channels, `dash/17` §2 item 8, `dash/18` on a real pull.

## Decisions (owner)

| decision | designed in |
|---|---|
| Two firmware modes; the host picks slcan with `vagcan --slcan` | `dash/14` §3 |
| Power from OBD pin 1 (ignition); sleep, power budget, wake button dropped | `.archive/specs/dash/` |
| The cruise lever pages the dash only with cruise OFF | `dash/14` §6a |
| Stopwatch on the gearbox's vehicle speed `IDE00075`, already km/h, factor 1 — not the output shaft speed `380B` with a factor measured per car, the 2026-09-13 decision (2026-09-27) | `dash/14` §6 |
| Acceleration on the panel from the ESC's own sensor `IDE03660`, `+` taken as speeding up; nothing derived on the board (2026-09-27) | `dash/14` §6 |
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
| Input backends: `[[button]]`s on GPIO 3–5 (one action each, no long press), the lever and `dashsim` in any mix, one command path; BOOT and RESET are technical, not inputs (owner, 2026-09-27) | `dash/19` |
| Knock retard alarm −6.0/−4.5, from the research (owner, 2026-09-26) | `dash/04` |
| A proven row outranks ODIS, and ODIS outranks VCDS, field by field (owner, 2026-09-28) | `label-lookup/02` (archived) |
| `calibrate` removed entirely — «паразитный, никто этим заморачиваться не будет» (owner, 2026-09-28) | `label-lookup/02` (archived) |
| `dev survey` removed entirely, with `faults --from`, `--diff` and the blind sweep: nobody runs a command just to feed the tool — the commands that meet the car record its units (owner, «б», 2026-09-28) | `label-lookup/03` (archived) |
| `measure` times a run from the gearbox's speed, the board's stopwatch channel, and finds its roles by text id (owner, 2026-09-28) | PR #17 |
| A Russian-only VCDS installation takes names and units from an English one on the same machine (owner, 2026-09-28) — not built | `label-lookup/04` |

## Next, in order

**Without the car**

1. **Flash `master`** — the owner. Everything on it is reviewed and merged; the owner's plan
   builds (above). In `crates/dash/vag-dash-fw`: `VAGCAN_DASH_VIN=XW8AD4NE9JH008917 cargo run
   --release --bin dash` builds and flashes through the `espflash` runner in `.cargo/config.toml`
   (it passes the partition table). Since #18 the plan is built from the car's `units.json`
   (checked on a copy of the owner's data, 2026-09-28). The image carries the lever, the stopwatch and the fault count;
   BOOT no longer pages.
2. **Runs in flash** — [`dash/21`](dash/21-runs-in-flash.md): how many runs, and when LIMIT
   writes; the owner set both aside on 2026-09-27.
3. **OLED and enclosure** — `dash/15`; waits for the panel.
4. **`measure view`'s speed** — [`measure/03`](measure/03-speed-series-unit.md): the saved speed
   series is m/s drawn as km/h, so the chart is 3.6 times low and a rolling mark is never drawn —
   visible on the owner's own 2026-09-26 session. Then [`measure/02`](measure/02-cross-check-tracks.md)
   (every unit's cross-check speed in one track) and [`measure/01`](measure/01-pedal-at-rest.md)
   (the pedal at rest; the owner's call).
5. **The VCDS follow-ups** — [`label-lookup/04`](label-lookup/04-vcds-follow-ups.md): three for
   the owner to decide (the gateway's reads, a plain list's first row, a witness), then the
   platform file's choice and the Russian fallback.
6. **Guard `sensors --ecu <unit>`** — it reads 32 standard OBD-II identifiers from any unit with
   no moving-car check (found in review, 2026-09-28; it predates that change). A command that
   resembles a sweep and is unguarded gets the guard (`CLAUDE.md`, Safety).

**With the car**

7. **`dash/19` on the car** — [`dash/19`](dash/19-stalk-and-stopwatch.md) "On the car", first
   of all: pressing and releasing + and − never opens the stopwatch or turns the page back (a
   release from 91 or 128 to 205 crosses the other bands; at 20 Hz two reads are 100 ms). Then
   paging with cruise off, cruise on closing the stopwatch, a 0–100 beside `vagcan measure --ble`
   started first (both time the gearbox's speed), and the run saved before `GO` surviving a
   power cycle. Pin buttons on the bench first.
8. **The fault count on the car** — [`dash/20`](dash/20-fault-count.md) "Car checks": the
   total against `vagcan faults`, the log's time, the units skipped, a BLE `info` during it.
9. **Alarms on a drive** — `dash/04`: the retard at −6.0 and the blink, the misfire window; a
   `watch --out` recording with `200A`–`200D` for the replay.
10. **The rest of `dash/17` §4** — through the board over the cable (USB before OBD power), the
   moving-car guard (`bleuds`), the CANable on car traffic, the ESC's channels (DRIVE's
   acceleration: `+` should read as speeding up); §2 item 8.
11. **A specified value on a real pull** — `dash/18` §6: boost's difference through a pull,
   then the owner's `percent`, `hold_ms` and `min_setpoint`.
12. **Faults without VCDS, live** — `vagcan faults` after an ODIS-only `setup` (on 2026-09-26 it
   ran with the `.rod` fallback beside the project); then freeze-frame layouts
   (`MCD_DB_ENV_DATA_DESC`) for `faults --details`.
13. **Questions only the car answers** — `dash/06`.
14. **Reverse-gear code** — `catalog.rs` says `0C`, ODIS says reverse is `7`. Select
    reverse, read `0x210F` on `7E0` and `0x3816` on `7E1`.
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
| [`dash/19-stalk-and-stopwatch.md`](dash/19-stalk-and-stopwatch.md) | the lever and `[[button]]` pins as input, the stopwatch page — merged (PR #12, 2026-09-27); in the owner's `dash.toml`; needs the car |
| [`dash/20-fault-count.md`](dash/20-fault-count.md) | the car's stored codes counted once after boot, a triangle and the count in the corner, `?` when there is no count — built 2026-09-27; needs the car |
| [`dash/21-runs-in-flash.md`](dash/21-runs-in-flash.md) | stopwatch runs in flash, read over BLE, saved on LIMIT; recorded 2026-09-27, open questions for the owner |
| [`label-lookup/04-vcds-follow-ups.md`](label-lookup/04-vcds-follow-ups.md) | what `label-lookup/02` and `03` left open — filed 2026-09-28; three items for the owner to decide |
| [`measure/01-pedal-at-rest.md`](measure/01-pedal-at-rest.md) | the engine's absolute pedal reads 15 % at rest and the coastdown waits for 1 % — found in review 2026-09-28; the owner's call |
| [`measure/02-cross-check-tracks.md`](measure/02-cross-check-tracks.md) | every unit's cross-check speed lands in one track — found in review 2026-09-28; open |
| [`measure/03-speed-series-unit.md`](measure/03-speed-series-unit.md) | the saved speed is in m/s, labelled and drawn as km/h: `measure view` shows it 3.6 times low and never draws a rolling mark — found in review 2026-09-28; open |

Finished task files are in `.archive/tasks/done/` (`dash/16`, UDS over BLE, moved there on
2026-09-15 — its car check is `dash/17` §4; `label-lookup/02` and `03`, merged as PRs #15 and
#18, moved there on 2026-09-28); superseded designs in `.archive/specs/`.

## Command names in older documents

`research/` and `.archive/` keep commands as they were typed on the day. Today:

```
setup devices info units faults sensors watch measure dev
dev: sniff glossary recording dash vcds
```

| older spelling | today |
|---|---|
| `vagcan survey`, `vagcan dev survey` | gone (2026-09-28) — the car's units are recorded by `watch`, `measure` and `units --identify`; `--diff` and the blind sweep went with it |
| `vagcan calibrate`, `vagcan dev recording calibrate` | gone (2026-09-28) |
| `vagcan sniff` | `vagcan dev sniff` |
| `vagcan vcds …`, `vagcan labels` | `vagcan dev vcds …` |
| `vagcan recording …` | `vagcan dev recording …` |
| `vagcan dash build` | `vagcan dev dash build` |
| `vagcan scan` | gone — one unit's sweep over any range; today `units --identify <unit>` reads that unit's identification block, `F100`–`F1FF`, and nothing else |
| `vagcan properties` | gone — it is `units --identify <unit>`, and now carries the moving-car guard |

Every command the skills under `.claude/skills/` name was run against `--help` on
2026-09-28 and resolves; `vagcan dev survey` and `faults --from` do not parse.

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
- **Scaling from the *VCDS* label files** — ~~refuted structurally, twice over~~ **OVERTURNED
  2026-09-28.** VCDS keeps every measurement's `(DID, layout, scaling)` in the global registry
  `RM.rod`; a unit's `MWB` lists 1-based row numbers into it. The old refutation
  (`.archive/research/labels/rod-labels.md` §4.0c, `label-linkage.md` §3, `scaling-audit.md`) read
  those row numbers as text-ids and concluded the join was absent. Found by using an ODIS project
  as a crib; verified on the car (gearbox 12/12). Implemented and merged the same
  day (PR #15): `setup`'s step 5 reads it — see
  [`research/vcds-registry/README.md`](../research/vcds-registry/README.md). ODIS
  is still the shipped route, and remains the route for the ~41% of files in the shifted-IV regime.
- **OBD-II Mode 01 as the product path** — dropped. The standard sensors survive as
  `vagcan sensors` and as reference readings, not as the measurement model.
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
  where it would live; since 2026-09-28 it holds `units.json` — each identified unit's request
  id and identity, not the unit number it answers for.

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

