# dash / 27 — the board indexes what the plan already knows

**Subsystem:** dash · **Crates:** `vag-cli-core` (codegen), `vag-dash-render` (`plan.rs`),
`vag-dash-fw` · **Needs the car:** no

**State (2026-10-19):** recorded, not started. Architectural review of 2026-10-19; owner approved.
Smaller items from the same review are listed at the end.

## Joins the board re-derives

- `Channel.unit` is a CAN request id, not a unit index, so the board searches: `Plan::unit_of`,
  `lever_unit()`, `PLAN.units.iter().position` on every miss, `channel.unit != request` loops.
- `Plan::strip_volts(page)` re-decides "the page shows the channel already" every frame and on
  every `follow_pages`; `Page::volts: bool`, `StripPlan.volts` and `strip_volts()` are three
  things for one field.
- `Plan::chart(index)` scans the pages for every chart frame.

The generator emits the joins resolved: `unit: u8` as an index, `Page.volts: Option<u16>`,
the chart's slot. The board indexes and never searches.

## Three ways to say "the pages on screen changed"

`dash.rs`: the `PAGES_CHANGED` signal, the `GLASS_PAGE` atomic and a 1 s `PAGE_RECHECK` poll
that rebuilds two `Vec`s and compares. Keep one: the signal (every change already raises it),
or the poll alone.

## Smaller, from the same review

- `NO_PAGE = 255` and `STOPWATCH_PAGE = 254` live in the page-index `u8`; an enum says what
  the screen shows.
- The empty-plan fallback in `config.rs` invents a values page of the first four channels —
  a page the generator never wrote (only the CI image reaches it).
- `max_dids_per_request: 8` is the reference car's measurement ("eight answered, twelve
  refused") shipped as the default; say so at the point of use.
- `MAX_HZ = 100` in the plan build and `MIN_PERIOD_MS = 20` (50 Hz) in the BLE guard: two caps
  on one bus.
- A channel shown only on a chart page cannot carry an alarm; the chart page could be raised
  with its header inverted.
