# dash / 22 — the glass: an SSD1322 on the board

**Opened 2026-09-30**, the day the panel arrived. Until now the board's picture went only to
`dashsim` over the cable; this puts the same framebuffer on a 3.12″ 256×64 OLED.

## State (2026-09-30)

- **The glass draws `dash`'s pages** on the rev v1.1 board with BLE, bench and car (owner);
  `dashcfg`'s `set brightness N` reaches it. The wiring is in `README.md` ("OLED wiring").
- **The wiring in `README.md` is the standard for every board** (owner, 2026-10-03): SCLK
  `GPIO10`, SDIN `GPIO8`, D/C `GPIO0`, CS `GPIO20`, RES `GPIO21`. It came from the rev v1.1
  board: after the owner rewired CAN on it the glass showed garbage, then stayed dark, while the
  board drew the right page (its `FRAME` lines over USB), and it lit again once the firmware
  followed how that board was wired — SDIN on `GPIO8` (the blue LED's pin, so the LED flickers
  with the data and `dash` no longer blinks it), CS and RES swapped. Whether `GPIO7` on that
  board is dead was not checked. On 2026-10-03 the rev v0.4 board was wired the same way and
  draws `dash`'s pages (without BLE) with the image from `master` (owner).
- **The rev v1.1 board stopped enumerating on USB** (2026-10-03): it takes power, but the Mac
  sees no device, or one for a second; not diagnosed. Holding BOOT through a reset (download
  mode) is the next thing to try.

## The module

Blue board, silk `3.12" OLED Ver: 2.1`, flex marked `SSD1322U`, a 2×8 pin header fitted (even
pins on the edge row, odd on the inner). Its own boost converter for the panel supply (`U1`,
`L1`, `D1`): 14.6 V on `C6`.

- **Interface links.** `R5`/`R6` set `BS1` (0/1), `R7`/`R8` set `BS0` (1/0); the silk table:
  4SPI = `R5`+`R8`, 3SPI = `R5`+`R7`, 80XX = `R6`+`R8`, 68XX = `R6`+`R7`. It came as 80XX; the
  owner moved `R6` to `R5`.
- **Supply.** From the board's `5V` pin. The module regulates its own logic supply: 3.1 V on
  the `(1)` pad of `R7` with 5 V on pin 2 (owner, multimeter).

## What is built

- `crates/dash/vag-dash-fw/src/ssd1322.rs` — the driver over `embedded-hal` traits: reset and
  set-up (the scan clock `B3 F1`, why in `INIT`), rows widened from 1 to 4 bits a pixel as they
  are sent, `Shown` (a checksum a row, so a frame sends only the span of rows that changed),
  brightness.
- `dash` — the panel task puts every frame on the glass, adapter screen included; the whole
  picture and the brightness are resent every 10 s, because nothing can be read back. The
  settings' `brightness` (0..255) is the controller's contrast current; it had no consumer
  before.
- `oledtest` — a bench image: border, `vagcan`, `TL`, a diagonal, then a dim picture and a
  checkerboard. Transmits nothing on CAN.
- `research/dash/host/tests/glass_wire.rs` — the driver compiled for the laptop, the wire read
  byte by byte.
- **RAM:** static +1,496 B with BLE (139,500 → 140,996), +1,488 B without (130,048 → 131,536).
  416 B is the glass's own (`OLED`: 256 B of checksums, a 128 B row); the rest is esp-hal's
  SPI driver and an 832 B `Debug` table for pin signals its pin routing links.

## Open

1. **Which way up.** `start(.., turned)` is `false` in `dash`. Decide with the housing
   (`15-enclosure.md`); if it varies, it is a `dash.toml` setting.
2. **Burn-in shift — set aside by the owner (2026-09-30):** burn-in will not come quickly.
   Not built. The design, for when it is wanted: shift where the rows are sent
   (`Oled::show`), not in the renderer; one pixel a minute round a 3×3 grid of offsets; a
   random start each boot, because trips are short; the renderer draws 254×62 so nothing is
   cut off — how its layout takes that size was not checked. Brightness and dimmer labels
   (item 4) do more against wear than a shift does.
3. **The housing** — `15-enclosure.md`.
4. **Grey levels** — the renderer is one bit a pixel; a dim label under a bright number is the
   renderer's open item, not this task's.
