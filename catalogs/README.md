# Measurement catalogs

**A catalog is named after the control unit it describes**, by the part number that
unit reports for itself (`F187`) or its ODX file name (`F19E`) — not by a unit number
like `01`. A scaling belongs to the unit it was measured on: `0x202A` is boost pressure
on engine `8V0906264H` and means nothing in particular elsewhere. Files live under
`catalogs/vehicles/` and are read at run time, so a car this project has never seen
finds no file and shows raw bytes rather than being given another car's numbers.

Measurement definitions are **data**, not code. Each row is
`(read identifier, raw byte form, factor, offset, unit)` — everything needed to
turn a UDS response into an engineering value — and adding a measurement means
adding a row here, never a new branch in Rust.

## Where these came from

**Measured on the car, not read out of a label file.** The `.rod` route to
scaling is a dead end: the read identifier is not stored in `STRUC` in any
encoding, which was established against ground truth rather than assumed
(`research/labels/rod-labels.md` §4.0c).

Instead the linear rows were fitted by `vagcan vcds analyse`, which crosses a passive
CAN capture against a VCDS log recorded at the same moment, aligns them by
wall-clock arithmetic, and accepts only exact linear relations
(`R² ≥ 0.995` over ≥ 20 points and ≥ 4 distinct raw values); every linear row
fitted at `R² = 1.00000`. The exceptions carry their own evidence: the gear and
selector enums were identified by arithmetic against the proven shaft speeds
(`research/car/gearbox-state.md`), the odometer by exact hit against a logged
reading. `vagcan recording calibrate` extends coverage by fitting raw columns against
already-trusted references in the same `watch --out` recording.

**The instrument-cluster rows added on 2026-08-02 came from a third route:**
three whole-car surveys — one parked, two taken while driving
(`research/dumps/survey-parked.jsonl`, `survey-driving-20260802-0314.jsonl`,
`-0322.jsonl`) — crossed against each other and against the host's own clock.
Three snapshots is one point more than a line needs, so each row below carries
a check it could have failed:

| row | check | what would have refuted it |
|---|---|---|
| `22B8` odometer, metres | `⌊22B8 / 1000⌋` equals the `2203` odometer in **all three** snapshots (212 805 188 → 212 805; 212 810 125 → 212 810), and its fractional part rises monotonically | any snapshot where the two disagreed; a 24-bit or little-endian reading breaks all three |
| `22D2` road speed, km/h | between the two driving reads, `22B8` says the car covered 4 856 m in the 497 s that `2216` says elapsed — mean 35 km/h, bracketed by the 5 and 53 km/h read at the ends | ×0.01 makes that drive 49 m; ×10 makes it 49 km |
| `2238`/`2239`/`223A`/`223B`/`223C` clock | they are byte-for-byte the fields of the block identifiers `2216` (`hh mm ss`) and `2217` (`yyyy mm dd`), and the assembled time lands within 4–7 s of when the host wrote each survey file. `2216` advanced **497 s** between the two driving sweeps; the host's file timestamps are **497 s** apart | a different byte order (the parked read would be hour 32); any of the three times missing its window |

The cluster's date is set four days slow (it reads 2026-07-28 for 2026-08-01),
which is the car's business and not a decoding error — the offset is constant
across both snapshots and the day rolled over correctly at midnight.

Rows are named in this project's own words. The VCDS display strings are
Ross-Tech's localised label text and are not reproduced; names are meant to
come from the label corpus.

## Files

| file | control unit | rows |
|---|---|---|
| `vehicles/8V0906264H.json` | Engine, 1.8 TFSI MQB | VW-specific identifiers |
| `vehicles/0CW300041G.json` | Gearbox, DQ200 | VW-specific identifiers |
| `vehicles/5E0920740D.json` | Instrument cluster | VW-specific identifiers |

No file exists for the brakes (`5Q0614517AQ`), body control (`5Q0937084CF`),
parking aid (`5QA919283A`), climate (`5E0907044AM`) or the doors
(`5Q4959393E`/`5Q4959392E`). Identifiers on those units *did* move between the
parked and driving surveys, but none of them could be pinned to an engineering
value against a reference, and a plausible-looking wrong scaling is worse than
none. In particular the climate unit's `F405` is **not** the engine's coolant
temperature, despite sitting at the OBD-II mirror address: no linear map takes
its three readings (87, 90, 109) to the engine's own PID-05 readings (129, 93,
137) at the same three moments, and the two move in opposite directions
overnight. Its `F40C` is one byte where SAE J1979 PID `0C` is two. So this
unit's `F4xx` block is not a faithful mirror and must not be read as one.

`vagcan sensors` now enforces that. It converts only on the emissions-related
units ISO 15765-4 addresses (`0x7E0..0x7E7`) — a property of the protocol, not
of this car — and only where the answer is the width J1979 defines. Both gates
are needed: the climate unit's `F405` is one byte, the right width for the
wrong quantity, and the gearbox is inside the ISO block yet answers `F40D` with
two little-endian bytes where PID `0D` is one. Anything refused is still shown,
as bytes, with the reason.

Not measurement catalogs, but kept alongside them:

| file / dir | what |
|---|---|
| `names-uds.json` | 17,009 measurement names recovered from `TTTEXT.ROD` (`research/labels/tttext-codec.md`), keyed by the corpus's 6-digit text-id — **not** by data identifier; the corpus holds no name→DID join. Searched by `vagcan vcds names`. |
| `rod-iv-cache.json` | Recovered `.rod` per-record IVs. Written by `cargo run -p vagcan --features rod-crack -- vcds rod <file.rod>`, read by `vagcan vcds labels`. |
| `label-cache/` | SQLite caches of the parsed label corpus, one per corpus directory (so the English and the Russian VCDS installs each keep their own; switching language = pointing at the other directory). Created by `vagcan vcds labels`; `--refresh` rebuilds. |

The **standard OBD-II parameters** are not duplicated here. They live in
`vag_data::obd` because they are defined by SAE J1979 rather than measured —
and five of them were independently confirmed against this car, which is why
the rest of that family is trusted.

## A caveat worth keeping

`F40D` means different things on the two units: one byte of km/h on the engine
(the OBD-II mirror) and two little-endian bytes at ×0.01 on the gearbox. That
is why the catalogs are per-unit and must not be merged.

These rows are proven for **this vehicle's control units**. Another car's
identifiers have to be proven the same way, not assumed.
