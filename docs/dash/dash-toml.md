# `dash.toml` — reference

`dash.toml` says what the dash board reads and shows: channels and their alarms, pages, the cruise lever,
the stopwatch and buttons on the board's pins. One file per car, written by hand:

```
~/.vagcan/dash/<VIN>/dash.toml
```

The build resolves every name in it against the car's recorded units (`~/.vagcan/cars/<VIN>/units.json`,
written by `vagcan watch`, `vagcan measure` or `vagcan units --identify` with the car) and the project's
label data, and writes the dash configuration the firmware is built from. The board resolves nothing
itself.

The steps from a new car to a flashed board are in the README: [Building the dash](../../README.md#building-the-dash).

## Build it

| what you want | run |
|---|---|
| see the dash configuration and the reasons, no firmware | `vagcan dev dash build <VIN>` |
| build and flash the board, in a clone | `just fw-flash` (needs `cargo install espflash --locked` once) |
| build the firmware for that car | `VAGCAN_DASH_VIN=<VIN> cargo build --release --bin dash` in `crates/dash/vag-dash-fw` |
| check the firmware compiles, no car | `VAGCAN_DASH_NO_CAR=1 cargo build --release --bin dash` (an empty configuration — do not flash it) |

Both run the same build, from one source: the ODIS project by default, the VCDS installation
with `vagcan --source vcds dev dash build <VIN>` — and for the firmware, `VAGCAN_SOURCE=vcds`,
or `[source] use = "vcds"` in `~/.vagcan/config.toml` for both. A channel that source does not
describe is refused, never taken from the other; the refusal says which source the build read
and the flag that builds from the other.

`vagcan dev dash build` is for reading the result — and, when the build reads the VCDS
installation, it is a step before the firmware build only when no command with the car has read
the channels of the units the configuration uses yet: it reads them from the installation (offline,
minutes the first time), which the firmware build never does. A firmware build that uses a unit
whose channels have not been read is refused; the refusal names `vagcan dev dash build
<VIN>`, or `vagcan setup <VCDS installation>` when the installation is no longer where `setup`
read it. A recorded unit the configuration does not use is a note in the build's output, never a refusal.

| setting | for | what it does |
|---|---|---|
| `VAGCAN_DASH_VIN=<VIN>` | the firmware build | Picks the car. With exactly one car under `~/.vagcan/dash/`, it can be left unset. `vagcan dev dash build` takes the VIN as its argument instead. |
| `VAGCAN_PROJECT=<id>` | both | Picks the project whose label data the names resolve against. |
| `vagcan --project <id> dev dash build <VIN>` | `vagcan dev dash build` | The same as `VAGCAN_PROJECT`, for one run. |
| `--input <FILE>` | `vagcan dev dash build` | Builds from another file. The outputs still land under the car. |

Outputs, both under `~/.vagcan/dash/<VIN>/`:

| file | for |
|---|---|
| `plan.json` | reading |
| `plan.rs` | the firmware, which `include!`s it |

Both carry the plan's id, and `vagcan dev dash build` prints it last. A board flashed with the
plan shows the same id in `vagcan devices` and `dashcfg`. Flashing a changed plan keeps the
board's settings.

### Reading the build's output

One line per channel:

```
COOLANT ← 01:IDE00025 F405@0/8 u BE ×1 -40 at 2 Hz, from the project's data (IDE00025)
```

| part | means |
|---|---|
| `COOLANT` | the label on the panel |
| `01:IDE00025` | the channel as `dash.toml` names it |
| `F405` | the identifier the board reads |
| `@0/8` | the value starts at bit 0 of the answer and is 8 bits long; ` bits` follows when the length is not whole bytes |
| `u` / `i` | unsigned / signed |
| `BE` / `LE` | big-endian / little-endian |
| `×1 -40` | the scaling: raw value × 1, then − 40 |
| `at 2 Hz` | readings a second while its page is up |
| `from the project's data` / `measured on a car` | the scaling comes from the project's label data (ODIS or VCDS) / was measured on a car |
| `(IDE00025)` | VW's channel id, or the row's name when it has none |

## A full example

Every section, every key. The identifiers, channel ids and state names are **one car's** (Škoda
Octavia III). Take yours from your car: see [How to find names for your car](#how-to-find-names-for-your-car).

```toml
vin = "XW8AD4NE9JH008917"
language = "ru"                    # labels (names.csv's ru column) and the board's own words

[[channel]]
ref = "01:IDE00025"                # coolant temperature, by channel id
label = "ОЖ"                       # the name on the panel
decimals = 0
hz = 10                            # readings a second while its page is up
alarm_max = 105                    # an alarm at 105 or above, on every page
alarm_max_off = 100                # off again at 100 or under

[[channel]]
ref = "01:IDE00191"                # boost pressure, actual
setpoint = "01:IDE00190"           # boost pressure, target value: same unit; no [[channel]] needed
decimals = 2
hz = 10
drift_percent = 10                 # an alarm 10 % or more from the target value…
drift_hold_ms = 1000               # …for a second
drift_off_percent = 6              # off again at 6 % or under
drift_min_setpoint = 0.5           # nothing said while the target value is under 0.5

[[channel]]
ref = "02:IDE00075"                # vehicle speed in km/h, the stopwatch's: on no page, read fast
hz = 50

[[channel]]
ref = "01:IDE00018"                # terminal 15 voltage, the strip's: on no page

[[page]]
kind = "values"
title = "MAIN"                     # in the strip
cells = ["01:IDE00025", "01:IDE00191"]   # the first values page with them: their alarms raise it

[[page]]
kind = "chart"
title = "ОЖ"                       # in the strip, instead of the chart header's label
cell = "01:IDE00025"
min = 70                           # fixed scale
max = 110
volts = false                      # no voltage in this page's strip

[strip]
volts = "01:IDE00018"              # the voltage in the strip over every page

[stalk]
read = "70C:1105"
rocker = "Linker Hebel axial 2 (GRA Set/Rest)"
switch = "Linker Hebel axial 3 (ON/CANCEL/OFF)"
next = "Blinker GRA beschleunigen"
previous = "Blinker GRA verzögern"
measure = "Blinker GRA neutral ohne Limiterverbau (weder beschleunigen noch verzoegern)"
switch_off = "Blinker GRA Aus"
cruise = "01:203C"
cruise_off = "main switch off"

[stopwatch]
speed = "02:IDE00075"              # the [[channel]] above, with its hz
km_h_per_unit = 1.0                # km/h already; 0 = not measured: the page shows НЕТ КОЭФ
marks = [60, 100]

[[button]]                         # a button from GPIO3 to GND
pin = 3
action = "next"

[[button]]
pin = 5
action = "stopwatch"
```

## Reference

**Every key is checked.** A key or section the build does not know is refused, with its line,
a near key when there is one, and the keys that table takes. So is an optional key of the wrong
type: `decimals = "2"` is refused, never read as absent.

- Top-level keys go above the first section. TOML gives a key to the section header above it.
- A key of the other `kind` is refused: `min` on a values page, `cells` on a chart.
- Strings are trimmed at both ends.
- Keywords are lowercase: `kind = "Values"` is refused.

Check the build's output after every edit. It prints one line per channel (with its rate) and per
target value, then one for the stopwatch, the lever and each button, then one per alarm, then
the bus lines (**Bus time**, under [What the board does with it](#what-the-board-does-with-it)).
Notes come in between, such as nothing turning the page.

### Top level

| key | type | required | default | what it does |
|---|---|---|---|---|
| `vin` | string | yes | — | The car. Must be the VIN the build is for (case does not matter), or the build stops. |
| `language` | `"en"` or `"ru"`, any case | no | `language` in `~/.vagcan/config.toml`, else `"en"` | The labels' language, and the words the board writes itself (the stopwatch page). |

- At least one `[[channel]]` and one `[[page]]` are required.
- A label comes from the channel's `label`. Without one, from `~/.vagcan/names.csv` in
  `language`, then from the project's wording.

### `[[channel]]`

One per value the board reads.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `ref` | string: a channel | yes | — | Which unit, which row. See [Naming a channel](#naming-a-channel). |
| `label` | string | no | `names.csv` in `language`, then the project's wording | The name on the panel. Ten characters fit a page of four cells; longer collides with the next cell. |
| `decimals` | integer, 0 to 3 | no | from the scaling: 0 for a step of 1 or more, one place per decade under 1, at most 3 | Places after the point. |
| `hz` | number, above 0, at most 50 | no | 2 | Readings a second while a page showing it is up. |
| `setpoint` | string: a channel on the same unit | no | none | The channel's target value, as its unit sets it. The panel draws the distance from target under the number; a drift alarm watches it. |
| `zero_at_rest` | `true` or `false` | no | `false` | Shows 0 while the car stands: while the stopwatch's `speed` reads 0. The speed is then read whenever this channel is, at the slower of its own `hz` and this channel's — at its own where a page shows it or the stopwatch is up. For an acceleration sensor, which reads the road's slope as acceleration at a standstill. Needs a `[stopwatch]`. Alarms and charts still see the value as read. |
| `alarm_max` | number | no | none | An alarm at this value or above. See [Alarms](#alarms). |
| `alarm_max_off` | number, under `alarm_max` | no | `alarm_max` less 5 % of its size, at least one step of the channel's resolution | The alarm goes off once the value is back at this or under. Needs `alarm_max`. |
| `alarm_min` | number | no | none | An alarm at this value or below. Either bound, or both. |
| `alarm_min_off` | number, over `alarm_min` | no | `alarm_min` plus 5 % of its size, at least one step of the channel's resolution | The alarm goes off once the value is back at this or above. Needs `alarm_min`. |
| `drift_percent` | number, above 0 | no | none | A drift alarm: the channel this far from its `setpoint`, in percent of the target value, or further… Needs a `setpoint`, and the three keys below. |
| `drift_hold_ms` | integer, 0 or more | with `drift_percent` | — | …for this many milliseconds without a break. |
| `drift_off_percent` | number, above 0, under `drift_percent` | with `drift_percent` | — | The alarm goes off once the drift is back at this share or under. |
| `drift_min_setpoint` | number, 0 or more | with `drift_percent` | — | While the target value is nearer 0 than this, either side, the drift alarm says nothing. In the channel's unit. |
| `alarm_priority` | integer, 1 or more | no | none | The alarm's priority, 1 first. Needs an alarm on the channel. See [Alarms](#alarms). |

- The row must scale linearly: a number. An enumeration, or a single point measured on a car with no
  slope, is refused.
- A channel on no page is not read, unless it is the stopwatch's `speed`, another channel's
  `setpoint`, or `[strip]`'s `volts`.

**`setpoint`**

- On the same control unit as the channel.
- It needs no `[[channel]]` of its own. The build adds it, read at the channel's `hz`.
- Without a `[[channel]]` of its own it is never shown: no page, strip or stopwatch may name it.
  Declare it under `[[channel]]` to show it.
- If it has a `[[channel]]` of its own, that one's `hz` must equal the channel's.
- Same unit of measure as the channel.
- It cannot be the channel itself, and cannot have a `setpoint` of its own.

### Alarms

An alarm is keys of the channel it watches: `alarm_max`, `alarm_min` and the `drift_*` keys
above. Example: an alarm at 125 or above, off at 120; and at 40 or below, off at 45.

```toml
[[channel]]
ref = "01:IDE00196"
alarm_max = 125
alarm_max_off = 120
alarm_min = 40
alarm_min_off = 45
```

- **The page.** An alarm raises the **first values page that shows the channel**, with the
  channel's cell inverted. A channel on no values page raises its chart page, with the chart's
  header inverted. A channel with an alarm must be on a page.
- **Priority:** channels with `alarm_priority` first, 1 highest; without it, the order in the
  file decides — for ties too. A channel's bounds come before its drift. With alarms out on two
  pages, the higher one's page is shown.
- **Several on one page:** every channel out on the page shown is inverted, each blinking while
  it is out and steady once it is back — also one whose own alarm raises another page. One press
  silences all of them.
- **Off values.** `alarm_max_off` is under `alarm_max`; `alarm_min_off` is over `alarm_min`. With
  both bounds, `alarm_min_off` must be under `alarm_max_off`. Left out, an off value is 5 % of
  the bound inside it, and never less than one step of the channel's resolution — what one count
  of its raw value is worth (`0.1` for a channel that scales by 0.1, whatever its `decimals`). The
  build's line for the alarm says which off values are defaults.
- Every number must fit a 32-bit float. They are compared as the board holds them.
- **The bus.** The board reads every alarm's channel at its `hz` on every page, and its
  `setpoint` with it, ahead of the page on screen. The build's `bus:` lines estimate what that
  costs on each page (**Bus time**, under [What the board does with it](#what-the-board-does-with-it)).

**Replacing `[[alarm]]`.** `[[alarm]]` tables are refused. The refusal lists the keys to add to
each channel, taken from the file's own rules:

| `[[alarm]]` | on each channel in its `channels` |
|---|---|
| `direction = "above"`, `trip = A`, `release = B` | `alarm_max = A`, `alarm_max_off = B` |
| `direction = "below"`, `trip = A`, `release = B` | `alarm_min = A`, `alarm_min_off = B` |
| `kind = "drift"`, `percent`, `release_percent`, `hold_ms`, `min_setpoint` | `drift_percent`, `drift_off_percent`, `drift_hold_ms`, `drift_min_setpoint`, the same numbers |
| `page` | nothing: the alarm raises the first values page with the channel, else its chart |

Where the old rules' order is not the channels' order in the file, each block also has the
`alarm_priority` that keeps it: no channel has to move.

### `[[page]]`

Pages come in the file's order. 1 to 8 of them. `kind` picks the page.

**`kind = "values"`** — up to four numbers side by side.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `kind` | `"values"` | yes | — | |
| `title` | string | no | `""` | Drawn in the strip over the page. |
| `cells` | list of channels | yes | — | 1 to 4 channels, one column each. Each must be under `[[channel]]`, in either spelling. |
| `volts` | `true` or `false` | no | `true` | Whether the strip shows the voltage on this page. Needs a `[strip]`. |

**`kind = "chart"`** — one channel, large, with its recent history.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `kind` | `"chart"` | yes | — | |
| `title` | string | no | `""` | Drawn in the strip over the page. The chart's header then leaves out the channel's label. |
| `cell` | string: a channel | yes | — | The channel, under `[[channel]]`. |
| `min` | number | yes | — | Bottom of the scale. |
| `max` | number | yes | — | Top of the scale. Above `min`. |
| `volts` | `true` or `false` | no | `true` | As on a values page. |

- The scale is fixed, never autoscaled.
- The board holds `min` and `max` as 32-bit floats: each must fit one, and `min` must stay below
  `max` in it.
- One chart per channel.
- An alarm raises a chart only for a channel no values page shows; the chart's header is
  inverted. A chart's `title` is for the strip.
- A key of the other kind (`min` on a values page, `cells` on a chart) is refused.

### `[strip]`

A line across the top of each page: the battery voltage on the left, the page's `title` in the
middle, the link icons and the fault badge on the right. The page is drawn in the rows under it.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `volts` | string: a channel | yes | — | The voltage, a `[[channel]]`. Shown as `14.2V`: one decimal, then `V`. `--V` while it has not answered. |

- Without `[strip]`, no page shows a voltage, and a page's `volts` is refused.
- A page whose cells, or chart, show the `volts` channel already has no voltage in its strip.
- Every page has the strip, the stopwatch's too. With no `title` and no voltage it shows only
  the icons and the badge.
- The page under the strip is the same whatever is connected or counted.
- The voltage is read at the channel's `hz` while its page is on screen, like a cell.
- The build lists the pages that show it, and says so when the channel's unit is not `V`.

### `[stalk]`

The cruise lever as buttons, while cruise control is off: one rocker state for the next page,
one for the previous, one to turn the stopwatch on and off.

**Every key but `read` and `cruise` is a text from your car's ODIS project, not an id.**

- `rocker` and `switch` are **field names**. A field is part of one identifier's answer. It has a
  name and its bits; in the project it usually has no channel id.
- `next`, `previous`, `measure`, `switch_off` and `cruise_off` are **state texts**. A state is a
  band of raw values with a text, and has no id at all.
- They are in the project's language: German for the steering column unit, English for the
  engine on the reference car.
- The build looks each text up in the project and takes the state's band of raw values from it.

Find them with `watch` on the identifier, `70C:1105` on the reference car:

```
vagcan watch --did 70C:1105
```

It lists every field of that identifier by name, with its current state's text. Work the lever
and note which state each position shows. A wrong text makes the build list the valid ones.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `read` | string: `<unit>:<DID>` | yes | — | The one identifier whose answer carries both the rocker and the switch. No channel id. No `@` but `@0`. |
| `rocker` | string: a field name from the project | yes | — | The field of `read` that the lever moves. |
| `switch` | string: a field name from the project | yes | — | The field of `read` with the cruise main switch. Not the rocker. |
| `next` | string: a state text of `rocker` | yes | — | Next page. |
| `previous` | string: a state text of `rocker` | yes | — | Previous page. |
| `measure` | string: a state text of `rocker` | yes | — | Stopwatch on and off. |
| `switch_off` | string: a state text of `switch` | yes | — | The switch's off state. |
| `cruise` | string: a channel | yes | — | The cruise control status, an enumerated field, on another identifier than `read`. Spelled as a channel. |
| `cruise_off` | string: a state text of `cruise` | yes | — | Its off state. |

- **Names are the project's, exactly and in full**, parentheses included. Spaces at either end
  do not count, on your side or the project's.
- `next`, `previous` and `measure` are three different states.
- Each state must be one band of raw values. A state name the project gives to several bands is
  refused.
- `cruise` cannot be in `read`'s identifier. The lever is the dash's only when two separate
  answers say cruise is off: `switch` reads `switch_off`, and `cruise` reads `cruise_off`.
- **A name that is valid but wrong builds and misbehaves.** `next` and `previous` swapped turn
  the pages backwards. An off state that is not off lets the lever turn pages while it also
  works cruise control. Check each state with `vagcan watch` before driving.
- `measure` on the reference car: LIMIT reads as `Blinker GRA neutral ohne Limiterverbau (…)`,
  despite the name. That state appeared only while LIMIT was pressed. Pick the state the
  position you press actually produces, as `vagcan watch` shows it, not the one whose name fits.
- The lever's read rates are fixed. `hz` does not apply.
- Without a `[stopwatch]`, `measure` only silences an alarm. The build says so.

### `[stopwatch]`

Times 0 to each mark, in km/h, on the board. Every key is required.

The `speed` channel must be under `[[channel]]` with an `hz`: without one it is read at 2 Hz and
refused. What the stopwatch adds to a file that already has `vin` and a `[[page]]`:

```toml
[[channel]]
ref = "02:IDE00075"
hz = 50

[stopwatch]
speed = "02:IDE00075"
km_h_per_unit = 1.0
marks = [60, 100]
```

| key | type | required | default | what it does |
|---|---|---|---|---|
| `speed` | string: a channel | yes | — | A `[[channel]]` with `hz` 7.5 or more (50 recommended). Its scaling has no offset and a factor above 0. |
| `km_h_per_unit` | number, 0 or more | yes | — | km/h per unit of `speed`. `0`: not measured — the page shows `NO FACTOR` (`НЕТ КОЭФ` with `language = "ru"`) and times nothing. |
| `marks` | list of integers | yes | — | 1 to 3 speeds in km/h, each above 0, each once. Shown in the order written; the run ends at the highest. |

- **The rate.** `speed` must be read every 133 ms or sooner: `hz` 7.5 or more.
- **`km_h_per_unit` multiplies the scaled value** of `speed` — the number `vagcan watch` shows
  for it — not its raw value.
- **Measure it.** Drive at a steady speed. Divide the speed in km/h shown by a GPS by the value
  of `speed` at the same moment. It is 1 when `speed` already reads true km/h.
- **A wrong factor makes every time wrong, silently.**
- `marks` are whole numbers: `60.0` is refused.
- **The panel shows `speed` rounded down**, on the stopwatch page and on any page: a mark counts
  when the speed reaches it, so `100` shows once the 100 mark is passed, never at 99.5.
- The page opens with `[stalk]`'s `measure` or a `[[button]]` with `action = "stopwatch"`. With
  neither, the build succeeds and its output says nothing opens the page.

### `[[button]]`

A button on one of the board's free pins. 0 to 3 of them, one per pin. Each does one thing.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `pin` | integer: 3, 4 or 5 | yes | — | The GPIO the button is on. |
| `action` | `"next"`, `"previous"` or `"stopwatch"` | yes | — | `next` / `previous`: the next or previous page, wrapping. `stopwatch`: the stopwatch page on and off. The lever's + / − / LIMIT. |

- **Wiring:** a push button from the pin to GND. No resistor: the board turns on the pin's
  pull-up.
- A press counts when the button is let go. Held 3 s, it counts then. Holding does not repeat.
- Two buttons may have the same `action`.
- With a `[stalk]`, the stopwatch stays open only while cruise is off: a `stopwatch` button
  opens it, and cruise engaged closes it again.
- A `stopwatch` button with no `[stopwatch]` builds; the output says a press of it only silences
  an alarm.
- The board's own BOOT and RESET buttons do nothing in the `dash` image. They are for flashing
  and resetting.
- A pin no `[[button]]` names is left alone.

The pins, with this board's wiring:

| GPIO | used by | a button? |
|---|---|---|
| 3, 4, 5 | nothing | yes |
| 0, 7, 10, 20, 21 | the OLED | no |
| 1, 6 | the CAN transceiver | no |
| 8 | the LED; a strapping pin | no |
| 9 | the BOOT button; a strapping pin | no |
| 2 | a strapping pin | no |
| 11 | not on the board's pads | no |
| 12–17 | the flash | no |
| 18, 19 | USB | no |

## Naming a channel

Two spellings, both `<unit number or request id>:<channel>`:

| spelling | example | when |
|---|---|---|
| channel id | `01:IDE00025` | The usual one: stable across variants, and what `names.csv` is keyed by. |
| identifier | `02:380A`, `02:3816@3` | A row with no channel id: one measured on a car, or a standard OBD-II parameter. `@` is the bit offset, 0 when left out. |

- The unit is a request id (`7E0`, `714`) or a unit number (`01`, `17`, `4B`).
- A request id always works.
- A unit number works only where the run's source numbers that unit: `17` on an ODIS run, not on a VCDS run that has no label file for the cluster. Otherwise write the request id: `714:IDE00698`. `vagcan units` lists both.
- `dev dash edit` writes a new channel by number where the run's source numbers the unit, and by request id otherwise. A ref already in the file keeps its spelling.
- A **channel id** is VW's own id for a measurement, such as `IDE00025`. The build's messages call it a "text id".
- An identifier is exactly four hex digits. Anything else after the colon is a channel id.
- A channel id takes no `@`.
- `vagcan watch --did` takes the same spellings as `ref`: `<unit>:<DID>`, `<unit>:<channel id>`,
  `<unit>:<DID>@<bit>`. It also takes a pair as the Key column shows it, `01:IDE00191/IDE00190`: both rows.
- Which variant of the unit the car has is not written here. It comes from the car's record of its units.
- One row is one channel however it is spelled. `01:IDE00191` and `01:202A` are the same row:
  declaring both is refused, and a page, a `setpoint` or `speed` may use either spelling for a
  row declared under the other.

## How to find names for your car

| you need | how |
|---|---|
| the units the car has, and their numbers | `vagcan units` |
| a channel's unit, identifier and channel id | `vagcan watch`, press `,` for settings, set "Key at the end of each row" to shown. The Unit, DID and Key columns are the control unit, the identifier and the channel id. |
| channels for pages, by name, unit or unit of measure | `vagcan dev dash edit <VIN>`: a page in the browser lists every channel the build accepts, draws your pages on the board's renderer, and Save writes them into `dash.toml` once the build accepts them. |
| a channel id by the channel's name | `vagcan dev glossary` writes `~/.vagcan/names.csv`: every channel id the project knows, with its name in the `current` column. Search it. It does not say which unit. |
| a bit offset | Write the channel id. When several rows answer to it, the build refuses and lists each as `DID@bit name`. Copy one as `<unit>:<DID>@<bit>`. An identifier with no `@` is bit 0: a row that starts at another bit is refused as one the unit does not have, with no list. |
| the lever's identifier | Record `vagcan watch --out lever.csv` with the identifiers of the lever's unit selected, working the lever. Then `vagcan dev recording discover --log lever.csv` sorts the identifiers into never moved, stepped and continuous. |
| `[stalk]` field names | Put any name in `rocker`. The build refuses and lists every field of `read`. |
| state names | A wrong state name makes the build list the field's states. `vagcan watch --did <unit>:<DID>` shows a field's current state by name: work the lever and watch which state appears. |
| what the build made of each name | `vagcan dev dash build <VIN>`: one line per channel (see [Reading the build's output](#reading-the-builds-output)), and per target value; then the stopwatch, the lever and each button; then the alarms. |

## Limits

| | |
|---|---|
| `[[channel]]` | 1 or more |
| `[[page]]` | 1 to 8 |
| cells on a values page | 1 to 4 |
| chart pages per channel | 1 |
| alarms | any number; the build's `bus:` lines estimate what they cost |
| `alarm_priority` | 1 or more |
| `hz` | above 0, at most 50; 2 when absent |
| `hz` of the stopwatch's `speed` | 7.5 or more; 50 recommended |
| `decimals` | 0 to 3 |
| `label` | ten characters on a page of four cells |
| `title` | 34 characters with the voltage, both hosts connected and a two-digit fault count |
| stopwatch `marks` | 1 to 3, whole km/h from 1 to 65535 |
| `[[button]]` | 0 to 3, on GPIO 3, 4 and 5, one per pin |
| chart `min`/`max`, alarm numbers, `km_h_per_unit` | within a 32-bit float |

## What the build refuses

Every refusal names what failed. `vagcan dev dash build` prints it after `Error:`; the firmware
build prints it after `dash plan for VIN <VIN>:`.

**The file**

| refusal | what to do |
|---|---|
| `dash.toml: …` with a line and column | A TOML syntax error. Fix it there. |
| `line N: [[channel]] 2: unknown key "hzz" — did you mean "hz"? [[channel]] takes …` | Fix the spelling. The message lists the keys that table takes. |
| `line N: unknown section [[pages]] — did you mean [[page]]? The top level takes …` | Fix the section's name. |
| `line N: [[page]] 1: "min" is a key of a chart page (kind = "chart"), and this is a values page` | Remove the key, or change `kind`. |
| `line N: [[alarm]] is gone: an alarm is now keys of the [[channel]] it watches. … add to each [[channel]]: …` | Delete the `[[alarm]]` tables and add the keys it lists to those channels. See [Alarms](#alarms). |
| `line N: [stopwatch]: "language" is a top-level key: write it above the first section` | Move it above the first section. |
| `line N: [[page]] 1: "hz" is a key of [[channel]]` | Move it under that section. |
| `line N: [[channel]] 1: label must be a string, not an integer` | Write the type this reference gives. The same for every optional key but `hz` and `setpoint`, whose refusals are under **Channels**. |
| `no dash.toml for <VIN> yet. Write <path>; the smallest one, …` | Write the file it shows at that path, or pass `--input`. |
| `vin is missing or not a string` | Add `vin = "<VIN>"`. |
| `<path> is for VIN X but the build asked for Y` | Build for X, or correct `vin`. |
| `language "xx" is not one this build has words for` | `"en"` or `"ru"`. |
| `no record of <VIN>'s control units — connect to the car and run …` | Connect to the car once: `vagcan units --identify` or `vagcan watch` writes it. |
| `the channels of N control units … have not been read from the VCDS installation (…)` | Only on a build that reads the VCDS installation. Only the units the configuration uses count. Run `vagcan dev dash build <VIN>` once: it reads them from the installation, offline, then builds. If the refusal goes on to say the installation `is not on this machine any more`, or names the project's own note of it, re-run `vagcan setup <VCDS installation>` first. If `dev dash build` is what just ran, the lines above it say what stopped the read. |
| `the channels of N control units … (…), which the plan does not use` | Not a refusal: a note among the build's lines, and the build goes on. Those units' channels are read by the next `watch`, `measure` or `units --identify` with the car, or by `vagcan dev dash build <VIN>`; when the note says the installation is gone, re-run `vagcan setup <VCDS installation>` if you want them. |
| `no [[channel]]` / `no [[page]]` | Add one. |
| `line N: [channel] has one pair of brackets — write [[channel]], two pairs` | Two pairs of brackets. The same for `[page]` and `[button]`. |
| `stalk must be one [stalk] table` / `stopwatch must be one [stopwatch] table` | One pair of brackets. |
| `<key> is missing or not a string` | Add the key, quoted. |

**Naming a channel**

| refusal | what to do |
|---|---|
| `a channel is <unit>:<text id> or <unit>:<DID>[@<bit>]` / `nothing after the unit` | Spell it as in [Naming a channel](#naming-a-channel). |
| `no control unit given` | Write the unit before the colon. |
| `"X" is not a control-unit number like 01 or 17` / `is not a hex request id like 714` | Fix the unit. |
| `control unit NN … has no known request id` | Write the unit's request id instead, as the `help:` line spells it: `714:IDE00698`. `vagcan units` lists them. |
| `… has no diagnostic address (700-795 or 7E0-7E7)` | Not a diagnostic unit. Fix the unit. |
| `a bit offset goes with an identifier, not a text id` | Drop the `@`, or write the identifier. |
| `"b" is not a bit offset` | A whole number after `@`. |

**Channels**

| refusal | what to do |
|---|---|
| `decimals N is not 0..=3` | 0 to 3. |
| `hz must be a number above 0 and at most 50` | Fix `hz`. |
| `hz V is too small for the board, which would hold it as 0` | A larger `hz`. |
| `setpoint is not a string` | Quote it. |
| `unit XXX is not in the car's record of its units` | Connect to the car with that unit answering (`vagcan units --identify`), or fix the unit. |
| `unit XXX: the car's record has no part number (F187) for it` | Run `vagcan units --identify` again. The board checks the unit by that number. |
| `X: unit NN has no channel with the id … in this run's label data, and none was measured on a car` | The build's one source has no such row for this car's unit. Find its ids in `vagcan watch`: press `,` and set "Key at the end of each row" to shown. Where the `help:` line offers the other source (`--source vcds` or `odis`), the row may be that source's: build from it, and use its ids throughout. |
| `X: unit NN has no channel at identifier DDDD, bit B in this run's label data, and none was measured on a car` | No row at that identifier and bit on this car's unit, in the build's source. Check the identifier and the `@` bit, or find the row in `vagcan watch` as above; or build from the other source as the `help:` line says. |
| `X: "…" is the channel's name — write its id: NN:ID` (or `is the name of N channels — write the id of one`) | Write one of the listed ids instead of the name. |
| `X: "…" is a name or a mistyped id, and dash.toml takes a channel's id` | Find the id in `vagcan watch` as above. |
| `X: "…" is the name of a channel the board cannot show — its scaling is not linear` | The board shows numbers only. Pick a numeric row. |
| `X: unit NN is in the car's record, but this run's label data has no channels for its variant` | The build's source describes nothing on that unit. Pick a channel of another unit, or build from the other source as the `help:` line says. |
| `X: N rows answer to it — name one by identifier and bit offset: DID@bit name, …` | Write one of the listed rows as `<unit>:<DID>@<bit>`. |
| `X: scaling is an enumeration, not linear` (or `a single proven point with no slope`) | The board shows numbers only. Pick a numeric row. |
| `X: its scaling is not a finite number the board's 32-bit float holds` | Pick another row. Also for a factor the board would hold as 0. |
| `X is listed twice under [[channel]]` | Delete one. |
| `X and Y are the same row …; keep one [[channel]]` | Delete one. |
| `X: its setpoint Y is on another unit` | Pair channels of one unit only. |
| `X: its setpoint Y reads in "a" against the channel's "b"` | Pair a row in the same unit of measure. |
| `X: its setpoint Y is the channel itself` | Name the target value, not the channel itself. |
| `X: its setpoint Y has a setpoint of its own` | Remove one of the two. |
| `X: its setpoint Y is read at A Hz and the channel it explains at B Hz` | Give both the same `hz`. Two channels sharing one target value need the same `hz` too. |

**Pages**

| refusal | what to do |
|---|---|
| `N [[page]] tables, and the board holds at most 8` | Remove pages. |
| `page #n: kind "x" is not "values" or "chart"` | Lowercase `values` or `chart`. |
| `page #n has no cells` / `a cell is not a string` | Add `cells`, quoted. |
| `page #n: a values page holds 1 to 4 cells, not N` | 1 to 4 cells. |
| `page #n needs min` / `needs max` | Add both, as numbers. |
| `line N: [[page]] n: min must be a number, not a string` (or `max`) | Write it unquoted. |
| `line N: [[page]] n: min must be a finite number the board's 32-bit float holds, within ±3.4028235e38` (or `max`) | A finite number within that range. |
| `page #n: min A is not below max B` | Put `min` below `max`, far enough apart to differ as 32-bit floats. |
| `page #n: min A and max B are further apart than the board's 32-bit float holds` | Narrow the scale. |
| `page #n: X already has a chart page; one range per channel` | Keep one chart of it. |
| `page #n: X is not in the [[channel]] list` | Declare it under `[[channel]]`. |
| `page #n: X is a setpoint with no [[channel]] of its own — declare it as a [[channel]] to show it` | Declare it under `[[channel]]`. |
| `line N: [[page]] n: volts must be true or false, not a string` | `volts = false`, unquoted. |
| `page #n: volts needs a [strip]` | Add a `[strip]`, or remove `volts`. |

**`[strip]`**

| refusal | what to do |
|---|---|
| `strip must be one [strip] table` | One pair of brackets. |
| `[strip] volts is missing or not a string` | Add `volts`, a quoted channel. |
| `[strip] volts X is not in the [[channel]] list` | Declare it under `[[channel]]`. |
| `[strip] volts X is a setpoint with no [[channel]] of its own — declare it as a [[channel]] to show it` | Declare it under `[[channel]]`. |

**Alarms**

The first group comes with a line and the channel's place, `line N: [[channel]] n: …`; the rest
name the channel, `X: …`.

| refusal | what to do |
|---|---|
| `alarm_max must be a number, not a string` (or any alarm key) | Write it unquoted. |
| `alarm_max V is not a number the board's 32-bit float holds` (or any alarm key) | A smaller number. |
| `alarm_max_off needs alarm_max` (or `alarm_min_off`) | Add the bound, or remove the off value. |
| `alarm_max_off A is not under alarm_max B` | Write `alarm_max_off` under `alarm_max`. |
| `alarm_min_off A is not over alarm_min B` | Write `alarm_min_off` over `alarm_min`. |
| `a drift alarm takes all four of drift_percent, drift_off_percent, drift_hold_ms, drift_min_setpoint — missing …` | Add the missing keys. |
| `drift_percent 0 is not above zero` (or `drift_off_percent`) | Above 0. |
| `drift_off_percent V is too small for the board, which would hold it as 0` (or `drift_percent`) | A larger share. |
| `drift_off_percent A is not under drift_percent B` | Lower `drift_off_percent`. |
| `drift_hold_ms must be whole milliseconds, 0 or more` | An integer, `1000` not `1000.0`. |
| `drift_min_setpoint V is below zero` | 0 or more. |
| `X: alarm_max needs a page that shows the channel` (or `alarm_min`, `drift_percent`) | Put X on a values page or a chart. |
| `X: alarm_min_off A is not under alarm_max_off B — no value would be off for both` | Move the two off values apart. `(its default)` marks one the build derived: write it. |
| `X: alarm_max_off A (its default) is alarm_max B on the board` (or `alarm_min_off`) | Write the off value. |
| `X: drift_percent needs a setpoint` | Give X a `setpoint`, or remove the `drift_*` keys. |
| `alarm_priority V is not a whole number, 1 or more` / `alarm_priority must be a whole number, 1 or more, not …` | 1, 2, 3 … unquoted. |
| `alarm_priority needs an alarm on the channel` | Add the alarm, or remove `alarm_priority`. |

**`[stalk]`**

| refusal | what to do |
|---|---|
| `[stalk] read X is not <unit>:<DID>` | One identifier, four hex digits, no `@`. |
| `unit XXX is not in the car's record of its units` | `read`'s or `cruise`'s unit: connect to the car with it answering (`vagcan units --identify`). |
| `[stalk] read X: the car's variant declares no such identifier` | Find the lever's identifier (see [How to find names](#how-to-find-names-for-your-car)). |
| `[stalk] rocker: X has no field named "N" — its fields are …` | Copy a name from the list. Same for `switch`. |
| `[stalk] rocker: X has 2 fields named "N"` | The project gives the name twice, possibly differing only by a space at an end. That field cannot be named. |
| `[stalk] rocker: "N" is a quantity, not a list of states` | Name an enumerated field. Same for `switch`. |
| `[stalk] rocker and switch are both "N"` | Two different fields. |
| `[stalk] next: "S" is not a state of "F" — its states are …` | Copy a state from the list. Same for every state key. |
| `[stalk] next: "S" names 2 bands of "F" (51–101, 204–255) — the board takes a button as one band, so this state cannot be used yet` | That state cannot be a button. Pick another. For `switch_off` and `cruise_off` it says "an off state". |
| `[stalk] next, previous and measure name the same state` | Three different states. |
| `[stalk] cruise X: the car's variant declares no such field` | Fix `cruise`. |
| `[stalk] cruise X: a quantity, not a list of states` | Name the enumerated cruise status. |
| `X: N rows answer to it — name one by identifier and bit offset: DID@bit name, …` | For `cruise`: write one of the listed rows as `<unit>:<DID>@<bit>`. |
| `[stalk] cruise X is in read's own identifier Y` | Name the cruise status the unit that runs cruise control reports. |
| `[stalk] unit XXX: the project's cache keeps only the lower end of each state` | Run `vagcan setup` again. |
| `[stalk] <key>: the proven row for "N" in <file> holds each state as a single value — it predates state ranges` | A row in the project's `measurements/` wins over the project and has lost the bands. Write its states as `[lower, upper, "name"]`, or remove the row. |

**`[stopwatch]`**

| refusal | what to do |
|---|---|
| `[stopwatch] km_h_per_unit must be a number at or above 0` | A number, 0 until measured. Also for a missing key or one below 0. |
| `[stopwatch] km_h_per_unit V is too large for the board, which holds it as a 32-bit float` | Measure it again. |
| `[stopwatch] km_h_per_unit V is too small for the board, which would hold it as 0` | Measure it again. |
| `[stopwatch] marks must be a list of speeds in km/h` | Add `marks = [60, 100]`. |
| `[stopwatch] mark V is not a whole speed in km/h above 0` | Whole numbers above 0. |
| `[stopwatch] mark N is listed twice` | Each once. |
| `[stopwatch] has N marks, and the page holds 1 to 3` | 1 to 3 marks. |
| `[stopwatch] speed X is not in the [[channel]] list` | Declare it under `[[channel]]`. |
| `[stopwatch] speed X is a setpoint with no [[channel]] of its own — declare it as a [[channel]] to show it` | Declare it under `[[channel]]`, with its `hz`. |
| `[stopwatch] speed X: its scaling has an offset` | Pick a speed row whose zero is a standstill. |
| `[stopwatch] speed X: its scaling's factor F is not above zero` | Pick another speed row. |
| `[stopwatch] speed X is read every P ms (hz = H)` | Give its `[[channel]]` `hz = 50`. |

**`[[button]]`**

| refusal | what to do |
|---|---|
| `button must be written as [[button]] tables, one per button` | Two pairs of brackets. |
| `N [[button]] tables, and the board has 3 pins free for one: 3, 4 and 5` | 3 buttons at most. |
| `button #n needs pin, a whole number: 3, 4 or 5` | Add `pin`, unquoted: `pin = 3`, not `"3"` or `3.0`. |
| `button #n: pin P is the OLED's CS — a button goes on pin 3, 4 or 5` | Move it to 3, 4 or 5. The message says what holds the pin, or that the chip has no such GPIO. |
| `button #n: pin P is button #m's already — one button per pin` | One button per pin. |
| `button #n needs action: "next", "previous" or "stopwatch"` | Add `action`, quoted. |
| `button #n: action "X" is not "next", "previous" or "stopwatch"` | One of the three, lowercase. |

## What the board does with it

**Rates**

- A channel on the page on screen: at its `hz`.
- A channel only on other pages, charts included: once a second, or at its `hz` if slower.
- A channel with an alarm: at its `hz`, whatever page is up.
- A target value (`setpoint`): with its channel.
- `[strip]`'s `volts`: as a cell of every page whose strip shows it.
- Any other channel on no page: not read, except the stopwatch's `speed` while the stopwatch is
  on.
- All channels together: at most 100 requests a second. Asked for more, the board reads them
  more slowly than their `hz` (**Bus time**, below).
- At start-up the board checks each unit's part number (`F187`) against the one the dash was built
  for. A unit that answers another number is not read: after replacing a unit, run `vagcan units --identify` and rebuild.

**Bus time**

- The board asks one control unit at a time. One request carries up to 8 channels of one unit,
  so a page costs its units more than its channels.
- The build estimates each page: the board's scheduler run over the plan, every exchange taken as
  34 ms. That is an upper bound, from a BLE recording on the reference car.
- One line per page on screen, with cruise off (the lever read 20 times a second) and on (2), and
  one for a stopwatch run. Each names the reads it would serve slower than their `hz`:

  ```
  bus: page 2 "TEMPERATURES" cruise off ~93 % — reads the lever 17.4/s of 20; cruise on ~41 %
  ```

- The build refuses nothing for bus time.
- The board measures the real number: `dashcfg`'s `bus` line (the board's `exch=…/s exch_ms=…`)
  says its exchanges a second, and how long an answered one took, over the last 10 s. Its USB
  console says the same every 10 s, by unit.

**Paging**

- Pages turn from `[[button]]`s, the lever (`[stalk]`) and `dashsim`. The board's BOOT and RESET
  buttons do nothing.
- With no `[stalk]` and no `next` or `previous` `[[button]]`, the board shows its active page.
  Only `dashsim` or `dashcfg`'s `set page` changes it. The build says so.
- With no `[stalk]` and no `[[button]]`, only `dashsim` silences an alarm (**Alarms on the
  board**, below).
  The build says so.
- With the stopwatch on, `next` and `previous` do nothing, from any of them. `stopwatch` leaves
  it; so does the board turning adapter (`vagcan --slcan`), and the lever (**The stopwatch**,
  below).

**The lever**

- The lever is the dash's only while `switch` reads `switch_off` and `cruise` reads
  `cruise_off`. A missing reading counts as not off.
- It is read every 50 ms while it is the dash's, every 500 ms otherwise, every 100 ms with the
  stopwatch on and `km_h_per_unit` above 0. The cruise status is read every 200 ms, while the
  lever's unit answers with the part number the dash was built for.
- A press is two readings in a row in the same state: hold it at least 0.1 s, 0.2 s with the
  stopwatch on. A shorter press can be missed. Holding does not repeat.
- `next` and `previous` wrap round the pages. With the stopwatch on they do nothing.
- While an alarm holds the screen, any press silences it instead.

**The stopwatch**

- Standing still for 1 s arms it: the page shows `GO` (`ПУСК` in Russian), inverted. The first
  moving reading starts the run.
- Times run from the launch, fitted to the first 400 ms of movement.
- The run ends at the highest mark.
- Stopping before the highest mark aborts the run. So does the speed not answering for over
  1.3 s. The page shows `ABORT` (`ПРЕРВАН`) over the times the run had, until the next run starts
  or you leave the page. An aborted run is never stored.
- A run with no times (the speed read too slowly for the launch fit) is not kept.
- With the stopwatch on, the pages' channels are read once a second at most. An alarm's page
  shown over it reads its cells at their `hz`, except while the stopwatch is armed or running.
  Alarm channels keep their rate.
- A finished run is kept in the board's settings. It is written to flash the next time the car
  stands still for 1 s with the stopwatch on, before `GO`: the stopwatch arms right after the
  write.
- With BLE, `save` in `dashcfg` writes it too. Until it is in flash, `dashcfg`'s storage line and
  `get` say a run is in RAM only.
- On a board whose flash holds no settings (never saved, or erased), the run is written with the
  default settings. Changes you have not saved stay unsaved.
- Leave the stopwatch before the car stops, and the run is not written until the stopwatch is on
  again and the car stands still 1 s. Power-off before that loses it. With BLE, `save` keeps it.
  An image built without BLE has no `save`.
- With a `[stalk]`, cruise switched on closes the stopwatch: two readings in a row with `switch`
  not at `switch_off` or `cruise` not at `cruise_off`. A run under way is dropped. However the
  stopwatch was opened.
- With a `[stalk]`, the lever unreadable for over 3 s closes it too: no answer, a stale cruise
  status, a reading no state claims, its unit silent. A shorter gap leaves it and a run alone.

**Alarms on the board**

- A channel with an alarm is watched whatever is on screen, the stopwatch included.
- When it crosses, the first values page with it comes up — its chart, if no values page has it —
  its cell (a chart's header) blinking 0.4 s on and 0.4 s off while the value is out. Another channel out on that page blinks too.
- Once the value is back at its off value, the page stays 2.5 s with the cell steadily inverted.
  Then the screen goes back to where it was.
- A press — a `[[button]]`, the lever or `dashsim` — silences every alarm on the screen until
  its value is back at its off value. An alarm out on another page then takes the screen.
- Unsilenced, it lasts until its value is back at its off value. A channel that stops answering
  keeps it on the glass.
