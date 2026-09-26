# dash / 19 — the cruise lever as buttons, and the stopwatch page on the board

**Subsystem:** dash · **Crates:** `vag-cli-core` (plan), `vag-dash-render` (lever, stopwatch,
screen), `vag-dash-fw` · **Needs the car:** partly (the speed factor, a run)

**State (2026-09-26):** approved by the owner. Phase 1 (the pure machines) and phase 2 (plan,
board, replay, docs) built on `feat/stalk-stopwatch`, in review for a PR, not merged. Review
fixes the same day: a run is written to flash at a standstill, never at speed (owner's
decision, below); a silent speed aborts a run; the stopwatch resets at each turn of the mode;
a mark crossed before the launch has no time; the lever's debounce starts over after adapter
mode; new plan refusals (a speed read at 5 Hz or slower, a state name given twice, a factor
too small for the board). RAM: see the PR. Nothing of it has run on the car or the bench
yet — see "On the car". The owner's `dash.toml` has no `[stalk]` or
`[stopwatch]` yet, and the owner's project cache predates the ODIS bands: `vagcan setup` has
to run again before a `[stalk]` builds (the build says so).

## What the owner asked (2026-09-26)

- The cruise lever pages the panel while cruise is **off**: **RES/+ next page, SET/− previous
  page**. The BOOT button keeps its job (short press: next page, or silence an alarm).
- **LIMIT switches the stopwatch ("measure") mode on and off.**
- One feature, the lever and the stopwatch page together (owner chose "all at once" over
  "lever first" and over "stopwatch on `F40D` first").

## What the car showed (2026-09-26, `dash/14` §6a, `research/captures/cruise-lever.csv`)

- `70C` `1105` answers over BLE at 10 Hz. Byte 8 ("Linker Hebel axial 2 (GRA Set/Rest)") is the
  rocker, byte 9 ("Linker Hebel axial 3 (ON/CANCEL/OFF)") the switch. Readings are analog ladder
  levels with ±2 of noise; ODIS gives each state as an interval (lower bounds 0/75/111/146/182/222
  and 0/75/111/147/183/222).
- Rocker: 205 rest ("neutral mit Limiterverbau"), 91 RES/+ ("beschleunigen", the engine's
  `4383` bit 3), 128 SET/− ("verzögern", bit 2). **167 ("neutral ohne Limiterverbau") is
  LIMIT, whatever its ODIS name says:** the owner pressed LIMIT twice during the capture, and 167
  appears on exactly those presses (≈0.6 s at 35.0 s, ≈0.4 s at 65.3 s, switch OFF), never at
  rest.
- Switch: 167 OFF (latched), 91 ON, 128 CANCEL (springs back to ON).
- With the switch OFF, the engine's `203C` reads 0 ("main switch off") and its `4383` stays
  `0x2000` through every rocker press: the engine ignores the lever. With ON it reads 2.
- **Press length.** A deliberate press held 0.25–0.5 s. A tap can be one read at 10 Hz: with the
  switch OFF, + at ~59.0 s and ~89.2 s and − at ~90.4 s were each a single answer (counted by
  answer time — a `watch` row repeats the last answer).

## Configuration — data, never code

Which unit, identifier, field and state a button is lives in the owner's `dash.toml`, by the
ODIS texts, resolved at plan build into intervals the board carries (`plan.rs`). Another car
names its own fields; nothing about `70C` or `1105` is in the code. **Texts match exactly, whole**:
a state is named as the project spells it, parenthesis and all.

```toml
[stalk]
read = "70C:1105"                                  # one identifier: the rocker's and the switch's
rocker = "Linker Hebel axial 2 (GRA Set/Rest)"     # field names as the ODIS project spells them
switch = "Linker Hebel axial 3 (ON/CANCEL/OFF)"
next = "Blinker GRA beschleunigen"                 # states of the rocker
previous = "Blinker GRA verzögern"
measure = "Blinker GRA neutral ohne Limiterverbau (weder beschleunigen noch verzoegern)"  # LIMIT
switch_off = "Blinker GRA Aus"                     # state of the switch
cruise = "01:203C"                                 # the engine's cruise status, an enumerated field
cruise_off = "main switch off"                     # its state that means off

[stopwatch]
speed = "02:380B"            # gearbox output shaft speed, a [[channel]] (give it hz = 50)
km_h_per_unit = 0.0          # measured on the car (below); 0 = not measured, the page says so
marks = [60, 100]            # km/h, at most 3
```

`dev dash build` refuses, by name (`vag-cli-core/src/dash.rs`, every one tested):

| what | message |
|---|---|
| the unit of `read` or `cruise` not in the survey | `unit 70C is not in the survey — …` |
| `read` or `cruise` the survey asked for and the car did not answer | `…: the survey asked the unit for this identifier and it did not answer` |
| a field `read` does not have | `rocker: 16:1105 has no field named "…" — its fields are …` |
| a field name `read` gives twice | `rocker: 16:1105 has 2 fields named "…"` |
| a field that is a quantity | `rocker: "…" is a quantity, not a list of states` |
| a state the field does not have | `next: "…" is not a state of "…" — its states are …` |
| a state name the field gives twice | `next: "…" has 2 states named "…" — which one is the button cannot be told` |
| two buttons on one state | `next, previous and measure name the same state` |
| rocker and switch the same field | `rocker and switch are both "…"` |
| `read` not an identifier the variant declares | `read …: the car's variant declares no such identifier` |
| `read` with a bit offset | `read … is not <unit>:<DID>` (at parse) |
| `cruise` not a field / a quantity / no such state | `cruise …: the car's variant declares no such field` / `a quantity, not a list of states` / `cruise_off: "…" is not a state of …` |
| `cruise` matching several enumerated rows | `…: 2 rows answer to it — name one by identifier and bit offset: …` |
| a unit whose cached states predate their bands | `unit 70C: the project's cache keeps only the lower end of each state — run vagcan setup again` |
| `speed` not a `[[channel]]` | `speed … is not in the [[channel]] list` |
| `speed` with an offset in its scaling | `speed …: its scaling has an offset (…) — a standstill is the channel's zero` |
| `speed` with a factor not above zero (0 or negative) | `speed …: its scaling's factor … is not above zero` |
| `speed` read at 5 Hz or slower | `speed … is read at 2 Hz — the launch fit needs 3 samples in its first 400 ms, so faster than 5 Hz; give its [[channel]] an hz, 50 or more` |
| a mark of 0, a mark twice, more than 3, none | at parse: `mark 0 is not a whole speed in km/h above 0`, `mark 60 is listed twice`, `has 4 marks, and the page holds 1 to 3` |
| `km_h_per_unit` below 0, not a number, or too large for an `f32` | at parse: `km_h_per_unit must be a number at or above 0` |
| `km_h_per_unit` above 0 but too small for an `f32` | at parse: `km_h_per_unit … is too small for the board, which would hold it as 0 — not measured` |

A build message starts with `[stalk]` or `[stopwatch]`, one at parse with `dash.toml: [stalk]` or
`dash.toml: [stopwatch]`; the unit-not-in-survey, not-answered and ambiguous rows are the
build's general messages and carry neither.

The lever's three fields join the plan as channels of their own, raw (×1 +0), on no page.
`plan.json` keeps each state's name beside its interval for a person; `plan.rs` carries the
intervals (`Band`), the states' places, and the marks. A `[stopwatch]` with no `[stalk]` builds
with a note: nothing enters the page.

## Behaviour (as built)

**The gate.** The lever is ours only when **both** say off: `cruise` reads `cruise_off` and
`switch` reads `switch_off`. Stale or absent counts as **on** — a lost read never turns a
cruise press into a page turn. The cruise status counts only if it is younger than three of its
periods (600 ms). Never CANCEL (`dash/14` §6a: plus after CANCEL resumes).

**A press.** A state counts once **two consecutive reads** agree on it; a press fires on the read
that confirms an edge into `next`/`previous`/`measure`, with the gate open on both reads.
Holding does not repeat; one noisy read never fires; a missing read breaks the pair; a lever
already held when reading starts is not a press. `Stalk::read` is fed **once per new answer**
of the rocker's identifier, never per frame — the same answer counted twice would confirm a
transitional ladder level and fire `measure`. What that needs of a press: two reads, so
≈0.1 s at 20 Hz (gate open) and ≈0.2 s at 10 Hz (stopwatch up). **Hold longer to be sure; a
shorter tap can be missed.** After adapter mode the debounce starts over (`Stalk::lost`): a read
from before it pairs with nothing, and the gate is closed until a read opens it.

**Rates** (`Plan::rates_in`, one scheduler, no second owner of the bus). Fixed in code, not the
owner's `hz`: they are what makes the lever a button and the gate safe, not a view preference.

| channel | rate |
|---|---|
| the rocker (its identifier; the switch is read out of the same answer, never on its own) | 20 Hz gate open, 2 Hz closed; 10 Hz while the stopwatch is up with a factor (with `km_h_per_unit = 0` it stays at 20 / 2 Hz) |
| the cruise status | 5 Hz |
| the stopwatch's speed | its own `hz`, foreground, while the mode is on with a factor; with `km_h_per_unit = 0` not read for the stopwatch |
| page cells, while the stopwatch page is on the glass | background (at most 1 Hz), in every phase — `STOP`, `GO`, `RUN`, `DONE`, `NO FACTOR` |
| an alarm's page shown over the stopwatch | its cells foreground, except while the stopwatch is armed or running (`Timing::Timing`): then background |
| the channels an alarm watches | their own `hz`, always |

**Alarms first.** While an alarm holds the screen, any lever press does what BOOT's short press
does there: silence the episode — LIMIT too, in a plan with no `[stopwatch]`. Alarms take the screen during the stopwatch too, and hand it
back to the stopwatch, not to the page under it (`GLASS_PAGE` stays the stopwatch's).

**The stopwatch page.** LIMIT enters it from any page and leaves it back to the page it came
from — the cursor never moves while it is up. While it is up the lever's + and − do nothing;
BOOT keeps its job: it turns the page, and a page on the screen ends the stopwatch. The adapter
screen ends it too. Whenever the mode turns on or off, however, the machine is reset — at the
turn, before the next sample, not on the next frame: `Screen::stopwatch_turns` counts the
turns and `Stopwatch::follow` resets on a count it has not seen, so an off and an on between
two samples still start over.

- Speed 0 (the channel's zero) for 1 s arms it; the first sample above 0 starts the run;
  back at 0 before the highest mark aborts it.
- **A silent speed ends it** (`SILENCE_MS` = 500): no answer for over 500 ms aborts a run in
  progress and disarms an armed standstill, which then has to be seen again for a whole second.
  Nothing is interpolated across the gap. Checked on every answer and on every panel frame, so
  a unit that stops answering altogether still ends the run. A hold not yet armed is left alone.
- The clock's origin is the launch as `vag-cli-measure` reconstructs it (`derive::start`,
  ported): the midpoint of a constant-jerk fit through `√v` over the first 0.4 s of movement and
  a line through the first two moving samples. It needs three moving samples in that 0.4 s: the
  speed must be read faster than 5 Hz, and the plan build refuses a slower one (at 50 Hz it
  has 20). Without a fit there is no time,
  only the crossings — a launch invented from two samples is not a measurement.
- Each mark is stamped where the speed first rises past it, interpolated between the samples
  either side, at the answer's own time. **A mark crossed before the launch has no time**
  (`Run::time` is `None`): the first moving sample already past a low mark says it was crossed,
  not when. `vag-cli-measure` likewise looks for a crossing only after the launch.
- The page is one values row: the phase (`STOP` / `GO` / `RUN` / `DONE`, Russian with a Russian
  plan) over the speed in km/h, then each mark's time — two decimals under 10 s, one under 100.
  With `km_h_per_unit = 0` it says `NO FACTOR` and nothing else, and the speed is not read.
- **Only a finished run is kept, and never written at speed** (owner's decision, 2026-09-26: no
  flash write at speed — a write erases a sector with the executor stalled, the glass frozen,
  answers and BLE events missed). The run finishes at its highest mark and is kept in RAM
  (`run_pending`). It is written at the next standstill the stopwatch sees — phase armed, a
  standstill held 1 s (`store_run`) — or by an explicit `save`. With no other unsaved change
  the whole configuration is written; with other unsaved changes only the run is added to the
  configuration flash already holds, and those changes stay unsaved for the owner to keep or
  not. One try: a run that could not be written waits for `save`. `load`, `defaults` and
  `erase` drop a pending run. **The write happens only when the stopwatch reaches armed, and
  the stopwatch is fed only while its mode is on.** A driver who leaves the stopwatch (LIMIT or
  the button) before stopping keeps the run in RAM only: it is lost at ignition off unless
  `save` is sent, or the stopwatch is turned on again and arms first.
- An aborted run is shown until the page is left, never stored; a finished run with no times
  (no launch fit) replaces no stored run. A stored run outlives a stored configuration whose
  pages no longer fit the plan.
- Settings schema 2 adds the run. The board carries old v1 settings forward and writes v2 only
  on its next save.

**The factor.** Measured, never written in (`dash/14` §6): on a steady stretch, `vagcan watch
--did "02:380B 01:F40D" --out steady.csv` (one `--did`; one group per unit, spaces between),
then `vagcan dev recording calibrate` fits km/h against `380B`; the fit goes into `km_h_per_unit`. Check first that `calibrate` can take a
recording with those two columns; if not, extend it (a separate commit).

## Where it lives

- `vag-dash-render/src/stalk.rs` — the gate and the press detector.
- `vag-dash-render/src/stopwatch.rs` — the machine, the launch fit, the page's cells.
- `vag-dash-render/src/screen.rs` — `Screen::lever`, `stopwatch()`, `stopwatch_on_glass()`,
  `stopwatch_turns()`.
- `vag-dash-render/src/plan.rs` — `StalkPlan`, `StopwatchPlan`, `Band`, `state_of`, `rates_in`.
- `vag-cli-core/src/dash.rs` — `[stalk]` / `[stopwatch]` parsed, resolved, refused, written.
- `vag-dash-fw/src/bin/dash.rs` — the lever fed per answer in the bus task, the stopwatch in a
  static, the page drawn, the silence checked and the run kept and stored at a standstill by the
  panel task; `src/schema.rs` — the settings
  record and its versions (host test: `research/dash/host/tests/settings_schema.rs`).
- `vagcan dev recording dash` says it replays neither the lever nor the stopwatch. `dashsim`
  shows whatever frame the board sends, the stopwatch's included; it has no lever key.

## Tests (hardware-free)

- Plan build: the texts resolve to intervals; each refusal above; `plan.json` round trip; the
  board's `state_of` agrees with the laptop's `level_for` value by value, on a ladder whose
  order of trying matters.
- The generated source, compiled in CI: `vag-cli-core/tests/generated_plan.rs` `include!`s
  `tests/fixtures/lever_plan.rs` (a plan with a lever — on a ladder with unbounded states —
  and a stopwatch, from the test fixture, not a car) under `deny(warnings)` — CI builds the firmware on an empty plan, so nothing else
  compiles that half of `to_rust`. A unit test fails when the fixture is not what `to_rust`
  writes; `BLESS=1 cargo test -p vag-cli-core generated_source` rewrites it.
- Gate and press: open only with both off; stale either → closed; CANCEL → closed; an edge held
  two reads fires once; one noisy read does not; holding does not repeat; held at start is not a
  press; the gate must be open on both reads; a read from before adapter mode is not half of a
  press.
- Screen: previous page wraps; an alarm up → any lever press silences; measure in and out back to
  the page; + and − ignored during the stopwatch; BOOT pages and ends it; the adapter ends it.
- Stopwatch: arming, start, launch midpoint, marks interpolated, abort, finish, reset, factor 0,
  a poll too slow for a fit, and the launch against a transcription of `derive::start` at 20, 100
  and 250 Hz to a microsecond; a silence aborts a run (with and without an answer after it) and
  disarms a standstill; a mark crossed before the launch has no time; a turn of the mode resets
  once; the page fits the panel in both languages.
- Rates: the three lever rates, the cruise status, the speed while up, page cells while timing.
- Settings: a schema-1 byte image loads with every field intact.
- Not host-tested: the firmware's `keep_run` / `store_run` (the run written at a standstill) —
  the firmware does not build for the host. Checked on the board.

## On the car

1. `vagcan setup` again, so the cache keeps the bands; then add `[stalk]` / `[stopwatch]` and
   build.
2. Paging: +, −, LIMIT with cruise off; nothing with cruise on or after CANCEL.
3. The factor on a steady stretch.
4. **A 0–100 run, compared through the board.** The laptop measures the same run with
   `vagcan measure --ble` on `380B` with the same factor. **Never with a second adapter
   (CANable) on the port while the board polls** — two testers on one bus breaks the one-owner
   rule.
5. After the run, stop with the stopwatch up: once the car has stood 1 s the board notes
   `stopwatch: the run is saved`, and the run's times survive a power cycle.
