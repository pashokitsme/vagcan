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
  rate — and it is never offered as a cell. Declaring it as well is allowed; it is added once.
- The pair reads in one unit of measure, or the build refuses it.
- A channel with no `setpoint` shows exactly what it always did.

## Alarms

An alarm watches its channels whatever page is up and, when one goes wrong, shows its page with
the offending cell inverted. A short press silences that episode; the view is held 2.5 s after
the value comes back. At most four rules, in the file's order, which is their priority.

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

## Limits

| | |
|---|---|
| pages | 8 |
| cells on a values page | 1 to 4 |
| alarms | 4 |
| `hz` | above 0, at most 100; 2 when absent |
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
- a `setpoint` on another unit, in another unit of measure, pointing at its own channel, or at
  a channel that has a `setpoint` of its own.
