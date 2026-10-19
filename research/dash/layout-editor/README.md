# Layout editor — the dash's pages in a browser (2026-10-19)

A page to lay out the dash's pages by dragging a car's channels onto a 256×64 panel, and get the
`[[page]]` / `[[channel]]` blocks for `dash.toml`. The panel is drawn by the dash's own renderer
(`vag-dash-render`, `draw_page`) compiled to WebAssembly, so text, fonts, the status strip and the
icons are what the board draws. The numbers on it are stand-ins, never readings.

**For:** choosing pages, labels and the strip per page before a firmware build. It produced the
owner's layout of 2026-10-19 (DRIVE, TEMPERATURES, an RPM chart, IGNITION TIMING ANGLE, MISFIRE,
BRAKES), which went into `~/.vagcan/dash/<VIN>/dash.toml` by hand.

**In:** a VIN whose project `vagcan setup` has cached, and optionally the evidence of which
identifiers answered: the August surveys (`research/dumps/survey-*.jsonl`, untracked) and
recordings (`research/captures/ble-10hz.csv`, `cruise-lever.csv`).

**Out:** `out/` (gitignored): `index.html`, `dash.wasm`, `catalog.json` (every channel the dash
build resolves for that car — 6,583 rows on the reference car, 3,607 numeric — with ODIS names),
`catalog-summary.txt`, and `channels.json` (the slice the page loads). The catalog is the
project's label data and one owner's car: it never goes into the checkout.

## Run

```
rustup target add wasm32-unknown-unknown
research/dash/layout-editor/build.sh <VIN> research/dumps/survey-*.jsonl research/captures/ble-10hz.csv research/captures/cruise-lever.csv
python3 -m http.server -d research/dash/layout-editor/out
```

Open http://localhost:8000. `build.sh` takes `CARGO_TARGET_DIR` like cargo; without it, builds
land in `research/dash/layout-editor/target/` (gitignored).

## What is where

| path | what |
|---|---|
| `index.html` | the editor. Opens on the reference car's pages of 2026-10-19 as an example; saves into a claude.ai artifact's store when it runs as one, otherwise keeps nothing |
| `wasm/` | the renderer for the browser: a page as JSON in, 256×64 bits out (`input`, `render`, `output`). Draws watts after the voltage itself — consumption is not in the renderer yet (`todo/dash/23`) |
| `catalog/` | `dash-catalog`: every ref goes through `vag-cli-core`'s own `dash::build`, so the editor offers only refs a `dash.toml` accepts. `run.sh` runs it with `HOME` at a copy of `~/.vagcan` |

`wasm/` and `catalog/` are crates outside the workspace with path dependencies on it: CI does not
build them, so a change to `dash::Input` or the renderer's API can break them (the strip added
`Input::strip` on 2026-10-19). Fix the field and rebuild.

## Notes from the session that made it

- `answered`: `true` from the surveys (identifier level) or from a recording's column whose heading
  names rows on one identifier; `false` inside the swept pages and silent; `null` outside them —
  the gearbox's `10xx` clutch temperatures and the ESC's `1822` are `null`, not silent.
- ODIS rows carry no range, so a chart's scale is set by hand.
- An alarm in `dash.toml` names its page by title; the editor holds on to the page instead and
  prints which `page =` to change after a rename.
