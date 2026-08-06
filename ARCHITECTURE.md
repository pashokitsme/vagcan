# Architecture

Why this tool is built the way it is. **This one is for the curious and for anyone
working on the code** — you do not need it to use `vagcan` (that is [`USAGE.md`](USAGE.md)).
The `research/…` files it links go deeper still, into the reverse-engineering: they are
developer notes, not instructions. For the rules that were paid for in a broken control
unit, [`SAFETY.md`](SAFETY.md).

---

## The one fact that shapes everything

**Names come from VCDS's label files. Scaling cannot.**

Ross-Tech's VCDS ships 300 MB of label and ODX files, and it holds a great
deal: what every control unit is called, what its measuring blocks are called, which
fault codes exist and how they read in words. What it does not hold — anywhere, in
any encoding — is the join from a measurement to the identifier that carries it, or
the factor and offset that turn its bytes into a number.

That is not "we did not look hard enough". The read identifier is not stored in
`STRUC` under any tested encoding, checked against ground truth from a live capture
rather than assumed, and `MWB` carries no per-ECU identifier either. There is no
route from "this unit's boost pressure" to "read `0x202A`, two bytes big-endian,
×0.001 bar" through any file Ross-Tech ships. The reasoning is in
[`research/labels/rod-labels.md`](research/labels/rod-labels.md) §4.0c and
[`research/labels/label-linkage.md`](research/labels/label-linkage.md) §3. Do not go
looking again.

So the tool has two sources and they are not interchangeable:

| | comes from | rebuildable? |
|---|---|---|
| names, unit numbers, fault text | a VCDS installation, via `vagcan setup` | yes, in minutes |
| `(identifier, raw form, factor, offset)` | measured on a vehicle | only by driving |

`~/.vagcan/data/extracted/` holds the first. `~/.vagcan/data/measured/` holds the second. A
tool short of one of them is in a completely different situation from a tool short of
the other, and the messages it prints say which.

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
(`F187`) or its ODX file name (`F19E`), in `~/.vagcan/data/measured/`. A row:

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
  being extrapolated.
- **`Anchor`** — one proven `(raw, value)` point and no slope. The honest state for a
  measurement where the zero is known and the scale is not; any other raw value is
  reported as unknown rather than guessed.

A car this tool has never seen finds no file and shows raw bytes. That is the
intended behaviour, not a gap.

### How a row gets proven

Two routes, both least-squares fits that accept nothing under **R² 0.995 over ≥ 20
points and ≥ 4 distinct raw values**.

`vagcan sniff` records the bus listen-only while VCDS runs an ordinary session beside
it, and `vagcan vcds analyse` crosses that capture with VCDS's own CSV export. The
two files are aligned by wall-clock arithmetic — a subtraction, never a search.

`vagcan recording calibrate` needs no VCDS at all: it fits unproven columns of a
`vagcan watch --out` recording against columns already trusted in the *same*
recording — the standard OBD-II parameters, whose conversions are SAE J1979's, or
rows proven earlier. One clock, tens of hertz, and whatever identifiers were asked
for. What it cannot do is **name** anything.

---

## The file formats

Full writeups under [`research/labels/`](research/labels/).

**`.lbl` — plain text.** The old format, still shipped for older control units. One
file per part number, human readable, with a `; Component: … (#02)` header naming the
unit and its number, then measuring-block and field names. Nothing to crack.

**`.clb` — the encrypted `.lbl`.** Same content in a TEA-CBC container; decrypted
in-tool by `vag-data`.

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
five bytes of its first-block IV are missing and have to be searched for, at about a
minute of every core per section. That is why the recovered keys are cached — the
live path reads the answer out of `~/.vagcan/data/extracted/rod-keys.json` and never
searches.

**A control unit tells you which `.rod` is its own.** Identifier `F19E` returns an ODX
file name — `EV_ECM18TFS0208V0906264H`, say. That is how `vagcan vcds labels
--from-car` finds the right file with no lookup table in the middle.

**`Codes.dat` — the fault-code text store.** A fault number does not resolve to words
directly. The chain is:

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
[`research/labels/fault-naming-hop.md`](research/labels/fault-naming-hop.md).

**`TTTEXT.ROD` — the names.** Every record of its `[TXT]` section is enciphered under
its **own** substitution, so there is no single key to find. The attack is
dictionary-driven and bootstraps: records sharing the repetition pattern of their
letter runs hold the same words under different keys, so one solve serves a whole
cluster, and words read off solved records become vocabulary for the next pass. See
[`research/labels/tttext-codec.md`](research/labels/tttext-codec.md).

---

## What `vagcan setup` actually does

One command, three steps, everything under `~/.vagcan/data/extracted/`.

**1. The label files → `cache.sqlite`.** Every `.lbl` parsed and every `.clb` decrypted
into a SQLite database keyed by part number, so a later lookup is milliseconds rather
than a re-parse of 300 MB. The cache records which directory it was built from — inside
itself, so it is one file that can say what it holds — because the freshness rule is an
mtime comparison and an mtime cannot tell "older than the label files" from "built from a
*different* set of label files", which matters as soon as somebody has both the English
and the Russian install.

**2. `TTTEXT.ROD` → `names.json`.** The `[TXT]` section is decrypted and inflated,
then the cipher above is attacked with the label files' own label files as the in-domain
vocabulary (weight 8) and the system word list as the general one (weight 1). This is
the slow step: minutes, mostly single-threaded search.

Then comes the **prior**, and it is the difference between four thousand names and
seventeen. Weighing every in-domain word alike leaves the search breaking a tie
between two same-shape words — `of`/`ob`, `oil`/`bil`, `voltage`/`boltage` — by
alphabetical order, and a cluster leader that guesses wrong pins that letter for
every record it feeds. The fix is the word's **frequency in the decoded label files
itself**: `of` outnumbers `ob` thousands to one, so re-solving every cluster under
that frequency settles the ties on evidence. It is measured from the decode at parse
time, never a table baked into the binary. Two wrinkles the reference label files forced:
the in-domain seed counts a word's label-file occurrences but **saturates** them,
because a label file lists status literals (`OK`, `ON`, `LC`) thousands of times and
uncapped they outrank `of` and make the search read `Status ok` for `Status of`; and
the frequency drives the search and the gate's ambiguity margin, while *membership* —
whether a word is a word at all — stays the pre-bootstrap seed, so a misreading fed
back into the vocabulary cannot vouch for itself.

Then the readings pass a **gate**, and the gate is the product rather than the
decode. About 63 % of records decode; far fewer may be shipped, because a fluent
wrong reading is indistinguishable from a right one at the point of use. A reading is
kept only if:

- no letter is unresolved;
- the trailing field run is cleanly separable — a record is
  `<name><sep><digit><sep><number>` and the tail survives the decode as noise, so it
  is cut, but only where the run's first character recurs later in it. Otherwise the
  name may itself have ended in a digit, and cutting silently turns `… of cylinder 4`
  into `… of cylinder`;
- it contains no digit at all (the glyph class carrying digits is unbroken, so a
  digit in a name is a guess);
- it has at least 12 letters;
- every word of length ≥ 3 is one the **seed** vocabulary knows;
- every word beats its best alternative reading by **20×**.

The last two are the ones that matter. `Hill bytes to maintain backward
compatibility` is fluent, dictionary-clean and stable across keys, and the word is
`Fill`: a letter occurring once in a record is pinned by nothing but the dictionary.
And the vocabulary has to be the *seed*, not the grown one — the bootstrap feeds
words from solved records back in at high weight, so gating against the grown
dictionary teaches the gate its own misreadings and then passes them.

**3. `RD.rod` and `MUX.rod` → `rod-keys.json`.** The label files-wide sections whose keys
every car needs. Per-unit files are deliberately not swept: there are over sixteen
thousand of them, a blocked section costs about a minute of every core, and which
handful a given car needs is a question only that car can answer.

Each step is skipped when what it would write is already newer than what it would
read; `--refresh` forces the lot.

---

## The crates

```
crates/
  vag-transport   the transport trait — the seam every backend implements
  vag-can         slcan USB-CAN backend, listen-only mode, ISO-TP sniffer
  vag-protocol    UDS client, ISO-TP framing, unit addressing
  vag-data        label parsers and decoders (.lbl/.clb/.rod), ODX resolution
  vag-db          SQLite cache over the label files
  vag-capture     capture and replay transport, so tests need no hardware
  vagcan          the CLI
```

`vag-protocol` cannot read a label file — it is the protocol layer and label files are
not a protocol — so the label files' unit numbering is pushed *in* from `vagcan`, and
what crosses the seam is plain numbers and strings.

**Two addressing conventions are live on the same car.** ISO 15765-4 pairs
`0x7E0..0x7E7` with `+8`, so the engine answers `0x7E0 → 0x7E8`. VW's own block
answers at `+0x6A`, so the instrument cluster is `0x714 → 0x77E`. Assuming only the
first makes every unit outside the powertrain invisible, which is exactly what
happened before it was measured. Which CAN id a *unit number* is answered on is in no
data file this project has found — the label files carry the numbers and the names and
no CAN id anywhere — so that half is established by reading the car (`vagcan units
--identify --labels`) or written down by hand.

**Sweeping is group testing, not 65,536 reads.** A multi-identifier request comes back
with only the identifiers the unit supports, and is refused outright when it supports
none of them — so one request is a presence test for a whole batch. That is what turns
a full sweep from hours into minutes.

**The CLI is split by what a command needs.** The top level is for commands that need
a car in front of you. `recording …` reads back drives this tool recorded, and
`vcds …` reads VCDS's own files. A top level crowded with offline analysis is a top
level nobody can scan while standing at an open driver's door. There is a test that
asserts it.

**Read-only is enforced in the client, not by convention.** The UDS service allowlist
admits `0x22` (read data), `0x19` (read faults), `0x10` (session control) and `0x3E`
(tester present), and that is the whole of it. See `SAFETY.md` for why that is not the
same as harmless.

---

## The repository

```
crates/         the Rust workspace
research/       reverse-engineering writeups and tooling, one directory per subject
  labels/         VW's label files: the .rod crack, the name codec, fault naming
  car/            what the reference car answers: identifier map, units, surveys
  eps/            the steering-assist incident — read alongside SAFETY.md
  clb-crack/      the RE scripts themselves
archive/        retired paths, kept as evidence
docs/           active design specs
todo/           the roadmap and the goal statement
```

**Nothing this tool reads at run time is in here.** The label data is Ross-Tech's and
cannot be redistributed; the measured rows are one owner's car and are not true of
anybody else's. Both live under `~/.vagcan/`.

**Nothing is deleted; things are moved.** Most of what this project knows was measured
on one car, once, and several of its most valuable documents are records of things
that did *not* work. A refutation you throw away is one you pay for twice. `archive/`
exists so that "we tried that, here is why it failed" survives a year.

Start here: [`todo/README.md`](todo/README.md) for where things stand,
[`todo/GOAL.md`](todo/GOAL.md) for the goal and the stack, and
[`research/labels/rod-labels.md`](research/labels/rod-labels.md) for the format work.
Design documents are in [`docs/superpowers/specs/`](docs/superpowers/specs/).
