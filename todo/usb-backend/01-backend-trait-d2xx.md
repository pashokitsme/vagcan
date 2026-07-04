# usb-backend / 01 — Backend trait + D2XX backend

**Subsystem:** usb-backend · **Crate:** `vag-hex` · **Wave:** 1 · **Depends:** none

## Goal
A pluggable byte-pipe backend for the cable, with the D2XX implementation. The
blocking FTDI D2XX handle must live on a DEDICATED OS thread, bridged to async via
channels — never `spawn_blocking` per call, never blocking inside the tokio reactor.

## Context
- Cable is FTDI, accessed via D2XX bulk: OUT endpoint `0x02`, IN endpoint `0x81`
  (see `research/vag-hex-framing.md`). VID `0x0403`, the HEX serial e.g. `RT000001`.
- Vendored driver in `driver/darwin-arm64/` (libftd2xx dylib + headers). Host = macOS
  Apple Silicon M4.
- FTDI IN transfers prefix each 64-byte packet with a 2-byte modem/line status that
  must be STRIPPED (see `research/clb-crack/usbpcap.py::strip_ftdi_in`) before the
  byte stream reaches the framer.
- The current `usb.rs` is a stub (`BytePipe` trait + `open()` returning Unspecified).
  Replace it with the real backend.

## Deliverables
- Add deps to `crates/vag-hex/Cargo.toml`: `tokio` (features: `rt`, `sync`, `macros`,
  `time`), an FTDI D2XX binding (`libftd2xx` crate — links the vendored/ system D2XX),
  `thiserror` (already). Gate D2XX behind a `d2xx` feature (default on) so the crate
  still builds without the native lib in CI if needed.
- `pub trait Backend: Send { async fn write(&mut self, bytes: &[u8]) -> Result<(),
  HexError>; async fn read(&mut self, buf: &mut [u8]) -> Result<usize, HexError>; }`
  — native async fn, static dispatch (no async_trait).
- `pub struct D2xxBackend` implementing `Backend`. Internally: a dedicated
  `std::thread` owns the blocking `Ftdi` handle and runs read/write loops; the async
  methods talk to it over `tokio::sync::mpsc` (+ `oneshot` for write acks / read
  results). FTDI IN status bytes stripped here so `read()` yields clean payload.
- `pub fn list_cables() -> Result<Vec<CableInfo>, HexError>` (serial/description/VID/PID)
  and `D2xxBackend::open(serial: Option<&str>) -> Result<Self, HexError>` (baud, latency
  timer, purge — set the FTDI params VCDS uses; document the values chosen).
- Map D2XX/FTDI errors → `HexError` (extend the enum if needed).

## TDD / verification
- Unit-test the status-stripping + the thread↔async bridge with a FAKE backend
  handle (a loopback in-memory "device") — do NOT require hardware in `cargo test`.
- Provide an example or `#[ignore]` integration test `open_real_cable` that
  enumerates + opens the physical cable (run manually on the M4). This is the
  **hardware checkpoint** input.

## Done criteria
- `cargo build -p vag-hex` (with `d2xx`) links against the vendored darwin-arm64 D2XX.
- `cargo test -p vag-hex` green (hardware test `#[ignore]`d); clippy `-D warnings` clean.
- No blocking call on the async path except inside the dedicated thread.

## Interfaces (Produces)
- `vag_hex::usb::{Backend, D2xxBackend, CableInfo, list_cables}` (consumed by cable-actor).

## Hardware checkpoint
After merge: run `open_real_cable` on the M4 with the cable plugged — must enumerate
and open. Report to user before wiring the actor.
