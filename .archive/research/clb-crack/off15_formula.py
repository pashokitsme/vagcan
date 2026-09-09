#!/usr/bin/env python3
"""Nail the exact off15 = f(off14, channel) closed form."""
import sys, os
from collections import defaultdict
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames


def load(path):
    b8, b7 = [], []
    for f in reassemble_frames(path):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        rec = bytes(p[1:17])
        if p[0] == 0xB8 and f["dir"] == "OUT":
            b8.append(rec)
        elif p[0] == 0xB7 and f["dir"] == "IN":
            b7.append(rec)
    return b8, b7


def ck(b):
    return (b[0], b[2], b[3], b[5])


def main(path):
    b8, b7 = load(path)
    chans = defaultdict(list)
    for b in b8 + b7:
        chans[ck(b)].append(b)

    # Build LUT per channel: (off14>>5) -> off15
    print("# per-channel LUT: off14_top3 (0..7) -> off15 ; test off15 = KS15 ^ g(top3)")
    # Hypotheses for g(top3): value derived from top3 bits.
    # Try to find a SINGLE global function g and per-channel KS15 such that
    #   off15 = KS15 ^ g(top3).
    luts = {}
    for c, bs in chans.items():
        lut = {}
        ok = True
        for b in bs:
            t = b[14] >> 5
            if t in lut and lut[t] != b[15]:
                ok = False
            lut[t] = b[15]
        luts[c] = (lut, ok)

    # Check: within each channel, is off15 ^ off15_at_top3=0 a function of top3
    # that is the SAME across channels? i.e. delta(top3) = off15(top3) ^ off15(ref)
    # Collect delta patterns.
    print("\n# delta LUT per channel: g(top3) = off15(top3) ^ off15(min top3 present)")
    delta_patterns = defaultdict(list)
    for c, (lut, ok) in luts.items():
        base_t = min(lut)
        base = lut[base_t]
        deltas = {t: (v ^ base) for t, v in lut.items()}
        key = tuple(sorted(deltas.items()))
        delta_patterns[key].append((c, base_t, base, lut))

    for key, members in sorted(delta_patterns.items(), key=lambda kv: -len(kv[1])):
        print(f"\n  delta pattern {dict(key)}  ({len(members)} channels)")
        for c, base_t, base, lut in members[:4]:
            print(f"    chan {c[0]:02x}{c[1]:02x}{c[2]:02x}{c[3]:02x}: "
                  f"lut(top3->off15)={ {t: f'{v:02x}' for t,v in sorted(lut.items())} }")

    # Direct hypothesis test: off15 = KS15 ^ (top3 mapped via g). Try g candidates.
    print("\n# GLOBAL closed-form search: off15 = KS15_chan ^ g(top3)")
    def g_ident(t): return t
    def g_bitrev3(t): return int(f"{t:03b}"[::-1], 2)
    def g_shift5(t): return t << 5
    def g_shift5rev(t): return int(f"{t:03b}"[::-1], 2) << 5
    cands = {
        "top3": g_ident,
        "bitrev3(top3)": g_bitrev3,
        "top3<<5": g_shift5,
        "bitrev3(top3)<<5": g_shift5rev,
    }
    for name, g in cands.items():
        good = 0
        tot = 0
        for c, (lut, ok) in luts.items():
            tot += 1
            # solve KS15 from first entry, verify all
            t0 = next(iter(lut))
            ks15 = lut[t0] ^ g(t0)
            if all((ks15 ^ g(t)) == v for t, v in lut.items()):
                good += 1
        print(f"  g = {name:20s}: {good}/{tot} channels fit (per-chan KS15)")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "../captures/reading-ecus.pcapng")
