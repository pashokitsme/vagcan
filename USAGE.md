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

Reads somebody's diagnostic data into a **project** under `~/.vagcan/data/<project id>/`.
Offline — no adapter, no car. Run it once.

```sh
vagcan setup                    # pick a source from a menu
vagcan setup ~/Downloads/SK37X  # …or name one outright, of either kind
```

**A project is a platform, not one car.** VW's own mapping files every Octavia III,
Karoq and Kodiaq under `SK37X`, so two cars can share a project; what is true of exactly
one car lives under `~/.vagcan/cars/<VIN>/` instead. Running a second source into a
project **adds** to it, and nothing already there is replaced.

#### Naming a source outright

The folder itself says which of the two kinds it is, so there is nothing to pick. This
is an extracted ODIS project:

```
$ vagcan setup ~/Downloads/SK37X
Project `SK37X` — the name ODIS gives this folder.
New — nothing has been read into it yet.
Opening the ODIS project — its two string pools are read whole, which
takes a moment:
    /Users/you/Downloads/SK37X
230 pools, project version 2610.2.688.
Writing into /Users/you/.vagcan/data/SK37X

Reading the ODIS project at /Users/you/Downloads/SK37X
[1/2] Control units — walking each variant's measurement chain.
[2/2] Names — every object in every pool, for the (text id, name)
      pairs they carry.

Done.

  the control units this project describes: 633 of 717 variants, 310734 channels, 0 refused, 2 unreadable
    /Users/you/.vagcan/data/SK37X/cache.sqlite
  the measurement names: 21576 names
    /Users/you/.vagcan/data/SK37X/names.json

Next:  vagcan devices      is the adapter connected?
       vagcan info         which car is this?
       vagcan faults       stored faults, named — the labels are copied in now

This project carries scalings, declared per ECU variant — so a channel it
describes reads as a number the first time, with no drive.

They are evidence, not proof: nothing in them has been confirmed against a
car, and where a row you proved yourself disagrees, yours wins. Confirming
one is the same three steps as ever: `vagcan survey`, then
`vagcan watch --out drive.csv`, then `vagcan recording calibrate`.
```

**Fault codes will read as numbers after an ODIS-only run like that one, and nothing on
screen says so.** That is a limit of this build rather than anything about the sources:
an ODIS project carries the fault codes *and* their descriptions in the clear, and the
loader for them is being written. Until it lands, the words come from a VCDS
installation — so if you want named faults today, read one in as well. The closing
`vagcan faults` line above is written for the VCDS branch and overstates what this run
did.

#### Or choose from the menu

With no path, `setup` asks. The first entry is the recommended one and takes both
sources, because they compose rather than compete — see
[`ARCHITECTURE.md`](ARCHITECTURE.md):

```
$ vagcan setup
? What should vagcan learn this car from?
❯ ODIS + VCDS names  channels and scalings from ODIS, wording from VCDS
  ODIS project       a folder like SK37X — what to read, and how to scale it
  VCDS installation  Labels/ and UDS_EV/ — when no ODIS project covers the car
  Download VCDS      fetch Ross-Tech's installer, about 90 MB, and read that
↑↓ move   ⏎ choose   1-4 pick   q quit
Drag the folder into this window, or paste its path. An empty line goes back.
Where is the ODIS project? /Users/you/Downloads/SK37X
? Where should the measurement names come from?
❯ VCDS installation  point at one — its text table carries the wording
  Download VCDS      fetch Ross-Tech's installer, about 90 MB
  Skip the names     the channels keep the phrasing ODIS gives them
↑↓ move   ⏎ choose   1-3 pick   q quit
Drag the folder into this window, or paste its path. An empty line goes back.
Where is the VCDS installation? /Users/you/vcds-en
Project `SK37X` — the name ODIS gives this folder.
New — nothing has been read into it yet.
Opening the ODIS project — its two string pools are read whole, which
takes a moment:
    /Users/you/Downloads/SK37X
230 pools, project version 2610.2.688.
Writing into /Users/you/.vagcan/data/SK37X

Reading the VCDS installation at /Users/you/vcds-en
[1/4] Raw files — copying the .rod files and the fault text into the
      shared pool, so the installation can be deleted afterwards.
[2/4] Label files — parsing every .lbl and decrypting every .clb.
cached 3035 label files (101241 measurements) in /Users/you/.vagcan/data/SK37X/cache.sqlite
[3/4] Measurement names — opening TTTEXT.ROD, then reading its cipher.
…
[4/4] .rod section keys — searching for the ones not already cached.
…
Reading the ODIS project at /Users/you/Downloads/SK37X
[1/2] Control units — walking each variant's measurement chain.
[2/2] Names — every object in every pool, for the (text id, name)
      pairs they carry.

Done.

  the raw files: 16577 files copied, 0 already current
    /Users/you/.vagcan/rod
  the label files: 3035 label files
    /Users/you/.vagcan/data/SK37X/cache.sqlite
  the measurement names: 14738 names
    /Users/you/.vagcan/data/SK37X/names.json
  the .rod section keys: 3 keys
    /Users/you/.vagcan/data/SK37X/rod-keys.json
  the control units this project describes: 633 of 717 variants, 310734 channels, 0 refused, 2 unreadable
    /Users/you/.vagcan/data/SK37X/cache.sqlite
  the measurement names: 36314 names
    /Users/you/.vagcan/data/SK37X/names.json
```

(The `…` are the two `.rod` section listings the key search prints as it goes; they run
to a few dozen lines and say nothing you have to act on.)

The VCDS half is read **first**, and the names count climbing from 14738 to 36314 is
why: recovering names from `TTTEXT.ROD` writes the file wholesale, while the ODIS pass
merges into whatever is already there. The other way round, the wholesale write would
land on top.

**Abandoning the second question is a real answer**, not a failed run. Press `q` or give
an empty path and the project keeps the phrasing ODIS gives its channels
(`Engine_temperature`), which reads and scales perfectly well. Adding wording later is
another `vagcan setup` into the same project.

**Run it again and it does almost nothing.** Each VCDS step is skipped when what it
would write is newer than what it would read; a second run on an unchanged installation
takes about a second and says which steps it skipped. `--refresh` forces the lot — what
you want after updating VCDS.

**Recovering `.rod` keys costs about a minute of every core per blocked section.**
The search is built in — there is no flag to pass — and `setup` only ever runs it for
the two files every car needs (`RD.rod`, `MUX.rod`). Nothing on the live path searches:
a section with no cached key is reported as unreadable rather than paid for again.

Without it, setup uses the keys already cached and says so rather than pretending the
sections are empty.

**A VCDS installation on its own supplies no scalings.** None carries them — see
[`ARCHITECTURE.md`](ARCHITECTURE.md) — so on that path every channel starts as raw bytes
until it is measured; see
[teaching it a new measurement](#flow-teaching-it-a-new-measurement). An ODIS project
does carry them, and says so in its closing lines.

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
vagcan units                          # just the ids
vagcan units --identify               # each unit names itself; slower
```

After `setup`, `--identify` also resolves each part number against the label files, which
supplies the unit's diagnostic number and name and pairs that number with the CAN id
that answered — none of it written in this program.

### `vagcan faults` — what is wrong with it?

```sh
vagcan faults                          # every unit, confirmed codes, named
vagcan faults --ecu 01,713 --details   # two units, with the raw freeze frames
vagcan faults --all                    # every code, not only the confirmed ones
```

Once a **VCDS installation** has been read, the codes come out in VW's own words with no
extra flag — the fault text is in `~/.vagcan/rod/`. A project set up from an ODIS
project alone shows the numbers instead: the fault text is in there too, in the clear,
but the loader for it is still being written, so today the words come from VCDS. Run
`setup` on an installation as well and this section fills in.

The raw files are shared across every project and only ever swapped wholesale for a
different **language build** — an English install landing on a Russian one clears it
first and says so, because layering the two would leave names from one beside fault text
from the other.

```
$ vagcan faults
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

`scan` reads one unit; `survey` reads every unit the car has. Each unit is asked
**only the identifiers its own data declares it answers** — the car reports what it is
(`F187`, `F19E`, `F1A2`), that resolves to an ODIS variant, and the variant says which
identifiers it defines. `survey` files its result under the car itself
(`~/.vagcan/cars/<VIN>/survey.jsonl`) whether or not `--out` was given, and that is
what makes every control unit watchable afterwards with no flag.

A unit nothing describes is identified and has its faults read, and is not swept.

Each line records **what was asked as well as what answered**, so a later reader can tell
an identifier this car refused from one that run never covered. That is the difference
`watch` uses to keep the declared-but-absent channels off its list, and a survey aimed
with `--blind --range` would otherwise have every identifier outside its range read as
something the car does not have.

> **`--blind` is the invasive one.** It asks a unit identifiers *nothing* says it
> answers, which is structurally a fuzz test of a diagnostic server and is what cost
> the reference car its power steering — twice, the second time with the car parked.
> It has to be aimed at units named one at a time (`--blind 712`); there is no way to
> ask for it car-wide. Both commands also refuse to run on a moving car unless
> `--while-driving` is passed, and both **stop the whole run** if a unit goes quiet or
> goes back on an identifier it already answered. Read [`SAFETY.md`](SAFETY.md).

The diff is the point:

```sh
vagcan survey --out parked.jsonl     # then drive, then:
vagcan survey --out driving.jsonl
vagcan survey --diff parked.jsonl driving.jsonl
```

The identifiers whose bytes moved are the live measurements. (An **identifier** is the
numbered address of one value inside a control unit — like `2029` for boost pressure;
you read it and get back raw bytes, and the job is to learn what those bytes mean.)
That list needs no label file at all.

---

## Watching it live

### `vagcan watch`

A full-screen view of several units at once, configured from inside rather than by
flags. The live screen offers `[c] configure`, `[g] chart`, `[s] lines` and `[q] quit`;
`c` opens the chooser, where `[space]` toggles a channel, `[f]` marks a favourite,
`[g]` charts it, `[/]` filters and `[u]` is explained below. `s` opens the chart-lines
screen with the chart up, so you pick what is drawn while you can see it.

```sh
vagcan watch                                 # everything this car can offer
vagcan watch --did '01:2029,202A 713:1001'   # start with these selected
vagcan watch --out drive.csv                 # record while watching
vagcan watch --for 30                        # 30 s of CSV to stdout, no screen
vagcan watch --replay drive.csv              # play a recording back, no car
```

Output that is not a terminal gets the plain CSV mode whether or not it asked, so this
works down a pipe or in a log.

**Two kinds of channel are hidden by default, and `u` brings both back.** The chooser's
title counts them together, as `choose what to show — 800 · 43 hidden`, and its key line
splits them by reason, because the two are answered differently:

- **Nothing anywhere has a name for it** — its label would be the identifier printed
  beside itself. This is the row somebody hunting an unproven measurement wants, which
  is why it is hidden and not dropped.
- **This car was asked for it and said nothing.** A project describes a whole vehicle
  family and no single car has all of it, so a fully named row can sit there unable to
  ever produce a value — worse than a nameless one, because it looks like it works.
  This one needs a survey, and specifically a survey that **recorded the range it
  asked**: a run only says a car lacks an identifier if it put that identifier to it.
  On a car nobody has surveyed, and on a survey written before that range was recorded,
  nothing is hidden for this reason at all.

Where a name
does exist it comes off a chain: a row you proved yourself, then the wording a VCDS
installation recovered for that channel's text id, then the name the ODIS variant
carries for it, then the bare identifier. The VCDS link is used only where a VCDS
installation has actually been read into the project — on an ODIS-only project the
row's own name is the more specific of the two, and the generic one would put two live
channels under a single label.

**Favourites persist per car**, in `~/.vagcan/cars/<VIN>/favourites.json`, and sort to
the top of the chooser.

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
vagcan vcds dump /path/to/VCDS/Labels --out labels.json
vagcan vcds tttext TXT.bin --words /usr/share/dict/words  # recover names from the text table
vagcan vcds analyse --capture c.jsonl --log vcds.csv --out 8V0906264H.json
```

`vcds names` searches names keyed by the label files' own **text id**, not by data
identifier — that join does not exist in the label files. A match is a hypothesis to
test against the car, not an identification.

`vcds labels --from-car` is the one thing in this group that touches a vehicle: it
reads `F19E` off the unit and resolves that.

---

## Flow: a first drive

```sh
vagcan setup                       # once, offline: pick a source
vagcan devices                     # adapter found?
vagcan info                        # which car
vagcan units --identify            # what it has
vagcan faults                      # what is wrong, in VW's words
vagcan survey                      # once, parked
vagcan watch                       # now every unit is on offer
```

---

## Flow: teaching it a new measurement

This is the loop that turns raw bytes into numbers, and it needs **a car**. No VCDS
label file carries a scaling. An ODIS project does, but what it gives you is a
manufacturer's declaration rather than a measurement — this is how you check one, or
how you get a number at all for a channel no project describes.

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

**5. Install and name.** Move the file into your project's `measurements/` directory —
`~/.vagcan/data/<project id>/measurements/` — named for the
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

No source has been read into a project yet. Both artefacts named there come from a VCDS
installation:

```sh
vagcan setup /path/to/VCDS
```

One command, no car. If you have no installation, run `vagcan setup` with no path and
pick the download — that copy is Ross-Tech's software, redistributed unmodified; you
can also get it from them directly at <https://www.ross-tech.com/vcds/download/> and
point `setup` at it.

### "This car has no proven measurement rows" / a screen full of `(raw)`

The car has never been calibrated, and no source describes the channel. **Re-running
`vagcan setup` on a VCDS installation cannot fix this**, however many times you do it:
label files carry names and no scaling at all. An ODIS project covering the car would
supply one — that is the cheap thing to try first — and where neither has it, the
number has to be measured.

```sh
vagcan survey
vagcan watch --out drive.csv
vagcan recording calibrate --log drive.csv --out <part-number>.json
```

The long version is [teaching it a new measurement](#flow-teaching-it-a-new-measurement)
above.

### Other things it may say

**"is not a directory"** from `setup` — nothing is at that path, or what is there is a
file. It names both shapes it would have accepted, and points at Ross-Tech's download
when you have neither:

```
$ vagcan setup ~/Downloads/SK37
Error: /Users/you/Downloads/SK37 is not a directory — there is nothing at that path.

    An extracted ODIS project is the folder holding `AStringData.data.gz` and the `<pool>.key` files.
    A VCDS installation root is the folder holding `Labels/` and `UDS_EV/`.

With no path at all, `vagcan setup` asks which to read — and offers to download an
installation if you have neither.
Ross-Tech's own: https://www.ross-tech.com/vcds/download/
```

A path that *is* a directory but neither shape gets the same list, with a guess in
front of it where there is one to make — pointing at the folder above a project, or at
`Labels/` inside an installation, are the two ordinary misses:

```
$ vagcan setup ~/Downloads
Error: /Users/you/Downloads is neither an ODIS project nor a VCDS installation.
    It does hold one. Did you mean:
        /Users/you/Downloads/SK37X
```

Pointing at an archive rather than an unpacked folder says so too — *"it is a file. If
that is an archive, unpack it and point at the folder it unpacks to."* From the menu
rather than the command line, none of these ends the run: it says the same thing and
asks again.

**"encrypted (recover with …)"** against a `.rod` section — no cached key for it. Run
the search against that file: `vagcan vcds rod <file.rod>`.

**"NO CRIB"** against a `.rod` section — no cached key, and the search cannot start on
that file: it is one of the 40 % that XOR a per-file mask over every section's IV. Not
a damaged file, and not something a retry fixes.

**"no readable fault registry"** — `~/.vagcan/rod/` has no `RD.rod` or no fault text
file, or the registry's key has not been recovered. All three come from `vagcan setup`
run on a VCDS installation.

**Nothing on screen and no error** from `watch` — the car answered nothing. Check
`vagcan devices`, then the termination jumper.
