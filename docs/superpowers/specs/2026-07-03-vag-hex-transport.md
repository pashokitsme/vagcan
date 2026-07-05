# vag-hex — Cable Transport Design

**Status:** capture done, framing implemented. Two USBPcap traces
(`init-only`, `reading-ecus`) resolved the wire format — see
`research/vag-hex-framing.md` (ground truth) and `research/SCOPE-BOUNDARY.md`
(interop line). `frame.rs` now carries the real flat `S/M` frame (tested).
Remaining unknowns are the FTDI backend wiring, the init handshake replay, and
the encrypted diagnostic transport (per-channel link keystream). The old
**[FROM CAPTURE]** placeholders below are annotated **[RESOLVED]** where the
trace settled them.

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

## 2. What the cable actually is — [RESOLVED] by capture

Confirmed from the traces (`research/vag-hex-framing.md`):

- USB → **FTDI** bridge, accessed via **D2XX bulk** (OUT endpoint `0x02`,
  IN endpoint `0x81`). VCP is not used on the wire. The repo vendors the FTDI
  D2XX driver in `driver/` (darwin-arm64 dylib + win-arm64 `FTD2XX.dll`/
  `FTDIBUS.sys`) — that is the byte pipe for `usb.rs`.
- On top of the byte pipe, the cable speaks a **flat frame**:
  `[marker][len][opcode][data..][xor]`, marker `0x53 'S'` host→cable / `0x4D 'M'`
  cable→host, `len` = total length, `xor` = XOR of all preceding bytes. One
  frame, not the nested layers the pre-capture static guess assumed. Implemented
  in `frame.rs` (`frame_encode`/`frame_decode`/`take_frame`), confirmed on 3407
  frames.
- The car side is **UDS (ISO 14229) over ISO-TP (ISO 15765-2)**, and ISO-TP
  framing lives **inside** the cable's diagnostic block (the recovered inner
  layout has the ISO-TP PCI at block offset 6, UDS SID at 7). So we build
  ISO-TP+UDS host-side (`vag-protocol` already does), encipher it, and wrap it in
  a diagnostic frame.
- **Catch — the diagnostic channel is encrypted.** UDS rides inside opcode
  `0xb8` (request) / `0xb7` (response) frames as a 16-byte block XOR-enciphered
  with a per-channel keystream. The cipher is recovered in research
  (`research/clb-crack/link_cipher.py`, a position-dependent XOR keystream, same
  family as `.clb`) but its 16-key schedule is not yet reversed. Need reverse

---

## 3. Module layout

```
crates/vag-hex/
  Cargo.toml
  src/
    lib.rs        public API + HexCable struct, error type
    usb.rs        device open/close/read/write over the byte pipe (D2XX bulk) [PENDING]
    frame.rs      flat S/M frame: encode/decode + stream cutter [DONE, tested]
    init.rs       open-time handshake sequence (02/09/04 identify, b0..b5 setup) [PENDING]
    transport.rs  impl of vag-transport trait(s): map cable frames <-> ISO-TP/UDS PDUs
```

Each file has one responsibility. `frame.rs` is done (capture-confirmed). `usb.rs`,
`init.rs`, and the encrypted diagnostic path in `transport.rs`/`frame.rs` remain.

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

## 7. Open questions — capture answers

- D2XX or VCP? → **D2XX bulk** (OUT 0x02 / IN 0x81). [RESOLVED]
- Cable envelope: length width, checksum, escaping? → flat `[marker][len][opcode]
  [data][xor]`, 1-byte len, XOR checksum, no escaping. [RESOLVED, `frame.rs`]
- Does the cable do ISO-TP, or do we segment host-side? → ISO-TP PCI lives inside
  the diagnostic block; we build ISO-TP+UDS host-side and wrap it. [RESOLVED]
- Init handshake exact bytes + replies? → open sequence `02` probe / `09` keyed /
  `04` identify ("ROSSTECH") / `82` / `0d` / `b0..b5` setup burst (`fe`-acked);
  exact `init.rs` replay still to code. [PARTIAL]
- **New, unresolved:** the diagnostic UDS transport is **encrypted** (opcode
  0xb8/0xb7, per-channel XOR keystream). Cipher recovered in research; the 16-key
  schedule and the Rust port remain. This is the main blocker to end-to-end reads.
- CAN sniff/promiscuous mode for a future `sniff` command? → not investigated;
  not required for P1.
