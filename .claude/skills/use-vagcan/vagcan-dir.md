# `~/.vagcan` — the map, and how to answer from it

Everything `vagcan` knows about cars lives in one directory (`VAGCAN_HOME` overrides it;
`crates/cli/vag-cli-core/src/datadir.rs` owns the layout). Most questions about a unit, a
channel, a fault code or an adaptation are answered from these files with `sqlite3`, `jq`
and `python3`, with no car and no `vagcan` run. This file is the map, so nobody has to
find the layout again.

## Rules for touching it

- **Read only.** Open every cache with `sqlite3 -readonly` (or `file:…?mode=ro`). Never
  write, `VACUUM`, or run a migration by hand: `vagcan` upgrades a cache in place when it
  opens one, and the owner's backups (`*.before-*`) are the only history.
- **Want to change something?** Change it with the command that owns it (`setup`,
  `dev dash edit`, `dev glossary`). If you have to edit a file by hand (`dash.toml`, a
  `measurements/` row), copy it to `<file>.before-<what>-<YYYY-MM-DD>` first.
- **One source per answer.** The ODIS project and the VCDS installation are two
  independent sources. Answer from one and say which. Never fill a gap in one with a row
  from the other. When both have it, give the two answers side by side.
- **The data is somebody else's.** Never copy rows, names or fault text from here into
  the repository.

## The tree

| Path | What it holds | Written by | How to read it |
|---|---|---|---|
| `config.toml` | `project`, `hz`, `[source] use`, the `watch` favourites by VIN, the names language | the owner, `watch` | text |
| `names.csv` | the owner's own channel names by text id: `text_id,en,ru,current` | `dev glossary` | CSV |
| `names-context.csv` | the owner's or an agent's notes; `vagcan` neither reads nor writes it | — | CSV, leave it alone |
| `cars/<VIN>/units.json` | the car's units as each named itself: `{vin, units:[{request, part_number (F187), odx_name (F19E), odx_version (F1A2), component (F197)}]}` | `units --identify`, `watch`, `measure` | `jq` |
| `cars/<VIN>/car.json` | mass, tyre, measured road load | `measure setup` | `jq` |
| `cars/<VIN>/measures/*.json` | one saved acceleration session per file | `measure` | `vagcan measure view` |
| `dash/<VIN>/dash.toml` | the dash's pages and channels | the owner, `dev dash edit` | text |
| `dash/<VIN>/plan.json`, `plan.rs` | what the firmware runs, built from `dash.toml` | `dev dash build` | generated: never edit |
| `dash/<VIN>/editor/` | the page `dev dash edit` serves | `dev dash edit` | generated |
| `data/<project>/cache.sqlite` | **the ODIS project**: channels, fault text, freeze frames, adaptations, unit names | `setup`, a lazy read | `sqlite3`, below |
| `data/<project>/names-odis.json` | ODIS text id (`IDE…`, `MAS…`, GUIDs) → name | `setup` | `jq` |
| `data/<project>/measurements/<part>.json` | **channels proven on the car**, by part number: `{defs:[{name, unit, address:{Uds:<did>}, raw_form, scaling}]}`, the DID in decimal (`8234` is `0x202A`) | a person, after a drive | `jq` |
| `vcds/cache.sqlite` | **the VCDS installation**: label files, channels from `RM.rod`, the other registries | `setup` | `sqlite3`, below |
| `vcds/names.json` | VCDS text id (`000003`) → name | `setup` | `jq` |
| `vcds/odx-ids.json` | VCDS text id → the `IDE…`/`MAS…` id it names | `setup` | `jq` |
| `vcds/registry.json` | `{installation, read_version, units:[{odx_name, odx_version, project, stem, registry_stem, outcome}]}`: which installation the channels came from, and each unit's `.rod` stem | `setup`, `units --identify` | `jq` |
| `vcds/rod-keys.json` | recovered `.rod` section keys | `setup`, `faults` | not data: leave it alone |
| `rod/` | the raw, encrypted VCDS files (`*.rod`, `Codes.dat`, `TTTEXT.ROD`, `UNIT.ROD`) | `setup` | `vagcan dev vcds rod`/`tttext` |
| `vendor/` | the VCDS installation `setup` downloaded | `setup` | — |
| `*.before-*` | backups from before a change | people and agents | history: never read as current |

`source.dir` in each cache names the folder it was read from. The ODIS project folder there
may still exist; reading it is `vag_data_labels::odis`'s job, not a grep's.

## The two caches

```bash
O=~/.vagcan/data/<project>/cache.sqlite   # ODIS (`ls ~/.vagcan/data`)
V=~/.vagcan/vcds/cache.sqlite             # VCDS
q() { sqlite3 -readonly -header -column "$@"; }
q "$O" "select id, kind, language, version, parsed_at, adaptations_at, units_at, dir from source"
```

`vagcan setup --list` says the same about both. `vagcan dev vcds stats` counts the VCDS cache's rows.

| Table | In | One row is | Index |
|---|---|---|---|
| `source` | both | one source: `kind` `odis`/`vcds`, `language` (ISO 639-2), `dir` | — |
| `reading` | both | one channel field of one variant: `did`, bits, sign, byte order, scaling | `(variant, did)` |
| `reading_level` | both | one text-table state of a `reading`: `raw`..`upper` inclusive → `meaning` | `(reading_id)` |
| `fault` | ODIS | one fault code of one variant: `code`, `display`, `text` | `(variant, code)` |
| `adaptation_channel`, `adaptation_level` | ODIS | adaptations, in `reading`'s shape | `(variant, did)` |
| `unit_name` | ODIS | a variant's unit name and number (`address`) | `(variant)` |
| `registry_row`, `registry_level` | VCDS | a unit's row of another registry, by `section` | `(variant, section, identifier)` |
| `label_file`, `measurement`, `redirect`, `adaptation`, `long_coding` | VCDS | the old `.lbl`/`.clb` label files: blocks and fields, not DIDs | `measurement(file_id, block, field)` |

`registry_row.section`: `ADP` adaptations (`RA.rod`, `identifier` = DID), `FFMUX` freeze-frame
fields (`RF.rod`), `MUX` cases of a multiplexed channel, `GES` basic settings and `SOT` output
tests (names only: `0x31` and `0x2F` are outside the allowlist for good), `XPL` named
coding bits. Column meanings are in the `CREATE TABLE` comments: `q "$V" .schema`.

**An empty `unit_name` or `adaptation_channel` in an ODIS cache means "not read yet", not
"none".** The first car command on `--source odis` that needs them reads them from the project
folder. Say they are unread; do not start a car command to fill them.

## Unit → variant: the join everything hangs on

Every channel, fault and adaptation row is keyed by `variant`, never by unit number. A unit's
variant comes from what it said about itself in `cars/<VIN>/units.json`: `odx_name` (F19E) and
the first three digits of `odx_version` (F1A2). The rule is `odx_match` in
`crates/data/vag-data-labels/src/label_files.rs`. Best rank first:

| Rank | Variant spelled | |
|---|---|---|
| 1 | `<odx_name>_<vvv>_<project>`, the car's project code: `registry.json`'s `.units[].project` (`SK37`), not the ODIS folder name in `data/` (`SK37X`) | VCDS stems carry one |
| 2 | `<odx_name>_<vvv>` | the usual ODIS match |
| 3 | `<odx_name>_<project>` | |
| 4 | `<odx_name>` | |
| 5 | `<odx_name>_<vvv>_<another tag>` | right version, another localisation |
| 6 | `<odx_name>_<anything else>` | **family only: a guess** |

Only the best rank that exists counts. A family-only match is the "variant not settled" case:
say the answer is a guess, as `vagcan` does. `adaptations` refuses it outright. `<odx_name>X`
is a different unit, not a variant.

```bash
# every variant of a unit (GLOB uses the index; LIKE scans every row)
q "$O" "select variant, count(*) rows from reading where variant = 'EV_Name' or variant glob 'EV_Name_*' group by variant"
```

## Recipes

Put the variant from the step above in `:v` (in the shell: `v=EV_…`, then `'$v'` inside the query).

```sql
-- a channel by identifier: same columns in both caches
select id, printf('%04X', did) did, name, unit, bit_offset, bit_length, signed, big_endian,
       scaling, factor, offset, anchor_raw, anchor_value, text_id
from reading where variant = :v and did = 0x202A;

-- its states, when scaling = 'enum'
select raw, upper, meaning from reading_level where reading_id = :id order by raw;

-- a channel by a word: a scan, about 0.3 s over 400 k rows
select printf('%04X', did) did, name, unit from reading
where variant = :v and name like '%boost%' order by did;

-- a fault code as `vagcan faults` printed it on an ODIS run: `display`, the file's own label
select printf('%06X', code) code, display, text, short_name, level, dop
from fault where variant = :v and display = 'U140100';
-- the same by the 24-bit number the car sent: `code`. Not an encoding of `display`:
-- in most rows the two do not agree, so look up by the one you were given
select printf('%06X', code) code, display, text from fault where variant = :v and code = 0xD40100;

-- ODIS adaptations (once read): same shape as reading
select printf('%04X', did) did, name, unit, scaling from adaptation_channel where variant = :v;

-- VCDS adaptations of a unit, by its stem
select printf('%04X', identifier) did, name, unit, kind, factor, offset, text_id
from registry_row where variant = :v and section = 'ADP' order by identifier;
```

How a `reading` row converts an answer: the field is `bit_length` bits from `bit_offset`,
counted after the `62 hi lo` header; `big_endian` and `signed` are stored, never assumed.

| `scaling` | Value |
|---|---|
| `linear` | `raw * factor + offset` |
| `enum` | the `reading_level` row with `raw <= raw_value <= upper`; `upper` NULL means one value wide |
| `anchor` | one proven point (`anchor_raw` → `anchor_value`) and no slope: any other raw is unknown |

`registry_row.kind` (VCDS's own type numbers): 0 linear, 2 identity, 3 text table (states in
`registry_level`), 6 multiplexed (cases are `MUX` rows, `table_id` the key), 7 raw bytes,
11 reserved.

## Names

- ODIS `reading.name` is what the project calls the field: English, German, or an internal
  short name (`CTR_IGK_CYC_RBM`), in one cache. Search in several spellings.
- `source.language` is what the source declares, not the language of every text in it: a
  `deu` project can hold English channel names and German fault text. Say the language of
  the text you quote.
- `text_id` keys a name across files: `names-odis.json` (ODIS ids), `vcds/names.json` (VCDS
  ids), `names.csv` (the owner's wording, which `vagcan` shows first).
- `vcds/odx-ids.json` maps a VCDS text id to the `IDE…`/`MAS…` id an ODIS row carries. A
  name found through that join is **a hypothesis**: say so, and say which file each half
  came from.

## What is not in any file

- **VCDS fault text.** A VCDS run names a fault at run time, through the unit's `.rod`,
  `RD.rod` and the encrypted `Codes.dat` in `rod/`. No cache holds the text, and no `vagcan`
  command names a code offline. With the car: `vagcan --source vcds faults`. Without it: a
  small program over `vag-cli-diag/src/faultnames.rs`, run with `VAGCAN_HOME` pointing at a
  copy of `vcds/`, `rod/`, `cars/` and `config.toml`, because opening the VCDS store can
  migrate it and write beside it.
- **Which identifiers a unit actually answers.** A source declares channels; the car may
  not answer them. Only `measurements/` holds rows proven on the car. Rank them
  above anything a cache says, and never present a cache row as proven.
- **Live values.** Those need the car: see `SKILL.md`.
