# vagcan — VAG CAN-bus diagnostics (Rust)

> **Read [`SAFETY.md`](SAFETY.md) before pointing this at a car you care about.**
> It only reads, and it has still cost one car its power steering.


A from-scratch, CLI-first diagnostics tool for VAG-group cars (VW / Audi / Škoda / SEAT),
targeting the **MQB** platform. Goal: talk to the car directly over the OBD-II cable — live
monitoring and fault-code reading — as an open alternative to VCDS / "Vasya".

Reference vehicle: Škoda Octavia III facelift, 1.8 TSI, 2017 (MQB, CAN/UDS).

Goal, stack and workflow: [`todo/GOAL.md`](todo/GOAL.md); live roadmap: [`todo/README.md`](todo/README.md).
(The original 2026-07-02 PRD is superseded and archived under `archive/specs/`.)

## Status (2026-08-01)

Goal: read the **whole car over CAN**, with measurement definitions as **data, not code**:
names from VW's own label files (`.lbl`/`.clb`/`.rod`), read address / scaling / unit
proven live on the car — so any value a block exposes is selectable, no hardcoded
addresses or formulas in Rust. Live product path = the **generic USB-CAN adapter**
(`vag-can`, slcan).
The HEX-clone cable is retired: its session KDF is VMProtect-sealed and dead, the
`vag-hex` crate and the vendored FTDI driver are deleted, and the research writeups
live on as negative results under `archive/research/`.

Milestones (see `todo/README.md` for the live task list):

| M | Component | State |
|---|---|---|
| M0 | ISO-TP + UDS + transport stack (read-only) | ✅ done |
| M1 | ECU identity + `vagcan info` (VIN + Engine/Gearbox passport, UDS RDBI) | ✅ **verified on the real car 2026-08-01** (VIN + both passports match the Auto-Scan oracle) |
| M2 | `.rod` decrypt+inflate in-tool; STRUC/DOP/TTTEXT/MWB all cracked; base-14 codec proven; `vagcan vcds labels` corpus tool | ✅ done |
| **M3** | **Measurements → `MeasurementDef` catalog → generic CAN reader → config-selectable** | 🟡 **the method works — 16 catalog rows proven across engine, gearbox and cluster; whole-car coverage is the open work** |
| HW | Generic USB-CAN (MKS CANable, slcan) bring-up on the car | ✅ live on the car — reads and writes at 500 kbit/s |

**Where it stands (M3):** the method works and is producing rows. Scaling does **not** come
from the label corpus — the read DID is provably not stored in `STRUC`
(`research/rod-labels.md` §4.0c, `research/label-linkage.md` §3) — it comes from the car:
`vagcan sniff` records a listen-only capture alongside a live VCDS session, `vagcan vcds analyse`
crosses it against the VCDS CSV and accepts only exact linear fits, and `vagcan recording calibrate`
extends coverage offline by fitting unknown raw columns against already-trusted references
in the same `watch` recording. Proven rows live in `catalogs/vehicles/<part number>.json`, one file per
control unit, keyed by what that unit reports about itself. Names come from the corpus: `TTTEXT.ROD` is cracked and `catalogs/names-uds.json`
carries 17,009 measurement names (`research/tttext-codec.md`) — but the corpus holds no
name→DID join, so a name match is a hypothesis to test on the car, not an answer.

Open: whole-car coverage — the units `vagcan survey` walks but the catalogs do not cover
yet (body control `0x70E`, cluster cold-start for the coolant scaling, the unidentified
units, deeper engine/gearbox) — and sourcing unit addresses from the corpus instead of a
table in code. See `todo/README.md`.

## Workspace

```
vag-transport   CAN frame + traits (RawCanTransport / IsoTpTransport) + scripted mock
vag-capture     JSON-lines capture format + ReplayCan (record-once / replay-forever testing)
vag-protocol    software ISO-TP (15765-2) + UDS client (14229), read-only allowlist
vag-can         slcan USB-CAN backend, listen-only mode, passive ISO-TP reassembly
vag-data        .lbl/.clb/.rod parsing+decrypt, TEA, LabelDb lookup, ODX file resolution
vag-db          SQLite cache of the label corpus (rusqlite)
```

Binaries: `vagcan` (the CLI) and `vag-db` (`build`/`lookup`/`stats`/`rod`). The
one-shot corpus tools that used to be separate binaries are subcommands now —
`vagcan vcds rod`, `vagcan vcds corpus`, `vagcan vcds tttext`.

## The CLI

```
vagcan devices      list connected USB-CAN adapters
vagcan info         VIN + engine and gearbox passports
vagcan units        ask the gateway which control units the car has
vagcan properties   everything a control unit says about itself, named
vagcan sniff        listen-only bus capture (runs alongside VCDS)
vagcan sensors      read the standard OBD-II sensors (27 live on the reference car)
vagcan watch        live values, full-screen TUI, multi-unit; `c` reconfigures in place
vagcan scan         every data identifier a control unit answers
vagcan faults       stored fault codes from every unit; --labels names them in VW's
                    own words; --details adds extended data
vagcan survey       walk every unit: identification, stored faults, identifier sweep
```

The top level is only what is worth having with the car in front of you. Everything
offline is grouped by what its input is:

```
vagcan recording calibrate   fit raw columns against trusted ones in a watch recording
vagcan recording discover    identifiers carrying discrete state (gear, mode, switches)

vagcan vcds labels           label lookup; --from-car resolves the ODX file a unit names
vagcan vcds names            search the 17,009 measurement names recovered from the corpus
vagcan vcds analyse          prove scalings from a capture + a VCDS log
vagcan vcds rod              decrypt + inflate a `.rod`, recovering blocked section keys
vagcan vcds corpus           parse a whole Labels/ directory into one JSON file
vagcan vcds tttext           recover names from the corpus's global text table
```

`--device` is optional: a recognised adapter is selected automatically. Read-only
throughout — the UDS allowlist admits no writes. `watch` draws a full-screen view on a
terminal and prints CSV without one; `--for SECONDS` picks the plain mode explicitly.

## Label ciphers (reverse-engineered for interoperability)

Ross-Tech's compiled label formats are undocumented. To read the owner's own vehicle's
measurement/DTC/adaptation definitions, the cipher was recovered from an **unpacked** build of
the VCDS binary (static analysis; no protection was circumvented and no software was patched):

- **`.clb` (modern)** — TEA (32-round, CBC) with `KEY_CLB` and a per-record IV.
- **`.rod`** — TEA-CBC with `KEY_ROD` (section-tag IV), compressed sections zlib-deflated.

`.rod` MWB rows are the UDS measurement *ID* index; human names live in `TTTEXT.ROD`,
whose glyph-substitution codec is broken (`research/tttext-codec.md`) — 17,009 names are
shipped in `catalogs/names-uds.json` and searchable with `vagcan vcds names`. The corpus holds
no name→DID join, so a name is a lead, not a binding.

The `.rod` per-record IV brute force is behind the `rod-crack` cargo feature
(`cargo run -p vagcan --features rod-crack -- vcds rod <file.rod>`).
`vagcan vcds labels` reads the cached results from `catalogs/rod-iv-cache.json` and caches the
parsed corpus to SQLite under `~/.vagcan/label-cache/` (`--refresh` rebuilds).

## Scope boundary

This project does **file-format and hardware-protocol interoperability** — reading the owner's
own car data with our own tool.

## Build / test

```
cargo test --workspace
cargo clippy --workspace --all-targets
```
