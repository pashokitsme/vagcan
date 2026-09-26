# dash / 14 — one bus, three clients: the consolidated design

**Subsystem:** dash · **Crates:** `vag-dash-fw`, `vag-uds-transport`, `vag-uds-can`, `vag-cli-core` ·
**Date:** 2026-09-13 · **Status:** design; §3 decided by the owner the same day, the order and state of the work in §7

The board reads the car (`05`, met 2026-09-13). Three wishes landed on the same day and
they all want the same wire:

1. **The laptop reads the car through the board** — `vagcan watch`, `info`, `faults`, the
   lot — *while the panel keeps showing its numbers* (owner, 2026-09-13).
2. **Screens**: the owner lays out channels on pages (`13-screens.md`); the chart page and
   page switching misbehave today and have to be fixed first.
3. **A stopwatch**: 0–60 and 0–100 timed on the board, with speed projected on time as a
   chart of the run.

Plus what was already queued: the OLED on the carrier (`05`, `08`), settings over BLE
(`11`, `12`), sleep (`07`). This file is the design that makes them one thing instead of
four, and names the one fork the owner has to pick.

---

## 1. The invariant everything hangs on

**One bus, one conversation, and the board is the only talker.** `CLAUDE.md` locks this
for the host and the firmware alike, and it is not a preference: two testers with the same
source address `7E0` on one bus get one stream of `7E8` answers between them, and an
ISO-TP multi-frame answer needs its flow-control frames back within milliseconds from the
one who asked. A laptop on the far side of a USB hop, and a board polling on its own
timer, cannot both be that one.

So every client of the bus — the panel, the laptop, the stopwatch — is a client of **one
scheduler on the board**, which owns the TWAI and runs one exchange at a time.

## 2. The scheduler (new, and the heart of it)

Today `dash.rs` polls the plan's cells round-robin and renders. The scheduler generalises
that into a queue of *requests* with three sources and a rate each:

| source | what it queues | rate | priority |
|---|---|---|---|
| **panel** | the visible page's channels | as fast as the page wants (≈50 Hz for four cells) | normal |
| **panel, background** | channels of pages not shown | slow (1 Hz) or none | low |
| **stopwatch** | the speed channel while the page is up | its own `hz` in `dash.toml` (at most 100) | foreground; with the stopwatch page on the glass no page cell is foreground, in any phase; an alarm's page over it drops to background while armed or running (`19`) |
| **lever** | the rocker's identifier, the cruise status | fixed in code (`plan.rs`): 2 / 20 Hz by the gate, 10 Hz while the stopwatch is up with a factor; cruise status 5 Hz (`19`) | foreground |
| **laptop** | whole UDS PDUs the host sends (§3) | as they arrive | normal, interleaved |

**Budget and degradation (owner, 2026-09-13: a small delay is fine, the bus is shared
with the whole car, a run needs speed fast).** The scheduler runs under a **ceiling of
≈100 exchanges a second** — half of what the one conversation could do — so the gateway
and the units see a sparse, even trickle from us. Default rates: temperatures 2 Hz,
boost and revs 10 Hz, the lever 2/20 Hz by the cruise gate (§6a); a four-cell page is ≈25
exchanges a second, a quarter of the budget. **Stopwatch page up** (as built, `19`): speed
at its own `hz` (50 gives 20 ms between points, and the crossing is interpolated between
them), every page cell at the background rate in every phase, an alarm's channels at theirs,
the lever at 10 Hz so LIMIT can still end the run. An alarm's page shown over the stopwatch
keeps its cells foreground except while the stopwatch is armed or running. When the sum asks for more than
the ceiling, nothing is dropped: sources are
**thinned in priority order** — background pages first, then the laptop's queue (it has
back-pressure anyway), then the visible cells down to their minimum rate, and never the
speed channel during a run. A cell older than its own deadline shows its age instead of
a stale number without a mark.

One exchange at a time: `0x22 DID` → answer → next. A multi-frame answer (part numbers,
`F19E`) is a single exchange with its flow control done on the board, in real time. The
allowlist (`0x22 0x19 0x10 0x3E`) is enforced **on the board** for laptop requests too —
the host's own allowlist cannot be trusted from a device that lives in the car.

Every answer the board receives updates every consumer that asked for it: a `62 202A …`
requested by the laptop's `watch` also refreshes the panel's НАДДУВ cell. That is what
makes "parallel with the display" true instead of a time-share: the two clients
*share the readings*, not the bus.

### Decided 2026-09-14 (owner): subscriptions, not a priority queue

"Функции программы имеют возможность подписаться на рассылку данных с такой-то частотой,
либо запросить их единоразово … шедулер сам решает кого когда оповестить. Должна быть
задействована drop семантика у подписчиков … Шедулер должен по максимуму ужимать запросы."

- **API.** `subscribe(unit, did, rate) -> Subscription` (`next().await` yields readings),
  `read_once(unit, did)`, and `exchange(unit, pdu)` for a raw non-`0x22` request.
  Dropping a `Subscription` unsubscribes; a dead consumer (a page closed, a BLE connection
  gone) takes its subscriptions with it.
- **Compression.** One read per `(unit, did)` whatever the number of subscribers; due
  identifiers of one unit go out as one `22 d1 … dn`, and ones due soon are pulled into
  that request; a unit that refuses multi-identifier requests is learned once and asked
  singly. A laptop's one-shot `22` may be merged with the panel's reads.
- **Rates come from the plan, never from a heuristic.** `hz` per channel in `dash.toml`,
  default 2 Hz. No rate derived from a unit of measure.
- **Ceiling 100 exchanges/s; the panel keeps at least 25/s** while a laptop is served.
- **A slow timing unit may leave `measure`'s other channels at one read per 5 s** (owner,
  2026-09-14: «Ну пусть ошибку дропаем. measure не основной кейс всё же»). When the unit
  holding the timing channel answers in ≥ 21 ms, the speed channel keeps what the bus allows
  after the panel's floor, and everything else gets only the starvation rule's one read per
  `starve_after_ms`. Accepted as is; no share cap for the timing channel.
- **Precedence per slot:** Timing (stopwatch) → a Remote or Background item due for
  longer than 5 s (`starve_after_ms`; nothing waits forever) → Foreground under its 25/s
  floor → Remote → Foreground above the floor → Background; within a rank, the most
  overdue first. A Timing request carries only Timing identifiers. **On the board**
  Foreground under its floor goes first, ahead of Timing (`Budget::timing_yields_to_floor`,
  2026-09-14): a host's timing channel on a unit slower than its period is always due and
  would take every slot, starved items included.
- **Shape.** A `no_std` core with no clock and no bus (`due(now) -> request`,
  `answered(now, request, answer) -> deliveries`), tested on the laptop.
- **One layer everywhere, now** (owner: "Общий слой, используется везде вместо текущего
  механизма … Не позже — сейчас. Не будем плодить легаси"). The board wraps the core in
  embassy over TWAI and replaces `can_task`'s round-robin; the laptop wraps it in tokio and
  replaces `plan::read_batch` polling in `watch` and `measure`, and one-shot reads in
  `info`/`faults` go through it too. Radio requests pass the board guard (`16`) first.

## 3. The fork: how the laptop talks to the board

### Option A — the board is an slcan adapter (raw frames)

The board speaks slcan over USB; `SlcanBackend` drives it unchanged; zero host code. This
is what `todo/dash/09` asked for and what the `slcan` binary delivers (merged,
`87dc9f2`). **It cannot share the bus with the panel.** The host sends raw frames on its own
clock; the board can only *yield* — stop polling while the host is mid-exchange, detect
the end by watching the bus, resume — and hope the host's flow-control frames arrive in
time through USB. The panel goes stale while `watch` runs. It is an *exclusive* mode: a
good one (`dev sniff`, bench work, a laptop-only session), and the wrong one for wish 1.

### Option B — the board is a UDS proxy (whole PDUs) — **recommended**

The host already has the seam: `vag_uds_transport::AsyncIsoTpTransport` moves whole PDUs,
and `IsoTpCan` over slcan is just one implementation. A second one, `BoardTransport`,
sends `(unit, pdu)` up a serial link and gets `(unit, pdu)` back; the board runs the
ISO-TP (it already links `vag-uds-can` `no_std` for its own polling) inside the
scheduler of §2. `watch`, `info`, `faults`, `units`, `sensors` work through it because
they never see frames. Sharing is by construction, flow control is on the bus's own clock,
and the allowlist is checked where the bus is.

What it costs: a link protocol (§4) and one transport impl on the host — a few hundred
lines, tested against a mock like `vag-uds-capture`. What it cannot do: `dev sniff` and
`dev survey`'s raw-frame view, which read *frames*. Those get a `frame` message on the same
link (§4) — mirrored, listen-only — and the exclusive slcan mode of option A stays for
transmitting raw frames from a bench.

### Decided 2026-09-13 (owner): both, as two modes of one firmware

- **Slowing down is allowed; dropping is forbidden** — a device that drops looks like a
  dead device. In mode 1 that holds by construction: the host's PDUs queue with
  back-pressure, `BoardTransport` awaits, `watch` slows to what the bus gives. In mode 2
  it holds as far as a buffer can make it: a ring of several hundred frames between the
  TWAI (esp-hal queues 32) and the USB writer, and if the host still falls behind the
  `E` register says overrun, as the CANable's does — a receiver cannot slow a bus.
- **Mode 1 — the panel plus the UDS scheduler** (§2, option B): the display keeps a
  guaranteed share of exchanges per second, the host gets the rest under one ceiling.
- **Mode 2 — the dumb slcan proxy** (option A): raw frames, the screen shows `SLCAN`, the
  bit rate, and rx/tx/error counters instead of the cells.
- **Switching is by the first bytes on USB, and the host chooses with a flag** (owner,
  2026-09-13): `vagcan --slcan` makes `vagcan` speak raw slcan to the board — the flag
  exists for the board alone, the CANable needs none — and the board, seeing ASCII lines
  (`C\r`, `S6\r`, `O\r`) rather than the framed binary link of §4, enters mode 2 and
  shows `SLCAN` and its counters. Without the flag `vagcan` opens the board through
  `BoardTransport` and the panel keeps running beside it. The board leaves mode 2 when the
  cable is pulled (USB disconnect) or on `C`, and the panel comes back. One image, no
  reflash, no jumper — the free pins (`GPIO0`, `GPIO3`) stay free.

### What this decides

- The `slcan` binary (merged, `87dc9f2`) became the **exclusive adapter
  mode**, kept, documented as such. Not the answer to wish 1.
- Wish 1 is Option B: `BoardTransport` on the host, the scheduler on the board.
- One link, typed messages, replaces the ad-hoc `FRAME …` lines `dash` prints for `dashsim`.

## 4. The link: one protocol, two carriers

A framed, typed message stream, the same over **USB-Serial-JTAG** and over **BLE NUS**
(`.archive/tasks/done/dash/11-ble.md` measured BLE cannot carry a loaded *bus*; it can carry PDUs at `watch` rates —
50 Hz × ~12 bytes is a kilobyte a second, and that measurement stands to be made):

**As built (2026-09-14), `vag_uds_transport::link`:** a NUL marker, a type byte, a
little-endian u16 body length, the body. No CRC: BLE's link layer and USB already check
integrity. Text without a NUL between frames (the `dashcfg` commands and state lines,
`FRAME` lines for `dashsim`, `BTN` presses) shares the carrier and is routed apart.

| type | message | direction | carries |
|---|---|---|---|
| `0x01` | Request | host → board | `seq`, request id, response id, a UDS PDU |
| `0x02` | Answer | board → host | `seq`, status (PDU / no answer / refused + reason / bus error), payload |
| `0x03` | Subscribe | host → board | sub id, request id, response id, identifier, period, priority (normal / timing) |
| `0x04` | Unsubscribe | host → board | sub id |
| `0x05` | Reading | board → host | sub id, board time of arrival, status, payload |
| `0x07` / `0x08` | Hello / HelloReply | host ↔ board | tells the `dash` image apart without slcan; a Hello closes that carrier's session |
| `0xFF` | broken frame | board → host | ends a frame the writer had to cut |

The 2026-09-13 sketch above it (`pdu`/`frame`/`image`/`button`/`config`/`log`, COBS or a
CRC-16, a `vag-dash-link` crate) was not built: no frame mirror (owner), no separate link
crate, and the panel and presses stay text lines.

## 5. Pages, and the defect in front of them

The page model gains kinds: `values` (today), `chart` (today, broken), **`stopwatch`**
(§6), `alarm` (`04`, takes the screen). The chart/page defect the owner saw on
2026-09-13 — pages "switching strangely", charts never drawn — is fixed first and alone,
with a `dashsim` reproduction, before any page is added. `dash.rs:585-1071` is where
`PageKind` is dispatched; `value_shrunk: true` on page 0 is a second, smaller thing (a
value that did not fit at full size).

Channels per page come from `13-screens.md` by the owner's choice, into `dash.toml`; the
scheduler (§2) polls the visible page fast and the rest slow, so a ten-cell page costs
the page it is on and nobody else.

## 6. The stopwatch page

`vag-cli-measure` already is the stopwatch, on the laptop: roles (`speed`, `engine speed`,
`gear`, `pedal`), the run detection, the report. The board's page is its small brother:

- **Source, decided 2026-09-13 (owner):** the gearbox **output shaft speed `380B`** — proven on
  the car (`~/.vagcan/data/<project>/measurements/0CW300041G.json`), 1 rpm a step and not
  averaged, so finer than either road speed below. It turns into km/h through a
  per-car factor (final drive × rolling circumference) that is **measured, never
  written in**: a steady-speed stretch reading `380B` and `F40D` together fits it, and the
  factor is stored under `~/.vagcan`. `F40D` stays beside it as the cross-check. Known
  limit: wheelspin at launch reads as speed. Before that decision the text read:
  one speed channel, chosen in `dash.toml` from `13-screens.md` — `2033`
  (0.01 km/h, declared) or `F40D` (1 km/h, standard); ESC wheel speeds if they answer.
  The choice is the owner's; the standard one is the safe default.
- **The ESC's channels, found 2026-09-14, not read on the car yet.** The owner asked whether an
  accelerometer could help; the board has none, the car's is in the ESC. This car's ESC (`713`,
  `5Q0614517AQ`, Continental MK100, ODIS variant `EV_Brake1UDSContiMK100ESP_036`) declares:
  - `1800`–`1803` wheel speeds, 0.1 km/h a step; which index is which wheel is not known. DQ200 is
    front-drive only, so a rear wheel is not driven and does not spin at launch — the limit of
    `380B` above — and, unlike an acceleration, it does not drift. A candidate speed source.
  - `1822` longitudinal acceleration, `u16 × 0.03125 − 16` m/s² (±16 m/s², 0.03 a step). **Not a
    speed source:** integrated, 0.01 g of offset is ~2.5 km/h by 100 km/h, and road grade and the
    body squatting (~1°, ~4 km/h) read as acceleration. Useful for the launch instant (a step,
    where the first moving speed sample comes up to one poll period late) and to flag a spinning
    start (wheel-derived acceleration above the measured one).
  - Both are on `713` and `380B` is on `7E1`: separate exchanges, sharing the 100/s ceiling.
  - The parked survey (`research/dumps/survey-parked.jsonl`) never asked `18xx`; the ESC answered
    48 identifiers in `02xx 06xx 19xx 2Axx F1xx`. Not answered there means not asked.
  - These come from the ODIS project for this car; another car resolves its own ESC's channels
    the same way. Nothing here goes into code.
  Car check: [`17-bench-ble-usb.md`](17-bench-ble-usb.md) §4, "Stopwatch sources on the ESC".
- **Arming**: speed at 0 for a second arms it; the first sample above 0 starts the run;
  crossing each mark stamps a time, interpolated between the samples either side. The clock's
  origin is the launch as `vag-cli-measure` reconstructs it (`derive::start`): the midpoint of
  a constant-jerk fit through `√v` and a line through the first two moving samples. There is
  no "half-sample correction" in `vag-cli-measure`; this text said so before `19` checked.
- **Rate**: the speed at its own `hz` in the foreground while the page is up (with a factor);
  every page cell is background whenever the stopwatch page is on the glass, in every phase.
  An alarm's page shown over it keeps its cells foreground except while armed or running. The
  fit needs three moving samples in its first 0.4 s, so the speed must be read faster than
  5 Hz; the plan build refuses a slower one (`19`).
- **Display** (as built, `19`): one values row — the phase over the speed, then a time per
  mark. **Not built:** a chart page of the run (speed on time, seconds since launch).
- **Record**: the run's samples go up the link as `pdu` answers anyway, so a laptop that
  is connected gets the full trace for `vagcan measure`'s report; the board keeps the last
  finished run's times in settings, written to flash at the next standstill, never at speed
  (owner, 2026-09-26; `12`, `19`).

## 6a. Controls: the button stays, the stalks are an event source

The owner asked (2026-09-13) whether the bus can deliver a button press, so the device
needs no button of its own. It can deliver the *state*, not the press:

- **Broadcast frames do not reach the OBD pins.** The stalks and wheel keys go out as
  cyclic messages on the comfort bus; the diagnostic CAN behind the gateway carries none
  of it — every sniff on this car saw one broadcast id there, the gateway's heartbeat.
- **Polling does.** The steering column module `70C` (`EV_SMLSVALEOMQBLRH_001`) declares
  one identifier, `1105`, carrying every stalk as an enum: the **MFA rocker** on the wiper
  stalk, the GRA lever (plus/minus, Set, Set+ACC), the left stalk's FAS / GRA Set-Reset /
  ON-CANCEL-OFF, indicators, lights, wipers, horn. `22 1105` at 20 Hz in the scheduler
  is a 50 ms button. The keys on the wheel's spokes are **not** in it (they reach `70C`
  over LIN; no identifier for them is declared) — whether `1105` answers on this car is
  one `watch` on `70C` away, not yet done.
- **But the car keeps its side of the press.** The MFA rocker still pages the cluster's
  trip computer; nothing on the diagnostic CAN can consume a press. So the stalks cannot
  be *the* button without the two displays paging together.

Decided (owner, 2026-09-13): **the cruise lever, gated by its own switch.** With the GRA
main switch **OFF** the car ignores the lever entirely, so its plus/minus page the dash
and Set may arm the stopwatch; with it **ON** the dash ignores the lever and cruise works
as always. Both the presses and the ON/OFF position are fields of `1105` (`Linker Hebel
axial 3 (ON/CANCEL/OFF)`, `GRA Hebel vertikal (Plus/Minus)`, `GRA Hebel axial (Set)`), so
one 20 Hz poll gives the gate and the events together. `1105` joins the scheduler as an
**event source** with the panel's quota. The device keeps its bench button (`GPIO9`,
BOOT) because the bench has no stalk; no wake button is needed — the board is fed from
OBD pin 1, +12 V with the ignition only (owner, 2026-09-13; `07` and `08` superseded).

**The gate is OFF, and OFF alone** — not CANCEL. The owner proposed OFF/CANCEL
(2026-09-13); CANCEL on an MQB stalk leaves cruise in standby with the set speed kept, and
the next plus/RES *resumes* it — paging the dash would accelerate the car to the stored
speed. So: the cruise state comes from the engine's own GRA status (`7E0`, beside `2018`),
which tells "off" from "standby" from "regulating", and the dash listens to the lever only
in "off"; the switch position in `1105` is the second witness, both must say off. Widening
the gate to CANCEL waits for a test on a parked car: switch on, set, cancel, press plus
standing still, read the status — if nothing resumes, the config may allow it; the default
stays OFF.

**What it costs, and the adaptive rate.** One `22 1105` exchange is six frames: the
request, then a 27-byte answer as ISO-TP multi-frame — first frame, our flow control, three
consecutive frames. That is ≈1.5 ms of wire time at 500 kbit/s; with the unit's reply delay
and the gateway in the path, an estimated 4–5 ms of the board's one conversation (not
measured yet). 20 Hz is ≈3 % of the diagnostic CAN (which carries nothing else of the car's)
and of the comfort bus behind the gateway, and ≈10 % of the board's exchanges. The bus is not the limit; the
board's one conversation is (≈4 ms per exchange with the gateway in the path, so
≈200–250 a second for everything). The stalk poll adapts to the gate: cruise ON → 2 Hz,
enough to notice the switch going off; OFF → 20 Hz, a 50 ms button; stopwatch up with a
factor → 10 Hz, so LIMIT can still end a run (`19`, as built). On average that is a few exchanges a second (owner's
concern, 2026-09-13).

To verify on the car first, one `watch` on `70C`: that `1105` answers; that the lever
fields move within a poll; that OFF reads as its own value and not as absent. If the
ON/OFF on this stalk turns out momentary rather than latched, the cruise state comes from
the engine instead (`7E0` carries the GRA status beside `2018`), and the gate is that.

**Probed on the car, 2026-09-26** (`research/captures/cruise-lever.csv`: `watch --device ble
--hz 10`, `70C:1105` with `7E0:203C,4383,2018`, 73 s parked, ignition on). Answer: **yes, the
lever pages with cruise OFF, and the engine ignores it.**

- `1105` answers over BLE at 10 Hz. On this car the cruise lever is the left stalk: byte 8
  ("Linker Hebel axial 2 (GRA Set/Rest)") is the rocker, byte 9 ("… axial 3
  (ON/CANCEL/OFF)") the switch. Bytes 15–17 ("GRA Hebel …") never move (247–250): not this
  stalk.
- The bytes are analog ladder readings with ±2 of noise, not exact enum values — decode by
  nearest level. Byte 8: 205 rest, 91 RES/+ (the engine's `4383` bit 3, accelerate), 128
  SET/− (bit 2, decelerate), 167 LIMIT — the owner pressed LIMIT twice during the capture and
  167 appears on exactly those presses (≈0.6 s and ≈0.4 s, switch OFF), never at rest, whatever
  its ODIS name ("neutral ohne Limiterverbau"). Byte 9: 167 OFF, 91 ON, 128 CANCEL.
- **Decoded by the project's own bands (2026-09-26, `feat/enum-ranges`).** Each ODIS
  text-table level has a coded lower *and* upper bound, and these levels are bands
  (byte 8: 75–110, 111–145, 146–181, 182–221); keyed on the lower bound alone, none of the
  noisy readings matched. `watch` now names them on screen and in `--out`: byte 8 91
  "beschleunigen", 128 "verzögern", 167 "neutral ohne Limiterverbau", 205 "neutral mit
  Limiterverbau"; byte 9 91 "Ein", 128 "Cancel", 167 "Aus". A cache from before needs
  `vagcan setup <ODIS project>` once; `watch` says so.
- **OFF is latched, CANCEL springs back:** 167 holds for tens of seconds; 128 lasts 0.1–0.8 s
  and returns to 91 (ON), or passes on to OFF when the switch is pushed through.
- **With OFF the engine ignores the rocker:** 55 samples of byte 8 at 91/128/167 while `203C`
  read 0 (main switch off) and `4383` read `2000` (only "control device verified"). With ON,
  `4383` is `3101` (+ bit 3 or bit 2 with the rocker, bit 1 at CANCEL) and `203C` is 2
  (passive). The two witnesses the gate needs both read "off", from different units.
- A deliberate press lasts 0.25–0.5 s at the rocker, but a tap can be one 10 Hz read: with
  the switch OFF, + at ~59.0 s and ~89.2 s and − at ~90.4 s were each one answer (counted by
  answer time; a `watch` row repeats the last answer). The board's debounce needs the same
  state on two consecutive reads: ≈0.1 s at 20 Hz (gate open), ≈0.2 s at 10 Hz (stopwatch up).
  Hold longer to be sure; a shorter tap can be missed (`19`).
- Not settled: widening the gate to CANCEL. Parked, nothing was ever set (`2018` stayed 0,
  `203C` never left 2), so "does plus resume after CANCEL" was not tested. The default stays
  OFF.

## 7. Order, and what each needs

| # | item | needs | state, dated per row |
|---|---|---|---|
| 1 | chart/page defect, with a `dashsim` repro | bench | **done**, merged (`599ba68`); a stale stored config no longer hides the plan's pages (review round 1) — not yet seen on the board |
| 2 | `vag-dash-link` crate + `image`/`log`/`button` over it; `dash` stops printing `FRAME` | bench | **folded into 4 and `16`** (owner could not see a reason for it alone): the message types arrive with the first transport that needs them |
| 3 | scheduler in `dash`: sources, rates, shared answers | bench | **done, merged in PR #2** (2026-09-14): `vag_uds_client::schedule::Planner`, `no_std`, clock-free. On the laptop `vag-cli-core/src/bus` owns the link, `watch` and `measure` subscribe, every other car command runs through it as a `UnitLink`. On the board `can_task` runs it under embassy in place of its old round-robin — the visible page at each channel's `hz` (`dash.toml`, default 2), hidden pages at 1 Hz, BLE requests and subscriptions through the same planner, the acceptance filter following the exchange. Bench: `research/dash/can-bring-up.md` §9.5 |
| 4 | `pdu` message + `BoardTransport` on the host, `vagcan --slcan`; `watch` through the board | bench, then car | **implemented; bench passed** (2026-09-14, merged in PR #2; `research/dash/can-bring-up.md` §9.9–9.10): the USB cable carries the framed link to a second session (`Guard::cable`), Hello/HelloReply (`0x07`/`0x08`) tells the `dash` image apart, `Bus::start_remote(SerialPipe)` on the host; adapter mode (mode 2) inside `dash` on an slcan line, ended by `C` or the host's SOF stopping (`vag_uds_client::console`); `--slcan` on `vagcan` and `vagcan-measure`; survey and `units --identify` refused through the board, `dev sniff` needs `--slcan`. Bench plan: [`17-bench-ble-usb.md`](17-bench-ble-usb.md); unplugging USB in adapter mode is not run yet. **Known limitation:** a laptop that sleeps stops SOF, so adapter mode and the USB session end without a word; a `vagcan --slcan dev sniff` running across the sleep gets no frames after it (run it again) |
| 5 | `slcan` binary as the exclusive mode | bench | **done**, merged (`87dc9f2`); bench passed 2026-09-13 (`research/dash/can-bring-up.md` §9.4) |
| 6 | stopwatch page | car, one straight road | **built** on `380B` (§6) — [`19`](19-stalk-and-stopwatch.md); the factor and a run on the car open |
| 7 | `frame` mirror for `dev sniff` over the link | bench | **dropped** (owner): sniffing through the board is mode 2 over the cable, and BLE cannot carry a loaded bus (`11`) |
| 8 | the same link over BLE NUS | bench | **became [`16-uds-over-ble.md`](../../.archive/tasks/done/dash/16-uds-over-ble.md)**: UDS over BLE as a slow transport, merged in PR #2 (2026-09-14) |
| 9 | OLED on the carrier | bench | later — the panel has not arrived; the enclosure is [`15-enclosure.md`](15-enclosure.md) |
| 10 | the cruise lever as an event source, gate OFF (§6a) | car | **probed 2026-09-26**: `1105` byte 8 moves with the rocker while `203C` = 0 and the engine's `4383` stays `2000` — the lever can page the dash with cruise OFF (§6a). Next: [`19`](19-stalk-and-stopwatch.md) — the lever as buttons (+ next, − previous, LIMIT the stopwatch), decoded by the ODIS intervals, **built** (2026-09-26); while the stopwatch is up the lever is read at 10 Hz, not paused, so LIMIT can end it |

[`09-bt-adapter.md`](../../.archive/specs/dash/09-bt-adapter.md) (archived) is superseded by this file (the wish is met by §3-B over USB and §8 over
BLE, not by Bluetooth SPP the C3 does not have). `13-screens.md` is the menu §5 draws from.

## 8. What is not decided here

- ~~The speed channel for the stopwatch~~ — decided 2026-09-13: `380B` (§6).
- ~~Whether `dev survey` (a sweep) may run through the board~~ — decided 2026-09-14: **no**.
  Through the board, `dev survey` and `units --identify <unit>` are refused before anything is
  opened; a sweep runs over the exclusive slcan mode, from a bench, with the guard the host has.
