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
| **info-identity** | vag-protocol/vagcan | **`EcuIdentity` + `read_identity` (UDS RDBI F190/F187/F191/F189/F197/F18C/0600, per-DID tolerant, read-only) + `vagcan info --port` (Engine 01 + Gearbox 02 over one slcan channel); mock-tested against golden Auto-Scan values** |
| cli-app | vagcan | `vagcan` bin: `doctor` / `decode` / `probe` / `handshake` / `replay-drive` / `info` |

### `vagcan info` — MVP scope & status
MVP = **VIN + Engine (01) + Gearbox (02) identification** (part no / HW / SW / component /
serial / coding). **Identification logic is DONE** (`info-identity` above), transport-generic,
mock-tested — it just needs the CANable to run live (checkpoint below). Remaining for the
full goal: (a) the live hardware run, (b) live **measurements** (RPM / speed / turbo boost),
which need the `.rod` (ODX) DID + scaling path — see "Measurements (path B)" below.

## HEX-clone: blocked offline — two LIVE probes staged

The clone speaks the OLD scheme (not the new build's RSA-OAEP). Each diagnostic ECU
needs a per-`b6` AES epoch key `K_epoch` that VCDS computes **app-side** — and that
app is **VMProtect-packed**. Every *offline* route to `K_epoch`/the KDF is exhausted:
static RE (sealed), replay-shortcut (cable keeps advancing state), memory-dump (custom AES
never leaves 240 contiguous cleartext round-key bytes), and the crack DLL (`vcds_hook.dll` =
pure FTD2XX proxy + license detour, no crypto). Full writeups:
`research/{clone-crypto, vcds-rus-crack, vag-hex-framing}.md`.

Two live experiments the owner can now run (each definitively resolves its hypothesis):

- **Probe 1 — full ordered replay (`vagcan replay-drive`, no new hardware).** Replays the
  *entire* recorded OUT-frame sequence verbatim from a **cold cable power-on** (all prior
  attempts were partial-sequence shortcuts that desynced early). If the cable's crypto state
  is deterministic-from-power-on, the replay tracks it to the engine `f3` channel (idx 1045,
  epoch #15) where the recovered `KS_F3` applies → inject a UDS `22 F1 90` and read the VIN.
  If the state is CSPRNG-fresh, it emits the **exact divergence index** (expected vs observed)
  — a clean empirical verdict. Run: `cargo run -p vagcan -- replay-drive --stream
  research/dumps/replay-stream.jsonl` (fresh power-on first).

- **Probe 2 — VMProtect dynamic on a real x86 host** (`research/clone-crypto.md` §4.2).
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
1. `generic-can` hardware bring-up: exercise `SlcanBackend` against the real CANable dongle
   on macOS (open, bitrate, frame TX/RX), fix anything the mock hid.
2. ~~`vin-info` identification~~ — **DONE** (`info-identity`): VIN + Engine/Gearbox passport,
   mock-tested. Live run pending the dongle; confirm the F187-spaces / DQ200-session /
   coding-DID `0600` caveats against a real read.

### Measurements (RPM / speed / turbo boost)
Path B (`.rod` ODX) is **PARTIAL — the crypto wall is gone** (full writeup:
`research/rod-labels.md`). The earlier NO-GO is **superseded**: the per-record `product`/IV
blocker is now **DEFEATED OFFLINE, no runtime dump** — a DEFLATE dynamic-Huffman-header oracle +
Kraft pruning + full-inflate confirmation recovers the 5 IV bytes (multithreaded Rust cracker
`research/clb-crack/rod_crack/`, ~1 min). `STRUC.rod` (the DID+scaling structure file, once
believed absent) is now present in the owner's install (gitignored symlink under
`research/vcds-data/`) and **fully inflates** (293,560 bytes, valid Adler-32); the same method
unblocks `TTTEXT.ROD` and every engine `MWB`/`INC`/`DTC`.

**Remaining is pure data-format RE — no crypto, no dump on the critical path.** The decoded
`STRUC` payload is a **packed 14-glyph codec** (`NNNNNN,<encoded>`, 1221 structure-ids), not
delimited `DID,factor,offset`. To finish (all offline):
1. **DID + scaling** — reverse the `STRUC`-parser fn (binary `+0x33758` region) to turn the
   14-glyph field codec into `read_id`/`raw`/`scale`/`unit`, and decode the engine-`MWB`
   2-char-code → `STRUC`-id mapping.
2. **Names** — crack `TTTEXT.ROD [TXT]` (same offline method as `STRUC`) for the text-id → name
   join; decrypt-only, mechanical (a nicety). *(An earlier existing-dump spike also harvested
   11,479 already-resolved RU names from the heap, but with no `id→name` linkage — see
   `research/rod-labels.md` Appendix B; no longer on the critical path.)*

Fallback still on the table if the heavy path stalls: **OBD-II Mode 01** (PID `0x0C` RPM,
`0x0D` speed, `0x0B` MAP − `0x33` baro = boost) — fixed SAE J1979 formulas, no labels, gets
exactly the three values quickly over service `0x01`.

**Validation oracle:** the owner's own full Auto-Scan is captured in
`research/vcds-rus-crack.md` (VIN `XW8AD4NE9JH008917`, Škoda NE-SK37, every ECU
part-number/coding/VCID) — golden fixtures for `vagcan info` regardless of transport.

## Hardware checkpoints (STOP, confirm on the real car)

Dongle chosen: **MKS CANable V2.0 Pro** (STM32G431 + ADM3050E isolated) — an exact fit for
`vag-can`'s `SlcanBackend`, no new backend needed. Before first use: (a) ensure **slcan**
firmware, not candleLight (candleLight = gs_usb = Linux-only, no serial port on macOS; reflash
via BOOT jumper + DFU if needed) → it enumerates as `/dev/cu.usbmodem*`; (b) wire OBD2 pin
6→H, 14→L, 4/5→G (power from USB-C, do NOT wire pin 16); (c) **TERM jumper OFF** (the car bus
is already terminated). Open with `SlcanBackend::open("/dev/cu.usbmodem*", baud, Rate500k)`.

1. slcan dongle: raw CAN frame TX/RX with the car (500 kbit/s).
2. `vagcan info --port <tty>` prints the real VIN (`XW8AD4NE9JH008917`) + Engine/Gearbox
   identity (part no / HW / SW / component / coding). Confirm the F187-spaces, DQ200-session,
   and coding-DID caveats here.

## Parked (future, designed but not being implemented now)
- **Cross-platform runtime (`no_std` core + `vag-runtime-*`)** — spec
  `docs/superpowers/specs/2026-07-06-cross-platform-runtime-design.md`, M1 plan
  `docs/superpowers/plans/2026-07-06-cross-platform-runtime-m1.md`. Unblocks desktop
  tri-platform + ESP32-S3 (USB-host to CANable). Below-the-seam refactor; does not block MVP.
