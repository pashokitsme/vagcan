# `vagcan race` — acceleration runs — design

**Date:** 2026-08-03
**Status:** approved (design), not yet implemented
**Depends on:** the `watch` polling stack (`crates/vagcan/src/watch/plan.rs`, `poll_batch`),
the proven catalogs under `catalogs/vehicles/`, and the standard OBD-II table in
`vag_data::obd`.

## Why

Everything needed to time an acceleration run is already on the car and already proven:
road speed at 0.01 km/h from the gearbox, engine speed, the engaged gear, the selector
lever, the accelerator pedal, and the legislated OBD-II parameters for air mass, ambient
temperature and barometric pressure. What is missing is the one thing a driver actually
asks for — *how long did 0 to 100 take, and where did it lose the time*.

A phone app answers that from GPS and knows nothing about the car. This tool has the
opposite problem and the opposite advantage: its speed is the car's own, quantised finely,
and it can put the gear, the pedal and the boost on the same time axis as the stopwatch.

## Scope

| In | Out |
|---|---|
| `vagcan race` — armed stopwatch over a live poll loop | GPS, or any external reference |
| user-defined marks (`0-100`, `50-100`, …) | a "dyno" claim — the power figure is an estimate and says so |
| live TUI: a value table and one chart at a time | drag-strip conventions (rollout, 1/4-mile traps) — later, if wanted |
| a session of several runs, saved as one raw JSON | writing anything to a control unit |
| `--view FILE.json` — a self-contained HTML chart page | a server, a bundler, or any external asset |
| proven channels only | discovering measurements — that is what `survey` and `watch --survey` are for |

`race` changes no diagnostic session (`0x10 0x03` is never sent), sweeps nothing, and reads
a fixed handful of known identifiers. `SAFETY.md`'s sweep gate does not apply: this is
`watch` with a stopwatch, not a fuzz test, and driving is the point of the command rather
than a hazard to refuse.

## Two kinds of number, and only two

Every figure this command shows is either **read** — a value that was on the bus and whose
meaning is proven, from a catalog row or from the SAE J1979 table — or **derived**, computed
here from read values. Nothing else is admitted.

**A number that was never on the bus must not look like one that was.** In the tables that
means a stated origin, in the charts a different line style, in the file an explicit field.

The third class the rest of this project deals with — bytes that were read but whose meaning
nobody has proven, rendered `(raw)` by `Channel::render` and suffixed `_raw` in the `watch`
CSV — **does not exist in `race`**. `watch` and `survey` show raw bytes because their job is
to *find* measurements. `race` is an instrument, not a search: an unproven byte cannot be
timed, integrated or differentiated, so there is nothing for it to do here.

That has a consequence worth stating plainly: **`race` refuses to start on a car whose
catalogs do not cover it.** After the units identify themselves, every required channel is
resolved by name from the catalog store or the standard table, and a missing one is a fatal
error naming the channel and the unit — not an empty column.

| channel | required? | if missing |
|---|---|---|
| speed (leading) | yes | refuse — there is no stopwatch without it |
| engine speed, gear, pedal | yes | refuse — a run with none of these explains nothing |
| boost, specified and actual | no | the series is absent |
| air mass, shaft speeds, selector | no | the series is absent |
| barometric pressure, ambient temperature | no | **power is not computed** — air density has no source |

A missing channel means the row is not there. It never means raw bytes, and it never means a
guessed value.

An unmapped value on a *proven* channel is a third thing again, and not the same as an
unproven channel: if the selector answers a code outside the P/R/N/D table, it shows as
`unknown code 07`, is excluded from every derived figure, and is stored as the byte with a
flag. That is an admission, not a claim.

## CLI

```
vagcan race [--device PATH]
            [--marks 0-10,0-25,0-50,0-60,0-80,0-100]
            [--mass KG] [--cda M2] [--crr N] [--inertia N]
            [--accel-window SECONDS] [--speed-scale N]
            [--hz N] [--out FILE] [--catalogs DIR]
            [--view FILE.json]
```

`--marks` takes `A-B` pairs in km/h, comma-separated, `A < B`. The default is
`0-10,0-25,0-50,0-60,0-80,0-100`.

`--view` reads a saved session and opens a chart page; it touches no adapter. The precedent
is `survey --diff`, which is likewise an offline mode of a command that otherwise needs the
car — the command stays where the user looks for it.

## 1. The run state machine — `race/session.rs`

Pure state and arithmetic, no I/O, no adapter, no terminal. Everything below is testable
against a synthetic speed profile.

```
Idle ──speed 0 held for 1 s──► Armed ──first sample v > 0──► Running
  ▲                              │                             │
  │           p toggles the trigger off (traffic lights)        │
  └────────────────── Esc cancels ──────────────────────────────┘
                                                                │
        Finished ──car stands still again──► Armed  ◄────────────┘
```

- **Standstill** is speed exactly zero on the leading channel, held for one second. The
  hold is what stops a crawling stop-and-go from arming the trigger between every gap.
- **The start** is the first non-zero sample after that.
- **t0** is not that sample. It is a backward linear extrapolation to `v = 0` from the
  first two non-zero samples, clamped to the interval `(last zero, first non-zero]`. Taking
  the first non-zero sample as the start throws away up to a whole polling cycle, which at
  ~20 Hz is 50 ms on a number quoted to hundredths.
- **A ring buffer of the 3 s before t0** is written into the run: pedal, engine speed and
  selector *before* the start are half of what explains a bad one.
- A run ends at the highest mark, at `Esc`, or when speed returns to zero. The last is
  recorded as `aborted`, and the marks that did close are kept — a run that died at 80 still
  measured 0-60.
- After a run the result stays on screen and the trigger re-arms by itself once the car is
  standing still again, so a session is a sequence of runs in one file.

Keys: `p` pauses the trigger, `Esc` cancels the current run, `s` saves the session, `←`/`→`
change which series the chart shows, `q` quits.

## 2. Polling — `race/mod.rs`

Two batches per cycle, both within `plan::BATCH` (8 identifiers, the measured per-request
limit on this car):

| batch | unit | identifiers |
|---|---|---|
| leading | gearbox `7E1` | speed `F40D`, gear `3816`, selector `3809`, shafts `380A`/`380B`, pedal `3804` |
| background | engine `7E0` | engine speed `206E`, boost `2029`/`202A`, air mass `F410`, speed `F40D` |

The leading batch is polled every cycle; the background batch every second cycle. Marks are
timed from the leading speed alone, so its sample rate is what matters and it gets twice the
rate of everything else. The engine's own road speed is read as a cross-check and stored,
never used for timing.

Which identifiers these are comes from the catalogs by name, exactly as
`plan::select_basics` already does — a car whose catalog uses the same words works, and a
car with no catalog gets an error naming what it could not find rather than a wrong number.

Every value carries its own timestamp, as in `watch --out`. Batches are separated in time,
and one shared timestamp has already corrupted evidence on this project once (the gear
proof moved from η² 0.872 to 0.972 when the columns got their own clocks).

The achieved rate is measured and written into the file. It is never asserted in advance.

## 3. What is computed — `race/session.rs`, `race/power.rs`

**Marks.** `t(B) − t(A)`, where `t(v)` is linearly interpolated between the two samples
that bracket the crossing. Both crossings must happen in the same run, in a monotonically
rising pass.

**Average acceleration per mark.** `Δv / Δt` across the mark's own endpoints. This is the
only acceleration figure here that is *measured* rather than differentiated — no smoothing,
no window, no lag. It sits in the results table next to the time.

**Instantaneous acceleration.** A difference quotient over `--accel-window` (default 0.3 s),
reported in m/s² and in g (`g = 9.80665 m/s²`, the SI definition, not a property of any
car). Smoothing is not optional: speed is quantised to 0.01 km/h and the samples jitter in
time, so a raw sample-to-sample difference is noise with the signal buried in it.

The window is applied two different ways, and the difference is load-bearing:

| where | method | why |
|---|---|---|
| live, on the TUI | **causal** — the trailing window only | the future half of a centred window does not exist yet |
| the results table, the JSON, the chart page | **central** — symmetric window over the finished run | a causal estimate lags by about half the window and clips the peak |

This forces a storage rule: **the file holds raw speed samples, and every derivative is a
separate, labelled layer recomputed in one pass over the complete run.** Numbers shown live
never reach the file. Without that rule the same run reports two different peaks depending
on whether it was read off the screen or out of the JSON.

**Peak acceleration.** Maximum of the central-method series, with the time and the gear it
happened in.

**Shifts.** A DSG upshift is a dip in acceleration, and its length is what the gear costs.
Each dip below a fraction of the run's peak is recorded as
`{ t, from, to, dip_seconds, speed_lost }` alongside the plain `gear_changes`.

**Distance.** Trapezoidal integration of speed from t0. Approximate, and labelled so.

**Power.** Wheel power from the dynamics:

```
P = (m·k·a  +  ½·ρ·CdA·v²  +  m·g·Crr) · v
```

Air density comes from the car itself rather than a constant: `ρ = p / (R·T)` with `p` the
barometric pressure (OBD-II PID 0x33), `T` the ambient air temperature (PID 0x46) and
`R = 287.05 J/(kg·K)` for dry air. `--mass` has **no default** — a mass is a property of one
specific car, and this project does not put those in code; without it the power column is
empty. `--cda`, `--crr` and `--inertia` carry generic documented defaults. Every power
figure is labelled an estimate at the wheels.

**Kickdown.** If the unit's catalog holds a row whose name contains `kickdown`, that is
used. Otherwise it is derived from the pedal (≥ 99 %) and labelled derived. No identifier
for it has been proven on this car, and a column that silently guesses is worse than an
empty one.

**Gearbox mode.** The selector lever from the catalog: P/R/N/D are proven. **D versus S
versus manual is not** — it is open work in `todo/README.md` (the stimulus was never given
during the recording that identified the lever). A code outside the proven table shows as
`unknown code 07` and enters nothing.

## 4. The live view — `race/mod.rs`

`ratatui::widgets::Chart` with `Dataset` and `Marker::Braille`. The dependency is already in
`crates/vagcan/Cargo.toml`; `textplots` and `rasciigraph` would render worse and add a
crate.

```
  RUN 4.31 s                                     marks
  ┌──────────────────────────────────────────┐  0-10   0.98 s
  │ speed    62.4 km/h    bus                │  0-25   2.11 s
  │ engine   4310 /min    bus                │  0-50   4.03 s
  │ gear     3            bus                │  0-60   ·
  │ pedal    100 %        bus                │  0-80   ·
  │ boost    1.71 / 1.62 bar   bus           │  0-100  ·
  │ accel    0.41 g       computed, trailing │
  │ power    164 hp       computed, estimate │
  └──────────────────────────────────────────┘
  ┌── speed ── ← → to change ────────────────┐
  │                                ╱─────    │
  │                      ╱────────╱          │
  │            ╱────────╱                    │
  └──────────────────────────────────────────┘
    0s        2s        4s        6s
```

**The table carries every value; the chart carries one.** A table of ten rows is readable
and a chart of ten series is not, so the chart shows a single series at a time, switched
with the arrow keys and named in its own border. Speed opens; acceleration is next along.

**Each row states where its number came from** — `bus` or `computed` — in a column of its
own. Derived rows carry the qualifier that matters for reading them: the live acceleration
is `trailing`, because live it can only be causal, and power is an `estimate`.

The chart is drawn from the accumulated run buffer, not from the last point alone. When the
series shown is a derived one, its running end is causal by construction and the border says
so.

With no terminal — a pipe, a log, an agent — the same loop runs and prints rows instead of
drawing, exactly as `watch::View::Plain` does.

**The results table — `race/report.rs`.** Shown when a run finishes or is cancelled, in two
blocks under their own headings rather than one list:

```
  Run 2 — measured
    mark     time      average acceleration
    0-10     0.98 s    2.83 m/s²
    0-100    6.12 s    4.54 m/s²
    peak engine speed  6480 /min at 5.9 s
  Run 2 — computed   (mass 1400 kg, CdA 0.65 m², Crr 0.012, window 0.30 s, central)
    distance          118.4 m
    peak acceleration 5.31 m/s²  (0.54 g) at 1.21 s, gear 2
    peak power        171 hp     estimate at the wheels
    shift 2→3         0.31 s dip at 2.44 s
```

The measured block holds times, the average accelerations that are `Δv/Δt` across a mark's
own endpoints, and peaks of channels the car reported. The computed block carries its
conditions in the heading: the same run under a different mass is a different set of numbers,
and a table that hides that invites the comparison it cannot support.

## 5. The saved session — raw JSON

```json
{ "tool": "vagcan race", "recorded_at": "2026-08-03T12:41:07+03:00",
  "car":      { "vin": "…", "units": [ { "request": "7E1", "part_number": "0CW300041G" } ] },
  "config":   { "marks": [[0,100]], "mass_kg": 1400, "cda": 0.65, "crr": 0.012,
                "inertia": 1.0, "speed_source": "7E1:F40D", "speed_scale": 1.0,
                "accel_window_s": 0.3, "accel_method": "central", "hz": 21.4 },
  "channels": [ { "key": "speed", "name": "Vehicle speed", "unit": "km/h",
                  "origin": "read", "request": "7E1", "did": "F40D" },
                { "key": "accel", "name": "Acceleration", "unit": "m/s2",
                  "origin": "derived", "from": ["speed"],
                  "method": "central-difference", "window_s": 0.3 },
                { "key": "power", "name": "Power at the wheels", "unit": "hp",
                  "origin": "derived", "estimate": true,
                  "from": ["speed", "barometric_pressure", "ambient_temperature"],
                  "method": "road-load" } ],
  "runs":     [ { "index": 1, "t0_wall": "…", "aborted": false,
                  "samples": [ { "t": -2.94, "speed": { "t": -2.94, "v": 0.0 } } ],
                  "marks":   [ { "from": 0, "to": 100, "seconds": 6.12,
                                 "avg_accel_ms2": 4.54 } ],
                  "derived": { "distance_m": 118.4, "peak_rpm": 6480, "peak_hp": 171,
                               "peak_accel_ms2": 5.31, "peak_accel_t": 1.21,
                               "peak_accel_gear": "2",
                               "shifts": [ { "t": 2.44, "from": "2", "to": "3",
                                             "dip_seconds": 0.31, "speed_lost": 0.0 } ] } } ] }
```

Every channel declares its `origin`. A derived one also declares what it was derived
**from** and by what **method**, with the parameters that method used — an acceleration
figure whose window is not recorded is not reproducible, and a power figure whose mass is
not recorded is not checkable. There is no `proven` flag: in `race` it would be `true` on
every row, since nothing else gets in.

A value that a proven channel returned but whose meaning is not in its table — an
unmapped selector code — is stored as `{ "raw": "07", "unmapped": true }` and enters no
derived figure.

`--out` writes continuously; `s` writes on demand. Both write the same document.

## 6. The chart page — `race/view.rs`

`--view FILE.json` generates a self-contained HTML file next to the input and opens it.
Inline SVG and inline JavaScript, no CDN, no external asset of any kind — a strict rule,
and a test asserts the output contains no `http://` or `https://`.

A run has more channels than any one chart can carry, and a wall of checkboxes makes that
the reader's problem. Two switches make it the page's.

**Switch 1 — the X axis.** What the run is plotted *against* is a question of its own, and
the answers are not interchangeable:

| axis | what it shows |
|---|---|
| time | the measurement itself: seconds to each mark, where the dips are |
| speed | the shape of the run with time divided out — two attempts compare directly |
| distance | how many metres the launch cost |
| engine speed | the run by revs — power against engine speed is the familiar dyno curve |

**Switch 2 — the series set.** Groups that are worth reading together, at most three series
and two Y axes each:

| set | series |
|---|---|
| Run | speed + acceleration |
| Engine | engine speed + boost specified/actual + air mass |
| Driver | pedal + kickdown + gear |
| Transmission | gear + shaft speeds + speed |
| Power | power + acceleration |
| Custom | the checkboxes, for a cut nobody anticipated |

Sets are named by channel key, not by identifier, and a set whose channels this car does not
have is not offered — which follows from the pre-flight check with no extra logic.

The rest:

- A cursor line with a tooltip reading every shown series at that point. This is the
  requirement the page exists for: any value, at any moment.
- **Read series are drawn solid, derived series dashed**, and the legend separates them
  under two headings. Power's tooltip carries the conditions it was computed under — mass,
  CdA, Crr, air density — because a power figure without them is not a figure.
- Background discriminators, one at a time: **by gear**, **by mark**, **by kickdown**, **by
  acceleration dip** (the shifts, without reading the gear row), or none. They work on any X
  axis — on the speed axis a gear change is still a vertical line, just at a km/h.
- A run selector when the session holds several.

Everything drawn comes from the recomputed derivative layer, so the page and the results
table cannot disagree.

## 7. Files

```
crates/vagcan/src/race/mod.rs      the command, the poll loop, the TUI
crates/vagcan/src/race/session.rs  state machine, marks, derived metrics — no I/O
crates/vagcan/src/race/power.rs    the dynamics model and air density
crates/vagcan/src/race/report.rs   the results table
crates/vagcan/src/race/view.rs     HTML generation
```

`session.rs` holds everything worth testing and knows nothing about adapters or terminals,
which is what makes the tests possible without a car.

## 8. Tests

| test | asserts |
|---|---|
| arming | a synthetic profile arms only after zero is held a full second |
| t0 | the extrapolated start matches a known-acceleration profile to within a sample |
| marks | interpolated `0-100` on an analytic profile equals the closed-form answer |
| abort | speed returning to zero keeps the marks that closed and flags the run |
| re-arm | a second standstill starts a second run in the same session |
| trigger pause | `p` prevents arming and does not lose the previous run |
| `--marks` parser | `0-100,50-100` parses, `100-50` and `abc` are rejected |
| causal vs central | the two methods differ on a known profile and the file holds the central one |
| air density | ρ from a known pressure and temperature matches the ideal-gas value |
| no mass | the power column is empty rather than defaulted |
| page | the generated HTML contains the sample data and no external URL |
| session hygiene | the run issues no `0x10` session change |
| pre-flight | a catalog store missing the speed channel refuses the run and names it |
| optional channels | no barometer means no power column, and the run still happens |
| origin | every channel in the file declares `origin`, and every derived one its method and parameters |
| unmapped code | a selector value outside the table is stored flagged and reaches no derived figure |
| series sets | a set whose channels this car lacks is not offered on the page |

## Open questions deliberately left open

- **D / S / Tiptronic** cannot be reported until the drive mode is identified on the car
  (`todo/README.md`, open work item 4). The column exists and reads from the catalog; today
  it shows the selector lever only.
- **Kickdown** has no proven identifier. The derived-from-pedal value is a stopgap, marked
  as such, and should be replaced the first time a survey run with a deliberate kickdown
  isolates the real one.
- **Speedometer error.** Bus speed is the speedometer's, which on this platform reads
  optimistically. `--speed-scale` exists for a user who has compared against GPS; the
  default is 1.0 and the file records which was used. This tool does not invent a
  correction factor.
