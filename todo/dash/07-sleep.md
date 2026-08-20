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

## The trigger is ignition, not a running engine

Decided 2026-08-20. Sitting with the ignition on watching temperatures is an ordinary
thing to do, and with the engine off most channels still answer — a device that only woke
for a running engine would be dark exactly when somebody is checking something in the
driveway. `06` §7 finds which cyclic frame carries it.
