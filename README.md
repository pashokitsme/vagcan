# vagcan

> **Under development, not battle-tested.** Commands and formats may change. Readings may be wrong. Reading a car can still make a control unit log a fault.

A command-line diagnostics tool for VW, Audi, Škoda and SEAT cars, written in Rust.

It talks to the car through a plain CAN adapter: an `MKS CANable V2.0 Pro`, or an `ESP32` with the firmware from this repository.

The tool is not designed for write operations: coding, adaptations, clearing faults and especially flashing are **not supported** (at the moment, at least).

## Features

### Reading the car

| Command | What it does |
|---|---|
| `vagcan` | Shows where you stand: adapter, car data, what to type next |
| `vagcan setup` | Reads an ODIS project or a VCDS installation, offline, in seconds |
| `vagcan setup --list` | The sources on this machine: the VCDS installation, then each ODIS project's |
| `vagcan setup --drop <SOURCE>` | Removes one source and everything it wrote. `SOURCE` is `odis`, `vcds` or an id from `--list` |
| `vagcan devices` | Lists USB-CAN adapters and dash boards on USB |
| `vagcan info` | VIN, and the engine's and gearbox's part numbers and software versions |
| `vagcan units` | The car's control units, named from your label files and earlier `--identify` runs. `--identify` makes each one name itself |
| `vagcan adaptations <unit>` | One unit's adaptation channels and their current values, read only. No unit: pick one. `--all`: every unit that has them. Refused on a moving car; needs a cable adapter |
| `vagcan faults` | Stored fault codes with VW's own text, in English where the run's source has it. `[faults] language = "de"` (or `en`, `ru`, …) in the config picks another of its languages. Under each code, its freeze frame: priority, aging counter and the unit's own conditions (engine speed, voltage, temperatures, …) from the ODIS project or the VCDS install; a code the source has no text for shows as its number, bytes nothing describes are shown raw |
| `vagcan sensors` | Standard OBD-II readings. `--unit <unit>` outside 7E0–7E7: parked car and a cable adapter only |
| `vagcan watch` | Live values from several units at once, with a chart of up to 6 channels. States show by name. `--out` records to CSV |
| `vagcan measure` | Acceleration run timing from the car's own speed signal. `measure view` opens a saved run as a chart |
| `--source odis\|vcds` | On the car commands and `dev dash build`/`edit`: which source names everything, for one run. One source per run, never mixed. Default: the ODIS project when there is one, else the VCDS installation |
| `-v`, `-vv` | On any command. `-v`: where the data came from, why a row is unnamed or skipped, caveats, timings. `-vv`: also every request and answer on the bus. Data is on stdout, the rest on stderr |

### Development commands – `vagcan dev`

| Command | What it does |
|---|---|
| `dev sniff` | Records the bus. Listen-only by default. Reports dropped frames if the adapter can tell |
| `dev sim` | A virtual car on a pseudo-terminal: every car command runs against it as through a USB-CAN adapter. `--from` writes its profile from a recording, a capture or a `units.json`. See [docs/virtual-car.md](docs/virtual-car.md) |
| `dev glossary` | Your own names for channels, in `~/.vagcan/names.csv` |
| `dev recording` | Works on recorded drives: `discover` finds gear and mode channels, `dash` plays one on the dash panel |
| `dev vcds` | Works on VCDS files: label lookup, name search, `.rod` decryption, log analysis |
| `dev dash build` | Builds the dash configuration for one car, offline |
| `dev dash edit` | Lays out the dash's pages in a browser, drawn by the board's renderer, from your car's channels. Save writes them into `dash.toml` once the build accepts them |

### Where names and numbers come from

- **ODIS-Service project** (the default when there is one): channels, byte layout, scaling and fault text for every control-unit variant.
- **VCDS installation** (with no ODIS project, or `--source vcds`): channel names, fault text, and the channels of the units of every car `watch`, `measure` or `units --identify` has run with.
- **Your own drives**: a scaling measured on a car overrides either.
- A run reads one of the two sources, never both mixed: `--source odis|vcds` picks it.

Everything lives under `~/.vagcan/`. Nothing about any car is built into the tool.

### Dash Display (implemented, assembled irl & tested)

An ESP32-C3 board on the OBD port that shows live values on a 3.12″ 256×64 OLED. To make one for your car, see [Building the dash](#building-the-dash).

- **Values page**: up to 4 cells, each with a label, a value and a unit.
- **Chart page**: one channel shown large, with its recent history on a fixed scale.
- **Status strip**: a line across the top of a page: its `title` in the middle, the battery voltage on the left (`14.2V`), the USB and BLE icons and the fault badge on the right.
- **Strip voltage**: from the channel `[strip]` names. Off on a page with `volts = false`.
- **No voltage twice**: a page that shows the voltage channel as a cell has none in its strip.
- **Every page has the strip**, the stopwatch's too. A host connecting or the fault count arriving changes the strip, never the page under it.
- **Pages** are set per car in `dash.toml` and switched with buttons on the board's pins (`[[button]]`) or with the cruise lever while cruise is off (`[stalk]`).
- **No input**: with no `[stalk]` and no `next`/`previous` `[[button]]`, only `dashsim` or `dashcfg`'s `set page` turns the page. With no `[stalk]` and no `[[button]]` at all, only `dashsim` silences an alarm; otherwise it stays up until its channel answers in range again. The build says so.
- **Alarms**: a `[[channel]]` in `dash.toml` may carry `alarm_max` and/or `alarm_min`, each with an optional off value (`alarm_max_off`, `alarm_min_off`; 5 % inside by default), or a drift alarm against its `setpoint` (`drift_percent`, `drift_off_percent`, `drift_hold_ms`, `drift_min_setpoint`). The board watches every such channel on every page. Past the bound, it shows the first values page with the channel (or its chart, its header inverted), every channel out on it inverted: blinking while the value is out, steady for the 2.5 s the page stays up after it is back at its off value. A press silences them until the value comes back. `alarm_priority` orders them (1 first); without it, the order in the file. The build's `bus:` lines estimate how busy the bus is with each page on screen; they refuse nothing.
- **Specified values**: a channel paired with `setpoint` shows the difference from what its control unit asked for on a line of its own, under the number. [`docs/dash/dash-toml.md`](docs/dash/dash-toml.md) is the whole file format.
- **Fault count**: once per start, 10 s in, the board counts the stored fault codes of the engine, the gearbox, the gateway and every unit the gateway lists. A listed id that shares a CAN id with a unit already asked is skipped, so the total can be lower than `vagcan faults`'.
- **Fault badge**: a warning triangle and the number, in the strip left of the link icons, on every page. Inverted while a code is failing now. Hidden at 0.
- **`?` on the badge**: no count — the gateway gave no list, the walk would pass 64 units, or no unit could be counted. The board's USB log says which; `dashcfg` shows `?`.
- **`dashcfg`** shows the count as stored, failing now and how many units it is of: `9 stored, 1 failing now — 17 of 18 units counted`.
- **The fault count waits** while the stopwatch is up, and while a laptop runs `vagcan measure` through the board. It goes on where it stopped.
- **No invented numbers.** A channel that does not answer shows dashes.
- **Part-number check**: the board polls a unit only if the part number the unit reports is the one the dash was built for.
- **BLE**: `dashcfg` sets brightness and the active page. Settings are stored on the board. BLE is always on: no button, no pairing.
- **Settings survive a new plan**: reflashed with a changed `dash.toml`, the board keeps brightness, the page and the last run. A page the new plan does not have becomes the first.
- **Plan id**: `vagcan dev dash build` prints the plan's id. `vagcan devices` and `dashcfg` show the id of the plan the board runs.
- **UDS over the USB cable**: on the `dash` image, `vagcan` reads the car through the board like through a CANable, and the panel keeps working. `vagcan devices` lists it as `dash image`.
- **UDS over BLE**: the same with no cable, slower.
- **Sweeps need a plain adapter**: through the board `units --identify <unit>` is refused, and `dev sniff` needs `--slcan`.
- **`--slcan`**: `vagcan --slcan …` makes the `dash` image a plain slcan adapter for that run, with no reflash. The panel shows `SLCAN`, the bit rate and frame counters. It ends when that run ends.
- **Laptop sleep ends it too**: a `--slcan` command running across a sleep gets no frames after it. Run it again.
- **`slcan` image**: flashed instead of `dash`, the board is only an adapter.
- **Power**: OBD pin 1, so the board is on only with the ignition.
- **OLED**: an SSD1322 module (3.12″, 256×64) on the board's SPI pins. Brightness 0–255, set with `dashcfg`.
- **`dashsim`**: shows the board's screen in a terminal over USB, with or without the OLED.
- **Replay without the board**: `vagcan dev recording dash <VIN> --log drive.csv` plays a `watch --out` recording on the panel in the terminal, alarms included, and prints each alarm event. `--press 12.5` turns the page at 12.5 s, as `dashsim`'s press does. Piped, it prints the events only.

## Roadmap

Updated 2026-10-22 (`dev dash edit` saves `dash.toml` itself).

**Done**
- [x] Read the car: identity, units, faults, OBD-II sensors, live values, acceleration timing with html-report
- [x] ODIS project as the main data source, with fault text from ODIS
- [x] `setup` in about 4 s on an ODIS project
- [x] `setup` in about 3 s cold on a VCDS installation, was about 5 min (845 s measured on a loaded machine) (2026-10-20): the `.rod` keys come from the file names, and the steps run on every core
- [x] Dash reads the car: 13 channels from 2 units, values and chart pages; an alarm takes the screen and blinks the cell
- [x] ESP32 board as a CAN adapter (`slcan`), tested on the bench
- [x] UDS over BLE: read the car from a laptop without a cable (info, faults, watch and units on the car; measure started, a full run waits for the road)
- [x] Replay a recorded drive on the dash panel, without the car
- [x] `dev survey` removed (2026-09-28): the car's units are recorded by `watch`, `measure` and `units --identify`, and those three read a VCDS installation's channels for them the first time; `dev dash build` reads them offline
- [x] Laptop reads the car through the dash while its screen keeps working (BLE passed on the car; the cable waits)
- [x] Dash shows how far a channel is from what its control unit asked for, with a drift alarm
- [x] OLED on the board: the dash draws on its own 3.12″ panel, in the car since 2026-09-30
- [x] Page the dash with the cruise-control buttons while cruise is off; LIMIT opens the stopwatch (on the car, 2026-09-30)
- [x] `vagcan faults` on the car with fault text from ODIS only
- [x] Scalings and names from a VCDS install alone, without an ODIS project (built 2026-09-28; checked offline on the reference car, waits for a drive)
- [x] 0–60 and 0–100 km/h stopwatch on the dash (works on the car, 2026-09-30; not yet compared with `vagcan measure`)
- [x] Dash counts the car's stored fault codes once after start and shows the number with a warning triangle
- [x] A project's sources listed and removed: `setup --list`, `setup --drop` (2026-10-20)
- [x] Units with no label file of their own, such as the body control module, read from a VCDS install through its `ReDir.rod` (2026-10-20)
- [x] `measure view` draws the speed in km/h, on old sessions too; it was 3.6 times low (2026-10-21)
- [x] A virtual car, `dev sim`, and e2e cases of the commands against it: `just e2e` (2026-10-21)
- [x] One VCDS installation per machine, for every car, in `~/.vagcan/vcds/`; projects are ODIS's alone; an older layout moves on its own (2026-10-21)
- [x] `setup` and `faults` print in the shape of established tools: steps as lines under a heading, faults as a block per unit (2026-10-21)
- [x] ODIS and VCDS independent: one source per run, never mixed; `--source`, `VAGCAN_SOURCE` or `[source] use` picks it (2026-10-21)
- [x] `dev dash edit` saves `dash.toml` itself, checked by the dash build first; the rest of the file stays as written (2026-10-22)
- [ ] Adaptation channels and their values, read only: `vagcan adaptations <unit>`, `--all` (built 2026-10-21, waiting for the car)
- [ ] Freeze frames under each stored code in `vagcan faults`, from an ODIS project or a VCDS install alone (built 2026-10-21, checked offline on one recorded answer; waits for the car)
- [ ] Page the dash with buttons on its pins
- [ ] Dash status strip: the page's title, the battery voltage, the link icons and the fault badge across the top (built 2026-10-19, waiting for the car)
- [ ] Alarms as keys of a channel: a maximum, a minimum and an off value for each, any number of them, an explicit priority; `[[alarm]]` removed (built 2026-10-19, waiting for the car)
- [ ] Electrical consumption in watts in the dash's strip, beside the voltage (not started: needs derived channels, and the currents checked on the car)

Details: [`todo/README.md`](todo/README.md).

## Requirements

- **Rust stable**, edition 2024.
- **An slcan USB-CAN adapter.** Developed and tested mainly on `MKS CANable V2.0 Pro`. No driver is needed: the adapter shows up as a serial port.
- **espflash**, to flash the dash board: `cargo install espflash --locked` (tested with 4.1.0).
- **Diagnostic data**, for names and numbers instead of raw bytes: the **ODIS-Service project** for your car's platform, or a **VCDS installation**. See [Setting up](#setting-up). `vagcan setup` offers to download VCDS if you have neither.

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

**OLED wiring** (dash only; a 16-pin SSD1322 module set to 4-wire SPI)

| Module pin | ESP32-C3 |
|---|---|
| 1 GND | `G` |
| 2 VCC | supply, per the module |
| 4 SCLK | `GPIO10` |
| 5 SDIN | `GPIO8` (the board's blue LED; it flickers with the panel's data) |
| 14 D/C | `GPIO0` |
| 15 RES | `GPIO21` |
| 16 CS | `GPIO20` |

Module pins 3 and 6 stay free; 7–13 go to GND.

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

In a clone, [`just`](https://github.com/casey/just) runs the common tasks. `just` alone lists them:

| command | does |
|---|---|
| `just install` | installs `vagcan` |
| `just vagcan …` | runs `vagcan` from the checkout |
| `just plan <VIN>` | builds the dash configuration for a car, as `vagcan dev dash build <VIN>` |
| `just fw-flash` | builds the dash firmware and flashes the board on USB |
| `just fw-flash dash --no-default-features` | the same without BLE, for a board whose BLE does not start |
| `just fw-flash oledtest` | the test picture on the OLED |
| `just fw-run` | flashes and stays on the board's console |
| `just fw-monitor` | the board's console |
| `just fw-info` | the board's chip revision and MAC |
| `just dashcfg` | the board's settings over BLE, from Terminal.app |
| `just dashsim` | the board's screen in this terminal, over USB |
| `just check` | everything CI checks |

The stopwatch is `vagcan measure`. A `vagcan-measure` an earlier install left there is removed by `cargo uninstall vag-cli-measure`.

`just fw-flash` finds the board by its USB id. With no board found, or several, set the port:

```sh
ESP_PORT=/dev/cu.usbmodem1101 just fw-flash
```

Run `vagcan` with no arguments to see what is connected and what to do next:

```sh
vagcan
```

Check that it found your adapter:

```
$ vagcan devices
path                           adapter  device
/dev/cu.usbmodem206E37A148451  yes      CANable 2.0 (slcan)
```

If nothing is listed while the adapter is plugged in, unplug it and plug it back in. It can appear on USB without the system creating a serial port for it.

## Setting up

```sh
vagcan setup                    # pick a source from a menu
vagcan setup ~/Downloads/SK37X  # or give the path directly
```

`setup` reads one of two sources, once, offline. The car is not needed.

### An ODIS-Service project (preferred)

ODIS-Service is VW's own diagnostic software for dealers. vagcan does not include its data and does not download it: you need your own copy.

- Its data comes as one **project per platform**, named like `SK37X`. `SK37X` covers every Octavia III, Karoq and Kodiaq.
- A project describes every channel of every control-unit variant: where the value is in the reply and how to scale it. It has the fault text too.
- A project alone is enough: `info`, `units`, `faults`, `watch`, `measure` and the dash all work from it.
- It reads in seconds.

1. Find your car's project in [the table of VW's projects](docs/odis-project-mapping.md).
2. Extract that project's folder. It is the folder that holds `AStringData.data.gz` and the `.key` files.
3. Read it:

   ```sh
   vagcan setup ~/Downloads/SK37X
   ```

### A VCDS installation

For cars no ODIS project covers.

- It has names and fault text, and the channels of the units of the cars `watch`, `measure` or `units --identify` records.
- About 40 % of its unit files are locked to VCDS itself: a run that reads VCDS shows those units raw. An ODIS project describes them, in a run of its own.
- It reads in about 3 s cold. The keys of VCDS's encrypted files come from their file names.
- **A Russian-only VCDS gives fault text, but no measurement names.** `watch` then shows its channels by reading id. Read an English VCDS instead: `vagcan setup` with no path, then pick "Download VCDS". A second VCDS installation replaces the first.
- **One installation serves every car.** It is saved once, in `~/.vagcan/vcds/`, and `setup` asks no name for it.

With VCDS alone, the channels come the first time `vagcan watch`, `vagcan measure` or
`vagcan units --identify` runs with the car: it records which control units the car has and
reads their channels from the installation. `vagcan dev dash build` does the same offline, for a
car already recorded. Keep the installation: a unit the tool meets later — swapped, updated, or
asleep the first time — is read from it.

```sh
vagcan setup ~/vcds    # names and fault text
vagcan watch           # with the car: records its units, reads their channels once
```

**VCDS is Ross-Tech's software.** It is free from <https://www.ross-tech.com/vcds/download/>. `vagcan setup` can download and unpack an unmodified copy for you. The data is read from your copy, and nothing from it is built into the tool.

### Both

The first menu entry of `vagcan setup` reads both. They are independent sources: a run reads **one** of them for everything — channels, scalings, fault text, freeze frames, adaptations. Nothing of one fills a gap in the other; what the run's source does not describe is shown raw or as its number.

- An ODIS project is saved as a **project** in `~/.vagcan/data/<project id>/`, one per platform. `setup` asks what to call it when its folder name will not do.
- A VCDS installation is saved in `~/.vagcan/vcds/`, one per machine, for every car.
- A car command reads the car's ODIS project when there is one, otherwise the VCDS installation.
- Data about one specific car is saved in `~/.vagcan/cars/<VIN>/`.

Pick the other source:

| For | Type |
|---|---|
| one command | `vagcan --source vcds watch` |
| a shell, or the firmware build | `export VAGCAN_SOURCE=vcds` |
| every run | `[source] use = "vcds"` in `~/.vagcan/config.toml` |

The flag wins over `VAGCAN_SOURCE`, which wins over `config.toml`. A source asked for that is not set up is an error, never answered from the other one.

What `setup` prints, on stderr:

```
Read the VCDS installation at ~/vcds-en
  copied 16577 files, all up to date
  parsed 3035 label files
  read 192469 measurement names, 42640 with an IDE or MAS id
  saved no .rod keys: every section opens by its file name
  read 6085 channels for 15 of 15 units
Ready: fault text and scalings for every car.
  (use "vagcan faults" with the car connected)
```

`-v` adds each step's time and path under it.

### Listing and removing sources

| Command | What it does |
|---|---|
| `vagcan setup --list` | One row per source, the VCDS installation first: id, store (`vcds` or the project), kind, state, language, when read, channels, path. `-v` adds each cache, the version and the other counts |
| `vagcan setup --drop vcds` | Removes the machine's VCDS installation, its rows and its log. Asks first |
| `vagcan setup --drop odis` | Removes the current project's ODIS source and its rows. Asks first |
| `vagcan setup --drop 3` | The same, by id from `--list`. An id the VCDS store and the project both have is refused: say `vcds` or `odis` |
| `vagcan setup --drop 3 --yes` | Does not ask. Required without a terminal |

- The list of sources is the `source` table in each `cache.sqlite`. Nothing else records it.
- A VCDS installation an older vagcan read into a project moves to `~/.vagcan/vcds/` the first time a command runs. If two projects held one, the one read last is kept, and a note names the other.
- After a drop, every command uses what the other sources wrote.
- `stale` in `--list`: a relative path written by an older vagcan. Nothing resolves it; drop it by id.
- `gone`: the folder is not on this machine any more. Its rows still work.
- Reading the same folder again replaces its rows. It never adds a second source.
- A second VCDS installation replaces the first.

## Reading the car

```sh
vagcan info               # VIN, engine, gearbox
vagcan units --identify   # every control unit on the car's list, each naming itself
vagcan faults             # stored fault codes with their text (after setup)
vagcan adaptations 09     # one unit's adaptation channels and their values (parked)
vagcan watch              # live values from several units at once
```

A unit is named as `vagcan units` lists it: its number or its request id (`vagcan faults --unit 01,713`, `vagcan watch --did 713:IDE00001`).

**Through the dash board over BLE:** no cable needed. Add `--ble`: `vagcan` finds the board and says which one it uses, or asks when there are several. The ignition must be on. On macOS:

- The terminal app needs Bluetooth permission: System Settings → Privacy & Security → Bluetooth.
- A terminal inside another app (an IDE, an agent) may be stopped without a message. Run `vagcan --ble` and `dashcfg` from Terminal.app.

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

**No car or adapter yet?** These work offline: `vagcan setup`, `vagcan dev vcds names <text>` to search the measurement names `setup` read (from VCDS, ODIS or both), and `vagcan dev recording …` to read a recorded drive.

## Building the dash

The dash is built for one car: what it reads is fixed when the firmware is built. Do this in a clone of the repository, after [Setting up](#setting-up).

**1. Read the car once.** With the car connected, run one of these. It records the car's control units in `~/.vagcan/cars/<VIN>/units.json`, which the dash build needs.

```sh
vagcan watch
vagcan units --identify
```

Without this record, the build stops with `no record of <VIN>'s control units`.

**2. Write `~/.vagcan/dash/<VIN>/dash.toml`.** The smallest one: one value on one page.

```toml
vin = "<VIN>"

[[channel]]
ref = "01:IDE00025"    # coolant temperature: unit 01, channel id IDE00025

[[page]]
kind = "values"
cells = ["01:IDE00025"]
```

To find your car's channel ids: run `vagcan watch`, press `,`, set "Key at the end of each row" to shown. The Unit column is the control unit, the Key column is the channel id. Every key of the file: [`docs/dash/dash-toml.md`](docs/dash/dash-toml.md).

To lay out the pages in a browser instead: `vagcan dev dash edit <VIN>`. It opens on your `dash.toml`, or on no pages when there is none, and offers only channels the build accepts.

- **Save** (Cmd-S or Ctrl-S) builds the plan from the new pages first. A refused build writes nothing; the page shows why.
- A build that passes writes `dash.toml`, `plan.json` and `plan.rs`. It replaces the `[[page]]` tables, adds a `[[channel]]` for a new channel and changes labels. Comments, alarms and every other line stay as written.
- A `dash.toml` changed on disk since the page opened is not overwritten: reload the page.
- It needs a `vagcan` built after `rustup target add wasm32-unknown-unknown`.

**3. Check it.**

```sh
vagcan dev dash build <VIN>
```

It prints one line per channel, or what it refused and why. Fix the file until it builds. With VCDS alone, the first run also reads the channels of the car's units from the installation.

**4. Flash the board.** Plug the board into USB, then:

```sh
cargo install espflash --locked   # once
just fw-flash
```

| when | run |
|---|---|
| several cars under `~/.vagcan/dash/` | `VAGCAN_DASH_VIN=<VIN> just fw-flash` |
| the board's BLE does not start | `just fw-flash dash --no-default-features` |
| no board found, or several | `ESP_PORT=/dev/cu.usbmodem1101 just fw-flash` |

**5. Set the brightness.** With the board powered, over BLE:

```sh
just dashcfg
```

At the `dash>` prompt:

```
set brightness 128
save
q
```

`save` keeps the setting after power-off. On macOS, run it from Terminal.app: it needs Bluetooth access.

| `dashcfg`, in a clone | connects to |
|---|---|
| `just dashcfg` | the board named `vagcan-dash` |
| `just dashcfg <name>` | the board with that Bluetooth name |
| `just dashcfg --help` | nothing; lists the commands you can type once connected |

## Where files are stored

Everything is in `~/.vagcan/`. Nothing is written to the checkout.

```
~/.vagcan/
  config.toml           settings: current project, source, name language, fault text language, favourites
  names.csv             your own channel names (vagcan dev glossary)
  rod/                  VCDS .rod files and fault text
  vcds/                 the VCDS installation, one per machine, for every car
    cache.sqlite          label rows, channels and scalings from VCDS, and its source (vagcan setup --list)
    names.json            measurement names
    odx-ids.json          which VW channel id each name is for
    rod-keys.json         recovered .rod section keys
    registry.json         which installation the channels were read from, and what came of each unit
  data/<project id>/    one ODIS project per platform, e.g. SK37X
    cache.sqlite          channels, scalings, fault text, and its source (vagcan setup --list)
    names-odis.json       measurement names, from ODIS
    measurements/         scalings measured on a car, one file per part number
  cars/<VIN>/           data about one car
    car.json              mass, tyre, measured road load
    units.json            the car's control units, as each said what it is
    measures/             saved acceleration runs
  dash/<VIN>/           the dash for one car
    dash.toml             pages and channels, written by hand
    plan.json, plan.rs    the dash configuration the firmware is built from, written by vagcan dev dash build
    editor/               what vagcan dev dash edit serves; its next run rebuilds it
```

- **Safe to delete:** `rod/`, `vcds/`, and everything in `data/<project id>/` except `measurements/`. `vagcan setup` rebuilds them.
- **Another location:** `VAGCAN_HOME=/absolute/path` is used instead of `~/.vagcan`.
- **Tests never use `~/.vagcan`.** A test build's is a temporary directory.
