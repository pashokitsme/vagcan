# link-decode / 01 — port the link-cipher DECODE to Rust (PoC #2 enabler)

**Subsystem:** link-decode · **Crate:** `vag-hex` · **Wave:** 3 · **Depends:** frame (done), research-keystream (done)

## Goal
Port the DECODE side of the link cipher to Rust so `vagcan decode <capture>` turns a real captured
`b8`/`b7` session into UDS PDUs. This is the second PoC: our tool decodes the user's real car data.

**Scope / boundary:** DECODE ONLY, using a per-session keystream recovered from known-plaintext
(exactly what `research/clb-crack/link_cipher.py` does). We are NOT synthesizing the AES session key
(that's auth-derived, out of scope — see `research/SCOPE-BOUNDARY.md` and the session-key verdict in
`research/vag-hex-framing.md`). So the decoder takes a recovered keystream (or recovers it from the
capture's known-plaintext) — it does not and must not derive the key.

## Context
- `research/clb-crack/link_cipher.py` is the reference: the 16-byte block layout (off6 = ISO-TP PCI,
  off7 = UDS SID, off8..13 data, etc.), per-channel keystream, `plain[i] = cipher[i] ^ KS[i]`.
- `vag_hex::frame` decodes the outer `S/M` frames; `0xb8`/`0xb7` payloads carry the 16-byte block.

## Deliverables (module in `vag-hex`, e.g. `link.rs`)
- A function to recover a channel's keystream from known-plaintext pairs (port the crib logic), and
  a `decrypt_block(cipher: &[u8;16], keystream: &[u8;16]) -> [u8;16]` (XOR).
- A `decode_diag_frame(frame_payload: &[u8], keystream) -> Option<UdsSlice>` that yields the inner
  UDS bytes (off7.. per the block layout), handling the ISO-TP PCI at off6 (single-frame at least;
  multiframe reassembly if feasible).
- Enough to reproduce, in Rust, the Python proof on `reading-ecus`: TesterPresent `3E 00`, RDBI
  `22 74 58`, SW-version "1003". A test asserts these decode correctly from committed byte fixtures
  (small hex vectors lifted from the capture — do NOT commit the pcapng; embed a handful of b8/b7
  block hex strings as test constants).

## TDD
- Unit-test `decrypt_block` + keystream recovery against the known vectors from
  `research/vag-hex-framing.md` "Link cipher" (e.g. f3 channel KS[6..13] = 02 A9 99 F6 DA 7C 9C 3A,
  block `f3 83 44 dd 7c 5f 00 97 99 f6 da 7c 9c 3a 00 fc` → `02 3E 00 …` = TesterPresent).
- Assert the SW-version channel block decodes to bytes containing `10 03`.

## Done criteria
- `cargo test -p vag-hex` green; clippy `-D warnings` clean; `cargo build --workspace` clean.
- Commit in worktree, mandatory trailers.

## Interfaces (Produces)
- `vag_hex::link::{recover_keystream, decrypt_block, decode_diag_frame}` — consumed by
  `vagcan decode` (cli-app) to render UDS from a captured session.
