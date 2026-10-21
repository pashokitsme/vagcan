# Adaptations on a moving car

Reading a unit's adaptations is a sweep: refused while the engine reports road speed, before
the unit is asked anything. With `--all` the refusal says to run `--all` again.

```console
$ vagcan adaptations 01
? failed
note: adapter from VAGCAN_DEVICE=[PORT]
error: the car is moving (30 km/h)

note: adaptations are read only on a parked car
help: stop the car and run `vagcan adaptations 01` again

```

```console
$ vagcan units --identify
? success
...

$ vagcan adaptations --all
? failed
note: adapter from VAGCAN_DEVICE=[PORT]
1 unit of this car is not recorded on this machine
  (use "vagcan units --identify" to record it)
0 of 1 unit read · 0 adaptations · stopped at unit 01 (7E0), 0 not read
error: the car is moving (30 km/h)

note: adaptations are read only on a parked car
help: stop the car and run `vagcan adaptations --all` again

```
