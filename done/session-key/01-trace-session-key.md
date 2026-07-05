# session-key / 01 — classify the AES session-key derivation (A, boundary-sensitive)

**Subsystem:** session-key · **Dir:** `research/` · **Wave:** 2 · **Depends:** research-keystream (done)

## READ THIS FIRST — the boundary decides the whole task
`research/SCOPE-BOUNDARY.md`. The b8/b7 link cipher is **AES-256**; the per-channel keystream is
`AES_encrypt(IV_row)` under a **32-byte runtime session key** set via `0x140072ec0` → `0x14007ce68`
during session setup, **adjacent to the `0xb6`** (see `research/vag-hex-framing.md`
"Link cipher"). This task is a **CLASSIFICATION with a hard stop**, not a key extractor.

The question to answer: **is the session-key derivation (a) self-contained app-side key setup that
our own tool can legitimately replicate for interop, or (b) derived from?**

- Only if the evidence clearly shows **(a)** — a key setup independent of proving genuineness,
  something the app itself computes from non-secret/negotiated inputs we're entitled to as the app
  — may you document the derivation (inputs → 32-byte key) enough to compute it. Even then: keep to
  the DATA-channel key; do not touch the challenge-response itself.

When unsure between (a) and (b), treat it as (b) and stop. Err toward stopping.

## Method (classification only)
- Environment: `research/clb-crack`, `.venv/bin/python`, `framing_dis.py` (AArch64 disasm),
  captures as oracle. Target the set-key path `0x140072ec0` → `0x14007ce68` and what feeds the
  32-byte key argument: where do those bytes come from?
- Trace ONLY far enough to classify the SOURCE of the key bytes:
  - constant/static in the binary, or derived from device serial / a fixed app secret, or from the
    `0xb6` challenge exchange, or from a negotiated/plaintext handshake value?
- The moment the trail enters the `0xb6` challenge/response logic or any genuineness check, STOP
  (that's (b)).

## Deliverable
- Append a short subsection to `research/vag-hex-framing.md`: the classification verdict **(a)** or
  **(b)** with the decisive evidence (addresses, what feeds the key), and a one-line recommendation
  (proceed to compute the key = viable live path on this cable / stop = use the generic-CAN
  fallback). NO key material, NO challenge-response reconstruction in any case.
- Commit to master (you run in the MAIN working directory — you need the venv + captures).
  Conventional Commit, mandatory `Assisted-By:` + `Claude-Session:` trailers.

## Report back
The verdict (a/b), the decisive evidence in 3-4 lines, and the recommendation. If you stopped at the
boundary, say exactly where the trail crossed into auth. Honesty and restraint over completeness.
