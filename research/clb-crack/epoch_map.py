#!/usr/bin/env python3
"""epoch_map.py -- segment reading-ecus.pcapng by b6 epoch, dump the b0..b5
addressing burst that precedes each b6, and the diagnostic channel headers that
follow. Clean-room interop research only.
"""
import sys
from collections import defaultdict, Counter
import usbpcap

PATH = sys.argv[1] if len(sys.argv) > 1 else "../reading-ecus.pcapng"
frames = list(usbpcap.reassemble_frames(PATH))


def op(f):
    return f["payload"][0] if f["payload"] else None


# Find all b6 frames (OUT)
b6_idx = [i for i, f in enumerate(frames) if op(f) == 0xB6 and f["dir"] == "OUT"]
print(f"total frames={len(frames)}  b6 count={len(b6_idx)}")
print("b6 frame indices:", b6_idx[:50])

# For each b6, print the preceding b0..b5 burst payloads and the b6 nonce.
def burst_before(bi):
    out = []
    j = bi - 1
    # walk back over IN acks and OUT b0..b5
    while j >= 0:
        f = frames[j]
        o = op(f)
        if f["dir"] == "OUT" and o in (0xB0, 0xB1, 0xB2, 0xB3, 0xB4, 0xB5):
            out.append((o, f["payload"][1:].hex()))
        elif f["dir"] == "IN" and o == 0xFE:
            pass
        elif f["dir"] == "OUT" and o in (0xB0, 0xB1, 0xB2, 0xB3, 0xB4, 0xB5, 0xB6):
            pass
        else:
            if f["dir"] == "OUT" and o not in (0xFE,):
                break
        j -= 1
    return list(reversed(out))


for n, bi in enumerate(b6_idx[:6]):
    print(f"\n==== b6 #{n+1} at frame {bi} ====")
    for o, pl in burst_before(bi):
        print(f"   b{o & 0xf} {pl}")
    print(f"   b6 nonce = {frames[bi]['payload'][1:].hex()}")

# Channel headers per epoch: cluster diag frames (b8/b7) by (off0,off2,off3,off5)
# within each epoch [b6_idx[n], b6_idx[n+1]).
print("\n\n==== channel headers per epoch ====")
bounds = b6_idx + [len(frames)]
for n in range(min(6, len(b6_idx))):
    lo, hi = bounds[n], bounds[n + 1]
    chans = defaultdict(lambda: [0, 0])
    for f in frames[lo:hi]:
        o = op(f)
        p = f["payload"]
        if o in (0xB8, 0xB7) and len(p) >= 17:
            b = p[1:17]
            key = (b[0], b[2], b[3], b[4], b[5])
            if o == 0xB8:
                chans[key][0] += 1
            else:
                chans[key][1] += 1
    print(f"\n-- epoch {n+1} (frames {lo}..{hi}) --")
    for key, (nq, ns) in sorted(chans.items()):
        print(
            f"   chan off0={key[0]:02x} off2={key[1]:02x} off3={key[2]:02x} "
            f"off4={key[3]:02x} off5={key[4]:02x}   b8req={nq} b7rsp={ns}"
        )
