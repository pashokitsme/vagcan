# vag-data

Static VAG diagnostic data: parsers and tables that turn raw ECU bytes and
Ross-Tech VCDS label files into human-meaningful names, units, and ranges.

## What P2 delivers

- `label` module — parser for the **plaintext `.lbl`** VCDS label format
  (ISO-8859-1, CRLF, `;`-comments). Handles measuring-value labels
  (`block,field,name,location,description` with `Range:`/unit extraction),
  `REDIRECT`, `A###` adaptation channels, `LC` long-coding, and keeps any other
  record kind verbatim.
- `vag-labels` binary — walks a VCDS `Labels/` directory, parses every `.lbl`
  into a structured JSON corpus, and prints a coverage summary.

Against the reference install (VCDS-RUS, 2884 files): **1202 `.lbl` files parse
into 42,738 measurements, 9,168 adaptation labels, 4,795 long-coding labels,
3,739 redirects.**

```
cargo run -p vag-data --bin vag-labels -- /path/to/VCDS/Labels --out corpus.json --summary
```

## Known gap: the `.clb` format (follow-up RE task)

1627 of the 2829 label files (57%) are **compiled/encrypted `.clb`**, not plaintext.
Critically, **the MQB-era engine labels we need for the Octavia mk3 (04E, 06K, 8V0,
5G0, 5Q0, …) ship ONLY as `.clb`** — none have a plaintext `.lbl`.

Investigation so far:
- `.clb` is a binary container. Byte 0 is `0x00`, byte 1 looks like a length/id.
- XOR of two different `.clb` files zeros out their shared leading bytes and ~31%
  of the whole file — i.e. it is a **fixed keystream XOR'd across all files**, not
  per-file random encryption. That makes it recoverable via known-plaintext /
  crib-dragging, but it is a dedicated reverse-engineering task (like the cable),
  not a quick parse.
- There is no clean known-plaintext pair in the corpus (only one part number
  exists as both `.lbl` and `.clb`, and those two hold different content), so the
  keystream must be recovered by crib-dragging common label text (`Range:`,
  `Engine Speed`, `RPM`, the copyright header) against the ciphertext.

This is tracked as a P2-follow-up; the plaintext parser above stands on its own and
covers the older KWP-era ECUs today.
