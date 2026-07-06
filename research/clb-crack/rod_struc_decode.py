#!/usr/bin/env python3
"""rod_struc_decode.py -- crack the .rod first-block IV[3:8] (the runtime
`product` term) for a zlib section by brute-forcing ONLY the 5 corrupted
plaintext bytes, using a DFS deflate dynamic-Huffman-header validator with
incremental Kraft-inequality pruning.

Background (see research/rod-labels.md):
  A .rod section is TEA-CBC(KEY_ROD) with an 8-byte IV. IV[0:3] is derived from
  the section tag (always exact). IV[3:8] carries the low 5 bytes of a runtime
  `product` term -- NOT in the file bytes. Only the FIRST cipher block's 8
  plaintext bytes depend on the IV; blocks 2..n are exact (CBC). For a zlib
  section that means the zlib header `78 da` (=plaintext[0:2]) is intact but
  deflate bytes 1..5 (=plaintext[3:8]) are corrupted, which kills inflate.

  Deflate byte 0 (=plaintext[2]) is exact and pins BFINAL/BTYPE/HLIT. The 5
  unknown deflate bytes 1..5 sit inside the dynamic-Huffman header, whose
  validity (code-length codes must satisfy the Kraft inequality, then the
  HLIT+HDIST code lengths must decode against the code-length Huffman using the
  EXACT tail bytes) is a very strong oracle. Combined with the reduced
  per-byte candidate sets (some IV multipliers are even -> fewer distinct
  values), a pruned DFS recovers the 5 bytes -> the whole section inflates.

Usage:
  rod_struc_decode.py <file.rod> [TAG]     # crack + inflate a zlib section
  rod_struc_decode.py --selftest           # validate the deflate parser
"""
import sys, os, zlib
import decrypt_modern as dm

MT = dm._ROD_MT
KS = dm._ROD_KS
OFF = dm.OFF_ROD

CLCL_ORDER = [16,17,18,0,8,7,9,6,10,5,11,4,12,3,13,2,14,1,15]


def iv_candidate_sets(tag, T):
    """For deflate bytes d[1..5] (= plaintext[3..8] = T[3:8] ^ IV[3:8]), return
    the list of candidate byte values for each, deduped. T = TEA_dec(C0)."""
    sets = []
    for i in range(3, 8):
        m = tag[1]
        # IV[i] = (s * MT[OFF[i]]) & 0xff ; s ranges over all 256 as the
        # product byte ranges 0..255 (additive KS shift is a bijection).
        ivvals = sorted({(s * MT[OFF[i]]) & 0xff for s in range(256)})
        cands = sorted({T[i] ^ v for v in ivvals})
        sets.append(cands)
    return sets  # sets[0] -> d[1], ... sets[4] -> d[5]


class NeedByte(Exception):
    def __init__(self, idx):
        self.idx = idx


class BitStream:
    """LSB-first bit reader over a byte-getter callback."""
    __slots__ = ("get", "byte", "nbits", "cur")
    def __init__(self, get):
        self.get = get      # get(byte_index) -> int  (may raise NeedByte)
        self.byte = 0
        self.nbits = 0
        self.cur = 0
    def read(self, n):
        while self.nbits < n:
            self.cur |= self.get(self.byte) << self.nbits
            self.byte += 1
            self.nbits += 8
        v = self.cur & ((1 << n) - 1)
        self.cur >>= n
        self.nbits -= n
        return v


def kraft_ok(lengths):
    """True iff code lengths form a valid (not over-subscribed) prefix code."""
    from collections import Counter
    c = Counter(l for l in lengths if l > 0)
    if not c:
        return True
    left = 1
    for bl in range(1, max(c) + 1):
        left <<= 1
        left -= c.get(bl, 0)
        if left < 0:
            return False  # over-subscribed
    return True  # left==0 complete, left>0 incomplete-but-valid


def build_huffman(lengths):
    """Canonical Huffman decode table: dict {(len,code)->symbol}. Returns None
    if over-subscribed."""
    from collections import Counter
    maxbl = max(lengths) if lengths else 0
    bl_count = Counter(l for l in lengths if l > 0)
    code = 0
    next_code = {}
    for bits in range(1, maxbl + 1):
        code = (code + bl_count.get(bits - 1, 0)) << 1
        next_code[bits] = code
    table = {}
    for sym, l in enumerate(lengths):
        if l:
            table[(l, next_code[l])] = sym
            next_code[l] += 1
    return table


def decode_sym(bs, table, maxlen):
    code = 0
    for l in range(1, maxlen + 1):
        code = (code << 1) | bs.read(1)
        if (l, code) in table:
            return table[(l, code)]
    return None


def parse_header(get):
    """Like validate_header but checks the Kraft inequality INCREMENTALLY (bails
    the instant a code-length set is over-subscribed) and propagates NeedByte so
    a DFS can branch lazily on unknown bytes. Returns True/False."""
    bs = BitStream(get)
    bs.read(1)                     # BFINAL
    if bs.read(2) != 2:            # BTYPE must be dynamic Huffman
        return False
    hlit = bs.read(5) + 257
    hdist = bs.read(5) + 1
    hclen = bs.read(4) + 4
    cl_lens = [0] * 19
    counts = [0] * 8
    for i in range(hclen):
        L = bs.read(3)
        cl_lens[CLCL_ORDER[i]] = L
        if L:
            counts[L] += 1
            # incremental over-subscription check
            left = 1
            over = False
            for bl in range(1, 8):
                left = (left << 1) - counts[bl]
                if left < 0:
                    over = True
                    break
            if over:
                return False
    cl_table = build_huffman(cl_lens)
    maxcl = max(cl_lens) if any(cl_lens) else 0
    if maxcl == 0:
        return False
    lengths = []
    n = hlit + hdist
    while len(lengths) < n:
        sym = decode_sym(bs, cl_table, maxcl)
        if sym is None:
            return False
        if sym < 16:
            lengths.append(sym)
        elif sym == 16:
            if not lengths:
                return False
            lengths += [lengths[-1]] * (bs.read(2) + 3)
        elif sym == 17:
            lengths += [0] * (bs.read(3) + 3)
        else:
            lengths += [0] * (bs.read(7) + 11)
        if len(lengths) > n:
            return False
    if not kraft_ok(lengths[:hlit]) or not kraft_ok(lengths[hlit:hlit + hdist]):
        return False
    return True


def validate_header(get):
    """Parse a deflate dynamic-Huffman header from byte-getter `get`. Returns
    True if it parses cleanly (code-length codes valid, all HLIT+HDIST code
    lengths decode and satisfy Kraft). Raises no exceptions."""
    try:
        bs = BitStream(get)
        bfinal = bs.read(1)  # noqa
        btype = bs.read(2)
        if btype != 2:
            return False  # we only handle dynamic Huffman
        hlit = bs.read(5) + 257
        hdist = bs.read(5) + 1
        hclen = bs.read(4) + 4
        cl_lens = [0] * 19
        for i in range(hclen):
            cl_lens[CLCL_ORDER[i]] = bs.read(3)
        if not kraft_ok(cl_lens):
            return False
        cl_table = build_huffman(cl_lens)
        maxcl = max(cl_lens) if any(cl_lens) else 0
        if maxcl == 0:
            return False
        # decode hlit+hdist code lengths
        lengths = []
        n = hlit + hdist
        while len(lengths) < n:
            sym = decode_sym(bs, cl_table, maxcl)
            if sym is None:
                return False
            if sym < 16:
                lengths.append(sym)
            elif sym == 16:
                if not lengths:
                    return False
                rep = bs.read(2) + 3
                lengths += [lengths[-1]] * rep
            elif sym == 17:
                rep = bs.read(3) + 3
                lengths += [0] * rep
            elif sym == 18:
                rep = bs.read(7) + 11
                lengths += [0] * rep
            if len(lengths) > n:
                return False
        lit_lens = lengths[:hlit]
        dist_lens = lengths[hlit:hlit + hdist]
        if not kraft_ok(lit_lens) or not kraft_ok(dist_lens):
            return False
        return True
    except Exception:
        return False


def crack_zlib_section(tag, cipher, plainlen):
    """Recover plaintext[3:8] for a zlib .rod section. Returns (guess5, inflated)
    or (None, None)."""
    T = dm.tea_decrypt_block(cipher[:8], dm.KEY_ROD)
    tail = dm.tea_cbc_decrypt(cipher[8:], dm.KEY_ROD, cipher[:8]) if len(cipher) > 8 else b""
    # deflate stream = plaintext[2:] ; d[0]=plaintext[2] known.
    # plaintext[0:3] = T[0:3]^IV[0:3]; IV[0:3] is exact (tag-derived, product=0).
    iv03 = dm.rod_block0_iv(tag, 0)[:3]
    p012 = bytes(T[i] ^ iv03[i] for i in range(3))
    assert p012[:2] == b"\x78\xda", "zlib magic mismatch: %r" % p012
    d0 = p012[2]
    sets = iv_candidate_sets(tag, T)  # d[1..5] candidates
    # tail deflate bytes start at deflate index 6 = tail[0:]
    total = 1
    for s in sets:
        total *= len(s)
    sys.stderr.write("naive space %d ; DFS with incremental-Kraft pruning...\n" % total)
    stats = {"nodes": 0, "inflates": 0}

    def try_inflate(assign):
        # fill any unknown byte not pinned by the header via full inflate
        rem = [i for i in range(1, 6) if i not in assign]
        import itertools
        for combo in itertools.product(*[sets[i - 1] for i in rem]):
            a = dict(assign)
            for i, v in zip(rem, combo):
                a[i] = v
            plain = bytes([0x78, 0xda, d0] + [a[i] for i in range(1, 6)]) + tail
            stats["inflates"] += 1
            try:
                out = zlib.decompress(plain)
            except Exception:
                continue
            return (tuple(a[i] for i in range(1, 6)), out)
        return None

    # optional restriction of the first branch (d1) for parallel workers
    d1lo = int(os.environ.get("D1LO", "0"))
    d1hi = int(os.environ.get("D1HI", str(len(sets[0]))))
    sets0_slice = sets[0][d1lo:d1hi]

    def dfs(assign):
        stats["nodes"] += 1
        if stats["nodes"] % 1000000 == 0:
            sys.stderr.write("  nodes=%d inflates=%d\n" % (stats["nodes"], stats["inflates"]))
            sys.stderr.flush()
        def get(i):
            if i == 0:
                return d0
            if 1 <= i <= 5:
                if i in assign:
                    return assign[i]
                raise NeedByte(i)
            return tail[i - 6]
        try:
            ok = parse_header(get)
        except NeedByte as e:
            cand = sets0_slice if e.idx == 1 else sets[e.idx - 1]
            for c in cand:
                a2 = dict(assign)
                a2[e.idx] = c
                r = dfs(a2)
                if r:
                    return r
            return None
        if not ok:
            return None
        return try_inflate(assign)

    r = dfs({})
    sys.stderr.write("nodes=%d inflates=%d\n" % (stats["nodes"], stats["inflates"]))
    if r:
        sys.stderr.write("HIT: %s inflated %d bytes\n" % (r[0].hex() if isinstance(r[0], bytes) else bytes(r[0]).hex(), len(r[1])))
        return r
    return (None, None)


def find_section(data, tag_str):
    open_m = ("[%s]\r\n" % tag_str).encode("latin1")
    close_m = ("\r\n[/%s]" % tag_str).encode("latin1")
    s = data.index(open_m) + len(open_m)
    e = data.index(close_m, s)
    pl = data[s:e]
    kind, storedlen, plainlen, cipher = dm.rod_section_cipher(pl)
    return tag_str.encode("latin1"), kind, plainlen, cipher


def selftest():
    # Validate parser against the known-good VW48 MWB zlib section (product=0).
    p = os.path.join(os.path.dirname(__file__), "samples/rod/EV_EPHBO18VW4810000_VW48.rod")
    data = open(p, "rb").read()
    tagb, kind, plainlen, cipher = find_section(data, "MWB")
    T = dm.tea_decrypt_block(cipher[:8], dm.KEY_ROD)
    iv = dm.rod_block0_iv(tagb, 0)
    p0 = bytes(T[i] ^ iv[i] for i in range(8))
    tail = dm.tea_cbc_decrypt(cipher[8:], dm.KEY_ROD, cipher[:8])
    d0 = p0[2]
    guess = (p0[3], p0[4], p0[5], p0[6], p0[7])
    def get(i):
        if i == 0: return d0
        if 1 <= i <= 5: return guess[i-1]
        return tail[i-6]
    ok = validate_header(get)
    print("selftest MWB header valid (should be True):", ok)
    # also confirm a wrong first byte fails
    def getbad(i):
        return 0xAA if i == 1 else get(i)
    print("selftest corrupted header valid (should be False):", validate_header(getbad))


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "--selftest":
        selftest()
        sys.exit(0)
    path = sys.argv[1]
    tag = sys.argv[2] if len(sys.argv) > 2 else "STRUC"
    data = open(path, "rb").read()
    tagb, kind, plainlen, cipher = find_section(data, tag)
    print("tag=%s kind=%s plainlen=%d cipherlen=%d" % (tag, kind, plainlen, len(cipher)))
    if kind != "zlib":
        print("not a zlib section")
        sys.exit(1)
    guess, out = crack_zlib_section(tagb, cipher, plainlen)
    if guess:
        g = bytes(guess)
        print("RECOVERED plaintext[3:8] =", g.hex())
        print("inflated %d bytes (expected %d)" % (len(out), plainlen))
        outp = "/tmp/decoded_%s_%s.txt" % (os.path.basename(path), tag)
        open(outp, "wb").write(out)
        print("wrote", outp)
    else:
        print("FAILED to recover  (increase compute / use rod_crack Rust tool)")
