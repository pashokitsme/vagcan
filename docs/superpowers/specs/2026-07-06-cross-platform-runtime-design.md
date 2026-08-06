# Cross-Platform Runtime — Portable Core + Runtime Adapters

**Date:** 2026-07-06
**Status:** Design proposal — pending owner spec review. Future architecture milestone; parked (see `todo/README.md`). Note: the `vag-hex` crate this spec lists among the desktop-only crates has since been deleted (HEX-clone path dead, research archived under `archive/research/`); the portable-core split is unaffected.
**Author:** Pavel Smirnov (with Claude Fable 5)
**Working name:** cross-platform runtime (`vag-runtime-*`)

---

## 1. Summary

Today the vag stack is desktop-and-tokio-shaped: `vag-can` and (transitively) the
async ISO-TP path depend on `tokio` for byte I/O (`tokio::io::{AsyncRead, AsyncWrite}`)
and timing (`tokio::time::{timeout, sleep, Instant}`). That is fine for a macOS/Linux/
Windows CLI, but it forecloses running the same protocol code on an ESP32-S3 (or, later,
WASM/WebSerial or mobile) without a rewrite.

This spec makes the stack **runtime-agnostic** by splitting the workspace into three
layers — a **portable core** (`no_std` + `alloc`, executor-agnostic), a set of
**desktop-only** crates (std, inherently non-portable, and staying that way), and thin
**runtime-adapter** crates that implement two small portable seams for one platform each.
The core depends only on `core` + `alloc` + `embedded-io-async` + its own one-method-ish
`Timer` trait — **zero tokio imports**.

Concretely, the goal is to run the full transport+protocol stack on:

- **Desktop Windows / Linux / macOS** — via `tokio-serial` (a `vag-runtime-tokio` adapter);
- **ESP32-S3** — as a USB host speaking slcan to an **MKS CANable V2.0 Pro** dongle over
  USB CDC-ACM, in the **esp-idf std** environment (a `vag-runtime-esp` adapter, milestone 2).

The transport trait seam already exists and is already runtime-agnostic
(`vag-transport::traits`: native `async fn` in trait, no `async_trait`, no tokio types in
signatures). This work extends that discipline down through the byte-I/O and timing layers
so the seam is abstract enough that future targets slot in without touching the core.

---

## 2. Goal

- **Maximize platform reach** for the portable protocol code: desktop tri-platform **and**
  ESP32-S3, with a seam clean enough that WASM/WebSerial and mobile can be added later by
  writing a new adapter crate only — **no core rewrite**. (Those extra targets are explicitly
  **not** implemented now.)
- **Contain runtime specifics** (tokio, embassy, esp-idf) entirely inside adapter crates.
  The core must compile for `thumb`/`riscv`/`xtensa`-style `no_std`+alloc targets and for
  desktop std alike, and must not name any executor.
- **Keep the desktop build, tests, and clippy 100% green throughout** the migration. On
  desktop, tri-platform support (Win/Lin/Mac) comes essentially for free because
  `tokio-serial` already covers all three; the migration's real payoff is unlocking the
  embedded target without regressing desktop.

### Non-goals

- Porting the label database (`vag-db`/`vag-data` label files) to embedded. It stays desktop-only.
- Shipping the `vag-runtime-esp` adapter in milestone 1. It is designed here at sketch depth
  and built in milestone 2.
- Replacing the existing transport traits. They are already the right shape; we build *below*
  them.

### Embedded scope line (explicit)

On the ESP32-S3 the device runs the **portable core** (transport traits + slcan codec +
software ISO-TP + UDS client) plus **minimal inline decode only**: VIN is ASCII and part
numbers are ASCII straight out of UDS reads (`22 F1 90`, `22 F1 87`, …), so basic
`vagcan info`-style output needs no label files. The full label-enrichment path
(`vag-db`/`vag-data`) stays desktop-only. Therefore `vagcan info` on the S3 yields **raw UDS
reads + basic decode**, not the label-enriched desktop output. Enriching embedded output
later (by making `vag-data`'s pure decoders `no_std`+alloc) is out of scope now and noted as
a future question in §9.

---

## 3. Chosen approach — A: layered workspace (portable core + thin runtime adapters)

We adopt **approach A**: a layered Cargo workspace where a `no_std`+alloc portable core is
kept strictly executor-agnostic, desktop-only concerns live in their own std crates, and each
target platform gets a thin adapter crate that implements exactly two portable seams. See §8
for the alternatives (B: `#[cfg]`-gated single crate; C: esp-idf std + tokio-everywhere) and
why they were rejected.

The core idea: **the core names abstractions, adapters name runtimes.** The core is generic
over `embedded_io_async::{Read, Write}` for bytes and over a `Timer` trait for time. An
adapter supplies concrete implementations of both for one platform, plus a small constructor
that wires them into a ready-to-use backend for that platform's binary.

### 3.1 Layer topology

Three layers; every existing crate is classified into exactly one.

**Portable core** — `#![no_std]` + `extern crate alloc`, executor-agnostic, must compile for
both desktop std and ESP32-S3:

| Crate | Role | Change required |
|---|---|---|
| `vag-transport` | transport traits (`RawCanTransport`, `IsoTpTransport`, `AsyncIsoTpTransport`) + `CanFrame`/`CanId` + `TransportError` + the **new** `Timer` trait | Add `#![no_std]`+alloc; `Duration` from `core::time`; tokio stays a **dev-dependency** only (it already is). Add the `Timer` trait (§6.2). |
| `vag-can` | slcan (LAWICEL) ASCII codec + stream-generic `SlcanBackend` + `IsoTpCan` (software ISO-TP over a `CanBackend`) | Byte I/O: `tokio::io` → `embedded-io-async`. Timing: `tokio::time` → the `Timer` trait. This is the crate most affected. |
| `vag-protocol` | UDS client (+ the sync software ISO-TP) | Thread the `Timer` through the async UDS path where it awaits transport. It already has **no** tokio in its normal deps (only a dev-dep), so there is little to remove here (see §5.3). `no_std`+alloc. |

**Desktop-only** — std, inherently non-portable, and that is fine; they stay std:

| Crate | Why it is desktop-only |
|---|---|
| `vag-hex` | FTDI D2XX clone cable via a native `.dylib`/`.dll` — no embedded equivalent. |
| `vag-db` | `rusqlite` → the SQLite C library. |
| `vag-data` | label parsers + on-disk label files file I/O. (Its *pure decoders* could later go `no_std`+alloc — §9.) |
| `vag-capture` | replay/test tooling (JSON-lines fixtures, host filesystem). |

**Runtime adapters** — thin; each implements the two portable seams for one platform:

| Crate | Target | Contents |
|---|---|---|
| `vag-runtime-tokio` (**NEW**, M1) | desktop Win/Lin/Mac | `TokioTimer` (`Timer` via `tokio::time`); a helper that opens a `tokio-serial` port and exposes it as an `embedded-io-async` byte stream; a constructor wiring `SlcanBackend` + `Timer` for the CLI. |
| `vag-runtime-esp` (**NEW**, M2) | ESP32-S3 | `EspUsbCdc` byte stream over esp-idf USB-host CDC-ACM (implements `embedded-io-async`); `EspTimer` (`Timer` via `embassy-time`). Sketch depth here; built in M2. |

**Binary:** `vagcan` (CLI) depends on `vag-runtime-tokio` (never on the core's I/O internals
directly).

```
                    ┌─────────────────────────────────────────┐
   desktop-only     │ vag-hex   vag-db   vag-data   vag-capture│  (std; not portable)
                    └─────────────────────────────────────────┘
                                        │ (desktop composition)
   binary                        ┌──────┴───────┐
                                 │    vagcan    │
                                 └──────┬───────┘
                                        │ depends on
   runtime adapters   ┌─────────────────┴───────┐        ┌──────────────────┐
                      │   vag-runtime-tokio      │        │  vag-runtime-esp │ (M2)
                      │ TokioTimer + serial→eio  │        │ EspTimer + CDC   │
                      └─────────────────┬────────┘        └────────┬─────────┘
                                        │  implement the two seams  │
   portable core      ┌────────────────┴───────────────────────────┴──────────┐
   (no_std + alloc)   │ vag-transport (traits + Timer)  vag-can (codec+Slcan    │
                      │ Backend + IsoTpCan)  vag-protocol (UDS client)          │
                      │ deps: core + alloc + embedded-io-async + own Timer      │
                      └────────────────────────────────────────────────────────┘
```

---

## 4. The two portable seams (heart of the design)

The core touches the outside world in exactly two ways: it moves **bytes** and it waits on
**time**. Abstract both and the core is portable.

### 4.1 Seam 1 — byte I/O via `embedded-io-async`

`SlcanBackend<S>` becomes generic over `embedded_io_async::{Read, Write}` instead of
`tokio::io::{AsyncRead, AsyncWrite}`. `embedded-io-async` is the de-facto standard async
byte-I/O trait set in the Rust embedded ecosystem, and it has first-class desktop adapters, so
no bespoke bridging code is needed on either side:

- **Desktop:** wrap the `tokio-serial` `SerialStream` as `embedded-io-async` via
  `tokio-util::compat` + the `embedded-io-adapters` crate (`embedded-io-adapters` provides the
  `tokio`/`futures` ↔ `embedded-io-async` shims). No hand-written glue.
- **ESP32-S3:** the esp-idf USB-CDC-host stream implements `embedded-io-async` natively or via
  a thin shim in `vag-runtime-esp` (§7.2).

The slcan codec is **unchanged**: `encode_frame`/`decode_frame` (`t`/`T` ASCII frames) are
pure byte→`String`/`&str`→bytes functions with no I/O, and the channel-setup sequence
(`open_channel` → `C\rS6\rO\r`, where `Rate500k = 6`; `close_channel` → `C\r`) is a plain
`write_all`. Only the trait bounds on `SlcanBackend<S>` and its `read_line`/`write_all`
helpers change from tokio's `AsyncReadExt/AsyncWriteExt` to `embedded_io_async::{Read, Write}`
(and their `read`/`write_all` methods). The existing codec unit tests stay as-is.

### 4.2 Seam 2 — a tiny `Timer` trait in `vag-transport`

The core needs to bound waits on futures (ISO-TP waits at most *N* for a PDU) and to delay
between frames (ISO-TP STmin inter-frame gap). `embedded-hal-async::DelayNs` gives us a delay
but **not** a timeout-over-a-future, which is the primitive ISO-TP actually needs — so we
define a small trait of our own in `vag-transport`. **Proposed** signature:

```rust
use core::future::Future;
use core::time::Duration;

/// Returned by `Timer::timeout` when the deadline fires before the future completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Elapsed;

/// Executor-agnostic time source for the portable core.
///
/// One implementor per runtime adapter (desktop: tokio; ESP32-S3: embassy-time).
/// Static dispatch only — callers take `T: Timer`, no `dyn`, no `async_trait`.
#[allow(async_fn_in_trait)] // same seam rationale as AsyncIsoTpTransport
pub trait Timer {
    /// Race `fut` against a deadline; `Err(Elapsed)` if the deadline hits first.
    async fn timeout<F: Future>(&self, dur: Duration, fut: F) -> Result<F::Output, Elapsed>;

    /// Sleep for `dur` (ISO-TP STmin inter-frame gap). See §6.2 for why this is here.
    async fn sleep(&self, dur: Duration);
}
```

- **`timeout`** wraps `tokio::time::timeout` (desktop) / `embassy_time::with_timeout` (S3).
- **`sleep`** wraps `tokio::time::sleep` (desktop) / `embassy_time::Timer::after` (S3).
- The async ISO-TP receive path and the slcan `read_line` deadline take a `&impl Timer`
  instead of calling tokio directly. `Duration` moves from `std::time` to `core::time`
  (same type, trivial).

Net: the core imports only `core`, `alloc`, `embedded-io-async`, and its own `Timer` — **no
tokio**. The `Timer` bound rides alongside the existing `AsyncIsoTpTransport`/`CanBackend`
static-dispatch seams (both already carry `#[allow(async_fn_in_trait)]` and a `Send` bound),
so it fits the established pattern.

---

## 5. Migration of the affected core crates

### 5.1 `vag-can` — the most-affected crate

Current coupling (measured):

- `slcan.rs` — `SlcanBackend<S>` is bounded `S: AsyncRead + AsyncWrite + Unpin + Send`;
  `read_line` uses `tokio::time::{timeout, Instant}` and `tokio::io::AsyncReadExt`.
- `isotp.rs` — `IsoTpCan<B>` uses `tokio::time::Instant` for absolute deadline math
  (`recv_own`, `wait_flow_control`) and `tokio::time::sleep(gap)` for the STmin gap between
  consecutive frames.
- `Cargo.toml` — `tokio = { features = ["time", "io-util"] }` normal dep; `tokio-serial`
  behind the `slcan` feature.

Target state:

- `SlcanBackend<S>` bounded `S: embedded_io_async::Read + embedded_io_async::Write` (+ any
  marker bounds `embedded-io-async` requires). `read_line` takes `&impl Timer` and uses
  `timer.timeout(remaining, stream.read(&mut chunk))`.
- `IsoTpCan<B>` takes a `&impl Timer`; STmin `sleep(gap)` → `timer.sleep(gap)`.
- **Absolute-deadline refactor (concrete work item):** the current `recv_own` /
  `wait_flow_control` loops compute `remaining = deadline.saturating_duration_since(now())`
  each iteration using `tokio::time::Instant::now()`. `no_std`+alloc has **no portable
  monotonic clock**, and the `Timer` trait deliberately does *not* expose `now()`/`Instant`
  (that would drag an associated clock type through the seam). Instead, wrap each bounded
  skip-loop in a single `timer.timeout(total, async { loop { … } })` — one relative deadline
  raced against the whole loop future — which removes the per-iteration `now()` call and the
  dependency on a portable monotonic clock. This is a behaviour-preserving restructure (same
  "give up after *N*"), validated by the existing timeout/skip-ack tests.
- The real serial-port constructor (`SlcanBackend::open`, behind the `slcan` feature, using
  `tokio_serial`) **moves out of the core** into `vag-runtime-tokio` (§7.1). `vag-can` no
  longer depends on `tokio-serial`. The `slcan` feature and the `tokio` normal dep are
  removed from `vag-can`; tokio remains a **dev-dependency** for the in-memory codec/backend
  tests (which today use `tokio::io::duplex` — these migrate to an `embedded-io-async`
  in-memory mock, see §12).

### 5.2 `vag-transport`

- Add `#![no_std]` + `extern crate alloc`; switch `Duration` to `core::time::Duration`
  (the traits already use `Vec` from alloc and `std::time::Duration` — the latter is the only
  std touchpoint).
- Add the `Timer` trait + `Elapsed` (§4.2).
- tokio stays a **dev-dependency** only (it already is — used by trait/mock tests). No normal
  tokio dep is added.

### 5.3 `vag-protocol`

- Add `#![no_std]` + `extern crate alloc`.
- **Correction to the initial plan:** `vag-protocol` already has **no tokio in its normal
  dependencies** (only a dev-dependency for its async tests), and its `src/isotp.rs` uses the
  **sync** `IsoTpTransport` seam (no `tokio::time` calls). So there is effectively nothing to
  "drop" here. The action is: keep it tokio-free in normal deps, and where the async UDS
  client awaits the transport, thread the `Timer` through (it needs only the async transport
  trait + `Timer`). The bulk of the async timing coupling lives in `vag-can`, not here.

---

## 6. Design details & rationale

### 6.1 Why `embedded-io-async` and not a bespoke trait

`embedded-io-async` is already the ecosystem standard: embassy, esp-hal, and most embedded
HALs speak it, and `embedded-io-adapters` bridges the desktop `tokio`/`futures` worlds for
free. A bespoke byte-I/O trait would reproduce it worse and lose the free adapters. The slcan
codec is pure byte functions, so it does not care which trait carries the bytes — only
`SlcanBackend`'s bounds change.

### 6.2 Why `Timer` has both `timeout` and `sleep` (sharpened from the decided design)

The decided design specified a single-method `Timer` with only `timeout`. Grounding in the
code shows the core also needs a **delay**: `IsoTpCan` sleeps for the STmin inter-frame gap
between consecutive frames (`isotp.rs`, `tokio::time::sleep(gap)`). A delay is not expressible
as a `timeout` over a future without an awkward never-completing future, so the **proposed**
`Timer` adds a second method, `sleep`. This is still a two-method trait, still one dependency,
and keeps the rationale intact: we need timeout-over-a-future (which `embedded-hal-async::
DelayNs` cannot give us), *and* a delay (which `DelayNs` *could* give but we fold into the
same trait to avoid a second seam). See §10 for this deviation called out explicitly.

### 6.3 Why the monotonic clock does not enter the seam

Exposing `now()`/`Instant` on `Timer` would force an associated clock type across the seam and
a portable monotonic-clock abstraction the core does not otherwise need. Restructuring the two
absolute-deadline loops in `IsoTpCan` into single `timeout`-wrapped relative waits (§5.1)
eliminates the need entirely. This keeps `Timer` to just `timeout` + `sleep`.

---

## 7. Runtime adapter design (proposed — pending owner review)

### 7.1 `vag-runtime-tokio` (M1, desktop)

Thin std crate, the only place tokio appears in the shipping dependency graph. Contents:

- **`TokioTimer`** — a unit struct implementing `Timer`: `timeout` → `tokio::time::timeout`
  (mapping its `Elapsed` to ours); `sleep` → `tokio::time::sleep`.
- **A serial-open helper** — opens a `tokio-serial` port
  (`tokio_serial::new(path, baud).open_native_async()`) and returns it wrapped as
  `embedded-io-async` via `tokio-util::compat` + `embedded-io-adapters`. This is the code that
  moves out of `vag-can::slcan::open`.
- **A CLI constructor** — e.g. `open_slcan(path, baud, SlcanBitrate) -> IsoTpCan<…>` (exact
  name pending) that opens the port, wraps it, builds `SlcanBackend`, runs `open_channel`, and
  hands back a `CanBackend`/`IsoTpCan` the `vagcan` CLI drives — with a `TokioTimer` supplied
  for all timed calls.

`vagcan` depends on `vag-runtime-tokio`; it does not touch `embedded-io-adapters` or
`tokio-serial` directly.

### 7.2 `vag-runtime-esp` (M2, ESP32-S3 — sketch depth)

- **`EspUsbCdc`** — a byte stream over esp-idf USB-host CDC-ACM (the S3 acting as USB **host**
  to the CANable dongle), implementing `embedded-io-async::{Read, Write}`. Likely wraps the C
  `usb_host` / `cdc_acm_host` component via `esp-idf-sys` if a native Rust async CDC-host is
  not yet mature (see §9).
- **`EspTimer`** — `Timer` over `embassy-time` (`with_timeout` + `Timer::after`).
- A constructor mirroring §7.1's shape, wiring `SlcanBackend` + `EspTimer` for the on-device
  `vagcan info` path (VIN + part-number reads, inline decode only).

Everything above the adapter — `SlcanBackend`, `IsoTpCan`, the UDS client — is the **same
portable-core code** the desktop uses.

---

## 8. Alternatives considered

- **B — one crate with `#[cfg]`-gated tokio/embassy.** Rejected: a single crate carrying both
  runtimes behind `cfg` flags becomes cfg-soup — every I/O and timing call sprouts a
  `#[cfg(feature = "tokio")]` / `#[cfg(feature = "embassy")]` fork, the two paths are never
  compiled together so bit-rot is invisible, testing the matrix is painful, and the runtime
  leaks straight into the core (defeating the point). The layered split makes the core have
  *no* runtime and pushes the choice to a leaf crate.
- **C — esp-idf std + tokio everywhere.** Rejected: ties the core to tokio permanently, which
  kills the future WASM/embassy/no_std targets the seam is meant to enable. tokio on esp-idf is
  also current-thread-only and heavier than embassy for an MCU. Approach A keeps tokio a
  desktop-adapter detail while the S3 uses embassy — each runtime where it is strongest.

---

## 9. Risks / open questions

- **esp-idf std + tokio interplay.** tokio on esp is current-thread-only and heavy. Contained:
  the core never depends on tokio, and `vag-runtime-esp` uses embassy-time, so this risk is
  isolated to whatever esp-idf std glue the adapter needs — it never reaches the core.
- **Rust-side maturity of esp-idf USB-host CDC-ACM.** The async Rust story for USB-**host**
  CDC-ACM on esp-idf may be immature; the adapter may need to wrap the C `cdc_acm_host`
  component through `esp-idf-sys` and present an `embedded-io-async` face over it. This is the
  main M2 unknown; the M1 desktop path does not depend on it.
- **`async fn in trait` + `no_std` `Send`-bound ergonomics.** The existing traits already use
  `#[allow(async_fn_in_trait)]` and static dispatch with caller-supplied `Send` bounds; the
  new `Timer` follows the same pattern. Some embassy executors are `?Send`; the adapter, not
  the core, decides the bound, so this stays a leaf concern.
- **`embedded-io-async` / `embedded-io-adapters` version pinning.** Desktop bridging relies on
  compatible versions of `embedded-io-async`, `embedded-io-adapters`, and `tokio-util::compat`.
  Pin these in `[workspace.dependencies]` and bump in lockstep.
- **`Timer::timeout`'s `Elapsed` vs `TransportError::Timeout`.** The core maps `Elapsed` into
  the existing `TransportError::Timeout` / `CanError::Timeout` at the ISO-TP boundary — no new
  error variant needed.
- **Making `vag-data`'s pure decoders `no_std`+alloc (future, out of scope).** To enrich
  embedded output beyond raw ASCII (e.g. run `.clb`/TEA decode on-device), the *pure* decoders
  could later move to `no_std`+alloc while file/label files I/O stays desktop-only. Not now.

---

## 10. Deviations / sharpenings from the decided design

Called out for the owner's review:

1. **`Timer` gains a `sleep` method** (the decided design specified `timeout` only). Grounding
   in `vag-can::isotp` shows the core delays for the ISO-TP STmin inter-frame gap
   (`tokio::time::sleep`), which cannot be expressed as `timeout` over a future. Proposed:
   a two-method `Timer` (`timeout` + `sleep`). Rationale unchanged (`DelayNs` alone is
   insufficient because we still need `timeout`). (§4.2, §6.2)
2. **A monotonic-clock refactor is required in `vag-can::isotp`.** The current code uses
   `tokio::time::Instant::now()` for absolute deadline math, which has no portable `no_std`
   equivalent and which we deliberately keep out of the `Timer` seam. Proposed: restructure the
   bounded skip-loops into single `timeout`-wrapped relative waits (behaviour-preserving).
   (§5.1, §6.3)
3. **`vag-protocol` has no tokio normal dep to drop.** The decided plan said "drop the tokio
   dep" for `vag-protocol`; grounding shows it already has none (only a dev-dep) and its
   `src` uses the *sync* ISO-TP seam. The real async timing/tokio coupling lives in `vag-can`.
   The `vag-protocol` action is reduced to "stay tokio-free + thread `Timer` through the async
   UDS path." (§5.3)
4. **The real slcan serial constructor moves crates.** `SlcanBackend::open` (currently in
   `vag-can` behind the `slcan`/`tokio-serial` feature) moves into `vag-runtime-tokio`, so the
   core drops its `tokio-serial` dependency and the `slcan` feature. (§5.1, §7.1)

---

## 11. Migration milestones

### M1 — decouple the core + ship `vag-runtime-tokio` (desktop stays green throughout)

1. `vag-transport`: `#![no_std]`+alloc, `core::time::Duration`, add `Timer` + `Elapsed`.
2. `vag-can`: `SlcanBackend<S>` → `embedded-io-async`; `read_line`/`IsoTpCan` → `Timer`;
   absolute-deadline refactor (§5.1); remove `tokio`/`tokio-serial` normal deps + the `slcan`
   feature; migrate codec tests to `embedded-io-async` mocks.
3. `vag-protocol`: `#![no_std]`+alloc; thread `Timer` through the async UDS path.
4. New crate `vag-runtime-tokio`: `TokioTimer`, serial→`embedded-io-async` helper, CLI
   constructor.
5. Point `vagcan` at `vag-runtime-tokio`; keep `cargo test --workspace` + `cargo clippy
   --workspace --all-targets` green at every step. Tri-platform (Win/Lin/Mac) falls out of
   `tokio-serial` for free.

**M1 does not block the in-flight clone-probe / Track A `vagcan info` work** — it is a
below-the-seam refactor that leaves the transport traits and desktop behaviour identical.

### M2 — `vag-runtime-esp` + hardware bring-up

1. New crate `vag-runtime-esp`: `EspUsbCdc` (USB-host CDC-ACM → `embedded-io-async`),
   `EspTimer` (embassy-time), constructor.
2. Hardware bring-up on a real ESP32-S3 + MKS CANable V2.0 Pro: raw CAN TX/RX first, then a
   live VIN read — a hardware checkpoint (§12).

---

## 12. Testing strategy

- **Portable core on the desktop host.** The core is exercised on the desktop with std test
  harnesses driving it through **`embedded-io-async` in-memory mock streams** (replacing the
  current `tokio::io::duplex` fixtures) and a **mock `Timer`** that is deterministic and
  advances no real wall-clock time (`timeout` resolves the inner future or returns `Elapsed`
  per the test's script; `sleep` is a no-op or a counter). This makes timing tests fast and
  non-flaky.
- **slcan codec unit tests stay as-is.** `encode_frame`/`decode_frame` are pure and unchanged;
  their existing tests carry over verbatim.
- **Desktop integration through `vag-runtime-tokio`.** An integration test opens a mock/loopback
  serial pair (or the existing duplex bridged via `embedded-io-adapters`) through the adapter's
  constructor and runs the UDS client end-to-end, mirroring today's replay-based discipline.
- **ESP32-S3 is a hardware checkpoint**, following the existing convention in `todo/README.md`
  (dongle: MKS CANable V2.0 Pro, slcan firmware, 500 kbit/s, OBD2 pin 6→H / 14→L / 4,5→G, TERM
  jumper OFF): first raw CAN TX/RX with the car, then a VIN read that the on-device path prints
  (expect `XW8AD4NE9JH008917`, the owner's golden VIN). CI stays hardware-free; the checkpoint
  is manual/opt-in, exactly like the existing `vag-hex` live smoke and slcan bring-up steps.
