# vag-data-labels

Static VAG diagnostic data: parsers, decoders, and lookup that turn Ross-Tech VCDS
label files into human-meaningful measurement names, units, and ranges.

## Modules

- **`label`** — parser for the plaintext `.lbl` VCDS label format (ISO-8859-1, CRLF,
  `;`-comments). Handles measuring-value labels
  (`block,field,name,location,description` with `Range:`/unit extraction), `REDIRECT`,
  `A###` adaptation channels, `LC` long-coding, and keeps any other record kind verbatim.
- **`clb`** — decrypts the compiled `.clb` label format (**TEA-CBC**, key `KEY_CLB`,
  per-record IV), then feeds the plaintext through the `label` parser. See "Ciphers" below.
- **`rod`** — decodes the `.rod` UDS/ODX format (`UDS_EV/`): section framing
  (`[CMP]/[ADP]/[MWB]/…`) + **TEA-CBC** (`KEY_ROD`, section-tag IV) + zlib inflate for the
  compressed sections. `decode_rod(&[u8]) -> Vec<RodSection>`.
- **`tea`** — shared TEA primitives used by `clb` and `rod`.
- **`db`** — `LabelDb`: resolves an ECU part number to its label file through `REDIRECT`
  chains (exact + `?`-wildcard, most-specific-wins, cycle-guarded) and looks up measurements
  by `(part_no, block, field)`. Empty-placeholder measurements are filtered here.
  Lookups are indexed at build time (exact-selector `HashMap`, length-bucketed wildcards,
  per-file `(block, field)` maps) and resolutions are memoized: ~30 ns memoized / ~0.6 µs
  cold per lookup on a 2,900-file label files (`cargo bench -p vag-data-labels --bench lookup`).
- **`label files`** — `load_label_files(dir)`: walk a `Labels/` dir, parse `.lbl`, decrypt+parse
  `.clb`, into a `Vec<LabelFile>`.

## Binary

```
# parse+decrypt a Labels dir into a JSON label files + coverage summary
cargo run -p vagcan -- vcds label files /path/to/VCDS/Labels --out label files.json

# resolve a part number to its measurements
cargo run -p vagcan -- vcds labels /path/to/VCDS/Labels --part 06F-906-056-AXW
```

Against the reference install (~2884 files): **1202 `.lbl` + 1627 `.clb` all parse**;
`.lbl` alone yields 42,738 measurements / 9,168 adaptations / 4,795 long-codings / 3,739
redirects, and every `.clb` now decrypts through the same parser.

## Ciphers (reverse-engineered for interoperability)

The compiled formats are not documented by Ross-Tech; the algorithms below were recovered
from an unpacked build of the VCDS binary to read the user's own vehicle's label data.

- **`.clb` (modern)** — TEA (32-round, `DELTA=0x9E3779B9`), CBC, `KEY_CLB`, a per-record IV
  derived from a file constant + record index. (Legacy pre-VCDS-11.3 `.clb` used a different
  keystream cipher; our files are the modern one.)
- **`.rod`** — same TEA in CBC with `KEY_ROD`; IV seeded from the section tag; compressed
  sections (MWB/ADP/…) are zlib-deflated under the encryption.
- The `KEY_ROD` IV also mixes two 256-byte tables (`rod_mt.bin`, `rod_ks.bin`) embedded in
  the crate.

### Known gap (documented, not built)
`.rod` MWB rows are `<6-digit measurement id>,<code>` — the UDS/ODX measurement **index**.
The human-readable names live in `TTTEXT.ROD` (same cipher) and require joining on those IDs.
That TTText name-resolution layer, plus the per-record IV "product" term for the small subset
of records that need it (one runtime memory dump), are future work — `.rod` is a standalone
decoder here and is not ingested into `LabelDb`/the SQLite label files. Readable measurement names
for MQB engines already come from the cracked `.clb` files, so this gap is a bonus layer, not
a blocker.
