# The stopwatch, short of channels

The made-up project describes no gear or road-speed channel for `measure` to time a run by,
so it stops before it opens the port and says what would bring them.

```console
$ vagcan measure
? 1
note: adapter from VAGCAN_DEVICE=[PORT]
error: no gear channel in this car's catalogs

help: vagcan setup <ODIS project | VCDS installation> brings the channels; -v lists what was looked for

```
