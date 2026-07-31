# `.rod` / `.clb` / `.lbl` labels + measurements — RE

Canonical writeup of the VCDS label/measurement corpus: the `.rod` TEA-CBC crypto, the
per-record `product`/IV blocker, `STRUC.rod`, `TTTEXT`, and what it takes to decode a UDS
measurement into a human value. **Merges** `rod-measurement-feasibility.md`,
`rod-product-term-dump.md`, and `rod-measurement-decode.md`.

Data lives under `research/vcds-data/` (gitignored symlinks to the owner's VCDS install:
`STRUC.rod` + `TTTEXT.ROD` + the engine `EV_ECM18TFS0208V0906264H.rod`) and
`research/clb-crack/samples/` — never committed. Tooling in `research/clb-crack/`.

---

## 0. CURRENT STATE (supersedes the earlier NO-GO)

To turn a raw UDS measurement into a human value we need three things: **(1)** the DID /
identifier the ECU is read with, **(2)** the COMPU-METHOD scaling (raw bytes → engineering
value + unit), **(3)** the human name (via a `TTTEXT` join).

- The `.rod` **TEA-CBC is cracked**; the per-record `product`/IV blocker is now **DEFEATED
  OFFLINE, no runtime dump** — a DEFLATE dynamic-Huffman-header oracle + Kraft pruning +
  full-inflate confirmation recovers the 5 IV bytes (~1 min, multithreaded Rust cracker
  `research/clb-crack/rod_crack/`; python ref `rod_crack_prep.py`). `STRUC.rod` **fully
  inflates** (293,560 bytes, valid Adler-32); the same method unblocks `TTTEXT.ROD` and all
  engine `MWB`/`INC`/`DTC`. **This refutes the prior "needs a runtime dump" conclusion for the
  crypto layer.**
- **Verdict: PARTIAL** (upgraded from NO-GO). What remains is pure data-format RE (no crypto,
  no dump on the critical path): the decoded `STRUC` payload is a **packed 14-glyph codec**
  (`NNNNNN,<encoded>`, 1221 structure-ids), not delimited `DID,factor,offset`. Extracting
  per-DID scaling/unit needs reversing the STRUC-parser fn (binary `+0x33758` region) and the
  engine-MWB 2-char-code → STRUC-id mapping. Names via TTTEXT are decrypt-only (crackable, a
  nicety).

Confidence: **HIGH** for the crypto crack (reproducible, exact inflate + Adler-32); **HIGH**
that `STRUC` carries the structure data; the unfinished part (the field codec) is characterised
honestly in §3.

---

## 1. The crypto crack — how the `product`/IV blocker falls offline

### 1.1 The blocker (verified against the binary)
Every encrypted `.rod` section is TEA-CBC / `KEY_ROD` with a section-tag IV. The 6-byte section
header is two BE24 ints: `read1` (bit `0x800000` = *uncompressed* flag, low 23 bits = stored
cipher length) and `read2` (plaintext / decompressed length). Flag-clear sections are
additionally zlib-DEFLATE'd. **`IV[0:3]` is derived from the section tag → always exact.**
`IV[3:8]` carries the low 5 bytes of a runtime `product` term. Because CBC makes blocks 2..n
independent of the IV, **only the first 8 plaintext bytes depend on `IV[3:8]`**. For a zlib
section the 2-byte header `78 da` stays intact but **deflate bytes 1..5 are corrupted** — which
kills the whole inflate.

Disassembly of the raw-`.rod` decrypt fn (`VCDS-arm64-unpacked.exe`, ImageBase `0x140000000`,
fn `+0x33758`) confirms the derivation:
- `+0x33910..+0x3399c`: `product = ∏ (signed) chars of a record-string` (a `mul` accumulator in
  `x25`); its low 5 bytes are stored into the IV seed at `buf+3..buf+7` (`+0x339a0` byte-extract).
- The record-string is built from a **runtime singleton** (`+0x337a0`, object at
  `singleton+0x18`), a constant path fragment (`".\UDS_EV\"` @ `0x1701_00d8`), then **truncated
  at the first `.`** (`+0x338a0`, char `0x2e`). It is a runtime buffer — **not any field of the
  file** — so `product` cannot be *computed* offline. Prior work stopped here and prescribed a
  memory dump.

The full IV construction (`crates/vag-data/src/rod.rs::rod_block0_iv` ≡
`research/clb-crack/decrypt_modern.py::rod_block0_iv`, identical logic):
```
seed = tag[0:3]  ||  product_bytes[0:5]                 (8 bytes)
s[i]  = (seed[i] + KS[(m*(i+2)) & 0xff]) & 0xff          m = tag[1]
IV[i] = (s[i] * MT[OFF_ROD[i]]) & 0xff
        OFF_ROD = [0x07,0xca,0x22,0x99,0x3e,0x88,0xc3,0x76]
product_bytes = (product & (2^40 - 1)).to_bytes(5, "little")
product       = ∏ chars of a runtime "record-string"    (64-bit)
```
Each `IV[i]` is an **independent** per-byte transform of one seed byte; the unknown is exactly
the 5-byte `product_bytes` (equivalently `product` mod 2^40), and it is **per-section** (one
`[TXT]` section ⇒ one `product`).

### 1.2 The insight — you don't need `product`, only 5 plaintext bytes
`IV[3:8]` is 5 bytes; the corrupted plaintext is exactly `deflate[1..5]`. Two facts make brute
force tractable (where a blind `2^40` full-inflate search was infeasible):
1. **Candidate space is ~2³⁶, not 2⁴⁰.** Each `IV[i] = (s · MT[OFF[i]]) & 0xff`. For `i = 3,5`
   the multiplier `MT[OFF[i]]` is **even** (`250`, `152`), so those IV bytes take only
   `256/gcd = 128` and `32` distinct values. Effective space =
   `128·256·32·256·256 = 2³⁶`.
2. **The DEFLATE dynamic-Huffman header is a strong, cheap oracle.** `deflate[0]` is exact and
   pins `BFINAL/BTYPE=dynamic/HLIT`. The 5 unknown bytes sit inside the header (HDIST, HCLEN,
   the code-length-code lengths, and the start of the HLIT+HDIST code-length list). A candidate
   is rejected the instant the code-length codes violate the **Kraft inequality** (checked
   *incrementally*), and again when the HLIT/HDIST code lengths — decoded against the **exact
   tail bytes** — over/under-subscribe. Only a true 5-tuple survives to a full inflate.

### 1.3 Tooling & result
- `research/clb-crack/rod_struc_decode.py` — pure-Python reference: DFS over the 5 bytes with
  incremental-Kraft pruning (`--selftest` validates the deflate parser against the known-good
  VW48 `MWB` section). Correct but slow (~120k nodes/s).
- `research/clb-crack/rod_crack/` — standalone multithreaded **Rust** brute-forcer (isolated:
  own empty `[workspace]`, not a workspace member). Header oracle + `miniz_oxide` inflate
  confirmation across all cores.
- `research/clb-crack/rod_crack_prep.py` — preps `crack_input.bin` for a given file+tag and
  re-inflates with a recovered 5-byte guess.

**Proof (STRUC.rod, `[STRUC]` section):**
```
plainlen = 293560, d0 = 0x8c, candidate set sizes = [128,256,32,256,256]
RECOVERED plaintext[3:8] = 9d 69 92 24 29   (=> IV[3:8] = ac 32 38 71 e1)
inflated 293560 bytes  (exact; valid zlib Adler-32)
```
Rust cracked it in ~1 min on 10 cores (STRUC's answer sat ~3% into the space, ~2.2·10⁹ checks).
The recovered IV inverts to seed/product bytes consistent with the `∏`-of-chars model — but we
**never needed the runtime string**. `TTTEXT.ROD [TXT]` (7.46 MB inflate) is the same section
type/blocker and is crackable the same way (a full sweep of 2³⁶ is minutes-scale; porting the
Python DFS pruning into Rust would make it fast regardless of where the answer lands).

---

## 2. `STRUC.rod` layout

- Container: a **single** `[STRUC] … [/STRUC]` ASCII-tagged section (tag is **5 letters** — note
  `rod.rs::find_next_tag` currently caps tags at 4 letters, so it must be widened to accept
  `STRUC`; see §5).
- Header: the standard `.rod` 6-byte header — two BE24 ints. `read1` = `0x800000` flag clear |
  23-bit stored cipher length = 78,744; `read2` = decompressed length = 293,560. TEA-CBC/
  `KEY_ROD`, tag IV, then zlib.
- Decoded content: **8,854 records**, one per line `NNNNNN,<payload>\r\n`. Structure-id `NNNNNN`
  runs `000001…001623` (**1,221 distinct ids**), 1–11 rows per id (mostly 1–3; a mode at 8).
  This id namespace is **small** and distinct from the 6-digit *text*-ids seen in `MWB` (which
  range up to ~152,526 and index `TTTEXT`).
- Payload: strings over a **14-symbol alphabet** `[0-9,._-]` (globally near-uniform ~7% each).
  **The codec is now IDENTIFIED (disasm, `rizin`): it is base-14.** VCDS carries the literal
  charset `"0123456789,.-_"` at `0x1401898b0` in `VCDS-arm64-unpacked.exe`, consumed by the
  radix-conversion routine `fcn.1400e6f80` (which does `msub …, #0xe` = arithmetic **mod 14**
  against that charset; a sibling path uses the `a-z` base-26 charset at `0x140189890`). Symbol
  values: `'0'..'9'`→0..9, `','`→10, `'.'`→11, `'-'`→12, `'_'`→13. The alphabet string is
  referenced from the record-fetch `fcn.1400e1400`, itself called by the comma-record parser
  `fcn.1400276f8` (splits `NNNNNN,payload,…` and `sscanf`s each field). So the punctuation are
  base-14 **digits**, not separators — which is why the earlier "digit groups → non-sane
  magnitudes" reading failed. **Corroboration:** decoding a payload as one big-endian base-14
  bignum yields, across the multiple rows of a single structure id, a **shared high-order prefix**
  (the structure template) + a varying low-order tail (the per-channel field) — e.g. id 000147's
  8 rows all begin `08 56 26 27 d2 03…`. Shipped as `vag_data::struc::{STRUC_BASE14_ALPHABET,
  base14_value, StrucRecord::decode_base14_be}`.
- **Still open (honest):** the **field segmentation** of that base-14 number — where each field
  begins/ends and which is the read identifier (DID) / raw byte spec / scaling / unit ref / name
  ref — is **not yet reversed**. So `decode_base14_be` returns the faithful packed value, not
  decoded measurement fields; no validated `(DID, scale, unit)` row exists yet.
- **Segmentation hypotheses TESTED against the owner's real bytes and REFUTED** (documented so
  they are not re-tried): (a) *fixed character-column fields with a per-channel index run* — of 62
  equal-length structures with ≥3 rows, only **1** has a contiguous-range varying run (id 000005
  `col7 = {2,3,4,5}`, almost certainly coincidence); (b) *per-byte index* — the varying bytes of a
  structure (e.g. id 000147's 6 low bytes `ee.., cd.., f5.., df..`) are high-entropy, not a small
  counter; (c) *decimal-digit-group index* — base-14 → decimal shares a high-order prefix but the
  differing low decimal digits (~15 of them) are unstructured, not a clean field. The varying
  per-channel field is therefore **high-entropy** (consistent with a name/TTTEXT pointer or an
  encoded value, NOT a plain index). Do not invent a fixed layout on top of these.
- **Disasm dead-end noted:** `fcn.1400e1400` (record-fetch, references the base-14 charset) turned
  out to be a **generic number↔base-N/base-26 formatter** (its "descriptor tables" at `0x1405e8720`
  are zeroed BSS runtime state), not the per-field STRUC slicer. The routine that decodes a payload
  string into structure fields (string→value, i.e. a `×14`/charset-position loop, the inverse of
  the `msub #0xe` encoder) has not been cleanly isolated. Next step remains reversing that consumer,
  or a ground-truth cross-reference via a cracked engine `MWB` (blocked on the also-unproven 2-char
  code → STRUC-id mapping — two unknowns, so not yet a validation path).
- Sections decoding cleanly vs blocked, for `STRUC.rod`: **1 / 1** (the sole section, cracked).
  No residual `product`-blocked sections remain in this file.

---

## 3. Engine `.rod` → STRUC → TTTEXT join — what resolves, what doesn't

**Owner's exact engine** `EV_ECM18TFS0208V0906264H.rod` (`8V0 906 264 H`, `J623-CJSA` 1.8 TFSI)
decodes to:

| tag | kind | status | content (format-level) |
|---|---|---|---|
| CMP | tea  | OK (product 0) | one ident row: `627023,<part/HW/SW tokens>` |
| SLV | tea  | OK (product 0) | one row `027305,…` |
| INC | zlib | **crackable** (product≠0) | not yet cracked (small ⇒ weak oracle, slower) |
| DTC | zlib | **crackable** (product≠0) | fault-code table |

The owner's `-H` file carries **no `MWB` section**; the measurement list is in sibling variant
files (e.g. `EV_ECM18TFS0208V0906264A.rod` has a `MWB`, also `product`-blocked and crackable).
`MWB` rows are `<6-digit text-id>,<2-char code>` where the text-id indexes `TTTEXT` (the *name*)
and the 2-char code (over a ~40-symbol alphabet `[0-9A-Z._,-]`, 40²≈1600 ≈ the STRUC id ceiling)
is the plausible **reference into `STRUC`** for the structure/scaling — this linkage is
consistent but **not yet decoded** (same field-codec gap as §2).

**The chain, honestly:**
- `engine MWB row` → (text-id → `TTTEXT` name) is a **decrypt-only** join (works once `TTTEXT` is
  cracked — §1 method).
- `engine MWB row` → (code → `STRUC` id → DID + scaling) is **decrypt-OK but the STRUC field
  codec is unresolved**, so the numeric scaling cannot yet be read out.

**Is `product` on the VALUE critical path?** No longer for *decryptability* — it is fully
brute-forced offline. It is on the path only in that every interesting section (`STRUC`,
`TTTEXT`, engine `MWB`) is `product`-blocked and must be cracked first. After cracking, the
remaining obstacle is a **plaintext format** problem, not crypto and not a dump.

### 3.1 The table graph + the `MWB` 2-char code (base-40 REFUTED)

The UDS_EV corpus is a graph of base-14-packed tables, each keyed by a decimal id:

| file/section | ids | rows | role (ODX) |
|---|---|---|---|
| `STRUC.rod [STRUC]` | 1–1623 (1221 distinct) | 8,853 | structure / byte layout |
| `TTDOP.rod [DOP]` (cracked: IV `9d59b2e52a` → **2,722,454 B**) | 1–28,932 (17,636 distinct) | 127,433 | **DOP** = COMPU-METHOD (scaling/unit); rows are COMPU-SCALE-shaped, e.g. id 1 `5_5_3.33,_`, `2_2_-3-5._`, … (texttable/enum) |
| `TTTEXT.ROD [TXT]` | up to ~152,526 | — | name strings |
| engine `MWB` | text-id up to ~152,526 | ~200 | measurement list: `(text-id→TTTEXT name, 2-char code)` |

`TTDOP` vs `MUX.rod` are loaded by `fcn.140028e28` (selects the path by arg1; the `#0x28`/`#0x24`
= 40/36 there are **struct-field offsets, not a base-40 radix**).

**`MWB` 2-char code → id: base-40 is REFUTED (not just unproven).** The code's character **set** is
exactly the 40 symbols `0-9 A-Z , . - _` (proven), and base-40's ceiling 40²=1600 matches STRUC's
1623 — but:
- base-40 `code → STRUC-id` lands in-range only **~3σ above chance** (best ≈188/221 vs ≈168 expected
  by the 0.75 STRUC-id density, across *every* alphabet order / endianness / offset); a real key
  would be ~100%.
- The mapped STRUC record does **not** echo the row's text-id (name pointer): **~0/180**.
- Range rules out `code → DOP-id` entirely (DOP ids reach 28,932 ≫ 1600).
- **No base-40 charset or `mod-40` arithmetic exists in the binary** for this purpose.
So the `code → table-id` mapping is a **lookup or non-trivial scheme, not base-40 arithmetic**; it
stays unproven. Shipped only the proven `MWB` row parse (`vag_data::mwb::{parse_mwb, MwbEntry,
MWB_CODE_SYMBOLS}`) — **no** invented `code → id` function.

**Net:** all four tables are located, decrypted, and inflated offline, and confirmed to share the
base-14 codec; what blocks a validated `(name, DID, scale, unit)` row is (a) the unproven
`code → id` link and (b) the unreversed base-14 **field segmentation** (§2) — two independent
data-format unknowns, no crypto and no dump on the path.

---

## 4. The three MVP measurements (RPM / vehicle speed / boost)

### 4.0 Capture lever (runtime crib) — TRIED, EXHAUSTED for scaling

Mined `research/captures/reading-ecus.pcapng` for a runtime `(DID → raw → value)` crib to
sidestep STRUC/DOP (tool: `research/clb-crack/extract_uds.py`, built on the recovered `b8`/`b7`
link cipher — `link_cipher.py`/`usbpcap.py`). Decoded fully: **763 request / 457 response** blocks
over **66 channels**. Classification:
- **43** channels are `TesterPresent` keep-alives (no data).
- **1** channel (`f3…44dd…5f`) carries both TP and RDBI → fully decodable: engine measurement
  **DID `0x7458`**, response `62 74 58 55` = a **static** value (the car was engine-OFF).
- **22** non-TP channels are identity/security reads. Cross-checked to the Auto-Scan ground truth:
  the **VIN** read (`eb…60…39…c9`) yields `XW8…NE9J…8917` = `XW8AD4NE9JH008917` ✓, and the gearbox
  channels (`b3…eb0d…55`, `eb…40…39…c9`) carry SW-version **`10 03` = "1003"** ✓ (DQ200 `1003`).
  These **confirm the decoder** but are identity, not scaling.

**Verdict:** this capture is an engine-OFF **identification scan**. There is **no varying
measurement data** (the one decodable measurement DID is static) and **no ordered measurement-read
sequence** to align to the engine `MWB` list — so neither Priority-1 (empirical scaling) nor
Priority-2 (`code ↔ DID`/STRUC crib) is obtainable from it. **The offline lever is exhausted;** a
fresh **live-car capture** (engine running, VCDS polling measuring blocks) is required to observe
`(DID → raw → engineering value)` for RPM/coolant/boost.

### 4.0a Engine-running capture — DONE, PARTIAL (one proven point; RPM/speed NOT fitted)

The live capture above was taken (`research/dumps/capture-w-logs.pcapng`, 318 s, engine running,
+ VCDS ADVMB logs `logs-engine.CSV` / `logs-dsg.CSV`; **all gitignored, never committed**) and
mined end-to-end: decode the link cipher → per-DID raw time-series → align to the logged
engineering values by curve shape → least-squares fit. Tooling:
`research/clb-crack/measure_{series,ttp,final}.py` (the last brute-forces every DID×interpretation×
CSV-measurement fit); `usbpcap.py` gained per-frame timestamps (`with_time`, a `t` field).

Decoded, per channel (link keystream verified — response padding decrypts to a constant `0xffff`):
- **Engine ECU** (2 TP-crib channels `7434e9c3`, `d454d17f`): 8 single-frame RDBI DIDs
  `7458, 82D4, A03B, A051, A058, A059, A05E, A05F`, ~575 responses.
- **DSG-window channel** `d75061e7` (no TP crib): 3 RDBI DIDs recovered by a **request-padding
  two-time-pad** — RDBI request data bytes are `0x00`, so `data = resp ⊕ modal_req` at the data
  offsets, and clustering responses by the echoed-DID cipher `(off8,off9)` separates the DIDs
  without ever knowing `ks[8..9]`.

**PROVEN (shipped, `vag_data::measure`):** the **ignition-angle zero point** — DIDs
`A058/A059/A05E/A05F` each return raw `0x5555` (BE `u16`) for a displayed **0.00°**, cross-validated
four ways against the four constant ignition-angle channels VCDS logged (`IDE00155/156/157/158`,
constant `0.00°`). Fixes the COMPU offset.

**NOT fitted (no forced fits):**
- *Ignition slope* — the one varying ignition DID `A051` ↔ `IDE00149` shape-matches only loosely
  (`|r|≈0.86`, non-monotonic/bimodal raw→°, `R²≈0.73`); no clean `(factor,offset)`.
- *RPM & vehicle speed* — **no DID tracks either with a proof-grade fit.** The engine-log session's
  RPM/speed excursions are small and clock-jittered (best `R²≈0.6–0.7`); the DSG channel that spans
  the 682→3800 rev does **not** linearly track RPM (its clusters give `R²≤0.64`, implausible negative
  slopes; a `raw·0.25`-scaled RPM would need raw up to `0x3B60`, never observed). The DSG log's
  RPM/speed are tagged `-ENG#####` (engine-sourced, gateway-mirrored) — their DID is not cleanly
  isolated on the captured DSG channel. A capture of ONE ECU polled through a wide, sustained rev
  with a tight-cadence log would settle RPM/speed.

This establishes the crib direction `G### group / IDE-id ↔ DID` for the engine ignition block
(`IDE00155/156/157/158 ↔ {A058,A059,A05E,A05F}` as a set), a foothold toward the MWB code→id map.

### 4.0b Wide-rev single-ECU capture — DONE, the "cleaner capture" hypothesis REFUTED

The §4.0a negative prescribed exactly this capture: **one ECU polled through a wide, sustained rev
with a tight-cadence log**. It was taken — `research/dumps/coolant-rpm-speed.{pcapng,CSV}` (135 s USB
trace + 77 s VCDS ADVMB log, **single ECU Engine 01 `8V0 906 264 H`**, gitignored, never committed) —
and it delivers the wide rev: `IDE00405` (RPM) spans **784 → 3807 /min** (a genuine double-rev), speed
`IDE00075` 0 → 14 km/h (a clean drive-away), coolant `IDE00025` 99 → 104 °C. Tooling:
`research/clb-crack/measure_{coolant,fit,overlay,channels,probe}.py`.

Decode (channel census `measure_channels.py`): the **two TP-crib channels carry exactly 7 frequently
polled single-frame RDBI DIDs** `{7410,7419,7444,7450,7458,A03B,A0EF}` (~62–65 samples each) plus
`82D4` (3 samples); **zero multi-frame responses; the 10 non-TP channels hold ≤64 frames and no
recoverable DID cluster.** So these 7 DIDs are the entire ADVMB read set, matching the 7 logged IDE
measurements 1:1 by count — but **not by value**:

- **RPM is absent.** All 7 channels share one clock; the true capture→log lag is **≈ 52 s** (pinned
  independently by the drive-away window: speed nonzero `t_csv∈[41,72]` ↔ `7458` active
  `t_cap∈[91,123]`). At that single lag, RPM (`IDE00405`) correlates with **nothing** — `|r| < 0.5`
  for every DID×{u8,u16be/le,i16be}. No DID's raw reaches the RPM band under any standard scaling
  (`raw·0.25`→`0x0C40..0x3B7C`, `raw·0.5`, `raw`, `raw·2` all fall outside the observed high bytes).
  The high per-pair `|r|` (e.g. `A0EF u16be r=-0.96`) occur only at *scattered, per-measurement* lags
  (RPM's best fits land at lags 34.5 / 40.5 / 68 / 72 / 84 / 88 / 89.5 s — never the true 52 s) and on
  **near-constant** DIDs — the textbook signature of spurious window-fishing plus overfit, not tracking.
- **The 2-byte DIDs are angle-family, not RPM/speed.** `A03B` decodes `56 4x` (hi byte pinned `0x56`),
  `A0EF` decodes `55 4x` (hi byte pinned `0x55`), `7458` idles at `0x55` and swings ± — all in the
  proven ignition-angle **`0x5555`** band, i.e. engine-internal signed angle/throttle signals. Fitting
  `7458→speed` needs **negative km/h** (`7458` dips below its `0x55` idle mid-drive); fitting
  `A0EF→throttle-%` needs a **negative slope** and covers only the idle band — both rejected.
- **Coolant is absent too.** `IDE00025` rises slowly 99 → 104 °C; the only slowly-drifting DID `7450`
  *falls* `0xDE(222) → 0xC5(197)` and anti-correlates (`r ≈ −0.66`); `raw·0.75−48` maps it to 118 → 99 °C
  (wrong direction/magnitude). `7450` is a different, cooling temperature — not the logged coolant.

**Conclusion (refines §4.0a):** a wide rev range is *not* the missing ingredient. VCDS's ADVMB display
values are computed from raw that the decodable RDBI channels **do not expose** — the polled DIDs on this
ECU are developer/angle/throttle-internal quantities, while RPM/speed/coolant reach the display via
group reads (`G004/G006/G009/G052/G067/G096/G138` in the CSV header) whose value-carrying traffic is not
recoverable here. **No new `(DID, factor, offset)` proves out; none is shipped** (guardrail: no forced
fits). Net crib gain: the poll-set membership `{7410,7419,7444,7450,7458,A03B,A0EF} ↔ {IDE00025,75,83,
349,405,583,1377}` (unordered) and the confirmation that `A03B/A0EF/7458` are additional `0x5555`-band
angle-family DIDs.

### 4.0c Supervised STRUC × crib attack — DONE, DID-in-STRUC REFUTED (the big negative)

The M3 lever (`todo/README.md` §M3): cross the decoded `STRUC` table with the capture crib's **real
valid DIDs** to locate the `read_id` field. Ran end-to-end; the result is a clean, multiply-confirmed
**negative** — the read DID is **not stored in `STRUC` at all**:

- **DID as u16 (BE/LE) at any byte offset of the base-14-decoded record — REFUTED.** Searched all 8,853
  records × every offset for each of the 13 crib DIDs (`7410,7419,7444,7450,7458,82D4,A03B,A051,A058,
  A059,A05E,A05F,A0EF`). No `(form,offset)` accumulates more than **5** hits across all 13 DIDs — pure
  coincidence level for ~8.8k records × ~12 bytes. No `read_id` column exists.
- **DID as a base-14 field (4- or 5-char window) at any char offset — REFUTED.** Same non-clustering
  (≤5 hits at any offset). So the DID is not a packed base-14 sub-field either.
- **`STRUC-id == IDE-measurement-id` — REFUTED.** VCDS's ADVMB `Loc. IDExxxxx` ids for the crib
  channels include `IDE00149` and `IDE00158` (ignition-regulation and ignition-cyl-4), but `STRUC.rod`
  has **zero records at ids 149 or 158**. So the STRUC id space is not the IDE measurement space, and
  reading STRUC record `N` for `IDE00N` yields the wrong/empty record.
- **`IDE-id == engine-MWB row index` — REFUTED.** The owner-family engine `MWB`
  (`EV_ECM18TFS0208V0906264A.rod`, cracked here: IV `5a478e243d`, 11,979 B, **1,089 rows**, all
  `<6-digit text-id>,<2-char code>`) has only 1,089 rows, but `IDE01377` is a logged measurement —
  out of range. IDE is a global measurement id, not a per-ECU list position.
- **DID as a literal decimal string anywhere (STRUC/`TTDOP`/`MWB`) — REFUTED.** The few substring hits
  in the 2.7 MB `TTDOP` blob are chance-level.

**What this fixes for the roadmap:** the §2/§3/§A.2 premise "the read identifier lives in `STRUC.ROD`"
is now **falsified against ground truth**. Every measurement section across the corpus — `MWB`, `GES`,
`SOT`, `XPL`, `ADP`, and `DOP` — is uniformly `<id>,<2-char-or-token code>` (confirmed on the
product-0 reference `EV_EPHBO18…VW48` and the cracked owner `MWB`); **no section carries a 16-bit DID as
text or as a locatable packed field.** So `read_id` is *not* recoverable from the decoded label bytes at
the STRUC/DOP/MWB layer as currently understood. The remaining place it could hide is the still-unproven
`code → table-id` **lookup** (not arithmetic — base-40 already refuted, §3.1) resolving into a record
whose field codec is also unreversed — i.e. **two stacked unknowns**, and the crib does not collapse
either. The name join (`text-id → TTTEXT`) that would let the crib label individual MWB rows needs
`TTTEXT.ROD [TXT]` cracked (mechanical, §1; a full 2³⁶ sweep — left running, not on the critical path
for this negative).

**Shipped from this pass (`vag-data`):** the honest architectural foundation, seeded only with
crib-proven data — a `catalog::MeasurementDef { name, unit, address: ReadId::Uds(did), raw_form,
scaling }` type with a `Scaling::{Linear, Anchor}` enum (so a *partially* reversed measurement is
representable without inventing the slope), plus `catalog::IGNITION_ANGLE` = the four proven DIDs
(`A058/A059/A05E/A05F`, `U16Be`, `0x5555 → 0.00°`). Tests assert the zero point and that the crib DIDs
do **not** appear in the real ignition-family STRUC records (regression-locking the negative). No STRUC
field layout is invented.

### 4.1 Static-analysis status

**Not yet provable as scaling formulas.** Getting `identifier + factor/offset + unit + sample
conversion` requires reading them out of the `STRUC` payload (and mapping via the engine `MWB`
code), which is blocked on the §2/§3 field codec, not on crypto. The pieces we can now place
offline:

| field | offline status |
|---|---|
| decrypt of STRUC / TTTEXT / engine-MWB | **YES** — brute-forced, exact (§1) |
| measurement **name** (RPM / speed / boost) | **YES in principle** — text-id → `TTTEXT` join, both decryptable offline; not yet dumped (nicety) |
| **DID / read identifier** | present in `STRUC`/`INC` but behind the field codec — **not yet extracted** |
| **scaling (factor/offset/formula)** | present in `STRUC` payload but **encoded** — **not yet extracted** |
| **unit** | ditto (a `STRUC`/`TTTEXT` reference) — **not yet extracted** |

So the honest answer for RPM/speed/boost: **the data is offline and decrypted, but the
raw→engineering conversion is still encoded in the STRUC field format.** Cross-referencing
units/names against the engine `.clb` is moot for scaling — `06K-907-425-V1/V2.clb` decode
cleanly (the clb IV is fully solved) but are **long-coding (LC) label** files (coding helpers),
not measurement scaling. `Scaling/OBD.SCL` is ASCII but only **generic OBD-II PID display
ranges** (`pid,min,max`), and the `*.a01` files are auto-scan **group presets** — neither
carries per-DID conversion constants.

---

## 4.2 The control unit names its own label file (`F19E`) — solved, live

Selecting *which* `.rod` describes an ECU was previously a part-number guess. It is not a
guess: the unit answers it. Read on the reference car 2026-08-01 over CAN:

| ECU | `F19E` value | file |
|---|---|---|
| Engine 01 (`8V0906264H`) | `EV_ECM18TFS0208V0906264H` | `EV_ECM18TFS0208V0906264H.rod` |
| Gearbox 02 (`0CW300041G`) | `EV_TCMDQ200021` | `EV_TCMDQ200021.rod` |

`F19E` is the standard ASAM ODX file identifier, and its value is exactly the `.rod` file
stem. Shipped as `vag_data::find_rod_by_odx_name` plus
`vagcan labels <dir> --from-car --ecu 01`, which reads the identifier off the car and
resolves it against the corpus in one step. This removes the last piece of guesswork from
the label path — it does **not** touch the STRUC field-codec problem, which is still what
blocks reading measurement *values*.

## 4.3 The live crib WORKS — first proven scalings (2026-08-01)

The parallel-adapter session finally happened: `vagcan sniff` recorded the OBD-II bus in
listen-only mode for 308 s while VCDS ran a normal measuring-block session on the same bus,
logging to CSV. `vagcan analyse` crossed the two. **Three scalings proved out**, each an
exact linear relation (`R² = 1.00000`), not a correlation:

| ECU | DID | form | scaling | measurement | points |
|---|---|---|---|---|---|
| Engine 01 | `F405` | `u8` | `raw − 40` | coolant temperature `°C` (IDE00025) | 16 |
| Engine 01 | `206E` | `u16` BE | `raw` | engine speed `/min` (IDE00405) | 16 |
| Gearbox 02 | `380A` | `u16` **LE** | `raw` | transmission input speed `/min` (IDE00022) | 14 |

**Why this is trustworthy:**
- The coolant fit produced `raw − 40`, which is exactly the **standard OBD-II PID 05
  formula**. Nothing in the pipeline knows that formula; recovering it from the data
  validates the clock alignment and the fitting end to end. `F4xx` is the UDS mirror of
  OBD PID `xx`, so this row was independently checkable — and it checked out.
- The gearbox row was verified **byte by byte** against the log rather than trusted:
  `B2 02` = 690 /min, `D7 02` = 727, `CC 08` = 2252, `7A 0E` = 3706 — matching the logged
  values exactly. Big-endian would read 45570 and 52232, so the little-endian reading is
  established, not assumed.
- Alignment was arithmetic: capture anchor 01:17:21, engine log header 01:19:39, offset
  `+138.0 s`. Nothing was slid against anything.

**A false positive was caught and closed.** The first run also "proved" `200C` as an
ignition angle with factor `−0.008824` at `R² = 1.0` — off **two distinct raw values**. Two
points define a line exactly, so a perfect fit there is arithmetic, not evidence. `analyse`
now requires a minimum number of distinct raw levels (default 4) and the row disappears.
This is the §4.0a/§4.0b failure mode reproducing itself, caught by a guard this time.

**What this does NOT settle:** the logs were short (19.7 s engine, ~25 s gearbox), giving
14–16 matched points against a default threshold of 20 — the runs above used `--min-points
10`. The relations are exact, so the evidence is strong, but a longer simultaneous log
would remove the caveat entirely. The remaining engine identifiers polled during the
session (`200A`, `200B`, `200D`, `2029`, `202A`, `293E`, `293F`) were either constant in
the overlap window or not covered by the logged measurements.

**Consequence for the roadmap:** scaling no longer depends on the `.rod` field codec at
all. The STRUC segmentation problem (§2–§3) stays unsolved and is now off the critical
path for *values*; the corpus is still what supplies names and per-ECU measurement lists.

## 5. Design proposal — how `vag-data` should consume this (TEXT ONLY)

*(No crate is modified in this research; another workflow owns `crates/`.)*

1. **Widen the `.rod` framing scanner.** `rod.rs::find_next_tag` accepts 2–4 uppercase tag
   letters; `STRUC` is 5. Widen to 2–5 (or 2–8) so `[STRUC]` is recognised. `find_close`/
   `decode_section` already generalise.
2. **Add a `product`/IV recovery step for zlib sections.** Give `rod_block0_iv` an explicit
   `iv3to8: Option<[u8;5]>` (or accept a full IV). When absent and a zlib section fails to
   inflate, run the brute-forcer (port the Rust header-oracle + Kraft-pruned DFS; feature-gated
   / offline-tool crate) to recover the 5 bytes, then inflate. Cache recovered IVs per
   (file, tag).
3. **New types (once the STRUC field codec is reversed):**
   ```rust
   struct MeasurementDef {
       struct_id: u16,          // STRUC record id
       read_id: ReadId,         // UDS DID / RecordLocalId + request bytes
       raw: RawSpec,            // type + byte length + endianness
       scale: Compu,            // Linear{factor,offset} | Rational | Table | Formula
       unit: Option<String>,    // via STRUC/TTTEXT reference
       name: Option<String>,    // via text-id → TTTEXT
   }
   ```
   `LabelDb` stays block/field-oriented; `MeasurementDef` is a separate, id-indexed table
   (mirrors the ODX model), loaded from a cracked+parsed `STRUC.rod` and joined to engine-`.rod`
   `MWB` rows and `TTTEXT` names.
4. **`vagcan info` runtime flow (once the codec is done):** resolve the ECU's measurement list
   (engine `MWB` → `MeasurementDef`s), issue the UDS read (`read_id`), then apply `scale` to the
   raw bytes and print `name = value unit`.
5. **Still requiring work (all offline, no dump):** (a) reverse the `STRUC` 14-glyph field codec
   into `read_id`/`raw`/`scale`/`unit`; (b) decode the engine `MWB` 2-char code → `STRUC` id
   mapping; (c) crack `TTTEXT [TXT]` for names (mechanical, §1). **No runtime memory dump is on
   the critical path any more.** Item (4) of the original list — *which* file to open — is
   solved and shipped (§4.2).

**Superseding note (2026-08-01):** the offline route to `(DID, scale, unit)` is no longer the
plan. §4.0c refuted the premise that the read identifier is recoverable from the label bytes,
and the live path — sniffing VCDS on the bus with `vagcan sniff` — now supplies the same
information empirically. The `.rod` work retains its value for **names**, **the measurement
list per ECU**, and **file selection** (§4.2); the scaling comes from the car.

---

## Appendix A — the original NO-GO feasibility spike (from `rod-measurement-feasibility.md`)

> Historical. The crypto conclusion ("NO-GO offline, `product` needs a runtime dump") is
> **superseded by §0–§1**. The section census and the absence findings below still hold as
> data-format ground truth.

### A.1 Section inventory (all sample `.rod` files)
Census across the sample corpus (`OK` = `product==0`, fully decoded; `BLOCKED` = `product!=0`,
first block corrupt):

| file | CMP | INC | GES | SLV | MWB | ADP | DTC | IDN | XPL | SOT | TXT |
|---|---|---|---|---|---|---|---|---|---|---|---|
| EV_…VW48 (product 0 throughout) | OK | OK | OK | OK | **OK** | OK | – | – | OK | OK | – |
| EV_…VW37 | OK | BLOCKED | OK | OK | **BLOCKED** | – | – | – | BLOCKED | – | – |
| EV_…3100000 | OK | OK | OK | OK | **BLOCKED** | BLOCKED | BLOCKED | – | BLOCKED | – | – |
| CRFT1_ESP | OK | – | – | – | – | BLOCKED | OK | BLOCKED | – | – | – |
| CRFT1_EPH | OK | – | – | – | – | – | OK | BLOCKED | – | – | – |
| CRFT1_ESP_3_1229x (×4) | OK | OK | – | – | – | BLOCKED | – | – | – | – | – |
| **TTTEXT.ROD** | OK | – | – | – | – | – | – | – | – | – | **BLOCKED** |

Tags: CMP=component/ident, MWB=Measuring Value Blocks (**the measurements**), ADP=adaptations,
DTC=fault codes, IDN=identification, INC/GES/SLV/SOT/XPL=other label tables, TXT=the global
text/name table (only in `TTTEXT.ROD`). The `product` blocker is real, per-record, and hits
measurements: the *same* section kind (`MWB`) decodes in VW48 and is blocked in VW37/3100000
purely on the runtime `product` term.

### A.2 DID recovery — NOT PRESENT in the `.rod` payloads
In the fully-decoded VW48 `MWB` (221 rows, `product==0`), every row is `<6-digit id>,<short
code>` (e.g. `043439,4.`  `043900,_5`  `095490,23`  `011809,B_`). The 6-digit numbers are **not
UDS DIDs** — they range `2009 … 152526`; 70 of 221 exceed `0xFFFF` (can't be 16-bit
DataIdentifiers). Their range/density match line indices into the decompressed `TTTEXT` string
table — i.e. **name pointers**, not read identifiers. The trailing 1–2 char token joins to
nothing in the sample corpus. No other decodable sample section (CMP/INC/GES/SLV) contains
anything DID-shaped. **The read identifier lives elsewhere — in `STRUC.ROD`** (which, at spike
time, was *not in the sample corpus*; it is **now present** and cracked — §1/§2, which is why
this premise is resolved).

### A.3 COMPU-METHOD / scaling — not in the readable sample sections
No factor/offset, conversion table, or formula in any decodable sample section; `MWB` rows carry
only `(text-id, code)`. The occasional numbers in CMP rows (`252.9864`, `8.98`, `84981`) are
ident/version/checksum-shaped. The scaling is structural data in `STRUC.ROD` (now cracked but
behind the field codec — §2/§3), not text — so even unblocking `TTTEXT` (a *string* table) would
not yield scaling.

### A.4 Name join (`TTTEXT.ROD`) — mechanism + why it was BLOCKED
`TTTEXT.ROD` = a tiny `[CMP]` header record plus one giant `[TXT]` section: `4,920,744` cipher
bytes → **`7,620,128`** decompressed bytes of `<id>,<text>` name rows; the 6-digit `MWB` ids are
the join keys. `[TXT]` is zlib-DEFLATE'd; its first TEA-CBC block decrypts (with `product=0`) to
`78 da 54 bc 37 be 3a d2` — `78 da` is the **intact zlib magic** (proof `IV[0:3]` is correct),
but bytes `[3:8]` depend on the per-record `product` (`!= 0` for `TTTEXT`), corrupting the
DEFLATE stream at its start. At spike time this looked like a 2⁴⁰ search with no cheap
early-reject — **now defeated by the §1 header oracle** (the "brute force impractical" claim is
superseded).

### A.5 End-to-end verdict for ONE measurement (spike-era, superseded)
For VW48 `MWB` row `043439,4.` the spike recovered only the opaque `(text-id, code)` — name
(needs `TTTEXT` join, then-blocked), unit (absent from `.rod` payloads), DID (absent — in
`STRUC`), and scaling (absent — in `STRUC`) all unrecovered → **0 of 3 useful fields**, hence
NO-GO. The crypto half of that verdict is now reversed (§0–§1); the field-codec half survives.

---

## Appendix B — recovering the `product` term / names from process dumps (from `rod-product-term-dump.md`)

> Historical / alternative. Now that the crypto blocker is defeated offline (§1), **no runtime
> dump is on the critical path**. This appendix records the dump-based route that was pursued
> before the offline crack, including the still-useful heap name-harvest.

### B.1 Was the raw `TTTEXT` ciphertext / IV / product resident? → NO
Searched all five VCDS-RUS minidumps (`VCDS.exe.dmp`, `VCDS-2..4`, `VCDS-after-scan`):

| probe | result |
|---|---|
| `TTTEXT.ROD [TXT]` block0 ciphertext `43 e5 d4 da 14 e4 c3 54` | **absent in all 5** |
| tag-IV prefix `38 bc 58` (product-independent IV[0:3] for `[TXT]`) | 1 hit/dump, unrelated (a code immediate) |

VCDS was not decrypting `TTTEXT.ROD` at capture time, so the materialised 8-byte IV and the
`product` term were not recoverable from these dumps (the IV lives in `x19[0:8]` only during the
decrypt call). *(The offline crack of §1 sidesteps this entirely.)*

### B.2 The DECODED name table IS resident → GOOD
Adjacent to a resident `TTTEXT-RUS\0` filename string (~332 MB into `VCDS-3.exe.dmp`) sits an
array of decoded name records — the runtime name-resolution output (the very data the `product`
gates), in cleartext on the heap.

**Record layout** (VCDS-RUS x86; fixed 72-byte stride):
```
record+40 : constant tag dword   a4 92 57 00
record+44 : u32  name length
record+56 : inline CP1251 name bytes (NUL-padded)
```
Scanning every occurrence of the tag dword and reading `(len, name)` yields the resident set.
Reusable tool: **`research/clb-crack/rod_name_harvest.py`**.

**Harvest (records / unique names):**

| dump | records | unique names |
|---|---|---|
| `VCDS.exe.dmp` | 25,302 | 8,313 |
| `VCDS-2.exe.dmp` | 10,411 | 4,260 |
| `VCDS-3.exe.dmp` | 9,804 | 3,941 |
| `VCDS-4.exe.dmp` | 9,298 | 3,802 |
| `VCDS-after-scan.exe.dmp` | 9,427 | 3,690 |
| **union** | — | **11,479** |

Verbatim samples from `VCDS-3.exe.dmp` right after `TTTEXT-RUS\0` (CP1251, glossed):
`Температура` (Temperature), `Воздушный зазор` (Air gap), `Состояние` (Status), `Объект 1/2: ID`
(Object 1/2: ID), `Код клавиши 1/2` (Key code 1/2), `Код события 1/2` (Event code 1/2),
`По умолчанию` (Default), `НЕИЗВЕСТНЫЙ` (Unknown). ASCII in the same region: `Coded_Value`,
`alert_code`, `audio_bitrate`, `not_available`, `EV_GatewNF_013`, `EV_GATEWNF`, `V03935243KR`,
`3Q0-907-530-B`. Unambiguously VAG measurement/label names — the crypto-blocked content,
obtained **without** the cipher or the `product`.

**Caveats:** this is the **Russian** localisation and only the resident set the live session had
resolved (~8k unique in the fullest dump — large, not provably complete); the records store the
resolved **name string** only, so the 6-digit `MWB` id → name **linkage is NOT present as an
integer** (recovers *names*, not a rebuildable `id → name` map); and it does not touch the DID
or the scaling (absent from all readable `.rod` payloads regardless).

### B.3 One-shot dump recipe to recover the `product` (superseded, kept for reference)
Break at `ImageBase+0x33b94` (`VCDS-arm64-unpacked.exe`, ImageBase `0x140000000`) and dump the 8
bytes at `[x19]` = the materialised IV for the section about to be decrypted; for `[TXT]`,
`IV[0:3]` reads `38 bc 58` (sanity) and `IV[3:8]` are the wanted bytes. Invert the per-byte
transform (`s = IV·MT⁻¹`, `product_byte = s − KS`; invertible when `MT[OFF_ROD[i]]` is odd) to
get `product_bytes`, or feed the 8-byte IV straight into a decrypt. Equivalent: break at
`ImageBase+0x33910` and dump `x22` (the ~0x80-byte record-string); `product = (∏ its chars) &
(2^40−1)`. One capture per distinct section-string. **The §1 offline crack makes this
unnecessary.**

---

## 6. Reproduction

```
cd research/clb-crack
# Rust cracker (once): cargo build --release --offline --manifest-path rod_crack/Cargo.toml
.venv/bin/python rod_crack_prep.py prep   <file.rod> <TAG>        # -> crack_input.bin
./rod_crack/target/release/rod_crack                              # -> prints plaintext[3:8] hex
.venv/bin/python rod_crack_prep.py decode <file.rod> <TAG> <hex5> # -> inflated plaintext

# pure-python reference + parser self-test:
.venv/bin/python rod_struc_decode.py --selftest
.venv/bin/python rod_struc_decode.py <file.rod> <TAG>            # slow DFS crack

# heap name harvest from a dump (dumps are gitignored PII, never committed):
.venv/bin/python rod_name_harvest.py <dump.dmp>
```
Cipher/IV facts cross-checked with `research/clb-crack/decrypt_modern.py` (`rod_block0_iv`,
`rod_section_cipher`) and `crates/vag-data/src/rod.rs` (unchanged). Everything under
`research/vcds-data/` and `research/dumps/` is gitignored and never committed; no VIN/PII or
proprietary label text beyond minimal format snippets is reproduced.
```
STRUC.rod [STRUC]: plaintext[3:8] = 9d 69 92 24 29 → 293,560 bytes inflated.
```
