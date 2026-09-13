#!/usr/bin/env python3
"""aes_ks_scan.py -- scan a process memory dump for live AES key schedules.

Clean-room interop: recover the OLD-scheme per-epoch AES session key `K_epoch`
that the (VMProtect-packed) old x86 VCDS computes app-side and uses to XOR the
b8/b7 link cipher. VMProtect hides CODE, not the live round-key material in
memory: while VCDS talks to the car, each epoch's expanded AES key schedule sits
in cleartext in the cipher context (it's used every frame). This scans a full
process memory dump for any valid AES-128/192/256 key schedule and prints the
recovered master key(s) `K` -- verifiable, zero false positives (the expansion
is a deterministic recurrence, so a 240-byte window either IS a real schedule or
is not).

Usage:
    aes_ks_scan.py <dump.bin>            # scan for AES-256 (and 128/192)
    aes_ks_scan.py <dump.bin> --256      # AES-256 only (the link cipher)

Then cross-check a recovered K against the capture's known keystream:
    KS_cid == AES-256-ECB(K).encrypt(IV_TABLE[cid])   (see link_cipher.py)
"""
import sys

SBOX = bytes.fromhex(
    "637c777bf26b6fc53001672bfed7ab76ca82c97dfa5947f0add4a2af9ca472c0"
    "b7fd9326363ff7cc34a5e5f171d8311504c723c31896059a071280e2eb27b275"
    "09832c1a1b6e5aa0523bd6b329e32f8453d100ed20fcb15b6acbbe394a4c58cf"
    "d0efaafb434d338545f9027f503c9fa851a3408f929d38f5bcb6da2110fff3d2"
    "cd0c13ec5f974417c4a77e3d645d197360814fdc222a908846eeb814de5e0bdb"
    "e0323a0a4906245cc2d3ac629195e479e7c8376d8dd54ea96c56f4ea657aae08"
    "ba78252e1ca6b4c6e8dd741f4bbd8b8a703eb5664803f60e613557b986c11d9e"
    "e1f8981169d98e949b1e87e9ce5528df8ca1890dbfe6426841992d0fb054bb16"
)
RCON = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1B, 0x36, 0x6C, 0xD8, 0xAB, 0x4D]


def _sub(w):
    return bytes(SBOX[b] for b in w)


def _rot(w):
    return w[1:] + w[:1]


def expand(key):
    """Return the full AES key schedule bytes for a 16/24/32-byte key (standard
    FIPS-197 word order, big-endian words)."""
    nk = len(key) // 4
    nr = {4: 10, 6: 12, 8: 14}[nk]
    total = 4 * (nr + 1)
    w = [key[4 * i:4 * i + 4] for i in range(nk)]
    for i in range(nk, total):
        t = w[i - 1]
        if i % nk == 0:
            t = bytes(a ^ b for a, b in zip(_sub(_rot(t)), bytes([RCON[i // nk - 1], 0, 0, 0])))
        elif nk > 6 and i % nk == 4:
            t = _sub(t)
        w.append(bytes(a ^ b for a, b in zip(w[i - nk], t)))
    return b"".join(w)


def _scan_one_layout(data, sizes, tag):
    """Scan one byte layout. Cheap cascade pre-filter: for AES the word at index
    (nk+1) obeys w[nk+1] = w[1] ^ w[nk] (that index is never a SubWord/Rcon slot),
    i.e. bytes[off+4(nk+1)..] == bytes[off+4..]^bytes[off+4nk..]. Check ONE byte of
    that relation first (rejects 255/256 offsets), then the full 4 bytes, then the
    whole expansion. Turns a per-offset AES expansion into one byte-compare for
    almost every offset."""
    hits = []
    n = len(data)
    for keylen in sizes:
        nk = keylen // 4
        sched_len = {16: 176, 24: 208, 32: 240}[keylen]
        a = 4               # byte offset of w[1]
        b = 4 * nk          # byte offset of w[nk]
        c = 4 * (nk + 1)    # byte offset of w[nk+1]  (== w[1]^w[nk])
        last = n - sched_len
        off = 0
        while off <= last:
            # one-byte pre-check
            if data[off + c] != (data[off + a] ^ data[off + b]):
                off += 1
                continue
            # 4-byte pre-check
            if (data[off + c:off + c + 4]
                    != bytes(x ^ y for x, y in zip(data[off + a:off + a + 4],
                                                   data[off + b:off + b + 4]))):
                off += 1
                continue
            key = data[off:off + keylen]
            if key != key[:1] * keylen and data[off:off + sched_len] == expand(key):
                hits.append((off, keylen, key.hex() + tag))
            off += 1
    return hits


def scan(data, sizes, both_layouts=True):
    hits = _scan_one_layout(data, sizes, "")
    if both_layouts:
        # LibTomCrypt stores round keys as ulong32 (may be byte-swapped per word).
        hits += _scan_one_layout(_wordswap(data), sizes, "  (word-swapped)")
    return hits


def _wordswap(data):
    b = bytearray(data)
    for i in range(0, len(b) - 3, 4):
        b[i], b[i + 1], b[i + 2], b[i + 3] = b[i + 3], b[i + 2], b[i + 1], b[i]
    return bytes(b)


def _selftest():
    import os
    k = bytes(range(32))
    sched = expand(k)
    buf = os.urandom(1000) + sched + os.urandom(1000)
    hits = scan(buf, [32])
    assert any(h[2].startswith(k.hex()) for h in hits), "self-test FAILED"
    print("self-test OK: AES-256 schedule found, K recovered", file=sys.stderr)


if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] == "--selftest":
        _selftest()
        sys.exit(0)
    sizes = [32] if "--256" in sys.argv else [32, 24, 16]
    data = open(sys.argv[1], "rb").read()
    print(f"scanning {len(data)} bytes for AES key schedules {sizes} ...", file=sys.stderr)
    hits = scan(data, sizes)
    if not hits:
        print("no AES key schedules found")
    seen = set()
    for off, keylen, khex in hits:
        if khex in seen:
            continue
        seen.add(khex)
        print(f"  off={off:#x}  AES-{keylen*8}  K={khex}")
    print(f"\n{len(seen)} distinct key(s) found", file=sys.stderr)
