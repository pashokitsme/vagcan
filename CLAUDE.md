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

TL;DR: build **`vagcan info`** (VIN + car + equipment) on **tokio / edition 2024**,
connection-actor architecture, pluggable D2XX backend, macOS M4, TDD with hardware
checkpoints. Details in `todo/GOAL.md`.

## Project structure

Rust workspace ([`README.md`](README.md)) + reverse-engineering research +
vendored driver + task tracking.

```
crates/          the Rust workspace (all libs; a vagcan bin crate lands with cli-app)
  vag-transport    transport trait(s) — the seam every backend implements (sync + async)
  vag-hex          the physical HEX cable: Backend, D2XX, CableActor, frame, init, link cipher
  vag-protocol     UDS client + ISO-TP (transport-agnostic)
  vag-data         label parsers/decoders (.lbl/.clb/.rod) + LabelDb lookup  → crates/vag-data/README.md
  vag-db           SQLite cache over the label corpus
  vag-capture      capture/replay transport (ReplayCan) for hardware-free tests
research/        RE writeups + tooling (NOT shipped). Key reads:
  vag-hex-framing.md   the cable wire format + link cipher (capture ground truth)
  SCOPE-BOUNDARY.md    what we reverse (interop) vs refuse (anti-clone auth) — READ THIS
  clb-crack/           RE scripts (usbpcap.py, link_cipher.py, framing_dis.py, decoders)
driver/          vendored FTDI D2XX (darwin-arm64 dylib + win-arm64), tracked via Git LFS
docs/            specs (docs/superpowers/specs/*.md, e.g. the vag-hex transport design)
todo/  done/     task tracking → todo/README.md (roadmap), todo/GOAL.md (goal/stack/workflow)
```

Start-here docs: [`todo/GOAL.md`](todo/GOAL.md), [`todo/README.md`](todo/README.md),
[`research/SCOPE-BOUNDARY.md`](research/SCOPE-BOUNDARY.md),
[`research/vag-hex-framing.md`](research/vag-hex-framing.md).
