# vagcan — MVP roadmap (`vagcan info`)

Goal: `vagcan info` prints VIN, vehicle name/model, and equipment (engine,
turbo?, gearbox kind+name, other basic info) read live from the car via the FTDI
HEX cable. See `/CLAUDE.md` for locked stack/architecture decisions.

Task files live in `todo/<subsystem>/NN-<task>.md`. When a task is done,
reviewed, and merged, its file moves to `done/<subsystem>/NN-<task>.md`.

## Subsystems

| subsystem | crate(s) | responsibility |
|-----------|----------|----------------|
| `async-core` | vag-transport | async transport trait(s) + async mock, error model |
| `usb-backend` | vag-hex | `Backend` trait + `D2xxBackend` (blocking handle on dedicated thread) |
| `cable-actor` | vag-hex | `CableActor<B>` + `CableHandle` (mpsc/oneshot multiplex over one link) |
| `link-transport` | vag-hex | port link cipher to Rust; b8/b7 diagnostic encode/decode |
| `init-handshake` | vag-hex | open-time handshake (02/09/04 identify, b0..b5 setup) |
| `uds-async` | vag-protocol | async ISO-TP + UDS client over the new transport |
| `label-lookup` | vag-db, vag-data | FAST part-number/coding → component-name lookup |
| `vin-info` | new `vagcan` crate | VIN read + VIN decode + equipment assembly |
| `cli-app` | new `vagcan` crate | `vagcan` bin + `info` subcommand (JoinSet concurrency) |
| `research-keystream` | research/ | reverse the 16-key link-cipher keystream schedule |

## Dependency waves (respect these when scheduling agents)

- **Wave 1 (independent — start in parallel, ≤4 agents):**
  `async-core` (vag-transport), `usb-backend` (vag-hex), `label-lookup`
  (vag-db/vag-data), `research-keystream` (research/). Different crates/dirs → no
  worktree conflicts.
- **Wave 2 (after wave 1 merges):** `cable-actor` (needs async-core trait +
  usb-backend Backend + existing frame.rs), `link-transport` (needs frame + the
  keystream port), `uds-async` (needs async-core trait).
- **Wave 3:** `vin-info` (needs uds-async + link-transport + label-lookup),
  `init-handshake` (needs usb-backend + cable-actor + frame), then `cli-app`
  wires it into `vagcan info`.

## Hardware checkpoints (STOP, ask user to verify on the real car)

1. **Cable open + init handshake** → cable reaches "ready", identity string
   ("ROSSTECH" + version) read back. (after `usb-backend` + `init-handshake`)
2. **VIN read** → `vagcan info` prints the real VIN. (after `vin-info`)

Do not proceed past a checkpoint until the user confirms the hardware result.

## Blockers (gate LIVE end-to-end, not fixture/TDD work)

- 16-key keystream **schedule** un-reversed → `research-keystream` track.
- VIN `22 F1 90` absent from the existing capture → need a fresh capture or the
  live hardware checkpoint for VIN fixtures.
