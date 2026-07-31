# vagcan — VAG CAN-bus diagnostics (Rust)

A from-scratch, CLI-first diagnostics tool for VAG-group cars (VW / Audi / Škoda / SEAT),
targeting the **MQB** platform. Goal: talk to the car directly over the OBD-II cable — live
monitoring and fault-code reading — as an open alternative to VCDS / "Vasya".

Reference vehicle: Škoda Octavia III facelift, 1.8 TSI, 2017 (MQB, CAN/UDS).

Design/PRD: [`docs/superpowers/specs/2026-07-02-vagcan-cli-design.md`](docs/superpowers/specs/2026-07-02-vagcan-cli-design.md).

## Status (2026-07-31)

Goal: read the **whole car over CAN**, with measurement definitions (name / read
address / scaling / unit) sourced **from VW's own label files** (`.lbl`/`.clb`/`.rod`) —
the way VCDS does — so any value a block exposes is selectable, no hardcoded addresses
or formulas. Live product path = the **generic USB-CAN adapter** (`vag-can`, slcan);
the HEX-clone is parked (its USB-side link crypto is a dead end for this goal).

Milestones (see `todo/README.md` for the live task list):

| M | Component | State |
|---|---|---|
| M0 | ISO-TP + UDS + transport stack (read-only) | ✅ done |
| M1 | ECU identity + `vagcan info` (VIN + Engine/Gearbox passport, UDS RDBI) | 🟡 built, **mock-tested only — NOT yet verified on the real car** |
| M2 | `.rod` decrypt+inflate in-tool; STRUC/DOP/TTTEXT/MWB all cracked; base-14 codec proven; `vagcan labels` corpus tool | ✅ done |
| **M3** | **Measurements from `.rod` → `MeasurementDef` catalog → generic CAN reader → config-selectable** | 🔴 **current — the offline path is refuted; now needs a live crib** |
| HW | Generic USB-CAN (MKS CANable, slcan) bring-up on the car | 🟡 bench-verified (slcan firmware, responsive); **not yet on the car** |

**Where it stands (M3):** every `.rod` label table is decrypted and inflated inside our own
tool (`vag-data`, `vag-rod` bin) and the packed payload codec is proven to be **base-14**
(disasm-verified). What is NOT reversed is the **STRUC field segmentation** — and the
supervised attack that was supposed to crack it instead proved the read DID is **not stored
in STRUC at all** (`research/rod-labels.md` §4.0c). Offline RE is exhausted.

So M3 no longer waits on the label corpus; it waits on **live evidence**. Prior cribs came
from USB captures of the HEX clone, where the link cipher hides the payload and VCDS's
multi-frame group reads — the source of RPM / speed / coolant — never decoded. CAN is
multi-drop, so a second adapter listens on the same OBD-II bus while VCDS runs and records
the whole conversation in the clear. That tooling now ships: `vagcan sniff` (listen-only
capture with live ISO-TP reassembly and a wall-clock anchor) and `vagcan scan-dids`
(read-only sweep of what an ECU actually exposes). Both are hardware-ready; see
`docs/superpowers/specs/2026-07-31-can-sniffer-design.md` and the checkpoint order in
`todo/README.md`.

One empirical anchor stands from the earlier captures: ignition-angle raw `0x5555` = `0.00°`.

## Workspace

```
vag-transport   CAN frame + traits (RawCanTransport / IsoTpTransport) + scripted mock
vag-capture     JSON-lines capture format + ReplayCan (record-once / replay-forever testing)
vag-protocol    software ISO-TP (15765-2) + UDS client (14229), read-only allowlist
vag-data        .lbl/.clb/.rod parsing+decrypt, TEA, LabelDb lookup, load_corpus
vag-db          SQLite cache of the label corpus (rusqlite)
```

Binaries: `vag-labels` (parse a Labels dir → JSON / lookup), `vag-db` (`build`/`lookup`/`stats`/`rod`).

## Label ciphers (reverse-engineered for interoperability)

Ross-Tech's compiled label formats are undocumented. To read the owner's own vehicle's
measurement/DTC/adaptation definitions, the cipher was recovered from an **unpacked** build of
the VCDS binary (static analysis; no protection was circumvented and no software was patched):

- **`.clb` (modern)** — TEA (32-round, CBC) with `KEY_CLB` and a per-record IV.
- **`.rod`** — TEA-CBC with `KEY_ROD` (section-tag IV), compressed sections zlib-deflated.

`.rod` MWB rows are the UDS measurement *ID* index; human names live in `TTTEXT.ROD` (a
future join, optional — `.clb` already yields readable MQB engine labels).

## Scope boundary

This project does **file-format and hardware-protocol interoperability** — reading the owner's
own car data with our own tool.

## Build / test

```
cargo test --workspace
cargo clippy --workspace --all-targets
```
