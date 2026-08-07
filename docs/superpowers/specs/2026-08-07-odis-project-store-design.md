# ODIS as a second data source, and a unified project store — design

**Goal:** read a VW ODIS-Service runtime project natively — no Python, no PBL DLL, no
Java/HSQLDB — and store what it and a VCDS installation both produce in one per-car
project directory, so `setup` becomes a choice of *source*, not a hardcoded VCDS path.

**Architecture:** a new `vag-data::odis` module ports the two open formats a VW ODIS
project is built from (Mission-Base's MIT-licensed PBL key-file library, and the ODX
object schema `ODIS-project-explorer` already reverse-engineered against it) into safe
Rust, read-only. `setup` gains an arrow-key source picker and writes into
`~/.vagcan/projects/<project_id>/`, a layout shared between a VCDS-derived and an
ODIS-derived parse rather than one per source.

**Tech Stack:** Rust edition 2024, `vag-data` (parsers), `vag-db` (SQLite cache),
`vagcan::setup` (CLI). No new external dependencies — the PBL port and the ODX object
loaders are hand-written Rust, not a wrapped C library.

## Global Constraints

- The UDS read-only allowlist stays `0x22`, `0x19`, `0x10`, `0x3E`. No object type whose
  only purpose is a write service (adaptation, coding, flashing, access-key handshake)
  is ever parsed into an executable path, regardless of how easy parsing it would be.
- Nothing this tool reads at run time lives in the checkout. Both a VCDS installation
  and an ODIS project are the vendor's, not redistributable, and stay under
  `~/.vagcan/`.
- `RUSTFLAGS="--force-warn dead_code" cargo check --workspace` stays clean throughout.
- Commit messages end with `Assisted-By:` + `Claude-Session:`, never
  `Co-Authored-By:`.
- No car-specific data in code. A scaling, a DID, a unit name — all of it comes from a
  source file (VCDS label file or ODIS project), never a hardcoded constant.

---

## 1. Why — the finding this design acts on

Two research writeups, both dated 2026-08-07, both under `research/labels/`:

- **`odis-crib.md`** — ODIS plaintext, read as a closed candidate list, breaks the
  `TTTEXT` substitution cipher (18,842 new names at 86.6 % measured precision) but
  gives nothing against the `.rod` container's TEA/deflate search.
- **The follow-up investigation** (to be written up as `research/labels/odis-format.md`
  per this spec's Task 1) — the `.db`/`.key` pair an ODIS project ships is **not
  encrypted at all**. It is a B+Tree index (Peter Graf's PBL library, MIT-licensed, C
  source available) over zlib-compressed ODX objects (a schema `ODIS-project-explorer`,
  a community tool, already reverse-engineered against a decompiled `MCD-Kernel`).
  Cross-checked against this project's own proven-on-car readings: `0x380A`
  (`Transmission_Input_Speed_Sensor`, `IDE00022`) comes back `compu_category: IDENTICAL`
  — exactly the `u16 LE, raw` scaling `research/labels/rod-labels.md:433` proved by
  driving, independently, before either side had seen the other.

That last point is the reason this is worth building: an ODIS project can hand over
**proven-shape scaling data with no drive required**, for every DID it defines, and the
cross-check says it is trustworthy where it has been checked. It is not a replacement
for a proven catalog — §5 below keeps the trust order the project already has — but it
turns "measure this car's every channel by driving it" into "measure the channels an
ODIS project does not already answer."

## 2. Scope — what `vag-data::odis` parses, and what it never will

The ODX object schema `ODIS-project-explorer` documents has 84 types. Most of them are
irrelevant to a read-only tool. Scope was set by walking every `vagcan` top-level
command and asking what UDS service it uses:

| for | vagcan commands | object types |
|---|---|---|
| **measurement chain** (service `0x22`) | `watch`, `scan`, `survey`, `measure`, `properties` | `MCD_DB_SERVICE`, `MCD_DB_DIAG_COM_PRIMITIVE`, `MCD_DB_REQUEST`, `MCD_DB_REQUEST_PARAMETERS`, `MCD_DB_RESPONSE`, `MCD_DB_RESPONSE_PARAMETERS`, `MCD_DB_PARAMETER`, `MCD_DB_PARAMETER_SIMPLE`, `MCD_DB_PARAMETER_STRUCTURE`, `MCD_DB_PARAMETER_STRUCT_FIELD`, `MCD_DB_PARAMETER_MULTIPLEXER`, `MCD_DB_PARAMETER_TABLESTRUCT`, `MCD_DB_PARAMETER_TABLE_KEY`, `DB_DOP_SIMPLE_BASE`, `DB_DIAG_CODED_TYPE`, `DB_PHYSICAL_TYPE`, `DB_LIMIT`, `DB_COMPU_METHOD`, `DB_COMPU_BASE`, `DB_COMPU_SCALE`, `DB_COMPU_SCALES`, `DB_COMPU_RATIONAL_COEFFS`, `MCD_DB_PHYSICAL_DIMENSION`, `MCD_DB_UNIT`, `MCD_DB_UNIT_GROUP` |
| **identification** | `setup`, `info` | `MCD_DB_ECU_VARIANT`, `MCD_DB_MATCHING_PATTERN`, `MCD_DB_MATCHING_PATTERNS`, `MCD_DB_MATCHING_PARAMETER`, `MCD_DB_MATCHING_PARAMETERS`, `MCD_DB_MATCHING_REQUEST_PARAMETER` |
| **fault codes** (service `0x19`) | `faults` | `DB_DOP_DTC`, `MCD_DB_DIAG_TROUBLE_CODE`, `MCD_DB_ENV_DATA_DESC`, `MCD_DB_PARAMETER_ENV_DATA`, `MCD_DB_PARAMETER_ENV_DATA_DESC` |
| **topology** | `units` | `MCD_DB_ECU`, `MCD_DB_ECU_BASE_VARIANT`, `MCD_DB_LOGICAL_LINK`, `MCD_DB_FUNCTIONAL_GROUP`, `MCD_DB_FUNCTIONAL_CLASS` |

**Never parsed, regardless of ease:** `MCD_DB_FLASH_JOB`, `MCD_ACCESS_KEY`,
`MCD_DB_SINGLE_ECU_JOB`, `MCD_DB_STARTCOMMUNICATION`, `MCD_DB_STOPCOMMUNICATION`,
`DB_CASE`, `DB_CASES`, `DB_DEFAULT_CASE`, `MCD_DB_CODE_INFORMATION`,
`MCD_DB_CODE_INFORMATIONS`. These describe flashing, access-key handshakes and
adaptation/coding write cases. Session control (`STARTCOMMUNICATION`) is also simply
redundant — `vag-protocol` already opens a UDS session with its own client. A loader
for any type in this list is out of scope for this project, permanently, not just for
this pass.

Everything not in either table (vehicle connector pinouts, special-data-group
captions, `MCD_INTERVAL`/`MCD_SCALE_CONSTRAINT` and the like) is either a field nested
inside an in-scope type — parsed as part of that type's loader, not separately — or
genuinely unneeded and left unimplemented.

## 3. The three format layers, and how each is ported

Established by reading Peter Graf's PBL source (`pblkf.c`, `pbl.c` — MIT license) and
`ODIS-project-explorer`'s Python reference (RE'd against a decompiled `MCD-Kernel`,
license status unclear — read for the *algorithm*, not copied). Everything below was
verified against a real project (`SK37X`) during this design's research phase, not
inferred from the source alone.

### 3.1 `.key` — a read-only B+Tree

4096-byte blocks, 13-byte header:

```
byte 0      level     (u8)
bytes 1-4   nblock    (i32 BE)  — next block at this level
bytes 5-8   pblock    (i32 BE)  — previous block at this level
bytes 9-10  nentries  (u16 BE)
bytes 11-12 free      (u16 BE)  — offset of first free byte
```

Item slots are 2-byte offsets stored backward from the end of the block; items
themselves grow forward from byte 13. Each item stores `keylen`, `keycommon` (bytes
shared with the predecessor key — a prefix-compression, not encryption), then a
variable-length integer (`datalen` on a leaf, `datablock` on an index node) whose
byte count is self-describing from the first byte's high bits (1–5 bytes, values up to
`0xFFFFFFFF`). A leaf item's data follows inline when short; PBL's own overflow chain
for data over `PBLDATALENGTH` (1024 B) is provably unused here, because every value
VW's tooling stores in a `.key` file is a 6/8/12-byte `(file_position, compressed_size,
decompressed_size)` triple pointing into the matching `.db` file.

**Port scope:** read-only traversal only — `First`/`Next`/`Find` over the tree PBL's
own C already implements in `pblkf.c`. No insert, delete, split, or write path; a
`.key` file is never modified by this tool.

### 3.2 Names — DJB2, no `.idx` parsing needed

An object's key in the `.key` tree is not a string; it is a 31-bit hash: `hash =
((hash << 5) + hash) + byte` seeded at `5381`, masked to `0x7FFFFFFF`, remapped to `5`
if it lands on `0`, collisions resolved by adding `11` and retrying. This is confirmed
against `ODIS-project-explorer`'s `StringStorage.py`.

Because `vag-data` already parses `AStringData.data`/`UStringData.data` (the plaintext
string pools — see `research/labels/odis-crib.md` §2) entirely on its own, name
resolution needs no `.idx` file at all: hash every string in the pool once, build an
in-memory `hash → name` table, and look up an `ObjectID` hash from the `.key` tree
against it. The `.idx` files exist and could serve as a shortcut, but are redundant
with data this project parses anyway, and skipping them is one less binary format to
port.

### 3.3 Objects — zlib + a tagged stream

A `.key` leaf's data triple locates a zlib member in the matching `.db` file (already
established, before this design started — `research/labels/odis-crib.md` §2, and
directly observable: a `.db` file is nothing but concatenated zlib streams, findable by
scanning for `78 9c` with no index at all). Inflated, an object is a byte stream:
2-byte little-endian type enum, then a sequence of tagged fields, terminated by the
literal bytes `23 3E 00` (`#>\0`) — confirmed by `ODIS-project-explorer`'s
`DbStream.__del__`, and independently by this design's own hex dumps before that code
was read: every inflated member sampled from `BL_LIBECM.sd.db` ends in exactly those
three bytes.

**Port scope:** the tag-stream reader (`object.rs`) plus one loader per type in §2's
tables — ~20 of the 84 documented types. Each loader is ported from the corresponding
Python file under `object_loaders/`, field for field, against `docs/MCD-2D.md` (the
ASAM MCD-2D/ODX reference bundled in that repository) as the authority for what each
field means.

## 4. Storage — `~/.vagcan/rod/` and `~/.vagcan/projects/<id>/`

Two directories, split by what is shared versus what is per-car:

```
~/.vagcan/
  rod/                              shared across every project — raw .rod files only.
    TTTEXT.ROD                      No .lbl/.clb kept: they parse straight into
    RD.rod                          cache.sqlite and a raw copy adds nothing a
    EV_ECM18TFS0208V0906264H.rod    re-parse couldn't reproduce from the VCDS install.
    …
  projects/
    <project_id>/
      rod-keys.json                 only created if a VCDS source ever contributed —
                                     see §4.2. Per-project: two projects may hold
                                     different VCDS builds of a same-named .rod file.
      cache.sqlite                  unified schema — §4.3
      names.json                    text-id -> name, VCDS TTTEXT ∪ ODIS-crib readings
      measurement/
        <PARTNUMBER>.json           proven-on-car catalog rows — unchanged shape
                                     (vag_data::catalog::MeasurementCatalog)
      sources.json                  provenance log — §4.4
```

`.rod` files are shared because they are a property of a VCDS **build**, not of a car:
the same `TTTEXT.ROD` byte-for-byte serves any project parsed from that build. Keeping
one copy avoids re-copying tens of megabytes per project and matches how `~/.vagcan`
already treats the extracted label cache today.

`rod-keys.json` stays **per-project**, not shared alongside the `.rod` pool, because the
recovered `IV[3..8]` (or the shift mask, for a shifted file) is a property of a
specific file's *bytes*, and two VCDS builds can ship a same-named `.rod` with
different content (the "Frankenstein install" case already hit once this session,
where an EN and a RU build's files did not agree). A project's key cache is only ever
valid for the exact `.rod` bytes it was built against.

### 4.1 `project_id` — VW's own naming, not an invented slug

An ODIS project's directory name is already the identifier VW's own tooling uses (`S42
— Fahrzeugprojektzuordnung`, VW's internal vehicle-to-project mapping spec): `SK37X` is
literally the folder `ODIS-project-explorer`'s README shows under
`MCD-Projects-E/VWMCD/<here>`. `setup` reuses this string directly as `project_id` when
the source is an ODIS project — no renaming, no user prompt.

A VCDS-only project has no such string to read off. `project_id` there is either typed
by the user at `setup` time, or — where the car's chassis type is already known (the
`F187` part number's type prefix, or a later live read) — looked up against the S42
mapping to land on the same identifier an ODIS project for that same car would use.
This second path is aspirational for this design (the S42 document is long and its
lookup is not yet built); the immediate requirement is only that `project_id` is a
free string `setup` can ask for, so a VCDS-only user is never blocked.

### 4.2 `rod-keys.json` — unchanged content, new location

Same shape as today's `rod-keys.json`/`.ivcache.json`: `{"<filename>\t<tag>":
[iv3, iv4, iv5, iv6, iv7]}`. Only the path moves, from
`~/.vagcan/data/extracted/rod-keys.json` to
`~/.vagcan/projects/<project_id>/rod-keys.json`, and the directory is created lazily —
only when `setup` actually decodes a `.rod` section for this project. A pure-ODIS
project never has this file.

### 4.3 `cache.sqlite` — one schema, two contributors

Today's schema (`vag-db`) is VCDS-shaped: `measurement(file_id, block, field, name,
location, description, unit, range_min, range_max)`, addressed by VCDS's own
block/field numbering. ODIS addresses everything by UDS DID directly, with a compu
formula rather than a `(min, max)` range. Reconciling these is this design's central
schema question, and belongs to the implementation plan rather than being decided here
in full — the two shapes need to converge on one row that can answer "what does DID
`0x380A` mean" regardless of which source populated it. What is decided: the source
column stays (`source.dir` already exists; it gains a `source.kind` alongside it,
`"vcds"` or `"odis"`), so a row's provenance is always recoverable without consulting
`sources.json`.

### 4.4 `sources.json` — provenance log

New file, one entry per `setup` run that touched this project:

```json
{
  "sources": [
    { "kind": "vcds", "version": "25.3.0", "build": "en", "path": "/Users/…/VCDS", "parsed_at": "2026-08-07T11:20:00Z" },
    { "kind": "odis", "project": "SK37X", "vw_project_id": "SK37x/0EU_K_5E0", "parsed_at": "2026-08-07T14:05:00Z" }
  ]
}
```

Read by nothing at run time (§4.5 below) — it exists for a person to answer "where did
this project's data come from", the way `git log` answers the same question for code.

### 4.5 Precedence when sources disagree

**A `measurement/` row always wins.** It is the only data proven on the actual car in
front of the tool; both `cache.sqlite` and `names.json` are *extracted* — recovered
from someone else's files, however good the ODIS cross-check looked in §1 — and fill
in only what a drive has not yet proven. This is `rod-labels.md` §4.0c's existing rule
("the label files provably cannot supply a scaling") extended to a second source, not
a new principle: an ODIS compu formula is treated exactly as a label-file scaling
always has been — evidence for a catalog, not the catalog itself, until confirmed.

Nothing in the read path branches on source at query time. `setup` is the only place
that ever needs to know which parser produced a row: everything downstream —
`watch`/`scan`/`survey`/`measure`/`faults`/`properties` — reads `cache.sqlite` and
`measurement/` and never asks where either came from.

## 5. `setup` — the source picker

Replaces today's `setup [PATH]` (parse-VCDS-or-offer-download) with an arrow-key menu,
consistent with how `EnterWorktree`/similar CLI pickers in common tools present a
choice:

```
? What should vagcan learn this car from?
❯ ODIS project        point at an extracted ODIS-Service project folder
  VCDS installation   point at an existing install
  Download VCDS       fetch Ross-Tech's installer and parse it
```

Re-running `setup` against an existing `project_id` **adds** the new source rather than
replacing the project — consistent with the "one project, sources merge" storage
decision in §4. The `--replace`-a-build behaviour `setup` already has for "installing a
different VCDS build over an old one" (`replace_if_another_build`) stays scoped to the
VCDS branch of the picker; an ODIS parse never deletes VCDS-derived rows or vice versa,
only `measurement/` rows are ever protected from being overwritten by either.

## 6. Migration

Today's single `~/.vagcan/data/{extracted,measured}` becomes the first project under
`~/.vagcan/projects/`. On first run of a build with this change, if `data/` exists and
`projects/` does not, `setup` prompts for a `project_id` for the existing data (default
suggestion: `default`) and moves `data/extracted/*` → the new locations (`.rod` files to
the shared pool, everything else into the named project) and `data/measured/` →
`projects/<id>/measurement/`. This is a one-time, one-directional move, not a
compatibility shim kept around afterward.

## 7. What this design explicitly does not settle

Left for the implementation plan, named here so they are not silently decided by
whoever writes the first line of code:

- The exact merged `cache.sqlite` schema (§4.3) — needs the DID-vs-block/field
  reconciliation worked out against real data from both sources, not designed on paper.
- The S42 chassis-type → `project_id` lookup for VCDS-only projects (§4.1) — a real
  document, not yet parsed into anything the tool can query.
- Which of the ~20 in-scope object loaders (§2) ship in the first increment versus a
  follow-up — the measurement chain is the one everything else is gated behind, so it
  goes first; identification, faults and topology can land in any order after it.
