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
|  |   |     |       |     car clock: packed date and time (see below)
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

**The clock is a packed calendar date and time.** Most significant bit first: 6 bits
year from 2000, 4 month, 5 day, 5 hour, 6 minute, 6 second. Two VCDS printouts of this
car are reproduced field for field — `0x69F60003` → `2026.07.27 00:00:03` on the brake
unit and `0x69F97C82` → `2026.07.28 23:50:02` on the steering assist.

This is the **second** correction of this field, and both earlier readings are dead:

* A plain 32-bit seconds counter. Refuted by the brake anchor, whose low half is exactly
  the scan's own `03` seconds — a 1-in-65 536 coincidence otherwise.
* `day counter << 16 | second of the day` (what this section used to say). It fits the
  brake anchor only because that fault is three seconds past midnight, where the two
  layouts agree. It reads the steering anchor as `08:51:14` where VCDS prints `23:50:02`.

### `0x02BD` is the same field, and it is not a counter

Identifier `0x02BD` returns the tail of a fault record without the fault —
`9x <mileage:3> <2 bytes> <clock:4>` — on nine of the fifteen units. Raw differences of
that `clock` looked like a free-running 1 Hz counter, which is what put the two readings
in conflict. They are not in conflict; **subtracting raw packed values is the mistake.**
The seconds field is six bits but wraps at 60, so a raw difference overshoots real
elapsed time by 4 per minute boundary crossed, 256 per hour and 32 768 per day.

Four checks, each of which could have failed:

| check | result |
|---|---|
| The instrument cluster keeps its own real-time clock (`2238`/`2239`/`223A`/`223B`/`223C`) and is read part-way through each sweep. | In all three sweeps it falls inside the bracket its neighbours' `02BD` stamps set — `23:51` between `23:50:17` and `23:52:41`; `03:18` between `03:17:19` and `03:19:44`; `03:26` between `03:25:36` and `03:28:01` — and its year/month/day match the unpacked ones exactly. |
| Units are read one after another, so unpacked times must rise in file order. | They do, in every sweep, for every unit. |
| Between the two driving sweeps, raw differences were 528–533 while real elapsed time was 496–497 s. | The packed layout predicts each unit's own value exactly (elapsed + 4 × minute boundaries): 529, 529, 533, 529, 533, 529, 528. A 1 Hz counter predicts 496–497 for all seven. |
| Two single-unit reads 94.5 s apart by the host clock. | 94 s apart unpacked; 98 raw. |

The old "1 Hz" evidence dissolves under the same arithmetic. The pair that read "33 756
counts, 9.4 hours later" is the body control module at `2026-07-28 23:50:00` and the same
unit at `2026-07-29 00:01:28` — **11 minutes 28 seconds** apart. The 9.4 came from
dividing the raw difference by 3600, i.e. from the assumption it was testing.

**The epoch was never the question — the car's date is.** This car's clock runs four days
behind real time while keeping the correct time of day: the three sweep files were closed
4 d + 3.0 s, 4 d + 3.6 s and 4 d + 4.3 s after the last stamp they contain, the residual
being the time to finish and write the file. So a stamp is an exact moment on the car's
clock, and four days must be added to reach a real one — for this car, which is not a
fact any other car inherits.

**One unit's record is not decoded.** The two door units (`0x74A`, `0x74B`) answer
`0x02BD` with **eleven** bytes, not ten, and the packed clock inside is offset by seven
bits. Read byte-aligned it gives `2013-03-30` and an odometer of 9 516 020 km on a car
that has done 212 805. At the one bit offset that yields a valid date at all — one of 57
— all four of their records land in exactly the right slot of the sweep order
(`23:53:14` and `23:53:46` between `746` at `23:52:41` and `767` at `23:54:08`, and the
same in the driving sweep). So the clock is there and it is this clock; the surrounding
record is not understood, and `UnitStamp::parse` now refuses any length but ten rather
than reading a wrong date out of the front.

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

34 716 records in English, 27 587 in Russian. The block-0 IV is per-record; it was
unsolved when this was written, and it is derived from the record's own key in
[`codes-dat.md`](codes-dat.md) §2.2. Both language files now decrypt in full — the first
8 characters are not lost.

**The key is not the VW fault number.** Ids below 65536 are legacy KWP codes and the
higher band is `SAE_code << 8 | failure_type`. Looking up 297 returns "…Speed Sensor
(G38)" where the car means the steering angle sensor — a plausible-looking wrong answer,
which is the worst kind. Naming faults from this file directly would be wrong on most
codes.

That is stronger than the individual absences listed below. It is not that this car's
particular numbers happen to be missing: **no** VW fault number is a key at all, because
every key is a 24-bit ISO DTC ([`codes-dat.md`](codes-dat.md) §4). One hop —
VW fault number → ISO DTC — is the whole of what remains; everything past it is a
dictionary lookup.

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

Row grammar is `<fault number><sep><A><sep><2-char code><sep><B><sep>…`, 210 734 of
228 392 rows. The ids run **monotonically ascending throughout** — zero descents — six
digits below 10⁶ and eight above, under 105 186 distinct keys.

### The digit substitution is broken — by the row order, not by cribs

Rows inside a table are stored in ascending order of their **plaintext**. The
substitution therefore leaks that order: for two consecutive rows, the first position
where they differ says which of the two glyphs is the smaller digit. Collecting those
constraints and topologically sorting them *is* the alphabet. No known plaintext is
involved anywhere, which is why every crib-based attempt had failed.

| check | result | what would have refuted it |
|---|---|---|
| constraint graph acyclic, tables with ≥4 rows | **10 916 / 10 916** | the same rows shuffled: 7 258 cycles |
| 14 958 values decoded from 680 independently-keyed tables | **2 080 distinct** | random per-table maps: 10 392 distinct |
| one value reached identically by | 42 different keys | — |
| 2 143 decoded 7/8-glyph values read as 24-bit fault codes, low byte | **66.5 % exactly `0xF0`**, 95 % in `0xF0..0xF7` | random maps: 0.2 % |
| system letter of those codes | B (1 526) or C (617), never P or U | — |

Worked example, the fault-531 table, alphabet `0 . - 8 3 2 1 5 7 4`:
`.0374730` → 10 489 840 = `0xA00FF0`, `4527503` → 9 758 704 = `0x94E7F0`, `.-0238` →
120 543. The decoder is `crates/vag-data/src/glyphs.rs`.

Coverage from ordering alone is 680 of 105 186 tables, because a table needs roughly
five rows before its order pins all ten glyphs and the registry averages 2.2. Tables for
faults 531, 297 and 527 are solved outright.

### …and it still does not give a fault name

The registry rows are **cross-references, not self-identification**:

* the two-character code is a function of field `A`, not of field `B` (97.3 % against
  66 %), which is why the previous pass's assumption scored zero;
* for faults 531 and 297, 0 of 20 and 1 of 21 of their decoded `A` values appear in
  their own ECU's `[DTC]` fault-name list — a fault's own name would have to be there;
* of 2 080 distinct decoded `A` values, 5.8 % are in `names-uds.json`, *below* the 8.8 %
  baseline, so `A` is not preferentially a name id;
* only 1 966 of 64 205 large-key tables contain their own code with the `0xF0` byte, so
  a row is not the fault describing itself;
* field `B` (190 000–450 000) is disjoint from TTTEXT's dense region — a second id space,
  most likely `TTTEXT2.ROD`, which still does not decrypt.

So the wall has moved rather than fallen: the digit substitution is no longer the
blocker. What blocks a fault name now is that nothing yet ties a fault *number* to a
position in a per-ECU `[DTC]` list, and that most of those lists' text-ids point outside
the part of `TTTEXT.ROD` that has been recovered.

**Worth doing next:** the same ordering attack on `STRUC.rod`, which is the blocker
`research/label-linkage.md` §2.4 named for *measurement* scaling rather than fault names.
A first pass gives 384 of 585 tables acyclic field-wise against 217 on shuffled controls
— real signal, and its rows are probably sorted on a subset of their eleven fields.

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

## 3b. A control unit was destroyed by sweeping it — read this before running one

**Summary: the identifier sweep in `vagcan survey` disabled the steering assist on the
reference car, twice, the second time permanently.** The unit still answers every
diagnostic request and its identification block is intact, but it stores
`B2000` (control unit defective — internal memory checksum), `B200F` (internal fault)
and `B1168` (steering angle: no initialisation), the warning lamp is on, and there is no
assist. The commissioning dataset at identifier `1923` read `01 00 28 61 D7 E9` before
and reads `01 00 00 00 00 00` after, and did not refill over a subsequent drive.

Sequence, from the owner:

1. First `survey` run — assist dropped out during the sweep. **Switching the engine off
   and on restored it.**
2. A kilometre or two later, second `survey` run — assist dropped out again, mid-drive,
   and a restart no longer helped. It has not returned since.

That the first event was recoverable is the important part: the hardware was working, and
what a sweep does is crash the unit's diagnostic server. A full sweep is 2816 requests
for identifiers the unit may never have been asked for in its life — a fuzz of a UDS
server, and a server with a defect in one of those paths falls over. VW's own bulletin
**TPI 2055045/4** (address 44, May 2024) documents `B200FF0` appearing *"during
maintenance or repair work"* with the technical reason *"wrong software for the power
steering control unit"*, which is the same failure from the factory's side of the desk.

What changed in the tool as a result:

* the extended diagnostic session (`0x10 0x03`) is no longer sent at all by default — it
  is workshop mode, and a unit that assists the driver may stop while it is in one;
* `survey` **refuses to run on a moving car** unless `--while-driving` is passed, and
  says why;
* `--extended` is refused whenever the car is moving, established by reading road speed
  first, with a car that will not report its speed counted as moving.

None of that makes a sweep safe. It makes it survivable to stop and restart, which is
what the first event turned out to be. A sweep is still the most invasive thing this
read-only tool does, and on a unit with a firmware defect it is enough.

## 3c. The same event, as it looked before the owner supplied the sequence

During a driving survey on 2026-08-02 the **steering assist stopped assisting**, about a
third of the way through the walk. `0x712` is the seventh of eighteen addresses in the
walk order, which is that third.

The cause is almost certainly ours. Before reading each unit the survey sent
`0x10 0x03` — extended diagnostic session. On this platform that is workshop mode, and a
unit responsible for assisting the driver is entitled to stop assisting while it is in
one. Nothing else in the command writes: the only other service issued is `0x22`.

What changed as a result: the session request is **off by default** in both `survey` and
`faults`, and where it is asked for explicitly (`--extended`) it is **refused unless the
car is standing still**, checked by reading road speed from the engine first. A car that
does not report its speed counts as moving.

Open check for the next session with the car: the driving surveys were recorded with the
session request still in place, so it is not yet known which units need it. If a unit
answers nothing without one, that is worth recording per unit rather than restoring the
blanket request.

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
