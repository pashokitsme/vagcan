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
for a name pointer). `setup` reads it for the units of every surveyed car (step 5 below). About
40 % of VCDS's files are *shifted* — encrypted under a key only a running VCDS holds — and stay
closed; ODIS covers those.

**VW's own ODIS-Service data can too, and that is why it leads today.** An extracted ODIS project
declares, per control-unit variant, every identifier that unit answers together with the
byte offset, the length, the byte order and the compu formula — the whole chain, for
every variant the project covers, where VCDS's registry has it only for the units a
surveyed car answered and not in its shifted files. It is a declaration by the manufacturer rather than a
measurement, so it ranks below a drive and above nothing; three rows this project had
proved by driving came back identical out of the ODIS file, including a pair of
engine-speed channels with opposite byte order that one wrong guess would have hidden.

So there are four sources and they are not interchangeable:

| | comes from | rebuildable? |
|---|---|---|
| which identifiers a variant answers, their shape and scaling; fault codes and their text | an ODIS project, via `vagcan setup` | yes, in minutes |
| names, unit numbers, fault text where the project has none | a VCDS installation, via `vagcan setup` | yes, in minutes |
| identifier, shape and scaling of a surveyed car's units, where ODIS has none | a VCDS installation's registry `RM.rod`, via `vagcan setup` | yes, in minutes |
| `(identifier, raw form, factor, offset)` | measured on a vehicle | only by driving |

The first three land in a **project** — `~/.vagcan/data/<project id>/`, holding
`cache.sqlite`, `names.json`, `odx-ids.json`, `rod-keys.json` and `sources.json`, with the raw `.rod`
files and the fault text in a shared `~/.vagcan/rod/` because those are a property of a
VCDS *build* rather than of any car. The last lands in that project's `measurements/`.
A tool short of one of them is in a completely different situation from a tool short of
the other, and the messages it prints say which.

**A project is keyed by platform, not by car**, and that is the whole reason it is not
keyed by VIN: `SK37X` is VW's own identifier for a platform covering every Octavia III,
Karoq and Kodiaq, and a proven scaling is a property of a *part number*, true of every
car carrying that part. What is true of exactly one car — its car file, its drives, its
survey — is keyed by the VIN the car itself answers, under `~/.vagcan/cars/<VIN>/`.
[`docs/odis-project-mapping.md`](docs/odis-project-mapping.md)
transcribes which vehicles each of VW's project names covers; nothing in the tool reads
it, because a project declares its own coverage in `PRNR-INFO.xml`.

### The two sources compose rather than compete

`setup`'s first menu entry offers both at once, and it is not a convenience. An ODIS
project names a channel the way a database does —
`Brake_pedal_information_plausibility` — and carries a **text id** beside it
(`MAS11563`, `IDE00030`). VCDS's `TTTEXT.ROD` names the same ids: a record's tail says
which `IDE`/`MAS` it is the text of. `names.json` is keyed by the record's own id
(`000080`), and `odx-ids.json` maps that id to the `IDE`/`MAS` it names. So one source
supplies which identifiers a variant answers, where the value sits in the reply and how
to scale it, and the other can supply the human wording for the very same ids. **That
join is written, not yet read:** a channel's name is looked up in `names.json` by the
row's `IDE`/`MAS` id, which never matches a record id, so with both sources a channel
still shows its ODIS phrasing. Neither source replaces the other, and a project holding
only the ODIS half is valid and useful — the channels simply keep their machine
phrasing.

The two sources write their names to separate files — VCDS to `names.json`, wholesale,
ODIS to `names-odis.json`, merged — so neither run overwrites the other's names.

**Fault text comes from the project first, and the VCDS chain is the fallback.** An
ODIS project carries, per ECU variant, a fault table (`DB_DOP_DTC`) mapping every
24-bit number the unit can send to a code object (`MCD_DB_DIAG_TROUBLE_CODE`) holding
the display code a tester prints and the text in the clear — 282,621 codes across the
reference project's 621 variants that have one. `setup` writes them into `cache.sqlite`'s
`fault` table keyed by variant and number, and `vagcan faults` names a code from the
variant the unit identifies itself as, by the same `F19E`/`F1A2` match the channels use.
The VCDS chain below answers for a unit the project has no table for. The layouts and the
evidence are in [`research/odis-dtc/README.md`](research/odis-dtc/README.md).

A text is one per code, in the language its supplier wrote — the object model has no
language field and no translations, and the reference project's engine texts are English
inside a project that declares `deu`. So language is a property of the *source*: each
`source` row records what its source declared (an ODIS project's `<LANGUAGE>`, a VCDS
build's `Codes.dat` or `Code-RUS.dat`), a second project in another language is a second
source, and `[faults] language` in `config.toml` chooses between sources.

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

A section whose `product` field is nonzero cannot be decrypted from the file alone:
five bytes of its first-block IV are missing and have to be searched for — about
fourteen seconds a section on a laptop. That is why the recovered keys are cached: the
live path reads the answer out of the project's `rod-keys.json` and never searches.

**Two fifths of the corpus are searchable in principle and not in practice, and the
reason is worth knowing.** Some files XOR an eight-byte mask over the *finished* IV,
read off VCDS's own code (`0x140033b70`) rather than guessed at. Three facts about that
mask decide everything:

- **It is a runtime global.** Not a field of the file, not a function of its name, not a
  checksum — a value filled elsewhere in VCDS's process. Measured from the outside it
  looks exactly like that: 348 distinct values across 349 such files, matching nothing
  structural. The files are simply not self-describing here; VCDS knows something they
  do not say.
- **It is eight bytes wide, so it reaches `IV[3..8]`.** Provable without cracking
  anything: byte 6 of a `<6-digit id>,<2-char code>` record is a comma, its multiplier
  is odd so the map inverts uniquely, and the value it yields is a property of the whole
  file — so two sections of one file must agree. Across the corpus the unmasked files
  agree 292 times out of 292, and the masked ones disagree 179 times out of 196.
- **It is skipped for the `CMP` tag** (467 of 467), and it is one mask per file — so
  opening any one section hands the rest of that file over for free.

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
--from-car` finds the right file with no lookup table in the middle.

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

One command, two branches, everything under `~/.vagcan/data/<project id>/` and the
shared `~/.vagcan/rod/`. Which branch runs is decided by what the source is, and the
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
5, so it is kept until the car it serves has been surveyed and `setup` has run again.

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
thousand of them, a blocked section costs about a minute of every core, and which
handful a given car needs is a question only that car can answer.

**5. `RM.rod` → the car's channels in `cache.sqlite`.** For every unit of every car this
machine has surveyed (`~/.vagcan/cars/<VIN>/survey.jsonl`), the unit's file is found by its
`F19E`/`F1A2`, its `[MWB]` list — its own or the store its `[INC]` names — points at 1-based
rows of `RM.rod`, and each row gives the DID, bit layout and scaling, named from `TTTEXT`, with
units from `UNIT.ROD` and text tables from `TTDOP.rod`. Read from the installation being set
up, not from the pool: a list and the registry it points into mean something only together,
and the pool can hold two builds' files under one name. A classic section's key is searched
for once (minutes); a shifted one never. With no car surveyed the step is *not yet*, not a
gap: it says what to type. A table that does not open, a row that does not read, a list
that lost its first row — each is said, never left as a smaller number — and a unit the
installation has nothing for is not a gap when the ODIS project describes it. When the step
writes nothing (no survey, no registry) it leaves no rows of an earlier installation behind.

The rows sit beside ODIS's under the installation's own source, and the last installation
read replaces the one before, as its label files do. **ODIS wins every field it describes
and VCDS fills only what ODIS lacks** (owner, 2026-09-28), and a proven row outranks both.
ODIS also keeps the ODX ids it gives: VCDS names a field by the record of the name it shows,
several fields can show one name, and a VCDS row filling a field ODIS lacks gives up an id
ODIS gives another field of the unit, so `unit:IDE…` picks out what it picked out with ODIS
alone. What a sweep asks a unit is still only what a drive proved and its ODIS variant
declares: a VCDS list comes from whichever of a family's platform files read first, and
whether it may widen a sweep is the owner's decision, not yet made.

Step 1 copies only what is newer than the pool's copy, and step 2 is skipped when its cache
is newer than the label files; `--refresh` forces both. Step 4 searches only for keys not
cached, or cached from another build. Steps 3 and 5 run every time: the names are read in seconds and written only
when they come out different — a file time cannot tell the table read last time from
another build's — and the registry step reads whichever units the surveys now name, which
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
  data/                    somebody else's diagnostic files
    vag-data-labels          parsers and decoders (.lbl/.clb/.rod), ODX/ODIS resolution
    vag-data-db              SQLite cache over the label files
  dash/                    the OLED device, laptop side and board side both
    vag-dash-render          a Frame in, pixels out, on any embedded-graphics DrawTarget
    vag-dash-ble             the laptop's BLE client — scan, pick a device, open a NUS pipe
    vag-dash-cfg             `dashcfg`, which configures the device over that pipe
    vag-dash-fw              the firmware. Outside the workspace: no_std for riscv32imc
  cli/                     what a person runs, in four layers
    vag-cli-core             which car, what channels, how to poll, where the files are
    vag-cli-diag             reading a car, and the files that explain what it said
    vag-cli-measure          binary `vagcan-measure` — the stopwatch, on `core` alone
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
was moved, and it is why `vagcan measure` and the standalone `vagcan-measure` can share
one set of flags and one `dispatch`, and why a build can leave the stopwatch out
(`--no-default-features`) without touching a line of diagnostics.

**One owner of the link: the bus.** Every command that talks to the car gets a `Bus`
(`vag-cli-core/src/bus`), not the adapter. One task owns the adapter and runs the
scheduler (`vag_uds_client::schedule::Planner`): consumers subscribe to a `(unit,
identifier)` at a rate or read it once, the scheduler puts one request on the bus at a
time, merges due identifiers of one unit into one `22 d1 … dn`, keeps under 100
exchanges a second, and hands every answer to everyone who asked for it, stamped with
when it arrived. `watch` and `measure` subscribe; `info`, `units`, `faults`, `survey`
use the `Bus` as an ordinary link, and each exchange queues in the same scheduler.
`dev sniff` alone opens the adapter bare, because it reads frames.

There is exactly one edge between families, `vag-uds-client -> vag-data-labels`, and it
exists for a single module: `read.rs`, decoding a measurement against a catalog. It goes
behind the `std` feature, because the board executes a plan with the scaling already
baked in and could not hold a catalog if it wanted to.

`vag-uds-client` cannot read a label file — it is the protocol layer and label files are
not a protocol — so the label files' unit numbering is pushed *in* from `vagcan`, and
what crosses the seam is plain numbers and strings.

**Two addressing conventions are live on the same car.** ISO 15765-4 pairs
`0x7E0..0x7E7` with `+8`, so the engine answers `0x7E0 → 0x7E8`. VW's own block
answers at `+0x6A`, so the instrument cluster is `0x714 → 0x77E`. Assuming only the
first makes every unit outside the powertrain invisible, which is exactly what
happened before it was measured. Which CAN id a *unit number* is answered on is in no
data file this project has found — the label files carry the numbers and the names and
no CAN id anywhere — so that half is established by reading the car (`vagcan units
--identify`) or written down by hand.

**A blind sweep is group testing, not 65,536 reads.** A multi-identifier request comes
back with only the identifiers the unit supports, and is refused outright when it
supports none of them — so one request is a presence test for a whole batch. That is
what turned a full sweep from hours into minutes.

It is no longer what `survey` does. A unit is asked only the identifiers a source says
that unit answers — the car reports `F187`/`F19E`/`F1A2`, that resolves to a variant,
and the variant declares its own list — and a unit nothing describes is identified and
has its faults read rather than being swept hardest of all. Blind sweeping survives as
`--blind`, aimed at units named one at a time; there is no spelling of any flag that
means "sweep the whole car blind", because that was the default and it turned one
unit's crash into a whole-car risk.

**The CLI is split by what a command needs.** The top level is for commands that need
a car in front of you, plus `setup`, which a new owner runs first. The workshop is under
`dev`: `dev recording …` reads drives this tool recorded, `dev vcds …` reads VCDS's own
files. A test (`the_top_level_is_only_what_needs_a_car`) keeps it that way.

**Read-only is enforced in the client, not by convention.** The UDS service allowlist
admits `0x22` (read data), `0x19` (read faults), `0x10` (session control) and `0x3E`
(tester present), and that is the whole of it. That is not the same as harmless:
"read-only" bounds what you can *change* about a car, not what you can *provoke*, and an
identifier sweep is a fuzz test of a control unit's diagnostic server.

---

## The dash

An ESP32-C3 with a CAN transceiver and an OLED, on the OBD port. Firmware in
`crates/dash/vag-dash-fw`, outside the workspace (`no_std`, `riscv32imc`).

**The board executes a plan; it resolves no label data.** `build.rs` runs the same generator as
`vagcan dev dash build`: it reads `~/.vagcan/dash/<VIN>/dash.toml`, the car's survey and
the project's cache, and writes a Rust `static` with every channel resolved — unit,
identifier, bit layout, scaling, unit, label. The image links it. A project cache is
~88 MB and the C3 has 400 KB of RAM, so nothing else could work; and a board holding a
fixed list of identifiers cannot sweep. What may be written in that file — channels, pages,
alarms, a channel's specified value, the cruise lever (`[stalk]`), the stopwatch
(`[stopwatch]`), buttons on the board's pins (`[[button]]`) — is
[`docs/dash/dash-toml.md`](docs/dash/dash-toml.md).

**One bus, one conversation, on the board too.** `can_task` owns the TWAI controller and
runs every exchange through one scheduler, `vag_uds_client::schedule::Planner`, one at a
time. It reads each unit's part number (`F187`) first and subscribes to the unit's channels
only when it matches the plan: the visible page and every alarm's channels at each channel's `hz` from `dash.toml`
(2 Hz by default), other pages at 1 Hz. `Plan::rates_in` sets the exceptions: the lever at
20 Hz while cruise and its switch both read off, 2 Hz otherwise, 10 Hz while the stopwatch is up with a factor; the
cruise status at 5 Hz while the lever's unit answers as the plan's; the stopwatch's speed at its `hz`, foreground on any
page, while the stopwatch is up with a factor, and as the board's timing channel (`Class::Timing`, ahead of a host's
reads) while the stopwatch is armed or running — unless a host already holds that channel. With the stopwatch page up, page
cells drop to at most 1 Hz; an alarm's page over it keeps its cells unless the stopwatch is armed or running.
A BLE host's requests go through the same planner.
The acceptance filter starts as the plan's answer ids and moves to an exchange's answer id
when the plan's does not pass it. Bus-off restarts the controller; a unit that goes silent
is asked only for its part number until it answers.

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
board's shell round it `vag_dash_fw::faults`, an exchange's waits `vag_dash_fw::exchange`; all
three are pure and tested on the host. The panel draws the number over a warning triangle in
the bottom-right corner, in the colours of the cell under it, inverted while a code is failing
now. `?` means no count: the gateway gave no list, the walk would pass 64 units, or no unit
could be counted — the USB log says which. `state` says `faults=9 failing=1 units=17/18`,
`faults=?`, or `faults=-` before the count ends.

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

**Rendering is shared with the laptop.** `vag-dash-render` turns a `Frame` (a values page
of up to four cells, or a chart page) into pixels on any `embedded-graphics` target. On
the board that is a 1-bit framebuffer; until the OLED is fitted, the board sends it over
USB and `dashsim` (`research/dash/host`) draws it in a terminal. The layout is decided
only on the board.

**A recorded drive on the panel, without the board.** `vagcan dev recording dash` runs a
`watch --out` recording through the board's own `Screen`, alarms, `Plan::rates` and
renderer, in the recording's time, and draws the panel in the terminal. The frame loop,
the value store's staleness rule and the cell composition are the firmware's
(`vag-dash-fw/src/bin/dash.rs`, not buildable on the host), mirrored in
`vag-cli-diag/src/dashreplay/engine.rs`; a change to one is a change to both. The lever, the
`[[button]]`s and the stopwatch are the exception: the replay does not replay them, and says so.
A recording holds no fault count, so the replay draws no badge.

**BLE, always on.** The board advertises a Nordic UART service from boot and again after
every disconnect; no button, no pairing. `dashcfg` sends text commands (`state`,
`set brightness N`, `set page N`, `save`, `load`, `defaults`). Settings are stored in a
flash partition; a stored page list that does not match the current plan is discarded at
boot. Framed UDS messages (`vag_uds_transport::link`) share the service: each request
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
