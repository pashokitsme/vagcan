# dash / 04 — alarms: retard and misfires take the screen

**Subsystem:** dash · **Crates:** `vag-dash-render`, `vag-cli-core` (plan), `vag-dash-fw` ·
**Needs the car:** partly (thresholds)

**State (2026-09-14):** wired on `ble-uds`, hardware-free tests only. Rules are
`[[alarm]]` tables in `dash.toml`, checked at plan build and carried into `plan.json` /
`plan.rs`; the board reads their channels at full rate on every page, takes the screen,
inverts the offending cell and silences on a short press. Open: the misfire rule's
numbers (a car measurement), a run on the car, and the demo from a recorded drive (no
hardware-free replay exists — see "Done when").

## Goal

A rule that watches a channel which is **not on the screen**, and when it crosses a
threshold, replaces whatever is showing with the view that explains why.

Agreed with the owner 2026-08-20. The reference device does this with a red LED and a
buzzer past −2°; we do it with the screen, which carries more.

## The rule — `[[alarm]]` in `dash.toml`

Rules are the owner's data, never code (controller, 2026-09-14):

```toml
[[alarm]]
channels = ["01:IDE…", "01:IDE…"]   # refs that appear under [[channel]]
page = "RETARD"                      # title of a values page showing all of them
direction = "below"                  # or "above"
trip = -2.0                          # fires at or beyond
release = -1.5                       # clears only past this, back the other way
```

Order in the file is priority. At most **4** rules (`alarm::MAX_ALARMS`): each rule's
channels are read at full rate on every page, and one press ends one episode.

Two rules to start: **ignition retard** (`200A`–`200D`, trip −2.0°, release −1.5°,
`below`) and **misfires** (`291D`–`2920`, trip and release to be set on the car — a count
per 1000 revolutions is not a quantity anyone should guess a threshold for). Neither is
in the owner's `dash.toml` yet; the owner writes them.

`vagcan dev dash build` refuses, with the rule's number (`alarm #n: …`):

| what | message |
|---|---|
| a channel not under `[[channel]]` | `01:X is not in the [[channel]] list` |
| no channels | `watches no channels` |
| no values page with that title | `no values page is titled "X" — a chart page has no title and cannot explain an alarm` |
| two values pages with that title | `N values pages are titled "X" — give them different titles` |
| the page misses a watched channel | `page "X" does not show 01:Y — the page an alarm raises shows every channel it watches` |
| `release` on the wrong side | `release R is not above trip T — a "below" alarm releases above where it trips` (and the `above` mirror) |
| more than 4 rules | `N [[alarm]] rules, and the board holds at most 4 — …` |
| more than 8 pages (`pages::MAX_PAGES`, the board's) | `N [[page]] tables, and the board holds at most 8` |

`trip` and `release` are compared in `f32`, as the board compares them: `trip = 100.000001`
with `release = 100` is one value there, and refused.

Shape errors (`direction` not `below`/`above`, a missing or non-finite `trip`/`release`,
a missing `page`) are refused when `dash.toml` is parsed. A `plan.json` from before alarms
loads with no rules.

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
supersedes what this file said about a third button). BLE is always on (2026-09-13/14), so
the long press no longer has a job:

| gesture | normally | while an alarm is showing |
|---|---|---|
| short press (BOOT, `dashsim` `BTN S`) | next page | silence this episode |
| held 3 s | nothing | nothing |

`dashcfg`'s `set page` is **not a press**: it moves the page cursor and leaves the alarm
up; when the alarm hands back, it hands back to the page that was set. On the adapter
screen (`--slcan`) no alarm runs and a short press turns the page as always.

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
**union** of the current page's channels and every rule's: four on screen, four retard,
four misfire — twelve, where a page alone would be four.

`Plan::rates` makes that union: a channel any rule watches is `foreground` at its own
`hz` on every page, never the hidden-page 1 Hz, whether or not its rule is silenced. The
board's panel subscribes by it, so a page switch moves the page's channels and leaves
the alarms' alone. **The page read in the foreground is the page on the glass**, not the
cursor's: during a takeover the alarm page's other cells run at their rates and the
cursor page, not drawn, drops to 1 Hz. The panel publishes the drawn page and signals the
bus on `Glass::page_changed` — a takeover and a hand-back as much as a page turn. The planner packs due identifiers of one unit into one `0x22` request
and learns a unit that refuses that (`todo/dash/14` §2). How many identifiers this ECU
accepts per request is still a bench measurement — see `06`.

## Where it lives

- **`vag-dash-render/src/alarm.rs`** — the state machine (15 tests). `no_std`,
  allocation-free, reads no clock. `Alarm` is plain data the plan carries as a `static`.
- **`vag-dash-render/src/screen.rs`** — `Screen`: the page cursor, the alarms and the short
  press, which the firmware only feeds (9 tests). `frame` answers a `Glass`: the page to
  draw, the cell to invert, whether the page changed, a page the board does not hold, and
  what to say (`Took { rule }`, `Over`, `Silenced`). The cursor stays the board's
  (`Config::active_page`: saved, set over BLE, reported by `state`); `Screen` reads and
  moves it and never keeps a copy.
- **`vag-dash-render/src/plan.rs`** — `Plan::alarms`; `ChannelId` is the plan's channel
  index and `PageId` its page index, because an image is built for one plan. `Plan::rates`
  keeps watched channels foreground.
- **`vag-cli-core/src/dash.rs`** — `[[alarm]]` parsed, checked, written to `plan.json` and
  `plan.rs`.
- **`vag-dash-fw/src/bin/dash.rs`** — every panel frame calls `Screen::frame` with the
  cursor and the value store (`None` when stale, the existing `STALE` rule), draws
  `glass.page`, and inverts the cell whose channel is `glass.offending`; it publishes the
  drawn page for the bus task's subscriptions. The button task routes a short press
  through `Screen::press`. On USB: each takeover (per rule), `over`, `silenced`, and once
  an alarm page the board does not hold — the cursor's page is drawn instead of freezing.

```rust
let mut screen = Screen::<ALARM_COUNT>::new(PLAN.alarms);
// Every frame: the page the driver chose, the clock, the store.
let glass = screen.frame(config.active_page, pages, now_ms, |i| value_of(i));
// The one button:
if screen.press(&mut config.active_page, pages) == Press::NextPage { /* paged */ }
```

Four decisions worth writing down:

- **Nothing here reads a clock** — `now_ms` is a parameter, because the callers are
  `embassy_time` on the board and `std::time` on the laptop, and because a synthetic
  clock is the only way the 2.5 s hold is ever exercised.
- **"Where you were" is not remembered here.** The caller passes the page it *would* be
  showing on every poll and that is what comes back. A second copy of the caller's own
  cursor is a second copy that can drift.
- **Pages and channels are identities** (`PageId`, `ChannelId`), not positions in the
  polled set, whose order changes with the page.
- **Rules are in priority order**, and two firing in the same poll resolve by that order
  rather than by whichever the loop saw first.

## Tests

`alarm.rs` (15, neutral channels and thresholds): one takeover for a value oscillating across the trip; release at the
release value; the 2.5 s hold and the hand-back by page identity; silence, re-arm after a
release, silence while still out; priority; the worst cell and its freeze through the
hold; a channel that stops answering neither trips nor releases.

`screen.rs` (9, neutral channels and thresholds): a hidden page's channel takes the screen
with its page and cell, and the takeover and hand-back change the page; during a takeover
the foreground channels are the alarm page's (through `Plan::rates`); an alarm page past
the board's pages draws the cursor page; silence then re-arm after a release, said as
`Silenced`; the cursor moves only on `NextPage`, and the hold hands back to where the
cursor is *now*; two rules by priority, each a `Took` of its own; a stale channel neither
trips nor releases; the adapter screen runs no alarms and a press there pages; a plan with
no alarms only pages.

`plan.rs`: a watched channel is foreground at its own rate on every page. `dash.rs`: every
refusal above, the `plan.json` round trip, an old `plan.json` without alarms, and the
generated `static`.

Inversion covers exactly the offending cell's rectangle — `render.rs`,
`an_alarm_inverts_its_own_column_and_leaves_the_others_alone`.

## Done when

The retard alarm can be demonstrated in the simulator (`03`) from a recorded drive, and
the flicker test passes on a series built to sit exactly on the threshold.

The flicker test passes, and the wiring is done. The demo cannot be run without hardware
today (checked 2026-09-14):

- `dashsim` shows the board's own frames — it needs the board, and the bench CAN pair is
  dead (`research/dash/can-bring-up.md` §9.7).
- `vagcan watch` replays a `watch --out` recording through the *terminal* view, not the
  dash renderer; `vag-dash-render/examples/panel.rs` renders fixed stand-in frames.
- The recorded drives (`research/dumps/drive-gear.csv`, `drive-gearbox.csv`) are gearbox
  channels; none holds `200A`–`200D`.

Left: record a drive with the retard channels, then either feed it to a host renderer or
show it on the board through `dashsim`; the misfire rule's two numbers, from the car.
