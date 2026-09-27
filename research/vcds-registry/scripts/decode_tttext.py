"""Decode every TTTEXT [TXT] record with srand(record id) and split on the comma separator.
OUT: out/tttext_plain.tsv  id \t full plaintext ; prints shape statistics."""
import sys, collections, re, json
sys.path.insert(0, __file__.rsplit('/', 1)[0])
from alphabet import decoder, load_records
src, out = sys.argv[1], sys.argv[2]
recs = load_records(src)
shapes = collections.Counter(); nf = collections.Counter(); f1 = collections.Counter(); f2len = collections.Counter()
bad = 0
with open(out, "w", encoding="utf-8") as fo:
    for rid in sorted(recs):
        p = decoder(rid)(recs[rid])
        fo.write(f"{rid}\t{p}\n")
        parts = p.split(",")
        nf[len(parts)] += 1
        if len(parts) >= 3:
            f1[parts[-2]] += 1
            f2len[len(parts[-1])] += 1
            shape = re.sub(r"\d", "9", parts[-2]) + "|" + re.sub(r"\d", "9", parts[-1])
            shapes[shape] += 1
print("records", len(recs))
print("fields by comma count", nf.most_common(10))
print("second-to-last field", f1.most_common(20))
print("last field len", f2len.most_common(10))
print("tail shapes", shapes.most_common(15))
