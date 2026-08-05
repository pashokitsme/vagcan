# vagcan

A command-line diagnostics tool for VW / Audi / Škoda / SEAT cars, written in Rust.
It plugs into the OBD-II port through a USB-CAN adapter and reads the car: which
control units it has, what they call themselves, what they measure, what faults they
have stored, and — with `vagcan measure` — how fast the thing actually accelerates.
It **only reads**. There is no coding, no adaptation, no clearing faults, no flashing,
and there never will be; the UDS service allowlist is four entries long and it is
short by policy rather than by omission. A read-only tool is still not a harmless one,
so read [`SAFETY.md`](SAFETY.md) before pointing it at a car you care about.

---

## What you need

- **Rust stable**, edition 2024. No system dependencies.
- **An slcan USB-CAN adapter.** Development was done on an MKS CANable V2.0 Pro
  (STM32G431 with an isolated transceiver). It enumerates as a serial device, so
  there is no driver to install on macOS or Linux.
- **A VCDS installation**, for names instead of numbers. Optional, but without one a
  fault reads `000127 (295)` and a measurement reads `2029 → 04 7E`.

Wire the adapter to the OBD-II port:

| OBD-II pin | Adapter |
|---|---|
| 6 | CAN-H |
| 14 | CAN-L |
| 5 (or 4) | GND |
| 16 | **leave unconnected** |

**Open the adapter's 120 Ω termination jumper.** The vehicle bus is already terminated
at both ends (~60 Ω); a third resistor drags it to 40 Ω, and you will spend an evening
blaming the software.

---

## Install

One command, if you have Rust:

```sh
cargo install --git https://github.com/pashokitsme/vagcan
```

That builds `vagcan` and puts it on your `PATH`, so every example below is just
`vagcan …`. To work on the tool instead, clone it and install from the checkout:

```sh
git clone https://github.com/pashokitsme/vagcan && cd vagcan
cargo install --path crates/vagcan
```

Check it found your adapter:

```
$ vagcan devices
/dev/cu.usbmodem206E37A148451  CANable 2.0 (slcan)
```

If it reports nothing and the adapter is definitely plugged in, unplug and replug it.
It can enumerate on USB without the OS attaching a serial node, and then there is
genuinely nothing to open.

---

## Set up, once

```sh
vagcan setup /path/to/VCDS      # an installation you have
vagcan setup                    # or be offered one to download
```

This parses the label corpus into `~/.vagcan/data/extracted/` — the names of measurements and
faults, the unit numbering, the keys that open VW's encrypted `.rod` files. It is
offline, it takes a few minutes, and it is the only setup step there is. Running it
again on an unchanged installation does nothing and says so.

**The data cannot ship with this tool.** It is Ross-Tech's, derived from their
commercial product, and this repository has none of it. Get VCDS from
<https://www.ross-tech.com/vcds/download/>.

What `setup` does *not* give you is measurement scalings. No label corpus carries
them — that is the single most expensive negative result in this project, and it is
why [`USAGE.md`](USAGE.md) has a section on proving one against your own car.

---

## Read the car

```sh
vagcan info               # VIN, engine, gearbox
vagcan units --identify   # every control unit the gateway knows about
vagcan faults             # stored fault codes, in VW's own words (after setup)
vagcan survey             # once, parked, ~8 min: what every unit answers
vagcan watch              # live values from several units at once
```

Once `setup` has run, `faults` names the codes with no extra flags — the labels are
already in `~/.vagcan`. Every command and every flag is in [`USAGE.md`](USAGE.md).

What the tool deliberately does not do is guess. A value with no proven scaling is
shown as raw bytes and tagged as raw. This project has twice caught itself believing a
number it had invented, and the guards are the scar tissue.

---

## Where your files go

Nothing the tool reads at run time lives in this repository — not the label data,
which is not ours to redistribute, and not the measured rows, which are true of one
car rather than of yours.

```
~/.vagcan/
  data/
    extracted/        parsed from VCDS by `vagcan setup`:
      cache.sqlite      the label corpus, queryable
      names.json        measurement names recovered from VCDS's text table
      rod-keys.json     recovered .rod section keys
    measured/         proven measurement rows, one file per part number
  cars/<VIN>/
    car.json          mass, tyre, measured road load
    survey.jsonl      what this car answered when it was last swept
    measures/         saved acceleration sessions
  config.json
```

`data/extracted/` is rebuilt by `vagcan setup` in minutes and can be deleted at any
time. `data/measured/` and `cars/` cannot be rebuilt without a vehicle.

---

## Status

The tool reads the whole car and names its faults. What is still open is **coverage**:
23 measurement rows are proven across engine, gearbox and instrument cluster, while the
brakes, the body control module and half a dozen other units have not been through the
same process yet.

`vagcan measure` is the newest piece and the least proven — two real drives, with seven
defects found and fixed on the first. Treat its numbers as good until a third drive
says otherwise.

The tool is written to work on any VAG car. It has been *proven* on the ones below.

## Tested on

Run `vagcan info` on your car and add a row. The columns are what other owners can
match against — the engine and gearbox **part numbers** are the keys a measurement
catalog is filed under, so a car sharing one inherits everything proven for it. (The
VIN `vagcan info` also prints identifies one physical car and helps nobody else, so it
is not listed here.)

| Make / model | Year | Platform | Engine | Gearbox | Adapter |
|---|---|---|---|---|---|
| Škoda Octavia III (facelift) | 2017 | MQB | `8V0906264H` — 1.8 R4 TFSI (HW `06K907425B`) | `0CW300041G` — DQ200 7-speed DSG (SW `1003`) | MKS CANable V2.0 Pro (slcan) |

---

- [`USAGE.md`](USAGE.md) — every command, with worked output
- [`ARCHITECTURE.md`](ARCHITECTURE.md) — why it is built this way, and what VCDS's file
  formats actually are
- [`SAFETY.md`](SAFETY.md) — what a read-only tool can still break, and the rules that
  came out of it
