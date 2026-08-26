# live-drive / 01 — HEX-clone talks live on macOS + link encode + auth-advance

**Subsystem:** live-drive · **Crate:** vag-hex, vagcan · **Done:** 2026-07-06

## What shipped
Made the physical HEX-clone cable actually communicate from `vagcan` on macOS, and
built the encode side of the link cipher + a dynamic session driver.

- **Cable opens & talks (macOS D2XX):**
  - `FT_SetVIDPID(0x0403, 0xFA24)` before enumerate/open — the clone's custom PID is
    absent from libftd2xx's built-in table, which had caused `FT_Open` → DEVICE_NOT_FOUND.
  - FTDI init matched to the captured working session: 8N1, baud dance 9600→19200→115200,
    DTR/RTS low, 1 ms latency.
  - **Clean-close:** `D2xxBackend` holds the worker `JoinHandle` and joins on drop so
    `FT_Close` always runs — fixes the cable dropping off the USB bus between runs.
  - Confirmed live: bring-up replies (`02/09/04/82/0d`), `b0..b5` setup, `b6`; with
    ignition the cable streams `b7`/`b9`.

- **Link encode (was decode-only):** `encode_f3_request` / `encode_request`,
  `f3_trailer(off14)` (off15 = `KS15 ^ H(off14&0xE0)`, validated 763/763 req + 457/457
  resp), off14 = plaintext counter. Round-trips the captured TesterPresent and RDBI
  blocks byte-for-byte; `decode(encode(x))==x`. `IsoTpReassembler` for multiframe.

- **Live drive commands (`vagcan`):**
  - `probe` — replay bring-up, report pushed frames; flags an RSA-OAEP wrapped-key frame
    (→ proved this cable is the OLD scheme, not RSA-OAEP).
  - `handshake` — sweep the `0x39` auth-completion off14 (rule: `observed & 0xF8`) until
    the cable advances past auth; then f3 TesterPresent → VIN tail. `--deep` replays the
    2nd `b6` (proven safe, no wedge, but does not re-key — see blocker).

## Blocker discovered (why this stops short of a live UDS read)
Each diagnostic ECU needs its own per-`b6` AES epoch key `K_epoch`, computed app-side by
the **VMProtect-packed** VCDS. All offline routes to it are exhausted (static / replay /
memory-dump / the `vcds_hook.dll` crack). The clone cannot be driven end-to-end without
defeating VMProtect. → product pivots to the generic USB-CAN bypass (`vag-can`); clone
crack deferred (Track B). Full analysis: `research/clone-crypto.md`,
`archive/research/vcds-rus-crack.md`.

## Tests / quality
vag-hex + vagcan unit tests green, `cargo clippy --all-targets -- -D warnings` clean.
Commits: eab6a29, 832d25a, 91f26ee, ce76621, + follow-ups through 6217691.
