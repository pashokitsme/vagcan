# Without the car

`setup --list` says what the project was read from. `dev dash build` works from the car's
record, which `units --identify` wrote a moment before, and its `dash.toml`.

```console
$ vagcan setup --list
? success
1 source on this machine

   ID  KIND  PROJECT  LANG  READ        CHANNELS  PATH
✗  p1  odis  SIM00    eng   2026-10-21         2  /nowhere/SIM00-odis

$ vagcan units --identify
? success
...
5 asked · 4 answered · 2 on the gateway's list

$ vagcan dev dash build SIMVIN00000000001
? success
vin        SIMVIN00000000001
plan       896cfd2bbee4a491
pages      1
channels   1 on 1 unit
alarms     0
plan.json  [HOME]/dash/SIMVIN00000000001/plan.json
plan.rs    [HOME]/dash/SIMVIN00000000001/plan.rs

```
