#!/usr/bin/env python3
"""aes_scan_fast.py -- fast (numpy-free) AES-256 key-schedule scanner.

Same verifiable result as aes_ks_scan.py but the O(n) per-byte AES-256 prefilter
is vectorised with a single big-integer XOR over the whole buffer instead of a
Python-level per-offset loop, so a 400 MB dump scans in seconds, not minutes.

The AES-256 schedule obeys w[9] = w[1] ^ w[8] (byte offset 36 == off4 ^ off32),
a relation that is NEVER a SubWord/Rcon slot. We compute (buf[4:] ^ buf[32:])
in one shot, compare to buf[36:], and only run the full key expansion on the
(few) offsets where all 4 bytes of that word match.

Usage: aes_scan_fast.py <file.bin|dump.dmp> [--wordswap-only|--both]
Prints  off=<hex>  K=<hex>  [layout]  for every verified AES-256 master key.
"""
import sys, os
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from aes_ks_scan import expand, _wordswap


import re
_ZERORUN = re.compile(rb"\x00{16,}")


def _candidates_chunk(data, keylen):
    """Yield offsets where the AES word relation w[nk+1]==w[1]^w[nk] holds."""
    nk = keylen // 4
    a, b, c = 4, 4 * nk, 4 * (nk + 1)
    n = len(data)
    if n < c + 4:
        return
    L = n - c
    A = int.from_bytes(data[a:a + L], "little")
    B = int.from_bytes(data[b:b + L], "little")
    C = int.from_bytes(data[c:c + L], "little")
    xb = ((A ^ B) ^ C).to_bytes(L, "little")
    start = 0
    while True:
        i = xb.find(b"\x00\x00\x00\x00", start)
        if i < 0:
            break
        yield i
        start = i + 1


def _candidates(data, keylen=32):
    """Split off long zero runs (real AES schedules contain none) so the
    candidate finder never wades through zero pages, then scan each data chunk."""
    pos = 0
    for m in _ZERORUN.finditer(data):
        chunk = data[pos:m.start() + 40]  # +40 so a key ending near the run survives
        for off in _candidates_chunk(chunk, keylen):
            yield pos + off
        pos = m.end()
    chunk = data[pos:]
    for off in _candidates_chunk(chunk, keylen):
        yield pos + off


def scan(data, layouts, sizes=(32,)):
    sched_len = {16: 176, 24: 208, 32: 240}
    ent_guard = {16: 8, 24: 10, 32: 12}
    hits = {}
    for lay, tag in layouts:
        buf = _wordswap(data) if lay == "ws" else data
        for keylen in sizes:
            sl = sched_len[keylen]
            for off in _candidates(buf, keylen):
                if off + keylen > len(buf):
                    continue
                key = buf[off:off + keylen]
                # cheap entropy guard: real AES keys are high-entropy. Rejects
                # the huge zero/pattern regions that satisfy the word relation.
                if len(set(key)) < ent_guard[keylen]:
                    continue
                if buf[off:off + sl] == expand(key):
                    hits.setdefault(key.hex(), []).append((off, keylen, tag))
    return hits


def main():
    path = sys.argv[1]
    layouts = [("std", ""), ("ws", "wordswap")]
    if "--wordswap-only" in sys.argv:
        layouts = [("ws", "wordswap")]
    sizes = (16, 24, 32) if "--all" in sys.argv else (32,)
    data = open(path, "rb").read()
    print(f"# {path}: {len(data)/1e6:.0f} MB  sizes={sizes}", file=sys.stderr)
    hits = scan(data, layouts, sizes)
    for k, locs in hits.items():
        kl = locs[0][1]
        tags = ",".join(t for _, _, t in locs if t) or "std"
        print(f"K(AES-{kl*8}) = {k}   ({tags}, {len(locs)} hit(s), first off={locs[0][0]:#x})")
    print(f"# {len(hits)} distinct AES key(s)", file=sys.stderr)


if __name__ == "__main__":
    main()
