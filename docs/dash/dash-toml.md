# `dash.toml` — what the dash board shows

The board resolves nothing for itself. Which unit it asks, which identifier, how the bytes
decode, what the channel is called, how often it is read, which pages exist and which alarms
watch what — all of it is decided on the laptop and written into a plan the firmware links.

`dash.toml` is the input to that. One file per car:

```
~/.vagcan/dash/<VIN>/dash.toml
```

Write it by hand. Nothing in it is guessed: a channel the car's variant does not declare, or
one the car was asked for and did not answer, fails the build and the message names it.

## Build it

| what you want | run |
|---|---|
| see the plan and the reasons, no firmware | `vagcan dev dash build <VIN>` |
| build the firmware for that car | `VAGCAN_DASH_VIN=<VIN> cargo build --release --bin dash` in `crates/dash/vag-dash-fw` |
| check the firmware compiles, no car | `VAGCAN_DASH_NO_CAR=1 cargo build --release --bin dash` (an empty plan — do not flash it) |

Both paths run the same generator, so `vagcan dev dash build` is for reading the result, not a
step before the firmware build.

- `VAGCAN_DASH_VIN` picks the car. With exactly one car under `~/.vagcan/dash/`, it can be left
  unset.
- `VAGCAN_PROJECT` (or `--project`) picks which project's catalogs to resolve against.
- `--input <FILE>` builds from another file; the outputs still land under the car.

Outputs, both under `~/.vagcan/dash/<VIN>/`:

| file | for |
|---|---|
| `plan.json` | reading, and the simulator |
| `plan.rs` | the firmware, which `include!`s it |

Neither is committed anywhere: they are derived from VW's data and describe one car.

## The file

```toml
vin = "XW8AD4NE9JH008917"
language = "ru"                  # optional; the setting's language otherwise
survey = "…/survey.jsonl"        # optional; ~/.vagcan/cars/<VIN>/survey.jsonl otherwise

[[channel]]
ref = "01:IDE00025"              # which unit, which row
label = "ОЖ"                     # optional; the glossary's wording otherwise
decimals = 0                     # optional; derived from the scaling otherwise
hz = 10                          # optional; readings a second while shown, 2 otherwise
setpoint = "01:IDE00190"         # optional; what the unit asked for, on the same unit

[[page]]
kind = "values"
title = "MAIN"                   # an alarm raises a page by this title
cells = ["01:IDE00025"]          # one to four channels

[[page]]
kind = "chart"
cell = "01:IDE00025"
min = 70                         # the scale is fixed, never autoscaled
max = 110

[[alarm]]
channels = ["01:IDE00025"]       # each under [[channel]], all shown on `page`
page = "MAIN"
direction = "above"              # or "below"
trip = 105                       # fires at or past this
release = 100                    # clears only once back past this

[[alarm]]
kind = "drift"                   # the other kind: distance from a specified value
channels = ["01:IDE00191"]       # each with a `setpoint` of its own
page = "MAIN"
percent = 10                     # fires past this share of the specified value
release_percent = 6              # clears under this share
hold_ms = 1000                   # and only once the drift has held that long
min_setpoint = 0.5               # under this specified value the rule says nothing
```

## Naming a channel

Two spellings, both `<unit>:<row>`:

| spelling | example | when |
|---|---|---|
| text id | `01:IDE00025` | the usual one — stable across variants, and what the glossary is written under |
| identifier | `02:380A`, `02:3816@3` | a row with no text id: a proven one, or a standard OBD-II parameter. `@` is the bit offset |

The unit is spelled as everywhere else: `01`, `02`, or a request id such as `7E0`. Which
variant the car has is not written here — it comes from the survey.

## A specified value

Several channels come in pairs: what the control unit asked for and what it got. `setpoint`
pairs them, and then the panel draws the difference under the number, and a `kind = "drift"`
alarm can watch it.

- The pair lives on **one unit**. Both identifiers are then asked for in one request, so the
  two numbers come from the same moment.
- The specified value needs no `[[channel]]` of its own — the build adds it, at its channel's
  rate — and it is never offered as a cell. Declaring it as well is allowed; it is added once,
  whichever spelling each of them uses. Its `hz` must then be the channel's: two rates are two
  moments, and the build refuses that.
- The pair reads in one unit of measure, or the build refuses it.
- A channel with no `setpoint` shows exactly what it always did.

## Alarms

An alarm watches its channels whatever page is up and, when one goes wrong, shows its page with
the offending cell inverted. The cell blinks (0.4 s on, 0.4 s off) while the value is out. The
view is held 2.5 s after the value comes back, with the cell steadily inverted. A short press
silences that episode. At most four rules, in the file's order, which is their priority.

| key | threshold rule | drift rule |
|---|---|---|
| `kind` | absent, or `"threshold"` | `"drift"` |
| `channels` | one or more, all shown on `page` | the same, and each needs a `setpoint` |
| `page` | the title of a values page | the same |
| `direction` | `"below"` or `"above"` | — |
| `trip` / `release` | fires at `trip`, clears past `release` | — |
| `percent` / `release_percent` | — | the share of the specified value it is away from |
| `hold_ms` | — | how long that has to hold, without a break, before the screen moves |
| `min_setpoint` | — | below this specified value the rule says nothing |

`hold_ms` is what makes a drift rule usable on a turbocharger: boost lags its own setpoint on
every throttle stab, and without the hold the rule would fire on every gear change.
`min_setpoint` is the other half — a percentage of a specified value near zero is noise.

## The cruise lever: `[stalk]`

With cruise off, the cruise lever pages the panel: one state next page, one previous, one turns
the stopwatch on and off. With cruise on, the lever is the car's and the dash ignores it.

```toml
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
```

The names above are one car's (Škoda Octavia III). Take yours from your car's project. Every
key is required. Field and state names are the project's, **exactly and in full** — parenthesis
and all.

| key | what |
|---|---|
| `read` | `<unit>:<DID>` — the one identifier that carries the rocker and the switch |
| `rocker` | the field of `read` that the lever moves |
| `switch` | the field of `read` with the cruise main switch |
| `next` / `previous` | rocker states: next page, previous page |
| `measure` | rocker state: the stopwatch on or off |
| `switch_off` | the switch's state that means off |
| `cruise` | the engine's cruise status, an enumerated field, spelled as a channel |
| `cruise_off` | its state that means off |

The lever is used only when `switch` reads `switch_off` **and** `cruise` reads `cruise_off`. A
press is two reads of the same state: about 0.1 s, or 0.2 s with the stopwatch on. Hold longer
to be sure. The lever's read rates are fixed, not taken from `hz`.

## The stopwatch: `[stopwatch]`

Times 0 to each mark in km/h, on the board. Needs `[stalk]`: its `measure` state opens the page.

```toml
[stopwatch]
speed = "02:380B"
km_h_per_unit = 0.0
marks = [60, 100]
```

| key | what |
|---|---|
| `speed` | a channel under `[[channel]]`, with `hz` above 5 — 50 recommended. Its scaling has no offset and a factor above 0 |
| `km_h_per_unit` | km/h per unit of `speed`, measured on the car. `0` until measured: the page shows `NO FACTOR` and times nothing |
| `marks` | 1 to 3 whole speeds in km/h, above 0, each once |

A finished run's times are kept in the board's settings. They are written to flash when the car
next stands still for 1 s with the stopwatch on, or when you type `save` in `dashcfg`. Leave the
stopwatch before the car stops and the run is lost at power-off unless you `save`.

## Limits

| | |
|---|---|
| pages | 8 |
| cells on a values page | 1 to 4 |
| alarms | 4 |
| `hz` | above 0, at most 100; 2 when absent |
| stopwatch `marks` | 1 to 3 |
| `decimals` | 0 to 3 |
| `label` | ten characters on a four-column page; longer is drawn and reported |

## What the build refuses

Every refusal names the thing that failed:

- a unit the survey has nothing about, or one with no `F187` to check the board against;
- a channel the resolved variant does not declare, one that is ambiguous, or one whose scaling
  is not linear (an enumeration, or a single proven point with no slope);
- a channel the car was asked for in the survey and did not answer;
- the same channel twice;
- a page naming a channel that is not under `[[channel]]`, a values page with no cells or more
  than four, a chart whose `min` is not below its `max`, a second chart of the same channel, or
  more pages than the board holds;
- an alarm whose page is not a values page showing every channel it watches — two pages under
  one title is the same refusal — one whose `release` is on the wrong side of its `trip`, one
  over a channel with no `setpoint`, or an unknown `kind`;
- a `setpoint` on another unit, in another unit of measure, pointing at its own channel, at a
  channel that has a `setpoint` of its own, or read at a different `hz` from the channel it
  explains — including when two channels share one specified value and ask for it at two rates;
- a `[stalk]` name that is not the project's field or state, a quantity where a list of states
  is wanted, a field or state name the project gives twice, a `read` or `cruise` the car did not
  answer in the survey, a `cruise` that matches several rows, two buttons on one state, or a
  cache from before the project's state bands (run `vagcan setup` again);
- a `[stopwatch]` speed not under `[[channel]]`, with an offset, with a factor at or below 0, or
  read at 5 Hz or slower; a `km_h_per_unit` below 0, or above 0 but too small for the board; a
  mark of 0, a mark twice, or more than 3.

A channel is compared by what it resolves to, never by how it is spelled:

- `01:IDE00191` and `01:202A` are one channel. Declaring both is refused, naming both; a page
  or an alarm may use either spelling for a channel declared under the other.
- A `setpoint` that reads the same unit, identifier and bits as its channel is "the channel
  itself".
- The label data offers one row per field — a unit, an identifier and its bits — so one field is
  never two channels with two scalings.
