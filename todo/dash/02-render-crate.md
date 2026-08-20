# dash / 02 — `vag-dash`, the rendering crate

**Subsystem:** dash · **Crate:** `vag-dash` (new) · **Needs the car:** no

## Goal

A `no_std + alloc` crate that draws a plan's pages onto any `embedded_graphics`
`DrawTarget`, at 256×32, and knows nothing about where it is drawing.

That last clause is the point. The same crate renders into the SSD1322 on the board and
into `embedded-graphics-simulator` on the laptop, so "the simulator" and "the firmware"
are one body of code with two sets of dependencies, and the layout can be finished
without leaving the desk.

## Dependencies

`embedded-graphics` 0.8.2, `eg-seven-segment` 0.2.0 (numbers), `u8g2-fonts` 0.8.0
(labels — the stock fonts have no Cyrillic), `embedded-layout` 0.4.2 (grid),
`embedded-canvas` 0.3.2 (off-screen composition for partial update).

No `std`, no float formatting via `format!` if it can be helped — fixed-point rendering
of `value * factor + offset` to `decimals` places.

## The two page kinds

**Values.** Four columns of 64 px. A 6 px label on top, a ~20 px number under it. 64 px
at 6 px per glyph is **ten characters of label**, so labels are abbreviations and the
generator must be told so rather than silently clipping. The unit string does not get its
own row — 32 px does not hold four tiers — so it goes small beside the number or into the
label, decided per cell in the plan.

One to four cells; fewer cells means wider columns, not blank space on the right.

The per-cylinder screens use this page kind unchanged: one column per cylinder.

**Chart.** The value large on the left (~26 px, ~60 px wide), a sparkline in the
remaining ~190 px. **One pixel per poll**, so the window is width ÷ rate — about 19
seconds at 10 Hz — and that number is drawn on the screen, because a window nobody can
see is a chart nobody can read (the same argument `watch/history.rs` makes for printing
`WINDOW_SECONDS`).

**The vertical scale is fixed, from the plan.** Autoscale is the tempting default and it
lies twice: it turns a flat trace into drama, and it flattens a real collapse the moment
one outlier widens the range. A boost trace that always fills the box tells you nothing.

A ring buffer of `u8` per charted channel, one per pixel column — 190 bytes. Sized at
compile time from the plan.

## Burn-in

This panel is on whenever the car is, showing the same four labels in the same four
places, for years. OLED burn-in is not recoverable, so the mitigation goes in from the
first version rather than after the labels are ghosted:

- shift the whole frame by one pixel on a slow cycle (a few minutes), and
- keep bright fills transient — which the alarm inversion in `04` already is.

## Not `mousefood`

The ratatui backend for `embedded-graphics` (0.5.2, actively maintained) would let this
reuse `watch`'s widgets, and it is the wrong tool: a character grid gives every cell one
height, and this design exists because the number is three times the label. Recorded here
so the next session does not re-derive it.

## Tests

Screenshot tests, via `03`'s simulator harness — `embedded-graphics-simulator` with
`default-features = false` renders to a PNG with no SDL and no window, so this runs in CI.

- A four-cell values page at 256×32 puts every glyph inside the frame. This is the test
  that catches the failure `watch` has already had twice: one added element wraps, and
  the wrap silently carries a row out of the box.
- A label longer than the column is reported by the layout, not clipped in silence.
- A chart with a constant series draws a straight line at the height the fixed scale
  implies — not a line through the middle, which is what autoscale would give.
- A chart with fewer samples than pixels draws only what it has; no invented points.
- The burn-in offset moves every drawn pixel and changes no reported value.

## Done when

`cargo test -p vag-dash` renders every page kind to PNG in CI with no display attached,
and the crate builds `no_std`.
