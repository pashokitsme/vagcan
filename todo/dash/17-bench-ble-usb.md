# dash / 17 — bench: `vagcan` through the board, over BLE and over USB

**Subsystem:** dash · **Needs:** the bench pair working, then the car once · **Opened 2026-09-14**

Everything built on 2026-09-14 (`ble-uds`) passed hardware-free tests and review, and
none of it has run end to end: the first run found the CAN pair dead in both directions
(`research/dash/can-bring-up.md` §9.7). This is the plan, from the agents' reports.

`B` = the board's port (`/dev/cu.usbmodem1101`), `C` = the CANable's
(`/dev/cu.usbmodem206E37A148451`). An agent starts BLE tools from Terminal.app
(`open -a Terminal <script>.command`). The owner allowed the Claude desktop app Bluetooth on
2026-09-26, so the owner's own runs from it work; a process the agent starts is attributed by
macOS to the embedded `com.anthropic.claude-code` bundle, which carries no Bluetooth usage
description, and TCC kills it (SIGABRT) on its first Bluetooth call without asking.

**2026-09-14 16:01 — the pair is still dead**; what runs without it passed
(`research/dash/can-bring-up.md` §9.8): the Hello probe (also right after a reset), the
refusals, `V`/`F`/`C` in adapter mode, `--slcan dev sniff` opening and closing, `dashcfg`,
`bleuds` forwarding and refusing, `vagcan info` over USB and over BLE up to "the car did not
answer". Two defects found and fixed there. Everything that needs frames on the pair is open.

**2026-09-14 17:44 — pair repaired (the transceiver module was unpowered); most of §1 and §2
passed** (`research/dash/can-bring-up.md` §9.9): BLE subscription 10.0 Hz, `watch` over BLE and
USB at 100 ms, both carriers at once, adapter mode in and out, `kill -9`, the `slcan` image.
**18:25 — `measure` through the board at 50 Hz over USB and BLE** (§9.10, after the timing
channel was added to the link). The board's USB output is clean for a minute (§9.9, in place of
`dashsim`). **Not run yet:** unplug USB in adapter mode (needs a hand at the bench), a 4095-byte
USB flood, the car. The hour-long stall test is dropped: both "quiet pair" episodes were the
unpowered transceiver.

## 0. The pair

- Repaired 2026-09-14: the transceiver module was unpowered (1.56 V on its 3.3 V pin; §9.7–§9.9).
- 2026-09-15: the SuperMini's `3V3` pad died; the transceiver runs from `5V`, which puts `RXD`
  into `GPIO1` past the C3's rating (§9.12). The 3V3 trace is broken; the owner keeps it on 5 V
  and accepts losing the board (2026-09-22).
- `research/dash/bench.sh 15 cantx` → PASS. Then reflash `dash` with the real plan.
- Power the board from the 12 V bench supply, not USB, so unplugging USB does not reset it.

## 1. BLE end to end (`dash/16`)

`benchecu --bench --device C --unit 7E0 --unit 7E1 --unit 710` running throughout (it stops
on a frame to any other id, so every unit a command addresses must be listed).

| command | expect |
|---|---|
| `vagcan info --device ble` | connects, `benchecu` counts `F190`… on `7E0`/`7E1`; allow ~2 min (rate cap, refusals) |
| `bleuds --subscribe 7E0 7E8 F40D 100 10` | ~10/s of `F40D` at `benchecu` (§9.5 saw 0.4 Hz only because nothing answered) |
| `vagcan watch --device ble --did 01:F40D --hz 10 --for 15 --out w.csv` | rows 100 ms apart with value 0; no ~5 s gaps (§9.7 saw them on a dead bus) |
| `vagcan measure --device ble` | its speed channel asked ~50/s at `benchecu` |
| walk out of range during `watch --device ble` | ends with "the BLE connection to vagcan-dash dropped", terminal restored |

## 2. The board over USB, and `--slcan` (`dash/14` §7 item 4)

1. `vagcan devices` → `B  vag-dash board — dash image (…), 0.1.0`; `dashcfg state` → `mode=panel`.
2. `dashsim B` for 2 min → frames and log lines, no `[bad frame]`.
3. `vagcan info --device B` with `vagcan dev sniff --device C --active` → `7E0 03 22 F1 90`; with `benchecu` instead → answers read.
4. `vagcan watch --device ble` and `vagcan info --device B` at once (with `benchecu`) → both answered.
5. `kill -9` a `vagcan watch --device B`, then `vagcan info --device B` → works, no "did not answer Hello"; then `vagcan --slcan dev sniff --device B` → switches at once.
6. `screen B 115200`: `V`⏎ → `V0101`; `F`⏎ → `F00`; `S6`⏎`L`⏎ with `C` transmitting → nothing acknowledged; `C`⏎ → `\r` and `mode=panel`.
7. `vagcan --slcan dev sniff --device B` with `C` transmitting → whole frames, no text lines (§9.4 again; the ring is 512 lines, so the rate may be lower).
8. Mode 2, channel open, busy bus, pull USB → within ~0.2 s `mode=panel`, BLE not refused.
9. Ctrl-C a `vagcan --slcan dev sniff --device B`, and a `vagcan --slcan info --device B` → `mode=panel` right after each.
10. `vagcan dev survey --device B` → refused before opening; `vagcan dev sniff --device B` → says it needs `--slcan`.
11. `kill -STOP` a `vagcan watch --device B` 3 s, `kill -CONT` → its link ends with "lost data"; a new command works. During `--slcan dev sniff`, `kill -STOP` → `mode=panel` after ~1 s.
12. `vagcan watch --device B` mid-run, start `vagcan --slcan dev sniff --device B` with `C` sniffing `--active` → no error frames at the switch, `F` flags clear.
13. A USB host flooding 4095-byte requests while `vagcan info --device ble` runs → no panic, `heap:` flat, the USB host slows.
14. `vagcan watch --device B`, quit normally → the board's `22` requests for it stop at once (sniff on `C`).
15. `vagcan devices` within a second of the board booting → still `dash image`, `mode=panel`.
16. Flash `slcan`; `vagcan devices` → `slcan image`; §9.4 still holds. Reflash `dash`.

**Run 2026-09-22** on the new board, USB only, `dash` without BLE (`can-bring-up.md` §9.13):
items 1, 3, 9, 16 pass; item 11 **fails** — a stopped host keeps the board in adapter mode (the
design ends it on `C` or on the cable's start-of-frame packets, and a stopped process sends
neither); a killed host on a silent pair does the same until the next Hello. BLE items (§1, 4,
13) wait on the board's BLE (`research/dash/ble-controller-hang.md`).

**Run 2026-09-22, old board with BLE** (§9.14): item 2 and §3 pass; item 13 found a heap
panic (fixed: `guard::MAX_REQUEST_BYTES`) and then a USB read that never woke (fixed: the
reader re-checks the FIFO every 50 ms), then a third (fixed: the board passes over a frame
longer than `console::MAX_HOST_BODY` instead of gathering it); three runs after that pass.
Item 11 passes, but in 5–15 s rather than ~1 s — macOS buffers the board's output for a
stopped host. `measure` over BLE ran at ~20/s; packing queued frames into one notification brought it to
34–38/s (§9.15). Item 8 is checked in the car (owner, 2026-09-22).

## 3. Older items carried here

- The busy-port message. (The board's `V`/Hello probe and `F` through `dev sniff` ran: §9.8, §9.9.)
- ~~The stored-config check at boot; whether opening the board's port twice resets it.~~ Both
  pass, 2026-09-22 (`research/dash/can-bring-up.md` §9.14).

## 4. The car, once

Recorded 2026-09-14, after PR #2's review. Each item is something the bench could not show:
the bench units answer at once or with a fixed delay, carry no traffic of their own, and
never move.

**Results on the car, 2026-09-26** (the old board, rev v1.1, `dash` with the owner's plan; at
boot it identified `7E0` `8V0906264H` and `7E1` `0CW300041G` as planned, and all 13 channels
answered, `200A`–`200D` among them):

- `vagcan info --device ble`: **passes** — VIN and both units' identities. Time not measured.
- `vagcan faults --device ble`: **passes** — all 18 units read, 9 stored codes (`70A`, `70C`,
  `712`, `713`), none failing now, texts from the ODIS project with the `.rod` fallback. About
  50 s (1 min 15 s including a 24 s build). Whether the panel kept updating was not recorded.
- `vagcan watch --device ble --hz 10`: **passes** — 140 s, rows 100 ms apart (median; p90
  110 ms, max 115 ms). `research/captures/ble-10hz.csv`; its comma headings split (PR #6).
- `vagcan units --device ble`: **finishes**, 15 units, no refusal
  (`research/captures/ble-units.out`).
- `vagcan measure --device ble`: one run, aborted and degraded, cycle median 44 ms
  (`research/captures/ble-measure.csv`); not a verdict yet.

**Through the board, parked, ignition on**

| check | expect |
|---|---|
| `vagcan faults --device ble` | the stored faults listed; the panel keeps updating |
| `vagcan info --device ble` and `--device B` | VIN and identities; note how long BLE takes with the rate cap |
| `vagcan watch --device ble` and `--device B` at `--hz 10` | rows 100 ms apart; the panel's values still move (its 25/s floor holds) |
| `vagcan measure --device B` and `--device ble` | how fast the timing channel's unit really answers (`7E1 F40D` on the bench plan); timing channel rate. Under ~20 ms: ~50/s. Slower: the other channels may drop to one read per 5 s — accepted by the owner (`dash/14` §2), note what it is |
| `vagcan units --device ble` | finishes; the guard's walk rule and rate cap do not refuse a normal run (delays are fine) |
| any command over BLE with engine traffic on the bus | no answers lost behind the acceptance filter (`vag_uds_can::filter`), no stale answer matched to the next request |
| `vagcan --slcan dev sniff --device B` | frames at the car's rate; count lines dropped by the 512-line ring |
| the board's USB panel output with `dashsim B` on the car's traffic | no `[bad frame]` |

**The moving-car guard**

| check | expect |
|---|---|
| `bleuds 7E0 7E8 1003` standing | forwarded after one `22 F40D` read |
| the same while rolling | refused, "moving" with the speed |
| ignition off, board powered | refused: no speed answer counts as moving |

**Alarms** (`dash/04`)

- The owner's `[[alarm]]` rules in `dash.toml`; the misfire rule's trip and release numbers set from the car.
- A takeover inverts the offending cell, blinking 400 ms on / 400 ms off while the value is out and
  steady through the 2.5 s hold (since 2026-09-26); a short press silences the episode.

**Cable adapter on the car**

- `vagcan info --device C` and `watch --device C`: the stale-answer sweep before each request (`discard_queued`, 5 ms bound) does not slow commands on a busy bus.

**Stopwatch sources on the ESC** (`dash/14` §6; added 2026-09-14)

| check | expect |
|---|---|
| `vagcan watch --did 713:1822,1800,1801,1802,1803` parked | all five answer; wheel speeds 0; acceleration near 0 on level ground (a grade reads as an offset) |
| the same, rolling slowly through a tight turn | which index is which wheel: the inner side reads slower, and on one side the front reads faster than the rear |
| rows' spacing in that `watch` | how fast the ESC answers, and so what rate its channels can have beside `380B` |
| `vagcan watch --did 713:1822,1800,1801,1802,1803 7E1:380B` through a launch | a rear wheel against `380B`: a gap at launch is wheelspin; `1822` steps at the launch instant |
