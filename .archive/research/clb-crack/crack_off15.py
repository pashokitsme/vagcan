#!/usr/bin/env python3
"""crack_off15.py -- reverse the b8/b7 link-block trailer byte (block off15).

Established facts (see research/vag-hex-framing.md "Link cipher"):
  * plain[i] = cipher[i] ^ KS_channel[i]        (pure position XOR keystream)
  * off14 = plaintext per-frame counter (KS14 = 0), NOT an input to off15.
  * off15 = a CHECKSUM over block content, INDEPENDENT of off14, LIKELY of the
    form  off15 = KS15_channel ^ C(plaintext bytes)  with KS15 constant per chan.

Because the whole block is a byte-local XOR keystream, an XOR checksum over the
PLAINTEXT reduces to a test on the CIPHERTEXT alone:

    plain15 = XOR_{i in R} plain[i]                     (XOR checksum, range R)
    cipher15 = plain15 ^ KS15
             = ( XOR_{i in R} cipher[i] ) ^ ( XOR_{i in R} KS[i] ^ KS15 )
             = ( XOR_{i in R} cipher[i] ) ^ K_channel

so  cipher15 ^ XOR_{i in R} cipher[i]  must be CONSTANT per channel. No keystream
recovery needed. (A SUM/CRC checksum does NOT reduce like this -- it needs the
actual plaintext, handled in Phase B via recovered keystreams.)
"""

import sys
import os
from collections import Counter, defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from usbpcap import reassemble_frames
from link_cipher import recover_all_channels, decrypt_link


def load(path):
    b8, b7 = [], []
    for f in reassemble_frames(path):
        p = f["payload"]
        if not p or len(p) < 17:
            continue
        blk = bytes(p[1:17])
        if p[0] == 0xB8 and f["dir"] == "OUT":
            b8.append(blk)
        elif p[0] == 0xB7 and f["dir"] == "IN":
            b7.append(blk)
    return b8, b7


def chan_key(b):
    """Constant addressing header identifying a logical channel."""
    return (b[0], b[2], b[3], b[5])


# ---------------------------------------------------------------------------
# Phase A: XOR checksum over ciphertext -> per-channel constant?
# ---------------------------------------------------------------------------
def xor_range(b, rng):
    x = 0
    for i in rng:
        x ^= b[i]
    return x


def phase_a(blocks, label):
    """For each candidate byte range, test whether
    cipher15 ^ XOR(cipher over range) is constant within every channel."""
    # ranges to try (off15 excluded from its own checksum). off14 excluded
    # (counter, proven independent). Include header/data permutations.
    ranges = {
        "0..14 (all but cnt)": range(0, 14),
        "0..13": range(0, 14),  # same as above (14 exclusive) -- keep name clarity
        "1..14": range(1, 14),
        "0..6": range(0, 6),
        "6..14": range(6, 14),
        "6..13": range(6, 13),
        "7..14": range(7, 14),
        "0..15 incl cnt": range(0, 15),
        "0..5 header": range(0, 6),
        "2..14": range(2, 14),
    }
    # dedup by tuple of indices
    seen = {}
    for name, r in ranges.items():
        seen.setdefault(tuple(r), name)

    by_chan = defaultdict(list)
    for b in blocks:
        by_chan[chan_key(b)].append(b)

    print(f"\n=== Phase A ({label}): XOR-checksum-over-ciphertext, per-channel constant test ===")
    print(f"  {len(blocks)} blocks in {len(by_chan)} channels")
    best = None
    for idxs, name in seen.items():
        ok_ch = 0
        ok_frames = 0
        tot_frames = 0
        for ck, bs in by_chan.items():
            consts = {xor_range(b, idxs) ^ b[15] for b in bs}
            tot_frames += len(bs)
            if len(consts) == 1:
                ok_ch += 1
                ok_frames += len(bs)
        print(f"  range {name:20s}: {ok_ch:3d}/{len(by_chan)} channels constant, "
              f"{ok_frames:4d}/{tot_frames} frames")
        if best is None or ok_frames > best[2]:
            best = (name, idxs, ok_frames, tot_frames, ok_ch, len(by_chan))
    return best


# ---------------------------------------------------------------------------
# Phase B: recover plaintext, test SUM / additive / CRC checksums over plaintext.
# ---------------------------------------------------------------------------
def build_plaintext_frames(b8, b7):
    """Return list of (chan_key, direction, cipher_block, plain_block_partial).
    plain_block_partial: list[16] of ints or None (None = unknown header byte)."""
    # recover_all_channels wants [(idx, block)] lists
    ks_map = recover_all_channels(
        [(i, b) for i, b in enumerate(b8)],
        [(i, b) for i, b in enumerate(b7)],
    )
    frames = []
    for direction, blocks in (("req", b8), ("resp", b7)):
        for b in blocks:
            ck = chan_key(b)
            if ck not in ks_map:
                continue
            ks, pci, sid, nreq, nrsp = ks_map[ck]
            dec = decrypt_link(b, ks)  # list[16], None where ks unknown
            # off14 plaintext == cipher (KS14 = 0, per RE)
            plain = list(dec)
            plain[14] = b[14]
            frames.append((ck, direction, b, plain))
    return frames, ks_map


def sum_range(vals, rng):
    """Sum of plaintext bytes over range; returns None if any is unknown."""
    s = 0
    for i in rng:
        if vals[i] is None:
            return None
        s += vals[i]
    return s & 0xFF


def phase_b_sum(frames, label):
    """Test additive checksum on plaintext: does there exist per-channel
    (KS15, C0) s.t. cipher15 = KS15 ^ ((C0 + sum(plain over R)) & 0xff)?

    We only sum bytes we KNOW (off6..13 data region + off1 SID). Unknown header
    bytes fold into C0 (constant per channel), so restrict R to known offsets."""
    ranges = {
        "6..13 data": range(6, 14),
        "6..13 no cnt": range(6, 14),
        "7..13": range(7, 14),
        "1,6..13": [1] + list(range(6, 14)),
        "6..14 (w/cnt)": range(6, 15),  # sanity: should FAIL (cnt independent)
    }
    seen = {}
    for name, r in ranges.items():
        seen.setdefault(tuple(r), name)

    by_chan = defaultdict(list)
    for ck, d, b, plain in frames:
        by_chan[ck].append((b, plain))

    print(f"\n=== Phase B ({label}): additive checksum on recovered plaintext ===")
    for idxs, name in seen.items():
        ok_ch = 0
        ok_frames = 0
        tot = 0
        chans_with_data = 0
        for ck, items in by_chan.items():
            sums = []
            usable = True
            for b, plain in items:
                s = sum_range(plain, idxs)
                if s is None:
                    usable = False
                    break
                sums.append((b[15], s))
            if not usable or len(sums) < 2:
                continue
            chans_with_data += 1
            tot += len(sums)
            # find KS15 s.t. (cipher15 ^ KS15 - s) mod 256 constant
            found = False
            for ks15 in range(256):
                c0s = {((c15 ^ ks15) - s) & 0xFF for c15, s in sums}
                if len(c0s) == 1:
                    found = True
                    break
            if found:
                ok_ch += 1
                ok_frames += len(sums)
        print(f"  range {name:16s}: {ok_ch:3d}/{chans_with_data} channels fit, "
              f"{ok_frames:4d}/{tot} frames")


def crc8(data, poly, init=0x00, xorout=0x00, refin=False, refout=False):
    def rev8(x):
        return int(f"{x:08b}"[::-1], 2)
    crc = init
    for b in data:
        if refin:
            b = rev8(b)
        crc ^= b
        for _ in range(8):
            crc = ((crc << 1) ^ poly) & 0xFF if (crc & 0x80) else (crc << 1) & 0xFF
    if refout:
        crc = rev8(crc)
    return crc ^ xorout


def phase_b_crc(frames, label):
    """Test CRC-8 (several polys) over the KNOWN plaintext data region 6..13,
    absorbing header+KS15 into a per-channel XOR constant (CRC is not XOR-linear,
    but for a FIXED-length prefix the header contributes a constant pre-state; we
    approximate by testing the data-only region and folding to a per-chan XOR)."""
    polys = [0x07, 0x1D, 0x31, 0x2F, 0x9B, 0xD5, 0x39, 0x1D]
    by_chan = defaultdict(list)
    for ck, d, b, plain in frames:
        by_chan[ck].append((b, plain))
    print(f"\n=== Phase B ({label}): CRC-8 over plaintext off6..13 (per-chan XOR fold) ===")
    for poly in sorted(set(polys)):
        ok_ch = 0
        tot_ch = 0
        for ck, items in by_chan.items():
            vals = []
            usable = True
            for b, plain in items:
                data = plain[6:14]
                if any(x is None for x in data):
                    usable = False
                    break
                vals.append((b[15], crc8(data, poly)))
            if not usable or len(vals) < 2:
                continue
            tot_ch += 1
            consts = {c15 ^ crc for c15, crc in vals}
            if len(consts) == 1:
                ok_ch += 1
        print(f"  poly {poly:#04x}: {ok_ch:3d}/{tot_ch} channels constant")


def report_f3(b8, label):
    """Sanity: dump the f3 channel off15 vs candidate on the reference blocks."""
    def is_f3(b):
        return b[0] == 0xF3 and b[2] == 0x44 and b[3] == 0xDD and b[5] == 0x5F
    f3 = [b for b in b8 if is_f3(b)]
    if not f3:
        return
    print(f"\n=== f3 channel detail ({label}) ===  {len(f3)} b8 frames")
    # cipher15 ^ XOR(cipher 0..14)
    variants = Counter()
    for b in f3:
        variants[(b[15] ^ xor_range(b, range(0, 14)))] += 1
    print(f"  cipher15 ^ XOR(cipher 0..13): {dict(variants)}")


def main(path):
    b8, b7 = load(path)
    print(f"# {path}: {len(b8)} b8, {len(b7)} b7 blocks")

    best_req = phase_a(b8, "b8 requests")
    best_rsp = phase_a(b7, "b7 responses")
    # combined direction-agnostic (channel shares keystream both dirs, but off4
    # direction bit differs -> header const differs per direction; test per-dir)

    report_f3(b8, "b8")

    frames, ks_map = build_plaintext_frames(b8, b7)
    print(f"\n# recovered keystreams for {len(ks_map)} channels; "
          f"{len(frames)} frames with (partial) plaintext")
    phase_b_sum(frames, "req+resp")
    phase_b_crc(frames, "req+resp")

    print("\n=== BEST XOR range ===")
    for tag, best in (("b8", best_req), ("b7", best_rsp)):
        name, idxs, ok_frames, tot, ok_ch, tot_ch = best
        print(f"  {tag}: range {name!r} -> {ok_ch}/{tot_ch} channels, "
              f"{ok_frames}/{tot} frames constant")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "../captures/reading-ecus.pcapng")
