# dash / 19 — the cruise lever as buttons, and the stopwatch page on the board

**Subsystem:** dash · **Crates:** `vag-cli-core` (plan), `vag-dash-render` (screen, stopwatch),
`vag-dash-fw` · **Needs the car:** partly (the speed factor, the LIMIT level, a run)

**State (2026-09-26):** a spec, waiting for the owner's approval. Starts after `feat/alarm-blink`
and `feat/enum-ranges` merge: it touches the same panel loop and screen as the blink, and it
recognises the lever by the ODIS intervals that `enum-ranges` keeps.

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
  `4383` bit 3), 128 SET/− ("verzögern", bit 2). **167 ("neutral ohne Limiterverbau") appeared
  twice with the switch OFF — taken to be LIMIT (the owner pressed it), to be confirmed** with
  `watch --did 70C:1105` once the intervals decode.
- Switch: 167 OFF (latched), 91 ON, 128 CANCEL (springs back to ON).
- With the switch OFF, the engine's `203C` reads 0 ("main switch off") and its `4383` stays
  `0x2000` through every rocker press: the engine ignores the lever. With ON it reads 2.

## Configuration — data, never code

Which unit, identifier, field and state a button is lives in the owner's `dash.toml`, by the
ODIS texts, resolved at plan build into intervals the board carries (`plan.rs`). Another car
names its own fields; nothing about `70C` or `1105` is in the code.

```toml
[stalk]
read = "70C:1105"                                  # one identifier, fields by the plan's layout
rocker = "Linker Hebel axial 2 (GRA Set/Rest)"     # field names as the ODIS project spells them
switch = "Linker Hebel axial 3 (ON/CANCEL/OFF)"
next = "Blinker GRA beschleunigen"                 # states of the rocker
previous = "Blinker GRA verzögern"
measure = "Blinker GRA neutral ohne Limiterverbau" # LIMIT — confirm on the car
switch_off = "Blinker GRA Aus"                     # state of the switch
cruise = "01:203C"                                 # the engine's cruise status
cruise_off = "main switch off"                     # its state that means off

[stopwatch]
speed = "02:380B"            # gearbox output shaft speed, a [[channel]]
km_h_per_unit = 0.0          # measured on the car (below); 0 = not measured, the page says so
marks = [60, 100]            # km/h
```

`dev dash build` refuses, by name: a field or state text the ODIS project does not have for that
identifier; `cruise` not an enum with that state; `speed` not a `[[channel]]`.

## Behaviour

**The gate.** The lever is ours only when **both** say off: `cruise` reads `cruise_off` and
`switch` reads `switch_off`. Stale or absent counts as **on** — the safe side: a lost read never
turns a cruise press into a page turn. Never CANCEL (`dash/14` §6a: plus after CANCEL resumes).

**A press.** An edge from any other rocker state into `next`/`previous`/`measure`, held for **two
consecutive reads** (debounce: one noisy read never fires), fires once; holding does not repeat.

**Rates.** `1105` at 20 Hz with the gate open, 2 Hz with it closed (enough to see it open), 5 Hz
while the stopwatch is armed or running (LIMIT must still end it; presses last 0.3–0.6 s).
`cruise` at 5 Hz. All through `Plan::rates` and the one scheduler — no second owner of the bus.

**Alarms first.** While an alarm holds the screen, a lever press does what BOOT's short press
does there: silence the episode. Alarms take the screen during the stopwatch too.

**The stopwatch page** (`dash/14` §6, unchanged except for entry and exit): LIMIT enters it
from any page and leaves it back to the page it came from. Speed 0 for 1 s arms it; the first
sample above 0 starts the clock (the half-sample correction `vag-cli-measure` uses); crossing
each mark stamps a time interpolated between the samples either side. The times are drawn
large; the last run's times are kept in settings. While armed or running, `speed` gets the
high-priority slot and the other cells drop to background rate. With `km_h_per_unit = 0` the page
says the factor is not measured and does nothing else.

**The factor.** Measured, never written in (`dash/14` §6): on a steady stretch, `vagcan watch
--did 02:380B 01:F40D --out steady.csv`, then `vagcan dev recording calibrate` fits km/h
against `380B`; the fit goes into `km_h_per_unit`. Check first that `calibrate` can take a
recording with those two columns; if not, extend it (a separate commit).

## Tests (hardware-free)

- Plan build: the texts resolve to intervals; each refusal above.
- Gate: open only with both off; stale either → closed; CANCEL → closed.
- Press: an edge held two reads fires once; one noisy read does not; holding does not repeat;
  previous page wraps.
- Alarm up: a lever press silences, does not page.
- Stopwatch: arming, start, marks interpolated, exit back to the page it came from, factor 0.
- Rates: the three stalk rates and the stopwatch's high slot through `Plan::rates`.
- The replay (`vagcan dev recording dash`) and `dashsim` understand the new page, or say
  they don't.

## On the car

1. `watch --did 70C:1105` with the intervals decoded: confirm LIMIT's state text.
2. Paging: +, −, LIMIT with cruise off; nothing with cruise on or after CANCEL.
3. The factor on a steady stretch; then a 0–100 run beside `vagcan measure` on the laptop.
