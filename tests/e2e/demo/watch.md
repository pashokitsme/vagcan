# The live view, plain

With `--for` the view is CSV on stdout, one row per poll. How many rows a second gives and
when each was read vary from run to run, so the rows and the counts are elided; the header
and the summary's shape are what is held.

```console
$ vagcan watch --for 1 --did 01:F405,F40C
? success
note: adapter from VAGCAN_DEVICE=[PORT]
units    4: 710 713 7E0 7E1
nothing to show for 710 713 7E1
t_s,Coolant temperature_t_s,Coolant temperature,Engine speed_t_s,Engine speed
...
[..] rows · [..] readings · [..] s · [..]/s

```
