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

### The CANable is not part of this device

Considered and rejected 2026-08-20: OBD → CANable → ESP32, with the ESP32 driving the
CANable. It is the natural thought, because the CANable is "the thing that reads CAN" in
every session so far — but it is a *bridge*, and it exists because **a laptop has no CAN
controller**. This chip does.

What the arrangement would cost: the CANable is a USB **device**, so the ESP32 has to be a
USB **host** — the USB Host stack plus a CDC-ACM driver in the firmware, 5 V of VBUS out
to it, the one USB port that would otherwise carry flashing and console, and any hope of
microamps asleep, because a host with a device attached is not a low-power state. All of
it to reach, through slcan's ASCII framing and somebody else's firmware, a bus that TWAI
reaches from the die.

What replaces it is a transceiver — SN65HVD230, TJA1051 or similar. Eight pins.

### The transceiver is not optional

Asked and answered 2026-08-20: can the ESP32 be soldered straight to the OBD plug?

**No.** TWAI gives logic-level TX/RX — it is a protocol controller, not a line driver.
CAN is a *differential* bus: recessive is both lines near 2.5 V, dominant is CAN-H ~3.5 V
against CAN-L ~1.5 V. The transceiver is the whole of the physical layer between those two
worlds. The same relationship a UART has to a MAX232, and nobody wires a UART pin into an
RS-232 socket either.

Three consequences, in increasing order of expense:

1. **It cannot work.** A GPIO cannot read a differential pair. It sees one wire loitering
   between 1.5 V and 3.5 V with no useful transitions against its own thresholds.
2. **The pin dies.** CAN-H reaches ~3.5 V in ordinary dominant state, already close to the
   ~3.9 V at which a 3.3 V input's clamp diodes conduct into the supply; a transient goes
   straight past them. Automotive CAN lines are required to survive a short to +12 V. A
   GPIO is not.
3. **It disturbs the bus.** A single-ended pin driving into a terminated 60 Ω differential
   pair is an unbalanced load, and the bad case is holding a line dominant and taking the
   powertrain bus down while the car is moving. This is the class of thing `SAFETY.md`
   exists for: a read-only tool has already cost this car its steering rack.

A transceiver also gives the common-mode range the pin has not got — ground at the OBD
socket and ground at the ECU differ by volts, and a transceiver tolerates ±12 V or more of
it where a GPIO has 0–3.3 V.

**A dev board with the transceiver already fitted counts** and is the easy path; the only
thing to check on one is that the 120 Ω jumper comes off.

**The display is the opposite case** — SPI, logic levels, wire it straight to the pins.
Nothing between the ESP32 and the panel.

**No 120 Ω terminator on the board.** The vehicle bus is already terminated at both ends
(~60 Ω); a third resistor drags it to 40 Ω and corrupts signalling. Established for the
sniffer (`docs/superpowers/specs/2026-07-31-can-sniffer-design.md`) and the rule is the
same here.

**TWAI is classic CAN 2.0, not CAN-FD.** Sufficient for everything read over `0x22`, and
named so nobody rediscovers it at bring-up. If FD is ever needed the answer is an external
controller over SPI (MCP2518FD), not a USB adapter.

## Hardware

- ESP32-S3 or C6 + CAN transceiver on TWAI. The owner has the board, with Wi-Fi and BT,
  and has run a `std` Rust toolchain on it (2026-08-20).
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
