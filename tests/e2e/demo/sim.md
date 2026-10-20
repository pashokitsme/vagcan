# Writing a profile

`dev sim --from` writes the profile of a car from what this machine holds about it: a car's
`units.json`; a capture of the engine read twice, its road speed going from 0 to 30 km/h
between the reads, and asked for its faults; and a `watch --out` recording, whose columns are
found through the units the car's record names — which `units --identify` writes. The profile
is on stdout; what was read is on stderr.

```console
$ vagcan dev sim --from units.json
? success
        Read units.json: 3 units, as each said what it is
    Finished profile of 3 units
about = "Written by `vagcan dev sim --from` from units.json."

[[unit]]
request = "710"
part = "SIM907000C"
component = "SIM GATEWAY"
odx = "EV_SimGatew"
odx_version = "001004"
installed = ["713"]

[[unit]]
request = "713"
part = "SIM614000D"
component = "SIM ESC"
odx = "EV_SimBrake"
odx_version = "001003"

[[unit]]
request = "7E0"
part = "SIM906000A"
component = "SIM ENGINE"
odx = "EV_SimEngine"
odx_version = "001001"
vin = "SIMVIN00000000004"

$ vagcan dev sim --from capture.jsonl
? success
        Read capture.jsonl: a capture of 6 frames, 1 unit, 1 identifier (1 changing), 1 fault
    Finished profile of 1 unit
about = "Written by `vagcan dev sim --from` from capture.jsonl."

[[unit]]
request = "7E0"

[[unit.did.F40D.series]]
at_ms = 8
value = "00"

[[unit.did.F40D.series]]
at_ms = 508
value = "1E"

[[unit.fault]]
code = "00 01 29"
status = 9

$ vagcan units --identify
? success
...

$ vagcan dev sim --from recording.csv --vin SIMVIN00000000001
? success
        Read recording.csv: a recording of 1 s, 2 columns, 2 identifiers
    Finished profile of 1 unit
about = "Written by `vagcan dev sim --from` from recording.csv."

[[unit]]
request = "7E0"
vin = "SIMVIN00000000001"

[[unit.did.F405.series]]
at_ms = 0
value = "50"

[[unit.did.F405.series]]
at_ms = 500
value = "51"

[[unit.did.F40C.series]]
at_ms = 0
value = "0F A0"

[[unit.did.F40C.series]]
at_ms = 1000
value = "12 C0"

$ vagcan dev sim --from recording.csv
? failed
Error: recording.csv is a recording, and a recording does not say which car

help: name it with --vin <VIN>, or give the car's units.json first

```
