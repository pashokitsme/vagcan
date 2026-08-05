# Label linkage — attacking the `.rod` corpus from the proven end

An attempt, using the first full offline VCDS installation plus the 16 measurement rows now
**proven on the car** (`catalogs/*.json`), to recover from VW's own label files what the car
cannot tell us: **names** for our measurements, and — if it exists — the stored **scaling**.

Companion to `research/labels/rod-labels.md`. That file records what is already refuted; this one
records what a fresh attack from the *known* end found. Read `rod-labels.md` §2, §3, §4.0c
and §4.3 first.

Working data: `research/VCDS-25.12.0/` (the English install, read-only, never committed).
Nothing under `crates/` or `catalogs/*-NN.json` was modified for this work.

---

## 0. Verdict up front

| question | answer | confidence |
|---|---|---|
| Can the car's two `.rod` files be decoded? | **Yes** — see §1 | high (exact inflate + Adler-32) |
| Is the `.rod` payload codec understood? | **Structurally yes, numerically no** — §2 | high for the structure, see §2.4 |
| Do the per-ECU sections carry the read identifier? | **No — and now provably so, not just "not found"** — §3 | **very high** |
| Does `TTDOP` carry our proven factors (0.001 bar, 0.4 %, 0.01 mm …)? | **No, and it cannot** — §5 | high |
| Names recovered? | see §4 | see §4 |

The headline is §3. `rod-labels.md` §4.0c established that the read identifier is *not found*
in `STRUC`. This pass upgrades that from an absence of evidence to a **structural
impossibility argument**: the only per-ECU payload in a `.rod` measurement section is a list
of `(text-id, 2-char code)` pairs, and the 2-char code is a **global function of the text-id**
— identical for the same text-id across up to 295 different control-unit files, at
**100.00 %** over 10,583 text-ids. A per-ECU quantity cannot be carried by a field that does
not vary per ECU. So the read identifier is not in the per-ECU files at all, by counting
argument rather than by failed search.

---

## 1. The car's two label files — what decoded (task 1)

Both files are named by the control unit itself (`F19E`, `rod-labels.md` §4.2).

### 1.1 Gearbox — `EV_TCMDQ200021.rod` (DQ200, `0CW300041G`)

| tag | kind | cipher B | plain B | state |
|---|---|---|---|---|
| CMP | tea | 40 | 38 | decrypted (first 8 B corrupt — `product != 0`, TEA sections lose block 0) |
| ADP | tea | 40 | 33 | decrypted |
| INC | tea | 40 | 40 | decrypted |
| DTC | zlib | 1136 | 3014 | blocked, crackable |
| FFMUX | zlib | 80 | 132 | blocked, crackable |
| GES | zlib | 160 | 286 | blocked, crackable |
| **MWB** | zlib | 4568 | **11220** | blocked; brute force **started and still running** when this was written (§4) |
| SLV | tea | 16 | 13 | decrypted |

### 1.2 Engine — `EV_ECM18TFS0208V0906264H.rod` (`8V0906264H`, 1.8 TFSI CJSA)

| tag | kind | cipher B | plain B | state |
|---|---|---|---|---|
| CMP | tea | 48 | 48 | decrypted (block 0 corrupt) |
| INC | zlib | 160 | 194 | blocked, crackable |
| DTC | zlib | 2840 | 7260 | blocked, crackable |
| SLV | tea | 16 | 13 | decrypted |

**The engine file still has no `MWB`.** Every sibling in the family was checked
(`…264A/D/F/H/J_SK37/K/L` and the nine `IV_…` variants): only `EV_ECM18TFS0208V0906264A.rod`
carries one. That section was re-inflated here from the vector recorded in `rod-labels.md`
§4.0c (`5a478e243d`): **11,979 bytes, 1,089 rows**, every row `<6-digit text-id>,<2-char code>`,
1,089 distinct text-ids, 596 distinct codes.

### 1.3 A bug in the shipped cracker — found here, since **fixed** in `27908a0`

`vagcan labels … --crack` panicked on very small sections:

```
thread '<unnamed>' panicked at crates/vag-data/src/rod/crack.rs:58:13:
index out of bounds: the len is 72 but the index is 72
```

Reproducible on the gearbox `FFMUX` section (80 cipher bytes → 72-byte tail): the header
oracle is speculative — it is fed candidate bytes that are almost always wrong — and when the
tail held fewer bytes than the header wanted to read, it indexed one past the end. Because
that happened on a worker thread the panic was silent and the search reported no hit, so a
real answer elsewhere in the space would have been lost with it. Fixed by flagging the
overrun and rejecting the candidate.

**Correction to an earlier draft of this section:** it claimed the standalone
`research/clb-crack/rod_crack` "does not have the bug". That is wrong. Its bit reader does the
same unchecked `self.tail[i-6]` at `src/main.rs:34`. It never tripped here only because every
section driven through it had a multi-kilobyte tail (`TTTEXT` 4,817,000 B, gearbox `MWB`
4,560 B). Feed it a short section and it will panic identically. See §7 item 2.

---

## 2. The `.rod` table payload codec — solved to the field level

This is new, and it corrects the model in `rod-labels.md` §2 ("a packed 14-glyph base-14
bignum with unknown field segmentation").

### 2.1 There is a per-record separator, and it is one of the 14 glyphs

Every row of `STRUC.rod [STRUC]` and `TTDOP.rod [DOP]` is `NNNNNN,<payload>`, the payload
drawn from `[0-9,._-]`. The payload is **fields joined by a single separator glyph**, and the
separator **is chosen per record** (per table id, in practice) — it is not a fixed character,
which is why every previous reading treated the punctuation as digits.

Evidence, `TTDOP` (127,433 rows): take the payload's last character as a separator candidate.
**124,760 rows (97.9 %) contain that character exactly three times.** If the separator were
not excluded from the field alphabet, a ~10-character payload would contain it ~0.7 extra
times on average and the count would be spread out. It is not.

### 2.2 `TTDOP` rows are COMPU-SCALEs: `(lower, upper, ref)`

Splitting on the separator gives, for 124,603 clean rows, three fields, and

> **field 1 == field 2 in 121,398 of 124,603 rows (97.4 %)**

which is exactly a texttable COMPU-SCALE whose lower and upper limit coincide (a single
point). The 2.6 % where they differ are genuine *ranges*, and they tile: e.g. id `000102`
carries single points plus two adjacent intervals `[a,b]`, `[b+1,c]` that both map to the
same third field. The third field is a reference (a name / compu-const pointer), and within
one table id those references sit in a tight consecutive block — a name block allocated per
table.

`TTDOP` therefore holds **texttables only**. No row shape in it looks like a linear
`factor/offset` pair. This matters for §5.

### 2.3 `STRUC` records have exactly eleven fields

The same model applied to `STRUC.rod` (8,853 rows, 1,221 table ids), choosing per id the
separator that makes every row of that id split into the same number of fields:

> **1,220 of 1,221 ids resolve to exactly 11 fields** (one to 12).

That is not a coincidence — it is the record layout. Within one id, fields 2–5 are empty in
~88 % of records, one field varies per row (the per-channel field), and one field is
near-unique across the whole table (8,285 distinct values over 8,853 records — a name/text
pointer).

The eleven fields, profiled over the 1,220 ids that resolve (8,852 records). "Varies" counts
ids where the field differs between rows of the same table, i.e. it is a per-channel field
rather than a per-table one:

| field | glyph length | empty | varies within a table | reading |
|---|---|---|---|---|
| 0 | 0, 3–5 | 5,109 | 640 ids | per-table mostly; large values |
| 1 | **always 1** | 0 | 741 ids | small code |
| 2–5 | 0–5 | ~7,700 each (87 %) | ~400 ids | optional attributes, usually absent |
| 6 | 1–3 | 0 | 941 ids | per-channel |
| 7 | **always exactly 1 glyph** | 0 | 394 ids | a single decimal digit — a kind/type code |
| 8 | 1–2 | 109 | 792 ids | per-channel |
| 9 | **5–6 glyphs**, 8,285 distinct of 8,852 | 264 | 957 ids | **the name pointer** — a 6-digit text-id, the same width and near-uniqueness as `MWB`'s text-ids |
| 10 | 1–3 | 9 | **0 ids** | a per-table constant |

Field 9 being 5–6 digits wide, near-unique, and varying per channel is the strongest single
identification in the record: it is the `TTTEXT` reference. Field 7 being *exactly* one digit
in 8,852 of 8,852 records is the second: a ≤10-valued type code.

`rod-labels.md` §2 tested *fixed character columns* and found no per-channel index run. Under
the separator model the run appears immediately: e.g. id `000005`'s four rows differ in
exactly one field, and its glyphs there are four distinct symbols — the per-channel index
that the fixed-column test could not see.

### 2.4 The digits are **base-10 under a per-table substitution** — and that is where it stops

The remaining question was the radix. It is not 14 and not 13:

| `TTDOP` table id | rows | payload chars | separator | distinct non-separator glyphs |
|---|---|---|---|---|
| 019785 | 1027 | 21,310 | `0` | **10** |
| 027305 | 649 | 9,463 | `6` | **10** |
| 028821 | 560 | 8,554 | `-` | **10** |
| 028535 | 557 | 8,499 | `3` | **10** |
| 024152 | 529 | 7,663 | `2` | **10** |
| … (15 largest tables) | | | | **10 in every case** |

Twenty thousand characters drawn from a 13-symbol alphabet would show all 13 symbols within
the first few dozen draws. Every large table shows exactly **ten**. The numbers are
**decimal**; each table id uses ten of the fourteen glyphs as its digits, one as its
separator, and leaves three unused.

Which ten, and in what order, **varies per table id** and is not derivable from the
separator: tables `028376` and `024375` share the separator `1` and use different digit sets.
The "digits are the used glyphs in alphabet order" rule was tested and fails: it predicts a
consecutive per-channel index run in `STRUC` for only 25 of 800 multi-row tables, and it makes
`TTDOP`'s third field land in the known-valid text-id set at 4.55 % against a 4.96 % chance
baseline.

Two further checks pin this down.

*The zero digit is identifiable, and it is not `0`.* A multi-digit number never starts with
its zero digit. Over the 37 largest `TTDOP` tables that resolve cleanly, **35 have exactly one
glyph that never appears in a leading position** — the zero digit, unambiguously. It differs
per table: `9`, `0`, `1`, `3`, `_`, `6`, `,`, `8`, … Under any fixed-alphabet reading (base 13,
base 14, ALPHA order) the zero digit would always be `0`. It is not. This is an independent
confirmation of both base 10 and the per-table substitution.

*The substitution is not a rotation of any fixed ring.* If each table took a contiguous window
of a fixed cyclic order (10 digits + separator, 3 glyphs left over), the three unused glyphs
would always be cyclically adjacent, so 63 of the 91 glyph pairs could never appear together
in an unused triple. Over **4,716** tables with exactly ten digit glyphs, **zero of the 91
pairs is absent** and the co-occurrence counts are flat (120–201). The unused set is
effectively random per table.

**So the field layout is recovered and the numerals are not.** The substitution is a per-table
permutation seeded by something in VCDS's code (the same `MT`/`KS` machinery as the IV, most
likely). Breaking it needs either that routine reversed, or a strong per-table oracle — a
cracked `TTTEXT` supplies exactly such an oracle, which is why §4 matters beyond names.

**Consequence, stated plainly:** *any* claim of the form "STRUC field N holds value V" is not
currently checkable, including negative ones. The `.rod` negatives below are therefore built
on **counting and structure**, not on decoded numbers.

---

## 3. The decisive negative: the per-ECU code carries no per-ECU information

Every measurement-bearing section of every per-ECU `.rod` — `MWB`, `ADP`, `GES`, `SOT`,
`XPL`, `FFMUX`, `DTC` — is a list of `<6-digit text-id>,<2-char code>`. Scanning **all 16,576
`.rod` files** in `UDS_EV` and decoding every section that opens with `product = 0`:

| section | distinct text-ids | rows decoded | text-ids seen ≥8× | of those, **exactly one** code string |
|---|---|---|---|---|
| MWB | 13,144 | 234,133 | 5,601 | **5,601 (100.0 %)** |
| DTC | 26,161 | 339,108 | 4,275 | **4,275 (100.0 %)** |
| GES | 440 | 4,652 | 123 | **123 (100.0 %)** |
| SOT | 318 | 2,655 | 85 | **85 (100.0 %)** |
| ADP | 300 | 5,067 | 88 | **88 (100.0 %)** |
| XPL | 319 | 760 | 21 | **21 (100.0 %)** |
| FFMUX | 56 | 1,276 | 13 | **13 (100.0 %)** |

Restricting to `MWB` alone over the 295 files whose `MWB` opens unblocked, and lowering the
threshold:

```
text-ids seen in >= 2 files : 10,583  ->  10,583 carry exactly one code string (100.00 %)
text-ids seen in >= 3 files :  9,135  ->   9,135 (100.00 %)
text-ids seen in >= 5 files :  6,856  ->   6,856 (100.00 %)
```

**Not one counter-example.** The code is a function of the text-id, full stop. Two engines
with different part numbers, different software, and different data identifiers list the same
measurement with byte-identical `(text-id, code)`.

Therefore:

1. **The read identifier is not in `MWB`.** A DID is per-ECU (`identifier-map.md` §0.4: the
   identifier space is ECU-local; `F40D` is one byte of km/h on the engine and two
   little-endian bytes ×0.01 on the gearbox). The only two columns present are a global name
   pointer and a global code. Neither varies per ECU. This is stronger than
   `rod-labels.md` §4.0c's search-based negative: no encoding of the DID can be hiding here,
   because there is no per-ECU degree of freedom to hide in.
2. **The `code → STRUC-id` hypothesis takes another hit.** `MWB` uses **1,492 distinct codes**
   of the 1,600 the 40-symbol × 2-character space allows — the space is saturated. `STRUC.rod`
   contains only **1,221 distinct table ids** (range 1…1623). Under any injective code → id
   map at least 271 codes would address records that do not exist. (`rod-labels.md` §3.1
   already refuted base-40 arithmetic for this map; this is an independent cardinality
   argument against the target being `STRUC` at all.)
3. **The code is also not a hash of the text-id** — it is a lookup. Tested `code == text-id
   mod 1600` under three alphabet orders × both digit orders: 5–12 matches out of 13,136.
   Digit-wise purity of `text-id mod 40 → code character` is 6.5–8.0 %, i.e. none.

What the code *is*: an attribute of the measurement shared by ~8.8 measurements on average —
a unit, a display class, or a structure. Which one cannot be settled without §2.4.

---

## 4. Names (task 2) — NOT DELIVERED *at the time of writing*, and the reason is worth stating precisely

> **Update:** superseded by `research/labels/tttext-codec.md`. `TTTEXT`'s codec was subsequently
> broken and `catalogs/names-uds.json` now exists — 17,009 names, keyed by text-id exactly
> as this section prescribes (not by identifier, for the reason given below, which stands).
> The `ENG######` lead at the end of this section is also settled there: it **is** the
> `TTTEXT` text-id, proven four for four on records solved blind (`tttext-codec.md` §2).

**`catalogs/names-uds.json` was not written then.** Not because the names are unreachable, but
because a `identifier → name` file needs two things and only one of them is within reach.

**What is reachable.** The name join itself is mechanical: `MWB` row → 6-digit text-id →
`TTTEXT.ROD [TXT]` string. The engine's list is already in hand — 1,089 text-ids from
`EV_ECM18TFS0208V0906264A.rod`, spanning 111…129,159. Across the whole corpus, **43,781
distinct text-ids** are referenced by decodable sections. What is missing is `TTTEXT` itself:
its `[TXT]` section is `product`-blocked, 4.82 MB of ciphertext inflating to 7.46 MB, and its
recovery is a 2³⁶ brute force. It was left running for this work and **had not landed when
this was written** — it is mechanical, not blocked, and whoever picks it up should simply let
it finish. Same for the gearbox `MWB` (4,568 cipher bytes → 11,220), which was also still
running.

**What is not reachable, and would still not be after `TTTEXT` lands.** A name list is not a
`{identifier: name}` map. §3 shows the corpus contains no per-ECU identifier at all, so
`TTTEXT` would yield *"here are the 1,089 measurements this engine family has, by name"* —
useful, but unordered with respect to `206E`, `2029`, `380A`. Writing a `names-uds.json` from
that would mean guessing which name belongs to which proven identifier, which is exactly what
this project keeps getting burned by. Omitted deliberately.

**One concrete lead that would close it.** The VCDS logs for **gearbox 02 only** carry a
second number per column:

```
Loc. IDE00022-ENG103074   Число оборотов на входе КП-Transmission Input Speed Sensor
Loc. IDE00075-ENG99967    Скорость автомобиля-Vehicle Speed Sensor
Loc. IDE00130-ENG103124   Заданное число оборотов … -Idle Speed Commanded Value
Loc. IDE03174-ENG100415   … -Q005 Driving Time Manual
```

The engine and cluster logs have no such suffix, and their names are single-language — the
gearbox's are bilingual `<localised>-<engineering>`. So `ENG######` reads as *the text-id of
the engineering-language name*, and those numbers sit squarely in the text-id range. That
matters because `vagcan analyse` proves `IDE00022 ↔ 7E9/380A` at `R² = 1.00000`
(re-run here on `research/dumps/session-2026-08-01.jsonl`), which would give a **real**
`text-id ↔ proven identifier` pair — the join this whole exercise is missing.

Status of that lead *when written*: **suggestive, not established** — since settled as
**established** by `research/labels/tttext-codec.md` §2 (four records solved blind all match the
log's names). Of the 15 distinct `ENG######` numbers
in our gearbox logs, 6 appear among the 43,781 text-ids the corpus references — against an
18.2 % density baseline in that numeric window, so 40 % vs 18 %, `p ≈ 0.05` at `n = 15`. That
is one-and-a-bit sigma of evidence, and the nine misses are equally well explained by those
measurements being DQ200-only (every `TCMDQ200*` file's `MWB` is `product`-blocked, so none of
them contributed text-ids to the corpus sample). **Two cheap checks settle it**, both blocked
only on cracks that are already running:

1. Does the cracked gearbox `MWB` contain text-id `103074`? If yes, `ENG######` is a `MWB`
   text-id and the pair `103074 ↔ 380A` is established.
2. Does cracked `TTTEXT` render `103074` as "Transmission Input Speed Sensor"? If yes, it is
   established twice over, and the same trick names every proven gearbox row whose `IDE` the
   log records.

If both hold, `catalogs/names-uds.json` becomes writable for the gearbox rows — and only for
those, since the engine and cluster logs do not print the second number.

---

## 5. The scaling linkage attempt (task 3) — NEGATIVE, and for a structural reason

The task was: *search `TTDOP` for COMPU entries whose constants match our proven factors
(0.001 with bar, 0.4 with %, 0.01 with mm, 10 with kPa, 1.0 with /min); if a small set of DOP
ids carries exactly those, work backwards.*

That search cannot be run, and the reason is not "we did not find them" — it is that the
premise does not hold. Three independent findings:

**5.1 There are no decimal constants in the corpus, because there are no decimal points.**
The payload alphabet is fourteen glyphs, of which one is the record's separator and ten are
its decimal digits (§2.4). `,`, `.`, `-` and `_` are *digits or separators*, never punctuation.
A factor is therefore never written as `0.001` or `0.4` anywhere in `STRUC` or `TTDOP` — the
representation has no way to express it. Grepping the 2.7 MB of decoded `TTDOP` for the
literal constants is meaningless, which is the same trap `rod-labels.md` §4.0c fell into for
DIDs (its "DID as a literal decimal string" test) and reached the same non-answer.

**5.2 `TTDOP` is a texttable table. It does not hold linear coefficients at all.**
All 17,636 ids were parsed. Every id — including the 1,016 ids with a single row and the
4,614 with two — consists of `(lowerLimit, upperLimit, ref)` triples, lower == upper in
97.4 % of 124,603 rows and the exceptions tiling as adjacent intervals (§2.2). There is no
second row shape, no coefficient pair, no numerator/denominator. `DOP` in this corpus means
**COMPU-SCALE list**, i.e. the enumerations — which is consistent with our own catalogs: of
16 proven rows, exactly the two enumerations (`3816` selected gear, `3809` selector lever)
are the kind of measurement `TTDOP` could describe. The fourteen linear ones have no home in
`TTDOP`.

So a linear factor, if the corpus stores one at all, lives in the eleven-field `STRUC`
record — and there it is behind the per-table digit substitution (§2.4), so **no numeric
comparison against our proven factors is possible today, in either direction**. Not "we
compared and it did not match": we cannot compare.

**5.3 Even if the numbers were readable, the chain has no per-ECU link.** §3 shows the only
per-ECU payload is `(text-id, code)` with the code globally determined by the text-id. So the
route `our DID → its measurement → its structure → its scaling` has no first step: nothing in
these files is keyed by, or varies with, the read identifier.

### 5.4 What was tried and rejected along the way

Recorded so the next person does not repeat them.

| attempt | result |
|---|---|
| The enumeration crib: find the `TTDOP` id whose scale points are exactly our proven gear set `{0,2,3,4,5,6,7,8,12}` | 24 candidate ids under an assumed digit order, one of them (`022063`) an exact 9-row match — **but worthless**, because §2.4 shows the digit order is a per-table permutation, so the "values" being matched are not values. Not evidence. |
| `MWB` 2-char code = the VCDS `IDE#####` measurement number in base-40 | **Refuted.** The code space is 40² = 1600; the logs from this car alone contain `IDE02307`, `IDE03174`, `IDE06282`. The IDE space exceeds the code space by at least 4×. |
| `IDE` number = a `TTDOP` id | **Refuted.** 23 of the 36 IDE numbers this car's logs name exist as `TTDOP` ids — against a 61 % density baseline, i.e. exactly chance. |
| `IDE` number = a `STRUC` id | **Refuted** on range alone, independently of `rod-labels.md` §4.0c: `STRUC` ids stop at 1623, and 10 of the 36 IDE numbers this car's logs name are larger (up to `IDE06282`). Among the 26 in range, 23 exist — against a 75 % density baseline, i.e. nothing. |
| `code` = a hash / modular function of the text-id | **Refuted.** `code == text-id mod 1600` matches 5–12 of 13,136 under three alphabet orders × both digit orders; digit-wise purity of `text-id mod 40 → code glyph` is 6.5–8.0 %. It is a table lookup. |
| Any `STRUC` field being a `TTDOP` reference | No field exceeds 84 % membership in the DOP id set, against a 61 % density baseline — and 84 % occurs on a field whose values never exceed 164, where DOP ids are ~100 % dense. Nothing distinguishable. (Weak test: depends on §2.4.) |
| The literal `IDE00405`-style string existing anywhere in the VCDS install | No ASCII match in any file under `research/VCDS-25.12.0/`. The prefix is synthesised at runtime from a number whose source is not in `UDS_EV`. (A wide-character encoding was not ruled out.) |

### 5.5 Where the read identifier could still be

Two places remain, both unexamined and both large enough to hold a global measurement
registry:

- **`TTTEXT2.ROD`** — 3.69 MB cipher → 5.52 MB plaintext, a single blocked `[TXT]` section.
  A second global table keyed by the same text-ids would be exactly the shape needed.
- **`MUX.rod`** — 0.50 MB cipher → 2.08 MB plaintext, a single blocked `[MUX]` section.
  ODX `MUX` is the multiplexer construct, which is where a "read this identifier, then switch
  on a byte" description would live.

Neither was cracked here (each is an independent multi-hour brute force). Note that §3's
argument does **not** exclude them: a global table keyed by text-id is compatible with the
code being global — it would mean **VW/Ross-Tech assign the read identifier per *measurement*,
globally, not per ECU**, and a control unit's `.rod` merely lists which measurements it has.
That is a testable prediction, and it is the single most valuable next experiment: our own
data contradicts it if two of our proven rows share a text-id but differ in identifier, and
supports it otherwise.

---

## 6. Reproduction

(As-run commands from the time of this work. `vagcan labels` has since lost `--crack`:
the IV brute force now lives behind the `rod-crack` cargo feature —
`cargo run -p vag-data --features rod-crack --bin vag-rod <file.rod>` — and `vagcan labels`
reads the cached IVs from `catalogs/rod-iv-cache.json`.)

```
# decode a car label file (IV cache kept outside research/VCDS-*)
cargo run --release -p vagcan -- labels research/VCDS-25.12.0 \
        --odx EV_TCMDQ200021 --crack --iv-cache /tmp/ivcache.json

# standalone cracker (avoids the crack.rs panic on tiny sections, §1.3)
cd research/clb-crack
.venv/bin/python rod_crack_prep.py prep  <file.rod> <TAG>       # -> crack_input.bin
./rod_crack/target/release/rod_crack                            # -> plaintext[3:8]
.venv/bin/python rod_crack_prep.py decode <file.rod> <TAG> <hex5>
```

Recovered vectors used here (the cache stores `IV[3:8]`; `rod_crack` prints `plaintext[3:8]`,
which is what `rod_crack_prep.py decode` wants — `rod-labels.md` labels the latter "IV", which
is a naming slip worth knowing about):

| file | tag | `plaintext[3:8]` | inflated |
|---|---|---|---|
| `STRUC.rod` | STRUC | `9d69922429` | 293,560 B |
| `TTDOP.rod` | DOP | (cached `IV[3:8]` = `bc0a086cec`) | 2,722,454 B |
| `EV_ECM18TFS0208V0906264A.rod` | MWB | `5a478e243d` | 11,979 B |

Analysis was done with throwaway scripts under `/tmp`; nothing was added to `research/clb-crack`.
The steps are small enough to restate: split each payload on its separator glyph, count
fields, and count distinct non-separator glyphs per table id.

---

## 7. What to do next, in order of value

1. **Let the two brute forces finish** (`TTTEXT.ROD [TXT]`, `EV_TCMDQ200021.rod [MWB]`). Both
   were started here and both are mechanical. They unlock §4's two checks, which are the only
   route to a defensible `identifier → name` file. A `TTTEXT` run was left going in
   `/tmp/vcdswork/tt/` (`crack_input.bin` prepped, answer lands in `tt.hex`); if it is gone,
   re-prep with the §6 recipe. Budget: the search is 2³⁶ and this machine sustained only
   ~2–3 effective cores even with `threads=10`, so allow **1–2 hours wall per section**, not
   the "~1 minute" `rod-labels.md` §1.3 quotes for `STRUC` (that one's answer happened to sit
   3 % into the space).
2. ~~Speed the searcher up~~ — **done, §9.** A full 2³⁶ sweep went from ~29 minutes to
   ~2 minutes on this machine.
3. **Crack `TTTEXT2.ROD` and `MUX.rod`** (§5.5). They are the only remaining places a global
   measurement registry — and with it the read identifier — could live. If neither holds one,
   the `.rod` corpus is *proven* to be name-and-list-only, which is a clean end to a question
   that has now been open for four writeups.
4. **Break the per-table digit substitution** (§2.4) — needed for any numeric field, ever. Two
   routes: reverse VCDS's decoder (the encoder side is the `msub #0xe` routine
   `fcn.1400e6f80` that `rod-labels.md` §2 already located; note that routine formats *base
   N*, and the finding here is that N is 10 with a keyed glyph map, so the key derivation is
   what to look for), or crack it per table against a cracked `TTTEXT` as the oracle. The
   leading-zero test already hands you one digit of each table's key for free.
5. **Do not** re-run: literal-constant greps over `TTDOP` (§5.1), base-40 code arithmetic
   (`rod-labels.md` §3.1 and §3 here), `IDE` as a `STRUC` or `TTDOP` id (§5.4), or any search
   for a per-ECU identifier inside a `.rod` measurement section (§3).

## 8. Honest scoring of this pass

- **Task 1 (decode the car's files): done**, minus two brute forces that were started and had
  not finished. The engine file's absence of an `MWB` is re-confirmed across the whole family.
- **Task 2 (names): not delivered.** Blocked on `TTTEXT` finishing, and even then a
  name *list* is what falls out, not a name *per identifier* — see §4 for the one lead that
  would change that and the two checks that decide it.
- **Task 3 (scaling linkage): negative, with a reason rather than an absence.** `TTDOP` holds
  texttables only and the corpus cannot express a decimal constant at all, so the search as
  posed is not merely unsuccessful, it is ill-formed (§5.1, §5.2). No single-row coincidence
  was promoted to a finding; the one that looked like a hit (`022063` matching our gear
  enumeration exactly) is written up in §5.4 as **not evidence**, because the numbers it
  matched are not numbers.
- **Net new ground:** the payload codec is now understood one full layer deeper than
  `rod-labels.md` §2 — separator-delimited fields, eleven of them in `STRUC`, `(lo, hi, ref)`
  in `TTDOP`, base **10** under a keyed per-table glyph substitution — and the read-identifier
  negative is now an argument from cardinality (§3) rather than from a search that came back
  empty.

---

## 9. The searcher: a 14× speedup, measured

The brute force was the binding constraint on everything above — §4's names were not delivered
because a sweep did not finish. `rod-labels.md` §1.3 has prescribed the fix since the
beginning ("porting the Python DFS pruning into Rust would make it fast regardless of where
the answer lands"); the Rust port had dropped it. It is now in.

**What was wrong.** The search was a flat five-deep loop calling a full header parse on every
one of the 2³⁶ leaves. Each call re-read the deflate header from bit 0, re-deriving the
code-length prefix that all 65,536 of a node's siblings share, and allocated a `Vec` for the
code-length Huffman on every candidate that got that far.

**What it is now.** A depth-first walk that prunes a subtree the moment the bytes pinned at
that depth make a valid header impossible. The bit layout is what makes it pay: `d0` is exact
and pins BFINAL/BTYPE/HLIT, `d1` pins HDIST and most of HCLEN, and by `d3` there are five
complete code-length-code entries to test — so one rejection there replaces 65,536 leaf
parses. Also: fixed-size arrays instead of a per-candidate allocation, a completeness check on
the code-length code (RFC 1951 requires it; the old parse did not test it), a size-bounded
inflate, and work-stealing over `(d1, d2)` instead of fixed chunks that left the last thread
running alone.

**Both pruning rules are sound, not heuristic.** This matters more than the speed: a wrong
prune would silently discard the answer and report `NO HIT`, which is indistinguishable from a
genuine miss. Over-subscription (Kraft sum above 1) admits no Huffman code, ever.
Unreachability (even every remaining entry at the shortest legal length cannot reach a
complete code) admits no completion below that node. Neither can discard a valid header.

**Measured, on `STRUC.rod` whose answer `9d69922429` is known independently.** Both searchers
were run on the same prepped input, uncontended, on the same machine; both returned the
correct five bytes.

| | space covered | wall | CPU-s | rate (space/CPU-s) | extrapolated full 2³⁶ sweep |
|---|---|---|---|---|---|
| old | 2.196 × 10⁹ | 55.3 s | 374.7 | 5.86 × 10⁶ | **~29 min** |
| new | 4.211 × 10¹⁰ | 76.3 s | 675.4 | 6.24 × 10⁷ | **~2.1 min** |

"Space covered" counts pruned subtrees, which is legitimate — they are proven empty, not
skipped. The new run reached a leaf for only 26.3 % of the positions it covered: 201,527
subtrees died at `d3` (65,536 leaves each) and 42,396,316 at `d4` (256 each).

**10.6× per CPU-second, 13.9× on full-sweep wall time.** The two runs did different amounts of
work — where the answer falls in the traversal order is luck, and `STRUC`'s sat 3.2 % into the
old ordering against 26 % into the new — which is exactly why the comparison is normalised by
space covered rather than by time-to-hit. Note this also revises the §7 budget: at ~2 minutes
per section, `TTTEXT2` and `MUX` are no longer multi-hour commitments, and the honest reason
§4 came back empty was a slow searcher rather than anything about the data.

Regression: `rod_crack_prep.py prep STRUC.rod STRUC` then `rod_crack` must print
`9d69922429`. Keep that check — it is the only thing standing between a pruning bug and a
silent false negative.


## Addendum — the `ENG######` lead is refuted (2026-08-02)

§4 flagged the second number in the gearbox VCDS logs (`Loc. IDE00022-ENG103074`) as a
possible text-id, on the strength of 6 of 15 such numbers appearing somewhere among the
corpus's 43,781 text-ids against an 18.2 % baseline — p ≈ 0.05, suggestive only.

The gearbox `MWB` is now cracked (1,020 rows, 907 distinct text-ids, range 171…176,260),
which allows the *right* test rather than the global one: do these numbers appear in **this
gearbox's own measurement list**? They are this gearbox's measurements, so they must.

**Zero of eleven.** Checked against every proven pair — `ENG103074`↔`380A`,
`ENG99005`↔`380B`, `ENG99967`↔`F40D`, `ENG98363`↔`3804`, `ENG120857`↔`3832`,
`ENG120861`↔`383B`, `ENG120895`↔`38F6`, `ENG120898`↔`38F9`, `ENG120909`↔`38AC`,
`ENG120910`↔`38AD`, plus `ENG103124` — and confirmed by raw byte search across **all eight
decoded sections**, not just `MWB`, in case the parse was at fault. In the window
103,000–103,200 the list holds only `103093`; `103074` and `103124` are simply absent.

The earlier global hit rate was therefore the baseline showing through, not a signal. This
was the last lead §4 had identified, so it closes.

**Refined hypothesis, untested:** the Russian VCDS displays these measurements bilingually
("Число оборотов на входе КП-Transmission Input Speed Sensor"), so `ENG` plausibly means
*English* rather than *engine* — a key into a separate English text table rather than into
the per-ECU list. The English install is on disk; its `TTTEXT.ROD` is not yet cracked. That
is a cheap check once it is, and it is the only remaining way this number could be useful.
