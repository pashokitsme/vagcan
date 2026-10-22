# vagcan — roadmap (detail)

The short roadmap is in [`README.md`](../README.md#roadmap) and is kept current there.
This file is the detail behind it: state, decisions, open items, and what is dead. Older
dated status sections moved verbatim to
[`.archive/tasks/roadmap-history.md`](../.archive/tasks/roadmap-history.md) on 2026-09-14,
2026-09-15, 2026-09-22, 2026-09-26, 2026-09-27, 2026-09-28, 2026-09-30, 2026-10-19 and 2026-10-21.

## Where things stand (2026-10-21)

**Milestone: one source per run — PR #56 merged 2026-10-21** (`f58678c`). An ODIS project and a
VCDS installation are independent sources; a run reads one of them for everything it names
(`CLAUDE.md`, "ODIS and VCDS are independent sources"). The 2026-10-19 status moved to
[`.archive/tasks/roadmap-history.md`](../.archive/tasks/roadmap-history.md) on 2026-10-21.
Merged since it, 2026-10-19 to 2026-10-21: #31 (`451e3b5`, `vagcan dev dash edit`), #32 (`a4c7600`,
the pre-project data layout dropped), #33 (`1069daf`, the bus-time estimate), #34 (`0ad5502`,
`vag-dash-core`), #35 (`14b87c9`), #36 (`85990ff`, English fault text by default) and #37–#56
below. `just check` green on `f58678c` on 2026-10-21 (2,568 tests passed across its recipes, 0 failed; firmware stack 17,164 B over its floor with BLE, 20,560 B without).

- **One source per run** (PR #56): `--source`, then `VAGCAN_SOURCE`, then `[source] use` in
  `config.toml`, else ODIS when the project has one, else VCDS (`vag_cli_core::source`). The
  owner picked VCDS on 2026-10-21 — English fault text — so `~/.vagcan/config.toml`
  has `[source] use = "vcds"` (the file before it beside it, `.before-source-vcds-2026-10-21`).
  Master's `dev dash build XW8AD4NE9JH008917` on it: plan `46df4dd19f9cc72c`, 6 pages, 23 channels
  on 4 units, 12 alarms (the ODIS plan was `296f9269b1da6778`). The dash's `language` is `"en"`
  since 2026-10-21 (`dash.toml.before-english-2026-10-21`).
- **One VCDS installation per machine** (PR #50, `5022b3c`): `~/.vagcan/vcds/`, read for every car;
  `data/<project>/` holds an ODIS project alone, and its `measurements/`. The reference machine
  migrated itself on 2026-10-21 (the whole tree before it in `~/.vagcan-backup-before-vcds-store-2026-10-21`,
  the `.rod` files left out); `data/SK37X/measurements/` holds the three proven part files.
- **What a VCDS installation gives** (2026-10-20): units with no label file of their own, the BCM
  among them, through `ReDir.rod` and `chassis.clb` (PR #39, `adaa60c`; a car joins the stem its
  own lookup stored, PR #43, `8639bce`); the `.rod` section keys from the file name, so a cold
  `setup` takes about 3 s, was 845 s measured (PR #40, `091b0b6`); the adaptation, basic-setting,
  output-test, freeze-frame, XPL and MUX registries in the cache (PR #42, `10ac072`;
  `registry.json`'s `READ_VERSION` 3). `setup --list` and `--drop` over the cache's `source` table,
  `sources.json` gone (PR #37, `6526c0a`).
- **Built, not on the car yet:** freeze frames under each stored code in `faults`, from either
  source alone (PRs #47, `8fba00e`, and #53, `bedd5d6`; `research/odis-dtc/freeze-frames.md`);
  `vagcan adaptations <unit>` and `--all`, read only and guarded as a sweep (PR #51, `5b78481`).
- **`sensors --unit <unit>` guarded** (2026-10-21, PR #44, Next item 6 dropped): off the emissions
  block (`7E0`–`7E7`) its read is a sweep and is guarded as `units --identify <unit>` is — one
  named unit, the fixed `obd::PIDS` list, refused on a moving car with "no answer" counted as
  moving, stopped by `anomaly::Monitor`, never through the dash board but as `--slcan`. Plain
  `sensors` (the engine) and the rest of the block are not a sweep: J1979 legislates those
  identifiers there, and the moving-car check is one of them; the monitor stops them too
  (`car::sensors`). No `--while-driving`. Not on the car yet.
- **Output in the shapes of established tools** (PRs #38, #50, #52, #55): `CLAUDE.md` "Command
  output" has the rule and an example of every shape; errors are cargo's `error:`/`help:`/`note:`;
  the ` · ` separator and the marks live in `vag_cli_core::marks`. Ctrl-C ends every command
  (PR #48, `1fffcc8`; `research/tools/sigint.py`), and no timer panics on exit (PR #54, `73a2f1b`).
- **A virtual car** (PR #49, `9b78620`): `vag-uds-sim` on a pseudo-terminal, `vagcan dev sim`,
  and trycmd cases of the commands against it under `tests/e2e/` (`just e2e`,
  [`docs/virtual-car.md`](../docs/virtual-car.md)). No case reads the reference car's data.
- **`vagcan-measure` removed** (PR #46, `a308d3d`): `vagcan measure` is the one way in.
- **The dash** (PR #41, `f004549`, 2026-10-20): a refused or blank `F187`, or one late answer, no
  longer strikes a unit off for the boot. That is what left the ESC's acceleration at `––` on the
  owner's board from 2026-10-20 on (the plan's ESC channel unchanged across plans).
- **The board in the car** runs `dash` without BLE from `f58678c` (`master`, PR #56), flashed by
  the owner on 2026-10-21 at 20:13 UTC from `.worktrees/flash` (detached at `f58678c`, clean):
  the VCDS plan (`channels from the VCDS installation` in the build), 6 pages, the English
  labels, #41's fix, #33's exchange count, #34's core. Before it, from 2026-10-19 21:21 UTC, an
  image of `451e3b5` (#31), dated from the session's log. Its glass is wired SDIN `GPIO8`, CS
  `GPIO20`, RES `GPIO21` (`365c21e`), the standard since 2026-10-03.
- **The housing** ([`dash/15`](dash/15-enclosure.md), CAD in `~/CAD/projects/vagcan/`): round 3
  printed by the owner; then a 0.4 mm rim in the wall in front of the socket, the ESP's clip
  0.5 mm further off and the face's teeth 0.7 mm (CAD `9554fc9`, `44c19c7`, 2026-10-03), not
  printed. The owner took the enclosure off the README roadmap on 2026-10-19.
- **README roadmap ticks by the owner (2026-10-19):** the laptop through the dash (BLE), the
  drift alarm, `faults` with ODIS text only, scalings from a VCDS install alone, the 0–100
  stopwatch, the fault count — done. Their task files keep their open car checks
  ([`dash/18`](dash/18-setpoints-and-drift.md), [`dash/20`](dash/20-fault-count.md),
  [`dash/19`](dash/19-stalk-and-stopwatch.md)) until the owner says they are closed.

**Not verified on hardware** (2026-10-21): the `f58678c` image on a drive (the ESC fix, the English
pages, the VCDS plan, `exch=`); the strip and PR #23 on the glass; the pin
buttons; `+`/`−` never opening the stopwatch; a 0–100 against `vagcan measure`; the run saved
before `GO`; the retard alarm and the blink on a drive; `adaptations`, freeze frames and the
`sensors --unit` guard; Ctrl-C on a real adapter; the cable through the board on the car; the
moving-car guard; the CANable on car traffic; `dash/17` §2 item 8; whether the gearbox's clutch
temperatures (`02:102A`–`102C`) answer — outside the August survey's pages.

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
| Link icons and the fault badge at the status strip's right end, on every values and chart page and the stopwatch's; the page under the strip never changes with them; none on the adapter screen (2026-10-19; a column over the page at the right edge from 2026-09-14) | `PR #3`, PR #28 |
| BLE on the rev v0.4 board is not pursued; the board is a spare, run without BLE (2026-09-22) | `research/dash/ble-controller-hang.md` |
| `measure` over BLE at 34–38 speed reads a second, under the cable's 45–48, is accepted (2026-09-22) | `research/dash/can-bring-up.md` §9.15 |
| The bench board's transceiver stays on 5 V: its 3V3 trace is broken, and the board is expendable (2026-09-22). The risk is `RXD` at 5 V into `GPIO1`, not the car's bus; if the board acts up in the car, unplug it | `research/dash/can-bring-up.md` §9.12 |
| A channel's specified value written by hand as `setpoint`, never guessed from names; the panel shows the difference, not the value; a drift alarm is a percentage, a hold and a floor (2026-09-14/15) | `dash/18` |
| A recording's cell off the plan's scaling drops the column in the replay; no guessing at old bare hex (2026-09-26) | `dash/04` |
| The alarm highlight blinks while the value is out, steady in the hold, the cell only (2026-09-26) | `dash/04` |
| Cruise lever with cruise off: RES/+ next page, SET/− previous, LIMIT the stopwatch; lever and stopwatch page as one feature (2026-09-26) | `dash/19` |
| Input backends: `[[button]]`s on GPIO 3–5 (one action each, no long press), the lever and `dashsim` in any mix, one command path; BOOT and RESET are technical, not inputs (owner, 2026-09-27) | `dash/19` |
| Knock retard alarm −6.0/−4.5, from the research (owner, 2026-09-26) | `dash/04` |
| An alarm is a channel's minimum and maximum with an "off" value each, not a rule of its own; `[[alarm]]` removed; priority by `alarm_priority`, then the file's order; no bus-budget refusal — the build only says what is read (owner, 2026-10-19) | `dash/04` |
| ~~A proven row outranks ODIS, and ODIS outranks VCDS, field by field (owner, 2026-09-28)~~ — superseded 2026-10-21: ODIS and VCDS are independent, one source per run, never mixed (`CLAUDE.md`) | `label-lookup/02` (archived) |
| One source per run, picked by `--source`, then `VAGCAN_SOURCE`, then `[source] use` in `config.toml`, else ODIS when the project has one, else VCDS; a source asked for and not set up is an error; built 2026-10-21. Unit names and numbering follow the rule (decided 2026-10-21, built 2026-10-22): an ODIS run names a unit by the logical link that reaches its base variant and numbers it by the project's table of logical links by diagnostic address; a VCDS run by the label file its part number resolves to; a unit the run's source does not name goes by its request id | `vag_cli_core::source`, `vag_cli_core::unit_names` |
| `calibrate` removed entirely — «паразитный, никто этим заморачиваться не будет» (owner, 2026-09-28) | `label-lookup/02` (archived) |
| `dev survey` removed entirely, with `faults --from`, `--diff` and the blind sweep: nobody runs a command just to feed the tool — the commands that meet the car record its units (owner, «б», 2026-09-28) | `label-lookup/03` (archived) |
| `measure` times a run from the gearbox's speed, the board's stopwatch channel, and finds its roles by text id (owner, 2026-09-28) | PR #17 |
| A Russian-only VCDS installation takes names and units from an English one on the same machine (owner, 2026-09-28) — not built; since #50 a machine keeps one installation, so this needs a second one beside it, the owner's call (2026-10-21) | `label-lookup/04` |
| The glass's pins follow how the board is wired: SDIN `GPIO8` (the blue LED's pin; the LED no longer blinks on its own), CS `GPIO20`, RES `GPIO21` (owner, 2026-09-30) | `README.md` "OLED wiring", `dash/22` |
| Scan clock `B3 F1`; burn-in shift set aside, burn-in will not come quickly (owner, 2026-09-30) | `dash/22` |
| `oledtest`'s clock sweep and the `oledpins` multimeter probe removed from PR #21 before the merge (owner, 2026-09-30); `oledtest`'s test picture stays | `dash/22` |
| A status strip on every page: the voltage at the left, the page's title in the middle, no rule; the voltage per page, never twice on one page (owner, 2026-10-19) | PR #24 |
| An alarm is a channel's threshold — a max and/or a min, each with the value it goes off at — and no cap on how many; what limits them is bus time, measured, not a count ([`dash/24`](../.archive/tasks/done/dash/24-bus-time-check.md)); priority explicit, `alarm_priority` (owner, 2026-10-19) | PR #26 |
| Oil alarm 115/110, the gearbox's control module 120/110, as researched (owner, 2026-10-19) | `research/dash/alarm-thresholds.md` |
| The enclosure off the README roadmap (owner, 2026-10-19); the CAD work goes on in `~/CAD` | `dash/15` |
| One VCDS installation per machine, read for every car, in `~/.vagcan/vcds/`; a project is an ODIS project alone (owner, «делай», 2026-10-21) | PR #50 |
| Every command's output in the shape of `rustc`/`cargo`/`git`/`gh`, picked from mockups: lists as `gh pr list`, one object as `gh pr view`, `faults` grouped by unit, `setup` as `git` reports, errors as cargo's, ` · ` from one constant (owner, 2026-10-21) | `CLAUDE.md` "Command output" |
| Adaptations are their own command, `vagcan adaptations [<unit>]`: a picker without a unit, `--all` for every unit (owner, 2026-10-21) | PR #51 |
| e2e cases are input → output, as `cargo` and `rustc` test themselves (trycmd); no case reads one owner's car, and the board's protocol is not emulated (owner, 2026-10-21) | PR #49 |
| `vagcan-measure` removed; `vagcan measure` is the one way in (owner, «убери», 2026-10-20) | PR #46 |

## Next, in order

**Without the car**

1. ~~**Flash the board with `master`**~~ — done 2026-10-21, `f58678c` (above).
2. ~~**Unit names from the run's source**~~ — done 2026-10-22: `units`, `faults --unit`,
   `sensors --unit`, `units --identify <unit>`, `adaptations` and `watch` name and number units
   from the run's one source (`vag_cli_core::unit_names`); `setup` caches an ODIS project's
   (`unit_name`, schema 6). On the reference project: 54 logical links name all 717 variants,
   47 of them numbered; `info` and the dash build were never named from a source.
3. **The housing** — [`dash/15`](dash/15-enclosure.md): print the 2026-10-03 changes (the
   socket's rim, the ESP's clip, the face's teeth).
4. **Runs in flash** — [`dash/21`](dash/21-runs-in-flash.md): how many runs, and when LIMIT
   writes; the owner set both aside on 2026-09-27.
5. **`measure`'s cross-check speeds** — [`measure/02`](measure/02-cross-check-tracks.md): every
   unit's cross-check speed lands in one track. Two fixes on file: a track per unit, or one unit's
   cross-check and the rest not polled; the second changes what is read on the car, so it is the
   owner's call. Then [`measure/01`](measure/01-pedal-at-rest.md) (the pedal at rest; the owner's
   call).
6. **The VCDS follow-ups** — [`label-lookup/04`](label-lookup/04-vcds-follow-ups.md): three for
   the owner to decide (the gateway's reads, a plain list's first row, a witness), and the Russian
   fallback, which since #50 needs a second installation beside the machine's one (2026-10-21).
7. **Which way up the glass hangs** — [`dash/22`](dash/22-oled.md): `turned`, with the housing.

**With the car**

8. **The `f58678c` image on the glass** (flashed 2026-10-21) — the ESC's acceleration back (no `––` the whole drive),
   the English pages, the strip, the stopwatch's rounding and the acceleration's zero at rest; then
   `dashcfg` → `state` after a drive: `exch=…/s`, the car's own exchange rate, which the bus-time
   estimate's 34 ms waits for ([`dash/24`](../.archive/tasks/done/dash/24-bus-time-check.md)).
9. **`vagcan adaptations` on the car** (built 2026-10-21): a unit's adaptation channels and
   values, read with `0x22` — ODIS's `ReadDataByIdentCalibData` rows (`adaptation_channel`,
   schema 5; an ODIS project read before needs `vagcan setup <dir>` again), or in a run of VCDS
   its `RA.rod` rows, never mixed. Check against VCDS's own adaptation screen on two units (the BCM's
   2,426 fields, the brake unit's 45), parked; then `--all`. Guarded as a sweep: one unit (or
   `--all`, each unit on its own), refused moving, stopped on silence, the part number re-read
   as a witness, cable only.
10. **Freeze frames on the car** — the reading is built (2026-10-21, either source alone,
    `research/odis-dtc/freeze-frames.md`): `vagcan faults -v` beside a VCDS auto-scan, one
    code's frame field by field, once with `--source odis` and once with `--source vcds`.
11. **The ESC's acceleration** — [`dash/17`](dash/17-bench-ble-usb.md) §4: if 8 still shows a
    dash, the board's console on the car (USB plugged in before OBD power) says whether `713` is
    polled and what `1822` answers; then `vagcan watch --did 713:1822` parked.
12. **Where "the car stands" comes from** — [`dash/26`](../.archive/tasks/done/dash/26-car-standing.md)
    (built: one fact of the plan, today the stopwatch's speed at 0):
    `vagcan watch --did 70A:25E0,01:414B` stopping and moving off (the parking aid's standstill
    state, the engine's stop counter), against the gearbox's speed at 0 — whether either is worth
    a `[standstill]` source.
13. **The gearbox's clutch temperatures** — `vagcan watch --did 02:102A,102B,102C` at idle,
    then after a few launches: whether they answer (outside the August survey's pages) and move;
    if so, on a page with 250/200 ([`research/dash/alarm-thresholds.md`](../research/dash/alarm-thresholds.md)).
14. **The rest of `dash/19` on the car** — [`dash/19`](dash/19-stalk-and-stopwatch.md) "On the car"
    items 4–6: `+` and `−` never open the stopwatch or turn the page back; a 0–100 beside
    `vagcan measure --ble` started first; the run saved before `GO` surviving a power cycle.
    Pin buttons on the bench first.
15. **Alarms on a drive** — `dash/04`: the retard at −6.0 and the blink, the misfire window; a
    `watch --out` recording with `200A`–`200D` for the replay.
16. **The rest of `dash/17` §4** — through the board over the cable (USB before OBD power), the
    moving-car guard (`bleuds`), the CANable on car traffic, the ESC's wheel speeds; §2 item 8.
17. **Questions only the car answers** — `dash/06`.
18. **Reverse-gear code** — `catalog.rs` says `0C`, ODIS says reverse is `7`. Select
    reverse, read `0x210F` on `7E0` and `0x3816` on `7E1`.
19. **`watch` and `measure` across all fifteen units** — measured against the file, not
    the car.
20. **Consumption** — `vagcan watch --did 710:2A09,2A1C`, engine off and running, for the signs
    and the factors [`dash/23`](dash/23-power-consumption.md) needs before watts go in the strip.

## Task files

| file | state |
|---|---|
| [`dash/04-alarms.md`](dash/04-alarms.md) | alarms as keys of a channel (max, min, off values, drift, `alarm_priority`), `[[alarm]]` and the cap of four removed — merged (PR #26, 2026-10-19); the owner's `dash.toml` is on the new keys; car pending |
| [`dash/06-car-and-bench.md`](dash/06-car-and-bench.md) | open questions for the car |
| [`dash/13-screens.md`](dash/13-screens.md) | channel menu for pages |
| [`dash/14-one-bus-three-clients.md`](dash/14-one-bus-three-clients.md) | design; §7 is the dash work order; §6a has the lever probe (2026-09-26) |
| [`dash/15-enclosure.md`](dash/15-enclosure.md) | enclosure hand-off; round 3 printed, the 2026-10-03 changes not (CAD `44c19c7`) |
| [`dash/17-bench-ble-usb.md`](dash/17-bench-ble-usb.md) | bench passed except §2 item 8; §4 on the car: BLE `info`, `faults`, `watch`, `units`, `measure` pass (2026-09-26), the cable and the guard open; the ESC's acceleration is a dash on the glass (2026-09-30) |
| [`dash/18-setpoints-and-drift.md`](dash/18-setpoints-and-drift.md) | specified vs actual channels, and the drift alarm — merged (PR #4, 2026-09-15); the owner's `dash.toml` pairs boost; car pending |
| [`dash/19-stalk-and-stopwatch.md`](dash/19-stalk-and-stopwatch.md) | the lever and `[[button]]` pins as input, the stopwatch page — merged (PR #12, 2026-09-27); on the car 2026-09-30 the lever pages and the stopwatch and LIMIT work; items 4–6 and the pin buttons open |
| [`dash/22-oled.md`](dash/22-oled.md) | the SSD1322 on the board — merged (PR #21, 2026-09-30), draws on the bench and in the car; GPIO8/20/21 the standard (2026-10-03); which way up is open |
| [`dash/20-fault-count.md`](dash/20-fault-count.md) | the car's stored codes counted once after boot, a triangle and the count at the status strip's right end, `?` when there is no count — built 2026-09-27; needs the car |
| [`dash/21-runs-in-flash.md`](dash/21-runs-in-flash.md) | stopwatch runs in flash, read over BLE, saved on LIMIT; recorded 2026-09-27, open questions for the owner |
| [`dash/23-power-consumption.md`](dash/23-power-consumption.md) | watts in the status strip — recorded 2026-10-19, not started: needs a derived channel and the currents verified on the car |
| [`label-lookup/04-vcds-follow-ups.md`](label-lookup/04-vcds-follow-ups.md) | what `label-lookup/02` and `03` left open — filed 2026-09-28; three items for the owner to decide |
| [`measure/01-pedal-at-rest.md`](measure/01-pedal-at-rest.md) | the engine's absolute pedal reads 15 % at rest and the coastdown waits for 1 % — found in review 2026-09-28; the owner's call |
| [`measure/02-cross-check-tracks.md`](measure/02-cross-check-tracks.md) | every unit's cross-check speed lands in one track — found in review 2026-09-28; open |

Finished task files are in `.archive/tasks/done/` (`dash/16`, UDS over BLE, moved there on
2026-09-15 — its car check is `dash/17` §4; `label-lookup/02` and `03`, merged as PRs #15 and
#18, moved there on 2026-09-28; `dash/26` and `dash/27`, merged as PR #30, moved there on
2026-10-19 — `dash/26`'s car check is Next item 12; `dash/24`, the bus-time estimate, and
`dash/25`, one core for the board and the replay, merged as PRs #33 and #34 on 2026-10-20;
`measure/03`, moved there on 2026-10-21); superseded designs in
`.archive/specs/`.

## Command names in older documents

`research/` and `.archive/` keep commands as they were typed on the day. Today:

```
setup devices info units adaptations faults sensors watch measure dev
dev: sniff sim glossary recording dash vcds
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
| `vagcan-measure …` | `vagcan measure …` (the binary removed 2026-10-20, PR #46) |

Every command and flag the skills under `.claude/skills/` name was run against `--help` on
2026-10-21 by `research/tools/skill-commands.py` and resolves; `vagcan dev survey` and
`faults --from` do not parse.

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
  **Corrected 2026-10-20 (PR #40), re-checked 2026-10-21:** the conclusion stands, two of its
  reasons and its costs do not. The "runtime product" is the file name, so every classic file
  opens with no search at all (the unmasked half's ~39 h is gone; a cold `setup` takes ~3 s).
  The mask is not a global of VCDS's: it is the interface's answer to cable command `0x45`, a
  challenge built from the file's `[CMP]` number and a name hash, and VCDS holds no copy — still
  not derivable offline. One recovered key now gives a shifted file's whole mask, so it costs one
  search per file, not one per section. A file is shifted exactly when its `[CMP]` number has 8
  digits (6,204 of 16,576 in the English 26.3). `.archive/research/labels/tttext2.md` §3.3b.

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
  Re-checked 2026-10-21 under PR #40's name keys (`vagcan dev vcds rod --no-crack -v` on the
  English 26.3's copy): `[CMP]` opens by the name, its number has 8 digits, and `[TXT]` is
  masked, "no crib". `[CMP]` is exempt from the mask, so its key does not give `[TXT]`'s; the
  ~21 h stands.
- **A per-car cache of learned unit pairings.** Which CAN request id answers a unit number
  is in no label file — the two numberings are unrelated — so it is learned per car by
  `units --identify` and lost when the process exits. `~/.vagcan/cars/<VIN>/` is
  where it would live; since 2026-09-28 it holds `units.json` — each identified unit's request
  id and identity, not the unit number it answers for. An ODIS project may carry the id after
  all (2026-10-22, unverified): its `DOP_TEXTTABLELogicLinkNamesNodeAddre` tables give each
  logical link a node address, and on the reference project the dash's is `0x14` — `0x714`
  is `0x700 + 0x14`. Not used: the pairing still comes from the car.

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

