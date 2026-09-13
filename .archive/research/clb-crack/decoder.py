#!/usr/bin/env python3
"""
decoder.py -- Ross-Tech VCDS .clb / .rod container tooling.

Status (see FINDINGS.md):
  * CONTAINER FORMAT for both .clb and .rod: fully reverse-engineered. Working here.
  * LEGACY .clb cipher (pre-MQB): position-dependent byte keystream cipher, fully
    documented and implemented here (this is the SVCdec algorithm; keystream = the
    Ross-Tech "How to Copy and Paste" help text). Validated by round-trip below.
  * MODERN / MQB .clb + .rod (our target samples): a real 8-byte (64-bit) BLOCK
    cipher, fixed key, CBC-with-fixed-IV per record. The legacy keystream does NOT
    decode them (verified at every offset / transform). The block key+algorithm live
    in the PACKED VCDS.exe and could not be extracted statically. `block_decrypt`
    below is the drop-in hook once the algorithm+key are recovered.

This file is pure research tooling. It does NOT touch anything under crates/.
"""
import os
import re
import sys
import struct

HERE = os.path.dirname(os.path.abspath(__file__))
SVCDEC_PHP = os.path.join(HERE, "ref", "SVCdec.php")


# --------------------------------------------------------------------------
# Legacy keystream (the SVCdec "$key" int array -> keycode bytes).
# keycode[f] = int( key[f] / ((f&3)+2) / ((f&5)+1) ) % 256
# The resulting 1597 bytes spell out the Ross-Tech "How to Copy and Paste" text.
# --------------------------------------------------------------------------
def load_legacy_keystream(path=SVCDEC_PHP):
    src = open(path, "r", errors="replace").read()
    m = re.search(r"\$key=array\((.*?)\);", src, re.S)
    if not m:
        raise RuntimeError("could not find $key array in %s" % path)
    nums = [int(x) for x in re.findall(r"-?\d+", m.group(1))]
    return bytes(int(nums[f] / ((f & 3) + 2) / ((f & 5) + 1)) % 256
                 for f in range(len(nums)))


# --------------------------------------------------------------------------
# Legacy per-byte cipher (PHP-exact).
#   keystream byte:  cch = keycode[pos] | 0x80
#   encode:          C = ((cch ^ P) + cch) & 0xFF
#   decode:          P = cch ^ ((C - cch) & 0xFF)
#   pos = keypos(lineNum) + f, wrapping to a rolling counter once pos > 255.
#   keypos(lineNum) = ((lineNum % 16) * p + z) % 256
#     method 1 (OLD): p=3 z=250   method 2 (NEW): p=2 z=250   method 3 (V3): p=3 z=233
# --------------------------------------------------------------------------
KEYPOS_METHODS = {1: (3, 250), 2: (2, 250), 3: (3, 233)}


def legacy_keypos(line_num, method=1, p=None, z=None):
    if p is None or z is None:
        p, z = KEYPOS_METHODS[method]
    return ((line_num % 16) * p + z) % 256


def legacy_decode_line(keycode, cipher, keypos):
    out = bytearray()
    d = 0
    for f, ch in enumerate(cipher):
        pos = keypos + f
        if pos > 255:
            if d > 255:
                d = 0
            pos = d
            d += 1
        cch = keycode[pos] | 0x80
        out.append((cch ^ ((ch - cch) & 0xFF)) & 0xFF)
    return bytes(out)


def legacy_encode_line(keycode, plain, keypos):
    out = bytearray()
    d = 0
    for f, ch in enumerate(plain):
        pos = keypos + f
        if pos > 255:
            if d > 255:
                d = 0
            pos = d
            d += 1
        cch = keycode[pos] | 0x80
        out.append(((cch ^ ch) + cch) & 0xFF)
    return bytes(out)


# --------------------------------------------------------------------------
# Container parsers (format-level; works for legacy AND modern files).
# --------------------------------------------------------------------------
def parse_clb(data):
    """Return list of records. Each record = dict(len, cipher, blocks).

    Layout:  [00 <len>] [ciphertext, padded up to a multiple of 8] [00 0a]
    `len` is the *plaintext* length; ciphertext is padded to 8-byte blocks.
    Blank lines appear as a bare `00 0a`.
    """
    recs = []
    pos = 0
    n = len(data)
    while pos + 2 <= n:
        if data[pos:pos + 2] == b"\x00\x0a":         # blank line
            recs.append({"len": 0, "cipher": b"", "blank": True})
            pos += 2
            continue
        L = (data[pos] << 8) | data[pos + 1]
        if L == 0:
            break
        clen = ((L + 7) // 8) * 8
        cipher = data[pos + 2:pos + 2 + clen]
        recs.append({"len": L, "cipher": cipher,
                     "blocks": [cipher[i:i + 8] for i in range(0, len(cipher), 8)]})
        pos = pos + 2 + clen
        if data[pos:pos + 2] == b"\x00\x0a":
            pos += 2
    return recs


SECTION_RE = re.compile(rb"\[([A-Z]{2,4})\]\r\n(.*?)\r\n\[/\1\]", re.S)


def parse_rod(data):
    """Return list of (tag, payload) sections.

    .rod files are plaintext-framed:  [CMP]\r\n <payload> \r\n[/CMP]\r\n  etc.
    Section tags seen: CMP ADP DTC IDN INC GES MWB SLV SOT XPL.
    Payload framing varies:
      * MWB / measurement payloads use the same  00 <len> <cipher> 00 0a  records
        as .clb (parse with parse_clb).
      * CMP/ADP/... payloads start with a 5-byte header  80 00 <len> 00 00  where
        <len> is the ciphertext byte count (a multiple of 8), then the 8-byte-block
        ciphertext.
    """
    out = []
    for m in SECTION_RE.finditer(data):
        out.append((m.group(1).decode("latin1"), m.group(2)))
    return out


def rod_cmp_cipher(payload):
    """For a CMP/ADP/DTC/IDN payload starting with 80 00 <len> 00 00, return the
    (len, ciphertext) with ciphertext as 8-byte blocks."""
    if len(payload) >= 5 and payload[0] == 0x80 and payload[1] == 0x00 \
            and payload[3] == 0x00 and payload[4] == 0x00:
        L = payload[2]
        cipher = payload[5:5 + L]
        return L, cipher
    return None, payload


# --------------------------------------------------------------------------
# Modern block-cipher hook (NOT yet recovered -- see FINDINGS.md).
# --------------------------------------------------------------------------
def block_decrypt(cipher, key, iv=b"\x00" * 8):
    """Modern .clb/.rod cipher: TEA (32 rounds, delta 0x9E3779B9, LE words), CBC.

    RECOVERED from bin/VCDS-arm64-unpacked.exe. See decrypt_modern.py for the
    full implementation, the two hardcoded keys (KEY_CLB / KEY_ROD) and the
    per-record IV derivation. `key` is a list of four 32-bit ints
    [k0,k1,k2,k3] used as: v1 -= f(v0,k2,k3); v0 -= f(v1,k0,k1).
    """
    from decrypt_modern import tea_cbc_decrypt
    return tea_cbc_decrypt(cipher, key, iv)


# Convenience re-exports (the real crack lives in decrypt_modern.py).
try:
    from decrypt_modern import (KEY_CLB, KEY_ROD, decode_clb as decode_modern_clb,
                                decode_rod as decode_modern_rod)
except Exception:  # pragma: no cover
    KEY_CLB = [0xfa7e14d0, 0x249b910e, 0x2fdd6ffc, 0x15834a78]
    KEY_ROD = [0x029b76a4, 0xcb6db50a, 0x71395d29, 0x0dbc09c2]


# --------------------------------------------------------------------------
# High-level: legacy decode
# --------------------------------------------------------------------------
def decode_legacy_clb(path, method=1, p=None, z=None):
    keycode = load_legacy_keystream()
    data = open(path, "rb").read()
    lines = data.split(b"\x00\x0a")
    out = []
    for i, cipher in enumerate(lines):
        if len(cipher) < 2:
            out.append("")
            continue
        kp = legacy_keypos(i, method, p, z)
        out.append(legacy_decode_line(keycode, cipher, kp).decode("latin1"))
    return "\n".join(out)


def _selftest():
    keycode = load_legacy_keystream()
    assert keycode[:11] == b"How to Copy", keycode[:11]
    # round-trip proves the legacy encode/decode pair is exact.
    plain = b"08,10,Engine Speed,,Range: 0..8000 /min"
    kp = legacy_keypos(3, method=1)
    ct = legacy_encode_line(keycode, plain, kp)
    back = legacy_decode_line(keycode, ct, kp)
    assert back == plain, back
    print("[selftest] legacy keystream len =", len(keycode))
    print("[selftest] legacy round-trip OK  :", back.decode())
    print("[selftest] ciphertext            :", ct.hex(" "))


if __name__ == "__main__":
    if len(sys.argv) == 1:
        _selftest()
        print("\nusage: decoder.py <file.clb|file.rod> [--legacy method]")
        sys.exit(0)
    path = sys.argv[1]
    data = open(path, "rb").read()
    if path.lower().endswith(".rod"):
        secs = parse_rod(data)
        print("rod sections:", [(t, len(pl)) for t, pl in secs])
        for t, pl in secs:
            L, c = rod_cmp_cipher(pl)
            if L is not None:
                print("  %-4s cipher %d bytes (%d blocks)" % (t, len(c), len(c) // 8))
    else:
        recs = parse_clb(data)
        print("clb records:", len(recs))
        for r in recs[:8]:
            if r.get("blank"):
                print("  <blank>")
            else:
                print("  len=%d cipher=%d bytes (%d blocks) %s"
                      % (r["len"], len(r["cipher"]), len(r["cipher"]) // 8,
                         r["cipher"][:8].hex(" ")))
