#!/usr/bin/env python3
"""nine_correlate.py -- where do 0x09 events sit relative to b6 and the
diagnostic channels? Are triplets per-b6 or rarer? Correlate the pos3 tag with
the epoch's recovered keystream. Offline only.
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import usbpcap, link_cipher
from collections import defaultdict, Counter

CAPS = [("reading-ecus", "../captures/reading-ecus.pcapng"), ("init-only", "../captures/init-only.pcapng")]


def op(f):
    return f["payload"][0] if f["payload"] else None


for name, path in CAPS:
    frames = list(usbpcap.reassemble_frames(path))
    b6 = [i for i, f in enumerate(frames) if op(f) == 0xB6 and f["dir"] == "OUT"]
    nine_out = [i for i, f in enumerate(frames) if op(f) == 0x09 and f["dir"] == "OUT"]
    print(f"\n{'='*72}\n{name}\n{'='*72}")
    print(f"b6 indices ({len(b6)}): {b6}")
    print(f"0x09 OUT indices ({len(nine_out)}): {nine_out}")

    # For each 0x09 OUT, show idx byte + how many b6 precede it, and nearest b6 delta
    print("\n0x09 OUT context (idx, #b6 before, nearest prior b6 delta, prior op window):")
    for i in nine_out:
        idxb = frames[i]["payload"][1]
        nb = sum(1 for x in b6 if x < i)
        prior_b6 = max([x for x in b6 if x < i], default=None)
        d = (i - prior_b6) if prior_b6 is not None else None
        # what non-fe ops in the 6 frames before?
        ctx = []
        for j in range(max(0, i - 5), i):
            o = op(frames[j])
            if o is not None:
                ctx.append(f"{frames[j]['dir'][0]}{o:02x}")
        print(f"  f{i:5d} idx={idxb:02x}  b6_before={nb:2d}  prior_b6=f{prior_b6} (d={d})  ctx={ctx}")

    # b6 that have NO 0x09 nearby (within +/- 20 frames)?
    b6_with_nine = set()
    for i in nine_out:
        for x in b6:
            if abs(x - i) <= 40:
                b6_with_nine.add(x)
    print(f"\nb6 events with a 0x09 within +/-40 frames: {len(b6_with_nine)} / {len(b6)}")
    print(f"  -> {sorted(b6_with_nine)}")

    # Correlate pos3 tag with recovered keystream for the epoch.
    # Recover all channel keystreams over the WHOLE capture (as link_cipher does).
    b8 = [(f["first_idx"], bytes(f["payload"][1:17])) for f in frames
          if op(f) == 0xB8 and f["dir"] == "OUT" and len(f["payload"]) >= 17]
    b7 = [(f["first_idx"], bytes(f["payload"][1:17])) for f in frames
          if op(f) == 0xB7 and f["dir"] == "IN" and len(f["payload"]) >= 17]
    allch = link_cipher.recover_all_channels(b8, b7)
    print(f"\nrecovered channels: {len(allch)}")
    # collect all keystream bytes seen
    ks_bytes = set()
    for k, (ks, pci, sid, nq, ns) in allch.items():
        for off, v in ks.items():
            ks_bytes.add(v)
    # tags per triplet
    nine_in = [(i, bytes(frames[i]["payload"][1:8])) for i in range(len(frames))
               if op(frames[i]) == 0x09 and frames[i]["dir"] == "IN"]
    tags = Counter(in7[3] for _, in7 in nine_in)
    print(f"0x09 IN pos3 tags: {[f'{t:02x}(x{n})' for t,n in tags.most_common()]}")
    print(f"  any tag appears as a recovered keystream byte? "
          f"{ {f'{t:02x}' for t in tags} & {f'{v:02x}' for v in ks_bytes} }")
