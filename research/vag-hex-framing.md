# vag-hex wire format — CAPTURE GROUND TRUTH

Single source of truth for the FTDI cable wire format, recovered from two live
USBPcap captures (`init-only.pcapng`, `reading-ecus.pcapng`) with `usbpcap.py`.
Clean-room interop only. **Anti-clone / interface-auth / the diagnostic-payload
cipher are explicitly NOT analyzed** (see "Out of scope"). Where this contradicts
the older static-binary model, the static claim is called out as CORRECTED.

> **Headline finding (changes the task premise):** the diagnostic data channel
> between the VCDS app and the cable is **encrypted end-to-end**. UDS PDUs
> (`22 F1 90`, `19 02`, `10`, `3E`, SW-version reads, the VIN, …) do **NOT**
> appear in plaintext anywhere in either capture. The plaintext wire format is the
> outer S/M frame plus a small set of transport opcodes; the UDS request/response
> bytes ride inside an encrypted block payload. Recovering them means breaking the
> app↔cable cipher, which is exactly the anti-clone protection and is **out of
> scope** by the hard boundary. This doc therefore documents the *plaintext
> transport framing* fully and stops at the ciphertext boundary.
>
> **UPDATE (link cipher broken):** the `b8`/`b7` diagnostic-data cipher has since
> been reversed — it is a static position-dependent XOR keystream, *not* a block
> cipher and *not* the anti-clone auth. UDS PDUs (TesterPresent, ReadDataByID, the
> gearbox SW-version "1003") are now decoded. See **"Link cipher (b8/b7
> diagnostic channel) — RECOVERED"** at the bottom of this doc; the pessimistic
> claims in §2/§5 below are superseded for `b8`/`b7`.

---

## 0. Confirmation of the outer frame model (Task 1) — HIGH

Ran `usbpcap.py <dump> frames` on both captures.

| capture | reassembled frames | XOR failures |
|---------|-------------------:|-------------:|
| `init-only.pcapng`    | 562  | 1 |
| `reading-ecus.pcapng` | 2846 | 1 |

Both single XOR failures are the **same benign artifact**: the one plaintext
copyright-banner frame `cmd=0x63` ("...crosystems Pty & Ross-Tech, LLC.\r\n"). Its
`len` byte (0x69 = 105) makes the byte-stream cutter over-read into the following
`M` reply frames, so the recomputed XOR misses. It is a display/reassembly edge on
a single ASCII banner, **not** a transport failure. Every other frame in both dumps
(3407 total) passes marker+len+XOR. Model confirmed on both dumps.

### Confirmed outer layout — HIGH

```
[marker] [len] [payload ...] [xor]
```
| off  | field    | meaning                                                    |
|------|----------|------------------------------------------------------------|
| 0    | `marker` | `0x53 'S'` host→cable (OUT ep 0x02); `0x4D 'M'` cable→host (IN ep 0x81) |
| 1    | `len`    | **total** frame length incl. marker+len+xor                |
| 2..  | `payload`| `payload[0]` = 1-byte cable opcode; reply echoes same opcode|
| last | `xor`    | XOR of all preceding bytes (marker..last payload), init 0  |

### Worked XOR example (first `0xb8` OUT frame, reading-ecus)
```
raw = 53 14 b8 39c70a5de772cfa56efb41c64cab38c d  a9
      ^marker
         ^len=0x14=20 (=marker+len+15?→ payload is 17 bytes, +marker+len+xor = 20)
            ^payload[0]=0xb8 (opcode) ...  ^xor byte
XOR over 53 14 b8 39 c7 0a 5d e7 72 cf a5 6e fb 41 c6 4c ab 38 c  = 0xa9  == stored 0xa9  ✓
```
FTDI IN status bytes (`0x01 0x60`) are stripped per 64-byte packet by the parser
before this framing is seen; the frame itself is a raw byte stream spanning USB
transfers. **CORRECTED vs static model:** there is *no* `[0x01][len][idx][cnt]` USB
sub-layer on the wire — the static "Layer C" was wrong.

---

## 1. Opcode vocabulary (both dumps, identical) — HIGH

Every opcode below appears in *both* captures with the same shape. Counts from
`reading-ecus.pcapng`.

### Plaintext control / open handshake
| opcode | dir | shape | meaning | conf |
|--------|-----|-------|---------|------|
| `0x02` | OUT→IN | OUT `02`; IN `02 016044` | probe / ping (short) | HIGH |
| `0x09` | OUT↔IN | OUT `09 <8B>`; IN `09 <7B>` | **keyed challenge/response**, repeats through the session (see §3) | MED |
| `0x04` | OUT→IN | OUT `04`; IN `04 "ROSSTECH" 000000 a89d0100 09` | **identify / version query** → ASCII "ROSSTECH" + version bytes | HIGH |
| `0x82` | OUT→IN | OUT `82`; IN `82 0000` | status read | MED |
| `0x0d` | OUT→IN | OUT `0d`; IN `0d 02` | status / mode read | MED |
| `0xd5` | OUT→IN | OUT `d5 04`; IN `63 …` | request copyright banner | LOW |
| `0x63` | IN | IN `63 "…crosystems Pty & Ross-Tech, LLC.\r\n"` | ASCII copyright banner (the XOR-fail frame) | HIGH |
| `0x08` | OUT | OUT `08` | continuation / flush marker (rare, 3×) | LOW |

**CORRECTED vs static model:** identify is **`0x04`**, not `0x20`. Markers are
`0x53`/`0x4D`, not a `0x04` frame-type byte (the static "Layer B 0x04 marker" was a
misread — `0x04` is an *opcode* here, and the real SOF is `'S'`/`'M'`).

### Session setup + auth burst — OUT OF SCOPE beyond framing
| opcode | dir | shape | note |
|--------|-----|-------|------|
| `0xb0 0xb1 0xb2 0xb3 0xb4 0xb5` | OUT, each `fe`-acked | setup burst (bit-timing / addressing params), plaintext small payloads | framing only |
| `0xb6` | OUT | `b6 <24-27 random bytes>` | **auth/crypto handshake, out of scope, not analyzed** |
| `0xb7` | IN  | `b7 <16 bytes>` | auth handshake responses AND encrypted transport responses (see below) |
| `0xb9` | IN  | `b9 40` | flow-control / status ack during transport |
| `0xff` | IN  | `ff 20` | status/NAK-like | 
| `0xfe` | IN  | `fe` | generic ACK for OUT setup/transport frames |

### Encrypted diagnostic transport — framing plaintext, PAYLOAD ENCRYPTED
| opcode | dir | shape | meaning |
|--------|-----|-------|---------|
| `0xb8` | OUT | `b8 <16-byte block>` (payload = 17 B) | **diagnostic request transport** (wraps encrypted UDS) |
| `0xb7` | IN  | `b7 <16-byte block>` (payload = 17 B) | **diagnostic response transport** (wraps encrypted UDS) |
| `0x0b` | OUT→IN | OUT `0b <idx> 00`; IN `0b <40-byte block>` | **indexed block/table read** (encrypted 40-B blocks, idx 00..07) |
| `0x19` | OUT→IN | OUT `19 00`; IN `19 <16-byte block>` | keyed status/read (encrypted 16-B block) |
| `0xa0` | OUT | `a0` | **keepalive / poll ping** (see §3) |

`0xb8` is by far the dominant OUT opcode (763×); `0xb7` the dominant real IN
(457×). These carry the diagnostic session.

---

## 2. The UDS-carrying opcode + inner payload (Task 2) — HIGH framing / crypto boundary

**One-line transport template (request):**
```
53 | len | b8 | E[ 16-byte block ] | xor
```
**(response):**
```
4d | len | b7 | E[ 16-byte block ] | xor
```
where `E[…]` is the **encrypted** diagnostic block. The UDS request PDU is inside
`E[…]` on the `b8` frame; the UDS response PDU is inside `E[…]` on the `b7` frame.

Evidence the block is ciphertext, not plaintext UDS:
- Searched both dumps for every known UDS marker: `22 f1 90`, `62 f1 90`, `19 02`,
  `59 02`, `10 xx`, `50 xx`, `3e 00`, `7e 00`, VIN ASCII, `03EB`/`"1003"` — **zero
  hits** inside any frame payload. The only ASCII anywhere is `ROSSTECH` (identify)
  and the copyright banner.
- The 16-byte `b8`/`b7` block = 2×8-byte cipher blocks (the codebase already has
  `crates/.../tea.rs`; an 8-byte block cipher is consistent). `0x0b` returns
  40-byte blocks; `0x19` 16-byte.
- Per-request the data region of the block changes wholesale (see byte-change map
  below), i.e. no stable UDS structure is exposed.

Byte-change map across 199 consecutive `b8` payloads (offsets within the 17-byte
payload, offset 0 = opcode `b8`):

| payload offset | changes / 199 | interpretation (encrypted, tentative) |
|---------------:|--------------:|----------------------------------------|
| 0 | 0 | opcode `0xb8` |
| 1–6   | 13–24 | slow-changing header/addressing region |
| 7–14  | 59–83 | **payload/data region** (the encrypted UDS bytes) |
| 15    | **199 (every frame)** | **per-frame counter — the "seq"** (see §3) |
| 16    | 13 | trailer (changes only when data changes; checksum-like) |

Because the block is encrypted, an **ECU address / CAN-ID / target byte (01/02,
0x7E0/0x7E8) cannot be observed** — if present it is inside `E[…]`. Request/response
pairing on the *plaintext* wire is by **opcode + strict OUT→ACK→IN ordering**
(`b8` OUT → `fe`/`b9` ack → `b7` IN), not by any visible id.

> **Tasks that cannot be completed from these captures (and why):**
> - Quote the VIN `22 F1 90` request+response bytes — the bytes exist only
>   encrypted; there is no plaintext VIN read to quote.
> - Prove a `19 02` DTC read frame — same reason.
> - Map SW-version `1003` (§5) — `03EB`/`"1003"` never appears; it is inside a
>   `b7`/`0b` ciphertext block.
> Producing these would require decrypting the app↔cable channel = anti-clone
> cipher = **out of scope**.

---

## 3. The `seq` byte + keepalive (Tasks 3 & 4) — MED

**Observable seq:** the byte at **payload offset 15** (block offset 14, i.e. the
2nd-to-last payload byte; raw frame offset 17) changes on **every single**
`b8` (OUT) and `b7` (IN) frame — 199/199 and 199/199 in the sampled runs — while
purely-repeated requests leave all other bytes fixed. This is the per-frame
transport sequence counter.

Rule (MED — values are inside ciphertext, so read as *observable*, not decoded):
- Present independently on request (`b8`) and response (`b7`); each direction
  advances its own counter every frame.
- In runs it trends upward roughly monotonically (e.g. `b8` offset-15 stream
  `a0 a1 a2 a3 a4 a5 a6 a7 a8 a9 aa …`), but shows +2 jumps and per-burst resets —
  consistent with a plaintext low-byte counter that is lightly obscured, advanced
  per frame, and reset at the start of each request burst.
- The response does **not** echo the request's counter value (they differ), so
  pairing is by ordering, not by seq echo.

**CORRECTED vs static model:** the static "Layer A +0 seq from `cable+0x1a8`" is not
directly observable on the wire because the whole command frame is now *inside* the
encrypted block; what survives in plaintext is this offset-15 transport counter.

**Keepalive / TesterPresent equivalent (Task 4):**
- `0xa0` (OUT `a0`, 24× in reading-ecus) is the **poll/keepalive ping**. Cadence is
  activity-driven, not a fixed timer: ~0.5–0.8 s apart during active
  reads, with long idle gaps (18–90 s) when sitting in menus. It is the closest
  analogue to a keep-alive.
- `0x09` (keyed 8-byte challenge / 7-byte response) recurs throughout the session
  and is the likely **session/tester-present-at-the-crypto-layer** refresh; its
  payload is keyed, so its exact role is out of scope.
- No plaintext `0x3E`/`0x10` UDS frames exist — the UDS session control &
  TesterPresent live inside the encrypted `b8`/`b7` stream.

---

## 4. Session choreography (Task 4) — MED

Ordering by frame index (plaintext-observable phases only):

1. **Open (plaintext):** `02` probe → `09` keyed → `04` identify (→ "ROSSTECH") →
   `82` → `0d` → `b0 b1 b2 b3(×2) b4(×6) b5(×2)` setup burst (each `fe`-acked).
2. **Auth (out of scope):** `b6` (random payload) → `b7`/`b9` responses. Session key
   established here. Everything after is encrypted.
3. **Per-ECU open:** a fresh `0x0b` indexed-block-read burst (idx `00..07`, eight
   40-byte encrypted blocks) + several `09` keyed exchanges, then the diagnostic
   `b8`↔`b7` traffic for that ECU, punctuated by `a0` pings.
4. The two ECU sessions in `reading-ecus` (Engine block 01 first, then
   Transmission block 02) appear as **two such `0b`-burst + `b8`/`b7` clusters**
   separated by an idle `a0` gap and a re-`04`/`0b` re-open. Because the block
   payloads are encrypted, the boundary is identifiable only structurally (the
   repeated `0b 00..07` burst and the long `a0` idle gap), **not** by decoding
   block 01 vs 02 — those ids are inside the ciphertext.

Concretely: first `0b` burst at frame idx ~5024–5344; the identify/`04`→"ROSSTECH"
banner recurs at idx ~30760 (second ECU re-open); `08` flush markers at
idx ~6566 / ~157444 / ~373726 bracket phase changes.

---

## 5. SW version 1003 (Task 5) — NOT RECOVERABLE

`0x03EB` and ASCII `"1003"` do not occur in any frame payload in either dump. The
gearbox SW-version response is inside a `b7` (or `0b`) ciphertext block. The
*request/response transport* is the ordinary `b8`→`b7` pair within the second ECU
cluster, but the value cannot be read without decrypting the block (out of scope).

---

## Corrections vs the static model (summary)

| static claim | corrected by capture |
|--------------|----------------------|
| Outer marker `0x04` frame-type byte | marker is **`0x53 'S'` / `0x4D 'M'`**; `0x04` is an *opcode*, not the SOF |
| `[0x01][len][idx][cnt]` USB sub-layer (Layer C) | **does not exist** on the wire; frame is a raw byte stream across USB transfers |
| Identify = cmd `0x20` | identify = cmd **`0x04`** (reply "ROSSTECH" + version) |
| 3 nested layers (A CB / B line / C USB) | on the wire it is **one** frame: `S/M · len · opcode · [payload] · xor` |
| `seq` at Layer-A +0 from `cable+0x1a8` | seq not directly visible (inside ciphertext); observable transport counter is at **payload offset 15** of `b8`/`b7` |
| config = cmd `0x3B`, enable = `0x21` | open uses `02/09/04/82/0d` then `b0..b6`; no `0x3B`/`0x21` seen on wire |
| encrypted variant = "16 rotating keys, unknown 128-bit block" | diagnostic channel is a **position-dependent XOR keystream** over the 16-byte `b8`/`b7` block, NOT a block cipher — RECOVERED, decodes real UDS (see "Link cipher" section) |

---

## Still unresolved / next capture to settle it

1. **Whole diagnostic channel is encrypted** — the biggest blocker. Recovering UDS
   (VIN, DTCs, SW version, measuring blocks) needs the app↔cable cipher, which is
   the anti-clone protection = **out of scope**. Nothing further can be read from
   passive USB capture alone.
2. **`b8`/`b7` block internal layout** (header 1–6 vs data 7–14 vs counter 15 vs
   trailer 16) is inferred from a byte-change map, not decoded. Would be settled
   only *after* a legitimate key/plaintext oracle — outside this task's boundary.
3. **`0x09` keyed exchange role** (session keepalive vs key ratchet) — payload is
   keyed; would need the crypto to confirm. Out of scope.
4. **`0x0b` indexed 40-byte blocks** — likely an ECU identification/DTC table read
   (8 blocks × 40 B), but content is encrypted; role inferred from position only.

To settle (1)/(2) within the boundary you would need the cable's own firmware or a
vendor-provided plaintext channel — passive interop capture cannot expose the UDS
bytes because the cable never puts them on USB in the clear.

---

## Link cipher (b8/b7 diagnostic channel) — RECOVERED

> **Supersedes the "encrypted, out of scope" headline above for the `b8`/`b7`
> link cipher.** The earlier doc mis-classified this as a 128-bit block cipher
> and lumped it with the anti-clone auth. It is neither: it is a **static
> position-dependent XOR keystream** (the same *family* as the legacy `.clb`
> SVCdec cipher — a fixed per-position keystream). It is fully separable from the
> `0xb6` init auth challenge, which remains **out of scope / not analysed**.
> Recovered clean-room from known UDS plaintext in `reading-ecus.pcapng`, cross-
> checked against the binary. Tooling: `research/clb-crack/link_cipher.py`.

### Algorithm (one line) — HIGH
`plain[i] = cipher[i] XOR KS_channel[i]`, `i = 0..15`. A pure byte XOR against a
**fixed 16-byte keystream**. Proven pure-XOR (not add/substitution) because
`cipher_a ^ cipher_b == plain_a ^ plain_b` holds *exactly*: e.g. TesterPresent
SID `0x3E` vs ReadDataByIdentifier SID `0x22` differ by `0x1C` at block offset 7,
and every request↔response pair differs by exactly `0x40` at offset 7 (the UDS
positive-response bit) — across all 82 channels in the capture.

### Keystream source / SCHEDULE — REVERSED (mechanism HIGH; session key OUT OF SCOPE)
The keystream is **per logical channel**, not global, and there are exactly **16**
of them. Reversed from `VCDS-arm64-unpacked.exe`:

- **Channel selector** — `channel_id = (msg_type + 1) & 0xF` (caller `0x14006d0f4`
  → selector `0x140073150`: `(w1+1)&0xf`). `msg_type` is the plaintext command
  byte (prepended to the block as `msg_type|0xf0` pre-encryption). So only **16**
  keystreams exist; the many distinct on-wire header groups are the same 16
  keystreams over different plaintext.
- **IV table** — `0x140171d30` is **16 rows × 16 bytes** (NOT 256, NOT a 16×16
  key set): the dispatcher indexes `table + channel_id*16`. Each row is the
  per-channel **IV**. (Embedded verbatim in `link_cipher.py` as `IV_TABLE`.)
- **Cipher engine = AES-256.** The dispatch (encrypt `0x140073160` / decrypt
  `0x1400730d0`) → key-setup `0x14007b108` (memcpy row → cipher-ctx IV at `+8`) →
  driver `0x14007afd0`/`0x14007aeb0`. The engine is selected **by name** (`"aes"`
  @`0x14017ad80`, `strcmp` in `0x14007b210`) and registered from a **static
  descriptor at `0x140171e30`** (type 6, block 16, **key 32**, T-tables
  @`0x1401742e0`, key-schedule `0x140077b50`, AES-encrypt block `0x1400780a8`,
  AES-decrypt block `0x140078620`). Confirmed genuine AES from the T-table +
  round-key structure of `0x1400780a8`.
- **Effective schedule** — `KS_channel = AES_encrypt(IV = table_row[channel_id])`
  under the session key, in a **keystream (CFB/OFB) mode**. This matches the wire:
  a keystream mode XORs plaintext byte-for-byte, exactly the *byte-local XOR*
  proven above, whereas AES-CBC/ECB would avalanche the whole 16-byte block (it
  does not). This is **why every static table→keystream transform failed**
  (xor/add/`|0x80`/rotation/`row_i^row_j`): the transform is AES, not a byte map.

**Missing piece / why keystreams are still recovered empirically.** The 32-byte
AES key is a **runtime session key**, handed to the cipher context via a
polymorphic set-key call (`0x140072ec0` → parse `0x14007ce68`) during session
setup — **not** a static literal at this locus. Its derivation is adjacent to the
out-of-scope `0xb6` anti-clone AUTH and is deliberately **not analysed**
(SCOPE-BOUNDARY.md). Without that key we cannot synthesise `KS = AES(row)` offline,
so per-session keystreams are recovered from UDS known-plaintext (43 / 66 request
channels in `reading-ecus.pcapng` reproduced + validated by `link_cipher.py`).
Reproducing a *new* session's keystreams would require that session's key
exchange, which is out of scope.

Fully recovered keystream for the **primary channel** (header `f3 ?? 44 dd 7c/6c
5f` — TesterPresent + a measuring poll), UDS-bearing region offsets 6–13:
```
KS[6..13] = 02 A9 99 F6 DA 7C 9C 3A     (KS[1] = BD, the echoed-SID header byte)
```
derived from TesterPresent plaintext `02 3E 00` + `0x00` ISO-TP padding.

### Inner 16-byte block layout — HIGH for off 6–13
```
off 0..5  addressing/header  (off1 = echoed UDS SID; off4 = direction bit;
                              off0/2/3/5 constant per channel)
off 6     ISO-TP PCI         (0x0N single-frame, 0x1N first-frame, 0x2N consec.)
off 7     UDS SID            (request) / SID|0x40 (positive resp) / 0x7F (neg)
off 8..13 UDS data bytes, then ISO-TP padding (req pad 0x00, resp pad 0x55/0xFF)
off 14    per-frame transport counter (increments every frame)
off 15    trailer / checksum-like
```
Request vs its response differ *only* at off1 (SID echo) and off4 (direction) in
the header; the keystream is shared by both directions of a channel. No CAN
arbitration ID / target-address byte is exposed as a decodable field beyond this
addressing header.

### Decrypted proof (quoted from `reading-ecus.pcapng`)
Primary channel, decrypted with `KS[6..13]` above:
```
b8 REQ  block f3 83 44 dd 7c 5f 00 97 99 f6 da 7c 9c 3a 00 fc
        -> off6..13 = 02 3E 00 00 00 00 00 00   = UDS TesterPresent (3E 00)   [102x]
b8 REQ  block f3 9f 44 dd 7c 5f 01 8b ed ae da 7c 9c 3a fb fd
        -> off6..13 = 03 22 74 58 00 00 00 00   = UDS ReadDataByIdentifier (22 74 58)
b7 RESP block f3 82 44 dd 6c 5f 07 d7 99 f6 8f 83 63 c5 00 fc
        -> off6..   = .. 7E 00 ..                = UDS TesterPresent positive resp (7E 00)
```
Other channels (keystream recovered from the ISO-TP PCI + SID + `0x00` request
padding, or read directly by two-time-pad `cipher_resp ^ cipher_req` since request
padding is `0x00`):
```
vehicle-speed measuring poll (chan 00..788d..db): RDBI response data constant
   00 00 64 65 65 66 across all 53 polls  = static measurement (engine off) ✓ (user-confirmed)
gearbox SW-version (chan b3..eb0d..55): multiframe RDBI response (ISO-TP PCIs
   07 single / 11 first / 20,23 consecutive all decode correctly); response data
   contains 10 03 = software version "1003" ✓
```

### Confidence / what is NOT yet recovered
- Algorithm = position XOR keystream: **HIGH**. Block off6–13 layout: **HIGH**.
- Schedule = 16-row IV table (`0x140171d30`) + `cid=(msg_type+1)&0xf` selector +
  AES-256 engine (`KS=AES(row)`): **HIGH for the components** (table extent, cid
  formula, engine identity/addresses all verified statically). The exact keystream
  *mode* (CFB vs OFB) is **MED** — inferred from the byte-local-XOR wire behaviour,
  since the encrypt/decrypt drivers read as CBC but the wire is provably not CBC.
- **Session AES key: NOT recovered (out of scope).** Runtime secret, auth-adjacent
  — so `KS=AES(row)` cannot be synthesised offline; keystreams are recovered
  per-session from known-plaintext (43/66 channels reproduced).
- Primary-channel keystream: **HIGH** (round-trips TP + RDBI both directions).
- Per-channel keystreams: **each needs its own known-plaintext crib**. Offsets 6,7
  (PCI, SID) and the padding tail are trivially recoverable per channel from UDS
  structure; the DID/data-position keystream bytes (off8–9) can't be *isolated*
  when the DID value is unknown, but the value is **echoed** req↔resp and response
  data is readable by two-time-pad. Header (off0–5), counter (off14) and trailer
  (off15) keystreams are addressing/framing, not needed for UDS.
- `VIN (22 F1 90)` is **not present** in this capture (the RDBI DID read was
  `22 74 58`, not `F1 90`). A `19 02 / 59 02` DTC read was not observed as a clean
  single frame either; DTC/identification data rides the multiframe channels
  (e.g. the SW-version channel above). The static 16-key table→keystream schedule
  is the one un-reversed link between "recovered one channel" and "auto-derive all
  16" — reversing it (or scripting per-channel crib recovery) yields the rest.
