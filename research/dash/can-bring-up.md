# dash / CAN bring-up on the car — hand-off

**State, 2026-09-13, the bench.** **The board is an adapter, proven on the bench.** The
`slcan` image speaks slcan on the USB console and `vagcan --device` drives it like the
CANable (§9) — the exclusive adapter mode, mode 2, of
`todo/dash/14-one-bus-three-clients.md`. §9.4: frames cross the pair whole in both
directions, a listen-only board acknowledges nothing, and 3,726 frames/s for 12 s
arrived with `F00` — twice, the second time with the host's reader stopped for 2 s.
Not proven: the controller-FIFO overrun path (`take`, §9.1), which no bench run
overflowed.

**State, 2026-09-13.** **The dash reads the car.** With the replaced transceiver,
`dash` on the reference car answered `7E0`/`7E1` and the panel (through `dashsim`
on the laptop) showed coolant 51 °C, boost 0.99 bar, oil 42.0 °C, gearbox 39 °C —
live, four channels, two units, both pages. `todo/dash/05`'s "done when" is met; the
physical OLED on the carrier is the next item. One polish note from the run: page 0
reports `value_shrunk: true` (a value did not fit at full size), page 1 nothing.

**State, 2026-09-12.** The transceiver is replaced and the bench passes:
`research/dash/bench.sh` saw **60,861 frames from the board in 15 s** on the
CANable — ≈4,060/s, the bus's ceiling for 8-byte frames at 500 kbit/s, so every
frame went through first time and was acknowledged. `PASS` is the unambiguous
verdict (§5.3): the board's dominant bits reach the pair, and the CANable's own
receive path is proven with them. What remains is the car, in the order at the
end of §5.3: `rxwatch` built with the `ack` feature (Normal mode), then `dash`.

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
(a continuous `7E0` transmitter, bench-only, built only with `--features bench`) to the ESP, then sniffs the pair
over the CANable and prints PASS/FAIL — the board's `7E0` in the capture is
the transmit path proven. `bench.sh 30` for a longer listen, `bench.sh 15 dash`
to flash `dash` instead. It finds the CANable by its fixed serial and the ESP
by exclusion; a wedged ESP port wants a BOOT-held replug first. It does not
reflash anything afterwards: a board left with `cantx` floods any bus from
power-on, so its last line tells you to flash `dash` or `slcan` back.

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

**On the car, after the fix, in this order:** `rxwatch` in `Normal` mode
(`cargo build --release --bin rxwatch --features ack`; without the feature it
stays listen-only and acknowledges nothing) — the heartbeat falling from 3106/s
to 2 Hz is the first dominant bit of ours the gateway has ever registered; then
`dash`, expecting `7E0 is 8V0906264H as planned` and `7E1 is 0CW300041G as
planned`, and numbers on the panel. The bench passed on 2026-09-12 (state
header), so this is the next thing to do.

## 6. The next experiment — CANable in parallel on the car

The owner's plan, and it is the right one: put the CANable on the car's pair
alongside the board and watch both ends at once.

**Terminal 1 — the board.** Flashes (now with the right partition table) and
monitors:

```bash
cd crates/dash/vag-dash-fw && cargo run --release --bin dash
```

(`--bin dash` is not optional: the crate has several binaries and no
`default-run`, so a bare `cargo run` refuses to guess.)

**Terminal 2 — the CANable, listening.** Note there is **no `--active`** here: on
a car the car's own units acknowledge, and listen-only cannot disturb anything.
`--device` names the CANable: with the board plugged in beside it, a bare
command would have two USB serial ports to choose from.

```bash
cargo run --release --bin vagcan -- dev sniff --device /dev/cu.usbmodem206E37A148451 --diag-only --out /tmp/car.jsonl
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
  `cantest.rs`, same reason. Both, and `cantx`, build only with `--features bench`.
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
  which is what `wiring.py` in the owner's CAD workspace (`~/CAD/projects/vagcan/`) draws. `GPIO8` is the blue LED,
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

## 9. The board as an adapter — `slcan`, 2026-09-13

`src/bin/slcan.rs` is the fourth image: a USB-Serial-JTAG ↔ TWAI bridge speaking
LAWICEL slcan, so `vag_uds_can::SlcanBackend` — the client every `vagcan` command
opens the CANable with — drives the board unchanged. Nothing is decided on the
board: it puts on the bus what the host asks, and the host's allowlist is what
bounds that. The image's own header is the protocol reference (which commands,
which replies, how the status byte is built); this section is the bench.

### 9.1 What runs on the board

Three embassy tasks and one ring:

- **`bridge`** owns the console's receive half and the controller. One `select`
  loop: a console byte goes to the line parser and, on `\r`, to the command
  handler; a bus frame goes into `OUT` as its `t…`/`T…` line. A transmit (`t`/`T`
  from the host) is awaited *together with* a receive loop, so the reply to a
  request is never lost to the request — and it is answered `z`/`Z` only when the
  controller's `tx_complete` bit says the frame completed on the bus, acknowledge
  included. esp-hal's transmit future resolves `Ok` when the buffer is *released*,
  which an abort does too, and its interrupt handler aborts a pending transmit on
  any error whose captured direction says transmit — a lost arbitration, the
  acknowledge error of a bench with no partner. So a clear `tx_complete` is
  retried, up to eight times inside 100 ms, then refused (`\x07`); the transmit
  error counter tells arbitration lost (unmoved; latched as `F` bit 6) from an
  error (moved; the live bits show it) — below error-passive only, because §3's
  exception makes an unmoved counter meaningless at 128 and above, so there the
  live bits are the whole report. `C` drops the controller, clears the ring and
  bumps an epoch the writer checks (below); `O` builds a new one from the
  stored bit rate and mode, and drains esp-hal's static 32-deep receive queue
  with non-blocking polls so nothing from before the `C` comes up as live. `O`
  or `L` on an open channel is refused, and so is the `O` after an `S` the
  board does not have (`S0`–`S3`, `S7`) — the host ignores acks, and a
  normal-mode node opened at the wrong bit rate answers every frame on the bus
  with an error flag. A bus-off rebuilds the controller on its own and latches
  `F` bit 7. A controller overrun is what esp-hal reports from `MISS_ST`
  (status bit 8): a per-packet marker on the placeholder the FIFO left at its
  head for a frame it had no room for, released in place with `RELEASE_BUF`
  as ESP-IDF does (`CLR_OVERRUN` clears the unrelated `OVERRUN_ST`, bit 1),
  latched as bit 3; only a marker that survives the release rebuilds the
  controller as after a bus-off. esp-hal's handler also *reads* the
  placeholder as a frame and queues it behind the report, so the `Ok` that
  follows the report is discarded — a defensive fix with a bench check the
  image's `take` names, not yet run.
- **`console_tx`** owns the console's transmit half and drains `OUT`, whole lines
  packed into each 64-byte USB packet — the USB-Serial-JTAG hands the host 64
  bytes per packet and esp-hal waits for each, so a packet per write is the
  natural unit, and never splitting a line across packets is what lets a `C`
  discard the one line the writer already holds (it compares the epoch around
  each write) without leaving the host half a line.
- **`feed`** feeds the RTC watchdog (4 s, every second) and does nothing else, as
  `dash`'s does. A bridge that stops yielding is a reboot, not a dead port.
- **`OUT`** is a ring of **2,048 lines** between the bridge and the writer: half a
  second of a saturated bus (≈4,000 eight-byte frames/s at 500 kbit/s), two
  thirds of a second at the gateway's 3,106/s heartbeat storm. It is sized for the
  host stalling — a tokio task descheduled behind a SQLite write, a USB service
  interval, a busy laptop: milliseconds to tens of milliseconds, covered ten times
  over. A stall past half a second is a host that stopped reading, and no ring
  wins that; what the ring cannot hold is dropped **and counted**, and `F` reports
  it as bit 3 (data overrun) with bit 0 (receive queue full) saying it was this
  ring, not the controller's FIFO. 74 KB of static RAM; the image's `.bss` is
  128 KB with 189 KB left for the stack.

No logger is installed in this image, so esp-hal's own `warn!`s evaporate at the
`log` facade — the only thing on the console is slcan traffic. (A panic still
prints, through `health.rs`; at that point the stream is dead anyway.)

### 9.2 Flash

```bash
cd crates/dash/vag-dash-fw && cargo build --release --bin slcan
espflash flash --chip esp32c3 --partition-table partitions.csv \
  --port /dev/cu.usbmodem1101 target/riscv32imc-unknown-none-elf/release/slcan
```

The port re-enumerates on reset; `bench.sh`'s rule finds it — the CANable has
the fixed serial `206E37A…`, the ESP is the other `usbmodem`. No `--monitor`:
the console is the protocol now, and a monitor would be a second reader on it.

`vagcan devices` must list the board with a `*`:

```
* /dev/cu.usbmodem1101
    vag-dash board — slcan firmware answering
```

The board enumerates under Espressif's `303a:1001` whatever image it carries —
and so does every other ESP32-C3 and -S3 — so the ids alone do not make it an
adapter. `vagcan` asks each such port slcan's `V` (and nothing else) before it
lists or picks it: a well-formed `V0101` makes it a recognised adapter; no
answer lists it unmarked as `not answering slcan (display firmware? flash the
slcan image)`, and a car command pointed at it fails at once with that reason
instead of timing out on the car. No other device is ever asked anything. The
host's bench probe asks more — close, `V`, `N`, `F`, `S6`, open listen-only,
close:

```bash
cargo run -p vag-uds-can --features slcan --example slcan_probe -- /dev/cu.usbmodem1101
```

### 9.3 The bench, both directions, no car

No tool here puts an arbitrary frame on the bus: `vagcan` sends only what the
UDS allowlist permits, and `slcan_probe` refuses every transmit command in its
custom commands — `t`/`T`/`r`/`R`, and the CANable 2 firmware's CAN FD
`d`/`D`/`b`/`B` — because it writes to the adapter past that allowlist. (A
terminal typed straight into the port is the exception nothing here can
prevent.) `cantx` cannot share the board with `slcan` either, so the two directions are proven by crossing the two
adapters' ordinary traffic. The bench pair is the standing one (§5.2): both
transceivers on one pair, terminated.

**Board transmits** — the CANable listens, the board is asked for a VIN. There
is no car to answer, so `info` times out; the proof is the CANable seeing the
board's `7E0` request on the pair.

```bash
# terminal 1 — CANable, acknowledging (--active), so the board's frame completes
vagcan dev sniff --device /dev/cu.usbmodem206E37A148451 --active --seconds 20 --out /tmp/esp-tx.jsonl
# terminal 2 — the board as the adapter
vagcan info --device /dev/cu.usbmodem1101
grep -c '"Standard":2016' /tmp/esp-tx.jsonl     # 0x7E0 = 2016: the board's requests
```

**Board receives** — the reverse: the board listens through `dev sniff`, the
CANable is asked for a VIN.

```bash
# terminal 1 — the board, acknowledging, so the CANable's frame completes
vagcan dev sniff --device /dev/cu.usbmodem1101 --active --seconds 20 --out /tmp/esp-rx.jsonl
# terminal 2
vagcan info --device /dev/cu.usbmodem206E37A148451
grep -c '"Standard":2016' /tmp/esp-rx.jsonl
```

Without `--active` on the listening side nobody acknowledges: the transmitter
tries until its controller refuses, a listen-only sniffer still counts the
attempts (that is how §5.2 saw `cantx`), and the sending side notices nothing —
`SlcanBackend` never reads an ack, and `info` times out either way. So a non-zero
count on a listen-only sniffer proves the transmit path; only `--active` proves
the whole frame, acknowledge included. For the board as the transmitter the
difference is visible afterwards in `F`: `00` after an `--active` run; after a
listen-only run the trail of the refusals — every transmit is eight attempts at
eight error-counter points each, so the live bits climb through warning to
passive (`24`) and stay there however long `info` keeps asking: §3's exception
stops the counter at 128 for a transmitter nobody acknowledges, so this bench
never shows bus-off (`80` takes a loaded bus, i.e. the car). Not `64`: from the
third request on the counter no longer moves, and an unmoved counter is read as
arbitration lost only below passive. On the CANable's side the count is up to
eight times the number of requests, one line per attempt.

**Listen-only is silence.** With the CANable transmitting (`info` on it) and the
board opened by plain `dev sniff` (no `--active`, i.e. `M1` before `O`), the
board must acknowledge nothing: on the CANable side the frames go unacknowledged
exactly as they would with no board on the pair. That is what `sniff` promises
next to VCDS, and this is the run that checks the board keeps it.

### 9.4 Results

**PASS, 2026-09-13**, both directions, listen-only silence, and a load run the
bench was not expected to give. Image `slcan` at `2152752`, flashed with the
§9.2 command; `vagcan` built from the same commit. Bench pair terminated, both
adapters on USB:

```
* /dev/cu.usbmodem1101
    vag-dash board (slcan over USB, when running the slcan firmware)
* /dev/cu.usbmodem206E37A148451
    CANable 2.0 (slcan)
```

`slcan_probe` on the board: 7/7 acknowledged, `V0101`, `N7054`, `F00`.

| run | listener | talker | frames seen | `7E0` | board `F` after |
|---|---|---|---|---|---|
| board transmits | CANable, `--active`, 25 s | `info` on the board | 12 | 7 (plus 5 to `7E1`) | `00` |
| board receives | board, `--active`, 25 s | `info` on the CANable | 12 | 7 (plus 5 to `7E1`) | `00` (read after the next run) |
| listen-only | board, plain `sniff`, 12 s | `info` on the CANable | 44,708 | 33,425 | `00` |
| listen-only, host stopped 2 s | board, plain `sniff`, 12 s | `info` on the CANable | 44,630 | 44,630 | `00` |

- **Both directions carry whole frames.** With an acknowledging listener each
  request appears exactly once, at the two-second spacing `info` asks at — no
  repeats, so every frame completed first time, acknowledge included. `info`
  itself reports "The car did not answer" after 28 s, as it must with no car.
- **Listen-only acknowledges nothing.** The same two requests, `7E0 22 F1 90` and
  `7E1 22 F1 8C`, came back 33,425 and 11,283 times: the CANable retransmitting
  an unacknowledged frame until the next one replaced it. Had the board
  acknowledged even once, each would have stopped at one. (The `7E1` storm had
  already started when the capture opened — the last request of the previous
  run went out after the board closed its channel, and the CANable was still
  retrying it.)
- **The load claim is met on this bench after all.** That retransmission storm
  is a transmitter that is not the board: 3,726 frames/s for 12 s, median gap
  268 µs — one 8-byte frame, the acknowledge-error flag and the intermission, so
  the bus's ceiling for this traffic. 12 s at 268 µs is 44,776 slots and the
  board delivered 44,708; the largest gap was 6.5 ms, the handover to `info`'s
  next request. `F` read afterwards is `00`: neither bit 3 with bit 0 (the ring)
  nor bit 3 alone (the controller's FIFO) latched. `C` does not clear the
  latched bits and the host never sends `F`, so that `00` covers the whole run.
  *(Note, 2026-09-13: true of this run only. `vagcan dev sniff` now sends `F` itself,
  at the start of a capture and at its end, and each read clears the latched bits — so
  an `F` read after a later `dev sniff` covers only the time since that run's last
  question, and the run's own drop report is the one to read.)*
  The frames are identical, so content cannot prove none were lost; the counts
  and the clean `F` together can. The car remains the test with varied traffic.
- **A stalled host loses nothing measurable either.** The same storm (this time
  `7E0 22 F1 90` alone), with `dev sniff` on the board stopped by `SIGSTOP` for
  2 s from the fourth second: the recording shows the 2.004 s hole, then the
  backlog, and 44,630 frames in 12 s against 44,708 unstalled — every line one
  id, nothing the bus did not carry. `F` after: `00`. About 7,500 lines were held
  through the stall, more than the ring's 2,048, so the rest waited in the host's
  serial driver; the ring's own overflow count (bit 3 with bit 0) was therefore
  not exercised. A longer stop would reach it.
- The CANable's firmware has no `F` (nor `L` or `N`), so its
  side reports nothing about the unacknowledged frames beyond the count above.

Not covered here, by construction: bus-off recovery (`F` bit 7) takes a loaded
bus to reach, and the esp-hal overrun path (the `take` fix in §9.1) needs the
FIFO to overflow, which 3,726 frames/s did not do even with the reader stopped —
the bridge keeps draining the FIFO into the ring whatever the host does. `slcan_probe`
hangs if run during a storm: it reads until 300 ms of quiet, and after its `L` there
is none.

### 9.5 `dash` with the planner and UDS over BLE, 2026-09-14

Image `dash` from branch `fw-bus` (planner shell, BLE always on, the NUS UDS server), real
plan (VIN …8917, units `7E0`/`7E1`), flashed with the §9.2 command. Bench pair as in §9.3,
no car, so no unit answers. CANable: `vagcan dev sniff --device /dev/cu.usbmodem206E37A148451
--active`. BLE host: `research/dash/host` `bleuds`, run from Terminal.app — a process
started by the Claude app has no Bluetooth usage description and macOS kills it (TCC),
and Terminal waited for the owner's "Allow" once.

| check | seen |
|---|---|
| a. visible without a button | `bleuds` found `vagcan-dash` after 251–765 ms of scanning, connected in ~0.9 s, six connections in a row, re-advertising after each. `dashcfg`: state pushed on connect, `get` answered (`brightness 128 page 0 of 2 \| 0:values[2, 3, 0, 1] \| 1:chart[1]`). |
| b. requests reach the pair | `7E0 7E8 22F190` → `7E0 22 F1 90` on the pair, Answer NoAnswer after 5.8 s. `710 77A 22F187` → `710 22 F1 87`, NoAnswer after 0.6 s; the board noted `the filter follows the exchange — moved to 77A/7FF in 121 µs`. |
| c. guard | `2EF19000` → Refused `service 0x2E not allowed` in 89 ms, `1002` → Refused `programming session` in 91 ms, nothing on the pair for either. `1003` → `7E0 22 F4 0D` on the pair, then Refused `the engine did not report road speed` after 932 ms. |
| d. subscription | `--subscribe 7E0 7E8 F40D 100 5`: 2 Readings (NoAnswer) in 5 s, and on the pair `22 F4 0D` batched into the panel's part-number read (`22 F1 87 F4 0D`) twice, 2.5 s apart. After the disconnect no `F4 0D` at all. **Not ~10 Hz**: see below. |
| e. the panel keeps polling | `22 F1 87` to `7E0` and `7E1` alternately, each unit every 2.5 s (the planner's 2 s backoff cap plus the 500 ms answer deadline), before, during and after the BLE runs; `FRAME` lines keep coming on USB. |

- **A silent unit is asked at its backoff rate, whoever asks.** The planner backs off a
  unit that does not answer (250 ms doubling to 2 s) and holds every candidate of that unit
  to it: the panel's `F187`, a Remote request, the Timing speed read, a subscription. On
  this bench every unit is silent, so the 100 ms subscription was read at 2.5 s, a request
  to `7E0` waited up to 2.5 s before it went out (b's 5.8 s), and the speed read waited for
  the next slot too. A unit that answers is not backed off; the 10 Hz check needs one.
- **ATT MTU**: 23 at connect, 251 agreed by macOS right after (`[host] agreed att MTU of
  251`), so notifications are 20 bytes for the first moments and 244 from then on.
- **Heap** (72 KB): 46,140 used after the BLE host is built, ~47,900 idle with the planner
  running, 48,852 at most across the BLE sessions. `Current usage` stays flat; `Total
  allocated` grows ~3 KB per 15 s with the part-number retries.
- **The pair went quiet once.** After ~50 minutes with nobody acknowledging (the CANable
  closed while Terminal waited for the Bluetooth prompt), two `--active` sniffs (10:50 and
  10:52) saw **no frame at all** — not the panel's reads, not a `bleuds` request — though
  the board answered the `bleuds` request NoAnswer after 7.5 s. A reset (espflash monitor)
  brought the frames back at once. No note was captured for that window. **Not
  reproduced in 5 minutes:** left unacknowledged from 10:55:46 and sniffed again at 11:01:25,
  the board put 17 frames on the pair in 20 s (`22 F1 87` to both units every 2.5 s), and the
  console, watched throughout, said nothing about bus-off or errors. Open: whether it takes
  the longer unacknowledged stretch, and what state the controller is in — the next run is
  an hour unacknowledged with the console captured from the start.

### 9.6 After the review fixes, 2026-09-14

Same bench as §9.5, `dash` from `fw-bus` at `e5fdb96` with the info log on (for the heap),
console captured throughout, CANable `--active` for 120 s.

| check | seen |
|---|---|
| a | found in 250–500 ms, seven connections, re-advertising after each |
| b | `7E0 22 F1 90` on the pair, NoAnswer after 6.2 s; `710 22 F1 87`, NoAnswer after 0.57 s; filter moved to `77A/7FF` in 129 µs |
| c | `2E…` Refused in 63 ms, nothing on the pair; `1003` → `7E0 22 F4 0D`, then Refused (no road speed) after 2.8 s |
| response-id sweep | `bleuds --sweep-response 7E0 7E8 50 F40D 100`: **49 refused** ("request id 7E0 already answers on 7E8 in this connection"), 4 readings for the one accepted. Heap `Current usage` 48,092 before, 48,560 after; `Max usage` 49,016 → 49,096. |
| `3E 80` | `7E0 3E 80` at 43.387 s, the panel's `7E0 22 F1 87` 152 ms later (the 150 ms suppressed wait), then every 2.5 s as before — no extra backoff, no burst of F187. Host: NoAnswer after 7.4 s (the silent unit's backoff before it went out). |

- The first sweep run counted 17 of 49: `bleuds` sent all 50 subscriptions before reading
  notifications, and btleplug's bounded broadcast channel dropped the rest. Fixed in the
  tool (`e5fdb96`); the board had sent them.

### 9.7 The pair went dead, 2026-09-14 12:14–12:31

First end-to-end run of `vagcan` over BLE (branch `ble-uds` at `09a82fd`, board on `dash`
with the §9.6 fixes, `benchecu --bench --unit 7E0 --unit 7E1 --unit 710` on the CANable,
tools started from Terminal.app with `open -a Terminal <script>.command` — `osascript`
to Terminal timed out waiting for an automation permission nobody was there to grant).

| check | seen |
|---|---|
| `vagcan info --device ble` | found and connected (`using vagcan-dash over BLE`), then no result within 90 s |
| `bleuds --subscribe 7E0 7E8 F40D 100 10` | 4 Readings, all NoAnswer, 3 s apart (the planner's backoff on a silent unit) |
| `vagcan watch --device ble --did 01:F40D --hz 10 --for 15` | connected, identified 7E0 from the project, CSV rows with no values; killed at 60 s because start-up over BLE took 52 s |
| `benchecu`, 170 s | **no request at all** |
| `vagcan dev sniff --active` on the CANable, 12 s | 0 frames |
| board reset (`espflash reset`), same run again | same: no request reached `benchecu` |
| `bench.sh 15 cantx` | **FAIL, 0 frames** — the reference transmit test that passed in §5.2 and §9 |
| `rxwatch` on the board while `vagcan info` transmits from the CANable, 30 s | 0 frames, 0 errors |

Both directions dead, on images that each passed on this pair earlier the same day, and a
reset does not bring it back: a physical fault of the pair (wire, connector, termination)
or of the CANable, not firmware. The "pair went quiet once" of §9.5 may have been the same
fault showing first. **Needs the owner at the bench.** The board was left on `dash`.

Not yet judged because of it: whether `watch` plain mode's CSV rows every ~20 ms with ~5 s
gaps (on a bus where nothing answers) is a host defect; re-run on a working pair.

**Timeline, rebuilt from commit and capture times:** the pair last worked at the §9.6 check
(commit `c6b1339`, 11:41: `F187` to both units on the pair). Nothing touched the hardware
between then and 12:19 — the 12:14 attempt was an `osascript` launch that timed out before
running anything — and at 12:19 `benchecu` heard no request from its first second. The
owner's own `bench.sh` runs at 14:56 and 16:04–16:06 failed too, after replugging and RST.

**`rxprobe`, 16:08 (no CAN controller involved):** idle `R was high 0% of 47618 samples, 0
change(s)` — STUCK LOW; echo `D recessive -> R low` in every round; `rise took 200001 µs
<-- never`. The transceiver's `R` never goes recessive, whatever `D` does: the pair is held
dominant, or the module's `R`/supply/`CRX` path is broken. Not firmware — it survives
reflashing, RST and replug, and `rxprobe` drives the pads by hand. Next, by hand: unplug
the CANable from the pair and run `rxprobe` again (`R` high at idle → the CANable side
holds the bus; still low → the board's module or its wiring), and measure CAN-H/CAN-L
at idle (recessive: both ≈2.5 V, difference ≈0), the module's 3.3 V, and H–L resistance
unpowered.

### 9.8 What runs with the pair still dead, 2026-09-14 15:49–16:01

`ble-uds` after the alarms merge, `dash` rebuilt with the real plan. The pair was re-tested
first and is still dead: `bench.sh 15 cantx` FAIL at 15:49 and 15:56, and an `--active`
sniff on the CANable saw no frame of the `dash` image in 10 s. So nothing below reached a
bus; it is the board's USB and BLE links on their own.

| check | seen |
|---|---|
| `vagcan devices` | `vag-dash board — dash image (reads through the board, panel keeps running), 0.1.0` — the Hello probe on real hardware; also 1 s after `espflash reset`; over BLE `--device ble:vagcan-dash -44 dBm` |
| `vagcan dev survey --device B`, `vagcan dev sniff --device B` | refused before opening, with the two refusal texts |
| `vagcan info --device B` (USB link) | ran, ended "The car did not answer" after 120 s |
| slcan by hand on `B` | `V` → `V0101\r`, `F` → `F00\r`, `C` → `\r` — **but** the first image sent the note "usb: adapter mode is over…" *before* the `\r`; fixed (note moved behind `serve`), re-flashed, `C` → `\r` first |
| `vagcan --slcan dev sniff --device B --seconds 4` | listen-only at 500 kbit/s, 0 frames, clean exit; the board answered Hello again right after |
| `dashcfg` (Terminal.app) | connected; `get` answered; the state panel cut `cells=[2, 3, 0, 1]` at `[2,` — `dashcfg` split the list on its spaces; fixed |
| `bleuds 7E0 7E8 22F190` | Answer NoAnswer after 8.8 s |
| `bleuds 7E0 7E8 2EF19000` | Refused in 91 ms: `service 0x2E not allowed: this link only reads` |
| `vagcan info --device ble` | connected, ended "The car did not answer" after 127 s |

### 9.9 The pair repaired; `vagcan` through the board, 2026-09-14 17:32–17:44

**The fault** (§9.7, §9.8): the SN65HVD230 module read 1.56 V on its 3.3 V pin and CAN-H/CAN-L
sat at 0 V — the transceiver was unpowered, which is exactly `rxprobe`'s "R stuck low".
Repaired by the owner; cause on the supply path to the module, not the firmware.

After the repair: `bench.sh 15 cantx` **PASS, 62,122 frames in 15 s**; `dash` (real plan) polls
`F187` to `7E0`/`7E1` on the pair. Board `B` = `/dev/cu.usbmodem1101`, CANable `C`. BLE runs
from Terminal.app (`open -a Terminal x.command`). `benchecu --bench --device C --unit 7E0
--unit 7E1 --unit 710` answered `F40D` (0 km/h); from 17:42 also `--part 7E0=… --part 7E1=…`
(F187, from the plan's own units).

| check | seen |
|---|---|
| `vagcan info --device ble` | identity reads on `7E0`/`7E1` at `benchecu` (F187 F189 F18C F190 F191 F197 0600); "the car did not answer" (nothing but F40D served) |
| `bleuds --subscribe 7E0 7E8 F40D 100 10` | **101 readings in 9,981 ms of board time, 10.0 Hz**; `benchecu` 10/s |
| `vagcan watch --device ble --did 01:F40D --hz 10 --for 20` | 199 rows, all with value 0, spacing 100 ms, max gap 102 ms; requests stop on the pair when it ends |
| `vagcan devices` | the board over USB as `dash image … 0.1.0`, over BLE as `ble:vagcan-dash` |
| `vagcan info --device B` (USB) | the whole identity sequence at `benchecu` within 1 s |
| `vagcan watch --device B --did 01:F40D --hz 10 --for 12` | 119 rows, all with value, spacing 100 ms, max gap 105 ms; `benchecu` 10/s |
| `kill -9` a `watch --device B` | its `F40D` requests stopped on the pair 1–2 s later, before any new host |
| `kill -STOP` a `watch --device B` 3 s, `kill -CONT` | nothing broke: macOS buffered the board's output; the subscription kept polling; a new `info` worked |
| `vagcan watch --device ble` (45 s) + `vagcan info --device B` + `vagcan --slcan dev sniff --device B` + `info --device B` again | the BLE `watch` wrote rows every 100 ms (max gap 102 ms) through all of it; the USB `info` ran beside it; adapter mode entered and left (`adapter: no frames dropped` — `F` read from the board); the second `info` was not refused. CANable `--active` saw 32 whole frames, 0 incomplete |
| `vagcan --slcan dev sniff --device B` while `vagcan info --device C` transmits | 25,945 frames of `7E0` in 10 s (the CANable retransmitting unacknowledged), whole |
| standalone `slcan` image | `vagcan devices` → `slcan image`; `\r`→`\r`, `V`→`V0101`, `F`→`F00`, `C`→`\r`; `--slcan dev sniff` 19,816 frames in 6 s, no drops; reflashed `dash` → `dash image` |
| `vagcan measure --device B` with part numbers | resolved `7E1 380B 3804 3809 380A 3816 F40D` and `7E0 2029 202A 206E F410 F40D`; ran at **10 Hz, not 50**: ~98 requests/s total, the board's 100/s ceiling, every host subscription `Class::Remote` because the link's Subscribe carries no class. After it ended the panel polled its plan (`202A 202F F405`, `028D`) at 2/s — the part check matched |
| the board's USB output, 60 s with nobody connected (in place of `dashsim`, which needs a terminal) | 291 `FRAME` lines (one per 200 ms), every one `FRAME 256 64 <hex>` and 1,319 characters long, 0 malformed, 0 other lines, 0 NUL bytes — no log line or link frame inside the panel stream |

**Open from this run:** a timing flag on the link's Subscribe so `measure`'s speed channel is
`Class::Timing` on the board (in progress, branch `timing-link`); then `measure` over USB and BLE
at 50 Hz. Not run: `dashsim` 2 min, unplugging USB in adapter mode, a 4095-byte USB flood,
the hour-long unacknowledged stall test.
