# Cross-Platform Runtime M1 — Portable Core + `vag-runtime-tokio` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Decouple the portable protocol core (`vag-transport`, `vag-can`, `vag-protocol`) from tokio — make it `no_std`+alloc and executor-agnostic via two seams (`embedded-io-async` bytes + a new `Timer` trait) — and ship the desktop `vag-runtime-tokio` adapter, keeping the workspace green at every step.

**Architecture:** The core names abstractions; adapters name runtimes. `SlcanBackend<S, T>` becomes generic over `embedded_io_async::{Read, Write}` for bytes and over a `Timer` field for time; `IsoTpCan<B, T>` carries the same `Timer` field. The real tokio-serial constructor moves out of the core into `vag-runtime-tokio`, which supplies a `TokioTimer` and a serial→`embedded-io-async` bridge. This is Milestone 1 of `docs/superpowers/specs/2026-07-06-cross-platform-runtime-design.md`; the ESP32 adapter (M2) is out of scope.

**Tech Stack:** Rust edition 2024, tokio (desktop test executor + adapter only), `embedded-io-async` 0.6, `embedded-io-adapters` 0.6 (`tokio-1`), `thiserror` 2 (no_std-capable), `tokio-serial` 5.4 (adapter only).

## Global Constraints

- Rust edition 2024; workspace `rust-version` floor unchanged (`1.85`).
- The three core crates (`vag-transport`, `vag-can`, `vag-protocol`) must become `#![cfg_attr(not(test), no_std)]` + `extern crate alloc;`, with ZERO tokio in their normal `[dependencies]` (tokio may remain a `[dev-dependencies]` test executor). Non-test code uses `core::time::Duration`, `alloc::{vec::Vec, string::String, format}` — no `std::*`.
- The transport trait SIGNATURES are frozen: `vag_transport::{AsyncIsoTpTransport, IsoTpTransport, RawCanTransport}` and `vag_can::backend::CanBackend` do NOT change shape. The `Timer` is internal struct state, never a new trait-method parameter.
- Pinned new deps in `[workspace.dependencies]`: `embedded-io-async = "0.6"`, `embedded-io-adapters = { version = "0.6", features = ["tokio-1"] }`.
- The workspace `thiserror` dependency is bumped `"1"` → `"2"` (thiserror 1 is std-only and cannot compile in a `no_std` lib; thiserror 2 emits `core::error::Error`, stable since Rust 1.81, below the 1.85 floor). All existing `#[error("…")]` / `#[from]` usages are compatible.
- Every task ends GREEN:
  - `cargo build --workspace`
  - `cargo test --workspace`
  - `cargo clippy --workspace --all-targets -- -D warnings`

---

### Task 1: `vag-transport` — add the `Timer` seam + go `no_std`

Adds the executor-agnostic `Timer` trait + `Elapsed` marker, a two-mode `MockTimer` test double, converts the crate to `#![cfg_attr(not(test), no_std)]`+alloc, and bumps `thiserror` to 2 workspace-wide.

**Files:**
- Modify: `Cargo.toml` (root) — `[workspace.dependencies]`: bump `thiserror`, add `embedded-io-async`, `embedded-io-adapters`.
- Modify: `crates/vag-transport/src/traits.rs:1` (imports), append `Timer`/`Elapsed`/`MockTimer`.
- Modify: `crates/vag-transport/src/lib.rs:1-11` (attrs + re-exports).
- Modify: `crates/vag-transport/src/error.rs:1` (alloc `String`).
- Modify: `crates/vag-transport/src/frame.rs:1` (alloc `Vec`).
- Modify: `crates/vag-transport/src/mock.rs:1-6,17-20,58-70` (alloc `Vec`/`VecDeque`, core `Duration`).

**Interfaces:**
- Produces:
  - `pub trait Timer { async fn timeout<F: Future>(&self, dur: Duration, fut: F) -> Result<F::Output, Elapsed>; async fn sleep(&self, dur: Duration); }` (in `vag_transport::traits`, re-exported as `vag_transport::Timer`).
  - `pub struct Elapsed;` — `#[derive(Debug, Clone, Copy, PartialEq, Eq)]`, re-exported as `vag_transport::Elapsed`.
  - `pub struct MockTimer` with `MockTimer::passthrough() -> Self` and `MockTimer::immediate() -> Self`, `#[derive(Debug, Clone, Copy)]`, gated `#[cfg(any(test, feature = "test-util"))]`, re-exported as `vag_transport::MockTimer`.

- [ ] **Step 1: Bump/add workspace dependencies**

Edit `Cargo.toml` (root) `[workspace.dependencies]` — change the `thiserror` line and add the two `embedded-io-*` lines:

```toml
[workspace.dependencies]
thiserror = "2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
# Async runtime — features selected per-crate (e.g. tokio = { workspace = true, features = [...] }).
tokio = { version = "1", default-features = false }
# Benchmarking (label-lookup perf, etc).
criterion = "0.5"
# Portable async byte-I/O seam for the no_std core (SlcanBackend<S>).
embedded-io-async = "0.6"
# Desktop bridge: wrap tokio AsyncRead/AsyncWrite as embedded-io-async.
embedded-io-adapters = { version = "0.6", features = ["tokio-1"] }
```

- [ ] **Step 2: Confirm the thiserror bump keeps the workspace building**

Run: `cargo build --workspace`
Expected: PASS (compiles clean; `thiserror` 2 is drop-in for the existing `#[error("…")]`/`#[from]` derives in `vag-hex`, `vag-protocol`, `vag-transport`, `vag-can`).

- [ ] **Step 3: Write the failing `Timer`/`MockTimer` tests**

Append to `crates/vag-transport/src/traits.rs`:

```rust
#[cfg(test)]
mod timer_tests {
    use super::{Elapsed, MockTimer, Timer};
    use core::time::Duration;

    #[tokio::test]
    async fn passthrough_runs_inner_future_to_completion() {
        let t = MockTimer::passthrough();
        let out = t.timeout(Duration::from_secs(1), async { 42u8 }).await;
        assert_eq!(out, Ok(42));
    }

    #[tokio::test]
    async fn immediate_returns_elapsed_without_running_future() {
        let t = MockTimer::immediate();
        let out: Result<u8, Elapsed> = t.timeout(Duration::from_secs(1), async { 42u8 }).await;
        assert_eq!(out, Err(Elapsed));
    }

    #[tokio::test]
    async fn sleep_returns_immediately() {
        // A one-hour sleep resolves at once because MockTimer never touches a clock.
        MockTimer::passthrough().sleep(Duration::from_secs(3600)).await;
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p vag-transport timer_tests`
Expected: FAIL — `cannot find type MockTimer`, `cannot find trait Timer`, `cannot find type Elapsed`.

- [ ] **Step 5: Implement `Timer`, `Elapsed`, and `MockTimer`**

Replace the import header at the top of `crates/vag-transport/src/traits.rs` (currently `use std::time::Duration;`) with:

```rust
use alloc::vec::Vec;
use core::future::Future;
use core::time::Duration;
use crate::{CanFrame, TransportError};
```

Then append (after the existing `AsyncIsoTpTransport` trait, before the `#[cfg(test)] mod timer_tests`):

```rust
/// Returned by [`Timer::timeout`] when the deadline fires before the future completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed;

/// Executor-agnostic time source for the portable core.
///
/// One implementor per runtime adapter (desktop: `TokioTimer`; ESP32-S3:
/// an embassy-time timer in M2). Static dispatch only — callers take
/// `T: Timer`, no `dyn`, no `async_trait`. The core owns a `Timer` as a
/// field; it never appears in a transport-trait method signature.
#[allow(async_fn_in_trait)] // same seam rationale as AsyncIsoTpTransport
pub trait Timer {
    /// Race `fut` against a deadline; `Err(Elapsed)` if the deadline hits first.
    async fn timeout<F: Future>(&self, dur: Duration, fut: F) -> Result<F::Output, Elapsed>;

    /// Sleep for `dur` (ISO-TP STmin inter-frame gap).
    async fn sleep(&self, dur: Duration);
}

/// Deterministic [`Timer`] for tests: no real clock, two behaviours.
///
/// - `passthrough()` — `timeout` polls the inner future to completion and
///   returns `Ok`; `sleep` is a no-op. Use for success-path tests.
/// - `immediate()` — `timeout` drops the inner future and returns `Err(Elapsed)`;
///   `sleep` is a no-op. Use to exercise the timeout branch with no clock.
#[cfg(any(test, feature = "test-util"))]
#[derive(Debug, Clone, Copy)]
pub struct MockTimer {
    immediate_timeout: bool,
}

#[cfg(any(test, feature = "test-util"))]
impl MockTimer {
    /// `timeout` runs the inner future to completion.
    pub fn passthrough() -> Self {
        MockTimer { immediate_timeout: false }
    }

    /// `timeout` always returns `Err(Elapsed)` without running the future.
    pub fn immediate() -> Self {
        MockTimer { immediate_timeout: true }
    }
}

#[cfg(any(test, feature = "test-util"))]
impl Timer for MockTimer {
    async fn timeout<F: Future>(&self, _dur: Duration, fut: F) -> Result<F::Output, Elapsed> {
        if self.immediate_timeout {
            drop(fut);
            Err(Elapsed)
        } else {
            Ok(fut.await)
        }
    }

    async fn sleep(&self, _dur: Duration) {}
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p vag-transport timer_tests`
Expected: PASS — 3 passed.

- [ ] **Step 7: Convert the crate to `no_std`+alloc**

Replace the top of `crates/vag-transport/src/lib.rs` (lines 1-11) with:

```rust
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod error;
pub mod frame;
pub mod mock;
pub mod traits;

pub use error::TransportError;
pub use frame::{CanFrame, CanId};
#[cfg(any(test, feature = "test-util"))]
pub use mock::MockAsyncTransport;
pub use mock::{ScriptStep, ScriptedCan};
pub use traits::{AsyncIsoTpTransport, Elapsed, IsoTpTransport, RawCanTransport, Timer};
#[cfg(any(test, feature = "test-util"))]
pub use traits::MockTimer;
```

- [ ] **Step 8: Fix `error.rs` for alloc**

Insert at the very top of `crates/vag-transport/src/error.rs` (before line 1):

```rust
use alloc::string::String;
```

- [ ] **Step 9: Fix `frame.rs` for alloc**

Insert at the very top of `crates/vag-transport/src/frame.rs` (before line 1):

```rust
use alloc::vec::Vec;
```

- [ ] **Step 10: Fix `mock.rs` for alloc + core**

Replace the import header of `crates/vag-transport/src/mock.rs` (lines 1-6) with:

```rust
use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::time::Duration;
use crate::{CanFrame, TransportError};
use crate::traits::RawCanTransport;

#[cfg(test)]
use crate::CanId;
```

Then replace each `std::collections::VecDeque` token with `VecDeque` in this file:
- line 18: `steps: VecDeque<ScriptStep>,`
- line 60: `script: VecDeque<(Vec<u8>, Vec<u8>)>,`
- line 61: `pending: VecDeque<Vec<u8>>,`
- line 69: `MockAsyncTransport { script: script.into(), pending: VecDeque::new(), sent: Vec::new() }`

- [ ] **Step 11: Verify the crate builds no_std and all tests pass**

Run: `cargo build -p vag-transport`
Expected: PASS (this is the `not(test)` = `no_std` build).

Run: `cargo test -p vag-transport`
Expected: PASS — existing `frame`/`mock`/scripted tests plus the 3 new `timer_tests` all pass.

- [ ] **Step 12: Full workspace gate**

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 13: Commit**

```bash
git add Cargo.toml crates/vag-transport
git commit -m "feat(vag-transport): add Timer seam + MockTimer; go no_std+alloc

Add the executor-agnostic Timer trait (timeout + sleep) and Elapsed marker,
a two-mode MockTimer (passthrough / immediate) behind test-util, and make the
crate no_std+alloc. Bump workspace thiserror 1->2 (no_std-capable) and pin
embedded-io-async / embedded-io-adapters for downstream tasks.

Assisted-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: decc3d5d-5524-428e-b26f-67a30857bc30"
```

---

### Task 2: `vag-can::slcan` — `SlcanBackend` to `embedded-io-async` + `Timer` field

Re-generics `SlcanBackend<S>` → `SlcanBackend<S, T>`, swaps tokio byte I/O for `embedded-io-async`, and replaces the `tokio::time` deadline with the `Timer` field. The crate stays std here (isotp + the `slcan`-feature `open` still use tokio; the no_std flip is Task 4). The pure `encode_frame`/`decode_frame` codec is unchanged.

**Files:**
- Modify: `crates/vag-can/Cargo.toml` (add `embedded-io-async` dep; add dev-deps).
- Modify: `crates/vag-can/src/slcan.rs:1-166` (imports, struct, impls, tests).

**Interfaces:**
- Consumes: `vag_transport::{Timer, Elapsed}` (Task 1); `MockTimer` via `vag-transport` `test-util` dev-dep.
- Produces:
  - `pub struct SlcanBackend<S, T> { … }`.
  - `impl<S: embedded_io_async::Read + embedded_io_async::Write, T: Timer + Clone> SlcanBackend<S, T>` with `pub fn new(stream: S, timer: T) -> Self`, `pub async fn open_channel(&mut self, bitrate: SlcanBitrate) -> Result<(), CanError>`, `pub async fn close_channel(&mut self) -> Result<(), CanError>`.
  - `impl<S: embedded_io_async::Read + embedded_io_async::Write + Send, T: Timer + Clone + Send> CanBackend for SlcanBackend<S, T>` — `send_frame`/`recv_frame` signatures unchanged.

- [ ] **Step 1: Add the byte-I/O dep and test deps**

Replace `crates/vag-can/Cargo.toml` `[dependencies]`/`[dev-dependencies]` blocks with:

```toml
[dependencies]
vag-transport = { path = "../vag-transport" }
thiserror.workspace = true
embedded-io-async.workspace = true
tokio = { workspace = true, features = ["time", "io-util"] }
# Serial port access for the slcan backend constructor only; the codec and the
# stream-generic backend build (and are tested) without it.
tokio-serial = { version = "5.4", optional = true }

[dev-dependencies]
tokio = { workspace = true, features = ["rt", "macros", "time", "io-util"] }
embedded-io-adapters = { workspace = true }
vag-transport = { path = "../vag-transport", features = ["test-util"] }
```

(Leave the `[features] slcan = ["dep:tokio-serial"]` block untouched — it is removed in Task 4.)

- [ ] **Step 2: Migrate the slcan tests to `embedded-io-async` + `MockTimer` (write the failing tests)**

Replace the entire `#[cfg(test)] mod tests` block in `crates/vag-can/src/slcan.rs` (lines 168-270) with:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::CAN_EFF_FLAG;
    use embedded_io_adapters::tokio_1::FromTokio;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use vag_transport::MockTimer;

    #[test]
    fn encodes_standard_frame() {
        let line = encode_frame(0x7E0, &[0x02, 0x10, 0x03, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(line, "t7E080210030000000000\r");
    }

    #[test]
    fn encodes_extended_frame() {
        let line = encode_frame(0x18DA_10F1 | CAN_EFF_FLAG, &[0x3E, 0x00]).unwrap();
        assert_eq!(line, "T18DA10F123E00\r");
    }

    #[test]
    fn encode_rejects_oversized_data() {
        let err = encode_frame(0x7E0, &[0u8; 9]).unwrap_err();
        assert!(matches!(err, CanError::Unsupported(_)), "got {err:?}");
    }

    #[test]
    fn decodes_standard_frame() {
        let (id, data) = decode_frame("t7E88025003AAAAAAAAAA\r").unwrap();
        assert_eq!(id, 0x7E8);
        assert_eq!(data, vec![0x02, 0x50, 0x03, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA]);
    }

    #[test]
    fn decodes_extended_frame() {
        let (id, data) = decode_frame("T18DAF11027E00").unwrap();
        assert_eq!(id, 0x18DA_F110 | CAN_EFF_FLAG);
        assert_eq!(data, vec![0x7E, 0x00]);
    }

    #[test]
    fn decode_tolerates_trailing_timestamp() {
        let (id, data) = decode_frame("t7E82500312AB").unwrap();
        assert_eq!(id, 0x7E8);
        assert_eq!(data, vec![0x50, 0x03]);
    }

    #[test]
    fn decode_rejects_garbage() {
        for bad in ["", "x123", "t7E", "t7E09", "t7E01ZZ"] {
            assert!(decode_frame(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[tokio::test]
    async fn backend_writes_frame_as_ascii_line() {
        let (client, mut adapter) = tokio::io::duplex(256);
        let mut backend = SlcanBackend::new(FromTokio::new(client), MockTimer::passthrough());
        backend.send_frame(0x7E0, &[0x02, 0x10, 0x03]).await.unwrap();

        let mut got = vec![0u8; 64];
        let n = adapter.read(&mut got).await.unwrap();
        assert_eq!(&got[..n], b"t7E03021003\r");
    }

    #[tokio::test]
    async fn backend_parses_incoming_frame_and_skips_acks() {
        let (client, mut adapter) = tokio::io::duplex(256);
        let mut backend = SlcanBackend::new(FromTokio::new(client), MockTimer::passthrough());
        // tx-ack 'z', a bare CR, a BEL error byte, then the actual frame.
        adapter.write_all(b"z\r\r\x07t7E825003\r").await.unwrap();

        let (id, data) = backend.recv_frame(Duration::from_millis(200)).await.unwrap();
        assert_eq!(id, 0x7E8);
        assert_eq!(data, vec![0x50, 0x03]);
    }

    #[tokio::test]
    async fn backend_recv_times_out() {
        // Empty duplex never yields a line; MockTimer::immediate forces the
        // timeout branch deterministically (a passthrough timer would hang).
        let (client, _adapter) = tokio::io::duplex(256);
        let mut backend = SlcanBackend::new(FromTokio::new(client), MockTimer::immediate());
        let err = backend.recv_frame(Duration::from_millis(10)).await.unwrap_err();
        assert!(matches!(err, CanError::Timeout), "got {err:?}");
    }

    #[tokio::test]
    async fn backend_recv_reports_disconnect() {
        let (client, adapter) = tokio::io::duplex(256);
        drop(adapter);
        let mut backend = SlcanBackend::new(FromTokio::new(client), MockTimer::passthrough());
        let err = backend.recv_frame(Duration::from_millis(50)).await.unwrap_err();
        assert!(matches!(err, CanError::Disconnected), "got {err:?}");
    }

    #[tokio::test]
    async fn open_channel_sends_close_bitrate_open() {
        let (client, mut adapter) = tokio::io::duplex(256);
        let mut backend = SlcanBackend::new(FromTokio::new(client), MockTimer::passthrough());
        backend.open_channel(SlcanBitrate::Rate500k).await.unwrap();

        let mut got = vec![0u8; 64];
        let n = adapter.read(&mut got).await.unwrap();
        assert_eq!(&got[..n], b"C\rS6\rO\r");
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p vag-can slcan`
Expected: FAIL — `SlcanBackend::new` takes 1 argument but 2 supplied; `SlcanBackend<S>` has 1 type parameter but 2 supplied.

- [ ] **Step 4: Replace the imports + struct + inherent impl in `slcan.rs`**

Replace the import header (lines 6-11) of `crates/vag-can/src/slcan.rs` with:

```rust
use std::time::Duration;
use embedded_io_async::{Read as _, Write as _};
use vag_transport::{Elapsed, Timer};

use crate::CanError;
use crate::backend::{CAN_EFF_FLAG, CAN_EFF_MASK, CAN_SFF_MASK, CanBackend};
```

Replace the struct + inherent impl (lines 74-129) with:

```rust
/// slcan backend over any async byte stream (serial port, or a `tokio::io::duplex`
/// bridged through `embedded-io-adapters` in tests). Skips non-frame lines
/// (command acks, status) on receive. `T` supplies the timeout/delay clock.
pub struct SlcanBackend<S, T> {
    stream: S,
    timer: T,
    buf: Vec<u8>,
}

impl<S: embedded_io_async::Read + embedded_io_async::Write, T: Timer + Clone> SlcanBackend<S, T> {
    pub fn new(stream: S, timer: T) -> Self {
        SlcanBackend { stream, timer, buf: Vec::new() }
    }

    /// Send the channel-open sequence: close, set bitrate, open.
    /// Fire-and-forget: many adapters NAK a redundant `C`, so acks are not checked.
    pub async fn open_channel(&mut self, bitrate: SlcanBitrate) -> Result<(), CanError> {
        let cmd = format!("C\rS{}\rO\r", bitrate as u8);
        self.write_all(cmd.as_bytes()).await
    }

    /// Send the channel-close command.
    pub async fn close_channel(&mut self) -> Result<(), CanError> {
        self.write_all(b"C\r").await
    }

    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), CanError> {
        self.stream
            .write_all(bytes)
            .await
            .map_err(|e| CanError::Io(format!("{e:?}")))?;
        self.stream.flush().await.map_err(|e| CanError::Io(format!("{e:?}")))
    }

    /// Next CR-terminated line (without the CR), reading more bytes as needed,
    /// bounded by `budget`. A single `timer.timeout` races the whole read loop,
    /// so no monotonic clock is required.
    async fn read_line(&mut self, budget: Duration) -> Result<Vec<u8>, CanError> {
        let timer = self.timer.clone();
        let buf = &mut self.buf;
        let stream = &mut self.stream;
        let result = timer
            .timeout(budget, async {
                loop {
                    if let Some(pos) = buf.iter().position(|&b| b == b'\r') {
                        let mut line: Vec<u8> = buf.drain(..=pos).collect();
                        line.pop(); // drop the CR
                        return Ok::<Vec<u8>, CanError>(line);
                    }
                    let mut chunk = [0u8; 256];
                    let n = stream
                        .read(&mut chunk)
                        .await
                        .map_err(|e| CanError::Io(format!("{e:?}")))?;
                    if n == 0 {
                        return Err(CanError::Disconnected);
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
            })
            .await;
        match result {
            Ok(inner) => inner,
            Err(Elapsed) => Err(CanError::Timeout),
        }
    }
}
```

- [ ] **Step 5: Replace the `CanBackend` impl in `slcan.rs`**

Replace the `CanBackend` impl (lines 131-152) with:

```rust
impl<S, T> CanBackend for SlcanBackend<S, T>
where
    S: embedded_io_async::Read + embedded_io_async::Write + Send,
    T: Timer + Clone + Send,
{
    async fn send_frame(&mut self, id: u32, data: &[u8]) -> Result<(), CanError> {
        let line = encode_frame(id, data)?;
        self.write_all(line.as_bytes()).await
    }

    async fn recv_frame(&mut self, timeout: Duration) -> Result<(u32, Vec<u8>), CanError> {
        loop {
            let line = self.read_line(timeout).await?;
            // Strip stray BEL (error ack) bytes; they are not CR-terminated.
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_matches(|c: char| c == '\u{7}' || c.is_whitespace());
            match text.as_bytes().first() {
                Some(b't' | b'T') => return decode_frame(text),
                // Command acks ('z', 'Z', version/status replies) and empty
                // lines are not bus traffic — skip them.
                _ => continue,
            }
        }
    }
}
```

- [ ] **Step 6: Delete the old `SlcanBackend::open` — leave a stub note (moves to Task 6)**

Replace the `#[cfg(feature = "slcan")]` impl block (lines 154-166) with (the constructor is recreated in `vag-runtime-tokio` in Task 6):

```rust
// NOTE: the real serial-port constructor `SlcanBackend::open` moved to the
// `vag-runtime-tokio` adapter (M1, Task 6). The core no longer knows tokio-serial.
```

- [ ] **Step 7: Run the slcan tests to verify they pass**

Run: `cargo test -p vag-can slcan`
Expected: PASS — the 7 codec unit tests + 5 async backend tests pass. The `backend_recv_times_out` test resolves through `MockTimer::immediate` → `CanError::Timeout`; `backend_recv_reports_disconnect` runs to `Ok(0)` → `CanError::Disconnected`.

- [ ] **Step 8: Build the whole crate (isotp still tokio here) + gate**

Run: `cargo build -p vag-can --features slcan`
Expected: PASS (the `slcan` feature still pulls `tokio-serial` but nothing uses it yet; the constructor is gone — that is fine, the feature is removed in Task 4).

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/vag-can/Cargo.toml crates/vag-can/src/slcan.rs
git commit -m "feat(vag-can): SlcanBackend over embedded-io-async + Timer field

SlcanBackend<S> -> SlcanBackend<S, T>: bytes via embedded_io_async::{Read,Write},
timeouts via the injected Timer (one timer.timeout races the whole read loop, no
monotonic clock). Error mapping via {e:?} (embedded_io::Error is Debug, not
Display). Tests bridge tokio::io::duplex through FromTokio and drive MockTimer.
The tokio-serial open constructor is removed here (recreated in vag-runtime-tokio).

Assisted-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: decc3d5d-5524-428e-b26f-67a30857bc30"
```

---

### Task 3: `vag-can::isotp` — `IsoTpCan` to `Timer` field + absolute-deadline refactor

Re-generics `IsoTpCan<B>` → `IsoTpCan<B, T>`, adds the `timer` field, threads it through `new`/`for_ecu`, and replaces every `tokio::time::Instant` deadline computation with a single `timer.timeout(budget, …)` per bounded loop. STmin `tokio::time::sleep(gap)` → `self.timer.sleep(gap)`. Behaviour-preserving; the existing timeout/skip tests must still pass.

**Files:**
- Modify: `crates/vag-can/src/isotp.rs:1-92` (imports, struct, constructors, receive helpers), `:104-155` (send STmin sleep, recv wrapper), `:218-407` (tests).

**Interfaces:**
- Consumes: `vag_transport::{Timer, Elapsed}`; `crate::backend::CanBackend`.
- Produces:
  - `pub struct IsoTpCan<B: CanBackend, T: Timer> { … }`.
  - `impl<B: CanBackend, T: Timer + Clone> IsoTpCan<B, T>` with `pub fn new(backend: B, tx: CanId, rx: CanId, timer: T) -> Self` and `pub fn for_ecu(backend: B, n: u8, timer: T) -> Self` and `pub fn into_backend(self) -> B`.
  - `impl<B: CanBackend, T: Timer + Clone + Send> AsyncIsoTpTransport for IsoTpCan<B, T>` — `send`/`recv` signatures unchanged.

- [ ] **Step 1: Update the tests to construct with a `MockTimer` (write the failing tests)**

In `crates/vag-can/src/isotp.rs`, replace the `use super::*;`/`use std::collections::VecDeque;` header of `mod tests` (lines 220-222) with:

```rust
    use super::*;
    use crate::CanError;
    use std::collections::VecDeque;
    use vag_transport::MockTimer;
```

Replace the `channel` helper (lines 249-251) with:

```rust
    fn channel(replies: Vec<(u32, Vec<u8>)>) -> IsoTpCan<MockCan, MockTimer> {
        IsoTpCan::for_ecu(MockCan::new(replies), 0, MockTimer::passthrough())
    }
```

Replace the body of `recv_times_out_when_bus_is_silent` (lines 375-380) with:

```rust
    #[tokio::test]
    async fn recv_times_out_when_bus_is_silent() {
        // No replies: MockTimer::immediate forces the timeout branch deterministically.
        let mut iso = IsoTpCan::for_ecu(MockCan::new(vec![]), 0, MockTimer::immediate());
        let err = iso.recv(Duration::from_millis(5)).await.unwrap_err();
        assert!(matches!(err, TransportError::Timeout), "got {err:?}");
    }
```

Replace `for_ecu_offsets_default_uds_ids` (lines 390-395) and `new_accepts_extended_ids` (lines 397-406) with:

```rust
    #[test]
    fn for_ecu_offsets_default_uds_ids() {
        let iso = IsoTpCan::for_ecu(MockCan::new(vec![]), 3, MockTimer::passthrough());
        assert_eq!(iso.tx, 0x7E3);
        assert_eq!(iso.rx, 0x7EB);
    }

    #[test]
    fn new_accepts_extended_ids() {
        let iso = IsoTpCan::new(
            MockCan::new(vec![]),
            CanId::Extended(0x18DA_10F1),
            CanId::Extended(0x18DA_F110),
            MockTimer::passthrough(),
        );
        assert_eq!(iso.tx, to_raw_id(CanId::Extended(0x18DA_10F1)));
        assert_eq!(iso.rx, to_raw_id(CanId::Extended(0x18DA_F110)));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p vag-can isotp`
Expected: FAIL — `IsoTpCan` has 1 type parameter but 2 supplied; `for_ecu`/`new` take 2/3 arguments but 3/4 supplied.

- [ ] **Step 3: Replace imports + struct + constructors + receive helpers**

Replace the import header (lines 1-3) of `crates/vag-can/src/isotp.rs` with:

```rust
use std::time::Duration;
use vag_transport::{AsyncIsoTpTransport, CanId, Elapsed, Timer, TransportError};

use crate::backend::{CanBackend, to_raw_id};
```

Replace the struct + inherent impl through `wait_flow_control` (lines 20-92) with:

```rust
/// One ISO-TP (ISO 15765-2) channel to a single ECU over a raw CAN backend.
///
/// Implements [`AsyncIsoTpTransport`], so the async UDS client rides it
/// unchanged. Classic CAN only (<= 4095-byte PDUs), frames padded to 8 bytes.
/// Frames from other CAN ids are skipped, not treated as errors — this is a
/// shared bus. `T` supplies the timeout/delay clock (no monotonic clock needed).
pub struct IsoTpCan<B: CanBackend, T: Timer> {
    backend: B,
    tx: u32,
    rx: u32,
    timer: T,
}

impl<B: CanBackend, T: Timer + Clone> IsoTpCan<B, T> {
    /// Channel with explicit tester (`tx`) and ECU (`rx`) ids.
    pub fn new(backend: B, tx: CanId, rx: CanId, timer: T) -> Self {
        IsoTpCan { backend, tx: to_raw_id(tx), rx: to_raw_id(rx), timer }
    }

    /// UDS physical addressing for ECU index `n`: tester `0x7E0+n`, ECU `0x7E8+n`.
    pub fn for_ecu(backend: B, n: u8, timer: T) -> Self {
        IsoTpCan {
            backend,
            tx: 0x7E0 + u32::from(n),
            rx: 0x7E8 + u32::from(n),
            timer,
        }
    }

    /// Consume the channel, returning the backend.
    pub fn into_backend(self) -> B {
        self.backend
    }

    fn pad8(mut frame: Vec<u8>) -> Vec<u8> {
        frame.resize(8, PAD);
        frame
    }

    /// Skip-loop with no timeout of its own; the caller bounds it via
    /// [`Timer::timeout`]. Returns the next frame carrying our `rx` id.
    async fn next_own_frame(&mut self, per_recv: Duration) -> Result<Vec<u8>, TransportError> {
        loop {
            let (id, data) = self.backend.recv_frame(per_recv).await?;
            if id == self.rx {
                return Ok(data);
            }
        }
    }

    /// Next frame from our ECU (`rx` id), skipping unrelated bus traffic,
    /// bounded by a single relative `budget`.
    async fn recv_own(&mut self, budget: Duration) -> Result<Vec<u8>, TransportError> {
        let timer = self.timer.clone();
        match timer.timeout(budget, self.next_own_frame(budget)).await {
            Ok(inner) => inner,
            Err(Elapsed) => Err(TransportError::Timeout),
        }
    }

    /// Wait for a flow-control frame; returns `(block_size, stmin)` on CTS.
    async fn wait_flow_control(&mut self) -> Result<(u8, u8), TransportError> {
        for _ in 0..=MAX_FC_WAIT {
            let data = self.recv_own(FC_TIMEOUT).await?;
            let pci = *data
                .first()
                .ok_or_else(|| TransportError::Protocol("empty flow control frame".into()))?;
            if pci >> 4 != 0x3 {
                return Err(TransportError::Protocol("expected flow control frame".into()));
            }
            match pci & 0x0F {
                0x0 => {
                    let bs = data.get(1).copied().unwrap_or(0);
                    let stmin = data.get(2).copied().unwrap_or(0);
                    return Ok((bs, stmin));
                }
                0x1 => continue, // FC.WAIT: sender must keep waiting
                0x2 => return Err(TransportError::Protocol("flow control: buffer overflow".into())),
                fs => {
                    return Err(TransportError::Protocol(format!("invalid flow status {fs:#x}")));
                }
            }
        }
        Err(TransportError::Protocol("too many FC.WAIT frames".into()))
    }
}
```

- [ ] **Step 4: Replace the STmin sleep in `send`**

In `crates/vag-can/src/isotp.rs`, inside the `AsyncIsoTpTransport::send` consecutive-frame loop, replace the gap sleep (lines 133-136):

```rust
            let gap = stmin_gap(stmin);
            if !gap.is_zero() {
                tokio::time::sleep(gap).await;
            }
```

with:

```rust
            let gap = stmin_gap(stmin);
            if !gap.is_zero() {
                self.timer.sleep(gap).await;
            }
```

- [ ] **Step 5: Refactor `recv` to a single `timer.timeout` over the whole reassembly**

Replace the entire `async fn recv` body (lines 153-215) with a thin wrapper that races the reassembly against one relative deadline, delegating to a private `recv_reassemble` that uses `next_own_frame`:

```rust
    async fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let timer = self.timer.clone();
        match timer.timeout(timeout, self.recv_reassemble(timeout)).await {
            Ok(inner) => inner,
            Err(Elapsed) => Err(TransportError::Timeout),
        }
    }
}

impl<B: CanBackend, T: Timer + Clone> IsoTpCan<B, T> {
    /// SF/FF+CF reassembly with no timeout of its own; `recv` bounds the whole
    /// thing with one `timer.timeout`, matching the old single-absolute-deadline
    /// behaviour without a monotonic clock. `per_recv` is the per-frame budget
    /// (the outer timeout is the true bound).
    async fn recv_reassemble(&mut self, per_recv: Duration) -> Result<Vec<u8>, TransportError> {
        let frame = self.next_own_frame(per_recv).await?;
        let pci = *frame
            .first()
            .ok_or_else(|| TransportError::Protocol("empty frame".into()))?;
        match pci >> 4 {
            // Single Frame.
            0x0 => {
                let len = (pci & 0x0F) as usize;
                let body = frame.get(1..1 + len).ok_or_else(|| {
                    TransportError::Protocol("single frame length exceeds data".into())
                })?;
                Ok(body.to_vec())
            }
            // First Frame: 12-bit length, 6 data bytes here.
            0x1 => {
                let len_low = *frame
                    .get(1)
                    .ok_or_else(|| TransportError::Protocol("malformed first frame".into()))?;
                let len = (((pci & 0x0F) as usize) << 8) | usize::from(len_low);
                if len <= 7 {
                    return Err(TransportError::Protocol("first frame with length <= 7".into()));
                }
                let mut out: Vec<u8> = frame
                    .get(2..8)
                    .ok_or_else(|| TransportError::Protocol("malformed first frame".into()))?
                    .to_vec();

                // Flow Control: ContinueToSend, block size 0 (send all), STmin 0.
                let fc = Self::pad8(vec![0x30, 0x00, 0x00]);
                self.backend.send_frame(self.tx, &fc).await?;

                let mut expected_seq: u8 = 1;
                while out.len() < len {
                    let cf = self.next_own_frame(per_recv).await?;
                    let cf_pci = *cf
                        .first()
                        .ok_or_else(|| TransportError::Protocol("empty consecutive frame".into()))?;
                    if cf_pci >> 4 != 0x2 {
                        return Err(TransportError::Protocol("expected consecutive frame".into()));
                    }
                    if cf_pci & 0x0F != expected_seq {
                        return Err(TransportError::Protocol(format!(
                            "CF sequence mismatch: got {}, want {}",
                            cf_pci & 0x0F,
                            expected_seq
                        )));
                    }
                    let take = (len - out.len()).min(7);
                    let payload = cf.get(1..1 + take).ok_or_else(|| {
                        TransportError::Protocol("malformed consecutive frame".into())
                    })?;
                    out.extend_from_slice(payload);
                    expected_seq = (expected_seq + 1) & 0x0F;
                }
                Ok(out)
            }
            _ => Err(TransportError::Protocol(
                "unexpected PCI in first frame position".into(),
            )),
        }
    }
}
```

Note: this closes the `AsyncIsoTpTransport for IsoTpCan` impl (with `recv`) and opens a second inherent `impl` block for `recv_reassemble`. Ensure the `send` method (unchanged except Step 4) still lives inside the `AsyncIsoTpTransport` impl above `recv`, and that the original trailing `}` of the old `recv`/impl is not duplicated.

- [ ] **Step 6: Update the `AsyncIsoTpTransport` impl header bound**

Change the impl header (line 104) from:

```rust
impl<B: CanBackend> AsyncIsoTpTransport for IsoTpCan<B> {
```

to:

```rust
impl<B: CanBackend, T: Timer + Clone + Send> AsyncIsoTpTransport for IsoTpCan<B, T> {
```

- [ ] **Step 7: Run the isotp tests to verify they pass**

Run: `cargo test -p vag-can isotp`
Expected: PASS — all 13 isotp tests pass, including `multi_frame_send_respects_wait_flow_status` (FC.WAIT loop), `multi_frame_send_honors_block_size`, and `recv_times_out_when_bus_is_silent` (via `MockTimer::immediate`).

- [ ] **Step 8: Full workspace gate**

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 9: Commit**

```bash
git add crates/vag-can/src/isotp.rs
git commit -m "refactor(vag-can): IsoTpCan Timer field + relative-deadline receive

IsoTpCan<B> -> IsoTpCan<B, T>. Replace tokio::time::Instant absolute-deadline
math in recv_own/wait_flow_control/recv with a single timer.timeout per bounded
loop (no monotonic clock needed) and STmin sleep with timer.sleep. Behaviour
preserved; existing timeout/skip/flow-control tests pass with MockTimer.

Assisted-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: decc3d5d-5524-428e-b26f-67a30857bc30"
```

---

### Task 4: `vag-can` — drop tokio normal deps, go `no_std`

Deletes the `slcan` feature + `tokio`/`tokio-serial` normal deps (the `open` constructor is already gone), adds `#![cfg_attr(not(test), no_std)]`+alloc, and sweeps every `std::*` import in the crate to `core`/`alloc`. tokio + `embedded-io-adapters` stay as dev-deps (the test executor + duplex bridge).

**Files:**
- Modify: `crates/vag-can/Cargo.toml` (remove tokio/tokio-serial normal deps + `slcan` feature).
- Modify: `crates/vag-can/src/lib.rs:1` (attrs).
- Modify: `crates/vag-can/src/backend.rs:1` (core `Duration`, alloc `Vec`).
- Modify: `crates/vag-can/src/error.rs:1` (alloc `String`).
- Modify: `crates/vag-can/src/slcan.rs:6` (core `Duration`, alloc `Vec`/`String`/`format`, core `ops::Range`).
- Modify: `crates/vag-can/src/isotp.rs:1` (core `Duration`, alloc `Vec`/`format`).

**Interfaces:**
- Consumes: nothing new.
- Produces: `vag-can` is a `no_std`+alloc lib; public surface (`SlcanBackend`, `IsoTpCan`, `CanBackend`, `CanError`, `SlcanBitrate`, `CAN_*`, `to_raw_id`/`from_raw_id`) unchanged.

- [ ] **Step 1: Strip tokio from `Cargo.toml`**

Replace the whole of `crates/vag-can/Cargo.toml` (below `[package]`) with:

```toml
[dependencies]
vag-transport = { path = "../vag-transport" }
thiserror.workspace = true
embedded-io-async.workspace = true

[dev-dependencies]
tokio = { workspace = true, features = ["rt", "macros", "time", "io-util"] }
embedded-io-adapters = { workspace = true }
vag-transport = { path = "../vag-transport", features = ["test-util"] }
```

(The `[features] slcan = …` block is removed entirely.)

- [ ] **Step 2: Add the no_std attrs to `lib.rs`**

Insert at the very top of `crates/vag-can/src/lib.rs` (before the `//!` module docs is not possible; put the inner attrs first, then keep the docs). Replace lines 1-11 with:

```rust
#![cfg_attr(not(test), no_std)]

//! Generic CAN transport — the fallback path that bypasses the HEX cable's
//! encrypted link entirely: UDS-over-ISO-TP-over-CAN through a plain USB-CAN
//! adapter (slcan/LAWICEL first, since the host is macOS).
//!
//! Plugs into the same [`vag_transport::AsyncIsoTpTransport`] seam the rest of
//! the stack consumes, so `vagcan info` works over it unchanged.

extern crate alloc;

pub mod backend;
pub mod error;
pub mod isotp;
pub mod slcan;
```

- [ ] **Step 3: Sweep `backend.rs` imports**

Replace line 1 of `crates/vag-can/src/backend.rs` (`use std::time::Duration;`) with:

```rust
use alloc::vec::Vec;
use core::time::Duration;
```

- [ ] **Step 4: Sweep `error.rs` imports**

Insert at the very top of `crates/vag-can/src/error.rs` (before line 1):

```rust
use alloc::string::String;
```

- [ ] **Step 5: Sweep `slcan.rs` imports**

Replace the import header of `crates/vag-can/src/slcan.rs` (currently `use std::time::Duration;` + the embedded-io/vag-transport lines added in Task 2) with:

```rust
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::time::Duration;
use embedded_io_async::{Read as _, Write as _};
use vag_transport::{Elapsed, Timer};

use crate::CanError;
use crate::backend::{CAN_EFF_FLAG, CAN_EFF_MASK, CAN_SFF_MASK, CanBackend};
```

Then change the `hex_field` signature (line 44) from `range: std::ops::Range<usize>` to `range: core::ops::Range<usize>`:

```rust
fn hex_field(s: &str, range: core::ops::Range<usize>) -> Result<u32, CanError> {
```

- [ ] **Step 6: Sweep `isotp.rs` imports**

Replace the import header of `crates/vag-can/src/isotp.rs` (the `use std::time::Duration;` line from Task 3) with:

```rust
use alloc::format;
use alloc::vec::Vec;
use core::time::Duration;
use vag_transport::{AsyncIsoTpTransport, CanId, Elapsed, Timer, TransportError};

use crate::backend::{CanBackend, to_raw_id};
```

- [ ] **Step 7: Verify the crate builds no_std**

Run: `cargo build -p vag-can`
Expected: PASS — the `not(test)` build is `no_std`, using only `core`/`alloc`/`embedded-io-async`/`vag-transport`. No tokio in the normal dependency graph.

- [ ] **Step 8: Verify tests still pass (std test build)**

Run: `cargo test -p vag-can`
Expected: PASS — codec, backend (duplex+MockTimer), isotp (MockTimer) tests all pass under the `test`-cfg std build.

- [ ] **Step 9: Full workspace gate**

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 10: Commit**

```bash
git add crates/vag-can/Cargo.toml crates/vag-can/src
git commit -m "refactor(vag-can): drop tokio normal deps, go no_std+alloc

Remove the slcan feature and tokio/tokio-serial normal deps (open moved to
vag-runtime-tokio). Add no_std+alloc; sweep std::* imports to core/alloc. tokio
and embedded-io-adapters remain dev-deps (test executor + duplex bridge).

Assisted-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: decc3d5d-5524-428e-b26f-67a30857bc30"
```

---

### Task 5: `vag-protocol` — go `no_std`

Adds `#![cfg_attr(not(test), no_std)]`+alloc and sweeps `std::*` to `core`/`alloc`. The `AsyncUdsClient` needs NO `Timer`: it only awaits `transport.recv(duration)`, so it stays untouched. `vag-protocol` already has no tokio in normal deps (dev-dep only).

**Files:**
- Modify: `crates/vag-protocol/src/lib.rs:1` (attrs + `extern crate alloc;`).
- Modify: `crates/vag-protocol/src/pdu.rs:4` (core `Duration`, alloc `Vec`/`format`).
- Modify: `crates/vag-protocol/src/uds.rs:1` (alloc `String`/`Vec`/`format`).
- Modify: `crates/vag-protocol/src/isotp.rs:1` (core `Duration`, alloc `Vec`/`format`).
- Modify: `crates/vag-protocol/src/uds_async.rs:9` (alloc `Vec`).

**Interfaces:**
- Consumes: `vag_transport::{AsyncIsoTpTransport, IsoTpTransport, …}` (unchanged).
- Produces: `vag-protocol` is a `no_std`+alloc lib; `UdsClient`, `AsyncUdsClient`, `SoftwareIsoTp`, `UdsError`, `RawDtc` unchanged.

- [ ] **Step 1: Add no_std attrs to `lib.rs`**

Replace the whole of `crates/vag-protocol/src/lib.rs` (lines 1-9) with:

```rust
#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod dtc;
pub mod isotp;
mod pdu;
pub mod uds;
pub mod uds_async;
pub use dtc::RawDtc;
pub use isotp::SoftwareIsoTp;
pub use uds::{UdsClient, UdsError};
pub use uds_async::AsyncUdsClient;
```

- [ ] **Step 2: Sweep `pdu.rs` imports**

Replace lines 1-7 of `crates/vag-protocol/src/pdu.rs` with:

```rust
//! Transport-agnostic UDS PDU encoding/decoding shared by the sync and async
//! clients. Pure functions over byte slices — no I/O, no timing.

use alloc::format;
use alloc::vec::Vec;
use core::time::Duration;

use crate::dtc::RawDtc;
use crate::uds::UdsError;
```

- [ ] **Step 3: Sweep `uds.rs` imports**

Replace line 1 of `crates/vag-protocol/src/uds.rs` (`use vag_transport::{IsoTpTransport, TransportError};`) with:

```rust
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use vag_transport::{IsoTpTransport, TransportError};
```

(The `#[cfg(test)] mod tests` keeps its `use std::time::Duration;` / `use std::collections::VecDeque;` — those compile under the `test`-cfg std build.)

- [ ] **Step 4: Sweep `isotp.rs` imports**

Replace line 1 of `crates/vag-protocol/src/isotp.rs` (`use std::time::Duration;`) with:

```rust
use alloc::format;
use alloc::vec::Vec;
use core::time::Duration;
```

- [ ] **Step 5: Sweep `uds_async.rs` imports**

Replace the import block near the top of `crates/vag-protocol/src/uds_async.rs` (line 9, `use vag_transport::AsyncIsoTpTransport;`) with:

```rust
use alloc::vec::Vec;
use vag_transport::AsyncIsoTpTransport;
```

- [ ] **Step 6: Verify no_std build + tests**

Run: `cargo build -p vag-protocol`
Expected: PASS — `not(test)` build is `no_std`; only `core`/`alloc`/`vag-transport`/`thiserror`(2).

Run: `cargo test -p vag-protocol`
Expected: PASS — sync UDS, async UDS (MockAsyncTransport), and SoftwareIsoTp tests pass.

- [ ] **Step 7: Full workspace gate**

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/vag-protocol/src
git commit -m "refactor(vag-protocol): go no_std+alloc

Add no_std+alloc; sweep std::* to core/alloc. AsyncUdsClient needs no Timer — it
only awaits transport.recv(duration) — so the async UDS path is unchanged. No
tokio in normal deps (dev-dep test executor only).

Assisted-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: decc3d5d-5524-428e-b26f-67a30857bc30"
```

---

### Task 6: New crate `vag-runtime-tokio` — the desktop adapter

Creates the only place tokio appears in the shipping graph: `TokioTimer` (implements `Timer` via `tokio::time`), a `tokio-serial`→`embedded-io-async` bridge, and `open_slcan(...)` that wires a live `IsoTpCan` for the desktop CLI.

**Files:**
- Create: `crates/vag-runtime-tokio/Cargo.toml`.
- Create: `crates/vag-runtime-tokio/src/lib.rs`.
- Modify: `Cargo.toml` (root) `[workspace] members` — add `crates/vag-runtime-tokio`.

**Interfaces:**
- Consumes: `vag_transport::{Timer, Elapsed}`; `vag_can::{SlcanBackend, IsoTpCan, SlcanBitrate, CanBackend, CanError}`; `embedded_io_adapters::tokio_1::FromTokio`; `tokio_serial::SerialStream`.
- Produces:
  - `pub struct TokioTimer;` — `#[derive(Debug, Clone, Copy, Default)]`, `impl Timer for TokioTimer`.
  - `pub async fn open_slcan(path: &str, baud: u32, bitrate: SlcanBitrate) -> Result<IsoTpCan<SlcanBackend<FromTokio<SerialStream>, TokioTimer>, TokioTimer>, CanError>`.

- [ ] **Step 1: Register the crate in the workspace**

Edit `Cargo.toml` (root) `[workspace] members` to add the new crate:

```toml
members = ["crates/vag-transport", "crates/vag-capture", "crates/vag-protocol", "crates/vag-data", "crates/vag-db", "crates/vag-hex", "crates/vag-can", "crates/vag-runtime-tokio", "crates/vagcan"]
```

- [ ] **Step 2: Create the crate manifest**

Create `crates/vag-runtime-tokio/Cargo.toml`:

```toml
[package]
name = "vag-runtime-tokio"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
vag-transport = { path = "../vag-transport" }
vag-can = { path = "../vag-can" }
tokio = { workspace = true, features = ["time"] }
tokio-serial = "5.4"
embedded-io-adapters = { workspace = true }

[dev-dependencies]
tokio = { workspace = true, features = ["rt", "macros", "time", "io-util"] }
```

- [ ] **Step 3: Write the failing `TokioTimer` + integration tests**

Create `crates/vag-runtime-tokio/src/lib.rs` with ONLY the tests first (so the step fails on missing items):

```rust
//! Desktop (Windows/Linux/macOS) runtime adapter: the only crate that names
//! tokio in the shipping graph. Supplies `TokioTimer` (the portable `Timer`
//! seam over `tokio::time`) and `open_slcan`, which bridges a `tokio-serial`
//! port into the `no_std` core's `embedded-io-async` byte seam.

#[cfg(test)]
mod tests {
    use super::*;
    use core::time::Duration;
    use embedded_io_adapters::tokio_1::FromTokio;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use vag_can::{CanBackend, SlcanBackend};
    use vag_transport::{Elapsed, Timer};

    #[tokio::test]
    async fn tokio_timer_times_out() {
        let t = TokioTimer;
        let r: Result<(), Elapsed> = t
            .timeout(Duration::from_millis(5), async {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            })
            .await;
        assert_eq!(r, Err(Elapsed));
    }

    #[tokio::test]
    async fn tokio_timer_passes_when_future_is_fast() {
        let t = TokioTimer;
        let r = t.timeout(Duration::from_secs(1), async { 7u8 }).await;
        assert_eq!(r, Ok(7));
    }

    #[tokio::test]
    async fn slcan_roundtrips_over_duplex_with_tokio_timer() {
        let (client, mut adapter) = tokio::io::duplex(256);
        let mut backend = SlcanBackend::new(FromTokio::new(client), TokioTimer);

        // Send a frame -> ASCII slcan line on the wire.
        backend.send_frame(0x7E0, &[0x02, 0x10, 0x03]).await.unwrap();
        let mut got = vec![0u8; 32];
        let n = adapter.read(&mut got).await.unwrap();
        assert_eq!(&got[..n], b"t7E03021003\r");

        // Feed a `t...` line -> decoded frame.
        adapter.write_all(b"t7E825003\r").await.unwrap();
        let (id, data) = backend.recv_frame(Duration::from_millis(200)).await.unwrap();
        assert_eq!(id, 0x7E8);
        assert_eq!(data, vec![0x50, 0x03]);
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail**

Run: `cargo test -p vag-runtime-tokio`
Expected: FAIL — `cannot find type TokioTimer in this scope`.

- [ ] **Step 5: Implement `TokioTimer` and `open_slcan`**

Prepend to `crates/vag-runtime-tokio/src/lib.rs` (above the `#[cfg(test)] mod tests`):

```rust
use core::future::Future;
use core::time::Duration;

use embedded_io_adapters::tokio_1::FromTokio;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use vag_can::{CanError, IsoTpCan, SlcanBackend, SlcanBitrate};
use vag_transport::{Elapsed, Timer};

/// [`Timer`] over `tokio::time`. Zero-sized; cloned freely into the core's
/// `SlcanBackend`/`IsoTpCan` fields.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioTimer;

impl Timer for TokioTimer {
    async fn timeout<F: Future>(&self, dur: Duration, fut: F) -> Result<F::Output, Elapsed> {
        tokio::time::timeout(dur, fut).await.map_err(|_| Elapsed)
    }

    async fn sleep(&self, dur: Duration) {
        tokio::time::sleep(dur).await;
    }
}

/// Open a real slcan serial adapter, open its CAN channel, and wrap it as a
/// per-ECU (`0x7E0`/`0x7E8`) ISO-TP channel driven by [`TokioTimer`].
///
/// This is the desktop replacement for the old `vag_can::SlcanBackend::open`:
/// it bridges `tokio-serial`'s `SerialStream` into the core's
/// `embedded-io-async` byte seam via `FromTokio`.
pub async fn open_slcan(
    path: &str,
    baud: u32,
    bitrate: SlcanBitrate,
) -> Result<IsoTpCan<SlcanBackend<FromTokio<SerialStream>, TokioTimer>, TokioTimer>, CanError> {
    let stream = tokio_serial::new(path, baud)
        .open_native_async()
        .map_err(|e| CanError::Io(e.to_string()))?;
    let mut backend = SlcanBackend::new(FromTokio::new(stream), TokioTimer);
    backend.open_channel(bitrate).await?;
    Ok(IsoTpCan::for_ecu(backend, 0, TokioTimer))
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p vag-runtime-tokio`
Expected: PASS — 3 passed (`tokio_timer_times_out`, `tokio_timer_passes_when_future_is_fast`, `slcan_roundtrips_over_duplex_with_tokio_timer`).

- [ ] **Step 7: Confirm `open_slcan` type-checks against the real serial type**

Run: `cargo build -p vag-runtime-tokio`
Expected: PASS — `SlcanBackend<FromTokio<SerialStream>, TokioTimer>` satisfies `CanBackend` (`SerialStream` and `TokioTimer` are `Send`), and `IsoTpCan::for_ecu` accepts it.

- [ ] **Step 8: Full workspace gate**

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml crates/vag-runtime-tokio
git commit -m "feat(vag-runtime-tokio): desktop adapter (TokioTimer + open_slcan)

New crate: the only place tokio appears in the shipping graph. TokioTimer
implements the portable Timer seam via tokio::time; open_slcan bridges a
tokio-serial SerialStream into embedded-io-async (FromTokio) and wires an
IsoTpCan<SlcanBackend<..>, TokioTimer> for the CLI. Recreates the constructor
removed from vag-can. Duplex integration test proves end-to-end roundtrip.

Assisted-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: decc3d5d-5524-428e-b26f-67a30857bc30"
```

---

### Task 7: Workspace gate + docs — M1 done

Final full-workspace verification and status-doc updates recording that the core is now `no_std`+alloc portable with `vag-runtime-tokio` as the desktop adapter. `vagcan info` wiring over CAN stays Track A (vin-info), unaffected by this below-the-seam refactor.

**Files:**
- Modify: `README.md:16-26` (status table — add core-portable + adapter rows/notes).
- Modify: `todo/README.md:30` (generic-can row note) + a short M1-done note.

**Interfaces:**
- Consumes: nothing (docs only).
- Produces: nothing.

- [ ] **Step 1: Full-workspace verification (evidence before claims)**

Run: `cargo build --workspace`
Expected: PASS.

Run: `cargo test --workspace`
Expected: PASS — all crates, including the new `vag-runtime-tokio`.

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings.

- [ ] **Step 2: Confirm the core builds with no tokio in the normal graph**

Run: `cargo tree -p vag-can --edges normal | grep -i tokio || echo "NO tokio in vag-can normal deps"`
Expected: prints `NO tokio in vag-can normal deps`.

Run: `cargo tree -p vag-protocol --edges normal | grep -i tokio || echo "NO tokio in vag-protocol normal deps"`
Expected: prints `NO tokio in vag-protocol normal deps`.

- [ ] **Step 3: Update `README.md` status table**

In `README.md`, replace the generic-CAN status row (line 25):

```markdown
| Generic USB-CAN bypass transport (slcan) | `vag-can` | 🟡 built, untested on hardware |
```

with these two rows:

```markdown
| Generic USB-CAN bypass transport (slcan) | `vag-can` | 🟡 built, untested on hardware |
| Portable core (`no_std`+alloc, executor-agnostic) + desktop runtime adapter | `vag-transport`/`vag-can`/`vag-protocol`, `vag-runtime-tokio` | ✅ M1 done — two seams (`embedded-io-async` bytes + `Timer` trait); tokio contained in `vag-runtime-tokio`; M2 = `vag-runtime-esp` |
```

- [ ] **Step 4: Update `todo/README.md`**

In `todo/README.md`, replace the generic-can row (line 30):

```markdown
| generic-can | vag-can | `SlcanBackend` + `IsoTpCan` (the bypass transport — built, untested on hw) |
```

with:

```markdown
| generic-can | vag-can | `SlcanBackend` + `IsoTpCan` (the bypass transport — built, untested on hw) |
| cross-platform-runtime M1 | vag-transport/vag-can/vag-protocol + vag-runtime-tokio | ✅ core now `no_std`+alloc, executor-agnostic (`embedded-io-async` + `Timer`); `TokioTimer`/`open_slcan` in `vag-runtime-tokio`. M2 = `vag-runtime-esp` (ESP32-S3). Does NOT affect Track A / `vagcan info` (vin-info) wiring. |
```

- [ ] **Step 5: Verify docs render (no build impact) and workspace still green**

Run: `cargo build --workspace && cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings`
Expected: PASS, no warnings (docs-only change; confirms nothing regressed).

- [ ] **Step 6: Commit**

```bash
git add README.md todo/README.md
git commit -m "docs: cross-platform-runtime M1 done — portable core + tokio adapter

Core (vag-transport/vag-can/vag-protocol) is now no_std+alloc and
executor-agnostic via two seams (embedded-io-async + Timer); tokio lives only in
vag-runtime-tokio (TokioTimer + open_slcan). M2 = vag-runtime-esp. vagcan info
over CAN remains Track A (vin-info), unaffected by this below-the-seam refactor.

Assisted-By: Claude Fable 5 <noreply@anthropic.com>
Claude-Session: decc3d5d-5524-428e-b26f-67a30857bc30"
```

---

## Self-Review

**Spec coverage (M1 items → tasks):**
- `vag-transport` no_std+alloc, `core::time::Duration`, add `Timer`+`Elapsed` (spec §5.2, §4.2, M1.1) → Task 1.
- `MockTimer` deterministic time source for tests (spec §12) → Task 1.
- `SlcanBackend<S>` → `embedded-io-async` (spec §4.1, §5.1) → Task 2.
- `read_line`/`IsoTpCan` → `Timer`; STmin `sleep` (spec §4.2, §5.1) → Tasks 2 & 3.
- Absolute-deadline refactor, no monotonic clock (spec §5.1, §6.3, deviation #2) → Tasks 2 (slcan) & 3 (isotp).
- Remove `tokio`/`tokio-serial` normal deps + `slcan` feature; `vag-can` no_std (spec §5.1, deviation #4, M1.2) → Task 4 (feature/dep removal) + Task 2 (constructor removal).
- `vag-protocol` no_std+alloc; confirm no Timer needed (spec §5.3, deviation #3, M1.3) → Task 5.
- New `vag-runtime-tokio`: `TokioTimer`, serial→`embedded-io-async`, constructor (spec §7.1, M1.4) → Task 6.
- Desktop stays green throughout; tri-platform free from `tokio-serial` (spec §2, M1.5) → per-task gates + Task 7.
- `Elapsed` → `TransportError::Timeout`/`CanError::Timeout` mapping (spec §9) → Tasks 2 & 3 (in `read_line`/`recv_own`/`recv`).
- Version pinning of `embedded-io-*` in `[workspace.dependencies]` (spec §9) → Task 1.
- `vagcan` NOT wired to `vag-can` in M1; that is Track A (spec §5 note, §11) → Task 7 doc note; no Task touches `crates/vagcan`.

**Items intentionally out of scope (M2, per spec §7.2, §11):** `vag-runtime-esp`, `EspUsbCdc`, `EspTimer`, ESP32 hardware checkpoint — not in this plan.

**Item needing a call-out not spelled out in the spec:** the spec assumes the no_std flip "just works," but `thiserror` 1 is std-only. This plan adds a workspace `thiserror` 1→2 bump (Task 1) — required for `TransportError`/`CanError`/`UdsError` to derive `Error` in a no_std lib. Confirmed compatible: the only thiserror users are `vag-transport`, `vag-can`, `vag-protocol` (`#[from]` in `UdsError`), and desktop `vag-hex` (plain `#[error("…")]`) — all patterns thiserror 2 supports unchanged.

**Placeholder scan:** none — every code step shows complete code; every command has an expected result. No "TBD"/"similar to"/"handle errors".

**Type consistency:** `SlcanBackend<S, T>` (`new(stream, timer)`), `IsoTpCan<B, T>` (`new(backend, tx, rx, timer)`, `for_ecu(backend, n, timer)`), `Timer` (`timeout` + `sleep`), `Elapsed`, `MockTimer` (`passthrough()`/`immediate()`), `TokioTimer`, `open_slcan(path, baud, bitrate) -> IsoTpCan<SlcanBackend<FromTokio<SerialStream>, TokioTimer>, TokioTimer>` — names and signatures identical across Tasks 1-6.

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-07-06-cross-platform-runtime-m1.md`. Two execution options:

**1. Subagent-Driven (recommended)** — dispatch a fresh subagent per task, review between tasks, fast iteration (REQUIRED SUB-SKILL: superpowers:subagent-driven-development).

**2. Inline Execution** — execute tasks in this session using executing-plans, batch execution with checkpoints (REQUIRED SUB-SKILL: superpowers:executing-plans).

Which approach?
