# research-keystream / 01 — reverse the 16-key link-cipher schedule

**Subsystem:** research-keystream · **Dir:** `research/` · **Wave:** 1 · **Depends:** none

**SCOPE BOUNDARY (mandatory):** this is the `b8`/`b7` **link/transport** cipher
(diagnostic-data obfuscation) — interop, in scope. It is NOT the `0xb6`-init
anti-clone AUTH challenge, which stays out of scope / untouched. See
`research/SCOPE-BOUNDARY.md`. If analysis drifts toward the auth challenge, stop.

## Goal
Recover the full keystream **schedule** so any channel's 16-byte keystream can be
derived programmatically, instead of the current per-channel empirical cribs.

## Context (what's already known)
- The link cipher is a **static position-dependent XOR keystream**: `plain[i] =
  cipher[i] ^ KS_channel[i]`, per logical channel (see the "Link cipher" section of
  `research/vag-hex-framing.md` and `research/clb-crack/link_cipher.py`).
- Selector in the binary: `(seq+1) & 0xF` (`0x140073150`) → dispatcher `0x140073160`
  → key-setup `0x14007b108` (memcpy 16-byte key) → XOR driver `0x14007afd0`. Raw
  16×16 table at `0x140171d30` was extracted but does NOT map to the effective
  keystreams under simple transforms tried so far.
- Tooling: `research/clb-crack/` venv (capstone+pefile), `framing_dis.py` (AArch64
  disasm), `link_cipher.py` (empirical per-channel keystreams + proof), the two
  captures (`init-only.pcapng`, `reading-ecus.pcapng`) as oracle.

## Deliverables
- Reverse `0x14007b108` (key setup) + `0x14007afd0` (XOR driver) + how the 16×16
  table at `0x140171d30` feeds them, to produce a function
  `keystream(channel_id, seq) -> [u8; 16]` that MATCHES the empirically-recovered
  keystreams for every channel present in `reading-ecus.pcapng`.
- Update `research/clb-crack/link_cipher.py` to derive keystreams from the schedule
  (replace/augment the per-channel cribs), and re-run its proof (`__main__`) — it
  must still decode TesterPresent / RDBI / the SW-version "1003".
- Document the schedule in `research/vag-hex-framing.md` (Link cipher section):
  algorithm, table role, channel-id derivation, confidence.

## Done criteria
- `keystream()` reproduces ALL empirically-known channel keystreams (cross-checked
  against `reading-ecus.pcapng`), not just the primary channel.
- If the schedule cannot be fully reversed, deliver the best partial (e.g. schedule
  for the channels we have) + a precise statement of what's missing and the next
  step. Honesty over overclaim.

## Interfaces (Produces)
- `keystream(channel_id, seq) -> [u8;16]` (Python now; will be ported to Rust in the
  `link-transport` wave-2 task).
