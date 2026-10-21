# Freeze frames — what a stored fault's `19 06` answer holds, and how it splits

2026-10-21. What VW's units store with a fault (the "freeze frame", VCDS's
*Сохраненный стоп-кадр*), how the answer is framed, and which description of it is proven.
Read off the reference project (`SK37X`, 230 pools) with the throwaway in [`dump/`](dump/)
and two scratch tools (§6), against the reference car's VCDS install (`vcds-en` 25.12) and
the one raw answer on record. Nothing here talked to a car.

---

## 0. Verdict up front

- **VW uses `19 06` only.** The protocol layer `PR_UDSOnCAN` carries a
  `ReadDTCInforReporDTCExtenDataRecorByDTCNumbe` service and **no** `19 04` (snapshot)
  service; its description reads "Dienst zum Auslesen von Umgebungsdaten zu einem DTC".
  `faults` already sent one `19 06 <DTC> FF` per stored code. No request was added.
- **The answer is records back to back, no lengths.** The response's last parameter is an
  end-of-PDU field of `STRUC_DTCExtenDataRecor` — one byte record number, then
  `MUX_DTCExtenDataRecor` switched on it. A reader splits by knowing each record's length.
- **Records `0x01` and `0x02` are VW's standard, the rest the unit's own.**
  `0x01` *Standard DTC Information* = 2 bytes (priority, occurrence counter);
  `0x02` *Standard Freeze Frame#1* = 10 bytes (central aging counter, km mileage u24, time
  indication, time stamp 5 bytes). Of the 171 multiplexers 169 carry both: 0x01 is 2 bytes
  in 169/169, 0x02 10 bytes in 168/169 (§2).
- **The earlier "13-byte record 0x01" was two records.** `dtc.rs`'s "counter that rises with
  time, shared across units" (`02B8`) was record `0x02`'s number and its aging counter `B8`.
  Mileage and date were right; their offsets did not move.
- **VCDS's `RF.rod` key is the record number; `999` is the default case.** The unit lists of
  VCDS and the ODIS multiplexers give the same record numbers on every unit both describe
  (§3). The lead − 100 is the record's length where the fields fit inside it (897/1,028).
- **The one raw answer on record splits exactly** (§4): the body control module's
  `01 [2] 02 [10] 70 [5]`, the third decoded by VCDS's default-case rows into the same
  fields, in the same order, that VCDS printed for that unit's frames.
- **ODIS describes the unit's own records too, but behind a refused type** (§5). Shipped:
  VCDS's rows for the unit's own records; ODIS's protocol layer for 01/02.

## 1. The chain in the project

```
DC_PR_UDSOnCAN_DiagnServi_ReadDTCInforReporDTCExtenDataRecorByDTCNumbe   (MCD_DB_SERVICE)
 └ RSP_…_Resp_ReadDTCInforReporDTCExtenDataRecorByDTCNumbe                (MCD_DB_RESPONSE)
    SID, subfunction, DTC value (3), DTC and status record, then
    └ DOP_PR_UDSOnCAN_ENVDESC_DTCExtenDataRecor     (0x0096, keyed by Param_DTCValue)
       └ DOP_PR_UDSOnCAN_ENVDAT_DTCExtenDataRecor   (0x0097, "All Reported DTC Extended Data Records")
          └ DOP_PR_UDSOnCAN_EOPDUF_DTCExtenDataRecor (0x0095, end-of-PDU field)
             └ STRUC_DTCExtenDataRecor: "Single DTC Extended Data Record" → MUX_DTCExtenDataRecor
```

`MUX_DTCExtenDataRecor` is resolved by short name through the variant's layers: 171 objects
in 43 pools — per variant (`DOP_EV_BCMMQB_017_MUX_…`), per base variant
(`DOP_BV_GatewUDS_MUX_…`), in the `ES_LIBVW80124V2x` libraries (VW 80124, VW's UDS standard)
and the protocol's own. The protocol's cases:

| record | case | structure |
|---|---|---|
| 1 | Standard DTC Information | `PR_UDSOnCAN_STRUC_StandDTCInfor` |
| 2 | Standard Freeze Frame#1 | `PR_UDSOnCAN_STRUC_StandFreezFrame1` |
| 113 (`0x71`) | Standard Freeze Frame Dynamic | wrapper: bytes to end of PDU |
| 144–147 (`0x90`–`0x93`) | B1 counter, DTC-based IUMPR, test result, monitor activity | OBD (ISO 14229-1's legislated range) |
| default | unknown freeze frame | bytes to end of PDU |

A variant replaces the default and adds its own: the engine `112 → StandFreezFrameECM`
(engine speed, load, speed, coolant, intake air, pressure, terminal 30 — 9 bytes); the BCM's
default case is `DataRecorMeasuValueAmbieDataLight11Er`.

## 2. Records 0x01 and 0x02 across the project

Every case 1 and case 2 structure of the 171 multiplexers, its extent computed from its
fields (byte position, bit position, coded bit length) through the shipped structure and DOP
loaders (`extent.rs`, §6):

| record | extent | multiplexers |
|---|---|---|
| 0x01 | 2 bytes | 167 (+2 at 3 bytes, see below) |
| 0x02 | 10 bytes | 164 (+2 at 11, +1 at 12, +2 declared 10 whose DOPs did not resolve) |

The other two of the 171 (`EV_LLPGen3LKEBODPO68X_002` and its right-hand twin, headlamp
modules) show a default case only in this string-level reading.

The 3- and 11-byte ones are the two DQ200 gearboxes (`EV_TCMDQ200021_001` — the reference
car's — and `_DQ250021_001`): their multiplexer's case-start byte (the u32 after the type
code) is 0 where the other 169 have 1, so their structures count the record-number byte.
After it they are 2 and 10. The 12-byte one is `EV_CamSysRVAV2ARCE2MQBAB_003` (rear-view
camera): a reserved byte before the aging counter and two after the mileage — **the one
variant the shipped split reads wrong**. Steering assist 013 and the DQ500 lay the time
indication out as a reserved constant at the same byte.

The time stamp is `STRUC_TimeStamp`, 5 bytes: year 7 bits (×1 + 2000) at byte 0 bit 2, month
4 at 1.6, day 5 at 2.1, hour 5 at 2.4, minute 6 at 3.6, second 6 at 4.0. On the low four bytes
that is `CarTime`'s packing exactly; byte 0 holds only the year's seventh bit.

## 3. VCDS's `RF.rod` against the multiplexers

Keys of the unit lists stored for the reference car (`registry_row`, section `FFMUX`) against
the cases of the same unit's ODIS multiplexer:

| unit | VCDS keys (lead) | ODIS cases (extent) |
|---|---|---|
| steering assist 712 | 113 (107), 114–125 (110) | 113–125, 1, 2, default |
| SMLS 70C | 112 (110), 113 (110), 114 (114) | 112 (10), 113 (10), 114, 1, 2, default |
| engine 7E0 | 112 (109), 113 (203) | 112 (9), 113 (dynamic), 1, 2, default |
| gearbox 7E1 | 112 (112), 113 (203) | 112, 113 (dynamic), 1, 2, default |
| BCM 70E | 999 (105), 113 (203) | default (BCM structure), 113 (dynamic), 1, 2 |
| climate, cluster, doors, OCU, MU, EPH | 113 (203), 144–147 | (no own multiplexer: protocol's — 113, 144–147) |

- Same record numbers on every unit both describe; `999` sits where ODIS has its only
  unit-specific case as the default.
- The generic row of key 113 — one row every such unit lists, lead 203, a type-11 field with
  no bit length and no name — is the protocol's dynamic record `0x71`: read to the end, not
  as a length of 103. The decoder runs `0x71` to the end whatever a unit's rows say, since
  the protocol defines it so (`vag_uds_client::dtc::DYNAMIC_FREEZE_FRAME`); the steering
  assist's and the SMLS's own fixed-length 113 records are bytes for that.
- Lengths: lead − 100 equals the ODIS extent on every pair with both (SMLS 112/113 = 10,
  engine 112 = 9). Over the whole of `RF.rod`, 1,028 runs of one (key, lead): lead − 100
  equals the fields' extent in 897; 47 have a field past it, 33 end short of it, 51 lead ≥ 200
  or a record with no length. The decoder uses lead − 100 only where the fields end exactly
  there and the lead is under 200 (`freezeframe::proven_length`): the 897. Every other record
  runs to the end and is bytes.
- A type-11 row is named `Reserved…` on every car unit and absent from VCDS's printout of the
  same records (gateway: VCDS shows "Terminal 30 power supply", "Terminal 15" of the six
  rows of record 112). Not shown.

Names from VCDS match ODIS by sense, not field by field: engine "Engine speed / Normed load
value / Vehicle speed / Coolant temperature / Intake air temperature / Ambient air pressure /
Voltage terminal 30" against ODIS's "Engine Speed / Calculated Load Value / …"; steering's 113
"Engine Run / Systemstate / Supply Voltage / Temperatur / KFC …" against VCDS's printed
"Статус двигателя / Состояние системы / Напряжение питания / Температура / KFC …".

## 4. The one raw answer

**Reference-car evidence, kept here and nowhere in the code** (the tests use an invented
answer of the same shape): the body control module's `19 06 000107 FF`, as an earlier
whole-car survey recorded it, split as shipped with the real cache's rows for
`EV_BCMMQB_017` (scratch copy, `ffcheck`):

```
01  06 09                          priority 6, occurrences 9
02  B8 03 3F 1B 00 00 69 F9 04 4B  aging 184, 212 763 km, 2026-07-28 16:17:11
70  04 04 0A 7B 91                 default case, lead 105: exactly the 5 bytes left
    Terminal 15 status OFF · Terminal 50 status OFF · CAN-Bus_aktiv On · brake light
    switch HW/CAN OFF · light switch: automatic high beam assist operated, the rest not ·
    last_wakeup 0A (multiplexed) · Voltage 12.3 V · Outside temperature 22.5 °C
```

The field list is VCDS's printout of the same unit's two frames on 2026-07-31, in the same
order. Not field-by-field proof — no VCDS printout of *this* fault exists — but the split is:
three records, the third exactly as long as its description, nothing left over.

What stays unproven: the time indication byte (0 in the one answer); the 33 records whose
fields end before lead − 100 (not used as lengths, so such a record is bytes); every unit-specific value
against VCDS on the same fault. The reference car's other 16 stored faults were read before
the bytes were kept.

## 4a. Checks the decoder makes (review of PR #47, 2026-10-21)

- **The unit's own variant only.** The layout is the VCDS rows of the stem the car's own
  lookup stored, else of the best-ranked name; either is used only when it is a name VCDS
  itself opens for the car (`OdxMatch::is_vcds_lookup`), with the car's project from its
  VIN. A `Family` or `Localised` match — 017's rows for a BCM on F1A2 `015…` — leaves the
  unit's records as bytes, "no layout for this variant".
- **The standard records are checked against the unit's own stamp** (`0x02BD`): mileage no
  further than the stamp's, and a time stamp that is a date. The camera variant's 12-byte
  record `0x02`, split as 10, reads 12 059 455 km; this check drops it. A unit that gives no
  stamp gets no context either: nothing to check by.
- **Misframing.** A split that ends short, or standard records that fail the check, mean the
  layout and the answer disagree: everything after record `0x02` is bytes. The default case
  (`999`) answers to any record number, so without this a misaligned answer read on as
  default-case fields.
- **One source.** The VCDS path reads VCDS rows only (`freeze_frame_fields(…, VCDS, …)`);
  records `0x01`/`0x02` are the UDS framing's.

## 5. ODIS as a source of its own (2026-10-21)

**Why `DB_CASE` was refused.** The project spec of 2026-08-07 (`21e63c58`) listed `DB_CASE`,
`DB_CASES` and `DB_DEFAULT_CASE` among the types "never parsed, regardless of ease", as
describing "adaptation/coding write cases"; the loader that enforced it (`23bb0b8b`) added the
consequence that a multiplexed *measurement* is skipped rather than reported wrong. Neither
was about freeze frames. The owner approved reading the extended-data multiplexer
(2026-10-21), and that multiplexer alone is read: `loaders::extended::multiplexer` reads its
cases inline, the dispatcher still refuses all three types, and a multiplexed measurement
value inside a record is still a field of unknown width.

**What is read.** The multiplexer by its short name through the variant's layers (own, then
parents nearest first); its case start (the u32 after the type code: 1, or 0 on the two DQ200
gearboxes, whose structures count the key's byte); the switch key (bit, byte, DOP — 5 bytes and
a reference; the first reading assumed a sixth byte and failed on every object at byte 23);
the cases, each two `DB_LIMIT`s and a structure; the default case. A case's limits are the
record number — or, on the two LLPGen3 headlamp variants, the key's text
(`"Standard DTC Information"`), turned into the number through the key's own text table. Each
structure through the shipped structure, DOP and compu loaders; a field whose DOP is a
structure (the 5-byte time stamp) is its fields at its offset. A record's length: the declared
size, else where its last field ends, a field of unknown width allowed only where a known one
follows it.

**Which source.** A run reads the one `Extracted::run_source` names — ODIS when the project
holds an ODIS source, else VCDS — and that source's rows alone (CLAUDE.md, 2026-10-21).

**Census, all of the reference project's variants.** 717 variants: 524 resolve to an
extended-data multiplexer, 193 to none, 0 errors. They reach 154 of the 171 multiplexer
objects (the other 17 are overridden on every variant that inherits them). Of the 154:

| | multiplexers |
|---|---|
| every field of every fixed record decodes (width and scaling known) | **79** |
| at least one field that does not | 75 |

528 fields do not decode in all; the most common are `Freeze Frame Operation Minute
Counter` (80), the climate units' `current/maximal/minimal_temperature` (29 each),
`Parameter` (16) and `last_wakeup` (14 — a multiplexed measurement, still refused). Those
show as bytes. Records `0x01`/`0x02` by length: `0x01` 2 bytes on 154/154, `0x02` 10 bytes on
153/154 and 12 on the camera variant — which the ODIS path now splits right.

**The BCM answer, each source on its own** (`ffcheck`, the cache's VCDS rows for
`EV_BCMMQB_017` against `Project::extended_records` of the same variant, never merged):

| field (VCDS name / ODIS name) | VCDS | ODIS |
|---|---|---|
| priority, aging counter | 6, 184 | 6, 184 |
| Terminal 15 status / signal terminal 15 | OFF | Off |
| Terminal 50 status / signal terminal 50 | OFF | Off |
| CAN-Bus_aktiv | On | On |
| brake light switch HW, CAN | OFF, OFF | Off, Off |
| light switch: OFF / LDS 0-Stellung | Not operated | not_actuated |
| Position lamps / LDS-SL-Stellung | Not operated | not_actuated |
| Automatic high beam assist / automatic_light_control | operated | actuated |
| Low beams, parking light left, right | Not operated ×3 | not_actuated ×3 |
| last_wakeup | `0A`, multiplexed, not decoded | width not known, not decoded |
| Voltage / voltage measurement | 12.3 V | 12.3 V |
| Outside temperature / Aussentemperatur (CAN) | 22.5 °C | 22.5 °C |

Same split (`01 [2] 02 [10] 70 [5]`, the default case on both), same 15 fields in the same
order; **13 of 13 fields both decode agree** in value, the wording each source's own; the 14th,
`last_wakeup`, neither decodes. A cross-check only: `faults` reads one source per run.

## 6. Reproduction (scratch, not shipped)

- `odis-dtc-dump <project> <pool> a0 100000 MUX_DTCExtenDataRecor`, every pool: 171 objects.
- `odis-dtc-dump <project> <pool> object <id>` for the service, response, ENVDESC, ENVDAT,
  EOPDUF and the structures in §1.
- `extent.rs`: a 60-line binary over `vag_data_labels::odis::{keyfile, pool, strings,
  loaders}` that loads a structure and its DOPs and prints the extent; fed the case
  structures a string pass over the multiplexer dumps lists.
- `rfdump`: `registries::load(Section::FreezeFrame)` over `~/.vagcan/rod`, every row as TSV.
- `ffcheck`: `freezeframe::split` + `lines` over §4's bytes, once per source: a copy of the
  cache for VCDS, `Project::extended_records` written to a scratch cache for ODIS.
- `muxcensus`: `Project::extended_data` over every variant, grouped by multiplexer.
