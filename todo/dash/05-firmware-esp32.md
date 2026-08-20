# dash / 05 — the firmware: ESP32-S3, TWAI, SSD1322

**Subsystem:** dash · **Crate:** `vag-dash-fw` (new, outside the workspace) · **Needs the car:** yes, at the end

## Goal

Put `vag-dash` on the board: a TWAI backend behind `vag-transport`, the panel driver, the
three buttons, and the plan linked in.

Gated on `06` — the bench numbers decide the poll loop's shape.

## The seam

`vag-transport`'s **synchronous** `IsoTpTransport` carries this. `esp-idf` gives a real
`std` with threads, so there is no tokio on the board and no rewrite of ISO-TP: checked
2026-08-20, `isotp.rs`, `uds.rs`, `pdu.rs` and `read.rs` reach into `std` only for
`Duration`, and that is `core::time::Duration`. The sync trait has existed since the
beginning and has never had a real user; this is it.

`address.rs` is the one file in `vag-protocol` that touches the filesystem (it reads the
per-car unit table). The firmware does not need it — the plan carries unit addresses — so
it must be reachable without pulling that in. If it is not, that is the refactor this
task starts with.

The TWAI backend is a new implementation of the same trait `SlcanBackend` implements. It
does not touch the protocol crates.

## Hardware

- ESP32-S3 or C6 + CAN transceiver. The owner has the board, with Wi-Fi and BT, and has
  run a `std` Rust toolchain on it (2026-08-20).
- 256×32 OLED. **Confirm the controller before choosing the driver** — `ssd1322` 0.3.0
  (sync SPI) if it is an SSD1322. 4 bpp, 16 grey levels; a whole frame is ~30 KB, which
  the S3 can just hold, so keep a full frame buffer and blit the changed rectangle.
- Three buttons.

## The one thing that only matters because it lives in the car

**Sleep.** Pulled out into [`07-sleep.md`](07-sleep.md) and deferred (2026-08-20). It is
real work and it is not what blocks a first look at the panel.

**The car check.** Read the VIN and the part numbers of the units in the plan at start-up
and compare to what the plan says. On a mismatch, say so and do not poll. This firmware
is built for one car; `0x2029` on another engine will answer, and answer plausibly. A
wrong number shown confidently is the failure this project exists to avoid.

## Refusals

`0x22`, `0x19`, `0x10`, `0x3E`. No DTC clearing, no Haldex control, no coding — the
reference device does the first two and we do not. A device permanently in the car is the
last place to relax the allowlist, not the first.

## The Wi-Fi config portal

The board has it, so the reference device's "hold the middle button, ignition on" flow is
available. Scope for a first version: **brightness, buzzer, thresholds** — run-time
settings only. Changing *which channels are read* means rebuilding the plan on the
laptop, where the catalogs are. A portal that could add an identifier would be a sweep
with a web page in front of it.

## Tests

Most of this cannot be unit-tested; what can:

- The TWAI backend against `vag-capture`'s `ReplayCan`, off the board, like every other
  backend.
- The button state machine and the sleep detector as pure logic with a synthetic clock.
- `vag-dash` itself is already covered by `02`/`03` and does not change here.

## Done when

The board, on the bench with the car's ignition on, shows live values on the panel, and a
plan built for a different VIN refuses to poll.
