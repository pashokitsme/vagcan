# vagcan — VAG CAN-bus Diagnostics CLI (PRD / Design)

**Date:** 2026-07-02
**Status:** Approved design, pre-implementation
**Author:** Pavel Smirnov (with Claude)
**Working name:** `vagcan` (subject to rename)

---

## 1. Purpose

A convenient command-line utility for diagnostics over CAN-bus with VAG-group cars
(VW / Audi / Skoda / SEAT), targeting the **MQB** platform. It is a from-scratch,
CLI-first alternative to VCDS / "Vasya Diagnost", focused on ergonomics and scriptability.

**Reference test vehicle:** Skoda Octavia III (mk3) facelift, 1.8 TSI, 2017 (MQB, CAN/UDS).

### Primary goals (v1)

1. **Live monitoring** of engine/vehicle parameters across multiple ECUs simultaneously:
   turbo boost pressure, engine RPM, vehicle speed, ignition timing angle, and
   temperatures (coolant, engine oil, gearbox/DSG oil, intake air), extensible to more
   measurements defined later.
2. **Fault (DTC) reading** from all modules, with rich output: description, status
   (active / stored / pending / intermittent), freeze-frame data, mileage and timestamp,
   occurrence count.
3. **CAN-packet grabber / sniffer** mode for development & debugging: raw-frame capture
   with ISO-TP reassembly + UDS decoding, plus log-to-file.

### Explicit non-goals (v1)

- No write operations: no coding, adaptation, flashing, ECU reset, or clear-DTC.
  Read-only, enforced. (Write kept as a future architectural slot — see §7.)
- No K-line / older non-CAN protocols.
- No parallel diagnostics across multiple physical buses.

---

## 2. Context & constraints

- **Tech stack:** Rust, developed on MacBook M4 (ARM64 / Apple Silicon).
- **Adapter (owned):** VAG25.3 "Dual-K & CAN <-> USB" cable — a **Ross-Tech HEX clone**.
  - Confirmed: HEX cables do **not** expose a dumb serial pass-through and speak a
    **proprietary, undocumented Ross-Tech protocol** over an FTDI USB-serial chip
    (not ELM327, not SLCAN, not J2534). See §3.
  - Consequence: the cable driver must be **reverse-engineered** (Phase 1). This is the
    single highest-risk item; the architecture isolates it behind a transport trait so the
    rest of the stack is built and tested independently.
- **RE environment:** VCDS is Windows-only → user will run a **Windows VM on the Mac** with
  **FTDI USB passthrough**, and capture PC↔cable traffic while VCDS drives the cable.
- **Data source for measurements/DTCs:** the user's VCDS install, especially the `Labels/`
  directory (`.clb` / `.lbl`) which maps measuring-block/DID IDs → human names, units, and
  scaling formulas — lifted into `vag-data` to avoid re-deriving the data layer.

### MQB protocol facts (from research)

- MQB diagnostics = **UDS (ISO 14229) over ISO-TP (ISO 15765-2) over CAN**, exposed on the
  OBD-II port through the **Gateway** module.
- The gateway keeps buses **silent on the OBD port until a diagnostic session is opened** to
  a specific module. → a passive sniffer on the OBD port mostly sees *our own* diagnostic
  traffic, not the car's internal powertrain CAN chatter. (Documented caveat for `sniff`.)
- Addressing: VAG uses 11-bit normal addressing in the `0x700–0x7FF` range and/or 29-bit
  extended IDs (`0x18DAxxF1` style). Exact IDs per module confirmed during RE.
- **Safety / S3 timer:** in a non-default session the tester must send **TesterPresent
  (0x3E)** at least every ~5 s or the ECU falls back to default session. `0x22` reads often
  work in the default session; keepalive used when an extended session is required.

---

## 3. Key decisions (agreed)

| # | Decision | Choice |
|---|----------|--------|
| 1 | RE scope | **Minimal** — RE only: open session, CAN-mode select, ISO-TP send/recv to one ECU. No K-line, no write. Read-only now, **write kept as architectural slot**. |
| 2 | UDS/ISO-TP stack | **Roll our own** lean UDS + software ISO-TP. `ecu_diagnostics` (rnd-ash) used only as a reference for formulas/ideas, not a dependency. |
| 3 | CLI UX | **Hybrid** — `scan`/`dtc` = plain pipeable output; `monitor` = live ratatui TUI; `sniff` = live decode + log file. |

---

## 4. Architecture

Cargo **workspace**, 6 crates, strictly layered (depends only downward):

```
vag-cli        bin: clap subcommands + ratatui TUI
   │
vag-core       session mgr, multi-ECU poll scheduler, TesterPresent keepalive
   │  ├── vag-data     DID/DTC/module DB, VCDS-label parser, byte→physical scaling
   │  └── vag-protocol UDS client (ISO 14229) + software ISO-TP (ISO 15765-2)
   │            │
vag-transport  TRAIT crate: RawCanTransport + IsoTpTransport (+ error types)
   │
vag-hex        RE'd Ross-Tech-clone driver over FTDI serial (impls transport traits)
```

### Transport abstraction (the swap point)

Two traits, because the layer the HEX cable exposes is unknown until RE:

- **`RawCanTransport`** — send/recv raw CAN frames. Used by the sniffer and by software ISO-TP.
- **`IsoTpTransport`** — send/recv full ISO-TP PDUs.

`vag-protocol` provides a **software ISO-TP** adapter turning any `RawCanTransport` into an
`IsoTpTransport`. If RE reveals the cable performs ISO-TP framing internally, `vag-hex`
implements `IsoTpTransport` directly and software ISO-TP is bypassed. **UDS only ever depends
on `IsoTpTransport`.**

Adding a future adapter (e.g. `vag-slcan`, `vag-obdlink`) = a new crate implementing the same
traits — zero changes above `vag-transport`.

### Crate responsibilities

- **vag-transport** — traits + typed errors only, no IO. The stable contract.
- **vag-hex** — Phase-1 RE deliverable. Talks FTDI via the `serialport` crate. Encodes/decodes
  the reverse-engineered Ross-Tech framing. Codec isolated behind a `Cable` type so recorded
  captures can be replayed in tests without hardware.
- **vag-protocol**
  - `isotp` module: single / first / consecutive frames, flow control, sequence + timeout
    handling (ISO 15765-2).
  - `uds` module: services v1 = `0x10` DiagnosticSessionControl, `0x3E` TesterPresent,
    `0x22` ReadDataByIdentifier, `0x19` ReadDTCInformation; full NRC decoding including
    `0x78` responsePending (→ wait). Write services (`0x2E`, `0x31`, `0x27`, `0x11`, `0x14`)
    are **defined as types but gated behind `feature = "write"`, off by default**.
- **vag-data** — module-address table (CAN ID ↔ engine / ABS / cluster / transmission / …),
  DID → (name, unit, scaling formula) map, DTC code → description. Bootstrapped from the VCDS
  `Labels/` parser plus a built-in set for the Octavia 1.8 TSI. Pure data + scaling functions.
- **vag-core** — connect / teardown, per-ECU session state, round-robin poll scheduler over
  the single physical bus, TesterPresent keepalive (< 5 s S3 timer).
- **vag-cli** — subcommands `scan`, `dtc`, `monitor` (TUI), `sniff`, and a global
  `--replay <capture>` mode.

---

## 5. Data flow

- **`monitor`** — scheduler round-robins the requested DIDs across active ECUs → UDS `0x22`
  → ISO-TP → cable → CAN → ECU → response back up → `vag-data` scales raw bytes to physical
  values → ratatui gauges. A single physical bus means requests are **interleaved, not truly
  parallel**; the scheduler prioritizes. TesterPresent held per active ECU.
- **`dtc`** — for each known module: UDS `0x19 / 0x02` (report DTCs by status mask) → parse
  code + status bits (active / stored / pending / intermittent) → `0x19 / 0x04` and `/ 0x06`
  for snapshot + extended data (**mileage, timestamp, freeze-frame, occurrence count**) →
  `vag-data` → formatted table.
- **`sniff`** — cable raw-CAN mode → frame stream → live decode (ISO-TP reassembly + UDS
  interpretation) → TUI + optional `.csv` / `.log`. **Caveat:** MQB gateway is silent on the
  OBD port until a session opens, so mostly our own diagnostic traffic is visible; capturing
  the car's internal buses would require tapping behind the gateway (out of scope v1).

---

## 6. Error handling & safety

- Typed errors per layer (`thiserror` in libs; `miette` for pretty CLI diagnostics).
- Transport IO / cable-drop → reconnect policy. ISO-TP flow-control timeout / sequence errors
  → typed. UDS negative responses → NRC decoded to meaning; `0x78` responsePending → wait & retry.
- **Hard read-only guarantee (v1):** outgoing UDS service IDs are checked against an
  **allowlist at the transport boundary**; any mutating service is rejected unless the
  `write` feature is compiled in (it is not for v1). No ECUReset, no clear-DTC, no coding, no
  flashing.
- Operational guidance (docs): ignition on / engine off for reads, stable battery voltage,
  do not run diagnostics while driving.

---

## 7. Future write-slot (design-only, not built)

Write services already exist as typed request/response structures behind
`feature = "write"`. Enabling later requires: (a) the feature flag, (b) removing the mutating
service from the read-only allowlist, (c) SecurityAccess (`0x27`) support, (d) explicit
per-command user confirmation. No architectural change needed — this is the "slot" agreed in
Decision 1.

---

## 8. Testing strategy — "capture once, replay forever"

**Claude never connects to the car.** The user runs captures on the Skoda, shares the log
files; all codec/parser work is built and tested against those files offline.

**Tier 1 — no car, no cable (majority of dev).**
Mock transports + recorded replays. All ISO-TP, UDS, data scaling, DTC parsing, CLI, TUI —
unit + integration tests, run in CI on the Mac.

**Tier 2 — car needed briefly, once per artifact.**
Plug in, run a session, **record raw bytes** to a `*.capture` fixture that becomes a
permanent test:
- RE (Phase 1): VCDS drives the cable while the VM logs PC↔cable traffic → logs are both the
  RE input and the codec test fixtures.
- Each ECU / DID / DTC: capture a real response once → replay-test forever.

**Tier 3 — car live (rare).**
Only for initial RE discovery and final per-feature acceptance ("does it work on the
Octavia"). User runs the built binary, pastes output/logs.

### Capture / replay harness (P0 deliverable)

- **`vag-capture`** record format: a timestamped, versioned log of transport-level
  exchanges (both raw-CAN frames and cable byte-streams), plus metadata (adapter, direction,
  session context). Human-inspectable (text/JSON-lines) so RE captures are diff-able.
- **`--replay <capture>`** global CLI mode: runs the full stack against a recorded session
  with **no hardware**, deterministic.
- **`MockCable` / mock transports**: replay fixtures at the transport boundary for
  crate-level tests.

This loop exists from day one so every subsequent feature is developed file-driven.

### Per-layer tests (TDD)

- **isotp** — mock `RawCanTransport` (in-memory frame pairs): multi-frame reassembly, flow
  control, timeouts.
- **uds** — mock `IsoTpTransport` with canned responses: RDBI / DTC parse, NRC paths.
- **data** — parse a sample VCDS label file, assert DID scaling + units.
- **vag-hex** — record real RE captures as byte fixtures, replay-test the framing codec offline.
- **integration** — `--replay` runs the whole stack against recorded sessions in CI.

---

## 9. Build phases

- **P0 — Scaffold + protocol on mocks (no hardware).** Workspace, `vag-transport` traits,
  mock transport, `vag-capture` record/replay harness, full ISO-TP + UDS green against mocks.
- **P1 — RE the HEX clone (highest risk, isolated).** Windows-VM capture → decode framing →
  `vag-hex` driver → talk to a real ECU.
- **P2 — Data DB.** VCDS `Labels/` parser + DID / DTC / module tables (Octavia 1.8 TSI first).
- **P3 — CLI.** `scan`, `dtc` output, `monitor` TUI.
- **P4 — Sniffer.** `sniff` live decode + log.

P0 before P1 lets the entire protocol stack be built and tested on mocks while RE proceeds in
parallel, so RE risk cannot block everything else.

---

## 10. Open items (resolved during implementation)

- Exact per-module CAN IDs for the Octavia mk3 — confirmed via RE + `scan`.
- Whether the cable exposes raw CAN or pre-framed ISO-TP — determined in P1; both paths designed.
- Exact `.clb` / `.lbl` label format — inspected from the user's VCDS install in P2.
- Whether `0x22` reads need an extended session (keepalive) or work in default — confirmed in P1.

---

## 11. Reference sources

- UDS 0x19 ReadDTCInformation: <https://piembsystech.com/read-dtc-information-service-0x19-uds-protocol/>,
  <https://www.csselectronics.com/pages/uds-protocol-tutorial-unified-diagnostic-services>
- MQB diagnostic protocol / framing: <https://www.elektroda.pl/rtvforum/topic3864647.html>
- Rust reference crate `ecu_diagnostics`: <https://github.com/rnd-ash/ecu_diagnostics>
- VW UDS / ISO-TP addressing: <https://github.com/bri3d/VW_Flash/blob/master/docs/docs.md>,
  <https://icanhack.nl/knowledge-base/diagnostics/uds/>
- HEX cable proprietary-protocol confirmation: <https://www.ross-tech.com/vcds/hex-v2.php>,
  <https://www.hex.co.za/clones/>
- CAN ID collections: <https://github.com/iDoka/awesome-automotive-can-id>
