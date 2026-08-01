# vagcan — project instructions

## Commit messages (MANDATORY)

- End every commit with an **`Assisted-By:`** trailer naming the AI model —
  **never** `Co-Authored-By:`. Example:

  ```
  Assisted-By: Claude Fable 5 <noreply@anthropic.com>
  ```

- Keep the `Claude-Session:` trailer line as well.
- Use Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:`…).

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
  vagcan           the CLI: devices / info / units / properties / sniff / sensors / watch /
                   scan / faults / survey / analyse / discover / calibrate / names / labels
research/        RE writeups + tooling (NOT shipped). Key reads:
  rod-labels.md        the .rod crack + the STRUC refutation (why scaling is live-only)
  tttext-codec.md      the TTTEXT name codec crack → catalogs/names-uds.json
  clb-crack/           RE scripts (usbpcap.py, link_cipher.py, framing_dis.py, decoders)
catalogs/        proven measurement rows + recovered names (see catalogs/README.md)
archive/         retired paths kept as evidence: research/ (HEX-clone framing, clone
                 crypto — negative results, do not retry) and specs/ (superseded designs)
docs/            active specs (docs/superpowers/specs/*.md, e.g. the CAN sniffer design)
todo/  done/     task tracking → todo/README.md (roadmap), todo/GOAL.md (goal/stack/workflow)
```

Start-here docs: [`todo/GOAL.md`](todo/GOAL.md), [`todo/README.md`](todo/README.md),
[`research/rod-labels.md`](research/rod-labels.md).
