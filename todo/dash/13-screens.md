# dash / 13 — what goes on which screen

**Subsystem:** dash · **Needs the car:** no for the choice, yes for the check · 2026-09-13

The dash reads the car (`05`). What it shows is four cells and one chart, chosen on
2026-08-20 before anything was proven. This file is the menu: every channel below is one
the reference car **answered** in the parked survey (✓ in `~/.vagcan/SK37X-channels.md`)
or one a published standard forces (OBD-II `F4xx`), with what is known about its
scaling. The owner picks; the picks go into `~/.vagcan/dash/<vin>/dash.toml`, never here.

**Status column.** *proven* = fitted against a drive (`.archive/research/car/identifier-map.md`,
`gearbox-state.md`); *standard* = OBD-II service 01, scaling from ISO 15031-5, no drive
needed; *declared* = the ODIS project's formula, unconfirmed on the car — reads as a number
the first time, and a wrong one reads exactly like a right one. A declared channel on the
panel is a live test of the declaration; the first drive tells.

**Cost column.** One `0x22` request per channel per refresh; single-frame answers only. The
board polls one unit at a time (`CLAUDE.md`: one conversation), ≈4 ms per exchange on this
bus, so a screen of four cells refreshes at ≈50 Hz if it wants to, and a 10-cell screen at
≈20 Hz. Multi-DID requests (`22 xxxx yyyy`) halve that if the unit accepts them — untested.

## Engine `7E0` (8V0906264H, 1.8 TFSI)

| # | DID | channel | unit | resolution | status | note |
|---|---|---|---|---|---|---|
| E1 | `206E` | crankshaft speed (RPM) | 1/min | 1 | **proven** | the RPM row of the identifier map |
| E2 | `202A` | boost pressure, actual | bar | 0.001 | **proven** | on the panel today; absolute, so 1.0 = atmospheric |
| E3 | `2029` | boost pressure, commanded | bar | 0.001 | **proven** | actual vs commanded gap = the wastegate story |
| E4 | `F405` | coolant temperature | °C | 1 | standard | on the panel today (OBD PID 05) |
| E5 | `202F` | engine oil temperature | °C | 0.1 | declared | on the panel today; cluster `202F` gives the same at 1 °C |
| E6 | `20A1` | calculated oil temperature | °C | 0.1 | declared | the model, not the sensor |
| E7 | `2004` | ignition angle | ° | 0.01 | declared | knock retard shows here first |
| E8 | `2949` / `294A` | average / dynamic average of ignition retard | ° | 0.01 | declared | the knock-control number tuners watch |
| E9 | `2027` | fuel high pressure, actual | bar | 0.1 | declared | rail; `293B` commanded beside it |
| E10 | `2025` | fuel low pressure, actual | bar | 0.001 | declared | lift pump |
| E11 | `2037` | air mass, rated | kg/h | 1 | declared | `394F` air flow at throttle is the measured one |
| E12 | `204C` | fuel consumption | l/h | 0.01 | declared | instantaneous |
| E13 | `2206` | fuel reserve, total | l | 1 | declared | the cluster's `22B0` total is 0.1 l |
| E14 | `206D` | throttle valve control value | % | 0.01 | declared | `20BA` actual angle in ° beside it |
| E15 | `F411` | throttle position | % | 0.4 | standard | OBD PID 11 |
| E16 | `F404` | calculated engine load | % | 0.4 | standard | OBD PID 04 |
| E17 | `F40B` | intake manifold absolute pressure | kPa | 1 | standard | OBD PID 0B; coarse, `202A` is finer |
| E18 | `F40F` | intake air temperature | °C | 1 | standard | OBD PID 0F |
| E19 | `F40D` | vehicle speed | km/h | 1 | standard | OBD PID 0D; **the 0–100 source, coarse** |
| E20 | `2033` | vehicle speed averaged | km/h | 0.01 | declared | **the 0–100 source, fine** — averaged over what is unknown |
| E21 | `29D4` / `29D5` | driver / transmission desired torque | Nm | 0.1 | declared | |
| E22 | `209C` | engine drag torque | Nm | 0.1 | declared | |
| E23 | `203F` | engine torque limitation | Nm | 0.1 | declared | which limiter is active is a separate enum |
| E24 | `0281` | terminal 15 voltage | V | 0.001 | declared | battery under ignition |
| E25 | `F433` | barometric pressure | kPa | 1 | standard | OBD PID 33 |
| E26 | `2976` / `296F` | exhaust gas temperature sensor 1 / before cat | °C | 0.1 | declared | |
| E27 | `203E` | fuel temperature | °C | 0.1 | declared | |
| E28 | `210F` | selected gear (as the engine sees it) | state | — | declared | gearbox `3816` is the proven one |
| E29 | `2018` | cruise control set speed | km/h | 0.01 | declared | |

## Gearbox `7E1` (0CW300041G, DQ200)

| # | DID | channel | unit | resolution | status | note |
|---|---|---|---|---|---|---|
| G1 | `3816` | gear engaged | state | — | **proven** | `code − 1`, `00` none, `0C` reverse |
| G2 | `3809` / `3815` | selector lever P/R/N/D | state | — | **proven** (N weak) | |
| G3 | `380A` | input shaft speed | 1/min | 1 | **proven** | |
| G4 | `380B` | output shaft speed | 1/min | 1 | **proven** | with `380A`: slip and ratio |
| G5 | `028D` | control module temperature | °C | 1 (LE) | declared | on the panel today as КОРОБКА; **this is the mechatronics, not the oil** |
| G6 | `382F` / `3839` | clutch 1 / clutch 2 actual torque | Nm | 0.05 | declared | the DQ200's health, live |
| G7 | `382E` / `3838` | clutch 1 / 2 commanded torque | Nm | 0.05 | declared | |
| G8 | `3821` / `3824` | hydraulic pressure part 1 / 2, actual | bar | 0.01 | declared | the accumulator; the DQ200's other failure |
| G9 | `3803` | brake pressure (as the gearbox sees it) | bar | 0.01 | declared | |
| G10 | `3804` | accelerator pedal position | % | 0.4 | declared | the engine's `F411`/`F449` are the standard ones |
| G11 | `2110` | calculated engine preset torque | Nm | 0.05 | declared | |
| G12 | `2170` | direction of travel | −1/0/+1 | — | **proven** | |

## Cluster `714` (5E0920740D)

| # | DID | channel | unit | resolution | status | note |
|---|---|---|---|---|---|---|
| K1 | `22B0` @272 | fuel level, total | l | 0.1 | declared | one DID, several fields; the tank |
| K2 | `22D1` | engine speed as displayed | 1/min | 0.25 | declared | what the needle shows |
| K3 | `202F` | oil temperature as displayed | °C | 1 | declared | |

## ESC `713` (5Q0614517AQ) — declared, none surveyed

| # | DID | channel | unit | resolution | status | note |
|---|---|---|---|---|---|---|
| S1 | `1800`–`1803` | wheel speeds, four | km/h | 0.1 | declared, **not asked** — the parked survey skipped `18xx` | a rear wheel is not driven on DQ200 and does not spin at launch; a candidate 0–100 source (`14` §6) |
| S2 | `F40D` | vehicle speed | km/h | 1 | standard | the ESC's copy of PID 0D |
| S3 | `1822` | longitudinal acceleration | m/s² | 0.03125 | declared, **not asked** | launch instant and a spin check, not a speed source — it drifts when integrated (`14` §6) |

## Derived, no new channel

| # | from | shows |
|---|---|---|
| D1 | E2 − 1.0 | boost as gauge pressure (0 = atmospheric), what a boost gauge shows |
| D2 | E2 vs E3 | boost error, actual − commanded — **built** as a `setpoint` pair, the difference on the panel (`18`, 2026-09-15) |
| D3 | G3 / G4 | gear ratio in effect; with G1, clutch slip |
| D4 | gearbox `380B` over time (decided 2026-09-13, `14` §6; `E19`/`E20` were the candidates) | **0–60 and 0–100 stopwatch**: armed at 0 km/h, the two times print when the speed crosses; the chart page projects speed on time for the run |
| D5 | E12 with E19 | l/100 km instantaneous |

## What the owner said on 2026-09-13

The chart page and page switching did not behave (pages switched "strangely", charts never
showed) — a firmware defect, not a data one; open as its own item before adding pages.
The stopwatch is wanted (`D4`). The choice of channels per screen is the owner's and is
recorded in `dash.toml` when made.
