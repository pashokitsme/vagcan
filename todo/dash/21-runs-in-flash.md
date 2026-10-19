# dash / 21 — runs kept in flash, read back over BLE

**Subsystem:** dash · **Crates:** `vag-dash-fw` (store, commands), `vag-dash-render` (the
stopwatch hands a run out), `vag-dash-cfg` (reading) · **Needs the car:** no, the bench does

**State (2026-09-27):** recorded, not designed. After dash/19 (PR #12) merges, since it builds
on the stopwatch and on the input backends that PR adds.

## What the owner asked (2026-09-27)

A review of PR #12 found that the no-BLE build has no `save`. There, a run can stay unwritten.
The owner: "We can save on LIMIT (or the matching button). Then read the reports out over BLE."
Then: "So we need to be able to read the flash from the dash." Then: "Just write it down as a
task."

## Why

- **Nothing on the host reads a run.** `config.last_run` sits in the settings record. `dashcfg`
  and the host tools never show it.
- **Leaving early loses the run.** Leave the stopwatch before the car stands still and the run
  is lost at power-off unless you send `save`. The no-BLE build has no `save`.
- **Only the last run is kept.** The next one overwrites it.
- **A close the driver did not choose.** Found in PR #12's third review (2026-09-27): finish a
  run, then switch cruise on. The lever closes the stopwatch, the speed is no longer read, no
  standstill writes the run, and it is lost at ignition off — unless `save` (BLE build only).
  The close's log line does not mention the run. Whatever this task settles for "when LIMIT
  writes" should cover this close too.

## Open questions

The owner was asked these on 2026-09-27 and set them aside for later.

1. **How many runs.** Either the last one only, as now, or a ring of the last N (e.g. 16).
   - A ring would go in its own flash sector, not in the settings record, so it costs no RAM
     and survives a settings `erase`.
   - The board has no clock. A run can carry a boot counter and the uptime, not a date.
2. **When LIMIT writes.**
   - **At once.** It is an explicit act, like `save`. The sector erase may then happen while
     driving, and the panel stalls for tens of ms.
   - **Mark it, write at the next standstill.** The stopwatch's speed is polled at its rate only
     while the stopwatch is up, so it would have to stay polled in the background until the car
     stops. Power-off before that loses the run.
3. **Reading.**
   - A `dashcfg` command (`runs`) over BLE.
   - Over USB for the no-BLE build? Its console has no settings commands today.
   - Clearing the history: a board-local command. It is not UDS and touches no car.

## The settings record (schema 3, 2026-10-19)

Architectural review, owner approved the fix 2026-10-19. The record was brightness, active
page, a copy of the plan's pages and the run. Nothing on the board edits pages, and a stored
list that was not the plan's was refused, so every `dash.toml` change reset brightness and the
page to the defaults and left `unsaved=1`.

- **Schema 3** (`vag-dash-core/src/schema.rs`): brightness, active page, the run, and the id of
  the plan it was saved against. No pages: the board shows the plan's.
- **The plan id** is FNV-1a 64 over the generated `plan.rs` short of its last lines
  (`vag_cli_core::dash::plan_id`). `plan.rs` ends with `PLAN_ID`, `plan.json` carries `id`,
  `vagcan dev dash build` prints it. A generator that spells the source another way changes
  every id; the cost is one note at boot.
- **On boot** (`schema::fit`): same id, all as stored. Another id, or a v1/v2 record (no id):
  brightness and the run kept, the page kept where the plan has it and the first where it does
  not. Not `unsaved`: the next boot makes the same of the same record. The next write names this
  plan. A note on the USB log says which.
- **Where a host sees it**: the HelloReply (`version`, a NUL, 16 hex digits; an older host reads
  the tail as part of the version and still parses), so `vagcan devices` shows `plan …`; and
  `state`'s `plan=`, which `dashcfg` shows. `dashsim` shows neither image nor version, so no plan.
- **Not done**: no host compares the board's id with the plan on disk for that VIN. `vagcan
  devices` has no VIN, and the bus that opens the board learns the VIN only later; a check would
  need its own plumbing. A candidate when someone asks for it.
- RAM (`ram-budget.sh`, empty plan): statics 140996 → 140872 B with BLE, 131536 → 131412 B
  without — the record lost its eight pages of eight cells.

## Constraints to keep

- **RAM** (`CLAUDE.md` "Firmware RAM"): the history lives in flash and is read on demand;
  report the static delta.
- **Store rules from PR #12's review:**
  - a record from a newer image keeps its generation and slot;
  - an automatic write never overwrites a record this image cannot read.
- **Reading is a configuration command on the board's own link, not UDS.** The allowlist is
  untouched.
