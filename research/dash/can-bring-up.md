# dash / CAN bring-up on the car — hand-off

**State, 2026-09-10.** The transceiver on the blue module is a counterfeit and is
the whole remaining fault (§5.3); everything else is proven. Below, the trail.

**State, 2026-09-04, evening.** The firmware polls the car for real
(`todo/dash/05`), it has been flashed and run on the reference car, and **no
control unit answered**. Two firmware defects were found and fixed on the way.
The third cause was found the same evening and it is not on the board: **the
home-made OBD plug**, whose CAN pins had come loose in solder-warped plastic and
only touched the socket while the plug was pressed in by hand (§5). It is being
rebuilt; the run after that is the checkpoint.

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

## 5. Found: the OBD plug

Written after the search, in the order the evidence came, because the order is
the lesson.

**The set-up nobody had written down.** The board and the CANable share **one
home-made OBD-II plug**; the pair leaves it as two wires and reaches both. Two
things follow. Any fault in that plug takes out both adapters at once. And the
board is on the bus whenever the plug is in the car, monitor or no monitor,
because the MP1584EN feeds it from pin 16 — "only the CANable on the bus" was
never true with this harness, and the board's frames, being valid, are simply
acknowledged by the CANable when it is active; they jam nothing.

**What was measured, at the wire ends, plug in the car.**

| | ignition | reading | says |
|---|---|---|---|
| red – black | off | 12.51 V | pin 16 and a ground are right |
| blue – green, plug held in by hand | off | 69 Ω | both lines reach the pair, *while pressed* |
| blue – green, plug released | off | **open** | they do not, in the position the plug actually runs in |
| blue / green – black | off | open | normal on a sleeping bus, proves nothing |

**What the day's runs said, once the plug is known.** The second car run
received nothing at all — no `swept`, no bus-off, only timeouts — where the
first run had swept 32 frames at boot. Between the two runs the wires had been
re-soldered onto the plug to match colours, and that is when the pins let go.
`vagcan info` over the CANable, which had worked on this car, failed on the same
plug in **both** polarities of the pair. ODIS Engineering, over `CAN` and `KL15`
on its own cable, read the car normally throughout. Car fine, board plausibly
fine, CANable plausibly fine, plug shared by exactly the two that failed.

**Two readings that misled, so the next reader is not misled by them.**

- **69 Ω is not 60 Ω.** Two 120 Ω terminators and a metre of probe lead read
  60–61 Ω. The extra eight or nine ohms were contact resistance in the loose
  pins, and were the first hint, read as meter error.
- **A static ohm reading with the plug held is not a contact test.** It shows
  the pins *can* touch, not that they *do* touch once the hand is off. Measure
  released, then wiggle the plug and the wires at the root; a pin pushed back
  into warped plastic drops out exactly when the plug is seated and let go.

**Candidates from the first draft of this section, closed:**

- *Two supplies fighting* — withdrawn. The buck drives the `3.3` pad, which is
  the on-board ME6211's output; with USB present the LDO holds that node at
  3.3 V under a 5 V input, which is its normal operating point, and the
  MP1584EN is non-synchronous and cannot sink, so it merely stops switching.
  The vendor's warning is about the `5V` pad on boards without the Schottky,
  which this harness leaves unconnected. Powering from both is fine. The only
  real effect is a ground loop if the laptop is on a car charger — noise, not
  a dead transmitter.
- *One line open reads ≈120 Ω* — wrong, and now removed from §6. An open line
  reads open; the ohmmeter across a pair with one side missing sees no path.
  What one loose line actually produced was the reading above: a value that
  depended on the hand.
- *`CANH`/`CANL` polarity* — not the cause here, but the check is cheap and was
  skipped for a while: ignition on, DC volts to ground, `CANH` above 2.5 V,
  `CANL` below. A swapped pair receives nothing and is heard by nobody, which
  is the same symptom as an open one; the voltmeter tells them apart.

**The fix** is a new plug — pins in heat-warped plastic go again on the next
insertion, glue or no glue — with the four wires re-soldered pin by pin, with
pauses, so the housing is not warped a second time. From the solder side, wide
edge up: top row 1–8 left to right, bottom row 9–16. Black on 4 or 5, blue on
6, green on 14, red on 16. Blue must share a row with black, green with red;
all four in one row is wrong before any meter is reached.

### 5.1 The rebuilt plug, 2026-09-09 — still not heard, and what that narrowed

New plug, pins checked, fuse replaced (pin 16 had found ground during the
rebuild; the cluster's warning cleared with the fuse). Then, in order:

- **`vagcan info` on the old `master` fails too**, so the software is out.
- **The CANable is out for a reason of its own: a trace on it was broken
  while soldering.** Its firmware still answers `V` (`16e7497-dirty`, stock
  canable2-fw, 500 kbit/s at 88 % sample point, auto-retransmit on) and `E`
  reads `4` = `ERR_CAN_TXFAIL`, a full TX FIFO — the same "nobody
  acknowledges" the board shows. Not evidence about the car until it is
  repaired.
- **`src/bin/rxwatch.rs`**, new: TWAI listen-only, no filter, frames per
  second and their ids. Transmits nothing, so it may be pointed at a car.
  Ignition on, engine running, first insertion: **0 frames in 60 s.**
  Plug out and back in: **3106 frames/s, every one `0x17F00010`**, zero
  errors. That id is the gateway's network-management heartbeat
  (`.archive/research/car/other-ecus.md`), 2 Hz when something acknowledges it.
  3106/s is the gateway retransmitting it back-to-back because nobody does —
  and 0 → 3106 across one re-seating says the contact at the socket comes
  and goes.
- **`rxprobe` echo on `GPIO1`** (the pin the wire is actually on; the probe
  had still said `GPIO3`) passes all three rounds, plug out.
- **`dash` in normal mode, contact present:** receives, times out, goes
  bus-off. Pressing the plug in every direction for a minute changes
  nothing. Bus-off with the heartbeat storming means our error flags met
  the gateway's dominant bits — so the pair reaches us, and **our dominant
  bits do not reach the gateway**, or it would have acknowledged and gone
  quiet.

**Where that leaves it — two candidates, and a bench test that separates
them.** "We hear the car, the car does not hear us" is either one line of
the pair on a resistive contact (a receiver decodes on half a pair, a
driver cannot put a dominant onto a 60 Ω-terminated one through it), or a
driver that cannot load a bus at all. Note that the first car run, on the
old plug, showed exactly the same shape: **this board has never once been
heard by the car.** And the echo test that clears the transceiver runs on
an *open* pair — the module's 120 Ω is desoldered — where a half-dead
output stage passes as easily as a healthy one.

The test: plug out, a 100–150 Ω load across `CANH`/`CANL` (the CANable's
own termination jumper, which is passive and needs no power, or any
resistor), then `rxprobe`. Echo under load passes → the driver is fine and
the fault is the socket's contacts 6/14, possibly spread by the old warped
plug; fails → the VP230 module is the fault, replace it. Then, with a meter
back in hand, `CANH`–`CANL` through the seated, released plug: 60 Ω,
steady, before anything else is believed.

**Operational notes from the day.** The C3's USB-JTAG re-enumerates on
every chip reset, so a console reader has to reopen the node; espflash's
DTR/RTS reset stopped working after a USB wedge and only a physical
replug brought it back. The board's own `dash` says its `can:` lines once,
so a run that missed the boot shows nothing.

### 5.2 The bench reproduces it without the car, 2026-09-09 evening

Two boards on one pair, a 120 Ω terminator, no car. This settles it, and it
is not the plug.

- **ODIS reads the whole car over `CAN` at `KL15`** — gateway `0019` OK,
  engine `0001` OK, all fifteen modules. So the car's diagnostic CAN and the
  gateway's receiver are healthy, and nothing we did damaged them. A
  known-good acknowledger exists on that socket.
- **`rxwatch` in `Normal` mode on the car** (it acknowledges every frame it
  receives, one dominant bit, no content) left the gateway's `0x17F00010`
  heartbeat storming at 3106/s. A node that heard our acknowledgement would
  fall silent to its 2 Hz rate; it did not. **A proven-good receiver does not
  hear the board's dominant bits.**
- **On the bench, the same asymmetry, both ways.** CANable transmitting `7E0`
  while `rxwatch` acknowledges: the board receives 3889 frames/s cleanly, but
  CANable's error register holds `ERR_CAN_TXFAIL` — its frames are never
  acknowledged, so it retransmits forever, which is what the board is
  receiving. Board transmitting (`dash`, and `cantest` self-test through the
  transceiver) while CANable sniffs: **0 frames reach CANable.** Lowering both
  ends to 125 kbit/s changes nothing.
- **`rxprobe` now has a timing stage.** Echo (stage 2) passes because the
  transceiver hears its own dominant bit, which needs nothing to leave the
  `CANH`/`CANL` pins. Stage 3 flips `D` and times `R`: the recessive edge
  takes ~2 µs — as long as a whole 500 kbit/s bit — while the dominant edge is
  under 1 µs. The pair the transceiver drives is slow to return to recessive.

**Conclusion. The board receives perfectly and is heard by nobody.** The fault
is on the board's transmit path *between the transceiver's `CANH`/`CANL` pins
and the bus*: the SN65HVD230's driver into a 60 Ω load, its `R_S` slope
resistor, or the `CANH`/`CANL` solder joints. Reception survives a marginal
pair (a differential receiver decodes on half of one); transmission into a
terminated bus does not. Everything upstream — the C3, `GPIO6`, the TWAI
controller, the ISO-TP/UDS stack — is proven, and the plug, once rebuilt,
reads 60 Ω.

**One command for the bench: `research/dash/bench.sh`.** Flashes `cantx`
(a continuous `7E0` transmitter, bench-only) to the ESP, then sniffs the pair
over the CANable and prints PASS/FAIL — the board's `7E0` in the capture is
the transmit path proven. `bench.sh 30` for a longer listen, `bench.sh 15 dash`
to flash `dash` instead. It finds the CANable by its fixed serial and the ESP
by exclusion; a wedged ESP port wants a BOOT-held replug first.

**The bench is now the whole test.** No car needed: `dash` (or any board
transmit) plus CANable's `E` register and a `sniff` is the fault, and the fix
is proven the moment CANable's `sniff` shows the board's `7E0` and its
`ERR_CAN_TXFAIL` stops. Next hardware step, the owner's: reflow `CANH`/`CANL`
and the `R_S` resistor, and if that does not do it, swap the VP230 module for
a fresh SN65HVD230.

### 5.3 The diagnosis, 2026-09-10 — a counterfeit transceiver

Everything after 5.2 either narrowed the fault or ruled out a way around it.

- **The joints are not it.** `rxprobe` echo passes under a 120 Ω load, and the
  owner rang `CANH`–`CANH` and `CANL`–`CANL` between the two modules on the
  bench. The pair between the transceivers is continuous and the driver holds a
  level into a load.
- **A slow edge is not it either.** With both ends at 125 kbit/s — an 8 µs bit,
  sampled at 6.4 µs — the board still acknowledges nothing the CANable can see.
  Any loop delay or slope that would fit inside that is not a fault a real
  transceiver has.
- **The marketplace reviews of this exact blue module say what our board
  says.** One buyer, on a scope: *the pair is driven for a moment, then the bus
  goes quiet* — which is `cantx`'s `TEC 128`, an error-passive node whose
  dominants nobody registers — and *"does not work with ESP-IDF TWAI"*. Another:
  both boards dead, replaced the SO-8 with a chip bought separately, worked.
  A third: would not start at 3.3 V, ran at 5 V. The chips are marked
  `VP230`, and a marked package is not a datasheet: **the module carries a
  counterfeit, and the fault is the chip.** §4.3's "genuine per the marking"
  is withdrawn.
- **Running it at 5 V was considered and is not worth it.** A 5 V `VCC` puts
  `R` at 5 V, and `GPIO1` on the C3 is not 5 V tolerant; `R` cannot simply be
  left off, because the controller reads its own transmitted bits back on RX
  and will not transmit without it. So 5 V needs a divider on `R` (2.2 kΩ over
  3.9 kΩ), for a part we are replacing anyway.
- **The 8-pin chip in the old scanner is not a transceiver.** `WA3393` is
  Way-On's dual comparator, an `LM393`; the two `2A` beside it are `MMBT3906`
  PNPs. That is how counterfeit ELM327s do CAN — a comparator and two
  transistors, no transceiver at all — and it explains why only `CAN-L` rang
  to it. Nothing there to salvage.

**Two fixes, either of which closes it.**

1. **A genuine `SN65HVD230D` from a distributor, soldered onto the blue board
   in place of the fake.** Same SO-8, same pinout, 3.3 V, no divider, no wiring
   change. This is what the reviewer who got a working board did. Another blue
   module from the marketplace is the same lottery.
2. **The CANable's own transceiver, which is what `todo/dash/05` designed in the
   first place** ("the CANable stays — as the transceiver, not as a bridge"). The
   MKS CANable V2.0 Pro carries an **ADM3050E**, isolated, logic side at the
   STM32's 3.3 V, so `GPIO6 → TXD` and `RXD → GPIO1` connect directly with a
   common ground. `TXD` is an input with one driver at a time: hold the STM32
   in reset (`NRST` to `GND`, its pins go high-impedance) for the bench, or do
   it `05`'s way, both sides open-drain into one pull-up. `SWD`/`SWC` are the
   debugger and play no part. While wired this way the CANable is not a
   sniffer — but the board is then on a transceiver whose transmit is proven,
   and can go straight to the car.

**One caveat on `bench.sh`, so its verdict is read right.** The CANable's
transmit is proven (the board receives its frames); its *receive* has never
been shown to work — its trace was broken until this evening and no
known-good transmitter has been on its pair since. So `FAIL` means "the board
was not heard *or* the CANable cannot hear"; **`PASS` is unambiguous** and
proves both at once. The car settles the board side regardless: the gateway
is a proven receiver, and it did not hear us.

**On the car, after the fix, in this order:** `rxwatch` in `Normal` mode — the
heartbeat falling from 3106/s to 2 Hz is the first dominant bit of ours the
gateway has ever registered; then `dash`, expecting `7E0 is 8V0906264H as
planned` and `7E1 is 0CW300041G as planned`, and numbers on the panel.

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

Ignition off, board unpowered, OBD plug in the car **and not held**, ohmmeter
across the module's `CANH` and `CANL`, then wiggle the plug:

- **≈60 Ω, steady** — both lines reach the car's pair (its two 120 Ω
  terminators in parallel). The transceiver's own 20–50 kΩ per line does not
  disturb this.
- **anything else** — a value that moves, sits well above 60, or is open — the
  plug (§5). Do not run anything until it reads 60 on its own.

## 7. State of the tree

- `9ca3547 feat(dash): the panel reads the car, and says so when it cannot` — the
  real-CAN firmware, the acceptance filter, the partition-table fix.
- `src/bin/rxprobe.rs` — the GPIO-level probe from §4.2. Bench tool: it holds a DC
  level on the pair, so it must never be pointed at a car. Same rule as
  `cantest.rs`, same reason.
- The wire is back on `GPIO1`, `rxprobe` reads `GPIO1`, and the echo passes
  there (§5.1).
- `src/bin/rxwatch.rs` — listen-only frame counter, safe on a car (§5.1).
- `src/bin/cantx.rs` — transmits one `7E0` request back-to-back and prints
  accepted/timed-out per second and `TEC`. Bench only.
- `research/dash/bench.sh` — flash a transmitter, sniff on the CANable, verdict.
  Read §5.3's caveat before trusting a `FAIL`.
- `rxprobe` has a third stage that times `D → R` on both edges.
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
nobody meant to reformat. Seen and undone by hand this session.

**Resolved 2026-09-04:** the crate moved to edition 2024 (`85df0aa`); `cargo fmt
--check` and the hook now agree.
