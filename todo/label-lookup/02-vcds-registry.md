# label-lookup / 02 — measurements from a VCDS install: the `RM.rod` registry

**Subsystem:** label-lookup · **Crates:** `vag-data-labels` (`tttext`, `mwb`, a new registry
module, `dtc` for `INC`), `vag-data-db` (the `reading` rows), `vag-cli-diag` (`setup`,
`dev vcds`), `vag-cli-core` (resolution) · **Needs the car:** no — the gates run on the private
data under `~/.vagcan` · **Depends:** [`research/vcds-registry/README.md`](../../research/vcds-registry/README.md)

**State:** filed 2026-09-28, not started. Its own branch, reviewed before merge.

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
   `measure` finds its roles by name: check that it does on VCDS's names (`Vehicle speed`,
   `Engine speed`).

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
  - The 15 `IDE–ENG` pairs from the gearbox logs resolve.
  - **The same checks fail at 0-based indexing.** Neighbouring rows of one key are
    near-duplicates, so an off-by-one looks plausible: `check_gearbox.py` gets 12/12 at 1-based
    and 11/12 at 0-based.
- **A report against ODIS** on the reference car's units: every disagreement listed. This is not
  a gate.
- **The usual checks:** `cargo test --workspace`, clippy with `-D warnings`, `cargo fmt`, and the
  dead-code check.

## Decisions for the owner

1. **ODIS and VCDS disagree.** In the research, 70 of 1,033 joined rows name a different DID.
   In 46 of them the ODIS variant has VCDS's DID under another id; in 24 it lacks that DID.
   Which wins when both are set up? The default is ODIS, as `CLAUDE.md` orders the sources.
2. **The sign of seven proven rows.** VCDS and ODIS both say signed for `380A`, `380B`, `206E`,
   `38AC`, `38AD`, `38F6` and `38F9`; the proven rows say unsigned. A drive cannot tell the two
   apart on positive values. `22D2` is 9 bits in VCDS and was read as 16. Fix the rows under
   `~/.vagcan` (data, not code), or leave them?
3. **A Russian-only install** gives numbers without names or units, because its text tables are
   shifted (README §6a). Show the IDE id in their place, or say the install cannot name them?
4. **`calibrate`**, once this is merged. It is then needed only for:
   - shifted units;
   - units with no VCDS file, like this car's BCM;
   - measurements no list has, like the cluster's clock `2238`–`223C`.

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
