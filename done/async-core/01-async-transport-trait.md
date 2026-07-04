# async-core / 01 — async transport trait + mock

**Subsystem:** async-core · **Crate:** `vag-transport` · **Wave:** 1 · **Depends:** none

## Goal
Turn the transport seam async so a tokio `CableActor` and an async UDS client can
ride it. Define the async trait(s) + error model + an in-memory async mock for
tests. This is the seam every later layer consumes.

## Context
Today `vag-transport` has a **sync** `IsoTpTransport` (`send`/`recv` with
`Duration`). We keep the sync trait for existing sync consumers if any, and ADD
an async trait. Check current callers (`vag-protocol`, `vag-capture`) — do not
break their build; if they only use the sync trait, leave them.

## Deliverables
- `AsyncIsoTpTransport` trait using native `async fn` in trait:
  ```rust
  pub trait AsyncIsoTpTransport: Send {
      async fn send(&mut self, pdu: &[u8]) -> Result<(), TransportError>;
      async fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError>;
  }
  ```
  (Confirm object-safety needs: we use STATIC dispatch — do not add `async_trait`.)
- Reuse/extend `TransportError` (already exists) for async paths; add variants only
  if a real gap exists (e.g. `Closed`).
- `MockAsyncTransport` (test util, behind `#[cfg(test)]` or a `test-util` feature):
  scripted request→response pairs, so upper layers can be tested with no hardware.
- Keep it `no_std`-agnostic? No — tokio implies std. Add `tokio` only if needed for
  the trait (it isn't; the trait is runtime-agnostic). Prefer NO tokio dep here.

## TDD
1. Write a failing test: a `MockAsyncTransport` scripted with one pair; an async
   test (`#[tokio::test]`) sends a PDU and asserts the recv matches. (tokio as a
   dev-dependency only, for the test runtime.)
2. Implement trait + mock to pass.
3. Test timeout path: recv with an empty script + short timeout → `TransportError::Timeout`.

## Done criteria
- `cargo test -p vag-transport` green; `cargo clippy -p vag-transport --all-targets
  -- -D warnings` clean. Workspace still builds (`cargo build --workspace`).
- No `async_trait` crate. tokio only as dev-dependency.
- Short doc-comment on the trait explaining the actor/handle model it feeds.

## Interfaces (Produces)
- `vag_transport::AsyncIsoTpTransport` (consumed by cable-actor, uds-async).
- `vag_transport::MockAsyncTransport` (consumed by uds-async tests).
