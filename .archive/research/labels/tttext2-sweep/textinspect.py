"""What is in an opened text-table blob — and is it the same thing as TTTEXT?

Given one or two inflated `[TXT]`-style sections (see `rodread.py`), this
reports the structure a claim about them has to rest on:

* the record framing (`NNNNNN,<payload>\\r\\n`), and how many lines fail it —
  a decode that is subtly wrong shows up here as a handful of ragged records,
  which is exactly the failure mode this project has been bitten by twice;
* the id namespace: count, range, whether ids are strictly increasing;
* the character-frequency bands of `tttext-codec.md` §1.1 — three flat bands
  (26 + 26 + 14) is the signature of a per-record substitution, and its absence
  would mean a different codec;
* the payload shape: field count under the 14-glyph separator, payload length;
* against a second blob: how the id namespaces overlap, and whether a shared id
  carries the same payload bytes.

    python3 inspect.py TTTEXT2.TXT.bin [--against TTTEXT.TXT.bin]
"""

import argparse
import collections
import re
import sys

REC = re.compile(rb"^(\d{6}),(.*)$")
GLYPHS = b"0123456789,._-"


def parse(blob):
	"""Split into records, keeping the ones that fail so they can be counted."""
	lines = blob.split(b"\r\n")
	trailing = lines.pop() if lines and lines[-1] == b"" else None
	good, bad = {}, []
	order = []
	for ln in lines:
		m = REC.match(ln)
		if m:
			i = int(m.group(1))
			good[i] = m.group(2)
			order.append(i)
		else:
			bad.append(ln)
	return good, bad, order, trailing


def bands(payloads):
	c = collections.Counter()
	for p in payloads:
		c.update(p)
	lower = [c[b] for b in range(ord("a"), ord("z") + 1)]
	upper = [c[b] for b in range(ord("A"), ord("Z") + 1)]
	glyph = [c[b] for b in GLYPHS]
	rest = [(bytes([b]).decode("latin-1"), n) for b, n in c.items() if not (chr(b).isalpha() or b in GLYPHS)]
	rest.sort(key=lambda kv: -kv[1])
	return lower, upper, glyph, rest


def band(name, xs):
	if not xs or max(xs) == 0:
		return "%-8s all zero" % name
	spread = (max(xs) - min(xs)) / (sum(xs) / len(xs))
	return "%-8s n=%2d  min=%-9d max=%-9d spread=%.1f%%" % (name, len(xs), min(xs), max(xs), 100 * spread)


def report(tag, blob):
	good, bad, order, trailing = parse(blob)
	print("== %s: %d bytes" % (tag, len(blob)))
	print("   records parsed %d, malformed lines %d, trailing %r" % (len(good), len(bad), trailing))
	for ln in bad[:5]:
		print("     bad: %r" % ln[:80])
	if not good:
		return good
	ids = sorted(good)
	print("   ids %d .. %d, distinct %d, strictly increasing in file: %s" % (ids[0], ids[-1], len(ids), order == sorted(order)))
	lens = [len(p) for p in good.values()]
	print("   payload length min %d, median %d, max %d" % (min(lens), sorted(lens)[len(lens) // 2], max(lens)))
	lower, upper, glyph, rest = bands(good.values())
	print("   " + band("a-z", lower))
	print("   " + band("A-Z", upper))
	print("   " + band("0-9,._-", glyph))
	print("   outside the three classes, top 12: %s" % rest[:12])
	return good


def main():
	ap = argparse.ArgumentParser()
	ap.add_argument("blob")
	ap.add_argument("--against")
	a = ap.parse_args()

	first = report(a.blob, open(a.blob, "rb").read())
	if not a.against:
		return
	second = report(a.against, open(a.against, "rb").read())

	shared = set(first) & set(second)
	print("\n== %s vs %s" % (a.blob, a.against))
	print("   ids only in the first  : %d" % len(set(first) - set(second)))
	print("   ids only in the second : %d" % len(set(second) - set(first)))
	print("   ids in both            : %d" % len(shared))
	if shared:
		same = sum(1 for i in shared if first[i] == second[i])
		print("   shared ids with byte-identical payload: %d / %d" % (same, len(shared)))
		for i in sorted(shared)[:5]:
			print("     %06d  %r" % (i, first[i][:48]))
			print("             %r" % (second[i][:48],))


if __name__ == "__main__":
	main()
