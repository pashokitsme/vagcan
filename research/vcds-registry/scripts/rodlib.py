"""Minimal .rod container reader: framing, TEA-CBC, tag-derived block-0 IV (port of rod/mod.rs + tea.rs)."""
import struct, zlib, os
SRC = os.path.join(os.path.dirname(os.path.abspath(__file__)), "../../../crates/data/vag-data-labels/src/rod/")
MT = open(SRC + "rod_mt.bin", "rb").read()
KS = open(SRC + "rod_ks.bin", "rb").read()
KEY_ROD = (0x029b76a4, 0xcb6db50a, 0x71395d29, 0x0dbc09c2)
OFF_ROD = (0x07, 0xca, 0x22, 0x99, 0x3e, 0x88, 0xc3, 0x76)
DELTA = 0x9E3779B9; SUM0 = 0xC6EF3720; M = 0xFFFFFFFF

def tea_dec_block(b, k=KEY_ROD):
    v0, v1 = struct.unpack("<II", b); s = SUM0
    for _ in range(32):
        v1 = (v1 - ((((v0 << 4) & M) + k[2]) ^ ((v0 + s) & M) ^ ((v0 >> 5) + k[3]))) & M
        v0 = (v0 - ((((v1 << 4) & M) + k[0]) ^ ((v1 + s) & M) ^ ((v1 >> 5) + k[1]))) & M
        s = (s - DELTA) & M
    return struct.pack("<II", v0, v1)

def cbc_dec(cipher, iv):
    out = bytearray(); prev = iv
    for i in range(0, len(cipher) - len(cipher) % 8, 8):
        blk = cipher[i:i+8]; d = tea_dec_block(blk)
        out += bytes(a ^ b for a, b in zip(d, prev)); prev = blk
    return bytes(out)

def block0_iv(tag):
    m = tag[1]; seed = bytes(tag[:3]) + b"\0" * 5
    s = [(seed[i] + KS[(m * (i + 2)) & 0xff]) & 0xff for i in range(8)]
    return bytes((s[i] * MT[OFF_ROD[i]]) & 0xff for i in range(8))

def sections(data):
    """yield (tag, compressed, plainlen, cipher) for every well-framed section."""
    pos = 0; n = len(data)
    while True:
        i = data.find(b"[", pos)
        if i < 0: return
        j = i + 1
        while j < n and 65 <= data[j] <= 90 and j - i - 1 < 8: j += 1
        tl = j - i - 1
        if not (2 <= tl <= 8 and data[j:j+3] == b"]\r\n"):
            pos = i + 1; continue
        tag = data[i+1:j]; ps = j + 3
        marker = b"\r\n[/" + tag + b"]\r\n"
        e = data.find(marker, ps)
        if e < 0: return
        payload = data[ps:e]; pos = e + len(marker)
        if len(payload) < 6: yield tag.decode(), None, None, None; continue
        read1 = int.from_bytes(payload[0:3], "big"); stored = read1 & 0x7fffff
        comp = (read1 & 0x800000) == 0; plain = int.from_bytes(payload[3:6], "big")
        if stored % 8 or len(payload) < 6 + stored: yield tag.decode(), None, None, None; continue
        yield tag.decode(), comp, plain, payload[6:6+stored]

def classify(tag, comp, plain, cipher):
    """'tea' | 'classic' | 'shifted:<D0D1>' | 'bad'"""
    if cipher is None or len(cipher) < 8: return "bad", None
    if not comp: return "tea", None
    b0 = cbc_dec(cipher[:8], block0_iv(tag.encode()))
    if b0[:2] == b"\x78\xda": return "classic", None
    return "shifted", bytes([b0[0] ^ 0x78, b0[1] ^ 0xda]).hex()

def tea_text(tag, plain, cipher):
    """Plaintext of an uncompressed section (first 8 bytes may be garbled when product != 0)."""
    return cbc_dec(cipher, block0_iv(tag.encode()))[:plain]
