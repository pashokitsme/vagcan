#!/usr/bin/env python3
"""extract_rsa_key.py -- extract the embedded RSA-1024 key-transport private key
from VCDS-arm64-unpacked.exe.

The VCDS diagnostic link is AES-256 in a per-channel keystream mode
(KS_cid = AES256(K).ecb_encrypt(IV_TABLE[cid])).  The 32-byte session key K is
NOT derived from the b6/b7 handshake -- it is RSA key-transport: the cable/
transport delivers a 128-byte RSA-1024-wrapped blob, and the app RSA-decrypts it
with a *static embedded private key* to recover K (sole AES-256 set-key path is
0x140072ec0 -> RSA-CRT decrypt 0x14007ce68 -> memcpy K into cipher-ctx+0x5da4 ->
AES schedule 0x14007b140).  The private key is DER-encoded in .rdata.

Clean-room interop: the owner extracts a key embedded in software they possess to
talk to their own genuine cable / own car.  No server, no circumvention.

The secret lives in the binary (already in the repo), so this regenerates it on
demand rather than committing a loose PEM.

usage: extract_rsa_key.py [--pem]
"""
import sys
import pefile
from Crypto.PublicKey import RSA

EXE = "bin/VCDS-arm64-unpacked.exe"
IMAGE_BASE = 0x140000000
KEY_VMA = 0x140171A30  # DER RSAPrivateKey, 609 bytes (30 82 02 5d ...)


def extract():
    pe = pefile.PE(EXE)
    data = open(EXE, "rb").read()
    rva = KEY_VMA - IMAGE_BASE
    for s in pe.sections:
        if s.VirtualAddress <= rva < s.VirtualAddress + max(s.Misc_VirtualSize, s.SizeOfRawData):
            off = s.PointerToRawData + (rva - s.VirtualAddress)
            break
    else:
        raise SystemExit("KEY_VMA not in any section")
    assert data[off] == 0x30 and data[off + 1] == 0x82, "not a DER SEQUENCE(0x82)"
    total = 4 + ((data[off + 2] << 8) | data[off + 3])
    return RSA.import_key(data[off:off + total])


if __name__ == "__main__":
    k = extract()
    print(f"# RSA-{k.size_in_bits()}  e={k.e}  private={k.has_private()}  pq==n={k.p*k.q==k.n}",
          file=sys.stderr)
    if "--pem" in sys.argv:
        print(k.export_key().decode())
    else:
        print(f"n = {k.n:#x}", file=sys.stderr)
