# The VCDS measurement registry — `RM.rod`

VCDS's own label files **do** carry, for every measurement, its DID, bit layout, scaling,
unit and name. It is in a global registry, **`RM.rod`**, that no earlier pass had opened. A
control unit's `.rod` file (its `MWB` section) is a list of **1-based row numbers** into that
registry — exactly as its `DTC` section indexes `RD.rod`, the fault registry we already ship.

This overturns the standing conclusion that "scaling is live-only" and that "the `.rod` corpus
holds no read identifier". Both rested on two mistakes, now corrected:

1. The `MWB` row's leading number was read as a **text-id** (a name pointer). It is a
   **row number** into `RM.rod`. `.archive/research/labels/label-linkage.md` §3's counting
   argument ("the per-ECU code has no per-ECU degree of freedom") fails at its premise: the
   per-ECU freedom is *which row the unit selects*.
2. `TTTEXT`'s per-record cipher key was thought unknown. It is `srand(record id)` — the same
   generator (`glyphs.rs::TableAlphabet::for_key`) we reversed for `RD.rod` in
   `.archive/research/labels/fault-naming-hop.md` §11. It was simply never tried on the name table.

Found by cross-checking a VW ODIS project (which declares the whole chain per variant) against
the VCDS files as a large known-plaintext crib, 2026-09-27/28. **ODIS was the oracle only** —
none of its data is needed at run time or shipped. In vagcan: the exact `TTTEXT` read
(2026-09-28, `label-lookup/02` phase 1); the registry, as `setup`'s step 5 (2026-09-28, §7).

Companion to `.archive/research/labels/` (read `label-linkage.md`, `tttext-codec.md`,
`fault-naming-hop.md` first). This file is the forward-looking record; the archive keeps the
reasoning those files were right and wrong about, each now cross-referenced here.

---

## 0. Verdict

| question | answer | confidence | evidence |
|---|---|---|---|
| Do the VCDS files alone give (DID, layout, scaling) for a unit's measurements? | **Yes**, for any unit whose `.rod` chain is not in the shifted-IV regime | high | 23 variants, 1,033 rows joined by ODX id: every field agrees in 91.3% (control 38.9%); car-proven rows present in the corpus: 18/18; log `IDE–ENG` pairs: 15/15 |
| Where does VCDS keep it? | In `RM.rod [MWB]`, a global registry (280,932 rows, 36,926 keys) the archive never opened. A unit's `[MWB]` lists **1-based row numbers** into it | very high | gearbox 12/12 proven DIDs at 1-based indexing; 1-based 91.3%, 0-based 72.4%, 2-based 64.6% |
| Is the DID a function of the VCDS text-id? | **No.** The text-id is per measurement; the file chooses the row | very high | 530/530 unit pairs sharing an id point to one key; where ODIS places an id differently, the units list different rows in 103/108; where two units list the same row, ODIS agrees 251/251 |
| `TTTEXT` record key | `srand(record id)` + two shuffles, `glyphs.rs::for_key`. Digits read too | certain | 195,910/195,910 records well-formed (control `srand(id+1)`: 6.6%); 4/4 ENG cribs incl. `Q005` |
| `TTTEXT`'s numeric tail | `<kind>,<ODX number>`: kind 2 = IDE, kind 7 = MAS. **Not the DID** | very high | 2,279/2,280 IDE and 2,749/2,810 MAS present; "tail = DID": 0/12,825 |
| The 2-char code | a hex byte under `srand`; a function of the RM key (the measurement), not a scaling class | high | purity by key 100% (shuffled 40.9%); by scaling 8.5–37.9% (shuffled 5.9–22.3%) |
| Does the bridge lean on names? | **No.** Joined by ODX id; names would match only 34.6% | certain | 357/1,033 |
| Reachable offline without ODIS | 479/542 matched variants have no shifted file on the known path; corpus-wide 7,273/12,284 `EV_` files are unshifted | medium (upper bound) | §1 census |

**One line:** VCDS keeps the full (DID, layout, scaling) for every measurement in `RM.rod`;
each unit's `.rod` selects the rows it uses; that selection matches ODIS and the car-proven
rows — readable offline wherever the file chain is not in the shifted regime (~41% of the
corpus is).

---

## 1. The mechanism

1. A unit's `[MWB]` row is `<n>,<code>`. `n` is a **1-based** row number in `RM.rod [MWB]`
   (row `n` = registry index `n-1`). Same shape as `[DTC]` → `RD.rod`.
2. Each `RM.rod` row is `<key>,<payload>`, the payload deciphered under `srand(key)` — 13
   fields in 99.3% of rows. The other 2,098 have 12, and what they lack is `f11`, not `f6`:
   their last field is the 4-character one in every row, where every 13-field row's `f11` has
   6 characters (review, 2026-09-28; this line said `f6` until then). Fields `f0`–`f10` keep
   their places either way:
   - **f0** — the DID, in decimal;
   - **f7·8 + f8** — bit offset; **f9** — bit length;
   - **f2** — type (`& 63`: 0 linear, 2 identity, 3 texttable, 4 OBD-II PID formula, 7 raw
     bytes, 8 ASCII), bit `64` = little-endian, bit `128` = signed;
   - value = `(raw·f4 + f3) / f5`; **f6** — unit id into `UNIT.ROD`; **f1** — TTDOP table id
     for a texttable; **f10** — the display name's text-id (the log's `ENG######`).
3. The RM key is the `TTTEXT` record whose tail names the measurement's IDE (kind 2) or MAS
   (kind 7). One key holds every layout of a measurement: `IDE00075` has 152 rows over 18 DIDs.
4. `TTTEXT` decodes in full with `srand(record id)`; the letter half matches the shipped
   solver, and the numeric half (previously unbroken) now reads too.

Worked example, `IDE00075` (vehicle speed), the unit's selected row = ODIS in every unit that
lists it: ACC `1016` s16BE ×1/256 m/s; BCM `2B16` u24 ×0.01 km/h; gearbox `F40D` u16LE ×0.01;
engine `F40D` u8 OBD PID `0x0D`. (`scripts/worked_example.py IDE00075`.)

---

## 2. Verification (this session, independent of the agent's scripts)

`scripts/check_gearbox.py` — the gearbox's own `MWB` (1,020 rows) → RM at each of three index
conventions, compared with the 12 car-proven rows in `0CW300041G.json`:

- **1-based: 12/12** proven DIDs present, and length + byte order + factor agree 12/12
  (×0.01 vehicle speed, ×0.4 pedal, the gear/selector texttables). 0-based 11/12, 2-based 9/12.
- The row for `380A` carries name text-id `103074`, exactly the VCDS log's `IDE00022-ENG103074`.
  No ODIS needed for this check.

## 3. The agent's wider numbers (`scripts/evaluate.py`, `scratch/out/eval.json`)

- **J1** (join by ODX id, DID level), n=1,033: DID 93.2%, position 92.7%, length 94.1%,
  signed 99.5%, order 100%, scale 95.1%, **all fields 91.3%**. Control (a random other row of
  the same key): all 38.9%.
- **J2** (parameter id + DID), n=2,739: all 99.3%.
- **J3** (structural, every ODIS row incl. those with no id), 7,789 rows: 84.8% have a VCDS row
  at the same (DID, bit, len); there sign+order+scale agree 93.0%. Control (another unit's
  list): 2.5%.
- Car-proven: gearbox 12/12, engine 3/3, cluster 3/8 (5 clock DIDs are in neither VCDS's list
  nor ODIS's variant). Log pairs: 15/15 (`IDE–ENG` shuffled control: mean 1.05).
- The bridge is joined by ODX id, not names; names would have matched 357/1,033 (34.6%).

## 4. Where things are — for the next session

```
research/vcds-registry/
  README.md            this file
  scripts/             the analysis (committed). Portable: paths from $ODIS_CRIB / $VAGCAN
                       or defaults relative to the repo. Python 3, sqlite3, no extra deps.
  scratch/             GITIGNORED — Ross-Tech-derived data, kept for convenience, regenerable:
    dump/<stem>/<TAG>.bin   decrypted+inflated sections (vagcan dev vcds rod --dump)
    out/tttext_plain.tsv    every TTTEXT record decoded (id <tab> text)
    out/rm_mwb.tsv          RM.rod [MWB] decoded (key <tab> row-in-key <tab> payload)
    out/bridge.jsonl        3,772 bridged rows (J1 1,033 + J2 2,739)
    out/eval.json           the J1/J2/J3 numbers above
    ivcache.json            recovered IV keys for the files opened here
```

Inputs the scripts read (not in the repo, per CLAUDE.md's data rule):
- `~/vcds-en/UDS_EV/` — the VCDS 26.3 English install (`.rod` corpus, `RM.rod`, `TTTEXT.ROD`, …).
- `~/.vagcan/data/SK37X/cache.sqlite` — the ODIS project, the oracle (`reading` table).
- `~/.vagcan/data/SK37X/measurements/*.json` — the car-proven rows, the ground truth.

Key scripts: `alphabet.py` (the `srand` codec, port of `glyphs.rs`), `decode_tttext.py`,
`decode_registry.py`, `evaluate.py` (J1/J2/J3 + controls), `worked_example.py`,
`check_gearbox.py` (this session's proof), `crack_driver.py` (drives `vagcan dev vcds rod`),
`step0_census.py`/`plan.py` (variant↔file map, section regimes), `inc.py` (INC section reader),
`census_installs.py` (installs compared, §6a).

## 5. Reproduction

```
export ODIS_CRIB=research/vcds-registry/scratch        # or wherever; default is ./scratch
cd research/vcds-registry/scripts
B=../../../target/release/vagcan

$B dev vcds rod ~/vcds-en/UDS_EV/TTTEXT.ROD --no-crack --cache $ODIS_CRIB/ivcache.json --dump $ODIS_CRIB/dump/TTTEXT
python3 decode_tttext.py  $ODIS_CRIB/dump/TTTEXT/TXT.bin $ODIS_CRIB/out/tttext_plain.tsv
$B dev vcds rod ~/vcds-en/UDS_EV/RM.rod --cache $ODIS_CRIB/ivcache.json --dump $ODIS_CRIB/dump/RM   # ~82 CPU-s
python3 decode_registry.py $ODIS_CRIB/dump/RM/MWB.bin $ODIS_CRIB/out/rm_mwb.tsv
python3 step0_census.py $ODIS_CRIB/out && python3 plan.py $ODIS_CRIB/out
python3 crack_driver.py 5400 $ODIS_CRIB/out/crack_order.txt   # opens the car's units, budget in CPU-s
python3 evaluate.py                                            # J1/J2/J3 -> out/eval.json, out/bridge.jsonl
python3 check_gearbox.py                                       # the 12/12 proof, needs only dump/ + the proven json
```

Cracking a unit costs 1–2 `.rod` opens (its `EV_` file for the `INC` list, and the `IV_…_M`
store), a median of ~147 CPU-s each; the reference car's 13 units together ~2,625 CPU-s.
`RM.rod` opens once (~82 CPU-s). Always pass `--no-crack` unless the section is classic — a
shifted section will search for hours (§6).

## 6. Not done, and the limits

- **Shifted-IV files: ~41% of the corpus (63/542 matched variants here).** The IV mask is a
  runtime global inside VCDS, not derivable offline (`.archive/research/labels/tttext2.md`
  §3.3a); opening one is hours of brute force per file. None of the reference car's 15 units is
  shifted. The 63 blocked variants of this project (`scratch/out/plan.json`, `reachable: false`)
  are mostly newer blocks: the ESC `Brake1ESCMQB37CLASS` (6) and late MK100 `IPB`/`OMG`, the EPS
  `SteerAssisBASGEN1MQB37` (7), the gateway `GatewMQB2020Class`, the clusters
  `DashBoardJCIMQBAB` 011–015 and `DashBoardVDDMQBA0`, two engines (`ECM00TFS01104C907309AL`,
  `ECM10TFS03005C906032Q`), Hella Gen3 headlights, the tailgate, seat memory, auxiliary heaters,
  the trailer module, infotainment (`MUCNS`, `MUHig6`, `MUOI`), telematics (`OCU3Clas5G`,
  `OCUClass2023`, `OCUGen3*`), the amplifier, cameras (`MFK*`), airbags TS6 22/23 and the ACC
  radar `MRRCONTI`. The reference car needs none of them. Route for those: ODIS (`calibrate`,
  the other route named here, was removed on 2026-09-28, owner).
- **BCM (`EV_BCMMQB`) has no file in any of the three installs of §6a** — a missing file, not
  encryption. `ReDir.rod` sends its variant to another file: §6b (2026-10-20).
- **Undecoded:** `f2` types 1/5/6/12, fields `f11`/`f12`, ~620 RM rows with an empty DID; the
  first row of a TEA `MWB` (block 0 not repaired); 3 unresolved `INC` first rows; how VCDS
  assembles the `ECM00*`/`GV_` engine lists.
- **`TTDOP` enum levels checked against the 25.12 table**, not re-cracked for 26.3.
- **`TTTEXT2.ROD` no longer matters for the DID** (it comes from RM); still unopened.
- **Budget spent:** ~111 CPU-min, incl. ~1,191 CPU-s wasted before the driver learned to skip
  shifted files. `crack_driver.py` now passes `--no-crack` unless INC/MWB is classic.

### 6a. Three installs compared (2026-09-28)

`scripts/census_installs.py` over the English 25.12 and the Russian install unpacked from
`vendor/*.zip`, and the English 26.3 in `~/vcds-en` — the first block of every section, seconds:

| | EN 25.12 (`vendor/vcds-en`) | EN 26.3 (`~/vcds-en`) | RU (`vendor/vcds-ru`) |
|---|---|---|---|
| data set (`DSVer.txt`) | 374.0 | 375.1 | 356.3 (files of June 2024) |
| `.rod` files | 16,576 | 20,088 | 15,465 |
| any section shifted | 6,204 (37%) | 7,830 (39%) | 5,152 (33%) |
| `RM.rod`, `RD.rod`, `STRUC`/`TTDOP`/`MUX`, `ReDir` | classic | classic | classic |
| names | `TTTEXT.ROD`, classic | same | `TTText-RUS.rod`, **shifted** |
| units | `UNIT.ROD`, classic | same | `Unit-RUS.rod`, **shifted** |
| fault texts | `Codes.dat` (English) | not in this copy (`Labels/`, `Scaling/`, `UDS_EV/` only) | `Code-RUS.dat` (Russian, 2019) — `setup` already reads it |
| `.lbl` label files | English | English | English too: no Cyrillic in any of 1,202 |

- **The regime is a property of the file, not of the release.** Between installs, by file name:
  `MWB` classic→shifted / shifted→classic in 1 / 3 of 5,021 files (EN 25.12 → EN 26.3) and 1 / 0
  of 2,910 (RU → EN 26.3); `INC` in 4 / 4 of 6,740 and 15 / 5 of 5,627. So no install opens what
  another keeps shifted — the 63 blocked variants of §6 are blocked in all three.
- **Every release is re-encrypted.** No file is byte-identical between any two installs (11,240
  to 15,113 common names each). A unit's `MWB` rows index **that install's** `RM.rod`: never mix
  files across installs.
- **RU is older.** 3,452 files are only in RU, 2,001 of them older versions of units the English
  installs carry under the same base name; 4,599 are only in EN 26.3 (newer units).
- **A Russian-only install gives numbers without words.** Its `RM.rod` reads (DID, layout,
  scaling), but its names and units are in shifted files. Russian measurement names would need
  the runtime mask (§6); Russian fault texts already work.
- **The reference car's units** are present and in the same regime in all three.

### 6b. `ReDir.rod` and `chassis.clb` — the unit names with no file (2026-10-20)

**`UDS_EV/ReDir.rod`** says which file VCDS reads for a unit name the installation has no file
of. Read by `vag-data-labels::redirect`; `label_files::find_rod_by_odx_variant` treats a source
as it treats a file name, so `setup`'s step 5 and the faults chain see it. `dev vcds labels
--odx` takes a file name, not a unit: it looks that exact name up, and with no file follows the
redirect of that very name (`Redirects::target`), not the ranked rule.

- **Container:** an ordinary `.rod`, 2 sections: `CMP` (TEA) and `DIR` (zlib, 1.1 MB, 18,478
  lines in EN 25.12). Classic, key in `rod-keys.json` under `ReDir.rod\tDIR`; opens in ~75 ms
  (release) with the key cached.
- **Line:** `NNNNNN,<glyphs>\r\n`. The decimal digits are the line's own key:
  `TableAlphabet::for_key(NNNNNN)` reads the rest as `<source name>,<target name>`, e.g.
  `EV_BCMMQB_017,EV_BCMBOSCH_017`, `EV_BCMMQB_016_SK37,EV_BCMBOSCH_016_VW37`,
  `CRFT1_ESP_3_12289,CRFT1_ESP_3_12288`. All 18,478 lines read.
- **`EMPTY`** as target (100 lines, e.g. `CRFT1_EPH_3_768,EMPTY`): VCDS has no data for that unit.
  A definite "no data", logged as `no data` — not "no file", and no sibling variant is guessed.
  Only at a rank VCDS tries for the car; another project's `EMPTY` says nothing about this car.
- **Measured on EN 25.12 only, not assumed by the code:** no source exists as a file; every
  target except `EMPTY` does; no target is itself redirected. The reader follows chains (8 hops),
  treats a missing target as no file, and when a source *is* a file the file wins.

**The order VCDS tries names in** — `OdxMatch`, one `rank` for file stems, redirect sources and
ODIS variant names:

1. `<name>_<vvv>_<project>` — `vvv` the first three digits of `F1A2`, the project the car's
2. `<name>_<vvv>`
3. `<name>_<project>`
4. `<name>`
5. `<name>_<vvv>_<other tag>` — the version under another localisation (this tool's guess)
6. `<name>_<anything else>` — the family (this tool's guess)

Within a rank: a file before a redirect, then by path. Ranks 1–4 name one name each, so the first
candidate is the name VCDS opens.

- **Evidence (`ReDir.rod` itself):** 154 lines redirect `<name>_<vvv>` and 59 lines
  `<name>_<project>` although `<name>` exists as a file or source — e.g.
  `EV_ACCBOSCHAU736_002 → EV_ACCBOSCHAU49X_002`, `EV_AIRBAVW10SMEVW360_SK37 →
  EV_AirbaVW10SMEVW360`. With the bare name first they would be dead lines. No line becomes
  dead under either order of 2 and 3 (0 lines `<name>_<vvv>_<p>` redirected beside an existing
  `<name>_<vvv>` or `<name>_<p>`), so 2 before 3 is the likelier order, not a measured one.
  `VCDS.exe` builds the names at run time; its strings hold only
  `Unable to find a .ROD file for: %s`.
- **Reachability census** (`ReDir.rod` sources parsed as `<name>[_<vvv>][_<project>]`, projects
  from `chassis.clb`; a line is reachable when, for some version and project, the lookup's first
  answer is that line): the first rule of this branch (bare name first, project a tie-break)
  left **213** lines unreachable (154 `<name>_<vvv>`, 59 `<name>_<project>`); this order leaves
  **0**, through the shipped `RodIndex::find` too. 9,859 sources have no such suffix and are
  reachable as their own `F19E`.
- **Joining rows:** a redirected unit's rows are stored under the **source** name, the name the
  unit joins on; the read looks a unit up once per recorded project that carries it, and a
  command naming the car joins by the stem its own project's lookup stored, logged as `stem`
  (`Extracted::for_car`), and by rank with its project only when none is logged — so two cars
  of two projects, or a car of no known project beside one, never share VCDS rows.
- **The log** (`registry.json`) records each unit per project, with the stem its rows went
  under, and a unit is untried until every project a read would look it up with has a line — a
  car of no known project is a lookup with none. A log with an older `read_version`
  (`READ_VERSION`: 1 for this order, 2 for the stems) has tried nothing, so a machine re-reads
  every unit once by itself. A "no file" logged while `ReDir.rod` did not open is read again
  only once the table opens.

**`UDS_EV/chassis.clb`** gives the project. A plain label file once decrypted: 282 lines
`<model code>,<project>` (18 with a numeric model code), the model code being VIN characters
7–8 by the VW group's convention (`NE`→`SK37`, `5E`→`SK37`, `8V`→`AU37`). The reference car's
`NE` gives `SK37`. Read by `vag-data-labels::chassis`; parsed on its own because `parse_label`
takes a numeric model code for a measurement block. `setup`'s step 5 takes the project of each
recorded car's VIN; `faults`, which reads no VIN, takes the projects of the recorded cars that
carry the unit. No project known: ranks 1 and 3 do not occur.

**On the reference car (copy of `~/.vagcan`, EN 25.12):** 70E (`EV_BCMMQB 017001`) →
`EV_BCMMQB_017 → EV_BCMBOSCH_017.rod`, 775 channels; against the ODIS variant `EV_BCMMQB_017`
(868 rows) joined on DID and bit offset, 769 agree on length, sign, byte order, scaling, factor and
offset, 0 disagree, 6 are VCDS-only. All 15 units: 4,905 agree, 0 disagree, 6,085 channels. The
project moved nine units to their `_SK37` names (eight through `ReDir`), with identical rows except
the gateway (`EV_GatewNF_SK37 → EV_GatewNF_VW37.rod`: 1,129 rows, the 1,123 of `EV_GatewNF_SK26`
plus 6), and needed 5 more section keys (one search each on first run).

## 7. Next — implementation (its own branch, reviewed)

**Built 2026-09-28**: [`.archive/tasks/done/label-lookup/02-vcds-registry.md`](../../.archive/tasks/done/label-lookup/02-vcds-registry.md)
has what landed, the acceptance on the reference car and what is left.

1. **Done 2026-09-28** (`label-lookup/02` phase 1): **read `TTTEXT` with
   `TableAlphabet::for_key(record id)`** in `vag-data-labels`, replacing the dictionary solver:
   seconds, no crack. Corrects ~1,870 of 14,736 catalog names (685 were letter misreads) and
   yields every record's digits. See §8 for the catalog delta. `setup` writes every record to
   `names.json` and the `IDE`/`MAS` ids to `odx-ids.json`; `vagcan dev vcds tttext` prints the
   same read as `scripts/decode_tttext.py`.
2. **A registry reader**: `RM.rod` once per install, then per unit INC → `IV_…_M` → rows;
   decode `f2` types 0/2/3/4/7/8 with the LE/signed bits; `UNIT.ROD` for units; `TTDOP` for
   enum levels. Feed the rows into the cache beside ODIS, so `setup` gives a VCDS-only owner
   scaling for any unshifted unit.
3. **Regression gates on the private data**: the 18 car-proven rows and the 15 log pairs. Test
   the 1-based row convention explicitly — neighbouring rows of a key are near-duplicates, so an
   off-by-one looks plausible (`check_gearbox.py` already shows 1-based 12/12 vs 0-based 11/12).
4. Then the `calibrate` decision: with a registry reader it is needed only for shifted units,
   units with no VCDS file, and measurements absent from VCDS's list. **Decided 2026-09-28
   (owner): removed entirely.**

## 8. What this refutes in the archive (each marked in place)

- `.archive/research/labels/tttext-codec.md` §5 ("key is not a function of the record id") and
  §6 ("numeric class unbroken") — **both wrong**: the key is `srand(record id)`.
- `.archive/research/labels/label-linkage.md` §3 — the premise ("`MWB` id is a text-id") is
  wrong; it is a registry row number, and the row is the per-ECU degree of freedom.
- `.archive/research/labels/scaling-audit.md` / `rod-labels.md` §4.0c — "`(DID → scaling)` not
  in the corpus / scaling live-only": **wrong**, it is in `RM.rod`.
- `.archive/README.md` "Do not look for a stored scaling in the label corpus" — **reversed.**
- `catalogs`/`names.json` — the shipped name catalog differs from the exact read in 1,870 of
  14,736 entries (1,153 digits/punctuation, 685 letter misreads, 32 unrelated); item 1 fixes it.

`.archive/tasks/roadmap-history.md` is dated history and is left as written; the dated status
that superseded it points here.

---

## 9. The other registries — RA, RG, RS, RF, RX, MUX (2026-10-20)

A unit's `.rod` (or the `IV_…` store its `[INC]` names) carries, beside `[MWB]` and `[DTC]`,
the sections `[ADP]`, `[GES]`, `[SOT]`, `[FFMUX]` and `[XPL]`. Each is a list of `<n>,<code>`
rows, `n` a **1-based** row number into one global registry, exactly as `[MWB]` → `RM.rod`.
Every registry row is `<key>,<payload>`, the payload under `TableAlphabet::for_key(key)`, and
after one or two fields of its own each carries **RM's value description in RM's order**:
`table, flags, offset, numerator, denominator, unit, byte, bit, bit length, name` (§1: type =
`flags & 63`, bit 6 little-endian, bit 7 signed; value = `(raw·num + off)/den`). Read by
`vag-data-labels::registries`, stored by `setup`'s step 5 and `registry::ensure` in
`cache.sqlite`'s `registry_row` / `registry_level` under the unit's file stem, beside its
channels. Nothing reads them back yet.

**What may ever use them.** Adaptations are read with `0x22` and freeze frames with `0x19`.
Basic settings are routines (`0x31`) and output tests input/output control (`0x2F`): both are
outside the allowlist for good, so those rows are names and nothing else.

Ground truth: the reference car's 14 units with a file, joined to their ODIS variants
(`EV_<unit>_<F1A2 major>`) through a throwaway reading of the ODIS project's
`ReadDataByIdentCalibData`, `RoutiContrRequeRoutiResulBasicSetti` and
`InputOutpuContrByIdentActuaTestShortTermAdjus` services (the measurement chain pointed at
another service; scratch only, nothing of it in the tree). Section keys came from the
computed-key formula (`perf/vcds-setup`), verification only.

**Store letters.** `IV_…_A*` holds `[ADP]`, `_G` `[GES]`, `_S` `[SOT]`, `_F` `[FFMUX]`, `_X`
`[XPL]` (as `_M` `[MWB]`, `_D` `[DTC]`): 51/51 stores of the car that opened.
`registry::list_in` reads every section from the **one** file the unit's `[MWB]` was found in:
searching the other candidates took the brake unit's freeze frames from `_032` for `_036`.

### 9.1 `RA.rod [ADP]` — adaptation channels (82,361 rows, 7,425 keys; 15 fields)

| field | meaning | evidence |
|---|---|---|
| f0 | the field's 1-based place among one DID's fields; empty for a one-field DID | car: 446/446 one-row DIDs empty; 108/108 several-row DIDs all set, rising with the bit offset |
| f1 | the DID `0x22` reads | ODIS adaptation DIDs present in the units' lists: **421/426** |
| f2–f11 | RM's value description (table, flags, offset, num, den, unit, byte, bit, length, name) | ODIS channels with a same-DID row: layout agrees 1666/1679; layout + scaling **1489/1679** (control, another row of the same key: 423/1337); 162 of the rest are VCDS type 7 (raw bytes) where ODIS scales |
| key | the `TTTEXT` record of the channel (its tail = the ODX `IDE`/`MAS` id) | gearbox `04FC` key IDE02332 = ODIS IDE02332; steering `1920` IDE04130 = ODIS |
| f12 | **unknown** — numbers as text (`0`, `1`, `100`, `0x0`, `26.0`); looks like a default value, unchecked | — |
| f13 | **unknown** 6-hex; first byte `40` in 82,355/82,355 (RM: `80`/`90`/`A0`/`B0`; RG `20`; RS `10`) | — |
| f14 | **unknown** 4-hex, as RM's f12 | — |

Per car unit (rows stored / ODIS DIDs found / ODIS channels whole): ACClima 110 / 52 of 52 /
106 of 108; airbag 90 / 2 of 2 / 8 of 9; brake 45 / 44 of 44 / 45 of 45; doors 8 / 8 of 8 /
2 of 8 each (4 raw); cluster 336 / 71 of 71 / 119 of 254 (135 raw); engine 20 / 18 of 18 /
18 of 20; EPB 47 / 29 of 29 / 46 of 47; gateway 677 / 47 of 47 / 151 of 157; infotainment 803 /
39 of 43 / 785 of 810; OCU 227 / 84 of 84 / 189 of 194; SMLS 62 / 9 of 9 / 9 of 9; steering 11 /
8 of 8 / 8 of 8; gearbox 2 / 2 of 3 / 1 of 2 (its TEA list lost its first row). **2,446 rows.**

### 9.2 `RG.rod [GES]` — basic settings (9,759 rows, 3,312 keys; 14 fields)

RA's layout without f14. f1 is the **routine id** (`0x31 01 <RID>`): ODIS routine-result RIDs
found in the units' lists **74/76** outside the engine (the engine's ODIS table has 560 RIDs, its
VCDS list 52, 50 of them in ODIS). The two misses are the first rows of TEA lists (EPB `0366`,
OCU `0317`), which no reader can recover. The key's tail is ODIS's text id (OCU `03E7` IDE02325,
`0317` IDE00498). Type 10 (new) puts a **constant** in the table field (`0x040000`, the
routine-control option record) — kept as written. Type 39 (new): a routine with no layout.
f0 ordinal: 152/152 one-row RIDs empty, 10/10 several-row rising. f12, f13 unknown.
**185 rows** on the car.

### 9.3 `RS.rod [SOT]` — output tests (11,165 rows, 1,868 keys; 14 fields)

RG's layout. f1 is the **input/output DID** (`0x2F <DID>`): the infotainment unit, the one whose
ODIS IO-control service reads with this chain, **13/13**; the key's tail is ODIS's text id
(`016E` IDE01627, `029C` IDE02060, `805F` IDE09544, …). Rows of one DID are the fields of its
control record (control parameter as type 10 `0x03FF`, then "Actuation time", "Start parameter").
These DIDs are not 0x22 measurement DIDs: 4/127 are (control 1/127). f0 ordinal: 15/15, 114/114.
f12 looks like a default value — unknown. **360 rows** on the car.

### 9.4 `RF.rod [FFMUX]` — freeze-frame fields (23,081 rows, 131 keys; 11 fields)

`<f0>,` then RM's value description; no DID field.

- **f0 = 100 + the record's length in bytes**: 917/1001 runs of one (key, f0) end exactly there
  (control, f0 shuffled across runs: 26/1001); on the car 41/52 records.
- **The key is not a name record**: keys are 3–147 plus `999` and the sentinel `999999` (last
  row, one field). Whether it is the `0x19 06` extended-data record number is **not
  established** — all keys but the two sentinels fit a byte; ODIS keeps these layouts in
  `MUX_DTCExtenDataRecor` DOPs that no reader here parses. Names come from f10 alone.
- The names match ODIS's structures by sense: SMLS `StrucExtenFreezFrameValeo1/2/3`
  (`Ubat30Volta`, `IgnitState`, `AD1`–`AD3`, `LINTemp`, `Isens`, `PWM`, `Kp`, `Ki`, …) against
  VCDS's "Ubat 30 Voltage", "Ignition State", "A/D #1"…, "LIN_Temp", "Isense", "PWM", "kp". Names
  only, not a field-by-field check.
- How a fault (RD) picks its record is **not found**: RD's 7 fields name no RF key.

**217 rows** on the car.

### 9.5 `RX.rod [XPL]` — named bits (22,350 rows, 7,131 keys; 13 fields)

`<ordinal>,<empty>,` then the value description with f11 (name) empty in 22,303/22,303 rows:
the key is the name record. Every list opens with a one-field header row (key `999999`,
payload `2`; 47 such rows). Mostly 1-bit text tables at byte/bit positions. The names are coding
options ("Lane departure warning", "Counter steer support", "Ignition Switch Key Lock
Solenoid"), so these are most likely the bits of the unit's coding — **not verified**: ODIS's
coding structure does not read through the measurement chain. Reading a coding is `0x22`;
writing one never happens here. **568 rows** on the car.

### 9.6 `MUX.rod [MUX]` — multiplexer cases (42,431 rows, 11,221 keys; 17 fields)

Until now `setup` copied `MUX.rod` into the pool (`SHARED_ROD_FILES`) for its key and nothing
parsed it. It holds the cases of a value of **type 6**: an RM, RF or RA row of type 6 names a MUX
key in its table field — RM 23,547/23,547 rows name an existing key, and the key's selector width
and case width equal the row's bit length in 21,193/22,947 (control, a random key: 9,657); RF
2,053/2,329 (control 915).

| field | meaning |
|---|---|
| f0–f3 | **unknown** (f0 mostly 0, 1, 8; f1, f2 always 0; f3 mostly 0) |
| f4 | the selector's width in bits |
| f5, f6 | the case's raw range, both ends included; `D,D` = the default case (one per key in 10,252 keys; none in 911) — 0 cases with lower > upper |
| f7–f16 | RM's value description for that case |

Stored per unit for the keys its listed rows name: **2,106 rows** on the car (one ACClima RF
row alone names a key of 327 cases).

### 9.7 Not done

- RF ↔ fault: which record a stored fault's environment data is read as.
- XPL: which bytes the bits describe.
- RA/RG/RS f12, every 6-hex and 4-hex field, MUX f0–f3: stored in `registry_row.raw` only.
- No reader. A cache from before the two tables is upgraded in place (schema version 3,
  `vag_data_db::migrate`): they are created empty, and the next registry read — `setup`, or
  `ensure` meeting a unit not read yet — fills them. Units already read are not re-read for it.
