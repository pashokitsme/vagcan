# vagcan — roadmap (detail)

The short roadmap is in [`README.md`](../README.md#roadmap) and is kept current there.
This file is the detail behind it: state, decisions, open items, and what is dead. Older
dated status sections moved verbatim to
[`.archive/tasks/roadmap-history.md`](../.archive/tasks/roadmap-history.md) on 2026-09-14 and
2026-09-15.

## Where things stand (2026-09-15)

**Milestone: the scheduler, the board's links and a channel's specified value are on
`master`.** The evening-of-2026-09-14 status moved to the history file on
2026-09-15.

- **PR #2, `ble-uds`, merged 2026-09-14** (`77c01c5`): one scheduler on the board and the
  laptop, UDS over BLE, the board over its USB cable and `--slcan`, alarms on the board.
- **PR #3, `link-icons`, merged 2026-09-14** (`0dd1c90`): link icons in a column at the
  panel's right edge, the adapter screen's kb/s (bench: `benchecu` at 20 frames/s → 111 bits a
  frame, 2,280 bit/s each way; `research/dash/can-bring-up.md` §9.12), `dashsim --snap`.
- **PR #4, `setpoint-drift`, merged 2026-09-15** (`7d8e0a5`): `setpoint` pairs a channel with the value its
  unit asked for; the panel shows the difference; `[[alarm]] kind = "drift"` watches it
  (`dash/18`). Four review rounds, the last with no findings. Workspace tests: 1,655 at
  `38dfdef`. CI's bench host job failed on a stale preview count, fixed in `97487d3`; CI
  green on it 2026-09-15.
  The file format is [`docs/dash/dash-toml.md`](../docs/dash/dash-toml.md).
- **Stopwatch sources on the ESC, recorded 2026-09-15** (`dash/14` §6, `dash/13`, `dash/17`
  §4): wheel speeds `1800`–`1803` and longitudinal acceleration `1822` on `713`, declared by
  the ODIS project and never asked — the parked survey skipped `18xx`.
- **Bench hardware, 2026-09-15.** The SuperMini's `3V3` pad is dead; the owner moved the
  SN65HVD230 to `5V` and the pair carries frames again. Out of spec: the transceiver's `RXD`
  now drives the C3's `GPIO1` at 5 V, and the pin takes 3.6 V (§9.12). The board and the
  CANable then dropped off USB together twice; both at once points at the cable or the hub,
  not checked.
- **The owner's `dash.toml`** pairs boost `202A` with its specified value `2029`
  (2026-09-15); the file before that is `dash.toml.before-setpoint` beside it.

**Not verified on hardware:** the car — [`dash/17`](dash/17-bench-ble-usb.md) §4 (faults,
info, watch, measure through the board; the moving-car guard; alarms; the cable on car
traffic; the ESC's channels) and `dash/18` (the difference and a drift rule on a real pull).
On the bench: unplugging USB in adapter mode, a USB flood of large requests (`dash/17` §2
items 8 and 13), the stored-config check at boot, whether opening the board's port twice
resets it (§3).

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
| The scheduler is subscriptions with drop semantics, one layer on the board and the laptop; rates only from `hz` in `dash.toml` (2026-09-14) | `dash/14` §2 |
| BLE with no pairing and no button, and only when asked (`--device ble`, no automatic scan); `watch` and `measure` over BLE (2026-09-14) | `dash/16` |
| Link icons in a column at the right edge; the chart ends before it, connected or not; no icons on the adapter screen so far (2026-09-14) | `PR #3` |
| A channel's specified value written by hand as `setpoint`, never guessed from names; the panel shows the difference, not the value; a drift alarm is a percentage, a hold and a floor (2026-09-14/15) | `dash/18` |

## Next, in order

**Without the car**

1. **The transceiver back on 3.3 V** — hardware, the owner's hands. `RXD` at 5 V into `GPIO1`
   is past the C3's rating; a wire to the regulator's output, or a 3.3 V regulator off `5V`
   (`research/dash/can-bring-up.md` §9.12). Before the car.
2. **Bench leftovers** — [`dash/17`](dash/17-bench-ble-usb.md) §2 items 8 and 13: unplug USB
   in adapter mode, a USB flood.
3. **OLED and enclosure** — `dash/15`; waits for the panel.
4. **Alarms on the board** — `dash/04`. On `master` since PR #2 (2026-09-14),
   hardware-free tests only: `[[alarm]]` in `dash.toml`, checked at plan build, watched
   channels foreground at their own rate, takeover and silence through
   `vag_dash_render::screen`; the drift rule came with PR #4. Next: the owner writes the rules into
   `dash.toml`; the misfire rule's numbers and a run on the car. The demo from a recorded drive
   waits for a recording with the retard channels and a way to replay it (none is
   hardware-free today).
5. **Car picks its project** — `project::covering()` returns `None`; blocked on which of a
   car's part numbers to believe.

**With the car**

6. **The car, through the board** — [`dash/17`](dash/17-bench-ble-usb.md) §4: faults, info,
   watch and measure over BLE and USB, the moving-car guard, alarms, the cable on car traffic.
7. **A specified value on a real pull** — `dash/18` §6: boost's difference through a pull,
   then the owner's `percent`, `hold_ms` and `min_setpoint`.
8. **Cruise-lever probe** — `dash/14` §7 item 10: `1105` on `70C`, and the engine's GRA status.
9. **Faults without VCDS, live** — `vagcan faults` after an ODIS-only `setup`; then
    freeze-frame layouts (`MCD_DB_ENV_DATA_DESC`) for `faults --details`.
10. **Stopwatch** — `dash/14` §6: fit `380B` → km/h on a steady stretch, then a run. Read the
    ESC's wheel speeds and longitudinal acceleration beside it (`713` `1800`–`1803`, `1822`;
    `dash/17` §4).
11. **Questions only the car answers** — `dash/06`.
12. **Reverse-gear code** — `catalog.rs` says `0C`, ODIS says reverse is `7`. Select
    reverse, read `0x210F` on `7E0` and `0x3816` on `7E1`.
13. **Sweep witness constants** — `WITNESS_EVERY = 64`, `QUIET_RUN = 3` are reasoned, not
    measured. One parked whole-car run.
14. **`watch` and `measure` across all fifteen units** — measured against the file, not
    the car.

## Task files

| file | state |
|---|---|
| [`dash/04-alarms.md`](dash/04-alarms.md) | on `master` (PR #2), hardware-free; rules, misfire numbers and a car run open |
| [`dash/06-car-and-bench.md`](dash/06-car-and-bench.md) | open questions for the car |
| [`dash/13-screens.md`](dash/13-screens.md) | channel menu for pages |
| [`dash/14-one-bus-three-clients.md`](dash/14-one-bus-three-clients.md) | design; §7 is the dash work order |
| [`dash/15-enclosure.md`](dash/15-enclosure.md) | enclosure hand-off |
| [`dash/17-bench-ble-usb.md`](dash/17-bench-ble-usb.md) | bench plan for 2026-09-14's work; §2 items 8 and 13 and §3 open, §4 is the car |
| [`dash/18-setpoints-and-drift.md`](dash/18-setpoints-and-drift.md) | specified vs actual channels, and the drift alarm — merged (PR #4, 2026-09-15); the owner's `dash.toml` pairs boost; car pending |

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
- **The BLE stack out of the firmware's build** (2026-09-14) — `vag-dash-fw`'s `build.rs`
  generates the plan through `vag-cli-core`, which now depends on `vag-dash-ble`
  (btleplug), so CI's firmware job installs libdbus. Cleaner: `vag-dash-ble` behind a
  default `ble` feature of `vag-cli-core` (the BLE branches of `device.rs`), with the
  firmware's build-dependency on `default-features = false`.
- **Cross-platform `no_std` core + `vag-runtime-*`** — spec + M1 plan retired with
  `docs/superpowers/` in `2e4721b`. Below-the-seam refactor.

