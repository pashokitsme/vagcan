# `TTTEXT2.ROD` — it does not open, and the reason is a second IV regime

`label-linkage.md` §5.5 named `TTTEXT2.ROD` and `MUX.rod` as the only two uncracked files
that could still hold a global measurement registry — the `identifier → measurement` link
this project has been missing since the first writeup. This is the attempt on `TTTEXT2.ROD`.

It did not open, and the reason is not the one anyone expected. The file is not slow to
crack — it is **refused before the search starts**, by a precondition that is false for two
fifths of the corpus and that nobody had reason to doubt. Every `.rod` this project has ever
cracked happened to sit on the right side of it. So the interesting result here is not about
`TTTEXT2.ROD` at all; it is that 7,830 files have been silently reported as unreadable, and
that one of them was opened here to prove they are not.

Companion to `research/rod-labels.md` (§1 for the crypto) and `research/label-linkage.md`
(§5.5 for why this file was picked). Working data is the English VCDS 26.3 install at
`~/vcds-en/`, plus the 25.12 copy under `research/VCDS-25.12.0/`; neither is committed.

---

## 0. Verdict up front

| question | answer | confidence |
|---|---|---|
| Does `TTTEXT2.ROD` decrypt and inflate with the existing tooling? | **No.** `vagcan vcds rod` prints `UNDECODABLE` after **0.1 s** — the key search never runs | certain (reproducible) |
| Is that a speed problem? | **No.** `recover_iv3to8` returns `None` at its first check | certain (code + timing) |
| Why? | its `[TXT]` first block does not decrypt to the zlib magic `78 da`, which the searcher requires as its anchor | certain |
| Is that specific to this file? | **No — 7,830 of 20,091 files** and **14,997 of 37,104** compressed sections are in the same regime | high (whole-corpus census, §3.1) |
| Is it the compression that differs, or the IV? | **the IV** — uncompressed `TEA` sections in the same files are hit identically | **very high** (237/237 vs 144/146, §3.2) |
| What is the deviation? | a **per-file XOR constant on the first-block IV**, applying to every section *after* `[CMP]` | **certain** — measured at 149/149 against a 13.8 % null (§3.3), then read off the binary (§3.3a) |
| Can `D` be derived from the file? | **No, and provably not.** VCDS XORs the finished IV with an 8-byte **runtime global**, skipped when the tag is literally `"CMP"` | **certain** (disassembly, §3.3a) |
| How wide is it? | **all eight IV bytes.** Bytes 0–2 by measurement (§3.3, §3.4), `IV[3:8]` first statistically (§3.5) then directly from a cracked key (§4.1) — so the searcher's 2³⁶ reduction is invalid on these files and the space is 2⁴⁰ | **very high** |
| Can a shifted section be opened at all? | **Yes — one was.** `EV_HCP4Contr2OBDAU41X_BY64.rod [FFMUX]` inflates to exactly its declared 55,121 bytes of `<text-id>,<code>` rows | certain (§4.1) |
| What is inside `TTTEXT2.ROD`? | **unknown — the file is not open**, and the sweep that would open it is 5–11 h of CPU that §8 argues should be spent differently first | — |
| Does anything in it link a measurement to a read identifier? | **unanswered.** Not "no": unanswered. See §6 before quoting this file as a closed question | — |
| Is `MUX.rod` blocked the same way? | **No** — it is in the classic regime and is crackable with today's tooling | certain (§6.1) |
| Anything shippable fall out? | yes, incidentally: the `product` is per **file**, not per section, so a classic file needs **one** search and not one per section | **very high** (16/16 exact, §3.6) |
| Does `label-linkage.md` §3's per-ECU negative survive the 40 % it never scanned? | **Yes** — 169/169 measurements listed in both regimes carry byte-identical `(text-id, code)`; but its "global function of the text-id" is really *(section kind, text-id)* | high (§6.2a) |

---

## 1. The file

`~/vcds-en/UDS_EV/TTTEXT2.ROD`, 3,939,518 bytes, two sections and no slack bytes between
them:

| tag | kind | cipher B | declared plain B | first-block plaintext `[0:2]` |
|---|---|---|---|---|
| `CMP` | tea | 40 | 34 | `30 32` — ASCII `"02…"`, an ordinary ident row |
| `TXT` | zlib | 3,939,432 | **5,887,861** | **`a5 1a`** — not `78 da` |

The 25.12 copy is the same shape (3,694,208 → 5,520,873; `[TXT]` prefix `43 d4`), so the
16 % growth is a version difference and the anomaly is not one: **no version of this file
presents the zlib magic.** `TTTEXT.ROD`, by contrast, presents `78 da 54` in both.

`label-linkage.md` §5.5's "3.69 MB cipher → 5.52 MB plaintext" was read off the section
header, not from an inflate. Nobody had decrypted a byte of it before this pass.

By §3's classification the file is **shifted**: its `[CMP]` reads correctly as-is (the exempt
first section) while `[TXT]` does not. One caveat is worth stating rather than buried — with
only two sections and one of them exempt, `TTTEXT2.ROD` is the one file where the shift cannot
be cross-checked *within* the file. The classification rests on the corpus-wide pattern of §3,
not on internal evidence.

---

## 2. What the existing tooling actually does, and why "UNDECODABLE" misleads

```
$ vagcan vcds rod ~/vcds-en/UDS_EV/TTTEXT2.ROD --cache ./iv.json
/Users/…/TTTEXT2.ROD: 2 section(s)
  [CMP     ]  tea         37 bytes  0219t çÙ,5,8_-59.08O,50KKKwtk08_..
  [TXT     ] UNDECODABLE          0 bytes
decoded 1/2 section(s)
                                             0.08s user   0.10s total
```

Eight hundredths of a second, with the `rod-crack` feature compiled in. The search did not
run. `crack.rs::recover_iv3to8` opens with

```rust
let p0 = t[0] ^ iv0[0];
let p1 = t[1] ^ iv0[1];
let d0 = t[2] ^ iv0[2];
if p0 != 0x78 || p1 != 0xda {
    return None; // not a zlib stream / wrong IV prefix
}
```

and `decode_rod_recover` turns that `None` into `RodStatus::Undecodable` — the same status a
truncated file, a misaligned cipher or a genuine inflate failure produces. The comment on
`vcds/rod.rs` is explicit that silence "would look like *this section is genuinely
unreadable*, which is the one conclusion that must never be reached by accident"; the
`--no-crack` note guards the *missing feature* case, but not this one. **A section whose
first block is not `78 da` is reported exactly like a section that cannot exist.** That is
how a file the roadmap called one of its two remaining hopes sat unexamined.

For the record the machinery itself is in good order. On the same corpus and machine:

```
$ vagcan vcds rod ~/vcds-en/UDS_EV/STRUC.rod
  [STRUC   ] zlib     297499 bytes   000001,2667._-_____5_5_24_7..6-4_…
                                             2.8 s wall, IV[3:8] = ac 32 38 71 e1
```

which is the vector `rod-labels.md` §1.3 recorded. The searcher is fast and correct; it is
being handed a false premise.

---

## 3. The premise is false for two fifths of the corpus

`rod-labels.md` §1.1 states it plainly, and it is the load-bearing assumption of the whole
crack: *"`IV[0:3]` is derived from the section tag → always exact. `IV[3:8]` carries the low
5 bytes of a runtime `product` term."* Because CBC makes only the first block depend on the
IV, an exact `IV[0:3]` means plaintext bytes 0, 1 and 2 are exact — the zlib magic survives
and deflate byte 0 is a free anchor. Everything downstream is built on that.

### 3.1 The census

Over all **20,091** files in `~/vcds-en/UDS_EV/`, decrypting each compressed section's first
block with the tag-derived IV and reading its two-byte prefix:

| files | |
|---|---|
| 12,197 | **classic** — every compressed section shows `78 da` |
| 7,827 | **shifted** — every compressed section shows *one and the same* other prefix |
| 3 | apparently mixed — see below |
| 64 | no compressed section at all |

Counted by section rather than by file: **22,107** compressed sections present `78 da` and
**14,997 do not — 40.4 %**. The shipped searcher declines every one of the 14,997.

The three "mixed" files are not exceptions but confirmations. In each, the `[CMP]` section is
itself zlib-compressed and shows `78 da`, while every later section shares one shifted prefix:

```
EV_DMCM1000081X0000000000.rod   CMP 78da | ADP DTC FFMUX GES MWB  all c046
EV_ECM20BTD03004L906056PC_006.rod CMP 78da | INC DTC MWB          all 7acb
IV_EV_EPHBO18VW3800000_VW38_D6.rod CMP 78da | DTC                     3760
```

So the rule is not per file but **per section position**: `[CMP]`, the first section, uses the
tag-derived IV; everything after it, in a shifted file, does not.

### 3.2 It is the IV, not the compression

The obvious first reading — "these sections use some compression that is not zlib" — is
wrong, and the test that kills it does not involve compression at all. Uncompressed `TEA`
sections carry ASCII (`<6-digit id>,<code>` rows), so their first bytes are printable if and
only if the IV is right. Excluding `[CMP]`, over 400 sampled files that contain both kinds of
section:

| file class | non-`CMP` TEA sections whose `plaintext[0:3]` is printable ASCII | binary |
|---|---|---|
| classic | **237** | 0 |
| shifted | 2 | **144** |

A compression change cannot corrupt an uncompressed section. The deviation is in the IV, and
it hits every section of a shifted file.

### 3.3 The deviation is a per-file XOR, and it is measurable for free

For any compressed section the true first two plaintext bytes are known — they are `78 da`.
So the deviation is readable directly:

```
D = observed_prefix XOR 78da          (two bytes, constant across every section of a file)
```

That `D` is constant across sections is already implied by the census (a shifted file shows
*one* prefix across tags whose tag-derived IVs all differ). The test that makes it a fact
about the IV rather than about zlib is to carry `D` over to the *uncompressed* sections of
the same file, where the plaintext is text and the expected content is completely different:

> **149 of 149** non-`CMP` TEA sections in shifted files decode to printable ASCII in bytes
> 0–1 once `D` is applied. **Zero** failures.

The null: two independent bytes landing in printable ASCII by chance is `(95/256)² ≈ 0.138`,
so 149 for 149 is not a result that can happen (`p ≈ 10⁻¹²⁷`). Before applying `D` those same
sections were binary (§3.2).

`D` is uniformly distributed — 348 distinct values over 349 shifted files sampled, with no
structure found in its low bits — and it is **not stored in the file**: 20,088 of the 20,091
containers have no slack bytes at all, every byte lying inside a `[TAG]…[/TAG]` span.

`[CMP]` is exempt. Over 467 shifted files with an uncompressed `[CMP]`, its id digits read
correctly **as-is** in 467 and correctly **under `D`** in 2 (records where both readings happen
to be digits). So `D` is state that exists only after the first section has been read.

### 3.3a The construction, read off the binary — and why it cannot be derived

Everything above was measured. It is also written down in VCDS, and reading it settles the one
question the statistics could not: where `D` comes from. `VCDS-arm64-unpacked.exe`
(ARM64 PE, ImageBase `0x140000000`, the build `rod-labels.md` §1.1 used), in the `.rod` IV
routine, immediately **after** the multiply that `rod-labels.md` §1.1 documents:

```asm
0x140033aa8  ; IV[i] = s[i] * MT[OFF_ROD[i]]  — the documented construction, offsets
0x140033ab0  ;   07 ca 22 99 3e 88 c3 76 in order, exactly OFF_ROD
…
0x140033b38  ldr  x0, [x21]                ; the section tag
0x140033b40  add  x1, x8, 0x160            ; -> the literal string "CMP"
0x140033b44  bl   0x14014f100
0x140033b48  cbz  w0, 0x140033b88          ; tag is CMP -> skip everything below
0x140033b50  ldr  w8, [x27, x8]            ; a runtime global
0x140033b54  ldr  w9, 0x140033d48          ; = 0x000f423f = 999999
0x140033b5c  b.le 0x140033b88              ; global <= 999999 -> skip
0x140033b64  add  x10, x27, x8             ; -> an 8-byte runtime global
0x140033b68  mov  w8, 8
0x140033b70  ldrsb w14, [x10], 1           ; mask byte
0x140033b78  ldrsb w11, [x9], 1            ; IV byte
0x140033b7c  eor   w11, w14, w11           ; IV[i] ^= mask[i]
0x140033b80  sturb w11, [x9, -1]
0x140033b84  cbnz  w8, 0x140033b70         ; eight times
```

Four things fall out, and each one matches a measurement above that was made before the
disassembly was read:

* the deviation is an **XOR**, applied to the finished IV rather than folded into the seed —
  which is why the cross-tag XOR is constant and the cross-tag difference is not (§3.3);
* it is **exactly eight bytes wide**, so it reaches `IV[3:8]` — §3.5 measured that
  statistically and §4.1 confirmed it from a cracked key;
* it is skipped when the tag is the literal **`"CMP"`** — §3.3 measured 467/467;
* the mask is a **runtime global**. It is not a field of the file, not a function of the file
  name, not a checksum. It is filled elsewhere in the process, exactly like the `product` term
  `rod-labels.md` §1.1 found to be "a runtime buffer — not any field of the file".

**So `D` cannot be derived offline, and that is a property of the design rather than a gap in
this analysis.** The corpus is not self-describing here; VCDS knows something the files do not
say. That closes the question the cheapest way it could have been closed — the alternative was
a search for a rule that is not there.

There is a second, unrelated find in the same routine worth recording: when the ODX name is
`TTTEXT` or `UNIT`, a file-wide byte from the global at `0x140552ba4` is added to every seed
byte (`0x140033a8c`). That is the same global and the same construction `research/codes-dat.md`
§2.2 calls `C`. Both of those files decode with `C = 0`, so nothing here depends on it, but a
future corpus where it is nonzero would break them in a way that looks like the shift and is
not.

`D` is otherwise uniform per file — 348 distinct values over 349 shifted files sampled — and
matches nothing structural (`MT`/`KS` adjacency, ratio, sum and XOR of its two bytes were all
tested against 242 files and all came back flat), which is what a runtime global should look
like from the outside.

### 3.4 The shift reaches at least byte 2, which is exactly the byte that costs

Byte 2 of a compressed section is deflate byte 0, the searcher's anchor, and no zlib magic
pins it. It can be reached through the *text* sections instead: a `<6-digit id>,<2-char code>`
record makes `plaintext[2]` a digit, so each such section admits ten values of `D[2]`, and
sections of one file must agree.

Over every file in the corpus with three or more sections of that exact record shape
(`plainlen % 11 == 0`, `\r\n` at the right offsets, all later records matching):

| file class | files | `D[2]` candidate sets that **intersect** |
|---|---|---|
| classic (control) | 31 | 31 |
| shifted | 66 | **66** |

If byte 2 were not shifted by the same constant, three ten-element subsets of 256 would
intersect with probability ≈ `256·(10/256)³ ≈ 0.0015`; the expected count is 0.1, the observed
is 66. So `D` is a constant XOR across at least bytes 0–2.

What this does **not** give is `D[2]` itself. The surviving sets are always 2, 4 or 8 values
wide (10 / 16 / 40 files respectively) and **never a singleton** — the digit alphabet
`0x30…0x39` covers every low-3-bit residue, so it cannot separate them. Two candidates is the
best case, and it needs at least three text sections in the file.

**`TTTEXT2.ROD` has no such section.** It holds exactly two: the exempt `[CMP]` and the
`[TXT]` we are trying to open. So its `D[2]` is not measurable, only searchable.

### 3.5 The shift reaches `IV[3:8]` as well — which is what makes it expensive

This is the finding that decides the cost of everything below, and it is measurable without
cracking anything.

`plaintext[6]` of a `<6-digit id>,<2-char code>` record is a **comma** — known exactly, not to
within ten digits. `MT[OFF_ROD[6]] = 151` is odd, so the map `seed → IV[6]` is a bijection and
the `product` byte behind it inverts uniquely:

```
product_byte[3] = (t[6] ^ ',') · 151⁻¹  −  KS[(tag[1]·8) & 0xff]        (mod 256)
```

The `product` is a property of the **file**, so two text sections of one file — different tags,
different `KS` shift, different ciphertext — must yield the same byte. Over every file with two
or more sections of that record shape:

| file class | files | the two readings **agree** | disagree |
|---|---|---|---|
| classic | 292 | **292** | 0 |
| shifted | 196 | 17 | **179** |

The classic column is the control, and it establishes two things at once: the documented
construction is exact at byte 6, and **the `product` really is shared across a file's
sections** (`rod-labels.md` §1.1 calls it "per-record", which reads as per-section). The
shifted column then says plainly that the deviation is not confined to bytes 0–2: it reaches
byte 6, and by implication the whole of `IV[3:8]`.

The 17 shifted files that *do* agree are not a leak in the argument — they are the rate the
model predicts. A constant XOR `D` on byte 6 survives the inversion only when the two sections
land on the same value of `(x ^ D) − x`, which takes `2^popcount(D)` values equally often, so a
uniformly-distributed `D` agrees with probability `E[2^−popcount] = (3/4)⁸ = 10.0 %`. Observed:
**8.7 %** (17/196). Under "no shift" it would be 100 %, which is what the classic column shows.

**Consequence.** The searcher's 2³⁶ reduction comes from `IV[i] = (s · MT[OFF[i]]) & 0xff`
being non-surjective for `i = 3, 5` (128 and 32 reachable values). XOR a shifted constant onto
those bytes and the true value leaves the reachable set, so the reduced search cannot find it
however long it runs — it returns a clean miss, indistinguishable from a wrong anchor byte.
A shifted section needs the full 2⁴⁰ space, **16× the work per anchor byte**.

A tempting cheaper test — "check that `IV[5]` inferred from a text section's digits is one of
its 32 reachable values" — is **vacuous and was discarded**: the reachable set for byte 5 is
the multiples of 8, and the digit alphabet `0x30…0x39` spans every low-3-bit residue, so *some*
digit always fits, for any file, shifted or not. It duly returned 1894/1894 and 1232/1232.
Recorded because it looks like evidence and is not; the comma above works precisely because it
admits no such freedom.

### 3.6 A free corollary for the classic 60 %: one crack per file, not one per section

That the `product` is shared per file is not just a lemma. Inverting a recovered `IV[3:8]`
gives the `product` bytes up to the ambiguity of the two even multipliers — 16 combinations —
and **every one of the 16 reproduces the other tags' keys exactly**, checked against the four
independently cracked sections of `EV_TCMDQ200021.rod`:

```
from the cracked MWB key -> DTC   bca678ea1c   reproduced by 16 of 16 candidates
                            FFMUX c0a6789a65   reproduced by 16 of 16
                            GES   7ebdc812c7   reproduced by 16 of 16
```

The ambiguous bits are annihilated by the even multipliers, so the recovery is exact. That
file cost four searches and needed one. Nothing in this pass depends on the corollary; it is
recorded because it is free CPU for whoever opens the corpus next.

---

## 4. The attempt

The searcher was extended — in a throwaway `git worktree`, **not** in `crates/` — with two
knobs: lift the `78 da` precondition, and take deflate byte 0 from the caller instead of
deriving it. It was validated against a known answer before use: on `STRUC.rod` the correct
anchor `0x8c` returns `ac 32 38 71 e1` in 1.9 s, and a wrong anchor `0x94` returns a clean
miss after a full 75-second sweep. So a miss is a miss and a hit is a hit.

### 4.1 A control file, where the anchor is nearly known — and it still misses

Sweeping 60 anchors blind is expensive, so the method was first tried where §3.4 narrows the
anchor to two values. `EV_HCP4Contr2OBDAU41X_BY64.rod` is shifted, has three text sections
fixing `D[2] ∈ {0x10, 0x11}`, and carries a 20,112-byte `[FFMUX]` section (55,121 plain) —
big enough for a strong oracle.

Both candidates are independently plausible: `d0 = 0x4c` and `0x4d` differ only in `BFINAL`,
and **both decode as `BTYPE = 2`, `HLIT = 9`** — a valid dynamic-Huffman header either way,
which a wrong `D[2]` would produce only a quarter of the time.

Both were searched with the reduced candidate sets. **Both missed**, cleanly, in 168 s and
146 s — which is what §3.5 predicts, since with the shift on `IV[3:8]` the reduced sets
provably do not contain the answer.

Re-run over the full 2⁴⁰ space, the same two anchors give:

```
full  d0=0x4c   miss   1089 s
full  d0=0x4d   HIT    iv3to8 = b1 a4 81 5c a3
```

and that key decodes the section:

```
plaintext[0:3] = 78 da 4d          <- the zlib magic, and HLIT=9 BFINAL=1 as predicted
inflated 55,121 bytes  ==  the declared plainlen, exactly
025483,3B / 025485,,G / 025478,1E / 025481,_W / 025480,.K / …
```

— the canonical `<6-digit text-id>,<2-char code>` rows, 55,121 bytes of them. **A shifted
section has been opened.** The whole model is confirmed end to end: `D = b0 bf 10` for this
file, recovered without any search for its first two bytes and to within two candidates for
its third.

The recovered key also settles §3.5 directly rather than statistically. Solving
`IV[i] = ((product_byte + KS[…]) · MT[OFF[i]]) & 0xff ^ D[i]` against all four sections of the
file leaves, for every byte, only solutions with `D` **nonzero**:

| `IV` byte | 3 | 5 | 6 |
|---|---|---|---|
| surviving `D[i]` | `51 53 71 73 d1 d3 f1 f3` | `19 39 99 b9` | `40 c0` |

Zero is in none of them. The shift covers `IV[3:8]`.

**Cost, measured rather than extrapolated.** The full 2⁴⁰ sweep took 1089 s against 168 s for
the reduced 2³⁶ — **6.5×, not the 16× the space ratio suggests**, because the header pruning
gets relatively better as the tree widens. That is the number to budget with.

### 4.2 `TTTEXT2.ROD` itself

Eight of the 60 anchors were swept with reduced candidate sets (`d0 = 0x04 … 0x3c`,
`HLIT 0…7`, `BFINAL = 0`), 67–149 s each, all misses, before §4.1 showed that mode cannot
succeed on a shifted file and the run was stopped. Nothing is learned from those misses;
they are recorded so nobody repeats them.

The run that *would* work was not affordable here. `TTTEXT2.ROD` has no text section to narrow
its anchor (§3.4), so all 60 legal values must be tried against the full space: at the measured
6.5× penalty on a ~100 s reduced sweep of this section, that is **≈ 11 minutes per anchor, 5–11
hours for the file** — five if `BFINAL = 0` is assumed and the answer falls mid-sweep, eleven
if not. It is mechanical, it is bounded, and §8 argues it is still the wrong thing to do first.

---

## 5. Answering the questions that were asked

1. *Does it decrypt and inflate at all with the existing tooling?* **No**, and the failure is
   instant rather than slow — 0.1 s, because the search is never entered. With the tooling
   extended as in §4 it becomes a 5–11 hour sweep rather than a 2-minute one; a sibling shifted
   file was opened that way here, so the route is demonstrated, not assumed.
2. *What is inside?* **Not established.** See §4.
3. *Does anything in it associate a measurement with a read identifier?* **Unanswered.** This
   is the question the exercise existed for and it is important not to launder a blocked file
   into a negative result. `label-linkage.md` §3's counting argument still stands for the
   *per-ECU* files; it never applied to a global table, and `TTTEXT2` is a global table.
4. *Names — how many, and do they extend `catalogs/names-uds.json`'s 17,009?* **Unanswered**,
   for the same reason. Note the shape is at least *consistent* with a name table: a single
   `[TXT]` section, the same tag as `TTTEXT.ROD`, and 5.89 MB of plaintext against
   `TTTEXT.ROD`'s 7.62 MB.

---

## 6. What is now left, and what changed

### 6.1 `MUX.rod` is not blocked

`MUX.rod` (522,631 B; one `[MUX]` section, 522,608 cipher → 2,188,449 plain) presents `78 da`.
It is a **classic** file and today's `vagcan vcds rod --features rod-crack` will crack it in
minutes. So `label-linkage.md` §7 item 3 — "crack `TTTEXT2.ROD` and `MUX.rod`" — is half a
much smaller job than it looked and half a larger one.

Worth noting for anyone budgeting the rest: `STRUC.rod`, `TTDOP.rod`, `TTTEXT.ROD`, `UNIT.ROD`
and `MUX.rod` are **all** classic. `TTTEXT2.ROD` is the only global table in the shifted
regime, which is precisely why four writeups' worth of `.rod` work never met this wall.

### 6.2 The cost of the blind spot is not one file

7,830 files and 14,997 compressed sections are unreadable with today's tooling, and every one
of them was silently reported as `UNDECODABLE`. That includes `MWB` and `DTC` sections of
control units this project may want later. The reference car's own two files
(`EV_ECM18TFS0208V0906264H_VW37.rod`, `EV_TCMDQ200021.rod`) are classic, so nothing already
proven is affected.

But `label-linkage.md` §3 — "scanning **all 16,576** `.rod` files in `UDS_EV` and decoding
every section that opens with `product = 0`" — could only ever have read the classic ones. In a
shifted file the sole section that opens is `[CMP]`, which carries no measurement rows. So the
100.00 % result behind the decisive per-ECU negative ("the 2-char code is a global function of
the text-id") rested on a **60 % sample that was believed to be the whole corpus**.

### 6.2a The blind spot has now been sampled, and §3 survives it

The shifted files do not have to be cracked to be read from, because **CBC only corrupts the
first eight bytes**. In an *uncompressed* section every record after the first is exact today,
with no key and no shift correction — the damage is confined to record one. That is enough to
run §3's test on the far side of the wall.

Parsing strictly (whole line must match `^\d{6},XX$`, code drawn from the proven 40-symbol
alphabet, any section with one bad line dropped entire) over all 20,091 files gives **5,699
rows from 4,763 sections — 2,302 of those rows from 1,725 sections of shifted files that §3
could not see at all**. Three tests:

| test | result |
|---|---|
| within one section kind, text-ids in ≥ 2 files carrying exactly one code | **429 / 429 (100.00 %)** |
| the same, restricted to **shifted** files only | **211 / 211 (100.00 %)** |
| a (kind, text-id) seen in **both** regimes carrying the same single code | **169 / 169 (100.00 %)**, zero disagreements |

**§3's rule holds in the 40 % it never covered.** The third row is the one that matters: 169
times, a measurement listed in a classic file and in a shifted file carries byte-identical
`(text-id, code)`. The shifted files are shifted, not different.

**One correction to §3, found on the way.** Its phrasing — "the code is a **global** function of
the text-id, full stop" — is slightly too strong. Pooling across section kinds produces 12
apparent counter-examples, and they are real: of 13 text-ids appearing in two or more section
kinds, **12 carry a different code in each kind**. The code is a function of *(section kind,
text-id)*, not of the text-id alone. §3's own numbers are unaffected because its table is
already computed per kind — but the sentence under the table would let a reader join `GES` and
`ADP` rows on the text-id and get the wrong code. That trap is now measured, not latent.

**What this does not cover.** Only uncompressed sections are reachable this way; the 14,997
compressed sections in shifted files stay closed without keys, and `MWB` — the measurement list,
and the section the question is really about — is compressed in almost every file (11 of the
429 text-ids come from an `MWB`). So this is a genuine sample of the blind spot, not an
exhaustive scan of it, and it is a smaller sample than §3's 10,583. It says the wall hides
nothing unusual; it cannot say the wall hides nothing at all.

### 6.3 The honest state of the central question

The `.rod` corpus is **not** yet proven to be names-and-lists-only. `label-linkage.md` §7 item
3 said that if neither `TTTEXT2` nor `MUX` holds a registry, the question closes for good.
Neither has been examined. `MUX.rod` can be examined today, in minutes. `TTTEXT2.ROD` is
reachable but expensive, and nothing here licenses treating it as answered — a file nobody has
decrypted a byte of is not evidence for or against anything.

One thing this pass does *not* change: the shifted files are shifted, not different. The one
that was opened holds exactly the `<6-digit text-id>,<2-char code>` rows every other per-ECU
section holds. There is no sign of a second format hiding in the 40 %, which mildly favours
`label-linkage.md` §3's conclusion rather than threatening it — but it is one file.

---

## 7. Reproduction

```
# the failure, in a tenth of a second
cargo run --release -p vagcan --features rod-crack -- \
      vcds rod ~/vcds-en/UDS_EV/TTTEXT2.ROD --cache /tmp/iv.json

# the regression that proves the searcher itself is fine
cargo run --release -p vagcan --features rod-crack -- \
      vcds rod ~/vcds-en/UDS_EV/STRUC.rod --cache /tmp/iv.json
#   -> [STRUC] zlib 297499 bytes,  IV[3:8] = ac 32 38 71 e1
```

The census and the three statistical tests of §3 are a few dozen lines of Python over a
re-implementation of `rod.rs::rod_block0_iv` + `tea_cbc_decrypt` (the tables are
`crates/vag-data/src/rod_{mt,ks}.bin`). The steps are small enough to restate rather than
ship: for each file, decrypt each section's first block with the tag-derived IV, take
`plaintext[0:2]`, and

* count files whose compressed sections all read `78 da` versus all read one other value;
* for shifted files take `D = prefix ^ 78da` and check `plaintext[0:2] ^ D` on the
  *uncompressed* sections;
* check `[CMP]` both ways;
* for §3.5, invert `plaintext[6] = ','` through `151⁻¹` in two sections of one file and compare.

The one vector recovered here, for anyone re-checking §4.1:

```
EV_HCP4Contr2OBDAU41X_BY64.rod  [FFMUX]   D = b0 bf 10   iv[3:8] = b1 a4 81 5c a3
                                          -> 55,121 bytes, matching the declared plainlen
```

It is deliberately **not** added to `catalogs/rod-iv-cache.json`: the shipped decoder has no
way to apply `D[0:3]`, so a cached `iv[3:8]` alone would not decode that section, and a cache
entry that does not work is worse than no entry.

Nothing under `research/clb-crack/` was modified, and the searcher change of §4 was made in a
throwaway `git worktree`, not in `crates/`.

---

## 8. What to do next, in order of value

1. **Open `MUX.rod`.** It is classic, it is minutes of CPU, and it is one of the two files
   `label-linkage.md` §5.5 said could still hold a global registry. There is no reason it is
   not already done.
2. **Stop reporting a failed precondition as `UNDECODABLE`.** `decode_rod_recover` collapses
   "this cannot be read" and "the searcher declined to look" into one status, and that is what
   kept `TTTEXT2.ROD` closed through four writeups that each named it as the next thing to try.
   A section that decrypts to something other than `78 da` should say so. This is the one
   change in `crates/` that this pass actually justifies.
3. ~~**Find where `D` comes from**~~ — **done, §3.3a, and the answer is that it cannot be
   derived.** It is an 8-byte runtime global. Do not spend more time looking for a rule; there
   isn't one to find. What is left is recovery from known plaintext, which §3.3 already does
   for two of its bytes for free.
4. **If it must be brute-forced, budget 5–11 h and run it unattended.** The recipe is proven
   (§4.1): take `D[0:2]` free from the section prefix, sweep all 60 legal anchor bytes, use the
   full 2⁴⁰ candidate sets. The measured penalty over the reduced search is 6.5×, not 16×.
5. **Use §3.6 on the classic files.** One search per file instead of one per section.
6. **Do not** re-run: the reduced-candidate-set search on any shifted section (§3.5, §4.1), the
   `IV[5]` reachability test (§3.5), or any attempt to read `D` out of the container bytes
   (§3.3 — there are none).
