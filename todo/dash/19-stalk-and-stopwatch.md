# dash / 19 — the cruise lever as buttons, and the stopwatch page on the board

**Subsystem:** dash · **Crates:** `vag-cli-core` (plan), `vag-dash-render` (lever, stopwatch,
screen), `vag-dash-fw` · **Needs the car:** partly (the speed factor, a run)

**State (2026-09-26):** approved by the owner. Phase 1 (the pure machines) and phase 2 (plan,
board, replay, docs) built on `feat/stalk-stopwatch`, not merged. Nothing of it has run on the
car or the bench yet — see "On the car". The owner's `dash.toml` has no `[stalk]` or
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
| a field `read` does not have | `rocker: 16:1105 has no field named "…" — its fields are …` |
| a field that is a quantity | `rocker: "…" is a quantity, not a list of states` |
| a state the field does not have | `next: "…" is not a state of "…" — its states are …` |
| two buttons on one state | `next, previous and measure name the same state` |
| rocker and switch the same field | `rocker and switch are both "…"` |
| `read` not an identifier the variant declares | `read …: the car's variant declares no such identifier` |
| `read` with a bit offset | `read … is not <unit>:<DID>` (at parse) |
| `cruise` not a field / a quantity / no such state | `cruise …: the car's variant declares no such field` / `a quantity, not a list of states` / `cruise_off: "…" is not a state of …` |
| a unit whose cached states predate their bands | `unit 70C: the project's cache keeps only the lower end of each state — run vagcan setup again` |
| `speed` not a `[[channel]]` | `speed … is not in the [[channel]] list` |
| `speed` with an offset in its scaling | `speed …: its scaling has an offset (…) — a standstill is the channel's zero` |
| `speed` with a factor not above zero | `speed …: its scaling's factor … is not above zero` |
| a mark of 0, a mark twice, more than 3, none | at parse: `mark 0 is not a whole speed in km/h above 0`, `mark 60 is listed twice`, `has 4 marks, and the page holds 1 to 3` |
| `km_h_per_unit` below 0 or not a number | at parse: `km_h_per_unit must be a number at or above 0` |

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
transitional ladder level and fire `measure`. What that needs of a press: held ≥ 0.1 s it is
sure to register at 20 Hz (gate open), ≥ 0.2 s at 10 Hz (stopwatch up). **A shorter tap can be
missed — hold to register.**

**Rates** (`Plan::rates_in`, one scheduler, no second owner of the bus). Fixed in code, not the
owner's `hz`: they are what makes the lever a button and the gate safe, not a view preference.

| channel | rate |
|---|---|
| the rocker (its identifier; the switch is read out of the same answer, never on its own) | 20 Hz gate open, 2 Hz closed, 10 Hz while the stopwatch is up |
| the cruise status | 5 Hz |
| the stopwatch's speed | its own `hz`, foreground, while the mode is on |
| page cells, while the stopwatch is armed or running | background (1 Hz); an alarm's channels keep theirs |

**Alarms first.** While an alarm holds the screen, any lever press does what BOOT's short press
does there: silence the episode — LIMIT too, in a plan with no `[stopwatch]`. Alarms take the screen during the stopwatch too, and hand it
back to the stopwatch, not to the page under it (`GLASS_PAGE` stays the stopwatch's).

**The stopwatch page.** LIMIT enters it from any page and leaves it back to the page it came
from — the cursor never moves while it is up. While it is up the lever's + and − do nothing;
BOOT keeps its job: it turns the page, and a page on the screen ends the stopwatch. The adapter
screen ends it too. Whenever the mode turns on or off, however, the machine is reset.

- Speed 0 (the channel's zero) for 1 s arms it; the first sample above 0 starts the run;
  back at 0 before the highest mark aborts it.
- The clock's origin is the launch as `vag-cli-measure` reconstructs it (`derive::start`,
  ported): the midpoint of a constant-jerk fit through `√v` over the first 0.4 s of movement and
  a line through the first two moving samples. It needs three moving samples in that 0.4 s: the
  speed must be read faster than ≈5 Hz (at 50 Hz it has 20). Without a fit there is no time,
  only the crossings — a launch invented from two samples is not a measurement.
- Each mark is stamped where the speed first rises past it, interpolated between the samples
  either side, at the answer's own time.
- The page is one values row: the phase (`STOP` / `GO` / `RUN` / `DONE`, Russian with a Russian
  plan) over the speed in km/h, then each mark's time — two decimals under 10 s, one under 100.
  With `km_h_per_unit = 0` it says `NO FACTOR` and nothing else, and the speed is not read.
- **Only a finished run is kept**: saved to flash at once when nothing else is unsaved, else kept
  with the other unsaved changes. An aborted run is shown until the page is left, never stored; a
  finished run with no times (no launch fit) replaces no stored run. A stored run outlives a
  stored configuration whose pages no longer fit the plan.
  Settings schema 2 adds the run; a board's schema-1 record is read and carried forward, not
  discarded.

**The factor.** Measured, never written in (`dash/14` §6): on a steady stretch, `vagcan watch
--did 02:380B 01:F40D --out steady.csv`, then `vagcan dev recording calibrate` fits km/h
against `380B`; the fit goes into `km_h_per_unit`. Check first that `calibrate` can take a
recording with those two columns; if not, extend it (a separate commit).

## Where it lives

- `vag-dash-render/src/stalk.rs` — the gate and the press detector.
- `vag-dash-render/src/stopwatch.rs` — the machine, the launch fit, the page's cells.
- `vag-dash-render/src/screen.rs` — `Screen::lever`, `stopwatch()`, `stopwatch_on_glass()`.
- `vag-dash-render/src/plan.rs` — `StalkPlan`, `StopwatchPlan`, `Band`, `state_of`, `rates_in`.
- `vag-cli-core/src/dash.rs` — `[stalk]` / `[stopwatch]` parsed, resolved, refused, written.
- `vag-dash-fw/src/bin/dash.rs` — the lever fed per answer in the bus task, the stopwatch in a
  static, the page drawn and the run kept by the panel task; `src/schema.rs` — the settings
  record and its versions (host test: `research/dash/host/tests/settings_schema.rs`).
- `vagcan dev recording dash` says it replays neither the lever nor the stopwatch. `dashsim`
  shows whatever frame the board sends, the stopwatch's included; it has no lever key.

## Tests (hardware-free)

- Plan build: the texts resolve to intervals; each refusal above; `plan.json` round trip; the
  generated source; the board's `state_of` agrees with the laptop's `level_for` value by value.
- Gate and press: open only with both off; stale either → closed; CANCEL → closed; an edge held
  two reads fires once; one noisy read does not; holding does not repeat; held at start is not a
  press; the gate must be open on both reads.
- Screen: previous page wraps; an alarm up → any lever press silences; measure in and out back to
  the page; + and − ignored during the stopwatch; BOOT pages and ends it; the adapter ends it.
- Stopwatch: arming, start, launch midpoint, marks interpolated, abort, finish, reset, factor 0,
  a poll too slow for a fit, and the launch against a transcription of `derive::start` at 20, 100
  and 250 Hz to a microsecond; the page fits the panel in both languages.
- Rates: the three lever rates, the cruise status, the speed while up, page cells while timing.
- Settings: a schema-1 byte image loads with every field intact.

## On the car

1. `vagcan setup` again, so the cache keeps the bands; then add `[stalk]` / `[stopwatch]` and
   build.
2. Paging: +, −, LIMIT with cruise off; nothing with cruise on or after CANCEL.
3. The factor on a steady stretch.
4. **A 0–100 run, compared through the board.** The laptop measures the same run with
   `vagcan measure --ble` on `380B` with the same factor. **Never with a second adapter
   (CANable) on the port while the board polls** — two testers on one bus breaks the one-owner
   rule.
