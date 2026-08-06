"""Standalone `.rod` section reader, for looking at what the sweep opened.

Research tooling. `crates/vag-data/src/rod.rs` is authoritative; this is a
re-implementation of the *decode* half only (no key search), so that a decoded
section can be pulled out and compared without a cargo build competing with a
running sweep for the machine's ten cores.

    python3 rodread.py <file.rod> <TAG> [--iv3to8 aa,bb,cc,dd,ee] [--anchor 0xac]
                                        [--keys ~/.vagcan/labels/rod-keys.json]
                                        [--out blob.bin]

A *classic* section needs only `iv[3:8]` — `iv[0:3]` comes from the tag. A
*shifted* one (`research/labels/tttext2.md` §3.3) additionally needs the
deflate anchor, because its `iv[0:3]` is masked: `iv[0]` and `iv[1]` are read
back off the zlib magic, and `iv[2]` off the anchor.
"""

import argparse
import json
import os
import struct
import sys
import zlib

KEY_ROD = [0x029B76A4, 0xCB6DB50A, 0x71395D29, 0x0DBC09C2]
OFF_ROD = [0x07, 0xCA, 0x22, 0x99, 0x3E, 0x88, 0xC3, 0x76]
DELTA = 0x9E3779B9
SUM0 = 0xC6EF3720
M32 = 0xFFFFFFFF

_SRC = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "crates", "vag-data", "src", "rod")
with open(os.path.join(_SRC, "rod_mt.bin"), "rb") as fh:
	MT = fh.read()
with open(os.path.join(_SRC, "rod_ks.bin"), "rb") as fh:
	KS = fh.read()


def tea_decrypt_block(block, key=KEY_ROD):
	v0, v1 = struct.unpack("<II", block)
	s = SUM0
	for _ in range(32):
		v1 = (v1 - ((((v0 << 4) & M32) + key[2]) ^ (v0 + s) ^ ((v0 >> 5) + key[3]))) & M32
		v0 = (v0 - ((((v1 << 4) & M32) + key[0]) ^ (v1 + s) ^ ((v1 >> 5) + key[1]))) & M32
		s = (s - DELTA) & M32
	return struct.pack("<II", v0, v1)


def tea_cbc_decrypt(data, iv, key=KEY_ROD):
	out = bytearray()
	prev = bytes(iv)
	for i in range(0, len(data) - len(data) % 8, 8):
		blk = data[i : i + 8]
		dec = tea_decrypt_block(blk, key)
		out += bytes(a ^ b for a, b in zip(dec, prev))
		prev = blk
	return bytes(out)


def rod_block0_iv(tag):
	"""The tag-derived IV, with `product` taken as zero — `rod.rs::rod_block0_iv`."""
	seed = bytes(tag[:3]) + b"\0" * 5
	m = tag[1]
	iv = bytearray(8)
	for i in range(8):
		s = (seed[i] + KS[(m * (i + 2)) & 0xFF]) & 0xFF
		iv[i] = (s * MT[OFF_ROD[i]]) & 0xFF
	return bytes(iv)


def sections(data):
	"""Yield (tag, compressed, plainlen, cipher) using the shipped framing."""
	pos = 0
	while True:
		i = data.find(b"[", pos)
		if i < 0:
			return
		j = i + 1
		while j < len(data) and 65 <= data[j] <= 90 and j - i - 1 < 8:
			j += 1
		if not (2 <= j - i - 1 <= 8 and data[j : j + 3] == b"]\r\n"):
			pos = i + 1
			continue
		tag = data[i + 1 : j]
		start = j + 3
		close = b"\r\n[/" + tag + b"]\r\n"
		k = data.find(close, start)
		if k < 0:
			return
		payload = data[start:k]
		pos = k + len(close)
		if len(payload) < 6:
			continue
		read1 = int.from_bytes(payload[0:3], "big")
		storedlen = read1 & 0x7FFFFF
		compressed = (read1 & 0x800000) == 0
		plainlen = int.from_bytes(payload[3:6], "big")
		if storedlen % 8 or len(payload) < 6 + storedlen:
			continue
		yield tag, compressed, plainlen, payload[6 : 6 + storedlen]


def main():
	ap = argparse.ArgumentParser()
	ap.add_argument("file")
	ap.add_argument("tag")
	ap.add_argument("--iv3to8")
	ap.add_argument("--anchor")
	ap.add_argument("--keys", default=os.path.expanduser("~/.vagcan/labels/rod-keys.json"))
	ap.add_argument("--out")
	a = ap.parse_args()

	data = open(a.file, "rb").read()
	want = a.tag.encode()
	for tag, compressed, plainlen, cipher in sections(data):
		if tag != want:
			continue
		iv3to8 = None
		if a.iv3to8:
			iv3to8 = bytes(int(x, 16) for x in a.iv3to8.replace(",", " ").split())
		else:
			try:
				keys = json.load(open(a.keys))
			except OSError:
				keys = {}
			k = keys.get("%s\t%s" % (os.path.basename(a.file), tag.decode()))
			if k:
				iv3to8 = bytes(k)
		if iv3to8 is None:
			sys.exit("no iv[3:8] for [%s]: pass --iv3to8" % tag.decode())

		t = tea_decrypt_block(cipher[0:8])
		model = rod_block0_iv(tag)
		iv = bytearray(model[:3]) + bytearray(iv3to8)
		shifted = not (t[0] ^ model[0] == 0x78 and t[1] ^ model[1] == 0xDA)
		if shifted:
			if not a.anchor:
				sys.exit("[%s] is shifted: pass --anchor" % tag.decode())
			iv[0] = t[0] ^ 0x78
			iv[1] = t[1] ^ 0xDA
			iv[2] = t[2] ^ int(a.anchor, 0)
		dec = tea_cbc_decrypt(cipher, bytes(iv))
		out = zlib.decompress(dec) if compressed else dec[:plainlen]
		print(
			"[%s] %s %s: %d bytes (declared %d)%s"
			% (
				tag.decode(),
				"zlib" if compressed else "tea",
				"shifted" if shifted else "classic",
				len(out),
				plainlen,
				"" if len(out) == plainlen else "  *** LENGTH MISMATCH ***",
			),
			file=sys.stderr,
		)
		if a.out:
			open(a.out, "wb").write(out)
		else:
			sys.stdout.buffer.write(out[:2000])
		return
	sys.exit("no [%s] section in %s" % (a.tag, a.file))


if __name__ == "__main__":
	main()
