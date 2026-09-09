#!/usr/bin/env python3
"""
decrypt_modern.py -- modern (MQB) VCDS .clb / .rod block-cipher decryptor.

CRACKED from bin/VCDS-arm64-unpacked.exe (PE32+ AArch64).

Algorithm  : TEA (Tiny Encryption Algorithm), 64-bit block, 32 rounds,
             delta = 0x9E3779B9, sum starts at 0xC6EF3720 for decrypt.
             Words are LITTLE-ENDIAN 32-bit.
Mode       : CBC. The engine prepends a per-record derived 8-byte IV to the
             ciphertext, TEA-CBC-decrypts, and drops the IV plaintext block.
Two keys   :
  KEY_CLB (".clb" label files) -- selected in the binary with keysel w4==2:
             fa7e14d0 249b910e 2fdd6ffc 15834a78
  KEY_ROD (".rod" UDS/ODX files) -- selected with keysel w4==0:
             029b76a4 cb6db50a 71395d29 0dbc09c2
  Key array order used by the round function is k = [w23,w22,w21,w24], i.e.
             v1 -= f(v0,k2,k3) ; v0 -= f(v1,k0,k1).

Binary evidence (VMAs, VCDS-arm64-unpacked.exe, ImageBase 0x140000000):
  * TEA decrypt core            : 0x1400e0658  (loop @0x1400e0720)
      sum const 0xC6EF3720      @ 0x1400e07ec
      step const 0x61C88647     @ 0x1400e07e8  (= -delta, sum += step)
      KEY_CLB literals          @ 0x1400e07c8..0x1400e07d4
      KEY_ROD literals          @ 0x1400e07d8..0x1400e07e4
  * LE 32-bit word load helper  : 0x1400e04a8
  * CBC XOR-with-prev helper    : 0x1400e0500  (memcpy prev<-cipher @0x1400e0784)
  * .clb record decode caller   : 0x1400651f4  (keysel=2 -> KEY_CLB)
  * .clb per-record IV derive   : 0x140064fd8  (formula below)
  * .rod decode caller          : (Key B path, keysel=0 -> KEY_ROD)

IV derivation for .clb (fn @0x140064fd8): from a per-line counter (w15, the
record index, stored/incremented at [obj+0x24]) and a context byte w7 that is
constant within a file. We recover w7 per file by a 256-way brute that
maximises first-block printability (unambiguous in practice; w7==233 for the
tested files -- note 233 is also the SVCdec "method 3" constant).

The .rod first-block IV uses a different (not fully reversed) derivation, so
for .rod the first 8 bytes of each record are approximate; every subsequent
block is exact (CBC makes blocks 2..n independent of the IV).
"""
import sys, os

M = 0xFFFFFFFF
DELTA = 0x9E3779B9
SUM0 = 0xC6EF3720

KEY_CLB = [0xfa7e14d0, 0x249b910e, 0x2fdd6ffc, 0x15834a78]
KEY_ROD = [0x029b76a4, 0xcb6db50a, 0x71395d29, 0x0dbc09c2]


def tea_decrypt_block(b8, k):
    """Decrypt one 8-byte TEA block (LE words). Returns 8 bytes (pre-CBC-XOR)."""
    v0 = int.from_bytes(b8[0:4], "little")
    v1 = int.from_bytes(b8[4:8], "little")
    s = SUM0
    for _ in range(32):
        v1 = (v1 - ((((v0 << 4) & M) + k[2] & M) ^ ((v0 + s) & M) ^ (((v0 >> 5) + k[3]) & M))) & M
        v0 = (v0 - ((((v1 << 4) & M) + k[0] & M) ^ ((v1 + s) & M) ^ (((v1 >> 5) + k[1]) & M))) & M
        s = (s - DELTA) & M
    return v0.to_bytes(4, "little") + v1.to_bytes(4, "little")


def tea_cbc_decrypt(cipher, k, iv):
    """CBC: P_i = TEA_dec(C_i) XOR C_{i-1}, C_0 == iv."""
    out = bytearray()
    prev = iv
    for i in range(0, len(cipher) - len(cipher) % 8, 8):
        c = cipher[i:i + 8]
        d = tea_decrypt_block(c, k)
        out += bytes(x ^ y for x, y in zip(d, prev))
        prev = c
    return bytes(out)


# --- .clb per-record IV (fn @0x140064fd8) -------------------------------------
def clb_iv(w7, w15):
    a = ((w7 + 2) * (w15 + 1) * (w15 + 3)) & M
    w24 = ((w7 + 1) * (w15 + 2) + a) & M
    r = w7 % (w15 + 1)
    w8 = r if r != 0 else (w15 % (w7 + 1))
    w23 = (((w7 + 3) * (w7 + 1) * (w15 + 2)) + w8) & M
    if w24 < 0xffff:
        w24 = (((w24 << 16) & M) + ((w15 + 4) * (w7 + 3) * (w7 + 1) * (w15 + 2))) & M
    if w23 < 0xffff:
        w23 = (((w23 << 16) & M) + ((w15 + 1) * (w15 + 2) * (w15 + 3))) & M
    return (w24 & M).to_bytes(4, "little") + (w23 & M).to_bytes(4, "little")


def _printable(b):
    return sum(1 for x in b if 32 <= x < 127 or x in (9, 10, 13))


def _brute_w7(records, k):
    """records: list of (cipher, plainlen). Recover the file-constant w7."""
    best = None
    for w7 in range(256):
        score = 0
        for idx, (cipher, plen) in enumerate(records):
            if len(cipher) < 8:
                continue
            d0 = tea_decrypt_block(cipher[:8], k)
            p0 = bytes(x ^ y for x, y in zip(d0, clb_iv(w7, idx)))
            score += _printable(p0[:min(8, plen)])
    # (kept simple: one pass; recompute best below)
        if best is None or score > best[0]:
            best = (score, w7)
    return best[1]


# --- container parsing (mirrors decoder.py) -----------------------------------
def parse_clb_records(data):
    recs = []
    pos, n = 0, len(data)
    while pos + 2 <= n:
        if data[pos:pos + 2] == b"\x00\x0a":
            recs.append((None, 0)); pos += 2; continue
        L = (data[pos] << 8) | data[pos + 1]
        if L == 0:
            break
        clen = ((L + 7) // 8) * 8
        cipher = data[pos + 2:pos + 2 + clen]
        recs.append((cipher, L))
        pos += 2 + clen
        if data[pos:pos + 2] == b"\x00\x0a":
            pos += 2
    return recs


def decode_clb(data):
    recs = parse_clb_records(data)
    real = [(c, L) for (c, L) in recs if c is not None]
    w7 = _brute_w7(real, KEY_CLB)
    out = []
    ri = 0
    for c, L in recs:
        if c is None:
            out.append(""); continue
        pt = tea_cbc_decrypt(c, KEY_CLB, clb_iv(w7, ri))[:L]
        out.append(pt.decode("latin1"))
        ri += 1
    return "\n".join(out), w7


import re
_SEC = re.compile(rb"\[([A-Z]{2,4})\]\r\n(.*?)\r\n\[/\1\]", re.S)

# --- .rod first-block IV (raw-file decryptor fn @0x140033900) ------------------
# IV = per-byte transform of an 8-byte seed, then TEA-CBC (Key B) with that IV.
#   key  = the section tag ("CMP","INC","GES","SLV",...); m = key[1].
#   seed = key[0:3] || product_bytes[0:5]
#          where product_bytes = low 5 bytes of the 64-bit product of the chars
#          of a per-record "record-string" (a runtime buffer; see NOTES).
#   additive:  s[i] = (seed[i] + KS[(m*(i+2)) & 0xff]) & 0xff       (D==0 mod 256)
#   multiply:  IV[i] = (s[i] * MT[OFF_ROD[i]]) & 0xff
# KS = static table @0x140171730 (rod_KS.bin); MT = runtime table (runtime_MT.bin).
OFF_ROD = [0x07, 0xca, 0x22, 0x99, 0x3e, 0x88, 0xc3, 0x76]


def _load(name):
    p = os.path.join(os.path.dirname(os.path.abspath(__file__)), name)
    return open(p, "rb").read() if os.path.exists(p) else None


_ROD_MT = _load("runtime_MT.bin")   # 256-byte MT (dumped from live process)
_ROD_KS = _load("rod_KS.bin")       # 256-byte KS (static, from the binary)


def rod_block0_iv(tag, product=0):
    """Compute the 8-byte IV for a .rod record's first block.

    `tag` is the section tag bytes (e.g. b"CMP"). `product` is the 64-bit char
    product of the record-string; default 0 (correct whenever that string is
    empty / contains a NUL -- fully decodes such records). IV[0:3] (from the
    tag) is always exact; IV[3:8] needs the true `product`.
    """
    if _ROD_MT is None or _ROD_KS is None:
        return None
    m = tag[1]
    seed = list(tag[:3]) + list((product & ((1 << 40) - 1)).to_bytes(5, "little"))
    s = [(seed[i] + _ROD_KS[(m * (i + 2)) & 0xff]) & 0xff for i in range(8)]
    return bytes((s[i] * _ROD_MT[OFF_ROD[i]]) & 0xff for i in range(8))


def rod_section_cipher(pl):
    """Return (kind, storedlen, plainlen, cipher) for a .rod section payload.

    Unified 6-byte header = two 24-bit big-endian ints (fn @0x140033d58):
        read1 = BE24(pl[0:3]);  flag = read1 & 0x800000;  storedlen = read1 & 0x7fffff
        read2 = BE24(pl[3:6]);  plainlen (== decompressed size for compressed
                                sections, or plaintext length for plain ones)
        cipher = pl[6 : 6 + storedlen]
    Both kinds are TEA-CBC / KEY_ROD with the section-tag IV. The flag decides
    post-processing (fn @0x140033758, dispatch on bit 0x800000):
      flag SET   (byte0 & 0x80): plaintext = TEA_dec(cipher)[:plainlen]
                 -> CMP/INC/GES/SLV (small, uncompressed)
      flag CLEAR (byte0 == 0x00): plaintext = zlib.decompress(TEA_dec(cipher))
                 -> MWB/ADP/DTC-large/XPL/SOT/IDN (zlib 1.2.11 deflate)
    """
    if len(pl) < 6:
        return ("plain", 0, 0, b"")
    read1 = (pl[0] << 16) | (pl[1] << 8) | pl[2]
    read2 = (pl[3] << 16) | (pl[4] << 8) | pl[5]
    storedlen = read1 & 0x7fffff
    cipher = pl[6:6 + storedlen]
    kind = "tea" if (read1 & 0x800000) else "zlib"
    return (kind, storedlen, read2, cipher)


def decode_rod(data):
    """Return list of (tag, kind, decoded_text_or_None).

    For 'tea' sections: blocks 2..n are EXACT (CBC makes them IV-independent);
    the first 8 bytes use a per-record IV that is NOT statically reproducible
    (see NOTES-modern.txt: needs runtime BSS tables + a cleartext seed field),
    so they are left as the raw TEA output (garbled) and flagged.
    For 'stream'/'plain' sections: returned as None (different cipher / not
    encrypted with TEA).
    """
    import zlib
    out = []
    for m in _SEC.finditer(data):
        tagb = m.group(1)
        tag = tagb.decode("latin1")
        kind, storedlen, plainlen, cipher = rod_section_cipher(m.group(2))
        if len(cipher) < 8 or len(cipher) % 8 != 0:
            out.append((tag, kind, None))
            continue
        iv = rod_block0_iv(tagb, product=0) or (b"\x00" * 8)
        dec = tea_cbc_decrypt(cipher, KEY_ROD, iv)
        if kind == "tea":
            out.append((tag, "tea", dec[:plainlen].decode("latin1")))
        else:  # zlib: TEA plaintext is a zlib deflate stream
            try:
                pt = zlib.decompress(dec)
                out.append((tag, "zlib", pt.decode("latin1")))
            except Exception:
                # block0 IV product != 0 corrupts the deflate stream start
                out.append((tag, "zlib-fail", None))
    return out


if __name__ == "__main__":
    for path in sys.argv[1:]:
        data = open(path, "rb").read()
        print("=" * 70)
        print(path)
        if path.lower().endswith(".rod"):
            for tag, kind, txt in decode_rod(data):
                if txt is None:
                    print("[%s] (%s - first-block IV product!=0, not decoded)" % (tag, kind))
                else:
                    print("[%s] (%s)\n%s" % (tag, kind, txt))
        else:
            text, w7 = decode_clb(data)
            print("(clb IV w7=%d)" % w7)
            print(text)
