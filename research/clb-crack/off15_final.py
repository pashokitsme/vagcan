#!/usr/bin/env python3
"""Decisive test: is off15 a per-channel counter-high-byte (function of off14),
or a content checksum? Tests across ALL channels + both directions."""
import sys, os
from collections import Counter, defaultdict
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames
from link_cipher import recover_all_channels, decrypt_link


def load(path):
    b8, b7 = [], []
    for f in reassemble_frames(path):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        rec = (f["first_idx"], bytes(p[1:17]))
        if p[0] == 0xB8 and f["dir"] == "OUT":
            b8.append(rec)
        elif p[0] == 0xB7 and f["dir"] == "IN":
            b7.append(rec)
    return b8, b7


def ck(b):
    return (b[0], b[2], b[3], b[5])


def main(path):
    b8, b7 = load(path)
    ks_map = recover_all_channels([(i, b) for i, b in b8], [(i, b) for i, b in b7])

    for label, frames in (("b8", b8), ("b7", b7)):
        chans = defaultdict(list)
        for i, b in frames:
            chans[ck(b)].append(b)
        print(f"\n===== {label}: {len(frames)} frames, {len(chans)} channels =====")

        # H1: off15 constant per channel?
        const_ch = sum(1 for bs in chans.values() if len({b[15] for b in bs}) == 1)
        print(f"  off15 CONSTANT per channel: {const_ch}/{len(chans)}")

        # H2: off15 a function of off14's top bit(s)? test masks
        for mask in (0x80, 0xC0, 0xE0, 0xF0):
            ok = 0
            for bs in chans.values():
                m = defaultdict(set)
                for b in bs:
                    m[b[14] & mask].add(b[15])
                if all(len(v) == 1 for v in m.values()):
                    ok += 1
            print(f"  off15 determined by (off14 & {mask:#04x}): {ok}/{len(chans)} channels")

        # H3: off15 determined by content (plain off6..13) ALONE? (needs KS)
        ok_content = 0
        tot_content = 0
        for c, bs in chans.items():
            if c not in ks_map:
                continue
            ks = ks_map[c][0]
            m = defaultdict(set)
            good = True
            for b in bs:
                dec = decrypt_link(b, ks)
                content = tuple(dec[6:14])
                if any(x is None for x in content):
                    good = False
                    break
                m[content].add(b[15])
            if not good:
                continue
            tot_content += 1
            if all(len(v) == 1 for v in m.values()):
                ok_content += 1
        print(f"  off15 determined by content(off6..13) alone: {ok_content}/{tot_content} channels")

        # H4: off15 determined by (content, off14&0xC0)?
        ok_both = 0
        tot_both = 0
        for c, bs in chans.items():
            if c not in ks_map:
                continue
            ks = ks_map[c][0]
            m = defaultdict(set)
            good = True
            for b in bs:
                dec = decrypt_link(b, ks)
                content = tuple(dec[6:14])
                if any(x is None for x in content):
                    good = False
                    break
                m[(content, b[14] & 0xC0)].add(b[15])
            if not good:
                continue
            tot_both += 1
            if all(len(v) == 1 for v in m.values()):
                ok_both += 1
        print(f"  off15 determined by (content, off14&0xC0): {ok_both}/{tot_both} channels")

    # detailed off15 value set per channel (b8), with off14 range
    print("\n===== per-channel off15 detail (b8) =====")
    chans = defaultdict(list)
    for i, b in b8:
        chans[ck(b)].append(b)
    for c, bs in sorted(chans.items(), key=lambda kv: -len(kv[1]))[:15]:
        o15 = Counter(b[15] for b in bs)
        o14 = sorted(set(b[14] for b in bs))
        print(f"  chan {c[0]:02x}{c[1]:02x}{c[2]:02x}{c[3]:02x} n={len(bs):3d} "
              f"off15={dict(o15)} off14_range=[{o14[0]:02x}..{o14[-1]:02x}] n14={len(o14)}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "../captures/reading-ecus.pcapng")
