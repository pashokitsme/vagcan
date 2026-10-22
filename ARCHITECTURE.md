# Architecture

Why this tool is built the way it is. **This one is for the curious and for anyone
working on the code** — you do not need it to use `vagcan` (that is [`README.md`](README.md)).
The `research/…` files it links go deeper still, into the reverse-engineering: they are
developer notes, not instructions. The rules about what this tool may do to a car are
in [`CLAUDE.md`](CLAUDE.md).

---

## The one fact that shapes everything

**Names come from VCDS's label files. Scaling comes from ODIS, from VCDS's own registry, or
from a drive.**

Ross-Tech's VCDS ships 300 MB of label and ODX files, and it holds a great
deal: what every control unit is called, what its measuring blocks are called, which
fault codes exist and how they read in words. It **also** holds the join from a measurement to
its identifier, layout and scaling — in a global registry, `RM.rod`, indexed by each unit's
`MWB` row numbers (found 2026-09-28, [`research/vcds-registry/README.md`](research/vcds-registry/README.md)).
This corrects a long-standing conclusion in the archive that the scaling was "live-only"; that
was an off-by-one in reading `MWB` (its leading number is a registry **row number**, mistaken
for a name pointer). `setup` reads it for the units of every car recorded on this machine (step 5
below), and `watch`, `measure` or `units --identify` reads it for the car in front of it, the
first time (`dev dash build` reads it offline, for a car already recorded). About
40 % of VCDS's files are *shifted* — encrypted under a key only a running VCDS holds — and stay
closed; ODIS covers those.

**VW's own ODIS-Service data can too, and that is why it leads today.** An extracted ODIS project
declares, per control-unit variant, every identifier that unit answers together with the
byte offset, the length, the byte order and the compu formula — the whole chain, for
every variant the project covers, where VCDS's registry has it only for the units a
connected car answered and not in its shifted files. It is a declaration by the manufacturer rather than a
measurement, so it ranks below a drive and above nothing; three rows this project had
proved by driving came back identical out of the ODIS file, including a pair of
engine-speed channels with opposite byte order that one wrong guess would have hidden.

So there are four sources and they are not interchangeable:

| | comes from | rebuildable? |
|---|---|---|
| which identifiers a variant answers, their shape and scaling; fault codes and their text | an ODIS project, via `vagcan setup` | yes, in minutes |
| names, unit numbers, fault text | a VCDS installation, via `vagcan setup` | yes, in minutes |
| identifier, shape and scaling of a connected car's units | a VCDS installation's registry `RM.rod`, via `vagcan setup` and the first `watch`, `measure` or `units --identify` with the car on a VCDS run | yes, in minutes |
| `(identifier, raw form, factor, offset)` | measured on a vehicle | only by driving |

An ODIS project lands in a **project** — `~/.vagcan/data/<project id>/`, holding its
`cache.sqlite` and `names-odis.json`. A VCDS installation lands in the machine's **VCDS
store**, `~/.vagcan/vcds/`, holding `cache.sqlite`, `names.json`, `odx-ids.json`,
`rod-keys.json` and `registry.json` (which installation the channels were read from, and what
came of each unit), with the raw `.rod` files and the fault text in a shared `~/.vagcan/rod/`
because those are a property of a VCDS *build* rather than of any car. Rows measured on a car
land in the project's `measurements/`.

**Why the VCDS store is one per machine** (2026-10-21): an ODIS project describes one
platform, a VCDS installation describes every VAG car. Filed under a project, the same
installation needed a copy per platform, and a machine with VCDS alone needed a project named
for nothing. So the store holds one installation, `setup` asks no name for it, and every car
command resolves both — the car's ODIS project, when there is one, and the store
(`vag_cli_core::project::Stores`) — and reads one of them (below). Both caches have one
schema and one set of readers. VCDS rows an older vagcan wrote into a project's
cache move into the store the first time either is opened (`project::migrate_in`): the
installation read last is kept, its files and rows are copied and counted back, and only then
do the projects lose theirs — so a run stopped anywhere is finished by the next one. A move in
progress is marked beside the store (`.migrating.json`, with its record entry) and a project
being emptied beside itself (`.vcds-taken-out`); the next run finishes or undoes each, and
`migrated.json` records the move once.

**Which sources a store holds is the `source` table in its `cache.sqlite`, and nothing
else.** One row per folder read: kind, the directory its rows are keyed by (an ODIS
project's folder, a VCDS installation's `Labels`), the root `setup` was pointed at, the
language, version and time read. Every row of `fault`, `reading`, `measurement` and
`label_file` names its source. A `sources.json` beside the cache used to keep a second list;
the two disagreed (an entry deleted there left its rows here, still read), so it was folded
into the table by a schema migration (2026-10-20): read once into the rows it matches, then
removed. `vagcan setup --list` prints the table; `--drop` deletes a source and every row
that names it in one transaction, and the rules that rank one source over another then rank
what is left. One folder is one row: paths are canonicalised before they become a key, the
migration folds rows that name one directory and removes rows that own nothing, and a
relative path an older vagcan stored is never resolved against the working directory —
`--list` shows it as `stale`, and it counts as no source of its kind. Only `setup`'s label
step replaces one VCDS installation with another; `dev vcds labels` never writes the
store's cache, and a registry read writes only into a VCDS source that is still there.
`setup` fills the project or the store, and with a VCDS installation alone the car's channels arrive with the
first `watch`, `measure` or `units --identify` after it (or `dev dash build`, offline); the
messages a short tool prints say whether it lacks label data or scalings.

**A project is keyed by platform, not by car**, and that is the whole reason it is not
keyed by VIN: `SK37X` is VW's own identifier for a platform covering every Octavia III,
Karoq and Kodiaq, and a proven scaling is a property of a *part number*, true of every
car carrying that part. What is true of exactly one car — its car file, its drives, the record of
its units — is keyed by the VIN the car itself answers, under `~/.vagcan/cars/<VIN>/`.
[`docs/odis-project-mapping.md`](docs/odis-project-mapping.md)
transcribes which vehicles each of VW's project names covers; nothing in the tool reads
it, because a project declares its own coverage in `PRNR-INFO.xml`.

### One source per run

An ODIS project and a VCDS installation are **independent sources** (owner, 2026-10-21,
replacing 2026-09-28's "ODIS outranks VCDS, field by field"): each describes a car on its
own, and a run names everything — channels, scalings, fault text, freeze frames,
adaptations — from one of them. Nothing of one fills a gap in the other; what the run's
source does not describe is shown raw, or as its number, and says so.

**Which one** is decided once, by `vag_cli_core::source::run_source`: `--source odis|vcds`,
then `VAGCAN_SOURCE` (the firmware's build has no flag), then `[source] use` in
`config.toml`, then the ODIS project when the run's project holds one, else the VCDS store
when it holds an installation. A source asked for and not set up is an `error:` naming what
asked, with the `setup` to type — never answered from the other. Every car command refuses
it before it opens the adapter.

**How it holds:** the decision yields a `source::Run`, one store and the source it holds,
and every reader of label data is handed that and nothing else. `extracted::Extracted` is
opened over one `Run` and holds one cache path; the dash build's and the editor's inputs are
`Run::files`; the glossary seed and `--did`'s text ids ask `vag_data_db::text_ids_of` the
run's kind. Two guards keep it so: a test opens both sources and checks that no reader of a
run sees the other's channels, wording or ids, and a scan of the CLI crates refuses
`cache_of`, `open_from` or a both-caches accessor outside the code that owns the stores.

What that means per reader:

- **channels:** the run's source's rows alone, a proven row still in front (it is a
  measurement of this car, not a source). A VCDS installation's `names.json` names VCDS rows
  only; an ODIS row keeps its own name. The owner's `names.csv` is read on every run.
- **fault text:** the ODIS project's tables, or the VCDS chain — never one borrowing from the
  other; an ODIS row with a display code and no text shows the code alone.
  `[faults] language` picks among the run's source's own languages.
- **the VCDS registry read** (`registry::ensure`) runs only on a VCDS run, for every unit of
  the car the installation has not been tried for, ODIS-described or not; an ODIS run reads
  none. "Unread" is a VCDS-run question: the dash build refuses unread units only there.
- **the dash build** refuses a channel the run's source does not describe, with a note saying
  which source it read and, where the other is set up, the flag that builds from it.
- **the overview and `setup`** count and close on one source each: the overview the run's,
  `setup` each source it read, with a hint when car commands read another.
- **unit names and numbers** (`vag_cli_core::unit_names`, 2026-10-22): an ODIS run names a
  unit by the logical link that reaches its base variant (the variant from its `F19E`/`F1A2`)
  and numbers it by VW's table of logical links by diagnostic address
  (`DOP_TEXTTABLELogicLinkNames`), as `setup` cached them in `unit_name`; a VCDS run names and
  numbers it by the label file its `F187` resolves to. A unit the run's source does not name
  goes by its request id, and the command says how many. `units`, `faults --unit`, `sensors
  --unit`, `units --identify <unit>`, `adaptations` and `watch` take the same numbering.

The two sources write their names to separate files — VCDS to `names.json`, wholesale,
ODIS to `names-odis.json`, merged — so neither run overwrites the other's names.

**Fault text on an ODIS run comes from the project's own tables.** An
ODIS project carries, per ECU variant, a fault table (`DB_DOP_DTC`) mapping every
24-bit number the unit can send to a code object (`MCD_DB_DIAG_TROUBLE_CODE`) holding
the display code a tester prints and the text in the clear — 282,621 codes across the
reference project's 621 variants that have one. `setup` writes them into `cache.sqlite`'s
`fault` table keyed by variant and number, and `vagcan faults` names a code from the
variant the unit identifies itself as, by the same `F19E`/`F1A2` match the channels use.
On a VCDS run the VCDS chain below names the codes instead. The layouts and the evidence are
in [`research/odis-dtc/README.md`](research/odis-dtc/README.md).

A text is one per code, in the language its supplier wrote — the object model has no
language field and no translations, and the reference project's engine texts are English
inside a project that declares `deu`. So language is a property of the *source*: each
`source` row records what its source declared (an ODIS project's `<LANGUAGE>`, a VCDS
build's `Codes.dat` or `Code-RUS.dat`), a second project in another language is a second
source, and `[faults] language` in `config.toml` chooses among the run's source's languages
(`en`, `de`, `ru`, or the ISO 639-2 code a source declares). On an ODIS run, unset, English
wins wherever an ODIS source of the project declares it, otherwise the project's own; the
listing says which in one line. A VCDS installation has one build's language (read off its
own files in `~/.vagcan/rod`); a setting it does not have is said at `-v`.

---

## Data, not code

No measurement scaling, identifier number, unit name or part number is written in
Rust. Adding a parameter is a row in a JSON file, never a new `match` arm.

The rule exists because the tool must work on **any** VAG car and it was developed
against one. A constant that is true of a 2017 Octavia and written into a decoder is
indistinguishable, at the call site, from a constant that is true of the ISO
standard — until somebody points the tool at an Audi and gets confident nonsense. So
an offset or a magic number is a red flag: before writing one, establish whether it
is a property of the *protocol* (ISO/UDS/OBD-II, fine, cite the standard) or of *one
car* (not fine — it comes from the label files, or from a read).

### The catalog schema

One file per control unit, named for the part number that unit reports for itself
(`F187`) or its ODX file name (`F19E`), in the project's
`~/.vagcan/data/<project id>/measurements/`. A row:

```json
{"name":"Input shaft speed","unit":"/min","address":{"Uds":14346},
 "raw_form":"U16Le","scaling":{"Linear":{"factor":1.0,"offset":0.0}}}
```

`address` is the UDS `ReadDataByIdentifier` identifier — `14346` is `0x380A`.
`raw_form` says how to read the answer's bytes: `U8First`, `U8Second`, `U16Be`,
`U16Le`, `I16Be`. `scaling` is one of three, and the choice carries meaning:

- **`Linear`** — `value = raw × factor + offset`. A proven straight line.
- **`Enum`** — a discrete state, as `levels: [[raw, "what it means"], …]`. A gear or
  a selector position is **not a quantity**, and forcing one into `Linear` produces
  confident nonsense: on the reference car the gear code is `gear + 1`, so factor 1
  offset −1 reports the reverse code `0C` as "gear 11" and neutral as "gear −1",
  across a third of a recording. Anything not listed reads as unknown rather than
  being extrapolated. A level can also be a range, `[lower, upper, "what it means"]`,
  both ends included: a switch read as a voltage answers anywhere in its band. The
  first level that holds a value names it. An ODIS text table gives every level
  both ends; the cache keeps them (`reading_level.upper`).
- **`Anchor`** — one proven `(raw, value)` point and no slope. The honest state for a
  measurement where the zero is known and the scale is not; any other raw value is
  reported as unknown rather than guessed.

A car this tool has never seen finds no file and shows raw bytes. That is the
intended behaviour, not a gap.

### How a row gets proven

One route, a least-squares fit that accepts nothing under **R² 0.995 over ≥ 20 points
and ≥ 4 distinct raw values**. `vagcan dev sniff` records the bus listen-only while VCDS
runs an ordinary session beside it, and `vagcan dev vcds analyse` crosses that capture
with VCDS's own CSV export. The two files are aligned by wall-clock arithmetic — a
subtraction, never a search.

There was a second, `vagcan dev recording calibrate`, which fitted unproven columns of a
`watch --out` recording against trusted ones in the same recording. It was removed on
2026-09-28 (owner): with ODIS and VCDS's registry giving scalings through `setup`, nobody
was going to drive to make them.

---

## The file formats

Full writeups under [`.archive/research/labels/`](.archive/research/labels/).

**`.lbl` — plain text.** The old format, still shipped for older control units. One
file per part number, human readable, with a `; Component: … (#02)` header naming the
unit and its number, then measuring-block and field names. Nothing to crack.

**`.clb` — the encrypted `.lbl`.** Same content in a TEA-CBC container; decrypted
in-tool by `vag-data-labels`.

**`.rod` — the ODX container, and the interesting one.** Where modern (UDS-era) label
data lives. Each file is TEA-CBC encrypted with a per-record IV and the plaintext is
zlib-deflated. Inside are several tables:

| Table | What is in it |
|---|---|
| `STRUC` | measurement structures — 1,221 of them |
| `DOP` / `TTDOP` | computation methods and scaling — 17,636 entries |
| `TTTEXT` | the global text table: every name, in every language |
| `MWB` | the engine measuring-block rows |
| `[DTC]` | the fault-code table, in `RD.rod` |

Payloads are encoded in **base-14** over the charset `0123456789,.-_`, established by
disassembling VCDS rather than guessed at.

Five bytes of a section's first-block IV come from a `product` VCDS computes, and since
2026-10-20 that is known to be **the file name**: the name up to its first `.`, upper-cased,
`product = stem[0] · ∏ stem[i]` (`vag_data_labels::rod::name_product`). So a section opens
with no key and no search; over the English 26.3 `UDS_EV` that is 10,372 of 16,576 files,
every section of them. The brute force is kept for what the name does not give — a file under
another name than its own, and the masked files below — and its answers go in the project's
`rod-keys.json`, which the live path reads and never searches.

**Two fifths of the corpus are searchable in principle and not in practice, and the
reason is worth knowing.** Some files XOR an eight-byte mask over the *finished* IV,
read off VCDS's own code (`0x140033b70`) rather than guessed at. Three facts about that
mask decide everything:

- **It is the interface's answer.** A file is masked exactly when the number its `[CMP]`
  section starts with has eight digits (6,204 of 6,204 files; the six-digit ones never are).
  VCDS builds a 32-byte challenge from that number and the file name, sends it to the cable
  (command `0x45`) and takes the mask from the reply (2026-10-20,
  `.archive/research/labels/tttext2.md` §3.3b). The function is the hardware's: offline it
  cannot be computed, and measured from the outside it matches nothing structural.
- **It is eight bytes wide, so it reaches `IV[3..8]`.** Provable without cracking
  anything: byte 6 of a `<6-digit id>,<2-char code>` record is a comma, its multiplier
  is odd so the map inverts uniquely, and the value it yields is a property of the whole
  file — so two sections of one file must agree. Across the corpus the unmasked files
  agree 292 times out of 292, and the masked ones disagree 179 times out of 196.
- **It is skipped for the `CMP` tag** (467 of 467), and it is one mask per file, all eight
  bytes: the IV of one opened section XOR its name's IV is the mask, and it opens the rest of
  that file with no search.

The cost follows from the second point. An ordinary file's deflate anchor is free
(the tag-derived IV is exact) and its candidate bytes are reduced by the multiplicative
construction to about 6.9 × 10¹⁰. A masked file loses both: every one of 60 legal
anchors must be tried against the full 2⁴⁰ space, roughly 960× more work — hours per
file rather than seconds. Cracking every masked file in the corpus would take about
five years; the unmasked ones would take a weekend.

**This is why the Russian VCDS build recovers no measurement names.** `TTTEXT.ROD` is
unmasked and opens in minutes; `TTText-RUS.rod` is masked, so `vagcan setup` checks
before it starts and says so rather than spinning for a day. Fault text and labels are
unaffected — only the names are out of reach, and the one thing that would change that
is reading the mask out of a running VCDS, not out of the files.
[`.archive/research/labels/tttext2.md`](.archive/research/labels/tttext2.md) has the full argument.

**A control unit tells you which `.rod` is its own.** Identifier `F19E` returns an ODX
file name — `EV_ECM18TFS0208V0906264H`, say. That is how `vagcan dev vcds labels
--from-car` finds the right file with no lookup table in the middle. The rest of the name
follows VCDS's order: `<name>_<vvv>_<project>`, `<name>_<vvv>`, `<name>_<project>`, `<name>`,
with `vvv` from `F1A2` and the project from the VIN through `chassis.clb`; a name the
installation has no file of can be a `UDS_EV/ReDir.rod` source, which names the file VCDS
reads instead (or `EMPTY`: no data) (`vag_data_labels::label_files`, `redirect`, `chassis`).

**The ODIS fault chain**, which is asked first, is the measurement chain's shape with
two hops fewer:

```
raw 24-bit code, and the unit's F19E/F1A2
  → the variant's DB_LAYER_DATA            (dtc_properties: the fault tables' names)
  → its property index                     (name → the DB_DOP_DTC object)
  → the DB_DOP_DTC                         (number → the code object)
  → MCD_DB_DIAG_TROUBLE_CODE               (display code, text, level)
```

A variant that names no fault table of its own is read through the first parent layer
that does, as the measurement service is. The number is the join and the display code
is a separate string the object carries — on the reference project only 1,515 of 43,378
`(display, number)` pairs agree with the SAE encoding, so nothing derives one from the
other.

**`Codes.dat` — the fault-code text store**, the VCDS chain. A fault number does not
resolve to words directly. The chain is:

```
raw 24-bit code
  → the [DTC] table in UDS_EV/RD.rod        (which faults exist at all)
  → the row the unit's own .rod selects     (which of them this unit reports)
  → a key into Codes.dat                    (the text store)
  → the words
```

Each `RD.rod` table's digits are substituted under a per-table alphabet, and that
alphabet turned out to be *generated* from the table key by `srand(key)` and two
Fisher-Yates shuffles sharing one stream — read off the binary, not inferred. 95 of
95 alphabets, 219,490 of 219,490 name fields, zero wrong. See
[`.archive/research/labels/fault-naming-hop.md`](.archive/research/labels/fault-naming-hop.md).

**`TTTEXT.ROD` — the names.** Every record of its `[TXT]` section is `<id>,<payload>`,
the payload enciphered under the alphabet `srand(id)` generates — the same generator as
`RD.rod`'s tables, keyed by the record's own id. So every record reads by lookup, digits
included: `<name>,` or `<name>,<kind>,<value>`, where kind 2 is an `IDE` and kind 7 a
`MAS`. All 195,910 records of the 26.3 table read, and the names VCDS prints in its own
logs as `ENG######` come back verbatim. See
[`research/vcds-registry/README.md`](research/vcds-registry/README.md); the dictionary
solver this replaced is in
[`.archive/research/labels/tttext-codec.md`](.archive/research/labels/tttext-codec.md).

---

## What `vagcan setup` actually does

One command, two branches: an ODIS project into `~/.vagcan/data/<project id>/`, a VCDS
installation into `~/.vagcan/vcds/` and the shared `~/.vagcan/rod/`. Which branch runs is decided by what the source is, and the
source is recognised from the folder rather than declared: a `UDS_EV/` inside makes it a
VCDS installation, an `AStringData.data.gz` beside at least one `<pool>.key` makes it an
ODIS project. Nothing is opened to decide — being wrong in the permissive direction
costs a parser error that explains itself, and being wrong in the strict direction turns
a real project away at the door.

**The ODIS branch is two steps**: every variant's fault table and measurement chain
walked into `cache.sqlite` — the `fault` and `reading` tables, with the language the
project declares written on its `source` row — then every `(text id, name)` pair in
every pool merged into `names-odis.json`. A variant whose chain reaches a type this reader
declines to open, or one it has no loader for, costs itself and nothing else — the count
of what was skipped is reported rather than hidden, because a project describes hundreds
of units and one bad one must not cost the rest.

**The VCDS branch is the five steps below**, numbered as `setup` prints them.

**1. The raw files → the shared pool.** What makes the installation unnecessary to every
car command: fault naming reads `.rod` files off disk at run time, so those are copied
out, flat, into `~/.vagcan/rod/`. The `.lbl`/`.clb` files are deliberately *not* copied —
they are read once, in step 2, into `cache.sqlite`, and that cache is what survives of
them. Every car command runs without the installation; `setup` reads it in steps 1, 2 and
5, and `watch`, `measure`, `units --identify` and `dev dash build` read it for a unit not yet
read — so it is kept: a unit the tool meets later (swapped, updated, or asleep the first
time) is read from it.

**2. The label files → `cache.sqlite`.** Every `.lbl` parsed and every `.clb` decrypted
into a SQLite database keyed by part number, so a later lookup is milliseconds rather
than a re-parse of 300 MB. The cache records which directory it was built from — inside
itself, so it is one file that can say what it holds — because the freshness rule is an
mtime comparison and an mtime cannot tell "older than the label files" from "built from a
*different* set of label files", which matters as soon as somebody has both the English
and the Russian install.

**3. `TTTEXT.ROD` → `names.json` and `odx-ids.json`.** The `[TXT]` section is
decrypted and inflated, then every record is read under its own key: seconds, and
nothing is guessed or withheld. `names.json` maps each record's id to its name;
`odx-ids.json` maps it to the `IDE`/`MAS` id its tail names — the key by which a VCDS
text can join what ODIS and a `dash.toml` call the same measurement; nothing reads it
yet. A record of neither shape would be reported, not guessed at; the 26.3 table has
none.

**4. `RD.rod` and `MUX.rod` → `rod-keys.json`.** The label files-wide sections whose keys
every car needs. Per-unit files are deliberately not swept: there are over sixteen
thousand of them, which handful a given car needs is a question only that car can answer,
and a file under its own name needs no key cached. With keys from the file names this step
caches nothing on a genuine installation.

**5. `RM.rod` → the car's channels in `cache.sqlite`.** For every unit of every car recorded on
this machine (`~/.vagcan/cars/<VIN>/units.json`, written by `watch`, `measure` and
`units --identify`), the unit's file is found by its
`F19E`/`F1A2`, its `[MWB]` list — its own or the store its `[INC]` names — points at 1-based
rows of `RM.rod`, and each row gives the DID, bit layout and scaling, named from `TTTEXT`, with
units from `UNIT.ROD` and text tables from `TTDOP.rod`. Read from the installation being set
up, not from the pool: a list and the registry it points into mean something only together,
and the pool can hold two builds' files under one name. A section opens by its file name; a
classic one under another name is searched for once (minutes), a shifted one never. The units'
lists are found side by side on rayon's pool. With no car recorded the step is *not yet*, not a
gap: it says what to type. A table that does not open, a row that does not read, a list
that lost its first row — each is said, never left as a smaller number — and a unit the
installation has nothing for is not a gap when the ODIS project describes it. When the step
writes nothing (no car, no registry) it leaves no rows of an earlier installation behind.
The same read stores, from the same unit file, the unit's rows of VCDS's other registries —
adaptations (`RA.rod`), basic settings (`RG.rod`), output tests (`RS.rod`), freeze-frame fields
(`RF.rod`), `XPL` bits (`RX.rod`) and the `MUX.rod` cases they defer to — in `registry_row`
(`vag_data_labels::registries`); `vagcan adaptations` reads the `RA.rod` rows back, and basic
settings and output tests are names only (`0x31`/`0x2F` stay outside the allowlist). `faults`
reads the freeze-frame rows back: a fault's `19 06` answer splits into VW's two standard records
and the unit's own, each own record laid out by its `RF.rod` key — or, from an ODIS project
alone, by the variant's extended-data multiplexer, which `setup` stores in the same table under the
ODIS source, in the project's cache (the `RF.rod` rows are in the VCDS store's); a run reads one
of the two (`vag_cli_diag::freezeframe`,
`research/odis-dtc/freeze-frames.md`).
The same read runs from `watch`, `measure` and `units --identify` with the car, and from
`dev dash build` offline (`vag_cli_core::registry::ensure`), for the units of the car in front of
them that the installation has not been tried for — the first time, with a line, a spinner and a
line per unit — and the store's `registry.json` records the installation and what came of each
unit, so a unit VCDS has no file for is not searched for again. A store set up before that log
existed is read from the installation its VCDS source names, when that path is absolute and
still holds `UDS_EV`. The firmware's build never reads: a plan that uses a unit whose channels
have not been read is refused, and the refusal names `dev dash build` — or `setup`, when the
installation is no longer where `setup` read it. A recorded unit the plan does not use is a note,
never a refusal.

The rows sit in the VCDS store under the installation's own source, and the last
installation read replaces the one before, as its label files do. They are read on a VCDS
run alone (one source per run, above), a proven row in front of them; each row keeps the
ODX id it carries — no id is given up to the other source's fields, since the two are never
read together. A VCDS list comes from whichever of a family's platform files read first;
there is no sweep left for a list to widen (2026-09-28).

Step 1 copies only what is newer than the pool's copy, and step 2 is skipped when its cache
is newer than the label files; `--refresh` forces both. Step 4 searches only for keys not
cached, or cached from another build. Steps 3 and 5 run every time: the names are read in seconds and written only
when they come out different — a file time cannot tell the table read last time from
another build's — and the registry step reads whichever units the records now name, which
no file time can say.

---

## The crates

Three families and the product.

```
crates/
  uds/                     talking to a car — ISO-TP underneath, UDS over it
    vag-uds-transport        the transport trait: the seam every backend implements
    vag-uds-can              slcan USB-CAN backend, listen-only mode, ISO-TP sniffer
    vag-uds-client           UDS client, ISO-TP framing, unit addressing
    vag-uds-capture          capture and replay transport, so tests need no hardware
    vag-uds-sim              a virtual car on a pseudo-terminal: `dev sim`, `just e2e`
  data/                    somebody else's diagnostic files
    vag-data-labels          parsers and decoders (.lbl/.clb/.rod), ODX/ODIS resolution
    vag-data-db              SQLite cache over the label files
  dash/                    the OLED device, laptop side and board side both
    vag-dash-render          a Frame in, pixels out, on any embedded-graphics DrawTarget
    vag-dash-core            the board's own logic, tested here: value store, frame loop,
                             framebuffer, glass driver, settings record, fault count, an
                             exchange's waits, the exchange count, input queue — the
                             replay runs it too
    vag-dash-wasm            that renderer as WebAssembly, for `dev dash edit`'s page
    vag-dash-ble             the laptop's BLE client — scan, pick a device, open a NUS pipe
    vag-dash-cfg             `dashcfg`, which configures the device over that pipe
    vag-dash-fw              the firmware. Outside the workspace: no_std for riscv32imc
  cli/                     what a person runs, in four layers
    vag-cli-core             which car, what channels, how to poll, where the files are
    vag-cli-diag             reading a car, and the files that explain what it said
    vag-cli-measure          the stopwatch behind `vagcan measure`, on `core` alone
    vag-cli                  binary `vagcan` — the command surface and nothing else
```

**A crate's directory is its package name**, and the family it sits in is already spelled
inside that name. The repetition is deliberate: a path and a package name that differ are
two things to learn, and everything that reports one — cargo, rustc, a stack trace, a
grep — then has to be translated into the other. Binaries are free of it and named for what a person
types — `vag-cli` builds `vagcan`, `vag-dash-cfg` builds `dashcfg`, `vag-dash-fw` builds
`dash` — because the name in a manifest serves the tree and the name in a shell serves
the reader, and they are not the same audience.

The families are not layers, and the rule that places the binaries is worth stating
because it looks arbitrary until you see it: **a binary lives in its family when it has
exactly one.** `dashcfg` and the firmware serve only the device. `vagcan` consumes `uds/`
and `data/` both, so it belongs to neither, sits at the root, and takes no family prefix —
naming the product after one of its dependencies would be the same mistake as filing the
renderer under whichever crate happens to draw with it today.

`cli/` is layered where the other families are flat, and the layering is load-bearing.
`measure` — an acceleration stopwatch, a third of what used to be one crate — needs
twelve modules from `core` and **nothing** from diagnostics. That was measured before it
was moved, and the manifest keeps it so: `vag-cli-measure` names no `vag-cli-diag`, so the
compiler refuses the first symbol it would borrow. It is a library; `vagcan measure` is the
one way in, and a build can leave the stopwatch out (`--no-default-features`) without
touching a line of diagnostics.

**One owner of the link: the bus.** Every command that talks to the car gets a `Bus`
(`vag-cli-core/src/bus`), not the adapter. One task owns the adapter and runs the
scheduler (`vag_uds_client::schedule::Planner`): consumers subscribe to a `(unit,
identifier)` at a rate or read it once, the scheduler puts one request on the bus at a
time, merges due identifiers of one unit into one `22 d1 … dn`, keeps under 100
exchanges a second, and hands every answer to everyone who asked for it, stamped with
when it arrived. `watch` and `measure` subscribe; `info`, `units`, `faults`
use the `Bus` as an ordinary link, and each exchange queues in the same scheduler.
`dev sniff` alone opens the adapter bare, because it reads frames.

There is exactly one edge between families, `vag-uds-client -> vag-data-labels`, and it
exists for a single module: `read.rs`, decoding a measurement against a catalog. It goes
behind the `std` feature, because the board executes a plan with the scaling already
baked in and could not hold a catalog if it wanted to.

`vag-uds-client` cannot read a label file or an ODIS project — it is the protocol layer
and neither is a protocol — so the run's source's unit numbering is pushed *in* from
`vag_cli_core::unit_names`, and what crosses the seam is plain numbers and strings.

**Two addressing conventions are live on the same car.** ISO 15765-4 pairs
`0x7E0..0x7E7` with `+8`, so the engine answers `0x7E0 → 0x7E8`. VW's own block
answers at `+0x6A`, so the instrument cluster is `0x714 → 0x77E`. Assuming only the
first makes every unit outside the powertrain invisible, which is exactly what
happened before it was measured. Which CAN id a *unit number* is answered on is in no
data file this project reads — the label files and an ODIS project's table of logical
links carry the numbers and the names, not the CAN id — so that half is established by
reading the car (`vagcan units --identify`) or written down by hand.

**There is no whole-car sweep.** `dev survey` — which walked every unit and asked each the
identifiers its data declared, with a blind sweep on request — was removed on 2026-09-28
(owner). What it supplied is supplied otherwise: which units a car has, `watch`, `measure` and
`units --identify` record (`~/.vagcan/cars/<VIN>/units.json`), asking every unit again on every
run and merging what it answered into its entry field by field; which identifiers they answer, a
source `setup` reads declares. Three sweeps are left, each of one unit at a time: `units --identify
<unit>`, a unit's identification block (`F100`–`F1FF`); `sensors --unit <unit>` outside 7E0–7E7, the
standard OBD-II identifiers asked of a unit no regulation binds to them; and `adaptations`, a unit's
adaptation identifiers — exactly those an ODIS project's `ReadDataByIdentCalibData` service
(`adaptation_channel`) or, in a run of VCDS, its `RA.rod` lists for its variant, never the two mixed, read with
`0x22`; `--all` is a run of single-unit reads, each guarded on its own. All are refused on a moving
car and never run through the dash board; `anomaly::Monitor` watches the unit and ends the run the
moment it changes under the reads.

**The CLI is split by what a command needs.** The top level is for commands that need
a car in front of you, plus `setup`, which a new owner runs first. The workshop is under
`dev`: `dev recording …` reads drives this tool recorded, `dev vcds …` reads VCDS's own
files. A test (`the_top_level_is_only_what_needs_a_car`) keeps it that way.

**Read-only is enforced in the client, not by convention.** The UDS service allowlist
admits `0x22` (read data), `0x19` (read faults), `0x10` (session control) and `0x3E`
(tester present), and that is the whole of it. That is not the same as harmless:
"read-only" bounds what you can *change* about a car, not what you can *provoke*, and an
identifier sweep is a fuzz test of a control unit's diagnostic server.

**The virtual car is the far side of the cable, not a seam in the program.** `vag-uds-sim`
opens a pseudo-terminal and speaks slcan on it as a USB-CAN adapter does, with the control
units of a TOML profile behind it; a car command opens it through `--device` (or
`VAGCAN_DEVICE`) and runs its whole path unchanged — the serial port, slcan, ISO-TP with flow
control both ways, the UDS client and its allowlist, the bus task. The sim's ISO-TP is its own,
not the client's run backwards, so a defect shared by both ends cannot pass between them
unseen; the answer id comes from the client's addressing rule, so it cannot drift from where
the client listens. It answers whatever is asked, because what may be asked is the client's to
decide. The e2e cases (`tests/e2e/`, trycmd) run the installed binary against a made-up car per
directory and a made-up project; `docs/virtual-car.md` has the profile format.

---

## The dash

An ESP32-C3 with a CAN transceiver and an OLED, on the OBD port. Firmware in
`crates/dash/vag-dash-fw`, outside the workspace (`no_std`, `riscv32imc`). What it decides by
and needs no chip for is `vag-dash-core`, in the workspace, `no_std`: the firmware links it,
and its tests are the board's.

**The board executes a plan; it resolves no label data.** `build.rs` runs the same generator as
`vagcan dev dash build`: it reads `~/.vagcan/dash/<VIN>/dash.toml`, the car's record of its
units (`units.json`) and the project's cache, and writes a Rust `static` with every channel resolved — unit,
identifier, bit layout, scaling, unit, label. The image links it. A project cache is
~88 MB and the C3 has 400 KB of RAM, so nothing else could work; and a board holding a
fixed list of identifiers cannot sweep. What may be written in that file — channels and their
alarms, pages, a channel's specified value, the cruise lever (`[stalk]`), the stopwatch
(`[stopwatch]`), buttons on the board's pins (`[[button]]`) — is
[`docs/dash/dash-toml.md`](docs/dash/dash-toml.md).

**One bus, one conversation, on the board too.** `can_task` owns the TWAI controller and
runs every exchange through one scheduler, `vag_uds_client::schedule::Planner`, one at a
time. It reads each unit's part number (`F187`) first and subscribes to the unit's channels
only when it matches the plan: the visible page and every channel with an alarm at each channel's `hz` from `dash.toml`
(2 Hz by default), other pages at 1 Hz. `Plan::rates_in` sets the exceptions: the lever at
20 Hz while cruise and its switch both read off, 2 Hz otherwise, 10 Hz while the stopwatch is up with a factor; the
cruise status at 5 Hz while the lever's unit answers as the plan's; the stopwatch's speed at its `hz`, foreground on any
page, while the stopwatch is up with a factor, and as the board's timing channel (`Class::Timing`, ahead of a host's
reads) while the stopwatch is armed or running — unless a host already holds that channel. With the stopwatch page up, page
cells drop to at most 1 Hz; an alarm's page over it keeps its cells unless the stopwatch is armed or running.
A BLE host's requests go through the same planner.
The acceptance filter starts as the plan's answer ids and moves to an exchange's answer id
when the plan's does not pass it. Bus-off restarts the controller; a unit that leaves three
reads in a row unanswered is asked only for its part number until it answers. A unit that
refuses its part number, or answers it with no number (empty, padding only, not text), at boot
or after it matched, is asked again every 2 s and read once it names the planned part, never
dropped for the boot: neither names another part — a unit starting up may refuse with `22` or
answer blank, and a late refusal of one of its reads (`7F 22 <NRC>`) names no identifier, so it
answers the `22 F1 87` asked after it. Until it matches, `F187` is all the board asks it. Only a
unit that names another part number is not polled again. Each unit's check is
`vag_dash_core::unit_check::UnitCheck`, which the board and the host tests both run.

**The fault count: one protocol read beside the plan.** Once per boot, 10 s in and once a
plan unit has answered, the board reads the gateway's installation list (`22 2A26`) and asks
the engine, the gearbox, the gateway and every listed unit for its stored codes (`19 02 08`).
Stored and failing now are counted as `vagcan faults` counts them; the units differ in one
way: a listed id that shares a CAN id with a unit already asked is skipped, where the laptop
asks it. Only VW's block of the list is decoded, and a walk that would pass 64 units — the
listed ids and the three never listed, less those skipped or unaddressable — is refused.
Each request is one background exchange through the planner, so the panel and a host keep
their turns; each ends 2 s from its start, the send and any `78`s included, where every other
exchange keeps the board's 500 ms and 10 s. A unit still asking for time at 2 s is not
counted. None starts while the board's stopwatch is up or a host holds its timing
channel; the walk goes on where it stopped. The count is `vag_uds_client::faultcount`, the
board's shell round it `vag_dash_core::faults`, an exchange's waits `vag_dash_core::exchange`; all
three are pure and tested on the host. The panel draws a warning triangle and the number at
the status strip's right end, left of the link icons, inverted while a code is failing now.
`?` means no count: the gateway gave no list, the walk would pass 64 units, or no unit could
be counted — the USB log says which. `state` says `faults=9 failing=1 units=17/18`,
`faults=?`, or `faults=-` before the count ends, then `exch=…/s exch_ms=…`, the bus's own rate.

**Every exchange drops a late answer to another request.** An answer to another service,
identifier or sub-function, or a refusal naming another service, that arrives while an
exchange waits is dropped and the wait goes on — one rule on the board and the laptop,
`vag_uds_client::schedule::answers`; the sweep before each send removes only what came before
it, and ISO-TP ignores the consecutive frames of an answer nobody waits for, receiving and
sending. On the board, a unit heard from during an exchange — a `78`, or late answers only — that
does not answer in time is busy, not silent: its readers miss one sample, its part is not checked
again, and it is backed off as a silent unit is — a unit a run is timing only from its second
busy exchange in a row.

**Input is commands, from any mix of backends.** Buttons on GPIO 3, 4 and 5 (`[[button]]`),
the cruise lever (`[stalk]`) and `dashsim` each turn a press into a `Command` — next, previous,
stopwatch — in a small machine of `vag-dash-render` (`control`, `stalk`), so each is tested on
the host. Every command goes through one bounded queue to one task, which applies it through
`Screen::command`: the screen does not know which input it was, so a press means one thing
whatever was pressed. A full queue drops the newest and says so; no input waits on the settings,
and the bus task only offers the lever's command. The lever closing the stopwatch — cruise
taken, or its data missing over 3 s (`stalk::Closer`) — is not a command: the bus task applies it
through `Screen::close_stopwatch`, as the adapter screen ends the mode, so it never silences an
alarm, is never dropped by a full queue and never waits for the settings. The board's BOOT and
RESET buttons are not inputs; `GPIO9` is left to the ROM.

**An alarm is part of a channel** (owner, 2026-10-19): a channel's `alarm_max`, `alarm_min` and
drift keys, not a rule of its own. Each is one rule in the plan, raising the first values page
that shows its channel, else its chart page; `alarm_priority`, then the order in the file, is
the priority. The
board's alarm state is sized by the plan, so nothing counts the rules. What limits them is bus
time, and the build estimates it: the board's own planner run over the plan offline, each page on
the glass with cruise off and on and a stopwatch run, every exchange at one cost
(`dash/bus_time.rs`). A count of identifiers would be the wrong unit: the planner batches up to
eight of one unit in a request, so a page costs its units, not its channels. The estimate refuses
nothing (2026-10-20): its cost, 34 ms, is an upper bound from a BLE recording, and at it the
owner's plan, which runs on the car, came out short on every page. The board counts its own
exchanges (`vag_dash_core::bus_rate`, pure, tested on the host): `state` says `exch=…/s
exch_ms=…` over the last 10 s, the USB console the same by unit. A strict check is for after a
drive has given that number.

**Rendering is shared with the laptop.** `vag-dash-render` turns a `Frame` (a values page
of up to four cells, or a chart page) into pixels on any `embedded-graphics` target. On
the board that is a 1-bit framebuffer. The board sends it to the OLED — an SSD1322 over
SPI, four bits a pixel, widened row by row as it is sent, and only the rows that changed —
and over USB, where `dashsim` (`research/dash/host`) draws it in a terminal. The layout is
decided only on the board.

**A recorded drive on the panel, without the board.** `vagcan dev recording dash` runs a
`watch --out` recording through the board's own `Screen`, alarms, `Plan::rates` and
renderer, in the recording's time, and draws the panel in the terminal. The value store and
its staleness rule (`vag_dash_core::values`) and the frame — the screen's answer, the charts'
samples, the page composed (`vag_dash_core::frames`) — are one code, which the board's panel
task and `vag-cli-diag/src/dashreplay/engine.rs` both call; the replay adds only the bus,
reading at the board's rates what the recording holds. The lever, the
`[[button]]`s and the stopwatch are the exception: the replay does not replay them, and says so.
A recording holds no fault count, so the replay draws no badge.

**Pages laid out in a browser.** `vagcan dev dash edit` serves a page on `127.0.0.1` that draws
the panel with `vag-dash-render` compiled to WebAssembly (`vag-dash-wasm`). `vagcan` cannot
compile WebAssembly when it runs, so `vag-cli-diag`'s build script compiles it while `vagcan` is
built, with a second cargo, and embeds it — only when the `wasm32-unknown-unknown` target is
installed; otherwise the command says to install it and build again. A compile that fails is
tried again at the next build, and the command shows its error. CI's `test` job installs the
target and sets `VAGCAN_DASH_WASM_REQUIRED`, which makes either case a failed build. The channels it offers are
`watch`'s rows, each put through `dash::build` (`vag-cli-diag/src/dash/catalog.rs`): a row is a
number only when a `[[channel]]` naming it builds, to that row.

**The editor saves `dash.toml`** (`vag-cli-diag/src/dash/save.rs`). The page sends its state as
JSON, never TOML; the server checks every field, edits the file with `toml_edit` — the
`[[page]]` tables replaced where the first one was, a `[[channel]]` added after the last for a
channel new to the file, `label` changed, every other line kept — and builds
the plan from the new text through `dash::resolve`, the function `dev dash build` runs. Only a
plan that builds is written: `dash.toml` replaced in one step, then `plan.json` and `plan.rs`
by `dash::write_plan`, each in one step. `layout.json` is answered from the file at the moment of the request and
carries its fingerprint (length and FNV-1a of the bytes); a save whose fingerprint is not the
file's is refused, so a hand edit made meanwhile is never overwritten. `POST /save` is the one
request that writes, taken only with this server's `Host`, an `Origin` of this server, a
128-bit token the run made, and a JSON body within 1 MiB; `OPTIONS` is refused, so a
browser's cross-origin preflight fails. The token is in the fragment of the address the
command prints and opens (`/#t=<token>`): a browser never sends a fragment, so no answer of
the server holds it, and another process on the machine that asks for `layout.json` gets no
key. A `dash.toml` that is a link is saved where it points, with its permissions.

**BLE, always on.** The board advertises a Nordic UART service from boot and again after
every disconnect; no button, no pairing. `dashcfg` sends text commands (`state`,
`set brightness N`, `set page N`, `save`, `load`, `defaults`). Settings — brightness, the
active page, the stopwatch's last run — are stored in a flash partition with the id of the
plan they were saved against (`vag_cli_core::dash::plan_id`, a hash of the generated
`plan.rs`); the pages are the plan's. Saved against another plan, they are kept, and a page
the plan does not have becomes the first. The board names its plan's id in its Hello reply
and its `state` line. Framed UDS messages (`vag_uds_transport::link`) share the service: each request
passes the board's guard (`vag_uds_client::guard`) and then the planner; subscriptions are
polled on the board's clock. `vag_uds_client::remote` is that session, host-tested.

**The USB cable: the same link, and a second mode.** The cable carries the same framed
link as BLE, to a second session of its own beside the BLE one, held to
`Guard::cable()`: the allowlist, no `10 02`, the speed gate, the memory bounds — and no
rate cap or sweep rules, because a cable is trusted as a CANable is. The laptop tells the
`dash` image apart with a framed Hello before any slcan byte, then drives it with
`Bus::start_remote` exactly as over BLE. An slcan command line on the cable, with no link
session holding anything, switches the image to **adapter mode** (`vagcan --slcan`): the
standalone `slcan` image's bridge, `vag_dash_fw::slcan`, takes the pins; the planner sends
nothing and keeps its subscriptions; framed requests on either carrier are refused; the
panel shows `SLCAN` and counters. `C`, or the host's start-of-frame packets stopping,
ends it. `vag_uds_client::console` makes those choices, host-tested. One task writes
the cable, so no log line lands inside a frame.

**Firmware images.**

| image | what it is |
|---|---|
| `dash` | the display: plan, polling, panel, BLE settings; UDS over BLE and USB; adapter mode |
| `slcan` | the board as a LAWICEL slcan adapter over USB and nothing else. Answers `V` with `V0101` |
| `rxwatch` | listen-only frame counter (`--features ack` acknowledges, for the car) |
| `cantx`, `cantest`, `rxprobe` | bench tools that drive the bus; built only with `--features bench` |

`vagcan devices` asks an Espressif port for a Hello first and for `V` only when no Hello
came, so it never switches a `dash` image by asking.

---

## The repository

```
crates/         the Rust workspace — uds/, data/, dash/, cli/
docs/           reference for users (which cars an ODIS project covers)
research/       work in progress: dash/ (bench rig, hardware record), odis-dtc/, tuning/
.archive/       retired paths, kept as evidence — research/, specs/, tasks/
todo/           the detailed roadmap and open task files
```

**Nothing this tool reads at run time is in here.** The label data is Ross-Tech's and
cannot be redistributed; the measured rows are one owner's car. Both live under
`~/.vagcan/`.

**Nothing is deleted; things are moved.** Most of what this project knows was measured on
one car, once, and several of its most valuable documents record what did *not* work.
`.archive/` keeps them, so a refutation is not paid for twice.

Start here: [`README.md`](README.md) for features and the roadmap,
[`todo/README.md`](todo/README.md) for the detail, [`CLAUDE.md`](CLAUDE.md) for the
working rules.
