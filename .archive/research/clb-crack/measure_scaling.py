#!/usr/bin/env python3
"""Extract per-DID RDBI response time-series from capture-w-logs.pcapng and fit
scaling raw->engineering against the VCDS ADVMB CSV logs (ground truth).

Stage 1 (this dump): per fully-decodable measurement DID, the ordered sequence
of raw response data bytes (after the 62 DIDhi DIDlo echo).
"""
import sys, os
from collections import defaultdict, Counter
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames

CAP = os.path.join(os.path.dirname(__file__), "../dumps/capture-w-logs.pcapng")
TP = bytes([0x02, 0x3E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
OKPCI = {0x02, 0x03, 0x04, 0x05, 0x06, 0x07}
OKSID = {0x3E, 0x22, 0x19, 0x2E}

def load(path):
    b8, b7 = [], []
    for f in reassemble_frames(path):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        blk = bytes(p[1:17])
        if p[0] == 0xB8 and f["dir"] == "OUT":
            b8.append((f["first_idx"], blk))
        elif p[0] == 0xB7 and f["dir"] == "IN":
            b7.append((f["first_idx"], blk))
    return b8, b7

def ck(b): return (b[0], b[2], b[3], b[5])

def tp_ks(reqs):
    uniq = [b for b, _ in Counter(reqs).most_common()]
    for cand in uniq:
        ks = {6 + i: cand[6 + i] ^ TP[i] for i in range(8)}
        if all((b[6] ^ ks[6]) in OKPCI and (b[7] ^ ks[7]) in OKSID for b in uniq):
            return ks
    return None

def dec(b, ks): return bytes(b[i] ^ ks.get(i, 0) for i in range(16))

def main():
    b8, b7 = load(CAP)
    print(f"# {CAP}: {len(b8)} req / {len(b7)} resp blocks")
    chreq = defaultdict(list); chrsp = defaultdict(list)
    for idx, b in b8: chreq[ck(b)].append((idx, b))
    for idx, b in b7: chrsp[ck(b)].append((idx, b))
    # per-DID ordered response data (single-frame RDBI: 62 DIDhi DIDlo d0 d1 d2..)
    series = defaultdict(list)  # did -> [(idx, data_bytes)]
    for c, reqs in chreq.items():
        ks = tp_ks([b for _, b in reqs])
        if ks is None:
            continue
        rsp = chrsp.get(c, [])
        for idx, b in rsp:
            d = dec(b, ks)
            pci = d[6]
            if d[7] != 0x62:      # not an RDBI positive response
                continue
            did = (d[8] << 8) | d[9]
            n = pci & 0x0F        # single-frame length
            data = d[10:7 + n] if pci & 0xF0 == 0x00 else d[10:14]
            series[did].append((idx, bytes(data)))
    print(f"# decodable measurement DIDs: {sorted('%04X' % d for d in series)}")
    for did in sorted(series):
        s = series[did]
        vals = Counter(v.hex(' ') for _, v in s)
        print(f"\n## DID {did:04X}: {len(s)} responses, {len(vals)} distinct data values")
        for hexv, n in vals.most_common(8):
            print(f"   {hexv}  x{n}")

if __name__ == "__main__":
    main()
