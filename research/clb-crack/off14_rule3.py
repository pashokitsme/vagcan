#!/usr/bin/env python3
"""off14_rule3.py -- dump ALL b8/b7/b9 frames in a temporal window, to see what
the cable pushed right before the host's 0x39 auth-completion b8."""
import sys
from usbpcap import reassemble_frames

PATH = "../captures/reading-ecus.pcapng"
LO = int(sys.argv[1]) if len(sys.argv) > 1 else 3800
HI = int(sys.argv[2]) if len(sys.argv) > 2 else 4600

rows = []
for f in reassemble_frames(PATH):
    op = f["payload"][:1].hex()
    if op not in ("b8", "b7", "b9", "a0", "fe"):
        continue
    idx = f["first_idx"]
    if not (LO <= idx <= HI):
        continue
    pl = f["payload"]
    blk = pl[1:17] if len(pl) >= 17 else pl[1:]
    off14 = blk[14] if len(blk) >= 16 else None
    chan = blk[0] if len(blk) >= 1 else None
    rows.append((idx, f["dir"], op, chan, off14, pl.hex()))

rows.sort(key=lambda r: r[0])
for idx, d, op, chan, off14, hexp in rows:
    c = f"{chan:02x}" if chan is not None else "--"
    o = f"{off14:02x}" if off14 is not None else "--"
    print(f"  {idx:7d} {d:3s} {op} chan={c} off14={o}  {hexp}")
