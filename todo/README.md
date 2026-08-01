# vagcan — roadmap & status

**Goal:** read the **whole car over CAN** and show measurements by name/value/unit,
with every definition (name, read address, scaling, unit) taken **from VW's own label
files** (`.lbl`/`.clb`/`.rod`) — exactly like VCDS — so any value a block exposes is
selectable from config, with **no hardcoded addresses or formulas**. Live transport =
the **generic USB-CAN adapter** (`vag-can`, slcan). See `/CLAUDE.md` for the locked
stack and `todo/GOAL.md` for the goal statement.

## Status (2026-08-01, overnight)

The protocol stack, the identity reader, and the whole `.rod` label-decrypt pipeline are
built and merged. The offline path to measurements is **exhausted**: the STRUC field
segmentation is unreversed *and* the read DID is now proven not to live in STRUC at all
(`research/rod-labels.md` §4.0c). M3 therefore no longer waits on the label corpus — it
waits on **live evidence from the car**, and the tooling to collect it is now built.

The adapter works on the car. `vagcan info` matches the Auto-Scan oracle, the identifier
sweep runs over the whole 16-bit space in minutes, and the CLI has been rebuilt around the
commands a person actually uses. What remains for M3 is one capture session with VCDS
running in parallel, and the offline tool that turns it into scalings.

### Milestones
| M | what | state |
|---|------|-------|
| M0 | ISO-TP + UDS + transport stack (read-only allowlist) | ✅ done |
| M1 | `vagcan info` — VIN + Engine/Gearbox identity (UDS RDBI) | ✅ **verified on the real car 2026-08-01** |
| M2 | `.rod` decrypt+inflate in-tool; STRUC/DOP/TTTEXT/MWB cracked; base-14 codec proven; `vagcan labels` | ✅ done |
| **M3** | measurements → `MeasurementDef` catalog → generic CAN reader → config-selectable | 🟡 **21 rows proven (engine, gearbox, cluster) + three OBD-II services decoded from the standard; the gear and selector are read as states** |
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
| rod-crack | vag-data | `.rod` TEA-CBC + product/IV recovery in-tool (`vag-rod` bin); STRUC/DOP/TTTEXT/MWB inflate; **base-14 codec proven (disasm)** |
| struc-table | vag-data | `StrucTable`/`StrucRecord` + `decode_base14_be`; `mwb` parser; `measure` (proven ignition `0x5555`→0.0° anchor) |
| labels-cli | vagcan | `vagcan labels` — corpus inventory + `--part` / `--block` lookup |
| cli-app | vagcan | `devices` / `info` / `properties` / `sniff` / `scan` / `labels` (HEX-clone commands removed) |

## M3 — measurements from `.rod` (the current work)

### How VCDS reads measurements on this car (MQB / UDS), = our target
The ECU's `.rod` (`EV_ECM…rod`) is the source of truth: each measuring value =
`{ read DID, COMPU-method ref → DOP scaling, text-id → TTTEXT name }`. VCDS issues
`UDS 22 <DID>`, applies the COMPU method, prints `name = value unit`. Groups (`G004…`)
bundle DIDs. So the product needs the ECU `.rod` decoded into `(DID, scale, unit, name)`.

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

### Next lever — sniff VCDS on the bus (the live crib)
Every prior crib came from USB captures of the HEX clone, where the link cipher hides the
payload and VCDS's **group reads** — the source of RPM / vehicle speed / coolant — never
decoded (§4.0a/§4.0b). CAN is multi-drop, so a second adapter can sit on the same OBD-II
bus in listen-only mode while VCDS runs a normal session and record the whole conversation
**in the clear**, multi-frame group reads included.

Tooling (built, `docs/superpowers/specs/2026-07-31-can-sniffer-design.md`):
- `vagcan sniff --port <tty> --out cap.jsonl` — listen-only by default; streams every frame
  to a `vag-capture` JSONL headed by a wall-clock anchor, reassembles ISO-TP live, and takes
  operator markers from stdin. The anchor exists because the capture↔CSV lag had to be
  *guessed* last time (~52 s), which is how several "correlations" turned out to be
  window-fishing.
- `vagcan scan --ecu 01` — read-only sweep of the RDBI space; finds what the ECU exposes
  regardless of what any label file names.
- `vagcan properties --ecu 01` — the identification range, named.

The pairing to collect: sniff + VCDS running ADVMB logging to CSV, engine running, a wide
rev. That yields `(read address → raw bytes → displayed engineering value)` directly, with
no dependence on the `.rod` field codec.

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
2. **A parked identification pass driven off the gateway list** — read `2A26`, then
   `F187/F197/F19E/F189/F191` on every id in it. Names nine currently-unknown units and
   yields their label-file keys without moving.
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
alongside a live VCDS session; `vagcan analyse` proved three scalings, one of which
(coolant = `raw − 40`) reproduces the standard OBD-II PID 05 formula and thereby validates
the whole pipeline. Details in `research/rod-labels.md` §4.3.

**1a. More coverage — the next session.** The logs were only ~20 s each, giving 14–16
matched points against a 20-point default. Record VCDS logging for **several minutes** per
group, covering the measurements that matter (boost, load, throttle, speed), with a wide
sustained rev. Everything else is already built.

**2. `vagcan analyse` — BUILT (2026-08-01).** The offline tool that turns a capture into
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

**3. Names and the per-ECU measurement list from the `.rod`.** Scaling now comes from the
car, and after the 2026-08-02 linkage attempt (`research/label-linkage.md`,
`research/rod-labels.md` §4.4) that is settled: the corpus holds **no linear coefficients**,
its values are base-10 under a per-table glyph substitution, and the `MWB` code is a global
function of the text-id with no per-ECU degree of freedom. So the corpus is for **names and
per-ECU lists**, nothing more — unless `TTTEXT2.ROD` or `MUX.rod` turns out to hold a global
registry, the last two uncracked candidates.

Outstanding for names: crack `TTTEXT.ROD` (mechanical, but one to two hours on this machine,
not the minute the old note claims) and join `MWB → text-id → TTTEXT`. One lead worth
settling first: the gearbox VCDS logs carry a second number per column
(`Loc. IDE00022-ENG103074`), and `IDE00022 ↔ 7E9/380A` is proven — if `ENG######` is a
text-id, that is a direct `text-id ↔ proven identifier` pair. Currently suggestive only
(6 of 15 appear among 43,781 text-ids against an 18.2 % baseline, p ≈ 0.05).

**4. `vagcan watch` — BUILT.** Polls live at bus speed (46 Hz measured on the boost set)
using batched reads. Presets carry their own control unit. Anything unproven prints its
bytes tagged `(raw)` rather than a bare number.

**5. Discrete state — `vagcan discover`, BUILT; identification in progress.** Gear,
gearbox mode, switches and lamps cannot be fitted: a two-level value fits any line
exactly. `discover` classifies a `watch --out` recording into never-moved / stepped /
continuous and ranks the stepped columns, with `--pairs` for candidates whose transitions
coincide. Still to do: confirm which candidate is the gear (cross against the
input/output shaft speed ratio, both already proven), then the same for the cluster (17)
and ABS (03), which have not been swept yet.

### Then — the extensible foundation (architecture)
```
MeasurementDef { name, unit, address: Uds(did) | Group(g,field), raw_form, scale }
MeasurementCatalog::load_from_rod(ecu_rod) -> Vec<MeasurementDef>   // data, not code
read_measurement(&def, uds) -> (name, value, unit)                 // one generic path
```
Add a parameter = a data row / config selection, never new match-arms. Scaling comes
from `.rod` (when STRUC is reversed) or empirically from a live crib — unified.

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
2. 🔴 **`vagcan sniff` + VCDS in parallel**, VCDS logging ADVMB to CSV, engine running, wide
   rev. The trophy — see "Next lever" above. **Next action.**
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

**Validation oracle:** the owner's full Auto-Scan is in `research/vcds-rus-crack.md`
(VIN `XW8AD4NE9JH008917`, every ECU part-number/coding/VCID) — golden fixtures.

## Parked (designed, not being implemented now)
- **HEX-clone live UDS** — blocked by a VMProtect-sealed session KDF; a dead end for the
  multi-platform/CAN goal even if cracked (`research/clone-crypto.md`). The clone capture
  decoder (`research/clb-crack/extract_uds.py`) stays useful as an offline crib source.
- **OBD-II Mode 01** — dropped. The product reads VW measuring blocks from label files,
  not fixed SAE PIDs.
- **Cross-platform `no_std` core + `vag-runtime-*`** — spec + M1 plan under
  `docs/superpowers/{specs,plans}/2026-07-06-cross-platform-*`. Below-the-seam refactor.
