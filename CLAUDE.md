# vagcan — project instructions

## Commit messages (MANDATORY)

- End every commit with an **`Assisted-By:`** trailer naming the AI model —
  **never** `Co-Authored-By:`. Example:

  ```
  Assisted-By: Claude Fable 5 <noreply@anthropic.com>
  ```

- Keep the `Claude-Session:` trailer line as well.
- Use Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:`…).

## The dead-code check is `--workspace`, never `--all-targets`

```
RUSTFLAGS="--force-warn dead_code" cargo check --workspace
```

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
--workspace` and workspace clippy skip them — and CI checks each on its own. Before a push
that touches them, run `cargo fmt -- --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test` in `research/dash/host`, and in `crates/dash/vag-dash-fw` its `cargo fmt -- --check`
and clippy on both builds — `VAGCAN_DASH_NO_CAR=1 cargo clippy --release --bins -- -D warnings`,
then the same with `--no-default-features` (no BLE, for a board whose BLE does not start).
`cargo fmt --all` from the root is still needed for the workspace crates a firmware change
touches: `1424c71` formatted the firmware only and left `guard.rs` red on `master`. Missed twice: an unformatted `dashsim` failed PR #3's CI
(2026-09-14), and a preview count its tests pin failed PR #4's (2026-09-15).

## Safety (MANDATORY)

This tool only reads, and reading is not the same as harmless: an identifier sweep is a
fuzz test of a control unit's diagnostic server, and a path with a defect in it crashes
the server. Read-only bounds what can be *changed* about a car, not what can be
*provoked*.

- **Never add a write service.** No coding, no adaptation, no clearing faults, no
  flashing. The UDS allowlist is `0x22`, `0x19`, `0x10`, `0x3E` and stays that way.
- **A sweep is a fuzz test of a diagnostic server.** It is the most invasive thing here.
  Guard anything new that resembles one the same way `survey` is guarded.
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

TL;DR: read the whole car over CAN, with channels, scalings and fault text from a VW
ODIS-Service project (a VCDS installation is the fallback, drives on the car override
both), on **tokio / edition 2024**, macOS M4, TDD with hardware checkpoints. The live
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
  data/              somebody else's diagnostic files, parsed and cached.
    vag-data-labels    parsers: .rod/.clb/.lbl, TTTEXT names, Codes.dat,
                       ODX/ODIS, OBD-II PIDs, catalog
    vag-data-db        SQLite cache over the parsed label files
  dash/              the OLED device, all of it — laptop side and board side.
    vag-dash-render    a Frame in, pixels out, on any DrawTarget
    vag-dash-ble       BLE client (btleplug): scan, pick, open a NUS pipe
    vag-dash-cfg       binary `dashcfg` — configures the dash over BLE
    vag-dash-fw        binary `dash` — the device itself. NOT a workspace member:
                       no_std for riscv32imc-unknown-none-elf with its own
                       build-std config. Build it from its own directory.
  cli/               what a person runs, in four layers.
    vag-cli-core       what both command crates stand on: which car this is,
                       what channels it has, the bus that polls them (the one owner of
                       the link: a cable, or the dash board over USB or BLE), where
                       its files live, the terminal widgets. Knows no command.
    vag-cli-diag       reading a car and the files that explain it: identify,
                       faults, the guarded sweeps, watch, setup, vcds tooling
    vag-cli-measure    binary `vagcan-measure` — the acceleration stopwatch.
                       Depends on `core` **alone**, checked symbol by symbol,
                       which is what makes it a crate rather than a directory.
    vag-cli            binary `vagcan` — the command surface and nothing else:
                       clap declarations and a dispatcher. Top level = needs the car:
                       devices / info / units / faults / sensors / watch / measure —
                       plus `setup`, the one offline command there, because it is
                       the first thing a new owner runs and what a car command short
                       of label data offers to run. The workshop is `dev …`: survey /
                       sniff / glossary / dash, and offline work grouped by input —
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
                       a control unit; bench pair only, refuses on car traffic)
  odis-dtc/            fault codes and their text in an ODIS project: the object
                       layouts the DTC loader reads, and the offline proof against
                       the reference car's stored faults (ODIS 15/15, VCDS 11/15)
  tuning/              the stage-1 FRF pipeline, not started; `frfscope/` opens a
                       Simos18 calibration as graphs (read-only, never talks to a car)
.archive/        retired paths kept as evidence — see .archive/README.md for the map:
  research/            subjects whose findings are implemented and shipped:
    labels/              VW's label files — the `.rod`/`.clb`/`.lbl` crack, the TTTEXT
                         name codec, `Codes.dat`, the fault-naming chain. Key reads:
                         `rod-labels.md` (the crack + the STRUC refutation, i.e. why
                         scaling is live-only), `tttext-codec.md` (→ names.json),
                         `fault-naming-hop.md` (number → words, end to end)
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
  FAST** — `vagcan setup` caches the parsed label files to SQLite under
  `~/.vagcan/data/<project>/cache.sqlite`.
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
