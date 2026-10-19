# Layout editor — the dash's pages in a browser (2026-10-19)

**Now a command: `vagcan dev dash edit <VIN>`** (2026-10-19). What was here moved into the
workspace; this file keeps what the research found.

| was here | is now |
|---|---|
| `index.html` | `crates/cli/vag-cli-diag/src/dash/editor.html`, embedded in `vagcan`, in English; opens on the car's own `dash.toml` (`layout.json`), not on a layout written into the page |
| `wasm/` | `crates/dash/vag-dash-wasm`, a workspace member (so the host build catches a renderer API change); `vag-cli-diag/build.rs` compiles it for `wasm32-unknown-unknown` when that target is installed |
| `catalog/` (`dash-catalog`) | `crates/cli/vag-cli-diag/src/dash/catalog.rs`, run in-process; `channels.json` is kept under `~/.vagcan/dash/<VIN>/editor/` until the project, the record, `names.csv`, the language or `vagcan` change |
| `build.sh`, `python3 -m http.server` | the command: writes the page, the wasm, `channels.json` and `layout.json` into that directory and serves it on `127.0.0.1` |

Dropped on the way, on purpose:

- **Survey and recording evidence** (`answered`, from `research/dumps/survey-*.jsonl` and
  `research/captures/*.csv`): one car's August reads, not data the command has for another car.
- **Stand-in numbers** for the reference car's channels: a cell shows the board's dashes.
- **Consumption in watts**: not a channel the build can resolve yet (`todo/dash/23`).
- **Saving into a claude.ai artifact's store**, and Google Fonts: the page talks to its own
  server only (a Content-Security-Policy of `default-src 'self'`).
- **A "save to dash.toml" button**: a complete file means rewriting a commented, hand-written
  TOML; a fragment written beside it is not a `dash.toml`. The page shows the blocks to copy.

## Notes from the session that made it

- On the reference car the catalog is 6,583 rows, 3,607 of them numeric — the same as the
  research tool's, built in about 3 s on an M4.
- ODIS rows carry no range, so a chart's scale is set by hand.
- An alarm raises the first values page that shows its channel; the editor says which, and warns
  when no values page shows an alarmed channel (the build refuses that).
