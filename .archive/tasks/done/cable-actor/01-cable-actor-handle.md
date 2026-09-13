# cable-actor / 01 — CableActor + CableHandle (mpsc/oneshot multiplex)

**Subsystem:** cable-actor · **Crate:** `vag-hex` · **Wave:** 2 · **Depends:** async-core (done), usb-backend (done), frame (done)

## Goal
The connection-actor: one tokio task owns a `Backend`, multiplexes N concurrent
requests over the single link, and hands out cheap-clone handles. This is the
core of the locked architecture (see `todo/GOAL.md`).

## Context
- `vag_transport::AsyncIsoTpTransport` (async trait) and `MockAsyncTransport` exist.
- `vag_hex::usb::Backend` (async `write`/`read`) + `D2xxBackend` exist. Tests use an
  in-memory fake `RawDevice`; you can build a mock `Backend` similarly for actor tests.
- `vag_hex::frame` has the plaintext `S/M` frame: `frame_encode(marker, opcode, data)`,
  `frame_decode`, and `take_frame(buf, marker)` (resyncing stream cutter). The DIAGNOSTIC
  (UDS) path is enciphered (`frame::encode`/`decode` gated on the link keystream) — DO NOT
  implement the encrypted path here; this task is the multiplex machinery + the PLAINTEXT
  request/response path (usable for the init handshake + the hardware checkpoint).

## Deliverables (new modules in `vag-hex`)
- `CableActor<B: Backend>` (static dispatch, generic over backend): owns `B`, a `tokio`
  task loop that `select!`s over an inbound `mpsc::Receiver<Request>` and drives the wire.
- `Request` = `{ frame_bytes: Vec<u8>, reply: oneshot::Sender<Result<Frame, HexError>> }`
  (or a higher-level `{ opcode, payload }` — your call, but keep the actor owning framing).
- `CableHandle` (cheap `Clone`, holds the `mpsc::Sender`): a method
  `async fn request(&self, opcode: u8, payload: &[u8], timeout: Duration) -> Result<Frame, HexError>`
  that sends a host `S/M` frame and awaits the cable's reply frame (matched by the strict
  OUT→reply ordering the actor enforces; the actor owns any seq counter).
- The actor's recv side accumulates bytes from `Backend::read` into a buffer and uses
  `frame::take_frame(buf, MARKER_CABLE)` to cut complete reply frames; apply `timeout`
  via `tokio::time::timeout`.
- `spawn(backend: B) -> CableHandle` (spawns the task, returns a handle). Dropping all
  handles closes the mpsc → actor exits → backend dropped.
- Do NOT implement `AsyncIsoTpTransport` for the encrypted UDS path yet (gated on the link
  keystream + session key). A `// TODO(link-cipher)` where that impl will live is fine.

## TDD (no hardware)
1. Build a mock `Backend` (in-memory: canned reply bytes for given written frames).
2. Test: `CableHandle::request(0x04, &[], t)` (identify) → the actor writes `53 04 04 <xor>`,
   the mock returns a `4d ...` reply frame, and `request` resolves with the decoded `Frame`.
3. Test concurrency: spawn 2+ `request` futures on one handle via `tokio::join!`/`JoinSet`;
   assert both resolve with their correct replies (multiplex correctness, ordering).
4. Test timeout: no reply within `timeout` → `HexError::Timeout`.

## Done criteria
- `cargo test -p vag-hex` green (+ existing 24); `cargo clippy -p vag-hex --all-targets -- -D warnings`
  clean (default and `--no-default-features`); `cargo build --workspace` clean.
- Commit in THIS worktree; Conventional Commit ending with the mandatory
  `Assisted-By:` + `Claude-Session:` trailers (see `/CLAUDE.md`).

## Interfaces (Produces)
- `vag_hex::{CableActor, CableHandle}`, `spawn(...)` — consumed by init-handshake + the
  future encrypted-UDS `AsyncIsoTpTransport` impl.

## Note for the controller
`transport.rs`'s `HexCable`/sync-`IsoTpTransport` is a transitional stub from wave 1 — this
task supersedes it with the async actor. Leave `HexCable` or remove it, your call; if removed,
update `lib.rs` re-exports and note it.
