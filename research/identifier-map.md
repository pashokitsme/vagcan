# Identifier map — engine `8V0906264H` / gearbox DQ200 `0CW300041G`

Analysis of the two exhaustive UDS `ReadDataByIdentifier` sweeps plus the two `vagcan watch`
driving recordings. Vehicle: Škoda Octavia III (MQB), VIN `XW8AD4NE9JH008917`, 1.8 TFSI CJSA.

**Status legend used throughout**

| tag | meaning |
|---|---|
| **PROVEN** | fitted against a VCDS log with `vagcan analyse`, or forced by a published standard (OBD-II / ISO 15031-5) |
| **STRUCTURAL** | derived from the data itself with no free parameters (record layout, byte order, aliasing, gear-ratio arithmetic) |
| **HYPOTHESIS** | consistent with the numbers, *not* proven. Do not ship a scaling on this basis |
| **UNEXPLAINED** | says so |

Nothing in this document was obtained from `.rod` label files. Per `research/rod-labels.md` §4.0c
they do not contain the read identifiers; that path stays closed.

---

## 0. Read this before using the data — three caveats that change every conclusion

**0.1 The engine sweep was taken with the engine OFF.** 450 of 896 engine identifiers (50 %)
answer all-zero. That is almost certainly "no signal right now", not "not implemented". Whole
families read 100 % zero (`17xx` 15/15, `46xx` 8/8, `09xx` 3/3) and are probably live families
that were simply idle. **The engine sweep therefore massively understates which identifiers
carry data.** A repeat sweep with the engine running is the single cheapest thing that would
improve this map. Same caveat, weaker, on the gearbox: 226/541 (42 %) zero.

**0.2 Both driving recordings are of the GEARBOX. There is no engine driving recording at all.**
`drive-gear.csv` is misnamed — 101 of its 101 hex columns resolve against `gearbox-full.jsonl`,
its decoded columns are the proven gearbox rows (`380A` input shaft, `380B` output shaft,
`3804` pedal), and only 10 of its columns even exist on the engine. So every engine entry below
is static-only: it has never been observed moving. Consequently the ranked shortlist is
gearbox-heavy by necessity, not by preference.

**0.3 The two recordings cover disjoint, and incomplete, slices of the gearbox.**

| file | duration | samples | gearbox block covered | notable gap |
|---|---|---|---|---|
| `drive-gear.csv` | 755 s | 1221 rows | `38xx` only `3800–381F` (nearly all 1-byte flags) | the entire 2-byte `3820–38FF` block — where the proven clutch/pressure rows live — was **not** recorded |
| `drive-gearbox.csv` | 142 s | 108 rows | `10xx` only `1000–1087` | `1088–10FF`, all of `11xx`, all of `38xx` |

`drive-gear.csv` is also **sparse**: only ~423 of 1221 rows carry the DID columns (the rest are
empty cells); only the two decoded shaft-speed columns sample every row. "Constant across the
drive" for that file means constant across ~423 samples, which is still adequate.

**0.4 The identifier number space is ECU-LOCAL.** 102 identifiers exist on both ECUs; only 16
carry the same value, and those 16 are either zero or genuine standard identifiers
(`F186`, `F190`, `F1DF`). Everything else differs in value *and often in length*
(`1090`: engine 4 bytes, gearbox 2 bytes). Never carry a meaning across ECUs.

---

## 1. Family structure

### 1.1 Engine — 896 identifiers, 44 high-byte families

| family | n | all-zero | dominant lengths | character |
|---|---|---|---|---|
| `00xx` | 2 | 0 | 8 B | two packed 4×u16 records (see §5) |
| `01xx` | 11 | 8 | 1 B, 3×90 B, 64 B | 90-byte timestamped history logs (§5) |
| `02xx` | 6 | 2 | 2 B, 2×64 B | mixed; `02A0` 64-byte record |
| `03xx` | 2 | 1 | 1 B | negligible on the engine (contrast gearbox) |
| `04xx` | 9 | 6 | 1 B, 3×10 B, 89 B | `0407`/`0408` = `0001` ×5 arrays; `0481` 89-byte record |
| `10xx` | 65 | 38 | 1 B / 2 B | mixed scalars |
| `11xx` | 77 | 25 | 2 B (42), 1 B, **9×9 B** | the 9 B entries are the ASCII string `NO_ERROR` (§5) |
| `12xx` | 21 | 9 | 2 B | mixed |
| `13xx` | 95 | 71 | 2 B (66) | largest engine family, 74 % zero with engine off — **prime suspect for live data** |
| `14xx` | 63 | 38 | 2 B (48) | contains obvious paired/mirrored rows (`1466`=`1468`, `146B`=`146D`, `1475`=`1477`) |
| `15xx` | 21 | 10 | 2 B | |
| `16xx` | 74 | 54 | 1 B / 2 B | 72 % zero; `16DF` = ASCII `SC8O4010 CBL20E0` |
| `17xx` | 15 | **15 (100 %)** | 1 B / 2 B | entirely silent with the engine off |
| `20xx` | 81 | 35 | 2 B (58) | **contains the proven boost pair `2029`/`202A` and the proven RPM `206E`** |
| `29xx` | 61 | 22 | 2 B (44) | shares many values with `20xx` — looks like a specified/actual or bank-2 twin |
| `39xx` | 52 | 27 | 2 B (37) | shares `03DF` with the proven boost rows (§3.1) |
| `3Dxx`–`44xx` | ~120 | ~55 | 2 B | mid-size families, mostly 2-byte |
| `46xx` | 8 | **8 (100 %)** | 2 B | silent |
| `4Exx` | 5 | 3 | 4×16 B | 16-byte structured records (§5) |
| `4Fxx` | 4 | 1 | 3×90 B | second timestamped history log (§5) |
| `F1xx` | 22 | 1 | ASCII/BCD | identification — **PROVEN** (§2) |
| `F4xx` | 42 | 9 | 1/2/4 B | **OBD-II service 01, PROVEN** — `F400 + PID` |
| `F6xx` | 17 | 0 | 4/8/17/26/35 B | **OBD-II service 06, PROVEN** (§2.2) |
| `F8xx` | 6 | 0 | 4/5/17/18/21/41 B | **OBD-II service 09, PROVEN** (§2.2) |

Engine encoding: **big-endian**, confirmed again here — the timestamps in `0153`/`4F39` only
parse as sane dates read big-endian (§5.1), matching the already-proven `206E`/`2029`/`202A`.

### 1.2 Gearbox — 541 identifiers, 19 high-byte families

| family | n | all-zero | dominant lengths | character |
|---|---|---|---|---|
| `01xx`–`02xx` | 15 | 7 | 1/2/6 B | `0285`/`0286` sit with `1000`/`1001` (§3.2) |
| `03xx` | **60** | **60 (100 %)** | 54×1 B, 6×2 B | contiguous `0300–0343`; **zero in the static sweep AND constant-zero through both drives.** A per-item status/flag array, almost certainly fault or component-status bits. Effectively dead weight for measurement work |
| `10xx` | **228** | 90 | **174×2 B**, 33×4 B, 17×1 B | the main measurement block; `1000–1087` is live sensor data, `1088–10B6` is a repeating adaptation record (§4.3), `10BB–10FF` mostly zero |
| `11xx` | 49 | 11 | 2 B, 4 B, **5×44 B** | identification + packed adaptation records (§5) |
| `38xx` | **145** | 48 | **101×2 B**, 36×1 B | the DSG-specific block. `3800–381F` = 1-byte state/selector flags (§4.1); `3820–38FF` = 2-byte clutch/pressure/torque values, including all six proven rows |
| `21xx`, `26xx`, `2Bxx`, `70xx` | 10 | 4 | 1/2 B | small, mostly flags |
| `17xx`, `18xx`, `3Bxx` | 3 | 0 | 24/4/8 B | singletons |
| `F1xx` | 20 | 0 | ASCII/BCD | identification — **PROVEN** |
| `F4xx` | 2 | 2 | 2 B | only `F40C`/`F40D`; the gearbox is **not** an OBD emissions ECU |

Gearbox encoding: **little-endian**, confirmed independently and overwhelmingly here.
In `drive-gearbox.csv` every varying 2-byte column has a tight physical range read
little-endian and a nonsense wrap-around range read big-endian — e.g. `102D` is
`2358..3279` LE versus `779..65034` BE. This is **STRUCTURAL**, not assumed.
Signed i16 LE is also in use: `1080`, `1084`, `1085`, `1086`, `1056`, `106B`, `100B`
all sit just below `0xFFFF` and behave as small negative numbers.

---

## 2. What is PROVEN

### 2.1 Already established before this analysis (restated for completeness)
Engine `206E` engine speed (u16 BE ×1 /min); `2029`/`202A` boost specified/actual
(u16 BE ×0.001 bar); gearbox `380A`/`380B` input/output shaft speed (u16 **LE** ×1);
`3804` pedal ×0.4 %; `3832`/`383B` ×0.01 %; `38F6`/`38F9`/`38AC`/`38AD` clutch positions
×0.01 mm; `F19E` ODX file name; `F187`/`F189`/`F191`/`F197` identification strings;
engine `F400 + PID` = OBD-II mode 01.

### 2.2 New, and forced by the standard rather than guessed

**`F6xx` = OBD-II service 06 (on-board monitoring test results).** The proof is the
supported-MID bitmask pattern at 0x20 boundaries, exactly as service 01 does at
`F400`/`F420`/`F440`…:

```
F600 C0000001   F620 80000809   F640 C0000001   F660 00000001   F680 00000001   F6A0 78000000
```

and the payload rows (`F601`, `F602`, `F635`, `F6A2`–`F6A5`) are runs of the
`MID / TID / value / min / max` test-result tuples that service 06 defines.

**`F8xx` = OBD-II service 09 (vehicle information).** Decisive, because the payloads
self-identify:

| DID | service 09 PID | payload |
|---|---|---|
| `F800` | supported-PID bitmask | `55400000` |
| `F802` | PID 02 VIN | `01` + `XW8AD4NE9JH008917` |
| `F804` | PID 04 CALID | `01` + `8V0264H 0005AEAJ` |
| `F808` | PID 08 in-use performance tracking | 41 B, 10 × u32 counters |
| `F80A` | PID 0A ECU name | `01` + `ECM\0-EngineControl\0\0` |

So the engine exposes **three** complete OBD services through the UDS DID space
(`F4xx`=01, `F6xx`=06, `F8xx`=09). This is a standardised mapping, so decoders can be
written for all of them without any car time. The gearbox exposes none of it
(`F40C`/`F40D` only, both zero).

**`F15B` = flash/programming history**, 10-byte records of
`[BCD date YY MM DD][6-byte programmer/tester id][00]`:

| ECU | records | dates | note |
|---|---|---|---|
| engine | 5 (all identical) | 2017-04-06 | tail `0001FE2D1ED6` = exactly `F19A` |
| gearbox | 3 (distinct) | 2017-02-27, 2017-03-03, 2017-04-06 | last tail `0000DE2C8251` = exactly `F19A` |

Both ECUs were programmed 2017-04-06 — factory build, consistent with the `J` (2018) VIN
model-year code. `F19A` is the *most recent* programming id, `F15B` the whole log.

**Engine `2029`/`202A` scaling independently cross-checked.** With the engine off, boost
actual must equal ambient. `2029` = `03DF` = 991 → 0.991 bar. OBD `F433` (PID 33, barometric
pressure) = `0x63` = 99 kPa, and `F40B` (PID 0B, MAP) = `0x62` = 98 kPa. The ×0.001 bar scaling
lands within 1 kPa of two independent standard PIDs. Not new proof, but a useful sanity anchor.

### 2.3 The static-capture anchor for the engine
From the standard PIDs, at the moment `engine-full.jsonl` was swept the engine was
**warm and just switched off**: coolant `F405`=`0x72`→74 °C, intake air `F40F`=`0x69`→65 °C
(heat-soaked), baro 99 kPa, 2 warm-ups and 34 km since DTC clear (`F430`, `F431`).
This is the reference frame for every engine hypothesis below — a raw byte near 114 is
"about 74 °C" if and only if it is a coolant-temperature mirror.

---

## 3. Live sensors identified from the driving recordings (gearbox)

### 3.1 Gear engaged — `3816` (STRUCTURAL, high confidence)
`3816` takes values `00, 02..08, 0C` while driving. Bucketing the measured ratio
input/output shaft speed (`380A`/`380B`, both proven) by `3816`:

| `3816` | 02 | 03 | 04 | 05 | 06 | 07 | 08 | 0C |
|---|---|---|---|---|---|---|---|---|
| median `380A/380B` | (launch, OSP≈0) | 10.09 | 6.80 | 5.04 | 3.80 | 3.08 | 2.57 | (OSP≤36) |

Successive step ratios 1.483, 1.350, 1.325, 1.233, 1.199 match the published DQ200 gear-step
ratios (2→3 = 1.484, 5→6 = 1.234, 6→7 = 1.211) with a **one-count offset**: `3816` = engaged
gear + 1, i.e. `02`=1st … `08`=7th. `0C` (12) only ever occurs with the selector in the
reverse position and output speed ≤36, so `0C` = reverse. `00` = no gear (P/N).
This is arithmetic on two already-proven rows, so it is as close to proof as anything
obtainable without a VCDS log — but the exact code table (what `01` would mean, whether
`00` distinguishes P from N) is still a hypothesis.

### 3.2 Selector lever — `3808`/`3818` and `3809`/`3815` (STRUCTURAL)
Perfect cross-tab over 423 samples:

| `3808`=`3818` | `3809`=`3815` | co-occurring `3816` | reading |
|---|---|---|---|
| `08` | `00` | `00` | P |
| `07` | `01` | `0C` | R |
| `06` | `02` | `00` | N |
| `05` | `03` | `02..08` | D |

So `3808`/`3818` are one encoding of the PRND lever (descending 8→5) and `3809`/`3815`
another (ascending 0→3), duplicated as a specified/actual or requested/actual pair.
`3810` is `1` almost exclusively in D and N, `0` in P and R — a "drive enabled"-like flag,
but not clean enough to call.

### 3.3 Supply voltage — gearbox `0285`, `0286`, `1000`, `1001` (HYPOTHESIS, strong)
u16 LE. Engine-off sweep: `0285`=120, `0286`=`1000`=`1001`=123. During the drive:
`0285` = 134..139, `0286`/`1000`/`1001` = 138..142. Read ×0.1 V that is **12.0/12.3 V at rest,
13.4..14.2 V running** — the textbook battery-then-alternator profile, and it brackets the
~12.75 V rest figure. Two rails ~3–4 counts apart (terminal 30 vs internal, presumably).
`3812` = `7C00` = 124 in the static sweep is a fourth copy in the `38xx` family.
Cheap to prove and worth proving first because it calibrates the whole LE ×0.1 pattern.

### 3.4 Hydraulic pressure — gearbox `102D` (HYPOTHESIS, strong shape evidence)
u16 LE, 103 distinct values in 108 samples, ranging 2358..3279, in a clean repeating
**sawtooth**: slow linear decay, abrupt jump back up. At rest (static sweep) it reads 437.
That is precisely the behaviour of a DQ200 electro-hydraulic accumulator — pump recharges
on a low threshold, pressure bleeds down between shifts, and drains away when parked. The
shape argument is strong; the scaling is not established (×0.02 bar would give 47..66 bar,
which is the right ballpark for a DQ200 accumulator, but that is numerology until fitted).

### 3.5 Torque cluster — gearbox `100B` / `100C` / `100D` (HYPOTHESIS)
`100B` is **signed** i16 LE and swings −500..3200; `100C`/`100D` are unsigned 0..3500/3560 and
track it closely. All three drop to exactly 0 during the drive, which rules out engine speed
(the engine was running). A signed quantity that goes negative on overrun and pins at zero
is torque — most likely actual (`100B`, signed, drag-capable) versus requested/limited
(`100C`, `100D`). Unexplained: the unit. ×0.1 Nm would give up to 356 Nm against a 250 Nm
engine, so the factor is probably not 0.1.

### 3.6 Slow monotone risers — gearbox `1017`, `102A`, `102B`, `102C` (HYPOTHESIS: temperatures)
Over the 142 s drive, with no reversals:

| DID | width | static (parked) | drive start → end |
|---|---|---|---|
| `1017` | u8 | 134 | 116 → 158 (peak) |
| `102A` | u16 LE | 60 | 92 → 127 |
| `102B` | u16 LE | 56 | 86 → 100 |
| `102C` | u16 LE | 63 | 93 → 134 |

Four independent, monotonically rising channels with different time constants is what a
set of temperatures looks like, and a DQ200 has several worth reporting (dry clutch 1,
dry clutch 2, gearbox oil, mechatronic). `102C` rises fastest — a dry clutch heats fast.
**But the offset is undetermined**: `102A`=92 could be 92 °C or 52 °C (raw−40), and both are
physically plausible for a warm car. `1017` does not fit the same family — its static value
(134) sits in the *middle* of its driving range while `102A`'s static value (60) sits well
below — so `1017` is measuring something else, possibly a coolant temperature copied over
CAN from the engine.

### 3.7 Slow monotone fallers and steppers — adaptation values (HYPOTHESIS)
`1015` 64→45, `1014` 106→102, `106F` 333→302, `105A` 325→307 drift down monotonically;
`1071` 2905→2976, `105C`, `1061`, `1083`, `1084`, `1085`, `1086`, `1087` change in one or two
discrete steps and then hold. Signed negatives `1080` (−102..−14), `106B` (−193..−172),
`1056` (−173..−150) drift smoothly. Continuous drift plus discrete step updates is the
signature of clutch touch-point / adaptation learning, not of a raw sensor. Low priority for
a measurement catalogue, high interest for a "gearbox health" feature later.

### 3.8 Shift-event triple — gearbox `102F`, `1030`, `1031` (STRUCTURAL)
These three spike on exactly the same samples and are zero otherwise: `1031` toggles 0/1,
`1030` jumps to 3..14, `102F` jumps to 1987..2213. An event flag, an event code and an event
magnitude, latched for one poll. Worth capturing during deliberate shifts.

### 3.9 What did NOT move (gearbox)
All 60 identifiers of `03xx` — constant `00` through both drives *and* zero in the static
sweep. `0410`=`0F`, `04FB`, `1002`, `1012`, `1013`, `1018`, `1034`, `1045`, `1046`, `1049`,
`104A`, `1051`, `1053`, `119B`, `11AF`, `2B2C`, `3802`, `3805`–`3807`, `3814`, `3819`, `381A`,
`381F` likewise. Treat these as coding/status constants, not measurements.

---

## 4. Engine — hypotheses only, no drive data

Everything here is static-only. Nothing in this section should be shipped.

### 4.1 Pressure sensors readable from the ambient anchor
With the engine off every absolute-pressure sensor must read ambient. `2029`/`202A` prove
ambient = `03DF` = 991 in u16 BE ×0.001 bar. **Three further identifiers read exactly `03DF`:
`39C0`, `39C2`, `3E70`.** HYPOTHESIS: these are also absolute pressures in mbar, u16 BE.
Same logic makes `2028`, `276D`, `4336`, `4384` (all `03E8` = 1000, a suspiciously round
number) more likely to be *defaults/limits* than live sensors.

### 4.2 Temperature-shaped bytes
Using the anchor (coolant 74 °C ↔ raw 114, IAT 65 °C ↔ raw 105):

- `1003` = `0x69` = 105 — **exactly** the raw `F40F` intake-air-temperature byte.
  HYPOTHESIS, strong: `1003` is an intake air temperature, u8, ×1 −40.
- `11F7` = 116, `11CD` = 115 → ~76/75 °C. Plausible coolant mirrors.
- `11D0` = `11D4` = `132F` = `14A6` = 122, `16B0` = 121 → ~82/81 °C. Plausible oil temperature.
- `1432`..`143E` = 131, 164, 57, 77, 97, 111, 124 — a monotone tail (57, 77, 97, 111, 124)
  reads more like a characteristic-curve table than a set of sensors.

Every one of these is "consistent with a temperature". None is a finding.

### 4.3 Duplicated pairs and twin families
`14xx` has exact duplicate pairs (`1466`=`1468`=`0019`, `146B`=`146D`=`006B`,
`1475`=`1477`=`000D`, `1470`=`1472`=`0014`) and `11xx` has an exact duplicate triple
(`112D`/`112E`/`112F` = `113F`/`1140`/`1141` = `0074`/`FF72`/`00B7`, i.e. +116/−142/+183
signed BE). Two identical banks of three signed values is what per-bank or per-pair
trim/adaptation looks like. `20xx` and `29xx` share nine values pairwise
(`2004`=`294A`, `2930`=`3959`, `209C`=`29D4`, `20A0`=`29D5`, `2061`=`20EB`=`29BC`, …),
suggesting `29xx` is a specified/actual or second-bank twin of `20xx` — which matters
because the proven boost pair lives in `20xx`.

### 4.4 Unexplained engine values worth naming
`121F` = `2998` = `0x37DC` = 14300, and the same 16-bit value appears inside the packed record
`00FF`. A five-digit counter replicated in three places is odometer- or operating-hours-shaped,
but this is pure speculation — it could equally be a calibration constant.

---

## 5. Long responses (>8 bytes) — structured records

These are packed multi-field records, not scalars. They are the highest-yield targets for a
structural decoder because one read returns many fields.

### 5.1 Timestamped history logs — engine `0153`, `4F39` (STRUCTURAL, decoded)
90 bytes = **10 slots × 9 bytes**, layout `[00][u32 BE seconds][01 00 00 00]`, unused slots
`FF`-filled. The u32 is a **Unix epoch timestamp**:

```
0153  2026-05-03T16:12:39  2026-05-03T07:32:35  2026-04-30T10:58:54
      2026-04-30T09:51:52  2026-04-21T06:31:49  2026-04-17T22:38:38   (6 of 10 used)
4F39  2026-03-26T15:32:38  2024-03-31T10:14:50  2024-03-31T07:38:58
      2024-03-30T23:55:10                                            (4 of 10 used)
```

Reading them big-endian is what produces sane dates — an independent confirmation of engine
byte order. The all-`FF` companions `0154`, `0155`, `4F3A`, `4F3B` are empty logs of the same
shape. What event is being logged is **UNEXPLAINED** (DTC set? adaptation? session?), but the
recency of the `0153` entries makes it something that happens every few days.

### 5.2 Packed 4×u16 snapshots — engine `00FE`, `00FF` (STRUCTURAL)
`00FF` = `003F 531B 37DC 1422`; the last two fields are exactly `121F` (`37DC`) and `121E`
(`1422`). `00FE` = `0001 39EE 35DD 140D` — same shape, different (older? other-trip?) values.
So `00FE`/`00FF` bundle four scalars that also exist individually.

### 5.3 Packed adaptation record — gearbox `1119` (STRUCTURAL, fully resolved)
32 bytes = **8 × {u16 value, u16 index}**, and every value byte-pair is also readable as its
own identifier:

| slot | value | index | individual DID |
|---|---|---|---|
| 0 | `FC81` | 0000 | `10B6` |
| 1 | `726F` | 0200 | `108B` |
| 2 | `92F8` | 0400 | `1099` |
| 3 | `DCFF` | 0500 | `108C` |
| 4 | `B1A6` | 0500 | `109A` |
| 5 | `6374` | 0400 | `10A7` |
| 6 | `6F6B` | 0300 | `10B5` |
| 7 | `BE81` | 0100 | `10A8` |

This is the key that unlocks `1088–10B6`: that block is a **repeating fixed-stride record**
(offset, value, counter, clutch position, …) instantiated ~5 times. The tell is
`6A00` (=106 LE) appearing at `107F`, `108D`, `109B`, `10A9`, `10B3` — one per record instance —
**and also at `38AC`, which is a PROVEN clutch position ×0.01 mm, i.e. 1.06 mm.**
Likewise `3D0A` appears at both `1076` and `38AB`, and `410A` at both `1061` and `38FF`.
So the `10A0`-block and the `38A0`-block are two views of the same clutch data.

### 5.4 Other long responses

**Engine**
| DID | len | content |
|---|---|---|
| `011B`, `02A1` | 64 B | all zero |
| `02A0` | 64 B | `22040000000003 01` then zeros — sparse record |
| `0407`, `0408` | 10 B | `0001` ×5 — a 5-element array |
| `0481` | 89 B | sparse record, mostly zeros with scattered small fields |
| `0600` | 10 B | `0C2500122324040B0000` |
| `11DD` | 12 B | ASCII `LSL_OC_VG_1` |
| `11DE`–`11E6` | 9×9 B | ASCII `NO_ERROR` ×9 — nine self-test channels, all passing |
| `16DF` | 17 B | ASCII `SC8O4010 CBL20E0` |
| `4E48`–`4E4B` | 4×16 B | 8×u16 records; `4E48` = `0019 000D 006B 0014 0000…`, three of four empty |
| `F1F0` | 20 B | binary + `AUDISC8` stored **reversed** (`…8CSIDUA`) |
| `F1F4` | 29 B | ASCII `SC8.1 CB.00.00.E0 C02.00 SC8` |
| `F808` | 41 B | OBD IPT: 10 × u32 counters |

**Gearbox**
| DID | len | content |
|---|---|---|
| `02EE` | 10 B | `00F8` then zeros |
| `02FF` | 18 B | `5311118A4B00800AFF5C…` — unexplained |
| `1022` | 14 B | ASCII `FEJRMRBEB.039` |
| `103E` | 16 B | high-entropy 16 B — looks like a hash/key, **not** a measurement |
| `103F` | 21 B | ASCII `0CW300041G_1003_ODUV` — software/dataset name |
| `10FE` | 22 B | ASCII `025K A01703023043VD 2` — mechatronic part number |
| `1108`, `1109` | 12 B | ASCII `000000142130`, `000000125261` — serials |
| `1119` | 32 B | see §5.3 |
| `111A`, `111B` | 16 B | 8×u16 records |
| `1178` | 22 B | ASCII `5Q1713025R     0  V L4` — **selector lever part number** |
| `11A5` | 28 B | high-entropy head, zero tail |
| `11B0`–`11B4` | 5×44 B | one filled (`U10` + 4-byte entries + `XCCF`), four identical empty slots — a 5-slot adaptation/history log |
| `1700` | 24 B | all `FF` |
| `3850` | 21 B | ASCII `006SMWR020931H170303` |
| `3BAE` | 8 B | unexplained |

---

## 6. RANKED shortlist for the next VCDS-paired capture

Ranking = (observed to vary) × (not yet explained) × (fields unlocked per identifier).
Log these in VCDS alongside `vagcan watch --out` and fit with `vagcan analyse`.

| # | ECU | identifier(s) | why |
|---|---|---|---|
| 1 | gearbox | **`3820`–`38FF` block, 2-byte LE** (esp. `3826`/`3827`, `382A`/`382B`, `3834`/`3835`, `3851`–`3870`) | ~101 two-byte identifiers, adjacent to all six proven gearbox rows, and **never once recorded while driving** — the largest unexplored block with the best prior in the whole dataset |
| 2 | gearbox | **`102D`** | textbook accumulator sawtooth, 103 distinct values in 108 samples; the single most obviously-live unexplained scalar. Log VCDS "hydraulic pressure" against it |
| 3 | gearbox | **`100B` / `100C` / `100D`** | signed + two unsigned companions that go to zero and negative — a torque triple; unit is completely unknown and only a log will settle it |
| 4 | gearbox | **`102A` / `102B` / `102C` / `1017`** | four monotone risers = four candidate temperatures; the offset (−40 or none) cannot be resolved from the data and is exactly what a log resolves. Do this on a **cold start** so the curve has range |
| 5 | gearbox | **`0285` / `0286` / `1000` / `1001`** | supply voltage ×0.1 V is nearly certain; cheapest possible proof, and it calibrates the whole "u16 LE ×0.1" pattern for the family |
| 6 | gearbox | **`1009` / `100A`** | a tight pair with a constant ratio ≈1.174 across the whole drive — a unit conversion or a per-clutch pair; both meanings are useful and neither is guessed |
| 7 | gearbox | **`3816`** (+ `3808`/`3818`, `3809`/`3815`) | gear and selector are ratio-derived, not proven; one VCDS log confirms the code table cheaply and gives the catalogue two headline rows |
| 8 | gearbox | **`100E` / `100F` / `1010` / `1029`** | all move with load; `1029` steps between 700/800/930/1000/1030 which is idle-speed-request-shaped. Unexplained |
| 9 | engine | **`13xx` block, 2-byte BE** (95 identifiers, 74 % zero with the engine off) | the largest engine family and the one most likely to be "silent because the engine was off". Needs a **running-engine sweep first**, then a drive |
| 10 | engine | **`20xx` + `29xx` together** | the proven boost pair lives in `20xx`, and `29xx` mirrors nine `20xx` values — logging both simultaneously should expose the specified/actual pairing across ~140 identifiers |
| 11 | engine | **`39C0`, `39C2`, `3E70`** | read exactly `03DF`, the proven ambient-pressure value; three more pressure channels for one log |
| 12 | engine | **`1003`, `11F7`, `11CD`, `11D0`, `11D4`, `132F`, `14A6`, `16B0`** | temperature-shaped bytes anchored to the known coolant/IAT raws; a warm-up log separates them in one run |
| 13 | gearbox | **`102F` / `1030` / `1031`** | latched shift-event triple; deliberate manual shifts against a VCDS log would decode the event codes |
| 14 | engine | **`0153` / `4F39`** | the timestamped log's *event* is unknown; clear a DTC or run an adaptation with VCDS open and see whether a slot fills |
| 15 | gearbox | **`1088`–`10B6` + `1119`** | the record layout is already resolved (§5.3) and one slot is a proven clutch position; a targeted read would confirm the stride and yield ~5 adaptation records at once |

### Explicitly deprioritised
- Gearbox `03xx` (all 60) — zero everywhere, in every capture. Skip.
- Engine `17xx`, `46xx`, `09xx` — 100 % zero, but **only** because the engine was off. Re-sweep with the engine running *before* deciding they are dead.
- Gearbox `103E`, `11A5` — high-entropy binary, almost certainly keys/hashes. Not measurements.
- All ASCII identifiers — already readable, nothing to fit.

---

## 7. Honest list of what is NOT explained

- **The entire engine dynamic behaviour.** No engine driving recording exists. Every engine
  statement above is static-only.
- **Every scaling factor proposed in §3 and §4.** Shape and range arguments are not proofs.
  The proof procedure is `vagcan analyse` against a VCDS log and it was not available here.
- **The temperature offset question** (raw vs raw−40) for gearbox `102A`/`102B`/`102C`/`1017`.
  Both readings are physically plausible; the data cannot choose.
- **The unit of the gearbox torque cluster** `100B`/`100C`/`100D`. ×0.1 Nm overshoots the
  engine's rating, so the obvious factor is probably wrong.
- **What `0153`/`4F39` log.** Timestamps decoded, event unknown.
- **Gearbox `02FF`, `3BAE`, `11A5`, engine `0481`, `02A0`, `0600`, `F1F0` payloads.** Records
  with no recoverable field structure from a single sample.
- **Whether `03xx` on the gearbox is fault flags or something else.** All-zero is consistent
  with "no faults" and with "not implemented"; nothing here distinguishes them.
- **Engine `121F`/`2998` = 14300.** Counter-shaped, meaning unknown.
- **Roughly 380 gearbox and 440 engine identifiers** that are non-zero, short, and have simply
  never been observed changing. They are not "explained" — they are unobserved.
