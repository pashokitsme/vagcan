# ODIS project store — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** read a VW ODIS-Service runtime project natively in Rust, and store what an
ODIS project and a VCDS installation both produce in one per-car project directory, so
`setup` becomes a choice of *source* rather than a hardcoded VCDS path.

**Architecture:** three independent seams. `vag-data::odis` is a new, self-contained
read-only parser for the three ODIS on-disk formats (PBL B+Tree, DJB2-hashed string
pool, zlib + tagged object stream). `vagcan::ui::menu` is a new arrow-key menu with the
same input-behind-a-trait split `ui::picker` already uses, so the source picker is
testable without a terminal. `vagcan::project` + `datadir` own the new
`~/.vagcan/projects/<id>/` layout and the one-time migration; `setup` wires all three
together.

**Tech Stack:** Rust edition 2024, `miniz_oxide` (already a `vag-data` dependency),
`crossterm` (already a `vagcan` dependency), `rusqlite` (already a `vag-db`
dependency). **No new external dependency is added by this plan.**

## Global Constraints

Copied verbatim from the spec — every task's requirements implicitly include these.

- The UDS read-only allowlist stays `0x22`, `0x19`, `0x10`, `0x3E`. No object type
  whose only purpose is a write service (adaptation, coding, flashing, access-key
  handshake) is ever parsed into an executable path, regardless of how easy parsing it
  would be. The permanent-exclusion list is spec §2: `MCD_DB_FLASH_JOB`,
  `MCD_ACCESS_KEY`, `MCD_DB_SINGLE_ECU_JOB`, `MCD_DB_STARTCOMMUNICATION`,
  `MCD_DB_STOPCOMMUNICATION`, `DB_CASE`, `DB_CASES`, `DB_DEFAULT_CASE`,
  `MCD_DB_CODE_INFORMATION`, `MCD_DB_CODE_INFORMATIONS`.
- Nothing this tool reads at run time lives in the checkout. Both a VCDS installation
  and an ODIS project are the vendor's, not redistributable, and stay under
  `~/.vagcan/`. **No test may commit an ODIS or VCDS byte into the repository** —
  fixtures are synthesised in code or built into a `tempfile` directory.
- `RUSTFLAGS="--force-warn dead_code" cargo check --workspace` stays clean throughout.
  Never `--all-targets`.
- `cargo fmt --all` before every commit; hard tabs, `tab_spaces = 2`, `max_width = 150`.
- Commit messages use Conventional Commits and end with `Assisted-By:` +
  `Claude-Session:`, never `Co-Authored-By:`. Stage by explicit path — never
  `git add -A`, `git add .`, `git commit -a`.
- No car-specific data in code. A scaling, a DID, a unit name — all of it comes from a
  source file, never a hardcoded constant.

---

## Decisions this plan makes that the spec left open

The spec (§7) named three things it did not settle, plus two gaps the codebase survey
turned up. Decided here so no implementer decides them by accident:

**D1 — the merged `cache.sqlite` schema (spec §4.3).** The existing `measurement`
table stays exactly as it is: it is VCDS block/field-addressed and nothing about that
is wrong. ODIS rows go into a **new table**, `reading`, addressed the way ODIS
addresses things — by UDS DID. Forcing one table would have to invent a block number
for a DID or a DID for a block, and both inventions are car-specific data in code.
Both tables gain a `source_id`; `source` gains `kind` (`"vcds"` / `"odis"`) and stops
being a one-row table.

**D2 — which loaders ship first.** Increment 1 is the measurement chain plus
identification (spec §2 rows 1 and 2) — that is what `watch`/`scan`/`survey`/`info`
need and what everything else is gated behind. Increment 2 is faults and topology
(rows 3 and 4). Both are in this plan; increment 2 is Tasks 11–12.

**D3 — S42 `project_id` lookup (spec §4.1).** Not built. An ODIS source names itself
(D7); a VCDS-only source asks the user for a free string, defaulting to the one project
already on disk, or to `default` where there is none. The S42 document stays unparsed,
and no code pretends otherwise.

**D4 — Codes.dat and the shared pool.** The spec's §4 tree lists only `.rod` files
under `~/.vagcan/rod/`, but `faults`/`faultnames` read the fault text file (`Codes.dat`
/ `Code-RUS.dat`) off disk at run time, so it cannot be dropped. It joins the shared
pool: `~/.vagcan/rod/` holds every raw VCDS file read at run time — the `.rod` files
flat, plus the fault text under its own build-specific name. `.lbl`/`.clb` are **not**
kept, per the spec and the owner's explicit instruction.

**D5 — the consequence of D4, stated plainly.** Dropping `.lbl`/`.clb` means
`cache.sqlite` becomes the only surviving representation of them, so rebuilding it
requires the VCDS installation again. The freshness rule must therefore tolerate a
source directory that is gone: a cache whose `source.dir` no longer exists is trusted
unconditionally rather than declared stale, or every run after the install is deleted
would try to rebuild from nothing.

**D6 — which project a command uses.** `~/.vagcan/config.json` gains
`"project": "<id>"`, set by `setup`. A global `--project <id>` flag and the
`VAGCAN_PROJECT` environment variable override it, in that order. Exactly one project
on disk is selected automatically without config. No project at all is the existing
"run `vagcan setup` first" error, reworded.

**D7 — an ODIS project names itself, and the name is read twice.** Established against
the real project during increment 1, and not in the spec: `index.xml` opens with
`<CATALOG …><SHORT-NAME>SK37X</SHORT-NAME>`, which beats the directory name because an
unzip produces `SK37X (1)` and `<SHORT-NAME>` survives it. `Project::id()` prefers it
and falls back to the directory name.

The ordering that follows is **C's to enforce, and nothing in the type system forces
it**: `setup::source::project_id` runs *before* `odis::Project::open`, so it can only
see the folder, and it answers with the folder name made into a legal directory name
(`SK 37X (copy)` → `SK-37X-copy`). If C creates `~/.vagcan/projects/<that>/` and only
then opens the project, one car's data lands in a store called `SK-37X-copy` while the
project inside it calls itself `SK37X` — one car in two places, which is the exact
failure `datadir.rs`'s `existing_folder` was written to undo for cars. **Open the
project first, compare `Project::id()` against the picker's answer, and prefer
`Project::id()` before any directory is created.**

Related, and also C's: the version for `sources.json` (spec §4.4) comes from
`DatabaseVersionInfo.txt` — `VWMCD_ProjectVersionInfo="2610.2.688"`, plus
`VWMCD_OdxVersionInfo` and `VWMCD_ConverterVersionInfo`, all plain `KEY="value"` lines.
It is read once after the parse, where the value is already in hand, and deliberately
not threaded through the picker.

---

## File structure

| file | owner | responsibility |
|---|---|---|
| `crates/vag-data/src/odis/mod.rs` | A | the `odis` public seam: `Project`, `Variant`, `Reading`, `Error` |
| `crates/vag-data/src/odis/hash.rs` | A | DJB2 ObjectID hashing (spec §3.2) |
| `crates/vag-data/src/odis/keyfile.rs` | A | read-only PBL B+Tree over `.key` (spec §3.1) |
| `crates/vag-data/src/odis/strings.rs` | A | `AStringData`/`UStringData` pools, hash → name table |
| `crates/vag-data/src/odis/pool.rs` | A | `.db` member locate + inflate |
| `crates/vag-data/src/odis/object.rs` | A | the tagged-field stream reader (spec §3.3) |
| `crates/vag-data/src/odis/loaders/mod.rs` | A | type enum, dispatch, the permanent refusal list |
| `crates/vag-data/src/odis/loaders/measurement.rs` | A | the §2 measurement-chain types |
| `crates/vag-data/src/odis/loaders/identity.rs` | A | the §2 identification types |
| `crates/vag-data/src/odis/loaders/faults.rs` | A (inc. 2) | the §2 fault types |
| `crates/vag-data/src/odis/loaders/topology.rs` | A (inc. 2) | the §2 topology types |
| `crates/vag-data/src/odis/compu.rs` | A | compu method → `vag_data::Scaling` |
| `crates/vagcan/src/ui/menu.rs` | B | arrow-key menu of fixed options, input behind a trait |
| `crates/vagcan/src/setup/source.rs` | B | the source picker flow, its copy, the project-id prompt |
| `crates/vagcan/src/project.rs` | C | `~/.vagcan/projects/<id>/`, `sources.json`, selection (D6) |
| `crates/vagcan/src/datadir.rs` | C | new paths, shared `rod/` pool, migration |
| `crates/vagcan/src/setup/mod.rs` | C | wiring: picker → VCDS branch / ODIS branch → project store |
| `crates/vag-db/src/lib.rs` | C | schema D1 |
| `crates/vagcan/src/main.rs` | C | `--project`, the reworded `setup` help |

Owners A/B/C never edit each other's files. The three shared files are
`crates/vag-data/src/lib.rs` (A adds one `pub mod odis;` line),
`crates/vagcan/src/ui/mod.rs` (B adds one `pub mod menu;` line) and
`crates/vagcan/src/setup/mod.rs` (B adds one `pub mod source;` line — a module
nobody declares is a module nobody compiles, so B's own task cannot go green
without it; everything else in that file is C's).

---

## Interfaces — the contracts between owners

These signatures are fixed. A implements them, C consumes them, and neither may change
them without the other being told.

```rust
// crates/vag-data/src/odis/mod.rs  — owner A, consumed by C

/// An extracted ODIS-Service runtime project: a directory of
/// `0.0.0@<name>.<kind>.db` / `.key` pairs — six kinds, not just `.sd` — plus
/// `AStringData.data.gz` / `UStringData.data.gz` (or their unpacked forms).
pub struct Project { /* private */ }

/// Everything that can go wrong reading one. Hand-rolled like `vag_db::Error`
/// — `vag-data` has no `anyhow` and gains none here.
///
/// `Refused` was added by owner A during Task 6 and is not cosmetic: a type on
/// the permanent refusal list can appear *inside* another object, where it
/// cannot be skipped (nothing in the stream says how long it is). `Format`
/// would say the opposite of what happened — `Format` means the file is wrong,
/// `Refused` means the file is fine and the tool declines. A caller that wants
/// the rest of a project matches on `Refused`, skips that object and carries on.
#[derive(Debug)]
pub enum Error { Io(std::io::Error), Format(String), Missing(String), Refused(&'static str) }

/// One ECU variant an ODIS project describes.
pub struct Variant {
  pub name: String,          // the ObjectID, e.g. "EV_ECM18TFS02..."
  pub pool: String,          // which pool it came from (a `.bv`, in practice)
  pub base_variant: Option<String>,   // None when this *is* a base variant
}

/// One readable channel of one variant, in this project's terms.
pub struct Reading {
  pub did: u16,              // the UDS identifier `0x22` would ask for
  pub name: String,
  pub unit: Option<String>,
  pub bit_offset: u32,       // within the positive response, after the 3-byte header
  pub bit_length: u32,
  pub signed: bool,
  pub scaling: vag_data::Scaling,   // via compu.rs — Linear / Enum / Anchor
  // Added by owner A in Task 8: the text id of `name`, which is the join to
  // TTTEXT (`research/labels/odis-crib.md` §3) and therefore what makes an
  // ODIS-derived name mergeable into `names.json` rather than only printable.
  pub text_id: Option<String>,
}

impl Project {
  /// Open a project directory. Reads only the string pools eagerly; pools are
  /// opened lazily, because a project is ~470 files and ~1.1M strings.
  pub fn open(dir: &std::path::Path) -> Result<Project, Error>;
  /// The project's own name — the identifier VW's tooling uses (spec §4.1).
  /// Taken from `index.xml`'s `<SHORT-NAME>`, falling back to the directory
  /// name: a folder gets renamed by an unzip (`SK37X (1)`) and the file does not.
  pub fn id(&self) -> &str;
  /// Added by owner A in Task 8, for `sources.json` (spec §4.4): the converter's
  /// project version from `DatabaseVersionInfo.txt`. `None` if the file is absent.
  pub fn version(&self) -> Option<&str>;
  /// Added by owner A in Task 8: every PoolID this project holds, sorted.
  /// Not needed by C's assembly; it exists so a failure can name a pool.
  pub fn pools(&self) -> &[String];
  /// Every ECU variant, across every pool.
  pub fn variants(&self) -> Result<Vec<Variant>, Error>;
  /// The readable channels of one variant.
  pub fn readings(&self, variant: &Variant) -> Result<Vec<Reading>, Error>;
  /// Every human-readable name this project knows, keyed by its text id, for
  /// merging into `names.json`.
  pub fn names(&self) -> Result<std::collections::BTreeMap<String, String>, Error>;
}
```

```rust
// crates/vagcan/src/ui/menu.rs  — owner B, consumed by C

/// One line of a menu: what it is, and one line saying what choosing it does.
pub struct Item<'a> { pub label: &'a str, pub detail: &'a str }

/// Where a menu's answer comes from — the same split `ui::picker::Chooser`
/// uses, so the copy and the ordering are testable with no terminal.
pub trait Asker {
  /// Show `items` under `question` with row `at` highlighted; `None` is a quit.
  /// A stdin that is not a terminal is an `Err` naming the command line that
  /// needs no menu; an empty `items` is an `Err` too, and a caller's bug.
  fn ask(&mut self, question: &str, items: &[Item<'_>], at: usize) -> anyhow::Result<Option<usize>>;
  /// A free-text answer, with a default taken on an empty line. A stdin that is
  /// not a terminal takes the default silently rather than erroring — a default
  /// *is* an answer, and this is what keeps `vagcan setup PATH </dev/null`
  /// working. An empty default therefore comes back as an empty string, which
  /// the caller reads as "never mind".
  fn line(&mut self, question: &str, default: &str) -> anyhow::Result<String>;
  fn say(&mut self, line: &str) -> anyhow::Result<()>;
}

/// The person's. Built with the command line that answers the question without
/// a menu — `ui::picker::Console` carries the same string for the same reason:
/// this module does not know which command it is serving, and the sentence a
/// redirected stdin gets is only useful if it names one.
pub struct Console { /* private */ }
impl Console { pub fn new(instead: impl Into<String>) -> Console; }

/// The tests'. Both are `#[cfg(test)]`: nothing outside a test builds either,
/// and a non-test `Answer` would be dead code in the workspace check.
pub struct Scripted { /* private */ }
impl Scripted { pub fn new(answers: Vec<Answer>) -> Scripted; }
pub enum Answer { Pick(usize), Type(String), Quit }
```

```rust
// crates/vagcan/src/setup/source.rs  — owner B, consumed by C

/// What `setup` was told to learn this car from.
pub enum Source {
  Odis { dir: std::path::PathBuf },
  Vcds { dir: std::path::PathBuf },
  DownloadVcds,
}

/// Run the picker. `None` means the person left without choosing, which is a
/// successful, zero-exit outcome — the same rule `setup`'s download prompt
/// already follows.
pub fn choose(io: &mut impl crate::ui::menu::Asker, preselected: Option<&str>) -> anyhow::Result<Option<Source>>;

/// Ask what to call this project. Returns the ODIS folder name unasked when
/// the source is ODIS (spec §4.1); asks, defaulting to `default`, otherwise.
pub fn project_id(io: &mut impl crate::ui::menu::Asker, source: &Source, existing: &[String]) -> anyhow::Result<String>;
```

```rust
// crates/vagcan/src/project.rs  — owner C, consumed by nothing outside vagcan

pub struct Project { pub id: String, pub dir: std::path::PathBuf }
pub fn open_or_create(id: &str) -> anyhow::Result<Project>;
pub fn list() -> anyhow::Result<Vec<String>>;
pub fn current() -> anyhow::Result<Project>;              // D6 resolution order
pub fn record_source(p: &Project, entry: SourceEntry) -> anyhow::Result<()>;   // sources.json
impl Project {
  pub fn cache(&self) -> std::path::PathBuf;              // cache.sqlite
  pub fn names(&self) -> std::path::PathBuf;              // names.json
  pub fn rod_keys(&self) -> std::path::PathBuf;           // rod-keys.json
  pub fn measurement_dir(&self) -> std::path::PathBuf;    // measurement/
}
pub fn rod_pool() -> anyhow::Result<std::path::PathBuf>;  // ~/.vagcan/rod, shared
```

---

## Tasks

Each task ends with a green `cargo test --workspace`, a clean
`RUSTFLAGS="--force-warn dead_code" cargo check --workspace`, `cargo fmt --all`, and a
commit staged by explicit path.

### Increment 1 — the formats (owner A)

- [ ] **Task 1 — DJB2 hashing + string pools.** `odis/hash.rs`, `odis/strings.rs`.
      Tests: the seed/mask/`0 → 5`/`+11`-collision rules against hand-computed values;
      a synthesised pool of `u32`-length-prefixed ASCII and `u32`-char-count-prefixed
      UTF-16LE parses to the last byte with nothing left over; a truncated pool is a
      `Format` error, not a panic.
- [ ] **Task 2 — the `.key` B+Tree, read-only.** `odis/keyfile.rs`. 4096-byte blocks,
      the 13-byte header, backward 2-byte slots, `keycommon` prefix decompression, the
      self-describing 1–5 byte varint. Tests: a hand-built one-leaf block; a two-level
      tree; prefix-compressed successive keys; `Find` on a key that is absent; a block
      whose `nentries` overruns the block is refused. **No insert/delete/split path
      exists** — a test asserts the module exposes no `&mut self` method.
- [ ] **Task 3 — `.db` members.** `odis/pool.rs`: given a `(position, compressed_size,
      decompressed_size)` triple, inflate with `miniz_oxide`. Tests: a member built by
      compressing known bytes round-trips; a triple whose decompressed size disagrees
      with what came out is a `Format` error (a truncated file must not be trusted).
- [ ] **Task 4 — the tagged object stream.** `odis/object.rs`: 2-byte LE type enum,
      tagged fields, terminator `23 3E 00`. Tests: a synthesised object of each field
      shape; a stream missing its terminator is refused; a field length that runs past
      the buffer is refused.
- [ ] **Task 5 — the refusal list.** `odis/loaders/mod.rs`: the type enum, dispatch,
      and the spec §2 never-parsed list. Test: every name in that list resolves to a
      `Refused` outcome, and `Refused` carries no parsed payload of any kind. This test
      is the executable form of the project's central safety rule and must not be
      weakened.
- [ ] **Task 6 — the measurement chain.** `odis/loaders/measurement.rs` +
      `odis/compu.rs`. `MCD_DB_SERVICE` → `DIAG_COM_PRIMITIVE` → `REQUEST` /
      `RESPONSE` → `PARAMETER*` → `DOP_SIMPLE_BASE` → `DIAG_CODED_TYPE` /
      `PHYSICAL_TYPE` / `COMPU_METHOD`, and `COMPU_*` → `vag_data::Scaling`. Tests:
      `IDENTICAL` maps to `Scaling::Linear { factor: 1.0, offset: 0.0 }`;
      `LINEAR` with a rational coefficient pair maps to the right factor/offset;
      `TEXTTABLE` maps to `Scaling::Enum`; an unsupported compu category is an error
      naming the category, never a silent factor of 1.
- [ ] **Task 7 — identification.** `odis/loaders/identity.rs`: `MCD_DB_ECU_VARIANT` +
      the `MATCHING_*` types, and `Project::variants` on top of them.
- [ ] **Task 8 — the seam.** `odis/mod.rs`: `Project::open`/`id`/`variants`/`readings`/
      `names`, the `Error` type, and one `pub mod odis;` line in `vag-data/src/lib.rs`.
      An end-to-end test builds a miniature project in a `tempfile` directory (one
      pool, one variant, two readings) and asserts `readings()` returns both with their
      scalings. **Nothing under `~/Downloads` or any real project is read by a test.**

### Increment 1 — the interface (owner B, parallel with A)

- [ ] **Task 9 — `ui::menu`.** `crates/vagcan/src/ui/menu.rs` + one line in
      `ui/mod.rs`. `Asker`/`Console`/`Scripted`, arrow keys, `Enter`, `q`/`Esc`/`Ctrl-C`.
      A stdin that is not a terminal must fail with the sentence that names the
      non-interactive way to do the same thing — `ui::picker`'s `no_terminal` is the
      model, not a new rule. Tests use `Scripted`: the highlight starts where asked,
      wraps or clamps as decided, and a quit is a quit.
- [ ] **Task 10 — the source picker.** `crates/vagcan/src/setup/source.rs`: `Source`,
      `choose`, `project_id`, the three-option menu from spec §5, and validation that a
      chosen directory actually looks like what it claims (an ODIS project has
      `AStringData.data.gz` and at least one `.sd.key`; a VCDS root has `UDS_EV/`).
      Tests: each option returns the right `Source`; a directory that is neither is
      refused with a sentence naming what was expected; an ODIS pick takes its
      `project_id` from the folder name without asking; a VCDS pick asks and defaults
      to `default`; a `project_id` colliding with an existing project is accepted (that
      is the merge case, spec §5) and one that is not a safe directory name is refused.

### Increment 1 — the assembly (owner C, after A and B)

- [x] **Task 11 — the project store.** `crates/vagcan/src/project.rs` + `datadir.rs`:
      the paths above, `sources.json` (spec §4.4), the D6 selection order. Tests
      mirror `datadir.rs`'s existing ones: nothing lands in the checkout, everything
      lands under `~/.vagcan`, a project id off a filesystem is refused as a directory
      name the way a VIN already is.
- [x] **Task 12 — the schema.** `crates/vag-db/src/lib.rs`: D1 — `source` gains `kind`
      and multiple rows, `measurement` gains `source_id`, the new `reading` table, and
      a migration for an existing single-row `source`. Tests: an old cache opens and
      migrates; a DID lookup returns the ODIS row; both sources coexist in one file.
- [x] **Task 13 — migration.** Spec §6: `data/{extracted,measured}` → the first named
      project, `.rod` + the fault text to the shared pool, `measured/` →
      `projects/<id>/measurement/`. One-time, one-directional, prompted. Tests build a
      fake old layout in a `tempfile` directory and assert every file arrives, that
      **nothing is deleted that was not copied first**, and that a second run is a
      no-op. D5's rule — a cache whose source directory is gone is trusted, not stale —
      is tested here.
- [x] **Task 14 — wiring.** `setup/mod.rs` + `main.rs`: the picker replaces the
      argument-or-download flow, the VCDS branch writes into the project store, the
      ODIS branch runs `odis::Project` into `cache.sqlite` + `names.json`, `--project`
      lands on the CLI, and every existing reader of `extracted_dir()` (`faults.rs`,
      `faultnames.rs`, `labels.rs`, `main.rs:865`) moves to the project store.
      `setup <PATH>` with a path still works and skips the menu.

### Blocking — found by owner C's acceptance run against `~/Downloads/SK37X`

Tasks 11–14 are done and the store, the migration and both `setup` branches work
on the real project. **The ODIS branch cannot yet produce a single reading**,
because `Project::variants()` returns nothing on the real project. Two separate
causes, both owner A's, both found only by running the Rust against the project
for the first time:

- [ ] **Task 14a — `DB_PROJECT_DATA` in a base-variant pool hits the refusal
      list.** All 54 `.bv` pools — the only pools that carry ECU variants —
      raise `Error::Refused("MCD_ACCESS_KEY")` from
      `loaders::identity::project_data`. This is exactly the case `Refused` was
      added for, and it is the one place it is not handled: `Project::variants`
      propagates with `?`, so one refused type inside one object costs the whole
      project every variant it has. Skipping the object is not enough either —
      the variant list *is* that object. The access key has to be stepped over
      inside `project_data`, the way `scan_for_layer_data` already steps around
      one for layer data.
- [ ] **Task 14b — `DB_PROJECT_DATA` is 17 bytes short.** All 166 `.sd` pools'
      project-data objects are 61 bytes; the loader consumes 44 (2-byte type +
      42 fields) and `Stream::end` then refuses the remaining 17 before the
      `23 3E 00` terminator. The parsed content is right as far as it goes —
      these pools genuinely carry no base variant and no ECU variants — so this
      is a missing tail in the field list, not a misread. `Project::variants`
      aborts the whole loop on it for the same reason as 14a.

Neither is worked around in owner C's code: `setup`'s ODIS branch reports the
error and writes nothing, which is the honest outcome until the parser lands.

**Not yet verified, and it is the claim the spec rests on:** DID `0x380A` coming
back `IDENTICAL` on the real engine. There is no path to a `Reading` while
`variants()` returns nothing, so `research/labels/rod-labels.md:433` remains
cross-checked only against owner A's synthetic fixture. It is the first thing to
re-run once 14a lands.

### Increment 2 — the rest of the scope (owner A, then C)

- [ ] **Task 15 — faults.** `odis/loaders/faults.rs`: `DB_DOP_DTC`,
      `MCD_DB_DIAG_TROUBLE_CODE`, `MCD_DB_ENV_DATA_DESC`, `MCD_DB_PARAMETER_ENV_DATA`,
      `MCD_DB_PARAMETER_ENV_DATA_DESC`, plus a `Project::faults(&Variant)` accessor.
- [ ] **Task 16 — topology.** `odis/loaders/topology.rs`: `MCD_DB_ECU`,
      `MCD_DB_ECU_BASE_VARIANT`, `MCD_DB_LOGICAL_LINK`, `MCD_DB_FUNCTIONAL_GROUP`,
      `MCD_DB_FUNCTIONAL_CLASS`, plus a `Project::units()` accessor.
- [ ] **Task 17 — wiring increment 2.** `faults` and `units` read the ODIS-derived
      rows where a VCDS row is absent, with `measurement/` still winning (spec §4.5).
- [ ] **Task 18 — the writeup.** `research/labels/odis-format.md`, which spec §1 names
      as this work's missing document: the three formats as implemented, the
      cross-check against `0x380A`, and what a second ODIS project would test.

---

## Review protocol — who checks whom, and when

Three owners, and none of them sees the whole tree. So the crossings are named rather
than left to whoever happens to look:

1. **During increment 1, A and B cross-check each other's contract.** Neither reads the
   other's implementation; each reads the other's *public surface* against the
   Interfaces section above and reports any drift — a renamed method, a changed return
   type, an error case the contract does not mention. A contract change agreed between
   them is written back into this plan in the same commit that makes it.
2. **Before C writes a line, C reviews both A's and B's work.** Not a rubber stamp: C
   is the only owner who will call both APIs, so C is the one who finds out that a
   signature is unusable, that an error type cannot be propagated, or that the picker
   asks for something the store cannot store. Anything found here goes back to its
   owner before assembly starts — C does not work around another owner's API.
3. **After C assembles, A and B both review the integration.** A checks that the parser
   is being *called* correctly — that no loader is invoked outside its scope, that the
   refusal list is still enforced at the call site, that an error is not being
   swallowed into a default. B checks that the interface still behaves as designed —
   that the copy survived the wiring, that a non-terminal stdin still gets the sentence
   that helps, that no prompt was quietly added or dropped.
4. **Every review is reported, including a clean one.** "Nothing found" is a result;
   silence is not.

## Verification against the real project

The owner has an extracted project at `~/Downloads/SK37X/` (472 files). It is the
acceptance check, run by hand, never by a test and never copied into the repository:

```
vagcan setup            # pick "ODIS project", point at ~/Downloads/SK37X
vagcan units
vagcan watch            # the engine's DIDs, named and scaled, with no drive
```

The claim to check is the one spec §1 rests on: DID `0x380A` on the engine comes back
`compu_category: IDENTICAL`, i.e. `u16` raw — matching what
`research/labels/rod-labels.md:433` proved by driving, from the other direction.
