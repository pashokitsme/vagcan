# dash / 20 — the fault count on the panel

**Subsystem:** dash · **Crates:** `vag-uds-client` (`faultcount`, `gateway`, `address`),
`vag-dash-render` (the badge), `vag-dash-fw` (wiring) · **Needs the car:** for the checks at the end

**State (2026-09-27):** phase 1 reviewed; phase 2, the firmware wiring, built on
`feat/fault-count` with the owner's four answers (below), not yet reviewed. `master` through
PR #13 merged in (`a90a539`). Nothing of it has run on the bench or the car — "Car checks".

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
  Decided by the controller on review (2026-09-26): "identified" means **at least one plan
  unit has answered**; while none has, the count waits — a board on permanent +12 V with the
  ignition off still shows the badge once the ignition comes on.
- **Hidden at 0.**

## The owner's answers (2026-09-27)

Asked after the 2026-09-26 review, with the badge's previews (`dashsim --preview`) and a
mock-up of the states:

1. **The unit list is read from the gateway at start**, as asked on 2026-09-26. `dash/README.md`'s
   "the board resolves nothing" is amended: the board resolves no *label data*; the gateway's
   installation list is a protocol read, bounded by `MAX_UNITS` and by VW's block.
2. **`MAX_UNITS` is 64**, the BLE guard's `guard::MAX_UNITS`: one number on the board for how
   many units it may touch. A list with every bit set is 150 addressable ids and still refused.
3. **A count's exchange has its own deadline, 2 s, `78`s included**; past it the unit is not
   counted (`no answer in 2 s`). The board's other exchanges keep `PENDING_DEADLINE` (10 s).
   **While the stopwatch is up no count exchange starts**; the count resumes when it closes.
4. **`?` when the count failed** — the gateway gave no list, or the walk is over `MAX_UNITS`:
   the triangle with `?` for the number. Before the count and at zero, nothing, as before.
   The count is not repeated (once per boot, and the board is powered with the ignition).

The badge sits under the rightmost cell's value on every page, the stopwatch's included: in
the previews it hides no pixel of any page (the values' ink ends at row 46, the badge's ground
starts at 47), and a two-digit count inverted stands close to a wide time (`18.4`) but clear of it.

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
needs a number, not text); a gateway that gives no list ends the count — a `?` badge, a log
line — where `vagcan faults` falls back to the three units it cannot list.

A unit that does not answer, refuses (NRC) or answers something that does not parse is **not
counted** and named on USB. The count is of the units that answered.

**Three guards the board has and the laptop does not** (review, 2026-09-26) — the board runs
this at every boot with nobody watching:

- **Only VW's block is decoded:** the first 24 bytes of the bitmap (`gateway::VW_BLOCK_BYTES`,
  `0x700..=0x7BF`), whatever the answer's length. A whole 4095-byte answer decoded would be
  32,736 ids, over 130 KB of heap on a 72 KB board — a panic, and a reset loop since it reruns
  every boot. Bits past the block are counted (`Tally::unaddressable`, logged), never
  decoded. `vagcan units`, `faults` and `dev survey` decode the whole answer; the reference
  car's is 32 bytes with nothing past byte 24.
- **A walk of more than 64 units is refused** (`faultcount::MAX_UNITS`, the BLE guard's
  `guard::MAX_UNITS` itself — answer 2; `Outcome::TooMany`): a `?` badge, a log line. A car
  lists 15–20 (the reference car: 15, 18 with the three); past 64 it is a sweep of the block
  with nobody watching, which `CLAUDE.md` guards as it guards `survey`.
- **A listed id that shares an id with a unit already walked is skipped and named**
  (`Why::SharedId`): an id that is another walked unit's answer id, whose own answer id is
  another walked unit's request id, or which answers on the id another walked unit answers
  on. The reference car lists `776` (= `70C`'s answer id; its own answer id would be `7E0`,
  the engine's request) and `777` (answer id `7E1`, the gearbox's); a listed `77E` or `77F`
  would answer on `7E8`/`7E9`, the engine's and the gearbox's own answer ids (review round 2).
  Walked in order, a unit comes before the id it answers on, so the first of a pair is kept. **The laptop's `faults` asks both today**, sending `19 02 FF` on `776` and
  listening on `7E0` — a separate question for later, not this branch.

## The count: `vag_uds_client::faultcount` (phase 1, done)

Sans-IO, as `schedule` is — no clock, no bus, no allocation beyond the walk and the tally.
Answers are read where they lie (`pdu::classify_response_ref`, `pdu::dtc_records`): nothing
copies an answer, and a unit's codes are counted as they are read, never collected.

```rust
let mut count = FaultCount::new();
while let Step::Ask { unit, pdu } = count.next() {
    count.answered(/* schedule::Answer for that exchange */);
}
let Step::Done(outcome) = count.next() else { unreachable!() };
// Outcome::NoList(Why) | Outcome::TooMany { units } | Outcome::Counted(Tally)
// Tally { read: Vec<UnitTally>, failed: Vec<Failed>, unaddressable: u32 }
// Tally::stored(), ::failing_now(), ::units_read(); Failed { request, why: Why }
```

Why sans-IO and not a `UnitLink`: the board's bus is the planner inside `can_task`, not a
`UnitLink`; a `UnitLink` on the board would be a second task with a channel back from
`can_task`. The machine is fed from the delivery routing `can_task` already has (the part
checks are routed the same way, by `ReqId`), and its tests drive it with a fake car.

**`no_std`:** `address.rs` was host-only as a whole; now only its short-number table (the
filesystem, a process-wide lock) is `#[cfg(feature = "std")]`, and the rule builds for the
board. Nothing on the host changed.

**Board RAM** (riscv32imc, measured with `size_of`): `FaultCount` 44 B — phase 2 keeps it inside
the board's `Count`, a 104-byte static in `.bss`, not in `can_task`'s future (the embassy task
arena); `Tally` and `Outcome` 28 B. On the heap during a
count: the walk 4 B a unit, 12 B an answering unit, 4 B a failing one — under 1 KB for 18 units,
and bounded whatever the car answers: the list decodes to 192 ids at most (384 B) and a walk
past 64 units is refused. **Peak:** one answer, held once by its caller — up to 4095 B, the
longest ISO-TP carries — plus what reading it allocates, measured in the tests with a counting
allocator: 48 B to count a 4095-byte `19 02` answer (the tally's first entry), 42 B to read a
4095-byte list (review round 2: it was 8,234 B, a copy of the answer and a list of its codes).
Freed at the end but for the `Tally` if the shell keeps it. `Faults` 8 B; `Board` grew from 16 to 24 B (a stack
local per frame). Phase 2's RAM: "Phase 2", below.

## The badge: `vag_dash_render` (phase 1, done)

`Board::faults: Option<Faults>`, `Faults::Counted { stored: u32, failing_now: bool }` or
`Faults::Failed` (phase 2, answer 4); drawn by `draw_with` last, over the page.

- **Corner: bottom-right, in the link icons' column.** That column is the board's own: the
  USB/BLE icons stand at its top (PR #3), and the chart's trace ends before it whether or not a
  host is connected, so on a chart page a count up to 99 covers nothing of the chart. The
  top-right is the icons'; the left corners hold the first cell's label and unit and the
  chart's number.
- **Geometry on 256×64:** the triangle 7×7 at `(245, 55)`, the icons' axis, `ICON_TOP` (2) up
  from the floor and `ICON_RIGHT` (4) in; the count in the unit face (`4×6`, 3×5 digits) over
  it, `ICON_GAP` (2) rows up, right-aligned to the column — a longer count grows left. With
  one-pixel ground round both, the box is `244..252 × 47..62` for one or two digits
  (`badge_box`). Two icons end at row 22: 25 rows apart.
- **Past 99 it overlaps the chart:** three digits are 11 columns of ink in a 7-column
  column, so the widest cover the trace's last column (240) — no more; each further digit
  four more columns. Not clipped (a count with a digit cut off is another number) and not
  capped at `99+` (also three glyphs, the same width). A hundred stored codes is no car seen
  so far; the reference car has nine.
- **The glyph:** a filled triangle with the `!` cut out (`render::TRIANGLE`).
- **Over the page:** the box is painted with the ground first, so the corner is the same
  picture over any page, an alarm's lit column included.
- **Failing now:** the whole badge inverted — lit ground, dark triangle and count — as an alarm
  inverts its whole cell. Steady.
- **Hidden** with no count, or a count of 0. **Not on the adapter screen**, which draws no link
  icons either.
- **`?` for a count that failed** (`Faults::Failed`, answer 4): the triangle with the unit
  face's `?` where the number goes, in the box of a one-digit count, never inverted — nothing
  says a code fails now.
- Laid out for the board's 64 rows, like the icons; on 32 rows with two hosts it would meet the
  second icon (no caller draws either there).

## Phase 2 — the firmware (built 2026-09-27, with the owner's answers)

`vag_dash_fw::faults` (`crates/dash/vag-dash-fw/src/faults.rs`) is the board's shell round
`faultcount`, pure like `saving.rs`: `research/dash/host/tests/fault_count.rs` compiles it as it
is and drives it through a real `Planner::new(Budget::board())` against a fake car. `dash.rs`
holds one `Count` (a `StaticCell` in `can_task`, in `.bss`, not the arena), gives it a turn
(`PanelReads::count_step`) as the last thing before `planner.due()`, and offers it every
`Delivery::Raw` in `PanelReads::take`.

- **Start:** once `ms() ≥ START_MS` (10 000) **and** a plan unit has answered its part check
  (`Check::Matched` or `Mismatch` now — a unit that answers is a bus that is on;
  `faults::bus_on`). While none has, the count waits however long that takes: a board on
  permanent +12 V with the ignition off counts once the ignition comes on. A plan with no units
  starts at 10 s. Not while the stopwatch is up; not in adapter mode (`panel_bus` does not run
  there). Once per boot, no retry. The bus task wakes for 10 s (`Count::wake_ms`, never a moment
  gone by); what starts it later comes with a wake of its own (a part check's answer, the
  stopwatch closing).
- **Through the planner:** each `Step::Ask` is `planner.exchange(now, Class::Background, unit,
  pdu)`; the `Delivery::Raw` is recognised by its `ReqId`, fed to `answered`, and the next one is
  queued at the next turn. One request of the count's with the planner at a time. The board's
  guard is not involved; the planner still refuses anything off the allowlist (were it to refuse
  one, the unit is left out as a bus error). Only `22 2A26` and `19 02 08` go out, and no `10`.
- **`MAX_UNITS` = 64** (answer 2): `faultcount::MAX_UNITS` is `guard::MAX_UNITS` itself — both
  in `vag-uds-client`, built together — with a test that says so.
- **Own deadline** (answer 3): the count's exchange ends `DEADLINE_MS` (2 s) after its send,
  `78`s included. `panel_bus` asks `Count::deadline_ms(planner.flying_raw())` for the exchange
  on the bus; `transact` takes it and cuts every wait, the send's, the first answer's
  (`RESPONSE_TIMEOUT`, 500 ms) and each after a `78`, to what is left (`faults::within`), with
  no backstop past it. Every other exchange passes `None` and keeps 500 ms and
  `PENDING_DEADLINE` (10 s). A unit past it is not counted: `no answer in 2 s` when its exchange
  held the bus the whole 2 s, `no answer` when it was silent from the start. Either way the
  planner backs the unit off, as it does any unit past its deadline: were it one of the plan's
  units, its cells show dashes and its part number is read again before they come back.
- **Paused while the stopwatch is up** (answer 3; `Screen::stopwatch()`, the mode, whatever holds
  the glass): no request of the count's is queued, one waiting in the planner is taken back
  (`Planner::cancel`), and the walk goes on where it stopped when the stopwatch closes. One
  already on the bus when it opens runs to its end, ≤ 2 s. **No run arms while it is out, by
  construction rather than by `Stopwatch::hold`:** the turn of the mode resets the stopwatch
  (`Stopwatch::follow`), and it arms only after `ARMING_HOLD_MS` of standstill answers, none of
  which can come while the count's exchange holds the one bus. The firmware says so beside the
  `SILENCE_MS` assert, and `stopwatch.rs` pins what it rests on
  (`after_a_turn_of_the_mode_it_arms_only_on_a_standstill_seen_since`).
- **An exchange given up for adapter mode** is a `BusError` for that unit: `panel_bus` answers
  the planner so, as for any exchange, and the count takes it. For the gateway's list that is
  `NoList(BusError)`: a `?` for that boot.
- **Published** in `FAULTS` (`Option<Found>`, 12 B: stored and failing now as counts, or
  failed), written once when the count ends, read by the panel task into `Board::faults` every
  frame (`Found::badge`), on every page and the stopwatch's. A change signals `STATE_CHANGED`.
- **USB log** (`faults::Line`, every text pinned in the host test):
  - `faults: counting the car's stored codes — the gateway's list first`
  - `faults: the list names 2 ids past 795, which no unit can answer on — not asked` (bits past
    VW's block, and the block's ids past `0x795`, whose answer id would be past `0x7FF`:
    `Tally::unaddressable`, renamed from `outside_block`, which counted both and said one)
  - `faults: 776, 777 skipped — each shares an id with a unit walked` (one line for all)
  - `faults: 7E0 2 stored, 1 failing now` (a unit with codes)
  - `faults: 7E1 not counted — no answer` / `— no answer in 2 s` / `— bus error` /
    `— refused, NRC 22` / `— answer did not parse`
  - `faults: waiting while the stopwatch is up` / `faults: the stopwatch is closed — counting on`
  - `faults: 9 stored, 1 failing now; 17 of 18 units answered in 1.4 s` (the end)
  - `faults: the gateway gave no list (no answer) — badge ?`
  - `faults: the walk would ask 65 units, more than 64 — not a car's list, badge ?`
- **`state`:** ` faults=9/1` (stored/failing now) once counted, ` faults=?` when the count
  failed, nothing before, after `mode=`. `dashcfg` skips keys it does not know. The longest
  line is 189 B, checked against `UART_MTU` (244) at compile time (`STATE_LINE_LONGEST`).
- **`dash/README.md`'s rule** (answer 1): amended — the board resolves no label data; the
  gateway's list is a protocol read, bounded by `MAX_UNITS` and by VW's block.
- **RAM** (`ram-budget.sh`, empty plan): static +244 B in both builds — 139,320 → 139,564 B with
  BLE, 129,868 → 130,112 B without; stack −240 B (156,644 → 156,404; 187,984 → 187,744). Of it
  `COUNT` 104 B and `FAULTS` 12 B; the rest the compiler's merged globals and switch tables.
  The count's heap is phase 1's.

## How long it takes

Per exchange with the gateway in the path the board measured ≈4 ms (`dash/14` §6a); a fault
read may take longer (units answer `7F 19 78` while they search), say 5–50 ms. The planner
spaces sends 10 ms apart (100/s ceiling), the panel keeps its 25/s floor, and `Background`
takes the slots left — a four-cell page with the four alarm rules leaves most of them. So:
**19 exchanges ≈ 0.3–1 s** when every unit answers, **+0.5 s per silent unit** (the board's
`RESPONSE_TIMEOUT`, during which the panel's next read waits too), and at most 2 s for a unit
that answers `78` and never the rest (the count's deadline). Time the stopwatch is up adds to it. Worst case, a page asking
for the whole ceiling: the starvation rule sends each `Background` item within 5 s, ≈ 95 s.
The laptop's ~50 s over BLE also read `F197`, `02BD`, `F19E`, `F1A2` and extended data per code,
each a BLE round trip.

## Tests

- `faultcount` (18): the gateway first and idempotent; every unit of the walk asked `19 02 08`
  in order, by its block's rule; only `22` once and `19` — no `10`; stored = confirmed, failing
  now = confirmed and failing, a unit that ignored the mask counted right; a unit that is
  silent, refuses, errors, leaks a `78` or answers the wrong subfunction is skipped and named;
  a gateway with no list (silence, bus error, NRC, another identifier, too short) asks no unit;
  an empty list still reads the three; bits past VW's block (`7C0`, `7E0`) counted, not
  decoded; an id of the block past `0x795` counted with them, not asked (phase 2); a 4095-byte
  all-ones answer decodes no more than the block and is refused; 64 units
  walked, 65 refused with nothing asked, and `MAX_UNITS` is the BLE guard's; `776`/`777` beside `70C`, and `77E`/`77F` (answering
  on the engine's and the gearbox's answer ids), skipped as shared ids, and no two units of a
  walk share a request id or an answer id, in either role; counting a 4095-byte answer
  allocates under 256 B, and so does reading a 4095-byte list (a counting allocator in the
  test binary); an answer after the end changes nothing; the tally so far while the walk goes
  on, and the outcome once it is over (phase 2).
- `gateway` (2, moved from `survey`): the walk covers the three the list cannot hold; a unit
  listed twice is walked once.
- `render` (10): the triangle pixel for pixel at the foot of the icon column, the count over it
  right-aligned, nothing outside the box moved; hidden at 0 and with no count; failing now
  swaps every pixel of the box and nothing else; the corner is the same over a blank panel,
  an alarm page, a values page and a chart; the icons untouched and 25 rows above; the chart's
  trace untouched; a longer count grows left; 1–99 clear of the trace, 100–999 covering its
  last column and no more; the adapter screen draws none; a failed count is the `?` pixel for
  pixel over a count's triangle, in a one-digit box, never inverted, the same over values,
  alarm, chart and stopwatch pages (phase 2).
- `stopwatch` (1, phase 2): after a turn of the mode it arms only on a standstill seen since —
  what "no run arms while a count exchange is out" rests on.
- `research/dash/host/tests/fault_count.rs` (17, phase 2), through the board's planner: nothing
  before 10 s or before a plan unit answers, then the gateway; a plan with no units starts at
  10 s; the start waits for the stopwatch; a whole count reads `22 2A26` once and `19 02 08` a
  unit, each exchange with the 2 s deadline, finds 3 stored / 1 failing now, and says each line
  word for word, then asks nothing more; one request with the planner at a time; the stopwatch
  takes back a waiting request and the walk goes on at the same unit, said once each way; one
  already out runs to its end and nothing follows it while the stopwatch is up; no other
  exchange gets the deadline; `within`'s arithmetic; a gateway with no list (silence, silence
  past 2 s, bus error, NRC, another identifier) is `?`, said once, never asked again; 65 units
  is `?` with nothing asked; an exchange given up for adapter mode is a bus error for its unit;
  a count of zero draws nothing; the wake is never a moment gone by; `state`'s `faults=` and
  its longest; `Option<Found>` is 12 B. `dashsim --preview` draws the `?` on a values page and on
  the three-mark stopwatch page (29 previews).

## Car checks (phase 2)

1. `vagcan faults` and the board on the same car, same ignition cycle: the board's total and
   failing-now equal `vagcan faults`' last line, less any codes it prints under `776` or `777`
   (the board skips those as shared ids; the laptop asks them). Any other mismatch means a
   unit honours mask `08` differently — then ask `FF` and filter, as the laptop does.
2. The USB log's time for the count; the panel keeps changing through it.
3. The units named as not counted or skipped, against `vagcan units`' list: `776` and `777`
   skipped as shared ids, and no id past `795`.
4. A BLE `vagcan info` started during the count completes.
5. The inverted badge only if the car has a code failing now — nothing is provoked to see it.
