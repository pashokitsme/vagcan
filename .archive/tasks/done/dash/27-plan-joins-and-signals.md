# dash / 27 — the board indexes what the plan already knows

**Subsystem:** dash · **Crates:** `vag-cli-core` (codegen), `vag-dash-render` (`plan.rs`),
`vag-dash-fw` · **Needs the car:** no

**State (2026-10-19):** done, branch `refactor/dash-plan-joins`. Recorded from the architectural
review of 2026-10-19 (owner approved); every item below is built or answered.

## Joins the board re-derived — now written by the generator

Before: `Channel.unit` was a CAN request id, so the board searched (`Plan::unit_of`,
`lever_unit()`, `PLAN.units.iter().position` on every miss, `channel.unit != request` loops);
`Plan::strip_volts(page)` re-decided "the page shows the channel already" every frame and on
every `follow_pages` (`Page::volts: bool`, `StripPlan.volts`, `strip_volts()`: three things for
one field); `Plan::chart(index)` scanned the pages for every chart frame.

Now (`vag_dash_render::plan`):

- `Channel.unit: u8` is an index into `Plan::units`; `Plan::unit(channel)` indexes,
  `Plan::channels_of(unit_index)` replaces the request loops. `unit_of` is gone.
- `Page::{Chart, Values}.volts: Option<u16>` is the channel the strip reads on that page, `None`
  where there is no `[strip]`, `volts = false`, or the page shows the channel itself. `StripPlan`,
  `Plan::strip` and `strip_volts()` are gone; `Plan::strip_listed` is `find_map(Page::volts)`.
- `Page::Chart.slot: u8` is the chart's history slot; the board and the replay draw a chart from
  its page (`histories[slot]`, the page's `min`/`max`). `Plan::chart(index)` and the board's
  "no chart range" fallback are gone — a chart page always has its range.
- `Page::reads()`: a page's cells and its strip voltage, what the bus lists for it on the board
  and in the replay.
- `Plan::joins_hold()` (`const`) checks every index — a channel's unit, a page's cells, its
  voltage, the slots `0..chart_count()` in page order, the standstill's channel. The firmware
  asserts it at build time (`src/plan.rs`), as it does `buttons_fit()`.

`plan.json` keeps naming a channel's unit by its request id and a page's `volts` as the owner's
`bool`, with `[strip]` beside them — it is for a person. `vag_cli_core::dash::Plan::joins` resolves
the joins once, and both `to_device()` (the replay) and `to_rust()` (the board) write them, so
the two cannot index different things. **Compatibility:** a `plan.json` whose channel names a
unit its `units` do not list is refused with "build it again" (a generated file gone wrong, never
guessed at); every other existing `plan.json` reads unchanged. The plan id moved for every plan
(the source spells the joins): the fixture `lever_plan.rs` is re-blessed (`0x1a93b72a285c90a2`)
and the empty plan's pinned id is `edfbdbaf3754546a`. A board reflashed with an unchanged
`dash.toml` says once that its settings were saved against another plan, and keeps them.

## Three ways to say "the pages on screen changed" — one now

Before: the `PAGES_CHANGED` signal, the `GLASS_PAGE` atomic and a 1 s `PAGE_RECHECK` poll that
rebuilt two `Vec`s and compared.

Kept: **the signal**, the only mechanism. Every change already raised it but one — a host's timing
subscription coming or going (the radio taking the free timing channel, a host's session closing,
a host unsubscribing), which only the poll caught, up to a second late. Those now raise it too:
each session step compares the planner's timing-subscription count before and after
(`host_timing_moved`), a session close raises it, and a cable `Subscribe` still raises it for the
preemption of the panel's own run (the count does not move there). `panel_bus` raises it once on
entry, at boot and back from the adapter. The idle wait sleeps until the next retry, close, fault
count step or due read — or a signal. The poll alone was the other choice: it costs up to a
second of dashes after every page turn, which is what the signal was added to avoid.

`GLASS_PAGE` was the data, not a third signal: what the glass shows. It is now `ON_GLASS`, a
`Cell<OnGlass>` behind a blocking mutex.

## Smaller, from the same review

- **`NO_PAGE`/`STOPWATCH_PAGE` sentinels → `enum OnGlass { Cursor, Page(u8), Stopwatch }`**
  (`bin/dash.rs`). `Cursor` is before the first frame: the cursor's page, about to be drawn.
- **The empty-plan fallback page in `config.rs`** — already gone: PR #27 (`210048d`) took the
  pages out of the settings record, and `Config::default` invents no page.
- **`max_dids_per_request: 8`** — `Budget::default` says at the line that it is the reference
  car's measurement (eight answered, twelve refused), not a protocol limit: ISO 14229-1 leaves the
  count to the server, and a unit that takes fewer is learned as single-only.
- **`MAX_HZ = 100` vs `MIN_PERIOD_MS = 20`** — one cap: `dash::MAX_HZ` is now
  `1000 / guard::MIN_PERIOD_MS` = 50 Hz, the floor the board's guard holds every host to. The BLE
  guard is unchanged. 50 is also `watch`'s ceiling (`config::MAX_HZ`) and what `measure` reads road
  speed at; the reference car's file asks 20 at most. A `dash.toml` with an `hz` above 50 is
  refused with the existing message, now "at most 50".
- **An alarm on a chart-only channel** — built: small and clean. The generator raises the first
  values page with the channel as a cell, else its chart page (the alarm machine already took any
  page that `shows` the channel); the renderer inverts a chart's header line when its cell is
  alarmed, the dark row under it and the trace untouched. A channel with an alarm on no page at all
  is refused ("needs a page that shows the channel").

## Checks

- Replay-vs-board parity (`dashreplay/engine.rs`): the replay's chart page with an alarm out
  draws, half for half, what the board's panel task composes for it; the zero-at-rest cell too
  (`dash/26`).
- `render.rs`: an alarmed chart inverts its header line and nothing under it.
- `plan.rs`: `joins_hold` refuses each broken index; `Page::reads`.
- The owner's plan, built offline from a copy of `~/.vagcan` (2026-10-19): the same 23 channels on
  4 units, 6 pages and 12 alarms as `master`'s `plan.json`, field for field; only `id` and the new
  `standstill` differ. The firmware's clippy on it is clean, `joins_hold` included.
- Firmware static RAM unchanged: 140,976 B with BLE, 131,516 B without (before and after).
