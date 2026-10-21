# What each level adds

`-v` adds where the data came from and why; `-vv` adds the wire, every request and answer.

```console
$ vagcan -v info
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

$ vagcan -vv info
? success
note: adapter from VAGCAN_DEVICE=[PORT]
→  710  22 2A 26
←  77A  62 2A 26 00 40 08 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00  [..] ms
→  7E0  22 F1 90
←  7E8  62 F1 90 53 49 4D 56 49 4E 30 30 30 30 30 30 30 30 30 30 31  [..] ms
→  7E0  22 F1 87
←  7E8  62 F1 87 53 49 4D 39 30 36 30 30 30 41  [..] ms
→  7E0  22 F1 91
←  7E8  7F 22 31  [..] ms
→  7E0  22 F1 89
←  7E8  62 F1 89 30 30 30 31  [..] ms
→  7E0  22 F1 97
←  7E8  62 F1 97 53 49 4D 20 45 4E 47 49 4E 45  [..] ms
→  7E0  22 F1 8C
←  7E8  7F 22 31  [..] ms
→  7E0  22 06 00
←  7E8  62 06 00 01 02  [..] ms
→  7E0  22 F1 9E
←  7E8  62 F1 9E 45 56 5F 53 69 6D 45 6E 67 69 6E 65  [..] ms
→  7E1  22 F1 90
←  7E9  7F 22 31  [..] ms
→  7E1  22 F1 87
←  7E9  62 F1 87 53 49 4D 33 30 30 30 30 30 42  [..] ms
→  7E1  22 F1 91
←  7E9  7F 22 31  [..] ms
→  7E1  22 F1 89
←  7E9  7F 22 31  [..] ms
→  7E1  22 F1 97
←  7E9  62 F1 97 53 49 4D 20 47 45 41 52 42 4F 58  [..] ms
→  7E1  22 F1 8C
←  7E9  7F 22 31  [..] ms
→  7E1  22 06 00
←  7E9  7F 22 31  [..] ms
→  7E1  22 F1 9E
←  7E9  62 F1 9E 45 56 5F 53 69 6D 47 65 61 72  [..] ms
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

$ vagcan -v units --identify 01
? success
note: adapter from VAGCAN_DEVICE=[PORT]
reading F100-F1FF of unit 01
unit 01 · 7E0 · SIM ENGINE
6 of 256 identifiers of F100-F1FF answered

IDENTIFICATION
  F187  VW spare part number       SIM906000A
  F189  VW software version        0001
  F190  VIN                        SIMVIN00000000001
  F197  System name / engine type  SIM ENGINE
  F19E  ODX label file             EV_SimEngine
  F1A2  ODX file version           001001

```
