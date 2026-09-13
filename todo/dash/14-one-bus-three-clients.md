# dash / 14 — one bus, three clients: the consolidated design

**Subsystem:** dash · **Crates:** `vag-dash-fw`, `vag-uds-transport`, `vag-uds-can`, `vag-cli-core` ·
**Date:** 2026-09-13 · **Status:** design, for the owner's decision on §3

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
| **stopwatch** | the speed channel while armed | maximum, single DID, ≈100 Hz | high |
| **laptop** | whole UDS PDUs the host sends (§3) | as they arrive | normal, interleaved |

**Budget and degradation (owner, 2026-09-13: a small delay is fine, the bus is shared
with the whole car, a run needs speed fast).** The scheduler runs under a **ceiling of
≈100 exchanges a second** — half of what the one conversation could do — so the gateway
and the units see a sparse, even trickle from us. Default rates: temperatures 2 Hz,
boost and revs 10 Hz, stalks 2/20 Hz by the cruise gate (§6a); a four-cell page is ≈25
exchanges a second, a quarter of the budget. **Stopwatch armed**: speed at 50 Hz (20 ms
between points, interpolation gives hundredths), every other source at 1 Hz, stalks
paused. When the sum asks for more than the ceiling, nothing is dropped: sources are
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

## 3. The fork: how the laptop talks to the board

### Option A — the board is an slcan adapter (raw frames)

The board speaks slcan over USB; `SlcanBackend` drives it unchanged; zero host code. This
is what `todo/dash/09` asked for and what the `slcan` binary now under construction
delivers. **It cannot share the bus with the panel.** The host sends raw frames on its own
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

- The `slcan` binary (in progress on branch `slcan`) becomes the **exclusive adapter
  mode**, kept, documented as such. Not the answer to wish 1.
- Wish 1 is Option B: `BoardTransport` on the host, the scheduler on the board.
- One link, typed messages, replaces the ad-hoc `FRAME …` lines `dash` prints for `dashsim`.

## 4. The link: one protocol, two carriers

A framed, typed message stream, the same over **USB-Serial-JTAG** and over **BLE NUS**
(`11-ble.md` measured BLE cannot carry a loaded *bus*; it can carry PDUs at `watch` rates —
50 Hz × ~12 bytes is a kilobyte a second, and that measurement stands to be made):

| message | direction | carries |
|---|---|---|
| `pdu` | both | `(unit address, UDS PDU)` — request up, answer down; the board tags answers with the request's id |
| `frame` | down | a raw CAN frame the board saw, for `dev sniff` — on request, listen-only |
| `image` | down | the panel's pixels, what `FRAME …` is today, for `dashsim` |
| `button` / `page` | up | what the phone or laptop pressed, so `dashsim` keeps driving the board |
| `config` | both | what `dashcfg` moves today over BLE (`12`) — folded in, one protocol |
| `log` | down | firmware log lines, so the console is no longer a mix of logs and data |

Framing: COBS or a length prefix plus a CRC-16, whichever `postcard`'s ecosystem already
does in `no_std`; `serde` on both ends, the message enum in **one crate both build**
(`vag-dash-link`, `no_std` + `alloc` off), the way the plan's types are shared now.

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

- **Source**: one speed channel, chosen in `dash.toml` from `13-screens.md` — `2033`
  (0.01 km/h, declared) or `F40D` (1 km/h, standard); ESC wheel speeds if they answer.
  The choice is the owner's; the standard one is the safe default.
- **Arming**: speed at 0 for a second arms it; the first sample above 0 starts the clock
  (with the half-sample correction `vag-cli-measure` uses — check `session.rs`); crossing
  60 and 100 km/h stamps the two times, interpolated between the samples either side.
- **Rate**: the scheduler gives the speed DID its high-priority slot (≈100 Hz single-DID);
  the other cells drop to background rate for the run.
- **Display**: the two times large; the chart page of the run shows speed on time, the
  same chart widget as `kind = "chart"`, with the x axis being seconds since launch.
- **Record**: the run's samples go up the link as `pdu` answers anyway, so a laptop that
  is connected gets the full trace for `vagcan measure`'s report; the board keeps the last
  run's two numbers in settings (`12`).

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

**What it costs, and the adaptive rate.** One `22 1105` exchange is two frames, ≈0.5 ms
of a 500 kbit/s bus: 20 Hz is 1 % of the diagnostic CAN (which carries nothing else of
the car's) and 1 % of the comfort bus behind the gateway. The bus is not the limit; the
board's one conversation is (≈4 ms per exchange with the gateway in the path, so
≈200–250 a second for everything). The stalk poll adapts to the gate: cruise ON → 2 Hz,
enough to notice the switch going off; OFF → 20 Hz, a 50 ms button; stopwatch armed →
paused, the slot goes to speed. On average that is a few exchanges a second (owner's
concern, 2026-09-13).

To verify on the car first, one `watch` on `70C`: that `1105` answers; that the lever
fields move within a poll; that OFF reads as its own value and not as absent. If the
ON/OFF on this stalk turns out momentary rather than latched, the cruise state comes from
the engine instead (`7E0` carries the GRA status beside `2018`), and the gate is that.

## 7. Order, and what each needs

| # | item | needs | moves |
|---|---|---|---|
| 1 | chart/page defect, with a `dashsim` repro | bench | 5 |
| 2 | `vag-dash-link` crate + `image`/`log`/`button` over it; `dashsim` on the link; `dash` stops printing `FRAME` | bench | 4 |
| 3 | scheduler in `dash`: sources, rates, shared answers | bench | 2 |
| 4 | `pdu` message + `BoardTransport` on the host; `watch` through the board on the bench (CANable answering as a mock unit is not possible — the check is the car) | bench, then car | 3 |
| 5 | `slcan` binary lands as the exclusive mode (branch `slcan`) | bench | 3-A |
| 6 | stopwatch page | car, one straight road | 6 |
| 7 | ~~`frame` mirror for `dev sniff` over the link~~ — dropped 2026-09-13 (owner): sniffing through the board is mode 2 over the cable | — | — |
| 8 | ~~the same link over BLE NUS; measure the PDU rate~~ → [`16-uds-over-ble.md`](16-uds-over-ble.md): UDS over BLE as a slow transport, after the merge | bench, then car | 4 |
| 9 | OLED on the carrier | bench | `05`/`08` |

[`09-bt-adapter.md`](../../.archive/specs/dash/09-bt-adapter.md) (archived) is superseded by this file (the wish is met by §3-B over USB and §8 over
BLE, not by Bluetooth SPP the C3 does not have). `13-screens.md` is the menu §5 draws from.

## 8. What is not decided here

- The speed channel for the stopwatch (owner, from `13`).
- Whether `dev survey` (a sweep) may run through the board at all. A sweep is the most
  invasive thing the tool does, and the board lives in the car; the safe default is **no** —
  the exclusive slcan mode is for that, from a bench, with the guard the host has.
