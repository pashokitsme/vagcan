# vagcan — roadmap (detail)

The short roadmap is in [`README.md`](../README.md#roadmap) and is kept current there.
This file is the detail behind it: state, decisions, open items, and what is dead. Older
dated status sections moved verbatim to
[`.archive/tasks/roadmap-history.md`](../.archive/tasks/roadmap-history.md) on 2026-09-14.

## Where things stand (2026-09-14, evening)

**Milestone: one scheduler on the board and the laptop, and the laptop reads the car
through the board.** Built on branch `ble-uds` (not on `master` yet), reviewed lens by
lens, hardware-free tests (1,522 in the workspace). The first end-to-end bench run found
the CAN pair dead (`research/dash/can-bring-up.md` §9.7).

- **Scheduler** (`dash/14` §2). `vag_uds_client::schedule::Planner`, `no_std`, no clock:
  subscriptions with drop semantics, one read per `(unit, did)`, a unit's due
  identifiers in one `22`, ceiling 100/s, panel floor 25/s, nothing starves. Laptop:
  `vag-cli-core/src/bus` owns the link, every car command runs through it, `watch` and
  `measure` subscribe (`read_batch` is gone). Board: replaces `can_task`'s round-robin;
  `hz` per channel in `dash.toml`, default 2; the acceptance filter follows the exchange.
- **UDS over BLE** (`dash/16`). The board always advertises and guards itself
  (`vag_uds_client::guard`); the laptop has `--device ble`, `ble:<name>`, and falls back
  to BLE when no cable adapter is found. `watch` and `measure` run on board-side
  subscriptions stamped with the board's clock.
- **The board over its USB cable, and `--slcan`** (`dash/14` §3, §7 item 4). The framed
  link on USB with `Guard::cable`; a Hello/HelloReply probe tells the `dash` image apart
  without sending it `V`; `vagcan --slcan` makes the `dash` image a plain adapter for
  one run.
- **Bench tools.** `bleuds` (one framed request or subscription over BLE) and `benchecu`
  (the CANable answering as a unit; bench pair only, stops on car traffic).
- **Bench, 2026-09-14.** The board's half of UDS over BLE passed with `bleuds`
  (§9.5, §9.6). `vagcan` over BLE connected and subscribed, but no frame crossed the pair
  in either direction, `cantx` included (§9.7). **Needs the owner at the bench.**
- **Also 2026-09-14:** `setup` suggests the nearest existing path on a typo; `watch
  --hz` given explicitly wins over the saved rate.

**Not verified on hardware:** everything of 2026-09-14 — the plan is
[`dash/17-bench-ble-usb.md`](dash/17-bench-ble-usb.md) — plus the older items it carries
(the board's `V` probe and busy-port message, `F` through `dev sniff` on the board, the
stored-config check at boot, whether opening the board's port twice resets it).

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
| BLE with zero friction: no pairing, no button, BLE when no cable; `watch` and `measure` over BLE (2026-09-14) | `dash/16` |

## Next, in order

**Without the car**

1. **Bench: `vagcan` through the board** — [`dash/17`](dash/17-bench-ble-usb.md). Fix the
   CAN pair first (§9.7), then BLE end to end with `benchecu`, the USB link and `--slcan`.
2. **`ble-uds` → `master`** — after the bench, by pull request.
3. **Why the pair went quiet** — §9.5, §9.7: an hour unacknowledged with the board's
   console captured, once the pair works again.
4. **OLED and enclosure** — `dash/15`; waits for the panel.
5. **Alarms on the board** — `dash/04`: the render side is done, the firmware wiring is not.
6. **Car picks its project** — `project::covering()` returns `None`; blocked on which of a
   car's part numbers to believe.

**With the car**

7. **Cruise-lever probe** — `dash/14` §7 item 10: `1105` on `70C`, and the engine's GRA status.
8. **Faults without VCDS, live** — `vagcan faults` after an ODIS-only `setup`; then
   freeze-frame layouts (`MCD_DB_ENV_DATA_DESC`) for `faults --details`.
9. **Stopwatch** — `dash/14` §6: fit `380B` → km/h on a steady stretch, then a run.
10. **Questions only the car answers** — `dash/06`.
11. **Reverse-gear code** — `catalog.rs` says `0C`, ODIS says reverse is `7`. Select
    reverse, read `0x210F` on `7E0` and `0x3816` on `7E1`.
12. **Sweep witness constants** — `WITNESS_EVERY = 64`, `QUIET_RUN = 3` are reasoned, not
    measured. One parked whole-car run.
13. **`watch` and `measure` across all fifteen units** — measured against the file, not
    the car.

## Task files

| file | state |
|---|---|
| [`dash/04-alarms.md`](dash/04-alarms.md) | render done, firmware wiring open |
| [`dash/06-car-and-bench.md`](dash/06-car-and-bench.md) | open questions for the car |
| [`dash/13-screens.md`](dash/13-screens.md) | channel menu for pages |
| [`dash/14-one-bus-three-clients.md`](dash/14-one-bus-three-clients.md) | design; §7 is the dash work order |
| [`dash/15-enclosure.md`](dash/15-enclosure.md) | enclosure hand-off |
| [`dash/16-uds-over-ble.md`](dash/16-uds-over-ble.md) | built on `ble-uds`; bench and car pending |
| [`dash/17-bench-ble-usb.md`](dash/17-bench-ble-usb.md) | bench plan for 2026-09-14's work |

Finished task files are in `.archive/tasks/done/`; superseded designs in `.archive/specs/`.

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
2026-09-13 and resolves.

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
- **Cross-platform `no_std` core + `vag-runtime-*`** — spec + M1 plan retired with
  `docs/superpowers/` in `2e4721b`. Below-the-seam refactor.

