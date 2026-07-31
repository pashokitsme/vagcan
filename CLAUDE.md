# vagcan — project instructions

## Commit messages (MANDATORY)

- End every commit with an **`Assisted-By:`** trailer naming the AI model —
  **never** `Co-Authored-By:`. Example:

  ```
  Assisted-By: Claude Fable 5 <noreply@anthropic.com>
  ```

- Keep the `Claude-Session:` trailer line as well.
- Use Conventional Commits (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:`…).

## Project

Goal, tech stack, architecture, and development workflow live in
**[`todo/GOAL.md`](todo/GOAL.md)**. The MVP task breakdown is in
**[`todo/README.md`](todo/README.md)**. Read both before working.

TL;DR: read the whole car over CAN with measurement definitions taken from VW's own label
files, on **tokio / edition 2024**, macOS M4, TDD with hardware checkpoints. The live
transport is a generic slcan USB-CAN adapter (`vag-can`); the HEX clone is parked research.
`vagcan info` works on the real car. Details in `todo/GOAL.md`.

## Project structure

Rust workspace ([`README.md`](README.md)) + reverse-engineering research +
vendored driver + task tracking.

```
crates/          the Rust workspace
  vag-transport    transport trait(s) — the seam every backend implements (sync + async)
  vag-can          slcan USB-CAN backend (the live path), listen-only mode, ISO-TP sniffer
  vag-protocol     UDS client + ISO-TP (transport-agnostic)
  vag-data         label parsers/decoders (.lbl/.clb/.rod) + LabelDb + ODX file resolution
  vag-db           SQLite cache over the label corpus
  vag-capture      capture/replay transport (ReplayCan) for hardware-free tests
  vagcan           the CLI: devices / info / properties / sniff / scan / labels
  vag-hex          the HEX clone: parked research, not a product path
research/        RE writeups + tooling (NOT shipped). Key reads:
  vag-hex-framing.md   the cable wire format + link cipher (capture ground truth)
  clb-crack/           RE scripts (usbpcap.py, link_cipher.py, framing_dis.py, decoders)
driver/          vendored FTDI D2XX (darwin-arm64 dylib + win-arm64), tracked via Git LFS
docs/            specs (docs/superpowers/specs/*.md, e.g. the vag-hex transport design)
todo/  done/     task tracking → todo/README.md (roadmap), todo/GOAL.md (goal/stack/workflow)
```

Start-here docs: [`todo/GOAL.md`](todo/GOAL.md), [`todo/README.md`](todo/README.md),
[`research/vag-hex-framing.md`](research/vag-hex-framing.md).
