#!/usr/bin/env python3
"""rod_name_harvest.py -- lift the RESIDENT decoded VAG label/measurement name
table out of a running VCDS-RUS minidump, WITHOUT solving the .rod per-record
`product` term.

Background
----------
`.rod`/`TTTEXT.ROD` sections are TEA-CBC/KEY_ROD. Each section's first cipher
block needs an 8-byte IV whose bytes [3:8] carry a per-record `product` term
(low 5 bytes of a 64-bit char-product of a runtime-only "record-string"). When
`product != 0` the first block is corrupt and, for zlib sections like
`TTTEXT.ROD [TXT]`, the whole inflate dies -- so the name table cannot be
decoded OFFLINE (see research/rod-measurement-feasibility.md).

BUT a live VCDS-RUS process that has already resolved names keeps the DECODED
name strings resident in the heap as an array of fixed 72-byte records. This
scanner lifts them directly -- no cipher, no product needed. The strings are
CP1251 (Russian localisation, from `TTTEXT-RUS`).

Record layout (empirically, VCDS-RUS x86; 72 bytes / record)
------------------------------------------------------------
    off +40 : constant tag dword   a4 92 57 00   (the MARK below)
    off +44 : u32 length of the inline name string
    off +56 : inline CP1251 name bytes (length as above), NUL-padded

The array sits right after a resident `TTTEXT-RUS\0` filename string in the
heap. We locate every record by its constant tag dword, read the length, and
decode the inline name. This recovered ~8.3k unique names from one dump and
~11.5k unique across five (see research/rod-product-term-dump.md).

Usage
-----
    rod_name_harvest.py <dump.dmp> [--min-len 1] [--cyrillic-only] [--out names.txt]

NOTE: minidumps are PII (contain VIN/telemetry). Do NOT commit dumps or their
raw output. This script is the reusable method; run it against your own dump.
"""
import sys, os, mmap, struct, argparse

MARK = bytes.fromhex("a4925700")   # constant record tag dword @ record+40
LEN_OFF = 4                        # u32 name length,   relative to MARK
NAME_OFF = 16                      # inline name bytes, relative to MARK


def _looks_texty(nm: bytes) -> bool:
    """All bytes printable ASCII or CP1251 letters (Cyrillic incl. Ё/ё)."""
    if not nm:
        return False
    for b in nm:
        if 0x20 <= b < 0x7F:            # ASCII printable
            continue
        if 0xC0 <= b <= 0xFF:           # CP1251 Cyrillic А-я
            continue
        if b in (0xA8, 0xB8):           # Ё, ё
            continue
        return False
    return True


def harvest(mm, min_len=1, max_len=64):
    """Yield decoded (CP1251) name strings for every resident record."""
    i = mm.find(MARK)
    n = len(mm)
    while i != -1:
        if i + NAME_OFF + max_len <= n:
            ln = struct.unpack_from("<I", mm, i + LEN_OFF)[0]
            if min_len <= ln <= max_len:
                nm = mm[i + NAME_OFF:i + NAME_OFF + ln]
                if _looks_texty(nm):
                    yield nm.decode("cp1251", errors="replace")
        i = mm.find(MARK, i + 1)


def _is_cyrillic(s: str) -> bool:
    return any(0x0410 <= ord(c) <= 0x044F or c in "Ёё" for c in s)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dump")
    ap.add_argument("--min-len", type=int, default=1)
    ap.add_argument("--cyrillic-only", action="store_true")
    ap.add_argument("--out")
    a = ap.parse_args()

    with open(a.dump, "rb") as f:
        mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
        names = list(harvest(mm, min_len=a.min_len))
        mm.close()

    if a.cyrillic_only:
        names = [n for n in names if _is_cyrillic(n)]

    uniq = sorted(set(names))
    print(f"# {os.path.basename(a.dump)}: {len(names)} records, "
          f"{len(uniq)} unique names", file=sys.stderr)
    if a.out:
        with open(a.out, "w", encoding="utf-8") as f:
            f.write("\n".join(uniq) + "\n")
        print(f"# wrote {len(uniq)} names -> {a.out}", file=sys.stderr)
    else:
        for n in uniq:
            print(n)


if __name__ == "__main__":
    main()
