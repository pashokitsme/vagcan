# dash / 25 — one core for the board and the replay

**Subsystem:** dash · **Crates:** `vag-dash-render` (or a new `vag-dash-core`), `vag-dash-fw`,
`vag-cli-diag` (`dashreplay`), `research/dash/host` · **Needs the car:** no

**State (2026-10-20):** built on `refactor/dash-core`; archived here. Recorded 2026-10-19 from the
architectural review; owner approved.

## Why

The frame loop, the staleness rule and five constants exist twice, by hand:

- `dashreplay/engine.rs` re-declares `FRAME_MS`, `STALE_MS`, `MAX_CELLS`, `WIDTH`, `HEIGHT`
  with comments "`X` in vag-dash-fw", and re-implements `fresh_for = 3 periods or STALE`
  (`vag-dash-fw/src/bin/dash.rs`, the panel task). `ARCHITECTURE.md` calls it a mirror.
- A third copy has already drifted: `vag-dash-render/src/lib.rs` `PANEL` is 256×32, which the
  renderer's unit tests lay out against, while the board's glass is 256×64 (`src/panel.rs`).
- Five pure firmware modules (`schema`, `saving`, `faults`, `exchange`, `input`) are compiled
  by path from `research/dash/host/tests/`, outside the workspace.

## What

One `no_std` crate in the workspace owns the value store and its staleness, one frame-loop
step, the panel's geometry and those five modules. The board and the replay call it; the
mirror in `engine.rs` and the `#[path]` tests go, and the host tests become its tests.

## Done (2026-10-20)

**A new crate, `crates/dash/vag-dash-core`**, `no_std`, a workspace member, linked by the firmware
and by `vag-cli-diag`. Not a wider `vag-dash-render`: the fault count and an exchange's waits need
`vag-uds-client`, the queue `embassy-sync`, the settings record serde and postcard, and the
renderer is what `vag-dash-wasm` compiles for the browser — it keeps depending on nothing of ours.

- **`values`** — the store: `STALE_MS`, `fresh_for_ms(period)` (three periods of a slow channel,
  else `STALE_MS`) and `Slot` (`heard`, `current`, `within`). Times are microseconds of the
  caller's clock: the board's `Instant` is embassy's 1 MHz tick, so its comparisons are tick for
  tick what they were. A slot is 24 bytes, not 32: its time is read only with a value.
- **`frames`** — the frame: `FRAME_MS` (and its assert against `BLINK_MS`), and `Frames` with three
  steps the board takes its locks between — `glass` (the screen over the store at the frame's
  time), `step` (every chart's sample; a page miss said once; nothing for a page the plan does not
  have) and `picture` (cells through `Plan::shown` with the deviation, `?` for an unnamed channel,
  inversion from `Glass::inverted`, the strip, or the stopwatch's row), drawn with the caller's
  `Board`. The board's `panel_task` and `dashreplay::engine::Replay` both run it; the replay keeps
  only the bus (reads at `Plan::rates`, stamped with when the recording heard them).
- **`panel`** — the framebuffer and its `FRAME` line, sized by the renderer's `PANEL`.
- **Moved from `vag-dash-fw/src` unchanged but for docs:** `schema`, `saving`, `faults`,
  `exchange`, `input`, and `ssd1322` (its `#[path]` test was the sixth; its own 256×64 went).
- **The `#[path]` tests** — `exchange_waits`, `fault_count`, `glass_wire`, `input_queue`,
  `settings_saving`, `settings_schema`, and `stopwatch_silence` (which copied the answer timeout)
  — are `vag-dash-core/tests/`, every test kept (63). `research/dash/host` keeps its own tests and
  one decoding the core's `FRAME` line.
- **`PANEL` is the glass**, 256×64: render's tests lay pages out on it, the two tests of the
  32-row fallback name a `SHORT` size. On the glass a unit the face cannot draw was dropped with
  only `unit_dropped` said; `row_layout` now reports `glyph_missing` there too.
- `Config::defaults(plan)` is the record's (a foreign type takes no `impl Default` in the
  firmware); `vag-dash-fw`'s `config::defaults()` and `config::fit()` give it the image's plan.
- `Cell` derives `Clone, Copy, Debug, PartialEq`. `just no-std` checks the core for the board's
  target.

Left as it was: the replay's model of the bus (when a read is due) is the replay's own, as the
board's planner is not run there. The firmware's CAN and scheduler code is untouched but for
`RESPONSE_TIMEOUT`'s path, one comment, and the store under it: `PanelReads::fresh_for` asks
`fresh_for_ms`, `store` and `fresh` write and read a `Slot`.

**The `FRAME` line no longer has a buffer to outgrow** (2026-10-20, after review). The board
kept the line in a 4,128-byte `PANEL_LINE` (`W × H / 8 × 2 + 32`), and a picture of single-pixel
runs makes a line of `LINE_MAX` = 32,783 bytes: such a frame was never sent. Now the panel task
copies its framebuffer into `PANEL_PICTURE` (2 KB, a bit a pixel) and `usb_writer_task` encodes
the line as it writes it, a USB packet at a time (`Framebuffer::frame_line`, `FrameLine::fill`);
`write_frame` is the same encoder. `dashsim` reads a line up to `2 × LINE_MAX`. Tests: the
stripes' line is `LINE_MAX` exactly, the checkerboard's within it; every piece size adds up to
the old encoder's line for six pictures; the host decodes both worst cases pixel for pixel.
Static RAM −2,096 B with BLE, −2,088 B without.

## Checks

- `vag-dash-core`: 16 unit tests (the store's boundary to the microsecond, `fresh_for_ms`, the
  framebuffer's bits and `FRAME` line, a frame's composition, the blink at two frames a half
  through `Frames`, charts sampled every frame shown or not, a miss said once, the stopwatch's row)
  and the 64 moved and new integration tests.
- The replay's parity tests with the board's composition pass unchanged, and one more: the replay
  drawn into the board's own `Framebuffer` is the laptop's canvas bit for bit.
- The owner's plan, built offline from a copy of `~/.vagcan`: the firmware's clippy clean with BLE
  and without.
- Firmware static RAM (`ram-budget.sh`, empty plan): 140,976 → 141,000 B with BLE, 131,516 →
  131,540 B without (+24) for the core; then 138,904 B and 129,452 B with the streamed `FRAME`
  line, on `master` with #32. With the empty plan `master`'s compiler saw no page and dropped the
  page drawing whole — `draw_page`, the float formatting; `Frames` holds the plan behind a
  pointer, so the empty-plan image links them now (+34 KB flash, the 24 B of `.data` with them).
  On the owner's plan, where both images draw: 142,788 → 142,604 B with BLE, 133,608 → 133,424 B
  without — −184 B, the store's 8 B a slot × 23 channels; flash +1.5 KB.
