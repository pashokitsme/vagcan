# Gearbox (address 02, DQ200 `0CW300041G`) — discrete state identifiers

What raw UDS identifier carries the **engaged gear**, the **selector lever position**, and
what other discrete state can be read off the same ECU. Derived entirely from the owner's
own recordings — no VCDS crib, no label file, no hardware access at analysis time.

Inputs (all gitignored, read-only):

| file | what |
|---|---|
| `research/dumps/drive-gear.csv` | `vagcan watch --out` over 104 gearbox identifiers, driven home. 1221 data lines, of which **423 carry values** (797 are blank timestamped rows emitted between sweeps, 1 is a truncated final line); `t` 0 → 755 s, sweep period **0.78 s** (1.28 Hz). Two already-proven reference columns: `Input shaft speed` (`0x380A`) and `Output shaft speed` (`0x380B`). |
| `research/dumps/drive-gearbox.csv` | earlier run over the `0x0xxx`/`0x10xx` range, 107 samples, **no shaft-speed columns** → no ratio reference → contributes nothing to the gear proof. |
| `research/dumps/gearbox-full.jsonl` | one-shot dump of all 541 identifiers the gearbox answers, car parked. Used as an independent cross-check of the "P" state. |

Column names in this document are the raw **hex DIDs** as `vagcan watch` labels them:
`3816` means UDS `22 38 16`.

---

## 0. Result at a glance

| finding | verdict |
|---|---|
| `0x3816` = **engaged gear**, `gear = code − 1` (`02`…`08` = 1st…7th), `00` = no gear, `0C` = reverse | **PROVEN** |
| `0x3809` / `0x3815` = **selector lever**, `00`=P `01`=R `02`=N `03`=D (`0x3808` / `0x3818` carry `8 − x`) | **PROVEN** for P/R/D, weak for N |
| `0x2112` = **no gear engaged** flag | **PROVEN** (99.5 % agreement) |
| `0x2170` = **direction of travel**, i8 `+1`/`0`/`−1`; `0x3810` is its `==+1` boolean | **PROVEN** |
| gearbox **mode** (D vs S vs manual) | **NOT FOUND — negative, and unfindable in this data** (§6) |
| **shift-in-progress** flag | **NOT FOUND — negative** (§6) |
| gear ratio of 1st and of reverse | **NOT MEASURABLE** from this drive (§4.3) |
| `0x103D`, `0x1019`, `0x2608`, `0x3800`/`0x3801` | behaviour characterised, **semantics unproven** (§5) |
| catalog row for the gear | **NOT ADDED** — the `Scaling` enum cannot express it honestly (§7) |

---

## 1. Method

A gear cannot be fitted with a line. It is found because it **partitions the shaft-speed
ratio**:

```
ratio = Input shaft speed / Output shaft speed        (both /min, already-proven DIDs)
moving sample := output > 150 /min AND input > 300 /min      → 225 of 423 rows
```

A true gear indicator is the column whose distinct values cut that ratio into tight,
well-separated clusters — one cluster per value — monotonically decreasing as the gear
rises. Quality metric: **η² = 1 − SS_within / SS_total** of the ratio explained by the
column, plus per-cluster coefficient of variation, plus the residual of a
regression-through-origin `input = a · output` inside each cluster (a rigid driveline
forces that residual to ~0).

### 1.1 The sweep skew — a correction that has to be made first

`vagcan watch` polls the 104 identifiers **sequentially** inside one 0.78 s row. The CSV
column order therefore **is a time order**: `Input/Output shaft speed` are read first,
`0x3816` is column 100 of 105, i.e. ~0.75 s *later* than the speeds printed on the same
line. Pairing them naively mis-aligns every sample by nearly a whole poll period.

Tested by scanning the pairing offset:

| offset of `0x3816` vs the shaft speeds | η² | cluster cv (code 08) |
|---|---|---|
| −2 rows | 0.925 | 5.5 % |
| **−1 row** | **0.972** | **2.3 %** |
| 0 (naive) | 0.872 | 5.7 % |
| +1 row | 0.791 | 7.9 % |
| +2 rows | 0.718 | 9.9 % |

The optimum is exactly the one row, in exactly the direction, that the sweep order
predicts. Independent confirmation without touching the ratio at all: `0x3808` and
`0x3818` are the same signal read at opposite ends of the sweep, and of the 423 rows they
disagree in exactly one where both cells are present (one further row has an empty late
cell) — `t = 262.346`, where the early column still reads `07` and the
late column already reads `06`, and the *next* row's early column reads `06`. The late
column carries the newer value. All numbers below use the −1 alignment.

---

## 2. PROVEN — `0x3816` is the engaged gear

### 2.1 It is the only column that explains the ratio

Every column with 2–24 distinct values was scored. There is **no tie**:

| column | η² (naive) | η² (aligned) |
|---|---|---|
| **`0x3816`** | **0.872** | **0.972** |
| `0x103D` | 0.072 | — |
| `0x3801` | 0.021 | — |
| `0x3800` | 0.019 | — |
| `0x1019` | 0.011 | — |
| everything else | < 0.01 | — |

The runner-up explains 7 % of the ratio variance. Nothing competes.

### 2.2 The clusters

Median ratio per code, over sweep-aligned moving samples whose code is stable across the
neighbouring samples too (kills shift transients):

| code | n | median ratio | quartile band | cv | cv after removing the non-locked samples | step from previous |
|---|---|---|---|---|---|---|
| `03` | 3 | 10.0962 | 10.0602 … 10.1138 | 0.22 % | 0.22 % (0 removed) | — |
| `04` | 18 | 6.7969 | 6.7571 … 6.8079 | 6.42 % | **0.63 %** (3 removed) | 1.4854 |
| `05` | 9 | 5.0320 | 5.0234 … 5.0522 | 0.53 % | 0.53 % (0 removed) | 1.3507 |
| `06` | 12 | 3.8013 | 3.7987 … 3.8051 | 0.21 % | 0.21 % (0 removed) | 1.3238 |
| `07` | 63 | 3.0837 | 3.0765 … 3.0936 | 1.45 % | **0.49 %** (2 removed) | 1.2327 |
| `08` | 50 | 2.5723 | 2.5626 … 2.5793 | 0.48 % | 0.48 % (0 removed) | 1.1988 |

Both raw and cleaned cv are given because the honest number for `04` is 6.4 %, not 0.6 %.
The "removed" samples are the five points more than 3 % off their cluster median; all five
are moments when the clutch is demonstrably not locked (§2.5), and they are named
individually there rather than quietly dropped. Even on the raw figures four of six
clusters sit at ≤ 0.5 % and the quartile bands are ±0.1 % throughout. The
regression-through-origin residual inside a cluster is **3–10 /min on a 1500–2500 /min
input**, i.e. 0.15 – 0.6 %. That is a rigid mechanical coupling, not a correlation. The
steps 1.485 / 1.351 / 1.324 / 1.233 / 1.199 are the progressive spacing of a real gearset.

### 2.3 The mapping `gear = code − 1` (proof independent of any published ratio)

From the standstill at `t = 0` the car pulled away and the code stepped

```
02 → 03 → 04 → 05 → 06 → 07 → 08
t=0    3.1   5.5   7.8   10.1  14.8  34.3 s
out=0  79    186   286   375   435   573 /min
```

— **seven consecutive codes, in strict order, on a seven-speed gearbox**, ratio falling
monotonically at each step. `02` is the code the box holds while standing in D and while
creeping (26 samples, output 0…45 /min); a DQ200 launches in 1st, so `02` = 1st gear and
the rest follow. `08` is the highest code seen anywhere in the recording, which is what a
7-speed's 7th should be.

The alternative offset `gear = code − 2` is refuted twice over: it makes `08` the 6th of a
box that would then need a `09`, it leaves `02` meaning "gear 0" while the box is
demonstrably engaged and creeping, and it makes the `03 → 04` step (1.485) a
first-to-second step, which no production gearset has — 1→2 is ~1.6–1.7 everywhere.

| code | gear | ratio (input/output) |
|---|---|---|
| `00` | no gear engaged (neutral) | — |
| `02` | **1st** | not measurable (§4.3) |
| `03` | **2nd** | 10.096 |
| `04` | **3rd** | 6.797 |
| `05` | **4th** | 5.032 |
| `06` | **5th** | 3.801 |
| `07` | **6th** | 3.084 |
| `08` | **7th** | 2.572 |
| `0C` | **reverse** | not measurable (§4.3) |

### 2.4 It is not a speed proxy

The obvious failure mode — a column that merely tracks road speed — is ruled out
directly. Bucketing output speed into 25 /min bins over the moving samples, **19 of 23
bins carry more than one gear code**; bins from 500 to 675 /min each carry codes `06`,
`07` *and* `08`. Point example, 1.6 s apart at the same road speed:

```
t = 43.7   in = 1952  out = 632   ratio 3.09   code 07
t = 44.5   in = 1953  out = 633   ratio 3.09   code 07
t = 45.3   in = 1955  out = 632   ratio 3.09   code 08   ← upshift inside this sweep
t = 46.0   in = 1638  out = 637   ratio 2.57   code 08
```

Output speed is flat at 632–637 /min across all four rows; the ratio drops from 3.09 to
2.57 the moment the code goes `07 → 08`, and lands on the tabulated 7th-gear value. Same
road speed, two codes, two ratios exactly as §2.2 predicts.

### 2.5 Whole-drive validation

Taking the six ratios of §2.2 as a lookup table and predicting the input shaft speed from
`(code, output speed)` over **all 225 moving samples** — including the transients the
cluster table deliberately excluded:

```
median |error| = 0.32 %      83.6 % within 2 %      88.4 % within 5 %
p90 = 6.7 %     p99 = 23.9 %     max = 28.8 %
```

The tail is fully accounted for and physical, not noise:

* rows where the shift happened *inside* the 0.78 s sweep — the code and the speeds are
  then genuinely from different gears, and no alignment can fix it;
* rows where the **clutch is not locked**, which decouples the two speeds entirely — the
  gear stays mechanically engaged and the code stays correct, but the ratio is free. All
  five of the >3 % cluster outliers from §2.2 are of this kind, and they are:
  * `t = 153.7` and `t = 154.5` (code `04`) — coasting to a stop at zero pedal in 3rd:
    `out = 175`, geometric input 1190, **measured 945**, i.e. the clutch has opened and
    the shaft has fallen back to engine idle;
  * `t = 64.0` (code `07`, 2.920 vs 3.084) — the same, milder;
  * `t = 81.2` (code `04`, 7.195) and `t = 172.5` (code `07`, 3.376) — input *above*
    geometric, i.e. the clutch slipping under load, the other direction of the same
    failure of the rigid-coupling assumption.

That last point also says something about the reference DID itself: **`0x380A "Input
shaft speed" reads the engine side of the clutch**, not a decoupled gearbox shaft. It sits
at ~800 /min (idle) with the car parked in P and gear code `00`. So the ratio measured
here is a whole-driveline ratio (engine rev per output-shaft rev), which is why the
absolute values 2.57–10.10 are far larger than any published *internal* DQ200 gear ratio.

### 2.6 Engaged gear or commanded gear? — timed, not settled

For each of the 38 shift events between two ratio-known codes, the moment the code changed
(corrected for the 0.75 s sweep offset) was compared with the moment the ratio crossed the
midpoint of the two cluster values:

```
median lead = -0.03 s     mean = -0.13 s     range -1.59 … +0.75 s
30 of 38 events within one sweep period (0.78 s)
25 events simultaneous, 9 with the code leading by 1–2 samples, 4 lagging by <1 sample
```

So the code **never lags the mechanics by more than one sample, and sometimes leads by
one or two**. That is exactly what "the code flips when the shift is commanded while the
torque handover takes a few hundred ms" predicts — and it is equally what a *commanded /
target* gear register would look like. **At 1.28 Hz the two cannot be separated.** The
name used throughout this document is "engaged gear" because it is right at every steady
state, which is the case that matters for display; a recording at ≥ 10 Hz on `0x3816`
alone would settle it.

---

## 3. PROVEN — selector lever at `0x3809` / `0x3815`

Four distinct values, and `0x3808 + 0x3809 = 8` holds in **422 of 422** rows where both
are present (`0x3818`/`0x3815` are the same two signals read later in the sweep;
`0x3808 == 0x3818` in 420 rows, the exceptions being the sweep-skew rows of §1.1). So
there are really only two lever DIDs' worth of information, in two encodings.

| `0x3809` | `0x3808` | n | co-occurring gear code | motion | verdict |
|---|---|---|---|---|---|
| `00` | `08` | 76 | `00` (always) | output ≤ 17 /min, always | **P** |
| `01` | `07` | 48 | `0C` in 46, `00` in 2 | output ≤ 36 /min, moving | **R** |
| `02` | `06` | 4 | `00` (always) | pass-through only | **N** (weak) |
| `03` | `05` | 294 | `02`…`08` | all forward driving | **D** |

Why the R/N assignment is not a coin flip:

* During `0x3809 = 01` a gear **is** engaged — the no-gear flag `0x2112` (§4.1) reads
  "gear engaged" and `0x3816` reads `0C`, a code that appears nowhere else. During
  `0x3809 = 02` no gear is engaged. Neutral by definition has no gear engaged; reverse
  does. That alone fixes `01` = R and `02` = N.
* `0x2170` (§4.2) takes its third value `FF` — which occurs **nowhere else in the entire
  recording** — only while `0x3809 = 01` and the car is rolling. A direction-of-travel
  signal going negative is reverse.
* The numbering is the physical lever order: P=0, R=1, N=2, D=3 top to bottom.
* Independent session cross-check: `research/dumps/gearbox-full.jsonl`, dumped with the
  car parked, has `3808 = 08`, `3809 = 00`, `3816 = 00` — P, no gear. Consistent.
* Behavioural cross-check: the recording ends with a textbook parking manoeuvre —
  `t = 228` lever→R and the car rolls backwards; `t = 238.5` lever→D and it creeps forward
  (gear `02` → `03`, output 49); `t = 243.6` back to R; `t = 257–265` two passes through
  `06`; `t = 277.9` → `08` and the car never moves again. P/R/N/D behave exactly as the
  labels demand.

**Caveat on N (`02`):** only 4 samples, all lever pass-throughs. The assignment rests on
"no gear engaged" plus the lever ordering, not on sustained observation. Everything else
here is solid; N is *consistent*, not independently demonstrated.

**Caveat on D (`03`):** the drive was made entirely in D. Whether S (or a tiptronic mode)
has its own value in this DID, or is reported elsewhere, is untested — see §6.

---

## 4. Other discrete state

### 4.1 `0x2112` = no gear engaged — PROVEN

`0x2112 == 01` and `0x3816 == 00` agree in **418 of 420** rows (99.5 %); the two
disagreements are single-row sweep-skew transitions. It carries no information beyond the
gear code, but it is a clean, independently readable neutral flag.

### 4.2 `0x2170` = direction of travel — PROVEN

Three values, and with the sweep alignment applied the partition is clean:

| value | as i8 | standstill (out < 10) | forward | reverse |
|---|---|---|---|---|
| `00` | 0 | **104** | 0 | 0 |
| `01` | +1 | 6 | **277** | 1 |
| `FF` | −1 | 9 | 0 | **25** |

`00` occurs **only** at standstill; `FF` occurs **only** with the reverse gear code; `01`
covers forward motion. The mismatches sit entirely in the standstill deadband, where the
sign of a ~5 /min output speed is a matter of definition. Read `0x2170` as a signed byte.

`0x3810` is exactly `(0x2170 == 01)` — **422 of 422** rows. Redundant.

### 4.3 What could NOT be measured

* **1st gear ratio (code `02`)** and **reverse ratio (code `0C`)**. Both codes only ever
  occur below ~45 /min output, where the dry clutch is slipping by design; the measured
  ratio there is never below 35 and rises without bound as the car slows (observed range
  35.2 … 1214 over the 34 reverse samples with non-zero output). There is no locked-up
  sample of either. Reporting a number would be fabricating one.
* Consequently the `02` = 1st claim rests on the launch-sequence argument of §2.3, not on
  a ratio measurement. That argument is strong but it is a different kind of evidence, and
  it is the one link in the chain that a second recording (a hard launch held in 1st to
  ~30 km/h) would upgrade from "forced by the structure" to "measured".

---

## 5. Characterised but UNPROVEN

Reported as behaviour, not as meaning. None of these is safe to name.

| DID | behaviour | why it is not called |
|---|---|---|
| `0x103D` | `00` (parked dump) → `01` (t < 1.6 s) → `02` → `03` (t = 64 s onward). Monotone, never falls. | **Two transitions in 755 s, one of them at t = 1.6.** Exactly the "changes once" failure the method warns about. Shape suggests a warm-up / readiness ladder; that is a guess. |
| `0x1019` | Counts *down* `05→04→03→02→01→00` during the launch and again during two deceleration cascades; counts *up* during others; `0F` once at the very end. | A counter of something, plausibly a countdown to the next shift. No hypothesis survives all six episodes. |
| `0x2608` | `01` in 68 rows, **all** with output < 150 /min, and only with gear `0C`, `02`, `03` or `00`. `00` also occurs at low speed, so it is not a speed threshold. | Behaves like a slip/creep indicator. Confounded with low speed; cannot be separated with this data. |
| `0x3800`, `0x3801` | Equal to each other in 421/423 rows. `== 01` implies accelerator = 0.0 % in **194 of 194** rows; the converse fails (43 of 237 zero-pedal rows read `00`). | A one-way implication only. Something coasting/overrun-related, but 43 counter-examples mean it is not "pedal released". |
| `0x3811` | Tracks `0x2170`/`0x3810` loosely, with its own `FF` excursions during the parking manoeuvre. | Noisier duplicate of the motion signals. No distinct meaning isolated. |
| `0x1003` | 45 transitions, correlates with nothing tested. | — |
| `0x104D`, `0x2113`, `0x2114`, `0x1035`, `0x3813` | one or two isolated transitions each, all in the last 5 s (shutdown) or as 1–2 sample glitches. | Single-event columns. Refused on principle. |

---

## 6. NEGATIVES

**Gearbox mode (D / S / manual) was NOT found, and cannot be found in this recording.**
The lever never left P/R/N/D for the whole 755 s — `0x3809` takes exactly four values and
S was never selected. No column can be shown to encode a mode that never occurred. This is
not "no candidate scored well"; it is an absence of the stimulus. It needs a new recording
in which the owner deliberately selects S (and, if the car has it, the tiptronic gate) with
`0x3809`/`0x3815`/`0x103D` and the whole `0x38xx` block in the watch list.

**No shift-in-progress flag exists among the polled identifiers.** 56 of the 423 rows are
gear-change rows. Every 2–4-valued byte column was tested for `P(col = v | gear changed)`
being both > 0.5 and more than double its baseline. **Zero columns qualify.** Either the
gearbox does not publish such a flag in this identifier range, or a DSG shift (~0.3 s) is
simply shorter than the 0.78 s sweep and is aliased away. The second explanation is
plausible enough that this negative is about *this recording*, not about the ECU.

**No target/requested gear identifier was found.** Nothing else in the `0x38xx` block
behaves like a gear. `research/dumps/drive-gearbox.csv` (the `0x0xxx`/`0x10xx` range) was
surveyed for staircase-shaped columns; it contains 16-bit analogue quantities and nothing
gear-shaped, and it has no shaft-speed columns to test against anyway. And `0x3816` itself
cannot be *distinguished* from a target gear at this sample rate — see §2.6, where the
measurement was actually made: a median lead of −0.03 s with a ±1-sample spread.

### 6.1 A comparison deliberately NOT used as evidence

The observed ratios were checked against DQ200 published gear ratios. **That check is
recorded here only to be honest that it was made; it is not part of any proof above.** The
ratio set available to the author is model recall, not a sourced document, and normalising
to 7th it matches the `code − 1` mapping to within 1 % for 4th–7th and diverges by 6–10 %
for 2nd–3rd, with an unexplained constant scale factor of ~1.29 (the §2.5 finding that the
"input shaft" DID reads the engine side is the likely reason, but that is not established
either). A number that half-matches from an unverifiable source is worth nothing. The
mapping stands on §2.3 — seven consecutive codes on a seven-speed box, in order, from a
standstill — which needs no external ratio at all.

---

## 7. Why no catalog row was added

`catalogs/gearbox-02.json` was read and **left untouched**. It holds 10 rows, every one
`Scaling::Linear{factor, offset}` over a `RawForm`. The `Scaling` enum
(`crates/vag-data/src/catalog.rs`) offers exactly two variants: `Linear` and `Anchor`
(a single proven point).

The gear is an **enumeration**, not a scaled quantity:

* `Linear{factor: 1.0, offset: -1.0}` would reproduce 1st…7th correctly and then emit
  **"gear 11" for reverse** (`0x0C`) and **"gear −1" for neutral** (`0x00`). Those are not
  approximations, they are false statements, and `0x0C`/`0x00` are 128 of the 423 samples —
  30 % of the recording. That is precisely the invented-linear-row this project's catalog
  doctrine forbids.
* `Anchor` is worse: it would honour one code and refuse the other eight.
* The selector lever (P/R/N/D) is categorical outright and has no linear reading at all.

**What the catalog needs before these rows can be added:** a third `Scaling` variant —
something like `Enum { map: Vec<(i32, Cow<'static, str>)> }` — mapping a raw integer to a
label, with `interpret` returning a symbolic value rather than `f64`, and no fallback for
unlisted raws. That is a change to `crates/vag-data/src/catalog.rs` (and to
`MeasurementDef::interpret`'s return type or a sibling method), which is out of scope for
this analysis and would also break the shipped-catalog row-count test. Until then, the
knowledge lives here, in this document, where it is honest.

The three rows that are ready the moment that variant exists:

```
Gear engaged            UDS 0x3816  U8First   Enum { 0x00: "-", 0x02: "1", 0x03: "2",
                                                     0x04: "3", 0x05: "4", 0x06: "5",
                                                     0x07: "6", 0x08: "7", 0x0C: "R" }
Selector lever position UDS 0x3809  U8First   Enum { 0x00: "P", 0x01: "R",
                                                     0x02: "N", 0x03: "D" }
Direction of travel     UDS 0x2170  I8First   Enum { 0x01: "forward", 0x00: "standstill",
                                                     0xFF: "reverse" }
```

---

## 8. Confidence

| claim | confidence | what would break it |
|---|---|---|
| `0x3816` is the engaged-gear code | **very high** | nothing in this data; η² 0.97, cluster cv 0.2–0.5 %, no competing column, whole-drive median error 0.32 % |
| `0x3816` codes `03`…`08` = gears 2…7 | **very high** | six measured, monotone, tightly-clustered ratios |
| `0x3816` code `02` = 1st | **high** | rests on the launch sequence + 7-speed structure, not on a measured ratio; a held-1st recording would settle it |
| `0x3816` code `0C` = reverse | **high** | gear engaged, lever position distinct from P/N/D, `0x2170` negative — but no measured ratio |
| `0x3809` `00/01/03` = P/R/D | **high** | consistent across two sessions and a full parking manoeuvre |
| `0x3809` `02` = N | **medium** | 4 pass-through samples only |
| `0x2112`, `0x2170`, `0x3810` | **high** | clean partitions, ≥ 99 % agreement |
| mode / shift-flag / target gear | **no claim** | absent from this data by construction |

All numbers above are reproducible from `research/dumps/drive-gear.csv` alone; no Rust
source and no catalog file was modified in producing them.
