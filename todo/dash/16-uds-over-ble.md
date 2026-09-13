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
  single requests only, nothing sweep-shaped (`dev survey` and `dev sniff` are refused over
  BLE — no frames go over this link at all). The host's own guards (road speed before a
  session change) still run, through the same transport.
- **Choosing the device:** `--device ble` scans and offers a menu of what answered, the
  way `setup` asks; exactly one found → taken, and said. `--device ble:<name>` picks by
  name without asking, for scripts. No terminal and several found → the list and a refusal.

## Done when

- Host transport and board message handling covered by hardware-free tests (mock NUS on the
  host, the board's decode/allowlist/chunking in a host-testable crate).
- Bench: `vagcan info --device ble` makes the board put `7E0 22 F1 90` on the pair, seen by
  the CANable with `dev sniff --device … --active` (no unit answers on the bench).
- Car: `vagcan faults --device ble` lists the stored faults, the panel still updating.
