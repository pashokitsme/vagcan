#!/usr/bin/env python3
"""rod_crack_prep.py -- prepare `crack_input.bin` for the Rust brute-forcer
(rod_crack) for one zlib .rod section, and (with a recovered 5-byte guess)
reconstruct + inflate that section.

Usage:
  rod_crack_prep.py prep  <file.rod> <TAG>            # -> crack_input.bin
  rod_crack_prep.py decode <file.rod> <TAG> <hex5>    # inflate with recovered bytes
"""
import sys, os, struct, zlib
import decrypt_modern as dm


def section_cipher(path, tag):
    data = open(path, "rb").read()
    op = ("[%s]\r\n" % tag).encode("latin1")
    cl = ("\r\n[/%s]" % tag).encode("latin1")
    s = data.index(op) + len(op)
    e = data.index(cl, s)
    kind, storedlen, plainlen, cipher = dm.rod_section_cipher(data[s:e])
    return kind, plainlen, cipher


def candidate_sets(tagb, T):
    MT, OFF = dm._ROD_MT, dm.OFF_ROD
    sets = []
    for i in range(3, 8):
        ivvals = sorted({(x * MT[OFF[i]]) & 0xff for x in range(256)})
        sets.append(sorted({T[i] ^ v for v in ivvals}))
    return sets


def prep(path, tag):
    tagb = tag.encode("latin1")
    kind, plainlen, cipher = section_cipher(path, tag)
    if kind != "zlib":
        print("section is not zlib (kind=%s); TEA sections lose only 8 bytes" % kind)
        return
    T = dm.tea_decrypt_block(cipher[:8], dm.KEY_ROD)
    iv03 = dm.rod_block0_iv(tagb, 0)[:3]
    p012 = bytes(T[i] ^ iv03[i] for i in range(3))
    assert p012[:2] == b"\x78\xda", "zlib magic mismatch %r" % p012
    d0 = p012[2]
    sets = candidate_sets(tagb, T)
    tail = dm.tea_cbc_decrypt(cipher[8:], dm.KEY_ROD, cipher[:8])
    with open("crack_input.bin", "wb") as f:
        f.write(struct.pack("<I", plainlen)); f.write(bytes([d0]))
        for cs in sets:
            f.write(struct.pack("<H", len(cs))); f.write(bytes(cs))
        f.write(struct.pack("<I", len(tail))); f.write(tail)
    print("prepped %s [%s] plainlen=%d d0=%#x setsizes=%s taillen=%d" % (
        os.path.basename(path), tag, plainlen, d0, [len(x) for x in sets], len(tail)))


def decode(path, tag, hex5):
    tagb = tag.encode("latin1")
    kind, plainlen, cipher = section_cipher(path, tag)
    rec = bytes.fromhex(hex5)
    T = dm.tea_decrypt_block(cipher[:8], dm.KEY_ROD)
    iv03 = dm.rod_block0_iv(tagb, 0)[:3]
    p0 = bytes(T[i] ^ iv03[i] for i in range(3)) + rec
    tail = dm.tea_cbc_decrypt(cipher[8:], dm.KEY_ROD, cipher[:8])
    out = zlib.decompress(p0 + tail)
    assert len(out) == plainlen, "len %d != %d" % (len(out), plainlen)
    outp = "/tmp/decoded_%s_%s.txt" % (os.path.basename(path), tag)
    open(outp, "wb").write(out)
    print("inflated %d bytes -> %s" % (len(out), outp))
    print(repr(out[:400]))


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "prep":
        prep(sys.argv[2], sys.argv[3])
    elif cmd == "decode":
        decode(sys.argv[2], sys.argv[3], sys.argv[4])
