#!/usr/bin/env python3
import sys
from collections import Counter
import usbpcap
from link_cipher import two_time_pad

PATH = "../reading-ecus.pcapng"
frames = list(usbpcap.reassemble_frames(PATH))
def op(f): return f["payload"][0] if f["payload"] else None
b6 = [(i, bytes(f["payload"][1:])) for i,f in enumerate(frames) if op(f)==0xB6 and f["dir"]=="OUT"]

# (Q1) b6 nonce uniqueness + leading byte + entropy proxy (distinct byte count)
print("==== b6 nonce analysis ====")
print("count:", len(b6), " all distinct:", len(set(n for _,n in b6))==len(b6))
print("lengths:", Counter(len(n) for _,n in b6))
print("leading byte:", Counter(n[0] for _,n in b6))
# any nonce repeated?
c = Counter(n for _,n in b6)
print("repeats:", [x.hex() for x,k in c.items() if k>1] or "none")
for i,(fi,n) in enumerate(b6[:3]):
    print(f"  b6#{i+1} f{fi}: {n.hex()}")

# (Q3/Q4) ASCII readout of TP2.0/gateway low-ID channels via two-time-pad
def epoch_diag(lo,hi):
    req=[bytes(f["payload"][1:17]) for f in frames[lo:hi] if op(f)==0xB8 and len(f["payload"])>=17]
    rsp=[bytes(f["payload"][1:17]) for f in frames[lo:hi] if op(f)==0xB7 and len(f["payload"])>=17]
    return req,rsp
bounds=[i for i,_ in b6]+[len(frames)]
def ascii_of(bs): return "".join(chr(x) if 32<=x<127 else "." for x in bs)

for label,n in [("epoch2 (0x21f)",1),("epoch9 (0x201)",8),("epoch15 engine",14),("epoch25 gearbox",24),("epoch39 vspeed",38)]:
    lo,hi=bounds[n],bounds[n+1]
    req,rsp=epoch_diag(lo,hi)
    if not req or not rsp:
        print(f"\n-- {label}: req={len(req)} rsp={len(rsp)} (skip)"); continue
    modal=Counter(req).most_common(1)[0][0]
    print(f"\n-- {label}: modal_req={modal.hex(' ')}")
    # reassemble response data stream by two-time-pad (resp_plain ^ req_plain);
    # req_plain unknown, but resp_plain^req_plain still shows ASCII deltas poorly.
    # Instead: dump resp cipher^modal for off6..13 and its ASCII (best-effort).
    seen=Counter(two_time_pad(s,modal)[6:14] for s in rsp)
    for v,k in seen.most_common(6):
        print(f"    ttp6:14={v.hex(' ')}  ascii='{ascii_of(v)}'  n={k}")
