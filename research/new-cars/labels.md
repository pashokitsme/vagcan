# New cars — the label data (2026-09-28)

What in the **label data** stops `vagcan` from reading a car newer than the reference
Škoda (≈ 2020 onward: MQB-evo, MEB, MLB-evo, PPE …), and what fixes each. Transport and
protocol, and code that hard-codes this car, are covered by sibling notes, not here.

Evidence tags: **sourced** (URL or `file:line`), **measured here** (a command run on this
Mac, with its output), **inferred** (reasoning, not checked).

Status: COMPLETE. Written section by section (a lost session leaves what was found); §1–§3 done.

**One line.** The label-data blocker for newer cars is almost entirely one thing — VCDS ships the
files for 2017+ platforms in an encrypted "shifted" regime whose key is a VCDS runtime value that
is **not in the files and cannot be derived from them** — and the fix already exists in the
codebase: read those units from the car's own **ODIS project** through `vagcan setup`, exactly as
the reference car's file-less BCM is read today. What is left is per-platform ODIS coverage and
making the ODIS reader robust to a newer converter version.

## 0. Summary

| blocker | which cars | blocks reading? | confidence | solution | cost | needs a car? |
|---|---|---|---|---|---|---|
| VCDS shifted-`.rod` units (§1): mask is a VCDS runtime value, not in the files | every platform-specific unit of 2017+ platforms; ~half the evo-engine files; MEB/PPE 100 % | **yes** — loses both the measurement selection and the fault-name selection for the unit | high (census of 3 installs) | read those units from **ODIS** (§1.3a / §2); mask cannot be derived offline (§1.3b, confirmed) | owner obtains the ODIS project; no new code for the read | no |
| ODIS project is per-platform (§2): SK37X ≠ a newer car | all newer cars | no, **with** the right project | high | download the platform's ODIS project, `vagcan setup <path>` — already supported | owner-side download | no |
| Units with no VCDS file at all (§3.1), e.g. `EV_BCMMQB` | newest units on any platform | yes for the VCDS path | high (proven on ref car) | ODIS covers them (BCM: 12,305 rows in SK37X) | none beyond the project | no |
| ODIS reader is a **positional** stream RE'd on one converter (§2.3) | any newer ODIS project | **partly** — mismatch fails loud (`Stream::end`), does not misread | medium | import a newer project; extend reader for new terminator / degrade per-variant | bounded RE of one project if it fires; reuses `odis/` | no to import; yes to confirm |
| `GV_`/`INC` engine-list assembly not implemented (§3.3) | engines (all platforms) | partly | medium | read engines from ODIS (flattens it); build the assembler later for VCDS-only owners | moderate code | no |
| `ReDir.rod` global directory unused (§3.2) | minor / VCDS-only owners | no | low | optional: decode it to recover some file-less units without ODIS | one classic crack | no |

## 1. VCDS shifted-regime `.rod` files

### 1.1 Cause

A `.rod` section is TEA-CBC; its first-block IV is built from the section tag and a per-file
`product` term. In a **shifted** file VCDS XORs the finished IV with an 8-byte **runtime
global** — skipped for the tag `CMP` and when another runtime value is ≤ 999,999 — so every
section after `[CMP]` opens only with that mask (sourced: `.archive/research/labels/tttext2.md`
§3.3a, disassembly of `VCDS-arm64-unpacked.exe` at `0x140033b38`–`0x140033b84`). The mask is
not in the file (no slack bytes, `tttext2.md` §3.3), is redrawn per file (7,375 distinct
`D[0:2]` over 7,830 files, birthday-level collisions only, `tttext2.md` §11.1), and the
regime of a file is stable across releases (`research/vcds-registry/README.md` §6a). Opening
one shifted file by brute force costs ~20 min per deflate anchor, up to ~18–21 h per file
(`tttext2.md` §6.2b, §12) — per file, not per corpus.

What a shifted file costs `vagcan`: the unit's `MWB` row list (which `RM.rod` rows it reads —
DID, layout, scaling) and its other compressed sections are closed. `RM.rod`, `RD.rod`,
`TTTEXT.ROD`, `UNIT.ROD`, `STRUC`/`TTDOP`/`MUX`, `ReDir` — the global tables — are classic in
all three installs (sourced: `research/vcds-registry/README.md` §6a), so the registry itself
is readable; what is missing is each shifted unit's *selection* from it.

### 1.2 Which families are shifted, and does the share grow with model year

**Measured here** over `census_installs.json` (per install, per file, per section regime), EV_
files only. Scripts: `suffix.py`, `lines.py`, `eras.py`, `versions.py`, `engines.py`,
`words.py` (session scratchpad; each reads only `census_installs.json`). The platform suffix
is the last `_XXnn[P]` token of the file name (`_SK37`, `_VW38P`, `_AUE4`); it is VW's
project code (the `-PA` projects carry `P`), and the generation names below are VW's own, from
the S42 project mapping (sourced: `docs/odis-project-mapping.md`).

**Answer: yes, and it is a step, not a slope.** Inside each model line the generation
launched before ≈2016 is 0–20 % shifted; the one after is 95–100 % shifted, in all three
installs. EN 26.3 counts, RU (June 2024 data) in brackets where present:

| model line | predecessor → successor (shifted / EV_ files with that suffix) |
|---|---|
| Golf | `VW36` A6 Golf 1/232 (0 %) → `VW37` A7 Golf 61/549 (11 %) → `VW38` A8 Golf **117/117** [RU 181/183] → `VW38P` Golf PA, Tiguan NF, Tayron **43/43** |
| Octavia | `SK35` Octavia II 2/43 (5 %) → `SK37` Octavia III, Karoq, Kodiaq 25/370 (7 %) → `SK38` Octavia IV **34/34** [RU 40/40] → `SK38P` Octavia IV PA, Kodiaq NF **91/91** |
| A3 | `AU35` AB2 0/7 → `AU37` AB3, Q2, Q3 27/369 (7 %) → `AU38` AB4 **26/26** [RU 63/63] → `AU38P` AB4, Q3NF **75/75** |
| Leon | `SE35` 0/5 → `SE37` Leon III, Ateca 5/243 (2 %) → `SE38` Leon IV, Formentor **15/15** → `SE38P` **32/32** |
| Passat | `VW46` B6/B7 0/24 → `VW48` B8, Arteon 54/289 (19 %) → `VW49` B9 **49/49** |
| Superb | `SK46` Superb II 0/19 → `SK48` Superb III 28/177 (16 %) → `SK49` Superb IV **13/13** |
| A4/A5 | `AU48` B8 1/18 (6 %) → `AU49` B9, Q5NF 11/72 (15 %) → `AU40` B10 (A5), C9 (A6) **159/160** |
| A6/A7 | `AU56` C6 0/5 → `AU57` C7 0/45 → `AU58` C8, Q8, e-tron **100/100** |
| A8 | `AU64` D4 0/21 → `AU65` D5 **21/22** |
| Polo | `VW25` Polo A05 6/53 (11 %) → `VW27` Polo 7, T-Roc, T-Cross **316/317** |
| Fabia | `SK25` Fabia II 11/55 (20 %) → `SK26` Fabia III 3/43 (7 %) → `SK27` Fabia NF, Scala, Kamiq **100/100** |
| Ibiza | `SE25` 0/14 → `SE26` 0/41 → `SE27` Ibiza, Arona **36/36** |
| A1 | `AU21` A1, Q3 8U 0/24 → `AU27` A1NF **60/60** |
| Caddy | `VN35` Caddy 3/4 0/17 → `VN3S` Caddy 5 **55/55** |
| Transporter | `VN75` T6 0/13 → `VN46` T6PA **19/19** → `VN47` T7 **14/14** |
| Touareg | `VW52` Touareg II 0/18 → `VW53` Touareg NF **57/57** |
| Crafter | `VN54` Crafter NF **28/28** |
| MEB | `VWE3` ID.3/4/5 **108/108**, `SKE3` Enyaq/Elroq **85/85**, `AUE3` Q4 e-tron **79/79**, `SEE3` Born **76/76**, `VWE4` ID.7 **16/16**, `VNE4` ID. Buzz **13/13** |
| PPE | `AUE4` Q6 e-tron, A6 e-tron **141/142** |

One counter-example, which dates the step rather than the platform: `AU73` (Q7NF, MLB-evo,
launched 2015 — inferred) is 5/79 (6 %), while its MLB-evo siblings launched from 2017 (`AU65`
D5, `AU58` C8, `VW53` Touareg NF) are 95–100 %. Years are general knowledge, not sourced here;
the generation order is VW's.

Aggregated by era (EN 26.3; RU and EN 25.12 within ±7 points, `eras.py`):

| era (suffix groups, by S42 names) | shifted / EV_ files |
|---|---|
| pre-MQB — PQ25/26/35/46, MLB 1, up!, T6, Caddy 3/4 | 54 / 1,504 (3.6 %) |
| MQB / MLB-evo first wave — `VW37 SK37 AU37 SE37 VW48 SK48 AU49 AU73 VW41` | 327 / 2,366 (13.8 %) |
| MQB-A0, MLB-evo second wave, Crafter NF | 973 / 975 (99.8 %) |
| MQB-evo, MEB, PPE/PPC, Caddy 5, T6PA, T7 | 1,391 / 1,393 (99.9 %) |
| no platform suffix (shared units: engines, gearboxes, …) | 2,218 / 5,995 (37.0 %) |

**The shifted files on old platforms are the new unit generations fitted to them.** On `SK37`,
all 25 shifted files are later units — `GatewMQB2020Class`, `AuxilHeateWOSVW38X`,
`AirbaVW22/23TS6VW48X`, `BCMBOSCH_020…024`, `DashBoardJCIMQBAB_012`, `HREntryHellaGen3`,
`MUOIMQB`, `TrailFunctGener3Hella`; the same pattern holds on `VW37`, `AU37`, `VW48`, `SK48`.
Measured by generation word in the file name (all EV_ files, EN 26.3): `UNECE` 104/104,
`MEB` 168/168, `ICAS` 52/52, `PPE` 59/59, `MQB37W` 96/96, `MQB2020` 17/17, `VW38X` 128/128,
`Class` 10/10 — against `VW36X` 2/18, `PQ35` 2/18, `4G` 39/162, `MK100` 11/42.

**Engines follow the same line** (suffix-less `ECM` files, by the part-number prefix inside
the name): EA211 `04E·906` 110/752 (15 %) → EA211 evo `05E·906` 190/363 (52 %), `05C·906`
33/40 (82 %); EA288 `04L·906` 144/1,141 (13 %) → EA288 evo `05L·906` 195/406 (48 %); EA189
`03L·906` 0/568; EA888 gen 3 `06K·906` 0/50, `8V0·906` 0/38; Audi MLB-evo second wave
`4N0·907` 57/57, `4K0·907` 54/54, `4M8·907` 43/43, `760·907` 30/30 against `8W0·907` 0/76,
`4G0·907` 0/54. (Engine family names for the prefixes are inferred, not sourced.)

**The share rises release on release** because new files are mostly shifted: EV_ shifted
37 % (RU) → 39 % (EN 25.12) → 41 % (EN 26.3); of the EV_ files in EN 26.3 that the RU install
does not have, 55 % are shifted, against 32 % of those it shares with RU.

**Regime vs. version inside one family** (`versions.py`, EN 26.3, families = base name + suffix
with ≥ 1 `_NNN` file): 1,208 all-classic, 812 all-shifted, 86 mixed; of the 86, 65 are
monotone classic→shifted as `_NNN` grows (25.8 expected by chance, shuffled within family) and
13 go the other way. So the regime is chiefly a property of the unit's ODX family — which is
why it tracks the unit generation — with a weaker drift toward shifted over time. It is not a
clean date cut-over.

**Which sections a shifted file closes** (measured here, EN 26.3, EV_ + IV_ files that carry
the section; a plain-TEA section in a shifted file still reads except its first 8 bytes,
`tttext2.md` §6.2a):

| section | in shifted files: compressed (closed) / plain TEA (readable) | what it gives `vagcan` |
|---|---|---|
| `MWB` | 2,382 / 39 | the unit's measurement rows into `RM.rod` |
| `DTC` | 2,889 / 84 | the unit's fault rows into `RD.rod` — the VCDS fault-name path of `vagcan faults --labels` (`fault-naming-hop.md` §10, §12) |
| `INC` | 3,711 / 1,053 | the family references that lead to the `IV_…_M` store |
| `ADP` / `GES` / `FFMUX` | 2,081 / 160, 1,952 / 650, 447 / 502 | not used by `vagcan` today |

So a shifted unit loses **both** VCDS paths: measurements and fault names. The global
registries (`RM.rod`, `RD.rod`, `Codes.dat`) are classic, but without the unit's own row
selection they cannot say which rows are this unit's (`fault-naming-hop.md` §5.2, §10).

**What it means for a newer car:** every platform-specific file of a 2017+ platform is shifted,
and so is roughly half of the suffix-less engine files of the evo engines. With VCDS files
alone, a Golf 8 / Octavia IV / ID.x / Q4 / Q6 owner gets, for nearly every unit, neither a
measurement selection nor fault names — codes and raw bytes only — until the mask problem
(§1.3) is solved or ODIS covers the unit (§1.3 (a), §2).

### 1.3 Solutions

Three routes were weighed. Only one is offline and none is cheap; the choice below explains why
the project's own answer is ODIS, plus (long-term) `calibrate` on the reference car.

#### (b) Derive the mask offline — checked, and it is not possible

The archive already concluded the mask is a runtime value, filled outside the file and skipped
for the `CMP` tag, so it cannot be computed from the file (sourced:
`.archive/research/labels/tttext2.md` §3.3a, §6.2b, §8 item 6). To confirm nothing had changed
and that (b) is truly closed, a **short static look only** was taken at the archived binary
`.archive/research/clb-crack/bin/VCDS-arm64-unpacked.exe` (PE ARM64, ImageBase `0x140000000`)
with `r2` 6.2.0 — about 10 minutes of the 30-minute box, no crack attempted, no code produced.

**Measured here:** the value the IV routine (near the site the archive names) mixes in is loaded
from process state set up during a session, not from any field of the `.rod` file, which is what
the archive reported. **So the mask is not a function of the label data**, and no amount of
further work on the corpus recovers it. Route (b) is a dead end for offline label-data work,
and this note records it as closed rather than as a lead.

- Cost: none beyond the look already done. Needs a car: no.
- Verdict: **abandon.** Do not budget the 5–21 h/file brute force of `tttext2.md` §12 as a
  general fix either — it opens one file you have a reason to open, never the 7,830-file corpus.

#### (a) ODIS for the shifted units — the project's actual answer

ODIS-Service projects carry the whole (DID, layout, scaling, name, fault text) chain per variant
already, with no shift to defeat — that is exactly how the registry finding used ODIS as its
oracle (sourced: `research/vcds-registry/README.md` intro, §0). So a shifted unit that `vagcan`
cannot read from VCDS files it **can** read from the car's own ODIS project, imported through
`vagcan setup` (see §2.4). This is what the roadmap means by "drives on the car override";
`research/vcds-registry/README.md` §6 lists ODIS (or `calibrate`) as the route for every shifted
variant.

- Cost: the owner of the newer car must obtain that platform's ODIS project (§2). Code cost:
  none new — `setup` already reads ODIS; the parser gaps are §2.3.
- Needs a car: no for the data; the project is a file. (Confirming the read needs the car.)

#### (c) Capture masks from a running VCDS — possible, not done, not recommended

The mask exists in VCDS's process while it has a file open, so it could in principle be read
from a running instance (the owner has VCDS-RUS in a Windows VM). **Not attempted, and only its
shape is described here, as the task asked.** It would mean instrumenting the running program to
read the 8 mask bytes for each file as VCDS opens it, then pairing each with its file — one mask
per file, 7,830 files, and the mask changes every release (`tttext2.md` §11.1, §6a), so the
capture is invalidated by the next VCDS data update.

- Cost: high and recurring — a per-release capture campaign over thousands of files, plus the
  legal question of extracting Ross-Tech's runtime data wholesale (the same reason the label data
  is not shipped; CLAUDE.md data rule). Needs a car: no.
- Verdict: **not worth it against (a).** ODIS gives the same units without touching VCDS.

### 1.4 Not known

- Whether any shifted unit the reference car needs is *absent* from ODIS (the inverse of §3.1);
  for newer cars this is the open risk, since ODIS coverage per platform is not surveyed here.
- The exact per-model launch dates behind the §1.2 step; the generation *order* is VW's (S42),
  the years are general knowledge.

## 2. ODIS project coverage

### 2.1 What SK37X covers

**Measured here** (`~/.vagcan/data/SK37X/`, `sources.json` + `cache.sqlite`): the cached project
is ODIS-Service **SK37X**, VW-MCD Converter version **2610.2.688** (26.1.0), imported from
`~/Downloads/SK37X` on 2026-09-27, "717 variants, 230 pools". The cache holds 399,283 `reading`
rows over **669 variants** and 4,559 DIDs, 282,621 `fault` rows, plus adaptation/coding/label
tables. By S42 (sourced: `docs/odis-project-mapping.md`), SK37X is the **platform** covering
Octavia III, Karoq and Kodiaq (EU/RU/IN) — type codes `5E0 5EU 5EF 5EP 55A 55U` — not one car.

**SK37X covers what VCDS cannot, on this very car.** The reference Škoda's BCM reports ODX name
`EV_BCMMQB`, and **no VCDS install has any `EV_BCMMQB*` file** (0 in RU, EN 25.12, EN 26.3 —
measured here; `fault-naming-hop.md` §10.5 reached the same for its faults). ODIS SK37X carries
it in full: 17 `EV_BCMMQB_0NN` variants, **12,305 reading rows**. So the two sources are
complementary, and ODIS is strictly the larger one for a modern car. It also covers the 63
shifted SK37X variants that VCDS blocks (§1.2) — they are ODIS variants with `reachable: false`
only against the *VCDS* corpus (`scratch/out/plan.json`).

### 2.2 Which newer platforms need other projects

A project is one platform, so a newer car needs its **own** ODIS project — SK37X does not
describe it. From S42 (sourced: `docs/odis-project-mapping.md`), the successors an owner would
download instead:

| newer car | ODIS project(s) | note |
|---|---|---|
| Octavia IV, Kodiaq NF | `SK38X`, `SK38X-PA` | `-PA` = the AGT/PT prototype-plus project |
| Golf 8, Tiguan NF, Tayron | `VW38X`, `VW38X-PA` | MQB-evo / "MQB2020" |
| Passat B9, Superb IV | `VW49X`, `SK49X` | shares `MQB(W)_49x` |
| A3 8Y, Q3 F3 | `AU38X`, `AU38X-PA` | |
| A4/A5/A6/A7 (2023+) | `AU40X` | B10/C9 |
| Q5 NF, A6/A7 (8W) | `AU49X` | MLB-evo |
| A6/A7/Q8/e-tron | `AU58X` | |
| ID.3/4/5, Enyaq, Q4, Born, ID.7, ID. Buzz | `VWE31 SKE31 AUE31 SEE31 VWE41 VNE41` | MEB |
| Q6 e-tron, A6 e-tron | `AUE41` | PPE |
| (any MEB/PPE Audi/VW) | `MEB`, dedicated `E`-suffixed projects | |

The exact download an owner needs is keyed by their car's **type code**, matched against a
project's `PRNR-INFO.xml` — the same field `Project::vehicles` already parses (sourced:
`docs/odis-project-mapping.md` "How to read an entry"; `crates/data/vag-data-labels/src/odis/mod.rs:1113`).
S42 is a human reference; it is not read at runtime, and the type codes are **not proven
disjoint** across projects (sourced: `docs/odis-project-mapping.md` caution 2), so "one code →
one project" is an assumption until a second project is imported.

### 2.3 What the parser assumes, and what a newer project could break

The reader is `crates/data/vag-data-labels/src/odis/`. Read here. Its assumptions, and the risk each carries:

1. **The object stream is positional — fixed field order per type code, no tags, no lengths**
   (sourced: `crates/data/vag-data-labels/src/odis/object.rs:1`). The field orders were
   reverse-engineered against one MCD kernel and verified on **one** converter version (26.1.0,
   ODX 2.0.1; `.archive/research/labels/odis-crib.md` §2). **This is the main risk:** a newer
   project built by a different converter that reordered or inserted a field in a type on the
   read path would misparse silently — "read a field of the wrong width and everything after it
   is garbage that still parses" (sourced: `object.rs:1`).
   **The guard already exists and fails loud, not silent:** `Stream::end` requires the
   terminator to fall exactly where the fields stopped, so a shape mismatch is an `Error`, not a
   wrong number (sourced: `object.rs:1`, `odis-format.md` §8.2). Two terminator forms are
   already accepted (`23 3E 00`, `23 3C 00`); a third would need adding.
2. **Layer data is found by the generated name `LD_<variant>`**, with a full-pool scan as
   fallback if that misses (sourced: `crates/data/vag-data-labels/src/odis/mod.rs:876`). So a
   converter that changed the naming costs speed, not correctness — the scan still finds it. Open
   question whether `LD_` is universal or 26.1.0-only (`odis-format.md` §8.3).
3. **34 of 663 layer-data objects have a tail this reader cannot follow** and are kept
   head-only (sourced: `odis-format.md` §8.2). A newer project may have more or fewer; unknown
   shape.
4. **String pools are gzip-or-plain, Windows-1252 (A) / UTF-16LE (U)** (sourced:
   `odis-crib.md` §2). No version dependence seen.
5. **The refusal list is by type code** (flash, access-key, adaptation/coding CASE, session —
   sourced: `crates/data/vag-data-labels/src/odis/loaders/mod.rs:1`). If a newer converter
   **renumbered** type codes, a refused write-type could collide with a read-type number. Low
   probability (MCD codes are stable), but it is a safety-relevant assumption, so a new project
   should be checked against the known code table before being trusted.

Nothing in the parser hardcodes SK37X, a car, or a scaling — it resolves everything from the
project's own pools, which is the design (`odis/mod.rs:1`). The version-fragile part is purely
the ODX **container** shape, not car data.

### 2.4 Solution: a newer car's project in `vagcan setup`

Concrete steps for an owner of a newer car:

1. Obtain the ODIS-Service project for the car's platform (§2.2) as an **extracted** directory
   of `<PoolID>.db`/`.key` pairs + `AStringData.data`/`UStringData.data` + `PRNR-INFO.xml` +
   `DatabaseVersionInfo.txt` (the shape `Project::open` expects — sourced: `odis/mod.rs:1`,
   `odis-crib.md` §2). This is the same form SK37X was imported in.
2. `vagcan setup <path>` — already reads ODIS and caches it to
   `~/.vagcan/data/<project>/cache.sqlite` (sourced: `README.md:165`,`README.md:175`). No new
   command.
3. The car self-selects its variant at read time from what it reports (`F187` part number,
   `F19E` ODX name, `F1A2` variant), so nothing about the new platform is hardcoded (sourced:
   CLAUDE.md data rule; `fault-naming-hop.md` §10.4).

What the parser must handle for this to be safe on a new converter version, in priority order:
(i) **treat a `Stream::end` terminator mismatch as "skip this variant, report why", never a
crash** — verify the current behaviour degrades per-variant, since a whole modern project failing
to import is the likely first symptom; (ii) accept any **new terminator byte** a newer converter
emits; (iii) re-verify the **type-code table** (read vs refused) against the new project before
trusting a read; (iv) expect more **unfollowed layer tails** (§2.3 item 3). Cost: bounded RE of
one new project's object stream if (i)/(ii) fire — days, not weeks, and it reuses all of
`odis/`. **Needs a car: no** to import and parse; **yes** to confirm the variant selection reads
correctly end to end (a hardware checkpoint, per the workflow).

### 2.5 Not known

- Whether `SK38X`/`VW38X`/MEB/PPE projects parse cleanly with today's reader — **no newer
  project has been imported**; §2.3 is a code read, not a test. This is the single highest-value
  next step and it needs only a downloaded project, no car.
- Whether type codes are disjoint across projects (`docs/odis-project-mapping.md` caution 2) —
  decides whether variant→project selection can be automatic.
- Whether a newer converter renumbers type codes or reorders fields on the read path (§2.3
  items 1, 5).

## 3. Other label-data gaps

### 3.1 Units with no VCDS file

Separate from the shift: some units have **no `EV_<name>` file at all** in the VCDS corpus, so
even the classic path has nothing to open. Proven on the reference car: `EV_BCMMQB` (BCM) — 0
files in all three installs (measured here, §2.1) — and its faults were already unreachable for
the same reason (`fault-naming-hop.md` §10.5). A newer car meets this more often, because VCDS
adds unit files release by release and lags VW's own rollout (§1.2: the newest units are the ones
most likely missing or shifted).

**Solution:** the same as §1.3 (a) — ODIS carries these units (`EV_BCMMQB` is fully in SK37X),
so a unit missing from VCDS is not missing from the car's ODIS project. This is the strongest
single argument that ODIS, not the VCDS corpus, is the base for newer cars. Cost: none beyond
having the project. Needs a car: no.

### 3.2 `ReDir.rod`

**Measured here:** `ReDir.rod` is present and **classic** (a `[DIR]` section, ~1.27 MB inflated)
in all three installs — it opens with today's tooling, unlike the shifted files. It is a global
redirect directory (name → file). **It is not referenced anywhere in `crates/`** (measured here:
no hit for `ReDir` / `redir.rod` in the crate sources; the `REDIRECT` handling in
`crates/data/vag-data-labels/src/db.rs:2` is for the `.clb`/`.lbl` label files, a different
mechanism). 

Its relevance to newer cars is a **lead, not a fact**: a unit whose `F19E` name has no direct
`EV_<name>` file (§3.1) might be redirected by `ReDir.rod [DIR]` to a file that does exist —
which would be a VCDS-only way to recover some §3.1 units without ODIS. Unconfirmed here: it
was not decoded (it is compressed-classic, a ~150 CPU-s crack, out of scope for this read-only
pass). If pursued, it is cheap (one classic crack) and needs no car; but ODIS (§3.1) already
solves the same problem, so this is a nice-to-have, not the answer.

### 3.3 `GV_` files (grouped/engine variant files)

**Measured here:** 127 `GV_*` files in EN 26.3, mostly classic (118 classic, 9 shifted). They
carry `INC` (all 127), `MWB` (76), `DTC` (99), plus `ADP`/`GES`/`SLV`/`SOT`. They are how VCDS
assembles some units' channel lists — engines especially — through `INC` chains rather than one
flat `MWB` (sourced: `research/vcds-registry/README.md` §6 lists "how VCDS assembles the
`ECM00*`/`GV_` engine lists" among the **undecoded**). The SK37X project's engine variants that
matched no VCDS `EV_` file resolve instead to `GV_ECM…SK37X` names (measured here, §3 census).

Two gaps for a newer car: (i) the `GV_`→unit assembly is **not yet implemented** in the registry
approach (`README.md` §6.3), so even a classic `GV_` engine file is not read end to end today;
(ii) a `GV_` file's `INC` can itself be **shifted** (e.g. `GV_ParkiAssisUDS_004_VW37`, measured
here), inheriting §1's blocker. **Solution:** engines are best read from ODIS (which flattens the
variant to its readings — SK37X has the engine's rows), and the `GV_`/`INC` assembly is a VCDS-
only follow-up worth doing for VCDS-only owners but not the primary path. Cost: moderate RE +
code for the assembler; needs a car: no to decode, yes to confirm an engine reads.

### 3.4 Not known

- The contents of `ReDir.rod [DIR]` (not decoded here) and whether it recovers any §3.1 unit.
- The full `GV_`/`INC` engine-list assembly (`README.md` §6.3, §6 open items).
- Whether newer platforms introduce unit ODX names with **neither** a VCDS file **nor** an ODIS
  variant — the true dead zone. Not observed on the reference car (ODIS covered every unit that
  VCDS lacked), but not surveyed for `SK38X`/`VW38X`/MEB/PPE.

## 4. Sources

**In-repo (primary — this project's own code, data and research):**
- `research/vcds-registry/README.md` — the `RM.rod` registry; §6, §6a the shifted regime and the
  three-install comparison; the classic global tables.
- `.archive/research/labels/tttext2.md` — the shifted-IV regime: §3.3a (the mask is a VCDS
  runtime value, read off the disassembly), §6.2b/§12 (per-file crack cost), §11.1 (mask redrawn
  per file).
- `.archive/research/labels/fault-naming-hop.md` — §10 (a unit's `[DTC]` selects `RD.rod` rows),
  §10.4/§10.5 (file resolution; the file-less BCM), §12 (`vagcan faults --labels`).
- `.archive/research/labels/odis-crib.md` — §2, what an ODIS project is (converter 26.1.0,
  ODX 2.0.1, pool shapes).
- `.archive/research/labels/odis-format.md` — §8.2/§8.3, the parser's open questions and what a
  second project would test.
- `docs/odis-project-mapping.md` — VW's S42 project→vehicle table (165 projects); the platform
  codes in §2.2 and the two cautions.
- `crates/data/vag-data-labels/src/odis/` — the reader: `object.rs` (positional stream,
  `Stream::end`), `mod.rs` (layer-data naming, `PRNR-INFO.xml`), `loaders/mod.rs` (refusal list).
- `README.md` §Setup — `vagcan setup <path>`; `~/.vagcan/data/<project>/`.

**Data read here (not in the repo, per the CLAUDE.md data rule):**
- `research/vcds-registry/scratch/out/census_installs.json` — per-install, per-file, per-section
  regime for RU, EN 25.12, EN 26.3; the base for every §1.2 count.
- `research/vcds-registry/scratch/out/plan.json`, `match.json` — the 63 blocked and 93 unmatched
  SK37X variants.
- `~/.vagcan/data/SK37X/{cache.sqlite,sources.json}` — the imported ODIS project (§2.1).
- The three VCDS installs under `vendor/vcds-{en,ru}/UDS_EV` and `~/vcds-en/UDS_EV`.
- `.archive/research/clb-crack/bin/VCDS-arm64-unpacked.exe` — the archived binary; a short static
  look only, confirming §1.3(b) (no crack, no code produced).

**Analysis scripts written here** (session scratchpad, each reads only `census_installs.json`
unless noted): `suffix.py`, `lines.py`, `eras.py`, `versions.py`, `engines.py`, `words.py`,
`newfiles.py`, `dstable.py` (reads the three installs directly). Not committed; regenerable.
