# Stored faults

Kept short on purpose: the listing is being redesigned, and these get blessed again with it.
The engine holds one confirmed code with its extended record, the brakes one pending code,
and 70E never answers. Both units answer their own record 70 too. The project is ODIS alone:
its multiplexer describes the engine's, decoded; the brakes' has no layout and stays bytes, and
says why (`vcds-only/faults.md` is the same from a VCDS install alone).

```console
$ vagcan faults
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

$ vagcan -v faults --unit 01
? success
note: adapter from VAGCAN_DEVICE=[PORT]
note: code names from ODIS SIM00-odis (eng)
2 fault texts for 2 variants from /nowhere/SIM00-odis (en)
a stored code is a record that something happened once — not a diagnosis, and not necessarily a fault now; `failing` is failing now
7E0  1 code listed, 1 confirmed
7E0  freeze frames described by ODIS's EV_SimEngine_001
7E0  000129  level 2
7E0  000129 (297)  failed now, confirmed · 123456 km, 3×, 0 s ago, 0 km ago
1 stored code on 1 of 1 unit · 1 failing now

7E0  SIM ENGINE  SIM906000A
  ✗ P000129  Made-up sensor A: signal too high
             failing · 3 times · 123456 km · last 2026-07-27 00:00
             priority          6
             aging counter     16
             Sim Engine Speed  1000 1/min
             Sim Terminal 30   12.3 V

```
