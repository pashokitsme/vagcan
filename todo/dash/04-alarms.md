# dash / 04 — alarms: retard and misfires take the screen

**Subsystem:** dash · **Crate:** `vag-dash-render` · **Needs the car:** partly (thresholds)

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
the display into a single frozen screen for the rest of the drive. A short press silences
the current episode; a *new* crossing after the release arms it again.

**One button, so the short press is modal** (settled with the owner 2026-08-25, and it
supersedes what this file said about a third button). The device has a single button
because configuration moved to BLE:

| gesture | normally | while an alarm is showing |
|---|---|---|
| short press | next page | silence this episode |
| held 3 s | start advertising for configuration | same |

The silence is bounded by evidence rather than by a timer: it lasts until the value
releases, and the rule is armed again by the next crossing after that. A rule that has
been silenced is therefore **still polled** — nobody can see the release that re-arms it
otherwise. One press ends one episode, so a second rule that is also out takes the screen
on the next poll and asks for its own press.

**Highlight by inverting the offending cell, not the screen.** Filling the whole panel loses
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

## Where it lives — `crates/dash/vag-dash-render/src/alarm.rs` (done)

The machine is in **`vag-dash-render`**, not in the firmware, and that is a testability
decision: `crates/dash/vag-dash-fw` is not a workspace member and cannot be built for the host,
so anything living there is untested by CI. The module is `no_std`, allocation-free and
does not touch the drawing code — it decides *which* page is on the glass, and it is the
only thing that ever sets `Cell::alarm`.

```rust
let mut alarms = Alarms::new([
  Alarm::below(&RETARD, RETARD_PAGE, -2.0, -1.5),
  Alarm::above(&MISFIRE, MISFIRE_PAGE, trip, release),
]);

// Every frame: what the caller would show, what the car said, and the time.
let Update { shown, changed } = alarms.poll(current_page, &readings, now_ms);
// `shown.page` to render; `shown.offending` is the ChannelId to draw inverted.

// The one button:
if alarms.press() == Press::NextPage { current_page = plan.next(current_page); }
```

Four decisions worth writing down:

- **Nothing here reads a clock** — `now_ms` is a parameter, because the callers are
  `embassy_time` on the board and `std::time` on the laptop, and because a synthetic
  clock is the only way the 2.5 s hold is ever exercised.
- **"Where you were" is not remembered here.** The caller passes the page it *would* be
  showing on every poll and that is what comes back. A second copy of the caller's own
  cursor is a second copy that can drift.
- **Pages and channels are identities** (`PageId`, `ChannelId`), not positions. The
  polled set is a union whose order changes with the page, so a position is not a name.
- **Rules are in priority order**, and two firing in the same poll resolve by that order
  rather than by whichever the loop saw first.

## Tests

Pure logic, no display: drive a synthetic series through the state machine. Fifteen tests
in `alarm.rs`, all green:

- A value oscillating across the trip point produces **one** takeover, not many.
- Release at the release threshold, not the trip threshold.
- The view stays up 2.5 s after the value returns inside, and hands back to the page that
  was showing before — asserted by page identity, not by index.
- Silencing ends the episode; a fresh crossing after a release re-arms; a silenced rule
  stays silent while the value is still out.
- Two alarms crossing in the same poll resolve by priority; silencing the showing one
  lets the one behind it through.
- The inverted cell is the *worst* channel, it follows the engine mid-episode, and it
  freezes on the last offender through the hold.
- A channel that stops answering neither trips nor releases — the button is the way out.

Still owed by the caller, not by this module:

- The polled set is the union of page and armed alarms, and splits at the plan's batch
  limit. `Alarms::watched()` supplies the alarms' half; the union and the split belong to
  the plan.
- Inversion covers exactly the offending cell's rectangle — already covered in
  `render.rs` by `an_alarm_inverts_its_own_column_and_leaves_the_others_alone`.

## Done when

The retard alarm can be demonstrated in the simulator (`03`) from a recorded drive, and
the flicker test passes on a series built to sit exactly on the threshold.

The flicker test passes. What is left is wiring: the firmware feeding `Alarms::poll` from
the plan's readings and routing its one button through `Alarms::press`, and the misfire
rule's two numbers, which are a bench measurement and not a guess.
