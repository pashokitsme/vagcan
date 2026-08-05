# vagcan

A command-line diagnostics tool for VW / Audi / Škoda / SEAT cars, written in Rust.
It plugs into the OBD-II port through a cheap USB-CAN adapter and reads the car:
which control units it has, what they call themselves, what they measure, what
faults they have stored, and — with `vagcan measure` — how fast the thing actually
accelerates.

It **only reads**. There is no coding, no adaptation, no clearing faults, no
flashing, and there never will be. The UDS service allowlist is four entries long,
and it is short by policy rather than by omission.

> ### Read [SAFETY.md](SAFETY.md) first
>
> A read-only tool is not a harmless tool. An identifier sweep on the reference car
> crashed its power steering unit twice — the second time permanently, needing a
> replacement. Read-only bounds what you can *change* about a car, not what you can
> *provoke*. The full account is in [`research/eps/eps-j500-report-ru.md`](research/eps/eps-j500-report-ru.md).
>
> Everything in `SAFETY.md` is there because something went wrong once.

---

## What it does, honestly

Most of this works. Some of it is two drives old. The table says which.

| | |
|---|---|
| Identify the car — VIN, engine and gearbox passports | works, matches a VCDS Auto-Scan |
| List every control unit, from the gateway's own installation list | works — 15 units on the reference car |
| Read stored fault codes, with occurrence counts, odometer and date | works |
| **Name** those faults in VW's own words | works, from your own VCDS installation |
| Sweep a unit for every identifier it answers | works |
| Live multi-unit view, with charts | works |
| Time an acceleration run, with power and shift costs | built; two drives so far |
| Standard OBD-II sensors | works, on any OBD-II car |

What it deliberately does not do is guess. A value with no proven scaling is shown as
raw bytes and tagged as raw. This project has twice caught itself believing a number
it had invented, and the guards are the scar tissue.

---

## Getting started

### 1. Hardware

Any **slcan** USB-CAN adapter. Development was done on an MKS CANable V2.0 Pro
(STM32G431 with an isolated transceiver, about €25). It enumerates as a serial
device, so there is no driver to install on macOS or Linux.

Wire it to the OBD-II port:

| OBD-II pin | Adapter |
|---|---|
| 6 | CAN-H |
| 14 | CAN-L |
| 5 (or 4) | GND |
| 16 | **leave unconnected** |

**Open the adapter's 120 Ω termination jumper.** The vehicle bus is already
terminated at both ends (~60 Ω); a third resistor drags it to 40 Ω, and you will
spend an evening blaming the software.

### 2. Build

```sh
git clone <this repo> && cd vcds
cargo build --release
./target/release/vagcan devices
```

Rust stable, edition 2024, no system dependencies.

```
$ vagcan devices
/dev/cu.usbmodem206E37A148451  CANable 2.0 (slcan)
```

If it reports nothing and you know the adapter is plugged in, unplug and replug it.
It can enumerate on USB without the OS attaching a serial node, and then there is
genuinely nothing to open. That is a USB-stack hang, not a bus fault.

### 3. Read the car

```sh
vagcan info      # VIN, engine, gearbox
vagcan units     # every control unit the gateway knows about
vagcan faults    # stored fault codes
```

That is the whole of the required setup. Everything below is optional, and makes the
output more readable rather than more complete.

### 4. Optional: label files, for names instead of numbers

Out of the box a fault reads `000127 (295)` and a measurement reads `2029 → 04 7E`.
Those are the car's own words, and they are not good words.

The names live in the label files that ship with **VCDS**, Ross-Tech's commercial
diagnostic software. This repository ships none of that data and cannot — it is
Ross-Tech's, and you need your own copy.

Get it from Ross-Tech: **<https://www.ross-tech.com/vcds/download/>**

Then point `vagcan` at the installation once. It parses the corpus into a SQLite
cache under `~/.vagcan/label-cache/`, and lookups are instant after that:

```sh
vagcan vcds labels /path/to/VCDS --part 8V0906264H
vagcan faults --labels /path/to/VCDS
```

```
16  70C  Lenks.Modul
  047120  (291104)   confirmed
      B1455 01  Temperature Sensor for Heated Steering Wheel
      212869 km, 111×
```

`--refresh` rebuilds the cache after a VCDS update. The cache is derived data and
lives outside the repository on purpose.

### 5. Optional: a survey, so every unit is watchable

`vagcan watch` can only offer measurements it knows a unit has. Run the sweep once,
parked; the result is filed under that car's own directory
(`~/.vagcan/cars/<VIN>/survey.jsonl`), and from then on `watch` picks it up with no
flags at all:

```sh
vagcan survey     # about 8 minutes, parked; engine off is fine
vagcan watch
```

**A sweep is the most invasive thing here.** Structurally it is a fuzz test of a
diagnostic server, and it is what damaged the reference car's steering unit. It
refuses to run on a moving car. Read `SAFETY.md` before the first one.

---

## Examples

**What is this car?**

```sh
$ vagcan info
VIN      XW8ZZZ…
Engine   8V0906264H  1.8l R4 TFSI   HW 06K907425B
Gearbox  0CW300041G  GSG DQ200G2_M  SW 1003
```

**What is wrong with it?**

```sh
$ vagcan faults --labels ~/VCDS
--  713  ESC
  000129  (297)   confirmed
      B1168 F2  Steering Angle Sensor: Not Initialized
      212869 km, 1×
      2026-07-30 18:15:06 by the car's own clock
```

A stored code is a record that something happened once — not a diagnosis, and not
necessarily a fault present now. Only codes marked *failed now* are currently
failing, and the tool says so above every listing.

**Watch it live.** A full-screen view of several units at once, configured from
inside with `c` rather than by flags. `/` filters across everything the survey found;
`g` marks a channel for the chart.

```sh
vagcan watch
```

**What does this unit expose?**

```sh
vagcan properties --ecu 01      # its identification block, named
vagcan scan --ecu 01            # every identifier it answers
vagcan sensors                  # the standard OBD-II parameters
```

**Time a run.** It arms itself when the car stands still, starts when it moves, and
times every mark on the way up. No keystroke is needed for a run to be measured, and
nothing prompts the driver while the car is moving.

```sh
vagcan measure --full
```

```
  Run 2 — measured
    mark (km/h)   time                   average acceleration
    0-100         9.09 s (8.94 … 9.24)   3.06 m/s²
    peak engine speed   6308 /min at 8.4 s
  Run 2 — computed   (mass 1575 kg, CdA 2.21 m², ρ 1.188 kg/m³ measured)
    peak power, wheel   193 PS (142.1 kW)    estimate
```

`--full` wants the car measured once first: `vagcan measure setup` walks you through
a coastdown and works out its actual drag area and rolling resistance instead of
assuming typical values. `vagcan measure view` opens a saved session as a chart page
in the browser.

A mark from a standstill carries a **range**, not a `±`. The car is already rolling
before its own speed signal wakes up, and where inside that gap it started cannot be
recovered; the two ways of extrapolating back to zero err in opposite directions, so
the answer is somewhere between them. Nothing there is more likely in the centre,
which is why it is not written as a tolerance.

**Offline, with no car attached:**

```sh
vagcan recording calibrate drive.csv     # fit unproven columns against trusted ones
vagcan vcds names "boost"                # search the recovered measurement names
vagcan vcds analyse --capture c.jsonl --log vcds.csv
```

---

## Architecture

A Rust workspace. The seam that matters is between **algorithm and data**: no
measurement scaling, identifier number, unit name or part number is written in Rust.
They come from the label corpus and from what the car reports about itself. Adding a
parameter is a row in a JSON file, never a new `match` arm.

```
crates/
  vag-transport   the transport trait — the seam every backend implements
  vag-can         slcan USB-CAN backend, listen-only mode, ISO-TP sniffer
  vag-protocol    UDS client, ISO-TP framing, unit addressing
  vag-data        label parsers and decoders (.lbl/.clb/.rod), ODX resolution
  vag-db          SQLite cache over the label corpus
  vag-capture     capture and replay transport, so tests need no hardware
  vagcan          the CLI
```

**Two addressing conventions are live on the same car.** ISO 15765-4 pairs
`0x7E0..0x7E7` with `+8`, so the engine answers `0x7E0 → 0x7E8`. VW's own block
answers at `+0x6A`, so the instrument cluster is `0x714 → 0x77E`. Assuming only the
first makes every unit outside the powertrain invisible, which is exactly what
happened before it was measured.

**Sweeping is group testing, not 65,536 reads.** A multi-identifier request comes
back with only the identifiers the unit supports, and is refused outright when it
supports none of them — so one request is a presence test for a whole batch. That is
what turns a full sweep from hours into minutes.

**The CLI is split by what a command needs.** The top level is for commands that need
a car in front of you. `recording …` reads back drives this tool recorded, and
`vcds …` reads VCDS's own files. A top level crowded with offline analysis is a top
level nobody can scan while standing at an open driver's door.

Design documents are in [`docs/superpowers/specs/`](docs/superpowers/specs/).

---

## VCDS's file formats

This is the part that took longest, so here is what those files actually are. Full
writeups are under [`research/labels/`](research/labels/).

**`.lbl` — plain text.** The old format, still shipped for older control units. One
file per part number, human readable, with a `; Component: … (#02)` header naming the
unit and its number, then measuring-block and field names. Nothing to crack.

**`.clb` — the encrypted `.lbl`.** Same content in a container; decrypted in-tool by
`vag-data`.

**`.rod` — the ODX container, and the interesting one.** This is where modern
(UDS-era) label data lives. Each file is TEA-CBC encrypted with a per-record IV and
the plaintext is zlib-deflated. Inside are several tables:

| Table | What is in it |
|---|---|
| `STRUC` | measurement structures — 1,221 of them |
| `DOP` / `TTDOP` | computation methods and scaling — 17,636 entries |
| `TTTEXT` | the global text table: every name, in every language |
| `MWB` | the engine measuring-block rows |
| `[DTC]` | the fault-code table, in `RD.rod` |

Payloads are encoded in **base-14** over the charset `0123456789,.-_`, which was
established by disassembling VCDS rather than guessed at.

**A control unit tells you which `.rod` is its own.** Identifier `F19E` returns an ODX
file name — `EV_ECM18TFS0208V0906264H`, say. That is how `vagcan vcds labels --from-car`
finds the right file with no lookup table in the middle.

**`Codes.dat` — the fault-code text store.** A fault number does not resolve to words
directly. The chain is: the raw 24-bit code → the `[DTC]` table in `RD.rod` → the row
the unit's own `.rod` selects → a key into `Codes.dat` → the text. Each `RD.rod`
table's digits are substituted under a per-table alphabet, and that alphabet turned
out to be *generated* from the table key by `srand(key)` and two Fisher-Yates
shuffles sharing one stream — read off the binary, not inferred. 95 of 95 alphabets,
219,490 of 219,490 name fields, zero wrong.
See [`research/labels/fault-naming-hop.md`](research/labels/fault-naming-hop.md).

**What the corpus does *not* hold is the scaling.** This is the most expensive
negative result in the project, and it is structural rather than a matter of not
having looked hard enough. The read identifier is not stored in `STRUC` under any
encoding, and `MWB` carries no per-ECU identifier — so there is no route from "this
unit's boost pressure" to "read `0x202A`, two bytes big-endian, ×0.001 bar" through
any file Ross-Tech ships. Do not go looking; the reasoning is in
[`research/labels/rod-labels.md`](research/labels/rod-labels.md) §4.0c and
[`research/labels/label-linkage.md`](research/labels/label-linkage.md) §3.

Scaling comes from the car instead. `vagcan sniff` records the bus listen-only while
VCDS runs an ordinary session beside it, and `vagcan vcds analyse` crosses that
capture with VCDS's own CSV export, fitting `(identifier, raw form, factor, offset)`
by least squares and accepting nothing under R² 0.995 over 20 points. The corpus
supplies names and per-unit lists. That is what it is for.

---

## Where things are kept

```
crates/         the Rust workspace
catalogs/       proven measurement rows and recovered names — checked-in evidence
  vehicles/       one file per control unit, keyed by the part number it reports
research/       reverse-engineering writeups and tooling, one directory per subject
  labels/         VW's label corpus: the .rod crack, the name codec, fault naming
  car/            what the reference car answers: identifier map, units, surveys
  eps/            the steering-assist incident — read alongside SAFETY.md
  clb-crack/      the RE scripts themselves
archive/        retired paths, kept as evidence
  research/       dead ends that are still true — do not retry them
  specs/          superseded designs
  tasks/          finished task files
docs/           active design specs
todo/           the roadmap and the goal statement
```

Two conventions are worth knowing, because they are why the tree looks like this.

**Nothing is deleted; things are moved.** Most of what this project knows was
measured on one car, once, and several of its most valuable documents are records of
things that did *not* work. A refutation you throw away is one you pay for twice.
`archive/` exists so that "we tried that, here is why it failed" survives a year.

**A measurement is not a cache.** `catalogs/` holds rows that took a drive to
establish and cannot be re-collected without the car. Anything the tool can
regenerate by itself — the label database, a survey, a car's own files — lives under
`~/.vagcan/` and is not in the repository.

Start here: [`todo/README.md`](todo/README.md) for where things stand,
[`todo/GOAL.md`](todo/GOAL.md) for the goal and the stack, and
[`research/labels/rod-labels.md`](research/labels/rod-labels.md) for the format work.

---

## Status

The tool reads the whole car and names its faults. What is still open is **coverage**:
23 measurement rows are proven across engine, gearbox and instrument cluster, while
the brakes, the body control module and half a dozen other units have not been
through the same process yet. The roadmap tracks it.

`vagcan measure` is the newest piece and the least proven — two real drives, with
seven defects found and fixed on the first. Treat its numbers as good until a third
drive says otherwise.

Reference vehicle throughout: Škoda Octavia III facelift, 1.8 TSI, 2017, DQ200. The
tool is written to work on any VAG car. It has been *proven* on one.
