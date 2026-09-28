# label-lookup / 02 — measurements from a VCDS install: the `RM.rod` registry

**Subsystem:** label-lookup · **Crates:** `vag-data-labels` (`tttext`, `mwb`, a new registry
module, `dtc` for `INC`), `vag-data-db` (the `reading` rows), `vag-cli-diag` (`setup`,
`dev vcds`), `vag-cli-core` (resolution) · **Needs the car:** no — the gates run on the private
data under `~/.vagcan` · **Depends:** [`research/vcds-registry/README.md`](../../../../research/vcds-registry/README.md)

**State:** built 2026-09-28 on `feat/vcds-registry-p2` (phases 1–4 and the acceptance below);
not merged, not driven. Phase 1 (`TTTEXT` read exactly) was done the same day on
`feat/vcds-registry`, which this branch continues.

## What the owner asked

- 2026-09-27, on `vagcan dev recording calibrate`: "I don't like calibrate at all, I propose to
  remove it entirely" — then "but VCDS works without any calibration; what did we fail to break
  in it?" and "wait, let's understand this before deleting".
- 2026-09-27: "we have the ODIS project now — can we project its mappings onto VCDS?"; "the
  names in ODIS differ, they may be German — search some other way"; "look at what ODIS and
  VCDS have for the same IDE".
- 2026-09-28: "let's form the implementation task in todo" — this file.

The research that answered it: VCDS keeps every measurement's DID, bit layout, scaling, unit and
name in the global registry `RM.rod`; a unit's `MWB` lists 1-based row numbers into it. Evidence,
controls and limits are in the research README; this file is the work order.

## Why

A VCDS-only owner (no ODIS project) gets channel names today and no `(DID, layout, scaling)`:
the scalings come from ODIS, from rows proven on a drive, or from `calibrate`. `CLAUDE.md` calls
a VCDS installation the fallback source; this task makes it one.

## The chain, per unit

```
F19E/F1A2 ──▶ its EV_ file: label_files::find_rod_by_odx_variant, first candidate that opens
          ──▶ its own [MWB], or [INC] ──▶ IV_EV_…_M<n>'s [MWB]
[MWB] row "n,code" ──▶ RM.rod [MWB], row n − 1 (1-based) ──▶ "key,payload", payload under srand(key)
payload ──▶ DID, layout, scaling ─┬▶ unit id ──▶ UNIT.ROD
                                 ├▶ enum table ──▶ TTDOP (a table under srand(table id))
                                 └▶ name id ──▶ TTTEXT
key ──▶ TTTEXT record "<name>,<kind>,<number>": kind 2 = IDE#####, kind 7 = MAS#####
```

`RM.rod` row fields after the key (README §1):

| field | meaning |
|---|---|
| f0 | DID, decimal |
| f1 | enum: the `TTDOP` table id; other types: a reference not yet decoded |
| f2 | `& 63` the type — 0 linear, 2 identity, 3 texttable, 4 OBD-II PID formula, 7 raw bytes, 8 ASCII; 1, 5, 6, 10, 12, 18–40 undecoded. `& 64` little-endian, `& 128` signed |
| f3, f4, f5 | value = (raw · f4 + f3) / f5 |
| f6 | unit id → `UNIT.ROD` |
| f7, f8, f9 | bit offset = f7 · 8 + f8; f9 = bit length |
| f10 | the name's text id — the `ENG######` VCDS logs print |
| f11, f12 | unknown |

This mirrors the fault chain already shipped (`vag-cli-diag/src/faultnames.rs`: `F19E/F1A2` →
the unit's `.rod [DTC]` → a row of `RD.rod`), and reuses its pieces: file choice, the key cache
(`rod-keys.json`), `UnitLookup`'s answers.

## Work, in order

1. **`TTTEXT` read exactly.** Each record decodes with `glyphs::TableAlphabet::for_key(record
   id)`, digits included; the tail `<kind>,<number>` parses into the IDE/MAS id.
   - The dictionary solver goes: two tools for one job is a defect. `vagcan dev vcds tttext` keeps
     its name and loses the vocabulary flags; its help says what it is for, what it reads and what
     it writes.
   - `names.json` is regenerated. 1,870 of 14,736 entries change, 685 of them letter misreads.
   - Its own commit: it fixes shipped names whatever happens to the rest.
2. **The registry reader** in `vag-data-labels`: sync and CPU-bound, rayon where it runs wide.
   - `RM.rod [MWB]` → rows. One classic crack per install, about 82 CPU-s; the key goes in
     `rod-keys.json`.
   - **`INC` followed** to the `IV_` stores by letter: M `MWB`, D `DTC`, A `ADP`, G `GES`, S `SOT`,
     F `FFMUX`. The fault chain stops here today (`UnitLookup::NoSection`), so this closes that
     gap for fault names too.
   - The first row of a TEA section is repaired, not dropped. Block 0 is damaged when
     `product ≠ 0`; the research's `inc.py` recovers it by trying the 1,000 keys the surviving
     digits allow and keeping the one that names an existing file.
   - `UNIT.ROD` gives the unit strings, `TTDOP` the enum levels, `TTTEXT` the names and the
     IDE/MAS ids.
   - Types 0/2/3/4/7/8 are decoded, with the byte-order and sign bits. Any other type is reported
     as undecoded, never guessed.
   - A shifted section is answered like the fault chain's `Locked`, with its own reason ("needs
     VCDS's runtime key"). A classic one is cracked on demand, a median of about 147 CPU-s per
     file.
   - `mwb.rs`'s module doc still calls the row number a text-id and is corrected with it.
3. **Into the cache** (`vag-data-db`). VCDS rows go into the `reading` table beside ODIS's, under
   the VCDS install's `source`.
   - The variant key must let `extracted.rs`'s `F19E`/`F1A2` pick find them.
   - Enum levels go into `reading_level`.
   - The IDE/MAS id goes into `text_id`, so a `dash.toml` reference such as `02:IDE00075`
     resolves from a VCDS-only setup too.
   - Precedence, as `CLAUDE.md` has it: car-proven rows, then ODIS, then VCDS.
4. **`setup`** opens the registry for the units the car reports, cracks what is classic, and names
   what it could not read and why: shifted, no file, undecoded type. The notes have the same
   shape as the fault chain's.
   - A `dev vcds` command prints one unit's rows, with the usual help (for, in, out). This is
     how the owner checks a unit by hand.
5. **Consumers.** `watch`, `measure` and `dev dash build` read the cache and need no new path.
   `measure` found its roles by name when this was written; it goes by text id since
   2026-09-28 ("Left after the build", item 2), so VCDS's rows reach it by the ids they carry.

## Must not

- **No car-specific data in code.** The file is chosen by `F19E`/`F1A2` through
  `find_rod_by_odx_variant`. The research preferred `_SK37` only because this project is a
  Škoda — that was the crib's rule, not a rule for the code.
- **No Ross-Tech data in the repository.** Tests build their own enciphered rows with the same
  generator. Tests on the private data skip when it is absent, as `need_rows!` does.
- **No guess on a shifted or undecoded row.**
- **Never mix installs.** A unit's `MWB` and the `RM.rod` it indexes come from the same install.
  Every release is re-encrypted, and its `RM.rod` differs in size (README §6a).

## Gates

- **Synthetic tests:**
  - the row parse;
  - (raw · f4 + f3) / f5;
  - the byte-order and sign bits;
  - `INC` routing by letter;
  - a shifted section refused;
  - **1-based indexing** — the fixture's neighbouring rows differ, so 0-based fails.
- **Private data**, skipped without it:
  - The 18 car-proven rows that VCDS lists — gearbox 12, engine 3, cluster 3 of its 8 — agree on
    DID, length, byte order, factor and offset, and enum levels.
  - The 13 `IDE–ENG` pairs from the gearbox logs resolve.
  - **The same checks fail at 0-based indexing.** Neighbouring rows of one key are
    near-duplicates, so an off-by-one looks plausible: `check_gearbox.py` gets 12/12 at 1-based
    and 11/12 at 0-based.
  - It needs `RM.rod`'s key, and prints `skipped:` and passes without it — a green run of
    `cargo test` does not say it ran. The owner's `rod-keys.json` gets that key from `vagcan
    setup` on the English install; until then point the test at a cache that has it and look
    for the `ok` with no `skipped:` line:

    ```sh
    VAGCAN_ROD_KEYS=<a rod-keys.json holding RM.rod's key> \
      cargo test -p vag-data-labels --lib the_reference_cars_proven_rows -- --nocapture
    ```
- **A report against ODIS** on the reference car's units: every disagreement listed. This is not
  a gate.
- **The usual checks:** `cargo test --workspace`, clippy with `-D warnings`, `cargo fmt`, and the
  dead-code check.

## The owner's decisions (asked and answered 2026-09-28)

1. **ODIS wins.** In the research, 70 of 1,033 joined rows name a different DID: in 46 the ODIS
   variant has VCDS's DID under another id, in 24 it lacks that DID. When both are set up, a
   channel ODIS describes is read ODIS's way; VCDS fills only what ODIS lacks — the order
   `CLAUDE.md` gives the sources.
2. **The seven proven rows become signed.** VCDS and ODIS both say signed for `380A`, `380B`,
   `206E`, `38AC`, `38AD`, `38F6` and `38F9`; the proven rows said unsigned, and a drive cannot
   tell the two apart on positive values. **Done 2026-09-28** in `~/.vagcan` (data, not code):
   `0CW300041G.json`'s six rows are `Int { byte_offset 0, byte_length 2, signed, little-endian }`,
   `8V0906264H.json`'s `206E` is `I16Be`; the files before sit beside them as
   `*.json.before-sign-2026-09-28`. `cargo test --workspace` then passed 1,954 tests, 0 failed
   (the proven-row tests run on this machine), and the owner's plan reads `206E` as `i BE`. `22D2`
   (9 bits in VCDS, read as 16 on the drive) was not part of the question and stays open.
3. **A Russian-only install falls back to English.** Its text and unit tables are shifted
   (README §6a), so its rows take their names and units from an English install on the same
   machine, by text id and unit id. Before relying on that, check that those ids agree between
   installs: compare the `RM.rod` rows of one unit in RU and EN. With no English install there
   is nothing to fall back to, and the row is named by its identifier: its IDE/MAS id comes
   from the same shifted text table — not asked, the only thing left.
4. **`calibrate` is deferred** — no decision until this task is merged. It is then needed only
   for:
   - shifted units;
   - units with no VCDS file, like this car's BCM;
   - measurements no list has, like the cluster's clock `2238`–`223C`.

   **Decided later the same day (owner, 2026-09-28): `calibrate` goes, entirely**, once this
   task is done — «давай полностью вырежем calibrate. Он скорее паразитный и никто этим
   заморачиваться не будет». Done the same day, in its own change (`feat/remove-calibrate`).

## Result (2026-09-28)

**The owner's `dash.toml` builds from a VCDS installation alone.** A project set up from the
English 26.3 install with no ODIS project and no proven rows (a scratch `HOME`) builds the same
plan as the ODIS project does: 19 channels on 4 units, 5 pages, 4 alarms, the stopwatch and the
cruise lever. Every channel has the same identifier, bit layout, sign, byte order, factor,
offset, unit, rate and ODX id. What differs:
- four channels are `declared` rather than `proven`, because that project had no proven rows;
- two automatic names — boost's specified value and the cruise control's status — are worded
  differently by VCDS and ODIS;
- two of the cruise switch's state names are worded differently too;
- the lever's rocker states are listed in another order, with the same bands behind `next`
  and `previous`.

**`setup`, step 5, on the owner's 15 units:**
- 5,314 channels for 14 units. The BCM has no file in any install checked.
- 857 registry rows are not channels (raw, text, a type not decoded).
- The first run took 2 min 40 s, most of it key searches, about 1,270 CPU-s. A second run
  takes 6 s.

**The gate on the private data** (`registry::tests::the_reference_cars_…`):
- The gearbox's 12 proven rows, whole, and 0-based numbering fails them.
- The engine's 3 through `[INC]`.
- The cluster's 3 listed rows. `22B8` is raw bytes in VCDS and `22D2` nine bits: the proven
  rows outrank both.
- The gear and selector text tables.
- The proven units.
- All 13 `IDE–ENG` pairs of this car's gearbox logs.

**Also changed on the way:**
- A cached `.rod` key is used only if it opens the section. Keys are cached by file name, and
  every VCDS release re-encrypts its files, so a key from 25.12 used to shut a 26.3 section for
  good.
- Type-4 registry rows (OBD-II mirrors) take SAE J1979's conversion from `crate::obd`.
- A channel's ODX id is its field's, from `f10`'s record, and not the whole identifier's.

## Left after the build

1. **Which platform file a unit reads.** It is the first readable in name order, so the gateway
   reads `EV_GatewNF_AU37`, not `_SK37`. Measured on the gateway and park assist, the brands'
   lists differ by 1–5 rows of the same identifiers. The fix is to choose by the car:
   `chassis.clb`, as VCDS does. (Choosing by the identifiers the unit answered in its survey
   is no longer possible: the survey was removed on 2026-09-28, and the car's record holds
   what each unit is, not which identifiers it answers.)
2. **`measure` finds its roles by name**, and VCDS words the gear, boost and shaft speeds
   differently from ODIS. With the owner's proven rows nothing changes. A VCDS-only owner with
   no drive gets speed, engine speed, pedal and a gear — the engine's `210F`, the unsettled
   channel `extracted.rs` warns about — but no boost and no shaft speeds. The fix is roles by
   ODX id, which ODIS and VCDS share.
   **Done 2026-09-28** on `feat/measure-roles-by-id` (owner: «да, давай пофиксим»). The rule
   (`vag-cli-measure/src/channels.rs`): every role lists its text ids, best first, and the
   physical units its consumers read it in; a row is a hit under one of the ids, or — a
   drive-proven row always, any other only when it carries no text id itself and no usable row
   on the car carries one of the ids — under the role's words, or, on an engine whose variant
   the project declares no OBD-II row for, as SAE J1979's own row at the PID the standard
   defines for the quantity (the prediction is offered to no other unit: gearboxes declare
   `F40C`, and the DSG ones `F40D`, in layouts of their own); a row in another unit of measure
   is not a hit. Speed ranks by step in m/s,
   then a drive-proven row, then a powertrain unit (ISO 15765-4's emissions addresses), then
   the request id; every other role by a drive-proven row, then a powertrain unit, then the
   id's position in the role's list, then the unit's own row before the J1979 prediction, then
   the request id. On the owner's project the leading speed moves from the BCM's `2B16` to the
   gearbox's `F40D` (owner: «да, переключи»); the gear and the pedal move to the gearbox's
   drive-proven `3816`/`3804` as a consequence of drive-proven-first; air mass resolves to
   nothing — the engine's known variant declares no row at PID 10 (the parked survey's `F400`
   bitmap has PID 10 clear), `13CD` has no id and a name no word matches, and `2037` is what
   both sources name a setpoint, following load in the 2026-09-26 capture (8–80 kg/h), so it
   is not an air-mass id. An emulation of an earlier form of the rule over a scratch VCDS-only
   project (2026-09-28,
   not reproduced by a test — the owner's cache holds no VCDS registry rows) gave that owner
   boost, both shaft speeds, the gearbox's gear and selector, and the engine's crankshaft speed
   instead of the cluster's.
3. **A Russian-only install** (decision 3, fall back to English names) is not built. Today its
   channels are named by their identifier in hex and carry no ODX id — the ODX id comes from
   the same shifted text table — so a `dash.toml` `unit:IDE…` reference cannot resolve there.
4. **Not built:**
   - the `dev vcds` command that prints one unit's rows;
   - `[INC]` for the fault chain (`UnitLookup::NoSection`).
5. **The shared pool can mix two builds** of one language: the copy is freshness-gated per
   file. Step 5 reads the installation itself, so it is not affected; the fault chain is.
6. **Keys are cached by file name.** A key from another build of the same name is tried,
   refused and searched again, so a machine that alternates two installs of one release
   searches each time. Caching by the section's own bytes would end that.
7. **A plain `[MWB]` list loses its first row** (said since the review): a nonzero `product`
   can spoil any of the row number's last three digits, and a spoiled digit still reads as a
   digit one time in twenty-five, so keeping a row that "looks intact" would sometimes attach
   another measurement to the unit. Keeping it needs the section's `product`, which a key search
   for plain sections would give; none exists. None of the reference car's lists is plain; 29
   of 103 plain lists in 26.3 keep a first row that reads as a number. A check exists if the
   owner wants those 29 rows (review, 2026-09-28): a list's two-glyph code is a function of the
   registry row (20,900 rows seen, each with exactly one code), and the code's second glyph
   lies outside the spoiled block — it matches the row's code in 29 of 29. With it, a wrong
   row drops from about one in 1,300 to about one in 5,000: not zero, so the owner's call.
8. **A lighter way to learn a car's units than a survey.** `units --identify` reads `F187`
   and `F197` and files nothing. If it read `F19E`/`F1A2` and filed them beside the car, setup
   could read a car's channels without the survey. **Done 2026-09-28**
   ([`03`](03-units-without-survey.md)): `watch`, `measure` and `units --identify` record the
   units under `cars/<VIN>/units.json`; `setup` reads the record, and the first of those three
   with the car — or `dev dash build`, offline — reads the channels of a car not yet read.
9. **The owner's decision: may a VCDS list widen what a sweep asks?** Until it is made,
   `declared_for_unit` joins the proven rows and ODIS alone, and `dev survey` asks what it
   asked before VCDS rows existed. Asking the VCDS lists would have taken the reference car
   from the proven rows' units to 2,067 identifiers with VCDS alone, and 120 more beside
   ODIS, 44 of them on the airbag unit — each one an identifier the unit's own VCDS list
   names, from whichever of the family's platform files read first. **Moot 2026-09-28**: the
   sweep went with `dev survey`, and `declared_for_unit` with it.

## Out of scope

The shifted regime (VCDS's runtime key), `TTTEXT2`, running or debugging VCDS, Russian
measurement names, and removing `calibrate`.

## Done when

- A `setup` with a VCDS install and no ODIS project gives `watch` and `dev dash build` the
  reference car's proven rows — DID, layout, scaling, unit, name — for its unshifted units.
- It names every unit and row it could not read, with the reason.
- `names.json` comes from the exact read.
- `README.md`'s roadmap item is ticked, `todo/README.md` is updated, and this file moves to
  `.archive/tasks/done/label-lookup/`.
