# vagcan

**UNDER DEVELOPMENT** and **not** battle-tested. Things might change, work wrong, cause faults on your cars and so on. 

A command-line diagnostics tool for VW / Audi / Škoda / SEAT cars, written in Rust. 
Talks to car over simple & stupid CAN transceiver – `MKS CANable V2.0 Pro` or `ESP32` with firmware provided by this repo. Won't work with VCDS' `HEX-V2`.

## Dependencies

- **Rust stable**, edition 2024
- **An slcan USB-CAN adapter.** Development and testing was mainly done on `MKS CANable V2.0 Pro`
- No driver needed as the devices is used as serial
- **Diagnostics data**, for names and numbers instead of raw bytes. Either a
  VW **ODIS-Service project** or a **VCDS installation** will do. Just run `vagcan setup` and it'll offer you to download a temp copy of VCDS for extraction
 
## Tested on

| Name & Year | Platform | Device |
|---|---|----|
| Škoda Octavia III FL, 2017 | MQB | MKS CANable V2.0 Pro (slcan) |
| Škoda Octavia III FL, 2017 | MQB | ESP32 SuperMini with SN65HVD230 (VP230-based) CAN Transceiver |


## Where to start?

First of all and as said earlier, this software will **neither** work over VCDS' `HEX-V2`, `VNCI` or any other diagnostic tool you might already have. Those are not sending CAN data plainly unlike SLCAN devices.

**Tested devices**
- `MKS CANable V2.0 Pro` – works out of box; any CANable device will probably work
- `ESP32 SuperMini + VP230-based CAN transceiver` – needs some soldering & firmware from this repo

**Remove 120 Ω termination jumper** if your device has one. The car's diagnostics port is already terminated

**OBD-II adapter pins**

| OBD-II pin | Adapter |
|---|---|
| 6 | CAN-H |
| 14 | CAN-L |
| 5 or 4 | GND |
| 16 or 1 | **optional, for standalone CAN devices** |

## Install

From cargo:

```sh
cargo install --git https://github.com/pashokitsme/vagcan vag-cli
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

Whatever it reads lands in a **project** under `~/.vagcan/data/<project id>/`. A project
is a **platform, not one car** — VW files every Octavia III, Karoq and Kodiaq under
`SK37X` — so several cars share one, and what is true of exactly one car lives under
`~/.vagcan/cars/<VIN>/` instead. Which vehicles each of VW's project names is [written here](./docs/odis-project-mapping.md).

**VCDS is Ross-Tech's software**, free to download from
<https://www.ross-tech.com/vcds/download/> and redistributed here unmodified, for convenient install only. So
`vagcan setup` offers to fetch a copy and unpacks it for you. Either way, the data
inside is read once, and none of it is baked into the tool.


## Read the car

```sh
vagcan info               # VIN, engine, gearbox
vagcan units --identify   # every control unit the gateway knows about
vagcan faults             # stored fault codes, in VW's own words (after setup)
vagcan watch              # live values from several units at once
```

**No car or adapter yet?** You can still do plenty offline: `vagcan setup` (above),
`vagcan dev vcds names <text>` to search VW's measurement names, and `vagcan dev recording …`
to read back a drive someone else recorded. The offline commands are grouped under
`vcds` and `recording` in [`USAGE.md`](USAGE.md).

<!--## Where your files is stored

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
without a vehicle.-->
