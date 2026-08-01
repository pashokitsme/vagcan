# The whole car, not just the powertrain

What the reference car (Škoda Octavia III 1.8 TFSI, DQ200, MQB) answers when every
control unit is asked rather than the two that ISO addressing reaches. Everything here
was read live on 2026-08-01/02 with `vagcan survey` and `vagcan faults`; nothing is
inferred from a table.

Sources: `research/dumps/survey-parked.jsonl` (one JSON object per unit, parked, ignition
on), the car's own VCDS auto-scans under `research/VCDS-RUS/Scans/` as an independent
oracle, and `research/other-ecus.md` for the addressing rules this builds on.

---

## 1. The inventory

15 of 18 addresses answered. Every unit that had been "unidentified" in
`research/other-ecus.md` §1 named itself the moment it was asked for `F187`/`F197` —
no reverse engineering was needed, only addressing it correctly.

| request | part number | component (`F197`) | identifiers | confirmed faults |
|---|---|---|---|---|
| `7E0` | `8V0906264H` | `1.8l R4 TFSI` | 163 | 0 |
| `7E1` | `0CW300041G` | `GSG DQ200G2_M` | 186 | 0 |
| `70A` | `5QA919283A` | `PDC 4 Kanal` | 39 | 5 |
| `70C` | `5Q0953521KM` | `Lenks.Modul` | 39 | 1 |
| `70E` | `5Q0937084CF` | `BCM MQBAB M+` | 126 | 3–4 |
| `710` | `3Q0907530B` | `GW MQB Mid` | 131 | 1 |
| `712` | `5Q0909144T` | `EPS_MQB_ZFLS` | 52 | 2–3 |
| `713` | `5Q0614517AQ` | `ESC` | 48 | 3 |
| `714` | `5E0920740D` | `KOMBI` | 117 | 0 |
| `715` | `3Q0959655BE` | `Airbag VW21` | 47 | 0 |
| `746` | `5E0907044AM` | `Climatronic` | 56 | 0 |
| `74A` | `5Q4959393E` | `TSG FS` (driver's door) | 53 | 0 |
| `74B` | `5Q4959392E` | `TSG BFS` (passenger door) | 50 | 0 |
| `767` | `3Q0035284` | `OCULowMQBLGE` (telematics) | 50 | 0 |
| `773` | `5E0035871C` | `MU-E--ER` (media) | 49 | 0 |

`0x700`, `0x776` and `0x777` did not answer at all — consistent with
`research/other-ecus.md` §3's warning that the last two are response ids appearing in the
installation list rather than addressable units.

1206 identifiers in total, up from the ~350 the two powertrain units account for. The
instrument cluster alone went from the 5 identifiers VCDS had been seen to ask for to
117 that answer.

Sweep cost: 485 s for the whole car, over the nine identifier pages
`0200-02FF,0600-06FF,1900-19FF,2000-22FF,2A00-2BFF,3800-38FF,F100-F1FF,F400-F4FF`. Units
that answer nothing are dropped after their identification block instead of costing 2816
timeouts each.

---

## 2. Fault codes

### 2.1 A code is one number

The three bytes of a UDS DTC are **one fault number**, big-endian, and VW's tools print
it in decimal. An earlier reading here split them into a 16-bit number and a symptom
byte; that is wrong.

Refuted by the car's own scan (`Scan-XW8AD4NE9JH008917-20260731-1522-212722km.txt`),
which prints, under the matching unit address and part number:

| read from the bus | 24-bit reading | 16-bit reading | VCDS printed |
|---|---|---|---|
| `00 01 29` on `0x713` | 297 | 1 | `0297 - Датчик угла поворота рулевого колеса` |
| `04 71 20` on `0x70C` | 291104 | 1137 | `291104 - Датчик температуры подогрева рулевого колеса` |
| `00 02 13` on `0x70E` | 531 | 2 | `0531 - Освещение пространства для ног` |
| `01 04 05` on `0x710` | 66565 | 260 | `66565 - Шина данных Диагностика` |

Four for four on the 24-bit reading, zero for four on the other.

### 2.2 Most listed codes are not faults

Asking with status mask `0xFF` returns everything a unit knows about. The body control
module answers 508 codes; 505 carry status `0x10` — testNotCompletedSinceClear, "this
test has not run since the memory was cleared". Only bit 3 (`0x08`, confirmedDTC) means
the unit stored a failure. Counting the rest would report a car with hundreds of faults
that has seventeen.

### 2.3 When a fault happened

Extended-data record `0x01`, returned by `0x19 06`, has the same layout on every unit
that answered:

```
06 09  02B8  033F1B  0000  69F9044B
^  ^   ^     ^       ^     ^
|  |   |     |       |     free-running seconds counter
|  |   |     |       two bytes, zero in every sample seen
|  |   |     odometer, km, u24 big-endian
|  |   reset counter — rises with driving cycles
|  occurrences, saturating at 0xFF
priority
```

**Cross-checked against VCDS, independently.** For the brake unit's fault 297 the scan
prints `Приоритет неисправности: 2`, `Кол-во проявлений: 8`, `Сброс счетчика: 140`,
`Пробег: 212722 km`. The bytes read off the bus give priority `0x02`, occurrences `0x08`,
mileage `0x033EF2` = 212 722. Three fields match exactly. The reset counter reads 693 now
against the scan's 140, which is what a counter of driving cycles should do over two days
of use — a field that matched exactly would have been the surprise.

Further evidence for the mileage field alone: across 17 stored faults on six units, no
value exceeds the instrument cluster's odometer, the newest equal it exactly, and mileage
and counter order the same way.

**The clock is a day counter and a second of the day, not a plain seconds counter.**
The car's own scan dates the brake unit's fault 297 at `2026.07.27 00:00:03`, and the
unit stores `0x69F60003` for it: the low half is exactly 3. A 32-bit seconds counter
would leave an arbitrary value there, so landing on the scan's own seconds is a
1-in-65 536 coincidence. Two records on the same day differ by their seconds; a day
advances the high half by one.

This corrects an earlier reading here. Subtracting the raw 32-bit values loses 20 864
seconds per day crossed, which made a fault stored the previous day look 18 hours old
instead of 24.

**The epoch is still not established.** `0x69F6` is some day in late July 2026 by the
scan, and nothing says what numbering that is; under the day reading the car's clock also
appears to run about two days behind real time, which is plausible for a car clock but
not verified. So ages are reported as **differences**, never as dates. Identifier
`0x02BD` returns the same stamp live — `91 <mileage:3> <2 bytes> <clock:4>`, the tail of a
fault record without the fault — and "42 km ago, 26 h ago" is true without knowing what
year the car thinks it is.

### 2.4 The unit's own code list is not available

`0x19 0A` (reportSupportedDTC) is refused by **every** unit on this car: NRC `0x12`
(subfunction not supported) on `0x70A`, `0x70E`, `0x713`; NRC `0x13` (incorrect length)
on `0x7E0`, `0x7E1`, `0x712`, `0x714`. Recorded because a supported-code list in the
unit's own order would have been a candidate join against the label corpus's per-ECU
fault sections. That route is closed.

---

## 3. Fault texts: found, but not joinable yet

`research/VCDS-25.12.0/Codes.dat` (2.1 MB, English) and `research/VCDS-RUS/Code-RUS.dat`
(1.9 MB, Russian) hold the fault texts, in the same TEA-CBC as the `.rod` files:

```
record := <8 ASCII digits: id> ' ' <u8 cipher_len> <u8 text_len> <cipher> "\r\n"
```

34 716 records in English, 27 587 in Russian. The block-0 IV is per-record and unsolved,
so the first 8 characters of each text are lost; the rest is exact.

**The key is not the VW fault number.** Ids below 65536 are legacy KWP codes and the
higher band is `SAE_code << 8 | failure_type`. Looking up 297 returns "…Speed Sensor
(G38)" where the car means the steering angle sensor — a plausible-looking wrong answer,
which is the worst kind. Naming faults from this file directly would be wrong on most
codes.

What VCDS evidently has and this project does not is the map from a VW fault number to
its SAE code (it prints both). That map is not in `Codes.dat`, not in the `.lbl`/`.clb`
label files, and not in the per-ECU `.rod` `[DTC]` sections — those hold
`<TTTEXT text-id>,<2-character code>` rows, i.e. *which* faults a unit can name, with no
number attached that anyone here can read yet.

### The registry that is keyed by the fault number — and what still blocks it

`UDS_EV/RD.rod` is a **global fault registry keyed by the number itself**, not per ECU.
Its `[DTC]` section needs `IV[3..8] = 5c b0 48 d4 3f`, inflates to 6.3 MB, and holds
228 394 rows under 66 903 six-digit plaintext ids in three ascending blocks.

The check that could have failed:

| key tried | hits |
|---|---|
| the 946 captured fault numbers below 10⁶, in block 0 | **946 / 946** |
| the same numbers ±1, ±2, +256, +1000 | 473–781 |
| 26 numbers from the scans, 9 of them in sparse regions | 26 / 26, joint p ≈ 6·10⁻¹⁰ |
| ⌊n/100⌋ for numbers ≥ 10⁶, in block 1 | **125 / 127** (9.8 expected by chance) |
| n//64, n//99, n//101, n//120, n//128 | 4–24 |

Row grammar is the familiar `<id><sep><2-char code><sep>…`, the same shape as an ECU
`[DTC]` row. **But the payload numbers are under the per-table digit substitution that
`research/label-linkage.md` §2.4 records as unbroken**, so the last hop — registry row to
text-id to name — does not resolve. Fitting without the code constraint gives thousands
of candidates per table; with it, zero for three of four cribs.

Refuted along the way, so nobody retries: the per-ECU `[DTC]` section is **not** the
source — 11 of the 23 units in the crib have no `[DTC]` section at all, including the
brake unit that reports fault 297, and only ~5 % of the rows that do exist resolve
against `names-uds.json`. The two-character code cannot carry a fault number either:
40² = 1600 and the crib includes 5386 and 6922. `Codes.dat` lacks 5386, 6922, 291104,
14751, 15187 and 25548 entirely. `TTText-RUS.rod`, `TTText2-RUS.rod` and `TTTEXT2.ROD`
do not decrypt under the known first-block rule — a different unknown from the old one,
and the Russian text table would otherwise have been free, Cyrillic being outside all
three cipher classes.

---

## 4. What to record next

* **Drive with the survey running.** One parked pass exists; the identifiers whose bytes
  differ between it and a moving pass are the live measurements, and that list needs no
  label file. This is the cheapest large win available.
* **The brakes (`0x713`) and the body control module (`0x70E`)** are the two units with
  the most obviously nameable signals — pedal, wheel speeds, lights, doors — and both now
  answer ~50 and ~126 identifiers respectively.
* **Cold start on the cluster.** `22D0` has read `0xB8` = 90 °C in every sample ever
  taken, so `×0.75 − 48` and `×0.5 − 2` are still indistinguishable. One warm-up settles
  it.
