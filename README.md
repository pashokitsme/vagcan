# vagcan — VAG CAN-bus diagnostics (Rust)

A from-scratch, CLI-first diagnostics tool for VAG-group cars (VW / Audi / Škoda / SEAT),
targeting the **MQB** platform. Goal: talk to the car directly over the OBD-II cable — live
monitoring and fault-code reading — as an open alternative to VCDS / "Vasya".

Reference vehicle: Škoda Octavia III facelift, 1.8 TSI, 2017 (MQB, CAN/UDS).

Design/PRD: [`docs/superpowers/specs/2026-07-02-vagcan-cli-design.md`](docs/superpowers/specs/2026-07-02-vagcan-cli-design.md).

## Status (2026-07-06)

Everything below is implemented, reviewed, and merged to `master` (tests green,
`cargo clippy --workspace` clean). See `todo/README.md` for the live roadmap.

| Component | Crate | State |
|---|---|---|
| ISO-TP + UDS protocol stack (read-only) | `vag-protocol`, `vag-transport`, `vag-capture` | ✅ done |
| `.lbl`/`.clb`/`.rod` label parse+decrypt, part-number→component lookup | `vag-data`, `vag-db` | ✅ done |
| HEX-clone wire framing + link cipher decode/encode (b8/b7, off14/off15, ISO-TP) | `vag-hex` | ✅ done |
| HEX-clone transport: cable opens & talks live on macOS (`FT_SetVIDPID`+FTDI init+clean-close) | `vag-hex` | ✅ done |
| Live drive: `doctor` / `probe` / `handshake` (auth-advance past 0x39) | `vagcan` | ✅ done |
| Session-replay reader: `replay-drive` (full ordered replay → f3 channel → VIN, + divergence report) | `vagcan` | ✅ done (untested on hw) |
| **Live UDS over the HEX-clone (session key `K_epoch`)** | — | 🔴 **blocked — VMProtect-sealed KDF; two live probes staged, see `todo/README.md`** |
| Generic USB-CAN bypass transport (slcan) | `vag-can` | 🟡 built, untested on hardware |
| **`vagcan info` (VIN + car + equipment)** | `vagcan` | ⬜ next — via generic CAN (Track A) |

**Where it stands:** the clone's encrypted diagnostic link needs a per-ECU AES session key
that the (VMProtect-packed) VCDS computes app-side — every *offline* route to it is exhausted
(`research/DYNAMIC-attack-RESULTS.md`). Two *live* probes are now staged for the owner to run:
(1) **`vagcan replay-drive`** — a full ordered session replay from a cold cable power-on that
tries to track the cable's state to the engine `f3` channel and read the VIN; if the cable's
state is not deterministic-from-power-on it emits a precise divergence report instead. (2) a
**VMProtect dynamic playbook** (`research/PATH2-vmprotect-dynamic-x86.md`) for a real x86 host —
HW-breakpoint / BCrypt-hook the live key (Tier A) or Pin+Triton-devirt the KDF (Tier B). The
extensible product path to `vagcan info` remains the **generic USB-CAN bypass** (`vag-can`,
ready) — talk UDS straight to the car, any ECU/DID.

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
