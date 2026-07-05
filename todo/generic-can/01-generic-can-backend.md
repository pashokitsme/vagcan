# generic-can / 01 — generic CAN transport (bypass the cable's encrypted link)

**Subsystem:** generic-can · **Crate:** new `vag-can` · **Wave:** 2 (background, "just in case")
· **Depends:** async-core (done)

## Why (context)
The HEX cable's diagnostic channel is AES-encrypted app↔cable and needs a runtime session key
(see `research/vag-hex-framing.md` "Link cipher" + `research/SCOPE-BOUNDARY.md`). A generic CAN
interface (a plain USB-CAN adapter) sidesteps that entirely: talk UDS-over-ISO-TP-over-CAN
straight to the car. This is a **fallback path** — build a clean, modest foundation, not a
gold-plated driver. It plugs into the SAME `vag_transport::AsyncIsoTpTransport` seam the rest of
the stack consumes, so `vagcan info` works over it unchanged.

## Deliverables (new crate `crates/vag-can`)
- Add to the workspace members. Deps: `vag-transport` (path), `tokio` (workspace), `thiserror`.
- `trait CanBackend { async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), CanError>;
  async fn recv_frame(&mut self, timeout: Duration) -> Result<(u32, Vec<u8>), CanError>; }`
  (static dispatch; 11/29-bit ids).
- One concrete backend behind a feature: **`slcan`** (serial-line CAN / LAWICEL over a serial
  USB-CAN adapter) is the most portable on macOS — parse/emit slcan ASCII frames over a serial
  port. (If you prefer, a `socketcan` feature gated to `#[cfg(target_os = "linux")]` as a second
  impl — optional; macOS M4 is the host, so slcan is the priority.) Keep the actual serial dep
  optional/feature-gated so the crate builds without hardware.
- `IsoTpCan<B: CanBackend>` implementing `vag_transport::AsyncIsoTpTransport`: ISO-TP
  (ISO 15765-2) single-frame + multi-frame segmentation/reassembly with flow control, over
  `CanBackend`, using the standard diagnostic CAN ids (configurable; default UDS physical
  addressing e.g. tester `0x7E0+n` / ECU `0x7E8+n`). If `vag-protocol` already has reusable
  ISO-TP logic, prefer sharing it over re-implementing.

## TDD (no hardware)
- Mock `CanBackend` (in-memory frame queue). Test: an ISO-TP single-frame UDS request →
  the right CAN frame(s) emitted; a multi-frame response reassembles to the whole PDU with
  correct flow-control handshake.
- slcan codec unit tests (encode/decode a couple of real slcan lines) if you implement it.

## Done criteria
- `cargo test -p vag-can` green; `cargo clippy -p vag-can --all-targets -- -D warnings` clean;
  `cargo build --workspace` clean (crate builds without any serial hardware present).
- Commit in THIS worktree; mandatory `Assisted-By:` + `Claude-Session:` trailers.

## Scope discipline
New crate `vag-can` + add it to root `[workspace] members`. This is the ONE task this wave that
edits the root `Cargo.toml` (members list) — keep that edit minimal (just the members line) to
avoid clashing with other worktrees. Do not touch other crates.

## Interfaces (Produces)
- `vag_can::{CanBackend, IsoTpCan}` — an alternate `AsyncIsoTpTransport` the CLI can select.
