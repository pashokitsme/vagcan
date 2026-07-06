#!/usr/bin/env python3
"""nine_final.py -- final probes on the 0x09 exchange.
1) keystream byte saturation (is the tag<->keystream 'match' just noise?)
2) within-burst structure across the idx 05/02/03 triplet
3) is IN pos3 truly independent of OUT? (constant across 3 different OUTs)
4) 0x0b EEPROM burst that wraps each 0x09 triplet
Offline only.
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
    print(f"\n{'='*72}\n{name}\n{'='*72}")

    # --- (1) keystream saturation ---
    b8 = [(f["first_idx"], bytes(f["payload"][1:17])) for f in frames
          if op(f) == 0xB8 and f["dir"] == "OUT" and len(f["payload"]) >= 17]
    b7 = [(f["first_idx"], bytes(f["payload"][1:17])) for f in frames
          if op(f) == 0xB7 and f["dir"] == "IN" and len(f["payload"]) >= 17]
    allch = link_cipher.recover_all_channels(b8, b7)
    ksb = set()
    for k, (ks, pci, sid, nq, ns) in allch.items():
        ksb.update(ks.values())
    print(f"(1) recovered keystream distinct byte values: {len(ksb)}/256 "
          f"-> P(random tag hits) ~ {len(ksb)/256:.2f}  (saturation => tag match is noise)")

    # --- (2)+(3) burst structure ---
    # collect 0x09 frames in order with dir
    nine = [(i, frames[i]["dir"], bytes(frames[i]["payload"])) for i in range(len(frames))
            if op(frames[i]) == 0x09]
    # pair OUT/IN, group into bursts (contiguous runs separated by >10 frame gap)
    pairs = []
    pend = None
    for i, d, p in nine:
        if d == "OUT":
            pend = (i, p[1], bytes(p[2:9]))
        else:
            if pend:
                pairs.append((pend[0], pend[1], pend[2], bytes(p[1:8])))
                pend = None
    # group bursts by frame proximity
    bursts = []
    cur = []
    for pr in pairs:
        if cur and pr[0] - cur[-1][0] > 15:
            bursts.append(cur); cur = []
        cur.append(pr)
    if cur: bursts.append(cur)
    print(f"(2) {len(bursts)} bursts (triplet groups); showing structure:")
    for bi, bu in enumerate(bursts):
        idxs = [f"{x[1]:02x}" for x in bu]
        in_pos3 = set(x[3][3] for x in bu)
        print(f"  burst{bi}: fstart={bu[0][0]} idx={idxs} INpos3={{{' '.join(f'{v:02x}' for v in sorted(in_pos3))}}} (n={len(bu)})")
        for fi, idxb, o7, i7 in bu:
            print(f"      idx{idxb:02x}  OUT {o7.hex()}  IN {i7.hex()}")
        # cross-idx OUT/IN xor within burst
        if len(bu) >= 3:
            o = [x[2] for x in bu[:3]]
            ii = [x[3] for x in bu[:3]]
            print(f"      OUT[0]^OUT[1]={bytes(a^b for a,b in zip(o[0],o[1])).hex()}  "
                  f"IN[0]^IN[1]={bytes(a^b for a,b in zip(ii[0],ii[1])).hex()}")

    # --- (4) 0x0b EEPROM frames wrapping the bursts ---
    zb = [(i, frames[i]["dir"], bytes(frames[i]["payload"])) for i in range(len(frames))
          if op(frames[i]) == 0x0B]
    print(f"(4) 0x0b frames: {len(zb)}  (dir/len):",
          Counter((d, len(p)) for _, d, p in zb).most_common())
    # show 0b OUT payloads distinctness
    zb_out = [p for _, d, p in zb if d == "OUT"]
    zb_in = [p for _, d, p in zb if d == "IN"]
    print(f"    0b OUT distinct={len(set(zb_out))}/{len(zb_out)}  IN distinct={len(set(zb_in))}/{len(zb_in)}")
    if zb_in:
        # is the 40-byte 0b IN constant (fixed cable EEPROM) across the whole capture?
        print(f"    0b IN all-equal? {len(set(zb_in))==1}   first IN={zb_in[0].hex()}")
