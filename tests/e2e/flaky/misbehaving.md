# Units that misbehave

The engine's `F189` comes after two response-pending answers, `F18C` is refused with NRC 22,
and `F191` never comes. The gearbox is absent. The brakes answer one request, then go quiet.

```console
$ vagcan info
? success
note: adapter from VAGCAN_DEVICE=[PORT]
SIMVIN00000000003 · SIM00
3 control units · read [..]

ENGINE  01 · 7E0
  SIM ENGINE
  part SIM906000A · software 0001 · EV_SimEngine

GEARBOX  02 · 7E1
  no answer

$ vagcan units --identify 01
? success
note: adapter from VAGCAN_DEVICE=[PORT]
unit 01 · 7E0 · SIM ENGINE
6 of 256 identifiers of F100-F1FF answered

IDENTIFICATION
  F187  VW spare part number       SIM906000A
  F189  VW software version        0001
  F190  VIN                        SIMVIN00000000003
  F197  System name / engine type  SIM ENGINE
  F19E  ODX label file             EV_SimEngine
  F1A2  ODX file version           001001

$ vagcan units --identify
? success
note: the units are not named: no label data on this machine
  (use "vagcan setup" to read it)
note: adapter from VAGCAN_DEVICE=[PORT]
5 control units on SIMVIN00000000003 · 3 answered · 2 on the gateway's list

   UNIT  ID   PART        COMPONENT    NAME
✗  09    70E                           (no answer)
✓  --    710  SIM907000C  SIM GATEWAY
✓  --    713  SIM614000D
✓  01    7E0  SIM906000A  SIM ENGINE
✗  02    7E1                           (no answer)

$ vagcan faults
? success
note: adapter from VAGCAN_DEVICE=[PORT]
names    ODIS SIM00-odis (eng)
warning: 7E1 fault list not read: transport: timeout
warning: 70E fault list not read: transport: timeout
warning: 713 fault list not read: transport: timeout
no stored codes on the 2 units read

```
