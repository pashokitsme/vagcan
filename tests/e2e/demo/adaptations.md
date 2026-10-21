# Adaptations

`adaptations <unit>` reads the adaptation identifiers the project describes for the unit's
variant, each with `0x22`, and prints a summary line and the unit's block. 0612 is described
and not on the car: it is refused, and shown as not answered.

```console
$ vagcan adaptations 01
note: adapter from VAGCAN_DEVICE=[PORT]
3 adaptations of unit 01 (7E0) · 2 of 3 identifiers answered · 1 refused

01 7E0  SIM ENGINE  SIM906000A
  0610  Sim_idle_speed_offset  50 rpm
  0611  Sim_service_interval   15000 km
  0612  Sim_not_on_the_car     (not answered)

```

`--all` reads every recorded unit with adaptations, one after another. `units --identify`
records the car's units first; its output is in `identify.md`.

```console
$ vagcan units --identify
? success
...

$ vagcan adaptations --all
note: adapter from VAGCAN_DEVICE=[PORT]
1 unit of this car is not recorded on this machine
  (use "vagcan units --identify" to record it)
2 of 2 units read · 4 adaptations · 3 of 4 identifiers answered · 1 refused

01 7E0  SIM ENGINE  SIM906000A
  0610  Sim_idle_speed_offset  50 rpm
  0611  Sim_service_interval   15000 km
  0612  Sim_not_on_the_car     (not answered)

02 7E1  SIM GEARBOX  SIM300000B
  0620  Sim_clutch_adaptation  40 %

```

The brake unit names no ODX file, so nothing describes its adaptations: refused, with
nothing asked of it beyond its identification.

```console
$ vagcan adaptations 713
? failed
note: adapter from VAGCAN_DEVICE=[PORT]
Error: nothing describes the adaptations of unit 713

```
