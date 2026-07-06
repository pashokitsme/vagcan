# Cracking the offline `.rod` measurement pipeline (STRUC.rod + engine `.rod`)

**Question (re-opened).** The prior spike (`research/rod-measurement-feasibility.md`)
returned **NO-GO** for an offline measurement decoder for two reasons: (1) the
DID + COMPU-METHOD scaling file `STRUC.ROD` was *not in our corpus*, and (2) the
`.rod` first-block `product`/IV term blocks `STRUC`/`TTTEXT`/`MWB` and was
believed to need a **runtime memory dump**. Both premises are now testable:
`STRUC.rod`, the full `TTTEXT.ROD`, and the owner's exact engine `.rod` are
present (gitignored VCDS install, symlinked under `research/vcds-data/`).

## TL;DR — VERDICT: **PARTIAL**, but the crypto wall is gone

- **The `product`/IV blocker is DEFEATED OFFLINE — no runtime dump needed.**
  Proven end-to-end on `STRUC.rod`: its single, previously-100%-blocked zlib
  section now **fully inflates to its exact 293,560-byte plaintext** (valid
  Adler-32). This *refutes* the prior "needs a runtime dump" conclusion for the
  crypto layer. The same method applies to `TTTEXT.ROD` and every blocked
  `MWB`/`INC`/`DTC` section.
- **`STRUC.rod` is present and decrypts** — it is the structure/definition table
  (1,221 structure-ids, 8,854 records). So premise (1) is also resolved.
- **What is still NOT solved:** the *decoded* `STRUC` payload is a **packed,
  comma-delimited numeric field format over a 14-glyph alphabet** (`[0-9,._-]`),
  not readable `DID/factor/offset/unit` text. Extracting the three MVP scalings
  needs a further **offline data-format RE pass** (no crypto, no dump). That pass
  is not completed here, so a clean RPM/speed/boost scaling formula is **not yet
  proven**. Hence PARTIAL (upgraded from NO-GO: the data is present and fully
  decryptable offline; only a field-layout decode remains).

Confidence: HIGH for the crypto crack (reproducible, exact inflate + Adler-32).
HIGH that STRUC carries the structure data. The unfinished part is characterised
honestly in §3.

---

## 1. The crypto crack — how the `product`/IV blocker falls offline

### 1.1 The blocker (recap, verified against the binary)
Each `.rod` section is TEA-CBC/`KEY_ROD` with an 8-byte IV. `IV[0:3]` is derived
from the section tag → **always exact**. `IV[3:8]` carries the low 5 bytes of a
runtime `product` term. Because CBC makes blocks 2..n independent of the IV,
**only the first 8 plaintext bytes depend on `IV[3:8]`**. For a zlib section that
means the 2-byte zlib header `78 da` is intact but **deflate bytes 1..5 are
corrupted**, which kills the whole inflate.

Disassembly of the raw-`.rod` decrypt fn (`VCDS-arm64-unpacked.exe`,
`ImageBase 0x140000000`, fn `+0x33758`) confirms the derivation exactly:
- `+0x33910..+0x3399c`: `product = ∏ (signed) chars of a record-string` (a
  `mul` accumulator in `x25`); its low 5 bytes are stored into the IV seed at
  `buf+3..buf+7` (`+0x339a0` byte-extract loop).
- The record-string is built from a **runtime singleton** (`+0x337a0`, object at
  `singleton+0x18`), a constant path fragment (`".\UDS_EV\"` @ `0x1701_00d8`),
  then **truncated at the first `.`** (`+0x338a0`, char `0x2e`). It is a runtime
  buffer — **not any field of the file**, so `product` cannot be *computed*
  offline. Prior work stopped here and prescribed a memory dump.

### 1.2 The insight — you don't need `product`, only 5 plaintext bytes
`IV[3:8]` is 5 bytes; the corrupted plaintext is exactly `deflate[1..5]`. Two
facts make brute force tractable:

1. **The candidate space is ~2³⁶, not 2⁴⁰.** Each `IV[i] = (s · MT[OFF[i]]) & 0xff`.
   For `i = 3,5` the multiplier `MT[OFF[i]]` is **even** (`250`, `152`), so those
   IV bytes take only `256/gcd = 128` and `32` distinct values. Effective space =
   `128·256·32·256·256 = 2³⁶`.
2. **The DEFLATE dynamic-Huffman header is a strong, cheap oracle.** `deflate[0]`
   is exact and pins `BFINAL/BTYPE=dynamic/HLIT`. The 5 unknown bytes sit inside
   the header (HDIST, HCLEN, the code-length-code lengths, and the start of the
   HLIT+HDIST code-length list). A candidate is rejected the instant the
   code-length codes violate the **Kraft inequality** (checked *incrementally*),
   and again when the HLIT/HDIST code lengths — decoded against the **exact tail
   bytes** — over/under-subscribe. Only a true 5-tuple survives to a full inflate.

### 1.3 Tooling & result
- `research/clb-crack/rod_struc_decode.py` — pure-Python reference: a DFS over the
  5 bytes with incremental-Kraft pruning (`--selftest` validates the deflate
  parser against the known-good VW48 `MWB` section). Correct but slow (~120k
  nodes/s).
- `research/clb-crack/rod_crack/` — standalone multithreaded **Rust** brute-forcer
  (isolated: own empty `[workspace]`, not a workspace member). Header oracle +
  `miniz_oxide` inflate confirmation across all cores.
- `research/clb-crack/rod_crack_prep.py` — preps `crack_input.bin` for a given
  file+tag and re-inflates with a recovered 5-byte guess.

**Proof (STRUC.rod, `[STRUC]` section):**

```
plainlen = 293560, d0 = 0x8c, candidate set sizes = [128,256,32,256,256]
RECOVERED plaintext[3:8] = 9d 69 92 24 29   (=> IV[3:8] = ac 32 38 71 e1)
inflated 293560 bytes  (exact; valid zlib Adler-32)
```

Rust cracked it in ~1 min on 10 cores. The recovered IV inverts to seed/product
bytes (`s = 14|142, 226, 29|61|…, 55, 143`), consistent with the `∏`-of-chars
model — but note we **never needed the runtime string**.

> Cost note: the Rust port is a flat nested sweep (early-exit on hit). STRUC's
> answer sat ~3% into the space (~2.2·10⁹ checks). `TTTEXT.ROD [TXT]`
> (7.46 MB inflate) is the *same* section type/blocker and is crackable the same
> way; a full sweep of 2³⁶ is minutes-scale, and porting the Python DFS pruning
> into Rust would make it fast regardless of where the answer lands.

---

## 2. `STRUC.rod` layout

- Container: a **single** `[STRUC] … [/STRUC]` ASCII-tagged section (tag is
  **5 letters** — note `rod.rs::find_next_tag` currently caps tags at 4 letters,
  so it must be widened to accept `STRUC`; see §5).
- Header: the standard `.rod` 6-byte header — two BE24 ints. `read1` = `0x800000`
  (uncompressed flag, clear here) | 23-bit stored cipher length = 78,744;
  `read2` = decompressed length = 293,560. TEA-CBC/`KEY_ROD`, tag IV, then zlib.
- Decoded content: **8,854 records**, one per line `NNNNNN,<payload>\r\n`.
  Structure-id `NNNNNN` runs `000001…001623` (**1,221 distinct ids**), 1–11 rows
  per id (mostly 1–3; a mode at 8). This id namespace is **small** and distinct
  from the 6-digit *text*-ids seen in `MWB` (which range up to ~152,526 and index
  `TTTEXT`).
- Payload: comma-bearing strings over a **uniform 14-symbol alphabet** `[0-9,._-]`
  (each symbol ~7% frequency). Field counts per payload are irregular (1–11),
  and rows for one id do not align into clean fixed columns — i.e. this is a
  **packed/encoded structure record**, not a plain CSV of `DID,factor,offset`.
  Treating `. , - _` as delimiters and reading the digit groups yields
  **non-sane magnitudes** (e.g. `557268788888`, `853721282`) — so the punctuation
  is *not* simple separators/signs and the digits are *not* literal values; it is
  a codec (very likely the same digit-transform VCDS applies when it parses a
  STRUC record). The scaling/DID data is *in here*, but behind a field codec that
  still needs RE (reverse the STRUC-parser fn in the binary).

Sections decoding cleanly vs blocked, for `STRUC.rod`: **1 / 1** (the sole
section, cracked). No residual `product`-blocked sections remain in this file.

---

## 3. Engine `.rod` → STRUC → TTTEXT join — what resolves, what doesn't

**Owner's exact engine** `EV_ECM18TFS0208V0906264H.rod` (`8V0 906 264 H`,
`J623-CJSA` 1.8 TFSI) decodes to:

| tag | kind | status | content (format-level) |
|---|---|---|---|
| CMP | tea  | OK (product 0) | one ident row: `627023,<part/HW/SW tokens>` |
| SLV | tea  | OK (product 0) | one row `027305,…` |
| INC | zlib | **crackable** (product≠0) | not yet cracked (small ⇒ weak oracle, slower) |
| DTC | zlib | **crackable** (product≠0) | fault-code table |

Note the owner's `-H` file carries **no `MWB` section**; the measurement list is
in sibling variant files (e.g. `EV_ECM18TFS0208V0906264A.rod` has a `MWB`, also
`product`-blocked and crackable). The `MWB` rows are `<6-digit text-id>,<2-char
code>` where the text-id indexes `TTTEXT` (the *name*) and the 2-char code (over
a ~40-symbol alphabet `[0-9A-Z._,-]`, 40²≈1600 ≈ the STRUC id ceiling) is the
plausible **reference into `STRUC`** for the structure/scaling — this linkage is
consistent but **not yet decoded** (same field-codec gap as §2).

**The chain, honestly:**
`engine MWB row` → (text-id → `TTTEXT` name) is a **decrypt-only** join (works
once `TTTEXT` is cracked — §1 method). `engine MWB row` → (code → `STRUC` id →
DID + scaling) is **decrypt-OK but the STRUC field codec is unresolved**, so the
numeric scaling cannot yet be read out.

**Is `product` on the VALUE critical path?** No longer for *decryptability* — it
is fully brute-forced offline. It is on the path only in that every interesting
section (`STRUC`, `TTTEXT`, engine `MWB`) is `product`-blocked and must be
cracked first. After cracking, the remaining obstacle is a **plaintext format**
problem, not crypto and not a dump.

---

## 4. The three MVP measurements (RPM / vehicle speed / boost)

**Not yet provable as scaling formulas.** Getting `identifier + factor/offset +
unit + sample conversion` requires reading them out of the `STRUC` payload
(and mapping via the engine `MWB` code), which is blocked on the §2 field codec,
not on crypto. The pieces we can now place offline:

| field | offline status |
|---|---|
| decrypt of STRUC / TTTEXT / engine-MWB | **YES** — brute-forced, exact (§1) |
| measurement **name** (RPM / speed / boost) | **YES in principle** — text-id → `TTTEXT` join, both decryptable offline; not yet dumped here (nicety) |
| **DID / read identifier** | present in `STRUC`/`INC` but behind the field codec — **not yet extracted** |
| **scaling (factor/offset/formula)** | present in `STRUC` payload but **encoded** — **not yet extracted** |
| **unit** | ditto (a `STRUC`/`TTTEXT` reference) — **not yet extracted** |

So the honest answer for RPM/speed/boost: **the data is offline and decrypted,
but the raw→engineering conversion is still encoded in the STRUC field format.**
Cross-referencing units/names against the engine `.clb` is moot for scaling —
`06K-907-425-V1/V2.clb` decode cleanly (clb IV is fully solved) but are
**long-coding (LC) label** files (coding helpers), not measurement scaling.
`Scaling/OBD.SCL` is ASCII but only **generic OBD-II PID display ranges**
(`pid,min,max`), and the `*.a01` files are auto-scan **group presets** — neither
carries per-DID conversion constants.

---

## 5. Design proposal — how `vag-data` should consume this (TEXT ONLY)

*(No crate is modified here; another workflow owns `crates/`.)*

1. **Widen the `.rod` framing scanner.** `rod.rs::find_next_tag` accepts 2–4
   uppercase tag letters; `STRUC` is 5. Widen to 2–5 (or 2–8) so `[STRUC]` is
   recognised. `find_close`/`decode_section` already generalise.
2. **Add a `product`/IV recovery step for zlib sections.** Give `rod_block0_iv`
   an explicit `iv3to8: Option<[u8;5]>` (or accept a full IV). When absent and a
   zlib section fails to inflate, run the brute-forcer (port the Rust
   header-oracle + Kraft-pruned DFS; feature-gated / offline-tool crate) to
   recover the 5 bytes, then inflate. Cache recovered IVs per (file, tag).
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
   `LabelDb` stays block/field-oriented; `MeasurementDef` is a separate,
   id-indexed table (mirrors the ODX model), loaded from a cracked+parsed
   `STRUC.rod` and joined to engine-`.rod` `MWB` rows and `TTTEXT` names.
4. **`vagcan info` runtime flow (once §3 codec is done):** resolve the ECU's
   measurement list (engine `MWB` → `MeasurementDef`s), issue the UDS read
   (`read_id`), then apply `scale` to the raw bytes and print `name = value unit`.
5. **Still requiring further work (all offline, no dump):** (a) reverse the
   `STRUC` 14-glyph field codec into `read_id`/`raw`/`scale`/`unit`; (b) decode
   the engine `MWB` 2-char code → `STRUC` id mapping; (c) crack `TTTEXT [TXT]`
   for names (mechanical, §1). **No runtime memory dump is on the critical path
   any more.**

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
```

Everything under `research/vcds-data/` (symlink to the owner's VCDS install) and
`research/dumps/` is gitignored and is **never** committed; no VIN/PII or
proprietary label text beyond the minimal format snippets above is reproduced.
```
STRUC.rod [STRUC]: plaintext[3:8] = 9d 69 92 24 29 → 293,560 bytes inflated.
```
