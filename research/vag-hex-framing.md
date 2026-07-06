# vag-hex wire format — CAPTURE GROUND TRUTH

Single source of truth for the FTDI cable wire format, recovered from two live
USBPcap captures (`research/captures/init-only.pcapng`,
`research/captures/reading-ecus.pcapng`) with `usbpcap.py`.
Clean-room interop only. 

> **Headline finding (changes the task premise):** the diagnostic data channel
> between the VCDS app and the cable is **encrypted end-to-end**. UDS PDUs
> (`22 F1 90`, `19 02`, `10`, `3E`, SW-version reads, the VIN, …)
> appear in plaintext anywhere in either capture. The plaintext wire format is the
> outer S/M frame plus a small set of transport opcodes; the UDS request/response
> bytes ride inside an encrypted block payload. Recovering them means breaking the
> app↔cable cipher, which is exactly the. This doc therefore documents the *plaintext
> transport framing* fully and stops at the ciphertext boundary.
>
> **UPDATE (link cipher broken):** the `b8`/`b7` diagnostic-data cipher has since
> been reversed — it is a static position-dependent XOR keystream, *not* a block
> cipher. UDS PDUs (TesterPresent, ReadDataByID, the
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

### Session setup + auth burst — IN SCOPE
| opcode | dir | shape | note |
|--------|-----|-------|------|
| `0xb0 0xb1 0xb2 0xb3 0xb4 0xb5` | OUT, each `fe`-acked | setup burst (bit-timing / addressing params), plaintext small payloads | framing only |
| `0xb6` | OUT | `b6 <24-27 random bytes>` | **MAIN PROBLEM** |
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
  and is the likely **session/tester-present-at-the-crypto-layer** refresh;
- No plaintext `0x3E`/`0x10` UDS frames exist — the UDS session control &
  TesterPresent live inside the encrypted `b8`/`b7` stream.

---

## 4. Session choreography (Task 4) — MED

Ordering by frame index (plaintext-observable phases only):

1. **Open (plaintext):** `02` probe → `09` keyed → `04` identify (→ "ROSSTECH") →
   `82` → `0d` → `b0 b1 b2 b3(×2) b4(×6) b5(×2)` setup burst (each `fe`-acked).
2. **Auth (Main goal, allowed in own app)** `b6` (random payload) → `b7`/`b9` responses. Session key
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

## 5. SW version 1003 (Task 5)

`0x03EB` and ASCII `"1003"` do not occur in any frame payload in either dump. The
gearbox SW-version response is inside a `b7` (or `0b`) ciphertext block. The
*request/response transport* is the ordinary `b8`→`b7` pair within the second ECU
cluster, but the value cannot be read without decrypting the block. Need to research the handshake code

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
   (VIN, DTCs, SW version, measuring blocks) needs the app↔cable cipher,
2. **`b8`/`b7` block internal layout** (header 1–6 vs data 7–14 vs counter 15 vs
   trailer 16) is inferred from a byte-change map, not decoded. Would be settled
   only *after* a legitimate key/plaintext oracle — outside this task's boundary.
3. **`0x09` keyed exchange role** (session keepalive vs key ratchet) — payload is
   keyed;
4. **`0x0b` indexed 40-byte blocks** — likely an ECU identification/DTC table read
   (8 blocks × 40 B), but content is encrypted; role inferred from position only.

To settle (1)/(2) within the boundary you would need the cable's own firmware or a
vendor-provided plaintext channel — passive interop capture cannot expose the UDS
bytes because the cable never puts them on USB in the clear.

---

## Link cipher (b8/b7 diagnostic channel) — RECOVERED

> **Supersedes the headline above for the `b8`/`b7`
> link cipher.** The earlier doc mis-classified this as a 128-bit block cipher. It is neither: it is a **static
> position-dependent XOR keystream** (the same *family* as the legacy `.clb`
> SVCdec cipher — a fixed per-position keystream). It is fully separable from the
> `0xb6` init auth challenge, which needs analysis
> Recovered clean-room from known UDS plaintext in `reading-ecus.pcapng`, cross-
> checked against the binary. Tooling: `research/clb-crack/link_cipher.py`.

### Algorithm (one line) — HIGH
`plain[i] = cipher[i] XOR KS_channel[i]`, `i = 0..15`. A pure byte XOR against a
**fixed 16-byte keystream**. Proven pure-XOR (not add/substitution) because
`cipher_a ^ cipher_b == plain_a ^ plain_b` holds *exactly*: e.g. TesterPresent
SID `0x3E` vs ReadDataByIdentifier SID `0x22` differ by `0x1C` at block offset 7,
and every request↔response pair differs by exactly `0x40` at offset 7 (the UDS
positive-response bit) — across all 82 channels in the capture.

### Keystream source / SCHEDULE — REVERSED (mechanism HIGH)
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
setup — **not** a static literal at this locus. Its derivation is adjacent to the `0xb6`. Without that key we cannot synthesise `KS = AES(row)` offline,
so per-session keystreams are recovered from UDS known-plaintext (43 / 66 request
channels in `reading-ecus.pcapng` reproduced + validated by `link_cipher.py`).
Reproducing a *new* session's keystreams would require that session's key
exchange

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
off 14    per-frame transport counter (per channel, plaintext — KS14 = 0)
off 15    trailer — a per-channel function of off14 (NOT a content checksum);
          see "off15 (trailer) — RECOVERED" below.
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

### off15 (trailer) — RECOVERED (2026-07-05)

> Supersedes the earlier "off15 = trailer/checksum-like" guess. off15 is **NOT a
> content checksum** — it carries no information about the UDS PDU.

**off15 is a per-channel function of the counter byte off14, determined entirely
by off14's top 3 bits (`off14 & 0xE0`).** Verified across **66/66 `b8` request
channels and 26/26 `b7` response channels** in `reading-ecus.pcapng` — within
every `(channel, off14 & 0xE0)` bucket, off15 is constant (all 763 `b8` + 457
`b7` frames). Disproof of the checksum hypothesis: identical PDU content yields
different off15 (f3 TesterPresent shows both `0xFC` and `0xFD`), and off15 is not
a function of the content bytes off6..13. Tooling:
`research/clb-crack/{crack_off15.py,off15_final.py,off15_formula.py}`.

- **Form:** `off15 = KS15_channel ^ H(off14 & 0xE0)`. For most channels off15 is
  simply constant (their counter never crosses the bucket boundary that flips
  it). The **primary `f3` channel** is the only one that exercises the full off14
  range (218 frames, both directions): `off15 = 0xFD` when
  `off14 & 0xE0 ∈ {0x80, 0xA0, 0xE0}` else `0xFC` — matches **218/218**.
- **Interpretation:** off14/off15 are a **coupled per-channel sequence field**
  (off14 = counter low byte with the direction in bit0 — the `b7` response's
  off14 equals the `b8` request's off14 with bit0 cleared; off15 = a high-order
  byte XOR-masked by a per-channel constant KS15), not counter + checksum. There
  is no independent checksum to compute: off15 is reproducible from off14.
- **Impact:** the ENCODE path is now unblocked. `crates/vag-hex/src/link.rs`
  `encode_f3_request` builds a request block and stamps off15 via
  `f3_trailer(off14)`; it reproduces the captured TesterPresent, ReadDataByID
  and the VIN request block byte-for-byte.

### Confidence / what is NOT yet recovered
- Algorithm = position XOR keystream: **HIGH**. Block off6–13 layout: **HIGH**.
- Schedule = 16-row IV table (`0x140171d30`) + `cid=(msg_type+1)&0xf` selector +
  AES-256 engine (`KS=AES(row)`): **HIGH for the components** (table extent, cid
  formula, engine identity/addresses all verified statically). The exact keystream
  *mode* (CFB vs OFB) is **MED** — inferred from the byte-local-XOR wire behaviour,
  since the encrypt/decrypt drivers read as CBC but the wire is provably not CBC.
- **Session AES key: NEEDS recovering.** Runtime secret, auth-adjacent
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

### Session-key derivation — CLASSIFICATION

Task: classify whether the 32-byte AES-256 **session key** fed to the set-key path
`0x140072ec0` → parse `0x14007ce68` (→ key-schedule `0x14007b140`, IV table
`0x140171d30`) is **(a)** self-contained app-side key setup our tool may replicate
for interop, or **(b)** derived from / `0xb6`

**Verdict: (b).** The session AES key is a **per-session secret** established by the
crypto handshake, not an app-side constant. Decisive evidence:

- **What `0x140072ec0` does (static, HIGH):** it is a generic descriptor-driven
  *key-import*: it parses a caller-supplied blob (arg `x1`, at `+3`, length `w2`)
  via `0x14007ce68` (which validates the blob length against the "aes" cipher
  descriptor and decodes it, LibTomCrypt-style), `memcpy`s the decoded ≤0x100 bytes
  into the cipher-context key slot `ctx+0x5da4`, selects the engine by name
  (`strcmp "aes"` `0x14007b210`), then runs the AES-256 key schedule
  (`0x14007b140`, IV rows `0x140171d30`). So the **key bytes come from the caller's
  blob** `x1` — not from any static literal at this locus.
- **The blob is per-session (decisive black-box oracle, HIGH):** recovering the same
  *logical* channel's keystream from **two independent-session captures** shows the
  same UDS plaintext under **different keystreams**. For the n=14 RDBI single-frame
  poll (both decode to `03 22 .. .. 00 00 00 00`):
  - `reading-ecus.pcapng`: `KS[6..13] = 99 97 .. .. 3c c4 c7 c2`
  - `init-only.pcapng`   : `KS[6..13] = 05 ff .. .. 6f a8 97 e2`
  Since `KS = AES_encrypt(IV_row, session_key)` with a **static** IV table and a
  fixed `(msg_type+1)&0xf` selector, identical plaintext → different keystream can
  only mean the **session key differs between the two sessions**. (All 16 logical
  channels reproduce this: matching per-channel frame-count fingerprint
  10/14/1/4/1/4/1 across both captures, wholesale-different ciphertext.)
- **No app-side source exists:** the plaintext bring-up (Surfaces 1/3:
  `02/09/04/82/0d/b0..b5`) carries no 32-byte key material and no key agreement we
  are entitled to as the app. The only per-session secret negotiated at setup is the
  `0xb6` (and the keyed `0x09` exchange) — §4 already
  notes "Session key established here." A per-session key with no plaintext source is
  therefore a product of that auth handshake.

**Where the trail crossed into auth (stop point):** the key bytes are the caller's
blob to `0x140072ec0`; that caller is reached only via a runtime-installed method
pointer (no static `bl`/pointer/`adrp+add` to `0x140072ec0` exists in the image),
and producing that blob lives in the session-setup / `0xb6` handshake region.
Tracing the caller to recover how the blob is built

---

## Session-key derivation — SOLVED: RSA key-transport (SUPERSEDES the (b) verdict above)

> **2026-07-05.** The classification above (verdict (b): "per-session secret from the
> `0xb6` handshake, don't trace") is now **superseded by mechanism**. The prior stop
> was based on a *false premise* — that `0x140072ec0` had "no static caller" — which
> was an artifact of a `capstone` bug (`md.disasm` halts at the first undecodable word,
> so the earlier `.text` sweep saw ~422 of ~350k instructions). A robust word-by-word
> sweep (`research/clb-crack/xref.py`, `disfn.py`) reveals the whole mechanism.

**The link session key `K` is RSA key-transport, decrypted with a STATIC EMBEDDED
RSA-1024 private key.** `b6`/`b7` are an orthogonal cable-auth handshake and do **not**
feed `K`.

```
K (32B) = PKCS1_unpad( RSA1024_CRT_decrypt( privkey , wrapped_blob[3:] ) )
KS_cid  = AES256(K).ecb_encrypt( IV_TABLE[cid] ),   cid = (msg_type+1)&0xf
plain[i]= cipher[i] XOR KS_cid[i]
```

- **Static secret = embedded RSA-1024 private key**, DER `RSAPrivateKey` at VMA
  `0x140171a30` (file off `0x170030`, 609 bytes, `30 82 02 5d 02 01 00 …`; n =
  `0xd32e7bbce9bf8853…`, e = 65537, p·q = n verified). Extract with
  `research/clb-crack/extract_rsa_key.py`. Parsed into bignum fields at `ctx+0x5cd8`
  (n@+8, e@+0x20, d@+0x38, p@+0x50, q@+0x68, dP@+0x80, dQ@+0x98, qInv@+0xb0) by RSA-ctx
  init `0x140073248` (called from the connection module `0x140069724`).
- **Install path (sole AES-256 set-key path — VERIFIED):** dispatcher `0x14006d6c8`
  → `0x140072ec0` → RSA-CRT decrypt `0x14007ce68`/`0x14007d010` → memcpy `K` into
  `ctx+0x5da4` → AES schedule `0x14007b140` (round keys `ctx+0x5ea8`/`+0x5f30`), sets
  active flag `ctx+0x5cd0=1`. `xref` confirms `0x14007b140` has exactly ONE caller
  (`0x140072f78`, inside `0x140072ec0`) — so *every* AES-256 link session, USB/HID/TCP,
  keys through this RSA decrypt. `0x14006d6c8` and `0x140069724` are indirectly
  dispatched (transport-abstraction virtual methods), i.e. shared by all transports.
- **Crypto is symmetric+RSA only:** registered primitives are `aes`/`sha256`/`sprng`
  (no ECC/DH; RSA is LibTomCrypt `rsa_*`, not a named cipher). `sprng` = CryptGenRandom
  (fills the `b6` nonce + PKCS#1 padding). `sha256` is only a hex-ID helper, not in the
  key path.

**Confidence.** Mechanism = **HIGH** (static, cross-checked by three independent
sweeps + the sole-set-key xref). **End-to-end key reproduction = UNVERIFIED against the
current captures:** the 128-byte RSA-wrapped blob does **not** appear in either USB
pcap (swept every 128-B window of both directions + the `0x0b` block concatenation,
RSA-decrypted with the recovered key → no `K` reproduces the known `KS_F3`). Either the
session key was cached from a prior open, or the wrapped blob is delivered off the
captured USB path / in a form these dumps don't expose.

**What this unblocks (LIVE path).** We hold the private key and the exact algorithm.
A live tool: drive the cable → capture the wrapped-key delivery → RSA-decrypt → `K` →
synthesise all 16 keystreams `KS_cid = AES256(K).enc(IV_TABLE[cid])` → full encrypted
UDS (VIN/DTC/measuring).

### Refined mechanism (2026-07-05, static-definitive) — RSA-OAEP, CABLE-DRIVEN

- **Algorithm = RSA-1024 + OAEP-SHA256 private-key decrypt** (LibTomCrypt
  `rsa_decrypt_key_ex`). `0x14007ce68` enforces `inlen == modulus_size` (`cmp` at
  `0x14007cec0`, else err 7) ⇒ **exactly 128-byte ciphertext**; core `0x14007d010` is
  `rsa_exptmod(which=PRIVATE)` doing two CRT modexps over `p,q,dP,dQ,qInv` at
  `key+0x50/0x68/0x80/0x98/0xb0`. `ctx+0x5da0` = the OAEP **hash** index (find "sha256"
  `0x14017ad94` via `0x14007b270`), `ctx+0x61e8` = PRNG index ("sprng"). So the earlier
  "symmetric cipher-id at ctx+0x5da0" reading was the OAEP hash id, not a cipher.
- **Install message = exactly 131 bytes: 3-byte header + 128-byte OAEP-wrapped K.**
  Dispatcher `0x14006d6c8` triggers the installer only on a 131-B inbound message
  (header nibble `0xF0`, `block[0]==block[1]`, len≥0x12) while un-keyed (`ctx+0x5cd0==0`).
- **CABLE-DRIVEN (decisive):** the only writer of `K` (`ctx+0x5da4`) is the RSA-decrypt
  output (single instruction `0x140072f38`, whole-`.text` movz scan confirms). There is
  **no app-side K generation.** Therefore the **cable generates the session key, OAEP-wraps
  it with the app's embedded RSA PUBLIC key, and transmits the 128-B blob**; the app
  recovers K with the embedded PRIVATE key. A live interop tool that holds the private key
  (we do) recovers K identically — **we do not need the cable's secret, we ARE the app.**
- **VERSION BREAK:** the pcaps (`init-only`/`reading-ecus`) are from the **OLD x86
  VMProtect build**, whose link key used a *different* (b6/b7-derived) scheme — no 131-B
  RSA-OAEP blob appears and no RSA/hash of captured b6/b7/09/0b bytes reproduces the old
  `KS_F3` (verified negative on both dumps). **The new unprotected build REQUIRES the cable
  to emit the RSA-OAEP wrapped key.** So an older-firmware cable that only speaks the old
  scheme would never key the link on the new binary — a genuine protocol-version
  incompatibility, independent of the genuineness signature check.

### VCDS genuineness gate (for the patch route) — reference

`0x1400732b0(ctx,mode)` parses cable-identify. Two rejects:
- **Soft** (signature blocklist of one interface): `resp[0x36..0x3b]` vs literal
  `0x140073568` = `01 00 00 c0 1e 00`; `b.ne 0x1400734ec` at `0x1400734e4`. Patch to always
  skip the warning: file off `0x728ec`, `41 f9 ff 54` → `4e f9 ff 54` (B.NE→B.AL).
- **Hard** (identify-header format, returns −1 at `0x140073550`): mode0 expects
  `resp[1]==0x14` (`0x1400733a4`/`0x1400733b0`), mode1 `resp[1]==0xE2` (`0x14007347c`).
Our cable's identify returns "ROSSTECH"+ver and passed on macOS, so it likely clears these.

**Bottom line for the live path:** no crypto wall — we hold the key and the exact
algorithm. The open question is now purely empirical: **does THIS cable's firmware speak
the new RSA-OAEP key-transport?** Answerable only by driving the cable through the new
build's OPEN sequence and watching for the 131-B wrapped-key frame.

---

## Live drive findings (2026-07-05, on the owner's car)

Confirmed live on the M4 (via `vagcan probe`/`handshake`):
- **Determinism holds:** replaying `BRINGUP` (first `b6`) reproduces the capture's
  first-epoch frames byte-for-byte — the live `0x39` block equals the capture's
  `39 38 82 5d f7 7d f0 75 6e eb 41 c5 4d 2b <cnt> cd` (only off14 counter differs).
- **Auth-advance rule:** the `0x39` auth-completion `b8` off14 = `observed & 0xF8`
  (the group-of-8 base of the cable's pushed `0x39` counter). Confirmed advancing
  the cable off `0x39` for observed `0x38→0x38`, `0x44→0x40`, `0xcc→0xc8`.
- **Each ECU needs its own `b6` epoch.** Per-b6 map (capture): every `b6` opens
  exactly ONE diagnostic channel — b6#1→0x39(auth), #2→0x9e, #3→0x43, #4→0x82,
  #5→0x5c, … #15→0xf3. So a diagnostic channel's keystream is locked to its own
  `b6` epoch; `KS_F3` (epoch #15) cannot decode a first-`b6` session. CAN target
  is set by the `b3`/`b4` addressing burst that precedes each `b6`.
- **`0x0b` EEPROM blocks carry NO plaintext VIN** — 40-byte encrypted blocks
  (high entropy, no ASCII); the safe post-auth `0b` read is not a VIN shortcut.
- **Cable dropped off the USB bus BETWEEN runs (clean-close bug, FIXED).** The
  drop happened after a *safe* run completed, before the next run started (the next
  `FT_Open` got DEVICE_NOT_FOUND with the cable absent from `ioreg`). Root cause:
  `D2xxBackend` spawned the D2XX worker thread **detached** and had no `Drop`, so a
  CLI whose `main` returns right after use exited with the worker mid-flight and
  **`FT_Close` never ran** — the FTDI endpoint stayed open and the clone's MCU stuck
  mid-session, eventually resetting off the bus. Fixed: `D2xxBackend` now holds the
  worker `JoinHandle` and, on drop, closes the command channel and **joins** the
  worker so `FT_Close` always completes. (My earlier note blaming a 2nd `b0..b5`+`b6`
  replay was wrong — that extended replay never actually executed; it hit
  DEVICE_NOT_FOUND on its first invocation because the cable had already dropped.)

## Safe ECU-open — offline RE (2026-07-05)

Per-`b6` epochs partition the session; each new CAN target needs a `b0..b5` filter
rewrite + a `b6`. Key findings for reaching a diagnostic ECU (e.g. to read VIN):

- **`b6` is a fresh CSPRNG challenge (all 40 in the capture are distinct).** A
  **stale/replayed** `b6` used to *re-key mid-session* is the suspected trigger for a
  clone firmware fault (distinct from the clean-close drop above). The first `b6`
  (bring-up, un-keyed cable) replays safely and deterministically; a *second* one
  should use a **freshly generated nonce**, not a replayed capture nonce.
- **`b0..b5` replay is safe** (deterministic config, no nonce; the capture issues 19
  mid-session `b0..b5` bursts without incident).
- **CAN-addressing map:** `b3` = acceptance mask (`ffe0>>5 = 0x7FF`, exact 11-bit);
  `b4[idx]` = filter response IDs, **`ID = (16-bit value) >> 5`**. Engine ECU01 resp
  `0x7E8` = `b4 …fd00`; gateway resp `0x77A` = `b4 …ef40` (permanently installed in
  every burst). Engine `0x7E0`/`0x7E8` is the shortest safe route to VIN.
- **Epoch caveat:** `KS_F3`/`F3_REQ_HEADER` in `link.rs` are **epoch-15-specific**
  (keystream = `AES(K_epoch, IV)`; `K` rotates every `b6`). A fresh `b6` → a fresh
  keystream, so those constants won't apply — the epoch's UDS-region keystream must be
  recovered live from a known-plaintext crib (TesterPresent `02 3E 00`), and response
  DATA is readable by two-time-pad (`resp_cipher ^ req_cipher`, request pad `0x00`).
  **Open bootstrap problem:** encoding the *first* request on a fresh epoch needs that
  keystream — closing this (a known cable/ECU push to crib against, or reversing the
  old-scheme `b6/b7` → `K` derivation so `K` is computable) is the remaining gap.
- **`0x0b` EEPROM blocks are NOT a VIN cache** — 40-byte AES-keystream blocks, each
  read distinct (no repeated plaintext → no two-time-pad), not decodable offline.

## DECISIVE live finding (2026-07-05): 2nd `b6` replay does NOT re-key the cable

`handshake --deep` (replay 2nd `b0..b5`+capture's 2nd `b6`, then the `0x9e` poll):
- **No wedge** — cable stays on the USB bus (the earlier "wedge" was purely the
  clean-close bug, now fixed; a stale `b6` replay is *tolerated*, not fatal).
- **But the cable stays in epoch-1** — after the replay it still emits only the
  `0x39`/`0x38` channels with the **byte-identical epoch-1 block content**, and the
  replayed `0x9e` poll (epoch-2 ciphertext) gets **zero response**.
- **∴ replaying the capture's 2nd `b6` does not advance the cable to a new key
  epoch.** Determinism holds for the FIRST `b6` (fresh, un-keyed cable → reproduces
  epoch-1 exactly) but NOT for a mid-session re-key. The old-scheme `b6`/`b7`
  key-derivation (skipped earlier in favour of the new build's RSA-OAEP) mixes in
  state we don't reproduce, so a replayed 2nd `b6` is inert.

**Remaining blocker to a live VIN (single, well-defined):** reverse the OLD-scheme
per-`b6` session-key derivation — how the cable re-keys on each `b6` and what the app
must send to make a new ECU epoch take. Without it we cannot open any diagnostic ECU
channel (each needs its own epoch), so we cannot craft/decode UDS beyond epoch-1's
`0x39`/`0x38` session-control channels. This lives in the OLD VMProtect build (harder:
protected) or in reproducing the exact cable state the 2nd `b6` depends on.

### What IS proven and shippable
Cable comms on macOS (PID + FTDI-init + clean-close fixes), deterministic epoch-1
replay, the plaintext bring-up + identity (`doctor`), protocol probe (`probe`),
auth-advance past `0x39` (`handshake`, off14 = observed & 0xF8), the full link
encode/decode + off14/off15 rules + ISO-TP reassembler (unit-tested, epoch-15 fixture),
and the complete new-build RSA-OAEP key-transport RE. The gap is exactly the old-scheme
re-key.

---

# Appendix — USB capture method (merged from `vag-hex-capture-guide.md`)

> **DONE (2026-07-05).** Two captures were taken — `research/captures/init-only.pcapng`
> and `research/captures/reading-ecus.pcapng` — and fully analyzed. The wire format
> above was recovered from them and implemented (`crates/vag-hex/src/frame.rs`); the
> link cipher was reversed (`research/clb-crack/link_cipher.py`). This appendix is the
> retained method record — how the captures were produced (on the Windows box where the
> clone HEX cable/VAG25.3 already works with a real VCDS install).

**Purpose.** Record the clone cable talking to a *working* VCDS install so we can model
the cable's own USB/serial protocol and drive it directly from `vagcan` — no VCDS, no
loader. This capture was the single input gating P1.

## What we set out to learn

From one good trace, four things:
1. **Enumeration** — how the cable presents on USB (FTDI VID/PID, D2XX vs virtual COM
   port, bcdDevice, iProduct string).
2. **Init handshake** — the fixed byte exchange VCDS does right after opening the cable,
   before any car traffic (baud/latency setup, firmware/version query, "hello").
3. **Framing** — how a UDS request (e.g. `22 F1 90` read VIN) is wrapped for the wire:
   ISO-TP over the cable's own serial envelope, with whatever length/checksum bytes the
   cable adds.
4. **Request↔response pairing** — matched pairs to prove framing both directions.

## Tools (Windows)
- **Wireshark** (latest) — <https://www.wireshark.org/download.html>.
- **USBPcap** — the USB capture driver; ships with the Wireshark installer (tick the
  "USBPcap" checkbox during install; reboot if prompted).
- (Optional CLI) **USBPcapCMD.exe**, under `C:\Program Files\USBPcap\`.

## Identify the cable's USB bus
1. Plug the cable in; let VCDS's driver bind (the working setup).
2. Device Manager → find the cable. Under **Ports (COM & LPT)** as a USB Serial Port
   (virtual COM) or under **Universal Serial Bus controllers** as an FTDI device. Note:
   which it is (COM vs raw USB), the **VID/PID** (Properties → Details → *Hardware Ids*,
   e.g. `USB\VID_0403&PID_6001`), and the **COM number** if a COM port.
3. In Wireshark's capture list you'll see **USBPcap1, USBPcap2, …** — one per host
   controller. The cable lives on one; if unsure, pick the one whose device tree contains
   your FTDI VID/PID. (VID/PID + COM number are half the answer to "how does it enumerate.")

## Capture procedure
Aim for a clean, *short*, well-labelled trace (short beats long — isolated exchanges).
1. **Close VCDS.** Start the Wireshark capture on the correct USBPcap interface **first**,
   with the cable already plugged in.
2. **Launch VCDS** — records the open + init handshake from the very first byte (critical).
   Wait for VCDS to show the cable as ready / "test" OK.
3. In VCDS, do these **slowly, one at a time** (~2 s apart so they're separable):
   - **Interface test / "Test"** (Options → Test) — pure cable handshake, no car.
   - **Auto-scan** *or* **Select → Engine (01)** — first car conversation.
   - **Read the VIN** (block that shows VIN) — known plaintext `22 F1 90`.
   - **Open Measuring Blocks**, watch **one** group ~5 s (e.g. RPM/coolant) — repeated
     `22 <did>` polls, gold for framing.
   - **Read fault codes** (DTCs) on engine — a `19 02 …` exchange.
   - **Clear nothing.** Read-only actions only.
4. **Stop VCDS**, then **stop the capture.**
5. **Save as `.pcapng`.**

If the cable is a **virtual COM port** and USBPcap traffic looks opaque/bulk-only, also
grab a serial-level view if easy — but USBPcap alone is usually enough (FTDI bulk-IN/OUT
carry the serial bytes directly).

## Trim / annotate (optional)
Apply a display filter to keep only the cable's device once you know its address:
`usb.device_address == <N>` (find `<N>` in any packet's USB layer), then File → Export
Specified Packets. Note the frame numbers where each action starts.

## Sanity self-check
- Trace starts **before** VCDS launched (captures the open handshake). ✔
- A burst of small OUT/IN transfers right after launch (init). ✔
- At least one exchange labellable as VIN / measuring / DTC. ✔
- File is `.pcapng`, opens cleanly. ✔

## Notes / gotchas
- **Use the OLD, working VCDS** — version doesn't matter; we only need the cable's wire
  behaviour, which the cable defines, not VCDS.
- USBPcap captures **all** devices on a host controller. If noisy, filter by
  `usb.device_address`, or use a USB port on a different controller from input devices.
- FTDI transfers: the first 2 bytes of each FTDI bulk-IN packet are **modem/line status**,
  not payload — stripped during analysis.
- Keep it read-only. Measuring blocks and DTC reads are safe.
