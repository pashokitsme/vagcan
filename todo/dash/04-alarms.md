# dash / 04 — alarms: retard and misfires take the screen

**Subsystem:** dash · **Crates:** `vag-dash-render`, `vag-cli-core` (plan), `vag-dash-fw` ·
**Needs the car:** partly (thresholds)

**State (2026-09-22):** on `master` — threshold rules since PR #2 (2026-09-14), the drift rule
(`18`) since PR #4 (2026-09-15); the four recommended rules below are in the owner's `dash.toml`
since 2026-09-22, and on the bench the board polls their channels on every page. Rules are
`[[alarm]]` tables in `dash.toml`, checked at plan build and carried into `plan.json` /
`plan.rs`; the board reads their channels at full rate on every page, takes the screen,
inverts the offending cell and silences on a short press. **2026-09-26:** the hardware-free
replay exists — `vagcan dev recording dash <VIN> --log FILE.csv [--press S]…` runs a
`watch --out` recording through the board's `Screen`, alarms, `Plan::rates` and renderer and
logs every takeover, hand-back and silence. Since the same day the offending cell blinks
while the value is out and holds still through the hold (owner). Open: a run on the car, where the misfire window
and every threshold are checked, and a recording with the retard channels to replay (see
"Done when").

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
per 1000 revolutions is not a quantity anyone should guess a threshold for).

**In the owner's `dash.toml` since 2026-09-22**, with values researched on that day (the
research note is not in the repository; its sources are summarised here). None is VW's own
number for this ECU, and the car confirms each:

| rule | channels | values | resting on |
|---|---|---|---|
| misfires, `above` | `291D`–`2920` (a count per 1000 revolutions, ×1) | trip 5, release 3 | VW's 0…2 per cylinder in the `06J-906-026-CCT` label (another engine unit); CARB 13 CCR 1968.2's 1 % = 20 per 1000 revolutions; if too eager, 20 / 10 |
| knock retard, `below` | `200A`–`200D` (s16 ×0.01 °, retard negative) | trip −2.0, release −1.5 | about one knock step (1.5–2.25° on the sibling `8V0906264L`, community data); if too eager, −3.0 / −2.0 |
| coolant, `above` | `F405` | trip 115, release 110 | the top of VW's 80…115 °C warm spec (EA888 gen1/2 labels); release inferred |
| boost drift | `202A` against `2029` | 10 %, release 5 %, 2000 ms, floor 1.3 bar | inference only; the floor matters because the pressures are absolute (~0.99 bar at rest) |

Unknown until the car: whether the misfire window rolls or latches and what it reads at idle,
the retard's sign and how often −2.0 comes on the owner's fuel, the boost lag on a tip-in.

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
it costs nothing in burn-in. Blinking the whole screen is refused for the same reason: on
the plain half the label is gone too.

**The inversion blinks while the value is out, and holds still in the hold** (owner,
2026-09-26: a steadily inverted cell did not draw the eye enough, "not very intuitive"):

| episode | the offending cell |
|---|---|
| firing (`Firing`) | inverted 400 ms, plain 400 ms, by turns (`alarm::BLINK_MS`), inverted first |
| back inside, the 2.5 s hold (`Holding`) | inverted on every frame — the driver sees it came back |
| handed back, silenced, or a drift rule still counting | nothing inverted |

**Firing includes two cases that are not "past the trip", and both blink on purpose**
(controller, 2026-09-26): a value back inside the hysteresis band but not past the release,
and channels that stop answering mid-episode. Neither is a release, so the alarm has not
released and the cell keeps blinking. Not a bug.

**The phase counts from the takeover** (review, 2026-09-26): inverted when
`((now_ms − from) / BLINK_MS) % 2 == 0`, so the first frame of every episode is inverted. A
phase from the boot clock left the glass unchanged for up to ~600 ms when the driver was
already on the rule's page and the takeover landed on a plain half. `from` lives in the
`Firing` episode, not in a timer: the moment it fired, fired again out of the hold, or took
the glass from a rule that was silenced or handed back. The board and the replay use the same
rule. The frames must be at most `BLINK_MS / 2` apart so every half holds one even when a
frame is slow to draw; the board's 200 ms gives two on, two off, and both `bin/dash.rs` and the
replay's `engine.rs` assert it at compile time. At a frame period of an even multiple of
400 ms the frames would alias onto one half and the blink would be lost. `dashsim` only shows
the frames the board sends, so it blinks as they do.

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

- **`vag-dash-render/src/alarm.rs`** — the state machine (33 tests, 12 of them the drift
  rule's). `no_std`, allocation-free, reads no clock. `Alarm` is plain data the plan carries
  as a `static`. `BLINK_MS` and the blink decision live here: an episode that is up says its
  `Highlight` — `Blinking { from_ms }` while `Firing`, `Steady` while `Holding`, read off the
  episode state rather than kept beside it — and `Shown::inverted(now_ms)` answers the frame.
- **`vag-dash-render/src/screen.rs`** — `Screen`: the page cursor, the alarms and the short
  press, which the firmware only feeds (15 tests). `frame` answers a `Glass`: the page to
  draw, the channel the alarm points at (`offending`, what the log names), its `highlight`,
  the cell inverted **on this frame** (`inverted`), whether the page changed, a page the
  board does not hold, and what to say (`Took { rule }`, `Over`, `Silenced`). The cursor stays the board's
  (`Config::active_page`: saved, set over BLE, reported by `state`); `Screen` reads and
  moves it and never keeps a copy.
- **`vag-dash-render/src/plan.rs`** — `Plan::alarms`; `ChannelId` is the plan's channel
  index and `PageId` its page index, because an image is built for one plan. `Plan::rates`
  keeps watched channels foreground.
- **`vag-cli-core/src/dash.rs`** — `[[alarm]]` parsed, checked, written to `plan.json` and
  `plan.rs`.
- **`vag-dash-fw/src/bin/dash.rs`** — every panel frame calls `Screen::frame` with the
  cursor and the value store (`None` when stale, the existing `STALE` rule), draws
  `glass.page`, and inverts the cell whose channel is `glass.inverted`; it publishes the
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

`alarm.rs` (33, neutral channels and thresholds; 12 are the drift rule's): one takeover for a
value oscillating across the trip; release at the
release value; the 2.5 s hold and the hand-back by page identity; silence, re-arm after a
release, silence while still out; priority; the worst cell and its freeze through the
hold; a channel that stops answering neither trips nor releases. The highlight (2026-09-26):
while firing the cell blinks in `BLINK_MS` halves from the takeover, to the millisecond, and
the first frame is inverted at any takeover time; it keeps blinking in the hysteresis band and
with its channels quiet; through the hold it is inverted on every frame, and the release is
not a `changed` picture; nothing after the hand-back, while silenced, or while a drift rule
is still counting, whose blink starts when it fires; out again inside the hold blinks again
from that moment.

`screen.rs` (15, neutral channels and thresholds): a hidden page's channel takes the screen
with its page and cell, and the takeover and hand-back change the page; during a takeover
the foreground channels are the alarm page's (through `Plan::rates`); an alarm page past
the board's pages draws the cursor page; silence then re-arm after a release, said as
`Silenced`; the cursor moves only on `NextPage`, and the hold hands back to where the
cursor is *now*; two rules by priority, each a `Took` of its own; a stale channel neither
trips nor releases; the adapter screen runs no alarms and a press there pages; a plan with
no alarms only pages. The blink through `Glass::inverted`: two frames on, two off at 200 ms,
steady through the hold, none after; a silenced alarm inverts nothing; a second rule taking
over blinks its own cell, inverted on its first frame though it fired earlier, behind the first
rule; a takeover of the page already on the glass is inverted on its first frame; frames at
any period up to `BLINK_MS` and any start, or jittered, see both halves and never draw one
picture longer than a half and a frame; and the plain half of a blink draws the page pixel
for pixel as with nothing inverted.

`plan.rs`: a watched channel is foreground at its own rate on every page. `dash.rs`: every
refusal above, the `plan.json` round trip, an old `plan.json` without alarms, and the
generated `static`.

Inversion covers exactly the offending cell's rectangle — `render.rs`,
`an_alarm_inverts_its_own_column_and_leaves_the_others_alone`.

## Done when

The retard alarm can be demonstrated in the simulator (`03`) from a recorded drive, and
the flicker test passes on a series built to sit exactly on the threshold.

The flicker test passes, and the wiring is done. The host replay exists (2026-09-26):

```
vagcan dev recording dash <VIN> --log drive.csv            # the panel in the terminal
vagcan dev recording dash <VIN> --log drive.csv --press 12.5 | cat   # the event log only
```

It resolves the plan as `dev dash build` does (nothing written), matches recording columns to
plan channels by unit, identifier and field (a heading two channels of the car share is
refused, not guessed), and runs `Screen`, the alarms and `Plan::rates` on the recording's own
clock, with the firmware's frame period (200 ms) and staleness rule (5 s, or three periods)
mirrored in `vag-cli-diag/src/dashreplay/engine.rs`. A plan channel the recording does not
hold is `None`: it neither trips nor releases. Its tests (neutral channels): one takeover for
a value hovering on the trip, the hand-back 2.5 s after the release by recording time,
silence and re-arm, a missing channel never tripping, columns matched in another order, the
piped output being the log alone, and the panel being the board's frame with the offending
cell blinking — inverted on one half, the plain page on the other — and steady through the
hold (2026-09-26).

Since 2026-09-26 `watch --out` quotes a heading with a comma ("Ignition retard, cylinder 1"
split in two before), writes a read that missed as its time in `_t_s` with the value empty, and
marks an answer it could not convert `0x…` in a converted column. The replay reads a miss only
there: an empty cell with no time of its own is no evidence (recordings without `_t_s` columns
leave whole rows empty between sweeps — `research/dumps/drive-gear.csv`). A recording made
before cannot show a miss (the replay keeps its last value `fresh_for` from when it was heard)
and wrote an unconverted answer as bare hex, which cannot be told from a number. Owner's call
(2026-09-26, after four review rounds on salvaging it): no salvage. A converted column holding
any cell that is not a number on the plan's scaling, a `0x…` cell or empty is dropped whole,
with a note saying it is another scaling or an old recording's bare hex. Columns are matched
by name against the plan's survey; a unit `watch` identified live that the survey lacks could
share a name, and the replay says it matched by name.

Left: record a drive with the retard channels (`watch --out` with `200A`–`200D` selected, on
the car, with this build) and replay it; the misfire rule's two numbers, from the car. The recorded drives in
`research/dumps/` are gearbox channels; none holds `200A`–`200D`.
