#!/usr/bin/env python3
"""crack_session_key.py -- recover the AES-256 session key from the setup-phase
wire bytes by verifying against the already-recovered KS_F3 keystream ground
truth.  Clean-room interop, owner's own capture.

Ground truth (link_cipher.py): primary channel off0=0xf3 -> msg_type=3 ->
cid=(3+1)&0xf=4 -> IV row = IV_TABLE[4].  Schedule: KS = AES256_ECB(K).enc(IV).
Known keystream bytes KS_F3[6..13] = 02 a9 99 f6 da 7c 9c 3a (+ off1=0xbd).

If any candidate K makes AES256(K).encrypt(IV_TABLE[4])[6:14] == those 8 bytes,
K is the session key -- proven against ground truth.
"""
import sys, hashlib, itertools
sys.path.insert(0, ".")
from Crypto.Cipher import AES
from usbpcap import reassemble_frames
from link_cipher import IV_TABLE

IV = IV_TABLE[4]
KNOWN = {6: 0x02, 7: 0xA9, 8: 0x99, 9: 0xF6, 10: 0xDA, 11: 0x7C, 12: 0x9C, 13: 0x3A, 1: 0xBD}


def ks_matches(k):
    if len(k) != 32:
        return None
    for mode, transform in (("enc", lambda c: c.encrypt(IV)), ("dec", lambda c: c.decrypt(IV))):
        ks = transform(AES.new(bytes(k), AES.MODE_ECB))
        n = sum(1 for off, v in KNOWN.items() if ks[off] == v)
        if n >= 6:  # 6+/9 known bytes = decisive
            return (mode, n, ks)
    return None


def collect(path):
    frames = list(reassemble_frames(path))
    setup = []
    for f in frames:
        p = f["payload"]
        if p and p[0] == 0xB8:
            break
        if p:
            setup.append((f["dir"], bytes(p)))  # includes opcode
    return setup


def candidates(setup):
    """Yield (label, 32-byte-key) candidates."""
    # raw payload pools
    allbytes = b"".join(pl for _, pl in setup)
    out_bytes = b"".join(pl for d, pl in setup if d == "OUT")
    in_bytes = b"".join(pl for d, pl in setup if d == "IN")
    # named blobs
    named = {}
    for d, pl in setup:
        op = pl[0]
        if op in (0x09, 0xB6, 0xB7):
            named.setdefault(f"{d}_{op:02x}", []).append(pl[1:])
    # 1) sliding 32-byte windows over concatenations
    for name, buf in (("all", allbytes), ("out", out_bytes), ("in", in_bytes)):
        for i in range(0, max(0, len(buf) - 31)):
            yield f"win_{name}[{i}]", buf[i:i + 32]
    # 2) concatenations of named blobs (b7#1||b7#2, b6-based, etc.)
    b7 = named.get("IN_b7", [])
    b6 = named.get("OUT_b6", [])
    o09 = named.get("OUT_09", [])
    i09 = named.get("IN_09", [])
    combos = []
    if len(b7) >= 2:
        combos += [("b7_1||b7_2", b7[0] + b7[1]), ("b7_2||b7_1", b7[1] + b7[0]),
                   ("b7_1||b7_1", b7[0] + b7[0])]
    if b6:
        combos += [("b6", b6[0]), ("b6||b6", b6[0] + b6[0])]
    for lab, blob in combos:
        if len(blob) >= 32:
            yield lab, blob[:32]
    # 3) SHA-256 derived (KDF-style) from many input combos
    pools = {"b6": b6[0] if b6 else b"", "b7_1": b7[0] if b7 else b"",
             "b7_2": b7[1] if len(b7) > 1 else b"", "o09": o09[0] if o09 else b"",
             "i09": i09[0] if i09 else b"", "all": allbytes, "out": out_bytes, "in": in_bytes}
    keys = [k for k, v in pools.items() if v]
    for r in (1, 2, 3):
        for combo in itertools.permutations(keys, r):
            blob = b"".join(pools[c] for c in combo)
            yield "sha256(" + "+".join(combo) + ")", hashlib.sha256(blob).digest()


def main(path):
    setup = collect(path)
    print(f"# {path}: {len(setup)} setup frames, "
          f"{sum(len(pl) for _,pl in setup)} total bytes")
    tested = 0
    for label, k in candidates(setup):
        tested += 1
        r = ks_matches(k)
        if r:
            mode, n, ks = r
            print(f"\n*** HIT ({n}/9 known bytes, mode={mode}) : {label}")
            print(f"    K   = {bytes(k).hex()}")
            print(f"    KS  = {ks.hex(' ')}")
            print(f"    KS[6:14] = {ks[6:14].hex(' ')}  (want 02 a9 99 f6 da 7c 9c 3a)")
    print(f"\ntested {tested} candidates")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "../captures/reading-ecus.pcapng")
