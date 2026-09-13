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

- **A transport, nothing more.** On the host, a second implementation of
  `vag_uds_transport::AsyncIsoTpTransport` over BLE NUS (the scan and NUS pipe already live
  in `vag-dash-ble`). `faults`, `info`, `units`, `watch` run through it unchanged, because
  they never see frames. Speed does not matter; one fault read at a time is the use.
- **On the board, the `dash` image.** A NUS message carrying `(request id, response id,
  UDS PDU)` is queued between the panel's own reads; the board runs ISO-TP on its TWAI and
  sends the answer PDU back. The panel keeps running; the board stays the one talker.
  No scheduler needed for this (`14` §2 stays separate work).
- **Framing:** length + message type + body, cut into NUS-MTU chunks. BLE's link layer
  already guarantees delivery and integrity. The type byte separates UDS from the
  text settings protocol `dashcfg` already speaks on the same NUS (`12`).
- **Always visible** (owner, 2026-09-13): "мы можем видимость всегда включенной держать и
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
  - **`10 02` (programming session) is refused outright** over BLE, whatever the car is
    doing. Nothing this project does needs it.
  - **Any other session change except `10 01` (default) is refused unless the board has
    just read road speed 0 from the engine** — `22 F40D` to `7E0`/`7E8`, the SAE J1979
    parameter on the ISO 15765-4 engine address (`safety.rs` reads the same), asked
    immediately before the request is forwarded, not taken from the panel's last poll.
    No answer, a negative response, or any non-zero speed is "moving": refused, and the
    refusal says which. `10 01` is always allowed — returning a unit to default is the
    safe direction.
  - **A sweep limit, per BLE connection:** at most 20 UDS requests in any 10 s, and a
    third strictly consecutive identifier to one unit (`22 n`, `22 n+1`, `22 n+2` on
    one request id, in the connection's order) is refused and every further `0x22` to that
    unit is refused until the connection drops. A walk is what `dev survey` and
    `units --identify <unit>` (all of `F100–F1FF`) do and what the service allowlist cannot see —
    both are refused over BLE by it, as they should be. Identification reads at most two
    adjacent identifiers (`F190`, `F191`), and faults and a watch page none in a row. The
    numbers are a starting point for the owner to set, not measured.
  - **Identifiers count, not requests** (review round 3, 2026-09-13): one `0x22` request
    can carry many identifiers, and the project relies on that (`ARCHITECTURE.md`, one
    request tests a whole batch), so `22 F100 F101 … F1FF` would pass a per-request cap.
    Over BLE the board allows at most 4 identifiers in one request, counts every identifier
    inside a request toward the rate cap and the walk rule, treats any fixed stride
    (`n`, `n+2`, `n+4` …) as a walk, and caps the distinct identifiers asked of one unit
    per connection (a starting figure: 32).
  - The host may still check road speed itself, over this transport, as a courtesy that
    fails early with a better message — never as the enforcement.
- **Choosing the device:** `--device ble` scans and offers a menu of what answered, the
  way `setup` asks; exactly one found → taken, and said. `--device ble:<name>` picks by
  name without asking, for scripts. No terminal and several found → the list and a refusal.
- **Zero friction is the point** (owner, 2026-09-14: "Сделать связь по BLE простой и
  доступной. Просто запускаю прогу и сразу коннекчусь"). No pairing, no confirmation, no
  button. Anyone nearby can read what the guards allow — VIN, faults — and that is
  accepted. Without `--device`, when no cable adapter is found, `vagcan` scans BLE itself
  and connects to the one board it finds, saying so.

## Done when

- Host transport and board message handling covered by hardware-free tests (mock NUS on the
  host, the board's decode/allowlist/chunking in a host-testable crate).
- The board's guards tested the same way: `10 02` refused; `10 03` refused on speed > 0,
  on a negative answer and on no answer, allowed on 0; the rate cap; a consecutive walk
  refused at its third identifier; a multi-identifier request over 4 refused, and
  its identifiers counted; a stride walk refused; the distinct-identifier cap; `units
  --identify` refused (it walks `F100–F1FF`); and the request sequences `info` and `faults`
  actually send passing every limit.
- Bench: `vagcan info --device ble` makes the board put `7E0 22 F1 90` on the pair, seen by
  the CANable with `dev sniff --device … --active` (no unit answers on the bench).
- Car: `vagcan faults --device ble` lists the stored faults, the panel still updating.
