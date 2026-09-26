# vagcan

> **Under development, not battle-tested.** Commands and formats may change. Readings may be wrong. Reading a car can still make a control unit log a fault.

A command-line diagnostics tool for VW, Audi, Škoda and SEAT cars, written in Rust.

It talks to the car through a plain CAN adapter: an `MKS CANable V2.0 Pro`, or an `ESP32` with the firmware from this repository.

The tool is not designed for write operations: coding, adaptations, clearing faults and especially flashing are **not supported** (at the moment, at least).

## Requirements

- **Rust stable**, edition 2024.
- **An slcan USB-CAN adapter.** Developed and tested mainly on `MKS CANable V2.0 Pro`. No driver is needed: the adapter shows up as a serial port.
- **Diagnostic data**, for names and numbers instead of raw bytes: a VW **ODIS-Service project** or a **VCDS installation**. `vagcan setup` offers to download VCDS if you have neither.


## Features

### Reading the car

| Command | What it does |
|---|---|
| `vagcan` | Shows where you stand: adapter, car data, what to type next |
| `vagcan setup` | Reads an ODIS project or a VCDS installation once, offline. Takes seconds for ODIS, minutes for VCDS |
| `vagcan devices` | Lists USB-CAN adapters and dash boards on USB |
| `vagcan info` | VIN, engine and gearbox identity |
| `vagcan units` | Control units the gateway lists. `--identify` makes each one name itself |
| `vagcan faults` | Stored fault codes with VW's own text. `[faults] language` in the config picks the language |
| `vagcan sensors` | Standard OBD-II readings |
| `vagcan watch` | Live values from several units at once, with a chart of up to 6 channels. States show by name. `--out` records to CSV |
| `vagcan measure` | Acceleration run timing from the car's own speed signal. `measure view` opens a saved run as a chart |

### Development commands – `vagcan dev`

| Command | What it does |
|---|---|
| `dev survey` | Reads every control unit: identity, faults, the identifiers its data declares. Refused while driving unless `--while-driving` is given |
| `dev sniff` | Records the bus. Listen-only by default. Reports dropped frames if the adapter can tell |
| `dev glossary` | Your own names for channels, in `~/.vagcan/names.csv` |
| `dev recording` | Works on recorded drives: `calibrate` proves scalings, `discover` finds gear and mode channels, `dash` plays one on the dash panel |
| `dev vcds` | Works on VCDS files: label lookup, name search, `.rod` decryption, log analysis |
| `dev dash build` | Builds the dash plan for one car |

### Where names and numbers come from

- **ODIS-Service project** (preferred): channels, byte layout, scaling and fault text for every control-unit variant.
- **VCDS installation** (fallback): channel names and fault text, no scalings.
- **Your own drives**: scalings proven on the car always override both.

Everything lives under `~/.vagcan/`. Nothing about any car is built into the tool.

### Dash Display (technically implemented but I didn't assemble it irl yet)

An ESP32-C3 board on the OBD port that shows live values on a 3.12″ 256×64 OLED.

- **Values page**: up to 4 cells, each with a label, a value and a unit.
- **Chart page**: one channel shown large, with its recent history on a fixed scale.
- **Pages** are set per car in `dash.toml` and switched with the board's button.
- **Alarms**: `[[alarm]]` rules in `dash.toml` watch channels on any page. Past the threshold — or, for a `kind = "drift"` rule, once a channel has held far enough from what its unit asked for — the board shows the rule's page with the offending cell inverted: blinking while the value is out, steady for the 2.5 s the page stays up after it is back. A short press silences it until the value comes back.
- **Specified values**: a channel paired with `setpoint` shows the difference from what its control unit asked for on a line of its own, under the number. [`docs/dash/dash-toml.md`](docs/dash/dash-toml.md) is the whole file format.
- **No invented numbers.** A channel that does not answer shows dashes.
- **Plan checks**: the board polls a unit only if the part number the unit reports matches the plan.
- **BLE**: `dashcfg` sets brightness and the active page. Settings are stored on the board. BLE is always on: no button, no pairing.
- **UDS over the USB cable**: on the `dash` image, `vagcan` reads the car through the board like through a CANable, and the panel keeps working. `vagcan devices` lists it as `dash image`.
- **UDS over BLE**: the same with no cable, slower.
- **Sweeps need a plain adapter**: through the board `dev survey` and `units --identify <unit>` are refused, and `dev sniff` needs `--slcan`.
- **`--slcan`**: `vagcan --slcan …` makes the `dash` image a plain slcan adapter for that run, with no reflash. The panel shows `SLCAN`, the bit rate and frame counters. It ends when that run ends.
- **Laptop sleep ends it too**: a `--slcan` command running across a sleep gets no frames after it. Run it again.
- **`slcan` image**: flashed instead of `dash`, the board is only an adapter.
- **Power**: OBD pin 1, so the board is on only with the ignition.
- **`dashsim`**: shows the board's screen in a terminal over USB, until the OLED is fitted.
- **Replay without the board**: `vagcan dev recording dash <VIN> --log drive.csv` plays a `watch --out` recording on the panel in the terminal, alarms included, and prints each alarm event. `--press 12.5` presses the button at 12.5 s. Piped, it prints the events only.

## Roadmap

Updated 2026-09-26.

**Done**
- [x] Read the car: identity, units, faults, OBD-II sensors, live values, acceleration timing with html-report
- [x] ODIS project as the main data source, with fault text from ODIS
- [x] `setup` in about 4 s on an ODIS project
- [x] Dash reads the car: 13 channels from 2 units, values and chart pages; an alarm takes the screen and blinks the cell
- [x] ESP32 board as a CAN adapter (`slcan`), tested on the bench
- [x] UDS over BLE: read the car from a laptop without a cable (info, faults, watch and units on the car; measure started, a full run waits for the road)
- [x] Replay a recorded drive on the dash panel, without the car
- [ ] Laptop reads the car through the dash while its screen keeps working (BLE passed on the car; the cable waits)
- [ ] Dash shows how far a channel is from what its control unit asked for, with a drift alarm (built, waiting for the car)
- [ ] OLED on the board, and an enclosure with snap-in boards (waiting for the display)
- [ ] Page the dash panel with the cruise-control buttons while cruise is off, LIMIT for the stopwatch (probed on the car)
- [ ] `vagcan faults` on the car with fault text from ODIS only
- [ ] 0–60 and 0–100 km/h stopwatch on the dash (with the buttons above)

Details: [`todo/README.md`](todo/README.md).

## Tested on

| Car | Platform | Adapter |
|---|---|---|
| Škoda Octavia III FL, 2017 | MQB | MKS CANable V2.0 Pro (slcan) |
| Škoda Octavia III FL, 2017 | MQB | ESP32-C3 SuperMini + SN65HVD230 CAN transceiver |

## Hardware

This tool does **not** work over VCDS's `HEX-V2`, a `VNCI`, or any other diagnostic interface you may already have. Those do not pass CAN frames through as plain data, the way slcan adapters do.

**Supported adapters**
- `MKS CANable V2.0 Pro`: works out of the box. Other CANable boards will probably work too.
- `ESP32-C3 SuperMini + SN65HVD230 CAN transceiver`: needs soldering and the firmware from this repository.

**Remove the 120 Ω termination jumper** if your adapter has one. The car's diagnostic port is already terminated.

**OBD-II wiring**

| OBD-II pin | Adapter |
|---|---|
| 6 | CAN-H |
| 14 | CAN-L |
| 5 or 4 | GND |
| 16 or 1 | +12 V, only for standalone boards such as the dash |

## Install

With cargo:

```sh
cargo install --git https://github.com/pashokitsme/vagcan vag-cli
```

From a local clone:

```sh
git clone https://github.com/pashokitsme/vagcan
cd vagcan
cargo install --path crates/cli/vag-cli
```

Run it with no arguments to see what is connected and what to do next:

```sh
vagcan
```

Check that it found your adapter:

```
$ vagcan devices
/dev/cu.usbmodem206E37A148451  CANable 2.0 (slcan)
```

If nothing is listed while the adapter is plugged in, unplug it and plug it back in. It can appear on USB without the system creating a serial port for it.

## Setting up

```sh
vagcan setup                    # pick a source from a menu
vagcan setup ~/Downloads/SK37X  # or give the path directly
```

`setup` reads one of two sources:

- **ODIS-Service project**, the better one. It describes every channel of every control-unit variant: where the value is in the reply and how to scale it.
- **VCDS installation**, the fallback, for cars no ODIS project covers. It has names and fault text but no scalings.

The first menu entry reads both: structure from ODIS, wording from VCDS.

The result is saved as a **project** in `~/.vagcan/data/<project id>/`. A project is a **platform, not a single car**: VW files every Octavia III, Karoq and Kodiaq under `SK37X`. Data about one specific car is saved in `~/.vagcan/cars/<VIN>/`. See [which cars each ODIS project covers](docs/odis-project-mapping.md).

**VCDS is Ross-Tech's software.** It is free from <https://www.ross-tech.com/vcds/download/>. `vagcan setup` can download and unpack an unmodified copy for you. The data is read once and nothing from it is built into the tool.

## Reading the car

```sh
vagcan info               # VIN, engine, gearbox
vagcan units --identify   # every control unit the gateway knows about
vagcan faults             # stored fault codes with their text (after setup)
vagcan watch              # live values from several units at once
```

**Through the dash board over BLE:** no cable needed. Add `--ble`: `vagcan` finds the board and says which one it uses, or asks when there are several. The ignition must be on. On macOS, the app you run it from needs Bluetooth access (System Settings → Privacy & Security → Bluetooth); Terminal.app asks on first use.

| `--device` | What it uses |
|---|---|
| omitted | the one USB-CAN adapter or dash board on USB |
| a serial path | that adapter, or that dash board on USB |
| `ble` | the board over BLE; asks which when there are several |
| `ble:<name>` | the board with that name, without asking |

`--ble` is short for `--device ble`.

Add `--slcan` to use a dash board on USB as a plain adapter (`vagcan --slcan dev sniff`).

```sh
vagcan faults --ble
```

**No car or adapter yet?** These work offline: `vagcan setup`, `vagcan dev vcds names <text>` to search VW's measurement names, and `vagcan dev recording …` to read a recorded drive.

## Where files are stored

Everything is in `~/.vagcan/`. Nothing is written to the checkout.

```
~/.vagcan/
  config.toml           settings: current project, name language, fault text language, favourites
  names.csv             your own channel names (vagcan dev glossary)
  rod/                  VCDS .rod files and fault text, shared by all projects
  data/<project id>/    one directory per platform, e.g. SK37X
    cache.sqlite          channels, scalings, fault text, label rows
    names.json            text id → name, from VCDS
    names-odis.json       text id → name, from ODIS
    rod-keys.json         recovered .rod section keys
    sources.json          which ODIS project or VCDS installation was read
    measurements/         scalings proven on a car, one file per part number
  cars/<VIN>/           data about one car
    car.json              mass, tyre, measured road load
    survey.jsonl          what the car answered on the last survey
    measures/             saved acceleration runs
  dash/<VIN>/           the dash for one car
    dash.toml             pages and channels, written by hand
    plan.json, plan.rs    the resolved plan, written by vagcan dev dash build
```

- **Safe to delete:** `rod/` and everything in `data/<project id>/` except `measurements/`. `vagcan setup` rebuilds them.
