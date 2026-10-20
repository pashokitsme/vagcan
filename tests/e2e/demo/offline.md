# Without the car

`setup --list` says what the project was read from. `dev dash build` works from the car's
record, which `units --identify` wrote a moment before, and its `dash.toml`.

```console
$ vagcan setup --list
? success
project  SIM00
cache    [HOME]/data/SIM00/cache.sqlite

id  kind  state  lang  version  read               faults  channels  labels  measurements  path
 1  odis  gone   eng   -        2026-10-21 00:00Z       2         2       0             0  /nowhere/SIM00-odis

$ vagcan units --identify
? success
...

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
