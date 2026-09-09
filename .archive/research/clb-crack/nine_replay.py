#!/usr/bin/env python3
"""nine_replay.py -- replay-determinism probes.
A) For each distinct 0x0b OUT command, do repeats give the SAME 41-byte IN?
   (If IN varies for a fixed OUT -> cable-internal counter/nonce -> not pure replay.)
B) Which diag channel opens right after each b6, and do the 0x09-burst b6 events
   coincide with a NEW channel-group vs re-opens?
C) OUT-byte entropy: do the 0x09 OUT 7-byte fields look random or carry a counter?
Offline only.
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import usbpcap
from collections import defaultdict, Counter

CAPS = [("reading-ecus", "../captures/reading-ecus.pcapng"), ("init-only", "../captures/init-only.pcapng")]


def op(f):
    return f["payload"][0] if f["payload"] else None


for name, path in CAPS:
    frames = list(usbpcap.reassemble_frames(path))
    print(f"\n{'='*72}\n{name}\n{'='*72}")

    # (A) 0x0b OUT->IN determinism
    zpairs = []
    pend = None
    for i, f in enumerate(frames):
        if op(f) != 0x0B:
            continue
        p = bytes(f["payload"])
        if f["dir"] == "OUT":
            pend = (i, p)
        elif pend:
            zpairs.append((pend[1], p)); pend = None
    bycmd = defaultdict(list)
    for o, ii in zpairs:
        bycmd[o].append(ii)
    print(f"(A) 0x0b: {len(zpairs)} OUT/IN pairs, {len(bycmd)} distinct OUT cmds")
    for cmd, ins in sorted(bycmd.items()):
        distinct = len(set(ins))
        print(f"   cmd {cmd.hex()}  n={len(ins)}  distinct_IN={distinct}"
              f"  {'DETERMINISTIC' if distinct==1 else 'VARIES->cable-state/nonce'}")

    # (C) 0x09 OUT entropy: per-position distinct values across all OUT
    nout = [bytes(f["payload"][2:9]) for f in frames if op(f) == 0x09 and f["dir"] == "OUT"]
    print(f"(C) 0x09 OUT ({len(nout)} frames) per-position distinct byte counts:")
    for pos in range(7):
        vals = [b[pos] for b in nout]
        print(f"    pos{pos}: distinct={len(set(vals))}/{len(vals)}")

# (B) channel-open map for reading-ecus
print(f"\n{'='*72}\n(B) channel opened after each b6 (reading-ecus)\n{'='*72}")
frames = list(usbpcap.reassemble_frames("../captures/reading-ecus.pcapng"))
b6 = [i for i, f in enumerate(frames) if op(f) == 0xB6 and f["dir"] == "OUT"]
nine_b6 = {36, 544, 1379, 1850}
bounds = b6 + [len(frames)]
for n in range(len(b6)):
    lo, hi = bounds[n], bounds[n + 1]
    # first diag channel header (b8/b7) in this window
    chans = []
    for f in frames[lo:hi]:
        if op(f) in (0xB8, 0xB7) and len(f["payload"]) >= 17:
            b = f["payload"][1:17]
            chans.append(b[0])
    first = chans[0] if chans else None
    has09 = "**0x09-BURST**" if b6[n] in nine_b6 else ""
    csum = Counter(chans)
    print(f"  b6#{n+1:2d} f{b6[n]:5d}: first_chan_off0={first if first is None else f'{first:02x}'}"
          f"  n_diag={len(chans)}  chans={[f'{c:02x}' for c in sorted(csum)]}  {has09}")
