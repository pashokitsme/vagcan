# The OBD-II sensors

The engine answers the standard parameters at J1979's widths; the gearbox has none of them,
and refuses each one: it answered, so nothing about the wiring is in question.

```console
$ vagcan sensors
? success
note: adapter from VAGCAN_DEVICE=[PORT]
3 of 32 standard parameters answered by unit 01 · 3 converted

DID          VALUE  NAME
F405    40.00 °C    Coolant temperature
F40C  1000.00 /min  Engine speed
F40D     0.00 km/h  Vehicle speed

$ vagcan sensors --unit 02
? success
note: adapter from VAGCAN_DEVICE=[PORT]
unit 02 (7E1) refused all 32 standard parameters

$ vagcan -v sensors --unit 02
? success
note: adapter from VAGCAN_DEVICE=[PORT]
unit 02 (7E1) refused all 32 standard parameters
it answered each one with a negative response: the unit is there, and serves none of them

```

70E is on the gateway's list and never answers. The read stops after the first requests go
unanswered, rather than waiting out every parameter's timeout.

```console
$ vagcan sensors --unit 70E
? success
note: adapter from VAGCAN_DEVICE=[PORT]
unit 70E does not answer
  (use "vagcan units --identify" to see which units answer)

$ vagcan -v sensors --unit 70E
? success
note: adapter from VAGCAN_DEVICE=[PORT]
unit 70E does not answer
  (use "vagcan units --identify" to see which units answer)
its first 3 requests went unanswered, so the rest were not asked

```
