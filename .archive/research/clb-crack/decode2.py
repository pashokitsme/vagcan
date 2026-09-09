#!/usr/bin/env python3
import sys
from collections import Counter, defaultdict
import usbpcap
from link_cipher import decrypt_link, two_time_pad, recover_channel_ks

PATH = "../captures/reading-ecus.pcapng"
frames = list(usbpcap.reassemble_frames(PATH))
def op(f): return f["payload"][0] if f["payload"] else None
b6_idx = [i for i, f in enumerate(frames) if op(f) == 0xB6 and f["dir"] == "OUT"]
bounds = b6_idx + [len(frames)]

def epoch_diag(n):
    lo, hi = bounds[n], bounds[n+1]
    req = [bytes(f["payload"][1:17]) for f in frames[lo:hi] if op(f)==0xB8 and len(f["payload"])>=17]
    rsp = [bytes(f["payload"][1:17]) for f in frames[lo:hi] if op(f)==0xB7 and len(f["payload"])>=17]
    return req, rsp

# ---- (A) decode epoch 2 (0x9e / target 0x21f) ----
print("==== EPOCH 2 (target 0x21f, off0=9e req / 9f rsp) ====")
req, rsp = epoch_diag(1)
modal = Counter(req).most_common(1)[0][0]
print("modal req :", modal.hex(" "))
print("distinct req:", len(set(req)), " distinct rsp:", len(set(rsp)))
# try TesterPresent crib (PCI 02 SID 3E) and RDBI (03/22)
for pci, sid, label in [(0x02,0x3E,"TP"),(0x03,0x22,"RDBI"),(0x10,0x10,"?"),(0x02,0x10,"StartDiag?")]:
    ks = recover_channel_ks(modal, pci=pci, sid=sid)
    dec = [decrypt_link(b, ks) for b in req]
    okpci = sum(1 for d in dec if d[6]==pci and d[7]==sid)
    print(f"  crib PCI={pci:02x} SID={sid:02x} ({label}): {okpci}/{len(req)} reqs match")
# two-time-pad responses vs modal request to read response DATA
print("  -- response data via two-time-pad (resp ^ modal_req), off6..13 --")
vals = Counter(two_time_pad(s, modal)[6:14] for s in rsp)
for v, n in vals.most_common(8):
    print(f"     {v.hex(' ')}   n={n}")

# ---- decode all epochs that have responses: guess ECU by UDS ----
print("\n==== per-epoch modal request/response (two-time-pad readout) ====")
for n in range(len(b6_idx)):
    req, rsp = epoch_diag(n)
    if not req: continue
    modal = Counter(req).most_common(1)[0][0]
    off0 = modal[0]
    line = f"epoch{n+1:2d} off0={off0:02x} nreq={len(req)} nrsp={len(rsp)}"
    if rsp:
        # response data with keystream cancelled where request had 0x00 pad
        ttp = Counter(two_time_pad(s, modal)[6:14] for s in rsp).most_common(1)[0][0]
        line += f"  rsp^req[6:14]={ttp.hex(' ')}"
    print(line)

# ---- (B) 0x0b indexed-block analysis ----
print("\n==== 0x0b indexed 40-byte blocks (epoch 1 pre-auth) ====")
ob = [(i, bytes(f["payload"][1:])) for i,f in enumerate(frames) if op(f)==0x0B and f["dir"]=="IN"]
print("total 0b IN blocks:", len(ob), " lengths:", Counter(len(b) for _,b in ob))
# group by the preceding OUT 0b idx
def idx_of(i):
    # the OUT 0b right before this IN
    for j in range(i-1, max(0,i-3), -1):
        f=frames[j]
        if op(f)==0x0B and f["dir"]=="OUT":
            return f["payload"][1]
    return None
byidx = defaultdict(list)
for i,b in ob:
    byidx[idx_of(i)].append(b)
for k in sorted(x for x in byidx if x is not None):
    blocks = byidx[k]
    n = len(blocks)
    # are they identical across the repeated bursts?
    uniq = len(set(blocks))
    print(f"  idx{k:02x}: {n} occurrences, {uniq} distinct")
# show first 3 idx00 blocks to eyeball structure
print("  first idx00 blocks:")
for b in byidx.get(0, [])[:4]:
    print("    ", b.hex(" "))
