# init-handshake / 01 — plaintext open handshake (live PoC #1)

**Subsystem:** init-handshake · **Crate:** `vag-hex` · **Wave:** 3 · **Depends:** cable-actor, usb-backend (done)

## Goal
Drive the cable's PLAINTEXT open handshake over the connection actor and return a
`CableIdentity` (firmware/version, "ROSSTECH"). This is the live hardware PoC: our tool
opens the real HEX cable and reads its identity. **No cipher, no auth — plaintext only.**

## Context (from `research/vag-hex-framing.md` §1, §4)
Plaintext open sequence, all over the flat `S/M` frame:
- `0x02` probe (OUT `02` → IN `02 016044…`),
- `0x04` identify (OUT `04` → IN `04 "ROSSTECH" 000000 <ver bytes> …`) — the identity/version,
- `0x82` status (→ `82 0000`), `0x0d` status (→ `0d 02`),
- (the `b0..b5` setup burst + `0xb6` auth come AFTER and are OUT OF SCOPE — do NOT drive auth;
  the PoC stops at plaintext identify. Driving the encrypted diagnostic session is not part of
  this task and is blocked by the auth-derived key — see `research/SCOPE-BOUNDARY.md`.)

## Deliverables
- `async fn handshake<B: Backend>(cable: &CableHandle) -> Result<CableIdentity, HexError>` (or on
  a `HexCable`/actor wrapper — match whatever cable-actor produced): sends `02` then `04` via
  `CableHandle::request`, parses the identify reply into `CableIdentity { firmware: Option<String>,
  raw: Vec<u8> }` (extract the ASCII "ROSSTECH" + version bytes), optionally `82`/`0d`. Returns the
  identity. Do NOT proceed into `b0..b6`.
- Wire it so a `vagcan doctor` (cli-app task) can call: open D2XX backend → spawn actor → handshake
  → print identity.

## TDD (no hardware)
- Use a mock `Backend`/`CableHandle` scripted with the real captured identify bytes (from
  `research/vag-hex-framing.md`: IN `04 52 4f 53 53 54 45 43 48 00 00 00 a8 9d 01 00 09`). Assert
  `handshake` returns firmware containing "ROSSTECH" + the version bytes parsed.
- Edge: a wrong/short reply → `HexError::Handshake(..)`.

## Done criteria
- `cargo test -p vag-hex` green; clippy `-D warnings` clean (default + no-default-features);
  `cargo build --workspace` clean. Commit in worktree, mandatory trailers.

## Hardware checkpoint (after merge)
On the M4 with the cable plugged: `vagcan doctor` must open the cable and print the real identity
("ROSSTECH" + firmware/version). This is the visible PoC — I stop and ask the user to run it.
