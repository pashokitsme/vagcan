# dash / 25 — one core for the board and the replay

**Subsystem:** dash · **Crates:** `vag-dash-render` (or a new `vag-dash-core`), `vag-dash-fw`,
`vag-cli-diag` (`dashreplay`), `research/dash/host` · **Needs the car:** no

**State (2026-10-19):** recorded, not started. Architectural review of 2026-10-19; owner approved.

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
