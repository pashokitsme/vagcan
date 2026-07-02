# vagcan P0 — Protocol Stack on Mocks — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and fully test the ISO-TP + UDS diagnostic protocol stack against mock/replay transports, with zero hardware, so later phases (cable RE, data DB, CLI) plug into a proven core.

**Architecture:** A Cargo workspace of three library crates. `vag-transport` defines hardware-agnostic traits (`RawCanTransport`, `IsoTpTransport`) plus a scripted mock. `vag-capture` defines a JSON-lines record/replay format and a replay transport. `vag-protocol` implements software ISO-TP (ISO 15765-2) over any `RawCanTransport`, and a UDS client (ISO 14229) over any `IsoTpTransport`, with a read-only service allowlist. Everything is validated with deterministic scripted exchanges.

**Tech Stack:** Rust (edition 2021), `thiserror` (error types), `serde` + `serde_json` (capture format). No async in P0 (blocking, simplest to test). No network, no hardware.

## Global Constraints

- Rust edition **2021**, minimum toolchain **1.75**. Copy into every `Cargo.toml`.
- **Read-only:** UDS write services (`0x2E`, `0x31`, `0x27`, `0x11`, `0x14`) are **not implemented** in P0. The UDS client enforces a service allowlist and returns `UdsError::Forbidden` for anything not on it.
- Classic CAN only (data length 0–8 bytes). No CAN-FD in P0.
- Crate names: `vag-transport`, `vag-capture`, `vag-protocol`. Library crates only (no binaries in P0).
- Every task ends with a green `cargo test` for the touched crate and a commit.
- No external network access anywhere in the code or tests.

---

### Task 1: Workspace + `vag-transport` core types

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/vag-transport/Cargo.toml`
- Create: `crates/vag-transport/src/lib.rs`
- Create: `crates/vag-transport/src/frame.rs`
- Create: `crates/vag-transport/src/error.rs`

**Interfaces:**
- Produces:
  - `enum CanId { Standard(u16), Extended(u32) }` — `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]`
  - `struct CanFrame { pub id: CanId, pub data: Vec<u8> }` — `#[derive(Debug, Clone, PartialEq, Eq)]`; `CanFrame::new(id, data)` panics if `data.len() > 8`
  - `enum TransportError { Io(String), Timeout, Disconnected, Unsupported(&'static str), Protocol(String) }` — `#[derive(thiserror::Error, Debug)]`

- [ ] **Step 1: Create the workspace root `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = ["crates/vag-transport", "crates/vag-capture", "crates/vag-protocol"]

[workspace.package]
edition = "2021"
rust-version = "1.75"

[workspace.dependencies]
thiserror = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

- [ ] **Step 2: Create `crates/vag-transport/Cargo.toml`**

```toml
[package]
name = "vag-transport"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
thiserror.workspace = true
```

- [ ] **Step 3: Write the failing test in `crates/vag-transport/src/frame.rs`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanId {
    Standard(u16),
    Extended(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanFrame {
    pub id: CanId,
    pub data: Vec<u8>,
}

impl CanFrame {
    pub fn new(id: CanId, data: Vec<u8>) -> Self {
        assert!(data.len() <= 8, "classic CAN frame data must be <= 8 bytes");
        CanFrame { id, data }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_holds_id_and_data() {
        let f = CanFrame::new(CanId::Standard(0x7E0), vec![0x02, 0x10, 0x03]);
        assert_eq!(f.id, CanId::Standard(0x7E0));
        assert_eq!(f.data, vec![0x02, 0x10, 0x03]);
    }

    #[test]
    #[should_panic(expected = "must be <= 8 bytes")]
    fn frame_rejects_oversized_data() {
        CanFrame::new(CanId::Standard(0x7E0), vec![0; 9]);
    }
}
```

- [ ] **Step 4: Create `crates/vag-transport/src/error.rs`**

```rust
#[derive(thiserror::Error, Debug)]
pub enum TransportError {
    #[error("io error: {0}")]
    Io(String),
    #[error("timeout")]
    Timeout,
    #[error("disconnected")]
    Disconnected,
    #[error("not supported: {0}")]
    Unsupported(&'static str),
    #[error("protocol error: {0}")]
    Protocol(String),
}
```

- [ ] **Step 5: Create `crates/vag-transport/src/lib.rs`**

```rust
pub mod error;
pub mod frame;

pub use error::TransportError;
pub use frame::{CanFrame, CanId};
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p vag-transport`
Expected: PASS (2 tests in `frame`)

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/vag-transport
git commit -m "feat(transport): workspace + CanFrame/CanId/TransportError"
```

---

### Task 2: Transport traits + scripted mock

**Files:**
- Create: `crates/vag-transport/src/traits.rs`
- Create: `crates/vag-transport/src/mock.rs`
- Modify: `crates/vag-transport/src/lib.rs`

**Interfaces:**
- Consumes: `CanFrame`, `CanId`, `TransportError` (Task 1)
- Produces:
  - `trait RawCanTransport { fn send_frame(&mut self, frame: &CanFrame) -> Result<(), TransportError>; fn recv_frame(&mut self, timeout: Duration) -> Result<CanFrame, TransportError>; }`
  - `trait IsoTpTransport { fn send(&mut self, data: &[u8]) -> Result<(), TransportError>; fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError>; }`
  - `struct ScriptedCan` with `ScriptedCan::new(steps: Vec<ScriptStep>)`, and after a run `.sent()` returns `&[CanFrame]`
  - `enum ScriptStep { ExpectSend(CanFrame), Reply(CanFrame) }` — `#[derive(Debug, Clone)]`

- [ ] **Step 1: Write the failing test in `crates/vag-transport/src/mock.rs`**

```rust
use std::time::Duration;
use crate::{CanFrame, CanId, TransportError};
use crate::traits::RawCanTransport;

#[derive(Debug, Clone)]
pub enum ScriptStep {
    /// Assert the next frame the code-under-test sends equals this frame.
    ExpectSend(CanFrame),
    /// The next `recv_frame` returns this frame.
    Reply(CanFrame),
}

/// Deterministic mock: replays a scripted sequence of expected sends and canned replies.
pub struct ScriptedCan {
    steps: std::collections::VecDeque<ScriptStep>,
    sent: Vec<CanFrame>,
}

impl ScriptedCan {
    pub fn new(steps: Vec<ScriptStep>) -> Self {
        ScriptedCan { steps: steps.into(), sent: Vec::new() }
    }
    pub fn sent(&self) -> &[CanFrame] {
        &self.sent
    }
}

impl RawCanTransport for ScriptedCan {
    fn send_frame(&mut self, frame: &CanFrame) -> Result<(), TransportError> {
        match self.steps.pop_front() {
            Some(ScriptStep::ExpectSend(expected)) => {
                assert_eq!(*frame, expected, "unexpected frame sent by code under test");
                self.sent.push(frame.clone());
                Ok(())
            }
            other => panic!("send_frame called but next script step was {other:?}"),
        }
    }

    fn recv_frame(&mut self, _timeout: Duration) -> Result<CanFrame, TransportError> {
        match self.steps.pop_front() {
            Some(ScriptStep::Reply(frame)) => Ok(frame),
            None => Err(TransportError::Timeout),
            other => panic!("recv_frame called but next script step was {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripted_can_expects_send_then_replies() {
        let tx = CanFrame::new(CanId::Standard(0x7E0), vec![0x02, 0x3E, 0x00]);
        let rx = CanFrame::new(CanId::Standard(0x7E8), vec![0x02, 0x7E, 0x00]);
        let mut can = ScriptedCan::new(vec![
            ScriptStep::ExpectSend(tx.clone()),
            ScriptStep::Reply(rx.clone()),
        ]);
        can.send_frame(&tx).unwrap();
        let got = can.recv_frame(Duration::from_millis(10)).unwrap();
        assert_eq!(got, rx);
        assert_eq!(can.sent(), &[tx]);
    }
}
```

- [ ] **Step 2: Create `crates/vag-transport/src/traits.rs`**

```rust
use std::time::Duration;
use crate::{CanFrame, TransportError};

/// Raw CAN frame I/O. Implemented by real adapters and by mocks.
pub trait RawCanTransport {
    fn send_frame(&mut self, frame: &CanFrame) -> Result<(), TransportError>;
    fn recv_frame(&mut self, timeout: Duration) -> Result<CanFrame, TransportError>;
}

/// A single ISO-TP channel bound (at construction) to one ECU's tx/rx addressing.
/// Sends/receives whole ISO-TP PDUs; the UDS client depends only on this.
pub trait IsoTpTransport {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError>;
    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError>;
}
```

- [ ] **Step 3: Update `crates/vag-transport/src/lib.rs`**

```rust
pub mod error;
pub mod frame;
pub mod mock;
pub mod traits;

pub use error::TransportError;
pub use frame::{CanFrame, CanId};
pub use mock::{ScriptStep, ScriptedCan};
pub use traits::{IsoTpTransport, RawCanTransport};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p vag-transport`
Expected: PASS (3 tests total)

- [ ] **Step 5: Commit**

```bash
git add crates/vag-transport
git commit -m "feat(transport): RawCanTransport/IsoTpTransport traits + ScriptedCan mock"
```

---

### Task 3: `vag-capture` record format (round-trip)

**Files:**
- Create: `crates/vag-capture/Cargo.toml`
- Create: `crates/vag-capture/src/lib.rs`
- Create: `crates/vag-capture/src/record.rs`

**Interfaces:**
- Consumes: `CanId` (Task 1)
- Produces:
  - `enum Direction { Tx, Rx }` — serde-tagged, `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]`
  - `enum CapturePayload { CanFrame { id: CanId, data: Vec<u8> }, CableBytes { bytes: Vec<u8> } }`
  - `struct CaptureRecord { pub ts_us: u64, pub dir: Direction, pub payload: CapturePayload }`
  - `fn write_records(w: impl std::io::Write, records: &[CaptureRecord]) -> std::io::Result<()>` (JSON-lines, one record per line)
  - `fn read_records(r: impl std::io::Read) -> std::io::Result<Vec<CaptureRecord>>`

Note: `CanId` needs `Serialize`/`Deserialize`. Add derives to it in `vag-transport` in Step 1 below.

- [ ] **Step 1: Add serde derives to `CanId` in `crates/vag-transport/src/frame.rs`**

Change the `CanId` derive line to include serde, gated so `vag-transport` stays dependency-light:

In `crates/vag-transport/Cargo.toml` add:
```toml
[dependencies]
thiserror.workspace = true
serde = { workspace = true, optional = true }

[features]
serde = ["dep:serde"]
```

In `crates/vag-transport/src/frame.rs`, replace the `CanId` derive attribute with:
```rust
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanId {
    Standard(u16),
    Extended(u32),
}
```

- [ ] **Step 2: Create `crates/vag-capture/Cargo.toml`**

```toml
[package]
name = "vag-capture"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
vag-transport = { path = "../vag-transport", features = ["serde"] }
serde.workspace = true
serde_json.workspace = true
```

- [ ] **Step 3: Write the failing test in `crates/vag-capture/src/record.rs`**

```rust
use serde::{Deserialize, Serialize};
use vag_transport::CanId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Tx,
    Rx,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapturePayload {
    CanFrame { id: CanId, data: Vec<u8> },
    CableBytes { bytes: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRecord {
    pub ts_us: u64,
    pub dir: Direction,
    pub payload: CapturePayload,
}

pub fn write_records(mut w: impl std::io::Write, records: &[CaptureRecord]) -> std::io::Result<()> {
    for rec in records {
        let line = serde_json::to_string(rec)?;
        w.write_all(line.as_bytes())?;
        w.write_all(b"\n")?;
    }
    Ok(())
}

pub fn read_records(r: impl std::io::Read) -> std::io::Result<Vec<CaptureRecord>> {
    use std::io::BufRead;
    let reader = std::io::BufReader::new(r);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_round_trip_through_jsonl() {
        let records = vec![
            CaptureRecord {
                ts_us: 1,
                dir: Direction::Tx,
                payload: CapturePayload::CanFrame {
                    id: CanId::Standard(0x7E0),
                    data: vec![0x02, 0x10, 0x03],
                },
            },
            CaptureRecord {
                ts_us: 2,
                dir: Direction::Rx,
                payload: CapturePayload::CableBytes { bytes: vec![0xAA, 0xBB] },
            },
        ];
        let mut buf = Vec::new();
        write_records(&mut buf, &records).unwrap();
        // JSON-lines: exactly two newline-terminated lines.
        assert_eq!(buf.iter().filter(|&&b| b == b'\n').count(), 2);
        let back = read_records(&buf[..]).unwrap();
        assert_eq!(back, records);
    }
}
```

- [ ] **Step 4: Create `crates/vag-capture/src/lib.rs`**

```rust
pub mod record;
pub use record::{read_records, write_records, CapturePayload, CaptureRecord, Direction};
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p vag-capture`
Expected: PASS (1 test). Also run `cargo test -p vag-transport` to confirm the serde feature change didn't break Task 1/2.

- [ ] **Step 6: Commit**

```bash
git add crates/vag-transport crates/vag-capture
git commit -m "feat(capture): JSON-lines CaptureRecord format + read/write round-trip"
```

---

### Task 4: `ReplayCan` — a `RawCanTransport` backed by a capture

**Files:**
- Create: `crates/vag-capture/src/replay.rs`
- Modify: `crates/vag-capture/src/lib.rs`

**Interfaces:**
- Consumes: `CaptureRecord`, `CapturePayload`, `Direction` (Task 3); `RawCanTransport`, `CanFrame`, `TransportError` (Tasks 1–2)
- Produces:
  - `struct ReplayCan { .. }` with `ReplayCan::new(records: Vec<CaptureRecord>) -> Self`
  - Behavior: `recv_frame` returns the next `Rx` `CanFrame` record in order; `send_frame` consumes/asserts against the next `Tx` `CanFrame` record. Non-`CanFrame` payloads (CableBytes) are skipped. Running past the end yields `TransportError::Timeout` on recv.

- [ ] **Step 1: Write the failing test in `crates/vag-capture/src/replay.rs`**

```rust
use std::collections::VecDeque;
use std::time::Duration;
use vag_transport::{CanFrame, CanId, RawCanTransport, TransportError};
use crate::record::{CapturePayload, CaptureRecord, Direction};

/// Replays a capture as a RawCanTransport: Rx records are handed to `recv_frame`,
/// Tx records are asserted against `send_frame`. CableBytes payloads are ignored.
pub struct ReplayCan {
    queue: VecDeque<CaptureRecord>,
}

impl ReplayCan {
    pub fn new(records: Vec<CaptureRecord>) -> Self {
        let queue = records
            .into_iter()
            .filter(|r| matches!(r.payload, CapturePayload::CanFrame { .. }))
            .collect();
        ReplayCan { queue }
    }
}

fn as_frame(rec: &CaptureRecord) -> CanFrame {
    match &rec.payload {
        CapturePayload::CanFrame { id, data } => CanFrame::new(*id, data.clone()),
        CapturePayload::CableBytes { .. } => unreachable!("filtered in new()"),
    }
}

impl RawCanTransport for ReplayCan {
    fn send_frame(&mut self, frame: &CanFrame) -> Result<(), TransportError> {
        match self.queue.front() {
            Some(rec) if rec.dir == Direction::Tx => {
                let expected = as_frame(rec);
                if *frame != expected {
                    return Err(TransportError::Protocol(format!(
                        "replay mismatch: sent {frame:?}, capture expected {expected:?}"
                    )));
                }
                self.queue.pop_front();
                Ok(())
            }
            _ => Err(TransportError::Protocol(
                "replay: send_frame with no matching Tx record".into(),
            )),
        }
    }

    fn recv_frame(&mut self, _timeout: Duration) -> Result<CanFrame, TransportError> {
        // Skip any leading Tx records that were not consumed (defensive), then take next Rx.
        while let Some(rec) = self.queue.front() {
            if rec.dir == Direction::Rx {
                let frame = as_frame(rec);
                self.queue.pop_front();
                return Ok(frame);
            }
            break;
        }
        Err(TransportError::Timeout)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tx(id: u16, data: Vec<u8>) -> CaptureRecord {
        CaptureRecord { ts_us: 0, dir: Direction::Tx, payload: CapturePayload::CanFrame { id: CanId::Standard(id), data } }
    }
    fn rx(id: u16, data: Vec<u8>) -> CaptureRecord {
        CaptureRecord { ts_us: 0, dir: Direction::Rx, payload: CapturePayload::CanFrame { id: CanId::Standard(id), data } }
    }

    #[test]
    fn replays_tx_then_rx_in_order() {
        let mut can = ReplayCan::new(vec![
            tx(0x7E0, vec![0x02, 0x10, 0x03]),
            rx(0x7E8, vec![0x06, 0x50, 0x03]),
        ]);
        can.send_frame(&CanFrame::new(CanId::Standard(0x7E0), vec![0x02, 0x10, 0x03])).unwrap();
        let got = can.recv_frame(Duration::from_millis(1)).unwrap();
        assert_eq!(got, CanFrame::new(CanId::Standard(0x7E8), vec![0x06, 0x50, 0x03]));
    }

    #[test]
    fn send_mismatch_is_protocol_error() {
        let mut can = ReplayCan::new(vec![tx(0x7E0, vec![0x01])]);
        let err = can.send_frame(&CanFrame::new(CanId::Standard(0x7E0), vec![0x99])).unwrap_err();
        assert!(matches!(err, TransportError::Protocol(_)));
    }
}
```

- [ ] **Step 2: Update `crates/vag-capture/src/lib.rs`**

```rust
pub mod record;
pub mod replay;
pub use record::{read_records, write_records, CapturePayload, CaptureRecord, Direction};
pub use replay::ReplayCan;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p vag-capture`
Expected: PASS (3 tests)

- [ ] **Step 4: Commit**

```bash
git add crates/vag-capture
git commit -m "feat(capture): ReplayCan RawCanTransport backed by a capture"
```

---

### Task 5: `vag-protocol` ISO-TP — single-frame send & recv

**Files:**
- Create: `crates/vag-protocol/Cargo.toml`
- Create: `crates/vag-protocol/src/lib.rs`
- Create: `crates/vag-protocol/src/isotp.rs`

**Interfaces:**
- Consumes: `RawCanTransport`, `CanFrame`, `CanId`, `TransportError`, `IsoTpTransport` (Tasks 1–2)
- Produces:
  - `struct SoftwareIsoTp<T: RawCanTransport> { .. }` with `SoftwareIsoTp::new(inner: T, tx: CanId, rx: CanId) -> Self`
  - Implements `IsoTpTransport`. This task handles only Single Frame (payload ≤ 7 bytes): send emits one SF; recv reads one SF.
  - Padding: transmitted CAN frames are right-padded to 8 bytes with `0x00`.

- [ ] **Step 1: Create `crates/vag-protocol/Cargo.toml`**

```toml
[package]
name = "vag-protocol"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true

[dependencies]
vag-transport = { path = "../vag-transport" }

[dev-dependencies]
vag-transport = { path = "../vag-transport" }
```

- [ ] **Step 2: Write the failing test in `crates/vag-protocol/src/isotp.rs`**

```rust
use std::time::Duration;
use vag_transport::{CanFrame, CanId, IsoTpTransport, RawCanTransport, TransportError};

const DEFAULT_TIMEOUT: Duration = Duration::from_millis(1000);

/// Software ISO-TP (ISO 15765-2) over a raw CAN transport, one ECU channel.
pub struct SoftwareIsoTp<T: RawCanTransport> {
    inner: T,
    tx: CanId,
    rx: CanId,
}

impl<T: RawCanTransport> SoftwareIsoTp<T> {
    pub fn new(inner: T, tx: CanId, rx: CanId) -> Self {
        SoftwareIsoTp { inner, tx, rx }
    }

    fn pad8(mut data: Vec<u8>) -> Vec<u8> {
        while data.len() < 8 {
            data.push(0x00);
        }
        data
    }
}

impl<T: RawCanTransport> IsoTpTransport for SoftwareIsoTp<T> {
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        if data.len() > 7 {
            return Err(TransportError::Unsupported("multi-frame send not yet implemented"));
        }
        // Single Frame: PCI byte high nibble = 0 (SF), low nibble = length.
        let mut frame = Vec::with_capacity(8);
        frame.push(data.len() as u8);
        frame.extend_from_slice(data);
        self.inner.send_frame(&CanFrame::new(self.tx, Self::pad8(frame)))
    }

    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let frame = self.inner.recv_frame(timeout)?;
        if frame.id != self.rx {
            return Err(TransportError::Protocol(format!(
                "unexpected rx id {:?}, want {:?}",
                frame.id, self.rx
            )));
        }
        let pci = *frame.data.first().ok_or_else(|| TransportError::Protocol("empty frame".into()))?;
        let kind = pci >> 4;
        match kind {
            0 => {
                let len = (pci & 0x0F) as usize;
                let body = frame.data.get(1..1 + len).ok_or_else(|| {
                    TransportError::Protocol("single frame length exceeds data".into())
                })?;
                Ok(body.to_vec())
            }
            _ => Err(TransportError::Unsupported("multi-frame recv not yet implemented")),
        }
    }
}

impl<T: RawCanTransport> SoftwareIsoTp<T> {
    #[allow(dead_code)]
    fn default_timeout() -> Duration {
        DEFAULT_TIMEOUT
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vag_transport::{ScriptStep, ScriptedCan};

    const TX: CanId = CanId::Standard(0x7E0);
    const RX: CanId = CanId::Standard(0x7E8);

    #[test]
    fn sends_single_frame_padded_to_8() {
        let expected = CanFrame::new(TX, vec![0x03, 0x10, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00]);
        let can = ScriptedCan::new(vec![ScriptStep::ExpectSend(expected)]);
        let mut iso = SoftwareIsoTp::new(can, TX, RX);
        iso.send(&[0x10, 0x03]).unwrap();
    }

    #[test]
    fn receives_single_frame() {
        let reply = CanFrame::new(RX, vec![0x02, 0x50, 0x03, 0, 0, 0, 0, 0]);
        let can = ScriptedCan::new(vec![ScriptStep::Reply(reply)]);
        let mut iso = SoftwareIsoTp::new(can, TX, RX);
        let got = iso.recv(Duration::from_millis(10)).unwrap();
        assert_eq!(got, vec![0x50, 0x03]);
    }
}
```

- [ ] **Step 3: Create `crates/vag-protocol/src/lib.rs`**

```rust
pub mod isotp;
pub use isotp::SoftwareIsoTp;
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p vag-protocol`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/vag-protocol
git commit -m "feat(protocol): ISO-TP single-frame send/recv over RawCanTransport"
```

---

### Task 6: ISO-TP — multi-frame send (FF + FC wait + CFs)

**Files:**
- Modify: `crates/vag-protocol/src/isotp.rs`

**Interfaces:**
- Consumes: same as Task 5.
- Produces: `SoftwareIsoTp::send` now handles payloads of 8–4095 bytes. Sequence: send First Frame (PCI `0x1` + 12-bit length, 6 data bytes), wait for a Flow Control frame (PCI `0x3`, flow status 0 = ContinueToSend), then send Consecutive Frames (PCI `0x2` + 4-bit sequence number wrapping 0–15, 7 data bytes each). This task honors FC ContinueToSend; it ignores block size and STmin (treated as 0) — acceptable for P0 mock testing.

- [ ] **Step 1: Write the failing test — add to the `tests` module in `crates/vag-protocol/src/isotp.rs`**

```rust
    #[test]
    fn sends_multi_frame_ff_then_cfs_after_flow_control() {
        // 10-byte payload -> FF carries 6 bytes, then 1 CF carries remaining 4.
        let payload: Vec<u8> = (0..10).collect();

        let ff = CanFrame::new(TX, vec![0x10, 0x0A, 0, 1, 2, 3, 4, 5]);
        let fc = CanFrame::new(RX, vec![0x30, 0x00, 0x00, 0, 0, 0, 0, 0]); // CTS, bs=0, stmin=0
        let cf = CanFrame::new(TX, vec![0x21, 6, 7, 8, 9, 0x00, 0x00, 0x00]);

        let can = ScriptedCan::new(vec![
            ScriptStep::ExpectSend(ff),
            ScriptStep::Reply(fc),
            ScriptStep::ExpectSend(cf),
        ]);
        let mut iso = SoftwareIsoTp::new(can, TX, RX);
        iso.send(&payload).unwrap();
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p vag-protocol sends_multi_frame -- --exact` (approximate; use `cargo test -p vag-protocol`)
Expected: FAIL — current `send` returns `Unsupported` for len > 7.

- [ ] **Step 3: Replace the `send` method body in `crates/vag-protocol/src/isotp.rs`**

```rust
    fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
        if data.len() <= 7 {
            let mut frame = Vec::with_capacity(8);
            frame.push(data.len() as u8);
            frame.extend_from_slice(data);
            return self.inner.send_frame(&CanFrame::new(self.tx, Self::pad8(frame)));
        }
        if data.len() > 4095 {
            return Err(TransportError::Unsupported("payload exceeds 4095 bytes"));
        }
        // First Frame: PCI = 0x1 (high nibble) + 12-bit length; 6 payload bytes follow.
        let len = data.len() as u16;
        let mut ff = Vec::with_capacity(8);
        ff.push(0x10 | ((len >> 8) as u8 & 0x0F));
        ff.push((len & 0xFF) as u8);
        ff.extend_from_slice(&data[..6]);
        self.inner.send_frame(&CanFrame::new(self.tx, ff))?;

        // Wait for Flow Control (ContinueToSend). Block size / STmin ignored in P0.
        let fc = self.inner.recv_frame(DEFAULT_TIMEOUT)?;
        let fc_pci = *fc.data.first().ok_or_else(|| TransportError::Protocol("empty FC".into()))?;
        if fc_pci >> 4 != 0x3 {
            return Err(TransportError::Protocol("expected flow control frame".into()));
        }
        if fc_pci & 0x0F != 0x0 {
            return Err(TransportError::Protocol("flow status not ContinueToSend".into()));
        }

        // Consecutive Frames: PCI = 0x2 + sequence (1..15 wrapping), 7 payload bytes each.
        let mut seq: u8 = 1;
        let mut offset = 6;
        while offset < data.len() {
            let end = (offset + 7).min(data.len());
            let mut cf = Vec::with_capacity(8);
            cf.push(0x20 | (seq & 0x0F));
            cf.extend_from_slice(&data[offset..end]);
            self.inner.send_frame(&CanFrame::new(self.tx, Self::pad8(cf)))?;
            offset = end;
            seq = seq.wrapping_add(1) & 0x0F;
            if seq == 0 {
                seq = 0; // 0x20 with seq 0 is valid after wrap from 15
            }
        }
        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p vag-protocol`
Expected: PASS (3 tests — SF send, SF recv, multi-frame send)

- [ ] **Step 5: Commit**

```bash
git add crates/vag-protocol
git commit -m "feat(protocol): ISO-TP multi-frame send (FF/FC/CF)"
```

---

### Task 7: ISO-TP — multi-frame recv (FF → send FC → collect CFs)

**Files:**
- Modify: `crates/vag-protocol/src/isotp.rs`

**Interfaces:**
- Consumes: same as Task 5.
- Produces: `SoftwareIsoTp::recv` now reassembles multi-frame responses. On receiving a First Frame it sends a Flow Control ContinueToSend (`0x30 0x00 0x00` padded) on `self.tx`, then reads Consecutive Frames until the declared length is collected. Sequence numbers are validated (must increment 1,2,…,15,0,1,…).

- [ ] **Step 1: Write the failing test — add to the `tests` module in `crates/vag-protocol/src/isotp.rs`**

```rust
    #[test]
    fn receives_multi_frame_sends_flow_control() {
        // 10-byte response: FF (len=10, 6 bytes) then CF with remaining 4 bytes.
        let ff = CanFrame::new(RX, vec![0x10, 0x0A, 0x50, 0x03, 1, 2, 3, 4]);
        let fc = CanFrame::new(TX, vec![0x30, 0x00, 0x00, 0, 0, 0, 0, 0]);
        let cf = CanFrame::new(RX, vec![0x21, 5, 6, 7, 8, 0, 0, 0]);

        let can = ScriptedCan::new(vec![
            ScriptStep::Reply(ff),
            ScriptStep::ExpectSend(fc),
            ScriptStep::Reply(cf),
        ]);
        let mut iso = SoftwareIsoTp::new(can, TX, RX);
        let got = iso.recv(Duration::from_millis(50)).unwrap();
        assert_eq!(got, vec![0x50, 0x03, 1, 2, 3, 4, 5, 6, 7, 8]);
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p vag-protocol`
Expected: FAIL — current `recv` returns `Unsupported` for FF (kind == 1).

- [ ] **Step 3: Replace the `recv` method body in `crates/vag-protocol/src/isotp.rs`**

```rust
    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, TransportError> {
        let frame = self.inner.recv_frame(timeout)?;
        if frame.id != self.rx {
            return Err(TransportError::Protocol(format!(
                "unexpected rx id {:?}, want {:?}",
                frame.id, self.rx
            )));
        }
        let pci = *frame.data.first().ok_or_else(|| TransportError::Protocol("empty frame".into()))?;
        match pci >> 4 {
            0 => {
                let len = (pci & 0x0F) as usize;
                let body = frame.data.get(1..1 + len).ok_or_else(|| {
                    TransportError::Protocol("single frame length exceeds data".into())
                })?;
                Ok(body.to_vec())
            }
            1 => {
                // First Frame: 12-bit length, 6 data bytes here.
                let len = (((pci & 0x0F) as usize) << 8) | (frame.data[1] as usize);
                let mut out: Vec<u8> = frame.data[2..8].to_vec();

                // Send Flow Control: ContinueToSend, block size 0, STmin 0.
                let fc = CanFrame::new(self.tx, Self::pad8(vec![0x30, 0x00, 0x00]));
                self.inner.send_frame(&fc)?;

                // Collect Consecutive Frames.
                let mut expected_seq: u8 = 1;
                while out.len() < len {
                    let cf = self.inner.recv_frame(timeout)?;
                    if cf.id != self.rx {
                        return Err(TransportError::Protocol("CF from unexpected id".into()));
                    }
                    let cf_pci = *cf.data.first().ok_or_else(|| TransportError::Protocol("empty CF".into()))?;
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
                    let remaining = len - out.len();
                    let take = remaining.min(7);
                    out.extend_from_slice(&cf.data[1..1 + take]);
                    expected_seq = (expected_seq + 1) & 0x0F;
                }
                Ok(out)
            }
            _ => Err(TransportError::Protocol("unexpected PCI in first frame position".into())),
        }
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p vag-protocol`
Expected: PASS (4 tests)

- [ ] **Step 5: Commit**

```bash
git add crates/vag-protocol
git commit -m "feat(protocol): ISO-TP multi-frame recv with flow control"
```

---

### Task 8: UDS client — request/response, NRC, responsePending, read-only allowlist

**Files:**
- Create: `crates/vag-protocol/src/uds.rs`
- Modify: `crates/vag-protocol/src/lib.rs`

**Interfaces:**
- Consumes: `IsoTpTransport`, `TransportError` (Task 2)
- Produces:
  - `enum UdsError { Transport(TransportError), NegativeResponse { sid: u8, nrc: u8 }, Malformed(String), Forbidden(u8) }` — `#[derive(thiserror::Error, Debug)]`
  - `struct UdsClient<C: IsoTpTransport> { .. }` with `UdsClient::new(channel: C) -> Self`
  - `fn request(&mut self, sid: u8, payload: &[u8]) -> Result<Vec<u8>, UdsError>` — returns the response bytes *after* the echoed SID. Enforces the read-only allowlist: `sid` must be in `{0x10, 0x11-excluded, 0x14-excluded, 0x19, 0x22, 0x3E}`. Concretely allowlist = `{0x10, 0x19, 0x22, 0x3E}`. Handles `0x7F` negative responses; if NRC == `0x78` (responsePending) it re-reads until a final response arrives (bounded by `MAX_PENDING = 30` reads).
- Note: `vag-protocol/Cargo.toml` needs `thiserror`. Add it in Step 1.

- [ ] **Step 1: Add `thiserror` to `crates/vag-protocol/Cargo.toml` dependencies**

```toml
[dependencies]
vag-transport = { path = "../vag-transport" }
thiserror.workspace = true
```

- [ ] **Step 2: Write the failing test in `crates/vag-protocol/src/uds.rs`**

```rust
use std::time::Duration;
use vag_transport::{IsoTpTransport, TransportError};

const RESPONSE_TIMEOUT: Duration = Duration::from_millis(2000);
const MAX_PENDING: usize = 30;
const READ_ONLY_ALLOWLIST: &[u8] = &[0x10, 0x19, 0x22, 0x3E];

#[derive(thiserror::Error, Debug)]
pub enum UdsError {
    #[error("transport: {0}")]
    Transport(#[from] TransportError),
    #[error("negative response: sid=0x{sid:02X} nrc=0x{nrc:02X}")]
    NegativeResponse { sid: u8, nrc: u8 },
    #[error("malformed response: {0}")]
    Malformed(String),
    #[error("service 0x{0:02X} blocked by read-only allowlist")]
    Forbidden(u8),
}

pub struct UdsClient<C: IsoTpTransport> {
    channel: C,
}

impl<C: IsoTpTransport> UdsClient<C> {
    pub fn new(channel: C) -> Self {
        UdsClient { channel }
    }

    /// Send a UDS request; return response bytes after the echoed SID.
    pub fn request(&mut self, sid: u8, payload: &[u8]) -> Result<Vec<u8>, UdsError> {
        if !READ_ONLY_ALLOWLIST.contains(&sid) {
            return Err(UdsError::Forbidden(sid));
        }
        let mut req = Vec::with_capacity(1 + payload.len());
        req.push(sid);
        req.extend_from_slice(payload);
        self.channel.send(&req)?;

        for _ in 0..MAX_PENDING {
            let resp = self.channel.recv(RESPONSE_TIMEOUT)?;
            let first = *resp.first().ok_or_else(|| UdsError::Malformed("empty response".into()))?;
            if first == 0x7F {
                // Negative: [0x7F, sid, nrc]
                let nrc = *resp.get(2).ok_or_else(|| UdsError::Malformed("short negative response".into()))?;
                if nrc == 0x78 {
                    // responsePending: read again.
                    continue;
                }
                let echoed = *resp.get(1).unwrap_or(&sid);
                return Err(UdsError::NegativeResponse { sid: echoed, nrc });
            }
            if first != sid + 0x40 {
                return Err(UdsError::Malformed(format!(
                    "response SID 0x{first:02X} does not match request 0x{:02X}",
                    sid + 0x40
                )));
            }
            return Ok(resp[1..].to_vec());
        }
        Err(UdsError::Malformed("too many responsePending replies".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Mock IsoTp channel: canned responses, records sent PDUs.
    struct MockChannel {
        replies: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
    }
    impl MockChannel {
        fn new(replies: Vec<Vec<u8>>) -> Self {
            MockChannel { replies: replies.into(), sent: Vec::new() }
        }
    }
    impl IsoTpTransport for MockChannel {
        fn send(&mut self, data: &[u8]) -> Result<(), TransportError> {
            self.sent.push(data.to_vec());
            Ok(())
        }
        fn recv(&mut self, _t: Duration) -> Result<Vec<u8>, TransportError> {
            self.replies.pop_front().ok_or(TransportError::Timeout)
        }
    }

    #[test]
    fn positive_response_strips_sid() {
        let ch = MockChannel::new(vec![vec![0x62, 0xF1, 0x90, 0xAB]]); // resp to 0x22
        let mut uds = UdsClient::new(ch);
        let data = uds.request(0x22, &[0xF1, 0x90]).unwrap();
        assert_eq!(data, vec![0xF1, 0x90, 0xAB]);
    }

    #[test]
    fn negative_response_surfaces_nrc() {
        let ch = MockChannel::new(vec![vec![0x7F, 0x22, 0x31]]); // requestOutOfRange
        let mut uds = UdsClient::new(ch);
        let err = uds.request(0x22, &[0x00, 0x00]).unwrap_err();
        assert!(matches!(err, UdsError::NegativeResponse { sid: 0x22, nrc: 0x31 }));
    }

    #[test]
    fn response_pending_then_final() {
        let ch = MockChannel::new(vec![
            vec![0x7F, 0x22, 0x78], // pending
            vec![0x7F, 0x22, 0x78], // pending
            vec![0x62, 0xF1, 0x90], // final positive
        ]);
        let mut uds = UdsClient::new(ch);
        let data = uds.request(0x22, &[0xF1, 0x90]).unwrap();
        assert_eq!(data, vec![0xF1, 0x90]);
    }

    #[test]
    fn write_service_is_forbidden() {
        let ch = MockChannel::new(vec![]);
        let mut uds = UdsClient::new(ch);
        let err = uds.request(0x2E, &[0xF1, 0x90, 0x01]).unwrap_err();
        assert!(matches!(err, UdsError::Forbidden(0x2E)));
    }
}
```

- [ ] **Step 3: Update `crates/vag-protocol/src/lib.rs`**

```rust
pub mod isotp;
pub mod uds;
pub use isotp::SoftwareIsoTp;
pub use uds::{UdsClient, UdsError};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p vag-protocol`
Expected: PASS (8 tests total)

- [ ] **Step 5: Commit**

```bash
git add crates/vag-protocol
git commit -m "feat(protocol): UDS client with NRC, responsePending, read-only allowlist"
```

---

### Task 9: UDS convenience services — RDBI (0x22), TesterPresent (0x3E), SessionControl (0x10)

**Files:**
- Modify: `crates/vag-protocol/src/uds.rs`

**Interfaces:**
- Consumes: `UdsClient::request` (Task 8)
- Produces (methods on `UdsClient`):
  - `fn read_data_by_identifier(&mut self, did: u16) -> Result<Vec<u8>, UdsError>` — sends `0x22 [did_hi did_lo]`; returns the response after the echoed 2-byte DID (validates the echo).
  - `fn tester_present(&mut self) -> Result<(), UdsError>` — sends `0x3E 0x00`.
  - `fn start_session(&mut self, session: u8) -> Result<(), UdsError>` — sends `0x10 [session]`.

- [ ] **Step 1: Write the failing test — add to the `tests` module in `crates/vag-protocol/src/uds.rs`**

```rust
    #[test]
    fn rdbi_validates_did_echo_and_returns_payload() {
        // Request DID 0xF190; response 0x62 F1 90 <data...>
        let ch = MockChannel::new(vec![vec![0x62, 0xF1, 0x90, b'W', b'V', b'W']]);
        let mut uds = UdsClient::new(ch);
        let data = uds.read_data_by_identifier(0xF190).unwrap();
        assert_eq!(data, vec![b'W', b'V', b'W']);
    }

    #[test]
    fn rdbi_rejects_wrong_did_echo() {
        let ch = MockChannel::new(vec![vec![0x62, 0xF1, 0x91, 0x00]]); // wrong DID echoed
        let mut uds = UdsClient::new(ch);
        let err = uds.read_data_by_identifier(0xF190).unwrap_err();
        assert!(matches!(err, UdsError::Malformed(_)));
    }

    #[test]
    fn tester_present_ok() {
        let ch = MockChannel::new(vec![vec![0x7E, 0x00]]);
        let mut uds = UdsClient::new(ch);
        uds.tester_present().unwrap();
    }

    #[test]
    fn start_session_ok() {
        let ch = MockChannel::new(vec![vec![0x50, 0x03, 0, 0x32, 0x01, 0xF4]]);
        let mut uds = UdsClient::new(ch);
        uds.start_session(0x03).unwrap();
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p vag-protocol`
Expected: FAIL — methods don't exist (compile error).

- [ ] **Step 3: Add the methods inside `impl<C: IsoTpTransport> UdsClient<C>` in `crates/vag-protocol/src/uds.rs`**

```rust
    pub fn read_data_by_identifier(&mut self, did: u16) -> Result<Vec<u8>, UdsError> {
        let resp = self.request(0x22, &[(did >> 8) as u8, (did & 0xFF) as u8])?;
        let echoed = resp
            .get(0..2)
            .ok_or_else(|| UdsError::Malformed("RDBI response missing DID echo".into()))?;
        if echoed != [(did >> 8) as u8, (did & 0xFF) as u8] {
            return Err(UdsError::Malformed(format!(
                "RDBI DID echo mismatch: got {echoed:02X?}, want 0x{did:04X}"
            )));
        }
        Ok(resp[2..].to_vec())
    }

    pub fn tester_present(&mut self) -> Result<(), UdsError> {
        self.request(0x3E, &[0x00])?;
        Ok(())
    }

    pub fn start_session(&mut self, session: u8) -> Result<(), UdsError> {
        self.request(0x10, &[session])?;
        Ok(())
    }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p vag-protocol`
Expected: PASS (12 tests total)

- [ ] **Step 5: Commit**

```bash
git add crates/vag-protocol
git commit -m "feat(protocol): UDS RDBI/TesterPresent/SessionControl helpers"
```

---

### Task 10: UDS ReadDTCInformation (0x19/0x02) — parse DTCs by status mask

**Files:**
- Create: `crates/vag-protocol/src/dtc.rs`
- Modify: `crates/vag-protocol/src/uds.rs`
- Modify: `crates/vag-protocol/src/lib.rs`

**Interfaces:**
- Consumes: `UdsClient::request` (Task 8)
- Produces:
  - `struct RawDtc { pub code: [u8; 3], pub status: u8 }` (in `dtc.rs`) — `#[derive(Debug, Clone, PartialEq, Eq)]`
  - `fn read_dtcs_by_status_mask(&mut self, mask: u8) -> Result<Vec<RawDtc>, UdsError>` on `UdsClient` — sends `0x19 0x02 [mask]`; response format `0x59 0x02 <availabilityMask> [dtc(3) status(1)]*`; returns the list. (Semantic decoding of the 3-byte code and status bits into human text is deferred to `vag-data` in P2 — P0 keeps raw bytes.)

- [ ] **Step 1: Create `crates/vag-protocol/src/dtc.rs`**

```rust
/// One DTC entry as returned by ReadDTCInformation subfunction 0x02:
/// 3 raw code bytes + 1 status byte. Semantic decoding happens in vag-data (P2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawDtc {
    pub code: [u8; 3],
    pub status: u8,
}
```

- [ ] **Step 2: Write the failing test — add to the `tests` module in `crates/vag-protocol/src/uds.rs`**

Add `use crate::dtc::RawDtc;` at the top of the `tests` module, then:

```rust
    #[test]
    fn read_dtcs_parses_entries() {
        // 0x59 0x02 <avail=0xFF> then two DTCs: [11 22 33 status 0x08], [44 55 66 status 0x2F]
        let ch = MockChannel::new(vec![vec![
            0x59, 0x02, 0xFF,
            0x11, 0x22, 0x33, 0x08,
            0x44, 0x55, 0x66, 0x2F,
        ]]);
        let mut uds = UdsClient::new(ch);
        let dtcs = uds.read_dtcs_by_status_mask(0xFF).unwrap();
        assert_eq!(dtcs, vec![
            RawDtc { code: [0x11, 0x22, 0x33], status: 0x08 },
            RawDtc { code: [0x44, 0x55, 0x66], status: 0x2F },
        ]);
    }

    #[test]
    fn read_dtcs_empty_list() {
        let ch = MockChannel::new(vec![vec![0x59, 0x02, 0xFF]]);
        let mut uds = UdsClient::new(ch);
        let dtcs = uds.read_dtcs_by_status_mask(0xFF).unwrap();
        assert!(dtcs.is_empty());
    }
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p vag-protocol`
Expected: FAIL — method + `RawDtc` import unresolved.

- [ ] **Step 4: Add the method inside `impl<C: IsoTpTransport> UdsClient<C>` and wire the import at top of `crates/vag-protocol/src/uds.rs`**

At the top of `uds.rs` add:
```rust
use crate::dtc::RawDtc;
```

Method:
```rust
    pub fn read_dtcs_by_status_mask(&mut self, mask: u8) -> Result<Vec<RawDtc>, UdsError> {
        // request: 0x19 0x02 <mask>; response after SID strip: 0x02 <avail> [dtc(3) status(1)]*
        let resp = self.request(0x19, &[0x02, mask])?;
        // resp[0] = subfunction echo (0x02), resp[1] = availability mask, then entries.
        if resp.len() < 2 || resp[0] != 0x02 {
            return Err(UdsError::Malformed("bad ReadDTCInformation 0x02 response".into()));
        }
        let entries = &resp[2..];
        if entries.len() % 4 != 0 {
            return Err(UdsError::Malformed("DTC entries not a multiple of 4 bytes".into()));
        }
        let mut out = Vec::with_capacity(entries.len() / 4);
        for chunk in entries.chunks_exact(4) {
            out.push(RawDtc { code: [chunk[0], chunk[1], chunk[2]], status: chunk[3] });
        }
        Ok(out)
    }
```

- [ ] **Step 5: Update `crates/vag-protocol/src/lib.rs`**

```rust
pub mod dtc;
pub mod isotp;
pub mod uds;
pub use dtc::RawDtc;
pub use isotp::SoftwareIsoTp;
pub use uds::{UdsClient, UdsError};
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p vag-protocol`
Expected: PASS (14 tests total)

- [ ] **Step 7: Commit**

```bash
git add crates/vag-protocol
git commit -m "feat(protocol): UDS ReadDTCInformation 0x19/0x02 raw DTC parsing"
```

---

### Task 11: End-to-end integration test — UDS over ISO-TP over ReplayCan

**Files:**
- Create: `crates/vag-protocol/tests/e2e_replay.rs`
- Create: `crates/vag-protocol/Cargo.toml` dev-dependency on `vag-capture`

**Interfaces:**
- Consumes: `SoftwareIsoTp` (Tasks 5–7), `UdsClient` (Tasks 8–10), `ReplayCan` + capture types (Tasks 3–4)
- Produces: a black-box test proving the full stack decodes a recorded multi-frame RDBI exchange. This is the template every future car-capture regression test will follow.

- [ ] **Step 1: Add dev-dependencies to `crates/vag-protocol/Cargo.toml`**

```toml
[dev-dependencies]
vag-capture = { path = "../vag-capture" }
vag-transport = { path = "../vag-transport", features = ["serde"] }
```

- [ ] **Step 2: Write the failing integration test `crates/vag-protocol/tests/e2e_replay.rs`**

```rust
use std::time::Duration;
use vag_capture::{CapturePayload, CaptureRecord, Direction, ReplayCan};
use vag_protocol::{SoftwareIsoTp, UdsClient};
use vag_transport::CanId;

fn tx(id: u16, data: Vec<u8>) -> CaptureRecord {
    CaptureRecord { ts_us: 0, dir: Direction::Tx, payload: CapturePayload::CanFrame { id: CanId::Standard(id), data } }
}
fn rx(id: u16, data: Vec<u8>) -> CaptureRecord {
    CaptureRecord { ts_us: 0, dir: Direction::Rx, payload: CapturePayload::CanFrame { id: CanId::Standard(id), data } }
}

/// Full stack: UdsClient -> SoftwareIsoTp -> ReplayCan.
/// Scenario: RDBI 0xF190 (VIN-like), multi-frame 17-byte response.
#[test]
fn rdbi_multiframe_over_replay() {
    // Request is a single frame: 0x03 0x22 0xF1 0x90 (padded).
    // Response is 20 bytes after SID: 0x62 F1 90 + 17 chars = 20 bytes -> multi-frame.
    // FF declares length 20: 0x10 0x14, then 6 bytes (0x62 F1 90 W V W)
    // FC from tester: 0x30 00 00
    // CFs carry the rest.
    let payload: Vec<u8> = {
        let mut v = vec![0x62, 0xF1, 0x90];
        v.extend_from_slice(b"WVWZZZ1KZAW000001"); // 17 bytes -> total 20
        v
    };
    assert_eq!(payload.len(), 20);

    let ff = rx(0x7E8, vec![0x10, 0x14, payload[0], payload[1], payload[2], payload[3], payload[4], payload[5]]);
    let fc = tx(0x7E0, vec![0x30, 0x00, 0x00, 0, 0, 0, 0, 0]);
    let cf1 = rx(0x7E8, vec![0x21, payload[6], payload[7], payload[8], payload[9], payload[10], payload[11], payload[12]]);
    let cf2 = rx(0x7E8, vec![0x22, payload[13], payload[14], payload[15], payload[16], payload[17], payload[18], payload[19]]);

    let req = tx(0x7E0, vec![0x03, 0x22, 0xF1, 0x90, 0, 0, 0, 0]);

    let records = vec![req, ff, fc, cf1, cf2];
    let can = ReplayCan::new(records);
    let iso = SoftwareIsoTp::new(can, CanId::Standard(0x7E0), CanId::Standard(0x7E8));
    let mut uds = UdsClient::new(iso);

    let data = uds.read_data_by_identifier(0xF190).unwrap();
    assert_eq!(&data, b"WVWZZZ1KZAW000001");
}
```

- [ ] **Step 3: Run the test to verify it fails, then passes**

Run: `cargo test -p vag-protocol --test e2e_replay`
Expected: PASS once Tasks 5–10 are complete. (If it fails, the failure pinpoints an ISO-TP/UDS bug — fix in the relevant module.)

- [ ] **Step 4: Run the full workspace test suite**

Run: `cargo test`
Expected: PASS (all crates green)

- [ ] **Step 5: Commit**

```bash
git add crates/vag-protocol
git commit -m "test(protocol): end-to-end RDBI multi-frame over ReplayCan"
```

---

## Self-Review

**1. Spec coverage (P0 scope):**
- Workspace + 6-crate layout → P0 builds 3 of them (`vag-transport`, `vag-capture`, `vag-protocol`); `vag-hex`/`vag-data`/`vag-core`/`vag-cli` are later phases. ✓
- Transport trait abstraction (`RawCanTransport` + `IsoTpTransport`, software ISO-TP bridging them) → Tasks 2, 5–7. ✓
- Software ISO-TP (SF/FF/CF/FC) → Tasks 5–7. ✓
- UDS services 0x10/0x22/0x19/0x3E + NRC + 0x78 → Tasks 8–10. ✓
- Read-only allowlist (write services not implemented, `Forbidden` returned) → Task 8. ✓
- Capture-once/replay-forever harness (`vag-capture` format + `ReplayCan` + `--replay` foundation) → Tasks 3–4, 11. (`--replay` CLI flag itself lands with `vag-cli` in P3; the replay transport it will use exists now.) ✓
- Testing per layer with mocks → every task is TDD; Task 11 is the end-to-end template. ✓

**2. Placeholder scan:** No TBD/TODO/"handle edge cases" left; every code step shows complete code. ✓

**3. Type consistency:** `CanFrame::new`, `CanId::{Standard,Extended}`, `RawCanTransport::{send_frame,recv_frame}`, `IsoTpTransport::{send,recv}`, `SoftwareIsoTp::new(inner,tx,rx)`, `UdsClient::{new,request,read_data_by_identifier,tester_present,start_session,read_dtcs_by_status_mask}`, `RawDtc{code,status}`, `CaptureRecord{ts_us,dir,payload}`, `CapturePayload::{CanFrame,CableBytes}`, `Direction::{Tx,Rx}`, `ReplayCan::new` — names used identically across producing and consuming tasks. ✓

**Deferred to later plans (not P0):** cable RE (`vag-hex`, P1), VCDS label parsing + DID/DTC/module DB (`vag-data`, P2), poll scheduler + keepalive (`vag-core`), CLI + TUI + `sniff` (`vag-cli`, P3–P4).
