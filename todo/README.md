# vagcan — roadmap & status

**Goal:** read the **whole car over CAN** and show measurements by name/value/unit,
with every definition (name, read address, scaling, unit) taken **from VW's own label
files** (`.lbl`/`.clb`/`.rod`) — exactly like VCDS — so any value a block exposes is
selectable from config, with **no hardcoded addresses or formulas**. Live transport =
the **generic USB-CAN adapter** (`vag-can`, slcan). See `/CLAUDE.md` for the locked
stack and `todo/GOAL.md` for the goal statement.

## Status (2026-08-01)

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
| **M3** | measurements from `.rod` → `MeasurementDef` catalog → generic CAN reader → config-selectable | 🔴 **current — offline path refuted; now needs live crib** |
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

### The remaining work, in order

**1. The capture session (blocked on nothing — both adapters now run together).**
`vagcan sniff --out <file>` alongside VCDS, engine running, ADVMB logged to CSV, a wide
sustained rev twice over, operator markers typed as it goes.

**2. `vagcan analyse` — the offline tool that turns that capture into scalings.** To be
written *before* the session, so the data can be checked while the car is still available.
It must:
- read the capture JSONL and the VCDS CSV, and align them by the **wall-clock anchor** —
  arithmetic, not curve fitting. Fitting the offset is what produced the false correlations
  in §4.0a/§4.0b;
- reassemble ISO-TP, group by read identifier, and build a raw time series per identifier,
  including the multi-frame group reads;
- for each (identifier interpretation × logged measurement) pair, fit `factor`/`offset` by
  least squares — and **reject** anything below a stated threshold rather than shipping a
  forced fit. A refusal is a result;
- emit accepted rows as `MeasurementDef` catalog entries (`vag_data::catalog`), which the
  existing `UdsReadExt::read_catalog` can already read.

**3. Names and the per-ECU measurement list from the `.rod`.** Scaling now comes from the
car, but the label corpus still supplies what a value is *called* and which values an ECU
exposes. `F19E` already resolves the right file (§ODX link above).

**4. `vagcan watch`** — poll a catalog and print live values. The product UX; pointless
until step 2 has proven scalings, so it stays last.

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
4. ✅ **`vagcan scan`** — swept `F100-F1FF` on both units, 2026-08-01.

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
