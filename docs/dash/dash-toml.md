# `dash.toml` — reference

`dash.toml` says what the dash board reads and shows: channels, pages, alarms, the cruise lever,
the stopwatch and buttons on the board's pins. One file per car, written by hand:

```
~/.vagcan/dash/<VIN>/dash.toml
```

The build resolves every name in it against the car's survey and the project's label data and
writes the plan the firmware links. The board resolves nothing itself.

## Build it

| what you want | run |
|---|---|
| see the plan and the reasons, no firmware | `vagcan dev dash build <VIN>` |
| build the firmware for that car | `VAGCAN_DASH_VIN=<VIN> cargo build --release --bin dash` in `crates/dash/vag-dash-fw` |
| check the firmware compiles, no car | `VAGCAN_DASH_NO_CAR=1 cargo build --release --bin dash` (an empty plan — do not flash it) |

Both paths run the same build, so `vagcan dev dash build` is for reading the result, not a step
before the firmware build.

- `VAGCAN_DASH_VIN` picks the car. With exactly one car under `~/.vagcan/dash/`, it can be left
  unset.
- `VAGCAN_PROJECT` (or `--project`) picks which project's label data to resolve against.
- `--input <FILE>` builds from another file; the outputs still land under the car.

Outputs, both under `~/.vagcan/dash/<VIN>/`:

| file | for |
|---|---|
| `plan.json` | reading, and the simulator |
| `plan.rs` | the firmware, which `include!`s it |

Neither is committed anywhere: they are derived from VW's data and describe one car.

## A full example

Every section, every key. The identifiers, text ids and state names are **one car's** (Škoda
Octavia III). Take yours from your car: see [How to find names for your car](#how-to-find-names-for-your-car).

```toml
vin = "XW8AD4NE9JH008917"
language = "ru"                    # labels (names.csv's ru column) and the board's own words
# survey = "/Users/me/surveys/octavia.jsonl"   # only to use a survey other than the car's own

[[channel]]
ref = "01:IDE00025"                # coolant temperature, by text id
label = "ОЖ"                       # the name on the panel
decimals = 0
hz = 10                            # readings a second while its page is up

[[channel]]
ref = "01:IDE00191"                # boost pressure, actual
setpoint = "01:IDE00190"           # boost pressure, specified: same unit; no [[channel]] needed
decimals = 2
hz = 10

[[channel]]
ref = "02:380B"                    # the stopwatch's speed: on no page, read fast
hz = 50

[[page]]
kind = "values"
title = "MAIN"                     # alarms name the page by this
cells = ["01:IDE00025", "01:IDE00191"]

[[page]]
kind = "chart"
cell = "01:IDE00025"
min = 70                           # fixed scale
max = 110

[[alarm]]                          # no `kind`: a threshold rule
channels = ["01:IDE00025"]
page = "MAIN"
direction = "above"
trip = 105
release = 100

[[alarm]]
kind = "drift"
channels = ["01:IDE00191"]         # each needs a `setpoint`
page = "MAIN"
percent = 10
release_percent = 6
hold_ms = 1000
min_setpoint = 0.5

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
speed = "02:380B"                  # the [[channel]] above, with its hz
km_h_per_unit = 0.0                # 0 until measured: the page shows NO FACTOR
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
- A key of the other `kind` is refused: `min` on a values page, `trip` on a drift rule.
- Strings are trimmed at both ends, except `survey`.
- Keywords are lowercase: `kind = "Values"` and `direction = "Above"` are refused.

Check the build's output after every edit. It prints one line per channel (with its rate), per
alarm, for the lever and for the stopwatch.

### Top level

| key | type | required | default | what it does |
|---|---|---|---|---|
| `vin` | string | yes | — | The car. Must be the VIN the build is for (case does not matter), or the build stops. |
| `language` | `"en"` or `"ru"`, any case | no | `language` in `~/.vagcan/config.toml`, else `"en"` | The labels' language, and the words the board writes itself (the stopwatch page). |
| `survey` | string, a file path | no | `~/.vagcan/cars/<VIN>/survey.jsonl` | The survey to resolve against. Give an absolute path: `~` is not expanded. |

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
| `hz` | number, above 0, at most 100 | no | 2 | Readings a second while a page showing it is up. |
| `setpoint` | string: a channel on the same unit | no | none | What the unit asked for. The panel draws the difference under the number; a `drift` alarm watches it. |

- The row must scale linearly: a number. An enumeration, or a proven point with no slope, is
  refused.
- A channel on no page is not read, unless it is the stopwatch's `speed` or another channel's
  `setpoint`.

**`setpoint`**

- Same unit as the channel: both are read in one request, so the two numbers are from the same
  moment.
- It needs no `[[channel]]` of its own. The build adds it, read at the channel's `hz`.
- Without a `[[channel]]` of its own it is never shown: no page, alarm or stopwatch may name it.
  Declare it under `[[channel]]` to show it.
- If it has a `[[channel]]` of its own, that one's `hz` must equal the channel's.
- Same unit of measure as the channel.
- It cannot be the channel itself, and cannot have a `setpoint` of its own.

### `[[page]]`

Pages come in the file's order. 1 to 8 of them. `kind` picks the page.

**`kind = "values"`** — up to four numbers side by side.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `kind` | `"values"` | yes | — | |
| `title` | string | no | `""` | The name an `[[alarm]]` raises the page by. Not drawn on the panel. |
| `cells` | list of channels | yes | — | 1 to 4 channels, one column each. Each must be under `[[channel]]`, in either spelling. |

**`kind = "chart"`** — one channel, large, with its recent history.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `kind` | `"chart"` | yes | — | |
| `cell` | string: a channel | yes | — | The channel, under `[[channel]]`. |
| `min` | number | yes | — | Bottom of the scale. |
| `max` | number | yes | — | Top of the scale. Above `min`. |

- The scale is fixed, never autoscaled.
- The board holds `min` and `max` as 32-bit floats: each must fit one, and `min` must stay below
  `max` in it.
- One chart per channel.
- A chart has no title, so no alarm can raise it.
- A key of the other kind (`min` on a values page, `title` on a chart) is refused.

### `[[alarm]]`

A rule that takes the screen when a value goes wrong. 0 to 4 rules. The file's order is their
priority, first highest. Two kinds.

**Threshold** — `kind` left out, or `"threshold"`.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `kind` | `"threshold"` | no | `"threshold"` | |
| `channels` | list of channels | yes | — | 1 or more, each under `[[channel]]`, all shown on `page`. |
| `page` | string | yes | — | The `title` of one values page. |
| `direction` | `"above"` or `"below"` | yes | — | Which way is wrong. |
| `trip` | number | yes | — | Fires at this value or past it. |
| `release` | number | yes | — | Clears once strictly back past this. Must be on the far side of `trip`: below it for `"above"`, above it for `"below"`. |

**Drift** — distance from the value the unit asked for.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `kind` | `"drift"` | yes | — | |
| `channels` | list of channels | yes | — | As above, and each must have a `setpoint`. |
| `page` | string | yes | — | As above. |
| `percent` | number, above 0 | yes | — | Fires when the channel is this far from its specified value, in percent of the specified value, or further… |
| `hold_ms` | integer, 0 or more | yes | — | …for this many milliseconds without a break. |
| `release_percent` | number, above 0, under `percent` | yes | — | Clears under this share. |
| `min_setpoint` | number, 0 or more | yes | — | While the specified value is nearer 0 than this, either side, the rule says nothing. In the channel's unit. |

- Any watched channel past `trip` (or `percent`) fires the rule. All of them must be back past
  `release` (or under `release_percent`) to clear it.
- Every number must fit a 32-bit float. They are compared as the board holds them.
- A key of the other kind (`trip` on a drift rule, `percent` on a threshold rule) is refused.
- Each rule's channels are read at their `hz` on every page, not only on theirs.

### `[stalk]`

The cruise lever as buttons, while cruise control is off: one rocker state for the next page,
one for the previous, one to turn the stopwatch on and off.

| key | type | required | default | what it does |
|---|---|---|---|---|
| `read` | string: `<unit>:<DID>` | yes | — | The one identifier whose answer carries both the rocker and the switch. No text id, no `@`. |
| `rocker` | string: a field name | yes | — | The field of `read` that the lever moves. |
| `switch` | string: a field name | yes | — | The field of `read` with the cruise main switch. Not the rocker. |
| `next` | string: a state of `rocker` | yes | — | Next page. |
| `previous` | string: a state of `rocker` | yes | — | Previous page. |
| `measure` | string: a state of `rocker` | yes | — | Stopwatch on and off. |
| `switch_off` | string: a state of `switch` | yes | — | The switch's off state. |
| `cruise` | string: a channel | yes | — | The cruise control status, an enumerated field, on another identifier than `read`. Spelled as a channel. |
| `cruise_off` | string: a state of `cruise` | yes | — | Its off state. |

- **Names are the project's, exactly and in full**, parentheses included. Spaces at either end
  do not count, on your side or the project's.
- `next`, `previous` and `measure` are three different states.
- Each state must be one band of raw values. A state name the project gives to several bands is
  refused for now.
- `cruise` cannot be in `read`'s identifier. The lever is the dash's only when two separate
  answers say cruise is off: `switch` reads `switch_off`, and `cruise` reads `cruise_off`.
- **A name that is valid but wrong builds and misbehaves.** `next` and `previous` swapped turn
  the pages backwards. An off state that is not off lets the lever turn pages while it also
  works cruise control. Check each state with `vagcan watch` before driving.
- `measure` on the reference car: LIMIT reads as `Blinker GRA neutral ohne Limiterverbau (…)`,
  despite the name. That state appeared only while LIMIT was pressed. Pick the state the
  position you press actually produces, as `vagcan watch` shows it, not the one whose name fits.
- The lever's read rates are fixed. `hz` does not apply.

### `[stopwatch]`

Times 0 to each mark, in km/h, on the board. Every key is required.

The `speed` channel must be under `[[channel]]` with an `hz`: without one it is read at 2 Hz and
refused. A snippet that builds:

```toml
[[channel]]
ref = "02:380B"
hz = 50

[stopwatch]
speed = "02:380B"
km_h_per_unit = 0.0
marks = [60, 100]
```

| key | type | required | default | what it does |
|---|---|---|---|---|
| `speed` | string: a channel | yes | — | A `[[channel]]` with `hz` 7.5 or more (50 recommended). Its scaling has no offset and a factor above 0. |
| `km_h_per_unit` | number, 0 or more | yes | — | km/h per unit of `speed`. `0`: not measured — the page shows `NO FACTOR` and times nothing. |
| `marks` | list of integers | yes | — | 1 to 3 speeds in km/h, each above 0, each once. Shown in the order written; the run ends at the highest. |

- **The rate.** `speed` must be read every 133 ms or sooner: `hz` 7.5 or more. The launch fit
  needs 3 readings in its first 400 ms even when one answer is late.
- **`km_h_per_unit` multiplies the scaled value** of `speed` — the number `vagcan watch` shows
  for it — not its raw value.
- **Measure it.** Drive at a steady speed. Divide the speed in km/h shown by a GPS by the value
  of `speed` at the same moment. It is 1 when `speed` already reads true km/h.
- **A wrong factor makes every time wrong, silently.**
- `marks` are whole numbers: `60.0` is refused.
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

Two spellings, both `<unit>:<row>`:

| spelling | example | when |
|---|---|---|
| text id | `01:IDE00025` | The usual one: stable across variants, and what `names.csv` is keyed by. |
| identifier | `02:380A`, `02:3816@3` | A row with no text id: a proven one, or a standard OBD-II parameter. `@` is the bit offset, 0 when left out. |

- The unit is a short number (`01`, `02`, `4B`) or a request id (`7E0`, `714`).
- An identifier is exactly four hex digits. Anything else after the colon is a text id.
- A text id takes no `@`.
- Which variant of the unit the car has is not written here. It comes from the survey.
- One row is one channel however it is spelled. `01:IDE00191` and `01:202A` are the same row:
  declaring both is refused, and a page, an alarm, a `setpoint` or `speed` may use either
  spelling for a row declared under the other.

## How to find names for your car

| you need | how |
|---|---|
| the units the car has, and their numbers | `vagcan units` |
| a channel's unit, identifier and text id | `vagcan watch`, press `,` for settings, set "Key at the end of each row" to shown. The ECU, DID and Key columns are the unit, the identifier and the text id. |
| a text id by the channel's name | `vagcan dev glossary` writes `~/.vagcan/names.csv`: every text id the project knows, with its name in the `current` column. Search it. It does not say which unit. |
| a bit offset | Write the text id or identifier. When several rows answer to it, the build refuses and lists each as `DID@bit name`. Copy one as `<unit>:<DID>@<bit>`. |
| the lever's identifier | Record `vagcan watch --out lever.csv` with the identifiers of the lever's unit selected, working the lever. Then `vagcan dev recording discover --log lever.csv` sorts the identifiers into never moved, stepped and continuous. |
| `[stalk]` field names | Put any name in `rocker`. The build refuses and lists every field of `read`. |
| state names | A wrong state name makes the build list the field's states. `vagcan watch --did <unit>:<DID>` shows a field's current state by name: work the lever and watch which state appears. |
| what the build made of each name | `vagcan dev dash build <VIN>`: one line per channel with its identifier, bits, scaling, rate and source, then the alarms, the lever and the stopwatch. |

## Limits

| | |
|---|---|
| `[[channel]]` | 1 or more |
| `[[page]]` | 1 to 8 |
| cells on a values page | 1 to 4 |
| chart pages per channel | 1 |
| `[[alarm]]` | 0 to 4 |
| `hz` | above 0, at most 100; 2 when absent |
| `hz` of the stopwatch's `speed` | 7.5 or more; 50 recommended |
| `decimals` | 0 to 3 |
| `label` | ten characters on a page of four cells |
| stopwatch `marks` | 1 to 3, whole km/h from 1 to 65535 |
| `[[button]]` | 0 to 3, on GPIO 3, 4 and 5, one per pin |
| chart `min`/`max`, alarm numbers, `km_h_per_unit` | within a 32-bit float |
| reads, all channels together | 100 requests a second |

## What the build refuses

Every refusal names what failed. `vagcan dev dash build` prints it after `Error:`; the firmware
build prints it after `dash plan for VIN <VIN>:`.

**The file**

| refusal | what to do |
|---|---|
| `dash.toml: …` with a line and column | A TOML syntax error. Fix it there. |
| `line N: [[channel]] 2: unknown key "hzz" — did you mean "hz"? [[channel]] takes …` | Fix the spelling. The message lists the keys that table takes. |
| `line N: unknown section [[alarms]] — did you mean [[alarm]]? The top level takes …` | Fix the section's name. |
| `line N: [[page]] 1: "min" is a key of a chart page (kind = "chart"), and this is a values page` | Remove the key, or change `kind`. The same for an `[[alarm]]`'s two kinds. |
| `line N: [stopwatch]: "survey" is a top-level key: write it above the first section` | Move it above the first section. |
| `line N: [[page]] 1: "hz" is a key of [[channel]]` | Move it under that section. |
| `line N: [[channel]] 1: label must be a string, not an integer` | Write the type this reference gives. Any optional key, any type. |
| `no build input at <path>` | Write `~/.vagcan/dash/<VIN>/dash.toml`, or pass `--input`. |
| `vin is missing or not a string` | Add `vin = "<VIN>"`. |
| `<path> is for VIN X but the build asked for Y` | Build for X, or correct `vin`. |
| `language "xx" is not one this build has words for` | `"en"` or `"ru"`. |
| `no survey at <path>` | Run `vagcan dev survey` on the car, or set `survey`. |
| `no [[channel]]` / `no [[page]]` | Add one. A single `[channel]` or `[page]` with one pair of brackets counts as none. |
| `alarm must be written as [[alarm]] tables` | Two pairs of brackets. |
| `stalk must be one [stalk] table` / `stopwatch must be one [stopwatch] table` | One pair of brackets. |
| `<key> is missing or not a string` | Add the key, quoted. |

**Naming a channel**

| refusal | what to do |
|---|---|
| `a channel is <unit>:<text id> or <unit>:<DID>[@<bit>]` / `nothing after the unit` | Spell it as in [Naming a channel](#naming-a-channel). |
| `"X" is not a control-unit number like 01 or 17` / `is not a hex request id like 714` | Fix the unit. |
| `control unit NN … has no known request id` | Write the unit's request id instead: `vagcan units` lists them. |
| `… has no diagnostic address (700-795 or 7E0-7E7)` | Not a diagnostic unit. Fix the unit. |
| `a bit offset goes with an identifier, not a text id` | Drop the `@`, or write the identifier. |
| `"b" is not a bit offset` | A whole number after `@`. |

**Channels**

| refusal | what to do |
|---|---|
| `decimals N is not 0..=3` | 0 to 3. |
| `hz must be a number above 0 and at most 100` | Fix `hz`. |
| `hz V is too small for the board, which would hold it as 0` | A larger `hz`. |
| `setpoint is not a string` | Quote it. |
| `unit XXX is not in the survey` | Survey the car with that unit answering, or fix the unit. |
| `unit XXX: the survey has no part number (F187) for it` | Survey again. The board checks the unit by that number. |
| `X: the car's variant does not declare this channel and nothing has proven it` | This car's unit has no such row. Pick another from `vagcan watch`. |
| `X: N rows answer to it — name one by identifier and bit offset: DID@bit name, …` | Write one of the listed rows as `<unit>:<DID>@<bit>`. |
| `X: scaling is an enumeration, not linear` (or `a single proven point with no slope`) | The board shows numbers only. Pick a numeric row. |
| `X: its scaling is not a finite number the board's 32-bit float holds` | Pick another row. Also for a factor the board would hold as 0. |
| `X: the survey asked the unit for this identifier and it did not answer` | This car does not answer it. Pick another row. |
| `X is listed twice under [[channel]]` | Delete one. |
| `X and Y are the same row …; keep one [[channel]]` | Delete one. |
| `X: its setpoint Y is on another unit` | Pair channels of one unit only. |
| `X: its setpoint Y reads in "a" against the channel's "b"` | Pair a row in the same unit of measure. |
| `X: its setpoint Y is the channel itself` | Name the specified value, not the actual one. |
| `X: its setpoint Y has a setpoint of its own` | Remove one of the two. |
| `X: its setpoint Y is read at A Hz and the channel it explains at B Hz` | Give both the same `hz`. Two channels sharing one specified value need the same `hz` too. |

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
| `page #n: min A is not below max B` | Put `min` below `max`. They are compared as 32-bit floats, so two ends a hair apart are one value. |
| `page #n: min A and max B are further apart than the board's 32-bit float holds` | Narrow the scale. |
| `page #n: X already has a chart page; one range per channel` | Keep one chart of it. |
| `page #n: X is not in the [[channel]] list` | Declare it under `[[channel]]`. |
| `page #n: X is a setpoint with no [[channel]] of its own — declare it as a [[channel]] to show it` | Declare it under `[[channel]]`. |

**Alarms**

| refusal | what to do |
|---|---|
| `N [[alarm]] rules, and the board holds at most 4` | Remove rules. |
| `alarm #n has no channels list` / `watches no channels` | Add `channels`. |
| `alarm #n: X is not in the [[channel]] list` | Declare it under `[[channel]]`. |
| `alarm #n: X is a setpoint with no [[channel]] of its own — declare it as a [[channel]] to show it` | Declare it under `[[channel]]`. |
| `alarm #n: kind "x" is not "threshold" or "drift"` | Lowercase `threshold` or `drift`. |
| `alarm #n: direction "x" is not "below" or "above"` | Lowercase `below` or `above`. |
| `alarm #n needs <key>, a finite number` | Add it, a number that fits a 32-bit float. |
| `alarm #n: no values page is titled "T"` | Give a values page that `title`. |
| `alarm #n: N values pages are titled "T"` | Give them different titles. |
| `alarm #n: page "T" does not show X` | Put X on that page. |
| `alarm #n: release A is not above trip B` / `not below trip B` | Move `release` to the far side of `trip`. |
| `alarm #n: percent 0 is not above zero` (or `release_percent`) | Above 0. |
| `alarm #n: release_percent V is too small for the board, which would hold it as 0` (or `percent`) | A larger share. |
| `alarm #n: release_percent A is not under percent B` | Lower `release_percent`. |
| `alarm #n needs hold_ms, whole milliseconds the drift has to hold` | An integer, `1000` not `1000.0`. |
| `alarm #n: min_setpoint V is below zero` | 0 or more. |
| `alarm #n: X has no setpoint` | Give X a `setpoint`, or use a threshold rule. |

**`[stalk]`**

| refusal | what to do |
|---|---|
| `[stalk] read X is not <unit>:<DID>` | One identifier, four hex digits, no `@`. |
| `unit XXX is not in the survey` | `read`'s or `cruise`'s unit: survey the car with it answering. |
| `X: the survey asked the unit for this identifier and it did not answer` | `read` or `cruise` is silent on this car. Pick another. |
| `[stalk] read X: the car's variant declares no such identifier` | Find the lever's identifier (see [How to find names](#how-to-find-names-for-your-car)). |
| `[stalk] rocker: X has no field named "N" — its fields are …` | Copy a name from the list. Same for `switch`. |
| `[stalk] rocker: X has 2 fields named "N"` | The project gives the name twice, possibly differing only by a space at an end. That field cannot be named. |
| `[stalk] rocker: "N" is a quantity, not a list of states` | Name an enumerated field. Same for `switch`. |
| `[stalk] rocker and switch are both "N"` | Two different fields. |
| `[stalk] next: "S" is not a state of "F" — its states are …` | Copy a state from the list. Same for every state key. |
| `[stalk] next: "S" names 2 bands of "F" (51–101, 204–255) — the board takes a button as one band, so this state cannot be used yet` | That state cannot be a button yet. Pick another. For `switch_off` and `cruise_off` it says "an off state". |
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
- A channel an alarm watches: at its `hz`, whatever page is up.
- A specified value: with its channel.
- Any other channel on no page: not read, except the stopwatch's `speed` while the stopwatch is
  on.
- At start-up the board checks each unit's part number (`F187`) against the plan. A unit that
  answers another number is not read: after replacing a unit, survey the car again and rebuild.

**Paging**

- Pages turn from `[[button]]`s, the lever (`[stalk]`) and `dashsim`. The board's BOOT and RESET
  buttons do nothing.
- With no `[stalk]` and no `[[button]]`, the board shows its active page. Only `dashsim` or
  `dashcfg`'s `set page` changes it.
- With the stopwatch on, `next` and `previous` do nothing, from any of them. `stopwatch` leaves
  it; so does the board turning adapter (`vagcan --slcan`), and the lever (**The stopwatch**,
  below).

**The lever**

- The lever is the dash's only while `switch` reads `switch_off` and `cruise` reads
  `cruise_off`. A missing reading counts as not off.
- It is read every 50 ms while it is the dash's, every 500 ms otherwise, every 100 ms with the
  stopwatch on. The cruise status is read every 200 ms.
- A press is two readings in a row in the same state: hold about 0.1 s, 0.2 s with the
  stopwatch on. Hold longer to be sure. Holding does not repeat.
- `next` and `previous` wrap round the pages. With the stopwatch on they do nothing.
- While an alarm holds the screen, any press silences it instead.

**The stopwatch**

- Standing still for 1 s arms it. The first moving reading starts the run.
- Times run from the launch, fitted to the first 400 ms of movement.
- The run ends at the highest mark. Stopping before it aborts the run: its times show until you
  leave the page and are never stored.
- While armed or running, the other channels on the page drop to once a second. Alarm channels
  keep their rate.
- A run with no times (the speed read too slowly for the launch fit) is not kept.
- A finished run is kept in the board's settings. It is written to flash when the car next
  stands still for 1 s with the stopwatch on, before `GO`: the stopwatch arms right after the
  write. Or on `save` in `dashcfg`. Until then `get` says a run is in RAM only.
- On a board whose flash holds no settings (never saved, or erased), the run is written with the
  default settings. Changes you have not saved stay unsaved.
- Leave the stopwatch before the car stops and the run is lost at power-off, unless you `save`.
- A run that ends short of its highest mark shows `ABORT` and is not kept.
- With a `[stalk]`, cruise switched on closes the stopwatch: two readings in a row with `switch`
  not at `switch_off` or `cruise` not at `cruise_off`. A run under way is dropped. However the
  stopwatch was opened.
- With a `[stalk]`, the lever unreadable for over 3 s closes it too: no answer, a stale cruise
  status, a reading no state claims, its unit silent. A shorter gap leaves it and a run alone.

**Alarms**

- A rule watches its channels whatever is on screen, the stopwatch included.
- When one goes wrong, the rule's page comes up with that cell blinking, 0.4 s on and 0.4 s off,
  while the value is out.
- After the value clears, the page stays 2.5 s with the cell steadily inverted. Then the screen
  goes back to where it was.
- A press — a `[[button]]`, the lever or `dashsim` — silences that episode until the value
  clears. With none of them, only the value clearing ends it.
