# `TTTEXT.ROD [TXT]` — the codec, and the names it gives up

The global text table of VW's ODX corpus. Its `[TXT]` section had already been decrypted and
inflated (7,461,935 B) by the time this work started; what remained was that the payloads
looked like word-shaped gibberish. **They are enciphered, per record, with a simple
substitution.** The letter half is broken and 92,420 of the 192,469 records now read as
fluent English. The numeric half is not broken, and §6 says exactly what was ruled out.

Companion to `research/labels/label-linkage.md` (read §2 and §4 first) and `research/labels/rod-labels.md`.
Working data lives outside the repo; nothing under `research/VCDS-*` or `crates/` was touched.

---

## 0. Verdict up front

| question | answer | confidence |
|---|---|---|
| What is the encoding? | a **per-record monoalphabetic substitution**, applied independently to three character classes, leaving all other characters alone — §1 | **very high** (§1.1 frequency bands, §1.2 cribs) |
| Is the key derivable from the record id? | **No rule found**, and every simple family is refuted — §5 | high for the refutations, open for "some rule exists" |
| Are the letters recovered? | **Yes** — 92,420 records (48.0 % of all records, 54.0 % of those with ≥8 letters) decode end-to-end — §3 | **very high**, 599/600 independent re-solves agree (§4) |
| Are the digits recovered? | **No.** The 14-glyph numeric class is unbroken and provably independent of everything we hold — §6 | high (the negative is measured, not assumed) |
| `catalogs/names-uds.json` written? | **Yes — 17,009 names**, gated hard (§7) | see §4/§7 |
| Is `ENG######` in the VCDS log a `TTTEXT` text-id? | **Yes — established, was "suggestive" in `label-linkage.md` §4** | **very high** (§2) |

---

## 1. What the encoding is

Each record is `NNNNNN,<payload>\r\n`. The 6-digit id is **plaintext** (they are sorted and
strictly increasing; 192,469 records, ids 3 … 454,841). The payload is enciphered.

The cipher is a **substitution chosen afresh for every record**, and it acts on three
disjoint alphabets, each permuted within itself:

| class | size | behaviour |
|---|---|---|
| `a`–`z` | 26 | permuted |
| `A`–`Z` | 26 | permuted by the **same** permutation — case is preserved, not keyed separately |
| `0 1 2 3 4 5 6 7 8 9 , . _ -` | 14 | permuted (this is the ODX numeric alphabet of `label-linkage.md` §2.4) |
| everything else | — | **passes through untouched**: space, `: ( ) [ ] / \| < > = % & + " ' # $ * °`, `ü ä ö ß é ë µ ³` |

### 1.1 The frequency evidence — why "three classes, per record"

Character counts over the whole 7.46 MB payload area fall into flat bands:

```
a–z    136,805 … 144,710   (26 symbols, spread ±3 %)
A–Z     24,177 …  26,275   (26 symbols, spread ±4 %)
0-9,._-  71,223 …  75,822  (14 symbols, spread ±3 %)
--- and then, not flat at all: ---
space 325,022   :18,595   ( 6,292   ) 5,834   ] 3,117   [ 2,785   / 2,618
|1,856   ü 1,045   ä 940   = 540   < 521   $ 511   % 398   + 394   ° 371 …
```

Three flat bands is the signature of **three permuted alphabets averaged over many
independent keys**: a single global key would merely permute English letter frequencies, not
flatten them (this is what the brief's "letter frequencies are FLAT" observation was
detecting). Everything outside those three bands keeps a natural, wildly uneven distribution
— so it is not enciphered. In particular **space is not in any class**: at 325,022 it is 4.4×
the mean of the 14-glyph band, which it would have to match if it were the class's 15th
member.

### 1.2 Constant within a record, different between records

* Constant within: record `044533` contains `°O` twice, at different offsets, and both are
  `°C`. Record `012389` contains `gur` twice and both are `lid`.
* Same permutation for both cases: `043891` maps `H→E` *and* `h→e`; `044533` maps `O→C` *and*
  `o→c`.
* Different between: no two records in the corpus share a payload (0 byte-identical payloads
  among the 151,463 records with ≥12 letters), and pairs of records carrying the *same*
  plaintext carry different ciphertext — e.g.

```
043891  Hxthajoa thiuhawtsah < 54°C.7.451-1   ->  Exterior temperature < 54°C
044248  Xoonqyf fcrecjqfajc < 93°X5953724_    ->  Coolant  temperature < 93°C
```

  Both contain `temperature`; the two records spell it `thiuhawtsah` and `fcrecjqfajc`.

### 1.3 Record layout

Payload = `<text><sep><field>…`, the fields joined by a separator drawn from the 14-glyph
class, exactly the construction `label-linkage.md` §2.1 found in `STRUC`/`TTDOP`. Probing
40,000 records for a glyph whose split leaves exactly one field containing letters: 39,548
resolve, and the text is **field 0** in 38,525 of them (2 fields 19,683 / 3 fields 15,739 /
4 fields 2,795 / 5–6 fields 308). Typical tail shape is `<sep><digit><sep><number>`:

```
Absolute intake pressure ,8,440.        Absolute load value _5_...9
ACC specified acceleration -5-2,38      Speed regulation requested torque 0_06-
```

The text itself is either an ODX **LONG-NAME** (spaces) or an ODX **SHORT-NAME**, in which
case the words are joined by `_` — which, being a member of the numeric class, comes out as
an arbitrary glyph:

```
445905  917Hzvs7lsznf7zn7hxs7syyswhzus7ucqhbfs7lzn7cn7BW7txbis79-
     -> Time_being_in_the_effective_voltage_bin_on_AC_phase
```

---

## 2. The cribs — including the one that settles `label-linkage.md` §4

`label-linkage.md` §4 flagged the second number in the VCDS gearbox log
(`Loc. IDE00022-ENG103074 …-Transmission Input Speed Sensor`) as *possibly* a `TTTEXT`
text-id, and rated it "suggestive, not established" at `p ≈ 0.05`. Decoding the corresponding
records settles it:

| record | ciphertext | decodes to | VCDS log said |
|---|---|---|---|
| `103074` | `Rvplcbuccuml Ulkxr Ckhha Chlcmv_` | **Transmission Input Speed Sensor** | `ENG103074` → Transmission Input Speed Sensor |
| `099967` | `Pfhdyof Wuffj Wfrwns-` | **Vehicle Speed Sensor** | `ENG99967` → Vehicle Speed Sensor |
| `103124` | `Xkpe Ofeek Crvvzqkek Bzpge6` | **Idle Speed Commanded Value** | `ENG103124` → Idle Speed Commanded Value |
| `100415` | `A77_ Kbrhrux Lrwp Wtugtm2` | **`?`005 Driving Time Manual** | `ENG100415` → Q005 Driving Time Manual |

Four for four, on records picked before any of them was solved, each under a different key.
The odds of a 26-letter permutation reproducing a 30-character named phrase by accident are
nil. **`ENG######` is the `TTTEXT` text-id.** (In `100415` the leading `Q` and `005` sit in
the unbroken numeric class and the letter `A` there is `Q` — see §6.)

Because `vagcan analyse` already ties `IDE00022` to `7E9/380A` at `R² = 1.00000`, the chain
*proven identifier → IDE → ENG → name* is now closed for every gearbox row whose `IDE` the
log prints. That is the join `label-linkage.md` §4 said was missing.

### 2.1 The ROT13 lead was a coincidence — closed

The brief's one weak lead was record `012389`, where a uniform ROT13 produced `the` twice.
Its actual key is not a shift, and the record reads:

```
Qyxxia wik khfk gur yagieduam ua khfk gur-3-.623.
Button for rear lid unlocking in rear lid
```

The two `the`s were `gur` → `lid`. `gur`/`the` is the single most famous ROT13 pair in
existence, and `lid` happened to be the plaintext. Nothing else in the record decoded because
nothing else was a coincidence. `12389 mod 26 == 13` is unrelated: §5 shows the key has no
relation to the id modulo anything.

---

## 3. How the letters were recovered

A per-record substitution with ~90 characters of ciphertext is solvable from a dictionary
alone, and the corpus supplies its own dictionary.

1. **Cluster.** Two records with the same *token-repetition pattern* (the isomorphism class of
   the letter runs, `pat('|'.join(tokens))`) hold the same plaintext words. 171,129 records
   carry ≥8 letters and collapse into **111,730 clusters**; one solve serves the whole cluster.
2. **Solve a representative.** Branch-and-bound over the cluster's tokens, longest first.
   Candidate plaintext words come from a pattern index; a candidate is admissible if it agrees
   with the partial map and keeps it injective. The objective is `4·len + log freq` summed over
   solved tokens, so coverage dominates and word frequency breaks ties. Base vocabulary:
   5,163 word types harvested from the 1,178 plaintext `.lbl` files of the VCDS install
   (weighted ×200 — this is the in-domain prior), plus `/usr/share/dict/web2`, plus
   conservative inflections of in-domain words.
3. **Complete.** Letters still unassigned are filled by re-filtering each token's pattern class
   against the letters the rest of the record already pins, accepting only a clear winner.
4. **Bootstrap.** Words read off well-solved records that were not in the dictionary are fed
   back in and everything is re-solved. Four passes: 58,646 → 60,341 → 60,652 → 60,626
   clusters fully resolved (converged; 1,561 words learned).
5. **Transfer.** For every other member of a solved cluster, the member's own key is
   reconstructed by zipping its cipher tokens against the cluster's plaintext tokens. This is
   also a **free consistency check** — 4,080 members failed injectivity and were dropped.

Result: **92,420 records decode with no unresolved letter** — 48.0 % of the corpus, 54.0 % of
records with ≥8 letters, 50.2 % of all letters. Whole-record examples, each under its own key:

```
449680  Time Out:Oil supply must be provided:Transmission oil temperature too low:Output
        speed too high (AES):Speed Outside Limit (Sailing):Drivers preferences change:Main
        pump leakage:Reserve
186415  Bank #/# # Sensor #/#: Oxygen Sensor Output Voltage and Short Term Fuel Trim
        associated with this sensor
262790  Excessive internal gear leakage: Volume flow offset of the adjustment pump must be
        increased (#V spools)
168634  Bank # heated oxygen sensor downstream of catalytic converter: exhaust gas
        temperature calculated
419539  Dlt_LOG_VERBOSE (Log messages with the highest communicative level: here all
        possible states: information and everything else can be logged)
```

(`#` marks the unbroken numeric class, §6.)

The 46 % that did not resolve are dominated by **German** records — the corpus is bilingual,
which the `ü ä ö ß` pass-throughs already hinted at — plus abbreviation-heavy short-names, and
records too short to constrain a 26-letter permutation.

---

## 4. Validation

**Independent re-solve under a different key.** For a random 600 clusters whose *second*
member ends up in the shipped catalog, that second record was solved from scratch — same
plaintext, different key, no transfer — and compared with the transferred decode:

```
agree = 599   disagree = 1   unusable = 0     (of 600)
```

The single disagreement is `level_result_OR_OL_49` vs `level_result_MR_ML_49`: a pair of
two-letter abbreviations that no dictionary can separate. An earlier, stricter run scored
**379/379**.

What this test does and does not prove: it proves the recovered plaintext is not an artefact
of one key or one search path. It does **not** by itself exclude word-level near-homograph
errors (`Hill`/`Fill`), because both solves face the same ambiguity — that is what the §7
gate is for, and it is why the shipped catalog is a quarter the size of the decoded set.

**Named-crib test:** §2, 4/4. **Task cribs:** of the 15 measurement names supplied in the
brief, 8 appear verbatim in the shipped catalog (`Transmission Input Speed Sensor`,
`Accelerator Pedal Position`, `Idle Speed Commanded Value`, `Ambient air temperature`,
`Fuel pressure`, `Barometric pressure`, `Charge air pressure: specified value`, and
`Vehicle Speed Sensor` in the decoded set). The rest are either shorter than the 12-letter
gate (`Engine speed`, `Fuel level`) or contain a digit and are excluded by §7
(`Clutch 1: actual position`).

---

## 5. The key is not a function of the record id — every simple family refuted

The prize would have been a rule; there is no evidence of a reachable one.

1. **Not a shift, not affine.** Solved maps are general permutations. `043891` sends
   `t→t, h→e, i→m, u→p, a→r, w→a, s→u` — differences `0, −3, +4, −5, +17, +4, +2`. Fitting
   an affine `y = ax + b (mod 26)` to any two pairs contradicts the third.
2. **Not a rotation of a fixed keyed alphabet.** If every record's permutation were a power of
   one 26-cycle (the classic "secret ring, public shift" construction) all the permutations
   would commute. Over all pairs drawn from the 20 best-solved records, letters where the
   commutator is checkable: **93 commute, 2,627 do not.** A cyclic group would give zero
   violations.
3. **No dependence on the id modulo anything.** Mutual information between `π(cipher 'a')` and
   `id mod N`, over 42,119 records, against the finite-sample independence floor:

   | N | 2 | 7 | 13 | 14 | 26 | 40 | 64 | 256 | 1024 |
   |---|---|---|---|---|---|---|---|---|---|
   | MI (bits) | .0003 | .0033 | .0062 | .0064 | .0122 | .0169 | .0279 | .1133 | .4469 |
   | floor | .0004 | .0026 | .0051 | .0056 | .0107 | .0167 | .0270 | .1092 | .4380 |

   Every value sits on the floor. There is no residue-class structure.
4. **Adjacent ids are unrelated.** For the 130 pairs of consecutive ids where both records have
   ≥20 of 26 letters known, the composition `π_{id+1} ∘ π_id^{-1}` was computed:
   **130 pairs, 130 distinct compositions**, none repeated. The key does not step.
5. **The keys are not reused.** No two records share a payload (§1.2), and the flat bands of
   §1.1 are what independent keys produce.

So the generator is presumably the same VCDS `MT`/`KS` machinery that `label-linkage.md` §2.4
blames for the per-table digit permutation, seeded per record. Recovering it needs that
routine reversed out of the VCDS binary, not more of this data. **It is also no longer on the
critical path for names** — §3 does not need it.

---

## 6. The numeric class is NOT recovered — and here is exactly what was ruled out

Everything above concerns the two letter alphabets. The 14-glyph class `0-9 , . _ -` is
untouched, so every digit inside a name is unknown. `Bank # heated oxygen sensor …` is as far
as it goes.

What was tried:

1. **A crib exists and gives one glyph per record, not fourteen.** Records whose text is an
   ODX SHORT-NAME reveal the cipher glyph standing for `_`: 19,339 such records were
   identified (letter runs joined ≥3 times by one and the same glyph). That is a single
   known pair out of the fourteen the permutation needs — not enough to solve a record on its
   own, but plenty to test structure with.
2. **The glyph key is uniformly distributed.** Over those 19,339 records the cipher glyph for
   `_` has entropy **3.807 bits**, i.e. exactly `log₂ 14`. It is flat.
3. **The glyph key is independent of the letter key.** Mutual information between that glyph
   and each of the 52 candidate predictors (`π(x)` for each cipher letter, `π⁻¹(x)` for each
   plaintext letter) peaks at **0.0155 bits** against an independence floor of 0.0120 bits at
   this sample size. Nothing. The two classes are keyed separately; knowing all 26 letters of
   a record tells you nothing about its digits.
4. **The glyph key is independent of the record id.** Same test against `id mod N`:
   `N = 256` gives MI 0.1231 bits against a floor of 0.1236. `N = 40` gives 0.0199 against
   0.0189 — i.e. at the floor, on 3,315 and 507 contingency cells respectively. Nothing.
5. **`label-linkage.md` §2.4's tricks do not port.** The "zero digit never leads" and
   "which ten glyphs are digits" arguments both need many numbers under *one* key. `TTDOP`
   supplies a thousand rows per table; `TTTEXT` supplies one record per key, with a tail of
   about eight glyphs. There is no per-key sample to do statistics on.

The remaining route is a chained inference — assume enumerated families (`Bank 1`/`Bank 2`,
`Sensor 1`…`4`) are numbered in text-id order and read the digits off that. It was **not**
taken: it is a guess dressed as a deduction, it would contaminate a catalog that is otherwise
verified, and it is exactly the class of move this project has been burned by. Recorded here
as the next person's best lead, not as a result.

**Consequence for names:** records whose name contains a digit are excluded from the catalog
outright (§7). Records whose name is an ODX SHORT-NAME are kept with `_` restored, because
that glyph is identified structurally — it separates letter runs and appears nowhere else in
the name, and ODX SHORT-NAME syntax admits no other separator. That inference is stated
rather than hidden; 5,352 of the 17,009 catalog entries rest on it.

---

## 7. `catalogs/names-uds.json` — what is in it and what it had to pass

**17,009 entries, `{"<6-digit text-id>": "<name>"}`, all names distinct.** Drawn from the
92,420 decoded records by five filters, each of which throws away far more than it keeps:

| filter | rejected |
|---|---|
| name still contains an unresolved numeric glyph (§6) | 30,077 |
| a token of length ≥3 is not in a real dictionary | 9,604 |
| fewer than 12 letters — too little to be sure of | 5,948 |
| **lexically ambiguous** — see below | 24,855 |
| record-framing not cleanly separable — see below | 4,927 |

**The ambiguity filter** is the important one. `Hill bytes to maintain backward compatibility`
is a fluent, dictionary-clean, cross-key-stable decode, and the word is `Fill`: a letter that
occurs exactly once in a record is pinned by nothing but the dictionary. So every token is
re-checked against every word of its pattern class that is consistent with the letters the
*rest of the record* pins, scored with a word-frequency prior measured on the decoded corpus
itself (12,685 types / 384,002 tokens). A record ships only if every one of its tokens beats
its best alternative reading by **20×**. This is the filter that took the catalog from 46,791
entries to 21,936, and it is not optional.

**The framing filter** protects against truncation. The name is the payload minus its trailing
run of glyph-class characters (§1.3) — but if the plaintext itself *ended* in a digit, that
digit is inside the run and gets eaten, silently turning `… of cylinder 4` into
`… of cylinder`. Six identical `Malfunction status of combustion chamber pressure of cylinder`
entries is what that looks like. Two rules remove it: the first character of the trailing run
must recur later in the run (that is what makes it the separator, `<sep><digit><sep><number>`),
and no two records of one cluster may yield the same name. Together they cut the count of
names ending in an enumerable noun from 1,426 to 654, and the survivors read as complete
(`Direction of rotation front right wheel speed sensor`, `Accelerator_Pedal_Position`).

**Known residual risk.** (a) A name that genuinely ends in a digit and survives both framing
rules is truncated and undetectable — bounded above by the 654 above, most of which are fine.
(b) The 20× margin is a likelihood ratio, not a proof; at 17,009 entries a handful of
near-homograph errors is likelier than none. (c) 5,352 entries render `_` by the structural
argument of §6. None of these is hidden behind a claim of certainty.

**What is deliberately not in it.** No `identifier → name` mapping. `label-linkage.md` §3
proved the per-ECU `.rod` sections carry no read identifier, and that is unchanged by this
work; the catalog is keyed by text-id, which is what `MWB` rows and the log's `ENG######`
actually reference. Joining it to `2029`/`380A`/`3816` is the next task, and §2 now supplies
the missing hop for gearbox rows.

---

## 8. Reproduction

The attack is ~400 lines of Python against the already-inflated `[TXT]` blob; it was not added
to `crates/` because it is analysis, not a shipped code path. Sketch, in the order things must
happen:

```
1. lblwords     harvest word types from research/VCDS-25.12.0/Labels/*.lbl  (plaintext)
2. cluster      key = pat('|'.join(letter-runs)) over records with >=8 letters -> 111,730
3. solve        branch and bound per cluster representative, objective 4*len + log freq
4. complete     re-filter each token's pattern class against the letters already pinned
5. bootstrap    feed back new words, repeat 4x (converges at pass 2)
6. transfer     zip each cluster member's cipher tokens against the plaintext tokens;
                injectivity failure = drop the member
7. gate         §7's five filters
```

Checks worth re-running if any of this is touched: the four `ENG######` cribs of §2, the
independent re-solve of §4, and the two mutual-information tables of §5/§6.
