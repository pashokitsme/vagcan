# On a moving car

The identification block is a sweep, and an extended session can make a unit stop assisting
the driver: both are refused while the engine reports road speed.

```console
$ vagcan units --identify 01
? failed
note: adapter from VAGCAN_DEVICE=[PORT]
error: the car is moving at 30 km/h

note: 256 reads of one unit can stop it doing its job while the car moves
help: park the car and run `vagcan units --identify 01` again
help: to read it while driving anyway, add `--while-driving`

$ vagcan faults --extended
? failed
note: adapter from VAGCAN_DEVICE=[PORT]
error: `--extended` refused: the car is moving at 30 km/h

note: an extended diagnostic session can make a control unit stop assisting the driver
help: stop the car and run `vagcan faults --extended` again

```
