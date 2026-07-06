#!/usr/bin/env python3
"""nine_analysis.py -- exhaustive 0x09 keyed-exchange analysis (offline).

Dumps every 0x09 OUT/IN pair across reading-ecus.pcapng + init-only.pcapng,
grouped by b6 epoch, and tests structural / keyed hypotheses.
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import usbpcap
from collections import defaultdict, Counter

CAPS = [
    ("reading-ecus", "../captures/reading-ecus.pcapng"),
    ("init-only", "../captures/init-only.pcapng"),
]


def op(f):
    return f["payload"][0] if f["payload"] else None


def load(path):
    return list(usbpcap.reassemble_frames(path))


def epoch_bounds(frames):
    b6 = [i for i, f in enumerate(frames) if op(f) == 0xB6 and f["dir"] == "OUT"]
    return b6


def dump_nine(name, frames):
    b6 = epoch_bounds(frames)
    print(f"\n{'='*72}\n{name}: {len(frames)} frames, {len(b6)} b6 events\n{'='*72}")
    bounds = b6 + [len(frames)]

    # Build a seq index over 0x09 frames only, tagged with epoch.
    def which_epoch(i):
        # epoch 0 = before first b6 (bring-up); epoch k = between b6[k-1] and b6[k]
        e = 0
        for k, bi in enumerate(b6):
            if i >= bi:
                e = k + 1
        return e

    nine = []
    for i, f in enumerate(frames):
        if op(f) == 0x09:
            nine.append((i, f["dir"], f["payload"], which_epoch(i)))

    print(f"total 0x09 frames: {len(nine)}")
    # group by epoch, pair OUT->IN
    by_ep = defaultdict(list)
    for i, d, p, e in nine:
        by_ep[e].append((i, d, p))

    pairs = []  # (epoch, idx_byte, out7, in7, frame_i)
    for e in sorted(by_ep):
        lst = by_ep[e]
        print(f"\n-- epoch {e} (frames {bounds[e-1] if e>0 else 0}..) --")
        # walk, pairing each OUT with the next IN
        pending = None
        for i, d, p in lst:
            if d == "OUT":
                # OUT: 09 <idx> <7 bytes>  => payload = 09 idx b0..b6  (len 9)
                idx = p[1] if len(p) > 1 else None
                out7 = bytes(p[2:9])
                pending = (i, idx, out7)
                print(f"   f{i:5d} OUT idx={idx:02x} out7={out7.hex()}  (len={len(p)})")
            else:
                in7 = bytes(p[1:8])
                print(f"   f{i:5d} IN            in7={in7.hex()}  (len={len(p)})")
                if pending:
                    fi, idx, out7 = pending
                    pairs.append((e, idx, out7, in7, fi))
                    pending = None
    return pairs, b6


def analyze_structure(name, pairs):
    print(f"\n### STRUCTURE: {name} ###")
    # group pairs by epoch, then by idx
    by_ep = defaultdict(list)
    for e, idx, out7, in7, fi in pairs:
        by_ep[e].append((idx, out7, in7, fi))
    for e in sorted(by_ep):
        lst = by_ep[e]
        print(f"\nepoch {e}: {len(lst)} pairs")
        # per-position constancy of IN across this epoch
        ins = [in7 for _, _, in7, _ in lst]
        outs = [out7 for _, out7, _, _ in lst]
        idxs = [idx for idx, _, _, _ in lst]
        print(f"  idx seq: {[f'{x:02x}' for x in idxs]}")
        for pos in range(7):
            invals = set(b[pos] for b in ins)
            outvals = set(b[pos] for b in outs)
            tagi = "CONST" if len(invals) == 1 else f"var{len(invals)}"
            tago = "CONST" if len(outvals) == 1 else f"var{len(outvals)}"
            print(f"   pos{pos}: IN {tagi:6s} {{{' '.join(f'{v:02x}' for v in sorted(invals))}}}   "
                  f"OUT {tago:6s} {{{' '.join(f'{v:02x}' for v in sorted(outvals))}}}")


def test_transforms(pairs):
    """Test simple/keyed OUT->IN transforms across ALL pairs."""
    print(f"\n### TRANSFORM TESTS (all {len(pairs)} pairs) ###")
    # 1. any position-wise fixed XOR mask (out^in const across pairs)?
    masks = [bytes(a ^ b for a, b in zip(out7, in7)) for _, _, out7, in7, _ in pairs]
    mc = Counter(masks)
    print(f"  distinct OUT^IN masks: {len(mc)} / {len(pairs)}")
    # per-position xor const?
    for pos in range(7):
        s = set(m[pos] for m in masks)
        if len(s) == 1:
            print(f"   pos{pos}: OUT^IN CONST = {list(s)[0]:02x}  <-- fixed!")
    # 2. per-position add const?
    for pos in range(7):
        s = set((in7[pos] - out7[pos]) & 0xff for _, _, out7, in7, _ in pairs)
        if len(s) == 1:
            print(f"   pos{pos}: IN-OUT CONST = {list(s)[0]:02x} (mod 256)")


def cross_ref(all_pairs):
    """Same idx byte across captures -- any shared structure?"""
    print(f"\n### CROSS-CAPTURE by idx ###")
    by_idx = defaultdict(list)
    for name, e, idx, out7, in7, fi in all_pairs:
        by_idx[idx].append((name, e, out7, in7))
    for idx in sorted(by_idx):
        lst = by_idx[idx]
        print(f"\nidx {idx:02x}: {len(lst)} pairs")
        for name, e, out7, in7 in lst:
            print(f"   [{name:12s} ep{e}] OUT {out7.hex()}  IN {in7.hex()}")


if __name__ == "__main__":
    all_pairs = []
    for name, path in CAPS:
        frames = load(path)
        pairs, b6 = dump_nine(name, frames)
        analyze_structure(name, pairs)
        test_transforms(pairs)
        for e, idx, out7, in7, fi in pairs:
            all_pairs.append((name, e, idx, out7, in7, fi))
    cross_ref(all_pairs)
