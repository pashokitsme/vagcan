# vagcan

A command-line diagnostics tool for VW / Audi / Škoda / SEAT cars, written in Rust.
It plugs into the diagnostics (OBD-II port) socket and reads
the car's control units: which units it has, what they call
themselves, what they measure, what faults they have stored, and — with
`vagcan measure` — how fast the car actually accelerates.

**This is a hobby project** which is **UNDER DEVELOPMENT**. **Things might (and likely will) work wrong and even cause faults on your car**.

## Dependencies

- **Rust stable**, edition 2024
- **An slcan USB-CAN adapter.** Development was done on an MKS CANable V2.0 Pro
  (STM32G431 with an isolated transceiver). It enumerates as a serial device, so
  there is no driver to install on macOS or Linux.
- **Somebody's diagnostic data**, for names and numbers instead of raw bytes. Either a
  VW **ODIS-Service project** or a **VCDS installation** will do, and you do not need to
  have either already: `vagcan setup` will fetch a VCDS copy if you point it at nothing
  (see below).

Wire the adapter to the OBD-II port:

| OBD-II pin | Adapter |
|---|---|
| 6 | CAN-H |
| 14 | CAN-L |
| 5 (or 4) | GND |
| 16 | **leave unconnected** |

**Open the adapter's 120 Ω termination jumper.** The vehicle bus is probably already terminated
at both ends (~60 Ω); a third resistor drags it to 40 Ω, and you will spend an evening
blaming the software.

## Tested on

Run `vagcan info` on your car and add a row. The columns are what other owners can
match against — the engine and gearbox **part numbers** are the keys a measurement
catalog is filed under, so a car sharing one inherits everything proven for it. (The
VIN `vagcan info` also prints identifies one physical car and helps nobody else, so it
is not listed here.)

| Make / model | Year | Platform | Engine | Gearbox | Adapter |
|---|---|---|---|---|---|
| Škoda Octavia III (facelift) | 2017 | MQB | `8V0906264H` — 1.8 R4 TFSI (HW `06K907425B`) | `0CW300041G` — DQ200 7-speed DSG (SW `1003`) | MKS CANable V2.0 Pro (slcan) |

## Install

From cargo:

```sh
cargo install --git https://github.com/pashokitsme/vagcan vagcan
```

From local repository:

```sh
git clone https://github.com/pashokitsme/vagcan
cd vagcan
cargo install --path crates/cli/vag-cli
```

Run it with no arguments to see where you stand — what it is, whether an adapter and
a car's data are there, and what to type next:

```sh
vagcan
```

Check it found your adapter:

```
$ vagcan devices
/dev/cu.usbmodem206E37A148451  CANable 2.0 (slcan)
```

If it reports nothing and the adapter is definitely plugged in, unplug and replug it.
It can enumerate on USB without the OS attaching a serial node, and then there is
genuinely nothing to open.


## Setting up

```sh
vagcan setup                    # pick a source from a menu
vagcan setup ~/Downloads/SK37X  # …or name one outright, of either kind
```

`setup` asks what to learn the car from, and there are two kinds of answer. A VW
**ODIS-Service project** is the good one: it declares, per control-unit variant, every
identifier that unit answers, where the value sits in the reply, and how to scale it. A
**VCDS installation** is the fallback — for a car no ODIS project covers, or for anyone
who cannot get one — and it carries wording and fault text but no scalings at all. The
top menu entry takes both at once, because they compose: the structure from ODIS, the
human wording from VCDS.

It needs no car and no adapter, takes minutes, and is the only setup step there is.
Running it again on an unchanged source does nothing and says so.

Whatever it reads lands in a **project** under `~/.vagcan/data/<project id>/`. A project
is a **platform, not one car** — VW files every Octavia III, Karoq and Kodiaq under
`SK37X` — so several cars share one, and what is true of exactly one car lives under
`~/.vagcan/cars/<VIN>/` instead. Which vehicles each of VW's project names covers is
transcribed in
[`research/labels/odis-project-mapping.md`](research/labels/odis-project-mapping.md);
it is a reading aid, and nothing in the tool consults it — a project declares its own
coverage.

**VCDS is Ross-Tech's software**, free to download from
<https://www.ross-tech.com/vcds/download/> and redistributed here unmodified, for convenient install only. So
`vagcan setup` offers to fetch a copy and unpacks it for you. Either way, the data
inside is read once, and none of it is baked into the tool.


## Read the car

```sh
vagcan info               # VIN, engine, gearbox
vagcan units --identify   # every control unit the gateway knows about
vagcan faults             # stored fault codes, in VW's own words (after setup)
vagcan survey             # once, parked: what every unit answers
vagcan watch              # live values from several units at once
```

Once a VCDS installation has been read, `faults` names the codes with no extra flag —
the fault text is already in `~/.vagcan`. Every command and every flag is in
[`USAGE.md`](USAGE.md).

What the tool deliberately does not do is guess. A value with no proven scaling is
shown as raw bytes and tagged as raw. This project has twice caught itself believing a
number it had invented, and the guards are the scar tissue.

**No car or adapter yet?** You can still do plenty offline: `vagcan setup` (above),
`vagcan vcds names <text>` to search VW's measurement names, and `vagcan recording …`
to read back a drive someone else recorded. The offline commands are grouped under
`vcds` and `recording` in [`USAGE.md`](USAGE.md).
<!--

## Where your files go

Nothing the tool reads at run time lives in this repository. The diagnostic data is
rebuilt into `~/.vagcan` by `setup` — the raw VCDS archives it parses are Ross-Tech's,
vendored under `vendor/` and redistributed unmodified — and the measured rows are true
of one car rather than of yours.

```
~/.vagcan/
  rod/                  the raw .rod files and the fault text, shared by every project
  data/<project id>/    one directory per platform, e.g. SK37X:
    cache.sqlite          the channels and the label rows, queryable
    names.json            text id → name
    rod-keys.json         recovered .rod section keys
    sources.json          which installation or project each of these came from
    measurements/         proven-on-a-car rows, one file per part number
  cars/<VIN>/           what is true of exactly one car
    car.json              mass, tyre, measured road load
    survey.jsonl          what this car answered when it was last swept
    measures/             saved acceleration sessions
  config.json
```

Everything a project holds except `measurements/` is rebuilt by `vagcan setup` in
minutes and can be deleted at any time. `measurements/` and `cars/` cannot be rebuilt
without a vehicle.

## Status

The tool reads the whole car and names its faults. What is still open is **coverage**:
23 measurement rows are proven across engine, gearbox and instrument cluster, while the
brakes, the body control module and half a dozen other units have not been through the
same process yet.

`vagcan measure` is the newest piece and the least proven — two real drives, with seven
defects found and fixed on the first. Treat its numbers as good until a third drive
says otherwise.

The tool is written to work on any VAG car. It has been *proven* on the ones below.-->

## Code quality

The whole project is built by Claude, and I don't really care about the code it produced. Althrough, the UX of the tool and test coverage is very important part and must be considered firstly
