# Using vagcan

Every command, what it prints, and the flows that span several of them. Start at
[`README.md`](README.md) if you have not built the tool yet; read
[`SAFETY.md`](SAFETY.md) before your first sweep; [`ARCHITECTURE.md`](ARCHITECTURE.md)
explains why any of this is shaped the way it is.

Commands are split by what they need. **The top level needs a car in front of you.**
`vagcan recording …` reads back drives this tool recorded, and `vagcan vcds …` reads
VCDS's own files; neither touches a vehicle.

---

## Contents

- [Once, before anything else](#once-before-anything-else)
- [Reading the car](#reading-the-car)
- [Watching it live](#watching-it-live)
- [Timing a run](#timing-a-run)
- [Offline](#offline)
- [Flow: a first drive](#flow-a-first-drive)
- [Flow: teaching it a new measurement](#flow-teaching-it-a-new-measurement)
- [When it says data is missing](#when-it-says-data-is-missing)

---

## Once, before anything else

### `vagcan setup`

Parses a VCDS installation into `~/.vagcan/labels/`. Offline — no adapter, no car.
Run it once.

```sh
vagcan setup /path/to/VCDS      # an installation you already have
vagcan setup                    # or be offered one to download
vagcan setup --lang en          # …and answer the question up front, for a script
```

It takes several minutes over a full corpus, almost all of it in step 2. Watch it
say what it is doing:

```
Reading the VCDS installation at /Users/you/vcds-en
Writing everything to /Users/you/.vagcan/labels

[1/3] Label corpus — parsing every .lbl and decrypting every .clb.
cached 3035 label files (101241 measurements) in …/labels/cache.sqlite
[2/3] Measurement names — opening TTTEXT.ROD, then reading its cipher.
      This is the slow part: every record is under its own substitution, and the
      attack bootstraps over several passes. Minutes, not seconds.
[3/3] .rod section keys — searching for the ones not already cached.

Done.

  the label corpus: 3035 label files
    /Users/you/.vagcan/labels/cache.sqlite
  the measurement names: 3987 names
    /Users/you/.vagcan/labels/names.json
  the .rod section keys: 11 keys
    /Users/you/.vagcan/labels/rod-keys.json
```

**Run it again and it does almost nothing.** Each step is skipped when what it would
write is newer than what it would read; a second run on an unchanged installation
takes about a second and says which steps it skipped. `--refresh` forces the lot —
what you want after updating VCDS.

**Recovering `.rod` keys needs the key search compiled in.** It is behind a feature
because it costs about a minute of every core per blocked section:

```sh
cargo install --path crates/vagcan --features rod-crack
```

Without it, setup uses the keys already cached and says so rather than pretending the
sections are empty.

**What setup does not do is supply scalings.** No VCDS installation carries them —
see [`ARCHITECTURE.md`](ARCHITECTURE.md). Those are measured; see
[teaching it a new measurement](#flow-teaching-it-a-new-measurement).

### `vagcan devices`

Lists connected USB-CAN adapters. Run it first when anything says it cannot find one.

```
$ vagcan devices
/dev/cu.usbmodem206E37A148451  CANable 2.0 (slcan)
```

Nothing listed, adapter definitely plugged in? Unplug and replug it. It can enumerate
on USB without the OS attaching a serial node, and then there is genuinely nothing to
open. That is a USB-stack hang, not a bus fault.

---

## Reading the car

### `vagcan info` — which car is this?

```
$ vagcan info
VIN      XW8ZZZ…
Engine   8V0906264H  1.8l R4 TFSI   HW 06K907425B
Gearbox  0CW300041G  GSG DQ200G2_M  SW 1003
```

### `vagcan units` — what is in it?

One read of the gateway's own installation list, rather than sweeping every address
and waiting out a timeout for each one the car does not have.

```sh
vagcan units                                    # just the ids
vagcan units --identify                         # each unit names itself; slower
vagcan units --identify --labels /path/to/VCDS  # and the corpus names it too
```

With `--labels`, a part number the car reports is resolved against the corpus, which
supplies the unit's diagnostic number and name — and pairs that number with the CAN id
that answered. Neither half is written in this program.

### `vagcan faults` — what is wrong with it?

```sh
vagcan faults                          # every unit, confirmed codes only
vagcan faults --labels /path/to/VCDS   # …in VW's own words
vagcan faults --ecu 01,713 --details   # two units, with the raw freeze frames
vagcan faults --all                    # every code, not only the confirmed ones
```

```
$ vagcan faults --labels ~/vcds-en
--  713  ESC
  000129  (297)   confirmed
      B1168 F2  Steering Angle Sensor: Not Initialized
      212869 km, 1×
      2026-07-30 18:15:06 by the car's own clock
```

A stored code is a record that something happened once — **not** a diagnosis, and not
necessarily a fault present now. Only codes marked *failed now* are currently failing,
and the tool says so above every listing.

Clearing faults is a write. This tool cannot do it and never will.

`--labels` still takes a directory even after `vagcan setup`: naming a fault needs the
installation's own `RD.rod` and `Codes.dat`, not just the parsed cache.

### `vagcan properties --ecu 01` — what does this unit say about itself?

Sweeps the identification range and names what answers: part numbers, software
versions, the ODX label file the unit is described by, and the OBD-II mode 09 block.

### `vagcan sensors` — the standard readings

The legislated SAE J1979 parameter set, mirrored at `F400 + PID`. Public conversions,
no reverse engineering, and works on any OBD-II car.

They are converted **only** on the emissions-related units ISO 15765-4 addresses
(`0x7E0..0x7E7`) and **only** where the answer is the width J1979 defines. Both gates
are needed: on the reference car the climate unit answers `F405` with one byte — the
right width for the wrong quantity — and the gearbox answers `F40D` with two
little-endian bytes where PID `0D` is one. Anything refused is still shown, as bytes,
with the reason.

### `vagcan scan --ecu 01` / `vagcan survey` — what does it answer?

`scan` sweeps one unit; `survey` sweeps every unit the car has, in about eight
minutes. `survey` files its result under the car itself
(`~/.vagcan/cars/<VIN>/survey.jsonl`) whether or not `--out` was given, and that is
what makes every control unit watchable afterwards with no flag.

> **A sweep is the most invasive thing here.** Structurally it is a fuzz test of a
> diagnostic server, and it is what cost the reference car its power steering. Both
> commands refuse to run on a moving car unless `--while-driving` is passed. Read
> [`SAFETY.md`](SAFETY.md) before the first one, and sweep parked.

The diff is the point:

```sh
vagcan survey --out parked.jsonl     # then drive, then:
vagcan survey --out driving.jsonl
vagcan survey --diff parked.jsonl driving.jsonl
```

The identifiers whose bytes moved are the live measurements. That list needs no label
file at all.

---

## Watching it live

### `vagcan watch`

A full-screen view of several units at once, configured from inside with `c` rather
than by flags. `/` filters across everything the survey found; `g` marks a channel
for the chart; `s` saves.

```sh
vagcan watch                                 # everything this car can offer
vagcan watch --did '01:2029,202A 713:1001'   # start with these selected
vagcan watch --out drive.csv                 # record while watching
vagcan watch --for 30                        # 30 s of CSV to stdout, no screen
vagcan watch --replay drive.csv              # play a recording back, no car
```

Output that is not a terminal gets the plain CSV mode whether or not it asked, so this
works down a pipe or in a log.

**Values with no proven scaling are shown as bytes and tagged `(raw)`**, never as a
bare number — a reader cannot tell an invented number from a measured one, and this
project has twice caught itself believing one of its own. The summary printed before
the screen opens says how many channels are in that state and what turns them into
numbers.

### `vagcan sniff`

Watches the bus listen-only, which cannot disturb anything. Made to run alongside
VCDS: CAN is multi-drop, so both adapters share the bus and this one records the whole
conversation.

```sh
vagcan sniff --out capture.jsonl --diag-only --seconds 120
```

---

## Timing a run

### `vagcan measure`

Arms itself when the car stands still, starts when it moves, and times every mark on
the way up. No keystroke is needed for a run to be measured, and nothing prompts the
driver while the car is moving.

```sh
vagcan measure                 # every time, every mark, acceleration, shift costs
vagcan measure --full          # …and the power column
vagcan measure setup           # describe this car once, by coastdown
vagcan measure view            # open a saved session as a chart page
```

```
  Run 2 — measured
    mark (km/h)   time                   average acceleration
    0-100         9.09 s (8.94 … 9.24)   3.06 m/s²
    peak engine speed   6308 /min at 8.4 s
  Run 2 — computed   (mass 1575 kg, CdA 2.21 m², ρ 1.188 kg/m³ measured)
    peak power, wheel   193 PS (142.1 kW)    estimate
```

A mark from a standstill carries a **range**, not a `±`. The car is already rolling
before its own speed signal wakes up, and where inside that gap it started cannot be
recovered; the two ways of extrapolating back to zero err in opposite directions, so
the answer is somewhere between them. Nothing there is more likely in the centre,
which is why it is not written as a tolerance.

`--full` needs the car measured first. `vagcan measure setup` walks through a
coastdown and works out its actual drag area and rolling resistance instead of
assuming typical values.

`measure` resolves its channels **by what the catalogs call them**, not by identifier.
A car whose rows are named differently is refused, and the refusal lists the names it
looked under.

---

## Offline

### `vagcan recording …` — drives this tool recorded

```sh
vagcan recording discover --log drive.csv          # which columns carry state
vagcan recording discover --log drive.csv --pairs  # …and which move together
vagcan recording calibrate --log drive.csv         # fit unknowns against knowns
vagcan recording calibrate --log drive.csv --out 8V0906264H.json
```

`calibrate` is the no-VCDS route to a proven scaling: it fits raw columns against
columns already trusted **in the same recording**, so there is one clock, tens of
hertz, and no alignment error. `--out` writes the fits as a catalog; the rows are
keyed by identifier and deliberately carry no name.

### `vagcan vcds …` — VCDS's own files

```sh
vagcan vcds names "boost"                      # search the recovered names
vagcan vcds labels /path/to/VCDS --part 8V0906264H
vagcan vcds labels /path/to/VCDS --block 2 --field 1
vagcan vcds labels /path/to/VCDS --from-car    # ask the unit which file is its own
vagcan vcds rod TTTEXT.ROD --dump out/         # open a .rod container
vagcan vcds corpus /path/to/VCDS/Labels --out corpus.json
vagcan vcds analyse --capture c.jsonl --log vcds.csv --out 8V0906264H.json
```

`vcds names` searches names keyed by the corpus's own **text id**, not by data
identifier — that join does not exist in the label files. A match is a hypothesis to
test against the car, not an identification.

`vcds labels --from-car` is the one thing in this group that touches a vehicle: it
reads `F19E` off the unit and resolves that.

---

## Flow: a first drive

```sh
vagcan setup /path/to/VCDS         # once, offline
vagcan devices                     # adapter found?
vagcan info                        # which car
vagcan units --identify            # what it has
vagcan faults --labels /path/to/VCDS
vagcan survey                      # once, parked, ~8 min
vagcan watch                       # now every unit is on offer
```

---

## Flow: teaching it a new measurement

This is the loop that turns raw bytes into numbers, and it needs **a car, not a VCDS
installation**. Nothing in any label corpus carries a scaling.

**1. Find what moves.** Two sweeps, one parked and one after a drive:

```sh
vagcan survey --out parked.jsonl
vagcan survey --out driving.jsonl
vagcan survey --diff parked.jsonl driving.jsonl
```

The identifiers whose bytes differ are the live measurements.

**2. Record them with something trusted beside them.**

```sh
vagcan watch --did '01:2029,202A 7E0:F40D' --out drive.csv
```

Include at least one channel that is already proven — a standard OBD-II parameter
will do. That is what the unknown gets fitted against.

**3. Sort them.**

```sh
vagcan recording discover --log drive.csv
```

Never-moved, stepped between a few values, or continuous. A stepped one is a gear, a
mode or a switch and wants an `Enum`, not a line.

**4. Fit them.**

```sh
vagcan recording calibrate --log drive.csv --out 8V0906264H.json
```

Nothing under R² 0.995 over 20 points and 4 distinct raw values is accepted.

**5. Install and name.** Move the file to `~/.vagcan/labels/data/`, named for the
part number the unit reports for itself (`vagcan properties --ecu 01` shows it). The
rows arrive keyed by identifier and unnamed, because a fit proves what the bytes mean
and not what the quantity is called — `vagcan vcds names <word>` is where the wording
comes from. `measure` in particular looks for rows named `speed` and `gear`.

The alternative route, if you have VCDS and want it to do the naming for you: run
`vagcan sniff --out capture.jsonl` while VCDS logs measuring blocks on the same car
at the same moment, then `vagcan vcds analyse --capture capture.jsonl --log
vcds-export.csv --out <part>.json`.

---

## When it says data is missing

There are exactly two shortages and they have opposite fixes. Running the wrong one
cannot help.

### "The measurement names are not on this machine" / "The .rod section keys are not…"

Nothing has been parsed out of a VCDS installation yet.

```sh
vagcan setup /path/to/VCDS
```

Offline, one command, no car. If you have no VCDS installation, run `vagcan setup`
with no path and it offers to download one, or get it from Ross-Tech directly:
<https://www.ross-tech.com/vcds/download/>. This project cannot ship the data — it is
Ross-Tech's.

### "This car has no proven measurement rows" / a screen full of `(raw)`

The car has never been calibrated. **`vagcan setup` cannot fix this**, however many
times you run it: a label corpus carries names and no scaling at all, so no
installation of VCDS contains what is missing.

```sh
vagcan survey
vagcan watch --out drive.csv
vagcan recording calibrate --log drive.csv --out <part-number>.json
```

The long version is [teaching it a new measurement](#flow-teaching-it-a-new-measurement)
above.

### Other things it may say

**"is not a directory"** from `setup` — the path is not a VCDS installation root. That
is the directory holding `Labels/` and `UDS_EV/`.

**"encrypted (recover with …)"** against a `.rod` section — no cached key for it, and
this build has no key search. Reinstall with `--features rod-crack`.

**"NO CRIB"** against a `.rod` section — no cached key, and the search cannot start on
that file: it is one of the 40 % that XOR a per-file mask over every section's IV. Not
a damaged file, and not something a retry fixes.

**"no readable fault registry"** — `--labels` is pointing somewhere without
`UDS_EV/RD.rod` and `Codes.dat`, or the registry's key has not been recovered. Both
come from `vagcan setup`.

**Nothing on screen and no error** from `watch` — the car answered nothing. Check
`vagcan devices`, then the termination jumper.
