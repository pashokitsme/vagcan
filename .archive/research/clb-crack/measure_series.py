#!/usr/bin/env python3
"""Stage 1+2: extract per-DID RDBI response TIME SERIES (with real USB
timestamps) from capture-w-logs.pcapng, and dump diagnostics so we can see the
ISO-TP PCI / response length / full data region per measurement DID.

Decodes the b8/b7 link cipher per channel using the TesterPresent known-plaintext
crib (see research/vag-hex-framing.md "Link cipher"). Prints, per DID:
  - number of responses, PCI histogram, full decrypted data region
  - the (t, raw_data) series head so we can eyeball the curve.
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
    uniq = [b for b, _ in Counter(reqs).most_common()]
    for cand in uniq:
        ks = {6 + i: cand[6 + i] ^ TP[i] for i in range(8)}
        if all((b[6] ^ ks[6]) in OKPCI and (b[7] ^ ks[7]) in OKSID for b in uniq):
            return ks
    return None


def dec(b, ks):
    return bytes(b[i] ^ ks.get(i, 0) for i in range(16))


def load():
    b8, b7 = [], []
    for f in reassemble_frames(CAP):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        blk = bytes(p[1:17])
        rec = (f["t"], f["first_idx"], blk)
        if p[0] == 0xB8 and f["dir"] == "OUT":
            b8.append(rec)
        elif p[0] == 0xB7 and f["dir"] == "IN":
            b7.append(rec)
    return b8, b7


def extract():
    """Return {did: [(t, data_bytes)]} for single-frame RDBI positive responses,
    plus per-channel keystreams. Only single-frame (PCI 0x0N) handled here."""
    b8, b7 = load()
    chreq, chrsp = defaultdict(list), defaultdict(list)
    for t, idx, b in b8:
        chreq[ck(b)].append(b)
    for t, idx, b in b7:
        chrsp[ck(b)].append((t, b))
    series = defaultdict(list)
    pci_hist = defaultdict(Counter)
    kslen = {}
    for c, reqs in chreq.items():
        ks = tp_ks(reqs)
        if ks is None:
            continue
        for t, b in chrsp.get(c, []):
            d = dec(b, ks)
            if d[7] != 0x62:
                continue
            did = (d[8] << 8) | d[9]
            pci = d[6]
            pci_hist[did][pci] += 1
            if pci & 0xF0 == 0x00:                 # single frame
                n = pci & 0x0F                      # total UDS payload length
                data = d[10:7 + n]                  # after 62 DIDhi DIDlo
            else:
                data = d[10:14]
            series[did].append((t, bytes(data)))
    return series, pci_hist


def main():
    series, pci_hist = extract()
    print(f"# capture-w-logs.pcapng: {len(series)} single-frame RDBI DIDs")
    for did in sorted(series):
        s = sorted(series[did])
        vals = Counter(v.hex(' ') for _, v in s)
        t0 = s[0][0]
        span = s[-1][0] - s[0][0]
        pcis = ' '.join(f"{p:02x}:{n}" for p, n in pci_hist[did].most_common())
        print(f"\n## DID {did:04X}: {len(s)} resp, {len(vals)} distinct, "
              f"span={span:.1f}s, PCI[{pcis}]")
        for hexv, n in vals.most_common(6):
            print(f"    {hexv:20s} x{n}")
        # head of the time series (relative seconds)
        head = ' '.join(f"({t - t0:.1f},{v.hex()})" for t, v in s[:12])
        print(f"    head: {head}")


if __name__ == "__main__":
    main()
