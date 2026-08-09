# vagcan — roadmap & status

**Goal:** read the **whole car over CAN** and show measurements by name/value/unit,
definitions as **data, not code**.

**The primary source is a VW ODIS-Service runtime project** (since 2026-08-08): it
declares, per ECU variant, every identifier that unit answers along with the byte
offset, length, byte order and compu formula — the whole chain, which a VCDS label file
provably does not carry (`research/labels/rod-labels.md` §4.0c).

**An ODIS project carries the fault codes and their text too** — 329,268 `DTC_*` objects
in `SK37X`, with descriptions in the clear in the Unicode pool (`Steuergerät Fehler im
RAM->defekt`), no cipher and no `Codes.dat` involved. What is missing is the *loader*,
not the data: `DB_DOP_DTC` and `MCD_DB_DIAG_TROUBLE_CODE` are in the type table and
`odis/loaders/` holds only `identity.rs` and `measurement.rs`. Until that lands,
`vagcan faults` still names codes from VCDS files — a limit of this implementation,
**not** a property of the sources, and it must not be written down as one (2026-08-09).

**A VCDS installation is therefore the fallback**: the path for a car no ODIS project
covers, or for someone who cannot get one.

Above both sits what was **proven live on the car** —
`~/.vagcan/data/<project>/measurements/<part number>.json`, keyed by what the unit
reports about itself. A drive outranks a file, always; an extracted row fills only what
no drive has proved.
The label files carry the scaling *values* but not the join from a measurement to its
scaling or read DID, so scaling is still proven live — the full audit, including which
earlier refutation rested on a broken decode, is in
`research/labels/scaling-audit.md` (2026-08-06). Any value a unit
exposes is selectable from config, with no hardcoded addresses or formulas in Rust.
Live transport = the **generic USB-CAN adapter** (`vag-can`, slcan). See `/CLAUDE.md`
for the locked stack and `todo/GOAL.md` for the goal statement.

## Status (2026-08-09) — the sweep stopped fuzzing, and `watch` became usable

**`survey` no longer sweeps blind, after a second near-miss with the same steering
rack.** On 2026-08-09 a run nearly repeated the 2 August incident with the car
**parked** — every guard in force, and all of them about *where the car was* rather than
what was being asked. Full account in [`SAFETY.md`](../SAFETY.md); what changed:

- A sweep asks only identifiers a source declares for that unit. The steering assist
  declares 161; it used to be asked 2816, and the other 2655 were the fuzz test.
- A unit that goes quiet or goes back on an identifier it already answered **ends the
  whole run**, non-zero. `SAFETY.md`'s "stop when something changes" was written after
  the first incident and lived nowhere but that file until now.
- Blind sweeping is `--blind <unit>`, aimed by hand. `survey --blind` bare is a parse
  error: whole-car blind was the default, and it is what did the damage.
- A safety message never goes on the self-rewriting progress line.

Thresholds `WITNESS_EVERY = 64` / `QUIET_RUN = 3` are reasoned, **not measured** — a
false halt teaches people to reach for the override, so one parked whole-car run is
owed.

**`watch` is usable.** Of 2,751 channels on the reference car, 1,964 now carry a human
name and the 787 that nothing can name are hidden (`u` shows them, the header says how
many). `f` marks a favourite — kept per car under `cars/<VIN>/favourites.json`, because
which identifiers a unit answers is a fact about that unit *in this car*, and a platform
project covers Karoq and Kodiaq too. `s` opens a chart-lines screen that says why a
marked row is not drawn: `no room` (cap of six) or `no number`.

**Naming has provenance now, and the join it was built for is still untested.** The
`text_id` → `names.json` lookup landed and immediately made names *worse* on an
ODIS-only project: `names.json` there was written by ODIS itself, so preferring it
reworded 340 channels toward generic pooled text, twice collapsing two distinct channels
onto one name (`MAS14374`: `Total_Physical_…` and `Total_Logical_Wakeup_Events_Counter`
both became `Total_CarWakeup_Events_Counter`). ODIS names now go to `names-odis.json`,
and `names.json` is trusted for wording only when `sources.json` records a VCDS run —
the one run-time read of that log, recorded as a deviation in the design's §4.4. **On a
VCDS-derived project the join has still never run**, so what it actually delivers is
unmeasured.

**`setup` is four options**, the first being both sources at once: ODIS for what to read
and how to scale it, VCDS for the wording. VCDS is read first — `tttext` writes
`names.json` wholesale while ODIS merges, so the other order would make one combined run
worse than the two runs it replaces.

## Status (2026-08-08) — ODIS becomes the primary source

**The car does not yet pick its own project.** `project::covering()` returns `None`; the
resolution order (`--project`, `VAGCAN_PROJECT`, covering, `config.json`, sole project)
is built and tested with that step empty. What blocks it is evidence, not code — see
next goals, item 4.

A VW ODIS-Service runtime project is now read natively — no Python, no PBL DLL, no
Java — and `setup` is a choice of *source* rather than a hardcoded VCDS path. Design in
`docs/superpowers/specs/2026-08-07-odis-project-store-design.md`, plan in
`docs/superpowers/plans/2026-08-07-odis-project-store.md`, formats in
`research/labels/odis-format.md`.

**The claim the whole branch rests on is measured, not asserted.** Three rows this
project proved *by driving the car* come back identical from the ODIS file with no
drive — including the endianness split that a single wrong guess would have hidden:

| DID | proven by driving | ODIS says |
|---|---|---|
| `F405` | `u8`, `raw − 40`, °C | 8 bits, `Linear{1.0, −40.0}`, °C ✓ |
| `206E` | `u16` **big**-endian, raw, /min | 16 bits, `be=true` ✓ |
| `380A` | `u16` **little**-endian, raw, /min | 16 bits, `be=false` ✓ |

**On the reference car, 13 of 15 control units resolve to an ODIS variant**, by the
`F19E`/`F1A2` join the label files already used — 6,669 channels found, **1,959
expressible** (1,691 before `RawForm` was widened). The two misses are real: the car
reports `EV_DCUDriveSideEWMAXCONT` and the project ships `EV_DCU2DriveSideMAXHCONT`, a
different unit rather than a failed match. Whole project: 633 of 717 variants yield
channels, 310,734 readings.

**`RawForm` was the constraint, and the measured part of it is closed.**
`RawForm::Int { byte_offset, byte_length, signed, big_endian }` says any whole-byte
field anywhere in a response, in either byte order and either sign — the signed 16-bit
**little**-endian shape that cost 146 channels, the 51 little-endian `u32`s, and every
field past byte 1. The seven original variants are kept, not replaced: `measurements/`
rows serialize by name (`"U16Be"`), a drive proved them and nothing else can recreate
them, so `RawForm::for_field` still answers with an old name wherever one fits. That is
1,691 → **1,959**, +219 of it on the engine and gearbox.

**Two things remain unsayable, and both are decisions rather than patches:**

- **2,693 sub-byte fields** — one-bit flags and 3-bit fields at bit 19. `RawForm`
  returns an `i32` read from whole bytes; a bit field needs a mask and a shift, and a
  one-bit flag needs a *name for each state* rather than a number, so the carrier
  question and the `Scaling::Enum` question are the same question.
- **A response holds several channels, and only the first is offered.**
  `Extracted::for_unit` and `merge` both key a channel by its DID, so of the 3,878
  fields now expressible, 1,959 survive. Lifting that is where the other 1,919 are —
  and it needs `watch`'s history and chart, keyed by `(request, did)` today, to be
  keyed by channel instead.

**Storage moved.** `~/.vagcan/data/<project_id>/{cache.sqlite, names.json,
rod-keys.json, measurements/, sources.json}`, with `.rod` files and the fault text in a
shared `~/.vagcan/rod/`. A project is a **platform** — VW's own mapping puts every
Octavia III, Karoq and Kodiaq under `SK37X` — so several cars share one, and
`cars/<VIN>/` still holds what is true of exactly one car.

**Two things recorded rather than resolved**, both cheap to close with the car:

- `catalog.rs` describes this car's reverse gear as `0C`; ODIS says `0C` is *Gear 9* on
  `0x210F` and reverse is `7`. The `0C` figure is a doc comment and a test, **not a
  proven row** — no measured file for that channel exists. Settled by selecting reverse
  and reading `0x210F` on `7E0` and `0x3816` on `7E1` (`research/labels/odis-format.md`
  §7.1).
- `watch` reported `measurements this project has proven: 7E0` on a machine holding **no
  proven rows at all** — the built-in OBD-II standard table was being labelled as
  proven. Fixed by a `proven` flag on the channel; worth knowing because it made a
  built-in table look like evidence.

## Status (2026-08-06) — prepared to go public

**Nothing the tool reads at run time lives in the repository any more.** `catalogs/` is
gone: the label cache, the recovered names and the `.rod` keys are rebuilt by
`vagcan setup` into `~/.vagcan/data/extracted/`, and the rows proven on a car live in
`~/.vagcan/data/measured/`. The two VCDS installations are vendored under `vendor/`
(zipped, Git LFS) so `setup` can fetch one when the user has none.

**`vagcan setup` is the whole install.** One command parses a VCDS installation — a path
you give it, or one it downloads (en/ru, with a progress bar) — into `~/.vagcan`, and
copies the raw label files in (`UDS_EV/`, `Labels/`, the fault text file, ~122 MB) so the
installation can then be deleted and `vagcan faults` names codes with no flag at all.

**Docs are split for a newcomer**: `README.md` (start here, install, first commands),
`USAGE.md` (every command with output), `ARCHITECTURE.md` (why, and VCDS's file formats),
`SAFETY.md`. A naive-user review signed off on the install path and the cross-links.

**`measure` second drive + the engine-channel fix**, and the **scaling audit** (label files
carries the values, not the join) are detailed in the 2026-08-05 status and the header
above. Research is reorganised by subject: `research/{labels,car,eps}/`.


## Status (2026-08-06, later) — the first run, walked end to end

A pass over what somebody meets before they trust anything, driven by a naive-user
audit and by the reference car's own output. Every item below is a defect that was
found and closed, not a plan:

- **`setup` could not recover names at all on a default install.** The `.rod` key
  search sat behind a `rod-crack` cargo feature that the README never mentioned, so a
  plain `cargo install` produced no `names.json`, and `vagcan vcds names` then told
  the reader to run the setup they had just run successfully. The feature carried no
  dependencies — it only hid code — and is gone.
- **The Russian build produced a broken installation, silently.** Ross-Tech names two
  files per language (`Codes.dat`/`Code-RUS.dat`, `TTTEXT.ROD`/`TTText-RUS.rod`) and
  both were matched by their English spelling alone, so choosing Russian — which
  `setup` offers as a first-class option — gave no names and no fault text. Both are
  candidate lists now, and an installation matching neither is offered as a file
  picker rather than declared broken.
- **A second installation layered on the first instead of replacing it.** The copy is
  freshness-gated per file and removed nothing, so the reference machine ended up
  holding Russian fault text over English labels, with nothing saying which was which.
  `setup` now reads which build a directory holds off the directory itself and clears
  it when the incoming one differs.
- **`--labels` is gone.** It dated from before `setup` copied the label files in, and
  its own justification ("naming a fault needs the installation's own `RD.rod` and
  `Codes.dat`, not just the parsed cache") stopped being true that day. What it left
  was a second place the tool could be reading from without the reader knowing.
  Switching installation is `vagcan setup` on the other one.
- **`cache.from` is gone**, and the fact it carried moved inside `cache.sqlite`: the
  file recorded the directory the cache was built from, which the copy step turned
  into a constant, and pointed at an installation setup itself says you can delete.
- **Setup stopped overstating what it achieved.** A run that read 63 % of the text
  table printed a bare `Done.`; it now reports `Step::Partial` and `Done, with gaps.`
  A number on its own reads as a total.
- **The two operations that ran for minutes in silence now spin**, with elapsed
  seconds: the key search (~160 s per sealed section, measured) and the label parse.
- **Sealed fault catalogues are offered once, not per unit.** A car with four of them
  printed the same recovery command four times; the whole set is now offered together
  after the listing, with the measured cost, and only when there is a terminal to ask.
- **"corpus" is gone from everything a reader sees** — it reads "label files" — and
  `vagcan vcds corpus` is `vagcan vcds dump`, since `labels` already meant the
  single-lookup command.

**Measured while doing it, and worth keeping:** those sections are *classic*, not
shifted — minutes, not the hours a shifted file costs. Opening all five sealed
sections of `EV_SMLSVALEOMQBLRH.rod` cost **483 s** at the start of the day and
**70.8 s** at the end, on the same M4, for **byte-identical keys** (compared as JSON
against the pre-change cache). Two independent wins: handing threads work from a
shared cursor instead of fixed slices, and — the larger one — noticing that the cheap
filter reads HDIST out of deflate byte 1 and then discards it, so the 128 candidates
are 8 groups of 16 differing only in bits the filter ignores, and the cascade was
being walked 16 times over. Measured on this machine: 192.9 s → 70.8 s for the second
alone (`research/labels/tttext2-sweep` grew a `classic <file> <TAG>` command to time
one section directly).
Decoding with a cached key is ~20 ms for the 2.2 MB `RD.rod` and below timer
resolution for a typical 1 KB per-unit file, so nothing on the live path is worth
caching further.

## Where it stands after the whole-car pass (2026-08-02)

The tool now reads **every control unit the car has**, not the two the ISO addressing
block reaches. On the reference car that is 15 units and 1206 identifiers
(`research/car/whole-car-survey.md`), and every previously unidentified unit named itself:
parking aid, steering assist, ESC, airbag, climate, both door modules, telematics,
media.

Done since the last update:

| what | where | note |
|---|---|---|
| whole-car sweep | `vagcan survey` | gateway list → every unit; identification, fault codes, nine identifier pages; `--diff` compares a parked and a driving run |
| fault reader | `vagcan faults` | confirmed codes only, sorted with what is failing now first, occurrence count, odometer, time of day, and a date stated as a bound |
| fault **names** | `vagcan faults` | VW's own words for a fault, out of `RD.rod` + `Codes.dat`; every code that cannot be named prints the reason instead (2026-08-05) |
| unit addressing | `vag-protocol::address` | two id blocks with different response rules; unit-number pairings live in `~/.vagcan/data/measured/unit-numbers.json`, not in the source |
| catalogs as data | `vag_data::catalog::CatalogStore` | one file per control unit under `~/.vagcan/data/measured/`, keyed by the part number the unit reports; nothing car-specific compiled in |
| label files unit labels | `LabelDb::unit_for_part` | `; Component: … (#02)` headers give an address and a name for 987 of 3035 label files |
| live view | `vagcan watch` | ratatui, several units at once, `/` filter over everything a survey found, actual/specified pairs on one line |

### The open work

1. ~~**Drive with the survey running.**~~ **The drive happened — 2026-08-02.** Two driving
   passes and their diffs are on disk (`research/dumps/survey-driving-20260802-{0314,0322}
   .jsonl`, `survey-diff-20260802-*.txt`): **272 identifiers moved** between parked and
   driving, across **13 units** — `70A 70E 710 712 713 714 746 74A 74B 767 773 7E0 7E1`,
   which includes the body control module and every unit listed as unidentified below.
   Per unit: `7E1` 96, `7E0` 77, `70E` 25, `714` 23, `710` 15, `746` 7, `712`/`767` 6,
   `773` 5, `74A`/`74B` 4, `70A` 3, **`713` only 1** — so the brakes are the one unit the
   drive did *not* open up, and they still need their own stimulus. What is left is not
   another blind drive but **mining what this produced**: a diff says an identifier is
   live, not what it means, and scaling still needs a reference for the unit in question.
2. ~~**Fault names.**~~ **Done (2026-08-05).** `vagcan faults`
   names them. The chain is fault number → `UDS_EV/RD.rod [DTC]` table → the row the
   unit's own `.rod` selects → a `Codes.dat` key; the per-table digit substitution that
   blocked it is generated by `srand(table key)` and two Fisher-Yates shuffles, read off
   `VCDS-ARM.exe`. 11 of this car's 15 confirmed codes, 57 of 57 on the three units whose
   `.rod` resolves, and word-for-word agreement with VCDS on all four codes both name.
   Full writeup, including everything refuted along the way:
   `research/labels/fault-naming-hop.md`. What is left is file *resolution*, not naming: the
   `INC` chain that leads from an ODX variant to the family file carrying `[DTC]`
   (§10.5), which is why the gateway and the two door modules still print numbers.
3. **The cluster's coolant scaling.** `22D0` has read `0xB8` = 90 °C in every sample ever
   taken; one cold start settles `×0.75 − 48` against `×0.5 − 2`.
4. **Brakes and body control.** `0x713` and `0x70E` answer 48 and 126 identifiers and own
   the signals a driver can provoke on demand — pedal, wheel speeds, lights, doors.
5. ~~**The clock's epoch.**~~ **Settled (2026-08-02).** There is no epoch: the stamp is a
   packed calendar date and time, in stored faults *and* at live `0x02BD`. The apparent
   free-running counter at `02BD` was raw subtraction of a packed field — the seconds
   field wraps at 60 in six bits, so a raw difference overshoots by 4 per minute boundary.
   Established against the instrument cluster's own clock across three sweeps; see
   `vag_protocol::dtc::CarTime` and `research/car/whole-car-survey.md` §2.3. What is *not* a
   protocol fact: this car's clock runs four days behind real time.

## Status (2026-08-05)

Two things closed since 2026-08-02, and both are worth stating before the older text
below, which remains true about scaling and stale about nothing else.

**Fault names ship.** `vagcan faults` prints VW's own words. The
chain and every refutation on the way to it are in `research/labels/fault-naming-hop.md`;
`research/labels/codes-dat.md` covers the text store it ends in. Zero wrong answers across every
check made, which is the property that matters more than the hit rate.

**`vagcan measure` exists** — an acceleration stopwatch: marks timed from the car's own
speed signal, a live full-screen view, a results table, a browser chart page, and a
`setup` that measures this car's road load by coastdown. Spec
`docs/superpowers/specs/2026-08-03-measure-design.md`, plan
`docs/superpowers/plans/2026-08-03-measure.md`. **It has had two real drives** — the
first (2026-08-04) found seven defects; the second (2026-08-05) surfaced an eighth, the
one that mattered most: every engine channel was being dropped because one unsupported
identifier in a batch voided the whole read (`split_records`, fixed 2026-08-06), so every
saved session before then reads `engine_speed: 0`. All fixed; the live screen also
became bars rather than a chart and the results table now reports every channel. None of
those latest fixes has been re-driven. It is code that should work, not code that is
known to.

**Architecture, measured rather than guessed**:
`docs/superpowers/specs/2026-08-05-architecture-design.md`. Phases 0–2 are done — an RAII
terminal guard, the dead-code sweep, `hex` in one place, `src/ui/` with `picker`, `term`
and `chart` in it. Its own arithmetic was corrected in the doing: the guard cost +328
lines, not −40, because a tested guard costs 150 lines of test.

---

The protocol stack, the identity reader, and the whole `.rod` label-decrypt pipeline are
built and merged. The offline path to measurement *scaling* is **audited to a sharper
conclusion** (2026-08-06, `research/labels/scaling-audit.md`): the label files **does** carry
the scaling values — MUX/DOP rows hold every proven factor and offset, and the earlier
"§4.0c: scaling is live-only" rested on a broken base-14 decode — but the **join** from a
measurement to its scaling or read DID is absent, and the read DID itself is not in the
label files under the *corrected* decode either. So scaling still comes from the car:
`vagcan vcds analyse` and `vagcan recording calibrate` turn recordings into proven rows,
and `~/.vagcan/data/measured/` holds 23 of them across engine, gearbox and cluster. Names
come from the label files — `TTTEXT.ROD` is cracked (`research/labels/tttext-codec.md`) — but
they are no longer shipped: `vagcan setup` rebuilds `names.json` from the installation.
The in-tool parser now carries the original solver's **word-frequency prior** (ported
2026-08-06), recovering **14,738 names** — 98.5 % agreement with the old 17,009-name
oracle on their ~7,857 shared ids, plus 6,881 the oracle never had. The gap is two
comparable catalogs differing on known-hard residuals (numeric separators, one-shot
acronyms, cluster-pattern collisions), not a bug. The label files have **no name→DID join**,
so a `vagcan vcds names` hit is a hypothesis to test live. The one door still unopened is
`TTTEXT2.ROD` (a bounded multi-hour sweep; its driver is committed under
`research/labels/tttext2-sweep/`, the sweep itself unrun).

The adapter works on the car. `vagcan info` matches the Auto-Scan oracle, `vagcan survey`
walks every unit the gateway lists (identification, stored DTCs, identifier sweep), and
`vagcan watch` is a full-screen multi-unit TUI. What remains for M3 is **coverage**: the
open-work list at the end of this file.

### Milestones
| M | what | state |
|---|------|-------|
| M0 | ISO-TP + UDS + transport stack (read-only allowlist) | ✅ done |
| M1 | `vagcan info` — VIN + Engine/Gearbox identity (UDS RDBI) | ✅ **verified on the real car 2026-08-01** |
| M2 | `.rod` decrypt+inflate in-tool; STRUC/DOP/TTTEXT/MWB cracked; base-14 codec proven; `vagcan vcds labels` | ✅ done |
| **M3** | measurements → `MeasurementDef` catalog → generic CAN reader → config-selectable | 🟡 **23 catalog rows proven (engine 3, gearbox 12, cluster 8) + three OBD-II services decoded from the standard; the gear and selector are read as states; open work = whole-car coverage** |
| M4 | fault **names** — number → `RD.rod` → row → `Codes.dat` → text | ✅ **done 2026-08-05**; 11 of this car's 15 confirmed codes, word-for-word with VCDS on all four both name, zero wrong |
| M5 | `vagcan measure` — acceleration timing, power, chart page | 🟡 **built, two drives** (2026-08-04/05); eight defects found and fixed — incl. every engine channel dropped by `split_records` until 2026-08-06 — latest fixes not re-driven |
| HW | generic USB-CAN (MKS CANable) bring-up on the car | ✅ live on the car: reads + writes at 500k |

### Done (merged to `master`, tests green, clippy clean)
| subsystem | crate | what |
|-----------|-------|------|
| async-core | vag-transport | async transport trait(s) + mock, error model |
| uds-async | vag-protocol | async ISO-TP (15765-2) + UDS client (14229), read-only allowlist |
| generic-can | vag-can | `SlcanBackend` + `IsoTpCan` (the bypass transport — built, untested on hw) |
| info-identity | vag-protocol/vagcan | `EcuIdentity` + `read_identity` + `vagcan info` (Engine 01 + Gearbox 02). **Live-verified on the car** |
| can-sniff | vag-can/vagcan | `SlcanMode::Silent`, passive `IsoTpSniffer`, `vagcan sniff` |
| scan | vagcan | `vagcan scan` — group-testing sweep of the identifier space; `vagcan properties` |
| odx-link | vag-data/vagcan | `find_rod_by_odx_name` + `labels --from-car`: the unit names its own `.rod` (F19E) |
| label files | vag-data/vag-db | `.lbl`/`.clb` parse+decrypt, `.rod` decrypt+inflate, `LabelDb` lookup, `load_label_files`/`scan_label_files` |
| rod-crack | vag-data | `.rod` TEA-CBC + product/IV recovery in-tool (`vagcan vcds rod`); STRUC/DOP/TTTEXT/MWB inflate; **base-14 codec proven (disasm)** |
| struc-table | vag-data | `StrucTable`/`StrucRecord` + `decode_base14_be`; `mwb` parser; `measure` (proven ignition `0x5555`→0.0° anchor) |
| labels-cli | vagcan | `vagcan vcds labels` — label files inventory + `--part` / `--block` lookup; SQLite cache at `~/.vagcan/data/extracted/cache.sqlite` (`--refresh` rebuilds); the IV brute force is built in, and only `vagcan vcds rod` and `vagcan setup` run it |
| addressing | vag-protocol | `address.rs` — `UnitAddress`: ISO block `7E0..7E7` → +8, VW block `700..7BF` → +0x6A; fixes `--ecu 17` resolving to `0x7F0` (nothing) instead of the cluster `0x714`; short numbers only for evidenced units (01/02/09/16/17), everything else by request id |
| survey | vagcan | `vagcan survey` — walk the gateway's installation list (plus engine/gearbox/gateway, which it never contains): identification, stored DTCs (`19 02 FF`), then the identifier bands in use on this car; JSON lines per unit; silent units skipped after ident |
| watch-tui | vagcan | `vagcan watch` — full-screen ratatui TUI, multi-unit, reconfigurable in place (`c`); `--survey FILE` offers everything a survey found; actual/specified pairs on one line; unconverted CSV columns suffixed `_raw` |
| calibrate | vagcan | `vagcan recording calibrate` — offline; fits `_raw` columns against trusted reference columns in the same `watch --out` recording |
| names | vagcan | `vagcan vcds names` — substring search over `~/.vagcan/data/extracted/names.json`, the names `vagcan setup` recovered from TTTEXT; a match is a hypothesis, the label files have no name→DID join |
| cli-app | vagcan | Top level, all needing the car: `devices` / `info` / `units` / `properties` / `sniff` / `sensors` / `watch` / `scan` / `survey` / `faults`. Offline work is grouped by what its input is — `recording calibrate|discover` over our own recordings, `vcds labels|names|analyse|rod|label files|tttext` over VCDS's files (2026-08-03) |

## M3 — measurements (the current work)

### How VCDS reads measurements on this car (MQB / UDS)
The ECU's `.rod` (`EV_ECM…rod`) is VCDS's source of truth: each measuring value =
`{ read DID, COMPU-method ref → DOP scaling, text-id → TTTEXT name }`. VCDS issues
`UDS 22 <DID>`, applies the COMPU method, prints `name = value unit`. Groups (`G004…`)
bundle DIDs. That was the original target — decode the ECU `.rod` into
`(DID, scale, unit, name)` — but the label files turned out not to carry the DID or readable
scaling (below), so this project gets the *names* from the label files and proves the
*(DID, scale, unit)* part on the car.

### What is done vs the wall
- ✅ `.rod` TEA-CBC + zlib + the per-record `product`/IV blocker — **defeated offline**
  (DEFLATE header oracle + Kraft pruning + inflate confirm); `STRUC.rod` inflates to
  293,560 bytes in our own tool.
- ✅ All four tables located + cracked: **STRUC** (1221 structure ids), **DOP/TTDOP**
  (17,636 COMPU/scaling ids), **TTTEXT** (names), **MWB** (engine measuring rows).
- ✅ Payload codec proven **base-14** over charset `0123456789,.-_` (disasm at
  `0x1401898b0`, mod-14 arith `fcn.1400e6f80`).
- 🔴 **NOT reversed: STRUC field segmentation** — where inside a `NNNNNN,<base-14>`
  record the `read_id (DID)` / `raw-spec` / `scale` / `unit-ref` / `name-ref` live.
  Offline static + data-only RE is exhausted (5 passes; base-40 `code→id`, fixed-column,
  per-byte index all refuted — `research/labels/rod-labels.md`).

### The supervised STRUC × crib attack — DONE, refuted
Crossing the capture crib's real DIDs with the decoded STRUC table was the M3 lever. It
ran end-to-end and produced a clean negative: the read DID is **not stored in STRUC** in
any tested encoding, `STRUC-id` is not the IDE measurement id, and `IDE-id` is not the
MWB row index (`research/labels/rod-labels.md` §4.0c). Do not re-run it.

### The lever that worked — sniff VCDS on the bus (the live crib)
Every prior crib came from USB captures of the HEX clone, where the link cipher hides the
payload and VCDS's **group reads** — the source of RPM / vehicle speed / coolant — never
decoded (§4.0a/§4.0b). CAN is multi-drop, so a second adapter can sit on the same OBD-II
bus in listen-only mode while VCDS runs a normal session and record the whole conversation
**in the clear**, multi-frame group reads included.

Tooling (built, `docs/superpowers/specs/2026-07-31-can-sniffer-design.md`):
- `vagcan sniff --out cap.jsonl` — listen-only by default; streams every frame
  to a `vag-capture` JSONL headed by a wall-clock anchor, reassembles ISO-TP live, and takes
  operator markers from stdin. The anchor exists because the capture↔CSV lag had to be
  *guessed* last time (~52 s), which is how several "correlations" turned out to be
  window-fishing.
- `vagcan scan --ecu 01` — read-only sweep of the RDBI space; finds what the ECU exposes
  regardless of what any label file names.
- `vagcan properties --ecu 01` — the identification range, named.

The pairing — sniff + VCDS running ADVMB logging to CSV, engine running, a wide rev —
was collected on 2026-08-01 and yielded `(read address → raw bytes → displayed engineering
value)` directly, with no dependence on the `.rod` field codec. See "The remaining work"
item 1 below.

### Overnight results (2026-08-01→02, four parallel analyses)

All from data already on disk; the car was not attached.

**The car enumerates itself.** The gateway's installation list (`0x2A26`, also `0x04A3`)
is a 32-byte bitmap, LSB-first, indexed by `id − 0x700`. One read replaces sweeping
`0x700..0x7BF`. Verified before use: all seven units separately observed answering appear,
with no false negatives; the opposite bit order finds two of seven. Shipped as
`vagcan units`. Nine units answer; four are identified by their own identification block
(steering column `5Q0 953 521 KM`, body control `5Q0 937 084 CF`, gateway `3Q0 907 530 B`,
cluster `5E0 920 740 D`).

**Two more OBD-II services are mirrored.** `F6xx` is service 06 and `F8xx` is service 09 —
proven by content, not convention: `F802` holds the VIN, `F804` the calibration identifier
`8V0264H 0005AEAJ`, `F80A` the string `ECM\0-EngineControl`. Service 09 now decodes in
`vagcan properties`; it carries what `F1xx` cannot, namely which emissions calibration the
unit is actually running.

**The engaged gear, without a VCDS log.** `0x3816`, proven by arithmetic against the
already-proven shaft speeds (η² = 0.972, runner-up 0.072). `gear = code − 1`; `0x0C`
reverse, `0x00` not engaged. Selector lever at `0x3809` (P/R/N/D). Both are represented
with the new `Scaling::Enum`, because a gear is not a quantity — a linear scaling would
report reverse as "gear 11".

**Odometer, by exact hit.** Cluster `0x2203` returned `03 3F 18` while a log read
212,760 km. `0x033F18` = 212,760.

**A defect in our own tool.** `watch --out` wrote one timestamp per row, but identifiers
are polled in batches, so columns are up to a polling cycle apart. Every value now carries
its own time column — the same thing VCDS's export does, and which this project already
parsed correctly for VCDS while producing the flawed version itself. Correcting it lifted
the gear evidence from η² 0.872 to 0.972.

Writeups: `research/car/identifier-map.md`, `research/car/other-ecus.md`,
`research/car/gearbox-state.md`.

### What the next session should do

1. **Re-sweep the engine with it running.** 450 of 896 identifiers read zero because the
   sweep was taken engine-off — very likely "no signal", not "unimplemented". Cheapest
   improvement available.
2. **A parked identification pass driven off the gateway list** — shipped as
   `vagcan survey`: reads `2A26`, walks every listed unit (plus engine/gearbox/gateway,
   which the list never contains), reads identification, stored DTCs and the identifier
   bands in use on this car, one JSON line per unit. Run it parked.
3. **Record the gearbox `3820–38FF` block while driving.** Every proven clutch row lives
   there and none of it has been recorded moving.
4. **Select S and paddle-shift deliberately.** The lever moved through P, R, N and D during
   the recording — that is what proved the selector at `0x3809` (P 76 samples, R 48, D 294;
   N only 4, hence flagged weak). What is missing is the **drive mode**, which on a DQ200 is
   a separate signal from lever position: D versus S versus manual. It was never selected,
   so the stimulus is absent, not the signal. Also worth holding N for ten seconds to settle
   it properly.
5. **A cold start** while polling cluster `0x22D0`, the one action that converts its
   coolant reading from "consistent with" into measured.

### The remaining work, in order

**1. The capture session — DONE 2026-08-01, and it worked.** 308 s of listen-only capture
alongside a live VCDS session; `vagcan vcds analyse` proved three scalings, one of which
(coolant = `raw − 40`) reproduces the standard OBD-II PID 05 formula and thereby validates
the whole pipeline. Details in `research/labels/rod-labels.md` §4.3.

**1a. More coverage — DONE, and it was already on disk (established 2026-08-05).** The
claim this item used to make — "the logs were only ~20 s each, giving 14–16 matched points
against a 20-point default, so record several minutes per group" — was **wrong about the
data, not about the bar**. The short logs were crossed; the long ones were not. A 956 s
capture, `research/logs/1.jsonl` (01:47:48 → 02:03:44 on 2026-08-01), spans four VCDS logs
in `research/logs/`, and crossing it with them now gives:

| log | matched points | scalings at R² = 1.00000 |
|---|---|---|
| `LOG-01-IDE00025_&10.CSV` | 102 | 8 (engine) |
| `LOG-02-IDE00022ENG103074_&11.CSV` | 59 | 10 (gearbox) |

**No new catalog row comes out of it, and that is the point.** All 18 are already shipped:
ten gearbox and three engine rows are in the project's `measurements/`, and the other five are
standard OBD-II PIDs mirrored at `F400 + PID`, already in `vag_data::obd` and correctly
*not* in any car file — `F405` = PID 05 coolant `raw − 40`, `F40D` = 0D speed, `F40F` = 0F
intake air `raw − 40`, `F423` = 23 fuel rail `raw × 10`, `F446` = 46 ambient `raw − 40`.
So the run is an independent reconfirmation of the whole pipeline against VCDS's own
displayed values at 102 points, not new coverage.

What the archive genuinely cannot supply, and why more logging alone will not fix it:
- **The cluster log has zero overlap** with the capture (`LOG-17-IDE00025_&3.CSV`), and
  `1.csv` — 12 engine measurements including fuel pressure, throttle mass flow, accelerator
  pedal and atmospheric pressure — starts 210 s **before** the capture. Both are orphaned:
  a log with no capture beside it proves nothing.
- **Three measurements need a stimulus, not a longer log.** Control-unit temperature
  (3 distinct values), atmospheric pressure (2 — it needs a change of altitude) and
  `IDE00588` (3) never varied enough to fit. Same for the cluster's coolant, flat at
  `90.00` across all 45 samples — that is the cold-start item, not a logging-length item.

**2. `vagcan vcds analyse` — BUILT (2026-08-01).** The offline tool that turns a capture into
scalings, written before the session so the data can be checked while the car is available.
It:
- reads the capture JSONL and the VCDS CSV (CP1251; each measurement carries its own time
  column) and aligns them by the **wall-clock anchor** — a subtraction, never a search;
- reassembles ISO-TP, pairs `0x22` requests with `0x62` responses, and splits
  multi-identifier responses by the requested order, **skipping** any response it cannot
  split unambiguously;
- fits `factor`/`offset` by least squares over every raw interpretation, accepting only
  `R² ≥ 0.995` over `≥ 20` points and reporting near misses as leads;
- emits accepted rows as `MeasurementDef` catalog entries, which `UdsReadExt::read_catalog`
  already reads.

Exercised against real capture+log data on 2026-08-01: it found the three scalings above
and rejected a two-level false positive, which is what the guards are for.

**3. Names from the `.rod` — DONE.** Scaling comes from the car, and after the linkage
attempt (`research/labels/label-linkage.md`, `research/labels/rod-labels.md` §4.4) that is settled: the
label files hold **no linear coefficients**, its values are base-10 under a per-table glyph
substitution, and the `MWB` code is a global function of the text-id with no per-ECU degree
of freedom. So the label files are for **names and per-ECU lists**, nothing more.

The name table itself is cracked (`research/labels/tttext-codec.md`): `vagcan setup` now
rebuilds the project's `names.json` from the installation rather than shipping
the file, and the in-tool parser carries the word-frequency prior (2026-08-06), recovering
**14,738 names** — comparable to the original solver's 17,009 (98.5 % agreement on shared
ids, 6,881 the oracle lacked). Searchable with `vagcan vcds names <text>`. The `ENG######`
question
is settled — the number **is** the `TTTEXT` text-id, proven four for four on records solved
blind (`research/labels/tttext-codec.md` §2, superseding `research/labels/label-linkage.md` §4's
"suggestive, not established"), and the recovered names are English text — the
`ENG`-means-*English* reading, not *engine*. That closes the chain
*proven identifier → IDE → ENG → name* for gearbox rows whose `IDE` the VCDS log prints —
but only for those. The label files themselves carries **no name→DID join**: `MWB` has no per-ECU
identifier, so a `vagcan vcds names` hit is a hypothesis to confirm on the car, not a binding.
`mwb.rs` is deliberately kept for the possible MWB→TTTEXT name join.

**4. `vagcan watch` — BUILT, now a full-screen TUI.** ratatui, multi-unit, reconfigurable
from inside (`c`); `--survey FILE` offers every identifier a survey found; actual/specified
pairs (e.g. boost `0x2029` specified / `0x202A` actual) render on one line. Polls live at
bus speed (46 Hz measured on the boost set) using batched reads. Anything unproven prints
its bytes tagged raw, and unconverted columns are written to CSV with a `_raw` suffix —
which is what `vagcan recording calibrate` fits against the trusted columns in the same recording.

**5. Discrete state — `vagcan recording discover` BUILT; gear and selector identified.** Gear,
gearbox mode, switches and lamps cannot be fitted: a two-level value fits any line
exactly. `discover` classifies a `watch --out` recording into never-moved / stepped /
continuous and ranks the stepped columns, with `--pairs` for candidates whose transitions
coincide. The gear (`0x3816`, η² = 0.972 against the proven shaft-speed ratio) and the
selector lever (`0x3809`) are identified and in the project's `measurements/0CW300041G.json` as enums.
Still to do: the drive mode (D/S/manual — never selected during the recording, so the
stimulus is missing, not the signal), and the same treatment for the other units.

### Then — the extensible foundation (architecture)
```
MeasurementDef { name, unit, address: Uds(did) | Group(g,field), raw_form, scale }
MeasurementCatalog                                                  // data, not code:
    names/lists from the label files, scaling from live calibration
read_measurement(&def, uds) -> (name, value, unit)                 // one generic path
```
Add a parameter = a data row / config selection, never new match-arms. Scaling is proven
empirically from a live crib (`analyse` / `calibrate`) — the label files provably cannot supply
it; names and per-ECU lists are what the label files are for.

## Hardware checkpoints (STOP, confirm on the real car)
Dongle: **MKS CANable V2.0 Pro** (STM32G431 + ADM3050E isolated) — fits `vag-can`'s
`SlcanBackend`, no new backend.

**Bench bring-up: DONE (2026-07-31).** It enumerates as CDC-ACM (`16d0:117e`,
`/dev/cu.usbmodem*`), so the firmware is **slcan**, not candleLight — no reflash. It answers
`V` and `E` and stays responsive, but acks nothing else; its whole command set is
`O C S Y M A V E t T r R d D b B X` — no `L`, no `N`, no `F`, **no loopback**. Listen-only is
`M1`, not `L`. Since it has no loopback and CAN needs a second node to ACK, TX/RX **cannot**
be proven on the bench (`crates/vag-can/examples/slcan_probe.rs`).

Before touching the car: wire OBD2 pin 6→CAN-H, 14→CAN-L, 4/5→GND, **do NOT** wire pin 16;
**open the 120R jumper** (the vehicle bus is already terminated at both ends, ~60 Ω; a third
resistor drags it to 40 Ω); leave **BOOT** open (DFU only).

Risk climbs monotonically — stop and confirm at each step:
1. ✅ **`vagcan sniff`, no VCDS.** Zero risk: listen-only cannot even ACK.
2. ✅ **`vagcan sniff` + VCDS in parallel** — done 2026-08-01, 308 s captured alongside a
   live VCDS session; see "The remaining work" item 1.
3. ✅ **`vagcan info`** — done 2026-08-01, see below.
4. ✅ **`vagcan scan`** — full `0000-FFFF` sweep of both units, 2026-08-01: the engine
   answers **896** identifiers (191 s, 10,840 requests), the gearbox **541** (274 s, 9,406
   requests). Results in `research/dumps/*-full.jsonl` (gitignored).

### First live session — 2026-08-01 (M1 CLOSED)
`vagcan info` over the CANable read the car and matched the Auto-Scan oracle on four
independent points: VIN `XW8AD4NE9JH008917`; Engine `8V0906264H` (the very part whose
`EV_ECM18TFS0208V0906264H.rod` the label work is built on) / HW `06K907425B` (the
`06K-907-425-V1/V2.clb` pair) / `1.8l R4 TFSI`; Gearbox `0CW300041G`, `GSG DQ200G2_M`,
SW `1003` — the same `1003` the old USB capture crib yielded.

What the bus actually looks like, measured rather than assumed:
- **The OBD-II diagnostic line is nearly silent.** 8 s of listening yields ~46 frames, all
  one periodic extended id `0x17F00010` (~6 Hz) from the gateway. So *silence is not
  evidence of a fault* on this platform, and `bus_doctor`'s functional-address probe is the
  test that discriminates.
- **Physical addressing only.** `0x7E0/0x7E8` answers; the functional broadcast `0x7DF`
  times out — the VAG gateway does not serve it on OBD.
- Rates other than 500k produce nothing, as expected.

### Sweeping is a group-testing problem, not 65,536 reads
Measured on the reference car, and the reason a full sweep is minutes rather than hours:

- A multi-identifier `0x22` request is answered with **only the identifiers the unit
  supports** — asking for `F190` (supported) together with `0001` (not) returns just
  `F190`. The unit refuses with `0x31` **exactly when it supports none** of them.
- That makes one request a **presence test for a whole batch**: a refusal skips the batch
  outright, a positive answer is halved until the responders are isolated and read singly.
  On `F100-F1FF` this finds the same 22 identifiers in **118 requests instead of 256**; over
  the sparse rest of the space the saving approaches the full batch factor.
- **The per-request limit is between 8 and 12 identifiers** on this unit: 8 are answered, 12
  are refused with `0x31`. Exceeding it is a *silent, total* failure — every batch looks
  empty and the sweep cheerfully reports zero hits. It did exactly that at batch 16 before
  the limit was found.
- Therefore `scan` probes with a **full-size batch** (one known-good identifier padded with
  impossible ones) before trusting group testing, and falls back to one-at-a-time when the
  probe fails. A token two-identifier probe would have passed and hidden the bug.

Debugging note worth keeping: the adapter can enumerate on USB (correct VID/PID/serial in
`system_profiler`) while macOS attaches **no** serial node — `/dev/cu.usbmodem*` simply is
not there and every open fails with "No such file or directory". That is a USB-stack hang,
not a bus fault; a full unplug/replug (power-cycling the MCU) restores it. Check
`ls /dev/cu.usbmodem*` before believing any "the bus is dead" result.

**Validation oracle:** the owner's full Auto-Scan is in `archive/research/vcds-rus-crack.md`
(VIN `XW8AD4NE9JH008917`, every ECU part-number/coding/VCID) — golden fixtures.

## The open work (M3 coverage and beyond)

- **`deflate_anchors` cannot open a fixed-Huffman section (2026-08-06).** Its 60-anchor
  set covers `BTYPE = 2` only, on a comment that claimed no section used anything else.
  A census of the whole corpus refuted that: **1,559 of 22,107 classic sections (7.1 %)
  open with a fixed block** (`0x33`/`0xb3`), so roughly a thousand *shifted* sections are
  closed to today's tooling — not slow, unopenable, and for an unrecorded reason until
  now (`research/labels/tttext2.md` §5, `crates/vag-data/src/rod/mod.rs`). The research
  driver carries `--all-btypes` as the widening; the shipped searcher does not, because
  admitting them doubles the search. **No car needed.** Cost of being wrong here is that
  a car naming one of those files gets "sealed" forever with no way to tell it apart from
  a section that merely has not been searched yet.

- **A car keeps its own files.** `~/.vagcan/cars/<VIN>/` (`crate::datadir`) holds
  `car.json`, `measures/` and, since 2026-08-05, **`survey.jsonl`** — the whole-car survey,
  written by every `survey` run whether or not `--out` was given, and loaded by `watch` with
  no flag. That closes the defect where `watch` showed three control units of fifteen:
  the other twelve were only reachable through `survey --out FILE` plus `watch --survey
  FILE`, which is two commands and a remembered file name, so nobody ran them. A run with
  `--only` **merges** into the cache rather than replacing it (`survey::merge_survey`), so
  the one-unit-at-a-time habit `SAFETY.md` asks for does not cost the other fourteen.
  Still open: **`faults --details` keeps nothing** — it should file its dump under the car
  the same way. `watch` deliberately does **not** offer to run the sweep itself: it holds
  the adapter open and a sweep is the one operation on this car that has hurt it, so it
  prints the single command instead (`vagcan survey`, parked) and leaves the decision with
  the driver.

- **Whole-car measurement coverage.** The catalogs cover engine, gearbox and cluster; the
  survey reaches every unit, and the 2026-08-02 driving diffs already say **which** of its
  identifiers are live — 272 of them across 13 units (§"The open work" item 1). So for most
  of the list below the missing step is a **scaling reference**, not another sweep:
  - body control module `0x70E` — lights, doors, indicators; 25 identifiers moved on the
    drive, and these are the signals a driver can provoke on demand, so a `watch --out`
    recording with deliberate stimulus can scale them without VCDS at all;
  - cluster `0x714` — a **cold start** while polling `0x22D0`, the one action that turns
    its coolant scaling from "consistent with" into measured. Not obtainable from any
    archive: every cluster sample on disk reads a flat `90.00 °C`;
  - the unidentified units `0x712` / `0x713` / `0x715` / `0x746` / `0x74A` / `0x74B` /
    `0x767` / `0x773` (`research/car/other-ecus.md`) — all but `0x715` already appear in the
    driving diff, so what they answer and what of it is live is known; naming and scaling
    are what is left;
  - deeper engine and gearbox coverage (the `3820–38FF` gearbox block while driving, the
    engine re-swept with the engine running).
- **Unit addresses from the label files.** Done for the half the label files can answer: the
  numbering (`44` is a power steering unit, and what it is called) now comes from the
  label files' `; Component: … (#44)` headers — 73 numbers, extracted once by
  `LabelDb::unit_numbers` — and is injected into `vag-protocol::address::install` by
  the commands that load label files. The five built-in pairings are the fallback, behind
  the override file and the label files. **Still open:** which CAN request id a number is
  answered on is in *no* label file — the two numberings are unrelated (`17` answers on
  `0x714`, whose own UDS address is `0x14`; `19` on `0x710` — `research/car/other-ecus.md`
  §3) — so that half is learned per car by `units --identify`, which asks each
  id for its part number and the label files whose part number that is, and is lost when the
  process exits. A per-car cache of learned pairings would keep it.
- **Electrical-system and brake channels.** Nothing from ABS/brakes or the electrical
  system is in a catalog yet.

## Dead and archived (kept as negative results — do not retry)

- **Measurement names from a masked (`shifted`) text table — 2026-08-06.** Not slow:
  out of reach. Such files XOR an 8-byte mask over the finished IV, and that mask is a
  **runtime global inside VCDS** — read off the binary at `0x140033b70`, and confirmed
  from the outside by 348 distinct values across 349 files matching nothing structural.
  The files do not carry it, so no amount of analysis recovers it. Because the mask is
  8 bytes it reaches `IV[3..8]`, which costs both the free deflate anchor and the
  multiplicative reduction of the candidate sets: 60 anchors against the full 2⁴⁰ space,
  ~960× the work, hours per file. Measured corpus-wide: the unmasked half is ~39 h of
  CPU, the masked half ~5.2 years. **The Russian build's `TTText-RUS.rod` is masked**,
  so that build gives fault text and labels but no measurement names, and `vagcan setup`
  now says so up front instead of spinning. The only route that would change this is
  lifting the mask out of a running VCDS process, which is a Windows-debugger job and
  not an offline one (`research/labels/tttext2.md` §3.3a, §3.5).

- **HEX-clone live UDS** — the session KDF is VMProtect-sealed and dead. The `vag-hex`
  crate and the vendored FTDI D2XX driver are **deleted**; the research writeups moved to
  `archive/research/` (`vag-hex-framing.md`, `clone-crypto.md`, `vcds-rus-crack.md`) and
  stay authoritative as negative results. The clone capture decoder
  (`research/clb-crack/extract_uds.py`) stays useful as an offline crib source.
- **Scaling from the *VCDS* label files** — refuted structurally, twice over
  (`research/labels/rod-labels.md` §4.0c, `research/labels/label-linkage.md` §3/§5).
  **Still true, and no longer the whole story (2026-08-08):** the refutation is about
  what a `.rod`/`.clb` label file contains, not about files in general. A VW ODIS
  project declares the entire chain — identifier, offset, length, byte order, compu
  formula — per ECU variant, and three rows this project had proved *by driving* came
  back identical from it with no drive. Read this entry as "VCDS cannot supply a
  scaling", never as "a scaling can only come from a drive".
- **OBD-II Mode 01 as the product path** — dropped. The standard sensors survive as
  `vagcan sensors` and as calibration references, not as the measurement model.
- **`MUX.rod` as the measurement registry** — opened 2026-08-04 and it is not one. It is
  the ODX multiplexer table, a leaf of the `STRUC` subgraph a car cannot enter, with no
  read identifier by four independent tests and a median table of three rows.
  `research/labels/mux.md`. No decoder ships: the only way in is a `STRUC` id and nothing a
  control unit reports yields one.
- **Pooling the `RD.rod` digit substitution across tables** — refuted 2026-08-05, then
  made irrelevant. 95 solved tables have 95 distinct alphabets, so there was nothing to
  intersect; the alphabet turned out to be *generated* from the table key by
  `srand(key)` and two shuffles, read off `VCDS-ARM.exe`
  (`research/labels/fault-naming-hop.md`).

### Open, and bounded

- **`TTTEXT2.ROD`** is the whole of `research/labels/label-linkage.md` §7 item 3 — whether the
  `.rod` label files are names-and-lists-only. It is now a **bounded 5–11 h unattended sweep**
  rather than an unknown: its `[CMP]` section is exempt from the shifted-IV regime, so its
  anchor byte cannot be narrowed and all 60 legal values need the full space
  (`research/labels/tttext2.md` §4.2). Nobody has started it.
- **A per-car cache of learned unit pairings.** Which CAN request id answers a unit number
  is in no label file — the two numberings are unrelated — so it is learned per car by
  `units --identify` and lost when the process exits. `~/.vagcan/cars/<VIN>/` is
  where it would live.

## The command surface after the ODIS pivot (2026-08-09)

Raised by the owner: *"нам точно нужны эти команды? сейчас `survey` ничего интересного
и нужного по идее не выдаёт"*. Mostly right, and the reason is the pivot — a project
declares 310,734 channels with names and scalings from file, which is exactly what
`survey` used to be the only way to learn.

What `survey` did, and who owns each part now:

| what it produces | who owns it after the pivot |
|---|---|
| which identifiers a unit answers | **the ODIS project** — from file, no car |
| which of those actually *change* — the parked/driving diff (`survey.rs:229`) | **only `survey`**. The file declares; the car decides. Nothing else can tell a live channel from a declared one |
| the unit list + identities that give `watch` its tabs | **only `survey`**, and that is the blocker below |
| stored faults on every unit (`survey.rs:532`, mask `0xFF`) | **duplicates `vagcan faults`** — two commands, one job |

So `survey` is not deleted. It is taken apart, in this order — the order matters,
because today it is load-bearing:

1. **`watch` must get its unit list from the gateway, not from a survey.** It walks
   only `preselect + ENGINE` (`crates/vagcan/src/watch/mod.rs:1882`), so every other
   unit on screen comes from `~/.vagcan/cars/<VIN>/survey.jsonl`. `crate::units`
   already reads the gateway's installation list and already identifies units — this
   is wiring, not new capability. Until it is done, deleting or narrowing `survey`
   takes fourteen of the fifteen units off the screen with it.
2. **Then drop the fault read from `survey`** and let `faults` own it. Keep the
   confirmed-only filter that `faults` has and `survey` lacks.
3. **Then narrow what is left** to what only a car can answer: the inventory and the
   diff. That is a command worth keeping and a much smaller one to guard.

Two smaller findings from the same look:

- **`properties` and `units --identify` are one command in two places.** `units` reads
  four identifiers per unit; `properties` sweeps `F100-F1FF` on one. Merge into
  `units --identify <ecu>` by capability, per the cleanup rule — the deep sweep is the
  survivor's mode, not a second command.
- **`properties` sweeps 256 undeclared identifiers with no `require_stationary`.** It
  carries an anomaly monitor and the comment at `crates/vagcan/src/main.rs:1052`
  argues the case: the identification block is standardised and 256 wide. That is a
  defensible line, but it is the only sweep-shaped path without the guard, so it is
  written down rather than left to be re-discovered.
- **`sniff` and `recording` stay as they are.** `sniff` is listen-only and is the only
  transport-level tool for "nothing answers"; the live-crib strategy it was built for
  lost priority to ODIS, but the command costs nothing. `recording calibrate` gained
  value from the pivot rather than losing it: it is how an ODIS scaling gets confirmed
  against a real drive, which §4.5's trust order requires.

## Deferred, from the owner's own runs (2026-08-09)

Raised while using the tool, not blocking, and written down so they are not
re-discovered:

- **A one-letter path typo reads as a missing directory.** Typing `~/Dowloads/SK37X`
  gets "is not a directory — there is nothing at that path", which is true and
  unhelpful: `Downloads` exists one letter away. The refusal already does a
  did-you-mean for *contents* (it will spot a project inside the folder you named); it
  should do one for the *name* too, against the siblings of the deepest parent that
  does exist. **Not** a trailing-slash rule — a slash changes nothing here, and the
  successful second attempt differed by the missing `n`, not by the slash.
- **`setup`'s variant walk is serial.** `[1/2]` walks 717 variants one at a time and is
  the long pole (minutes on release, ~10× that on a debug build). The pools are
  independent and read-only, so this parallelises without a shared-state question.
  Worth a word about the debug/release difference wherever a user could read the wait
  as a hang.
- **The closing footer's calibrate advice may be noise on the ODIS path.** It is
  correct under the trust order (§4.5) — an ODIS row is evidence until a drive confirms
  it — but telling somebody who just imported 310,734 declared scalings to go and
  measure them reads as though nothing was gained. Decide whether the sentence belongs
  at the end of a `setup` run at all, or only where a channel is actually used.

## Next goals (2026-08-09)

Ordered, and each says whether the car is needed — that single fact decides what can be
done tonight.

**No car needed:**

0. **The ODIS fault loaders.** The largest functional gap: a project holds 329,268
   `DTC_*` objects with their descriptions in the clear, and `vagcan faults` still reads
   VCDS files, so a person set up from ODIS alone gets numbers. `DB_DOP_DTC` and
   `MCD_DB_DIAG_TROUBLE_CODE` are in the type table; `odis/loaders/` has only
   `identity.rs` and `measurement.rs`. Plan Tasks 15–16 (faults, then topology).
   Research the chain in the real project first, as the measurement chain was.
   Two things to report as findings, not assume: which **language** the text is in
   (this project is `deu`, and a user whose faults arrive in German after VCDS gave
   them English needs telling), and whether the **freeze-frame** fields are reachable —
   `SAFETY.md` prescribes reading one before touching anything after a unit misbehaves.
0b. **Measure the naming join where it was meant to work.** It has never run against a
   VCDS-derived project: the wording preference was written, measured on an ODIS-only
   project where it made names *worse*, and fenced off by provenance. What it actually
   delivers on a VCDS project — how many of the 1,209 text ids get better wording, and
   whether the pooled-text collapse recurs — is unmeasured. One `setup` with an
   installation answers it, offline.
0c. **Take `watch`'s unit list from the gateway.** The first step of "The command
   surface after the ODIS pivot" above, and the one that unblocks the rest: while
   `watch` can only reach fourteen of the fifteen units through a cached survey,
   `survey` cannot be narrowed at all. Wiring `crate::units`' gateway walk into
   `watch/mod.rs:1882` is offline work with an offline test.
1. **Several channels per response, not one.** `Extracted::for_unit` and `merge` key a
   channel by its DID, so of 3,878 expressible fields only 1,959 survive. The other
   1,919 are here, already parsed, and thrown away at the last step. Needs `watch`'s
   history and chart, keyed by `(request, did)` today, keyed by channel instead. Moves
   the goal directly: it is the largest remaining block of readable channels.
2. **Sub-byte fields — 2,693 of them.** One-bit flags and 3-bit fields. `RawForm`
   reads whole bytes and returns an `i32`; a bit field needs a mask and a shift, and a
   one-bit flag needs a *name per state* rather than a number, so this and the
   `Scaling::Enum` question are one question. A decision first, then code.
3. **A second ODIS project.** Settles three things one project cannot: whether
   `PRODUCT-ID` sets are disjoint across projects (the whole car-picks-its-project
   design rests on it), whether the two unresolved units resolve elsewhere, and whether
   the TTTEXT crib generalises beyond one German project.
4. **Car-picks-its-project.** `project::covering()` is a named function returning
   `None`, and `Project::covers(type_code)` exists to fill it. Blocked on deciding
   which of a car's fifteen part numbers to believe — `5E0` × 3 against `8V0`, `5Q0`,
   `3Q0`, `0CW` — which is evidence, not code. Item 3 is the evidence.

**Car needed:**

5. **Confirm the sweep change on the parked car.** `WITNESS_EVERY = 64` and
   `QUIET_RUN = 3` are reasoned, not measured. A false halt has its own cost: it
   teaches people to reach for the override. One parked whole-car run answers it.
6. **The reverse-gear code.** `catalog.rs` says `0C`, ODIS says `0C` is Gear 9 and
   reverse is `7`, and the `0C` figure is a doc comment rather than a proven row.
   Select reverse, read `0x210F` on `7E0` and `0x3816` on `7E1`
   (`research/labels/odis-format.md` §7.1). One minute.
7. **`watch` and `measure` across the fifteen units.** The join is written and its
   per-unit numbers are measured against the *file*; nothing has confirmed them against
   the *car*.

## Next up (added 2026-08-07)

### 1. Read ODIS data alongside VCDS

A VW ODIS-Service runtime project (`SK37X`, VW-MCD Converter 26.1.0, ODX 2.0.1) was
examined on 2026-08-07 and it is **not encrypted**: every `.sd.db` is a store of
concatenated zlib members (`BL_LIBECM.sd.db` — 2,450 members, 386,843 B inflated), and
the two string pools sit beside it in the clear —

| file | contents | size inflated |
|---|---|---|
| `AStringData.data.gz` | 1,155,437 short names, `u32` length + ASCII | 73 MB |
| `UStringData.data.gz` | 153,704 texts, `u32` char count + UTF-16LE | 15 MB |

Both parse to the last byte in one pass. The pool carries 8,368 `IDE…` measurement
identifiers, 255,351 `DOP…` (the objects that hold a scaling), 652 `Unit_…` and 651
`EV_…` ECU variants. The scaling coefficients themselves are IEEE-754 doubles inside
`.sd.db`, unobscured — a scan of `BL_LIBECM.sd` turns up `1.0`, `100.0`, `0.01`,
`0.001` and `3.0517578125e-05` (2⁻¹⁵).

The reference car is covered: `EV_BCMMQB`, `EV_Brake1UDSContiMK100ESP`,
`EV_DashBoardVDDMQBAB`, `EV_GatewNF`, `EV_SteerAssisMQB`, `EV_TCMDQ200021`,
`EV_SMLSVALEOMQBLRH` and `EV_ECM18TFS0208V0906264H_001` all appear in the pool.

What this would give is the thing a drive currently pays for — measurement scalings
straight from the manufacturer — and names with no cipher between us and them.

The work is **not decryption**; it is reverse-engineering the `.sd.db` record layout
(binary, TLV-shaped, strings referenced by index into the pool). One file solved to the
level of *identifier → data object → compu coefficients → unit* generalises to the
other 165 by the same code. If the source `SK37X.pdx` can be obtained instead, it is
ODX XML in a zip and no RE is needed at all.

Two things this must not disturb:

- **The UDS allowlist stays `0x22`, `0x19`, `0x10`, `0x3E`.** ODIS data describes
  adaptation, coding and routines. Parsing that data is fine; letting any of it reach an
  executable path is the one thing `CLAUDE.md` forbids outright.
- **Nothing goes in the repo.** ODIS data is VW's, exactly as the label files are
  Ross-Tech's. Same position: the user brings their own copy, it lives under
  `~/.vagcan/`, the checkout ships none of it.

**It is also a crib.** Tested the same day against both ciphers in the label files
(`research/labels/odis-crib.md`): useless against the `.rod` container — the bytes a known
plaintext would have to predict are already-compressed ones — but the strongest lever yet
found against the `TTTEXT` substitution. A signature lookup against the ODIS strings as a
*closed* candidate list read **18,842 new names** at a measured precision of 86.6 %, more
than doubling the 14,738 the catalog holds, and resolved the 14-glyph numeric class that
`tttext-codec.md` §6 records as unbroken in 77.1 % of them. It also corrects names already
shipped (`Overgeneral AT` was `Continental AG`). Next steps are §7 of that file.

The user-facing cost is the real cost: `setup` currently means "point at a VCDS
installation". Admitting a second, differently-shaped source changes what `setup` asks
for, what the catalogs hold, what `info`/`sensors` prefer when both are present, and what
every "no data" message says. That is a UX rework, not a parser.

### 2. VNCI adapter support

A VNCI cable is now on hand (2026-08-07). Today the only live transport is the generic
slcan USB-CAN adapter (`vag-can`); the seam every backend implements is
`vag-transport`, so this is a new backend behind that trait rather than a change to the
protocol crates. Listen-only mode and the moving-car guard have to hold on it exactly
as they do on slcan — read `SAFETY.md` before the first connection.

## Parked (designed, not being implemented now)
- **Cross-platform `no_std` core + `vag-runtime-*`** — spec + M1 plan under
  `docs/superpowers/{specs,plans}/2026-07-06-cross-platform-*`. Below-the-seam refactor.
