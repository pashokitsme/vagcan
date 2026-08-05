# `MUX.rod` — opened, read, and it is not the registry

One of the two files `research/labels/label-linkage.md` §5.5 named as the last places a **global
measurement registry**, and with it the per-ECU read identifier, could still hide. It opens
in under a minute, its record grammar falls out cleanly, and its seventeen fields all have
identifiable roles. **None of them is a read identifier**, and the file's shape rules out
its being a per-ECU registry at all.

Companion to `research/labels/rod-labels.md` (§4.0c, §5) and `research/labels/label-linkage.md` (§2, §3,
§5.5). `research/labels/tttext2.md` covers the other candidate and is not this work.

Working data: a VCDS 26.3 English installation at `~/vcds-en/` (read-only, never
committed). Nothing under `crates/` was changed for this work.

---

## 0. Verdict up front

| question | answer | confidence |
|---|---|---|
| Does it decrypt and inflate with the existing tooling? | **Yes** — §1, 51.6 s wall | very high (exact inflate, valid Adler-32) |
| Is the record grammar recoverable? | **Yes** — 17 fields, 11,350 of 11,350 tables — §2 | very high |
| Are the numbers readable? | **Partly** — 50 tables / 9,533 rows (21 %) — §3 | high, and the coverage limit is stated |
| Does anything in it bind a measurement to a read identifier? | **No** — four tests, §5 | high |
| Could it be a per-ECU registry at all? | **No** — its median table has **3 rows** — §5.1 | very high |
| Is it useful for anything? | **Yes, but not reachable from a car** — §4, §6 | see §6 |

The one-line answer: `MUX.rod` is exactly what its name says — ODX multiplexers — and it
sits **downstream of `STRUC.rod`**, reached only from a `STRUC` parameter of kind 6
(§4.1, 835 of 835). It is a leaf of the same subgraph that `rod-labels.md` §4.0c already
showed a car cannot enter. It holds fine scaling factors (§4.3) that the project has wanted
for four writeups, and there is still no edge from a control unit to them.

---

## 1. It opens

`~/vcds-en/UDS_EV/MUX.rod` is 522,102 bytes: a single `[MUX]` section, header
`07 f9 70 | 21 64 a1` — the `0x800000` flag clear (so zlib), 522,096 stored cipher bytes,
2,188,449 plaintext bytes. `product != 0`, so it is blocked exactly like `STRUC` and
`TTTEXT` and yields to the same offline search.

```
cargo build --release -p vagcan --features rod-crack
vagcan vcds rod ~/vcds-en/UDS_EV/MUX.rod --cache catalogs/rod-iv-cache.json --dump DIR
→ [MUX] zlib 2188449 bytes            IV[3:8] = f4 6a c0 0d 18
→ 51.6 s wall / 404.6 s CPU at 787 %  (this machine, 8 effective cores)
```

That is the post-speedup searcher of `label-linkage.md` §9 doing what §9 predicted: a
minute, not the hours §7 budgeted. The recovered key is committed to
`catalogs/rod-iv-cache.json` (it belongs to this 26.3 English install; the cache is keyed
by file basename and tag, so a key from a different VCDS generation simply fails to
inflate rather than decoding to nonsense).

For the joins below the same command was run on `STRUC.rod` (297,499 B), `TTDOP.rod`
(3,524,175 B), `UNIT.ROD` (3,370 B) and `TTTEXT.ROD [TXT]` (7,623,242 B); all four opened
first time. Their keys are *not* committed — `rod-labels.md`'s figures for `STRUC` come
from an older install (293,560 B there, 297,499 B here) and one cache cannot serve both.

## 2. The record grammar — seventeen fields, 11,350 of 11,350

44,471 rows of `NNNNNN,<payload>`, under **11,350 table ids** running 1 … 19,911, the ids
plaintext and monotonically ascending. The payload alphabet is the usual fourteen ODX
glyphs (1,665,483 characters) plus a flat `A`–`Z` band (20,942 characters, 658–1,026 each)
and a trace of lowercase — the same three-class picture as `tttext-codec.md` §1.1.

Applying `label-linkage.md` §2.1's separator model: per table, take the glyph that occurs
the same number of times in every row of that table.

> **Every one of the 11,350 tables has exactly one such glyph that yields 17 fields.**
> 11,350 / 11,350, no exceptions, including the 5,378 tables with only one or two rows.

For comparison, `STRUC` gives eleven fields and `TTDOP` four. Seventeen is the record
layout, not an artefact of the search: 3,154 tables resolve to a *unique* separator without
being told the field count, and all 3,154 of those give 17.

### 2.1 The rows are sorted, and the sort key is field 5

The ordering attack of `whole-car-survey.md` §3 needs to know what the rows are sorted on.
Testing each field in isolation over the 809 tables with ≥4 non-letter rows — constraints
from consecutive rows, a cycle meaning "not sorted on this", a *length violation* meaning a
longer number sorted before a shorter one:

| key | tables with variation | cycles | length violations | shuffled control |
|---|---|---|---|---|
| **f5** | 797 | **0** | **0** | 139 cycles, 2,222 violations |
| **f6** | 797 | **0** | **0** | 183 cycles, 2,059 violations |
| f7 | 270 | 60 | 2,411 | 52 / 3,329 |
| f13 | 129 | 80 | 471 | 114 / 1,517 |
| f16 | 204 | 107 | 1,312 | 117 / 1,809 |

Zero and zero, twice, against a control that produces hundreds of each. Field 5 is the sort
key and field 6 tracks it.

### 2.2 The default case is a letter row

10,693 rows carry letters. In **10,682** of them the letters sit in exactly fields 5 and 6;
in **10,622** the two fields hold the *same single letter*; **10,438** are the first row of
their table; and 10,372 of the 11,350 tables have exactly one such row. The 26 letters are
used flatly (328–513 times each), i.e. one plaintext letter under `TTTEXT`'s per-record
letter substitution.

A multiplexer case that has a name but no limits, sorting before every numbered case, is
the ODX **`DEFAULT-CASE`**.

## 3. Reading the numbers — and the honest coverage limit

`crates/vag-data/src/glyphs.rs`'s `digit_order`, fed the f5/f6 constraints, pins a complete
ten-digit alphabet for **50 tables = 9,533 rows = 21 % of the file**. The other 11,300 fail
for the reason `whole-car-survey.md` already measured: a table needs roughly five rows
before its order pins ten glyphs, and MUX's median table has **three**. The 50 are selected
by row count alone — nothing about their content — which is what makes them usable as a
sample below.

32 of the 50 use twelve or thirteen glyphs, i.e. they carry punctuation as well as digits.
`rod-labels.md` §5.3's rule assigns those roles, and it holds here without exception: of
the 43 extra glyphs, **38 appear only inside fields 9/10 and never at an edge** (a decimal
point) and **5 appear only at the start of field 9** (a minus sign). 43 of 43, no residue.

## 4. What the seventeen fields are

Each row was checked against something that could have said no.

| field | reading | the check that could have failed |
|---|---|---|
| f0 | switch-key byte position | constant within all 11,350 tables; values 0,1,2,8 |
| f1–f3 | reserved / zero | decode to 0 in 9,533, 9,151, 9,129 of their rows |
| f4 | **switch-key bit length** | ∈ {5,8,16,32}; every case limit ≤ 2^f4−1 in **9,151 / 9,151**, and the ceiling is *tight* — the 16-bit group reaches 65,105 (99.3 %) and the 8-bit group 237 (93 %) |
| f5, f6 | **case lower / upper limit** | `f5 ≤ f6` in **9,533 / 9,533**; and it is the sort key (§2.1) |
| f7 | **`TTDOP` reference** | a valid `TTDOP` id in **99.7 %** of 5,521 rows, against a 61.3 % density baseline |
| f8 | kind code | `f8 = 3` ⟹ f7 present and no linear triple: **2,681 / 2,681**. `f8 ∈ {0,128}` ⟹ f9, f10, f11 all present: **2,999 / 2,999** |
| f9, f10, f11 | offset, numerator, denominator | §4.3 |
| f12 | **`UNIT.ROD` id** | in range for every decoded row; `UNIT` decodes to `%`, `°`, `/qwj` (= `/min` under its own letter key), `_/c²` (= `m/s²`) |
| f13 | byte offset | 0 … 16 |
| f14 | **bit offset** | ∈ 0…7 in **9,533 / 9,533** |
| f15 | bit length | 1 … 192 |
| f16 | **`TTTEXT` name id** | a valid `TTTEXT` record id in **8,386 / 8,386 = 100.0 %**, against a 42.7 % density baseline |

f16 at 100.0 % over 8,386 rows is the single strongest identification in the record — the
same test that pinned `STRUC`'s field 9 (`rod-labels.md` §5.2, 3,837 / 3,837).

### 4.1 Where MUX sits in the corpus graph — proven, not assumed

`STRUC.rod` was solved by the same method (64 tables) and its reference field tested
against every id space in the corpus:

| `STRUC` kind (`f1`) | rows with a reference | → `TTDOP` ids | → `MUX` ids |
|---|---|---|---|
| 3 | 129 | **100.0 %** | 20.2 % |
| 6 | 835 | 50.5 % | **100.0 %** |

Two disjoint kind codes, each landing wholly inside one id space and at or below the
density baseline in the other. **`STRUC` kind 3 references a `TTDOP` compu-method; `STRUC`
kind 6 references a `MUX` table.** That is the edge, and it is the *only* one found:
nothing in a per-ECU `.rod` references a MUX id (§5.1).

### 4.2 A worked table

`MUX 018167`, joined to `catalogs/names-uds.json`:

```
case 53520..53520   kind 0   byte 0 bit 0 len 16   ×1/100   unit 9   text 371365
                                                  → "ECU Power Supply Voltage"
case 18665..18665   kind 3   byte 0 bit 0 len  8   TTDOP 26867          text 417025
case 56835..56835   kind 0   byte 0..3 (4 rows)    ×100/255 unit 1 (%)  text 417535 …
```

The name comes from the letter break (`tttext-codec.md`), the numbers from the row-order
break (`whole-car-survey.md` §3) — two independent decodes agreeing on a physically sane
reading: a 16-bit quantity in units of 10 mV called a supply voltage.

### 4.3 MUX carries the fine scalings

2,875 linear rows decode to **78 distinct factors**, and they are the right shape:

```
0.01 (575)   0.0439453125 = 45/1024 (254)   0.1 (177)   0.0009765625 = 1/1024 (131)
0.001 (101)  0.25 (64)   0.0078125 = 1/128 (55)   0.5 (54)   0.03125 (52)
6.103515625e-05 = 1/16384 (48)   …           offsets: −327.68, −50, −40, 0
```

Binary fractions and a −40 temperature offset are what a VW measurement scaling looks like.
This extends `rod-labels.md` §5.3 (which found `0.001` and `0.01` in `STRUC` and withdrew
the claim that the corpus could not express them): the fine factors are real, and a large
share of them live in `MUX`. `0.75 / −48` is still absent, and our gearbox's proven `×0.4`
does not appear in the 2,875 rows read here — on 21 % coverage that is not evidence either
way.

## 5. The question this was opened for: no read identifier, four ways

Ground truth: 1,640 distinct identifiers that answered on the reference car — the 15 units
of `research/dumps/survey-parked.jsonl` plus the engine and gearbox full sweeps — and the
thirteen crib DIDs of `rod-labels.md` §4.0c.

**Test 1 — is any field the identifier?** All 17 fields, over the 9,533 decoded rows,
distinct values in `0…0xFFFF`, against a windowed null (the identifier set's local density
around each value):

| field | distinct values | observed hits | expected | ratio |
|---|---|---|---|---|
| f5 | 380 | 10 | 7.9 | 1.26 |
| f6 | 383 | 11 | 8.0 | 1.37 |
| f7 | 203 | 12 | 7.3 | 1.65 |
| f16 | 38 | 1 | 0.5 | 1.86 |
| every other field | — | ≤1 | — | — |

Nothing. **Zero** of the thirteen crib DIDs occurs in any field. f7's 1.65 is on twelve
hits, and f7 is independently identified as a `TTDOP` reference at 99.7 % — it is not a
spare column for an identifier.

**Test 2 — is any *table*'s switch key an ECU's identifier list?** Each of the 50 decoded
tables against each of the 17 real identifier sets, 850 comparisons:

```
pooled  observed 67   expected 437   ratio 0.15
best single table×unit:  7 observed vs 2.8 expected  (over 850 comparisons)
```

Below chance in aggregate, and the best single cell is what 850 draws produce for free.

**Test 3 — key-free, over the whole file.** Digit substitution does not change how many
digits a number has, nor the order of the rows. So: is there an injective glyph→digit map,
*consistent with the table's own row order*, carrying a table's case values into a real
unit's identifier list? Over all 46 tables with ≥6 distinct case values, × 17 units:

```
REAL identifier lists                    1 match   (table 001044, 6 values, all 3-digit)
NULL random sets of the same size        0, 0, 0
NULL the same real lists shifted by k     0,1,1,1,2,0,1  for k = 1,3,7,13,29,61,127
```

One match against a structured null averaging 0.86. That is chance, and the matched values
(`0x2ff`, `0x30c`–`0x30f`, `0x320`) are the least informative kind. An earlier version of
this test reported four matches; it had two bugs — it did not enforce injectivity *within*
a value and it ignored the row order — and both are fixed above. The four "matches" were an
artefact of the first bug (they mapped two glyphs to the same digit).

**Test 4 — the standard identifiers.** Every UDS control unit on this platform answers
`F186`, `F187`, `F189`, `F18C`, `F191`, `F19E`, `F1A2`, `F1A3`, `F1DF`. Of the eight
decoded tables with a 16-bit switch key, **none contains a single one of them.**

### 5.1 And it could not be a per-ECU registry anyway

The corpus has 16,576 per-ECU `.rod` files. A control unit's measurement list runs to
hundreds of entries — the cracked engine `MWB` has 1,089 rows, the gearbox 1,020, and our
own engine answered 896 identifiers in a sweep. `MUX.rod` has:

```
11,350 tables / 44,471 rows      median 3 rows per table, mean 3.9
tables with ≥10 rows: 147    ≥100 rows: 33    ≥500 rows: 5    largest: 800
median distinct case values per table: 2
```

Three-quarters of the file is two- and three-case multiplexers. There is no room in it for
one measurement list per control unit, and no key by which a control unit could ask for
one: `label-linkage.md` §3 showed the only per-ECU payload is `(text-id, 2-char code)` with
the code a global function of the text-id, and the code's 1,600-value space cannot address
11,350 tables in any case.

### 5.2 The one piece of contrary evidence, stated rather than buried

Some 16-bit tables' case values read like data identifiers in hex — `018167` has
`0x481D`, `0x48E7`–`0x48F8`, `0xD00B`, `0xD110`, `0xDD00`, `0xDE00`–`0xDE12`, in runs, and
`0xD110` is named "ECU Power Supply Voltage" with a ×1/100 volt scaling (§4.2). An ODX
multiplexer switching on a 16-bit key at a byte offset is also exactly how one would model
"the identifier echoed in a `62` response selects the layout of what follows".

That reading is *not* adopted here, for the reasons above: no such table matches any of our
seventeen real identifier lists (tests 2 and 3), none carries a standard identification
DID (test 4), and the file is the wrong shape and size to hold per-ECU lists (§5.1). It is
recorded as the strongest thing pointing the other way, and as the thing that would have to
be attacked to overturn this note — most cheaply by decoding the `TTTEXT` names of one such
table's cases and seeing whether they read as one control unit's measurement menu.

## 6. What closes, and what this leaves

**Closed.** `label-linkage.md` §5.5 and `rod-labels.md` §5's "`TTTEXT2.ROD` and `MUX.rod`
are the only uncracked files that could hold a global measurement registry". `MUX.rod` does
not. It is the ODX multiplexer table, reachable only from `STRUC`, carrying case limits,
layouts, units, names and scalings — and no identifier and no per-ECU column. Half of that
sentence is now settled; `TTTEXT2` is the other half.

**Do not re-run:** any search for a read identifier inside `MUX.rod` (§5, four tests); the
"MUX table = an ECU's DID list" hypothesis (§5.1, cardinality; §5.2 says what evidence
would revive it); `MUX` id ← 2-char code (1,600 codes cannot address 11,350 tables).

**No decoder change is shipped.** A `vag_data::mux` parser would be correct and unreachable:
the only way into MUX is a `STRUC` id (§4.1), and nothing a car says yields one. That is the
same trap the deleted `struc` module fell into, and this note is not going to re-dig it.
What *is* committed is the recovered key, because it is the expensive part and it is data,
not code.

**The lever this moves.** The bottleneck is unchanged and now sharper: the corpus holds
names (`TTTEXT`), layouts (`STRUC`), enumerations (`TTDOP`), units (`UNIT`) and — as of
this note — fine linear scalings (`MUX`), all keyed to one another and none of them keyed
to anything a control unit reports. Every one of those tables is now open. The missing edge
is the single hop from a per-ECU `.rod` row to a `STRUC` id, and it is the only thing left
worth attacking on this side of the problem.

## 7. Reproduction

```
cargo build --release -p vagcan --features rod-crack
vagcan vcds rod ~/vcds-en/UDS_EV/MUX.rod --dump /tmp/mux     # 52 s, IV[3:8] f46ac00d18
vagcan vcds rod ~/vcds-en/UDS_EV/STRUC.rod  --dump /tmp/struc
vagcan vcds rod ~/vcds-en/UDS_EV/TTDOP.rod  --dump /tmp/ttdop
vagcan vcds rod ~/vcds-en/UDS_EV/UNIT.ROD   --dump /tmp/unit
vagcan vcds rod ~/vcds-en/UDS_EV/TTTEXT.ROD --dump /tmp/tttext
```

The analysis is a few hundred lines of throwaway Python over the inflated blobs, done under
a scratch directory and not added to `research/clb-crack`. The steps are small enough to
restate: split each payload on the per-table separator; check the field count; derive glyph
order from consecutive rows on field 5 and topologically sort it (`glyphs::digit_order`);
then the membership counts of §4 and the four tests of §5. The checks worth re-running if
any of this is touched are the two zeroes of §2.1, the 8,386 / 8,386 of §4, the 835 / 835 of
§4.1, and the shifted-list null of §5 test 3.
