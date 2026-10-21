# The OBD-II sensors

The engine answers the standard parameters at J1979's widths; the gearbox has none of them.

```console
$ vagcan sensors
? success
note: adapter from VAGCAN_DEVICE=[PORT]
did          value  name
F405    40.00 °C    Coolant temperature
F40C  1000.00 /min  Engine speed
F40D     0.00 km/h  Vehicle speed

3 of 3 converted · unit 01

$ vagcan sensors --unit 02
? success
note: adapter from VAGCAN_DEVICE=[PORT]
no answer from the car
check: ignition on · OBD-II pin 6 CAN-H, pin 14 CAN-L, pin 5 GND · adapter termination off · `vagcan devices`

```
