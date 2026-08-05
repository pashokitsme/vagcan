# `vagcan` — size, widgets, and where the crate boundaries should be

**Date:** 2026-08-05
**Status:** proposal, argued — nothing here is implemented, and two of the four
things asked for are recommended *against*.
**Measured at:** `4c8fc3c` (tree compiles, all tests pass; nothing in this
document was changed to measure it).
**Answers:** the two questions on the table — "выделить набор виджетов … tab,
picker, graph" and "крейт vagcan выглядит невероятно большим … может стоит
разделить".

## What this document concludes, before the evidence

1. **`vagcan` is big, and the reason is not crate layout.** 53 % of it is one
   command (`measure/`, 13 907 lines). A crate split cannot make a command
   smaller.
2. **There is almost no dead code** — 7 items in the real build, not the ~140 a
   naive `cargo check --all-targets` reports. The consolidation pass the owner
   wants first is therefore *small*, which is good news and also means it will
   not shrink anything much.
3. **`vag-widgets` should be a module, not a crate.** `vag-device` should not
   exist at all; its 89 lines belong in `vag-can`. If one crate is to be carved
   out, it is neither of those — it is the 3 378 lines of dependency-free
   physics and numerics under `measure/`.
4. **The chart widget is worth extracting, on one condition**: it is split into
   a pure function that produces a plotted structure and a thin ratatui
   renderer. Without that split it cannot be tested except by grepping braille,
   and it is not an improvement.
5. **No template engine.** `view.html` has exactly **one** hole in it, and the
   one escaping hazard it has is already handled correctly and would be handled
   *wrongly* by an HTML-escaping engine.
6. **The owner's stated target — `vagcan` as glue with UX only — does not
   survive contact with the code.** About 9 100 lines are car-facing
   orchestration that is neither pure logic nor UX, and it cannot leave, because
   it is where the async UDS client, the terminal and the deadline all meet.
   That is a finding, not a failure.

---

## 1. The size, measured

### Per crate

Two columns, because `vagcan` carries a disproportionate share of the tests and
a single total hides it. "code" is everything before the first `#[cfg(test)]`.

| crate | total | tests | code | `#[test]` fns |
|---|---:|---:|---:|---:|
| **vagcan** | **26 185** | 8 533 | **17 652** | **411** |
| vag-data | 7 053 | 3 345 | 3 708 | 129 |
| vag-protocol | 2 468 | 914 | 1 554 | 62 |
| vag-can | 1 486 | 514 | 972 | 41 |
| vag-db | 751 | 296 | 455 | 5 |
| vag-capture | 271 | 119 | 152 | 6 |
| vag-transport | 244 | 160 | 84 | 5 |

`vagcan` is 68 % of the workspace's lines and 62 % of its tests. The owner's
impression is correct and the ratio is not subtle.

### Inside `vagcan`

| module | total | code | share of the crate's code |
|---|---:|---:|---:|
| `measure/` | 13 907 | **9 097** | **52 %** |
| top-level `*.rs` | 8 682 | 5 778 | 33 % |
| `watch/` | 2 659 | 1 919 | 11 % |
| `vcds/` | 897 | 858 | 5 % |

One command is half the crate. That is the single most important number in this
document, and no crate boundary changes it.

Per file, code only (test tail removed), everything over 300 lines:

| file | code | test |
|---|---:|---:|
| `measure/mod.rs` | 1 735 | 441 |
| `measure/setup.rs` | 1 588 | 795 |
| `watch/mod.rs` | 1 222 | 324 |
| `measure/ui.rs` | 925 | 566 |
| `main.rs` | 891 | 87 |
| `analyse.rs` | 793 | 262 |
| `measure/coastdown.rs` | 759 | 411 |
| `measure/session.rs` | 726 | 511 |
| `measure/derive.rs` | 713 | 757 |
| `measure/report.rs` | 680 | 264 |
| `measure/carfile.rs` | 639 | 269 |
| `survey.rs` | 490 | 124 |
| `measure/channels.rs` | 487 | 283 |
| `watch/plan.rs` | 425 | 230 |
| `scan.rs` | 412 | 161 |
| `picker.rs` | **370** | **1 043** |
| `sniff.rs` | 364 | 142 |
| `vcds/tttext.rs` | 348 | 20 |
| `measure/power.rs` | 347 | 233 |
| `faults.rs` | 347 | 97 |
| `labels.rs` | 333 | 69 |
| `vcds/mod.rs` | 322 | 0 |

`picker.rs` is worth staring at: 370 lines of code carrying 1 043 lines of test.
That ratio is the reason the owner likes its shape, and it is the bar a chart
widget has to clear.

### Logic that does not belong in a CLI, versus UX that does

This is the harder question and it should be answered by what each file
*imports*, not by what it feels like. Fingerprint of every file in
`crates/vagcan/src`, by whether it mentions `ratatui`, `crossterm`, `clap`,
`tokio`, any `vag_*` crate, `std::fs`, or `serde_json`:

**A. Pure — imports nothing at all beyond `std` primitives (4 284 code lines, 24 %)**

| file | code |
|---|---:|
| `measure/coastdown.rs` | 759 |
| `measure/session.rs` | 726 |
| `measure/derive.rs` | 713 |
| `measure/report.rs` | 680 |
| `measure/power.rs` | 347 |
| `measure/messages.rs` | 259 |
| `discover.rs` | 232 |
| `vcdslog.rs` | 184 |
| `measure/types.rs` | 153 |
| `props.rs` | 123 |
| `progress.rs` | 108 |

This is the block that most obviously "does not belong in a CLI": a Gauss-Newton
coastdown fit, a Savitzky-Golay differentiator, a run state machine, a physics
model. It is also the block with the densest tests (`derive.rs` has more test
than code).

**B. Terminal UX — imports `ratatui` or `crossterm` (4 252 code lines, 24 %)**

| file | code |
|---|---:|
| `measure/mod.rs` | 1 735 |
| `watch/mod.rs` | 1 222 |
| `measure/ui.rs` | 925 |
| `picker.rs` | 370 |

**C. Everything else — car-facing orchestration and file work (≈ 9 100 lines, 52 %)**

`measure/setup.rs` (1 588), `main.rs` (891), `analyse.rs` (793),
`measure/carfile.rs` (639), `survey.rs` (490), `measure/channels.rs` (487),
`watch/plan.rs` (425), `scan.rs` (412), `sniff.rs` (364), `vcds/tttext.rs` (348),
`faults.rs` (347), `labels.rs` (333), `vcds/mod.rs` (322), `calibrate.rs` (273),
`watch/replay.rs` (272), `datadir.rs` (217), `render.rs` (190), `units.rs` (119),
`recording.rs` (89), `device.rs` (89), `vcds/corpus.rs` (92), `vcds/rod.rs` (96),
`measure/view.rs` (86), `names.rs` (81), `safety.rs` (69).

**Note that B and C overlap badly in two files.** `measure/mod.rs` and
`watch/mod.rs` are each a poll loop *and* a terminal host *and* a file writer;
they are counted in B because they draw, but roughly two-thirds of each is
neither drawing nor physics. That is where `measure/mod.rs`'s 1 735 lines come
from and it is the honest reason `vagcan` is large.

### The hypothesis, tested

> "Я бы хотел чтобы vagcan был клеем … в нём минимум логики и в основном она вся
> UX-направленная."

Take the target seriously and compute what it costs. Move block A out (4 284
lines) and move the identification pipeline out (`units.rs` + `props.rs` +
`labels.rs` + `measure/channels.rs` ≈ 1 000 lines, from block C). `vagcan` still
holds **≈ 12 400 lines**, still four times `vag-data`, and the residue is block C
minus what left — the poll loops, the deadline handling, the keyboard drains,
the CSV and JSON writers, the clap surface.

So the hypothesis fails, and it fails for a structural reason worth stating:
**an orchestration layer that owns a real-time loop is not glue.** `measure`
polls two batches per cycle, drains the keyboard *between* batches so `Esc` does
not wait out a two-second response deadline, times marks off the leading
channel's own timestamps, and writes a file on `s` without ever creating one
between two batches of a cycle. None of that is UX and none of it can move to a
library, because moving it means moving the terminal and the transport with it.

The realistic target is not "glue". It is: **`vagcan` owns the loops, the
terminal and the CLI; nothing else.** That is about 12 000 lines and it is a
defensible place to stop.

---

## 2. Dead and duplicated code, found rather than guessed

### The trap in the obvious measurement

`RUSTFLAGS="--force-warn dead_code" cargo check --workspace --all-targets`
reports **~140** dead items in `crates/`, including `fn main is never used` and
most of `main.rs`. That is not dead code. `--all-targets` compiles the binary a
second time as a test harness, where `main` is unreachable by construction, so
the report is a *test-reachability* map: it lists everything no unit test calls.
Chasing it would delete the program.

The number that means something is the ordinary build:

```
RUSTFLAGS="--force-warn dead_code" cargo check --workspace
```

`--force-warn` overrides the `#![allow(dead_code)]` blankets, so this sees
through them.

### The whole of the dead code

Seven items, in the entire workspace:

| where | what |
|---|---|
| `crates/vagcan/src/picker.rs:480` | `fn pick` — never used (17 lines) |
| `crates/vagcan/src/datadir.rs:113` | `fn reports_dir` — never used |
| `crates/vagcan/src/measure/carfile.rs:267` | `method path` — never used |
| `crates/vagcan/src/measure/messages.rs:21` | field `tried` — never read |
| `crates/vagcan/src/measure/mod.rs:415` | field `rho` — never read |
| `crates/vagcan/src/measure/power.rs:307,311` | `len`, `is_empty` ×2 — never used |
| `crates/vagcan/src/measure/types.rs:121` | `is_empty` — never used |

Call it 40 lines. **The consolidation pass will not shrink this crate**, and it
is better to know that before starting than to discover it three commits in.

### The two `#![allow(dead_code)]` blankets hide one function and one field

`picker.rs:43` and `measure/mod.rs:33` each carry a file-wide allow, both
justified in a comment as "written a command ahead of its callers". Under
`--force-warn` the total they are hiding is `picker::pick` and
`measure::Prepared::rho`. **Both blankets can be deleted for the price of two
deletions**, and deleting them turns `cargo check` back into a dead-code
detector for the two files most likely to accumulate it. `datadir.rs`'s two
targeted `#[allow]`s (lines 102, 112) are the correct form and one of them
(`reports_dir`) is now genuinely dead.

### Duplication, with paths

**`fn hex(bytes: &[u8]) -> String` — three copies, two identical.**

- `render.rs:85` and `sniff.rs:73` are byte-identical:
  `bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")`
- `survey.rs:487` is different — no separator:
  `bytes.iter().map(|b| format!("{b:02X}")).collect()`

Merging all three into one is a **bug**: `survey`'s output format would change.
The fix is two named functions (`hex_spaced`, `hex_packed`) in one place, and
`props.rs:67`'s `hex` method should call one of them.

**Terminal enter/leave — three hand-written copies, none of them RAII.**

| site | what it does |
|---|---|
| `measure/mod.rs:1084–1092` → restore at `1348–1349` | `enable_raw_mode`, `EnterAlternateScreen`, `Terminal::new` |
| `watch/mod.rs:742–747` → restore at `843–847` | same, plus `EnableMouseCapture` |
| `watch/mod.rs:1096–1105` → restore at `1204–1208` | same again, different error text |

All three restore with a bare statement on the success path. A `?` between enter
and restore — a serial write that fails, a file that will not open — leaves the
user in raw mode inside an alternate screen. **`picker.rs:321–338` already
solves this**, with a `RawMode` guard whose `Drop` restores through `?` and
through a panic, and with a comment explaining exactly why. Lifting that guard
and giving it an alternate-screen variant fixes a real defect and deletes three
copies. It is the single best first commit available.

**`device::resolve` is called from 12 sites** (`main.rs` ×10,
`measure/mod.rs:766`, `measure/setup.rs:813`). That is not duplication — that is
a shared helper working. Left alone.

**`watch::run` and `watch::run_recording`** (`watch/mod.rs:943` and `:671`) are
two ~200-line loops that differ in where samples come from and share the
terminal setup, the key drain and the draw dispatch. This is the largest genuine
duplicate in the crate and the only one worth a real refactor — but it is
`watch`-internal and it should be done *by* `watch`, not by a widget layer.

### Single-caller code that could be inlined

`picker.rs` has exactly **one** caller in the whole crate
(`measure/mod.rs:713–722`, the `measure view` file chooser). Its 370 lines and
32 tests exist for reuse that has not happened yet. That is not an argument to
delete it — it is the argument that decides §4: **the widget layer's problem is
not too few abstractions, it is abstractions with one client.**

---

## 3. The crate split, judged

### What a crate boundary actually costs here

Before judging each proposal, the price list, because this project should hear
it rather than assume it:

- **`pub(crate)` dies.** Everything shared across the boundary becomes public
  API. In this codebase that is not theoretical: `measure/ui.rs` and
  `measure/mod.rs` pass `Screen`, `Series`, `Track`, `Origin`, `Controls`,
  `Action` between them, and `Track` is defined in `measure/types.rs` — a file
  in block A. Splitting the widget out drags `Track` with it, and then `measure`
  depends on the widget crate for its *buffer type*.
- **A `Cargo.toml` and a feature surface per crate.** `vagcan` already carries a
  `rod-crack` feature that forwards to `vag-data/rod-crack`. Every new crate is
  another place a feature has to be forwarded through.
- **Compile-time is not obviously better.** A crate is a codegen unit, so
  splitting can parallelise — but only if the graph is wide. A chain
  `vagcan → vag-widgets → …` is not wider than what exists.

### The test a split has to pass

Look at what the *existing* boundaries buy, and apply the same test:

| boundary | what it keeps out | verdict |
|---|---|---|
| `vag-transport` (84 code lines) | the trait seam: `vag-can`'s serial code never reaches `vag-protocol`, and `vag-capture`'s replay satisfies the same trait so tests need no hardware | **pays, and it is the smallest crate here** |
| `vag-db` (455 code lines, **one consumer**) | `rusqlite` with bundled C SQLite is not in `vag-data`'s tree, so `vag-data`'s 129 tests build without compiling SQLite | **pays** — one consumer is fine when a dependency is firewalled |
| `vag-capture` (152 lines) | a file format that both `vag-can` tests and `vagcan` replay need | pays |

So the test is: **does the boundary keep a dependency, a compile cost, or a
capability out of somewhere it should not be?** Not "is this module cohesive" —
a module is already cohesive.

### `vag-device` — refuse

`device.rs` is **89 code lines**: `resolve` (pick the adapter, or say why one
cannot be picked), `list` (a wrapper over what `vag-can` already enumerates),
`render_list`, `open_failure`. It imports `vag_can` and nothing else.

- `resolve` and `list` are properties of *the backend*. "Which adapter, when
  exactly one is plugged in" is a `vag-can` question, and `vag-can` already owns
  the enumeration. **Push them down into `vag-can`**, not sideways.
- `render_list` and `open_failure` produce sentences a person reads. That is UX
  and it stays in `vagcan`, which is exactly where the owner wants UX.

A `vag-device` crate would be a `Cargo.toml`, a name, and a boundary between a
40-line function and its twelve callers. It buys nothing on the test above.
**Refuse.**

If "discover" was meant more broadly — identify the car, resolve what it can
measure — then the real content is `units.rs` (119) + `props.rs` (123) +
`labels.rs` (333) + `measure/channels.rs` (487) ≈ 1 060 lines: gateway
installation list → part numbers → `F19E` ODX name → corpus → resolved channels.
That *is* non-UX, it *is* cohesive, and it *does* pass the test weakly (it
firewalls `vag-db` and the corpus lookup away from the loops). But its name is
not `vag-device`; it is `vag-vehicle`, and it should go **after** the crate
below, because it is the one whose API is least obvious.

### `vag-widgets` — refuse as a crate, accept as a module

Candidate content: `picker.rs` (370 + 1 043 test) and the reusable half of
`measure/ui.rs` (925 + 566 test) ≈ 1 300 code lines, 59 tests.

Against the test:

- It firewalls **no dependency**. `vagcan` depends on `ratatui` and `crossterm`
  regardless — it is the thing with a terminal. Moving widgets out does not
  remove either from `vagcan`'s tree.
- It has **one consumer, permanently**. There is one binary in this workspace
  with a terminal, and the roadmap does not add a second.
- It forces `Track`, `Series`, `Origin` public and drags `measure/types.rs`
  across a boundary that `measure`'s pure numerics also sit behind.
- It costs the thing the boundary is supposed to protect: `picker.rs`'s
  `Scripted` double is `#[cfg(test)]`, so *other commands' tests* can drive a
  picker with no terminal. Across a crate boundary `#[cfg(test)]` items are not
  visible to the consumer — the double would have to become a `test-util`
  feature, exactly as `vag-transport` already had to do. That is a real,
  paid-in-`Cargo.toml` cost for zero benefit.

**Recommendation: `crates/vagcan/src/ui/` — a module tree, not a crate.**

```
crates/vagcan/src/ui/
  mod.rs        the seam (§4): Chooser, Series, Plot, Table
  picker.rs     moved verbatim from src/picker.rs
  chart.rs      lifted from measure/ui.rs (§4)
  term.rs       the RAII terminal guard (§2), lifted from picker::RawMode
```

Same code, same tests, no `Cargo.toml`, `pub(crate)` still available, `Scripted`
still `#[cfg(test)]`. Revisit the crate the day a second binary needs it.

### The split nobody proposed, which is the one that pays — `vag-measure`

Block A under `measure/` is **3 378 code lines** with **≈ 2 500 lines of tests**
and **zero imports outside `std`**:

`coastdown.rs` 759 · `session.rs` 726 · `derive.rs` 713 · `report.rs` 680 ·
`power.rs` 347 · `types.rs` 153

What it buys, in this project's own terms: **"this code cannot touch the car" is
enforced by the compiler rather than by convention.** A crate that depends on
nothing but `std` cannot `use vag_can`, cannot open a serial port, cannot send a
UDS request. For a repository whose `SAFETY.md` exists because an identifier
sweep permanently destroyed a steering assist unit, an invariant that a reviewer
currently has to check by reading imports becomes one `Cargo.toml` line.

The honest counter-argument, which the owner should have: **the invariant
already holds** — the fingerprint table in §1 proves those six files import
nothing today. The crate buys enforcement of a property that is currently true,
not a fix for one that is false. Whether that is worth a `Cargo.toml` is a
judgement call, and it is a closer call than the paragraph above makes it sound.

What tips it: it is the only proposed split that moves a large block (19 % of
`vagcan`) with **no API design at all** — the module boundaries already exist
and are already `pub` within `measure`. It is the cheapest big move available.

### Summary

| proposal | verdict | why |
|---|---|---|
| `vag-device` | **refuse** | 89 lines; `resolve`/`list` belong in `vag-can`, the messages are UX |
| `vag-widgets` | **refuse as a crate** | firewalls nothing, one consumer forever, breaks `#[cfg(test)] Scripted`. Do it as `src/ui/` |
| `vag-measure` (new) | **accept, second** | 3 378 lines, zero deps, compiler-enforced "cannot touch the car", no API design needed |
| `vag-vehicle` (new) | **maybe, third** | ≈ 1 060 lines of identification; real, but the API is the least obvious |
| leave the rest | **accept** | the loops, the terminal and the CLI are `vagcan`'s job |

---

## 4. The widget seam, in Rust

Three widgets are on the table. Only one of them is hard.

### `picker` — already done, move it and stop

`Chooser` / `Console` / `Scripted` / `Decision` / `Level` is the right shape and
its 32 tests need no terminal. It needs **no redesign**: move the file to
`src/ui/picker.rs`, delete `pick` (dead, §2), delete the `#![allow(dead_code)]`
blanket, and get a second caller. The obvious second caller is
`recording …` — `watch --out` recordings are exactly the "eighteen timestamps
that look alike" case the module's own doc comment describes.

### `tab` — extract only what two callers already share

`watch/mod.rs` has a tab strip: `App.tab`, `draw_units` (`:315`), `step_tab`
(`:523`), and mouse hit-testing against `App.unit_area` (`:546`). Nothing else
in the crate has tabs. **One caller is not a widget.** Extract it when
`measure`'s results view or the `watch` chart page needs the second one, and
extract exactly the part they share — which will be "a strip of labels, an index,
and the rect it was drawn in", not `App`.

Explicitly *do not* generalise `draw_select` (`:458`). The multi-select list with
a live substring filter and a per-unit tab is a different animal from
`picker`'s single-select list, and folding them together produces a widget with a
`multi: bool` and a `filter: Option<String>` — the dialect failure mode of §5.

### `chart` — the one that needs designing

Today `draw_chart` (`measure/ui.rs:619–739`) does six things in one function:

1. picks a page via `pages()`,
2. drops trailing series until the key fits the width,
3. chooses which unit owns the Y axis and folds the rest onto it,
4. computes the shared time origin,
5. builds `ratatui::Dataset`s with colours and markers,
6. renders.

Steps 1–4 are arithmetic. Steps 5–6 are ratatui. **The seam goes between 4 and
5**, and that is the whole proposal:

```rust
// src/ui/chart.rs

/// One line. Owns nothing about cars, marks, or where time started.
pub struct Series {
    pub label: String,
    /// What it is measured in. This is what groups lines onto a scale, so a
    /// series with no unit has a scale of its own by definition.
    pub unit: String,
    pub points: Track,
    pub origin: Origin,          // Bus | Computed(&'static str)
}

/// Which series share one chart. Greedy and stable, exactly as today.
pub fn pages(series: &[Series]) -> Vec<Vec<usize>>;

/// Everything the renderer needs, and nothing a renderer produces.
///
/// This is the assertion surface. It is `PartialEq + Debug`.
pub struct Plot {
    /// The unit drawn on the Y axis, and its bounds.
    pub axis_unit: String,
    pub y: (f64, f64),
    /// Seconds, relative to the earliest point drawn.
    pub x: (f64, f64),
    /// One per drawn series, already folded onto `y`, already time-shifted.
    pub lines: Vec<PlotLine>,
    /// Series that did not fit the width, in the order they were dropped.
    pub dropped: usize,
    pub page: (usize, usize),    // (this, of)
}

pub struct PlotLine {
    pub label: String,
    pub colour: usize,           // index into the palette, not a ratatui Color
    pub dotted: bool,            // Origin::Computed
    /// `Some((lo, hi, unit))` only when this line was folded onto somebody
    /// else's axis. A folded line without a range is a lying chart, so this is
    /// not optional and there is no flag to suppress it.
    pub folded_from: Option<(f64, f64, String)>,
    pub note: Option<&'static str>,
    pub points: Vec<(f64, f64)>,
}

/// Pure. No `Frame`, no `Rect` — `width` because the key has to fit, and
/// fitting the key is what decides how many lines survive.
pub fn plot(series: &[Series], page: usize, width: u16) -> Option<Plot>;

/// Thin. Turns a `Plot` into a `ratatui::Chart` and renders it.
pub fn draw(frame: &mut Frame, plot: &Plot, area: Rect);
```

**What each part must not know.** `plot` must not know which series is speed,
what a mark is, that time started at a launch, or how long a buffer should be —
it draws `[min t, max t]` of whatever it is handed, and trimming stays with the
caller (§6 turns on this). `draw` must not compute anything; if it does
arithmetic, that arithmetic is untestable again.

**How a chart's output is asserted — the answer to the question that matters.**

Today the chart's 27 tests go through `TestBackend` and grep the rendered text:
`text.contains("km/h")`, `text.contains("/min]")`, `text.contains("⋯ computed")`.
That works for the *key* — the key is text. It cannot touch the fold arithmetic
at all, because the fold's only visible output is which braille cells are lit.
The one property the fold has to satisfy is invisible to every existing test.

With `plot()` pure, the fold becomes an equality:

```rust
#[test]
fn a_folded_line_lands_on_the_axis_it_was_folded_onto() {
    let speed  = series("speed", "km/h",  &[(0.0, 0.0), (1.0, 100.0)]);
    let engine = series("engine", "1/min", &[(0.0, 800.0), (1.0, 6480.0)]);
    let p = plot(&[speed, engine], 0, 80).unwrap();

    assert_eq!(p.axis_unit, "km/h");
    assert_eq!(p.y, (0.0, 100.0));
    // 800 is the bottom of its own range, so it lands on the bottom of the axis;
    // 6480 is the top, so it lands on the top. Not "somewhere in the middle".
    assert_eq!(p.lines[1].points, vec![(0.0, 0.0), (1.0, 100.0)]);
    assert_eq!(p.lines[1].folded_from, Some((800.0, 6480.0, "1/min".into())));
}

#[test]
fn a_flat_series_still_has_bounds_and_does_not_render_as_nothing() {
    let p = plot(&[series("speed", "km/h", &[(0.0, 50.0), (1.0, 50.0)])], 0, 80).unwrap();
    assert_eq!(p.y, (49.5, 50.5));   // today's `widen`, now assertable
}

#[test]
fn the_key_is_what_gets_cut_when_the_terminal_is_narrow_and_it_says_so() {
    let p = plot(&three_series(), 0, 24).unwrap();
    assert!(p.lines.len() < 3);
    assert_eq!(p.dropped, 3 - p.lines.len());
}
```

The `TestBackend` tests stay. They are the only thing that proves the key
*renders* inside 40 columns and that `⋯ computed` reaches the screen, and losing
them would be a regression. The claim is not "replace them" — it is "add the
layer they cannot reach".

**Table widgets: leave them.** `draw_values` (`ui.rs:568`), `draw_marks`
(`:816`) and `watch::draw_live` (`:356`) all build a `ratatui::Table` with
content-derived column widths. They look alike and they are not the same: the
marks panel's width comes from the widest `name + value` pair clamped to
`12..=24`, `watch`'s from six independently-computed columns including a
`did_w` that depends on whether a row is an actual/specified pair. A shared
"auto-width table" would take a closure per column and be longer than both.
`ratatui::Table` is already the widget. **Refuse.**

---

## 5. The hard part, named honestly

> Is a general chart widget possible without becoming a configuration language?

The pressure comes from three places, and they are not equal.

**One Y axis.** `ratatui::Chart` has `x_axis` and `y_axis` and no third. The
current code's answer — one unit owns the axis, the rest are folded onto it, and
the fold's source range is printed in the key — is not a workaround, it is the
correct answer, and its correctness depends on the key. So `folded_from` above
is **not** an `Option` the caller can decline; it is computed by `plot()` and
consumed by `draw()`. There is no `show_key: bool`. A parameter that can turn a
chart into a lie is not a parameter.

**Caps.** `MAX_LINES = 3` and `MAX_UNITS = 2` are constants in `measure/ui.rs`
with a paragraph of justification (rpm at 6480 beside boost at 2.1 bar destroys
both scales). The temptation is `pages(series, max_lines, max_units)`. **Resist
it.** If `watch` wants four lines, that is not a different caller preference —
it is evidence that the cap should be a function of `area.height` and the number
of distinguishable colours, computed inside the widget. Two callers with
different constants is one widget with a bug.

**Colour.** `LINE_COLOURS` is a fixed array so a series does not change colour
between cycles. Making it a parameter buys a themeable chart nobody asked for
and costs the stability property. `PlotLine.colour` is an *index*, and `draw()`
owns the palette.

### Where the line is

The widget owns: paging, folding, the key, the palette, dropping-to-fit, axis
bounds, the time origin.

The caller owns: **which series, in what order, and what is in their buffers.**

That is two parameters — a slice and a page number — plus the width. Everything
else the widget decides. If a caller ever needs a third, the right response is
to ask why the widget cannot decide it from `area`.

### What stays in `measure`, and it is a lot

Not everything in `measure/ui.rs` generalises, and pretending otherwise is how a
widget layer becomes a second problem:

- `Screen` (`:286`) — `band`, `banner`, `rows`, `marks`, `series`, `chart`,
  `hz`, `file`, `warning`, `table`. This is `measure`'s screen and it should
  stay `measure`'s screen. The widget takes `&[Series]`, not `&Screen`.
- `draw` (`:481`) — the layout, including the rule that the marks panel takes
  what its content needs and the values take the rest, and the rule that a
  finished run's results table takes the whole middle. Layout is per-command.
- `band` / `phase_of` (`:63`, `:93`) — the run state machine's line.
- `Controls` / `on_key` / `Action` (`:314`, `:444`) — keyboard state including
  the two-keystroke quit guard.
- `MarkRow` (`:162`) and its `value()`, which prints `≈1.04 s` for a launch mark
  and `3.24 s` for a rolling one. That distinction is the `measure` design's
  whole §3.
- `plain_line` (`:842`) — the piped-output path.

Of `measure/ui.rs`'s 925 code lines, roughly **330** move (`Series`, `Origin`,
`pages`, `draw_chart`, `key_line`, `notes_line`, `tick`, `LINE_COLOURS`,
`MAX_*`). The other ~600 stay. **"Part of this should stay in `measure`" is the
answer**, and it is most of it.

---

## 6. `watch`'s charts, concretely

This is where the proposal is tested, so it gets specifics rather than
enthusiasm.

### What `watch` does not have

**`watch` keeps no history.** `App.latest` (`watch/mod.rs:57`) is a
`BTreeMap<(u16, u16), (f64, Vec<u8>)>` — *the* latest body per identifier, with
its timestamp. There is no `Track` anywhere in `watch/`. A chart is not a
rendering change; it is a new buffer, its trim policy, and the memory that
policy implies.

That is the first concrete consequence and it lands entirely on the **caller**
side of the seam, which is the design working: `plot()` never learns that
`watch`'s time axis is unbounded, because `watch` hands it a trimmed buffer,
exactly as `measure` does today (`measure/mod.rs` trims to `CHART_SECONDS` and
empties the buffer at each launch).

| | `measure` | `watch` |
|---|---|---|
| time origin | the launch — buffer cleared at each one | the session, or the last N seconds |
| trim | fixed window, cleared per run | rolling window; a 4-hour drive at 10 Hz is 144 000 points per channel |
| channels | 5 named roles, resolved before the loop | whatever `c` selected at runtime, changeable mid-run |
| marks | yes | none |
| runs | a session of them | none |
| origin classes | `Bus`, `Computed` | `Bus`, `Computed`, **and `(raw)`** |

### The three things that actually bite

**1. Runtime-chosen channels break page stability.** `pages()` is greedy and
stable *given a stable series list*, and `measure`'s list is fixed before the
loop starts. In `watch` the list changes when someone presses `c`. Pressing `c`
and deselecting one channel can reshuffle every page after it, so `←`/`→` no
longer means what it meant. This is not a widget bug — `pages()` is a pure
function of its input — but it is `watch`'s problem to solve, and the honest
solution is that **the chart page is drawn from the current selection in
selection order**, and changing the selection resets the page index to 0 with a
note saying so. Silently renumbering pages is worse than resetting them.

**2. Nobody chose the units, so the pages can be absurd.** With 30 selected
channels across four units, `pages()` at 3 lines / 2 units produces 10–15 pages.
`←`/`→` through fifteen pages is not a UI. `watch` needs a **chart selection**
distinct from its value-table selection — which it can get almost free, because
`draw_select` already exists and already has a checkbox column. The rule: the
chart draws the channels marked for charting, capped by a number `watch`
enforces, and the widget is not told about the cap.

**3. `watch` has a class `measure` refuses, and it cannot be charted.**
`plan::Channel::render` returns `"… (raw)"` for a channel whose scaling is not
proven, and `watch`'s whole purpose is to *show* those so they can be found.
There is no float in a raw channel — there is a byte string. So the chart must
exclude them. Per this project's ethic that is not a silent exclusion: the chart
key says `3 raw channels not plotted — unproven scaling`, because a driver who
selects a raw channel and sees nothing appear will conclude the tool is broken.

This is a genuine addition to the seam and the only one I would accept: `plot()`
takes `&[Series]`, and `Series` has no representation for "a value with no
number". So the caller filters, and the *count of what it filtered* has to reach
the key. Two options, and I prefer the second:

- add `excluded: usize` to `plot()`'s signature — a parameter that exists for
  one caller, which is the dialect smell;
- let `watch` render the sentence in its own footer, next to the chart, where it
  already renders `"{n} of {m} shown"`. **This one.** The widget stays a
  two-parameter function and `watch` says the `watch`-specific thing in the
  `watch`-specific place.

### What `watch` gets for free

Everything in `Plot`: folding a 6 480 rpm line onto a 100 km/h axis with the
range in the key, dropping lines when the terminal is narrow and saying how
many, the dotted marker for computed lines, stable colours. That is the bulk of
`draw_chart`'s 120 lines, and `watch` would otherwise write it again — badly,
because the fold is the part that is easy to get subtly wrong and impossible to
see.

### The browser page for `watch`

The owner also wants "такие же view" for `watch`. That is `view.html` reading a
different document, and it is the fact that decides §7 — see below.

---

## 7. The HTML question

### What is there

`crates/vagcan/src/measure/view.html`, 1 815 lines:

| lines | what |
|---:|---|
| 7–236 | `<style>` — 229 lines of CSS, sectioned by comment (controls, results, chart) |
| 238–297 | `<body>` — 60 lines of markup scaffold |
| 241 | `<script type="application/json" id="session-data">/*{{SESSION}}*/</script>` |
| 298–1812 | `<script>` — **1 514 lines of ES5** |

`measure/view.rs` is 86 code lines and its entire templating is:

```rust
const PAGE: &str = include_str!("view.html");
const MARKER: &str = "/*{{SESSION}}*/";
PAGE.replace(MARKER, &json)
```

### Adopt a template engine? No, and the reason is arithmetic

**The page has one hole.** Not "few" — one. `askama` compiles templates and
gives typed field access; there are no fields to type, because the session
crosses as an opaque `serde_json::Value` and every field is read by ES5 in the
browser. `view.rs`'s own doc comment says why that is deliberate: reading it as
a document "keeps this module from being rewritten every time a field is added
to the writer".

**The one thing an engine is genuinely for — escaping — is already solved, and
an engine would solve it wrongly.** `view.rs:35–37`:

```rust
let json = serde_json::to_string(session)
    .unwrap_or_else(|_| "null".to_string())
    .replace("</", "<\\/");
```

That is the correct fix for the only hazard a `<script type="application/json">`
block has: a `</script>` inside a string closing the block early. `\/` is a JSON
escape for `/` and round-trips. An HTML-escaping template engine would emit
`&lt;/script&gt;` into a JSON blob and produce a page that parses as HTML and
fails as JSON. **The engine's flagship feature is a regression here.**

Cost side, for completeness: `askama` adds `askama` + `askama_derive` and pulls
`syn`/`quote` into the build; `minijinja`/`tera` add a runtime parser and move
template errors from compile time to the moment a driver opens a chart. Both add
a template *directory* — which is the fragment split, with an engine bolted on.

**Recommendation: no engine, and this is unlikely ever to change.** The
condition that would overturn it: a page that interpolates Rust data at many
points in *markup* — loops emitting `<tr>` per row. This design deliberately
cannot reach that state, because all rendering happens in the browser from one
JSON blob. Abandoning that is the decision to revisit; the engine is downstream
of it.

### Split into fragments? Yes — but only when the second page exists

The 1 814-line file is a real irritation and it is a **JavaScript organisation**
problem, not a Rust templating one. The split is free:

```rust
const PAGE: &str = concat!(
    include_str!("view.head.html"),
    "<style>",  include_str!("view.css"),  "</style>",
    include_str!("view.body.html"),
    "<script>", include_str!("view.js"),   "</script>",
);
```

Byte-identical output, no dependency, no build step, and each file opens in an
editor that understands it. What it buys over the status quo *today*: editor
tooling and smaller diffs. That is not nothing, and it is not much either — it
moves 1 814 lines from one file into four without deleting a line, and §8 says
consolidation comes before rearranging.

**What tips it is `watch`'s page.** Two pages that share a chart renderer and a
stylesheet and duplicate them is the failure mode; two pages that
`include_str!` the same `chart.js` and `view.css` is the fix, and it needs no
library. So: **split the file when the second page is written, not before, and
split it by language rather than by feature.** If the second page never happens,
the split was never needed.

The one thing an engine buys that a fragment split does not is **template
inheritance** — a base page with named blocks. With two pages and shared
`include_str!` fragments, inheritance is a `concat!` with different middles.
That is enough for two. It would stop being enough at five or six, and this
project will not have five HTML pages.

---

## 8. Ordering

Consolidate and delete first, restructure second — the owner's sequence, and it
happens to be the right one here because every item in phase 1 is independently
revertible and none of them changes an interface.

### Phase 0 — the first commit, small and reversible

**One RAII terminal guard.** Lift `picker::RawMode` (`picker.rs:321–338`) into
`src/ui/term.rs`, give it an alternate-screen + mouse-capture variant, and use it
at the three sites in §2. Net: about −40 lines, one real defect fixed (a `?`
between enter and restore currently leaves the terminal broken), zero interface
change, and it is the seed of `src/ui/` without committing to anything in §4.

Revert cost: one `git revert`.

### Phase 1 — consolidation, in this order

1. Delete the 7 dead items (§2). ~40 lines.
2. Delete both `#![allow(dead_code)]` blankets — they hide one function and one
   field, both of which item 1 removed. From here `cargo check` is a dead-code
   detector again.
3. Add a note to `CLAUDE.md` or the workflow doc: the dead-code check is
   `cargo check --workspace`, **not** `--all-targets`, and say why (§2). The
   ~140-item false report will otherwise be rediscovered and acted on.
4. `hex_spaced` / `hex_packed` in one place; `render.rs:85`, `sniff.rs:73`,
   `survey.rs:487`, `props.rs:67` call them. **Assert `survey`'s output is
   unchanged first** — its copy has no separator.
5. Give `picker` its second caller (`recording …`). This is not consolidation,
   but it is the cheapest way to find out whether the picker seam actually
   generalises, and the answer changes §4's confidence.

### Phase 2 — the module, not the crate

6. `src/picker.rs` → `src/ui/picker.rs`; `term.rs` joins it. Pure move.
7. Split `plot()` from `draw()` in `measure/ui.rs` **in place**, still inside
   `measure`, and write the equality tests of §4. If the fold arithmetic does
   not survive being asserted, better to find out before it moves.
8. Move `plot`/`draw`/`pages`/`Series`/`Origin`/`Track` to `src/ui/chart.rs`.
   `measure/ui.rs` shrinks by ~330 lines and keeps ~600.

### Phase 3 — `watch`'s charts

9. Give `watch` a `Track` buffer and a trim policy, and a chart-selection mark
   in `draw_select`. Then the chart page is 20 lines of caller.
10. Deduplicate `watch::run` / `watch::run_recording` (§2) — by now they share
    the terminal guard, so what is left is the sample source.

### Phase 4 — the crate, if still wanted

11. `crates/vag-measure` — `coastdown`, `session`, `derive`, `report`, `power`,
    `types`. 3 378 lines, no API design, `vagcan` drops to ≈ 14 300.
12. `crates/vag-vehicle` — the identification pipeline, if its API has become
    obvious by then. If it has not, leave it.

### What a widget layer or a crate split makes *worse*

Named explicitly, because these are the things that will be broken by accident:

- **`picker::Console` deliberately has no alternate screen** (`picker.rs:36–38`:
  "the lines it prints while working … should still be on the screen
  afterwards"). A shared terminal guard that always enters the alternate screen
  destroys that. The guard needs two modes and the picker's is the default one.
- **`plain_line`** (`ui.rs:842`) and `watch`'s plain-console mode: a widget layer
  that speaks only `Frame` pushes the piped-output path out of the design. Every
  offline path in this tool works redirected and that must not become a
  casualty.
- **`Scripted`** is `#[cfg(test)]` so other commands' tests can use it. A crate
  boundary breaks that and forces a `test-util` feature. This alone is enough to
  refuse `vag-widgets`.
- **The chart's key is load-bearing.** Any generalisation that lets a caller
  suppress it, or that makes `folded_from` optional, converts a correct chart
  into a lying one. §5 names the parameters that must not exist:
  `show_key`, `max_lines`, `max_units`, `colours`.
- **`measure/types.rs::Track`** straddles the physics block and the widget block.
  It moves to `src/ui/chart.rs` in phase 2 and would then have to move *back* if
  `vag-measure` is carved out in phase 4 — or `vag-measure` depends on the widget
  module for its buffer type, which is backwards. **Decide this before phase 2**:
  the clean answer is that `Track` is a numerics type, it lives with the physics,
  and `ui::chart` depends on it rather than defining it. That makes phase 4's
  crate the *lower* layer, which is the correct direction.
- **`measure/mod.rs`'s deferred keyboard actions** (`Controls::take_save`,
  `take_discard`, `take_keep`) exist so a file is never created between two
  batches of one cycle. Any widget layer that handles keys itself, rather than
  reporting them, breaks that guarantee silently.

---

## Open questions

- **Does `picker` generalise?** One caller today. Phase 1 item 5 answers it, and
  if the second caller needs a `Level` field that only it uses, the module is a
  `measure view` helper wearing a trait and should be treated as one.
- **How large a `Track` can `watch` afford?** 10 Hz × 4 hours × 30 channels is
  ~4.3 M points. The trim policy is a real design decision and this document
  does not make it. **Answered** (`watch/history.rs`): a fixed window of sixty
  seconds, and not a fixed number of samples. `watch`'s poll rate is not a
  constant — one identifier on one unit answers at tens of hertz and thirty
  across four units cost a request and a deadline each — so N samples is a
  window of unknown length whose extent changes under the reader every time
  somebody presses `c`. The window is printed under the chart for the same
  reason the fold's range is printed in the key.
- **Is `vag-measure` worth a `Cargo.toml`?** Argued both ways in §3 on purpose.
  The invariant it enforces already holds; the crate makes it a compiler's job
  instead of a reviewer's. That is a judgement about how much the project
  distrusts its future self.
- **Guessed, not measured:** that `watch`'s chart selection can reuse
  `draw_select`'s checkbox column cheaply. I read the function; I did not try it.
  **Tried, and it holds** — with one correction: the mark is a sixth column
  reading `chart` rather than a second `[x]`, because two boxes side by side on
  one row is a puzzle rather than a choice. What it did cost was a second row of
  hints: that screen's key line was already longer than a hundred columns, and
  one more key pushed `[enter] back` off the end of it.
