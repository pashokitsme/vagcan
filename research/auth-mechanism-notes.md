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
  bytes come from the caller's blob, not any static literal.** The caller of
  `0x140072ec0` is reached only through a **runtime-installed method pointer** — there
  is no static `bl`, stored pointer, or `adrp+add` to it anywhere in the image — and
  the code that *builds* that blob lives in the session-setup / `0xb6` region. **The
  trail into blob construction = the challenge-response; not followed.**
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
