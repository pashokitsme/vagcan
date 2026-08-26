# dash / 07 — sleeping in the car (deferred)

**Subsystem:** dash · **Crate:** `vag-dash-fw` · **Needs the car:** yes · **Deferred 2026-08-20**

Split out of `05` so it stops blocking a first look at the panel. It is not optional
before the device spends a night in the car — it is optional before the device exists.

Depends on `06` §7, the frame that says the ignition is on.

**Paired with [`08-power.md`](08-power.md), and neither is worth much alone.** This task
gets the processor down to microamps; that one decides whether the regulator in front of
it idles at microamps too. A sleep state machine behind a buck converter that draws 5 mA
has been cancelled by a component choice.

## Why it cannot be skipped forever

The device is plugged into OBD permanently. A poll loop that never stops holds the
gateway awake and flattens the battery, and the orders of magnitude are the whole
argument: deep sleep is tens of microamps — years off a car battery — while a running
poll loop is around a hundred milliamps *and* keeps every module on the bus from
sleeping, which is days.

## The shape, settled with the owner 2026-08-20

**Sleep.** The device is plugged into OBD permanently. It must stop polling when the car
sleeps, or it holds the gateway awake and flattens the battery. Get this right before the
first night parked. The shape, settled with the owner 2026-08-20:

1. **Deep sleep**, transceiver in standby, panel off, woken by a GPIO edge on the
   transceiver's RX pin — the first dominant bit on the bus. The CPU does not "listen"
   while parked; listening with the CPU awake is already a current draw.
2. **Wake, then listen only** for a second or two. Classify from received frames alone.
3. **Ignition present → enable transmit and start polling. Otherwise back to sleep** — the
   wake was somebody locking the car.

**The decision is made entirely from passive reception. Nothing is transmitted to reach
it.** The obvious version of this — "saw traffic, send a request after a second and see"
— is the one thing that defeats the purpose: a diagnostic request is exactly what holds
the gateway awake, so the device would be waking the car in order to find out whether the
car is awake.

The orders of magnitude are what make it worth the care: deep sleep is tens of
microamps — years off a car battery — while a running poll loop is around a hundred
milliamps *and* keeps every module on the bus from sleeping, which is days.

Two failure modes to design against:

- **Going to sleep mid-drive.** Sleep must require *sustained* silence, never a single
  unanswered request. One lost exchange on the road is ordinary. Note also that "no
  answer" is what the moving-car guard reads as *moving* — the same input, two opposite
  responses, so the two paths must not share a predicate.
- **Holding a session open while parked.** Do not send `0x3E` to keep a diagnostic
  session alive. Letting it lapse on the S3 timer (5 s, ISO 14229) is the correct
  behaviour; holding it is "keeping the car awake", politely.

What distinguishes "ignition on" from "somebody unlocked the car" is a cyclic frame we do
not yet know. It is found by sniffing, not by reasoning — see `06`.

## Waking on bus activity is not available with this hardware

Found 2026-08-20, and it invalidates step 1 of the shape above as written.

The wake was to be a GPIO edge from the transceiver's `RXD` on the first dominant bit.
That needs the transceiver powered — and the transceiver is the ADM3050E on the CANable,
whose bus side is fed by a Hi-Link **B0505S-1WR3**, an isolated 1 W module. Modules of that
class are known for an unhelpful no-load current, and on a permanently live OBD pin that
is the whole battery budget (`08`). Switched off while parked, the transceiver goes off
with it, and nothing is left listening.

## The decided design: cheap sensor up, rich signal down

Settled with the owner 2026-08-20.

**Wake on the rail: 13 V or above.** A divider from OBD pin 16 into an ADC. Two resistors,
powered from the same buck as everything else, available whether or not the CAN side has
any power at all — which is exactly the property the `RXD` wake turned out not to have.

Two details that decide how well it works, both properties of the classic ESP32:

- **The divider goes on ADC1** (`GPIO32…39`, and `D34` in the allocation in `05`). ADC2 is
  unusable while Wi-Fi is on — the radio claims it and reads block or return rubbish — and
  Wi-Fi and Bluetooth are both wanted (`09`). Not a preference; ADC2 is simply out.
- ~~**The ULP coprocessor can read ADC1 during deep sleep**~~ — **this does not exist on
  the C3, 2026-08-26.** Checked in `esp-metadata`'s device table: `esp32c3.toml` lists no
  `ulp_supported`, no ext0, no ext1, no touch wake — only `gpio_support_deepsleep_wakeup`
  and Wi-Fi/BT wake, and the last two are light-sleep only. The S3 has a ULP; this part
  does not.

  So the mechanism this section was built on is gone, and with it the "comparison running
  at hundreds of microamps". What remains for waking on the rail is one of two, and both
  are hardware questions rather than firmware ones:

  - **a periodic timer wake** that samples the divider and goes back down. Costs a full
    wake-up per sample, so the sample interval sets the average current;
  - **an external comparator** holding an RTC pin, which is the ULP's job done in two
    parts for a few cents. `GPIO0`–`GPIO5` are the only pins that can wake the chip.

  This is the single largest thing the board change cost us, and it was invisible until
  somebody read the device table.

### Pin allocation on the C3 (settled 2026-08-26)

`GPIO0`–`GPIO5` are the only RTC pins, so only they can wake the chip; `GPIO0`–`GPIO4`
are also the whole of ADC1. The two needs therefore compete for six pins, and they divide
cleanly:

| pin | assignment | why |
|---|---|---|
| `GPIO5` | **wake button** | the one RTC pin that is not ADC1. ADC2 does not read with the radio up, so its analog capability was already worthless — spending it on a digital job costs nothing |
| `GPIO4` | **rail divider from OBD pin 16** | must be ADC1, is not a strapping pin |
| `GPIO2` | avoid | strapping pin |
| `GPIO0`, `GPIO1`, `GPIO3` | free | one spare wake pin, two spare analog inputs |

With no ULP the divider does **not** consume a wake slot: nothing can read it while
asleep, so `GPIO4` is an ordinary ADC input that happens to sit on an RTC pad.

And the board's BOOT button on `GPIO9` **cannot** be a wake source — not an RTC pin — and
held low at reset it boots the USB loader instead of the firmware.

### Both wake sources verified on hardware, 2026-08-26

No wire and no soldering: `GPIO5` is held high by the chip's own pull-up and wakes on
`Low`, so a press is a short to ground and a pair of tweezers is a good enough button.

```
GPIO5 shorted to ground   → wake reason = pin level
nothing touching it       → wake reason = RTC timer
```

The second line is the half worth stating: across the whole run there was **not one
spurious wake**, which is what says the internal pull-up is enough and no external
resistor is owed. A floating RTC pin would have woken the device on stray charge.

`sleeptest` also flashes the reason on the board's LED before it prints anything — one
long flash for a cold boot, one short for the timer, three short for the button. That is
not decoration: see the next paragraph.

### Deep sleep makes the board invisible to its own console

The USB Serial/JTAG peripheral powers down with everything else, so the host un-enumerates
the device and takes a second or two to bring it back. Anything printed in that window is
written into a port nobody is holding open — so **every wake looked like a cold boot**,
because the only line you ever catch is the next one. A 1.2 s delay before printing was
not enough either; the reason is now printed once a second for the whole awake window.

Two more that only appear with a board attached:

- **`espflash` cannot catch a sleeping board.** It took six attempts to hit a five-second
  awake window. Flashing a device that sleeps wants a long awake window or the BOOT-hold
  replug.
- **Reading that port with `head` loses everything.** stdio buffers into a file and the
  buffer dies with the process, so the log looks empty while the firmware is talking
  normally. `dd bs=1` sees it. This cost an hour of believing the wrong thing.

A bench that watches sleep closely wants a USB-serial adapter on UART0 (`GPIO20`/`GPIO21`,
free — the console is on native USB): a separate chip stays enumerated across the sleep.

### Two more things the source says, both load-bearing

- **Nothing survives the sleep.** `RtcSleepConfig::deep()` powers down both RTC fast and
  slow memory, so `#[esp_hal::ram(persistent)]` does not persist across `sleep_deep`.
  Every wake is a cold start; anything that must outlive a sleep belongs in flash
  (`store.rs` already does this for settings).
- **A timer-only current figure is an upper bound, not the floor.** esp-hal runs its
  equivalent of IDF's `esp_sleep_isolate_digital_gpio` only from `RtcioWakeupSource::apply`.
  With timer-only wake the digital pads are left as they are, and esp-hal's own comment
  says the bottom current rises without that step. Measure with the real wake source
  before believing a number.

**Sleep on either of two conditions:**

- **the ignition going off**, read from the bus — and this is affordable precisely because
  we are awake when we ask. The transceiver is powered, the cyclic frame from `06` §7 is
  there to be seen, and its disappearance is the signal;
- **or fifteen minutes idle**, as the backstop for everything the first condition misses.

The split is the point. The always-available sensor is crude and only has to get the
device *up*; the sensor that costs power is rich and only has to be right about going
*down*, by which time its power is already on.

Two consequences worth having written down:

- **"No answer" plays no part in the sleep decision at all.** It was going to, and it is
  the same input the moving-car guard reads as *moving* — one signal with two opposite
  meanings, which is how a device ends up asleep on the motorway. It is now simply not
  consulted: sleep is a positive statement from the bus, or a timer.
- **No hysteresis needed on the voltage.** Waking is a rising edge past 13 V and sleeping
  never consults the rail, so cranking dips and alternator ripple cannot make it flap.

**What this costs, and why it was accepted.** 13 V means the engine is *running*. A rested
battery sits near 12.6 V and the ignition alone, with its loads, pulls it to about 12.2 —
so turning the key without starting leaves the panel dark. Put to the owner and accepted
outright, 2026-08-20: waking on the engine is fine.

It is a good trade. It buys a wake that works with the entire CAN side unpowered, needs no
always-on transceiver and costs two resistors — and the fifteen-minute timeout means the
panel stays up *after* the engine stops rather than dying with it, which covers most of
what key-on-engine-off would have been for anyway.

## Ignition is the trigger for staying up, not for waking

Decided 2026-08-20, and it survives the change above in the half where it still applies.
Once the device is awake, sitting with the ignition on and the engine off is an ordinary
thing to do and most channels still answer, so ignition — not a running engine — is what
keeps the panel up. `06` §7 finds which cyclic frame carries it. Waking is the rail, per
the section above.
