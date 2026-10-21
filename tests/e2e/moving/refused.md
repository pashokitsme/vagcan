# On a moving car

The identification block is a sweep, and an extended session can make a unit stop assisting
the driver: both are refused while the engine reports road speed.

```console
$ vagcan units --identify 01
? failed
Error: the car is moving at 30 km/h.
Read it parked: 256 reads of one unit can stop it doing its job while the car moves (--while-driving lifts this).

$ vagcan faults --extended
? failed
Error: --extended refused: the car is moving at 30 km/h. An extended diagnostic session can make a control unit stop assisting the driver. Stop the car first.

```
