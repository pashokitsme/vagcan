# dash — the OLED frontend

**Opened 2026-08-20.** A second frontend for `vagcan`: a 256×32 OLED that lives in the
car's air vent, plugged into OBD, showing a handful of numbers while you drive. The
terminal UI (`watch`) is for a laptop on the passenger seat; this is for the thing you
glance at doing 120.

Reference for the shape: a commercial device the owner has seen in the flesh — four
columns, small label over a large number, three buttons under it. We copy the shape, not
its feature list.

## The one architectural decision

**The device resolves nothing. It executes a plan.**

`vagcan dash build` runs on the laptop, where the catalogs are, and emits a plan: for
every cell on every page, the unit address, the identifier, bit offset and length, byte
order, the linear scaling, the unit string, and the label **already rendered** in the
chosen language. The firmware links that plan in and does exactly one thing at run time:
send `0x22`, take the bits, multiply, draw.

Three reasons, and the third is the one that decides it:

1. **It does not fit.** Measured 2026-08-20: `~/.vagcan/data/SK37X/cache.sqlite` is
   **88 MB**, with 100 MB of `.rod` beside it. An ESP32-S3 has 512 KB of SRAM, at best
   8 MB of PSRAM and 8–16 MB of flash. Two orders of magnitude short — so "put SQLite
   on the device in memory" is settled by arithmetic, not by taste.
2. **There is nothing for a query engine to do.** A catalog exists in order to be
   *searched*, and searching is an act of build time. On the road the device needs
   forty rows and all forty are known before it is flashed.
3. **A plan cannot sweep.** An identifier sweep is a fuzz test of a control unit's
   diagnostic server, and this device has no business running one. A device that holds
   forty identifiers proven on the bench cannot ask for a forty-first — not because it
   was asked not to, but because it has no mechanism to.

## Built for one car, and that is allowed

The owner's decision, 2026-08-20: this device is for **their** car. Car-specific data
baked into the firmware is fine; generating the firmware from the catalogs is fine.

That does not weaken `CLAUDE.md`'s "no car-specific data in the code". It satisfies it
literally: the checkout still contains none. The generator produces it, and

- the generated plan and the firmware image are **gitignored** — the data behind them is
  VW's and Ross-Tech's, exactly as the label files are;
- they are written under `~/.vagcan/` or `target/`, never into the checkout;
- `vag-dash-render` itself is generic — it takes a plan. A universal version later reads the
  plan from flash instead of linking it in. The decision is reversible by construction.

And because the firmware is built for one car, it can **check that it is in that car**:
the VIN and the part numbers of the units in the plan are baked in, read back at
start-up and compared. This is not paperwork. `0x2029` on this engine is boost pressure;
on another car's engine it is some other quantity that will answer, and answer with a
plausible number. The danger has never been the refusal — it is the confident wrong
value.

> **Superseded in part, 2026-08-25.** The board on the bench is an **ESP32-C3 SuperMini**,
> not the WROOM-32 described below. Wi-Fi AP, DHCP and a web server run on it today; the
> measurements, the stack decision (`no_std`/esp-hal) and what it voids in `05` and `09`
> are in [`10-c3-recon.md`](10-c3-recon.md); BLE — which the C3 has and the WROOM's SPP
> is replaced by — is in [`11-ble.md`](11-ble.md), and settings that survive a power cut
> are in [`12-settings.md`](12-settings.md).

## Hardware — all of it already on the bench (2026-08-20)

Nothing to buy. Both boards are on hand and the second one turned out to carry the part
the first one lacks.

- **ESP32-WROOM-32** devkit, USB-C. The classic Xtensa part, not an S3, and that cuts both
  ways. Against: **no USB peripheral at all** (its USB-C goes to a CP2102 bridge), so it
  can never host anything. For: **Bluetooth Classic**, hence SPP — which is what makes the
  wireless-adapter idea in `09` nearly free, where a BLE-only S3 would have needed a
  custom GATT service and a custom client per platform. TWAI is present, as on every ESP32.
- **MKS CANable V2.0 Pro**, already soldered to an OBD plug: **STM32G431C8** in LQFP48,
  **ADM3050E** isolated CAN FD transceiver, **Hi-Link B0505S-1WR3** isolated 1 W DC-DC for
  the bus side.
- **256×64 OLED, 3.12″, SSD1322** — the part to buy, settled 2026-08-20. Driver crate
  `ssd1322` 0.3.0 (sync SPI) or `ssd1322_rs` 0.3.1. 4 bpp, sixteen grey levels, ~30 KB for
  a full frame buffer, which the ESP32 can simply hold.

  **256×32 was the original target and is not a part you can buy.** Every 3.12″ SSD1322
  module on the market is 256×64, and the height difference is smaller than it sounds: the
  pixel is 0.3 mm square, so 64 rows is a **19 mm** active area against 9.6 for 32. Both are
  a thin strip; only one exists.

  The extra rows are not a consolation. Rendered the same nine frames at 64 with no change
  to the renderer — it reads its height from the target — and the chart is a different
  instrument: at 32 rows the boost trace was a squiggle, at 64 the ripple, the spool shape
  and the dip at the shift are all legible. The values page, by contrast, works but does not
  yet *use* the height (label at the top, number at the bottom, a gap between). That is
  layout work in `02`, not a limitation, and at 64 rows it can be either the reference
  photograph's four tiers or two rows of four cells.

  What to check when ordering: **7-pin SPI, not the 16-pin parallel version** (those want
  0 Ω jumpers moved to select 4-wire SPI); **temperature range** — Winstar-grade parts are
  −40…+85 °C where cheap ones stop at +70, and a vent in summer sun goes past that;
  **yellow or green** rather than white or blue, for night vision; and confirm the module
  has its own charge pump, which the SSD1322 provides for the ~14.5 V the panel wants.

### The bezel, and why it does not matter

There is no bezel-less OLED, and the reason is the glass rather than anybody's parsimony:
a sealing frit runs around the emissive area and the driver is bonded to the glass or its
flex. On the `WEX025664B` the outline is 88.0 × 27.8 mm against an active area of
76.778 × 19.178 — 5.6 mm at each side, 4.3 mm above and below.

But two different borders get seen as one, and only the first is avoidable:

- **The carrier PCB.** Many modules are that panel glued to a breakout of about
  100.5 × 33.5 mm — twelve millimetres of width and six of height *on top of the glass*.
  A bare COF panel drops them. The price is a 30-pin 0.5 mm FPC connector and the support
  parts (`IREF` resistor, charge-pump capacitors) on your own board: the finest soldering
  in the whole build, finer than the tap on the ADM3050E.
- **The glass border.** Not removable at any price.

**So do not fight it — hide it behind a dark faceplate**, which is what the device in the
reference photograph does: there is no visible screen in that vent, only digits floating
in dark gloss. Smoked acrylic, or clear acrylic with automotive tint film. The bezel
disappears because nothing but lit pixels is visible through it.

And the contrast **improves**. Ambient light crosses the filter **twice** — in, and out
again after reflecting off the black bezel — so it is attenuated by T², while the emitted
light crosses once and is attenuated by T. At 30 % transmission the reflections fall to
9 % and the digits to 30 %: a threefold gain. It is why every instrument cluster in every
car sits under smoked plastic.

Which settles the choice: **buy the ordinary breakout and put it behind acrylic.** The
vent mount is a printed shell in any case and the faceplate is part of it; twelve extra
millimetres of PCB behind the fascia bother nobody, because only the aperture is seen.
- **Three buttons.**
- **Power from OBD pin 16**, permanent battery positive per SAE J1962 — live whenever the
  car is parked, so the regulator's own quiescent draw is the real budget (`08`).

**The transceiver is shared.** The ESP32 uses its own TWAI and taps the ADM3050E's logic
side: `RXD` is an output, so a second listener is free; `TXD` is an input and wants one
driver at a time. No bridge between the two processors, no fork of anybody's firmware in
the data path, and the CANable is still a CANable when it is plugged into a laptop. The
reasoning, and the two wrong turns it took to get there, are in `05`.

`esp-idf` gives a real `std` with threads, which means the **synchronous** side of
`vag-uds-transport` (`IsoTpTransport`) carries the device. No tokio on the board and no rewrite
of ISO-TP: `isotp.rs`, `uds.rs`, `pdu.rs` and `read.rs` reach into `std` for `Duration`
alone, and that is `core::time::Duration` (checked 2026-08-20).

## Graphics stack (surveyed 2026-08-20)

`embedded-graphics` 0.8.2 is the whole ecosystem — it draws nothing itself, it defines
`DrawTarget`, and drivers implement it. On top:

- **`eg-seven-segment` 0.2.0** — the large numbers. Drawn from primitives, so the size is
  a parameter rather than a second set of bitmaps; the reference device's "large font"
  setting becomes one integer.
- **`u8g2-fonts` 0.8.0** — the labels. A port of the whole U8g2 collection, Cyrillic
  included, which the stock `embedded-graphics` fonts are not.
- **`embedded-layout` 0.4.2** for the grid, **`embedded-canvas` 0.3.2** for off-screen
  composition (partial update — do not push 30 KB over SPI to change one digit).
- **`embedded-graphics-simulator` 0.8.0** for the desktop. With SDL2 it opens a window;
  with `default-features = false` the same code emits PNG, and `EG_SIMULATOR_DUMP` dumps
  a frame and exits. That is what makes the layout **screenshot-testable** rather than
  eyeballed.

Rejected: **`mousefood`** (the ratatui backend for `embedded-graphics`, 0.5.2, alive) —
tempting, because it would reuse `watch`'s ratatui code, but a character grid gives every
cell the same height and this design lives on the label being a third of the number.
**Slint** supports ESP32-S3 officially and wants up to 300 KB of RAM for colour touch
panels; wrong tool for a monochrome strip with three buttons. **LVGL** — C, and the Rust
bindings are stale.

## What is refused, and stays refused

The reference device clears DTCs and switches Haldex modes. **We do neither.** Both are
write services; the UDS allowlist is `0x22`, `0x19`, `0x10`, `0x3E` and does not move
for a device that lives permanently in the car. Reading faults (`0x19`) is fine.
(The owner agreed, 2026-08-20; Haldex and air suspension are moot on this car in any
case — the survey finds no `0x22` unit and no air suspension, so it is front-wheel drive.)

## The three views (settled 2026-08-20)

1. **Chart** — one channel: the value large on the left, a sparkline on the right. One
   pixel per poll, so the window is width ÷ rate: ~19 s at 10 Hz on 190 px. **Fixed**
   vertical scale, not autoscale — autoscale turns a flat trace into drama and a real
   collapse into a flat trace.
2. **Values** — four columns of 64 px: a 6 px label over a ~20 px number. The photograph.
   Four cylinders fall into the same frame with no change: one column per cylinder.
3. **Alarm** — *not* a page. A rule: a channel, a threshold, a direction, and the view it
   raises. See `04-alarms.md`; the behaviour is where this gets hard, not the drawing.

Navigation is **one-dimensional**: "selectable" is implemented as separate pages (a chart
page per channel, a values page per preset), so two buttons cycle everything and the
third is free for "silence this alarm" plus a long press for brightness.

Preset *contents* are fixed at build time, generated from `[favourites]` and `[charted]`
in `config.toml` — which `watch` already writes, per VIN. *Which* preset is showing is a
run-time thing, on a button.

## What this car actually answers (catalog, 2026-08-20)

Engine `EV_ECM18TFS0208V0906264H_001` (`8V0906264H`, TFSI), gearbox
`EV_TCMDQ200021_001` — **DQ200, dry clutch**.

| quantity | unit | DID | units |
|---|---|---|---|
| Engine torque | engine | `437C` | Nm |
| Boost, actual / commanded | engine | `202A` / `2029` | bar |
| Crankshaft speed | engine | `206E` | 1/min |
| Vehicle speed (averaged) | engine | `2033` | km/h |
| Oil / coolant / intake / fuel temperature | engine | `202F` / `3E0A` / … / `203E` | °C |
| Air flow rate (averaged) | engine | `2032` | kg/h |
| **Ignition retard, cyl 1–4** | engine | `200A`–`200D` | ° |
| **Misfires per 1000 rev, cyl 1–4** | engine | `291D`–`2920` | — |
| Misfire sum, cyl 1–4 | engine | `2966`–`2969` | — |
| Displayed gear | gearbox | `3816` | — |
| Transmission input speed | gearbox | `380A` | 1/min |
| **Clutch slip speed** | **engine** | `4E60` | 1/min |

That last row matters more than it looks. "The difference between gearbox and engine
speed" otherwise means subtracting a value from `0x7E0` from a value from `0x7E1` — two
control units, two clocks, two deadlines — and during a shift, which is the only moment
anyone cares about, the two samples are not simultaneous. The difference of two readings
taken at different instants is not slip, it is an artefact of the clocks drifting apart;
this project has already bought one false proof that way (`watch/history.rs`, the gear
evidence). `IDE10634 Clutch_protection_function_clutch_slip_speed` is computed by the
engine ECU itself and read as **one number from one unit**.

The other way out would be to take both speeds from the gearbox, and that instinct is
sound — the gearbox does carry engine-side data, it declares `100C Motordrehmoment`. But
checked 2026-08-20: **it does not declare engine speed.** Everything it offers in `1/min`
is its own — output shaft (`1009`), input shafts (`380A`, `380C`/`380D`), idle target
(`3871`). It may well answer an undeclared identifier that holds it; going to look is a
sweep, and sweeps are what destroyed the rack.

So `4E60` is the primary. Subtraction of engine `206E` and gearbox `380A` stays as the
fallback if it turns out silent on the road — and if it is used, it needs an explicit
freshness rule (both samples inside one window, or no number at all), never a bare
subtraction of whatever each unit last said.

Two items from the owner's original list are gone on hardware grounds rather than by
refusal: **Haldex** (no `0x22` unit — front-wheel drive) and **air suspension**. And
"DSG temperature" does not mean on a dry DQ200 what it means on the DQ250 in the
photograph: there is no clutch oil. There is a mechatronic temperature (`028D`), and a
clutch temperature model that in the declared set appears only inside snapshot blocks
(`IDE80032`) — a live one has to be looked for on the car.

## Status (2026-08-20) — the panel renders

`vag-dash-render` exists and draws. Nine frames are in `~/.vagcan/dash/preview/`, regenerated by

```
cargo run -p vag-dash-render --example panel -- ~/.vagcan/dash/preview
```

It is the real renderer, not a mock-up: the same code that will drive the OLED, pointed at
`embedded-graphics-simulator` with SDL switched off, so it needs neither a window nor a
display and writes PNG.

**Shipped:** the `no_std` crate (`frame`, `theme`, `render`), both page kinds, the alarm
inversion, the numeral ladder, the `Report`, and nine tests. 1051 workspace tests green,
clippy and fmt clean.

**Three defects the first renders caught**, all of the same family and each now a test:

- The **degree sign vanished silently.** u8g2's `_t_cyrillic` faces carry no `°` —
  measured against nine of them — `render()` returned `Err`, and a `let _ =` ate it. Two
  faces now, for two jobs: labels are words in the reader's language, units are `°C`,
  `bar`, `Nm` and never Cyrillic. A glyph a face lacks is reported.
- **A row has to decide as a row.** With each cell choosing for itself, one cylinder kept
  its `°` while three lost theirs, and one reading sat a size below its neighbours. Both
  differences are visible at a glance and neither means anything — so the eye reads the
  odd cell as the important one, which is backwards.
- **The chart header truncated after nine characters.** Sixteen bytes looked ample for
  `"1234"`; Cyrillic costs two bytes each.

**Open decision:** the typeface for the numbers — frames 1–3 are the same page in
Inconsolata Bold (monospaced, no digit jitter, closest to the reference photograph),
FreeUniversal Bold (proportional, heavier), and seven segments. Everything downstream is
drawn in whichever wins.

**Waiting on hardware:** the panel itself. 256×32 is not the SSD1322's usual 256×64, so
the driver crate follows from the part number (`06` §8).

## Order

**Now:** `02` (render crate) + `03` (simulator) together — the simulator is how `02` is
tested, and the two of them are what turns the layout from a claim into a PNG somebody
can look at. `01` (the plan format) firms up alongside them, driven by what the renderer
actually needs rather than guessed in advance.

**Then:** `04` (alarms) → `05` (firmware), gated on `06`, the list of things only the car
and the bench can answer.

**Then:** `09` (the device as a wireless CANable — **over Wi-Fi TCP, not Bluetooth SPP**; see `10`) — nearly free, given
the board is the classic ESP32 and `vag-uds-can`'s slcan backend is already stream-generic.

**Deferred:** `07` (sleeping in the car) and `08` (power). One problem from two sides —
`07` is what the firmware does to draw nothing, `08` is whether the hardware lets it.
Required before the device spends a night plugged in; not required before it exists.

Sleep is settled in shape (2026-08-20): **wake on the rail at 13 V**, a divider into an
ADC that works with the whole CAN side unpowered; **sleep on the ignition going off**,
read from the bus while we are still awake to read it, **or fifteen minutes idle**. Cheap
sensor to come up, rich signal to go down. "No answer" plays no part — it is the same
input the moving-car guard reads as *moving*.
