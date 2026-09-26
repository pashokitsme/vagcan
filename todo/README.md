# vagcan — roadmap (detail)

The short roadmap is in [`README.md`](../README.md#roadmap) and is kept current there.
This file is the detail behind it: state, decisions, open items, and what is dead. Older
dated status sections moved verbatim to
[`.archive/tasks/roadmap-history.md`](../.archive/tasks/roadmap-history.md) on 2026-09-14,
2026-09-15 and 2026-09-22.

## Where things stand (2026-09-22)

**Milestone: the bench over USB and BLE passes on `master`, alarms are in the owner's
`dash.toml`, and the board survives a hostile cable host.** The 2026-09-15 status moved to
[`.archive/tasks/roadmap-history.md`](../.archive/tasks/roadmap-history.md) on 2026-09-22.
Bench record: `research/dash/can-bring-up.md` §9.13–§9.15.

- **Boards, 2026-09-22.** The old SuperMini (rev v1.1, broken pins, `3V3` pad dead, the
  SN65HVD230 on `5V`) stays on the bench — the owner keeps it on 5 V and accepts losing it. The
  new one (rev v0.4) is a spare: its BLE controller never starts while its Wi-Fi scans, with our
  firmware, the probes and the August recon image alike; cause not found
  (`research/dash/ble-controller-hang.md`). The firmware's `ble` feature (default on) lets it run
  as `dash --no-default-features`; CI lints both builds. Not pursued further (owner,
  2026-09-22): the new board is a spare.
- **The bench, 2026-09-22** (`dash/17`): USB items 1, 2, 3, 9, 11, 13, 16 and §3 pass; BLE
  `info`, a board subscription, `watch` and `measure` pass on the old board. Item 11 passes in
  5–15 s, not ~1 s — macOS buffers the board's output for a stopped host. Left: item 8 (pull USB
  in adapter mode, the board on 12 V).
- **A 4 KB USB request flood, three faults fixed** (§9.14): a heap panic (the guard refuses a
  request over `MAX_REQUEST_BYTES` = 64, the laptop refuses it before sending), a USB read that
  never woke again (esp-hal rc.0 races `int_ena`; the reader re-checks the FIFO), and a panic
  while the frame was gathered (the board passes over a frame longer than
  `console::MAX_HOST_BODY` = 69).
- **`measure` over BLE** (§9.15): ~20 speed reads a second with the owner's plan; packing queued
  frames into one notification brought it to 34–38 (the cable: 45–48). Left there (owner,
  2026-09-22).
- **Alarms in the owner's `dash.toml`, 2026-09-22**: misfires 5/3, knock retard −2.0/−1.5,
  coolant 115/110 and boost drift 10 %/5 %, 2000 ms, floor 1.3 bar — recommended values from
  the regulation, VW workshop specs of other engine units and inference, none VW's own number
  for this ECU (`dash/04`). 13 channels, 4 pages; the file before is `dash.toml.before-alarms`.
- **BLE discovery says "adapter"**, not "dash board" (owner, 2026-09-22); the board still
  advertises as `vagcan-dash`.

**Not verified on hardware:** the car — [`dash/17`](dash/17-bench-ble-usb.md) §4 (faults,
info, watch, measure through the board; the moving-car guard; alarms; the cable on car
traffic; the ESC's channels) and `dash/18` (the difference and a drift rule on a real pull).
`dash/17` §2 item 8 (USB pulled in adapter mode) goes with the car too.

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
| BLE on the rev v0.4 board is not pursued; the board is a spare, run without BLE (2026-09-22) | `research/dash/ble-controller-hang.md` |
| `measure` over BLE at 34–38 speed reads a second, under the cable's 45–48, is accepted (2026-09-22) | `research/dash/can-bring-up.md` §9.15 |
| The bench board's transceiver stays on 5 V: its 3V3 trace is broken, and the board is expendable (2026-09-22). The risk is `RXD` at 5 V into `GPIO1`, not the car's bus; if the board acts up in the car, unplug it | `research/dash/can-bring-up.md` §9.12 |
| A channel's specified value written by hand as `setpoint`, never guessed from names; the panel shows the difference, not the value; a drift alarm is a percentage, a hold and a floor (2026-09-14/15) | `dash/18` |

## Next, in order

**Without the car**

1. **OLED and enclosure** — `dash/15`; waits for the panel.
2. **Alarms on the board** — `dash/04`. On `master` since PR #2 (2026-09-14),
   hardware-free tests only: `[[alarm]]` in `dash.toml`, checked at plan build, watched
   channels foreground at their own rate, takeover and silence through
   `vag_dash_render::screen`; the drift rule came with PR #4. The four recommended rules are in
   the owner's `dash.toml` since 2026-09-22 (on the bench: their channels polled on every page).
   Next: a run on the car, where the misfire window and the thresholds are checked. The
   hardware-free replay exists since 2026-09-26 (`vagcan dev recording dash`); the demo from a
   recorded drive waits for a recording with the retard channels, made on the car.

**With the car**

3. **The car, through the board** — [`dash/17`](dash/17-bench-ble-usb.md) §4: faults, info,
   watch and measure over BLE and USB, the moving-car guard, alarms, the cable on car traffic;
   and §2 item 8, USB pulled in adapter mode — the owner checks it in the car (2026-09-22).
4. **A specified value on a real pull** — `dash/18` §6: boost's difference through a pull,
   then the owner's `percent`, `hold_ms` and `min_setpoint`.
5. **Cruise-lever probe** — `dash/14` §7 item 10: `1105` on `70C`, and the engine's GRA status.
6. **Faults without VCDS, live** — `vagcan faults` after an ODIS-only `setup`; then
    freeze-frame layouts (`MCD_DB_ENV_DATA_DESC`) for `faults --details`.
7. **Stopwatch** — `dash/14` §6: fit `380B` → km/h on a steady stretch, then a run. Read the
    ESC's wheel speeds and longitudinal acceleration beside it (`713` `1800`–`1803`, `1822`;
    `dash/17` §4).
8. **Questions only the car answers** — `dash/06`.
9. **Reverse-gear code** — `catalog.rs` says `0C`, ODIS says reverse is `7`. Select
    reverse, read `0x210F` on `7E0` and `0x3816` on `7E1`.
10. **Sweep witness constants** — `WITNESS_EVERY = 64`, `QUIET_RUN = 3` are reasoned, not
    measured. One parked whole-car run.
11. **`watch` and `measure` across all fifteen units** — measured against the file, not
    the car.

## Task files

| file | state |
|---|---|
| [`dash/04-alarms.md`](dash/04-alarms.md) | on `master`; four rules in the owner's `dash.toml` (2026-09-22); a car run open |
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

