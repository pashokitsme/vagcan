# Unit names from a VCDS installation alone

With no ODIS project, the VCDS installation names the units: the engine by its part number.
The others it has no label file for go by their ids, and the run says so.

```console
$ vagcan -v units --identify
? success
unit names from the VCDS installation
note: adapter from VAGCAN_DEVICE=[PORT]
recorded the units in ~/.vagcan/cars/SIMVIN00000000002/units.json
note: 2 control units of this car have no channels yet: the project was set up from a VCDS installation before this tool wrote down where it is, and its own note of it (/nowhere/sim-vcds/Labels) is not a directory on this machine holding UDS_EV — re-run `vagcan setup <VCDS installation>`
710: no name in the VCDS installation
713: no name in the VCDS installation
7E1: no name in the VCDS installation
4 control units on SIMVIN00000000002 · 4 answered · 1 on the gateway's list · 1 named

   UNIT  ID   PART        NAME               COMPONENT
✓  --    710  SIM907000C                     SIM GATEWAY
✓  --    713  SIM614000D                     SIM ESC
✓  01    7E0  SIM906000A  Sim Engine (VCDS)  SIM ENGINE
✓  02    7E1  SIM300000B                     SIM GEARBOX

$ vagcan units
? success
note: adapter from VAGCAN_DEVICE=[PORT]
1 control unit on the gateway's list

UNIT  ID   PART        NAME  COMPONENT
--    713  SIM614000D        SIM ESC

```
