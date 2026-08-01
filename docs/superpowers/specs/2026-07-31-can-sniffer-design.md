# Silent CAN sniffer + active DID scan — design

**Date:** 2026-07-31
**Status:** approved (design), not yet implemented
**Hardware:** MKS CANable V2.0 Pro, firmware `normaldotcom/canable2-fw` (slcan over USB CDC-ACM),
enumerating on macOS as `/dev/cu.usbmodem*`.

## Why

`todo/README.md` M3 is blocked because no capture ever exposed the traffic that carries
VCDS's displayed measurements. Every prior crib came from **USB captures of the HEX clone**,
where the link cipher hides the payload and — decisively — where VCDS's **group reads**
(`G004/G006/G009/…`, the source of RPM / vehicle speed / coolant) never decoded
(`research/rod-labels.md` §4.0a–§4.0c).

A second adapter on the same OBD-II bus removes that whole layer. CAN is multi-drop: the
CANable can sit on the bus in **listen-only** mode while VCDS runs a normal session, and
observe every request and response **in the clear**, including multi-frame group reads. Paired
with a VCDS ADVMB CSV log recorded at the same time, that yields the
`(read address → raw bytes → displayed engineering value)` triples the whole M3 effort needs.

This design covers the tooling for that capture, plus the active-side counterpart.

## Scope

| In | Out |
|---|---|
| slcan listen-only (silent) channel mode | CAN-FD (the car is classic CAN) |
| passive frame capture to `vag-capture` JSONL | writing to the car (any UDS service but `0x22`) |
| passive ISO-TP reassembly + live decode | offline analysis tooling (separate work) |
| `vagcan sniff` (silent default, `--active` opt-in) | measurement scaling inference itself |
| `vagcan scan` (read-only DID sweep) | GUI / TUI |

## Hardware facts established on the bench (2026-07-31)

Verified with `crates/vag-can/examples/slcan_probe.rs`, dongle on the desk, no bus:

- Enumerates as CDC-ACM (`/dev/cu.usbmodem206E37A148451`, VID `0x16d0`, PID `0x117e`) — so the
  firmware is **slcan**, not candleLight. No reflash needed.
- Firmware answers `V` (`16e7497-dirty github.com/normaldotcom/canable2.git`) and `E`
  (`CANable Error Register: <hex>`) repeatedly and stays responsive after any other command.
- The firmware **acknowledges nothing else** — no CR ack, no BEL. Its whole command set is
  `O C S Y M A V E t T r R d D b B X`; there is no `L`, no `N`, no `F`, and **no loopback**.
  `SlcanBackend::open_channel` is already fire-and-forget, which matches this exactly.
- Because there is no loopback command and CAN needs a second node to ACK a frame, **TX/RX
  cannot be proven on the bench**. First real proof happens on the car.
- Listen-only is `M1`, not `L`. The parser converts ASCII arguments to nibbles
  (`buf[i] = buf[i] - '0'`) *before* the `switch`, so the literal text `M1\r` sets
  `can_set_silent(1)`.
- Board jumpers: **120R must be OPEN** on the car (the vehicle bus is already terminated at
  both ends, ~60 Ω; a third 120 Ω resistor drags the bus to 40 Ω and corrupts signalling).
  **BOOT stays open** (DFU only).

## Components

### 1. `vag-can` — channel mode

```rust
pub enum SlcanMode { Normal, Silent }

impl<S: AsyncRead + AsyncWrite + Unpin + Send> SlcanBackend<S> {
    pub async fn open_channel(&mut self, bitrate: SlcanBitrate) -> Result<(), CanError>;
    pub async fn open_channel_mode(&mut self, bitrate: SlcanBitrate, mode: SlcanMode)
        -> Result<(), CanError>;
}
```

Wire sequences, exactly:

| mode | bytes |
|---|---|
| `Normal` | `C\rS6\rM0\rO\r` |
| `Silent` | `C\rS6\rM1\rO\r` |

`open_channel` keeps its current meaning (`Normal`) so existing callers are untouched.

**Invariant: `M` precedes `O`.** The STM32G431 FDCAN accepts mode configuration only while the
peripheral is in init state; after `O` the registers are locked and a later `M1` silently does
nothing. A unit test asserts the exact byte string written to an in-memory stream, so this
ordering cannot regress unnoticed.

`SlcanBackend::open_silent(path, baud, bitrate)` is added alongside `open` for the
serial-port constructor (feature `slcan`).

### 2. `vag-capture` — a time anchor and user markers

```rust
pub enum CapturePayload {
    CanFrame { id: CanId, data: Vec<u8> },
    CableBytes { bytes: Vec<u8> },
    Marker { note: String },        // new
}
```

Adding a variant is backward compatible: existing JSONL files still deserialize, and
`ReplayCan` ignores markers.

Markers carry two things:

1. **A wall-clock anchor**, written as the first record of every capture (RFC-3339 UTC plus the
   local offset). `ts_us` stays monotonic-from-start; the anchor makes it absolute.
2. **Operator notes**, injected live (see `sniff` below) — "engine started", "pulling away",
   "back to idle".

This exists specifically to kill the failure mode that wasted the previous two captures: the
capture↔CSV lag had to be guessed (≈52 s in `rod-labels.md` §4.0b), and several apparent
correlations turned out to be window-fishing at wrong lags. With an anchor, alignment is
arithmetic.

### 3. `vag-can` — passive ISO-TP reassembly

`IsoTpCan` is an active transport (send request, await response) and cannot observe a third
party's conversation. A separate passive reassembler is needed:

```rust
pub struct IsoTpSniffer { /* per-CAN-id in-flight state */ }

pub struct SnifferPdu {
    pub id: u32,
    pub data: Vec<u8>,
    pub frames: usize,      // 1 = single frame; >1 = reassembled
}

impl IsoTpSniffer {
    pub fn new() -> Self;
    /// Feed one observed CAN frame; returns a PDU when one completes.
    pub fn observe(&mut self, id: u32, data: &[u8]) -> Option<SnifferPdu>;
}
```

Behaviour:

- **SF** (`0x0N`) → emit immediately.
- **FF** (`0x1N`) → start per-id assembly with the declared length.
- **CF** (`0x2N`) → append, tracking the 4-bit sequence number; a gap **drops** the assembly for
  that id (a sniffer cannot request retransmission) and counts a `dropped` statistic.
- **FC** (`0x3N`) → ignored (flow control is the tester's business, not ours).
- A new FF on an id with an assembly in flight replaces it and counts a drop.
- Assemblies older than a timeout (default 2 s) are discarded.

Pure state machine, no I/O — fully unit-testable without hardware. This is the component that
turns VCDS's multi-frame group reads into readable PDUs.

### 4. `vagcan sniff`

```
vagcan sniff [--device <path>]
             [--out <file.jsonl>] [--diag-only]
             [--seconds <n>] [--active]
```

- **Silent is the default.** `--active` opts into `Normal` mode and is documented as "our node
  will ACK frames on the bus".
- Every observed frame is written as `CaptureRecord { ts_us, dir: Rx, CanFrame }`. `Rx` is from
  *our* point of view — we only ever receive, even when the frame is a request another tester
  transmitted. Direction on the bus is recovered from the CAN id, not from this field.
- `--diag-only` keeps standard ids `0x700..=0x7FF` and extended ids matching the UDS
  diagnostic patterns (`0x18DA_xxxx`, `0x17FC_00xx`, `0x17FE_00xx`); everything else is dropped
  **from the file as well as the display**. Default is unfiltered — a trip to the car is
  expensive, and broadcast frames are an independent crib (RPM and speed also live there).
- Live display, one line per PDU (reassembled), frames shown raw:

  ```
    12.345  7E0 ->  22 F1 90
    12.351  7E8 <-  62 F1 90 38 56 30 39 30 36 32 36 34 48        (13B, 3 frames)
  ```

  Direction arrows are derived from the id (`7E0..7E7` = tester→ECU, `7E8..7EF` = ECU→tester);
  unknown ids print without an arrow.
- Pressing Enter on stdin writes a `Marker` record; any text typed before Enter becomes the
  note.
- `--seconds` stops cleanly at a deadline; otherwise Ctrl-C does. Either way: flush the JSONL
  writer, send `C\r`, close the port.

**Timestamp accuracy, stated honestly:** this firmware has no timestamp command (`Z` is absent),
so `ts_us` is a host-side stamp taken when the line is parsed out of the USB read buffer. CDC
delivers frames in batches, so per-frame jitter is tens of milliseconds. That is fine for
aligning against a VCDS CSV (samples arrive every ~100 ms at best) and useless for bus-level
timing analysis. Do not read physics into sub-100 ms structure.

### 5. `vagcan scan`

```
vagcan scan [--device <path>] --ecu <01|02|714|…>
            [--range 7400-7500] [--out <file.jsonl>] [--delay-ms 5]
```

Issues UDS `ReadDataByIdentifier` (`0x22`) across the range and records every positive
response with its raw bytes. Negative responses `0x31` (requestOutOfRange) and `0x11`/`0x12`
are counted, not stored. The existing read-only allowlist already forbids anything but `0x22`,
so no new write path is introduced.

- A `TesterPresent` is sent every ~2 s to hold the session.
- Results stream to disk as they arrive, so an interrupted sweep keeps what it found.
- Default range = the bands the crib already showed to be live (`7400-7500`, `A000-A100`,
  `F100-F200`); a full `0000-FFFF` sweep is opt-in because at 5 ms/DID it is 5.5 minutes at
  best and realistically 15–30.

Value: VCDS only reads identifiers its label files name. A sweep enumerates what the ECU
actually exposes — an independent second crib source next to the sniffer.

## Testing

Everything above is unit-testable without a car:

| unit | test |
|---|---|
| `open_channel_mode` | exact bytes for both modes against an in-memory stream; `M` before `O` |
| `IsoTpSniffer` | SF; FF+CF reassembly; interleaved ids; sequence gap drops; FF-restart; stale timeout; FC ignored |
| `Marker` | JSONL round-trip; old capture files (no markers) still parse |
| `--diag-only` filter | id classification table |
| `scan` | scripted `MockAsyncTransport`: positive/negative mix, resumability, TesterPresent cadence |

Existing suite (198 tests) must stay green; `cargo clippy --all-targets -- -D warnings` clean.

## Hardware checkpoints — order of operations

Risk climbs monotonically; stop and confirm at each step.

1. **Open the 120R jumper.** Verify: CAN-H↔CAN-L on the dongle reads open; pins 6↔14 on the
   car's OBD-II socket read ~60 Ω with the ignition off.
2. **`sniff` alone, no VCDS.** Any broadcast traffic proves wiring, bitrate and RX. Zero risk:
   in silent mode the node cannot even ACK.
3. **`sniff` + VCDS in parallel**, with VCDS logging ADVMB to CSV. The trophy: cleartext
   diagnostic traffic including multi-frame group reads, with an aligned engineering-value log.
4. **`info`** — first transmission, closes the long-outstanding M1 live verification.
5. **`scan`** — the widest active read.

## Risks

- **Silent mode cannot be confirmed from outside.** A listen-only node is by definition
  invisible. Mitigation: the firmware source was read (not guessed) for the `M1` semantics, and
  a unit test locks the byte sequence. Residual risk accepted.
- **Bus load.** Sniffing adds none. `scan` adds a request every few ms on a diagnostic
  address — the same thing VCDS does continuously.
- **Capture size.** Unfiltered, ~50–100 MB per 5 minutes. Acceptable; `--diag-only` exists for
  when it is not.
- **Captures are personal data.** They contain the VIN. `research/dumps/` is gitignored and
  stays that way; no capture is ever committed.
