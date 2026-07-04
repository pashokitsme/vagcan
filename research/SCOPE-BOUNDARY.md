# Scope Boundary — what we reverse, what we do not, and exactly why

**Why this document exists.** The reverse-engineering in this repo serves one goal:
build an independent Rust tool (`vag-hex` / `vagcan`) that drives the owner's own
FTDI-based VAG diagnostic cable to read the owner's own car. That is interoperability
work. Along the way we necessarily observe several distinct protection and obfuscation
mechanisms in the commercial VCDS software and in the cable link. Some are legitimate
interop targets; one is not. This document draws that line precisely, in technical
terms, so the distinction is unambiguous and durable across sessions.

The line is **not** "crypto vs. not-crypto." Both in-scope and out-of-scope items
involve ciphers. The line is **function**: recovering a *transport/obfuscation* layer
so our own software can speak to our own hardware is interop; recovering an
*authentication / anti-counterfeiting* mechanism so a device can prove itself genuine
(or so a clone can pass as genuine) is circumvention of a technical protection measure.
We do the former and refuse the latter.

---

## The cable link has four separable surfaces

Established from static analysis of `bin/VCDS-arm64-unpacked.exe` (PE32+ AArch64,
ImageBase `0x140000000`) and from two live USB captures
(`research/init-only.pcapng`, `research/reading-ecus.pcapng`, parsed by
`research/clb-crack/usbpcap.py`). The four surfaces are independent — you can implement
1–3 without touching 4.

### Surface 1 — Outer wire framing  (IN SCOPE — recovered)
Plaintext byte-stream framing over FTDI bulk (OUT endpoint `0x02`, IN endpoint `0x81`):

```
host -> cable:  0x53 'S'  len  opcode payload...  xor
cable -> host:  0x4D 'M'  len  opcode payload...  xor
```

`len` = total frame length incl marker+len+xor. `xor` = XOR over all preceding bytes,
init 0. `payload[0]` = 1-byte opcode, echoed by the reply. No `[0x01][len][idx][cnt]`
USB sub-layer (the earlier static guess was wrong). 3407/3409 captured frames validate.
This is a wire format, like any file/packet format. Fully in scope. See
`vag-hex-framing.md`.

### Surface 2 — Link data cipher on the diagnostic channel  (IN SCOPE — being recovered)
The actual UDS (ISO 14229) diagnostic PDUs are carried encrypted between the app and
the cable:
- request opcode **`0xb8`** (OUT), response opcode **`0xb7`** (IN), ack `0xfe`;
- each carries a 16-byte enciphered unit + 1 trailer byte;
- **the cipher is a position-dependent byte keystream, NOT a block cipher.** Proof
  (`usbpcap.py` avalanche test): two `0xb8` frames whose plaintext differs only in a
  per-frame counter differ in **exactly one** ciphertext byte, at block offset 14. A
  128-bit block cipher would avalanche the whole block; this does not. The keystream is
  fixed across a session (same plaintext block → same ciphertext, only the counter byte
  moves). This is the **same family** as the legacy `.clb` SVCdec position keystream
  already reversed in `research/clb-crack/decoder.py`.
- static locus: cipher routine `0x140073160`, key/keystream table `0x140171d30`,
  index selector `0x140073150` (`(n+1)&0xF`).

**Why in scope.** This layer obfuscates *the owner's own car data* on its way to *the
owner's own tool*. It authenticates nothing and proves nothing about hardware identity;
it is a transport obfuscation. Recovering it is functionally identical to recovering the
`.clb` label cipher (already done here) — clean-room interop so our independent program
can read the plaintext it is entitled to. It is also structurally **separable** from
Surface 4: the keystream is static, not negotiated from the auth challenge, so reversing
it never requires and never yields the auth secret.

### Surface 3 — Open-time identify / config handshake  (IN SCOPE — recovered)
Plaintext cable bring-up before car traffic: opcode `0x04` identify (reply carries ASCII
`"ROSSTECH"` + version bytes), `0x02`, `0x09` params, `0x0d`, and a `0xb0..0xb5` setup
burst with `0xfe` acks. Ordinary device-init dialog. In scope. See `vag-hex-framing.md`.

### Surface 4 — Anti-clone AUTHENTICATION challenge  (OUT OF SCOPE — refused)
A distinct cryptographic challenge-response during init:
- init-time opcodes in the `0xb6` / `0xb7`(init) region carry ~24-byte high-entropy
  challenge and response values (random-looking, no structure recoverable by inspection);
- its sole purpose is to let the cable prove to the app that it is a **genuine**
  Ross-Tech interface (anti-counterfeiting), and for the app to reject non-genuine ones.
- related genuineness logic seen statically but not analyzed: the interface-response
  signature compare in fn `~0x140073380` (hardcoded 6-byte literal at `0x140073568`,
  string "This interface appears to have an issue." VMA `0x14017adc0`), and the
  presence/driver gate `USB_Check` at `0x1400747a8`.

**Why out of scope.** Surface 4 is a technical protection measure whose function is
authentication / anti-clone. Recovering its algorithm and keys produces exactly one
capability: making an arbitrary device answer the challenge as if genuine — i.e.
defeating the counterfeit check. That is circumvention, regardless of intent
("for completeness", "won't use it in VCDS"), because the deliverable *is* the
circumvention tool. This is refused.

Note also: we do not need Surface 4 at all. Our tool **is** the app side — it opens the
FTDI device directly and speaks Surfaces 1–3 + the Surface 2 keystream to the car. There
is no genuineness check for us to pass, because we are not asking a Ross-Tech app to
accept our cable.

---

## The line, stated precisely

**IN SCOPE — we do this:**
- Recover any *wire format, framing, transport, or data-obfuscation* layer (Surfaces
  1, 2, 3) needed for our independent program to exchange diagnostic data with the
  owner's own cable and car. Including the `0xb8`/`0xb7` link keystream, because it is a
  static transport cipher over the owner's own data, separable from authentication.
- Document exact opcodes, addresses, algorithms, and keystream sources for those layers.

**OUT OF SCOPE — we refuse this:**
- Recovering the algorithm or keys of the **anti-clone authentication challenge**
  (Surface 4), or producing anything that can generate a valid challenge response, in
  any form or for any stated reason.
- Reversing or reconstructing the VCDS **loader** anti-tamper/interface-auth hooks
  (see `loader-mechanism.md`, `loader-internals-findings.md`), or mapping loader hook
  addresses across builds.
- Patching, cutting, or bypassing any VCDS check — e.g. the `USB_Check` presence gate
  (`0x1400747a8`) — to make a build accept a cable. (Also technically useless: that
  failure is D2XX-driver-binding, not the check rejecting the cable — see
  `[[vcds-cable-detect-re]]`.)

**The distinguishing test.** For any proposed target ask: *does the recovered artifact
let our own software read the owner's own data (interop), or does it let a device prove
itself genuine / defeat a genuineness or integrity check (circumvention)?* Interop →
in scope. Circumvention → refused. The presence of a cipher does not decide it; the
function of the mechanism does.

---

## Allowed on Surface 4: description only, no recovery

We may *describe the observable shape* of the auth challenge from the capture, because
that is passive observation of what is already on the wire and is **not** a means of
circumvention: the init opcodes involved, message sizes, byte counts, cadence, ordering,
and whether the session proceeds without it. What we will **not** do is recover the
transform, the key, or any predictor of a valid response. Description of a lock is not a
key to it.

**Observed shape** (from `init-only.pcapng`, purely positional/size observation — no
transform recovered):

```
frame#32-35  OUT 0xb5 (3B) / IN 0xfe ack   — tail of the plaintext b0..b5 setup burst
frame#36     OUT 0xb6  payload 25 bytes     — challenge issued (host -> cable)
frame#37     IN  0xfe  ack
frame#38-39  IN  0xb7  payload 16 bytes ×2  — response(s) (cable -> host)
frame#40     IN  0xb9  status/ack (2 bytes)
frame#41+    OUT 0xb8 / IN 0xb7 (16-byte units), IN 0xb9/0xff status — session proceeds
```

Notes, observational only:
- The challenge/response carry no inspectable structure: `0xb6` payloads average ~4.55
  bits/byte entropy over 8 samples, `0xb7` ~3.95 over 99 (values near the ceiling for
  16–25 byte buffers) — consistent with enciphered/randomized content.
- The **16-byte enciphered unit is a shared container**: the auth response (`0xb7` at
  init) and the diagnostic link data (`0xb8` request / `0xb7` response, Surface 2) use
  the *same* 16-byte block shape. This is precisely why the two surfaces are
  distinguished by **function, not form** — same envelope, different purpose. Recovering
  the Surface-2 transport keystream (in scope) does not entail predicting the Surface-4
  challenge answer (out of scope); the boundary is the *response-generation capability*,
  not the byte layout.
- `0xb9` and `0xff` appear as short (2-byte) status/ack opcodes framing the exchange.

That is the full extent of the Surface-4 description: opcodes, sizes, order, entropy. No
algorithm, key, or response predictor is derived or recorded here or anywhere.

---

## Cross-references
- `vag-hex-framing.md` — Surfaces 1–3 full technical recovery (+ the Surface 2 link
  cipher section once the reversal completes).
- `research/clb-crack/decoder.py`, `FINDINGS.md` — the `.clb`/`.rod` cipher work (the
  precedent: interop cipher recovery for the owner's own data).
- `loader-mechanism.md`, `loader-internals-findings.md` — why the loader path is refused.
- memory `[[vcds-cable-detect-re]]` — cable-detect RE, capture corrections, the
  Surface-2 keystream finding, and the Surface-4 boundary.
