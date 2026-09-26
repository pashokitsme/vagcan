# dash / 20 — the fault count on the panel

**Subsystem:** dash · **Crates:** `vag-uds-client` (`faultcount`, `gateway`, `address`),
`vag-dash-render` (the badge), `vag-dash-fw` (wiring) · **Needs the car:** for the checks at the end

**State (2026-09-26):** approved by the owner. Phase 1 on `feat/fault-count`: the count and
the badge, pure and tested on the laptop. Phase 2, the firmware wiring, starts when the
controller says.

## What the owner asked (2026-09-26)

"Add fault reading on the board. The board gets the list of units at start anyway. Read the
faults 10 s after start and after reading the units, and show it as a number somewhere in a
corner of the screen (it may overlap the rest of the UI). Just a number and a triangular
warning icon." Answers to the questions that followed:

- **All units of the car**, from the gateway's installation list, as `vagcan units` does. The
  board today identifies only its plan's units, so it reads the list too.
- **The number is all stored codes.** If any code is failing now, the icon is highlighted —
  **inverted, not blinking** (blinking is an alarm's).
- **Read once, 10 s after boot**, or once the plan's units are identified, whichever is later.
- **Hidden at 0.**

## What is read — the same as `vagcan faults`

| step | request | unit | why it is not car data |
|---|---|---|---|
| 1 | `22 2A26` | the gateway, `710` → `77A` | `gateway::GATEWAY` (VW's diagnostic address `19`), `gateway::INSTALLATION_LIST`, the bitmap decoder `gateway::decode_installation_list` |
| 2 | `19 02 08` | each unit of `gateway::walk_order`: `7E0`, `7E1`, `710`, then the list | ISO 15765-4's first two servers and the gateway, which the list never holds (`gateway::NOT_LISTED`); each addressed by its block's rule, `address::UnitAddress::from_request` (ISO `+8`, VW `+6A`) |

- **Stored** = a code with `confirmedDTC` (bit 3, `dtc::CONFIRMED`); **failing now** = stored
  and `testFailed` (bit 0, `dtc::FAILED_NOW`) — ISO 14229-1 table D.1, exactly what `vagcan
  faults` prints as its total and its "failing now" (it filters bit 3, then counts bit 0 among
  those).
- **Mask `08`, not `FF`.** ISO 14229-1 reports a code when `status & mask ≠ 0`, so `08` returns
  the stored codes and nothing else; `vagcan faults` asks `FF` because it also prints everything
  with `--all`. On the reference car `FF` makes the body control module answer 508 codes
  (2 KB, ~290 frames) for 3 stored. The bits are still checked on every code that comes back,
  so a unit that ignores the mask is counted the same. The car check below compares the two.
- **No session.** `vagcan faults` enters one only with `--extended` (`10 03`, after
  `safety::require_stationary`); its default run reads every unit in the session it is in. The
  board never sends `10`, so **no unit is affected**, and `19` does not change how a unit
  behaves: the count may run while the car moves.
- `walk_order` is now one function in `vag-uds-client`, used by `faults`, `dev survey` and the
  board, so the three read the same units by construction.

**Differences from the laptop**, on purpose: no `F197`/`02BD`/`F19E`/extended data (the badge
needs a number, not text); a gateway that gives no list ends the count — no badge, a log line —
where `vagcan faults` falls back to the three units it cannot list.

A unit that does not answer, refuses (NRC) or answers something that does not parse is **not
counted** and named on USB; a listed id in neither block (none on the reference car) is named
and not asked. The count is of the units that answered.

## The count: `vag_uds_client::faultcount` (phase 1, done)

Sans-IO, as `schedule` is — no clock, no bus, no allocation beyond the walk:

```rust
let mut count = FaultCount::new();
while let Step::Ask { unit, pdu } = count.next() {
    count.answered(/* schedule::Answer for that exchange */);
}
let Step::Done(outcome) = count.next() else { unreachable!() };
// Outcome::NoList(Why) | Outcome::Counted(Tally { read: Vec<UnitTally>, failed: Vec<Failed> })
// Tally::stored(), ::failing_now(), ::units_read(); Failed { request, why: Why }
```

Why sans-IO and not a `UnitLink`: the board's bus is the planner inside `can_task`, not a
`UnitLink`; a `UnitLink` on the board would be a second task with a channel back from
`can_task`. The machine is fed from the delivery routing `can_task` already has (the part
checks are routed the same way, by `ReqId`), and its tests drive it with a fake car.

**`no_std`:** `address.rs` was host-only as a whole; now only its short-number table (the
filesystem, a process-wide lock) is `#[cfg(feature = "std")]`, and the rule builds for the
board. Nothing on the host changed.

**Board RAM** (riscv32imc, measured with `size_of`): `FaultCount` 40 B, living in `can_task`'s
future (the embassy task arena), not static; on the heap during a count: the walk 4 B a unit,
12 B an answering unit, 4 B a failing one, one answer PDU at a time (`3 + 4 × codes` bytes) —
under 1 KB for 18 units, freed at the end but for the `Tally` if the shell keeps it. `Faults`
8 B; `Board` grew from 16 to 24 B (a stack local per frame).

## The badge: `vag_dash_render` (phase 1, done)

`Board::faults: Option<Faults>`, `Faults { stored: u32, failing_now: bool }`; drawn by
`draw_with` last, over the page.

- **Corner: bottom-right, in the link icons' column.** That column is the board's own: the
  USB/BLE icons stand at its top (PR #3), and the chart's trace ends before it whether or not a
  host is connected, so on a chart page the badge covers nothing. The top-right is the icons';
  the left corners hold the first cell's label and unit and the chart's number.
- **Geometry on 256×64:** the triangle 7×7 at `(245, 55)`, the icons' axis, `ICON_TOP` (2) up
  from the floor and `ICON_RIGHT` (4) in; the count in the unit face (`4×6`, 3×5 digits) over
  it, `ICON_GAP` (2) rows up, right-aligned to the column — a longer count grows left. With
  one-pixel ground round both, the box is `244..252 × 47..62` for one or two digits
  (`badge_box`). Two icons end at row 22: 25 rows apart.
- **The glyph:** a filled triangle with the `!` cut out (`render::TRIANGLE`).
- **Over the page:** the box is painted with the ground first, so the corner is the same
  picture over any page, an alarm's lit column included.
- **Failing now:** the whole badge inverted — lit ground, dark triangle and count — as an alarm
  inverts its whole cell. Steady.
- **Hidden** with no count, or a count of 0. **Not on the adapter screen**, which draws no link
  icons either.
- Laid out for the board's 64 rows, like the icons; on 32 rows with two hosts it would meet the
  second icon (no caller draws either there).

## Phase 2 — the firmware (after the controller's word)

- **Start:** once `ms() ≥ 10 000` **and** every plan unit's first part check has come back
  (matched, mismatch or silent — none still pending); a plan with no units starts at 10 s. Not
  in adapter mode: the count waits for the panel to come back. Once per boot, no retry.
- **Through the planner, as the part checks are:** each `Step::Ask` is
  `planner.exchange(now, Class::Background, unit, pdu)`; `PanelReads::take` (or a sibling
  beside it) recognises the `Delivery::Raw` by its `ReqId`, feeds `answered`, queues the next.
  One of ours in flight at a time; the planner serialises it with the panel and any host, and
  the board's guard is not involved (the board's own reads never pass it; the planner still
  refuses anything off the allowlist). An exchange given up for adapter mode is a `BusError`
  for that unit.
- **Published** in an 8-byte static the panel task reads into `Board::faults` each frame.
- **USB log:** the start; each unit with codes (`faults: 7E0 2 stored, 1 failing now`); each
  unit not counted (`faults: 7E1 not counted — no answer`); the end (`faults: 9 stored, 1
  failing now; 17 of 18 units answered in 1.4 s`) or `faults: the gateway gave no list (no
  answer) — no badge`.
- **`dashcfg`'s state line:** ` faults=9/1` (stored/failing now) once counted — `dashcfg`
  ignores keys it does not know, so this is cheap; to check against `UART_MTU` then.
- Docs: `README.md` (Features, the dash), `ARCHITECTURE.md` (the dash), this file's state.

## How long it takes

Per exchange with the gateway in the path the board measured ≈4 ms (`dash/14` §6a); a fault
read may take longer (units answer `7F 19 78` while they search), say 5–50 ms. The planner
spaces sends 10 ms apart (100/s ceiling), the panel keeps its 25/s floor, and `Background`
takes the slots left — a four-cell page with the four alarm rules leaves most of them. So:
**19 exchanges ≈ 0.3–1 s** when every unit answers, **+0.5 s per silent unit** (the board's
`RESPONSE_TIMEOUT`, during which the panel's next read waits too). Worst case, a page asking
for the whole ceiling: the starvation rule sends each `Background` item within 5 s, ≈ 95 s.
The laptop's ~50 s over BLE also read `F197`, `02BD`, `F19E`, `F1A2` and extended data per code,
each a BLE round trip.

## Tests

- `faultcount` (10): the gateway first and idempotent; every unit of the walk asked `19 02 08`
  in order, by its block's rule; only `22` once and `19` — no `10`; stored = confirmed, failing
  now = confirmed and failing, a unit that ignored the mask counted right; a unit that is
  silent, refuses, errors, leaks a `78` or answers the wrong subfunction is skipped and named;
  a gateway with no list (silence, bus error, NRC, another identifier, too short) asks no unit;
  an empty list still reads the three; an id outside both blocks named, not asked; an answer
  after the end changes nothing.
- `gateway` (2, moved from `survey`): the walk covers the three the list cannot hold; a unit
  listed twice is walked once.
- `render` (8): the triangle pixel for pixel at the foot of the icon column, the count over it
  right-aligned, nothing outside the box moved; hidden at 0 and with no count; failing now
  swaps every pixel of the box and nothing else; the corner is the same over a blank panel,
  an alarm page, a values page and a chart; the icons untouched and 25 rows above; the chart's
  trace untouched; a longer count grows left; the adapter screen draws none.

## Car checks (phase 2)

1. `vagcan faults` and the board on the same car, same ignition cycle: the board's total and
   failing-now equal `vagcan faults`' last line. A mismatch means a unit honours mask `08`
   differently — then ask `FF` and filter, as the laptop does.
2. The USB log's time for the count; the panel keeps changing through it.
3. The units named as not counted, against `vagcan units`' list (`776`/`777` are the suspects).
4. A BLE `vagcan info` started during the count completes.
5. The inverted badge only if the car has a code failing now — nothing is provoked to see it.
