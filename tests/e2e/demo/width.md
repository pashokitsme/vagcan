# At 80 and 100 columns

A table is laid out for the terminal it is printed on. `VAGCAN_COLUMNS` names the width, as a
terminal of that width would: these are the layouts at 80 and 100 columns.

```console
$ VAGCAN_COLUMNS=80 vagcan units --identify
? success
note: adapter from VAGCAN_DEVICE=[PORT]
5 control units on SIMVIN00000000001 · 4 answered · 2 on the gateway's list · 2 named

   UNIT  ID   PART        NAME         COMPONENT
✗  --    70E                           (no answer)
✓  --    710  SIM907000C               SIM GATEWAY
✓  --    713  SIM614000D               SIM ESC
✓  01    7E0  SIM906000A  Sim Engine   SIM ENGINE
✓  02    7E1  SIM300000B  Sim Gearbox  SIM GEARBOX

$ VAGCAN_COLUMNS=100 vagcan units --identify
? success
note: adapter from VAGCAN_DEVICE=[PORT]
5 control units on SIMVIN00000000001 · 4 answered · 2 on the gateway's list · 2 named

   UNIT  ID   PART        NAME         COMPONENT
✗  --    70E                           (no answer)
✓  --    710  SIM907000C               SIM GATEWAY
✓  --    713  SIM614000D               SIM ESC
✓  01    7E0  SIM906000A  Sim Engine   SIM ENGINE
✓  02    7E1  SIM300000B  Sim Gearbox  SIM GEARBOX

$ VAGCAN_COLUMNS=80 vagcan units
? success
note: adapter from VAGCAN_DEVICE=[PORT]
2 control units on the gateway's list

UNIT  ID   PART  NAME  COMPONENT
--    70E
--    713

$ VAGCAN_COLUMNS=100 vagcan units
? success
note: adapter from VAGCAN_DEVICE=[PORT]
2 control units on the gateway's list

UNIT  ID   PART  NAME  COMPONENT
--    70E
--    713

$ VAGCAN_COLUMNS=80 vagcan sensors
? success
note: adapter from VAGCAN_DEVICE=[PORT]
3 of 32 standard parameters answered by unit 01 · 3 converted

DID          VALUE  NAME
F405    40.00 °C    Coolant temperature
F40C  1000.00 /min  Engine speed
F40D     0.00 km/h  Vehicle speed

$ VAGCAN_COLUMNS=100 vagcan sensors
? success
note: adapter from VAGCAN_DEVICE=[PORT]
3 of 32 standard parameters answered by unit 01 · 3 converted

DID          VALUE  NAME
F405    40.00 °C    Coolant temperature
F40C  1000.00 /min  Engine speed
F40D     0.00 km/h  Vehicle speed

$ VAGCAN_COLUMNS=80 vagcan units --identify 01
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

$ VAGCAN_COLUMNS=100 vagcan units --identify 01
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

$ VAGCAN_COLUMNS=80 vagcan faults
? success
note: adapter from VAGCAN_DEVICE=[PORT]
note: code names from ODIS SIM00-odis (eng)
warning: 70E fault list not read: transport: timeout
2 stored codes on 2 of 4 units · 1 failing now

7E0  SIM ENGINE  SIM906000A
  ✗ P000129  Made-up sensor A: signal too high
             failing · 3 times · 123456 km · last 2026-07-27 00:00
             priority          6
             aging counter     16
             Sim Engine Speed  1000 1/min
             Sim Terminal 30   12.3 V

713  SIM ESC  SIM614000D
  · 047120
             stored · 1 time · 123392 km · last 2026-07-27 00:00
             priority       4
             aging counter  32
             record 70      AA BB (no layout for this variant, not decoded)

$ VAGCAN_COLUMNS=100 vagcan faults
? success
note: adapter from VAGCAN_DEVICE=[PORT]
note: code names from ODIS SIM00-odis (eng)
warning: 70E fault list not read: transport: timeout
2 stored codes on 2 of 4 units · 1 failing now

7E0  SIM ENGINE  SIM906000A
  ✗ P000129  Made-up sensor A: signal too high
             failing · 3 times · 123456 km · last 2026-07-27 00:00
             priority          6
             aging counter     16
             Sim Engine Speed  1000 1/min
             Sim Terminal 30   12.3 V

713  SIM ESC  SIM614000D
  · 047120
             stored · 1 time · 123392 km · last 2026-07-27 00:00
             priority       4
             aging counter  32
             record 70      AA BB (no layout for this variant, not decoded)

$ VAGCAN_COLUMNS=80 vagcan setup --list
? success
1 source on this machine

   ID  KIND  PROJECT  LANG  READ        CHANNELS  PATH
✗  p1  odis  SIM00    eng   2026-10-21         2  /nowhere/SIM00-odis

$ VAGCAN_COLUMNS=100 vagcan setup --list
? success
1 source on this machine

   ID  KIND  PROJECT  LANG  READ        CHANNELS  PATH
✗  p1  odis  SIM00    eng   2026-10-21         2  /nowhere/SIM00-odis

```
