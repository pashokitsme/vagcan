# dash / 19 — the cruise lever as buttons, and the stopwatch page on the board

**Subsystem:** dash · **Crates:** `vag-cli-core` (plan), `vag-dash-render` (lever, stopwatch,
screen), `vag-dash-fw` · **Needs the car:** partly (the speed factor, a run)

**State (2026-09-27):** merged, PR #12 (`6fa2bc8`). In the owner's `dash.toml` since 2026-09-27:
`vagcan setup` re-run, the plan builds, and the lever's bands hold the capture's readings
(`todo/README.md`). Not flashed; nothing of it has run on the car or the bench — "On the car".

**2026-09-26:** approved by the owner. Phase 1 (the pure machines) and phase 2 (plan,
board, replay, docs) built on `feat/stalk-stopwatch`, in review for a PR, not merged. Review
fixes the same day: a run is written to flash at a standstill, never at speed (owner's
decision, below); a silent speed aborts a run; the stopwatch resets at each turn of the mode;
a mark crossed before the launch has no time; the lever's debounce starts over after adapter
mode; new plan refusals (a speed read too seldom for the launch fit — since `f685bac`, less often
than every 133 ms, i.e. slower than 7.5 Hz — a state name given twice, a factor too small for the
board). RAM: see the PR. Nothing of it has run on the car or the bench
yet — see "On the car". The owner's `dash.toml` has no `[stalk]` or
`[stopwatch]` yet, and the owner's project cache predates the ODIS bands: `vagcan setup` has
to run again before a `[stalk]` builds (the build says so).

**2026-09-27, in PR #12:** input backends, approved by the owner — `[[button]]`s on GPIO 3, 4
and 5, one command path for every input, and BOOT no longer an input. Built on
`fix/pr12-input`; see "Input backends". Not on the board yet.

**2026-09-27, in PR #12:** the lever closes the stopwatch — cruise taken, or its data missing
over 3 s (owner's decision on the review's open question). Built on `fix/pr12-gate-close`; see
"The lever closes the stopwatch". Not on the board yet. Same branch, review fixes to the run's
write: it happens at the standstill **before** `GO` (the stopwatch does not arm while it waits);
a board whose flash holds nothing writes the defaults and the run; `run_pending` stays set
until the run is in flash; the no-BLE image's boot note no longer offers a `save` it has not.

**2026-09-27, in PR #12, UX and docs review (round 2):** `GO` is drawn inverted; an aborted run
reads `ПРЕРВАН` in Russian (`СБРОС` read as "reset"); the plan build notes a board nothing pages
or silences, and a `[stalk]` with no `[stopwatch]`; `dashcfg` shows `run_pending`;
`SLOWEST_SPEED_PERIOD_MS` is the build's 133 ms (it was 200). Built on `fix/pr12-polish`.

## What the owner asked (2026-09-26)

- The cruise lever pages the panel while cruise is **off**: **RES/+ next page, SET/− previous
  page**. ~~The BOOT button keeps its job (short press: next page, or silence an alarm).~~
  Superseded 2026-09-27: BOOT is technical, not an input — see "Input backends".
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
speed = "02:IDE00075"        # the gearbox's vehicle speed in km/h, a [[channel]] (hz 7.5 or more)
km_h_per_unit = 1.0          # km/h already (2026-09-27, below); 0 = not measured, the page says so
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
| `speed` read less often than every 133 ms (slower than 7.5 Hz), on the board's rounded period | `speed … is read every 500 ms (hz = 2) — the launch fit needs 3 samples in its first 400 ms even when one answer is late, so a reading every 133 ms or sooner: give its [[channel]] an hz of 7.5 or more; 50 is recommended` |
| a mark of 0, a mark twice, more than 3, none | at parse: `mark 0 is not a whole speed in km/h above 0`, `mark 60 is listed twice`, `has 4 marks, and the page holds 1 to 3` |
| `km_h_per_unit` below 0, not a number, or too large for an `f32` | at parse: `km_h_per_unit must be a number at or above 0` |
| `km_h_per_unit` above 0 but too small for an `f32` | at parse: `km_h_per_unit … is too small for the board, which would hold it as 0 — not measured` |

A build message starts with `[stalk]` or `[stopwatch]`, one at parse with `dash.toml: [stalk]` or
`dash.toml: [stopwatch]`; the unit-not-in-survey, not-answered and ambiguous rows are the
build's general messages and carry neither.

The lever's three fields join the plan as channels of their own, raw (×1 +0), on no page.
`plan.json` keeps each state's name beside its interval for a person; `plan.rs` carries the
intervals (`Band`), the states' places, and the marks. A `[stopwatch]` that nothing opens — no
`[stalk]`, no `[[button]]` with `action = "stopwatch"` — builds with a note saying so.

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
| the cruise status | 5 Hz, while the rocker's unit answers as the plan's (its part number matched); not read otherwise |
| the stopwatch's speed | its own `hz` while the mode is on with a factor: foreground while `STOP` / `DONE` / `ABORT` (`Timing::Up`), and `Class::Timing` — the board's timing channel, ahead of a host's reads — while armed or running (`Timing::Timing`), unless a host already holds that channel: then foreground. With `km_h_per_unit = 0` not read for the stopwatch |
| page cells, while the stopwatch page is on the glass | background (at most 1 Hz), in every phase — `STOP`, `GO`, `RUN`, `DONE`, `ABORT`, `NO FACTOR`: the stopwatch shows no page's cells |
| an alarm's page shown over the stopwatch | its cells foreground, except while the stopwatch is armed or running (`Timing::Timing`): then background |
| the channels an alarm watches | their own `hz`, always |

**Alarms first.** While an alarm holds the screen, any command — the lever, a `[[button]]`,
`dashsim` — silences the episode and does nothing else: `Stopwatch` too, in a plan with no
`[stopwatch]`. Alarms take the screen during the stopwatch too, and hand it
back to the stopwatch, not to the page under it. `GLASS_PAGE` follows the glass: the alarm's
page while it holds it — so that page's cells are read, in the background while a run is armed
or running — and `STOPWATCH_PAGE` again at the hand-back.

**The stopwatch page.** A `Stopwatch` command (LIMIT, or a stopwatch `[[button]]`) enters it
from any page and leaves it back to the page it came from — no command moves the cursor while it
is up (`dashcfg`'s `set page` still can, and the stopwatch then leaves to that page). While it is
up `Next` and `Previous` do nothing, whoever gives them, `dashsim` included: only `Stopwatch`
leaves it (2026-09-27, "Input backends"). The adapter screen ends it too, and so does the lever,
with a `[stalk]` in the plan: cruise engaged on two reads in a row closes it at once, and the
gate not seen open for over 3 s closes it (below, "The lever closes the stopwatch"). A run under
way is dropped either way. Whenever the mode turns on or off, however, the machine is reset — at the
turn, before the next sample, not on the next frame: `Screen::stopwatch_turns` counts the
turns and `Stopwatch::follow` resets on a count it has not seen, so an off and an on between
two samples still start over.

- Speed 0 (the channel's zero) for 1 s arms it (`GO`); the first sample above 0 starts the run;
  back at 0 before the highest mark aborts it. While a finished run waits for its write, the
  standstill writes it first and arms after (below): never `GO` before the write.
- **A silent speed ends it** (`SILENCE_MS` = 1300): no answer for over 1.3 s aborts a run in
  progress and disarms an armed standstill, which then has to be seen again for a whole second.
  Nothing is interpolated across the gap. Checked on every answer and on every panel frame, so
  a unit that stops answering altogether still ends the run. A hold not yet armed is left alone.
  1.3 s outlasts two answer timeouts and the slowest speed period the build allows
  (`SLOWEST_SPEED_PERIOD_MS`, 133 ms) between two speed answers (the firmware asserts it), not a
  run of `7F xx 78` (response pending), which may hold the bus for up to 10 s
  (`PENDING_DEADLINE`). Accepted (PR #12 review): a unit that holds the bus that long during a
  run is a bus in trouble, and the run aborts.
- The clock's origin is the launch as `vag-cli-measure` reconstructs it (`derive::start`,
  ported): the midpoint of a constant-jerk fit through `√v` over the first 0.4 s of movement and
  a line through the first two moving samples. It needs three moving samples in that 0.4 s, even
  with one answer late: the speed must be read every 133 ms or sooner (7.5 Hz), and the plan
  build refuses a slower one (at 50 Hz it has 21, both ends counted). Without a fit there is no time,
  only the crossings — a launch invented from two samples is not a measurement.
- Each mark is stamped where the speed first rises past it, interpolated between the samples
  either side, at the answer's own time. **A mark crossed before the launch has no time**
  (`Run::time` is `None`): the first moving sample already past a low mark says it was crossed,
  not when. `vag-cli-measure` differs here: it interpolates the pair that straddles the launch and
  reports a zero or negative time (`stopwatch.rs`, `Run::time`).
- The page is one values row: the phase — `STOP` / `GO` / `RUN` / `DONE` / `ABORT`, with a
  Russian plan `СТОП` / `ПУСК` / `ЗАМЕР` / `ГОТОВО` / `ПРЕРВАН` — over the speed in km/h, then each
  mark's time — two decimals under 10 s, one under 100. Armed (`GO`), the phase's cell is drawn
  inverted, as an alarm's is (PR #12 review: `STOP 0` and `GO 0` differed by a small word). With
  `km_h_per_unit = 0` it says `NO FACTOR` (`НЕТ КОЭФ`) and nothing else, and the speed is not read.
- **Only a finished run is kept, and never written at speed** (owner's decision, 2026-09-26: no
  flash write at speed — a write erases a sector with the executor stalled, the glass frozen,
  answers and BLE events missed). The run finishes at its highest mark and is kept in RAM
  (`run_pending`). It is written at the next standstill the stopwatch sees, **before `GO`**: while
  a run waits for its write the stopwatch does not arm (the page stays `STOP` or `DONE`); once
  the car has stood the arming hold (1 s) and the zero answer is fresh, the board writes
  (`store_run`), and the next zero answer arms at once. Before (PR #12 review) the write came
  0.5 s past `GO`, just as a driver who saw it set off, while the speed still read 0 — and the
  erase stalled the launch fit's first answers. Or by an explicit `save`. With no other unsaved
  change the whole configuration is written; with other unsaved changes only the run is added to
  the configuration flash already holds — or, **flash holding none** (a board never saved, or
  erased), to the defaults the next boot would run on — and those changes stay unsaved for the
  owner to keep or not. One try: a run that could not be written stays `run_pending` until
  `save`, and the stopwatch arms after the try either way. `load` and `defaults` drop a pending
  run; `erase` keeps it pending — into the defaults at the next standstill if it has not had its
  try, for `save` if it has. A configuration flash could not be read at boot counts as unsaved,
  so a run is added to it, never the defaults written over it. **The write happens only at a
  standstill the stopwatch sees, and the stopwatch is fed only while its mode is on.** A driver
  who leaves the stopwatch (LIMIT, a stopwatch button, or the lever's close) before stopping keeps
  the run in RAM only: it is lost at ignition off unless `save` is sent, or the stopwatch is
  turned on again and the car stands. `save` is BLE's: the no-BLE image has the standstill alone
  ([`dash/21`](21-runs-in-flash.md)).
- An aborted run (`ABORT`: back at 0 before the highest mark, or the speed silent) is shown until
  the next run starts or the page is left, never stored; a finished run with no times
  (no launch fit) replaces no stored run. A stored run outlives a stored configuration whose
  pages no longer fit the plan.
- Settings schema 2 adds the run. The board carries old v1 settings forward and writes v2 only
  on its next save.

**The factor.** None on the reference car since 2026-09-27 (owner): the speed is the gearbox's
vehicle speed, already km/h (`02:IDE00075`, `F40D` ×0.01), so `km_h_per_unit = 1`. Before that
it was the output shaft speed `380B`, whose factor was to be fitted on a steady stretch
(`dash/14` §6). A car whose `speed` does not read km/h still needs its factor:
`docs/dash/dash-toml.md`, "Measure it".

## Input backends (2026-09-27)

**What the owner asked**, translated: "The buttons have to be pulled out into a separate backend
for controlling the screens. I plan a configuration where the screen is controlled by buttons on
pins, not by the stalk." His answers: sources combine any way (stalk only, buttons only, both,
neither); each button has exactly one action — `next`, `previous` or `stopwatch`, the lever's
+ / − / LIMIT — with no long press; pins are configured in `dash.toml`. Later the same day:
**"the boot/reset buttons must not affect the board. They are technical, nothing more."**

**One command, one entry point.** `vag-dash-render/src/control.rs` has
`Command { Next, Previous, Stopwatch }`. Every input turns a press into one;
`Screen::command(cmd, &mut cursor, pages) -> Outcome` applies it and never learns the source.
It replaced `Screen::press` and `Screen::lever`. The rules, first match wins:

| where | `Next` / `Previous` | `Stopwatch` |
|---|---|---|
| 1. the adapter screen | move the cursor, wrapping | ignored (`Ignored`) |
| 2. an alarm on the glass | silences it (`Silenced`) | silences it (`Silenced`) |
| 3. the stopwatch up | ignored (`StopwatchHeld`) | leaves it (`StopwatchOff`) |
| 4. otherwise | turn the page, wrapping (`Paged`) | opens it (`StopwatchOn`); with no `[stopwatch]` in the plan, `NoStopwatch`, logged |

Only `Stopwatch` enters or leaves the stopwatch, and only an input that has it can; the
adapter screen (`--slcan`) ends it too, and so do the lever's close rules (next section).
**The behaviour change:** a page turn no longer ends the
stopwatch. Before, BOOT's short press (and `dashsim`'s, which went through it) turned the page
and so left the stopwatch.

**The lever closes the stopwatch (owner, 2026-09-27).** The review's open question: with the
lever as the only `Stopwatch` input, a gate that closed while the stopwatch was up — cruise
switched on, its status stale, the rocker's unit silent — kept the stopwatch on the glass until
the gate opened again, which a silent unit never does; the gauge pages and, with no `[[button]]`,
alarm silencing were gone until a power cycle. The owner chose both behaviours:

- **Cruise taken → the stopwatch closes at once.** Two consecutive reads that both say cruise is
  engaged: the switch in a state other than `switch_off`, or the cruise status in one other than
  `cruise_off`. Positive evidence, never a missing read; the pair is of lever reads, as for a
  press, so one noisy switch read never ends a run. The cruise status is read at 5 Hz and the
  lever at 10–20 Hz, so one cruise-status answer that is not off fills both reads of the pair
  and closes it (`stalk.rs`, `Closer`). Taken as intended: the capture's `203C` has no
  single-answer excursion (725 answers, shortest run 7). A run in progress aborts, as when the
  page is left. A run that **finished** before the close stays in RAM: the speed is no longer
  read, so no standstill writes it, and it is lost at ignition off unless `save` — filed with
  `todo/dash/21-runs-in-flash.md`.
- **The gate closed for lack of data → the stopwatch stays** — stale, unanswered, NRC, a reading
  no state claims, the unit silent — a run included, until the gate has not been seen open for
  over 3 s (`stalk::STALE_CLOSE_MS`): then it closes. A read that shows the gate open starts the
  3 s over; an engaged read that finds no pair does not.
- Whatever opened the stopwatch, the lever or a button. Only with a `[stalk]` in the plan;
  without one nothing changes. These rules only ever close it: never open it, never page. Whether
  a press pages is still the gate's alone.

How: `stalk::Closer`, pure and clock-free like the stopwatch — fed every read `Stalk::read` gets,
at the same moment, and ticked by the bus task's clock between them (a silent unit answers
nothing). Level, not edge: while a rule holds, every read and tick says so, and the bus task
closes the stopwatch if it is up. **Delivered directly, not through the command queue:**
`Screen::close_stopwatch`, a sibling of `Screen::adapter`, called from the bus task where the
lever is read. `Screen::set_stopwatch` stays the one place the mode changes. Not the queue,
because a close is no driver's command: through `Screen::command` it would silence an alarm
instead, a full queue would drop it, and it would wait behind a flash write holding the settings.
The bus task wakes for the stale close (`Closer::due`) only while the stopwatch is up. Logged:
`lever: cruise engaged — stopwatch closed`, `lever: cruise not seen off for 3 s — stopwatch
closed`.

**Follows from the rule, for the owner to know:** a stopwatch opened by a `[[button]]` closes
too while the lever's data is missing — at once if the gate has not been seen open for 3 s (a
lever unit that never answered, or whose part number did not match), and within two reads while
cruise is on. With a `[stalk]` in the plan, a button stopwatch needs the lever readable and
cruise off.

**The backends.** Each is a small machine that produces `Option<Command>`:

| backend | where it runs | `Next` | `Previous` | `Stopwatch` |
|---|---|---|---|---|
| `[[button]]` on a pin | input task, every 5 ms | `action = "next"` | `action = "previous"` | `action = "stopwatch"` |
| the lever (`Stalk::read`) | bus task, once per answer | rocker `next` | rocker `previous` | rocker `measure` |
| `dashsim` (`control::remote`) | input task, on `BTN S` / `BTN L` | `BTN S` | — | — |

- **A pin button** (`control::PinButton`) is `button.rs`'s machine, unchanged: 10 ms debounce,
  250 ms between presses. It has no long press: a press is its action on release, or at 3 s if
  still held; a hold never repeats. Two buttons may share an action.
- **The lever's** gate, debounce and rates are unchanged: `Stalk::read` returns a `Command`
  where it returned a `Lever` (removed).
- **`dashsim`**: `BTN S` is `Next`, through one button machine's gate; `BTN L` asks for nothing.
  `vagcan dev recording dash --press` is the same press.
- **BOOT and RESET are not inputs** (owner, 2026-09-27). `GPIO9` is not configured at all: it
  is a strapping pin, and the ROM reads it at reset. Checked before removing it: BOOT's long
  press only logged since BLE became always on (2026-09-13/14); holding BOOT for download mode
  is the ROM's and never involved the firmware. No other image configures `GPIO9`.
- **With no `[stalk]` and no `[[button]]`**, the board shows its active page and changes it
  only from `dashsim` or `dashcfg set page`. Only `dashsim` silences an alarm; otherwise it
  clears when its channel answers in range again — **not on its own**: a channel gone silent
  mid-episode holds it on the glass (`screen.rs`, `a_stale_channel_neither_trips_nor_releases`).
  The plan build notes both (PR #12 review): more than one page and no `[stalk]` or `next` /
  `previous` button; any alarm and no `[stalk]` or `[[button]]`. The owner's own `dash.toml` had
  alarms and no `[stalk]` then, so this image as it stood would have taken his paging away.
  A `[stalk]` with no `[stopwatch]` is noted too: `measure` then only silences an alarm.

**Firmware.** Every producer offers `(Source, Command)` to one `embassy_sync` channel
(`vag_dash_fw::input::CommandQueue`, 4 deep). Nothing waits: a command that finds it full is
dropped and logged (`GPIO3: next dropped — 4 commands are already waiting`) — full means the
consumer is stuck behind a flash write, and a page turn landing seconds late is worse than one
lost. One `control_task` applies them: the settings lock, `active_page`, the log line, the state
push. The input task polls the pins and takes `dashsim`'s presses; the lever keeps being fed in
the bus task, which now only offers its command and never waits on the settings lock. No new
owner of the link; nothing new transmits. `Source` (`GPIO3`, `lever`, `dashsim`) is for the log
line only.

**Config**, `[[button]]` in `dash.toml` (`vag-cli-core/src/dash.rs`):

```toml
[[button]]
pin = 3
action = "next"   # next | previous | stopwatch
```

The pins are the SuperMini's free GPIOs with this board's wiring (`dash/15` §3) — a property of
the board, so they live in code (`control::BUTTON_PINS`; the refusal texts in `pin_taken`). A
button goes from the pin to GND; the pin's internal pull-up holds it high, so it is active low.

| GPIO | held by | a button? |
|---|---|---|
| 3, 4, 5 | nothing (the RS wire, the rail divider, the wake button — all gone) | **yes** |
| 0, 7, 10, 20, 21 | OLED: D/C, SDIN, SCLK, RES, CS | no |
| 1, 6 | CAN transceiver: RX, TX | no |
| 8 | LED, and a strapping pin | no |
| 9 | BOOT, and a strapping pin | no |
| 2 | a strapping pin | no |
| 11 | not broken out on the SuperMini | no |
| 12–17 | SPI flash | no |
| 18, 19 | USB D−, D+ | no |
| anything else | not a GPIO of the ESP32-C3 | no |

Refused at parse, every one tested and mutation-checked: not `[[button]]` tables; more than 3;
no whole-number `pin`; a pin not free (the message says what holds it); a pin twice; no
`action`, or one that is not `next`, `previous` or `stopwatch` (lowercase). A `stopwatch`
button with no `[stopwatch]` is a note, not a refusal.

**Plan.** `plan.json` carries `buttons` (the action by its word; absent when empty, so an old
reader sees the plan it knew); `plan.rs` carries `static BUTTONS: [ButtonPlan; N]`, or
`buttons: &[]` so nothing is imported unused. The golden fixture has one button of each action.
The firmware matches pins 3/4/5 to the typed `GPIO3/4/5` — no stolen peripheral — and makes an
`Input` with a pull-up only for a pin the plan names; an unused pin is not touched.
`Plan::buttons_fit`, a `const fn`, refuses at build time an image whose plan has a pin off the
list, a pin twice or more than three.

**RAM** (`ram-budget.sh`, empty plan as CI builds it): see the PR; with three buttons in the
plan the pin table adds ~300 B of statics.

**Not in this task:** runs kept in flash and read back over BLE are
[`dash/21`](21-runs-in-flash.md), a separate, later task.

## Where it lives

- `vag-dash-render/src/control.rs` — `Command`, `PinButton`, `BUTTON_PINS`, `remote`.
- `vag-dash-render/src/stalk.rs` — the gate and the press detector; `Closer`, the lever closing
  the stopwatch.
- `vag-dash-render/src/stopwatch.rs` — the machine, the launch fit, the page's cells.
- `vag-dash-render/src/screen.rs` — `Screen::command`, `stopwatch()`, `stopwatch_on_glass()`,
  `stopwatch_turns()`, `close_stopwatch()`.
- `vag-dash-render/src/plan.rs` — `StalkPlan`, `StopwatchPlan`, `ButtonPlan`, `Band`,
  `state_of`, `rates_in`, `buttons_fit`.
- `vag-cli-core/src/dash.rs` — `[stalk]` / `[stopwatch]` / `[[button]]` parsed, resolved,
  refused, written.
- `vag-dash-fw/src/input.rs` — the command queue (host test:
  `research/dash/host/tests/input_queue.rs`); `bin/dash.rs` — `input_task` (pins, `dashsim`),
  `control_task` (the one consumer).
- `vag-dash-fw/src/bin/dash.rs` — the lever fed per answer in the bus task, the stopwatch in a
  static, the page drawn, the silence checked and the run kept and stored at a standstill by the
  panel task; `src/schema.rs` — the settings
  record and its versions (host test: `research/dash/host/tests/settings_schema.rs`).
- `vagcan dev recording dash` says it replays neither the lever, the `[[button]]`s nor the
  stopwatch. `dashsim` shows whatever frame the board sends, the stopwatch's included; it has no
  lever key.

## Tests (hardware-free)

- Plan build: the texts resolve to intervals; each refusal above; the notes for a board nothing
  pages or silences and for a `[stalk]` with no `[stopwatch]`; the speed rate it takes is
  `SLOWEST_SPEED_PERIOD_MS`; `plan.json` round trip; the
  board's `state_of` agrees with the laptop's `level_for` value by value, on a ladder whose
  order of trying matters.
- The generated source, compiled in CI: `vag-cli-core/tests/generated_plan.rs` `include!`s
  `tests/fixtures/lever_plan.rs` (a plan with a lever — on a ladder with unbounded states —
  a stopwatch and a button of each action, from the test fixture, not a car) under `deny(warnings)` — CI builds the firmware on an empty plan, so nothing else
  compiles that half of `to_rust`. A unit test fails when the fixture is not what `to_rust`
  writes; `BLESS=1 cargo test -p vag-cli-core generated_source` rewrites it.
- Gate and press: open only with both off; stale either → closed; CANCEL → closed; an edge held
  two reads fires once; one noisy read does not; holding does not repeat; held at start is not a
  press; the gate must be open on both reads; a read from before adapter mode is not half of a
  press.
- The lever closes the stopwatch (`Closer`, mutation-checked): two engaged reads close, either
  witness, CANCEL too; one does not; a read that says nothing breaks the pair; 2.9 s without the
  gate seen open then open does not close, 3.1 s does, 3.0 s does not; a silent unit closes on
  the clock alone; engaged reads that never pair close at 3 s; no `[stalk]` never; after adapter
  mode it starts over. With the screen and the stopwatch as the board runs them: cruise on while
  idle, armed, running (the run dropped) and done (the finished run kept); one noisy read leaves
  a run to finish; 2.9 s of silence leaves it, 3.1 s ends it; a button's stopwatch closes too;
  `close_stopwatch` silences no alarm and turns no page.
- Screen: every command on every screen — the adapter, an alarm up, the stopwatch up, no
  stopwatch in the plan, a page; previous wraps; `Stopwatch` in and out back to the page;
  `Next` and `Previous` ignored during the stopwatch, and a turn of the mode only on
  `Stopwatch`; the adapter ends it.
- Inputs: a pin button's clean press, a bounce, a bouncing press, a hold (one press, no
  repeat), two buttons not gating each other; `dashsim`'s presses; the pins a button may take;
  every `[[button]]` refusal, mutation-checked; the queue keeps order and drops the newest when
  full (host test).
- Stopwatch: arming, start, launch midpoint, marks interpolated, abort, finish, reset, factor 0,
  a poll too slow for a fit, and the launch against a transcription of `derive::start` at 20, 100
  and 250 Hz to a microsecond; a silence aborts a run (with and without an answer after it) and
  disarms a standstill; a mark crossed before the launch has no time; a turn of the mode resets
  once; the page fits the panel in both languages; the phase's cell is inverted armed, and in no
  other phase, drawn.
- Rates: the three lever rates, the cruise status, the speed while up, page cells while timing.
- Settings: a schema-1 byte image loads with every field intact.
- The run's write (`saving.rs`, host test `settings_saving.rs`): the whole configuration, or
  the run added to flash's, or to the defaults when flash holds none; a newer image's record
  never written over; pending until in flash, one try, a new run a new try; `save`, `load`,
  `defaults` and `erase`. The stopwatch (`stopwatch.rs`, mutation-checked): a finished run holds
  the arming until its write, and arms at once after it; a run not yet taken holds it whatever
  the board says; a kept run waiting holds it when the page comes back; with none waiting it
  arms at 1 s as ever; the write's standstill: fresh zero, the hold, never armed, never moving.
- Not host-tested: the firmware's `keep_run` / `store_run` / `write_run` themselves (the run
  written at a standstill) — the firmware does not build for the host. Checked on the board.

## On the car

**2026-09-30, a drive (owner):** paging with the lever works, the stopwatch works, LIMIT opens
it. Items 4–6 below were not reported one by one.

1. `vagcan setup` again, so the cache keeps the bands; then add `[stalk]` / `[stopwatch]` and
   build.
2. Paging: +, −, LIMIT with cruise off; nothing with cruise on or after CANCEL. With the
   stopwatch up, switch cruise on: it closes (`lever: cruise engaged — stopwatch closed`).
3. ~~The factor on a steady stretch.~~ Not needed since 2026-09-27: the speed reads km/h.
4. **A 0–100 run, compared through the board.** The laptop measures the same run with
   `vagcan measure --ble`, from the same gearbox speed. **Start `measure --ble` first, then
   press LIMIT:** once the board's stopwatch arms, its speed holds the board's one timing
   channel, and `measure` over BLE is refused (`the board's timing channel is held by its own
   stopwatch or another client`). **Never with a second adapter (CANable) on the port while the
   board polls** — two testers on one bus breaks the one-owner rule.
5. After the run, stop with the stopwatch up: once the car has stood 1 s the board notes
   `stopwatch: the run is saved`, then `stopwatch: armed` (`GO`) — never `GO` first — and the
   run's times survive a power cycle.
6. **+ and − never open the stopwatch.** With cruise off, press and let go of + and of −, a
   dozen times each, quickly and slowly: no `lever: the stopwatch`. At 20 Hz the rocker's ladder
   passes LIMIT's band (146–181) on its way back to rest (205) from + (91) and from − (128), and
   two reads 50 ms apart inside it would count as LIMIT. The same for +'s release crossing −'s
   band (111–145): no page turned back.
7. **Pin buttons, on the bench first.** A button from GPIO3 (4, 5) to GND, `[[button]]` with
   its `action`, build, flash: each press notes `GPIO3: page N of M`; a hold is one press; with
   the stopwatch up `next` notes that it does nothing; BOOT does nothing at all.
