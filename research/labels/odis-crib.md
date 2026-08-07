# ODIS as a crib against the label files — what it breaks, and what it does not

VW's own ODIS-Service runtime data was examined on 2026-08-07 and tested as a source of
known plaintext against the two ciphers in Ross-Tech's label files. The result splits
cleanly: it is **useless against the `.rod` container** and it is the **strongest lever
yet found against the `TTTEXT` substitution**, where a single ODIS project more than
doubles the recovered name catalog and lands the first blow on the numeric class that
`tttext-codec.md` §6 recorded as unbroken.

Companion to `research/labels/rod-labels.md` (the container) and
`research/labels/tttext-codec.md` (the cipher, read §1 and §6 first). The pilot script is
`research/labels/odis_crib.py`. Working data — the ODIS project, the decoded `[TXT]`
section, the recovered names — lives outside the repository.

---

## 0. Verdict up front

| question | answer | evidence |
|---|---|---|
| Does ODIS crib the `.rod` IV search? | **No**, and structurally so | §1 |
| Is ODIS itself encrypted? | **No.** zlib page stores plus two plaintext string pools | §2 |
| Does ODIS crib the `TTTEXT` substitution? | **Yes** | §3 |
| Is the matcher sound? | **Yes** — 2,000/2,000 positive control | §3.2 |
| Measured precision | **86.6 %** on a hold-out, and most of the disagreements are ODIS correcting *us* | §4 |
| New names from one project | **18,842**, against a catalog of 14,738 | §5 |
| The unbroken numeric class | **77.1 %** of the new readings carry resolved numeric glyphs | §5.1 |
| Below the 12-letter floor | **0/11 correct.** The floor is real and ODIS does not lower it | §4.1 |

---

## 1. Why it cannot help the `.rod` container

The TEA key is already known; what the search hunts is `IV[3..8]`, five bytes. CBC gives
`P1 = D_K(C1) ⊕ IV`, so a fully known first plaintext block would hand over the IV outright,
with no search at all. Bytes 0–2 of `P1` are already pinned — `78 da` plus the deflate
header byte that `deflate_anchors` sweeps. Bytes 3–8 are *already-compressed* data: the
Huffman header of the stream.

Predicting them needs not the text but the exact output of Ross-Tech's deflate
implementation on Ross-Tech's exact input. ODIS supplies VW's German text, not Ross-Tech's
English labels, and even a byte-identical text under a different encoder produces different
bytes. There is no crib here.

The tempting exception is `TTTEXT.ROD`, whose content genuinely *is* VW's global ODX text
table. It does not help either: inside the container each record is separately enciphered,
so a perfect prediction of the ODX text still says nothing about the bytes that were fed to
the compressor.

This also leaves the shifted-IV regime untouched — that defect is in the deflate header
byte, which is a property of the stream, not of the text.

## 2. What the ODIS data actually is

Project `SK37X`, built by VW-MCD Converter 26.1.0 from `SK37X.pdx`, ODX 2.0.1. Nothing in
it is encrypted. Every `.sd.db` is a run of concatenated zlib members (`BL_LIBECM.sd.db` —
2,450 members, 386,843 B inflated), and the strings sit beside it in two pools:

| file | contents | inflated |
|---|---|---|
| `AStringData.data.gz` | 1,155,437 short names, `u32` length + ASCII | 73 MB |
| `UStringData.data.gz` | 153,704 texts, `u32` char count + UTF-16LE | 15 MB |

Both parse to the last byte in a single pass. Taken together with the `;`-split of the
`DESC` texts they yield **1,302,316 candidate strings**.

## 3. The attack

### 3.1 Why a signature lookup is the right shape

`tttext-codec.md` §1 establishes the cipher: a per-record monoalphabetic substitution over
three disjoint classes — `a-z`/`A-Z` under one permutation with case preserved, the
14-glyph numeric class `0123456789,._-` under another, everything else passing through.

Such a cipher preserves a *signature*: replace each letter by the first-occurrence index of
its lowercase within the string, each numeric-class glyph by its own first-occurrence index,
keep case, keep everything else literal. Two strings share a signature exactly when some
legal key maps one onto the other. So a signature lookup against a closed candidate list is
both sound and complete — it never misses a candidate that a key could reach, and a lookup
that returns exactly one candidate has solved the record, key and all.

That is the property an open dictionary cannot offer, and it is why a closed list is worth
more here than a bigger word list: a bigger dictionary *widens* the candidate set, while a
list of the actual strings VW ships *is* the answer set.

### 3.2 The two things that had to be got right

**The positive control.** Take 2,000 ODIS texts, encipher each under a fresh random key of
the modelled shape, and look them up. **2,000/2,000 found.** The matcher is sound; anything
it fails to find is absent from the candidate list, not lost by the method.

**The record layout.** A first attempt matched nothing at all — 14,738 of 14,738 known
records returned no candidate. The cause was `tttext-codec.md` §1.3: the payload is
`<text><sep><field>…`, with the separator drawn from the numeric class and therefore itself
enciphered. The text is field 0, so the whole payload is *never* the text:

```
000116  Ahglmi laq givxiqlgwqi57533104   ->  Intake air temperature
```

The fix is to try every prefix that ends where a numeric-class glyph sits, longest first —
a longer span is more constrained and therefore the stronger claim. With that, the same run
turned 0 matches into 220.

## 4. Precision, measured on a hold-out

The 14,738 records already read by `vagcan vcds tttext` were matched blind through ODIS and
compared. Bucketed by letters **in the matched span** (not in the record):

```
span letters   unique hit   agrees   disagrees   precision
    2-7             1            0        1          0.0 %
    8-11           10            0       10          0.0 %
    12+           209          181       28         86.6 %
```

86.6 % is a floor, not the true rate, because the disagreements were inspected and **ODIS is
right in most of them**:

| record | ODIS says | the catalog said |
|---|---|---|
| `022664` | `Continental AG` | `Overgeneral AT` |
| `024022` | `Volvo Car Corporation` | `Bombo Car Corporation` |
| `062139` | `E2E Library Profile XOR` | `E,E Runaway Failure VIA` |
| `019703` | `Matching coding` | `Duration albion` |
| `000838` | `Generator DF Signal` | `Generator BY Signal` |
| `054747` | `Compressor on` | `Compressor of` |
| `044289` | `Average wheel speed` | `Average speed wheel` |
| `017871` | `Shut-off from Engine Control Module (ECM) via CAN` | `Shut_off from …` |
| `101934` | `development message S101` | `development message S` |

`DF` is the alternator terminal; `E2E` is end-to-end; the `-` in `Shut-off` and the `101` in
the last row are numeric-class glyphs the existing solver cannot resolve at all. So the crib
is not only new coverage — it is an **error-corrector for the catalog we already ship**.

### 4.1 The 12-letter floor is real

Every unique hit on a span shorter than 12 letters was wrong: 0 for 11. A short span has
too little structure, and a candidate list of 1.3 M strings is large enough that a spurious
unique hit is ordinary. `MIN_LETTERS = 12` is confirmed by this experiment rather than
undermined by it, and the safe set below is cut at the same place.

## 5. Yield

Over the 177,731 records not already in the catalog:

```
span letters   unique   ambiguous
    2-7          5,661     52,511
    8-11         9,825      6,995
    12+         18,842      9,551
```

The **safe set is the 18,842 unique matches on spans of 12 letters or more**. Against a
current catalog of 14,738 that is a catalog of 33,580 — more than double, from a single
ODIS project, at a measured precision of at least 86.6 %.

### 5.1 The numeric class gives way

`tttext-codec.md` §6 records the 14-glyph numeric class as unbroken and independent of
everything held at the time. A signature match breaks it for the matched record: the
candidate supplies the plaintext of the numeric positions directly, so the permutation
falls out with the letters.

**14,529 of the 18,842 safe readings (77.1 %) carry numeric glyphs** — `Glow time control
module 2`, `Exhaust gas temperature sensor 1`, `Standard - ambient data 1`. These are the
first digits recovered by any means.

## 6. Limits, stated plainly

- **Coverage is bounded by the candidate list, not by the method.** 14,263 of the 14,738
  known records found no candidate at all: their plaintext simply is not in this project.
  Only 188 of the 14,738 known plaintexts appear verbatim anywhere in the ODIS pools.
- **This is one project.** `SK37X` declares `<LANGUAGE>deu</LANGUAGE>`; the matches are the
  texts it happens to carry in English. A wider or English-language ODIS set should raise
  the yield, and how far is unmeasured.
- **The safe set is unverified individually.** 86.6 % measured on a hold-out means roughly
  one in eight is wrong, and a wrong name reads exactly like a right one — the same hazard
  `tttext-codec.md` §7 gates against. Nothing here should reach a shipped catalog without
  the same gate, or better, corroboration from a second ODIS project.
- **Ambiguous hits were discarded, not resolved.** 9,551 records at 12+ letters have more
  than one candidate. A second project, or a language model over the candidates, would
  decide many of them; neither was tried.

## 7. What to do with it

1. Re-run the pilot with a second ODIS project and keep only readings both agree on. That
   converts "86.6 % precision" into something that can be shipped without a gate.
2. Feed the safe set back as vocabulary to `vagcan vcds tttext`. The solver bootstraps on
   words it has read; 18,842 in-domain names is far more vocabulary than it has ever had,
   and the records it then solves are *independent* corroboration of the crib.
3. Resolve the ambiguous 9,551 by requiring the letter permutation to be consistent with a
   neighbouring record already solved — records with adjacent ids often share a text.

Nothing from this analysis belongs in the checkout. The ODIS data is VW's exactly as the
label files are Ross-Tech's, and the recovered names are derived from both.
