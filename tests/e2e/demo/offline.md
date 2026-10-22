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
✓  02    7E1  SIM300000B  SIM GEARBOX  Sim Gearbox UDS
note: 2 of 4 units have no name in the ODIS project SIM00

$ vagcan dev dash build SIMVIN00000000001
? success
plan 896cfd2bbee4a491 · SIMVIN00000000001
1 page · 1 channel on 1 unit · 0 alarms

WRITTEN
  ~/.vagcan/dash/SIMVIN00000000001/plan.json
  ~/.vagcan/dash/SIMVIN00000000001/plan.rs

```
