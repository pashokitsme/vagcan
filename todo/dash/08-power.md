# dash / 08 — power: 12 V from the socket, and the microamps that matter

**Subsystem:** dash · **Needs the car:** for measurement, yes · **Opened 2026-08-20**

Paired with [`07-sleep.md`](07-sleep.md). They are one problem seen from two sides: `07`
is what the firmware does to draw nothing, `08` is whether the hardware lets it.

## Where the power comes from

**OBD-II pin 16 is permanent battery positive**, pins 4 and 5 are chassis and signal
ground, pins 6 and 14 are CAN-H and CAN-L (SAE J1962). This is the connector standard, not
a property of this car.

Two consequences, and the second is the whole task:

1. There is no wiring to run. One cable to the socket carries both the bus and the supply.
2. **It is permanent.** Pin 16 is live with the car locked and asleep. Nothing switches it
   off for us, so whatever the device draws, it draws for as long as the car is parked.

## The quiescent current *is* the sleep design

`07` gets the ESP32 down to tens of microamps in deep sleep. That number is meaningless
if the regulator in front of it idles at milliamps — the device then draws milliamps, and
the whole sleep state machine has been cancelled by one component choice.

The arithmetic, so the target is a number rather than a feeling: 10 mA is 240 mA·h a day,
about 7 A·h a month. A car battery is 60–70 A·h and the whole vehicle's permitted
parasitic draw is usually specified in the tens of milliamps, so a single accessory at
10 mA is a large fraction of the budget. At 100 µA it is 72 mA·h a month, which is
nothing.

**Target: under about 1 mA for the whole device asleep.** That is a buck-converter
specification before it is a firmware one — a part with microamp-class quiescent current
and, ideally, an enable pin the firmware can pull.

The panel is the other half: an OLED must be **off**, not merely dark, when asleep.

## Automotive input, not a bench 12 V

The car's rail is a hostile supply and a bare module will not survive it. What the front
end has to take, per ISO 16750-2 / ISO 7637-2 rather than per guesswork:

- **Cranking dips** — the rail collapses at every start. The device should ride through
  rather than brown out and reboot; a display that reboots every time the engine starts
  is a display nobody trusts.
- **Load dump and transients** — tens of volts, well above the nominal 12–14.
- **Reverse polarity**, because one day something will be wired backwards.

So: TVS, reverse protection, and a buck rated well above the nominal rail — not a bare
step-down module.

## USB-C — yes on the bench, and be careful about the rest

For development USB-C is exactly right: the ESP32-S3 has native USB, so one cable gives
flashing, serial and 5 V. Keep it.

Two things it does not do:

- **USB-C does not carry CAN.** In the car the bus comes from OBD pins 6 and 14; USB is a
  bench convenience, not a second data path.
- **Two supplies on one rail need ORing.** With the car's 5 V and USB's 5 V both connected,
  one back-feeds the other — into the laptop's port or into the buck's output. An ideal-
  diode power mux, or at minimum Schottky ORing.

**And do not use a USB-C receptacle as a cheap connector for 12 V + CAN.** It is tempting —
24 rugged pins for pennies — and it is a trap with a date on it: sooner or later somebody
plugs a real charger, or a laptop, into a socket that has the car's CAN lines on it. If a
single-cable connector is wanted, use one that cannot be mistaken for something else.

## The panel's own supply

Check before ordering: many SSD1322 modules want **3.3 V logic and a separate 12–15 V
panel supply**, and only some carry the boost converter on board. A module without it
needs one, and that changes the power design rather than a footprint.

## Measure, do not assume

- The device's draw asleep and awake, on the bench, at 12 V.
- The rail during cranking, at the socket, on this car — the front end is designed against
  what was measured, not against a datasheet's worst case.

## Done when

The device runs from the socket alone, survives a start without rebooting, and measures
under 1 mA with the car asleep.
