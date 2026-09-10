# The fault codes in an ODIS project — two objects, one number, one language

2026-09-10. What `DB_DOP_DTC` and `MCD_DB_DIAG_TROUBLE_CODE` hold, read off the objects
of the reference project (`SK37X`, converter 2610.2.688, 230 pools) with the throwaway in
[`dump/`](dump/); what joins them to a `0x19` response; and what "language" means for a
text in there. Every number below was measured, and the commands that produced it are
in §8. The loader this established is `crates/data/vag-data-labels/src/odis/loaders/dtc.rs`;
the walk is `Project::faults` in `odis/mod.rs`.

Read `.archive/research/labels/odis-format.md` §1 and §3 first — the pools, the
positional stream, and the four ways the community reference misreads it. Nothing here
was transcribed from that reference; the field order was read off the bytes.

---

## 0. Verdict up front

- **A fault code is a 34-byte object and there are 291,346 of them** in the project,
  every one the same length. It carries the 24-bit number the control unit sends, the
  code a tester prints, the text, and a level — and one text, not one per language.
- **The join from the car is the number, not the display code.** `trouble_code` equals
  the number in `DTC_<n>` on 291,346 of 291,346 objects; the display code (`P150B00`)
  agrees with the SAE encoding of that number on 1,515 of 43,378 pairs. The three bytes
  `00 01 29` are 297, and 297 is the key.
- **The chain is the measurement chain's shape with two hops fewer**, and it holds on
  every variant: 717 variants, 621 with a fault table, 282,621 codes, none refused, none
  failed, 94 s.
- **Language is a property of the source, not of a row.** The project declares `deu`
  once, in `index.xml`; the object model has no language field; the engine's 202,863
  texts are English inside that German project. So the cache records what each source
  declared, and choosing a language means choosing a source.
- **On the reference car's fifteen stored faults**, the project names all fifteen; the
  display codes agree with what VCDS printed on every code both name (§7).

---

## 1. Where the codes live

Type census, whole project (`dump … all dtcstat`, `dump <pool> census`):

| object | count | length |
|---|---|---|
| `0x0057 MCD_DB_DIAG_TROUBLE_CODE` | **291,346** | 34 bytes, every one |
| `0x0028 DB_DOP_DTC` | **696** | 70 to 9,454 bytes |

Where they are — the twelve pools with the most code objects:

```
202,863  0.0.0@BV_EnginContrModul1UDS.bv        12,611  0.0.0@BV_Brake1UDS.bv
 23,836  0.0.0@BL_ECMDFCC.sd                     10,840  0.0.0@BV_SteerAssisUDS.bv
  7,908  0.0.0@BV_CentrElectUDS.bv                4,518  0.0.0@BV_AirbaUDS.bv
  3,070  0.0.0@BV_AdaptCruisContrUDS.bv           2,270  0.0.0@BV_FrontSensoDriveAssisSysteUDS.bv
  2,241  0.0.0@BV_DashBoardUDS.bv                 2,151  0.0.0@BV_TransContrModulUDS.bv
  1,939  0.0.0@BV_TelemCommuUnitUDS.bv            1,299  0.0.0@BL_DataLibraInfot.sd
```

The engine pool alone is seven tenths of the total, because every engine variant carries
its own table of ~500 codes and there are 402 of them. Shared-data (`.sd`) pools hold
tables too — `BL_BCM.sd` is where the body control module's codes are — which is why a
variant's table is reached through its layer's index rather than by scanning its own pool.

(`todo/README.md` used to say 329,268 `DTC_*` objects. That was a count of names in the
ASCII pool; the object count is 291,346. Both are the same data.)

---

## 2. `MCD_DB_DIAG_TROUBLE_CODE` — 34 bytes

Transcribed from `DTC_EV_ECM20TDI01104L906026RA_004_DTCDOP_VAGUDS.DTC_17154` in the
engine pool. Every four-byte word is a hash into a string pool (`A` = ASCII, `U` =
Unicode, `0` = absent); the two integers are little-endian.

```
0000: 57 00 cb c8 c6 0a 01 14 72 6f 8d e4 19 08 76 92
0010: 2b 3f 02 00 00 00 02 43 00 00 00 8d e4 19 08 23
0020: 3e 00
```

| offset | width | field | this object |
|---|---|---|---|
| `00` | u16 | type code | `0x0057` |
| `02` | A | ODX `ID` of the DTC element — the supplier's name for the check | `RB_DFC_VLCAvl_aVeh_VW.17154` |
| `06` | A | short name | `DTC_17154` |
| `0a` | A | **display code** — what a tester prints | `P150B00` |
| `0e` | U | **text** | `Acceleration monitoring⏎Control limit exceeded` |
| `12` | u32 | **level** | 2 |
| `16` | u32 | **trouble code** — the number the unit sends | `0x4302` = 17154 |
| `1a` | u8 | flag — `IS-TEMPORARY` | 0 |
| `1b` | A | text id | `P150B00` |
| `1f` | 3 | terminator `#>\0` | |

What varies across all 291,346, and what does not:

- **Level** is 1..9: `{1: 12,402, 2: 227,297, 3: 5,525, 4: 12,936, 5: 3,202, 6: 29,375,
  7: 88, 8: 2, 9: 519}`. It is the fault *priority* VW's tools show: the reference car's
  brake fault 297 is level 2 and VCDS printed `Приоритет неисправности: 2` for it; the
  body control module's fault 531 is level 6 and the extended-data record the unit
  returned for it opens `06` (`.archive/research/car/whole-car-survey.md` §2.3). Recorded
  in the cache as `level`, not interpreted — two agreements are evidence, not a decoder.
- **The flag at `1a` is 0 on every object.**
- **The text id at `1b` equals the display code on 291,290 objects.** The 56 that differ:
  `TXE<display code>` in two supplier libraries (`BL_LIBMVEM.sd`, `BL_LIBNVEM.sd` — e.g.
  `P351812` / `TXEP351812`), and absent on the engine pool's `P000000` dummy codes. So it
  is the identifier of the *text*, the same kind of key a measurement's `long_name_id`
  is, and on this project it is spelled with the display code.
- **The ODX id at `02` is absent on the body electronics** (`DTC_BL_BCM_DTCDOP_VAGUDS.*`
  all carry `0`) and present on the engine's Bosch checks.
- **2 objects have no text and 15 no display code**, of 291,346.
- **Display codes are seven characters**: a letter, four hex digits, two more —
  `A999999` 195,180 times, the rest the same shape with hex letters among the digits.
  The trailing pair is the failure type byte (`B1168F2` → `F2`), which VCDS prints
  separated: `B1168 F2`.

---

## 3. `DB_DOP_DTC` — a table of codes

The smallest one, 70 bytes, `DOP_EV_ECM14TFS01104E907309E_001_DTCDOP_VAGUDS`:

```
0000: 28 00 01 00 94 15 00 00 aa 34 f6 06 63 17 55 54
0010: 00 00 01 0a 00 00 00 00 01 23 00 02 18 00 00 00
0020: 00 01 0b 01 00 01 3c 00 01 00 10 ac ad 58 31 af
0030: 21 87 7c 00 00 00 00 00 00 00 00 00 00 00 00 00
0040: 00 00 00 23 3e 00
```

| offset | width | field | this object |
|---|---|---|---|
| `02` | u16 | count of codes | 1 |
| `04` | per code: u32, A, A | number, object id, pool id | 5524, `DTC_…_DTCDOP_VAGUDS.DTC_5524`, `0.0.0@BV_EnginContrModul1UDS.bv` |
| `10` | u16 | a second collection — **0 on all 696** | 0 |
| `12` | nested | `DB_COMPU_METHOD`, category `IDENTICAL` | `01 0a 00 · 00 00 00` |
| `18` | nested | `DB_DIAG_CODED_TYPE`: standard length, **24 bits**, no mask, `UINT32`, encoding none, **high-low byte order**, not condensed | |
| `25` | nested | `DB_PHYSICAL_TYPE`: `UINT32`, no precision, radix 16 (10 on one object) | |
| `2b` | A | short name | `DTCDOP_VAGUDS` |
| `2f` | U | long name | `VAG UDS` |
| `33` | U | description | absent here; `<p>ADA-DTC</p>` on `DOP_BL_LIBADA_DTCDOP_ADADTC` |
| `37` | A, A, A | reserved, long name id, description id | absent |
| `43` | 3 | terminator | |

The last six names are the shape `MCD_DB_ECU` ends on (`identity.rs::ecu`). Across all
696 DOPs the bytes after the code map take four patterns, differing only in which of the
three trailing names is present (`dump … all dopstat`); the field order above ends on the
terminator for all of them.

The map's references: **291,400 name the DOP's own pool, 17,736 another pool, 0 fail to
resolve.** The "another pool" entries are how a `.bv` variant's table reaches code objects
that live in a shared `.sd` library — resolved through the usual `Ref{object, pool}`.

The coded type says what the number *is*: a 24-bit unsigned integer, most significant
byte first — exactly the three bytes of a UDS `0x19` record read as `format_code` reads
them.

---

## 4. The chain, and that it holds

From a real layer, `LD_EV_ECM14TFS01104E907309E_001` (3,941 bytes), the two places the
table appears (`dump … object LD_…`):

```
@015f A "DTCDOP_VAGUDS"                                   ← dtc_properties, right after the services map
@06c9 A "DTCDOP_VAGUDS"                                   ← the property index: key …
@06cd A "DOP_EV_ECM14TFS01104E907309E_001_DTCDOP_VAGUDS"  ← … and the object it names
```

So:

```
DB_LAYER_DATA (LD_<variant>)
 ├ dtc_properties        a name list: the fault tables' short names   ("DTCDOP_VAGUDS")
 └ properties            name → Ref{object, pool}                     (→ "DOP_EV_…_DTCDOP_VAGUDS")
    └ DB_DOP_DTC         number → Ref{object, pool}                   (→ "DTC_EV_…_DTCDOP_VAGUDS.DTC_5524")
       └ MCD_DB_DIAG_TROUBLE_CODE
```

`layer_head` in `identity.rs` had been reading the name list as `_dtc_properties` and
discarding it; it is kept now. A variant whose own layer names no table is read through
the first parent layer that does — the same rule `Store::measurement_layer` applies to
the service, and for the same reason (a door unit's layer declares nothing and names its
base variant's pool).

Over the whole project (`dump … all validate`):

| | |
|---|---|
| variants | 717 |
| with a fault table | **621** |
| with none — own layer and parents alike | 96 |
| codes | **282,621** |
| refused (a type on the never-parse list in the way) | 0 |
| failed to parse | 0 |
| wall time, cold | 93.9 s |

Two table names occur: `DTCDOP_VAGUDS` (282,489 codes) and `DTCDOP_VAGUDSUserDefinMemor`
(132) — a second, user-defined memory on a few variants, kept apart by the `dop` column.

---

## 5. The number is the key; the display code is a string

Three facts, measured over every code object:

1. `trouble_code` (offset `16`) equals the `<n>` of `DTC_<n>`: **291,346 of 291,346**.
2. The display code is **not** the SAE encoding of that number: of the 43,378 distinct
   `(display, number)` pairs, 1,515 agree with `P/C/B/U` + hex, 41,863 do not.
   `B102C46` sits over 143, 338, 4240 *and* 9,448,518; `DTC_17154` displays as `P150B00`
   (0x150B00 would be 1,379,072).
3. A number means different things on different units: 6,092 of the 37,921 distinct
   numbers carry more than one text across the project (number 0 alone has seven, from
   `Development DTC: Dummy DTC` to `ECU unlocked`). So a text is looked up **per variant**,
   never by number alone — which is why the `fault` table is keyed `(variant, code)`.

On the reference car this reads as (the units' `F19E` from the parked survey, the numbers
from `vagcan faults`, the variant the identity picks in brackets):

| unit | bytes | number | display in the project | VCDS printed |
|---|---|---|---|---|
| 713 ESC | `00 01 29` | 297 | `B1168F2` (`_035`–`_038`; `B116816` on `_032`–`_034`) | `B1168 F2` |
| 713 ESC | `00 00 4B` | 75 | `C101C07` | `C101C 07` |
| 70C column | `04 71 20` | 291104 | `B145501` (`EV_SMLSVALEOMQBLRH_001`) | `B1455 01` |
| 70A parking | `D0 17 32` | 13637426 | `U112300` | `U1123 00` |
| 712 steering | `00 4F 04` | 20228 | `U112300` | `U1123 00` |
| 70E BCM | `00 02 13` | 531 | `B11FF01` (`EV_BCMMQB_017`) | — (no `.rod`) |
| 710 gateway | `01 04 05` | 66565 | `U120200` (`EV_GatewNF_013`) | — (no catalogue) |

Every display code VCDS printed is the project's, character for character.

---

## 6. Language: declared once, carried nowhere

**Where the declaration is.** `index.xml`, in the catalog's `ADMIN-DATA`:

```xml
<ADMIN-DATA>
    <LANGUAGE>deu</LANGUAGE>
```

(`odis-crib.md` §6 placed it in `_META/SK37X_META.xml`; that file is a feature manifest
of update revisions and has no language element. `index.xml` is the one; `grep -il lang`
over every XML in the project finds only it.) `Project::language()` reads it.

**What a code carries.** One Unicode hash, at offset `0e`. There is no second text field,
no language byte, no per-language table anywhere in the object; and no pool is a language
table — the 230 pools are 54 `.bv`, 166 `.sd`, 4 `.fg`, 3 `.pr`, 2 `.cp`, 1 `.vi`, named
for base variants, libraries, groups, protocols, com params and the vehicle, none for a
language.

**What language the texts are in.** A word-list guess over all 291,346 texts:

| guess | texts | where |
|---|---|---|
| English | 210,966 | the engine pool's Bosch/Continental texts: `Crankshaft Position Sensor A Circuit` |
| German | 15,150 | body, gateway, steering: `Lokaler Datenbus 6 (LIN)->elektrischer Fehler im Stromkreis` |
| undecided | 65,228 | nearly all German on inspection: `Airbag_02 Datenbus fehlende Botschaft` |

So **the owner's expectation that one project carries several languages does not hold
for this one, and the opposite expectation — that a project declaring `deu` is German —
does not hold either.** The text is whatever the supplier delivered: the engine's in
English, the body's in German, one text per code. A second language would be a second
project, with its own `index.xml` and its own 291,346 objects.

**The design that follows.** Language is recorded once per *source* (`source.language`,
filled from the declaration — `deu` here; a VCDS build's `Codes.dat` records `eng`,
`Code-RUS.dat` `rus`), never per row. `[faults] language = "…"` in `config.toml` picks a
source when several are set up and the codes they declare differ; with one source the
setting is never needed; unset with several, the first ODIS source wins and the listing
says so and names the setting. That is the smallest honest thing: it cannot translate,
so it does not pretend to.

---

## 7. `faults --from` on the reference car, before and after

The survey those figures came from (`research/dumps/survey-parked.jsonl`) is gitignored
and no longer on disk; [`reference-survey.py`](reference-survey.py) writes
[`reference-survey.jsonl`](reference-survey.jsonl) from what the archive recorded of it —
the fifteen confirmed codes and each unit's `F19E`, plus the `F1A2` of the three units
VCDS was seen to identify (`fault-naming-hop.md` §12.1, `other-ecus.md` §2). The two runs
are `vagcan faults --from research/odis-dtc/reference-survey.jsonl` on this branch, before
and after `vagcan setup ~/Downloads/SK37X` wrote the fault table.

Both runs, offline, on the same six units and fifteen codes. "VCDS" is the `dash` build
before this branch (`Codes.dat` + the unit's `.rod`); "ODIS" is this branch with the fault
table written.

| unit | number | VCDS | ODIS |
|---|---|---|---|
| 16 `EV_SMLSVALEOMQBLRH` | 291104 | `B1455 01` Temperature Sensor for Heated Steering Wheel | `B145501` Unterbrechung des Signalpfades "Init" |
| 03 `EV_Brake1UDSContiMK100ESP` | 75 | `C101C 07` Rear Left Wheel Speed Sensor | `C101C07` missing wheel speed sensor signal or wheel speed sensor signal continuously indicates a too low wheel speed |
| 03 | 91 | `C101D 07` Rear Right Wheel Speed Sensor | `C101D07` (same text) |
| 03 | 297 | `B1168 F2` Steering Angle Sensor: Not Initialized | `B116816` Swa_lost_initialisation |
| 10 `EV_EPHVA14AU3700000` | 13637409 | `U1123 00` Databus: Received Error Message | `U112300` Bremsensteuergerät - ESP_10.ESP_WegimpulsHL - Fehlerspeicher auslesen |
| 10 | 13637410 | `U1123 00` Databus: Received Error Message | `U112300` Bremsensteuergerät - ESP_10.ESP_WegimpulsHR - Fehlerspeicher auslesen |
| 10 | 13637422 | `U1123 00` Databus: Received Error Message | `U112300` ESP - ESP_19.ESP_HL_Radgeschw_02 - Fehlerspeicher auslesen |
| 10 | 13637423 | `U1123 00` Databus: Received Error Message | `U112300` ESP - ESP_19.ESP_HR_Radgeschw_02 - Fehlerspeicher auslesen |
| 10 | 13637426 | `U1123 00` Databus: Received Error Message | `U112300` ESP - ESP_21.ESP_Systemstatus - Fehlerspeicher auslesen |
| 44 `EV_SteerAssisMQB` | 20228 | `U1123 00` Databus: Received Error Message | `U112300` ESP_19 Botschaft - SNA-Check |
| 44 | 19716 | `U1123 00` Databus: Received Error Message | `U112300` ESP_10 Botschaft - SNA-Check |
| 09 `EV_BCMMQB` | 263 | — (no `.rod` of that ODX name) | `B131411` Klemme 30 für Innenbeleuchtung (KL30G)->Kurzschluß nach Masse |
| 09 | 531 | — | `B11FF01` Fußraumbeleuchtung->Teilausfall |
| 09 | 395521 | — | `B202300` Störschutzzählerüberlauf |
| 19 `EV_GatewNF` | 66565 | — (no fault catalogue in the family) | `U120200` Bus defekt CAN-Diagnose |

| | VCDS | ODIS |
|---|---|---|
| named | 11 | **15** |
| named by both | 11 | 11 |
| display code agrees, of those | 10 | |
| ODIS only | | 4 (the body control module's three, the gateway's one) |
| VCDS only | 0 | |

What the table says beyond the count:

- **The texts are different kinds of thing.** VCDS's are a tester's sentence (`Rear Left
  Wheel Speed Sensor`); ODIS's are the supplier's own (`Swa_lost_initialisation`,
  `ESP_10 Botschaft - SNA-Check`) — more specific where VCDS collapses seven faults into
  `Databus: Received Error Message`, and less readable where the supplier never meant a
  driver to see it. Language follows the supplier, as §6 measured: the brakes in English,
  the body in German, inside one `deu` project.
- **The one display-code disagreement is a variant choice, not a text error — and the
  line says which choice.** The survey carries no `F1A2` for the ESC, so the identity
  falls back to the family: eight variants of `EV_Brake1UDSContiMK100ESP` match, and
  they do not all write 297 the same way. The row is still shown — it is the project's
  answer and the alternative is no name at all — but nothing presents it as settled:

  ```
  03  EV_Brake1UDSContiMK100ESP
    00004B  (75)   confirmed
        C101C07  missing wheel speed sensor signal or wheel speed sensor signal continuously indicates a too low wheel speed  level 1  (variant _008 of 8 matching)
    00005B  (91)   confirmed
        C101D07  missing wheel speed sensor signal or wheel speed sensor signal continuously indicates a too low wheel speed  level 1  (variant _008 of 8 matching)
    000129  (297)   confirmed
        B116816  Swa_lost_initialisation  level 2  (variant _032 of 8 matching; they disagree — record F1A2 to settle it)
  ```

  The car's own variant (`_035`–`_038`, §5) prints `B1168F2`, which is what VCDS
  printed. Recording `F1A2` picks it and the whole parenthesis disappears; `vagcan dev
  survey` records it.
- **`0x19` bytes, the number, the variant, the text — no step of it needs a `.rod` key.**
  The four units the VCDS chain could not name had no catalogue to look in; the project
  had a table for every one of the six.

---

## 8. Reproducing

The throwaway in [`dump/`](dump/) links the crate and builds from its own directory
(`cargo build --release` there; it is excluded from the workspace). `P` is the project
directory, `E` the engine pool `0.0.0@BV_EnginContrModul1UDS.bv`:

```
dump P E census                      §1 — type census of one pool
dump P E 0057 4                      §2 — four code objects, every word resolved
dump P E smallest 0028               §3 — the 70-byte DOP
dump P all dtcstat                   §2, §5, §6 — every code object in every pool
dump P all dopstat                   §3 — what follows the map, over all 696 DOPs
dump P E object LD_EV_ECM14TFS01104E907309E_001    §4 — where a layer names its table
dump P all validate <variant prefix>:<number> …    §4, §5 — Project::faults over all 717
```

Nothing under `~/Downloads` is read by any test: the crate's end-to-end test synthesises
a miniature project with a fault table of two codes (`odis/mod.rs`, `miniature_project`),
and `loaders/dtc.rs` tests the two field orders on literal bytes.
