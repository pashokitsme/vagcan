# Units that misbehave

The engine's `F189` comes after two response-pending answers, `F18C` is refused with NRC 22,
and `F191` never comes. The gearbox is absent. The brakes answer one request, then go quiet.

```console
$ vagcan info
? success
note: adapter from VAGCAN_DEVICE=[PORT]
vin  SIMVIN00000000003

           engine 01   gearbox 02
part       SIM906000A  —
hardware   —           —
software   0001        —
component  SIM ENGINE  —
serial     —           —
coding     —           —

$ vagcan units --identify 01
? success
note: adapter from VAGCAN_DEVICE=[PORT]
unit  01

did   name                       value
F187  VW spare part number       SIM906000A
F189  VW software version        0001
F190  VIN                        SIMVIN00000000003
F197  System name / engine type  SIM ENGINE
F19E  ODX label file             EV_SimEngine
F1A2  ODX file version           001001

$ vagcan units --identify
? success
names    SIM00 label files
note: adapter from VAGCAN_DEVICE=[PORT]
unit  id   part        component    name
09    70E                           (no answer)
--    710  SIM907000C  SIM GATEWAY
--    713  SIM614000D
01    7E0  SIM906000A  SIM ENGINE
02    7E1                           (no answer)

5 asked · 3 answered · 2 on the gateway's list · 0 named

$ vagcan faults
? success
note: adapter from VAGCAN_DEVICE=[PORT]
names    ODIS SIM00-odis (eng)
7E1  fault list not read: transport: timeout
70E  fault list not read: transport: timeout
713  fault list not read: transport: timeout
0 stored · 0 failing now · 2 units read

```
