# uds-async / 01 — async UDS client + ISO-TP over AsyncIsoTpTransport

**Subsystem:** uds-async · **Crate:** `vag-protocol` · **Wave:** 2 · **Depends:** async-core (done)

## Goal
An async UDS client + ISO-TP layer that rides `vag_transport::AsyncIsoTpTransport`, so the
`vagcan info` command can issue concurrent UDS reads. Mirror the existing sync client's
semantics (esp. the read-only allowlist) — do not regress them.

## Context
- `vag-protocol` today has a SYNC UDS client + ISO-TP over sync `IsoTpTransport`. Explore it
  first (services, the read-only allowlist `{0x10, 0x19, 0x22, 0x3E}` → `Forbidden` otherwise,
  ISO-TP multi-frame recv/flow-control).
- `vag_transport::AsyncIsoTpTransport` (async `send`/`recv`) + `MockAsyncTransport` (behind the
  `test-util` feature) exist for testing with no hardware.

## Deliverables
- An async UDS client generic over `T: AsyncIsoTpTransport` (static dispatch). Provide the reads
  the info command needs: `read_data_by_identifier(did: u16)` (0x22), `read_dtc_information`
  (0x19), `diagnostic_session_control` (0x10), `tester_present` (0x3E). **Preserve the read-only
  allowlist** — reject write services exactly as the sync client does.
- If ISO-TP segmentation currently lives in the transport layer vs the client, match the existing
  split; if the async transport delivers whole PDUs (it does — `AsyncIsoTpTransport` sends/recvs
  whole PDUs), the client works at the PDU level and ISO-TP framing stays below the trait. Confirm
  which and keep it consistent with the sync design. DRY: share request/response encoding with the
  sync client where practical (extract shared helpers rather than copy-paste).
- Keep the sync client intact (additive) unless a clean shared-core refactor is obviously better —
  if you refactor shared code, keep the sync client's tests green.

## TDD (no hardware — use MockAsyncTransport)
1. `read_data_by_identifier(0xF190)` → mock scripted with `(22 F1 90, 62 F1 90 <VIN ASCII>)`;
   assert the client returns the VIN bytes.
2. `read_dtc_information` (0x19 02) round-trip against a scripted mock.
3. A write/forbidden service is rejected without touching the transport (`Forbidden`).
4. tester_present (0x3E) round-trip.

## Done criteria
- `cargo test -p vag-protocol` green (async + existing sync tests); `cargo clippy -p vag-protocol
  --all-targets -- -D warnings` clean; `cargo build --workspace` clean. Add vag-protocol's
  `test-util`/dev-dep wiring so it can use `MockAsyncTransport` + `#[tokio::test]`.
- Commit in THIS worktree; mandatory `Assisted-By:` + `Claude-Session:` trailers.

## Interfaces (Produces)
- The async UDS client (consumed by `vin-info` / `cli-app`): the DID/DTC/session/tester-present
  read methods over any `AsyncIsoTpTransport`.
