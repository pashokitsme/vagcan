"""Minimal standalone `.rod` reader — the Python twin of `crates/vag-data/src/rod.rs`.

Research tooling only. It exists because the workspace binary is not always
buildable while other agents edit it, and because the attacks in
`research/labels/codes-dat.md` §5 want to sweep 110 000 tables, which is a script's job.

Nothing here is authoritative: `rod.rs` is. If the two disagree, `rod.rs` wins.
"""

import struct
import zlib
from pathlib import Path

KEY_ROD = [0x029B76A4, 0xCB6DB50A, 0x71395D29, 0x0DBC09C2]
OFF_ROD = [0x07, 0xCA, 0x22, 0x99, 0x3E, 0x88, 0xC3, 0x76]

_SRC = Path(__file__).resolve().parents[2] / "crates" / "vag-data" / "src" / "rod"
MT = _SRC.joinpath("rod_mt.bin").read_bytes()
KS = _SRC.joinpath("rod_ks.bin").read_bytes()

DELTA = 0x9E3779B9
SUM0 = 0xC6EF3720
M32 = 0xFFFFFFFF


def tea_decrypt_block(block, key=KEY_ROD):
    v0, v1 = struct.unpack("<II", block)
    s = SUM0
    for _ in range(32):
        v1 = (v1 - ((((v0 << 4) & M32) + key[2]) ^ (v0 + s) ^ ((v0 >> 5) + key[3]))) & M32
        v0 = (v0 - ((((v1 << 4) & M32) + key[0]) ^ (v1 + s) ^ ((v1 >> 5) + key[1]))) & M32
        s = (s - DELTA) & M32
    return struct.pack("<II", v0, v1)


def tea_cbc_decrypt(cipher, iv, key=KEY_ROD):
    out = bytearray()
    prev = bytes(iv)
    for i in range(0, len(cipher) - len(cipher) % 8, 8):
        blk = cipher[i:i + 8]
        dec = tea_decrypt_block(blk, key)
        out += bytes(a ^ b for a, b in zip(dec, prev))
        prev = blk
    return bytes(out)


def rod_block0_iv(tag):
    m = tag[1]
    seed = bytes(tag[:3]) + b"\0" * 5
    s = [(seed[i] + KS[(m * (i + 2)) & 0xFF]) & 0xFF for i in range(8)]
    return bytes((s[i] * MT[OFF_ROD[i]]) & 0xFF for i in range(8))


def _be24(b):
    return (b[0] << 16) | (b[1] << 8) | b[2]


def _sections_raw(data):
    """Yield (tag_bytes, payload_bytes) for every well-formed section."""
    pos = 0
    n = len(data)
    while True:
        i = data.find(b"[", pos)
        if i < 0:
            return
        j = i + 1
        while j < n and 0x41 <= data[j] <= 0x5A and j - i - 1 < 8:
            j += 1
        tl = j - i - 1
        if not (2 <= tl <= 8 and j + 2 < n and data[j:j + 3] == b"]\r\n"):
            pos = i + 1
            continue
        tag = data[i + 1:j]
        marker = b"\r\n[/" + tag + b"]\r\n"
        end = data.find(marker, j + 3)
        if end < 0:
            pos = i + 1
            continue
        yield tag, data[j + 3:end]
        pos = end + len(marker)


def _cipher(payload):
    if len(payload) < 6:
        return None
    read1 = _be24(payload[0:3])
    storedlen = read1 & 0x7FFFFF
    compressed = (read1 & 0x800000) == 0
    plainlen = _be24(payload[3:6])
    if storedlen % 8 or len(payload) < 6 + storedlen:
        return None
    return compressed, plainlen, payload[6:6 + storedlen]


def _anchors():
    for hlit in range(30):
        yield 0b100 | (hlit << 3)
        yield 0b101 | (hlit << 3)


def decode_section(tag, payload, iv3to8=None):
    """Return decoded bytes or None. `iv3to8` is the recovered IV tail, if known."""
    parsed = _cipher(payload)
    if parsed is None:
        return None
    compressed, plainlen, cipher = parsed
    ivs = []
    base = rod_block0_iv(tag)
    if iv3to8 is None:
        ivs.append(base)
    else:
        ivs.append(bytes(base[:3]) + bytes(iv3to8))
        # the shifted-IV regime (research/labels/tttext2.md §3.3)
        if compressed and len(cipher) >= 8:
            t = tea_decrypt_block(cipher[:8])
            for d0 in _anchors():
                ivs.append(bytes([t[0] ^ 0x78, t[1] ^ 0xDA, t[2] ^ d0]) + bytes(iv3to8))
    for iv in ivs:
        dec = tea_cbc_decrypt(cipher, iv)
        if not compressed:
            if plainlen <= len(dec):
                return dec[:plainlen]
            continue
        try:
            out = zlib.decompress(dec)
        except zlib.error:
            continue
        if len(out) == plainlen:
            return out
    return None


def sections(path, ivs=None):
    """{tag: bytes|None} for a file. `ivs` maps tag -> 5-byte IV tail."""
    data = Path(path).read_bytes()
    ivs = ivs or {}
    out = {}
    for tag, payload in _sections_raw(data):
        t = tag.decode("latin-1")
        out[t] = decode_section(tag, payload, ivs.get(t))
    return out
