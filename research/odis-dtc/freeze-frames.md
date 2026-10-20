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
- The generic row (`RF.rod` line 21,547: `203,,11,,,,,0,0,,`) is the protocol's dynamic record
  `0x71`: a type-11 field of no length — read to the end, not as a length of 103.
- Lengths: lead − 100 equals the ODIS extent on every pair with both (SMLS 112/113 = 10,
  engine 112 = 9). Over the whole of `RF.rod`, 1,028 runs of one (key, lead): lead − 100
  equals the fields' extent in 897; 47 have a field past it, 33 end short of it, 51 lead ≥ 200
  or a record with no length. The decoder uses lead − 100 only where every field fits
  (`freezeframe::proven_length`); otherwise the record runs to the end and is bytes.
- A type-11 row is named `Reserved…` on every car unit and absent from VCDS's printout of the
  same records (gateway: VCDS shows "Terminal 30 power supply", "Terminal 15" of the six
  rows of record 112). Not shown.

Names from VCDS match ODIS by sense, not field by field: engine "Engine speed / Normed load
value / Vehicle speed / Coolant temperature / Intake air temperature / Ambient air pressure /
Voltage terminal 30" against ODIS's "Engine Speed / Calculated Load Value / …"; steering's 113
"Engine Run / Systemstate / Supply Voltage / Temperatur / KFC …" against VCDS's printed
"Статус двигателя / Состояние системы / Напряжение питания / Температура / KFC …".

## 4. The one raw answer

`dtc.rs`'s `BCM_000107`, the body control module's `19 06 000107 FF`, split as shipped with
the real cache's rows for `EV_BCMMQB_017` (scratch copy, `ffcheck`):

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
fields end before lead − 100 (their tail bytes are not shown); every unit-specific value
against VCDS on the same fault. The reference car's other 16 stored faults were read before
the bytes were kept.

## 5. Why ODIS is not the shipped source for the unit's own records

`MUX_DTCExtenDataRecor` is an `MCD_DB_PARAMETER_MULTIPLEXER` whose cases are `DB_CASE` /
`DB_DEFAULT_CASE` sub-objects — two of the ten types on the reader's permanent refusal list
(`vag_data_labels::odis::loaders::REFUSED`; "nothing on this list comes off it"). The
research above read their strings from hex dumps and never parsed one into code. ODIS would
give the exact per-variant record → structure map, the camera variant's 12-byte record 0x02
and the gearbox's case-start offset; whether a reader of these cases, for the
extended-data multiplexer alone, may be written is the owner's decision, not this branch's.

## 6. Reproduction (scratch, not shipped)

- `odis-dtc-dump <project> <pool> a0 100000 MUX_DTCExtenDataRecor`, every pool: 171 objects.
- `odis-dtc-dump <project> <pool> object <id>` for the service, response, ENVDESC, ENVDAT,
  EOPDUF and the structures in §1.
- `extent.rs`: a 60-line binary over `vag_data_labels::odis::{keyfile, pool, strings,
  loaders}` that loads a structure and its DOPs and prints the extent; fed the case
  structures a string pass over the multiplexer dumps lists.
- `rfdump`: `registries::load(Section::FreezeFrame)` over `~/.vagcan/rod`, every row as TSV.
- `ffcheck`: `freezeframe::split` + `lines` over §4's bytes and a copy of the cache.
