#!/usr/bin/env python3
"""can_addr.py -- decode the b3/b4 CAN acceptance-filter payloads per b6 epoch
and correlate to the diagnostic channel that answered. Interpret the 16-bit
filter register as an 11-bit CAN ID left-justified (ID = value>>5)."""
import sys
from collections import defaultdict
import usbpcap

PATH = sys.argv[1] if len(sys.argv) > 1 else "../reading-ecus.pcapng"
frames = list(usbpcap.reassemble_frames(PATH))
def op(f): return f["payload"][0] if f["payload"] else None
b6_idx = [i for i, f in enumerate(frames) if op(f) == 0xB6 and f["dir"] == "OUT"]
bounds = b6_idx + [len(frames)]

def burst_before(bi):
    out = {}
    j = bi - 1
    while j >= 0:
        f = frames[j]; o = op(f)
        if f["dir"] == "OUT" and o in (0xB3, 0xB4, 0xB5):
            idx = f["payload"][1]
            out.setdefault(o, {})[idx] = f["payload"][2:]
        elif f["dir"] == "OUT" and o in (0xB0, 0xB1, 0xB2, 0xB6):
            pass
        elif f["dir"] == "IN":
            pass
        else:
            break
        j -= 1
    return out

def id11(fourbytes):
    v = int.from_bytes(fourbytes[:2], "big")
    return v >> 5

# Known VAG UDS-on-CAN 11-bit IDs (MQB physical request 0x700+id, resp 0x700+id+8 style varies)
KNOWN = {
    0x7E0: "ECU01 engine req", 0x7E8: "ECU01 engine resp",
    0x7E1: "ECU02 gearbox req", 0x7E9: "ECU02 gearbox resp",
    0x710: "ECU19 gateway req", 0x77A: "ECU19 gateway resp",
    0x714: "ECU03 ABS req", 0x77E: "ECU03 ABS resp",
    0x712: "ECU17 dash req", 0x77C: "ECU17 dash resp",
    0x715: "ECU09 cent-elec req", 0x77F: "ECU09 cent-elec resp",
    0x711: "ECU25? req",
    0x7DF: "functional broadcast",
    0x700: "diag broadcast base",
}

for n in range(len(b6_idx)):
    lo, hi = bounds[n], bounds[n + 1]
    burst = burst_before(b6_idx[n])
    b4 = burst.get(0xB4, {})
    b3 = burst.get(0xB3, {})
    # channel headers that answered (b7 responses) in this epoch
    resp_hdrs = set()
    req_hdrs = set()
    for f in frames[lo:hi]:
        o = op(f); p = f["payload"]
        if o in (0xB7, 0xB8) and len(p) >= 17:
            h = (p[1], p[3], p[5])
            (resp_hdrs if o == 0xB7 else req_hdrs).add(h)
    ids = []
    for idx in sorted(b4):
        i11 = id11(b4[idx])
        raw = int.from_bytes(b4[idx][:2], "big")
        tag = KNOWN.get(i11, "")
        ids.append(f"b4[{idx}]={raw:04x}->{i11:#05x}{('['+tag+']') if tag else ''}")
    b3s = " ".join(f"b3[{i}]={int.from_bytes(b3[i][:2],'big'):04x}->{id11(b3[i]):#05x}" for i in sorted(b3))
    rq = ",".join(f"{h[0]:02x}" for h in sorted(req_hdrs))
    rs = ",".join(f"{h[0]:02x}" for h in sorted(resp_hdrs))
    print(f"epoch{n+1:2d} f{b6_idx[n]:4d} | {b3s} | {' '.join(ids)} | req_off0={rq} rsp_off0={rs}")
