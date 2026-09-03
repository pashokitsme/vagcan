# dash / CAN bring-up on the car — hand-off

**State, 2026-09-04.** The firmware polls the car for real (`todo/dash/05`), it has
been flashed and run on the reference car once, and **no control unit answered**.
Two firmware defects were found and fixed on the way; a third, hardware-side
cause is still open. This document is what the next session needs so it does not
repeat the search.

Everything below was measured, not reasoned about. Where something is an
inference it says so, and one inference that turned out to be wrong is recorded
too, because it cost an evening and would cost the next one.

## 1. The symptom

`vagcan dev dash build` resolves 4 channels on 2 units for VIN
`XW8AD4NE9JH008917` — engine `7E0`/`7E8` (`8V0906264H`) and gearbox `7E1`/`7E9`
(`0CW300041G`). The image was flashed, the board plugged into the car's OBD
socket, **ignition on, engine running**. The whole of what the device said:

```
plan: VIN XW8AD4NE9JH008917 — 2 unit(s), 4 channel(s)
can: swept 32 stale frame(s) before 7E0 F187 — a late reply, or another tester
can: 7E0 did not answer F187 (transport: timeout) — will keep asking
can: 7E1 did not answer F187 (transport: disconnected) — will keep asking
can: controller went bus-off — restarting it
can: no unit answers — asking every 2 s until one does
can: controller is back on the bus
```

The panel drew four dashes, which is correct behaviour for four channels nobody
answered — `wobble()` is gone and nothing invents a number.

Two things to read out of that log:

- **`swept 32`** is the whole depth of esp-hal's software receive queue, and it
  was already full before the first request went out. Entries only ever land
  there from the receive interrupt, so the car's bus was alive and the board was
  hearing it. **Reception worked.**
- **`bus-off`** means the transmit error counter reached 256. Every path to that
  number runs through this node transmitting and the attempt failing.

## 2. What was fixed in the firmware

Both are real, both are in `9ca3547`, both are verified on the board.

### 2.1 The controller had no acceptance filter

esp-hal's async driver queues every accepted frame in a 32-deep channel and
`try_send` **drops** what arrives when it is full. A powertrain bus is not quiet,
so between our request and its answer the queue fills with other people's frames
and the answer is what falls off the end. `swept 32` on the very first exchange
is that happening.

`dash.rs::response_filter()` now builds an ESP32 single standard filter out of
the plan's own answer ids — code = the bits every answer id agrees on, mask = the
rest dropped. For this car that is code `0x7E8`, mask `0x7FE`, which accepts
exactly `0x7E8` and `0x7E9` (checked by enumerating all 2048 standard ids). No
car-specific number reaches the source: it is derived from `PLAN.units`.

**Consequence for reading the old log: `7E0 did not answer (timeout)` is not
trustworthy evidence.** The engine may well have answered and the driver may well
have dropped it. Only a log taken after this fix says anything about `7E0`.

### 2.2 `cargo run` flashed the wrong partition table

The runner in `.cargo/config.toml` never passed `--partition-table
partitions.csv`, so espflash wrote its own default — `nvs`, `phy_init`, and a
`factory` filling the flash — and the board came up with no `config` partition at
all. Hence `no settings storage (NoPartition)` and `no panic storage` on every
boot, settings that never survived a reboot, and panics that were printed and
lost. `partitions.csv` had been sitting beside the runner unused since `0f2ed1f`.

Verified after the fix:

```
 2 factory      factory app    00 00 00010000 00300000
 3 config       Unknown data   01 06 00310000 00008000
[health] panic slot at 0x312000, 24576 bytes spare in the partition
```

## 3. The inference that was wrong

**Recorded so nobody repeats it.** The bench work after the car run was built on
"no bus-off, therefore the controller is not transmitting". That is false.

ISO 11898-1 has an explicit exception for the acknowledgement error: a
transmitter that is already *error-passive* and detects no dominant bit while
sending its passive error flag does **not** increment its transmit error counter.
So a node alone on a bus climbs TEC to 128, becomes error-passive, and stops
there. **It never reaches bus-off.** It retries forever, and every exchange
above it times out.

Which means the bench — one ESP and nothing else that answers — could not have
produced any other result, whatever the hardware did. Hours went into chasing a
fault that the test could not have distinguished from health. A two-node bench
needs the second node to be *acknowledging* (`vagcan dev sniff --active`) before
its silence means anything, and even then the second node has to be proven to
receive at all first.

The car log is different: there the bus was busy, so the retries and passive
error flags met real dominant bits, TEC kept climbing, and bus-off followed. Same
root cause — **nobody acknowledged our frames** — but only a loaded bus turns it
into bus-off.

## 4. What is proven good

### 4.1 The chip and the protocol stack — `src/bin/cantest.rs`

Loopback inside the C3 through the GPIO matrix, no transceiver involved:

```
[1/3 backend] heard itself: id 0x7E0 ... — OK
[2/3 iso-tp]  pdu came back intact: [22, F1, 90] — OK
[3/3 uds]     client parsed a positive response — OK
== all stages passed: the stack runs on this chip ==
```

TWAI controller, GPIO6 as a TWAI pad, ISO-TP segmentation, the UDS client and its
allowlist: all healthy on this silicon.

### 4.2 The transceiver and its wiring — `src/bin/rxprobe.rs`

Written for this hunt. It drives `D` and reads `R` as plain GPIO, with no CAN
controller in the path at all:

```
[1/2 idle] R was high 100% of 47618 samples, 0 change(s) in one second
[2/2 echo] D dominant  -> R low  (0/64 high)
[2/2 echo] D recessive -> R high (64/64 high)
== echo passes: pads, wires, transceiver and the pair all carry a level ==
```

A clean recessive idle, and three rounds of the whole path — pad, wire, `D`, the
chip, the pair, the receiver, `R`, wire, pad — with no error. Note what this does
**not** prove: the transceiver hears its own dominant bit whether or not
`CANH`/`CANL` reach anything, so a passing echo says nothing about the cable to
the car.

### 4.3 The module, by inspection and datasheet

Marked **VP230**, which per SLOS346O §6 is a genuine SN65HVD230 — a 3.3 V part,
so the "5 V transceiver starved at 3.3 V" theory is dead.

| measured | datasheet | verdict |
|---|---|---|
| `VCC` (pin 3) = 3.31 V | 3.0–3.6 V | in range |
| `RS` (pin 8) = 0.61 V | standby needs 0.75·V<sub>CC</sub> = 2.48 V; 10 k–100 k to GND = slope control | **slope control, driver enabled** — not standby |
| the on-board `103` resistor | 10 kΩ to GND → slew ≈ 15 V/µs, differential edges 80–160 ns | fine at 500 kbit/s (2000 ns bit) |
| the `121` resistor, desoldered | correct: the car terminates at both ends inside its own units | do not refit for car use |

Loop delay driver-input → receiver-output with R<sub>S</sub> = 10 kΩ is 105–185 ns
(SLOS346O §8.9), comfortably inside the 1600 ns sample point of the B500K timing
esp-hal uses (BRP 8, TSEG1 15, TSEG2 4, SJW 3 → 20 Tq of 100 ns, sample at 80%).

One margin worth knowing: TI only guarantees the receiver's `V_OH` ≥ **2.4 V**
(at −8 mA), while the ESP32-C3 wants `V_IH` ≥ 0.75·VDD = **2.475 V**. At the ~0 mA
a GPIO input actually draws it sits at the rail — `rxprobe` measured a clean 100%
high — but the specs do overlap on paper.

Pin numbering used throughout, from SLOS346O §7: `1 D`, `2 GND`, `3 V_CC`,
`4 R`, `5 V_ref` (= V<sub>CC</sub>/2), `6 CANL`, `7 CANH`, `8 R_S`.

### 4.4 Multimeter readings that turned out to be noise

`R` measured 0 V, then 2.25 V, on a node `rxprobe` then found sitting at a clean
100% high. A high-impedance CMOS node and a hand-held meter disagree; the probe
wins. **Do not re-open the hunt on the strength of a voltmeter reading on that
pin.**

## 5. What is still unexplained

On the car, our frames were not acknowledged. Every node that receives a valid
frame acknowledges it in hardware, unconditionally — so either the frames did not
reach the car's pair, or they arrived malformed.

Untested link, and the only one left: **`CANH`/`CANL` from the module to the OBD
plug and into the car.** Reception worked over that same pair, which proves it is
connected — but a differential receiver decodes on a marginal pair (one line
open, the other biased internally) far more readily than a driver can put a
dominant bit onto one that other nodes will see. So "we heard the car" does not
imply "the car can hear us".

Two further candidates, both cheap to exclude:

- **Two supplies fighting.** The board's own documentation says USB and external
  power are mutually exclusive. On the car it was fed from the MP1584EN *and*
  plugged into the laptop for the monitor. VBUS is Schottky-isolated on this
  board, but the `3.3` pad is the regulator's output node, so back-feeding it
  while USB powers the same rail is exactly the case the vendor warns about.
- **The shared ground through the buck.** The transceiver's ground reaches the
  ESP through the MP1584EN's ground plane rather than directly.

## 6. The next experiment — CANable in parallel on the car

The owner's plan, and it is the right one: put the CANable on the car's pair
alongside the board and watch both ends at once.

**Terminal 1 — the board.** Flashes (now with the right partition table) and
monitors:

```bash
cd crates/dash/vag-dash-fw && cargo run --release
```

**Terminal 2 — the CANable, listening.** Note there is **no `--active`** here: on
a car the car's own units acknowledge, and listen-only cannot disturb anything.

```bash
cargo run --release --bin vagcan -- dev sniff --diag-only --out /tmp/car.jsonl
```

Read the two together:

| CANable sees | board says | conclusion |
|---|---|---|
| no traffic at all | anything | the CANable is not on the bus — fix that before believing anything else |
| the car's traffic, **no `7E0`** | timeouts | **the board's frames never reach the pair.** ESP → module → cable. This is the owner's hypothesis and this is how it is confirmed |
| `7E0` **and** `7E8`/`7E9` | timeouts | the frames go out and the units answer, but the board loses the reply — firmware, and the filter did not do its job |
| `7E0` **and** `7E8`/`7E9` | values on the panel | done; `todo/dash/05` closes |
| `7E0`, no answers | timeouts | the bus carries our request and no unit replies — addressing, ignition state, or gateway routing, not the physical layer |

**Watch the CANable itself.** It dropped off USB twice during the bench session,
and at no point in that session was it observed to receive a single frame. Its
silence is not yet evidence of anything. `vagcan devices` must list it with a `*`
before the run, and the car's own broadcast traffic must appear in the capture
within a second or two of starting.

### The one measurement worth taking first

Ignition off, board unpowered, OBD plug in the car, ohmmeter across the module's
`CANH` and `CANL`:

- **≈60 Ω** — both lines reach the car's pair (its two 120 Ω terminators in
  parallel). The transceiver's own 20–50 kΩ per line does not disturb this.
- **≈120 Ω** — only one line arrives. That is the fault, and it explains
  receiving while being unable to transmit.
- **open** — neither arrives.

## 7. State of the tree

- `9ca3547 feat(dash): the panel reads the car, and says so when it cannot` — the
  real-CAN firmware, the acceptance filter, the partition-table fix.
- `src/bin/rxprobe.rs` — the GPIO-level probe from §4.2. Bench tool: it holds a DC
  level on the pair, so it must never be pointed at a car. Same rule as
  `cantest.rs`, same reason.
- **The board currently carries an image with TWAI RX on `GPIO3`**, from the
  experiment that ruled the pin out; the source is back on the documented
  `GPIO1`. Move the wire back to `GPIO1` and reflash before the car run, or the
  receive line is not connected. `GPIO1` was cleared: pad-to-ground reads open,
  and `GPIO3` behaved identically.
- Board pinout confirmed against the vendor datasheet — with the USB-C connector
  at the bottom the left row reads `0, 1, 2, 3, 4, 3.3, G, 5V` top to bottom,
  which is what `research/dash/frame/wiring.py` draws. `GPIO8` is the blue LED,
  `GPIO9` is BOOT.
- The LED not blinking is **not** a fault: `led_task` keeps it dark unless BLE is
  advertising or connected.

## 8. Tooling defect noticed on the way

`.claude/settings.json`'s format hook runs `rustfmt --edition 2024`, but
`vag-dash-fw` declares `edition = "2021"`. Every edit of a file in this crate
therefore reorders its imports the 2024 way, which is not what `cargo fmt`
produces for a 2021 crate — so the next `cargo fmt -- --check` fails on a file
nobody meant to reformat. Seen and undone by hand this session. Either the hook
should read the crate's edition or this crate should move to 2024.
