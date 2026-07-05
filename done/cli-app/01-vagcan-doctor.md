# cli-app / 01 — `vagcan` binary + `doctor` (live PoC #1)

**Subsystem:** cli-app · **Crate:** new `vagcan` (bin) · **Wave:** 3 · **Depends:** init-handshake (done), cable-actor (done), usb-backend (done)

## Goal
The `vagcan` binary with a `doctor` subcommand that opens the real HEX cable, runs the
plaintext handshake, and prints the cable identity. This is the runnable live PoC #1 /
hardware checkpoint. Keep the CLI MINIMAL and extensible — no polished UI (per `todo/GOAL.md`):
just a clean command surface we grow later.

## Deliverables
- New crate `crates/vagcan` (bin), added to root `[workspace] members` (you are the only task
  this wave touching root Cargo.toml — keep it to the members line). Deps: `vag-hex` (path),
  `tokio` (workspace, features `rt-multi-thread`, `macros`), a minimal arg parser (`clap` with
  `derive` is fine), `anyhow` for the bin's top-level error.
- `#[tokio::main]` entry with a subcommand enum. Implement **`doctor`**:
  ```
  vagcan doctor [--serial <FTDI_SERIAL>]
  ```
  → `D2xxBackend::open(serial)` → `vag_hex::spawn(backend)` → `vag_hex::handshake(&handle).await`
  → print the identity (firmware string + raw bytes hex). On failure print a clear diagnostic
  (cable not found / handshake error) and exit non-zero.
- Scaffold a `decode` subcommand as a documented stub (returns "not yet wired" / `todo`), since
  the link-decode port lands in parallel — a follow-up wires it. Do NOT implement decryption here.
- Read-only by construction: `doctor` only opens + identifies; no diagnostic writes.

## TDD
- The hardware path can't be unit-tested, but keep logic testable: put identity RENDERING
  (`CableIdentity` → display string) in a small pure function with a unit test (using the captured
  identify bytes), so `doctor`'s output formatting is covered without a cable.
- `cargo build -p vagcan` + `cargo test -p vagcan` green; clippy `-D warnings` clean.

## Done criteria
- `cargo build --workspace` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `vagcan doctor --help` works. Commit in worktree, mandatory trailers.

## Hardware checkpoint (after merge)
On the M4 with the cable plugged: `cargo run -p vagcan -- doctor` must open the cable and print
the real "ROSSTECH" + version identity. I stop and ask the user to run this — the visible PoC.
