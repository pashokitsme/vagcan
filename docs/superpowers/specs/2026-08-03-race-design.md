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
opposite problem and the opposite advantage: its speed is the car's own — finely quantised,
though its accuracy and update rate are set by the sensor rather than by the resolution — and
it can put the gear, the pedal and the boost on the same time axis as the stopwatch.

## Scope

| In | Out |
|---|---|
| `vagcan race` — armed stopwatch over a live poll loop | GPS, or any external reference |
| user-defined marks (`0-100`, `50-100`, …) | a "dyno" claim — the power figure is an estimate and says so |
| live TUI: a value table and one chart at a time | drag-strip conventions (rollout, 1/4-mile traps) — later, if wanted |
| a session of several runs, saved as one raw JSON | writing anything to a control unit |
| `--view FILE.json` — a self-contained HTML chart page | a server, a bundler, or any external asset |
| proven channels only | discovering measurements — that is what `survey` and `watch --survey` are for |
| `--setup` and `--coastdown`: the car described once, and its road load measured | asking for a number before every run that nobody knows off the top of their head |

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
vagcan race [--device PATH] [--profile FILE] [--minimal]
            [--marks 0-10,0-25,0-50,0-60,0-80,0-100]
            [--accel-window SECONDS] [--hz N] [--out FILE] [--catalogs DIR]

vagcan race --setup [--device PATH]        interview this car once, write its profile
vagcan race --coastdown [--device PATH]    measure CdA and Crr on the road
vagcan race --view FILE.json               open a saved session as a chart page

overrides, all of which normally live in the profile:
            [--mass KG] [--tyre 205/55R16] [--cda M2 --crr N] [--inertia N]
            [--grade PERCENT] [--headwind M_S] [--air-density KG_M3]
            [--speed-scale N]
```

**The ordinary invocation is `vagcan race` with no flags at all.** Everything the model needs
either comes from the car, or was answered once by `--setup` and measured once by
`--coastdown`, and lives in the car's profile (§0). The override flags exist for a one-off —
a loaded boot, a different set of wheels — and what was used is recorded in every file. None
of them has a generic default: a parameter that cannot be had honestly is one the run does
without, which is what `--minimal` describes.

`--marks` takes `A-B` pairs in km/h, comma-separated, `A < B`. The default is
`0-10,0-25,0-50,0-60,0-80,0-100`.

`--speed-scale` is applied **before** mark detection, not to the printed result: otherwise
`0-100` would mean an indicated 100 rather than a corrected one, and the correction would
silently not apply to the thing it was set for. Which was used is recorded in the file.

`--minimal` forces the mode a car without a profile is in anyway (§0): telemetry and times,
no power, and no polling of the channels that exist only to feed the power model. With a
complete profile it is a way to buy sample rate back when the run matters more than the
wattage.

`--tyre` is the only flag that describes the car's hardware, and it is there because the
rolling radius closes the loop on the equivalent-inertia factor (§3).

`--view` reads a saved session and opens a chart page; it touches no adapter. The precedent
is `survey --diff`, which is likewise an offline mode of a command that otherwise needs the
car — the command stays where the user looks for it.

## 0. Knowing the car — `race --setup` and the profile

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

So the car needs a profile, and the profile needs to be written once.

**`vagcan race --setup`** does it, standing still:

1. Identifies the car — VIN from the engine, part numbers and component strings from every
   unit the gateway lists. The VIN is the profile's key, exactly as a part number is a
   measurement catalog's key: the car names itself and the file is found by that name.
2. Runs the pre-flight channel check and prints what it found and what it did not, so a
   missing channel is discovered at a standstill rather than at a green light.
3. Asks for what only a person can supply, with the source named rather than the units:
   *kerb mass from the registration document, plus the people and fuel that will be in the
   car* — and the tyre size as written on the sidewall.
4. Writes `catalogs/cars/<VIN>.json` and says plainly that the profile is **not finished**:
   the coastdown supplies the other half and needs a road, so it is a separate command run
   when there is one. Until it has run, `race` is in minimal mode (below).

```json
{ "vin": "XW8AD4NE9JH008917",
  "units": [ { "request": "7E0", "part_number": "8V0906264H" } ],
  "mass_kg":       { "value": 1400,  "source": "stated",   "at": "2026-08-03" },
  "tyre":          { "value": "205/55R16", "source": "stated" },
  "rolling_radius_m": { "value": 0.313, "source": "derived-from-tyre" },
  "cda":           { "value": 0.63,  "source": "coastdown", "r2": 0.998, "passes": 2,
                     "at": "2026-08-04" },
  "crr":           { "value": 0.0114,"source": "coastdown", "r2": 0.998, "passes": 2 },
  "speed_scale":   { "value": 1.0,   "source": "uncorrected" },
  "refresh_estimate_s": { "value": 0.048, "source": "measured" } }
```

**Every field carries its provenance**, and the sources are not the same kind of thing:
`stated` came from a person, `coastdown` was measured on this car, `derived-from-tyre` is
arithmetic on a stated value, `uncorrected` means no correction was applied rather than that
one was chosen. There is no `default`: a parameter this tool cannot get honestly is a
parameter it does without (below). The results table and the chart page name the source, so
that two runs are never compared across a change in how the car was described.

Precedence is flag → profile → default, and the file records the value *and* where it came
from.

### Two modes, and no third — minimal, or complete

There are exactly two states this command runs in:

| mode | requires | produces |
|---|---|---|
| **minimal** | nothing at all | every time, every mark, acceleration, distance, shift costs, full telemetry |
| **complete** | `--setup` **and** `--coastdown` | all of the above, plus power |

There is deliberately **no state in between**. A profile with a mass but no coastdown does
not "fall back to a generic CdA" — it runs minimal. Generic road-load numbers are gone from
the model entirely: a power figure resting on a hatchback-shaped guess is exactly the sort of
number this document spends nine sections refusing to print. Either the road load was
measured on this car or there is no power column.

`--cda` and `--crr` remain as overrides for someone who genuinely has the figures — a
manufacturer's coastdown, a wind-tunnel number — and passing both satisfies the requirement,
recorded as `stated` rather than `coastdown`. Passing one is not enough; the fit produces
them as a pair.

A car that has never been set up is the normal first encounter, and it must not be a wall.
`race` runs anyway, in **minimal mode**, announced in one line at the top of the screen:

```
  no profile for XW8AD4NE9JH008917 — minimal mode: times, speeds and telemetry,
  no power. Park, then: vagcan race --setup, and vagcan race --coastdown.
```

Minimal mode is not a stripped-down recording. It records **every channel worth having on its
own** — speed, engine speed, gear, selector, pedal, boost specified and actual, air mass,
shaft speeds — and computes every figure that needs no parameter: the marks, their average
accelerations, the instantaneous acceleration, the distance, the shift costs. All of those
come from speed, gear and time, and none of them needs a mass.

What it drops is the **parameter-dependent** layer — power, and only power.

And it drops it at the source: **channels that exist solely to feed a computation are not
polled at all.** Barometric pressure and ambient air temperature are read for one purpose,
air density, which feeds one figure, power. With no mass there is no power, so there is
nothing for them to feed, and reading them would spend bus time to store two numbers nobody
will look at. A cycle spent on nothing is a cycle not spent on speed.

The consequence is worth stating rather than discovering later: **a minimal recording can
never be turned into a power figure afterwards, even once a profile exists**, because the
density its model needs was never sampled. Everything else about the run — every time, every
mark, every acceleration — is complete and stays comparable with runs recorded later.

Where a profile *does* exist, those two channels are read **once per run** rather than every
cycle. Barometric pressure and outside air temperature do not change measurably in seven
seconds, and polling them at 20 Hz would cost cycles for no information.

### The coastdown — `race --coastdown`

Road load can be measured instead of guessed. Coasting in neutral, the only forces left are
drag and rolling resistance, so the deceleration decomposes:

```
−m·k·a(v)  =  ½·ρ·CdA·v²  +  m·g·Crr
```

Everything on the left is known — speed from the bus, mass from the profile, `k ≈ 1.03` since
only the wheels still turn — and a least-squares fit against `v²` returns **both** unknowns,
for this car, with its wheels and its tyre pressures. This is the standard method (SAE J1263 /
J2263) reduced to what a bus and a laptop can do.

- The run is **two-way**: down the stretch and back, averaged. Grade and steady wind reverse
  sign between the two passes and cancel to first order, and a coastdown is *more* sensitive
  to grade than an acceleration run is, because the force it measures is ten times smaller.
  A one-way coastdown is not accepted; the tool asks for the return pass.
- A useful range is roughly 120 down to 40 km/h, which takes a quiet, flat, dry road with no
  traffic. The tool records; when and whether to select neutral is the driver's decision and
  the tool does not prompt for it while the car is moving.
- The fit is rejected below an R² threshold, and rejected outright if the two passes disagree
  by more than a stated margin — which is what a slope or a gusty day looks like in the data.
- Nothing here is a write. The car is coasting; the tool is reading speed.

### The odometer cross-check — a hypothesis, not a method

The cluster reports the odometer (`0x2203`, proven by an exact hit at 212 760 km). Integrating
bus speed over a long drive and comparing against the odometer's increment would yield a scale
factor — **if** the odometer and the speed signal are derived differently. They may share a
source, in which case the check is an identity and measures nothing. It is written down here
as a hypothesis one drive settles, not as a calibration this design relies on.

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
- **t0** is not that sample, and it is not a straight line back from it either. A launch is
  *convex* — acceleration rises from zero as the clutch engages — so a backward **linear**
  extrapolation runs under the true curve and reaches zero **late**, which makes every
  0-based mark come out **short**. On a constant-jerk launch reaching 1 km/h at 0.29 s the
  linear estimate lands 157 ms late: three times larger than the 50 ms polling cycle it was
  meant to recover, and always in the flattering direction. So t0 is a **least-squares fit of
  `v = ½jt²` over the first ~0.4 s of movement**, extrapolated to `v = 0` and clamped to
  `(last zero, first non-zero]`. The clamp bounds the extrapolation; it does not bound the
  error.
- **The residual is larger than the method.** A wheel-speed signal has a low-speed dead band:
  at roughly 48 teeth per revolution the pulse interval at 1 km/h is ~150 ms, so below about
  2–3 km/h the signal's own update rate collapses and it reports zero. The car can therefore
  have been moving for 100–250 ms before the first non-zero sample, and no extrapolation from
  inside that region recovers it. **Every mark starting at 0 carries a systematic uncertainty
  of order 50–200 ms**, one-signed, and is printed with it: `0-100 6.12 s ±0.1`. Rolling
  marks like `50-100` are printed to hundredths, because there both endpoints are
  interpolated crossings and the staleness bias cancels (§3).
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
rate of everything else. The engine's own road speed is read for two purposes and used for
neither of the obvious ones: as a cross-check against the leading channel, and — by comparing
the two channels' values against their own timestamps — as an empirical handle on how often
the units actually refresh an identifier, which is the number that sets the smoothing window
(§3). It never times anything.

Engine speed earns its place twice as well: as a channel in its own right, and as the
numerator of the engine-to-wheel ratio that the equivalent-inertia factor needs (§3).

Boost is read as the pair the unit publishes, specified `2029` and actual `202A`. The
catalog's unit is `bar`; whether that is absolute or gauge is stated on screen and in the
file, because a stock EA888 runs about 1.0–1.2 bar **gauge** and 1.6 bar **absolute** is the
same pressure written two ways — this is the first number a knowledgeable driver checks.

Which identifiers these are comes from the catalogs by name, exactly as
`plan::select_basics` already does — a car whose catalog uses the same words works, and a
car with no catalog gets an error naming what it could not find rather than a wrong number.

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
- **Uncertainty is computed, not tabulated.** The `±0.1` and `±0.02` printed with the marks
  are worked out from *this run's* measured sample interval and `T_refresh`, not from
  constants in the source. Select more channels and the cycle lengthens and the bound grows;
  the numbers must say so.
- **Degradation is visible.** A unit that starts timing out halves the cycle while the
  figures keep printing at the same apparent confidence. Below a floor the run is flagged
  `degraded`, on screen and in the file.

`--minimal` polls only what the stopwatch needs — speed and gear — for the highest achievable
rate, at the cost of the telemetry. It is a deliberate trade and therefore a flag rather than
a hidden heuristic.

The achieved rate is written into the file. It is never asserted in advance.

## 3. What is computed — `race/session.rs`, `race/power.rs`

**Marks.** `t(B) − t(A)`, where `t(v)` is linearly interpolated between the two samples that
bracket the crossing. Both crossings must happen in the same run, in a monotonically rising
pass.

The interpolation itself is not a source of error worth discussing: the chord deviates from a
locally parabolic `v(t)` by at most `(a/8)·Δt²`, which converts to a time error of
`Δt²/8 = 0.3 ms` at 20 Hz, independent of acceleration and twenty times below the printed
resolution.

What *is* worth stating is why a rolling mark is the trustworthy one. Each sample is stale by
an unknown amount, but both crossings of a mark are biased late by the **same** mean
staleness, so it **cancels exactly in the difference**. The residual is only the staleness
jitter, `σ ≈ T_refresh/√12 ≈ 14 ms` per endpoint, about 20 ms on the difference. That is why
`50-100` is quotable to hundredths while `0-100` is not: its lower endpoint is not a
crossing at all, it is t0, and t0 has no partner to cancel against.

Speed is converted once, `v[m/s] = v[km/h] / 3.6` exactly (ISO 80000-3), and every formula
below is in SI.

**Average acceleration per mark.** `Δv / Δt` across the mark's own endpoints. This is a
difference quotient too — calling it "measured rather than differentiated" would be a false
distinction. What makes it the most trustworthy acceleration figure here is that its
**numerator is exact by construction**: `Δv = 100 km/h` is the mark's definition and carries
no measurement error at all, so the relative error of the average equals the relative error
of the mark time — about **0.3 % on `0-100`** and about **2 % on `0-10`**, against tens of
percent for any instantaneous estimate. It sits in the results table next to the time.

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

— at `a = 4 m/s²`, `T_refresh = 50 ms`, `W = 0.3 s` that is **0.27 m/s² (0.028 g)**, seventy
times the quantisation floor. It shows up as consecutive polls returning the identical value
followed by a double-sized step, which is what makes a raw difference *look* like noise.
`T_refresh` is measurable on this car for free — the engine's own `F40D` is already polled
as a cross-check, and comparing the two channels' value-against-timestamp bounds it — so the
window is chosen from a measurement rather than guessed, and the measured value is recorded.

**Causal live, central afterwards.**

| where | method | why |
|---|---|---|
| live, on the TUI | **causal** — trailing window, reported at `t̄` | the future half of a centred window does not exist yet |
| the results table, the JSON, the chart page | **central** — symmetric window over the finished run | the causal estimate is delayed by exactly `t_now − t̄ ≈ W/2 = 150 ms` |

The central scheme fixes the **lag and nothing else**. Both schemes have the same magnitude
response — attenuation is a property of the window, not of where it sits — so switching to
central recovers no peak height whatever. What the window costs is `sinc(πfW)`: **14 % on a
~1 Hz acceleration peak, 36 % on a ~0.3 s shift dip** at `W = 0.3 s`. The dip case is the
uncomfortable one, because the window is the same size as the feature, and it is why shifts
are located from the gear channel rather than from the derivative (below).

At the run's edges the central window has no symmetric neighbourhood. The first and last
`W/2` use a one-sided fit over whatever samples exist, flagged in the series — a DQ200's
peak acceleration is often inside the first 0.5 s, so simply skipping that region would
truncate the peak search exactly where the peak lives.

This forces a storage rule: **the file holds raw speed samples, and every derivative is a
separate, labelled layer recomputed in one pass over the complete run.** Numbers shown live
never reach the file. Without that rule the same run reports two different peaks depending
on whether it was read off the screen or out of the JSON — and, more usefully, every method
below can be corrected later without re-driving the car.

**Peak acceleration.** Not the maximum of the series: taking the max of a noisy estimator
selects positive noise excursions, which biases the peak **upward** by roughly σ — a 3–5 %
effect on both peak acceleration and peak power, in the flattering direction. Reported
instead as the mean over a ±0.2 s neighbourhood of the argmax, with the time and the gear.
The statistic used is named in the file.

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
speed_deficit = ∫_dip ( a_pre − a(t) ) dt          [m/s]
```

Always positive, directly interpretable ("this shift cost 0.8 km/h"), and dividable by the
acceleration at that speed to say what it cost in seconds on the mark.

**Distance.** Trapezoidal integration over each interval's own `Δtᵢ`, never a nominal
`1/hz`. The numerical error is negligible and should not be the caveat: composite-trapezoid
error is `(h²/12)·Δa ≈ 1 mm` over a 10 s run. The real distance error is the speed signal
itself — a few per cent of bias (below) and driven-wheel slip — which is **three orders of
magnitude larger**. The caveat names that, so nobody optimises the integrator.

**Power.** Road-load power at the contact patch:

```
P = ( m·k·a  +  ½·ρ·CdA·(v + v_head)²  +  m·g·Crr  +  m·g·sin θ ) · v
```

Note the asymmetry: drag acts on air speed, power is delivered against ground speed. `v_head`
is 0 and `θ` is 0 unless given — see "Which way the numbers lean" for what that costs and how
to cancel it.

`k` is the **equivalent-inertia factor**, and it multiplies the inertial term only — never
drag, never rolling resistance. `k = 1` would say the car has no rotating mass, which is
never true: wheels alone are `k ≈ 1.04`, and the drivetrain's contribution scales with the
square of the overall ratio, so `k` is strongly gear-dependent. Wong, *Theory of Ground
Vehicles*, gives the generic form

```
k = 1 + δ₁ + δ₂·ξ²        δ₁ ≈ 0.04, δ₂ ≈ 0.0025, ξ = overall engine-to-wheel ratio
```

which for a DQ200-like set runs from `k ≈ 1.5` in first to `k ≈ 1.05` in sixth. Taking
`k = 1.0` understates the inertial term — some 90 % of the total — by 5 % in fourth and
**35 % in first**. That is the largest single error available in this model, so:

- `ξ` is **measured live**, not tabulated: `ξ = ω_engine·r / v`, from engine speed `206E` and
  road speed, once the rolling radius `r` is known.
- `r` comes from the tyre size in the profile — the owner's statement about their own car,
  not a constant in the source. With it, `k` is computed per sample from the ratio the car
  itself is reporting, which is exactly the shape this project wants: algorithm in code, car
  in data.
- There is no fallback, because there is no half-configured mode: a run without a tyre size
  has no mass and no road load either, so it is minimal and computes no power at all.
  `--inertia` overrides `k` with a flat factor for anyone who wants to force one.

Air density is `ρ = p / (R·T)`, `R = 287.05287 J/(kg·K)` for dry air (ISO 2533), `p` from
OBD-II PID 0x33 (absolute barometric pressure, 1 kPa/bit) and `T` from PID 0x46 (ambient air
temperature, `A − 40 °C`; `T_K = T_C + 273.15`). The dry-air assumption costs −0.4 % at 20 °C
and 50 % relative humidity, −1.6 % at a worst-case 30 °C and saturation, and J1979 has no
humidity parameter to do better with. If either PID is absent, **power is not computed** —
`--air-density` may be given explicitly, and the file records whether ρ was measured or
stated.

Mass, `CdA` and `Crr` come from the car's profile (§0), not from flags typed before every
run, and **none of the three has a default**. Mass belongs to one specific car; `CdA` and
`Crr` are measured by the coastdown on that same car. Without all three there is no power
column and the run is minimal — there is no generic-number fallback, because a power figure
resting on the drag of hatchbacks-in-general is not a measurement of this car at all.

(For sizing an error budget, generic values are still worth naming: `CdA ≈ 0.65 m²` for a
C-segment hatchback, `Crr ≈ 0.012` for passenger radials on asphalt — Gillespie,
*Fundamentals of Vehicle Dynamics*, gives 0.010–0.015. They appear in §3a for that purpose
and nowhere in the code path.)

Every power figure is labelled an estimate at the contact patch. It is **not** a chassis-dyno
"wheel horsepower": because `k` folds in the power spent accelerating the drivetrain's
rotating masses, this figure legitimately exceeds a steady-state roller number, and a driver
will otherwise compare the two. Stored in kW (SI); any horsepower display states which
horsepower (metric PS = 735.49875 W, DIN 66036) since the two differ by 1.4 %.

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

**Kickdown.** If the unit's catalog holds a row whose name contains `kickdown`, that is
used. Otherwise it is derived from the pedal (≥ 99 %) and labelled derived. No identifier
for it has been proven on this car, and a column that silently guesses is worse than an
empty one.

**Gearbox mode.** The selector lever from the catalog: P/R/N/D are proven. **D versus S
versus manual is not** — it is open work in `todo/README.md` (the stimulus was never given
during the recording that identified the lever). A code outside the proven table shows as
`unknown code 07` and enters nothing.

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
| `k = 1` instead of the gear-dependent factor | 5 % of power in 4th, **35 % in 1st** | understates power |
| t0 from a launch that is convex | 50–200 ms on any 0-based mark | **flatters** — the run looks shorter |
| road speed is the car's own signal, which regulation forbids to under-read | ~0.2–0.3 s on a 7 s 0-100 | **flatters** |
| driven-wheel slip during the launch, 2–5 % in 1st and 2nd | tenths on early marks, 1–3 m on distance | **flatters** |
| unknown grade, ±1 % | ±3.8 kW ≈ 5 PS | downhill **flatters** |
| unknown headwind, 5 m/s | 3.3 kW ≈ 4.5 PS | tailwind **flatters** |
| peak taken as the max of a noisy series | 3–5 % on peak power and peak acceleration | **flatters** |
| CdA uncertainty ±0.05 m², which is what a *generic* value is worth | ±0.67 kW ≈ 0.9 PS | either way — and the coastdown is what removes it |
| air density from the car's own PIDs, quantisation | ±0.045 kW ≈ 0.06 PS | either way |

The last row is there to keep the effort proportionate: reading ρ from the barometer and the
ambient sensor is correct, but it is **80 times smaller than an unnoticed 1 % grade**. The
framing "density comes from the car itself rather than a constant" oversells it, and the doc
should not.

Two mitigations are procedural, not code, and belong in the command's own help:

- **Run out and back on the same stretch and average at matched speeds.** Grade and steady
  wind both reverse sign between the two runs and cancel to first order. Nothing else
  available here removes them.
- **Compare against GPS once** to settle whether this car's *bus* speed carries the
  speedometer's optimism at all, then set `--speed-scale`.

`--grade PERCENT` and `--headwind M_S` exist for a user who knows the figures; both default
to zero and both are recorded in the file.

## 4. The live view — `race/mod.rs`

`ratatui::widgets::Chart` with `Dataset` and `Marker::Braille`. The dependency is already in
`crates/vagcan/Cargo.toml`; `textplots` and `rasciigraph` would render worse and add a
crate.

```
  RUN 4.31 s                                     marks
  ┌──────────────────────────────────────────┐  0-10   0.98 s
  │ speed    62.4 km/h    bus                │  0-25   2.1 s ±0.1
  │ engine   4310 /min    bus                │  0-50   4.0 s ±0.1
  │ gear     3            bus                │  0-60   ·
  │ pedal    100 %        bus                │  0-80   ·
  │ boost    1.71 / 1.62 bar abs   bus       │  0-100  ·
  │ accel    0.41 g       computed, trailing │
  │ power    110 kW       computed, estimate │
  └──────────────────────────────────────────┘
  ┌── speed ── ← → to change ────────────────┐
  │                                ╱─────    │
  │                      ╱────────╱          │
  │            ╱────────╱                    │
  └──────────────────────────────────────────┘
    0s        2s        4s        6s
```

**A run needs no keystroke.** Arming, starting, finishing and saving all happen by
themselves; the output file names itself from the time and the VIN. The keys are for
exceptions only — cancelling, pausing the trigger, changing the series — and none of them has
to be pressed for the tool to do its job. Nothing prompts the driver while the car is moving,
and the results table appears when the car is stopped, not at the finish of the mark.

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
    mark      time          average acceleration
    0-10      1.0 s  ±0.1   2.8 m/s²
    0-100     6.1 s  ±0.1   4.5 m/s²
    50-100    3.24 s ±0.02  4.28 m/s²
    peak engine speed   6480 /min at 5.9 s
  Run 2 — computed   (mass 1400 kg, tyre 205/55R16, CdA 0.65 m², Crr 0.012,
                      ρ 1.19 kg/m³ measured, grade 0 %, window 0.30 s, central)
    distance            118 m      speed-signal bias dominates, not the integrator
    peak acceleration   5.3 m/s²   (0.54 g) at 1.21 s, gear 2, ±0.2 s mean
    peak power          112 kW     (152 PS) estimate at the contact patch
    shift 2→3           cost 0.79 km/h, 0.18 s on the 0-100
```

Marks starting at 0 print to a tenth with their systematic bound; rolling marks print to
hundredths (§1, §3). The two are not the same kind of number and the table does not pretend
they are.

The measured block holds times, the average accelerations that are `Δv/Δt` across a mark's
own endpoints, and peaks of channels the car reported. The computed block carries its
conditions in the heading: the same run under a different mass is a different set of numbers,
and a table that hides that invites the comparison it cannot support.

## 5. The saved session — raw JSON

```json
{ "tool": "vagcan race", "recorded_at": "2026-08-03T12:41:07+03:00",
  "car":      { "vin": "…", "units": [ { "request": "7E1", "part_number": "0CW300041G" } ] },
  "config":   { "marks": [[0,100]],
                "mass_kg": 1400, "tyre": "205/55R16", "rolling_radius_m": 0.313,
                "inertia_model": "wong-1+d1+d2*ratio^2", "d1": 0.04, "d2": 0.0025,
                "cda": 0.63, "cda_source": "coastdown",
                "crr": 0.0114, "crr_source": "coastdown",
                "profile": "catalogs/cars/XW8AD4NE9JH008917.json",
                "grade_percent": 0.0, "headwind_ms": 0.0,
                "air_density_kg_m3": 1.19, "air_density_source": "measured",
                "degraded": false, "cycle_median_s": 0.047,
                "speed_source": "7E1:F40D", "speed_scale": 1.0, "speed_scale_applied": "before-marks",
                "t0_method": "quadratic-fit", "t0_clamp_s": 0.048,
                "accel_window_s": 0.3, "accel_method": "central-least-squares",
                "peak_statistic": "mean-over-0.2s-neighbourhood",
                "refresh_estimate_s": 0.05, "hz": 21.4 },
  "channels": [ { "key": "speed", "name": "Vehicle speed", "unit": "km/h",
                  "origin": "read", "request": "7E1", "did": "F40D" },
                { "key": "accel", "name": "Acceleration", "unit": "m/s2",
                  "origin": "derived", "from": ["speed"],
                  "method": "central-least-squares", "window_s": 0.3 },
                { "key": "power", "name": "Power at the contact patch", "unit": "kW",
                  "origin": "derived", "estimate": true,
                  "from": ["speed", "engine_speed",
                           "barometric_pressure", "ambient_temperature"],
                  "method": "road-load" } ],
  "runs":     [ { "index": 1, "t0_wall": "…", "aborted": false,
                  "samples": [ { "t": -2.94, "speed": { "t": -2.94, "v": 0.0 } } ],
                  "marks":   [ { "from": 0, "to": 100, "seconds": 6.12,
                                 "uncertainty_s": 0.1, "from_t0": true,
                                 "avg_accel_ms2": 4.54 } ],
                  "derived": { "distance_m": 118.4, "peak_rpm": 6480,
                               "peak_power_kw": 112.3,
                               "peak_accel_ms2": 5.31, "peak_accel_t": 1.21,
                               "peak_accel_gear": "2",
                               "shifts": [ { "t": 2.44, "from": "2", "to": "3",
                                             "speed_deficit_ms": 0.22,
                                             "cost_on_mark_s": 0.18 } ] } } ] }
```

Every channel declares its `origin`. A derived one also declares what it was derived
**from** and by what **method**, with the parameters that method used — an acceleration
figure whose window is not recorded is not reproducible, and a power figure whose mass is
not recorded is not checkable. There is no `proven` flag: in `race` it would be `true` on
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
crates/vagcan/src/race/mod.rs       the command, the poll loop, the TUI
crates/vagcan/src/race/session.rs   state machine, marks, derived metrics — no I/O
crates/vagcan/src/race/power.rs     the dynamics model and air density
crates/vagcan/src/race/profile.rs   the per-VIN car profile: read, write, precedence
crates/vagcan/src/race/coastdown.rs the road-load fit
crates/vagcan/src/race/report.rs    the results table
crates/vagcan/src/race/view.rs      HTML generation
```

`session.rs` holds everything worth testing and knows nothing about adapters or terminals,
which is what makes the tests possible without a car.

## 8. Tests

| test | asserts |
|---|---|
| arming | a synthetic profile arms only after zero is held a full second |
| t0 on a **constant-jerk** launch | the quadratic fit's bias stays inside a stated bound. A *constant-acceleration* profile must not be used: linear back-extrapolation is exact there, so the obvious test is vacuous against the only failure mode that exists |
| t0 dead band | a profile whose first samples are suppressed below 2 km/h still reports its uncertainty rather than a confident number |
| marks | interpolated `0-100` on an analytic profile equals the closed-form answer |
| mark precision | a 0-based mark prints a tenth with a bound; a rolling mark prints hundredths |
| staleness cancellation | with a simulated uniform staleness, the rolling mark's error is an order below the 0-based one's |
| abort | speed returning to zero keeps the marks that closed and flags the run |
| re-arm | a second standstill starts a second run in the same session |
| trigger pause | `p` prevents arming and does not lose the previous run |
| `--marks` parser | `0-100,50-100` parses, `100-50` and `abc` are rejected |
| `--speed-scale` | a scale of 0.97 moves the detected crossing, not just the printed number |
| least squares on uneven samples | the slope of a known ramp is recovered from deliberately jittered timestamps |
| causal vs central | the two differ in **phase only** on a known profile — equal peak magnitude — and the file holds the central one |
| window attenuation | a 0.3 s synthetic dip is recovered ~36 % shallow, which is asserted rather than discovered later |
| shifts | a shift is located from the gear channel, and its cost is the integrated deficit, positive on a profile where speed never falls |
| peak statistic | on a series with injected noise the reported peak is not the maximum, and its upward bias is bounded |
| air density | ρ = **1.225 kg/m³** at 101.325 kPa and 288.15 K — the ISO 2533 sea-level value, four significant figures. Not a comparison against the same formula: this anchor catches a wrong R, a K/°C slip and a kPa/Pa slip in one assertion |
| equivalent inertia | `k` from a measured ratio exceeds 1.4 in first gear and approaches 1.05 in top |
| two modes only | a profile with a mass but no coastdown runs **minimal**, not a generic-CdA power run; `--cda` alone is refused, `--cda` with `--crr` is accepted as `stated` |
| minimal completeness | a minimal run still produces every mark, acceleration, distance and shift cost, and every read channel is in the file |
| minimal frugality | barometric pressure and ambient temperature are **not polled** with no profile, and are polled **once**, not per cycle, with one |
| profile round trip | `--setup` writes a profile the next run reads, and the run then needs no flags |
| provenance | every parameter in the file names its source, and none of them may be a guess |
| precedence | a flag beats the profile, and the file names the winner |
| coastdown fit | synthetic coastdown data with known `CdA`/`Crr` recovers both; a one-way pass is refused; two passes that disagree are rejected |
| uncertainty from data | doubling the simulated cycle time doubles the printed bound — it is computed, not tabulated |
| one time base | a derived value whose engine-speed input is a cycle stale is interpolated onto the leading grid, and one that is beyond the bound is suppressed |
| degradation | a run whose rate falls below the floor is flagged, and the flag reaches both the screen and the file |
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
- **Speedometer error — unverified, and the earlier draft asserted it.** UNECE R39 forbids a
  speedometer to under-read and permits over-reading by 10 % + 4 km/h, so the *indicated*
  speed is optimistic by regulation. Whether the **bus** value carries that optimism is a
  different question: on many VAG platforms the gearbox/ABS value is close to true wheel
  speed and the bias is added in the instrument cluster. Stating it as fact in a design
  document was out of character for this project. One GPS comparison run settles it.
  `--speed-scale` exists for whoever does that; the default is 1.0, and the file records
  both the value and that it was applied before mark detection.
- **The rolling radius** is taken from `--tyre`, a nominal size, not from a measured rolling
  circumference — which differs by a few per cent with pressure, load and wear. It feeds `k`,
  where a few per cent is second-order, and nothing else.
- **A non-driven wheel speed** would turn the launch-slip caveat into a measurement. The
  brake unit answers 48 identifiers and none of them is proven; this is the concrete reason
  to go and prove one.
