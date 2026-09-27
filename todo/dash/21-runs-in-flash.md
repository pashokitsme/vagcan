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
   - **Mark it, write at the next standstill.** The speed channel (`380B`) is polled only while
     the stopwatch is up, so it would have to stay polled in the background until the car
     stops. Power-off before that loses the run.
3. **Reading.**
   - A `dashcfg` command (`runs`) over BLE.
   - Over USB for the no-BLE build? Its console has no settings commands today.
   - Clearing the history: a board-local command. It is not UDS and touches no car.

## Constraints to keep

- **RAM** (`CLAUDE.md` "Firmware RAM"): the history lives in flash and is read on demand;
  report the static delta.
- **Store rules from PR #12's review:**
  - a record from a newer image keeps its generation and slot;
  - an automatic write never overwrites a record this image cannot read.
- **Reading is a configuration command on the board's own link, not UDS.** The allowlist is
  untouched.
