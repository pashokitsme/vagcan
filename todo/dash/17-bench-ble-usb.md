# dash / 17 — bench: `vagcan` through the board, over BLE and over USB

**Subsystem:** dash · **Needs:** the bench pair working, then the car once · **Opened 2026-09-14**

Everything built on 2026-09-14 (`ble-uds`) passed hardware-free tests and review, and
none of it has run end to end: the first run found the CAN pair dead in both directions
(`research/dash/can-bring-up.md` §9.7). This is the plan, from the agents' reports.

`B` = the board's port (`/dev/cu.usbmodem1101`), `C` = the CANable's
(`/dev/cu.usbmodem206E37A148451`). BLE tools must be started from Terminal.app: a process
started by the Claude app is killed by macOS on its first Bluetooth call, and `osascript`
to Terminal waits for an automation permission; `open -a Terminal <script>.command` works.

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

## 3. Older items carried here

- The busy-port message. (The board's `V`/Hello probe and `F` through `dev sniff` ran: §9.8, §9.9.)
- The stored-config check at boot; whether opening the board's port twice resets it.

## 4. The car, once

Recorded 2026-09-14, after PR #2's review. Each item is something the bench could not show:
the bench units answer at once or with a fixed delay, carry no traffic of their own, and
never move.

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
- A takeover inverts the offending cell; a short press silences the episode.

**Cable adapter on the car**

- `vagcan info --device C` and `watch --device C`: the stale-answer sweep before each request (`discard_queued`, 5 ms bound) does not slow commands on a busy bus.
