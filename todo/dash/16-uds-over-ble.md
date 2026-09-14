# dash / 16 — UDS over BLE: a second, slow transport to the car

**Subsystem:** dash + uds · **Needs the car:** once, for the end-to-end check ·
**Opened 2026-09-13** · **Starts after `dash` is merged to `master`** (owner).

Replaces item 8 of `14` §7 ("the same link over BLE NUS; measure the PDU rate") and makes
item 7 (the `frame` mirror) unnecessary.

## The ask, in the owner's words (2026-09-13)

"Я хочу иметь возможность общаться с машиной по uds не только по кабелю, но и по ble – это
не основной способ, поэтому производительность ему не очень важна – скорее просто единично
прочитать ошибки. Поэтому нужно просто реализовать бэкенд."

## Decided

- **A transport, nothing more.** On the host, `Bus::start_remote` over a `link::Pipe`
  (`vag_dash_ble::NusPipe`): the same `Bus` a cable gives, so every car command runs through
  it unchanged. Speed does not matter; one fault read at a time is the use.
- **On the board, the `dash` image.** A NUS message carrying `(request id, response id,
  UDS PDU)` is queued between the panel's own reads; the board runs ISO-TP on its TWAI and
  sends the answer PDU back. The panel keeps running; the board stays the one talker.
  It goes through the board's planner (`14` §2).
- **Framing:** a NUL marker, a type byte, a u16 length, the body (`vag_uds_transport::link`), cut into NUS-MTU chunks. BLE's link layer
  already guarantees delivery and integrity. The type byte separates UDS from the
  text settings protocol `dashcfg` already speaks on the same NUS (`12`).
- **Always visible** (owner, 2026-09-13; implemented 2026-09-14): "мы можем видимость всегда включенной держать и
  не париться с этим? антенна всё равно далеко не бьёт". The board has one button (BOOT,
  inside the enclosure) and no wake button, so a long-press gate is not practical. No
  pairing in the first step.
- **Because it is always visible, the board enforces the limits itself** — a host across
  the radio is not trusted: the allowlist `0x22 0x19 0x10 0x3E` is checked on the board;
  single requests only (`dev survey` and `dev sniff` are refused over BLE — no frames go
  over this link at all). What "nothing sweep-shaped" and "not on a moving car" mean on
  the board is the next bullet; a check the host makes is not one of them.
- **The board's own guards** — *a reviewer's requirement, accepted by the author on
  2026-09-13, pending the owner.* The bullet above left the moving-car check on the host
  and called it "through the same transport", which was false twice: the host is the
  party not trusted, and `safety::require_stationary` takes a CAN backend, not a PDU
  transport, so over BLE it does not run at all. And an allowlist by service lets
  `10 02` through, and cannot tell one read from a sweep. So, on the board, per BLE
  request, before anything reaches the pair:
  Implemented in `vag_uds_client::guard` (2026-09-14); the figures are its constants.
  - **`10 02` (programming session) is refused outright** over BLE, whatever the car is
    doing. Nothing this project does needs it. Bit 7 of the session byte only suppresses
    the answer (ISO 14229-1), so the board masks it first: `10 82` is `10 02`, refused;
    `10 81` is `10 01`, allowed.
  - **Any other session change except `10 01` (default) is refused unless the board has
    just read road speed 0 from the engine** — `22 F40D` to `7E0`/`7E8`, the SAE J1979
    parameter on the ISO 15765-4 engine address (`safety.rs` reads the same), asked
    immediately before the request is forwarded, not taken from the panel's last poll.
    No answer, a negative response, or any non-zero speed is "moving": refused, and the
    refusal says which. `10 01` is always allowed — returning a unit to default is the
    safe direction. The speed read is a request to the car too: it counts one toward the
    rate cap, whether the session change is then allowed or not.
  - **A rate cap, per BLE connection:** at most 20 counted units in any 10 s — one per
    identifier in a `0x22`, one per other request, one per speed read. Over the cap the
    board **delays** the request until it fits; it never refuses or drops it (owner:
    slowing is allowed, dropping is forbidden).
  - **Identifiers count, not requests** (review round 3, 2026-09-13): one `0x22` request
    can carry many identifiers, so `22 F100 F101 … F1FF` would pass a per-request cap.
    Over BLE the board allows at most **4 identifiers in one request**, so the host must
    batch at most 4 over this link (`Budget::max_dids_per_request: 4` for this link; the cable's default is 8).
  - **A walk rule, per unit, per connection, blind to order:** the board keeps the set of
    different identifiers asked of each unit. A request that would put 8 evenly spaced
    identifiers into that set (`WALK_RUN = 8`, any non-zero stride: `F100…F107`,
    `2000 2002 … 200E`) is refused, and every further `0x22` to that unit is refused until
    the connection drops. Order, repeats and padding do not change the set, so they do
    not dodge it. `dev survey` and `units --identify <unit>` (all of `F100–F1FF`) are
    refused by their 8th identifier. A watch page with a few neighbours (`2029 202A 202B`)
    passes; identification and faults ask no run at all. (It replaced "three in a row",
    which refused such watch pages and was dodged by padding.)
  - **Caps, per connection:** at most 32 different identifiers per unit, and at most 64
    units (`MAX_UNITS`) — the board's memory for all of it stays under 8 KB.
  - **Subscriptions:** `measure` reads speed at 50 Hz and `watch` polls a page; across a
    radio link neither works as one request per reading, and timing has to be taken where
    the bus is. So the host subscribes (`did`, period) and the board polls on its own clock
    and streams timestamped readings (`link.rs`, types `0x03`–`0x05`). A subscription is
    only ever `22 did`. Its identifier counts once, at subscribe time, toward the walk
    rule and the identifier and unit caps; a locked unit refuses subscriptions. It does
    **not** count toward the rate cap, and neither do the board's polls, so a 20-channel
    watch starts at once. At most 32 live subscriptions per connection, none faster than
    20 ms; an unsubscribe frees a slot.
  - **A timing flag** (2026-09-14, after `measure` over the board on the bench ran its
    speed at 10 Hz: every host subscription ran as the board's `Remote` class, thinned at
    the planner's 100/s ceiling). A Subscribe ends with a priority byte, `0` normal,
    `1` timing. Normal runs as `Class::Remote`; timing as `Class::Timing`, ahead of a
    host's other work.
    At most **one timing subscription per connection** (`MAX_TIMING_SUBSCRIPTIONS`), on
    the radio and the cable alike; a second is refused, and a timing subscription counts
    toward every other cap like a normal one. The caps bound subscriptions, not bus time:
    a timing read on a unit slower than 20 ms is always due again. So on the board the
    panel's floor goes ahead of a host's timing channel (`Budget::board()`,
    `timing_yields_to_floor`, review of the first commit, 2026-09-14); with slow units the
    timing channel gets what is left, and a host's normal subscriptions what it leaves.
    The laptop keeps Timing first. The laptop's remote `Bus`
    sends `timing` for a `Class::Timing` subscription, `normal` for any other class, and
    refuses a second timing one itself.
  - **One timing channel for the whole board** (decided 2026-09-14: one stopwatch at a
    time). The board runs a radio session and a cable session side by side on one
    planner, and a timing subscription on each would take the whole ceiling. So beside
    the per-connection cap, a session forwards a timing subscription only while that
    planner holds no other `Class::Timing` subscription; a second from either carrier is
    refused, "another client holds the board's timing channel". It frees when the holder
    unsubscribes, re-subscribes normal, or its session closes (disconnect, Hello, stall).
  - The host may still check road speed itself, over this transport, as a courtesy that
    fails early with a better message — never as the enforcement.
- **Choosing the device:** `--device ble` scans and offers a menu of what answered, the
  way `setup` asks; exactly one found → taken, and said. `--device ble:<name>` picks by
  name without asking, for scripts. No terminal and several found → the list and a refusal.
- **Zero friction is the point** (owner, 2026-09-14: "Сделать связь по BLE простой и
  доступной. Просто запускаю прогу и сразу коннекчусь"). No pairing, no confirmation, no
  button. Anyone nearby can read what the guards allow — VIN, faults — and that is
  accepted. BLE only when asked (owner, 2026-09-14 evening: «вообще можно сделать --ble для поиска ble
  устройства, автопоиск выключить», then «ну либо --device ble»): no automatic scan, not
  even with no cable adapter; `--device ble` scans, `--device ble:<name>` picks.

## Done when

- Host transport and board message handling covered by hardware-free tests (mock NUS on the
  host, the board's decode/allowlist/chunking in a host-testable crate). *(Board side done
  2026-09-14: `vag_uds_client::remote::Session` — guard, planner, readings,
  walk lock, close — tested against a real planner and a simulated bus; the firmware's NUS
  server wraps it. Host side done 2026-09-14: `link::Pipe` and
  `pipe_pair`; `vag_dash_ble::NusPipe` (notifications drained by a task of their own,
  connect bounded to 10 s) and `scan_boards` (stops 1 s after the first board); 
  `Bus::start_remote` in `vag-cli-core/src/bus/remote.rs`, tested against a scripted board —
  subscriptions on the board's clock, one-shot reads, exchanges and every outcome, refusals,
  notifications of 20 and 244 bytes with the state line between, reads taken in between
  writes, a lost chunk breaking the link, a drop mid-exchange, the 33rd subscription;
  `--device ble` / `ble:<name>` in `device.rs` (the automatic fallback with no cable was dropped by the owner on 2026-09-14), every branch
  tested; `dev survey`, `units --identify <unit>` and `dev sniff` refused over BLE before
  anything is opened. The host side ran against the board on 2026-09-14, `research/dash/can-bring-up.md` §9.9.)*
- The board's guards tested the same way: `10 02` refused; `10 03` refused on speed > 0,
  on a negative answer and on no answer, allowed on 0; the rate cap delaying; a run of 8
  refused in any order and through padding; a multi-identifier request over 4 refused, and
  its identifiers counted; a stride run refused; the distinct-identifier and unit caps;
  `units --identify` refused (it walks `F100–F1FF`); subscriptions and their limits; and
  the request sequences `info`, `faults`, a watch page and a measure session passing every
  limit. *(Guard and wire format done 2026-09-14, `vag_uds_client::guard`,
  `vag_uds_transport::link`.)*
- Bench: `vagcan info --device ble` makes the board put `7E0 22 F1 90` on the pair, seen by
  the CANable with `dev sniff --device … --active` (no unit answers on the bench). *(The
  board's half passed 2026-09-14 with the bench tool `bleuds` in place of `vagcan`:
  `research/dash/can-bring-up.md` §9.5. `vagcan info --device ble` itself ran on 2026-09-14 once the pair was repaired, §9.9.)*
- Car: `vagcan faults --device ble` lists the stored faults, the panel still updating.
