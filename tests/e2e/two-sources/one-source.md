# One source per run, with both set up

The ODIS project and the VCDS store each describe the engine their own way. A run reads one
of them: the ODIS project by default, the VCDS installation with `--source vcds`. Nothing of
the other fills a gap — the ODIS run refuses the channel only VCDS describes, and the VCDS
run, which has no fault text, shows the engine's code as its number.

```console
$ vagcan -v faults --unit 01
? success
note: adapter from VAGCAN_DEVICE=[PORT]
names    ODIS SIM00-odis (eng)
1 fault text for 1 variant from /nowhere/SIM00-odis (en)
a stored code is a record that something happened once — not a diagnosis, and not necessarily a fault now; `failing` is failing now
7E0  1 code listed, 1 confirmed
7E0  freeze frames described by ODIS's EV_SimEngine_001
7E0  000129  level 2
7E0  000129 (297)  failed now, confirmed · 123456 km, 3×, 0 s ago, 0 km ago
1 stored code on 1 of 1 unit · 1 failing now

7E0  SIM ENGINE  SIM906000A
  ✗ P000129  Made-up sensor A: signal too high
             failing · 3 times · 123456 km · last 2026-07-27 00:00
             priority           6
             aging counter      16
             Odis Engine Speed  1000 1/min

$ vagcan -v --source vcds faults --unit 01
? success
note: adapter from VAGCAN_DEVICE=[PORT]
names    none — the VCDS installation has no fault text
codes below are shown as numbers: the VCDS installation has no fault text
note: looked in [HOME]/rod
help: read one: `vagcan setup <VCDS installation>`
help: or name them from an ODIS project: `--source odis`
a stored code is a record that something happened once — not a diagnosis, and not necessarily a fault now; `failing` is failing now
7E0  1 code listed, 1 confirmed
7E0  freeze frames described by VCDS's EV_SimEngine
7E0  000129 (297)  failed now, confirmed · 123456 km, 3×, 0 s ago, 0 km ago
1 stored code on 1 of 1 unit · 1 failing now

7E0  SIM ENGINE  SIM906000A
  ✗ 000129
             failing · 3 times · 123456 km · last 2026-07-27 00:00
             priority             6
             aging counter        16
             Vcds engine speed    1000 /min
             Vcds supply voltage  12.3 V

$ vagcan units --identify
? success
...

$ vagcan dev dash build SIMVIN00000000002
? failed
error: 01:1003: unit 01 has no channel at identifier 1003, bit 0 in this run's label data, and none was measured on a car — find your car's ids with `vagcan watch`: press `,` and set "Key at the end of each row" to shown

note: this build reads the ODIS project SIM00 alone; the other source does not fill in for it
help: build from a VCDS installation instead: `vagcan --source vcds dev dash build SIMVIN00000000002`

$ vagcan --source vcds dev dash build SIMVIN00000000002
? success
vin        SIMVIN00000000002
plan       2714d10679e8dd09
pages      1
channels   2 on 1 unit
alarms     0
plan.json  [HOME]/dash/SIMVIN00000000002/plan.json
plan.rs    [HOME]/dash/SIMVIN00000000002/plan.rs

$ VAGCAN_SOURCE=vcds vagcan dev dash build SIMVIN00000000002
? success
vin        SIMVIN00000000002
plan       2714d10679e8dd09
pages      1
channels   2 on 1 unit
alarms     0
plan.json  [HOME]/dash/SIMVIN00000000002/plan.json
plan.rs    [HOME]/dash/SIMVIN00000000002/plan.rs

$ VAGCAN_SOURCE=both vagcan faults
? failed
error: VAGCAN_SOURCE=both: not a source

help: name one: `odis` or `vcds`

```
