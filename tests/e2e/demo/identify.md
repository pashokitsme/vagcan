# Who the car is

`info` names the car and its powertrain, `units` lists what the gateway says is fitted, and
`units --identify` has every unit say what it is. 70E is on the gateway's list and never
answers.

```console
$ vagcan info
? success
note: adapter from VAGCAN_DEVICE=[PORT]
vin  SIMVIN00000000001

           engine 01   gearbox 02
part       SIM906000A  SIM300000B
hardware   —           —
software   0001        —
component  SIM ENGINE  SIM GEARBOX
serial     —           —
coding     01 02       —

$ vagcan units
? success
names    SIM00 label files
note: adapter from VAGCAN_DEVICE=[PORT]
unit  id   part  name
09    70E
--    713

2 units

$ vagcan units --identify
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

$ vagcan units --identify 01
? success
note: adapter from VAGCAN_DEVICE=[PORT]
unit  01

did   name                       value
F187  VW spare part number       SIM906000A
F189  VW software version        0001
F190  VIN                        SIMVIN00000000001
F197  System name / engine type  SIM ENGINE
F19E  ODX label file             EV_SimEngine
F1A2  ODX file version           001001

```
