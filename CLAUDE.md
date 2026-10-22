# vagcan — project instructions

## Commit messages (MANDATORY)

- End every commit with an **`Assisted-By:`** trailer naming the AI model —
  **never** `Co-Authored-By:`. Example:

  ```
  Assisted-By: Claude Fable 5 <noreply@anthropic.com>
  ```

- Keep the `Claude-Session:` trailer line as well.
- Use Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:`…).
- **Commit subjects and PR titles are impersonal**: they name the change, never a person —
  no "owner's", "his", "the user's" (`fix: the first-run path — …`, not `fix: a new owner's
  path — …`; owner, 2026-10-19). A squash merge takes the PR's title, so the title counts as a
  commit subject.

## Commands: `just` (MANDATORY)

The [`justfile`](justfile) at the root is how this tree is built, flashed and checked; `just`
lists the recipes. Use them rather than retyping the cargo lines, so what an agent runs is what
CI runs.

- **Before a push: `just check`** — every CI job, one recipe each (`fmt-check`, `clippy`,
  `test`, `e2e`, `no-std`, `host-check`, `fw-check`, `fw-ram`, `dead-code`). A change that touches
  only one area may run that recipe; the push still has to pass all of them.
- **CI calls the recipes and nothing else** ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)):
  a job installs its toolchain and system packages, then runs `just <recipe>`. A new check is
  a recipe first and a job step second; never a cargo line in the workflow alone.
- **Firmware:** `just fw-build|fw-flash|fw-run [bin] [cargo flags]` — the image name, then
  anything cargo takes: `just fw-flash dash --no-default-features` (a board whose BLE does not
  start), `just fw-flash oledtest`, `just fw-flash slcan`. `fw-run` stays on the console;
  `just fw-monitor`, `just fw-info`. A new image or variant is a flag, not a new recipe. The board's port is `ESP_PORT`, or the one port with Espressif's USB vendor id (`dashsim --boards`);
  the car is `VAGCAN_DASH_VIN`, or the one car under `~/.vagcan/dash`. Build into your own
  `CARGO_TARGET_DIR` when another agent builds at the same time — two reviewers sharing one
  flashed each other's binaries during PR #14 — the recipes honour it.
- **`just bench` flashes a transmitting image** (`research/dash/bench.sh`) and does not flash
  `dash` back: follow it with `just fw-flash`, always (Safety, below).
- **The laptop side:** `just build`, `just install` (over any earlier install), `just vagcan …`,
  `just plan <VIN>`, `just dashcfg …`, `just dashsim`, `just host <bin> …`.

## The dead-code check is `--workspace`, never `--all-targets`

```
RUSTFLAGS="--force-warn dead_code" cargo check --workspace
```

`just dead-code` runs it and fails on a warning that points into `crates/`, as CI does.

`--force-warn` sees through any `#[allow(dead_code)]`; `--workspace` alone is
what makes the answer true. **Adding `--all-targets` recompiles the binary a
second time as a test harness**, where `main` is unreachable by construction and
only what a unit test calls looks live — so it reports `fn main is never used`
and ~140 items where the real build reports none. That is a test-reachability
map, not dead code, and acting on it deletes the program.

Before deleting anything it does report: a function nobody calls that was
*written to be called* is a missing call site, not dead code
(`measure::messages::MissingChannel::tried` was exactly that).

## Formatting

The tree is `rustfmt`-formatted and CI enforces it (`cargo fmt --all -- --check`
is a blocking job). The style lives in [`rustfmt.toml`](rustfmt.toml): hard tabs,
`tab_spaces = 2`, `max_width = 150`, Unix newlines. Run `cargo fmt --all` before
committing, or just let the hook below do it.

**Auto-format hook.** [`.claude/settings.json`](.claude/settings.json) carries a
`PostToolUse` hook that runs `rustfmt` on every `.rs` file an `Edit`/`Write`
touches, so the tree never drifts out of format between `cargo fmt` runs. Notes:

- It passes **`--edition 2024`** explicitly. `rustfmt.toml` sets no `edition`, so
  a bare `rustfmt <file>` would default to 2015 and error on `async fn` (silently
  formatting nothing). `cargo fmt` is unaffected — it injects the edition itself.
- It ships tracked, via a `!.claude/settings.json` exception in `.gitignore`
  (the rest of `.claude/*` is ignored), so every clone formats on edit.
- It only fires inside **Claude Code**. Other editors/agents (Codex, etc.) do not
  run it — for them the CI `fmt` job is the backstop. Needs `jq` and `rustfmt` on
  `PATH`; if either is missing the hook no-ops rather than failing the edit.

**Two crates the workspace commands never reach.** `research/dash/host` and
`crates/dash/vag-dash-fw` are not workspace members, so `cargo fmt --all`, `cargo test
--workspace` and workspace clippy skip them — and CI checks each on its own (`just host-check`,
`just fw-check`, `just fw-ram`; `just check` runs them with the rest). Before a push
that touches them, run `cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test` in `research/dash/host`, and in `crates/dash/vag-dash-fw` its `cargo fmt -- --check`
and clippy on both builds — `VAGCAN_DASH_NO_CAR=1 cargo clippy --release --bins -- -D warnings`,
then the same with `--no-default-features` (no BLE, for a board whose BLE does not start).
`cargo fmt --all` from the root is still needed for the workspace crates a firmware change
touches: `1424c71` formatted the firmware only and left `guard.rs` red on `master`. Missed twice: an unformatted `dashsim` failed PR #3's CI
(2026-09-14), and a preview count its tests pin failed PR #4's (2026-09-15).

**Firmware RAM.** The ESP32-C3 has one data-RAM region: statics and IRAM code fill it from
the bottom, the stack gets the rest. [`crates/dash/vag-dash-fw/ram-budget.sh`](crates/dash/vag-dash-fw/ram-budget.sh)
builds `dash` with and without BLE (empty plan, as CI does), prints the sections and the 15
biggest statics, and fails when statics (`.data`+`.bss`+`.noinit`) pass a ceiling or `.stack`
falls under a floor. CI's `firmware` job runs it. The limits and what they were measured at
live in the script and nowhere else.

- A change to `vag-dash-fw`, or to a `no_std` crate it links (`vag-uds-*`, `vag-dash-render`, `vag-dash-core`),
  reports its static-RAM delta: run the script before and after.
- Growth over 1 KB is flagged in the PR and justified.
- Raising a limit is a change to `ram-budget.sh` with its reason beside it.
- A pixel is a bit, not a `bool` — on the board and in host tools that hold a panel.

## Safety (MANDATORY)

This tool only reads, and reading is not the same as harmless: an identifier sweep is a
fuzz test of a control unit's diagnostic server, and a path with a defect in it crashes
the server. Read-only bounds what can be *changed* about a car, not what can be
*provoked*.

- **Never add a write service.** No coding, no adaptation, no clearing faults, no
  flashing. The UDS allowlist is `0x22`, `0x19`, `0x10`, `0x3E` and stays that way.
- **A sweep is a fuzz test of a diagnostic server.** It is the most invasive thing here.
  Guard anything new that resembles one the way `units --identify <unit>` is guarded: one
  named unit, or `--all` as a sequence of single-unit runs each guarded on its own
  (owner, 2026-10-21, for `adaptations --all`): a stationary check per unit, and **once any
  unit has been read in the run, a recorded unit that does not answer its identification ends
  the run, with a STOPPED notice**; before anything has been read such a unit may be skipped,
  with a note; a bounded range (its identification block, `F100`–`F1FF`; for `adaptations`,
  exactly the identifiers of the unit's settled variant — never the union of a family's
  variants, so a unit whose `F1A2` is missing or matches no variant better than its family is
  refused); refused on a moving
  car (`require_stationary`, with "no answer" counted as moving); stopped the moment the
  unit falls silent under the reads (`anomaly::Monitor`); never through the dash board —
  `--slcan`, or the board's `slcan` image, either of which makes the board a plain adapter,
  is the one exception. The allowlist holds
  for every request it makes. Two gaps in that command today, not the pattern to copy: the
  three OBD-II mode-09 identifiers (`F802`, `F804`, `F80A`) it reads after the block are
  outside the guard, and it re-reads no witness, so a unit going back on an earlier answer is
  not something the monitor can see. Never a blind sweep of the whole car — that command was
  removed on 2026-09-28 and does not come back.
- **Anything that can change how a unit behaves is refused on a moving car** — checked
  by reading road speed, with "no answer" counted as moving.
- **A firmware image that transmits on its own is a bench tool, and builds only with the
  `bench` feature** (`cantx`, `cantest`, `rxprobe` in `vag-dash-fw`). `cantx` floods
  `7E0` at the bus's ceiling from power-on; a board left with it and plugged into a car
  floods the engine's diagnostic server at once. `bench.sh` ends by saying to reflash
  `dash` or `slcan`. A new transmitting bench image goes behind the same feature.
- **The board guards itself on any link that is not a cable.** A host across a radio is
  not trusted, so over BLE the board enforces the allowlist, the moving-car check and a
  sweep limit on its own (`.archive/tasks/done/dash/16-uds-over-ble.md`).

## No car-specific data in the code (MANDATORY)

The tool must work on **any VAG car**, not on this Škoda. That means a hard line
between algorithm and data:

- **Never hardcode data that belongs to one car** — measurement scalings, identifier
  numbers, unit names, part numbers, coding bytes. Those come from the label files
  (`.rod` / `.lbl` / `.clb`), cached in SQLite, resolved per car through what the car
  itself reports (`F187` part number, `F19E` ODX file name, the gateway's installation
  list).
- **Nothing the tool reads at run time lives in the checkout.** The label data is
  Ross-Tech's and may not be redistributed; the proven measurement rows are one
  owner's car. Both are under `~/.vagcan/` — see `crates/cli/vag-cli-core/src/datadir.rs`, which
  owns the layout — and `catalogs/` is gitignored. A new default path that resolves
  relative to the working directory is a bug: it works in a checkout and nowhere else,
  and after `cargo install` there is no checkout.
- **An offset or a magic number is a red flag.** Before writing one, establish whether
  it is a property of the *protocol* (ISO/UDS/OBD-II — fine, cite the standard) or of
  *this car* (not fine — it has to come from the label files or from a read).
- **A special case for one control unit or one reading belongs in its own module**, fed
  by data, not sprinkled through the generic path.
- Facts measured on the reference car are **evidence for a decoder**, not a table to
  ship. Where something is genuinely known only for this car, say so at the point of
  use and keep it out of the code path other cars take.

## Project

Goal, tech stack, architecture and development workflow are **the sections below in
this file** — `todo/GOAL.md` was folded into it and no longer exists. The task
breakdown is in **[`todo/README.md`](todo/README.md)**; read it before working.

TL;DR: read the whole car over CAN, with channels, scalings and fault text from either a VW
ODIS-Service project or a VCDS installation — two independent sources (below), on
**tokio / edition 2024**, macOS M4, TDD with hardware checkpoints. The live
transport is a generic slcan USB-CAN adapter (`vag-uds-can`), or the dash board: over its
USB cable or BLE through its own scheduler, or as a plain adapter (`--slcan`, or its
`slcan` image). The dash (ESP32-C3 + OLED) reads the car since 2026-09-13. The HEX clone
is dead — research archived under `.archive/research/`.

## Project structure

Rust workspace ([`README.md`](README.md)) + reverse-engineering research +
task tracking.

**A crate's directory is its package name**, and the family it sits in is already spelled
inside that name: `uds/vag-uds-client`, `dash/vag-dash-fw`. The repetition is deliberate —
a path and a package name that differ are two things to learn, and everything that reports
one (cargo, rustc, a stack trace, a grep) then has to be translated into the other. A
crate's *binaries* are free
of the rule and named for what a person types: `vag-cli` builds `vagcan`, `vag-dash-cfg`
builds `dashcfg`, `vag-dash-fw` builds `dash`. The rule that places them: **a binary lives
in its family when it has exactly one.** `dashcfg` and
the firmware serve only the device, so they sit inside `dash/`; `vagcan` consumes both
`uds/` and `data/`, so it belongs to neither and carries no family prefix.

```
crates/            all Rust. Three families and the product.
  uds/               talking to a car: ISO-TP underneath, UDS over it.
    vag-uds-transport  the seam every backend implements, whole PDUs
    vag-uds-can        slcan backend, async ISO-TP over CAN, listen-only sniffer
    vag-uds-client     UDS client + allowlist, DTCs, gateway, addressing
    vag-uds-capture    capture/replay transport, hardware-free tests
    vag-uds-sim        a virtual car on a pseudo-terminal: `dev sim`, the e2e
                       cases (`just e2e`, `tests/e2e/`, docs/virtual-car.md)
  data/              somebody else's diagnostic files, parsed and cached.
    vag-data-labels    parsers: .rod/.clb/.lbl, TTTEXT names, Codes.dat,
                       ODX/ODIS, OBD-II PIDs, catalog
    vag-data-db        SQLite cache over the parsed label files
  dash/              the OLED device, all of it — laptop side and board side.
    vag-dash-render    a Frame in, pixels out, on any DrawTarget
    vag-dash-core      the board's pure modules and the frame/value core the board and the
                       replay share; no_std
    vag-dash-ble       BLE client (btleplug): scan, pick, open a NUS pipe
    vag-dash-cfg       binary `dashcfg` — configures the dash over BLE
    vag-dash-wasm      the renderer as WebAssembly for `dev dash edit`'s page; built
                       by `vag-cli-diag`'s build script and embedded, checked for wasm
                       in CI's `test` job (`VAGCAN_DASH_WASM_REQUIRED`)
    vag-dash-fw        binary `dash` — the device itself. NOT a workspace member:
                       no_std for riscv32imc-unknown-none-elf with its own
                       build-std config. Build it from its own directory.
  cli/               what a person runs, in four layers.
    vag-cli-core       what both command crates stand on: which car this is,
                       what channels it has, the bus that polls them (the one owner of
                       the link: a cable, or the dash board over USB or BLE), where
                       its files live, the terminal widgets. Knows no command.
    vag-cli-diag       reading a car and the files that explain it: identify,
                       faults, the guarded sweeps (`units --identify <unit>`,
                       `sensors --unit <unit>` off 7E0–7E7, `adaptations`),
                       watch, setup, dash build and edit, vcds tooling
    vag-cli-measure    the acceleration stopwatch, a library: `vagcan measure`
                       is the one way in. Depends on `core` **alone** (its
                       manifest names no `vag-cli-diag`, so the compiler holds
                       it), which is what makes it a crate rather than a directory.
    vag-cli            binary `vagcan` — the command surface and nothing else:
                       clap declarations and a dispatcher. Top level = needs the car:
                       devices / info / units / adaptations / faults / sensors /
                       watch / measure —
                       plus `setup`, the one offline command there, because it is
                       the first thing a new owner runs and what a car command short
                       of label data offers to run. The workshop is `dev …`: sniff /
                       glossary / dash, and offline work grouped by input —
                       `dev recording …` (our own `watch --out` recordings) and
                       `dev vcds …` (VCDS's own files). `main.rs`'s
                       `the_top_level_is_only_what_needs_a_car` test holds the line
research/        RE writeups + tooling (NOT shipped) for work still in progress:
  dash/                the ESP32 board from the laptop's side. `can-bring-up.md` is the
                       hardware hand-off; `bench.sh` the one-command bench; `probes/` is
                       firmware that answered a question (wifi-ap, wifi-scan, wifi-sta,
                       ble-scan); `host/` is the bench rig — `dashsim` (be the panel
                       and the buttons), `bleecho`, `bleuds` (one framed UDS request or
                       subscription over BLE) and `benchecu` (the CANable answering as
                       a control unit; bench pair only, refuses on car traffic);
                       `layout-editor/` is notes only — the tool is `vagcan dev dash edit`
                       now; `alarm-thresholds.md` is
                       where the oil and gearbox alarm numbers came from (2026-10-19)
  odis-dtc/            fault codes and their text in an ODIS project: the object
                       layouts the DTC loader reads, and the offline proof against
                       the reference car's stored faults (ODIS 15/15, VCDS 11/15) —
                       made through `faults --from`, removed 2026-09-28; the results
                       stand as recorded
  vcds-registry/       VCDS keeps every measurement's DID, layout and scaling in the global
                       registry `RM.rod` (2026-09-28); a unit's `MWB` lists 1-based row numbers
                       into it. Overturns "scaling is live-only" (the archive labels/ files,
                       corrected in place). Read by `vag-data-labels::registry` and `setup`'s
                       step 5 since 2026-09-28. `README.md` + `scripts/`;
                       `scratch/` is the Ross-Tech-derived data, gitignored
  tuning/              the stage-1 FRF pipeline, not started; `frfscope/` opens a
                       Simos18 calibration as graphs (read-only, never talks to a car)
  new-cars/            what blocks newer VAG cars: label data, code, transport (in progress)
  measure/             `roles-sweep/`: `measure`'s roles over every engine and gearbox of an
                       ODIS project, run by hand against a machine's own cache
  tools/               one-shot tools that found bugs: `orphans.py` (pub items only tests
                       use), `sigint.py` (does Ctrl-C end a long command),
                       `skill-commands.py` (the skills' commands against `--help`)
.archive/        retired paths kept as evidence — see .archive/README.md for the map:
  research/            subjects whose findings are implemented and shipped:
    labels/              VW's label files — the `.rod`/`.clb`/`.lbl` crack, the TTTEXT
                         name codec, `Codes.dat`, the fault-naming chain. Key reads:
                         `rod-labels.md` (the crack), `tttext-codec.md` (→ names.json),
                         `fault-naming-hop.md` (number → words, end to end). NB the
                         "scaling is live-only" refutation here is **overturned** —
                         `research/vcds-registry/` found it in `RM.rod`
    car/                 what the reference car answers: identifier map, the units
                         outside the powertrain, the whole-car survey, gearbox state
    clb-crack/           RE scripts (usbpcap.py, link_cipher.py, framing_dis.py, decoders)
    *.md                 HEX-clone framing, clone crypto — negative results, do not retry
  specs/               superseded designs
  tasks/done/          finished task files
docs/            reference for users (docs/odis-project-mapping.md)
todo/            task tracking → todo/README.md (detailed roadmap) and todo/<subsystem>/;
                 finished task files retire to .archive/tasks/done/, dated status
                 history to .archive/tasks/roadmap-history.md
```


Start-here docs: [`README.md`](README.md) (features, roadmap), [`todo/README.md`](todo/README.md),
[`ARCHITECTURE.md`](ARCHITECTURE.md).

## Documentation for people (MANDATORY)

The documents a person reads are split by audience and stay split:

- **[`README.md`](README.md)** — what the tool does and how to start. Sections, in order:
  a warning, **Features** (commands and the dash, as facts), **Roadmap**, requirements,
  tested cars, hardware, install, setup, reading the car. `USAGE.md` was removed by the
  owner on 2026-09-13; do not bring it back.
- **`README.md`'s Roadmap is always current.** Any commit that finishes, adds or drops a
  roadmap item updates it in the same commit, with the date on its first line. The detail
  behind it is `todo/README.md`.
- **[`docs/`](docs/)** — reference a user looks things up in.
- **[`ARCHITECTURE.md`](ARCHITECTURE.md)** — why it is built this way: data sources, file
  formats, `setup`, the crates, the dash.

How that text is written (owner, 2026-09-14): **short, plain, unambiguous.** State what
the tool does and what to type. No lecturing, no reasoning the reader did not ask for, no
hedging. Formatting matters: tables for commands, one idea per bullet, fenced blocks for
anything typed. `CLAUDE.md`, `todo/` and `research/` are for agents and keep their own
style.

## ODIS and VCDS are independent sources (MANDATORY)

An ODIS project and a VCDS installation are **not companions**: each is a complete source on
its own, and each has to work alone (owner, 2026-10-21: «vcds и odis – не компаньоны. они
независимы друг от друга и должны уметь работать самостоятельно»). This replaces the
2026-09-28 rule "ODIS outranks VCDS, field by field".

- **One source per run.** With both installed, a run uses one of them for everything it names —
  channels, scalings, fault text, freeze frames, adaptations: ODIS by default when there is
  one, otherwise VCDS; chosen by `[source] use = "odis" | "vcds"` in `config.toml` or
  `--source` (owner's pick, 2026-10-21).
- **Never mixed.** No field, text or record of one source fills a gap in the other. What the
  chosen source does not describe is shown raw, or as its number, and says so.
- **Each path is tested alone:** ODIS-only and VCDS-only, on their own fixtures.

## Command output (MANDATORY)

Every command prints the way established CLI tools do — `rustc`, `cargo`, `git` — never a format
of our own (owner, 2026-10-21: «это должно быть актуально для любого вывода»; the home-made
`setup` step table and the wide `faults` table were rejected the same day).

- **Results are grouped records, rustc-diagnostic style.** A heading line per item (a unit, a
  source, a channel), its details indented beneath it. Free text wraps under itself and is
  never cut. Readable at 80 columns with no width tricks.
- **A table only for genuinely tabular data**: every row the same few short columns, no
  free-text column, fitting 80 columns (`setup --list`, `devices`). A wide table with a text
  column at the end is a grouped record instead.
- **Lists are `gh pr list`'s**: a one-line summary first, a blank line, headers in CAPITALS,
  short columns; a status mark (`✓`/`✗`) as the first column where a row can fail.
- **One object is a card, `gh pr view`'s**: what it is on the first line, a meta line of facts,
  then titled sections with their lines indented.
- **A multi-step command reports as `git` does**, on stderr: a heading line saying what it
  works on, indented past-tense step lines, one result line, and a hint in parentheses
  indented under the line it belongs to (`  (use "vagcan faults" with the car connected)`).
- **Facts on one line are joined with ` · `** (U+00B7 with a space each side) — never `•`, `|`
  or `/`. The separator and the marks live in one module of `vag-cli-core`; write them through
  it, never as literals (a test holds it). Paths are shortened with `~`.
- **Streams and levels:** data on stdout, notes and status on stderr; `-v` adds provenance and
  reasons, `-vv` the per-item detail and the wire, in the same line style. Colour only on a tty.
- **Errors are cargo's:** `error: <what failed>` in lower case, one short line; then a blank
  line and `help: <one action>` lines, each with the command to type in backticks; `note:` only
  for a reason the reader needs to act. Never prose paragraphs, never an example file inside an
  error. Exit code 1. (owner, 2026-10-21, after comparing `cargo`, `rustc`, `uv`, `fd`/`clap`,
  `deno`, `git` and `gh` side by side; `Error:`/`Fix:` was ours and is retired.)
- A change to a command's output is checked against the e2e goldens (`just e2e`, trycmd cases
  under `tests/e2e/`) once they exist, and the golden diff is shown in the PR.

**Examples — the shape to copy** (each picked by the owner from alternatives, 2026-10-21).

Grouped results, summary first (`faults`):

```
2 stored codes on 2 of 15 units · 1 failing now

70C  Lenks.Modul  5Q0953521KM
  ✗ B145501  Unterbrechung des Signalpfades "Init"
             failing · 254 times · 214592 km · last 2026-09-28 14:20

713  ESC  5Q0614517AQ
  · B1168F2  Steering Angle Sensor: Not Initialized
             stored · 3 times · 210101 km
```

A list (`units`; `setup --list` and `devices` the same way):

```
15 control units on XW8AD4NE9JH008917

UNIT  ID   PART         NAME                         COMPONENT
01    7E0  8V0906264H   Engine Control Module 1      1.8l R4 TFSI
02    7E1  0CW300041G   Transmission Control Module  GSG DQ200G2_M
03    713  5Q0614517AQ  Brakes 1                     ESC
```

NAME is the run's source's name for the unit and nothing else; COMPONENT is what the unit
calls itself (`F197`). Either may be blank (owner, 2026-10-22).

A card (`info`):

```
XW8AD4NE9JH008917 · SK37
15 control units · read 2026-10-21 14:02

ENGINE  01 · 7E0
  1.8l R4 TFSI
  part 8V0906264H · software 0008 · EV_ECM18TFS0208V0906264H

GEARBOX  02 · 7E1
  GSG DQ200G2_M
  part 0CW300041G · software 0516 · EV_TCMDQ200021
```

A multi-step command, on stderr (`setup`):

```
Read the VCDS installation at ~/Source/repos/vcds/vendor/vcds-en
  copied 16577 .rod files, all up to date
  parsed 3035 label files
  read 6085 channels of 15 units
Ready: fault text and scalings for every car.
  (use "vagcan faults" with the car connected)
```

Errors:

```
error: no USB-CAN adapter found

help: plug one in and check `vagcan devices`
help: plugged in but not listed? unplug it and plug it back in
help: over Bluetooth, add `--ble`
```

```
error: the car is moving (42 km/h)

note: adaptations are read only on a parked car
help: stop the car and run `vagcan adaptations 713` again
```

Not this — a home-made step table, a wide table with a text column, prose in an error:

```
source  step      state    detail
vcds    copy      current  16577 files, all current
```

```
Error: /tmp/nope is not a directory — there is nothing at that path.
    An extracted ODIS project is the folder holding `AStringData.data.gz` and …
With no path, `vagcan setup` asks which, and offers to download an installation; …
```

## Tech stack & architecture (locked)

- **Rust edition 2024, MSRV 1.85. Async runtime: tokio.**
- **One bus, one conversation at a time, one owner.** On the laptop the scheduler task
  (`vag-cli-core/src/bus`, over `vag_uds_client::schedule::Planner`) is the single
  owner of the link. Everything else holds a cheap `Bus` handle and talks to the task
  over a channel: `subscribe(unit, did, period)` (dropping the `Subscription`
  unsubscribes), `read_once`, `exchange`. The task addresses the link to one unit,
  runs one exchange to completion and releases it; the planner never has two requests
  out, so exactly one exchange is in flight. Do not add a second owner of the link,
  do not open the adapter beside a running `Bus` (only `dev sniff` opens it bare, for
  frames), and do not reintroduce an `Arc<Mutex<link>>`.
- **Pluggable backend, static dispatch:** `vag_uds_can::CanBackend` (send/receive one
  frame) under `vag_uds_transport::AsyncIsoTpTransport` (send/receive one PDU), native
  async-fn-in-trait, no `dyn`/`async-trait`. `vag_uds_can::UnitLink` is the seam a
  command addresses one unit through: every `CanBackend` is one (ISO-TP per unit), a
  link that carries whole PDUs implements it directly, and `Bus` is one too — so a
  command generic over `UnitLink` runs through the scheduler unchanged. The live
  backends are `SlcanBackend` over a serial port and the firmware's `TwaiBackend`; the
  same `vag-uds-*` crates compile `no_std` for the board under embassy
  (`default-features = false`).
- `vag-data-labels`/`vag-data-db` stay sync (CPU-bound). **Label lookup must be
  FAST** — `vagcan setup` caches the parsed label files to SQLite: an ODIS project's in
  `~/.vagcan/data/<project>/cache.sqlite`, the machine's one VCDS installation's in
  `~/.vagcan/vcds/cache.sqlite` (`vag-cli-core/src/datadir.rs` owns the layout).
- **Host = macOS Apple Silicon (M4).**

## Development workflow

- **TDD wherever possible.** Every task ends with passing tests + `cargo clippy
  --all-targets -- -D warnings` clean.
- **Parallel dev with up to 4 subagents**, each on its **own git worktree**
  (isolation). The controller (main session) splits tasks, reviews each agent's
  diff/MR against its task brief, verifies, and merges. Never merge un-reviewed.
- **Hardware checkpoints:** at milestones with visible results on the real car
  (e.g. init handshake works; VIN read works), STOP and ask the user to verify on
  hardware before continuing.
- **Task tracking:** active tasks live in `todo/<subsystem>/<task>.md`; when a
  task is done+reviewed+merged, move its file to `.archive/tasks/done/<subsystem>/<task>.md`
  (preserve the subsystem subdir) and update both roadmaps — `README.md` (short) and
  `todo/README.md` (detail). `todo/README.md` holds only what is live; a dated status
  that has been superseded moves to `.archive/tasks/roadmap-history.md`. Each subsystem
  dir may carry a short `README.md`.
