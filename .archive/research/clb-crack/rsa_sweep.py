#!/usr/bin/env python3
"""rsa_sweep.py -- empirical: RSA-decrypt every candidate blob in the capture
with the embedded private key and test the recovered 32 bytes against the
KS_F3 oracle. Also test symmetric interpretations."""
import sys, itertools, hashlib
sys.path.insert(0, ".")
from Crypto.Cipher import AES
from Crypto.PublicKey import RSA
from usbpcap import reassemble_frames
from link_cipher import IV_TABLE
from extract_rsa_key import extract

IV = IV_TABLE[4]
WANT = bytes.fromhex("02a999f6da7c9c3a")  # KS_F3[6:14]

def oracle(k):
    if len(k) != 32:
        return None
    for m, fn in (("enc", lambda c: c.encrypt(IV)), ("dec", lambda c: c.decrypt(IV))):
        ks = fn(AES.new(bytes(k), AES.MODE_ECB))
        if ks[6:14] == WANT:
            return (m, ks)
    return None

def test_key_material(label, buf):
    """Given decrypted bytes, try any 32-byte window + sha256 as K."""
    hits = []
    for i in range(0, max(1, len(buf) - 31)):
        w = buf[i:i+32]
        if len(w) == 32:
            r = oracle(w)
            if r:
                hits.append((f"{label}[win{i}]", w, r))
    r = oracle(hashlib.sha256(buf).digest())
    if r:
        hits.append((f"sha256({label})", hashlib.sha256(buf).digest(), r))
    return hits

def rsa_raw(key, ct):
    """raw c^d mod n -> big-endian 128 bytes."""
    n = key.n; d = key.d
    c = int.from_bytes(ct, "big")
    if c >= n:
        return None
    m = pow(c, d, n)
    return m.to_bytes(128, "big")

def unpad_variants(m):
    """yield labelled candidate payloads from a raw RSA output block."""
    yield ("raw", m)
    # PKCS#1 v1.5 type 2: 00 02 PS 00 M
    if len(m) >= 11 and m[0] == 0x00 and m[1] == 0x02:
        z = m.find(b"\x00", 2)
        if z > 0:
            yield ("pkcs1v15", m[z+1:])
    # type 1: 00 01 FF.. 00 M
    if len(m) >= 11 and m[0] == 0x00 and m[1] == 0x01:
        z = m.find(b"\x00", 2)
        if z > 0:
            yield ("pkcs1v15sig", m[z+1:])
    # trailing 32 bytes / leading 32 bytes
    yield ("tail32", m[-32:])
    yield ("head_after0", m.lstrip(b"\x00"))

def main(path):
    key = extract()
    frames = list(reassemble_frames(path))
    print(f"# {path}: {len(frames)} frames")
    # group by (dir, opcode) preserving order
    byop = {}
    seq = []
    for f in frames:
        p = f["payload"]
        if not p:
            continue
        op = p[0]
        rec = (f["dir"], op, bytes(p[1:]))  # strip opcode
        seq.append(rec)
        byop.setdefault((f["dir"], op), []).append(bytes(p[1:]))

    # candidate ciphertext byte-strings
    cands = {}
    # 1) each opcode's concatenated payloads (per dir), and per-frame
    for (d, op), lst in byop.items():
        cat = b"".join(lst)
        cands[f"{d}_{op:02x}_cat({len(lst)})"] = cat
        for i, pl in enumerate(lst):
            cands[f"{d}_{op:02x}_#{i}"] = pl
    # 2) whole IN / OUT streams concatenated
    in_all = b"".join(r[2] for r in seq if r[0] == "IN")
    out_all = b"".join(r[2] for r in seq if r[0] == "OUT")
    cands["IN_all"] = in_all
    cands["OUT_all"] = out_all
    # 3) runs of consecutive same-opcode frames concatenated (with growing prefixes)
    #    e.g. 0b blocks 0..k
    for d in ("IN", "OUT"):
        run = []
        lastop = None
        for r in seq:
            if r[0] != d:
                continue
            if r[1] == lastop:
                run.append(r[2])
            else:
                run = [r[2]]
                lastop = r[1]
            if len(run) >= 2:
                cands[f"{d}_{lastop:02x}_run{len(run)}"] = b"".join(run)

    print(f"# {len(cands)} candidate byte-strings")

    all_hits = []
    for label, buf in cands.items():
        # sliding 128-byte windows for RSA (and also try buf[3:] to mimic blob+3)
        variants = [("", buf)]
        if len(buf) > 3:
            variants.append(("+3", buf[3:]))
        for suf, b in variants:
            # try b itself if <=128 and reduce; also sliding 128 windows
            wins = []
            if 1 <= len(b) <= 128:
                wins.append((f"{label}{suf}", b))
            for i in range(0, max(0, len(b) - 128) + 1, 1):
                w = b[i:i+128]
                if len(w) == 128:
                    wins.append((f"{label}{suf}[rsa128@{i}]", w))
            for wl, w in wins:
                m = rsa_raw(key, w if len(w) == 128 else w.rjust(128, b"\x00"))
                if m is None:
                    continue
                for ulabel, payload in unpad_variants(m):
                    h = test_key_material(f"{wl}/{ulabel}", payload)
                    all_hits += h

    if all_hits:
        for lab, k, r in all_hits:
            print(f"\n*** RSA HIT: {lab}\n    mode={r[0]} K={k.hex()}")
    else:
        print("# no RSA-decrypt candidate reproduced the oracle")

if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "../captures/reading-ecus.pcapng")
