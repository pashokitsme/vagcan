# The ODIS project on disk — three formats, and where the references are wrong

VW's ODIS-Service ships each vehicle platform as a directory of binary files. This is
what those files are, established on 2026-08-07/08 against project `SK37X`
(472 files, 230 pool pairs) and implemented in `crates/vag-data/src/odis/`.

Companion to `research/labels/odis-crib.md` (which established that none of it is
encrypted, and used the string pools against the `TTTEXT` cipher) and
`research/labels/rod-labels.md` (the Ross-Tech container, and the drive that proved the
scalings this document cross-checks against).

Two open-source references were read for the *algorithm* and neither was copied or
linked: Peter Graf's **PBL** (`pblkf.c`, MIT) for the `.key` B+Tree, and
**`ODIS-project-explorer`** (a community Python tool RE'd against a decompiled
MCD-Kernel, no licence file) for the object schema. §3 is the part that matters most:
four things the second of those gets wrong or leaves out, each of which reads as a
working parse right up until it does not.

---

## 0. Verdict up front

| question | answer | §
|---|---|---|
| Is a project readable without VW's kernel, PBL, or Java? | **Yes**, entirely | §1 |
| Does it give measurement scalings with no drive? | **Yes** — 1,232 channels for one engine variant | §4 |
| Do those scalings agree with what driving proved? | **Yes, 3 of 3, including a byte-order split** | §5 |
| Is `ODIS-project-explorer` a sufficient spec? | **No** — four defects, §3 | §3 |
| Can the refusal list be honoured on a real project? | **Yes**, but not by refusing to *move* | §6 |
| Does the `.vi` pool say which vehicles a project covers? | **No** — it is bus topology. `PRNR-INFO.xml` does | §6 |
| Is anything still unexplained? | Yes — 34 layer tails, and a gear conflict | §8 |

---

## 1. The three layers

A project directory holds `0.0.0@<name>.<kind>.db` / `.key` pairs plus two string pools.
Six kinds occur — `.sd` shared service data, `.bv` base variants, `.fg` functional
groups, `.pr` protocols, `.cp` com params, `.vi` vehicle info — and **the ECU variants
are all in `.bv`**, which is worth saying because the design assumed `.sd` and a reader
that believed it finds no variants at all.

### 1.1 `.key` — a read-only B+Tree

Peter Graf's PBL, unmodified. 4096-byte blocks, block 0 the root, a 13-byte header:

```
byte  0     level     u8     0 = leaf, higher = inner, 255 = overflow data block
bytes 1-4   nblock    i32BE  next block at this level (0 = none)
bytes 5-8   pblock    i32BE  previous block at this level
bytes 9-10  nentries  u16BE
bytes 11-12 free      u16BE  offset of the first free byte
```

Items grow forward from byte 13; their offsets are a **backward** array of 2-byte
big-endian slots, item *i* at `4096 - 2*(i+1)`. An item is `keylen`, `keycommon`, a
variable-length integer, then the last `keylen - keycommon` bytes of the key. The first
`keycommon` bytes are shared with the *previous item on the same block* — prefix
compression, not encryption — so keys expand in slot order and a block cannot be read
from the middle.

The varint is `pbl_VarBufToLong`, self-describing from the first byte's high bits:
`0xxxxxxx` one byte, `10` two, `110` three, `1110` four, `1111` a plain four-byte
big-endian value. It carries `datablock` on an inner node and `datalen` on a leaf.

PBL inserts a `keylen == 0` pseudo-item as record 0 of every file, holding the magic
string `1.00 Peter's B Tree`. It sorts before any real key and is not an object.

**PBL's overflow chain for data over 1024 bytes never occurs**, because every value VW
stores is a 6/8/12-byte locator. The implementation refuses it rather than following it:
if a file ever did use it, saying so beats handing back whatever bytes follow.

Measured on the engine's `.bv`: 2,631 blocks, **576,793 records**, every one of which
resolves to a string-pool name.

### 1.2 Names — DJB2, and the `.idx` files are redundant

A key is not a string. It is four bytes, little-endian, of a 31-bit hash:

```
h = 5381;  for each code unit c:  h = h * 33 + c
h &= 0x7FFFFFFF;  if h == 0 { h = 5 }
while the slot is taken by a different string:  h = (h + 11) & 0x7FFFFFFF; if h == 0 { h = 5 }
```

The `+ 11` probe is not decoration — a reader that probes differently reconstructs a
*different* table from the same pool, and every key that ever collided then resolves to
the wrong name.

The two pools parse to their last byte in one forward pass, with no index:

| file | contents | inflated |
|---|---|---|
| `AStringData.data(.gz)` | 1,155,437 names, `u32` **byte** count + Windows-1252 | 72,832,132 B |
| `UStringData.data(.gz)` | 153,704 texts, `u32` **character** count + UTF-16LE | ~15 MB |

The character-vs-byte distinction is load-bearing: reading the Unicode count as bytes
desynchronises on the very first string.

So the `.idx` files are never read. Recomputing is one less binary format to port and it
is exact where it matters: **all 576,793 keys of the engine pool resolve** against a
table built this way. Cross-checked against the `.idx` anyway, 1,155,427 of 1,155,437 A
hashes agree; the 10 that do not are strings that occur twice, where the writer's
insertion order and ours disagree about which copy took the base hash. No `.key` entry
pointed at one.

The pools are gzip (RFC 1952), not zlib. `miniz_oxide` has no gzip wrapper, so the
ten-byte header is walked by hand; the CRC trailer is not checked, because "the pool
parses to its last byte" is a stronger statement about the same bytes.

### 1.3 `.db` — concatenated zlib members

A `.db` has no index and no framing of its own. What says where a member starts is the
paired `.key`, whose every leaf holds `(position, compressed size, decompressed size)`.
**The triple's width is carried by the record's length alone**: 6 bytes means `u8` sizes,
8 means `u16`, 12 means `u32`. All three are real — on the engine pool the census is
552,223 / 24,569 / **1** — so none may be dropped, and a length nobody defined has to be
refused rather than guessed at.

The declared decompressed size is verified against what came out. That check is the point
of keeping it: a `.db` truncated mid-member still inflates to a prefix, and a reader that
trusted the length would parse that prefix as a whole object and report a control unit's
measurements from half a record.

### 1.4 The object stream

An inflated member opens with a **two-byte little-endian type code** and then runs
straight into that type's fields. Most string fields are a four-byte hash into one of the
two pools, `0` meaning "no string" — which is why a hash of `0` is illegal and remapped
to 5.

Type census, engine `.bv` pool, all 576,793 members:

```
0x0057 MCD_DB_DIAG_TROUBLE_CODE     202,863     0x0031 DB_LAYER_DATA                403
0x00AC MCD_DB_TABLE_PARAMETER       168,819     0x005C MCD_DB_ECU_VARIANT           402
0x0203 MCD_CONSTRAINT                78,550     0x0095 MCD_DB_PARAMETER_END_OF_PDU  401
0x00AA MCD_DB_PARAMETER_STRUCTURE    55,675     0x0028 DB_DOP_DTC                   357
0x002C DB_DOP_SIMPLE_BASE            46,230     0x00A0 MCD_DB_PARAMETER_MULTIPLEXER  69
0x00BE MCD_DB_SERVICE                 8,122     0x0102 MCD_DB_UNIT                   27
0x00AB MCD_DB_TABLE                   6,165     0x00BF MCD_DB_SINGLE_ECU_JOB         10
0x0078 MCD_DB_REQUEST                 4,696     0x005A MCD_DB_ECU_BASE_VARIANT        1
0x0091 MCD_DB_RESPONSE                4,002     0x0033 DB_PROJECT_DATA                1
```

---

## 2. The measurement chain

```
DB_LAYER_DATA                       one per variant, ObjectID `LD_<variant>`
 └ MCD_DB_SERVICE                   named `DiagnServi_ReadDataByIdentMeasuValue`
    └ MCD_DB_RESPONSE               the positive response's parameter list
       ├ …PARAMETER_TABLE_KEY       byte 1: the DID
       │  └ MCD_DB_TABLE → DB_DOP_SIMPLE_BASE, a TEXTTABLE mapping DID → channel name
       └ …PARAMETER_TABLESTRUCT     byte 3: the measurement
          └ MCD_DB_TABLE → MCD_DB_TABLE_PARAMETER, one row per DID
             └ MCD_DB_PARAMETER → MCD_DB_PARAMETER_STRUCTURE
                └ MCD_DB_PARAMETER → DB_DOP_SIMPLE_BASE
                   ├ DB_DIAG_CODED_TYPE   bit length, base type, **byte order**
                   ├ DB_PHYSICAL_TYPE     what it becomes
                   └ DB_COMPU_METHOD      how
```

Byte positions 1 and 3 are the protocol, not this car: a UDS `0x22` positive response is
`62 <DID hi> <DID lo>` then data. The implementation checks the file against that rather
than assuming they agree.

A structure with several fields is several channels on one DID, each at its own bit
offset — engine DID `48B6` yields eight, at bits 0/16/32/48/64/80/96/112, each separately
named.

Of ODX's eight compu categories, three become a `vag_data::Scaling` honestly:
`IDENTICAL`, `LINEAR` (rational coefficients, `(VN0 + VN1·x)/VD0`) and `TEXTTAB` (an
`Enum`, keyed on the **coded** bound — the physical bounds hold the same text as the
constant, and reading the key off them yields no key at all). The other five are
piecewise, interpolated, polynomial or externally coded; they are an error that names the
category, never a silent factor of 1.

---

## 3. Where the references are wrong

This is the section worth keeping. Everything here was found only by reading real files;
each one produced a parse that looked correct for a long time before failing.

### 3.1 The fields are positional. There are no tags

The design document called these "tagged fields". They are not — the tag is the
*object's*, not the field's, and the two-byte type code is the only self-description in
the whole stream. The consequence is the whole difficulty of this format: **there is no
way to skip a field you do not understand**, a field read at the wrong width silently
shifts everything after it, and every loader must be a literal transcription of a field
order including the fields nobody wants.

### 3.2 There is a second terminator, `23 3C 00`

`ODIS-project-explorer` documents `23 3E 00` (`#>\0`). On the engine pool the census is
**407,974 `23 3E 00` and 168,819 `23 3C 00`** — the second count being exactly the
`MCD_DB_TABLE_PARAMETER` count. Both end an object.

### 3.3 Only the outermost object is terminated

A nested object — a compu method inside a data object property — carries no terminator
and runs straight into the field that follows it. A loader that consumed one of its own
would eat the next field's first bytes. The Python reference never consumes terminators
inside `load_object_from_stream_if_exists`, which is easy to read as an omission and is
in fact the rule.

### 3.4 A terminator is not always the last three bytes

Some types append named sub-streams *after* it — `MCD_DB_TABLE_PARAMETER` ends
`23 3E 00 | 41 01 23 3E 01 23 3C 00 42 00 00 23 3E 01 | 23 3C 00`, where `41`/`42` are
`A`/`B`. So a reader asserts the terminator is where the fields stopped, and says nothing
about what follows.

### 3.5 A named-reference collection carries two names, not three

`load_reference`'s own default is three, and `loadNamedObjectReferenceCollectionFrom
ObjectStream` passes `third_string=False`. Transcribing the default instead is the single
most expensive mistake available here: with three names, entry *n*'s object id is entry
*n-1*'s pool id, and after a 402-entry variant list the cursor is deep inside the access
keys beyond it. **Every `.bv` pool and every `.sd` pool failed on this**, in two different
ways, and neither failure pointed at the cause.

### 3.6 `DB_PROJECT_DATA` recurses

It ends in a list of nested `DB_PROJECT_DATA` objects, one per ECU variant. Not following
it leaves 77,988 of the engine pool's 137,294 bytes unread.

### 3.7 `DB_LAYER_DATA` has a long tail

After the unit maps: protocol parameters, a byte, a byte, and a final map. Before them,
environment-data descriptions — freeze-frame layouts, present on 26 of the project's 663
layer-data objects, and absent from the shape a first reading suggests.

---

## 4. What comes out

Per ECU variant, with no car present and no drive:

```
EV_ECM18TFS0208V0906264H_001  (engine)    1,232 channels
EV_TCMDQ200021_001            (gearbox)     859 channels
```

Each channel carries a DID, a name, a text id (`IDE00022`, `MAS18568` — the join to
`TTTEXT`, `odis-crib.md` §3), a unit, a bit offset and length, signedness, **byte order**,
and a scaling.

---

## 4.1 What the project declares against what the car answers (2026-08-09)

The parser's numbers describe a **vehicle family**. The question nobody had asked is how
much of that family one car is, and the answer changes what the tool should put on a
screen.

Measured on the reference car (Škoda Octavia III, VIN `XW8AD4NE9JH008917`) by joining its
cached survey — fifteen units, each with its `F187`/`F19E`/`F1A2` — against the SK37X
project in `~/.vagcan/data/SK37X/cache.sqlite`, matching each unit to its variant as
`<F19E>_<first three digits of F1A2>`:

| | identifiers |
|---|---|
| the project declares, across the fifteen units | 2,251 |
| the car answered | 1,198 |
| **declared *and* answered** | **505** |
| **answered, and the project does not declare** | **693** |
| **declared, and the car said nothing** | **1,746** |

Per unit, declared / answered / both:

```
70A  EV_EPHVA14AU3700000          109 /  38 /   5      74A  EV_DCUDriveSideEWMAXCONT  0 / 53 / 0
70C  EV_SMLSVALEOMQBLRH             6 /  39 /   0      74B  EV_DCUPasseSideEWMAXCONT  0 / 50 / 0
70E  EV_BCMMQB                    168 / 126 /  54      767  EV_OCULowMQBLGE          73 / 50 / 15
710  EV_GatewNF                   205 / 131 /  66      773  EV_MUEnt4CGen2LGE        67 / 49 /  8
712  EV_SteerAssisMQB              48 /  48 /  10      7E0  EV_ECM18TFS…264H        809 /160 /126
713  EV_Brake1UDSContiMK100ESP     50 /  48 /  11      7E1  EV_TCMDQ200021          513 /186 /153
714  EV_DashBoardVDDMQBAB          65 / 117 /  36
715  EV_AirbaVW21TS6VW48X          18 /  47 /   1
746  EV_ACClimaBHBVW37X           120 /  56 /  20
```

Three things follow.

1. **The two door units have no ODIS coverage at all**, and the car answers 103
   identifiers between them. They are the two unresolved units of §8, quantified.
2. **693 of what this car answers — 58% — the project never mentions.** That is what a
   sweep is *for*, and it is also the class of data the safety change of 2026-08-09 makes
   uncollectable: a declared-only sweep cannot find an undeclared identifier by
   construction. The cached survey that produced these numbers is a **blind** sweep from
   2026-08-08, so it cannot be reproduced under the current rules. Do not delete it.
3. **1,746 declared identifiers are silent**, and they were on `watch`'s selection screen
   looking exactly like the working ones. `watch` now holds them back by default
   (`plan::Answered`, `crates/vagcan/src/watch/plan.rs`), counted separately from the
   nameless rows and restored with `u`.

**The caveat that limited claim 3, and what closed it.** A survey line used to record what
*answered* and never what was *asked*. For a full sweep those are the same question and
absence means silence; for a run aimed with `--blind --range` they are not, and an
identifier outside the range would read as silent when nobody ever asked. A survey now
writes an `asked` field — the spans it swept, `0102-0104` or a bare `F187` — and
`plan::Answered::saw` returns `None` outside them, so silence is only ever claimed where
somebody asked. Files written before that field fall back to the old reading, which is
correct for the full sweeps those files are, including the one this section measures. The
filter stays a default rather than a deletion because a channel can also be silent for
reasons the next paragraph lists.

**What is not established.** Why a declared identifier is silent. Wrong variant match,
another session, a state the car was not in when parked, or an identifier this trim level
genuinely does not implement — the data here cannot tell them apart, and 1,746 is an upper
bound on "this car does not have it".

---

## 5. The cross-check — three rows proven by driving, reproduced from a file

`rod-labels.md` §5 proved three scalings by driving the reference car next to a
listen-only CAN capture, each an exact linear relation (`R² = 1.00000`), the gearbox row
verified byte by byte against the log. Those three rows, and what this parser reads out of
`SK37X` without any of that:

| DID | unit | proven by driving | ODIS project says |
|---|---|---|---|
| `F405` | engine | `u8`, `raw − 40`, °C, IDE00025 | 8 bits, BE, `Linear{1.0, −40.0}`, `°C`, IDE00025 |
| `206E` | engine | `u16` **BE**, `raw`, /min, IDE00405 | 16 bits, **BE**, `Linear{1.0, 0.0}`, `1/min`, IDE00405 |
| `380A` | gearbox | `u16` **LE**, `raw`, /min, IDE00022 | 16 bits, **LE**, `Linear{1.0, 0.0}`, `1/min`, IDE00022 |

Three for three, and **the byte-order split lands on the right side of both rows**. That
is the result that makes this branch trustworthy rather than merely convenient: two
independent derivations, years and methods apart, agreeing on a detail neither could have
copied from the other. `380A` big-endian would read 690 /min as 45570, and nothing in a
run would look wrong.

It is also why `odis::Reading` carries `big_endian` and why it has to survive into
`cache.sqlite` as a column. UDS payloads are big-endian *by convention*; this car's own
proven row is not.

---

## 6. The `.vi` pool: how to talk to the car, not which car

A project has to be selectable by the car plugged into it (spec §4.1), and the pool named
`VI_<project>.vi` is the obvious place to look. **It does not answer that question**, and
the name is the whole reason anyone would think it does.

`0.0.0@VI_SK37X.vi` holds 62 objects:

```
0x006A MCD_DB_LOGICAL_LINK                        59   one per control unit
0x006E MCD_DB_PHYSICAL_VEHICLE_LINK_OR_INTERFACE   1   PhysLink_CAN1
0x00D4 MCD_DB_VEHICLE_INFORMATION                  1   VI_VINFO_SK37XCAN
0x0034 DB_VEHICLE_INFO_DATA                        1   points at that one
```

The single `MCD_DB_VEHICLE_INFORMATION` parses to `short_name = VINFO_SK37XCAN`,
`long_name = "SK37X CAN"`, 59 logical links, one physical link, and one connector
(`Diagnostics On CAN Connector`, pins CANH and CANL). It is **named after the project, not
after a vehicle**, and everything in it is topology: which bus, which links, which pins.
"Vehicle information" means *how to reach the car*.

Nor do the pools answer it anywhere else. Searched across all 1,155,437 ASCII and 153,704
Unicode strings: **no `Karoq`, no `Kodiaq`, no `Octavia`, no `SK371`, no `5EP`, no
`Fahrzeugprojekt`.** `SK326` occurs six times, every one of them free-text prose about a
single amplifier variant ("*ECU Variant for the GEN_2 AMPLIFIER 16 CH for SK37, SK326*").
`5E0` occurs only inside part numbers quoted in descriptions. This is a measured negative,
not an inference from a type name.

### 6.1 `PRNR-INFO.xml` is where it lives

Outside the pools, in the project root:

```xml
<ODX-PLATFORM>SK37X</ODX-PLATFORM>
<VEHICLE HAS-PRNR-DIFFERENTIATION="false" IS-DEFAULT="true">
    <VEHICLE-PROJECT>SK37X/0EU_X</VEHICLE-PROJECT>
    <NAME>A7 / Octavia III (Limo, Combi)</NAME>
    <PRODUCT-ID>5E0</PRODUCT-ID>
</VEHICLE>
```

Six vehicles for `SK37X`: `5E0` Octavia III, `5EU` Octavia III (Russia), `5EF` Octavia III
(India), `55A` Kodiaq EU, `5EP` Karoq EU, `55U` Kodiaq RU. VW's `S42 —
Fahrzeugprojektzuordnung` is a rendering of exactly these fields; an `S42` line is
`VEHICLE-PROJECT` + `PRODUCT-ID` + `NAME` run together.

Note that **all six carry `IS-DEFAULT="true"`**, so that attribute disambiguates nothing
and no selection should rest on it.

The file is one of the 472 entries declared in `rt_index.xml`'s `RUNTIME` block, so it is
part of **every** extracted project by construction and not by luck of one copy. That is
what makes depending on it safe, and it is why no table of type codes needs to live in
this repository.

### 6.2 The car-side key is not settled, and the obvious rule is wrong

A `PRODUCT-ID` is a three-character VW type code. The tempting match is the leading three
characters of an `F187` part number — and the reference car refutes it:

```
712 5Q0909144T   713 5Q0614517AQ   714 5E0920740D   715 3Q0959655BE   746 5E0907044AM
767 3Q0035284    773 5E0035871C    7E0 8V0906264H   7E1 0CW300041G
```

Three units report `5E0`, which is this project's default vehicle and the right answer.
But the **engine reports `8V0`** — an Audi platform number sitting in a Škoda — and
`5Q0`/`3Q0` are MQB-common across the whole group. A part number is evidence about a
*component*, and only sometimes about the vehicle it is bolted into. Choosing which of
fifteen answers to believe is a policy over live data, not a property of a file format,
and `Project::covers()` therefore takes the type code as an argument rather than deriving
it.

One thing this project cannot establish on its own: **whether `PRODUCT-ID` sets are
disjoint between projects.** Selection works here because `5E0` is present and specific.
That is a sample of one, and a second project settles it.

## 7. The refusal list, and how it is enforced

`SAFETY.md` and the design's §2 name ten object types that are never parsed into anything
executable. Two of them sit directly in the path of reading a car at all, and the way that
was resolved is worth recording, because the obvious resolution is wrong.

**`MCD_ACCESS_KEY` is embedded in the objects that hold the variant list.** Every one of
the 54 base-variant pools carries access keys inside `DB_PROJECT_DATA` — the engine pool
has 1,209 of them in front of its variants — and inside every `MCD_DB_ECU`. Refusing to
*move past* them refuses the variant list too, which is not a safety property; it is an
inability to read the car. The resolution: a `skip_access_key` that returns `()`. It
builds nothing, keeps nothing, hands nothing back, and the only thing it changes is the
cursor. The type stays on the list, dispatching it still yields `Refused`, and a test
asserts both halves together. Stepping over bytes is not parsing them.

`ODIS-project-explorer` also reaches a variant's layer data *through* an access key
(`ecu.location_refs[0].access_key.layer_data_object_id`). This implementation does not:
the layer data is at the generated ObjectID `LD_<variant>`, and failing that is found by
scanning the pool for the `DB_LAYER_DATA` that names the variant. Same answer, no key
parsed.

**`DB_CASE`/`DB_CASES`/`DB_DEFAULT_CASE` are inline sub-objects of
`MCD_DB_PARAMETER_MULTIPLEXER`**, a measurement type, and they have no fixed length — so
they cannot be stepped over, only stopped at. A multiplexed channel is therefore skipped
rather than reported wrong. The cost is small and measured: 69 multiplexers against 46,230
simple data object properties on the engine pool.

---

## 8. Open questions

### 8.1 The gear conflict — one drive settles it

`crates/vag-data/src/catalog.rs` records that on this car the gear code is `gear + 1` and
that **reverse is code `0C`**. The ODIS project says engine DID `0x210F` ("Selected gear",
IDE00090) is a text table reading:

```
0 → Gear 1   1 → Gear 2   2 → Gear 3   3 → Gear 4   4 → Gear 5   5 → Gear 6
7 → Reverse gear   8 → Shifting process active   9 → Malfunction
10 → Gear 7   11 → Gear 8   12 → Gear 9   13 → Gear 10
```

On that table `0C` is *Gear 9* and reverse is `7`. Either the two describe different
channels on different units, or one of them is wrong. **This is not resolved here and
must not be assumed either way.**

*What would settle it, in one minute with the car:* select reverse, read `0x210F` on the
engine (`7E0`) and `0x3816` (`Display_Driving_Gear`, IDE02736) on the gearbox (`7E1`), and
see which raw value each returns. If `0x210F` answers `7`, ODIS is right about it and
`catalog.rs`'s `0C` belongs to some other channel — most likely the gearbox's, which is
where the original measurement was taken.

### 8.2 Thirty-four layer tails that do not parse

663 `DB_LAYER_DATA` objects live in the `.bv` pools. All 663 give up their service index;
**34 end in a tail shape this reader cannot follow.** The implementation splits the type:
the head — everything the measurement chain needs — is kept, and the terminator check is
forgone for those 34 objects only (`LayerData::complete` says which). That is a deliberate
trade, not an oversight: the alternative was 34 control units losing every channel to
protect bookkeeping nobody reads. Someone with a second project should look at what those
34 have in common.

### 8.3 What a second ODIS project would test

Everything above is one project. The specific things a second one would falsify or
confirm:

- whether the `LD_<variant>` naming for layer data is universal or `SK37X`'s converter
  version (26.1.0) only;
- whether the twelve-byte locator — **one** occurrence in 576,793 — is a real width or an
  artefact, and whether an overflow-chain record ever appears;
- whether the 34 unfollowed tails are a shape or a corruption;
- whether `MCD_DB_LOCATION_REFERENCES`'s access-key count is ever above 1;
- whether `23 3C 00` is exclusively `MCD_DB_TABLE_PARAMETER`'s, as it is here;
- **whether `PRODUCT-ID` sets are disjoint between projects** (§6.2) — the one thing that
  decides whether a connected car can select its project from a type code at all.

---

## 9. Where this lives in the code

```
crates/vag-data/src/odis/
  hash.rs      DJB2, the 31-bit mask, the 0→5 rule, the +11 probe
  strings.rs   both pools, gzip-or-plain, Windows-1252, hash → name
  keyfile.rs   the PBL B+Tree, read-only — no insert, delete or split exists
  pool.rs      locator widths and member inflate
  object.rs    the positional stream cursor and both terminators
  compu.rs     COMPU-METHOD → vag_data::Scaling
  loaders/     the type table, the refusal list, and one transcription per type
  mod.rs       Project / Variant / Reading
```

Nothing this reads lives in the checkout, and no test reads a real project: the
end-to-end test synthesises a whole miniature project — pools, objects, string pools — in
a `tempfile` directory.
