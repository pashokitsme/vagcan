# vagcan — Goal, tech stack, architecture, workflow

The single source of truth for what we're building and how. `/CLAUDE.md` links
here. The MVP task breakdown lives in `todo/README.md`.

## Goal

Ship **`vagcan info`** — prints VIN, vehicle name/model, and equipment (engine,
turbo?, gearbox kind+name, other basic info) read live from the owner's own car
via the owner's own FTDI HEX cable. Extensible foundation first; UI later.

## Tech stack & architecture (locked)

- **Rust edition 2024, MSRV 1.85. Async runtime: tokio.**
- **Connection-actor architecture** (NOT `Arc<Mutex<device>>`): one cable = one
  actor task owning the byte pipe; N async tasks query N ECUs concurrently via
  bounded `mpsc<Request{pdu, oneshot<Reply>}>`; the actor multiplexes/pipelines
  over the single serial link, owning seq counter + per-channel link keystream +
  ISO-TP state + timeouts. Clients get concurrency (latency-hiding), not wire
  parallelism. Multiple cables later = actor-per-cable.
- **Pluggable backend, static dispatch:** `trait Backend { async fn read/write }`
  (native async-fn-in-trait, no `dyn`/`async-trait`); `CableActor<B: Backend>`;
  runtime pick via `match config` at startup. **D2XX now** (blocking handle on a
  DEDICATED std::thread bridged to async via mpsc — never `spawn_blocking` per
  call, never in the reactor); **nusb seam for later.**
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
