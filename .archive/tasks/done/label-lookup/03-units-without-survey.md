# label-lookup/03 — the car's units, learned without a survey; `dev survey` removed

## What the owner asked (2026-09-28)

- Asked whether `dev survey` may be widened with the VCDS lists: «survey такая же паразитная команда,
  как и calibrate. юзер не должен задумываться об этом».
- Asked whether to copy the parked survey into `~/.vagcan/cars/<VIN>/survey.jsonl` so `setup`'s
  step 5 has units on the owner's machine: «нет».
- Told that `dev dash build` fails without a survey and the firmware's `build.rs` panics: «Ну да.
  А зачем ему survey?» — it needs only the car's units (what each said about itself), and the
  survey's "asked but silent" check.
- Asked whether to keep `dev survey` under `dev` as a workshop tool or remove it entirely like
  `calibrate` (losing `dev dash build`'s answered check, the blind sweep, `faults --from` and
  `--diff`): **«б» — remove it entirely.**

So: nobody runs a survey for anything. The tool learns which control units a car has from the
commands that already identify them, and reads their channels by itself.

## Why a list of units is needed at all

- A VCDS installation cannot be read for every unit up front: `UDS_EV` holds 12,285 unit files, each
  with its own section keys. Measured 2026-09-28 on the owner's install (English 26.3, M4): the keys
  for one car's 15 units took **4 min 56 s** (2,368 CPU-s), with the shared tables' keys cached.
  Only the car can say which units it has.
- `watch` and `measure` already identify every unit live (`vag_cli_core::units::identify`: the
  gateway's installation list, then `F187`/`F197`/`F19E`/`F1A2` per unit, and `read_vin`), and write
  nothing down. On the owner's machine `~/.vagcan/cars/` does not exist.

## Design

1. **The record.** `~/.vagcan/cars/<VIN>/units.json` (path owned by `datadir`): per unit its request
   id, `F187`, `F19E`, `F1A2`, `F197`, as `units::identify` returned them. Merged by request id: a
   unit identified now replaces its entry; a unit not seen this time (asleep) is kept. Written
   atomically (a temporary file renamed over it). Identity only: no sweep, no faults, no request that
   is not already made.
2. **Who writes it.** `watch`, `measure` (the run and its setup wizard) and `vagcan units` (which
   switches to `units::identify` and `read_vin`, so it reads `F19E`/`F1A2` too — the command a dash
   owner runs once). Silently; a write that fails is one line, never a stop.
3. **Channels for units nothing has read yet.** A core function reads the VCDS registry rows (step
   5's reading, moved from `vag-cli-diag`'s `setup/registry.rs` into `vag-cli-core`, since `measure`
   stands on core alone) for the units of a car that no ODIS variant describes and that have not
   been tried with this installation — when the project was set up from a VCDS installation that is
   still on disk. Called by `watch` and `measure` after identifying, and by `dev dash build` after
   reading the record (offline: the read needs the units' identities and the installation, not the
   car). Never by the firmware's `build.rs`.
   - One line and a spinner: how many units, "the first time only", about how long.
   - Ctrl-C ends the command; every key found is already saved (`Keys::save` after each search), so
     the next run continues.
   - The installation gone: one line, and the command runs with what it has.
   - Which units were tried with which installation, and the outcome (read / no file / shifted /
     key not found), is recorded per project, so a unit VCDS has no file for is not retried.
   - It writes every known unit's rows at once (the last installation read replaces the previous
     one's rows, `Replace::Kind`), so cached keys make the known units a few seconds.
4. **`setup`'s step 5** reads the units of every car recorded on this machine. With none: `not yet —
   no car recorded on this machine; `vagcan watch`, `measure` or `units --identify` with the car
   reads the channels of its units`. No other instruction.
5. **`dev dash build`** (and `build.rs`, and `dev recording dash`) take the car's units from
   `units.json`. `dash.toml`'s `survey =` key goes: an input that still has it is refused with what to
   do (delete the line; the units come from `units.json`, written by `vagcan units` or `vagcan
   watch`). With no record: the refusal says to connect to the car once with `vagcan units`. The
   answered check goes with the survey: every channel the project describes is `declared`.
6. **`watch`** takes its known identities from `units.json`. `--survey` goes, and `--replay` takes
   identities from the car's record when the recording names the car. *(The first half is
   superseded by review round 2's B1: `watch` asks every unit every run and never takes an
   identity from the record; the record is for the commands that cannot ask the car.)*
7. **`dev survey` is removed entirely**, with everything only it uses: the sweep, `--only`,
   `--blind`, `--diff`, `merge_survey`, `unswept_notice`, `declared_for_unit` and `declared.rs` /
   `anomaly.rs` where nothing else uses them, the survey's refusals over the board, `faults --from`,
   `poll::identities_from_survey` / `answered_from_survey` / `Answered` / `with_survey`,
   `datadir::survey_cache` / `SURVEY_FILE`. The dead-code check decides what else goes — and a
   function it reports that was written to be called is a missing call site, not dead code.
   **Guards stay:** `require_stationary` (still used by `faults` and `units --identify <unit>`'s
   identification sweep), the allowlist, the board's own BLE sweep limit. `survey` joins
   `calibrate` on the top-level denylist test.
8. **No text names `dev survey`.** Every message, help text and document in the map below. What a
   user types to get channels: `vagcan setup <source>`, then `watch`, `measure` or
   `units --identify` with the car (or `dev dash build`, offline, for a car already recorded).

## Map (2026-09-28, `feat/remove-calibrate`)

Readers of `survey.jsonl`:
- `vag-cli-core/src/dash.rs:2421-2439` `resolve_for_car` — identities + answered; absent → hard error
  "no survey at … — run vagcan dev survey". Reached from `vag-cli-diag/src/dash.rs:73` (`dev dash
  build`), `crates/dash/vag-dash-fw/build.rs:47` (panics), `vag-cli-diag/src/dashreplay/mod.rs:33`.
- `vag-cli-diag/src/setup/registry.rs:36-51` `surveyed_units` — `(F19E, F1A2)` of every car.
- `vag-cli-diag/src/watch/mod.rs:2551-2598` — identities as `known`, answered; absent → soft.
- `watch/mod.rs:1860-1907` (`--replay --survey`), `faults.rs:318-393` (`faults --from`),
  `survey.rs:237-279` (`--diff`) — explicit paths.

Live identification: `units::identify` + `read_vin` in `watch` (`watch/mod.rs:2551,2582`) and
`measure` (`lib.rs:987`, `setup.rs:781`); `vagcan units` has its own `F187`/`F197` loop
(`vag-cli/src/main.rs:1048`); `info`, `faults` read less. None writes to disk.

User-facing text naming `dev survey` (53 places in 15 files): `vag-cli/src/main.rs` (272, 297-299,
364-380, 862-871), `vag-cli-diag/src/dash.rs:31`, `recording.rs:52`, `declared.rs:187,241`,
`anomaly.rs:133`, `watch/mod.rs:2454,2464`, `survey.rs`, `vag-cli-core/src/missing.rs` (13, 51-58,
128-140, 259-264), `vag-cli-measure/src/messages.rs:34-38,133`, `vag-cli-measure/src/setup.rs:
1250-1258`, `setup/registry.rs:112-119`; `README.md` (38, 48, 73, 171-178, 234), `ARCHITECTURE.md`
(23-56, 297-347, 406-460), `docs/dash/dash-toml.md` (the `survey =` key and every refusal naming
`dev survey`); `CLAUDE.md` (the sweep rule names `survey` as the guarded example; the `dev …`
list). `research/`, `todo/` logs and `.archive/` keep old spellings on purpose.

## Must not

- Sweep anything, or add a request: recording reuses what `units::identify` already reads.
  **One exception, added by the controller on 2026-09-28:** the gateway (`0x710`) is now
  asked the four identification identifiers (`F187`/`F197`/`F19E`/`F1A2`) on every run, as
  the survey always did — without them the gateway's channels are never read. The owner was
  told; to undo it, drop the gateway from `to_identify`'s walk (`vag_uds_client::gateway::
  walk_order` puts it after the powertrain) and its channels are then never read.
- Read the whole `UDS_EV` up front, or search keys inside the firmware's `build.rs`.
- Name a unit, an identifier or a car in code.
- Write into the checkout. Everything read at run time is under `~/.vagcan` (`datadir`).
- Remove a guard.

## Gates

The workspace gates; `research/dash/host`; the firmware's clippy on both builds and `ram-budget.sh`
(`build.rs` changes; no RAM change expected — say so); the private-data tests with
`VAGCAN_ROD_KEYS`. On the owner's data: a scratch `HOME` with a VCDS-only project and no `cars/` —
`setup` says `not yet`; a scripted car (the capture/replay transport) identifies the 15 units,
`units.json` is written, the channels of 14 units are read once, and a second run reads nothing;
`dev dash build` builds the owner's `dash.toml` (without its `survey =` line) from that record.

## Done when

A VCDS-only owner types `vagcan setup <VCDS>` and then `vagcan watch` with the car, and gets the
channels VCDS lists for every unit it has a file for, with no other command; `dev dash build` builds
from what `watch` or `units` recorded; `dev survey` does not exist and no text names it.

## Built (2026-09-28, `feat/units-without-survey`)

Phase 1 — the mechanism:

- `~/.vagcan/cars/<VIN>/units.json` (`vag_cli_core::units::record`): merged by request id,
  replaced whole through `datadir::replace_file` (a temporary file renamed over it; a reader
  never sees a torn file — tested with four concurrent writers). Written silently by `watch`,
  `measure`, its setup wizard and `units --identify`; a write that fails is one line.
- `vag_cli_core::registry`: step 5's reading, moved out of `setup` with every note it had; a
  per-project `registry.json` (which installation, which units, `read` / `no file` / `shifted` /
  `key not found` …), written after the rows and never before; `ensure()` — for the car in
  front of `watch`, `measure` or `dev dash build`, the units no ODIS variant describes and the
  installation has not been tried for, read over every unit recorded on this machine at once.
  `setup` records the installation even with no car yet, so the first `watch` knows where to read.
- The dash build resolves against the record; `dash.toml`'s `survey =` is refused with what to
  do; the answered check is gone (every channel the project describes is `declared`).
- The gateway identifies itself too (`to_identify` follows `gateway::walk_order`): four more
  `22` reads, every run (since round 2 every unit is asked every run).

Phase 2 — the removal: `dev survey` and everything only it used (`survey.rs`, `declared.rs`,
`faults --from`, `watch --survey`, `plan::Answered` and the survey readers,
`datadir::survey_cache`, the generator's `answered`, `scan_dids_fast`, `hex_packed`,
`extracted::declared_for_unit`); `watch --replay --vin` takes a replay's tabs from a car's
record; no text names `dev survey` outside dated history.

Checked on the owner's data, scratch `HOME`: `setup ~/vcds-en --project SK37X-vcds` with no
`cars/` says `not yet`; with a `units.json` of the reference car's 15 units, `dev dash build`
reads 14 units' channels once (5,314 channels; `EV_BCMMQB` has no file), builds the same
`plan.json` as from the survey, and a second run reads nothing. `watch` against a scripted car
records what it identified (`crates/cli/vag-cli-diag/tests/watch_records_units.rs`).

Firmware RAM, measured with `ram-budget.sh` before and after phase 2 (the `build.rs` change):
static 139,500 B with BLE, 130,048 B without — no change.

Not built: the record holds identities only, so nothing says which identifiers a car answers; a
unit nothing describes has nothing to show, and the summary says so.

## Review round 2 (2026-09-28, four reviewers on `09d50d9`: not ready)

Fixed the same day, on the branch:

- **A recorded unit was never re-identified** (`watch` passed the record as `known`), and the
  record replaced an entry whole, so a missed `F19E` deadline left a unit without its ODX name
  for good. Now every run asks every unit — the gateway's four reads included, every run — and
  the record merges field by field (a value is never replaced with nothing) and is written only
  when it changes (a rewrite re-ran the firmware's `build.rs`).
- **`units --identify` reads the channels** it recorded, like `watch` and `measure`; the dash
  build (`dev dash build`, the firmware's `build.rs`, `dev recording dash`) **refuses** a car
  with units whose channels a VCDS read has yet to bring, naming `dev dash build` — which reads
  them offline, after parsing `dash.toml` and the record and before building. The docs say
  when it is a step before the firmware build.
- **Ctrl-C** during the registry read: `dev dash build` and `setup`'s step 5 run the read on
  the blocking pool, so `main`'s `select!` sees the signal.
- **A project set up before `registry.json`** is read from the installation its `sources.json`
  names (absolute, holding `UDS_EV`); one it cannot use says to re-run `setup`; the owner's
  SK37X (relative `vendor/vcds-en`, every unit ODIS-described) stays silent.
- One unreadable `units.json` no longer aborts `setup` (a step that did not complete, in the
  report), stops another car's command, or fails `watch --replay`; a read that would drop that
  car's rows is skipped and says so. `ensure` prints a line per unit with the cause, "unit N of
  M" on the spinner, and what to type when the installation is gone. `watch`'s summary is
  singular for one unit and carries the logged cause. The texts that said "any car command"
  name `watch`, `measure`, `units --identify` and `dev dash build`; the stale help and docs
  (the `--range`/`--blind` reference, "two fields", "three identifiers", the anomaly notice's
  "every unit", the standard-OBD "no record" clause, `datadir`'s `reports/`) are corrected.

## Left

- **VCDS's fill for units ODIS describes, on a car recorded after a pair setup.** `ensure`
  skips ODIS-described units — reading them would be minutes of key search for units that have
  channels already — so a car first seen after a pair (ODIS + VCDS) setup never gets VCDS's
  fill for the fields ODIS lacks on those units. A `setup` run after the car is recorded does
  fill them (its step 5 reads every recorded unit). Not now: the cost is minutes per car for
  fields ODIS lacks, and the owner's `dash.toml` needs none of them.
- **Callers that only tests have now.** `AsyncUdsClient::read_data_by_identifiers` and
  `anomaly::Monitor::seed`/`heard`/`silent_span` have no caller outside tests, and the witness
  path of `scan::Guard` is unused (`units --identify <unit>` runs with `witness: None`). Whether
  the one sweep left should read a witness is a new request, the owner's call; until it is made
  they stay, as written to be called.
