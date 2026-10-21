# When it cannot go on

No adapter at the path given; a unit that refuses every standard parameter; the sim asked to
open a port.

```console
$ vagcan info --device /dev/no-such-adapter
? failed
error: opening the adapter at /dev/no-such-adapter
caused by: io error: No such file or directory

help: run `vagcan devices` to list what is connected

$ vagcan sensors --unit 713
? success
note: adapter from VAGCAN_DEVICE=[PORT]
no answer from the car
check: ignition on · OBD-II pin 6 CAN-H, pin 14 CAN-L, pin 5 GND · adapter termination off · `vagcan devices`

$ vagcan dev sim /dev/null
? failed
error: /dev/null is a device, not a profile

note: `dev sim` opens no port, it makes its own pseudo-terminal
help: give it a profile: `vagcan dev sim <profile.toml>` (docs/virtual-car.md)

```
