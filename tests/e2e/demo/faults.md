# Stored faults

Kept short on purpose: the listing is being redesigned, and these get blessed again with it.
The engine holds one confirmed code with its extended record, the brakes one pending code,
and 70E never answers.

```console
$ vagcan faults
? success
names    ODIS SIM00-odis (eng)
70E  fault list not read: transport: timeout
unit  id   name        code    dtc      status         km  count  last seen            src   text
01    7E0  SIM ENGINE  000129  P000129  failing    123456      3  2026-07-27 00:00:03  odis  Made-up sensor A: signal too high
--    713  SIM ESC     047120           confirmed

2 stored · 1 failing now · 4 units read

$ vagcan -v faults --unit 01
? success
names    ODIS SIM00-odis (eng)
2 fault texts for 2 variants from /nowhere/SIM00-odis (en)
a stored code is a record that something happened once — not a diagnosis, and not necessarily a fault now; `failing` is failing now
7E0  1 code listed, 1 confirmed
7E0  000129  level 2
7E0  000129  failed now, confirmed · 123456 km, 3×
unit  id   name        code    dtc      status       km  count  last seen            src   text
01    7E0  SIM ENGINE  000129  P000129  failing  123456      3  2026-07-27 00:00:03  odis  Made-up sensor A: signal too high

1 stored · 1 failing now · 1 unit read

```
