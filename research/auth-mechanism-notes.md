Everything below is either passive observation of the two USB captures
(`init-only.pcapng`, `reading-ecus.pcapng`, via `research/clb-crack/usbpcap.py`)
or static loci **noted but not analyzed**. Where a static address is given, it
marks *where the mechanism lives*, not how it works.

---

## 1. What it is / what it does

A cryptographic **challenge-response** run during session setup whose function is
to prove to the VCDS app that the cable is a **genuine Ross-Tech interface** (and
for the app to reject non-genuine ones). It is an authentication / anti-counterfeit
measure — distinct from the data-channel obfuscation cipher, which is interop.

It has a second consequence that matters to us: **the AES-256 session key that
encrypts the `b8`/`b7` diagnostic channel is a product of this exchange.**

---

## 2. Observable shape on the wire (HIGH — from captures)

Position in the open sequence (`init-only.pcapng`, plaintext-observable):

```
02 probe → 04 identify ("ROSSTECH"+ver) → 82 → 0d          (plaintext bring-up)
→ b0 b1 b2 b3(x2) b4(x6) b5(x2)   each fe-acked            (setup burst)
→ frame#36  OUT 0xb6  payload ~24–27 bytes                 (CHALLENGE issued)
→ frame#37  IN  0xfe  ack
→ frame#38-39 IN 0xb7 payload 16 bytes ×2                  (RESPONSE(s))
→ frame#40  IN  0xb9  status/ack (2 bytes)
→ frame#41+ OUT b8 / IN b7 (16-byte units), b9/ff status   (session proceeds, ENCRYPTED)
```

Observed properties (positional/size only — no transform recovered):

| opcode | dir | shape | note |
|--------|-----|-------|------|
| `0xb6` | OUT | ~24–27 random-looking bytes | the challenge; entropy ~4.55 bits/byte over 8 samples |
| `0xb7` (init) | IN | 16-byte block(s) | the response(s); entropy ~3.95 bits/byte |
| `0xb9` | IN | `b9 40` (2 B) | flow/status ack around the exchange |
| `0xff` | IN | `ff 20` (2 B) | status / NAK-like |
| `0x09` | OUT↔IN | `09 <8B>` / `09 <7B>` | a keyed exchange that RECURS through the session — likely a crypto-layer/session refresh; keyed, not analyzed |

**Shared container.** The 16-byte enciphered unit carrying the `0xb7` auth
response is the **same envelope** the diagnostic data channel (`0xb8`/`0xb7`)
uses. Auth and data differ by **function, not form** — same 16-byte block shape,
different purpose. This is exactly why the boundary is drawn on *function*.

---

## 3. Static loci — NOTED, NOT ANALYZED

Addresses in `research/clb-crack/bin/VCDS-arm64-unpacked.exe` (ImageBase
`0x140000000`). Listed so the mechanism's location is on record; the internals
were deliberately not traced.

- **Session-key install (adjacent to auth):** `0x140072ec0` is a descriptor-driven
  key *import* — it takes a caller-supplied blob (`x1`, len `w2`), decodes it via
  `0x14007ce68`, memcpy's the 32 bytes into the cipher-context key slot `ctx+0x5da4`,
  and runs the AES-256 schedule (`0x14007b140`, IV table `0x140171d30`). **The key
  bytes come from the caller's blob, not any static literal.**

  > **CORRECTION (2026-07-05):** the earlier claim that "the caller is reached only
  > through a runtime-installed method pointer — no static `bl` anywhere" was **WRONG**.
  > It was an artifact of a capstone bug (`md.disasm` stops at the first undecodable
  > word, so the prior sweep saw only ~422 of ~350k `.text` instructions). A word-by-word
  > sweep (`research/clb-crack/xref.py`, `disfn.py`) finds **two direct `bl 0x140072ec0`
  > callers**, both inside function **`0x14006d6c8`** (the received-block dispatcher):
  > `0x14006d9d8` and `0x14006dc04`. Each installs the key from a stack buffer
  > (`sp+0x30..`) holding a **plaintext `0xf0`-marked block** (`(byte&0xf0)==0xf0` guard
  > at `0x14006d844`), with `x1 = sp+0x32`, `w2 = [sp+0x34]-3`; the decode reads
  > `blob+3`. So the derivation trail is **statically reachable**, not hidden.

- **The decode `0x14007ce68` is a structured/length-driven decode (2026-07-05):**
  a 128-byte-stride algorithm-descriptor table at `0x14055555c`, size-driven
  allocations (`0x1401318a0`), bit-length arithmetic (`w4 lsr 3`, `tst w4,#7`), and a
  multi-slot walk (`x21+0x50/0x68/0x98/0xb0`) in `0x14007d010`/`0x14007c858`. The
  32-byte AES key is the *product* of this decode over the caller's blob, not raw
  bytes lifted off the wire.

- **The whole crypto is SYMMETRIC — no public-key primitive (2026-07-05, decisive):**
  a name-string scan of the image finds only THREE registered crypto primitives:
  **`aes`** (`0x14017ad80`), **`sha256`** (`0x14017ad94`), **`sprng`** (secure PRNG,
  `0x14017ae18`). No `ecc`/`rsa`/`dh`/`ecdh`/`x25519`/`curve25519` anywhere. So the
  session-key derivation and the `0xb6` genuineness check are built from AES + SHA-256
  + a PRNG — **not** an asymmetric key agreement. Consequence: the derivation is very
  likely `key = KDF_sha256/aes( b6-challenge, b7-response, STATIC_APP_SECRET )` with a
  **static secret embedded in the binary** (the `0xb6` random bytes look like `sprng`
  output = the app's nonce). This is **potentially replicable** by a live interop tool
  once the static secret + the exact KDF are recovered — no PK wall. The earlier
  SHA-256 brute (§4b) failed precisely because it lacked the embedded-secret input.
  **Next:** locate the static secret (high-entropy 16/32-byte `.rdata` constants
  referenced from `0x14006d6c8`/the `b7` handler) and trace the KDF from `b7`-receipt
  into `0x140072ec0`; then re-run the brute with the secret as a KDF input.
- **Genuineness signature compare:** fn `~0x140073380` parses an interface-response
  packet and compares a 6-byte cable signature against a hardcoded literal at
  `0x140073568` (bytes `01 00 00 c0 1e 00 00 00`); the reject path emits
  "This interface appears to have an issue." (string VMA `0x14017adc0`, xref
  `0x1400734f4`). Noted as the genuineness check; its validation logic not analyzed.
- **Presence/driver gate (separate, not auth):** `USB_Check` `0x1400747a8` — the
  SETUPAPI enumeration + D2XX driver-load path behind the "Driver/Interface Not Found"
  dialog. This is a device-presence gate. See
  `[[vcds-cable-detect-re]]`.

---

## 4. Why the session key can't be synthesized offline (evidence, not a method)

Passive black-box observation across the **two independent-session captures**: the
same logical diagnostic channel decodes to **identical UDS plaintext** but under
**different keystreams** (e.g. an RDBI channel: `reading-ecus` KS ≠ `init-only` KS,
ciphertext wholesale-different, frame-count fingerprint identical). With a static IV
table and a fixed channel selector, `keystream = AES(IV, key)` differing per session
means the **AES key changes every session**. The only per-session secret established
at setup is the `0xb6` challenge/response. Ergo the key is a product of the auth.

This is a *classification result* (the key is auth-derived), reached entirely from
captures — no key, challenge, or response-predictor was recovered.

### 4b. Two things pinned down (2026-07-05) — narrows the crack

- **The session key is NOT present as clear bytes in the setup capture.**
  `research/clb-crack/crack_session_key.py` slides a 32-byte window over every
  setup-phase byte of `reading-ecus.pcapng` (all/OUT/IN concatenations) plus named
  blobs (`b6`, both `b7`, the `09` exchange) plus SHA-256 KDFs over every 1/2/3-way
  combination — **681 candidates**, each verified against the already-recovered
  `KS_F3` keystream ground truth (`AES256(K).enc(IV_TABLE[4])[6:14] == 02 a9 99 f6
  da 7c 9c 3a`). **Zero hits.** The key is neither a raw wire slice nor a simple
  hash of the handshake bytes.
- **The key can only come from the derivation — you cannot back it out of the
  keystream.** `KS_channel = AES_encrypt_block(K, IV_row)` is a single AES block;
  recovering `K` from a known `(IV_row → KS)` pair is exactly breaking AES-256.
  So there is no shortcut: obtaining `K` for a *new live session* requires
  reproducing the derivation, which (§3 correction) decodes a **structured
  asymmetric/bignum blob** — the genuineness crypto. This is the real wall for a
  live interop key, and it is squarely the anti-counterfeit mechanism.

---

## 5. What is deliberately NOT in this document (and why)

- The **challenge algorithm** (how a valid `0xb7` response is computed from a `0xb6`
  challenge).
- The **session-key derivation** (how the 32-byte blob fed to `0x140072ec0` is built).
- Any **response predictor** or the meaning of the 6-byte signature as a validator.

---

## Cross-references
- `research/vag-hex-framing.md` — wire format, the link cipher (AES), and the
  session-key classification verdict `(b)`.
- memory `[[vcds-cable-detect-re]]` — cable-detect RE + the cipher/auth facts.
