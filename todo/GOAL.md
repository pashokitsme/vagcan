# vagcan — Goal, tech stack, architecture, workflow

The single source of truth for what we're building and how. `/CLAUDE.md` links
here. The MVP task breakdown lives in `todo/README.md`.

## Goal

Read the **whole car over CAN** and show measurements by name/value/unit, with every
definition sourced from VW's own label files rather than hardcoded. Extensible foundation
first; UI later.

**Done and verified on the car (2026-08-01):** `vagcan info` prints the VIN and the engine
and gearbox passports, read live over a generic slcan USB-CAN adapter. The remaining work
is measurement *scaling* — see `todo/README.md` M3.

The original plan routed this through the owner's FTDI HEX cable. That path is parked: its
session crypto is a dead end, and a generic USB-CAN adapter reaches the same bus with no
crypto at all.

## Tech stack & architecture (locked)

- **Rust edition 2024, MSRV 1.85. Async runtime: tokio.**
- **Connection-actor architecture** (NOT `Arc<Mutex<device>>`): one cable = one
  actor task owning the byte pipe; N async tasks query N ECUs concurrently via
  bounded `mpsc<Request{pdu, oneshot<Reply>}>`; the actor multiplexes/pipelines
  over the single serial link, owning seq counter + per-channel link keystream +
  ISO-TP state + timeouts. Clients get concurrency (latency-hiding), not wire
  parallelism. Multiple cables later = actor-per-cable.
- **Pluggable backend, static dispatch:** `trait Backend { async fn read/write }`
  (native async-fn-in-trait, no `dyn`/`async-trait`). The live backend is `SlcanBackend`
  over a serial port (`vag-can`); the D2XX/HEX backend remains behind the same seam but is
  parked.
- `CableHandle` (cheap clone) implements the async transport `vag-protocol`'s UDS
  client rides. `vag-data`/`vag-db` stay sync (CPU-bound). **Label lookup must be
  FAST** (indexed/prepared SQLite or preloaded map; benchmark it).
- **Host = macOS Apple Silicon (M4).** Vendored darwin-arm64 D2XX in `driver/`.

## Development workflow

- **TDD wherever possible.** Every task ends with passing tests + `cargo clippy
  --all-targets -- -D warnings` clean.
- **Parallel dev with up to 4 subagents**, each on its **own git worktree**
  (isolation). The controller (main session) splits tasks, reviews each agent's
  diff/MR against its task brief, verifies, and merges. Never merge un-reviewed.
- **Hardware checkpoints:** at milestones with visible results on the real car
  (e.g. init handshake works; VIN read works), STOP and ask the user to verify on
  hardware before continuing.
- **Task tracking:** active tasks live in `todo/<subsystem>/<task>.md`; when a
  task is done+reviewed+merged, move its file to `done/<subsystem>/<task>.md`
  (preserve the subsystem subdir). Each subsystem dir may carry a short `README.md`.
