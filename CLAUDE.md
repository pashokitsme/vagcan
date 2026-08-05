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
  numbers, unit names, part numbers, coding bytes. Those come from the label corpus
  (`.rod` / `.lbl` / `.clb`), cached in SQLite, resolved per car through what the car
  itself reports (`F187` part number, `F19E` ODX file name, the gateway's installation
  list).
- **An offset or a magic number is a red flag.** Before writing one, establish whether
  it is a property of the *protocol* (ISO/UDS/OBD-II — fine, cite the standard) or of
  *this car* (not fine — it has to come from the corpus or from a read).
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
crates/          the Rust workspace
  vag-transport    transport trait(s) — the seam every backend implements (sync + async)
  vag-can          slcan USB-CAN backend (the live path), listen-only mode, ISO-TP sniffer
  vag-protocol     UDS client + ISO-TP (transport-agnostic) + unit addressing (address.rs)
  vag-data         label parsers/decoders (.lbl/.clb/.rod) + LabelDb + ODX file resolution
  vag-db           SQLite cache over the label corpus
  vag-capture      capture/replay transport (ReplayCan) for hardware-free tests
  vagcan           the CLI. Top level = needs the car: devices / info / units /
                   properties / sniff / sensors / watch / scan / faults / survey.
                   Offline work is grouped by what its input is: `recording …`
                   (our own `watch --out` recordings) and `vcds …` (VCDS's files —
                   labels, names, analyse, rod, corpus, tttext)
research/        RE writeups + tooling (NOT shipped), one directory per subject:
  labels/              VW's label corpus — the `.rod`/`.clb`/`.lbl` crack, the TTTEXT
                       name codec, `Codes.dat`, and the fault-naming chain. Key reads:
                       `rod-labels.md` (the crack + the STRUC refutation, i.e. why
                       scaling is live-only), `tttext-codec.md` (→ names-uds.json),
                       `fault-naming-hop.md` (number → words, end to end)
  car/                 what the reference car answers: identifier map, the units
                       outside the powertrain, the whole-car survey, gearbox state
  eps/                 the steering-assist incident — read with SAFETY.md
  clb-crack/           RE scripts (usbpcap.py, link_cipher.py, framing_dis.py, decoders)
catalogs/        proven measurement rows + recovered names (see catalogs/README.md)
archive/         retired paths kept as evidence: research/ (HEX-clone framing, clone
                 crypto — negative results, do not retry), specs/ (superseded designs)
                 and tasks/done/ (finished task files)
docs/            active specs (docs/superpowers/specs/*.md, e.g. the CAN sniffer design)
todo/            task tracking → todo/README.md (roadmap), todo/GOAL.md (goal/stack/workflow);
                 finished task files retire to archive/tasks/done/
```

Start-here docs: [`todo/GOAL.md`](todo/GOAL.md), [`todo/README.md`](todo/README.md),
[`research/labels/rod-labels.md`](research/labels/rod-labels.md).
