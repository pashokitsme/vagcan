#!/usr/bin/env python3
"""Understand off14 advance + off15 rule; verify encode reproduces captures."""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames

KS_F3 = {1: 0xBD, 6: 0x02, 7: 0xA9, 8: 0x99, 9: 0xF6, 10: 0xDA, 11: 0x7C, 12: 0x9C, 13: 0x3A, 14: 0x00}


def load(path):
    out = []
    for f in reassemble_frames(path):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        if p[0] in (0xB8, 0xB7):
            out.append((f["first_idx"], "b8" if p[0] == 0xB8 else "b7", bytes(p[1:17])))
    return out


def is_f3(b):
    return b[0] == 0xF3 and b[2] == 0x44 and b[3] == 0xDD and b[5] == 0x5F


def f3_off15(off14):
    """Empirical f3 LUT: off15 = fd when off14 top3 in {4,5,7} else fc."""
    return 0xFD if (off14 >> 5) in (4, 5, 7) else 0xFC


def main(path):
    frames = load(path)
    f3 = [(i, d, b) for i, d, b in frames if is_f3(b)]
    print("# f3 b8/b7 interleaved (idx dir off14 off15):")
    for i, d, b in f3[:30]:
        print(f"  {i:7d} {d} off14={b[14]:02x} off15={b[15]:02x}")

    # verify f3_off15 rule against ALL f3 frames (b8 and b7)
    bad = sum(1 for _, _, b in f3 if f3_off15(b[14]) != b[15])
    print(f"\n# f3_off15(off14) rule: {len(f3)-bad}/{len(f3)} f3 frames match "
          f"(both directions)")

    # Verify the reference blocks encode correctly.
    TP = bytes.fromhex("f38344dd7c5f009799f6da7c9c3a00fc")
    RDBI = bytes.fromhex("f39f44dd7c5f018bedaeda7c9c3afbfd")
    for name, blk in (("TP", TP), ("RDBI", RDBI)):
        off14 = blk[14]
        pred = f3_off15(off14)
        print(f"  {name}: off14={off14:02x} captured off15={blk[15]:02x} "
              f"predicted={pred:02x} {'OK' if pred==blk[15] else 'MISMATCH'}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "../reading-ecus.pcapng")
