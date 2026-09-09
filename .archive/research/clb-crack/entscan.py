#!/usr/bin/env python3
"""entscan.py -- scan data sections for high-entropy constant blobs (candidate secrets).

Slides a window of length L (16/24/32) over .rdata and the raw part of .data,
computes Shannon entropy over the window, and reports windows that look like
random key material: high entropy, not ASCII, not mostly-zero, not mostly-FF.

usage: entscan.py [minent]
"""
import sys
import math
import pefile

EXE = "bin/VCDS-arm64-unpacked.exe"
IMAGE_BASE = 0x140000000
pe = pefile.PE(EXE)
with open(EXE, "rb") as f:
    DATA = f.read()

MINENT = float(sys.argv[1]) if len(sys.argv) > 1 else 3.5
LENGTHS = (16, 24, 32)


def shannon(b):
    if not b:
        return 0.0
    counts = [0] * 256
    for x in b:
        counts[x] += 1
    n = len(b)
    e = 0.0
    for c in counts:
        if c:
            p = c / n
            e -= p * math.log2(p)
    return e


def is_ascii_ish(b):
    printable = sum(1 for x in b if 0x20 <= x < 0x7f or x in (9, 10, 13))
    return printable >= len(b) * 0.9


def scan_section(name, vstart_rva, raw_off, raw_size):
    hits = []
    data = DATA[raw_off:raw_off + raw_size]
    for L in LENGTHS:
        step = 4  # blobs usually aligned; check every 4 bytes
        for i in range(0, len(data) - L, step):
            w = data[i:i + L]
            # reject mostly-zero / mostly-ff
            z = w.count(0)
            ff = w.count(0xff)
            if z > L * 0.4 or ff > L * 0.4:
                continue
            if is_ascii_ish(w):
                continue
            e = shannon(w)
            if e < MINENT:
                continue
            # distinct byte count as secondary signal
            distinct = len(set(w))
            vma = IMAGE_BASE + vstart_rva + i
            hits.append((e, distinct, L, vma, w))
    return hits


def main():
    all_hits = []
    for s in pe.sections:
        nm = s.Name.decode().strip("\x00")
        if nm not in (".rdata", ".data"):
            continue
        raw_size = s.SizeOfRawData
        all_hits += scan_section(nm, s.VirtualAddress, s.PointerToRawData, raw_size)
    # dedup overlapping windows: keep highest entropy per 4-byte start cluster is messy;
    # instead sort by entropy and print top windows, but suppress near-duplicate starts.
    all_hits.sort(key=lambda t: (-t[0], -t[1]))
    seen = []
    printed = 0
    for e, distinct, L, vma, w in all_hits:
        # skip if within 16 bytes of an already-printed hit of same/greater len
        if any(abs(vma - pv) < 8 for pv in seen):
            continue
        seen.append(vma)
        print(f"{vma:#012x}  L={L:2d}  ent={e:.3f}  distinct={distinct:3d}  {w.hex()}")
        printed += 1
        if printed >= 60:
            break


if __name__ == "__main__":
    main()
