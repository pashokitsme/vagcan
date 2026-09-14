# dash / 06 §7–§10 — questions for the sleep and power designs

Moved verbatim from `todo/dash/06-car-and-bench.md` on 2026-09-15. §7–§9 served the wake and
load-switch designs `07`/`08`, superseded on 2026-09-13 (the board takes power from OBD pin 1
with the ignition, and the deep-sleep module was deleted — `.archive/specs/dash/`). §10 was
settled on 2026-08-20. Kept for the reasoning; none of it is an open question now.

## 7. The frame that says the ignition is on

The wake discriminator for `05`, and the only way to get it is to listen. Three
**listen-only** captures with `vagcan dev sniff` — locked and parked, ignition on with the
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
