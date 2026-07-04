# vag-hex — Cable Transport Design

**Status:** design / pre-implementation. Blocked on the USB capture
(`research/vag-hex-capture-guide.md`). Fields marked **[FROM CAPTURE]** are placeholders
resolved from the trace before coding the affected layer.

**Goal:** let `vagcan` talk to the physical clone HEX cable (VAG25.3) directly — open it,
initialize it, and exchange UDS-over-ISO-TP with the car — with no VCDS and no loader in the
loop.

---

## 1. Where it fits

```
vag-cli / vag-core
      │  (commands: monitor, dtc, sniff)
      ▼
vag-protocol      UDS client + ISO-TP  (DONE, transport-agnostic)
      ▼  IsoTpTransport / RawCanTransport traits  (defined in vag-transport, DONE)
      ▼
vag-hex   ◄── THIS CRATE ── drives the cable's USB/serial protocol
      ▼
libftd2xx / serial   the physical cable
```

`vag-hex` implements the **existing** transport trait(s) from `vag-transport`, so the whole
protocol stack above it (already built and tested against the scripted mock + replay) works
unchanged the moment `vag-hex` produces real frames. No changes to `vag-protocol`.

**Key design constraint:** the transport seam already exists and is proven. `vag-hex` is a new
*implementation* of a known interface, not a redesign. This keeps the risk contained to the one
unknown — the cable's wire protocol.

---

## 2. What the cable actually is (to be confirmed by capture)

Working hypothesis (VAG25.3 clones are near-universally FTDI-based):

- USB → **FTDI** bridge (FT232-class). Two access paths:
  - **D2XX** (Ross-Tech's own path): open by FTDI device index/serial, raw bulk IN/OUT. The
    repo already vendors `libftd2xx` for darwin-arm64, matching this path.
  - **VCP** (virtual COM): OS serial port, `open("/dev/tty.usbserial-*")`.
- On top of the byte pipe, the cable speaks a **cable-specific serial envelope** carrying UDS
  payloads (length prefix + payload + checksum, exact form **[FROM CAPTURE]**).
- The car side is **UDS (ISO 14229) over ISO-TP (ISO 15765-2)**. `vag-protocol` already does
  ISO-TP + UDS; the open question is only what envelope the *cable* wraps around a UDS PDU and
  whether the cable does ISO-TP itself or expects raw UDS PDUs and segments internally.

**[FROM CAPTURE]** decides: D2XX vs VCP, exact VID/PID, and whether ISO-TP lives in the cable
or must be done host-side (we already have the host-side ISO-TP if needed).

---

## 3. Module layout

```
crates/vag-hex/
  Cargo.toml
  src/
    lib.rs        public API + HexCable struct, error type
    usb.rs        device open/close/read/write over the byte pipe (D2XX or serial backend)
    frame.rs      cable serial envelope: encode/decode (length, checksum, escaping) [FROM CAPTURE]
    init.rs       open-time handshake sequence (version query, baud/latency, "hello") [FROM CAPTURE]
    transport.rs  impl of vag-transport trait(s): map cable frames <-> ISO-TP/UDS PDUs
```

Each file has one responsibility; `frame.rs` and `init.rs` are the two carrying the reversed
protocol and are the ones fully specified only after the capture.

## 4. Public API (stable regardless of wire details)

```rust
pub struct HexCable { /* backend handle + config */ }

pub struct HexConfig {
    pub backend: Backend,          // D2XX { index or serial } | Serial { path }
    pub read_timeout: Duration,
    pub write_timeout: Duration,
}

pub enum Backend {
    D2xx { serial: Option<String> },   // None = first device
    Serial { path: String },
}

impl HexCable {
    /// Open the cable and run the init handshake. Fails if no cable / handshake mismatch.
    pub fn open(cfg: HexConfig) -> Result<Self, HexError>;

    /// Cable firmware/identity string recovered during init (for `vagcan doctor`).
    pub fn identity(&self) -> &CableIdentity;
}

// The bridge to the existing stack: vag-hex implements the transport trait that
// vag-protocol's ISO-TP/UDS client already consumes.
impl vag_transport::IsoTpTransport for HexCable { /* ... */ }

pub enum HexError {
    NotFound,                 // no cable enumerated
    Handshake(String),        // init exchange didn't match expected
    Io(std::io::Error),
    Timeout,
    Framing(String),          // bad length/checksum from cable
}
```

Consumes: the transport trait(s) from `vag-transport`. Produces: a `HexCable` the CLI opens and
hands to the existing `vag-protocol` UDS client.

## 5. Safety / scope

- **Read-only preserved end-to-end.** `vag-protocol`'s UDS client already enforces the
  read-only service allowlist `{0x10, 0x19, 0x22, 0x3E}` (returns `Forbidden` otherwise).
  `vag-hex` is a dumb pipe under that gate — it adds no write services. Live car reads only.
- No VCDS, no host-binary patching, no cable-auth defeat. This talks to owned hardware over its
  own protocol.

## 6. Implementation order (once capture is in hand)

TDD, each task independently testable against **captured byte fixtures** (record-once, like the
existing `ReplayCan`):

1. **`usb.rs` backend** — open/read/write the byte pipe; pick D2XX vs serial per capture.
   Test: open the real cable OR a loopback fixture; assert bytes round-trip.
2. **`frame.rs`** — encode/decode the cable envelope. Test: feed captured OUT bytes → decode to
   the known UDS PDU; encode the PDU → assert equals captured bytes (both directions, from real
   pairs in the trace).
3. **`init.rs`** — replay the handshake; assert the cable's real responses (from capture) drive
   it to "ready" and yield the identity string.
4. **`transport.rs`** — wire `frame` + `init` into the `vag-transport` trait; run the existing
   `vag-protocol` UDS client through it against a replay of the captured session (VIN read,
   measuring poll, DTC read) and assert decoded results.
5. **Live smoke** (manual, on the car): `vagcan doctor` opens the cable, prints identity; read
   VIN; poll RPM. Behind a feature/opt-in so CI stays hardware-free.

Every automated test runs off captured fixtures — no car required in CI, same discipline as the
existing replay tests.

## 7. Open questions (all resolved by the capture)

- D2XX or VCP? VID/PID? → §2, enumeration.
- Cable envelope: length width, checksum algorithm, escaping? → `frame.rs`.
- Does the cable do ISO-TP, or do we segment host-side (we can)? → `transport.rs`.
- Init handshake exact bytes + expected replies? → `init.rs`.
- Does the cable expose CAN sniff/promiscuous mode (for the future `sniff` command)? → note if
  visible in the trace; not required for P1.
