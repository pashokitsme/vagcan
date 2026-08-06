# vagcan — Goal, tech stack, architecture, workflow

The single source of truth for what we're building and how. `/CLAUDE.md` links
here. The MVP task breakdown lives in `todo/README.md`.

## Goal

Read the **whole car over CAN** and show measurements by name/value/unit. Names come from
VW's own label files (`~/.vagcan/labels/names.json`); scaling is proven **live on the car** —
the label files provably does not carry the read identifier (`research/labels/rod-labels.md` §4.0c,
`research/labels/label-linkage.md` §3). Extensible foundation first; UI later.

**Done and verified on the car (2026-08-01):** `vagcan info` prints the VIN and the engine
and gearbox passports, read live over a generic slcan USB-CAN adapter; 16 measurement rows
are proven across engine, gearbox and cluster and watched live in the `vagcan watch` TUI.
The remaining work is whole-car *coverage* — see `todo/README.md` M3.

The original plan routed this through the owner's FTDI HEX cable. That path is dead: its
session KDF is VMProtect-sealed, the `vag-hex` crate and the vendored driver are deleted,
and the research is archived under `archive/research/` as negative results. A generic
USB-CAN adapter reaches the same bus with no crypto at all.

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
  over a serial port (`vag-can`); any future backend implements the same seam.
- `CableHandle` (cheap clone) implements the async transport `vag-protocol`'s UDS
  client rides. `vag-data`/`vag-db` stay sync (CPU-bound). **Label lookup must be
  FAST** — `vagcan vcds labels` caches the parsed label files to SQLite under
  `~/.vagcan/labels/cache.sqlite`.
- **Host = macOS Apple Silicon (M4).**

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
  task is done+reviewed+merged, move its file to `archive/tasks/done/<subsystem>/<task>.md`
  (preserve the subsystem subdir). Each subsystem dir may carry a short `README.md`.
