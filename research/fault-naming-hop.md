# The last hop: naming a VW fault number

`research/codes-dat.md` §4 left one thing between this tool and a named fault: a VAG
control unit answers `0x19` with a **VW-internal 24-bit number**, and `Codes.dat` is keyed
by something else. This file identifies the hop, verifies it end to end on ten independent
faults across two cars, and then says precisely why it is still **not shippable** — which
is the honest half.

Read `research/codes-dat.md` §5 and `research/whole-car-survey.md` §3 first. This file
**supersedes three of their statements** (§7) and refutes two proposals that looked cheap.

---

## 0. Verdict up front

| question | answer | confidence |
|---|---|---|
| Where does the pairing live? | **`UDS_EV/RD.rod [DTC]`**, keyed by the raw number in decimal — §2 | very high |
| What is in a row? | `f0` = a `Codes.dat` key (the name), `f1` = the failure-type byte, `f2` = a second id space — §3 | very high (10/10, and `f1` derived from `f0`) |
| Does the chain produce VCDS's own answer? | **Yes, 10 of 10, 0 wrong** — §4 | very high |
| Can `vagcan faults` do it blind? | **No, and for two separate reasons** — §5 | high, clean negative |
| Pool ordering constraints across tables? | **Refuted** — the alphabet is per-table — §5.1 | very high (95 tables → 95 alphabets) |
| A different `RD.rod` section? | **No such thing** — the file has two sections — §5.3 | certain |
| Ship the units that "already answer ISO DTCs"? | **Refuted — it names nothing** — §6 | very high |
| Which row of the table is *this* car's? | **The unit's own `.rod` says: its `[DTC]` ids are 1-based `RD.rod` row numbers** — §10 | very high (22/22, 33/33, 2/2) |
| Does that also break the substitution? | **No. Untouched** — §10.3 | very high |

The chain is **identified and verified**. It is not **reachable**, which by this project's
own rule (`research/mux.md`) means this is a writeup and not a feature.

**§10 closes obstacle 5.2 outright** and leaves 5.1 exactly where it was. Read §10 before
§8, which it corrects.

---

## 1. The Rosetta stone was in the repository

Four VCDS Auto-Scans sit in `research/VCDS-RUS/Scans/` (cp1251) — two of the reference car
`XW8AD4NE9JH008917` and two of a second, unrelated VW `WVWZZZAUZFW063260`. **VCDS prints
both numbers, on consecutive lines:**

```
0297 - Датчик угла поворота рулевого колеса
          B1168 F2 [10001001] - отсутствует инициализация
```

and `Code-RUS.dat[9529586]` is *"Датчик угла поворота рулевого колеса: отсутствует
инициализация"* — **the two printed lines are that one string split at `": "`.** So VCDS
names a fault through exactly one text source, and the display is component + symptom.

Extracting every such pair gives **38 distinct `(raw VW number, SAE code, failure type,
component text)` quadruples over 15 control units and two cars**. That is the crib set,
it needed no reverse engineering, and it is what the rest of this file is measured
against. It is reproduced in `research/rd-rod/pairs.tsv`.

Two facts it settles immediately:

* **The map is many-to-one and therefore a table, not arithmetic.** `0x00431A` (engine),
  `0xD01732` (parking aid), `0xC45201`, `0x005704` and `0xD01600` all name `U1123 00`. No
  function sends one number to two answers *and* five numbers to one; and `B1168 F2` is
  reached from `0x000129` on the brakes and from `0x003F93` on the steering assist.
* **The low byte is not the failure type.** `0x003F93` and `0x003F08` both name `B1168`
  `F2`. The failure type is not in the number the car sends at all.

---

## 2. `RD.rod [DTC]` is the registry, and its key is the raw number in decimal

`~/vcds-en/UDS_EV/RD.rod` holds exactly **two** sections, `[CMP]` (26 bytes of build
stamp) and `[DTC]`. `[DTC]` needs `IV[3..8] = 5c b0 48 d4 3f`, inflates to 6 577 695 bytes
and holds **236 755 rows in 110 767 tables**.

```
row := <table key> ',' <payload>
```

The key is plaintext: 62 400 keys of six digits, 48 365 of eight, two malformed. Rows
sharing a key are one table, and **the key is the raw 24-bit DTC written in decimal,
zero-padded** — every one of the 38 crib numbers is a key, as are all 18 raw codes the
reference car reports.

Table sizes are the whole problem: 61 100 tables have one row, 25 415 two, 10 716 three,
5 343 four, and only **8 193 have five or more**.

---

## 3. The row grammar

The payload is written in a per-table substitution alphabet over the 14 glyphs
`0-9 . - _ ,` — ten are digits, one is the separator, three are unused
(`research/label-linkage.md` §2.4). **The separator is the glyph occurring exactly six
times in every row of the table**, which identifies it in 108 819 of 110 767 tables and
splits each row into **seven fields**:

```
f0 <sep> f1 <sep> f2 <sep> <sep> <sep> <sep>
```

| field | what it is | evidence |
|---|---|---|
| `f0` | a **`Codes.dat` key** — the name | §4: 10/10 reproduce VCDS's text |
| `f1` | the **failure-type byte**, two hex digits, digits under the table's alphabet and `A-F` under a second letter substitution | 10/10 agree with VCDS's printed FTB; and for every `f0` in the `B/C/U` band, `f1` **is** the low byte of `f0` |
| `f2` | 190 000–450 000, a different id space | `whole-car-survey.md` §3, unchanged |

That `f1` is derived from `f0` is the same observation `whole-car-survey.md` §3 recorded
as *"the two-character code is a function of field A, not of field B (97.3 %)"*. It is now
explained rather than measured — and §5.2 turns on the consequence.

---

## 4. Breaking the substitution with the far half — and the chain verified

`crates/vag-data/src/glyphs.rs` recovers a table's alphabet from the order its rows are
stored in. It needs a table to pin all ten digits by itself, which takes about ten rows,
and the reference car's tables have one to sixteen. **The far half of the chain is a much
stronger constraint: every `f0` must be one of the 34 716 keys of `Codes.dat`** — 20 059
of six digits, 3 107 of seven, 4 739 of eight. A table with a handful of distinct `f0`
values is then over-determined, and the ordering constraints prune what is left.

The solver is `research/rd-rod/solve.py`: a DFS over `f0` values longest-first, each
placed against a real key, with the ordering constraints as a filter. Two escapes are
explicit and both are reported — one `f0` is allowed not to be a key (`codes-dat.md` §5
measured 47 of 48 present), and a node budget that is hit is reported as *not exhausted*
rather than as "no solution". Without the second, table `000531` reads as a refutation.

**Result over the 38 crib pairs: 10 tables solve to a unique alphabet, and all 10 then
contain, as a row, exactly the component text VCDS printed. Zero wrong.**

| raw | VCDS | `Codes.dat` key found | text |
|---|---|---|---|
| 297 | `B1168 F2` | 137224 / 9529586 | Steering angle sensor / **: Not Initialized** |
| 527 | `B1065 13` | 136965 | Rear Left Bass Speaker |
| 531 | `B11FF 01` | 137375 | Footwell Illumination |
| 6922 | `B1218 15` | 137400 | Bulb for Low Beam Headlamp Right |
| 12289 | `U1011 00` | 153265 | Supply voltage: Voltage too Low |
| 32807 | `C10BD 01` | 120669 | Parking Brake Motor; Right |
| 66052 | `U0065 00` | 149253 | Infotainment CAN Bus (SAE Bus E) |
| 131602 | `U101D 00` | 153277 | Power Steering Control Module: No Communication |
| 589825 | `U10BA 00` | 153434 | Local Data Bus: No Communication |
| 13637120 | `U1123 00` | 153539 | Databus: Received Error Message |

In every one of the ten the row's `f1` decodes to VCDS's printed failure type as well.
Ten independent faults, two cars, eight control units, one rule, nothing tuned. **The hop
is `raw DTC → RD.rod table → f0 → Codes.dat`, and it is right.**

Two of those ten are the reference car's own confirmed faults, named end to end from
VW's own files: **`713` fault `000129` → "Steering Angle Sensor: Not Initialized"** and
**`70E` fault `000213` → "Footwell Illumination"**.

The reference car's EPS codes resolve too, once their table is solved: `007680`'s rows
carry `10489840` = "Internal Fault" and `229504`'s single `f0` fits `140960` = "Internal
Control Module Memory Check Sum Error" — which are exactly `B200F F0` and `B2000 00`, the
codes `research/eps-j500-report-ru.md` records VCDS naming on this car.

---

## 5. Why it still cannot be shipped

Two obstacles, independent, both measured. Neither is the one `codes-dat.md` §5 expected.

### 5.1 The alphabet is per-table, and pooling is refuted

The cheapest hoped-for win was that the substitution is a property of the *file* or of
some grouping larger than the table, so constraints from thousands of tables could be
intersected and a four-row table would inherit a solved alphabet.

**It is not.** Running the ordering attack over the 8 193 tables with ≥5 rows solves 95 of
them, and those 95 tables have **95 distinct alphabets**. Not one is shared. The alphabet
is keyed by the table and nothing coarser, so there is nothing to pool.

What that leaves is the crib solver of §4, and it runs out on small tables:
**28 of the 38 crib tables stay ambiguous**, from 3 candidate alphabets up to 10 126. A
table with one distinct `f0` of six glyphs simply does not carry ten digits' worth of
constraint, whatever it is intersected with. Adding the `f1`-is-the-low-byte-of-`f0`
constraint (§3) prunes hard — `016275` goes from 11 alphabets to 3 — but resolves nothing
to unique.

### 5.2 Even solved, nothing in `RD.rod` says which row is *this* car's

This is the one that matters, and it was not on the list.

A table is not one answer. Table `000297` has 36 rows and 23 distinct `f0`, naming a
front right wheel speed sensor, a camera, an auxiliary heater control module and a
steering angle sensor. Table `000531` has 50 rows and 38 values; `007680` has 16 and 8.
The same raw number means different things on different control units — which is
precisely why the registry is a table and not a function (§1).

`f1` cannot be the selector, because §3 shows it is **derived from `f0`**: for every
`f0` in the `B/C/U` band, `f1` is literally `f0`'s low byte in hex. A field computed from
another field carries no information about anything outside the row.

So the selector is **external — the control unit's ODX dataset**, which VCDS has and this
tool does not join on. Verified in §4 only because the crib supplied the answer; blind,
the tool would have 23 candidates for fault 297 and no ground to choose.

Stated as a table, for the reference car's own eighteen confirmed codes:

| obstacle | codes affected |
|---|---|
| substitution underdetermined (§5.1) | `70A` ×5, `70C 047120`, `70E 060901`, `710 010405`, `712 004F04`/`004D04`/`038080`/`003F08` — 11 |
| solved, but 8–38 rows and no selector (§5.2) | `70E 000107`/`000213`, `712 001E00`, `713 00004B`/`00005B`/`000129` — 6 |

Zero are nameable blind. That is why nothing was merged.

### 5.3 `codes-dat.md` §5 note (b) has no surface

The suggestion was that the pairing might live in a *different* `RD.rod` section, since
only `[DTC]` had been decoded, and that `rod.rs`'s shifted-IV support might have opened
sections that were unreadable before. **`RD.rod` contains exactly two sections, `[CMP]`
and `[DTC]`.** There is nothing else in the file. Checked first because it was cheap; it
cost five minutes and is a dead end, not a lead.

---

## 6. Refuted: "some units already answer ISO DTCs, ship those"

Arithmetically the observation is real. On the second car `9440027 = 0x900B1B` is exactly
`B100B` + FTB `1B`, and `10485833 = 0xA00049` is `B2000` + `49`; the airbag and cluster
report in that form.

**It names nothing, on either car.** `Codes.dat`'s high band covers only **2 306 distinct
16-bit codes**, and it is sparse in the failure-type dimension — a mean of 13 failure
types each, concentrated on `F0..F7`. Concretely:

| code | entries in `Codes.dat` |
|---|---|
| `B2000` | `F0 F1 F2 F3 F4 F5` only — **not** `49`, the one the car reports |
| `B100B` | **none at all** |
| `B100A` | **none at all** |
| `U1014` | **none at all** |

And on the reference car the case does not arise: **none of its eighteen raw codes is a
`Codes.dat` key**, including all five of `70A`'s `D0172x`. The claim that `70A` answers
ISO DTCs directly is refuted by VCDS itself — it prints `U1123 00` for `D01732`, not
`U1017 32`, and `0xD01732` is absent from `Codes.dat`.

A "direct band" decoder would therefore be a decoder nothing reaches: the `mux.md` failure
mode exactly. Not written.

---

## 7. Corrections to earlier writeups

1. **`codes-dat.md` §5** — *"`RD.rod` rows are cross-references, not self-identification …
   the reference car's own tables do not name themselves."* Half right. The rows are not
   cross-references: `f0` is the name, and the table for a fault does contain that fault's
   own answer (10/10, §4). What is true is that it contains **other units'** answers too,
   and nothing inside the file picks between them (§5.2).
2. **`codes-dat.md` §5** — *"field 0 is an ISO DTC."* Too narrow. `f0` is a `Codes.dat`
   key, and the six-digit ones are **component names with no failure type**
   (137375 "Footwell Illumination"); the failure type is `f1`. Reading `f0` as an ISO DTC
   works only for the eight-digit rows.
3. **`whole-car-survey.md` §3** — *"only 1 966 of 64 205 large-key tables contain their own
   code with the `0xF0` byte, so a row is not the fault describing itself."* The test was
   whether a decoded value equals the table key. The table key is a VW number and `f0` is
   a `Codes.dat` key; they are different spaces (`codes-dat.md` §4), so they were never
   going to be equal. The measurement is correct and the inference does not follow.

---

## 8. What would settle it

In order of cost, and the first two are the whole of it:

*(Item 1 was done and it worked — but not the way it is described here. **§10 supersedes
it**: the per-unit ids are not `RD.rod` table keys, they are `RD.rod` row numbers, and the
"180 of 274" below is a coincidence. Item 2 stands untouched and is now the whole of what
is left. The paragraph is kept as written so the correction has something to point at.)*

1. **The row selector — the per-unit ODX join.** VCDS knows which unit it is talking to
   (`EV_Brake1UDSContiMK100ESP 036010`) and this is the only thing it has that we do not.
   The per-unit `.rod` files carry a `[DTC]` section of `<6-digit id>,<2-char code>` rows
   — 274 of them for the DQ200, of which **180 are `RD.rod` table keys**. That is the
   shape of the join and it is worth the ~35 CPU-seconds per file the `rod-crack` IV
   search costs. Caveat, and it is a real one: **the parking aid's own `.rod` has no
   `[DTC]` section at all** and it reports five faults, so this cannot be the whole
   mechanism. The steering column module (`EV_SMLSVALEOMQBLRH.rod`, 935-byte `[DTC]`) is
   the cheap test — it is the unit that reports `291104`.
2. **The substitution on small tables.** The crib solver is already the strongest
   constraint available and it leaves 28 of 38 open. The remaining leverage is either
   `f2` (a second id space, whose valid set is unknown) or reversing the routine that
   generates the alphabet from the table key — the `MT`/`KS` machinery that
   `codes-dat.md` §2.2 read straight out of `VCDS-ARM.exe` rather than attacking. With
   95 solved `(key, alphabet)` pairs in `/tmp/solved_alphabets.txt` there is a fit set to
   check any candidate against instantly.
3. **More scans.** Every VCDS scan of any VAG car is more crib pairs, free, and they
   double as the acceptance set. 38 pairs came from four scans.

What would refute the whole model: a table whose unique alphabet places an `f0` on a
`Codes.dat` text that contradicts what VCDS printed for that fault. Ten tries, zero so far.

---

## 9. Reproducing

Nothing here needs the car. `research/rd-rod/` holds standalone Python — deliberately not
shipped code, and `crates/vag-data/src/rod.rs` and `codes.rs` remain authoritative:

* `rod.py` — `.rod` container, TEA-CBC, the block-0 IV, the shifted-IV regime;
* `codes.py` — `Codes.dat` / `Code-RUS.dat`, the per-record IV (pinned against key
  9 529 586 → `47 02 c8 cd 6c 50 dc d3`);
* `tables.py` — `[DTC]` as tables, plus the ordering attack;
* `sweep.py` — §5.1, the 95-alphabets refutation;
* `solve.py` — §4, the `Codes.dat` crib solver;
* `unitjoin.py` — §10, the row selector: unit `.rod` `[DTC]` index → `RD.rod` row → name;
* `pairs.tsv` — the 38 crib pairs from the owner's own scans.

---

## 10. The row selector, found: a unit `.rod` `[DTC]` id is an `RD.rod` **row number**

§8.1 was right that the selector is the per-unit ODX file and wrong about how it points.
The pointer is not an identifier in any shared id space — it is a **1-based row index
into `RD.rod`'s `[DTC]` section**, and it names one row, not a set.

```
raw 24-bit DTC ──▶ RD.rod [DTC] table (key = the number in decimal, zero-padded)
                        │  … 2 to 50 rows, and nothing inside the file picks one
unit's F19E ──▶ EV_*.rod [DTC] ──▶ <index>,<2-char code>
                        │  index-1 IS the row, and its table key IS the raw DTC
                        ▼
                     f0 ──▶ Codes.dat ──▶ the text
```

### 10.1 The evidence

Four unit files cracked (§10.4). For each, every `[DTC]` id was resolved as
`RD.rod` row `index - 1`, and the table key of that row was compared with what the car
actually reported in `research/dumps/survey-parked.jsonl`:

| unit | file | `[DTC]` rows | distinct table keys | reported by the car | covered |
|---|---|---|---|---|---|
| `70C` steering column | `EV_SMLSVALEOMQBLRH.rod` | 85 | **85** | 22 | **22** |
| `713` ESP | `EV_Brake1UDSContiMK100ESP_036.rod` | 518 | **518** | 33 | **33** |
| `712` power steering | `EV_SteerAssisMQB_013.rod` | 750 | 750 | 2 | **2** |
| `70A` parking aid | `EV_EPHVA14AU3700000_VW26.rod` † | 139 | 139 | 131 | 50 |

† the family's base file, picked by hand — the parking aid's *own* variant has no `[DTC]`
(§10.5), so this row is not a clean test and its 50/131 is not evidence either way.

Three things fall out at once and each is a check the reading had to pass:

* **1-based, not 0-based.** Read 0-based, the steering column's 85 ids collapse to 79
  distinct table keys — two ids landing on the same fault is not something a catalogue
  does — and cover only 20 of the 22 reported faults. Read 1-based: 85 ids, 85 keys, 22
  of 22. The ESP: 468 keys and 29/33 against 518 keys and 33/33.
* **A catalogue is a superset of what is stored.** 85 ids for a steering column module,
  518 for an ESP, 750 for an EPS — the right order of magnitude, and every fault the car
  had stored was in it.
* **One row.** The index is a row, so §5.2's 36 candidates for fault `297` become one.
  Obstacle 5.2 is closed, not narrowed.

### 10.2 …and the reading §8.1 proposed is refuted

§8.1 recorded that *"180 of the DQ200's 274 ids are `RD.rod` table keys"* and read that as
the join. It is a coincidence, and a structural one: row indices and table keys of the
low, dense band are numbers of the same magnitude — `RD.rod`'s six-digit keys are 78 %
dense below 50 000 — so an index there usually *is* also a key, meaning nothing.

The direct refutation needs no statistics. Taking the ids as table keys:

| unit | ids that "are" table keys | the unit's own reported faults found |
|---|---|---|
| `713` ESP | 345 of 518 (66.6 %) | **0 of 33** |
| `70C` steering column | 2 of 85 (2.4 %) | **0 of 22** |

A catalogue that omits every fault its own unit is currently storing is not a catalogue.
Two further readings were tried and are dead: the ids are **not** `Codes.dat` keys
(`Codes.dat` has *no* key at all between 130 700 and 130 900, where the steering column
puts 43 of its 85 ids), and they are **not** the unit's raw DTC numbers under any digit
substitution — searched exhaustively, the best any alphabet achieves is 2 of the steering
column's 22 reported faults and 8 of the ESP's 33, i.e. noise.

### 10.3 It does not touch the substitution

Restricting a table to one row removes the *selection* freedom, not the *decoding*
freedom: the row's `f0` still has to be read through the per-table alphabet, and §5.1's
finding that the alphabet is per-table is unaffected. Measured on the reference car's own
confirmed faults, running `unitjoin.name` on the row the unit selected:

| unit | fault | result |
|---|---|---|
| `713` | `00004B` | **120508 — "Rear Left Wheel Speed Sensor", FTB 07** |
| `713` | `00005B` | **120509 — "Rear Right Wheel Speed Sensor", FTB 07** |
| `713` | `000129` | **9 529 586 — "Steering Angle Sensor: Not Initialized"** |
| `70C` | `047120` | 86 candidate names (10 126 alphabets) |
| `712` | `004F04` | 35 candidate names |
| `712` | `004D04` | 94 candidate names |
| `70A` | `D01721 D01722 D0172E D0172F D01732` | 94 candidate names each |
| `710` | `010405` | no `.rod` `[DTC]` reachable (§10.5) |
| `09` | `000107 000213 060901` | no `.rod` at all (§10.5) |

`713 000129` is crib pair `297` and the answer is **exactly what VCDS printed** —
`B1168 F2`, *"Датчик угла поворота рулевого колеса: отсутствует инициализация"*. Three of
the car's fifteen parked-survey confirmed faults are named end to end, from VW's own files,
with the row chosen by the car rather than by the crib.

The other eight stay unnamed, and the model is not contradicted by any of them: the true
answer is **inside** the candidate set every time it is known — `047120`'s 86 candidates
contain 137 973 *"Temperature Sensor for Heated Steering Wheel"* and `D01732`'s 94 contain
153 539 *"Databus"*, which are the two the crib knows. Zero wrong, as before.

One further constraint was tried and does not bite. An eight-digit `Codes.dat` key encodes
its own failure type — 9 529 586 is `0x9168F2` and VCDS prints `B1168 F2`, so
`f1` must be `f0 & 0xFF` — which pins two more glyphs per such row. **None of the reference
car's tables has a row with an eight-digit `f0`**, so the filter (kept in `unitjoin.py`,
it is a real invariant) removes no alphabet here.

### 10.4 Finding the file the way a tool would have to

The file name comes off the car, not out of a table: `F19E` gives the ODX base name and
`F1A2`'s **first three digits** give the variant number, with a brand/platform suffix
(`SK37`, `VW37`, `VW48`, …) selecting the localisation —
`EV_Brake1UDSContiMK100ESP` + `036010` → `EV_Brake1UDSContiMK100ESP_036.rod`, and
`EV_SteerAssisMQB` + `013144` → `EV_SteerAssisMQB_013.rod`. Twelve of the reference car's
fifteen units resolve to at least one candidate that way.

`vag_data::corpus::find_rod_by_odx_name` matches the file **stem exactly**, so it finds
only the units whose ODX name happens to have no variant suffix — two of the fifteen.
Teaching it the `_<F1A2[0..3]>` and brand-suffix forms is the one code change this section
strictly requires, and it is independent of everything else here.

IV tails recovered for this section (`rod_crack` output, i.e. what
`rod_crack_prep.py decode` wants), ~95 s wall each on ten cores:

| file | tag | `plaintext[3:8]` | inflated |
|---|---|---|---|
| `EV_SMLSVALEOMQBLRH.rod` | DTC | `d34b6e5d31` | 935 B |
| `EV_Brake1UDSContiMK100ESP_036.rod` | DTC | `9849722b31` | 5 698 B |
| `EV_SteerAssisMQB_013.rod` | DTC | `99496e3331` | 8 250 B |
| `EV_EPHVA14AU3700000_VW26.rod` | DTC | `d44b722331` | 1 529 B |

### 10.5 Why some units have no `[DTC]` — and the one that has no file

§8.1's caveat — *"the parking aid's own `.rod` has no `[DTC]` section at all"* — is
explained rather than fatal. Within an ODX family exactly one file carries the section and
the variants carry an **`INC`** section instead: 14 parking-aid files, one with `[DTC]`
(`_VW26`) and thirteen with `INC`; 11 gateway files, none with `[DTC]`, all with `INC`.
`INC`'s payload is itself enciphered, and partially decoding
`EV_SteerAssisMQB_013.rod`'s shows two rows whose repeated-letter shape is the same string
under two alphabets and reads as `SteerAssisMQB…` — so `INC` is an ODX-name reference and
following it is the remaining piece of file resolution. It was not chased here.

Across the fifteen units of the reference car, **nine** have a family file with a `[DTC]`
section and six do not (engine, gateway, BCM, both door modules, infotainment) — for those
the `INC` chain leaves the family.

And one is simply absent: `09`'s `F19E` is `EV_BCMMQB` and **no file in the English corpus
starts with that name**, so its three confirmed faults (`000107`, `000213`, `060901`) are
unreachable for a reason that has nothing to do with any of the above.

### 10.6 What is left

Exactly one thing, and it is §5.1 unchanged: **the per-table substitution on small
tables.** The selector is solved; the decoder is not. §8.2's two leads stand —
`f2`'s valid set, and reversing the `MT`/`KS` routine that generates a table's alphabet
from its key, now checkable instantly against the 95 solved `(key, alphabet)` pairs *and*
against every row a unit file selects.

Worth adding to that list, because §10 makes it newly usable: `f2` is 190 000–450 000 and
`RD.rod [DTC]` has **236 755 rows**. Now that one id space in these files is known to be a
row number, `f2` being a row number into some other registry is a cheap thing to test, and
a known valid range would prune alphabets on every table at once.
