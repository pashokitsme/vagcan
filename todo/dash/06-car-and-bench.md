# dash / 06 — what only the car can answer

**Subsystem:** dash · **Needs the car:** yes — this whole file

Everything here gates `05`, and none of it can be settled from the catalogs. Read
`CLAUDE.md`'s safety section before any of it. Nothing in this list is a sweep: every identifier named
below is one the catalog declares for this car's own variants.

## 1. How many identifiers per `0x22` request

The first number wanted. The survey records this car answering batched reads
(`"batched": true`, `research/dumps/survey-parked.jsonl`), but not the limit. It decides
whether an alarm costs one exchange or three, and therefore what poll rate the panel can
hold. Measure on engine `0x7E0` and gearbox `0x7E1` separately — they need not agree.

## 2. Does `0x4E60` answer, and does it mean what it says

`IDE10634 Clutch_protection_function_clutch_slip_speed`, on the **engine**, in 1/min. If
it answers, "the difference between gearbox and engine speed" is one number from one unit
and the two-clock problem disappears. Verify against the fallback during a shift: engine
`206E` minus gearbox `380A` should track it, allowing for the clocks being different —
which is precisely why the single channel is wanted.

Declared but unanswered is an ordinary outcome: of 2,251 identifiers ODIS declares for
this car, 505 answer.

## 3. The retard and misfire channels, live

`200A`–`200D` (°) and `291D`–`2920`. Confirm they answer, confirm the sign convention —
"retard" should go negative under load — and get a feel for the resting values, because
`04`'s misfire threshold cannot be guessed from a catalog.

## 4. Is boost absolute or gauge

`202A` in bar. VAG usually reports absolute, in which case idle reads ~1.0 and not 0, and
a panel that shows it raw will confuse anyone who has read a boost gauge before. Settle it
by reading at idle with the engine warm; if absolute, the plan carries the offset, and
ambient pressure is its own channel if a true gauge reading is wanted.

## 5. A live DSG temperature on a dry DQ200

There is no clutch oil on this box, so the photograph's "DQ250 oil 72°" has no equivalent.
`028D` is the mechatronic temperature. The clutch temperature model (`IDE80032`) appears
in the declared set only inside snapshot blocks; find out whether a live channel exists,
by reading what is declared — not by looking for one that is not.

## 6. What poll rate the panel actually reaches

With the real request set (a page plus two armed alarms) over the real cable. This decides
the chart's window: one pixel per poll over 190 px is ~19 s at 10 Hz and ~63 s at 3 Hz,
and the number is drawn on the screen, so it has to be true.

## 7. The frame that says the ignition is on

The wake discriminator for `05`, and the only way to get it is to listen. Three
**listen-only** captures with `vagcan sniff` — locked and parked, ignition on with the
engine off, engine running — and the difference of the ID sets. Nothing is transmitted,
so this is as safe as bench work gets; `vag-uds-can` has had the silent mode since the
sniffer landed (`SlcanMode::Silent`).

What is wanted is a frame that is *cyclic* and present only from ignition on, so its
absence for N seconds is a reliable "the car has gone". Record the period as well as the
ID — the timeout in `05` is a multiple of it, not a guessed constant.

Half the answer is already in the catalog: `118A` on the gearbox is a bitfield of
statuses for the CAN messages it receives from the engine — `Motor_04`, `Motor_07`,
`Motor_11`, `Motor_12`, `Motor_14`, `Motor_16`, `Motor_17`, `Motor_18`, `Motor_20`,
`Motor_26`, `Motor_35`, `Motor_Code_01`. The names of the cyclic frames are known; their
bus identifiers are what the sniff supplies.

`research/dumps/bus-idle.jsonl` does **not** serve here: 47 lines of one extended frame
repeated, which is a capture that went wrong rather than an idle bus. A fresh one is
needed.

## 8. The CANable's idle current

An ammeter in series with its 5 V, board idle, two minutes. It decides whether the load
switch in `08` is mandatory and whether `07`'s wake had to move off the bus — both already
designed as though the answer is "tens of milliamps", which wants confirming.

Take a second reading with the ESP32 asleep and the CANable switched off, against the
under-1 mA target.

## 9. The rail, parked and running

For `07`'s wake threshold: the resting voltage at the socket after a night, and the
charging voltage warm. 13 V is the assumed line between them and it is worth seeing the
real numbers on this car before it is fixed in firmware.

## 10. The panel's controller

Settled 2026-08-20 without needing the car: buy the 3.12″ **256×64 SSD1322**, 7-pin SPI.
256×32 is not a part anybody sells. Details and the buying checklist are in the subsystem
README.
