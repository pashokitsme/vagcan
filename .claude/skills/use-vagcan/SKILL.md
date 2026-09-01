---
name: use-vagcan
description: Use when asked to read something off the car with vagcan — fault codes, live measurements, which control units it has, what a unit exposes — or when a command that touches the vehicle needs choosing.
---

# Reading the car with `vagcan`

## Overview

`vagcan` reads a VAG car over a USB-CAN adapter on the OBD-II port. It is read-only
by construction: the UDS allowlist admits `0x22`, `0x19`, `0x10` and `0x3E` and
nothing else, so no command here can change anything on the car.

Read-only is not the same as harmless. **A sweep is a fuzz test of a diagnostic
server**, and on this reference car one crashed the electric power steering, twice —
the second time permanently; `research/eps/eps-j500-report-ru.md` has the account. That is why the section on
what not to run is not advisory.

Everything below needs the car present and the ignition on. Commands under
`vagcan vcds` and `vagcan recording` need only files and are out of scope here.

## Invoking it

There is no installed binary. Everything below is `cargo run -q -p vagcan -- <args>`
from the repository root; the commands are written bare for readability.

## Quick reference

| Task | Command |
|---|---|
| Is the adapter there? | `vagcan devices` |
| Which car is this? | `vagcan info` |
| Which control units does it have? | `vagcan units --identify` |
| What does one unit say about itself? | `vagcan properties --ecu 01` |
| Fault codes, one unit | `vagcan faults --ecu 01` |
| Fault codes, whole car, **named** | `vagcan faults` |
| Standard OBD-II sensors | `vagcan sensors --ecu 01` |
| Monitor for N seconds | `vagcan watch --did "01:2029,202A" --for 20 --hz 10` |
| One instantaneous sample | `vagcan watch --did "01:2029" --for 1 --hz 2` |
| Everything one unit exposes | `vagcan survey --only 01 --out unit01.jsonl` |
| Time an acceleration run | `vagcan measure` |
| Open a saved run as a chart page | `vagcan measure view` (offline) |

Naming a unit: a short number (`01` engine, `02` gearbox, `09`, `16`, `17`) or a
request id (`713`, `70E`). `vagcan units` lists this car's. The two id blocks answer
by different rules, which is why `--ecu 17` and `--ecu 714` are not interchangeable
guesses — pass what `units` printed.

## Monitoring for a set time

`watch` has two modes. With a terminal it draws a full-screen view; **with `--for
SECONDS` it prints CSV and needs no terminal at all**, which is the mode to use from
a script or an agent. Output that is not a terminal takes that mode automatically and
runs until interrupted, so **always pass `--for`** — an agent has no way to send the
interrupt.

```bash
vagcan watch --did "01:2029,202A 02:3816" --for 30 --hz 10 --out drive.csv
```

- `--did` is `unit:did,did unit:did`. A bare list means the engine.
- One CSV row per poll cycle. **Every value carries its own time column** (`name_t_s`)
  because identifiers are polled in batches and columns are up to a cycle apart —
  use those, not the row's `t_s`, when correlating anything.
- A column suffixed `_raw` is **unconverted bytes in hex**, not a number. It means no
  proven scaling exists for that identifier on this unit. Do not do arithmetic on it
  and do not report it as a value.
- `--hz` is a target, not a guarantee: a unit that is slow to answer sets the pace.

Without `--out` the rows go to stdout, flushed per cycle, so they can be read as they
happen.

## Faults, per unit

```bash
vagcan faults                    # named, whole car — the form to prefer
vagcan faults --ecu 01            # one unit
vagcan faults --ecu 01,02,713     # several
vagcan faults                     # every unit, codes only
```

**Run `vagcan setup` once, and faults come out named.** Without it the output is
bare numbers; with it each code carries its SAE code and the text VCDS itself would
print — `000129 → B1168 F2 Steering Angle Sensor: Not Initialized`. On the reference
car 11 of 15 confirmed codes name.

A code stays a number when the chain cannot reach it, and the reason is printed above
the unit: no ODX file of that name in the label files, or no file of its family carrying a
fault catalogue. **That is not a failure to work around.** Naming a fault wrongly is
the one thing this path refuses to do, and it has held at zero wrong answers across
every check (`research/labels/fault-naming-hop.md`).

The first run against an installation recovers the encryption keys of the `.rod`
catalogues, which costs about 95 s of every core per unit file. They are cached — the
project's own cache is `~/.vagcan/data/<project>/rod-keys.json` — so only the first run pays.

Only codes the unit has **confirmed** are shown. `--all` adds the hundreds of tests
that have merely never run since the memory was last cleared; they are not faults and
reporting them as such is wrong. `--supported` lists what a unit *can* report, which
is a capability list, not a diagnosis.

Each fault carries an occurrence count, an odometer reading and a time. Report the
date as the bound the tool states it as — the car's own clock is the source and on
this car it runs days behind.

Clearing faults is a write. This tool cannot do it and must not gain the ability.

## Timing an acceleration run

`vagcan measure` arms itself when the car stands still, starts when it moves, and
times every mark on the way up. No keystroke is needed for a run to be measured and
nothing prompts the driver while the car is moving.

- `measure` alone gives times, speeds, telemetry and shift costs.
- `measure --full` adds power, and is **refused** without a car file rather than
  falling back to generic road-load numbers. `measure setup` writes that file: an
  interview at a standstill, then a coastdown from 120 to 40 km/h, driven twice in
  opposite directions over the same stretch.
- `measure view` opens a saved session as a chart page, offline. With no path it
  offers a car and then one of its sessions.

Two spellings of a time, and they mean different things. A mark from a standstill
prints `9.09 s (8.94 … 9.24)` — the launch instant was never observed, so the figure
in front is the midpoint of a bracket and no better known than the bracket. A rolling
mark prints `5.17 s ± 0.02`, a real ± from the unit's own refresh period. **Never
restate a bracket as a ±.**

Sessions live under `~/.vagcan/cars/<VIN>/measures/`.

## Finding out what a unit exposes

`vagcan units --identify` names every unit the gateway lists. To learn what one of
them actually answers:

```bash
vagcan survey --only 713 --out unit713.jsonl
```

This is the expensive, invasive one — see below. Scope it with `--only` and a
`--range` whenever you can, and prefer an existing survey file over a fresh run.

Two surveys, one parked and one after a drive, name the live measurements without any
label file:

```bash
vagcan survey --diff parked.jsonl driving.jsonl   # offline, no car
```

## Never

- **Never pass `--while-driving`.** A sweep is thousands of requests a unit may never
  have handled, and this is the flag that killed the steering assist. The tool refuses
  by default by reading road speed; a car that will not report speed counts as moving.
- **Never pass `--extended` casually.** The extended diagnostic session is workshop
  mode, and a unit that assists the driver may stop assisting while it is in one.
- **Never run a full `survey` to answer a small question.** It is about eight minutes
  and it is the most invasive thing in the tool. `properties`, `faults --ecu`, or a
  scoped `survey --only` answer most questions.
- **Never suggest adding a write service** — coding, adaptation, clearing faults,
  flashing. `CLAUDE.md` forbids it outright.
- **Never hardcode a car's identifier, scaling or unit name into the code** to make a
  reading work. Those come from the catalogs, keyed by what the unit reports about
  itself.

## When something does not answer

- **"cannot find an adapter"** — run `vagcan devices` first. It is the only command
  that diagnoses this.
- **The adapter enumerates but no serial node exists.** `/dev/cu.usbmodem*` simply is
  not there and every open fails with "No such file or directory". That is a USB-stack
  hang, not a bus fault: a full unplug and replug restores it. Check
  `ls /dev/cu.usbmodem*` before believing any "the bus is dead" result.
- **Silence on the bus is not evidence of a fault** on this platform. The OBD-II
  diagnostic line is nearly idle — about 46 frames in 8 seconds, all one periodic id
  from the gateway.
- **A unit that answers nothing after identifying** is normal; the survey skips it.
- **`watch` refuses with a terminal error** — pass `--for SECONDS`.

## Reporting what was read

- A value with no proven scaling is bytes. Say so; do not convert it.
- A name found with `vagcan vcds names` is a **hypothesis**, not an identification:
  the label files carry no name-to-identifier join. Never present one as the meaning of a
  reading without a live confirmation.
- Quote the unit by both its number and what it called itself, because the numbering
  differs between the two addressing blocks.
