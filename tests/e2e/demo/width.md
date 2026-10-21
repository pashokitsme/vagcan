# At 80 and 100 columns

A table is laid out for the terminal it is printed on. `VAGCAN_COLUMNS` names the width, as a
terminal of that width would: these are the layouts at 80 and 100 columns.

```console
$ VAGCAN_COLUMNS=80 vagcan units --identify
? success
names    SIM00 label files
note: adapter from VAGCAN_DEVICE=[PORT]
unit  id   part        component    name
09    70E                           (no answer)
--    710  SIM907000C  SIM GATEWAY
--    713  SIM614000D  SIM ESC
01    7E0  SIM906000A  SIM ENGINE
02    7E1  SIM300000B  SIM GEARBOX

5 asked · 4 answered · 2 on the gateway's list · 0 named

$ VAGCAN_COLUMNS=100 vagcan units --identify
? success
names    SIM00 label files
note: adapter from VAGCAN_DEVICE=[PORT]
unit  id   part        component    name
09    70E                           (no answer)
--    710  SIM907000C  SIM GATEWAY
--    713  SIM614000D  SIM ESC
01    7E0  SIM906000A  SIM ENGINE
02    7E1  SIM300000B  SIM GEARBOX

5 asked · 4 answered · 2 on the gateway's list · 0 named

$ VAGCAN_COLUMNS=80 vagcan faults
? success
note: adapter from VAGCAN_DEVICE=[PORT]
names    ODIS SIM00-odis (eng)
70E  fault list not read: transport: timeout
unit  id   name        code    dtc
      status         km  count  last seen         src
01    7E0  SIM ENGINE  000129  P000129
      failing    123456      3  2026-07-27 00:00  odis
      Made-up sensor A: signal too high
--    713  SIM ESC     047120
      confirmed  123392      1  2026-07-27 00:00

7E0  000129  P000129
    priority          6
    aging counter     16
    Sim Engine Speed  1000 1/min
    Sim Terminal 30   12.3 V
713  047120
    priority       4
    aging counter  32
    record 70      AA BB (no layout for this variant, not decoded)

2 stored · 1 failing now · 4 units read

$ VAGCAN_COLUMNS=100 vagcan faults
? success
note: adapter from VAGCAN_DEVICE=[PORT]
names    ODIS SIM00-odis (eng)
70E  fault list not read: transport: timeout
unit  id   name        code    dtc      status         km  count  last seen            src
01    7E0  SIM ENGINE  000129  P000129  failing    123456      3  2026-07-27 00:00:03  odis
      Made-up sensor A: signal too high
--    713  SIM ESC     047120           confirmed  123392      1  2026-07-27 00:00:01

7E0  000129  P000129
    priority          6
    aging counter     16
    Sim Engine Speed  1000 1/min
    Sim Terminal 30   12.3 V
713  047120
    priority       4
    aging counter  32
    record 70      AA BB (no layout for this variant, not decoded)

2 stored · 1 failing now · 4 units read

$ VAGCAN_COLUMNS=80 vagcan setup --list
? success
project  SIM00
cache    [HOME]/data/SIM00/cache.sqlite

id  kind  state  lang  version  read               faults  channels  labels  measurements
 1  odis  gone   eng   -        2026-10-21 00:00Z       2         2       0             0
    /nowhere/SIM00-odis

$ VAGCAN_COLUMNS=100 vagcan setup --list
? success
project  SIM00
cache    [HOME]/data/SIM00/cache.sqlite

id  kind  state  lang  version  read               faults  channels  labels  measurements
 1  odis  gone   eng   -        2026-10-21 00:00Z       2         2       0             0
    /nowhere/SIM00-odis

```
