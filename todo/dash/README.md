# dash — the display in the air vent

An ESP32-C3 board on the OBD port with a 3.12″ 256×64 OLED, showing a few live values
while driving. `watch` is for a laptop on the passenger seat; the dash is for a glance at
speed. What it does today is in [`README.md`](../../README.md#dash--a-display-in-the-air-vent).

The subsystem's history until 2026-09-13 — hardware survey, the bezel argument, the
graphics-stack survey, the first catalog, the dated statuses — is archived verbatim in
[`.archive/tasks/done/dash/README-2026-09-13.md`](../../.archive/tasks/done/dash/README-2026-09-13.md).

## Rules that stay

**The board resolves nothing; it executes a plan.** `vagcan dev dash build` (or the
firmware's `build.rs`) resolves every channel on the laptop — unit address, identifier,
bit layout, scaling, unit, label in the chosen language — and the firmware links that plan
in. At run time it sends `0x22`, takes the bits, scales, draws. Why:

1. **It does not fit.** A project's `cache.sqlite` is ~88 MB; the C3 has 400 KB of RAM.
2. **Nothing to search.** The board needs a few dozen rows, all known before flashing.
3. **A plan cannot sweep.** The board has no way to ask for an identifier the plan does
   not hold.

**Built for one car, and that is allowed** (owner, 2026-08-20). The plan and the image
are generated under `~/.vagcan/` and `target/`, never committed. Because the image is for
one car, it checks it is in that car: a unit is polled only when the part number it
reports matches the plan. The danger is a plausible number from the wrong identifier,
not a refusal.

**No write services, ever.** The allowlist `0x22 0x19 0x10 0x3E` does not move for a
device that lives in the car. No clearing faults, no mode switching.

**No invented data.** A channel that does not answer shows dashes. No demo ramps, no
placeholder values.

**Charts use a fixed vertical scale** from the plan. Autoscale turns a flat trace into
drama and a real collapse into a flat line.

## Where things are

| what | where |
|---|---|
| firmware | `crates/dash/vag-dash-fw` (`dash`, `slcan`, `rxwatch`; bench images behind `--features bench`) |
| rendering | `crates/dash/vag-dash-render` |
| BLE client, `dashcfg` | `crates/dash/vag-dash-ble`, `crates/dash/vag-dash-cfg` |
| bench rig, `dashsim` | `research/dash/host`, `research/dash/bench.sh` |
| hardware record | `research/dash/can-bring-up.md` |
| a car's plan input | `~/.vagcan/dash/<VIN>/dash.toml` |
| enclosure CAD | `~/CAD/projects/vagcan/` (owner's workspace, not this repo) |

## Tasks

| file | state |
|---|---|
| [`04-alarms.md`](04-alarms.md) | render side done; firmware wiring open |
| [`06-car-and-bench.md`](06-car-and-bench.md) | questions only the car answers |
| [`13-screens.md`](13-screens.md) | channel menu for pages |
| [`14-one-bus-three-clients.md`](14-one-bus-three-clients.md) | design and work order (§7) |
| [`15-enclosure.md`](15-enclosure.md) | enclosure hand-off |
| [`16-uds-over-ble.md`](16-uds-over-ble.md) | UDS over BLE — next |

Done: `01`, `02`, `03`, `05`, `10`, `11`, `12` in `.archive/tasks/done/dash/`.
Superseded: `07`, `08`, `09` in `.archive/specs/dash/`.
