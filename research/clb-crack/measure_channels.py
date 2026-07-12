#!/usr/bin/env python3
"""Census every wire channel: PCI histogram, multi-frame presence, TP-crib status,
and two-time-pad DID clusters, to be sure no wide-range (RPM/speed) DID is hiding
on a non-TP channel or in multi-frame responses."""
import sys, os
from collections import defaultdict, Counter
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure_coolant as M


def main():
    b8, b7 = M.load()
    chreq, chrsp = defaultdict(list), defaultdict(list)
    for t, b in b8:
        chreq[M.ck(b)].append((t, b))
    for t, b in b7:
        chrsp[M.ck(b)].append((t, b))
    print("# channel census (req/resp counts, TP crib?, resp PCI hist after decode)")
    for c in sorted(chrsp, key=lambda c: -len(chrsp[c])):
        reqs = [b for _, b in chreq.get(c, [])]
        rsp = chrsp[c]
        ks = M.tp_ks(reqs) if reqs else None
        hdr = "%02x%02x%02x%02x" % c
        if ks:
            # decode responses, PCI + SID histogram
            pci = Counter(); sids = Counter(); dids = Counter()
            multiframe = 0
            for t, b in rsp:
                d = bytes(b[i] ^ ks.get(i, 0) for i in range(16))
                pci[d[6] >> 4] += 1
                if d[6] >> 4 in (1, 2):
                    multiframe += 1
                sid = d[7]
                sids[sid] += 1
                if sid == 0x62:
                    dids[(d[8] << 8) | d[9]] += 1
            print(f"  {hdr} req={len(reqs)} rsp={len(rsp)} TP-crib=YES "
                  f"PCItype={dict(pci)} multiframe={multiframe}")
            print(f"       SIDs={ {'%02x'%k:v for k,v in sids.items()} } "
                  f"DIDs={ {'%04X'%k:v for k,v in dids.items()} }")
        else:
            # two-time-pad: cluster positive responses by echoed DID cipher
            modal = Counter(reqs).most_common(1)[0][0] if reqs else None
            info = ""
            if modal is not None:
                pos = [(t, r) for t, r in rsp if (r[7] ^ modal[7]) == 0x40]
                cl = defaultdict(list)
                for t, r in pos:
                    cl[(r[8], r[9])].append((t, bytes([r[10] ^ modal[10], r[11] ^ modal[11]])))
                clusters = {k: len(v) for k, v in cl.items() if len(v) >= 8}
                # value range per cluster (u16be)
                rng = {}
                for k, v in cl.items():
                    if len(v) >= 8:
                        vals = [(d[0] << 8) | d[1] for _, d in v]
                        rng["%02x%02x" % k] = (min(vals), max(vals), len(set(vals)))
                info = f"ttp-clusters(n>=8)={len(clusters)} u16be-ranges={rng}"
            print(f"  {hdr} req={len(reqs)} rsp={len(rsp)} TP-crib=NO   {info}")


if __name__ == "__main__":
    main()
