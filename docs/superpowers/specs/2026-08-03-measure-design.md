# `vagcan measure` — acceleration runs — design

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
opposite problem and the opposite advantage: its speed is the car's own — finely quantised,
though its accuracy and update rate are set by the sensor rather than by the resolution — and
it can put the gear, the pedal and the boost on the same time axis as the stopwatch.

## Scope

| In | Out |
|---|---|
| `vagcan measure` — armed stopwatch over a live poll loop | GPS, or any external reference |
| user-defined marks (`0-100`, `50-100`, …) | a "dyno" claim — the power figure is an estimate and says so |
| live TUI: a value table and one chart at a time | drag-strip conventions (rollout, 1/4-mile traps) — later, if wanted |
| a session of several runs, saved as one raw JSON | writing anything to a control unit |
| `--view FILE.json` — a self-contained HTML chart page | a server, a bundler, or any external asset |
| proven channels only | discovering measurements — that is what `survey` and `watch --survey` are for |
| `measure setup`: the car described once, then its road load measured on the road | asking for a number before every run that nobody knows off the top of their head |

`measure` changes no diagnostic session (`0x10 0x03` is never sent), sweeps nothing, and reads
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
to *find* measurements. `measure` is an instrument, not a search: an unproven byte cannot be
timed, integrated or differentiated, so there is nothing for it to do here.

That has a consequence worth stating plainly: **`measure` refuses to start on a car whose
catalogs do not cover it.** After the units identify themselves, every required channel is
resolved by name from the catalog store or the standard table, and a missing one is a fatal
error naming the channel and the unit — not an empty column.

| channel | required? | if missing |
|---|---|---|
| speed (leading) | yes | refuse — there is no stopwatch without it |
| engine speed, gear, pedal | yes | refuse — a run with none of these explains nothing |
| boost, specified and actual | no | the series is absent |
| air mass, shaft speeds, selector | no | the series is absent |
| barometric pressure, ambient temperature | no | **power is not computed** unless `--air-density` is given — density has no source |

A missing channel means the row is not there. It never means raw bytes, and it never means a
guessed value.

An unmapped value on a *proven* channel is a third thing again, and not the same as an
unproven channel: if the selector answers a code outside the P/R/N/D table, it shows as
`unknown code 07`, is excluded from every derived figure, and is stored as the byte with a
flag. That is an admission, not a claim.

Read and derived classify **channels** — series that vary through a run. The car's mass, its
`CdA`, its tyre size are **parameters**: constants a derived channel is computed with. They
are not series and they are not on the bus, so they live in the profile and in `config`, each
with its own provenance — `stated`, `coastdown`, `derived-from-tyre` (§0). The two questions
stay separate: *was this number on the bus* is about channels, *where did this constant come
from* is about parameters. What both answers have in common is that neither may be a guess.

## CLI

```
vagcan measure [--device PATH] [--car FILE] [--full] [--minimal]
            [--marks 0-10,0-25,0-50,0-60,0-80,0-100]
            [--accel-window SECONDS] [--out FILE] [--catalogs DIR]

vagcan measure setup [--device PATH] [--coast-from 120] [--coast-to 40]
                                     describe this car once, then measure its road
                                     load on the road. Writes the car file.
vagcan measure view FILE.json           open a saved session as a chart page

overrides, all of which normally live in the car file:
            [--mass KG] [--tyre 205/55R16] [--cda M2 --crr N] [--inertia-factor N]
            [--grade PERCENT] [--headwind M_S] [--air-density KG_M3]
            [--speed-scale N]
```

`setup` and `view` are **subcommands, not flags**. `--view` is a different input to the same
job and could have been a flag — `survey --diff` is that precedent — but `setup` is a
different command: it prompts, it runs a driving script, it takes flags that mean nothing
elsewhere, and it produces a different artefact. The repository already groups that way with
`recording` and `vcds`.

**The ordinary invocation is `vagcan measure` with no flags at all**, and `vagcan measure --full`
once the car has been set up. Everything the model needs either comes from the car, or was
answered once by `measure setup` and measured by the coastdown it ends with, and lives in the
car file (§0). The override flags exist for a one-off — a loaded boot, a different set of
wheels — and what was used is recorded in every file. None of them has a generic default: a
parameter that cannot be had honestly is one the run does without, which is why power is
opt-in and gated on a car file.

**There is no `--hz`.** §2 promises the rate is measured and never asserted in advance, and a
flag that throttles a stopwatch could only make it worse. The leading/background cadence is
the only rate control, and the achieved rate is reported.

The argument relationships are clap's job, not prose, so that a bad combination fails before
the adapter is opened — the reason `duration_arg` exists in `main.rs` today:

| flag | rule |
|---|---|
| `--cda` / `--crr` | each `requires` the other: the fit produces them as a pair |
| `--air-density` | `requires = "full"`: it feeds power and nothing else |
| `--minimal` | `conflicts_with = "full"` |
| `--coast-from` / `--coast-to` | belong to `setup` only |
| `--marks` | a `value_parser` that rejects `100-50`, `abc` and an empty list at parse time |
| `--speed-scale` | a `value_parser` that rejects zero, negatives and non-finite values |

`--marks` takes `A-B` pairs in **km/h**, comma-separated, `A < B`. The default is
`0-10,0-25,0-50,0-60,0-80,0-100`. The unit is stated in the flag's own help and in the
results table header, because `0-60` is the American figure and that one is in mph — an
ambiguity that would otherwise sit unnoticed in the default list.

`--speed-scale` is applied **before** mark detection, not to the printed result: otherwise
`0-100` would mean an indicated 100 rather than a corrected one, and the correction would
silently not apply to the thing it was set for. Which was used is recorded in the file.

`--full` asks for the power column, and is the only thing that does. It requires a car file
completed by `measure setup` (§0) and is **refused** without one, naming what is missing —
it never quietly degrades to generic numbers. Without it the run is the default one:
telemetry and times, no power, and no polling of the channels that exist only to feed the
power model. Leaving it off is also how a run buys sample rate back when the times matter
more than the wattage.

`--tyre` is the only flag that describes the car's hardware. The rolling radius it gives is
no longer needed by the power model — the exact engine-side inertia term (§3) cancels it —
but it is what turns the generic inertias into this car's coefficients, and it is what a
ratio sanity check needs.

`measure view` reads a saved session and opens a chart page; it touches no adapter. It stays
under `measure` rather than moving to the offline groups because it is the other half of the
same job, and `survey --diff` is the precedent for an offline mode living on a live command.

## 0. Knowing the car — `measure setup` and the car file

A first draft of this design asked the driver for seven numbers. That is a design failure
dressed as configurability: nobody knows their car's `CdA`, and asking for air density is
absurd when the car carries a barometer. Sorted by where each figure actually comes from:

| figure | where it really comes from |
|---|---|
| air density | **the car already knows** — PID 0x33 and PID 0x46, read every run. `--air-density` is a fallback for a unit that lacks them, not an input |
| road speed, engine speed, gear, pedal, boost | the car, every run |
| mass | the car does not know it. The registration document does — asked **once** |
| tyre size | the sidewall — asked **once** |
| `CdA`, `Crr` | nobody knows theirs. **Measured on the car** by a coastdown, once |
| speed correction | GPS, or possibly the odometer (below). Optional, once |

So the car needs a file of its own, written once.

**`vagcan measure setup`** is the whole of it — one command that starts parked and ends on the
road:

1. **Identifies the car** — VIN from the engine, part numbers and component strings from
   every unit the gateway lists. The VIN is the file's key: the car names itself and its file
   is found by that name.
2. **Runs the pre-flight channel check** and prints what it found and what it did not, so a
   missing channel is discovered at a standstill rather than at a green light.
3. **Asks what to call this car.** No control unit broadcasts a make or a model: the engine
   describes itself (`1.8l R4 TFSI`), every unit reports a part number, and a marque and a
   generation would have to come from a table this project does not have and will not invent.
   The owner knows, so the owner is asked — one line, defaulted to the engine's own
   description, and it becomes the readable half of the car's directory name:
   `Škoda-Octavia-III-XW8AD4NE9JH008917`.
4. **Asks for what only a person can supply**, in two parts so the arithmetic is the tool's
   and not the owner's: the **mass in running order** (EU registration field G, "mass in
   service" on a UK V5C) and then who and what else will be aboard. This matters more than it
   looks: under Regulation 1230/2012 that figure *already includes* a 75 kg driver and a
   near-full tank, so "kerb mass plus yourself and your fuel" — what an earlier draft of this
   document asked — double-counts about 150 kg on a 1400 kg car. The tool asks whether the
   stated figure includes the driver, adds only what is left, shows the sum, and stores the
   parts rather than the total. Then the tyre size as written on the sidewall.
5. **Explains the road part while the car is still parked**, because none of it can be
   explained at 120 km/h:

   ```
     The road part needs about a kilometre of clear, flat, dry road with no traffic
     behind you — twice, once in each direction. Coasting from 120 to 40 km/h takes
     30 to 45 seconds; the car does not slow quickly in neutral. Find the road
     before you set off.

     Each pass: get to 120, select N, take your foot off, let it roll to 40, then
     drive normally. Nothing here touches the car — it is coasting and this tool is
     reading its speed. Decide about neutral before the pass, not during it: I will
     not ask you anything while you are moving.

     You can stop at any point. Everything answered and every accepted pass is kept.
   ```
6. **Then waits, and says what it is waiting for.** The screen shows the current speed and the
   target; the bus decides when a pass starts and ends (below). If two minutes pass without
   ever reaching the target it offers the way out, since the flags are unreachable at the
   moment they are needed:

   ```
     still waiting for 120 km/h — the fastest so far is 96.
     If this road will not do it, press Esc and start again with a lower range:
         vagcan measure setup --coast-from 90 --coast-to 30
     A narrower range separates drag from rolling resistance less well, and the fit
     will say by how much rather than hiding it.
   ```
7. **Asks for the return pass** — pointing the other way, same stretch — and repeats.
8. **Fits, then writes the car file** and says what the car is now known to be, and what that
   makes available:

   ```
     Setup complete — ~/.vagcan/cars/Škoda-Octavia-III-XW8AD4NE9JH008917/car.json

       mass    1475 kg        you, 2026-08-03
       tyre    205/55R16      you
       CdA     0.63 m²        measured on this car, 2 passes
       Crr     0.0114         measured on this car
       ρ 1.183 kg/m³ and 1385 kg at fit time, wind ≈ 0.8 m/s, slope ≈ 0.3 %

     Power is now available:   vagcan measure --full

     Two things worth doing once, neither of which needs this tool:
       • run out and back on the same stretch and average at matched speeds — slope
         and steady wind reverse sign between the two and cancel
       • compare one run against GPS to see whether this car's bus speed carries the
         speedometer's optimism, then pass --speed-scale
   ```

A resumed setup says where it stands rather than starting over:

```
  resuming setup for XW8AD4NE9JH008917
    mass, tyre      answered 2026-08-03
    coastdown       1 of 2 passes done (2026-08-03, 38.2 s)
  Drive the return pass on the same stretch. If you no longer know which way the
  first pass went, press r to discard it and drive both again.
```

If the run is abandoned before the passes are done, everything already obtained is kept —
the answers **and any accepted pass** — and `--full` stays unavailable until the rest is
done. `measure setup` can be re-run; it says where it stands and asks only for what is missing.

**Where the file lives.** Not in `catalogs/`. That directory is a checked-in corpus of shared
knowledge keyed by part number — a measurement row proven on one `0CW300041G` is true of every
`0CW300041G` in the world. A car file is the opposite on every axis: keyed by a **VIN**, which
is a personal identifier; holding numbers a person typed and measurements of one physical car
on one day with its wheels and its tyre pressures; and worth nothing to anybody else. The
repository's `.gitignore` already keeps VIN-bearing captures out of git, and putting a VIN
under `catalogs/` would be one `git add` from undoing that.

It belongs in the user's own data directory. `datadir::resolve` is not it — that function
walks parent directories looking for something that already exists, which is right for
*reading* the corpus and wrong for *writing* a file, since it would pick whichever checkout
the shell happens to be standing in. A sibling is needed:

```rust
/// Where files this tool writes about *your* car live. Not the corpus.
pub fn vagcan_dir() -> anyhow::Result<PathBuf>;   // ~/.vagcan
pub fn car_dir()    -> anyhow::Result<PathBuf>;   // ~/.vagcan/cars
```

`--car FILE` overrides it explicitly, the way `--catalogs DIR` overrides the corpus, and
`measure setup` prints the path it wrote so the file is never a matter of faith.

```json
{ "vin": "XW8AD4NE9JH008917",
  "name":          { "value": "Škoda Octavia III", "source": "stated" },
  "units": [ { "request": "7E0", "part_number": "8V0906264H" } ],
  "mass_kg":       { "value": 1475,  "source": "stated",   "at": "2026-08-03",
                     "parts": { "running_order": 1395, "includes_driver": true,
                                "aboard": 80 } },
  "tyre":          { "value": "205/55R16", "source": "stated" },
  "rolling_radius_m": { "value": 0.313, "source": "derived-from-tyre" },
  "i_wheels_kgm2": { "value": 5.5,   "source": "wong-typical" },
  "i_engine_kgm2": { "value": 0.34,  "source": "wong-typical", "uncertainty": 0.3 },
  "cda":           { "value": 0.63,  "source": "coastdown", "passes": 2, "at": "2026-08-04",
                     "rho_at_fit": 1.183, "rho_source": "measured",
                     "mass_at_fit_kg": 1385,
                     "wind_estimate_ms": 0.8, "grade_estimate_percent": 0.3 },
  "crr":           { "value": 0.0114,"source": "coastdown", "passes": 2,
                     "includes": "bearings, seals, pad rub, gearbox churning" },
  "speed_scale":   { "value": 1.0,   "source": "uncorrected" },
  "refresh_estimate_s": { "value": 0.048, "source": "measured" } }
```

**Every field carries its provenance**, and the sources are not the same kind of thing:
`stated` came from a person, `coastdown` was measured on this car, `derived-from-tyre` is
arithmetic on a stated value, `uncorrected` means no correction was applied rather than that
one was chosen. There is no `default`: a parameter this tool cannot get honestly is a
parameter it does without (below). The results table and the chart page name the source, so
that two runs are never compared across a change in how the car was described.

Precedence is flag → car file, and every run records the value *and* where it came from.
There is no third tier: a parameter neither source supplies is one the run does without.

### Two modes, and no third — default, or full

There are exactly two states this command runs in:

| mode | requires | produces |
|---|---|---|
| **default** | nothing at all | every time, every mark, acceleration, distance, shift costs, full telemetry |
| **full** (`--full`) | a profile from `measure setup`, carried through both coastdown passes | all of the above, plus power |

There is deliberately **no state in between**. A setup abandoned after the questions does not
"fall back to a generic CdA": `--full` is refused, naming what is missing, and the run is the
default one. Generic road-load numbers are gone from the model entirely — a power figure
resting on a hatchback-shaped guess is exactly the sort of number this document spends nine
sections refusing to print. Either the road load was measured on this car, or there is no
power column.

`--cda` and `--crr` remain as overrides for someone who genuinely has the figures — a
manufacturer's coastdown, a wind-tunnel number — and passing both satisfies the requirement,
recorded as `stated` rather than `coastdown`. Passing one is not enough; the fit produces
them as a pair.

A car that has never been set up is the normal first encounter, and it must not be a wall.
`measure` runs anyway, in the **default mode**, announced in one line at the top of the screen:

```
  no car file for XW8AD4NE9JH008917 — default mode: times, speeds and telemetry,
  no power. Park, then run: vagcan measure setup
```

And once a car file *does* exist, the banner does not simply disappear — a user who spent
twenty minutes on a coastdown and then sees no difference whatsoever concludes it did
nothing:

```
  XW8AD4NE9JH008917 — car file 2026-08-04 (mass 1475 kg, CdA 0.63 measured)
  default mode: times and telemetry. Add --full for the power column.
```

The default mode is not a stripped-down recording. It records **every channel worth having
on its own** — speed, engine speed, gear, selector, pedal, boost specified and actual, air
mass, shaft speeds — and computes every figure that needs no parameter: the marks, their average
accelerations, the instantaneous acceleration, the distance, the shift costs. All of those
come from speed, gear and time, and none of them needs a mass.

What it drops is the **parameter-dependent** layer — power, and only power.

And it drops it at the source: **channels that exist solely to feed a computation are not
polled at all.** Barometric pressure and ambient air temperature are read for one purpose,
air density, which feeds one figure, power. With no mass there is no power, so there is
nothing for them to feed, and reading them would spend bus time to store two numbers nobody
will look at. A cycle spent on nothing is a cycle not spent on speed.

The consequence is worth stating rather than discovering later: **a default-mode recording can
never be turned into a power figure afterwards, even once a profile exists**, because the
density its model needs was never sampled. Everything else about the run — every time, every
mark, every acceleration — is complete and stays comparable with runs recorded later.

Where a profile *does* exist, those two channels are read **once per run** rather than every
cycle. Barometric pressure and outside air temperature do not change measurably in seven
seconds, and polling them at 20 Hz would cost cycles for no information.

### The coastdown, inside setup

Road load is measured rather than guessed. Coasting in neutral, the only forces left are drag
and rolling resistance:

```
m·(1 + δ₁)·(−dv/dt)  =  ½·ρ·CdA·v²  +  m·g·Crr
```

`1 + δ₁` because only the wheels turn with the drivetrain disconnected — the same `δ₁` the
power model already carries, not a third number written down separately. (An earlier draft
said 1.03 while §3 said wheels alone are 1.04; that inconsistency biased **both** coefficients
1.4 % low, since they scale together with it.)

**Fit `v(t)`, not force against `v²`.** The differential equation has a closed form,
`v(t) = v_c·tan( arctan(v₀/v_c) − t/τ )` with `v_c = √(B/A)` and `τ = m(1+δ₁)/√(AB)`, so the
fit runs directly against the raw speed samples with **no differentiation at all**. That
removes a question the force form leaves open — what smoothing window the coastdown uses,
which is not the run's 0.3 s: at that window the force fit reaches only R² ≈ 0.93 and would
reject every valid coastdown, needing 1–2 s instead. It is also better conditioned by an
order of magnitude and gives residuals in km/h, which a person can judge. One Gauss-Newton
loop over three parameters.

**A pass is recognised from the bus, not from a keystroke.** It starts when speed passes
`--coast-from` with **pedal at zero and the selector in N**, and it ends at `--coast-to`. Both
conditions are proven channels, so the tool knows a coast is happening without asking. A pass
is discarded, with the reason on screen, if the pedal moves, the selector leaves N, the speed
rises, or the deceleration jumps in a way braking looks like — a partial coast fitted as a
whole one would put the brakes into `Crr`.

- **Two passes, opposite directions — and what that does and does not buy.** A constant slope
  adds a constant force, which in a model linear in `[v², 1]` is absorbed *entirely into the
  intercept*: a 1 % grade nearly **doubles `Crr`** and moves the residual by less than
  0.01 km/h. No measure of fit quality can see it. Averaging two reciprocal passes cancels it
  **exactly**, and the half-difference is the slope itself:

  ```
  sin θ = (Crr_A − Crr_B) / 2
  ```

  with no `(1+δ₁)` in it — the fit already divides by `m·(1+δ₁)`, so gravity's `m·g·sin θ`
  sits in the same units as `m·g·Crr` and the factor would overstate every slope by about 4 %.

  The implied slope is **reported**, because it comes free and says something about the road
  the driver chose. It is not, however, an acceptance test, and the disagreement between
  passes is a much weaker guard than an earlier draft of this document claimed:

  | what happened | `Crr` disagreement | the average | reported slope |
  |---|---|---|---|
  | two reciprocal passes on a 1 % slope | ~88 % | **correct** — the slope cancels | ~1 %, true |
  | two passes the **same way** on that slope | ~0 % — they agree | **biased** by the slope | ~0 %, false |

  The bad case looks perfect and the good case looks alarming. So the disagreement bar can
  only be loose — it means "both directions measured *a* road load at all", not "the pair was
  reciprocal" — and a tight one would reject exactly the measurements the two-way procedure
  exists to make possible.

  **A non-reciprocal pair is undetectable from the bus.** There is no compass here, and a
  reported slope near zero means either a flat road or a driver who went the same way twice.
  The tool cannot tell those apart; it counts passes, says to turn around, and reports what
  it measured. That is the whole of the protection, and saying so is better than implying a
  check that does not exist.
- **Grade cancels exactly; wind does not.** Drag acts on `(v ± w)²`, whose cross-term is
  antisymmetric and cancels between passes, but whose `w²` term is symmetric and does not. It
  is a constant, so it lands wholly in `Crr` and always positive: `ΔCrr = ½ρ·CdA·w²/(m·g)`,
  which is **+6 % at 5 m/s of wind and +1.5 % at 2.5 m/s**. That gives the help text a real
  criterion — do this in under about 2 m/s or `Crr` reads 2 % high — and the per-pass `CdA`
  spread is itself a wind estimate, `w ≈ (CdA_A − CdA_B)/(CdA_A + CdA_B) · v̄`, free from the
  same two passes.
- **`Crr` as fitted is the whole speed-independent road load**, not a tyre property: wheel
  bearings, seals, brake-pad rub and gearbox oil churning are all roughly speed-independent
  over this range and land in the same intercept. A realistic 15 N of that is **+10 % on a
  `Crr` of 0.0114**. It is the right number for the power model — it is what actually resists
  the car — and the wrong number to compare against a tyre catalogue, or to replace with one
  from a datasheet.
- **What must be recorded with the answer.** The fit returns `½ρ·CdA`, so `CdA` is
  meaningless without the `ρ` that was in the air at the time — and `CdA` scales with the mass
  used in the fit, which is that day's load, not the run's. Both go in the car file, along
  with the estimated wind and grade. An unrecorded ±3 % in `ρ` is a direct ±3 % in `CdA`,
  larger than everything else in the fit. This also means **`measure setup` polls the barometer
  and the ambient sensor regardless of mode**: the frugality rule that skips them belongs to
  runs, not to the measurement that makes runs possible.
- **The range is a default, not a rule.** 120 to 40 km/h separates the two terms; narrowing it
  correlates them badly (`ρ(CdA, Crr) = −0.86` over the full range, −0.97 over 120→80).
  `--coast-from`/`--coast-to` narrow it for a road or a limit that does not allow 120. But the
  fit reports the **two-pass disagreement**, not a condition number: with ~2000 samples the
  statistical error is 0.2 %, some thirty times below the systematic floor, so conditioning is
  not what is actually limiting and inviting the reader to worry about it would be misdirection.
- **The fit is rejected** when the passes disagree beyond the stated margin, or when the
  residuals show the pass was not a clean coast. A rejected fit leaves the car file without
  road load, which means `--full` stays unavailable, which is the correct outcome and not a
  failure of the tool.
- It needs a quiet, flat, dry road with no traffic. **Nothing here is a write**: the car is
  coasting and the tool is reading speed. Whether and when to select neutral is the driver's
  decision, taken before the pass begins, and the tool never prompts for it at speed.

### The odometer cross-check — a hypothesis, not a method

The cluster reports the odometer (proven by an exact hit at 212 760 km), so integrating bus
speed over a long drive and comparing would yield a scale factor — **if** the odometer and the
speed signal are derived differently. On this platform they very likely share a source, in
which case the check is an identity and cannot distinguish "perfectly calibrated" from "no
information". Recorded as an open question, not as something this design rests on.

## 1. The run state machine — `measure/session.rs`

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

- **Standstill** is decided on the **raw integer the unit answered**, not on a converted
  float: `--speed-scale` multiplies before mark detection, and `v == 0.0` on a scaled float is
  a comparison this project would regret. Where a float comparison is unavoidable the bound is
  half the channel's own quantum — `factor / 2` from its catalog row, so even the tolerance is
  data rather than a magic number. Zero must hold for one second: the hold is what stops a
  crawling stop-and-go from arming the trigger in every gap.
- **The start** is the first non-zero sample after that.
- **t0** is not that sample. A launch is *convex* — acceleration rises from zero as the clutch
  engages — so extrapolating backwards runs under the true curve and reaches zero **late**,
  which makes every 0-based mark come out **short**. t0 is a least-squares fit of `v = ½jt²`
  over the first ~0.4 s of movement, extrapolated to `v = 0`, bounded above by the first
  non-zero sample and **not bounded below**.
- **The clamp an earlier draft proposed was worse than the disease.** Clamping t0 into
  `(last zero, first non-zero]` sounds conservative and is the opposite: under a dead band the
  true t0 lies *before* that window, so the clamp bounds the estimate into a region that
  provably excludes the answer. Simulated on a realistic ramp launch it fires on every run and
  adds **+100 to +150 ms**, one-signed, in the flattering direction — turning a 160 ms error
  into a 280–350 ms one. The lower bound is gone.
- **The dominant term is unrecoverable, and the quadratic fit does not rescue it.** A
  wheel-speed signal has a low-speed dead band: at roughly 48 teeth per revolution the pulse
  interval at 1 km/h is ~150 ms, so below about 2–3 km/h the signal's own update collapses and
  it reports zero. Against that, quadratic and linear extrapolation differ by less than their
  own scatter (164 ms versus 156 ms at a 1 km/h cutoff). The quadratic is kept because it is
  the right shape, not because it buys accuracy.
- **The bias is not one-signed after all, and the two estimators bracket it.** An earlier
  draft of this section argued that a convex launch makes every 0-based mark read *short*.
  That is true of the linear extrapolation it was arguing against, and false of the
  constant-jerk fit it then specified: `v = ½jt²` forces `v/v̇ = (t−t₀)/2`, so on a real
  launch — which is already straightening out inside the fit window — it reaches back **too
  far** and the mark reads *long*. Simulated on ramp and exponential torque build-ups with
  the dead band in place:

  | launch | dead band | constant-jerk fit | linear fit | truth |
  |---|---|---|---|---|
  | ramp to 4 m/s² over 0.5 s | 2 km/h | **−0.06 s** | +0.20 s | 0 |
  | exponential, τ = 0.2 s | 2 km/h | **−0.19 s** | +0.13 s | 0 |
  | ramp over 0.3 s | 3 km/h | **−0.14 s** | +0.25 s | 0 |

  The truth sits **between them**, every time, because the two models err in opposite
  directions by construction.

- **So both are computed, and the pair is the answer.** `t₀` is reported as a bracket —
  earliest from the quadratic fit, latest from the linear one — with the mark time printed as
  the interval that follows from it: `0-100  6.12 … 6.38 s`. That is an uncertainty derived
  from this run's own samples rather than a tolerance copied out of a table, and it is honest
  in both directions instead of confidently wrong in one. Rolling marks like `50-100` still
  print to hundredths with a genuine ±, because there both endpoints are interpolated
  crossings and the staleness bias cancels (§3).
- **A ring buffer of the 3 s before t0** is written into the run: pedal, engine speed and
  selector *before* the start are half of what explains a bad one.
- A run ends at the highest mark, at `Esc`, or when speed returns to zero. The last is
  recorded as `aborted`, and the marks that did close are kept — a run that died at 80 still
  measured 0-60.
- After a run the result stays on screen and the trigger re-arms by itself once the car is
  standing still again, so a session is a sequence of runs in one file.

Keys: `p` pauses the trigger, `Esc` cancels the current run, `s` saves the session, `←`/`→`
change which series the chart shows, `q` quits — and argues back if anything is unsaved.
The keyboard is drained between batches, never around one: a read that is waiting out a
two-second response deadline must not make `Esc` wait with it.

## 2. Polling — `measure/channels.rs` and `measure/mod.rs`

**Nothing here names an identifier.** Which identifier carries road speed on this car is a
fact about this car; the tool resolves every channel by the **name its catalog gives it**,
the way `plan::select_basics` already does, and a car whose catalog uses the same words works
without a line of code changing. The identifiers on the reference car appear once, in a
footnote, as evidence — never in the code path.

| role | resolved by name | required? |
|---|---|---|
| leading speed | "vehicle speed" / "road speed" | yes |
| engine speed | "engine speed" | yes |
| gear | "selected gear" | yes |
| pedal | "accelerator pedal position" | yes |
| selector | "selector lever" | no |
| shaft speeds | "input/output shaft speed" | no |
| boost | "boost pressure", actual and specified | no |
| air mass | SAE J1979 PID 0x10 | no |
| barometric pressure, ambient temperature | SAE J1979 PIDs 0x33 and 0x46 | only under `--full` |

**The leading unit is derived, not declared.** It is whichever control unit owns the speed
channel that won resolution — writing "the gearbox" would be this Škoda's accident, not a
rule: on this car the finest road speed happens to sit on the gearbox at 0.01 km/h, while the
cluster publishes 1 km/h and the engine's OBD mirror is one byte.

That makes the **tie-break** load-bearing, because more than one unit answers to those names.
The rule is algorithmic: among the name matches, take the one whose `Scaling::Linear` factor
is finest; break a remaining tie by unit id, so the choice is stable across runs; and record
the winner in `config.speed_source`. Every other unit that matched is polled too, as a
cross-check, and never times anything.

Everything else is the **background** set. Two batches per cycle, each within `plan::BATCH`
(8 identifiers, the measured per-request limit — `plan.rs` carries that number and its
caveat, and this design does not copy it). The leading batch is polled every cycle, the
background batch every second: marks are timed from the leading speed alone, so its rate is
what matters and it gets twice the rate of everything else.

A second speed channel earns its place twice over: as a cross-check on the leading one, and —
by comparing the two channels' values against their own timestamps — as the empirical handle
on how often a unit actually refreshes an identifier, which is the number that sets the
smoothing window (§3).

Engine speed likewise: a channel in its own right, and the input to the engine-side inertia
term (§3). Under `--full` it moves to the **leading** batch: that term needs `ω̇_engine`, and
differentiating a half-rate channel across a gearshift costs about 2.5 % of the power figure
at each end of the shift.

Boost is read as the pair the unit publishes, actual and specified, and rendered in that
order — the order `watch` already uses. The catalog's unit is `bar`; whether that is absolute
or gauge is stated on screen and in the file, and the two are far apart: a stock EA888 runs
roughly 1.0–1.2 bar **gauge** at full load, which is 2.0–2.2 bar **absolute**, while 1.6 bar
absolute is only 0.6 gauge — part load. This is the first number a knowledgeable driver
checks, and an unlabelled column would have them reading a healthy car as a sick one. (Those
figures are this engine's, quoted here to make the point; nothing about them belongs in the
code path.)

**Values are never rendered through `plan::Channel::render`.** That function returns
`"… (raw)"` for anything unproven, which is exactly the class this design excludes;
`measure` goes through `MeasurementDef::interpret` and `describe`, and treats `None` as an
absent channel rather than as bytes to show.

Every value carries its own timestamp, as in `watch --out`. Batches are separated in time,
and one shared timestamp has already corrupted evidence on this project once (the gear
proof moved from η² 0.872 to 0.972 when the columns got their own clocks).

**The rate is a measurement, not a setting, and it propagates into the answers.** Three
consequences, none of which the first draft handled:

- **One time base.** Channels are sampled at different rates and at different instants, so a
  derived value at time `t` — power needs `v(t)`, `a(t)` and engine speed for `k` — is
  computed on the **leading channel's grid**, with every other input linearly interpolated
  onto it. Each derived sample carries the largest staleness among its inputs, and an input
  older than a stated bound suppresses the value rather than approximating it.
- **Uncertainty is computed where it can be.** A rolling mark's ± comes from this run's own
  measured refresh period, `σ ≈ √2·T_refresh/√12`, not from a constant in the source. A
  0-based mark's launch bias cannot be computed at all — the dead band is not observable from
  the bus — so it is printed as a one-signed range, and the document says which of the two a
  given number is.
- **Degradation is visible.** A unit that starts timing out halves the cycle while the
  figures keep printing at the same apparent confidence. Below a floor the run is flagged
  `degraded`, on screen and in the file.

`--minimal` polls only what the stopwatch needs — speed and gear — for the highest achievable
rate, at the cost of the telemetry. It is a deliberate trade and therefore a flag rather than
a hidden heuristic.

The achieved rate is written into the file. It is never asserted in advance.

## 3. What is computed — `measure/session.rs`, `measure/power.rs`

**Marks.** `t(B) − t(A)`, where `t(v)` is linearly interpolated between the two samples that
bracket the crossing. Both crossings must happen in the same run, in a monotonically rising
pass.

The interpolation itself is negligible: the chord's deviation from a locally parabolic `v(t)`
is at most `j·Δt²/8` — the curvature of `v` is **jerk**, not acceleration — so the time error
is `j·Δt²/(8a)`, about **0.8 ms** at 20 Hz with `j = 10 m/s³`. Twenty times below the printed
resolution.

What *is* worth stating is why a rolling mark is the trustworthy one. Each sample is stale by
an unknown amount, but both crossings of a mark are biased late by the **same** mean
staleness, so it **cancels in the difference** — simulated at 24.6 ms and 25.0 ms on the two
endpoints, 0.4 ms on the difference, and independent of how hard the car is accelerating.
What is left is the staleness jitter:

```
σ_mark ≈ √2 · T_refresh / √12          ≈ 20 ms at T_refresh = 50 ms
```

That is the **whole** of the printed uncertainty on a rolling mark, and it is a 1σ figure —
the 1st-to-99th-percentile spread is about ±40 ms. It depends on the unit's refresh period
and **not** on our cycle time: halving the poll interval at fixed refresh moves it by
nothing. Any test that asserts otherwise is asserting a falsehood.

`0-100` has no such formula, because its lower endpoint is not a crossing — it is t0, whose
error is dominated by a dead band that is not observable from the bus at all (§1). So
"uncertainty is computed, not tabulated" is true of rolling marks and only half true of
0-based ones, and the document should say which half.

Speed is converted once, `v[m/s] = v[km/h] / 3.6` exactly (ISO 80000-3), and every formula
below is in SI.

**Average acceleration per mark.** `Δv / Δt` across the mark's own endpoints. This is a
difference quotient too — calling it "measured rather than differentiated" would be a false
distinction. What makes it the most trustworthy acceleration figure here is that its
**numerator is exact by construction**: `Δv = 100 km/h` is the mark's definition and carries
no measurement error at all, so the relative error of the average equals the relative error
of the mark time. That makes it excellent for a **rolling** mark — 0.6 % on `50-100` — and
progressively worse for a short 0-based one, where the launch bias dominates: 2–4 % on
`0-100`, **10–25 % on `0-10`**. Against roughly 8 % for the instantaneous estimate below, the
rolling average is the most trustworthy acceleration figure here and the `0-10` average is
the least. It sits in the results table next to the time.

**Instantaneous acceleration.** A **first-order least-squares slope** over `--accel-window`
(default 0.3 s), fitted against each sample's own timestamp:

```
a = Σ(tᵢ − t̄)(vᵢ − v̄) / Σ(tᵢ − t̄)²        valid at t = t̄
```

Reported in m/s² and in g (`g = 9.80665 m/s²`, 3rd CGPM 1901, an SI definition and not a
property of any car). This is a Savitzky-Golay filter of order 1 generalised to uneven
spacing, and it is chosen over a plain endpoint difference for three reasons that matter
more than its ~20 % variance advantage: an endpoint difference throws away five of seven
samples in the window, so one stale endpoint corrupts the whole estimate; the samples are
**unevenly spaced in time**, which is the entire reason each value carries its own
timestamp, and an endpoint difference's effective baseline then wanders with the jitter; and
the fit has a well-defined attachment point `t̄`, which makes the causal lag exactly
`t_now − t̄` instead of "about half the window".

**Why smoothing is needed — and it is not quantisation.** Speed quantised at 0.01 km/h gives
`σ_q = q/√12 = 8·10⁻⁴ m/s`, so a raw sample-to-sample difference at 50 ms carries
`√2·σ_q/Δt = 0.023 m/s²` against a signal of 4–5 m/s². That is nothing. The real noise is
**value staleness**: the control unit refreshes each identifier on its own schedule,
asynchronous to our polling, so a reading is stale by an unknown `0…T_refresh`. With
staleness uniform on that interval the induced error in a slope over window `W` is

```
σ_a ≈ a · √2 · (T_refresh/√12) / W
```

— which is a **lower bound**, because it assumes the staleness is independent per sample. It
is not: a zero-order hold with a slowly beating phase makes consecutive samples' staleness
strongly correlated, which is precisely the "identical value, then a double-sized step" the
mechanism produces. Simulated against the real hold, `σ_a` at `a = 4 m/s²`,
`T_refresh = 50 ms`, `W = 0.3 s` is **0.38 m/s² (0.039 g)**, about 1.4× the formula and
skewed — the estimator sits low on a plateau and overshoots on a step. Sixteen times the
quantisation floor rather than seventy, which changes nothing about the conclusion and
everything about quoting the right number.

**Measuring `T_refresh` is harder than the first draft assumed.** Comparing the leading speed
against a second unit's speed does not do it: the background batch runs at half the rate, so
it cannot resolve a 50 ms period at all, and on this platform the second channel is very
likely the *same* signal gatewayed onward — the identity trap this document already flags for
the odometer, walked into without noticing. It has to come from the **leading channel itself**:
the intervals between consecutive *distinct* values. When the poll interval and the refresh
period are close, as they are here, that yields a **bound rather than a measurement**, and it
is recorded as such — one significant figure and a flag, not three digits of false
precision.

**Causal live, central afterwards.**

| where | method | why |
|---|---|---|
| live, on the TUI | **causal** — trailing window, reported at `t̄` | the future half of a centred window does not exist yet |
| the results table, the JSON, the chart page | **central** — symmetric window over the finished run | the causal estimate is delayed by exactly `t_now − t̄ ≈ W/2 = 150 ms` |

The central scheme fixes the **lag and nothing else**. Both schemes have the same magnitude
response — attenuation is a property of the window, not of where it sits — so switching to
central recovers no peak height whatever.

What the window costs is **not** `sinc(πfW)`. That is the response of a boxcar *smoother*;
this is a first-order least-squares *differentiator*, whose response relative to ideal
differentiation is

```
|H(f)| = (3/x²)·| sin x / x − cos x | ,        x = πfW
```

At `W = 0.3 s` that is **9 % lost on a ~1 Hz acceleration peak** and **~25 % on a 0.3 s shift
dip** — sinc would have said 14 % and 36 %, and would also have claimed the dip is nulled out
entirely at 3.3 Hz, where the true response is still 0.30. A synthetic 0.3 s dip of 3.0 m/s²
comes back at 2.18 m/s² when sampled at 21 Hz, which is the number to assert in a test.

The dip case is still the uncomfortable one — the window is the size of the feature — but
that is not why shifts are located from the gear channel. They are located there because the
gear is *on the bus*, so no threshold and no baseline are needed at all.

At the run's edges the central window has no symmetric neighbourhood. The first and last
`W/2` use a one-sided fit over whatever samples exist, flagged in the series — a DQ200's
peak acceleration is often inside the first 0.5 s, so simply skipping that region would
truncate the peak search exactly where the peak lives.

This forces a storage rule: **the file holds raw speed samples, and every derivative is a
separate, labelled layer recomputed in one pass over the complete run.** Numbers shown live
never reach the file. Without that rule the same run reports two different peaks depending
on whether it was read off the screen or out of the JSON — and, more usefully, every method
below can be corrected later without re-driving the car.

**Peak acceleration.** Not the maximum of the series: the max of a noisy estimator selects
positive noise excursions, which biases it **upward by about 7 %**, in the flattering
direction. Reported instead as the mean over a ±τ neighbourhood of the argmax, with the time
and the gear, and the statistic named in the file.

That correction has a residual of its own, and it is now the dominant one: averaging a locally
parabolic peak of curvature `c` over ±τ under-reads it by exactly `c·τ²/6` — **−1.3 % on a
broad peak and −4 % on a sharp first-gear one** at τ = 0.2 s. Shrinking τ to 0.1 s quarters
it. Either way the residual is stated rather than left for someone to rediscover, because the
whole reason for the correction was that a biased peak is not acceptable.

**The peak search excludes the edges it cannot measure.** The first and last `W/2` have no
symmetric window, and a one-sided fit over a shrinking span gets noisy fast — five times the
interior noise by the time only two samples remain. Combined with `argmax`, that would make
the very first eligible sample the reported peak far more often than it should be, which is
exactly where a DQ200's real peak lives and therefore exactly where the error would hide. A
sample enters the search only if its fit span is at least 0.6·W, and each peak carries its own
σ, which the fit already computes as part of `Σ(tᵢ − t̄)²`.

**Shifts.** A gear change is **observed**, not inferred: `3816` is polled and says which gear
is engaged. Deriving an event from an attenuated derivative when the event itself is on the
bus is the weaker method, and a threshold relative to the run's peak cannot work — in a tall
gear the acceleration is *permanently* a small fraction of the run's peak, so no fixed
fraction distinguishes fifth gear from a shift.

The gear channel locates the shift; the acceleration trace only measures its cost, and the
cost is not the dip's duration. A full-load upshift does not decelerate the car — it
accelerates it less — so `speed_lost` would read `0.0` essentially always. The cost is the
**velocity deficit**, the integrated acceleration shortfall against the pre-shift local
acceleration:

```
speed_deficit = ∫_shift ( a_post − a(t) ) dt          [m/s]
cost_on_mark  = speed_deficit / a(v_mark)             [s]
```

Three details decide whether that number means anything:

- **The baseline is the acceleration in the gear being shifted *into*, not out of.** The
  counterfactual for "what did this shift cost" is an instantaneous change into the new gear,
  which is already slower than the old one. Using the pre-shift value charges the shift for
  the ratio change as well, and overstates the cost by about 35 %.
- **The divisor is the acceleration at the mark's upper endpoint, not at the shift.** A
  velocity deficit taken at 60 km/h persists all the way to 100, so the time it costs is
  `Δv / a(100 km/h)`. Dividing by the acceleration where the shift happened understates the
  cost by nearly a factor of two.
- **The limits come from the gear channel**, plus a fixed pad — never from "wherever `a` fell
  below the baseline". A threshold-found window makes the metric positive by construction and
  reports a cost where no shift occurred, which is the same selection-on-noise mistake the
  peak statistic exists to avoid.

**Distance.** Trapezoidal integration over each interval's own `Δtᵢ`, never a nominal
`1/hz`. The numerical error is negligible and should not be the caveat: composite-trapezoid
error is `(h²/12)·Δa ≈ 1 mm` over a 10 s run. The real distance error is the speed signal
itself, and it has **three** multiplicative parts, all of them thousands of times larger than
the integrator: the speedometer question (§3a), driven-wheel slip at the launch, and the
rolling circumference the car's own unit is calibrated with — which shifts with a non-stock
tyre size, with wear (a worn tyre is ~1.5 % smaller) and with pressure. The last is the same
knob `--speed-scale` addresses. The caveat names all three, so nobody reaches for Simpson's
rule.

**Power.** Two figures, not one, because they are two different quantities and a single
number with a paragraph of apology is worse than two labelled ones:

```
P_wheel = ( m·(1+δ₁)·a  +  ½·ρ·CdA·(v + v_head)²  +  m·g·Crr  +  m·g·sin θ ) · v
P_shaft = P_wheel  +  I_e · ω_engine · ω̇_engine
```

`P_wheel` is what crosses the contact patch and is the figure comparable to a rolling road.
`P_shaft` adds the power going into spinning the engine and clutch up, which is real work the
engine is doing and is not delivered to the road. Calling the sum "power at the contact
patch", as an earlier draft did, was simply wrong: the engine-side term is upstream of the
clutch. Note the asymmetry in the drag term — drag acts on air speed, power is delivered
against ground speed. `v_head` is 0 and `θ` is 0 unless given.

**The engine-side term is written exactly, never through a gear ratio.** An earlier draft
computed an equivalent-inertia factor `k = 1 + δ₁ + δ₂·ξ²` from a measured engine-to-wheel
ratio `ξ = ω_engine·r/v`. That is correct while the clutch is locked and **catastrophic when
it is not**: at launch the engine sits at ~2200 rpm while `v` is near zero, so `ξ → ∞` and the
factor explodes. Simulated on this car's own numbers it reports **330 kW at 1 km/h** — on a
132 kW car, from the first sample of every single run, straight into `peak_power_kw`. The
exact form above is algebraically identical under lock (verified term by term), is finite at
launch because `ω̇_engine ≈ 0` there, and correctly goes **negative** during an upshift, when
the engine gives its stored energy back. It also makes the rolling radius cancel out entirely.

**And it is still suppressed while the clutch slips**, because even the exact form is wrong
there: the energy the engine releases goes into the clutch as heat, not to the road. Slip is
detected against the ratio the car itself reports — `|ξ_measured − ξ_gear| / ξ_gear > 5 %`,
where `ξ_gear` is learned per gear from the plateaus the car shows during steady driving, so
no ratio table is written down anywhere. A floor of 15 km/h backs it up. A suppressed sample
produces no power value at all, exactly as a stale input does.

`δ₁` and `δ₂` are **not universal constants**, and treating them as such would be precisely
the failure this project's rules are about. They are `I_wheels/(m·r²)` and `I_engine/(m·r²)` —
so the textbook 0.04 and 0.0025 are a *typical car's inertias divided by someone else's mass
and wheels*, and they change by nearly a factor of two across the range of cars this tool is
meant to serve. What is generic is the **inertia**, not the ratio. So the car file stores
`I_wheels` and `I_engine` with a source of their own, and the coefficients are computed from
this car's mass and radius:

```
δ₁ = I_wheels / (m·r²)      δ₂ = I_engine / (m·r²)
```

Wong, *Theory of Ground Vehicles*, is the source of the typical inertias, and it is worth
noting that the implied `I_engine ≈ 0.34 kg·m²` is the right order for a four-cylinder crank
with a dual-mass flywheel and a clutch pack — which is the most that can be said for it. They
carry roughly ±30 %, which is **±2 % of power in top gear and ±12 % in first**, and that row
belongs in §3a with everything else that can move the number.

For sizing only, and never in the code path: with this gearbox's published ratios the
equivalent factor runs from about **1.66 in first to 1.05 in seventh** (it is a seven-speed),
so ignoring rotational inertia altogether would understate the inertial term by 40 % in first
and 9 % in fourth.

Mass, `CdA` and `Crr` come from the car file (§0), not from flags typed before every run, and
**none of the three has a default**. Mass belongs to one specific car; `CdA` and `Crr` are
measured by the coastdown on that same car. Without all three there is no power column and
the run is the default one — there is no generic-number fallback, because a power figure
resting on the drag of hatchbacks-in-general is not a measurement of this car at all.

(For sizing an error budget, generic values are still worth naming: `CdA ≈ 0.65 m²` for a
C-segment hatchback, `Crr ≈ 0.012` for passenger radials on asphalt — Gillespie,
*Fundamentals of Vehicle Dynamics*, gives 0.010–0.015. They appear in §3a for that purpose
and nowhere in the code path.)

Air density is `ρ = p / (R·T)`, `R = 287.05287 J/(kg·K)` for dry air (ISO 2533), `p` from
OBD-II PID 0x33 (absolute barometric pressure, 1 kPa/bit) and `T` from PID 0x46 (ambient air
temperature, `A − 40 °C`; `T_K = T_C + 273.15`). Dry air costs up to −1.6 % on `ρ` at 30 °C
and saturation, which is under 0.1 kW — mentioned once and not defended further.

What does matter is **when** `T` is read. That sensor is heavily damped and heat-soaks while
the car stands still: +5 to +15 K after idling is ordinary, and +10 K reads `ρ` 3.4 % low,
which is an order of magnitude more than the humidity and quantisation terms this document
used to dwell on. So it is read **at the end of the run, at speed**, not at the standstill
where the driver was waiting. If either PID is absent, power is not computed — `--air-density`
may be given explicitly, and the file records whether `ρ` was measured or stated.

Every power figure is labelled an estimate. Stored in kW (SI); any horsepower display states
which horsepower (metric PS = 735.49875 W, DIN 66036) since the two differ by 1.4 %.

Air mass (`F410`) is recorded but takes no part in the model. A MAF-based cross-check exists
in folklore at roughly 1 PS per g/s, but that ratio is an empirical BSFC artefact rather than
a published standard, so it stays out of every reported figure.

**Wheel slip.** The gearbox's road speed is derived from the **driven** axle, so during a
launch it exceeds ground speed by the longitudinal tyre slip — 2–5 % at 0.5 g. That inflates
the early marks and the integrated distance, in the flattering direction, and it is separate
from and additive to the speedometer question. It is caveated, not corrected. If a
non-driven wheel speed is ever proven on this car (the brake unit `0x713` answers 48
identifiers, none of them proven), the difference between driven and undriven becomes a
direct measurement of wheelspin — a column no phone app can produce. Recorded here as the
reason to want it.

**Kickdown.** If the unit's catalog holds a row whose name contains `kickdown`, that is used.
Otherwise it is derived from the pedal — but **not** against a fixed 99 %, which would be this
car's number in disguise: the pedal here is a byte scaled by 0.4, so full travel reads 102 %,
and what full travel reads is a property of one unit's scaling. The threshold is the run's own
observed maximum, less one raw step, and the result is labelled derived. No identifier for
kickdown has been proven on this car, and a column that silently guesses is worse than an
empty one.

**Gearbox mode.** The selector lever from the catalog: P/R/N/D are proven. **D versus S
versus manual is not** — it is open work in `todo/README.md` (the stimulus was never given
during the recording that identified the lever). A code outside the proven table shows as
`unknown code 07` and enters nothing.

**Gears are read by their labels, never by the order of their codes.** This car's enum is
`[[0,"not engaged"],[2,"1"],…,[8,"7"],[12,"R"]]` — the codes are neither contiguous nor
ordered by ratio, and two of the levels are not gears at all. A transition into or out of a
non-numeric level is not a shift and must not be measured as one. This is the exact bug the
project already made once and documented in `catalog.rs`, where `gear + 1` reported reverse
as "gear 11".

## 3a. Which way the numbers lean

The errors in this tool are not symmetric noise around a true value. **Almost every one of
them is one-signed, and almost every one makes the car look quicker or more powerful than it
is.** A driver comparing these numbers against a phone app or a magazine figure will see a
systematic gap, and the honest thing is to say so on the page rather than let them conclude
the tool is broken.

Sized against a reference case (1400 kg, CdA 0.65 m², Crr 0.012, ρ 1.20 kg/m³, 100 km/h,
a ≈ 2.5 m/s², total ≈ 120 kW):

| error | size | direction |
|---|---|---|
| ignoring rotational inertia entirely | 9 % of power in 4th, **40 % in 1st** | understates power |
| mass wrong by ±50 kg | ±3.8 kW ≈ ±5.2 PS | either way |
| speed scale wrong by ±2 % | ±4.8 kW ≈ ±6.5 PS | either way, and it moves the times too |
| inertias generic to ±30 % | ±2 % of power in top, **±12 % in 1st** | either way |
| `Crr` uncertain by ±0.002 | ±0.76 kW | either way |
| t0 before the speed signal wakes up | 0.15–0.45 s of bracket on any 0-based mark | **either way** — the two fits straddle the truth, so the mark is printed as an interval |
| road speed is the car's own signal, which regulation forbids to under-read | ~0.2–0.3 s on a 7 s 0-100 | **flatters** |
| driven-wheel slip during the launch, 2–5 % in 1st and 2nd | tenths on early marks, 1–3 m on distance | **flatters** |
| unknown grade, ±1 % | ±3.8 kW ≈ 5 PS | downhill **flatters** |
| unknown headwind, 5 m/s | 3.3 kW ≈ 4.5 PS | tailwind **flatters** |
| wind during the coastdown, 5 m/s | +6 % on `Crr`, and it does **not** cancel between passes | overstates road load, so understates power |
| peak taken as the max of a noisy series | ~7 % on peak power and peak acceleration | **flatters**, which is why it is not taken that way |
| CdA uncertainty ±0.05 m², which is what a *generic* value is worth | ±0.67 kW ≈ 0.9 PS | either way — and the coastdown is what removes it |
| air density from the car's own PIDs, quantisation | ±0.045 kW ≈ 0.06 PS | either way |

The last rows are there to keep the effort proportionate. Reading `ρ` from the barometer and
the ambient sensor is correct, but its quantisation is **80 times smaller than an unnoticed
1 % grade** — and smaller still than the heat-soak in the same sensor, which is why *when* it
is read matters more than how finely it is quantised (§3). The framing "density comes from the
car itself rather than a constant" oversells it, and the doc should not.

Two mitigations are procedural, not code, and belong in the command's own help:

- **Run out and back on the same stretch and average at matched speeds.** Grade and steady
  wind both reverse sign between the two runs and cancel to first order. Nothing else
  available here removes them.
- **Compare against GPS once** to settle whether this car's *bus* speed carries the
  speedometer's optimism at all, then set `--speed-scale`.

`--grade PERCENT` and `--headwind M_S` exist for a user who knows the figures; both default
to zero and both are recorded in the file.

## 4. The live view — `measure/mod.rs`

`ratatui::widgets::Chart` with `Dataset` and `Marker::Braille`. The dependency is already in
`crates/vagcan/Cargo.toml`; `textplots` and `rasciigraph` would render worse and add a
crate.

```
  RUN 4.31 s                       full       marks
  ┌──────────────────────────────────────────┐  0-10   1.0+ s
  │ speed    62.4 km/h                       │  0-25   2.1+ s
  │ engine   4310 rpm                        │  0-50   4.0+ s
  │ gear     3                               │  0-60    ·
  │ pedal    100 %                           │  0-80    ·
  │ boost    2.06 / 2.15 bar abs (act/spec)  │  0-100   ·
  │ accel    0.41 g       trailing           │
  │ power    108 kW       estimate           │
  └──────────────────────────────────────────┘
  ┌── speed ── ← → to change ────────────────┐
  │                                ╱─────    │
  │                      ╱────────╱          │
  │            ╱────────╱                    │
  └──────────────────────────────────────────┘
    0s        2s        4s        6s
```

**A run needs no keystroke.** Arming, starting and finishing all happen by themselves. The
keys are for exceptions — cancelling, pausing the trigger, changing the series, saving — and
none of them has to be pressed for a run to be *measured*. Nothing prompts the driver while
the car is moving.

**Saving is explicit**, and that is a deliberate choice against the alternative: `s` writes
the session, and `--out` writes continuously for anyone who wants that. What must not happen
is losing a drive by accident, so quitting with unsaved runs does not quit:

```
  4 runs not saved.   [s] save    [q] again to discard
```

Two keystrokes to throw away a drive, one to keep it, and no file appears that nobody asked
for.

**The screen always says which state it is in**, in a band across the top, because the state
machine in §1 is otherwise invisible and the first thing a new user meets is `Idle` — with
nothing on screen telling them the tool is waiting for the car to stop:

| state | band |
|---|---|
| Idle, moving | `WAITING — come to a full stop to arm      0.4 km/h` |
| Idle, stopped, counting | `ARMING — hold still  0.6 s` |
| Armed | `ARMED — go when you are ready` |
| Running | `RUN  4.31 s` |
| Finished | `DONE  6.12 s — stop completely to arm the next run` |
| Aborted | `ABORTED at 82 km/h — kept 0-10, 0-25, 0-50, 0-60` |
| Trigger paused | `PAUSED — will not arm.  [p] resume` |
| Rate collapsed | the band gains `  SLOW — 6 Hz, times less certain` |

The right-hand corner carries the achieved rate and whether a file is open. `WAITING` shows
the current speed next to it for one specific reason: arming needs a true zero, and a car
creeping at 0.4 km/h would otherwise sit there looking broken with nothing to explain it.

**A tone marks each closed mark**, because the screen is unreadable at the moment the
information arrives. One short tone per mark, a different one when the run finishes, a low
one when a run is aborted or a coastdown pass is rejected — that last is the one that matters
most, since without it a rejected pass is discovered a kilometre later. macOS carries usable
sounds in `/System/Library/Sounds`; the terminal bell is the fallback, and `--quiet` turns it
off. The player is spawned and never waited on: a poll loop that blocks on audio would put
the sound ahead of the measurement.

**The results table waits for the car to stop.** At the finish the marks panel simply stops
filling in and the band says `DONE`; the two-block table redraws the screen only once the car
is stationary. Redrawing a dense table at 100 km/h is exactly what the rest of this design
avoids.

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

**The results table — `measure/report.rs`.** Shown when a run finishes or is cancelled, in two
blocks under their own headings rather than one list:

```
  Run 2 — measured
    mark (km/h)   time                average acceleration
    0-10          0.94 … 1.13 s       2.8 m/s²
    0-100         6.03 … 6.38 s       4.5 m/s²
    50-100        3.24 s ± 0.02       4.28 m/s²

    Marks from a standstill are a range, not a number: the car is already rolling
    before its own speed signal wakes up, and where inside that gap it started
    cannot be recovered. The two ways of extrapolating back to zero err in
    opposite directions, so the answer is between them. 50-100 starts from a real
    crossing and has no such gap.
    peak engine speed   6480 /min at 5.9 s
  Run 2 — computed   (mass 1400 kg, tyre 205/55R16, CdA 0.65 m², Crr 0.012,
                      ρ 1.19 kg/m³ measured, grade 0 %, window 0.30 s, central)
    distance             92 m      ±3 % — the car's own speed signal, not the maths
    peak acceleration   5.3 m/s²   (0.54 g) at 1.21 s in gear 2  (mean over ±0.2 s)
    peak power, wheel   108 kW     (147 PS) estimate
    peak power, shaft   112 kW     (152 PS) estimate, engine-side inertia included
    shift 2→3           cost 1.6 km/h, 0.18 s on the 0-100
```

A 0-based mark prints as a **range**, one-signed, because its error is one-signed; a rolling
mark prints a single number with a real ±. On the live screen, where there is room for one
number and no time to read, the 0-based marks carry a trailing `+` and the range waits for the
results table. The two are not the same kind of number and neither display pretends they
are.

The measured block holds times, the average accelerations that are `Δv/Δt` across a mark's
own endpoints, and peaks of channels the car reported. The computed block carries its
conditions in the heading: the same run under a different mass is a different set of numbers,
and a table that hides that invites the comparison it cannot support.

## 4a. What the user reads when something goes wrong

Every refusal in §8's test table needs words, and the design had none — an assertion that a
command "refuses, naming what is missing" is not a message. The rule these follow: say what
happened, say what it cost, and end with something to do.

Two of them are already written. `measure` resolves its adapter through the same helper the rest
of the CLI uses, so "no adapter" and the documented failure where this hardware enumerates on
USB but macOS attaches no serial node — the one that needs a physical unplug — inherit copy
that already exists rather than growing a worse second version.

**A required channel is missing** (checked at a standstill, in `measure` as well as in `setup`):

```
  race needs road speed, and this car's catalogs do not have it.

    gearbox  7E1  0CW300041G   speed          not in the catalog
    engine   7E0  8V0906264H   engine speed   ok
                               pedal          ok

  There is no stopwatch without a speed channel, and race will not guess one from
  raw bytes. To find it:
      vagcan survey --out parked.jsonl      then, after a drive:
      vagcan survey --out driving.jsonl
      vagcan survey --diff parked.jsonl driving.jsonl
  The identifiers whose bytes moved are the live measurements.
```

**`--full` without a finished car file:**

```
  --full computes power, and power needs this car measured, not assumed.

    mass 1475 kg        answered 2026-08-03
    tyre 205/55R16      answered 2026-08-03
    CdA, Crr            missing — the coastdown was never completed

  Park the car and run:  vagcan measure setup
  It keeps what you already answered and asks only for the coastdown.

  Running without --full: every time, every mark, acceleration, distance and shift
  cost. Only the power column is absent.
```

**A coastdown pass is rejected** — and it must say which way to point next, because the tool
cannot see direction and a mis-paired set poisons the fit silently:

```
  pass 2 rejected — the brake was used (deceleration jumped at 71 km/h).
  Stay pointing the way you are now and do it again: you still owe one pass in
  this direction.   Passes so far: 1 of 2.
```

**The fit is rejected** — the worst moment in the whole feature, twenty minutes of driving for
nothing, so this one ends with a plan rather than a verdict:

```
  The two passes disagree by 11 % on Crr (limit 5 %), so neither is trusted and no
  road load was written.

    pass 1   CdA 0.61 m²   Crr 0.0109      implied slope between them: 0.9 %
    pass 2   CdA 0.68 m²   Crr 0.0121      implied wind: 4.1 m/s

  Both passes fit their own data well, so this is not noise — something differed
  between the two directions. A slope of about 1 % would do it, and so would that
  much wind.

  What to try, in order:
    • a flatter stretch — both passes must be the same piece of road
    • a calmer day; above roughly 2 m/s the wind alone shifts Crr by 2 %
    • warm the car first: cold tyres and cold gearbox oil read a higher rolling
      resistance than the car will have on a run

  Nothing else is lost. Mass and tyre size are saved. Re-run vagcan measure setup and
  it asks only for the passes.
```

**The rate collapses mid-run:**

```
  SLOW — 6 Hz (was 21). A control unit has started timing out.

  The times are still real, but their uncertainty has roughly tripled and the run is
  flagged `degraded` in the file. Marks from a standstill are worst affected.
  Try --minimal, or check the adapter at the OBD port.
```

**The car stops answering** — ignition off, a pulled connector, a unit that went quiet — in
both `measure` and `setup`, which the first draft never covered at all:

```
  the car stopped answering. Current run discarded; 3 saved runs are untouched.
  Waiting — this will pick up again when the ignition is back on.
```

**The car file is for a different car:**

```
  that car file is for XW8AD4NE9JH008917 and this car says XW8AD4NE9JH000123.
  Ignoring it: mass and road load belong to one specific car. Run
  vagcan measure setup for this one, or pass --car with the right file.
```

**Two keys that mean different things in different commands.** `Esc` cancels a run here and
quits in `watch`. That is a deliberate divergence rather than an oversight — a stopwatch needs
a cheap "throw this one away" and `watch` has nothing to throw away — and it is written down
so it stays deliberate. `q` quits both, and here it argues back when there is unsaved work.

## 5. The saved session — raw JSON

```json
{ "schema": 1, "tool": "vagcan measure", "recorded_at": "2026-08-03T12:41:07+03:00",
  "car":      { "vin": "…", "units": [ { "request": "7E1", "part_number": "0CW300041G" } ] },
  "config":   { "marks": [[0,100]],
                "mass_kg": 1475, "tyre": "205/55R16", "rolling_radius_m": 0.313,
                "inertia_model": "exact-engine-side",
                "i_wheels_kgm2": 5.5, "i_engine_kgm2": 0.34,
                "i_source": "wong-typical", "i_uncertainty": 0.3,
                "cda": 0.63, "cda_source": "coastdown",
                "crr": 0.0114, "crr_source": "coastdown",
                "car_file_source": "user",
                "grade_percent": 0.0, "headwind_ms": 0.0,
                "air_density_kg_m3": 1.19, "air_density_source": "measured",
                "degraded": false, "cycle_median_s": 0.047, "hz": 21.4,
                "speed_source": "7E1:F40D", "speed_scale": 1.0,
                "speed_scale_applied": "before-marks",
                "t0_method": "quadratic-fit", "t0_clamp_s": 0.048,
                "accel_window_s": 0.3, "accel_method": "central-least-squares",
                "peak_statistic": "mean-over-0.2s-neighbourhood",
                "refresh_estimate_s": 0.05 },
  "channels": [ { "key": "speed", "name": "Vehicle speed", "unit": "km/h",
                  "origin": "read", "request": "7E1", "did": "F40D" },
                { "key": "accel", "name": "Acceleration", "unit": "m/s2",
                  "origin": "derived", "from": ["speed"],
                  "method": "central-least-squares", "window_s": 0.3 },
                { "key": "power_wheel", "name": "Power at the wheels", "unit": "kW",
                  "origin": "derived", "estimate": true,
                  "from": ["speed", "barometric_pressure", "ambient_temperature"],
                  "method": "road-load" },
                { "key": "power_shaft", "name": "Power including engine-side inertia",
                  "unit": "kW", "origin": "derived", "estimate": true,
                  "from": ["power_wheel", "engine_speed"],
                  "method": "road-load+engine-inertia",
                  "suppressed_when": "clutch slipping" } ],
  "runs":     [ { "index": 1, "t0_wall": "…", "aborted": false,
                  "series": { "speed":  { "t": [-2.94, -2.89], "v": [0.0, 0.0] },
                              "gear":   { "t": [-2.94],        "v": ["not engaged"] } },
                  "marks":   [ { "from": 0, "to": 100, "seconds": 6.12, "from_t0": true,
                                 "bracket_s": { "earliest": 6.03, "latest": 6.38,
                                                "from": "quadratic-and-linear-t0" },
                                 "avg_accel_ms2": 4.54 } ],
                  "derived": { "stamp": "t0=quadratic-fit accel=central-least-squares/0.3 \
peak=mean-0.2s",
                               "distance_m": 118.4, "peak_rpm": 6480,
                               "peak_power_wheel_kw": 108.1, "peak_power_shaft_kw": 112.3,
                               "peak_accel_ms2": 5.31, "peak_accel_t": 1.21,
                               "peak_accel_gear": "2",
                               "shifts": [ { "t": 2.44, "from": "2", "to": "3",
                                             "speed_deficit_ms": 0.45,
                                             "cost_on_mark_s": 0.18 } ] } } ] }
```

**`schema` is checked before anything else.** A session cannot be regenerated — it is
evidence from a drive — so `measure view` refuses a schema it does not know, naming it, rather
than half-reading it. Every field added later carries `#[serde(default)]`, or old files stop
loading and the storage rule defeats itself.

**Samples are columnar, not a list of objects.** `{"speed": {"t": [...], "v": [...]}}` rather
than a `speed` key repeated on every sample: twenty runs of ten seconds at 20 Hz over ten
channels is about a megabyte of JSON written the verbose way, and `measure view` inlines that
into the page the browser has to load. Columnar is the same information at roughly a quarter
of the size, and it is the shape the page's JavaScript wants anyway. Each channel keeps its
own `t` array, which is what "every value carries its own timestamp" means in this layout.

**Five details the writer must get right**, each of which the page had to guess at:

- **Channel keys are `snake_case` in the file** — `engine_speed`, `boost_actual`,
  `input_shaft_speed` — even though `channels.rs` matches on the catalog's prose names. One
  spelling in the document, and the reader does not normalise.
- **A rolling mark carries `sigma_s`**, its 1σ from `√2·T_refresh/√12`. A 0-based mark
  carries `bracket_s: { earliest, latest }` instead — absolute times, not offsets, because
  they come from two different extrapolations rather than from a tolerance around one number.
- **Distance is a derived channel**, written out like any other, not only a scalar in
  `derived`. The chart offers a distance axis, and integrating speed a second time in the
  page would be a second implementation of §3.
- **`derived` is written already recomputed.** The page draws stored series and reads
  `derived` for its table; it has no arithmetic layer and cannot honour the stamp rule by
  itself. Whoever writes the file recomputes first.
- **Marks that never closed are still listed**, with no time. A run that died at 80 says so
  by having `0-100` present and empty, not by omitting it.

**`derived` is a cache, and `config` is what makes it safe.** The storage rule says the file
holds raw samples and derivatives are recomputed; `derived` exists so a person or a script
can read the answers without reimplementing §3. It carries a `stamp` naming the methods that
produced it, and `report` and `measure view` **recompute and ignore `derived` whenever the stamp
does not match the maths they are running**. Without that rule the same file would hold two
answers and no way to choose.

Every channel declares its `origin`. A derived one also declares what it was derived
**from** and by what **method**, with the parameters that method used — an acceleration
figure whose window is not recorded is not reproducible, and a power figure whose mass is
not recorded is not checkable. There is no `proven` flag: in `measure` it would be `true` on
every row, since nothing else gets in.

A value that a proven channel returned but whose meaning is not in its table — an
unmapped selector code — is stored as `{ "raw": "07", "unmapped": true }` and enters no
derived figure.

`config` records not just the inputs but **which method produced each derived layer** —
`t0_method`, `accel_method`, `peak_statistic`, `air_density_source`, `inertia_model`, the
measured `refresh_estimate_s`. Every correction in this document arrived after the design was
first written, and the reason they are all retrofittable is that the file keeps raw speed.
Recording the vintage of the maths alongside it is what makes an old file comparable to a new
one rather than merely readable.

`--out` writes continuously; `s` writes on demand. Both write the same document.

## 6. The chart page — `measure/view.rs`

`race view FILE.json` generates a self-contained HTML file next to the input and opens it.
Inline SVG and inline JavaScript, no CDN, no external asset of any kind — a strict rule, and
a test asserts the output contains no `http://` or `https://`.

**The page is a real file, not a Rust string.** What §6 asks for — a cursor with a tooltip
over every shown series, four X axes, six series sets, five background discriminators, a run
selector, a legend split by origin — is a small charting application. Written as `format!`
in Rust it becomes a thousand lines of unlintable string soup that no editor understands, and
the "no external URL" test would pass on a page that renders nothing at all.

```rust
// race/view.rs — the whole of it
const PAGE: &str = include_str!("view.html");
let html = PAGE.replace("/*{{SESSION}}*/", &serde_json::to_string(&session)?);
```

`view.html` carries the CSS, the JavaScript and the SVG scaffold, with the session's JSON
substituted at one marked point. It is editable, lintable and viewable on its own with a
sample session pasted in. The tests then assert three things instead of one: the placeholder
is gone, the embedded JSON parses back to the session it came from, and neither the template
nor the output contains an external URL. Nothing verifies that the chart *draws* — say so,
rather than letting a green test imply it.

Opening the file is a convenience, not the job: the path is printed first, and a platform
without a usable `open` is not a failure of the command.

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
- **Runs overlay, up to four.** The speed axis exists so that two attempts compare directly,
  and a page that can only show one run cannot do the thing its own axis was chosen for. The
  first selected run draws in the full palette and the rest in muted variants of the same
  hues, so the solid/dashed distinction between read and derived still reads. The cursor
  tooltip gains a difference row — `run 3 is 0.14 s ahead of run 1 at 80 km/h` — and the
  background discriminator applies to the reference run only, since four sets of gear bands
  would be mud. Comparing runs across two session files is out of scope, and the page says so
  rather than leaving it to be discovered.

Everything drawn comes from the recomputed derivative layer, so the page and the results
table cannot disagree.

## 7. Files

```
crates/vagcan/src/measure/mod.rs        the command and the poll loop
crates/vagcan/src/measure/channels.rs   resolve every channel by name; the pre-flight check
crates/vagcan/src/measure/session.rs    the state machine and run assembly — no I/O
crates/vagcan/src/measure/derive.rs     t0, the accel series, peaks, shifts, distance — pure
crates/vagcan/src/measure/power.rs      the dynamics model and air density
crates/vagcan/src/measure/carfile.rs    the per-VIN car file: read, write, precedence
crates/vagcan/src/measure/setup.rs      the interview and the driving script — I/O only
crates/vagcan/src/measure/coastdown.rs  pass detection and the road-load fit — pure
crates/vagcan/src/measure/ui.rs         the live screen: drawing and keys
crates/vagcan/src/measure/report.rs     the results table
crates/vagcan/src/measure/view.rs       thirty lines: template plus substitution
crates/vagcan/src/measure/view.html     the page itself
```

Eleven files rather than seven, and the reason is that the first split put two of them on a
path to being unreadable. `watch/mod.rs` is 1615 lines — the largest file in the crate —
doing command, loop and TUI; `race`'s screen is more complex than `watch`'s, so `ui.rs` is
split off at the seam `watch` never split. And `session.rs` as first drafted held eleven
algorithms and something like twenty-five tests; the boundary between "state over time" and
"arithmetic over a finished run" is the one §3's storage rule already draws, so `derive.rs`
draws it in the file system too.

The split also separates by testability: `channels.rs`, `session.rs`, `derive.rs`,
`power.rs`, `coastdown.rs`, `carfile.rs` and `report.rs` know nothing about adapters or
terminals, which is what makes almost every test below runnable without a car.

## 7a. What this reuses, and the two extractions it needs

`measure` is a third live loop in a crate that already has two. Almost everything it needs is
written; what is missing is that the useful parts are private or inline.

**Reusable unchanged:** `plan::Channel`, `plan::Batch`, `plan::plan`, `plan::BATCH`,
`plan::available` (which already merges the standard OBD parameters with per-part catalog
rows, and already resolves the collision where the same identifier means different things on
two units), `plan::select_basics`'s name-matching approach, `plan::identities_from_survey`,
and — for the coastdown — `analyse::fit_linear`, which is least squares **with R²** and whose
two `None` paths are exactly the "not enough speed range" rejection.

**Two extractions, no behaviour change:**

```rust
// out of watch/mod.rs:poll_batch, which is private and writes into watch::App
pub async fn read_batch<B: vag_can::CanBackend>(
    backend: &mut Option<B>, batch: &Batch, started: Instant,
) -> BatchOutcome;                 // must report "no answer" explicitly — see below

// out of the identification block inlined in watch::run
pub async fn identify<B: vag_can::CanBackend>(
    backend: B, also: &[u16], progress: &mut crate::progress::Line,
) -> (B, Vec<plan::UnitIdentity>);
```

The second matters more than it looks: the gateway walk, the 300 ms probe, and the rule that
the powertrain is never in the gateway's list and so is added rather than discovered, all
live inline in `watch::run` today. `measure` needs them and `measure setup` needs them. Copied,
they drift.

**Do not write a second least squares, and do not write a second R² number.** The repository
has one stated bar in `analyse::Thresholds::default()` and a test in `main.rs` whose whole
purpose is to catch a copy of it drifting from the original. A coastdown needing a different
bar adds a *named field* to `Thresholds`, not a literal.

**Four hazards specific to this loop:**

- `read_batch` must return an explicit no-answer. `poll_batch` currently drops the error and
  leaves the previous value in place, which is invisible — and `degraded` (§2) cannot be
  detected without it. Measure the floor in **cycle time**, not in missed answers: a unit
  replying "response pending" can legally stall for many seconds without missing anything.
- **Do not wrap the batch read in `select!`.** The backend is `take()`n out of an `Option`
  and put back after the await; if that future is dropped mid-flight the `Option` stays
  `None` and the adapter is silently gone for the rest of the run. Drain the keyboard
  *between* batches, as `watch` does, so `Esc` never waits out a two-second response timeout.
- The loop takes its reader through a seam so the scheduling is testable without a car:
  `trait BatchReader { async fn read(&mut self, batch: &Batch) -> BatchOutcome; }` — live
  implementation wrapping `read_batch`, test implementation replaying a synthetic drive.
- `analyse::split_records` — which is what makes a multi-identifier response usable — lives
  in the offline-analysis module. It is the inverse of `read_data_by_identifiers` and belongs
  beside it in `vag-protocol`; moving it keeps a live loop from importing the offline module.

## 8. Tests

| test | asserts |
|---|---|
| arming | a synthetic profile arms only after zero is held a full second |
| t0 on a **constant-jerk** launch **with a dead band** | the bias stays inside a stated bound and stays one-signed. Two traps: a *constant-acceleration* trace makes linear extrapolation exact and the test vacuous, and a trace with no dead band hides the dominant term entirely |
| t0 lower clamp | there isn't one: a trace with a dead band must not have t0 pushed later than the fit puts it, and an implementation that clamps is caught here |
| marks | interpolated `0-100` on an analytic profile equals the closed-form answer |
| mark precision | a 0-based mark prints a tenth with a bound; a rolling mark prints hundredths |
| staleness cancellation | with a simulated uniform staleness, the rolling mark's error is an order below the 0-based one's |
| abort | speed returning to zero keeps the marks that closed and flags the run |
| re-arm | a second standstill starts a second run in the same session |
| trigger pause | `p` prevents arming and does not lose the previous run |
| `--marks` parser | `0-100,50-100` parses, `100-50` and `abc` are rejected |
| `--speed-scale` | a scale of 0.97 moves the detected crossing, not just the printed number |
| least squares on uneven samples | the slope of a known ramp is recovered from deliberately jittered timestamps |
| causal vs central | the two differ in **phase only** on a known trace — equal peak magnitude — and the file holds the central one |
| window attenuation | a 0.3 s synthetic dip of 3.0 m/s² is recovered at ~2.2 m/s² at 21 Hz — the least-squares differentiator's response `(3/x²)|sin x/x − cos x|`, **not** `sinc(πfW)`, which would have predicted a third less |
| shifts | a shift is located from the gear channel, and its cost is the integrated deficit, positive on a profile where speed never falls |
| peak statistic | on a series with injected noise the reported peak is not the maximum, and its upward bias is bounded |
| air density | ρ = **1.225 kg/m³** at 101.325 kPa and 288.15 K — the ISO 2533 sea-level value, four significant figures. Not a comparison against the same formula: this anchor catches a wrong R, a K/°C slip and a kPa/Pa slip in one assertion |
| launch power | a synthetic launch with engine speed held at 2200 rpm while the car crawls produces **no power sample at all** until lock-up — the ratio-based form would have reported 330 kW here |
| slip gate | a ratio 5 % from the gear's learned plateau suppresses power, and the plateaus are learned from the trace rather than tabulated |
| engine-side inertia | the exact form matches the ratio-based one term for term while locked, and goes negative during an upshift |
| inertia provenance | `δ₁`/`δ₂` are computed from this car's mass and radius, and the file records the inertias and their source rather than the coefficients alone |
| peak edges | an injected noise spike inside the first 0.1 s does not become the reported peak |
| peak residual | the ±τ statistic under-reads a known parabolic peak by `c·τ²/6`, and the figure is asserted |
| shift cost | the baseline is the post-shift gear, the divisor is the acceleration at the mark's upper endpoint, and the window comes from the gear channel — a trace with no shift yields no cost |
| coastdown grade | a 1 % slope leaves R² at 0.999 and moves `Crr` by ~88 %: the fit is rejected by the two-pass disagreement, never by R², and the implied slope is reported |
| coastdown wind | a 5 m/s wind survives two-pass averaging as +6 % on `Crr`, matching `½ρCdA·w²/(mg)`, and the per-pass `CdA` spread recovers the wind speed |
| coastdown inputs | `CdA` is unusable without the `ρ` and the mass that were in force at fit time, and both are in the car file |
| two modes only | `--full` on a profile with a mass but no coastdown is refused, not silently downgraded to a generic-CdA power run; `--cda` alone is refused, `--cda` with `--crr` is accepted as `stated` |
| the default is not a stub | a default run still produces every mark, acceleration, distance and shift cost, and every read channel is in the file |
| frugality | barometric pressure and ambient temperature are **not polled** in the default mode, and are polled **once**, not per cycle, under `--full` — reachable through the `BatchReader` seam |

Almost every row above runs on synthetic sample vectors with no transport at all, which is
the point of the module split. Two do not, and they are the two that matter for safety:
"never sends `0x10`" and the polling-frugality rule both need something that answers like a
control unit. Neither existing double will do — `ReplayCan` is synchronous and order-exact, so
a loop that skips the background batch on odd cycles desynchronises it immediately, and
`MockAsyncTransport` sits at the PDU layer while the loop constructs its own ISO-TP channel
over a `CanBackend`. **There is no `CanBackend` double in the workspace.**

Two steps, in order. First the `BatchReader` seam (§7a), which makes scheduling and frugality
testable with no CAN at all and costs nothing. Then a `FakeCar` in `vag-can`, gated the way
`vag-transport` already gates its mock, recording every service byte and every
`(request id, identifier)` it was asked for — about 120 lines, most of it ISO-TP framing. It
pays for itself beyond `race`: `watch`'s loop, `faults` and `survey` are equally untestable
today.
| profile round trip | `measure setup` writes a profile the next run reads, and the run then needs no flags |
| provenance | every parameter in the file names its source, and none of them may be a guess |
| precedence | a flag beats the profile, and the file names the winner |
| coastdown fit | synthetic coastdown data with known `CdA`/`Crr` recovers both; a one-way pass is refused; two passes that disagree are rejected |
| uncertainty from data | doubling the simulated **refresh period** doubles a rolling mark's bound; doubling the **cycle time** at fixed refresh moves it by nothing. The first draft's test asserted the second and would have failed |
| one time base | a derived value whose engine-speed input is a cycle stale is interpolated onto the leading grid, and one that is beyond the bound is suppressed |
| degradation | a run whose rate falls below the floor is flagged, and the flag reaches both the screen and the file |
| page | the placeholder is substituted, the embedded JSON parses back to the session, and neither template nor output contains an external URL. **Nothing asserts the chart draws** |
| schema | `measure view` refuses an unknown `schema` by name instead of half-reading the file |
| columnar round trip | a session survives write-then-read with every channel's own timestamps intact |
| stale cache | a `derived` block whose `stamp` does not match the current maths is ignored and recomputed |
| channels by name | resolution succeeds against a catalog whose identifiers differ from the reference car's, and the leading unit follows the finest speed channel rather than a fixed unit |
| speed tie-break | with three units matching "speed", the finest `factor` wins and the choice is recorded in `speed_source` |
| gear labels | a gear enum with non-contiguous codes and non-numeric levels yields shifts only between numeric levels |
| kickdown threshold | with a pedal that reads 102 % at full travel, kickdown is still detected, and no literal percentage appears in the source |
| standstill | arming is decided on the raw integer, and a `--speed-scale` of 0.97 does not prevent a stopped car from arming |
| session hygiene | the run issues no `0x10` session change — **needs a `CanBackend` double, which the workspace does not have** (below) |
| pre-flight | a catalog store missing the speed channel refuses the run and names it |
| optional channels | no barometer means no power column, and the run still happens |
| origin | every channel in the file declares `origin`, and every derived one its method and parameters |
| unmapped code | a selector value outside the table is stored flagged and reaches no derived figure |
| series sets | a set whose channels this car lacks is not offered on the page |
| run overlay | four runs render together, the difference row is computed at matched speeds, and the background follows the reference run only |
| quit guard | quitting with unsaved runs does not quit, and a second `q` discards |
| state band | every state in §1 has a band, and `WAITING` shows the speed that is keeping it there |
| sound | a closed mark plays a tone without blocking the poll loop, and `--quiet` silences it |
| error copy | each refusal names what happened, what it cost and what to do next; a missing channel names the channel and the unit |
| wrong car | a car file whose VIN differs from the car's is ignored with a message, never applied |

## Open questions deliberately left open

- **D / S / Tiptronic** cannot be reported until the drive mode is identified on the car
  (`todo/README.md`, open work item 4). The column exists and reads from the catalog; today
  it shows the selector lever only.
- **Kickdown** has no proven identifier. The derived-from-pedal value is a stopgap, marked
  as such, and should be replaced the first time a survey run with a deliberate kickdown
  isolates the real one.
- **Speedometer error — unverified, and the earlier draft asserted it.** UNECE R39 forbids a
  speedometer to under-read and permits over-reading by 10 % + 4 km/h, so the *indicated*
  speed is optimistic by regulation. Whether the **bus** value carries that optimism is a
  different question: on many VAG platforms the gearbox/ABS value is close to true wheel
  speed and the bias is added in the instrument cluster. Stating it as fact in a design
  document was out of character for this project. One GPS comparison run settles it.
  `--speed-scale` exists for whoever does that; the default is 1.0, and the file records
  both the value and that it was applied before mark detection.
- **The rolling radius** is a nominal size from `--tyre`, not a measured rolling
  circumference, and the two differ by a few per cent with pressure, load and wear. The exact
  inertia form no longer uses it (§3); what it still affects is turning the generic inertias
  into this car's coefficients, where a 3 % error in `r` is 6 % on `I/(m·r²)`. Which
  convention the derivation uses must be stated: the geometric radius of a 205/55R16 is
  0.316 m, its dynamic radius about 0.98 of that.
- **A non-driven wheel speed** would turn the launch-slip caveat into a measurement. The
  brake unit answers 48 identifiers and none of them is proven; this is the concrete reason
  to go and prove one.
