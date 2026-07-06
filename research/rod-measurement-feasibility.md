# `.rod` measurement feasibility — can we build a UDS measurement decoder OFFLINE?

**Question.** To turn a raw UDS measurement into a human value we need three
things: **(1)** the DID / identifier the ECU is read with, **(2)** the
COMPU-METHOD scaling (raw bytes → engineering value + unit), **(3)** the human
name (via a `TTTEXT` join). Can all three be recovered **fully offline** from
the VCDS `.rod` (UDS/ODX) corpus we have?

**VERDICT: NO-GO for offline.** Of the three, only the *hook* for the name is
present (a 6-digit text index per measurement), and even that name table
(`TTTEXT.ROD [TXT]`) is **crypto-blocked offline** by the per-record `product`
term. The **DID and the COMPU-METHOD scaling are not present in any section we
can decode at all** — they are not merely undecoded, they are absent from the
readable `.rod` payloads, and the file that would hold them (`STRUC.ROD`) is not
in our corpus. A runtime dump (see §7) unblocks the *names* but does **not** by
itself supply the DID or the scaling. See §6 for the honest breakdown.

Confidence: HIGH. Everything below is reproduced from the sample corpus with the
already-cracked TEA-CBC/KEY_ROD pipeline (`crates/vag-data/src/rod.rs`, mirrored
by `research/clb-crack/decrypt_modern.py`; both use identical keys/tables/IV, so
these findings apply directly to `decode_rod`).

---

## 1. Section inventory (all sample `.rod` files)

Every encrypted section is TEA-CBC / `KEY_ROD` with a section-tag IV. The
6-byte section header is two BE24 ints: `read1` (bit `0x800000` = *uncompressed*
flag, low 23 bits = stored cipher length) and `read2` (plaintext / decompressed
length). Flag-clear sections are additionally zlib-DEFLATE'd. The **first block**
of every record needs a per-record `product` term in `IV[3:8]`; `IV[0:3]` is
exact from the tag. When `product == 0` the whole record decodes; otherwise the
first block is corrupt — which for a zlib section kills the *entire* inflate.

Census across the corpus (`OK` = `product==0`, fully decoded; `BLOCKED` =
`product!=0`, first block corrupt):

| file | CMP | INC | GES | SLV | MWB | ADP | DTC | IDN | XPL | SOT | TXT |
|---|---|---|---|---|---|---|---|---|---|---|---|
| EV_…VW48 (product 0 throughout) | OK | OK | OK | OK | **OK** | OK | – | – | OK | OK | – |
| EV_…VW37 | OK | BLOCKED | OK | OK | **BLOCKED** | – | – | – | BLOCKED | – | – |
| EV_…3100000 | OK | OK | OK | OK | **BLOCKED** | BLOCKED | BLOCKED | – | BLOCKED | – | – |
| CRFT1_ESP | OK | – | – | – | – | BLOCKED | OK | BLOCKED | – | – | – |
| CRFT1_EPH | OK | – | – | – | – | – | OK | BLOCKED | – | – | – |
| CRFT1_ESP_3_1229x (×4) | OK | OK | – | – | – | BLOCKED | – | – | – | – | – |
| **TTTEXT.ROD** | OK | – | – | – | – | – | – | – | – | – | **BLOCKED** |

Tags: CMP=component/ident, MWB=Measuring Value Blocks (**the measurements**),
ADP=adaptations, DTC=fault codes, IDN=identification, INC/GES/SLV/SOT/XPL=other
label tables, TXT=the global text/name table (only in `TTTEXT.ROD`).

**Undecodable / `product` blocker is real, per-record, and it hits
measurements.** The *same* section kind (`MWB`) decodes in one file (VW48) and
is blocked in two others (VW37, 3100000) purely on the runtime `product` term —
the file bytes are otherwise complete. So 1 of 3 UDS `MWB` sections decodes, and
the crucial global name table `TTTEXT [TXT]` is blocked.

---

## 2. DID recovery — **NOT PRESENT offline**

Take the *fully decoded* VW48 `MWB` section (221 rows, `product==0`). Every row
is exactly `<6-digit id>,<short code>`:

```
043439,4.
043900,_5
095490,23
011809,B_
...
```

- The 6-digit numbers are **not UDS DIDs**. They range `2009 … 152526`; 70 of
  221 exceed `0xFFFF`, so they cannot be 16-bit DataIdentifiers. Their range and
  density match **line indices into the 7.6 MB decompressed `TTTEXT` string
  table** (see §4) — i.e. they are *name pointers*, not read identifiers.
- The trailing token is a 1–2 char opaque code (charset `[0-9A-Z_.,-]`); it
  joins to nothing in our corpus and is far too small to be a DID either.
- No other decodable section (CMP/INC/GES/SLV) contains anything DID-shaped —
  CMP is one ident/version row (`776939,--_252.9864.HR0HCIJZ98RX,…`), the rest
  are the same `<text-id>,<code>` shape.

**There is no DID anywhere in the `.rod` payloads we can read.** The read
protocol/identifier for these measurements lives elsewhere — almost certainly in
`.\UDS_EV\STRUC.ROD` (the structure file the binary references), which is **not
in our sample corpus**. Cannot be recovered offline from what we have.

---

## 3. COMPU-METHOD / scaling — **NOT PRESENT offline**

There is **no factor/offset, no conversion table, and no formula** in any
decodable section. The `MWB` rows carry only `(text-id, code)` — no numeric
scaling fields. The occasional numbers in CMP rows (`252.9864`, `8.98`, `84981`)
are ident/version/checksum-shaped and are not per-measurement conversions.

The scaling would be structural data (in `STRUC.ROD`), not text — and `TTTEXT`
is a *string* table, so even unblocking `TTTEXT` (§7) would not yield scaling.
**This is the make-or-break finding: the raw→engineering conversion is simply
not in the data we can access offline.**

---

## 4. Name join (`TTTEXT.ROD`) — mechanism exists, but the table is **BLOCKED**

`TTTEXT.ROD` is the global name table: a tiny `[CMP]` header record plus one
giant `[TXT]` section — `4,920,744` cipher bytes → **`7,620,128`** decompressed
bytes of (presumably) `<id>,<text>` name rows. The 6-digit `MWB` ids (§2) are
the join keys into it.

The join is blocked at the crypto layer. `[TXT]` is zlib-DEFLATE'd; its first
TEA-CBC block decrypts (with `product=0`) to:

```
78 da 54 bc 37 be 3a d2
```

`78 da` is the **intact zlib magic** — proof that `IV[0:3]` (tag-derived) is
correct. But bytes `[3:8]` (`bc 37 be 3a d2`) depend on the per-record `product`
term, which is `!= 0` for `TTTEXT`. Wrong bytes 3–7 corrupt the DEFLATE stream
at its very start, so `zlib.decompress` fails and the **entire 7.6 MB name table
is unreadable**. So the worked `id → name` example cannot be produced offline:
`043439 → <name>` needs `TTTEXT [TXT]`, which is `product`-blocked.

Brute force is impractical: `IV[3:8]` is 5 unknown bytes = **2⁴⁰** candidates,
each requiring an inflate attempt, with no cheap early-reject beyond the first
DEFLATE block header. The `product` is the low 5 bytes of a 64-bit char-product
of a **runtime-only** "record-string" (not derivable from the file bytes — see
`NOTES-modern.txt`), so it cannot be enumerated offline either.

---

## 5. Confirm/refute the `rod.rs` module-doc claims

| module-doc claim | verdict |
|---|---|
| `MWB` rows are `<6-digit id>,<code>`, an index not a name | **CONFIRMED** — exactly `NNNNNN,XX`. |
| Human names live in `TTTEXT.ROD` and need a join | **CONFIRMED** — no names in any `.rod` payload; `TTTEXT [TXT]` is the name table. |
| Some first-records are `Undecodable`: need a nonzero `product` term (runtime dump) | **CONFIRMED & quantified** — `TTTEXT [TXT]` blocked; `MWB` blocked in 2/3 EV files; ADP/DTC/IDN/INC/XPL blocked in several. |
| COMPU-METHOD / scaling is not decoded | **CONFIRMED — and stronger:** scaling is *not present at all* in any readable section, not merely undecoded. |

---

## 6. End-to-end verdict for ONE measurement

**Measurement:** VW48 `MWB` row `043439,4.` (from the one file that decodes
completely — best possible offline case).

| field | recovered offline? | why |
|---|---|---|
| raw text-index `043439` + code `4.` | **YES** | present in the decoded `MWB` row |
| **name** | **NO — BLOCKED** | needs `TTTEXT [TXT]` join; that section is `product`-blocked (§4) |
| **unit** | **NO** | not in `.rod` payloads; would be a `TTTEXT`/`STRUC` string — blocked/absent |
| **DID / identifier** | **NO — ABSENT** | no DID in any readable section; lives in `STRUC.ROD` (not in corpus) |
| **scaling (COMPU-METHOD)** | **NO — ABSENT** | no factor/offset/table/formula in any readable section (§3) |

So offline we recover **only an opaque pointer**, and **0 of the 3 useful
fields** (name, DID, scaling). Even in the single fully-decrypting file, the
measurement is not decodable into a human value.

---

## 7. What a runtime dump would need to provide (mirrors the clone-crypto blocker)

Two independent gaps, both requiring a Windows dynamic session on
`VCDS-arm64-unpacked.exe` (ImageBase `0x140000000`):

1. **The `product` term (unblocks names + all blocked measurement records).**
   Break at `ImageBase+0x33910` and dump the record-string at `x22` (~32 bytes),
   **or** dump the 8-byte `IV` at `x19` just before `ImageBase+0x33b94`, while
   VCDS opens `TTTEXT.ROD` / the target `EV_*.rod`. One dump per record-string
   value closes `IV[3:8]` and lets the whole `[TXT]` table (and blocked `MWB`)
   inflate. This is the *same class of blocker* as the clone-crypto runtime-dump
   wall in `research/DYNAMIC-attack-RESULTS.md`. **This gives us names + units
   text only.**

2. **`STRUC.ROD` (supplies DID + scaling) — separately required.** Even with #1,
   the DID and COMPU-METHOD are absent from the corpus. We would need to obtain
   `.\UDS_EV\STRUC.ROD` (referenced by the binary) *and* reverse its record
   format — and it is likely subject to the same `product` blocker, so it would
   also need #1.

**Conclusion:** offline with the current corpus this is a **NO-GO**. A runtime
dump alone yields names, not measurements. A full `.rod`-driven measurement
decoder additionally requires `STRUC.ROD` + its format RE. Given that the ECU's
UDS layer already exposes readable measurement blocks, a **generic-CAN /
live-ECU** measurement path (read + interpret from the ECU directly) is the
pragmatic route rather than reconstructing DID+scaling from VCDS files offline.

*Reproduction:* throwaway analysis used `research/clb-crack/decrypt_modern.py`
(unchanged) over `research/clb-crack/samples/rod/*.rod` + `TTTEXT.ROD`.
