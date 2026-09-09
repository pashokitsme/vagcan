#!/usr/bin/env python3
"""validate_k.py -- confirm a recovered AES-256 link key K against a dump.

Given a candidate 32-byte session key K, derive the 16 per-channel keystreams
KS_row = AES256-ECB(K).encrypt(IV_TABLE[row]) and try to decode any b8/b7 link
blocks found in the dump to valid UDS structure. A hit (PCI+SID, or 62 F1 90
VIN, or part-number ASCII) confirms K for this session.

Two block sources:
  1. framed blocks: search for  53 xx b8 <16>  (OUT) / 4d xx b7 <16> (IN).
  2. --raw : brute every 16-byte window (slow; use only on a small slice).

Usage:
    validate_k.py <dump.dmp> <Khex>            # framed b8/b7 blocks
    validate_k.py <dump.dmp> <Khex> --show     # print all decodes
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from link_cipher import IV_TABLE, CHANNEL_ID
from Crypto.Cipher import AES


def keystreams(K):
    c = AES.new(K, AES.MODE_ECB)
    return [c.encrypt(row) for row in IV_TABLE]


def looks_uds(p):
    """Heuristic: does a 16-byte decoded block look like a UDS single frame?"""
    pci = p[6]
    sid = p[7]
    # single frame PCI 0x01..0x08, SID a plausible UDS request/response
    if pci in (0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07):
        if sid in (0x10, 0x11, 0x14, 0x19, 0x22, 0x27, 0x28, 0x2E, 0x2F, 0x31,
                   0x3E, 0x50, 0x51, 0x54, 0x59, 0x62, 0x67, 0x6E, 0x6F, 0x71, 0x7F):
            return True
    if pci in (0x10, 0x21, 0x22, 0x23):  # multi-frame
        return True
    return False


def main():
    path, Khex = sys.argv[1], sys.argv[2].replace(" ", "")
    show = "--show" in sys.argv
    K = bytes.fromhex(Khex)
    assert len(K) == 32
    ks = keystreams(K)
    data = open(path, "rb").read()

    blocks = []
    for marker, opc, tag in ((0x53, 0xb8, "OUT/b8"), (0x4d, 0xb7, "IN/b7")):
        s = 0
        while True:
            i = data.find(bytes([opc]), s)
            if i < 0:
                break
            s = i + 1
            # require preceding framing byte marker within 2 bytes
            if i >= 2 and data[i - 2] == marker:
                blk = data[i + 1:i + 17]
                if len(blk) == 16:
                    blocks.append((i, tag, blk))
    print(f"# {path}: {len(blocks)} candidate framed b8/b7 blocks", file=sys.stderr)

    hits = 0
    for off, tag, blk in blocks:
        for row in range(16):
            p = bytes(a ^ b for a, b in zip(blk, ks[row]))
            if looks_uds(p):
                hits += 1
                if show or hits <= 40:
                    print(f"  off={off:#x} {tag} row={row:2d}  dec={p.hex(' ')}  "
                          f"PCI={p[6]:02x} SID={p[7]:02x}")
    print(f"# {hits} UDS-looking decodes", file=sys.stderr)
    if hits == 0:
        print("  (no framed blocks decoded to UDS -- K may be wrong, or link "
              "buffers not framed in memory)")


if __name__ == "__main__":
    main()
