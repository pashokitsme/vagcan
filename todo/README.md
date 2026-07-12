# vagcan — roadmap & status

**Goal:** read the **whole car over CAN** and show measurements by name/value/unit,
with every definition (name, read address, scaling, unit) taken **from VW's own label
files** (`.lbl`/`.clb`/`.rod`) — exactly like VCDS — so any value a block exposes is
selectable from config, with **no hardcoded addresses or formulas**. Live transport =
the **generic USB-CAN adapter** (`vag-can`, slcan). See `/CLAUDE.md` for the locked
stack and `todo/GOAL.md` for the goal statement.

## Status (2026-07-13)

The protocol stack, the identity reader, and the whole `.rod` label-decrypt pipeline are
built and merged. The one wall between here and the product is the **STRUC field
segmentation** inside the `.rod` measurement structures.

### Milestones
| M | what | state |
|---|------|-------|
| M0 | ISO-TP + UDS + transport stack (read-only allowlist) | ✅ done |
| M1 | `vagcan info` — VIN + Engine/Gearbox identity (UDS RDBI) | 🟡 **mock-tested only — NOT verified on the real car** |
| M2 | `.rod` decrypt+inflate in-tool; STRUC/DOP/TTTEXT/MWB cracked; base-14 codec proven; `vagcan labels` | ✅ done |
| **M3** | measurements from `.rod` → `MeasurementDef` catalog → generic CAN reader → config-selectable | 🔴 **current — blocked on STRUC segmentation** |
| HW | generic USB-CAN (MKS CANable) bring-up on the car | 🚚 dongle shipping next week |

### Done (merged to `master`, tests green, clippy clean)
| subsystem | crate | what |
|-----------|-------|------|
| async-core | vag-transport | async transport trait(s) + mock, error model |
| uds-async | vag-protocol | async ISO-TP (15765-2) + UDS client (14229), read-only allowlist |
| generic-can | vag-can | `SlcanBackend` + `IsoTpCan` (the bypass transport — built, untested on hw) |
| info-identity | vag-protocol/vagcan | `EcuIdentity` + `read_identity` + `vagcan info` (Engine 01 + Gearbox 02). **Mock-tested; live run pends the CANable** |
| label-corpus | vag-data/vag-db | `.lbl`/`.clb` parse+decrypt, `.rod` decrypt+inflate, `LabelDb` lookup, `load_corpus`/`scan_corpus` |
| rod-crack | vag-data | `.rod` TEA-CBC + product/IV recovery in-tool (`vag-rod` bin); STRUC/DOP/TTTEXT/MWB inflate; **base-14 codec proven (disasm)** |
| struc-table | vag-data | `StrucTable`/`StrucRecord` + `decode_base14_be`; `mwb` parser; `measure` (proven ignition `0x5555`→0.0° anchor) |
| labels-cli | vagcan | `vagcan labels` — corpus inventory + `--part` / `--block` lookup |
| cli-app | vagcan | `doctor` / `decode` / `probe` / `handshake` / `replay-drive` / `info` / `labels` |

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

### Next lever — supervised STRUC attack with the owner's capture (crib × STRUC)
The prior passes never combined the two: the capture crib was only used to *fit RPM
scaling* (failed — those values ride undecodable group reads), and STRUC was reversed
*without* the crib. Cross them:
1. Owner's engine-running captures (`research/dumps/`, gitignored) decode to **real valid
   engine DIDs** (`A058/A059/A05E/A05F`,`A03B`,`A051`,`7410…7458`,`82D4`,`A0EF`) + VCDS's
   displayed names/units/values (CSV logs). One anchor proven: `A058/9/E/F` raw `0x5555`
   = `0.00°`.
2. **Search the decoded STRUC / engine-`.rod` records for those known DID bytes** → reveals
   the offset of the `read_id` field.
3. Known scaling (`0x5555`→0°) + unit/name from CSV → pin the `scale` / `unit-ref` /
   `name-ref` fields. Multiple anchors cross-confirm the layout.
4. Once segmentation falls → decode all STRUC → the `MeasurementDef` catalog.

Requires the VCDS data volume mounted (`research/vcds-data/en/UDS_EV/STRUC.rod` +
`EV_ECM…rod`).

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
`SlcanBackend`, no new backend. Before first use: (a) ensure **slcan** firmware not
candleLight (candleLight = gs_usb = Linux-only, reflash via BOOT+DFU) → enumerates as
`/dev/cu.usbmodem*`; (b) wire OBD2 pin 6→CAN-H, 14→CAN-L, 4/5→GND, **do NOT** wire pin 16;
(c) **TERM jumper OFF**. Open with `SlcanBackend::open("/dev/cu.usbmodem*", baud, Rate500k)`.

1. slcan dongle: raw CAN frame TX/RX with the car at 500 kbit/s.
2. **`vagcan info --port <tty>` prints the real VIN + Engine/Gearbox identity** — this is
   the M1 live-verification that is still outstanding. Confirm the F187-spaces,
   DQ200-session, coding-DID caveats here.
3. If STRUC is still stuck: a live-engine capture (engine polling measuring blocks) gives
   more `(DID → raw → value)` anchors for the supervised attack.

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
