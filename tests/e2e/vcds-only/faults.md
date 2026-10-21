# Freeze frames, from a VCDS install alone

The machine's VCDS store holds an install's `RF.rod` rows for the engine's record 70 and nothing for
the brakes. The engine's frame decodes field by field; the brakes' record 70 stays bytes, and
says why.

```console
$ vagcan faults
? success
note: adapter from VAGCAN_DEVICE=[PORT]
names    none — `vagcan setup` names the codes
2 stored codes on 2 of 4 units · 1 failing now

7E0  SIM ENGINE  SIM906000A
  ✗ 000129
             failing · 3 times · 123456 km · last 2026-07-27 00:00
             priority            6
             aging counter       16
             Sim engine speed    1000 /min
             Sim supply voltage  12.3 V

713  SIM ESC  SIM614000D
  · 047120
             stored · 1 time · 123392 km · last 2026-07-27 00:00
             priority       4
             aging counter  32
             record 70      AA BB (no layout for this variant, not decoded)

$ vagcan -v faults --unit 713
? success
note: adapter from VAGCAN_DEVICE=[PORT]
names    none — `vagcan setup` names the codes
codes below are shown as numbers: no fault-name labels on this machine
note: looked in [HOME]/rod
help: run `vagcan setup <ODIS project or VCDS installation>` (offline)
help: with no path, `vagcan setup` asks which and can fetch VCDS; see `vagcan setup --help`
a stored code is a record that something happened once — not a diagnosis, and not necessarily a fault now; `failing` is failing now
713  1 code listed, 1 confirmed
713  047120 (291104)  confirmed · 123392 km, 1×, 2 s ago, 64 km ago
1 stored code on 1 of 1 unit

713  SIM ESC  SIM614000D
  · 047120
             stored · 1 time · 123392 km · last 2026-07-27 00:00
             priority       4
             aging counter  32
             record 70      AA BB (no layout for this variant, not decoded)

```
