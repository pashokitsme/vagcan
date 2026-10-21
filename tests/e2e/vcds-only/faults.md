# Freeze frames, from a VCDS install alone

The project holds a VCDS install's `RF.rod` rows for the engine's record 70 and nothing for
the brakes. The engine's frame decodes field by field; the brakes' record 70 stays bytes, and
says why.

```console
$ vagcan faults
? success
note: adapter from VAGCAN_DEVICE=[PORT]
names    none — `vagcan setup` names the codes
unit  id   name        code    dtc  status         km  count  last seen            src  text
01    7E0  SIM ENGINE  000129       failing    123456      3  2026-07-27 00:00:03
--    713  SIM ESC     047120       confirmed  123392      1  2026-07-27 00:00:01

7E0  000129
    priority            6
    aging counter       16
    Sim engine speed    1000 /min
    Sim supply voltage  12.3 V
713  047120
    priority       4
    aging counter  32
    record 70      AA BB (no layout for this variant, not decoded)

2 stored · 1 failing now · 4 units read

$ vagcan -v faults --unit 713
? success
note: adapter from VAGCAN_DEVICE=[PORT]
names    none — `vagcan setup` names the codes
Codes below are shown as numbers: no fault-name labels on this machine.
Looked in: [HOME]/rod
Fix: `vagcan setup <ODIS project folder | VCDS installation>` — offline; with no path it asks which and can fetch VCDS (`vagcan setup --help`)
a stored code is a record that something happened once — not a diagnosis, and not necessarily a fault now; `failing` is failing now
713  1 code listed, 1 confirmed
713  047120  confirmed · 123392 km, 1×, 2 s ago, 64 km ago
unit  id   name     code    dtc  status         km  count  last seen            src  text
--    713  SIM ESC  047120       confirmed  123392      1  2026-07-27 00:00:01

713  047120
    priority       4
    aging counter  32
    record 70      AA BB (no layout for this variant, not decoded)

1 stored · 0 failing now · 1 unit read

```
