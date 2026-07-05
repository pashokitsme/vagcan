# vagcan — VAG CAN-bus diagnostics (Rust)

A from-scratch, CLI-first diagnostics tool for VAG-group cars (VW / Audi / Škoda / SEAT),
targeting the **MQB** platform. Goal: talk to the car directly over the OBD-II cable — live
monitoring and fault-code reading — as an open alternative to VCDS / "Vasya".

Reference vehicle: Škoda Octavia III facelift, 1.8 TSI, 2017 (MQB, CAN/UDS).

Design/PRD: [`docs/superpowers/specs/2026-07-02-vagcan-cli-design.md`](docs/superpowers/specs/2026-07-02-vagcan-cli-design.md).

## Status (2026-07-03)

Everything below is implemented, reviewed, and merged to `master` (13 test binaries green,
`cargo clippy --workspace` clean).

| Component | Crate | State |
|---|---|---|
| ISO-TP + UDS protocol stack (read-only) | `vag-protocol`, `vag-transport`, `vag-capture` | ✅ done |
| `.lbl` label parser | `vag-data` | ✅ done |
| `.clb` decrypt (TEA-CBC) | `vag-data` | ✅ done |
| `.rod` decoder (TEA-CBC + zlib) | `vag-data` | ✅ done |
| Part-number → measurement lookup (`REDIRECT` resolution) | `vag-data` (`LabelDb`) | ✅ done |
| SQLite corpus cache | `vag-db` | ✅ done |
| **Cable transport (reverse-engineer the clone HEX cable)** | `vag-hex` | 🔴 **not started — blocked on a USB capture** |
| `monitor` / `dtc` / `sniff` commands + TUI | `vag-cli`, `vag-core` | ⬜ future (needs the transport) |
| Run-logger / drag mode | — | ⬜ future ([spec §12](docs/superpowers/specs/2026-07-02-vagcan-cli-design.md)) |

**The one thing between here and reading the car** is the cable transport (P1): reverse-engineer
the cable's own USB/serial protocol so `vagcan` can drive it. That needs a USB capture of
VCDS↔cable traffic, taken on a working VCDS install. Nothing else is blocking.

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
