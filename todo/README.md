# vagcan — roadmap & status (`vagcan info`)

Goal: `vagcan info` prints VIN, vehicle name/model, and equipment (engine, turbo?,
gearbox kind+name, basic info) read live from the car. See `/CLAUDE.md` for the
locked stack/architecture and `todo/GOAL.md` for the goal.

Task files live in `todo/<subsystem>/NN-<task>.md`; done ones move to
`done/<subsystem>/NN-<task>.md`.

## Status (2026-07-06)

The whole software stack is built and the **HEX-clone cable talks live on macOS**.
The one thing blocking a live read over the clone is its **encrypted link session
key**, which is sealed inside a VMProtect-packed app (see "HEX-clone: blocked" below).
Everything else works and is transport-agnostic.

### Done (merged to `master`, tests green, clippy clean)
| subsystem | crate | what |
|-----------|-------|------|
| async-core | vag-transport | async transport trait(s) + mock, error model |
| usb-backend | vag-hex | `Backend` + `D2xxBackend`; **`FT_SetVIDPID` + FTDI init + clean-close** → cable opens & talks live on macOS |
| cable-actor | vag-hex | `CableActor<B>` + `CableHandle` multiplex |
| init-handshake | vag-hex | plaintext open handshake (`doctor` → "ROSSTECH") |
| link-decode | vag-hex | b8/b7 XOR decode + keystream recovery + ISO-TP |
| **link-encode** | vag-hex | **b8 encode + off14 counter + off15 trailer rule; round-trips captured TP/RDBI** |
| **live-drive** | vag-hex | **`probe` (protocol id) + `handshake` (auth-advance past 0x39, off14=observed&0xF8) + sweep + dynamic session driver** |
| **replay-drive** | vag-hex/vagcan | **full ordered session-replay from cold power-on → f3 channel → own UDS read (VIN); exact divergence report if cable state desyncs. Extractor + `--dry-run` + 8 tests** |
| uds-async | vag-protocol | async ISO-TP + UDS client |
| label-lookup | vag-db/vag-data | fast part-number/coding → component lookup |
| generic-can | vag-can | `SlcanBackend` + `IsoTpCan` (the bypass transport — built, untested on hw) |
| research-keystream / session-key | research | link cipher = AES-256 keystream; RSA-OAEP (new build) fully reversed |
| cli-app | vagcan | `vagcan` bin: `doctor` / `decode` / `probe` / `handshake` |

### Not done → `vagcan info` itself
`vin-info` (VIN read + decode + equipment assembly) and wiring `info` — both gated on
a working transport that delivers plaintext UDS. See the two tracks below.

## HEX-clone: blocked offline — two LIVE probes staged

The clone speaks the OLD scheme (not the new build's RSA-OAEP). Each diagnostic ECU
needs a per-`b6` AES epoch key `K_epoch` that VCDS computes **app-side** — and that
app is **VMProtect-packed**. Every *offline* route to `K_epoch`/the KDF is exhausted:
static RE (sealed), replay-shortcut (cable keeps advancing state), memory-dump (custom AES
never leaves 240 contiguous cleartext round-key bytes), and the crack DLL (`vcds_hook.dll` =
pure FTD2XX proxy + license detour, no crypto). Full writeups:
`research/{RE-PLAN-old-scheme-rekey, DYNAMIC-attack-RESULTS, vcds-rus-crack-findings,
vag-hex-framing, auth-mechanism-notes}.md`.

Two live experiments the owner can now run (each definitively resolves its hypothesis):

- **Probe 1 — full ordered replay (`vagcan replay-drive`, no new hardware).** Replays the
  *entire* recorded OUT-frame sequence verbatim from a **cold cable power-on** (all prior
  attempts were partial-sequence shortcuts that desynced early). If the cable's crypto state
  is deterministic-from-power-on, the replay tracks it to the engine `f3` channel (idx 1045,
  epoch #15) where the recovered `KS_F3` applies → inject a UDS `22 F1 90` and read the VIN.
  If the state is CSPRNG-fresh, it emits the **exact divergence index** (expected vs observed)
  — a clean empirical verdict. Run: `cargo run -p vagcan -- replay-drive --stream
  research/dumps/replay-stream.jsonl` (fresh power-on first).

- **Probe 2 — VMProtect dynamic on a real x86 host** (`research/PATH2-vmprotect-dynamic-x86.md`).
  NOT the owner's ARM x86-emulation VM (VMProtect detects both hypervisor and emulation) — a
  real x86 machine. *Tier 0:* unpack a clean x86 image (Scylla). *Tier A (cheap, first):* HW
  read-breakpoint on `IV_TABLE[4]` (`0x538510`) + CNG `BCryptGenerateSymmetricKey`/`BCryptEncrypt`
  hooks to catch the native AES / raw `K` (~55–65%; a miss is itself the diagnosis that the
  crypto is virtualized). *Tier B (heavy):* Pin+Triton-devirt the KDF (hackyboiz method) and
  reimplement it in Rust. Every tier validates via `research/clb-crack/validate_k.py` →
  VIN `XW8AD4NE9JH008917`. Helper scripts: `research/clb-crack/hwbp_ivtable.{x64dbg,windbg}.txt`,
  `kdf_trace.pin.cpp`, `kdf_triton.py`.

If both probes fail, the clone link stays sealed and the product goal rides Track A below.

## Track A (recommended — the extensible product path): generic USB-CAN

Bypass the clone's encrypted link entirely. The `vag-can` crate (`SlcanBackend` +
`IsoTpCan`) already implements the same `vag_transport::AsyncIsoTpTransport` the whole
stack rides — talk UDS-over-ISO-TP-over-CAN straight to the car via a cheap slcan
USB-CAN dongle. Any ECU/DID, own logic, repeatable, no clone crypto.

Remaining tasks for `vagcan info` over Track A:
1. `generic-can` hardware bring-up: exercise `SlcanBackend` against a real slcan dongle
   on macOS (open, bitrate, frame TX/RX), fix anything the mock hid.
2. `vin-info`: read VIN `22 F1 90` from the gateway/engine, decode; assemble equipment
   from part-number/coding lookups (label-lookup is ready).
3. `cli-app`: wire `vagcan info` (JoinSet over ECUs), print VIN + car + equipment.

**Validation oracle:** the owner's own full Auto-Scan is captured in
`research/vcds-rus-crack-findings.md` (VIN `XW8AD4NE9JH008917`, Škoda NE-SK37, every ECU
part-number/coding/VCID) — golden fixtures for `vagcan info` regardless of transport.

## Hardware checkpoints (STOP, confirm on the real car)

Dongle chosen: **MKS CANable V2.0 Pro** (STM32G431 + ADM3050E isolated) — an exact fit for
`vag-can`'s `SlcanBackend`, no new backend needed. Before first use: (a) ensure **slcan**
firmware, not candleLight (candleLight = gs_usb = Linux-only, no serial port on macOS; reflash
via BOOT jumper + DFU if needed) → it enumerates as `/dev/cu.usbmodem*`; (b) wire OBD2 pin
6→H, 14→L, 4/5→G (power from USB-C, do NOT wire pin 16); (c) **TERM jumper OFF** (the car bus
is already terminated). Open with `SlcanBackend::open("/dev/cu.usbmodem*", baud, Rate500k)`.

1. slcan dongle: raw CAN frame TX/RX with the car (500 kbit/s).
2. VIN read → `vagcan info` prints the real VIN (expect `XW8AD4NE9JH008917`).
