# dash / 04 — alarms: retard and misfires take the screen

**Subsystem:** dash · **Crate:** `vag-dash` · **Needs the car:** partly (thresholds)

## Goal

A rule that watches a channel which is **not on the screen**, and when it crosses a
threshold, replaces whatever is showing with the view that explains why.

Agreed with the owner 2026-08-20. The reference device does this with a red LED and a
buzzer past −2°; we do it with the screen, which carries more.

## The rule

```
Alarm {
  channels: [Channel],      // e.g. the four retard channels
  page:     PageRef,        // the values page to raise
  trip:     f32,            // fire at or beyond
  release:  f32,            // clear only past this, back the other way
  direction: Below | Above,
}
```

Two rules to start: **ignition retard** (`200A`–`200D`, trip −2.0°, release −1.5°,
`Below`) and **misfires** (`291D`–`2920`, trip and release to be set on the car — a count
per 1000 revolutions is not a quantity anyone should guess a threshold for).

## The behaviour, which is the hard part

**It must not flicker.** A retard hovering at −2 would swap the screen ten times a second
and be useless. Three mechanisms, all required:

1. **Hysteresis** — fire at −2.0, release at −1.5. One threshold is not enough.
2. **Hold after release: 2.5 s** (the owner's number, 2026-08-20). The alarm view stays
   up for 2.5 seconds after the value comes back inside, then hands the screen back.
3. **Return to where you were** — the page that was showing before, not page one.

**It must not trap.** If the engine is genuinely misfiring, an un-dismissable alarm turns
the display into a single frozen screen for the rest of the drive. The third button
silences the current episode for a while; a *new* crossing after the release arms it
again.

**Highlight by inverting the offending cell, not the screen.** Filling all 256×32 loses
the one thing the view exists to say — *which cylinder*. Inverted, the cell reads black
on white and the label survives along with the number. It is brief by construction, so
it costs nothing in burn-in.

## The polling consequence — the non-obvious cost

An alarm has to watch channels that are not being displayed. So the polled set is the
**union** of the current page's channels and every armed alarm's: four on screen, four
retard, four misfire — twelve, where a page alone would be four.

What saves the frame rate is that `0x22` takes several identifiers in one request, and
the survey records this car answering batched reads (`"batched": true` in
`research/dumps/survey-parked.jsonl`). Twelve channels on the engine is then one exchange,
not twelve. **How many identifiers this ECU accepts per request is a bench measurement**
and it is the first number wanted from hardware — see `06`. Until it is known, the plan
must not assume batching: build the request set from the union and split it by a limit
the plan carries.

## Tests

Pure logic, no display: drive a synthetic series through the state machine.

- A value oscillating across the trip point produces **one** takeover, not many.
- Release at the release threshold, not the trip threshold.
- The view stays up 2.5 s after the value returns inside, and hands back to the page that
  was showing before — asserted by page identity, not by index.
- Silencing ends the episode; a fresh crossing after a release re-arms.
- The polled set is the union of page and armed alarms, and splits at the plan's batch
  limit.
- Inversion covers exactly the offending cell's rectangle (screenshot test).

## Done when

The retard alarm can be demonstrated in the simulator (`03`) from a recorded drive, and
the flicker test passes on a series built to sit exactly on the threshold.
