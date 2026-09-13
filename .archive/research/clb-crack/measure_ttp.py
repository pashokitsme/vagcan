#!/usr/bin/env python3
"""Two-time-pad extraction of EVERY RDBI DID's response DATA, TP-crib or not.

A wire channel (grouped by header bytes 0,2,3,5) multiplexes many DIDs at block
offsets 8..9. An RDBI poll request is `.. PCI 22 DIDhi DIDlo 00 00 00 00 ..` so the
data region (offsets 10..13) is ISO-TP padding 0x00; the per-channel keystream
there equals the modal request cipher, and response data decodes by pure XOR.

We can't read the DID *value* without ks[8..9], but every response to the same DID
shares the same cipher[8..9] (fixed keystream x fixed echoed DID), so clustering
responses by (cipher[8],cipher[9]) separates DIDs. That yields a per-DID data
time-series for alignment even on channels with no TesterPresent crib.

If a channel also carries TesterPresent (crib recoverable) we additionally label
the real DID value.
"""
import sys, os
from collections import defaultdict, Counter
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames

CAP = os.path.join(os.path.dirname(__file__), "../dumps/capture-w-logs.pcapng")
TP = bytes([0x02, 0x3E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])
OKPCI = {0x02, 0x03, 0x04, 0x05, 0x06, 0x07}
OKSID = {0x3E, 0x22, 0x19, 0x2E}


def ck(b):
    return (b[0], b[2], b[3], b[5])


def tp_ks(reqs):
    for cand in [b for b, _ in Counter(reqs).most_common()]:
        ks = {6 + i: cand[6 + i] ^ TP[i] for i in range(8)}
        if all((b[6] ^ ks[6]) in OKPCI and (b[7] ^ ks[7]) in OKSID
               for b in set(reqs)):
            return ks
    return None


def load():
    b8, b7 = [], []
    for f in reassemble_frames(CAP):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        blk = bytes(p[1:17])
        if p[0] == 0xB8 and f["dir"] == "OUT":
            b8.append((f["t"], blk))
        elif p[0] == 0xB7 and f["dir"] == "IN":
            b7.append((f["t"], blk))
    return b8, b7


def extract_all():
    """Return list of dicts: {chan, echo, did(or None), n, series[(t,(d0,d1))]}.
    One entry per (channel, DID-echo cluster)."""
    b8, b7 = load()
    chreq, chrsp = defaultdict(list), defaultdict(list)
    for t, b in b8:
        chreq[ck(b)].append(b)
    for t, b in b7:
        chrsp[ck(b)].append((t, b))
    out = []
    for c, reqs in chreq.items():
        rsp = chrsp.get(c, [])
        if len(rsp) < 8:
            continue
        modal = Counter(reqs).most_common(1)[0][0]
        ks_tp = tp_ks(reqs)  # may be None
        # keep only positive-response frames (resp[7]^modal[7]==0x40 -> 22->62)
        pos = [(t, r) for t, r in rsp if (r[7] ^ modal[7]) == 0x40]
        if len(pos) < 8:
            continue
        ks10, ks11 = modal[10], modal[11]   # padding-crib keystream (data region)
        clusters = defaultdict(list)
        for t, r in pos:
            clusters[(r[8], r[9])].append((t, bytes([r[10] ^ ks10, r[11] ^ ks11])))
        for echo, ser in clusters.items():
            if len(ser) < 8:
                continue
            did = None
            if ks_tp is not None:
                did = ((echo[0] ^ ks_tp[8]) << 8) | (echo[1] ^ ks_tp[9])
            out.append({"chan": c, "echo": echo, "did": did,
                        "n": len(ser), "series": sorted(ser)})
    return out


def main():
    ents = extract_all()
    ents.sort(key=lambda e: -e["n"])
    print(f"# {len(ents)} (channel,DID) response clusters recovered")
    for e in ents:
        s = e["series"]
        vals = Counter(v.hex() for _, v in s)
        t0 = s[0][0]
        did = f"DID {e['did']:04X}" if e["did"] is not None else "DID ????"
        hdr = "%02x%02x%02x%02x" % e["chan"]
        print(f"\n## {did}  chan {hdr} echo {e['echo'][0]:02x}{e['echo'][1]:02x}"
              f"  n={e['n']} distinct={len(vals)} span={s[-1][0]-t0:.0f}s")
        for hv, n in vals.most_common(4):
            print(f"    {hv} x{n}")
        print("    head:", " ".join(f"{v.hex()}" for _, v in s[:16]))


if __name__ == "__main__":
    main()
