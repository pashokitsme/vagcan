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

## Safety (MANDATORY — read [`SAFETY.md`](SAFETY.md))

This tool only reads, and it has still cost the reference car its power steering: an
identifier sweep crashed the steering assist unit, twice, the second time permanently
(`research/eps/eps-j500-report-ru.md`). Read-only bounds what can be *changed* about a car,
not what can be *provoked*.

- **Never add a write service.** No coding, no adaptation, no clearing faults, no
  flashing. The UDS allowlist is `0x22`, `0x19`, `0x10`, `0x3E` and stays that way.
- **A sweep is a fuzz test of a diagnostic server.** It is the most invasive thing here.
  Guard anything new that resembles one the same way `survey` is guarded.
- **Anything that can change how a unit behaves is refused on a moving car** — checked
  by reading road speed, with "no answer" counted as moving.

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
  owner's car. Both are under `~/.vagcan/` — see `crates/vagcan/src/datadir.rs`, which
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

Goal, tech stack, architecture, and development workflow live in
**[`todo/GOAL.md`](todo/GOAL.md)**. The MVP task breakdown is in
**[`todo/README.md`](todo/README.md)**. Read both before working.

TL;DR: read the whole car over CAN with measurement scalings proven live on the car and
names from VW's own label files, on **tokio / edition 2024**, macOS M4, TDD with hardware
checkpoints. The live transport is a generic slcan USB-CAN adapter (`vag-can`); the HEX
clone is dead — crate and driver deleted, research archived under `archive/research/`.
`vagcan info` works on the real car. Details in `todo/GOAL.md`.

## Project structure

Rust workspace ([`README.md`](README.md)) + reverse-engineering research +
task tracking.

```
crates/          all Rust, split by what the code IS rather than what it does
  infra/           libraries. Nothing here is run directly.
    vag-transport    the transport seam every backend implements (sync + async)
    vag-can          slcan USB-CAN backend (the live path), listen-only, ISO-TP
    vag-protocol     UDS client + unit addressing (address.rs)
    vag-data         label parsers (.lbl/.clb/.rod) + LabelDb + ODX resolution
    vag-db           SQLite cache over the label files
    vag-capture      capture/replay transport (ReplayCan) for hardware-free tests
    vag-dash         the panel renderer — no_std, drawn on the board AND on the laptop
    vag-ble          BLE client (btleplug): scan, pick a device, open a NUS pipe
  bin/             what a person runs on a laptop.
    vag-cli          the CLI. Binary is `vagcan`. Top level = needs the car: devices /
                     info / units / properties / sniff / sensors / watch / scan / faults /
                     survey. Offline work is grouped by input: `recording …` (our own
                     `watch --out` recordings) and `vcds …` (VCDS's own files)
    vag-dash-config  binary `dashcfg` — configures the dash over BLE
  firmware/        what runs on the board. NOT workspace members: no_std for
                   riscv32imc-unknown-none-elf with their own build-std config.
    vag-dash         package `vag-dash-fw`, binary `dash` — the device
research/        RE writeups + tooling (NOT shipped), one directory per subject:
  labels/              VW's label files — the `.rod`/`.clb`/`.lbl` crack, the TTTEXT
                       name codec, `Codes.dat`, and the fault-naming chain. Key reads:
                       `rod-labels.md` (the crack + the STRUC refutation, i.e. why
                       scaling is live-only), `tttext-codec.md` (→ names.json),
                       `fault-naming-hop.md` (number → words, end to end)
  car/                 what the reference car answers: identifier map, the units
                       outside the powertrain, the whole-car survey, gearbox state
  eps/                 the steering-assist incident — read with SAFETY.md
  clb-crack/           RE scripts (usbpcap.py, link_cipher.py, framing_dis.py, decoders)
  dash/                the ESP32 board from the laptop's side. `probes/` is firmware
                       that answered a question (wifi-ap, wifi-scan, wifi-sta,
                       ble-scan); `host/` is the bench rig — `dashsim` (be the panel
                       and the buttons) and `bleecho`
.archive/        retired paths kept as evidence: research/ (HEX-clone framing, clone
                 crypto — negative results, do not retry), specs/ (superseded designs)
                 and tasks/done/ (finished task files)
todo/            task tracking → todo/README.md (roadmap), todo/GOAL.md (goal/stack/workflow);
                 finished task files retire to .archive/tasks/done/
```


Start-here docs: [`todo/GOAL.md`](todo/GOAL.md), [`todo/README.md`](todo/README.md),
[`ARCHITECTURE.md`](ARCHITECTURE.md), [`research/labels/rod-labels.md`](research/labels/rod-labels.md).

The three front-page documents split by audience and must stay split:
[`README.md`](README.md) is "is this for me, and how do I start" and nothing else;
[`USAGE.md`](USAGE.md) is every command with worked output and the multi-command
flows; [`ARCHITECTURE.md`](ARCHITECTURE.md) is why — the file formats, the setup
pipeline, the catalog schema, the crate layout.

## Tech stack & architecture (locked)

- **Rust edition 2024, MSRV 1.85. Async runtime: tokio.**
- **Connection-actor architecture** (NOT `Arc<Mutex<device>>`): one cable = one
  actor task owning the byte pipe; N async tasks query N ECUs concurrently via
  bounded `mpsc<Request{pdu, oneshot<Reply>}>`; the actor multiplexes/pipelines
  over the single serial link, owning seq counter + per-channel link keystream +
  ISO-TP state + timeouts. Clients get concurrency (latency-hiding), not wire
  parallelism. Multiple cables later = actor-per-cable.
- **Pluggable backend, static dispatch:** `trait Backend { async fn read/write }`
  (native async-fn-in-trait, no `dyn`/`async-trait`). The live backend is `SlcanBackend`
  over a serial port (`vag-can`); any future backend implements the same seam.
- `CableHandle` (cheap clone) implements the async transport `vag-protocol`'s UDS
  client rides. `vag-data`/`vag-db` stay sync (CPU-bound). **Label lookup must be
  FAST** — `vagcan vcds labels` caches the parsed label files to SQLite under
  `~/.vagcan/labels/cache.sqlite`.
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
  task is done+reviewed+merged, move its file to `archive/tasks/done/<subsystem>/<task>.md`
  (preserve the subsystem subdir). Each subsystem dir may carry a short `README.md`.
