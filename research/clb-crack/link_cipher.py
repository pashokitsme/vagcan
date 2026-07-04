#!/usr/bin/env python3
"""link_cipher.py -- recovered b8/b7 diagnostic link cipher (vag-hex interop).

STATUS: BROKEN (algorithm) + one channel fully recovered, rest via known-plaintext.

The b8 (OUT, request) / b7 (IN, response) transport frames each carry a 16-byte
block right after the opcode:  53|len|b8|<16-byte block>|xor  (host->cable) and
4d|len|b7|<16-byte block>|xor (cable->host).

CIPHER (proven from reading-ecus.pcapng, cross-checked against the binary):
  * It is a POSITION-DEPENDENT XOR KEYSTREAM, fixed per logical channel:
        plain[i] = cipher[i] ^ KS_channel[i]          (i = 0..15)
    Proven pure-XOR because cipher_a ^ cipher_b == plain_a ^ plain_b holds
    exactly (e.g. TesterPresent 0x3E vs ReadDataByIdentifier 0x22 differ by the
    UDS SID bit 0x1C at block offset 7, and every request/response pair differs
    by exactly 0x40 at offset 7 = the UDS positive-response bit).
  * The keystream is NOT global: each ECU conversation ("channel") uses its own
    16-byte keystream, selected per-message. The binary picks a key with
    selector = (seq+1)&0xF from a 16-entry table (fn 0x140073150/0x140073160),
    i.e. up to 16 rotating keystreams. The raw table at 0x140171d30 does NOT map
    to the effective keystream by any simple transform (xor/add/|0x80/rotation),
    so the effective per-channel keystream is recovered here empirically from
    UDS known-plaintext rather than lifted from the table.

INNER BLOCK LAYOUT (16 bytes, HIGH confidence for offsets 6-13):
  off 0..5 : addressing/header. off1 = echoed UDS SID, off4 = direction bit
             (request vs response differ only at off1 and off4), off0/2/3/5 const
             per channel. (absolute plaintext of the constant bytes not needed.)
  off 6    : ISO-TP PCI  (0x0N single-frame, 0x1N first-frame, 0x2N consecutive)
  off 7    : UDS SID  (request), SID|0x40 (positive response), 0x7F (negative)
  off 8..13: UDS data bytes, then ISO-TP padding
             (request pad = 0x00, response pad = 0x55/0xFF)
  off 14   : per-frame transport counter (increments each frame)
  off 15   : trailer / checksum-like

Fully recovered keystream for the TesterPresent/measuring channel (header
f3 ?? 44 dd 7c/6c 5f). Offsets 6..13 are the UDS-bearing region:
"""
import sys
from collections import Counter, defaultdict

# ---- Recovered keystream for the primary "f3" channel (UDS region 6..13) ----
# Derived from TesterPresent plaintext "02 3E 00" + 0x00 ISO-TP padding.
KS_F3 = {
    1: 0xBD,   # header off1 = echoed SID
    6: 0x02, 7: 0xA9, 8: 0x99, 9: 0xF6,
    10: 0xDA, 11: 0x7C, 12: 0x9C, 13: 0x3A,
}


def decrypt_link(block16, keystream):
    """XOR a 16-byte b8/b7 block with a channel keystream.

    keystream: dict {offset: ks_byte} or a 16-byte sequence. Offsets without a
    known keystream byte are returned as None (unknown)."""
    if isinstance(keystream, dict):
        return [ (block16[i] ^ keystream[i]) if i in keystream else None
                 for i in range(16) ]
    return bytes(block16[i] ^ keystream[i] for i in range(16))


def encrypt_link(plain16, keystream):
    """Inverse (same XOR)."""
    return decrypt_link(plain16, keystream)


def recover_channel_ks(modal_request, pci=0x03, sid=0x22):
    """Recover a channel keystream from a modal single-frame request.

    Assumes the request is `PCI SID <data...>` and everything past the PDU is
    0x00 ISO-TP padding. Returns {offset: ks} for offsets 6,7 and the padding
    tail (data-position keystream needs the request's data plaintext, which for
    a fixed DID poll is unknown but ECHOED by the response)."""
    ks = {6: modal_request[6] ^ pci, 7: modal_request[7] ^ sid}
    # request bytes after the PDU are 0x00 padding -> ks == ciphertext there
    for i in range(6 + 1 + (pci & 0x0F), 14):
        ks[i] = modal_request[i]
    return ks


def two_time_pad(cipher_resp, cipher_req):
    """resp_plain[i] ^ req_plain[i]. Where the request byte is 0x00 padding, the
    result IS the response plaintext byte (keystream cancels). This reads
    response DATA with no keystream at all."""
    return bytes(a ^ b for a, b in zip(cipher_resp, cipher_req))


# --------------------------------------------------------------------------
def _load(path):
    sys.path.insert(0, __import__("os").path.dirname(__import__("os").path.abspath(__file__)))
    from usbpcap import reassemble_frames
    b8 = []; b7 = []
    for f in reassemble_frames(path):
        p = f["payload"]
        if not p:
            continue
        if p[0] == 0xb8 and f["dir"] == "OUT":
            b8.append((f["first_idx"], bytes(p[1:17])))
        elif p[0] == 0xb7 and f["dir"] == "IN":
            b7.append((f["first_idx"], bytes(p[1:17])))
    return b8, b7


def _hx(seq):
    return " ".join(f"{x:02x}" if x is not None else ".." for x in seq)


def _main(path):
    b8, b7 = _load(path)
    print(f"# {path}: {len(b8)} b8 (request) blocks, {len(b7)} b7 (response) blocks\n")

    def is_f3(b):
        return b[0] == 0xf3 and b[2] == 0x44 and b[3] == 0xdd and b[4] in (0x7c, 0x6c) and b[5] == 0x5f

    print("== f3 channel, FULLY-recovered keystream (UDS region off6..13) ==")
    seen = Counter()
    for idx, b in b8:
        if is_f3(b):
            d = decrypt_link(b, KS_F3)
            seen[tuple(d[6:14])] += 1
    for pdu, n in seen.most_common():
        pci = pdu[0]
        uds = bytes(pdu[1:1 + pci]) if pci and pci <= 8 else b""
        name = {b"\x3e\x00": "UDS TesterPresent",
                }.get(uds, "UDS ReadDataByIdentifier" if uds[:1] == b"\x22" else "?")
        print(f"  b8 REQ  n={n:4d}  PCI={pci:02x}  UDS={uds.hex(' ')}   <- {name}")
    seen = Counter()
    for idx, b in b7:
        if is_f3(b):
            d = decrypt_link(b, KS_F3)
            seen[tuple(x for x in d[6:14])] += 1
    for pdu, n in seen.most_common():
        pci = pdu[0]
        uds = bytes(pdu[1:1 + pci]) if pci and pci <= 8 else b""
        name = "UDS TesterPresent positive response" if uds[:2] == b"\x7e\x00" else "?"
        print(f"  b7 RESP n={n:4d}  PCI={pci:02x}  UDS={uds.hex(' ')}   <- {name}")

    # ---- cluster all frames into channels (const addressing off0,2,3,5) ----
    ch = defaultdict(lambda: {"req": [], "rsp": []})
    for idx, b in b8:
        ch[(b[0], b[2], b[3], b[5])]["req"].append(b)
    for idx, b in b7:
        ch[(b[0], b[2], b[3], b[5])]["rsp"].append(b)

    print("\n== vehicle-speed measuring poll (channel 00..788d..db), two-time-pad ==")
    c = ch[(0x00, 0x78, 0x8d, 0xdb)]
    rb = Counter(c["req"]).most_common(1)[0][0]
    vals = Counter(two_time_pad(s, rb)[8:14] for s in c["rsp"])
    for v, n in vals.most_common():
        print(f"  RDBI response data (off8..13) = {v.hex(' ')}   n={n}"
              f"   (DID echo off8-9 = {v[:2].hex(' ')})")
    print("  -> single constant value across the whole poll = static measurement"
          " (engine off, speed unchanging)")

    print("\n== gearbox SW-version channel (b3..eb0d..55), recovered keystream ==")
    c = ch[(0xb3, 0xeb, 0x0d, 0x55)]
    rb = Counter(c["req"]).most_common(1)[0][0]
    ks = recover_channel_ks(rb, pci=0x03, sid=0x22)  # RDBI poll base
    print(f"  modal request = {rb.hex(' ')}")
    print(f"  recovered ks (off6..13) = {_hx([ks.get(i) for i in range(6,14)])}")
    for s in Counter(c["rsp"]):
        d = decrypt_link(s, ks)
        pci = d[6]
        # ISO-TP: 0x0N single, 0x1N first, 0x2N consecutive
        tail = bytes(x for x in d[10:14] if x is not None)
        if b"\x10\x03" in tail:
            print(f"  b7 RESP PCI={pci:02x} data(off10..13)={tail.hex(' ')}"
                  f"  <- contains SW-version 10 03 = '1003'")
            print(f"     full block dec = {_hx(d)}")
            break


if __name__ == "__main__":
    p = sys.argv[1] if len(sys.argv) > 1 else "../reading-ecus.pcapng"
    _main(p)
