# Who the car is

`info` names the car and its powertrain, `units` lists what the gateway says is fitted, and
`units --identify` has every unit say what it is. 70E is on the gateway's list and never
answers.

```console
$ vagcan info
? success
note: adapter from VAGCAN_DEVICE=[PORT]
SIMVIN00000000001 · SIM00
5 control units · read [..]

ENGINE  01 · 7E0
  SIM ENGINE
  part SIM906000A · software 0001 · EV_SimEngine
  coding 01 02

GEARBOX  02 · 7E1
  SIM GEARBOX
  part SIM300000B · EV_SimGear
  (use "vagcan units" for the other units, "vagcan faults" for their fault codes)

$ vagcan units
? success
note: adapter from VAGCAN_DEVICE=[PORT]
2 control units on the gateway's list

UNIT  ID   PART  NAME
09    70E
--    713
note: 2 of 2 units have no name in the ODIS project SIM00
  (use "vagcan units --identify" to name each unit by what it says it is)

$ vagcan units --identify
? success
note: adapter from VAGCAN_DEVICE=[PORT]
5 control units on SIMVIN00000000001 · 4 answered · 2 on the gateway's list · 2 named

   UNIT  ID   PART        COMPONENT    NAME
✗  09    70E                           (no answer)
✓  --    710  SIM907000C  SIM GATEWAY
✓  --    713  SIM614000D  SIM ESC
✓  01    7E0  SIM906000A  SIM ENGINE   Sim Engine UDS
✓  02    7E1  SIM300000B  SIM GEARBOX  Sim Gearbox UDS
note: 2 of 4 units have no name in the ODIS project SIM00

$ vagcan units --identify 01
? success
note: adapter from VAGCAN_DEVICE=[PORT]
unit 01 · 7E0 · SIM ENGINE
6 of 256 identifiers of F100-F1FF answered

IDENTIFICATION
  F187  VW spare part number       SIM906000A
  F189  VW software version        0001
  F190  VIN                        SIMVIN00000000001
  F197  System name / engine type  SIM ENGINE
  F19E  ODX label file             EV_SimEngine
  F1A2  ODX file version           001001

$ vagcan units --identify 70E
? success
note: adapter from VAGCAN_DEVICE=[PORT]
unit 09 (70E) does not answer
  (use "vagcan units --identify" to see which units answer)

```
