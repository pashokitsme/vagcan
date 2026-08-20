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
