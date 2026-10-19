# Alarm thresholds: engine oil, DQ200 control module, DQ200 clutch linings

Car: Škoda Octavia III (5E), 1.8 TSI EA888 gen3, Simos 18 (`8V0906264H`, ODIS `EV_ECM18TFS0208V0906264H`);
gearbox DQ200 gen 2 (`0CW 300 041 G`, ODIS `EV_TCMDQ200021`, "GSG DQ200G2_M"). Research 2026-10-19.

Strength scale used: **OEM** (VW/Škoda document) · **OEM-derived** (label/ODIS data) · **car** (this car's own
reads, `research/dumps/`) · **many logs** · **anecdote** · **inference**.

---

## 1. Engine oil temperature — `01:IDE00196`, DID `202F`, 0.1 °C

### Where the unit or VW draws a line

| value | what it is | source | strength |
|---|---|---|---|
| 80–110 °C | "engine operating temperature is reached" | Škoda Octavia owner's manual, English, edition 11.2015, p. 42 (MAXI DOT oil-temperature display). Copy: <https://www.sneeuwkettingen4u.nl/sitezilla/uploads/2016/10/A7_Octavia_OwnersManual.pdf> | **OEM** |
| > 110 °C (and < 80 °C) | the manual says to "avoid high engine revs, full throttle and high engine loads". This is advice to the driver. It is not a point where the engine protects itself | same page | **OEM** |
| < 50 °C | the display shows dashes instead of a value | same page | **OEM** |
| 107 °C / 85 °C coolant | thermal-management coolant target at part load and at full load. The oil cooler is a coolant heat exchanger, so oil runs a few degrees above coolant | Audi SSP 920243 "1.8L and 2.0L Third Generation EA888", p. 45. <https://static.nhtsa.gov/odi/tsbs/2014/MC-10122162-9999.pdf> | **OEM** (context only) |
| 113 °C coolant | fail-safe wax thermostat opens | same SSP | **OEM** (coolant, not oil) |
| engine's own oil-temperature protection (torque limit, warning) | **not found.** The Simos18 variant's ODIS fault list has only sensor-circuit codes for oil temperature (`P0196`, `P0198`) and no over-temperature code. `P102B00` "Torque limitation due to engine oil temperature" exists only in EA288 TDI variants (`EV_ECM20TDI…05L906022…`). The owner's manual lists no oil-temperature warning; its "Engine overheat. Stop!" is the coolant warning. No Simos18 map could be found without an A2L/XDF | ODIS cache `~/.vagcan/data/SK37X/cache.sqlite`; owner's manual | **OEM-derived** (absence) |
| 137–140 °C "cluster oil-temperature warning" / "warning at 135 °C" | claimed in forum comments. The poster of the 135 °C figure says he only heard it. **Not confirmed**, and it contradicts the manual, which lists no such warning | drive2 <https://www.drive2.ru/l/655850715582840345/> (comments) | **anecdote, unverified** |

VCDS legacy labels: across 1,854 decoded `.clb` files, no petrol-engine label gives an oil-temperature specification.
Some TDI and V6 labels show `Range: 30.0...120.0 °C` for oil temperature, which is a display range and not a limit. EA888 gen1/2
(`06J-906-026-CCT`) has the oil-temperature block 134 with no specification.

### Normal range people actually see (1.8/2.0 TSI gen3, stock)

| observed | conditions | source | strength |
|---|---|---|---|
| 104–112 °C | highway at 100 km/h with 20–30 °C outside; town 106–108 | octavia-club A7 thread p7 <https://www.octavia-club.ru/f/A7/28899-temperatura-masla-na-1-8-tsi/p7> | many logs |
| 105–108, 111 in traffic; 105 ± 2 average; 95–96 in traffic; "always 112 in summer" | several owners, 1.8 TSI | same thread p10, p12 (`…/p10`, `…/p12`) | many logs |
| 109–114 °C average for 5 years; 115–120 °C with dirty radiators, back to 105 after washing them; 116 "sometimes" | 1.8 TSI A7 owners | drive2 <https://www.drive2.ru/l/655850715582840345/> | many logs |
| 104–107, up to 111–113 in summer traffic; 117–118 on Stage 3 at 200+ km/h in heat | 1.8 TSI A7 | drive2 <https://www.drive2.ru/l/588645266358285024/> | many logs |
| 103–110 °C town and motorway; 115 before an oil change | Octavia vRS TSI (EA888) | BRISKODA <https://www.briskoda.net/forums/topic/415751-normal-oil-temp/> | many logs |
| 126 °C after 3 laps, over 133 °C by lap 5, coolant 119 °C, "timing pull and temp warnings" (the poster blames the water). With a separate oil cooler the peak was 118 °C | 2017 Golf R on track, stock cooling | 529 Innovations product page <https://www.529innovations.com/drivetrain-engine/p/e888gen3-e888gen4-oilcooler> | anecdote (vendor) |
| 81.0 °C | this car, parked scan 2026-08-01 | `research/dumps/engine-full.log` (`202F 0D D5`) | car |

**Summary:** a stock road car sits at 100–112 °C, reaches 115–120 °C in heat or with clogged radiators, and reaches
125–133 °C only on a track. The one VW line is 110 °C, and it is advice to the driver. No engine protection
threshold was found.

---

## 2. DQ200 control module (mechatronic) temperature — `02:IDE00102`, DID `028D`, integer °C

(The engine unit also has a `028D` with a different scaling, 0.1 °C big-endian. That is not this channel.)

### Where the unit protects itself

| value | what it is | source | strength |
|---|---|---|---|
| **139 °C** | sender G510, inside the mechatronic electronics: "At a temperature of 139 °C, engine torque reduction can be implemented." The test question at the end gives "approx. 140 °C" for the same measure | VW SSP 390 "7-speed DSG 0AM" (DQ200 **gen 1**). Copies: <https://idoc.pub/documents/ssp390-dsg-7-gearbox-0ampdf-6klzgxkg6qng>, <https://pdfcoffee.com/ssp390-dsg-7-gearbox-0ampdf-pdf-free.html> | **OEM** (for 0AM) |
| 0CW (gen 2) figure | **not found.** No gen-2 SSP was located. 0CW keeps the G510 design, so applying 139 °C to it is an **inference** | — | inference |
| torque cut, then clutches opened, at 138 → 145 °C; clutch warning jerk at 160, clutches open at 170 | **These are 02E / DQ250 (wet clutch) numbers**, from senders G93 and G509, in Škoda SSP 94 §4 p. 24–25. A search summary attributed them to 0AM, which is wrong. **Do not use them for DQ200.** | Škoda SSP 94 "Diagnosis of the automatic gearboxes 0AM and 02E" <https://www.team-bhp.com/forum/attachments/technical-stuff/2220949d1634616536-vw-dsg-how-check-dq200s-clutch-wear-ssp-094-diagnosis-automatic-gearboxes-0am-02e.pdf> | OEM, wrong gearbox |
| cluster warning | Octavia manual: "Gearbox overheated. Stop!" with the instruction to stop and switch off. No temperature is given | owner's manual 11.2015 | **OEM** (no number) |
| fault codes present in the variant | `P021800` Transmission Fluid Over Temperature, `P278500` Clutch Actuator Temperature Too High, `P278700` Clutch Temperature Too High, `P189600/P189700` Clutch 1/2 Temperature shock | ODIS cache, `EV_TCMDQ200021_001` | **OEM-derived** (no thresholds) |

### Normal range

| observed | source | strength |
|---|---|---|
| 68 °C parked; 71 °C flat through a 142 s drive | `research/dumps/gearbox-full.log`, `drive-gearbox.csv` (u16 little-endian, ×1) | car |
| **101 °C lifetime maximum** in `1021` "Temperature_hybrid_maximum". Histogram `101B`–`1020`: 2537 / 1307 / 1137 / 818 / 118 h, 5.4 s, 0, 0. "Hybrid" is taken to mean the hybrid-circuit control electronics, the same quantity as `028D`. That reading is an **inference**: the bin edges are unknown and the name is not documented | `research/dumps/gearbox-full.log` | car + inference |
| 30–100 °C is the condition for adaptation; 30–60 °C (G510, block 011.1) for basic settings | drive2 <https://www.drive2.ru/l/472157506953543828/>; Ross-Tech wiki <https://wiki.ross-tech.com/wiki/index.php/7-Speed_Direct_Shift_Gearbox_(DSG/0AM)> | anecdote / Ross-Tech |

Owner logs of `028D` in heat or on track: **not found.**

---

## 3. DQ200 clutch lining temperatures — `02:102B`, `02:102C` (linings K1/K2), `02:102A` (central plate)

These values come from the TCU's thermal model, not from a sensor. 0AM up to MY2013 had no G509, and SSP 94 says the
clutch temperature is "recalculated".

### Where the unit warns or protects

| value | what it is | source | strength |
|---|---|---|---|
| staged thresholds behind `387F`–`3886` (Warning 1/2, Protection 1/2 per clutch) | the numbers are **not found.** The ODIS names give counts and no thresholds | ODIS cache | OEM-derived (names only) |
| **> 308 °C without a single warning** | this car: lifetime maximum K2 `1079` = 308 °C, K1 `1064` = 233 °C, and all eight `387F`–`3886` counters = 0. So on this software, Warning 1 sits above 308 °C, **or** the counters were reset after the 308 °C event (inference) | `research/dumps/gearbox-full.log` | car |
| 350 °C "first overheating warning" | one owner, gen 1, no source given | drive2 Golf Mk6 <https://www.drive2.ru/l/538296979654967321/> | anecdote |
| gen-1 histogram bins: < 100, 101–150, 151–200, 201–250, 251–300, 301–350, > 350 °C | VCDS groups 099/102 (K1) and 119/122 (K2) | drive2 <https://www.drive2.ru/l/472157506953543828/>; sarafan-auto.by <https://sarafan-auto.by/blog/n3-dsg7-dq200-diagnostika-resurs-sceplenie-mekhatronik-mahovik.html> | many logs |

**Cross-check of the bins against this car (strong).** Gen 2 has 7 range counters per clutch (`1065`–`1068`,
`1120`–`1122` for K1; `107A`–`107D`, `1124`–`1126` for K2). They line up with the gen-1 edges:

| bin | K1 | K2 |
|---|---|---|
| <100 | 5107.8 h | 4984.2 h |
| 101–150 | 796.8 h | 916.8 h |
| 151–200 | 12.7 h | 16.0 h |
| 201–250 | 204 s | 1047 s |
| 251–300 | 0 | 224 s |
| 301–350 | 0 | 5.3 s |
| >350 | 0 | 0 |
| lifetime max | 233 °C (falls in 201–250 ✓) | 308 °C (falls in 301–350 ✓) |

Both maxima fall in the last non-empty bin, so the edges very likely carry over to gen 2. That is still an inference, since there is no OEM text.

### Normal range

| observed | source | strength |
|---|---|---|
| 85–105 (K1), 91–134 (K2), 91–127 °C (central plate) in a 142 s drive | `research/dumps/drive-gearbox.csv` | car |
| 99.6 % of 5,917 h below 150 °C; 15–16 h in 151–200 °C (table above) | `research/dumps/gearbox-full.log` | car |
| traffic 70–90, rarely over 100 °C | drive2 Golf Mk6 (link above) | many logs |
| calm traffic ≤ 100, jerky traffic 100–110, brisk driving 140–150, peak 200; one lifetime maximum of 225 | t-one.ru (repair shop) <https://t-one.ru/knowledge/articles/dsg/samostoyatelnaya-diagnostika-temperaturnykh-parame> | anecdote (shop) |
| "normal up to 150, sometimes 200"; above 250 counts as overheating | drive2 Passat B7 (link above) | anecdote |

---

## Recommendation

| channel | trip | release | basis | confidence |
|---|---|---|---|---|
| `01:IDE00196` oil (above) | **115.0** | **110.0** | Release at the OEM line (Škoda manual: above 110 °C, avoid full load), so a release means back inside VW's operating band. Trip 5 °C over it (inference): stock cars sit at 105–112 °C in town and summer traffic and would hold an alarm at 110. Many owner logs put 115–120 °C at heat or clogged radiators only, which is worth seeing | medium |
| — strict alternative | 110.0 | 107.0 | the OEM figure taken literally. Expect it to fire in hot-weather town driving | — |
| `02:IDE00102` control module (above) | **120** | **110** | 19 °C below the unit's own torque cut at 139 °C (SSP 390, 0AM; applied to 0CW by inference), and 19 °C above this car's lifetime maximum of 101 °C (`1021`, inference that it is the same quantity). Release 110 is above the 30–100 °C adaptation window and 9 °C beyond the car's lifetime max (inference) | medium |
| `02:102B` / `02:102C` linings (above) | **250** | **200** | No OEM number. Trip at the bin edge where this car spent only 224 s in 5,917 h (K2) and K1 never went. Owners and a shop put 200 as the peak of brisk driving and above 250 as overheating. Trip stays under the observed no-warning point of 308 °C and under the anecdotal 350 °C first warning, so the dash speaks before the TCU acts. Release at 200, the top of the "sometimes" band | low |
| `02:102A` central plate | no alarm | — | no source at all. It tracks between the linings (127 vs 105/134 in the drive log). Alarm on 102B/102C instead | — |

Other option for the clutches: the TCU's own verdict is the counters `387F`–`3886`, all 0 today. An alarm on
any increase would need no threshold of ours. But the counters are per-event, the dash has no "counter moved"
rule, and polling eight more DIDs costs bus time (inference; not evaluated).

## Contradictions and traps

- SSP 390's body text says 139 °C; its test question says "approx. 140 °C". Same measure, so use 139.
- The 138/145 and 160/170 °C figures in Škoda SSP 94 are **DQ250 (02E)**, from sensors G93 and G509. A web summary
  credited them to 0AM. They do not apply.
- Quotes of the oil range from "the manual" disagree: 80–110 (verified in the 11.2015 manual), 85–105 (a BRISKODA
  poster) and 90–110 (a drive2 commenter). Only 80–110 was confirmed against the document.
- A "cluster oil-temperature warning at 135 / 137–140 °C" (drive2 comments) is not in the Octavia manual, and the variant has no
  oil-over-temperature fault code. Treat it as unconfirmed.
- `1069` / `107E` "Protective_Function_**Capacity**_Clutch_N" are non-zero on this car (57 and 6). They count capacity
  (torque) protection events, **not** temperature. Do not read them as overheating.
- kfz-dietrich.com, runningroo.com.au and similar workshop-SEO pages give DTC lists and block numbers that
  contradict each other. None was used as a basis.
