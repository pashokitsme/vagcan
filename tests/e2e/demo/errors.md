# When it cannot go on

No adapter at the path given; a unit that refuses every standard parameter; the sim asked to
open a port.

```console
$ vagcan info --device /dev/no-such-adapter
? failed
Error: opening the adapter at /dev/no-such-adapter — run `vagcan devices` to list what is connected

Caused by:
    io error: No such file or directory

$ vagcan sensors --unit 713
? success
note: adapter from VAGCAN_DEVICE=[PORT]
no answer from the car
check: ignition on · OBD-II pin 6 CAN-H, pin 14 CAN-L, pin 5 GND · adapter termination off · `vagcan devices`

$ vagcan dev sim /dev/null
? failed
Error: /dev/null is a device, not a profile: `dev sim` opens no port, it makes its own pseudo-terminal

help: vagcan dev sim <profile.toml>   (docs/virtual-car.md)

```
