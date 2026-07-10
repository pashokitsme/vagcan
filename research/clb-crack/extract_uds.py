#!/usr/bin/env python3
"""Extract + classify the UDS traffic in reading-ecus.pcapng, to test whether it
carries a (DID -> raw -> engineering value) measurement crib.

Result (see research/rod-labels.md 4): it does NOT. The capture is an engine-OFF
identification scan — VIN + ECU SW-version + security-access reads (which match
the Auto-Scan identity ground truth, confirming the decoder), plus 43
TesterPresent keep-alive channels. Only one engine measurement DID (0x7458) is
fully decodable and it returns a static value (engine off). There is no varying
measurement data to pin scaling, and no ordered measurement-read sequence to
align to the engine MWB list. A fresh LIVE capture is required.

Usage: .venv/bin/python extract_uds.py [capture.pcapng]
"""
import sys, os
from collections import defaultdict, Counter
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames

TP = bytes([0x02, 0x3E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00])  # off6..13 plaintext
OKPCI = {0x02, 0x03, 0x04, 0x05, 0x06, 0x07}
OKSID = {0x3E, 0x22, 0x19, 0x2E}

def load(path):
    b8, b7 = defaultdict(list), defaultdict(list)
    for f in reassemble_frames(path):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        b = bytes(p[1:17]); c = (b[0], b[2], b[3], b[5])
        if p[0] == 0xB8 and f["dir"] == "OUT":
            b8[c].append(b)
        elif p[0] == 0xB7 and f["dir"] == "IN":
            b7[c].append(b)
    return b8, b7

def tp_keystream(reqs):
    """If the channel carries TesterPresent, return full ks[6..13], else None."""
    uniq = [b for b, _ in Counter(reqs).most_common()]
    for cand in uniq:
        ks = {6 + i: cand[6 + i] ^ TP[i] for i in range(8)}
        if all((b[6] ^ ks[6]) in OKPCI and (b[7] ^ ks[7]) in OKSID for b in uniq):
            return ks
    return None

def main(path):
    b8, b7 = load(path)
    print(f"# {path}")
    print(f"# {sum(len(v) for v in b8.values())} request / {sum(len(v) for v in b7.values())} "
          f"response blocks; {len(b8)} request channels, {len(b7)} response channels")
    tp_only = rdbi = nontp = 0
    meas_dids = set()
    for c, reqs in b8.items():
        ks = tp_keystream(reqs)
        if ks is None:
            nontp += 1
            continue
        sids = {b[7] ^ ks[7] for b in set(reqs)}
        if sids == {0x3E}:
            tp_only += 1
        else:
            rdbi += 1
            for b in set(reqs):
                if (b[7] ^ ks[7]) == 0x22:
                    meas_dids.add((b[8] ^ ks[8]) << 8 | (b[9] ^ ks[9]))
    print(f"# TesterPresent keep-alive channels : {tp_only}")
    print(f"# TP+RDBI (fully decodable) channels : {rdbi}  -> engine measurement DIDs "
          f"{sorted('%04X' % d for d in meas_dids)}")
    print(f"# non-TP channels (no crib; identity/security reads via response) : {nontp}")

    # Identity ground truth visible via two-time-pad (resp ^ modal-request):
    print("\n# identity reads (two-time-pad response, VIN + SW-version cross-checked to Auto-Scan):")
    for c, tag in [((0xEB, 0x60, 0x39, 0xC9), "VIN (expect XW8AD4NE9JH008917)"),
                   ((0xB3, 0xEB, 0x0D, 0x55), "gearbox SW-version (expect 1003)"),
                   ((0xEB, 0x40, 0x39, 0xC9), "SW-version (expect 1003)")]:
        rs = b7.get(c, [])
        rq = b8.get(c, [])
        if not rs or not rq:
            continue
        mr = Counter(rq).most_common(1)[0][0]
        cs = "%02x%02x%02x%02x" % c
        frames = sorted({bytes(r[i] ^ mr[i] for i in range(16)) for r in rs}, key=lambda x: x[14])
        blob = b"".join(f[6:14] for f in frames)
        ascii_s = "".join(chr(x) if 32 <= x < 127 else "." for x in blob)
        print(f"  chan={cs}  {tag}")
        print(f"     ttp-ascii: {ascii_s}")

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "../captures/reading-ecus.pcapng")
