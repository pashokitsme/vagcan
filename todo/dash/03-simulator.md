# dash / 03 — `vagcan dash`, the panel on the desktop

**Subsystem:** dash · **Crate:** `vagcan` · **Needs the car:** no

## Goal

Run the real panel on the laptop: `vag-dash` rendering a real plan, fed either by a
`watch --out` recording or by the live cable, in a window at 256×32 (scaled up), with the
three buttons on the keyboard.

This is how `02` gets tested and how the layout gets finished before any hardware
arrives. It is not a mock of the panel — it is the panel, with a different `DrawTarget`.

## Status (2026-08-20) — the PNG half is done

`cargo run -p vag-dash --example panel -- <dir>` writes nine frames and prints each one's
`Report`. That was enough to settle the layout and to catch three defects, so it came
first.

**What remains is the interactive half:** the SDL window, replaying a `watch --out`
recording through a plan, the three buttons on the keyboard, and diffing the PNGs in CI.
None of it is blocked; it simply was not what the first look needed. When it lands it
belongs in `vagcan` as `vagcan dash`, not in the example.

## Two backends, one flag

- **With SDL2** (`embedded-graphics-simulator` default features): a window, scaled ×4 so
  256×32 is legible, live.
- **Without SDL** (`default-features = false`): the same frames to PNG. This is what CI
  uses, and what lets a frame be produced and looked at without a display attached.
  `EG_SIMULATOR_DUMP=<path>` dumps the first frame and exits.

Keep the SDL dependency behind a cargo feature so `cargo test --workspace` on a machine
with no SDL2 still builds.

## Sources

- `--from <recording>` — replay a `watch --out` recording through the plan. The existing
  `watch/replay.rs` already owns seeking and the playhead; reuse it rather than writing a
  second clock. `research/dumps/` holds real drives.
- Live, over the cable, for a bench sanity check before the firmware exists.

A recording that does not contain a channel the plan names must say so plainly on the
cell — not draw a zero. A number the car never gave is the failure mode this whole
project is built against; on a 32-pixel panel with no room for a footnote it matters
more, not less.

## Buttons

`←`/`→` cycle pages, `space` is the third button (silence an alarm; long press =
brightness). Same handling code as the firmware's, so the interaction is tested here too.

## What this unblocks

Every layout argument — how many columns, how long a label can be, whether the unit
string fits, whether the chart's window reads — becomes a PNG someone can look at instead
of a claim someone makes.

## Tests

- A plan + a fixture recording produce a byte-identical PNG across runs (deterministic
  rendering).
- Seeking backwards in a replay does not leave the chart's ring buffer out of time order.
  `watch/history.rs` already carries this bug's test and its fix; the ring buffer here is
  a different structure and needs its own.
- A page whose channel is absent from the recording renders "no data", and the assertion
  is on the absence of a numeral.

## Done when

`vagcan dash --from research/dumps/<a real drive> --plan <plan.json>` opens a window that
looks like the panel, and the same command with `--png out/` writes frames CI can diff.
