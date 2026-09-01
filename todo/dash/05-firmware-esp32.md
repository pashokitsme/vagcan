# dash / 05 — the firmware: ESP32-S3, TWAI, SSD1322

**Subsystem:** dash · **Crate:** `vag-dash-fw` (new, outside the workspace) · **Needs the car:** yes, at the end

> **The board changed, 2026-08-25.** The bench board is an **ESP32-C3 SuperMini**, not the
> WROOM-32 this document was written against: RISC-V on stable Rust, native USB, **BLE only
> — no Bluetooth Classic, no SPP**, 22 GPIO. What was proven on hardware, and what it voids
> here, is in [`10-c3-recon.md`](10-c3-recon.md). Read that first.

## Goal

Put `vag-dash-render` on the board: a TWAI backend behind `vag-uds-transport`, the panel driver, the
three buttons, and the plan linked in.

Gated on `06` — the bench numbers decide the poll loop's shape.

## The seam

`vag-uds-transport`'s **synchronous** `IsoTpTransport` carries this. `esp-idf` gives a real
`std` with threads, so there is no tokio on the board and no rewrite of ISO-TP: checked
2026-08-20, `isotp.rs`, `uds.rs`, `pdu.rs` and `read.rs` reach into `std` only for
`Duration`, and that is `core::time::Duration`. The sync trait has existed since the
beginning and has never had a real user; this is it.

`address.rs` is the one file in `vag-uds-client` that touches the filesystem (it reads the
per-car unit table). The firmware does not need it — the plan carries unit addresses — so
it must be reachable without pulling that in. If it is not, that is the refactor this
task starts with.

The TWAI backend is a new implementation of the same trait `SlcanBackend` implements. It
does not touch the protocol crates.

### The CANable stays — as the transceiver, not as a bridge

Settled 2026-08-20, after two wrong turns worth recording so neither is taken again.

**Wrong turn one: OBD → CANable → ESP32, with the ESP32 driving the adapter over USB.**
Impossible on this board, and not for a soft reason. The module is an **ESP32-WROOM-32**,
the classic Xtensa part, and it has **no USB peripheral at all** — the USB-C on the devkit
goes to a CP2102 bridge, itself a USB *device*. Two USB devices and no host; joining their
sockets is joining two flash drives. (An S3 could host. This is not an S3.)

**Wrong turn two: buy a transceiver.** Correct in principle, and unnecessary — there is
already a good one in the box.

**What is actually done.** The CANable Pro carries an **ADM3050E**, Analog Devices'
isolated CAN FD transceiver, and the ESP32 shares it. Its logic side is two pins and they
divide differently:

- **`RXD` is an output.** A second listener on it is free — the ESP32 reads exactly what
  the STM32 reads, and there is no conflict to have.
- **`TXD` is an input**, so one driver at a time. Either the idle side holds its pin
  high-impedance, or both drive open-drain into one pull-up — which is the same wired-AND
  that CAN itself runs on, dominant being the zero.

So the ESP32 uses **its own TWAI**, natively. No bridge, no slcan on the device, no fork
of anybody's firmware in the data path. And the CANable is still a CANable: plug it into a
laptop and it is the adapter it always was.

**Where to solder:** to the ADM3050E, not to the processor. It is SOIC-8 at 1.27 mm pitch
— twice the STM32's 0.5 mm — with `RXD` and `TXD` adjacent on the logic side.

**The one contention to resolve is `TXD`.** The STM32's `PB9` is configured push-pull by
the stock firmware and idles recessive, i.e. high; an ESP32 pulling the same line low
fights it through tens of milliohms. Three ways out, best last: cut power to the STM32
(ugly — current then finds its ESD diodes), make both ends open-drain with a pull-up, or
**a few lines of firmware** leaving `PB9` an input until the USB host opens the channel.
[Elmue's CANable 2.5 firmware](https://github.com/Elmue/CANable-2.5-firmware-Slcan-and-Candlelight)
is written for exactly this STM32G431 on exactly these isolated MKS boards and is the base
to start from. Flashing needs no ST-Link: the G431 has a USB DFU bootloader in system
memory and the board exposes a `BOOT` pad.

**The 120 Ω jumper on the CANable stays open**, as it does today for sniffing.

### Pin allocation

Settled 2026-08-20. The ESP32 routes peripherals through a GPIO matrix, so TWAI can sit on
almost any pin — but "almost" carries three real exclusions and one of them is specific to
this wiring.

| function | pin | why this one |
|---|---|---|
| TWAI TX → ADM3050E `TXD` | **D27** | plain GPIO, not a strapping pin |
| TWAI RX ← ADM3050E `RXD` | **D26** | same |
| Panel SCK | **D18** | VSPI default |
| Panel MOSI | **D23** | VSPI default |
| Panel CS | **D5** | |
| Panel DC / RST | **D17 / D16** | free on WROOM-32 (a WROVER would have PSRAM here) |
| Buttons | **D32, D33, D25** | D32/D33 are RTC-capable, so they can also wake |
| Divider from OBD pin 16 | **D34** | must be ADC1 — see below |
| Load switch for the CANable | **D14** | |

MISO is unused; the panel is write-only.

**Do not put RX on D12.** It is `MTDI`, and its level at reset selects the flash supply
voltage: held high the chip decides the flash is 1.8 V and does not boot. The
transceiver's `RXD` idles **high** — recessive — so this exact wiring would stop the board
starting, with a cause nobody would guess.

**D34, D35, VP and VN are input-only.** No output driver, no pull-ups. TX cannot go there;
a divider can, and does.

**TX0/RX0 are the console** through the CP2102. Taking them costs flashing and logs.

`D2`, `D5` and `D15` are strapping pins. They work, but they add noise at boot, so nothing
important goes on them beyond the panel's chip select.

**The divider must be on ADC1** — `GPIO32…39`. On the classic ESP32, **ADC2 does not work
while Wi-Fi is on**: the radio takes it, and reads either block or come back as rubbish.
Wi-Fi and Bluetooth are both wanted (`09`), so ADC2 is out for good, not merely awkward.

**Common ground between the two boards** is required for the logic-side tap. Both sides
are 3.3 V, so no level shifter.


## Hardware

- **ESP32-WROOM-32** (classic Xtensa LX6) devkit with USB-C, on hand. Has TWAI. Has
  Wi-Fi and — the part that matters for `09` — **Bluetooth Classic**, so SPP is available.
  The owner has run a `std` Rust toolchain on it (2026-08-20).
- **MKS CANable V2.0 Pro** on hand, already soldered to an OBD plug: STM32G431C8 in
  LQFP48, ADM3050E isolated transceiver, Hi-Link B0505S-1WR3 isolated DC-DC for the bus
  side. The transceiver is shared with the ESP32; see above.
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

- The TWAI backend against `vag-uds-capture`'s `ReplayCan`, off the board, like every other
  backend.
- The button state machine and the sleep detector as pure logic with a synthetic clock.
- `vag-dash-render` itself is already covered by `02`/`03` and does not change here.

## Done when

The board, on the bench with the car's ignition on, shows live values on the panel, and a
plan built for a different VIN refuses to poll.
