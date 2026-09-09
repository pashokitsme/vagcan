#!/usr/bin/env python3
"""Decode the SW-version multiframe response to learn the ISO-TP block layout."""
import sys, os
from collections import Counter
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames
from link_cipher import recover_channel_ks, decrypt_link


def load(path):
    b7 = []
    for f in reassemble_frames(path):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        if p[0] == 0xB7 and f["dir"] == "IN":
            b7.append((f["first_idx"], bytes(p[1:17])))
        if p[0] == 0xB8 and f["dir"] == "OUT":
            pass
    return b7


def load_reqs(path):
    b8 = []
    for f in reassemble_frames(path):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        if p[0] == 0xB8 and f["dir"] == "OUT":
            b8.append(bytes(p[1:17]))
    return b8


def main(path):
    b7 = load(path)
    b8 = load_reqs(path)
    # SW-version channel b3..eb0d..55
    def is_sw(b):
        return b[0] == 0xB3 and b[2] == 0xEB and b[3] == 0x0D and b[5] == 0x55
    sw_req = [b for b in b8 if is_sw(b)]
    modal = Counter(sw_req).most_common(1)[0][0]
    ks = recover_channel_ks(modal, pci=0x03, sid=0x22)
    sw_rsp = [(i, b) for i, b in b7 if is_sw(b)]
    print(f"# SW-version channel: {len(sw_req)} req, {len(sw_rsp)} rsp; ks={ks}")
    print("# decoded response blocks (off6..15), in order:")
    for i, b in sw_rsp:
        d = decrypt_link(b, ks)
        row = ' '.join(f'{x:02x}' if x is not None else '..' for x in d[6:16])
        print(f"  {i:7d} off6..15 = {row}")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "../captures/reading-ecus.pcapng")
