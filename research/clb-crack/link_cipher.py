#!/usr/bin/env python3
"""link_cipher.py -- recovered b8/b7 diagnostic link cipher (vag-hex interop).

STATUS: mechanism REVERSED (AES-256 keystream over a 16-entry per-channel IV
table); per-session keystreams reproduced EMPIRICALLY from UDS known-plaintext
(the AES session key itself is a runtime secret, see "Missing piece" below).

The b8 (OUT, request) / b7 (IN, response) transport frames each carry a 16-byte
block right after the opcode:  53|len|b8|<16-byte block>|xor  (host->cable) and
4d|len|b7|<16-byte block>|xor (cable->host).

WIRE CIPHER (proven from reading-ecus.pcapng):
  * It is a POSITION-DEPENDENT XOR KEYSTREAM, fixed per logical channel:
        plain[i] = cipher[i] ^ KS_channel[i]          (i = 0..15)
    Proven pure-XOR / byte-local (NOT a diffusing block cipher): the cipher-diff
    of two frames of one channel is SPARSE and equals the plaintext-diff exactly,
    e.g. TesterPresent req vs its response differ only at off1 (SID echo), off4
    (direction), off6 (PCI), off7 (0x40 positive-response bit) and the pad tail
    (00 vs 55/ff) -> `00 01 00 00 10 00 07 40 00 00 55 ff ff ff 00 00`. A block
    cipher (CBC/ECB) would avalanche the whole 16-byte block; this does not.
  * The keystream is per logical channel; request and response of one channel
    share the SAME keystream (XOR is symmetric -> keystream mode).

RECOVERED SCHEDULE (static analysis of VCDS-arm64-unpacked.exe) -- see
vag-hex-framing.md "Link cipher" for the full write-up and confidence:
  * Channel selector:  channel_id = (msg_type + 1) & 0xF   -> only 16 keystreams.
    (caller 0x14006d0f4 -> selector 0x140073150: `(w1+1)&0xf`; msg_type is the
    plaintext command byte, prepended as (msg_type|0xf0) before encryption.)
  * IV table:  0x140171d30 = 16 rows x 16 bytes (NOT 256; the dispatcher indexes
    `table + channel_id*16`). Each row is the per-channel IV. (embedded below.)
  * Cipher engine:  AES-256 (!). Engine descriptor 0x140171e30, name "aes"
    (@0x14017ad80), block=16, key=32, T-tables @0x1401742e0, key-schedule fn
    0x140077b50, AES-encrypt block 0x1400780a8, AES-decrypt block 0x140078620.
    dispatcher (enc) 0x140073160 / (dec) 0x1400730d0 -> key-setup 0x14007b108
    (memcpy row -> ctx+8 IV) -> driver 0x14007afd0/0x14007aeb0.
  * Effective schedule:  KS_channel = AES_encrypt(IV = table_row[channel_id])
    under the session key (keystream / CFB mode -- matches the byte-local XOR
    wire behaviour). This is why NO simple table->keystream transform (xor / add
    / |0x80 / rotation / row_i^row_j) ever matched: the transform is AES.

MISSING PIECE (why we still recover keystreams empirically, not from the table):
  * The 32-byte AES key is a RUNTIME session key, supplied to the cipher context
    via a polymorphic set-key call (0x140072ec0 -> parse 0x14007ce68) from session
    setup -- it is NOT a static literal at this locus. Its derivation is adjacent
    to the out-of-scope 0xb6 anti-clone AUTH and is deliberately NOT analysed
    (see research/SCOPE-BOUNDARY.md). Without that key we cannot synthesise
    KS = AES(row) offline; we recover each session's keystreams from UDS
    known-plaintext instead (below). Reproducing a NEW session's keystreams
    would require capturing that session's key exchange, which is out of scope.

INNER BLOCK LAYOUT (16 bytes, HIGH confidence for offsets 6-13):
  off 0..5 : addressing/header. off1 = echoed UDS SID, off4 = direction bit
             (request vs response differ only at off1 and off4), off0/2/3/5 const
             per channel.
  off 6    : ISO-TP PCI  (0x0N single-frame, 0x1N first-frame, 0x2N consecutive)
  off 7    : UDS SID  (request), SID|0x40 (positive response), 0x7F (negative)
  off 8..13: UDS data bytes, then ISO-TP padding
             (request pad = 0x00, response pad = 0x55/0xFF)
  off 14   : per-frame transport counter (increments each frame)
  off 15   : trailer / checksum-like
"""
import sys
from collections import Counter, defaultdict

# --- Static 16-entry IV table @0x140171d30 (the per-channel AES IV rows) --------
IV_TABLE = [
    bytes.fromhex("5651543b2445152103541a345482104c"),
    bytes.fromhex("7655ed1c86571234c032db2ff3542802"),
    bytes.fromhex("3426433439bd64528d11442f32074522"),
    bytes.fromhex("87e8239f0c7b87560cfe1f5cbcd9fa89"),
    bytes.fromhex("4d3986a3dee2ba2ad04c1cdf233445ee"),
    bytes.fromhex("6a3d431030a34a2176a4cb64a3238687"),
    bytes.fromhex("5bdc54abf4de7c4d5a831243653f5ca5"),
    bytes.fromhex("54e5fd6bcedc4d3422b5c065665c3b89"),
    bytes.fromhex("87e43ca3341236f4273d9736236a7632"),
    bytes.fromhex("bc6834f675b8766cca5fe8cabedb3b90"),
    bytes.fromhex("a3a3bcbd46c323472c8be65af3233495"),
    bytes.fromhex("54213437ce2365a10b6bc4f5c2b765ea"),
    bytes.fromhex("c36675c6f573dd4b0d76c7f3766c2328"),
    bytes.fromhex("b5a823bf325d441a3b34f34e027864c1"),
    bytes.fromhex("2367650c763f771a6487384803fc877a"),
    bytes.fromhex("cd4c873f2a2a334e7a43c365404aa423"),
]

CHANNEL_ID = lambda msg_type: (msg_type + 1) & 0xF   # selector 0x140073150


def keystream_from_row(row16, aes_session_key):
    """The RECOVERED schedule:  KS = AES-256(IV = table_row) under the session key.

    Not runnable offline: `aes_session_key` (32 bytes) is the runtime session
    secret set at cipher-context init from session setup (see module docstring /
    SCOPE-BOUNDARY.md). This function documents the schedule and is the drop-in
    hook once a session's key is available from a legitimate capture."""
    if aes_session_key is None:
        raise NotImplementedError(
            "AES session key unavailable offline (runtime secret, auth-adjacent, "
            "out of scope to derive). Use recover_channel_ks() on captured UDS "
            "known-plaintext to obtain per-session keystreams instead.")
    # AES-256-ECB single block of the IV == CFB/OFB keystream for a per-frame-reset IV.
    from Crypto.Cipher import AES  # optional; only if a real session key is provided
    return AES.new(bytes(aes_session_key), AES.MODE_ECB).encrypt(bytes(row16))


# ---- Recovered keystream for the primary "f3" channel (UDS region 6..13) -------
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
        return [(block16[i] ^ keystream[i]) if i in keystream else None
                for i in range(16)]
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


def recover_all_channels(b8, b7):
    """Generalised per-channel keystream recovery (replaces the 3 hardcoded cribs).

    Clusters frames by the constant addressing header (off0,2,3,5), then for each
    channel recovers a keystream from the modal request by trying the common
    single-frame UDS cribs (RDBI/TesterPresent) and validating it decodes ALL of
    that channel's requests consistently and a response as a positive UDS reply.
    Returns {header_key: (ks_dict, pci, sid, n_req, n_rsp)}."""
    reqs = defaultdict(list); resps = defaultdict(list)
    for _, b in b8:
        reqs[(b[0], b[2], b[3], b[5])].append(b)
    for _, b in b7:
        resps[(b[0], b[2], b[3], b[5])].append(b)
    out = {}
    for k, rq in reqs.items():
        rs = resps.get(k, [])
        modal = Counter(rq).most_common(1)[0][0]
        for pci, sid in [(0x03, 0x22), (0x02, 0x3e), (0x04, 0x22),
                         (0x05, 0x22), (0x03, 0x19), (0x03, 0x2e)]:
            ks = recover_channel_ks(modal, pci=pci, sid=sid)
            dec = [decrypt_link(b, ks) for b in rq]
            if not all(d[6] == pci and d[7] == sid for d in dec):
                continue
            if rs:
                dr = [decrypt_link(b, ks) for b in rs]
                if not any(dd[7] == (sid | 0x40)
                           or dd[6] in (0x07, 0x10, 0x20, 0x21, 0x22, 0x23)
                           for dd in dr):
                    continue
            out[k] = (ks, pci, sid, len(rq), len(rs))
            break
    return out


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
        name = {b"\x3e\x00": "UDS TesterPresent"}.get(
            uds, "UDS ReadDataByIdentifier" if uds[:1] == b"\x22" else "?")
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
        tail = bytes(x for x in d[10:14] if x is not None)
        if b"\x10\x03" in tail:
            print(f"  b7 RESP PCI={pci:02x} data(off10..13)={tail.hex(' ')}"
                  f"  <- contains SW-version 10 03 = '1003'")
            print(f"     full block dec = {_hx(d)}")
            break

    # ---- generalised recovery across ALL channels (schedule -> per-session KS) --
    print("\n== generalised per-channel keystream recovery (all channels) ==")
    allch = recover_all_channels(b8, b7)
    total = len({(b[0], b[2], b[3], b[5]) for _, b in b8})
    print(f"  reproduced + validated keystreams: {len(allch)} / {total} request channels")
    print("  (each recovered from that channel's UDS known-plaintext; the AES")
    print("   session key needed to derive KS=AES(row) offline is out of scope)")


if __name__ == "__main__":
    p = sys.argv[1] if len(sys.argv) > 1 else "../reading-ecus.pcapng"
    _main(p)
