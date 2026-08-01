# Measurement catalogs

Measurement definitions are **data**, not code. Each row is
`(read identifier, raw byte form, factor, offset, unit)` — everything needed to
turn a UDS response into an engineering value — and adding a measurement means
adding a row here, never a new branch in Rust.

## Where these came from

**Measured on the car, not read out of a label file.** The `.rod` route to
scaling is a dead end: the read identifier is not stored in `STRUC` in any
encoding, which was established against ground truth rather than assumed
(`research/rod-labels.md` §4.0c).

Instead the linear rows were fitted by `vagcan analyse`, which crosses a passive
CAN capture against a VCDS log recorded at the same moment, aligns them by
wall-clock arithmetic, and accepts only exact linear relations
(`R² ≥ 0.995` over ≥ 20 points and ≥ 4 distinct raw values); every linear row
fitted at `R² = 1.00000`. The exceptions carry their own evidence: the gear and
selector enums were identified by arithmetic against the proven shaft speeds
(`research/gearbox-state.md`), the odometer by exact hit against a logged
reading. `vagcan calibrate` extends coverage by fitting raw columns against
already-trusted references in the same `watch --out` recording.

Rows are named in this project's own words. The VCDS display strings are
Ross-Tech's localised label text and are not reproduced; names are meant to
come from the label corpus.

## Files

| file | control unit | rows |
|---|---|---|
| `engine-01.json` | Engine, `8V0906264H` (1.8 TFSI, MQB) | VW-specific identifiers |
| `gearbox-02.json` | Gearbox, `0CW300041G` (DQ200) | VW-specific identifiers |
| `cluster-17.json` | Instrument cluster, `5E0 920 740 D` | VW-specific identifiers |

Not measurement catalogs, but kept alongside them:

| file / dir | what |
|---|---|
| `names-uds.json` | 17,009 measurement names recovered from `TTTEXT.ROD` (`research/tttext-codec.md`), keyed by the corpus's 6-digit text-id — **not** by data identifier; the corpus holds no name→DID join. Searched by `vagcan names`. |
| `rod-iv-cache.json` | Recovered `.rod` per-record IVs. Written by the `vag-rod` binary (`cargo run -p vag-data --features rod-crack --bin vag-rod <file.rod>`), read by `vagcan labels`. |
| `label-cache/` | SQLite caches of the parsed label corpus, one per corpus directory (so the English and the Russian VCDS installs each keep their own; switching language = pointing at the other directory). Created by `vagcan labels`; `--refresh` rebuilds. |

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
