# Unit names and numbers from the run's one source

The ODIS project names the engine and the brakes, by what each unit's `F19E` says it is; the
VCDS installation names the engine and the gearbox, by part number. A run names the units from
its own source alone: the ODIS run leaves the gearbox unnamed, though VCDS names it, and the
VCDS run leaves the brakes unnamed, though ODIS names them. Each goes by its id, and the run
says so.

```console
$ vagcan -v units --identify
? success
unit names from the ODIS project SIM00
note: adapter from VAGCAN_DEVICE=[PORT]
recorded the units in ~/.vagcan/cars/SIMVIN00000000002/units.json
710: no name in the ODIS project SIM00
7E1: no name in the ODIS project SIM00
4 control units on SIMVIN00000000002 · 4 answered · 1 on the gateway's list · 2 named

   UNIT  ID   PART        COMPONENT    NAME
✓  --    710  SIM907000C  SIM GATEWAY
✓  03    713  SIM614000D  SIM ESC      Odis Brakes UDS
✓  01    7E0  SIM906000A  SIM ENGINE   Odis Engine UDS
✓  02    7E1  SIM300000B  SIM GEARBOX
note: 2 of 4 units have no name in the ODIS project SIM00

$ vagcan --source vcds -v units --identify
? success
unit names from the VCDS installation
note: adapter from VAGCAN_DEVICE=[PORT]
recorded the units in ~/.vagcan/cars/SIMVIN00000000002/units.json
710: no name in the VCDS installation
713: no name in the VCDS installation
4 control units on SIMVIN00000000002 · 4 answered · 1 on the gateway's list · 2 named

   UNIT  ID   PART        COMPONENT    NAME
✓  --    710  SIM907000C  SIM GATEWAY
✓  --    713  SIM614000D  SIM ESC
✓  01    7E0  SIM906000A  SIM ENGINE   Vcds Engine
✓  02    7E1  SIM300000B  SIM GEARBOX  Vcds Gearbox
note: 2 of 4 units have no name in the VCDS installation

```

A number is the run's source's too: ODIS numbers the brakes `03`, VCDS numbers no unit `03`.

```console
$ vagcan faults --unit 03
? success
note: adapter from VAGCAN_DEVICE=[PORT]
note: code names from ODIS SIM00-odis (eng)
1 stored code on 1 of 1 unit

713  SIM ESC  SIM614000D
  · 047120
             stored · 1 time · 123392 km · last 2026-07-27 00:00
             priority       4
             aging counter  32
             record 70      AA BB (no layout for this variant, not decoded)

$ vagcan --source vcds faults --unit 03
? failed
error: --unit: the VCDS installation numbers no control unit 03

help: give its request id instead: `vagcan units` lists them
help: or number the units as ODIS does: `--source odis`

$ vagcan units --identify 03
? success
note: adapter from VAGCAN_DEVICE=[PORT]
unit 03 · 713 · SIM ESC
4 of 256 identifiers of F100-F1FF answered

IDENTIFICATION
  F187  VW spare part number       SIM614000D
  F197  System name / engine type  SIM ESC
  F19E  ODX label file             EV_SimBrake
  F1A2  ODX file version           001001

$ vagcan --source vcds units --identify 03
? failed
error: --identify: the VCDS installation numbers no control unit 03

help: give its request id instead: `vagcan units` lists them
help: or number the units as ODIS does: `--source odis`

```

The plain listing names the units from what they said to `--identify`, in the run's source.

```console
$ vagcan units
? success
note: adapter from VAGCAN_DEVICE=[PORT]
1 control unit on the gateway's list

UNIT  ID   PART        NAME
03    713  SIM614000D  Odis Brakes UDS

$ vagcan --source vcds units
? success
note: adapter from VAGCAN_DEVICE=[PORT]
1 control unit on the gateway's list

UNIT  ID   PART        NAME
--    713  SIM614000D  SIM ESC
note: 1 of 1 unit has no name in the VCDS installation

```
