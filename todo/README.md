# vagcan — roadmap & status

**Goal:** read the **whole car over CAN** and show measurements by name/value/unit,
definitions as **data, not code**: names from VW's own label files
(`catalogs/names-uds.json`), read address + scaling + unit **proven live on the car**
(`catalogs/vehicles/<part number>.json`, keyed by what the unit reports about itself) —
the corpus provably does not carry them
(`research/rod-labels.md` §4.0c, `research/label-linkage.md` §3). Any value a unit
exposes is selectable from config, with no hardcoded addresses or formulas in Rust.
Live transport = the **generic USB-CAN adapter** (`vag-can`, slcan). See `/CLAUDE.md`
for the locked stack and `todo/GOAL.md` for the goal statement.


## Where it stands after the whole-car pass (2026-08-02)

The tool now reads **every control unit the car has**, not the two the ISO addressing
block reaches. On the reference car that is 15 units and 1206 identifiers
(`research/whole-car-survey.md`), and every previously unidentified unit named itself:
parking aid, steering assist, ESC, airbag, climate, both door modules, telematics,
media.

Done since the last update:

| what | where | note |
|---|---|---|
| whole-car sweep | `vagcan survey` | gateway list → every unit; identification, fault codes, nine identifier pages; `--diff` compares a parked and a driving run |
| fault reader | `vagcan faults` | confirmed codes only, sorted with what is failing now first, occurrence count, odometer, time of day, and a date stated as a bound |
| unit addressing | `vag-protocol::address` | two id blocks with different response rules; unit-number pairings live in `catalogs/unit-numbers.json`, not in the source |
| catalogs as data | `vag_data::catalog::CatalogStore` | one file per control unit under `catalogs/vehicles/`, keyed by the part number the unit reports; nothing car-specific compiled in |
| corpus unit labels | `LabelDb::unit_for_part` | `; Component: … (#02)` headers give an address and a name for 987 of 3035 label files |
| live view | `vagcan watch` | ratatui, several units at once, `/` filter over everything a survey found, actual/specified pairs on one line |

### The open work

1. **Drive with the survey running.** One parked pass exists; `survey --diff parked.jsonl
   driving.jsonl` names the identifiers that moved, and those are the live measurements.
   Cheapest large win available, needs no label file.
2. **Fault names.** The chain is: fault number → `UDS_EV/RD.rod` registry (**established**,
   946/946) → text-id → `catalogs/names-uds.json`. The last hop is blocked by the
   per-table digit substitution (`research/label-linkage.md` §2.4). Everything else about
   this is recorded in `research/whole-car-survey.md` §3, including what has been refuted.
3. **The cluster's coolant scaling.** `22D0` has read `0xB8` = 90 °C in every sample ever
   taken; one cold start settles `×0.75 − 48` against `×0.5 − 2`.
4. **Brakes and body control.** `0x713` and `0x70E` answer 48 and 126 identifiers and own
   the signals a driver can provoke on demand — pedal, wheel speeds, lights, doors.
5. ~~**The clock's epoch.**~~ **Settled (2026-08-02).** There is no epoch: the stamp is a
   packed calendar date and time, in stored faults *and* at live `0x02BD`. The apparent
   free-running counter at `02BD` was raw subtraction of a packed field — the seconds
   field wraps at 60 in six bits, so a raw difference overshoots by 4 per minute boundary.
   Established against the instrument cluster's own clock across three sweeps; see
   `vag_protocol::dtc::CarTime` and `research/whole-car-survey.md` §2.3. What is *not* a
   protocol fact: this car's clock runs four days behind real time.

## Status (2026-08-01)

The protocol stack, the identity reader, and the whole `.rod` label-decrypt pipeline are
built and merged. The offline path to measurement *scaling* is **refuted, not just stalled**:
the read DID provably does not live in the corpus (`research/rod-labels.md` §4.0c,
`research/label-linkage.md` §3 — a structural impossibility, do not retry). Scaling comes
from the car: the parallel-VCDS capture session ran, `vagcan vcds analyse` and `vagcan recording calibrate`
turn recordings into proven rows, and `catalogs/vehicles/` holds 16 of them across engine,
gearbox and cluster. Names come from the corpus after all — `TTTEXT.ROD` is cracked
(`research/tttext-codec.md`) and `catalogs/names-uds.json` carries 17,009 names, but the
corpus has **no name→DID join**, so `vagcan vcds names` output is a hypothesis to test live.

The adapter works on the car. `vagcan info` matches the Auto-Scan oracle, `vagcan survey`
walks every unit the gateway lists (identification, stored DTCs, identifier sweep), and
`vagcan watch` is a full-screen multi-unit TUI. What remains for M3 is **coverage**: the
open-work list at the end of this file.

### Milestones
| M | what | state |
|---|------|-------|
| M0 | ISO-TP + UDS + transport stack (read-only allowlist) | ✅ done |
| M1 | `vagcan info` — VIN + Engine/Gearbox identity (UDS RDBI) | ✅ **verified on the real car 2026-08-01** |
| M2 | `.rod` decrypt+inflate in-tool; STRUC/DOP/TTTEXT/MWB cracked; base-14 codec proven; `vagcan vcds labels` | ✅ done |
| **M3** | measurements → `MeasurementDef` catalog → generic CAN reader → config-selectable | 🟡 **16 catalog rows proven (engine, gearbox, cluster) + three OBD-II services decoded from the standard; the gear and selector are read as states; open work = whole-car coverage** |
| HW | generic USB-CAN (MKS CANable) bring-up on the car | ✅ live on the car: reads + writes at 500k |

### Done (merged to `master`, tests green, clippy clean)
| subsystem | crate | what |
|-----------|-------|------|
| async-core | vag-transport | async transport trait(s) + mock, error model |
| uds-async | vag-protocol | async ISO-TP (15765-2) + UDS client (14229), read-only allowlist |
| generic-can | vag-can | `SlcanBackend` + `IsoTpCan` (the bypass transport — built, untested on hw) |
| info-identity | vag-protocol/vagcan | `EcuIdentity` + `read_identity` + `vagcan info` (Engine 01 + Gearbox 02). **Live-verified on the car** |
| can-sniff | vag-can/vagcan | `SlcanMode::Silent`, passive `IsoTpSniffer`, `vagcan sniff` |
| scan | vagcan | `vagcan scan` — group-testing sweep of the identifier space; `vagcan properties` |
| odx-link | vag-data/vagcan | `find_rod_by_odx_name` + `labels --from-car`: the unit names its own `.rod` (F19E) |
| label-corpus | vag-data/vag-db | `.lbl`/`.clb` parse+decrypt, `.rod` decrypt+inflate, `LabelDb` lookup, `load_corpus`/`scan_corpus` |
| rod-crack | vag-data | `.rod` TEA-CBC + product/IV recovery in-tool (`vagcan vcds rod`); STRUC/DOP/TTTEXT/MWB inflate; **base-14 codec proven (disasm)** |
| struc-table | vag-data | `StrucTable`/`StrucRecord` + `decode_base14_be`; `mwb` parser; `measure` (proven ignition `0x5555`→0.0° anchor) |
| labels-cli | vagcan | `vagcan vcds labels` — corpus inventory + `--part` / `--block` lookup; SQLite cache under `~/.vagcan/label-cache/` per corpus dir (`--refresh` rebuilds); the IV brute force is behind the `rod-crack` feature (`vagcan vcds rod` only) |
| addressing | vag-protocol | `address.rs` — `UnitAddress`: ISO block `7E0..7E7` → +8, VW block `700..7BF` → +0x6A; fixes `--ecu 17` resolving to `0x7F0` (nothing) instead of the cluster `0x714`; short numbers only for evidenced units (01/02/09/16/17), everything else by request id |
| survey | vagcan | `vagcan survey` — walk the gateway's installation list (plus engine/gearbox/gateway, which it never contains): identification, stored DTCs (`19 02 FF`), then the identifier bands in use on this car; JSON lines per unit; silent units skipped after ident |
| watch-tui | vagcan | `vagcan watch` — full-screen ratatui TUI, multi-unit, reconfigurable in place (`c`); `--survey FILE` offers everything a survey found; actual/specified pairs on one line; unconverted CSV columns suffixed `_raw` |
| calibrate | vagcan | `vagcan recording calibrate` — offline; fits `_raw` columns against trusted reference columns in the same `watch --out` recording |
| names | vagcan | `vagcan vcds names` — substring search over `catalogs/names-uds.json` (17,009 TTTEXT names); a match is a hypothesis, the corpus has no name→DID join |
| cli-app | vagcan | Top level, all needing the car: `devices` / `info` / `units` / `properties` / `sniff` / `sensors` / `watch` / `scan` / `survey` / `faults`. Offline work is grouped by what its input is — `recording calibrate|discover` over our own recordings, `vcds labels|names|analyse|rod|corpus|tttext` over VCDS's files (2026-08-03) |

## M3 — measurements (the current work)

### How VCDS reads measurements on this car (MQB / UDS)
The ECU's `.rod` (`EV_ECM…rod`) is VCDS's source of truth: each measuring value =
`{ read DID, COMPU-method ref → DOP scaling, text-id → TTTEXT name }`. VCDS issues
`UDS 22 <DID>`, applies the COMPU method, prints `name = value unit`. Groups (`G004…`)
bundle DIDs. That was the original target — decode the ECU `.rod` into
`(DID, scale, unit, name)` — but the corpus turned out not to carry the DID or readable
scaling (below), so this project gets the *names* from the corpus and proves the
*(DID, scale, unit)* part on the car.

### What is done vs the wall
- ✅ `.rod` TEA-CBC + zlib + the per-record `product`/IV blocker — **defeated offline**
  (DEFLATE header oracle + Kraft pruning + inflate confirm); `STRUC.rod` inflates to
  293,560 bytes in our own tool.
- ✅ All four tables located + cracked: **STRUC** (1221 structure ids), **DOP/TTDOP**
  (17,636 COMPU/scaling ids), **TTTEXT** (names), **MWB** (engine measuring rows).
- ✅ Payload codec proven **base-14** over charset `0123456789,.-_` (disasm at
  `0x1401898b0`, mod-14 arith `fcn.1400e6f80`).
- 🔴 **NOT reversed: STRUC field segmentation** — where inside a `NNNNNN,<base-14>`
  record the `read_id (DID)` / `raw-spec` / `scale` / `unit-ref` / `name-ref` live.
  Offline static + data-only RE is exhausted (5 passes; base-40 `code→id`, fixed-column,
  per-byte index all refuted — `research/rod-labels.md`).

### The supervised STRUC × crib attack — DONE, refuted
Crossing the capture crib's real DIDs with the decoded STRUC table was the M3 lever. It
ran end-to-end and produced a clean negative: the read DID is **not stored in STRUC** in
any tested encoding, `STRUC-id` is not the IDE measurement id, and `IDE-id` is not the
MWB row index (`research/rod-labels.md` §4.0c). Do not re-run it.

### The lever that worked — sniff VCDS on the bus (the live crib)
Every prior crib came from USB captures of the HEX clone, where the link cipher hides the
payload and VCDS's **group reads** — the source of RPM / vehicle speed / coolant — never
decoded (§4.0a/§4.0b). CAN is multi-drop, so a second adapter can sit on the same OBD-II
bus in listen-only mode while VCDS runs a normal session and record the whole conversation
**in the clear**, multi-frame group reads included.

Tooling (built, `docs/superpowers/specs/2026-07-31-can-sniffer-design.md`):
- `vagcan sniff --out cap.jsonl` — listen-only by default; streams every frame
  to a `vag-capture` JSONL headed by a wall-clock anchor, reassembles ISO-TP live, and takes
  operator markers from stdin. The anchor exists because the capture↔CSV lag had to be
  *guessed* last time (~52 s), which is how several "correlations" turned out to be
  window-fishing.
- `vagcan scan --ecu 01` — read-only sweep of the RDBI space; finds what the ECU exposes
  regardless of what any label file names.
- `vagcan properties --ecu 01` — the identification range, named.

The pairing — sniff + VCDS running ADVMB logging to CSV, engine running, a wide rev —
was collected on 2026-08-01 and yielded `(read address → raw bytes → displayed engineering
value)` directly, with no dependence on the `.rod` field codec. See "The remaining work"
item 1 below.

### Overnight results (2026-08-01→02, four parallel analyses)

All from data already on disk; the car was not attached.

**The car enumerates itself.** The gateway's installation list (`0x2A26`, also `0x04A3`)
is a 32-byte bitmap, LSB-first, indexed by `id − 0x700`. One read replaces sweeping
`0x700..0x7BF`. Verified before use: all seven units separately observed answering appear,
with no false negatives; the opposite bit order finds two of seven. Shipped as
`vagcan units`. Nine units answer; four are identified by their own identification block
(steering column `5Q0 953 521 KM`, body control `5Q0 937 084 CF`, gateway `3Q0 907 530 B`,
cluster `5E0 920 740 D`).

**Two more OBD-II services are mirrored.** `F6xx` is service 06 and `F8xx` is service 09 —
proven by content, not convention: `F802` holds the VIN, `F804` the calibration identifier
`8V0264H 0005AEAJ`, `F80A` the string `ECM\0-EngineControl`. Service 09 now decodes in
`vagcan properties`; it carries what `F1xx` cannot, namely which emissions calibration the
unit is actually running.

**The engaged gear, without a VCDS log.** `0x3816`, proven by arithmetic against the
already-proven shaft speeds (η² = 0.972, runner-up 0.072). `gear = code − 1`; `0x0C`
reverse, `0x00` not engaged. Selector lever at `0x3809` (P/R/N/D). Both are represented
with the new `Scaling::Enum`, because a gear is not a quantity — a linear scaling would
report reverse as "gear 11".

**Odometer, by exact hit.** Cluster `0x2203` returned `03 3F 18` while a log read
212,760 km. `0x033F18` = 212,760.

**A defect in our own tool.** `watch --out` wrote one timestamp per row, but identifiers
are polled in batches, so columns are up to a polling cycle apart. Every value now carries
its own time column — the same thing VCDS's export does, and which this project already
parsed correctly for VCDS while producing the flawed version itself. Correcting it lifted
the gear evidence from η² 0.872 to 0.972.

Writeups: `research/identifier-map.md`, `research/other-ecus.md`,
`research/gearbox-state.md`.

### What the next session should do

1. **Re-sweep the engine with it running.** 450 of 896 identifiers read zero because the
   sweep was taken engine-off — very likely "no signal", not "unimplemented". Cheapest
   improvement available.
2. **A parked identification pass driven off the gateway list** — shipped as
   `vagcan survey`: reads `2A26`, walks every listed unit (plus engine/gearbox/gateway,
   which the list never contains), reads identification, stored DTCs and the identifier
   bands in use on this car, one JSON line per unit. Run it parked.
3. **Record the gearbox `3820–38FF` block while driving.** Every proven clutch row lives
   there and none of it has been recorded moving.
4. **Select S and paddle-shift deliberately.** The lever moved through P, R, N and D during
   the recording — that is what proved the selector at `0x3809` (P 76 samples, R 48, D 294;
   N only 4, hence flagged weak). What is missing is the **drive mode**, which on a DQ200 is
   a separate signal from lever position: D versus S versus manual. It was never selected,
   so the stimulus is absent, not the signal. Also worth holding N for ten seconds to settle
   it properly.
5. **A cold start** while polling cluster `0x22D0`, the one action that converts its
   coolant reading from "consistent with" into measured.

### The remaining work, in order

**1. The capture session — DONE 2026-08-01, and it worked.** 308 s of listen-only capture
alongside a live VCDS session; `vagcan vcds analyse` proved three scalings, one of which
(coolant = `raw − 40`) reproduces the standard OBD-II PID 05 formula and thereby validates
the whole pipeline. Details in `research/rod-labels.md` §4.3.

**1a. More coverage — the next session.** The logs were only ~20 s each, giving 14–16
matched points against a 20-point default. Record VCDS logging for **several minutes** per
group, covering the measurements that matter (boost, load, throttle, speed), with a wide
sustained rev. Everything else is already built.

**2. `vagcan vcds analyse` — BUILT (2026-08-01).** The offline tool that turns a capture into
scalings, written before the session so the data can be checked while the car is available.
It:
- reads the capture JSONL and the VCDS CSV (CP1251; each measurement carries its own time
  column) and aligns them by the **wall-clock anchor** — a subtraction, never a search;
- reassembles ISO-TP, pairs `0x22` requests with `0x62` responses, and splits
  multi-identifier responses by the requested order, **skipping** any response it cannot
  split unambiguously;
- fits `factor`/`offset` by least squares over every raw interpretation, accepting only
  `R² ≥ 0.995` over `≥ 20` points and reporting near misses as leads;
- emits accepted rows as `MeasurementDef` catalog entries, which `UdsReadExt::read_catalog`
  already reads.

Exercised against real capture+log data on 2026-08-01: it found the three scalings above
and rejected a two-level false positive, which is what the guards are for.

**3. Names from the `.rod` — DONE.** Scaling comes from the car, and after the linkage
attempt (`research/label-linkage.md`, `research/rod-labels.md` §4.4) that is settled: the
corpus holds **no linear coefficients**, its values are base-10 under a per-table glyph
substitution, and the `MWB` code is a global function of the text-id with no per-ECU degree
of freedom. So the corpus is for **names and per-ECU lists**, nothing more.

The name table itself is cracked (`research/tttext-codec.md`): **17,009 names** shipped in
`catalogs/names-uds.json`, searchable with `vagcan vcds names <text>`. The `ENG######` question
is settled — the number **is** the `TTTEXT` text-id, proven four for four on records solved
blind (`research/tttext-codec.md` §2, superseding `research/label-linkage.md` §4's
"suggestive, not established"), and the recovered names are English text — the
`ENG`-means-*English* reading, not *engine*. That closes the chain
*proven identifier → IDE → ENG → name* for gearbox rows whose `IDE` the VCDS log prints —
but only for those. The corpus itself carries **no name→DID join**: `MWB` has no per-ECU
identifier, so a `vagcan vcds names` hit is a hypothesis to confirm on the car, not a binding.
`mwb.rs` is deliberately kept for the possible MWB→TTTEXT name join.

**4. `vagcan watch` — BUILT, now a full-screen TUI.** ratatui, multi-unit, reconfigurable
from inside (`c`); `--survey FILE` offers every identifier a survey found; actual/specified
pairs (e.g. boost `0x2029` specified / `0x202A` actual) render on one line. Polls live at
bus speed (46 Hz measured on the boost set) using batched reads. Anything unproven prints
its bytes tagged raw, and unconverted columns are written to CSV with a `_raw` suffix —
which is what `vagcan recording calibrate` fits against the trusted columns in the same recording.

**5. Discrete state — `vagcan recording discover` BUILT; gear and selector identified.** Gear,
gearbox mode, switches and lamps cannot be fitted: a two-level value fits any line
exactly. `discover` classifies a `watch --out` recording into never-moved / stepped /
continuous and ranks the stepped columns, with `--pairs` for candidates whose transitions
coincide. The gear (`0x3816`, η² = 0.972 against the proven shaft-speed ratio) and the
selector lever (`0x3809`) are identified and in `catalogs/vehicles/0CW300041G.json` as enums.
Still to do: the drive mode (D/S/manual — never selected during the recording, so the
stimulus is missing, not the signal), and the same treatment for the other units.

### Then — the extensible foundation (architecture)
```
MeasurementDef { name, unit, address: Uds(did) | Group(g,field), raw_form, scale }
MeasurementCatalog                                                  // data, not code:
    names/lists from the corpus, scaling from live calibration
read_measurement(&def, uds) -> (name, value, unit)                 // one generic path
```
Add a parameter = a data row / config selection, never new match-arms. Scaling is proven
empirically from a live crib (`analyse` / `calibrate`) — the corpus provably cannot supply
it; names and per-ECU lists are what the corpus is for.

## Hardware checkpoints (STOP, confirm on the real car)
Dongle: **MKS CANable V2.0 Pro** (STM32G431 + ADM3050E isolated) — fits `vag-can`'s
`SlcanBackend`, no new backend.

**Bench bring-up: DONE (2026-07-31).** It enumerates as CDC-ACM (`16d0:117e`,
`/dev/cu.usbmodem*`), so the firmware is **slcan**, not candleLight — no reflash. It answers
`V` and `E` and stays responsive, but acks nothing else; its whole command set is
`O C S Y M A V E t T r R d D b B X` — no `L`, no `N`, no `F`, **no loopback**. Listen-only is
`M1`, not `L`. Since it has no loopback and CAN needs a second node to ACK, TX/RX **cannot**
be proven on the bench (`crates/vag-can/examples/slcan_probe.rs`).

Before touching the car: wire OBD2 pin 6→CAN-H, 14→CAN-L, 4/5→GND, **do NOT** wire pin 16;
**open the 120R jumper** (the vehicle bus is already terminated at both ends, ~60 Ω; a third
resistor drags it to 40 Ω); leave **BOOT** open (DFU only).

Risk climbs monotonically — stop and confirm at each step:
1. ✅ **`vagcan sniff`, no VCDS.** Zero risk: listen-only cannot even ACK.
2. ✅ **`vagcan sniff` + VCDS in parallel** — done 2026-08-01, 308 s captured alongside a
   live VCDS session; see "The remaining work" item 1.
3. ✅ **`vagcan info`** — done 2026-08-01, see below.
4. ✅ **`vagcan scan`** — full `0000-FFFF` sweep of both units, 2026-08-01: the engine
   answers **896** identifiers (191 s, 10,840 requests), the gearbox **541** (274 s, 9,406
   requests). Results in `research/dumps/*-full.jsonl` (gitignored).

### First live session — 2026-08-01 (M1 CLOSED)
`vagcan info` over the CANable read the car and matched the Auto-Scan oracle on four
independent points: VIN `XW8AD4NE9JH008917`; Engine `8V0906264H` (the very part whose
`EV_ECM18TFS0208V0906264H.rod` the label work is built on) / HW `06K907425B` (the
`06K-907-425-V1/V2.clb` pair) / `1.8l R4 TFSI`; Gearbox `0CW300041G`, `GSG DQ200G2_M`,
SW `1003` — the same `1003` the old USB capture crib yielded.

What the bus actually looks like, measured rather than assumed:
- **The OBD-II diagnostic line is nearly silent.** 8 s of listening yields ~46 frames, all
  one periodic extended id `0x17F00010` (~6 Hz) from the gateway. So *silence is not
  evidence of a fault* on this platform, and `bus_doctor`'s functional-address probe is the
  test that discriminates.
- **Physical addressing only.** `0x7E0/0x7E8` answers; the functional broadcast `0x7DF`
  times out — the VAG gateway does not serve it on OBD.
- Rates other than 500k produce nothing, as expected.

### Sweeping is a group-testing problem, not 65,536 reads
Measured on the reference car, and the reason a full sweep is minutes rather than hours:

- A multi-identifier `0x22` request is answered with **only the identifiers the unit
  supports** — asking for `F190` (supported) together with `0001` (not) returns just
  `F190`. The unit refuses with `0x31` **exactly when it supports none** of them.
- That makes one request a **presence test for a whole batch**: a refusal skips the batch
  outright, a positive answer is halved until the responders are isolated and read singly.
  On `F100-F1FF` this finds the same 22 identifiers in **118 requests instead of 256**; over
  the sparse rest of the space the saving approaches the full batch factor.
- **The per-request limit is between 8 and 12 identifiers** on this unit: 8 are answered, 12
  are refused with `0x31`. Exceeding it is a *silent, total* failure — every batch looks
  empty and the sweep cheerfully reports zero hits. It did exactly that at batch 16 before
  the limit was found.
- Therefore `scan` probes with a **full-size batch** (one known-good identifier padded with
  impossible ones) before trusting group testing, and falls back to one-at-a-time when the
  probe fails. A token two-identifier probe would have passed and hidden the bug.

Debugging note worth keeping: the adapter can enumerate on USB (correct VID/PID/serial in
`system_profiler`) while macOS attaches **no** serial node — `/dev/cu.usbmodem*` simply is
not there and every open fails with "No such file or directory". That is a USB-stack hang,
not a bus fault; a full unplug/replug (power-cycling the MCU) restores it. Check
`ls /dev/cu.usbmodem*` before believing any "the bus is dead" result.

**Validation oracle:** the owner's full Auto-Scan is in `archive/research/vcds-rus-crack.md`
(VIN `XW8AD4NE9JH008917`, every ECU part-number/coding/VCID) — golden fixtures.

## The open work (M3 coverage and beyond)

- **A car keeps its own files.** `~/.vagcan/cars/<description>-<VIN>/` now exists
  (`crate::datadir`), holding `car.json`, `races/` and `reports/`. What is left is to point
  the commands at it: **`survey` with no `--out` should write into that car's `reports/`**
  with a timestamped name and say where it went, instead of printing a run nobody kept. The
  same applies to `faults --details`. The directory is named for what the car said about
  itself plus its VIN — no make or model, because a car does not broadcast one.

- **Whole-car measurement coverage.** The catalogs cover engine, gearbox and cluster; the
  survey reaches every unit. Specifically:
  - body control module `0x70E` — lights, doors, indicators;
  - cluster `0x714` — a **cold start** while polling `0x22D0`, the one action that turns
    its coolant scaling from "consistent with" into measured;
  - the unidentified units `0x712` / `0x713` / `0x715` / `0x746` / `0x74A` / `0x74B` /
    `0x767` / `0x773` (`research/other-ecus.md`) — identify, then sweep;
  - deeper engine and gearbox coverage (the `3820–38FF` gearbox block while driving, the
    engine re-swept with the engine running).
- **Unit addresses from the corpus.** `vag-protocol/src/address.rs` is a table in code,
  deliberately limited to the five evidenced short numbers. The addresses (and the full
  unit-number ↔ name mapping) should come from the label corpus (`.rod`/label files)
  instead. Open — not started.
- **Electrical-system and brake channels.** Nothing from ABS/brakes or the electrical
  system is in a catalog yet.

## Dead and archived (kept as negative results — do not retry)
- **HEX-clone live UDS** — the session KDF is VMProtect-sealed and dead. The `vag-hex`
  crate and the vendored FTDI D2XX driver are **deleted**; the research writeups moved to
  `archive/research/` (`vag-hex-framing.md`, `clone-crypto.md`, `vcds-rus-crack.md`) and
  stay authoritative as negative results. The clone capture decoder
  (`research/clb-crack/extract_uds.py`) stays useful as an offline crib source.
- **Scaling from the corpus** — refuted structurally, twice over
  (`research/rod-labels.md` §4.0c, `research/label-linkage.md` §3/§5).
- **OBD-II Mode 01 as the product path** — dropped. The standard sensors survive as
  `vagcan sensors` and as calibration references, not as the measurement model.

## Parked (designed, not being implemented now)
- **Cross-platform `no_std` core + `vag-runtime-*`** — spec + M1 plan under
  `docs/superpowers/{specs,plans}/2026-07-06-cross-platform-*`. Below-the-seam refactor.
